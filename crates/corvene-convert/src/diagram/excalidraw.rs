//! Excalidraw drawings (`.excalidraw`, `.excalidraw.json`): rectangles,
//! ellipses, diamonds, arrows and lines (with their arrowheads), free
//! drawing, text, frames and image placeholders, drawn from their stored
//! geometry with their colors, stroke width and style, opacity and
//! rotation. Drawn with clean lines rather than Excalidraw's hand-drawn
//! strokes; hachure fills show as a light tint.

use serde_json::Value;

use super::svg::{Anchor, Style, Svg, color, n};

fn f(v: &Value, key: &str) -> f64 {
    v.get(key)
        .and_then(Value::as_f64)
        .filter(|x| x.is_finite())
        .unwrap_or(0.0)
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn arrowhead(v: &Value, key: &str) -> Option<&'static str> {
    match v.get(key).and_then(Value::as_str)? {
        "arrow" => Some("open"),
        "bar" => Some("bar"),
        "dot" | "circle" => Some("dot"),
        "circle_outline" => Some("circle"),
        "triangle" => Some("arrow"),
        "triangle_outline" => Some("triangle"),
        "diamond" => Some("diamond"),
        "diamond_outline" => Some("odiamond"),
        "crowfoot_many" | "crowfoot_one_or_many" => Some("crow"),
        "crowfoot_one" => Some("bar"),
        _ => Some("open"),
    }
}

/// The picture of an Excalidraw file.
pub fn render(text: &str) -> Result<Vec<u8>, String> {
    let root: Value =
        serde_json::from_str(text).map_err(|e| format!("Not a readable Excalidraw file: {e}."))?;
    let elements = root
        .get("elements")
        .and_then(Value::as_array)
        .ok_or("Not an Excalidraw file (no elements).")?;
    let mut svg = Svg::new();
    let background = root
        .get("appState")
        .and_then(|a| a.get("viewBackgroundColor"))
        .and_then(Value::as_str)
        .unwrap_or("#ffffff");
    svg.set_background(Some(&color(background, "#ffffff")));
    let mut drawn = 0;
    for el in elements.iter().take(50_000) {
        if el.get("isDeleted").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let kind = s(el, "type");
        let (x, y, w, h) = (f(el, "x"), f(el, "y"), f(el, "width"), f(el, "height"));
        let stroke = color(s(el, "strokeColor"), "#1e1e1e");
        let bg = color(s(el, "backgroundColor"), "none");
        let fill_style = s(el, "fillStyle");
        let mut style = Style::new(&bg, &stroke, f(el, "strokeWidth").max(0.5));
        match s(el, "strokeStyle") {
            "dashed" => style = style.dashed("8 6"),
            "dotted" => style = style.dashed("2 4"),
            _ => {}
        }
        let mut opacity = el.get("opacity").and_then(Value::as_f64).unwrap_or(100.0) / 100.0;
        if bg != "none" && fill_style != "solid" && kind != "text" {
            // hachure / cross-hatch / zigzag: a tint of the fill color
            let tinted = Style {
                fill: bg.clone(),
                stroke: "none".into(),
                width: 0.0,
                dash: None,
                opacity: Some((opacity * 0.35).clamp(0.0, 1.0)),
            };
            draw_fill(&mut svg, kind, x, y, w, h, &tinted);
            style.fill = "none".into();
        }
        opacity = opacity.clamp(0.0, 1.0);
        if opacity < 1.0 {
            style.opacity = Some(opacity);
        }
        let angle = f(el, "angle");
        let rotated = angle.abs() > 1e-6;
        if rotated {
            svg.raw(&format!(
                r#"<g transform="rotate({} {} {})">"#,
                n(angle.to_degrees()),
                n(x + w / 2.0),
                n(y + h / 2.0)
            ));
        }
        let rounded = el.get("roundness").is_some_and(|r| !r.is_null());
        match kind {
            "rectangle" => {
                let r = if rounded {
                    (w.min(h) * 0.25).min(32.0)
                } else {
                    0.0
                };
                svg.rect(x, y, w, h, r, &style);
            }
            "ellipse" => svg.ellipse(x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0, &style),
            "diamond" => svg.polygon(
                &[
                    (x + w / 2.0, y),
                    (x + w, y + h / 2.0),
                    (x + w / 2.0, y + h),
                    (x, y + h / 2.0),
                ],
                &style,
            ),
            "arrow" | "line" | "freedraw" => {
                let pts: Vec<(f64, f64)> = el
                    .get("points")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|p| {
                                let p = p.as_array()?;
                                Some((x + p.first()?.as_f64()?, y + p.get(1)?.as_f64()?))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let mut line = style.clone();
                if kind != "line" || pts.first() != pts.last() {
                    line.fill = "none".into();
                }
                let (head, tail) = if kind == "arrow" {
                    (
                        arrowhead(el, "endArrowhead"),
                        arrowhead(el, "startArrowhead"),
                    )
                } else {
                    (None, None)
                };
                let smooth = kind == "freedraw" || (rounded && pts.len() > 2);
                svg.line(&pts, smooth, &line, head, tail);
            }
            "text" => {
                let size = match f(el, "fontSize") {
                    s if s <= 0.0 => 20.0,
                    s => s.clamp(4.0, 400.0),
                };
                let content = el
                    .get("text")
                    .or_else(|| el.get("originalText"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let lines = content.split('\n').count() as f64;
                let line_h = el.get("lineHeight").and_then(Value::as_f64).unwrap_or(1.25) * size;
                let (tx, anchor) = match s(el, "textAlign") {
                    "center" => (x + w / 2.0, Anchor::Middle),
                    "right" => (x + w, Anchor::End),
                    _ => (x, Anchor::Start),
                };
                // the text block's middle (the writer centers lines on it)
                let ty = if h > 0.0 {
                    y + h / 2.0
                } else {
                    y + line_h * lines / 2.0
                };
                svg.text(tx, ty, content, size, anchor, &stroke, None);
            }
            "frame" | "magicframe" => {
                svg.rect(
                    x,
                    y,
                    w,
                    h,
                    4.0,
                    &Style::new("none", "#868e96", 1.0).dashed("6 4"),
                );
                let name = s(el, "name");
                svg.text(
                    x,
                    y - 10.0,
                    if name.is_empty() { "Frame" } else { name },
                    14.0,
                    Anchor::Start,
                    "#868e96",
                    None,
                );
            }
            "image" | "embeddable" | "iframe" => {
                svg.rect(
                    x,
                    y,
                    w,
                    h,
                    0.0,
                    &Style::new("#f1f3f5", "#adb5bd", 1.0).dashed("4 3"),
                );
                svg.text(
                    x + w / 2.0,
                    y + h / 2.0,
                    if kind == "image" { "image" } else { "embed" },
                    13.0,
                    Anchor::Middle,
                    "#868e96",
                    None,
                );
            }
            _ => {}
        }
        if rotated {
            svg.raw("</g>");
        }
        drawn += 1;
    }
    if drawn == 0 {
        return Err("The drawing is empty.".into());
    }
    Ok(svg.finish(24.0))
}

fn draw_fill(svg: &mut Svg, kind: &str, x: f64, y: f64, w: f64, h: f64, style: &Style) {
    match kind {
        "rectangle" => svg.rect(x, y, w, h, 0.0, style),
        "ellipse" => svg.ellipse(x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0, style),
        "diamond" => svg.polygon(
            &[
                (x + w / 2.0, y),
                (x + w, y + h / 2.0),
                (x + w / 2.0, y + h),
                (x, y + h / 2.0),
            ],
            style,
        ),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_elements() {
        let json = r##"{"type":"excalidraw","version":2,"elements":[
 {"id":"a","type":"rectangle","x":10,"y":20,"width":100,"height":50,"angle":0,"strokeColor":"#1e1e1e","backgroundColor":"#a5d8ff","fillStyle":"solid","strokeWidth":2,"strokeStyle":"solid","opacity":100,"roundness":{"type":3}},
 {"id":"b","type":"arrow","x":110,"y":45,"width":80,"height":0,"angle":0,"strokeColor":"#e03131","backgroundColor":"transparent","fillStyle":"solid","strokeWidth":2,"points":[[0,0],[80,0]],"endArrowhead":"arrow","startArrowhead":null},
 {"id":"c","type":"text","x":20,"y":35,"width":60,"height":25,"text":"Hello","fontSize":20,"textAlign":"left","strokeColor":"#1e1e1e"},
 {"id":"d","type":"ellipse","x":0,"y":0,"width":10,"height":10,"isDeleted":true}
],"appState":{"viewBackgroundColor":"#ffffff"}}"##;
        let svg = String::from_utf8(render(json).unwrap()).unwrap();
        assert!(svg.contains("fill=\"#a5d8ff\""), "{svg}");
        assert!(svg.contains(">Hello<"), "{svg}");
        assert!(svg.contains("marker-end"), "{svg}");
        assert!(
            !svg.contains("<ellipse"),
            "deleted elements are skipped: {svg}"
        );
        assert!(render(r#"{"elements":[]}"#).is_err());
    }
}
