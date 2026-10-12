//! Mermaid entity relationship diagrams: entities with their attributes
//! (type, name, keys, comment), relationships with crow's foot ends,
//! identifying (solid) or not (dashed), and their labels.

use std::collections::HashMap;

use super::super::graph::{GEdge, GNode, GraphDiagram, LineStyle, Shape};
use super::flowchart::direction;
use super::label;

/// The marker for one end of a relationship (`||`, `|o`, `}o`, `}|` and
/// their mirrors).
fn end(card: &str) -> Option<&'static str> {
    Some(match card {
        "||" => "bar",
        "|o" | "o|" => "circle",
        "}o" | "o{" | "}|" | "|{" => "crow",
        _ => return None,
    })
}

pub fn parse(lines: &[String]) -> Result<GraphDiagram, String> {
    let mut g = GraphDiagram::default();
    let mut attrs: Vec<Vec<String>> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut open: Option<usize> = None;
    let mut entity = |g: &mut GraphDiagram, attrs: &mut Vec<Vec<String>>, raw: &str| -> usize {
        let raw = raw.trim();
        // `CUSTOMER["Customer"]` gives an alias
        let (id, shown) = match raw.find('[') {
            Some(b) => (&raw[..b], label(raw[b + 1..].trim_end_matches(']'))),
            None => (raw, raw.trim_matches('"').to_string()),
        };
        let id = id.trim().trim_matches('"');
        if let Some(&ix) = index.get(id) {
            return ix;
        }
        g.nodes.push(GNode {
            id: id.to_string(),
            label: shown,
            ..Default::default()
        });
        attrs.push(Vec::new());
        let ix = g.nodes.len() - 1;
        index.insert(id.to_string(), ix);
        ix
    };
    for line in lines {
        let s = line.trim();
        if s.is_empty()
            || s.starts_with("accTitle")
            || s.starts_with("accDescr")
            || s.starts_with("style ")
            || s.starts_with("classDef ")
            || s.starts_with("class ")
        {
            continue;
        }
        if let Some(e) = open {
            if s == "}" {
                open = None;
                continue;
            }
            // `string name PK, FK "comment"`
            let (body, comment) = match s.find('"') {
                Some(q) => (&s[..q], Some(s[q..].trim_matches('"').to_string())),
                None => (s, None),
            };
            let mut words = body.split_whitespace();
            let ty = words.next().unwrap_or("");
            let name = words.next().unwrap_or("");
            let keys: Vec<&str> = words
                .flat_map(|w| w.split(','))
                .filter(|k| !k.is_empty())
                .collect();
            let mut row = format!("{name} : {ty}");
            if !keys.is_empty() {
                row.push_str(&format!("  [{}]", keys.join(", ")));
            }
            if let Some(c) = comment {
                row.push_str(&format!("  “{c}”"));
            }
            attrs[e].push(row);
            continue;
        }
        if let Some(d) = s.strip_prefix("direction ") {
            if let Some(d) = direction(d) {
                g.dir = d;
            }
            continue;
        }
        if let Some(name) = s.strip_suffix('{') {
            open = Some(entity(&mut g, &mut attrs, name));
            continue;
        }
        // `A ||--o{ B : label`
        let (rel, text) = match s.split_once(':') {
            Some((r, t)) => (r.trim(), label(t)),
            None => (s, String::new()),
        };
        let core = rel.find("--").or_else(|| rel.find(".."));
        if let Some(at) = core
            && at >= 2
            && rel.len() >= at + 4
            && rel.is_char_boundary(at - 2)
            && rel.is_char_boundary(at + 4)
        {
            let left_card = &rel[at - 2..at];
            let right_card = &rel[at + 2..at + 4];
            if let (Some(tail), Some(head)) = (end(left_card), end(right_card)) {
                let a = entity(&mut g, &mut attrs, &rel[..at - 2]);
                let b = entity(&mut g, &mut attrs, &rel[at + 4..]);
                let mut e = GEdge::new(a, b);
                e.tail = Some(tail);
                e.head = Some(head);
                e.label = text;
                e.style = if rel[at..].starts_with("..") {
                    LineStyle::Dashed
                } else {
                    LineStyle::Solid
                };
                g.edges.push(e);
                continue;
            }
        }
        entity(&mut g, &mut attrs, s);
    }
    if g.nodes.is_empty() {
        return Err("The ER diagram has no entities.".into());
    }
    for (node, rows) in g.nodes.iter_mut().zip(attrs) {
        let mut parts = vec![vec![node.label.clone()]];
        if !rows.is_empty() {
            parts.push(rows);
        }
        node.shape = Shape::Compartments(parts);
    }
    Ok(g)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entities_and_relationships() {
        let lines: Vec<String> = "    CUSTOMER ||--o{ ORDER : places\n    ORDER ||--|{ LINE-ITEM : contains\n    CUSTOMER }|..|{ DELIVERY-ADDRESS : uses\n    CUSTOMER {\n        string name PK \"full name\"\n        int age\n    }"
            .lines()
            .map(str::to_string)
            .collect();
        let g = parse(&lines).unwrap();
        assert_eq!(g.nodes.len(), 4);
        assert_eq!(g.edges[0].tail, Some("bar"));
        assert_eq!(g.edges[0].head, Some("crow"));
        assert_eq!(g.edges[0].label, "places");
        assert_eq!(g.edges[2].style, LineStyle::Dashed);
        let Shape::Compartments(parts) = &g.nodes[0].shape else {
            panic!()
        };
        assert_eq!(
            parts[1],
            vec!["name : string  [PK]  “full name”", "age : int"]
        );
    }
}
