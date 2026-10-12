//! In-memory caches behind the diff and history selections (Corvene; GHD
//! runs git again for every selection and every refresh).
//!
//! - A commit's changed files and a committed file's diff never change, so
//!   they are kept by SHA ([`changeset`], [`commit_diff`]): selecting a commit
//!   again, or the reload after every refresh, needs no git process.
//! - A working-directory diff is kept with the facts it was computed from
//!   ([`WorkingStamp`]: the status code, `HEAD`, and the size, mtime and inode
//!   of the working file, the checks git itself uses to skip unchanged files);
//!   a refresh or re-selection with the same facts reuses it.
//!
//! Each cache holds the most recently used entries only.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use corvene_models::{ChangesetData, FileStatusKind, WorkingDirectoryFileChange};

use crate::dispatcher::LoadedDiff;

/// A small most-recently-used list with a size budget; linear search is
/// cheaper than hashing for a few dozen keys.
struct Lru<K, V> {
    cap: usize,
    /// Approximate bytes held; the oldest entries go past this.
    budget: usize,
    bytes: usize,
    entries: VecDeque<(K, V, usize)>,
}

impl<K: PartialEq, V: Clone> Lru<K, V> {
    const fn new(cap: usize, budget: usize) -> Self {
        Self {
            cap,
            budget,
            bytes: 0,
            entries: VecDeque::new(),
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    fn get(&mut self, key: &K) -> Option<V> {
        let ix = self.entries.iter().position(|(k, _, _)| k == key)?;
        let entry = self.entries.remove(ix)?;
        let value = entry.1.clone();
        self.entries.push_front(entry);
        Some(value)
    }

    /// Keep `value` (`size` bytes); one larger than a quarter of the budget
    /// is not kept.
    fn insert(&mut self, key: K, value: V, size: usize) {
        if let Some(ix) = self.entries.iter().position(|(k, _, _)| *k == key)
            && let Some((_, _, old)) = self.entries.remove(ix)
        {
            self.bytes -= old;
        }
        if size > self.budget / 4 {
            return;
        }
        self.entries.push_front((key, value, size));
        self.bytes += size;
        while self.entries.len() > self.cap || self.bytes > self.budget {
            let Some((_, _, size)) = self.entries.pop_back() else {
                break;
            };
            self.bytes -= size;
        }
    }
}

const MB: usize = 1024 * 1024;

/// Approximate heap bytes of a loaded diff and its contents.
fn loaded_size((diff, new, old): &LoadedDiff) -> usize {
    let lines = |l: &Option<Arc<Vec<String>>>| {
        l.as_ref()
            .map_or(0, |l| l.iter().map(|s| s.len() + 24).sum::<usize>())
    };
    let diff_bytes = match &**diff {
        corvene_models::Diff::Text { hunks, .. }
        | corvene_models::Diff::LargeText { hunks, .. } => hunks
            .iter()
            .flat_map(|h| &h.lines)
            .map(|l| l.text.len() + 48)
            .sum(),
        corvene_models::Diff::Archive(archive) => {
            archive.entries.iter().map(|e| e.path.len() + 320).sum()
        }
        corvene_models::Diff::Image { previous, current } => {
            previous.as_ref().map_or(0, |i| i.bytes.len())
                + current.as_ref().map_or(0, |i| i.bytes.len())
        }
        corvene_models::Diff::Table { previous, current } => [previous, current]
            .into_iter()
            .flatten()
            .flat_map(|b| &b.sheets)
            .flat_map(|s| &s.rows)
            .flatten()
            .map(|c| c.len() + 24)
            .sum(),
        corvene_models::Diff::Font { previous, current } => {
            previous.as_ref().map_or(0, |f| f.bytes.len())
                + current.as_ref().map_or(0, |f| f.bytes.len())
        }
        corvene_models::Diff::FontSource {
            previous, current, ..
        } => [previous, current]
            .into_iter()
            .flatten()
            .map(|s| s.bytes.len() + s.siblings.values().map(Vec::len).sum::<usize>())
            .sum(),
        _ => 0,
    };
    diff_bytes + lines(new) + lines(old)
}

fn changeset_size(data: &ChangesetData) -> usize {
    data.files.iter().map(|f| f.path.len() + 96).sum()
}

type ChangesetKey = (PathBuf, Vec<String>);
type CommitDiffKey = (PathBuf, Vec<String>, String, bool);

// at most ~72 MB together, far less in practice (a diff's shared contents
// are counted in full although the view holds the same allocation)
static CHANGESETS: LazyLock<Mutex<Lru<ChangesetKey, Arc<ChangesetData>>>> =
    LazyLock::new(|| Mutex::new(Lru::new(256, 8 * MB)));
static COMMIT_DIFFS: LazyLock<Mutex<Lru<CommitDiffKey, LoadedDiff>>> =
    LazyLock::new(|| Mutex::new(Lru::new(64, 32 * MB)));
static WORKING_DIFFS: LazyLock<Mutex<Lru<WorkingKey, LoadedDiff>>> =
    LazyLock::new(|| Mutex::new(Lru::new(64, 32 * MB)));

/// `798-blame`: (workdir, commit or working-tree key, path, options).
type BlameKey = (PathBuf, String, String, corvene_git::BlameOptions);

static BLAMES: LazyLock<Mutex<Lru<BlameKey, Arc<crate::blame::BlameResult>>>> =
    LazyLock::new(|| Mutex::new(Lru::new(32, 16 * MB)));

/// `798-blame`: a finished blame of `path` at `revision` (a commit SHA, or
/// `HEAD` plus a hash of the working file).
pub fn blame(
    workdir: &Path,
    revision: &str,
    path: &str,
    options: corvene_git::BlameOptions,
) -> Option<Arc<crate::blame::BlameResult>> {
    let key = (
        workdir.to_path_buf(),
        revision.to_string(),
        path.to_string(),
        options,
    );
    BLAMES.lock().ok()?.get(&key)
}

pub fn store_blame(
    workdir: &Path,
    revision: &str,
    path: &str,
    options: corvene_git::BlameOptions,
    result: Arc<crate::blame::BlameResult>,
    size: usize,
) {
    if let Ok(mut cache) = BLAMES.lock() {
        let key = (
            workdir.to_path_buf(),
            revision.to_string(),
            path.to_string(),
            options,
        );
        cache.insert(key, result, size);
    }
}

/// The changed files of `shas` (one commit or a range) in `workdir`.
pub fn changeset(workdir: &Path, shas: &[String]) -> Option<Arc<ChangesetData>> {
    let key = (workdir.to_path_buf(), shas.to_vec());
    CHANGESETS.lock().ok()?.get(&key)
}

pub fn store_changeset(workdir: &Path, shas: &[String], data: Arc<ChangesetData>) {
    if let Ok(mut cache) = CHANGESETS.lock() {
        let size = changeset_size(&data);
        cache.insert((workdir.to_path_buf(), shas.to_vec()), data, size);
    }
}

/// The diff of `path` in `shas` (one commit or a range).
pub fn commit_diff(
    workdir: &Path,
    shas: &[String],
    path: &str,
    hide_whitespace: bool,
) -> Option<LoadedDiff> {
    let key = (
        workdir.to_path_buf(),
        shas.to_vec(),
        path.to_string(),
        hide_whitespace,
    );
    COMMIT_DIFFS.lock().ok()?.get(&key)
}

pub fn store_commit_diff(
    workdir: &Path,
    shas: &[String],
    path: &str,
    hide_whitespace: bool,
    diff: LoadedDiff,
) {
    if let Ok(mut cache) = COMMIT_DIFFS.lock() {
        let size = loaded_size(&diff);
        cache.insert(
            (
                workdir.to_path_buf(),
                shas.to_vec(),
                path.to_string(),
                hide_whitespace,
            ),
            diff,
            size,
        );
    }
}

/// What a working-directory diff was computed from; the same stamp means
/// git would print the same diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkingStamp {
    code: String,
    old_path: Option<String>,
    head: Option<String>,
    /// size, mtime (ns), inode of the working file; `None` when missing
    file: Option<(u64, i128, u64)>,
    /// the diff options (whitespace, rename, symlink, as-text, SVG as image,
    /// LFS images, LFS text)
    options: [bool; 8],
}

type WorkingKey = (PathBuf, String, WorkingStamp);

/// Windows: the last write time in 100 ns ticks; a file index needs an open
/// handle, so the third field stays 0.
#[cfg(windows)]
fn file_stamp(path: &Path) -> Option<(u64, i128, u64)> {
    use std::os::windows::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path).ok()?;
    Some((meta.len(), i128::from(meta.last_write_time()), 0))
}

#[cfg(not(windows))]
fn file_stamp(path: &Path) -> Option<(u64, i128, u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path).ok()?;
    let mtime = i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec());
    Some((meta.len(), mtime, meta.ino()))
}

/// The stamp for `file`'s diff, or `None` when it must not be cached
/// (conflicts and submodules depend on more than the file).
pub fn working_stamp(
    workdir: &Path,
    file: &WorkingDirectoryFileChange,
    head: Option<&str>,
    options: [bool; 8],
) -> Option<WorkingStamp> {
    if file.status.submodule || file.status.kind == FileStatusKind::Conflicted {
        return None;
    }
    Some(WorkingStamp {
        code: file.status.code.clone(),
        old_path: file.old_path.clone(),
        head: head.map(str::to_string),
        file: file_stamp(&workdir.join(&file.path)),
        options,
    })
}

/// Corvene `1323-archive-diff`: an archive entry's diff, by the pinned
/// versions of its archive, its path and the hide-whitespace setting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveEntryKey {
    old: Option<corvene_models::ArchiveSource>,
    new: Option<corvene_models::ArchiveSource>,
    path: String,
    hide_whitespace: bool,
}

impl ArchiveEntryKey {
    pub fn new(level: &corvene_models::ArchiveDiff, path: &str, hide_whitespace: bool) -> Self {
        Self {
            old: level.old.as_ref().map(|s| s.source.clone()),
            new: level.new.as_ref().map(|s| s.source.clone()),
            path: path.to_string(),
            hide_whitespace,
        }
    }
}

static ARCHIVE_ENTRIES: LazyLock<Mutex<Lru<ArchiveEntryKey, LoadedDiff>>> =
    LazyLock::new(|| Mutex::new(Lru::new(64, 32 * MB)));

pub fn archive_entry(key: &ArchiveEntryKey) -> Option<LoadedDiff> {
    ARCHIVE_ENTRIES.lock().ok()?.get(key)
}

pub fn store_archive_entry(key: ArchiveEntryKey, diff: LoadedDiff) {
    let size = loaded_size(&diff);
    if let Ok(mut cache) = ARCHIVE_ENTRIES.lock() {
        cache.insert(key, diff, size);
    }
}

/// Forget every cached diff (a flag changed how diffs are read).
pub fn clear_diffs() {
    if let Ok(mut cache) = COMMIT_DIFFS.lock() {
        cache.clear();
    }
    if let Ok(mut cache) = WORKING_DIFFS.lock() {
        cache.clear();
    }
    if let Ok(mut cache) = ARCHIVE_ENTRIES.lock() {
        cache.clear();
    }
}

pub fn working_diff(workdir: &Path, path: &str, stamp: &WorkingStamp) -> Option<LoadedDiff> {
    let key = (workdir.to_path_buf(), path.to_string(), stamp.clone());
    WORKING_DIFFS.lock().ok()?.get(&key)
}

pub fn store_working_diff(workdir: &Path, path: &str, stamp: WorkingStamp, diff: LoadedDiff) {
    // a file written within the last second may change again within the same
    // mtime tick (git's "racily clean" case): do not trust its stamp
    let racy = stamp.file.is_some_and(|(_, mtime, _)| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|now| now.as_nanos() as i128 - mtime < 1_000_000_000)
            .unwrap_or(true)
    });
    if racy {
        return;
    }
    if let Ok(mut cache) = WORKING_DIFFS.lock() {
        let size = loaded_size(&diff);
        cache.insert((workdir.to_path_buf(), path.to_string(), stamp), diff, size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lru_keeps_the_most_recent_entries() {
        let mut lru = Lru::new(2, 100);
        lru.insert(1, "a", 10);
        lru.insert(2, "b", 10);
        assert_eq!(lru.get(&1), Some("a"));
        lru.insert(3, "c", 10);
        // 2 was the least recently used
        assert_eq!(lru.get(&2), None);
        assert_eq!(lru.get(&1), Some("a"));
        assert_eq!(lru.get(&3), Some("c"));
        lru.insert(3, "d", 10);
        assert_eq!(lru.get(&3), Some("d"));
        assert_eq!(lru.entries.len(), 2);
        assert_eq!(lru.bytes, 20);
    }

    #[test]
    fn lru_stays_within_its_budget() {
        let mut lru = Lru::new(10, 100);
        for key in 0..5 {
            lru.insert(key, key, 24);
        }
        // 5 × 24 > 100: the oldest went
        assert_eq!(lru.bytes, 96);
        assert_eq!(lru.get(&0), None);
        assert_eq!(lru.get(&4), Some(4));
        // over a quarter of the budget: not kept
        lru.insert(9, 9, 26);
        assert_eq!(lru.get(&9), None);
    }

    #[test]
    fn a_changed_file_changes_the_stamp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "one\n").unwrap();
        let before = file_stamp(&path);
        std::fs::write(&path, "one\ntwo\n").unwrap();
        assert_ne!(before, file_stamp(&path));
        assert_eq!(file_stamp(&dir.path().join("missing")), None);
    }
}
