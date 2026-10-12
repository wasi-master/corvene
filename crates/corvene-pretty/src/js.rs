//! JavaScript / TypeScript printer in the js-beautify spirit, at token
//! level: newline + indent after `{` and before `}`, a newline after `;`
//! (not in `for (...)` headers), after `}` when a statement follows, and
//! after commas directly inside `{}`; spaces around binary operators and
//! after commas and keywords. Every source newline is kept, so code that
//! relies on automatic semicolon insertion keeps its meaning, and no
//! newline is ever inserted after `return`, `throw`, `break`, `continue`,
//! `yield` or `async`. Template literal substitutions print on one line.
//!
//! The output tokenizes to exactly the input's token sequence
//! ([`tokenize`] is exposed to the tests for that check).

use crate::out::Out;

/// Token kinds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Identifier, keyword or private name (`#x`).
    Word,
    /// Numeric literal.
    Number,
    /// Quoted string.
    Str,
    /// Template literal without substitutions.
    Template,
    /// `` `...${ ``.
    TemplateHead,
    /// `}...${`.
    TemplateMiddle,
    /// `` }...` ``.
    TemplateTail,
    /// Regular expression literal.
    Regex,
    /// Operator or punctuation.
    Punct,
    /// `/* ... */`.
    BlockComment,
    /// `// ...` (or a `#!` line at the start).
    LineComment,
}

/// One token: its kind, byte range and whether a line break preceded it.
#[derive(Clone, Copy, Debug)]
pub struct Token {
    /// What it is.
    pub kind: Kind,
    /// Byte offset of its first byte.
    pub start: usize,
    /// Byte offset past its last byte.
    pub end: usize,
    /// A line terminator sat between it and the previous token.
    pub newline_before: bool,
}

const PUNCTS: [&str; 52] = [
    ">>>=", "...", "===", "!==", "**=", "<<=", ">>=", ">>>", "&&=", "||=", "??=", "=>", "==", "!=",
    "<=", ">=", "&&", "||", "??", "?.", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=",
    "<<", ">>", "**", "{", "}", "(", ")", "[", "]", ";", ",", "<", ">", "+", "-", "*", "/", "%",
    "&", "|", "^", "!",
];
const PUNCT_SINGLES: &[u8] = b"~?:=.@#";

/// Length of the longest punctuator at the start of `b` (0 if none).
fn punct_len(b: &[u8]) -> usize {
    match b.first() {
        None => return 0,
        Some(b'{' | b'}' | b'(' | b')' | b'[' | b']' | b';' | b',' | b'~' | b':' | b'@' | b'#') => {
            return 1;
        }
        Some(c) if c.is_ascii_alphanumeric() || *c == b'"' || *c == b'\'' || *c >= 0x80 => {
            return 0;
        }
        _ => {}
    }
    for p in PUNCTS {
        if b.starts_with(p.as_bytes()) {
            if p == "?." && b.get(2).is_some_and(|c| c.is_ascii_digit()) {
                continue;
            }
            return p.len();
        }
    }
    match b.first() {
        Some(c) if PUNCT_SINGLES.contains(c) => 1,
        _ => 0,
    }
}

const REGEX_AFTER_WORDS: &[&str] = &[
    "return",
    "typeof",
    "instanceof",
    "in",
    "of",
    "new",
    "delete",
    "void",
    "throw",
    "case",
    "do",
    "else",
    "yield",
    "await",
];

/// Words after which the next token starts an expression (they never end
/// one).
const OPERATOR_WORDS: &[&str] = &[
    "return",
    "typeof",
    "instanceof",
    "in",
    "of",
    "new",
    "delete",
    "void",
    "throw",
    "case",
    "do",
    "else",
    "yield",
    "await",
    "extends",
    "if",
    "for",
    "while",
    "switch",
    "catch",
    "with",
    "const",
    "let",
    "var",
    "export",
    "import",
    "from",
    "as",
    "satisfies",
    "keyof",
    "is",
    "default",
];

/// Keywords that take a space before a following `(`.
const PAREN_SPACE_WORDS: &[&str] = &[
    "if",
    "for",
    "while",
    "switch",
    "catch",
    "with",
    "return",
    "typeof",
    "void",
    "delete",
    "await",
    "yield",
    "in",
    "instanceof",
    "new",
    "case",
    "throw",
    "else",
    "do",
    "extends",
    "export",
];

/// No line break may follow these.
const RESTRICTED: &[&str] = &["return", "throw", "break", "continue", "yield", "async"];

fn ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b'$' || c == b'\\' || c >= 0x80
}

fn ident_part(c: u8) -> bool {
    ident_start(c) || c.is_ascii_digit()
}

/// Splits `text` into tokens (whitespace dropped, comments kept).
pub fn tokenize(text: &str) -> Vec<Token> {
    let bytes = text.as_bytes();
    let mut tokens: Vec<Token> = Vec::with_capacity(text.len() / 3 + 1);
    // Brace depth inside each open template substitution.
    let mut templates: Vec<u32> = Vec::new();
    // Last non-comment token kind and text, for the regex rule.
    let mut prev: Option<(Kind, usize, usize)> = None;
    let mut i = 0;
    let mut newline = false;
    if text.starts_with("#!") {
        i = text.find('\n').unwrap_or(text.len());
        tokens.push(Token {
            kind: Kind::LineComment,
            start: 0,
            end: i,
            newline_before: false,
        });
    }
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\n' || c == b'\r' {
            newline = true;
            i += 1;
            continue;
        }
        if c == b' ' || c == b'\t' || c == b'\x0b' || c == b'\x0c' {
            i += 1;
            continue;
        }
        if c >= 0x80
            && let Some(ch) = text[i..].chars().next()
        {
            if ch == '\u{2028}' || ch == '\u{2029}' {
                newline = true;
                i += ch.len_utf8();
                continue;
            }
            if ch.is_whitespace() || ch == '\u{feff}' {
                i += ch.len_utf8();
                continue;
            }
        }
        let start = i;
        let kind;
        if c == b'/' && bytes.get(i + 1) == Some(&b'/') {
            while i < bytes.len() && bytes[i] != b'\n' && bytes[i] != b'\r' {
                i += 1;
            }
            kind = Kind::LineComment;
        } else if c == b'/' && bytes.get(i + 1) == Some(&b'*') {
            i = match text[i + 2..].find("*/") {
                Some(p) => i + 2 + p + 2,
                None => bytes.len(),
            };
            kind = Kind::BlockComment;
        } else if c == b'"' || c == b'\'' {
            i += 1;
            while i < bytes.len() && bytes[i] != c && bytes[i] != b'\n' {
                i += if bytes[i] == b'\\' { 2 } else { 1 };
            }
            if i < bytes.len() && bytes[i] == c {
                i += 1;
            }
            kind = Kind::Str;
        } else if c == b'`' {
            let (end, open) = scan_template(bytes, i + 1);
            i = end;
            if open {
                templates.push(0);
                kind = Kind::TemplateHead;
            } else {
                kind = Kind::Template;
            }
        } else if c == b'}' && templates.last() == Some(&0) {
            let (end, open) = scan_template(bytes, i + 1);
            i = end;
            if open {
                kind = Kind::TemplateMiddle;
            } else {
                templates.pop();
                kind = Kind::TemplateTail;
            }
        } else if c.is_ascii_digit()
            || (c == b'.' && bytes.get(i + 1).is_some_and(|d| d.is_ascii_digit()))
        {
            i = scan_number(bytes, i);
            kind = Kind::Number;
        } else if ident_start(c) || (c == b'#' && bytes.get(i + 1).is_some_and(|&d| ident_start(d)))
        {
            i += 1;
            while i < bytes.len() && ident_part(bytes[i]) {
                if bytes[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            kind = Kind::Word;
        } else if c == b'/' && regex_allowed(text, prev) {
            match scan_regex(bytes, i) {
                Some(end) => {
                    i = end;
                    kind = Kind::Regex;
                }
                None => {
                    i += punct_len(&bytes[i..]).max(1);
                    kind = Kind::Punct;
                }
            }
        } else {
            let n = punct_len(&bytes[i..]);
            if n == 0 {
                // Stray byte (`\x00`, a lone backslash, ...): its own token.
                i += text[i..].chars().next().map_or(1, char::len_utf8);
                kind = Kind::Word;
            } else {
                i += n;
                kind = Kind::Punct;
                if let Some(depth) = templates.last_mut() {
                    match c {
                        b'{' => *depth += 1,
                        b'}' => *depth = depth.saturating_sub(1),
                        _ => {}
                    }
                }
            }
        }
        i = i.min(bytes.len());
        while i < bytes.len() && !text.is_char_boundary(i) {
            i += 1;
        }
        tokens.push(Token {
            kind,
            start,
            end: i,
            newline_before: newline,
        });
        newline = false;
        if !matches!(kind, Kind::LineComment | Kind::BlockComment) {
            prev = Some((kind, start, i));
        }
    }
    tokens
}

/// Scans template characters from `i` to the closing backtick (exclusive
/// of nothing: the returned end is past it) or past a `${`; returns the end
/// and whether a substitution opened.
fn scan_template(bytes: &[u8], mut i: usize) -> (usize, bool) {
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'`' => return (i + 1, false),
            b'$' if bytes.get(i + 1) == Some(&b'{') => return (i + 2, true),
            _ => i += 1,
        }
    }
    (bytes.len(), false)
}

fn scan_number(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    let radix = bytes[i] == b'0'
        && matches!(
            bytes.get(i + 1),
            Some(b'x' | b'X' | b'b' | b'B' | b'o' | b'O')
        );
    let mut dot = false;
    let mut exp = false;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'.' {
            if dot || exp || radix {
                break;
            }
            dot = true;
        } else if i > start
            && (c == b'+' || c == b'-')
            && !radix
            && matches!(bytes[i - 1], b'e' | b'E')
        {
            exp = true;
        } else if c.is_ascii_alphanumeric() || c == b'_' {
            if !radix && (c == b'e' || c == b'E') {
                exp = true;
            }
        } else {
            break;
        }
        i += 1;
    }
    i
}

fn is_punct_byte(b: u8) -> bool {
    b.is_ascii_punctuation() && !matches!(b, b'"' | b'\'' | b'`' | b'_' | b'$' | b'\\')
}

/// Whether the number `pb` would swallow a `.` glued after it.
fn number_takes_dot(pb: &[u8]) -> bool {
    let radix =
        pb.len() > 1 && pb[0] == b'0' && matches!(pb[1], b'x' | b'X' | b'b' | b'B' | b'o' | b'O');
    !radix && !pb.iter().any(|b| matches!(b, b'.' | b'e' | b'E'))
}

fn regex_allowed(text: &str, prev: Option<(Kind, usize, usize)>) -> bool {
    let Some((kind, start, end)) = prev else {
        return true;
    };
    let s = &text[start..end];
    match kind {
        Kind::Word => REGEX_AFTER_WORDS.contains(&s),
        Kind::Punct => !matches!(s, ")" | "]" | "}"),
        Kind::TemplateHead | Kind::TemplateMiddle => true,
        _ => false,
    }
}

/// End of a regex literal starting at `start`, or `None` if it does not
/// close on this line.
fn scan_regex(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start + 1;
    let mut class = false;
    loop {
        match *bytes.get(i)? {
            b'\n' | b'\r' => return None,
            b'\\' if matches!(bytes.get(i + 1), Some(b'\n' | b'\r')) => return None,
            b'\\' => i += 2,
            b'[' => {
                class = true;
                i += 1;
            }
            b']' => {
                class = false;
                i += 1;
            }
            b'/' if !class => break,
            _ => i += 1,
        }
    }
    i += 1;
    while i < bytes.len() && ident_part(bytes[i]) {
        i += 1;
    }
    Some(i)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Bracket {
    Brace,
    Paren {
        for_header: bool,
        switch_header: bool,
    },
    Square,
    Subst,
    /// TypeScript type arguments `<...>` (commas stay inline).
    Angle,
}

#[derive(Clone, Copy)]
struct Frame {
    bracket: Bracket,
    /// Indent to restore when this frame closes.
    indent_before: usize,
    /// Inside a template substitution: structural newlines become spaces.
    inline: bool,
    /// Open ternary `?`s waiting for their `:`.
    ternary: u32,
    /// Between `case`/`default` and its `:`.
    in_case: bool,
    /// The body of a `switch`: case labels outdent one level.
    switch_body: bool,
}

struct Printer<'a> {
    text: &'a str,
    tokens: &'a [Token],
    out: Out,
    stack: Vec<Frame>,
    /// Line break owed before the next token.
    newline_owed: bool,
    /// Space owed before the next token (after an inline comment).
    space_owed: bool,
    /// TypeScript: `<` and `>` keep their source spacing (generics).
    ts: bool,
    /// The last `)` closed a `switch (...)` header.
    closed_switch: bool,
}

/// Pretty prints JavaScript or TypeScript.
pub(crate) fn pretty(text: &str, ts: bool) -> String {
    let tokens = tokenize(text);
    let mut p = Printer {
        text,
        tokens: &tokens,
        out: Out::with_capacity(text.len() + text.len() / 2),
        stack: vec![Frame {
            bracket: Bracket::Brace,
            indent_before: 0,
            inline: false,
            ternary: 0,
            in_case: false,
            switch_body: false,
        }],
        newline_owed: false,
        space_owed: false,
        ts,
        closed_switch: false,
    };
    // Index of the last non-comment token printed.
    let mut prev_sig: Option<usize> = None;
    let mut prev_sig2: Option<usize> = None;
    let mut idx = 0;
    while idx < tokens.len() {
        let t = tokens[idx];
        if matches!(t.kind, Kind::LineComment | Kind::BlockComment) {
            p.comment(idx, prev_sig);
            idx += 1;
            continue;
        }
        p.token(idx, prev_sig, prev_sig2);
        prev_sig2 = prev_sig;
        prev_sig = Some(idx);
        idx += 1;
        // A `/` where a regex could start but none closed on this line: the
        // rest of the line is copied as is, since added spaces could close
        // a regex there.
        let prev_info = prev_sig2.map(|q| (tokens[q].kind, tokens[q].start, tokens[q].end));
        if t.kind == Kind::Punct
            && text[t.start..].starts_with('/')
            && regex_allowed(text, prev_info)
        {
            let line_end = tokens[idx..]
                .iter()
                .position(|n| n.newline_before)
                .map_or(tokens.len(), |n| idx + n);
            if line_end > idx {
                p.out.text(&text[t.end..tokens[line_end - 1].end]);
                p.stray_line();
                prev_sig2 = None;
                prev_sig = Some(line_end - 1);
                idx = line_end;
            }
        }
    }
    p.out.finish()
}

impl<'a> Printer<'a> {
    fn s(&self, idx: usize) -> &'a str {
        let t = self.tokens[idx];
        &self.text[t.start..t.end]
    }

    fn top(&self) -> Frame {
        self.stack.last().copied().unwrap_or(Frame {
            bracket: Bracket::Brace,
            indent_before: 0,
            inline: false,
            ternary: 0,
            in_case: false,
            switch_body: false,
        })
    }

    /// The previous non-comment token before `idx`.
    fn prev_sig_of(&self, idx: usize) -> Option<usize> {
        self.tokens[..idx]
            .iter()
            .rposition(|t| !matches!(t.kind, Kind::LineComment | Kind::BlockComment))
    }

    /// The word at `idx` when it can be a keyword (not a property name
    /// after `.` or `?.`).
    fn keyword(&self, idx: usize) -> Option<&'a str> {
        if self.tokens[idx].kind != Kind::Word {
            return None;
        }
        match self.prev_sig_of(idx) {
            Some(p) if self.tokens[p].kind == Kind::Punct && matches!(self.s(p), "." | "?.") => {
                None
            }
            _ => Some(self.s(idx)),
        }
    }

    fn is_op_word(&self, idx: usize) -> bool {
        self.keyword(idx)
            .is_some_and(|w| OPERATOR_WORDS.contains(&w))
    }

    /// After a verbatim line: drop owed separators (the next token starts a
    /// new source line anyway).
    fn stray_line(&mut self) {
        self.newline_owed = true;
        self.space_owed = false;
    }

    fn top_mut(&mut self) -> Option<&mut Frame> {
        self.stack.last_mut()
    }

    fn is_restricted(&self, idx: Option<usize>) -> bool {
        idx.is_some_and(|i| self.keyword(i).is_some_and(|w| RESTRICTED.contains(&w)))
    }

    /// Whether token `idx` ends an expression (so a following `+` is binary
    /// and a following `++` is postfix).
    fn value_end(&self, idx: Option<usize>) -> bool {
        let Some(i) = idx else {
            return false;
        };
        let s = self.s(i);
        match self.tokens[i].kind {
            Kind::Word => !self.is_op_word(i),
            Kind::Number | Kind::Str | Kind::Template | Kind::TemplateTail | Kind::Regex => true,
            Kind::Punct => match s {
                ")" | "]" | "}" => true,
                "++" | "--" | "!" => self.is_postfix(i),
                _ => false,
            },
            _ => false,
        }
    }

    /// Whether the `++`/`--`/`!` at `idx` is postfix.
    fn is_postfix(&self, idx: usize) -> bool {
        if self.tokens[idx].newline_before {
            return false;
        }
        match self.prev_sig_of(idx) {
            None => false,
            // Depth is bounded: a chain of postfix operators is rare and
            // each step moves back one token.
            Some(p) => self.value_end_shallow(p),
        }
    }

    /// `value_end` without the postfix recursion (treats `++`/`--`/`!` as
    /// not ending a value).
    fn value_end_shallow(&self, i: usize) -> bool {
        let s = self.s(i);
        match self.tokens[i].kind {
            Kind::Word => !self.is_op_word(i),
            Kind::Number | Kind::Str | Kind::Template | Kind::TemplateTail | Kind::Regex => true,
            Kind::Punct => matches!(s, ")" | "]" | "}"),
            _ => false,
        }
    }

    fn comment(&mut self, idx: usize, prev_sig: Option<usize>) {
        let t = self.tokens[idx];
        let s = self.s(idx);
        let restricted = self.is_restricted(prev_sig);
        let inline_frame = self.top().inline;
        if t.kind == Kind::LineComment {
            if t.newline_before || self.newline_owed {
                self.out.newline();
            } else {
                self.out.space();
            }
            self.newline_owed = false;
            self.out.text(s);
            // The line comment runs to the line end: something must break.
            self.out.newline();
            return;
        }
        let own_line = !restricted
            && !inline_frame
            && (t.newline_before
                || self.newline_owed
                || s.starts_with("/*!")
                || s.starts_with("/**")
                || s.contains('\n'));
        if own_line || t.newline_before {
            self.out.newline();
        } else {
            self.out.space();
        }
        self.newline_owed = false;
        self.out.text(s);
        if own_line {
            self.newline_owed = true;
        } else {
            self.space_owed = true;
        }
    }

    fn token(&mut self, idx: usize, prev: Option<usize>, prev2: Option<usize>) {
        let t = self.tokens[idx];
        let s = self.s(idx);
        let frame = self.top();
        let prev_s = prev.map(|p| self.s(p)).unwrap_or("");
        let prev_kind = prev.map(|p| self.tokens[p].kind);

        // Line break after a `}` when a statement follows.
        let mut newline = t.newline_before || self.newline_owed;
        if prev_kind == Some(Kind::Punct)
            && prev_s == "}"
            && !frame.inline
            && self.closed_brace_breaks(idx)
        {
            newline = true;
        }

        // Closing brackets: pop frames, set the indent.
        match (t.kind, s) {
            (Kind::Punct, "}") => {
                let empty = prev_kind == Some(Kind::Punct) && prev_s == "{" && !t.newline_before;
                let popped = self.pop(Bracket::Brace);
                if let Some(f) = popped {
                    self.out.indent = f.indent_before;
                    if !f.inline && !empty {
                        newline = true;
                    }
                }
            }
            (Kind::Punct, ")") => {
                self.closed_switch = false;
                if let Some(f) = self.pop_paren() {
                    self.out.indent = f.indent_before;
                    self.closed_switch = matches!(
                        f.bracket,
                        Bracket::Paren {
                            switch_header: true,
                            ..
                        }
                    );
                }
            }
            (Kind::Punct, "]") => {
                if let Some(f) = self.pop(Bracket::Square) {
                    self.out.indent = f.indent_before;
                }
            }
            (Kind::TemplateMiddle | Kind::TemplateTail, _) => {
                if let Some(f) = self.pop(Bracket::Subst) {
                    self.out.indent = f.indent_before;
                }
            }
            _ => {}
        }
        let frame = self.top();
        let case_label = frame.switch_body
            && matches!(self.keyword(idx), Some("case" | "default"))
            && prev.is_none_or(|p| {
                self.tokens[p].kind == Kind::Punct && matches!(self.s(p), "{" | ";" | "}" | ":")
            });
        if case_label {
            self.out.indent = frame.indent_before + 1;
            newline = true;
        }

        if newline && (!self.is_restricted(prev) || matches!(s, "}" | ";")) {
            self.out.newline();
        } else if newline {
            // A source line break after `return` etc. must stay.
            if t.newline_before {
                self.out.newline();
            } else {
                self.out.space();
            }
        } else if self.space_owed || self.space_between(prev, prev2, idx, frame) {
            self.out.space();
        }
        self.newline_owed = false;
        self.space_owed = false;
        self.out.text(s);

        // Openers and statement ends.
        let next_closes = self.tokens.get(idx + 1).is_some_and(|n| {
            n.kind == Kind::Punct && !n.newline_before && &self.text[n.start..n.end] == "}"
        });
        match (t.kind, s) {
            (Kind::Punct, "{") => {
                let inline = frame.inline;
                let indent_before = self.out.indent;
                let switch_body = self.closed_switch && prev_s == ")";
                self.stack.push(Frame {
                    bracket: Bracket::Brace,
                    indent_before,
                    inline,
                    ternary: 0,
                    in_case: false,
                    switch_body,
                });
                if !inline {
                    self.out.indent += 1;
                    if !next_closes {
                        self.newline_owed = true;
                    }
                } else if !next_closes {
                    self.space_owed = true;
                }
            }
            (Kind::Punct, "(") => {
                let prev_kw = prev.and_then(|p| self.keyword(p));
                let for_header = prev_kw == Some("for")
                    || (prev_kw == Some("await") && prev2.is_some_and(|p| self.s(p) == "for"));
                let switch_header = prev_kw == Some("switch");
                self.push(
                    Bracket::Paren {
                        for_header,
                        switch_header,
                    },
                    frame.inline,
                );
            }
            (Kind::Punct, "[") => self.push(Bracket::Square, frame.inline),
            (Kind::Punct, "<") if self.ts && self.opens_type_args(idx, prev) => {
                self.push(Bracket::Angle, frame.inline)
            }
            (Kind::Punct, ">" | ">>" | ">>>") => {
                for _ in 0..s.len() {
                    if self.top().bracket == Bracket::Angle && self.stack.len() > 1 {
                        self.stack.pop();
                    }
                }
            }
            (Kind::TemplateHead | Kind::TemplateMiddle, _) => self.push(Bracket::Subst, true),
            (Kind::Punct, ";") => {
                let in_header = matches!(
                    frame.bracket,
                    Bracket::Paren {
                        for_header: true,
                        ..
                    }
                );
                if let Some(f) = self.top_mut() {
                    f.in_case = false;
                    f.ternary = 0;
                }
                if !in_header && !frame.inline {
                    self.newline_owed = true;
                }
            }
            (Kind::Punct, ",") => {
                if frame.bracket == Bracket::Brace && !frame.inline && frame.ternary == 0 {
                    self.newline_owed = true;
                }
            }
            (Kind::Punct, "?") => {
                if !self.is_optional_mark(idx)
                    && let Some(f) = self.top_mut()
                {
                    f.ternary += 1;
                }
            }
            (Kind::Punct, ":") => {
                if frame.ternary > 0 {
                    if let Some(f) = self.top_mut() {
                        f.ternary -= 1;
                    }
                } else if frame.in_case {
                    if let Some(f) = self.top_mut() {
                        f.in_case = false;
                    }
                    if !frame.inline {
                        self.out.indent = frame.indent_before + 2;
                        self.newline_owed = true;
                    }
                }
            }
            _ if case_label => {
                if let Some(f) = self.top_mut() {
                    f.in_case = true;
                }
            }
            _ => {}
        }
    }

    fn push(&mut self, bracket: Bracket, inline: bool) {
        let indent_before = self.out.indent;
        self.stack.push(Frame {
            bracket,
            indent_before,
            inline,
            ternary: 0,
            in_case: false,
            switch_body: false,
        });
    }

    /// Pops up to and including the nearest frame of kind `want` (searching
    /// at most a few frames, so mismatched closers cost O(1)). The root
    /// frame is never popped.
    fn pop(&mut self, want: Bracket) -> Option<Frame> {
        let len = self.stack.len();
        let found = (1..len)
            .rev()
            .take(8)
            .find(|&i| self.stack[i].bracket == want)?;
        let frame = self.stack[found];
        self.stack.truncate(found);
        Some(frame)
    }

    fn pop_paren(&mut self) -> Option<Frame> {
        let len = self.stack.len();
        let found = (1..len)
            .rev()
            .take(8)
            .find(|&i| matches!(self.stack[i].bracket, Bracket::Paren { .. }))?;
        let frame = self.stack[found];
        self.stack.truncate(found);
        Some(frame)
    }

    /// A `<` glued to a type name on both sides (`Map<string`), taken as
    /// opening type arguments.
    fn opens_type_args(&self, idx: usize, prev: Option<usize>) -> bool {
        let t = self.tokens[idx];
        let glued_before = prev.is_some_and(|p| {
            self.tokens[p].end == t.start && matches!(self.tokens[p].kind, Kind::Word)
        });
        let glued_after = self.tokens.get(idx + 1).is_some_and(|n| n.start == t.end);
        glued_before && glued_after
    }

    /// A `?` that marks a TypeScript optional (`x?: T`, `(a?, b)`).
    fn is_optional_mark(&self, idx: usize) -> bool {
        self.tokens.get(idx + 1).is_some_and(|n| {
            n.kind == Kind::Punct && matches!(&self.text[n.start..n.end], ":" | ")" | "," | "=")
        })
    }

    /// Whether a line break follows the `}` just before token `idx`.
    fn closed_brace_breaks(&self, idx: usize) -> bool {
        let t = self.tokens[idx];
        let s = self.s(idx);
        match t.kind {
            Kind::Word => !matches!(
                s,
                "else"
                    | "catch"
                    | "finally"
                    | "while"
                    | "in"
                    | "of"
                    | "instanceof"
                    | "as"
                    | "satisfies"
                    | "from"
                    | "extends"
                    | "implements"
            ),
            Kind::Punct => {
                matches!(s, "}" | "{" | "!" | "~" | "@")
                    || (s == "*"
                        && self.tokens.get(idx + 1).is_some_and(|n| {
                            n.kind == Kind::Word || &self.text[n.start..n.end] == "["
                        }))
            }
            Kind::Str | Kind::Number | Kind::Regex => true,
            _ => false,
        }
    }

    /// Whether to print a space between `prev` and token `idx` on one line.
    fn space_between(
        &self,
        prev: Option<usize>,
        prev2: Option<usize>,
        idx: usize,
        frame: Frame,
    ) -> bool {
        let Some(p) = prev else {
            return false;
        };
        let pt = self.tokens[p];
        let ct = self.tokens[idx];
        let ps = self.s(p);
        let cs = self.s(idx);
        if self.must_separate(p, idx) {
            return true;
        }
        let source_space = pt.end < ct.start;
        let punct = |k: Kind| k == Kind::Punct;
        let pk = self.keyword(p);
        let pw = |w: &[&str]| pk.is_some_and(|k| w.contains(&k));

        // Template boundaries hug their expressions.
        if matches!(pt.kind, Kind::TemplateHead | Kind::TemplateMiddle)
            || matches!(ct.kind, Kind::TemplateMiddle | Kind::TemplateTail)
        {
            return false;
        }
        // `<`, `>` (comparisons or TypeScript generics): keep the source.
        let angle = |k: Kind, s: &str| punct(k) && matches!(s, "<" | ">" | ">>" | ">>>");
        if angle(pt.kind, ps) || angle(ct.kind, cs) {
            return if self.ts {
                source_space
            } else {
                !(punct(pt.kind) && matches!(ps, "(" | "["))
            };
        }
        if punct(ct.kind) {
            match cs {
                ")" | "]" | "," | ";" | "." | "?." => return false,
                "}" if ps == "{" => return false,
                ":" => return frame.ternary > 0,
                "?" => return !self.is_optional_mark(idx),
                "!" if self.is_postfix(idx) => return false,
                "++" | "--" if self.is_postfix(idx) => return false,
                _ => {}
            }
        }
        if punct(pt.kind) {
            match ps {
                "(" | "[" | "." | "?." | "..." | "~" | "@" | "#" => return false,
                "!" if !self.is_postfix(p) => return false,
                "++" | "--" if !self.is_postfix(p) => return false,
                "+" | "-" if !self.value_end(prev2) => return false,
                "*" if prev2.is_some_and(|q| matches!(self.s(q), "function" | "yield")) => {
                    return true;
                }
                // A generator method marker: `*name() {}`.
                "*" if prev2.is_none_or(|q| {
                    self.tokens[q].kind == Kind::Punct && matches!(self.s(q), "{" | "}" | ";" | ",")
                }) =>
                {
                    return false;
                }
                _ => {}
            }
        }
        if punct(ct.kind) && cs == "*" && pw(&["function", "yield"]) {
            return false;
        }
        if punct(ct.kind) && cs == "(" {
            return match pt.kind {
                Kind::Word => pw(PAREN_SPACE_WORDS),
                Kind::Punct => !matches!(ps, ")" | "]" | "}" | "!"),
                _ => false,
            };
        }
        if punct(ct.kind) && cs == "[" {
            return match pt.kind {
                Kind::Word => pw(OPERATOR_WORDS),
                Kind::Punct => !matches!(ps, ")" | "]" | "}"),
                _ => false,
            };
        }
        if punct(ct.kind) && cs == "{" {
            return true;
        }
        if punct(ct.kind) && is_binary(cs) && (!matches!(cs, "+" | "-") || self.value_end(prev)) {
            return true;
        }
        if punct(pt.kind)
            && (is_binary(ps) || matches!(ps, "," | ";" | "=>" | ":" | "?" | "{" | "}"))
        {
            return true;
        }
        if punct(ct.kind) && cs == "=>" {
            return true;
        }
        if pt.kind == Kind::Word || ct.kind == Kind::Word {
            // Words next to words, literals or closers.
            if ct.kind == Kind::Word && pt.kind != Kind::Punct {
                return true;
            }
            if pw(OPERATOR_WORDS) {
                return true;
            }
            if ct.kind == Kind::Word && punct(pt.kind) && matches!(ps, ")" | "]" | "}") {
                return true;
            }
        }
        if matches!(pt.kind, Kind::Word | Kind::Number | Kind::Str | Kind::Regex)
            && matches!(ct.kind, Kind::Number | Kind::Str | Kind::Regex)
        {
            return true;
        }
        source_space
    }

    /// Whether printing `prev` and `cur` with nothing between them would
    /// tokenize differently.
    fn must_separate(&self, p: usize, c: usize) -> bool {
        let pt = self.tokens[p];
        let ct = self.tokens[c];
        let pb = self.s(p).as_bytes();
        let cb = self.s(c).as_bytes();
        let (Some(&plast), Some(&cfirst)) = (pb.last(), cb.first()) else {
            return false;
        };
        if ident_part(plast) && ident_part(cfirst) {
            return true;
        }
        if pt.kind == Kind::Number && matches!(plast, b'e' | b'E') && matches!(cfirst, b'+' | b'-')
        {
            return true;
        }
        if plast == b'#' && pt.kind == Kind::Punct && (ident_start(cfirst) || cfirst == b'!') {
            return true;
        }
        if pt.kind == Kind::Regex && ident_part(cfirst) {
            return true;
        }
        if pt.kind == Kind::Number
            && (ct.kind == Kind::Number || (cfirst == b'.' && number_takes_dot(pb)))
        {
            return true;
        }
        if plast == b'.' && cfirst.is_ascii_digit() {
            return true;
        }
        if (plast == b'/' && matches!(pt.kind, Kind::Punct | Kind::Regex))
            && (cfirst == b'/' || cfirst == b'*')
        {
            return true;
        }
        if pt.kind == Kind::Punct
            && (ct.kind == Kind::Punct || ct.kind == Kind::Regex || ct.kind == Kind::Number)
        {
            // The printed tail may hold several glued punctuators (`.` `.`),
            // so check every punctuator start in its last bytes.
            let out = self.out.buf.as_bytes();
            let tail = &out[out.len().saturating_sub(4)..];
            let glued = tail.len() - tail.iter().rev().take_while(|b| is_punct_byte(**b)).count();
            let tail = &tail[glued..];
            let m = cb.len().min(4);
            for k in 0..tail.len() {
                let mut buf = [0u8; 8];
                let n = tail.len() - k;
                buf[..n].copy_from_slice(&tail[k..]);
                buf[n..n + m].copy_from_slice(&cb[..m]);
                if punct_len(&buf[..n + m]) > n {
                    return true;
                }
            }
        }
        false
    }
}

fn is_binary(s: &str) -> bool {
    matches!(
        s,
        "=" | "=="
            | "==="
            | "!="
            | "!=="
            | "<="
            | ">="
            | "+"
            | "-"
            | "*"
            | "/"
            | "%"
            | "**"
            | "&&"
            | "||"
            | "??"
            | "&"
            | "|"
            | "^"
            | "<<"
            | "+="
            | "-="
            | "*="
            | "/="
            | "%="
            | "**="
            | "<<="
            | ">>="
            | ">>>="
            | "&="
            | "|="
            | "^="
            | "&&="
            | "||="
            | "??="
            | "=>"
    )
}
