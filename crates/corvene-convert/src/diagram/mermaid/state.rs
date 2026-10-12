//! Mermaid state diagrams (`stateDiagram`, `stateDiagram-v2`): states and
//! their descriptions, `[*]` start and end points (one pair per composite
//! state), transitions with labels, composite states as clusters, forks,
//! joins and choices, notes.

use std::collections::HashMap;

use super::super::graph::{GCluster, GEdge, GNode, GraphDiagram, LineStyle, Shape};
use super::flowchart::{direction, statements};
use super::{label, style_colors};

struct Parser {
    g: GraphDiagram,
    index: HashMap<String, usize>,
    /// Open composite states (cluster index, state id).
    stack: Vec<(usize, String)>,
    descriptions: HashMap<usize, Vec<String>>,
    classes: HashMap<String, String>,
    notes: usize,
    /// Composite states: their cluster and id.
    composites: Vec<(usize, String)>,
}

impl Parser {
    fn scope(&self) -> String {
        self.stack
            .last()
            .map(|(_, id)| id.clone())
            .unwrap_or_default()
    }

    fn state(&mut self, raw: &str, as_target: bool) -> usize {
        let raw = raw.trim();
        let (name, class) = match raw.split_once(":::") {
            Some((n, c)) => (n.trim(), Some(c.trim().to_string())),
            None => (raw, None),
        };
        let ix = if name == "[*]" {
            let id = format!(
                "[*]{}{}",
                self.scope(),
                if as_target { "end" } else { "start" }
            );
            let ix = self.g.node_indexed(&id, &mut self.index);
            let node = &mut self.g.nodes[ix];
            node.label.clear();
            node.shape = if as_target { Shape::End } else { Shape::Point };
            ix
        } else {
            let fresh = !self.index.contains_key(name);
            let ix = self.g.node_indexed(name, &mut self.index);
            if fresh {
                self.g.nodes[ix].shape = Shape::Rounded;
            }
            ix
        };
        if self.g.nodes[ix].cluster.is_none()
            && let Some((c, _)) = self.stack.last()
        {
            self.g.nodes[ix].cluster = Some(*c);
        }
        if let Some(class) = class
            && let Some(spec) = self.classes.get(&class).cloned()
        {
            let (fill, stroke, text, _) = style_colors(&spec);
            let node = &mut self.g.nodes[ix];
            node.fill = fill.or(node.fill.take());
            node.stroke = stroke.or(node.stroke.take());
            node.text = text.or(node.text.take());
        }
        ix
    }
}

pub fn parse(lines: &[String]) -> Result<GraphDiagram, String> {
    let mut p = Parser {
        g: GraphDiagram::default(),
        index: HashMap::new(),
        stack: Vec::new(),
        descriptions: HashMap::new(),
        classes: HashMap::new(),
        notes: 0,
        composites: Vec::new(),
    };
    let mut note: Option<(usize, Vec<String>)> = None;
    for line in lines {
        let raw = line.trim();
        if let Some((target, text)) = note.as_mut() {
            if raw == "end note" {
                let target = *target;
                let text = text.join("\n");
                add_note(&mut p, target, &text);
                note = None;
            } else {
                text.push(raw.to_string());
            }
            continue;
        }
        for stmt in statements(raw) {
            let s = stmt.trim();
            if s.is_empty()
                || s == "--"
                || s.starts_with("accTitle")
                || s.starts_with("accDescr")
                || s.starts_with("click ")
            {
                continue;
            }
            if s == "}" {
                p.stack.pop();
                continue;
            }
            if let Some(d) = s.strip_prefix("direction ") {
                if p.stack.is_empty()
                    && let Some(d) = direction(d)
                {
                    p.g.dir = d;
                }
                continue;
            }
            if let Some(rest) = s.strip_prefix("classDef ") {
                let mut parts = rest.splitn(2, char::is_whitespace);
                let name = parts.next().unwrap_or("").to_string();
                p.classes
                    .insert(name, parts.next().unwrap_or("").to_string());
                continue;
            }
            if let Some(rest) = s.strip_prefix("class ") {
                let mut parts = rest.rsplitn(2, char::is_whitespace);
                let class = parts.next().unwrap_or("").to_string();
                for id in parts.next().unwrap_or("").split(',') {
                    p.state(&format!("{}:::{class}", id.trim()), false);
                }
                continue;
            }
            if let Some(rest) = s.strip_prefix("note ") {
                // `note right of A : text` or a block up to `end note`
                let rest = rest
                    .trim_start_matches("right of ")
                    .trim_start_matches("left of ")
                    .trim_start_matches("over ");
                match rest.split_once(':') {
                    Some((target, text)) => {
                        let t = p.state(target, false);
                        add_note(&mut p, t, &label(text));
                    }
                    None => {
                        let t = p.state(rest, false);
                        note = Some((t, Vec::new()));
                    }
                }
                continue;
            }
            if let Some(rest) = s.strip_prefix("state ") {
                let rest = rest.trim();
                // `state "Long name" as A`, `state A <<fork>>`, `state A {`
                let opens = rest.ends_with('{');
                let rest = rest.trim_end_matches('{').trim();
                let (id, title) = if let Some(q) = rest.strip_prefix('"') {
                    let end = q.find('"').unwrap_or(q.len());
                    let title = q[..end].to_string();
                    let after = q[end..].trim_start_matches('"').trim();
                    let id = after
                        .strip_prefix("as ")
                        .map(str::trim)
                        .unwrap_or(&title)
                        .to_string();
                    (id, Some(title))
                } else if let Some((id, title)) = rest.split_once(" as ") {
                    // `state A as "Long name"`
                    (
                        title.trim().trim_matches('"').to_string(),
                        Some(id.trim().trim_matches('"').to_string()),
                    )
                } else {
                    (rest.to_string(), None)
                };
                let (id, kind) = match id.split_once("<<") {
                    Some((i, k)) => (
                        i.trim().to_string(),
                        Some(k.trim_end_matches(">>").trim().to_string()),
                    ),
                    None => (id, None),
                };
                if opens {
                    p.g.clusters.push(GCluster {
                        label: title.clone().unwrap_or_else(|| id.clone()),
                        parent: p.stack.last().map(|(c, _)| *c),
                        ..Default::default()
                    });
                    let c = p.g.clusters.len() - 1;
                    p.composites.push((c, id.clone()));
                    p.stack.push((c, id.clone()));
                    continue;
                }
                let ix = p.state(&id, false);
                if let Some(title) = title {
                    p.g.nodes[ix].label = label(&title);
                }
                match kind.as_deref() {
                    Some("fork") | Some("join") => p.g.nodes[ix].shape = Shape::Bar,
                    Some("choice") => {
                        p.g.nodes[ix].shape = Shape::Diamond;
                        p.g.nodes[ix].label.clear();
                    }
                    _ => {}
                }
                continue;
            }
            // transitions: `A --> B : label`
            if let Some(arrow) = s.find("-->") {
                let left = &s[..arrow];
                let right = &s[arrow + 3..];
                let (target, text) = match right.split_once(':') {
                    Some((t, l)) => (t, label(l)),
                    None => (right, String::new()),
                };
                let a = p.state(left, false);
                let b = p.state(target, true);
                let mut e = GEdge::new(a, b);
                e.label = text;
                p.g.edges.push(e);
                continue;
            }
            // `A : description`
            if let Some((id, text)) = s.split_once(':') {
                let ix = p.state(id, false);
                p.descriptions.entry(ix).or_default().push(label(text));
                continue;
            }
            p.state(s, false);
        }
    }
    // composite states that also got edges as plain states stay boxes
    for (ix, lines) in p.descriptions {
        let node = &mut p.g.nodes[ix];
        node.label = format!("{}\n{}", node.label, lines.join("\n"));
    }
    // a transition into a composite state goes to its start point (or
    // first state), one out of it leaves from its end point
    let mut into: HashMap<usize, usize> = HashMap::new();
    let mut out_of: HashMap<usize, usize> = HashMap::new();
    for (c, id) in &p.composites {
        let Some(&ix) = p.index.get(id) else {
            continue;
        };
        let members: Vec<usize> = (0..p.g.nodes.len())
            .filter(|&n| n != ix && p.g.nodes[n].cluster == Some(*c))
            .collect();
        let Some(&first) = members.first() else {
            continue;
        };
        let start = p
            .index
            .get(&format!("[*]{id}start"))
            .copied()
            .unwrap_or(first);
        let end = p
            .index
            .get(&format!("[*]{id}end"))
            .copied()
            .unwrap_or(first);
        into.insert(ix, start);
        out_of.insert(ix, end);
    }
    if !into.is_empty() {
        for e in &mut p.g.edges {
            if let Some(&m) = out_of.get(&e.from) {
                e.from = m;
            }
            if let Some(&m) = into.get(&e.to) {
                e.to = m;
            }
        }
        let mut remap = vec![usize::MAX; p.g.nodes.len()];
        let mut keep = Vec::new();
        for (ix, node) in p.g.nodes.drain(..).enumerate() {
            if !into.contains_key(&ix) {
                remap[ix] = keep.len();
                keep.push(node);
            }
        }
        p.g.nodes = keep;
        p.g.edges
            .retain(|e| remap[e.from] != usize::MAX && remap[e.to] != usize::MAX);
        for e in &mut p.g.edges {
            e.from = remap[e.from];
            e.to = remap[e.to];
        }
    }
    if p.g.nodes.is_empty() {
        return Err("The state diagram has no states.".into());
    }
    Ok(p.g)
}

fn add_note(p: &mut Parser, target: usize, text: &str) {
    p.notes += 1;
    p.g.nodes.push(GNode {
        id: format!("note {}", p.notes),
        label: text.to_string(),
        shape: Shape::Note,
        cluster: p.g.nodes[target].cluster,
        ..Default::default()
    });
    let note = p.g.nodes.len() - 1;
    let mut e = GEdge::new(target, note);
    e.head = None;
    e.style = LineStyle::Dotted;
    e.minlen = 0;
    p.g.edges.push(e);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(str::to_string).collect()
    }

    #[test]
    fn states_and_transitions() {
        let g = parse(&lines(
            "    [*] --> Still\n    Still --> Moving : push\n    Moving --> [*]\n    state \"Very still\" as Still\n    Moving : has speed\n    state Active {\n      [*] --> Running\n    }\n    state f <<fork>>\n    note right of Still : resting",
        ))
        .unwrap();
        let n = |id: &str| g.nodes.iter().find(|n| n.id == id).unwrap();
        assert_eq!(n("[*]start").shape, Shape::Point);
        assert_eq!(n("[*]end").shape, Shape::End);
        assert_eq!(n("Still").label, "Very still");
        assert_eq!(n("Moving").label, "Moving\nhas speed");
        assert_eq!(n("Running").cluster, Some(0));
        assert_eq!(n("[*]Activestart").cluster, Some(0));
        assert_eq!(n("f").shape, Shape::Bar);
        assert_eq!(g.edges[1].label, "push");
        assert!(
            g.nodes
                .iter()
                .any(|n| n.shape == Shape::Note && n.label == "resting")
        );
        assert!(!super::super::super::graph::render(&g).is_empty());
    }
}
