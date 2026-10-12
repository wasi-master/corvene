//! Graphviz DOT files (`.dot`, `.gv`): graphs, digraphs, subgraphs and
//! clusters, node, edge and graph attributes (label, shape, style, colors,
//! `rankdir`, arrowheads, `dir`, `minlen`), record and HTML labels shown
//! as text. Drawn with Corvene's own layered layout, so positions differ
//! from `dot`'s, but the structure is the same.

use std::collections::HashMap;

use super::graph::{GCluster, GEdge, GNode, GraphDiagram, LineStyle, Shape};
use super::layout::Dir;

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Id(String),
    /// A double-quoted string (kept apart so keywords can be quoted).
    Quoted(String),
    Html(String),
    Punct(char),
    Arrow,
    Line,
}

fn tokenize(src: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line_start = true;
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            line_start = true;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // `#` lines are preprocessor output
        if line_start && c == '#' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        line_start = false;
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        if c == '-' && matches!(chars.get(i + 1), Some('>') | Some('-')) {
            out.push(if chars[i + 1] == '>' {
                Tok::Arrow
            } else {
                Tok::Line
            });
            i += 2;
            continue;
        }
        if c == '"' {
            let mut s = String::new();
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    match chars[i + 1] {
                        '"' => s.push('"'),
                        '\n' => {}
                        other => {
                            // keep escapes like \n and \l for the label pass
                            s.push('\\');
                            s.push(other);
                        }
                    }
                    i += 2;
                    continue;
                }
                s.push(chars[i]);
                i += 1;
            }
            i += 1;
            // "a" + "b" concatenation
            let joins = out.len() >= 2
                && out[out.len() - 1] == Tok::Punct('+')
                && matches!(out[out.len() - 2], Tok::Quoted(_));
            if joins {
                out.pop();
                if let Some(Tok::Quoted(prev)) = out.last_mut() {
                    prev.push_str(&s);
                }
            } else {
                out.push(Tok::Quoted(s));
            }
            continue;
        }
        if c == '<' {
            let mut depth = 0;
            let mut s = String::new();
            while i < chars.len() {
                match chars[i] {
                    '<' => depth += 1,
                    '>' => depth -= 1,
                    _ => {}
                }
                s.push(chars[i]);
                i += 1;
                if depth == 0 {
                    break;
                }
            }
            out.push(Tok::Html(s[1..s.len().saturating_sub(1)].to_string()));
            continue;
        }
        if c.is_alphanumeric()
            || c == '_'
            || c == '.'
            || (c == '-'
                && chars
                    .get(i + 1)
                    .is_some_and(|d| d.is_ascii_digit() || *d == '.'))
            || (c as u32) > 127
        {
            let start = i;
            i += 1;
            while i < chars.len()
                && (chars[i].is_alphanumeric()
                    || chars[i] == '_'
                    || chars[i] == '.'
                    || (chars[i] as u32) > 127)
            {
                i += 1;
            }
            out.push(Tok::Id(chars[start..i].iter().collect()));
            continue;
        }
        if "{}[]=;,:+".contains(c) {
            out.push(Tok::Punct(c));
            i += 1;
            continue;
        }
        return Err(format!("Not a DOT graph (unexpected `{c}`)."));
    }
    Ok(out)
}

type Attrs = Vec<(String, String)>;

struct Scope {
    node: Attrs,
    edge: Attrs,
    cluster: Option<usize>,
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
    directed: bool,
    g: GraphDiagram,
    index: HashMap<String, usize>,
    node_attrs: Vec<Attrs>,
    graph_attrs: Attrs,
    scopes: Vec<Scope>,
    edges: Vec<(usize, usize, Attrs)>,
    cluster_attrs: Vec<Attrs>,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }
    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(&Tok::Punct(c)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn id(&mut self) -> Option<String> {
        match self.peek()? {
            Tok::Id(s) | Tok::Quoted(s) | Tok::Html(s) => {
                let s = s.clone();
                self.pos += 1;
                Some(s)
            }
            _ => None,
        }
    }
    fn keyword(&self, word: &str) -> bool {
        matches!(self.peek(), Some(Tok::Id(s)) if s.eq_ignore_ascii_case(word))
    }

    fn attr_list(&mut self) -> Attrs {
        let mut out = Vec::new();
        while self.eat('[') {
            while !self.eat(']') {
                let Some(key) = self.id() else {
                    // skip anything unexpected inside the brackets
                    if self.next().is_none() {
                        return out;
                    }
                    continue;
                };
                let value = if self.eat('=') {
                    self.id().unwrap_or_default()
                } else {
                    "true".into()
                };
                out.push((key, value));
                let _ = self.eat(',') || self.eat(';');
            }
        }
        out
    }

    fn node_ref(&mut self, name: &str) -> usize {
        let ix = self.g.node_indexed(name, &mut self.index);
        if ix == self.node_attrs.len() {
            let mut attrs = Vec::new();
            for scope in &self.scopes {
                attrs.extend(scope.node.iter().cloned());
            }
            self.node_attrs.push(attrs);
            self.g.nodes[ix].cluster = self.scopes.iter().rev().find_map(|s| s.cluster);
        }
        ix
    }

    /// A statement list up to `}`; returns the nodes it mentioned.
    fn stmts(&mut self) -> Vec<usize> {
        let mut members = Vec::new();
        while let Some(tok) = self.peek().cloned() {
            if tok == Tok::Punct('}') {
                self.pos += 1;
                break;
            }
            if self.eat(';') {
                continue;
            }
            if self.keyword("graph") || self.keyword("node") || self.keyword("edge") {
                let Some(Tok::Id(kind)) = self.next() else {
                    break;
                };
                if self.peek() == Some(&Tok::Punct('[')) {
                    let attrs = self.attr_list();
                    match kind.to_ascii_lowercase().as_str() {
                        "node" => {
                            if let Some(scope) = self.scopes.last_mut() {
                                scope.node.extend(attrs);
                            }
                        }
                        "edge" => {
                            if let Some(scope) = self.scopes.last_mut() {
                                scope.edge.extend(attrs);
                            }
                        }
                        _ => self.set_graph_attrs(attrs),
                    }
                    continue;
                }
                // `graph` used as a node name is not valid DOT; treat as one
                self.pos -= 1;
            }
            let first = self.operand(&mut members);
            let Some(mut left) = first else {
                // `key = value` at statement level
                self.pos += 1;
                continue;
            };
            if self.eat('=') {
                // a graph attribute: the "operand" was its key
                let key = match &left[..] {
                    [ix] => self.g.nodes[*ix].id.clone(),
                    _ => String::new(),
                };
                let value = self.id().unwrap_or_default();
                // undo the node the key created
                if let [ix] = &left[..]
                    && *ix + 1 == self.g.nodes.len()
                    && self.node_attrs.len() == self.g.nodes.len()
                {
                    let id = self.g.nodes.pop().map(|n| n.id).unwrap_or_default();
                    self.node_attrs.pop();
                    self.index.remove(&id);
                    members.retain(|m| m != ix);
                }
                self.set_graph_attrs(vec![(key, value)]);
                continue;
            }
            let mut chain: Vec<Vec<usize>> = vec![left.clone()];
            while matches!(self.peek(), Some(Tok::Arrow) | Some(Tok::Line)) {
                self.pos += 1;
                match self.operand(&mut members) {
                    Some(right) => {
                        chain.push(right.clone());
                        left = right;
                    }
                    None => break,
                }
            }
            let attrs = self.attr_list();
            if chain.len() == 1 {
                for &ix in &left {
                    self.node_attrs[ix].extend(attrs.iter().cloned());
                }
            } else {
                let mut edge_attrs: Attrs = Vec::new();
                for scope in &self.scopes {
                    edge_attrs.extend(scope.edge.iter().cloned());
                }
                edge_attrs.extend(attrs);
                for pair in chain.windows(2) {
                    for &a in &pair[0] {
                        for &b in &pair[1] {
                            self.edges.push((a, b, edge_attrs.clone()));
                        }
                    }
                }
            }
        }
        members
    }

    /// A node id (with an ignored port) or a subgraph: the nodes it names.
    fn operand(&mut self, members: &mut Vec<usize>) -> Option<Vec<usize>> {
        if self.keyword("subgraph") || self.peek() == Some(&Tok::Punct('{')) {
            let mut name = String::new();
            if self.keyword("subgraph") {
                self.pos += 1;
                if self.peek() != Some(&Tok::Punct('{')) {
                    name = self.id().unwrap_or_default();
                }
            }
            if !self.eat('{') {
                return Some(Vec::new());
            }
            let parent = self.scopes.last().and_then(|s| s.cluster);
            let cluster = if name.starts_with("cluster") {
                self.g.clusters.push(GCluster {
                    parent,
                    ..Default::default()
                });
                self.cluster_attrs.push(Vec::new());
                Some(self.g.clusters.len() - 1)
            } else {
                parent
            };
            self.scopes.push(Scope {
                node: Vec::new(),
                edge: Vec::new(),
                cluster,
            });
            let inner = self.stmts();
            self.scopes.pop();
            members.extend(inner.iter().copied());
            return Some(inner);
        }
        let name = match self.peek()? {
            Tok::Id(_) | Tok::Quoted(_) | Tok::Html(_) => self.id()?,
            _ => return None,
        };
        // port and compass point
        if self.eat(':') {
            self.id();
            if self.eat(':') {
                self.id();
            }
        }
        let ix = self.node_ref(&name);
        members.push(ix);
        Some(vec![ix])
    }

    fn set_graph_attrs(&mut self, attrs: Attrs) {
        match self.scopes.last().and_then(|s| s.cluster) {
            Some(c) if self.scopes.len() > 1 => self.cluster_attrs[c].extend(attrs),
            _ => self.graph_attrs.extend(attrs),
        }
    }
}

fn get<'a>(attrs: &'a Attrs, key: &str) -> Option<&'a str> {
    attrs
        .iter()
        .rev()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
}

/// A label's escapes resolved (`\n`, `\l`, `\r` break lines, `\N` names
/// the node); HTML labels lose their markup.
fn label_text(raw: &str, node: &str, graph: &str) -> String {
    if raw.trim_start().starts_with('<')
        || raw.contains("</")
        || raw.contains("<br")
        || raw.contains("<BR")
    {
        return html_label(raw);
    }
    let mut out = String::new();
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') | Some('l') | Some('r') => out.push('\n'),
            Some('N') => out.push_str(node),
            Some('G') => out.push_str(graph),
            Some('E') | Some('T') | Some('H') => {}
            Some(other) => out.push(other),
            None => {}
        }
    }
    out.trim_end_matches('\n').to_string()
}

fn html_label(raw: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let mut tag = String::new();
    for c in raw.chars() {
        if in_tag {
            if c == '>' {
                in_tag = false;
                let t = tag.trim_start_matches('/').to_ascii_lowercase();
                if t.starts_with("br") || t.starts_with("tr") && tag.starts_with('/') {
                    out.push('\n');
                } else if (t.starts_with("td") || t.starts_with("th")) && tag.starts_with('/') {
                    out.push(' ');
                }
                tag.clear();
            } else {
                tag.push(c);
            }
        } else if c == '<' {
            in_tag = true;
        } else {
            out.push(c);
        }
    }
    let out = out
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&nbsp;", " ");
    out.lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The fields of a record label (`<f0> a | {b | c}`), ports dropped.
fn record_fields(label: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut cur = String::new();
    let mut escaped = false;
    for c in label.chars() {
        if escaped {
            cur.push(c);
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '|' | '{' | '}' => {
                let f = strip_port(&cur);
                if !f.is_empty() {
                    fields.push(f);
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    let f = strip_port(&cur);
    if !f.is_empty() {
        fields.push(f);
    }
    fields
}

fn strip_port(s: &str) -> String {
    let s = s.trim();
    let s = match (s.starts_with('<'), s.find('>')) {
        (true, Some(end)) => s[end + 1..].trim(),
        _ => s,
    };
    s.replace("\\n", "\n")
        .replace("\\l", "\n")
        .trim()
        .to_string()
}

fn arrow(name: &str) -> Option<&'static str> {
    let name = name.trim_start_matches('l').trim_start_matches('r');
    Some(match name {
        "none" => return None,
        "normal" | "" => "arrow",
        "vee" | "open" => "open",
        "empty" | "onormal" => "triangle",
        "diamond" => "diamond",
        "odiamond" | "ediamond" => "odiamond",
        "dot" => "dot",
        "odot" => "circle",
        "tee" => "bar",
        "inv" | "box" | "obox" | "crow" => "crow",
        _ => "arrow",
    })
}

/// Parses DOT text into a diagram.
pub fn parse(src: &str) -> Result<GraphDiagram, String> {
    let toks = tokenize(src)?;
    let mut p = Parser {
        toks,
        pos: 0,
        directed: false,
        g: GraphDiagram::default(),
        index: HashMap::new(),
        node_attrs: Vec::new(),
        graph_attrs: Vec::new(),
        scopes: vec![Scope {
            node: Vec::new(),
            edge: Vec::new(),
            cluster: None,
        }],
        edges: Vec::new(),
        cluster_attrs: Vec::new(),
    };
    if p.keyword("strict") {
        p.pos += 1;
    }
    if p.keyword("digraph") {
        p.directed = true;
    } else if !p.keyword("graph") {
        return Err("Not a DOT graph (no `graph` or `digraph`).".into());
    }
    p.pos += 1;
    let graph_name = if p.peek() != Some(&Tok::Punct('{')) {
        p.id().unwrap_or_default()
    } else {
        String::new()
    };
    if !p.eat('{') {
        return Err("Not a DOT graph (no `{`).".into());
    }
    p.stmts();
    let mut g = p.g;
    g.dir = match get(&p.graph_attrs, "rankdir")
        .map(str::to_ascii_uppercase)
        .as_deref()
    {
        Some("LR") => Dir::LeftRight,
        Some("RL") => Dir::RightLeft,
        Some("BT") => Dir::BottomTop,
        _ => Dir::TopBottom,
    };
    if let Some(label) = get(&p.graph_attrs, "label") {
        let t = label_text(label, "", &graph_name);
        if !t.is_empty() {
            g.title = Some(t);
        }
    }
    for (c, attrs) in p.cluster_attrs.iter().enumerate() {
        let cl = &mut g.clusters[c];
        cl.label = get(attrs, "label")
            .map(|l| label_text(l, "", &graph_name))
            .unwrap_or_default();
        let style = get(attrs, "style").unwrap_or("");
        if let Some(color) = get(attrs, "color").or_else(|| get(attrs, "pencolor")) {
            cl.stroke = Some(super::svg::color(color, "#57606a"));
        }
        if style.contains("filled")
            && let Some(fill) = get(attrs, "fillcolor")
                .or_else(|| get(attrs, "color"))
                .or_else(|| get(attrs, "bgcolor"))
        {
            cl.fill = Some(super::svg::color(fill, "#f6f8fa"));
        } else if let Some(bg) = get(attrs, "bgcolor") {
            cl.fill = Some(super::svg::color(bg, "#f6f8fa"));
        }
    }
    for (ix, attrs) in p.node_attrs.iter().enumerate() {
        let node: &mut GNode = &mut g.nodes[ix];
        let id = node.id.clone();
        let style = get(attrs, "style").unwrap_or("").to_ascii_lowercase();
        let raw_label = get(attrs, "label").unwrap_or("\\N");
        let shape_name = get(attrs, "shape")
            .unwrap_or("ellipse")
            .to_ascii_lowercase();
        node.label = label_text(raw_label, &id, &graph_name);
        node.shape = match shape_name.as_str() {
            "box" | "rect" | "rectangle" | "square" | "box3d" | "component" | "folder" | "tab"
            | "msquare" | "underline" | "cds" => {
                if style.contains("rounded") {
                    Shape::Rounded
                } else {
                    Shape::Rect
                }
            }
            "circle" => Shape::Circle,
            "doublecircle" | "mcircle" => Shape::DoubleCircle,
            "diamond" | "mdiamond" => Shape::Diamond,
            "hexagon" | "octagon" | "doubleoctagon" | "tripleoctagon" => Shape::Hexagon,
            "parallelogram" => Shape::Parallelogram(true),
            "trapezium" => Shape::Trapezoid(true),
            "invtrapezium" => Shape::Trapezoid(false),
            "cylinder" => Shape::Cylinder,
            "note" => Shape::Note,
            "point" => Shape::Point,
            "plaintext" | "plain" | "none" => Shape::Plain,
            "record" | "mrecord" => {
                let fields = record_fields(raw_label.trim());
                let fields = if fields.is_empty() {
                    vec![id.clone()]
                } else {
                    fields
                };
                Shape::Compartments(fields.into_iter().map(|f| vec![f]).collect())
            }
            _ => Shape::Ellipse,
        };
        let color = get(attrs, "color");
        node.stroke = Some(super::svg::color(color.unwrap_or("black"), "#000000"));
        node.fill = Some(if style.contains("filled") {
            super::svg::color(
                get(attrs, "fillcolor").or(color).unwrap_or("lightgrey"),
                "#d3d3d3",
            )
        } else {
            "#ffffff".into()
        });
        if let Some(fc) = get(attrs, "fontcolor") {
            node.text = Some(super::svg::color(fc, "#000000"));
        }
        node.dashed = style.contains("dashed") || style.contains("dotted");
        if style.contains("invis") {
            node.shape = Shape::Plain;
            node.label.clear();
        }
    }
    for (a, b, attrs) in p.edges {
        let mut e = GEdge::new(a, b);
        e.label = get(&attrs, "label")
            .map(|l| label_text(l, "", &graph_name))
            .unwrap_or_default();
        let dir = get(&attrs, "dir").unwrap_or(if p.directed { "forward" } else { "none" });
        let head = arrow(get(&attrs, "arrowhead").unwrap_or("normal"));
        let tail = arrow(get(&attrs, "arrowtail").unwrap_or("normal"));
        (e.head, e.tail) = match dir {
            "back" => (None, tail),
            "both" => (head, tail),
            "none" => (None, None),
            _ => (head, None),
        };
        let style = get(&attrs, "style").unwrap_or("").to_ascii_lowercase();
        e.style = if style.contains("invis") {
            LineStyle::Invisible
        } else if style.contains("dashed") {
            LineStyle::Dashed
        } else if style.contains("dotted") {
            LineStyle::Dotted
        } else if style.contains("bold")
            || get(&attrs, "penwidth")
                .and_then(|w| w.parse::<f64>().ok())
                .is_some_and(|w| w >= 2.0)
        {
            LineStyle::Thick
        } else {
            LineStyle::Solid
        };
        e.color = Some(super::svg::color(
            get(&attrs, "color").unwrap_or("black"),
            "#000000",
        ));
        e.minlen = get(&attrs, "minlen")
            .and_then(|m| m.parse().ok())
            .unwrap_or(1)
            .min(10);
        if get(&attrs, "constraint") == Some("false") {
            e.minlen = 0;
        }
        g.edges.push(e);
    }
    if g.nodes.is_empty() {
        return Err("The graph has no nodes.".into());
    }
    if g.nodes.len() > 5_000 || g.edges.len() > 20_000 {
        return Err("The graph is too large to draw.".into());
    }
    Ok(g)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_digraph() {
        let g = parse(
            r##"// comment
digraph G {
  rankdir=LR; label="Pipeline";
  node [shape=box, style="rounded,filled", fillcolor="#eeeeee"];
  subgraph cluster_a { label = "Build"; compile -> test; }
  test -> deploy [label="on main", style=dashed];
  deploy [shape=doublecircle, label="Ship\nit"];
  a -> {b c};
}"##,
        )
        .unwrap();
        assert_eq!(g.dir, Dir::LeftRight);
        assert_eq!(g.title.as_deref(), Some("Pipeline"));
        let ids: Vec<&str> = g.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["compile", "test", "deploy", "a", "b", "c"]);
        assert_eq!(g.nodes[0].shape, Shape::Rounded);
        assert_eq!(g.nodes[0].cluster, Some(0));
        assert_eq!(g.clusters[0].label, "Build");
        assert_eq!(g.nodes[2].shape, Shape::DoubleCircle);
        assert_eq!(g.nodes[2].label, "Ship\nit");
        assert_eq!(g.edges.len(), 4);
        assert_eq!(g.edges[1].label, "on main");
        assert_eq!(g.edges[1].style, LineStyle::Dashed);
        assert_eq!(g.edges[2].from, 3);
        assert_eq!(g.edges[3].to, 5);
        let svg = super::super::graph::render(&g);
        assert!(String::from_utf8(svg).unwrap().contains(">Pipeline<"));
    }

    #[test]
    fn undirected_records_and_html() {
        let g = parse(
            r#"graph { n [shape=record, label="<f0> left|<f1> mid|right"]; h [label=<<b>bold</b><br/>line>]; n -- h }"#,
        )
        .unwrap();
        assert_eq!(
            g.nodes[0].shape,
            Shape::Compartments(vec![
                vec!["left".into()],
                vec!["mid".into()],
                vec!["right".into()]
            ])
        );
        assert_eq!(g.nodes[1].label, "bold\nline");
        assert_eq!(g.edges[0].head, None);
    }

    #[test]
    fn refuses_other_text() {
        assert!(parse("hello world").is_err());
    }
}
