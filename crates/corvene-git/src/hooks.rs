//! GitHub Desktop's hooks interception (`lib/hooks/with-hooks-env.ts`,
//! `lib/hooks/hooks-proxy.ts`, `lib/hooks/get-repo-hooks.ts`): while
//! Settings › Git › Hooks › "Load Git hook environment variables from shell"
//! is on ([`set_enabled`]; with Corvene's
//! `1121-hook-diagnostics-without-shell-env` always), a command built with
//! [`GitCommand::intercept_hooks`](crate::GitCommand::intercept_hooks) runs
//! with `core.hooksPath` pointing at a temporary directory of stand-in
//! hooks, one per hook of the repository it intercepts. Each stand-in runs
//! the Corvene binary as a proxy ([`run_proxy`], GHD's `process-proxy`),
//! which reports to the app over a local socket, runs the real hook with
//! `git hook run` and writes its output to stderr, where it joins git's
//! terminal output. The app hears when a hook starts and ends
//! ([`HookCallbacks::on_hook_progress`]) and, when one fails, whether to
//! ignore the failure ([`HookCallbacks::on_hook_failure`], GHD's HookFailed
//! dialog).
//!
//! Corvene (flag `1114-hook-results`, desktop/desktop#22476): GHD drops the
//! outcome of the hooks that cannot stop an operation
//! (`ignoredOnFailureHooks`), so a failed post-merge after a pull goes
//! unseen. With [`HookCallbacks::on_hook_result`] set, the proxy reports
//! their exit and output ([`HookResult`]) and, the app having taken it in,
//! ends with 0: git ignores those exit codes except post-checkout's, which
//! would otherwise fail a checkout that went through.
//!
//! Deviations: the hooks directory comes from `git rev-parse --git-path
//! hooks`, which expands `~` in `core.hooksPath` the way git does (GHD reads
//! `core.hooksPath` itself and resolves it against the repository without
//! expanding it). The hook environment is git's own (Corvene puts the shell
//! environment on every git command, `crate::hook_env`) minus the variables
//! GHD keeps from hooks; GHD starts from a fresh shell environment and adds
//! git's `GIT_*` variables. Android never intercepts hooks: the app has no
//! binary git could run.

use std::collections::hash_map::RandomState;
use std::ffi::{OsStr, OsString};
use std::hash::{BuildHasher, Hasher};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tracing::{debug, warn};

use crate::detect::GitBinary;
use crate::process::{
    GitCommand, OUTPUT_HELD, PIPE_BACKLOG, PipeChunk, StreamedPipe, boxed_pipe, read_pipe,
    receive_until_exit,
};

/// `hooks-proxy.ts` `ignoredOnFailureHooks`: their failure never asks.
const IGNORED_ON_FAILURE_HOOKS: &[&str] = &[
    "post-applypatch",
    "post-commit",
    "post-checkout",
    "post-merge",
    "pre-auto-gc",
    "post-rewrite",
];

/// `hooks-proxy.ts` `excludedEnvVars`, plus the proxy's own variables and
/// Corvene's askpass ones (GHD's trampoline variables never reach a hook).
const EXCLUDED_ENV_VARS: &[&str] = &[
    "GIT_SYSTEM_CONFIG",
    "GIT_EXEC_PATH",
    "GIT_TEMPLATE_DIR",
    "GIT_CONFIG_PARAMETERS",
    "GIT_ASKPASS",
    "GIT_SSH_COMMAND",
    "GIT_USER_AGENT",
    "SSH_ASKPASS",
    "SSH_ASKPASS_REQUIRE",
    "CORVENE_ASKPASS",
    "CORVENE_ASKPASS_LOGINS",
    "CORVENE_ASKPASS_GH_CLI",
    PORT_VAR,
    TOKEN_VAR,
    GIT_VAR,
    DIR_VAR,
];

/// `get-repo-hooks.ts` `knownHooks`.
const KNOWN_HOOKS: &[&str] = &[
    "applypatch-msg",
    "pre-applypatch",
    "post-applypatch",
    "pre-commit",
    "pre-merge-commit",
    "prepare-commit-msg",
    "commit-msg",
    "post-commit",
    "pre-rebase",
    "post-checkout",
    "post-merge",
    "pre-push",
    "pre-receive",
    "update",
    "proc-receive",
    "post-receive",
    "post-update",
    "reference-transaction",
    "push-to-checkout",
    "pre-auto-gc",
    "post-rewrite",
    "sendemail-validate",
    "fsmonitor-watchman",
    "p4-changelist",
    "p4-prepare-changelist",
    "p4-post-changelist",
    "p4-pre-submit",
    "post-index-change",
];

/// The hooks whose outcome [`HookCallbacks::on_hook_result`] hears:
/// [`IGNORED_ON_FAILURE_HOOKS`] but `pre-auto-gc`, whose non-zero exit only
/// means "no gc now".
pub const REPORTED_HOOKS: &[&str] = &[
    "post-applypatch",
    "post-commit",
    "post-checkout",
    "post-merge",
    "post-rewrite",
];

/// GHD `createCommit`'s `interceptHooks`; `post-rewrite` only for an amend.
pub const COMMIT_HOOKS: &[&str] = &[
    "pre-commit",
    "prepare-commit-msg",
    "commit-msg",
    "post-commit",
    "pre-auto-gc",
];

const PORT_VAR: &str = "CORVENE_HOOK_PROXY_PORT";
const TOKEN_VAR: &str = "CORVENE_HOOK_PROXY_TOKEN";
const GIT_VAR: &str = "CORVENE_HOOK_PROXY_GIT";
const DIR_VAR: &str = "CORVENE_HOOK_PROXY_DIR";

/// How long the proxy waits for the app to take in the hook's exit.
const EXIT_ACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The argument a stand-in hook runs the Corvene binary with.
pub const PROXY_ARG: &str = "--hook-proxy";

/// GHD `HookProgress.status`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookStatus {
    Started,
    Finished,
    Failed,
}

/// GHD `HookProgress` (without `abort`, which no GHD view calls).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookProgress {
    pub hook_name: String,
    pub status: HookStatus,
}

/// What the HookFailed dialog decided (`'abort' | 'ignore'`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookFailureResolution {
    Abort,
    Ignore,
}

/// How a [`REPORTED_HOOKS`] hook ended (`1114-hook-results`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookResult {
    pub hook_name: String,
    /// The hook's exit code; `None` when a signal ended it.
    pub code: Option<i32>,
    pub duration: std::time::Duration,
    /// What the hook printed (git runs it with stdout on stderr).
    pub output: Vec<u8>,
    /// The proxy's closing line ("post-merge hook failed with code 1 after
    /// 0.20s").
    pub termination: String,
}

impl HookResult {
    pub fn failed(&self) -> bool {
        self.code != Some(0)
    }
}

pub type HookProgressFn = Arc<dyn Fn(HookProgress) + Send + Sync>;
/// Called on the proxy's connection thread; must not block for long.
pub type HookResultFn = Arc<dyn Fn(HookResult) + Send + Sync>;
/// Called on a thread of its own and blocks until the user decides; gets
/// the hook's name and its terminal output.
pub type HookFailureFn = Arc<dyn Fn(&str, Vec<u8>) -> HookFailureResolution + Send + Sync>;

/// GHD `HookCallbackOptions` (`onTerminalOutputAvailable` is
/// [`GitCommand::run_with_terminal_output`]'s).
#[derive(Clone, Default)]
pub struct HookCallbacks {
    pub on_hook_progress: Option<HookProgressFn>,
    pub on_hook_failure: Option<HookFailureFn>,
    /// Corvene (`1114-hook-results`): the outcome of every
    /// [`REPORTED_HOOKS`] hook.
    pub on_hook_result: Option<HookResultFn>,
}

impl std::fmt::Debug for HookCallbacks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HookCallbacks")
            .field("on_hook_progress", &self.on_hook_progress.is_some())
            .field("on_hook_failure", &self.on_hook_failure.is_some())
            .field("on_hook_result", &self.on_hook_result.is_some())
            .finish()
    }
}

static ENABLED: AtomicBool = AtomicBool::new(false);
static PROXY_PROGRAM: Mutex<Option<PathBuf>> = Mutex::new(None);

/// GHD `getHooksEnvEnabled()`: Settings › Git › Hooks.
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    !cfg!(target_os = "android") && ENABLED.load(Ordering::Relaxed)
}

/// The binary the stand-in hooks run (with [`PROXY_ARG`]); the current
/// executable unless set.
pub fn set_proxy_program(program: Option<PathBuf>) {
    if let Ok(mut slot) = PROXY_PROGRAM.lock() {
        *slot = program;
    }
}

fn proxy_program() -> Option<PathBuf> {
    PROXY_PROGRAM
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
        .or_else(|| std::env::current_exe().ok())
}

thread_local! {
    static SCOPED: std::cell::RefCell<Option<HookCallbacks>> =
        const { std::cell::RefCell::new(None) };
}

/// Run `f` with `callbacks` for every command it builds on this thread with
/// [`GitCommand::intercept_hooks`].
pub fn with_hook_callbacks<R>(callbacks: &HookCallbacks, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<HookCallbacks>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            SCOPED.with(|scoped| *scoped.borrow_mut() = previous);
        }
    }
    let _restore = Restore(SCOPED.with(|scoped| scoped.replace(Some(callbacks.clone()))));
    f()
}

/// Whether the callbacks of [`with_hook_callbacks`] on this thread take
/// hook results: commands GHD never intercepts (checkout) intercept only
/// then.
pub(crate) fn scoped_reports_results() -> bool {
    SCOPED.with(|scoped| {
        scoped
            .borrow()
            .as_ref()
            .is_some_and(|c| c.on_hook_result.is_some())
    })
}

/// What a command intercepts: GHD's `interceptHooks` and the callbacks.
#[derive(Clone, Debug)]
pub(crate) struct Interception {
    names: Vec<String>,
    callbacks: HookCallbacks,
}

/// `None` while interception is off.
pub(crate) fn interception(names: &[&str]) -> Option<Interception> {
    enabled().then(|| Interception {
        names: names.iter().map(|n| n.to_string()).collect(),
        callbacks: SCOPED
            .with(|scoped| scoped.borrow().clone())
            .unwrap_or_default(),
    })
}

/// GHD `getRepoHooks(path, filter)`: the executable hooks of the
/// repository at `workdir` among `filter`.
pub fn get_repo_hooks(git: Arc<GitBinary>, workdir: &Path, filter: &[&str]) -> Vec<String> {
    let Ok(out) = GitCommand::new(git)
        .args(["rev-parse", "--git-path", "hooks"])
        .current_dir(workdir)
        .run()
    else {
        return Vec::new();
    };
    let dir = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
    let dir = workdir.join(dir);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut hooks = Vec::new();
    for entry in entries.flatten() {
        if !entry
            .file_type()
            .is_ok_and(|t| t.is_file() || t.is_symlink())
        {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let name = name.strip_suffix(".exe").unwrap_or(&name).to_string();
        if !filter.contains(&name.as_str()) || !KNOWN_HOOKS.contains(&name.as_str()) {
            continue;
        }
        if cfg!(windows) || is_executable(&entry.path()) {
            hooks.push(name);
        }
    }
    hooks.sort();
    hooks
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// A random hex string (the proxy token, temporary names).
fn random_hex(len: usize) -> String {
    let mut out = String::new();
    while out.len() < len {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default(),
        );
        out.push_str(&format!("{:016x}", hasher.finish()));
    }
    out.truncate(len);
    out
}

/// git's `sq_quote_buf`: `'…'`, a single quote as `'\''`.
fn sq_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

struct Shared {
    token: String,
    callbacks: HookCallbacks,
    stop: AtomicBool,
}

/// One intercepted command's stand-in hooks and proxy server (GHD
/// `withHooksEnv`); removed when dropped.
pub(crate) struct Session {
    dir: PathBuf,
    port: u16,
    shared: Arc<Shared>,
    accept: Option<std::thread::JoinHandle<()>>,
    git: PathBuf,
}

impl Interception {
    /// `None` when the repository has none of the hooks (git runs as usual)
    /// or the proxy could not be set up.
    pub(crate) fn start(&self, git: &Arc<GitBinary>, cwd: Option<&Path>) -> Option<Session> {
        let workdir = cwd?;
        let names: Vec<&str> = self.names.iter().map(String::as_str).collect();
        let hooks = get_repo_hooks(git.clone(), workdir, &names);
        if hooks.is_empty() {
            return None;
        }
        match Session::start(&hooks, self.callbacks.clone(), git) {
            Ok(session) => Some(session),
            Err(err) => {
                warn!(%err, "could not set up the hooks proxy; hooks run as usual");
                None
            }
        }
    }
}

impl Session {
    fn start(
        hooks: &[String],
        callbacks: HookCallbacks,
        git: &Arc<GitBinary>,
    ) -> std::io::Result<Self> {
        let program = proxy_program().ok_or_else(|| std::io::Error::other("no proxy program"))?;
        let dir = loop {
            let dir = std::env::temp_dir().join(format!("corvene-git-hooks-{}", random_hex(8)));
            match std::fs::create_dir(&dir) {
                Ok(()) => break dir,
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(err),
            }
        };
        let session = Self::serve(dir, callbacks, git.path.clone())?;
        // `process-proxy` copied once per hook; a script naming the binary here
        let script = format!(
            "#!/bin/sh\nexec {} {PROXY_ARG} \"$0\" \"$@\"\n",
            sq_quote(&program.to_string_lossy())
        );
        for hook in hooks {
            let path = session.dir.join(hook);
            std::fs::write(&path, &script)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
            }
        }
        debug!(?hooks, dir = %session.dir.display(), "intercepting hooks");
        Ok(session)
    }

    fn serve(dir: PathBuf, callbacks: HookCallbacks, git: PathBuf) -> std::io::Result<Self> {
        let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, 0)) {
            Ok(listener) => listener,
            Err(err) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(err);
            }
        };
        let port = listener.local_addr()?.port();
        let shared = Arc::new(Shared {
            token: random_hex(32),
            callbacks,
            stop: AtomicBool::new(false),
        });
        let accept = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("hooks-proxy".into())
                .spawn(move || {
                    for conn in listener.incoming() {
                        if shared.stop.load(Ordering::SeqCst) {
                            break;
                        }
                        let Ok(conn) = conn else { continue };
                        let shared = shared.clone();
                        // a connection waits for the HookFailed dialog: never
                        // joined, the command ends once the hook has
                        let _ = std::thread::Builder::new()
                            .name("hooks-proxy-conn".into())
                            .spawn(move || handle_connection(conn, &shared));
                    }
                })?
        };
        Ok(Self {
            dir,
            port,
            shared,
            accept: Some(accept),
            git,
        })
    }

    /// The variables the intercepted git command gets: `existing` is its
    /// `GIT_CONFIG_PARAMETERS`, if any.
    pub(crate) fn env(&self, existing: Option<&OsStr>) -> Vec<(OsString, OsString)> {
        let existing = existing
            .map(|v| v.to_string_lossy().into_owned())
            .or_else(|| std::env::var("GIT_CONFIG_PARAMETERS").ok())
            .unwrap_or_default();
        let prefix = if existing.is_empty() {
            String::new()
        } else {
            format!("{existing} ")
        };
        let hooks_path = format!("core.hooksPath={}", self.dir.display());
        vec![
            (
                "GIT_CONFIG_PARAMETERS".into(),
                format!("{prefix}{}", sq_quote(&hooks_path)).into(),
            ),
            (PORT_VAR.into(), self.port.to_string().into()),
            (TOKEN_VAR.into(), self.shared.token.clone().into()),
            (GIT_VAR.into(), self.git.clone().into_os_string()),
            (DIR_VAR.into(), self.dir.clone().into_os_string()),
        ]
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        // wakes the accept loop
        let _ = TcpStream::connect((Ipv4Addr::LOCALHOST, self.port));
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The app's side of one proxy (`createHooksProxy`): `<token> <hook>` →
/// started, `failed <n>` + n bytes of output → the failure callback's
/// answer, `result <code|signal> <ms> <n>` + n bytes (output, then the
/// closing line) → `shown` when the result callback took it, else `ok`,
/// `exit <code>` → finished or failed.
fn handle_connection(conn: TcpStream, shared: &Shared) {
    let Ok(write) = conn.try_clone() else { return };
    let mut write = write;
    let mut reader = BufReader::new(conn);
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let Some((token, hook_name)) = line.trim_end().split_once(' ') else {
        return;
    };
    if token != shared.token {
        warn!("hooks proxy: connection with a wrong token");
        return;
    }
    let hook_name = hook_name.to_string();
    let progress = |status| {
        if let Some(on_progress) = &shared.callbacks.on_hook_progress {
            on_progress(HookProgress {
                hook_name: hook_name.clone(),
                status,
            });
        }
    };
    progress(HookStatus::Started);
    // a reported hook that failed ends with 0 once shown: still a failure
    let mut reported_failure = false;
    if write.write_all(b"run\n").is_err() {
        return;
    }
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            // the proxy went away (the command was stopped)
            debug!(hook = %hook_name, "hooks proxy closed");
            return;
        }
        let line = line.trim_end();
        if let Some(len) = line.strip_prefix("failed ") {
            let mut output = vec![0; len.parse().unwrap_or(0)];
            if reader.read_exact(&mut output).is_err() {
                return;
            }
            let resolution = match &shared.callbacks.on_hook_failure {
                Some(on_failure) => on_failure(&hook_name, output),
                None => HookFailureResolution::Abort,
            };
            let answer: &[u8] = match resolution {
                HookFailureResolution::Ignore => b"ignore\n",
                HookFailureResolution::Abort => b"abort\n",
            };
            if write.write_all(answer).is_err() {
                return;
            }
        } else if let Some(rest) = line.strip_prefix("result ") {
            let mut parts = rest.split(' ');
            let code = parts.next().and_then(|c| c.parse::<i32>().ok());
            let millis = parts.next().and_then(|m| m.parse().ok()).unwrap_or(0);
            let len = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
            let mut bytes = vec![0; len];
            if reader.read_exact(&mut bytes).is_err() {
                return;
            }
            reported_failure = code != Some(0);
            let answer: &[u8] = match &shared.callbacks.on_hook_result {
                Some(on_result) => {
                    // the closing line is the last one
                    let body = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
                    let split = body.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
                    on_result(HookResult {
                        hook_name: hook_name.clone(),
                        code,
                        duration: std::time::Duration::from_millis(millis),
                        termination: String::from_utf8_lossy(&body[split..]).into_owned(),
                        output: bytes[..split].to_vec(),
                    });
                    b"shown\n"
                }
                None => b"ok\n",
            };
            if write.write_all(answer).is_err() {
                return;
            }
        } else if let Some(code) = line.strip_prefix("exit ") {
            progress(if code == "0" && !reported_failure {
                HookStatus::Finished
            } else {
                HookStatus::Failed
            });
            let _ = write.shutdown(Shutdown::Both);
            return;
        }
    }
}

/// The proxy's line to the app; `None` when it cannot be reached, and the
/// hook then runs without it (no progress, no HookFailed dialog).
struct ProxyConnection {
    write: TcpStream,
    reader: BufReader<TcpStream>,
}

impl ProxyConnection {
    fn connect(hook_name: &str) -> Option<Self> {
        let port: u16 = std::env::var(PORT_VAR).ok()?.parse().ok()?;
        let token = std::env::var(TOKEN_VAR).ok()?;
        let mut write = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).ok()?;
        let reader = BufReader::new(write.try_clone().ok()?);
        write
            .write_all(format!("{token} {hook_name}\n").as_bytes())
            .ok()?;
        Some(Self { write, reader })
    }

    fn read_line(&mut self) -> Option<String> {
        let mut line = String::new();
        (self.reader.read_line(&mut line).ok()? > 0).then(|| line.trim_end().to_string())
    }
}

/// Node's name for a signal (`child.on('close', (code, signal))`).
#[cfg(unix)]
fn signal_name(signal: i32) -> String {
    match signal {
        libc::SIGHUP => "SIGHUP".into(),
        libc::SIGINT => "SIGINT".into(),
        libc::SIGQUIT => "SIGQUIT".into(),
        libc::SIGILL => "SIGILL".into(),
        libc::SIGABRT => "SIGABRT".into(),
        libc::SIGBUS => "SIGBUS".into(),
        libc::SIGFPE => "SIGFPE".into(),
        libc::SIGKILL => "SIGKILL".into(),
        libc::SIGSEGV => "SIGSEGV".into(),
        libc::SIGPIPE => "SIGPIPE".into(),
        libc::SIGALRM => "SIGALRM".into(),
        libc::SIGTERM => "SIGTERM".into(),
        libc::SIGUSR1 => "SIGUSR1".into(),
        libc::SIGUSR2 => "SIGUSR2".into(),
        other => format!("SIG{other}"),
    }
}

/// The stand-in hook (`corvene --hook-proxy <hook path> <args…>`, GHD's
/// `process-proxy` with `createHooksProxy` behind it): tells the app the
/// hook started, runs it with `git hook run`, copies its output to stderr,
/// asks the app what to do when it fails and returns the exit code to end
/// with.
pub fn run_proxy(hook_path: &OsStr, args: &[OsString]) -> i32 {
    let started = Instant::now();
    let hook_name = Path::new(hook_path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let hook_name = hook_name
        .strip_suffix(".exe")
        .unwrap_or(&hook_name)
        .to_string();
    let mut stderr = std::io::stderr();
    let _ = writeln!(stderr, "Running {hook_name} hook...");
    let mut conn = ProxyConnection::connect(&hook_name);
    if let Some(c) = conn.as_mut()
        && c.read_line().as_deref() != Some("run")
    {
        let _ = writeln!(stderr, "hook {hook_name} aborted");
        return 1;
    }

    // git hands a hook its input on stdin (pre-push's refs) or /dev/null
    let mut input = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut input);
    let stdin_path = (!input.is_empty()).then(|| {
        let dir = std::env::var_os(DIR_VAR)
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        dir.join(format!("{hook_name}-{}.stdin", random_hex(8)))
    });
    if let Some(path) = &stdin_path
        && let Err(err) = std::fs::write(path, &input)
    {
        let _ = writeln!(stderr, "Failed to buffer stdin for {hook_name} hook: {err}");
        if let Some(c) = conn.as_mut() {
            let _ = c.write.write_all(b"exit 1\n");
        }
        return 1;
    }

    let mut git_args: Vec<OsString> = vec!["hook".into(), "run".into(), hook_name.clone().into()];
    // `pre-auto-gc`: git may run it without the repository having one
    if hook_name == "pre-auto-gc" {
        git_args.push("--ignore-missing".into());
    }
    if let Some(path) = &stdin_path {
        let mut arg = OsString::from("--to-stdin=");
        arg.push(path);
        git_args.push(arg);
    }
    git_args.push("--".into());
    git_args.extend(args.iter().cloned());

    let git = std::env::var_os(GIT_VAR).unwrap_or_else(|| "git".into());
    let mut cmd = std::process::Command::new(git);
    cmd.args(&git_args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    // the proxy is the GUI binary and has no console: without this git
    // opens a console window of its own for every hook (the window
    // desktop/desktop#23104 sees flash), where Corvene starts every other
    // git with none
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(crate::CREATE_NO_WINDOW);
    }
    for key in EXCLUDED_ENV_VARS {
        cmd.env_remove(key);
    }
    // GITHUB_DESKTOP lets hooks know they're run from GitHub Desktop
    // (desktop/desktop#19001); hooks written for it work here too
    cmd.env("GITHUB_DESKTOP", "1");
    let mut terminal_output = Vec::new();
    let status = match cmd.spawn() {
        Ok(mut child) => {
            let (tx, pipe) = std::sync::mpsc::sync_channel(PIPE_BACKLOG);
            if let Some(hook_stderr) = child.stderr.take() {
                read_pipe(StreamedPipe::Stderr, boxed_pipe(hook_stderr), tx);
            } else {
                drop(tx);
            }
            // a process the hook left running keeps the pipe open: once the
            // hook is done, git goes on without it (desktop/desktop#23104)
            let ended = receive_until_exit(
                || !matches!(child.try_wait(), Ok(None)),
                &pipe,
                |(_, chunk): PipeChunk| {
                    let _ = stderr.write_all(&chunk);
                    let _ = stderr.flush();
                    terminal_output.extend_from_slice(&chunk);
                },
            );
            if !ended {
                debug!(hook = %hook_name, "{OUTPUT_HELD}");
            }
            child.wait()
        }
        Err(err) => Err(err),
    };
    if let Some(path) = &stdin_path {
        let _ = std::fs::remove_file(path);
    }
    let (code, signal) = match &status {
        Ok(status) => {
            #[cfg(unix)]
            let signal = {
                use std::os::unix::process::ExitStatusExt;
                status.signal().map(signal_name)
            };
            #[cfg(not(unix))]
            let signal: Option<String> = None;
            (status.code(), signal)
        }
        Err(err) => {
            let _ = writeln!(stderr, "Failed to run {hook_name} hook: {err}");
            (Some(1), None)
        }
    };

    let dur = format!("after {:.2}s", started.elapsed().as_secs_f64());
    let prefix = format!("{hook_name} hook");
    let termination = match (&signal, code) {
        (Some(signal), _) => format!("{prefix} killed by signal {signal} {dur}"),
        (None, Some(code)) if code != 0 => format!("{prefix} failed with code {code} {dur}"),
        _ => format!("{prefix} done {dur}"),
    };
    // the dialog shows the termination message too
    let mut ignore = false;
    if let Some(code) = code
        && code != 0
        && !IGNORED_ON_FAILURE_HOOKS.contains(&hook_name.as_str())
        && let Some(c) = conn.as_mut()
    {
        terminal_output.extend_from_slice(format!("{termination}\n").as_bytes());
        let header = format!("failed {}\n", terminal_output.len());
        if c.write.write_all(header.as_bytes()).is_ok()
            && c.write.write_all(&terminal_output).is_ok()
        {
            ignore = c.read_line().as_deref() == Some("ignore");
        }
    }
    // `1114-hook-results`: the app shows the outcome of a hook that cannot
    // stop the operation; once it has, the hook ends as a success
    if REPORTED_HOOKS.contains(&hook_name.as_str())
        && let Some(c) = conn.as_mut()
    {
        let mut body = terminal_output.clone();
        body.extend_from_slice(format!("{termination}\n").as_bytes());
        let code_field = match code {
            Some(code) if signal.is_none() => code.to_string(),
            _ => "signal".to_string(),
        };
        let header = format!(
            "result {code_field} {} {}\n",
            started.elapsed().as_millis(),
            body.len()
        );
        if c.write.write_all(header.as_bytes()).is_ok() && c.write.write_all(&body).is_ok() {
            ignore = c.read_line().as_deref() == Some("shown");
        }
    }
    let _ = writeln!(stderr, "{termination}");
    if ignore && !REPORTED_HOOKS.contains(&hook_name.as_str()) {
        let _ = writeln!(stderr, "{hook_name} hook failure ignored by user");
    }
    let exit_code = if ignore { 0 } else { code.unwrap_or(1) };
    if let Some(c) = conn.as_mut() {
        let _ = c.write.write_all(format!("exit {exit_code}\n").as_bytes());
        // waits for the app to take the exit in, so progress lands before
        // git goes on, but not for an app that stopped answering
        let _ = c.write.set_read_timeout(Some(EXIT_ACK_TIMEOUT));
        let _ = c.read_line();
    }
    exit_code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_like_git() {
        assert_eq!(sq_quote("/tmp/a"), "'/tmp/a'");
        assert_eq!(sq_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn random_hex_has_the_length_asked_for() {
        assert_eq!(random_hex(8).len(), 8);
        assert_eq!(random_hex(32).len(), 32);
        assert_ne!(random_hex(32), random_hex(32));
    }

    #[test]
    fn finds_executable_known_hooks_in_the_filter() {
        let dir = tempfile::tempdir().unwrap();
        let git = Arc::new(crate::find_git().unwrap());
        let repo = dir.path();
        GitCommand::new(git.clone())
            .args(["init", "-q"])
            .current_dir(repo)
            .run()
            .unwrap();
        let hooks = repo.join(".git/hooks");
        for (name, mode) in [
            ("pre-commit", 0o755),
            ("commit-msg", 0o644),
            ("pre-push", 0o755),
            ("not-a-hook", 0o755),
        ] {
            let path = hooks.join(name);
            std::fs::write(&path, "#!/bin/sh\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            }
            #[cfg(not(unix))]
            let _ = mode;
        }
        let found = get_repo_hooks(git, repo, COMMIT_HOOKS);
        if cfg!(unix) {
            assert_eq!(found, vec!["pre-commit".to_string()]);
        }
    }

    /// The app's side of the protocol against a scripted proxy.
    #[test]
    fn the_server_reports_progress_and_asks_on_failure() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let callbacks = HookCallbacks {
            on_hook_progress: Some({
                let events = events.clone();
                Arc::new(move |p: HookProgress| events.lock().unwrap().push(format!("{p:?}")))
            }),
            on_hook_failure: Some({
                let events = events.clone();
                Arc::new(move |name: &str, output: Vec<u8>| {
                    events.lock().unwrap().push(format!(
                        "failure {name} {}",
                        String::from_utf8_lossy(&output)
                    ));
                    HookFailureResolution::Ignore
                })
            }),
            on_hook_result: None,
        };
        let dir = std::env::temp_dir().join(format!("corvene-hooks-test-{}", random_hex(8)));
        std::fs::create_dir(&dir).unwrap();
        let session = Session::serve(dir.clone(), callbacks, PathBuf::from("git")).unwrap();
        let mut conn = TcpStream::connect((Ipv4Addr::LOCALHOST, session.port)).unwrap();
        let mut reader = BufReader::new(conn.try_clone().unwrap());
        let mut line = String::new();
        conn.write_all(format!("{} pre-commit\n", session.shared.token).as_bytes())
            .unwrap();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line, "run\n");
        conn.write_all(b"failed 3\nbad").unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line, "ignore\n");
        conn.write_all(b"exit 0\n").unwrap();
        line.clear();
        let _ = reader.read_line(&mut line);
        drop(session);
        assert!(!dir.exists());
        assert_eq!(
            *events.lock().unwrap(),
            vec![
                "HookProgress { hook_name: \"pre-commit\", status: Started }".to_string(),
                "failure pre-commit bad".to_string(),
                "HookProgress { hook_name: \"pre-commit\", status: Finished }".to_string(),
            ]
        );
    }

    /// `1114-hook-results`: a post-merge outcome reaches the result callback
    /// split into output and closing line.
    #[test]
    fn the_server_takes_hook_results() {
        let results = Arc::new(Mutex::new(Vec::new()));
        let callbacks = HookCallbacks {
            on_hook_result: Some({
                let results = results.clone();
                Arc::new(move |r: HookResult| results.lock().unwrap().push(r))
            }),
            ..Default::default()
        };
        let dir = std::env::temp_dir().join(format!("corvene-hooks-test-{}", random_hex(8)));
        std::fs::create_dir(&dir).unwrap();
        let session = Session::serve(dir, callbacks, PathBuf::from("git")).unwrap();
        let mut conn = TcpStream::connect((Ipv4Addr::LOCALHOST, session.port)).unwrap();
        let mut reader = BufReader::new(conn.try_clone().unwrap());
        let mut line = String::new();
        conn.write_all(format!("{} post-merge\n", session.shared.token).as_bytes())
            .unwrap();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line, "run\n");
        let body = "Bad data\nmore\npost-merge hook failed with code 1 after 0.20s\n";
        conn.write_all(format!("result 1 200 {}\n{body}", body.len()).as_bytes())
            .unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line, "shown\n");
        conn.write_all(b"exit 0\n").unwrap();
        line.clear();
        let _ = reader.read_line(&mut line);
        drop(session);
        let results = results.lock().unwrap();
        assert_eq!(
            *results,
            vec![HookResult {
                hook_name: "post-merge".into(),
                code: Some(1),
                duration: std::time::Duration::from_millis(200),
                output: b"Bad data\nmore\n".to_vec(),
                termination: "post-merge hook failed with code 1 after 0.20s".into(),
            }]
        );
        assert!(results[0].failed());
    }

    #[test]
    fn a_wrong_token_is_turned_away() {
        let dir = std::env::temp_dir().join(format!("corvene-hooks-test-{}", random_hex(8)));
        std::fs::create_dir(&dir).unwrap();
        let session = Session::serve(dir, HookCallbacks::default(), PathBuf::from("git")).unwrap();
        let mut conn = TcpStream::connect((Ipv4Addr::LOCALHOST, session.port)).unwrap();
        conn.write_all(b"nope pre-commit\n").unwrap();
        let mut line = String::new();
        let n = BufReader::new(conn).read_line(&mut line).unwrap_or(0);
        assert_eq!(n, 0);
    }

    #[test]
    fn the_session_env_points_git_at_the_stand_ins() {
        let dir = std::env::temp_dir().join(format!("corvene-hooks-test-{}", random_hex(8)));
        std::fs::create_dir(&dir).unwrap();
        let session =
            Session::serve(dir.clone(), HookCallbacks::default(), PathBuf::from("git")).unwrap();
        let env = session.env(Some(OsStr::new("'user.name=x'")));
        assert_eq!(env[0].0, "GIT_CONFIG_PARAMETERS");
        assert_eq!(
            env[0].1,
            OsString::from(format!("'user.name=x' 'core.hooksPath={}'", dir.display()))
        );
    }
}
