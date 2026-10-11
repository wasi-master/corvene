//! OpenStep ("old-style ASCII") property lists, the syntax of `.glyphs`
//! files: `{ key = value; }` dicts, `( a, b )` arrays, quoted strings with
//! backslash escapes, bare words and `<hex>` data. Numbers stay strings;
//! readers convert what they need. Strings borrow from the input unless
//! they hold escapes, so a 50 MB file does not cost millions of
//! allocations.

use std::borrow::Cow;

/// Deeper nesting is refused (the parser recurses once per level).
const MAX_DEPTH: usize = 128;

/// A property list value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Value<'a> {
    Dict(Vec<(Cow<'a, str>, Value<'a>)>),
    Array(Vec<Value<'a>>),
    Str(Cow<'a, str>),
}

impl<'a> Value<'a> {
    /// A dict entry (`None` for a missing key or a non-dict).
    pub fn get(&self, key: &str) -> Option<&Value<'a>> {
        match self {
            Value::Dict(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    /// A dict entry's text.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Value::str)
    }

    /// A dict entry read as a number.
    pub fn get_num(&self, key: &str) -> Option<f32> {
        self.get(key).and_then(Value::num)
    }

    pub fn num(&self) -> Option<f32> {
        self.str().and_then(|s| s.trim().parse::<f32>().ok())
    }

    /// Array items (empty for anything else).
    pub fn items(&self) -> &[Value<'a>] {
        match self {
            Value::Array(a) => a,
            _ => &[],
        }
    }

    /// A dict entry's array items (empty when missing).
    pub fn get_items(&self, key: &str) -> &[Value<'a>] {
        self.get(key).map_or(&[], Value::items)
    }

    /// Dict entries (empty for anything else).
    pub fn entries(&self) -> &[(Cow<'a, str>, Value<'a>)] {
        match self {
            Value::Dict(e) => e,
            _ => &[],
        }
    }

    /// A short one-line rendering, cut at about `max` bytes.
    pub fn summary(&self, max: usize) -> String {
        let mut out = String::new();
        self.write_summary(&mut out, max);
        if out.len() > max {
            let mut cut = max;
            while !out.is_char_boundary(cut) {
                cut -= 1;
            }
            out.truncate(cut);
            out.push('…');
        }
        out
    }

    fn write_summary(&self, out: &mut String, max: usize) {
        if out.len() > max {
            return;
        }
        match self {
            Value::Str(s) => {
                for (i, line) in s.lines().enumerate() {
                    if i > 0 {
                        out.push_str(" ⏎ ");
                    }
                    out.push_str(line.trim());
                    if out.len() > max {
                        return;
                    }
                }
            }
            Value::Array(a) => {
                out.push('(');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    v.write_summary(out, max);
                    if out.len() > max {
                        return;
                    }
                }
                out.push(')');
            }
            Value::Dict(d) => {
                out.push('{');
                for (i, (k, v)) in d.iter().enumerate() {
                    if i > 0 {
                        out.push(' ');
                    }
                    out.push_str(k);
                    out.push_str(" = ");
                    v.write_summary(out, max);
                    out.push(';');
                    if out.len() > max {
                        return;
                    }
                }
                out.push('}');
            }
        }
    }
}

/// Parses a whole document. Errors carry the line number.
#[cfg(test)]
pub(crate) fn parse(text: &str) -> Result<Value<'_>, String> {
    run(Parser {
        s: text,
        b: text.as_bytes(),
        pos: 0,
        stream: None,
    })
}

/// Parses a whole document (errors carry the line number), except that
/// the items of the top-level array under `key` go to
/// `on_item` one at a time instead of into the tree (that key is left with
/// an empty array), so a font's glyphs need not all be held as values.
pub(crate) fn parse_streaming<'a>(
    text: &'a str,
    key: &str,
    on_item: &mut dyn FnMut(Value<'a>),
) -> Result<Value<'a>, String> {
    run(Parser {
        s: text,
        b: text.as_bytes(),
        pos: 0,
        stream: Some((key, on_item)),
    })
}

fn run<'a>(mut p: Parser<'a, '_>) -> Result<Value<'a>, String> {
    p.skip_ws();
    let v = p.value(0).map_err(|e| p.error(&e))?;
    p.skip_ws();
    if p.pos < p.b.len() {
        return Err(p.error("text after the top-level value"));
    }
    Ok(v)
}

/// The top-level key whose array items are streamed, and where to.
type Stream<'a, 'p> = (&'p str, &'p mut dyn FnMut(Value<'a>));

struct Parser<'a, 'p> {
    s: &'a str,
    b: &'a [u8],
    pos: usize,
    stream: Option<Stream<'a, 'p>>,
}

fn is_word_byte(c: u8) -> bool {
    !(c.is_ascii_whitespace()
        || matches!(
            c,
            b'{' | b'}' | b'(' | b')' | b'=' | b';' | b',' | b'"' | b'\'' | b'<' | b'>'
        ))
}

impl<'a> Parser<'a, '_> {
    fn error(&self, msg: &str) -> String {
        let at = self.pos.min(self.b.len());
        let line = self.b[..at].iter().filter(|&&c| c == b'\n').count() + 1;
        format!("{msg} on line {line}")
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() {
                self.pos += 1;
            } else if c == b'/' && self.b.get(self.pos + 1) == Some(&b'/') {
                while self.peek().is_some_and(|c| c != b'\n') {
                    self.pos += 1;
                }
            } else if c == b'/' && self.b.get(self.pos + 1) == Some(&b'*') {
                match self.s[self.pos + 2..].find("*/") {
                    Some(end) => self.pos += 2 + end + 2,
                    None => self.pos = self.b.len(),
                }
            } else {
                break;
            }
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value<'a>, String> {
        if depth > MAX_DEPTH {
            return Err("values nested too deeply".into());
        }
        match self.peek() {
            Some(b'{') => self.dict(depth),
            Some(b'(') => {
                let mut items = Vec::new();
                self.array(depth, &mut |v| items.push(v))?;
                Ok(Value::Array(items))
            }
            Some(b'"' | b'\'') => self.quoted().map(Value::Str),
            Some(b'<') => self.data().map(Value::Str),
            Some(c) if is_word_byte(c) => Ok(Value::Str(self.word())),
            Some(c) => Err(format!("unexpected '{}'", c as char)),
            None => Err("unexpected end of file".into()),
        }
    }

    fn dict(&mut self, depth: usize) -> Result<Value<'a>, String> {
        self.pos += 1; // {
        let mut entries = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Value::Dict(entries));
                }
                None => return Err("unclosed dict".into()),
                _ => {}
            }
            let key = match self.peek() {
                Some(b'"' | b'\'') => self.quoted()?,
                Some(c) if is_word_byte(c) => self.word(),
                Some(c) => return Err(format!("unexpected '{}' for a dict key", c as char)),
                None => return Err("unclosed dict".into()),
            };
            self.skip_ws();
            if self.peek() != Some(b'=') {
                return Err(format!("missing '=' after key {key:?}"));
            }
            self.pos += 1;
            self.skip_ws();
            let streamed = depth == 0
                && self.peek() == Some(b'(')
                && self.stream.as_ref().is_some_and(|(k, _)| *k == key);
            let value = match self.stream.take() {
                Some((k, on_item)) if streamed => {
                    let done = self.array(depth + 1, &mut *on_item);
                    self.stream = Some((k, on_item));
                    done?;
                    Value::Array(Vec::new())
                }
                stream => {
                    self.stream = stream;
                    self.value(depth + 1)?
                }
            };
            entries.push((key, value));
            self.skip_ws();
            match self.peek() {
                Some(b';') => self.pos += 1,
                Some(b'}') => {}
                _ => return Err("missing ';' after a dict value".into()),
            }
        }
    }

    /// An array's items, handed to `item` in order.
    fn array(&mut self, depth: usize, item: &mut dyn FnMut(Value<'a>)) -> Result<(), String> {
        self.pos += 1; // (
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b')') => {
                    self.pos += 1;
                    return Ok(());
                }
                None => return Err("unclosed array".into()),
                _ => {}
            }
            item(self.value(depth + 1)?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b')') => {}
                _ => return Err("missing ',' between array items".into()),
            }
        }
    }

    fn word(&mut self) -> Cow<'a, str> {
        let start = self.pos;
        while self.peek().is_some_and(is_word_byte) {
            self.pos += 1;
        }
        Cow::Borrowed(&self.s[start..self.pos])
    }

    fn data(&mut self) -> Result<Cow<'a, str>, String> {
        self.pos += 1; // <
        let start = self.pos;
        match self.s[start..].find('>') {
            Some(end) => {
                self.pos = start + end + 1;
                Ok(Cow::Borrowed(&self.s[start..start + end]))
            }
            None => Err("unclosed <data>".into()),
        }
    }

    fn quoted(&mut self) -> Result<Cow<'a, str>, String> {
        let quote = self.b[self.pos];
        self.pos += 1;
        let start = self.pos;
        // fast path: no escapes, borrow the text
        while let Some(c) = self.peek() {
            if c == quote {
                self.pos += 1;
                return Ok(Cow::Borrowed(&self.s[start..self.pos - 1]));
            }
            if c == b'\\' {
                break;
            }
            self.pos += 1;
        }
        let mut out = String::new();
        let mut run = start;
        // a high surrogate from a `\U` escape waiting for its low half
        let mut high: Option<u32> = None;
        loop {
            let Some(c) = self.peek() else {
                return Err("unclosed string".into());
            };
            if c == quote {
                out.push_str(&self.s[run..self.pos]);
                if high.take().is_some() {
                    out.push('\u{FFFD}');
                }
                self.pos += 1;
                return Ok(Cow::Owned(out));
            }
            if c != b'\\' {
                self.pos += 1;
                continue;
            }
            out.push_str(&self.s[run..self.pos]);
            self.pos += 1;
            let Some(e) = self.peek() else {
                return Err("unclosed string".into());
            };
            self.pos += 1;
            let code: Option<u32> = match e {
                b'n' => Some(10),
                b't' => Some(9),
                b'r' => Some(13),
                b'a' => Some(7),
                b'b' => Some(8),
                b'f' => Some(12),
                b'v' => Some(11),
                b'0'..=b'7' => {
                    let mut v = u32::from(e - b'0');
                    for _ in 0..2 {
                        match self.peek() {
                            Some(d @ b'0'..=b'7') => {
                                v = v * 8 + u32::from(d - b'0');
                                self.pos += 1;
                            }
                            _ => break,
                        }
                    }
                    Some(v)
                }
                b'U' | b'u' => {
                    let mut v = 0u32;
                    for _ in 0..4 {
                        match self.peek().and_then(|d| (d as char).to_digit(16)) {
                            Some(d) => {
                                v = v * 16 + d;
                                self.pos += 1;
                            }
                            None => break,
                        }
                    }
                    Some(v)
                }
                _ => None,
            };
            match code {
                Some(v @ 0xD800..=0xDBFF) => {
                    if high.replace(v).is_some() {
                        out.push('\u{FFFD}');
                    }
                }
                Some(v @ 0xDC00..=0xDFFF) => match high.take() {
                    Some(h) => {
                        let c = 0x10000 + ((h - 0xD800) << 10) + (v - 0xDC00);
                        out.push(char::from_u32(c).unwrap_or('\u{FFFD}'));
                    }
                    None => out.push('\u{FFFD}'),
                },
                Some(v) => {
                    if high.take().is_some() {
                        out.push('\u{FFFD}');
                    }
                    out.push(char::from_u32(v).unwrap_or('\u{FFFD}'));
                }
                None => {
                    if high.take().is_some() {
                        out.push('\u{FFFD}');
                    }
                    // `\"`, `\\` and unknown escapes keep the character;
                    // step back over a multi-byte one to copy it whole
                    self.pos -= 1;
                    let ch = self.s[self.pos..].chars().next().unwrap_or('\u{FFFD}');
                    out.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
            run = self.pos;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> Value<'_> {
        Value::Str(v.into())
    }

    #[test]
    fn parses_dicts_arrays_and_words() {
        let v = parse(
            "{\n.formatVersion = 3;\nname = A.sc;\nlist = (1, -2.5, (x,y,l),);\nempty = {};\n}",
        )
        .unwrap();
        assert_eq!(v.get_num(".formatVersion"), Some(3.0));
        assert_eq!(v.get_str("name"), Some("A.sc"));
        assert_eq!(
            v.get("list"),
            Some(&Value::Array(vec![
                s("1"),
                s("-2.5"),
                Value::Array(vec![s("x"), s("y"), s("l")]),
            ]))
        );
        assert_eq!(v.get("empty"), Some(&Value::Dict(vec![])));
    }

    #[test]
    fn decodes_escapes() {
        let v = parse(r#"{ a = "q\"b\\s\012n\U0041é"; e = "\UD83D\UDE00"; c = "a\tb"; }"#).unwrap();
        assert_eq!(v.get_str("a"), Some("q\"b\\s\nnAé"));
        assert_eq!(v.get_str("e"), Some("😀"));
        assert_eq!(v.get_str("c"), Some("a\tb"));
    }

    #[test]
    fn skips_comments_and_reads_data() {
        let v = parse("// head\n{ /* c */ d = <0fA0>; k = v }").unwrap();
        assert_eq!(v.get_str("d"), Some("0fA0"));
        assert_eq!(v.get_str("k"), Some("v"));
    }

    #[test]
    fn errors_name_the_line() {
        let e = parse("{\na = 1;\nb 2;\n}").unwrap_err();
        assert!(e.contains("line 3"), "{e}");
        assert!(parse("{ a = \"open").is_err());
        assert!(parse("(1, 2").is_err());
        assert!(parse("{a=1;} extra").is_err());
        assert!(parse("").is_err());
        let deep = "(".repeat(MAX_DEPTH + 10);
        assert!(parse(&deep).is_err());
    }

    #[test]
    fn streams_a_top_level_array() {
        let mut seen = Vec::new();
        let v = parse_streaming(
            "{ a = (1); glyphs = ({n = x;}, {n = y;}); b = (glyphs); }",
            "glyphs",
            &mut |item| seen.push(item.get_str("n").unwrap_or_default().to_string()),
        )
        .unwrap();
        assert_eq!(seen, vec!["x", "y"]);
        assert_eq!(v.get("glyphs"), Some(&Value::Array(vec![])));
        assert_eq!(v.get_items("a").len(), 1);
        assert_eq!(v.get_items("b"), &[s("glyphs")]);
    }

    #[test]
    fn summary_is_short() {
        let v = parse("{ a = (1, 2); b = \"x\ny\"; }").unwrap();
        assert_eq!(v.summary(100), "{a = (1, 2); b = x ⏎ y;}");
        assert_eq!(v.summary(5), "{a = …");
    }
}
