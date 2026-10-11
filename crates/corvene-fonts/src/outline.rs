//! Glyph outlines as plain path commands in font units (y up), shared by
//! compiled fonts (drawn through skrifa) and font sources (`.glif`,
//! `.glyphs`), so both are hashed, measured and rasterised the same way.

use skrifa::outline::OutlinePen;

/// One path command, in font units with y pointing up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathCmd {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    /// Control point, end point.
    QuadTo(f32, f32, f32, f32),
    /// Two control points, end point.
    CubicTo(f32, f32, f32, f32, f32, f32),
    Close,
}

/// A point of an outline as the inspector marks it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointMark {
    pub x: f32,
    pub y: f32,
    /// On-curve (a contour's nodes) rather than a control point.
    pub on_curve: bool,
}

/// A glyph's outline.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlyphPath {
    pub cmds: Vec<PathCmd>,
}

/// Bounding box in font units: x min, y min, x max, y max.
pub type Bounds = [f32; 4];

impl GlyphPath {
    pub fn is_empty(&self) -> bool {
        self.cmds.is_empty()
    }

    /// Number of contours (sub-paths).
    pub fn contours(&self) -> u32 {
        self.cmds
            .iter()
            .filter(|c| matches!(c, PathCmd::MoveTo(..)))
            .count() as u32
    }

    /// Number of points, on- and off-curve, the way a font editor counts
    /// them (a closing point that repeats the start is not counted twice).
    pub fn point_count(&self) -> u32 {
        self.points().len() as u32
    }

    /// Every point with its role, for the inspector.
    pub fn points(&self) -> Vec<PointMark> {
        let mut out = Vec::new();
        let mut start: Option<(f32, f32)> = None;
        let on = |x, y| PointMark {
            x,
            y,
            on_curve: true,
        };
        let off = |x, y| PointMark {
            x,
            y,
            on_curve: false,
        };
        for (i, cmd) in self.cmds.iter().enumerate() {
            // the end of a contour's last segment usually repeats its start
            let closes_on = |x: f32, y: f32| {
                start.is_some_and(|(sx, sy)| sx == x && sy == y)
                    && matches!(self.cmds.get(i + 1), Some(PathCmd::Close) | None)
            };
            match *cmd {
                PathCmd::MoveTo(x, y) => {
                    start = Some((x, y));
                    out.push(on(x, y));
                }
                PathCmd::LineTo(x, y) => {
                    if !closes_on(x, y) {
                        out.push(on(x, y));
                    }
                }
                PathCmd::QuadTo(cx, cy, x, y) => {
                    out.push(off(cx, cy));
                    if !closes_on(x, y) {
                        out.push(on(x, y));
                    }
                }
                PathCmd::CubicTo(c1x, c1y, c2x, c2y, x, y) => {
                    out.push(off(c1x, c1y));
                    out.push(off(c2x, c2y));
                    if !closes_on(x, y) {
                        out.push(on(x, y));
                    }
                }
                PathCmd::Close => start = None,
            }
        }
        out
    }

    /// Control-point handles: (on-curve, off-curve) pairs to draw as lines.
    pub fn handles(&self) -> Vec<[(f32, f32); 2]> {
        let mut out = Vec::new();
        let mut last = (0.0, 0.0);
        for cmd in &self.cmds {
            match *cmd {
                PathCmd::MoveTo(x, y) | PathCmd::LineTo(x, y) => last = (x, y),
                PathCmd::QuadTo(cx, cy, x, y) => {
                    out.push([last, (cx, cy)]);
                    out.push([(x, y), (cx, cy)]);
                    last = (x, y);
                }
                PathCmd::CubicTo(c1x, c1y, c2x, c2y, x, y) => {
                    out.push([last, (c1x, c1y)]);
                    out.push([(x, y), (c2x, c2y)]);
                    last = (x, y);
                }
                PathCmd::Close => {}
            }
        }
        out
    }

    /// Bounds of the points (control points included, as `glyf` stores
    /// them); `None` for an empty glyph.
    pub fn bounds(&self) -> Option<Bounds> {
        let mut b: Option<Bounds> = None;
        let mut add = |x: f32, y: f32| {
            let r = b.get_or_insert([x, y, x, y]);
            r[0] = r[0].min(x);
            r[1] = r[1].min(y);
            r[2] = r[2].max(x);
            r[3] = r[3].max(y);
        };
        for cmd in &self.cmds {
            match *cmd {
                PathCmd::MoveTo(x, y) | PathCmd::LineTo(x, y) => add(x, y),
                PathCmd::QuadTo(a, b2, x, y) => {
                    add(a, b2);
                    add(x, y);
                }
                PathCmd::CubicTo(a, b2, c, d, x, y) => {
                    add(a, b2);
                    add(c, d);
                    add(x, y);
                }
                PathCmd::Close => {}
            }
        }
        b
    }

    /// A stable hash of the outline, coordinates rounded to 1/16 unit so a
    /// round trip through another tool does not count as a change.
    pub fn hash(&self) -> u64 {
        let mut h = Fnv::new();
        let q = |v: f32| (v * 16.0).round() as i64;
        for cmd in &self.cmds {
            match *cmd {
                PathCmd::MoveTo(x, y) => {
                    h.write(&[1]);
                    h.int(q(x));
                    h.int(q(y));
                }
                PathCmd::LineTo(x, y) => {
                    h.write(&[2]);
                    h.int(q(x));
                    h.int(q(y));
                }
                PathCmd::QuadTo(a, b, x, y) => {
                    h.write(&[3]);
                    for v in [a, b, x, y] {
                        h.int(q(v));
                    }
                }
                PathCmd::CubicTo(a, b, c, d, x, y) => {
                    h.write(&[4]);
                    for v in [a, b, c, d, x, y] {
                        h.int(q(v));
                    }
                }
                PathCmd::Close => h.write(&[5]),
            }
        }
        h.finish()
    }

    /// The outline moved by `(dx, dy)` and scaled by `(sx, sy)` about the
    /// origin (component placement in sources).
    pub fn transformed(&self, m: [f32; 6]) -> GlyphPath {
        let [a, b, c, d, e, f] = m;
        let t = |x: f32, y: f32| (a * x + c * y + e, b * x + d * y + f);
        let cmds = self
            .cmds
            .iter()
            .map(|cmd| match *cmd {
                PathCmd::MoveTo(x, y) => {
                    let (x, y) = t(x, y);
                    PathCmd::MoveTo(x, y)
                }
                PathCmd::LineTo(x, y) => {
                    let (x, y) = t(x, y);
                    PathCmd::LineTo(x, y)
                }
                PathCmd::QuadTo(p, q, x, y) => {
                    let (p, q) = t(p, q);
                    let (x, y) = t(x, y);
                    PathCmd::QuadTo(p, q, x, y)
                }
                PathCmd::CubicTo(p, q, r, s, x, y) => {
                    let (p, q) = t(p, q);
                    let (r, s) = t(r, s);
                    let (x, y) = t(x, y);
                    PathCmd::CubicTo(p, q, r, s, x, y)
                }
                PathCmd::Close => PathCmd::Close,
            })
            .collect();
        GlyphPath { cmds }
    }
}

/// Collects skrifa's drawing calls.
impl OutlinePen for GlyphPath {
    fn move_to(&mut self, x: f32, y: f32) {
        self.cmds.push(PathCmd::MoveTo(x, y));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.cmds.push(PathCmd::LineTo(x, y));
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.cmds.push(PathCmd::QuadTo(cx0, cy0, x, y));
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.cmds.push(PathCmd::CubicTo(cx0, cy0, cx1, cy1, x, y));
    }
    fn close(&mut self) {
        self.cmds.push(PathCmd::Close);
    }
}

/// FNV-1a: stable across runs (std's hasher is randomly seeded).
#[derive(Clone, Copy)]
pub(crate) struct Fnv(u64);

impl Fnv {
    pub(crate) fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
    pub(crate) fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
    pub(crate) fn int(&mut self, v: i64) {
        self.write(&v.to_le_bytes());
    }
    pub(crate) fn finish(self) -> u64 {
        self.0
    }
}

/// FNV-1a of a byte slice.
pub(crate) fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = Fnv::new();
    h.write(bytes);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> GlyphPath {
        let mut p = GlyphPath::default();
        p.move_to(0.0, 0.0);
        p.line_to(100.0, 0.0);
        p.quad_to(150.0, 50.0, 100.0, 100.0);
        p.line_to(0.0, 100.0);
        p.line_to(0.0, 0.0);
        p.close();
        p
    }

    #[test]
    fn counts_points_without_the_repeated_start() {
        let p = square();
        assert_eq!(p.contours(), 1);
        // 4 nodes + 1 control point
        assert_eq!(p.point_count(), 5);
        assert_eq!(p.points().iter().filter(|p| !p.on_curve).count(), 1);
        assert_eq!(p.handles().len(), 2);
    }

    #[test]
    fn bounds_include_control_points() {
        assert_eq!(square().bounds(), Some([0.0, 0.0, 150.0, 100.0]));
        assert_eq!(GlyphPath::default().bounds(), None);
    }

    #[test]
    fn hash_ignores_sub_sixteenth_noise() {
        let a = square();
        let mut b = square();
        if let PathCmd::LineTo(x, _) = &mut b.cmds[1] {
            *x += 0.01;
        }
        assert_eq!(a.hash(), b.hash());
        if let PathCmd::LineTo(x, _) = &mut b.cmds[1] {
            *x += 1.0;
        }
        assert_ne!(a.hash(), b.hash());
    }
}
