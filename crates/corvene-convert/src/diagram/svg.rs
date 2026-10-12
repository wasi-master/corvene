//! A small SVG writer for the diagram and map pictures: shapes, paths,
//! text and arrowheads, collected with their bounds so the picture's
//! `viewBox` fits them.

use std::fmt::Write as _;

/// The font family every picture's text asks for (the system's sans
/// serif; usvg falls back through the list).
pub const FONT: &str = "Helvetica, Arial, 'Liberation Sans', 'DejaVu Sans', sans-serif";

/// The widest a picture is drawn (pictures larger than this are scaled
/// down by the image view anyway; it bounds the raster).
pub const MAX_SIDE: f64 = 6000.0;

/// Colors for the generated diagrams (draw.io and Excalidraw carry their
/// own).
pub mod colors {
    pub const INK: &str = "#24292f";
    pub const MUTED: &str = "#57606a";
    pub const LINE: &str = "#57606a";
    pub const NODE_FILL: &str = "#ddf4ff";
    pub const NODE_STROKE: &str = "#0969da";
    pub const CLUSTER_FILL: &str = "#f6f8fa";
    pub const CLUSTER_STROKE: &str = "#d0d7de";
    pub const NOTE_FILL: &str = "#fff8c5";
    pub const NOTE_STROKE: &str = "#d4a72c";
    /// Pie slices, gantt bars: a categorical palette that reads on white.
    pub const SERIES: [&str; 10] = [
        "#0969da", "#bf3989", "#1a7f37", "#bc4c00", "#8250df", "#0598bc", "#cf222e", "#4d2d00",
        "#6e7781", "#9a6700",
    ];
}

/// XML-escapes text for an attribute or element.
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c if (c as u32) < 0x20 && c != '\n' && c != '\t' => {}
            c => out.push(c),
        }
    }
    out
}

/// A color as SVG takes it (`#rgb`, `#rrggbb`, a name), or `fallback`
/// for anything unsafe to put in an attribute.
pub fn color(value: &str, fallback: &str) -> String {
    let v = value.trim();
    if v.is_empty() {
        return fallback.to_string();
    }
    if v.eq_ignore_ascii_case("none") || v.eq_ignore_ascii_case("transparent") {
        return "none".into();
    }
    let ok = v.starts_with('#') && v.len() <= 9 && v[1..].bytes().all(|b| b.is_ascii_hexdigit())
        || v.bytes().all(|b| b.is_ascii_alphabetic()) && v.len() <= 24
        || (v.starts_with("rgb")
            && v.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"(),.% ".contains(&b)));
    if ok {
        v.to_string()
    } else {
        fallback.to_string()
    }
}

/// Estimated width of `text` at `size` px in the sans serif font.
pub fn text_width(text: &str, size: f64) -> f64 {
    let mut em = 0.0;
    for ch in text.chars() {
        em += match ch {
            'i' | 'l' | 'j' | '.' | ',' | ':' | ';' | '|' | '!' | '\'' | '`' => 0.24,
            'f' | 't' | 'r' | 'I' | '(' | ')' | '[' | ']' | '{' | '}' | ' ' | '-' | '/' | '\\' => {
                0.33
            }
            'm' | 'w' | 'M' | 'W' | '@' | '%' => 0.85,
            'A'..='Z' | '&' | '#' => 0.66,
            '0'..='9' | 'a'..='z' | '$' | '?' | '_' | '+' | '=' | '<' | '>' | '*' | '"' | '~' => {
                0.55
            }
            c if (c as u32) >= 0x2E80 => 1.0,
            _ => 0.6,
        };
    }
    em * size
}

/// A picture being drawn.
pub struct Svg {
    body: String,
    defs: String,
    markers: Vec<String>,
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
    background: Option<String>,
}

impl Default for Svg {
    fn default() -> Self {
        Self::new()
    }
}

impl Svg {
    pub fn new() -> Self {
        Svg {
            body: String::new(),
            defs: String::new(),
            markers: Vec::new(),
            min_x: f64::INFINITY,
            min_y: f64::INFINITY,
            max_x: f64::NEG_INFINITY,
            max_y: f64::NEG_INFINITY,
            background: Some("#ffffff".into()),
        }
    }

    pub fn set_background(&mut self, color: Option<&str>) {
        self.background = color.map(str::to_string);
    }

    /// Grows the bounds to hold a point.
    pub fn include(&mut self, x: f64, y: f64) {
        if x.is_finite() && y.is_finite() {
            self.min_x = self.min_x.min(x);
            self.min_y = self.min_y.min(y);
            self.max_x = self.max_x.max(x);
            self.max_y = self.max_y.max(y);
        }
    }

    pub fn include_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        self.include(x, y);
        self.include(x + w, y + h);
    }

    pub fn is_empty(&self) -> bool {
        self.body.is_empty()
    }

    /// What has been drawn: `(min x, min y, max x, max y)`, if anything.
    pub fn bounds(&self) -> Option<(f64, f64, f64, f64)> {
        self.min_x
            .is_finite()
            .then_some((self.min_x, self.min_y, self.max_x, self.max_y))
    }

    /// Raw markup (already escaped) with the bounds it covers.
    pub fn raw(&mut self, markup: &str) {
        self.body.push_str(markup);
        self.body.push('\n');
    }

    pub fn rect(&mut self, x: f64, y: f64, w: f64, h: f64, rx: f64, style: &Style) {
        self.include_rect(
            x - style.width,
            y - style.width,
            w + 2.0 * style.width,
            h + 2.0 * style.width,
        );
        let _ = writeln!(
            self.body,
            r#"<rect x="{}" y="{}" width="{}" height="{}" rx="{}" {}/>"#,
            n(x),
            n(y),
            n(w.max(0.0)),
            n(h.max(0.0)),
            n(rx),
            style.attrs()
        );
    }

    pub fn ellipse(&mut self, cx: f64, cy: f64, rx: f64, ry: f64, style: &Style) {
        self.include_rect(cx - rx, cy - ry, 2.0 * rx, 2.0 * ry);
        let _ = writeln!(
            self.body,
            r#"<ellipse cx="{}" cy="{}" rx="{}" ry="{}" {}/>"#,
            n(cx),
            n(cy),
            n(rx.max(0.0)),
            n(ry.max(0.0)),
            style.attrs()
        );
    }

    pub fn polygon(&mut self, points: &[(f64, f64)], style: &Style) {
        if points.is_empty() {
            return;
        }
        for (x, y) in points {
            self.include(*x, *y);
        }
        let pts: Vec<String> = points
            .iter()
            .map(|(x, y)| format!("{},{}", n(*x), n(*y)))
            .collect();
        let _ = writeln!(
            self.body,
            r#"<polygon points="{}" {}/>"#,
            pts.join(" "),
            style.attrs()
        );
    }

    /// A path from SVG path data (whose points the caller adds to the
    /// bounds with [`Svg::include`]).
    pub fn path(
        &mut self,
        d: &str,
        style: &Style,
        marker_end: Option<&str>,
        marker_start: Option<&str>,
    ) {
        let mut extra = String::new();
        if let Some(m) = marker_end {
            let id = self.marker(m, &style.stroke);
            let _ = write!(extra, r#" marker-end="url(#{id})""#);
        }
        if let Some(m) = marker_start {
            let id = self.marker(m, &style.stroke);
            let _ = write!(extra, r#" marker-start="url(#{id})""#);
        }
        let _ = writeln!(self.body, r#"<path d="{d}" {}{extra}/>"#, style.attrs());
    }

    /// A polyline through `points` (bounds included), smoothed when asked.
    pub fn line(
        &mut self,
        points: &[(f64, f64)],
        smooth: bool,
        style: &Style,
        marker_end: Option<&str>,
        marker_start: Option<&str>,
    ) {
        if points.len() < 2 {
            return;
        }
        for (x, y) in points {
            self.include(*x, *y);
        }
        let d = if smooth {
            smooth_path(points)
        } else {
            poly_path(points)
        };
        self.path(&d, style, marker_end, marker_start);
    }

    /// Text centered (or anchored) at a point; `\n` starts new lines.
    #[allow(clippy::too_many_arguments)]
    pub fn text(
        &mut self,
        x: f64,
        y: f64,
        text: &str,
        size: f64,
        anchor: Anchor,
        fill: &str,
        weight: Option<&str>,
    ) {
        let lines: Vec<&str> = text.split('\n').collect();
        let line_h = size * 1.2;
        let top = y - line_h * (lines.len() as f64 - 1.0) / 2.0;
        let widest = lines
            .iter()
            .map(|l| text_width(l, size))
            .fold(0.0, f64::max);
        let left = match anchor {
            Anchor::Start => x,
            Anchor::Middle => x - widest / 2.0,
            Anchor::End => x - widest,
        };
        self.include_rect(left, top - size * 0.7, widest, line_h * lines.len() as f64);
        let anchor = match anchor {
            Anchor::Start => "start",
            Anchor::Middle => "middle",
            Anchor::End => "end",
        };
        let weight = weight
            .map(|w| format!(r#" font-weight="{w}""#))
            .unwrap_or_default();
        let mut tspans = String::new();
        for (ix, line) in lines.iter().enumerate() {
            let _ = write!(
                tspans,
                r#"<tspan x="{}" y="{}">{}</tspan>"#,
                n(x),
                n(top + line_h * ix as f64 + size * 0.35),
                esc(line)
            );
        }
        let _ = writeln!(
            self.body,
            r#"<text font-family="{}" font-size="{}" text-anchor="{anchor}" fill="{}"{weight}>{tspans}</text>"#,
            esc(FONT),
            n(size),
            esc(fill)
        );
    }

    /// The id of an arrowhead marker of `kind` in `color`, defining it on
    /// first use. Kinds: `arrow`, `open`, `diamond`, `odiamond`, `circle`,
    /// `triangle` (hollow), `cross`, `bar`.
    fn marker(&mut self, kind: &str, color_value: &str) -> String {
        // named by kind and color, so pictures merged by `absorb` share them
        let id: String = format!("m-{kind}-{color_value}")
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        if !self.markers.contains(&id) {
            self.markers.push(id.clone());
            {
                let c = esc(color_value);
                let shape = match kind {
                    "open" => format!(
                        r#"<path d="M1,1 L9,5 L1,9" fill="none" stroke="{c}" stroke-width="1.5"/>"#
                    ),
                    "diamond" => format!(r#"<path d="M1,5 L5,1 L9,5 L5,9 z" fill="{c}"/>"#),
                    "odiamond" => format!(
                        r##"<path d="M1,5 L5,1 L9,5 L5,9 z" fill="#ffffff" stroke="{c}"/>"##
                    ),
                    "circle" => {
                        format!(r##"<circle cx="5" cy="5" r="3.5" fill="#ffffff" stroke="{c}"/>"##)
                    }
                    "dot" => format!(r#"<circle cx="5" cy="5" r="3.5" fill="{c}"/>"#),
                    "triangle" => {
                        format!(r##"<path d="M1,1 L9,5 L1,9 z" fill="#ffffff" stroke="{c}"/>"##)
                    }
                    "cross" => format!(
                        r#"<path d="M2,2 L8,8 M8,2 L2,8" stroke="{c}" stroke-width="1.5"/>"#
                    ),
                    "bar" => format!(r#"<path d="M5,1 L5,9" stroke="{c}" stroke-width="1.5"/>"#),
                    // crow's foot (ER "many")
                    "crow" => format!(
                        r#"<path d="M9,5 L1,1 M9,5 L1,9 M9,5 L1,5" fill="none" stroke="{c}"/>"#
                    ),
                    _ => format!(r#"<path d="M1,1 L9,5 L1,9 z" fill="{c}"/>"#),
                };
                let _ = writeln!(
                    self.defs,
                    r#"<marker id="{id}" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="8" markerHeight="8" orient="auto-start-reverse">{shape}</marker>"#
                );
            }
        }
        id
    }

    /// Draws `other` over this picture (its markers merged by name).
    pub fn absorb(&mut self, other: Svg) {
        for (id, def) in other.markers.iter().zip(other.defs.lines()) {
            if !self.markers.contains(id) {
                self.markers.push(id.clone());
                self.defs.push_str(def);
                self.defs.push('\n');
            }
        }
        self.body.push_str(&other.body);
        if other.min_x.is_finite() {
            self.include(other.min_x, other.min_y);
            self.include(other.max_x, other.max_y);
        }
    }

    /// The finished document, padded by `pad` around its content.
    pub fn finish(self, pad: f64) -> Vec<u8> {
        let bounds = if self.min_x.is_finite() {
            (
                self.min_x - pad,
                self.min_y - pad,
                self.max_x + pad,
                self.max_y + pad,
            )
        } else {
            (0.0, 0.0, 100.0, 60.0)
        };
        self.finish_in(bounds)
    }

    /// The finished document showing exactly `(min x, min y, max x, max y)`.
    pub fn finish_in(self, (min_x, min_y, max_x, max_y): (f64, f64, f64, f64)) -> Vec<u8> {
        let (w, h) = ((max_x - min_x).max(1.0), (max_y - min_y).max(1.0));
        let scale = (MAX_SIDE / w.max(h)).min(1.0);
        let mut out = String::with_capacity(self.body.len() + self.defs.len() + 512);
        let _ = writeln!(
            out,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" viewBox="{} {} {} {}">"#,
            n((w * scale).ceil()),
            n((h * scale).ceil()),
            n(min_x),
            n(min_y),
            n(w),
            n(h)
        );
        if !self.defs.is_empty() {
            let _ = writeln!(out, "<defs>\n{}</defs>", self.defs);
        }
        if let Some(bg) = &self.background {
            let _ = writeln!(
                out,
                r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}"/>"#,
                n(min_x),
                n(min_y),
                n(w),
                n(h),
                esc(bg)
            );
        }
        out.push_str(&self.body);
        out.push_str("</svg>\n");
        out.into_bytes()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    Start,
    Middle,
    End,
}

/// Fill and stroke of a shape or line.
#[derive(Clone, Debug)]
pub struct Style {
    pub fill: String,
    pub stroke: String,
    pub width: f64,
    pub dash: Option<String>,
    pub opacity: Option<f64>,
}

impl Style {
    pub fn new(fill: &str, stroke: &str, width: f64) -> Self {
        Style {
            fill: fill.to_string(),
            stroke: stroke.to_string(),
            width,
            dash: None,
            opacity: None,
        }
    }

    pub fn line(stroke: &str, width: f64) -> Self {
        Style::new("none", stroke, width)
    }

    pub fn dashed(mut self, dash: &str) -> Self {
        self.dash = Some(dash.to_string());
        self
    }

    fn attrs(&self) -> String {
        let mut out = format!(
            r#"fill="{}" stroke="{}" stroke-width="{}""#,
            esc(&self.fill),
            esc(&self.stroke),
            n(self.width)
        );
        if let Some(d) = &self.dash {
            let _ = write!(out, r#" stroke-dasharray="{}""#, esc(d));
        }
        if let Some(o) = self.opacity {
            let _ = write!(out, r#" opacity="{}""#, n(o));
        }
        out.push_str(r#" stroke-linejoin="round""#);
        out
    }
}

/// A number as short SVG text.
pub fn n(v: f64) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let r = (v * 100.0).round() / 100.0;
    if r == r.trunc() {
        format!("{}", r as i64)
    } else {
        format!("{r}")
    }
}

/// `M x y L …` through the points.
pub fn poly_path(points: &[(f64, f64)]) -> String {
    let mut d = String::new();
    for (ix, (x, y)) in points.iter().enumerate() {
        let _ = write!(d, "{}{},{} ", if ix == 0 { "M" } else { "L" }, n(*x), n(*y));
    }
    d.trim_end().to_string()
}

/// A smooth curve through the points (Catmull-Rom as cubic Béziers).
pub fn smooth_path(points: &[(f64, f64)]) -> String {
    if points.len() < 3 {
        return poly_path(points);
    }
    let mut d = format!("M{},{}", n(points[0].0), n(points[0].1));
    for i in 0..points.len() - 1 {
        let p0 = if i == 0 { points[0] } else { points[i - 1] };
        let p1 = points[i];
        let p2 = points[i + 1];
        let p3 = if i + 2 < points.len() {
            points[i + 2]
        } else {
            p2
        };
        let c1 = (p1.0 + (p2.0 - p0.0) / 6.0, p1.1 + (p2.1 - p0.1) / 6.0);
        let c2 = (p2.0 - (p3.0 - p1.0) / 6.0, p2.1 - (p3.1 - p1.1) / 6.0);
        let _ = write!(
            d,
            " C{},{} {},{} {},{}",
            n(c1.0),
            n(c1.1),
            n(c2.0),
            n(c2.1),
            n(p2.0),
            n(p2.1)
        );
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documents_fit_their_content() {
        let mut svg = Svg::new();
        svg.rect(
            10.0,
            20.0,
            100.0,
            50.0,
            4.0,
            &Style::new("#fff", "#000", 1.0),
        );
        svg.text(60.0, 45.0, "A & B", 14.0, Anchor::Middle, "#000", None);
        let out = String::from_utf8(svg.finish(10.0)).unwrap();
        assert!(out.contains(r#"viewBox="-1 9 122 72""#), "{out}");
        assert!(out.contains("A &amp; B"));
    }

    #[test]
    fn colors_are_sanitized() {
        assert_eq!(color("#FF0000", "x"), "#FF0000");
        assert_eq!(color("red", "x"), "red");
        assert_eq!(color("url(javascript:1)", "x"), "x");
        assert_eq!(color("\" onload=\"", "x"), "x");
    }

    #[test]
    fn numbers_are_short() {
        assert_eq!(n(1.0), "1");
        assert_eq!(n(1.234), "1.23");
        assert_eq!(n(f64::NAN), "0");
    }
}
