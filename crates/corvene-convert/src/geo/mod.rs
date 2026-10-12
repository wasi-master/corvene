//! Corvene `1354-map-diff`: geographic files drawn as maps, both sides on
//! the same projection and bounds so the image diff's swipe and onion
//! skin line them up. Features found on only one side are colored (added
//! green on the new side, removed red on the old one); the others are
//! drawn neutral. Longitude/latitude data is drawn in Web Mercator with a
//! graticule; projected data as it is.
//!
//! Formats: GeoJSON, TopoJSON, KML and KMZ, GPX, shapefiles (`.shp`, the
//! geometry only: the `.dbf` beside it is a table diff of its own) and
//! GeoPackages. No base map is drawn (nothing is downloaded).

mod read;

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use crate::Format;
use crate::diagram::svg::{Anchor, Style, Svg, n};

/// Points kept per side after simplification; beyond it the map is
/// refused (a side would take too long to draw).
const MAX_POINTS: usize = 2_000_000;
/// Width the map is drawn at.
const WIDTH: f64 = 1200.0;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Geometry {
    Point((f64, f64)),
    Line(Vec<(f64, f64)>),
    /// Rings: the outer one first, holes after.
    Polygon(Vec<Vec<(f64, f64)>>),
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Feature {
    pub parts: Vec<Geometry>,
    /// A name to label points with (`name`, `title`, `label` properties).
    pub name: Option<String>,
    /// The feature's properties as text, so a changed property changes
    /// its identity.
    pub props: String,
}

impl Feature {
    fn key(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.props.hash(&mut h);
        for part in &self.parts {
            match part {
                Geometry::Point(p) => hash_pt(&mut h, *p),
                Geometry::Line(l) => l.iter().for_each(|p| hash_pt(&mut h, *p)),
                Geometry::Polygon(rings) => {
                    rings.iter().flatten().for_each(|p| hash_pt(&mut h, *p))
                }
            }
        }
        h.finish()
    }

    fn points(&self) -> impl Iterator<Item = &(f64, f64)> {
        self.parts
            .iter()
            .flat_map(|g| -> Box<dyn Iterator<Item = &(f64, f64)>> {
                match g {
                    Geometry::Point(p) => Box::new(std::iter::once(p)),
                    Geometry::Line(l) => Box::new(l.iter()),
                    Geometry::Polygon(r) => Box::new(r.iter().flatten()),
                }
            })
    }
}

fn hash_pt(h: &mut impl Hasher, (x, y): (f64, f64)) {
    // rounded so float noise from rewriting a file does not count
    ((x * 1e7).round() as i64).hash(h);
    ((y * 1e7).round() as i64).hash(h);
}

/// Both sides' maps.
#[allow(clippy::type_complexity)]
pub fn to_svg_pair(
    format: Format,
    old: Option<&[u8]>,
    new: Option<&[u8]>,
) -> Result<(Option<Vec<u8>>, Option<Vec<u8>>), String> {
    let old_f = old.map(|b| read::features(format, b)).transpose()?;
    let new_f = new.map(|b| read::features(format, b)).transpose()?;
    let all = old_f.iter().chain(new_f.iter()).flatten();
    let total: usize = all.clone().map(|f| f.points().count()).sum();
    if total == 0 {
        return Err("The file has no geometry.".into());
    }
    if total > MAX_POINTS * 2 {
        return Err("The map has too many points to draw.".into());
    }
    let geographic = all
        .clone()
        .flat_map(Feature::points)
        .all(|(x, y)| (-180.0..=180.0).contains(x) && (-90.0..=90.0).contains(y));
    let project = move |(x, y): (f64, f64)| -> (f64, f64) {
        if geographic {
            let lat = y.clamp(-85.05, 85.05).to_radians();
            (
                x,
                -(std::f64::consts::FRAC_PI_4 + lat / 2.0)
                    .tan()
                    .ln()
                    .to_degrees(),
            )
        } else {
            (x, -y)
        }
    };
    let (mut x0, mut y0, mut x1, mut y1) = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    for p in all.clone().flat_map(Feature::points) {
        let (x, y) = project(*p);
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    let span = (x1 - x0).max(y1 - y0).max(1e-9);
    // a lone point (or a tiny extent) gets some room around it
    let pad = if (x1 - x0) < 1e-9 && (y1 - y0) < 1e-9 {
        1.0
    } else {
        span * 0.04
    };
    let (x0, y0, x1, y1) = (x0 - pad, y0 - pad, x1 + pad, y1 + pad);
    let scale = WIDTH / (x1 - x0).max((y1 - y0) * 0.75).max(1e-12);
    let view = View {
        x0,
        y0,
        x1,
        y1,
        scale,
        geographic,
    };
    let count = |fs: &Option<Vec<Feature>>| -> HashMap<u64, usize> {
        let mut m = HashMap::new();
        for f in fs.iter().flatten() {
            *m.entry(f.key()).or_insert(0) += 1;
        }
        m
    };
    let (old_keys, new_keys) = (count(&old_f), count(&new_f));
    let both = old.is_some() && new.is_some();
    let draw = |fs: &Vec<Feature>, other: &HashMap<u64, usize>, changed: Status| {
        let mut left = other.clone();
        let statuses: Vec<Status> = fs
            .iter()
            .map(|f| {
                if !both {
                    return Status::Same;
                }
                match left.get_mut(&f.key()) {
                    Some(c) if *c > 0 => {
                        *c -= 1;
                        Status::Same
                    }
                    _ => changed,
                }
            })
            .collect();
        render(fs, &statuses, &view, &project)
    };
    Ok((
        old_f
            .as_ref()
            .map(|fs| draw(fs, &new_keys, Status::Removed)),
        new_f.as_ref().map(|fs| draw(fs, &old_keys, Status::Added)),
    ))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Same,
    Added,
    Removed,
}

struct View {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    scale: f64,
    geographic: bool,
}

impl View {
    fn px(&self, (x, y): (f64, f64)) -> (f64, f64) {
        ((x - self.x0) * self.scale, (y - self.y0) * self.scale)
    }
}

/// Drops points closer than `tolerance` pixels to the line through their
/// neighbors (Douglas-Peucker), after dropping near-duplicates.
fn simplify(points: &[(f64, f64)], tolerance: f64) -> Vec<(f64, f64)> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let mut thinned: Vec<(f64, f64)> = Vec::with_capacity(points.len());
    for p in points {
        if thinned
            .last()
            .is_none_or(|q: &(f64, f64)| (p.0 - q.0).abs() + (p.1 - q.1).abs() > tolerance * 0.5)
        {
            thinned.push(*p);
        }
    }
    if let Some(last) = points.last()
        && thinned.last() != Some(last)
        && thinned.len() > 1
    {
        thinned.push(*last);
    }
    if thinned.len() <= 2 {
        return thinned;
    }
    let mut keep = vec![false; thinned.len()];
    keep[0] = true;
    keep[thinned.len() - 1] = true;
    let mut stack = vec![(0, thinned.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (ax, ay) = thinned[a];
        let (bx, by) = thinned[b];
        let (dx, dy) = (bx - ax, by - ay);
        let len = (dx * dx + dy * dy).sqrt();
        let mut best = (0.0, a);
        for (i, &(px, py)) in thinned.iter().enumerate().take(b).skip(a + 1) {
            let d = if len < 1e-12 {
                ((px - ax).powi(2) + (py - ay).powi(2)).sqrt()
            } else {
                ((px - ax) * dy - (py - ay) * dx).abs() / len
            };
            if d > best.0 {
                best = (d, i);
            }
        }
        if best.0 > tolerance {
            keep[best.1] = true;
            stack.push((a, best.1));
            stack.push((best.1, b));
        }
    }
    thinned
        .into_iter()
        .zip(keep)
        .filter(|(_, k)| *k)
        .map(|(p, _)| p)
        .collect()
}

fn path_of(points: &[(f64, f64)], close: bool) -> String {
    let mut d = String::with_capacity(points.len() * 12);
    for (ix, (x, y)) in points.iter().enumerate() {
        d.push(if ix == 0 { 'M' } else { 'L' });
        d.push_str(&n(*x));
        d.push(',');
        d.push_str(&n(*y));
    }
    if close {
        d.push('Z');
    }
    d
}

fn render(
    features: &[Feature],
    statuses: &[Status],
    view: &View,
    project: &impl Fn((f64, f64)) -> (f64, f64),
) -> Vec<u8> {
    let mut svg = Svg::new();
    svg.set_background(Some("#ffffff"));
    let (w, h) = (
        (view.x1 - view.x0) * view.scale,
        (view.y1 - view.y0) * view.scale,
    );
    svg.include(0.0, 0.0);
    svg.include(w, h);
    if view.geographic {
        graticule(&mut svg, view, project);
    }
    let colors = |s: Status| match s {
        Status::Same => ("#6e7781", "#d0d7de"),
        Status::Added => ("#1a7f37", "#aceebb"),
        Status::Removed => ("#cf222e", "#ffcecb"),
    };
    // unchanged features first, so changes draw on top
    let mut order: Vec<usize> = (0..features.len()).collect();
    order.sort_by_key(|&i| statuses[i] != Status::Same);
    let mut labels = 0;
    let (mut added, mut removed) = (0, 0);
    for &i in &order {
        let f = &features[i];
        let (stroke, fill) = colors(statuses[i]);
        match statuses[i] {
            Status::Added => added += 1,
            Status::Removed => removed += 1,
            Status::Same => {}
        }
        for part in &f.parts {
            match part {
                Geometry::Point(p) => {
                    let (x, y) = view.px(project(*p));
                    svg.ellipse(x, y, 4.5, 4.5, &Style::new(stroke, "#ffffff", 1.5));
                    if labels < 300
                        && let Some(name) = &f.name
                    {
                        svg.text(x + 8.0, y, name, 12.0, Anchor::Start, "#24292f", None);
                        labels += 1;
                    }
                }
                Geometry::Line(l) => {
                    let pts: Vec<(f64, f64)> = l.iter().map(|p| view.px(project(*p))).collect();
                    let pts = simplify(&pts, 0.5);
                    if pts.len() >= 2 {
                        svg.raw(&format!(
                            r#"<path d="{}" fill="none" stroke="{stroke}" stroke-width="2" stroke-linejoin="round" stroke-linecap="round"/>"#,
                            path_of(&pts, false)
                        ));
                    }
                }
                Geometry::Polygon(rings) => {
                    let mut d = String::new();
                    for ring in rings {
                        let pts: Vec<(f64, f64)> =
                            ring.iter().map(|p| view.px(project(*p))).collect();
                        let pts = simplify(&pts, 0.5);
                        if pts.len() >= 3 {
                            d.push_str(&path_of(&pts, true));
                        }
                    }
                    if !d.is_empty() {
                        svg.raw(&format!(
                            r#"<path d="{d}" fill="{fill}" fill-opacity="0.6" fill-rule="evenodd" stroke="{stroke}" stroke-width="1.2" stroke-linejoin="round"/>"#
                        ));
                    }
                }
            }
        }
    }
    let mut caption = format!("{} features", features.len());
    if added > 0 {
        caption.push_str(&format!(", {added} added"));
    }
    if removed > 0 {
        caption.push_str(&format!(", {removed} removed"));
    }
    svg.text(8.0, -14.0, &caption, 13.0, Anchor::Start, "#57606a", None);
    // fixed bounds, so both sides' pictures line up (labels past the
    // right edge get a margin)
    svg.finish_in((-12.0, -32.0, w + 160.0, h + 12.0))
}

/// Meridians and parallels every 1, 2, 5, 10, 15 or 30 degrees.
fn graticule(svg: &mut Svg, view: &View, project: &impl Fn((f64, f64)) -> (f64, f64)) {
    // the extent back in degrees
    let inv_y =
        |y: f64| (2.0 * (-y).to_radians().exp().atan() - std::f64::consts::FRAC_PI_2).to_degrees();
    let (lon0, lon1) = (view.x0.max(-180.0), view.x1.min(180.0));
    let (lat0, lat1) = (inv_y(view.y1), inv_y(view.y0));
    let span = (lon1 - lon0).max(lat1 - lat0);
    let step = [
        0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0,
    ]
    .into_iter()
    .find(|s| span / s <= 10.0)
    .unwrap_or(30.0);
    let style = Style::line("#eaeef2", 1.0);
    let mut lon = (lon0 / step).ceil() * step;
    while lon <= lon1 {
        let a = view.px(project((lon, lat0)));
        let b = view.px(project((lon, lat1)));
        svg.line(&[a, b], false, &style, None, None);
        lon += step;
    }
    let mut lat = (lat0 / step).ceil() * step;
    while lat <= lat1 {
        let a = view.px(project((lon0, lat)));
        let b = view.px(project((lon1, lat)));
        svg.line(&[a, b], false, &style, None, None);
        lat += step;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_are_colored() {
        let old = br#"{"type":"FeatureCollection","features":[
            {"type":"Feature","properties":{"name":"A"},"geometry":{"type":"Point","coordinates":[10,50]}},
            {"type":"Feature","properties":{"name":"B"},"geometry":{"type":"Point","coordinates":[11,51]}}]}"#;
        let new = br#"{"type":"FeatureCollection","features":[
            {"type":"Feature","properties":{"name":"A"},"geometry":{"type":"Point","coordinates":[10,50]}},
            {"type":"Feature","properties":{"name":"C"},"geometry":{"type":"Polygon","coordinates":[[[10,50],[12,50],[12,52],[10,50]]]}}]}"#;
        let (o, n) = to_svg_pair(Format::GeoJson, Some(old), Some(new)).unwrap();
        let o = String::from_utf8(o.unwrap()).unwrap();
        let n = String::from_utf8(n.unwrap()).unwrap();
        assert!(o.contains("1 removed"), "{o}");
        assert!(o.contains("#cf222e"));
        assert!(n.contains("1 added"), "{n}");
        assert!(n.contains("#aceebb"));
        // same bounds on both sides
        let vb = |s: &str| {
            s.split("viewBox=\"")
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap()
                .to_string()
        };
        assert_eq!(vb(&o), vb(&n));
    }

    #[test]
    fn simplify_drops_collinear_points() {
        let pts: Vec<(f64, f64)> = (0..100).map(|i| (i as f64, 0.0)).collect();
        assert_eq!(simplify(&pts, 0.5), vec![(0.0, 0.0), (99.0, 0.0)]);
        let corner = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)];
        assert_eq!(simplify(&corner, 0.5), corner);
    }
}
