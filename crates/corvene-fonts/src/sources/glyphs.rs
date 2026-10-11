//! Glyphs `.glyphs` files, formats 2 and 3, read the way glyphsLib reads
//! them (node order, component transforms, layer widths default to 600).
//! A `.glyphspackage`'s `fontinfo.plist` reads as a font without glyphs
//! and one of its `glyphs/*.glyph` files as a font with a single glyph.

use std::collections::BTreeMap;

use super::openstep::{self, Value};
use super::{
    Anchor, Component, GlyphsFont, GlyphsGlyph, Master, OutlineItem, PointKind, SourceGlyph,
    SrcPoint, contour_cmds, flatten,
};
use crate::FontError;

const FORMAT: &str = "Glyphs";

/// Summaries of parameter values are cut at this many bytes.
const SUMMARY_MAX: usize = 160;

/// Reads a `.glyphs` file (or a `.glyphspackage` part).
pub fn parse_glyphs(bytes: &[u8]) -> Result<GlyphsFont, FontError> {
    let text = String::from_utf8_lossy(bytes);
    // glyphs are read one by one as the parser meets them
    let mut raw: Vec<RawGlyph> = Vec::new();
    let root = openstep::parse_streaming(&text, "glyphs", &mut |g| raw.push(raw_glyph(&g)))
        .map_err(|e| FontError::malformed(FORMAT, e))?;
    if !matches!(root, Value::Dict(_)) {
        return Err(FontError::malformed(FORMAT, "top level is not a dict"));
    }
    // a package's glyph file is one glyph dict (packages are format 3)
    let single = root.get("glyphname").is_some();
    let version = root
        .get_num(".formatVersion")
        .map_or(if single { 3 } else { 2 }, |v| v as u32);
    let g3 = version >= 3;

    let axes = axes(&root, g3);
    let masters: Vec<Master> = root
        .get_items("fontMaster")
        .iter()
        .map(|m| master(m, &root, g3, axes.len()))
        .collect();
    if single {
        raw.push(raw_glyph(&root));
    }
    // format 2 writes code points in hex, format 3 in decimal
    let radix = if g3 { 10 } else { 16 };
    for g in &mut raw {
        g.unicodes = g
            .unicode_text
            .iter()
            .filter_map(|u| u32::from_str_radix(u.trim(), radix).ok())
            .collect();
    }

    let major = root.get_num("versionMajor").unwrap_or(1.0) as i64;
    let minor = root.get_num("versionMinor").unwrap_or(0.0) as i64;
    let mut font = GlyphsFont {
        family: root.get_str("familyName").unwrap_or_default().to_string(),
        units_per_em: root.get_num("unitsPerEm").unwrap_or(1000.0),
        version: format!("{major}.{minor:03}"),
        info: info(&root, version),
        axes: axes
            .iter()
            .map(|(name, tag, hidden)| {
                let mut s = if tag.is_empty() {
                    name.clone()
                } else {
                    format!("{name} ({tag})")
                };
                if *hidden {
                    s.push_str(", hidden");
                }
                s
            })
            .collect(),
        masters,
        instances: root
            .get_items("instances")
            .iter()
            .map(|i| instance(i, &axes))
            .collect(),
        glyphs: Vec::new(),
        features: features(&root),
        kerning: kerning(&root),
    };
    // the parsed tree is the bulk of the memory; free it before drawing
    drop(root);
    drop(text);
    font.glyphs = resolve(&raw);
    Ok(font)
}

/// Axis name, tag, hidden.
type Axis = (String, String, bool);

fn axes(root: &Value, g3: bool) -> Vec<Axis> {
    if g3 {
        return root
            .get_items("axes")
            .iter()
            .map(|a| {
                (
                    a.get_str("name").unwrap_or_default().to_string(),
                    a.get_str("tag").unwrap_or_default().to_string(),
                    a.get_str("hidden") == Some("1"),
                )
            })
            .collect();
    }
    let param = root
        .get_items("customParameters")
        .iter()
        .find(|p| p.get_str("name") == Some("Axes"));
    if let Some(p) = param {
        return p
            .get_items("value")
            .iter()
            .map(|a| {
                (
                    a.get_str("Name").unwrap_or_default().to_string(),
                    a.get_str("Tag").unwrap_or_default().to_string(),
                    a.get_str("Hidden") == Some("1"),
                )
            })
            .collect();
    }
    // glyphsLib's defaults for a format 2 file without an "Axes" parameter
    let mut axes = vec![
        ("Weight".to_string(), "wght".to_string(), false),
        ("Width".to_string(), "wdth".to_string(), false),
    ];
    let custom = root
        .get_items("fontMaster")
        .iter()
        .any(|m| m.get("customValue").is_some());
    if custom {
        axes.push(("Custom".to_string(), "XXXX".to_string(), false));
    }
    axes
}

/// Format 2 master keys for the axes, in order, with their defaults.
const G2_AXIS_KEYS: [(&str, f32); 6] = [
    ("weightValue", 100.0),
    ("widthValue", 100.0),
    ("customValue", 0.0),
    ("customValue1", 0.0),
    ("customValue2", 0.0),
    ("customValue3", 0.0),
];

/// Format 2 master metrics: label, key, default.
const G2_METRICS: [(&str, &str, f32); 5] = [
    ("Ascender", "ascender", 800.0),
    ("Cap height", "capHeight", 700.0),
    ("x-height", "xHeight", 500.0),
    ("Descender", "descender", -200.0),
    ("Italic angle", "italicAngle", 0.0),
];

fn custom_param<'v, 'a>(v: &'v Value<'a>, name: &str) -> Option<&'v Value<'a>> {
    v.get_items("customParameters")
        .iter()
        .find(|p| p.get_str("name") == Some(name))
        .and_then(|p| p.get("value"))
}

fn master(m: &Value, root: &Value, g3: bool, axis_count: usize) -> Master {
    let id = m.get_str("id").unwrap_or_default().to_string();
    if g3 {
        let axes = m
            .get_items("axesValues")
            .iter()
            .filter_map(Value::num)
            .collect();
        let values = m.get_items("metricValues");
        let metrics = root
            .get_items("metrics")
            .iter()
            .enumerate()
            .map(|(i, def)| {
                let pos = values.get(i).and_then(|v| v.get_num("pos")).unwrap_or(0.0);
                (metric_label(def), pos)
            })
            .collect();
        let name = custom_param(m, "Master Name")
            .and_then(Value::str)
            .or(m.get_str("name"))
            .unwrap_or("Regular")
            .to_string();
        return Master {
            id,
            name,
            axes,
            metrics,
        };
    }
    let axes = G2_AXIS_KEYS
        .iter()
        .take(axis_count)
        .map(|(key, default)| m.get_num(key).unwrap_or(*default))
        .collect();
    let metrics = G2_METRICS
        .iter()
        .map(|(label, key, default)| (label.to_string(), m.get_num(key).unwrap_or(*default)))
        .collect();
    Master {
        id,
        name: g2_master_name(m),
        axes,
        metrics,
    }
}

/// glyphsLib's name for a format 2 master: "Master Name", else width,
/// weight and custom name joined without redundant "Regular"s.
fn g2_master_name(m: &Value) -> String {
    if let Some(name) = custom_param(m, "Master Name").and_then(Value::str) {
        return name.to_string();
    }
    if let Some(name) = m.get_str("name") {
        return name.to_string();
    }
    let mut parts: Vec<&str> = [
        m.get_str("width").unwrap_or("Regular"),
        m.get_str("weight").unwrap_or("Regular"),
        m.get_str("custom").unwrap_or(""),
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect();
    while parts.len() > 1 && parts.contains(&"Regular") {
        if let Some(i) = parts.iter().position(|p| *p == "Regular") {
            parts.remove(i);
        }
    }
    if parts.is_empty() {
        "Regular".to_string()
    } else {
        parts.join(" ")
    }
}

fn metric_label(def: &Value) -> String {
    let base = match def.get_str("type") {
        Some("ascender") => "Ascender".to_string(),
        Some("cap height") => "Cap height".to_string(),
        Some("x-height") => "x-height".to_string(),
        Some("baseline") => "Baseline".to_string(),
        Some("descender") => "Descender".to_string(),
        Some("italic angle") => "Italic angle".to_string(),
        Some("topHeight") => "Top height".to_string(),
        Some("bodyHeight") => "Body height".to_string(),
        Some(other) => other.to_string(),
        None => def.get_str("name").unwrap_or("Metric").to_string(),
    };
    match def.get("filter") {
        Some(f) => format!("{base} ({})", f.summary(SUMMARY_MAX)),
        None => base,
    }
}

/// Font-level keys shown as info: key, label.
const INFO_KEYS: [(&str, &str); 11] = [
    ("copyright", "Copyright"),
    ("designer", "Designer"),
    ("designerURL", "Designer URL"),
    ("manufacturer", "Manufacturer"),
    ("manufacturerURL", "Manufacturer URL"),
    ("date", "Date"),
    ("note", "Note"),
    ("gridLength", "Grid length"),
    ("keyboardIncrement", "Keyboard increment"),
    ("disablesAutomaticAlignment", "Disables automatic alignment"),
    ("disablesNiceNames", "Disables nice names"),
];

fn property_label(key: &str) -> String {
    match key {
        "copyrights" => "Copyright",
        "designers" | "designer" => "Designer",
        "designerURL" => "Designer URL",
        "manufacturers" | "manufacturer" => "Manufacturer",
        "manufacturerURL" => "Manufacturer URL",
        "familyNames" => "Family names",
        "licenses" => "License",
        "licenseURL" => "License URL",
        "trademarks" => "Trademark",
        "descriptions" => "Description",
        "sampleTexts" => "Sample text",
        "vendorID" => "Vendor ID",
        "versionString" => "Version string",
        "uniqueID" => "Unique ID",
        "compatibleFullNames" => "Compatible full names",
        "postscriptFullNames" => "PostScript full names",
        "postscriptFontName" => "PostScript name",
        other => other,
    }
    .to_string()
}

fn info(root: &Value, version: u32) -> Vec<(String, String)> {
    let mut out = vec![("Format version".to_string(), version.to_string())];
    if let Some(v) = root.get_str(".appVersion") {
        out.push(("App version".into(), v.to_string()));
    }
    for (key, label) in INFO_KEYS {
        if let Some(v) = root.get(key) {
            out.push((label.to_string(), v.summary(SUMMARY_MAX)));
        }
    }
    // format 3 properties: one value, or localized values
    for p in root.get_items("properties") {
        let label = property_label(p.get_str("key").unwrap_or_default());
        let value = match p.get("value") {
            Some(v) => v.summary(SUMMARY_MAX),
            None => {
                let values = p.get_items("values");
                match values {
                    [one] => one
                        .get("value")
                        .map(|v| v.summary(SUMMARY_MAX))
                        .unwrap_or_default(),
                    _ => values
                        .iter()
                        .map(|v| {
                            format!(
                                "{}: {}",
                                v.get_str("language").unwrap_or("?"),
                                v.get("value")
                                    .map(|v| v.summary(SUMMARY_MAX))
                                    .unwrap_or_default()
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("; "),
                }
            }
        };
        out.push((label, value));
    }
    out.extend(parameters(root, &["Axes"]));
    let keys: Vec<&str> = root.get("userData").map_or(Vec::new(), |u| {
        u.entries().iter().map(|(k, _)| k.as_ref()).collect()
    });
    if !keys.is_empty() {
        out.push(("User data keys".into(), keys.join(", ")));
    }
    out
}

/// Custom parameters as "Parameter <name>" fields, except `skip`.
fn parameters(v: &Value, skip: &[&str]) -> Vec<(String, String)> {
    v.get_items("customParameters")
        .iter()
        .filter_map(|p| {
            let name = p.get_str("name")?;
            if skip.contains(&name) {
                return None;
            }
            let value = p
                .get("value")
                .map(|v| v.summary(SUMMARY_MAX))
                .unwrap_or_default();
            Some((format!("Parameter {name}"), value))
        })
        .collect()
}

fn instance(i: &Value, axes: &[Axis]) -> (String, Vec<(String, String)>) {
    let name = i.get_str("name").unwrap_or_default().to_string();
    let mut values = Vec::new();
    for (key, v) in i.entries() {
        match key.as_ref() {
            "name" => {}
            "axesValues" => {
                for (n, x) in v.items().iter().enumerate() {
                    let axis = axes
                        .get(n)
                        .map_or_else(|| format!("Axis {}", n + 1), |a| a.0.clone());
                    values.push((axis, x.summary(SUMMARY_MAX)));
                }
            }
            "customParameters" => values.extend(parameters(i, &[])),
            _ => values.push((key.to_string(), v.summary(SUMMARY_MAX))),
        }
    }
    (name, values)
}

fn features(root: &Value) -> Vec<(String, String)> {
    let code = |f: &Value| {
        let code = f.get_str("code").unwrap_or_default();
        if f.get_str("disabled") == Some("1") {
            format!("# disabled\n{code}")
        } else {
            code.to_string()
        }
    };
    let mut out = Vec::new();
    for p in root.get_items("featurePrefixes") {
        let name = p.get_str("name").unwrap_or_default();
        out.push((format!("Prefix {name}"), code(p)));
    }
    for c in root.get_items("classes") {
        let name = c.get_str("name").unwrap_or_default();
        out.push((format!("@{name}"), code(c)));
    }
    for f in root.get_items("features") {
        let tag = f.get_str("tag").or(f.get_str("name")).unwrap_or_default();
        out.push((tag.to_string(), code(f)));
    }
    out
}

fn kerning(root: &Value) -> Vec<(String, String, String, f32)> {
    let Some(table) = root.get("kerningLTR").or(root.get("kerning")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (master, lefts) in table.entries() {
        for (left, rights) in lefts.entries() {
            for (right, value) in rights.entries() {
                if let Some(v) = value.num() {
                    out.push((master.to_string(), left.to_string(), right.to_string(), v));
                }
            }
        }
    }
    out
}

struct RawLayer {
    key: String,
    id: String,
    master: Option<String>,
    width: f32,
    height: Option<f32>,
    items: Vec<OutlineItem>,
    anchors: Vec<Anchor>,
    guides: usize,
    keys: Vec<String>,
}

impl RawLayer {
    fn is_master(&self) -> bool {
        self.master.as_ref().is_none_or(|m| *m == self.id)
    }
}

struct RawGlyph {
    name: String,
    /// Code points as written, read once the format is known.
    unicode_text: Vec<String>,
    unicodes: Vec<u32>,
    category: Option<String>,
    export: bool,
    note: Option<String>,
    keys: Vec<String>,
    layers: Vec<RawLayer>,
}

fn user_keys(v: &Value) -> Vec<String> {
    v.get("userData")
        .map(|u| u.entries().iter().map(|(k, _)| k.to_string()).collect())
        .unwrap_or_default()
}

fn raw_glyph(g: &Value) -> RawGlyph {
    let unicode_text = match g.get("unicode") {
        Some(Value::Str(s)) => s.split(',').map(str::to_string).collect(),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    };
    RawGlyph {
        name: g.get_str("glyphname").unwrap_or_default().to_string(),
        unicode_text,
        unicodes: Vec::new(),
        category: g.get_str("category").map(str::to_string),
        export: g.get_str("export") != Some("0"),
        note: g
            .get_str("note")
            .map(str::to_string)
            .filter(|n| !n.is_empty()),
        keys: user_keys(g),
        layers: g.get_items("layers").iter().map(raw_layer).collect(),
    }
}

fn raw_layer(l: &Value) -> RawLayer {
    let id = l.get_str("layerId").unwrap_or_default().to_string();
    let master = l.get_str("associatedMasterId").map(str::to_string);
    let is_master = master.as_ref().is_none_or(|m| *m == id);
    let key = if is_master {
        id.clone()
    } else if let Some(name) = l.get_str("name") {
        name.to_string()
    } else if let Some(c) = l.get("attr").and_then(|a| a.get("coordinates")) {
        format!(
            "{{{}}}",
            c.items()
                .iter()
                .filter_map(Value::str)
                .collect::<Vec<_>>()
                .join(", ")
        )
    } else {
        id.clone()
    };

    let mut items = Vec::new();
    for shape in l.get_items("shapes").iter().chain(l.get_items("paths")) {
        if shape.get("nodes").is_some() {
            if let Some(c) = path(shape) {
                items.push(OutlineItem::Contour(c));
            }
        } else if let Some(c) = component(shape) {
            items.push(OutlineItem::Component(c));
        }
    }
    for c in l.get_items("components") {
        if let Some(c) = component(c) {
            items.push(OutlineItem::Component(c));
        }
    }
    let anchors = l
        .get_items("anchors")
        .iter()
        .map(|a| {
            let (x, y) = a
                .get("pos")
                .or(a.get("position"))
                .and_then(pair)
                .unwrap_or((0.0, 0.0));
            Anchor {
                name: a.get_str("name").unwrap_or_default().to_string(),
                x,
                y,
            }
        })
        .collect();
    RawLayer {
        key,
        id,
        master,
        width: l.get_num("width").unwrap_or(600.0),
        height: l.get_num("vertWidth"),
        items,
        anchors,
        guides: l.get_items("guides").len() + l.get_items("guideLines").len(),
        keys: user_keys(l),
    }
}

/// Numbers out of `(1, 2)` or `"{1, 2}"`.
fn numbers(v: &Value) -> Vec<f32> {
    match v {
        Value::Array(a) => a.iter().filter_map(Value::num).collect(),
        Value::Str(s) => s
            .trim_matches(|c: char| c == '{' || c == '}' || c.is_whitespace())
            .split(',')
            .filter_map(|n| n.trim().parse::<f32>().ok())
            .collect(),
        Value::Dict(_) => Vec::new(),
    }
}

fn pair(v: &Value) -> Option<(f32, f32)> {
    match numbers(v).as_slice() {
        [x, y, ..] => Some((*x, *y)),
        _ => None,
    }
}

fn node(n: &Value) -> Option<SrcPoint> {
    let (x, y, kind) = match n {
        Value::Str(s) => {
            let mut it = s.split_whitespace();
            (it.next()?, it.next()?, it.next().unwrap_or("LINE"))
        }
        Value::Array(a) => (
            a.first()?.str()?,
            a.get(1)?.str()?,
            a.get(2).and_then(Value::str)?,
        ),
        Value::Dict(_) => return None,
    };
    let kind = match kind.as_bytes().first().map(u8::to_ascii_lowercase) {
        Some(b'c') => PointKind::Curve,
        Some(b'q') => PointKind::QCurve,
        Some(b'o') => PointKind::OffCurve,
        _ => PointKind::Line,
    };
    Some(SrcPoint {
        x: x.parse().ok()?,
        y: y.parse().ok()?,
        kind,
    })
}

/// A path drawn like glyphsLib: an open path starts with a `move`; a closed
/// one stores its start point last.
fn path(p: &Value) -> Option<Vec<crate::outline::PathCmd>> {
    let mut points: Vec<SrcPoint> = p.get_items("nodes").iter().filter_map(node).collect();
    if points.is_empty() {
        return None;
    }
    if p.get_str("closed") == Some("0") {
        points[0].kind = PointKind::Move;
    } else {
        points.rotate_right(1);
    }
    let mut cmds = Vec::new();
    contour_cmds(&points, &mut cmds);
    Some(cmds)
}

fn component(c: &Value) -> Option<Component> {
    let base = c.get_str("ref").or(c.get_str("name"))?.to_string();
    if let Some(t) = c.get("transform") {
        let m = numbers(t);
        let transform = match m.as_slice() {
            [a, b, cc, d, e, f] => [*a, *b, *cc, *d, *e, *f],
            _ => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        };
        return Some(Component { base, transform });
    }
    // format 3: scaled, rotated, slanted, then moved
    let (x, y) = c.get("pos").and_then(pair).unwrap_or((0.0, 0.0));
    let (sx, sy) = c.get("scale").and_then(pair).unwrap_or((1.0, 1.0));
    let angle = c.get_num("angle").unwrap_or(0.0);
    let (kx, ky) = c.get("slant").and_then(pair).unwrap_or((0.0, 0.0));
    let transform = if angle == 0.0 && kx == 0.0 && ky == 0.0 {
        [sx, 0.0, 0.0, sy, x, y]
    } else {
        let (sin, cos) = angle.to_radians().sin_cos();
        let scale = [sx, 0.0, 0.0, sy, 0.0, 0.0];
        let rotate = [cos, sin, -sin, cos, 0.0, 0.0];
        let skew = [
            1.0,
            ky.to_radians().tan(),
            kx.to_radians().tan(),
            1.0,
            0.0,
            0.0,
        ];
        let translate = [1.0, 0.0, 0.0, 1.0, x, y];
        mul(translate, mul(skew, mul(rotate, scale)))
    };
    Some(Component { base, transform })
}

/// `p` after `q`.
fn mul(p: [f32; 6], q: [f32; 6]) -> [f32; 6] {
    [
        p[0] * q[0] + p[2] * q[1],
        p[1] * q[0] + p[3] * q[1],
        p[0] * q[2] + p[2] * q[3],
        p[1] * q[2] + p[3] * q[3],
        p[0] * q[4] + p[2] * q[5] + p[4],
        p[1] * q[4] + p[3] * q[5] + p[5],
    ]
}

/// Draws every layer, components resolved within the file through the
/// base glyph's layer for the same master.
fn resolve(raw: &[RawGlyph]) -> Vec<GlyphsGlyph> {
    let mut index: BTreeMap<&str, usize> = BTreeMap::new();
    for (i, g) in raw.iter().enumerate() {
        index.entry(g.name.as_str()).or_insert(i);
    }
    raw.iter()
        .map(|g| {
            let layers = g
                .layers
                .iter()
                .map(|layer| {
                    let lookup = |base: &str| -> Option<&[OutlineItem]> {
                        let b = &raw[*index.get(base)?];
                        let hit = b
                            .layers
                            .iter()
                            .find(|l| l.id == layer.id)
                            .or_else(|| {
                                let m = layer.master.as_deref()?;
                                b.layers.iter().find(|l| l.id == m)
                            })
                            .or_else(|| b.layers.iter().find(|l| l.is_master()))?;
                        Some(&hit.items)
                    };
                    let mut unresolved = Vec::new();
                    let path = flatten(&layer.items, &lookup, &mut unresolved);
                    let mut lib_keys = g.keys.clone();
                    lib_keys.extend(layer.keys.iter().cloned());
                    let glyph = SourceGlyph {
                        name: g.name.clone(),
                        unicodes: g.unicodes.clone(),
                        advance: layer.width,
                        height: layer.height,
                        path,
                        components: layer
                            .items
                            .iter()
                            .filter_map(|i| match i {
                                OutlineItem::Component(c) => Some(c.clone()),
                                OutlineItem::Contour(_) => None,
                            })
                            .collect(),
                        anchors: layer.anchors.clone(),
                        guidelines: layer.guides,
                        note: g.note.clone(),
                        lib_keys,
                        unresolved,
                    };
                    (layer.key.clone(), glyph)
                })
                .collect();
            GlyphsGlyph {
                name: g.name.clone(),
                unicodes: g.unicodes.clone(),
                category: g.category.clone(),
                export: g.export,
                layers,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compare::Status;
    use crate::outline::PathCmd;
    use crate::sources::diff_glyphs;

    const G2: &str = r#"{
.appVersion = "1342";
copyright = "(c) Someone";
customParameters = (
{
name = "Use Typo Metrics";
value = 1;
}
);
designer = "Jane Doe";
familyName = "Test Sans";
featurePrefixes = (
{
code = "languagesystem DFLT dflt;";
name = Languagesystems;
}
);
classes = (
{
code = "A B";
name = Upper;
}
);
features = (
{
code = "sub f i by f_i;";
name = liga;
}
);
fontMaster = (
{
ascender = 750;
capHeight = 700;
descender = -250;
id = "M1";
xHeight = 500;
},
{
ascender = 760;
id = "M2";
weight = Bold;
weightValue = 700;
xHeight = 520;
}
);
glyphs = (
{
glyphname = A;
layers = (
{
layerId = "M1";
paths = (
{
closed = 1;
nodes = (
"100 0 LINE",
"200 700 LINE",
"300 0 LINE"
);
}
);
width = 400;
},
{
layerId = "M2";
anchors = (
{
name = top;
position = "{250, 700}";
}
);
paths = (
{
closed = 1;
nodes = (
"110 0 LINE",
"210 700 LINE",
"330 0 OFFCURVE",
"330 100 OFFCURVE",
"310 0 CURVE SMOOTH"
);
}
);
width = 450;
}
);
unicode = 0041;
},
{
glyphname = Aacute;
export = 0;
layers = (
{
components = (
{
name = A;
},
{
name = acutecomb;
transform = "{1, 0, 0, 1, 100, 0}";
}
);
layerId = "M1";
width = 400;
},
{
components = (
{
name = A;
transform = "{-1, 0, 0, 1, 450, 0}";
}
);
layerId = "M2";
width = 450;
}
);
unicode = "00C1,01FA";
}
);
instances = (
{
interpolationWeight = 400;
name = Regular;
},
{
interpolationWeight = 700;
isBold = 1;
name = Bold;
}
);
kerning = {
"M1" = {
A = {
V = -50;
};
};
"M2" = {
A = {
V = -60;
};
};
};
unitsPerEm = 1000;
versionMajor = 1;
versionMinor = 2;
}"#;

    const G3: &str = r#"{
.appVersion = "3260";
.formatVersion = 3;
axes = (
{
name = Weight;
tag = wght;
}
);
familyName = "Test Sans";
features = (
{
code = "sub f i by f_i;";
tag = liga;
},
{
code = "pos A V -40;";
tag = kern;
}
);
fontMaster = (
{
axesValues = (
400
);
id = m01;
metricValues = (
{
over = 16;
pos = 750;
},
{
pos = 700;
},
{
}
);
name = Regular;
},
{
axesValues = (
700
);
id = m02;
metricValues = (
{
pos = 760;
},
{
pos = 710;
},
{
}
);
name = Bold;
}
);
glyphs = (
{
glyphname = A;
layers = (
{
layerId = m01;
shapes = (
{
closed = 1;
nodes = (
(100,0,l),
(200,700,l),
(300,0,l)
);
}
);
width = 400;
},
{
layerId = m02;
shapes = (
{
closed = 1;
nodes = (
(110,0,l),
(210,700,ls),
(330,0,o),
(330,100,o),
(310,0,cs)
);
}
);
},
{
associatedMasterId = m01;
layerId = "B1";
name = "Mar 1 21, 10:00";
width = 400;
}
);
unicode = 65;
},
{
glyphname = Aacute;
layers = (
{
layerId = m01;
shapes = (
{
ref = A;
},
{
pos = (100,0);
ref = acutecomb;
}
);
width = 400;
},
{
layerId = m02;
shapes = (
{
angle = 90;
pos = (450,0);
ref = A;
scale = (2,1);
}
);
width = 450;
}
);
unicode = (193,506);
}
);
instances = (
{
axesValues = (
400
);
name = Regular;
}
);
kerningLTR = {
m01 = {
A = {
V = -50;
};
};
};
metrics = (
{
type = ascender;
},
{
type = "cap height";
},
{
type = baseline;
}
);
properties = (
{
key = designers;
values = (
{
language = dflt;
value = "Jane Doe";
}
);
}
);
unitsPerEm = 1000;
}"#;

    #[test]
    fn reads_a_format_2_file() {
        let f = parse_glyphs(G2.as_bytes()).unwrap();
        assert_eq!(f.family, "Test Sans");
        assert_eq!(f.version, "1.002");
        assert_eq!(f.axes, vec!["Weight (wght)", "Width (wdth)"]);
        assert_eq!(f.masters.len(), 2);
        assert_eq!(f.masters[0].name, "Regular");
        assert_eq!(f.masters[1].name, "Bold");
        assert_eq!(f.masters[1].axes, vec![700.0, 100.0]);
        assert!(
            f.masters[1]
                .metrics
                .contains(&("Cap height".to_string(), 700.0))
        );
        assert!(
            f.info
                .contains(&("Designer".to_string(), "Jane Doe".to_string()))
        );
        assert!(
            f.info
                .contains(&("Parameter Use Typo Metrics".to_string(), "1".to_string()))
        );
        assert_eq!(
            f.features
                .iter()
                .map(|(l, _)| l.as_str())
                .collect::<Vec<_>>(),
            vec!["Prefix Languagesystems", "@Upper", "liga"]
        );
        assert_eq!(f.kerning.len(), 2);
        assert_eq!(f.kerning[0], ("M1".into(), "A".into(), "V".into(), -50.0));
        assert_eq!(f.instances[1].0, "Bold");
        assert!(
            f.instances[1]
                .1
                .contains(&("isBold".to_string(), "1".to_string()))
        );

        let a = &f.glyphs[0];
        assert_eq!(a.unicodes, vec![0x41]);
        let (key, regular) = &a.layers[0];
        assert_eq!(key, "M1");
        assert_eq!(regular.advance, 400.0);
        // the start node is stored last
        assert_eq!(
            regular.path.cmds,
            vec![
                PathCmd::MoveTo(300.0, 0.0),
                PathCmd::LineTo(100.0, 0.0),
                PathCmd::LineTo(200.0, 700.0),
                PathCmd::Close
            ]
        );
        let bold = &a.layers[1].1;
        assert_eq!(bold.anchors[0].x, 250.0);
        assert_eq!(bold.path.cmds[0], PathCmd::MoveTo(310.0, 0.0));
        assert!(matches!(bold.path.cmds[3], PathCmd::CubicTo(..)));

        let aacute = &f.glyphs[1];
        assert!(!aacute.export);
        assert_eq!(aacute.unicodes, vec![0xC1, 0x1FA]);
        let l1 = &aacute.layers[0].1;
        assert_eq!(l1.unresolved, vec!["acutecomb"]);
        assert_eq!(l1.path, regular.path);
        let l2 = &aacute.layers[1].1;
        assert_eq!(
            l2.components[0].transform,
            [-1.0, 0.0, 0.0, 1.0, 450.0, 0.0]
        );
        assert_eq!(l2.path.cmds[0], PathCmd::MoveTo(140.0, 0.0));
    }

    #[test]
    fn reads_a_format_3_file() {
        let f = parse_glyphs(G3.as_bytes()).unwrap();
        assert_eq!(f.axes, vec!["Weight (wght)"]);
        assert_eq!(f.masters[1].name, "Bold");
        assert_eq!(f.masters[1].axes, vec![700.0]);
        assert_eq!(
            f.masters[0].metrics,
            vec![
                ("Ascender".to_string(), 750.0),
                ("Cap height".to_string(), 700.0),
                ("Baseline".to_string(), 0.0)
            ]
        );
        assert!(
            f.info
                .contains(&("Designer".to_string(), "Jane Doe".to_string()))
        );
        assert_eq!(f.features[1].0, "kern");
        assert_eq!(
            f.kerning,
            vec![("m01".into(), "A".into(), "V".into(), -50.0)]
        );
        assert_eq!(
            f.instances[0].1,
            vec![("Weight".to_string(), "400".to_string())]
        );

        let a = &f.glyphs[0];
        assert_eq!(a.unicodes, vec![65]);
        assert_eq!(a.layers.len(), 3);
        assert_eq!(a.layers[1].1.advance, 600.0, "width defaults to 600");
        assert_eq!(a.layers[2].0, "Mar 1 21, 10:00");

        let aacute = &f.glyphs[1];
        assert_eq!(aacute.unicodes, vec![193, 506]);
        assert_eq!(aacute.layers[0].1.unresolved, vec!["acutecomb"]);
        assert_eq!(aacute.layers[0].1.path, a.layers[0].1.path);
        // scale (2, 1), rotate 90°, then move by (450, 0): (x, y) → (450 - y, 2x)
        let t = aacute.layers[1].1.components[0].transform;
        let expect = [0.0, 2.0, -1.0, 0.0, 450.0, 0.0];
        for (a, b) in t.iter().zip(expect) {
            assert!((a - b).abs() < 1e-5, "{t:?}");
        }
        match aacute.layers[1].1.path.cmds[0] {
            PathCmd::MoveTo(x, y) => {
                assert!((x - 450.0).abs() < 1e-3 && (y - 620.0).abs() < 1e-3);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn reads_a_package_glyph_file() {
        let glyph = "{\nglyphname = B;\nlayers = (\n{\nlayerId = m01;\nshapes = (\n{\nref = A;\n}\n);\nwidth = 500;\n}\n);\nunicode = 66;\n}";
        let f = parse_glyphs(glyph.as_bytes()).unwrap();
        assert_eq!(f.glyphs.len(), 1);
        assert_eq!(f.glyphs[0].unicodes, vec![66]);
        assert_eq!(f.glyphs[0].layers[0].1.unresolved, vec!["A"]);
    }

    #[test]
    fn diffs_two_revisions() {
        let old = parse_glyphs(G3.as_bytes()).unwrap();
        let text = G3
            .replace("(200,700,l)", "(200,710,l)")
            .replace("V = -50;", "V = -55;\nW = -10;")
            .replace("tag = kern;", "tag = kern;\ndisabled = 1;")
            .replace("name = Bold;", "name = Black;");
        let new = parse_glyphs(text.as_bytes()).unwrap();
        let d = diff_glyphs(Some(&old), Some(&new));

        let a = &d.glyphs[0];
        assert_eq!(a.status, Status::Modified);
        assert_eq!(a.layers[0].1.status, Status::Modified);
        assert_eq!(a.layers[1].1.status, Status::Unchanged);
        // Aacute draws A in the first master
        assert_eq!(d.glyphs[1].status, Status::Modified);

        assert_eq!(
            d.kerning,
            vec![
                (
                    "Regular".into(),
                    "A".into(),
                    "V".into(),
                    Some(-50.0),
                    Some(-55.0)
                ),
                ("Regular".into(), "A".into(), "W".into(), None, Some(-10.0)),
            ]
        );
        assert_eq!(
            d.features,
            vec![
                ("liga".to_string(), Status::Unchanged),
                ("kern".to_string(), Status::Modified)
            ]
        );
        assert_eq!(d.masters[1].0, "Black");
        assert_eq!(d.masters[1].1, Status::Modified);

        let added = diff_glyphs(None, Some(&new));
        assert!(added.glyphs.iter().all(|g| g.status == Status::Added));
        let removed = diff_glyphs(Some(&old), None);
        assert!(removed.glyphs.iter().all(|g| g.status == Status::Removed));
        assert!(
            diff_glyphs(Some(&old), Some(&old))
                .glyphs
                .iter()
                .all(|g| g.status == Status::Unchanged)
        );
    }

    #[test]
    fn malformed_input_is_an_error() {
        assert!(parse_glyphs(b"(1, 2)").is_err());
        assert!(parse_glyphs(b"{ glyphs = ( { glyphname = A; ").is_err());
        // junk inside a well-formed file is skipped, not fatal
        let f = parse_glyphs(
            b"{ glyphs = ( { glyphname = A; layers = ( { layerId = x; paths = ( { nodes = (\"a b LINE\", (1)); } ); width = wide; } ); } ); }",
        )
        .unwrap();
        assert!(f.glyphs[0].layers[0].1.path.is_empty());
    }
}
