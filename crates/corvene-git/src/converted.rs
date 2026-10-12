//! Corvene `1350-compiled-code-diff`, `1351-serialized-data-diff`,
//! `1352-translation-diff`, `1353-diagram-diff`, `1354-map-diff` and
//! `1355-email-diff`: a changed file of a format [`corvene_convert`] reads
//! becomes a [`Diff::Converted`]: its decoded text (diffed by `git diff
//! --no-index`), its table or its pictures, next to git's own diff.
//!
//! GitHub Desktop shows all of these as text or binary diffs
//! (`app/src/ui/diff/index.tsx`).

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use corvene_convert::{Format, Views};
use corvene_models::{ConvertedKind, Diff, FileStatusKind, ImageBlob};

use crate::detect::GitBinary;

/// One bit per [`ConvertedKind`] (in `ConvertedKind::ALL` order), set by
/// the dispatcher from the flags.
static KINDS: AtomicU8 = AtomicU8::new(0);

fn bit(kind: ConvertedKind) -> u8 {
    1 << ConvertedKind::ALL
        .iter()
        .position(|k| *k == kind)
        .unwrap_or(0)
}

/// Turns each kind of conversion on or off.
pub fn set_converted_kinds(enabled: &[ConvertedKind]) {
    KINDS.store(
        enabled.iter().map(|k| bit(*k)).fold(0, |a, b| a | b),
        Ordering::Relaxed,
    );
}

fn enabled(kind: ConvertedKind) -> bool {
    KINDS.load(Ordering::Relaxed) & bit(kind) != 0
}

/// Whether `path` may convert (by its name; files without one may be
/// native binaries and need their bytes to tell).
pub fn may_convert(path: &str) -> bool {
    match corvene_convert::format_by_name(path) {
        Some(format) => enabled(format.kind()),
        None => corvene_convert::needs_sample(path) && KINDS.load(Ordering::Relaxed) != 0,
    }
}

/// `source` (git's diff of `path`) as a converted diff when the file reads
/// as a format whose flag is on; `source` itself otherwise. `current` and
/// `previous` read the two versions (not called for a side that does not
/// exist).
pub fn converted_diff(
    git: Arc<GitBinary>,
    path: &str,
    kind: FileStatusKind,
    current: impl FnOnce() -> Option<Vec<u8>>,
    previous: impl FnOnce() -> Option<Vec<u8>>,
    source: Diff,
) -> Diff {
    if !may_convert(path) {
        return source;
    }
    // only content changes of files git could read: text, binary, or an
    // archive diff of a zipped format (KMZ)
    let by_name = corvene_convert::format_by_name(path);
    match &source {
        Diff::Text { hunks, .. } | Diff::LargeText { hunks, .. } => {
            if hunks.is_empty() || crate::lfs::is_pointer_diff(hunks) {
                return source;
            }
        }
        Diff::Binary => {}
        Diff::Archive(_) if by_name.is_some_and(|f| f.kind() == ConvertedKind::Map) => {}
        _ => return source,
    }
    let current = (kind != FileStatusKind::Deleted).then(current).flatten();
    let previous = (!kind.is_new_or_untracked()).then(previous).flatten();
    if current.is_none() && previous.is_none() {
        return source;
    }
    let sample = current.as_deref().or(previous.as_deref());
    let Some(format) = corvene_convert::format_of(path, sample) else {
        return source;
    };
    // a file without a telling name converts only from a binary diff
    if by_name.is_none() && !matches!(source, Diff::Binary) {
        return source;
    }
    if !enabled(format.kind()) {
        return source;
    }
    match converted_view(git, format, previous.as_deref(), current.as_deref()) {
        Some(view) => Diff::Converted {
            kind: format.kind(),
            format: format.name().to_string(),
            view: Box::new(view),
            source: Box::new(source),
        },
        None => source,
    }
}

/// The converted view of two versions held in memory, or `None` when a
/// side does not read as `format`.
pub(crate) fn converted_view(
    git: Arc<GitBinary>,
    format: Format,
    old: Option<&[u8]>,
    new: Option<&[u8]>,
) -> Option<Diff> {
    let views = match corvene_convert::convert(format, old, new) {
        Ok(views) => views,
        Err(reason) => {
            tracing::debug!(?format, %reason, "file did not convert");
            return None;
        }
    };
    Some(match views {
        Views::Text { old, new } => {
            let diff = crate::diff::no_index_diff(
                git,
                old.as_deref().map(str::as_bytes),
                new.as_deref().map(str::as_bytes),
                false,
            )?;
            match diff {
                Diff::Text {
                    hunks,
                    mut warnings,
                } => {
                    warnings.derived_text = true;
                    warnings.decoded_from = Some(format.name().to_string());
                    Diff::Text { hunks, warnings }
                }
                Diff::LargeText {
                    hunks,
                    mut warnings,
                } => {
                    warnings.derived_text = true;
                    warnings.decoded_from = Some(format.name().to_string());
                    Diff::LargeText { hunks, warnings }
                }
                other => other,
            }
        }
        Views::Table { old, new } => Diff::Table {
            previous: old,
            current: new,
        },
        Views::Svg { old, new } => {
            let blob = |bytes: Vec<u8>| ImageBlob {
                bytes,
                media_type: crate::diff::SVG_MEDIA_TYPE.to_string(),
                original: None,
            };
            Diff::Image {
                previous: old.map(blob),
                current: new.map(blob),
            }
        }
    })
}

/// An archive entry's diff as a converted diff (a class file in a jar, a
/// binary property list in an app bundle's zip).
pub(crate) fn entry_converted(
    git: &Arc<GitBinary>,
    path: &str,
    old: Option<&[u8]>,
    new: Option<&[u8]>,
    source: Diff,
) -> Diff {
    let kind = match (old.is_some(), new.is_some()) {
        (false, _) => FileStatusKind::New,
        (_, false) => FileStatusKind::Deleted,
        _ => FileStatusKind::Modified,
    };
    converted_diff(
        git.clone(),
        path,
        kind,
        || new.map(<[u8]>::to_vec),
        || old.map(<[u8]>::to_vec),
        source,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_switch_on_and_off() {
        set_converted_kinds(&[ConvertedKind::Diagram]);
        assert!(may_convert("flow.mmd"));
        assert!(!may_convert("Main.class"));
        set_converted_kinds(&ConvertedKind::ALL);
        assert!(may_convert("Main.class"));
        assert!(may_convert("bin/tool"));
        assert!(!may_convert("src/main.rs"));
        set_converted_kinds(&[]);
    }
}
