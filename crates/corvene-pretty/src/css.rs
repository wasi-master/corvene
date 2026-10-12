//! CSS printer (also SCSS and Less): `selector {` / one declaration per
//! line / `}`, nested at-rules and CSS nesting indented, selectors split
//! after top-level commas, `name: value` spacing in declarations. Strings,
//! `url(...)` and comments stay verbatim and no `;` is ever added: only
//! whitespace changes.

use crate::out::Out;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Space,
    Comment,
    LineComment,
    Str,
    /// A string cut off by a line break (CSS ends it there).
    OpenStr,
    Url,
    Open,
    Close,
    Semi,
    Comma,
    Colon,
    LParen,
    RParen,
    /// Any other run of characters.
    Word,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Token {
    pub(crate) kind: Kind,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c')
}

/// Splits `text` into tokens covering every byte.
pub(crate) fn tokenize(text: &str, line_comments: bool) -> Vec<Token> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::with_capacity(text.len() / 4 + 1);
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let b = bytes[i];
        let kind = if is_space(b) {
            while i < bytes.len() && is_space(bytes[i]) {
                i += 1;
            }
            Kind::Space
        } else if b == b'/' && bytes.get(i + 1) == Some(&b'*') {
            i = match text[i + 2..].find("*/") {
                Some(p) => i + 2 + p + 2,
                None => bytes.len(),
            };
            Kind::Comment
        } else if line_comments
            && b == b'/'
            && bytes.get(i + 1) == Some(&b'/')
            && (i == 0 || bytes[i - 1] != b':')
        {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            Kind::LineComment
        } else if b == b'"' || b == b'\'' {
            i += 1;
            while i < bytes.len() && bytes[i] != b && bytes[i] != b'\n' {
                i += if bytes[i] == b'\\' { 2 } else { 1 };
            }
            if i < bytes.len() && bytes[i] == b {
                i += 1;
                Kind::Str
            } else {
                i = i.min(bytes.len());
                Kind::OpenStr
            }
        } else if starts_url(bytes, i) {
            // `url(` with an unquoted body runs to the first `)`.
            i += 4;
            while i < bytes.len() && bytes[i] != b')' {
                i += if bytes[i] == b'\\' { 2 } else { 1 };
            }
            i = (i + 1).min(bytes.len());
            Kind::Url
        } else {
            i += 1;
            match b {
                b'{' => Kind::Open,
                b'}' => Kind::Close,
                b';' => Kind::Semi,
                b',' => Kind::Comma,
                b':' => Kind::Colon,
                b'(' => Kind::LParen,
                b')' => Kind::RParen,
                _ => {
                    if b == b'\\' {
                        i += 1;
                    }
                    while i < bytes.len() {
                        let c = bytes[i];
                        if is_space(c) || b"{};,:()\"'".contains(&c) {
                            break;
                        }
                        if c == b'/'
                            && (bytes.get(i + 1) == Some(&b'*')
                                || (line_comments
                                    && bytes.get(i + 1) == Some(&b'/')
                                    && bytes[i - 1] != b':'))
                        {
                            break;
                        }
                        if starts_url(bytes, i) {
                            break;
                        }
                        i += if c == b'\\' { 2 } else { 1 };
                    }
                    i = i.min(bytes.len());
                    Kind::Word
                }
            }
        };
        // Escapes may step into a multi-byte char: move to its end.
        while i < bytes.len() && !text.is_char_boundary(i) {
            i += 1;
        }
        tokens.push(Token {
            kind,
            start,
            end: i,
        });
    }
    tokens
}

fn starts_url(bytes: &[u8], i: usize) -> bool {
    bytes.len() >= i + 4
        && bytes[i..i + 4].eq_ignore_ascii_case(b"url(")
        && (i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'-'))
        && {
            let mut j = i + 4;
            while j < bytes.len() && is_space(bytes[j]) {
                j += 1;
            }
            !matches!(bytes.get(j), Some(b'"' | b'\''))
        }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stmt {
    /// A selector list or at-rule prelude that opens a block.
    Rule { at: bool },
    /// A declaration or block-less at-rule (`@import x;`).
    Decl { colon_done: bool, plain: bool },
}

pub(crate) fn pretty(text: &str, line_comments: bool) -> String {
    let tokens = tokenize(text, line_comments);
    let mut out = Out::with_capacity(text.len() + text.len() / 2);
    let mut depth = 0usize;
    let mut parens = 0usize;
    let mut stmt: Option<Stmt> = None;
    // Whether whitespace preceded the current token in the source.
    let mut had_space = false;
    for (idx, t) in tokens.iter().enumerate() {
        let s = &text[t.start..t.end];
        match t.kind {
            Kind::Space => {
                had_space = true;
                continue;
            }
            Kind::OpenStr => {
                if had_space {
                    out.space();
                }
                out.text(s);
                out.newline();
                had_space = false;
                continue;
            }
            Kind::Comment | Kind::LineComment => {
                let own_line = stmt.is_none() || s.starts_with("/*!") || s.contains('\n');
                if own_line {
                    out.newline();
                } else if had_space {
                    out.space();
                }
                out.text(s);
                if own_line || t.kind == Kind::LineComment {
                    out.newline();
                } else {
                    // Keep the source's spacing after an inline comment.
                    had_space = false;
                    if tokens.get(idx + 1).is_some_and(|n| n.kind == Kind::Space) {
                        out.space();
                    }
                }
                continue;
            }
            _ => {}
        }
        if stmt.is_none() && !matches!(t.kind, Kind::Close | Kind::Semi) {
            stmt = Some(classify(text, &tokens[idx..], depth));
        }
        match t.kind {
            Kind::Open => {
                parens = 0;
                out.space();
                out.text("{");
                depth += 1;
                out.indent = depth;
                stmt = None;
                if next_significant(&tokens[idx + 1..]) == Some(Kind::Close) {
                    // `{}` stays on one line.
                } else {
                    out.newline();
                }
            }
            Kind::Close => {
                parens = 0;
                depth = depth.saturating_sub(1);
                out.indent = depth;
                if !(idx > 0 && prev_significant(&tokens[..idx]) == Some(Kind::Open)) {
                    out.newline();
                }
                out.text("}");
                out.newline();
                stmt = None;
            }
            Kind::Semi if parens == 0 => {
                out.text(";");
                out.newline();
                stmt = None;
            }
            Kind::Comma if parens == 0 && stmt == Some(Stmt::Rule { at: false }) => {
                out.text(",");
                out.newline();
            }
            Kind::Colon
                if parens == 0
                    && matches!(
                        stmt,
                        Some(Stmt::Decl {
                            colon_done: false,
                            plain: true
                        })
                    ) =>
            {
                out.text(":");
                // `a:http://x` must not turn into a `//` line comment.
                let slash_next = tokens
                    .get(idx + 1)
                    .is_some_and(|n| text[n.start..].starts_with('/'));
                if !slash_next {
                    out.space();
                }
                stmt = Some(Stmt::Decl {
                    colon_done: true,
                    plain: true,
                });
            }
            _ => {
                match t.kind {
                    Kind::LParen => parens += 1,
                    Kind::RParen => parens = parens.saturating_sub(1),
                    _ => {}
                }
                if had_space && !matches!(t.kind, Kind::Semi | Kind::Comma) {
                    out.space();
                }
                out.text(s);
            }
        }
        had_space = false;
    }
    out.finish()
}

/// Whether the statement starting at `rest[0]` opens a block (a `{` comes
/// before any top-level `;` or `}`).
fn classify(text: &str, rest: &[Token], depth: usize) -> Stmt {
    let at = text[rest[0].start..].starts_with('@');
    if depth == 0 {
        return Stmt::Rule { at };
    }
    let mut parens = 0usize;
    for t in rest {
        match t.kind {
            Kind::LParen => parens += 1,
            Kind::RParen => parens = parens.saturating_sub(1),
            Kind::Open => return Stmt::Rule { at },
            Kind::Semi if parens == 0 => break,
            Kind::Close => break,
            _ => {}
        }
    }
    Stmt::Decl {
        colon_done: false,
        plain: !at && rest[0].kind == Kind::Word,
    }
}

fn next_significant(rest: &[Token]) -> Option<Kind> {
    rest.iter()
        .map(|t| t.kind)
        .find(|k| !matches!(k, Kind::Space))
}

fn prev_significant(before: &[Token]) -> Option<Kind> {
    before
        .iter()
        .rev()
        .map(|t| t.kind)
        .find(|k| !matches!(k, Kind::Space))
}
