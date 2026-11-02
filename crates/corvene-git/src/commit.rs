//! Staging, committing, discarding and undoing - GHD `lib/git/{update-index,
//! reset,commit,checkout-index}.ts`, `app-store._commitIncludedChanges` and
//! `GitStore.discardChanges` / `undoCommit` (`lib/stores/git-store.ts`).
//!
//! Deviation: [`discard_changes`] resets paths without naming `HEAD`, so it
//! also works on an unborn branch, and moves the files to the Trash in
//! batches instead of one call per file. Corvene can put executable bits staged
//! with `update-index --chmod` back after the restage
//! ([`staged_mode_changes`], `781-keep-staged-mode-changes`), and count the
//! files a commit stages as it goes ([`stage_files_with_progress`],
//! `1307-commit-progress`).

use std::path::Path;
use std::sync::Arc;

use corvene_models::{DiffSelectionType, FileStatusKind, WorkingDirectoryFileChange};
use tracing::info;

use crate::detect::GitBinary;
use crate::error::Result;
use crate::process::GitCommand;

#[derive(Clone, Debug, Default)]
pub struct CommitOptions {
    pub amend: bool,
    pub no_verify: bool,
    pub signoff: bool,
    pub allow_empty: bool,
    /// Corvene `783-amend-author`: another author for the commit.
    pub author: Option<CommitAuthor>,
    /// Corvene `799-fixup-commits`: `--fixup=<sha>`, git writes the message
    /// (`fixup! <summary of sha>`) and the one passed in is not used.
    pub fixup: Option<String>,
    /// Corvene `1116-ssh-signing-prompt`: a signing key's passphrase is
    /// asked for in the app.
    pub signing_askpass: bool,
}

/// Corvene `783-amend-author`: who an amended commit is by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommitAuthor {
    /// `--author="Name <email>"` (the author date stays).
    Given { name: String, email: String },
    /// `--reset-author`: the committer's identity, with a new author date.
    ResetToCommitter,
}

/// Corvene `783-amend-author`: a `Name <email>` author as typed, trimmed.
pub fn parse_commit_author(text: &str) -> std::result::Result<(String, String), &'static str> {
    const INVALID: &str = "Enter the author as Name <email>";
    let text = text.trim();
    let inner = text.strip_suffix('>').ok_or(INVALID)?;
    let (name, email) = inner.rsplit_once('<').ok_or(INVALID)?;
    let (name, email) = (name.trim(), email.trim());
    if name.is_empty() || name.contains(['<', '>', '\n']) {
        return Err(INVALID);
    }
    if email.is_empty() || email.contains(|c: char| c.is_whitespace() || c == '<' || c == '>') {
        return Err(INVALID);
    }
    Ok((name.to_string(), email.to_string()))
}

/// GHD `ReceiveLimit` (`lib/large-files.ts`): GitHub.com refuses pushes of
/// files over 100 MiB.
pub const RECEIVE_LIMIT: u64 = 100 * 1024 * 1024;

/// GHD `getLargeFilePaths` (`lib/large-files.ts`): the repository-relative
/// `paths` whose working file is larger than `limit` bytes, in order (a
/// path that cannot be read is left out, as GHD logs and skips it).
pub fn large_file_paths<S: AsRef<str>>(workdir: &Path, paths: &[S], limit: u64) -> Vec<String> {
    paths
        .iter()
        .map(AsRef::as_ref)
        .filter(|path| {
            std::fs::metadata(workdir.join(path))
                .is_ok_and(|meta| meta.is_file() && meta.len() > limit)
        })
        .map(str::to_string)
        .collect()
}

/// `git reset -- .` (GHD `unstageAll`). On an unborn branch there is no HEAD
/// to reset to, so fall back to clearing the index.
pub fn unstage_all(git: Arc<GitBinary>, workdir: &Path) -> Result<()> {
    let out = GitCommand::new(git.clone())
        .args(["reset", "--", "."])
        .current_dir(workdir)
        .allow_exit_code(128)
        .run()?;
    if !out.status.success() {
        GitCommand::new(git)
            .args(["rm", "-r", "--cached", "-q", "--ignore-unmatch", "--", "."])
            .current_dir(workdir)
            .allow_exit_code(128)
            .run()?;
    }
    Ok(())
}

/// GHD `stageFiles` (`lib/git/update-index.ts`) for every fully-included
/// file, in three `update-index` steps: force-remove the old paths of
/// renames (a new file may sit at the old path and stay out of the commit),
/// add the paths themselves, then force-remove the deleted files (one still
/// on disk, e.g. after `git rm --cached`, is committed as deleted).
/// Partially-selected files are staged separately by `stage_partial_files`
/// (`apply --cached`).
pub fn stage_files(
    git: Arc<GitBinary>,
    workdir: &Path,
    files: &[WorkingDirectoryFileChange],
) -> Result<()> {
    stage_whole_files_with(git, workdir, whole_files(files), None)
}

/// Corvene `1307-commit-progress`: [`stage_files`], telling `on_staged`
/// how many of the fully-included files are in the index so far. The
/// `update-index` that adds them runs with `--verbose`, which prints a line
/// per path as git hashes it; the count never passes the number of files.
pub fn stage_files_with_progress(
    git: Arc<GitBinary>,
    workdir: &Path,
    files: &[WorkingDirectoryFileChange],
    on_staged: &mut dyn FnMut(usize),
) -> Result<()> {
    stage_whole_files_with(git, workdir, whole_files(files), Some(on_staged))
}

/// The files [`stage_files`] stages: the fully-included ones.
fn whole_files(
    files: &[WorkingDirectoryFileChange],
) -> impl Iterator<Item = &WorkingDirectoryFileChange> {
    files
        .iter()
        .filter(|f| f.selection.kind() == DiffSelectionType::All)
}

/// The `update-index` steps of [`stage_files`] for `files`, whatever their
/// selection.
pub(crate) fn stage_whole_files<'a>(
    git: Arc<GitBinary>,
    workdir: &Path,
    files: impl IntoIterator<Item = &'a WorkingDirectoryFileChange>,
) -> Result<()> {
    stage_whole_files_with(git, workdir, files, None)
}

fn stage_whole_files_with<'a>(
    git: Arc<GitBinary>,
    workdir: &Path,
    files: impl IntoIterator<Item = &'a WorkingDirectoryFileChange>,
    on_staged: Option<&mut dyn FnMut(usize)>,
) -> Result<()> {
    let mut normal: Vec<&str> = Vec::new();
    let mut old_renamed: Vec<&str> = Vec::new();
    let mut deleted: Vec<&str> = Vec::new();
    for file in files {
        normal.push(&file.path);
        match file.status.kind {
            FileStatusKind::Renamed => old_renamed.extend(file.old_path.as_deref()),
            FileStatusKind::Deleted => deleted.push(&file.path),
            _ => {}
        }
    }
    update_index(git.clone(), workdir, &old_renamed, true, None)?;
    update_index(git.clone(), workdir, &normal, false, on_staged)?;
    update_index(git, workdir, &deleted, true, None)
}

/// GHD `updateIndex`: `update-index --add --remove [--force-remove]
/// --replace -z --stdin` for `paths`; nothing without paths. With
/// `on_staged` (`1307-commit-progress`), `--verbose` as well, its lines
/// counted as git writes them.
fn update_index(
    git: Arc<GitBinary>,
    workdir: &Path,
    paths: &[&str],
    force_remove: bool,
    on_staged: Option<&mut dyn FnMut(usize)>,
) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut args = vec!["update-index", "--add", "--remove"];
    if force_remove {
        args.push("--force-remove");
    }
    args.extend(["--replace", "-z"]);
    if on_staged.is_some() {
        // `--stdin` must come last
        args.push("--verbose");
    }
    args.push("--stdin");
    let cmd = GitCommand::new(git)
        .args(args)
        .current_dir(workdir)
        .stdin(nul_separated(paths));
    match on_staged {
        // one `add '<path>'` or `remove '<path>'` line per path (a path
        // with a line break in it counts twice, hence the cap)
        Some(on_staged) => {
            let mut staged = 0;
            cmd.forget_streamed().run_streaming_stdout(|_| {
                staged = (staged + 1).min(paths.len());
                on_staged(staged);
            })?;
        }
        None => {
            cmd.run()?;
        }
    }
    Ok(())
}

/// Corvene `781-keep-staged-mode-changes`: the executable bits the index
/// gives files differently from `HEAD` (the empty tree on an unborn
/// branch), as `git update-index --chmod=+x` leaves them, as `(path,
/// executable)` pairs. Only while `core.fileMode` is false: the working
/// tree cannot carry the bit then, so [`unstage_all`] and the restage of
/// [`stage_files`] would drop it (with file modes trusted, the restage takes
/// the working file's mode, as GHD's commit does). Empty otherwise.
pub fn staged_mode_changes(git: Arc<GitBinary>, workdir: &Path) -> Result<Vec<(String, bool)>> {
    let trusted = crate::handle::open(workdir)
        .ok()
        .map(|repo| repo.config_snapshot().boolean("core.fileMode"))
        .unwrap_or_else(|| {
            crate::config::boolean_config_value(git.clone(), workdir, "core.fileMode", false)
        })
        .unwrap_or(true);
    if trusted {
        return Ok(Vec::new());
    }
    let run = |base: &str| {
        GitCommand::new(git.clone())
            .args([
                "diff-index",
                "--cached",
                "--raw",
                "--no-renames",
                "-z",
                base,
                "--",
            ])
            .current_dir(workdir)
            .allow_exit_code(128)
            .run()
    };
    let mut out = run("HEAD")?;
    if out.status.code() == Some(128) {
        out = run(crate::log::NULL_TREE_SHA)?;
    }
    Ok(parse_mode_changes(&String::from_utf8_lossy(&out.stdout)))
}

/// The regular files of `diff-index --raw -z --no-renames` output whose
/// mode the index changes (or sets, for an added file), with whether the
/// index makes them executable.
fn parse_mode_changes(raw: &str) -> Vec<(String, bool)> {
    let pieces: Vec<&str> = raw.split('\0').collect();
    let (pairs, _) = pieces.as_chunks::<2>();
    pairs
        .iter()
        .filter_map(|[meta, path]| {
            let mut fields = meta.trim_start_matches(':').split(' ');
            let (old, new) = (fields.next()?, fields.next()?);
            (old != new && matches!(new, "100644" | "100755"))
                .then(|| (path.to_string(), new == "100755"))
        })
        .collect()
}

/// Puts the [`staged_mode_changes`] of the files the commit stages back
/// into the index (`update-index --chmod=±x`).
pub fn restore_mode_changes(
    git: Arc<GitBinary>,
    workdir: &Path,
    changes: &[(String, bool)],
) -> Result<()> {
    for executable in [true, false] {
        let paths: Vec<&str> = changes
            .iter()
            .filter(|(_, x)| *x == executable)
            .map(|(p, _)| p.as_str())
            .collect();
        if paths.is_empty() {
            continue;
        }
        GitCommand::new(git.clone())
            .args([
                "update-index",
                if executable {
                    "--chmod=+x"
                } else {
                    "--chmod=-x"
                },
                "-z",
                "--stdin",
            ])
            .current_dir(workdir)
            .stdin(nul_separated(&paths))
            .run()?;
    }
    Ok(())
}

/// Corvene `715-assume-unchanged`: `update-index --[no-]assume-unchanged`
/// for tracked `paths`, so git stops (or resumes) reporting their changes.
pub fn set_assume_unchanged(
    git: Arc<GitBinary>,
    workdir: &Path,
    paths: &[String],
    assume: bool,
) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut stdin: Vec<u8> = Vec::new();
    for path in paths {
        stdin.extend_from_slice(path.as_bytes());
        stdin.push(0);
    }
    GitCommand::new(git)
        .args([
            "update-index",
            if assume {
                "--assume-unchanged"
            } else {
                "--no-assume-unchanged"
            },
            "-z",
            "--stdin",
        ])
        .current_dir(workdir)
        .stdin(stdin)
        .run()?;
    Ok(())
}

/// Paths marked assume-unchanged (`ls-files -v`: a lower-case tag).
pub fn assume_unchanged_paths(git: Arc<GitBinary>, workdir: &Path) -> Result<Vec<String>> {
    let out = GitCommand::new(git)
        .args(["ls-files", "-v", "-z"])
        .current_dir(workdir)
        .run()?;
    Ok(out
        .stdout
        .split(|&b| b == 0)
        .filter_map(|entry| {
            let (tag, path) = (entry.first()?, entry.get(2..)?);
            tag.is_ascii_lowercase()
                .then(|| String::from_utf8_lossy(path).into_owned())
        })
        .collect())
}

/// `git add -- <paths>` (the tutorial repository's README).
pub fn add_paths(git: Arc<GitBinary>, workdir: &Path, paths: &[&str]) -> Result<()> {
    let mut args = vec!["add", "--"];
    args.extend_from_slice(paths);
    GitCommand::new(git).args(args).current_dir(workdir).run()?;
    Ok(())
}

/// GHD `parseCommitSHA` (`lib/git/core.ts`): the second word of `git
/// commit`'s `[<branch> <sha>] <summary>` line, which is the abbreviated
/// sha, or `(root-commit)` for a branch's first commit.
pub fn parse_commit_sha(stdout: &str) -> String {
    let head = stdout.split(']').next().unwrap_or_default();
    head.split(' ').nth(1).unwrap_or_default().to_string()
}

/// `git commit -F -` with the message on stdin (GHD `createCommit`).
/// Returns [`parse_commit_sha`] of git's output, as GHD does; callers that
/// need the full sha read [`head_sha`].
pub fn commit(
    git: Arc<GitBinary>,
    workdir: &Path,
    message: &str,
    opts: &CommitOptions,
) -> Result<String> {
    commit_with_terminal_output(git, workdir, message, opts, None)
}

/// [`commit`] with GHD's `onTerminalOutputAvailable` (the Committing changes
/// dialog's live output). The commit hooks (`interceptHooks`) run through
/// the hooks proxy while it is on, with the callbacks of
/// [`crate::hooks::with_hook_callbacks`].
pub fn commit_with_terminal_output(
    git: Arc<GitBinary>,
    workdir: &Path,
    message: &str,
    opts: &CommitOptions,
    on_terminal_output_available: Option<&crate::TerminalOutputCallback>,
) -> Result<String> {
    let mut args = vec!["commit".to_string()];
    match &opts.fixup {
        Some(sha) => args.push(format!("--fixup={sha}")),
        None => args.extend(["-F".into(), "-".into()]),
    }
    if opts.amend && opts.fixup.is_none() {
        args.push("--amend".into());
    }
    if opts.no_verify {
        args.push("--no-verify".into());
    }
    if opts.signoff {
        args.push("--signoff".into());
    }
    if opts.allow_empty {
        args.push("--allow-empty".into());
    }
    match &opts.author {
        Some(CommitAuthor::Given { name, email }) => {
            args.push(format!("--author={name} <{email}>"))
        }
        Some(CommitAuthor::ResetToCommitter) => args.push("--reset-author".into()),
        None => {}
    }
    // https://git-scm.com/docs/githooks/2.46.1
    let mut hooks = crate::hooks::COMMIT_HOOKS.to_vec();
    if opts.amend {
        hooks.insert(4, "post-rewrite");
    }
    let mut cmd = GitCommand::new(git)
        .args(args)
        .current_dir(workdir)
        .intercept_hooks(&hooks);
    if opts.fixup.is_none() {
        cmd = cmd.stdin(message.as_bytes().to_vec());
    }
    if opts.signing_askpass {
        cmd = crate::remote_ops::with_signing_askpass(cmd);
    }
    let out = match on_terminal_output_available {
        Some(callback) => cmd.run_with_terminal_output(callback)?,
        None => cmd.run()?,
    };
    let sha = parse_commit_sha(&String::from_utf8_lossy(&out.stdout));
    info!(%sha, "created commit");
    Ok(sha)
}

pub fn head_sha(git: Arc<GitBinary>, workdir: &Path) -> Result<String> {
    // in-process (a `git rev-parse` spawn otherwise)
    if let Some(id) = crate::handle::open(workdir)
        .ok()
        .and_then(|repo| repo.head_id().ok().map(|id| id.detach()))
    {
        return Ok(id.to_string());
    }
    let out = GitCommand::new(git)
        .args(["rev-parse", "HEAD"])
        .current_dir(workdir)
        .run()?;
    Ok(out.stdout_string()?.trim().to_string())
}

/// Corvene (`340-message-rules-defer-to-hooks`): whether git would run one
/// of the hooks `names` (`commit-msg`, …) on a commit: the file
/// `git rev-parse --git-path hooks/<name>` names (`core.hooksPath` and
/// linked worktrees included) exists and, outside Windows, is executable.
pub fn hook_exists(git: Arc<GitBinary>, workdir: &Path, names: &[&str]) -> bool {
    if names.is_empty() {
        return false;
    }
    let mut cmd = GitCommand::new(git).arg("rev-parse").current_dir(workdir);
    for name in names {
        cmd = cmd.arg("--git-path").arg(format!("hooks/{name}"));
    }
    let Ok(out) = cmd.run() else {
        return false;
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .any(|line| {
            let path = Path::new(line);
            is_runnable_hook(&if path.is_absolute() {
                path.to_path_buf()
            } else {
                workdir.join(path)
            })
        })
}

fn is_runnable_hook(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

/// GHD `mergeTrailers`: `git interpret-trailers --no-divider --trailer k=v …`
/// appends the trailers to a commit message (folding into an existing
/// trailer block when there is one).
pub fn merge_trailers(
    git: Arc<GitBinary>,
    workdir: &Path,
    message: &str,
    trailers: &[(String, String)],
) -> Result<String> {
    if trailers.is_empty() {
        return Ok(message.to_string());
    }
    let mut cmd = GitCommand::new(git)
        .args(["interpret-trailers", "--no-divider"])
        .current_dir(workdir);
    for (token, value) in trailers {
        cmd = cmd.arg("--trailer").arg(format!("{token}={value}"));
    }
    let out = cmd.stdin(message.as_bytes().to_vec()).run()?;
    out.stdout_string()
}

/// Summary + blank line + description, as GHD formats the message.
pub fn format_message(summary: &str, description: &str) -> String {
    let summary = summary.trim();
    let description = description.trim();
    if description.is_empty() {
        format!("{summary}\n")
    } else {
        format!("{summary}\n\n{description}\n")
    }
}

/// GHD `GitStore.undoCommit` for the `HEAD` commit: `git reset <parent>`
/// (mixed), so its changes stay in the working directory, unstaged. A root
/// commit (GHD `undoFirstCommit`): files deleted from the working directory
/// are checked out again (they would be lost with the commit), the branch's
/// HEAD ref is deleted and the index is cleared, leaving every file
/// untracked.
pub fn undo_last_commit(git: Arc<GitBinary>, workdir: &Path) -> Result<()> {
    let parent = GitCommand::new(git.clone())
        .args(["rev-parse", "--verify", "--quiet", "HEAD^"])
        .current_dir(workdir)
        .allow_exit_code(1)
        .run()?;
    if parent.status.success() {
        let parent = parent.stdout_string()?.trim().to_string();
        GitCommand::new(git)
            .args(["reset", &parent])
            .current_dir(workdir)
            .run()?;
    } else {
        let status = crate::status::get_status(git.clone(), workdir)?;
        let deleted: Vec<&str> = status
            .files
            .iter()
            .filter(|f| f.status.kind == FileStatusKind::Deleted)
            .map(|f| f.path.as_str())
            .collect();
        if !deleted.is_empty() {
            GitCommand::new(git.clone())
                .args(["checkout", "HEAD", "--"])
                .args(&deleted)
                .current_dir(workdir)
                .run()?;
        }
        GitCommand::new(git.clone())
            .args(["update-ref", "-d", "HEAD", "-m", "Reverting first commit"])
            .current_dir(workdir)
            .run()?;
        // keep the files: clear the index back to "unstaged"
        unstage_all(git, workdir)?;
    }
    Ok(())
}

/// Discard working-directory changes (GHD `GitStore.discardChanges`):
///
/// 1. every file that is not deleted (nor a submodule) goes to the Trash
///    (`moveToTrash`, so a discard is recoverable), or is deleted when
///    `move_to_trash` is off or the Trash fails;
/// 2. submodules are reset to their recorded commit (`submodule update
///    --recursive --force`);
/// 3. the discarded paths that differ between the index and `HEAD` are reset
///    (`git reset -- <paths>`), so other staged files stay staged;
/// 4. the discarded paths are checked out from the index (`checkout-index`;
///    exit code 1, for paths that are not in it, is fine). A copy or rename
///    resets its new path and checks out (and resets) its old one.
///
/// Corvene resets the paths without naming `HEAD` (GHD `reset HEAD --`), so
/// a staged new file can be discarded on an unborn branch too.
///
/// With `clean_submodules` (Corvene, flag `discard-submodule-changes`), a
/// submodule entry with changes inside also has its modified files checked
/// out and its untracked (not ignored) files moved to the Trash, so the entry
/// goes away; GHD leaves a submodule's untracked files.
///
/// A file the Trash refuses is deleted, or with `keep_untrashable` (GHD
/// `askForConfirmationOnDiscardChangesPermanently`) left alone, its changes
/// kept, and returned, so the caller can ask before discarding it for good
/// (GHD `DiscardChangesRetry`, which discards again without the Trash).
pub fn discard_changes(
    git: Arc<GitBinary>,
    workdir: &Path,
    files: &[WorkingDirectoryFileChange],
    move_to_trash: bool,
    clean_submodules: bool,
    keep_untrashable: bool,
) -> Result<Vec<String>> {
    let mut untrashable: Vec<String> = Vec::new();
    if clean_submodules {
        for file in files {
            if let Some(sub) = file.status.submodule_status {
                discard_inside_submodule(
                    git.clone(),
                    &workdir.join(&file.path),
                    sub,
                    move_to_trash,
                )?;
            }
        }
    }
    let on_disk = |file: &WorkingDirectoryFileChange| {
        !file.status.submodule && file.status.kind != FileStatusKind::Deleted
    };
    let full_path =
        |file: &WorkingDirectoryFileChange| workdir.join(file.path.trim_end_matches('/'));
    // one batch instead of a Trash call per file (#7155)
    let mut went_to_trash = if move_to_trash {
        let paths: Vec<std::path::PathBuf> =
            files.iter().filter(|f| on_disk(f)).map(full_path).collect();
        trash_paths(&paths)
    } else {
        Vec::new()
    }
    .into_iter();
    let mut to_checkout: Vec<&str> = Vec::new();
    let mut to_reset: Vec<&str> = Vec::new();
    let mut submodules: Vec<&str> = Vec::new();
    for file in files {
        if file.status.submodule {
            submodules.push(&file.path);
        } else if on_disk(file) {
            let full = full_path(file);
            let trashed = move_to_trash && went_to_trash.next().unwrap_or(false);
            if move_to_trash && keep_untrashable {
                if !trashed && full.symlink_metadata().is_ok() {
                    // kept as it is until the user agrees to lose it
                    untrashable.push(file.path.clone());
                    continue;
                }
            } else if !trashed {
                let _ = std::fs::remove_file(&full).or_else(|_| std::fs::remove_dir_all(&full));
            }
        }
        match (file.status.kind, file.old_path.as_deref()) {
            (FileStatusKind::Copied | FileStatusKind::Renamed, Some(old)) => {
                // the new path is gone already; the index must forget it
                to_reset.push(&file.path);
                to_checkout.push(old);
                to_reset.push(old);
            }
            _ => {
                to_checkout.push(&file.path);
                to_reset.push(&file.path);
            }
        }
    }
    let index_changes = index_changes(git.clone(), workdir)?;
    let to_reset: Vec<&str> = to_reset
        .into_iter()
        .filter(|p| index_changes.contains_key(*p))
        .collect();
    let submodule_paths: Vec<&str> = to_checkout
        .iter()
        .copied()
        .filter(|p| submodules.contains(p))
        .collect();
    // GHD's filter: only a submodule that was added in the index is left out
    let to_checkout: Vec<&str> = to_checkout
        .into_iter()
        .filter(|p| !submodule_paths.contains(p) || index_changes.get(*p) != Some(&'A'))
        .collect();
    crate::submodule::reset_submodule_paths(git.clone(), workdir, &submodule_paths)?;
    if !to_reset.is_empty() {
        GitCommand::new(git.clone())
            .args([
                "reset",
                "-q",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ])
            .env("GIT_LITERAL_PATHSPECS", "1")
            .current_dir(workdir)
            .stdin(nul_separated(&to_reset))
            .run()?;
    }
    if !to_checkout.is_empty() {
        GitCommand::new(git)
            .args(["checkout-index", "-f", "-u", "-q", "--stdin", "-z"])
            .current_dir(workdir)
            .stdin(nul_separated(&to_checkout))
            .allow_exit_code(1)
            .run()?;
    }
    Ok(untrashable)
}

/// Corvene `1365-discard-explains-leftovers` (desktop/desktop#11784): what
/// still differs from the index after a discard.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiscardLeftovers {
    /// Only the file mode differs: the drive does not keep it.
    pub mode_only: Vec<String>,
    /// The contents differ too.
    pub changed: Vec<String>,
}

/// Refresh the index's cached file times, which clears files whose contents
/// never changed (GHD reads a status that does not take the index lock, so
/// they stay listed), and list what still differs among `paths`.
pub fn discard_leftovers(
    git: Arc<GitBinary>,
    workdir: &Path,
    paths: &[&str],
) -> Result<DiscardLeftovers> {
    GitCommand::new(git.clone())
        .args(["update-index", "-q", "--refresh"])
        .current_dir(workdir)
        .allow_exit_code(1)
        .run()?;
    let mut leftovers = DiscardLeftovers::default();
    if paths.is_empty() {
        return Ok(leftovers);
    }
    let out = GitCommand::new(git.clone())
        .args(["diff-files", "--raw", "--no-renames", "-z", "--"])
        .args(paths.iter().copied())
        .env("GIT_LITERAL_PATHSPECS", "1")
        .current_dir(workdir)
        .run()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let pieces: Vec<&str> = text.split('\0').collect();
    let (pairs, _) = pieces.as_chunks::<2>();
    for [meta, path] in pairs {
        let mut fields = meta.trim_start_matches(':').split(' ');
        let (Some(old_mode), Some(new_mode)) = (fields.next(), fields.next()) else {
            continue;
        };
        // a file on disk with the index's blob differs in its mode alone
        let index_oid = fields.next().unwrap_or_default();
        let same_blob = GitCommand::new(git.clone())
            .args(["hash-object", "--"])
            .arg(path)
            .current_dir(workdir)
            .run()
            .ok()
            .and_then(|out| out.stdout_string().ok())
            .is_some_and(|oid| oid.trim() == index_oid);
        if old_mode != new_mode && same_blob {
            leftovers.mode_only.push(path.to_string());
        } else {
            leftovers.changed.push(path.to_string());
        }
    }
    Ok(leftovers)
}

/// Delete worktree paths (files or folders) for good: the new files
/// [`discard_changes`] could not move to the Trash, once the user agreed.
pub fn delete_worktree_paths(workdir: &Path, paths: &[String]) -> Result<()> {
    for path in paths {
        let full = workdir.join(path);
        let removed = if full.is_dir() && !full.is_symlink() {
            std::fs::remove_dir_all(&full)
        } else {
            std::fs::remove_file(&full)
        };
        match removed {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}

fn nul_separated(paths: &[&str]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    for path in paths {
        out.extend_from_slice(path.as_bytes());
        out.push(0);
    }
    out
}

/// GHD `getIndexChanges` (`lib/git/diff-index.ts`): the paths whose index
/// entry differs from `HEAD` (the empty tree on an unborn branch), with
/// their `--name-status` letter.
fn index_changes(
    git: Arc<GitBinary>,
    workdir: &Path,
) -> Result<std::collections::HashMap<String, char>> {
    let run = |base: &str| {
        GitCommand::new(git.clone())
            .args([
                "diff-index",
                "--cached",
                "--name-status",
                "--no-renames",
                "-z",
                base,
                "--",
            ])
            .current_dir(workdir)
            .allow_exit_code(128)
            .run()
    };
    let mut out = run("HEAD")?;
    if out.status.code() == Some(128) {
        out = run(crate::log::NULL_TREE_SHA)?;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let pieces: Vec<&str> = text.split('\0').collect();
    let (pairs, _) = pieces.as_chunks::<2>();
    Ok(pairs
        .iter()
        .filter_map(|[status, path]| Some((path.to_string(), status.chars().next()?)))
        .collect())
}

/// How many paths go to the Trash in one call.
#[cfg(not(target_os = "android"))]
const TRASH_BATCH: usize = 200;

/// The Trash as GHD reaches it (Electron's `shell.trashItem`): on macOS
/// `NSFileManager trashItemAtURL:`, without the `trash` crate's default
/// Finder round trip (an `osascript` per call, the Finder sound and an
/// Automation prompt).
#[cfg(not(target_os = "android"))]
fn trash_context() -> trash::TrashContext {
    #[allow(unused_mut)]
    let mut context = trash::TrashContext::default();
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        context.set_delete_method(DeleteMethod::NsFileManager);
    }
    context
}

/// Moves `paths` to the Trash, [`TRASH_BATCH`] at a time, and says for each
/// one whether it went (or is gone). A batch stops at its first failure, so
/// a failed batch is retried one path at a time: a path that is no longer
/// there went with the batch, the others are tried alone. GHD trashes one
/// file at a time. Android has no Trash: nothing goes and the caller
/// deletes.
fn trash_paths(paths: &[std::path::PathBuf]) -> Vec<bool> {
    #[cfg(not(target_os = "android"))]
    {
        let context = trash_context();
        let mut went = Vec::with_capacity(paths.len());
        for batch in paths.chunks(TRASH_BATCH) {
            if context.delete_all(batch).is_ok() {
                went.extend(std::iter::repeat_n(true, batch.len()));
                continue;
            }
            for path in batch {
                went.push(path.symlink_metadata().is_err() || context.delete(path).is_ok());
            }
        }
        went
    }
    #[cfg(target_os = "android")]
    {
        vec![false; paths.len()]
    }
}

/// Check out a submodule's modified files and trash its untracked ones.
fn discard_inside_submodule(
    git: Arc<GitBinary>,
    submodule: &Path,
    status: corvene_models::SubmoduleStatus,
    move_to_trash: bool,
) -> Result<()> {
    if !submodule.is_dir() {
        return Ok(());
    }
    if status.modified_changes {
        GitCommand::new(git.clone())
            .args(["checkout", "-f", "-q", "--", "."])
            .current_dir(submodule)
            .run()?;
    }
    if status.untracked_changes {
        let out = GitCommand::new(git)
            .args([
                "ls-files",
                "--others",
                "--exclude-standard",
                "--directory",
                "-z",
            ])
            .current_dir(submodule)
            .run()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let paths: Vec<std::path::PathBuf> = text
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(|path| submodule.join(path.trim_end_matches('/')))
            .collect();
        let went = if move_to_trash {
            trash_paths(&paths)
        } else {
            vec![false; paths.len()]
        };
        for (full, went) in paths.iter().zip(went) {
            if !went {
                let _ = std::fs::remove_file(full).or_else(|_| std::fs::remove_dir_all(full));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn delete_worktree_paths_removes_files_and_folders() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "a").unwrap();
        std::fs::create_dir_all(dir.path().join("d/e")).unwrap();
        std::fs::write(dir.path().join("d/e/f.txt"), "f").unwrap();
        delete_worktree_paths(
            dir.path(),
            &["a.txt".to_string(), "d".to_string(), "gone.txt".to_string()],
        )
        .unwrap();
        assert!(!dir.path().join("a.txt").exists());
        assert!(!dir.path().join("d").exists());
    }

    fn repo() -> (tempfile::TempDir, Arc<GitBinary>) {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(dir.path())
                    .status()
                    .unwrap()
                    .success()
            )
        };
        run(&["init", "-q", "-b", "main"]);
        // Git for Windows checks text out with CRLF unless told not to
        run(&["config", "core.autocrlf", "false"]);
        run(&["config", "commit.gpgsign", "false"]);
        run(&["config", "user.name", "T"]);
        run(&["config", "user.email", "t@example.com"]);
        (dir, Arc::new(crate::find_git().unwrap()))
    }

    #[test]
    fn finds_runnable_commit_message_hooks() {
        let (dir, git) = repo();
        let path = dir.path();
        let names = ["prepare-commit-msg", "commit-msg"];
        assert!(!hook_exists(git.clone(), path, &names));
        let hook = path.join(".git/hooks/commit-msg");
        std::fs::write(&hook, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // not executable: git skips it
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(!hook_exists(git.clone(), path, &names));
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(hook_exists(git.clone(), path, &names));
        // core.hooksPath moves them
        std::fs::create_dir_all(path.join("hooks")).unwrap();
        assert!(
            Command::new("git")
                .args(["config", "core.hooksPath", "hooks"])
                .current_dir(path)
                .status()
                .unwrap()
                .success()
        );
        assert!(!hook_exists(git, path, &names));
    }

    #[test]
    fn stage_commit_undo_round_trip() {
        let (dir, git) = repo();
        let path = dir.path();
        std::fs::write(path.join("a.txt"), "one\n").unwrap();
        std::fs::write(path.join("b.txt"), "two\n").unwrap();
        let mut status = crate::get_status(git.clone(), path).unwrap();
        // exclude b.txt
        for f in &mut status.files {
            if f.path == "b.txt" {
                f.selection = corvene_models::DiffSelection::none();
            }
        }
        unstage_all(git.clone(), path).unwrap();
        stage_files(git.clone(), path, &status.files).unwrap();
        let parsed = commit(
            git.clone(),
            path,
            &format_message("Add a", "details"),
            &CommitOptions::default(),
        )
        .unwrap();
        // GHD `parseCommitSHA` of a branch's first commit
        assert_eq!(parsed, "(root-commit)");
        let sha = head_sha(git.clone(), path).unwrap();
        let after = crate::get_status(git.clone(), path).unwrap();
        let paths: Vec<_> = after.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["b.txt"]);
        let log = Command::new("git")
            .args(["log", "-1", "--format=%B"])
            .current_dir(path)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&log.stdout).trim(),
            "Add a\n\ndetails"
        );

        // second commit then undo keeps changes
        std::fs::write(path.join("a.txt"), "changed\n").unwrap();
        let status = crate::get_status(git.clone(), path).unwrap();
        unstage_all(git.clone(), path).unwrap();
        stage_files(git.clone(), path, &status.files).unwrap();
        commit(git.clone(), path, "second\n", &CommitOptions::default()).unwrap();
        undo_last_commit(git.clone(), path).unwrap();
        let head = head_sha(git.clone(), path).unwrap();
        assert_eq!(head, sha);
        let status = crate::get_status(git.clone(), path).unwrap();
        assert!(status.files.iter().any(|f| f.path == "a.txt"));

        // undo the root commit: files stay, HEAD unborn
        undo_last_commit(git.clone(), path).unwrap();
        let info = crate::open_repository(path).unwrap();
        assert!(matches!(info.tip, corvene_models::Tip::Unborn { .. }));
        assert!(path.join("a.txt").exists());
    }

    #[test]
    fn staging_reports_progress_for_every_file() {
        let (dir, git) = repo();
        let path = dir.path();
        // Git for Windows refuses paths past MAX_PATH unless told not to
        assert!(
            Command::new("git")
                .args(["config", "core.longpaths", "true"])
                .current_dir(path)
                .status()
                .unwrap()
                .success()
        );
        // far more paths and verbose output than a pipe holds (~600 KB
        // each way): stdin and stdout must not wait on each other
        let folder = format!("many-{}", "x".repeat(180));
        std::fs::create_dir(path.join(&folder)).unwrap();
        for i in 0..2500 {
            let name = format!("{folder}/{}-{i:05}.txt", "y".repeat(60));
            std::fs::write(path.join(name), format!("{i}\n")).unwrap();
        }
        let status = crate::get_status(git.clone(), path).unwrap();
        assert_eq!(status.files.len(), 2500);
        let mut reports = Vec::new();
        stage_files_with_progress(git.clone(), path, &status.files, &mut |n| reports.push(n))
            .unwrap();
        assert_eq!(reports.len(), 2500);
        assert!(reports.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(reports.last(), Some(&2500));
        let staged = Command::new("git")
            .args(["ls-files", "--cached"])
            .current_dir(path)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&staged.stdout).lines().count(),
            2500
        );
    }

    #[test]
    fn mode_changes_survive_the_restage_without_trusted_modes() {
        let (dir, git) = repo();
        let path = dir.path();
        let run = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(path)
                    .status()
                    .unwrap()
                    .success()
            )
        };
        run(&["config", "core.fileMode", "false"]);
        std::fs::write(path.join("a.sh"), "a\n").unwrap();
        std::fs::write(path.join("b.txt"), "b\n").unwrap();
        run(&["add", "a.sh", "b.txt"]);
        run(&["commit", "-q", "-m", "init"]);
        std::fs::write(path.join("new.sh"), "n\n").unwrap();
        run(&["add", "new.sh"]);
        run(&["update-index", "--chmod=+x", "a.sh", "new.sh"]);
        let modes = staged_mode_changes(git.clone(), path).unwrap();
        assert_eq!(
            modes,
            vec![("a.sh".to_string(), true), ("new.sh".to_string(), true)]
        );
        let status = crate::get_status(git.clone(), path).unwrap();
        unstage_all(git.clone(), path).unwrap();
        stage_files(git.clone(), path, &status.files).unwrap();
        restore_mode_changes(git.clone(), path, &modes).unwrap();
        commit(git.clone(), path, "chmod\n", &CommitOptions::default()).unwrap();
        let tree = Command::new("git")
            .args(["ls-tree", "HEAD"])
            .current_dir(path)
            .output()
            .unwrap();
        let tree = String::from_utf8_lossy(&tree.stdout);
        assert!(
            tree.lines()
                .any(|l| l.starts_with("100755") && l.ends_with("a.sh"))
        );
        assert!(
            tree.lines()
                .any(|l| l.starts_with("100755") && l.ends_with("new.sh"))
        );
        assert!(
            tree.lines()
                .any(|l| l.starts_with("100644") && l.ends_with("b.txt"))
        );
        // trusted modes: nothing to keep
        run(&["config", "core.fileMode", "true"]);
        assert!(staged_mode_changes(git, path).unwrap().is_empty());
    }

    #[test]
    fn large_file_paths_are_the_files_over_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("small.txt"), "1234").unwrap();
        std::fs::write(dir.path().join("big.bin"), "123456").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let paths = ["small.txt", "big.bin", "gone.bin", "sub/"];
        assert_eq!(large_file_paths(dir.path(), &paths, 5), vec!["big.bin"]);
        assert!(large_file_paths(dir.path(), &paths, RECEIVE_LIMIT).is_empty());
    }

    #[test]
    fn parses_commit_authors() {
        assert_eq!(
            parse_commit_author("  Ada Lovelace <ada@example.com> "),
            Ok(("Ada Lovelace".to_string(), "ada@example.com".to_string()))
        );
        assert_eq!(
            parse_commit_author("Ada<ada@x>"),
            Ok(("Ada".to_string(), "ada@x".to_string()))
        );
        for bad in [
            "",
            "Ada",
            "<ada@x>",
            "Ada <>",
            "Ada <a b@x>",
            "Ada <ada@x",
            "A<b> <c@d>",
        ] {
            assert!(parse_commit_author(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn amends_with_another_author_or_the_committer() {
        let (dir, git) = repo();
        let path = dir.path();
        std::fs::write(path.join("a.txt"), "a\n").unwrap();
        let status = crate::get_status(git.clone(), path).unwrap();
        stage_files(git.clone(), path, &status.files).unwrap();
        commit(git.clone(), path, "init\n", &CommitOptions::default()).unwrap();
        let author = || {
            let out = Command::new("git")
                .args(["log", "-1", "--format=%an <%ae>"])
                .current_dir(path)
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        let amend = |author: CommitAuthor| CommitOptions {
            amend: true,
            author: Some(author),
            ..CommitOptions::default()
        };
        commit(
            git.clone(),
            path,
            "init\n",
            &amend(CommitAuthor::Given {
                name: "Ada Lovelace".into(),
                email: "ada@example.com".into(),
            }),
        )
        .unwrap();
        assert_eq!(author(), "Ada Lovelace <ada@example.com>");
        commit(git, path, "init\n", &amend(CommitAuthor::ResetToCommitter)).unwrap();
        assert_eq!(author(), "T <t@example.com>");
    }

    #[test]
    fn parses_mode_changes() {
        let raw = ":100644 100755 aaa aaa M\0x.sh\0:100644 100644 aaa bbb M\0y.txt\0\
                   :000000 100644 000 ccc A\0z.txt\0:120000 100644 ddd ddd T\0l\0";
        assert_eq!(
            parse_mode_changes(raw),
            vec![
                ("x.sh".to_string(), true),
                ("z.txt".to_string(), false),
                ("l".to_string(), false)
            ]
        );
    }

    #[test]
    fn parses_the_commit_sha_like_ghd() {
        assert_eq!(
            parse_commit_sha("[main 1a2b3c4] Add a\n 1 file changed\n"),
            "1a2b3c4"
        );
        assert_eq!(
            parse_commit_sha("[main (root-commit) 1a2b3c4] Add a\n"),
            "(root-commit)"
        );
        assert_eq!(parse_commit_sha(""), "");
    }

    #[test]
    fn assume_unchanged_round_trip() {
        let (dir, git) = repo();
        let path = dir.path();
        std::fs::write(path.join("a.txt"), "one\n").unwrap();
        std::fs::write(path.join("b c.txt"), "two\n").unwrap();
        let status = crate::get_status(git.clone(), path).unwrap();
        stage_files(git.clone(), path, &status.files).unwrap();
        commit(git.clone(), path, "init\n", &CommitOptions::default()).unwrap();
        std::fs::write(path.join("a.txt"), "dirty\n").unwrap();
        std::fs::write(path.join("b c.txt"), "dirty\n").unwrap();
        let both = vec!["a.txt".to_string(), "b c.txt".to_string()];
        set_assume_unchanged(git.clone(), path, &both, true).unwrap();
        assert_eq!(assume_unchanged_paths(git.clone(), path).unwrap(), both);
        let status = crate::get_status(git.clone(), path).unwrap();
        assert!(status.files.is_empty());
        set_assume_unchanged(git.clone(), path, &both, false).unwrap();
        assert!(
            assume_unchanged_paths(git.clone(), path)
                .unwrap()
                .is_empty()
        );
        let status = crate::get_status(git, path).unwrap();
        assert_eq!(status.files.len(), 2);
    }

    #[test]
    fn discard_leftovers_refreshes_and_names_what_differs() {
        let (dir, git) = repo();
        let path = dir.path();
        std::fs::write(path.join("touched"), "t\n").unwrap();
        std::fs::write(path.join("edited"), "e\n").unwrap();
        std::fs::write(path.join("moded"), "m\n").unwrap();
        let run_git = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(path)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        };
        run_git(&["add", "."]);
        run_git(&["commit", "-qm", "files"]);
        // same contents, new file times
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(path.join("touched"), "t\n").unwrap();
        std::fs::write(path.join("edited"), "changed\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path.join("moded"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        let leftovers = discard_leftovers(git, path, &["touched", "edited", "moded"]).unwrap();
        assert_eq!(leftovers.changed, vec!["edited".to_string()]);
        #[cfg(unix)]
        assert_eq!(leftovers.mode_only, vec!["moded".to_string()]);
    }

    #[test]
    fn discard_restores_and_removes() {
        let (dir, git) = repo();
        let path = dir.path();
        std::fs::write(path.join("a.txt"), "one\n").unwrap();
        let status = crate::get_status(git.clone(), path).unwrap();
        stage_files(git.clone(), path, &status.files).unwrap();
        commit(git.clone(), path, "init\n", &CommitOptions::default()).unwrap();
        std::fs::write(path.join("a.txt"), "dirty\n").unwrap();
        std::fs::write(path.join("new.txt"), "x\n").unwrap();
        let status = crate::get_status(git.clone(), path).unwrap();
        discard_changes(git.clone(), path, &status.files, false, false, false).unwrap();
        assert_eq!(
            std::fs::read_to_string(path.join("a.txt")).unwrap(),
            "one\n"
        );
        assert!(!path.join("new.txt").exists());
        let status = crate::get_status(git, path).unwrap();
        assert!(status.files.is_empty());
    }

    #[test]
    #[cfg(not(target_os = "android"))]
    fn trash_counts_missing_paths_as_gone() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.txt");
        assert_eq!(trash_paths(&[missing.clone(), missing]), vec![true, true]);
    }

    #[test]
    fn discard_cleans_inside_a_submodule() {
        let (sub_dir, git) = repo();
        let git_in = |dir: &Path, args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(["-c", "protocol.file.allow=always"])
                    .args(args)
                    .current_dir(dir)
                    .status()
                    .unwrap()
                    .success()
            )
        };
        std::fs::write(sub_dir.path().join("lib.txt"), "lib\n").unwrap();
        git_in(sub_dir.path(), &["add", "."]);
        git_in(sub_dir.path(), &["commit", "-q", "-m", "lib"]);
        let (dir, _) = repo();
        let path = dir.path();
        let url = sub_dir.path().to_string_lossy().into_owned();
        git_in(path, &["submodule", "add", "-q", &url, "sub"]);
        git_in(path, &["commit", "-q", "-m", "add sub"]);
        let sub = path.join("sub");
        std::fs::write(sub.join("lib.txt"), "dirty\n").unwrap();
        std::fs::write(sub.join("junk.txt"), "x\n").unwrap();
        std::fs::create_dir(sub.join("junkdir")).unwrap();
        std::fs::write(sub.join("junkdir/more.txt"), "y\n").unwrap();
        let status = crate::get_status(git.clone(), path).unwrap();
        assert_eq!(status.files.len(), 1);
        let hidden = crate::get_status_with(
            git.clone(),
            path,
            crate::StatusOptions {
                ignore_submodules: crate::IgnoreSubmodules::Dirty,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(hidden.files.is_empty());
        // GHD behaviour: `submodule update --force` checks the recorded
        // commit out again, the untracked files stay
        discard_changes(git.clone(), path, &status.files, false, false, false).unwrap();
        assert!(sub.join("junk.txt").exists());
        assert_eq!(
            std::fs::read_to_string(sub.join("lib.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "lib\n"
        );
        std::fs::write(sub.join("lib.txt"), "dirty\n").unwrap();
        discard_changes(git.clone(), path, &status.files, false, true, false).unwrap();
        // the submodule's clone has the machine's `core.autocrlf`
        assert_eq!(
            std::fs::read_to_string(sub.join("lib.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "lib\n"
        );
        assert!(!sub.join("junk.txt").exists());
        assert!(!sub.join("junkdir").exists());
        let status = crate::get_status(git, path).unwrap();
        assert!(status.files.is_empty());
    }

    #[test]
    fn fixup_commits_fold_into_their_target() {
        let (dir, git) = repo();
        let path = dir.path();
        let log = |path: &Path| {
            let out = Command::new("git")
                .args(["log", "--format=%s", "--name-only"])
                .current_dir(path)
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).to_string()
        };
        let commit_file = |name: &str, contents: &str, opts: &CommitOptions| {
            std::fs::write(path.join(name), contents).unwrap();
            add_paths(git.clone(), path, &[name]).unwrap();
            commit(git.clone(), path, &format!("Add {name}\n"), opts).unwrap();
        };
        commit_file("a.txt", "a\n", &CommitOptions::default());
        commit_file("b.txt", "b\n", &CommitOptions::default());
        commit_file("c.txt", "c\n", &CommitOptions::default());
        let shas = Command::new("git")
            .args(["rev-list", "HEAD"])
            .current_dir(path)
            .output()
            .unwrap();
        let shas: Vec<String> = String::from_utf8_lossy(&shas.stdout)
            .lines()
            .map(str::to_string)
            .collect();
        // a fixup of "Add b.txt" (the middle commit)
        commit_file(
            "b2.txt",
            "b2\n",
            &CommitOptions {
                fixup: Some(shas[1].clone()),
                ..CommitOptions::default()
            },
        );
        assert!(log(path).starts_with("fixup! Add b.txt\n"));
        let (result, folded) = crate::autosquash(
            git.clone(),
            path,
            Some(&format!("{}^", shas[1])),
            crate::RebaseOptions::default(),
            |_| {},
        );
        assert_eq!(result, crate::RebaseResult::CompletedWithoutError);
        assert_eq!(folded, 1);
        assert_eq!(
            log(path),
            "Add c.txt\n\nc.txt\nAdd b.txt\n\nb.txt\nb2.txt\nAdd a.txt\n\na.txt\n"
        );
    }
}
