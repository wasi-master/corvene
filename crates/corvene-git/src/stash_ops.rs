//! Corvene's stash steps beyond GHD's `lib/git/stash.ts` (ported in
//! [`crate::branch_ops`]).
//!
//! Deviation (`774-stash-conflict-flow`): a restore whose changes conflict
//! keeps the stash entry git kept ([`StashPopOptions::keep_on_conflict`]);
//! GHD's `popStashEntry` takes `git stash pop`'s exit code 1 with nothing on
//! stderr for success and drops the entry. The conflicted files are marked
//! resolved with [`mark_conflicts_resolved`] (the index only).
//!
//! Deviation (`775-stash-restore-unstages-new-files`): Desktop stashes add
//! untracked files to the index first (`createDesktopStashEntry`), so a pop
//! brings them back staged as new files, and a branch whose `.gitignore`
//! ignores them still lists them; with
//! [`StashPopOptions::unstage_new_files`] a restore unstages the files the
//! stash added ([`stash_new_files`]) and they are untracked again.
//!
//! Deviation (`776-stash-add-to-existing`): [`add_to_desktop_stash`] folds
//! the current changes into the branch's stash instead of replacing it (GHD
//! can only overwrite, `createStashAndDropPreviousEntry`).
//!
//! Deviation (`777-stash-selected-files`): [`create_desktop_stash_of_files`]
//! stashes some of the changed files (GHD stashes all of them).
//!
//! Deviation (`797-stash-list`): every stash can be applied without dropping
//! it ([`apply_stash_entry_with`]), turned into a branch
//! ([`create_branch_from_stash`]) and put back after a discard
//! ([`store_stash`]); [`create_stash_with_message`] makes a stash with the
//! user's message. GHD only pops or drops the branch's Desktop stash.

use std::path::Path;
use std::sync::Arc;

use corvene_models::{FileStatusKind, StashEntry, WorkingDirectoryFileChange};

use crate::branch_ops::{drop_desktop_stash_entry, stash_entry_matching_sha};
use crate::detect::GitBinary;
use crate::error::{GitError, Result};
use crate::git_errors::{KnownGitError, known_git_error};
use crate::process::GitCommand;

/// How [`pop_stash_entry_with`] goes beyond GHD's `popStashEntry`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StashPopOptions {
    /// `774-stash-conflict-flow`: a pop that leaves conflicted files keeps
    /// the entry (GHD drops it).
    pub keep_on_conflict: bool,
    /// `775-stash-restore-unstages-new-files`: after a restore, the files
    /// the stash added are unstaged (GHD leaves them staged).
    pub unstage_new_files: bool,
    /// `1303-partial-stash`: when git refuses because files the stash
    /// changes have local changes (a line stash next to the lines left
    /// behind), the stash is merged into them instead
    /// ([`restore_over_local_changes`]). GHD reports git's refusal.
    pub merge_over_local_changes: bool,
}

/// What [`pop_stash_entry_with`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StashPop {
    /// No stash entry has that commit (any more).
    Missing,
    /// The changes are back and the entry is gone.
    Restored,
    /// The changes are back with conflicts and the entry is still there
    /// (always with [`StashPopOptions::keep_on_conflict`]; without it only
    /// when git named the conflicts, as GHD's `MergeConflicts`).
    Conflicted,
}

/// `popStashEntry` with [`StashPopOptions`]: `git stash pop --quiet <name>`
/// of the entry whose commit is `stash_sha`. Without options it is GHD's
/// exactly: exit code 1 with nothing on stderr counts as applied and the
/// entry is dropped.
pub fn pop_stash_entry_with(
    git: Arc<GitBinary>,
    workdir: &Path,
    stash_sha: &str,
    options: StashPopOptions,
) -> Result<StashPop> {
    restore_stash_entry(git, workdir, stash_sha, options, true)
}

/// Corvene (`797-stash-list`): `git stash apply --quiet <name>` of the entry
/// whose commit is `stash_sha`, which stays in the list. [`StashPop::Restored`]
/// then means the changes are back (and the entry is still there).
pub fn apply_stash_entry_with(
    git: Arc<GitBinary>,
    workdir: &Path,
    stash_sha: &str,
    options: StashPopOptions,
) -> Result<StashPop> {
    restore_stash_entry(git, workdir, stash_sha, options, false)
}

/// [`pop_stash_entry_with`] (`pop`) or [`apply_stash_entry_with`].
fn restore_stash_entry(
    git: Arc<GitBinary>,
    workdir: &Path,
    stash_sha: &str,
    options: StashPopOptions,
    pop: bool,
) -> Result<StashPop> {
    let verb = if pop { "pop" } else { "apply" };
    let Some(entry) = stash_entry_matching_sha(git.clone(), workdir, stash_sha)? else {
        return Ok(StashPop::Missing);
    };
    let new_files = if options.unstage_new_files {
        stash_new_files(git.clone(), workdir, &entry).unwrap_or_default()
    } else {
        Vec::new()
    };
    // conflicts already there are not this pop's (git refuses to apply)
    let unmerged_before =
        options.keep_on_conflict && !unmerged_paths(git.clone(), workdir)?.is_empty();
    let out = GitCommand::new(git.clone())
        .args(["stash", verb, "--quiet", &entry.name])
        .current_dir(workdir)
        .allow_any_exit_code()
        .run()?;
    if out.status.success() {
        unstage_new_files(git, workdir, &new_files);
        return Ok(StashPop::Restored);
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let known = known_git_error(&out.stderr).or_else(|| known_git_error(&stdout));
    if known == Some(KnownGitError::MergeConflicts) {
        return Ok(StashPop::Conflicted);
    }
    if known == Some(KnownGitError::MergeWithLocalChanges)
        && options.merge_over_local_changes
        && restore_over_local_changes(git.clone(), workdir, &entry)?
    {
        if pop {
            drop_desktop_stash_entry(git, workdir, stash_sha)?;
        }
        return Ok(StashPop::Restored);
    }
    if out.status.code() == Some(1)
        && options.keep_on_conflict
        && !unmerged_before
        && !unmerged_paths(git.clone(), workdir)?.is_empty()
    {
        return Ok(StashPop::Conflicted);
    }
    if out.status.code() == Some(1) && out.stderr.is_empty() {
        if pop {
            drop_desktop_stash_entry(git.clone(), workdir, stash_sha)?;
        }
        unstage_new_files(git, workdir, &new_files);
        return Ok(StashPop::Restored);
    }
    Err(GitError::Failed {
        args: format!("stash {verb} --quiet {}", entry.name),
        code: out.status.code(),
        stderr: out.stderr.trim().to_string(),
    })
}

/// `1303-partial-stash`: put stash `entry` back over local changes to the
/// same files, which `git stash apply` refuses to touch. The working
/// directory is snapshotted (untracked files included, nothing changes),
/// `merge-tree` merges the stash into the snapshot over the stash's base,
/// and the difference between the snapshot and the merge is applied to the
/// working copy (the index stays as it is). False when there are no local
/// changes or the stash keeps untracked files in a third commit; a merge
/// that conflicts is an error naming the files, and nothing is changed.
pub fn restore_over_local_changes(
    git: Arc<GitBinary>,
    workdir: &Path,
    entry: &StashEntry,
) -> Result<bool> {
    let [base, _] = entry.parents.as_slice() else {
        return Ok(false);
    };
    let Some(snapshot) = snapshot_working_directory(git.clone(), workdir)? else {
        return Ok(false);
    };
    let tree = match merge_stash_trees(git.clone(), workdir, base, &snapshot, &entry.sha)? {
        Ok(tree) => tree,
        Err(files) => {
            return Err(GitError::Gix(format!(
                "The stash and your changes both change the same lines in {}, so the stash was \
                 not restored. Nothing was changed.",
                if files.is_empty() {
                    "some files".to_string()
                } else {
                    files.join(", ")
                }
            )));
        }
    };
    let patch = GitCommand::new(git.clone())
        .args([
            "diff",
            "--binary",
            "--full-index",
            "--no-ext-diff",
            "--no-color",
            &snapshot,
            &tree,
        ])
        .current_dir(workdir)
        .run()?
        .stdout;
    if !patch.is_empty() {
        GitCommand::new(git)
            .args(["apply", "--binary", "--whitespace=nowarn", "-"])
            .current_dir(workdir)
            .stdin(patch)
            .run()?;
    }
    Ok(true)
}

/// The files stash `entry` added to the index: those its index commit
/// (second parent) has and its base (first parent) does not. For a Desktop
/// stash that is every file that was untracked, plus new files that were
/// staged.
pub fn stash_new_files(
    git: Arc<GitBinary>,
    workdir: &Path,
    entry: &StashEntry,
) -> Result<Vec<String>> {
    let [base, index, ..] = entry.parents.as_slice() else {
        return Ok(Vec::new());
    };
    let out = GitCommand::new(git)
        .args([
            "diff",
            "--name-only",
            "-z",
            "--no-renames",
            "--diff-filter=A",
            base,
            index,
        ])
        .current_dir(workdir)
        .run()?;
    Ok(split_nul(&out.stdout))
}

/// After a restore: unstage those of `added` that are staged as new files
/// now (index only). A failure leaves them staged; the restore stands.
fn unstage_new_files(git: Arc<GitBinary>, workdir: &Path, added: &[String]) {
    if added.is_empty() {
        return;
    }
    let staged = GitCommand::new(git.clone())
        .args([
            "diff",
            "--cached",
            "--name-only",
            "-z",
            "--no-renames",
            "--diff-filter=A",
        ])
        .current_dir(workdir)
        .run()
        .map(|out| split_nul(&out.stdout));
    let result = staged.and_then(|staged| {
        let paths: Vec<String> = added
            .iter()
            .filter(|p| staged.contains(p))
            .cloned()
            .collect();
        mark_conflicts_resolved(git, workdir, &paths)
    });
    if let Err(err) = result {
        tracing::warn!(%err, "could not unstage the files the stash added");
    }
}

fn split_nul(stdout: &[u8]) -> Vec<String> {
    stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect()
}

/// `777-stash-selected-files`: `createDesktopStashEntry` for `files` only:
/// their untracked ones are added to the index first, a staged rename
/// brings its old path back to the index (`git stash push` refuses a path
/// it no longer knows; the rename comes back as a deletion plus a new
/// file), then `git stash push -m !!GitHub_Desktop<branch> -- <paths>`.
/// Returns false when there was nothing to stash.
pub fn create_desktop_stash_of_files(
    git: Arc<GitBinary>,
    workdir: &Path,
    branch: &str,
    files: &[WorkingDirectoryFileChange],
    guard_assume_unchanged: bool,
) -> Result<bool> {
    if files.is_empty() {
        return Ok(false);
    }
    let mut paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
    if guard_assume_unchanged {
        let hidden: Vec<String> = crate::modified_assume_unchanged(git.clone(), workdir)?
            .into_iter()
            .filter(|p| paths.contains(p))
            .collect();
        if !hidden.is_empty() {
            return Err(GitError::Gix(format!(
                "Your changes were not stashed: {} {} marked assume-unchanged, and a stash would \
                 discard {} local changes without saving them. Run git update-index \
                 --no-assume-unchanged first.",
                hidden.join(", "),
                if hidden.len() == 1 { "is" } else { "are" },
                if hidden.len() == 1 { "its" } else { "their" },
            )));
        }
    }
    let untracked: Vec<String> = files
        .iter()
        .filter(|f| f.status.kind == FileStatusKind::Untracked)
        .map(|f| f.path.clone())
        .collect();
    if !untracked.is_empty() {
        GitCommand::new(git.clone())
            .args(["update-index", "--add", "-z", "--stdin"])
            .current_dir(workdir)
            .stdin(nul_separated(&untracked))
            .run()?;
    }
    let renamed_from: Vec<String> = files
        .iter()
        .filter(|f| f.status.kind == FileStatusKind::Renamed)
        .filter_map(|f| f.old_path.clone())
        .collect();
    if !renamed_from.is_empty() {
        mark_conflicts_resolved(git.clone(), workdir, &renamed_from)?;
        paths.extend(renamed_from);
    }
    let out = GitCommand::new(git)
        .args([
            "stash",
            "push",
            "-m",
            &crate::desktop_stash_message(branch),
            "--pathspec-from-file=-",
            "--pathspec-file-nul",
        ])
        .env("GIT_LITERAL_PATHSPECS", "1")
        .current_dir(workdir)
        .stdin(nul_separated(&paths))
        .allow_exit_code(1)
        .run()?;
    if out.stdout_string()?.trim() == "No local changes to save" {
        return Ok(false);
    }
    if !out.status.success() {
        return Err(GitError::Failed {
            args: "stash push".into(),
            code: out.status.code(),
            stderr: out.stderr,
        });
    }
    Ok(true)
}

/// What [`add_to_desktop_stash`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddToStash {
    /// One stash now holds the old entry's changes and the current ones.
    Added,
    /// There was nothing to add; the old entry is untouched.
    NothingToAdd,
    /// The old entry and the current changes both change these files;
    /// nothing was touched.
    Conflicts(Vec<String>),
}

/// The oldest git whose `merge-tree` takes `--merge-base`, which
/// [`add_to_desktop_stash`] needs (`776-stash-add-to-existing`).
pub const ADD_TO_STASH_MIN_VERSION: (u32, u32) = (2, 40);

/// `776-stash-add-to-existing`: fold the working directory's changes into
/// the Desktop stash entry `old_sha` as one new entry made on `branch`.
///
/// 1. Snapshot without touching anything: a copy of the index (in the git
///    dir) gets the untracked files the Desktop way and `git stash create`
///    records the working directory from it, then `merge-tree` applies the
///    old entry onto that snapshot (base: the entry's own base commit, as
///    `git stash apply` does). Any conflict or error stops here, with the
///    index, the working directory and the stash list as they were.
/// 2. [`crate::create_desktop_stash`] stashes the changes (the worktree is
///    clean), the merge is redone with that entry, and the merged tree is
///    stored as a new entry (`commit-tree` + `stash store`) before the two
///    others are dropped. A failure before the new entry is stored pops the
///    just made entry back (onto the clean worktree it applies as is).
pub fn add_to_desktop_stash(
    git: Arc<GitBinary>,
    workdir: &Path,
    old_sha: &str,
    branch: &str,
    guard_assume_unchanged: bool,
) -> Result<AddToStash> {
    let Some(old) = stash_entry_matching_sha(git.clone(), workdir, old_sha)? else {
        return Err(GitError::Gix(
            "The stash is no longer there; it may have been restored or discarded already.".into(),
        ));
    };
    // `stash -u` entries keep untracked files in a third parent the merge
    // below would miss; Desktop entries have two
    let [old_base, _] = old.parents.as_slice() else {
        return Err(GitError::Gix(
            "This stash was not made by Corvene or GitHub Desktop, so changes cannot be added \
             to it."
                .into(),
        ));
    };
    if guard_assume_unchanged {
        crate::ensure_no_modified_assume_unchanged(git.clone(), workdir)?;
    }
    let Some(snapshot) = snapshot_working_directory(git.clone(), workdir)? else {
        return Ok(AddToStash::NothingToAdd);
    };
    if let Err(files) = merge_stash_trees(git.clone(), workdir, old_base, &snapshot, &old.sha)? {
        return Ok(AddToStash::Conflicts(files));
    }
    // nothing has been touched so far; from here the changes are stashed
    if !crate::create_desktop_stash(git.clone(), workdir, branch, false)? {
        return Ok(AddToStash::NothingToAdd);
    }
    let Some(made) = crate::get_last_desktop_stash_entry_for_branch(git.clone(), workdir, branch)?
    else {
        return Err(GitError::Gix(
            "The new stash entry could not be found.".into(),
        ));
    };
    let stored = (|| -> Result<std::result::Result<(), Vec<String>>> {
        let tree = match merge_stash_trees(git.clone(), workdir, old_base, &made.sha, &old.sha)? {
            Ok(tree) => tree,
            Err(files) => return Ok(Err(files)),
        };
        let head = made
            .parents
            .first()
            .cloned()
            .ok_or_else(|| GitError::Gix("The new stash entry has no base commit.".into()))?;
        let message = crate::desktop_stash_message(branch);
        let commit_tree = |message: &str, parents: &[&str]| -> Result<String> {
            let mut args = vec!["commit-tree", tree.as_str(), "-m", message];
            for parent in parents {
                args.extend(["-p", parent]);
            }
            Ok(GitCommand::new(git.clone())
                .args(args)
                .current_dir(workdir)
                .run()?
                .stdout_string()?
                .trim()
                .to_string())
        };
        let index = commit_tree(&format!("index on {branch}"), &[&head])?;
        let stash = commit_tree(&format!("On {branch}: {message}"), &[&head, &index])?;
        GitCommand::new(git.clone())
            .args([
                "stash",
                "store",
                "-m",
                &format!("On {branch}: {message}"),
                &stash,
            ])
            .current_dir(workdir)
            .run()?;
        Ok(Ok(()))
    })();
    match stored {
        Ok(Ok(())) => {
            drop_desktop_stash_entry(git.clone(), workdir, &made.sha)?;
            drop_desktop_stash_entry(git, workdir, &old.sha)?;
            Ok(AddToStash::Added)
        }
        // put the changes back as they were stashed a moment ago
        Ok(Err(files)) => {
            restore_made_entry(git, workdir, &made.sha)?;
            Ok(AddToStash::Conflicts(files))
        }
        Err(err) => {
            restore_made_entry(git, workdir, &made.sha)?;
            Err(err)
        }
    }
}

fn restore_made_entry(git: Arc<GitBinary>, workdir: &Path, sha: &str) -> Result<()> {
    let options = StashPopOptions {
        keep_on_conflict: true,
        unstage_new_files: true,
        merge_over_local_changes: false,
    };
    match pop_stash_entry_with(git, workdir, sha, options)? {
        StashPop::Restored | StashPop::Missing => Ok(()),
        StashPop::Conflicted => Err(GitError::Gix(
            "Your changes could not be put back cleanly; they are kept in the stash list.".into(),
        )),
    }
}

/// `git stash create` of the whole working directory, untracked files
/// included the Desktop way, through a copy of the index so that neither
/// the index nor the working directory changes. `None`: nothing to stash.
fn snapshot_working_directory(git: Arc<GitBinary>, workdir: &Path) -> Result<Option<String>> {
    /// Removes the copy whatever happens.
    struct TempIndex(std::path::PathBuf);
    impl Drop for TempIndex {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let git_dir = crate::paths::git_dir(workdir);
    let index =
        TempIndex(git_dir.join(format!("corvene-add-to-stash-{}.index", std::process::id())));
    std::fs::copy(git_dir.join("index"), &index.0)?;
    let untracked = GitCommand::new(git.clone())
        .args(["ls-files", "--others", "--exclude-standard", "-z"])
        .current_dir(workdir)
        .run()?;
    if untracked.stdout.iter().any(|b| *b != 0) {
        GitCommand::new(git.clone())
            .args(["update-index", "--add", "-z", "--stdin"])
            .env("GIT_INDEX_FILE", &index.0)
            .current_dir(workdir)
            .stdin(untracked.stdout)
            .run()?;
    }
    let sha = GitCommand::new(git)
        .args(["stash", "create"])
        .env("GIT_INDEX_FILE", &index.0)
        .current_dir(workdir)
        .run()?
        .stdout_string()?;
    let sha = sha.trim();
    Ok((!sha.is_empty()).then(|| sha.to_string()))
}

/// `merge-tree` of stash commits `ours` and `theirs` over `base`: the
/// merged tree, or the conflicted files.
fn merge_stash_trees(
    git: Arc<GitBinary>,
    workdir: &Path,
    base: &str,
    ours: &str,
    theirs: &str,
) -> Result<std::result::Result<String, Vec<String>>> {
    let out = GitCommand::new(git)
        .args([
            "merge-tree",
            "--write-tree",
            "--name-only",
            "--no-messages",
            "-z",
            &format!("--merge-base={base}"),
            ours,
            theirs,
        ])
        .current_dir(workdir)
        .allow_exit_code(1)
        .run()?;
    let tree = out
        .stdout
        .split(|b| *b == 0)
        .next()
        .map(|t| String::from_utf8_lossy(t).trim().to_string())
        .unwrap_or_default();
    if out.status.success() && !tree.is_empty() {
        Ok(Ok(tree))
    } else {
        Ok(Err(crate::rebase_ops::parse_merge_tree_names(&out.stdout)))
    }
}

/// The paths with unmerged index entries (`ls-files -u`), each once.
/// Corvene (`797-stash-list`): `git stash push [-u] [-m <message>]`, a stash
/// that is not the branch's Desktop stash (no `!!GitHub_Desktop` marker), so
/// branch switches leave it alone. Returns false when there was nothing to
/// stash. `guard_assume_unchanged` is flag `869`'s check.
pub fn create_stash_with_message(
    git: Arc<GitBinary>,
    workdir: &Path,
    message: Option<&str>,
    include_untracked: bool,
    guard_assume_unchanged: bool,
) -> Result<bool> {
    if guard_assume_unchanged {
        crate::branch_ops::ensure_no_modified_assume_unchanged(git.clone(), workdir)?;
    }
    let mut args = vec!["stash", "push"];
    if include_untracked {
        args.push("--include-untracked");
    }
    let message = message.map(str::trim).filter(|m| !m.is_empty());
    if let Some(message) = message {
        args.extend(["-m", message]);
    }
    let out = GitCommand::new(git)
        .args(args)
        .current_dir(workdir)
        .allow_exit_code(1)
        .run()?;
    if out.stdout_string()?.trim() == "No local changes to save" {
        return Ok(false);
    }
    if !out.status.success() {
        return Err(GitError::Failed {
            args: "stash push".into(),
            code: out.status.code(),
            stderr: out.stderr.trim().to_string(),
        });
    }
    Ok(true)
}

/// Corvene (`797-stash-list`): `git stash branch <branch> <name>` of the
/// entry whose commit is `stash_sha`: a new branch at the commit the stash
/// was made on, checked out, with the stash applied and (when that worked)
/// dropped. [`StashPop::Missing`] when the entry is gone.
pub fn create_branch_from_stash(
    git: Arc<GitBinary>,
    workdir: &Path,
    stash_sha: &str,
    branch: &str,
) -> Result<StashPop> {
    let Some(entry) = stash_entry_matching_sha(git.clone(), workdir, stash_sha)? else {
        return Ok(StashPop::Missing);
    };
    let out = GitCommand::new(git.clone())
        .args(["stash", "branch", branch, &entry.name])
        .current_dir(workdir)
        .allow_any_exit_code()
        .run()?;
    if out.status.success() {
        return Ok(StashPop::Restored);
    }
    // the branch is made and checked out before the apply; a conflicted
    // apply keeps the entry
    if !unmerged_paths(git, workdir)?.is_empty() {
        return Ok(StashPop::Conflicted);
    }
    Err(GitError::Failed {
        args: format!("stash branch {branch} {}", entry.name),
        code: out.status.code(),
        stderr: out.stderr.trim().to_string(),
    })
}

/// Corvene (`797-stash-list`): put a dropped entry back (`git stash store
/// -m <message> <sha>`), the "Discarded stash" banner's Undo. The stash
/// commit stays in the object database until git's gc prunes it.
pub fn store_stash(git: Arc<GitBinary>, workdir: &Path, sha: &str, message: &str) -> Result<()> {
    GitCommand::new(git)
        .args(["stash", "store", "-m", message, sha])
        .current_dir(workdir)
        .run()?;
    Ok(())
}

pub fn unmerged_paths(git: Arc<GitBinary>, workdir: &Path) -> Result<Vec<String>> {
    let out = GitCommand::new(git)
        .args(["ls-files", "-u", "-z"])
        .current_dir(workdir)
        .run()?;
    Ok(parse_unmerged_paths(&out.stdout))
}

/// `ls-files -u -z` records: `<mode> <sha> <stage>\t<path>` NUL.
pub fn parse_unmerged_paths(stdout: &[u8]) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for entry in parse_unmerged_entries(stdout) {
        if paths.last() != Some(&entry.path) {
            paths.push(entry.path);
        }
    }
    paths
}

/// `ls-files -u -z` records with their stages (Corvene `1361-merge-editor`).
pub fn parse_unmerged_entries(stdout: &[u8]) -> Vec<corvene_models::UnmergedEntry> {
    let mut entries = Vec::new();
    for record in stdout.split(|b| *b == 0) {
        let record = String::from_utf8_lossy(record);
        let Some((head, path)) = record.split_once('\t') else {
            continue;
        };
        let mut fields = head.split(' ');
        let (Some(mode), Some(oid), Some(stage)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let Ok(stage) = stage.parse::<u8>() else {
            continue;
        };
        entries.push(corvene_models::UnmergedEntry {
            stage,
            mode: mode.to_string(),
            oid: oid.to_string(),
            path: path.to_string(),
        });
    }
    entries
}

/// `774-stash-conflict-flow` › Mark as Resolved: `git reset -q -- <paths>`.
/// The unmerged index entries go back to `HEAD`'s; the working files, as
/// resolved, stay as they are (as changes of their own).
pub fn mark_conflicts_resolved(
    git: Arc<GitBinary>,
    workdir: &Path,
    paths: &[String],
) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    GitCommand::new(git)
        .args([
            "reset",
            "-q",
            "--pathspec-from-file=-",
            "--pathspec-file-nul",
        ])
        .env("GIT_LITERAL_PATHSPECS", "1")
        .current_dir(workdir)
        .stdin(nul_separated(paths))
        .run()?;
    Ok(())
}

/// The entry of `refs/stash` whose commit is `sha`, if it is still there.
pub fn stash_entry(git: Arc<GitBinary>, workdir: &Path, sha: &str) -> Result<Option<StashEntry>> {
    stash_entry_matching_sha(git, workdir, sha)
}

fn nul_separated(paths: &[String]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    for path in paths {
        out.extend_from_slice(path.as_bytes());
        out.push(0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn repo() -> (tempfile::TempDir, Arc<GitBinary>) {
        let dir = tempfile::tempdir().unwrap();
        for args in [
            &["init", "-q", "-b", "main"][..],
            // Git for Windows checks text out with CRLF unless told not to
            &["config", "core.autocrlf", "false"],
            &["config", "commit.gpgsign", "false"],
            &["config", "user.name", "T"],
            &["config", "user.email", "t@example.com"],
        ] {
            run(dir.path(), args);
        }
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        run(dir.path(), &["add", "."]);
        run(dir.path(), &["commit", "-q", "-m", "first"]);
        (dir, Arc::new(crate::find_git().unwrap()))
    }

    fn run(path: &Path, args: &[&str]) {
        assert!(
            Command::new("git")
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

    /// A stash of `a.txt` = "two" made on "one", then "three" committed.
    fn conflicting_stash() -> (tempfile::TempDir, Arc<GitBinary>, String) {
        let (dir, git) = repo();
        let path = dir.path();
        std::fs::write(path.join("a.txt"), "two\n").unwrap();
        assert!(crate::create_desktop_stash(git.clone(), path, "main", false).unwrap());
        std::fs::write(path.join("a.txt"), "three\n").unwrap();
        run(path, &["commit", "-q", "-am", "three"]);
        let sha = crate::get_stashes(git.clone(), path).unwrap().0[0]
            .sha
            .clone();
        (dir, git, sha)
    }

    #[test]
    fn parses_unmerged_paths_once_each() {
        let out = b"100644 aaa 1\ta.txt\x00100644 bbb 2\ta.txt\x00100644 ccc 3\ta.txt\x00100644 ddd 2\tb c.txt\x00";
        assert_eq!(parse_unmerged_paths(out), ["a.txt", "b c.txt"]);
        assert!(parse_unmerged_paths(b"").is_empty());
    }

    #[test]
    fn conflicting_pop_keeps_the_entry_when_asked() {
        let (dir, git, sha) = conflicting_stash();
        let path = dir.path();
        let options = StashPopOptions {
            keep_on_conflict: true,
            ..Default::default()
        };
        assert_eq!(
            pop_stash_entry_with(git.clone(), path, &sha, options).unwrap(),
            StashPop::Conflicted
        );
        assert_eq!(unmerged_paths(git.clone(), path).unwrap(), ["a.txt"]);
        assert!(stash_entry(git.clone(), path, &sha).unwrap().is_some());
        // resolved in an editor, then marked: index only, the file stays
        std::fs::write(path.join("a.txt"), "two and three\n").unwrap();
        mark_conflicts_resolved(git.clone(), path, &["a.txt".to_string()]).unwrap();
        assert!(unmerged_paths(git.clone(), path).unwrap().is_empty());
        assert_eq!(
            std::fs::read_to_string(path.join("a.txt")).unwrap(),
            "two and three\n"
        );
        assert!(stash_entry(git, path, &sha).unwrap().is_some());
    }

    #[test]
    fn conflicting_pop_drops_the_entry_like_ghd_by_default() {
        let (dir, git, sha) = conflicting_stash();
        let path = dir.path();
        assert_eq!(
            pop_stash_entry_with(git.clone(), path, &sha, StashPopOptions::default()).unwrap(),
            StashPop::Restored
        );
        assert!(stash_entry(git, path, &sha).unwrap().is_none());
    }

    #[test]
    fn restore_can_leave_stashed_new_files_untracked() {
        let (dir, git) = repo();
        let path = dir.path();
        std::fs::write(path.join("a.txt"), "two\n").unwrap();
        std::fs::write(path.join("new.txt"), "n\n").unwrap();
        assert!(crate::create_desktop_stash(git.clone(), path, "main", false).unwrap());
        let entry = crate::get_stashes(git.clone(), path).unwrap().0.remove(0);
        assert_eq!(
            stash_new_files(git.clone(), path, &entry).unwrap(),
            ["new.txt"]
        );
        let options = StashPopOptions {
            unstage_new_files: true,
            ..Default::default()
        };
        assert_eq!(
            pop_stash_entry_with(git.clone(), path, &entry.sha, options).unwrap(),
            StashPop::Restored
        );
        let status = crate::get_status(git.clone(), path).unwrap();
        let new = status.files.iter().find(|f| f.path == "new.txt").unwrap();
        assert_eq!(new.status.kind, corvene_models::FileStatusKind::Untracked);
        let modified = status.files.iter().find(|f| f.path == "a.txt").unwrap();
        assert_eq!(
            modified.status.kind,
            corvene_models::FileStatusKind::Modified
        );
        assert!(stash_entry(git, path, &entry.sha).unwrap().is_none());
    }

    /// `git status --porcelain` of `path`.
    fn porcelain(path: &Path) -> String {
        let out = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(path)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap()
    }

    #[test]
    fn adding_to_a_stash_folds_both_into_one_entry() {
        let (dir, git) = repo();
        let path = dir.path();
        std::fs::write(path.join("b.txt"), "b1\n").unwrap();
        run(path, &["add", "b.txt"]);
        run(path, &["commit", "-q", "-m", "b"]);
        std::fs::write(path.join("a.txt"), "two\n").unwrap();
        std::fs::write(path.join("n.txt"), "n\n").unwrap();
        assert!(crate::create_desktop_stash(git.clone(), path, "main", false).unwrap());
        let old = crate::get_stashes(git.clone(), path).unwrap().0.remove(0);
        std::fs::write(path.join("b.txt"), "b2\n").unwrap();
        std::fs::write(path.join("u.txt"), "u\n").unwrap();
        assert_eq!(
            add_to_desktop_stash(git.clone(), path, &old.sha, "main", false).unwrap(),
            AddToStash::Added
        );
        assert_eq!(porcelain(path), "");
        let (entries, total) = crate::get_stashes(git.clone(), path).unwrap();
        assert_eq!(total, 1);
        assert_eq!(entries[0].branch.as_deref(), Some("main"));
        crate::pop_stash_entry(git, path, &entries[0].sha).unwrap();
        for (file, text) in [
            ("a.txt", "two\n"),
            ("b.txt", "b2\n"),
            ("n.txt", "n\n"),
            ("u.txt", "u\n"),
        ] {
            assert_eq!(
                std::fs::read_to_string(path.join(file)).unwrap(),
                text,
                "{file}"
            );
        }
    }

    #[test]
    fn adding_conflicting_changes_touches_nothing() {
        let (dir, git) = repo();
        let path = dir.path();
        std::fs::write(path.join("a.txt"), "two\n").unwrap();
        assert!(crate::create_desktop_stash(git.clone(), path, "main", false).unwrap());
        let old = crate::get_stashes(git.clone(), path).unwrap().0.remove(0);
        std::fs::write(path.join("a.txt"), "three\n").unwrap();
        std::fs::write(path.join("u.txt"), "u\n").unwrap();
        let before = porcelain(path);
        let index = std::fs::read(path.join(".git/index")).unwrap();
        assert_eq!(
            add_to_desktop_stash(git.clone(), path, &old.sha, "main", false).unwrap(),
            AddToStash::Conflicts(vec!["a.txt".to_string()])
        );
        assert_eq!(std::fs::read(path.join(".git/index")).unwrap(), index);
        assert_eq!(porcelain(path), before);
        assert_eq!(
            std::fs::read_to_string(path.join("a.txt")).unwrap(),
            "three\n"
        );
        let (entries, total) = crate::get_stashes(git, path).unwrap();
        assert_eq!(total, 1);
        assert_eq!(entries[0].sha, old.sha);
        assert!(std::fs::read_dir(path.join(".git")).unwrap().all(|e| {
            !e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("corvene-")
        }));
    }

    #[test]
    fn stashes_only_the_selected_files() {
        let (dir, git) = repo();
        let path = dir.path();
        for (file, text) in [("b.txt", "b1\n"), ("r.txt", "r\n")] {
            std::fs::write(path.join(file), text).unwrap();
        }
        run(path, &["add", "."]);
        run(path, &["commit", "-q", "-m", "more"]);
        std::fs::write(path.join("a.txt"), "two\n").unwrap();
        std::fs::write(path.join("b.txt"), "b2\n").unwrap();
        std::fs::write(path.join("u.txt"), "u\n").unwrap();
        std::fs::write(path.join("v.txt"), "v\n").unwrap();
        run(path, &["mv", "r.txt", "r2.txt"]);
        let status = crate::get_status(git.clone(), path).unwrap();
        let picked: Vec<WorkingDirectoryFileChange> = status
            .files
            .into_iter()
            .filter(|f| ["a.txt", "u.txt", "r2.txt"].contains(&f.path.as_str()))
            .collect();
        assert_eq!(picked.len(), 3);
        assert!(create_desktop_stash_of_files(git.clone(), path, "main", &picked, false).unwrap());
        assert_eq!(porcelain(path), " M b.txt\n?? v.txt\n");
        let entry = crate::get_stashes(git.clone(), path).unwrap().0.remove(0);
        assert_eq!(entry.branch.as_deref(), Some("main"));
        crate::pop_stash_entry(git, path, &entry.sha).unwrap();
        assert_eq!(
            std::fs::read_to_string(path.join("a.txt")).unwrap(),
            "two\n"
        );
        assert!(path.join("u.txt").exists() && path.join("r2.txt").exists());
        assert!(!path.join("r.txt").exists());
    }

    #[test]
    fn missing_entry_is_reported() {
        let (dir, git) = repo();
        assert_eq!(
            pop_stash_entry_with(git, dir.path(), "0000", StashPopOptions::default()).unwrap(),
            StashPop::Missing
        );
    }

    #[test]
    fn applying_a_stash_keeps_the_entry() {
        let (dir, git) = repo();
        let path = dir.path();
        std::fs::write(path.join("a.txt"), "two\n").unwrap();
        run(path, &["stash", "push", "-m", "wip"]);
        let sha = crate::get_stashes(git.clone(), path).unwrap().0[0]
            .sha
            .clone();
        assert_eq!(
            apply_stash_entry_with(git.clone(), path, &sha, StashPopOptions::default()).unwrap(),
            StashPop::Restored
        );
        assert_eq!(
            std::fs::read_to_string(path.join("a.txt")).unwrap(),
            "two\n"
        );
        let (entries, _) = crate::get_stashes(git, path).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].sha, sha);
    }

    #[test]
    fn a_stash_with_a_message_is_not_a_desktop_stash() {
        let (dir, git) = repo();
        let path = dir.path();
        std::fs::write(path.join("a.txt"), "two\n").unwrap();
        std::fs::write(path.join("new.txt"), "new\n").unwrap();
        assert!(create_stash_with_message(git.clone(), path, Some(" wip "), true, false).unwrap());
        assert_eq!(porcelain(path), "");
        let (entries, _) = crate::get_stashes(git.clone(), path).unwrap();
        assert_eq!(entries[0].message, "On main: wip");
        assert_eq!(entries[0].branch, None);
        assert!(entries[0].date > 0);
        // without --include-untracked the new file stays
        std::fs::write(path.join("a.txt"), "three\n").unwrap();
        std::fs::write(path.join("other.txt"), "x\n").unwrap();
        assert!(create_stash_with_message(git.clone(), path, None, false, false).unwrap());
        assert_eq!(porcelain(path), "?? other.txt\n");
        std::fs::remove_file(path.join("other.txt")).unwrap();
        assert!(!create_stash_with_message(git, path, None, false, false).unwrap());
    }

    #[test]
    fn a_branch_from_a_stash_starts_where_the_stash_was_made() {
        let (dir, git) = repo();
        let path = dir.path();
        std::fs::write(path.join("a.txt"), "two\n").unwrap();
        run(path, &["stash", "push"]);
        std::fs::write(path.join("a.txt"), "three\n").unwrap();
        run(path, &["commit", "-q", "-am", "three"]);
        let sha = crate::get_stashes(git.clone(), path).unwrap().0[0]
            .sha
            .clone();
        assert_eq!(
            create_branch_from_stash(git.clone(), path, &sha, "from-stash").unwrap(),
            StashPop::Restored
        );
        assert_eq!(
            std::fs::read_to_string(path.join("a.txt")).unwrap(),
            "two\n"
        );
        assert!(crate::get_stashes(git.clone(), path).unwrap().0.is_empty());
        assert_eq!(
            create_branch_from_stash(git, path, &sha, "again").unwrap(),
            StashPop::Missing
        );
    }

    #[test]
    fn a_discarded_stash_can_be_stored_again() {
        let (dir, git) = repo();
        let path = dir.path();
        std::fs::write(path.join("a.txt"), "two\n").unwrap();
        run(path, &["stash", "push", "-m", "keep me"]);
        let entry = crate::get_stashes(git.clone(), path).unwrap().0.remove(0);
        crate::drop_desktop_stash_entry(git.clone(), path, &entry.sha).unwrap();
        assert!(crate::get_stashes(git.clone(), path).unwrap().0.is_empty());
        store_stash(git.clone(), path, &entry.sha, &entry.message).unwrap();
        let (entries, _) = crate::get_stashes(git, path).unwrap();
        assert_eq!(entries[0].sha, entry.sha);
        assert_eq!(entries[0].message, "On main: keep me");
    }
}
