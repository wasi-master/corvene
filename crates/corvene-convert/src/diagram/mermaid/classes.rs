//! Mermaid class diagrams: classes with their members (from `class X {…}`
//! blocks and `X : member` lines), generics (`List~T~`), annotations
//! (`<<interface>>`), relationships (inheritance, composition,
//! aggregation, association, dependency, realization, links) with
//! cardinalities and labels, namespaces as clusters.

use std::collections::HashMap;

use super::super::graph::{GCluster, GEdge, GraphDiagram, LineStyle, Shape};
use super::flowchart::direction;
use super::label;

#[derive(Default)]
struct Class {
    title: String,
    annotation: Option<String>,
    fields: Vec<String>,
    methods: Vec<String>,
}

/// `Name~T~` as `Name<T>`.
fn generic(name: &str) -> String {
    let mut out = String::new();
    let mut open = true;
    for c in name.chars() {
        if c == '~' {
            out.push(if open { '<' } else { '>' });
            open = !open;
        } else {
            out.push(c);
        }
    }
    out
}

fn bare(name: &str) -> String {
    let name = name.trim().trim_matches('`');
    name.split('~').next().unwrap_or(name).trim().to_string()
}

/// The markers of a relationship operator's two ends and its line.
fn relation(op: &str) -> Option<(Option<&'static str>, Option<&'static str>, LineStyle)> {
    let style = if op.contains("..") {
        LineStyle::Dashed
    } else if op.contains("--") {
        LineStyle::Solid
    } else {
        return None;
    };
    let (left, right) = {
        let core = if op.contains("..") { ".." } else { "--" };
        let at = op.find(core)?;
        (&op[..at], &op[at + core.len()..])
    };
    let end = |s: &str| -> Option<&'static str> {
        match s.trim_matches(|c| c == '-' || c == '.') {
            "<|" | "|>" => Some("triangle"),
            "*" => Some("diamond"),
            "o" => Some("odiamond"),
            "<" | ">" => Some("open"),
            "()" => Some("circle"),
            _ => None,
        }
    };
    Some((end(left), end(right), style))
}

/// A relationship: its classes, label, end markers and line.
type Relation = (
    usize,
    usize,
    String,
    Option<&'static str>,
    Option<&'static str>,
    LineStyle,
);

pub fn parse(lines: &[String]) -> Result<GraphDiagram, String> {
    let mut classes: Vec<Class> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut cluster_of: HashMap<usize, usize> = HashMap::new();
    let mut g = GraphDiagram::default();
    let mut namespaces: Vec<usize> = Vec::new();
    let mut open_class: Option<usize> = None;
    let mut relations: Vec<Relation> = Vec::new();
    let mut get = |name: &str,
                   classes: &mut Vec<Class>,
                   namespaces: &Vec<usize>,
                   cluster_of: &mut HashMap<usize, usize>|
     -> usize {
        let key = bare(name);
        if let Some(&ix) = index.get(&key) {
            return ix;
        }
        classes.push(Class {
            title: generic(name.trim().trim_matches('`')),
            ..Default::default()
        });
        let ix = classes.len() - 1;
        index.insert(key, ix);
        if let Some(&ns) = namespaces.last() {
            cluster_of.insert(ix, ns);
        }
        ix
    };
    for line in lines {
        let s = line.trim();
        if s.is_empty()
            || s.starts_with("accTitle")
            || s.starts_with("accDescr")
            || s.starts_with("click ")
            || s.starts_with("link ")
            || s.starts_with("callback ")
            || s.starts_with("cssClass ")
            || s.starts_with("style ")
            || s.starts_with("classDef ")
        {
            continue;
        }
        if let Some(c) = open_class {
            if s == "}" {
                open_class = None;
                continue;
            }
            if let Some(a) = s.strip_prefix("<<") {
                classes[c].annotation = Some(a.trim_end_matches(">>").trim().to_string());
                continue;
            }
            add_member(&mut classes[c], s);
            continue;
        }
        if s == "}" {
            namespaces.pop();
            continue;
        }
        if let Some(d) = s.strip_prefix("direction ") {
            if let Some(d) = direction(d) {
                g.dir = d;
            }
            continue;
        }
        if let Some(rest) = s.strip_prefix("namespace ") {
            g.clusters.push(GCluster {
                label: rest.trim_end_matches('{').trim().to_string(),
                parent: namespaces.last().copied(),
                ..Default::default()
            });
            namespaces.push(g.clusters.len() - 1);
            continue;
        }
        if s.starts_with("note ") || s == "note" {
            continue;
        }
        if let Some(rest) = s.strip_prefix("class ") {
            let rest = rest.trim();
            let opens = rest.ends_with('{');
            let mut head = rest.trim_end_matches('{').trim();
            // `class A:::css`, `class A["Label"]`
            if let Some((h, _)) = head.split_once(":::") {
                head = h.trim();
            }
            let (name, shown) = match head.find('[') {
                Some(b) => (&head[..b], Some(label(head[b + 1..].trim_end_matches(']')))),
                None => (head, None),
            };
            let ix = get(name, &mut classes, &namespaces, &mut cluster_of);
            if let Some(shown) = shown {
                classes[ix].title = shown;
            }
            if opens {
                open_class = Some(ix);
            }
            continue;
        }
        // `<<interface>> Shape`
        if let Some(rest) = s.strip_prefix("<<")
            && let Some((a, name)) = rest.split_once(">>")
        {
            let ix = get(name, &mut classes, &namespaces, &mut cluster_of);
            classes[ix].annotation = Some(a.trim().to_string());
            continue;
        }
        // relationships: `A "1" <|-- "*" B : label`
        if let Some((a, op, b, text)) = split_relation(s)
            && let Some((tail, head, style)) = relation(&op)
        {
            let (a_name, a_card) = cardinality(&a, true);
            let (b_name, b_card) = cardinality(&b, false);
            let ia = get(&a_name, &mut classes, &namespaces, &mut cluster_of);
            let ib = get(&b_name, &mut classes, &namespaces, &mut cluster_of);
            let mut text = label(&text);
            if a_card.is_some() || b_card.is_some() {
                let cards = format!(
                    "{} … {}",
                    a_card.unwrap_or_default(),
                    b_card.unwrap_or_default()
                );
                text = if text.is_empty() {
                    cards
                } else {
                    format!("{text}\n{cards}")
                };
            }
            relations.push((ia, ib, text, tail, head, style));
            continue;
        }
        // `A : +member`
        if let Some((name, member)) = s.split_once(" : ").or_else(|| s.split_once(':')) {
            let ix = get(name, &mut classes, &namespaces, &mut cluster_of);
            add_member(&mut classes[ix], member.trim());
            continue;
        }
        get(s, &mut classes, &namespaces, &mut cluster_of);
    }
    if classes.is_empty() {
        return Err("The class diagram has no classes.".into());
    }
    for (ix, class) in classes.into_iter().enumerate() {
        let mut title = Vec::new();
        if let Some(a) = class.annotation {
            title.push(format!("«{a}»"));
        }
        title.push(class.title.clone());
        let mut parts = vec![title];
        parts.push(class.fields);
        parts.push(class.methods);
        g.nodes.push(super::super::graph::GNode {
            id: class.title,
            shape: Shape::Compartments(parts),
            cluster: cluster_of.get(&ix).copied(),
            ..Default::default()
        });
    }
    for (a, b, text, tail, head, style) in relations {
        let mut e = GEdge::new(a, b);
        e.label = text;
        e.tail = tail;
        e.head = head;
        e.style = style;
        g.edges.push(e);
    }
    Ok(g)
}

fn add_member(class: &mut Class, member: &str) {
    let m = member.trim();
    if m.is_empty() {
        return;
    }
    // `$` static and `*` abstract markers trail the member
    let m = generic(m.trim_end_matches(['$', '*']));
    if m.contains('(') {
        class.methods.push(m);
    } else {
        class.fields.push(m);
    }
}

/// The two sides of a relationship line, its operator and label.
fn split_relation(s: &str) -> Option<(String, String, String, String)> {
    let (main, text) = match s.split_once(" : ").or_else(|| {
        // `A --> B:label` (no spaces)
        let colon = s.rfind(':')?;
        (!s[..colon].ends_with(':')).then(|| (&s[..colon], &s[colon + 1..]))
    }) {
        Some((m, t)) => (m.to_string(), t.trim().to_string()),
        None => (s.to_string(), String::new()),
    };
    // the operator is the first run holding `--` or `..`
    let bytes = main.as_bytes();
    let mut start = None;
    for (ix, w) in bytes.windows(2).enumerate() {
        if w == b"--" || w == b".." {
            start = Some(ix);
            break;
        }
    }
    let at = start?;
    let mut l = at;
    while l > 0 && matches!(bytes[l - 1], b'<' | b'|' | b'*' | b'o' | b'(' | b')') {
        // `o` only as a marker when a space precedes it
        if bytes[l - 1] == b'o' && l >= 2 && bytes[l - 2] != b' ' {
            break;
        }
        l -= 1;
    }
    let mut r = at;
    while r < bytes.len() && matches!(bytes[r], b'-' | b'.') {
        r += 1;
    }
    while r < bytes.len() && matches!(bytes[r], b'>' | b'|' | b'*' | b'o' | b'(' | b')') {
        if bytes[r] == b'o' && bytes.get(r + 1).is_some_and(|c| *c != b' ') {
            break;
        }
        r += 1;
    }
    let a = main[..l].trim().to_string();
    let b = main[r..].trim().to_string();
    if a.is_empty() || b.is_empty() {
        return None;
    }
    Some((a, main[l..r].to_string(), b, text))
}

/// `Name "card"` (left side) or `"card" Name` (right side).
fn cardinality(side: &str, left: bool) -> (String, Option<String>) {
    let side = side.trim();
    let Some(q0) = side.find('"') else {
        return (side.to_string(), None);
    };
    let Some(q1) = side[q0 + 1..].find('"').map(|q| q + q0 + 1) else {
        return (side.to_string(), None);
    };
    let card = side[q0 + 1..q1].to_string();
    let name = if left { &side[..q0] } else { &side[q1 + 1..] };
    (name.trim().to_string(), Some(card))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(str::to_string).collect()
    }

    #[test]
    fn classes_and_relations() {
        let g = parse(&lines(
            "    class Animal {\n      <<abstract>>\n      +String name\n      +eat() void\n    }\n    Animal <|-- Duck\n    Duck : +swim()\n    Duck : +int age\n    Zoo \"1\" *-- \"many\" Animal : holds\n    List~T~ ..> Animal\n    <<interface>> Swimmer\n    Duck ..|> Swimmer",
        ))
        .unwrap();
        let n = |id: &str| g.nodes.iter().position(|n| n.id == id).unwrap();
        let Shape::Compartments(parts) = &g.nodes[n("Animal")].shape else {
            panic!()
        };
        assert_eq!(parts[0], vec!["«abstract»", "Animal"]);
        assert_eq!(parts[1], vec!["+String name"]);
        assert_eq!(parts[2], vec!["+eat() void"]);
        let Shape::Compartments(duck) = &g.nodes[n("Duck")].shape else {
            panic!()
        };
        assert_eq!(duck[1], vec!["+int age"]);
        assert_eq!(duck[2], vec!["+swim()"]);
        assert!(g.nodes.iter().any(|n| n.id == "List<T>"));
        let inherit = &g.edges[0];
        assert_eq!((inherit.from, inherit.to), (n("Animal"), n("Duck")));
        assert_eq!(inherit.tail, Some("triangle"));
        let holds = &g.edges[1];
        assert_eq!(holds.tail, Some("diamond"));
        assert_eq!(holds.label, "holds\n1 … many");
        assert_eq!(g.edges[3].head, Some("triangle"));
        assert_eq!(g.edges[3].style, LineStyle::Dashed);
    }
}
