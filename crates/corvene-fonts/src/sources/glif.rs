//! UFO `.glif` glyphs (GLIF formats 1 and 2, UFO 2 and 3).

use std::collections::BTreeMap;

use super::plist::{Element, parse_xml, plist_value};
use super::{
    Anchor, Component, MAX_COMPONENT_DEPTH, OutlineItem, PointKind, SourceGlyph, SrcPoint,
    contour_cmds, flatten,
};
use crate::FontError;

const FORMAT: &str = "GLIF";

/// Bases read from `siblings` at most, per glyph (bounds the work a
/// malicious UFO can cause).
const MAX_SIBLINGS: usize = 4096;

/// Reads a `.glif` glyph. `siblings` are the other glyphs of the same UFO
/// layer at the same revision (glif bytes by glyph name), used to draw
/// components; bases that are missing, cyclic or nested more than 8 deep
/// are listed in [`SourceGlyph::unresolved`].
pub fn parse_glif(
    bytes: &[u8],
    siblings: &BTreeMap<String, Vec<u8>>,
) -> Result<SourceGlyph, FontError> {
    let root = parse_xml(bytes, FORMAT)?;
    if root.name != "glyph" {
        return Err(FontError::malformed(FORMAT, "root element is not <glyph>"));
    }
    let format1 = is_format1(&root);
    let (items, mut anchors) = outline(&root, format1);

    // read the bases the outline reaches, as deep as they can be drawn
    let mut bases: BTreeMap<String, Vec<OutlineItem>> = BTreeMap::new();
    let mut queue: Vec<(String, usize)> = component_bases(&items)
        .into_iter()
        .map(|b| (b, 1))
        .collect();
    while let Some((name, depth)) = queue.pop() {
        if depth > MAX_COMPONENT_DEPTH || bases.len() >= MAX_SIBLINGS || bases.contains_key(&name) {
            continue;
        }
        let Some(glif) = siblings.get(&name) else {
            continue;
        };
        let Ok(el) = parse_xml(glif, FORMAT) else {
            continue;
        };
        let (base_items, _) = outline(&el, is_format1(&el));
        queue.extend(
            component_bases(&base_items)
                .into_iter()
                .map(|b| (b, depth + 1)),
        );
        bases.insert(name, base_items);
    }

    let mut unresolved = Vec::new();
    let lookup = |name: &str| bases.get(name).map(Vec::as_slice);
    let path = flatten(&items, &lookup, &mut unresolved);

    for a in root.children_named("anchor") {
        anchors.push(Anchor {
            name: a.attr("name").unwrap_or_default().to_string(),
            x: a.num("x").unwrap_or(0.0),
            y: a.num("y").unwrap_or(0.0),
        });
    }
    let advance = root.child("advance");
    Ok(SourceGlyph {
        name: root.attr("name").unwrap_or_default().to_string(),
        unicodes: root
            .children_named("unicode")
            .filter_map(|u| u32::from_str_radix(u.attr("hex")?.trim(), 16).ok())
            .collect(),
        advance: advance.and_then(|a| a.num("width")).unwrap_or(0.0),
        height: advance.and_then(|a| a.num("height")),
        path,
        components: items
            .into_iter()
            .filter_map(|i| match i {
                OutlineItem::Component(c) => Some(c),
                OutlineItem::Contour(_) => None,
            })
            .collect(),
        anchors,
        guidelines: root.children_named("guideline").count(),
        note: root
            .child("note")
            .map(|n| n.text.trim().to_string())
            .filter(|n| !n.is_empty()),
        lib_keys: root
            .child("lib")
            .and_then(|l| l.children.first())
            .and_then(plist_value)
            .map(|v| v.keys())
            .unwrap_or_default(),
        unresolved,
    })
}

fn is_format1(root: &Element) -> bool {
    root.attr("format").is_none_or(|f| f.trim() == "1")
}

fn component_bases(items: &[OutlineItem]) -> Vec<String> {
    items
        .iter()
        .filter_map(|i| match i {
            OutlineItem::Component(c) => Some(c.base.clone()),
            OutlineItem::Contour(_) => None,
        })
        .collect()
}

/// A glyph's contours and components in order, plus the anchors GLIF 1
/// writes as single named `move` points.
fn outline(root: &Element, format1: bool) -> (Vec<OutlineItem>, Vec<Anchor>) {
    let mut items = Vec::new();
    let mut anchors = Vec::new();
    let Some(outline) = root.child("outline") else {
        return (items, anchors);
    };
    for el in &outline.children {
        match el.name.as_str() {
            "contour" => {
                let points: Vec<&Element> = el.children_named("point").collect();
                if format1
                    && let [p] = points.as_slice()
                    && p.attr("type") == Some("move")
                    && let Some(name) = p.attr("name")
                {
                    anchors.push(Anchor {
                        name: name.to_string(),
                        x: p.num("x").unwrap_or(0.0),
                        y: p.num("y").unwrap_or(0.0),
                    });
                    continue;
                }
                let points: Vec<SrcPoint> = points.iter().map(|p| point(p)).collect();
                let mut cmds = Vec::new();
                contour_cmds(&points, &mut cmds);
                if !cmds.is_empty() {
                    items.push(OutlineItem::Contour(cmds));
                }
            }
            "component" => {
                let Some(base) = el.attr("base") else {
                    continue;
                };
                let t = |k: &str, d: f32| el.num(k).unwrap_or(d);
                items.push(OutlineItem::Component(Component {
                    base: base.to_string(),
                    transform: [
                        t("xScale", 1.0),
                        t("xyScale", 0.0),
                        t("yxScale", 0.0),
                        t("yScale", 1.0),
                        t("xOffset", 0.0),
                        t("yOffset", 0.0),
                    ],
                }));
            }
            _ => {}
        }
    }
    (items, anchors)
}

fn point(p: &Element) -> SrcPoint {
    let kind = match p.attr("type").map(str::trim) {
        Some("move") => PointKind::Move,
        Some("line") => PointKind::Line,
        Some("curve") => PointKind::Curve,
        Some("qcurve") => PointKind::QCurve,
        Some("offcurve") | None => PointKind::OffCurve,
        Some(_) => PointKind::Line,
    };
    SrcPoint {
        x: p.num("x").unwrap_or(0.0),
        y: p.num("y").unwrap_or(0.0),
        kind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outline::PathCmd;

    fn no_siblings() -> BTreeMap<String, Vec<u8>> {
        BTreeMap::new()
    }

    #[test]
    fn reads_a_format_2_glyph_with_mixed_segments() {
        let glif = br#"<?xml version="1.0" encoding="UTF-8"?>
<glyph name="o" format="2">
  <advance width="520"/>
  <unicode hex="006F"/>
  <unicode hex="004F"/>
  <anchor name="top" x="260" y="600"/>
  <guideline x="10" angle="90"/>
  <outline>
    <contour>
      <point x="0" y="0" type="line"/>
      <point x="50" y="100" type="offcurve"/>
      <point x="100" y="100" type="offcurve"/>
      <point x="150" y="0" type="curve" smooth="yes"/>
      <point x="200" y="50"/>
      <point x="250" y="50"/>
      <point x="300" y="0" type="qcurve"/>
    </contour>
  </outline>
  <note>
    round
  </note>
  <lib><dict><key>public.markColor</key><string>1,0,0,1</string></dict></lib>
</glyph>"#;
        let g = parse_glif(glif, &no_siblings()).unwrap();
        assert_eq!(g.name, "o");
        assert_eq!(g.unicodes, vec![0x6F, 0x4F]);
        assert_eq!(g.advance, 520.0);
        assert_eq!(g.height, None);
        assert_eq!(g.anchors.len(), 1);
        assert_eq!(g.guidelines, 1);
        assert_eq!(g.note.as_deref(), Some("round"));
        assert_eq!(g.lib_keys, vec!["public.markColor"]);
        assert_eq!(
            g.path.cmds,
            vec![
                PathCmd::MoveTo(0.0, 0.0),
                PathCmd::CubicTo(50.0, 100.0, 100.0, 100.0, 150.0, 0.0),
                PathCmd::QuadTo(200.0, 50.0, 225.0, 50.0),
                PathCmd::QuadTo(250.0, 50.0, 300.0, 0.0),
                PathCmd::Close,
            ]
        );
        // the implied on-curve point between the two quadratic controls
        // counts too
        assert_eq!(g.path.point_count(), 8);
    }

    #[test]
    fn all_offcurve_contour_is_a_quadratic_loop() {
        let glif = br#"<glyph name="O" format="2"><outline><contour>
<point x="0" y="100"/><point x="100" y="100"/><point x="100" y="0"/><point x="0" y="0"/>
</contour></outline></glyph>"#;
        let g = parse_glif(glif, &no_siblings()).unwrap();
        assert_eq!(
            g.path.cmds,
            vec![
                PathCmd::MoveTo(0.0, 50.0),
                PathCmd::QuadTo(0.0, 100.0, 50.0, 100.0),
                PathCmd::QuadTo(100.0, 100.0, 100.0, 50.0),
                PathCmd::QuadTo(100.0, 0.0, 50.0, 0.0),
                PathCmd::QuadTo(0.0, 0.0, 0.0, 50.0),
                PathCmd::Close,
            ]
        );
    }

    #[test]
    fn components_resolve_through_siblings() {
        let base = br#"<glyph name="a" format="2"><advance width="500"/><outline>
<contour><point x="0" y="0" type="line"/><point x="100" y="0" type="line"/><point x="100" y="100" type="line"/></contour>
</outline></glyph>"#;
        let mid = br#"<glyph name="acomb" format="2"><outline>
<component base="a" xOffset="10"/></outline></glyph>"#;
        let siblings: BTreeMap<String, Vec<u8>> = [
            ("a".to_string(), base.to_vec()),
            ("acomb".to_string(), mid.to_vec()),
        ]
        .into_iter()
        .collect();
        let glif = br#"<glyph name="aacute" format="2"><advance width="500"/><outline>
<component base="acomb" xScale="2" yScale="2" yOffset="5"/>
<component base="acutecomb" xOffset="200"/>
</outline></glyph>"#;
        let g = parse_glif(glif, &siblings).unwrap();
        assert_eq!(g.components.len(), 2);
        assert_eq!(g.components[0].transform, [2.0, 0.0, 0.0, 2.0, 0.0, 5.0]);
        assert_eq!(g.unresolved, vec!["acutecomb"]);
        // (x + 10) * 2, y * 2 + 5
        assert_eq!(g.path.cmds[0], PathCmd::MoveTo(20.0, 5.0));
        assert_eq!(g.path.cmds[1], PathCmd::LineTo(220.0, 5.0));
        assert_eq!(g.path.contours(), 1);
    }

    #[test]
    fn self_reference_does_not_loop() {
        let glif =
            br#"<glyph name="a" format="2"><outline><component base="a"/></outline></glyph>"#;
        let siblings = [("a".to_string(), glif.to_vec())].into_iter().collect();
        let g = parse_glif(glif, &siblings).unwrap();
        assert!(g.path.is_empty());
        assert_eq!(g.unresolved, vec!["a"]);
    }

    #[test]
    fn format_1_named_move_points_are_anchors() {
        let glif = br#"<glyph name="a" format="1"><advance width="500" height="1000"/><outline>
<contour><point x="250" y="700" type="move" name="top"/></contour>
<contour><point x="0" y="0" type="move"/><point x="10" y="10" type="line"/></contour>
</outline></glyph>"#;
        let g = parse_glif(glif, &no_siblings()).unwrap();
        assert_eq!(g.height, Some(1000.0));
        assert_eq!(
            g.anchors,
            vec![Anchor {
                name: "top".into(),
                x: 250.0,
                y: 700.0
            }]
        );
        assert_eq!(
            g.path.cmds,
            vec![PathCmd::MoveTo(0.0, 0.0), PathCmd::LineTo(10.0, 10.0)]
        );
    }

    #[test]
    fn malformed_input_is_an_error() {
        assert!(parse_glif(b"<notaglyph/>", &no_siblings()).is_err());
        assert!(parse_glif(b"<glyph><outline>", &no_siblings()).is_err());
        assert!(parse_glif(b"\xff\xfe", &no_siblings()).is_err());
        // junk numbers read as zero rather than failing
        let g = parse_glif(
            br#"<glyph name="x" format="2"><advance width="wide"/></glyph>"#,
            &no_siblings(),
        )
        .unwrap();
        assert_eq!(g.advance, 0.0);
    }
}
