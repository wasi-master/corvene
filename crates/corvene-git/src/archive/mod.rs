//! Corvene `1323-archive-diff`: a binary change of an archive (zip and its
//! relatives, tar with or without compression) or of a single compressed
//! file (`.gz`, `.bz2`, `.xz`, `.zst`) read as the list of its entries,
//! each of which diffs on its own: text through `git diff --no-index`,
//! images as image diffs, archives inside archives as archives again.
//! GitHub Desktop shows all of these as binary files
//! (`app/src/lib/git/diff.ts` `getBinaryPaths`,
//! `app/src/ui/diff/binary-file.tsx`).
//!
//! Only binary diffs get here: a repository whose `.gitattributes` gives
//! archives a `diff=<driver>` with a `textconv` keeps git's text diff.
//!
//! Each side is pinned when the archive is listed (a blob's object id, or
//! a file's size and mtime), so entry diffs read later see the same bytes.
//! Listed archives are cached ([`CACHE_BUDGET`]); entries are unpacked on
//! demand and never written anywhere.

mod compare;
mod read;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use corvene_models::{
    ArchiveDiff, ArchiveEntryKind, ArchiveEntryStatus, ArchiveFormat, ArchiveRoot, ArchiveSource,
    ArchiveSummary, Diff, FileStatusKind,
};

use crate::detect::GitBinary;
use crate::process::GitCommand;

use read::OpenArchive;

/// Larger archives stay binary.
pub const MAX_ARCHIVE_BYTES: u64 = 256 << 20;
/// Entries listed per archive; the rest are left out (`truncated`).
pub const MAX_ENTRIES: usize = 50_000;
/// Bytes unpacked while listing one compressed archive (a zip bomb stops
/// here, `truncated`).
pub const MAX_UNPACKED: u64 = 1 << 30;
/// Largest entry unpacked to be diffed, saved or opened.
pub const MAX_ENTRY_BYTES: u64 = 64 << 20;
/// Archives inside archives opened, the outermost one not counted.
pub const MAX_DEPTH: usize = 4;
/// Memory the listed archives may hold.
pub const CACHE_BUDGET: usize = 384 << 20;

/// Set by the dispatcher from the flag.
static READ_ARCHIVES: AtomicBool = AtomicBool::new(false);

/// Turn archive diffs on or off.
pub fn set_read_archives(enabled: bool) {
    READ_ARCHIVES.store(enabled, Ordering::Relaxed);
}

/// Whether archive diffs are on.
pub fn read_archives() -> bool {
    READ_ARCHIVES.load(Ordering::Relaxed)
}

/// The archive format a path's extension names.
pub fn archive_format(path: &str) -> Option<ArchiveFormat> {
    // `1324-document-diff`: an EPUB file is shown as a book
    if crate::document_file::is_binary_document(path) {
        return None;
    }
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    let (stem, ext) = name.rsplit_once('.')?;
    if stem.is_empty() {
        return None;
    }
    let tarred = stem.ends_with(".tar");
    Some(match ext {
        "zip" | "jar" | "war" | "ear" | "apk" | "aar" | "aab" | "whl" | "nupkg" | "vsix"
        | "xpi" | "ipa" | "crx" | "epub" | "egg" | "kmz" | "sublime-package" => ArchiveFormat::Zip,
        "tar" => ArchiveFormat::Tar,
        "tgz" | "taz" | "crate" => ArchiveFormat::TarGz,
        "tbz" | "tbz2" | "tb2" => ArchiveFormat::TarBz2,
        "txz" => ArchiveFormat::TarXz,
        "tzst" => ArchiveFormat::TarZst,
        "gz" if tarred => ArchiveFormat::TarGz,
        "bz2" if tarred => ArchiveFormat::TarBz2,
        "xz" if tarred => ArchiveFormat::TarXz,
        "zst" if tarred => ArchiveFormat::TarZst,
        "gz" => ArchiveFormat::Gzip,
        "bz2" => ArchiveFormat::Bzip2,
        "xz" => ArchiveFormat::Xz,
        "zst" => ArchiveFormat::Zstd,
        _ => return None,
    })
}

/// Where the hook finds one side of a changed archive.
#[derive(Clone, Copy, Debug)]
pub enum SideRef<'a> {
    /// The side does not exist (a new or a deleted file).
    Missing,
    /// A file on disk.
    File(&'a Path),
    /// `<rev>:<path>` (an empty `rev` names the index).
    Blob(&'a str, &'a str),
}

struct Cache {
    items: VecDeque<(ArchiveSource, Arc<OpenArchive>, usize)>,
    used: usize,
}

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| {
    Mutex::new(Cache {
        items: VecDeque::new(),
        used: 0,
    })
});

fn cached(source: &ArchiveSource) -> Option<Arc<OpenArchive>> {
    let mut cache = CACHE.lock().ok()?;
    let ix = cache.items.iter().position(|(s, _, _)| s == source)?;
    let item = cache.items.remove(ix)?;
    let archive = item.1.clone();
    cache.items.push_back(item);
    Some(archive)
}

fn remember(source: ArchiveSource, archive: Arc<OpenArchive>) {
    let cost = archive.cost();
    if cost > CACHE_BUDGET {
        return;
    }
    let Ok(mut cache) = CACHE.lock() else {
        return;
    };
    if let Some(ix) = cache.items.iter().position(|(s, _, _)| *s == source)
        && let Some((_, _, old)) = cache.items.remove(ix)
    {
        cache.used -= old;
    }
    while cache.used + cost > CACHE_BUDGET {
        let Some((_, _, freed)) = cache.items.pop_front() else {
            break;
        };
        cache.used -= freed;
    }
    cache.used += cost;
    cache.items.push_back((source, archive, cost));
}

/// Drops every listed archive (tests, and a flag turned off).
pub fn clear_cache() {
    if let Ok(mut cache) = CACHE.lock() {
        cache.items.clear();
        cache.used = 0;
    }
}

fn mtime_ns(meta: &std::fs::Metadata) -> i128 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as i128)
}

/// Pins a side: a file's size and mtime, a blob's id. `Ok(None)` for a
/// missing side, `Err` when it cannot be read or is too large.
fn pin(git: &Arc<GitBinary>, workdir: &Path, side: SideRef) -> Result<Option<ArchiveRoot>, ()> {
    match side {
        SideRef::Missing => Ok(None),
        SideRef::File(path) => {
            // a regular file only: never read through a link (a FIFO hangs)
            let meta = std::fs::symlink_metadata(path).map_err(|_| ())?;
            if !meta.is_file() || meta.len() > MAX_ARCHIVE_BYTES {
                return Err(());
            }
            Ok(Some(ArchiveRoot::File {
                path: path.to_path_buf(),
                len: meta.len(),
                mtime_ns: mtime_ns(&meta),
            }))
        }
        SideRef::Blob(rev, path) => {
            let spec = format!("{rev}:{path}");
            let in_process = crate::handle::open(workdir).ok().and_then(|repo| {
                let id = repo.rev_parse_single(spec.as_str()).ok()?;
                let header = id.header().ok()?;
                Some((
                    header.kind() == gix::object::Kind::Blob,
                    header.size(),
                    id.to_string(),
                ))
            });
            let (blob, size, oid) = match in_process {
                Some(found) => found,
                None => {
                    let oid = GitCommand::new(git.clone())
                        .args(["rev-parse", "--verify", "-q", &spec])
                        .current_dir(workdir)
                        .run()
                        .ok()
                        .and_then(|o| o.stdout_string().ok())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .ok_or(())?;
                    let size = GitCommand::new(git.clone())
                        .args(["cat-file", "-s", &oid])
                        .current_dir(workdir)
                        .run()
                        .ok()
                        .and_then(|o| o.stdout_string().ok())
                        .and_then(|s| s.trim().parse::<u64>().ok())
                        .ok_or(())?;
                    (true, size, oid)
                }
            };
            if !blob || size > MAX_ARCHIVE_BYTES {
                return Err(());
            }
            Ok(Some(ArchiveRoot::Blob { oid }))
        }
    }
}

fn read_root(git: &Arc<GitBinary>, workdir: &Path, root: &ArchiveRoot) -> Result<Vec<u8>, String> {
    match root {
        ArchiveRoot::File {
            path,
            len,
            mtime_ns: stamp,
        } => {
            let meta = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
            if !meta.is_file() || meta.len() != *len || mtime_ns(&meta) != *stamp {
                return Err("The file changed on disk.".into());
            }
            let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
            if bytes.len() as u64 != *len {
                return Err("The file changed on disk.".into());
            }
            Ok(bytes)
        }
        ArchiveRoot::Blob { oid } => {
            let in_process = crate::handle::open(workdir).ok().and_then(|repo| {
                let id = gix::ObjectId::from_hex(oid.as_bytes()).ok()?;
                let object = repo.find_object(id).ok()?;
                (object.kind == gix::object::Kind::Blob).then(|| object.detach().data)
            });
            match in_process {
                Some(bytes) => Ok(bytes),
                None => GitCommand::new(git.clone())
                    .args(["cat-file", "blob", oid])
                    .current_dir(workdir)
                    .run()
                    .map(|o| o.stdout)
                    .map_err(|e| e.to_string()),
            }
        }
    }
}

/// The listed archive at `source`, from the cache or read now.
fn open(
    git: &Arc<GitBinary>,
    workdir: &Path,
    source: &ArchiveSource,
) -> Result<Arc<OpenArchive>, String> {
    if source.chain.len() > MAX_DEPTH {
        return Err(format!(
            "Archives nested more than {MAX_DEPTH} levels deep are not opened."
        ));
    }
    if let Some(archive) = cached(source) {
        return Ok(archive);
    }
    let bytes: Arc<[u8]> = match source.chain.split_last() {
        None => Arc::from(read_root(git, workdir, &source.root)?),
        Some((entry, parents)) => {
            let parent = ArchiveSource {
                root: source.root.clone(),
                name: source.name.clone(),
                chain: parents.to_vec(),
            };
            open(git, workdir, &parent)?.read(entry)?
        }
    };
    let archive = Arc::new(OpenArchive::parse(source.level_name(), bytes)?);
    remember(source.clone(), archive.clone());
    Ok(archive)
}

fn summary(source: ArchiveSource, archive: &OpenArchive) -> ArchiveSummary {
    ArchiveSummary {
        source,
        format: archive.format,
        bytes: archive.bytes,
        unpacked: archive.entries.iter().map(|(_, m)| m.size).sum(),
        entry_count: archive.entries.len(),
        comment: archive.comment.clone(),
    }
}

/// The diff of two versions of an archive (a version that is `None` does
/// not exist). A version that does not read gives a diff with
/// `unreadable` set and no entries.
pub fn diff_sources(
    git: &Arc<GitBinary>,
    workdir: &Path,
    old: Option<ArchiveSource>,
    new: Option<ArchiveSource>,
) -> ArchiveDiff {
    let name = new
        .as_ref()
        .or(old.as_ref())
        .map(|s| s.level_name().to_string())
        .unwrap_or_default();
    let fallback = archive_format(&name).unwrap_or(ArchiveFormat::Zip);
    let unreadable = |reason: String| ArchiveDiff {
        format: fallback,
        old: None,
        new: None,
        entries: Vec::new(),
        single: None,
        truncated: false,
        unreadable: Some(reason),
    };
    let open_side = |source: &Option<ArchiveSource>| -> Result<Option<Arc<OpenArchive>>, String> {
        source.as_ref().map(|s| open(git, workdir, s)).transpose()
    };
    let new_archive = match open_side(&new) {
        Ok(a) => a,
        Err(e) => return unreadable(e),
    };
    let old_archive = match open_side(&old) {
        Ok(a) => a,
        Err(e) => return unreadable(format!("The previous version: {e}")),
    };
    let empty = Vec::new();
    let mut entries = compare::compare(
        old_archive.as_ref().map_or(&empty, |a| &a.entries),
        new_archive.as_ref().map_or(&empty, |a| &a.entries),
    );
    let single = new_archive
        .as_ref()
        .or(old_archive.as_ref())
        .and_then(|a| a.single.clone());
    // one compressed file: both sides name their file the same, so a
    // renamed `.gz` still pairs its contents
    if single.is_some()
        && let [a, b] = &entries[..]
        && a.status != b.status
    {
        {
            let (gone, came) = if a.status == ArchiveEntryStatus::Deleted {
                (a.clone(), b.clone())
            } else {
                (b.clone(), a.clone())
            };
            let same = gone
                .old
                .as_ref()
                .zip(came.new.as_ref())
                .is_some_and(|(o, n)| o.same_contents(n));
            entries = vec![corvene_models::ArchiveEntryChange {
                path: came.path.clone(),
                status: if same {
                    ArchiveEntryStatus::Unchanged
                } else {
                    ArchiveEntryStatus::Modified
                },
                old_path: Some(gone.path.clone()),
                old: gone.old,
                new: came.new,
            }];
        }
    }
    let format = new_archive
        .as_ref()
        .or(old_archive.as_ref())
        .map_or(fallback, |a| a.format);
    let truncated = new_archive.as_ref().is_some_and(|a| a.truncated)
        || old_archive.as_ref().is_some_and(|a| a.truncated);
    ArchiveDiff {
        format,
        old: old.zip(old_archive.as_deref()).map(|(s, a)| summary(s, a)),
        new: new.zip(new_archive.as_deref()).map(|(s, a)| summary(s, a)),
        entries,
        single,
        truncated,
        unreadable: None,
    }
}

/// A binary change of `path` as an archive diff; `None` when the flag is
/// off, `path` is not an archive, or a side is too large or not there (the
/// caller keeps its binary diff).
pub fn archive_diff(
    git: Arc<GitBinary>,
    workdir: &Path,
    path: &str,
    new: SideRef,
    old: SideRef,
) -> Option<Diff> {
    if !read_archives() {
        return None;
    }
    archive_format(path)?;
    let source = |root: ArchiveRoot| ArchiveSource {
        root,
        name: path.to_string(),
        chain: Vec::new(),
    };
    let new = pin(&git, workdir, new).ok()?.map(source);
    let old = pin(&git, workdir, old).ok()?.map(source);
    if new.is_none() && old.is_none() {
        return None;
    }
    Some(Diff::Archive(Box::new(diff_sources(
        &git, workdir, old, new,
    ))))
}

/// One entry's diff, and the lines of its two versions for expanding
/// hunks (text only).
#[derive(Debug)]
pub struct EntryDiff {
    pub diff: Diff,
    pub new_lines: Option<Vec<String>>,
    pub old_lines: Option<Vec<String>>,
}

/// Which version of an entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EntrySide {
    Old,
    New,
}

/// The contents of one version of the entry at `path` of `level`.
pub fn entry_bytes(
    git: &Arc<GitBinary>,
    workdir: &Path,
    level: &ArchiveDiff,
    path: &str,
    side: EntrySide,
) -> Result<Arc<[u8]>, String> {
    let entry = level
        .entry(path)
        .ok_or_else(|| "The entry is not in the archive.".to_string())?;
    let (summary, entry_path, meta) = match side {
        EntrySide::New => (level.new.as_ref(), entry.path.as_str(), entry.new.as_ref()),
        EntrySide::Old => (
            level.old.as_ref(),
            entry.previous_path(),
            entry.old.as_ref(),
        ),
    };
    let (Some(summary), Some(meta)) = (summary, meta) else {
        return Err("That version of the entry does not exist.".into());
    };
    match &meta.kind {
        ArchiveEntryKind::File => {}
        ArchiveEntryKind::Symlink(target) | ArchiveEntryKind::Hardlink(target) => {
            return Ok(Arc::from(target.as_bytes()));
        }
        _ => return Err("The entry is not a file.".into()),
    }
    open(git, workdir, &summary.source)?.read(entry_path)
}

/// The diff of the entry at `path` of `level`.
pub fn entry_diff(
    git: &Arc<GitBinary>,
    workdir: &Path,
    level: &ArchiveDiff,
    path: &str,
    hide_whitespace: bool,
) -> Result<EntryDiff, String> {
    let entry = level
        .entry(path)
        .ok_or_else(|| "The entry is not in the archive.".to_string())?;
    let plain = |m: Option<&corvene_models::ArchiveEntryMeta>| {
        m.is_none_or(|m| m.kind == ArchiveEntryKind::File)
    };
    if !entry.status.is_content_change() || !plain(entry.old.as_ref()) || !plain(entry.new.as_ref())
    {
        // the metadata card says it all
        return Ok(EntryDiff {
            diff: Diff::Empty,
            new_lines: None,
            old_lines: None,
        });
    }
    let has_new = entry.new.is_some();
    let has_old = entry.old.is_some();
    if archive_format(path).is_some() {
        let source = |summary: Option<&ArchiveSummary>, entry: &str, exists: bool| {
            summary.filter(|_| exists).map(|s| s.source.nested(entry))
        };
        let old = source(level.old.as_ref(), entry.previous_path(), has_old);
        let new = source(level.new.as_ref(), &entry.path, has_new);
        let depth = new.as_ref().or(old.as_ref()).map_or(0, |s| s.chain.len());
        if depth <= MAX_DEPTH {
            return Ok(EntryDiff {
                diff: Diff::Archive(Box::new(diff_sources(git, workdir, old, new))),
                new_lines: None,
                old_lines: None,
            });
        }
    }
    let new = has_new
        .then(|| entry_bytes(git, workdir, level, path, EntrySide::New))
        .transpose()?;
    let old = has_old
        .then(|| entry_bytes(git, workdir, level, path, EntrySide::Old))
        .transpose()?;
    let diff =
        crate::diff::no_index_diff(git.clone(), old.as_deref(), new.as_deref(), hide_whitespace)
            .ok_or_else(|| "git could not compare the entry.".to_string())?;
    let kind = match entry.status {
        ArchiveEntryStatus::Added => FileStatusKind::New,
        ArchiveEntryStatus::Deleted => FileStatusKind::Deleted,
        _ => FileStatusKind::Modified,
    };
    let diff = match diff {
        Diff::Binary => binary_entry(
            git,
            path,
            kind,
            old.as_deref(),
            new.as_deref(),
            hide_whitespace,
        ),
        other => other,
    };
    let text = matches!(diff, Diff::Text { .. } | Diff::LargeText { .. });
    Ok(EntryDiff {
        new_lines: new.filter(|_| text).map(|b| crate::diff::file_lines(&b)),
        old_lines: old.filter(|_| text).map(|b| crate::diff::file_lines(&b)),
        diff,
    })
}

/// A binary entry: an image diff for images (and PDF files), a table diff
/// for spreadsheets and databases, a UTF-16 text diff while
/// `1306-utf16-diffs` is on, else binary.
fn binary_entry(
    git: &Arc<GitBinary>,
    path: &str,
    kind: FileStatusKind,
    old: Option<&[u8]>,
    new: Option<&[u8]>,
    hide_whitespace: bool,
) -> Diff {
    // spreadsheets and databases while `1321-table-file-diff` is on
    if let Some(table) = crate::table_file::table_diff(
        path,
        kind,
        || new.map(<[u8]>::to_vec),
        || old.map(<[u8]>::to_vec),
    ) {
        return table;
    }
    // images, and PDF files while `1322-pdf-diff` is on
    if let Some(media_type) = crate::diff::binary_media_type(path) {
        return crate::diff::image_diff_as(
            media_type,
            kind,
            || new.map(<[u8]>::to_vec),
            || old.map(<[u8]>::to_vec),
        );
    }
    if crate::utf16::decode_utf16() {
        let decode = |bytes: Option<&[u8]>| -> Option<Option<String>> {
            match bytes {
                None => Some(None),
                Some([]) => Some(Some(String::new())),
                Some(bytes) => crate::utf16::decode(bytes).map(|(_, text)| Some(text)),
            }
        };
        if let (Some(old), Some(new)) = (decode(old), decode(new))
            && let Some(diff @ (Diff::Text { .. } | Diff::LargeText { .. })) =
                crate::diff::no_index_diff(
                    git.clone(),
                    old.as_deref().map(str::as_bytes),
                    new.as_deref().map(str::as_bytes),
                    hide_whitespace,
                )
        {
            return diff;
        }
    }
    Diff::Binary
}

/// A file name for one version of an entry written out to be opened: the
/// entry's own file name with only safe characters (`a.txt` for
/// `../../a.txt`), the old version marked.
pub fn safe_file_name(path: &str, side: EntrySide) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    let mut clean: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    if clean.is_empty() || clean == "." || clean == ".." {
        clean = "entry".into();
    }
    if side == EntrySide::Old {
        clean = match clean.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() => format!("{stem} (previous).{ext}"),
            _ => format!("{clean} (previous)"),
        };
    }
    clean
}

/// Writes one version of an entry to a new private temporary directory
/// (kept until the app exits) and returns the file's path.
pub fn extract_to_temp(
    git: &Arc<GitBinary>,
    workdir: &Path,
    level: &ArchiveDiff,
    path: &str,
    side: EntrySide,
) -> Result<PathBuf, String> {
    let bytes = entry_bytes(git, workdir, level, path, side)?;
    let dir = tempfile::Builder::new()
        .prefix("corvene-archive-")
        .tempdir()
        .map_err(|e| e.to_string())?
        .keep();
    let file = dir.join(safe_file_name(path, side));
    std::fs::write(&file, &bytes).map_err(|e| e.to_string())?;
    Ok(file)
}

/// `git difftool -y --no-index` on two files, with the repository's
/// `diff.tool` (refused without one, as [`crate::diff::open_difftool`]).
/// Blocks until the tool exits.
pub fn open_difftool_files(
    git: Arc<GitBinary>,
    workdir: &Path,
    old: &Path,
    new: &Path,
) -> crate::error::Result<()> {
    if crate::config_value(git.clone(), workdir, "diff.tool").is_none() {
        return Err(crate::error::GitError::Gix(
            "No diff tool is configured. Set one with git config --global diff.tool <tool>.".into(),
        ));
    }
    GitCommand::new(git)
        .args(["difftool", "-y", "--no-index", "--"])
        .arg(old)
        .arg(new)
        .current_dir(workdir)
        .allow_exit_code(1)
        .run()
        .map(|_| ())
}

#[cfg(test)]
mod tests;
