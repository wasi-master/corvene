//! `git status --porcelain=2 -z` (GHD `lib/status-parser.ts` + `lib/git/status.ts`).
//! Files come in git's order, built like GHD's `buildStatusMap`; the
//! changes list sorts them (`WorkingDirectoryStatus::sort_files`, GHD
//! `updateChangedFiles`).
//!
//! Deviations behind flags: [`StatusOptions`] (`respect-show-untracked-files`,
//! `ignore-submodules`, `1205-lfs-conflicts-pick-a-side`) and
//! [`working_directory_line_stats`] (`changes-line-counts`). A bisect in
//! progress is read whatever the flags say (`1212-bisect` gates its UI).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use corvene_models::{
    AheadBehind, DiffSelection, FileStatus, FileStatusKind, GitStatusEntry,
    WorkingDirectoryFileChange, WorkingDirectoryStatus,
};

use crate::detect::GitBinary;
use crate::error::Result;
use crate::process::GitCommand;

/// GHD `conflictStatusCodes`.
const CONFLICT_CODES: &[&str] = &["DD", "AU", "UD", "UA", "DU", "AA", "UU"];

/// Corvene deviations from GHD's status invocation, set from flags by the
/// dispatcher's refresh. The default is GHD's behaviour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatusOptions {
    /// `status.showUntrackedFiles=no` (or false) hides untracked files
    /// (`--untracked-files=no`); GHD always passes `--untracked-files=all`.
    /// Flag `respect-show-untracked-files`.
    pub respect_show_untracked_files: bool,
    /// `--ignore-submodules=<when>`; GHD passes nothing, so only
    /// `submodule.<name>.ignore` applies. Flag `ignore-submodules`.
    pub ignore_submodules: IgnoreSubmodules,
    /// Read the status in-process with gitoxide (`status_gix.rs`), git
    /// only when that fails. Flag `906-in-process-status`.
    pub in_process: bool,
    /// Conflicted files whose `filter` attribute is `lfs` are manual
    /// conflicts (pick a side): git merges LFS pointer files as text, so
    /// their conflict markers sit in the pointer, not in the file. Flag
    /// `1205-lfs-conflicts-pick-a-side`; GHD offers the editor.
    pub lfs_conflicts_manual: bool,
    /// The in-process reader pairs a deleted tracked file with a similar
    /// untracked one as a rename in the working tree (`.R`), which `git
    /// status` never does. Flag `786-worktree-rename-detection`.
    pub worktree_renames: bool,
}

/// What `git status` leaves out about submodules.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IgnoreSubmodules {
    /// No option: `submodule.<name>.ignore` decides (GHD).
    #[default]
    AsConfigured,
    /// `--ignore-submodules=dirty`: changes inside submodules are hidden,
    /// a changed submodule commit is still listed.
    Dirty,
    /// `--ignore-submodules=all`: submodules are never listed.
    All,
}

/// Flag `911-git-status-timeout`: seconds `git status` may run before it
/// is stopped (0: no limit, GHD). A file that exists only in the cloud
/// (iCloud Drive's Optimize Mac Storage, OneDrive and Dropbox on demand)
/// makes git wait for a download that may never come, and each refresh
/// would start another `git status` behind the stuck one.
static STATUS_TIMEOUT_SECS: AtomicU32 = AtomicU32::new(0);

pub fn set_status_timeout(seconds: u32) {
    STATUS_TIMEOUT_SECS.store(seconds, Ordering::Relaxed);
}

/// Run status and build the model (GHD `getStatus`). The files come with
/// default selections; the caller carries over the previous ones
/// (`corvene_core::changes_state::merge_changed_files`, GHD
/// `updateChangedFiles`).
pub fn get_status(git: Arc<GitBinary>, workdir: &Path) -> Result<WorkingDirectoryStatus> {
    get_status_with(git, workdir, StatusOptions::default())
}

/// [`get_status`] with Corvene's [`StatusOptions`].
pub fn get_status_with(
    git: Arc<GitBinary>,
    workdir: &Path,
    options: StatusOptions,
) -> Result<WorkingDirectoryStatus> {
    let hide_untracked = hide_untracked(git.clone(), workdir, options);
    let in_process = options
        .in_process
        .then(|| crate::status_gix::status(workdir, options, hide_untracked))
        .flatten();
    let status = match in_process {
        Some(status) => status,
        None => {
            let untracked = if hide_untracked {
                "--untracked-files=no"
            } else {
                "--untracked-files=all"
            };
            let mut args = vec!["status", untracked];
            match options.ignore_submodules {
                IgnoreSubmodules::AsConfigured => {}
                IgnoreSubmodules::Dirty => args.push("--ignore-submodules=dirty"),
                IgnoreSubmodules::All => args.push("--ignore-submodules=all"),
            }
            args.extend(["--branch", "--porcelain=2", "-z"]);
            let mut cmd = GitCommand::new(git.clone()).args(args).current_dir(workdir);
            let limit = STATUS_TIMEOUT_SECS.load(Ordering::Relaxed);
            if limit > 0 {
                cmd = cmd.timeout(Duration::from_secs(u64::from(limit)));
            }
            let out = cmd.run()?;
            parse_porcelain_v2(&out.stdout)
        }
    };
    Ok(finish_status(git, workdir, status, options))
}

/// Flag `906-in-process-status`'s reader on its own: the status gitoxide
/// reads, finished as [`get_status_with`] finishes it, or `None` where
/// [`get_status_with`] runs git instead (`options.in_process` is ignored).
/// For tests that hold the two side by side.
pub fn get_status_in_process(
    git: Arc<GitBinary>,
    workdir: &Path,
    options: StatusOptions,
) -> Option<WorkingDirectoryStatus> {
    let hide_untracked = hide_untracked(git.clone(), workdir, options);
    let status = crate::status_gix::status(workdir, options, hide_untracked)?;
    Some(finish_status(git, workdir, status, options))
}

/// `respect_show_untracked_files` and `status.showUntrackedFiles` says no.
fn hide_untracked(git: Arc<GitBinary>, workdir: &Path, options: StatusOptions) -> bool {
    options.respect_show_untracked_files
        && crate::remote_ops::config_value(git, workdir, "status.showUntrackedFiles").is_some_and(
            |v| {
                matches!(
                    v.to_ascii_lowercase().as_str(),
                    "no" | "false" | "off" | "0"
                )
            },
        )
}

/// What [`get_status_with`] adds to the parsed files and branch headers,
/// whichever reader produced them: the operation in progress and conflict
/// details.
fn finish_status(
    git: Arc<GitBinary>,
    workdir: &Path,
    mut status: WorkingDirectoryStatus,
    options: StatusOptions,
) -> WorkingDirectoryStatus {
    let git_dir = crate::paths::git_dir(workdir);
    status.merge_head_found = git_dir.join("MERGE_HEAD").exists();
    status.rebase_in_progress =
        git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists();
    status.cherry_pick_head_found = git_dir.join("CHERRY_PICK_HEAD").exists();
    status.squash_msg_found = git_dir.join("SQUASH_MSG").exists();
    status.rebase_internal_state = crate::rebase_ops::rebase_internal_state(workdir);
    // git writes `amend` only at an `edit` stop that applied cleanly
    status.rebase_edit_stop = git_dir.join("rebase-merge/amend").exists();
    // Corvene `1212-bisect` (the flag gates what is shown, not this read)
    if git_dir.join("BISECT_START").is_file() {
        status.bisect = crate::bisect::bisect_state(git.clone(), workdir)
            .ok()
            .flatten()
            .map(|mut bisect| {
                if let Ok(Some(range)) = crate::bisect::bisect_range(git.clone(), workdir, &bisect)
                {
                    bisect.candidates = range.candidates;
                }
                bisect
            });
    }
    if status.has_conflicts() {
        apply_conflict_details(git, workdir, &mut status, options.lfs_conflicts_manual);
    }
    // git's order (tracked before untracked, bytewise), as GHD `getStatus`
    // returns it; the changes list sorts them
    // (`WorkingDirectoryStatus::sort_files`)
    status
}

/// Corvene `903-refresh-stale-index`: `git update-index -q --refresh`, which
/// writes the working files' current stat data into the index. Status runs
/// with `GIT_OPTIONAL_LOCKS=0` (as GHD's does), so it never saves what it
/// learns: once many files' stat data is stale (a copied or restored
/// checkout, a tool that rewrote files unchanged) every status re-reads them
/// (6 s instead of 0.2 s on a 50,000-file tree). At most once a minute per
/// repository; a held `index.lock` makes it give up, which is fine.
pub fn refresh_stale_index(git: Arc<GitBinary>, workdir: &Path) {
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};
    use std::time::{Duration, Instant};
    static LAST: LazyLock<Mutex<HashMap<std::path::PathBuf, Instant>>> =
        LazyLock::new(Default::default);
    if let Ok(mut last) = LAST.lock() {
        if last
            .get(workdir)
            .is_some_and(|t| t.elapsed() < Duration::from_secs(60))
        {
            return;
        }
        last.insert(workdir.to_path_buf(), Instant::now());
    }
    let _ = GitCommand::new(git)
        .args(["update-index", "-q", "--refresh"])
        .current_dir(workdir)
        // it exits 1 when files need updating, the normal case here
        .allow_exit_code(1)
        .run();
}

/// Lines added / deleted in one changed file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineStats {
    pub added: u64,
    pub deleted: u64,
}

/// Untracked files larger than this are not read for a line count.
const UNTRACKED_LINE_COUNT_LIMIT: u64 = 1024 * 1024;

/// Per-file line counts of the working directory against `HEAD` (the empty
/// tree on an unborn branch): `git diff --numstat --no-renames -z` for
/// tracked files; untracked files are read and their lines counted. Binary
/// files and untracked files over 1 MiB are left out.
pub fn working_directory_line_stats(
    git: Arc<GitBinary>,
    workdir: &Path,
    status: &WorkingDirectoryStatus,
) -> Result<std::collections::HashMap<String, LineStats>> {
    let run = |base: &str| {
        GitCommand::new(git.clone())
            .args(["diff", "--numstat", "--no-renames", "-z", base, "--"])
            .current_dir(workdir)
            .run()
    };
    let out = match run("HEAD") {
        Ok(out) => out,
        Err(crate::error::GitError::Failed { stderr, .. })
            if stderr.contains("bad revision")
                || stderr.contains("unknown revision")
                || stderr.contains("ambiguous argument") =>
        {
            run(crate::log::NULL_TREE_SHA)?
        }
        Err(err) => return Err(err),
    };
    let mut stats = parse_numstat(&out.stdout);
    // untracked files are counted again only when their size or mtime moved:
    // reading every file of a 100,000-file untracked tree on each refresh
    // takes seconds
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};
    type Counts = HashMap<String, (u64, std::time::SystemTime, Option<u64>)>;
    static SEEN: LazyLock<Mutex<HashMap<std::path::PathBuf, Counts>>> =
        LazyLock::new(Default::default);
    let previous = SEEN
        .lock()
        .ok()
        .and_then(|mut seen| seen.remove(workdir))
        .unwrap_or_default();
    let mut counted = Counts::new();
    for file in &status.files {
        if file.status.kind != FileStatusKind::Untracked {
            continue;
        }
        let Ok(meta) = std::fs::metadata(workdir.join(&file.path)) else {
            continue;
        };
        let stamp = (meta.len(), meta.modified().unwrap_or(std::time::UNIX_EPOCH));
        let lines = match previous.get(&file.path) {
            Some(&(len, mtime, lines)) if (len, mtime) == stamp => lines,
            _ => untracked_line_count(&workdir.join(&file.path), &meta),
        };
        counted.insert(file.path.clone(), (stamp.0, stamp.1, lines));
        if let Some(lines) = lines {
            stats.insert(
                file.path.clone(),
                LineStats {
                    added: lines,
                    deleted: 0,
                },
            );
        }
    }
    if let Ok(mut seen) = SEEN.lock() {
        seen.insert(workdir.to_path_buf(), counted);
    }
    Ok(stats)
}

/// `added TAB deleted TAB path NUL` records; binary files (`-`) are skipped.
fn parse_numstat(stdout: &[u8]) -> std::collections::HashMap<String, LineStats> {
    let text = String::from_utf8_lossy(stdout);
    text.split('\0')
        .filter_map(|record| {
            let mut cols = record.splitn(3, '\t');
            let added = cols.next()?.trim_start_matches('\n').parse().ok()?;
            let deleted = cols.next()?.parse().ok()?;
            let path = cols.next().filter(|p| !p.is_empty())?;
            Some((path.to_string(), LineStats { added, deleted }))
        })
        .collect()
}

fn untracked_line_count(path: &Path, meta: &std::fs::Metadata) -> Option<u64> {
    if !meta.is_file() || meta.len() > UNTRACKED_LINE_COUNT_LIMIT {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    // git's binary heuristic: a NUL in the first 8000 bytes
    if bytes.iter().take(8000).any(|&b| b == 0) {
        return None;
    }
    let newlines = bytes.iter().filter(|&&b| b == b'\n').count() as u64;
    let unterminated = bytes.last().is_some_and(|&b| b != b'\n');
    Some(newlines + u64::from(unterminated))
}

/// Parse NUL-separated porcelain v2 output.
pub fn parse_porcelain_v2(stdout: &[u8]) -> WorkingDirectoryStatus {
    let text = String::from_utf8_lossy(stdout);
    let mut fields = text.split('\0').filter(|f| !f.is_empty()).peekable();
    let mut status = WorkingDirectoryStatus::default();
    let mut files = FileMap::default();

    while let Some(field) = fields.next() {
        let mut parts = field.splitn(2, ' ');
        let tag = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("");
        match tag {
            "#" => parse_header(rest, &mut status),
            "1" => {
                // <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>
                let cols: Vec<&str> = rest.splitn(8, ' ').collect();
                if cols.len() == 8 {
                    let code = cols[0];
                    let sub = cols[1];
                    files.push(cols[7], None, code, sub, None);
                }
            }
            "2" => {
                // <XY> <sub> <mH> <mI> <mW> <hH> <hI> <Xscore> <path>\0<origPath>
                let cols: Vec<&str> = rest.splitn(9, ' ').collect();
                if cols.len() == 9 {
                    let code = cols[0];
                    let sub = cols[1];
                    let score = cols[7].trim_start_matches(['R', 'C']).parse::<u8>().ok();
                    let orig = fields.next().map(str::to_string);
                    files.push(cols[8], orig, code, sub, score);
                }
            }
            "u" => {
                // <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>
                let cols: Vec<&str> = rest.splitn(10, ' ').collect();
                if cols.len() == 10 {
                    files.push(cols[9], None, cols[0], cols[1], None);
                }
            }
            "?" => files.push(rest, None, "??", "N...", None),
            "!" => {}
            _ => {}
        }
    }
    files.finish(&mut status);
    status
}

/// GHD `getConflictDetails`: text conflicts carry their marker count, binary
/// and add/delete conflicts stay "manual" (`conflict_markers: None`).
/// Binary files are detected against `MERGE_HEAD` during a merge,
/// `REBASE_HEAD` during a rebase and `HEAD` otherwise (conflicts from
/// popping a stash, where an unborn `HEAD` just means no binary files). A
/// failure during a merge or rebase drops all details, as GHD's does.
/// With `lfs_manual` (flag `1205`) conflicted LFS files are manual too
/// ([`lfs_paths`]).
fn apply_conflict_details(
    git: Arc<GitBinary>,
    workdir: &Path,
    status: &mut WorkingDirectoryStatus,
    lfs_manual: bool,
) {
    let conflicted: Vec<String> = status
        .files
        .iter()
        .filter(|f| f.status.is_conflicted())
        .map(|f| f.path.clone())
        .collect();
    let details = || -> Result<_> {
        let markers = crate::rebase_ops::conflict_marker_counts(git.clone(), workdir)?;
        let binary_paths = |reference| {
            crate::rebase_ops::binary_paths(git.clone(), workdir, reference, &conflicted)
        };
        let binary = if status.merge_head_found {
            binary_paths("MERGE_HEAD")?
        } else if status.rebase_internal_state.is_some() {
            binary_paths("REBASE_HEAD")?
        } else {
            binary_paths("HEAD").unwrap_or_default()
        };
        let mut binary = binary;
        if lfs_manual {
            binary.extend(lfs_paths(git.clone(), workdir, &conflicted).unwrap_or_default());
        }
        Ok((markers, binary))
    };
    let (markers, binary) = details().unwrap_or_else(|err| {
        tracing::error!(%err, "unexpected error from git operations in getConflictDetails");
        Default::default()
    });
    for file in &mut status.files {
        if !file.status.is_conflicted() {
            continue;
        }
        // GHD `TextConflictDetails`: both added or both modified, and not binary
        let text_codes = matches!(file.status.code.as_str(), "AA" | "UU");
        file.status.conflict_markers = if text_codes && !binary.contains(&file.path) {
            Some(markers.get(&file.path).copied().unwrap_or(0))
        } else {
            None
        };
    }
}

/// The `paths` whose `filter` attribute is `lfs` (`check-attr --stdin -z
/// filter`).
pub fn lfs_paths(git: Arc<GitBinary>, workdir: &Path, paths: &[String]) -> Result<Vec<String>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let mut stdin: Vec<u8> = Vec::new();
    for path in paths {
        stdin.extend_from_slice(path.as_bytes());
        stdin.push(0);
    }
    let out = GitCommand::new(git)
        .args(["check-attr", "--stdin", "-z", "filter"])
        .current_dir(workdir)
        .stdin(stdin)
        .run()?;
    Ok(parse_lfs_attributes(&String::from_utf8_lossy(&out.stdout)))
}

/// `check-attr -z filter` output: `<path> NUL filter NUL <value> NUL` per
/// path; the paths whose value is `lfs`.
pub fn parse_lfs_attributes(stdout: &str) -> Vec<String> {
    let records: Vec<&str> = stdout.split('\0').collect();
    let (records, _) = records.as_chunks::<3>();
    records
        .iter()
        .filter(|[_, attribute, value]| *attribute == "filter" && *value == "lfs")
        .map(|[path, _, _]| path.to_string())
        .collect()
}

fn parse_header(rest: &str, status: &mut WorkingDirectoryStatus) {
    let mut parts = rest.splitn(2, ' ');
    let key = parts.next().unwrap_or("");
    let value = parts.next().unwrap_or("").trim();
    match key {
        "branch.oid" if value != "(initial)" => status.current_tip = Some(value.to_string()),
        "branch.head" if value != "(detached)" => status.branch = Some(value.to_string()),
        "branch.upstream" => status.upstream = Some(value.to_string()),
        "branch.ab" => {
            // "+A -B"
            let mut ab = AheadBehind::default();
            for token in value.split_whitespace() {
                if let Some(a) = token.strip_prefix('+') {
                    ab.ahead = a.parse().unwrap_or(0);
                } else if let Some(b) = token.strip_prefix('-') {
                    ab.behind = b.parse().unwrap_or(0);
                }
            }
            status.ahead_behind = Some(ab);
        }
        _ => {}
    }
}

/// GHD `buildStatusMap`: the changed files keyed on their path, in git's
/// order (a `Map` keeps insertion order). Removed entries leave an empty
/// slot so the index stays valid. The in-process status (`status_gix.rs`)
/// builds its files with it too, fed in git's order.
#[derive(Default)]
pub(crate) struct FileMap {
    slots: Vec<Option<WorkingDirectoryFileChange>>,
    index: std::collections::HashMap<String, usize>,
    /// An index entry was left out
    /// ([`WorkingDirectoryStatus::hidden_index_entries`]).
    hidden_index_entries: bool,
}

impl FileMap {
    pub(crate) fn push(
        &mut self,
        path: &str,
        old_path: Option<String>,
        code: &str,
        sub: &str,
        score: Option<u8>,
    ) {
        let Some(file_status) = map_status(code, sub, score) else {
            return;
        };
        // added in the index, then removed from the working directory: the
        // file won't be part of the commit, so it is not listed
        if file_status.index == GitStatusEntry::Added
            && file_status.working_tree == GitStatusEntry::Deleted
        {
            self.hidden_index_entries = true;
            return;
        }
        // a staged delete and an untracked file at the same path: only the
        // untracked one is listed (`files.delete`, so it goes to the end)
        if file_status.kind == FileStatusKind::Untracked
            && let Some(ix) = self.index.remove(path)
        {
            self.slots[ix] = None;
        }
        // a submodule whose commit did not change has nothing to commit
        let selection = if file_status.kind == FileStatusKind::Modified
            && file_status
                .submodule_status
                .is_some_and(|s| !s.commit_changed)
        {
            DiffSelection::none()
        } else {
            DiffSelection::all()
        };
        let file = WorkingDirectoryFileChange {
            path: path.to_string(),
            old_path,
            status: file_status,
            selection,
        };
        match self.index.get(path) {
            Some(&ix) => self.slots[ix] = Some(file),
            None => {
                self.index.insert(path.to_string(), self.slots.len());
                self.slots.push(Some(file));
            }
        }
    }

    /// The files in insertion order, and whether an index entry was left
    /// out, into `status`.
    pub(crate) fn finish(self, status: &mut WorkingDirectoryStatus) {
        status.hidden_index_entries = self.hidden_index_entries;
        status.files = self.slots.into_iter().flatten().collect();
    }
}

fn entry(c: char) -> GitStatusEntry {
    match c {
        'M' | 'T' => GitStatusEntry::Modified,
        'A' => GitStatusEntry::Added,
        'D' => GitStatusEntry::Deleted,
        'R' => GitStatusEntry::Renamed,
        'C' => GitStatusEntry::Copied,
        'U' => GitStatusEntry::Unmerged,
        '?' => GitStatusEntry::Untracked,
        _ => GitStatusEntry::Unchanged,
    }
}

/// GHD `mapStatus` + `convertToAppStatus`.
pub fn map_status(code: &str, sub: &str, score: Option<u8>) -> Option<FileStatus> {
    let mut chars = code.chars();
    let x = chars.next()?;
    let y = chars.next()?;
    let index = entry(x);
    let working_tree = entry(y);
    let submodule = sub.starts_with('S');
    // `S<c><m><u>`: commit changed / modified tracked files / untracked files
    let submodule_status = submodule.then(|| {
        let flag = |ix: usize, ch: char| sub.chars().nth(ix) == Some(ch);
        corvene_models::SubmoduleStatus {
            commit_changed: flag(1, 'C'),
            modified_changes: flag(2, 'M'),
            untracked_changes: flag(3, 'U'),
        }
    });

    let kind = if code == "??" {
        FileStatusKind::Untracked
    } else if CONFLICT_CODES.contains(&code) {
        FileStatusKind::Conflicted
    } else {
        match (x, y) {
            ('R', _) | (_, 'R') => FileStatusKind::Renamed,
            ('C', _) | (_, 'C') => FileStatusKind::Copied,
            // GHD: added-then-deleted / added-then-modified count as New
            ('A', _) | (_, 'A') => FileStatusKind::New,
            ('D', _) | (_, 'D') => FileStatusKind::Deleted,
            ('M', _) | (_, 'M') | ('T', _) | (_, 'T') => FileStatusKind::Modified,
            _ => return None,
        }
    };
    Some(FileStatus {
        kind,
        index,
        working_tree,
        score,
        code: code.to_string(),
        submodule,
        submodule_status,
        conflict_markers: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lfs_attributes_name_the_lfs_paths() {
        let out = "a.psd\0filter\0lfs\0b.txt\0filter\0unspecified\0c.bin\0filter\0lfs\0";
        assert_eq!(parse_lfs_attributes(out), ["a.psd", "c.bin"]);
        assert!(parse_lfs_attributes("").is_empty());
    }

    #[test]
    fn parses_headers_and_entries() {
        let raw = concat!(
            "# branch.oid abc\0",
            "# branch.head main\0",
            "# branch.upstream origin/main\0",
            "# branch.ab +2 -1\0",
            "1 .M N... 100644 100644 100644 aaa bbb src/lib.rs\0",
            "1 A. N... 000000 100644 100644 000 ccc new.txt\0",
            "1 D. N... 100644 000000 000000 ddd 000 gone.txt\0",
            "2 R. N... 100644 100644 100644 eee eee R100 b.txt\0a.txt\0",
            "u UU N... 100644 100644 100644 100644 f1 f2 f3 conflict.txt\0",
            "? untracked.md\0",
            "! ignored.log\0",
        );
        let s = parse_porcelain_v2(raw.as_bytes());
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!(s.upstream.as_deref(), Some("origin/main"));
        assert_eq!(
            s.ahead_behind,
            Some(AheadBehind {
                ahead: 2,
                behind: 1
            })
        );
        let kinds: Vec<(String, FileStatusKind)> = s
            .files
            .iter()
            .map(|f| (f.path.clone(), f.status.kind))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("src/lib.rs".into(), FileStatusKind::Modified),
                ("new.txt".into(), FileStatusKind::New),
                ("gone.txt".into(), FileStatusKind::Deleted),
                ("b.txt".into(), FileStatusKind::Renamed),
                ("conflict.txt".into(), FileStatusKind::Conflicted),
                ("untracked.md".into(), FileStatusKind::Untracked),
            ]
        );
        let renamed = &s.files[3];
        assert_eq!(renamed.old_path.as_deref(), Some("a.txt"));
        assert_eq!(renamed.status.score, Some(100));
        assert_eq!(s.files[0].status.index, GitStatusEntry::Unchanged);
        assert_eq!(s.files[0].status.working_tree, GitStatusEntry::Modified);
        assert!(s.has_conflicts());
        assert_eq!(s.include_all(), Some(true));
    }

    #[test]
    fn builds_the_status_map_like_ghd() {
        use corvene_models::DiffSelectionType;
        let raw = concat!(
            "1 D. N... 100644 000000 000000 aaa 000 first\0",
            "1 AD N... 000000 100644 000000 000 bbb gone\0",
            "1 .M SC.. 160000 160000 160000 ccc ccc sub-moved\0",
            "1 .M S.M. 160000 160000 160000 ddd ddd sub-dirty\0",
            "? first\0",
        );
        let s = parse_porcelain_v2(raw.as_bytes());
        let files: Vec<(&str, FileStatusKind, DiffSelectionType)> = s
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.status.kind, f.selection.kind()))
            .collect();
        assert_eq!(
            files,
            [
                (
                    "sub-moved",
                    FileStatusKind::Modified,
                    DiffSelectionType::All
                ),
                (
                    "sub-dirty",
                    FileStatusKind::Modified,
                    DiffSelectionType::None
                ),
                ("first", FileStatusKind::Untracked, DiffSelectionType::All),
            ]
        );
        // `gone` is still in the index, so a commit has to reset it first
        assert!(s.hidden_index_entries);
        assert!(
            !parse_porcelain_v2(b"1 .M N... 100644 100644 100644 a b f\0").hidden_index_entries
        );
    }

    #[test]
    fn parses_numstat_and_skips_binary() {
        let stats = parse_numstat(b"3\t1\tsrc/a b.rs\0-\t-\timg.png\0" as &[u8]);
        assert_eq!(
            stats.get("src/a b.rs"),
            Some(&LineStats {
                added: 3,
                deleted: 1
            })
        );
        assert!(!stats.contains_key("img.png"));
        assert_eq!(stats.len(), 1);
    }

    #[test]
    fn detached_head_has_no_branch() {
        let s = parse_porcelain_v2(b"# branch.oid abc\0# branch.head (detached)\0");
        assert!(s.branch.is_none());
        assert!(s.files.is_empty());
        assert_eq!(s.include_all(), Some(true));
    }

    #[test]
    fn live_status_on_temp_repo() {
        use std::process::Command;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        let run = |args: &[&str]| {
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
                    .success()
            )
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "commit.gpgsign", "false"]);
        std::fs::write(path.join("a.txt"), "one\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "init"]);
        std::fs::write(path.join("a.txt"), "two\n").unwrap();
        std::fs::write(path.join("b.txt"), "new\n").unwrap();

        let git = Arc::new(crate::find_git().unwrap());
        let s = get_status(git.clone(), path).unwrap();
        assert_eq!(s.branch.as_deref(), Some("main"));
        let mut paths: Vec<_> = s
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.status.kind))
            .collect();
        paths.sort_by(|a, b| a.0.cmp(b.0));
        assert_eq!(
            paths,
            vec![
                ("a.txt", FileStatusKind::Modified),
                ("b.txt", FileStatusKind::Untracked)
            ]
        );
        run(&["config", "status.showUntrackedFiles", "no"]);
        let hidden = get_status_with(
            git.clone(),
            path,
            StatusOptions {
                respect_show_untracked_files: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(hidden.files.len(), 1);
        assert_eq!(hidden.files[0].path, "a.txt");
        // GHD ignores the setting
        assert_eq!(get_status(git.clone(), path).unwrap().files.len(), 2);
        let stats = working_directory_line_stats(git, path, &s).unwrap();
        assert_eq!(
            stats.get("a.txt"),
            Some(&LineStats {
                added: 1,
                deleted: 1
            })
        );
        assert_eq!(
            stats.get("b.txt"),
            Some(&LineStats {
                added: 1,
                deleted: 0
            })
        );
    }
}
