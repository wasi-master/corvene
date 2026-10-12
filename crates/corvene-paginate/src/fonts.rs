//! The system fonts pages are drawn with: the sans-serif and monospace
//! families of the platform's UI (the faces the Rendered view's text uses),
//! their bold and italic faces (a variable font's `wght` axis, or a
//! synthesised weight or slant when the family has no such face), and a
//! fallback face for characters the family does not cover.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};

use rustybuzz::ttf_parser;

/// The family a stretch of text is set in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    Sans,
    Mono,
}

/// One face, ready to shape with and to embed.
pub struct Face {
    id: fontdb::ID,
    pub data: Arc<Vec<u8>>,
    pub index: u32,
    /// Variation coordinates (`wght` of a variable font).
    pub variations: Vec<(ttf_parser::Tag, f32)>,
    pub font: krilla::text::Font,
    /// Draw a heavier stroke (no bold face).
    pub fake_bold: bool,
    /// Slant the glyphs (no italic face).
    pub fake_italic: bool,
    /// Ascender and descender (negative) per em.
    pub ascender: f32,
    pub descender: f32,
}

pub type FaceRef = Arc<Face>;

/// Families tried in order: the platform UI fonts first (San Francisco,
/// Segoe UI, the GNOME and KDE defaults), then common fallbacks.
const SANS: &[&str] = &[
    "System Font",
    ".SF NS",
    "SF Pro",
    "SF Pro Text",
    "Helvetica Neue",
    "Segoe UI",
    "Inter",
    "Cantarell",
    "Noto Sans",
    "Ubuntu",
    "DejaVu Sans",
    "Liberation Sans",
    "Arial",
    "Helvetica",
];

const MONO: &[&str] = &[
    "SF Mono",
    "Menlo",
    "Cascadia Mono",
    "Consolas",
    "JetBrains Mono",
    "DejaVu Sans Mono",
    "Liberation Mono",
    "Noto Sans Mono",
    "Courier New",
];

/// Faces looked at first for a character the text's family lacks.
const FALLBACK: &[&str] = &[
    "Helvetica Neue",
    "Arial Unicode MS",
    "PingFang SC",
    "Hiragino Sans",
    "Apple SD Gothic Neo",
    "Noto Sans CJK SC",
    "Noto Sans CJK JP",
    "Source Han Sans SC",
    "Microsoft YaHei",
    "Yu Gothic",
    "Malgun Gothic",
    "Geeza Pro",
    "Noto Sans Arabic",
    "Segoe UI",
    "Arial Hebrew",
    "Noto Sans Hebrew",
    "Kohinoor Devanagari",
    "Noto Sans Devanagari",
    "Nirmala UI",
    "Thonburi",
    "Noto Sans Thai",
    "Leelawadee UI",
    "Apple Symbols",
    "Segoe UI Symbol",
    "Noto Sans Symbols",
    "Noto Sans Symbols 2",
    "Noto Sans Math",
    "STIX Two Math",
    "Apple Color Emoji",
    "Noto Color Emoji",
    "Segoe UI Emoji",
    "DejaVu Sans",
];

struct Fonts {
    db: fontdb::Database,
    files: Mutex<HashMap<PathBuf, Arc<Vec<u8>>>>,
    styled: Mutex<HashMap<(Family, bool, bool), Option<FaceRef>>>,
    by_char: Mutex<HashMap<char, Option<FaceRef>>>,
    by_id: Mutex<HashMap<(fontdb::ID, bool, bool), Option<FaceRef>>>,
}

static FONTS: LazyLock<Fonts> = LazyLock::new(|| {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    Fonts {
        db,
        files: Mutex::new(HashMap::new()),
        styled: Mutex::new(HashMap::new()),
        by_char: Mutex::new(HashMap::new()),
        by_id: Mutex::new(HashMap::new()),
    }
});

/// The face of a family in a weight and style; `None` when the system has
/// no font at all.
pub fn face(family: Family, bold: bool, italic: bool) -> Option<FaceRef> {
    let fonts = &*FONTS;
    let key = (family, bold, italic);
    if let Some(found) = fonts.styled.lock().ok().and_then(|m| m.get(&key).cloned()) {
        return found;
    }
    let names = match family {
        Family::Sans => SANS,
        Family::Mono => MONO,
    };
    let generic = match family {
        Family::Sans => fontdb::Family::SansSerif,
        Family::Mono => fontdb::Family::Monospace,
    };
    let query_families: Vec<fontdb::Family> = names
        .iter()
        .map(|n| fontdb::Family::Name(n))
        .chain([generic, fontdb::Family::SansSerif])
        .collect();
    let id = fonts.db.query(&fontdb::Query {
        families: &query_families,
        weight: if bold {
            fontdb::Weight::BOLD
        } else {
            fontdb::Weight::NORMAL
        },
        stretch: fontdb::Stretch::Normal,
        style: if italic {
            fontdb::Style::Italic
        } else {
            fontdb::Style::Normal
        },
    });
    let id = id.or_else(|| fonts.db.faces().next().map(|f| f.id));
    let found = id.and_then(|id| load(fonts, id, bold, italic));
    if let Ok(mut m) = fonts.styled.lock() {
        m.insert(key, found.clone());
    }
    found
}

/// A face that has a glyph for `c`, regular weight, for text whose own
/// face lacks it (the bold and italic of the text are synthesised).
pub fn fallback(c: char, bold: bool, italic: bool) -> Option<FaceRef> {
    let fonts = &*FONTS;
    let base = fonts.by_char.lock().ok().and_then(|m| m.get(&c).cloned());
    let base = match base {
        Some(found) => found,
        None => {
            let found = find_fallback(fonts, c);
            if let Ok(mut m) = fonts.by_char.lock() {
                m.insert(c, found.clone());
            }
            found
        }
    }?;
    if !bold && !italic {
        return Some(base);
    }
    load(fonts, base.id, bold, italic)
}

fn find_fallback(fonts: &Fonts, c: char) -> Option<FaceRef> {
    let has = |id: fontdb::ID| -> bool {
        fonts
            .db
            .with_face_data(id, |data, index| {
                ttf_parser::Face::parse(data, index)
                    .ok()
                    .and_then(|f| f.glyph_index(c))
                    .is_some()
            })
            .unwrap_or(false)
    };
    let named = FALLBACK.iter().find_map(|name| {
        let id = fonts.db.query(&fontdb::Query {
            families: &[fontdb::Family::Name(name)],
            ..fontdb::Query::default()
        })?;
        has(id).then_some(id)
    });
    let id = named.or_else(|| {
        // any regular face that has it, the smallest file first (a
        // symbol font over a 100 MB CJK collection)
        let mut regular: Vec<&fontdb::FaceInfo> = fonts
            .db
            .faces()
            .filter(|f| f.style == fontdb::Style::Normal && f.weight == fontdb::Weight::NORMAL)
            .collect();
        regular.sort_by_key(|f| match &f.source {
            fontdb::Source::File(p) | fontdb::Source::SharedFile(p, _) => {
                std::fs::metadata(p).map_or(u64::MAX, |m| m.len())
            }
            fontdb::Source::Binary(_) => 0,
        });
        regular.into_iter().map(|f| f.id).find(|id| has(*id))
    })?;
    load(fonts, id, false, false)
}

/// Read a face's file once and set it up in the wanted weight and style.
fn load(fonts: &Fonts, id: fontdb::ID, bold: bool, italic: bool) -> Option<FaceRef> {
    let key = (id, bold, italic);
    if let Some(found) = fonts.by_id.lock().ok().and_then(|m| m.get(&key).cloned()) {
        return found;
    }
    let found = load_uncached(fonts, id, bold, italic);
    if let Ok(mut m) = fonts.by_id.lock() {
        m.insert(key, found.clone());
    }
    found
}

fn load_uncached(fonts: &Fonts, id: fontdb::ID, bold: bool, italic: bool) -> Option<FaceRef> {
    let info = fonts.db.face(id)?;
    let data = match &info.source {
        fontdb::Source::File(path) | fontdb::Source::SharedFile(path, _) => {
            let cached = fonts.files.lock().ok()?.get(path).cloned();
            match cached {
                Some(data) => data,
                None => {
                    let data = Arc::new(std::fs::read(path).ok()?);
                    fonts.files.lock().ok()?.insert(path.clone(), data.clone());
                    data
                }
            }
        }
        fontdb::Source::Binary(bytes) => Arc::new((**bytes).as_ref().to_vec()),
    };
    let index = info.index;
    let parsed = ttf_parser::Face::parse(&data, index).ok()?;
    let wght = ttf_parser::Tag::from_bytes(b"wght");
    let mut variations = Vec::new();
    let mut fake_bold = false;
    if let Some(axis) = parsed.variation_axes().into_iter().find(|a| a.tag == wght) {
        let want: f32 = if bold { 700.0 } else { 400.0 };
        variations.push((wght, want.clamp(axis.min_value, axis.max_value)));
        fake_bold = bold && axis.max_value < 600.0;
    } else if bold && info.weight.0 < 600 {
        fake_bold = true;
    }
    let fake_italic = italic && info.style == fontdb::Style::Normal && !parsed.is_italic();
    let upem = f32::from(parsed.units_per_em()).max(1.0);
    let ascender = f32::from(parsed.ascender()) / upem;
    let descender = f32::from(parsed.descender()) / upem;
    let coords: Vec<(krilla::text::Tag, f32)> = variations
        .iter()
        .map(|(tag, v)| (krilla::text::Tag::new(&tag.to_bytes()), *v))
        .collect();
    let font = krilla::text::Font::new_variable(data.clone().into(), index, &coords)?;
    Some(Arc::new(Face {
        id,
        data,
        index,
        variations,
        font,
        fake_bold,
        fake_italic,
        ascender: if ascender > 0.0 { ascender } else { 0.9 },
        descender: if descender < 0.0 { descender } else { -0.25 },
    }))
}

/// The face has a glyph for `c`.
pub fn covers(face: &ttf_parser::Face, c: char) -> bool {
    face.glyph_index(c).is_some()
}

/// Characters that stay in the face of the character before them
/// (combining marks, joiners, variation selectors, emoji modifiers).
pub fn joins_previous(c: char) -> bool {
    matches!(u32::from(c),
        0x0300..=0x036F
        | 0x1AB0..=0x1AFF
        | 0x1DC0..=0x1DFF
        | 0x200C..=0x200F
        | 0x20D0..=0x20FF
        | 0xFE00..=0xFE0F
        | 0xFE20..=0xFE2F
        | 0x1F3FB..=0x1F3FF
        | 0xE0000..=0xE01EF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_has_a_face() {
        let Some(face) = face(Family::Sans, false, false) else {
            // a machine without fonts (some CI images)
            return;
        };
        assert!(!face.data.is_empty());
        let bold = super::face(Family::Sans, true, false).expect("bold");
        assert!(bold.fake_bold || !bold.variations.is_empty() || !Arc::ptr_eq(&bold, &face));
    }
}
