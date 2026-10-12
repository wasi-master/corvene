//! The single choke point for spawning `git` (never spawn it elsewhere).
//! Environment mirrors GitHub Desktop's `lib/git/core.ts`.
//!
//! A failed command is GitHub Desktop's `GitError`: [`GitError::Failed`]
//! carries the command's terminal output (stdout and stderr, the last
//! [`TERMINAL_CAPACITY`] UTF-16 code units, untrimmed), which is the message
//! GitHub Desktop shows when it has no description for the error. GitHub
//! Desktop keeps the output in the order git wrote it; [`GitCommand::run`]
//! and the streamed runs collect stdout and stderr apart, so their output is
//! stdout followed by stderr ([`GitCommand::run_with_terminal_output`] keeps
//! the order). A known error the caller expects
//! ([`GitCommand::expected_errors`]) is looked for in stderr, then in stdout
//! (`parseError`), and is a result, not a failure. The known error of a
//! failure (`GitError::known`, `GitFailure::known`) is looked for in its
//! terminal output, the only text the error keeps: where stdout and stderr
//! name different known errors, the one listed first wins rather than
//! stderr's (`.docs/deviations.md`).

use std::ffi::{OsStr, OsString};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tracing::{debug, warn};

use crate::detect::GitBinary;
use crate::error::{GitError, Result};
use crate::git_errors::{KnownGitError, known_git_error};
use crate::terminal::{LiveOutput, TERMINAL_CAPACITY, TerminalBuffer, TerminalOutputCallback};

#[derive(Debug)]
pub struct GitOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl GitOutput {
    pub fn stdout_string(&self) -> Result<String> {
        Ok(String::from_utf8(self.stdout.clone())?)
    }
}

/// Builder for one git invocation.
#[derive(Clone, Debug)]
pub struct GitCommand {
    bin: Arc<GitBinary>,
    args: Vec<OsString>,
    cwd: Option<PathBuf>,
    env: Vec<(OsString, OsString)>,
    /// Exit codes that are not failures (e.g. `diff --exit-code` → 1).
    ok_codes: Vec<i32>,
    /// Every exit code counts as success ([`GitCommand::allow_any_exit_code`]).
    any_code: bool,
    /// [`GitCommand::forget_streamed`]
    forget_streamed: bool,
    /// Bytes written to git's stdin (`commit -F -`, `update-index --stdin`).
    stdin: Option<Vec<u8>>,
    /// Variables removed from the inherited environment (`GIT_SEQUENCE_EDITOR`).
    env_removed: Vec<OsString>,
    /// Lets another thread stop a streamed command ([`GitCommand::cancel_token`]).
    cancel: Option<CancelToken>,
    /// [`GitCommand::timeout`]
    timeout: Option<Duration>,
    /// Known errors that are results ([`GitCommand::expected_errors`]).
    expected_errors: Vec<KnownGitError>,
    /// The hooks it intercepts ([`GitCommand::intercept_hooks`]).
    hooks: Option<crate::hooks::Interception>,
}

/// Stops a running [`GitCommand::run_streaming`] from another thread with
/// `SIGTERM`, so git runs its own cleanup (`git clone` removes the directory
/// it created). Cancelling before the command starts makes it stop at once.
///
/// Outside Windows and Android a command with a token runs in a process
/// group of its own and the signal goes to the whole group: `git pull` runs
/// its fetch as a child process that outlives a signal sent to `git pull`
/// alone. (Windows' `taskkill /T` ends the tree.)
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<CancelInner>);

#[derive(Debug, Default)]
struct CancelInner {
    cancelled: AtomicBool,
    /// The running git process, and whether it leads a process group of
    /// its own.
    pid: Mutex<Option<(u32, bool)>>,
}

impl PartialEq for CancelToken {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::SeqCst);
        if let Ok(pid) = self.0.pid.lock()
            && let Some((pid, group)) = *pid
        {
            terminate(pid, group);
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::SeqCst)
    }

    /// `pid` was started by [`own_process_group`] when `group`.
    fn attach(&self, pid: u32, group: bool) {
        if let Ok(mut live) = LIVE.lock() {
            live.push(self.clone());
        }
        if let Ok(mut slot) = self.0.pid.lock() {
            *slot = Some((pid, group));
            if self.is_cancelled() {
                terminate(pid, group);
            }
        }
    }

    fn detach(&self) {
        if let Ok(mut slot) = self.0.pid.lock() {
            *slot = None;
        }
        if let Ok(mut live) = LIVE.lock()
            && let Some(ix) = live.iter().position(|token| token == self)
        {
            live.swap_remove(ix);
        }
    }
}

/// The tokens of the git processes running now ([`terminate_all`]).
static LIVE: Mutex<Vec<CancelToken>> = Mutex::new(Vec::new());

/// Stop every git process Corvene started that is still running, with the
/// processes they started (a `git status` waits on one per submodule).
/// For quitting: git is not tied to Corvene's lifetime, and a command stuck
/// on a file that never arrives (a cloud-only file) would outlive it.
pub fn terminate_all() {
    let live = LIVE.lock().map(|live| live.clone()).unwrap_or_default();
    if !live.is_empty() {
        debug!(count = live.len(), "stopping running git processes");
    }
    for token in live {
        token.cancel();
    }
}

/// Puts a cancellable command in a process group of its own, so
/// [`CancelToken::cancel`] reaches the processes git starts; returns whether
/// it did. The command also starts with no signal blocked: Rust's `Command`
/// hands the spawning thread's signal mask on, and the background executor's
/// threads (GCD on macOS) block `SIGTERM`, which left git and the helpers it
/// starts deaf to the token.
#[cfg(all(unix, not(target_os = "android")))]
fn own_process_group(cmd: &mut Command) -> bool {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
    // SAFETY: `sigemptyset` and `pthread_sigmask` are async-signal-safe and
    // touch only the child's stack, as `pre_exec` requires
    unsafe {
        cmd.pre_exec(|| {
            let mut empty = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
            libc::sigemptyset(empty.as_mut_ptr());
            libc::pthread_sigmask(libc::SIG_SETMASK, empty.as_ptr(), std::ptr::null_mut());
            Ok(())
        });
    }
    true
}

#[cfg(not(unix))]
fn own_process_group(_cmd: &mut Command) -> bool {
    false
}

thread_local! {
    /// The token [`with_cancel_token`] gives the git commands of this thread.
    static SCOPED_CANCEL: std::cell::RefCell<Option<CancelToken>> =
        const { std::cell::RefCell::new(None) };
}

/// Run `f` with `token` stopping every git command it starts on the current
/// thread that has no token of its own (a fetch, pull or push with the
/// commands around it). Once `token` is cancelled, the commands still to
/// come stop at once.
pub fn with_cancel_token<R>(token: &CancelToken, f: impl FnOnce() -> R) -> R {
    /// Puts the previous token back, also when `f` panics.
    struct Restore(Option<CancelToken>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            SCOPED_CANCEL.with(|scoped| *scoped.borrow_mut() = previous);
        }
    }
    let _restore = Restore(SCOPED_CANCEL.with(|scoped| scoped.replace(Some(token.clone()))));
    f()
}

fn scoped_cancel_token() -> Option<CancelToken> {
    SCOPED_CANCEL.with(|scoped| scoped.borrow().clone())
}

/// A token for streamed commands that were given none: lets a caller stop
/// whatever network command runs (Android's headless fetch, when the
/// application itself is opened).
static DEFAULT_CANCEL: Mutex<Option<CancelToken>> = Mutex::new(None);

pub fn set_default_cancel_token(token: Option<CancelToken>) {
    if let Ok(mut slot) = DEFAULT_CANCEL.lock() {
        *slot = token;
    }
}

fn default_cancel_token() -> Option<CancelToken> {
    DEFAULT_CANCEL.lock().ok().and_then(|slot| slot.clone())
}

/// Windows has no SIGTERM. `taskkill /T` ends the whole tree: `cmd\git.exe`
/// is a wrapper around the real git, which in turn runs the remote helper.
#[cfg(windows)]
fn terminate(pid: u32, _group: bool) {
    use std::os::windows::process::CommandExt;
    let _ = Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .creation_flags(crate::CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(not(windows))]
fn terminate(pid: u32, group: bool) {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return;
    };
    // SAFETY: plain kill(2) with no memory access; the pid is the running
    // child's (the token forgets it as soon as `wait` reaped the process),
    // and with `group` also the id of the group it leads
    unsafe {
        libc::kill(if group { -pid } else { pid }, libc::SIGTERM);
    }
}

/// Cancels a command's token once its time is up ([`GitCommand::timeout`]).
#[cfg(not(target_os = "android"))]
struct Watchdog {
    done: std::sync::mpsc::Sender<()>,
    fired: Arc<AtomicBool>,
}

#[cfg(not(target_os = "android"))]
impl Watchdog {
    fn start(limit: Duration, token: CancelToken) -> Self {
        let (done, finished) = std::sync::mpsc::channel::<()>();
        let fired = Arc::new(AtomicBool::new(false));
        let flag = fired.clone();
        let spawned = std::thread::Builder::new()
            .name("git-timeout".into())
            .spawn(move || {
                if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
                    finished.recv_timeout(limit)
                {
                    flag.store(true, Ordering::SeqCst);
                    token.cancel();
                }
            });
        if let Err(err) = spawned {
            warn!(%err, "no timeout for git: could not start its thread");
        }
        Watchdog { done, fired }
    }

    /// Ends the wait; returns whether the time ran out first.
    fn stop(self) -> bool {
        let _ = self.done.send(());
        self.fired.load(Ordering::SeqCst)
    }
}

/// Process-wide toggle for `-c credential.helper=manager` (set per network
/// operation by the dispatcher for non-GitHub remotes).
static CREDENTIAL_HELPER: AtomicBool = AtomicBool::new(false);

pub fn set_credential_helper(enabled: bool) {
    CREDENTIAL_HELPER.store(enabled, Ordering::Relaxed);
}

/// Flag `876-git-spawn-error-details`: a missing working directory is named
/// instead of "could not run git: Not a directory" (set by the dispatcher).
static EXPLAIN_MISSING_WORKDIR: AtomicBool = AtomicBool::new(false);

pub fn set_explain_missing_workdir(enabled: bool) {
    EXPLAIN_MISSING_WORKDIR.store(enabled, Ordering::Relaxed);
}

/// Seconds a network command's HTTP transfer may stay below 1 byte/s before
/// git aborts it (`GIT_HTTP_LOW_SPEED_LIMIT` / `GIT_HTTP_LOW_SPEED_TIME`);
/// 0 = no limit, as in GHD. Set by the dispatcher from flag
/// `network-stall-timeout`.
static LOW_SPEED_TIME: AtomicU32 = AtomicU32::new(0);

pub fn set_network_stall_timeout(seconds: u32) {
    LOW_SPEED_TIME.store(seconds, Ordering::Relaxed);
}

thread_local! {
    /// Variables [`with_env`] adds to the git commands of this thread.
    static SCOPED_ENV: std::cell::RefCell<Vec<(OsString, OsString)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Run `f` with `env` added to the environment of every git command it
/// starts on the current thread, as if the variables were in the process
/// environment: a command's own [`GitCommand::env`] /
/// [`GitCommand::env_remove`] still win. Commands started on other threads,
/// and after `f` returns, do not see them.
///
/// This is how GitHub Desktop's tests that set `process.env` around one call
/// (`GIT_CONFIG_PARAMETERS`, …) are ported without touching the environment
/// of the other tests, which run on parallel threads of the same process.
pub fn with_env<R>(env: &[(&str, &str)], f: impl FnOnce() -> R) -> R {
    /// Drops the variables again, also when `f` panics.
    struct Restore(usize);
    impl Drop for Restore {
        fn drop(&mut self) {
            SCOPED_ENV.with(|scoped| scoped.borrow_mut().truncate(self.0));
        }
    }
    let _restore = SCOPED_ENV.with(|scoped| {
        let mut scoped = scoped.borrow_mut();
        let len = scoped.len();
        scoped.extend(
            env.iter()
                .map(|(k, v)| (OsString::from(k), OsString::from(v))),
        );
        Restore(len)
    });
    f()
}

/// The [`with_env`] variables of the current thread, for a git command this
/// thread has another thread run (`lfs_progress::run_with_progress`).
pub(crate) fn scoped_env() -> Vec<(String, String)> {
    SCOPED_ENV.with(|scoped| {
        scoped
            .borrow()
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.to_string_lossy().into_owned(),
                )
            })
            .collect()
    })
}

/// Told when a network command starts (`true`) and ends (`false`): Android
/// keeps the process alive with a foreground service while one runs.
static NETWORK_OBSERVER: std::sync::OnceLock<fn(bool)> = std::sync::OnceLock::new();

pub fn set_network_observer(observer: fn(bool)) {
    let _ = NETWORK_OBSERVER.set(observer);
}

/// Reports a running network command to [`NETWORK_OBSERVER`] until dropped.
struct NetworkGuard(fn(bool));

impl NetworkGuard {
    fn for_command(command: &GitCommand) -> Option<Self> {
        let observer = *NETWORK_OBSERVER.get()?;
        is_network_command(command.subcommand()).then(|| {
            observer(true);
            Self(observer)
        })
    }
}

impl Drop for NetworkGuard {
    fn drop(&mut self) {
        (self.0)(false);
    }
}

/// Commands that talk to a remote.
fn is_network_command(arg: Option<&OsString>) -> bool {
    arg.is_some_and(|a| {
        matches!(
            a.to_str(),
            Some("fetch" | "pull" | "push" | "clone" | "ls-remote")
        )
    })
}

impl GitCommand {
    pub fn new(bin: Arc<GitBinary>) -> Self {
        Self {
            bin,
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            ok_codes: vec![0],
            any_code: false,
            forget_streamed: false,
            stdin: None,
            env_removed: Vec::new(),
            cancel: None,
            timeout: None,
            expected_errors: Vec::new(),
            hooks: None,
        }
    }

    /// Corvene: [`GitCommand::run`] stops git (and what it started) after
    /// `limit` and fails with [`GitError::TimedOut`]. GHD waits for every
    /// command however long it takes.
    pub fn timeout(mut self, limit: Duration) -> Self {
        self.timeout = Some(limit);
        self
    }

    /// GHD `IGitExecutionOptions.interceptHooks`: while Settings › Git ›
    /// Hooks is on, the repository's hooks among `names` run through the
    /// hooks proxy (`crate::hooks`), with the callbacks
    /// [`crate::hooks::with_hook_callbacks`] set on this thread.
    pub fn intercept_hooks(mut self, names: &[&str]) -> Self {
        self.hooks = crate::hooks::interception(names);
        self
    }

    /// Runs `f` with the hooks proxy set up when the command intercepts
    /// hooks the repository has (GHD `withHooksEnv`), else with `self`.
    fn with_hooks<R>(&self, f: impl FnOnce(&Self) -> R) -> R {
        let Some(session) = self
            .hooks
            .as_ref()
            .and_then(|hooks| hooks.start(&self.bin, self.cwd.as_deref()))
        else {
            return f(self);
        };
        let mut cmd = self.clone();
        cmd.hooks = None;
        let existing = self
            .env
            .iter()
            .rev()
            .find(|(k, _)| k == "GIT_CONFIG_PARAMETERS")
            .map(|(_, v)| v.as_os_str());
        cmd.env.extend(session.env(existing));
        let result = f(&cmd);
        drop(session);
        result
    }

    /// GHD `IGitExecutionOptions.expectedErrors`: when the command exits
    /// with a code that is not a success and its output (stderr, then
    /// stdout) is one of these errors, it returns its [`GitOutput`] instead
    /// of failing.
    pub fn expected_errors(mut self, errors: impl IntoIterator<Item = KnownGitError>) -> Self {
        self.expected_errors.extend(errors);
        self
    }

    /// Stop the streamed command when `token` is cancelled.
    pub fn cancel_token(mut self, token: CancelToken) -> Self {
        self.cancel = Some(token);
        self
    }

    /// The command's token, else the one [`with_cancel_token`] set for this
    /// thread.
    fn effective_cancel(&self) -> Option<CancelToken> {
        self.cancel.clone().or_else(scoped_cancel_token)
    }

    /// Gives the command this thread's [`with_cancel_token`] token, for a
    /// command another thread runs (`lfs_progress::run_with_progress`).
    pub(crate) fn with_scoped_cancel(mut self) -> Self {
        if self.cancel.is_none() {
            self.cancel = scoped_cancel_token();
        }
        self
    }

    /// Drop an inherited environment variable for this invocation.
    pub fn env_remove(mut self, key: impl AsRef<OsStr>) -> Self {
        self.env_removed.push(key.as_ref().to_os_string());
        self
    }

    pub fn stdin(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(bytes.into());
        self
    }

    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_os_string());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args
            .extend(args.into_iter().map(|a| a.as_ref().to_os_string()));
        self
    }

    pub fn current_dir(mut self, dir: impl AsRef<Path>) -> Self {
        self.cwd = Some(dir.as_ref().to_path_buf());
        self
    }

    pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.env
            .push((key.as_ref().to_os_string(), value.as_ref().to_os_string()));
        self
    }

    pub fn allow_exit_code(mut self, code: i32) -> Self {
        self.ok_codes.push(code);
        self
    }

    /// [`GitError::Spawn`], or with flag `876` [`GitError::MissingWorkdir`]
    /// when the working directory is gone or not a folder (the OS reports
    /// that as "No such file or directory" / "Not a directory", which reads
    /// as if git were missing).
    fn spawn_error(&self, err: std::io::Error) -> GitError {
        use std::io::ErrorKind;
        match &self.cwd {
            Some(cwd)
                if EXPLAIN_MISSING_WORKDIR.load(Ordering::Relaxed)
                    && matches!(err.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory)
                    && !cwd.is_dir() =>
            {
                GitError::MissingWorkdir(cwd.clone())
            }
            _ => GitError::Spawn(err),
        }
    }

    /// Return [`GitOutput`] for whatever code git exits with, so the caller
    /// reads `status` itself (dugite's `exec`, which the ported GitHub
    /// Desktop tests assert on). A process killed by a signal, without an
    /// exit code, is still an error.
    pub fn allow_any_exit_code(mut self) -> Self {
        self.any_code = true;
        self
    }

    /// A streamed run hands each line to its callback only: the output it
    /// returns has no streamed text (a long `log --numstat` would
    /// otherwise be held in memory twice).
    pub fn forget_streamed(mut self) -> Self {
        self.forget_streamed = true;
        self
    }

    /// The git subcommand: the first argument after any `-c name=value`
    /// pairs (clone always starts with `-c init.defaultBranch=…`).
    fn subcommand(&self) -> Option<&OsString> {
        let mut args = self.args.iter();
        let mut subcommand = args.next();
        while subcommand.is_some_and(|arg| arg == "-c") {
            subcommand = args.nth(1);
        }
        subcommand
    }

    fn exit_ok(&self, code: Option<i32>) -> bool {
        code.is_some_and(|c| self.any_code || self.ok_codes.contains(&c))
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.bin.path);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(crate::CREATE_NO_WINDOW);
        }
        // Settings › Advanced › Use Git Credential Manager: `-c credential.helper=manager`
        // for the network commands (GHD `useExternalCredentialHelper`).
        let network = is_network_command(self.subcommand());
        if CREDENTIAL_HELPER.load(Ordering::Relaxed) && network {
            cmd.args(["-c", "credential.helper=manager"]);
        }
        cmd.args(&self.args);
        if let Some(cwd) = &self.cwd {
            cmd.current_dir(cwd);
        }
        // Settings › Git › Hooks: the user's login-shell environment first,
        // so hooks see the same PATH as a terminal.
        if let Some(env) = crate::hook_env::hook_env() {
            for (k, v) in env {
                cmd.env(k, v);
            }
        }
        // `with_env`: part of the inherited environment for this thread
        SCOPED_ENV.with(|scoped| {
            for (k, v) in scoped.borrow().iter() {
                cmd.env(k, v);
            }
        });
        // GHD: never let git prompt on a terminal; force stable English output.
        cmd.env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "en_US.UTF-8")
            .env("GIT_OPTIONAL_LOCKS", "0");
        let stall = LOW_SPEED_TIME.load(Ordering::Relaxed);
        if network && stall > 0 {
            cmd.env("GIT_HTTP_LOW_SPEED_LIMIT", "1")
                .env("GIT_HTTP_LOW_SPEED_TIME", stall.to_string());
        }
        for k in &self.env_removed {
            cmd.env_remove(k);
        }
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        cmd.stdin(if self.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        cmd
    }

    fn describe(&self) -> String {
        self.args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Run to completion, capturing stdout/stderr. Blocking: call from a
    /// background thread (GPUI `background_spawn`). A [`cancel_token`]
    /// (`Self::cancel_token`) stops the process and makes this return
    /// [`GitError::Cancelled`] (not on Android, where `run` has no child to
    /// signal before it exits).
    pub fn run(&self) -> Result<GitOutput> {
        self.with_hooks(Self::run_now)
    }

    fn run_now(&self) -> Result<GitOutput> {
        let started = Instant::now();
        let _network = NetworkGuard::for_command(self);
        let args = self.describe();
        // Android: without `fork` (`spawn.rs`)
        #[cfg(target_os = "android")]
        let output = {
            let mut child = crate::spawn::spawn(self.command(), self.stdin.is_some())
                .map_err(|err| self.spawn_error(err))?;
            if let (Some(bytes), Some(mut stdin)) = (&self.stdin, child.stdin.take()) {
                use std::io::Write;
                let _ = stdin.write_all(bytes);
            }
            child.wait_with_output().map_err(GitError::Spawn)?
        };
        // every command gets a token, so [`terminate_all`] reaches it
        let cancel = Some(self.effective_cancel().unwrap_or_default());
        #[cfg(not(target_os = "android"))]
        let (output, timed_out) = {
            let mut cmd = self.command();
            let group = own_process_group(&mut cmd);
            let mut child = cmd
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|err| self.spawn_error(err))?;
            if let Some(token) = &cancel {
                token.attach(child.id(), group);
            }
            let watchdog = self
                .timeout
                .zip(cancel.clone())
                .map(|(limit, token)| Watchdog::start(limit, token));
            if let (Some(bytes), Some(mut stdin)) = (&self.stdin, child.stdin.take()) {
                use std::io::Write;
                // git may exit early; a broken pipe is then reported via the exit code
                let _ = stdin.write_all(bytes);
            }
            let output = child.wait_with_output();
            if let Some(token) = &cancel {
                token.detach();
            }
            let timed_out = watchdog.is_some_and(Watchdog::stop);
            (output.map_err(GitError::Spawn)?, timed_out)
        };
        #[cfg(target_os = "android")]
        let timed_out = false;
        if timed_out {
            let seconds = self.timeout.map_or(0, |limit| limit.as_secs());
            warn!(git = %args, seconds, "git timed out");
            return Err(GitError::TimedOut { args, seconds });
        }
        if cancel.as_ref().is_some_and(CancelToken::is_cancelled) {
            debug!(git = %args, "git cancelled");
            return Err(GitError::Cancelled(args));
        }
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        debug!(
            git = %args,
            cwd = ?self.cwd,
            code = output.status.code(),
            ms = started.elapsed().as_millis(),
            "git finished"
        );
        self.finish(
            args,
            output.status,
            output.stdout,
            stderr,
            |stdout, stderr| terminal_output(stdout, stderr.as_bytes()),
        )
    }

    /// The end of every run: the output, or GHD's `GitError` when the exit
    /// code is not a success and the error is not expected. `terminal`
    /// builds the error's terminal output from stdout and stderr.
    fn finish(
        &self,
        args: String,
        status: ExitStatus,
        stdout: Vec<u8>,
        stderr: String,
        terminal: impl FnOnce(&[u8], &str) -> String,
    ) -> Result<GitOutput> {
        let code = status.code();
        if self.exit_ok(code) {
            return Ok(GitOutput {
                status,
                stdout,
                stderr,
            });
        }
        if !self.expected_errors.is_empty() {
            let known = known_git_error(&stderr)
                .or_else(|| known_git_error(&String::from_utf8_lossy(&stdout)));
            if known.is_some_and(|known| self.expected_errors.contains(&known)) {
                return Ok(GitOutput {
                    status,
                    stdout,
                    stderr,
                });
            }
        }
        let output = terminal(&stdout, &stderr);
        // GHD logs even less of the output than it keeps
        warn!(git = %args, code, output = %js_tail(&output, 1024), "git failed");
        Err(GitError::Failed {
            args,
            code,
            stderr: output,
        })
    }

    /// Run with `--progress`-style stderr streamed line by line (clone, fetch, push).
    /// Lines are split on `\n` and `\r` so percentage updates arrive as they happen.
    pub fn run_streaming(&self, on_stderr_line: impl FnMut(&str)) -> Result<GitOutput> {
        self.run_streaming_on(StreamedPipe::Stderr, on_stderr_line)
    }

    /// Like `run_streaming` but streams stdout (cherry-pick prints one
    /// `[branch sha] summary` line per applied commit there).
    pub fn run_streaming_stdout(&self, on_stdout_line: impl FnMut(&str)) -> Result<GitOutput> {
        self.run_streaming_on(StreamedPipe::Stdout, on_stdout_line)
    }

    fn run_streaming_on(&self, pipe: StreamedPipe, on_line: impl FnMut(&str)) -> Result<GitOutput> {
        self.with_hooks(|cmd| cmd.run_streaming_now(pipe, on_line))
    }

    fn run_streaming_now(
        &self,
        pipe: StreamedPipe,
        mut on_line: impl FnMut(&str),
    ) -> Result<GitOutput> {
        let started = Instant::now();
        let _network = NetworkGuard::for_command(self);
        let args = self.describe();
        let cancel = Some(
            self.effective_cancel()
                .or_else(default_cancel_token)
                .unwrap_or_default(),
        );
        #[cfg(not(target_os = "android"))]
        let (mut child, group) = {
            let mut cmd = self.command();
            let group = own_process_group(&mut cmd);
            let child = cmd
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|err| self.spawn_error(err))?;
            (child, group)
        };
        #[cfg(target_os = "android")]
        let (mut child, group) = (
            crate::spawn::spawn(self.command(), self.stdin.is_some())
                .map_err(|err| self.spawn_error(err))?,
            false,
        );
        if let Some(token) = &cancel {
            token.attach(child.id(), group);
        }
        // stdin on a thread of its own: git may fill the streamed pipe
        // before it has read everything (`update-index --verbose --stdin`)
        let stdin_thread = match (&self.stdin, child.stdin.take()) {
            (Some(bytes), Some(mut stdin)) => {
                let bytes = bytes.clone();
                Some(std::thread::spawn(move || {
                    use std::io::Write;
                    // git may exit early; a broken pipe shows in the exit code
                    let _ = stdin.write_all(&bytes);
                }))
            }
            _ => None,
        };

        // the pipe that is not streamed is drained on a thread so git never blocks
        let (Some(stdout_pipe), Some(stderr_pipe)) = (child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill();
            let _ = child.wait();
            if let Some(token) = &cancel {
                token.detach();
            }
            return Err(GitError::Spawn(std::io::Error::other(
                "git was started without its output pipes",
            )));
        };
        let (streamed, drained): (Box<dyn std::io::Read + Send>, Box<dyn std::io::Read + Send>) =
            match pipe {
                StreamedPipe::Stderr => (Box::new(stderr_pipe), Box::new(stdout_pipe)),
                StreamedPipe::Stdout => (Box::new(stdout_pipe), Box::new(stderr_pipe)),
            };
        let drain_thread = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let mut drained = drained;
            let _ = drained.read_to_end(&mut buf);
            buf
        });

        let mut streamed_all = String::new();
        let mut reader = BufReader::new(streamed);
        let mut chunk = Vec::new();
        loop {
            chunk.clear();
            // read until \r or \n
            let n = read_until_any(&mut reader, b"\r\n", &mut chunk);
            if n == 0 {
                break;
            }
            let line = String::from_utf8_lossy(&chunk);
            let line = line.trim_end_matches(['\r', '\n']);
            if !line.is_empty() {
                on_line(line);
            }
            if !self.forget_streamed {
                streamed_all.push_str(line);
                streamed_all.push('\n');
            }
        }

        let status = child.wait();
        if let Some(token) = &cancel {
            token.detach();
        }
        let status = status.map_err(GitError::Spawn)?;
        let drained = drain_thread.join().unwrap_or_default();
        if let Some(thread) = stdin_thread {
            let _ = thread.join();
        }
        if cancel.as_ref().is_some_and(CancelToken::is_cancelled) {
            debug!(git = %args, "git cancelled");
            return Err(GitError::Cancelled(args));
        }
        let (stdout, stderr) = match pipe {
            StreamedPipe::Stderr => (drained, streamed_all),
            StreamedPipe::Stdout => (
                streamed_all.into_bytes(),
                String::from_utf8_lossy(&drained).into_owned(),
            ),
        };
        debug!(
            git = %args,
            code = status.code(),
            ms = started.elapsed().as_millis(),
            "git finished (streamed)"
        );
        // GHD's `GitError` shows the combined terminal output: what a
        // pre-push hook prints to stdout (git passes it through) comes
        // before git's own error lines
        self.finish(args, status, stdout, stderr, |stdout, stderr| {
            terminal_output(stdout, stderr.as_bytes())
        })
    }

    /// GHD `git()` with `onTerminalOutputAvailable`: runs to completion like
    /// [`GitCommand::run`] and hands `on_terminal_output_available` a
    /// listener once git has started. A subscriber first receives the output
    /// buffered so far, then every chunk of stdout and stderr as git writes
    /// it; the callbacks run on this thread. The error of a failed command
    /// carries the output in the order git wrote it.
    pub fn run_with_terminal_output(
        &self,
        on_terminal_output_available: &TerminalOutputCallback,
    ) -> Result<GitOutput> {
        self.with_hooks(|cmd| cmd.run_with_terminal_output_now(on_terminal_output_available))
    }

    fn run_with_terminal_output_now(
        &self,
        on_terminal_output_available: &TerminalOutputCallback,
    ) -> Result<GitOutput> {
        let started = Instant::now();
        let _network = NetworkGuard::for_command(self);
        let args = self.describe();
        // a streamed run: the default token stops it too
        let cancel = Some(
            self.effective_cancel()
                .or_else(default_cancel_token)
                .unwrap_or_default(),
        );
        #[cfg(not(target_os = "android"))]
        let (mut child, group) = {
            let mut cmd = self.command();
            let group = own_process_group(&mut cmd);
            let child = cmd
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|err| self.spawn_error(err))?;
            (child, group)
        };
        #[cfg(target_os = "android")]
        let (mut child, group) = (
            crate::spawn::spawn(self.command(), self.stdin.is_some())
                .map_err(|err| self.spawn_error(err))?,
            false,
        );
        if let Some(token) = &cancel {
            token.attach(child.id(), group);
        }
        // stdin on a thread of its own: git may fill its output pipes first
        let stdin_thread = match (&self.stdin, child.stdin.take()) {
            (Some(bytes), Some(mut stdin)) => {
                let bytes = bytes.clone();
                Some(std::thread::spawn(move || {
                    use std::io::Write;
                    // git may exit early; a broken pipe shows in the exit code
                    let _ = stdin.write_all(&bytes);
                }))
            }
            _ => None,
        };
        let (tx, rx) = std::sync::mpsc::channel::<(StreamedPipe, Vec<u8>)>();
        let readers: Vec<_> = [
            child
                .stdout
                .take()
                .map(|pipe| (StreamedPipe::Stdout, Box::new(pipe) as Box<dyn Read + Send>)),
            child
                .stderr
                .take()
                .map(|pipe| (StreamedPipe::Stderr, Box::new(pipe) as Box<dyn Read + Send>)),
        ]
        .into_iter()
        .flatten()
        .map(|(kind, mut pipe)| {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    match pipe.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if tx.send((kind, buf[..n].to_vec())).is_err() {
                                break;
                            }
                        }
                    }
                }
            })
        })
        .collect();
        drop(tx);

        // GHD calls `onTerminalOutputAvailable` from `processCallback`, as
        // soon as the process exists
        let live = LiveOutput::new(TERMINAL_CAPACITY);
        on_terminal_output_available(live.listener());
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        // ends once both pipes are closed
        for (kind, chunk) in rx {
            match kind {
                StreamedPipe::Stdout => stdout.extend_from_slice(&chunk),
                StreamedPipe::Stderr => stderr.extend_from_slice(&chunk),
            }
            live.push(&chunk);
        }
        let status = child.wait();
        if let Some(token) = &cancel {
            token.detach();
        }
        let status = status.map_err(GitError::Spawn)?;
        for reader in readers {
            let _ = reader.join();
        }
        if let Some(thread) = stdin_thread {
            let _ = thread.join();
        }
        if cancel.as_ref().is_some_and(CancelToken::is_cancelled) {
            debug!(git = %args, "git cancelled");
            return Err(GitError::Cancelled(args));
        }
        debug!(
            git = %args,
            code = status.code(),
            ms = started.elapsed().as_millis(),
            "git finished (terminal output)"
        );
        let stderr = String::from_utf8_lossy(&stderr).into_owned();
        self.finish(args, status, stdout, stderr, |_, _| live.joined())
    }
}

/// GHD's terminal output of a command whose stdout and stderr were read
/// apart: stdout, then stderr, the last [`TERMINAL_CAPACITY`] code units.
fn terminal_output(stdout: &[u8], stderr: &[u8]) -> String {
    let mut buffer = TerminalBuffer::new(TERMINAL_CAPACITY);
    // a UTF-16 code unit takes at most 3 bytes of UTF-8: only the tail of a
    // long output can be kept, so only the tail is decoded
    let tail = |bytes: &[u8]| -> usize { bytes.len().saturating_sub(4 * TERMINAL_CAPACITY) };
    buffer.push(&stdout[tail(stdout)..]);
    buffer.push(&stderr[tail(stderr)..]);
    buffer.joined()
}

/// JavaScript's `s.slice(-units)`, at a character boundary.
fn js_tail(s: &str, units: usize) -> &str {
    let mut kept = 0;
    for (at, c) in s.char_indices().rev() {
        kept += c.len_utf16();
        if kept > units {
            return &s[at + c.len_utf8()..];
        }
    }
    s
}

#[derive(Clone, Copy)]
enum StreamedPipe {
    Stdout,
    Stderr,
}

fn read_until_any(reader: &mut impl BufRead, delims: &[u8], out: &mut Vec<u8>) -> usize {
    let mut total = 0;
    loop {
        let available = match reader.fill_buf() {
            Ok(buf) => buf,
            Err(_) => return total,
        };
        if available.is_empty() {
            return total;
        }
        match available.iter().position(|b| delims.contains(b)) {
            Some(i) => {
                out.extend_from_slice(&available[..=i]);
                reader.consume(i + 1);
                return total + i + 1;
            }
            None => {
                let n = available.len();
                out.extend_from_slice(available);
                reader.consume(n);
                total += n;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envs(cmd: &GitCommand) -> Vec<(String, String)> {
        cmd.command()
            .get_envs()
            .filter_map(|(k, v)| {
                Some((
                    k.to_string_lossy().into_owned(),
                    v?.to_string_lossy().into_owned(),
                ))
            })
            .filter(|(k, _)| k.starts_with("GIT_HTTP_LOW_SPEED"))
            .collect()
    }

    #[test]
    fn run_honours_the_cancel_token() {
        let git = Arc::new(crate::find_git().unwrap());
        let token = CancelToken::new();
        let version = GitCommand::new(git).arg("version");
        assert!(version.clone().cancel_token(token.clone()).run().is_ok());
        token.cancel();
        assert!(matches!(
            version.cancel_token(token).run(),
            Err(GitError::Cancelled(_))
        ));
    }

    /// A git that never finishes: an alias running `sleep` in a child
    /// process, as a `git status` waits on its submodules.
    fn hanging(git: Arc<GitBinary>) -> GitCommand {
        GitCommand::new(git).args(["-c", "alias.hang=!sleep 30", "hang"])
    }

    #[cfg(unix)]
    #[test]
    fn timeout_stops_git_and_what_it_started() {
        let git = Arc::new(crate::find_git().unwrap());
        let started = Instant::now();
        let result = hanging(git).timeout(Duration::from_millis(300)).run();
        assert!(
            matches!(result, Err(GitError::TimedOut { .. })),
            "{result:?}"
        );
        // `run` returns once the pipes close: the sleep was stopped too
        assert!(started.elapsed() < Duration::from_secs(20));
    }

    #[test]
    fn missing_working_directory_is_named() {
        let git = Arc::new(crate::find_git().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file");
        std::fs::write(&file, "").unwrap();
        let gone = dir.path().join("gone");
        let run = |cwd: &Path| {
            GitCommand::new(git.clone())
                .args(["status"])
                .current_dir(cwd)
                .run()
                .unwrap_err()
        };
        set_explain_missing_workdir(true);
        for cwd in [&gone, &file] {
            match run(cwd) {
                GitError::MissingWorkdir(path) => assert_eq!(&path, cwd),
                other => panic!("{other:?}"),
            }
        }
        set_explain_missing_workdir(false);
        assert!(matches!(run(&gone), GitError::Spawn(_)));
    }

    #[test]
    fn stall_timeout_applies_to_network_commands_only() {
        let git = Arc::new(crate::find_git().unwrap());
        set_network_stall_timeout(30);
        let fetch = GitCommand::new(git.clone()).args(["fetch", "origin"]);
        let status = GitCommand::new(git.clone()).args(["status"]);
        let mut fetch_env = envs(&fetch);
        fetch_env.sort();
        assert_eq!(
            fetch_env,
            vec![
                ("GIT_HTTP_LOW_SPEED_LIMIT".into(), "1".into()),
                ("GIT_HTTP_LOW_SPEED_TIME".into(), "30".into()),
            ]
        );
        assert!(envs(&status).is_empty());
        // the subcommand follows `-c name=value` pairs
        let clone = GitCommand::new(git.clone()).args(["-c", "init.defaultBranch=main", "clone"]);
        assert_eq!(envs(&clone).len(), 2);
        set_network_stall_timeout(0);
        assert!(envs(&fetch).is_empty());
    }

    #[test]
    fn failures_keep_the_output_and_expected_errors_are_results() {
        let git = Arc::new(crate::find_git().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let cmd = GitCommand::new(git)
            .args(["rev-parse", "--git-dir"])
            .current_dir(dir.path());
        let err = cmd.run().unwrap_err();
        let GitError::Failed { code, stderr, .. } = &err else {
            panic!("{err}");
        };
        assert_eq!(*code, Some(128));
        assert!(
            stderr.starts_with("fatal: not a git repository"),
            "{stderr}"
        );
        assert!(stderr.ends_with('\n'), "untrimmed: {stderr:?}");
        let output = cmd
            .clone()
            .expected_errors([KnownGitError::NotAGitRepository])
            .run()
            .unwrap();
        assert_eq!(output.status.code(), Some(128));
        assert!(
            cmd.expected_errors([KnownGitError::BadRevision])
                .run()
                .is_err()
        );
    }

    #[test]
    fn terminal_output_keeps_the_tail() {
        assert_eq!(terminal_output(b"out\n", b"err\n"), "out\nerr\n");
        let long = vec![b'a'; 5 * TERMINAL_CAPACITY];
        let kept = terminal_output(&long, b"end");
        assert_eq!(kept.len(), TERMINAL_CAPACITY);
        assert!(kept.ends_with("aend"));
        assert_eq!(js_tail("ab👋", 2), "👋");
        assert_eq!(js_tail("ab👋", 3), "b👋");
        assert_eq!(js_tail("ab", 9), "ab");
    }

    fn env_value(cmd: &GitCommand, key: &str) -> Option<String> {
        cmd.command()
            .get_envs()
            .find(|(k, _)| *k == OsStr::new(key))
            .and_then(|(_, v)| Some(v?.to_string_lossy().into_owned()))
    }

    #[test]
    fn with_env_reaches_this_threads_commands_inside_the_call_only() {
        const KEY: &str = "GIT_CONFIG_PARAMETERS";
        let git = Arc::new(crate::find_git().unwrap());
        let plain = GitCommand::new(git.clone()).args(["status"]);
        let own = plain.clone().env(KEY, "'a.b=own'");
        with_env(&[(KEY, "'protocol.version=0'")], || {
            assert_eq!(
                env_value(&plain, KEY).as_deref(),
                Some("'protocol.version=0'")
            );
            assert_eq!(env_value(&own, KEY).as_deref(), Some("'a.b=own'"));
            let other_thread = plain.clone();
            let seen = std::thread::spawn(move || env_value(&other_thread, KEY))
                .join()
                .unwrap();
            assert_eq!(seen, None);
        });
        assert_eq!(env_value(&plain, KEY), None);
    }
}
