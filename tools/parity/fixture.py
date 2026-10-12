"""Deterministic fixture repository shown by both apps.

Fixed author, committer and dates make every copy byte-identical (same SHAs),
so GHD and Corvene each get their own copy (no index.lock races, destructive
scenarios stay possible) that renders the same text.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import zipfile
from pathlib import Path

NAME = "parity-fixture"
AUTHOR = ("Parity Bot", "parity@example.com")

_COMMITS = [
    ("2026-08-03T09:12:00+00:00", "Initial commit", "", {
        "README.md": "# Parity fixture\n\nA small repository for GitHub Desktop parity runs.\n",
        ".gitignore": "target/\n*.log\n",
    }),
    ("2026-08-05T14:40:00+00:00", "Add the command line entry point", "Parses arguments and prints a greeting.", {
        "src/main.rs": 'fn main() {\n    let name = std::env::args().nth(1).unwrap_or_else(|| "world".into());\n    println!("Hello, {name}!");\n}\n',
        "Cargo.toml": '[package]\nname = "parity-fixture"\nversion = "0.1.0"\nedition = "2024"\n',
    }),
    ("2026-08-11T10:05:00+00:00", "Write the user guide", "", {
        "docs/guide.md": "# Guide\n\n1. Build with `cargo build`.\n2. Run `parity-fixture <name>`.\n\nThat is all.\n",
        "docs/old.md": "Outdated notes that will be deleted.\n",
    }),
    ("2026-08-19T16:30:00+00:00", "Support a --shout flag", "Upper-cases the greeting.\n\nCloses #4.", {
        "src/main.rs": 'fn main() {\n    let args: Vec<String> = std::env::args().skip(1).collect();\n    let shout = args.iter().any(|a| a == "--shout");\n    let name = args.iter().find(|a| !a.starts_with("--")).cloned().unwrap_or_else(|| "world".into());\n    let line = format!("Hello, {name}!");\n    println!("{}", if shout { line.to_uppercase() } else { line });\n}\n',
    }),
    ("2026-09-02T08:00:00+00:00", "Add a logo", "", {
        "assets/logo.svg": '<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><circle cx="8" cy="8" r="7" fill="#2da44e"/></svg>\n',
    }),
    ("2026-09-14T11:45:00+00:00", "Explain flags in the README", "", {
        "README.md": "# Parity fixture\n\nA small repository for GitHub Desktop parity runs.\n\n## Flags\n\n- `--shout`: upper-case the greeting\n",
    }),
]

_WORKING_CHANGES = {
    "README.md": "# Parity fixture\n\nA small repository for GitHub Desktop parity runs.\n\n## Flags\n\n- `--shout`: upper-case the greeting\n- `--quiet`: print nothing\n\n## License\n\nMIT\n",
    "notes.txt": "Remember to update the changelog.\n",
    "src/lib.rs": "pub fn greet(name: &str) -> String {\n    format!(\"Hello, {name}!\")\n}\n",
    # one changed word (intra-line highlight) next to a line long enough to wrap
    "src/main.rs": 'fn main() {\n    let args: Vec<String> = std::env::args().skip(1).collect();\n    let shout = args.iter().any(|a| a == "--loud");\n    let name = args.iter().find(|a| !a.starts_with("--")).cloned().unwrap_or_else(|| std::env::var("USER").unwrap_or_else(|_| "world".into())).trim().to_string(); // falls back to $USER, then to "world"\n    let line = format!("Hello, {name}!");\n    println!("{}", if shout { line.to_uppercase() } else { line });\n}\n',
}
_DELETED = ["docs/old.md"]

# `repo-codeowners` (`1314-code-owners`): owners for every file, the
# sources (one of them the stub's signed-in octocat), the docs and README
_CODEOWNERS = """# Owners of the parity fixture
*            @octo-org/maintainers
/src/        @octocat @octo-org/rust
docs/        @mona
README.md    @hubot docs@example.com
notes.txt
"""

# `repo-coauthors`: commits by several people (`AvatarStack`), the newest
# first in History: six people (the stack's "more" sliver), four, three
# (a different committer), two
_PEOPLE = [
    ("Mona Lisa", "mona@example.com"),
    ("Hubot", "hubot@example.com"),
    ("Octo Cat", "octocat@example.com"),
    ("Jane Doe", "jane@example.com"),
    ("John Roe", "john@example.com"),
]
_CO_AUTHORED = [
    ("2026-09-20T09:00:00+00:00", "Pair on the greeting", 1, None),
    ("2026-09-21T09:00:00+00:00", "Apply the review", 1, _PEOPLE[2]),
    ("2026-09-22T09:00:00+00:00", "Mob on the parser", 3, None),
    ("2026-09-23T09:00:00+00:00", "Team cleanup", 5, None),
]


def _git(repo: Path, *args: str, date: str | None = None, committer: tuple[str, str] = AUTHOR):
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    env.update(
        GIT_AUTHOR_NAME=AUTHOR[0], GIT_AUTHOR_EMAIL=AUTHOR[1],
        GIT_COMMITTER_NAME=committer[0], GIT_COMMITTER_EMAIL=committer[1],
        GIT_CONFIG_NOSYSTEM="1",
    )
    if date:
        env.update(GIT_AUTHOR_DATE=date, GIT_COMMITTER_DATE=date)
    subprocess.run(["git", *args], cwd=repo, env=env, check=True, capture_output=True)


def remove_tree(path: Path) -> None:
    """`shutil.rmtree` that also removes read-only files: git's objects are,
    and Windows refuses to delete those."""
    import os
    import stat
    import sys

    def writable(function, target, _):
        os.chmod(target, stat.S_IWRITE)
        function(target)

    if sys.version_info >= (3, 12):
        shutil.rmtree(path, onexc=writable)
    else:
        shutil.rmtree(path, onerror=writable)


def _signed(parent: Path, repo: Path) -> None:
    """`1214-commit-signatures`: signed commits on top, newest first a
    tampered (bad) SSH signature, an SSH key the allowed signers file does
    not list, a GPG key no keyring here has (when gpg can make one) and a
    good SSH signature. The keys live in `<parent>/parity-signing`; the
    repository's own `gpg.ssh.allowedSignersFile` lists the good one, so
    no environment is needed to verify."""
    keys = parent / "parity-signing"
    if keys.exists():
        remove_tree(keys)
    keys.mkdir(parents=True)
    for name in ("good", "unknown"):
        subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", AUTHOR[1], "-f", str(keys / name)],
                       check=True, capture_output=True)
    allowed = keys / "allowed_signers"
    allowed.write_text(f"{AUTHOR[1]} {(keys / 'good.pub').read_text()}")
    _git(repo, "config", "gpg.ssh.allowedSignersFile", str(allowed))

    def commit(summary: str, date: str, *sign: str):
        (repo / "SIGNED.md").write_text(f"{summary}\n")
        _git(repo, "add", "-A")
        _git(repo, *sign, "commit", "-q", "-S", "-m", summary, date=date)

    def ssh(key: str) -> tuple[str, ...]:
        return ("-c", "gpg.format=ssh", "-c", f"user.signingkey={keys / key}.pub")

    commit("Sign with a known SSH key", "2026-09-25T09:00:00+00:00", *ssh("good"))
    # a short GNUPGHOME: gpg-agent's socket path has a length limit
    import tempfile
    home = Path(tempfile.mkdtemp(prefix="pg"))
    home.chmod(0o700)
    env = {**os.environ, "GNUPGHOME": str(home)}
    made = shutil.which("gpg") and subprocess.run(
        ["gpg", "--batch", "--pinentry-mode", "loopback", "--passphrase", "", "--quick-gen-key",
         f"{AUTHOR[0]} <{AUTHOR[1]}>", "ed25519", "sign", "never"], env=env, capture_output=True).returncode == 0
    if made:
        os.environ["GNUPGHOME"], saved = str(home), os.environ.get("GNUPGHOME")
        try:
            commit("Sign with a GPG key nobody has", "2026-09-25T10:00:00+00:00",
                   "-c", "gpg.format=openpgp", "-c", f"user.signingkey={AUTHOR[1]}")
        finally:
            if saved is None:
                del os.environ["GNUPGHOME"]
            else:
                os.environ["GNUPGHOME"] = saved
        subprocess.run(["gpgconf", "--kill", "gpg-agent"], env=env, capture_output=True)
    remove_tree(home)
    commit("Sign with an SSH key nobody listed", "2026-09-25T11:00:00+00:00", *ssh("unknown"))
    commit("Sign and then change the message", "2026-09-25T12:00:00+00:00", *ssh("good"))
    raw = subprocess.run(["git", "cat-file", "commit", "HEAD"], cwd=repo, check=True, capture_output=True).stdout
    tampered = raw.replace(b"\n\nSign and then change", b"\n\nSigned and then changed", 1)
    oid = subprocess.run(["git", "hash-object", "-t", "commit", "-w", "--stdin"], cwd=repo, input=tampered,
                         check=True, capture_output=True).stdout.decode().strip()
    _git(repo, "update-ref", "HEAD", oid)


_PATCH = """diff --git a/docs/guide.md b/docs/guide.md
--- a/docs/guide.md
+++ b/docs/guide.md
@@ -3,4 +3,5 @@
 1. Build with `cargo build`.
 2. Run `parity-fixture <name>`.
+3. Pass `--shout` to shout.
 
 That is all.
diff --git a/CONTRIBUTING.md b/CONTRIBUTING.md
new file mode 100644
--- /dev/null
+++ b/CONTRIBUTING.md
@@ -0,0 +1,3 @@
+# Contributing
+
+Open a pull request.
"""


def _tools(parent: Path, repo: Path) -> None:
    """Ignored files in `repo`, and beside it `parity-fixture.patch` (a plain
    diff) and `parity-fixture.mbox` (two `git format-patch` commits made on a
    branch that is deleted again)."""
    (repo / "build.log").write_text("build output\n")
    (repo / "target" / "debug").mkdir(parents=True, exist_ok=True)
    (repo / "target" / "debug" / "parity-fixture").write_text("binary\n")
    (parent / f"{NAME}.patch").write_text(_PATCH)
    date = "2026-09-30T10:00:00+00:00"
    _git(repo, "checkout", "-q", "-b", "changelog", date=date)
    (repo / "CHANGELOG.md").write_text("# Changelog\n")
    _git(repo, "add", "CHANGELOG.md", date=date)
    _git(repo, "commit", "-q", "-m", "Add a changelog", date=date)
    (repo / "CHANGELOG.md").write_text("# Changelog\n\n- 0.1.0\n")
    _git(repo, "commit", "-q", "-am", "Note 0.1.0 in the changelog", date=date)
    mbox = subprocess.run(["git", "format-patch", "-2", "--stdout"], cwd=repo, check=True,
                          capture_output=True, text=True).stdout
    (parent / f"{NAME}.mbox").write_text(mbox)
    _git(repo, "checkout", "-q", "main", date=date)
    _git(repo, "branch", "-q", "-D", "changelog", date=date)


# `repo-pull-request`: the stub GitHub API's pull request #7, `feature/login`
# into `main`, two commits (the second one "not pushed yet" as far as the stub
# is concerned: its pull request head is the first); the review threads in
# `github_stub.py` name lines of these files. The stub names the bare
# `origin` as the pull request's repository, so the branch matches it.
_PULL_REQUEST_COMMITS = [
    ("2026-09-16T09:00:00+00:00", "Add a --login flag", {
        "src/main.rs": 'fn main() {\n'
                       '    let args: Vec<String> = std::env::args().skip(1).collect();\n'
                       '    let shout = args.iter().any(|a| a == "--shout");\n'
                       '    let login = args.iter().any(|a| a == "--login");\n'
                       '    let name = args.iter().find(|a| !a.starts_with("--")).cloned();\n'
                       '    let name = name.unwrap_or_else(|| "world".into());\n'
                       '    let greeting = if login {\n'
                       '        format!("Welcome back, {name}!")\n'
                       '    } else {\n'
                       '        format!("Hello, {name}!")\n'
                       '    };\n'
                       '    if shout {\n'
                       '        println!("{}", greeting.to_uppercase());\n'
                       '    } else {\n'
                       '        println!("{greeting}");\n'
                       '    }\n'
                       '}\n',
        "README.md": "# Parity fixture\n\nA small repository for GitHub Desktop parity runs.\n\n## Flags\n\n"
                     "- `--shout`: upper-case the greeting\n- `--login`: greet a returning user\n",
    }),
    ("2026-09-17T10:00:00+00:00", "Document the greeter", {
        "src/main.rs": '//! The parity fixture\'s greeter.\n'
                       'fn main() {\n'
                       '    let args: Vec<String> = std::env::args().skip(1).collect();\n'
                       '    let shout = args.iter().any(|a| a == "--shout");\n'
                       '    let login = args.iter().any(|a| a == "--login");\n'
                       '    let name = args.iter().find(|a| !a.starts_with("--")).cloned();\n'
                       '    let name = name.unwrap_or_else(|| "world".into());\n'
                       '    let greeting = if login {\n'
                       '        format!("Welcome back, {name}!")\n'
                       '    } else {\n'
                       '        format!("Hello, {name}!")\n'
                       '    };\n'
                       '    if shout {\n'
                       '        println!("{}", greeting.to_uppercase());\n'
                       '    } else {\n'
                       '        println!("{greeting}");\n'
                       '    }\n'
                       '}\n',
        "docs/guide.md": "# Guide\n\n1. Build with `cargo build`.\n2. Run `parity-fixture <name>`.\n"
                         "3. Add `--login` to greet a returning user.\n\nThat is all.\n",
    }),
]
def _structure(parent: Path, repo: Path, lfs_url: str | None) -> None:
    """Submodules and Git LFS files in `repo` (flags 1111-1113).

    `vendor/lib` is checked out one commit past the one recorded,
    `vendor/theme` was deinitialized (its `.git/modules` stays, so Update
    needs no clone from a local path, which git refuses in submodules) and
    `tools/scripts` is up to date. `art/*.psd` are LFS files; `lfs_url` (the
    `lfs_stub.py` server) is their LFS server, where `art/hero.psd` is
    locked by Mona Lisa and `art/logo.psd` by the user. Both are changed in
    the working tree."""
    date = "2026-09-26T10:00:00+00:00"
    sources = {"vendor/lib": "parity-lib", "vendor/theme": "parity-theme", "tools/scripts": "parity-scripts"}
    for name in sources.values():
        source = parent / name
        if source.exists():
            remove_tree(source)
        source.mkdir(parents=True)
        _git(source, "init", "-q", "-b", "main")
        _git(source, "config", "commit.gpgsign", "false")
        (source / "README.md").write_text(f"# {name}\n")
        _git(source, "add", "-A", date=date)
        _git(source, "commit", "-q", "-m", f"Start {name}", date=date)
    for path, name in sources.items():
        _git(repo, "-c", "protocol.file.allow=always", "submodule", "add", "-q", str((parent / name).resolve()), path, date=date)
    _git(repo, "commit", "-q", "-m", "Add submodules", date=date)
    # vendor/lib: a newer commit checked out
    lib = parent / sources["vendor/lib"]
    (lib / "lib.txt").write_text("more\n")
    _git(lib, "add", "-A", date=date)
    _git(lib, "commit", "-q", "-m", "Grow the library", date=date)
    _git(repo / "vendor" / "lib", "pull", "-q", "origin", "main", date=date)
    # vendor/theme: deinitialized
    _git(repo, "submodule", "deinit", "-q", "-f", "vendor/theme", date=date)
    # Git LFS
    _git(repo, "lfs", "install", "--local", date=date)
    (repo / ".gitattributes").write_text("*.psd filter=lfs diff=lfs merge=lfs -text\n")
    (repo / "art").mkdir(exist_ok=True)
    for name in ("hero", "logo", "banner"):
        (repo / "art" / f"{name}.psd").write_text(f"{name} artwork v1\n")
    _git(repo, "add", ".gitattributes", "art", date=date)
    _git(repo, "commit", "-q", "-m", "Add artwork", date=date)
    if lfs_url:
        _git(repo, "config", "lfs.url", lfs_url)
    for name in ("hero", "logo"):
        (repo / "art" / f"{name}.psd").write_text(f"{name} artwork v2\n")


def _remotes(parent: Path, repo: Path) -> None:
    """A second bare remote, `fork`, with all of `main`, and more tags: an
    annotated `v0.2.0` with a message and a lightweight `nightly`."""
    fork = parent / f"{NAME}-fork.git"
    if fork.exists():
        remove_tree(fork)
    _git(repo, "init", "-q", "--bare", str(fork))
    _git(repo, "remote", "add", "fork", str(fork))
    _git(repo, "push", "-q", "fork", "main:refs/heads/main")
    _git(repo, "fetch", "-q", "fork")
    _git(repo, "tag", "-a", "-m", "Second release", "v0.2.0", "HEAD~2",
         date="2026-09-26T12:00:00+00:00")
    _git(repo, "tag", "nightly", "HEAD")


# `repo-forks` (`1223-checkout-from-fork`): two forks of the fixture beside
# it, not added as remotes. The stub API serves them as alice's and bob's.
FORKS = {"alice": ("fix/typo", "feature/dark-mode"), "bob": ("experiment",)}


def fork_path(parent: Path, owner: str) -> Path:
    return parent / f"{NAME}-{owner}.git"


def _forks(parent: Path, repo: Path) -> None:
    """`FORKS`: bare repositories with all of `main` and each owner's
    branches, one commit each on top of `main`."""
    for owner, branches in FORKS.items():
        bare = fork_path(parent, owner)
        if bare.exists():
            remove_tree(bare)
        _git(repo, "init", "-q", "--bare", str(bare))
        _git(repo, "push", "-q", str(bare), "main:refs/heads/main")
        for branch in branches:
            _git(repo, "checkout", "-q", "-b", f"tmp-{owner}", "main")
            (repo / "FORK.md").write_text(f"{owner}'s {branch}\n")
            _git(repo, "add", "FORK.md")
            _git(repo, "commit", "-q", "-m", f"Work on {branch}", date="2026-09-27T12:00:00+00:00",
                 committer=(owner.title(), f"{owner}@example.com"))
            _git(repo, "push", "-q", str(bare), f"HEAD:refs/heads/{branch}")
            _git(repo, "checkout", "-q", "main")
            _git(repo, "branch", "-q", "-D", f"tmp-{owner}")


# `repo-utf16`: an MQL5 expert saved the way MetaEditor saves it, UTF-16LE
# with a byte order mark and CRLF line endings (git calls it binary;
# `1306-utf16-diffs` diffs its text), committed and then edited
_UTF16_PATH = "Experts/Greeter.mq5"
_UTF16_COMMITTED = """//+------------------------------------------------------------------+
//|                                                      Greeter.mq5 |
//+------------------------------------------------------------------+
#property copyright "Parity Bot"
#property version   "1.00"
#property strict

input int    InpPeriod = 14;     // Period
input double InpLots   = 0.10;   // Lots

int handle = INVALID_HANDLE;

//+------------------------------------------------------------------+
//| Expert initialization function                                   |
//+------------------------------------------------------------------+
int OnInit()
  {
   handle = iMA(_Symbol, _Period, InpPeriod, 0, MODE_SMA, PRICE_CLOSE);
   if(handle == INVALID_HANDLE)
      return(INIT_FAILED);
   return(INIT_SUCCEEDED);
  }

//+------------------------------------------------------------------+
//| Expert tick function                                             |
//+------------------------------------------------------------------+
void OnTick()
  {
   double ma[];
   if(CopyBuffer(handle, 0, 0, 1, ma) != 1)
      return;
   Print("MA: ", ma[0]);
  }
"""
_UTF16_EDITS = [
    ('#property version   "1.00"', '#property version   "1.01"'),
    ("input int    InpPeriod = 14;     // Period",
     "input int    InpPeriod = 21;     // Period\ninput int    InpShift  = 0;      // Shift"),
    ('   Print("MA: ", ma[0]);', '   Comment("MA: ", DoubleToString(ma[0], _Digits));'),
]


def _utf16_bytes(text: str) -> bytes:
    return b"\xff\xfe" + text.replace("\n", "\r\n").encode("utf-16-le")


def _utf16(repo: Path) -> None:
    """Commit `_UTF16_PATH` and leave it edited in the working copy."""
    date = "2026-09-24T09:00:00+00:00"
    p = repo / _UTF16_PATH
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_bytes(_utf16_bytes(_UTF16_COMMITTED))
    _git(repo, "add", _UTF16_PATH, date=date)
    _git(repo, "commit", "-q", "-m", "Add the greeter expert", date=date)
    edited = _UTF16_COMMITTED
    for old, new in _UTF16_EDITS:
        edited = edited.replace(old, new)
    p.write_bytes(_utf16_bytes(edited))


# `repo-stacked`: two branches stacked on main, `stack/top` (checked out) on
# `stack/base`, two commits each (`1221-stacked-branch-refs`)
_STACKED_COMMITS = [
    ("stack/base", "2026-09-26T09:00:00+00:00", "Lay the groundwork"),
    ("stack/base", "2026-09-26T10:00:00+00:00", "Add the parser"),
    ("stack/top", "2026-09-26T11:00:00+00:00", "Use the parser"),
    ("stack/top", "2026-09-26T12:00:00+00:00", "Polish the output"),
]


def _stacked(repo: Path) -> None:
    for i, (branch, date, summary) in enumerate(_STACKED_COMMITS):
        if i == 0 or branch != _STACKED_COMMITS[i - 1][0]:
            _git(repo, "checkout", "-q", "-b", branch)
        p = repo / "stack" / f"{i + 1}.md"
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(f"{summary}\n")
        _git(repo, "add", "-A")
        _git(repo, "commit", "-q", "-m", summary, date=date)


# `repo-diff-marks` (flags 1316-1320): files committed on top, then edited
# so the working directory has a moved block and a reformatted call, a CSV
# with edited, removed and added rows, a script made executable, a patch
# whose index line and hunk numbers changed, and an image with a new square
_MARKS_CONFIG = """import { readFile } from 'fs';

function formatScore(score: number): string {
  return `${Math.round(score * 100)} points`;
}

function parseConfig(text: string): Config {
  const lines = text.split('\\n');
  const pairs = lines.map(line => line.trim()).filter(Boolean);
  return Object.fromEntries(pairs.map(pair => pair.split('=')));
}

export function main() {
  const config = loadConfiguration('settings.json', { strict: true, cache: false });
  console.log('starting', config);
  run(config);
}

function run(config: Config) {
  return startServer(config.port);
}
"""
_MARKS_CONFIG_EDITED = """import { readFile } from 'fs';

function parseConfig(text: string): Config {
  const lines = text.split('\\n');
  const pairs = lines.map(line => line.trim()).filter(Boolean);
  return Object.fromEntries(pairs.map(pair => pair.split('=')));
}

export function main() {
  const config = loadConfiguration(
    "settings.json",
    { strict: true, cache: false },
  );
  console.log("starting", config);
  run(config, 8080);
}

function run(config: Config, port: number) {
  return startServer(port);
}

function formatScore(score: number): string {
  return `${Math.round(score * 100)} points`;
}
"""
_MARKS_CSV = "id,name,city,score\n1,Alice,Paris,90\n2,Bob,London,85\n3,Carol,Berlin,77\n4,Dave,Madrid,60\n"
_MARKS_CSV_EDITED = "id,name,city,score\n1,Alice,Paris,92\n3,Carol,Berlin,77\n4,Dave,\"Madrid, ES\",60\n5,Eve,Rome,88\n"
_MARKS_PATCH = """From 1111111111111111111111111111111111111111 Mon Sep 17 00:00:00 2001
From: Parity Bot <parity@example.com>
Subject: [PATCH] Say hello louder

diff --git a/src/main.rs b/src/main.rs
index 1111111..2222222 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -10,2 +10,2 @@ fn main() {
-    println!("hello");
+    println!("HELLO");
-- 
2.39.2
"""


def _png(width: int, height: int, pixel) -> bytes:
    """A truecolour PNG of `pixel(x, y) -> (r, g, b)`."""
    import struct
    import zlib

    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    raw = b"".join(b"\0" + bytes(c for x in range(width) for c in pixel(x, y)) for y in range(height))
    header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")


def _marks_picture(square: bool):
    def pixel(x: int, y: int):
        if 40 <= x <= 120 and 40 <= y <= 120:
            return (200, 30, 30)
        if square and 180 <= x <= 210 and 130 <= y <= 160:
            return (30, 120, 220)
        return (30, 30, 30) if x % 20 == 0 else (250, 250, 250)
    return pixel


# `1321-table-file-diff`: a workbook and a database under `tables/` (after
# the other files, so their rows keep their places). The edit changes a
# price, removes a row and adds one, adds a sheet and drops a table.
_BOOK_STOCK = [["Item", "Price", "Stock"], ["Pen", 1.5, 120], ["Ink", 4, 30], ["Pad", 2.25, 75]]
_BOOK_STOCK_EDITED = [["Item", "Price", "Stock"], ["Pen", 1.75, 120], ["Pad", 2.25, 75],
                      ["Clip", 0.1, 500]]
_BOOK_NOTES = [["Note"], ["Prices in euros"]]
_BOOK_ORDERS = [["Order", "Item", "Qty"], [1, "Pen", 10], [2, "Pad", 4]]


def _xlsx(sheets: list[tuple[str, list[list]]]) -> bytes:
    """A minimal .xlsx (inline strings, no styles) of `sheets`."""
    import io
    import zipfile
    from xml.sax.saxutils import escape

    def cell_xml(row: int, col: int, value) -> str:
        ref = f"{chr(65 + col)}{row}"
        if isinstance(value, str):
            return f'<c r="{ref}" t="inlineStr"><is><t>{escape(value)}</t></is></c>'
        return f'<c r="{ref}"><v>{value}</v></c>'

    ns = 'xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"'
    rel_ns = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
    files = {
        "[Content_Types].xml": '<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
        '<Default Extension="xml" ContentType="application/xml"/>'
        '<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>'
        + "".join(f'<Override PartName="/xl/worksheets/sheet{i + 1}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
                  for i in range(len(sheets)))
        + "</Types>",
        "_rels/.rels": '<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        f'<Relationship Id="rId1" Type="{rel_ns}/officeDocument" Target="xl/workbook.xml"/></Relationships>',
        "xl/workbook.xml": f'<?xml version="1.0" encoding="UTF-8"?><workbook {ns} xmlns:r="{rel_ns}"><sheets>'
        + "".join(f'<sheet name="{escape(name)}" sheetId="{i + 1}" r:id="rId{i + 1}"/>' for i, (name, _) in enumerate(sheets))
        + "</sheets></workbook>",
        "xl/_rels/workbook.xml.rels": '<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        + "".join(f'<Relationship Id="rId{i + 1}" Type="{rel_ns}/worksheet" Target="worksheets/sheet{i + 1}.xml"/>'
                  for i in range(len(sheets)))
        + "</Relationships>",
    }
    for i, (_, rows) in enumerate(sheets):
        body = "".join(f'<row r="{r + 1}">' + "".join(cell_xml(r + 1, c, v) for c, v in enumerate(row)) + "</row>"
                       for r, row in enumerate(rows))
        files[f"xl/worksheets/sheet{i + 1}.xml"] = f'<?xml version="1.0" encoding="UTF-8"?><worksheet {ns}><sheetData>{body}</sheetData></worksheet>'
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        for name, text in files.items():
            z.writestr(zipfile.ZipInfo(name, date_time=(2026, 9, 27, 9, 0, 0)), text)
    return out.getvalue()


def _sqlite(path: Path, script: str) -> None:
    import sqlite3
    if path.exists():
        path.unlink()
    db = sqlite3.connect(path)
    db.executescript(script)
    db.commit()
    db.close()


_SHOP_SQL = """CREATE TABLE customers(id INTEGER PRIMARY KEY, name TEXT, city TEXT);
INSERT INTO customers VALUES (1, 'Ada', 'London'), (2, 'Linus', 'Helsinki'), (3, 'Grace', NULL);
CREATE TABLE legacy(code TEXT); INSERT INTO legacy VALUES ('old');"""
_SHOP_SQL_EDITED = """CREATE TABLE customers(id INTEGER PRIMARY KEY, name TEXT, city TEXT);
INSERT INTO customers VALUES (1, 'Ada', 'Cambridge'), (2, 'Linus', 'Helsinki'), (3, 'Grace', NULL),
  (4, 'Margaret', 'Boston');"""


def _table_files(repo: Path, edited: bool) -> None:
    (repo / "tables").mkdir(exist_ok=True)
    sheets = [("Stock", _BOOK_STOCK_EDITED if edited else _BOOK_STOCK), ("Notes", _BOOK_NOTES)]
    if edited:
        sheets.append(("Orders", _BOOK_ORDERS))
    (repo / "tables" / "inventory.xlsx").write_bytes(_xlsx(sheets))
    _sqlite(repo / "tables" / "shop.sqlite", _SHOP_SQL_EDITED if edited else _SHOP_SQL)


def _diff_marks(repo: Path) -> None:
    files = {
        "src/config.ts": _MARKS_CONFIG,
        "data/scores.csv": _MARKS_CSV,
        "run.sh": "#!/bin/sh\necho hello\n",
        "patches/0001-hello.patch": _MARKS_PATCH,
    }
    for rel, text in files.items():
        p = repo / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text)
    (repo / "assets").mkdir(exist_ok=True)
    (repo / "assets" / "picture.png").write_bytes(_png(300, 200, _marks_picture(False)))
    _table_files(repo, False)
    _git(repo, "add", "-A")
    _git(repo, "commit", "-q", "-m", "Add the config, scores, script and patch", date="2026-09-27T09:00:00+00:00")
    (repo / "src" / "config.ts").write_text(_MARKS_CONFIG_EDITED)
    (repo / "data" / "scores.csv").write_text(_MARKS_CSV_EDITED)
    (repo / "run.sh").chmod(0o755)
    (repo / "patches" / "0001-hello.patch").write_text(
        _MARKS_PATCH.replace("1111111..2222222", "3333333..4444444").replace("-10,2 +10,2", "-12,2 +12,2"))
    (repo / "assets" / "picture.png").write_bytes(_png(300, 200, _marks_picture(True)))
    _table_files(repo, True)


# `repo-pdf` (`1322-pdf-diff`): a three-page PDF committed on top, then
# edited so the working copy has a changed second page and a fourth page
_PDF_PAGES = [
    ["Quarterly report", "Prepared by the parity team"],
    ["Revenue grew by 4 percent", "Costs stayed flat"],
    ["Next steps", "Hire two engineers"],
]
_PDF_PAGES_EDITED = [
    _PDF_PAGES[0],
    ["Revenue grew by 9 percent", "Costs stayed flat", "Margins improved"],
    _PDF_PAGES[2],
    ["Appendix", "Raw figures are in the spreadsheet"],
]


def _pdf_bytes(pages: list[list[str]]) -> bytes:
    """A PDF with one page per entry, each line in 24 pt Helvetica; the
    binary comment after the header makes git call it binary, as it does
    most PDF files (`corvene_core::pdf` tests use the same writer)."""
    objects = [b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"]
    pages_id = 1 + 2 * len(pages) + 1
    kids = []
    for lines in pages:
        ops = "BT /F1 24 Tf 72 720 Td 30 TL\n" + "".join(f"({line}) Tj T*\n" for line in lines) + "ET"
        objects.append(f"<< /Length {len(ops)} >>\nstream\n{ops}\nendstream".encode())
        content = len(objects)
        objects.append((f"<< /Type /Page /Parent {pages_id} 0 R /MediaBox [0 0 612 792] "
                        f"/Resources << /Font << /F1 1 0 R >> >> /Contents {content} 0 R >>").encode())
        kids.append(f"{len(objects)} 0 R")
    objects.append(f"<< /Type /Pages /Kids [{' '.join(kids)}] /Count {len(kids)} >>".encode())
    objects.append(f"<< /Type /Catalog /Pages {pages_id} 0 R >>".encode())
    out = bytearray(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n\0\n")
    offsets = []
    for i, obj in enumerate(objects, 1):
        offsets.append(len(out))
        out += f"{i} 0 obj\n".encode() + obj + b"\nendobj\n"
    xref = len(out)
    out += f"xref\n0 {len(objects) + 1}\n0000000000 65535 f \n".encode()
    for offset in offsets:
        out += f"{offset:010} 00000 n \n".encode()
    out += f"trailer\n<< /Size {len(objects) + 1} /Root {len(objects)} 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode()
    return bytes(out)


def _pdf(repo: Path) -> None:
    p = repo / "docs" / "report.pdf"
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_bytes(_pdf_bytes(_PDF_PAGES))
    _git(repo, "add", "-A")
    _git(repo, "commit", "-q", "-m", "Add the quarterly report", date="2026-09-28T09:00:00+00:00")
    p.write_bytes(_pdf_bytes(_PDF_PAGES_EDITED))
# `repo-archives` (`1323-archive-diff`, `1328-archive-entries-in-file-list`):
# a zip with an archive inside it, a gzipped tar, a gzipped log and a zip
# rebuilt with new timestamps only; one commit changes the tar, then the
# working copy changes all four
def _zip(entries: list, stamp=(2026, 9, 1, 12, 0, 0)) -> bytes:
    """A zip of `(name, bytes, mode)` entries; a name ending in `/` is a folder."""
    import io
    import zipfile
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data, mode in entries:
            info = zipfile.ZipInfo(name, date_time=stamp)
            info.compress_type = zipfile.ZIP_STORED if name.endswith("/") else zipfile.ZIP_DEFLATED
            kind = 0o040000 if name.endswith("/") else 0o100000
            info.external_attr = ((kind | mode) << 16) | (0x10 if name.endswith("/") else 0)
            z.writestr(info, data)
    return out.getvalue()


def _targz(entries: list) -> bytes:
    """A gzipped tar of `(name, bytes | None, mode, link)` entries (a link when bytes is None)."""
    import gzip
    import io
    import tarfile
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.USTAR_FORMAT) as t:
        for name, data, mode, link in entries:
            info = tarfile.TarInfo(name)
            info.mtime = 1_790_000_000
            info.mode = mode
            info.uname, info.gname = "build", "build"
            if data is None:
                info.type = tarfile.SYMTYPE
                info.linkname = link
                t.addfile(info)
            else:
                info.size = len(data)
                t.addfile(info, io.BytesIO(data))
    return gzip.compress(raw.getvalue(), mtime=0)


_ARCHIVE_MAIN = """use std::env;

fn main() {
    let name = env::args().nth(1).unwrap_or_else(|| "world".into());
    println!("Hello, {name}!");
}
"""
_ARCHIVE_MAIN_EDITED = """use std::env;

mod util;

fn main() {
    let name = env::args().nth(1).unwrap_or_else(|| "world".into());
    println!("{}", util::greeting(&name));
}
"""
_ARCHIVE_UTIL = """/// The greeting for `name`.
pub fn greeting(name: &str) -> String {
    format!("Hello, {name}! Welcome back.")
}
"""
_ARCHIVE_INDEX = "<!doctype html>\n<title>Site</title>\n<h1>Welcome</h1>\n<p>Version 1</p>\n"
_ARCHIVE_LOG = "".join(f"2026-09-01 12:00:{i:02d} INFO request {i} served in {10 + i} ms\n" for i in range(20))


def _archives_bundle(edited: bool) -> bytes:
    lib = _zip([
        ("META-INF/MANIFEST.MF", b"Manifest-Version: 1.0\n", 0o644),
        ("com/example/App.class", b"class App v2" if edited else b"class App v1", 0o644),
    ] + ([("com/example/Helper.class", b"class Helper", 0o644)] if edited else []))
    entries = [
        ("README.md", b"# Bundle\n\nEverything the app ships with.\n", 0o644),
        ("src/", b"", 0o755),
        ("src/main.rs", (_ARCHIVE_MAIN_EDITED if edited else _ARCHIVE_MAIN).encode(), 0o644),
        ("img/logo.png", _png(64, 64, _marks_picture(edited)), 0o644),
        ("libs/lib.jar", lib, 0o644),
        ("new/name.txt" if edited else "old/name.txt", b"This file moves without changing.\n", 0o644),
        ("bin/run.sh", b"#!/bin/sh\nexec ./app \"$@\"\n", 0o755 if edited else 0o644),
    ]
    if edited:
        entries.append(("src/util.rs", _ARCHIVE_UTIL.encode(), 0o644))
    else:
        entries.append(("docs/guide.md", b"# Guide\n\nStart the app with bin/run.sh.\n", 0o644))
    return _zip(entries)


def _archives_site(version: int) -> bytes:
    entries = [
        ("site/index.html", _ARCHIVE_INDEX.replace("Version 1", f"Version {version}").encode(), 0o644, None),
        ("site/style.css", b"body { font: 14px sans-serif; }\n", 0o644, None),
        ("site/current", None, 0o777, "index.html"),
    ]
    entries.append(("site/main.js" if version > 2 else "site/app.js", b"console.log('hi');\n", 0o644, None))
    return _targz(entries)


def _archives(repo: Path) -> None:
    import gzip
    rebuilt = [("a.txt", b"same\n", 0o644), ("b.txt", b"also same\n", 0o644)]
    files = {
        "assets/bundle.zip": _archives_bundle(False),
        "build/rebuilt.zip": _zip(rebuilt),
        "dist/site.tar.gz": _archives_site(1),
        "logs/run.txt.gz": gzip.compress(_ARCHIVE_LOG.encode(), mtime=0),
    }
    for rel, data in files.items():
        p = repo / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_bytes(data)
    _git(repo, "add", "-A")
    _git(repo, "commit", "-q", "-m", "Add the archives", date="2026-09-27T09:00:00+00:00")
    (repo / "dist" / "site.tar.gz").write_bytes(_archives_site(2))
    _git(repo, "commit", "-q", "-am", "Publish the site again", date="2026-09-28T09:00:00+00:00")
    (repo / "assets" / "bundle.zip").write_bytes(_archives_bundle(True))
    (repo / "build" / "rebuilt.zip").write_bytes(_zip(rebuilt, stamp=(2026, 10, 12, 9, 30, 0)))
    (repo / "dist" / "site.tar.gz").write_bytes(_archives_site(3))
    log = _ARCHIVE_LOG.replace("request 7 served", "request 7 failed") + "2026-09-01 12:00:20 WARN slow request\n"
    (repo / "logs" / "run.txt.gz").write_bytes(gzip.compress(log.encode(), mtime=0))


# `repo-images` (`1326-more-image-formats`, `794-svg-image-diff`,
# `1327-image-diff-zoom`): a HEIC photo, a TIFF scan and an SVG logo with
# text, each edited in the working copy. The HEIC and TIFF files are made
# from PNGs with macOS `sips`.
_LOGO_SVG = (
    '<svg xmlns="http://www.w3.org/2000/svg" width="240" height="120" viewBox="0 0 240 120">'
    '<rect x="4" y="4" width="232" height="112" rx="16" fill="{fill}"/>'
    '<text x="120" y="74" font-family="Helvetica" font-size="40" text-anchor="middle" '
    'fill="#ffffff">{label}</text></svg>\n'
)


def _sips(png: bytes, fmt: str, out: Path) -> None:
    import tempfile
    with tempfile.TemporaryDirectory() as tmp:
        src = Path(tmp) / "in.png"
        src.write_bytes(png)
        subprocess.run(["sips", "-s", "format", fmt, str(src), "--out", str(out)],
                       check=True, capture_output=True)


def _images(repo: Path, edited: bool) -> None:
    art = repo / "art"
    art.mkdir(exist_ok=True)
    _sips(_png(320, 200, _marks_picture(edited)), "heic", art / "photo.heic")
    _sips(_png(300, 200, _marks_picture(not edited)), "tiff", art / "scan.tiff")
    (art / "logo.svg").write_text(_LOGO_SVG.format(
        fill="#2da44e" if edited else "#0969da", label="Corvene 2" if edited else "Corvene"))


# `repo-documents` (`1324-document-diff`): a Word file, a slide deck and an
# RTF file committed on top, then edited: the Word file gets an edited
# paragraph, a removed list item and a new table row; the deck an edited
# bullet on slide 2 and a new slide 3; the RTF file a new bold sentence
_W_NS = ('xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" '
         'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"')
_P_NS = ('xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" '
         'xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" '
         'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"')
_REL_NS = 'http://schemas.openxmlformats.org/package/2006/relationships'
_REL = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships'


def _zip_bytes(files: dict[str, str | bytes]) -> bytes:
    """A zip package with fixed timestamps (the same bytes every run)."""
    import io
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in files.items():
            info = zipfile.ZipInfo(name, date_time=(2026, 10, 1, 12, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, data)
    return out.getvalue()


def _docx(paragraphs: list[tuple[str, str]], table: list[list[str]]) -> bytes:
    """A .docx of `(style, text)` paragraphs (`Heading1`, `Bullet`, `` for
    body text; `**word**` is bold) and a table at the end."""
    def runs(text: str) -> str:
        out = []
        for i, part in enumerate(text.split("**")):
            if part:
                rpr = "<w:rPr><w:b/></w:rPr>" if i % 2 else ""
                out.append(f'<w:r>{rpr}<w:t xml:space="preserve">{part}</w:t></w:r>')
        return "".join(out)
    body = []
    for style, text in paragraphs:
        if style == "Bullet":
            ppr = '<w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr>'
        elif style:
            ppr = f'<w:pPr><w:pStyle w:val="{style}"/></w:pPr>'
        else:
            ppr = ""
        body.append(f"<w:p>{ppr}{runs(text)}</w:p>")
    rows = "".join(
        "<w:tr>" + "".join(f"<w:tc><w:p>{runs(c)}</w:p></w:tc>" for c in row) + "</w:tr>"
        for row in table)
    body.append(f"<w:tbl>{rows}</w:tbl>")
    return _zip_bytes({
        "[Content_Types].xml": "<Types/>",
        "_rels/.rels": f'<Relationships xmlns="{_REL_NS}"><Relationship Id="rId1" '
                       f'Type="{_REL}/officeDocument" Target="word/document.xml"/></Relationships>',
        "word/document.xml": f'<w:document {_W_NS}><w:body>{"".join(body)}</w:body></w:document>',
        "word/_rels/document.xml.rels": f'<Relationships xmlns="{_REL_NS}"><Relationship Id="rId1" '
                                        f'Type="{_REL}/styles" Target="styles.xml"/></Relationships>',
        "word/styles.xml": f'<w:styles {_W_NS}><w:style w:type="paragraph" w:styleId="Heading1">'
                           '<w:name w:val="heading 1"/></w:style><w:style w:type="paragraph" '
                           'w:styleId="Heading2"><w:name w:val="heading 2"/></w:style></w:styles>',
        "word/numbering.xml": f'<w:numbering {_W_NS}><w:abstractNum w:abstractNumId="0"><w:lvl '
                              'w:ilvl="0"><w:numFmt w:val="bullet"/></w:lvl></w:abstractNum>'
                              '<w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>',
    })


def _pptx(slides: list[tuple[str, list[str]]]) -> bytes:
    """A .pptx of `(title, bullets)` slides."""
    files: dict[str, str | bytes] = {
        "[Content_Types].xml": "<Types/>",
        "_rels/.rels": f'<Relationships xmlns="{_REL_NS}"><Relationship Id="rId1" '
                       f'Type="{_REL}/officeDocument" Target="ppt/presentation.xml"/></Relationships>',
    }
    ids = "".join(f'<p:sldId id="{256 + i}" r:id="rId{i + 10}"/>' for i in range(len(slides)))
    files["ppt/presentation.xml"] = f"<p:presentation {_P_NS}><p:sldIdLst>{ids}</p:sldIdLst></p:presentation>"
    files["ppt/_rels/presentation.xml.rels"] = f'<Relationships xmlns="{_REL_NS}">' + "".join(
        f'<Relationship Id="rId{i + 10}" Type="{_REL}/slide" Target="slides/slide{i + 1}.xml"/>'
        for i in range(len(slides))) + "</Relationships>"

    def shape(ph: str, paras: list[str]) -> str:
        ps = "".join(f"<a:p><a:r><a:t>{t}</a:t></a:r></a:p>" for t in paras)
        return (f'<p:sp><p:nvSpPr><p:cNvPr id="2" name="{ph}"/><p:cNvSpPr/><p:nvPr><p:ph type="{ph}"/>'
                f"</p:nvPr></p:nvSpPr><p:txBody>{ps}</p:txBody></p:sp>")
    for i, (title, bullets) in enumerate(slides):
        files[f"ppt/slides/slide{i + 1}.xml"] = (
            f"<p:sld {_P_NS}><p:cSld><p:spTree>{shape('title', [title])}{shape('body', bullets)}"
            "</p:spTree></p:cSld></p:sld>")
    return _zip_bytes(files)


_DOCX = [
    ("Heading1", "Project proposal"),
    ("", "We plan to ship the **new importer** before the end of the quarter."),
    ("Heading2", "Goals"),
    ("Bullet", "Read every spreadsheet format"),
    ("Bullet", "Keep memory under 200 MB"),
    ("Bullet", "Support right-to-left text"),
    ("Heading2", "Budget"),
]
_DOCX_EDITED = [
    _DOCX[0],
    ("", "We plan to ship the **new importer** in the first week of next quarter."),
    _DOCX[2], _DOCX[3], _DOCX[5],
    _DOCX[6],
    ("", "Numbers are in thousands of euros."),
]
_DOCX_TABLE = [["Item", "Cost"], ["Design", "12"], ["Engineering", "40"]]
_DOCX_TABLE_EDITED = [["Item", "Cost"], ["Design", "12"], ["Engineering", "46"], ["Testing", "8"]]
_PPTX = [
    ("Quarterly review", ["Prepared by the parity team"]),
    ("Highlights", ["Revenue grew by 4 percent", "Two new hires", "Office moved"]),
]
_PPTX_EDITED = [
    _PPTX[0],
    ("Highlights", ["Revenue grew by 9 percent", "Two new hires", "Office moved"]),
    ("Next steps", ["Hire two engineers", "Ship the importer"]),
]
_RTF = r"{\rtf1\ansi{\fonttbl{\f0 Helvetica;}}\f0\fs24 {\pard\outlinelevel0\b Meeting notes\par}" \
       r"\pard Everyone agreed on the plan.\par }"
_RTF_EDITED = r"{\rtf1\ansi{\fonttbl{\f0 Helvetica;}}\f0\fs24 {\pard\outlinelevel0\b Meeting notes\par}" \
              r"\pard Everyone agreed on the plan. {\b The launch moves to May.}\par }"


def _documents(repo: Path) -> None:
    docs = repo / "docs"
    docs.mkdir(parents=True, exist_ok=True)
    (docs / "proposal.docx").write_bytes(_docx(_DOCX, _DOCX_TABLE))
    (docs / "review.pptx").write_bytes(_pptx(_PPTX))
    (docs / "notes.rtf").write_text(_RTF)
    _git(repo, "add", "-A")
    _git(repo, "commit", "-q", "-m", "Add the proposal, review deck and notes",
         date="2026-09-28T09:00:00+00:00")
    (docs / "proposal.docx").write_bytes(_docx(_DOCX_EDITED, _DOCX_TABLE_EDITED))
    (docs / "review.pptx").write_bytes(_pptx(_PPTX_EDITED))
    (docs / "notes.rtf").write_text(_RTF_EDITED)


# `repo-notebooks` (`1331-notebook-diff`, `1332-latex-diff`): two notebooks,
# a LaTeX paper and its bibliography committed, then edited. analysis.ipynb
# (nbformat 4.5, cell ids) gets an edited code cell with a new stream, a
# recoloured plot, a changed DataFrame cell, a new traceback, a moved cell,
# a deleted and an added cell, an execution-count-only change and timing
# metadata; legacy.ipynb (4.4, no ids) an edited markdown cell with math;
# paper.tex an edited sentence, a new numbered equation and a figure
# caption; refs.bib an edited and an added entry.
def _png(width: int, height: int, rgb: tuple[int, int, int]) -> bytes:
    """A solid-colour PNG with a diagonal stripe (stdlib only)."""
    import struct
    import zlib
    rows = b""
    for y in range(height):
        row = bytearray([0])
        for x in range(width):
            row += bytes((255, 255, 255)) if abs(x - y) < 3 else bytes(rgb)
        rows += bytes(row)

    def chunk(kind: bytes, data: bytes) -> bytes:
        return (struct.pack(">I", len(data)) + kind + data
                + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows, 9)) + chunk(b"IEND", b""))


def _b64(data: bytes) -> str:
    import base64
    return base64.b64encode(data).decode()


def _lines(text: str) -> list[str]:
    parts = text.split("\n")
    return [p + "\n" for p in parts[:-1]] + ([parts[-1]] if parts[-1] else [])


def _code(cid: str | None, source: str, count: int | None, outputs: list, metadata: dict | None = None) -> dict:
    cell = {"cell_type": "code", "execution_count": count, "metadata": metadata or {},
            "outputs": outputs, "source": _lines(source)}
    if cid:
        cell["id"] = cid
    return cell


def _md(cid: str | None, source: str, attachments: dict | None = None) -> dict:
    cell = {"cell_type": "markdown", "metadata": {}, "source": _lines(source)}
    if cid:
        cell["id"] = cid
    if attachments:
        cell["attachments"] = attachments
    return cell


def _stream(text: str, name: str = "stdout") -> dict:
    return {"name": name, "output_type": "stream", "text": _lines(text)}


def _result(count: int, data: dict) -> dict:
    return {"data": data, "execution_count": count, "metadata": {}, "output_type": "execute_result"}


def _display(data: dict) -> dict:
    return {"data": data, "metadata": {}, "output_type": "display_data"}


def _frame(rows: list[tuple[str, int, float]]) -> str:
    body = "".join(f"<tr><th>{i}</th><td>{n}</td><td>{c}</td><td>{p}</td></tr>"
                   for i, (n, c, p) in enumerate(rows))
    return ('<div><table border="1" class="dataframe"><thead><tr style="text-align: right;">'
            "<th></th><th>name</th><th>count</th><th>share</th></tr></thead>"
            f"<tbody>{body}</tbody></table></div>")


def _notebook(cells: list, ids: bool = True) -> str:
    import json
    nb = {"cells": cells, "metadata": {
        "kernelspec": {"display_name": "Python 3 (ipykernel)", "language": "python", "name": "python3"},
        "language_info": {"file_extension": ".py", "name": "python", "version": "3.12.4"}},
        "nbformat": 4, "nbformat_minor": 5 if ids else 4}
    return json.dumps(nb, indent=1, sort_keys=True, ensure_ascii=False) + "\n"


def _analysis(edited: bool) -> str:
    rows = [("apples", 12, 0.4), ("pears", 9, 0.3), ("plums", 9, 0.3)]
    rows2 = [("apples", 14, 0.45), ("pears", 9, 0.29), ("plums", 8, 0.26)]
    plot = _png(48, 32, (31, 111, 235) if not edited else (218, 54, 51))
    tb = ["\x1b[0;31m---------------------------------------------------------------------------\x1b[0m",
          "\x1b[0;31mZeroDivisionError\x1b[0m                         Traceback (most recent call last)",
          "Cell \x1b[0;32mIn[6], line 1\x1b[0m\n\x1b[0;32m----> 1\x1b[0m \x1b[38;5;241m1\x1b[39m\x1b[38;5;241m/\x1b[39m\x1b[38;5;241m0\x1b[39m",
          "\x1b[0;31mZeroDivisionError\x1b[0m: division by zero"]
    title = _md("c0", "# Fruit survey\n\nShares are $p_i = n_i / \\sum_j n_j$, and\n\n"
                      "$$H = -\\sum_i p_i \\log_2 p_i$$")
    imports = _code("c1", "import pandas as pd\nimport matplotlib.pyplot as plt", 1, [])
    load = _code("c2", "df = pd.read_csv('fruit.csv')\nprint(len(df), 'rows')" if not edited else
                 "df = pd.read_csv('fruit.csv', comment='#')\nprint(len(df), 'rows')\nprint('done')",
                 2 if not edited else 5,
                 [_stream("3 rows\n")] if not edited else [_stream("3 rows\ndone\n")],
                 {} if not edited else {"ExecuteTime": {"end_time": "2026-10-12T09:00:01Z"}})
    table = _code("c3", "df", 3 if not edited else 6,
                  [_result(3 if not edited else 6, {"text/html": _frame(rows if not edited else rows2),
                                                    "text/plain": "    name  count  share"})])
    chart = _code("c4", "df.plot.bar(x='name', y='count', color='C0')" if not edited else
                  "df.plot.bar(x='name', y='count', color='C3')", 4 if not edited else 7,
                  [_display({"image/png": _b64(plot), "text/plain": "<Figure size 640x480 with 1 Axes>"})])
    formula = _code("c5", "import sympy as sp\nx = sp.symbols('x')\nsp.integrate(x**2, x)", 5 if not edited else 8,
                    [_result(5 if not edited else 8, {"text/latex": "$\\displaystyle \\frac{x^{3}}{3}$",
                                                      "text/plain": "x**3/3"})])
    helper = _code("c6", "def share(n, total):\n    return n / total", 6 if not edited else 2, [])
    old_note = _md("c7", "Remember to check the plums again next week.")
    boom = _code("c8", "1/0", 9, [{"ename": "ZeroDivisionError", "evalue": "division by zero",
                                     "output_type": "error", "traceback": tb}])
    new_note = _md("c9", "## Next steps\n\n- Ask **two** more shops\n- Plot the shares over time\n\n"
                         "![logo](attachment:logo.png)",
                   {"logo.png": {"image/png": _b64(_png(16, 16, (46, 160, 67)))}})
    if not edited:
        cells = [title, helper, imports, load, table, chart, formula, old_note]
    else:
        cells = [title, imports, load, table, chart, formula, helper, boom, new_note]
    return _notebook(cells)


def _legacy(edited: bool) -> str:
    text = ("# Notes\n\nThe mean of $n$ values is $\\bar x = \\frac{1}{n}\\sum x_i$." if not edited else
            "# Notes\n\nThe weighted mean of $n$ values is $\\bar x = \\sum w_i x_i / \\sum w_i$.")
    cells = [_md(None, text), _code(None, "values = [3, 4, 5]\nsum(values) / len(values)", 1,
                                   [_result(1, {"text/plain": "4.0"})])]
    return _notebook(cells, ids=False)


_PAPER = r"""\documentclass{article}
\usepackage{amsmath}
\newcommand{\R}{\mathbb{R}}
\title{Counting Fruit}
\author{Parity Team}
\begin{document}
\maketitle
\begin{abstract}
We count fruit in three shops.
\end{abstract}
\section{Introduction}
Fruit is counted by hand~\cite{smith2020}. Each count $n_i \in \R$ is a share of the total.
\section{Method}
The share of fruit $i$ is
\begin{equation}\label{eq:share}
  p_i = \frac{n_i}{\sum_j n_j}.
\end{equation}
\begin{figure}
  \includegraphics[width=4cm]{fig.png}
  \caption{Counts per shop.}
\end{figure}
\end{document}
"""

_PAPER_EDITED = r"""\documentclass{article}
\usepackage{amsmath}
\newcommand{\R}{\mathbb{R}}
\title{Counting Fruit}
\author{Parity Team}
\begin{document}
\maketitle
\begin{abstract}
We count fruit in five shops over two weeks.
\end{abstract}
\section{Introduction}
Fruit is counted by hand~\cite{smith2020,lee2024}. Each count $n_i \in \R$ is a share of the total.
\section{Method}
The share of fruit $i$ is
\begin{equation}\label{eq:share}
  p_i = \frac{n_i}{\sum_j n_j}.
\end{equation}
Its entropy, from \eqref{eq:share}, is
\begin{equation}
  H = -\sum_i p_i \log_2 p_i.
\end{equation}
\begin{figure}
  \includegraphics[width=4cm]{fig.png}
  \caption{Counts per shop and week.}
\end{figure}
\end{document}
"""

_BIB = """@article{smith2020,
  author = {Smith, Jane and Doe, John},
  title = {Counting Things by Hand},
  journal = {Journal of Counting},
  year = {2020},
  volume = {12},
  pages = {1--10}
}
"""

_BIB_EDITED = """@article{smith2020,
  author = {Smith, Jane and Doe, John},
  title = {Counting Things by Hand},
  journal = {Journal of Counting},
  year = {2021},
  volume = {13},
  pages = {1--12}
}

@book{lee2024,
  author = {Lee, Ana},
  title = {Fruit in Numbers},
  publisher = {Orchard Press},
  year = {2024}
}
"""


def _notebooks(repo: Path) -> None:
    nbs = repo / "notebooks"
    paper = repo / "paper"
    nbs.mkdir(parents=True, exist_ok=True)
    paper.mkdir(parents=True, exist_ok=True)
    (nbs / "analysis.ipynb").write_text(_analysis(False))
    (nbs / "legacy.ipynb").write_text(_legacy(False))
    (paper / "paper.tex").write_text(_PAPER)
    (paper / "refs.bib").write_text(_BIB)
    (paper / "fig.png").write_bytes(_png(64, 40, (130, 80, 223)))
    _git(repo, "add", "-A")
    _git(repo, "commit", "-q", "-m", "Add the fruit survey notebooks and paper",
         date="2026-09-28T09:00:00+00:00")
    (nbs / "analysis.ipynb").write_text(_analysis(True))
    (nbs / "legacy.ipynb").write_text(_legacy(True))
    (paper / "paper.tex").write_text(_PAPER_EDITED)
    (paper / "refs.bib").write_text(_BIB_EDITED)


def build(parent: Path, remote: bool = False, coauthors: bool = False, graph: bool = False,
          signed: bool = False, reflog: bool = False, tools: bool = False,
          structure: bool = False, lfs_url: str | None = None, remotes: bool = False,
          pull_request: bool = False, utf16: bool = False, stacked: bool = False,
          forks: bool = False, mismatch: bool = False, codeowners: bool = False,
          diff_marks: bool = False, pdf: bool = False, *, images: bool = False, archives: bool = False,
          documents: bool = False, notebooks: bool = False) -> Path:
    """(Re)create `<parent>/parity-fixture` and return its path.

    With `remote`, a bare `<parent>/parity-fixture.git` is added as `origin`
    and `main` is pushed up to its third commit, so the branch is two ahead
    with the `v0.1.0` tag still to push (History's unpushed indicators, the
    toolbar's Push origin). With `coauthors`, `_CO_AUTHORED` commits go on
    top. With `graph`, a merged branch and an octopus merge of two more go on
    top (History's commit graph). With `signed`, see `_signed`. With
    `reflog`, HEAD's reflog gets a rebase, a branch deleted after use and
    three commits a hard reset left behind (Recent Activity,
    `1216-recent-activity`). With `tools`, see `_tools` (Clean Untracked
    Files and Apply Patch, flags 1105 and 1106). With `structure`, see
    `_structure` (submodules, sparse checkout and LFS locks, flags
    1111-1113). With `remotes` (and `remote`), see `_remotes` (Repository
    Settings' remotes and Branch › Tags…, flags 1109 and 1219). With
    `pull_request` (and `remote`), `main` is pushed whole and `feature/login`
    is checked out with `_PULL_REQUEST_COMMITS` pushed
    (`348-pull-request-review`). With `utf16`, see `_utf16`
    (`1306-utf16-diffs`). With `stacked`, see `_stacked`, and no working
    changes, so commits can be squashed. With `forks` (and `remote`), see
    `_forks` (`1223-checkout-from-fork`). With `mismatch` (and `remote`),
    `issue-134` is checked out off main, tracking `origin/main`
    (`1222-push-target-guard`). With `codeowners`, a commit adds
    `_CODEOWNERS` as `.github/CODEOWNERS` (`1314-code-owners`). With `diff_marks`, see `_diff_marks`
    and no other working changes (flags 1316-1320). With `pdf`, see `_pdf`
    and no other working changes (`1322-pdf-diff`). With `documents`, see
    `_documents` (`1324-document-diff`). With `notebooks`, see `_notebooks`
    (`1331-notebook-diff`, `1332-latex-diff`)."""
    repo = parent / NAME
    if repo.exists():
        remove_tree(repo)
    repo.mkdir(parents=True)
    _git(repo, "init", "-q", "-b", "main")
    for key, value in (("user.name", AUTHOR[0]), ("user.email", AUTHOR[1]), ("commit.gpgsign", "false"), ("tag.gpgsign", "false")):
        _git(repo, "config", key, value)
    for i, (date, summary, body, files) in enumerate(_COMMITS):
        for rel, text in files.items():
            p = repo / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(text)
        _git(repo, "add", "-A")
        msg = summary + (f"\n\n{body}" if body else "")
        _git(repo, "commit", "-q", "-m", msg, date=date)
        if i == 3:
            _git(repo, "branch", "feature/login")
            _git(repo, "branch", "bugfix/typo-in-guide")
    _git(repo, "tag", "v0.1.0", "HEAD~1")
    if codeowners:
        p = repo / ".github" / "CODEOWNERS"
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(_CODEOWNERS)
        _git(repo, "add", "-A")
        _git(repo, "commit", "-q", "-m", "Add code owners", date="2026-09-20T09:00:00+00:00")
    if coauthors:
        for i, (date, summary, count, committer) in enumerate(_CO_AUTHORED):
            (repo / "TEAM.md").write_text("".join(f"- {n}\n" for n, _ in _PEOPLE[: i + 1]))
            _git(repo, "add", "-A")
            trailers = "".join(f"\nCo-authored-by: {n} <{e}>" for n, e in _PEOPLE[:count])
            _git(repo, "commit", "-q", "-m", f"{summary}\n{trailers}", date=date, committer=committer or AUTHOR)
    if graph:
        def commit(rel: str, summary: str, date: str):
            p = repo / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(f"{summary}\n")
            _git(repo, "add", "-A")
            _git(repo, "commit", "-q", "-m", summary, date=date)

        _git(repo, "checkout", "-q", "-b", "topic/lanes", "HEAD~1")
        commit("graph/lanes.md", "Draw the lanes", "2026-09-24T09:00:00+00:00")
        _git(repo, "checkout", "-q", "main")
        commit("graph/plan.md", "Plan the graph", "2026-09-24T10:00:00+00:00")
        _git(repo, "merge", "-q", "--no-ff", "-m", "Merge branch 'topic/lanes'", "topic/lanes",
             date="2026-09-24T11:00:00+00:00")
        for name, hour in (("one", 12), ("two", 13)):
            _git(repo, "checkout", "-q", "-b", f"topic/{name}", "main")
            commit(f"graph/{name}.md", f"Add part {name}", f"2026-09-24T{hour}:00:00+00:00")
        _git(repo, "checkout", "-q", "main")
        _git(repo, "merge", "-q", "--no-ff", "-m", "Merge parts one and two", "topic/one", "topic/two",
             date="2026-09-24T14:00:00+00:00")
        # a branch that is not merged (All branches)
        _git(repo, "checkout", "-q", "-b", "topic/later", "main")
        commit("graph/later.md", "Start the next part", "2026-09-24T15:00:00+00:00")
        _git(repo, "checkout", "-q", "main")
    if signed:
        _signed(parent, repo)
    if reflog:
        def day(hour: int) -> str:
            return f"2026-09-25T{hour:02}:00:00+00:00"

        def commit_at(rel: str, summary: str, hour: int):
            p = repo / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(f"{summary}\n")
            _git(repo, "add", "-A", date=day(hour))
            _git(repo, "commit", "-q", "-m", summary, date=day(hour))

        _git(repo, "checkout", "-q", "-b", "topic/rebase", "HEAD~1", date=day(8))
        commit_at("notes/one.md", "Write the first note", 8)
        commit_at("notes/two.md", "Write the second note", 9)
        _git(repo, "rebase", "-q", "main", date=day(10))
        _git(repo, "checkout", "-q", "main", date=day(10))
        _git(repo, "checkout", "-q", "-b", "experiment", date=day(11))
        commit_at("experiment/idea.md", "Try an idea", 11)
        commit_at("experiment/more.md", "Push the idea further", 12)
        _git(repo, "checkout", "-q", "main", date=day(13))
        _git(repo, "branch", "-q", "-D", "experiment", date=day(13))
        for hour, name in ((14, "A"), (15, "B"), (16, "C")):
            commit_at(f"drafts/{name}.md", f"Draft {name}", hour)
        _git(repo, "reset", "-q", "--hard", "HEAD~3", date=day(17))
    if remote:
        bare = parent / f"{NAME}.git"
        if bare.exists():
            remove_tree(bare)
        _git(repo, "init", "-q", "--bare", str(bare))
        _git(repo, "remote", "add", "origin", str(bare))
        # the two newest commits and the tag stay behind
        _git(repo, "push", "-q", "origin", "main~2:refs/heads/main")
        _git(repo, "branch", "-q", "--set-upstream-to=origin/main", "main")
    if remote and remotes:
        _remotes(parent, repo)
    if remote and forks:
        _forks(parent, repo)
    if tools:
        _tools(parent, repo)
    if structure:
        _structure(parent, repo, lfs_url)
    if pull_request and remote:
        _git(repo, "push", "-q", "origin", "main")
        # the fixture already has a stale `feature/login`: reset it onto main
        _git(repo, "checkout", "-q", "-B", "feature/login", "main")
        for date, summary, files in _PULL_REQUEST_COMMITS:
            for rel, text in files.items():
                p = repo / rel
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_text(text)
            _git(repo, "add", "-A")
            _git(repo, "commit", "-q", "-m", summary, date=date)
        _git(repo, "push", "-q", "-u", "origin", "feature/login")
    if utf16:
        _utf16(repo)
    if mismatch and remote:
        _git(repo, "checkout", "-q", "-b", "issue-134")
        _git(repo, "branch", "-q", "--set-upstream-to=origin/main", "issue-134")
    if stacked:
        _stacked(repo)
        return repo
    if diff_marks:
        _diff_marks(repo)
        return repo
    if pdf:
        _pdf(repo)
        return repo
    if images:
        _images(repo, False)
        _git(repo, "add", "-A")
        _git(repo, "commit", "-q", "-m", "Add the artwork", date="2026-09-28T10:00:00+00:00")
        _images(repo, True)
        return repo
    if archives:
        _archives(repo)
        return repo
    if documents:
        _documents(repo)
        return repo
    if notebooks:
        _notebooks(repo)
        return repo
    for rel, text in _WORKING_CHANGES.items():
        p = repo / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text)
    for rel in _DELETED:
        (repo / rel).unlink()
    return repo


if __name__ == "__main__":
    import sys

    print(build(Path(sys.argv[1] if len(sys.argv) > 1 else ".")))
