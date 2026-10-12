//! Node-and-edge diagrams (Graphviz graphs, Mermaid flowcharts, state,
//! class and ER diagrams) as one model, laid out by [`super::layout`]
//! and drawn here.

use super::layout::{self, Clip, ClusterSpec, Dir, EdgeSpec, NodeSpec};
use super::svg::{Anchor, Style, Svg, colors, text_width};

pub const FONT_SIZE: f64 = 14.0;
const LINE_H: f64 = FONT_SIZE * 1.25;

#[derive(Clone, Debug, PartialEq, Default)]
pub enum Shape {
    #[default]
    Rect,
    Rounded,
    Stadium,
    Circle,
    DoubleCircle,
    Ellipse,
    Diamond,
    Hexagon,
    /// Leaning right (`/text/`), or left.
    Parallelogram(bool),
    /// Wide at the bottom (`[/text\]`), or at the top.
    Trapezoid(bool),
    Cylinder,
    Subroutine,
    Note,
    /// A small filled dot (Graphviz `point`, a state chart's start).
    Point,
    /// A state chart's end: a ringed dot.
    End,
    /// A fork or join bar.
    Bar,
    /// Text only.
    Plain,
    /// A box split in compartments (class members, record fields, an
    /// entity's attributes): the first holds the title.
    Compartments(Vec<Vec<String>>),
}

#[derive(Clone, Debug, Default)]
pub struct GNode {
    pub id: String,
    pub label: String,
    pub shape: Shape,
    pub fill: Option<String>,
    pub stroke: Option<String>,
    pub text: Option<String>,
    pub dashed: bool,
    pub cluster: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LineStyle {
    #[default]
    Solid,
    Dashed,
    Dotted,
    Thick,
    Invisible,
}

#[derive(Clone, Debug)]
pub struct GEdge {
    pub from: usize,
    pub to: usize,
    pub label: String,
    /// The marker at the target (`arrow`, `open`, `diamond`, …; see
    /// [`super::svg::Svg::path`]), and at the source.
    pub head: Option<&'static str>,
    pub tail: Option<&'static str>,
    pub style: LineStyle,
    pub color: Option<String>,
    pub minlen: usize,
}

impl GEdge {
    pub fn new(from: usize, to: usize) -> Self {
        GEdge {
            from,
            to,
            label: String::new(),
            head: Some("arrow"),
            tail: None,
            style: LineStyle::Solid,
            color: None,
            minlen: 1,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct GCluster {
    pub label: String,
    pub parent: Option<usize>,
    pub fill: Option<String>,
    pub stroke: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct GraphDiagram {
    pub nodes: Vec<GNode>,
    pub edges: Vec<GEdge>,
    pub clusters: Vec<GCluster>,
    pub dir: Dir,
    pub title: Option<String>,
    pub node_sep: f64,
    pub rank_sep: f64,
}

impl GraphDiagram {
    /// The index of the node with `id`, adding it (labeled with its id)
    /// if new.
    pub fn node(&mut self, id: &str) -> usize {
        if let Some(ix) = self.nodes.iter().position(|n| n.id == id) {
            return ix;
        }
        self.nodes.push(GNode {
            id: id.to_string(),
            label: id.to_string(),
            ..Default::default()
        });
        self.nodes.len() - 1
    }

    /// Like [`GraphDiagram::node`], with a map from ids for big graphs.
    pub fn node_indexed(
        &mut self,
        id: &str,
        index: &mut std::collections::HashMap<String, usize>,
    ) -> usize {
        if let Some(&ix) = index.get(id) {
            return ix;
        }
        self.nodes.push(GNode {
            id: id.to_string(),
            label: id.to_string(),
            ..Default::default()
        });
        let ix = self.nodes.len() - 1;
        index.insert(id.to_string(), ix);
        ix
    }
}

fn lines_width(text: &str) -> f64 {
    text.split('\n')
        .map(|l| text_width(l, FONT_SIZE))
        .fold(0.0, f64::max)
}

fn line_count(text: &str) -> usize {
    text.split('\n').count().max(1)
}

/// A node's size and outline.
fn measure(node: &GNode) -> (f64, f64, Clip) {
    let tw = lines_width(&node.label);
    let th = line_count(&node.label) as f64 * LINE_H;
    match &node.shape {
        Shape::Point => (12.0, 12.0, Clip::Ellipse),
        Shape::End => (18.0, 18.0, Clip::Ellipse),
        Shape::Bar => (70.0, 8.0, Clip::Rect),
        Shape::Circle | Shape::DoubleCircle => {
            let d = (tw.max(th) + 24.0).max(36.0);
            (d, d, Clip::Ellipse)
        }
        Shape::Ellipse => (
            (tw * 1.3 + 24.0).max(60.0),
            (th * 1.4 + 12.0).max(36.0),
            Clip::Ellipse,
        ),
        Shape::Diamond => {
            let w = (tw + 24.0) * 1.5;
            let h = (th + 16.0) * 1.6;
            (w.max(50.0), h.max(40.0), Clip::Diamond)
        }
        Shape::Hexagon => (tw + 48.0, th + 20.0, Clip::Rect),
        Shape::Parallelogram(_) | Shape::Trapezoid(_) => (tw + 56.0, th + 20.0, Clip::Rect),
        Shape::Cylinder => (tw + 32.0, th + 36.0, Clip::Rect),
        Shape::Compartments(parts) => {
            let w = parts
                .iter()
                .flatten()
                .map(|l| text_width(l, FONT_SIZE))
                .fold(0.0, f64::max)
                + 24.0;
            let rows: usize = parts.iter().map(|p| p.len().max(1)).sum();
            let h = rows as f64 * LINE_H + parts.len() as f64 * 10.0;
            (w.max(80.0), h, Clip::Rect)
        }
        Shape::Plain => (tw + 8.0, th + 4.0, Clip::Rect),
        _ => ((tw + 28.0).max(48.0), (th + 18.0).max(36.0), Clip::Rect),
    }
}

/// The picture of a graph diagram.
pub fn render(g: &GraphDiagram) -> Vec<u8> {
    let sizes: Vec<(f64, f64, Clip)> = g.nodes.iter().map(measure).collect();
    let spec = layout::Graph {
        nodes: g
            .nodes
            .iter()
            .zip(&sizes)
            .map(|(n, (w, h, clip))| NodeSpec {
                w: *w,
                h: *h,
                clip: *clip,
                cluster: n.cluster,
            })
            .collect(),
        edges: g
            .edges
            .iter()
            .map(|e| EdgeSpec {
                from: e.from,
                to: e.to,
                label: (!e.label.is_empty()).then(|| {
                    (
                        lines_width(&e.label) + 8.0,
                        line_count(&e.label) as f64 * LINE_H + 4.0,
                    )
                }),
                minlen: e.minlen,
            })
            .collect(),
        clusters: g
            .clusters
            .iter()
            .map(|c| ClusterSpec {
                parent: c.parent,
                title_h: if c.label.is_empty() {
                    0.0
                } else {
                    LINE_H + 4.0
                },
            })
            .collect(),
        dir: g.dir,
        node_sep: if g.node_sep > 0.0 { g.node_sep } else { 36.0 },
        rank_sep: if g.rank_sep > 0.0 { g.rank_sep } else { 56.0 },
    };
    let laid = layout::layout(&spec);
    let mut svg = Svg::new();
    // clusters, outermost first
    let mut order: Vec<usize> = (0..g.clusters.len()).collect();
    let depth = |mut c: usize| {
        let mut d = 0;
        while let Some(p) = g.clusters[c].parent {
            c = p;
            d += 1;
            if d > 64 {
                break;
            }
        }
        d
    };
    order.sort_by_key(|c| depth(*c));
    for c in order {
        let (x, y, w, h) = laid.clusters[c];
        if w <= 0.0 {
            continue;
        }
        let cl = &g.clusters[c];
        let style = Style::new(
            &cl.fill
                .clone()
                .unwrap_or_else(|| colors::CLUSTER_FILL.into()),
            &cl.stroke
                .clone()
                .unwrap_or_else(|| colors::CLUSTER_STROKE.into()),
            1.0,
        );
        svg.rect(x, y, w, h, 6.0, &style);
        if !cl.label.is_empty() {
            svg.text(
                x + 8.0,
                y + LINE_H * 0.6 + 2.0,
                &cl.label,
                FONT_SIZE - 1.0,
                Anchor::Start,
                colors::MUTED,
                Some("600"),
            );
        }
    }
    // edges under nodes
    for (e, route) in g.edges.iter().zip(&laid.edges) {
        if e.style == LineStyle::Invisible || route.points.len() < 2 {
            continue;
        }
        let stroke = e.color.clone().unwrap_or_else(|| colors::LINE.into());
        let mut style = Style::line(
            &stroke,
            if e.style == LineStyle::Thick {
                3.0
            } else {
                1.4
            },
        );
        match e.style {
            LineStyle::Dashed => style = style.dashed("6 4"),
            LineStyle::Dotted => style = style.dashed("2 3"),
            _ => {}
        }
        svg.line(
            &route.points,
            route.points.len() > 2,
            &style,
            e.head,
            e.tail,
        );
        if let (Some((lx, ly)), false) = (route.label, e.label.is_empty()) {
            let w = lines_width(&e.label) + 8.0;
            let h = line_count(&e.label) as f64 * LINE_H + 2.0;
            svg.rect(
                lx - w / 2.0,
                ly - h / 2.0,
                w,
                h,
                3.0,
                &Style::new("#ffffff", "none", 0.0),
            );
            svg.text(
                lx,
                ly,
                &e.label,
                FONT_SIZE - 1.0,
                Anchor::Middle,
                colors::INK,
                None,
            );
        }
    }
    for (node, ((x, y), (w, h, _))) in g.nodes.iter().zip(laid.nodes.iter().zip(&sizes)) {
        draw_node(&mut svg, node, *x, *y, *w, *h);
    }
    if let Some(title) = &g.title {
        // above everything drawn so far
        let top = svg_top(&laid, &sizes) - 28.0;
        let left = svg_left(&laid, &sizes);
        svg.text(
            left,
            top,
            title,
            FONT_SIZE + 4.0,
            Anchor::Start,
            colors::INK,
            Some("600"),
        );
    }
    svg.finish(16.0)
}

fn svg_top(l: &layout::Layout, sizes: &[(f64, f64, Clip)]) -> f64 {
    let nodes = l
        .nodes
        .iter()
        .zip(sizes)
        .map(|((_, y), (_, h, _))| y - h / 2.0)
        .fold(f64::INFINITY, f64::min);
    let clusters = l
        .clusters
        .iter()
        .filter(|c| c.2 > 0.0)
        .map(|c| c.1)
        .fold(f64::INFINITY, f64::min);
    nodes.min(clusters)
}

fn svg_left(l: &layout::Layout, sizes: &[(f64, f64, Clip)]) -> f64 {
    l.nodes
        .iter()
        .zip(sizes)
        .map(|((x, _), (w, _, _))| x - w / 2.0)
        .fold(f64::INFINITY, f64::min)
}

fn draw_node(svg: &mut Svg, node: &GNode, x: f64, y: f64, w: f64, h: f64) {
    let fill = node
        .fill
        .clone()
        .unwrap_or_else(|| colors::NODE_FILL.into());
    let stroke = node
        .stroke
        .clone()
        .unwrap_or_else(|| colors::NODE_STROKE.into());
    let mut style = Style::new(&fill, &stroke, 1.3);
    if node.dashed {
        style = style.dashed("5 3");
    }
    let ink = node.text.clone().unwrap_or_else(|| colors::INK.into());
    let (l, t, r, b) = (x - w / 2.0, y - h / 2.0, x + w / 2.0, y + h / 2.0);
    let mut label = true;
    match &node.shape {
        Shape::Rect => svg.rect(l, t, w, h, 0.0, &style),
        Shape::Rounded => svg.rect(l, t, w, h, 8.0, &style),
        Shape::Stadium => svg.rect(l, t, w, h, h / 2.0, &style),
        Shape::Circle => svg.ellipse(x, y, w / 2.0, h / 2.0, &style),
        Shape::DoubleCircle => {
            svg.ellipse(x, y, w / 2.0, h / 2.0, &style);
            svg.ellipse(x, y, w / 2.0 - 4.0, h / 2.0 - 4.0, &style);
        }
        Shape::Ellipse => svg.ellipse(x, y, w / 2.0, h / 2.0, &style),
        Shape::Diamond => svg.polygon(&[(x, t), (r, y), (x, b), (l, y)], &style),
        Shape::Hexagon => {
            let d = h / 2.0;
            svg.polygon(
                &[
                    (l + d, t),
                    (r - d, t),
                    (r, y),
                    (r - d, b),
                    (l + d, b),
                    (l, y),
                ],
                &style,
            );
        }
        Shape::Parallelogram(right) => {
            let d = h / 2.0;
            let pts = if *right {
                [(l + d, t), (r, t), (r - d, b), (l, b)]
            } else {
                [(l, t), (r - d, t), (r, b), (l + d, b)]
            };
            svg.polygon(&pts, &style);
        }
        Shape::Trapezoid(wide_bottom) => {
            let d = h / 2.0;
            let pts = if *wide_bottom {
                [(l + d, t), (r - d, t), (r, b), (l, b)]
            } else {
                [(l, t), (r, t), (r - d, b), (l + d, b)]
            };
            svg.polygon(&pts, &style);
        }
        Shape::Cylinder => {
            let ry = 8.0;
            let d = format!(
                "M{l},{t1} A{rx},{ry} 0 0 1 {r},{t1} L{r},{b1} A{rx},{ry} 0 0 1 {l},{b1} Z",
                l = super::svg::n(l),
                r = super::svg::n(r),
                t1 = super::svg::n(t + ry),
                b1 = super::svg::n(b - ry),
                rx = super::svg::n(w / 2.0),
                ry = super::svg::n(ry)
            );
            svg.include_rect(l, t, w, h);
            svg.path(&d, &style, None, None);
            let top = format!(
                "M{l},{t1} A{rx},{ry} 0 0 0 {r},{t1}",
                l = super::svg::n(l),
                r = super::svg::n(r),
                t1 = super::svg::n(t + ry),
                rx = super::svg::n(w / 2.0),
                ry = super::svg::n(ry)
            );
            svg.path(&top, &Style::line(&stroke, 1.3), None, None);
        }
        Shape::Subroutine => {
            svg.rect(l, t, w, h, 0.0, &style);
            let line = Style::line(&stroke, 1.3);
            svg.line(&[(l + 8.0, t), (l + 8.0, b)], false, &line, None, None);
            svg.line(&[(r - 8.0, t), (r - 8.0, b)], false, &line, None, None);
        }
        Shape::Note => {
            let f = 10.0;
            let note = Style::new(
                node.fill.as_deref().unwrap_or(colors::NOTE_FILL),
                node.stroke.as_deref().unwrap_or(colors::NOTE_STROKE),
                1.2,
            );
            svg.polygon(&[(l, t), (r - f, t), (r, t + f), (r, b), (l, b)], &note);
        }
        Shape::Point => {
            svg.ellipse(
                x,
                y,
                w / 2.0,
                h / 2.0,
                &Style::new(colors::INK, colors::INK, 1.0),
            );
            label = false;
        }
        Shape::End => {
            svg.ellipse(
                x,
                y,
                w / 2.0,
                h / 2.0,
                &Style::new("#ffffff", colors::INK, 1.4),
            );
            svg.ellipse(
                x,
                y,
                w / 2.0 - 4.0,
                h / 2.0 - 4.0,
                &Style::new(colors::INK, colors::INK, 1.0),
            );
            label = false;
        }
        Shape::Bar => {
            svg.rect(l, t, w, h, 2.0, &Style::new(colors::INK, colors::INK, 1.0));
            label = false;
        }
        Shape::Plain => {}
        Shape::Compartments(parts) => {
            svg.rect(l, t, w, h, 0.0, &style);
            let mut cy = t;
            for (ix, part) in parts.iter().enumerate() {
                let rows = part.len().max(1) as f64;
                let ph = rows * LINE_H + 10.0;
                if ix > 0 {
                    svg.line(
                        &[(l, cy), (r, cy)],
                        false,
                        &Style::line(&stroke, 1.0),
                        None,
                        None,
                    );
                }
                for (k, line) in part.iter().enumerate() {
                    let ly = cy + 5.0 + LINE_H * (k as f64 + 0.5);
                    if ix == 0 {
                        svg.text(x, ly, line, FONT_SIZE, Anchor::Middle, &ink, Some("600"));
                    } else {
                        svg.text(
                            l + 10.0,
                            ly,
                            line,
                            FONT_SIZE - 1.0,
                            Anchor::Start,
                            &ink,
                            None,
                        );
                    }
                }
                cy += ph;
            }
            label = false;
        }
    }
    if label && !node.label.is_empty() {
        svg.text(x, y, &node.label, FONT_SIZE, Anchor::Middle, &ink, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_nodes_edges_and_clusters() {
        let mut g = GraphDiagram::default();
        let a = g.node("a");
        let b = g.node("b");
        g.nodes[b].shape = Shape::Diamond;
        g.clusters.push(GCluster {
            label: "group".into(),
            ..Default::default()
        });
        g.nodes[a].cluster = Some(0);
        let mut e = GEdge::new(a, b);
        e.label = "yes".into();
        g.edges.push(e);
        let svg = String::from_utf8(render(&g)).unwrap();
        assert!(svg.contains("<polygon"), "{svg}");
        assert!(svg.contains(">group<"), "{svg}");
        assert!(svg.contains(">yes<"), "{svg}");
        assert!(svg.contains("marker-end"), "{svg}");
    }
}
