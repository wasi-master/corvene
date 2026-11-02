//! Network operations - GHD `lib/git/{fetch,pull,push,remote,lfs}.ts`,
//! `lib/git/for-each-ref.ts` (`getBranchesDifferingFromUpstream`),
//! `lib/git/tag.ts` (`fetchTagsToPush`) and
//! `lib/progress/{git,clone,fetch,pull,push}.ts`. Git LFS transfer progress
//! is `crate::lfs_progress`.
//!
//! Authentication: every remote command gets `GIT_ASKPASS` pointing at the
//! Corvene binary itself (see `crates/corvene/src/askpass.rs`), which answers
//! git's username/password prompts from the macOS Keychain. GHD runs a
//! credential-helper trampoline over a socket instead; the askpass helper is
//! simpler and keeps tokens out of the environment. SSH remotes are left to
//! the user's ssh-agent, as in GHD.
//!
//! Deviations, off by default (GHD's behaviour): [`ProgressParser::with_alias`]
//! lets the clone parser count git's `Updating files` lines as GHD's
//! `Checking out files` step (`lib/progress/clone.ts`,
//! `281-clone-updating-files-step`), and [`fast_forward_branches_with`]
//! leaves out the branches checked out in other worktrees, which make GHD's
//! `fastForwardBranches` (`lib/git/fetch.ts`) fail as a whole
//! (`282-fast-forward-skips-worktree-branches`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use corvene_models::Remote;

use crate::detect::GitBinary;
use crate::error::{GitError, Result};
use crate::lfs_progress::run_with_progress;
use crate::paths::git_dir;
use crate::process::GitCommand;

// ---------------------------------------------------------------------------
// progress
// ---------------------------------------------------------------------------

/// GHD `IGitProgressInfo`: one parsed `--progress` line.
#[derive(Clone, Debug, PartialEq)]
pub struct ProgressLine {
    /// Everything before the last `": "` (`remote: Compressing objects`).
    pub title: String,
    /// Units processed (`159` of `14% (159/1133)`, `123` of `123`).
    pub value: u64,
    /// `1133` of `14% (159/1133)`; `None` for a bare count.
    pub total: Option<u64>,
    /// The integer before `%` (`14` of `14% (159/1133)`); `None` for a bare
    /// count.
    pub percent: Option<u32>,
    /// The line has a trailing `, done.`.
    pub done: bool,
    /// The line as given.
    pub text: String,
}

/// GHD `parse` in `lib/progress/git.ts`:
/// `remote: Compressing objects:  14% (159/1133)` → title/value/total.
/// A line without `": "` is not progress (GHD's `lastIndexOf` of -1 would
/// give an empty title, which no step has).
pub fn parse_progress_line(line: &str) -> Option<ProgressLine> {
    let title_len = line.rfind(": ")?;
    if title_len == 0 {
        return None;
    }
    let title = &line[..title_len];
    let rest = line[title_len + 2..].trim();
    if rest.is_empty() {
        return None;
    }
    let mut parts = rest.split(", ");
    let first = parts.next()?;
    let all_digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let (value, total, percent) = if all_digits(first) {
        (first.parse::<u64>().ok()?, None, None)
    } else {
        // GHD `percentRe`: /^(\d{1,3})% \((\d+)\/(\d+)\)$/
        let (percent, rest) = first.split_once("% (")?;
        let (value, total) = rest.strip_suffix(')')?.split_once('/')?;
        if percent.len() > 3 || !all_digits(percent) || !all_digits(value) || !all_digits(total) {
            return None;
        }
        (
            value.parse::<u64>().ok()?,
            Some(total.parse::<u64>().ok()?),
            Some(percent.parse::<u32>().ok()?),
        )
    };
    let done = parts.any(|p| p == "done.");
    Some(ProgressLine {
        title: title.to_string(),
        value,
        total,
        percent,
        done,
        text: line.to_string(),
    })
}

/// GHD `stripVTControlCharacters` (node's `util`), which
/// `GitProgressParser.parse` applies to every line so hook output reads as
/// plain text: drops CSI (`ESC [ … final`, `0x9B … final`) and OSC (`ESC ]
/// … BEL` / `ESC \`) sequences, charset selections (`ESC ( B`) and other
/// two-character `ESC x` escapes.
pub fn strip_vt_control_characters(line: &str) -> String {
    if !line.contains(['\u{1b}', '\u{9b}']) {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        let introducer = match c {
            '\u{9b}' => '[',
            '\u{1b}' => match chars.next() {
                Some(next) => next,
                None => break,
            },
            _ => {
                out.push(c);
                continue;
            }
        };
        match introducer {
            // CSI: parameter and intermediate bytes, then one final byte
            '[' => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: up to BEL, ST or ESC '\'
            ']' => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' || c == '\u{9c}' {
                        break;
                    }
                    if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            '(' | ')' | '#' => {
                chars.next();
            }
            _ => {}
        }
    }
    out
}

/// GHD `IGitProgress | IGitOutput`: what [`ProgressParser::parse_event`]
/// makes of one line.
#[derive(Clone, Debug, PartialEq)]
pub enum GitProgressEvent {
    /// GHD `IGitProgress`: a line of one of the parser's steps; `percent` is
    /// the overall 0..1 of the operation.
    Progress { percent: f64, details: ProgressLine },
    /// GHD `IGitOutput`: any other line, with the last overall percent.
    Context { percent: f64, text: String },
}

impl GitProgressEvent {
    /// The overall 0..1.
    pub fn percent(&self) -> f64 {
        match self {
            Self::Progress { percent, .. } | Self::Context { percent, .. } => *percent,
        }
    }

    /// GHD `progress.kind === 'progress' ? progress.details.text :
    /// progress.text`: the line to show.
    pub fn text(&self) -> &str {
        match self {
            Self::Progress { details, .. } => &details.text,
            Self::Context { text, .. } => text,
        }
    }
}

/// GHD `GitProgressParser`: weighted steps → overall 0..1. Weights and the
/// overall percent are `f64`, GHD's JavaScript numbers; [`Self::parse`]
/// rounds the result to `f32` for [`ProgressFn`]. One parser per git run.
#[derive(Clone, Debug)]
pub struct ProgressParser {
    steps: Vec<(&'static str, f64)>,
    /// `(title, step)`: lines titled `title` count as the step `step`
    /// ([`Self::with_alias`]).
    aliases: Vec<(&'static str, &'static str)>,
    step_index: usize,
    last_percent: f64,
}

impl ProgressParser {
    /// `steps` are `(title, weight)` in the order git prints them; weights
    /// are relative and scaled to sum to 1.
    ///
    /// # Panics
    ///
    /// When `steps` is empty (GHD's constructor throws "must specify at
    /// least one step"): the steps are constants, so this is a programming
    /// error.
    pub fn new(steps: &[(&'static str, f64)]) -> Self {
        assert!(!steps.is_empty(), "must specify at least one step");
        let total: f64 = steps.iter().map(|(_, w)| w).sum();
        Self {
            steps: steps.iter().map(|(t, w)| (*t, w / total)).collect(),
            aliases: Vec::new(),
            step_index: 0,
            last_percent: 0.,
        }
    }

    /// Lines titled `title` also count as the step `step` (Corvene; GHD
    /// matches step titles exactly). `clone-updating-files-step` uses it
    /// for the `Updating files` lines git prints for the checkout today,
    /// which GHD's clone steps still call `Checking out files`.
    pub fn with_alias(mut self, title: &'static str, step: &'static str) -> Self {
        self.aliases.push((title, step));
        self
    }

    fn is_step(&self, line_title: &str, step: &str) -> bool {
        line_title == step
            || self
                .aliases
                .iter()
                .any(|(title, target)| *target == step && *title == line_title)
    }

    /// GHD `CloneProgressParser` (`lib/progress/clone.ts`).
    pub fn for_clone() -> Self {
        Self::new(&[
            ("remote: Compressing objects", 0.1),
            ("Receiving objects", 0.6),
            ("Resolving deltas", 0.1),
            ("Checking out files", 0.2),
        ])
    }

    /// GHD `FetchProgressParser` (`lib/progress/fetch.ts`).
    pub fn fetch() -> Self {
        Self::new(&[
            ("remote: Compressing objects", 0.1),
            ("Receiving objects", 0.7),
            ("Resolving deltas", 0.2),
        ])
    }

    /// GHD `PullProgressParser` (`lib/progress/pull.ts`).
    pub fn pull() -> Self {
        Self::new(&[
            ("remote: Compressing objects", 0.1),
            ("Receiving objects", 0.7),
            ("Resolving deltas", 0.15),
            ("Checking out files", 0.15),
        ])
    }

    /// GHD `PushProgressParser` (`lib/progress/push.ts`).
    pub fn push() -> Self {
        Self::new(&[
            ("Compressing objects", 0.2),
            ("Writing objects", 0.7),
            ("remote: Resolving deltas", 0.1),
        ])
    }

    /// GHD `GitProgressParser.parse`. Once a step was seen, lines of the
    /// steps before it are context: the steps come in order, and the
    /// earlier ones count as complete.
    pub fn parse_event(&mut self, line: &str) -> GitProgressEvent {
        let text = strip_vt_control_characters(line);
        let Some(progress) = parse_progress_line(&text) else {
            return GitProgressEvent::Context {
                percent: self.last_percent,
                text,
            };
        };
        let mut percent = 0.;
        for (i, (title, weight)) in self.steps.iter().enumerate() {
            if i >= self.step_index && self.is_step(&progress.title, title) {
                if let Some(total) = progress.total.filter(|t| *t > 0) {
                    percent += weight * (progress.value as f64 / total as f64);
                }
                self.step_index = i;
                self.last_percent = percent;
                return GitProgressEvent::Progress {
                    percent,
                    details: progress,
                };
            }
            percent += weight;
        }
        GitProgressEvent::Context {
            percent: self.last_percent,
            text,
        }
    }

    /// [`Self::parse_event`] as `Some((percent, text))` for a step's line
    /// and `None` for context.
    pub fn parse(&mut self, line: &str) -> Option<(f32, String)> {
        match self.parse_event(line) {
            GitProgressEvent::Progress { percent, details } => Some((percent as f32, details.text)),
            GitProgressEvent::Context { .. } => None,
        }
    }

    pub fn last_percent(&self) -> f32 {
        self.last_percent as f32
    }
}

/// `(percent 0..1, description)` reported while a remote operation runs.
pub type ProgressFn<'a> = &'a mut dyn FnMut(f32, String);

// ---------------------------------------------------------------------------
// errors
// ---------------------------------------------------------------------------

/// The remote-operation failures Corvene reacts to (dugite's `GitError`s).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteFailure {
    /// Corvene (`528-proxy-credentials`): the proxy answered 407 Proxy
    /// Authentication Required.
    ProxyAuthenticationRequired,
    /// `! [rejected] … (fetch first)` / `non-fast-forward`.
    PushNotFastForward,
    /// HTTPS authentication failed or a username could not be read.
    AuthenticationFailed,
    /// `remote: Repository not found.`
    RepositoryNotFound,
    /// `remote: Permission to … denied`
    PermissionDenied,
    /// Protected branch / push rejected by a remote hook.
    ProtectedBranch,
    /// `GITHUB PUSH PROTECTION`: secret scanning blocked the push.
    PushWithSecretDetected,
    /// The token lacks the `workflow` scope for a workflow file change.
    MissingWorkflowScope,
    /// An organization enforces SAML SSO; the token must be re-authorized.
    SamlReauthRequired,
    Other,
}

/// Classify a failed remote command from its stderr.
pub fn classify_remote_failure(stderr: &str) -> RemoteFailure {
    let s = stderr;
    // before the authentication failures: git's "could not read Password
    // for 'http://user@proxy:3128'" is about the proxy too
    if crate::proxy::is_proxy_auth_failure(s) {
        return RemoteFailure::ProxyAuthenticationRequired;
    }
    if s.contains("GITHUB PUSH PROTECTION") && s.contains("Push cannot contain secrets") {
        return RemoteFailure::PushWithSecretDetected;
    }
    if s.contains("without `workflow` scope") {
        return RemoteFailure::MissingWorkflowScope;
    }
    if s.contains("organization has enabled or enforced SAML SSO") {
        return RemoteFailure::SamlReauthRequired;
    }
    if s.contains("Authentication failed for")
        || s.contains("could not read Username for")
        || s.contains("could not read Password for")
        || s.contains("Permission denied (publickey")
        || s.contains("Invalid username or password")
    {
        return RemoteFailure::AuthenticationFailed;
    }
    if s.contains("Repository not found") || s.contains("repository not found") {
        return RemoteFailure::RepositoryNotFound;
    }
    if s.contains("Permission to") && s.contains("denied") {
        return RemoteFailure::PermissionDenied;
    }
    if s.contains("protected branch") || s.contains("GH006") {
        return RemoteFailure::ProtectedBranch;
    }
    if s.contains("[rejected]")
        && (s.contains("fetch first")
            || s.contains("non-fast-forward")
            || s.contains("stale info")
            || s.contains("remote contains work"))
    {
        return RemoteFailure::PushNotFastForward;
    }
    RemoteFailure::Other
}

/// The `fatal:` line that explains git's closing "Could not read from remote
/// repository." when it is something else (out of memory, a broken pack, a
/// lost connection): dugite, and so GHD, reads every such failure as an SSH
/// permission problem (desktop#22413).
pub fn remote_read_failure_cause(stderr: &str) -> Option<&str> {
    let lines = stderr.lines().map(str::trim);
    let at = lines
        .clone()
        .position(|l| l.ends_with("Could not read from remote repository."))?;
    lines
        .take(at)
        .filter_map(|l| {
            ["fatal: ", "error: ", "ssh: "]
                .iter()
                .find_map(|prefix| l.strip_prefix(prefix))
        })
        .map(str::trim)
        .filter(|cause| {
            !cause.is_empty()
                && !cause.contains("Could not read from remote repository")
                && !cause.contains("Permission denied")
                && !cause.contains("Host key verification failed")
        })
        .last()
}

/// Corvene (`288-dead-remote-indicator`): the remote command failed because
/// the remote repository is not there: GitHub's and Bitbucket's "Repository
/// not found", GitLab's "could not be found", and the "does not appear to be
/// a git repository" of a path (local or over SSH) without one.
pub fn remote_repository_missing(err: &GitError) -> bool {
    match err {
        GitError::Failed { stderr, .. } => {
            classify_remote_failure(stderr) == RemoteFailure::RepositoryNotFound
                || stderr.contains("does not appear to be a git repository")
                || (stderr.contains("remote:") && stderr.contains("could not be found"))
        }
        _ => false,
    }
}

pub fn remote_failure(err: &GitError) -> RemoteFailure {
    match err {
        GitError::Failed { stderr, .. } => classify_remote_failure(stderr),
        _ => RemoteFailure::Other,
    }
}

// ---------------------------------------------------------------------------
// askpass environment
// ---------------------------------------------------------------------------

/// Environment for commands that may need credentials: `GIT_ASKPASS` is this
/// binary in askpass mode; `logins` maps hosts to the account to use
/// (`github.com=octocat;ghe.corp=me`).
pub struct AskpassEnv {
    pub program: std::path::PathBuf,
    pub logins: String,
    /// Corvene (`529-gh-cli-account`): passwords the keychain lacks may
    /// come from the GitHub CLI (`CORVENE_ASKPASS_GH_CLI`).
    pub gh_cli: bool,
    /// A background task: ssh's questions take stored answers only (GHD
    /// `getIsBackgroundTaskEnvironment`).
    pub no_ui: bool,
}

/// Where this binary in askpass mode reaches the running app.
static SSH_PROMPT_SERVER: std::sync::OnceLock<(u16, String)> = std::sync::OnceLock::new();

/// `corvene_core::ssh_prompts` listens: ssh's questions may go to the app.
pub fn set_ssh_prompt_server(port: u16, token: String) {
    let _ = SSH_PROMPT_SERVER.set((port, token));
}

/// ssh's questions through `program` (`SSH_ASKPASS`), without a terminal or
/// a display (`SSH_ASKPASS_REQUIRE`, from OpenSSH 8.4; before it `DISPLAY`
/// must be set and there must be no terminal).
fn with_ssh_askpass(cmd: GitCommand, program: &std::path::Path) -> GitCommand {
    let Some((port, token)) = SSH_PROMPT_SERVER.get() else {
        // Older gits only consult GIT_ASKPASS when SSH_ASKPASS is also
        // unset; make sure the user's shell setup does not interfere.
        return cmd.env_remove("SSH_ASKPASS");
    };
    let cmd = if std::env::var_os("DISPLAY").is_none() {
        cmd.env("DISPLAY", ".")
    } else {
        cmd
    };
    cmd.env("SSH_ASKPASS", program)
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env("CORVENE_ASKPASS", "1")
        .env("CORVENE_ASKPASS_PORT", port.to_string())
        .env("CORVENE_ASKPASS_TOKEN", token)
}

/// Corvene `1116-ssh-signing-prompt`: a signing `ssh-keygen` started by
/// `git commit` asks through the app. Nothing without a server.
pub fn with_signing_askpass(cmd: GitCommand) -> GitCommand {
    match std::env::current_exe() {
        Ok(program) => with_ssh_askpass(cmd, &program),
        Err(_) => cmd,
    }
}

impl AskpassEnv {
    /// Android: the process is the activity, not a program git could run;
    /// `CORVENE_ASKPASS_PROGRAM` names the packaged helper
    /// (`crates/corvene-askpass`), which asks the running app.
    pub fn current_exe(logins: String) -> Option<Self> {
        #[cfg(target_os = "android")]
        let program = std::env::var_os("CORVENE_ASKPASS_PROGRAM").map(std::path::PathBuf::from);
        #[cfg(not(target_os = "android"))]
        let program = std::env::current_exe().ok();
        program.map(|program| Self {
            program,
            logins,
            gh_cli: false,
            no_ui: false,
        })
    }

    pub(crate) fn apply(&self, cmd: GitCommand) -> GitCommand {
        let cmd = if self.gh_cli {
            cmd.env("CORVENE_ASKPASS_GH_CLI", "1")
        } else {
            cmd
        };
        let cmd = if self.no_ui {
            cmd.env("CORVENE_ASKPASS_NO_UI", "1")
        } else {
            cmd
        };
        let cmd = cmd
            .env("GIT_ASKPASS", &self.program)
            .env("CORVENE_ASKPASS", "1")
            .env("CORVENE_ASKPASS_LOGINS", &self.logins);
        with_ssh_askpass(cmd, &self.program)
    }
}

fn remote_command(git: Arc<GitBinary>, workdir: &Path, askpass: Option<&AskpassEnv>) -> GitCommand {
    let cmd = GitCommand::new(git).current_dir(workdir);
    match askpass {
        Some(a) => a.apply(cmd),
        None => cmd,
    }
}

/// GHD `envForRemoteOperation(remote.url)`: [`remote_command`] (the
/// credentials) plus the system proxy for `remote`'s URL
/// ([`proxy_env_for_remote`]).
pub(crate) fn remote_operation(
    git: Arc<GitBinary>,
    workdir: &Path,
    remote: &str,
    askpass: Option<&AskpassEnv>,
) -> GitCommand {
    let proxy = proxy_env_for_remote(git.clone(), workdir, remote);
    proxy.into_iter().fold(
        remote_command(git, workdir, askpass),
        |cmd, (key, value)| cmd.env(key, value),
    )
}

/// The proxy variables for a remote operation on `remote`, a remote's name
/// or a URL ([`crate::proxy::env_for_remote_operation_with`]: the system
/// proxy and the saved proxy credentials). The URL is only read while
/// either is on.
fn proxy_env_for_remote(
    git: Arc<GitBinary>,
    workdir: &Path,
    remote: &str,
) -> Vec<(String, String)> {
    crate::proxy::env_for_remote_operation_with(git.clone(), Some(workdir), || {
        Some(remote_url_for_proxy(git, workdir, remote))
    })
}

/// `remote`'s configured `remote.<name>.url` (GHD resolves the proxy for
/// `remote.url`), else `remote` itself (a URL or path given directly).
fn remote_url_for_proxy(git: Arc<GitBinary>, workdir: &Path, remote: &str) -> String {
    if remote.contains("://") {
        return remote.to_string();
    }
    config_value(git, workdir, &format!("remote.{remote}.url"))
        .unwrap_or_else(|| remote.to_string())
}

/// Corvene (`submodules-follow-checkout` flag): `git submodule update
/// --init --recursive` for every submodule recorded in the index except
/// `skip` (those the caller saw changed before, whose work must not be moved
/// away). New submodules are cloned, hence `askpass`. GHD 3.6.6 updates
/// every submodule after a checkout (`update_submodules_after_operation`)
/// and none after a merge.
pub fn update_submodules(
    git: Arc<GitBinary>,
    workdir: &Path,
    skip: &[String],
    askpass: Option<&AskpassEnv>,
) -> Result<()> {
    let out = GitCommand::new(git.clone())
        .args(["ls-files", "--stage", "-z"])
        .current_dir(workdir)
        .run()?;
    let paths: Vec<String> = out
        .stdout
        .split(|b| *b == 0)
        .filter_map(|entry| {
            let entry = std::str::from_utf8(entry).ok()?;
            let (meta, path) = entry.split_once('\t')?;
            meta.starts_with("160000 ").then(|| path.to_string())
        })
        .filter(|path| !skip.contains(path))
        .collect();
    if paths.is_empty() {
        return Ok(());
    }
    let proxy_env = crate::proxy::env_for_fallback(git.clone(), workdir, askpass.is_some());
    proxy_env
        .into_iter()
        .fold(
            remote_command(git, workdir, askpass),
            |cmd, (key, value)| cmd.env(key, value),
        )
        .args(["submodule", "update", "--init", "--recursive", "--"])
        .args(&paths)
        .run()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// remotes
// ---------------------------------------------------------------------------

/// GHD `getRemotes` via the CLI (`remote -v`, fetch URLs), sorted by name.
/// A directory that is not a repository has no remotes (GHD expects
/// `NotAGitRepository` and returns `[]`).
pub fn get_remotes(git: Arc<GitBinary>, workdir: &Path) -> Result<Vec<Remote>> {
    let out = GitCommand::new(git)
        .args(["remote", "-v"])
        .current_dir(workdir)
        .expected_errors([crate::KnownGitError::NotAGitRepository])
        .run()?;
    if !out.status.success() {
        // the expected `NotAGitRepository`; any other failure is an `Err`
        return Ok(Vec::new());
    }
    let text = out.stdout_string()?;
    let mut remotes: Vec<Remote> = text.lines().filter_map(parse_remote_line).collect();
    remotes.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(remotes)
}

/// One fetch line of `git remote -v`, GHD's `/^(.+)\t(.+)\s\(fetch\)/`: the
/// URL ends at the last whitespace + `(fetch)`, so a promisor remote's
/// `<url> (fetch) [blob:none]` counts too.
fn parse_remote_line(line: &str) -> Option<Remote> {
    let mut end = line.len();
    let before_marker = loop {
        let at = line[..end].rfind("(fetch)")?;
        let before = &line[..at];
        match before.chars().next_back() {
            Some(c) if c.is_whitespace() => break &before[..before.len() - c.len_utf8()],
            _ => end = at,
        }
    };
    let (name, url) = before_marker.rsplit_once('\t')?;
    (!name.is_empty() && !url.is_empty()).then(|| Remote {
        name: name.to_string(),
        url: url.to_string(),
    })
}

/// GHD `findDefaultRemote`: `origin`, else the first remote.
pub fn find_default_remote(remotes: &[Remote]) -> Option<&Remote> {
    remotes
        .iter()
        .find(|r| r.name == "origin")
        .or_else(|| remotes.first())
}

pub fn add_remote(git: Arc<GitBinary>, workdir: &Path, name: &str, url: &str) -> Result<()> {
    GitCommand::new(git)
        .args(["remote", "add", name, url])
        .current_dir(workdir)
        .run()?;
    Ok(())
}

pub fn set_remote_url(git: Arc<GitBinary>, workdir: &Path, name: &str, url: &str) -> Result<()> {
    GitCommand::new(git)
        .args(["remote", "set-url", name, url])
        .current_dir(workdir)
        .run()?;
    Ok(())
}

pub fn remove_remote(git: Arc<GitBinary>, workdir: &Path, name: &str) -> Result<()> {
    GitCommand::new(git)
        .args(["remote", "remove", name])
        .current_dir(workdir)
        .allow_exit_code(2)
        .allow_exit_code(128)
        .run()?;
    Ok(())
}

/// Corvene (`1109-remote-manager`): `git remote rename <old> <new>`, which
/// also moves the remote-tracking branches and the `branch.*.remote`,
/// `branch.*.pushRemote` and `remote.pushDefault` settings naming it.
pub fn rename_remote(git: Arc<GitBinary>, workdir: &Path, old: &str, new: &str) -> Result<()> {
    GitCommand::new(git)
        .args(["remote", "rename", old, new])
        .current_dir(workdir)
        .run()?;
    Ok(())
}

/// Corvene (`1109-remote-manager`): how many commits of `rev` no
/// remote-tracking branch of `remote` has (`git rev-list --count <rev>
/// --not --remotes=<remote>`), for a branch not on that remote yet.
pub fn commits_not_on_remote(
    git: Arc<GitBinary>,
    workdir: &Path,
    rev: &str,
    remote: &str,
) -> Result<u32> {
    let out = GitCommand::new(git)
        .args([
            "rev-list".to_string(),
            "--count".to_string(),
            rev.to_string(),
            "--not".to_string(),
            format!("--remotes={remote}"),
        ])
        .current_dir(workdir)
        .run()?;
    Ok(out.stdout_string()?.trim().parse().unwrap_or(0))
}

/// Corvene (`1109-remote-manager`): whether `name` can name a remote, as
/// `git remote add` checks it (`refs/remotes/<name>/test` must be a valid
/// ref name). A cheap local check, no git run.
pub fn remote_name_is_valid(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && !name.starts_with('.')
        && !name.ends_with('.')
        && !name.ends_with(".lock")
        && !name.contains("..")
        && !name.contains("@{")
        && name != "@"
        && !name.contains("//")
        && !name.starts_with('/')
        && !name.ends_with('/')
        && !name.split('/').any(|part| part.starts_with('.'))
        && !name.chars().any(|c| {
            c.is_ascii_control() || matches!(c, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\')
        })
}

/// GHD `updateRemoteHEAD`: `remote set-head -a <remote>` (best effort).
pub fn update_remote_head(
    git: Arc<GitBinary>,
    workdir: &Path,
    remote: &str,
    askpass: Option<&AskpassEnv>,
) -> Result<()> {
    remote_operation(git, workdir, remote, askpass)
        .args(["remote", "set-head", "-a", remote])
        .allow_exit_code(1)
        .allow_exit_code(128)
        .run()?;
    Ok(())
}

/// `refs/remotes/<remote>/HEAD` exists and points at a branch that exists,
/// so [`update_remote_head`] (which asks the server for every ref) can be
/// skipped (desktop#22039).
pub fn remote_head_resolves(git: Arc<GitBinary>, workdir: &Path, remote: &str) -> bool {
    GitCommand::new(git)
        .args(["rev-parse", "-q", "--verify"])
        .arg(format!("refs/remotes/{remote}/HEAD"))
        .current_dir(workdir)
        .allow_exit_code(1)
        .allow_exit_code(128)
        .run()
        .is_ok_and(|o| o.status.success())
}

/// git could not update a remote-tracking ref ("cannot lock ref …", "unable
/// to update local ref"), usually because a stale ref such as
/// `origin/feature` blocks a new `origin/feature/x`; `git remote prune`
/// clears it (desktop#11391).
pub fn is_stale_remote_ref_failure(err: &GitError) -> bool {
    matches!(err, GitError::Failed { stderr, .. }
        if stderr.contains("unable to update local ref") || stderr.contains("cannot lock ref"))
}

/// `git remote prune <remote>`: delete remote-tracking refs the remote no
/// longer has.
pub fn prune_remote(
    git: Arc<GitBinary>,
    workdir: &Path,
    remote: &str,
    askpass: Option<&AskpassEnv>,
) -> Result<()> {
    remote_operation(git, workdir, remote, askpass)
        .args(["remote", "prune", remote])
        .run()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// fetch / pull / push
// ---------------------------------------------------------------------------

/// GHD `fetch`: `fetch --progress --prune --recurse-submodules=on-demand <remote>`.
pub fn fetch(
    git: Arc<GitBinary>,
    workdir: &Path,
    remote: &str,
    askpass: Option<&AskpassEnv>,
    on_progress: ProgressFn<'_>,
) -> Result<()> {
    fetch_with_prune_tags(git, workdir, remote, false, askpass, on_progress)
}

/// [`fetch`], plus `--prune-tags` when `prune_tags` is set: local tags the
/// remote no longer has are deleted (Corvene addition; GHD never prunes tags,
/// so a tag deleted on the remote stays forever).
pub fn fetch_with_prune_tags(
    git: Arc<GitBinary>,
    workdir: &Path,
    remote: &str,
    prune_tags: bool,
    askpass: Option<&AskpassEnv>,
    on_progress: ProgressFn<'_>,
) -> Result<()> {
    let options = FetchOptions {
        prune_tags,
        ..FetchOptions::default()
    };
    fetch_with(git, workdir, remote, options, askpass, on_progress)
}

/// Corvene additions to GHD's fixed `fetch` arguments; the default is GHD's
/// command.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FetchOptions {
    /// `--prune-tags` (see [`fetch_with_prune_tags`]).
    pub prune_tags: bool,
    /// `--write-commit-graph`: git extends the commit-graph file, which
    /// speeds up history walks and ahead/behind counts (desktop#22045).
    pub write_commit_graph: bool,
    /// `--no-recurse-submodules` instead of `--recurse-submodules=on-demand`:
    /// submodules are left to the user (desktop#15758).
    pub skip_submodules: bool,
    /// When git cannot read `.gitmodules` ("bad config line N in file
    /// .gitmodules", e.g. a merge conflict in it), fetch again without
    /// submodules instead of failing (flag `875`, desktop#6200).
    pub retry_bad_gitmodules: bool,
}

/// [`fetch`] with [`FetchOptions`].
pub fn fetch_with(
    git: Arc<GitBinary>,
    workdir: &Path,
    remote: &str,
    options: FetchOptions,
    askpass: Option<&AskpassEnv>,
    on_progress: ProgressFn<'_>,
) -> Result<()> {
    let mut parser = ProgressParser::fetch();
    let mut args = vec!["fetch", "--progress", "--prune"];
    if options.prune_tags {
        args.push("--prune-tags");
    }
    if options.write_commit_graph {
        args.push("--write-commit-graph");
    }
    args.push(if options.skip_submodules {
        "--no-recurse-submodules"
    } else {
        "--recurse-submodules=on-demand"
    });
    args.push(remote);
    let result = run_with_progress(
        remote_operation(git.clone(), workdir, remote, askpass).args(&args),
        &mut parser,
        &mut fetch_progress(&mut *on_progress),
    );
    match result {
        // flag `875`: git cannot read `.gitmodules`, so fetch without
        // submodules
        Err(GitError::Failed { stderr, .. })
            if options.retry_bad_gitmodules
                && !options.skip_submodules
                && crate::bad_config_line(&stderr)
                    .is_some_and(|(_, file)| file.ends_with(".gitmodules")) =>
        {
            tracing::warn!("fetching without submodules: {}", stderr.trim());
            if let Some(arg) = args
                .iter_mut()
                .find(|a| a.starts_with("--recurse-submodules"))
            {
                *arg = "--no-recurse-submodules";
            }
            let mut parser = ProgressParser::fetch();
            let cmd = remote_operation(git, workdir, remote, askpass).args(&args);
            run_with_progress(cmd, &mut parser, &mut fetch_progress(on_progress))?;
            Ok(())
        }
        result => result.map(|_| ()),
    }
}

/// GHD `fetch` / `pull`'s progress callback: the ref updates git prints are
/// left out, so of the context lines only `remote: Counting objects` is
/// shown.
fn fetch_progress(on_progress: ProgressFn<'_>) -> impl FnMut(GitProgressEvent) + '_ {
    move |event| {
        if matches!(&event, GitProgressEvent::Context { text, .. }
            if !text.starts_with("remote: Counting objects"))
        {
            return;
        }
        on_progress(event.percent() as f32, event.text().to_string());
    }
}

/// `git fetch --tags <remote>`: every tag of `remote`, not only those
/// pointing into fetched history (Corvene, Repository › Fetch All Tags,
/// `899-tags-in-branch-list`). A separate command: adding `--tags` to the
/// normal fetch makes it fail as a whole when a local tag differs from the
/// remote's ("would clobber existing tag").
pub fn fetch_tags(
    git: Arc<GitBinary>,
    workdir: &Path,
    remote: &str,
    askpass: Option<&AskpassEnv>,
) -> Result<()> {
    remote_operation(git, workdir, remote, askpass)
        .args(["fetch", "--tags", "--no-recurse-submodules", remote])
        .run()?;
    Ok(())
}

/// GHD `fetchRefspec`
pub fn fetch_refspec(
    git: Arc<GitBinary>,
    workdir: &Path,
    remote: &str,
    refspec: &str,
    askpass: Option<&AskpassEnv>,
) -> Result<()> {
    remote_operation(git, workdir, remote, askpass)
        .args(["fetch", remote, refspec])
        .allow_exit_code(128)
        .run()?;
    Ok(())
}

/// Corvene (`1223-checkout-from-fork`): the branches of the repository at
/// `url` (a URL or a remote's name), `git ls-remote --heads <url>`, sorted.
pub fn ls_remote_heads(
    git: Arc<GitBinary>,
    workdir: &Path,
    url: &str,
    askpass: Option<&AskpassEnv>,
) -> Result<Vec<String>> {
    let out = remote_operation(git, workdir, url, askpass)
        .args(["ls-remote", "--heads", url])
        .run()?;
    let mut heads: Vec<String> = out
        .stdout_string()?
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .filter_map(|(_, name)| name.strip_prefix("refs/heads/"))
        .map(str::to_string)
        .collect();
    heads.sort_by_key(|h| h.to_lowercase());
    Ok(heads)
}

/// Corvene (`1223-checkout-from-fork`): fetch only `remote`'s `branch` into
/// its remote-tracking branch (`fetch <remote>
/// +refs/heads/<branch>:refs/remotes/<remote>/<branch>`), failing like a
/// fetch does (unlike [`fetch_refspec`]).
pub fn fetch_remote_branch(
    git: Arc<GitBinary>,
    workdir: &Path,
    remote: &str,
    branch: &str,
    askpass: Option<&AskpassEnv>,
) -> Result<()> {
    remote_operation(git, workdir, remote, askpass)
        .args([
            "fetch".to_string(),
            "--no-tags".to_string(),
            remote.to_string(),
            format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}"),
        ])
        .run()?;
    Ok(())
}

/// Fast-forward the local branch `local`, which must not be checked out, to
/// `remote`'s `remote_branch` without switching to it:
/// `fetch <remote> refs/heads/<remote_branch>:refs/heads/<local>` (git refuses
/// a non-fast-forward and updates the remote-tracking branch too). Corvene
/// addition (desktop#19837).
pub fn fast_forward_branch_from_remote(
    git: Arc<GitBinary>,
    workdir: &Path,
    remote: &str,
    remote_branch: &str,
    local: &str,
    askpass: Option<&AskpassEnv>,
) -> Result<()> {
    remote_operation(git, workdir, remote, askpass)
        .args([
            "fetch".to_string(),
            remote.to_string(),
            format!("refs/heads/{remote_branch}:refs/heads/{local}"),
        ])
        .run()?;
    Ok(())
}

/// Corvene (`1103-implicit-upstream-push-default`): the remote a plain `git
/// push` of `branch`, which has no upstream, goes to, when it goes to the
/// same-named branch there (`push.default=current`): `branch.<name>.pushRemote`,
/// else `remote.pushDefault`, else `default_remote`.
pub fn implicit_push_remote(
    git: Arc<GitBinary>,
    workdir: &Path,
    branch: &str,
    default_remote: &str,
) -> Option<String> {
    let push_default = config_value(git.clone(), workdir, "push.default")?;
    if !push_default.eq_ignore_ascii_case("current") {
        return None;
    }
    Some(
        config_value(git.clone(), workdir, &format!("branch.{branch}.pushRemote"))
            .or_else(|| config_value(git, workdir, "remote.pushDefault"))
            .unwrap_or_else(|| default_remote.to_string()),
    )
}

/// `pull.rebase` is set to anything (GHD `pullWithRebase`).
pub fn pull_with_rebase(git: Arc<GitBinary>, workdir: &Path) -> bool {
    config_value(git, workdir, "pull.rebase").is_some_and(|v| {
        !matches!(
            v.to_ascii_lowercase().as_str(),
            "false" | "no" | "off" | "0"
        )
    })
}

/// `git config --get <key>` inside `workdir`. Read in-process from the
/// cached gitoxide handle (every configuration file, includes resolved, the
/// last value wins as with git); git itself when gitoxide cannot open the
/// repository or does not fully trust it. Status asks on every refresh,
/// before it can start.
pub fn config_value(git: Arc<GitBinary>, workdir: &Path, key: &str) -> Option<String> {
    if let Some(repo) = crate::handle::open_trusted(workdir) {
        return repo
            .config_snapshot()
            .string(key)
            .map(|v| v.to_string().trim().to_string())
            .filter(|s| !s.is_empty());
    }
    GitCommand::new(git)
        .args(["config", "--get", key])
        .current_dir(workdir)
        .allow_exit_code(1)
        .run()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| o.stdout_string().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// GHD `pull`: `pull [--ff] --recurse-submodules --progress <remote>`;
/// `skip_submodules` passes `--no-recurse-submodules` instead (Corvene
/// addition, desktop#15758).
pub fn pull(
    git: Arc<GitBinary>,
    workdir: &Path,
    remote: &str,
    skip_submodules: bool,
    askpass: Option<&AskpassEnv>,
    on_progress: ProgressFn<'_>,
) -> Result<()> {
    let mut parser = ProgressParser::pull();
    let mut args: Vec<&str> = vec!["-c", "rebase.backend=merge", "pull"];
    let pull_ff = config_value(git.clone(), workdir, "pull.ff");
    if pull_ff.is_none() {
        args.push("--ff");
    }
    args.push(if skip_submodules {
        "--no-recurse-submodules"
    } else {
        "--recurse-submodules"
    });
    args.extend(["--progress", remote]);
    let cmd = remote_operation(git, workdir, remote, askpass)
        .args(&args)
        .env("GIT_EDITOR", ":")
        .intercept_hooks(&[
            "pre-merge-commit",
            "prepare-commit-msg",
            "commit-msg",
            "post-merge",
            "pre-rebase",
            "pre-commit",
            "post-rewrite",
        ]);
    run_with_progress(cmd, &mut parser, &mut fetch_progress(on_progress))?;
    Ok(())
}

/// Corvene (`295-cancel-network-operations`): a [`pull`] that started at
/// `since` has gone past its fetch and is merging or rebasing, which must
/// not be interrupted: `ORIG_HEAD` (which `git merge` and `git rebase` write
/// first) changed since, or the index is locked, or a merge or rebase is
/// under way. Errs on the side of "merging".
pub fn pull_merge_started(workdir: &Path, since: SystemTime) -> bool {
    let dir = git_dir(workdir);
    let orig_head_written = std::fs::metadata(dir.join("ORIG_HEAD"))
        .and_then(|m| m.modified())
        .is_ok_and(|modified| modified + std::time::Duration::from_secs(1) >= since);
    orig_head_written
        || ["index.lock", "MERGE_HEAD", "rebase-merge", "rebase-apply"]
            .iter()
            .any(|name| dir.join(name).exists())
}

/// Corvene (`1101-push-size-tooltip`): roughly what a push of HEAD to its
/// upstream sends, from the objects git would consider for the pack.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PushSize {
    /// The objects' uncompressed size; the pack git sends is compressed and
    /// deltified, so usually smaller.
    pub bytes: u64,
    /// What the Git LFS pointers among the new files name (uploaded by
    /// git-lfs unless the server has it already).
    pub lfs_bytes: u64,
    pub lfs_files: usize,
}

/// [`PushSize`]: `rev-list --objects HEAD --not @{upstream}` sized by
/// `cat-file --batch-check`, and the small blobs read with `cat-file
/// --batch` for Git LFS pointers.
pub fn push_size(git: Arc<GitBinary>, workdir: &Path) -> Result<PushSize> {
    let listed = GitCommand::new(git.clone())
        .args([
            "rev-list",
            "--objects",
            "HEAD",
            "--not",
            "@{upstream}",
            "--",
        ])
        .current_dir(workdir)
        .run()?
        .stdout_string()?;
    let ids: Vec<&str> = listed
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    let mut size = PushSize::default();
    if ids.is_empty() {
        return Ok(size);
    }
    let checked = GitCommand::new(git.clone())
        .args([
            "cat-file",
            "--batch-check=%(objectname) %(objecttype) %(objectsize)",
        ])
        .stdin(format!("{}\n", ids.join("\n")))
        .current_dir(workdir)
        .run()?
        .stdout_string()?;
    let mut small_blobs = Vec::new();
    for line in checked.lines() {
        let mut parts = line.split(' ');
        let (Some(id), Some(kind), Some(bytes)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let Ok(bytes) = bytes.parse::<u64>() else {
            continue;
        };
        size.bytes += bytes;
        if kind == "blob" && bytes <= 1024 {
            small_blobs.push(id.to_string());
        }
    }
    if small_blobs.is_empty() {
        return Ok(size);
    }
    let contents = GitCommand::new(git)
        .args(["cat-file", "--batch"])
        .stdin(format!("{}\n", small_blobs.join("\n")))
        .current_dir(workdir)
        .run()?
        .stdout;
    for blob in batch_contents(&contents) {
        if let Some(pointer) = crate::lfs::lfs_pointer(blob) {
            size.lfs_bytes += pointer.size;
            size.lfs_files += 1;
        }
    }
    Ok(size)
}

/// The contents of `git cat-file --batch` output (`<id> <type> <size>\n`,
/// the bytes, `\n` per object; `<id> missing\n` for none).
fn batch_contents(mut out: &[u8]) -> Vec<&[u8]> {
    let mut contents = Vec::new();
    while let Some(end) = out.iter().position(|b| *b == b'\n') {
        let header = String::from_utf8_lossy(&out[..end]);
        out = &out[end + 1..];
        let Some(len) = header
            .rsplit(' ')
            .next()
            .and_then(|n| n.parse::<usize>().ok())
            .filter(|_| !header.ends_with(" missing"))
        else {
            continue;
        };
        let Some(body) = out.get(..len) else {
            break;
        };
        contents.push(body);
        out = out.get(len + 1..).unwrap_or_default();
    }
    contents
}

/// GHD `IPushProgress` (`models/progress.ts`): what [`push_with_progress`]
/// reports.
#[derive(Clone, Debug, PartialEq)]
pub struct PushProgress {
    /// GHD's discriminant, always `"push"`.
    pub kind: &'static str,
    /// `Pushing to <remote>`.
    pub title: String,
    /// The line git (or Git LFS) printed last; `None` for the first event,
    /// sent before git runs.
    pub description: Option<String>,
    /// Overall 0..1.
    pub value: f32,
    pub remote: String,
    /// The local branch being pushed.
    pub branch: String,
}

/// GHD `push`: `push <remote> <local>[:<remote branch>] [tags…]
/// [--set-upstream | --force-with-lease] --progress`, reporting
/// `(percent, description)` for every line git prints (GHD shows each one,
/// at the last percent for a line that is not a step) and Git LFS's
/// uploads. [`push_with_progress`] with GHD's `IPushProgress` callback
/// minus its first, line-less event.
#[allow(clippy::too_many_arguments)]
pub fn push(
    git: Arc<GitBinary>,
    workdir: &Path,
    remote: &str,
    local_branch: &str,
    remote_branch: Option<&str>,
    tags: &[String],
    force_with_lease: bool,
    askpass: Option<&AskpassEnv>,
    on_progress: ProgressFn<'_>,
) -> Result<()> {
    push_with_progress(
        git,
        workdir,
        remote,
        local_branch,
        remote_branch,
        tags,
        force_with_lease,
        askpass,
        &mut |progress| {
            if let Some(description) = progress.description {
                on_progress(progress.value, description);
            }
        },
    )
}

/// GHD `push` with a progress callback: a first [`PushProgress`] at 0
/// before git runs, then one per line git or Git LFS prints.
#[allow(clippy::too_many_arguments)]
pub fn push_with_progress(
    git: Arc<GitBinary>,
    workdir: &Path,
    remote: &str,
    local_branch: &str,
    remote_branch: Option<&str>,
    tags: &[String],
    force_with_lease: bool,
    askpass: Option<&AskpassEnv>,
    on_progress: &mut dyn FnMut(PushProgress),
) -> Result<()> {
    let mut parser = ProgressParser::push();
    let refspec = match remote_branch {
        Some(rb) => format!("{local_branch}:{rb}"),
        None => local_branch.to_string(),
    };
    let mut args: Vec<String> = vec!["push".into(), remote.into(), refspec];
    args.extend(tags.iter().cloned());
    if remote_branch.is_none() {
        args.push("--set-upstream".into());
    } else if force_with_lease {
        args.push("--force-with-lease".into());
    }
    args.push("--progress".into());
    let title = format!("Pushing to {remote}");
    let mut report = |description: Option<String>, value: f32| {
        on_progress(PushProgress {
            kind: "push",
            title: title.clone(),
            description,
            value,
            remote: remote.to_string(),
            branch: local_branch.to_string(),
        })
    };
    report(None, 0.);
    let cmd = remote_operation(git, workdir, remote, askpass)
        .args(&args)
        .intercept_hooks(&["pre-push"]);
    run_with_progress(cmd, &mut parser, &mut |event| {
        report(Some(event.text().to_string()), event.percent() as f32)
    })?;
    Ok(())
}

/// GHD `fetchTagsToPush` (`lib/git/tag.ts`): the local tags a push of
/// `branch` to `remote` would send, from the `[new tag]` lines of `git push
/// <remote> <branch> --follow-tags --dry-run --no-verify --porcelain`. Exit
/// code 1 (the branch itself would be rejected) still lists the tags; any
/// other failure is an error. GHD 3.6.6 calls it only from its tests (the
/// app keeps its own list of tags to push, as Corvene does).
pub fn fetch_tags_to_push(
    git: Arc<GitBinary>,
    workdir: &Path,
    remote: &str,
    branch: &str,
    askpass: Option<&AskpassEnv>,
) -> Result<Vec<String>> {
    let out = remote_operation(git, workdir, remote, askpass)
        .args([
            "push",
            remote,
            branch,
            "--follow-tags",
            "--dry-run",
            "--no-verify",
            "--porcelain",
        ])
        .allow_exit_code(1)
        .run()?;
    let text = out.stdout_string()?;
    // the first line is `To <url>`, the last `Done`
    Ok(text
        .split('\n')
        .skip(1)
        .take_while(|line| *line != "Done")
        .filter_map(|line| {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.first() != Some(&"*") || parts.get(2) != Some(&"[new tag]") {
                return None;
            }
            let tag = parts.get(1)?.split(':').next()?;
            Some(tag.strip_prefix("refs/tags/").unwrap_or(tag).to_string())
        })
        .collect())
}

/// Corvene addition (flag `826`): `git push <remote> --delete
/// refs/tags/<tag>`.
pub fn delete_remote_tag(
    git: Arc<GitBinary>,
    workdir: &Path,
    remote: &str,
    tag: &str,
    askpass: Option<&AskpassEnv>,
) -> Result<()> {
    remote_operation(git, workdir, remote, askpass)
        .args(["push", remote, "--delete", &format!("refs/tags/{tag}")])
        .run()?;
    Ok(())
}

/// Corvene (`346-releases`): `git push <remote> refs/tags/<tag>`, one
/// tag on its own (a release needs its tag on GitHub first).
pub fn push_tag(
    git: Arc<GitBinary>,
    workdir: &Path,
    remote: &str,
    tag: &str,
    askpass: Option<&AskpassEnv>,
) -> Result<()> {
    remote_operation(git, workdir, remote, askpass)
        .args(["push", remote, &format!("refs/tags/{tag}")])
        .run()?;
    Ok(())
}

/// Corvene (`346-releases`): the newest tag reachable from `target`'s
/// first parent (`git describe --tags --abbrev=0 <target>^`), the one a
/// release at `target` follows; `None` when no tag comes before it or
/// `target` has no parent.
pub fn previous_tag(git: Arc<GitBinary>, workdir: &Path, target: &str) -> Result<Option<String>> {
    let out = GitCommand::new(git)
        .args(["describe", "--tags", "--abbrev=0", &format!("{target}^")])
        .current_dir(workdir)
        .allow_exit_code(128)
        .run()?;
    if !out.status.success() {
        return Ok(None);
    }
    let name = out.stdout_string()?.trim().to_string();
    Ok((!name.is_empty()).then_some(name))
}

/// Corvene (`346-releases`): the remote-tracking branches that contain
/// `sha` (`git branch -r --contains <sha>`), as `<remote>/<branch>`; empty
/// when no remote has the commit yet.
pub fn remote_branches_containing(
    git: Arc<GitBinary>,
    workdir: &Path,
    sha: &str,
) -> Result<Vec<String>> {
    let out = GitCommand::new(git)
        .args([
            "branch",
            "-r",
            "--contains",
            sha,
            "--format=%(refname:short)",
        ])
        .current_dir(workdir)
        .allow_exit_code(129)
        .run()?;
    if !out.status.success() {
        return Ok(Vec::new());
    }
    Ok(out
        .stdout_string()?
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.contains("HEAD"))
        .map(str::to_string)
        .collect())
}

/// GHD `ITrackingBranch` (`models/branch.ts`): a local branch and its
/// upstream, by full ref name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackingBranch {
    /// `refs/heads/<name>` (GHD `ref`).
    pub reference: String,
    pub sha: String,
    /// `refs/remotes/<remote>/<name>`.
    pub upstream_ref: String,
    pub upstream_sha: String,
}

/// GHD `getBranchesDifferingFromUpstream` (`lib/git/for-each-ref.ts`): the
/// local branches with an upstream whose tip differs from the upstream's
/// (ahead, behind or both), except the checked-out branch and symbolic refs.
/// A directory that is not a repository has none.
pub fn get_branches_differing_from_upstream(
    git: Arc<GitBinary>,
    workdir: &Path,
) -> Result<Vec<TrackingBranch>> {
    let out = GitCommand::new(git)
        .args([
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(upstream)%00%(symref)%00%(HEAD)",
            "refs/heads",
            "refs/remotes",
        ])
        .current_dir(workdir)
        .expected_errors([crate::KnownGitError::NotAGitRepository])
        .run()?;
    if !out.status.success() {
        // the expected `NotAGitRepository`; any other failure is an `Err`
        return Ok(Vec::new());
    }
    let text = out.stdout_string()?;
    let mut local = Vec::new();
    let mut remote_shas = std::collections::HashMap::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split('\0').collect();
        let [full_name, sha, upstream, symref, head] = cols[..] else {
            continue;
        };
        // symbolic refs (`origin/HEAD`) and the current branch are skipped
        if !symref.is_empty() || head == "*" {
            continue;
        }
        if full_name.starts_with("refs/heads") {
            if !upstream.is_empty() {
                local.push((full_name, sha, upstream));
            }
        } else {
            remote_shas.insert(full_name, sha);
        }
    }
    Ok(local
        .into_iter()
        .filter_map(|(reference, sha, upstream)| {
            let upstream_sha = *remote_shas.get(upstream)?;
            (upstream_sha != sha).then(|| TrackingBranch {
                reference: reference.to_string(),
                sha: sha.to_string(),
                upstream_ref: upstream.to_string(),
                upstream_sha: upstream_sha.to_string(),
            })
        })
        .collect())
}

/// GHD `AppStore.fastForwardBranches`: [`get_branches_differing_from_upstream`]
/// fed to [`fast_forward_tracking_branches`]. Returns how many branches
/// were offered to git (the ones it could not fast-forward stay as they
/// are).
pub fn fast_forward_branches(git: Arc<GitBinary>, workdir: &Path) -> Result<usize> {
    fast_forward_branches_with(git, workdir, false)
}

/// [`fast_forward_branches`]; with `skip_worktree_branches`
/// (`282-fast-forward-skips-worktree-branches`) the branches checked out in
/// another worktree are left out, and so is any branch `git fetch` still
/// refuses as checked out (being rebased or bisected) elsewhere. GHD offers
/// them, and git then refuses the whole update with exit code 128, so no
/// branch moves.
pub fn fast_forward_branches_with(
    git: Arc<GitBinary>,
    workdir: &Path,
    skip_worktree_branches: bool,
) -> Result<usize> {
    let mut branches = get_branches_differing_from_upstream(git.clone(), workdir)?;
    if !skip_worktree_branches {
        fast_forward_tracking_branches(git, workdir, &branches)?;
        return Ok(branches.len());
    }
    if !branches.is_empty() {
        let checked_out: std::collections::HashSet<String> =
            crate::list_worktrees(git.clone(), workdir)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|worktree| worktree.branch)
                .collect();
        branches.retain(|branch| !checked_out.contains(&branch.reference));
    }
    loop {
        match fast_forward_tracking_branches(git.clone(), workdir, &branches) {
            Ok(()) => return Ok(branches.len()),
            Err(err) => {
                // `refusing to fetch into branch 'refs/heads/x' checked out at '<path>'`
                let refused = err.branch_in_other_worktree().and_then(|(reference, _)| {
                    branches.iter().position(|b| b.reference == reference)
                });
                match refused {
                    Some(index) => {
                        branches.remove(index);
                    }
                    None => return Err(err),
                }
            }
        }
    }
}

/// GHD `fastForwardBranches` (`lib/git/fetch.ts`): `fetch .
/// --show-forced-updates --no-write-fetch-head --stdin` with
/// `<upstream>:<ref>` pairs. git refuses the pairs that are not a
/// fast-forward (a branch ahead of its upstream) and exits 1, which is
/// expected.
pub fn fast_forward_tracking_branches(
    git: Arc<GitBinary>,
    workdir: &Path,
    branches: &[TrackingBranch],
) -> Result<()> {
    if branches.is_empty() {
        return Ok(());
    }
    let pairs: Vec<String> = branches
        .iter()
        .map(|b| format!("{}:{}", b.upstream_ref, b.reference))
        .collect();
    GitCommand::new(git)
        .args([
            "fetch",
            ".",
            "--show-forced-updates",
            "--no-write-fetch-head",
            "--stdin",
        ])
        .env("GIT_REFLOG_ACTION", "pull")
        .current_dir(workdir)
        .stdin(pairs.join("\n").into_bytes())
        .allow_exit_code(1)
        .run()?;
    Ok(())
}

/// Fast-forward the checked-out branch to its upstream when the working
/// directory is clean, no merge, rebase or cherry-pick is in progress and
/// the branch is behind but not ahead: `merge --ff-only @{upstream}`.
/// `Ok(false)` when any of that does not hold. Corvene addition
/// (desktop#16586: pull after a background fetch).
pub fn fast_forward_if_only_behind(git: Arc<GitBinary>, workdir: &Path) -> Result<bool> {
    Ok(fast_forward_outcome(git, workdir)? == FastForward::Done)
}

/// What [`fast_forward_outcome`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FastForward {
    /// The branch caught up with its upstream.
    Done,
    /// Nothing to pull: the upstream has no commits the branch lacks.
    UpToDate,
    /// No upstream (or a detached HEAD).
    NoUpstream,
    /// A merge, rebase or cherry-pick is in progress.
    OperationInProgress,
    /// The branch and its upstream both have commits the other lacks.
    Diverged,
    /// Behind only, but the working directory has changes.
    LocalChanges,
}

/// [`fast_forward_if_only_behind`], saying why nothing happened (Corvene,
/// `299-pull-all-repositories`).
pub fn fast_forward_outcome(git: Arc<GitBinary>, workdir: &Path) -> Result<FastForward> {
    let status = crate::get_status(git.clone(), workdir)?;
    if status.merge_head_found || status.rebase_in_progress || status.cherry_pick_head_found {
        return Ok(FastForward::OperationInProgress);
    }
    let Some(ab) = status.ahead_behind.filter(|_| status.upstream.is_some()) else {
        return Ok(FastForward::NoUpstream);
    };
    if ab.behind == 0 {
        return Ok(FastForward::UpToDate);
    }
    if ab.ahead > 0 {
        return Ok(FastForward::Diverged);
    }
    if !status.files.is_empty() {
        return Ok(FastForward::LocalChanges);
    }
    GitCommand::new(git)
        .args(["merge", "--ff-only", "@{upstream}"])
        .env("GIT_REFLOG_ACTION", "pull")
        .current_dir(workdir)
        .run()?;
    Ok(FastForward::Done)
}

/// The local branch `name` once pointed at `upstream`'s current tip (it is
/// in the branch's reflog): a branch that is ahead of and behind its
/// upstream then had the pushed commits rewritten away (amend, rebase or
/// reset outside Corvene) rather than someone else pushing new work.
/// Corvene addition (desktop#9739).
pub fn upstream_tip_in_reflog(
    git: Arc<GitBinary>,
    workdir: &Path,
    name: &str,
    upstream: &str,
) -> bool {
    let Ok(out) = GitCommand::new(git.clone())
        .args(["rev-parse", "-q", "--verify"])
        .arg(format!("{upstream}^{{commit}}"))
        .current_dir(workdir)
        .run()
    else {
        return false;
    };
    let Ok(tip) = out.stdout_string() else {
        return false;
    };
    let tip = tip.trim();
    GitCommand::new(git)
        .args(["reflog", "show", "--format=%H"])
        .arg(format!("refs/heads/{name}"))
        .arg("--")
        .current_dir(workdir)
        .allow_exit_code(128)
        .run()
        .ok()
        .and_then(|o| o.stdout_string().ok())
        .is_some_and(|log| !tip.is_empty() && log.lines().any(|l| l.trim() == tip))
}

/// GHD `updateLastFetched`: mtime of a non-empty `FETCH_HEAD`. A linked
/// worktree has its own `FETCH_HEAD`, so a fetch made from another worktree
/// leaves it "never fetched"; with `shared` (flag `874`) the main
/// repository's `FETCH_HEAD` counts too and the newer one wins.
pub fn last_fetched(workdir: &Path, shared: bool) -> Option<SystemTime> {
    let mtime = |dir: PathBuf| {
        let meta = std::fs::metadata(dir.join("FETCH_HEAD")).ok()?;
        (meta.len() > 0).then(|| meta.modified().ok()).flatten()
    };
    let own = mtime(git_dir(workdir));
    if !shared {
        return own;
    }
    let common = crate::paths::common_dir(workdir);
    if common == git_dir(workdir) {
        return own;
    }
    own.max(mtime(common))
}

/// When the repository was cloned: the time of `HEAD`'s first reflog entry
/// if it is git's "clone: from …". A clone writes no `FETCH_HEAD`, so
/// [`last_fetched`] says "never" until the first fetch (desktop#13401);
/// Corvene falls back to this.
pub fn cloned_at(workdir: &Path) -> Option<SystemTime> {
    let log = std::fs::read_to_string(git_dir(workdir).join("logs").join("HEAD")).ok()?;
    let (head, message) = log.lines().next()?.split_once('\t')?;
    if !message.starts_with("clone: from ") {
        return None;
    }
    let mut fields = head.rsplit(' ');
    let _tz = fields.next()?;
    let secs: u64 = fields.next()?.parse().ok()?;
    Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs))
}

// ---------------------------------------------------------------------------
// LFS
// ---------------------------------------------------------------------------

/// `git lfs` is installed.
pub fn lfs_available(git: Arc<GitBinary>) -> bool {
    GitCommand::new(git).args(["lfs", "version"]).run().is_ok()
}

/// GHD `isUsingLFS`: `lfs track --json` reports tracked patterns.
pub fn is_using_lfs(git: Arc<GitBinary>, workdir: &Path) -> bool {
    let out = GitCommand::new(git)
        .args(["lfs", "track", "--json"])
        .env("GIT_LFS_TRACK_NO_INSTALL_HOOKS", "1")
        .current_dir(workdir)
        .run();
    match out.and_then(|o| o.stdout_string()) {
        Ok(text) => text.contains("\"tracked\": true") || text.contains("\"tracked\":true"),
        Err(_) => false,
    }
}

/// [`is_using_lfs`] without `git lfs track`, which walks every directory
/// (untracked ones included) looking for `.gitattributes` and takes minutes in
/// large worktrees: reads the `.gitattributes` files in the index, the root
/// one on disk and `info/attributes`, and looks for a `filter=lfs` attribute.
/// Corvene addition (desktop#5198).
pub fn is_using_lfs_by_attributes(git: Arc<GitBinary>, workdir: &Path) -> bool {
    let mut files = vec![
        workdir.join(".gitattributes"),
        git_dir(workdir).join("info/attributes"),
    ];
    if let Ok(text) = GitCommand::new(git)
        .args(["ls-files", "-z", "--", ":(glob)**/.gitattributes"])
        .current_dir(workdir)
        .run()
        .and_then(|o| o.stdout_string())
    {
        files.extend(
            text.split('\0')
                .filter(|p| !p.is_empty() && *p != ".gitattributes")
                .map(|p| workdir.join(p)),
        );
    }
    files
        .iter()
        .any(|path| std::fs::read_to_string(path).is_ok_and(|text| attributes_use_lfs(&text)))
}

/// A `.gitattributes` text sets `filter=lfs` on some pattern.
fn attributes_use_lfs(text: &str) -> bool {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .any(|line| line.split_whitespace().skip(1).any(|a| a == "filter=lfs"))
}

/// GHD `isTrackedByLFS`: `git check-attr filter <path>` says `filter: lfs`
/// for the repository-relative `path` (`README.md: filter: unspecified`
/// when no `.gitattributes` rule covers it). Corvene puts `--` before the
/// path, so one starting with `-` is not read as an option (GHD's command
/// fails on it).
pub fn is_tracked_by_lfs(git: Arc<GitBinary>, workdir: &Path, path: &str) -> Result<bool> {
    let out = GitCommand::new(git)
        .args(["check-attr", "filter", "--"])
        .arg(path)
        .current_dir(workdir)
        .run()?;
    Ok(out.stdout_string()?.contains(": filter: lfs"))
}

/// GHD `filesNotTrackedByLFS`: the repository-relative `paths` that
/// [`is_tracked_by_lfs`] rejects, in order, for the commit flow's check for
/// files over 100 MB (`OversizedFiles`, `corvene_core::commit_checks`).
pub fn files_not_tracked_by_lfs<S: AsRef<str>>(
    git: Arc<GitBinary>,
    workdir: &Path,
    paths: &[S],
) -> Result<Vec<String>> {
    let mut untracked = Vec::new();
    for path in paths {
        let path = path.as_ref();
        if !is_tracked_by_lfs(git.clone(), workdir, path)? {
            untracked.push(path.to_string());
        }
    }
    Ok(untracked)
}

/// Corvene `784-suggest-lfs-tracking`: `git lfs track` the `patterns` (Git
/// LFS set up for the repository first when git has no LFS filter, `git
/// lfs install --local`), then `git add --renormalize` the tracked ones of
/// `files`, so an index entry made before the patterns existed becomes a
/// pointer too (untracked files become pointers when the commit stages
/// them). `.gitattributes` is left for the commit.
pub fn track_in_lfs(
    git: Arc<GitBinary>,
    workdir: &Path,
    patterns: &[String],
    files: &[String],
) -> Result<()> {
    if patterns.is_empty() {
        return Ok(());
    }
    if config_value(git.clone(), workdir, "filter.lfs.clean").is_none() {
        GitCommand::new(git.clone())
            .args(["lfs", "install", "--local"])
            .current_dir(workdir)
            .run()?;
    }
    GitCommand::new(git.clone())
        .args(["lfs", "track"])
        .args(patterns)
        .current_dir(workdir)
        .run()?;
    if !files.is_empty() {
        GitCommand::new(git)
            .args(["add", "--renormalize", "--"])
            .args(files)
            .env("GIT_LITERAL_PATHSPECS", "1")
            .current_dir(workdir)
            .run()?;
    }
    Ok(())
}

/// The repository's `pre-push` hook was written by Git LFS.
pub fn lfs_hooks_installed(workdir: &Path) -> bool {
    std::fs::read_to_string(git_dir(workdir).join("hooks/pre-push"))
        .map(|s| s.contains("git lfs") || s.contains("git-lfs"))
        .unwrap_or(false)
}

/// GHD `installLFSHooks(repository, force = true)`
pub fn install_lfs_hooks(git: Arc<GitBinary>, workdir: &Path) -> Result<()> {
    GitCommand::new(git)
        .args(["lfs", "install", "--force"])
        .current_dir(workdir)
        .run()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_remote_names() {
        for good in ["origin", "fork", "my-fork", "team/upstream", "a.b"] {
            assert!(remote_name_is_valid(good), "{good}");
        }
        for bad in [
            "", "-x", ".hidden", "a..b", "a b", "a:b", "a~1", "x.lock", "a/", "a//b", "@", "a/.b",
        ] {
            assert!(!remote_name_is_valid(bad), "{bad}");
        }
    }

    #[test]
    fn push_size_counts_new_objects_and_lfs_pointers() {
        use std::process::Command;
        let dir = tempfile::tempdir().unwrap();
        let run = |cwd: &Path, args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(cwd)
                    .status()
                    .unwrap()
                    .success()
            )
        };
        let remote = dir.path().join("remote.git");
        let work = dir.path().join("work");
        run(
            dir.path(),
            &["init", "-q", "--bare", "-b", "main", "remote.git"],
        );
        run(dir.path(), &["clone", "-q", "remote.git", "work"]);
        for args in [
            ["config", "commit.gpgsign", "false"],
            ["config", "user.name", "T"],
            ["config", "user.email", "t@example.com"],
        ] {
            run(&work, &args);
        }
        std::fs::write(work.join("a.txt"), "a\n").unwrap();
        run(&work, &["add", "."]);
        run(&work, &["commit", "-q", "-m", "a"]);
        run(&work, &["push", "-q", "-u", "origin", "main"]);
        let git = Arc::new(crate::find_git().unwrap());
        assert_eq!(push_size(git.clone(), &work).unwrap(), PushSize::default());
        std::fs::write(work.join("big.bin"), vec![b'x'; 5000]).unwrap();
        std::fs::write(
            work.join("lfs.bin"),
            format!(
                "version https://git-lfs.github.com/spec/v1\noid sha256:{}\nsize 123456\n",
                "a".repeat(64)
            ),
        )
        .unwrap();
        run(&work, &["add", "."]);
        run(&work, &["commit", "-q", "-m", "b"]);
        let size = push_size(git, &work).unwrap();
        assert!(size.bytes > 5000, "{size:?}");
        assert_eq!((size.lfs_bytes, size.lfs_files), (123_456, 1));
        let _ = remote;
    }

    #[test]
    fn batch_contents_splits_objects() {
        let out = b"aaa blob 3\nabc\nbbb missing\nccc blob 0\n\nddd blob 2\nxy\n";
        assert_eq!(batch_contents(out), vec![&b"abc"[..], &b""[..], &b"xy"[..]]);
    }

    #[test]
    fn pull_merge_started_reads_the_git_dir() {
        let dir = tempfile::tempdir().unwrap();
        let git = dir.path().join(".git");
        std::fs::create_dir(&git).unwrap();
        let started = SystemTime::now();
        assert!(!pull_merge_started(dir.path(), started));
        // an ORIG_HEAD from long before the pull does not count
        std::fs::write(git.join("ORIG_HEAD"), "x\n").unwrap();
        assert!(!pull_merge_started(
            dir.path(),
            started + std::time::Duration::from_secs(60)
        ));
        assert!(pull_merge_started(dir.path(), started));
        std::fs::remove_file(git.join("ORIG_HEAD")).unwrap();
        std::fs::create_dir(git.join("rebase-merge")).unwrap();
        assert!(pull_merge_started(dir.path(), started));
    }

    #[test]
    fn missing_remote_repositories() {
        let failed = |stderr: &str| GitError::Failed {
            args: "fetch origin".into(),
            code: Some(128),
            stderr: stderr.into(),
        };
        for stderr in [
            "remote: Repository not found.\nfatal: repository 'https://github.com/o/r.git/' not found",
            "fatal: '/srv/gone.git' does not appear to be a git repository\nfatal: Could not read from remote repository.",
            "remote: The project you were looking for could not be found or you don't have permission to view it.",
        ] {
            assert!(remote_repository_missing(&failed(stderr)), "{stderr}");
        }
        assert!(!remote_repository_missing(&failed(
            "fatal: unable to access 'https://github.com/o/r.git/': Could not resolve host: github.com"
        )));
    }

    #[test]
    fn update_submodules_initialises_all_but_skipped() {
        use std::process::Command;
        let dir = tempfile::tempdir().unwrap();
        let run = |cwd: &Path, args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(cwd)
                    .env("GIT_AUTHOR_NAME", "T")
                    .env("GIT_AUTHOR_EMAIL", "t@example.com")
                    .env("GIT_COMMITTER_NAME", "T")
                    .env("GIT_COMMITTER_EMAIL", "t@example.com")
                    .status()
                    .unwrap()
                    .success()
            )
        };
        let sub = dir.path().join("sub-origin");
        let main = dir.path().join("main");
        for repo in [&sub, &main] {
            std::fs::create_dir(repo).unwrap();
            run(repo, &["init", "-q", "-b", "main"]);
            run(repo, &["config", "commit.gpgsign", "false"]);
        }
        std::fs::write(sub.join("file.txt"), "x\n").unwrap();
        run(&sub, &["add", "."]);
        run(&sub, &["commit", "-q", "-m", "sub"]);
        run(
            &main,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                sub.to_str().unwrap(),
                "sub",
            ],
        );
        run(&main, &["commit", "-q", "-m", "add sub"]);
        run(&main, &["submodule", "deinit", "-q", "-f", "sub"]);
        assert!(!main.join("sub/file.txt").exists());

        let git = Arc::new(crate::find_git().unwrap());
        update_submodules(git.clone(), &main, &["sub".to_string()], None).unwrap();
        assert!(!main.join("sub/file.txt").exists());
        update_submodules(git, &main, &[], None).unwrap();
        assert!(main.join("sub/file.txt").exists());
    }

    #[test]
    fn parses_progress_lines() {
        let p = parse_progress_line("remote: Compressing objects:  14% (159/1133)").unwrap();
        assert_eq!(
            (p.title.as_str(), p.value, p.total),
            ("remote: Compressing objects", 159, Some(1133))
        );
        assert!(!p.done);
        let p = parse_progress_line("Checking out files: 100% (728/728), done.").unwrap();
        assert!(p.done);
        let p = parse_progress_line("remote: Counting objects: 123").unwrap();
        assert_eq!((p.value, p.total), (123, None));
        assert!(parse_progress_line("Everything up-to-date").is_none());
    }

    #[test]
    fn progress_parser_weights_steps() {
        let mut parser = ProgressParser::fetch();
        let (percent, _) = parser.parse("Receiving objects:  50% (5/10)").unwrap();
        assert!((percent - 0.45).abs() < 0.001, "{percent}");
        let (percent, _) = parser.parse("Resolving deltas: 100% (3/3), done.").unwrap();
        assert!((percent - 1.0).abs() < 0.001, "{percent}");
        // earlier steps are ignored once a later one was seen
        assert!(
            parser
                .parse("remote: Compressing objects: 10% (1/10)")
                .is_none()
        );
    }

    #[test]
    fn clone_progress_counts_updating_files_with_the_alias() {
        // GHD's steps: `Updating files` is context at the 80 % reached
        let mut ghd = ProgressParser::for_clone();
        ghd.parse("Resolving deltas: 100% (3/3), done.").unwrap();
        assert!(ghd.parse("Updating files:  50% (1/2)").is_none());
        assert!((ghd.last_percent() - 0.8).abs() < 0.001);
        // `clone-updating-files-step`
        let mut parser =
            ProgressParser::for_clone().with_alias("Updating files", "Checking out files");
        parser.parse("Resolving deltas: 100% (3/3), done.").unwrap();
        let (percent, text) = parser.parse("Updating files:  50% (1/2)").unwrap();
        assert!((percent - 0.9).abs() < 0.001, "{percent}");
        assert_eq!(text, "Updating files:  50% (1/2)");
        let (percent, _) = parser.parse("Checking out files: 100% (2/2)").unwrap();
        assert!((percent - 1.0).abs() < 0.001, "{percent}");
    }

    #[test]
    fn progress_lines_lose_terminal_escapes() {
        assert_eq!(strip_vt_control_characters("\x1b[31mred\x1b[0m"), "red");
        assert_eq!(
            strip_vt_control_characters("a\x1b]8;;https://x\x07link\x1b]8;;\x1b\\b"),
            "alinkb"
        );
        assert_eq!(strip_vt_control_characters("\x1b(Bplain\x1b[K"), "plain");
        let mut parser = ProgressParser::push();
        let (percent, text) = parser
            .parse("\x1b[1mWriting objects:  50% (1/2)\x1b[0m")
            .unwrap();
        assert_eq!(text, "Writing objects:  50% (1/2)");
        assert!((percent - 0.55).abs() < 1e-6, "{percent}");
        // a line that is not a step keeps the last percent
        let last = parser.parse_event("To /tmp/remote.git").percent();
        assert!((last - 0.55).abs() < 1e-9, "{last}");
    }

    #[test]
    fn classifies_failures() {
        assert_eq!(
            classify_remote_failure(
                "fatal: Authentication failed for 'https://github.com/a/b.git/'"
            ),
            RemoteFailure::AuthenticationFailed
        );
        assert_eq!(
            classify_remote_failure(
                " ! [rejected]        main -> main (fetch first)\nerror: failed to push some refs\nhint: Updates were rejected because the remote contains work that you do not have locally."
            ),
            RemoteFailure::PushNotFastForward
        );
        assert_eq!(
            classify_remote_failure(
                "remote: Repository not found.\nfatal: repository 'https://github.com/a/b.git/' not found"
            ),
            RemoteFailure::RepositoryNotFound
        );
        assert_eq!(
            classify_remote_failure("fatal: unable to access"),
            RemoteFailure::Other
        );
    }

    fn run(path: &Path, args: &[&str]) {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(path)
                .env("GIT_AUTHOR_NAME", "T")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "T")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .status()
                .unwrap()
                .success(),
            "git {args:?}"
        );
    }

    #[test]
    fn lfs_attributes() {
        assert!(attributes_use_lfs(
            "# lfs\n*.psd filter=lfs diff=lfs merge=lfs -text\n"
        ));
        assert!(!attributes_use_lfs(
            "*.psd -filter\n# *.x filter=lfs\n*.md text\n"
        ));
        let git = Arc::new(crate::find_git().unwrap());
        let dir = tempfile::tempdir().unwrap();
        run(dir.path(), &["init", "-q"]);
        assert!(!is_using_lfs_by_attributes(git.clone(), dir.path()));
        std::fs::create_dir(dir.path().join("art")).unwrap();
        std::fs::write(dir.path().join("art/.gitattributes"), "*.png filter=lfs\n").unwrap();
        // an untracked nested file is not read (git lfs track would)
        assert!(!is_using_lfs_by_attributes(git.clone(), dir.path()));
        run(dir.path(), &["add", "art/.gitattributes"]);
        assert!(is_using_lfs_by_attributes(git, dir.path()));
    }

    /// The rules `git lfs track` writes, without needing git-lfs.
    #[test]
    fn lfs_tracking_by_check_attr() {
        let git = Arc::new(crate::find_git().unwrap());
        let dir = tempfile::tempdir().unwrap();
        run(dir.path(), &["init", "-q"]);
        let tracked = |path: &str| is_tracked_by_lfs(git.clone(), dir.path(), path).unwrap();
        assert!(!tracked("README.md"));
        std::fs::write(
            dir.path().join(".gitattributes"),
            "*.png filter=lfs diff=lfs merge=lfs -text\n\
             app/src/*.psd filter=lfs diff=lfs merge=lfs -text\n",
        )
        .unwrap();
        assert!(tracked("photo.png"));
        assert!(tracked("app/src/some cool photo.png"));
        assert!(tracked("app/src/art.psd"));
        assert!(!tracked("art.psd"));
        assert_eq!(
            files_not_tracked_by_lfs(
                git,
                dir.path(),
                &["a.mp4", "b.png", "art.psd", "app/src/art.psd", "-dash.md"]
            )
            .unwrap(),
            ["a.mp4", "art.psd", "-dash.md"]
        );
    }

    #[test]
    fn fast_forwards_a_branch_that_is_not_checked_out() {
        let git = Arc::new(crate::find_git().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let bare = dir.path().join("remote.git");
        let (work, other) = (dir.path().join("work"), dir.path().join("other"));
        run(
            dir.path(),
            &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()],
        );
        run(
            dir.path(),
            &["init", "-q", "-b", "main", work.to_str().unwrap()],
        );
        run(&work, &["config", "commit.gpgsign", "false"]);
        run(&work, &["commit", "-q", "--allow-empty", "-m", "first"]);
        run(&work, &["remote", "add", "origin", bare.to_str().unwrap()]);
        run(&work, &["push", "-q", "-u", "origin", "main"]);
        run(&work, &["checkout", "-q", "-b", "topic"]);
        run(
            dir.path(),
            &[
                "clone",
                "-q",
                bare.to_str().unwrap(),
                other.to_str().unwrap(),
            ],
        );
        run(&other, &["config", "commit.gpgsign", "false"]);
        run(&other, &["commit", "-q", "--allow-empty", "-m", "second"]);
        run(&other, &["push", "-q", "origin", "main"]);
        fast_forward_branch_from_remote(git.clone(), &work, "origin", "main", "main", None)
            .unwrap();
        let rev = |r: &str| {
            let out = std::process::Command::new("git")
                .args(["rev-parse", r])
                .current_dir(&work)
                .output()
                .unwrap();
            String::from_utf8(out.stdout).unwrap()
        };
        assert_eq!(rev("main"), rev("origin/main"));
        assert_ne!(rev("main"), rev("topic"));
        // a diverged branch is refused
        run(&work, &["checkout", "-q", "main"]);
        run(&work, &["commit", "-q", "--allow-empty", "-m", "local"]);
        run(&work, &["checkout", "-q", "topic"]);
        run(&other, &["commit", "-q", "--allow-empty", "-m", "third"]);
        run(&other, &["push", "-q", "origin", "main"]);
        assert!(
            fast_forward_branch_from_remote(git, &work, "origin", "main", "main", None).is_err()
        );
    }

    #[test]
    fn fetch_skips_submodules_when_gitmodules_is_broken() {
        let git = Arc::new(crate::find_git().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let up = dir.path().join("up");
        let work = dir.path().join("work");
        run(
            dir.path(),
            &["init", "-q", "-b", "main", up.to_str().unwrap()],
        );
        run(&up, &["config", "commit.gpgsign", "false"]);
        run(&up, &["commit", "-q", "--allow-empty", "-m", "first"]);
        run(
            dir.path(),
            &["clone", "-q", up.to_str().unwrap(), work.to_str().unwrap()],
        );
        std::fs::write(
            work.join(".gitmodules"),
            "[submodule \"x\"]\n<<<<<<< HEAD\n\tpath = x\n=======\n",
        )
        .unwrap();
        let fetch = |retry| {
            let options = FetchOptions {
                retry_bad_gitmodules: retry,
                ..FetchOptions::default()
            };
            fetch_with(git.clone(), &work, "origin", options, None, &mut |_, _| {})
        };
        assert!(fetch(false).is_err());
        fetch(true).unwrap();
    }

    #[test]
    fn linked_worktree_counts_the_main_fetch_head() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        let linked = dir.path().join("linked");
        run(
            dir.path(),
            &["init", "-q", "-b", "main", main.to_str().unwrap()],
        );
        run(&main, &["config", "commit.gpgsign", "false"]);
        run(&main, &["commit", "-q", "--allow-empty", "-m", "first"]);
        run(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "side",
                linked.to_str().unwrap(),
            ],
        );
        assert_ne!(crate::git_dir(&linked), crate::paths::common_dir(&linked));
        assert_eq!(
            crate::paths::common_dir(&linked).canonicalize().unwrap(),
            main.join(".git").canonicalize().unwrap()
        );
        std::fs::write(main.join(".git/FETCH_HEAD"), "abc\t\tbranch 'main'\n").unwrap();
        assert!(last_fetched(&linked, false).is_none());
        assert!(last_fetched(&linked, true).is_some());
        assert_eq!(last_fetched(&main, true), last_fetched(&main, false));
    }

    #[test]
    fn qualified_refspecs_push_a_branch_shadowed_by_a_tag() {
        let git = Arc::new(crate::find_git().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let bare = dir.path().join("remote.git");
        let work = dir.path().join("work");
        run(
            dir.path(),
            &["init", "-q", "--bare", bare.to_str().unwrap()],
        );
        run(
            dir.path(),
            &["init", "-q", "-b", "main", work.to_str().unwrap()],
        );
        run(&work, &["config", "commit.gpgsign", "false"]);
        run(&work, &["commit", "-q", "--allow-empty", "-m", "first"]);
        run(&work, &["tag", "main"]);
        run(&work, &["remote", "add", "origin", bare.to_str().unwrap()]);
        let push_as = |local: &str, remote: Option<&str>| {
            push(
                git.clone(),
                &work,
                "origin",
                local,
                remote,
                &[],
                false,
                None,
                &mut |_, _| {},
            )
        };
        // `main` alone matches both refs/heads/main and refs/tags/main
        assert!(push_as("main", None).is_err());
        push_as("refs/heads/main", None).unwrap();
        run(&work, &["commit", "-q", "--allow-empty", "-m", "second"]);
        assert!(push_as("main", Some("main")).is_err());
        push_as("refs/heads/main", Some("refs/heads/main")).unwrap();
        assert_eq!(
            config_value(git.clone(), &work, "branch.main.merge").as_deref(),
            Some("refs/heads/main")
        );
        run(&work, &["push", "-q", "origin", "refs/tags/main"]);
        assert!(crate::delete_remote_branch(git.clone(), &work, "origin", "main").is_err());
        crate::delete_remote_branch(git, &work, "origin", "refs/heads/main").unwrap();
    }

    #[test]
    fn the_proxy_is_resolved_for_the_remote_url() {
        let git = Arc::new(crate::find_git().unwrap());
        let dir = tempfile::tempdir().unwrap();
        run(dir.path(), &["init", "-q", "-b", "main"]);
        run(
            dir.path(),
            &["remote", "add", "origin", "https://example.com/o/n.git"],
        );
        let url = |remote: &str| remote_url_for_proxy(git.clone(), dir.path(), remote);
        assert_eq!(url("origin"), "https://example.com/o/n.git");
        assert_eq!(
            url("http://other.example/x.git"),
            "http://other.example/x.git"
        );
        assert_eq!(url("/some/path"), "/some/path");
    }

    #[test]
    fn fast_forward_skips_branches_checked_out_in_other_worktrees() {
        let git = Arc::new(crate::find_git().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let path = |name: &str| dir.path().join(name);
        let (bare, work, other) = (path("remote.git"), path("work"), path("other"));
        let (linked, rebasing) = (path("linked"), path("rebasing"));
        let s = |p: &Path| p.to_str().unwrap().to_string();
        run(
            dir.path(),
            &["init", "-q", "--bare", "-b", "main", &s(&bare)],
        );
        run(dir.path(), &["init", "-q", "-b", "main", &s(&work)]);
        run(&work, &["config", "commit.gpgsign", "false"]);
        run(&work, &["commit", "-q", "--allow-empty", "-m", "first"]);
        run(&work, &["remote", "add", "origin", &s(&bare)]);
        run(&work, &["push", "-q", "-u", "origin", "main"]);
        for branch in ["a", "b", "c"] {
            run(&work, &["branch", "-q", "--track", branch, "origin/main"]);
        }
        // `b` is checked out in a linked worktree, `c` is being rebased in one
        run(&work, &["worktree", "add", "-q", &s(&linked), "b"]);
        run(&work, &["worktree", "add", "-q", &s(&rebasing), "c"]);
        run(&rebasing, &["commit", "-q", "--allow-empty", "-m", "local"]);
        let rebase = std::process::Command::new("git")
            .args(["rebase", "-q", "-x", "false", "HEAD~1"])
            .current_dir(&rebasing)
            .env("GIT_COMMITTER_NAME", "T")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .unwrap();
        assert!(
            !rebase.status.success(),
            "the rebase stops at the failing exec"
        );
        run(dir.path(), &["clone", "-q", &s(&bare), &s(&other)]);
        run(&other, &["config", "commit.gpgsign", "false"]);
        run(&other, &["commit", "-q", "--allow-empty", "-m", "second"]);
        run(&other, &["push", "-q", "origin", "main"]);
        run(&work, &["fetch", "-q", "origin"]);
        let rev = |r: &str| {
            let out = std::process::Command::new("git")
                .args(["rev-parse", r])
                .current_dir(&work)
                .output()
                .unwrap();
            String::from_utf8(out.stdout).unwrap()
        };
        let (b_before, c_before) = (rev("b"), rev("c"));
        // GHD: git refuses the whole update, so `a` stays behind too
        assert!(fast_forward_branches(git.clone(), &work).is_err());
        assert_ne!(rev("a"), rev("origin/main"));
        // `282-fast-forward-skips-worktree-branches`
        assert_eq!(
            fast_forward_branches_with(git.clone(), &work, true).unwrap(),
            1
        );
        assert_eq!(rev("a"), rev("origin/main"));
        assert_eq!(rev("b"), b_before);
        assert_eq!(rev("c"), c_before);
    }

    #[test]
    fn push_error_includes_hook_stdout() {
        let git = Arc::new(crate::find_git().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let bare = dir.path().join("remote.git");
        let work = dir.path().join("work");
        run(
            dir.path(),
            &["init", "-q", "--bare", bare.to_str().unwrap()],
        );
        run(
            dir.path(),
            &["init", "-q", "-b", "main", work.to_str().unwrap()],
        );
        run(&work, &["config", "commit.gpgsign", "false"]);
        run(&work, &["commit", "-q", "--allow-empty", "-m", "first"]);
        let hook = work.join(".git/hooks/pre-push");
        std::fs::write(
            &hook,
            "#!/bin/sh\necho hook-stdout\necho hook-stderr >&2\nexit 1\n",
        )
        .unwrap();
        // Git for Windows runs a hook that starts with `#!`, whatever its mode
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        run(&work, &["remote", "add", "origin", bare.to_str().unwrap()]);
        let err = push(
            git,
            &work,
            "origin",
            "main",
            None,
            &[],
            false,
            None,
            &mut |_, _| {},
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("hook-stdout\nhook-stderr"), "{err}");
    }

    /// Git LFS's pre-push hook writes to `GIT_LFS_PROGRESS`; a stand-in hook
    /// does the same without git-lfs.
    #[test]
    fn push_reports_lfs_progress_from_the_progress_file() {
        let git = Arc::new(crate::find_git().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let bare = dir.path().join("remote.git");
        let work = dir.path().join("work");
        run(
            dir.path(),
            &["init", "-q", "--bare", bare.to_str().unwrap()],
        );
        run(
            dir.path(),
            &["init", "-q", "-b", "main", work.to_str().unwrap()],
        );
        run(&work, &["config", "commit.gpgsign", "false"]);
        run(&work, &["commit", "-q", "--allow-empty", "-m", "first"]);
        let hook = work.join(".git/hooks/pre-push");
        std::fs::write(
            &hook,
            "#!/bin/sh\nprintf '%s' \"$GIT_LFS_PROGRESS\" > ../progress-path\n\
             printf 'upload 1/1 0/4 a.bin\\nupload 1/1 4/4 a.bin\\n' >> \"$GIT_LFS_PROGRESS\"\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        run(&work, &["remote", "add", "origin", bare.to_str().unwrap()]);
        let mut events = Vec::new();
        push_with_progress(
            git,
            &work,
            "origin",
            "main",
            None,
            &[],
            false,
            None,
            &mut |progress| events.push(progress),
        )
        .unwrap();
        assert_eq!(events[0].kind, "push");
        assert_eq!(events[0].title, "Pushing to origin");
        assert_eq!(
            (events[0].description.as_deref(), events[0].value),
            (None, 0.)
        );
        let descriptions: Vec<_> = events
            .iter()
            .filter_map(|e| e.description.as_deref())
            .collect();
        assert!(
            descriptions
                .contains(&"Uploading a.bin (1 out of an estimated 1 completed, 4 B / 4 B)"),
            "{descriptions:?}"
        );
        let progress_path = std::fs::read_to_string(dir.path().join("progress-path")).unwrap();
        assert!(!progress_path.is_empty());
        assert!(!Path::new(&progress_path).exists(), "{progress_path}");
    }

    #[test]
    fn prune_remote_clears_a_ref_blocking_a_fetch() {
        let git = Arc::new(crate::find_git().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        run(
            dir.path(),
            &["init", "-q", "-b", "main", src.to_str().unwrap()],
        );
        run(&src, &["config", "commit.gpgsign", "false"]);
        std::fs::write(src.join("a.txt"), "one\n").unwrap();
        run(&src, &["add", "."]);
        run(&src, &["commit", "-q", "-m", "first"]);
        run(&src, &["branch", "feature"]);
        let copy = dir.path().join("copy");
        run(
            dir.path(),
            &["clone", "-q", src.to_str().unwrap(), copy.to_str().unwrap()],
        );
        // `feature` becomes `feature/x` upstream; origin/feature blocks it
        run(&src, &["branch", "-D", "feature"]);
        run(&src, &["branch", "feature/x"]);
        let fetch_no_prune = || {
            remote_command(git.clone(), &copy, None)
                .args(["fetch", "origin"])
                .run()
        };
        let err = fetch_no_prune().unwrap_err();
        assert!(is_stale_remote_ref_failure(&err), "{err}");
        prune_remote(git.clone(), &copy, "origin", None).unwrap();
        fetch_no_prune().unwrap();
    }

    #[test]
    fn cloned_at_reads_the_clone_reflog_entry() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        run(
            dir.path(),
            &["init", "-q", "-b", "main", src.to_str().unwrap()],
        );
        run(&src, &["config", "commit.gpgsign", "false"]);
        std::fs::write(src.join("a.txt"), "one\n").unwrap();
        run(&src, &["add", "."]);
        run(&src, &["commit", "-q", "-m", "first"]);
        assert!(cloned_at(&src).is_none());
        let copy = dir.path().join("copy");
        run(
            dir.path(),
            &["clone", "-q", src.to_str().unwrap(), copy.to_str().unwrap()],
        );
        assert!(last_fetched(&copy, false).is_none());
        let git = Arc::new(crate::find_git().unwrap());
        assert!(remote_head_resolves(git.clone(), &copy, "origin"));
        assert!(!remote_head_resolves(git.clone(), &src, "origin"));
        run(&copy, &["update-ref", "-d", "refs/remotes/origin/main"]);
        assert!(!remote_head_resolves(git, &copy, "origin"));
        let at = cloned_at(&copy).unwrap();
        let age = SystemTime::now().duration_since(at).unwrap();
        assert!(age.as_secs() < 600, "{age:?}");
    }

    #[test]
    fn fetch_pull_push_against_a_local_bare_remote() {
        let git = Arc::new(crate::find_git().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let bare = dir.path().join("remote.git");
        let work = dir.path().join("work");
        let other = dir.path().join("other");
        run(
            dir.path(),
            &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()],
        );
        run(
            dir.path(),
            &["init", "-q", "-b", "main", work.to_str().unwrap()],
        );
        run(&work, &["config", "commit.gpgsign", "false"]);
        std::fs::write(work.join("a.txt"), "one\n").unwrap();
        run(&work, &["add", "."]);
        run(&work, &["commit", "-q", "-m", "first"]);
        add_remote(git.clone(), &work, "origin", bare.to_str().unwrap()).unwrap();
        let remotes = get_remotes(git.clone(), &work).unwrap();
        assert_eq!(remotes.len(), 1);
        assert_eq!(find_default_remote(&remotes).unwrap().name, "origin");
        // publish the branch (sets the upstream)
        let mut seen = 0;
        push(
            git.clone(),
            &work,
            "origin",
            "main",
            None,
            &[],
            false,
            None,
            &mut |_, _| seen += 1,
        )
        .unwrap();
        assert_eq!(
            config_value(git.clone(), &work, "branch.main.remote").as_deref(),
            Some("origin")
        );
        // a tracking branch that is not checked out gets fast-forwarded later
        run(&work, &["branch", "--track", "mirror", "origin/main"]);
        // a second clone pushes a new commit; the first one is then behind
        run(
            dir.path(),
            &[
                "clone",
                "-q",
                bare.to_str().unwrap(),
                other.to_str().unwrap(),
            ],
        );
        run(&other, &["config", "commit.gpgsign", "false"]);
        std::fs::write(other.join("b.txt"), "two\n").unwrap();
        run(&other, &["add", "."]);
        run(&other, &["commit", "-q", "-m", "second"]);
        run(&other, &["push", "-q", "origin", "main"]);
        // a tag deleted on the remote survives a plain fetch; --prune-tags drops it
        run(&other, &["tag", "gone"]);
        run(&other, &["push", "-q", "origin", "gone"]);
        fetch(git.clone(), &work, "origin", None, &mut |_, _| {}).unwrap();
        run(&other, &["push", "-q", "origin", ":refs/tags/gone"]);
        fetch(git.clone(), &work, "origin", None, &mut |_, _| {}).unwrap();
        let has_tag = || {
            std::process::Command::new("git")
                .args(["rev-parse", "-q", "--verify", "refs/tags/gone"])
                .current_dir(&work)
                .output()
                .unwrap()
                .status
                .success()
        };
        assert!(has_tag());
        fetch_with_prune_tags(git.clone(), &work, "origin", true, None, &mut |_, _| {}).unwrap();
        assert!(!has_tag());
        let graph = FetchOptions {
            write_commit_graph: true,
            ..FetchOptions::default()
        };
        fetch_with(git.clone(), &work, "origin", graph, None, &mut |_, _| {}).unwrap();
        let objects = git_dir(&work).join("objects").join("info");
        assert!(objects.join("commit-graph").exists() || objects.join("commit-graphs").exists());
        assert!(last_fetched(&work, false).is_some());
        let ab = crate::symmetric_ahead_behind(git.clone(), &work, "main", "origin/main")
            .unwrap()
            .unwrap();
        assert_eq!((ab.ahead, ab.behind), (0, 1));
        // fast-forwarding updates `mirror` but leaves the checked-out `main` alone
        assert_eq!(fast_forward_branches(git.clone(), &work).unwrap(), 1);
        // a dirty working directory blocks the checked-out branch's fast-forward
        std::fs::write(work.join("a.txt"), "dirty\n").unwrap();
        assert!(!fast_forward_if_only_behind(git.clone(), &work).unwrap());
        run(&work, &["checkout", "--", "a.txt"]);
        let behind = crate::symmetric_ahead_behind(git.clone(), &work, "main", "origin/main")
            .unwrap()
            .unwrap();
        assert_eq!((behind.ahead, behind.behind), (0, 1));
        assert!(fast_forward_if_only_behind(git.clone(), &work).unwrap());
        assert!(!fast_forward_if_only_behind(git.clone(), &work).unwrap());
        run(&work, &["reset", "-q", "--hard", "HEAD~1"]);
        let ab = crate::symmetric_ahead_behind(git.clone(), &work, "mirror", "origin/main")
            .unwrap()
            .unwrap();
        assert_eq!((ab.ahead, ab.behind), (0, 0));
        let ab = crate::symmetric_ahead_behind(git.clone(), &work, "main", "origin/main")
            .unwrap()
            .unwrap();
        assert_eq!((ab.ahead, ab.behind), (0, 1));
        pull(git.clone(), &work, "origin", false, None, &mut |_, _| {}).unwrap();
        assert!(work.join("b.txt").exists());
        // diverge, then a plain push is rejected as non-fast-forward
        std::fs::write(other.join("c.txt"), "three\n").unwrap();
        run(&other, &["add", "."]);
        run(&other, &["commit", "-q", "-m", "third"]);
        run(&other, &["push", "-q", "origin", "main"]);
        std::fs::write(work.join("d.txt"), "four\n").unwrap();
        run(&work, &["add", "."]);
        run(&work, &["commit", "-q", "-m", "fourth"]);
        let err = push(
            git.clone(),
            &work,
            "origin",
            "main",
            Some("main"),
            &[],
            false,
            None,
            &mut |_, _| {},
        )
        .unwrap_err();
        assert_eq!(remote_failure(&err), RemoteFailure::PushNotFastForward);
        // someone else's push is not a rewrite of ours
        fetch(git.clone(), &work, "origin", None, &mut |_, _| {}).unwrap();
        let upstream = "refs/remotes/origin/main";
        assert!(!upstream_tip_in_reflog(
            git.clone(),
            &work,
            "main",
            upstream
        ));
        // pull merges the remote work in
        pull(git.clone(), &work, "origin", true, None, &mut |_, _| {}).unwrap();
        assert!(work.join("c.txt").exists());
        push(
            git.clone(),
            &work,
            "origin",
            "main",
            Some("main"),
            &[],
            false,
            None,
            &mut |_, _| {},
        )
        .unwrap();
        // amending the pushed commit rewrites it away
        run(&work, &["commit", "-q", "--amend", "-m", "merge, amended"]);
        assert!(upstream_tip_in_reflog(git, &work, "main", upstream));
    }

    #[test]
    fn deletes_a_tag_from_the_remote() {
        let git = Arc::new(crate::find_git().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let bare = dir.path().join("remote.git");
        let work = dir.path().join("work");
        run(
            dir.path(),
            &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()],
        );
        run(
            dir.path(),
            &["init", "-q", "-b", "main", work.to_str().unwrap()],
        );
        run(&work, &["config", "commit.gpgsign", "false"]);
        std::fs::write(work.join("a.txt"), "one\n").unwrap();
        run(&work, &["add", "."]);
        run(&work, &["commit", "-q", "-m", "first"]);
        run(&work, &["tag", "v1"]);
        add_remote(git.clone(), &work, "origin", bare.to_str().unwrap()).unwrap();
        run(&work, &["push", "-q", "origin", "main", "v1"]);
        let remote_tags = || {
            GitCommand::new(git.clone())
                .args(["tag", "-l"])
                .current_dir(&bare)
                .run()
                .unwrap()
                .stdout_string()
                .unwrap()
        };
        assert_eq!(remote_tags().trim(), "v1");
        delete_remote_tag(git.clone(), &work, "origin", "v1", None).unwrap();
        assert_eq!(remote_tags().trim(), "");
    }
}
