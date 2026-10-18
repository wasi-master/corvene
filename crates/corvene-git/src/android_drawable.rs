//! Corvene `1362-android-drawable-diff`: Android vector and shape drawables
//! (`res/drawable*/*.xml`, `res/mipmap*/*.xml`) as SVG, so the SVG image
//! diff draws them. GHD (`app/src/ui/diff/index.tsx`) only shows their XML.
//!
//! `<vector>` (and the vector inlined in an `<animated-vector>`) keeps its
//! groups, transforms, clip paths, gradients and tint; `<shape>` draws its
//! rectangle, oval, line or ring with its fill, gradient, stroke and
//! corners. `@color/` and `@dimen/` references are looked up in the
//! module's `values/colors.xml` and `values/dimens.xml` at the same
//! revision; a theme attribute (`?attr/…`) or another unknown color draws
//! gray, an unknown tint is left out. Other drawables (`<selector>`,
//! `<layer-list>`, …) keep the text diff. Trimmed paths draw untrimmed.

use std::collections::HashMap;
use std::fmt::Write as _;

use corvene_models::{Diff, FileStatusKind};
use roxmltree::Node;

/// Drawables over this size keep the text diff.
const MAX_BYTES: usize = 1 << 20;

const ANDROID_NS: &str = "http://schemas.android.com/apk/res/android";

/// The canvas of a shape without a `<size>`.
const DEFAULT_SHAPE_SIZE: f32 = 96.;

/// Drawn for a color that cannot be resolved (a theme attribute, a color
/// state list, a resource defined elsewhere).
const UNKNOWN_COLOR: Rgba = Rgba {
    r: 0x88,
    g: 0x88,
    b: 0x88,
    a: 1.,
};

/// Whether `path` is an XML file in a `drawable` or `mipmap` folder.
pub fn is_android_drawable(path: &str) -> bool {
    let mut parts = path.rsplit('/');
    let xml = parts
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("xml"));
    let folder = |dir: &str, kind: &str| {
        dir.strip_prefix(kind)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('-'))
    };
    xml && parts
        .next()
        .is_some_and(|dir| folder(dir, "drawable") || folder(dir, "mipmap"))
}

/// The drawable before and after as SVG images; `None` when a side is not a
/// vector or shape drawable (the caller shows the text diff then).
pub fn drawable_image_diff(
    path: &str,
    previous_path: &str,
    kind: FileStatusKind,
    read_current: impl Fn(&str) -> Option<Vec<u8>>,
    read_previous: impl Fn(&str) -> Option<Vec<u8>>,
) -> Option<Diff> {
    let current = if kind != FileStatusKind::Deleted {
        side(path, &read_current)?
    } else {
        None
    };
    let previous = if !kind.is_new_or_untracked() {
        side(previous_path, &read_previous)?
    } else {
        None
    };
    if current.is_none() && previous.is_none() {
        return None;
    }
    Some(crate::diff::image_diff_as(
        crate::diff::SVG_MEDIA_TYPE,
        kind,
        || current.map(String::into_bytes),
        || previous.map(String::into_bytes),
    ))
}

/// One side: `Some(None)` when the file is missing there, `None` when it
/// cannot be drawn.
fn side(path: &str, read: &impl Fn(&str) -> Option<Vec<u8>>) -> Option<Option<String>> {
    let Some(bytes) = read(path) else {
        return Some(None);
    };
    if bytes.len() > MAX_BYTES {
        return None;
    }
    let text = std::str::from_utf8(&bytes).ok()?;
    let res = Resources::of(path, text, read);
    to_svg(text, &res).map(Some)
}

/// Colors and dimensions of a module's `values` folder.
#[derive(Default, Debug)]
pub struct Resources {
    colors: HashMap<String, String>,
    dimens: HashMap<String, String>,
}

impl Resources {
    /// The `values/colors.xml` and `values/dimens.xml` next to the
    /// drawable's folder.
    fn of(path: &str, text: &str, read: &impl Fn(&str) -> Option<Vec<u8>>) -> Self {
        let mut res = Self::default();
        let mut parts = path.rsplitn(3, '/');
        let (_, _, root) = (parts.next(), parts.next(), parts.next());
        // only the files the drawable refers to (a missing one costs a
        // failed git read)
        let wanted = [("@color/", "colors.xml"), ("@dimen/", "dimens.xml")];
        for (_, name) in wanted.iter().filter(|(r, _)| text.contains(r)) {
            let values = match root {
                Some(root) => format!("{root}/values/{name}"),
                None => format!("values/{name}"),
            };
            if let Some(text) = read(&values).and_then(|b| String::from_utf8(b).ok()) {
                res.add(&text);
            }
        }
        res
    }

    /// Reads a `<resources>` file's colors and dimensions.
    pub fn add(&mut self, xml: &str) {
        let Ok(doc) = roxmltree::Document::parse(xml) else {
            return;
        };
        for node in doc.root_element().children().filter(Node::is_element) {
            let Some(name) = node.attribute("name") else {
                continue;
            };
            let kind = match node.tag_name().name() {
                "item" => node.attribute("type").unwrap_or_default(),
                tag => tag,
            };
            let value = node.text().unwrap_or_default().trim().to_string();
            match kind {
                "color" => self.colors.insert(name.to_string(), value),
                "dimen" => self.dimens.insert(name.to_string(), value),
                _ => None,
            };
        }
    }
}

/// `xml` as an SVG document, or `None` when it is not a drawable this
/// draws.
pub fn to_svg(xml: &str, res: &Resources) -> Option<String> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let root = doc.root_element();
    match root.tag_name().name() {
        "vector" => vector(root, res),
        "shape" => shape(root, res),
        // the vector inlined with `aapt:attr name="android:drawable"`
        "animated-vector" => root
            .children()
            .filter(|n| n.tag_name().name() == "attr")
            .filter(|n| n.attribute("name") == Some("android:drawable"))
            .flat_map(|n| n.children())
            .find(|n| n.tag_name().name() == "vector")
            .and_then(|v| vector(v, res)),
        _ => None,
    }
}

/// The SVG being written: definitions (gradients, clip paths, the tint
/// filter) and the drawing.
#[derive(Default)]
struct Svg {
    defs: String,
    body: String,
    ids: usize,
}

impl Svg {
    fn id(&mut self, kind: &str) -> String {
        self.ids += 1;
        format!("{kind}{}", self.ids)
    }

    /// The document of size `w`×`h` drawing `viewport`, the drawing
    /// tinted (`src_in`) and faded by `alpha`.
    fn finish(
        mut self,
        (w, h): (f32, f32),
        viewport: (f32, f32),
        tint: Option<Rgba>,
        alpha: f32,
    ) -> String {
        let mut wrapper = String::new();
        if let Some(tint) = tint {
            let id = self.id("tint");
            let _ = write!(
                self.defs,
                r#"<filter id="{id}"><feFlood flood-color="{}" flood-opacity="{}"/><feComposite in2="SourceGraphic" operator="in"/></filter>"#,
                tint.hex(),
                tint.a
            );
            let _ = write!(wrapper, r#" filter="url(#{id})""#);
        }
        if alpha < 1. {
            let _ = write!(wrapper, r#" opacity="{}""#, alpha.max(0.));
        }
        let mut out = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {} {}">"#,
            viewport.0, viewport.1
        );
        if !self.defs.is_empty() {
            let _ = write!(out, "<defs>{}</defs>", self.defs);
        }
        let _ = write!(out, "<g{wrapper}>{}</g></svg>", self.body);
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rgba {
    r: u8,
    g: u8,
    b: u8,
    a: f32,
}

impl Rgba {
    fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

/// An `android:` attribute (or one without a namespace).
fn attr<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|a| a.name() == name && matches!(a.namespace(), Some(ANDROID_NS) | None))
        .map(|a| a.value())
}

fn float(node: Node, name: &str, res: &Resources) -> Option<f32> {
    dimension(attr(node, name)?, res)
}

/// `12dp`, `3.5`, `@dimen/margin` in dp (a `px` is taken as a dp).
fn dimension(value: &str, res: &Resources) -> Option<f32> {
    let value = value.trim();
    if let Some(name) = value.strip_prefix("@dimen/") {
        let next = res.dimens.get(name)?;
        return (next != value).then(|| dimension(next, res)).flatten();
    }
    let split = value
        .find(|c: char| c.is_ascii_alphabetic() || c == '%')
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    let number: f32 = number.trim().parse().ok()?;
    let scale = match unit {
        "" | "dp" | "dip" | "px" | "sp" => 1.,
        "pt" => 160. / 72.,
        "in" => 160.,
        "mm" => 160. / 25.4,
        _ => return None,
    };
    Some(number * scale).filter(|n| n.is_finite())
}

/// `#RGB`, `#ARGB`, `#RRGGBB`, `#AARRGGBB`, a framework color or an
/// `@color/` of the module. `None` for what cannot be resolved.
fn color(value: &str, res: &Resources) -> Option<Rgba> {
    color_at(value.trim(), res, 0)
}

fn color_at(value: &str, res: &Resources, depth: u8) -> Option<Rgba> {
    if let Some(hex) = value.strip_prefix('#') {
        return hex_color(hex);
    }
    if let Some(name) = value.strip_prefix("@android:color/") {
        return hex_color(match name {
            "black" => "000000",
            "white" => "ffffff",
            "transparent" => "00000000",
            "darker_gray" => "aaaaaa",
            "background_dark" => "000000",
            "background_light" => "ffffff",
            _ => return None,
        });
    }
    let name = value.strip_prefix("@color/")?;
    if depth > 8 {
        return None;
    }
    color_at(res.colors.get(name)?.trim(), res, depth + 1)
}

fn hex_color(hex: &str) -> Option<Rgba> {
    let digit = |i: usize| u8::from_str_radix(hex.get(i..i + 1)?, 16).ok();
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    let (a, r, g, b) = match hex.len() {
        3 => (15 * 17, digit(0)? * 17, digit(1)? * 17, digit(2)? * 17),
        4 => (
            digit(0)? * 17,
            digit(1)? * 17,
            digit(2)? * 17,
            digit(3)? * 17,
        ),
        6 => (255, byte(0)?, byte(2)?, byte(4)?),
        8 => (byte(0)?, byte(2)?, byte(4)?, byte(6)?),
        _ => return None,
    };
    Some(Rgba {
        r,
        g,
        b,
        a: f32::from(a) / 255.,
    })
}

/// An attribute value made safe inside `"…"`.
fn esc(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('"', "&quot;")
}

fn vector(node: Node, res: &Resources) -> Option<String> {
    let vw = float(node, "viewportWidth", res).filter(|v| *v > 0.)?;
    let vh = float(node, "viewportHeight", res).filter(|v| *v > 0.)?;
    let w = float(node, "width", res).filter(|v| *v > 0.).unwrap_or(vw);
    let h = float(node, "height", res).filter(|v| *v > 0.).unwrap_or(vh);
    // `tintMode` other than `src_in` / `src_atop` (multiply, screen, add)
    // draws untinted
    let tinted = matches!(attr(node, "tintMode"), None | Some("src_in" | "src_atop"));
    let tint = attr(node, "tint")
        .and_then(|t| color(t, res))
        .filter(|_| tinted);
    let alpha = float(node, "alpha", res).unwrap_or(1.);
    let mut svg = Svg::default();
    vector_children(node, res, &mut svg, 0);
    Some(svg.finish((w, h), (vw, vh), tint, alpha))
}

/// A vector's or group's paths, groups and clip paths, in order (a clip
/// path clips what follows it in its group).
fn vector_children(node: Node, res: &Resources, svg: &mut Svg, depth: usize) {
    if depth > 64 {
        return;
    }
    let mut clips = 0;
    for child in node.children().filter(Node::is_element) {
        match child.tag_name().name() {
            "group" => {
                let _ = write!(svg.body, "<g{}>", group_transform(child, res));
                vector_children(child, res, svg, depth + 1);
                svg.body.push_str("</g>");
            }
            "path" => vector_path(child, res, svg),
            "clip-path" => {
                let Some(d) = attr(child, "pathData") else {
                    continue;
                };
                let id = svg.id("clip");
                let _ = write!(
                    svg.defs,
                    r#"<clipPath id="{id}"><path d="{}"/></clipPath>"#,
                    esc(d)
                );
                let _ = write!(svg.body, r#"<g clip-path="url(#{id})">"#);
                clips += 1;
            }
            _ => {}
        }
    }
    for _ in 0..clips {
        svg.body.push_str("</g>");
    }
}

/// Android's group matrix: scale and rotate about the pivot, then
/// translate.
fn group_transform(node: Node, res: &Resources) -> String {
    let get = |name, default| float(node, name, res).unwrap_or(default);
    let (px, py) = (get("pivotX", 0.), get("pivotY", 0.));
    let (sx, sy) = (get("scaleX", 1.), get("scaleY", 1.));
    let (tx, ty) = (get("translateX", 0.), get("translateY", 0.));
    let rotation = get("rotation", 0.);
    if (px, py, sx, sy, tx, ty, rotation) == (0., 0., 1., 1., 0., 0., 0.) {
        return String::new();
    }
    format!(
        r#" transform="translate({} {}) rotate({rotation}) scale({sx} {sy}) translate({} {})""#,
        px + tx,
        py + ty,
        -px,
        -py
    )
}

fn vector_path(node: Node, res: &Resources, svg: &mut Svg) {
    let Some(d) = attr(node, "pathData") else {
        return;
    };
    let mut out = format!(r#"<path d="{}""#, esc(d));
    let fill_alpha = float(node, "fillAlpha", res).unwrap_or(1.);
    let fill = paint(node, "fillColor", res, svg, None);
    write_paint(&mut out, "fill", fill, fill_alpha);
    if attr(node, "fillType") == Some("evenOdd") {
        out.push_str(r#" fill-rule="evenodd""#);
    }
    let stroke_width = float(node, "strokeWidth", res).unwrap_or(0.);
    let stroke = paint(node, "strokeColor", res, svg, None).filter(|_| stroke_width > 0.);
    if stroke.is_some() {
        let stroke_alpha = float(node, "strokeAlpha", res).unwrap_or(1.);
        write_paint(&mut out, "stroke", stroke, stroke_alpha);
        let _ = write!(out, r#" stroke-width="{stroke_width}""#);
        if let Some(cap @ ("round" | "square")) = attr(node, "strokeLineCap") {
            let _ = write!(out, r#" stroke-linecap="{cap}""#);
        }
        if let Some(join @ ("round" | "bevel")) = attr(node, "strokeLineJoin") {
            let _ = write!(out, r#" stroke-linejoin="{join}""#);
        }
        if let Some(limit) = float(node, "strokeMiterLimit", res) {
            let _ = write!(out, r#" stroke-miterlimit="{limit}""#);
        }
    }
    out.push_str("/>");
    svg.body.push_str(&out);
}

/// How a path or shape is filled or stroked.
enum Paint {
    Color(Rgba),
    /// A gradient in the definitions.
    Url(String),
}

fn write_paint(out: &mut String, what: &str, paint: Option<Paint>, alpha: f32) {
    match paint {
        None => {
            let _ = write!(out, r#" {what}="none""#);
        }
        Some(Paint::Color(c)) => {
            let _ = write!(out, r#" {what}="{}""#, c.hex());
            let opacity = c.a * alpha;
            if opacity < 1. {
                let _ = write!(out, r#" {what}-opacity="{}""#, opacity.max(0.));
            }
        }
        Some(Paint::Url(id)) => {
            let _ = write!(out, r#" {what}="url(#{id})""#);
            if alpha < 1. {
                let _ = write!(out, r#" {what}-opacity="{}""#, alpha.max(0.));
            }
        }
    }
}

/// A vector path's `fillColor` / `strokeColor`: a color attribute or an
/// inline `<aapt:attr>` gradient. `bounds` places a shape's gradient.
fn paint(
    node: Node,
    name: &str,
    res: &Resources,
    svg: &mut Svg,
    bounds: Option<(f32, f32)>,
) -> Option<Paint> {
    if let Some(value) = attr(node, name) {
        let c = color(value, res).unwrap_or(UNKNOWN_COLOR);
        return (c.a > 0.).then_some(Paint::Color(c));
    }
    let full = format!("android:{name}");
    let gradient = node
        .children()
        .filter(|n| n.tag_name().name() == "attr" && n.attribute("name") == Some(full.as_str()))
        .flat_map(|n| n.children())
        .find(|n| n.tag_name().name() == "gradient")?;
    gradient_paint(gradient, res, svg, bounds)
}

/// A `<gradient>`: vector gradients are in path coordinates; a shape's
/// (`bounds` = its size) use `angle` and fractions of its size.
fn gradient_paint(
    node: Node,
    res: &Resources,
    svg: &mut Svg,
    bounds: Option<(f32, f32)>,
) -> Option<Paint> {
    let stops = gradient_stops(node, res);
    let first = stops.first()?.1;
    let kind = attr(node, "type").unwrap_or("linear");
    let spread = match attr(node, "tileMode") {
        Some("repeat") => r#" spreadMethod="repeat""#,
        Some("mirror") => r#" spreadMethod="reflect""#,
        _ => "",
    };
    let get = |name, default| float(node, name, res).unwrap_or(default);
    let id = svg.id("gradient");
    let open = match (kind, bounds) {
        ("linear", None) => format!(
            r#"<linearGradient id="{id}" gradientUnits="userSpaceOnUse" x1="{}" y1="{}" x2="{}" y2="{}"{spread}>"#,
            get("startX", 0.),
            get("startY", 0.),
            get("endX", 0.),
            get("endY", 0.)
        ),
        ("linear", Some((w, h))) => {
            // GradientDrawable: the angle snaps to 45° steps, 0 is left to
            // right, 90 bottom to top
            let angle = (get("angle", 0.).rem_euclid(360.) / 45.).round() as i32 % 8;
            let (x1, y1, x2, y2) = match angle {
                1 => (0., h, w, 0.),
                2 => (0., h, 0., 0.),
                3 => (w, h, 0., 0.),
                4 => (w, 0., 0., 0.),
                5 => (w, 0., 0., h),
                6 => (0., 0., 0., h),
                7 => (0., 0., w, h),
                _ => (0., 0., w, 0.),
            };
            format!(
                r#"<linearGradient id="{id}" gradientUnits="userSpaceOnUse" x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}"{spread}>"#
            )
        }
        ("radial", None) => format!(
            r#"<radialGradient id="{id}" gradientUnits="userSpaceOnUse" cx="{}" cy="{}" r="{}"{spread}>"#,
            get("centerX", 0.),
            get("centerY", 0.),
            get("gradientRadius", 0.).max(0.)
        ),
        ("radial", Some((w, h))) => {
            let radius = attr(node, "gradientRadius")
                .and_then(|r| match r.trim().trim_end_matches('p').strip_suffix('%') {
                    Some(pct) => pct.trim().parse::<f32>().ok().map(|p| p / 100. * w.min(h)),
                    None => dimension(r, res),
                })
                .unwrap_or(w.min(h) / 2.);
            format!(
                r#"<radialGradient id="{id}" gradientUnits="userSpaceOnUse" cx="{}" cy="{}" r="{}"{spread}>"#,
                get("centerX", 0.5) * w,
                get("centerY", 0.5) * h,
                radius.max(0.)
            )
        }
        // SVG has no sweep gradient: its first color
        _ => return Some(Paint::Color(first)),
    };
    svg.defs.push_str(&open);
    for (offset, c) in &stops {
        let _ = write!(
            svg.defs,
            r#"<stop offset="{}" stop-color="{}" stop-opacity="{}"/>"#,
            offset.clamp(0., 1.),
            c.hex(),
            c.a
        );
    }
    let close = if open.starts_with("<linear") {
        "</linearGradient>"
    } else {
        "</radialGradient>"
    };
    svg.defs.push_str(close);
    Some(Paint::Url(id))
}

/// `<item android:offset android:color>` children, else the start, center
/// and end colors.
fn gradient_stops(node: Node, res: &Resources) -> Vec<(f32, Rgba)> {
    let items: Vec<_> = node
        .children()
        .filter(|n| n.tag_name().name() == "item")
        .filter_map(|n| {
            let offset = float(n, "offset", res)?;
            let c = color(attr(n, "color")?, res).unwrap_or(UNKNOWN_COLOR);
            Some((offset, c))
        })
        .collect();
    if !items.is_empty() {
        return items;
    }
    let get = |name| attr(node, name).map(|v| color(v, res).unwrap_or(UNKNOWN_COLOR));
    [
        (0., get("startColor")),
        (0.5, get("centerColor")),
        (1., get("endColor")),
    ]
    .into_iter()
    .filter_map(|(offset, c)| Some((offset, c?)))
    .collect()
}

fn shape(node: Node, res: &Resources) -> Option<String> {
    let kind = attr(node, "shape").unwrap_or("rectangle");
    let child = |name: &str| node.children().find(|n| n.tag_name().name() == name);
    let size = child("size");
    let sized = |name| size.and_then(|s| float(s, name, res)).filter(|v| *v > 0.);
    let mut svg = Svg::default();
    let stroke = child("stroke");
    let stroke_width = stroke
        .and_then(|s| float(s, "width", res))
        .unwrap_or(0.)
        .max(0.);
    let (w, h) = if kind == "ring" {
        let w = sized("width");
        let inner = float(node, "innerRadius", res).unwrap_or_else(|| {
            w.unwrap_or(DEFAULT_SHAPE_SIZE) / float(node, "innerRadiusRatio", res).unwrap_or(3.)
        });
        let thickness = float(node, "thickness", res).unwrap_or_else(|| {
            w.unwrap_or(DEFAULT_SHAPE_SIZE) / float(node, "thicknessRatio", res).unwrap_or(9.)
        });
        let side = 2. * (inner + thickness);
        let (w, h) = (w.unwrap_or(side), sized("height").unwrap_or(side));
        let (cx, cy) = (w / 2., h / 2.);
        let circle = |r: f32| {
            format!(
                "M{} {cy}a{r} {r} 0 1 0 {} 0a{r} {r} 0 1 0 {} 0Z",
                cx - r,
                2. * r,
                -2. * r
            )
        };
        svg.body = format!(
            r#"<path fill-rule="evenodd" d="{}{}""#,
            circle(inner + thickness),
            circle(inner)
        );
        (w, h)
    } else {
        let (w, h) = (
            sized("width").unwrap_or(DEFAULT_SHAPE_SIZE),
            sized("height").unwrap_or(DEFAULT_SHAPE_SIZE),
        );
        // the stroke is drawn inside the bounds
        let inset = stroke_width / 2.;
        let (iw, ih) = ((w - stroke_width).max(0.), (h - stroke_width).max(0.));
        svg.body = match kind {
            "oval" => format!(
                r#"<ellipse cx="{}" cy="{}" rx="{}" ry="{}""#,
                w / 2.,
                h / 2.,
                iw / 2.,
                ih / 2.
            ),
            "line" => format!(r#"<line x1="0" y1="{0}" x2="{w}" y2="{0}""#, h / 2.),
            _ => rectangle(inset, iw, ih, child("corners"), res),
        };
        (w, h)
    };
    // a line only has its stroke
    let fill = if kind == "line" {
        None
    } else if let Some(gradient) = child("gradient") {
        gradient_paint(gradient, res, &mut svg, Some((w, h)))
    } else {
        child("solid")
            .and_then(|s| attr(s, "color"))
            .map(|c| color(c, res).unwrap_or(UNKNOWN_COLOR))
            .filter(|c| c.a > 0.)
            .map(Paint::Color)
    };
    let mut body = std::mem::take(&mut svg.body);
    write_paint(&mut body, "fill", fill, 1.);
    if let Some(stroke) = stroke.filter(|_| stroke_width > 0.) {
        let c = attr(stroke, "color").map(|c| color(c, res).unwrap_or(UNKNOWN_COLOR));
        write_paint(
            &mut body,
            "stroke",
            c.filter(|c| c.a > 0.).map(Paint::Color),
            1.,
        );
        let _ = write!(body, r#" stroke-width="{stroke_width}""#);
        let dash = float(stroke, "dashWidth", res).unwrap_or(0.);
        if dash > 0. {
            let gap = float(stroke, "dashGap", res).unwrap_or(0.).max(0.);
            let _ = write!(body, r#" stroke-dasharray="{dash} {gap}""#);
        }
    }
    body.push_str("/>");
    svg.body = body;
    let tint = attr(node, "tint").and_then(|t| color(t, res));
    Some(svg.finish((w, h), (w, h), tint, 1.))
}

/// A rectangle at `inset` of `w`×`h` with its `<corners>` (one radius, or
/// one per corner).
fn rectangle(inset: f32, w: f32, h: f32, corners: Option<Node>, res: &Resources) -> String {
    let max = w.min(h) / 2.;
    let radius = |name| {
        corners
            .and_then(|c| float(c, name, res))
            .unwrap_or(0.)
            .clamp(0., max)
    };
    let all = radius("radius");
    let corner = |name| match corners.and_then(|c| attr(c, name)) {
        Some(_) => radius(name),
        None => all,
    };
    let (tl, tr, br, bl) = (
        corner("topLeftRadius"),
        corner("topRightRadius"),
        corner("bottomRightRadius"),
        corner("bottomLeftRadius"),
    );
    if tl == tr && tr == br && br == bl {
        let rounded = if tl > 0. {
            format!(r#" rx="{tl}" ry="{tl}""#)
        } else {
            String::new()
        };
        return format!(r#"<rect x="{inset}" y="{inset}" width="{w}" height="{h}"{rounded}"#);
    }
    let (x, y) = (inset, inset);
    let mut d = format!("M{} {y}", x + tl);
    let _ = write!(d, "H{}", x + w - tr);
    let _ = write!(d, "A{tr} {tr} 0 0 1 {} {}", x + w, y + tr);
    let _ = write!(d, "V{}", y + h - br);
    let _ = write!(d, "A{br} {br} 0 0 1 {} {}", x + w - br, y + h);
    let _ = write!(d, "H{}", x + bl);
    let _ = write!(d, "A{bl} {bl} 0 0 1 {x} {}", y + h - bl);
    let _ = write!(d, "V{}", y + tl);
    let _ = write!(d, "A{tl} {tl} 0 0 1 {} {y}Z", x + tl);
    format!(r#"<path d="{d}""#)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svg(xml: &str) -> String {
        to_svg(xml, &Resources::default()).expect("drawable")
    }

    fn parses(svg: &str) {
        roxmltree::Document::parse(svg).expect("well-formed svg");
    }

    #[test]
    fn drawable_paths() {
        assert!(is_android_drawable("app/src/main/res/drawable/ic_add.xml"));
        assert!(is_android_drawable(
            "res/drawable-v24/ic_launcher_foreground.xml"
        ));
        assert!(is_android_drawable("res/mipmap-anydpi-v26/ic_launcher.XML"));
        assert!(is_android_drawable("drawable/a.xml"));
        assert!(!is_android_drawable("res/layout/main.xml"));
        assert!(!is_android_drawable("res/drawables/a.xml"));
        assert!(!is_android_drawable("res/drawable/a.png"));
        assert!(!is_android_drawable("a.xml"));
    }

    #[test]
    fn vector_with_group_clip_and_tint() {
        let out = svg(
            r##"<vector xmlns:android="http://schemas.android.com/apk/res/android"
            android:width="24dp" android:height="24dp"
            android:viewportWidth="48" android:viewportHeight="48"
            android:tint="#80FF0000" android:alpha="0.5">
            <group android:rotation="90" android:pivotX="24" android:pivotY="24">
              <clip-path android:pathData="M0,0h24v24h-24z"/>
              <path android:fillColor="#F00" android:pathData="M12,4L4,20h16z"
                    android:strokeColor="#ff00ff00" android:strokeWidth="2"
                    android:strokeLineCap="round" android:fillType="evenOdd"/>
            </group>
            <path android:fillColor="@android:color/white" android:fillAlpha="0.25" android:pathData="M0 0h1"/>
          </vector>"##,
        );
        parses(&out);
        assert!(out.contains(r##"width="24" height="24" viewBox="0 0 48 48""##));
        assert!(out.contains(
            r##"transform="translate(24 24) rotate(90) scale(1 1) translate(-24 -24)""##
        ));
        assert!(out.contains(r##"<clipPath id="clip1"><path d="M0,0h24v24h-24z"/></clipPath>"##));
        assert!(out.contains(r##"fill="#ff0000""##));
        assert!(out.contains(r##"stroke="#00ff00""##));
        assert!(out.contains(r##"stroke-linecap="round""##));
        assert!(out.contains(r##"fill-rule="evenodd""##));
        assert!(out.contains(r##"fill="#ffffff" fill-opacity="0.25""##));
        assert!(out.contains(r##"flood-color="#ff0000""##));
        assert!(out.contains(r##"opacity="0.5""##));
    }

    #[test]
    fn vector_gradient_from_aapt_attr() {
        let out = svg(
            r##"<vector xmlns:android="http://schemas.android.com/apk/res/android"
            xmlns:aapt="http://schemas.android.com/aapt"
            android:width="10dp" android:height="10dp"
            android:viewportWidth="10" android:viewportHeight="10">
            <path android:pathData="M0 0h10v10h-10z">
              <aapt:attr name="android:fillColor">
                <gradient android:type="linear" android:startX="0" android:startY="0"
                  android:endX="10" android:endY="0" android:tileMode="mirror">
                  <item android:offset="0" android:color="#FF000000"/>
                  <item android:offset="1" android:color="#00FFFFFF"/>
                </gradient>
              </aapt:attr>
            </path>
          </vector>"##,
        );
        parses(&out);
        assert!(out.contains(r##"<linearGradient id="gradient1" gradientUnits="userSpaceOnUse" x1="0" y1="0" x2="10" y2="0" spreadMethod="reflect">"##));
        assert!(out.contains(r##"<stop offset="1" stop-color="#ffffff" stop-opacity="0"/>"##));
        assert!(out.contains(r##"fill="url(#gradient1)""##));
    }

    #[test]
    fn colors_and_dimens_from_values() {
        let mut res = Resources::default();
        res.add(
            r##"<resources><color name="brand">@color/base</color><color name="base">#123456</color>
            <dimen name="side">32dp</dimen><item name="r" type="dimen">4dp</item></resources>"##,
        );
        let out = to_svg(
            r##"<shape xmlns:android="http://schemas.android.com/apk/res/android">
                <solid android:color="@color/brand"/>
                <corners android:radius="@dimen/r"/>
                <size android:width="@dimen/side" android:height="@dimen/side"/>
              </shape>"##,
            &res,
        )
        .expect("shape");
        parses(&out);
        assert!(out.contains(r##"width="32" height="32""##));
        assert!(out.contains(
            r##"<rect x="0" y="0" width="32" height="32" rx="4" ry="4" fill="#123456"/>"##
        ));
    }

    #[test]
    fn unknown_colors_are_gray() {
        let out = svg(
            r##"<shape xmlns:android="http://schemas.android.com/apk/res/android"
            android:shape="oval"><solid android:color="?attr/colorPrimary"/></shape>"##,
        );
        assert!(out.contains(r##"<ellipse cx="48" cy="48" rx="48" ry="48" fill="#888888"/>"##));
    }

    #[test]
    fn shape_stroke_gradient_and_corners() {
        let out = svg(
            r##"<shape xmlns:android="http://schemas.android.com/apk/res/android">
            <size android:width="100dp" android:height="40dp"/>
            <gradient android:angle="90" android:startColor="#000" android:endColor="#fff"/>
            <stroke android:width="2dp" android:color="#f00" android:dashWidth="4dp" android:dashGap="2dp"/>
            <corners android:radius="8dp" android:topLeftRadius="0dp"/>
          </shape>"##,
        );
        parses(&out);
        assert!(out.contains(r##"x1="0" y1="40" x2="0" y2="0""##));
        assert!(out.contains(r##"<path d="M1 1H91A8 8 0 0 1 99 9"##));
        assert!(out.contains(r##"stroke-dasharray="4 2""##));
    }

    #[test]
    fn line_and_ring() {
        let line = svg(
            r##"<shape xmlns:android="http://schemas.android.com/apk/res/android"
            android:shape="line"><stroke android:width="1dp" android:color="#000"/></shape>"##,
        );
        assert!(
            line.contains(r##"<line x1="0" y1="48" x2="96" y2="48" fill="none" stroke="#000000""##)
        );
        let ring = svg(
            r##"<shape xmlns:android="http://schemas.android.com/apk/res/android"
            android:shape="ring" android:innerRadius="10dp" android:thickness="5dp"
            android:useLevel="false"><solid android:color="#0f0"/></shape>"##,
        );
        parses(&ring);
        assert!(ring.contains(r##"width="30" height="30""##));
        assert!(ring.contains(r##"fill-rule="evenodd""##));
    }

    #[test]
    fn animated_vector_draws_its_inline_vector() {
        let out = svg(
            r##"<animated-vector xmlns:android="http://schemas.android.com/apk/res/android"
            xmlns:aapt="http://schemas.android.com/aapt">
            <aapt:attr name="android:drawable">
              <vector android:width="8dp" android:height="8dp" android:viewportWidth="8" android:viewportHeight="8">
                <path android:fillColor="#000" android:pathData="M0 0h8v8z"/>
              </vector>
            </aapt:attr>
          </animated-vector>"##,
        );
        assert!(out.contains(r##"<path d="M0 0h8v8z" fill="#000000"/>"##));
    }

    #[test]
    fn other_drawables_keep_their_text() {
        let res = Resources::default();
        assert!(
            to_svg(
                r##"<selector xmlns:android="http://schemas.android.com/apk/res/android"/>"##,
                &res
            )
            .is_none()
        );
        assert!(to_svg("<vector/>", &res).is_none());
        assert!(to_svg("not xml", &res).is_none());
    }

    #[test]
    fn diff_reads_values_next_to_the_folder() {
        let file = br##"<shape xmlns:android="http://schemas.android.com/apk/res/android"><solid android:color="@color/c"/></shape>"##;
        let read = |p: &str| match p {
            "app/res/drawable/bg.xml" => Some(file.to_vec()),
            "app/res/values/colors.xml" => {
                Some(br##"<resources><color name="c">#abcdef</color></resources>"##.to_vec())
            }
            _ => None,
        };
        let diff = drawable_image_diff(
            "app/res/drawable/bg.xml",
            "app/res/drawable/bg.xml",
            FileStatusKind::New,
            read,
            |_| None,
        );
        let Some(Diff::Image {
            current: Some(blob),
            previous: None,
        }) = diff
        else {
            panic!("image diff expected");
        };
        assert!(
            String::from_utf8(blob.bytes)
                .expect("utf8")
                .contains("#abcdef")
        );
        // a selector on one side: the text diff
        let selector = |_: &str| Some(b"<selector/>".to_vec());
        assert!(
            drawable_image_diff(
                "res/drawable/a.xml",
                "res/drawable/a.xml",
                FileStatusKind::Modified,
                read,
                selector
            )
            .is_none()
        );
    }
}
