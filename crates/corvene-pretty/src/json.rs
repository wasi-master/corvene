//! JSON printer: key order, number text and string escapes are copied byte
//! for byte; only whitespace changes. Several top-level values (JSON Lines)
//! print one after another. Returns `None` for invalid JSON so the caller
//! can fall back to the bracket printer.

use crate::out::Out;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tok {
    Open(u8),
    Close(u8),
    Colon,
    Comma,
    Str,
    Scalar,
}

struct Lexer<'a> {
    text: &'a str,
    pos: usize,
}

impl<'a> Lexer<'a> {
    /// The next token and its text; `Err` on a lexical error.
    fn next(&mut self) -> Result<Option<(Tok, &'a str)>, ()> {
        let bytes = self.text.as_bytes();
        while self.pos < bytes.len() && matches!(bytes[self.pos], b' ' | b'\t' | b'\n' | b'\r') {
            self.pos += 1;
        }
        let start = self.pos;
        let Some(&b) = bytes.get(start) else {
            return Ok(None);
        };
        let tok = match b {
            b'{' | b'[' => Tok::Open(b),
            b'}' | b']' => Tok::Close(b),
            b':' => Tok::Colon,
            b',' => Tok::Comma,
            b'"' => {
                let mut i = start + 1;
                loop {
                    match bytes.get(i) {
                        None => return Err(()),
                        Some(b'"') => break,
                        Some(b'\\') => i += 2,
                        Some(_) => i += 1,
                    }
                }
                self.pos = i + 1;
                return Ok(Some((Tok::Str, &self.text[start..self.pos])));
            }
            _ => {
                let mut i = start;
                while i < bytes.len()
                    && !matches!(
                        bytes[i],
                        b'{' | b'}'
                            | b'['
                            | b']'
                            | b':'
                            | b','
                            | b'"'
                            | b' '
                            | b'\t'
                            | b'\n'
                            | b'\r'
                    )
                {
                    i += 1;
                }
                let word = &self.text[start..i];
                if !(matches!(word, "true" | "false" | "null") || is_number(word)) {
                    return Err(());
                }
                self.pos = i;
                return Ok(Some((Tok::Scalar, word)));
            }
        };
        self.pos += 1;
        Ok(Some((tok, &self.text[start..start + 1])))
    }

    fn peek(&mut self) -> Result<Option<Tok>, ()> {
        let saved = self.pos;
        let tok = self.next()?.map(|(t, _)| t);
        self.pos = saved;
        Ok(tok)
    }
}

fn is_number(word: &str) -> bool {
    let b = word.as_bytes();
    let mut i = 0;
    if b.get(i) == Some(&b'-') {
        i += 1;
    }
    let digits = |i: &mut usize| {
        let s = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        *i > s
    };
    if !digits(&mut i) {
        return false;
    }
    if b.get(i) == Some(&b'.') {
        i += 1;
        if !digits(&mut i) {
            return false;
        }
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        if !digits(&mut i) {
            return false;
        }
    }
    i == b.len()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Want {
    /// A value (top level, after `:`, or after `,` in an array).
    Value,
    /// A key (after `,` in an object).
    Key,
    Colon,
    /// `,` or the container's close (or another top-level value).
    After,
}

pub(crate) fn pretty(text: &str) -> Option<String> {
    let mut lex = Lexer { text, pos: 0 };
    let mut out = Out::with_capacity(text.len() * 2);
    let mut stack: Vec<u8> = Vec::new();
    let mut want = Want::Value;
    let mut any = false;
    while let Some((tok, s)) = lex.next().ok()? {
        match (want, tok) {
            (Want::Value, Tok::Str | Tok::Scalar) => {
                if stack.is_empty() && any {
                    out.newline();
                }
                out.text(s);
                any = true;
                want = Want::After;
            }
            (Want::Value, Tok::Open(b)) => {
                if stack.is_empty() && any {
                    out.newline();
                }
                any = true;
                let close = if b == b'{' { b'}' } else { b']' };
                if lex.peek().ok()? == Some(Tok::Close(close)) {
                    lex.next().ok()?;
                    out.text(if b == b'{' { "{}" } else { "[]" });
                    want = Want::After;
                } else {
                    out.text(s);
                    stack.push(b);
                    out.indent = stack.len();
                    out.newline();
                    want = if b == b'{' { Want::Key } else { Want::Value };
                }
            }
            (Want::Key, Tok::Str) => {
                out.text(s);
                want = Want::Colon;
            }
            (Want::Colon, Tok::Colon) => {
                out.text(":");
                out.space();
                want = Want::Value;
            }
            (Want::After, Tok::Comma) if !stack.is_empty() => {
                out.text(",");
                out.newline();
                want = if stack.last() == Some(&b'{') {
                    Want::Key
                } else {
                    Want::Value
                };
            }
            (Want::After, Tok::Close(b)) => {
                let open = if b == b'}' { b'{' } else { b'[' };
                if stack.pop() != Some(open) {
                    return None;
                }
                out.indent = stack.len();
                out.newline();
                out.text(s);
            }
            (Want::After, Tok::Str | Tok::Scalar | Tok::Open(_)) if stack.is_empty() => {
                // Another top-level value (JSON Lines): reprocess as a value.
                lex.pos -= s.len();
                want = Want::Value;
            }
            _ => return None,
        }
    }
    if !stack.is_empty() || (any && want != Want::After) {
        return None;
    }
    Some(out.finish())
}
