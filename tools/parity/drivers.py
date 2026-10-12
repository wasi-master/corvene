"""Drivers for the two apps under comparison.

GitHub Desktop (Electron) is driven over the Chrome DevTools Protocol: a
private instance runs with its own `--user-data-dir` (the user's real GHD and
its settings are never touched) and `--remote-debugging-port`. Input goes
through `Input.dispatch*Event`, which reaches the renderer without window
focus, and `Page.captureScreenshot` renders the page offscreen.

Corvene is driven through its `CORVENE_CONTROL` socket (crates/corvene/src/
parity_control.rs, `--features snapshots`), which injects the same input into
GPUI's event dispatch and renders frames offscreen.

Both expose the same interface: move / down / up / click / drag / scroll /
key / type / menu / popup / snap.
"""

from __future__ import annotations

import base64
import json
import os
import shlex
import signal
import socket
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

import websocket

IS_MAC = sys.platform == "darwin"
IS_WIN = sys.platform == "win32"


def _windows_ghd() -> str:
    """The newest `app-<version>\\GitHubDesktop.exe` of the per-user install
    (the `GitHubDesktop.exe` next to those folders is Squirrel's stub, which
    starts the app as a process of its own and returns)."""
    root = Path(os.environ.get("LOCALAPPDATA", "")) / "GitHubDesktop"

    def version(app: Path) -> tuple:
        return tuple(int(part) if part.isdigit() else 0 for part in app.name[len("app-"):].split("."))

    apps = sorted(root.glob("app-*"), key=version)
    return str((apps[-1] if apps else root) / "GitHubDesktop.exe")


# PARITY_GHD_APP overrides; on Linux, a GitHub Desktop 3.6.6 build (`yarn
# build:prod` → dist/desktop-linux-x64/desktop) or a packaged github-desktop
GHD_APP = Path(
    os.environ.get("PARITY_GHD_APP")
    or (
        "/Applications/GitHub Desktop.app/Contents/MacOS/GitHub Desktop"
        if IS_MAC
        else _windows_ghd()
        if IS_WIN
        else "/usr/bin/github-desktop"
    )
)
# PARITY_OFFLINE=1: both apps without network (an unreachable proxy), so
# avatars, emoji and API calls fail the same way in both
OFFLINE = os.environ.get("PARITY_OFFLINE") == "1"
DEAD_PROXY = "http://127.0.0.1:9"

# PARITY_FOREGROUND=1: let both apps activate and show their windows as
# they normally do. By default (macOS) they launch in the background with
# their windows transparent and parked off screen: input and captures need
# neither focus nor a visible window, and the user's typing stays put.
BACKGROUND = IS_MAC and os.environ.get("PARITY_FOREGROUND") != "1"

# Runs in GHD's main process before its own code (`--inspect-brk`): showing a
# window never activates the app (`show` activates on macOS, `showInactive`
# does not), focusing is a no-op, and a shown window goes transparent, lets
# clicks through and moves off the left edge (AppKit keeps a sliver on screen).
GHD_BACKGROUND_PATCH = """(() => {
  const { app, BrowserWindow } = require('electron')
  BrowserWindow.prototype.show = function () {
    this.showInactive()
    this.setOpacity(0)
    this.setIgnoreMouseEvents(true)
    this.setPosition(-30000, 0)
  }
  BrowserWindow.prototype.focus = function () {}
  app.focus = function () {}
})()"""

# Retina on the Macs the harness grew up on; X11 under Xvfb is 1x
DEFAULT_SCALE = 2.0 if IS_MAC else 1.0


# Scenarios place fixed points and rectangles on GHD's macOS page, which
# starts with its 32 pt title bar (#desktop-app-title-bar); off macOS the
# page has none (Electron's menu bar sits outside it). The page off macOS is
# the macOS page's content below the title bar (scenario height minus 32),
# so elements anchored to the top and to the bottom alike sit 32 higher.
# Windows: GHD's page has a title bar of its own there, 28 px
# (`--win32-title-bar-height`), so its content sits 4 higher than on macOS.
TITLE_BAR = 0.0 if IS_MAC else 4.0 if IS_WIN else 32.0


def _spawn(command: list, log, env: dict) -> subprocess.Popen:
    """Start an app detached from the harness's terminal."""
    if IS_WIN:
        return subprocess.Popen(
            command, stdout=log, stderr=log, env=env, creationflags=subprocess.CREATE_NEW_PROCESS_GROUP
        )
    return subprocess.Popen(command, stdout=log, stderr=log, env=env, start_new_session=True)


def _terminate(proc: subprocess.Popen, wait: float) -> None:
    """End an app and the processes it started: politely, then for good."""
    if IS_WIN:
        # no signals on Windows: taskkill asks the windows to close, /F ends
        # the tree
        subprocess.run(["taskkill", "/T", "/PID", str(proc.pid)], capture_output=True)
        try:
            proc.wait(wait)
        except subprocess.TimeoutExpired:
            subprocess.run(["taskkill", "/T", "/F", "/PID", str(proc.pid)], capture_output=True)
        return
    os.killpg(proc.pid, signal.SIGTERM)
    try:
        proc.wait(wait)
    except subprocess.TimeoutExpired:
        os.killpg(proc.pid, signal.SIGKILL)


def park_pointer():
    """Off macOS, move the real X pointer to the screen's bottom-right corner.

    Both apps are driven by synthetic input, but the window manager centres
    their windows under the real pointer (Xvfb starts it mid-screen), and
    Chromium hovers whatever is under it while Corvene does not."""
    if IS_MAC or not os.environ.get("DISPLAY"):
        return
    try:
        out = subprocess.run(["xdotool", "getdisplaygeometry"], capture_output=True, text=True, check=True)
        w, h = (int(v) for v in out.stdout.split())
        subprocess.run(["xdotool", "mousemove", str(w - 1), str(h - 1)], check=True)
    except (OSError, ValueError, subprocess.CalledProcessError):
        print("note: xdotool missing; the X pointer may hover GHD content", file=sys.stderr)


def page_height(height: int) -> int:
    """The page height that holds a scenario's macOS content area."""
    return int(height - TITLE_BAR)


def page_point(x: float, y: float) -> tuple[float, float]:
    """A scenario's macOS page point on this platform's page."""
    return float(x), max(0.0, float(y) - TITLE_BAR)


def page_rect(rect):
    """A scenario's macOS `[x, y, w, h]` on this platform's page (the part
    over the title bar is dropped)."""
    if not rect or not TITLE_BAR:
        return rect
    x, y, w, h = rect
    top = y - TITLE_BAR
    return [x, max(0.0, top), w, h + min(0.0, top)]


def platform_keys(spec: str) -> str:
    """Scenario chords say `cmd` for GHD's CmdOrCtrl: Ctrl off macOS."""
    if IS_MAC or not spec:
        return spec
    return " ".join(
        "-".join("ctrl" if part in ("cmd", "meta") else part for part in chord.split("-"))
        if chord not in ("-",) else chord
        for chord in spec.split(" ")
    )

# GHD menu-event names (app/src/main-process/menu/menu-event.ts) → Corvene
# actions (crates/corvene-ui/src/actions.rs). GHD menu accelerators live in the
# main process and never see CDP key events, so shortcuts are replayed as the
# menu events they trigger.
MENU_ACTIONS = {
    "show-changes": "corvene::ShowChanges",
    "show-history": "corvene::ShowHistory",
    "choose-repository": "corvene::ShowRepositoryList",
    "show-branches": "corvene::ShowBranchesList",
    "show-worktrees": "corvene::ShowWorktreesList",
    "create-worktree": "corvene::NewWorktree",
    "go-to-commit-message": "corvene::GoToSummary",
    "show-stashed-changes": "corvene::ToggleStashedChanges",
    "hide-stashed-changes": "corvene::ToggleStashedChanges",
    "toggle-changes-filter": "corvene::ToggleChangesFilter",
    "show-preferences": "corvene::OpenSettings",
    "show-about": "corvene::About",
    "add-local-repository": "corvene::AddLocalRepository",
    "create-repository": "corvene::NewRepository",
    "clone-repository": "corvene::CloneRepository",
    "create-branch": "corvene::NewBranch",
    "rename-branch": "corvene::RenameBranch",
    "delete-branch": "corvene::DeleteBranch",
    "discard-all-changes": "corvene::DiscardAllChanges",
    "stash-all-changes": "corvene::StashAllChanges",
    "update-branch-with-contribution-target-branch": "corvene::UpdateFromDefaultBranch",
    "compare-to-branch": "corvene::CompareToBranch",
    "merge-branch": "corvene::MergeIntoCurrentBranch",
    "squash-and-merge-branch": "corvene::SquashAndMergeIntoCurrentBranch",
    "rebase-branch": "corvene::RebaseCurrentBranch",
    "show-repository-settings": "corvene::RepositorySettings",
    "remove-repository": "corvene::RemoveRepository",
    "push": "corvene::Push",
    "pull": "corvene::Pull",
    "fetch": "corvene::Fetch",
    "preview-pull-request": "corvene::PreviewPullRequest",
    "open-pull-request": "corvene::CreatePullRequest",
    "find-text": "corvene::Find",
    "select-all": "corvene::SelectAll",
    "increase-active-resizable-width": "corvene::ExpandActiveResizable",
    "decrease-active-resizable-width": "corvene::ContractActiveResizable",
}

# GPUI key name → (DOM key, DOM code, Windows virtual key code, mac editing command)
_KEYS = {
    "enter": ("Enter", "Enter", 13, "insertNewline"),
    "escape": ("Escape", "Escape", 27, None),
    "tab": ("Tab", "Tab", 9, None),
    "backspace": ("Backspace", "Backspace", 8, "deleteBackward"),
    "delete": ("Delete", "Delete", 46, "deleteForward"),
    "space": (" ", "Space", 32, None),
    "up": ("ArrowUp", "ArrowUp", 38, "moveUp"),
    "down": ("ArrowDown", "ArrowDown", 40, "moveDown"),
    "left": ("ArrowLeft", "ArrowLeft", 37, "moveLeft"),
    "right": ("ArrowRight", "ArrowRight", 39, "moveRight"),
    "home": ("Home", "Home", 36, "moveToBeginningOfLine"),
    "end": ("End", "End", 35, "moveToEndOfLine"),
    "pageup": ("PageUp", "PageUp", 33, "scrollPageUp"),
    "pagedown": ("PageDown", "PageDown", 34, "scrollPageDown"),
    "f2": ("F2", "F2", 113, None),
}
_CMD_COMMANDS = {"a": "selectAll", "c": "copy", "v": "paste", "x": "cut", "z": "undo"}


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def parse_mods(spec: str) -> dict:
    spec = platform_keys(spec)
    parts = set(spec.replace("+", "-").split("-")) if spec else set()
    return {
        "cmd": bool(parts & {"cmd", "meta"}),
        "shift": "shift" in parts,
        "alt": bool(parts & {"alt", "option"}),
        "ctrl": bool(parts & {"ctrl", "control"}),
    }


def _cdp_mods(m: dict) -> int:
    return (1 if m["alt"] else 0) | (2 if m["ctrl"] else 0) | (4 if m["cmd"] else 0) | (8 if m["shift"] else 0)


class Ghd:
    """A private GitHub Desktop instance under CDP control."""

    name = "ghd"

    def __init__(self, profile: Path, log: Path, env: dict | None = None):
        self.profile = profile
        self.port = free_port()
        self.log = log
        # scenario `ghd_env`, e.g. GITHUB_DESKTOP_PREVIEW_FEATURES=1 for the
        # `test-*` popups (enableTestMenuItems)
        self.env = env or {}
        self.proc: subprocess.Popen | None = None
        self.ws = None
        self._id = 0
        self.scale = DEFAULT_SCALE

    # -- process -----------------------------------------------------------
    def start(self, timeout: float = 30):
        self.profile.mkdir(parents=True, exist_ok=True)
        inspect_port = free_port() if BACKGROUND else None
        with open(self.log, "ab") as log:
            self.proc = _spawn(
                [
                    str(GHD_APP),
                    # the main process waits for `_background` to patch it
                    *([f"--inspect-brk={inspect_port}"] if inspect_port else []),
                    f"--remote-debugging-port={self.port}",
                    f"--user-data-dir={self.profile}",
                    # captures in sRGB, like Corvene's render_to_image; without it
                    # Chromium converts to the display profile (#1d2125 → #16191c)
                    "--force-color-profile=srgb",
                    # Chromium refuses to run as root with its sandbox
                    *(["--no-sandbox"] if not IS_MAC and not IS_WIN and os.geteuid() == 0 else []),
                    # extra switches, e.g. `--proxy-server=127.0.0.1:9` to keep
                    # GHD offline where its network would fail differently
                    # from Corvene's (an intercepting proxy Chromium does not
                    # trust opens an "Untrusted server" dialog)
                    *shlex.split(os.environ.get("PARITY_GHD_ARGS", "")),
                    *([f"--proxy-server={DEAD_PROXY}"] if OFFLINE else []),
                ],
                log,
                {**os.environ, **{k: str(v) for k, v in self.env.items()}},
            )
        if inspect_port:
            self._background(inspect_port, timeout)
        deadline = time.time() + timeout
        while time.time() < deadline:
            try:
                tabs = json.load(urllib.request.urlopen(f"http://127.0.0.1:{self.port}/json", timeout=1))
                pages = [t for t in tabs if t.get("type") == "page" and "index.html" in t.get("url", "")]
                if pages:
                    self.ws = websocket.create_connection(pages[0]["webSocketDebuggerUrl"], suppress_origin=True, timeout=60)
                    break
            except OSError:
                pass
            time.sleep(0.25)
        else:
            raise RuntimeError("GitHub Desktop did not expose a DevTools page")
        self.wait_for("document.readyState === 'complete' && !!document.querySelector('#desktop-app-container, #desktop-app')", timeout)

    def _background(self, port: int, timeout: float):
        """Apply GHD_BACKGROUND_PATCH to the main process, paused on its first
        line by `--inspect-brk`, then let it run."""
        deadline = time.time() + timeout
        while True:
            try:
                targets = json.load(urllib.request.urlopen(f"http://127.0.0.1:{port}/json", timeout=1))
                if targets:
                    break
            except OSError:
                pass
            if time.time() > deadline:
                raise RuntimeError("GitHub Desktop's main process did not open its inspector")
            time.sleep(0.1)
        ws = websocket.create_connection(targets[0]["webSocketDebuggerUrl"], suppress_origin=True, timeout=timeout)
        try:
            ws.send(json.dumps({"id": 1, "method": "Debugger.enable"}))
            ws.send(json.dumps({"id": 2, "method": "Runtime.runIfWaitingForDebugger"}))
            while True:
                msg = json.loads(ws.recv())
                if msg.get("method") == "Debugger.paused":
                    frame = msg["params"]["callFrames"][0]["callFrameId"]
                    break
            # `require` only exists in the module's frame, not the global scope
            ws.send(json.dumps({"id": 3, "method": "Debugger.evaluateOnCallFrame",
                                "params": {"callFrameId": frame, "expression": GHD_BACKGROUND_PATCH}}))
            while True:
                msg = json.loads(ws.recv())
                if msg.get("id") == 3:
                    if "exceptionDetails" in msg.get("result", {}) or "error" in msg:
                        print(f"note: GHD background patch failed: {msg}", file=sys.stderr)
                    break
            ws.send(json.dumps({"id": 4, "method": "Debugger.resume"}))
            ws.recv()
        finally:
            ws.close()

    def stop(self):
        if self.ws:
            try:
                self.ws.close()
            except Exception:
                pass
            self.ws = None
        if self.proc and self.proc.poll() is None:
            # Chromium shuts down cleanly on SIGTERM (IndexedDB / localStorage flushed)
            _terminate(self.proc, 8)
        self.proc = None

    # -- protocol ----------------------------------------------------------
    def call(self, method: str, **params):
        self._id += 1
        self.ws.send(json.dumps({"id": self._id, "method": method, "params": params}))
        while True:
            msg = json.loads(self.ws.recv())
            if msg.get("id") == self._id:
                if "error" in msg:
                    raise RuntimeError(f"{method}: {msg['error']}")
                return msg.get("result", {})

    def eval(self, expression: str):
        # An evaluation that lands while the page is being replaced (a
        # reload, which GHD on Windows also does on its own after a theme
        # change) is dropped without an answer: ask again rather than wait
        # out the socket.
        patient = self.ws.gettimeout()
        for attempt in range(4):
            self.ws.settimeout(min(patient or 15, 15))
            try:
                r = self.call("Runtime.evaluate", expression=expression, returnByValue=True, awaitPromise=True)
                break
            except websocket.WebSocketTimeoutException:
                if attempt == 3:
                    raise
            finally:
                self.ws.settimeout(patient)
        if "exceptionDetails" in r:
            raise RuntimeError(f"eval failed: {r['exceptionDetails'].get('exception', {}).get('description', r['exceptionDetails'])}")
        return r.get("result", {}).get("value")

    def wait_for(self, expression: str, timeout: float = 15, interval: float = 0.15) -> bool:
        deadline = time.time() + timeout
        # an evaluation sent while the page reloads is never answered (its
        # execution context is gone): give up on it after a moment and ask
        # again, instead of waiting out the socket's own timeout
        patient = self.ws.gettimeout()
        self.ws.settimeout(3)
        try:
            while time.time() < deadline:
                try:
                    if self.eval(f"!!({expression})"):
                        return True
                except (RuntimeError, websocket.WebSocketTimeoutException):
                    pass
                time.sleep(interval)
            return False
        finally:
            self.ws.settimeout(patient)

    def emit(self, channel: str, payload) -> None:
        """Deliver an IPC message to the renderer as if the main process sent it."""
        self.eval(f"require('electron').ipcRenderer.emit({json.dumps(channel)}, {{}}, {json.dumps(payload)})")

    # -- setup -------------------------------------------------------------
    def configure(self, width: int, height: int, scale: float, local_storage: dict, freeze: bool):
        """Viewport, focus emulation and localStorage (then reload so GHD reads it)."""
        self.scale = scale
        if local_storage:
            items = ";".join(f"localStorage.setItem({json.dumps(k)}, {json.dumps(str(v))})" for k, v in local_storage.items())
            if IS_WIN:
                # GHD's page stops answering for good when it is reloaded
                # while the app is still starting up
                time.sleep(4)
            # the mark tells the page being replaced from the reloaded one:
            # the old one is still there, and complete, for a moment
            self.eval(items + ";window.__parityStale = true")
            self.call("Page.reload", ignoreCache=False)
            time.sleep(0.5)
            self.wait_for(
                "!window.__parityStale && document.readyState === 'complete'"
                " && !!document.querySelector('#desktop-app-container, #desktop-app')",
                30,
            )
        self.resize(width, height)
        self.hook_context_menus()
        self.call("Emulation.setFocusEmulationEnabled", enabled=True)
        self.emit("focus", None)
        if freeze:
            # transitions finish instantly so a capture never lands mid-animation
            self.eval(
                "(()=>{const s=document.createElement('style');s.id='parity-freeze';"
                "s.textContent='*,*::before,*::after{transition-duration:0s!important;transition-delay:0s!important;"
                "animation-duration:0s!important;animation-delay:0s!important}';document.head.appendChild(s)})()"
            )

    def resize(self, width: int, height: int):
        """Resize the real window (hidden title bar: window size == content size).

        Electron has no `Browser.setWindowBounds`, but `window.resizeTo` on the
        main frame resizes the BrowserWindow. When the screen is too small for
        it, the viewport is emulated at the requested size instead."""
        # the frame around the page (Electron's menu bar on Linux, nothing
        # with macOS's hidden title bar)
        chrome = self.eval("[outerWidth - innerWidth, outerHeight - innerHeight]") or [0, 0]
        self.eval(f"window.resizeTo({width + chrome[0]}, {height + chrome[1]})")
        deadline = time.time() + 3
        while time.time() < deadline:
            if self.eval(f"innerWidth === {width} && innerHeight === {height}"):
                self.call("Emulation.clearDeviceMetricsOverride")
                return
            time.sleep(0.1)
        self.call("Emulation.setDeviceMetricsOverride", width=width, height=height, deviceScaleFactor=self.scale, mobile=False)

    def hook_context_menus(self):
        """Record `show-contextual-menu` IPC calls instead of letting the main
        process pop a native menu; the promise stays pending until
        `pick_menu` / `dismiss_menu` (GHD then runs the chosen action)."""
        self.eval(
            "(()=>{if(window.__parityMenuHook)return;window.__parityMenuHook=true;"
            "const {ipcRenderer}=require('electron');const orig=ipcRenderer.invoke.bind(ipcRenderer);"
            "window.__parityMenuPop=false;"
            "ipcRenderer.invoke=(ch,...a)=>{if(ch==='show-contextual-menu'){window.__parityMenu=a[0];"
            "if(window.__parityMenuPop)return orig(ch,...a);"
            "return new Promise(r=>{window.__parityMenuResolve=r;});}return orig(ch,...a);};})()"
        )

    def set_menu_pop(self, pop: bool):
        """Let GHD pop its real menus (visual passes; something on screen has
        to dismiss them) instead of only recording them."""
        self.eval(f"window.__parityMenuPop={'true' if pop else 'false'}")

    def menu_items(self) -> list[str]:
        items = self.eval("window.__parityMenu || null") or []

        def walk(items, depth, out):
            for it in items:
                pad = "  " * depth
                if it.get("type") == "separator":
                    out.append(pad + "-")
                    continue
                line = pad + (it.get("label") or "")
                if it.get("enabled") is False:
                    line += " [disabled]"
                if it.get("checked"):
                    line += " [x]"
                out.append(line)
                if it.get("submenu"):
                    walk(it["submenu"], depth + 1, out)
        out: list[str] = []
        walk(items, 0, out)
        return out

    def pick_menu(self, label: str):
        found = self.eval(
            "(()=>{const find=(items,path)=>{for(let i=0;i<items.length;i++){const it=items[i];"
            # scenarios name macOS labels; GHD's Linux ones are sentence case
            "const L=%s;if((it.label===L||(it.label||'').toLowerCase()===L.toLowerCase())&&it.enabled!==false)return path.concat(i);"
            "if(it.submenu){const r=find(it.submenu,path.concat(i));if(r)return r;}}return null;};"
            "const p=find(window.__parityMenu||[],[]);if(p&&window.__parityMenuResolve){window.__parityMenuResolve(p);"
            "window.__parityMenu=null;}return p;})()" % json.dumps(label)
        )
        if not found:
            raise LookupError(f"GHD menu has no enabled item {label!r}")

    def dismiss_menu(self):
        self.eval("(()=>{if(window.__parityMenuResolve)window.__parityMenuResolve(null);window.__parityMenu=null;})()")

    def add_repository(self, path: Path) -> bool:
        """`cli-action open-repository` → Add Repository dialog → submit."""
        self.emit("cli-action", {"kind": "open-repository", "path": str(path)})
        if not self.wait_for("document.querySelector('dialog button[type=submit]')", 10):
            return False
        time.sleep(0.3)
        self.eval("document.querySelector('dialog button[type=submit]').click()")
        return self.wait_for(f"!document.querySelector('dialog') && document.body.innerText.includes({json.dumps(path.name)})", 15)

    # -- geometry ----------------------------------------------------------
    def resolve(self, target) -> tuple[float, float]:
        """`[x, y]`, `{css: sel}` or `{text: label}` (+ `offset`) → window point."""
        if isinstance(target, (list, tuple)):
            return page_point(target[0], target[1])
        if "css" in target:
            js = f"document.querySelector({json.dumps(target['css'])})"
        elif "contains" in target:
            js = (
                "(()=>{const t=%s;const root=document.querySelector(%s)||document.body;"
                "const all=[...root.querySelectorAll('*')].filter(e=>{"
                "const r=e.getBoundingClientRect();return r.width>0&&r.height>0&&e.textContent.includes(t)});"
                "return all.find(e=>![...e.children].some(c=>c.textContent.includes(t)))||null})()"
            ) % (json.dumps(target["contains"]), json.dumps(target.get("within", "body")))
        elif "text" in target:
            # innermost visible element whose own text matches (optionally inside `within`)
            js = (
                "(()=>{const t=%s;const root=document.querySelector(%s)||document.body;"
                "const all=[...root.querySelectorAll('*')].filter(e=>{"
                "const r=e.getBoundingClientRect();return r.width>0&&r.height>0&&e.textContent.trim()===t});"
                "return all.find(e=>![...e.children].some(c=>c.textContent.trim()===t))||null})()"
            ) % (json.dumps(target["text"]), json.dumps(target.get("within", "body")))
        else:
            raise ValueError(f"bad target {target}")
        rect = self._rect(js)
        if not rect and ("text" in target or "contains" in target):
            # scenarios name macOS labels; GHD's Linux ones are sentence case
            key = "text" if "text" in target else "contains"
            lowered = js.replace(json.dumps(target[key]), json.dumps(target[key].lower()), 1)
            lowered = lowered.replace("e.textContent.trim()===t", "e.textContent.trim().toLowerCase()===t")
            lowered = lowered.replace("c.textContent.trim()===t", "c.textContent.trim().toLowerCase()===t")
            lowered = lowered.replace("e.textContent.includes(t)", "e.textContent.toLowerCase().includes(t)")
            lowered = lowered.replace("c.textContent.includes(t)", "c.textContent.toLowerCase().includes(t)")
            rect = self._rect(lowered)
        if not rect:
            raise LookupError(f"GHD element not found: {target}")
        ox, oy = target.get("offset", [0, 0])
        ax, ay = target.get("anchor", [0.5, 0.5])
        return rect[0] + rect[2] * ax + ox, rect[1] + rect[3] * ay + oy

    def _rect(self, js: str):
        return self.eval(f"(()=>{{const e={js};if(!e)return null;const r=e.getBoundingClientRect();return [r.x,r.y,r.width,r.height]}})()")

    def describe(self, x: float, y: float) -> str:
        """CSS path of the element at a point (for diff regions)."""
        return self.eval(
            "(()=>{let e=document.elementFromPoint(%f,%f);const out=[];while(e&&e!==document.body&&out.length<5){"
            "let s=e.tagName.toLowerCase();if(e.id)s+='#'+e.id;const c=[...e.classList].slice(0,3).join('.');if(c)s+='.'+c;"
            "out.unshift(s);e=e.parentElement}return out.join(' > ')})()" % (x, y)
        ) or ""

    def dump(self, root: str = "body") -> list:
        """Visible elements under `root` with their box and the styles that
        decide how they look (a spec to implement against)."""
        js = r"""
(() => {
  const props = ['color','backgroundColor','fontSize','fontWeight','lineHeight','fontFamily',
    'paddingTop','paddingRight','paddingBottom','paddingLeft','marginTop','marginRight','marginBottom','marginLeft',
    'borderTopWidth','borderTopColor','borderRightWidth','borderBottomWidth','borderBottomColor','borderLeftWidth',
    'borderRadius','boxShadow','opacity','zoom','textAlign','letterSpacing'];
  const root = document.querySelector(%s) || document.body;
  const out = [];
  const walk = (e, depth) => {
    const r = e.getBoundingClientRect();
    const cs = getComputedStyle(e);
    if (r.width > 0 && r.height > 0 && cs.visibility !== 'hidden' && cs.display !== 'none') {
      const own = [...e.childNodes].filter(n => n.nodeType === 3).map(n => n.textContent.trim()).join(' ').trim();
      const st = {};
      for (const p of props) {
        const v = cs[p];
        if (v && !['0px','none','normal','auto','rgba(0, 0, 0, 0)','1','start'].includes(v)) st[p] = v;
      }
      out.push({depth, tag: e.tagName.toLowerCase(), id: e.id || undefined,
        cls: [...e.classList].join(' ') || undefined, text: own.slice(0, 80) || undefined,
        rect: [r.x, r.y, r.width, r.height].map(v => Math.round(v * 100) / 100), style: st});
    }
    for (const c of e.children) walk(c, depth + 1);
  };
  walk(root, 0);
  return out;
})()""" % json.dumps(root)
        return self.eval(js) or []

    # -- input -------------------------------------------------------------
    def _mouse(self, kind, x, y, button="left", clicks=1, mods="", buttons=0):
        self.call(
            "Input.dispatchMouseEvent",
            type=kind,
            x=x,
            y=y,
            button=button if kind != "mouseMoved" else ("left" if buttons else "none"),
            buttons=buttons,
            clickCount=clicks,
            modifiers=_cdp_mods(parse_mods(mods)),
        )

    def move(self, x, y, mods="", pressed=False):
        self._mouse("mouseMoved", x, y, mods=mods, buttons=1 if pressed else 0)

    def down(self, x, y, button="left", clicks=1, mods=""):
        self._mouse("mouseMoved", x, y, mods=mods)
        for n in range(1, clicks + 1):
            self._mouse("mousePressed", x, y, button, n, mods, buttons=1)
            if n < clicks:
                self._mouse("mouseReleased", x, y, button, n, mods)

    def up(self, x, y, button="left", clicks=1, mods=""):
        self._mouse("mouseReleased", x, y, button, clicks, mods)

    def click(self, x, y, button="left", clicks=1, mods=""):
        self.down(x, y, button, clicks, mods)
        self.up(x, y, button, clicks, mods)

    def drag(self, x, y, x2, y2, steps=10):
        self._mouse("mouseMoved", x, y)
        self._mouse("mousePressed", x, y, buttons=1)
        for i in range(1, steps + 1):
            t = i / steps
            self._mouse("mouseMoved", x + (x2 - x) * t, y + (y2 - y) * t, buttons=1)
            time.sleep(0.01)
        self._mouse("mouseReleased", x2, y2)

    def scroll(self, x, y, dx, dy):
        self.call("Input.dispatchMouseEvent", type="mouseWheel", x=x, y=y, deltaX=dx, deltaY=dy)

    def key(self, keys: str):
        for chord in platform_keys(keys).split():
            parts = chord.split("-")
            base = parts[-1] if parts[-1] != "" else "-"
            m = parse_mods("-".join(parts[:-1]))
            params = {"modifiers": _cdp_mods(m)}
            if base in _KEYS:
                k, code, vk, command = _KEYS[base]
                params.update(key=k, code=code, windowsVirtualKeyCode=vk)
                # editing commands are AppKit's; Chromium elsewhere acts on the key
                if command and not m["cmd"] and IS_MAC:
                    params["commands"] = [command]
                if base == "enter":
                    params["text"] = "\r"
                if base == "space":
                    params["text"] = " "
            else:
                ch = base.upper() if m["shift"] and len(base) == 1 else base
                vk = ord(base.upper()) if len(base) == 1 else 0
                code = f"Key{base.upper()}" if base.isalpha() and len(base) == 1 else (f"Digit{base}" if base.isdigit() else "")
                params.update(key=ch, code=code, windowsVirtualKeyCode=vk)
                if m["cmd"] and base in _CMD_COMMANDS and IS_MAC:
                    params["commands"] = [_CMD_COMMANDS[base] if not (base == "z" and m["shift"]) else "redo"]
                elif not (m["cmd"] or m["ctrl"]):
                    params["text"] = ch
            self.call("Input.dispatchKeyEvent", type="keyDown" if "text" not in params else "keyDown", **params)
            up = {k: v for k, v in params.items() if k not in ("text", "commands")}
            self.call("Input.dispatchKeyEvent", type="keyUp", **up)

    def type(self, text: str):
        self.call("Input.insertText", text=text)

    def menu(self, event: str):
        self.emit("menu-event", event)

    def popup(self, name: str):
        """GHD's own UI test hooks are menu events (`test-*`)."""
        self.emit("menu-event", name)

    def snap(self, path: Path):
        r = self.call("Page.captureScreenshot", format="png", fromSurface=True)
        path.write_bytes(base64.b64decode(r["data"]))


class Corvene:
    """A Corvene instance with an isolated data dir under `CORVENE_CONTROL`."""

    name = "corvene"

    def __init__(self, binary: Path, data_dir: Path, log: Path, theme: str, flags: str | None = None):
        self.binary = binary
        self.data_dir = data_dir
        self.log = log
        self.theme = theme
        # a scenario's `corvene_flags`, after the preset
        self.flags = flags
        # a scenario's `corvene_env`, set on every start
        self.env: dict = {}
        self.port = free_port()
        self.proc: subprocess.Popen | None = None
        self.sock = None
        self.file = None
        self.scale = DEFAULT_SCALE

    def start(self, timeout: float = 20, extra_env: dict | None = None):
        self.data_dir.mkdir(parents=True, exist_ok=True)
        env = {k: v for k, v in os.environ.items() if not k.startswith("CORVENE_")}
        env.update(
            CORVENE_DATA_DIR=str(self.data_dir),
            CORVENE_CONTROL=str(self.port),
            CORVENE_THEME=self.theme,
            CORVENE_LOG=env.get("PARITY_CORVENE_LOG", "info"),
            # Settings › Integrations' SSH key (flag 350) reads this, not ~/.ssh
            CORVENE_SSH_DIR=str(self.data_dir / "ssh"),
            # every flag at its GHD value, so the diff measures true parity
            # (.docs/flags.md); PARITY_CORVENE_FLAGS overrides
            CORVENE_FLAGS=",".join(
                f for f in (env.get("PARITY_CORVENE_FLAGS", "preset=github-desktop"), self.flags) if f
            ),
        )
        if not BACKGROUND:
            env["CORVENE_FOREGROUND"] = "1"
        if not IS_MAC and not IS_WIN:
            # the avatar and emoji caches live under XDG_CACHE_HOME, not the
            # data dir: a private one keeps earlier runs' downloads out
            env["XDG_CACHE_HOME"] = str(self.data_dir / "cache")
        if OFFLINE:
            for key in ("HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"):
                env[key] = DEAD_PROXY
            env["NO_PROXY"] = env["no_proxy"] = "localhost,127.0.0.1"
        env.update(self.env)
        env.update(extra_env or {})
        with open(self.log, "ab") as log:
            self.proc = _spawn([str(self.binary)], log, env)
        deadline = time.time() + timeout
        while time.time() < deadline:
            try:
                self.sock = socket.create_connection(("127.0.0.1", self.port), timeout=60)
                self.file = self.sock.makefile("rw")
                info = self.cmd("ping")
                self.scale = info.get("scale", DEFAULT_SCALE)
                # CORVENE_THEME only overrides the look; store the setting too,
                # as GHD's fixture does (Settings › Appearance shows it)
                self.hook("theme", self.theme)
                return
            except OSError:
                if self.proc.poll() is not None:
                    raise RuntimeError(f"corvene exited early (see {self.log})")
                time.sleep(0.2)
        raise RuntimeError("corvene control socket did not come up")

    def stop(self):
        if self.file:
            try:
                self.cmd("quit")
            except Exception:
                pass
        if self.sock:
            self.sock.close()
        self.sock = self.file = None
        if self.proc:
            try:
                self.proc.wait(5)
            except subprocess.TimeoutExpired:
                self.proc.kill()
        self.proc = None

    def cmd(self, cmd: str, **args) -> dict:
        self.file.write(json.dumps({"cmd": cmd, **args}) + "\n")
        self.file.flush()
        line = self.file.readline()
        if not line:
            raise RuntimeError("corvene closed the control socket")
        reply = json.loads(line)
        if not reply.get("ok"):
            raise RuntimeError(f"corvene {cmd}: {reply.get('error')}")
        return reply

    def hook(self, name: str, arg: str = ""):
        return self.cmd("hook", name=name, arg=arg)

    def resize(self, w, h):
        self.cmd("resize", w=w, h=h)
        deadline = time.time() + 3
        while time.time() < deadline:
            info = self.cmd("ping")
            if int(info["w"]) == w and int(info["h"]) == h:
                return info
            time.sleep(0.1)
        return self.cmd("ping")

    def move(self, x, y, mods="", pressed=False):
        self.cmd("move", x=x, y=y, mods=platform_keys(mods), pressed=pressed)

    def down(self, x, y, button="left", clicks=1, mods=""):
        self.cmd("down", x=x, y=y, button=button, clicks=clicks, mods=platform_keys(mods))

    def up(self, x, y, button="left", clicks=1, mods=""):
        self.cmd("up", x=x, y=y, button=button, clicks=clicks, mods=platform_keys(mods))

    def click(self, x, y, button="left", clicks=1, mods=""):
        for n in range(1, clicks + 1):
            self.cmd("click", x=x, y=y, button=button, clicks=n, mods=platform_keys(mods))

    def drag(self, x, y, x2, y2, steps=10):
        self.cmd("drag", x=x, y=y, x2=x2, y2=y2, steps=steps)

    def scroll(self, x, y, dx, dy):
        self.cmd("scroll", x=x, y=y, dx=dx, dy=dy)

    def key(self, keys: str):
        self.cmd("key", keys=platform_keys(keys))

    def type(self, text: str):
        self.cmd("type", text=text)

    def menu(self, event: str):
        action = MENU_ACTIONS.get(event)
        if not action:
            raise LookupError(f"no Corvene action mapped for GHD menu event {event!r} (drivers.MENU_ACTIONS)")
        self.cmd("action", name=action)

    def popup(self, name: str):
        self.hook("popup", name)

    def menu_items(self) -> list[str]:
        return self.cmd("menu").get("items", [])

    def pick_menu(self, label: str):
        self.cmd("menu-pick", label=label)

    def dismiss_menu(self):
        pass

    def snap(self, path: Path, window: int | None = None):
        if window is None:
            self.cmd("snap", path=str(path))
        else:
            self.cmd("snap", path=str(path), window=window)

    # `429-multiple-windows`: the open windows and which one later commands target
    def windows(self) -> list[dict]:
        return self.cmd("windows").get("windows", [])

    def activate_window(self, index: int):
        self.cmd("activate-window", window=index)
