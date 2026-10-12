//! Readers for the map formats, into [`Feature`]s.

use std::io::Read as _;

use serde_json::Value;

use super::{Feature, Geometry};
use crate::Format;

/// Features read per file; the rest are left out.
const MAX_FEATURES: usize = 200_000;

pub(crate) fn features(format: Format, bytes: &[u8]) -> Result<Vec<Feature>, String> {
    let out = match format {
        Format::GeoJson => geojson(bytes)?,
        Format::TopoJson => topojson(bytes)?,
        Format::Kml => kml(&String::from_utf8_lossy(bytes))?,
        Format::Kmz => kmz(bytes)?,
        Format::Gpx => gpx(&String::from_utf8_lossy(bytes))?,
        Format::Shapefile => shapefile(bytes)?,
        Format::GeoPackage => geopackage(bytes)?,
        _ => return Err("Not a map file.".into()),
    };
    Ok(out)
}

fn name_of(props: &Value) -> Option<String> {
    ["name", "NAME", "Name", "title", "label", "id"]
        .iter()
        .find_map(|k| props.get(k))
        .and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
}

fn pt(v: &Value) -> Option<(f64, f64)> {
    let a = v.as_array()?;
    let x = a.first()?.as_f64()?;
    let y = a.get(1)?.as_f64()?;
    (x.is_finite() && y.is_finite()).then_some((x, y))
}

fn line(v: &Value) -> Vec<(f64, f64)> {
    v.as_array()
        .map(|a| a.iter().filter_map(pt).collect())
        .unwrap_or_default()
}

fn rings(v: &Value) -> Vec<Vec<(f64, f64)>> {
    v.as_array()
        .map(|a| a.iter().map(line).filter(|r| r.len() >= 3).collect())
        .unwrap_or_default()
}

/// A GeoJSON geometry's parts.
fn geometry(g: &Value, out: &mut Vec<Geometry>, depth: usize) {
    if depth > 32 {
        return;
    }
    let coords = g.get("coordinates").unwrap_or(&Value::Null);
    match g.get("type").and_then(Value::as_str).unwrap_or("") {
        "Point" => out.extend(pt(coords).map(Geometry::Point)),
        "MultiPoint" => out.extend(line(coords).into_iter().map(Geometry::Point)),
        "LineString" => out.push(Geometry::Line(line(coords))),
        "MultiLineString" => {
            if let Some(a) = coords.as_array() {
                out.extend(a.iter().map(|l| Geometry::Line(line(l))));
            }
        }
        "Polygon" => out.push(Geometry::Polygon(rings(coords))),
        "MultiPolygon" => {
            if let Some(a) = coords.as_array() {
                out.extend(a.iter().map(|p| Geometry::Polygon(rings(p))));
            }
        }
        "GeometryCollection" => {
            if let Some(a) = g.get("geometries").and_then(Value::as_array) {
                for inner in a {
                    geometry(inner, out, depth + 1);
                }
            }
        }
        _ => {}
    }
}

fn geojson(bytes: &[u8]) -> Result<Vec<Feature>, String> {
    let root: Value =
        serde_json::from_slice(bytes).map_err(|e| format!("Not readable GeoJSON: {e}."))?;
    let mut out = Vec::new();
    let mut add = |f: &Value| {
        let mut parts = Vec::new();
        let props = f.get("properties").cloned().unwrap_or(Value::Null);
        if let Some(g) = f.get("geometry") {
            geometry(g, &mut parts, 0);
        } else {
            geometry(f, &mut parts, 0);
        }
        out.push(Feature {
            parts,
            name: name_of(&props),
            props: props.to_string(),
        });
    };
    match root.get("type").and_then(Value::as_str) {
        Some("FeatureCollection") => {
            for f in root
                .get("features")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .take(MAX_FEATURES)
            {
                add(f);
            }
        }
        Some("Feature") => add(&root),
        Some(_) => add(&root),
        None => return Err("Not GeoJSON (no type).".into()),
    }
    Ok(out)
}

fn topojson(bytes: &[u8]) -> Result<Vec<Feature>, String> {
    let root: Value =
        serde_json::from_slice(bytes).map_err(|e| format!("Not readable TopoJSON: {e}."))?;
    if root.get("type").and_then(Value::as_str) != Some("Topology") {
        return Err("Not TopoJSON.".into());
    }
    let (scale, translate) = match root.get("transform") {
        Some(t) => (
            pt(t.get("scale").unwrap_or(&Value::Null)),
            pt(t.get("translate").unwrap_or(&Value::Null)),
        ),
        None => (None, None),
    };
    let quantized = scale.is_some() && translate.is_some();
    let place = |(x, y): (f64, f64)| -> (f64, f64) {
        match (scale, translate) {
            (Some((sx, sy)), Some((tx, ty))) => (x * sx + tx, y * sy + ty),
            _ => (x, y),
        }
    };
    // decode the arcs (delta encoded when quantized)
    let arcs: Vec<Vec<(f64, f64)>> = root
        .get("arcs")
        .and_then(Value::as_array)
        .map(|arcs| {
            arcs.iter()
                .map(|arc| {
                    let mut out = Vec::new();
                    let (mut x, mut y) = (0.0, 0.0);
                    for p in arc.as_array().into_iter().flatten() {
                        let Some((dx, dy)) = pt(p) else { continue };
                        if quantized {
                            x += dx;
                            y += dy;
                            out.push(place((x, y)));
                        } else {
                            out.push((dx, dy));
                        }
                    }
                    out
                })
                .collect()
        })
        .unwrap_or_default();
    let ring = |refs: &Value| -> Vec<(f64, f64)> {
        let mut out: Vec<(f64, f64)> = Vec::new();
        for r in refs.as_array().into_iter().flatten() {
            let Some(i) = r.as_i64() else { continue };
            let (ix, reversed) = if i < 0 {
                ((!i) as usize, true)
            } else {
                (i as usize, false)
            };
            let Some(arc) = arcs.get(ix) else { continue };
            let mut pts = arc.clone();
            if reversed {
                pts.reverse();
            }
            // shared ends appear once
            if !out.is_empty() && !pts.is_empty() {
                pts.remove(0);
            }
            out.extend(pts);
        }
        out
    };
    let mut out = Vec::new();
    fn walk(
        g: &Value,
        ring: &dyn Fn(&Value) -> Vec<(f64, f64)>,
        place: &dyn Fn((f64, f64)) -> (f64, f64),
        out: &mut Vec<Feature>,
        depth: usize,
    ) {
        if depth > 32 || out.len() >= MAX_FEATURES {
            return;
        }
        let props = g.get("properties").cloned().unwrap_or(Value::Null);
        let arcs = g.get("arcs").unwrap_or(&Value::Null);
        let mut parts = Vec::new();
        match g.get("type").and_then(Value::as_str).unwrap_or("") {
            "GeometryCollection" => {
                for inner in g
                    .get("geometries")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    walk(inner, ring, place, out, depth + 1);
                }
                return;
            }
            "Point" => parts.extend(
                pt(g.get("coordinates").unwrap_or(&Value::Null)).map(|p| Geometry::Point(place(p))),
            ),
            "MultiPoint" => parts.extend(
                line(g.get("coordinates").unwrap_or(&Value::Null))
                    .into_iter()
                    .map(|p| Geometry::Point(place(p))),
            ),
            "LineString" => parts.push(Geometry::Line(ring(arcs))),
            "MultiLineString" => parts.extend(
                arcs.as_array()
                    .into_iter()
                    .flatten()
                    .map(|l| Geometry::Line(ring(l))),
            ),
            "Polygon" => parts.push(Geometry::Polygon(
                arcs.as_array().into_iter().flatten().map(ring).collect(),
            )),
            "MultiPolygon" => parts.extend(arcs.as_array().into_iter().flatten().map(|p| {
                Geometry::Polygon(p.as_array().into_iter().flatten().map(ring).collect())
            })),
            _ => {}
        }
        out.push(Feature {
            parts,
            name: name_of(&props),
            props: props.to_string(),
        });
    }
    for obj in root
        .get("objects")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|o| o.values())
    {
        walk(obj, &ring, &place, &mut out, 0);
    }
    Ok(out)
}

/// KML `coordinates` text: `lon,lat[,alt]` tuples.
fn kml_coords(text: &str) -> Vec<(f64, f64)> {
    text.split_whitespace()
        .filter_map(|t| {
            let mut it = t.split(',');
            let x: f64 = it.next()?.parse().ok()?;
            let y: f64 = it.next()?.parse().ok()?;
            (x.is_finite() && y.is_finite()).then_some((x, y))
        })
        .collect()
}

fn xml(text: &str) -> Result<roxmltree::Document<'_>, String> {
    roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
    .map_err(|e| format!("Not readable XML: {e}."))
}

fn named<'a, 'i>(n: roxmltree::Node<'a, 'i>, tag: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == tag)
}

fn kml(text: &str) -> Result<Vec<Feature>, String> {
    let doc = xml(text)?;
    let mut out = Vec::new();
    for pm in doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "Placemark")
        .take(MAX_FEATURES)
    {
        let name = named(pm, "name")
            .and_then(|n| n.text())
            .map(|t| t.trim().to_string());
        let mut parts = Vec::new();
        for g in pm.descendants().filter(|n| n.is_element()) {
            match g.tag_name().name() {
                "Point" => {
                    if let Some(c) = named(g, "coordinates").and_then(|c| c.text()) {
                        parts.extend(kml_coords(c).into_iter().take(1).map(Geometry::Point));
                    }
                }
                "LineString" => {
                    if let Some(c) = named(g, "coordinates").and_then(|c| c.text()) {
                        parts.push(Geometry::Line(kml_coords(c)));
                    }
                }
                "Polygon" => {
                    let mut rings = Vec::new();
                    for boundary in ["outerBoundaryIs", "innerBoundaryIs"] {
                        for b in g
                            .children()
                            .filter(|c| c.is_element() && c.tag_name().name() == boundary)
                        {
                            if let Some(c) = b
                                .descendants()
                                .find(|c| c.tag_name().name() == "coordinates")
                                .and_then(|c| c.text())
                            {
                                rings.push(kml_coords(c));
                            }
                        }
                    }
                    parts.push(Geometry::Polygon(rings));
                }
                "Track" => {
                    let pts: Vec<(f64, f64)> = g
                        .children()
                        .filter(|c| c.is_element() && c.tag_name().name() == "coord")
                        .filter_map(|c| {
                            let mut it = c.text()?.split_whitespace();
                            Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
                        })
                        .collect();
                    parts.push(Geometry::Line(pts));
                }
                _ => {}
            }
        }
        let desc = named(pm, "description")
            .and_then(|n| n.text())
            .unwrap_or("");
        out.push(Feature {
            parts,
            props: format!("{}|{desc}", name.clone().unwrap_or_default()),
            name,
        });
    }
    Ok(out)
}

fn kmz(bytes: &[u8]) -> Result<Vec<Feature>, String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| format!("Not a readable KMZ file: {e}."))?;
    let name = (0..zip.len())
        .filter_map(|i| zip.by_index_raw(i).ok().map(|f| f.name().to_string()))
        .filter(|n| n.to_ascii_lowercase().ends_with(".kml"))
        .min_by_key(|n| (!n.eq_ignore_ascii_case("doc.kml"), n.len()))
        .ok_or("The KMZ file holds no KML document.")?;
    let mut file = zip.by_name(&name).map_err(|e| e.to_string())?;
    let mut text = String::new();
    file.by_ref()
        .take(crate::MAX_INPUT_BYTES as u64)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    kml(&text)
}

fn gpx(text: &str) -> Result<Vec<Feature>, String> {
    let doc = xml(text)?;
    if doc.root_element().tag_name().name() != "gpx" {
        return Err("Not a GPX file.".into());
    }
    let latlon = |n: roxmltree::Node<'_, '_>| -> Option<(f64, f64)> {
        Some((
            n.attribute("lon")?.parse().ok()?,
            n.attribute("lat")?.parse().ok()?,
        ))
    };
    let name = |n: roxmltree::Node<'_, '_>| {
        n.children()
            .find(|c| c.is_element() && c.tag_name().name() == "name")
            .and_then(|c| c.text())
            .map(|t| t.trim().to_string())
    };
    let mut out = Vec::new();
    for n in doc.root_element().children().filter(|c| c.is_element()) {
        match n.tag_name().name() {
            "wpt" => {
                if let Some(p) = latlon(n) {
                    out.push(Feature {
                        parts: vec![Geometry::Point(p)],
                        props: name(n).unwrap_or_default(),
                        name: name(n),
                    });
                }
            }
            "rte" => {
                let pts: Vec<(f64, f64)> = n
                    .children()
                    .filter(|c| c.is_element() && c.tag_name().name() == "rtept")
                    .filter_map(latlon)
                    .collect();
                out.push(Feature {
                    parts: vec![Geometry::Line(pts)],
                    props: name(n).unwrap_or_default(),
                    name: name(n),
                });
            }
            "trk" => {
                let parts: Vec<Geometry> = n
                    .children()
                    .filter(|c| c.is_element() && c.tag_name().name() == "trkseg")
                    .map(|seg| {
                        Geometry::Line(
                            seg.children()
                                .filter(|c| c.is_element() && c.tag_name().name() == "trkpt")
                                .filter_map(latlon)
                                .collect(),
                        )
                    })
                    .collect();
                out.push(Feature {
                    parts,
                    props: name(n).unwrap_or_default(),
                    name: name(n),
                });
            }
            _ => {}
        }
        if out.len() >= MAX_FEATURES {
            break;
        }
    }
    Ok(out)
}

fn le_i32(b: &[u8], at: usize) -> Option<i32> {
    b.get(at..at + 4)
        .map(|s| i32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn le_f64(b: &[u8], at: usize) -> Option<f64> {
    b.get(at..at + 8)
        .and_then(|s| s.try_into().ok())
        .map(f64::from_le_bytes)
}

fn shapefile(bytes: &[u8]) -> Result<Vec<Feature>, String> {
    if bytes.len() < 100 || bytes[..4] != [0, 0, 0x27, 0x0a] {
        return Err("Not a shapefile.".into());
    }
    let mut out = Vec::new();
    let mut pos = 100;
    let mut record = 0;
    while pos + 12 <= bytes.len() && out.len() < MAX_FEATURES {
        let len = u32::from_be_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize
            * 2;
        let start = pos + 8;
        let end = start
            .checked_add(len)
            .filter(|e| *e <= bytes.len())
            .ok_or("A shapefile record runs past the end.")?;
        let rec = &bytes[start..end];
        pos = end;
        record += 1;
        let ty = le_i32(rec, 0).unwrap_or(0);
        let mut parts = Vec::new();
        match ty {
            1 | 11 | 21 => {
                if let (Some(x), Some(y)) = (le_f64(rec, 4), le_f64(rec, 12)) {
                    parts.push(Geometry::Point((x, y)));
                }
            }
            8 | 18 | 28 => {
                let n = le_i32(rec, 36).unwrap_or(0).max(0) as usize;
                for i in 0..n.min(rec.len() / 16) {
                    if let (Some(x), Some(y)) = (le_f64(rec, 40 + i * 16), le_f64(rec, 48 + i * 16))
                    {
                        parts.push(Geometry::Point((x, y)));
                    }
                }
            }
            3 | 13 | 23 | 5 | 15 | 25 => {
                let nparts = le_i32(rec, 36).unwrap_or(0).max(0) as usize;
                let npoints = le_i32(rec, 40).unwrap_or(0).max(0) as usize;
                if nparts > rec.len() / 4 || npoints > rec.len() / 16 {
                    continue;
                }
                let starts: Vec<usize> = (0..nparts)
                    .filter_map(|i| le_i32(rec, 44 + i * 4))
                    .map(|s| s.max(0) as usize)
                    .collect();
                let base = 44 + nparts * 4;
                let pts: Vec<(f64, f64)> = (0..npoints)
                    .filter_map(|i| {
                        Some((le_f64(rec, base + i * 16)?, le_f64(rec, base + i * 16 + 8)?))
                    })
                    .collect();
                let mut rings = Vec::new();
                for (k, &s) in starts.iter().enumerate() {
                    let e = starts
                        .get(k + 1)
                        .copied()
                        .unwrap_or(pts.len())
                        .min(pts.len());
                    if s < e {
                        rings.push(pts[s..e].to_vec());
                    }
                }
                if matches!(ty, 5 | 15 | 25) {
                    parts.push(Geometry::Polygon(rings));
                } else {
                    parts.extend(rings.into_iter().map(Geometry::Line));
                }
            }
            _ => {}
        }
        out.push(Feature {
            parts,
            name: None,
            props: format!("#{record}"),
        });
    }
    Ok(out)
}

/// Well-known binary geometry, from `at`; returns the end.
fn wkb(b: &[u8], at: usize, out: &mut Vec<Geometry>, depth: usize) -> Option<usize> {
    if depth > 32 {
        return None;
    }
    let le = *b.get(at)? == 1;
    let u32_at = |p: usize| -> Option<u32> {
        let s: [u8; 4] = b.get(p..p + 4)?.try_into().ok()?;
        Some(if le {
            u32::from_le_bytes(s)
        } else {
            u32::from_be_bytes(s)
        })
    };
    let f64_at = |p: usize| -> Option<f64> {
        let s: [u8; 8] = b.get(p..p + 8)?.try_into().ok()?;
        Some(if le {
            f64::from_le_bytes(s)
        } else {
            f64::from_be_bytes(s)
        })
    };
    let raw = u32_at(at + 1)?;
    // ISO (1000s) and EWKB (high bits) dimensions
    let mut dims = 2;
    let mut ty = raw & 0x0fff_ffff;
    if raw & 0x8000_0000 != 0 {
        dims += 1;
    }
    if raw & 0x4000_0000 != 0 {
        dims += 1;
    }
    let mut pos = at + 5;
    if raw & 0x2000_0000 != 0 {
        pos += 4; // SRID
    }
    match ty / 1000 {
        1 | 2 => dims = 3,
        3 => dims = 4,
        _ => {}
    }
    ty %= 1000;
    let point = |p: usize| -> Option<(f64, f64)> { Some((f64_at(p)?, f64_at(p + 8)?)) };
    let points = |pos: &mut usize| -> Option<Vec<(f64, f64)>> {
        let n = u32_at(*pos)? as usize;
        if n > b.len() / 16 {
            return None;
        }
        *pos += 4;
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(point(*pos)?);
            *pos += 8 * dims;
        }
        Some(v)
    };
    match ty {
        1 => {
            let p = point(pos)?;
            if p.0.is_finite() && p.1.is_finite() {
                out.push(Geometry::Point(p));
            }
            pos += 8 * dims;
        }
        2 => out.push(Geometry::Line(points(&mut pos)?)),
        3 => {
            let n = u32_at(pos)? as usize;
            pos += 4;
            let mut rings = Vec::new();
            for _ in 0..n.min(b.len() / 4) {
                rings.push(points(&mut pos)?);
            }
            out.push(Geometry::Polygon(rings));
        }
        4..=7 => {
            let n = u32_at(pos)? as usize;
            pos += 4;
            for _ in 0..n.min(b.len() / 5) {
                pos = wkb(b, pos, out, depth + 1)?;
            }
        }
        _ => return None,
    }
    Some(pos)
}

fn geopackage(bytes: &[u8]) -> Result<Vec<Feature>, String> {
    if !bytes.starts_with(b"SQLite format 3\0") {
        return Err("Not a GeoPackage.".into());
    }
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let path = dir.path().join("map.gpkg");
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    let db =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(|e| format!("Not a readable GeoPackage: {e}."))?;
    let layers: Vec<(String, String)> = db
        .prepare("SELECT table_name, column_name FROM gpkg_geometry_columns ORDER BY table_name")
        .and_then(|mut st| {
            st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map(|rows| rows.filter_map(Result::ok).collect())
        })
        .map_err(|e| format!("Not a GeoPackage (no geometry columns): {e}."))?;
    let quote = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    let mut out = Vec::new();
    for (table, column) in layers {
        let Ok(mut st) = db.prepare(&format!("SELECT * FROM {}", quote(&table))) else {
            continue;
        };
        let names: Vec<String> = st.column_names().iter().map(|s| s.to_string()).collect();
        let geom_ix = names.iter().position(|n| n.eq_ignore_ascii_case(&column));
        let name_ix = names
            .iter()
            .position(|n| matches!(n.to_ascii_lowercase().as_str(), "name" | "title" | "label"));
        let Some(geom_ix) = geom_ix else { continue };
        let Ok(mut rows) = st.query([]) else { continue };
        while let Ok(Some(row)) = rows.next() {
            if out.len() >= MAX_FEATURES {
                break;
            }
            let blob: Vec<u8> = row
                .get::<_, Option<Vec<u8>>>(geom_ix)
                .ok()
                .flatten()
                .unwrap_or_default();
            let mut parts = Vec::new();
            if blob.len() >= 8 && &blob[..2] == b"GP" {
                let flags = blob[3];
                let envelope = match (flags >> 1) & 0x07 {
                    1 => 32,
                    2 | 3 => 48,
                    4 => 64,
                    _ => 0,
                };
                let _ = wkb(&blob, 8 + envelope, &mut parts, 0);
            }
            let mut props = Vec::new();
            for (i, n) in names.iter().enumerate() {
                if i == geom_ix {
                    continue;
                }
                let v: rusqlite::types::Value = row.get(i).unwrap_or(rusqlite::types::Value::Null);
                props.push(format!("{n}={v:?}"));
            }
            let name = name_ix.and_then(|i| row.get::<_, Option<String>>(i).ok().flatten());
            out.push(Feature {
                parts,
                name,
                props: format!("{table}|{}", props.join("|")),
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kml_and_gpx() {
        let kml_text = r#"<?xml version="1.0"?><kml xmlns="http://www.opengis.net/kml/2.2"><Document>
<Placemark><name>Pin</name><Point><coordinates>-122.08,37.42,0</coordinates></Point></Placemark>
<Placemark><Polygon><outerBoundaryIs><LinearRing><coordinates>0,0 1,0 1,1 0,0</coordinates></LinearRing></outerBoundaryIs></Polygon></Placemark>
</Document></kml>"#;
        let f = kml(kml_text).unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].name.as_deref(), Some("Pin"));
        assert_eq!(f[0].parts, vec![Geometry::Point((-122.08, 37.42))]);
        assert!(matches!(&f[1].parts[0], Geometry::Polygon(r) if r[0].len() == 4));
        let gpx_text = r#"<gpx version="1.1"><wpt lat="1" lon="2"><name>W</name></wpt><trk><trkseg><trkpt lat="1" lon="2"/><trkpt lat="3" lon="4"/></trkseg></trk></gpx>"#;
        let f = gpx(gpx_text).unwrap();
        assert_eq!(f[0].parts, vec![Geometry::Point((2.0, 1.0))]);
        assert_eq!(
            f[1].parts,
            vec![Geometry::Line(vec![(2.0, 1.0), (4.0, 3.0)])]
        );
    }

    #[test]
    fn topojson_arcs() {
        let topo = br#"{"type":"Topology","transform":{"scale":[1,1],"translate":[0,0]},
            "objects":{"shapes":{"type":"GeometryCollection","geometries":[
              {"type":"Polygon","arcs":[[0,1]],"properties":{"name":"sq"}},
              {"type":"Point","coordinates":[5,5]}]}},
            "arcs":[[[0,0],[1,0],[0,1]],[[1,1],[-1,0],[0,-1]]]}"#;
        let f = topojson(topo).unwrap();
        assert_eq!(f[0].name.as_deref(), Some("sq"));
        assert_eq!(
            f[0].parts,
            vec![Geometry::Polygon(vec![vec![
                (0.0, 0.0),
                (1.0, 0.0),
                (1.0, 1.0),
                (0.0, 1.0),
                (0.0, 0.0)
            ]])]
        );
        assert_eq!(f[1].parts, vec![Geometry::Point((5.0, 5.0))]);
    }

    #[test]
    fn shapefile_points_and_polygons() {
        let mut shp = vec![0u8; 100];
        shp[..4].copy_from_slice(&[0, 0, 0x27, 0x0a]);
        // a point record
        let mut rec = Vec::new();
        rec.extend(1i32.to_le_bytes());
        rec.extend(3.5f64.to_le_bytes());
        rec.extend(4.5f64.to_le_bytes());
        shp.extend(1u32.to_be_bytes());
        shp.extend(((rec.len() / 2) as u32).to_be_bytes());
        shp.extend(&rec);
        // a polygon record with one ring
        let mut rec = Vec::new();
        rec.extend(5i32.to_le_bytes());
        rec.extend([0u8; 32]);
        rec.extend(1i32.to_le_bytes());
        rec.extend(4i32.to_le_bytes());
        rec.extend(0i32.to_le_bytes());
        for (x, y) in [(0.0f64, 0.0f64), (0.0, 1.0), (1.0, 1.0), (0.0, 0.0)] {
            rec.extend(x.to_le_bytes());
            rec.extend(y.to_le_bytes());
        }
        shp.extend(2u32.to_be_bytes());
        shp.extend(((rec.len() / 2) as u32).to_be_bytes());
        shp.extend(&rec);
        let f = shapefile(&shp).unwrap();
        assert_eq!(f[0].parts, vec![Geometry::Point((3.5, 4.5))]);
        assert!(matches!(&f[1].parts[0], Geometry::Polygon(r) if r[0].len() == 4));
    }

    #[test]
    fn wkb_geometries() {
        // little-endian LineString with two points
        let mut b = vec![1u8];
        b.extend(2u32.to_le_bytes());
        b.extend(2u32.to_le_bytes());
        for v in [0.0f64, 0.0, 1.0, 2.0] {
            b.extend(v.to_le_bytes());
        }
        let mut out = Vec::new();
        assert_eq!(wkb(&b, 0, &mut out, 0), Some(b.len()));
        assert_eq!(out, vec![Geometry::Line(vec![(0.0, 0.0), (1.0, 2.0)])]);
    }

    #[test]
    fn geopackage_layers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.gpkg");
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE gpkg_geometry_columns (table_name TEXT, column_name TEXT);
             INSERT INTO gpkg_geometry_columns VALUES ('places', 'geom');
             CREATE TABLE places (fid INTEGER PRIMARY KEY, name TEXT, geom BLOB);",
        )
        .unwrap();
        let mut blob = b"GP\0\x01".to_vec();
        blob.extend(4326i32.to_le_bytes());
        blob.push(1);
        blob.extend(1u32.to_le_bytes());
        blob.extend(7.0f64.to_le_bytes());
        blob.extend(8.0f64.to_le_bytes());
        db.execute(
            "INSERT INTO places (name, geom) VALUES ('Here', ?1)",
            [blob],
        )
        .unwrap();
        drop(db);
        let bytes = std::fs::read(&path).unwrap();
        let f = geopackage(&bytes).unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].name.as_deref(), Some("Here"));
        assert_eq!(f[0].parts, vec![Geometry::Point((7.0, 8.0))]);
    }
}
