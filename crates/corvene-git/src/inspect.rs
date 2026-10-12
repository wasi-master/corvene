//! Corvene `1338-lockfile-diff`, `1339-pretty-diff`, `1340-tensor-diff`
//! and `1341-signing-material-diff`: files whose line diff says little,
//! read into something that does: a lockfile's dependency changes, a
//! minified file pretty printed, a model file's tensors, a certificate's
//! or key's public fields. Each keeps git's own diff as `source` (for the
//! Source view and staging).
//!
//! GitHub Desktop shows all of them as text or binary diffs
//! (`app/src/ui/diff/index.tsx`).

// `Err` hands the diff back unchanged; it is not an error to keep small
#![allow(clippy::result_large_err)]

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use corvene_models::{ArchiveRoot, Diff, FileStatusKind, SigningFile, TensorFile};

use crate::archive::SideRef;
use crate::detect::GitBinary;

static LOCKFILES: AtomicBool = AtomicBool::new(false);
static PRETTY: AtomicBool = AtomicBool::new(false);
static TENSORS: AtomicBool = AtomicBool::new(false);
static SIGNING: AtomicBool = AtomicBool::new(false);

/// Set by the dispatcher from the flags.
pub fn set_inspected_diffs(lockfiles: bool, pretty: bool, tensors: bool, signing: bool) {
    LOCKFILES.store(lockfiles, Ordering::Relaxed);
    PRETTY.store(pretty, Ordering::Relaxed);
    TENSORS.store(tensors, Ordering::Relaxed);
    SIGNING.store(signing, Ordering::Relaxed);
}

/// Larger lockfiles keep git's diff only.
const LOCKFILE_MAX_BYTES: usize = 64 << 20;
/// Larger minified files keep git's diff only.
const PRETTY_MAX_BYTES: usize = 16 << 20;
/// Larger signing material is not signing material.
const SIGNING_MAX_BYTES: usize = 16 << 20;
/// A weight file in the object database is read whole; larger ones (and
/// any not in Git LFS) stay binary. Files on disk are read in place.
const TENSOR_BLOB_MAX_BYTES: u64 = 1 << 30;

/// Whether the path is one this module may read (while its flag is on);
/// the binary-file fallbacks leave these alone.
pub fn is_inspected(path: &str) -> bool {
    (TENSORS.load(Ordering::Relaxed) && corvene_tensors::is_tensor_file(path))
        || (SIGNING.load(Ordering::Relaxed) && corvene_keys::is_signing_file(path))
}

/// `diff` of the file at `path` read into one of this module's diffs, or
/// handed back unchanged (`Err`) when the path is none of them, its flag
/// is off, or a side cannot be read. `new` and `old` locate the versions
/// git compared.
#[allow(clippy::too_many_arguments)]
pub fn inspect_diff(
    git: &Arc<GitBinary>,
    workdir: &Path,
    path: &str,
    kind: FileStatusKind,
    new: SideRef,
    old: SideRef,
    diff: Diff,
    hide_whitespace: bool,
) -> Result<Diff, Diff> {
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
    let text = matches!(diff, Diff::Text { .. } | Diff::LargeText { .. });
    let pointer = diff.hunks().is_some_and(crate::lfs::is_pointer_diff);
    let binary = matches!(diff, Diff::Binary);
    if TENSORS.load(Ordering::Relaxed)
        && corvene_tensors::is_tensor_file(path)
        && (binary || pointer)
    {
        return tensor_diff(git, workdir, path, &new, &old, diff);
    }
    if SIGNING.load(Ordering::Relaxed)
        && corvene_keys::is_signing_file(path)
        && (binary || (text && !pointer) || pointer)
    {
        return signing_diff(git, workdir, path, &new, &old, diff);
    }
    if !text || pointer {
        return Err(diff);
    }
    if LOCKFILES.load(Ordering::Relaxed) && corvene_lockfiles::is_lockfile(path) {
        let read = |side: &SideRef| side_text(git, workdir, side, LOCKFILE_MAX_BYTES);
        let (Some(new_text), Some(old_text)) = (read(&new), read(&old)) else {
            return Err(diff);
        };
        return match corvene_lockfiles::lockfile_diff(
            path,
            old_text.as_deref(),
            new_text.as_deref(),
        ) {
            Some(changes) => Ok(Diff::Lockfile {
                diff: Arc::new(changes),
                source: Box::new(diff),
            }),
            None => Err(diff),
        };
    }
    if PRETTY.load(Ordering::Relaxed)
        && let Some(language) = corvene_pretty::Language::of_path(path)
    {
        return pretty_diff(git, workdir, path, language, &new, &old, diff, hide_whitespace);
    }
    Err(diff)
}

/// A side's bytes: `Some(None)` for a missing side, `None` when it cannot
/// be read or is larger than `max`.
fn side_bytes(
    git: &Arc<GitBinary>,
    workdir: &Path,
    side: &SideRef,
    max: usize,
) -> Option<Option<Vec<u8>>> {
    let bytes = match side {
        SideRef::Missing => return Some(None),
        SideRef::File(path) => {
            let meta = std::fs::symlink_metadata(path).ok()?;
            if !meta.is_file() || meta.len() > max as u64 {
                return None;
            }
            std::fs::read(path).ok()?
        }
        SideRef::Blob(rev, path) => crate::diff::blob_bytes(git.clone(), workdir, rev, path).ok()?,
    };
    (bytes.len() <= max).then_some(Some(bytes))
}

fn side_text(
    git: &Arc<GitBinary>,
    workdir: &Path,
    side: &SideRef,
    max: usize,
) -> Option<Option<String>> {
    Some(side_bytes(git, workdir, side, max)?.map(|bytes| {
        let text = crate::text_encoding::file_text(&bytes);
        text.strip_prefix('\u{feff}').unwrap_or(&text).to_string()
    }))
}

#[allow(clippy::too_many_arguments)]
fn pretty_diff(
    git: &Arc<GitBinary>,
    workdir: &Path,
    path: &str,
    language: corvene_pretty::Language,
    new: &SideRef,
    old: &SideRef,
    diff: Diff,
    hide_whitespace: bool,
) -> Result<Diff, Diff> {
    let read = |side: &SideRef| side_text(git, workdir, side, PRETTY_MAX_BYTES);
    let (Some(new_text), Some(old_text)) = (read(new), read(old)) else {
        return Err(diff);
    };
    let minified = |t: &Option<String>| {
        t.as_deref()
            .is_some_and(|t| corvene_pretty::is_minified(language, t))
    };
    let named = corvene_pretty::is_minified_name(path);
    if !named && !minified(&new_text) && !minified(&old_text) {
        return Err(diff);
    }
    let format = |t: Option<String>| t.map(|t| corvene_pretty::pretty(language, &t));
    let (new_pretty, old_pretty) = (format(new_text), format(old_text));
    let Some(pretty) = crate::diff::no_index_diff(
        git.clone(),
        old_pretty.as_deref().map(str::as_bytes),
        new_pretty.as_deref().map(str::as_bytes),
        hide_whitespace,
    ) else {
        return Err(diff);
    };
    let mut pretty = match pretty {
        // identical once printed (only whitespace changed)
        Diff::Binary | Diff::Empty | Diff::TooLarge => return Err(diff),
        pretty if pretty.hunks().is_some_and(<[_]>::is_empty) => return Err(diff),
        pretty => pretty,
    };
    // the printed lines are not the file's, so they cannot be staged
    if let Diff::Text { warnings, .. } | Diff::LargeText { warnings, .. } = &mut pretty {
        warnings.derived_text = true;
    }
    Ok(Diff::Pretty {
        language: language.name().to_string(),
        pretty: Box::new(pretty),
        source: Box::new(diff),
    })
}

fn signing_diff(
    git: &Arc<GitBinary>,
    workdir: &Path,
    path: &str,
    new: &SideRef,
    old: &SideRef,
    diff: Diff,
) -> Result<Diff, Diff> {
    let read = |side: &SideRef| -> Option<Option<Vec<u8>>> {
        let bytes = side_bytes(git, workdir, side, SIGNING_MAX_BYTES)?;
        // a side stored in Git LFS: its object when it is local
        Some(match bytes {
            Some(b) => match crate::lfs::lfs_pointer(&b) {
                Some(pointer) => Some(crate::lfs::lfs_object(git.clone(), workdir, &pointer)?),
                None => Some(b),
            },
            None => None,
        })
    };
    let (Some(new_bytes), Some(old_bytes)) = (read(new), read(old)) else {
        return Err(diff);
    };
    let parse = |b: Option<Vec<u8>>| -> Option<Option<Arc<SigningFile>>> {
        match b {
            None => Some(None),
            Some(b) => corvene_keys::read(path, &b, None).map(|f| Some(Arc::new(f))),
        }
    };
    let (Some(current), Some(previous)) = (parse(new_bytes), parse(old_bytes)) else {
        return Err(diff);
    };
    if current.is_none() && previous.is_none() {
        return Err(diff);
    }
    Ok(Diff::Signing {
        previous,
        current,
        source: Box::new(diff),
    })
}

/// Corvene `1341-signing-material-diff`: `file` read again with the
/// password the user typed (a PKCS#12 bundle). The password is not kept.
pub fn unlock_signing_file(path: &str, file: &SigningFile, password: &str) -> Option<SigningFile> {
    let raw = file.raw.as_ref()?;
    corvene_keys::read(path, raw, Some(password))
}

/// Where a weight file's bytes are: an LFS side becomes its local object.
/// `Some(None)` for a missing side; `None` when it cannot be read.
fn tensor_root(git: &Arc<GitBinary>, workdir: &Path, side: &SideRef) -> Option<Option<ArchiveRoot>> {
    let file_root = |path: &Path| -> Option<ArchiveRoot> {
        let meta = std::fs::symlink_metadata(path).ok()?;
        meta.is_file().then(|| ArchiveRoot::File {
            path: path.to_path_buf(),
            len: meta.len(),
            mtime_ns: crate::archive::mtime_ns(&meta),
        })
    };
    let pointer_object = |bytes: &[u8]| -> Option<Option<ArchiveRoot>> {
        let pointer = crate::lfs::lfs_pointer(bytes)?;
        let object = crate::lfs::lfs_object_path(git.clone(), workdir, &pointer)?;
        Some(file_root(&object))
    };
    match side {
        SideRef::Missing => Some(None),
        SideRef::File(path) => {
            let meta = std::fs::symlink_metadata(path).ok()?;
            if meta.len() <= 1024
                && let Ok(bytes) = std::fs::read(path)
                && let Some(object) = pointer_object(&bytes)
            {
                return object.map(Some);
            }
            file_root(path).map(Some)
        }
        SideRef::Blob(..) => {
            let root = crate::archive::pin(git, workdir, side, TENSOR_BLOB_MAX_BYTES).ok()??;
            let ArchiveRoot::Blob { oid } = &root else {
                return Some(Some(root));
            };
            // a small blob may be an LFS pointer
            let repo = crate::handle::open(workdir).ok()?;
            let id = gix::ObjectId::from_hex(oid.as_bytes()).ok()?;
            let size = repo.find_header(id).ok()?.size();
            if size <= 1024 {
                let bytes = crate::archive::read_root(git, workdir, &root).ok()?;
                if let Some(object) = pointer_object(&bytes) {
                    return object.map(Some);
                }
            }
            Some(Some(root))
        }
    }
}

/// A weight file's bytes, opened for random access.
pub enum TensorSource {
    File(std::fs::File),
    Bytes(Vec<u8>),
}

impl TensorSource {
    /// The source behind `root`; `Err` when the file changed on disk.
    pub fn open(git: &Arc<GitBinary>, workdir: &Path, root: &ArchiveRoot) -> Result<Self, String> {
        match root {
            ArchiveRoot::File { path, len, mtime_ns } => {
                let meta = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
                if !meta.is_file()
                    || meta.len() != *len
                    || crate::archive::mtime_ns(&meta) != *mtime_ns
                {
                    return Err("The file changed on disk.".into());
                }
                std::fs::File::open(path)
                    .map(TensorSource::File)
                    .map_err(|e| e.to_string())
            }
            ArchiveRoot::Blob { .. } => {
                crate::archive::read_root(git, workdir, root).map(TensorSource::Bytes)
            }
        }
    }

    /// As the reader's random access source.
    pub fn source(&self) -> &dyn corvene_tensors::Source {
        match self {
            TensorSource::File(f) => f,
            TensorSource::Bytes(b) => b,
        }
    }
}

fn tensor_diff(
    git: &Arc<GitBinary>,
    workdir: &Path,
    path: &str,
    new: &SideRef,
    old: &SideRef,
    diff: Diff,
) -> Result<Diff, Diff> {
    let read = |side: &SideRef| -> Option<Option<Arc<TensorFile>>> {
        let Some(root) = tensor_root(git, workdir, side)? else {
            return Some(None);
        };
        let source = TensorSource::open(git, workdir, &root).ok()?;
        let mut file = corvene_tensors::read(path, source.source()).ok()?;
        file.root = Some(root);
        Some(Some(Arc::new(file)))
    };
    let (Some(current), Some(previous)) = (read(new), read(old)) else {
        return Err(diff);
    };
    if current.is_none() && previous.is_none() {
        return Err(diff);
    }
    Ok(Diff::Tensors {
        previous,
        current,
        source: Box::new(diff),
    })
}

/// Corvene `1340-tensor-diff`: the values of the tensors `names` (all when
/// empty) of both versions compared, reading at most `max_bytes` per side.
pub fn tensor_values(
    git: &Arc<GitBinary>,
    workdir: &Path,
    previous: Option<&TensorFile>,
    current: Option<&TensorFile>,
    names: &[String],
    max_bytes: u64,
    cancel: &AtomicBool,
) -> Result<Vec<corvene_models::tensors::TensorValues>, String> {
    let open = |file: Option<&TensorFile>| -> Result<Option<TensorSource>, String> {
        match file.and_then(|f| f.root.as_ref()) {
            Some(root) => TensorSource::open(git, workdir, root).map(Some),
            None => Ok(None),
        }
    };
    let (old, new) = (open(previous)?, open(current)?);
    fn side<'a>(
        file: Option<&'a TensorFile>,
        source: &'a Option<TensorSource>,
    ) -> Option<(&'a TensorFile, &'a dyn corvene_tensors::Source)> {
        Some((file?, source.as_ref()?.source()))
    }
    Ok(corvene_tensors::compare_values(
        side(previous, &old),
        side(current, &new),
        names,
        max_bytes,
        cancel,
    ))
}
