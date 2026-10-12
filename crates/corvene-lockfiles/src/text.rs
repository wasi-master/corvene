//! Text helpers: nesting guard, JSONC cleanup, a small Elixir / Erlang
//! term reader (mix.lock, rebar.lock) and YAML / JSON value accessors.

use serde_json::Value;
use yaml_rust::Yaml;

/// Deepest bracket nesting or indentation accepted before handing text to a
/// recursive parser.
const MAX_DEPTH: usize = 200;
const MAX_INDENT: usize = 800;

/// Whether `text` nests too deep for the recursive parsers (brackets outside
/// strings, or absurd indentation for YAML).
pub(crate) fn too_deep(text: &str) -> bool {
    let mut depth = 0usize;
    let mut in_str: Option<u8> = None;
    let mut escape = false;
    let mut line_start = true;
    let mut indent = 0usize;
    for b in text.bytes() {
        if line_start {
            if b == b' ' || b == b'\t' {
                indent += 1;
                if indent > MAX_INDENT {
                    return true;
                }
                continue;
            }
            line_start = false;
        }
        if b == b'\n' {
            line_start = true;
            indent = 0;
        }
        if let Some(q) = in_str {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == q || b == b'\n' {
                in_str = None;
            }
            continue;
        }
        match b {
            b'"' => in_str = Some(b'"'),
            b'[' | b'{' | b'(' => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return true;
                }
            }
            b']' | b'}' | b')' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    false
}

/// JSON with comments and trailing commas (bun.lock) to plain JSON.
pub(crate) fn strip_jsonc(text: &str) -> String {
    let b = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'"' {
            let start = i;
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(b.len());
            out.extend_from_slice(&b[start..i]);
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(b.len());
            continue;
        }
        if c == b']' || c == b'}' {
            // Drop a trailing comma before the closer.
            let mut k = out.len();
            while k > 0 && out[k - 1].is_ascii_whitespace() {
                k -= 1;
            }
            if k > 0 && out[k - 1] == b',' {
                out.remove(k - 1);
            }
        }
        out.push(c);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_default()
}

/// Parses JSON after stripping a BOM; `None` on errors or deep nesting.
pub(crate) fn json(text: &str) -> Option<Value> {
    // serde_json limits nesting itself (128 levels).
    let t = text.trim_start_matches('\u{feff}');
    serde_json::from_str(t).ok()
}

/// Parses TOML; `None` on errors or deep nesting.
pub(crate) fn toml(text: &str) -> Option<toml::Table> {
    let t = text.trim_start_matches('\u{feff}');
    if too_deep(t) {
        return None;
    }
    t.parse::<toml::Table>().ok()
}

/// Parses the first YAML document; `None` on errors or deep nesting.
pub(crate) fn yaml(text: &str) -> Option<Yaml> {
    let t = text.trim_start_matches('\u{feff}');
    if too_deep(t) {
        return None;
    }
    let mut docs = yaml_rust::YamlLoader::load_from_str(t).ok()?;
    if docs.is_empty() {
        return None;
    }
    let d = docs.swap_remove(0);
    matches!(d, Yaml::Hash(_)).then_some(d)
}

/// A scalar YAML value as text (numbers keep their written form).
pub(crate) fn ys(y: &Yaml) -> Option<String> {
    match y {
        Yaml::String(s) | Yaml::Real(s) => Some(s.clone()),
        Yaml::Integer(i) => Some(i.to_string()),
        Yaml::Boolean(b) => Some(b.to_string()),
        _ => None,
    }
}

/// `y[key]` as text.
pub(crate) fn yget(y: &Yaml, key: &str) -> Option<String> {
    ys(&y[key])
}

/// The entries of a YAML mapping with text keys.
pub(crate) fn yhash(y: &Yaml) -> impl Iterator<Item = (String, &Yaml)> {
    y.as_hash()
        .into_iter()
        .flat_map(|h| h.iter())
        .filter_map(|(k, v)| ys(k).map(|k| (k, v)))
}

/// The items of a YAML sequence.
pub(crate) fn ylist(y: &Yaml) -> impl Iterator<Item = &Yaml> {
    y.as_vec().into_iter().flat_map(|v| v.iter())
}

/// `v[key]` as text (strings, numbers).
pub(crate) fn js(v: &Value, key: &str) -> Option<String> {
    match v.get(key)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// The entries of `v[key]` when it is an object.
pub(crate) fn jobj<'a>(v: &'a Value, key: &str) -> impl Iterator<Item = (&'a String, &'a Value)> {
    v.get(key)
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|m| m.iter())
}

/// A TOML value as text.
pub(crate) fn ts(v: Option<&toml::Value>) -> Option<String> {
    match v? {
        toml::Value::String(s) => Some(s.clone()),
        toml::Value::Integer(i) => Some(i.to_string()),
        toml::Value::Float(f) => Some(f.to_string()),
        _ => None,
    }
}

/// An Elixir / Erlang term (the subset lockfiles use).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Term {
    Str(String),
    Atom(String),
    Num(String),
    List(Vec<Term>),
    Tuple(Vec<Term>),
    Map(Vec<(Term, Term)>),
}

impl Term {
    pub fn text(&self) -> Option<&str> {
        match self {
            Term::Str(s) | Term::Atom(s) | Term::Num(s) => Some(s),
            _ => None,
        }
    }

    pub fn items(&self) -> &[Term] {
        match self {
            Term::List(v) | Term::Tuple(v) => v,
            _ => &[],
        }
    }

    /// `key: value` / `{key, value}` lookups in a keyword list.
    pub fn kw(&self, key: &str) -> Option<&Term> {
        self.items().iter().find_map(|t| match t {
            Term::Tuple(v) if v.len() == 2 && v[0].text() == Some(key) => Some(&v[1]),
            _ => None,
        })
    }
}

const MAX_TERM_DEPTH: usize = 64;

/// Reads consecutive terms (each optionally ending in `.`), as rebar.lock
/// writes them; mix.lock is one map.
pub(crate) fn terms(text: &str) -> Option<Vec<Term>> {
    let mut p = TermParser {
        b: text.as_bytes(),
        i: 0,
        kw_pending: false,
    };
    let mut out = Vec::new();
    loop {
        p.ws();
        if p.i >= p.b.len() {
            break;
        }
        out.push(p.term(0)?);
        p.ws();
        if p.peek() == Some(b'.') {
            p.i += 1;
        }
    }
    Some(out)
}

struct TermParser<'a> {
    b: &'a [u8],
    i: usize,
    /// The last term was a `key:` keyword key.
    kw_pending: bool,
}

impl TermParser<'_> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn ws(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() {
                self.i += 1;
            } else if c == b'%' && self.b.get(self.i + 1) != Some(&b'{') {
                // Erlang comment.
                while self.peek().is_some_and(|c| c != b'\n') {
                    self.i += 1;
                }
            } else if c == b'#' {
                while self.peek().is_some_and(|c| c != b'\n') {
                    self.i += 1;
                }
            } else {
                break;
            }
        }
    }

    fn string(&mut self) -> Option<String> {
        // At the opening quote.
        self.i += 1;
        let mut out = Vec::new();
        while let Some(c) = self.peek() {
            self.i += 1;
            match c {
                b'"' => return String::from_utf8(out).ok(),
                b'\\' => {
                    if let Some(n) = self.peek() {
                        out.push(n);
                        self.i += 1;
                    }
                }
                _ => out.push(c),
            }
        }
        None
    }

    fn word(&mut self) -> String {
        let st = self.i;
        while self.peek().is_some_and(|c| {
            c.is_ascii_alphanumeric() || matches!(c, b'_' | b'@' | b'?' | b'!' | b'.' | b'-' | b'+')
        }) {
            // A trailing `.` ends an Erlang form, not a word.
            if self.peek() == Some(b'.')
                && !self
                    .b
                    .get(self.i + 1)
                    .is_some_and(|c| c.is_ascii_alphanumeric())
            {
                break;
            }
            self.i += 1;
        }
        String::from_utf8_lossy(&self.b[st..self.i]).into_owned()
    }

    fn seq(&mut self, close: u8, depth: usize) -> Option<Vec<Term>> {
        let mut out = Vec::new();
        loop {
            self.ws();
            match self.peek()? {
                c if c == close => {
                    self.i += 1;
                    return Some(out);
                }
                b',' => self.i += 1,
                _ => {
                    let t = self.term(depth + 1)?;
                    // Elixir keyword shorthand `key: value`.
                    if self.kw_pending {
                        self.kw_pending = false;
                        let k = t.text().unwrap_or_default().to_string();
                        let v = self.term(depth + 1)?;
                        out.push(Term::Tuple(vec![Term::Atom(k), v]));
                    } else {
                        out.push(t);
                    }
                }
            }
        }
    }

    fn term(&mut self, depth: usize) -> Option<Term> {
        if depth > MAX_TERM_DEPTH {
            return None;
        }
        self.ws();
        let c = self.peek()?;
        let t = match c {
            b'%' => {
                self.i += 1;
                if self.peek() != Some(b'{') {
                    return None;
                }
                self.i += 1;
                let mut pairs = Vec::new();
                loop {
                    self.ws();
                    match self.peek()? {
                        b'}' => {
                            self.i += 1;
                            break;
                        }
                        b',' => self.i += 1,
                        _ => {
                            let k = self.term(depth + 1)?;
                            self.ws();
                            if self.kw_pending {
                                self.kw_pending = false;
                            } else if self.b[self.i..].starts_with(b"=>") {
                                self.i += 2;
                            } else if self.peek() == Some(b':') {
                                self.i += 1;
                            }
                            let v = self.term(depth + 1)?;
                            pairs.push((k, v));
                        }
                    }
                }
                Term::Map(pairs)
            }
            b'{' => {
                self.i += 1;
                Term::Tuple(self.seq(b'}', depth)?)
            }
            b'[' => {
                self.i += 1;
                Term::List(self.seq(b']', depth)?)
            }
            b'<' => {
                if !self.b[self.i..].starts_with(b"<<") {
                    return None;
                }
                self.i += 2;
                self.ws();
                let s = if self.peek() == Some(b'"') {
                    self.string()?
                } else {
                    String::new()
                };
                self.ws();
                if !self.b[self.i..].starts_with(b">>") {
                    return None;
                }
                self.i += 2;
                Term::Str(s)
            }
            b'"' => {
                let s = self.string()?;
                // `"name": value` keys in Elixir maps.
                if self.peek() == Some(b':') && self.b.get(self.i + 1) != Some(&b':') {
                    self.i += 1;
                    self.kw_pending = true;
                }
                Term::Str(s)
            }
            b'\'' => {
                self.i += 1;
                let st = self.i;
                while self.peek().is_some_and(|c| c != b'\'') {
                    self.i += 1;
                }
                let s = String::from_utf8_lossy(&self.b[st..self.i]).into_owned();
                self.i += 1;
                Term::Atom(s)
            }
            b':' => {
                self.i += 1;
                if self.peek() == Some(b'"') {
                    Term::Atom(self.string()?)
                } else {
                    Term::Atom(self.word())
                }
            }
            c if c.is_ascii_digit() || c == b'-' => Term::Num(self.word()),
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let w = self.word();
                if w.is_empty() {
                    return None;
                }
                if self.peek() == Some(b':') && self.b.get(self.i + 1) != Some(&b':') {
                    self.i += 1;
                    self.kw_pending = true;
                }
                Term::Atom(w)
            }
            _ => return None,
        };
        Some(t)
    }
}
