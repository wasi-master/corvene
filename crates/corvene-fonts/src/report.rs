//! Everything one face of a font says about itself, read once and kept as
//! plain data for [`crate::compare`] and the UI.

use std::collections::{BTreeMap, HashMap};

use skrifa::instance::{LocationRef, NormalizedCoord, Size};
use skrifa::outline::{DrawSettings, OutlineGlyphFormat};
use skrifa::raw::tables::glyf::Glyph;
use skrifa::raw::traversal::{FieldType, SomeArray, SomeTable};
use skrifa::raw::types::{GlyphId, Tag};
use skrifa::raw::{TableProvider, TopLevelTable};
use skrifa::string::{LocalizedString, StringId};
use skrifa::{FontRef, MetadataProvider};

use crate::layout::{self, Kerning, LayoutReport};
use crate::outline::{Fnv, GlyphPath, hash_bytes};

/// At most this many fields are listed per table.
pub const MAX_FIELDS: usize = 40_000;
/// Glyph outlines drawn at most at this many axis extremes for the
/// variation hash (two per axis).
const MAX_VARIATION_LOCATIONS: usize = 8;

/// How the glyphs are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OutlineKind {
    TrueType,
    Cff,
    Cff2,
    /// Bitmaps or colour layers only.
    None,
}

impl OutlineKind {
    pub fn label(self) -> &'static str {
        match self {
            OutlineKind::TrueType => "TrueType (glyf)",
            OutlineKind::Cff => "PostScript (CFF)",
            OutlineKind::Cff2 => "PostScript (CFF2)",
            OutlineKind::None => "No outlines",
        }
    }
}

/// One table of the directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableEntry {
    pub tag: String,
    pub len: u32,
    pub checksum: u32,
    pub hash: u64,
}

/// A table's fields, flattened to `path = value` lines (like `ttx`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableFields {
    pub fields: Vec<(String, String)>,
    pub truncated: bool,
}

/// One `name` record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameEntry {
    pub name_id: u16,
    pub platform: u16,
    pub encoding: u16,
    pub language: u16,
    /// BCP 47 tag where known ("en", "de-CH"), else the raw id.
    pub language_tag: String,
    pub value: String,
}

/// One glyph.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphInfo {
    pub name: String,
    /// Whether `name` was made up (no `post` names, no CFF charset).
    pub synthesized_name: bool,
    pub codepoints: Vec<u32>,
    pub advance: f32,
    pub lsb: f32,
    pub bounds: Option<[f32; 4]>,
    pub contours: u32,
    pub points: u32,
    /// Component glyph ids (TrueType composites).
    pub components: Vec<u32>,
    /// Hash of what the glyph itself stores: its outline, or for a
    /// composite its components (names, offsets, transforms).
    pub own_hash: u64,
    /// Hash of the drawn outline, components resolved.
    pub flat_hash: u64,
    /// Hash of outlines and advances at the axis extremes.
    pub var_hash: u64,
    /// Drawn with colour layers (COLR), not only an outline.
    pub color: bool,
}

/// One variation axis.
#[derive(Clone, Debug, PartialEq)]
pub struct AxisInfo {
    pub tag: String,
    pub name: String,
    pub min: f32,
    pub default: f32,
    pub max: f32,
    pub hidden: bool,
}

/// One named instance.
#[derive(Clone, Debug, PartialEq)]
pub struct InstanceInfo {
    pub name: String,
    pub ps_name: Option<String>,
    pub coords: Vec<f32>,
}

/// Vertical metrics in font units.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VMetrics {
    pub ascender: f32,
    pub descender: f32,
    pub line_gap: f32,
    pub x_height: Option<f32>,
    pub cap_height: Option<f32>,
}

/// Colour formats and palettes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ColorInfo {
    pub formats: Vec<&'static str>,
    /// RGBA per palette entry.
    pub palettes: Vec<Vec<[u8; 4]>>,
}

/// One face of a font file, read.
#[derive(Clone, Debug, PartialEq)]
pub struct FaceReport {
    pub index: u32,
    /// "Inter Bold".
    pub title: String,
    pub family: String,
    pub style: String,
    pub ps_name: Option<String>,
    /// Labelled values for the overview, in display order.
    pub overview: Vec<(String, String)>,
    pub units_per_em: u16,
    pub outline: OutlineKind,
    pub vmetrics: VMetrics,
    pub tables: Vec<TableEntry>,
    pub table_fields: BTreeMap<String, TableFields>,
    pub names: Vec<NameEntry>,
    /// Code point → glyph id.
    pub cmap: BTreeMap<u32, u32>,
    pub glyphs: Vec<GlyphInfo>,
    /// Glyph name → glyph id.
    pub by_name: HashMap<String, u32>,
    pub axes: Vec<AxisInfo>,
    pub instances: Vec<InstanceInfo>,
    pub gsub: Option<LayoutReport>,
    pub gpos: Option<LayoutReport>,
    pub kerning: Kerning,
    pub color: ColorInfo,
}

/// What a face is called and a hash of all its tables: enough to pair the
/// faces of two collections and see which changed, without reading them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaceSummary {
    pub index: u32,
    pub title: String,
    pub ps_name: Option<String>,
    pub hash: u64,
}

fn string(font: &FontRef, id: StringId) -> Option<String> {
    font.localized_strings(id)
        .english_or_first()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

/// Family and style the way a menu shows them (typographic names first).
fn family_style(font: &FontRef) -> (String, String) {
    let family = string(font, StringId::TYPOGRAPHIC_FAMILY_NAME)
        .or_else(|| string(font, StringId::FAMILY_NAME))
        .unwrap_or_default();
    let style = string(font, StringId::TYPOGRAPHIC_SUBFAMILY_NAME)
        .or_else(|| string(font, StringId::SUBFAMILY_NAME))
        .unwrap_or_default();
    (family, style)
}

fn title(family: &str, style: &str) -> String {
    match (family.is_empty(), style.is_empty()) {
        (true, _) => "Untitled".to_string(),
        (false, true) => family.to_string(),
        (false, false) => format!("{family} {style}"),
    }
}

/// The summary of face `index`.
pub fn summary(font: &FontRef, index: u32) -> FaceSummary {
    let (family, style) = family_style(font);
    let mut h = Fnv::new();
    for record in font.table_directory.table_records() {
        h.write(&record.tag().to_be_bytes());
        if let Some(data) = font.table_data(record.tag()) {
            h.int(hash_bytes(data.as_bytes()) as i64);
        }
    }
    FaceSummary {
        index,
        title: title(&family, &style),
        ps_name: string(font, StringId::POSTSCRIPT_NAME),
        hash: h.finish(),
    }
}

impl FaceReport {
    /// Reads face `index` of `font`.
    pub fn read(font: &FontRef, index: u32) -> FaceReport {
        let (family, style) = family_style(font);
        let skrifa_metrics = font.metrics(Size::unscaled(), LocationRef::default());
        let outline = match font.outline_glyphs().format() {
            Some(OutlineGlyphFormat::Glyf) => OutlineKind::TrueType,
            Some(OutlineGlyphFormat::Cff) => OutlineKind::Cff,
            Some(OutlineGlyphFormat::Cff2) => OutlineKind::Cff2,
            None => OutlineKind::None,
        };
        let cmap: BTreeMap<u32, u32> = font
            .charmap()
            .mappings()
            .map(|(cp, gid)| (cp, gid.to_u32()))
            .collect();
        let axes: Vec<AxisInfo> = font
            .axes()
            .iter()
            .map(|a| AxisInfo {
                tag: a.tag().to_string().trim_end().to_string(),
                name: string(font, a.name_id()).unwrap_or_else(|| a.tag().to_string()),
                min: a.min_value(),
                default: a.default_value(),
                max: a.max_value(),
                hidden: a.is_hidden(),
            })
            .collect();
        let instances = font
            .named_instances()
            .iter()
            .map(|i| InstanceInfo {
                name: string(font, i.subfamily_name_id()).unwrap_or_default(),
                ps_name: i.postscript_name_id().and_then(|id| string(font, id)),
                coords: i.user_coords().collect(),
            })
            .collect();
        let glyphs = glyphs(font, &cmap, axes.len());
        let names_by_gid: Vec<&str> = glyphs.iter().map(|g| g.name.as_str()).collect();
        let gsub = layout::gsub(font, &names_by_gid);
        let (gpos, mut kerning) = match layout::gpos(font, &names_by_gid) {
            Some((gpos, kerning)) => (Some(gpos), kerning),
            None => (None, Kerning::default()),
        };
        if kerning.pairs.is_empty() {
            kerning = layout::legacy_kern(font);
        }
        let tables = font
            .table_directory
            .table_records()
            .iter()
            .map(|r| TableEntry {
                tag: r.tag().to_string().trim_end().to_string(),
                len: r.length(),
                checksum: r.checksum(),
                hash: font
                    .table_data(r.tag())
                    .map(|d| hash_bytes(d.as_bytes()))
                    .unwrap_or_default(),
            })
            .collect::<Vec<_>>();
        let table_fields = tables
            .iter()
            .filter_map(|t| {
                let tag = Tag::new_checked(format!("{:<4}", t.tag).as_bytes()).ok()?;
                Some((t.tag.clone(), table_fields(font, tag, &names_by_gid)?))
            })
            .collect();
        let vmetrics = VMetrics {
            ascender: skrifa_metrics.ascent,
            descender: skrifa_metrics.descent,
            line_gap: skrifa_metrics.leading,
            x_height: skrifa_metrics.x_height,
            cap_height: skrifa_metrics.cap_height,
        };
        let color = color_info(font);
        let mut report = FaceReport {
            index,
            title: title(&family, &style),
            family,
            style,
            ps_name: string(font, StringId::POSTSCRIPT_NAME),
            overview: Vec::new(),
            units_per_em: skrifa_metrics.units_per_em,
            outline,
            vmetrics,
            tables,
            table_fields,
            names: name_entries(font),
            cmap,
            by_name: glyphs
                .iter()
                .enumerate()
                .map(|(i, g)| (g.name.clone(), i as u32))
                .collect(),
            glyphs,
            axes,
            instances,
            gsub,
            gpos,
            kerning,
            color,
        };
        report.overview = overview(font, &report, &skrifa_metrics);
        report
    }

    /// The glyph id of the glyph named `name`.
    pub fn glyph_by_name(&self, name: &str) -> Option<u32> {
        self.by_name.get(name).copied()
    }

    /// The glyph's name, or `#gid` for an id out of range.
    pub fn glyph_name(&self, gid: u32) -> String {
        self.glyphs
            .get(gid as usize)
            .map(|g| g.name.clone())
            .unwrap_or_else(|| format!("#{gid}"))
    }

    /// Whether this face is a variable font.
    pub fn is_variable(&self) -> bool {
        !self.axes.is_empty()
    }
}

/// Normalised locations at each axis's minimum and maximum.
fn axis_extremes(axis_count: usize) -> Vec<Vec<NormalizedCoord>> {
    let mut out = Vec::new();
    for axis in 0..axis_count {
        for v in [-1.0, 1.0] {
            if out.len() >= MAX_VARIATION_LOCATIONS {
                return out;
            }
            let mut coords = vec![NormalizedCoord::ZERO; axis_count];
            coords[axis] = NormalizedCoord::from_f32(v);
            out.push(coords);
        }
    }
    out
}

/// Draws glyph `gid` at `coords`, components resolved.
pub fn draw(font: &FontRef, gid: u32, coords: &[NormalizedCoord]) -> GlyphPath {
    let mut path = GlyphPath::default();
    if let Some(glyph) = font.outline_glyphs().get(GlyphId::new(gid)) {
        let settings = DrawSettings::unhinted(Size::unscaled(), LocationRef::new(coords));
        if glyph.draw(settings, &mut path).is_err() {
            path.cmds.clear();
        }
    }
    path
}

fn glyphs(font: &FontRef, cmap: &BTreeMap<u32, u32>, axis_count: usize) -> Vec<GlyphInfo> {
    let count = font.maxp().map(|m| u32::from(m.num_glyphs())).unwrap_or(0);
    let outlines = font.outline_glyphs();
    let names = font.glyph_names();
    let metrics = font.glyph_metrics(Size::unscaled(), LocationRef::default());
    let colors = font.color_glyphs();
    let mut by_glyph: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for (&cp, &gid) in cmap {
        by_glyph.entry(gid).or_default().push(cp);
    }
    let extremes = axis_extremes(axis_count);
    let extreme_metrics: Vec<_> = extremes
        .iter()
        .map(|c| font.glyph_metrics(Size::unscaled(), LocationRef::new(c)))
        .collect();
    let glyf = font.loca(None).ok().zip(font.glyf().ok());
    let mut out = Vec::with_capacity(count as usize);
    // composite glyph id, its (component glyph, placement) records
    type Composite = (u32, Vec<(u32, [i64; 6])>);
    let mut composites: Vec<Composite> = Vec::new();
    for gid in 0..count {
        let id = GlyphId::new(gid);
        let name = names.get(id);
        let mut path = GlyphPath::default();
        if let Some(glyph) = outlines.get(id) {
            let settings = DrawSettings::unhinted(Size::unscaled(), LocationRef::default());
            if glyph.draw(settings, &mut path).is_err() {
                path.cmds.clear();
            }
        }
        let flat_hash = path.hash();
        let mut var = Fnv::new();
        for (coords, m) in extremes.iter().zip(&extreme_metrics) {
            var.int(draw(font, gid, coords).hash() as i64);
            var.int(
                m.advance_width(id)
                    .map(|a| (a * 16.0).round() as i64)
                    .unwrap_or(0),
            );
        }
        let mut components = Vec::new();
        if let Some((loca, glyf)) = &glyf
            && let Ok(Some(Glyph::Composite(c))) = loca.get_glyf(id, glyf)
        {
            let mut records = Vec::new();
            for comp in c.components() {
                let t = comp.transform;
                let (dx, dy) = match comp.anchor {
                    skrifa::raw::tables::glyf::Anchor::Offset { x, y } => {
                        (i64::from(x), i64::from(y))
                    }
                    skrifa::raw::tables::glyf::Anchor::Point { base, component } => {
                        (i64::from(base) << 32, i64::from(component) << 32)
                    }
                };
                let q = |v: skrifa::raw::types::F2Dot14| (v.to_f32() * 16384.0).round() as i64;
                records.push((
                    comp.glyph.to_u32(),
                    [q(t.xx), q(t.yx), q(t.xy), q(t.yy), dx, dy],
                ));
                components.push(comp.glyph.to_u32());
            }
            composites.push((gid, records));
        }
        out.push(GlyphInfo {
            synthesized_name: name.as_ref().is_none_or(|n| n.is_synthesized()),
            name: name
                .map(|n| n.as_str().to_string())
                .unwrap_or_else(|| format!("glyph{gid:05}")),
            codepoints: by_glyph.remove(&gid).unwrap_or_default(),
            advance: metrics.advance_width(id).unwrap_or(0.0),
            lsb: metrics.left_side_bearing(id).unwrap_or(0.0),
            bounds: path.bounds(),
            contours: path.contours(),
            points: path.point_count(),
            components,
            own_hash: flat_hash,
            flat_hash,
            var_hash: var.finish(),
            color: colors.get(id).is_some(),
        });
    }
    // a composite's own data is its component list, by name so a renumbered
    // glyph order does not count as a change
    for (gid, records) in composites {
        let mut h = Fnv::new();
        for (base, t) in records {
            h.write(
                out.get(base as usize)
                    .map(|g| g.name.as_bytes())
                    .unwrap_or_default(),
            );
            for v in t {
                h.int(v);
            }
        }
        if let Some(g) = out.get_mut(gid as usize) {
            g.own_hash = h.finish();
        }
    }
    out
}

fn name_entries(font: &FontRef) -> Vec<NameEntry> {
    let Ok(name) = font.name() else {
        return Vec::new();
    };
    name.name_record()
        .iter()
        .map(|r| {
            let s = LocalizedString::new(&name, r);
            NameEntry {
                name_id: r.name_id().to_u16(),
                platform: r.platform_id(),
                encoding: r.encoding_id(),
                language: r.language_id(),
                language_tag: s
                    .language()
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("0x{:04X}", r.language_id())),
                value: s.chars().collect(),
            }
        })
        .collect()
}

/// The registered meaning of a `name` id.
pub fn name_id_label(id: u16) -> &'static str {
    match id {
        0 => "Copyright",
        1 => "Family",
        2 => "Subfamily",
        3 => "Unique ID",
        4 => "Full name",
        5 => "Version",
        6 => "PostScript name",
        7 => "Trademark",
        8 => "Manufacturer",
        9 => "Designer",
        10 => "Description",
        11 => "Vendor URL",
        12 => "Designer URL",
        13 => "License",
        14 => "License URL",
        16 => "Typographic family",
        17 => "Typographic subfamily",
        18 => "Compatible full name",
        19 => "Sample text",
        20 => "PostScript CID name",
        21 => "WWS family",
        22 => "WWS subfamily",
        23 => "Light background palette",
        24 => "Dark background palette",
        25 => "Variations PostScript prefix",
        256.. => "Font-specific",
        _ => "Reserved",
    }
}

/// `usWeightClass` with its common name.
pub fn weight_label(weight: u16) -> String {
    let name = match weight {
        0..=149 => "Thin",
        150..=249 => "Extra Light",
        250..=349 => "Light",
        350..=449 => "Regular",
        450..=549 => "Medium",
        550..=649 => "Semi Bold",
        650..=749 => "Bold",
        750..=849 => "Extra Bold",
        _ => "Black",
    };
    format!("{weight} {name}")
}

fn width_label(width: u16) -> String {
    let name = match width {
        1 => "Ultra Condensed",
        2 => "Extra Condensed",
        3 => "Condensed",
        4 => "Semi Condensed",
        5 => "Normal",
        6 => "Semi Expanded",
        7 => "Expanded",
        8 => "Extra Expanded",
        9 => "Ultra Expanded",
        _ => "Unknown",
    };
    format!("{width} {name}")
}

/// `fsType` as the embedding permission it grants.
fn embedding_label(fs_type: u16) -> String {
    let base = match fs_type & 0x000F {
        0 => "Installable",
        2 => "Restricted license",
        4 => "Preview & print",
        8 => "Editable",
        _ => "Mixed",
    };
    let mut out = base.to_string();
    if fs_type & 0x0100 != 0 {
        out.push_str(", no subsetting");
    }
    if fs_type & 0x0200 != 0 {
        out.push_str(", bitmap only");
    }
    out
}

/// A `LongDateTime` (seconds since 1904-01-01) as `YYYY-MM-DD HH:MM`.
pub fn format_date(secs: i64) -> String {
    let unix = secs - 2_082_844_800;
    let days = unix.div_euclid(86_400);
    let rem = unix.rem_euclid(86_400);
    // civil from days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60
    )
}

fn color_info(font: &FontRef) -> ColorInfo {
    let mut formats = Vec::new();
    if let Ok(colr) = font.colr() {
        formats.push(if colr.version() >= 1 {
            "COLRv1"
        } else {
            "COLRv0"
        });
    }
    for (tag, label) in [
        (Tag::new(b"SVG "), "SVG"),
        (Tag::new(b"sbix"), "sbix"),
        (Tag::new(b"CBDT"), "CBDT"),
        (Tag::new(b"EBDT"), "EBDT"),
    ] {
        if font.table_data(tag).is_some() {
            formats.push(label);
        }
    }
    let palettes = font.color_palettes();
    let palettes = (0..palettes.len())
        .filter_map(|i| palettes.get(i))
        .map(|p| {
            p.colors()
                .iter()
                .map(|c| [c.red, c.green, c.blue, c.alpha])
                .collect()
        })
        .collect();
    ColorInfo { formats, palettes }
}

fn overview(font: &FontRef, r: &FaceReport, m: &skrifa::metrics::Metrics) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut push = |label: &str, value: Option<String>| {
        if let Some(v) = value.filter(|v| !v.is_empty()) {
            out.push((label.to_string(), v));
        }
    };
    push("Family", Some(r.family.clone()));
    push("Style", Some(r.style.clone()));
    push("Full name", string(font, StringId::FULL_NAME));
    push("PostScript name", r.ps_name.clone());
    push("Version", string(font, StringId::VERSION_STRING));
    let os2 = font.os2().ok();
    let head = font.head().ok();
    push(
        "Font revision",
        head.as_ref()
            .map(|h| format!("{:.3}", h.font_revision().to_f64())),
    );
    push(
        "Vendor",
        os2.as_ref()
            .map(|o| o.ach_vend_id().to_string().trim_end().to_string()),
    );
    push("Designer", string(font, StringId::DESIGNER));
    push("Manufacturer", string(font, StringId::MANUFACTURER));
    push("Copyright", string(font, StringId::COPYRIGHT_NOTICE));
    push("Trademark", string(font, StringId::TRADEMARK));
    push("License", string(font, StringId::LICENSE_DESCRIPTION));
    push("License URL", string(font, StringId::LICENSE_URL));
    push("Outlines", Some(r.outline.label().to_string()));
    push("Glyphs", Some(r.glyphs.len().to_string()));
    push("Code points", Some(r.cmap.len().to_string()));
    push("Units per em", Some(r.units_per_em.to_string()));
    push(
        "Weight",
        os2.as_ref().map(|o| weight_label(o.us_weight_class())),
    );
    push(
        "Width",
        os2.as_ref().map(|o| width_label(o.us_width_class())),
    );
    push("Italic angle", Some(format!("{}°", m.italic_angle)));
    push(
        "Monospaced",
        Some(if m.is_monospace { "Yes" } else { "No" }.to_string()),
    );
    push("Ascender", Some(format!("{}", r.vmetrics.ascender)));
    push("Descender", Some(format!("{}", r.vmetrics.descender)));
    push("Line gap", Some(format!("{}", r.vmetrics.line_gap)));
    push("x-height", r.vmetrics.x_height.map(|v| v.to_string()));
    push("Cap height", r.vmetrics.cap_height.map(|v| v.to_string()));
    push(
        "Embedding",
        os2.as_ref().map(|o| embedding_label(o.fs_type())),
    );
    let hinted = [b"fpgm", b"prep", b"cvt "]
        .iter()
        .any(|t| font.table_data(Tag::new(t)).is_some());
    push(
        "Hinting",
        Some(
            if hinted {
                "TrueType instructions"
            } else {
                "None"
            }
            .to_string(),
        ),
    );
    push(
        "Created",
        head.as_ref().map(|h| format_date(h.created().as_secs())),
    );
    push(
        "Modified",
        head.as_ref().map(|h| format_date(h.modified().as_secs())),
    );
    if !r.axes.is_empty() {
        let axes = r
            .axes
            .iter()
            .map(|a| format!("{} {}–{} ({})", a.tag, a.min, a.max, a.default))
            .collect::<Vec<_>>()
            .join(", ");
        push("Axes", Some(axes));
        push("Named instances", Some(r.instances.len().to_string()));
    }
    if !r.color.formats.is_empty() {
        push("Colour", Some(r.color.formats.join(", ")));
        push("Palettes", Some(r.color.palettes.len().to_string()));
    }
    let features = |l: &Option<LayoutReport>| l.as_ref().map(|l| l.features.len()).unwrap_or(0);
    push(
        "Layout features",
        Some(format!(
            "GSUB {} · GPOS {}",
            features(&r.gsub),
            features(&r.gpos)
        )),
    );
    push("Kerning pairs", Some(r.kerning.pairs.len().to_string()));
    out
}

/// Tables whose fields are listed elsewhere (glyphs, names, code points)
/// or are opaque data.
fn skip_fields(tag: &[u8; 4]) -> bool {
    matches!(
        tag,
        b"glyf"
            | b"loca"
            | b"CFF "
            | b"CFF2"
            | b"gvar"
            | b"hmtx"
            | b"vmtx"
            | b"hdmx"
            | b"name"
            | b"cvar"
            | b"CBDT"
            | b"EBDT"
            | b"SVG "
            | b"sbix"
            | b"DSIG"
    )
}

macro_rules! traverse {
    ($font:expr, $tag:expr, $names:expr, [$($t:literal => $f:ident),* $(,)?]) => {
        match &$tag.to_be_bytes() {
            $( $t => $font.$f().ok().map(|t| flatten_table(&t, $names)), )*
            _ => None,
        }
    };
}

/// A table's fields, for the tables read-fonts can walk.
fn table_fields(font: &FontRef, tag: Tag, names: &[&str]) -> Option<TableFields> {
    if skip_fields(&tag.to_be_bytes()) {
        return None;
    }
    traverse!(font, tag, names, [
        b"head" => head,
        b"hhea" => hhea,
        b"vhea" => vhea,
        b"maxp" => maxp,
        b"OS/2" => os2,
        b"post" => post,
        b"gasp" => gasp,
        b"fvar" => fvar,
        b"avar" => avar,
        b"HVAR" => hvar,
        b"VVAR" => vvar,
        b"MVAR" => mvar,
        b"STAT" => stat,
        b"GDEF" => gdef,
        b"GSUB" => gsub,
        b"GPOS" => gpos,
        b"BASE" => base,
        b"cmap" => cmap,
        b"CPAL" => cpal,
        b"COLR" => colr,
        b"meta" => meta,
        b"feat" => feat,
        b"trak" => trak,
        b"ltag" => ltag,
        b"VORG" => vorg,
        b"CBLC" => cblc,
        b"EBLC" => eblc,
    ])
}

fn flatten_table<'a>(table: &(impl SomeTable<'a> + TopLevelTable), names: &[&str]) -> TableFields {
    let mut out = TableFields::default();
    let mut walker = Walker {
        names,
        out: &mut out,
    };
    walker.table(table, "", 0);
    out
}

/// A hash of a table's fields with glyph ids written as names, so the same
/// data under another glyph order hashes the same.
pub(crate) fn hash_table<'a>(table: &(impl SomeTable<'a> + ?Sized), names: &[&str]) -> u64 {
    let mut out = TableFields::default();
    let mut walker = Walker {
        names,
        out: &mut out,
    };
    walker.table(table, "", 0);
    let mut h = Fnv::new();
    for (path, value) in &out.fields {
        h.write(path.as_bytes());
        h.write(&[0]);
        h.write(value.as_bytes());
        h.write(&[1]);
    }
    h.finish()
}

struct Walker<'n, 'o> {
    names: &'n [&'n str],
    out: &'o mut TableFields,
}

const MAX_DEPTH: usize = 24;

/// A field name without read-fonts' `_offset` / `_record` suffixes.
fn field_name(name: &str) -> &str {
    name.trim_end_matches("_offsets")
        .trim_end_matches("_offset")
}

impl Walker<'_, '_> {
    fn push(&mut self, path: &str, value: String) {
        if self.out.fields.len() >= MAX_FIELDS {
            self.out.truncated = true;
            return;
        }
        self.out.fields.push((path.to_string(), value));
    }

    fn full(&self) -> bool {
        self.out.fields.len() >= MAX_FIELDS
    }

    fn table<'a>(&mut self, table: &(impl SomeTable<'a> + ?Sized), path: &str, depth: usize) {
        let mut i = 0;
        while let Some(field) = table.get_field(i) {
            i += 1;
            if self.full() {
                self.out.truncated = true;
                return;
            }
            let name = field_name(field.name);
            let child = if path.is_empty() {
                name.to_string()
            } else {
                format!("{path}.{name}")
            };
            self.value(field.value, &child, depth + 1);
        }
    }

    fn array<'a>(&mut self, array: &(impl SomeArray<'a> + ?Sized), path: &str, depth: usize) {
        for idx in 0..array.len() {
            if self.full() {
                self.out.truncated = true;
                return;
            }
            if let Some(item) = array.get(idx) {
                self.value(item, &format!("{path}[{idx}]"), depth + 1);
            }
        }
    }

    fn value(&mut self, value: FieldType<'_>, path: &str, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        let scalar = match value {
            FieldType::I8(v) => v.to_string(),
            FieldType::U8(v) => v.to_string(),
            FieldType::I16(v) => v.to_string(),
            FieldType::U16(v) => v.to_string(),
            FieldType::I32(v) => v.to_string(),
            FieldType::U32(v) => v.to_string(),
            FieldType::I24(v) => i32::from(v).to_string(),
            FieldType::U24(v) => v.to_u32().to_string(),
            FieldType::Tag(t) => t.to_string(),
            FieldType::FWord(v) => v.to_i16().to_string(),
            FieldType::UfWord(v) => v.to_u16().to_string(),
            FieldType::MajorMinor(v) => format!("{}.{}", v.major, v.minor),
            FieldType::Version16Dot16(v) => format!("{v}"),
            FieldType::F2Dot14(v) => format!("{}", v.to_f32()),
            FieldType::Fixed(v) => format!("{}", v.to_f64()),
            FieldType::LongDateTime(v) => format_date(v.as_secs()),
            FieldType::GlyphId16(g) => {
                let gid = g.to_u16();
                match self.names.get(gid as usize) {
                    Some(name) => format!("{name} ({gid})"),
                    None => gid.to_string(),
                }
            }
            FieldType::NameId(n) => n.to_u16().to_string(),
            FieldType::BareOffset(o) => format!("0x{:X}", o.to_u32()),
            FieldType::ResolvedOffset(r) => {
                match r.target {
                    Ok(t) => self.table(&t, path, depth),
                    Err(_) if r.offset.to_u32() == 0 => self.push(path, "null".to_string()),
                    Err(e) => self.push(path, format!("unreadable ({e})")),
                }
                return;
            }
            FieldType::StringOffset(s) => match s.target {
                Ok(t) => t.iter_chars().take(512).collect(),
                Err(e) => format!("unreadable ({e})"),
            },
            FieldType::ArrayOffset(a) => {
                match a.target {
                    Ok(arr) => self.array(&arr, path, depth),
                    Err(e) => self.push(path, format!("unreadable ({e})")),
                }
                return;
            }
            FieldType::Record(r) => {
                self.table(&r, path, depth);
                return;
            }
            FieldType::Array(arr) => {
                self.array(&arr, path, depth);
                return;
            }
            FieldType::Unknown => return,
        };
        self.push(path, scalar);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_long_date_times() {
        // 2006-07-24 00:00 UTC
        assert_eq!(
            format_date(1_153_699_200 + 2_082_844_800),
            "2006-07-24 00:00"
        );
        assert_eq!(format_date(0), "1904-01-01 00:00");
    }

    #[test]
    fn labels_weights() {
        assert_eq!(weight_label(400), "400 Regular");
        assert_eq!(weight_label(700), "700 Bold");
    }
}
