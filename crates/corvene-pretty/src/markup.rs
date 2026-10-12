//! HTML / XML printer: one tag per line, indented by nesting; text trimmed
//! onto its own line with whitespace runs collapsed; comments, CDATA,
//! doctypes and processing instructions on their own lines, verbatim.
//! An element holding only short text stays on one line (`<b>x</b>`).
//! Tags keep their attributes as written (whitespace runs collapsed).
//!
//! HTML: void elements never indent, a few implied end tags (`li`, `p`,
//! `td`, ...) are honoured, `pre`/`textarea` content is kept verbatim, and
//! `script`/`style` content is printed by the JavaScript/JSON/CSS printers
//! when it sits on one line (verbatim otherwise). Malformed markup never
//! loses text: unknown end tags print in place and unclosed tags just
//! leave the indent deeper.

use crate::out::Out;

#[derive(Clone, Copy, Debug)]
enum Tok<'a> {
    Text(&'a str),
    /// Comment, CDATA, doctype or processing instruction.
    Verbatim(&'a str),
    Start {
        raw: &'a str,
        name: &'a str,
        closed: bool,
    },
    End {
        raw: &'a str,
        name: &'a str,
    },
    /// Content of a raw text element (`script`, `style`, `pre`, ...).
    Raw(&'a str),
}

const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr", "keygen", "command", "basefont", "frame", "isindex",
];
const RAW: &[&str] = &[
    "script",
    "style",
    "textarea",
    "pre",
    "xmp",
    "listing",
    "plaintext",
    "title",
];

/// How far an end tag searches the open element stack for its start tag.
const END_SEARCH: usize = 64;

fn name_char(c: u8) -> bool {
    !(c.is_ascii_whitespace() || c == b'/' || c == b'>')
}

fn tokenize(text: &str, html: bool) -> Vec<Tok<'_>> {
    let bytes = text.as_bytes();
    let mut toks = Vec::new();
    let mut i = 0;
    let mut text_start = 0;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        let rest = &text[i..];
        let end = if rest.starts_with("<!--") {
            Some((
                find_from(text, i + 4, "-->").map_or(bytes.len(), |p| p + 3),
                None,
            ))
        } else if rest.starts_with("<![CDATA[") {
            Some((
                find_from(text, i + 9, "]]>").map_or(bytes.len(), |p| p + 3),
                None,
            ))
        } else if rest.starts_with("<?") {
            Some((
                find_from(text, i + 2, "?>").map_or(bytes.len(), |p| p + 2),
                None,
            ))
        } else if rest.starts_with("<!") {
            Some((scan_decl(bytes, i + 2), None))
        } else if rest.starts_with("</")
            && bytes
                .get(i + 2)
                .is_some_and(|c| c.is_ascii_alphabetic() || *c >= 0x80 || *c == b'_' || *c == b':')
        {
            Some((scan_tag(bytes, i + 2), Some(true)))
        } else if bytes.get(i + 1).is_some_and(|c| {
            c.is_ascii_alphabetic() || *c >= 0x80 || (!html && (*c == b'_' || *c == b':'))
        }) {
            Some((scan_tag(bytes, i + 1), Some(false)))
        } else {
            None
        };
        let Some((end, tag)) = end else {
            i += 1;
            continue;
        };
        if tag.is_some() && end > bytes.len() {
            // Unterminated tag: the rest is text.
            break;
        }
        let end = end.min(bytes.len());
        if text_start < i {
            toks.push(Tok::Text(&text[text_start..i]));
        }
        let raw = &text[i..end];
        match tag {
            None => toks.push(Tok::Verbatim(raw)),
            Some(true) => {
                let n = 2 + raw[2..].bytes().take_while(|c| name_char(*c)).count();
                toks.push(Tok::End {
                    raw,
                    name: &raw[2..n],
                });
            }
            Some(false) => {
                let n = 1 + raw[1..].bytes().take_while(|c| name_char(*c)).count();
                let name = &raw[1..n];
                let closed = raw.ends_with("/>");
                toks.push(Tok::Start { raw, name, closed });
                if html && !closed && RAW.iter().any(|r| r.eq_ignore_ascii_case(name)) {
                    // Raw text runs to the matching end tag.
                    let close = find_end_tag(text, end, name).unwrap_or(bytes.len());
                    if close > end {
                        toks.push(Tok::Raw(&text[end..close]));
                    }
                    i = close;
                    text_start = close;
                    continue;
                }
            }
        }
        i = end;
        text_start = end;
    }
    if text_start < bytes.len() {
        toks.push(Tok::Text(&text[text_start..]));
    }
    toks
}

fn find_from(text: &str, from: usize, needle: &str) -> Option<usize> {
    text.get(from..)?.find(needle).map(|p| from + p)
}

/// Case-insensitive search for `</name` from `from`.
fn find_end_tag(text: &str, from: usize, name: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = from;
    while let Some(p) = memchr_lt(bytes, i) {
        let after = p + 2 + name.len();
        if bytes.get(p + 1) == Some(&b'/')
            && bytes.len() >= after
            && bytes[p + 2..after].eq_ignore_ascii_case(name.as_bytes())
            && bytes
                .get(after)
                .is_none_or(|c| !name_char(*c) || *c == b'/')
        {
            return Some(p);
        }
        i = p + 1;
    }
    None
}

fn memchr_lt(bytes: &[u8], from: usize) -> Option<usize> {
    bytes
        .get(from..)?
        .iter()
        .position(|&c| c == b'<')
        .map(|p| from + p)
}

/// End (past `>`) of a tag whose name starts at `i`; quoted attribute
/// values may hold `>`. Returns `usize::MAX` when unterminated.
fn scan_tag(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() {
        match bytes[i] {
            b'>' => return i + 1,
            b'=' => {
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                if let Some(&q) = bytes.get(i).filter(|c| matches!(c, b'"' | b'\'')) {
                    i += 1;
                    while i < bytes.len() && bytes[i] != q {
                        i += 1;
                    }
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    usize::MAX
}

/// End of `<!...>`, skipping quotes and a `[...]` internal subset.
fn scan_decl(bytes: &[u8], mut i: usize) -> usize {
    let mut brackets = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'"' | b'\'' => {
                let q = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != q {
                    i += 1;
                }
            }
            b'[' => brackets += 1,
            b']' => brackets = brackets.saturating_sub(1),
            b'>' if brackets == 0 => return i + 1,
            _ => {}
        }
        i += 1;
    }
    bytes.len()
}

/// `s` with whitespace runs collapsed to one space and trimmed (quoted
/// attribute values in tags are copied as is when `tag`).
fn collapse(s: &str, tag: bool) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    let mut quote: Option<char> = None;
    let mut after_eq = false;
    for c in s.chars() {
        if let Some(q) = quote {
            out.push(c);
            if c == q {
                quote = None;
            }
            continue;
        }
        if c.is_whitespace() {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
        }
        space = false;
        if tag && after_eq && (c == '"' || c == '\'') {
            quote = Some(c);
        }
        after_eq = c == '=' || (after_eq && c.is_whitespace());
        out.push(c);
    }
    out
}

fn same(a: &str, b: &str, html: bool) -> bool {
    if html {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// Whether opening `new` implicitly closes an open `top` (HTML).
fn implied_close(new: &str, top: &str) -> bool {
    let n = new.to_ascii_lowercase();
    let t = top.to_ascii_lowercase();
    match n.as_str() {
        "li" => t == "li",
        "dt" | "dd" => t == "dt" || t == "dd",
        "tr" => matches!(t.as_str(), "tr" | "td" | "th"),
        "td" | "th" => t == "td" || t == "th",
        "option" => t == "option",
        "thead" | "tbody" | "tfoot" => {
            matches!(t.as_str(), "thead" | "tbody" | "tfoot" | "tr" | "td" | "th")
        }
        "p" | "div" | "ul" | "ol" | "table" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
        | "section" | "article" | "header" | "footer" | "nav" | "form" | "blockquote" | "pre"
        | "hr" | "dl" => t == "p",
        _ => false,
    }
}

pub(crate) fn pretty(text: &str, html: bool) -> String {
    let toks = tokenize(text, html);
    let mut out = Out::with_capacity(text.len() + text.len() / 2);
    let mut stack: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        out.indent = stack.len();
        match toks[i] {
            Tok::Text(t) => {
                let t = collapse(t, false);
                if !t.is_empty() {
                    out.newline();
                    out.text(&t);
                    out.newline();
                }
            }
            Tok::Verbatim(v) | Tok::Raw(v) => {
                out.newline();
                out.text(v);
                out.newline();
            }
            Tok::End { raw, name } => {
                let found = stack
                    .iter()
                    .rev()
                    .take(END_SEARCH)
                    .position(|open| same(open, name, html));
                if let Some(depth) = found {
                    stack.truncate(stack.len() - depth - 1);
                }
                out.indent = stack.len();
                out.newline();
                out.text(&collapse(raw, true));
                out.newline();
            }
            Tok::Start { raw, name, closed } => {
                if html {
                    while stack.last().is_some_and(|top| implied_close(name, top)) {
                        stack.pop();
                    }
                }
                out.indent = stack.len();
                out.newline();
                let tag = collapse(raw, true);
                out.text(&tag);
                let void = closed || (html && VOID.iter().any(|v| v.eq_ignore_ascii_case(name)));
                if void {
                    out.newline();
                    i += 1;
                    continue;
                }
                let raw_el = html && RAW.iter().any(|r| r.eq_ignore_ascii_case(name));
                // Content and the end tag that may follow.
                let (content, end_at) = match toks.get(i + 1) {
                    Some(Tok::Raw(c)) => (Some(*c), i + 2),
                    Some(Tok::Text(c)) if !raw_el => (Some(*c), i + 2),
                    _ => (None, i + 1),
                };
                let end = match toks.get(end_at) {
                    Some(Tok::End { raw, name: n }) if same(n, name, html) => Some(*raw),
                    _ => None,
                };
                if raw_el {
                    let c = content.unwrap_or("");
                    if let Some(printed) = embedded(name, raw, c) {
                        out.newline();
                        out.indent = stack.len() + 1;
                        out.block(&printed);
                        out.indent = stack.len();
                        if let Some(e) = end {
                            out.text(&collapse(e, true));
                        }
                    } else {
                        // Verbatim: content and end tag right after the tag.
                        out.buf.push_str(c);
                        if let Some(e) = end {
                            out.buf.push_str(&collapse(e, true));
                        }
                    }
                    out.newline();
                    i = if end.is_some() { end_at + 1 } else { end_at };
                    if end.is_none() {
                        stack.push(name);
                    }
                    continue;
                }
                if let Some(e) = end {
                    let inner = content.map(|c| collapse(c, false)).unwrap_or_default();
                    if inner.len() <= 120 {
                        out.text(&inner);
                        out.text(&collapse(e, true));
                        out.newline();
                        i = end_at + 1;
                        continue;
                    }
                }
                out.newline();
                stack.push(name);
            }
        }
        i += 1;
    }
    out.finish()
}

/// One-line `script`/`style` content printed by its language's printer.
fn embedded(name: &str, tag: &str, content: &str) -> Option<String> {
    let trimmed = content.trim();
    if trimmed.is_empty() || trimmed.contains('\n') {
        return None;
    }
    let ty = attr(tag, "type").map(|t| t.to_ascii_lowercase());
    if name.eq_ignore_ascii_case("style") {
        return Some(crate::css::pretty(trimmed, false));
    }
    if !name.eq_ignore_ascii_case("script") {
        return None;
    }
    match ty.as_deref() {
        None | Some("") | Some("module") => Some(crate::js::pretty(trimmed, false)),
        Some(t) if t.contains("json") || t == "importmap" || t == "speculationrules" => {
            Some(crate::json::pretty(trimmed).unwrap_or_else(|| crate::generic::pretty(trimmed)))
        }
        Some(t)
            if t.contains("javascript")
                || t.contains("ecmascript")
                || t.contains("babel")
                || t.contains("jsx")
                || t.contains("typescript") =>
        {
            Some(crate::js::pretty(trimmed, false))
        }
        _ => None,
    }
}

/// The value of attribute `name` in a start tag (unquoted).
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let lower = tag.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut from = 0;
    while let Some(p) = lower.get(from..)?.find(name).map(|p| p + from) {
        let before_ok = p > 0 && bytes[p - 1].is_ascii_whitespace();
        let mut j = p + name.len();
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if before_ok && bytes.get(j) == Some(&b'=') {
            j += 1;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            let (start, end) = match bytes.get(j) {
                Some(&q @ (b'"' | b'\'')) => {
                    let e = lower[j + 1..]
                        .find(q as char)
                        .map_or(lower.len(), |e| j + 1 + e);
                    (j + 1, e)
                }
                _ => {
                    let e = lower[j..]
                        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
                        .map_or(lower.len(), |e| j + e);
                    (j, e)
                }
            };
            return tag.get(start..end);
        }
        from = p + name.len();
    }
    None
}
