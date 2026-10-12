//! Mermaid diagrams (`.mmd`, `.mermaid`), drawn by Corvene: flowcharts,
//! state, class and ER diagrams go through the layered layout; sequence
//! diagrams, pie charts, gantt charts and mind maps have layouts of their
//! own. Other kinds (journey, timeline, gitGraph, quadrant, …) are not
//! drawn: the diff stays git's text diff.
//!
//! This is a reading of Mermaid's syntax, not its renderer: the pictures
//! follow the diagram's structure and labels, not Mermaid's exact look.

mod classes;
mod er;
mod flowchart;
mod gantt;
mod mindmap;
mod pie;
mod sequence;
mod state;

/// The diagram's lines after the front matter and `%%` comments and
/// directives, with the front matter's `title`.
pub(crate) struct Source {
    pub lines: Vec<String>,
    pub title: Option<String>,
}

fn preprocess(src: &str) -> Source {
    let src = src.trim_start_matches('\u{feff}');
    let mut title = None;
    let mut body = src;
    // YAML front matter
    if let Some(rest) = src.trim_start().strip_prefix("---")
        && let Some(end) = rest.find("\n---")
    {
        for line in rest[..end].lines() {
            if let Some(t) = line.trim().strip_prefix("title:") {
                let t = t.trim().trim_matches('"').trim_matches('\'');
                if !t.is_empty() {
                    title = Some(t.to_string());
                }
            }
        }
        body = &rest[end + 4..];
    }
    let mut lines = Vec::new();
    for line in body.lines() {
        let t = line.trim_end();
        let trimmed = t.trim_start();
        if trimmed.starts_with("%%") {
            continue;
        }
        // a trailing `%%` comment (not inside a quoted label)
        let t = match t.find("%%") {
            Some(ix) if t[..ix].matches('"').count() % 2 == 0 => &t[..ix],
            _ => t,
        };
        lines.push(t.to_string());
    }
    Source { lines, title }
}

/// The kind word that starts a diagram (`flowchart`, `sequenceDiagram`).
fn kind_of(lines: &[String]) -> Option<(usize, String)> {
    lines
        .iter()
        .enumerate()
        .find(|(_, l)| !l.trim().is_empty())
        .map(|(ix, l)| (ix, l.trim().to_string()))
}

/// The picture of a Mermaid diagram.
pub fn render(src: &str) -> Result<Vec<u8>, String> {
    let source = preprocess(src);
    let (at, first) = kind_of(&source.lines).ok_or("The diagram is empty.")?;
    let word = first
        .split(|c: char| c.is_whitespace() || c == ';')
        .next()
        .unwrap_or("")
        .to_string();
    let rest: Vec<String> = source.lines[at + 1..].to_vec();
    let header = first[word.len()..].trim().to_string();
    let title = source.title.clone();
    match word.as_str() {
        "graph" | "flowchart" | "flowchart-elk" => {
            let mut g = flowchart::parse(&header, &rest)?;
            g.title = title.or(g.title);
            Ok(super::graph::render(&g))
        }
        "stateDiagram" | "stateDiagram-v2" => {
            let mut g = state::parse(&rest)?;
            g.title = title.or(g.title);
            Ok(super::graph::render(&g))
        }
        "classDiagram" | "classDiagram-v2" => {
            let mut g = classes::parse(&rest)?;
            g.title = title.or(g.title);
            Ok(super::graph::render(&g))
        }
        "erDiagram" => {
            let mut g = er::parse(&rest)?;
            g.title = title.or(g.title);
            Ok(super::graph::render(&g))
        }
        "sequenceDiagram" => sequence::render(&rest, title),
        "pie" => pie::render(&header, &rest, title),
        "gantt" => gantt::render(&rest, title),
        "mindmap" => mindmap::render(&rest, title),
        other => Err(format!("Mermaid `{other}` diagrams are not drawn.")),
    }
}

/// A label with Mermaid's quoting, entity codes (`#quot;`, `#9829;`),
/// Markdown backticks and `<br>` resolved.
pub(crate) fn label(raw: &str) -> String {
    let mut s = raw.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s = &s[1..s.len() - 1];
    }
    if s.len() >= 2 && s.starts_with('`') && s.ends_with('`') {
        s = &s[1..s.len() - 1];
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(hash) = rest.find('#') {
        out.push_str(&rest[..hash]);
        let tail = &rest[hash + 1..];
        let end = tail.find(';').filter(|e| *e <= 10);
        let decoded = end.and_then(|e| {
            let code = &tail[..e];
            match code {
                "quot" => Some('"'),
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "nbsp" => Some(' '),
                _ => code.parse::<u32>().ok().and_then(char::from_u32),
            }
        });
        match (decoded, end) {
            (Some(ch), Some(e)) => {
                out.push(ch);
                rest = &tail[e + 1..];
            }
            _ => {
                out.push('#');
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    let mut out = out
        .replace("<br/>", "\n")
        .replace("<br />", "\n")
        .replace("<br>", "\n")
        .replace("\\n", "\n");
    // Markdown emphasis markers in "`…`" strings
    for marker in ["**", "__"] {
        out = out.replace(marker, "");
    }
    // other HTML tags lose their markup
    let mut plain = String::with_capacity(out.len());
    let mut in_tag = false;
    for c in out.chars() {
        match c {
            '<' if !in_tag => in_tag = true,
            '>' if in_tag => in_tag = false,
            c if !in_tag => plain.push(c),
            _ => {}
        }
    }
    if in_tag {
        // a lone `<` was text after all
        return out;
    }
    plain
}

/// `key:value,key:value` style attributes (`classDef`, `style`) as fill,
/// stroke and text colors.
pub(crate) fn style_colors(spec: &str) -> (Option<String>, Option<String>, Option<String>, bool) {
    let mut fill = None;
    let mut stroke = None;
    let mut text = None;
    let mut dashed = false;
    for part in spec.split([',', ';']) {
        let Some((k, v)) = part.split_once(':') else {
            continue;
        };
        let v = v.trim();
        match k.trim() {
            "fill" => fill = Some(super::svg::color(v, "#ddf4ff")),
            "stroke" => stroke = Some(super::svg::color(v, "#0969da")),
            "color" => text = Some(super::svg::color(v, "#24292f")),
            "stroke-dasharray" => dashed = true,
            _ => {}
        }
    }
    (fill, stroke, text, dashed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_resolve_codes_and_breaks() {
        assert_eq!(label("\"A #quot;quoted#quot; #9829;\""), "A \"quoted\" ♥");
        assert_eq!(label("one<br/>two"), "one\ntwo");
        assert_eq!(label("\"`**bold** text`\""), "bold text");
        assert_eq!(label("a < b"), "a < b");
    }

    #[test]
    fn front_matter_and_comments() {
        let s = preprocess("---\ntitle: Flow\n---\n%% note\nflowchart LR %% trailing\n  A-->B\n");
        assert_eq!(s.title.as_deref(), Some("Flow"));
        assert_eq!(kind_of(&s.lines).unwrap().1, "flowchart LR");
        assert!(render("journey\n  title x").is_err());
    }
}
