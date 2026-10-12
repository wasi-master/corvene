//! Mermaid flowcharts (`flowchart` / `graph`): node shapes, chained and
//! `&`-joined links with their texts, arrowheads and line styles,
//! subgraphs, `classDef`/`class`/`:::`/`style` colors.

use std::collections::HashMap;

use super::super::graph::{GCluster, GEdge, GraphDiagram, LineStyle, Shape};
use super::super::layout::Dir;
use super::{label, style_colors};

pub(crate) fn direction(word: &str) -> Option<Dir> {
    Some(match word.trim().to_ascii_uppercase().as_str() {
        "TB" | "TD" => Dir::TopBottom,
        "BT" => Dir::BottomTop,
        "LR" => Dir::LeftRight,
        "RL" => Dir::RightLeft,
        _ => return None,
    })
}

/// Splits a line at `;` outside quotes and brackets.
pub(crate) fn statements(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote = false;
    let mut depth = 0i32;
    for c in line.chars() {
        match c {
            '"' => quote = !quote,
            '[' | '(' | '{' if !quote => depth += 1,
            ']' | ')' | '}' if !quote => depth -= 1,
            ';' if !quote && depth <= 0 => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    out.push(cur);
    out.into_iter().filter(|s| !s.trim().is_empty()).collect()
}

struct Link {
    head: Option<&'static str>,
    tail: Option<&'static str>,
    style: LineStyle,
    text: String,
    minlen: usize,
}

fn marker(c: char) -> Option<&'static str> {
    match c {
        '>' | '<' => Some("arrow"),
        'o' => Some("circle"),
        'x' => Some("cross"),
        _ => None,
    }
}

/// A link at the start of `s` (spaces skipped): the link and the bytes it
/// took.
fn link(s: &str) -> Option<(Link, usize)> {
    let lead = s.len() - s.trim_start().len();
    let t = &s[lead..];
    let b = t.as_bytes();
    let mut i = 0;
    let mut tail = None;
    // a start marker right before the line
    if b.len() > 2 && matches!(b[0], b'<' | b'o' | b'x') && matches!(b[1], b'-' | b'=' | b'.') {
        tail = marker(b[0] as char);
        i = 1;
    }
    let body_start = i;
    while i < b.len() && matches!(b[i], b'-' | b'=' | b'.' | b'~') {
        i += 1;
    }
    let body = &t[body_start..i];
    if body.len() < 2 && !(body.len() == 1 && b.get(i) == Some(&b'>') && body == "-") {
        return None;
    }
    let mut text = String::new();
    let mut whole = body.to_string();
    // the part whose length sets the link's length (the closer, for a
    // link with text in the middle)
    let mut measure_from = 0;
    // `-- text -->` / `== text ==>` / `-. text .->`
    let ends_open = !matches!(b.get(i), Some(b'>') | Some(b'x') | Some(b'o'))
        && body.len() == 2
        && matches!(body, "--" | "==" | "-.")
        && b.get(i).is_some_and(|c| *c == b' ');
    if ends_open {
        let rest = &t[i..];
        let closers = [
            "-->", "--x", "--o", "---", "==>", "===", "==x", "==o", ".->", ".-", "->",
        ];
        let found = closers
            .iter()
            .filter_map(|c| rest.find(c).map(|ix| (ix, *c)))
            .min_by_key(|(ix, c)| (*ix, std::cmp::Reverse(c.len())));
        if let Some((ix, closer)) = found {
            text = rest[..ix].trim().to_string();
            measure_from = whole.len();
            let mut j = i + ix + closer.len();
            // a longer closer (`--->`)
            let tb = t.as_bytes();
            while j < tb.len() && matches!(tb[j], b'-' | b'=' | b'.') {
                j += 1;
            }
            whole.push_str(&t[i + ix..j]);
            i = j;
            if i < tb.len()
                && matches!(tb[i], b'>' | b'x' | b'o')
                && !whole.ends_with(['>', 'x', 'o'])
            {
                whole.push(tb[i] as char);
                i += 1;
            }
        }
    } else if i < b.len() && matches!(b[i], b'>' | b'x' | b'o') {
        // `o` / `x` end markers only before a space or `|`
        let next = b.get(i + 1);
        if b[i] == b'>' || next.is_none_or(|c| *c == b' ' || *c == b'|') {
            whole.push(b[i] as char);
            i += 1;
        }
    }
    let head = whole.chars().last().and_then(marker);
    let style = if whole.contains('~') {
        LineStyle::Invisible
    } else if whole.contains('.') {
        LineStyle::Dotted
    } else if whole.contains('=') {
        LineStyle::Thick
    } else {
        LineStyle::Solid
    };
    let measured = &whole[measure_from..];
    let count = |ch: char| measured.chars().filter(|c| *c == ch).count();
    let minlen = match style {
        LineStyle::Dotted => count('.'),
        LineStyle::Thick => count('=').saturating_sub(if head.is_some() { 1 } else { 2 }),
        _ => count('-').saturating_sub(if head.is_some() { 1 } else { 2 }),
    }
    .clamp(1, 6);
    // `|text|` after the link
    let after = &t[i..];
    let after_trim = after.trim_start();
    if text.is_empty()
        && let Some(inner) = after_trim.strip_prefix('|')
        && let Some(end) = inner.find('|')
    {
        text = inner[..end].to_string();
        i += (after.len() - after_trim.len()) + end + 2;
    }
    Some((
        Link {
            head,
            tail,
            style,
            text: label(&text),
            minlen,
        },
        lead + i,
    ))
}

const OPENERS: [(&str, &str, u8); 15] = [
    ("(((", ")))", 1),
    ("((", "))", 2),
    ("([", "])", 3),
    ("[[", "]]", 4),
    ("[(", ")]", 5),
    ("[/", "/]", 6),
    ("[/", "\\]", 7),
    ("[\\", "\\]", 8),
    ("[\\", "/]", 9),
    ("{{", "}}", 10),
    ("(", ")", 11),
    ("[", "]", 12),
    ("{", "}", 13),
    (">", "]", 14),
    ("@{", "}", 15),
];

fn shape_of(code: u8) -> Shape {
    match code {
        1 => Shape::DoubleCircle,
        2 => Shape::Circle,
        3 => Shape::Stadium,
        4 => Shape::Subroutine,
        5 => Shape::Cylinder,
        6 => Shape::Parallelogram(true),
        7 => Shape::Trapezoid(true),
        8 => Shape::Parallelogram(false),
        9 => Shape::Trapezoid(false),
        10 => Shape::Hexagon,
        11 => Shape::Rounded,
        13 => Shape::Diamond,
        _ => Shape::Rect,
    }
}

/// The new `A@{ shape: …, label: "…" }` syntax.
fn at_shape(spec: &str) -> (Shape, Option<String>) {
    let mut shape = Shape::Rect;
    let mut text = None;
    for part in spec.split(',') {
        let Some((k, v)) = part.split_once(':') else {
            continue;
        };
        let v = v.trim().trim_matches('"').to_string();
        match k.trim() {
            "label" => text = Some(v),
            "shape" => {
                shape = match v.as_str() {
                    "rounded" | "event" => Shape::Rounded,
                    "stadium" | "pill" | "terminal" => Shape::Stadium,
                    "circle" | "circ" | "sm-circ" | "small-circle" | "start" => Shape::Circle,
                    "dbl-circ" | "double-circle" | "fr-circ" | "stop" => Shape::DoubleCircle,
                    "diam" | "diamond" | "decision" | "question" => Shape::Diamond,
                    "hex" | "hexagon" | "prepare" => Shape::Hexagon,
                    "cyl" | "cylinder" | "db" | "database" => Shape::Cylinder,
                    "subproc" | "subroutine" | "fr-rect" => Shape::Subroutine,
                    "lean-r" | "in-out" => Shape::Parallelogram(true),
                    "lean-l" | "out-in" => Shape::Parallelogram(false),
                    "trap-b" | "priority" => Shape::Trapezoid(true),
                    "trap-t" | "manual" => Shape::Trapezoid(false),
                    "doc" | "document" | "notch-rect" | "card" => Shape::Note,
                    "text" => Shape::Plain,
                    "f-circ" | "junction" | "filled-circle" => Shape::Point,
                    _ => Shape::Rect,
                }
            }
            _ => {}
        }
    }
    (shape, text)
}

struct State {
    g: GraphDiagram,
    index: HashMap<String, usize>,
    defined: Vec<bool>,
    classes: HashMap<String, String>,
    node_classes: Vec<(usize, String)>,
    styles: Vec<(usize, String)>,
    stack: Vec<usize>,
    cluster_ids: Vec<String>,
}

impl State {
    /// A node at the start of `s`: its index and the bytes taken.
    fn node(&mut self, s: &str) -> Option<(usize, usize)> {
        let lead = s.len() - s.trim_start().len();
        let t = &s[lead..];
        let id_len = t
            .char_indices()
            .find(|(_, c)| !(c.is_alphanumeric() || *c == '_' || *c == '$' || *c == '\''))
            .map_or(t.len(), |(ix, _)| ix);
        if id_len == 0 {
            return None;
        }
        let id = &t[..id_len];
        let mut i = id_len;
        let ix = self.g.node_indexed(id, &mut self.index);
        if self.defined.len() < self.g.nodes.len() {
            self.defined.push(false);
        }
        if let Some(&c) = self.stack.last() {
            let current = self.g.nodes[ix].cluster;
            if current.is_none() || current == self.g.clusters[c].parent {
                self.g.nodes[ix].cluster = Some(c);
            }
        }
        let rest = &t[i..];
        for (open, close, code) in OPENERS {
            if !rest.starts_with(open) {
                continue;
            }
            let inner = &rest[open.len()..];
            // a quoted text may hold the closer
            let text_end = if inner.trim_start().starts_with('"') {
                let q0 = inner.find('"').unwrap_or(0);
                inner[q0 + 1..].find('"').map(|q1| q0 + 1 + q1 + 1)
            } else {
                Some(0)
            };
            let Some(text_end) = text_end else {
                continue;
            };
            let Some(close_at) = inner[text_end..].find(close).map(|c| c + text_end) else {
                continue;
            };
            let text = &inner[..close_at];
            if code == 15 {
                let (shape, text) = at_shape(text);
                self.g.nodes[ix].shape = shape;
                if let Some(text) = text {
                    self.g.nodes[ix].label = label(&text);
                }
            } else {
                self.g.nodes[ix].shape = shape_of(code);
                self.g.nodes[ix].label = label(text);
            }
            self.defined[ix] = true;
            i += open.len() + close_at + close.len();
            break;
        }
        // `:::class`
        let rest = &t[i..];
        if let Some(cls) = rest.strip_prefix(":::") {
            let len = cls
                .char_indices()
                .find(|(_, c)| !(c.is_alphanumeric() || *c == '_' || *c == '-'))
                .map_or(cls.len(), |(ix, _)| ix);
            self.node_classes.push((ix, cls[..len].to_string()));
            i += 3 + len;
        }
        Some((ix, lead + i))
    }

    /// `A & B` at the start of `s`.
    fn group(&mut self, s: &str) -> Option<(Vec<usize>, usize)> {
        let (first, mut used) = self.node(s)?;
        let mut nodes = vec![first];
        loop {
            let rest = &s[used..];
            let trimmed = rest.trim_start();
            let Some(after) = trimmed.strip_prefix('&') else {
                break;
            };
            let skip = rest.len() - after.len();
            match self.node(after) {
                Some((n, took)) => {
                    nodes.push(n);
                    used += skip + took;
                }
                None => break,
            }
        }
        Some((nodes, used))
    }

    fn statement(&mut self, s: &str) -> Result<(), String> {
        let Some((mut left, mut used)) = self.group(s) else {
            return Ok(());
        };
        while let Some((l, took)) = link(&s[used..]) {
            used += took;
            let Some((right, took)) = self.group(&s[used..]) else {
                break;
            };
            used += took;
            for &a in &left {
                for &b in &right {
                    let mut e = GEdge::new(a, b);
                    e.head = l.head;
                    e.tail = l.tail;
                    e.style = l.style;
                    e.label = l.text.clone();
                    e.minlen = l.minlen;
                    self.g.edges.push(e);
                }
            }
            left = right;
        }
        Ok(())
    }
}

/// Parses the lines after the `flowchart <dir>` header.
pub fn parse(header: &str, lines: &[String]) -> Result<GraphDiagram, String> {
    let mut st = State {
        g: GraphDiagram::default(),
        index: HashMap::new(),
        defined: Vec::new(),
        classes: HashMap::new(),
        node_classes: Vec::new(),
        styles: Vec::new(),
        stack: Vec::new(),
        cluster_ids: Vec::new(),
    };
    st.g.dir = direction(header.split_whitespace().next().unwrap_or("TB")).unwrap_or_default();
    for line in lines {
        for stmt in statements(line) {
            let stmt = stmt.trim();
            let word = stmt.split_whitespace().next().unwrap_or("");
            let rest = stmt[word.len()..].trim();
            match word {
                "subgraph" => {
                    // `subgraph id [Title]`, `subgraph "Title"`, `subgraph Title words`
                    let (id, title) = match rest.find('[') {
                        Some(b) if rest.ends_with(']') => (
                            rest[..b].trim().to_string(),
                            label(&rest[b + 1..rest.len() - 1]),
                        ),
                        _ => (rest.trim_matches('"').to_string(), label(rest)),
                    };
                    st.g.clusters.push(GCluster {
                        label: title,
                        parent: st.stack.last().copied(),
                        ..Default::default()
                    });
                    st.cluster_ids.push(id);
                    st.stack.push(st.g.clusters.len() - 1);
                }
                "end" if rest.is_empty() => {
                    st.stack.pop();
                }
                "direction" => {
                    if st.stack.is_empty()
                        && let Some(d) = direction(rest)
                    {
                        st.g.dir = d;
                    }
                }
                "classDef" => {
                    let mut parts = rest.splitn(2, char::is_whitespace);
                    let names = parts.next().unwrap_or("");
                    let spec = parts.next().unwrap_or("").trim().to_string();
                    for name in names.split(',') {
                        st.classes.insert(name.trim().to_string(), spec.clone());
                    }
                }
                "class" => {
                    let mut parts = rest.rsplitn(2, char::is_whitespace);
                    let cls = parts.next().unwrap_or("").to_string();
                    for id in parts.next().unwrap_or("").split(',') {
                        let id = id.trim();
                        if !id.is_empty() {
                            let ix = st.g.node_indexed(id, &mut st.index);
                            if st.defined.len() < st.g.nodes.len() {
                                st.defined.push(false);
                            }
                            st.node_classes.push((ix, cls.clone()));
                        }
                    }
                }
                "style" => {
                    let mut parts = rest.splitn(2, char::is_whitespace);
                    let id = parts.next().unwrap_or("");
                    let spec = parts.next().unwrap_or("").to_string();
                    if let Some(&ix) = st.index.get(id) {
                        st.styles.push((ix, spec));
                    } else if let Some(c) = st.cluster_ids.iter().position(|c| c == id) {
                        let (fill, stroke, _, _) = style_colors(&spec);
                        st.g.clusters[c].fill = fill;
                        st.g.clusters[c].stroke = stroke;
                    }
                }
                "linkStyle" | "click" | "accTitle:" | "accDescr:" | "accTitle" | "accDescr"
                | "callback" => {}
                _ => st.statement(stmt)?,
            }
        }
    }
    let default_class = st.classes.get("default").cloned();
    for ix in 0..st.g.nodes.len() {
        let mut specs: Vec<String> = default_class.iter().cloned().collect();
        specs.extend(
            st.node_classes
                .iter()
                .filter(|(n, _)| *n == ix)
                .filter_map(|(_, c)| st.classes.get(c).cloned()),
        );
        specs.extend(
            st.styles
                .iter()
                .filter(|(n, _)| *n == ix)
                .map(|(_, s)| s.clone()),
        );
        for spec in specs {
            let (fill, stroke, text, dashed) = style_colors(&spec);
            let node = &mut st.g.nodes[ix];
            node.fill = fill.or(node.fill.take());
            node.stroke = stroke.or(node.stroke.take());
            node.text = text.or(node.text.take());
            node.dashed |= dashed;
        }
    }
    // links to a subgraph's id go to its first member
    let mut redirect: HashMap<usize, usize> = HashMap::new();
    for (c, id) in st.cluster_ids.iter().enumerate() {
        let Some(&ix) = st.index.get(id) else {
            continue;
        };
        if st.defined.get(ix).copied().unwrap_or(false) {
            continue;
        }
        if let Some(member) =
            (0..st.g.nodes.len()).find(|&n| n != ix && st.g.nodes[n].cluster == Some(c))
        {
            redirect.insert(ix, member);
        }
    }
    if !redirect.is_empty() {
        for e in &mut st.g.edges {
            if let Some(&m) = redirect.get(&e.from) {
                e.from = m;
            }
            if let Some(&m) = redirect.get(&e.to) {
                e.to = m;
            }
        }
        let mut keep = Vec::new();
        let mut remap = vec![usize::MAX; st.g.nodes.len()];
        for (ix, node) in st.g.nodes.drain(..).enumerate() {
            if !redirect.contains_key(&ix) {
                remap[ix] = keep.len();
                keep.push(node);
            }
        }
        st.g.nodes = keep;
        for e in &mut st.g.edges {
            e.from = remap[e.from];
            e.to = remap[e.to];
        }
    }
    if st.g.nodes.is_empty() {
        return Err("The flowchart has no nodes.".into());
    }
    if st.g.nodes.len() > 5_000 {
        return Err("The flowchart is too large to draw.".into());
    }
    Ok(st.g)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(str::to_string).collect()
    }

    #[test]
    fn shapes_links_and_texts() {
        let g = parse(
            "LR",
            &lines(
                "    A[Start] --> B{Is it?}\n    B -->|Yes| C([Done])\n    B -- No --> D[(DB)]\n    C -.-> E((Circle)) ==> F[/Lean/]\n    A & B --- G>Flag]\n    H@{ shape: cyl, label: \"Store\" }",
            ),
        )
        .unwrap();
        assert_eq!(g.dir, Dir::LeftRight);
        let n = |id: &str| g.nodes.iter().find(|n| n.id == id).unwrap();
        assert_eq!(n("A").label, "Start");
        assert_eq!(n("B").shape, Shape::Diamond);
        assert_eq!(n("C").shape, Shape::Stadium);
        assert_eq!(n("D").shape, Shape::Cylinder);
        assert_eq!(n("E").shape, Shape::Circle);
        assert_eq!(n("F").shape, Shape::Parallelogram(true));
        assert_eq!(n("H").shape, Shape::Cylinder);
        assert_eq!(n("H").label, "Store");
        let labels: Vec<&str> = g.edges.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels[1], "Yes");
        assert_eq!(labels[2], "No");
        assert_eq!(g.edges[3].style, LineStyle::Dotted);
        assert_eq!(g.edges[4].style, LineStyle::Thick);
        // A & B --- G: two edges without heads
        assert_eq!(g.edges.len(), 7);
        assert_eq!(g.edges[5].head, None);
    }

    #[test]
    fn subgraphs_and_classes() {
        let g = parse(
            "TD",
            &lines(
                "subgraph one [First]\n  a1-->a2\nend\nsubgraph two\n  b1-->b2\nend\none --> two\nclassDef hot fill:#f96,stroke:#333\nclass a1 hot\nb1:::hot\nstyle b2 fill:#0f0",
            ),
        )
        .unwrap();
        assert_eq!(g.clusters[0].label, "First");
        assert_eq!(g.clusters[1].label, "two");
        let a1 = g.nodes.iter().find(|n| n.id == "a1").unwrap();
        assert_eq!(a1.cluster, Some(0));
        assert_eq!(a1.fill.as_deref(), Some("#f96"));
        let b2 = g.nodes.iter().find(|n| n.id == "b2").unwrap();
        assert_eq!(b2.fill.as_deref(), Some("#0f0"));
        // `one --> two` became a link between members, not two new boxes
        assert!(g.nodes.iter().all(|n| n.id != "one" && n.id != "two"));
        assert_eq!(g.edges.len(), 3);
    }

    #[test]
    fn link_lengths() {
        let (l, _) = link(" ---> B").unwrap();
        assert_eq!(l.minlen, 2);
        assert_eq!(l.head, Some("arrow"));
        let (l, _) = link(" <--> B").unwrap();
        assert_eq!((l.head, l.tail), (Some("arrow"), Some("arrow")));
        let (l, _) = link(" --o B").unwrap();
        assert_eq!(l.head, Some("circle"));
        assert!(link(" B").is_none());
    }
}
