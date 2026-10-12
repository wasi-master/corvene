//! Font files read for the font viewer (Corvene `1329-font-diff`; GHD shows
//! them as "Binary file not shown", `app/src/lib/git/diff.ts`), and font
//! sources read for their Glyphs view (Corvene `1330-font-source-preview`;
//! GHD shows the text diff only).
//!
//! Only the bytes are read here; `corvene_fonts` parses them on the UI side.
//! A UFO glyph takes the glyphs its components use with it (same layer,
//! same revision, through `contents.plist`) so it can be drawn whole.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

use corvene_models::{
    Diff, FileStatusKind, FontBlob, FontSourceBlob, FontSourceKind, is_font_file,
};

/// Set by the dispatcher from the flag.
static READ_FONT_FILES: AtomicBool = AtomicBool::new(false);

/// Turn the font viewer on or off (flag `1329-font-diff`).
pub fn set_read_font_files(enabled: bool) {
    READ_FONT_FILES.store(enabled, Ordering::Relaxed);
}

/// Whether binary fonts become font diffs.
pub fn read_font_files() -> bool {
    READ_FONT_FILES.load(Ordering::Relaxed)
}

/// Component glyphs followed this deep at most.
const MAX_COMPONENT_DEPTH: usize = 8;
/// At most this many sibling glyphs are read for one `.glif`.
const MAX_SIBLINGS: usize = 64;

/// A binary change of a font file as a font diff: `None` when the flag is
/// off, the path is not a font, or neither side looks like one (the caller
/// keeps `Binary`). Sides over `corvene_fonts::MAX_BYTES` are left out.
pub fn font_diff(
    path: &str,
    kind: FileStatusKind,
    current: impl FnOnce() -> Option<Vec<u8>>,
    previous: impl FnOnce() -> Option<Vec<u8>>,
) -> Option<Diff> {
    if !read_font_files() || !is_font_file(path) {
        return None;
    }
    let side = |bytes: Vec<u8>| {
        (bytes.len() <= corvene_fonts::MAX_BYTES && corvene_fonts::sniff(&bytes).is_some())
            .then_some(FontBlob { bytes })
    };
    let current = (kind != FileStatusKind::Deleted)
        .then(current)
        .flatten()
        .and_then(side);
    let previous = (!kind.is_new_or_untracked())
        .then(previous)
        .flatten()
        .and_then(side);
    if current.is_none() && previous.is_none() {
        return None;
    }
    Some(Diff::Font { previous, current })
}

/// A font source file's sides for its Glyphs view (`1330`). `read_current`
/// and `read_previous` read a repository path at each side's revision;
/// `previous_path` is the old path of a renamed file. `None` when the path
/// is not a font source or neither side can be read.
pub fn font_source_diff(
    path: &str,
    previous_path: &str,
    kind: FileStatusKind,
    read_current: impl Fn(&str) -> Option<Vec<u8>>,
    read_previous: impl Fn(&str) -> Option<Vec<u8>>,
) -> Option<Diff> {
    let source_kind = corvene_models::font_source_kind(path)?;
    let current = (kind != FileStatusKind::Deleted)
        .then(|| source_blob(source_kind, path, &read_current))
        .flatten();
    let previous = (!kind.is_new_or_untracked())
        .then(|| source_blob(source_kind, previous_path, &read_previous))
        .flatten();
    if current.is_none() && previous.is_none() {
        return None;
    }
    Some(Diff::FontSource {
        kind: source_kind,
        previous,
        current,
    })
}

/// One side: the file, and for a `.glif` the glyphs its components use.
fn source_blob(
    kind: FontSourceKind,
    path: &str,
    read: &impl Fn(&str) -> Option<Vec<u8>>,
) -> Option<FontSourceBlob> {
    let bytes = read(path)?;
    if bytes.len() > corvene_fonts::MAX_BYTES {
        return None;
    }
    let siblings = match kind {
        FontSourceKind::Glif => glif_siblings(path, &bytes, read),
        _ => BTreeMap::new(),
    };
    Some(FontSourceBlob { bytes, siblings })
}

/// The component glyphs `bytes` (a `.glif` at `path`) uses, by glyph name,
/// recursively, looked up through the layer's `contents.plist`.
fn glif_siblings(
    path: &str,
    bytes: &[u8],
    read: &impl Fn(&str) -> Option<Vec<u8>>,
) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let dir = path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let join = |file: &str| {
        if dir.is_empty() {
            file.to_string()
        } else {
            format!("{dir}/{file}")
        }
    };
    let Some(contents) = read(&join("contents.plist"))
        .and_then(|b| corvene_fonts::sources::parse_contents_plist(&b).ok())
    else {
        return out;
    };
    let components = |b: &[u8]| -> Vec<String> {
        corvene_fonts::sources::parse_glif(b, &BTreeMap::new())
            .map(|g| g.components.into_iter().map(|c| c.base).collect())
            .unwrap_or_default()
    };
    let mut queue: Vec<(String, usize)> = components(bytes).into_iter().map(|n| (n, 1)).collect();
    while let Some((name, depth)) = queue.pop() {
        if out.len() >= MAX_SIBLINGS || depth > MAX_COMPONENT_DEPTH || out.contains_key(&name) {
            continue;
        }
        let Some(file) = contents.get(&name) else {
            continue;
        };
        let Some(glif) = read(&join(file)) else {
            continue;
        };
        queue.extend(components(&glif).into_iter().map(|n| (n, depth + 1)));
        out.insert(name, glif);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_flag<T>(f: impl FnOnce() -> T) -> T {
        set_read_font_files(true);
        f()
    }

    #[test]
    fn reads_fonts_only_with_the_flag_and_a_font_magic() {
        let ttf = || Some(vec![0, 1, 0, 0, 0, 0]);
        set_read_font_files(false);
        assert!(font_diff("a.ttf", FileStatusKind::Modified, ttf, ttf).is_none());
        with_flag(|| {
            assert!(matches!(
                font_diff("a.ttf", FileStatusKind::Modified, ttf, ttf),
                Some(Diff::Font {
                    previous: Some(_),
                    current: Some(_)
                })
            ));
            assert!(font_diff("a.png", FileStatusKind::Modified, ttf, ttf).is_none());
            assert!(
                font_diff(
                    "a.ttf",
                    FileStatusKind::Modified,
                    || Some(b"text".to_vec()),
                    || None
                )
                .is_none()
            );
            assert!(matches!(
                font_diff(
                    "a.woff2",
                    FileStatusKind::New,
                    || Some(b"wOF2....".to_vec()),
                    ttf
                ),
                Some(Diff::Font {
                    previous: None,
                    current: Some(_)
                })
            ));
        });
    }

    #[test]
    fn a_glif_brings_its_component_glyphs() {
        let files: BTreeMap<&str, &str> = BTreeMap::from([
            (
                "f.ufo/glyphs/contents.plist",
                r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>a</key><string>a.glif</string>
<key>acutecomb</key><string>acutecomb.glif</string>
<key>aacute</key><string>aacute.glif</string>
</dict></plist>"#,
            ),
            (
                "f.ufo/glyphs/a.glif",
                r#"<glyph name="a" format="2"><advance width="500"/><outline><contour><point x="0" y="0" type="line"/><point x="100" y="0" type="line"/><point x="100" y="100" type="line"/></contour></outline></glyph>"#,
            ),
            (
                "f.ufo/glyphs/acutecomb.glif",
                r#"<glyph name="acutecomb" format="2"><advance width="0"/></glyph>"#,
            ),
            (
                "f.ufo/glyphs/aacute.glif",
                r#"<glyph name="aacute" format="2"><advance width="500"/><outline><component base="a"/><component base="acutecomb" xOffset="100"/></outline></glyph>"#,
            ),
        ]);
        let read = |p: &str| files.get(p).map(|s| s.as_bytes().to_vec());
        let diff = font_source_diff(
            "f.ufo/glyphs/aacute.glif",
            "f.ufo/glyphs/aacute.glif",
            FileStatusKind::New,
            read,
            |_| None,
        );
        let Some(Diff::FontSource {
            kind: FontSourceKind::Glif,
            previous: None,
            current: Some(blob),
        }) = diff
        else {
            panic!("expected a glif source diff");
        };
        assert_eq!(blob.siblings.keys().collect::<Vec<_>>(), ["a", "acutecomb"]);
    }
}
