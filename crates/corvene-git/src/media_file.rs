//! Corvene `1335-video-diff` / `1336-audio-diff`: a video or sound file
//! becomes a [`MediaDiff`] of two files on disk. GHD shows them as "This
//! binary file has changed." (`ui/diff/binary-file.tsx`).
//!
//! The working tree's file is used where it is; a committed blob is written
//! once to the media cache (`$TMPDIR/corvene-media/<blob id>.<ext>`, pruned
//! to [`CACHE_BYTES`]), and an LFS pointer is swapped for its local object.
//! The probe that reads a file (`corvene_media::probe`) is set by the
//! dispatcher, so this crate stays free of decoders.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use corvene_models::{Diff, FileStatusKind, MediaDiff, MediaInfo, MediaKind, MediaSide};

use crate::archive::SideRef;
use crate::detect::GitBinary;
use crate::process::GitCommand;

/// Reads what a video or sound file holds; `None` when nothing can.
pub type MediaProber = fn(&Path, MediaKind) -> Option<MediaInfo>;

static PROBER: std::sync::RwLock<Option<MediaProber>> = std::sync::RwLock::new(None);
static VIDEO: AtomicBool = AtomicBool::new(false);
static AUDIO: AtomicBool = AtomicBool::new(false);

/// Set by the dispatcher from the flags; `None` turns both off.
pub fn set_media_diffs(prober: Option<MediaProber>, video: bool, audio: bool) {
    if let Ok(mut slot) = PROBER.write() {
        *slot = prober;
    }
    VIDEO.store(video, Ordering::Relaxed);
    AUDIO.store(audio, Ordering::Relaxed);
}

fn prober() -> Option<MediaProber> {
    PROBER.read().ok().and_then(|p| *p)
}

/// The kind of media `path` is shown as, while its flag is on.
pub fn media_diff_kind(path: &str) -> Option<MediaKind> {
    prober()?;
    let kind = corvene_models::media_kind(path)?;
    let on = match kind {
        MediaKind::Video => VIDEO.load(Ordering::Relaxed),
        MediaKind::Audio => AUDIO.load(Ordering::Relaxed),
    };
    on.then_some(kind)
}

/// The media cache keeps at most this many bytes of committed versions.
pub const CACHE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

fn cache_dir() -> PathBuf {
    std::env::temp_dir().join("corvene-media")
}

/// A diff of the sides of `path`, `None` when neither can be read (git's
/// own diff stays).
pub fn media_diff(
    git: Arc<GitBinary>,
    workdir: &Path,
    path: &str,
    kind: FileStatusKind,
    new: SideRef,
    old: SideRef,
) -> Option<Diff> {
    let media = media_diff_kind(path)?;
    let probe = prober()?;
    let _span = tracing::debug_span!("media diff", path).entered();
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("bin")
        .to_ascii_lowercase();
    let new = if kind == FileStatusKind::Deleted {
        SideRef::Missing
    } else {
        new
    };
    let old = if kind.is_new_or_untracked() {
        SideRef::Missing
    } else {
        old
    };
    let new = locate(&git, workdir, new, &ext);
    let old = locate(&git, workdir, old, &ext);
    if new.is_none() && old.is_none() {
        return None;
    }
    let side = |found: Option<(PathBuf, u64, String)>| {
        let (file, bytes, key) = found?;
        let info = probe(&file, media)?;
        Some(MediaSide {
            file,
            bytes,
            key,
            info,
        })
    };
    let (previous, current) = std::thread::scope(|scope| {
        let previous = scope.spawn(|| side(old));
        let current = side(new);
        (previous.join().ok().flatten(), current)
    });
    if previous.is_none() && current.is_none() {
        return None;
    }
    Some(Diff::Media(Box::new(MediaDiff {
        kind: media,
        previous,
        current,
    })))
}

/// The file a side is read from, its size and its cache key.
fn locate(
    git: &Arc<GitBinary>,
    workdir: &Path,
    side: SideRef,
    ext: &str,
) -> Option<(PathBuf, u64, String)> {
    match side {
        SideRef::Missing => None,
        SideRef::File(path) => {
            // a regular file only: never read through a link (a FIFO hangs)
            let meta = std::fs::symlink_metadata(path).ok()?;
            if !meta.is_file() {
                return None;
            }
            // an LFS pointer left in the working tree: its object
            if meta.len() < 1024
                && let Ok(bytes) = std::fs::read(path)
                && let Some(pointer) = crate::lfs::lfs_pointer(&bytes)
            {
                let object = crate::lfs::lfs_object_path(git.clone(), workdir, &pointer)?;
                return Some((object, pointer.size, format!("lfs:{}", pointer.oid)));
            }
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos());
            Some((
                path.to_path_buf(),
                meta.len(),
                format!("file:{}:{}:{mtime}", path.display(), meta.len()),
            ))
        }
        SideRef::Blob(rev, path) => {
            let spec = format!("{rev}:{path}");
            let oid = crate::handle::open(workdir)
                .ok()
                .and_then(|repo| Some(repo.rev_parse_single(spec.as_str()).ok()?.to_string()))
                .or_else(|| {
                    GitCommand::new(git.clone())
                        .args(["rev-parse", "--verify", "-q", &spec])
                        .current_dir(workdir)
                        .run()
                        .ok()
                        .and_then(|o| o.stdout_string().ok())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                })?;
            let dir = cache_dir();
            let file = dir.join(format!("{oid}.{ext}"));
            if let Ok(meta) = std::fs::metadata(&file) {
                return Some((file, meta.len(), format!("blob:{oid}")));
            }
            let bytes = crate::diff::blob_bytes(git.clone(), workdir, rev, path).ok()?;
            if let Some(pointer) = crate::lfs::lfs_pointer(&bytes) {
                let object = crate::lfs::lfs_object_path(git.clone(), workdir, &pointer)?;
                return Some((object, pointer.size, format!("lfs:{}", pointer.oid)));
            }
            std::fs::create_dir_all(&dir).ok()?;
            let partial = dir.join(format!(".{oid}.{}.part", std::process::id()));
            std::fs::write(&partial, &bytes).ok()?;
            std::fs::rename(&partial, &file).ok()?;
            prune(&dir);
            Some((file, bytes.len() as u64, format!("blob:{oid}")))
        }
    }
}

/// Removes the least recently written files past [`CACHE_BYTES`].
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            Some((meta.modified().ok()?, meta.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= CACHE_BYTES {
        return;
    }
    files.sort();
    for (_, len, path) in files {
        if total <= CACHE_BYTES {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_probe(path: &Path, _kind: MediaKind) -> Option<MediaInfo> {
        let bytes = std::fs::read(path).ok()?;
        bytes.starts_with(b"MEDIA").then(|| MediaInfo {
            container: String::from_utf8_lossy(&bytes).into_owned(),
            ..Default::default()
        })
    }

    #[test]
    fn sides_come_from_the_working_tree_and_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        let git = Arc::new(crate::find_git().unwrap());
        let run = |args: &[&str]| {
            GitCommand::new(git.clone())
                .args(args)
                .current_dir(repo)
                .run()
                .unwrap();
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "T"]);
        std::fs::write(repo.join("clip.mp4"), b"MEDIA old").unwrap();
        run(&["add", "."]);
        run(&["commit", "-qm", "one"]);
        std::fs::write(repo.join("clip.mp4"), b"MEDIA new").unwrap();
        set_media_diffs(Some(fake_probe), true, false);
        assert_eq!(media_diff_kind("a/b.MOV"), Some(MediaKind::Video));
        assert_eq!(media_diff_kind("a.mp3"), None);
        let working = repo.join("clip.mp4");
        let diff = media_diff(
            git.clone(),
            repo,
            "clip.mp4",
            FileStatusKind::Modified,
            SideRef::File(&working),
            SideRef::Blob("HEAD", "clip.mp4"),
        )
        .unwrap();
        let Diff::Media(m) = diff else { panic!() };
        assert_eq!(m.current.as_ref().unwrap().info.container, "MEDIA new");
        let old = m.previous.unwrap();
        assert_eq!(old.info.container, "MEDIA old");
        assert!(old.file.starts_with(cache_dir()));
        assert!(old.key.starts_with("blob:"));
        // nothing readable: git's own diff stays
        std::fs::write(repo.join("clip.mp4"), b"junk").unwrap();
        assert!(
            media_diff(
                git,
                repo,
                "clip.mp4",
                FileStatusKind::New,
                SideRef::File(&working),
                SideRef::Missing,
            )
            .is_none()
        );
        set_media_diffs(None, false, false);
    }
}
