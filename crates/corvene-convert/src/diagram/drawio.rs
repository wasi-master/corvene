//! draw.io / diagrams.net files (`.drawio`, `.dio`): every page drawn
//! from its stored geometry (pages stacked, each under its name). A page
//! may be plain XML or draw.io's compressed form (base64 of raw deflate of
//! the URI-encoded XML). Cells keep their position, size, fill, stroke,
//! font color and size, rounding and dashes; shapes map to rectangles,
//! ellipses, rhombi, triangles, hexagons, cylinders, documents, notes,
//! swimlanes and text; edges follow their waypoints (orthogonal styles get
//! elbows) with their arrowheads and labels. Stencils (`mxgraph.*`) show
//! as boxes with their label; images as framed placeholders.

use std::collections::HashMap;
use std::io::Read as _;

use base64::Engine as _;

use super::svg::{Anchor, Style, Svg, color, text_width};

/// Cells drawn per page; the rest are left out.
const MAX_CELLS: usize = 20_000;

struct Cell {
    id: String,
    parent: String,
    value: String,
    style: HashMap<String, String>,
    shape: String,
    vertex: bool,
    edge: bool,
    source: Option<String>,
    target: Option<String>,
    geo: Option<(f64, f64, f64, f64)>,
    relative: bool,
    points: Vec<(f64, f64)>,
    source_point: Option<(f64, f64)>,
    target_point: Option<(f64, f64)>,
}

fn parse_style(style: &str) -> (String, HashMap<String, String>) {
    let mut map = HashMap::new();
    let mut shape = String::new();
    for (ix, part) in style.split(';').enumerate() {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match part.split_once('=') {
            Some((k, v)) => {
                map.insert(k.to_string(), v.to_string());
            }
            None if ix == 0 => shape = part.to_string(),
            None => {
                map.insert(part.to_string(), "1".into());
            }
        }
    }
    if let Some(s) = map.get("shape") {
        shape = s.clone();
    }
    (shape, map)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A compressed page's XML.
fn inflate_page(text: &str) -> Option<String> {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let raw = base64::engine::general_purpose::STANDARD
        .decode(compact)
        .ok()?;
    let mut out = Vec::new();
    flate2::read::DeflateDecoder::new(&raw[..])
        .take(64 << 20)
        .read_to_end(&mut out)
        .ok()?;
    let text = String::from_utf8_lossy(&out);
    Some(percent_decode(&text))
}

/// HTML labels as text (tags dropped, `<br>` and block ends break lines).
fn plain(value: &str, html: bool) -> String {
    if !html {
        return value.to_string();
    }
    let text = crate::email::html::to_text(value);
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// `text` wrapped to `width` at `size`.
fn wrap(text: &str, width: f64, size: f64) -> String {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split_whitespace() {
            let candidate = if line.is_empty() {
                word.to_string()
            } else {
                format!("{line} {word}")
            };
            if text_width(&candidate, size) > width && !line.is_empty() {
                out.push(std::mem::take(&mut line));
                line = word.to_string();
            } else {
                line = candidate;
            }
        }
        out.push(line);
    }
    out.join("\n")
}

fn num(map: &HashMap<String, String>, key: &str, default: f64) -> f64 {
    map.get(key)
        .and_then(|v| v.parse().ok())
        .filter(|v: &f64| v.is_finite())
        .unwrap_or(default)
}

fn cells(model: roxmltree::Node<'_, '_>) -> Vec<Cell> {
    let mut out = Vec::new();
    for node in model
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "mxCell")
    {
        if out.len() >= MAX_CELLS {
            break;
        }
        // `<UserObject label="…">` / `<object label="…">` wrap a cell
        let wrapper = node
            .parent()
            .filter(|p| p.is_element() && matches!(p.tag_name().name(), "UserObject" | "object"));
        let id = wrapper
            .and_then(|w| w.attribute("id"))
            .or_else(|| node.attribute("id"))
            .unwrap_or("")
            .to_string();
        let value = wrapper
            .and_then(|w| w.attribute("label"))
            .or_else(|| node.attribute("value"))
            .unwrap_or("")
            .to_string();
        let (shape, style) = parse_style(node.attribute("style").unwrap_or(""));
        let geo_node = node
            .children()
            .find(|c| c.is_element() && c.tag_name().name() == "mxGeometry");
        let attr = |n: roxmltree::Node<'_, '_>, k: &str| {
            n.attribute(k)
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.0)
        };
        let mut cell = Cell {
            id,
            parent: node.attribute("parent").unwrap_or("").to_string(),
            value,
            style,
            shape,
            vertex: node.attribute("vertex") == Some("1"),
            edge: node.attribute("edge") == Some("1"),
            source: node.attribute("source").map(str::to_string),
            target: node.attribute("target").map(str::to_string),
            geo: None,
            relative: false,
            points: Vec::new(),
            source_point: None,
            target_point: None,
        };
        if let Some(g) = geo_node {
            cell.geo = Some((
                attr(g, "x"),
                attr(g, "y"),
                attr(g, "width"),
                attr(g, "height"),
            ));
            cell.relative = g.attribute("relative") == Some("1");
            for p in g
                .descendants()
                .filter(|c| c.is_element() && c.tag_name().name() == "mxPoint")
            {
                let pt = (attr(p, "x"), attr(p, "y"));
                match p.attribute("as") {
                    Some("sourcePoint") => cell.source_point = Some(pt),
                    Some("targetPoint") => cell.target_point = Some(pt),
                    Some("offset") => {}
                    _ => {
                        if p.parent().is_some_and(|a| a.tag_name().name() == "Array") {
                            cell.points.push(pt);
                        }
                    }
                }
            }
        }
        out.push(cell);
    }
    out
}

/// Where a cell's geometry is measured from: its parent vertex's origin
/// plus that vertex's position (groups, containers, swimlanes).
fn origin_of(
    ix: usize,
    cells: &[Cell],
    by_id: &HashMap<&str, usize>,
    memo: &mut HashMap<usize, (f64, f64)>,
    depth: usize,
) -> (f64, f64) {
    if let Some(o) = memo.get(&ix) {
        return *o;
    }
    let parent = by_id.get(cells[ix].parent.as_str()).copied();
    let o = match parent {
        Some(p) if depth < 64 && cells[p].vertex => {
            let (px, py) = origin_of(p, cells, by_id, memo, depth + 1);
            let (gx, gy, _, _) = cells[p].geo.unwrap_or_default();
            (px + gx, py + gy)
        }
        Some(p) if depth < 64 => origin_of(p, cells, by_id, memo, depth + 1),
        _ => (0.0, 0.0),
    };
    memo.insert(ix, o);
    o
}

/// Draws one page's cells at a vertical offset; returns the lowest y it
/// drew (or `-inf` for an empty page).
fn draw_page(svg: &mut Svg, cells: &[Cell], dy: f64) -> f64 {
    let by_id: HashMap<&str, usize> = cells
        .iter()
        .enumerate()
        .map(|(i, c)| (c.id.as_str(), i))
        .collect();
    // absolute origin of each cell (children of vertices are relative)
    let mut origin: HashMap<usize, (f64, f64)> = HashMap::new();
    let mut boxes: HashMap<usize, (f64, f64, f64, f64)> = HashMap::new();
    for (ix, c) in cells.iter().enumerate() {
        if c.vertex
            && let Some((x, y, w, h)) = c.geo
        {
            let (ox, oy) = origin_of(ix, cells, &by_id, &mut origin, 0);
            boxes.insert(ix, (ox + x, oy + y + dy, w, h));
        }
    }
    let bottom = boxes
        .values()
        .map(|b| b.1 + b.3)
        .fold(f64::NEG_INFINITY, f64::max);
    // vertices in file order (which is draw.io's z order), edges after
    // their endpoints exist
    for (ix, c) in cells.iter().enumerate() {
        if c.vertex
            && let Some(&b) = boxes.get(&ix)
        {
            draw_vertex(svg, c, b);
        } else if c.edge {
            let (ox, oy) = origin_of(ix, cells, &by_id, &mut origin, 0);
            draw_edge(svg, c, cells, &by_id, &boxes, (ox, oy + dy));
        }
    }
    bottom
}

fn draw_vertex(svg: &mut Svg, c: &Cell, (x, y, w, h): (f64, f64, f64, f64)) {
    let st = &c.style;
    let get = |k: &str| st.get(k).map(String::as_str);
    let fill = color(get("fillColor").unwrap_or("#ffffff"), "#ffffff");
    let stroke = color(get("strokeColor").unwrap_or("#000000"), "#000000");
    let mut style = Style::new(&fill, &stroke, num(st, "strokeWidth", 1.0));
    if get("dashed") == Some("1") {
        style = style.dashed("6 4");
    }
    let opacity = num(st, "opacity", 100.0);
    if opacity < 100.0 {
        style.opacity = Some((opacity / 100.0).clamp(0.0, 1.0));
    }
    let shape = c.shape.as_str();
    let rounded = get("rounded") == Some("1");
    let font_size = num(st, "fontSize", 12.0).clamp(4.0, 200.0);
    let font_color = color(get("fontColor").unwrap_or("#000000"), "#000000");
    let mut label_box = (x, y, w, h);
    match shape {
        "text" | "label" if get("fillColor").is_none() && get("strokeColor").is_none() => {}
        "edgeLabel" => {}
        "ellipse" | "doubleEllipse" | "cloud" => {
            svg.ellipse(x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0, &style);
            if shape == "doubleEllipse" {
                svg.ellipse(
                    x + w / 2.0,
                    y + h / 2.0,
                    w / 2.0 - 4.0,
                    h / 2.0 - 4.0,
                    &style,
                );
            }
        }
        "rhombus" => svg.polygon(
            &[
                (x + w / 2.0, y),
                (x + w, y + h / 2.0),
                (x + w / 2.0, y + h),
                (x, y + h / 2.0),
            ],
            &style,
        ),
        "triangle" => svg.polygon(&[(x, y), (x + w, y + h / 2.0), (x, y + h)], &style),
        "hexagon" => {
            let d = w * 0.2;
            svg.polygon(
                &[
                    (x + d, y),
                    (x + w - d, y),
                    (x + w, y + h / 2.0),
                    (x + w - d, y + h),
                    (x + d, y + h),
                    (x, y + h / 2.0),
                ],
                &style,
            );
        }
        "parallelogram" => {
            let d = w * 0.2;
            svg.polygon(
                &[(x + d, y), (x + w, y), (x + w - d, y + h), (x, y + h)],
                &style,
            );
        }
        "trapezoid" => {
            let d = w * 0.2;
            svg.polygon(
                &[(x + d, y), (x + w - d, y), (x + w, y + h), (x, y + h)],
                &style,
            );
        }
        "cylinder" | "cylinder3" | "datastore" => {
            let ry = (h * 0.1).min(15.0);
            svg.rect(x, y + ry, w, h - 2.0 * ry, 0.0, &style);
            svg.ellipse(x + w / 2.0, y + h - ry, w / 2.0, ry, &style);
            svg.rect(
                x + style.width,
                y + ry,
                w - 2.0 * style.width,
                h - 2.0 * ry,
                0.0,
                &Style::new(&fill, "none", 0.0),
            );
            svg.ellipse(x + w / 2.0, y + ry, w / 2.0, ry, &style);
        }
        "document" => {
            let wave = h * 0.1;
            let d = format!(
                "M{},{} L{},{} L{},{} Q{},{} {},{} T{},{} Z",
                super::svg::n(x),
                super::svg::n(y),
                super::svg::n(x + w),
                super::svg::n(y),
                super::svg::n(x + w),
                super::svg::n(y + h - wave),
                super::svg::n(x + w * 0.75),
                super::svg::n(y + h - 2.0 * wave),
                super::svg::n(x + w / 2.0),
                super::svg::n(y + h - wave),
                super::svg::n(x),
                super::svg::n(y + h - wave)
            );
            svg.include_rect(x, y, w, h);
            svg.path(&d, &style, None, None);
        }
        "note" => {
            let f = num(st, "size", 15.0);
            svg.polygon(
                &[
                    (x, y),
                    (x + w - f, y),
                    (x + w, y + f),
                    (x + w, y + h),
                    (x, y + h),
                ],
                &style,
            );
        }
        "process" => {
            svg.rect(x, y, w, h, 0.0, &style);
            let d = w * 0.1;
            let line = Style::line(&stroke, style.width);
            svg.line(&[(x + d, y), (x + d, y + h)], false, &line, None, None);
            svg.line(
                &[(x + w - d, y), (x + w - d, y + h)],
                false,
                &line,
                None,
                None,
            );
        }
        "swimlane" | "table" => {
            let head = num(st, "startSize", 26.0).min(h);
            let horizontal = get("horizontal") != Some("0");
            svg.rect(
                x,
                y,
                w,
                h,
                if rounded { 6.0 } else { 0.0 },
                &Style::new(
                    &color(get("swimlaneFillColor").unwrap_or("none"), "none"),
                    &stroke,
                    style.width,
                ),
            );
            if horizontal {
                svg.rect(x, y, w, head, 0.0, &style);
                label_box = (x, y, w, head);
            } else {
                svg.rect(x, y, head, h, 0.0, &style);
                label_box = (x, y, head, h);
            }
        }
        "image" => {
            svg.rect(
                x,
                y,
                w,
                h,
                0.0,
                &Style::new("#f6f8fa", "#8c959f", 1.0).dashed("4 3"),
            );
            if c.value.is_empty() {
                svg.text(
                    x + w / 2.0,
                    y + h / 2.0,
                    "image",
                    11.0,
                    Anchor::Middle,
                    "#57606a",
                    None,
                );
            }
        }
        "line" => svg.line(
            &[(x, y + h / 2.0), (x + w, y + h / 2.0)],
            false,
            &Style::line(&stroke, num(st, "strokeWidth", 1.0)),
            None,
            None,
        ),
        _ => {
            let r = if rounded {
                (w.min(h) * num(st, "arcSize", 10.0) / 100.0).min(20.0)
            } else {
                0.0
            };
            svg.rect(x, y, w, h, r, &style);
        }
    }
    if c.value.is_empty() || get("noLabel") == Some("1") {
        return;
    }
    let html = get("html") == Some("1");
    let mut text = plain(&c.value, html);
    let (lx, ly, lw, lh) = label_box;
    if get("whiteSpace") == Some("wrap") && lw > 10.0 {
        text = wrap(&text, lw - 8.0, font_size);
    }
    let lines = text.split('\n').count() as f64;
    let block = lines * font_size * 1.2;
    let (tx, anchor) = match get("align") {
        Some("left") => (lx + 4.0, Anchor::Start),
        Some("right") => (lx + lw - 4.0, Anchor::End),
        _ => (lx + lw / 2.0, Anchor::Middle),
    };
    let ty = match (get("verticalLabelPosition"), get("verticalAlign")) {
        (Some("bottom"), _) => ly + lh + 4.0 + block / 2.0,
        (Some("top"), _) => ly - 4.0 - block / 2.0,
        (_, Some("top")) => ly + 4.0 + block / 2.0,
        (_, Some("bottom")) => ly + lh - 4.0 - block / 2.0,
        _ => ly + lh / 2.0,
    };
    let weight = (num(st, "fontStyle", 0.0) as i64 & 1 == 1).then_some("bold");
    svg.text(tx, ty, &text, font_size, anchor, &font_color, weight);
}

fn arrow_kind(
    name: Option<&str>,
    fill: bool,
    default: Option<&'static str>,
) -> Option<&'static str> {
    match name {
        None => default,
        Some("none") | Some("") => None,
        Some("open") | Some("openThin") | Some("openAsync") => Some("open"),
        Some("diamond") | Some("diamondThin") => Some(if fill { "diamond" } else { "odiamond" }),
        Some("oval") | Some("circle") | Some("circlePlus") => {
            Some(if fill { "dot" } else { "circle" })
        }
        Some("dash") => Some("bar"),
        Some("cross") => Some("cross"),
        Some("ERmany") | Some("ERoneToMany") | Some("ERzeroToMany") => Some("crow"),
        Some("ERone") | Some("ERmandOne") => Some("bar"),
        Some("block") | Some("classic") | Some("classicThin") | Some("blockThin")
        | Some("async") => Some(if fill { "arrow" } else { "triangle" }),
        Some(_) => Some("arrow"),
    }
}

/// Draws an edge; `origin` is where its own points are measured from.
fn draw_edge(
    svg: &mut Svg,
    c: &Cell,
    cells: &[Cell],
    by_id: &HashMap<&str, usize>,
    boxes: &HashMap<usize, (f64, f64, f64, f64)>,
    origin: (f64, f64),
) {
    let st = &c.style;
    let get = |k: &str| st.get(k).map(String::as_str);
    let ends = |id: &Option<String>| {
        id.as_ref()
            .and_then(|i| by_id.get(i.as_str()))
            .and_then(|i| boxes.get(i))
            .copied()
    };
    let src = ends(&c.source);
    let dst = ends(&c.target);
    let center = |b: (f64, f64, f64, f64)| (b.0 + b.2 / 2.0, b.1 + b.3 / 2.0);
    let shift = |p: (f64, f64)| (p.0 + origin.0, p.1 + origin.1);
    let mut pts: Vec<(f64, f64)> = Vec::new();
    let start = src.map(center).or(c.source_point.map(shift));
    let end = dst.map(center).or(c.target_point.map(shift));
    let (Some(start), Some(end)) = (start, end) else {
        return;
    };
    pts.push(start);
    pts.extend(c.points.iter().map(|p| shift(*p)));
    pts.push(end);
    // elbows for orthogonal styles
    let orthogonal = get("edgeStyle").is_some_and(|s| {
        s.contains("orthogonal") || s.contains("elbow") || s.contains("entityRelation")
    });
    if orthogonal {
        let mut elbowed = vec![pts[0]];
        for w in pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            if (a.0 - b.0).abs() > 0.5 && (a.1 - b.1).abs() > 0.5 {
                let horizontal_first =
                    get("elbow") != Some("vertical") && (b.0 - a.0).abs() > (b.1 - a.1).abs();
                elbowed.push(if horizontal_first {
                    (b.0, a.1)
                } else {
                    (a.0, b.1)
                });
            }
            elbowed.push(b);
        }
        pts = elbowed;
    }
    // clip ends to the boxes
    let clip = |b: (f64, f64, f64, f64), toward: (f64, f64)| -> (f64, f64) {
        let (cx, cy) = center(b);
        let (dx, dy) = (toward.0 - cx, toward.1 - cy);
        if dx.abs() < 1e-9 && dy.abs() < 1e-9 {
            return (cx, cy);
        }
        let tx = if dx.abs() > 1e-9 {
            b.2 / 2.0 / dx.abs()
        } else {
            f64::INFINITY
        };
        let ty = if dy.abs() > 1e-9 {
            b.3 / 2.0 / dy.abs()
        } else {
            f64::INFINITY
        };
        let t = tx.min(ty).min(1.0);
        (cx + dx * t, cy + dy * t)
    };
    if let Some(b) = src {
        pts[0] = clip(b, pts[1]);
    }
    let k = pts.len();
    if let Some(b) = dst {
        pts[k - 1] = clip(b, pts[k - 2]);
    }
    let stroke = color(get("strokeColor").unwrap_or("#000000"), "#000000");
    let mut style = Style::line(&stroke, num(st, "strokeWidth", 1.0));
    if get("dashed") == Some("1") {
        style = style.dashed("6 4");
    }
    let head = arrow_kind(get("endArrow"), get("endFill") != Some("0"), Some("arrow"));
    let tail = arrow_kind(get("startArrow"), get("startFill") != Some("0"), None);
    let smooth = get("curved") == Some("1");
    svg.line(&pts, smooth, &style, head, tail);
    // the edge's own label, and labels of child cells
    let mut labels: Vec<String> = Vec::new();
    if !c.value.is_empty() {
        labels.push(plain(&c.value, get("html") == Some("1")));
    }
    for child in cells
        .iter()
        .filter(|x| x.parent == c.id && x.vertex && x.relative)
    {
        if !child.value.is_empty() {
            labels.push(plain(
                &child.value,
                child.style.get("html").map(String::as_str) == Some("1"),
            ));
        }
    }
    if !labels.is_empty() {
        let mid = pts.len() / 2;
        let (mx, my) = if pts.len().is_multiple_of(2) {
            (
                (pts[mid - 1].0 + pts[mid].0) / 2.0,
                (pts[mid - 1].1 + pts[mid].1) / 2.0,
            )
        } else {
            pts[mid]
        };
        let text = labels.join("\n");
        let size = num(st, "fontSize", 11.0).clamp(4.0, 100.0);
        let w = text
            .split('\n')
            .map(|l| text_width(l, size))
            .fold(0.0, f64::max)
            + 6.0;
        let h = text.split('\n').count() as f64 * size * 1.2 + 2.0;
        svg.rect(
            mx - w / 2.0,
            my - h / 2.0,
            w,
            h,
            0.0,
            &Style::new("#ffffff", "none", 0.0),
        );
        svg.text(
            mx,
            my,
            &text,
            size,
            Anchor::Middle,
            &color(get("fontColor").unwrap_or("#000000"), "#000000"),
            None,
        );
    }
}

/// The picture of a draw.io file.
pub fn render(text: &str) -> Result<Vec<u8>, String> {
    let doc = roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
    .map_err(|e| format!("Not a readable draw.io file: {e}."))?;
    let root = doc.root_element();
    let mut pages: Vec<(String, Vec<Cell>)> = Vec::new();
    match root.tag_name().name() {
        "mxGraphModel" => pages.push((String::new(), cells(root))),
        "mxfile" => {
            for diagram in root
                .children()
                .filter(|c| c.is_element() && c.tag_name().name() == "diagram")
            {
                let name = diagram.attribute("name").unwrap_or("").to_string();
                if let Some(model) = diagram
                    .children()
                    .find(|c| c.is_element() && c.tag_name().name() == "mxGraphModel")
                {
                    pages.push((name, cells(model)));
                } else if let Some(xml) = diagram.text().and_then(inflate_page) {
                    let inner = roxmltree::Document::parse(&xml)
                        .map_err(|e| format!("A compressed page does not read: {e}."))?;
                    pages.push((name, cells(inner.root_element())));
                }
            }
        }
        _ => return Err("Not a draw.io file.".into()),
    }
    if pages.is_empty()
        || pages
            .iter()
            .all(|(_, c)| c.iter().all(|c| !c.vertex && !c.edge))
    {
        return Err("The drawing has no shapes.".into());
    }
    let mut svg = Svg::new();
    let many = pages.len() > 1;
    let mut dy = 0.0;
    for (name, cells) in &pages {
        // top-level shapes (a child's y is relative to its container)
        let vertices: std::collections::HashSet<&str> = cells
            .iter()
            .filter(|c| c.vertex)
            .map(|c| c.id.as_str())
            .collect();
        let page_top = cells
            .iter()
            .filter(|c| c.vertex && !vertices.contains(c.parent.as_str()))
            .filter_map(|c| c.geo.map(|g| g.1))
            .fold(f64::INFINITY, f64::min);
        let offset = if page_top.is_finite() {
            dy - page_top
        } else {
            dy
        };
        if many {
            svg.text(
                0.0,
                dy - 18.0,
                if name.is_empty() { "Page" } else { name },
                16.0,
                Anchor::Start,
                "#57606a",
                Some("600"),
            );
        }
        let bottom = draw_page(&mut svg, cells, offset);
        dy = if bottom.is_finite() {
            bottom + 80.0
        } else {
            dy + 120.0
        };
    }
    Ok(svg.finish(20.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: &str = r#"<mxfile host="app.diagrams.net"><diagram id="a" name="Page-1"><mxGraphModel><root>
<mxCell id="0"/><mxCell id="1" parent="0"/>
<mxCell id="2" value="Start" style="rounded=1;whiteSpace=wrap;html=1;fillColor=#dae8fc;strokeColor=#6c8ebf;" vertex="1" parent="1"><mxGeometry x="40" y="40" width="120" height="60" as="geometry"/></mxCell>
<mxCell id="3" value="&lt;b&gt;Check&lt;/b&gt;" style="rhombus;html=1;" vertex="1" parent="1"><mxGeometry x="40" y="160" width="120" height="80" as="geometry"/></mxCell>
<mxCell id="4" value="yes" style="edgeStyle=orthogonalEdgeStyle;endArrow=open;" edge="1" parent="1" source="2" target="3"><mxGeometry relative="1" as="geometry"/></mxCell>
</root></mxGraphModel></diagram></mxfile>"#;

    #[test]
    fn plain_pages() {
        let svg = String::from_utf8(render(PLAIN).unwrap()).unwrap();
        assert!(svg.contains("fill=\"#dae8fc\""), "{svg}");
        assert!(svg.contains(">Start<"), "{svg}");
        assert!(svg.contains(">Check<"), "{svg}");
        assert!(svg.contains("<polygon"), "{svg}");
        assert!(svg.contains(">yes<"), "{svg}");
        assert!(svg.contains("marker-end"), "{svg}");
    }

    #[test]
    fn compressed_pages() {
        // the page above, compressed the way draw.io does it
        let start = PLAIN.find("<mxGraphModel>").unwrap();
        let end = PLAIN.find("</mxGraphModel>").unwrap() + "</mxGraphModel>".len();
        let model = &PLAIN[start..end];
        let encoded: String = model
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect();
        let mut deflate =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut deflate, encoded.as_bytes()).unwrap();
        let packed = base64::engine::general_purpose::STANDARD.encode(deflate.finish().unwrap());
        let file = format!(
            "<mxfile><diagram name=\"One\">{packed}</diagram><diagram name=\"Two\">{packed}</diagram></mxfile>"
        );
        let svg = String::from_utf8(render(&file).unwrap()).unwrap();
        assert!(svg.contains(">One<") && svg.contains(">Two<"), "{svg}");
        assert_eq!(svg.matches(">Start<").count(), 2);
    }
}
