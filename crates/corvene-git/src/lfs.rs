//! Git LFS pointer files and their local objects, read without git-lfs.
//!
//! A file stored in Git LFS is committed as a small pointer (`version
//! https://git-lfs.github.com/spec/v1`, `oid sha256:…`, `size …`); git-lfs
//! keeps the contents in `<git dir>/lfs/objects/<oid[0..2]>/<oid[2..4]>/<oid>`
//! (or under `lfs.storage`) once downloaded. GHD diffs the pointers: an LFS
//! image shows as three changed text lines.
//!
//! Corvene `795-lfs-image-previews` ([`resolve_lfs_images`]): an image whose
//! sides are pointers shows the images when their objects are local, else
//! the pointer diff with a note that the contents are not downloaded.
//!
//! Corvene `796-lfs-text-diff` ([`resolve_lfs_text`]): any other file whose
//! sides are pointers with local objects diffs those contents (`git diff
//! --no-index`), its lines not selectable (the index holds the pointers).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use corvene_models::{Diff, FileStatusKind};

use crate::detect::GitBinary;

/// A pointer is at most this large (git-lfs' own limit).
const MAX_POINTER_BYTES: usize = 1024;

/// What an LFS pointer file names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LfsPointer {
    /// The SHA-256 of the contents, 64 lowercase hex digits.
    pub oid: String,
    /// The contents' size in bytes.
    pub size: u64,
}

/// `bytes` as an LFS pointer file, when they are one.
pub fn lfs_pointer(bytes: &[u8]) -> Option<LfsPointer> {
    if bytes.len() > MAX_POINTER_BYTES {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let mut lines = text.lines();
    let version = lines.next()?.strip_prefix("version ")?;
    if !version.starts_with("https://git-lfs.github.com/spec/")
        && !version.starts_with("https://hawser.github.com/spec/")
    {
        return None;
    }
    let (mut oid, mut size) = (None, None);
    for line in lines {
        if let Some(hex) = line.strip_prefix("oid sha256:") {
            oid = Some(hex.trim().to_string());
        } else if let Some(n) = line.strip_prefix("size ") {
            size = n.trim().parse().ok();
        }
    }
    let oid = oid.filter(|o| o.len() == 64 && o.bytes().all(|b| b.is_ascii_hexdigit()))?;
    Some(LfsPointer {
        oid: oid.to_ascii_lowercase(),
        size: size?,
    })
}

/// Corvene (`1104-lfs-server-authentication`): the Git LFS server `remote`
/// uses (lfs.url, `.lfsconfig`, `remote.<name>.lfsurl` or the remote's own),
/// from `git lfs env`.
pub fn lfs_endpoint(git: Arc<GitBinary>, workdir: &Path, remote: &str) -> Option<String> {
    let out = crate::process::GitCommand::new(git)
        .args(["lfs", "env"])
        .current_dir(workdir)
        .run()
        .ok()?
        .stdout_string()
        .ok()?;
    parse_lfs_endpoint(&out, remote)
}

/// `Endpoint (<remote>)=<url> (auth=…)` for a remote other than the default
/// one, else `Endpoint=<url> (auth=…)`.
fn parse_lfs_endpoint(env: &str, remote: &str) -> Option<String> {
    let named = format!("Endpoint ({remote})=");
    let value = env
        .lines()
        .find_map(|line| line.strip_prefix(named.as_str()))
        .or_else(|| env.lines().find_map(|line| line.strip_prefix("Endpoint=")))?;
    value.split_whitespace().next().map(str::to_string)
}

/// git-lfs' storage directory: `lfs.storage` (relative to the git dir) or
/// `<common git dir>/lfs`.
fn lfs_storage(git: Arc<GitBinary>, workdir: &Path) -> PathBuf {
    let common = crate::paths::common_dir(workdir);
    match crate::config_value(git, workdir, "lfs.storage") {
        Some(dir) if Path::new(&dir).is_absolute() => PathBuf::from(dir),
        Some(dir) => common.join(dir),
        None => common.join("lfs"),
    }
}

/// The file holding the downloaded contents `pointer` names; `None` when
/// they are not local (or do not have the size the pointer records).
pub fn lfs_object_path(
    git: Arc<GitBinary>,
    workdir: &Path,
    pointer: &LfsPointer,
) -> Option<PathBuf> {
    let path = lfs_storage(git, workdir)
        .join("objects")
        .join(&pointer.oid[0..2])
        .join(&pointer.oid[2..4])
        .join(&pointer.oid);
    let meta = std::fs::metadata(&path).ok()?;
    (meta.len() == pointer.size).then_some(path)
}

/// The downloaded contents `pointer` names ([`lfs_object_path`]).
pub fn lfs_object(git: Arc<GitBinary>, workdir: &Path, pointer: &LfsPointer) -> Option<Vec<u8>> {
    std::fs::read(lfs_object_path(git, workdir, pointer)?).ok()
}

/// Contents larger than this are not diffed (`796-lfs-text-diff`).
pub const LFS_TEXT_DIFF_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// One side of a file for [`resolve_lfs_text`].
pub enum LfsSide {
    /// A file on disk (the working copy): itself, or its object when it is
    /// a pointer.
    File(PathBuf),
    /// A committed blob: the object it points to.
    Blob(Vec<u8>),
}

/// The file holding one side's contents: a committed pointer's object, a
/// working copy that is a pointer's object, or the working copy itself;
/// `None` when it is not local or larger than `max`.
fn side_path(git: &Arc<GitBinary>, workdir: &Path, side: LfsSide, max: u64) -> Option<PathBuf> {
    let object = |bytes: &[u8]| {
        let pointer = lfs_pointer(bytes)?;
        lfs_object_path(git.clone(), workdir, &pointer)
    };
    let path = match side {
        LfsSide::Blob(bytes) => object(&bytes)?,
        LfsSide::File(path) => {
            let meta = std::fs::metadata(&path).ok()?;
            if meta.len() as usize <= MAX_POINTER_BYTES
                && let Ok(bytes) = std::fs::read(&path)
                && lfs_pointer(&bytes).is_some()
            {
                object(&bytes)?
            } else {
                path
            }
        }
    };
    let size = std::fs::metadata(&path).ok()?.len();
    (size <= max).then_some(path)
}

/// Corvene `1323-archive-diff`: a pointer diff of an archive stored in Git
/// LFS as the archive diff of its objects (the working copy for a working
/// file that is not a pointer). Anything else, or a side whose object is
/// not local, keeps `diff`.
pub fn resolve_lfs_archive(
    git: Arc<GitBinary>,
    workdir: &Path,
    path: &str,
    kind: FileStatusKind,
    diff: Diff,
    current: impl FnOnce() -> Option<LfsSide>,
    previous: impl FnOnce() -> Option<LfsSide>,
) -> Diff {
    if !crate::archive::read_archives()
        || crate::archive::archive_format(path).is_none()
        || !diff.hunks().is_some_and(is_pointer_diff)
    {
        return diff;
    }
    let max = crate::archive::MAX_ARCHIVE_BYTES;
    let side = |wanted: bool, side: Option<LfsSide>| -> Option<Option<PathBuf>> {
        if !wanted {
            return Some(None);
        }
        Some(Some(side_path(&git, workdir, side?, max)?))
    };
    let (Some(new), Some(old)) = (
        side(kind != FileStatusKind::Deleted, current()),
        side(!kind.is_new_or_untracked(), previous()),
    ) else {
        return diff;
    };
    fn side_ref(p: &Option<PathBuf>) -> crate::archive::SideRef<'_> {
        match p {
            Some(p) => crate::archive::SideRef::File(p),
            None => crate::archive::SideRef::Missing,
        }
    }
    crate::archive::archive_diff(git.clone(), workdir, path, side_ref(&new), side_ref(&old))
        .unwrap_or(diff)
}

/// Corvene `796-lfs-text-diff`: a pointer diff of a file stored in Git LFS
/// as the diff of the contents (`git diff --no-index` of the objects, or of
/// the working copy), marked `DiffWarnings::lfs_contents`. `current` and
/// `previous` are the new and old sides (`None` for a new or deleted file).
/// Any other diff, a side whose contents are not local or too large, or
/// contents git takes for binary keep `diff` as it is.
pub fn resolve_lfs_text(
    git: Arc<GitBinary>,
    workdir: &Path,
    kind: FileStatusKind,
    diff: Diff,
    current: impl FnOnce() -> Option<LfsSide>,
    previous: impl FnOnce() -> Option<LfsSide>,
) -> Diff {
    if !diff.hunks().is_some_and(is_pointer_diff) {
        return diff;
    }
    let file = |side: LfsSide| side_path(&git, workdir, side, LFS_TEXT_DIFF_MAX_BYTES);
    let side = |wanted: bool, side: Option<LfsSide>| -> Option<Option<PathBuf>> {
        if !wanted {
            return Some(None);
        }
        Some(Some(file(side?)?))
    };
    let (Some(new), Some(old)) = (
        side(kind != FileStatusKind::Deleted, current()),
        side(!kind.is_new_or_untracked(), previous()),
    ) else {
        return diff;
    };
    let null = PathBuf::from("/dev/null");
    // run outside the repository: inside it `--no-index` still applies the
    // LFS clean filter to the working copy, which turns it back into a pointer
    let Ok(out) = crate::process::GitCommand::new(git)
        .args([
            "diff",
            "--no-index",
            "--no-ext-diff",
            "--patch-with-raw",
            "-z",
            "--no-color",
            "--",
        ])
        .arg(old.unwrap_or_else(|| null.clone()))
        .arg(new.unwrap_or(null))
        .current_dir(std::env::temp_dir())
        .allow_exit_code(1)
        .run()
    else {
        return diff;
    };
    match crate::diff::parse_raw_diff(&out.stdout) {
        Diff::Text {
            hunks,
            mut warnings,
        } => {
            warnings.lfs_contents = true;
            Diff::Text { hunks, warnings }
        }
        Diff::LargeText {
            hunks,
            mut warnings,
        } => {
            warnings.lfs_contents = true;
            Diff::LargeText { hunks, warnings }
        }
        _ => diff,
    }
}

/// Whether a text diff's lines are those of LFS pointer files.
fn is_pointer_diff(hunks: &[corvene_models::DiffHunk]) -> bool {
    hunks.iter().flat_map(|h| h.lines.iter()).any(|l| {
        l.text.starts_with("oid sha256:")
            || l.text
                .starts_with("version https://git-lfs.github.com/spec/")
    })
}

/// Corvene `795-lfs-image-previews`: `diff` of the image at `path` with
/// sides stored as LFS pointers shown as images. `current` / `previous`
/// read the new and old sides' bytes (the working file or a blob): a side
/// that is a pointer becomes its local object. A pointer diff whose
/// objects are not all local stays a text diff, marked
/// `DiffWarnings::lfs_not_downloaded`. Any other diff is returned as it is.
pub fn resolve_lfs_images(
    git: Arc<GitBinary>,
    workdir: &Path,
    path: &str,
    kind: FileStatusKind,
    diff: Diff,
    current: impl FnOnce() -> Option<Vec<u8>>,
    previous: impl FnOnce() -> Option<Vec<u8>>,
) -> Diff {
    let Some(media_type) = crate::diff::binary_media_type(path) else {
        return diff;
    };
    let resolve = |bytes: Vec<u8>| -> (bool, Option<Vec<u8>>) {
        match lfs_pointer(&bytes) {
            Some(pointer) => (true, lfs_object(git.clone(), workdir, &pointer)),
            None => (false, Some(bytes)),
        }
    };
    match diff {
        Diff::Text {
            hunks,
            mut warnings,
        } if is_pointer_diff(&hunks) => {
            let current = (kind != FileStatusKind::Deleted)
                .then(current)
                .flatten()
                .map(resolve);
            let previous = (!kind.is_new_or_untracked())
                .then(previous)
                .flatten()
                .map(resolve);
            let missing = |side: &Option<(bool, Option<Vec<u8>>)>, expected: bool| {
                expected && side.as_ref().is_none_or(|(_, bytes)| bytes.is_none())
            };
            if missing(&current, kind != FileStatusKind::Deleted)
                || missing(&previous, !kind.is_new_or_untracked())
            {
                warnings.lfs_not_downloaded = true;
                return Diff::Text { hunks, warnings };
            }
            crate::diff::image_diff_as(
                media_type,
                kind,
                || current.and_then(|(_, bytes)| bytes),
                || previous.and_then(|(_, bytes)| bytes),
            )
        }
        Diff::Image { previous, current } => {
            // git-lfs not installed: git compares a pointer with the image;
            // a pointer whose object is not local has nothing to draw
            let swap = |blob: Option<corvene_models::ImageBlob>| {
                let mut blob = blob?;
                if let Some(pointer) = lfs_pointer(&blob.bytes) {
                    blob.bytes = lfs_object(git.clone(), workdir, &pointer)?;
                    // `1326-more-image-formats`; a side that does not decode
                    // shows as one that cannot be drawn
                    crate::diff::decode_image_blob(&mut blob);
                }
                Some(blob)
            };
            Diff::Image {
                previous: swap(previous),
                current: swap(current),
            }
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn lfs_endpoints_from_git_lfs_env() {
        let env = "git-lfs/3.8.0 (GitHub; darwin arm64; go 1.27.0)\n\
                   git version 2.54.0\n\n\
                   Endpoint=http://127.0.0.1:18767/repo.git/info/lfs (auth=basic)\n\
                   Endpoint (fork)=https://lfs.corp/fork.git/info/lfs (auth=none)\n\
                   LocalWorkingDir=/w\n";
        assert_eq!(
            parse_lfs_endpoint(env, "origin").as_deref(),
            Some("http://127.0.0.1:18767/repo.git/info/lfs")
        );
        assert_eq!(
            parse_lfs_endpoint(env, "fork").as_deref(),
            Some("https://lfs.corp/fork.git/info/lfs")
        );
        assert_eq!(parse_lfs_endpoint("LocalWorkingDir=/w\n", "origin"), None);
    }

    use super::*;

    const OID: &str = "4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393";

    #[test]
    fn parses_pointers() {
        let text =
            format!("version https://git-lfs.github.com/spec/v1\noid sha256:{OID}\nsize 12345\n");
        assert_eq!(
            lfs_pointer(text.as_bytes()),
            Some(LfsPointer {
                oid: OID.to_string(),
                size: 12345
            })
        );
        assert_eq!(lfs_pointer(b"\x89PNG\r\n"), None);
        assert_eq!(
            lfs_pointer(b"version https://example.com\noid sha256:x\nsize 1\n"),
            None
        );
        let short = "version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 1\n";
        assert_eq!(lfs_pointer(short.as_bytes()), None);
    }

    #[test]
    fn lfs_text_diffs_the_contents() {
        use std::process::Command;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(path)
                .status()
                .unwrap()
                .success()
        );
        let old = b"one\ntwo\nthree\n";
        let old_oid = "1".repeat(64);
        let object = path.join(".git/lfs/objects/11/11");
        std::fs::create_dir_all(&object).unwrap();
        std::fs::write(object.join(&old_oid), old).unwrap();
        let pointer = |oid: &str, size: usize| {
            format!("version https://git-lfs.github.com/spec/v1\noid sha256:{oid}\nsize {size}\n")
        };
        // the working copy holds the smudged contents
        std::fs::write(path.join("data.txt"), "one\nTWO\nthree\n").unwrap();
        let git = Arc::new(crate::find_git().unwrap());
        let pointer_diff = crate::parse_unified(&format!(
            "@@ -1,3 +1,3 @@\n version https://git-lfs.github.com/spec/v1\n-oid sha256:{old_oid}\n-size {}\n+oid sha256:{}\n+size 14\n",
            old.len(),
            "2".repeat(64)
        ));
        let diff = resolve_lfs_text(
            git.clone(),
            path,
            FileStatusKind::Modified,
            pointer_diff.clone(),
            || Some(LfsSide::File(path.join("data.txt"))),
            || Some(LfsSide::Blob(pointer(&old_oid, old.len()).into_bytes())),
        );
        let changed: Vec<String> = diff
            .hunks()
            .unwrap()
            .iter()
            .flat_map(|h| h.lines.iter())
            .filter(|l| {
                matches!(
                    l.kind,
                    corvene_models::DiffLineKind::Add | corvene_models::DiffLineKind::Delete
                )
            })
            .map(|l| l.text.clone())
            .collect();
        assert_eq!(changed, ["two", "TWO"]);
        assert!(diff.warnings().unwrap().lfs_contents);
        // the old contents are not downloaded: the pointer diff stays
        let kept = resolve_lfs_text(
            git,
            path,
            FileStatusKind::Modified,
            pointer_diff.clone(),
            || Some(LfsSide::File(path.join("data.txt"))),
            || Some(LfsSide::Blob(pointer(&"3".repeat(64), 3).into_bytes())),
        );
        assert_eq!(kept, pointer_diff);
    }

    #[test]
    fn resolves_local_objects_into_an_image_diff() {
        use std::process::Command;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(path)
                .status()
                .unwrap()
                .success()
        );
        let png = b"\x89PNG\r\n\x1a\nnot really".to_vec();
        let object = path
            .join(".git/lfs/objects")
            .join(&OID[0..2])
            .join(&OID[2..4]);
        std::fs::create_dir_all(&object).unwrap();
        std::fs::write(object.join(OID), &png).unwrap();
        let pointer = format!(
            "version https://git-lfs.github.com/spec/v1\noid sha256:{OID}\nsize {}\n",
            png.len()
        );
        let git = Arc::new(crate::find_git().unwrap());
        let diff = crate::parse_unified(&format!(
            "@@ -0,0 +1,3 @@\n+{}",
            pointer.lines().collect::<Vec<_>>().join("\n+")
        ));
        let resolved = resolve_lfs_images(
            git.clone(),
            path,
            "a.png",
            FileStatusKind::New,
            diff.clone(),
            || Some(pointer.clone().into_bytes()),
            || None,
        );
        match resolved {
            Diff::Image { previous, current } => {
                assert!(previous.is_none());
                assert_eq!(current.unwrap().bytes, png);
            }
            other => panic!("image expected, got {other:?}"),
        }
        // not downloaded: the pointer diff with the note
        let other = pointer.replace(OID, &"0".repeat(64));
        match resolve_lfs_images(
            git,
            path,
            "a.png",
            FileStatusKind::New,
            diff,
            || Some(other.into_bytes()),
            || None,
        ) {
            Diff::Text { warnings, .. } => assert!(warnings.lfs_not_downloaded),
            other => panic!("text expected, got {other:?}"),
        }
    }
}
