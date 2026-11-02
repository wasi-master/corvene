//! Multi-commit operations: rebase, interactive rebase (squash / reorder),
//! cherry-pick, and the conflict helpers they share - GHD
//! `lib/git/{rebase,squash,reorder,cherry-pick,merge-tree,diff-check,stage,rev-list}.ts`.
//!
//! Outcomes are classified from the repository state after git exits
//! (`REBASE_HEAD`, `CHERRY_PICK_HEAD`, `MERGE_HEAD`) rather than by matching
//! dugite's stderr regexes, which is more robust across git versions.
//!
//! Deviation: with `keep_messages` (flag `834`) rebases use
//! `commit.cleanup=scissors` so `#` message lines survive a conflict
//! (GHD `lib/git/rebase.ts` keeps git's `strip`); cherry-picks likewise
//! (flag `836`, GHD `lib/git/cherry-pick.ts`). Squash can `--autostash`
//! (flag `829`; GHD refuses to start with local changes). [`binary_paths`]
//! counts a renamed binary file under its new path, where GHD's
//! `binaryListRegex` (`lib/git/diff.ts`) captures an empty one.
//!
//! Corvene addition (`799-fixup-commits`): [`autosquash`] folds `fixup!`
//! commits into their targets with a [`fixup_todo`] list (GHD has no fixup
//! commits).
//!
//! Corvene addition (`1221-stacked-branch-refs`): [`RebaseOptions::update_refs`]
//! adds `update-ref` lines to the todo lists ([`with_update_refs`]), so other
//! local branches in the rewritten range follow their commits as with
//! `rebase --update-refs` (GHD's `lib/git/rebase.ts` todo lists have none, and
//! replacing git's own todo drops those `rebase.updateRefs` adds).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use corvene_models::{
    Commit, CommitOneLine, FileStatusKind, GitStatusEntry, ManualConflictResolution, McoProgress,
    Mergeability, RebaseInternalState, WorkingDirectoryFileChange,
};
use tracing::warn;

use crate::detect::GitBinary;
use crate::error::{GitError, Result};
use crate::paths::git_dir;
use crate::process::GitCommand;

/// GHD `RebaseResult`
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RebaseResult {
    CompletedWithoutError,
    AlreadyUpToDate,
    /// Stopped on conflicts; `REBASE_HEAD` is set.
    ConflictsEncountered,
    /// `--continue` refused because conflicted files were not staged.
    OutstandingFilesNotStaged,
    /// The repository was not in a state where the rebase could go on.
    Aborted,
    Error(String),
}

/// GHD `CherryPickResult`
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CherryPickResult {
    CompletedWithoutError,
    ConflictsEncountered,
    OutstandingFilesNotStaged,
    UnableToStart,
    Error(String),
}

/// GHD `ICherryPickSnapshot`: reconstructed from `.git/sequencer`.
#[derive(Clone, Debug, PartialEq)]
pub struct CherryPickSnapshot {
    pub progress: McoProgress,
    pub remaining: Vec<CommitOneLine>,
    pub commits: Vec<CommitOneLine>,
    /// The target branch tip before the cherry-pick (`sequencer/head`).
    pub target_branch_undo_sha: String,
    pub cherry_picked_count: usize,
}

/// GHD `GitRebaseSnapshot`
#[derive(Clone, Debug, PartialEq)]
pub struct RebaseSnapshot {
    pub commits: Vec<CommitOneLine>,
    pub progress: McoProgress,
    /// A branch at the rebase's `onto` commit ([`branch_at`]), for a rebase
    /// started elsewhere.
    pub base_branch: Option<String>,
}

// ---------------------------------------------------------------------------
// repository state files
// ---------------------------------------------------------------------------

/// A rebase is in progress. Git ≥ 2.4x leaves `REBASE_HEAD` behind after a
/// finished rebase, so the `rebase-merge` / `rebase-apply` directories are
/// the reliable signal (GHD checks `REBASE_HEAD` and then reads those files).
pub fn rebase_head_set(workdir: &Path) -> bool {
    let dir = git_dir(workdir);
    dir.join("rebase-merge").exists() || dir.join("rebase-apply").exists()
}

pub fn cherry_pick_head_found(workdir: &Path) -> bool {
    git_dir(workdir).join("CHERRY_PICK_HEAD").exists()
}

pub fn merge_head_set(workdir: &Path) -> bool {
    git_dir(workdir).join("MERGE_HEAD").exists()
}

pub fn squash_msg_set(workdir: &Path) -> bool {
    git_dir(workdir).join("SQUASH_MSG").exists()
}

fn read_trimmed(path: PathBuf) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// GHD `getRebaseInternalState`
pub fn rebase_internal_state(workdir: &Path) -> Option<RebaseInternalState> {
    if !rebase_head_set(workdir) {
        return None;
    }
    let dir = git_dir(workdir).join("rebase-merge");
    let original_branch_tip = read_trimmed(dir.join("orig-head"))?;
    let head_name = read_trimmed(dir.join("head-name"))?;
    let base_branch_tip = read_trimmed(dir.join("onto"))?;
    let target_branch = head_name
        .strip_prefix("refs/heads/")
        .unwrap_or(&head_name)
        .to_string();
    Some(RebaseInternalState {
        target_branch,
        base_branch_tip,
        original_branch_tip,
    })
}

/// The stash's newest entry (`refs/stash`), to tell whether an autostash
/// could not be reapplied and was kept there.
pub fn stash_tip(git: Arc<GitBinary>, workdir: &Path) -> Option<String> {
    GitCommand::new(git)
        .args(["rev-parse", "-q", "--verify", "refs/stash"])
        .current_dir(workdir)
        .allow_exit_code(1)
        .run()
        .ok()
        .and_then(|o| o.stdout_string().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// GHD `formatRebaseValue`: clamp to 0..=1 with two decimals.
pub fn format_rebase_value(value: f32) -> f32 {
    (value.clamp(0., 1.) * 100.).round() / 100.
}

/// GHD `getRebaseSnapshot`: progress of a rebase started outside Corvene
/// (`.git/rebase-merge/{msgnum,end}`).
pub fn rebase_snapshot(git: Arc<GitBinary>, workdir: &Path) -> Option<RebaseSnapshot> {
    if !rebase_head_set(workdir) {
        return None;
    }
    let dir = git_dir(workdir).join("rebase-merge");
    let next: usize = read_trimmed(dir.join("msgnum"))?.parse().ok()?;
    let last: usize = read_trimmed(dir.join("end"))?.parse().ok()?;
    let original_branch_tip = read_trimmed(dir.join("orig-head"))?;
    let base_branch_tip = read_trimmed(dir.join("onto"))?;
    if next == 0 || last == 0 {
        return None;
    }
    let commits = commits_between(git.clone(), workdir, &base_branch_tip, &original_branch_tip)
        .ok()
        .flatten()?;
    if commits.is_empty() {
        return None;
    }
    let current_summary = commits
        .get(next - 1)
        .map(|c| c.summary.clone())
        .unwrap_or_default();
    let target = read_trimmed(dir.join("head-name"));
    let base_branch = branch_at(git, workdir, &base_branch_tip, target.as_deref());
    Some(RebaseSnapshot {
        base_branch,
        progress: McoProgress {
            value: format_rebase_value(next as f32 / last as f32),
            position: next,
            total: last,
            current_summary,
        },
        commits,
    })
}

/// A branch whose tip is `sha` (`for-each-ref --points-at`), local ones
/// first, never `except` (a full ref name) or a remote's `HEAD`.
pub fn branch_at(
    git: Arc<GitBinary>,
    workdir: &Path,
    sha: &str,
    except: Option<&str>,
) -> Option<String> {
    let out = GitCommand::new(git)
        .args([
            "for-each-ref",
            "--points-at",
            sha,
            "--format=%(refname)",
            "refs/heads",
            "refs/remotes",
        ])
        .current_dir(workdir)
        .run()
        .ok()?;
    let text = out.stdout_string().ok()?;
    let refs: Vec<&str> = text
        .lines()
        .filter(|r| Some(*r) != except && !r.ends_with("/HEAD"))
        .collect();
    let local = refs.iter().find_map(|r| r.strip_prefix("refs/heads/"));
    let remote = refs.iter().find_map(|r| r.strip_prefix("refs/remotes/"));
    local.or(remote).map(str::to_string)
}

// ---------------------------------------------------------------------------
// rev-list helpers
// ---------------------------------------------------------------------------

fn is_bad_revision(err: &GitError) -> bool {
    matches!(err, GitError::Failed { stderr, .. }
        if stderr.contains("bad revision") || stderr.contains("unknown revision"))
}

/// GHD `getCommitsInRange`: oldest first. `None` for a bad revision range.
pub fn commits_in_range(
    git: Arc<GitBinary>,
    workdir: &Path,
    range: &str,
) -> Result<Option<Vec<CommitOneLine>>> {
    let out = GitCommand::new(git)
        .args([
            "rev-list",
            range,
            "--reverse",
            "--oneline",
            "--no-abbrev-commit",
            "--",
        ])
        .current_dir(workdir)
        .run();
    let out = match out {
        Ok(out) => out,
        Err(err) if is_bad_revision(&err) => return Ok(None),
        Err(err) => return Err(err),
    };
    Ok(Some(parse_one_line_commits(&out.stdout_string()?)))
}

pub fn parse_one_line_commits(text: &str) -> Vec<CommitOneLine> {
    text.lines()
        .filter_map(|line| {
            let (sha, summary) = line.split_once(' ').unwrap_or((line, ""));
            (sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit())).then(|| CommitOneLine {
                sha: sha.to_string(),
                summary: summary.to_string(),
            })
        })
        .collect()
}

/// GHD `getCommitsBetweenCommits`: commits reachable from `target` but not
/// from `base`, in the order a rebase would apply them.
pub fn commits_between(
    git: Arc<GitBinary>,
    workdir: &Path,
    base: &str,
    target: &str,
) -> Result<Option<Vec<CommitOneLine>>> {
    commits_in_range(git, workdir, &format!("{base}..{target}"))
}

/// GHD `doMergeCommitsExistAfterCommit`
pub fn merge_commits_exist_after(
    git: Arc<GitBinary>,
    workdir: &Path,
    commit_ref: Option<&str>,
) -> Result<bool> {
    let revision = match commit_ref {
        Some(r) => format!("{r}..HEAD"),
        None => "HEAD".to_string(),
    };
    let out = GitCommand::new(git)
        .args(["rev-list", "-1", "--merges", &revision, "--"])
        .current_dir(workdir)
        .allow_exit_code(128)
        .run()?;
    Ok(!out.stdout.is_empty())
}

/// GHD `determineMergeability`: `merge-tree --write-tree` without touching
/// the working directory.
pub fn determine_mergeability(
    git: Arc<GitBinary>,
    workdir: &Path,
    ours: &str,
    theirs: &str,
) -> Result<Mergeability> {
    Ok(match merge_tree_conflicts(git, workdir, ours, theirs)? {
        None => Mergeability::Invalid,
        Some(files) if files.is_empty() => Mergeability::Clean,
        Some(files) => Mergeability::Conflicts(files.len() as u32),
    })
}

/// The files merging `theirs` into `ours` would leave conflicted
/// ([`determine_mergeability`]'s `merge-tree`); `None` for unrelated
/// histories. Corvene `889-compare-shows-conflicts` lists them.
pub fn merge_tree_conflicts(
    git: Arc<GitBinary>,
    workdir: &Path,
    ours: &str,
    theirs: &str,
) -> Result<Option<Vec<String>>> {
    let out = GitCommand::new(git)
        .args([
            "merge-tree",
            "--write-tree",
            "--name-only",
            "--no-messages",
            "-z",
            ours,
            theirs,
        ])
        .current_dir(workdir)
        .allow_exit_code(1)
        .run();
    match out {
        Ok(out) => Ok(Some(parse_merge_tree_names(&out.stdout))),
        Err(GitError::Failed { stderr, .. }) if stderr.contains("unrelated histories") => Ok(None),
        Err(err) => Err(err),
    }
}

/// `merge-tree --write-tree --name-only -z` output: "<tree-id>\0[<filename>\0]*".
pub fn parse_merge_tree_names(stdout: &[u8]) -> Vec<String> {
    stdout
        .split(|b| *b == 0)
        .skip(1)
        .filter(|name| !name.is_empty())
        .map(|name| String::from_utf8_lossy(name).into_owned())
        .collect()
}

// ---------------------------------------------------------------------------
// conflicts
// ---------------------------------------------------------------------------

/// GHD `getFilesWithConflictMarkers`: `diff --check` leftover markers per path.
pub fn conflict_marker_counts(git: Arc<GitBinary>, workdir: &Path) -> Result<HashMap<String, u32>> {
    let out = GitCommand::new(git)
        .args(["diff", "--check"])
        .current_dir(workdir)
        .allow_exit_code(2)
        .run()?;
    Ok(parse_conflict_markers(
        &out.stdout_string().unwrap_or_default(),
    ))
}

pub fn parse_conflict_markers(stdout: &str) -> HashMap<String, u32> {
    let mut counts = HashMap::new();
    for line in stdout.lines() {
        let Some(rest) = line.strip_suffix(" leftover conflict marker") else {
            continue;
        };
        // "<path>:<line>:"
        let Some(rest) = rest.strip_suffix(':') else {
            continue;
        };
        let Some((path, line_no)) = rest.rsplit_once(':') else {
            continue;
        };
        if line_no.parse::<u32>().is_ok() {
            *counts.entry(path.to_string()).or_insert(0) += 1;
        }
    }
    counts
}

/// GHD `getBinaryPaths` (`lib/git/diff.ts`): the files git detects as
/// binary when diffing the working tree against `reference`
/// (`getDetectedBinaryFiles`: `diff --numstat -z <reference>` records of
/// `-\t-`), then the `conflicted` paths whose `merge` attribute is `binary`
/// (`getFilesUsingBinaryMergeDriver`: `check-attr --stdin -z merge`), each
/// once. Fails when git does (an unborn `HEAD`, a missing `MERGE_HEAD`).
///
/// Deviation: a renamed binary file counts with its new path. GHD's
/// `binaryListRegex` runs on across the NUL-separated records there and
/// captures an empty path, losing the binary files listed after it.
pub fn binary_paths(
    git: Arc<GitBinary>,
    workdir: &Path,
    reference: &str,
    conflicted: &[String],
) -> Result<Vec<String>> {
    let out = GitCommand::new(git.clone())
        .args(["diff", "--numstat", "-z", reference])
        .current_dir(workdir)
        .run()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut fields = text.split('\0');
    let mut paths: Vec<String> = Vec::new();
    let mut push = |path: &str| {
        if !paths.iter().any(|p| p == path) {
            paths.push(path.to_string());
        }
    };
    while let Some(record) = fields.next() {
        let mut parts = record.splitn(3, '\t');
        let (Some(added), Some(deleted), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        // a rename: "added\tdeleted\t" NUL old NUL new
        let path = if path.is_empty() {
            fields.next();
            fields.next().unwrap_or_default()
        } else {
            path
        };
        if added == "-" && deleted == "-" {
            push(path);
        }
    }
    if !conflicted.is_empty() {
        let mut stdin: Vec<u8> = Vec::new();
        for path in conflicted {
            stdin.extend_from_slice(path.as_bytes());
            stdin.push(0);
        }
        let out = GitCommand::new(git)
            .args(["check-attr", "--stdin", "-z", "merge"])
            .current_dir(workdir)
            .stdin(stdin)
            .run()?;
        let text = String::from_utf8_lossy(&out.stdout);
        // `<path> NUL <attribute> NUL <value> NUL` per path
        let records: Vec<&str> = text.split('\0').collect();
        let (records, _) = records.as_chunks::<3>();
        for [path, attribute, value] in records {
            if *attribute == "merge" && *value == "binary" {
                push(path);
            }
        }
    }
    Ok(paths)
}

/// GHD `stageManualConflictResolution`: take one side of a conflicted file.
pub fn stage_manual_conflict_resolution(
    git: Arc<GitBinary>,
    workdir: &Path,
    file: &WorkingDirectoryFileChange,
    resolution: ManualConflictResolution,
) -> Result<()> {
    let status = &file.status;
    if !status.is_conflicted() {
        warn!(path = %file.path, "tried to manually resolve an unconflicted file");
        return Ok(());
    }
    if status.conflict_markers == Some(0) {
        // resolved in an editor after all; keep the file as is
        return Ok(());
    }
    let chosen = match resolution {
        ManualConflictResolution::Ours => status.us(),
        ManualConflictResolution::Theirs => status.them(),
    };
    let added_in_both =
        status.us() == GitStatusEntry::Added && status.them() == GitStatusEntry::Added;
    if chosen == GitStatusEntry::Unmerged || added_in_both {
        let side = match resolution {
            ManualConflictResolution::Ours => "--ours",
            ManualConflictResolution::Theirs => "--theirs",
        };
        GitCommand::new(git.clone())
            .args(["checkout", side, "--", &file.path])
            .current_dir(workdir)
            .run()?;
    }
    match chosen {
        GitStatusEntry::Deleted => {
            GitCommand::new(git)
                .args(["rm", "--", &file.path])
                .current_dir(workdir)
                .run()?;
        }
        _ => {
            GitCommand::new(git)
                .args(["add", "--", &file.path])
                .current_dir(workdir)
                .run()?;
        }
    }
    Ok(())
}

/// Corvene addition (flag `842`): `git mergetool --no-prompt -- <path>` with
/// the user's `merge.tool`; blocks until the tool exits (git stages the file
/// when the tool reports success). Refuses without a configured
/// `merge.tool`, since git would otherwise fall back to terminal tools.
pub fn open_merge_tool(git: Arc<GitBinary>, workdir: &Path, path: &str) -> Result<()> {
    if crate::config_value(git.clone(), workdir, "merge.tool").is_none() {
        return Err(GitError::Gix(
            "No merge tool is configured. Set one with git config --global merge.tool <tool> \
             (for example bc, kdiff3, opendiff or p4merge)."
                .into(),
        ));
    }
    GitCommand::new(git)
        .args(["mergetool", "--no-prompt", "--", path])
        .current_dir(workdir)
        .run()?;
    Ok(())
}

/// Stage tracked files before continuing an operation: manual resolutions
/// first, then `add` for everything else (GHD `continueRebase` /
/// `continueCherryPick` / `createMergeCommit` share this).
fn stage_for_continue(
    git: Arc<GitBinary>,
    workdir: &Path,
    files: &[WorkingDirectoryFileChange],
    resolutions: &BTreeMap<String, ManualConflictResolution>,
) -> Result<()> {
    let tracked: Vec<&WorkingDirectoryFileChange> = files
        .iter()
        .filter(|f| f.status.kind != FileStatusKind::Untracked)
        .collect();
    for (path, resolution) in resolutions {
        match tracked.iter().find(|f| &f.path == path) {
            Some(file) => {
                stage_manual_conflict_resolution(git.clone(), workdir, file, *resolution)?
            }
            None => warn!(%path, "manual resolution for a file that is not in the status"),
        }
    }
    let others: Vec<WorkingDirectoryFileChange> = tracked
        .iter()
        .filter(|f| !resolutions.contains_key(&f.path))
        .map(|f| (*f).clone())
        .collect();
    stage_all(git, workdir, &others)
}

/// GHD `stageFiles` for every given file (ignores the partial-selection
/// state, unlike `commit::stage_files`).
fn stage_all(
    git: Arc<GitBinary>,
    workdir: &Path,
    files: &[WorkingDirectoryFileChange],
) -> Result<()> {
    crate::commit::stage_whole_files(git, workdir, files)
}

/// GHD `createMergeCommit`: stage the conflicted files (with resolutions) and
/// commit with the prepared `MERGE_MSG` / `SQUASH_MSG`. `--cleanup=strip`
/// drops its `#` commentary (the `# Conflicts:` list), which git keeps with
/// `--no-edit` otherwise. Returns `parse_commit_sha` of git's output (the
/// abbreviated sha), as GHD does.
pub fn create_merge_commit(
    git: Arc<GitBinary>,
    workdir: &Path,
    conflicted: &[WorkingDirectoryFileChange],
    resolutions: &BTreeMap<String, ManualConflictResolution>,
) -> Result<String> {
    stage_for_continue(git.clone(), workdir, conflicted, resolutions)?;
    let out = GitCommand::new(git)
        .args(["commit", "--no-edit", "--cleanup=strip"])
        .env("GIT_EDITOR", ":")
        .current_dir(workdir)
        .run()?;
    Ok(crate::commit::parse_commit_sha(&String::from_utf8_lossy(
        &out.stdout,
    )))
}

/// GHD `_abortSquashMerge`: a `merge --squash` has no `MERGE_HEAD` to abort,
/// so go back to the tip and drop the prepared message.
pub fn abort_squash_merge(git: Arc<GitBinary>, workdir: &Path, tip: &str) -> Result<()> {
    GitCommand::new(git)
        .args(["reset", "--hard", tip])
        .current_dir(workdir)
        .run()?;
    let dir = git_dir(workdir);
    for name in ["SQUASH_MSG", "MERGE_MSG"] {
        let _ = std::fs::remove_file(dir.join(name));
    }
    Ok(())
}

fn stderr_says_unresolved(stderr: &str) -> bool {
    stderr.contains("unresolved conflict")
        || stderr.contains("You must edit all merge conflicts")
        || stderr.contains("mark them as resolved")
}

// ---------------------------------------------------------------------------
// rebase
// ---------------------------------------------------------------------------

/// The cut line git writes above the conflict hint in `scissors` cleanup mode.
const SCISSORS: &str = "------------------------ >8 ------------------------";

/// `-c commit.cleanup=scissors` when `keep`: commit messages are kept as
/// written (lines starting with `#` included) and only the conflict hint git
/// appends below a cut line is dropped. Git's default, `strip`, drops every
/// `#` line, so a message whose summary starts with `#` ends up empty after
/// a conflict ("Aborting commit due to empty commit message").
fn cleanup_config(keep: bool) -> &'static [&'static str] {
    if keep {
        &["-c", "commit.cleanup=scissors"]
    } else {
        &[]
    }
}

/// The stopped pick's message has the cut line, i.e. it was started with
/// [`cleanup_config`]; a rebase started elsewhere keeps git's default.
fn message_has_scissors(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|m| m.lines().any(|l| l.contains(SCISSORS)))
}

/// Marker in `rebase-merge/` (git deletes the folder when the rebase ends)
/// saying the rebase was started with [`cleanup_config`]. Older git (2.43,
/// Ubuntu 24.04's) writes no cut line into a stopped pick's message, so
/// [`message_has_scissors`] alone cannot tell.
const KEEP_MESSAGES_MARKER: &str = "rebase-merge/corvene-keep-messages";

/// After a rebase started with `keep` stopped: leave [`KEEP_MESSAGES_MARKER`].
fn mark_kept_messages(workdir: &Path, keep: bool, result: &RebaseResult) {
    if keep && *result == RebaseResult::ConflictsEncountered {
        let marker = git_dir(workdir).join(KEEP_MESSAGES_MARKER);
        if let Err(err) = std::fs::write(&marker, "") {
            warn!(%err, "could not mark the rebase as keeping messages");
        }
    }
}

/// Whether the stopped rebase was started with [`cleanup_config`].
fn rebase_keeps_messages(workdir: &Path) -> bool {
    let dir = git_dir(workdir);
    dir.join(KEEP_MESSAGES_MARKER).is_file()
        || message_has_scissors(&dir.join("rebase-merge/message"))
}

/// GHD `GitRebaseParser`: `Rebasing (n/m)` on stderr.
pub fn parse_rebase_progress(line: &str, commits: &[CommitOneLine]) -> Option<McoProgress> {
    let rest = line.trim().strip_prefix("Rebasing (")?;
    let rest = rest.strip_suffix(')')?;
    let (done, total) = rest.split_once('/')?;
    let done: usize = done.parse().ok()?;
    let total: usize = total.parse().ok()?;
    if total == 0 {
        return None;
    }
    Some(McoProgress {
        value: format_rebase_value(done as f32 / total as f32),
        position: done,
        total,
        current_summary: commits
            .get(done.saturating_sub(1))
            .map(|c| c.summary.clone())
            .unwrap_or_default(),
    })
}

fn classify_rebase(workdir: &Path, result: Result<crate::process::GitOutput>) -> RebaseResult {
    match result {
        Ok(out) => {
            let stdout = out.stdout_string().unwrap_or_default();
            let up_to_date = stdout.lines().any(|l| {
                l.starts_with("Current branch ") && l.trim_end().ends_with("is up to date.")
            });
            if up_to_date {
                RebaseResult::AlreadyUpToDate
            } else {
                RebaseResult::CompletedWithoutError
            }
        }
        Err(GitError::Failed { stderr, .. }) => {
            let unresolved = stderr_says_unresolved(&stderr);
            if rebase_head_set(workdir) && (!unresolved || stderr.contains("could not apply")) {
                RebaseResult::ConflictsEncountered
            } else if unresolved {
                RebaseResult::OutstandingFilesNotStaged
            } else {
                RebaseResult::Error(stderr)
            }
        }
        Err(err) => RebaseResult::Error(err.to_string()),
    }
}

/// GHD `rebase`: `git rebase <base> <target>` with progress from stderr.
/// `keep_messages`: see [`cleanup_config`].
pub fn rebase(
    git: Arc<GitBinary>,
    workdir: &Path,
    base_branch: &str,
    target_branch: &str,
    commits: &[CommitOneLine],
    keep_messages: bool,
    mut on_progress: impl FnMut(McoProgress),
) -> RebaseResult {
    let result = GitCommand::new(git)
        .args(cleanup_config(keep_messages))
        .args([
            "-c",
            "rebase.backend=merge",
            "rebase",
            base_branch,
            target_branch,
        ])
        .env("GIT_EDITOR", ":")
        .current_dir(workdir)
        .run_streaming(|line| {
            if let Some(p) = parse_rebase_progress(line, commits) {
                on_progress(p);
            }
        });
    let result = classify_rebase(workdir, result);
    mark_kept_messages(workdir, keep_messages, &result);
    result
}

/// GHD `abortRebase`
pub fn abort_rebase(git: Arc<GitBinary>, workdir: &Path) -> Result<()> {
    GitCommand::new(git)
        .args(["rebase", "--abort"])
        .current_dir(workdir)
        .run()?;
    crate::rewrite_untracked::restore_set_aside(workdir)
}

/// GHD `continueRebase`: stage the (resolved) tracked files, then
/// `rebase --continue`, or `--skip` when nothing is left to commit.
/// `git_editor` is `GIT_EDITOR` for git (GHD `opts.gitEditor`, e.g. `cat
/// "<message file>" >` to reword the stopped commit); `None` is `:`, which
/// keeps the prepared message.
#[allow(clippy::too_many_arguments)]
pub fn continue_rebase(
    git: Arc<GitBinary>,
    workdir: &Path,
    files: &[WorkingDirectoryFileChange],
    resolutions: &BTreeMap<String, ManualConflictResolution>,
    commits: &[CommitOneLine],
    keep_messages: bool,
    git_editor: Option<&str>,
    mut on_progress: impl FnMut(McoProgress),
) -> Result<RebaseResult> {
    stage_for_continue(git.clone(), workdir, files, resolutions)?;
    if !rebase_head_set(workdir) {
        return Ok(RebaseResult::Aborted);
    }
    let keep_messages = keep_messages && rebase_keeps_messages(workdir);
    let status = crate::status::get_status(git.clone(), workdir)?;
    let tracked_after = status
        .files
        .iter()
        .filter(|f| f.status.kind != FileStatusKind::Untracked)
        .count();
    let action = if tracked_after == 0 {
        warn!("no tracked changes to commit; skipping this commit");
        "--skip"
    } else {
        "--continue"
    };
    let result = GitCommand::new(git)
        .args(cleanup_config(keep_messages))
        .args(["rebase", action])
        .env("GIT_EDITOR", git_editor.unwrap_or(":"))
        .current_dir(workdir)
        .run_streaming(|line| {
            if let Some(p) = parse_rebase_progress(line, commits) {
                on_progress(p);
            }
        });
    Ok(restored_after(workdir, classify_rebase(workdir, result)))
}

/// Corvene `892-squash-message-survives-conflicts`: GIT_EDITOR for
/// continuing a squash that stopped on a conflict. git hands the editor the
/// squashed commit's message (it starts with "This is a combination of N
/// commits"; git runs in English here) only for the last squash of a chain;
/// that buffer gets `message_path`'s message, any other one (a conflicted
/// pick's own message) is left as it is, as `:` would.
pub fn squash_continue_editor(message_path: &Path) -> Option<String> {
    let path = message_path.to_string_lossy();
    if path.contains('"') {
        return None;
    }
    Some(format!(
        "f() {{ if grep -q 'This is a combination of [0-9]* commits' \"$1\"; then cat \"{path}\" > \"$1\"; fi; }}; f"
    ))
}

/// Corvene `892-squash-message-survives-conflicts`: [`continue_rebase`] for
/// a squash, keeping the message typed for it (GHD continues with git's
/// combined message once a squash hit a conflict).
#[allow(clippy::too_many_arguments)]
pub fn continue_squash_rebase(
    git: Arc<GitBinary>,
    workdir: &Path,
    files: &[WorkingDirectoryFileChange],
    resolutions: &BTreeMap<String, ManualConflictResolution>,
    commits: &[CommitOneLine],
    keep_messages: bool,
    message: &str,
    on_progress: impl FnMut(McoProgress),
) -> Result<RebaseResult> {
    if message.trim().is_empty() {
        return continue_rebase(
            git,
            workdir,
            files,
            resolutions,
            commits,
            keep_messages,
            None,
            on_progress,
        );
    }
    let message_path = temp_file("squash-message", message)?;
    let editor = squash_continue_editor(&message_path);
    let result = continue_rebase(
        git,
        workdir,
        files,
        resolutions,
        commits,
        keep_messages && editor.is_some(),
        editor.as_deref(),
        on_progress,
    );
    let _ = std::fs::remove_file(&message_path);
    result
}

/// Options for the interactive rebases behind squash and reorder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RebaseOptions {
    /// See [`cleanup_config`] (flag `834`).
    pub keep_messages: bool,
    /// `--autostash`: local changes are stashed first and reapplied after
    /// (flag `829`).
    pub autostash: bool,
    /// Corvene `1221-stacked-branch-refs`: the branches to move with the
    /// commits they point at ([`with_update_refs`]); empty leaves every
    /// other branch where it is.
    pub update_refs: UpdateRefs,
    /// Corvene `1226-rewrite-keeps-untracked`: files a commit in the range
    /// stopped tracking are kept out of the replay's way.
    pub keep_untracked: bool,
}

/// Corvene `1221-stacked-branch-refs`: full branch refs
/// (`refs/heads/<name>`) by the commit they point at, for the todo's
/// `update-ref` lines.
pub type UpdateRefs = BTreeMap<String, Vec<String>>;

/// Corvene `1221-stacked-branch-refs`: `todo` with an `update-ref <ref>`
/// line for each of `refs` whose commit is picked, the way
/// `rebase --update-refs` writes them: after the commit's line, or after the
/// whole `squash` / `fixup` group it is part of, so a branch on a squashed
/// commit (or on the commit squashed onto) lands on the combined commit and
/// not on an intermediate one that ends up in no history. Refs of commits
/// the todo does not pick are left out; each ref is written once.
pub fn with_update_refs(todo: &str, refs: &UpdateRefs) -> String {
    if refs.is_empty() {
        return todo.to_string();
    }
    let mut out = String::new();
    let mut written: HashSet<&str> = HashSet::new();
    // the refs of the current pick and the squashes / fixups after it
    let mut pending: Vec<&str> = Vec::new();
    let flush = |out: &mut String, pending: &mut Vec<&str>| {
        for r in pending.drain(..) {
            out.push_str("update-ref ");
            out.push_str(r);
            out.push('\n');
        }
    };
    for line in todo.lines() {
        let mut words = line.split_whitespace();
        let action = words.next().unwrap_or_default();
        let folds = matches!(action, "squash" | "s" | "fixup" | "f");
        if !folds {
            flush(&mut out, &mut pending);
        }
        out.push_str(line);
        out.push('\n');
        if matches!(
            action,
            "pick" | "p" | "reword" | "r" | "edit" | "e" | "squash" | "s" | "fixup" | "f"
        ) && let Some(sha) = words.next()
            && let Some(names) = refs.get(sha)
        {
            for r in names {
                if written.insert(r.as_str()) {
                    pending.push(r.as_str());
                }
            }
        }
    }
    flush(&mut out, &mut pending);
    out
}

/// Corvene `1221-stacked-branch-refs`: Undo of a rewrite that moved other
/// branches puts each `(full ref, sha)` of `tips` back, in one
/// `update-ref --stdin` transaction.
pub fn restore_branch_tips(
    git: Arc<GitBinary>,
    workdir: &Path,
    tips: &[(String, String)],
) -> Result<()> {
    if tips.is_empty() {
        return Ok(());
    }
    let input: String = tips
        .iter()
        .map(|(r, sha)| format!("update {r} {sha}\n"))
        .collect();
    GitCommand::new(git)
        .args([
            "update-ref",
            "-m",
            "undo: move stacked branches back",
            "--stdin",
        ])
        .stdin(input.into_bytes())
        .current_dir(workdir)
        .run()?;
    Ok(())
}

/// [`temp_file`] for a todo list, with `options`' `update-ref` lines.
fn todo_file(prefix: &str, todo: &str, options: &RebaseOptions) -> Result<PathBuf> {
    temp_file(prefix, &with_update_refs(todo, &options.update_refs))
}

/// GHD `rebaseInteractive`: replay `todo` with `sequence.editor=cat todo >`.
/// `last_retained_ref` is the commit before the first rewritten one, or
/// `None` to rebase from the root.
#[allow(clippy::too_many_arguments)]
pub fn rebase_interactive(
    git: Arc<GitBinary>,
    workdir: &Path,
    todo: &Path,
    last_retained_ref: Option<&str>,
    git_editor: Option<&str>,
    commits: &[CommitOneLine],
    options: RebaseOptions,
    mut on_progress: impl FnMut(McoProgress),
) -> RebaseResult {
    let todo_path = todo.to_string_lossy();
    if todo_path.contains('"') {
        return RebaseResult::Error("temporary todo path contains a quote".into());
    }
    if options.keep_untracked
        && let Err(err) = crate::rewrite_untracked::set_aside_for_rewrite(
            git.clone(),
            workdir,
            last_retained_ref,
        )
    {
        return RebaseResult::Error(err.to_string());
    }
    let sequence_editor = format!("sequence.editor=cat \"{todo_path}\" >");
    let base = last_retained_ref.unwrap_or("--root");
    let mut args = vec![
        "-c",
        &sequence_editor,
        "-c",
        "rebase.backend=merge",
        "rebase",
        "-i",
    ];
    if options.autostash {
        args.push("--autostash");
    }
    args.push(base);
    let result = GitCommand::new(git)
        .args(cleanup_config(options.keep_messages))
        .args(args)
        .env_remove("GIT_SEQUENCE_EDITOR")
        .env("GIT_EDITOR", git_editor.unwrap_or(":"))
        .current_dir(workdir)
        .run_streaming(|line| {
            if let Some(p) = parse_rebase_progress(line, commits) {
                on_progress(p);
            }
        });
    let result = classify_rebase(workdir, result);
    mark_kept_messages(workdir, options.keep_messages, &result);
    restored_after(workdir, result)
}

/// `1226-rewrite-keeps-untracked`: the files set aside come back once the
/// rebase is over. A no-op without any.
fn restored_after(workdir: &Path, result: RebaseResult) -> RebaseResult {
    match crate::rewrite_untracked::restore_set_aside(workdir) {
        Ok(()) => result,
        Err(err) => RebaseResult::Error(err.to_string()),
    }
}

/// Commits from `last_retained_ref` (exclusive) to HEAD, oldest first; the
/// whole branch when rebasing from the root.
fn commits_to_replay(
    git: Arc<GitBinary>,
    workdir: &Path,
    last_retained_ref: Option<&str>,
) -> Result<Vec<CommitOneLine>> {
    let range = match last_retained_ref {
        Some(r) => format!("{r}..HEAD"),
        None => "HEAD".to_string(),
    };
    commits_in_range(git, workdir, &range)?
        .ok_or_else(|| GitError::Gix(format!("could not list commits for {range}")))
}

/// GHD `squash`'s todo list. `commits` are oldest first; `to_squash` never
/// contains `squash_onto`. Returns `None` when `squash_onto` is not in the log.
pub fn squash_todo(
    commits: &[CommitOneLine],
    to_squash: &HashSet<String>,
    squash_onto: &str,
) -> Option<String> {
    let mut todo = String::new();
    let mut found_onto = false;
    let mut replay_at_squash: Vec<&CommitOneLine> = Vec::new();
    let mut replay_after: Vec<&CommitOneLine> = Vec::new();
    for commit in commits {
        if to_squash.contains(&commit.sha) {
            if found_onto {
                todo.push_str(&format!("squash {} {}\n", commit.sha, commit.summary));
            } else {
                replay_at_squash.push(commit);
            }
            continue;
        }
        if commit.sha == squash_onto {
            found_onto = true;
            replay_at_squash.push(commit);
            for (i, c) in replay_at_squash.iter().enumerate() {
                let action = if i == 0 { "pick" } else { "squash" };
                todo.push_str(&format!("{action} {} {}\n", c.sha, c.summary));
            }
            continue;
        }
        if found_onto {
            replay_after.push(commit);
            continue;
        }
        todo.push_str(&format!("pick {} {}\n", commit.sha, commit.summary));
    }
    for c in replay_after {
        todo.push_str(&format!("pick {} {}\n", c.sha, c.summary));
    }
    found_onto.then_some(todo)
}

/// GHD `reorder`'s todo list. `to_move` commits end up right before
/// `before` (or at the newest end when `before` is `None`).
pub fn reorder_todo(
    commits: &[CommitOneLine],
    to_move: &HashSet<String>,
    before: Option<&str>,
) -> Option<String> {
    let mut todo = String::new();
    let mut found_base = before.is_none() && false;
    let mut replay_before_base: Vec<&CommitOneLine> = Vec::new();
    let mut replay_after: Vec<&CommitOneLine> = Vec::new();
    for commit in commits {
        if to_move.contains(&commit.sha) {
            if found_base {
                todo.push_str(&format!("pick {} {}\n", commit.sha, commit.summary));
            } else {
                replay_before_base.push(commit);
            }
            continue;
        }
        if before == Some(commit.sha.as_str()) {
            found_base = true;
            replay_after.push(commit);
            for c in &replay_before_base {
                todo.push_str(&format!("pick {} {}\n", c.sha, c.summary));
            }
            continue;
        }
        if found_base {
            replay_after.push(commit);
            continue;
        }
        todo.push_str(&format!("pick {} {}\n", commit.sha, commit.summary));
    }
    for c in replay_after {
        todo.push_str(&format!("pick {} {}\n", c.sha, c.summary));
    }
    if before.is_none() {
        for c in &replay_before_base {
            todo.push_str(&format!("pick {} {}\n", c.sha, c.summary));
        }
    } else if !found_base {
        return None;
    }
    Some(todo)
}

/// Corvene `799-fixup-commits`: the subject prefix `git commit --fixup`
/// writes.
pub const FIXUP_PREFIX: &str = "fixup! ";

/// Corvene `799-fixup-commits`: which commit each `fixup!` commit among
/// `commits` (oldest first) belongs to, as `(fixup, target)` shas, the way
/// `rebase --autosquash` matches them (`todo_list_rearrange_squash`): the
/// rest of the subject is an earlier commit's whole subject (the oldest
/// such), else a prefix of its sha, else a prefix of its subject. A fixup of
/// a fixup belongs to that one's target. Unmatched fixups are left out.
pub fn autosquash_pairs(commits: &[CommitOneLine]) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    for (i, commit) in commits.iter().enumerate() {
        let Some(rest) = commit.summary.strip_prefix(FIXUP_PREFIX) else {
            continue;
        };
        let rest = rest.trim();
        if rest.is_empty() {
            continue;
        }
        let earlier = &commits[..i];
        let target = earlier
            .iter()
            .find(|c| c.summary == rest)
            .or_else(|| {
                (rest.len() >= 4 && !rest.contains(' '))
                    .then(|| earlier.iter().find(|c| c.sha.starts_with(rest)))
                    .flatten()
            })
            .or_else(|| earlier.iter().find(|c| c.summary.starts_with(rest)));
        let Some(target) = target else {
            continue;
        };
        // a fixup of a fixup goes where that one goes
        let root = pairs
            .iter()
            .find(|(fixup, _)| *fixup == target.sha)
            .map(|(_, root)| root.clone())
            .unwrap_or_else(|| target.sha.clone());
        pairs.push((commit.sha.clone(), root));
    }
    pairs
}

/// Corvene `799-fixup-commits`: the todo list folding each `(fixup,
/// target)` of `pairs` into its target with git's `fixup` action (the
/// target keeps its message), the fixups of one target in their order and
/// everything else replayed as it was. `commits` are oldest first. `None`
/// when a pair's commits are not both in the list or a fixup comes before
/// its target.
pub fn fixup_todo(commits: &[CommitOneLine], pairs: &[(String, String)]) -> Option<String> {
    let position = |sha: &str| commits.iter().position(|c| c.sha == sha);
    for (fixup, target) in pairs {
        if position(fixup)? <= position(target)? {
            return None;
        }
    }
    let is_fixup = |sha: &str| pairs.iter().any(|(fixup, _)| fixup == sha);
    let mut todo = String::new();
    for commit in commits.iter().filter(|c| !is_fixup(&c.sha)) {
        todo.push_str(&format!("pick {} {}\n", commit.sha, commit.summary));
        for fixup in commits.iter().filter(|c| {
            pairs
                .iter()
                .any(|(fixup, target)| *fixup == c.sha && *target == commit.sha)
        }) {
            todo.push_str(&format!("fixup {} {}\n", fixup.sha, fixup.summary));
        }
    }
    Some(todo)
}

/// Corvene `799-fixup-commits`: fold the `fixup!` commits after
/// `last_retained_ref` (`None`: the whole branch) into their targets
/// ([`autosquash_pairs`], [`fixup_todo`]), what `rebase -i --autosquash`
/// does, through [`rebase_interactive`] so progress and conflicts work as
/// for a squash. Returns the result and how many fixups were folded.
pub fn autosquash(
    git: Arc<GitBinary>,
    workdir: &Path,
    last_retained_ref: Option<&str>,
    options: RebaseOptions,
    on_progress: impl FnMut(McoProgress),
) -> (RebaseResult, usize) {
    let commits = match commits_to_replay(git.clone(), workdir, last_retained_ref) {
        Ok(c) if !c.is_empty() => c,
        Ok(_) => {
            return (
                RebaseResult::Error("could not find commits to replay".into()),
                0,
            );
        }
        Err(err) => return (RebaseResult::Error(err.to_string()), 0),
    };
    let pairs = autosquash_pairs(&commits);
    if pairs.is_empty() {
        return (RebaseResult::AlreadyUpToDate, 0);
    }
    let Some(todo) = fixup_todo(&commits, &pairs) else {
        return (
            RebaseResult::Error("a fixup commit comes before the commit it fixes".into()),
            0,
        );
    };
    let todo_path = match todo_file("autosquash-todo", &todo, &options) {
        Ok(p) => p,
        Err(err) => return (RebaseResult::Error(err.to_string()), 0),
    };
    let result = rebase_interactive(
        git,
        workdir,
        &todo_path,
        last_retained_ref,
        None,
        &commits,
        options,
        on_progress,
    );
    let _ = std::fs::remove_file(&todo_path);
    (result, pairs.len())
}

#[doc(hidden)]
pub fn temp_file(prefix: &str, contents: &str) -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "corvene-{prefix}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&path, contents)?;
    Ok(path)
}

/// Corvene `892-edit-commit-message`: the todo list giving `sha` a new
/// message (`reword`) and replaying the rest unchanged. `None` when `sha` is
/// not among `commits`.
pub fn reword_todo(commits: &[CommitOneLine], sha: &str) -> Option<String> {
    commits.iter().any(|c| c.sha == sha).then(|| {
        commits
            .iter()
            .map(|c| {
                let action = if c.sha == sha { "reword" } else { "pick" };
                format!("{action} {} {}\n", c.sha, c.summary)
            })
            .collect()
    })
}

/// Corvene `892-edit-commit-message`: give commit `sha` (not HEAD) the
/// message `message` with an interactive rebase from `last_retained_ref`
/// (its parent, `None` for a root commit), GIT_EDITOR writing the message.
/// Only the message changes, so nothing can conflict; local changes are
/// stashed for the rebase and put back. A rebase that stops anyway is
/// aborted.
pub fn reword(
    git: Arc<GitBinary>,
    workdir: &Path,
    sha: &str,
    last_retained_ref: Option<&str>,
    message: &str,
    keep_messages: bool,
    update_refs: &UpdateRefs,
) -> Result<()> {
    // an interactive rebase would flatten them
    if merge_commits_exist_after(git.clone(), workdir, last_retained_ref)? {
        return Err(GitError::Gix(
            "the branch has merge commits after this commit".into(),
        ));
    }
    let commits = commits_to_replay(git.clone(), workdir, last_retained_ref)?;
    let todo = reword_todo(&commits, sha)
        .ok_or_else(|| GitError::Gix("the commit is not on the current branch".into()))?;
    let todo_path = temp_file("reword-todo", &with_update_refs(&todo, update_refs))?;
    let message_path = temp_file("reword-message", message)?;
    let message_arg = message_path.to_string_lossy().to_string();
    let result = if message_arg.contains('"') {
        RebaseResult::Error("temporary message path contains a quote".into())
    } else {
        rebase_interactive(
            git.clone(),
            workdir,
            &todo_path,
            last_retained_ref,
            Some(&format!("cat \"{message_arg}\" >")),
            &commits,
            RebaseOptions {
                keep_messages,
                autostash: true,
                update_refs: UpdateRefs::new(),
                // a Corvene operation: nothing of GHD's to keep
                keep_untracked: true,
            },
            |_| {},
        )
    };
    let _ = std::fs::remove_file(&todo_path);
    let _ = std::fs::remove_file(&message_path);
    match result {
        RebaseResult::CompletedWithoutError | RebaseResult::AlreadyUpToDate => Ok(()),
        RebaseResult::Error(err) => Err(GitError::Gix(err)),
        other => {
            if rebase_head_set(workdir) || git_dir(workdir).join("rebase-merge").exists() {
                abort_rebase(git, workdir)?;
            }
            Err(GitError::Gix(format!(
                "the commit message could not be changed ({other:?})"
            )))
        }
    }
}

/// Corvene `892-edit-commit-message`: HEAD's message only (`commit --amend
/// --only -F -`), whatever is staged.
pub fn reword_head(git: Arc<GitBinary>, workdir: &Path, message: &str) -> Result<()> {
    GitCommand::new(git)
        .args(["commit", "--amend", "--only", "--allow-empty", "-F", "-"])
        .stdin(message.as_bytes().to_vec())
        .current_dir(workdir)
        .run()?;
    Ok(())
}

/// GHD `squash`: squash `to_squash` onto `squash_onto` with `message`
/// (summary line + body) via interactive rebase.
#[allow(clippy::too_many_arguments)]
pub fn squash(
    git: Arc<GitBinary>,
    workdir: &Path,
    to_squash: &[Commit],
    squash_onto: &Commit,
    last_retained_ref: Option<&str>,
    message: &str,
    options: RebaseOptions,
    on_progress: impl FnMut(McoProgress),
) -> RebaseResult {
    if to_squash.is_empty() {
        return RebaseResult::Error("no commits to squash".into());
    }
    let shas: HashSet<String> = to_squash.iter().map(|c| c.sha.clone()).collect();
    if shas.contains(&squash_onto.sha) {
        return RebaseResult::Error("cannot squash a commit onto itself".into());
    }
    let commits = match commits_to_replay(git.clone(), workdir, last_retained_ref) {
        Ok(c) if !c.is_empty() => c,
        Ok(_) => return RebaseResult::Error("could not find commits to replay".into()),
        Err(err) => return RebaseResult::Error(err.to_string()),
    };
    let Some(todo) = squash_todo(&commits, &shas, &squash_onto.sha) else {
        return RebaseResult::Error(
            "the commit to squash onto is not in the log; continuing would drop commits".into(),
        );
    };
    let todo_path = match todo_file("squash-todo", &todo, &options) {
        Ok(p) => p,
        Err(err) => return RebaseResult::Error(err.to_string()),
    };
    let message_path = if message.trim().is_empty() {
        None
    } else {
        match temp_file("squash-message", message) {
            Ok(p) => Some(p),
            Err(err) => return RebaseResult::Error(err.to_string()),
        }
    };
    let editor = message_path
        .as_ref()
        .map(|p| format!("cat \"{}\" >", p.to_string_lossy()));
    let involved: Vec<CommitOneLine> = to_squash
        .iter()
        .chain(std::iter::once(squash_onto))
        .map(|c| CommitOneLine {
            sha: c.sha.clone(),
            summary: c.summary.clone(),
        })
        .collect();
    // without a message git's own (with its `#` notes) would be kept
    let options = RebaseOptions {
        keep_messages: options.keep_messages && editor.is_some(),
        ..options
    };
    let result = rebase_interactive(
        git,
        workdir,
        &todo_path,
        last_retained_ref,
        editor.as_deref(),
        &involved,
        options,
        on_progress,
    );
    let _ = std::fs::remove_file(&todo_path);
    if let Some(p) = message_path {
        let _ = std::fs::remove_file(p);
    }
    result
}

/// GHD `reorder`: move `to_move` right before `before` (or to the tip).
pub fn reorder(
    git: Arc<GitBinary>,
    workdir: &Path,
    to_move: &[Commit],
    before: Option<&Commit>,
    last_retained_ref: Option<&str>,
    options: RebaseOptions,
    on_progress: impl FnMut(McoProgress),
) -> RebaseResult {
    if to_move.is_empty() {
        return RebaseResult::Error("no commits to reorder".into());
    }
    let shas: HashSet<String> = to_move.iter().map(|c| c.sha.clone()).collect();
    let commits = match commits_to_replay(git.clone(), workdir, last_retained_ref) {
        Ok(c) if !c.is_empty() => c,
        Ok(_) => return RebaseResult::Error("could not find commits to replay".into()),
        Err(err) => return RebaseResult::Error(err.to_string()),
    };
    let Some(todo) = reorder_todo(&commits, &shas, before.map(|c| c.sha.as_str())) else {
        return RebaseResult::Error(
            "the base commit is not in the log; continuing would drop commits".into(),
        );
    };
    let todo_path = match todo_file("reorder-todo", &todo, &options) {
        Ok(p) => p,
        Err(err) => return RebaseResult::Error(err.to_string()),
    };
    let result = rebase_interactive(
        git,
        workdir,
        &todo_path,
        last_retained_ref,
        None,
        &commits,
        options,
        on_progress,
    );
    let _ = std::fs::remove_file(&todo_path);
    result
}

// ---------------------------------------------------------------------------
// cherry-pick
// ---------------------------------------------------------------------------

/// GHD `GitCherryPickParser`: `[branch sha] summary` on stdout per commit.
pub fn parse_cherry_pick_progress(
    line: &str,
    commits: &[CommitOneLine],
    count: &mut usize,
) -> Option<McoProgress> {
    let rest = line.strip_prefix('[')?;
    let (inside, _) = rest.split_once(']')?;
    if !inside.contains(' ') {
        return None;
    }
    *count += 1;
    let total = commits.len().max(1);
    Some(McoProgress {
        value: format_rebase_value(*count as f32 / total as f32),
        position: *count,
        total: commits.len(),
        current_summary: commits
            .get(*count - 1)
            .map(|c| c.summary.clone())
            .unwrap_or_default(),
    })
}

fn classify_cherry_pick(
    workdir: &Path,
    result: Result<crate::process::GitOutput>,
) -> CherryPickResult {
    match result {
        Ok(_) => CherryPickResult::CompletedWithoutError,
        Err(GitError::Failed { stderr, .. }) => {
            if stderr_says_unresolved(&stderr) && !stderr.contains("could not apply") {
                CherryPickResult::OutstandingFilesNotStaged
            } else if cherry_pick_head_found(workdir) {
                CherryPickResult::ConflictsEncountered
            } else {
                CherryPickResult::Error(stderr)
            }
        }
        Err(err) => CherryPickResult::Error(err.to_string()),
    }
}

/// GHD `cherryPick`: `cherry-pick <shas> --empty=keep -m 1` (oldest first).
/// `keep_messages` adds `--cleanup=scissors` (see [`cleanup_config`]; the
/// sequencer remembers it for the later picks).
pub fn cherry_pick(
    git: Arc<GitBinary>,
    workdir: &Path,
    commits: &[CommitOneLine],
    keep_messages: bool,
    mut on_progress: impl FnMut(McoProgress),
) -> CherryPickResult {
    if commits.is_empty() {
        return CherryPickResult::UnableToStart;
    }
    let mut args: Vec<String> = vec!["cherry-pick".into()];
    args.extend(commits.iter().map(|c| c.sha.clone()));
    // `--empty=keep` (git ≥ 2.45) keeps empty picks in the history like GHD;
    // older gits get the equivalent pair of flags.
    if git.version.major > 2 || (git.version.major == 2 && git.version.minor >= 45) {
        args.push("--empty=keep".into());
    } else {
        args.push("--keep-redundant-commits".into());
        args.push("--allow-empty".into());
    }
    args.push("-m".into());
    args.push("1".into());
    if keep_messages {
        args.push("--cleanup=scissors".into());
    }
    let mut count = 0;
    let result = GitCommand::new(git)
        .args(&args)
        .env("GIT_EDITOR", ":")
        .current_dir(workdir)
        .run_streaming_stdout(|line| {
            if let Some(p) = parse_cherry_pick_progress(line, commits, &mut count) {
                on_progress(p);
            }
        });
    classify_cherry_pick(workdir, result)
}

/// GHD `abortCherryPick`
pub fn abort_cherry_pick(git: Arc<GitBinary>, workdir: &Path) -> Result<()> {
    GitCommand::new(git)
        .args(["cherry-pick", "--abort"])
        .current_dir(workdir)
        .run()?;
    Ok(())
}

/// GHD `getCherryPickSnapshot`: reconstruct progress from `.git/sequencer`.
pub fn cherry_pick_snapshot(git: Arc<GitBinary>, workdir: &Path) -> Option<CherryPickSnapshot> {
    if !cherry_pick_head_found(workdir) {
        return None;
    }
    let dir = git_dir(workdir);
    let sequencer = dir.join("sequencer");
    let parsed = (|| {
        let abort_safety = read_trimmed(sequencer.join("abort-safety"))?;
        let head = read_trimmed(sequencer.join("head"))?;
        let todo = read_trimmed(sequencer.join("todo"))?;
        let remaining: Vec<CommitOneLine> = todo
            .lines()
            .filter_map(|line| {
                let line = line.strip_prefix("pick ").unwrap_or(line);
                let (sha, summary) = line.trim().split_once(' ')?;
                Some(CommitOneLine {
                    sha: sha.to_string(),
                    summary: summary.to_string(),
                })
            })
            .collect();
        (!remaining.is_empty()).then_some((abort_safety, head, remaining))
    })();
    match parsed {
        Some((abort_safety, head, remaining)) => {
            let picked = if abort_safety != head {
                commits_between(git, workdir, &head, &abort_safety)
                    .ok()
                    .flatten()?
            } else {
                Vec::new()
            };
            let mut commits = picked.clone();
            commits.extend(remaining.iter().cloned());
            let position = picked.len() + 1;
            Some(CherryPickSnapshot {
                progress: McoProgress {
                    value: format_rebase_value(position as f32 / commits.len().max(1) as f32),
                    position,
                    total: commits.len(),
                    current_summary: remaining[0].summary.clone(),
                },
                remaining,
                commits,
                target_branch_undo_sha: head,
                cherry_picked_count: picked.len(),
            })
        }
        None => {
            // a single-commit cherry-pick does not use the sequencer files
            let sha = read_trimmed(dir.join("CHERRY_PICK_HEAD"))?;
            let summary = GitCommand::new(git)
                .args(["log", "-1", "--format=%s", &sha, "--"])
                .current_dir(workdir)
                .run()
                .ok()
                .and_then(|o| o.stdout_string().ok())
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            let commit = CommitOneLine {
                sha,
                summary: summary.clone(),
            };
            Some(CherryPickSnapshot {
                progress: McoProgress {
                    value: 1.,
                    position: 1,
                    total: 1,
                    current_summary: summary,
                },
                remaining: Vec::new(),
                commits: vec![commit],
                target_branch_undo_sha: String::new(),
                cherry_picked_count: 0,
            })
        }
    }
}

/// GHD `continueCherryPick`. With `keep_messages` and a stopped pick started
/// that way (its `MERGE_MSG` has the cut line), the pick is committed here
/// with `--cleanup=scissors` and the sequencer, if any, continued after it:
/// `cherry-pick --continue` itself commits a single pick with `strip`,
/// dropping `#` lines, and `commit --no-edit` keeps git's conflict note.
pub fn continue_cherry_pick(
    git: Arc<GitBinary>,
    workdir: &Path,
    files: &[WorkingDirectoryFileChange],
    resolutions: &BTreeMap<String, ManualConflictResolution>,
    keep_messages: bool,
    mut on_progress: impl FnMut(McoProgress),
) -> Result<CherryPickResult> {
    stage_for_continue(git.clone(), workdir, files, resolutions)?;
    if !cherry_pick_head_found(workdir) {
        return Ok(CherryPickResult::UnableToStart);
    }
    let status = crate::status::get_status(git.clone(), workdir)?;
    let (commits, mut count) = match cherry_pick_snapshot(git.clone(), workdir) {
        Some(s) => (s.commits, s.cherry_picked_count),
        None => return Ok(CherryPickResult::UnableToStart),
    };
    let tracked_after = status
        .files
        .iter()
        .filter(|f| f.status.kind != FileStatusKind::Untracked)
        .count();
    let dir = git_dir(workdir);
    if keep_messages && message_has_scissors(&dir.join("MERGE_MSG")) {
        // the editor "runs" (`:`), so `scissors` cuts at the line
        let committed = GitCommand::new(git.clone())
            .args(["commit", "--allow-empty", "--cleanup=scissors"])
            .env("GIT_EDITOR", ":")
            .current_dir(workdir)
            .run();
        match committed {
            Ok(out) => {
                for line in out.stdout_string().unwrap_or_default().lines() {
                    if let Some(p) = parse_cherry_pick_progress(line, &commits, &mut count) {
                        on_progress(p);
                    }
                }
            }
            Err(GitError::Failed { stderr, .. }) => return Ok(CherryPickResult::Error(stderr)),
            Err(err) => return Err(err),
        }
        if !dir.join("sequencer").join("todo").exists() {
            return Ok(CherryPickResult::CompletedWithoutError);
        }
        let result = GitCommand::new(git)
            .args(["cherry-pick", "--continue"])
            .env("GIT_EDITOR", ":")
            .current_dir(workdir)
            .run_streaming_stdout(|line| {
                if let Some(p) = parse_cherry_pick_progress(line, &commits, &mut count) {
                    on_progress(p);
                }
            });
        return Ok(classify_cherry_pick(workdir, result));
    }
    let result = if tracked_after == 0 {
        warn!("no tracked changes to commit; continuing cherry-pick with an empty commit");
        GitCommand::new(git)
            .args(["commit", "--allow-empty", "--no-edit"])
            .env("GIT_EDITOR", ":")
            .current_dir(workdir)
            .run()
    } else {
        GitCommand::new(git)
            .args(["cherry-pick", "--continue"])
            .env("GIT_EDITOR", ":")
            .current_dir(workdir)
            .run_streaming_stdout(|line| {
                if let Some(p) = parse_cherry_pick_progress(line, &commits, &mut count) {
                    on_progress(p);
                }
            })
    };
    Ok(classify_cherry_pick(workdir, result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn squash_continue_editor_writes_only_squash_messages() {
        let editor = squash_continue_editor(Path::new("/tmp/msg")).unwrap();
        assert!(editor.contains("cat \"/tmp/msg\" > \"$1\""));
        assert!(editor.contains("This is a combination of"));
        assert_eq!(squash_continue_editor(Path::new("/tmp/a\"b")), None);
    }

    #[test]
    fn reword_todo_rewords_one_commit() {
        let commits = vec![c("a1", "First"), c("b2", "Second"), c("c3", "Third")];
        assert_eq!(
            reword_todo(&commits, "b2").as_deref(),
            Some("pick a1 First\nreword b2 Second\npick c3 Third\n")
        );
        assert_eq!(reword_todo(&commits, "zz"), None);
    }

    fn refs(pairs: &[(&str, &[&str])]) -> UpdateRefs {
        pairs
            .iter()
            .map(|(sha, names)| {
                (
                    sha.to_string(),
                    names.iter().map(|n| n.to_string()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn update_refs_follow_picks_and_rewords() {
        let todo = "pick a1 A\nreword b2 B\npick c3 C\n";
        assert_eq!(with_update_refs(todo, &UpdateRefs::new()), todo);
        assert_eq!(
            with_update_refs(
                todo,
                &refs(&[
                    ("a1", &["refs/heads/a"]),
                    ("b2", &["refs/heads/b", "refs/heads/b2"]),
                    ("zz", &["refs/heads/gone"]),
                ])
            ),
            "pick a1 A\nupdate-ref refs/heads/a\nreword b2 B\nupdate-ref refs/heads/b\n\
             update-ref refs/heads/b2\npick c3 C\n"
        );
    }

    #[test]
    fn update_refs_go_after_the_whole_squash_group() {
        // squash "C" onto "A" (`squash_todo` moves it up), branches on all three
        let commits = vec![c("o0", "O"), c("a1", "A"), c("b2", "B"), c("c3", "C")];
        let to_squash: HashSet<String> = ["c3".to_string()].into();
        let todo = squash_todo(&commits, &to_squash, "a1").unwrap();
        assert_eq!(todo, "pick o0 O\npick a1 A\nsquash c3 C\npick b2 B\n");
        assert_eq!(
            with_update_refs(
                &todo,
                &refs(&[
                    ("a1", &["refs/heads/a"]),
                    ("b2", &["refs/heads/b"]),
                    ("c3", &["refs/heads/c"]),
                ])
            ),
            "pick o0 O\npick a1 A\nsquash c3 C\nupdate-ref refs/heads/a\n\
             update-ref refs/heads/c\npick b2 B\nupdate-ref refs/heads/b\n"
        );
        // a fixup group at the end of the list
        let pairs = vec![("c3".to_string(), "b2".to_string())];
        let todo = fixup_todo(&commits, &pairs).unwrap();
        assert_eq!(
            with_update_refs(&todo, &refs(&[("c3", &["refs/heads/c"])])),
            "pick o0 O\npick a1 A\npick b2 B\nfixup c3 C\nupdate-ref refs/heads/c\n"
        );
    }

    #[test]
    fn update_refs_follow_reordered_commits() {
        let commits = vec![c("a1", "A"), c("b2", "B"), c("c3", "C")];
        let to_move: HashSet<String> = ["a1".to_string()].into();
        let todo = reorder_todo(&commits, &to_move, None).unwrap();
        assert_eq!(
            with_update_refs(
                &todo,
                &refs(&[("a1", &["refs/heads/a"]), ("b2", &["refs/heads/b"])])
            ),
            "pick b2 B\nupdate-ref refs/heads/b\npick c3 C\npick a1 A\nupdate-ref refs/heads/a\n"
        );
    }

    #[test]
    fn merge_tree_names_skip_the_tree() {
        assert_eq!(
            parse_merge_tree_names(b"abc123\0a.txt\0dir/b.txt\0"),
            vec!["a.txt".to_string(), "dir/b.txt".to_string()]
        );
        assert!(parse_merge_tree_names(b"abc123\n").is_empty());
        assert!(parse_merge_tree_names(b"abc123\0").is_empty());
    }

    fn c(sha: &str, summary: &str) -> CommitOneLine {
        CommitOneLine {
            sha: sha.into(),
            summary: summary.into(),
        }
    }

    #[test]
    fn squash_todo_replays_in_log_order() {
        // oldest → newest: A B C D E; squash A and E onto C → B, A-C-E, D
        let commits = [
            c("A", "a"),
            c("B", "b"),
            c("C", "c"),
            c("D", "d"),
            c("E", "e"),
        ];
        let to_squash: HashSet<String> = ["A".to_string(), "E".to_string()].into();
        let todo = squash_todo(&commits, &to_squash, "C").unwrap();
        assert_eq!(
            todo,
            "pick B b\npick A a\nsquash C c\nsquash E e\npick D d\n"
        );
        assert!(squash_todo(&commits, &to_squash, "Z").is_none());
    }

    #[test]
    fn autosquash_pairs_match_like_git() {
        let c = |sha: &str, summary: &str| CommitOneLine {
            sha: sha.into(),
            summary: summary.into(),
        };
        let commits = vec![
            c("aaaa1111", "Add parser"),
            c("bbbb2222", "Add parser tests"),
            c("cccc3333", "fixup! Add parser"),
            c("dddd4444", "fixup! fixup! Add parser"),
            c("eeee5555", "fixup! bbbb"),
            c("ffff6666", "fixup! Add pars"),
            c("9999aaaa", "fixup! Nothing like this"),
        ];
        assert_eq!(
            autosquash_pairs(&commits),
            vec![
                ("cccc3333".to_string(), "aaaa1111".to_string()),
                ("dddd4444".to_string(), "aaaa1111".to_string()),
                ("eeee5555".to_string(), "bbbb2222".to_string()),
                ("ffff6666".to_string(), "aaaa1111".to_string()),
            ]
        );
    }

    #[test]
    fn fixup_todo_folds_fixups_into_their_targets() {
        let c = |sha: &str, summary: &str| CommitOneLine {
            sha: sha.into(),
            summary: summary.into(),
        };
        let commits = vec![
            c("a", "A"),
            c("b", "B"),
            c("c", "fixup! A"),
            c("d", "C"),
            c("e", "fixup! A"),
        ];
        let pairs = vec![
            ("c".to_string(), "a".to_string()),
            ("e".to_string(), "a".to_string()),
        ];
        assert_eq!(
            fixup_todo(&commits, &pairs).as_deref(),
            Some("pick a A\nfixup c fixup! A\nfixup e fixup! A\npick b B\npick d C\n")
        );
        assert_eq!(fixup_todo(&commits, &[("a".into(), "c".into())]), None);
        assert_eq!(fixup_todo(&commits, &[("x".into(), "a".into())]), None);
    }

    #[test]
    fn reorder_todo_moves_before_base_or_to_tip() {
        let commits = [
            c("A", "a"),
            c("B", "b"),
            c("C", "c"),
            c("D", "d"),
            c("E", "e"),
        ];
        let to_move: HashSet<String> = ["A".to_string(), "E".to_string()].into();
        assert_eq!(
            reorder_todo(&commits, &to_move, Some("C")).unwrap(),
            "pick B b\npick A a\npick E e\npick C c\npick D d\n"
        );
        assert_eq!(
            reorder_todo(&commits, &to_move, None).unwrap(),
            "pick B b\npick C c\npick D d\npick A a\npick E e\n"
        );
        assert!(reorder_todo(&commits, &to_move, Some("Z")).is_none());
    }

    #[test]
    fn parses_progress_lines() {
        let commits = [c("A", "first"), c("B", "second")];
        let p = parse_rebase_progress("Rebasing (2/2)", &commits).unwrap();
        assert_eq!((p.position, p.total, p.value), (2, 2, 1.0));
        assert_eq!(p.current_summary, "second");
        assert!(parse_rebase_progress("Successfully rebased", &commits).is_none());
        let mut count = 0;
        let p = parse_cherry_pick_progress("[main 1234abc] first", &commits, &mut count).unwrap();
        assert_eq!((p.position, p.total), (1, 2));
        assert!(parse_cherry_pick_progress(" Date: x", &commits, &mut count).is_none());
    }

    #[test]
    fn parses_conflict_markers() {
        let out = "a.txt:3: leftover conflict marker\na.txt:5: leftover conflict marker\nb.txt:1: trailing whitespace.\n";
        let counts = parse_conflict_markers(out);
        assert_eq!(counts.get("a.txt"), Some(&2));
        assert_eq!(counts.get("b.txt"), None);
    }

    fn repo() -> (tempfile::TempDir, Arc<GitBinary>) {
        let dir = tempfile::tempdir().unwrap();
        run(dir.path(), &["init", "-q", "-b", "main"]);
        run(dir.path(), &["config", "commit.gpgsign", "false"]);
        // Git for Windows checks text out with CRLF unless told not to
        run(dir.path(), &["config", "core.autocrlf", "false"]);
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

    fn commit_file(path: &Path, name: &str, contents: &str, message: &str) {
        std::fs::write(path.join(name), contents).unwrap();
        run(path, &["add", "."]);
        run(path, &["commit", "-q", "-m", message]);
    }

    #[test]
    fn binary_paths_reads_a_renamed_binary_files_new_path() {
        let (dir, git) = repo();
        let path = dir.path();
        let blob = |seed: u8| -> Vec<u8> { (0..4096u32).map(|i| (i % 251) as u8 ^ seed).collect() };
        std::fs::write(path.join("old.bin"), blob(0)).unwrap();
        std::fs::write(path.join("z.bin"), blob(1)).unwrap();
        run(path, &["add", "."]);
        run(path, &["commit", "-q", "-m", "binaries"]);
        run(path, &["mv", "old.bin", "new.bin"]);
        let mut renamed = blob(0);
        renamed[0] = 0xff;
        std::fs::write(path.join("new.bin"), renamed).unwrap();
        run(path, &["add", "new.bin"]);
        std::fs::write(path.join("z.bin"), blob(2)).unwrap();
        assert_eq!(
            binary_paths(git.clone(), path, "HEAD", &[]).unwrap(),
            ["new.bin", "z.bin"]
        );
        // `merge=binary` adds conflicted text files, each path once
        std::fs::write(path.join(".gitattributes"), "*.txt merge=binary\n").unwrap();
        assert_eq!(
            binary_paths(git, path, "HEAD", &["a.txt".into(), "z.bin".into()]).unwrap(),
            ["new.bin", "z.bin", "a.txt"]
        );
    }

    #[test]
    fn rebase_completes_and_reports_up_to_date() {
        let (dir, git) = repo();
        let path = dir.path();
        run(path, &["checkout", "-q", "-b", "feature"]);
        commit_file(path, "f.txt", "f\n", "feature work");
        run(path, &["checkout", "-q", "main"]);
        commit_file(path, "m.txt", "m\n", "main work");
        run(path, &["checkout", "-q", "feature"]);
        let commits = commits_between(git.clone(), path, "main", "feature")
            .unwrap()
            .unwrap();
        assert_eq!(commits.len(), 1);
        let mut seen = Vec::new();
        let result = rebase(git.clone(), path, "main", "feature", &commits, false, |p| {
            seen.push(p)
        });
        assert_eq!(result, RebaseResult::CompletedWithoutError);
        assert!(!seen.is_empty(), "progress lines were parsed");
        assert!(path.join("m.txt").exists());
        assert_eq!(
            rebase(git, path, "main", "feature", &commits, false, |_| {}),
            RebaseResult::AlreadyUpToDate
        );
    }

    #[test]
    fn rebase_conflict_state_continue_and_abort() {
        let (dir, git) = repo();
        let path = dir.path();
        run(path, &["checkout", "-q", "-b", "feature"]);
        commit_file(path, "a.txt", "feature\n", "feature change");
        run(path, &["checkout", "-q", "main"]);
        commit_file(path, "a.txt", "main\n", "main change");
        run(path, &["checkout", "-q", "feature"]);
        let result = rebase(git.clone(), path, "main", "feature", &[], false, |_| {});
        assert_eq!(result, RebaseResult::ConflictsEncountered);
        let state = rebase_internal_state(path).unwrap();
        assert_eq!(state.target_branch, "feature");
        assert_eq!(
            branch_at(
                git.clone(),
                path,
                &state.base_branch_tip,
                Some("refs/heads/feature")
            ),
            Some("main".to_string())
        );
        let status = crate::status::get_status(git.clone(), path).unwrap();
        let conflicted = status
            .files
            .iter()
            .find(|f| f.path == "a.txt")
            .expect("conflicted file");
        assert_eq!(conflicted.status.kind, FileStatusKind::Conflicted);
        assert!(conflicted.status.conflict_markers.unwrap_or(0) > 0);
        assert!(status.rebase_internal_state.is_some());
        // resolve by taking our side (the base branch during a rebase)
        let mut resolutions = BTreeMap::new();
        resolutions.insert("a.txt".to_string(), ManualConflictResolution::Theirs);
        let result = continue_rebase(
            git.clone(),
            path,
            &status.files,
            &resolutions,
            &[],
            false,
            None,
            |_| {},
        )
        .unwrap();
        assert_eq!(result, RebaseResult::CompletedWithoutError);
        assert!(!rebase_head_set(path));
        assert_eq!(
            std::fs::read_to_string(path.join("a.txt")).unwrap(),
            "feature\n"
        );
        // abort path
        run(path, &["checkout", "-q", "main"]);
        run(path, &["checkout", "-q", "-b", "other", "HEAD~1"]);
        commit_file(path, "a.txt", "other\n", "other change");
        assert_eq!(
            rebase(git.clone(), path, "main", "other", &[], false, |_| {}),
            RebaseResult::ConflictsEncountered
        );
        abort_rebase(git, path).unwrap();
        assert!(!rebase_head_set(path));
        assert_eq!(
            std::fs::read_to_string(path.join("a.txt")).unwrap(),
            "other\n"
        );
    }

    #[test]
    fn rebase_keeps_hash_messages_across_a_conflict() {
        let (dir, git) = repo();
        let path = dir.path();
        run(path, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(path.join("a.txt"), "feature\n").unwrap();
        run(path, &["add", "."]);
        run(
            path,
            &["commit", "-q", "-m", "#12 feature", "-m", "# kept too"],
        );
        run(path, &["checkout", "-q", "main"]);
        commit_file(path, "a.txt", "main\n", "main change");
        run(path, &["checkout", "-q", "feature"]);
        assert_eq!(
            rebase(git.clone(), path, "main", "feature", &[], true, |_| {}),
            RebaseResult::ConflictsEncountered
        );
        let status = crate::status::get_status(git.clone(), path).unwrap();
        let mut resolutions = BTreeMap::new();
        resolutions.insert("a.txt".to_string(), ManualConflictResolution::Theirs);
        let result = continue_rebase(
            git.clone(),
            path,
            &status.files,
            &resolutions,
            &[],
            true,
            None,
            |_| {},
        )
        .unwrap();
        assert_eq!(result, RebaseResult::CompletedWithoutError);
        let out = GitCommand::new(git)
            .args(["log", "-1", "--format=%B"])
            .current_dir(path)
            .run()
            .unwrap()
            .stdout_string()
            .unwrap();
        assert_eq!(out.trim_end(), "#12 feature\n\n# kept too");
    }

    #[test]
    fn squash_and_reorder_rewrite_history() {
        let (dir, git) = repo();
        let path = dir.path();
        commit_file(path, "b.txt", "b\n", "second");
        commit_file(path, "c.txt", "c\n", "third");
        let all = commits_in_range(git.clone(), path, "HEAD")
            .unwrap()
            .unwrap();
        assert_eq!(all.len(), 3);
        let identity = corvene_models::CommitIdentity {
            name: "T".into(),
            email: "t@example.com".into(),
            seconds: 0,
            offset: 0,
        };
        let full = |one: &CommitOneLine| Commit {
            sha: one.sha.clone(),
            summary: one.summary.clone(),
            body: String::new(),
            author: identity.clone(),
            committer: identity.clone(),
            parents: Vec::new(),
            trailers: Vec::new(),
            tags: Vec::new(),
            signature: None,
        };
        // squash "third" onto "second", keeping "first"
        let result = squash(
            git.clone(),
            path,
            &[full(&all[2])],
            &full(&all[1]),
            Some(&all[0].sha),
            "combined\n\nsecond + third",
            RebaseOptions::default(),
            |_| {},
        );
        assert_eq!(result, RebaseResult::CompletedWithoutError);
        let after = commits_in_range(git.clone(), path, "HEAD")
            .unwrap()
            .unwrap();
        assert_eq!(after.len(), 2);
        assert_eq!(after[1].summary, "combined");
        assert!(path.join("c.txt").exists());
        // reorder: move "first" after "combined" is impossible (root); move combined before first → same order
        commit_file(path, "d.txt", "d\n", "fourth");
        let all = commits_in_range(git.clone(), path, "HEAD")
            .unwrap()
            .unwrap();
        let result = reorder(
            git.clone(),
            path,
            &[full(&all[2])],
            Some(&full(&all[1])),
            Some(&all[0].sha),
            RebaseOptions::default(),
            |_| {},
        );
        assert_eq!(result, RebaseResult::CompletedWithoutError);
        let after = commits_in_range(git, path, "HEAD").unwrap().unwrap();
        let summaries: Vec<&str> = after.iter().map(|c| c.summary.as_str()).collect();
        assert_eq!(summaries, ["first", "fourth", "combined"]);
    }

    #[test]
    fn squash_moves_stacked_branches_with_update_refs() {
        let (dir, git) = repo();
        let path = dir.path();
        // main → A (branch a) → B (branch b, checked out)
        run(path, &["checkout", "-q", "-b", "a"]);
        commit_file(path, "b.txt", "b\n", "A");
        run(path, &["checkout", "-q", "-b", "b"]);
        commit_file(path, "c.txt", "c\n", "B");
        let all = commits_in_range(git.clone(), path, "main..b")
            .unwrap()
            .unwrap();
        let identity = corvene_models::CommitIdentity {
            name: "T".into(),
            email: "t@example.com".into(),
            seconds: 0,
            offset: 0,
        };
        let full = |one: &CommitOneLine| Commit {
            sha: one.sha.clone(),
            summary: one.summary.clone(),
            body: String::new(),
            author: identity.clone(),
            committer: identity.clone(),
            parents: Vec::new(),
            trailers: Vec::new(),
            tags: Vec::new(),
            signature: None,
        };
        let rev = |r: &str| {
            GitCommand::new(git.clone())
                .args(["rev-parse", r])
                .current_dir(path)
                .run()
                .unwrap()
                .stdout_string()
                .unwrap()
                .trim()
                .to_string()
        };
        let old_a = rev("a");
        // squash B onto A, moving `a`
        let result = squash(
            git.clone(),
            path,
            &[full(&all[1])],
            &full(&all[0]),
            Some("main"),
            "A and B",
            RebaseOptions {
                update_refs: refs(&[(&all[0].sha, &["refs/heads/a"])]),
                ..RebaseOptions::default()
            },
            |_| {},
        );
        assert_eq!(result, RebaseResult::CompletedWithoutError);
        assert_eq!(rev("a"), rev("b"));
        assert_eq!(rev("b~1"), rev("main"));
        // Undo puts it back
        restore_branch_tips(git.clone(), path, &[("refs/heads/a".into(), old_a.clone())]).unwrap();
        assert_eq!(rev("a"), old_a);
    }

    /// desktop/desktop#18331: base, then A adds `f`, B runs `git rm --cached
    /// f` and ignores it, C changes `g`. `f` has local work on disk.
    fn stopped_tracking_repo(ignored: bool) -> (tempfile::TempDir, Arc<GitBinary>, Vec<Commit>) {
        let (dir, git) = repo();
        let path = dir.path();
        std::fs::write(path.join("z"), "base\n").unwrap();
        run(path, &["add", "."]);
        run(path, &["commit", "-qm", "base"]);
        std::fs::write(path.join("f"), "v1\n").unwrap();
        std::fs::write(path.join("g"), "x\n").unwrap();
        run(path, &["add", "."]);
        run(path, &["commit", "-qm", "A"]);
        run(path, &["rm", "-q", "--cached", "f"]);
        if ignored {
            std::fs::write(path.join(".gitignore"), "f\n").unwrap();
            run(path, &["add", ".gitignore"]);
        }
        run(path, &["commit", "-qm", "B"]);
        std::fs::write(path.join("g"), "y\n").unwrap();
        run(path, &["add", "g"]);
        run(path, &["commit", "-qm", "C"]);
        std::fs::write(path.join("f"), "local work\n").unwrap();
        let all = commits_in_range(git.clone(), path, "HEAD~3..HEAD")
            .unwrap()
            .unwrap();
        let identity = corvene_models::CommitIdentity {
            name: "t".into(),
            email: "t@t".into(),
            seconds: 0,
            offset: 0,
        };
        let commits = all
            .iter()
            .map(|one| Commit {
                sha: one.sha.clone(),
                summary: one.summary.clone(),
                body: String::new(),
                author: identity.clone(),
                committer: identity.clone(),
                parents: Vec::new(),
                trailers: Vec::new(),
                tags: Vec::new(),
                signature: None,
            })
            .collect();
        (dir, git, commits)
    }

    fn squash_stopped_tracking(
        git: Arc<GitBinary>,
        path: &Path,
        commits: &[Commit],
        keep_untracked: bool,
    ) -> RebaseResult {
        squash(
            git,
            path,
            &commits[1..],
            &commits[0],
            Some("HEAD~3"),
            "All",
            RebaseOptions {
                keep_untracked,
                ..RebaseOptions::default()
            },
            |_| {},
        )
    }

    #[test]
    fn squash_keeps_a_file_that_stopped_being_tracked() {
        for ignored in [true, false] {
            let (dir, git, commits) = stopped_tracking_repo(ignored);
            let path = dir.path();
            let result = squash_stopped_tracking(git.clone(), path, &commits, true);
            assert_eq!(result, RebaseResult::CompletedWithoutError);
            assert_eq!(
                std::fs::read_to_string(path.join("f")).unwrap(),
                "local work\n"
            );
            assert!(!git_dir(path).join("corvene-set-aside").exists());
            let tracked = GitCommand::new(git)
                .args(["ls-files", "--", "f"])
                .current_dir(path)
                .run()
                .unwrap();
            assert!(tracked.stdout.is_empty());
        }
    }

    #[test]
    fn plain_squash_loses_a_file_that_stopped_being_tracked() {
        // why `keep_untracked` exists: git's own replay deletes it
        let (dir, git, commits) = stopped_tracking_repo(true);
        let path = dir.path();
        let result = squash_stopped_tracking(git, path, &commits, false);
        assert_eq!(result, RebaseResult::CompletedWithoutError);
        assert!(!path.join("f").exists());
    }

    #[test]
    fn set_aside_files_wait_for_a_stopped_rebase() {
        let (dir, git, _) = stopped_tracking_repo(true);
        let path = dir.path();
        // a `rebase-merge` folder stands in for a stop on a conflict
        crate::rewrite_untracked::set_aside_for_rewrite(git.clone(), path, Some("HEAD~3")).unwrap();
        assert!(!path.join("f").exists());
        let stopped = git_dir(path).join("rebase-merge");
        std::fs::create_dir(&stopped).unwrap();
        crate::rewrite_untracked::restore_set_aside(path).unwrap();
        assert!(!path.join("f").exists());
        std::fs::remove_dir(&stopped).unwrap();
        crate::rewrite_untracked::restore_set_aside(path).unwrap();
        assert_eq!(
            std::fs::read_to_string(path.join("f")).unwrap(),
            "local work\n"
        );
    }

    #[test]
    fn a_restored_file_never_overwrites_what_the_history_tracks() {
        let (dir, git, _) = stopped_tracking_repo(true);
        let path = dir.path();
        crate::rewrite_untracked::set_aside_for_rewrite(git, path, Some("HEAD~3")).unwrap();
        std::fs::write(path.join("f"), "from the rewrite\n").unwrap();
        crate::rewrite_untracked::restore_set_aside(path).unwrap();
        assert_eq!(
            std::fs::read_to_string(path.join("f")).unwrap(),
            "from the rewrite\n"
        );
        assert_eq!(
            std::fs::read_to_string(path.join("f.orig")).unwrap(),
            "local work\n"
        );
    }

    #[test]
    fn cherry_pick_copies_and_conflicts() {
        let (dir, git) = repo();
        let path = dir.path();
        run(path, &["checkout", "-q", "-b", "feature"]);
        commit_file(path, "f.txt", "f\n", "feature commit");
        let feature = commits_in_range(git.clone(), path, "main..feature")
            .unwrap()
            .unwrap();
        run(path, &["checkout", "-q", "main"]);
        let mut progress = Vec::new();
        assert_eq!(
            cherry_pick(git.clone(), path, &feature, false, |p| progress.push(p)),
            CherryPickResult::CompletedWithoutError
        );
        assert!(path.join("f.txt").exists());
        assert_eq!(progress.len(), 1);
        // conflicting pick
        run(path, &["checkout", "-q", "feature"]);
        commit_file(path, "a.txt", "feature\n", "feature edits a");
        let picks = commits_in_range(git.clone(), path, "HEAD~1..HEAD")
            .unwrap()
            .unwrap();
        run(path, &["checkout", "-q", "main"]);
        commit_file(path, "a.txt", "main\n", "main edits a");
        assert_eq!(
            cherry_pick(git.clone(), path, &picks, false, |_| {}),
            CherryPickResult::ConflictsEncountered
        );
        assert!(cherry_pick_head_found(path));
        let snapshot = cherry_pick_snapshot(git.clone(), path).unwrap();
        assert_eq!(snapshot.commits.len(), 1);
        assert_eq!(
            determine_mergeability(git.clone(), path, "main", "feature").unwrap(),
            Mergeability::Conflicts(1)
        );
        abort_cherry_pick(git, path).unwrap();
        assert!(!cherry_pick_head_found(path));
    }

    #[test]
    fn cherry_pick_keeps_hash_messages_across_a_conflict() {
        let (dir, git) = repo();
        let path = dir.path();
        run(path, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(path.join("a.txt"), "feature\n").unwrap();
        run(path, &["add", "."]);
        run(path, &["commit", "-q", "-m", "#7 edits a", "-m", "# body"]);
        commit_file(path, "b.txt", "b\n", "#8 adds b");
        let picks = commits_in_range(git.clone(), path, "main..feature")
            .unwrap()
            .unwrap();
        run(path, &["checkout", "-q", "main"]);
        commit_file(path, "a.txt", "main\n", "main edits a");
        let log = |n: &str| {
            GitCommand::new(git.clone())
                .args(["log", n, "--format=%B"])
                .current_dir(path)
                .run()
                .unwrap()
                .stdout_string()
                .unwrap()
        };
        let resolve = |side: ManualConflictResolution| {
            let status = crate::status::get_status(git.clone(), path).unwrap();
            let mut resolutions = BTreeMap::new();
            resolutions.insert("a.txt".to_string(), side);
            continue_cherry_pick(git.clone(), path, &status.files, &resolutions, true, |_| {})
                .unwrap()
        };
        // two picks, the first conflicts: the sequencer goes on afterwards
        assert_eq!(
            cherry_pick(git.clone(), path, &picks, true, |_| {}),
            CherryPickResult::ConflictsEncountered
        );
        assert_eq!(
            resolve(ManualConflictResolution::Theirs),
            CherryPickResult::CompletedWithoutError
        );
        assert!(!cherry_pick_head_found(path));
        assert_eq!(log("-2"), "#8 adds b\n\n#7 edits a\n\n# body\n\n");
        // one pick resolved to our side: an empty commit, same message
        run(path, &["reset", "-q", "--hard", "HEAD~2"]);
        assert_eq!(
            cherry_pick(git.clone(), path, &picks[..1], true, |_| {}),
            CherryPickResult::ConflictsEncountered
        );
        assert_eq!(
            resolve(ManualConflictResolution::Ours),
            CherryPickResult::CompletedWithoutError
        );
        assert_eq!(log("-1"), "#7 edits a\n\n# body\n\n");
    }

    #[test]
    fn squash_with_autostash_keeps_local_changes() {
        let (dir, git) = repo();
        let path = dir.path();
        commit_file(path, "b.txt", "b\n", "second");
        commit_file(path, "c.txt", "c\n", "third");
        let all = commits_in_range(git.clone(), path, "HEAD")
            .unwrap()
            .unwrap();
        let identity = corvene_models::CommitIdentity {
            name: "T".into(),
            email: "t@example.com".into(),
            seconds: 0,
            offset: 0,
        };
        let full = |one: &CommitOneLine| Commit {
            sha: one.sha.clone(),
            summary: one.summary.clone(),
            body: String::new(),
            author: identity.clone(),
            committer: identity.clone(),
            parents: Vec::new(),
            trailers: Vec::new(),
            tags: Vec::new(),
            signature: None,
        };
        std::fs::write(path.join("a.txt"), "local\n").unwrap();
        let result = squash(
            git.clone(),
            path,
            &[full(&all[2])],
            &full(&all[1]),
            Some(&all[0].sha),
            "combined",
            RebaseOptions {
                autostash: true,
                ..RebaseOptions::default()
            },
            |_| {},
        );
        assert_eq!(result, RebaseResult::CompletedWithoutError);
        assert_eq!(
            std::fs::read_to_string(path.join("a.txt")).unwrap(),
            "local\n"
        );
        assert_eq!(stash_tip(git.clone(), path), None);
        assert_eq!(
            commits_in_range(git, path, "HEAD").unwrap().unwrap().len(),
            2
        );
    }

    #[test]
    fn squash_merge_uses_the_given_message() {
        let (dir, git) = repo();
        let path = dir.path();
        run(path, &["checkout", "-q", "-b", "feature"]);
        commit_file(path, "f.txt", "f\n", "feature one");
        commit_file(path, "a.txt", "feature\n", "feature two");
        run(path, &["checkout", "-q", "main"]);
        let log = || {
            GitCommand::new(git.clone())
                .args(["log", "-1", "--format=%B"])
                .current_dir(path)
                .run()
                .unwrap()
                .stdout_string()
                .unwrap()
        };
        assert_eq!(
            crate::merge_branch_with_message(
                git.clone(),
                path,
                "feature",
                true,
                Some("Feature\n\nall of it")
            )
            .unwrap(),
            crate::MergeOutcome::Success
        );
        assert_eq!(log().trim_end(), "Feature\n\nall of it");
        // conflicting: the message waits in SQUASH_MSG for the commit
        run(path, &["reset", "-q", "--hard", "HEAD~1"]);
        commit_file(path, "a.txt", "main\n", "main edits a");
        assert_eq!(
            crate::merge_branch_with_message(git.clone(), path, "feature", true, Some("Squashed"))
                .unwrap(),
            crate::MergeOutcome::Conflicts
        );
        let status = crate::status::get_status(git.clone(), path).unwrap();
        let conflicted: Vec<_> = status
            .files
            .iter()
            .filter(|f| f.status.kind == FileStatusKind::Conflicted)
            .cloned()
            .collect();
        let mut resolutions = BTreeMap::new();
        resolutions.insert("a.txt".to_string(), ManualConflictResolution::Theirs);
        create_merge_commit(git.clone(), path, &conflicted, &resolutions).unwrap();
        assert_eq!(log().trim_end(), "Squashed");
    }

    #[test]
    fn merge_conflict_commit_and_squash_abort() {
        let (dir, git) = repo();
        let path = dir.path();
        run(path, &["checkout", "-q", "-b", "feature"]);
        commit_file(path, "a.txt", "feature\n", "feature edits a");
        run(path, &["checkout", "-q", "main"]);
        commit_file(path, "a.txt", "main\n", "main edits a");
        assert_eq!(
            crate::merge_branch(git.clone(), path, "feature", false).unwrap(),
            crate::MergeOutcome::Conflicts
        );
        assert!(merge_head_set(path));
        let status = crate::status::get_status(git.clone(), path).unwrap();
        let conflicted: Vec<_> = status
            .files
            .iter()
            .filter(|f| f.status.kind == FileStatusKind::Conflicted)
            .cloned()
            .collect();
        assert_eq!(conflicted.len(), 1);
        let mut resolutions = BTreeMap::new();
        resolutions.insert("a.txt".to_string(), ManualConflictResolution::Ours);
        let sha = create_merge_commit(git.clone(), path, &conflicted, &resolutions).unwrap();
        // git's abbreviated sha (GHD `parseCommitSHA`)
        assert!(
            crate::head_sha(git.clone(), path)
                .unwrap()
                .starts_with(&sha)
        );
        assert!(sha.len() >= 7);
        assert!(!merge_head_set(path));
        assert_eq!(
            std::fs::read_to_string(path.join("a.txt")).unwrap(),
            "main\n"
        );
        // squash merge that conflicts, then abort it
        run(path, &["checkout", "-q", "-b", "other", "HEAD~1"]);
        commit_file(path, "a.txt", "other\n", "other edits a");
        let tip = crate::head_sha(git.clone(), path).unwrap();
        assert_eq!(
            crate::merge_branch(git.clone(), path, "feature", true).unwrap(),
            crate::MergeOutcome::Conflicts
        );
        assert!(squash_msg_set(path));
        abort_squash_merge(git.clone(), path, &tip).unwrap();
        assert!(!squash_msg_set(path));
        assert_eq!(crate::head_sha(git, path).unwrap(), tip);
    }
}
