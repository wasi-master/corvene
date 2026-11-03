//! Write-side operations via the git CLI: init, initial commit, clone with
//! progress, path validation. Semantics follow GitHub Desktop's `lib/git/*`.
//!
//! Clone (`lib/git/clone.ts`) refuses a destination in a sensitive location
//! before anything is created, passes `-c init.defaultBranch` (the caller's
//! or the user's default branch) and `GIT_CLONE_PROTECTION_ACTIVE=false`,
//! the proxy of the system ([`crate::proxy`]), and reports progress through
//! GHD's `CloneProgressParser` with Git LFS progress merged in
//! (`executionOptionsWithProgress` with `trackLFSProgress`). The gits
//! Corvene supports name the checkout step `Updating files`, which GHD's
//! step list still calls `Checking out files`: GHD shows that step as
//! context, at the 80 % the steps before it reached. Deviation
//! ([`CloneOptions::updating_files_step`], flag
//! `281-clone-updating-files-step`): `Updating files` counts as the
//! checkout step, so the bar moves on to 100 %.
//!
//! Deviation ([`clone_failed_in_submodule`], flag
//! `285-clone-keeps-repo-on-submodule-failure`): a clone that only failed in
//! a submodule is kept; GHD `CloningRepositoriesStore.clone`
//! (`lib/stores/cloning-repositories-store.ts`) drops it on any error.
//!
//! Deviation ([`init_repository_with`], flag
//! `286-initial-commit-skips-large-files`): files over 100 MB that Git LFS
//! does not track stay out of a new repository's first commit; GHD
//! `createRepository` (`ui/add-repository/create-repository.tsx`) commits
//! the whole folder.

use std::path::{Component, MAIN_SEPARATOR_STR, Path, PathBuf};
use std::sync::Arc;

use tracing::info;

use crate::detect::GitBinary;
use crate::error::{GitError, Result};
use crate::process::GitCommand;
use crate::remote_ops::{AskpassEnv, GitProgressEvent, ProgressParser};

/// What a user-typed path looks like for Add / Create dialogs: GHD
/// `getRepositoryType`'s kind, read from the file system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathStatus {
    /// Nothing there (or not a directory).
    Missing,
    /// A directory with no repository in it or above it.
    NotARepository,
    /// Bare repository (GHD: "Bare repositories are not currently supported").
    Bare,
    /// Inside a working directory: `path` or a directory above it has a
    /// `.git` directory or file (submodule / worktree pointer).
    Repository,
}

/// GHD `getRepositoryType(path)`'s kind: like `git rev-parse` run in
/// `path`, the repository is looked for in `path` and the directories above
/// it, so a subdirectory of a working directory is `Repository`
/// ([`crate::top_level_working_directory`] names its top level). Read from
/// the file system rather than by running git, as the dialogs ask on every
/// keystroke: a repository's `.git` folder itself counts as bare (git calls
/// it regular), and `GIT_CEILING_DIRECTORIES` is not honoured.
pub fn path_status(path: &Path) -> PathStatus {
    if !path.is_dir() {
        return PathStatus::Missing;
    }
    path.ancestors()
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(root_path_status)
        .find(|status| matches!(status, PathStatus::Repository | PathStatus::Bare))
        .unwrap_or(PathStatus::NotARepository)
}

/// [`path_status`] of `path` itself, not looking above it: `Repository`
/// only for a working directory's top level. For the checks that need the
/// folder itself to be the repository (creating a repository there,
/// cloning from it).
pub fn root_path_status(path: &Path) -> PathStatus {
    if !path.is_dir() {
        return PathStatus::Missing;
    }
    let dot_git = path.join(".git");
    if dot_git.exists() {
        return PathStatus::Repository;
    }
    // bare layout: HEAD + objects + refs at the top level
    if path.join("HEAD").is_file() && path.join("objects").is_dir() && path.join("refs").is_dir() {
        return PathStatus::Bare;
    }
    PathStatus::NotARepository
}

/// Corvene (`289-add-repositories-in-folder`): the working directories in
/// the folders of `dir`, `depth` levels down (`.git` folder or file; found
/// repositories are not looked into, symbolic links not followed), sorted.
pub fn repositories_inside(dir: &Path, depth: usize) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let path = entry.path();
            if entry.file_name() == ".git" {
                continue;
            }
            if path.join(".git").exists() {
                out.push(path);
            } else if depth > 1 {
                walk(&path, depth - 1, out);
            }
        }
    }
    let mut out = Vec::new();
    if depth > 0 {
        walk(dir, depth, &mut out);
    }
    out.sort();
    out
}

/// Corvene addition (flag `875`): why git cannot open the repository at
/// `path`, when the reason is an unreadable configuration file
/// ([`crate::explain_bad_config`] of `git rev-parse --git-dir`).
pub fn explain_open_failure(git: Arc<GitBinary>, path: &Path) -> Option<String> {
    let out = GitCommand::new(git)
        .args(["rev-parse", "--git-dir"])
        .current_dir(path)
        .allow_exit_code(128)
        .run()
        .ok()?;
    crate::explain_bad_config(&out.stderr)
}

/// Flag `stale-core-worktree-hint`: when the repository found at `picked`
/// resolves to a working directory `workdir` that does not exist while
/// `picked` has its own `.git` (a `core.worktree` left over from a move),
/// what to tell the user.
pub fn explain_stale_worktree(picked: &Path, workdir: &Path) -> Option<String> {
    if workdir.is_dir() || !picked.join(".git").exists() {
        return None;
    }
    Some(format!(
        "The repository's core.worktree setting points to {}, which does not exist, so Git \
         does not use {} as its working directory.\n\nIf this folder is the working \
         directory, remove the setting and add it again:\n\n\
         git -C '{}' config --unset core.worktree",
        workdir.display(),
        picked.display(),
        picked.display()
    ))
}

/// Whether `dir` already has a `README.md` that "Initialize this repository
/// with a README" would replace (GHD `readMeExists`).
/// `git update-index -q --refresh`: records the files' current stat data in
/// the index, so the first status after the files were copied or moved does
/// not re-read every one of them. Modified files make git exit 1, which is
/// not a failure here.
pub fn refresh_index(git: Arc<GitBinary>, workdir: &Path) -> Result<()> {
    GitCommand::new(git)
        .args(["update-index", "-q", "--refresh"])
        .current_dir(workdir)
        .allow_exit_code(1)
        .run()?;
    Ok(())
}

pub fn readme_exists(dir: &Path) -> bool {
    dir.join("README.md").exists()
}

/// Turn `owner/name` or a GitHub URL without scheme into a clone URL
/// (GHD `parseRepositoryIdentifier` + `getDefaultDir`).
pub fn normalize_clone_url(input: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    let looks_like_url = input.contains("://")
        || input.starts_with("git@")
        || input.contains(':') && !input.contains('/');
    if looks_like_url {
        return Some(input.to_string());
    }
    let trimmed = input.trim_matches('/');
    let parts: Vec<&str> = trimmed.split('/').collect();
    match parts.as_slice() {
        [owner, name] if !owner.is_empty() && !name.is_empty() => {
            let name = name.strip_suffix(".git").unwrap_or(name);
            Some(format!("https://github.com/{owner}/{name}.git"))
        }
        ["github.com", owner, name] | ["www.github.com", owner, name] => {
            let name = name.strip_suffix(".git").unwrap_or(name);
            Some(format!("https://github.com/{owner}/{name}.git"))
        }
        _ => None,
    }
}

/// GHD `sanitizeCloneName` (`lib/remote-parsing.ts`): the folder the Clone
/// dialog names after a URL, on every platform the last non-empty `/`, `\`
/// or `:` separated part without `.git`; `None` when that is empty, `.` or
/// `..`, so a crafted URL cannot make the clone land outside the chosen
/// directory. Surrounding white space of the typed URL is ignored.
pub fn repository_name_from_url(url: &str) -> Option<String> {
    let last = url
        .trim()
        .split(['/', '\\', ':'])
        .rfind(|component| !component.is_empty())?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    if name.is_empty() || name == "." || name == ".." {
        None
    } else {
        Some(name.to_string())
    }
}

pub struct InitOptions {
    pub path: PathBuf,
    pub default_branch: Option<String>,
    pub description: Option<String>,
    pub readme: bool,
    /// `.gitignore` contents (a bundled template).
    pub gitignore: Option<String>,
    /// `LICENSE` contents (a rendered bundled template).
    pub license: Option<String>,
    /// `.gitattributes` contents; GHD always writes one when missing.
    pub git_attributes: Option<String>,
    /// Leave an existing README.md / .gitignore / LICENSE alone instead of
    /// replacing it (`220-create-repository-in-folder`, where the folder
    /// usually has files already).
    pub keep_existing: bool,
}

/// `git init` (+ README + initial commit when requested). Returns the workdir.
pub fn init_repository(git: Arc<GitBinary>, opts: InitOptions) -> Result<PathBuf> {
    init_repository_with(git, opts, None).map(|(path, _)| path)
}

/// [`init_repository`]; with `large_file_limit` (Corvene,
/// `286-initial-commit-skips-large-files`) the files larger than that many
/// bytes that Git LFS does not track are left out of "Initial commit" (they
/// stay as untracked files) and returned with the workdir. GHD
/// `createRepository` commits everything in the folder, and pushing the
/// result to GitHub then fails on files over 100 MB.
pub fn init_repository_with(
    git: Arc<GitBinary>,
    opts: InitOptions,
    large_file_limit: Option<u64>,
) -> Result<(PathBuf, Vec<String>)> {
    std::fs::create_dir_all(&opts.path).map_err(crate::error::GitError::Spawn)?;
    let mut args = vec!["init".to_string()];
    if let Some(branch) = &opts.default_branch {
        args.push("-b".into());
        args.push(branch.clone());
    }
    GitCommand::new(git.clone())
        .args(args)
        .current_dir(&opts.path)
        .run()?;
    // GHD `createRepository`: `writeGitDescription` of a non-empty
    // description, as typed (no newline added)
    if let Some(desc) = opts.description.as_deref().filter(|d| !d.is_empty())
        && let Err(err) = crate::write_git_description(&opts.path, desc)
    {
        tracing::warn!(%err, "could not write the repository description");
    }
    let mut wrote_files = false;
    let writable = |name: &str| !(opts.keep_existing && opts.path.join(name).exists());
    if opts.readme && writable("README.md") {
        let name = opts
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut readme = format!("# {name}\n");
        if let Some(desc) = opts.description.as_deref().filter(|d| !d.trim().is_empty()) {
            readme.push_str(&format!("{desc}\n"));
        }
        std::fs::write(opts.path.join("README.md"), readme)
            .map_err(crate::error::GitError::Spawn)?;
        wrote_files = true;
    }
    if let Some(text) = opts.gitignore.as_ref().filter(|_| writable(".gitignore")) {
        std::fs::write(opts.path.join(".gitignore"), text)
            .map_err(crate::error::GitError::Spawn)?;
        wrote_files = true;
    }
    if let Some(text) = opts.license.as_ref().filter(|_| writable("LICENSE")) {
        std::fs::write(opts.path.join("LICENSE"), text).map_err(crate::error::GitError::Spawn)?;
        wrote_files = true;
    }
    if let Some(text) = &opts.git_attributes
        && !opts.path.join(".gitattributes").exists()
    {
        std::fs::write(opts.path.join(".gitattributes"), text)
            .map_err(crate::error::GitError::Spawn)?;
        wrote_files = true;
    }
    // GHD `createRepository`: everything written above goes into "Initial commit".
    let mut left_out = Vec::new();
    if wrote_files {
        GitCommand::new(git.clone())
            .args(["add", "-A", "--"])
            .current_dir(&opts.path)
            .run()?;
        if let Some(limit) = large_file_limit {
            left_out = unstage_large_files(git.clone(), &opts.path, limit)?;
        }
        GitCommand::new(git)
            .args(["commit", "-q", "-m", "Initial commit"])
            .current_dir(&opts.path)
            .run()?;
    }
    info!(path = %opts.path.display(), "initialised repository");
    Ok((opts.path, left_out))
}

/// The staged files of a new repository over `limit` bytes that Git LFS does
/// not track, taken out of the index again (`rm --cached`).
fn unstage_large_files(git: Arc<GitBinary>, workdir: &Path, limit: u64) -> Result<Vec<String>> {
    let out = GitCommand::new(git.clone())
        .args(["ls-files", "-z"])
        .current_dir(workdir)
        .run()?;
    let staged: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect();
    let large = crate::large_file_paths(workdir, &staged, limit);
    if large.is_empty() {
        return Ok(large);
    }
    let large = crate::files_not_tracked_by_lfs(git.clone(), workdir, &large)?;
    if large.is_empty() {
        return Ok(large);
    }
    let mut stdin = Vec::new();
    for path in &large {
        stdin.extend_from_slice(path.as_bytes());
        stdin.push(0);
    }
    GitCommand::new(git)
        .args([
            "--literal-pathspecs",
            "rm",
            "--cached",
            "-q",
            "--pathspec-from-file=-",
            "--pathspec-file-nul",
        ])
        .current_dir(workdir)
        .stdin(stdin)
        .run()?;
    Ok(large)
}

/// `git config --global --get user.name/email`.
pub fn global_identity(git: Arc<GitBinary>) -> corvene_models::Identity {
    let get = |key: &str| {
        GitCommand::new(git.clone())
            .args(["config", "--global", "--get", key])
            .allow_exit_code(1)
            .run()
            .ok()
            .and_then(|o| o.stdout_string().ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    corvene_models::Identity {
        name: get("user.name"),
        email: get("user.email"),
        local: false,
    }
}

/// `git config --global user.name <name>` / `user.email <email>` (skips empty values).
pub fn set_global_identity(git: Arc<GitBinary>, name: &str, email: &str) -> Result<()> {
    if !name.trim().is_empty() {
        GitCommand::new(git.clone())
            .args(["config", "--global", "user.name", name.trim()])
            .run()?;
    }
    if !email.trim().is_empty() {
        GitCommand::new(git)
            .args(["config", "--global", "user.email", email.trim()])
            .run()?;
    }
    Ok(())
}

/// Progress reported while cloning: GHD's `ICloneProgress` (the line to show
/// and the overall fraction, 0..1).
#[derive(Clone, Debug, PartialEq)]
pub struct CloneProgress {
    pub description: String,
    /// From [`CloneProgressParser::parse`]: `None` for a line that is not
    /// progress of a clone step (GHD's `{ kind: 'context' }`). The clone's
    /// own reports always have a value: such a line keeps the fraction the
    /// steps before it reached, as GHD's do.
    pub value: Option<f32>,
}

/// GHD `CloneProgressParser` (`lib/progress/clone.ts`): a
/// [`ProgressParser`] over the clone steps, which keeps the highest step it
/// has seen and treats a line of an earlier step as context. One parser per
/// clone.
#[derive(Clone, Debug)]
pub struct CloneProgressParser(ProgressParser);

impl Default for CloneProgressParser {
    fn default() -> Self {
        Self::new()
    }
}

impl CloneProgressParser {
    pub fn new() -> Self {
        Self(ProgressParser::for_clone())
    }

    /// [`Self::new`] that counts `Updating files` as the `Checking out
    /// files` step (`281-clone-updating-files-step`).
    pub fn with_updating_files_step() -> Self {
        Self(ProgressParser::for_clone().with_alias("Updating files", "Checking out files"))
    }

    /// GHD `parse(line)`: the line's progress, `value` the overall fraction
    /// for a line of a clone step and `None` for context.
    pub fn parse(&mut self, line: &str) -> CloneProgress {
        clone_progress(&self.0.parse_event(line), false)
    }
}

/// What the clone callback gets for a parser event: the line, and for
/// `reports` (the clone's own reports) the last fraction for context too.
fn clone_progress(event: &GitProgressEvent, reports: bool) -> CloneProgress {
    let value = match event {
        GitProgressEvent::Progress { .. } => Some(event.percent() as f32),
        GitProgressEvent::Context { .. } => reports.then(|| event.percent() as f32),
    };
    CloneProgress {
        description: event.text().to_string(),
        value,
    }
}

/// [`CloneProgressParser::parse`] of one line by a fresh parser.
pub fn parse_clone_progress(line: &str) -> CloneProgress {
    CloneProgressParser::new().parse(line)
}

/// GHD `CloneOptions` (`models/clone-options.ts`) and the rest of what a
/// clone takes.
#[derive(Default)]
pub struct CloneOptions {
    /// `-b <branch>`: the branch to check out after cloning.
    pub branch: Option<String>,
    /// `init.defaultBranch` for an empty repository; the user's
    /// `init.defaultBranch`, else `main`, when `None` (GHD `getDefaultBranch`).
    pub default_branch: Option<String>,
    /// `--depth <n>`: a shallow clone (`233-shallow-clone`).
    pub depth: Option<u32>,
    /// Credentials for the clone (GHD `envForAuthentication`).
    pub askpass: Option<AskpassEnv>,
    /// Count git's `Updating files` lines as GHD's `Checking out files`
    /// step (`281-clone-updating-files-step`); off is GHD's parser.
    pub updating_files_step: bool,
    /// Skip the [`is_clone_path_sensitive`] backstop: the user confirmed
    /// the destination (`1123-confirm-sensitive-clone`).
    pub allow_sensitive_path: bool,
}

/// GHD `isClonePathSensitive`: `path`, resolved and lower-cased, is the
/// home directory, or `.ssh`, `.gnupg`, `.config`, `.config/git` or
/// `.gitconfig` in it or anything below them (Windows also `%APPDATA%` and
/// `%APPDATA%\gnupg`). A backstop against a crafted URL that made the Clone
/// dialog derive a path outside the chosen directory.
pub fn is_clone_path_sensitive(path: &Path) -> bool {
    let Some(home) = std::env::home_dir() else {
        return false;
    };
    let lower = |path: &Path| path.to_string_lossy().to_lowercase();
    let clone_path = lower(&resolve(path));
    let home = resolve(&home);
    if clone_path == lower(&home) {
        return true;
    }
    #[allow(unused_mut)]
    let mut sensitive: Vec<String> = [
        home.join(".ssh"),
        home.join(".gnupg"),
        home.join(".config"),
        home.join(".config").join("git"),
        home.join(".gitconfig"),
    ]
    .iter()
    .map(|path| lower(path))
    .collect();
    #[cfg(windows)]
    if let Some(app_data) = std::env::var_os("APPDATA").filter(|a| !a.is_empty()) {
        let app_data = PathBuf::from(app_data);
        sensitive.push(lower(&app_data));
        sensitive.push(lower(&resolve(&app_data.join("gnupg"))));
    }
    sensitive.iter().any(|location| {
        clone_path == *location
            || clone_path
                .strip_prefix(location.as_str())
                .is_some_and(|rest| rest.starts_with(MAIN_SEPARATOR_STR))
    })
}

/// Node's `Path.resolve(path)`: absolute (against the current directory),
/// `.` and `..` folded lexically, no symbolic links followed. Also used by
/// [`crate::parse_config_lock_file_path_from_error`].
pub(crate) fn resolve(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !matches!(
                    resolved.components().next_back(),
                    None | Some(Component::RootDir | Component::Prefix(_))
                ) {
                    resolved.pop();
                }
            }
            other => resolved.push(other),
        }
    }
    resolved
}

/// `git clone --progress --recurse-submodules <url> <path>` streaming
/// progress, with `default_branch` and `depth` as in [`CloneOptions`]. A
/// cancelled `cancel` token stops git, which removes what it created
/// (Windows: a terminated git removes nothing, so Corvene does).
pub fn clone(
    git: Arc<GitBinary>,
    url: &str,
    path: &Path,
    default_branch: Option<&str>,
    depth: Option<u32>,
    cancel: Option<crate::CancelToken>,
    on_progress: impl FnMut(CloneProgress),
) -> Result<()> {
    let options = CloneOptions {
        default_branch: default_branch.map(str::to_string),
        depth,
        ..CloneOptions::default()
    };
    clone_with_options(git, url, path, &options, cancel, on_progress)
}

/// GHD `clone(url, path, options, progressCallback)`: refuses a sensitive
/// destination ([`is_clone_path_sensitive`]) before creating anything, then
/// `git -c init.defaultBranch=<branch> clone --progress
/// --recurse-submodules [--depth <n>] [-b <branch>] -- <url> <path>`.
pub fn clone_with_options(
    git: Arc<GitBinary>,
    url: &str,
    path: &Path,
    options: &CloneOptions,
    cancel: Option<crate::CancelToken>,
    mut on_progress: impl FnMut(CloneProgress),
) -> Result<()> {
    if !options.allow_sensitive_path && is_clone_path_sensitive(path) {
        return Err(GitError::SensitiveClonePath(path.to_path_buf()));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(GitError::Spawn)?;
    }
    info!(url, path = %path.display(), "cloning");
    // GHD: `-c init.defaultBranch=<options.defaultBranch ?? getDefaultBranch()>`
    // so an empty repository starts on the right branch
    let default_branch = match &options.default_branch {
        Some(branch) => branch.clone(),
        None => crate::branch_ops::configured_default_branch(git.clone()),
    };
    let proxy_env =
        crate::proxy::env_for_remote_operation_with(git.clone(), None, || Some(url.to_string()));
    let mut cmd = GitCommand::new(git)
        .args([
            "-c".to_string(),
            format!("init.defaultBranch={default_branch}"),
        ])
        .env("GIT_CLONE_PROTECTION_ACTIVE", "false");
    for (key, value) in proxy_env {
        cmd = cmd.env(key, value);
    }
    if let Some(askpass) = &options.askpass {
        cmd = askpass.apply(cmd);
    }
    if let Some(token) = cancel {
        cmd = cmd.cancel_token(token);
    }
    cmd = cmd.args(["clone", "--progress", "--recurse-submodules"]);
    if let Some(depth) = options.depth {
        cmd = cmd.args(["--depth".to_string(), depth.to_string()]);
    }
    if let Some(branch) = &options.branch {
        cmd = cmd.args(["-b", branch.as_str()]);
    }
    #[cfg(windows)]
    let existed = path.exists();
    let cmd = cmd.args(["--", url]).arg(path);
    let mut parser = if options.updating_files_step {
        CloneProgressParser::with_updating_files_step()
    } else {
        CloneProgressParser::new()
    };
    let cloned = crate::lfs_progress::run_with_progress(cmd, &mut parser.0, &mut |event| {
        on_progress(clone_progress(&event, true))
    });
    #[cfg(windows)]
    if matches!(cloned, Err(GitError::Cancelled(_))) && !existed {
        // the killed processes let go of their files a moment later
        for _ in 0..20 {
            if std::fs::remove_dir_all(path).is_ok() || !path.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    cloned?;
    Ok(())
}

/// Corvene (`285-clone-keeps-repo-on-submodule-failure`): whether a failed `git
/// clone --recurse-submodules` only failed in a submodule, so the
/// repository itself is at `path`, cloned and checked out (git keeps it
/// then; it removes what it created only when the clone itself fails).
/// GHD `CloningRepositoriesStore.clone` drops the repository on any error.
pub fn clone_failed_in_submodule(path: &Path, err: &GitError) -> bool {
    let GitError::Failed { stderr, .. } = err else {
        return false;
    };
    // "clone of '…' into submodule path '…' failed", "Unable to checkout
    // '…' in submodule path '…'", "Fetched in submodule path '…'"
    stderr.contains("submodule path")
        && path.join(".git").exists()
        && gix::open(path)
            .ok()
            .is_some_and(|repo| repo.head_id().is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repositories_inside_look_two_levels_down() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for repo in ["a", "group/b", "group/deeper/c", "a/nested"] {
            std::fs::create_dir_all(root.join(repo).join(".git")).unwrap();
        }
        std::fs::create_dir_all(root.join("plain")).unwrap();
        std::fs::create_dir_all(root.join("wt")).unwrap();
        std::fs::write(root.join("wt/.git"), "gitdir: elsewhere").unwrap();
        assert_eq!(
            repositories_inside(root, 2),
            vec![root.join("a"), root.join("group/b"), root.join("wt")]
        );
        assert_eq!(
            repositories_inside(root, 1),
            vec![root.join("a"), root.join("wt")]
        );
    }

    #[test]
    fn large_files_stay_out_of_the_initial_commit() {
        // the user's global commit.gpgsign must not reach the test repo
        unsafe {
            std::env::set_var("GIT_CONFIG_PARAMETERS", "'commit.gpgsign=false'");
            std::env::set_var("GIT_AUTHOR_NAME", "T");
            std::env::set_var("GIT_AUTHOR_EMAIL", "t@example.com");
            std::env::set_var("GIT_COMMITTER_NAME", "T");
            std::env::set_var("GIT_COMMITTER_EMAIL", "t@example.com");
        }
        let git = Arc::new(crate::find_git().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("repo");
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("big [1].bin"), vec![0u8; 64]).unwrap();
        std::fs::write(path.join("small.txt"), "x").unwrap();
        let (workdir, left_out) = init_repository_with(
            git.clone(),
            InitOptions {
                path: path.clone(),
                default_branch: Some("main".into()),
                description: None,
                readme: true,
                gitignore: None,
                license: None,
                git_attributes: None,
                keep_existing: true,
            },
            Some(32),
        )
        .unwrap();
        assert_eq!(left_out, vec!["big [1].bin".to_string()]);
        let out = GitCommand::new(git)
            .args(["ls-files"])
            .current_dir(&workdir)
            .run()
            .unwrap();
        let committed = out.stdout_string().unwrap();
        assert!(committed.contains("small.txt"), "{committed}");
        assert!(!committed.contains("big"), "{committed}");
        assert!(path.join("big [1].bin").exists());
    }

    #[test]
    fn a_failed_submodule_keeps_the_clone() {
        let git = Arc::new(crate::find_git().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str], cwd: &Path| {
            GitCommand::new(git.clone())
                .args([
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "protocol.file.allow=always",
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.com",
                ])
                .args(args)
                .current_dir(cwd)
                .run()
                .unwrap();
        };
        let sub = dir.path().join("sub");
        let src = dir.path().join("src");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(&src).unwrap();
        run(&["init", "-q"], &sub);
        run(&["commit", "-q", "--allow-empty", "-m", "s"], &sub);
        run(&["init", "-q"], &src);
        run(&["submodule", "add", "-q", "../sub", "sub"], &src);
        run(&["commit", "-q", "-m", "init"], &src);
        let gone = dir.path().join("gone");
        run(
            &[
                "config",
                "-f",
                ".gitmodules",
                "submodule.sub.url",
                gone.to_string_lossy().as_ref(),
            ],
            &src,
        );
        run(&["commit", "-q", "-am", "broken"], &src);
        let dst = dir.path().join("dst");
        let err = clone_with_options(
            git.clone(),
            src.to_string_lossy().as_ref(),
            &dst,
            &CloneOptions::default(),
            None,
            |_| {},
        )
        .unwrap_err();
        assert!(clone_failed_in_submodule(&dst, &err), "{err}");
        // a clone that failed itself leaves nothing to keep
        let missing = dir.path().join("missing");
        let err = clone_with_options(
            git.clone(),
            gone.to_string_lossy().as_ref(),
            &missing,
            &CloneOptions::default(),
            None,
            |_| {},
        )
        .unwrap_err();
        assert!(!clone_failed_in_submodule(&missing, &err));
    }

    #[test]
    fn explains_a_stale_core_worktree() {
        let git = Arc::new(crate::find_git().unwrap());
        let dir = tempfile::tempdir().unwrap();
        GitCommand::new(git.clone())
            .args(["init", "-q"])
            .current_dir(dir.path())
            .run()
            .unwrap();
        let info = crate::open_repository(dir.path()).unwrap();
        assert_eq!(explain_stale_worktree(dir.path(), &info.workdir), None);
        let gone = dir.path().join("moved-away");
        GitCommand::new(git)
            .args(["config", "core.worktree"])
            .arg(gone.to_string_lossy().as_ref())
            .current_dir(dir.path())
            .run()
            .unwrap();
        let info = crate::open_repository(dir.path()).unwrap();
        let text = explain_stale_worktree(dir.path(), &info.workdir).unwrap();
        assert!(text.contains("config --unset core.worktree"), "{text}");
    }

    #[test]
    fn explains_a_broken_repository_config() {
        let git = Arc::new(crate::find_git().unwrap());
        let dir = tempfile::tempdir().unwrap();
        GitCommand::new(git.clone())
            .args(["init", "-q"])
            .current_dir(dir.path())
            .run()
            .unwrap();
        assert_eq!(explain_open_failure(git.clone(), dir.path()), None);
        let config = dir.path().join(".git").join("config");
        let mut text = std::fs::read_to_string(&config).unwrap();
        text.push_str("[core\n");
        std::fs::write(&config, text).unwrap();
        let explained = explain_open_failure(git, dir.path()).unwrap();
        assert!(explained.contains(".git/config"), "{explained}");
    }

    #[test]
    fn normalizes_clone_inputs() {
        assert_eq!(
            normalize_clone_url("wasi-master/corvene").as_deref(),
            Some("https://github.com/wasi-master/corvene.git")
        );
        assert_eq!(
            normalize_clone_url("github.com/wasi-master/corvene.git").as_deref(),
            Some("https://github.com/wasi-master/corvene.git")
        );
        assert_eq!(
            normalize_clone_url("git@github.com:a/b.git").as_deref(),
            Some("git@github.com:a/b.git")
        );
        assert_eq!(
            normalize_clone_url("https://example.com/x/y").as_deref(),
            Some("https://example.com/x/y")
        );
        assert!(normalize_clone_url("nope").is_none());
        assert!(normalize_clone_url("").is_none());
    }

    #[test]
    fn repository_names() {
        assert_eq!(
            repository_name_from_url("https://github.com/a/b.git").as_deref(),
            Some("b")
        );
        assert_eq!(
            repository_name_from_url("git@github.com:a/b").as_deref(),
            Some("b")
        );
        assert_eq!(
            repository_name_from_url("https://x/y/").as_deref(),
            Some("y")
        );
        assert_eq!(
            repository_name_from_url("x..\\..\\.ssh.git").as_deref(),
            Some(".ssh")
        );
        assert_eq!(repository_name_from_url("https://x/.."), None);
        assert_eq!(repository_name_from_url("https://x/.git"), None);
    }

    #[test]
    fn cancelled_clone_stops_and_leaves_nothing() {
        let git = Arc::new(crate::find_git().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let bare = dir.path().join("remote.git");
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q", "--bare"])
                .arg(&bare)
                .status()
                .unwrap()
                .success()
        );
        let url = bare.to_str().unwrap();
        let target = dir.path().join("clone");
        let token = crate::CancelToken::new();
        token.cancel();
        let err = clone(git.clone(), url, &target, None, None, Some(token), |_| {}).unwrap_err();
        assert!(matches!(err, crate::GitError::Cancelled(_)), "{err}");
        assert!(!target.exists());
        clone(
            git,
            url,
            &target,
            None,
            None,
            Some(crate::CancelToken::new()),
            |_| {},
        )
        .unwrap();
        assert!(target.join(".git").exists());
    }

    #[test]
    fn clone_progress_phases() {
        let p = parse_clone_progress("Receiving objects:  50% (500/1000), 1.2 MiB | 3 MiB/s");
        assert!((p.value.unwrap() - 0.4).abs() < 0.001);
        let p = parse_clone_progress("Resolving deltas: 100% (10/10), done.");
        assert!((p.value.unwrap() - 0.8).abs() < 0.001);
        assert!(parse_clone_progress("Cloning into 'x'...").value.is_none());
    }

    #[test]
    fn path_status_kinds() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(path_status(&dir.path().join("nope")), PathStatus::Missing);
        assert_eq!(path_status(dir.path()), PathStatus::NotARepository);
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        assert_eq!(path_status(dir.path()), PathStatus::Repository);
        // a subdirectory is inside the repository, but not its top level
        let sub = dir.path().join("src").join("deep");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(path_status(&sub), PathStatus::Repository);
        assert_eq!(root_path_status(&sub), PathStatus::NotARepository);
        assert_eq!(root_path_status(dir.path()), PathStatus::Repository);
    }

    #[test]
    fn sensitive_clone_paths() {
        let Some(home) = std::env::home_dir() else {
            return;
        };
        assert!(is_clone_path_sensitive(&home));
        assert!(is_clone_path_sensitive(&home.join(".ssh")));
        assert!(is_clone_path_sensitive(&home.join(".SSH").join("x")));
        assert!(is_clone_path_sensitive(
            &home.join("GitHub").join("..").join(".gnupg").join("y")
        ));
        assert!(is_clone_path_sensitive(&home.join(".config").join("git")));
        assert!(!is_clone_path_sensitive(&home.join("GitHub").join("repo")));
        assert!(!is_clone_path_sensitive(&home.join(".sshkeys-repo")));
    }

    #[test]
    fn shallow_clone_fetches_one_commit() {
        let dir = tempfile::tempdir().unwrap();
        let git = Arc::new(crate::find_git().unwrap());
        let source = dir.path().join("source");
        let run = |cwd: &Path, args: &[&str]| {
            let out = std::process::Command::new("git")
                .args([
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "user.name=T",
                    "-c",
                    "user.email=t@example.com",
                ])
                .args(args)
                .current_dir(cwd)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
            String::from_utf8(out.stdout).unwrap()
        };
        run(
            dir.path(),
            &["init", "-q", "-b", "main", source.to_str().unwrap()],
        );
        for n in ["one", "two"] {
            std::fs::write(source.join("a.txt"), n).unwrap();
            run(&source, &["add", "."]);
            run(&source, &["commit", "-q", "-m", n]);
        }
        // `--depth` is ignored for a plain local path
        let url = format!("file://{}", source.display());
        let shallow = dir.path().join("shallow");
        clone(git.clone(), &url, &shallow, None, Some(1), None, |_| {}).unwrap();
        assert_eq!(run(&shallow, &["rev-list", "--count", "HEAD"]).trim(), "1");
        let full = dir.path().join("full");
        clone(git, &url, &full, None, None, None, |_| {}).unwrap();
        assert_eq!(run(&full, &["rev-list", "--count", "HEAD"]).trim(), "2");
    }

    #[test]
    fn readme_exists_checks_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!readme_exists(dir.path()));
        assert!(!readme_exists(&dir.path().join("nope")));
        std::fs::write(dir.path().join("README.md"), "# mine\n").unwrap();
        assert!(readme_exists(dir.path()));
    }

    #[test]
    fn init_with_readme_commits() {
        // the user's global commit.gpgsign must not reach the test repo
        unsafe { std::env::set_var("GIT_CONFIG_PARAMETERS", "'commit.gpgsign=false'") };
        let dir = tempfile::tempdir().unwrap();
        let git = Arc::new(crate::find_git().unwrap());
        let path = dir.path().join("new-repo");
        // identity for the commit
        unsafe {
            std::env::set_var("GIT_AUTHOR_NAME", "T");
            std::env::set_var("GIT_AUTHOR_EMAIL", "t@example.com");
            std::env::set_var("GIT_COMMITTER_NAME", "T");
            std::env::set_var("GIT_COMMITTER_EMAIL", "t@example.com");
        }
        init_repository(
            git,
            InitOptions {
                path: path.clone(),
                default_branch: Some("main".into()),
                description: Some("desc".into()),
                readme: true,
                gitignore: None,
                license: None,
                git_attributes: None,
                keep_existing: false,
            },
        )
        .unwrap();
        let info = crate::open_repository(&path).unwrap();
        assert_eq!(info.current_branch().unwrap().name, "main");
        assert!(path.join("README.md").exists());

        // an existing folder keeps its README
        let existing = dir.path().join("existing");
        std::fs::create_dir(&existing).unwrap();
        std::fs::write(existing.join("README.md"), "mine\n").unwrap();
        init_repository(
            Arc::new(crate::find_git().unwrap()),
            InitOptions {
                path: existing.clone(),
                default_branch: Some("main".into()),
                description: None,
                readme: true,
                gitignore: Some("target\n".into()),
                license: None,
                git_attributes: None,
                keep_existing: true,
            },
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(existing.join("README.md")).unwrap(),
            "mine\n"
        );
        assert!(existing.join(".gitignore").exists());
    }
}
