//! Corvene `1334-markdown-preview-diff`: a Markdown file read into blocks
//! for its rendered diff. YAML (`---`) or TOML (`+++`) front matter becomes
//! a code block and each list item is a block of its own, so an edit to one
//! item marks that item only (`crate::block_diff` aligns whole blocks).
//!
//! MDX is read as the common doc sites write it: `import` / `export` lines
//! and `{/* comments */}` are left out, callout components (`<Note>`,
//! `<Warning>`, `<Callout type>`, `<Admonition type>`, `<Aside>` …) and
//! `:::note` fences become GitHub alerts, `<Accordion title>`-style
//! components become `<details>`, other block components show their
//! `title` / `label` as a bold line (their tags left out), self-closing
//! components and `{expressions}` show as code, and indented content inside
//! a component is not a code block (MDX has none).
//!
//! GitHub Desktop shows Markdown as its text diff only; github.com's rich
//! diff renders the front matter as a table and the rest as here.

use crate::markdown::{AlertKind, Block, ParseOptions, parse_with_lines};

/// The blocks of a Markdown file (`mdx`: an MDX file; `math`: typeset
/// `$…$` as GitHub does).
pub fn parse_document(text: &str, mdx: bool, math: bool) -> Vec<Block> {
    parse_document_lines(text, mdx, math).0
}

/// [`parse_document`], with the 1-based line of `text` each block ends on.
pub fn parse_document_lines(text: &str, mdx: bool, math: bool) -> (Vec<Block>, Vec<u32>) {
    let (front, body) = front_matter(text);
    let skipped = text[..text.len() - body.len()].matches('\n').count();
    let (body, map) = if mdx {
        mdx_to_markdown(body, math)
    } else {
        (body.to_string(), Vec::new())
    };
    // a parsed line back to its line in `text`
    let line_of = |line: usize| {
        let body_line = map.get(line).copied().unwrap_or(line);
        (skipped + body_line + 1) as u32
    };
    let mut out = Vec::new();
    let mut lines = Vec::new();
    if let Some((language, code)) = front {
        out.push(Block::CodeBlock {
            language: Some(language.to_string()),
            code: code.to_string(),
        });
        lines.push(skipped.max(1) as u32);
    }
    let (blocks, ends) = parse_with_lines(&body, ParseOptions { math });
    for (block, end) in blocks.into_iter().zip(ends) {
        let end = line_of(end);
        match block {
            Block::List { start, items } if items.len() > 1 => {
                for (i, item) in items.into_iter().enumerate() {
                    out.push(Block::List {
                        start: start.map(|s| s + i as u64),
                        items: vec![item],
                    });
                    lines.push(end);
                }
            }
            block => {
                out.push(block);
                lines.push(end);
            }
        }
    }
    (out, lines)
}

/// Whether `path` is an MDX file.
pub fn is_mdx(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, e)| e.eq_ignore_ascii_case("mdx"))
}

/// The front matter's language and text, and the rest of the document.
fn front_matter(text: &str) -> (Option<(&'static str, &str)>, &str) {
    for (fence, language) in [("---", "yaml"), ("+++", "toml")] {
        let Some(rest) = text
            .strip_prefix(fence)
            .and_then(|r| r.strip_prefix("\r\n").or_else(|| r.strip_prefix('\n')))
        else {
            continue;
        };
        let mut offset = 0;
        for line in rest.split_inclusive('\n') {
            let bare = line.trim_end_matches(['\r', '\n']);
            if bare == fence || (language == "yaml" && bare == "...") {
                let code = rest[..offset].trim_end_matches(['\r', '\n']);
                return (Some((language, code)), &rest[offset + line.len()..]);
            }
            offset += line.len();
        }
    }
    (None, text)
}

/// A component or `:::` fence open around the line being read.
enum Open {
    /// An alert: its lines are quoted.
    Alert,
    /// `<Accordion>`-style: closed by `</details>`.
    Details,
    /// Any other component: its tags are left out.
    Plain,
}

struct Frame {
    name: String,
    open: Open,
    /// The indent stripped from its lines (set by the first one).
    indent: Option<usize>,
}

/// MDX as the Markdown the parser reads, and for each line of that the
/// 0-based line of `text` it came from.
fn mdx_to_markdown(text: &str, math: bool) -> (String, Vec<usize>) {
    let mut out = Out::default();
    let mut frames: Vec<Frame> = Vec::new();
    let mut fence: Option<String> = None;
    let mut statement = false;
    let mut comment = false;
    // a component tag whose attributes run over several lines
    let mut pending: Option<(String, usize)> = None;
    for (ix, raw) in text.lines().enumerate() {
        let quote = frames
            .iter()
            .filter(|f| matches!(f.open, Open::Alert))
            .count();
        let line = dedent(raw, &mut frames);
        let trimmed = line.trim();
        if let Some(marker) = &fence {
            if trimmed.starts_with(marker.as_str()) {
                fence = None;
            }
            out.push(quote, line, ix);
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = Some(
                trimmed
                    .chars()
                    .take_while(|c| *c == '`' || *c == '~')
                    .collect(),
            );
            out.push(quote, line, ix);
            continue;
        }
        if let Some((mut tag, from)) = pending.take() {
            tag.push(' ');
            tag.push_str(trimmed);
            if tag_ends(&tag) {
                component(&tag, from, quote, &mut frames, &mut out);
            } else {
                pending = Some((tag, from));
            }
            continue;
        }
        if statement {
            statement =
                !(trimmed.ends_with('}') || trimmed.ends_with("};") || trimmed.ends_with(';'));
            continue;
        }
        if frames.is_empty() && (raw.starts_with("import ") || raw.starts_with("export ")) {
            // a multi-line statement runs until a line ending its braces
            statement = raw.contains('{') && !raw.contains('}');
            continue;
        }
        let line = strip_comments(line, &mut comment);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !comment || frames.iter().any(|f| matches!(f.open, Open::Alert)) {
                out.push(quote, "", ix);
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix(":::") {
            let rest = rest.trim_start_matches(':').trim();
            if rest.is_empty() {
                if let Some(at) = frames.iter().rposition(|f| f.name == ":::") {
                    frames.truncate(at);
                    out.push(frames_quote(&frames), "", ix);
                    continue;
                }
            } else {
                let (kind, title) = rest
                    .split_once(['[', ' ', '{'])
                    .map_or((rest, ""), |(k, t)| {
                        (k, t.trim_end_matches([']', '}']).trim())
                    });
                if let Some(kind) = alert_kind(kind) {
                    open_alert(kind, title, ":::", quote, ix, &mut frames, &mut out);
                    continue;
                }
            }
        }
        if is_component_tag(trimmed) {
            if tag_ends(trimmed) {
                component(trimmed, ix, quote, &mut frames, &mut out);
            } else {
                pending = Some((trimmed.to_string(), ix));
            }
            continue;
        }
        let quote = frames_quote(&frames);
        out.push(quote, &expressions(&line, math), ix);
    }
    (out.text, out.map)
}

#[derive(Default)]
struct Out {
    text: String,
    map: Vec<usize>,
}

impl Out {
    fn push(&mut self, quote: usize, line: &str, from: usize) {
        for _ in 0..quote {
            self.text.push('>');
        }
        if quote > 0 && !line.is_empty() {
            self.text.push(' ');
        }
        self.text.push_str(line);
        self.text.push('\n');
        self.map.push(from);
    }
}

fn frames_quote(frames: &[Frame]) -> usize {
    frames
        .iter()
        .filter(|f| matches!(f.open, Open::Alert))
        .count()
}

/// `line` without the indent of the component it sits in.
fn dedent<'a>(line: &'a str, frames: &mut [Frame]) -> &'a str {
    let Some(frame) = frames.last_mut() else {
        return line;
    };
    let lead = line.len() - line.trim_start_matches([' ', '\t']).len();
    if line.trim().is_empty() {
        return line.trim_start();
    }
    let indent = *frame.indent.get_or_insert(lead);
    &line[lead.min(indent)..]
}

/// `line` without `{/* … */}` comments (`comment`: inside one that spans
/// lines).
fn strip_comments(line: &str, comment: &mut bool) -> String {
    let mut out = String::new();
    let mut rest = line;
    loop {
        if *comment {
            match rest.find("*/") {
                Some(end) => {
                    rest = &rest[end + 2..];
                    rest = rest.trim_start().strip_prefix('}').unwrap_or(rest);
                    *comment = false;
                }
                None => return out,
            }
        }
        match rest.find("{/*") {
            Some(start) => {
                out.push_str(&rest[..start]);
                rest = &rest[start + 3..];
                *comment = true;
            }
            None => {
                out.push_str(rest);
                return out;
            }
        }
    }
}

/// A line that is one JSX component tag (`<Name …>`, `</Name>`,
/// `<Name … />`), maybe with its text and end tag on the same line.
fn is_component_tag(trimmed: &str) -> bool {
    let name = trimmed
        .strip_prefix("</")
        .or_else(|| trimmed.strip_prefix('<'));
    name.and_then(|n| n.chars().next())
        .is_some_and(|c| c.is_ascii_uppercase())
}

/// Whether a tag's text has reached its `>` (outside `{…}` and quotes).
fn tag_ends(tag: &str) -> bool {
    tag_close(tag).is_some()
}

/// The byte index of the `>` ending the first tag in `tag`.
fn tag_close(tag: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    for (i, c) in tag.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'' | '`') => quote = Some(c),
            (None, '{') => depth += 1,
            (None, '}') => depth = depth.saturating_sub(1),
            (None, '>') if depth == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

/// The value of attribute `name` in a tag (a string, or `{"…"}`).
fn attribute(tag: &str, name: &str) -> Option<String> {
    let mut rest = tag;
    while let Some(at) = rest.find(name) {
        let before = rest[..at].chars().last();
        let after = rest[at + name.len()..].trim_start();
        rest = &rest[at + name.len()..];
        if !before.is_some_and(char::is_whitespace) {
            continue;
        }
        let Some(value) = after.strip_prefix('=') else {
            continue;
        };
        let value = value.trim_start();
        let value = value
            .strip_prefix('{')
            .map(str::trim_start)
            .unwrap_or(value);
        let q = value.chars().next()?;
        if matches!(q, '"' | '\'' | '`') {
            let inner = &value[1..];
            return inner.find(q).map(|end| inner[..end].to_string());
        }
    }
    None
}

fn alert_kind(kind: &str) -> Option<AlertKind> {
    Some(match kind.to_ascii_lowercase().as_str() {
        "note" | "info" | "information" | "default" | "neutral" | "secondary" | "abstract"
        | "summary" | "quote" => AlertKind::Note,
        "tip" | "success" | "check" | "hint" | "done" => AlertKind::Tip,
        "important" | "primary" | "question" | "example" => AlertKind::Important,
        "warning" | "warn" | "attention" => AlertKind::Warning,
        "caution" | "danger" | "error" | "destructive" | "failure" | "bug" => AlertKind::Caution,
        _ => return None,
    })
}

fn alert_marker(kind: AlertKind) -> &'static str {
    match kind {
        AlertKind::Note => "[!NOTE]",
        AlertKind::Tip => "[!TIP]",
        AlertKind::Important => "[!IMPORTANT]",
        AlertKind::Warning => "[!WARNING]",
        AlertKind::Caution => "[!CAUTION]",
    }
}

fn open_alert(
    kind: AlertKind,
    title: &str,
    name: &str,
    quote: usize,
    ix: usize,
    frames: &mut Vec<Frame>,
    out: &mut Out,
) {
    if quote == 0 {
        // an alert's quote must not run on from a paragraph above
        out.push(0, "", ix);
    }
    out.push(quote + 1, alert_marker(kind), ix);
    if !title.is_empty() {
        out.push(quote + 1, &format!("**{title}**"), ix);
    }
    frames.push(Frame {
        name: name.to_string(),
        open: Open::Alert,
        indent: None,
    });
}

/// Components read as `<details>`.
const DETAILS: [&str; 5] = [
    "Accordion",
    "Expandable",
    "Collapsible",
    "Details",
    "Disclosure",
];

/// Read one component tag line (`tag`, from line `ix`).
fn component(tag: &str, ix: usize, quote: usize, frames: &mut Vec<Frame>, out: &mut Out) {
    let Some(end) = tag_close(tag) else {
        return;
    };
    let head = &tag[..=end];
    let rest = tag[end + 1..].trim();
    if let Some(name) = head.strip_prefix("</") {
        let name = name.trim_end_matches('>').trim();
        if let Some(at) = frames.iter().rposition(|f| f.name == name) {
            let closed = frames.split_off(at);
            if closed
                .first()
                .is_some_and(|f| matches!(f.open, Open::Details))
            {
                out.push(frames_quote(frames), "", ix);
                out.push(frames_quote(frames), "</details>", ix);
            }
        }
        out.push(frames_quote(frames), "", ix);
        return;
    }
    let name: String = head[1..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '_')
        .collect();
    if head.ends_with("/>") {
        out.push(quote, "", ix);
        out.push(quote, &format!("`{}`", collapse(head)), ix);
        out.push(quote, "", ix);
        return;
    }
    let title = attribute(head, "title")
        .or_else(|| attribute(head, "label"))
        .or_else(|| attribute(head, "heading"))
        .unwrap_or_default();
    let alert = match name.as_str() {
        "Callout" | "Admonition" | "Aside" | "Alert" | "Banner" => Some(
            attribute(head, "type")
                .or_else(|| attribute(head, "variant"))
                .or_else(|| attribute(head, "kind"))
                .or_else(|| attribute(head, "intent"))
                .and_then(|k| alert_kind(&k))
                .unwrap_or(AlertKind::Note),
        ),
        other => alert_kind(other),
    };
    // text and the end tag on the same line
    let inline = rest
        .strip_suffix(&format!("</{name}>"))
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let one_line = inline.is_some() || rest.ends_with(&format!("</{name}>"));
    if let Some(kind) = alert {
        open_alert(kind, &title, &name, quote, ix, frames, out);
        if let Some(text) = inline {
            out.push(quote + 1, text, ix);
        }
        if one_line {
            frames.pop();
            out.push(quote, "", ix);
        }
        return;
    }
    if DETAILS.contains(&name.as_str()) {
        out.push(quote, "", ix);
        out.push(quote, "<details>", ix);
        out.push(quote, &format!("<summary>{}</summary>", title.trim()), ix);
        out.push(quote, "", ix);
        if let Some(text) = inline {
            out.push(quote, text, ix);
        }
        if one_line {
            out.push(quote, "", ix);
            out.push(quote, "</details>", ix);
            out.push(quote, "", ix);
        } else {
            frames.push(Frame {
                name,
                open: Open::Details,
                indent: None,
            });
        }
        return;
    }
    out.push(quote, "", ix);
    if !title.is_empty() {
        out.push(quote, &format!("**{title}**"), ix);
        out.push(quote, "", ix);
    }
    if let Some(text) = inline {
        out.push(quote, text, ix);
        out.push(quote, "", ix);
    }
    if !one_line {
        frames.push(Frame {
            name,
            open: Open::Plain,
            indent: None,
        });
    }
}

/// White space runs in `s` as one space.
fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `{expressions}` in a text line as code spans (outside code spans, math
/// and HTML / JSX tags).
fn expressions(line: &str, math: bool) -> String {
    if !line.contains('{') {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len() + 4);
    let mut chars = line.char_indices().peekable();
    let mut code = 0usize;
    let mut in_math = false;
    let mut in_tag = false;
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => {
                out.push(c);
                if let Some((_, next)) = chars.next() {
                    out.push(next);
                }
                continue;
            }
            '`' if !in_math => {
                let run = line[i..].chars().take_while(|c| *c == '`').count();
                for _ in 1..run {
                    chars.next();
                }
                code = if code == run {
                    0
                } else if code == 0 {
                    run
                } else {
                    code
                };
                out.push_str(&line[i..i + run]);
                continue;
            }
            '$' if math && code == 0 => in_math = !in_math,
            '<' if code == 0 && !in_math => {
                in_tag = line[i + 1..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == '/');
            }
            '>' if in_tag => in_tag = false,
            '{' if code == 0 && !in_math && !in_tag => {
                let mut depth = 0usize;
                let end = line[i..].char_indices().find_map(|(j, c)| {
                    match c {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                return Some(i + j);
                            }
                        }
                        _ => {}
                    }
                    None
                });
                if let Some(end) = end {
                    let expr = &line[i..=end];
                    let ticks = if expr.contains('`') { "`` " } else { "`" };
                    let close = if expr.contains('`') { " ``" } else { "`" };
                    out.push_str(ticks);
                    out.push_str(expr);
                    out.push_str(close);
                    while chars.peek().is_some_and(|(j, _)| *j <= end) {
                        chars.next();
                    }
                    continue;
                }
            }
            _ => {}
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_matter_becomes_code() {
        let blocks = parse_document("---\ntitle: Hi\n---\n# Head\n", false, false);
        assert!(matches!(
            &blocks[0],
            Block::CodeBlock { language: Some(l), code } if l == "yaml" && code == "title: Hi"
        ));
        assert!(matches!(&blocks[1], Block::Heading { level: 1, .. }));
        // a rule that is not front matter stays a rule
        assert!(matches!(
            parse_document("text\n\n---\n\nmore", false, false)[1],
            Block::Rule
        ));
    }

    #[test]
    fn list_items_are_blocks() {
        let blocks = parse_document("3. a\n4. b\n\n- x\n- y\n", false, false);
        let starts: Vec<Option<u64>> = blocks
            .iter()
            .map(|b| match b {
                Block::List { start, items } => {
                    assert_eq!(items.len(), 1);
                    *start
                }
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(starts, [Some(3), Some(4), None, None]);
    }

    #[test]
    fn mdx_statements_are_left_out() {
        let text = "import A from './a'\nexport const meta = {\n  title: 'x',\n}\n\n# Title\n\n```js\nimport kept\n```\n";
        let blocks = parse_document(text, true, false);
        assert!(matches!(&blocks[0], Block::Heading { .. }), "{blocks:?}");
        assert!(
            matches!(&blocks[1], Block::CodeBlock { code, .. } if code.contains("import kept"))
        );
        assert!(is_mdx("a/b.MDX"));
    }

    fn alert(block: &Block) -> Option<AlertKind> {
        match block {
            Block::Alert { kind, .. } => Some(*kind),
            _ => None,
        }
    }

    #[test]
    fn mdx_callouts_are_alerts() {
        let text = "Intro\n<Warning>\n  Mind **this**.\n\n    - and that\n</Warning>\n\n<Callout type=\"tip\" title=\"Hey\">Short</Callout>\n\n:::danger[Careful]\nBody\n:::\n";
        let blocks = parse_document(text, true, false);
        let kinds: Vec<_> = blocks.iter().filter_map(alert).collect();
        assert_eq!(
            kinds,
            [AlertKind::Warning, AlertKind::Tip, AlertKind::Caution],
            "{blocks:?}"
        );
        // indented content inside a component is not code
        let Block::Alert { blocks: inner, .. } = &blocks[1] else {
            panic!("{blocks:?}");
        };
        assert!(
            inner.iter().all(|b| !matches!(b, Block::CodeBlock { .. })),
            "{inner:?}"
        );
    }

    #[test]
    fn mdx_components_comments_and_expressions() {
        let text = "{/* hidden */}\n<Tabs>\n<TabItem value=\"a\" label=\"First\">\nText {props.name}\n</TabItem>\n</Tabs>\n\n<Chart\n  data={[1, 2]}\n/>\n\n<Accordion title=\"More\">\nInside\n</Accordion>\n";
        let blocks = parse_document(text, true, false);
        let dump = format!("{blocks:?}");
        assert!(!dump.contains("hidden"), "{dump}");
        assert!(dump.contains("First"), "{dump}");
        assert!(dump.contains("{props.name}"), "{dump}");
        assert!(dump.contains("<Chart data={[1, 2]} />"), "{dump}");
        assert!(
            blocks.iter().any(|b| matches!(b, Block::Details { .. })),
            "{dump}"
        );
    }

    #[test]
    fn block_lines() {
        let text = "---\na: 1\n---\n# One\n\npara\ngraph\n\n- x\n- y\n";
        let (blocks, lines) = parse_document_lines(text, false, false);
        assert_eq!(blocks.len(), 5);
        assert_eq!(lines, [3, 4, 7, 10, 10]);
        let (_, mdx) = parse_document_lines(
            "import A from 'a'\n\n# One\n\n<Note>\nHi\n</Note>\n",
            true,
            false,
        );
        assert_eq!(mdx, [3, 6]);
    }
}
