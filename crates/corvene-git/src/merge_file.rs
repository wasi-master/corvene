//! Corvene `1361-merge-editor`: what the merge editor reads from git for a
//! conflicted file and how its result goes back.
//!
//! GitHub Desktop resolves conflicts outside the app and only counts the
//! markers left (`app/src/lib/git/diff-check.ts`); there is no GHD source
//! for this module.
//!
//! The three versions are the path's index stages (1 base, 2 ours, 3
//! theirs), read by object id from `ls-files -u` (not `:2:path`, which
//! gitoxide's rev-parse does not promise, and not `merge-file
//! --object-id`, which needs git 2.43). The working file is converted to
//! the stages' line ends while it is edited and back when it is written.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use corvene_models::UnmergedEntry;

use crate::GitError;
use crate::blame::{BlameCommit, BlameEvent, BlameOptions, blame};
use crate::detect::GitBinary;
use crate::error::Result;
use crate::paths::git_dir;
use crate::process::{CancelToken, GitCommand};
use crate::rebase_ops::{cherry_pick_head_found, merge_head_set, rebase_head_set, squash_msg_set};

/// git's `buffer_is_binary` looks this far for a NUL byte.
const BINARY_PROBE: usize = 8000;

/// The operation that left the conflict.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ConflictOrigin {
    #[default]
    Merge,
    Rebase,
    CherryPick,
    Revert,
    /// No operation head: a stash applied over changes, or a patch.
    StashPop,
}

/// Which operation is in progress in `workdir`.
pub fn conflict_origin(workdir: &Path) -> ConflictOrigin {
    if rebase_head_set(workdir) {
        ConflictOrigin::Rebase
    } else if cherry_pick_head_found(workdir) {
        ConflictOrigin::CherryPick
    } else if git_dir(workdir).join("REVERT_HEAD").exists() {
        ConflictOrigin::Revert
    } else if merge_head_set(workdir) || squash_msg_set(workdir) {
        ConflictOrigin::Merge
    } else {
        ConflictOrigin::StashPop
    }
}

/// The text after each conflict marker.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConflictLabels {
    pub ours: Option<String>,
    pub base: Option<String>,
    pub theirs: Option<String>,
}

/// The revisions each version comes from, for blaming a chunk.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SideRevs {
    pub base: Option<String>,
    pub ours: Option<String>,
    pub theirs: Option<String>,
}

/// The attributes that change how a path merges.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeAttrs {
    /// `conflict-marker-size`.
    pub marker_size: Option<usize>,
    /// `merge`: a driver name, `set` or `unset`.
    pub merge: Option<String>,
    pub eol: Option<String>,
    pub text: Option<String>,
    pub working_tree_encoding: Option<String>,
    pub filter: Option<String>,
}

impl MergeAttrs {
    /// `merge=binary` or `-merge`: git keeps ours and calls it a conflict.
    pub fn binary_merge(&self) -> bool {
        matches!(self.merge.as_deref(), Some("binary" | "unset"))
    }

    pub fn is_lfs(&self) -> bool {
        self.filter.as_deref() == Some("lfs")
    }
}

/// Why a conflicted file goes to the side picker rather than the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PickReason {
    /// A version holds a NUL byte or is not UTF-8.
    Binary,
    /// `merge=binary` or `-merge`.
    BinaryMergeDriver,
    /// Stored in Git LFS: the versions are pointers.
    Lfs,
    /// A `working-tree-encoding` other than UTF-8.
    Encoding,
    Submodule,
    Symlink,
    /// One side deleted the file.
    Deleted,
}

/// A file's bytes as a length and hash, to tell the editor's own writes
/// from someone else's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WriteStamp {
    pub len: u64,
    pub hash: u64,
}

impl WriteStamp {
    pub fn of(bytes: &[u8]) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut hasher);
        Self {
            len: bytes.len() as u64,
            hash: hasher.finish(),
        }
    }
}

/// The stamp of the working file now; `None` when it cannot be read.
pub fn file_stamp(workdir: &Path, path: &str) -> Option<WriteStamp> {
    std::fs::read(workdir.join(path))
        .ok()
        .map(|b| WriteStamp::of(&b))
}

/// Everything the merge editor opens a conflicted file with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeInputs {
    pub path: String,
    pub entries: Vec<UnmergedEntry>,
    /// Stage 1; `None` when both sides added the file.
    pub base: Option<String>,
    /// Stage 2; `None` when ours deleted the file.
    pub ours: Option<String>,
    /// Stage 3; `None` when theirs deleted the file.
    pub theirs: Option<String>,
    /// The working file in the stages' line ends; `None` when missing.
    pub working: Option<String>,
    /// The working file's bytes as read.
    pub working_stamp: Option<WriteStamp>,
    /// The working file uses CRLF where the stages use LF: results are
    /// written back with CRLF.
    pub crlf: bool,
    pub pick: Option<PickReason>,
    pub attrs: MergeAttrs,
    /// `merge.conflictStyle`, as configured.
    pub conflict_style: Option<String>,
    pub labels: ConflictLabels,
    pub origin: ConflictOrigin,
    pub revs: SideRevs,
}

/// `ls-files -u -z -- <path>`: the path's index stages.
pub fn unmerged_entries(
    git: Arc<GitBinary>,
    workdir: &Path,
    path: &str,
) -> Result<Vec<UnmergedEntry>> {
    let out = GitCommand::new(git)
        .args(["ls-files", "-u", "-z", "--", path])
        .env("GIT_LITERAL_PATHSPECS", "1")
        .current_dir(workdir)
        .run()?;
    Ok(crate::stash_ops::parse_unmerged_entries(&out.stdout)
        .into_iter()
        .filter(|e| e.path == path)
        .collect())
}

fn attr_value(value: &str) -> Option<String> {
    (value != "unspecified").then(|| value.to_string())
}

/// `check-attr -z` for the attributes that change a merge.
pub fn merge_attributes(git: Arc<GitBinary>, workdir: &Path, path: &str) -> Result<MergeAttrs> {
    let out = GitCommand::new(git)
        .args([
            "check-attr",
            "-z",
            "conflict-marker-size",
            "merge",
            "eol",
            "text",
            "working-tree-encoding",
            "filter",
            "--",
            path,
        ])
        .current_dir(workdir)
        .run()?;
    Ok(parse_attributes(&String::from_utf8_lossy(&out.stdout)))
}

/// `<path> NUL <attribute> NUL <value> NUL` records.
pub fn parse_attributes(text: &str) -> MergeAttrs {
    let mut attrs = MergeAttrs::default();
    let records: Vec<&str> = text.split('\0').collect();
    let (records, _) = records.as_chunks::<3>();
    for [_, attribute, value] in records {
        let value = attr_value(value);
        match *attribute {
            "conflict-marker-size" => {
                attrs.marker_size = value.and_then(|v| v.parse().ok()).filter(|&n| n > 0)
            }
            "merge" => attrs.merge = value,
            "eol" => attrs.eol = value,
            "text" => attrs.text = value,
            "working-tree-encoding" => attrs.working_tree_encoding = value,
            "filter" => attrs.filter = value,
            _ => {}
        }
    }
    attrs
}

/// `merge.conflictStyle`.
pub fn conflict_style(git: Arc<GitBinary>, workdir: &Path) -> Option<String> {
    crate::config_value(git, workdir, "merge.conflictStyle")
}

fn read_head_file(workdir: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(git_dir(workdir).join(name))
        .ok()
        .and_then(|s| s.lines().next().map(|l| l.trim().to_string()))
        .filter(|s| !s.is_empty())
}

/// The commit that is being applied: `MERGE_HEAD`, `REBASE_HEAD`,
/// `CHERRY_PICK_HEAD` or `REVERT_HEAD`.
fn operation_head(workdir: &Path, origin: ConflictOrigin) -> Option<String> {
    match origin {
        ConflictOrigin::Merge => read_head_file(workdir, "MERGE_HEAD"),
        ConflictOrigin::Rebase => read_head_file(workdir, "REBASE_HEAD"),
        ConflictOrigin::CherryPick => read_head_file(workdir, "CHERRY_PICK_HEAD"),
        ConflictOrigin::Revert => read_head_file(workdir, "REVERT_HEAD"),
        ConflictOrigin::StashPop => None,
    }
}

fn rev_exists(git: Arc<GitBinary>, workdir: &Path, rev: &str) -> bool {
    GitCommand::new(git)
        .args(["rev-parse", "--verify", "-q", &format!("{rev}^{{commit}}")])
        .current_dir(workdir)
        .allow_exit_code(1)
        .run()
        .is_ok_and(|o| o.status.success())
}

/// The revision each stage was taken from.
pub fn side_revs(git: Arc<GitBinary>, workdir: &Path, origin: ConflictOrigin) -> SideRevs {
    let head = operation_head(workdir, origin);
    let ours = Some("HEAD".to_string());
    let (base, theirs) = match (origin, head) {
        (ConflictOrigin::Merge, Some(head)) => {
            let base = GitCommand::new(git.clone())
                .args(["merge-base", "HEAD", &head])
                .current_dir(workdir)
                .allow_exit_code(1)
                .run()
                .ok()
                .and_then(|o| o.stdout_string().ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            (base, Some(head))
        }
        (ConflictOrigin::Rebase | ConflictOrigin::CherryPick, Some(head)) => {
            (Some(format!("{head}^")), Some(head))
        }
        (ConflictOrigin::Revert, Some(head)) => (Some(head.clone()), Some(format!("{head}^"))),
        (ConflictOrigin::StashPop, _) if rev_exists(git.clone(), workdir, "refs/stash") => (
            Some("refs/stash^1".to_string()),
            Some("refs/stash".to_string()),
        ),
        _ => (None, None),
    };
    SideRevs { base, ours, theirs }
}

/// `<short sha> (<subject>)`, git's label for a replayed commit.
fn commit_label(git: Arc<GitBinary>, workdir: &Path, rev: &str) -> Option<String> {
    let out = GitCommand::new(git)
        .args(["log", "-1", "--format=%h%x00%s", rev, "--"])
        .current_dir(workdir)
        .run()
        .ok()?;
    let text = out.stdout_string().ok()?;
    let (sha, subject) = text.trim_end().split_once('\0')?;
    Some(format!("{sha} ({subject})"))
}

/// The branch a merge's message names (`Merge branch 'x'`,
/// `Merge remote-tracking branch 'origin/x'`).
pub fn merge_msg_branch(message: &str) -> Option<String> {
    let line = message.lines().next()?;
    let rest = line.strip_prefix("Merge ")?;
    let start = rest.find('\'')? + 1;
    let end = start + rest[start..].find('\'')?;
    Some(rest[start..end].to_string())
}

/// Labels for conflict blocks the editor writes itself (reset to conflict
/// in a file git's markers are gone from), as git would label them.
pub fn conflict_labels(
    git: Arc<GitBinary>,
    workdir: &Path,
    origin: ConflictOrigin,
) -> ConflictLabels {
    let head = operation_head(workdir, origin);
    match origin {
        ConflictOrigin::Merge => {
            let branch = std::fs::read_to_string(git_dir(workdir).join("MERGE_MSG"))
                .ok()
                .and_then(|m| merge_msg_branch(&m));
            ConflictLabels {
                ours: Some("HEAD".into()),
                base: Some("merged common ancestors".into()),
                theirs: branch.or_else(|| head.map(|h| h[..h.len().min(7)].to_string())),
            }
        }
        ConflictOrigin::Rebase | ConflictOrigin::CherryPick => {
            let label = head.and_then(|h| commit_label(git, workdir, &h));
            ConflictLabels {
                ours: Some("HEAD".into()),
                base: label.as_ref().map(|l| format!("parent of {l}")),
                theirs: label,
            }
        }
        ConflictOrigin::Revert => {
            let label = head.and_then(|h| commit_label(git, workdir, &h));
            ConflictLabels {
                ours: Some("HEAD".into()),
                base: label.clone(),
                theirs: label.map(|l| format!("parent of {l}")),
            }
        }
        ConflictOrigin::StashPop => ConflictLabels {
            ours: Some("Updated upstream".into()),
            base: Some("Stash base".into()),
            theirs: Some("Stashed changes".into()),
        },
    }
}

/// A stage's mode and bytes, by object id; `None` when the side has no such
/// stage.
pub fn stage_bytes(
    git: Arc<GitBinary>,
    workdir: &Path,
    entries: &[UnmergedEntry],
    stage: u8,
) -> Result<Option<(String, Vec<u8>)>> {
    let Some(entry) = entries.iter().find(|e| e.stage == stage) else {
        return Ok(None);
    };
    if let Some(bytes) = crate::handle::object_bytes(workdir, &entry.oid) {
        return Ok(Some((entry.mode.clone(), bytes)));
    }
    let out = GitCommand::new(git)
        .args(["cat-file", "blob", &entry.oid])
        .current_dir(workdir)
        .run()?;
    Ok(Some((entry.mode.clone(), out.stdout)))
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(BINARY_PROBE)].contains(&0) || std::str::from_utf8(bytes).is_err()
}

fn is_crlf(text: &str) -> bool {
    let lf = text.matches('\n').count();
    lf > 0 && text.matches("\r\n").count() * 2 > lf
}

/// `\r\n` as `\n`.
pub fn to_lf(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// Every bare `\n` as `\r\n`.
pub fn to_crlf(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 32);
    let mut prev = '\0';
    for c in text.chars() {
        if c == '\n' && prev != '\r' {
            out.push('\r');
        }
        out.push(c);
        prev = c;
    }
    out
}

/// Reads a conflicted file's stages, working file, attributes and labels.
pub fn load_merge_inputs(git: Arc<GitBinary>, workdir: &Path, path: &str) -> Result<MergeInputs> {
    let entries = unmerged_entries(git.clone(), workdir, path)?;
    let attrs = merge_attributes(git.clone(), workdir, path)?;
    let origin = conflict_origin(workdir);
    let mut pick = None;
    let mut note = |reason: PickReason| {
        pick.get_or_insert(reason);
    };
    if attrs.is_lfs() {
        note(PickReason::Lfs);
    }
    if attrs.binary_merge() {
        note(PickReason::BinaryMergeDriver);
    }
    if attrs
        .working_tree_encoding
        .as_deref()
        .is_some_and(|e| !e.eq_ignore_ascii_case("utf-8") && !e.eq_ignore_ascii_case("utf8"))
    {
        note(PickReason::Encoding);
    }
    let mut sides: [Option<String>; 3] = [None, None, None];
    for (slot, stage) in sides.iter_mut().zip(1u8..=3) {
        let Some((mode, bytes)) = stage_bytes(git.clone(), workdir, &entries, stage)? else {
            continue;
        };
        match mode.as_str() {
            "160000" => note(PickReason::Submodule),
            "120000" => note(PickReason::Symlink),
            _ if is_binary(&bytes) => note(PickReason::Binary),
            _ => {}
        }
        *slot = String::from_utf8(bytes).ok();
    }
    let [base, ours, theirs] = sides;
    let has = |stage: u8| entries.iter().any(|e| e.stage == stage);
    if !has(2) || !has(3) {
        note(PickReason::Deleted);
    }
    let raw = std::fs::read(workdir.join(path)).ok();
    let working_stamp = raw.as_deref().map(WriteStamp::of);
    let working = match raw {
        Some(bytes) if !is_binary(&bytes) => String::from_utf8(bytes).ok(),
        Some(_) => {
            note(PickReason::Binary);
            None
        }
        None => None,
    };
    let stage_crlf = ours.as_deref().or(theirs.as_deref()).is_some_and(is_crlf);
    let crlf = working.as_deref().is_some_and(is_crlf) && !stage_crlf;
    let working = working.map(|w| if crlf { to_lf(&w) } else { w });
    Ok(MergeInputs {
        path: path.to_string(),
        entries,
        base,
        ours,
        theirs,
        working,
        working_stamp,
        crlf,
        pick,
        attrs,
        conflict_style: conflict_style(git.clone(), workdir),
        labels: conflict_labels(git.clone(), workdir, origin),
        origin,
        revs: side_revs(git, workdir, origin),
    })
}

/// What [`write_merge_result`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteOutcome {
    Written(WriteStamp),
    /// The file is not what the editor last read or wrote; nothing was
    /// written.
    ChangedOnDisk(Option<WriteStamp>),
}

/// Writes the editor's result over the working file, in place (its mode
/// stays), unless the file changed since `expected`.
pub fn write_merge_result(
    workdir: &Path,
    path: &str,
    text: &str,
    crlf: bool,
    expected: Option<WriteStamp>,
) -> Result<WriteOutcome> {
    let full = workdir.join(path);
    if let Some(expected) = expected {
        let now = file_stamp(workdir, path);
        if now != Some(expected) {
            return Ok(WriteOutcome::ChangedOnDisk(now));
        }
    }
    let bytes = if crlf {
        to_crlf(text).into_bytes()
    } else {
        text.as_bytes().to_vec()
    };
    std::fs::write(&full, &bytes)?;
    Ok(WriteOutcome::Written(WriteStamp::of(&bytes)))
}

/// `git add -- <path>`: the file is resolved as it is.
pub fn stage_resolved_path(git: Arc<GitBinary>, workdir: &Path, path: &str) -> Result<()> {
    GitCommand::new(git)
        .args(["add", "--", path])
        .env("GIT_LITERAL_PATHSPECS", "1")
        .current_dir(workdir)
        .run()?;
    Ok(())
}

/// `git rm -q -- <path>`: the file is resolved as deleted.
pub fn remove_resolved_path(git: Arc<GitBinary>, workdir: &Path, path: &str) -> Result<()> {
    GitCommand::new(git)
        .args(["rm", "-q", "--", path])
        .env("GIT_LITERAL_PATHSPECS", "1")
        .current_dir(workdir)
        .run()?;
    Ok(())
}

/// A commit that wrote some of a chunk's lines on one side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChunkBlame {
    pub commit: BlameCommit,
    pub lines: u32,
}

/// Who wrote `lines` (0-based) of `path` in `rev`, the most lines first.
/// Empty for an empty range or a path `rev` does not have (renamed on
/// that side).
pub fn blame_lines(
    git: Arc<GitBinary>,
    workdir: &Path,
    rev: &str,
    path: &str,
    lines: Range<u32>,
    cancel: Option<CancelToken>,
) -> Result<Vec<ChunkBlame>> {
    if lines.is_empty() {
        return Ok(Vec::new());
    }
    let options = BlameOptions {
        line_range: Some((lines.start + 1, lines.end)),
        ..BlameOptions::default()
    };
    let mut commits: HashMap<String, BlameCommit> = HashMap::new();
    let mut counts: HashMap<String, u32> = HashMap::new();
    let result = blame(
        git,
        workdir,
        Some(rev),
        path,
        options,
        cancel,
        |event| match event {
            BlameEvent::Commit(commit) => {
                commits.insert(commit.sha.clone(), commit);
            }
            BlameEvent::Range(range) => *counts.entry(range.sha).or_default() += range.count,
        },
    );
    match result {
        Ok(()) => {}
        Err(GitError::Failed { stderr, .. })
            if stderr.contains("no such path") || stderr.contains("has only") =>
        {
            return Ok(Vec::new());
        }
        Err(e) => return Err(e),
    }
    let mut out: Vec<ChunkBlame> = counts
        .into_iter()
        .filter_map(|(sha, lines)| {
            commits
                .remove(&sha)
                .map(|commit| ChunkBlame { commit, lines })
        })
        .collect();
    out.sort_by(|a, b| {
        b.lines
            .cmp(&a.lines)
            .then(b.commit.author_time.cmp(&a.commit.author_time))
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

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

    /// git that may fail (a merge that conflicts).
    fn try_run(path: &Path, args: &[&str]) {
        let _ = Command::new("git")
            .args(args)
            .current_dir(path)
            .env("GIT_AUTHOR_NAME", "T")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "T")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .unwrap();
    }

    fn commit(path: &Path, name: &str, contents: &str, message: &str) {
        std::fs::write(path.join(name), contents).unwrap();
        run(path, &["add", "."]);
        run(path, &["commit", "-q", "-m", message]);
    }

    fn repo() -> (tempfile::TempDir, Arc<GitBinary>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        run(path, &["init", "-q", "-b", "main"]);
        run(path, &["config", "commit.gpgsign", "false"]);
        run(path, &["config", "core.autocrlf", "false"]);
        commit(path, "a.txt", "1\n2\n3\n4\n5\n", "first");
        (dir, Arc::new(crate::find_git().unwrap()))
    }

    /// main and feature both edit line 3 of a.txt.
    fn conflicted_merge() -> (tempfile::TempDir, Arc<GitBinary>) {
        let (dir, git) = repo();
        let path = dir.path();
        run(path, &["checkout", "-q", "-b", "feature"]);
        commit(path, "a.txt", "1\n2\nfeature\n4\n5\n", "feature edits 3");
        run(path, &["checkout", "-q", "main"]);
        commit(path, "a.txt", "1\n2\nmain\n4\n5\n", "main edits 3");
        try_run(path, &["merge", "feature"]);
        (dir, git)
    }

    #[test]
    fn reads_the_stages_of_a_merge() {
        let (dir, git) = conflicted_merge();
        let path = dir.path();
        let inputs = load_merge_inputs(git.clone(), path, "a.txt").unwrap();
        assert_eq!(inputs.origin, ConflictOrigin::Merge);
        assert_eq!(
            inputs.entries.iter().map(|e| e.stage).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert_eq!(inputs.base.as_deref(), Some("1\n2\n3\n4\n5\n"));
        assert_eq!(inputs.ours.as_deref(), Some("1\n2\nmain\n4\n5\n"));
        assert_eq!(inputs.theirs.as_deref(), Some("1\n2\nfeature\n4\n5\n"));
        assert!(
            inputs
                .working
                .as_deref()
                .unwrap()
                .contains("<<<<<<< HEAD\nmain\n=======\nfeature\n>>>>>>> feature\n")
        );
        assert_eq!(inputs.pick, None);
        assert_eq!(inputs.labels.theirs.as_deref(), Some("feature"));
        assert_eq!(inputs.revs.ours.as_deref(), Some("HEAD"));
        assert!(inputs.revs.base.is_some());
    }

    #[test]
    fn rebase_cherry_pick_and_stash_origins() {
        let (dir, git) = repo();
        let path = dir.path();
        run(path, &["checkout", "-q", "-b", "feature"]);
        commit(path, "a.txt", "1\n2\nfeature\n4\n5\n", "feature edits 3");
        run(path, &["checkout", "-q", "main"]);
        commit(path, "a.txt", "1\n2\nmain\n4\n5\n", "main edits 3");
        try_run(path, &["cherry-pick", "feature"]);
        let inputs = load_merge_inputs(git.clone(), path, "a.txt").unwrap();
        assert_eq!(inputs.origin, ConflictOrigin::CherryPick);
        assert!(
            inputs
                .labels
                .theirs
                .as_deref()
                .unwrap()
                .ends_with("(feature edits 3)")
        );
        assert_eq!(inputs.theirs.as_deref(), Some("1\n2\nfeature\n4\n5\n"));
        run(path, &["cherry-pick", "--abort"]);

        run(path, &["checkout", "-q", "feature"]);
        try_run(path, &["rebase", "main"]);
        let inputs = load_merge_inputs(git.clone(), path, "a.txt").unwrap();
        assert_eq!(inputs.origin, ConflictOrigin::Rebase);
        assert_eq!(inputs.ours.as_deref(), Some("1\n2\nmain\n4\n5\n"));
        assert_eq!(inputs.theirs.as_deref(), Some("1\n2\nfeature\n4\n5\n"));
        run(path, &["rebase", "--abort"]);

        std::fs::write(path.join("a.txt"), "1\n2\nstashed\n4\n5\n").unwrap();
        run(path, &["stash"]);
        commit(path, "a.txt", "1\n2\nnewer\n4\n5\n", "newer");
        try_run(path, &["stash", "pop"]);
        let inputs = load_merge_inputs(git, path, "a.txt").unwrap();
        assert_eq!(inputs.origin, ConflictOrigin::StashPop);
        assert_eq!(inputs.theirs.as_deref(), Some("1\n2\nstashed\n4\n5\n"));
        assert_eq!(inputs.revs.theirs.as_deref(), Some("refs/stash"));
    }

    #[test]
    fn add_add_and_delete_modify() {
        let (dir, git) = repo();
        let path = dir.path();
        run(path, &["checkout", "-q", "-b", "feature"]);
        commit(path, "new.txt", "theirs\n", "feature adds new");
        run(path, &["rm", "-q", "a.txt"]);
        run(path, &["commit", "-q", "-m", "feature deletes a"]);
        run(path, &["checkout", "-q", "main"]);
        commit(path, "new.txt", "ours\n", "main adds new");
        commit(path, "a.txt", "1\n2\n3\n4\nmain\n", "main edits a");
        try_run(path, &["merge", "feature"]);
        let added = load_merge_inputs(git.clone(), path, "new.txt").unwrap();
        assert_eq!(added.base, None);
        assert_eq!(added.pick, None);
        let deleted = load_merge_inputs(git, path, "a.txt").unwrap();
        assert_eq!(deleted.theirs, None);
        assert_eq!(deleted.pick, Some(PickReason::Deleted));
    }

    #[test]
    fn binary_attributes_and_marker_size() {
        let (dir, git) = repo();
        let path = dir.path();
        std::fs::write(
            path.join(".gitattributes"),
            "a.txt conflict-marker-size=10\n*.bin binary\n",
        )
        .unwrap();
        commit(path, "x.bin", "\0one", "binary");
        run(path, &["checkout", "-q", "-b", "feature"]);
        commit(path, "x.bin", "\0feature", "feature bin");
        commit(path, "a.txt", "1\n2\nfeature\n4\n5\n", "feature edits 3");
        run(path, &["checkout", "-q", "main"]);
        commit(path, "x.bin", "\0main", "main bin");
        commit(path, "a.txt", "1\n2\nmain\n4\n5\n", "main edits 3");
        try_run(path, &["merge", "feature"]);
        let text = load_merge_inputs(git.clone(), path, "a.txt").unwrap();
        assert_eq!(text.attrs.marker_size, Some(10));
        assert!(text.working.unwrap().contains("<<<<<<<<<< HEAD\n"));
        let bin = load_merge_inputs(git, path, "x.bin").unwrap();
        assert!(matches!(
            bin.pick,
            Some(PickReason::Binary | PickReason::BinaryMergeDriver)
        ));
    }

    #[test]
    fn zdiff3_config_and_crlf_working_file() {
        let (dir, git) = conflicted_merge();
        let path = dir.path();
        run(path, &["config", "merge.conflictStyle", "zdiff3"]);
        let working = std::fs::read_to_string(path.join("a.txt")).unwrap();
        std::fs::write(path.join("a.txt"), to_crlf(&working)).unwrap();
        let inputs = load_merge_inputs(git, path, "a.txt").unwrap();
        assert_eq!(inputs.conflict_style.as_deref(), Some("zdiff3"));
        assert!(inputs.crlf);
        assert!(!inputs.working.unwrap().contains('\r'));
    }

    #[test]
    fn writes_refuse_a_changed_file_then_stage() {
        let (dir, git) = conflicted_merge();
        let path = dir.path();
        let stamp = file_stamp(path, "a.txt");
        let written = write_merge_result(path, "a.txt", "1\n2\nboth\n4\n5\n", true, stamp).unwrap();
        let WriteOutcome::Written(stamp) = written else {
            panic!("not written: {written:?}");
        };
        assert_eq!(
            std::fs::read_to_string(path.join("a.txt")).unwrap(),
            "1\r\n2\r\nboth\r\n4\r\n5\r\n"
        );
        std::fs::write(path.join("a.txt"), "someone else\n").unwrap();
        assert!(matches!(
            write_merge_result(path, "a.txt", "mine\n", false, Some(stamp)).unwrap(),
            WriteOutcome::ChangedOnDisk(Some(_))
        ));
        stage_resolved_path(git.clone(), path, "a.txt").unwrap();
        assert!(unmerged_entries(git, path, "a.txt").unwrap().is_empty());
    }

    #[test]
    fn blames_a_chunk_on_each_side() {
        let (dir, git) = conflicted_merge();
        let path = dir.path();
        let revs = side_revs(git.clone(), path, ConflictOrigin::Merge);
        let ours = blame_lines(
            git.clone(),
            path,
            revs.ours.as_deref().unwrap(),
            "a.txt",
            2..3,
            None,
        )
        .unwrap();
        assert_eq!(ours.len(), 1);
        assert_eq!(ours[0].commit.summary, "main edits 3");
        let theirs = blame_lines(
            git.clone(),
            path,
            revs.theirs.as_deref().unwrap(),
            "a.txt",
            2..3,
            None,
        )
        .unwrap();
        assert_eq!(theirs[0].commit.summary, "feature edits 3");
        assert!(
            blame_lines(git.clone(), path, "HEAD", "a.txt", 3..3, None)
                .unwrap()
                .is_empty()
        );
        assert!(
            blame_lines(git, path, "HEAD", "gone.txt", 0..1, None)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn parses_attributes_and_merge_messages() {
        let attrs = parse_attributes(
            "a\0conflict-marker-size\x0012\0a\0merge\0unset\0a\0filter\0lfs\0a\0eol\0unspecified\0",
        );
        assert_eq!(attrs.marker_size, Some(12));
        assert!(attrs.binary_merge());
        assert!(attrs.is_lfs());
        assert_eq!(attrs.eol, None);
        assert_eq!(
            merge_msg_branch("Merge branch 'feature/x'\n\n# Conflicts:\n").as_deref(),
            Some("feature/x")
        );
        assert_eq!(
            merge_msg_branch("Merge remote-tracking branch 'origin/main' into topic").as_deref(),
            Some("origin/main")
        );
        assert_eq!(to_crlf("a\r\nb\n"), "a\r\nb\r\n");
    }
}
