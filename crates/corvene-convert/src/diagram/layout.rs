//! A layered ("Sugiyama") graph layout for Graphviz graphs and Mermaid
//! flowcharts, state and class diagrams:
//!
//! 1. cycles are broken by reversing DFS back edges;
//! 2. nodes get ranks by longest path (sources pulled down next to their
//!    children);
//! 3. long edges are split by dummy nodes (a labeled edge's middle dummy
//!    is as large as its label);
//! 4. each rank is ordered by barycenter sweeps (cluster members kept
//!    together), keeping the order with the fewest crossings;
//! 5. x positions come from repeated isotonic regressions: each node is
//!    pulled to its neighbors' mean under the minimum gaps of its rank,
//!    which pool-adjacent-violators solves exactly;
//! 6. edges are polylines through their dummies, clipped to node shapes.
//!
//! Layout runs top to bottom; other directions transform the result.

/// Which way ranks run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Dir {
    #[default]
    TopBottom,
    BottomTop,
    LeftRight,
    RightLeft,
}

impl Dir {
    fn horizontal(self) -> bool {
        matches!(self, Dir::LeftRight | Dir::RightLeft)
    }
}

/// The outline edges are clipped to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Clip {
    #[default]
    Rect,
    Ellipse,
    Diamond,
}

#[derive(Clone, Debug, Default)]
pub struct NodeSpec {
    pub w: f64,
    pub h: f64,
    pub clip: Clip,
    /// The innermost cluster holding the node.
    pub cluster: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct EdgeSpec {
    pub from: usize,
    pub to: usize,
    /// The label's size, when the edge has one.
    pub label: Option<(f64, f64)>,
    /// Ranks the edge spans at least (1 normally; 0 keeps both ends on
    /// one rank where possible).
    pub minlen: usize,
}

#[derive(Clone, Debug, Default)]
pub struct ClusterSpec {
    pub parent: Option<usize>,
    /// Room kept above the members for the cluster's title.
    pub title_h: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Graph {
    pub nodes: Vec<NodeSpec>,
    pub edges: Vec<EdgeSpec>,
    pub clusters: Vec<ClusterSpec>,
    pub dir: Dir,
    pub node_sep: f64,
    pub rank_sep: f64,
}

#[derive(Clone, Debug, Default)]
pub struct EdgeRoute {
    /// From the edge's source to its target (clipped to their outlines).
    pub points: Vec<(f64, f64)>,
    pub label: Option<(f64, f64)>,
}

#[derive(Clone, Debug, Default)]
pub struct Layout {
    /// Node centers.
    pub nodes: Vec<(f64, f64)>,
    pub edges: Vec<EdgeRoute>,
    /// Cluster boxes `(x, y, w, h)` (empty clusters get a zero box).
    pub clusters: Vec<(f64, f64, f64, f64)>,
}

#[derive(Clone, Debug)]
struct VNode {
    w: f64,
    h: f64,
    rank: usize,
    /// The real node, for real ones.
    real: Option<usize>,
    group: Option<usize>,
    ups: Vec<usize>,
    downs: Vec<usize>,
    x: f64,
    y: f64,
    order: usize,
}

/// Lays `graph` out.
pub fn layout(graph: &Graph) -> Layout {
    let n = graph.nodes.len();
    if n == 0 {
        return Layout::default();
    }
    let horizontal = graph.dir.horizontal();
    let size = |w: f64, h: f64| if horizontal { (h, w) } else { (w, h) };
    let node_sep = if graph.node_sep > 0.0 {
        graph.node_sep
    } else {
        40.0
    };
    let mut rank_sep = if graph.rank_sep > 0.0 {
        graph.rank_sep
    } else {
        50.0
    };
    let labeled = graph
        .edges
        .iter()
        .any(|e| e.label.is_some() && e.from != e.to);
    if labeled {
        rank_sep /= 2.0;
    }

    // 1. break cycles: reverse back edges of a DFS
    let mut out_edges: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (ix, e) in graph.edges.iter().enumerate() {
        if e.from < n && e.to < n && e.from != e.to {
            out_edges[e.from].push(ix);
        }
    }
    let mut reversed = vec![false; graph.edges.len()];
    {
        // 0 unvisited, 1 on stack, 2 done
        let mut state = vec![0u8; n];
        for start in 0..n {
            if state[start] != 0 {
                continue;
            }
            let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
            state[start] = 1;
            while let Some((v, i)) = stack.last_mut() {
                let v = *v;
                if *i < out_edges[v].len() {
                    let e = out_edges[v][*i];
                    *i += 1;
                    let w = graph.edges[e].to;
                    match state[w] {
                        0 => {
                            state[w] = 1;
                            stack.push((w, 0));
                        }
                        1 => reversed[e] = true,
                        _ => {}
                    }
                } else {
                    state[v] = 2;
                    stack.pop();
                }
            }
        }
    }
    let ends = |ix: usize| {
        let e = &graph.edges[ix];
        if reversed[ix] {
            (e.to, e.from)
        } else {
            (e.from, e.to)
        }
    };
    let minlen = |ix: usize| {
        let base = graph.edges[ix].minlen;
        if labeled { base * 2 } else { base }
    };

    // 2. ranks by longest path from the sources
    let real_edges: Vec<usize> = (0..graph.edges.len())
        .filter(|&ix| {
            let e = &graph.edges[ix];
            e.from < n && e.to < n && e.from != e.to
        })
        .collect();
    let mut indeg = vec![0usize; n];
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut pred: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &ix in &real_edges {
        let (u, v) = ends(ix);
        indeg[v] += 1;
        succ[u].push(ix);
        pred[v].push(ix);
    }
    let mut topo = Vec::with_capacity(n);
    let mut queue: std::collections::VecDeque<usize> = (0..n).filter(|v| indeg[*v] == 0).collect();
    let mut left = indeg.clone();
    while let Some(v) = queue.pop_front() {
        topo.push(v);
        for &ix in &succ[v] {
            let (_, w) = ends(ix);
            left[w] -= 1;
            if left[w] == 0 {
                queue.push_back(w);
            }
        }
    }
    // (the DFS left no cycles; this guards against bad input anyway)
    if topo.len() < n {
        let mut listed = vec![false; n];
        for &v in &topo {
            listed[v] = true;
        }
        topo.extend((0..n).filter(|v| !listed[*v]));
    }
    let mut rank = vec![0usize; n];
    for &v in &topo {
        for &ix in &pred[v] {
            let (u, _) = ends(ix);
            rank[v] = rank[v].max(rank[u] + minlen(ix));
        }
    }
    // pull sources down next to their nearest child
    for &v in topo.iter().rev() {
        if pred[v].is_empty() && !succ[v].is_empty() {
            let best = succ[v]
                .iter()
                .map(|&ix| rank[ends(ix).1].saturating_sub(minlen(ix)))
                .min()
                .unwrap_or(rank[v]);
            rank[v] = rank[v].max(best);
        }
    }

    // 3. the layered graph, with dummies on long edges
    let mut vnodes: Vec<VNode> = (0..n)
        .map(|v| {
            let (w, h) = size(graph.nodes[v].w, graph.nodes[v].h);
            VNode {
                w,
                h,
                rank: rank[v],
                real: Some(v),
                group: top_cluster(graph, graph.nodes[v].cluster),
                ups: Vec::new(),
                downs: Vec::new(),
                x: 0.0,
                y: 0.0,
                order: 0,
            }
        })
        .collect();
    // per edge: its chain of virtual nodes from the (layout) source
    let mut chains: Vec<Vec<usize>> = vec![Vec::new(); graph.edges.len()];
    let mut label_node: Vec<Option<usize>> = vec![None; graph.edges.len()];
    for &ix in &real_edges {
        let (u, v) = ends(ix);
        let (ru, rv) = (rank[u], rank[v]);
        let mut chain = vec![u];
        let mut prev = u;
        if rv > ru + 1 {
            let mid = ru + (rv - ru) / 2;
            for r in ru + 1..rv {
                let (lw, lh) = match graph.edges[ix].label {
                    Some((w, h)) if r == mid => size(w, h),
                    _ => (0.0, 0.0),
                };
                let group = match (vnodes[u].group, vnodes[v].group) {
                    (Some(a), Some(b)) if a == b => Some(a),
                    _ => None,
                };
                vnodes.push(VNode {
                    w: lw,
                    h: lh,
                    rank: r,
                    real: None,
                    group,
                    ups: vec![prev],
                    downs: Vec::new(),
                    x: 0.0,
                    y: 0.0,
                    order: 0,
                });
                let d = vnodes.len() - 1;
                vnodes[prev].downs.push(d);
                if r == mid && graph.edges[ix].label.is_some() {
                    label_node[ix] = Some(d);
                }
                chain.push(d);
                prev = d;
            }
        }
        if rv == ru {
            // same rank (minlen 0): no link between ranks
        } else {
            vnodes[prev].downs.push(v);
            vnodes[v].ups.push(prev);
        }
        chain.push(v);
        chains[ix] = chain;
    }
    let ranks = vnodes.iter().map(|v| v.rank).max().unwrap_or(0) + 1;
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); ranks];

    // 4. order: first appearance in a BFS, then barycenter sweeps
    {
        let mut seen = vec![false; vnodes.len()];
        let mut roots: Vec<usize> = (0..vnodes.len())
            .filter(|&v| vnodes[v].ups.is_empty())
            .collect();
        roots.sort_by_key(|&v| (vnodes[v].rank, v));
        let mut queue: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
        for r in roots {
            if seen[r] {
                continue;
            }
            seen[r] = true;
            queue.push_back(r);
            while let Some(v) = queue.pop_front() {
                layers[vnodes[v].rank].push(v);
                for &w in &vnodes[v].downs {
                    if !seen[w] {
                        seen[w] = true;
                        queue.push_back(w);
                    }
                }
            }
        }
        for v in 0..vnodes.len() {
            if !seen[v] {
                layers[vnodes[v].rank].push(v);
            }
        }
    }
    let renumber = |layers: &Vec<Vec<usize>>, vnodes: &mut Vec<VNode>| {
        for layer in layers {
            for (ix, &v) in layer.iter().enumerate() {
                vnodes[v].order = ix;
            }
        }
    };
    renumber(&layers, &mut vnodes);
    let big = vnodes.len() > 3000;
    let mut best = layers.clone();
    let mut best_crossings = crossings(&layers, &vnodes);
    let sweeps = if big { 4 } else { 24 };
    for sweep in 0..sweeps {
        let down = sweep % 2 == 0;
        let order: Vec<usize> = if down {
            (1..ranks).collect()
        } else {
            (0..ranks.saturating_sub(1)).rev().collect()
        };
        for r in order {
            let mut keyed: Vec<(f64, usize)> = layers[r]
                .iter()
                .map(|&v| {
                    let ns = if down {
                        &vnodes[v].ups
                    } else {
                        &vnodes[v].downs
                    };
                    let bc = if ns.is_empty() {
                        vnodes[v].order as f64
                    } else {
                        ns.iter().map(|&w| vnodes[w].order as f64).sum::<f64>() / ns.len() as f64
                    };
                    (bc, v)
                })
                .collect();
            // cluster members sort by their cluster's mean, so they stay
            // next to each other
            let mut group_mean: std::collections::HashMap<usize, (f64, usize)> = Default::default();
            for (bc, v) in &keyed {
                if let Some(g) = vnodes[*v].group {
                    let e = group_mean.entry(g).or_insert((0.0, 0));
                    e.0 += bc;
                    e.1 += 1;
                }
            }
            keyed.sort_by(|a, b| {
                let key = |(bc, v): &(f64, usize)| match vnodes[*v].group {
                    Some(g) => {
                        let (s, c) = group_mean[&g];
                        (s / c as f64, g as f64, *bc)
                    }
                    None => (*bc, -1.0, *bc),
                };
                key(a)
                    .partial_cmp(&key(b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            layers[r] = keyed.into_iter().map(|(_, v)| v).collect();
            for (ix, &v) in layers[r].iter().enumerate() {
                vnodes[v].order = ix;
            }
        }
        let c = crossings(&layers, &vnodes);
        if c < best_crossings {
            best_crossings = c;
            best = layers.clone();
        }
        if best_crossings == 0 {
            break;
        }
    }
    layers = best;
    renumber(&layers, &mut vnodes);

    // 5. x by repeated isotonic regression
    let gap = |a: &VNode, b: &VNode| {
        let sep = if a.real.is_none() && b.real.is_none() {
            node_sep / 3.0
        } else if a.real.is_none() || b.real.is_none() {
            node_sep / 2.0
        } else {
            node_sep
        };
        let cluster_pad = if a.group != b.group {
            node_sep / 2.0
        } else {
            0.0
        };
        (a.w + b.w) / 2.0 + sep + cluster_pad
    };
    for layer in &layers {
        let mut x = 0.0;
        for (ix, &v) in layer.iter().enumerate() {
            if ix > 0 {
                x += gap(&vnodes[layer[ix - 1]], &vnodes[v]);
            }
            vnodes[v].x = x;
        }
    }
    let passes = if big { 4 } else { 16 };
    for pass in 0..passes {
        let mode = pass % 3; // 0 ups, 1 downs, 2 both
        let order: Vec<usize> = if mode == 1 {
            (0..ranks).rev().collect()
        } else {
            (0..ranks).collect()
        };
        for r in order {
            let layer = &layers[r];
            if layer.is_empty() {
                continue;
            }
            let mut desired = Vec::with_capacity(layer.len());
            let mut weights = Vec::with_capacity(layer.len());
            for &v in layer {
                let node = &vnodes[v];
                let ns: Vec<usize> = match mode {
                    0 => node.ups.clone(),
                    1 => node.downs.clone(),
                    _ => node.ups.iter().chain(&node.downs).copied().collect(),
                };
                if ns.is_empty() {
                    desired.push(node.x);
                    weights.push(0.2);
                } else {
                    desired.push(ns.iter().map(|&w| vnodes[w].x).sum::<f64>() / ns.len() as f64);
                    // long edges straighten first
                    weights.push(if node.real.is_none() { 4.0 } else { 1.0 });
                }
            }
            let gaps: Vec<f64> = (1..layer.len())
                .map(|i| gap(&vnodes[layer[i - 1]], &vnodes[layer[i]]))
                .collect();
            let xs = place(&desired, &weights, &gaps);
            for (i, &v) in layer.iter().enumerate() {
                vnodes[v].x = xs[i];
            }
        }
    }
    let min_x = vnodes
        .iter()
        .map(|v| v.x - v.w / 2.0)
        .fold(f64::INFINITY, f64::min);
    for v in &mut vnodes {
        v.x -= min_x;
    }

    // y per rank
    let mut y = 0.0;
    for layer in &layers {
        let h = layer.iter().map(|&v| vnodes[v].h).fold(0.0, f64::max);
        for &v in layer {
            vnodes[v].y = y + h / 2.0;
        }
        y += h + rank_sep;
    }

    // 6. routes, then the direction's transform
    let transform = |(x, y): (f64, f64)| -> (f64, f64) {
        match graph.dir {
            Dir::TopBottom => (x, y),
            Dir::BottomTop => (x, -y),
            Dir::LeftRight => (y, x),
            Dir::RightLeft => (-y, x),
        }
    };
    let centers: Vec<(f64, f64)> = (0..n)
        .map(|v| transform((vnodes[v].x, vnodes[v].y)))
        .collect();
    let mut routes = vec![EdgeRoute::default(); graph.edges.len()];
    for (ix, e) in graph.edges.iter().enumerate() {
        if e.from >= n || e.to >= n {
            continue;
        }
        if e.from == e.to {
            let (cx, cy) = centers[e.from];
            let (w, h) = (graph.nodes[e.from].w, graph.nodes[e.from].h);
            let r = (h / 2.0).clamp(12.0, 30.0);
            routes[ix] = EdgeRoute {
                points: vec![
                    (cx + w / 2.0, cy - h / 5.0),
                    (cx + w / 2.0 + r, cy - h / 3.0 - 4.0),
                    (cx + w / 2.0 + r, cy + h / 3.0 + 4.0),
                    (cx + w / 2.0, cy + h / 5.0),
                ],
                label: e
                    .label
                    .map(|(lw, _)| (cx + w / 2.0 + r + 6.0 + lw / 2.0, cy)),
            };
            continue;
        }
        let chain = &chains[ix];
        if chain.is_empty() {
            continue;
        }
        let mut points: Vec<(f64, f64)> = chain
            .iter()
            .map(|&v| transform((vnodes[v].x, vnodes[v].y)))
            .collect();
        if reversed[ix] {
            points.reverse();
        }
        // clip the ends to the node outlines
        let (from, to) = (e.from, e.to);
        if points.len() >= 2 {
            let p1 = points[1];
            points[0] = clip(centers[from], &graph.nodes[from], p1);
            let k = points.len();
            let pk = points[k - 2];
            points[k - 1] = clip(centers[to], &graph.nodes[to], pk);
        }
        let label = label_node[ix]
            .map(|d| transform((vnodes[d].x, vnodes[d].y)))
            .or_else(|| {
                e.label.map(|_| {
                    let a = points[0];
                    let b = points[points.len() - 1];
                    ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0)
                })
            });
        routes[ix] = EdgeRoute { points, label };
    }

    // cluster boxes, innermost first so parents hold them
    let mut boxes = vec![
        (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY
        );
        graph.clusters.len()
    ];
    let pad = node_sep / 3.0;
    for (v, node) in graph.nodes.iter().enumerate() {
        let mut c = node.cluster;
        let (x, y) = centers[v];
        let mut depth = 0;
        while let Some(ci) = c {
            if ci >= boxes.len() || depth > 64 {
                break;
            }
            let b = &mut boxes[ci];
            let extra = pad * (depth as f64 + 1.0);
            b.0 = b.0.min(x - node.w / 2.0 - extra);
            b.1 =
                b.1.min(y - node.h / 2.0 - extra - graph.clusters[ci].title_h);
            b.2 = b.2.max(x + node.w / 2.0 + extra);
            b.3 = b.3.max(y + node.h / 2.0 + extra);
            c = graph.clusters[ci].parent;
            depth += 1;
        }
    }
    let clusters = boxes
        .into_iter()
        .map(|(x0, y0, x1, y1)| {
            if x0.is_finite() {
                (x0, y0, x1 - x0, y1 - y0)
            } else {
                (0.0, 0.0, 0.0, 0.0)
            }
        })
        .collect();
    Layout {
        nodes: centers,
        edges: routes,
        clusters,
    }
}

fn top_cluster(graph: &Graph, mut c: Option<usize>) -> Option<usize> {
    let mut depth = 0;
    while let Some(ci) = c {
        match graph.clusters.get(ci).and_then(|cl| cl.parent) {
            Some(p) if depth < 64 => {
                c = Some(p);
                depth += 1;
            }
            _ => return Some(ci),
        }
    }
    None
}

/// Edge crossings between neighboring ranks.
fn crossings(layers: &[Vec<usize>], vnodes: &[VNode]) -> usize {
    let mut total = 0;
    for layer in layers {
        let mut pairs: Vec<(usize, usize)> = Vec::new();
        for &v in layer {
            for &w in &vnodes[v].downs {
                pairs.push((vnodes[v].order, vnodes[w].order));
            }
        }
        if pairs.len() > 4000 {
            continue;
        }
        pairs.sort_unstable();
        for i in 0..pairs.len() {
            for j in i + 1..pairs.len() {
                if pairs[j].0 > pairs[i].0 && pairs[j].1 < pairs[i].1 {
                    total += 1;
                }
            }
        }
    }
    total
}

/// The positions closest (weighted least squares) to `desired` with
/// `x[i+1] - x[i] >= gaps[i]`: shifting by the running gap sum turns it
/// into an isotonic regression, solved by pool-adjacent-violators.
fn place(desired: &[f64], weights: &[f64], gaps: &[f64]) -> Vec<f64> {
    let k = desired.len();
    let mut offset = vec![0.0; k];
    for i in 1..k {
        offset[i] = offset[i - 1] + gaps[i - 1];
    }
    // blocks of (weighted mean, weight, count)
    let mut blocks: Vec<(f64, f64, usize)> = Vec::with_capacity(k);
    for i in 0..k {
        let w = weights[i].max(1e-6);
        let mut block = (desired[i] - offset[i], w, 1usize);
        while let Some(&(m, bw, c)) = blocks.last() {
            if m <= block.0 {
                break;
            }
            blocks.pop();
            let total = bw + block.1;
            block = ((m * bw + block.0 * block.1) / total, total, c + block.2);
        }
        blocks.push(block);
    }
    let mut out = Vec::with_capacity(k);
    for (m, _, c) in blocks {
        for _ in 0..c {
            let i = out.len();
            out.push(m + offset[i]);
        }
    }
    out
}

/// Where the line from `toward` to a node's center crosses its outline.
fn clip(center: (f64, f64), node: &NodeSpec, toward: (f64, f64)) -> (f64, f64) {
    let (cx, cy) = center;
    let (dx, dy) = (toward.0 - cx, toward.1 - cy);
    if dx.abs() < 1e-9 && dy.abs() < 1e-9 {
        return center;
    }
    let (hw, hh) = (node.w / 2.0, node.h / 2.0);
    if hw <= 0.0 || hh <= 0.0 {
        return center;
    }
    let t = match node.clip {
        Clip::Rect => {
            let tx = if dx.abs() > 1e-9 {
                hw / dx.abs()
            } else {
                f64::INFINITY
            };
            let ty = if dy.abs() > 1e-9 {
                hh / dy.abs()
            } else {
                f64::INFINITY
            };
            tx.min(ty)
        }
        Clip::Ellipse => 1.0 / ((dx / hw).powi(2) + (dy / hh).powi(2)).sqrt(),
        Clip::Diamond => 1.0 / (dx.abs() / hw + dy.abs() / hh),
    };
    let t = t.min(1.0);
    (cx + dx * t, cy + dy * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node() -> NodeSpec {
        NodeSpec {
            w: 60.0,
            h: 30.0,
            ..Default::default()
        }
    }

    fn edge(from: usize, to: usize) -> EdgeSpec {
        EdgeSpec {
            from,
            to,
            label: None,
            minlen: 1,
        }
    }

    #[test]
    fn chains_run_down() {
        let g = Graph {
            nodes: vec![node(), node(), node()],
            edges: vec![edge(0, 1), edge(1, 2)],
            ..Default::default()
        };
        let l = layout(&g);
        assert!(l.nodes[0].1 < l.nodes[1].1 && l.nodes[1].1 < l.nodes[2].1);
        assert!((l.nodes[0].0 - l.nodes[2].0).abs() < 1.0, "{:?}", l.nodes);
        // ends clipped to the boxes
        assert!((l.edges[0].points[0].1 - (l.nodes[0].1 + 15.0)).abs() < 1e-6);
    }

    #[test]
    fn cycles_and_direction() {
        let g = Graph {
            nodes: vec![node(), node()],
            edges: vec![edge(0, 1), edge(1, 0)],
            dir: Dir::LeftRight,
            ..Default::default()
        };
        let l = layout(&g);
        assert!(l.nodes[0].0 < l.nodes[1].0, "{:?}", l.nodes);
        // the back edge still runs from 1 to 0
        let back = &l.edges[1].points;
        assert!(back[0].0 > back[back.len() - 1].0);
    }

    #[test]
    fn siblings_do_not_overlap() {
        let g = Graph {
            nodes: vec![node(), node(), node(), node()],
            edges: vec![edge(0, 1), edge(0, 2), edge(0, 3)],
            ..Default::default()
        };
        let l = layout(&g);
        let mut xs: Vec<f64> = l.nodes[1..].iter().map(|p| p.0).collect();
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!(xs[1] - xs[0] >= 60.0 && xs[2] - xs[1] >= 60.0, "{xs:?}");
        // the parent sits over the middle child
        assert!((l.nodes[0].0 - xs[1]).abs() < 1.0);
    }

    #[test]
    fn isotonic_placement() {
        let xs = place(&[10.0, 10.0, 10.0], &[1.0, 1.0, 1.0], &[5.0, 5.0]);
        assert_eq!(xs, vec![5.0, 10.0, 15.0]);
        let xs = place(&[0.0, 100.0], &[1.0, 1.0], &[5.0]);
        assert_eq!(xs, vec![0.0, 100.0]);
    }
}
