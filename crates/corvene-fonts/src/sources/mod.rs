//! Font sources: UFO `.glif` glyphs, Glyphs `.glyphs` files (formats 2 and
//! 3, and the glyph files of a `.glyphspackage`) and `.designspace`
//! documents, read into plain structs and compared side by side.
//!
//! Git diffs these as text; the "Glyphs" side of the diff switch draws
//! them. Point lists become [`GlyphPath`]s the way fontTools'
//! `PointToSegmentPen` draws them, and components are drawn in place
//! (resolved through the other glyphs of the same layer or master).

mod designspace;
mod glif;
mod glyphs;
mod openstep;
mod plist;

use std::collections::BTreeMap;

pub use designspace::parse_designspace;
pub use glif::parse_glif;
pub use glyphs::parse_glyphs;
pub use plist::parse_contents_plist;

use crate::compare::{Field, Status, merge_fields};
use crate::outline::{GlyphPath, PathCmd};

/// Which kind of font source a file is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SourceKind {
    /// A UFO glyph (`.glif`).
    Glif,
    /// A Glyphs file (`.glyphs`), or a `.glyphspackage`'s `fontinfo.plist`
    /// or one of its `glyphs/*.glyph` files.
    Glyphs,
    /// A `.designspace` document.
    Designspace,
}

/// The source kind of a repository path, by its extension.
pub fn source_kind(path: &str) -> Option<SourceKind> {
    let lower = path.to_ascii_lowercase();
    let mut parts = lower.rsplit(['/', '\\']);
    let file = parts.next()?;
    let parent = parts.next().unwrap_or("");
    let ext = file.rsplit_once('.').map(|(_, e)| e)?;
    match ext {
        "glif" => Some(SourceKind::Glif),
        "glyphs" => Some(SourceKind::Glyphs),
        "designspace" => Some(SourceKind::Designspace),
        "glyph" if parent == "glyphs" && parts.next()?.ends_with(".glyphspackage") => {
            Some(SourceKind::Glyphs)
        }
        "plist" if file == "fontinfo.plist" && parent.ends_with(".glyphspackage") => {
            Some(SourceKind::Glyphs)
        }
        _ => None,
    }
}

/// A component as written: the base glyph and its affine transform
/// `[xx, xy, yx, yy, dx, dy]` (fontTools order).
#[derive(Clone, Debug, PartialEq)]
pub struct Component {
    pub base: String,
    pub transform: [f32; 6],
}

/// A named anchor point.
#[derive(Clone, Debug, PartialEq)]
pub struct Anchor {
    pub name: String,
    pub x: f32,
    pub y: f32,
}

/// One glyph layer of a source.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceGlyph {
    pub name: String,
    pub unicodes: Vec<u32>,
    pub advance: f32,
    pub height: Option<f32>,
    /// Contours and components drawn in place, in font units (y up).
    pub path: GlyphPath,
    pub components: Vec<Component>,
    pub anchors: Vec<Anchor>,
    pub guidelines: usize,
    pub note: Option<String>,
    /// Top-level keys of the glyph's lib (UFO) or user data (Glyphs).
    pub lib_keys: Vec<String>,
    /// Component bases that could not be drawn: missing, cyclic or nested
    /// too deeply.
    pub unresolved: Vec<String>,
}

/// A font master of a Glyphs file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Master {
    pub id: String,
    pub name: String,
    /// Location on each of the font's axes, in axis order.
    pub axes: Vec<f32>,
    /// Ascender, cap height, x-height, descender, baseline, italic angle…
    pub metrics: Vec<(String, f32)>,
}

/// A glyph of a Glyphs file with all its layers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlyphsGlyph {
    pub name: String,
    pub unicodes: Vec<u32>,
    pub category: Option<String>,
    pub export: bool,
    /// Master layers keyed by master id, other layers by their name.
    pub layers: Vec<(String, SourceGlyph)>,
}

/// A `.glyphs` file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlyphsFont {
    pub family: String,
    pub units_per_em: f32,
    pub version: String,
    /// Label, value: designer, manufacturer, copyright, date, custom
    /// parameters (summarised)…
    pub info: Vec<(String, String)>,
    /// "Weight (wght)"…
    pub axes: Vec<String>,
    pub masters: Vec<Master>,
    /// Instance name, then its labelled settings.
    pub instances: Vec<(String, Vec<(String, String)>)>,
    pub glyphs: Vec<GlyphsGlyph>,
    /// Feature tag, `@class` or `Prefix: name`, then its code.
    pub features: Vec<(String, String)>,
    /// Master id, left, right, value.
    pub kerning: Vec<(String, String, String, f32)>,
}

/// A design-space axis.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DsAxis {
    pub tag: String,
    pub name: String,
    pub min: f32,
    pub default: f32,
    pub max: f32,
    /// User value → design value.
    pub map: Vec<(f32, f32)>,
    pub hidden: bool,
    /// The allowed values of a discrete axis.
    pub values: Vec<f32>,
}

/// A source (master) of a designspace.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DsSource {
    pub name: String,
    pub filename: String,
    pub family: String,
    pub style: String,
    pub layer: Option<String>,
    /// Axis name → design value.
    pub location: Vec<(String, f32)>,
}

/// An instance of a designspace.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DsInstance {
    pub name: String,
    pub family: String,
    pub style: String,
    pub filename: String,
    /// Axis name → design value.
    pub location: Vec<(String, f32)>,
}

/// A substitution rule of a designspace.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DsRule {
    pub name: String,
    /// Axis, minimum, maximum.
    pub conditions: Vec<(String, Option<f32>, Option<f32>)>,
    /// Glyph, replacement.
    pub subs: Vec<(String, String)>,
}

/// A `.designspace` document (format 4 or 5).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Designspace {
    pub format: String,
    pub axes: Vec<DsAxis>,
    pub sources: Vec<DsSource>,
    pub instances: Vec<DsInstance>,
    pub rules: Vec<DsRule>,
    pub variable_fonts: Vec<String>,
}

/// One glyph (layer) compared.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphChange {
    pub name: String,
    pub status: Status,
    pub old: Option<SourceGlyph>,
    pub new: Option<SourceGlyph>,
    /// Advance, unicodes, contours, points, bounds, components, anchors (by
    /// name), guidelines, note, lib keys.
    pub fields: Vec<Field>,
}

/// One glyph of a Glyphs file compared, layer by layer.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphsGlyphChange {
    pub name: String,
    pub status: Status,
    /// Unicodes, export, category.
    pub fields: Vec<Field>,
    /// Layer key (master id or layer name), then that layer compared.
    pub layers: Vec<(String, GlyphChange)>,
}

/// Two Glyphs files compared.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlyphsDiff {
    /// Family, units per em, version, then the font info.
    pub info: Vec<Field>,
    /// Masters matched by id, labelled by name.
    pub masters: Vec<(String, Status, Vec<Field>)>,
    pub axes: Vec<Field>,
    pub instances: Vec<(String, Status, Vec<Field>)>,
    /// Every glyph, old order first, then glyphs only the new side has.
    pub glyphs: Vec<GlyphsGlyphChange>,
    pub features: Vec<(String, Status)>,
    /// Changed pairs only: master name, left, right, old value, new value.
    pub kerning: Vec<KernChange>,
}

/// A kerning pair compared: master name, left, right, old value, new value.
pub type KernChange = (String, String, String, Option<f32>, Option<f32>);

/// Two designspace documents compared, item by item (matched by name).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DesignspaceDiff {
    pub axes: Vec<(String, Status, Vec<Field>)>,
    pub sources: Vec<(String, Status, Vec<Field>)>,
    pub instances: Vec<(String, Status, Vec<Field>)>,
    pub rules: Vec<(String, Status, Vec<Field>)>,
}

// ---------------------------------------------------------------------------
// Outlines shared by the readers

/// A point of a source contour; its type names the segment that ends at
/// it (UFO and Glyphs agree on this).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SrcPoint {
    pub x: f32,
    pub y: f32,
    pub kind: PointKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PointKind {
    Move,
    Line,
    Curve,
    QCurve,
    OffCurve,
}

/// A contour (already drawn) or a component, in drawing order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum OutlineItem {
    Contour(Vec<PathCmd>),
    Component(Component),
}

/// Components resolve at most this many levels deep.
pub(crate) const MAX_COMPONENT_DEPTH: usize = 8;

/// Draws `items`, resolving components through `lookup` (a base glyph's
/// items by name). Bases that cannot be drawn are added to `unresolved`.
pub(crate) fn flatten<'a>(
    items: &[OutlineItem],
    lookup: &dyn Fn(&str) -> Option<&'a [OutlineItem]>,
    unresolved: &mut Vec<String>,
) -> GlyphPath {
    let mut f = Flattener {
        lookup,
        memo: BTreeMap::new(),
        stack: Vec::new(),
        unresolved,
    };
    let mut cmds = Vec::new();
    f.draw(items, 0, &mut cmds);
    GlyphPath { cmds }
}

struct Flattener<'a, 'b> {
    lookup: &'b dyn Fn(&str) -> Option<&'a [OutlineItem]>,
    /// Bases already drawn (only those drawn without hitting a cycle).
    memo: BTreeMap<String, GlyphPath>,
    stack: Vec<String>,
    unresolved: &'b mut Vec<String>,
}

impl Flattener<'_, '_> {
    fn draw(&mut self, items: &[OutlineItem], depth: usize, out: &mut Vec<PathCmd>) -> bool {
        let mut clean = true;
        for item in items {
            match item {
                OutlineItem::Contour(cmds) => out.extend_from_slice(cmds),
                OutlineItem::Component(c) => match self.base(&c.base, depth + 1) {
                    Some((path, ok)) => {
                        clean &= ok;
                        out.extend(path.transformed(c.transform).cmds);
                    }
                    None => clean = false,
                },
            }
        }
        clean
    }

    /// A base glyph drawn, and whether it was drawn completely.
    fn base(&mut self, name: &str, depth: usize) -> Option<(GlyphPath, bool)> {
        if let Some(p) = self.memo.get(name) {
            return Some((p.clone(), true));
        }
        if depth > MAX_COMPONENT_DEPTH || self.stack.iter().any(|s| s == name) {
            self.note(name);
            return None;
        }
        let Some(items) = (self.lookup)(name) else {
            self.note(name);
            return None;
        };
        self.stack.push(name.to_string());
        let mut cmds = Vec::new();
        let clean = self.draw(items, depth, &mut cmds);
        self.stack.pop();
        let path = GlyphPath { cmds };
        if clean {
            self.memo.insert(name.to_string(), path.clone());
        }
        Some((path, clean))
    }

    fn note(&mut self, name: &str) {
        if !self.unresolved.iter().any(|u| u == name) {
            self.unresolved.push(name.to_string());
        }
    }
}

/// Converts one contour's points into path commands the way fontTools'
/// `PointToSegmentPen` (over a `BasePen`) does: a contour starting with a
/// `move` point is open; a closed one is rotated to end on its first
/// on-curve point, and its implied closing line is left out. `curve`
/// segments with more than two control points are split like
/// `decomposeSuperBezierSegment`, `qcurve` ones get TrueType's implied
/// on-curve midpoints, and a contour of control points only becomes a
/// closed quadratic loop.
pub(crate) fn contour_cmds(points: &[SrcPoint], out: &mut Vec<PathCmd>) {
    let Some(first) = points.first() else {
        return;
    };
    if points.len() == 1 {
        out.push(PathCmd::MoveTo(first.x, first.y));
        return;
    }
    let closed = first.kind != PointKind::Move;
    let body: Vec<SrcPoint> = if closed {
        match points.iter().position(|p| p.kind != PointKind::OffCurve) {
            Some(i) => points[i + 1..]
                .iter()
                .chain(&points[..=i])
                .copied()
                .collect(),
            None => {
                quad_loop(points, out);
                return;
            }
        }
    } else {
        points[1..].to_vec()
    };

    // segments: (kind, control points, end point)
    let mut segments: Vec<Segment> = Vec::new();
    let mut offs: Vec<(f32, f32)> = Vec::new();
    for p in &body {
        if p.kind == PointKind::OffCurve {
            offs.push((p.x, p.y));
        } else {
            segments.push((p.kind, std::mem::take(&mut offs), (p.x, p.y)));
        }
    }

    let move_pt = if closed {
        match segments.last() {
            Some(s) => s.2,
            None => return,
        }
    } else {
        (first.x, first.y)
    };
    out.push(PathCmd::MoveTo(move_pt.0, move_pt.1));
    let n = segments.len();
    let mut last = move_pt;
    for (i, (kind, offs, pt)) in segments.iter().enumerate() {
        match kind {
            PointKind::Curve => curve_to(offs, *pt, out),
            PointKind::QCurve => qcurve_to(offs, *pt, out),
            // `line`, or a stray `move` inside a contour
            _ => {
                if i + 1 != n || !closed || *pt == last {
                    out.push(PathCmd::LineTo(pt.0, pt.1));
                } else {
                    continue;
                }
            }
        }
        last = *pt;
    }
    if closed {
        out.push(PathCmd::Close);
    }
}

/// Segment type, control points, end point.
type Segment = (PointKind, Vec<(f32, f32)>, (f32, f32));

fn mid(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    (0.5 * (a.0 + b.0), 0.5 * (a.1 + b.1))
}

/// `BasePen.curveTo`.
fn curve_to(offs: &[(f32, f32)], pt: (f32, f32), out: &mut Vec<PathCmd>) {
    match offs.len() {
        0 => out.push(PathCmd::LineTo(pt.0, pt.1)),
        1 => qcurve_to(offs, pt, out),
        2 => out.push(PathCmd::CubicTo(
            offs[0].0, offs[0].1, offs[1].0, offs[1].1, pt.0, pt.1,
        )),
        _ => {
            // decomposeSuperBezierSegment
            let mut points: Vec<(f32, f32)> = offs.to_vec();
            points.push(pt);
            let n = points.len() - 1;
            let mut p1 = points[0];
            let mut p2: Option<(f32, f32)> = None;
            for i in 2..=n {
                let divisions = i.min(3).min(n - i + 2);
                for j in 1..divisions {
                    let factor = j as f32 / divisions as f32;
                    let (t1, t2) = (points[i - 1], points[i - 2]);
                    let temp = (t2.0 + factor * (t1.0 - t2.0), t2.1 + factor * (t1.1 - t2.1));
                    match p2 {
                        None => p2 = Some(temp),
                        Some(c2) => {
                            let p3 = mid(c2, temp);
                            out.push(PathCmd::CubicTo(p1.0, p1.1, c2.0, c2.1, p3.0, p3.1));
                            p1 = temp;
                            p2 = None;
                        }
                    }
                }
            }
            let c2 = points[n - 1];
            out.push(PathCmd::CubicTo(p1.0, p1.1, c2.0, c2.1, pt.0, pt.1));
        }
    }
}

/// `BasePen.qCurveTo` with an on-curve end point.
fn qcurve_to(offs: &[(f32, f32)], pt: (f32, f32), out: &mut Vec<PathCmd>) {
    let Some(&last_off) = offs.last() else {
        out.push(PathCmd::LineTo(pt.0, pt.1));
        return;
    };
    for w in offs.windows(2) {
        let implied = mid(w[0], w[1]);
        out.push(PathCmd::QuadTo(w[0].0, w[0].1, implied.0, implied.1));
    }
    out.push(PathCmd::QuadTo(last_off.0, last_off.1, pt.0, pt.1));
}

/// A closed contour with no on-curve point: starts at the midpoint of its
/// last and first control points.
fn quad_loop(points: &[SrcPoint], out: &mut Vec<PathCmd>) {
    let offs: Vec<(f32, f32)> = points.iter().map(|p| (p.x, p.y)).collect();
    let (Some(&first), Some(&last)) = (offs.first(), offs.last()) else {
        return;
    };
    let start = mid(last, first);
    out.push(PathCmd::MoveTo(start.0, start.1));
    qcurve_to(&offs, start, out);
    out.push(PathCmd::Close);
}

// ---------------------------------------------------------------------------
// Diffs

/// A number as a font editor shows it: no trailing `.0`, no `-0`.
pub(crate) fn num(v: f32) -> String {
    if v == 0.0 {
        "0".to_string()
    } else if v.fract() == 0.0 && v.abs() < 1e9 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

fn unicodes_text(u: &[u32]) -> String {
    u.iter()
        .map(|c| format!("U+{c:04X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn component_text(c: &Component) -> String {
    let [a, b, cc, d, e, f] = c.transform;
    if [a, b, cc, d] == [1.0, 0.0, 0.0, 1.0] {
        if e == 0.0 && f == 0.0 {
            c.base.clone()
        } else {
            format!("{} at {}, {}", c.base, num(e), num(f))
        }
    } else {
        let m: Vec<String> = c.transform.iter().map(|v| num(*v)).collect();
        format!("{} [{}]", c.base, m.join(" "))
    }
}

/// A glyph's comparable values, labelled.
fn glyph_values(g: &SourceGlyph) -> Vec<(String, String)> {
    let mut v = vec![("Advance width".to_string(), num(g.advance))];
    if let Some(h) = g.height {
        v.push(("Advance height".into(), num(h)));
    }
    if !g.unicodes.is_empty() {
        v.push(("Unicodes".into(), unicodes_text(&g.unicodes)));
    }
    v.push(("Contours".into(), g.path.contours().to_string()));
    v.push(("Points".into(), g.path.point_count().to_string()));
    if let Some([x0, y0, x1, y1]) = g.path.bounds() {
        let b = [x0, y0, x1, y1].map(|v| num(v.round()));
        v.push(("Bounds".into(), b.join(" ")));
    }
    if !g.components.is_empty() {
        let list: Vec<String> = g.components.iter().map(component_text).collect();
        v.push(("Components".into(), list.join(", ")));
    }
    for a in &g.anchors {
        v.push((
            format!("Anchor {}", a.name),
            format!("{}, {}", num(a.x), num(a.y)),
        ));
    }
    if g.guidelines > 0 {
        v.push(("Guidelines".into(), g.guidelines.to_string()));
    }
    if let Some(n) = &g.note {
        v.push(("Note".into(), n.clone()));
    }
    if !g.lib_keys.is_empty() {
        v.push(("Lib keys".into(), g.lib_keys.join(", ")));
    }
    if !g.unresolved.is_empty() {
        v.push(("Missing components".into(), g.unresolved.join(", ")));
    }
    v
}

/// Two versions of a glyph compared. Modified when the outline or any
/// listed field (advance, unicodes, components, anchors, note…) differs.
pub fn diff_glif(old: Option<&SourceGlyph>, new: Option<&SourceGlyph>) -> GlyphChange {
    let name = new.or(old).map(|g| g.name.clone()).unwrap_or_default();
    let fields = merge_fields(
        old.map(glyph_values).as_deref(),
        new.map(glyph_values).as_deref(),
    );
    let status = match (old, new) {
        (Some(a), Some(b)) => {
            if a.path.hash() != b.path.hash() || fields.iter().any(|f| f.status().is_change()) {
                Status::Modified
            } else {
                Status::Unchanged
            }
        }
        (None, Some(_)) => Status::Added,
        (Some(_), None) => Status::Removed,
        (None, None) => Status::Unchanged,
    };
    GlyphChange {
        name,
        status,
        old: old.cloned(),
        new: new.cloned(),
        fields,
    }
}

/// Pairs items of two lists by key (first unmatched item with the same
/// key), old order first, then items only the new side has.
fn pair_by<'a, T>(
    old: &'a [T],
    new: &'a [T],
    key: impl Fn(&T) -> &str,
) -> Vec<(Option<&'a T>, Option<&'a T>)> {
    let mut queue: BTreeMap<&str, std::collections::VecDeque<usize>> = BTreeMap::new();
    for (i, n) in new.iter().enumerate() {
        queue.entry(key(n)).or_default().push_back(i);
    }
    let mut used = vec![false; new.len()];
    let mut out = Vec::with_capacity(old.len().max(new.len()));
    for o in old {
        let hit = queue.get_mut(key(o)).and_then(|q| q.pop_front());
        if let Some(i) = hit {
            used[i] = true;
        }
        out.push((Some(o), hit.and_then(|i| new.get(i))));
    }
    for (i, n) in new.iter().enumerate() {
        if !used[i] {
            out.push((None, Some(n)));
        }
    }
    out
}

/// Status of a matched pair from its fields.
fn status_of<T>(old: Option<&T>, new: Option<&T>, fields: &[Field]) -> Status {
    match (old, new) {
        (None, Some(_)) => Status::Added,
        (Some(_), None) => Status::Removed,
        _ if fields.iter().any(|f| f.status().is_change()) => Status::Modified,
        _ => Status::Unchanged,
    }
}

/// Items compared: label (new side's, else old), status, fields.
fn keyed_diff<T>(
    old: &[T],
    new: &[T],
    key: impl Fn(&T) -> &str,
    label: impl Fn(&T) -> String,
    values: impl Fn(&T) -> Vec<(String, String)>,
) -> Vec<(String, Status, Vec<Field>)> {
    pair_by(old, new, key)
        .into_iter()
        .map(|(o, n)| {
            let fields = merge_fields(o.map(&values).as_deref(), n.map(&values).as_deref());
            let name = n.or(o).map(&label).unwrap_or_default();
            (name, status_of(o, n, &fields), fields)
        })
        .collect()
}

fn font_values(f: &GlyphsFont) -> Vec<(String, String)> {
    let mut v = vec![
        ("Family".to_string(), f.family.clone()),
        ("Units per em".to_string(), num(f.units_per_em)),
        ("Version".to_string(), f.version.clone()),
    ];
    v.extend(f.info.iter().cloned());
    v
}

fn master_values(m: &Master, axes: &[String]) -> Vec<(String, String)> {
    let mut v = vec![("Name".to_string(), m.name.clone())];
    for (i, value) in m.axes.iter().enumerate() {
        let axis = axes
            .get(i)
            .cloned()
            .unwrap_or_else(|| format!("Axis {}", i + 1));
        v.push((axis, num(*value)));
    }
    v.extend(m.metrics.iter().map(|(k, x)| (k.clone(), num(*x))));
    v
}

fn glyphs_glyph_values(g: &GlyphsGlyph) -> Vec<(String, String)> {
    let mut v = Vec::new();
    if !g.unicodes.is_empty() {
        v.push(("Unicodes".to_string(), unicodes_text(&g.unicodes)));
    }
    v.push((
        "Export".to_string(),
        if g.export { "Yes" } else { "No" }.to_string(),
    ));
    if let Some(c) = &g.category {
        v.push(("Category".to_string(), c.clone()));
    }
    v
}

fn diff_glyphs_glyph(old: Option<&GlyphsGlyph>, new: Option<&GlyphsGlyph>) -> GlyphsGlyphChange {
    let name = new.or(old).map(|g| g.name.clone()).unwrap_or_default();
    let fields = merge_fields(
        old.map(glyphs_glyph_values).as_deref(),
        new.map(glyphs_glyph_values).as_deref(),
    );
    let empty: &[(String, SourceGlyph)] = &[];
    let layers: Vec<(String, GlyphChange)> = pair_by(
        old.map_or(empty, |g| &g.layers),
        new.map_or(empty, |g| &g.layers),
        |(k, _)| k.as_str(),
    )
    .into_iter()
    .map(|(o, n)| {
        let key = n.or(o).map(|(k, _)| k.clone()).unwrap_or_default();
        (key, diff_glif(o.map(|(_, g)| g), n.map(|(_, g)| g)))
    })
    .collect();
    let status = match status_of(old, new, &fields) {
        Status::Unchanged if layers.iter().any(|(_, l)| l.status.is_change()) => Status::Modified,
        s => s,
    };
    GlyphsGlyphChange {
        name,
        status,
        fields,
        layers,
    }
}

/// Two versions of a Glyphs file compared.
pub fn diff_glyphs(old: Option<&GlyphsFont>, new: Option<&GlyphsFont>) -> GlyphsDiff {
    let empty = GlyphsFont::default();
    let (o, n) = (old.unwrap_or(&empty), new.unwrap_or(&empty));

    let info = merge_fields(
        old.map(font_values).as_deref(),
        new.map(font_values).as_deref(),
    );
    let axes_of = |f: &GlyphsFont| -> Vec<(String, String)> {
        f.axes
            .iter()
            .enumerate()
            .map(|(i, a)| (format!("Axis {}", i + 1), a.clone()))
            .collect()
    };
    let axes = merge_fields(old.map(axes_of).as_deref(), new.map(axes_of).as_deref());

    // a master's values use its own side's axis names
    let masters = pair_by(&o.masters, &n.masters, |m| m.id.as_str())
        .into_iter()
        .map(|(om, nm)| {
            let fields = merge_fields(
                om.map(|m| master_values(m, &o.axes)).as_deref(),
                nm.map(|m| master_values(m, &n.axes)).as_deref(),
            );
            let name = nm.or(om).map(|m| m.name.clone()).unwrap_or_default();
            (name, status_of(om, nm, &fields), fields)
        })
        .collect();

    let instances = keyed_diff(
        &o.instances,
        &n.instances,
        |(name, _)| name.as_str(),
        |(name, _)| name.clone(),
        |(_, values)| values.clone(),
    );

    let glyphs = pair_by(&o.glyphs, &n.glyphs, |g| g.name.as_str())
        .into_iter()
        .map(|(og, ng)| diff_glyphs_glyph(og, ng))
        .collect();

    let features = pair_by(&o.features, &n.features, |(label, _)| label.as_str())
        .into_iter()
        .map(|(of, nf)| {
            let label = nf.or(of).map(|(l, _)| l.clone()).unwrap_or_default();
            (label, Status::of(of.map(|f| &f.1), nf.map(|f| &f.1)))
        })
        .collect();

    // kerning by master name, so a master's pairs line up across sides
    let master_name = |f: &GlyphsFont, id: &str| -> String {
        n.masters
            .iter()
            .chain(&f.masters)
            .find(|m| m.id == id)
            .map_or_else(|| id.to_string(), |m| m.name.clone())
    };
    let kern_map = |f: &GlyphsFont| -> BTreeMap<(String, String, String), f32> {
        f.kerning
            .iter()
            .map(|(m, l, r, v)| ((master_name(f, m), l.clone(), r.clone()), *v))
            .collect()
    };
    let (ok, nk) = (kern_map(o), kern_map(n));
    let mut kerning = Vec::new();
    for (k, v) in &ok {
        let new_v = nk.get(k).copied();
        if new_v != Some(*v) {
            kerning.push((k.0.clone(), k.1.clone(), k.2.clone(), Some(*v), new_v));
        }
    }
    for (k, v) in &nk {
        if !ok.contains_key(k) {
            kerning.push((k.0.clone(), k.1.clone(), k.2.clone(), None, Some(*v)));
        }
    }
    kerning.sort_by(|a, b| (&a.0, &a.1, &a.2).cmp(&(&b.0, &b.1, &b.2)));

    GlyphsDiff {
        info,
        masters,
        axes,
        instances,
        glyphs,
        features,
        kerning,
    }
}

fn location_values(loc: &[(String, f32)]) -> impl Iterator<Item = (String, String)> + '_ {
    loc.iter()
        .map(|(axis, v)| (format!("Location {axis}"), num(*v)))
}

fn axis_values(a: &DsAxis) -> Vec<(String, String)> {
    let mut v = vec![
        ("Tag".to_string(), a.tag.clone()),
        ("Minimum".to_string(), num(a.min)),
        ("Default".to_string(), num(a.default)),
        ("Maximum".to_string(), num(a.max)),
    ];
    if !a.values.is_empty() {
        let list: Vec<String> = a.values.iter().map(|x| num(*x)).collect();
        v.push(("Values".into(), list.join(" ")));
    }
    if !a.map.is_empty() {
        let list: Vec<String> = a
            .map
            .iter()
            .map(|(i, o)| format!("{}→{}", num(*i), num(*o)))
            .collect();
        v.push(("Map".into(), list.join(", ")));
    }
    if a.hidden {
        v.push(("Hidden".into(), "Yes".into()));
    }
    v
}

fn source_values(s: &DsSource) -> Vec<(String, String)> {
    let mut v = vec![
        ("File".to_string(), s.filename.clone()),
        ("Family".to_string(), s.family.clone()),
        ("Style".to_string(), s.style.clone()),
    ];
    if let Some(l) = &s.layer {
        v.push(("Layer".into(), l.clone()));
    }
    v.extend(location_values(&s.location));
    v
}

fn instance_values(i: &DsInstance) -> Vec<(String, String)> {
    let mut v = vec![
        ("Family".to_string(), i.family.clone()),
        ("Style".to_string(), i.style.clone()),
        ("File".to_string(), i.filename.clone()),
    ];
    v.extend(location_values(&i.location));
    v
}

fn rule_values(r: &DsRule) -> Vec<(String, String)> {
    let mut v = Vec::new();
    for (axis, lo, hi) in &r.conditions {
        let lo = lo.map(num).unwrap_or_else(|| "…".into());
        let hi = hi.map(num).unwrap_or_else(|| "…".into());
        v.push((format!("Condition {axis}"), format!("{lo} to {hi}")));
    }
    for (from, to) in &r.subs {
        v.push((format!("Substitute {from}"), to.clone()));
    }
    v
}

/// Two versions of a designspace compared.
pub fn diff_designspace(old: Option<&Designspace>, new: Option<&Designspace>) -> DesignspaceDiff {
    let empty = Designspace::default();
    let (o, n) = (old.unwrap_or(&empty), new.unwrap_or(&empty));
    DesignspaceDiff {
        axes: keyed_diff(
            &o.axes,
            &n.axes,
            |a| a.name.as_str(),
            |a| a.name.clone(),
            axis_values,
        ),
        sources: keyed_diff(
            &o.sources,
            &n.sources,
            |s| s.name.as_str(),
            |s| s.name.clone(),
            source_values,
        ),
        instances: keyed_diff(
            &o.instances,
            &n.instances,
            |i| i.name.as_str(),
            |i| i.name.clone(),
            instance_values,
        ),
        rules: keyed_diff(
            &o.rules,
            &n.rules,
            |r| r.name.as_str(),
            |r| r.name.clone(),
            rule_values,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pts(list: &[(f32, f32, PointKind)]) -> Vec<SrcPoint> {
        list.iter()
            .map(|&(x, y, kind)| SrcPoint { x, y, kind })
            .collect()
    }

    #[test]
    fn kinds_by_extension() {
        assert_eq!(
            source_kind("Font.ufo/glyphs/A_.glif"),
            Some(SourceKind::Glif)
        );
        assert_eq!(source_kind("src/Font.GLYPHS"), Some(SourceKind::Glyphs));
        assert_eq!(
            source_kind("Font.designspace"),
            Some(SourceKind::Designspace)
        );
        assert_eq!(
            source_kind("Font.glyphspackage/glyphs/A_.glyph"),
            Some(SourceKind::Glyphs)
        );
        assert_eq!(
            source_kind("Font.glyphspackage/fontinfo.plist"),
            Some(SourceKind::Glyphs)
        );
        assert_eq!(source_kind("glyphs/A.glyph"), None);
        assert_eq!(source_kind("Font.ufo/fontinfo.plist"), None);
        assert_eq!(source_kind("README"), None);
    }

    #[test]
    fn closed_lines_drop_the_implied_closing_line() {
        use PointKind::*;
        let mut out = Vec::new();
        contour_cmds(
            &pts(&[(0., 0., Line), (100., 0., Line), (100., 100., Line)]),
            &mut out,
        );
        assert_eq!(
            out,
            vec![
                PathCmd::MoveTo(0., 0.),
                PathCmd::LineTo(100., 0.),
                PathCmd::LineTo(100., 100.),
                PathCmd::Close
            ]
        );
    }

    #[test]
    fn open_contour_is_not_closed_and_drops_trailing_controls() {
        use PointKind::*;
        let mut out = Vec::new();
        contour_cmds(
            &pts(&[(0., 0., Move), (10., 0., Line), (20., 5., OffCurve)]),
            &mut out,
        );
        assert_eq!(out, vec![PathCmd::MoveTo(0., 0.), PathCmd::LineTo(10., 0.)]);
    }

    #[test]
    fn super_bezier_splits_into_cubics() {
        use PointKind::*;
        let mut out = Vec::new();
        contour_cmds(
            &pts(&[
                (0., 0., Line),
                (0., 30., OffCurve),
                (30., 60., OffCurve),
                (60., 60., OffCurve),
                (90., 30., Curve),
            ]),
            &mut out,
        );
        // as fontTools' decomposeSuperBezierSegment splits it; the closing
        // line back to the start is implied
        assert_eq!(
            out,
            vec![
                PathCmd::MoveTo(0., 0.),
                PathCmd::CubicTo(0., 30., 15., 45., 30., 52.5),
                PathCmd::CubicTo(45., 60., 60., 60., 90., 30.),
                PathCmd::Close,
            ]
        );
    }

    #[test]
    fn flatten_resolves_nested_components_and_reports_cycles() {
        let square = OutlineItem::Contour(vec![
            PathCmd::MoveTo(0., 0.),
            PathCmd::LineTo(10., 0.),
            PathCmd::Close,
        ]);
        let comp = |base: &str, dx: f32| {
            OutlineItem::Component(Component {
                base: base.into(),
                transform: [1., 0., 0., 1., dx, 0.],
            })
        };
        let lib: BTreeMap<String, Vec<OutlineItem>> = [
            ("a".to_string(), vec![square.clone()]),
            ("b".to_string(), vec![comp("a", 5.)]),
            ("x".to_string(), vec![comp("y", 0.)]),
            ("y".to_string(), vec![comp("x", 0.)]),
        ]
        .into_iter()
        .collect();
        let lookup = |n: &str| lib.get(n).map(Vec::as_slice);
        let mut unresolved = Vec::new();
        let p = flatten(
            &[comp("b", 1.), comp("missing", 0.), comp("x", 0.)],
            &lookup,
            &mut unresolved,
        );
        assert_eq!(p.cmds[0], PathCmd::MoveTo(6., 0.));
        assert_eq!(p.contours(), 1);
        assert_eq!(unresolved, vec!["missing".to_string(), "x".to_string()]);
    }

    fn glyph(name: &str, advance: f32) -> SourceGlyph {
        SourceGlyph {
            name: name.into(),
            advance,
            path: GlyphPath {
                cmds: vec![
                    PathCmd::MoveTo(0., 0.),
                    PathCmd::LineTo(10., 0.),
                    PathCmd::LineTo(10., 10.),
                    PathCmd::Close,
                ],
            },
            ..SourceGlyph::default()
        }
    }

    #[test]
    fn glif_diff_statuses_and_fields() {
        let a = glyph("a", 500.);
        assert_eq!(diff_glif(None, Some(&a)).status, Status::Added);
        assert_eq!(diff_glif(Some(&a), None).status, Status::Removed);
        assert_eq!(diff_glif(Some(&a), Some(&a)).status, Status::Unchanged);

        let mut b = a.clone();
        b.advance = 520.;
        b.anchors.push(Anchor {
            name: "top".into(),
            x: 5.,
            y: 700.,
        });
        let d = diff_glif(Some(&a), Some(&b));
        assert_eq!(d.status, Status::Modified);
        let changed: Vec<&str> = d
            .fields
            .iter()
            .filter(|f| f.status().is_change())
            .map(|f| f.label.as_str())
            .collect();
        assert_eq!(changed, vec!["Advance width", "Anchor top"]);

        // an outline change with the same counts and bounds still counts
        let mut c = a.clone();
        c.path.cmds[2] = PathCmd::LineTo(0., 10.);
        c.path.cmds.swap(1, 2);
        assert_eq!(diff_glif(Some(&a), Some(&c)).status, Status::Modified);
    }

    #[test]
    fn designspace_diff_matches_by_name() {
        let axis = |name: &str, max: f32| DsAxis {
            tag: "wght".into(),
            name: name.into(),
            min: 100.,
            default: 400.,
            max,
            ..DsAxis::default()
        };
        let old = Designspace {
            axes: vec![axis("Weight", 900.), axis("Gone", 1.)],
            ..Designspace::default()
        };
        let new = Designspace {
            axes: vec![axis("New", 1.), axis("Weight", 700.)],
            ..Designspace::default()
        };
        let d = diff_designspace(Some(&old), Some(&new));
        let s: Vec<(&str, Status)> = d.axes.iter().map(|(n, s, _)| (n.as_str(), *s)).collect();
        assert_eq!(
            s,
            vec![
                ("Weight", Status::Modified),
                ("Gone", Status::Removed),
                ("New", Status::Added)
            ]
        );
        let max = d.axes[0].2.iter().find(|f| f.label == "Maximum").unwrap();
        assert_eq!(
            (max.old.as_deref(), max.new.as_deref()),
            (Some("900"), Some("700"))
        );
    }

    #[test]
    fn numbers_print_like_an_editor() {
        assert_eq!(num(500.0), "500");
        assert_eq!(num(-0.0), "0");
        assert_eq!(num(12.5), "12.5");
    }
}
