//! Remote control for the GitHub Desktop parity harness (`tools/parity`).
//!
//! `CORVENE_CONTROL=<port>` (build with `--features snapshots`) listens on
//! `127.0.0.1:<port>` for newline-delimited JSON commands and answers each
//! with one JSON line. Input is injected straight into the window's event
//! dispatch (hover, press, drag, scroll, keys, text) and frames are rendered
//! offscreen, so neither focus nor screen-recording rights are needed and the
//! user's own windows are left alone. GHD is driven the same way over the
//! Chrome DevTools Protocol; the harness compares the two captures.
//!
//! Commands (`{"cmd": …}`; coordinates are window points, top-left origin):
//! on Linux they are page points, below the menu bar, which GHD's captures
//! (CDP, the web contents only) leave out too: sizes, input and snapshots
//! all skip the bar's [`PAGE_TOP`] pixels.
//! - `ping` → `{w, h, scale}`
//! - `resize {w, h}`
//! - `move {x, y, pressed}`, `down {x, y}`, `up {x, y}`, `click {x, y}`
//!   (`button`: left|right|middle, `clicks`, `mods`: "cmd-shift" …)
//! - `drag {x, y, x2, y2, steps}`
//! - `scroll {x, y, dx, dy}` (pixel deltas, positive dy scrolls content down)
//! - `key {keys}` (GPUI keystroke syntax, space separated: "cmd-a escape")
//! - `type {text}`
//! - `action {name}` (a registered action, e.g. `corvene::OpenSettings`)
//! - `hook {name, arg}`: `complete-welcome`, `add-repo <path>`,
//!   `theme light|dark|high-contrast|system`, `popup <name>` (a
//!   `CORVENE_POPUP` name, opened now), `fake-host-accounts <json>` (GitLab,
//!   Gitea and Bitbucket accounts), `fake-accounts <json>` (signed-in
//!   accounts + their repository lists, `tools/parity/accounts.py`)
//! - `snap {path, cached}` → draws a fresh frame and saves it as PNG
//!   (`cached`: re-render only invalidated views, like a real frame)
//! - `menu` → the items of the last native menu (while the control socket is
//!   on, menus still pop up but are recorded and close themselves after
//!   `CORVENE_MENU_HOLD_MS`, default 1500); `menu-pick {label}` runs one
//! - `bench {steps, until, timeout_ms}` → `{input_ms, settle_ms, draw_ms,
//!   total_ms}`: runs `steps` (commands as above) back to back, waits until the
//!   `until` predicate holds on the app state ([`predicate`]), then draws one
//!   frame (only the views that were invalidated, as the display link would). `input_ms` is the synchronous handling of the steps, `settle_ms`
//!   the time from the first step to the predicate, `draw_ms` the frame
//!   (layout + paint on the main thread). Latency benchmarks (`tools/perf`).
//! - `frames {n}` → `{avg_ms, max_ms, cpu_ms, minstr}`: draws `n` frames of the
//!   current state, every view re-rendered; `scroll-frames {x, y, dx, dy, n}`
//!   scrolls by `dx` / `dy` before each frame and draws what the scroll
//!   invalidated
//! - `state` → the selected repository's selected file / commit, the first
//!   200 history SHAs, the changed-file count, the section and the open
//!   foldout
//! - `focus` → `{contexts}`: the focused element's key contexts, outermost
//!   first (which bindings a key reaches)
//! - `windows` → `{windows: [{index, workspace, selected, focused}]}`: the
//!   open windows (`429-multiple-windows`); every window command above takes
//!   an optional `window: <index>` (default: the focused window), and
//!   `activate-window {window}` makes one the focused window
//! - `quit`

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::Duration;

use corvene_core::Dispatcher;
use gpui_kit::*;
use serde_json::{Value, json};
use tracing::{error, info};

type Reply = mpsc::Sender<Value>;

/// Where GHD's page starts in the window: below Electron's menu bar on
/// Linux (`corvene_ui::menu_bar`), at the top on macOS and on Windows,
/// where the title bar with the app menu is part of GHD's page.
#[cfg(not(any(target_os = "macos", windows)))]
const PAGE_TOP: f32 = corvene_ui::menu_bar::HEIGHT;
#[cfg(any(target_os = "macos", windows))]
const PAGE_TOP: f32 = 0.;
/// `open_dev_popup` from `main.rs` (the `CORVENE_POPUP` names).
pub type PopupHook = fn(&str, &mut App);

/// Start the listener thread and the foreground command loop.
pub fn start(port: u16, popup: PopupHook, cx: &mut App) {
    let (tx, rx) = mpsc::channel::<(Value, Reply)>();
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listener,
        Err(err) => {
            error!(?err, port, "parity control: could not listen");
            return;
        }
    };
    info!(port, "parity control listening");
    // menus pop for real (screen captures compare them with GHD's) but are
    // recorded and close themselves, so this loop is only held for a moment
    corvene_ui::native_menu::set_auto_dismiss(Some(Duration::from_millis(
        std::env::var("CORVENE_MENU_HOLD_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1500),
    )));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let Ok(mut writer) = stream.try_clone() else {
                    return;
                };
                for line in BufReader::new(stream).lines() {
                    let Ok(line) = line else { break };
                    let reply = match serde_json::from_str::<Value>(&line) {
                        Ok(request) => {
                            let (reply_tx, reply_rx) = mpsc::channel();
                            if tx.send((request, reply_tx)).is_err() {
                                break;
                            }
                            reply_rx
                                .recv_timeout(Duration::from_secs(30))
                                .unwrap_or_else(|_| json!({"ok": false, "error": "timeout"}))
                        }
                        Err(err) => json!({"ok": false, "error": err.to_string()}),
                    };
                    if writeln!(writer, "{reply}").is_err() {
                        break;
                    }
                }
            });
        }
    });
    // A GPUI app only runs foreground work from its own executor: poll the
    // channel (a dev-only path, 4 ms is plenty).
    cx.spawn(async move |cx: &mut AsyncApp| {
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(4))
                .await;
            while let Ok((request, reply)) = rx.try_recv() {
                if request["cmd"] == "bench" {
                    bench(request, reply, popup, cx);
                    continue;
                }
                let quit = request["cmd"] == "quit";
                let result = cx.update(|cx| handle(&request, popup, cx));
                let _ = reply.send(match result {
                    Ok(mut value) => {
                        value["ok"] = json!(true);
                        value
                    }
                    Err(err) => json!({"ok": false, "error": err}),
                });
                if quit {
                    cx.update(|cx| cx.quit());
                    return;
                }
            }
        }
    })
    .detach();
}

fn handle(request: &Value, popup: PopupHook, cx: &mut App) -> Result<Value, String> {
    let cmd = request["cmd"].as_str().unwrap_or_default();
    match cmd {
        "quit" => return Ok(json!({})),
        "hook" => return hook(request, popup, cx),
        "state" => return Ok(state_summary(cx)),
        "windows" => return Ok(windows_summary(cx)),
        "activate-window" => {
            let entry = target_window(request, cx)?;
            Dispatcher::activate_workspace(entry.workspace, cx);
            return Ok(json!({}));
        }
        _ => {}
    }
    // `429-multiple-windows`: the command runs for the window's workspace
    let entry = target_window(request, cx)?;
    Dispatcher::enter_workspace(entry.workspace, cx);
    let result = entry
        .window
        .update(cx, |_, window, cx| window_command(cmd, request, window, cx))
        .map_err(|err| err.to_string());
    Dispatcher::leave_workspace(cx);
    result?
}

/// The window a command names (`window: <index>` into [`windows_summary`]),
/// else the focused one.
fn target_window(request: &Value, cx: &App) -> Result<corvene_ui::windows::WindowEntry, String> {
    match request["window"].as_u64() {
        Some(index) => corvene_ui::windows::entries(cx)
            .get(index as usize)
            .cloned()
            .ok_or_else(|| format!("no window {index}")),
        None => corvene_ui::windows::focused(cx).ok_or_else(|| "no window".to_string()),
    }
}

/// `windows`: the open windows in creation order.
fn windows_summary(cx: &App) -> Value {
    let state = corvene_core::AppState::global(cx).read(cx);
    let windows: Vec<Value> = corvene_ui::windows::entries(cx)
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let ws = state.workspace(entry.workspace);
            json!({
                "index": index,
                "workspace": entry.workspace.0,
                "selected": ws.and_then(|w| w.selected),
                "tabs": ws.map(|w| w.tabs.clone()).unwrap_or_default(),
                "focused": state.focused == entry.workspace,
            })
        })
        .collect();
    json!({"windows": windows})
}

fn window_command(
    cmd: &str,
    request: &Value,
    window: &mut Window,
    cx: &mut App,
) -> Result<Value, String> {
    let position = point(px(num(request, "x")), px(num(request, "y") + PAGE_TOP));
    let button = match request["button"].as_str() {
        Some("right") => MouseButton::Right,
        Some("middle") => MouseButton::Middle,
        _ => MouseButton::Left,
    };
    let clicks = request["clicks"].as_u64().unwrap_or(1) as usize;
    let modifiers = parse_modifiers(request["mods"].as_str().unwrap_or_default());
    let down = |position| {
        PlatformInput::MouseDown(MouseDownEvent {
            button,
            position,
            modifiers,
            click_count: clicks,
            first_mouse: false,
        })
    };
    let up = |position| {
        PlatformInput::MouseUp(MouseUpEvent {
            button,
            position,
            modifiers,
            click_count: clicks,
        })
    };
    let moved = |position, pressed_button| {
        PlatformInput::MouseMove(MouseMoveEvent {
            position,
            pressed_button,
            modifiers,
        })
    };
    match cmd {
        "ping" => {}
        "focus" => {
            let contexts: Vec<String> = window
                .context_stack()
                .iter()
                .map(|context| format!("{context:?}"))
                .collect();
            return Ok(json!({"contexts": contexts}));
        }
        "resize" => window.resize(size(
            px(num(request, "w")),
            px(num(request, "h") + PAGE_TOP),
        )),
        "move" => {
            // `pressed`: the button stays down (a drag, or moving off a
            // pressed button before releasing to cancel its click)
            let pressed = request["pressed"].as_bool().unwrap_or_default();
            window.dispatch_event(moved(position, pressed.then_some(button)), cx);
        }
        "down" => {
            window.dispatch_event(moved(position, None), cx);
            window.dispatch_event(down(position), cx);
        }
        "up" => {
            window.dispatch_event(up(position), cx);
        }
        "click" => {
            // a menu this click opens must not be confused with an older one
            if button == MouseButton::Right {
                corvene_ui::native_menu::clear_recorded();
            }
            // a recorded menu still inside its hold would swallow the click
            // (the macOS `NSMenu` holds this loop until it closes instead)
            #[cfg(not(target_os = "macos"))]
            corvene_ui::views_menu::close_all(cx);
            // hit testing uses the last drawn frame, and an unfocused window
            // gets none: without this a click right after a popup opened or
            // a list changed lands on what was there before
            window.draw(cx).clear(cx);
            window.dispatch_event(moved(position, None), cx);
            window.dispatch_event(down(position), cx);
            window.dispatch_event(up(position), cx);
        }
        "drag" => {
            let to = point(px(num(request, "x2")), px(num(request, "y2") + PAGE_TOP));
            let steps = request["steps"].as_u64().unwrap_or(10).max(1);
            // a frame after the press and every move, as with real input:
            // GPUI registers drag-move listeners when it paints an active
            // drag, so moves within one frame never reach them
            window.dispatch_event(moved(position, None), cx);
            window.dispatch_event(down(position), cx);
            window.draw(cx).clear(cx);
            for step in 1..=steps {
                let t = step as f32 / steps as f32;
                let at = point(
                    position.x + (to.x - position.x) * t,
                    position.y + (to.y - position.y) * t,
                );
                window.dispatch_event(moved(at, Some(button)), cx);
                window.draw(cx).clear(cx);
            }
            window.dispatch_event(up(to), cx);
        }
        "scroll" => {
            window.dispatch_event(moved(position, None), cx);
            window.dispatch_event(
                PlatformInput::ScrollWheel(ScrollWheelEvent {
                    position,
                    // GPUI: positive delta moves content towards the bottom
                    delta: ScrollDelta::Pixels(point(
                        px(-num(request, "dx")),
                        px(-num(request, "dy")),
                    )),
                    modifiers,
                    touch_phase: TouchPhase::Moved,
                }),
                cx,
            );
        }
        "key" => {
            for key in request["keys"]
                .as_str()
                .unwrap_or_default()
                .split_whitespace()
            {
                let keystroke = Keystroke::parse(key).map_err(|err| err.to_string())?;
                window.dispatch_keystroke(keystroke.clone(), cx);
                // the release too: a focused control's Space / Enter click
                // fires on key up
                window.dispatch_event(PlatformInput::KeyUp(KeyUpEvent { keystroke }), cx);
            }
        }
        "type" => {
            for ch in request["text"].as_str().unwrap_or_default().chars() {
                let key = match ch {
                    ' ' => "space".to_string(),
                    '-' => "-".to_string(),
                    c => c.to_string(),
                };
                let mut keystroke = Keystroke::parse(&key).map_err(|err| err.to_string())?;
                keystroke.key_char = Some(ch.to_string());
                if ch.is_uppercase() {
                    keystroke.modifiers.shift = true;
                }
                window.dispatch_keystroke(keystroke, cx);
            }
        }
        "action" => {
            let name = request["name"].as_str().unwrap_or_default();
            let action = cx
                .build_action(name, None)
                .map_err(|err| format!("{name}: {err}"))?;
            window.dispatch_action(action, cx);
        }
        "snap" => {
            let path = request["path"]
                .as_str()
                .ok_or_else(|| "snap needs a path".to_string())?;
            // draw now: an unfocused window gets no display-link frames, and
            // render_to_image uses the last drawn scene. `cached`: only what
            // invalidation re-renders, as a display-link frame would (shows
            // a cached view that failed to re-render)
            if !request["cached"].as_bool().unwrap_or_default() {
                window.refresh();
            }
            window.draw(cx).clear(cx);
            let mut image = window.render_to_image().map_err(|err| err.to_string())?;
            let top = (PAGE_TOP * window.scale_factor()).round() as u32;
            if top > 0 && top < image.height() {
                let (w, h) = (image.width(), image.height() - top);
                image = image::imageops::crop_imm(&image, 0, top, w, h).to_image();
            }
            image.save(path).map_err(|err| err.to_string())?;
            return Ok(json!({"w": image.width(), "h": image.height()}));
        }
        "frames" | "scroll-frames" => {
            let n = request["n"].as_u64().unwrap_or(20).max(1);
            let dy = num(request, "dy");
            let dx = num(request, "dx");
            let (mut total, mut max, mut cpu_total, mut instr_total) = (0.0f64, 0.0f64, 0.0, 0.0);
            for _ in 0..n {
                if cmd == "scroll-frames" {
                    window.dispatch_event(
                        PlatformInput::ScrollWheel(ScrollWheelEvent {
                            position,
                            delta: ScrollDelta::Pixels(point(px(-dx), px(-dy))),
                            modifiers,
                            touch_phase: TouchPhase::Moved,
                        }),
                        cx,
                    );
                }
                let (started, cpu, instr) =
                    (std::time::Instant::now(), thread_cpu_ms(), thread_minstr());
                // `frames`: everything re-rendered (the worst case, e.g. after
                // a focus change); `scroll-frames`: what the scroll invalidated
                if cmd == "frames" {
                    window.refresh();
                }
                window.draw(cx).clear(cx);
                let took = ms(started.elapsed());
                cpu_total += thread_cpu_ms() - cpu;
                instr_total += thread_minstr() - instr;
                total += took;
                max = max.max(took);
            }
            return Ok(json!({
                "avg_ms": total / n as f64,
                "max_ms": max,
                "cpu_ms": cpu_total / n as f64,
                "minstr": instr_total / n as f64,
            }));
        }
        // the last native menu the app tried to show (recorded headless)
        "menu" => return Ok(json!({"items": corvene_ui::native_menu::recorded_menu()})),
        "menu-pick" => {
            let label = request["label"].as_str().unwrap_or_default();
            if !corvene_ui::native_menu::pick_recorded(label, window, cx) {
                return Err(format!("no enabled menu item {label:?}"));
            }
        }
        other => return Err(format!("unknown command {other:?}")),
    }
    let viewport = window.viewport_size();
    Ok(json!({
        "w": f32::from(viewport.width),
        "h": f32::from(viewport.height) - PAGE_TOP,
        "scale": window.scale_factor(),
    }))
}

/// `bench`: see the module docs. The predicate is polled every 250 µs on the
/// foreground executor, so `settle_ms` is exact to about a quarter millisecond.
fn bench(request: Value, reply: Reply, popup: PopupHook, cx: &mut AsyncApp) {
    let until = request["until"].as_str().unwrap_or("idle").to_string();
    let timeout = Duration::from_millis(request["timeout_ms"].as_u64().unwrap_or(20_000));
    let (started, cpu, instr) = (std::time::Instant::now(), thread_cpu_ms(), thread_minstr());
    let steps = request["steps"].as_array().cloned().unwrap_or_default();
    for step in &steps {
        if let Err(err) = cx.update(|cx| handle(step, popup, cx)) {
            let _ = reply.send(json!({"ok": false, "error": err}));
            return;
        }
    }
    let input_ms = ms(started.elapsed());
    let input_cpu_ms = thread_cpu_ms() - cpu;
    let input_minstr = thread_minstr() - instr;
    cx.spawn(async move |cx: &mut AsyncApp| {
        loop {
            let done = cx.update(|cx| predicate(&until, cx));
            match done {
                Err(err) => {
                    let _ = reply.send(json!({"ok": false, "error": err}));
                    return;
                }
                Ok(true) => break,
                Ok(false) if started.elapsed() > timeout => {
                    let _ = reply.send(
                        json!({"ok": false, "error": format!("timeout waiting for {until}")}),
                    );
                    return;
                }
                Ok(false) => {
                    cx.background_executor()
                        .timer(Duration::from_micros(250))
                        .await
                }
            }
        }
        let settle_ms = ms(started.elapsed());
        let (draw_ms, draw_cpu_ms, draw_minstr) =
            cx.update(draw_frame).unwrap_or((-1.0, -1.0, f64::NAN));
        let _ = reply.send(json!({
            "ok": true,
            "input_ms": input_ms,
            "input_cpu_ms": input_cpu_ms,
            "settle_ms": settle_ms,
            "draw_ms": draw_ms,
            "draw_cpu_ms": draw_cpu_ms,
            "draw_minstr": draw_minstr,
            "input_minstr": input_minstr,
            "total_ms": ms(started.elapsed()),
        }));
    })
    .detach();
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// Instructions this thread has retired, in millions (Apple's
/// `thread_selfcounts`, no privileges needed): the one frame cost that stays
/// the same on a loaded machine, whichever core the thread runs on.
#[cfg(target_os = "macos")]
fn thread_minstr() -> f64 {
    unsafe extern "C" {
        fn thread_selfcounts(kind: libc::c_int, buf: *mut u64, nbytes: libc::size_t)
        -> libc::c_int;
    }
    let mut counts = [0u64; 2];
    // SAFETY: kind 1 (instructions, cycles) fills two u64s
    let ok = unsafe { thread_selfcounts(1, counts.as_mut_ptr(), std::mem::size_of_val(&counts)) };
    if ok == 0 {
        counts[0] as f64 / 1e6
    } else {
        f64::NAN
    }
}

#[cfg(not(target_os = "macos"))]
fn thread_minstr() -> f64 {
    f64::NAN
}

/// CPU time this thread has used, in ms: frame costs measured this way do
/// not grow when other processes preempt the app (a loaded machine).
#[cfg(windows)]
fn thread_cpu_ms() -> f64 {
    f64::NAN
}

/// CPU time this thread has used, in ms (`CLOCK_THREAD_CPUTIME_ID`).
#[cfg(not(windows))]
fn thread_cpu_ms() -> f64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime writes the timespec it is given
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    ts.tv_sec as f64 * 1000.0 + ts.tv_nsec as f64 / 1_000_000.0
}

/// Draw one fresh frame of the focused window; its duration in ms (wall
/// clock, main-thread CPU) and its main-thread instructions (millions).
fn draw_frame(cx: &mut App) -> Result<(f64, f64, f64), String> {
    let handle = corvene_ui::windows::focused(cx)
        .map(|e| e.window)
        .ok_or("no window")?;
    handle
        .update(cx, |_, window, cx| {
            let (started, cpu, instr) =
                (std::time::Instant::now(), thread_cpu_ms(), thread_minstr());
            // what the next display-link frame would draw: views the input
            // or the state change invalidated, cached views reused
            window.draw(cx).clear(cx);
            (
                ms(started.elapsed()),
                thread_cpu_ms() - cpu,
                thread_minstr() - instr,
            )
        })
        .map_err(|err| err.to_string())
}

/// `until` predicates on the selected repository's state:
/// - `idle`: nothing of it is loading (status, diff, history, commit)
/// - `repo:<path suffix>`: that repository is selected, its status loaded
/// - `diff:<path>`: `path` is the selected changed file and its diff is loaded
/// - `commits:<n>`: at least `n` history commits are loaded
/// - `commit:<sha prefix>`: that commit is selected, its files and (when a
///   file is selected) its diff loaded
/// - `branch:<name>`: `name` is checked out and the refresh after it is done
/// - `files:<n>`: the status lists `n` changed files (shown; a later refresh
///   may still be running)
/// - `frame`: true at once (the bench then only times the steps + a frame)
fn predicate(until: &str, cx: &mut App) -> Result<bool, String> {
    let (kind, arg) = until.split_once(':').unwrap_or((until, ""));
    if kind == "frame" {
        return Ok(true);
    }
    let state = corvene_core::AppState::global(cx).read(cx);
    let Some(id) = state.selected else {
        return Ok(false);
    };
    if kind == "repo" {
        let Some(repo) = state.repository(id) else {
            return Ok(false);
        };
        if !repo.path.to_string_lossy().ends_with(arg) {
            return Ok(false);
        }
    }
    let Some(rs) = state.repo_states.get(&id) else {
        return Ok(false);
    };
    let settled = !rs.loading && rs.status.is_some() && rs.info.is_some();
    Ok(match kind {
        "idle" => {
            settled
                && !rs.diff_loading
                && !rs.commits_loading
                && !rs.committing
                && (rs.selected_file.is_none() || rs.diff.is_some())
        }
        "repo" => settled,
        "diff" => rs.selected_file.as_deref() == Some(arg) && !rs.diff_loading && rs.diff.is_some(),
        "commits" => rs.commits.len() >= arg.parse().unwrap_or(1) && !rs.commits_loading,
        "commit" => {
            rs.selected_commit
                .as_deref()
                .is_some_and(|sha| sha.starts_with(arg))
                && rs.changeset.is_some()
                && (rs.commit_selected_file.is_none() || rs.commit_diff.is_some())
        }
        "branch" => {
            settled
                && rs
                    .info
                    .as_ref()
                    .and_then(|info| info.current_branch())
                    .is_some_and(|b| b.name == arg)
        }
        // what the list shows, even while another refresh runs
        "files" => {
            rs.info.is_some()
                && rs
                    .status
                    .as_deref()
                    .is_some_and(|st| st.files.len() == arg.parse::<usize>().unwrap_or(0))
        }
        // `1311-stacked-diff`: at least N files' diffs of the stack have loaded
        "stacked" => rs.stacked.entries.len() >= arg.parse().unwrap_or(1),
        other => return Err(format!("unknown predicate {other:?}")),
    })
}

/// `state`: the selected repository's selections (for benchmarks).
fn state_summary(cx: &mut App) -> Value {
    let state = corvene_core::AppState::global(cx).read(cx);
    let Some(rs) = state.selected.and_then(|id| state.repo_states.get(&id)) else {
        return json!({"windows": state.workspaces.len(), "workspace": state.current.0});
    };
    json!({
        // `429-multiple-windows`
        "windows": state.workspaces.len(),
        "workspace": state.current.0,
        "tabs": state.tabs,
        "selected_file": rs.selected_file,
        "selected_commit": rs.selected_commit,
        "commits": rs.commits.iter().take(200).map(|c| c.sha.clone()).collect::<Vec<_>>(),
        "files": rs.status.as_deref().map(|s| s.files.len()),
        // the "Committed … Undo" bar moves the commit form up
        "undo_bar": rs.last_commit.is_some(),
        "repository": state.selected_repository().map(|r| r.name()),
        // `527-multiple-accounts`: the stored login and the one in use
        "repository_account": state.selected_repository().and_then(|r| r.account.clone()),
        "account_in_use": state
            .selected
            .and_then(|id| state.account_for_repository(id))
            .map(|a| a.login.clone()),
        "popup": state.popup().map(|p| {
            let text = format!("{p:?}");
            text.split([' ', '{', '(']).next().unwrap_or_default().to_string()
        }),
        "section": format!("{:?}", rs.section),
        "foldout": state.foldout.map(|f| format!("{f:?}")),
        // `886-history-search`
        "history_filter": {
            "loading": rs.history_filter.loading,
            "searched": rs.history_filter.searched,
            "commits": rs.history_filter.commits.len(),
        },
    })
}

fn hook(request: &Value, popup: PopupHook, cx: &mut App) -> Result<Value, String> {
    let arg = request["arg"].as_str().unwrap_or_default();
    match request["name"].as_str().unwrap_or_default() {
        "complete-welcome" => Dispatcher::complete_welcome(cx),
        "add-repo" => Dispatcher::add_repository(std::path::PathBuf::from(arg), cx),
        "theme" => {
            let theme = match arg {
                "light" => corvene_core::ThemeSetting::Light,
                "high-contrast" => corvene_core::ThemeSetting::HighContrast,
                "system" => corvene_core::ThemeSetting::System,
                _ => corvene_core::ThemeSetting::Dark,
            };
            Dispatcher::update_settings(cx, |s| s.theme = theme);
        }
        "popup" => popup(arg, cx),
        // GHD's `focus` IPC: refresh the selected repository
        "refresh" => {
            if let Some(id) = corvene_core::AppState::global(cx).read(cx).selected {
                Dispatcher::refresh_repository(id, cx);
            }
        }
        "fake-accounts" => fake_accounts(arg, cx)?,
        // flags 342-344: GitLab / Gitea / Bitbucket accounts, no tokens
        "fake-host-accounts" => {
            let accounts: Vec<corvene_core::HostAccount> =
                serde_json::from_str(arg).map_err(|e| e.to_string())?;
            corvene_core::AppState::global(cx).update(cx, |s, cx| {
                s.hosts.accounts = accounts;
                cx.notify();
            });
        }
        // Settings › Git › Hooks › Load Git hook environment variables from
        // shell (GHD's `git-hooks-env-enabled`): hooks interception
        "hooks-env" => {
            let enabled = arg != "off";
            Dispatcher::update_settings(cx, |s| s.enable_git_hook_env = enabled);
            Dispatcher::refresh_hook_env(cx);
        }
        "fake-github" => fake_github(arg, cx)?,
        // switches the selected repository to branch `arg`
        // (`1114-hook-results`' post-checkout)
        "checkout" => {
            if let Some(id) = corvene_core::AppState::global(cx).read(cx).selected {
                // the changes come along (Switch's "Bring my changes")
                Dispatcher::checkout_branch(
                    id,
                    arg.to_string(),
                    Some(corvene_core::UncommittedChangesStrategy::MoveToNewBranch),
                    cx,
                );
            }
        }
        // `428-menu-bar-status-item`: what the status item shows (the model)
        // and whether the NSStatusItem is in the menu bar
        "menu-bar-model" => {
            let model = corvene_core::AppState::global(cx).read(cx).menu_bar_model();
            #[cfg(target_os = "macos")]
            let shown = corvene_ui::status_item::is_shown();
            #[cfg(not(target_os = "macos"))]
            let shown = false;
            return Ok(json!({"model": model, "shown": shown}));
        }
        // `menu-bar-watch <id>` / `menu-bar-unwatch <id>`: Settings › Advanced
        // › Menu bar without the dialog
        "menu-bar-watch" => {
            let id: u64 = arg
                .trim()
                .parse()
                .map_err(|e| format!("menu-bar-watch: {e}"))?;
            Dispatcher::set_menu_bar_repository(id, true, cx);
        }
        "menu-bar-unwatch" => {
            let id: u64 = arg
                .trim()
                .parse()
                .map_err(|e| format!("menu-bar-unwatch: {e}"))?;
            Dispatcher::set_menu_bar_repository(id, false, cx);
        }
        // `354-pull-request-event-notifications`: the notifications posted
        // (newest last) and each account's event source
        "pr-events" => {
            let s = corvene_core::AppState::global(cx).read(cx);
            let events = &s.alive.pull_request_events;
            return Ok(json!({"posted": events.recent, "sources": events.sources()}));
        }
        // ... and a poll of every account now
        "pr-events-poll" => Dispatcher::poll_pull_request_events_now(cx),
        // ... and a click on the newest one whose page contains `arg`
        "pr-events-click" => {
            let newest = corvene_core::AppState::global(cx)
                .read(cx)
                .alive
                .pull_request_events
                .recent
                .iter()
                .rev()
                .find(|p| p.notification.html_url.contains(arg))
                .map(|p| p.notification.clone())
                .ok_or("no such pull request event notification was posted")?;
            Dispatcher::open_pull_request_event(newest, cx);
        }
        other => return Err(format!("unknown hook {other:?}")),
    }
    Ok(json!({}))
}

/// `fake-github {port, owner, name, login[, remote]}` (`tools/parity/github_stub.py`):
/// an Enterprise account on the stub GitHub API at
/// `http://127.0.0.1:<port>/api/v3` with an injected token, and the
/// selected repository made the stub's `owner/name` GitHub repository, so
/// the real API client runs against the stub (`345-issues`,
/// `346-releases`). `{"permission": "admin" | "read"}` sets the
/// repository permission (write otherwise). `{"extra_logins": [..]}` signs
/// in more accounts on the stub (token `stub-token-<login>`,
/// `527-multiple-accounts`) and leaves the repository's account to be
/// looked up, as for a repository added with several accounts.
/// `{"parent": {"owner", "name", "clone_url"}}` makes it a fork of that
/// repository (its `default_branch` is `main`), and `{"fork_target":
/// "parent" | "self"}` then picks its Fork Behavior as Choose Fork Settings
/// does (`353-fork-tracks-upstream`).
fn fake_github(arg: &str, cx: &mut App) -> Result<(), String> {
    let fake: Value = serde_json::from_str(arg).map_err(|e| e.to_string())?;
    let owner = fake["owner"].as_str().unwrap_or("octocat").to_string();
    let name = fake["name"]
        .as_str()
        .unwrap_or("parity-fixture")
        .to_string();
    let login = fake["login"].as_str().unwrap_or("octocat").to_string();
    // `{"account": false}`: a GitHub.com repository with no account for it
    // (the signed-out state of the views)
    let with_account = fake["account"].as_bool().unwrap_or(true);
    let (web, endpoint) = match fake["port"].as_u64() {
        Some(port) if with_account => (
            format!("http://127.0.0.1:{port}"),
            format!("http://127.0.0.1:{port}/api/v3"),
        ),
        Some(_) | None if !with_account => (
            "https://github.com".to_string(),
            "https://api.github.com".to_string(),
        ),
        _ => return Err("fake-github: no port".to_string()),
    };
    let account = corvene_core::Account {
        endpoint: endpoint.clone(),
        id: 583231,
        login: login.clone(),
        name: Some("Mona Lisa Octocat".to_string()),
        avatar_url: None,
        emails: vec!["mona@example.com".to_string()],
        scopes: vec!["repo".to_string(), "workflow".to_string()],
        plan: Some("free".to_string()),
        private_primary_email: false,
        from_gh_cli: false,
    };
    if with_account {
        corvene_platform::keychain::inject_token(&account.host(), &login, "stub-token");
    }
    let github = corvene_core::GitHubRepository {
        endpoint: endpoint.clone(),
        owner: owner.clone(),
        name: name.clone(),
        html_url: format!("{web}/{owner}/{name}"),
        clone_url: format!("{web}/{owner}/{name}.git"),
        default_branch: Some("main".to_string()),
        private: false,
        fork: fake["parent"].is_object(),
        parent: fake["parent"].as_object().map(|p| {
            let (owner, name) = (
                p["owner"].as_str().unwrap_or("upstream").to_string(),
                p["name"].as_str().unwrap_or(&name).to_string(),
            );
            Box::new(corvene_core::GitHubRepository {
                endpoint: endpoint.clone(),
                html_url: format!("{web}/{owner}/{name}"),
                clone_url: p["clone_url"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("{web}/{owner}/{name}.git")),
                owner,
                name,
                default_branch: Some("main".to_string()),
                private: false,
                fork: false,
                parent: None,
                archived: false,
                permissions: Some(corvene_core::RepositoryPermission::Read),
                allow_forking: Some(true),
                node_id: None,
            })
        }),
        archived: false,
        // `{"permission": "admin"}`: `1113-lfs-locks`' Force Unlock
        permissions: Some(match fake["permission"].as_str() {
            Some("admin") => corvene_core::RepositoryPermission::Admin,
            Some("read") => corvene_core::RepositoryPermission::Read,
            _ => corvene_core::RepositoryPermission::Write,
        }),
        allow_forking: Some(true),
        node_id: Some("R_stub".to_string()),
    };
    // `remote`: `origin` becomes the stub repository's clone URL, so the
    // current branch's pushed tip is a ref on it (`334-branch-ci-status`,
    // `41-ci-checks.yaml`)
    let point_origin = fake["remote"].as_bool().unwrap_or(false);
    let clone_url = github.clone_url.clone();
    let extra: Vec<corvene_core::Account> = fake["extra_logins"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|_| with_account)
        .enumerate()
        .map(|(ix, extra_login)| {
            corvene_platform::keychain::inject_token(
                &account.host(),
                extra_login,
                &format!("stub-token-{extra_login}"),
            );
            corvene_core::Account {
                id: 583232 + ix as u64,
                login: extra_login.to_string(),
                name: Some(format!("{extra_login} at work")),
                emails: vec![format!("{extra_login}@example.com")],
                ..account.clone()
            }
        })
        .collect();
    let lookup = !extra.is_empty();
    let (selected, retarget) = corvene_core::AppState::global(cx).update(cx, |s, cx| {
        s.accounts.retain(|a| a.endpoint != endpoint);
        if with_account {
            s.accounts.push(account);
            s.accounts.extend(extra);
        }
        let mut retarget = None;
        if let Some(id) = s.selected
            && let Some(repo) = s.repositories.iter_mut().find(|r| r.id == id)
        {
            repo.github = Some(github);
            // `527-multiple-accounts`: looked up again below
            repo.account = None;
            s.account_probes.probed.remove(&id);
            if point_origin && let Some(git) = s.git.clone() {
                retarget = Some((git, repo.path.clone()));
            }
        }
        cx.notify();
        (s.selected, retarget)
    });
    if let Some((git, path)) = retarget {
        corvene_git::remote_ops::set_remote_url(git, &path, "origin", &clone_url)
            .map_err(|e| format!("fake-github: set-url: {e}"))?;
        if let Some(id) = selected {
            Dispatcher::refresh_repository(id, cx);
        }
    }
    let fork_target = match fake["fork_target"].as_str() {
        Some("parent") => Some(corvene_core::ForkContributionTarget::Parent),
        Some("self") => Some(corvene_core::ForkContributionTarget::Own),
        _ => None,
    };
    if let (Some(target), Some(id)) = (fork_target, selected) {
        Dispatcher::set_fork_contribution_target(id, target, cx);
    }
    // `348-pull-request-review`: the stub's pull requests fill the cache
    // (Branch › Review Pull Request… needs the branch's pull request)
    if with_account && let Some(id) = corvene_core::AppState::global(cx).read(cx).selected {
        Dispatcher::refresh_pull_requests(id, true, cx);
        // `1314-code-owners`: the owners that are the new account's
        Dispatcher::refresh_code_owners(id, cx);
        if lookup {
            Dispatcher::resolve_repository_account(id, cx);
        }
    }
    Ok(())
}

/// `fake-accounts {accounts, repositories}` (`tools/parity/accounts.py`):
/// signed-in accounts with their repository lists, without tokens or API
/// calls (the lists are already there, so nothing is fetched).
fn fake_accounts(arg: &str, cx: &mut App) -> Result<(), String> {
    let fake: Value = serde_json::from_str(arg).map_err(|e| e.to_string())?;
    let accounts: Vec<corvene_core::Account> =
        serde_json::from_value(fake["accounts"].clone()).map_err(|e| e.to_string())?;
    let repositories: std::collections::HashMap<String, Vec<corvene_core::GitHubRepository>> =
        serde_json::from_value(fake["repositories"].clone()).map_err(|e| e.to_string())?;
    // lists keyed by endpoint belong to its first account (`Account::key`)
    let repositories = repositories
        .into_iter()
        .map(|(key, repos)| {
            let key = match accounts.iter().find(|a| a.endpoint == key) {
                Some(account) if !key.contains('|') => account.key(),
                _ => key,
            };
            (key, repos)
        })
        .collect();
    corvene_core::AppState::global(cx).update(cx, |s, cx| {
        s.accounts = accounts;
        s.api_repositories = repositories;
        s.api_repositories_loading.clear();
        cx.notify();
    });
    Ok(())
}

fn num(request: &Value, key: &str) -> f32 {
    request[key].as_f64().unwrap_or_default() as f32
}

fn parse_modifiers(spec: &str) -> Modifiers {
    let mut modifiers = Modifiers::default();
    for part in spec.split(['-', '+']) {
        match part {
            "cmd" | "meta" => modifiers.platform = true,
            "shift" => modifiers.shift = true,
            "alt" | "option" => modifiers.alt = true,
            "ctrl" | "control" => modifiers.control = true,
            _ => {}
        }
    }
    modifiers
}
