//! LaTeX source → [`markdown::Block`]s, for Corvene `1332-latex-diff`: a
//! `.tex` file is shown rendered next to its text diff, through the same
//! GitHub-style block renderer as Markdown. GHD has no rendered LaTeX view
//! (`app/src/ui/diff/` shows `.tex` as text only).
//!
//! This is a tolerant reader, not TeX: it never fails, and whatever it does
//! not know degrades to text. It reads what papers and theses are made of:
//! the preamble's `\title` / `\author` / `\date`, macro definitions and
//! `\newtheorem`s; sectioning with LaTeX's numbering (and `\appendix`);
//! paragraphs with TeX's ligatures, accents and escapes; inline styles,
//! links, footnotes, citations and cross references (resolved in a second
//! pass, so forward `\ref`s get their number); inline and display math,
//! numbered like amsmath (math is passed through as TeX for the KaTeX-style
//! engine, with `align` → `aligned` and friends); lists; quotes; alignment
//! environments; theorem-likes and proofs; figures and tables with their
//! captions; `tabular`s; verbatim and listings as code blocks. Simple text
//! macros (`\newcommand{\name}{Corvene}`) are expanded in text, within a
//! size budget; all definitions also go to [`LatexDoc::macros`] for the
//! math engine.
//!
//! Choices where LaTeX has no Markdown equivalent: `abstract` is a level-3
//! "Abstract" heading followed by its paragraphs; theorem-likes are block
//! quotes starting with a bold "Theorem 3." (amsthm style: "**Theorem 3**
//! (Title)**.**"), their body in italics for the `plain` theorem style;
//! citations are code spans with one `cite:key` link per key; TikZ-like
//! pictures are `latex` code blocks; an unknown environment shows its name
//! as a code span before its body.

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use crate::markdown::{
    Align, Block, InlineImage, InlineMath, InlineStyle, ListItem, RichText, Span,
};

/// Nesting limit for groups, environments and macro expansion.
const MAX_DEPTH: usize = 64;
/// Bytes of macro expansion allowed per document (recursion bombs).
const EXPANSION_BUDGET: usize = 4 << 20;

/// A LaTeX file read for display.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LatexDoc {
    pub blocks: Vec<Block>,
    /// Every `\newcommand` / `\renewcommand` / `\providecommand` / `\def` /
    /// `\DeclareMathOperator` / `\let` of the preamble and body, verbatim and
    /// one per line, for the math engine.
    pub macros: String,
    /// The `\title`, as plain text.
    pub title: Option<String>,
}

/// Read a LaTeX document. With `\begin{document}`, the preamble only
/// contributes definitions and the title block; without, the whole text is
/// body (a chapter pulled in by `\input`).
pub fn parse(text: &str) -> LatexDoc {
    let normalized;
    let text = if text.contains('\r') {
        normalized = text.replace("\r\n", "\n");
        normalized.as_str()
    } else {
        text
    };
    let src = strip_comments(text);
    let chapters = uses_chapters(&src);
    let first = run_pass(&src, chapters, HashMap::new());
    if first.unresolved == 0 {
        return first.doc;
    }
    run_pass(&src, chapters, first.labels).doc
}

struct Pass {
    doc: LatexDoc,
    labels: HashMap<String, Label>,
    unresolved: usize,
}

fn run_pass(src: &str, chapters: bool, known: HashMap<String, Label>) -> Pass {
    let mut st = State::new(chapters, known);
    let (preamble, body) = split_document(src);
    if let Some(preamble) = preamble {
        let mut sink = Sink::default();
        Reader::new(src, &mut st).run(preamble, &mut sink, &mut Style::default());
    }
    let mut sink = Sink::default();
    Reader::new(src, &mut st).run(body, &mut sink, &mut Style::default());
    sink.flush();
    let mut blocks = sink.blocks;
    if !st.footnotes.is_empty() {
        blocks.push(Block::Footnotes(std::mem::take(&mut st.footnotes)));
    }
    let title = st
        .title
        .as_deref()
        .map(|t| tex_to_plain(t).replace('\n', " "))
        .filter(|t| !t.trim().is_empty());
    Pass {
        doc: LatexDoc {
            blocks,
            macros: st.macros,
            title,
        },
        labels: st.labels,
        unresolved: st.unresolved,
    }
}

fn split_document(src: &str) -> (Option<Range<usize>>, Range<usize>) {
    const BEGIN: &str = "\\begin{document}";
    const END: &str = "\\end{document}";
    match src.find(BEGIN) {
        Some(at) => {
            let start = at + BEGIN.len();
            let end = src[start..].rfind(END).map_or(src.len(), |e| start + e);
            (Some(0..at), start..end)
        }
        None => (None, 0..src.len()),
    }
}

/// Book-like classes number sections within chapters.
fn uses_chapters(src: &str) -> bool {
    if src.contains("\\chapter") {
        return true;
    }
    let Some(at) = src.find("\\documentclass") else {
        return false;
    };
    let rest = &src[at..];
    let Some(open) = rest.find('{') else {
        return false;
    };
    let class = rest[open + 1..].split('}').next().unwrap_or("").trim();
    matches!(class, "book" | "report" | "memoir" | "scrbook" | "scrreprt")
}

// ---------------------------------------------------------------------------
// Comments

/// Environments whose bodies are read raw (comments, `\end` of others and
/// `%` stay as written).
const VERBATIM_ENVS: &[&str] = &[
    "verbatim",
    "verbatim*",
    "Verbatim",
    "Verbatim*",
    "BVerbatim",
    "LVerbatim",
    "lstlisting",
    "minted",
    "alltt",
    "spverbatim",
    "comment",
    "filecontents",
    "filecontents*",
];

fn is_verbatim(name: &str) -> bool {
    VERBATIM_ENVS.contains(&name)
}

/// Drop `%` comments (with their line end and the next line's indent, as
/// TeX does) and `comment` environments; verbatim text, `\verb` and URLs
/// stay as written.
fn strip_comments(text: &str) -> String {
    let b = text.as_bytes();
    let len = b.len();
    let mut out = String::with_capacity(len);
    let mut copied = 0;
    let mut i = 0;
    while i < len {
        match b[i] {
            b'\\' => {
                let j = i + 1;
                if j < len && b[j].is_ascii_alphabetic() {
                    let mut k = j;
                    while k < len && b[k].is_ascii_alphabetic() {
                        k += 1;
                    }
                    match &text[j..k] {
                        "begin" => {
                            i = k;
                            if let Some((name, after)) = env_name_at(text, k, len)
                                && is_verbatim(name)
                            {
                                let close = format!("\\end{{{name}}}");
                                let stop = text[after..]
                                    .find(&close)
                                    .map_or(len, |e| after + e + close.len());
                                if name == "comment" {
                                    out.push_str(&text[copied..i - "\\begin".len()]);
                                    copied = stop;
                                }
                                i = stop;
                            }
                        }
                        "verb" | "lstinline" => i = verb_extent(text, k, len).map_or(k, |v| v.1),
                        "url" | "href" => {
                            let mut m = k;
                            while m < len && matches!(b[m], b' ' | b'\t') {
                                m += 1;
                            }
                            i = if m < len && b[m] == b'{' {
                                raw_brace_end(b, m, len).map_or(len, |e| e + 1)
                            } else {
                                k
                            };
                        }
                        _ => i = k,
                    }
                } else {
                    i = j + text[j.min(len)..].chars().next().map_or(0, char::len_utf8);
                }
            }
            b'%' => {
                out.push_str(&text[copied..i]);
                let mut k = i;
                while k < len && b[k] != b'\n' {
                    k += 1;
                }
                if k < len {
                    k += 1;
                    while k < len && matches!(b[k], b' ' | b'\t') {
                        k += 1;
                    }
                    // the line end went with the comment; a blank line
                    // after it still ends the paragraph
                    if k < len && b[k] == b'\n' {
                        out.push('\n');
                    }
                }
                copied = k;
                i = k;
            }
            _ => i += 1,
        }
    }
    out.push_str(&text[copied.min(len)..]);
    out
}

/// `{name}` right after `\begin` (spaces allowed), short.
fn env_name_at(text: &str, at: usize, end: usize) -> Option<(&str, usize)> {
    let b = text.as_bytes();
    let mut i = at;
    while i < end && matches!(b[i], b' ' | b'\t') {
        i += 1;
    }
    if i >= end || b[i] != b'{' {
        return None;
    }
    let limit = end.min(i + 80);
    let close = (i + 1..limit).find(|&k| b[k] == b'}')?;
    let name = &text[i + 1..close];
    (!name.contains(['{', '\\', '\n'])).then(|| (name.trim(), close + 1))
}

/// The matching `}` of the `{` at `open`, without escapes (URLs).
fn raw_brace_end(b: &[u8], open: usize, end: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (k, &c) in b.iter().enumerate().take(end).skip(open) {
        match c {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(k);
                }
            }
            _ => {}
        }
    }
    None
}

/// The content and end of `\verb|…|` / `\lstinline[…]{…}` starting right
/// after the command name. Stays on one line.
fn verb_extent(text: &str, at: usize, end: usize) -> Option<(Range<usize>, usize)> {
    let b = text.as_bytes();
    let mut i = at;
    if i < end && b[i] == b'*' {
        i += 1;
    }
    if i < end && b[i] == b'[' {
        let close = (i..end).find(|&k| b[k] == b']' || b[k] == b'\n')?;
        if b[close] != b']' {
            return None;
        }
        i = close + 1;
    }
    if i >= end {
        return None;
    }
    let delim = b[i];
    if delim.is_ascii_alphabetic() || delim.is_ascii_whitespace() || !delim.is_ascii() {
        return None;
    }
    let close_delim = if delim == b'{' { b'}' } else { delim };
    let start = i + 1;
    let close = (start..end).find(|&k| b[k] == close_delim || b[k] == b'\n')?;
    (b[close] == close_delim).then_some((start..close, close + 1))
}

// ---------------------------------------------------------------------------
// Shared helpers (also for `bibtex`)

/// The precomposed letter for `base` under a TeX accent, from its command
/// character (`'`, `` ` ``, `^`, `"`, `~`, `=`, `.`, `u`, `v`, `H`, `c`,
/// `k`, `r`, `d`, `b`, `t`); a combining mark when there is none.
pub(crate) fn compose_accent(accent: char, base: &str) -> String {
    let mut chars = base.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let rest = chars.as_str();
    let letter = match first {
        'ı' => 'i',
        'ȷ' => 'j',
        c => c,
    };
    let table: &str = match accent {
        '\'' => "aáeéiíoóuúyýAÁEÉIÍOÓUÚYÝcćCĆnńNŃsśSŚzźZŹlĺLĹrŕRŔgǵGǴkḱKḰmḿMḾpṕPṔwẃWẂ",
        '`' => "aàeèiìoòuùAÀEÈIÌOÒUÙnǹNǸwẁWẀyỳYỲ",
        '^' => "aâeêiîoôuûAÂEÊIÎOÔUÛcĉCĈgĝGĜhĥHĤjĵJĴsŝSŜwŵWŴyŷYŶ",
        '"' => "aäeëiïoöuüyÿAÄEËIÏOÖUÜYŸ",
        '~' => "aãoõnñAÃOÕNÑiĩIĨuũUŨ",
        '=' => "aāeēiīoōuūAĀEĒIĪOŌUŪ",
        '.' => "cċCĊeėEĖgġGĠzżZŻIİ",
        'u' => "aăAĂgğGĞuŭUŬeĕEĔiĭIĬoŏOŎ",
        'v' => "cčCČsšSŠzžZŽeěEĚrřRŘnňNŇdďDĎtťTŤlľLĽ",
        'H' => "oőOŐuűUŰ",
        'c' => "cçCÇsşSŞtţTŢgģGĢkķKĶlļLĻnņNŅrŗRŖ",
        'k' => "aąAĄeęEĘiįIĮuųUŲ",
        'r' => "aåAÅuůUŮ",
        _ => "",
    };
    let mut it = table.chars();
    while let (Some(b), Some(composed)) = (it.next(), it.next()) {
        if b == letter {
            return format!("{composed}{rest}");
        }
    }
    let mark = match accent {
        '\'' => '\u{301}',
        '`' => '\u{300}',
        '^' => '\u{302}',
        '"' => '\u{308}',
        '~' => '\u{303}',
        '=' => '\u{304}',
        '.' => '\u{307}',
        'u' => '\u{306}',
        'v' => '\u{30C}',
        'H' => '\u{30B}',
        'c' => '\u{327}',
        'k' => '\u{328}',
        'r' => '\u{30A}',
        'd' => '\u{323}',
        'b' => '\u{331}',
        't' => '\u{361}',
        _ => return format!("{first}{rest}"),
    };
    format!("{first}{mark}{rest}")
}

fn is_accent_word(name: &str) -> Option<char> {
    match name {
        "c" | "v" | "u" | "H" | "k" | "r" | "d" | "b" | "t" => name.chars().next(),
        _ => None,
    }
}

fn is_accent_symbol(name: &str) -> Option<char> {
    match name {
        "'" | "`" | "^" | "\"" | "~" | "=" | "." => name.chars().next(),
        _ => None,
    }
}

/// Text-mode commands that stand for a character or a logo.
fn text_symbol(name: &str) -> Option<&'static str> {
    Some(match name {
        "ss" => "ß",
        "SS" => "SS",
        "o" => "ø",
        "O" => "Ø",
        "ae" => "æ",
        "AE" => "Æ",
        "oe" => "œ",
        "OE" => "Œ",
        "l" => "ł",
        "L" => "Ł",
        "aa" => "å",
        "AA" => "Å",
        "i" => "ı",
        "j" => "ȷ",
        "dh" => "ð",
        "DH" => "Ð",
        "th" => "þ",
        "TH" => "Þ",
        "ng" => "ŋ",
        "NG" => "Ŋ",
        "dj" => "đ",
        "DJ" => "Đ",
        "ldots" | "dots" | "textellipsis" => "…",
        "LaTeX" => "LaTeX",
        "TeX" => "TeX",
        "LaTeXe" => "LaTeX2ε",
        "XeTeX" => "XeTeX",
        "XeLaTeX" => "XeLaTeX",
        "LuaTeX" => "LuaTeX",
        "LuaLaTeX" => "LuaLaTeX",
        "BibTeX" => "BibTeX",
        "pdfTeX" => "pdfTeX",
        "pdfLaTeX" => "pdfLaTeX",
        "AmS" => "AMS",
        "copyright" | "textcopyright" => "©",
        "textregistered" => "®",
        "texttrademark" => "™",
        "S" | "textsection" => "§",
        "P" | "textparagraph" => "¶",
        "dag" | "textdagger" => "†",
        "ddag" | "textdaggerdbl" => "‡",
        "pounds" | "textsterling" => "£",
        "euro" | "texteuro" => "€",
        "textyen" => "¥",
        "textcent" => "¢",
        "textdegree" => "°",
        "textbackslash" => "\\",
        "textasciitilde" => "~",
        "textasciicircum" => "^",
        "textbar" => "|",
        "textless" => "<",
        "textgreater" => ">",
        "textendash" => "–",
        "textemdash" => "—",
        "textbullet" => "•",
        "textperiodcentered" => "·",
        "guillemotleft" | "guillemetleft" => "«",
        "guillemotright" | "guillemetright" => "»",
        "textquoteleft" => "‘",
        "textquoteright" => "’",
        "textquotedblleft" => "“",
        "textquotedblright" => "”",
        "textquotedbl" => "\"",
        "textexclamdown" => "¡",
        "textquestiondown" => "¿",
        "checkmark" => "✓",
        "slash" => "/",
        "textunderscore" => "_",
        "textbraceleft" => "{",
        "textbraceright" => "}",
        "textdollar" => "$",
        "textmu" => "µ",
        "textonehalf" => "½",
        "textpm" => "±",
        "texttimes" => "×",
        "textdiv" => "÷",
        "textminus" => "−",
        "textasteriskcentered" => "*",
        "textvisiblespace" => "␣",
        "qed" => "∎",
        _ => return None,
    })
}

/// LaTeX → plain text for labels, alt texts and BibTeX fields: braces go,
/// accents compose, escapes and ligatures resolve, commands keep their
/// arguments' text, white space collapses. Math keeps its TeX.
pub(crate) fn tex_to_plain(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    plain_into(s, &mut out, 0);
    let collapsed: Vec<&str> = out.split_whitespace().collect();
    collapsed.join(" ")
}

fn plain_into(s: &str, out: &mut String, depth: usize) {
    let b = s.as_bytes();
    let len = b.len();
    let mut i = 0;
    while i < len {
        match b[i] {
            b'\\' => {
                let j = i + 1;
                if j >= len {
                    break;
                }
                if b[j].is_ascii_alphabetic() {
                    let mut k = j;
                    while k < len && b[k].is_ascii_alphabetic() {
                        k += 1;
                    }
                    let name = &s[j..k];
                    while k < len && matches!(b[k], b' ' | b'\t') {
                        k += 1;
                    }
                    if let Some(sym) = text_symbol(name) {
                        out.push_str(sym);
                        i = k;
                    } else if let Some(accent) = is_accent_word(name) {
                        let (arg, after) = plain_arg(s, k, depth);
                        out.push_str(&compose_accent(accent, &arg));
                        i = after;
                    } else {
                        if matches!(name, "quad" | "qquad" | "enspace" | "space" | "newline") {
                            out.push(' ');
                        }
                        i = k;
                    }
                } else {
                    let ch = s[j..].chars().next().unwrap_or(' ');
                    let k = j + ch.len_utf8();
                    let name = &s[j..k];
                    if let Some(accent) = is_accent_symbol(name) {
                        let (arg, after) = plain_arg(s, k, depth);
                        out.push_str(&compose_accent(accent, &arg));
                        i = after;
                    } else {
                        match ch {
                            '&' | '%' | '$' | '#' | '_' | '{' | '}' => out.push(ch),
                            '\\' | ',' | ';' | ':' | ' ' | '\n' => out.push(' '),
                            _ => {}
                        }
                        i = k;
                    }
                }
            }
            b'{' | b'}' => i += 1,
            b'~' => {
                out.push(' ');
                i += 1;
            }
            b'-' => {
                let n = b[i..len.min(i + 3)]
                    .iter()
                    .take_while(|&&c| c == b'-')
                    .count();
                out.push_str(match n {
                    1 => "-",
                    2 => "–",
                    _ => "—",
                });
                i += n;
            }
            b'`' => {
                if i + 1 < len && b[i + 1] == b'`' {
                    out.push('“');
                    i += 2;
                } else {
                    out.push('‘');
                    i += 1;
                }
            }
            b'\'' if i + 1 < len && b[i + 1] == b'\'' => {
                out.push('”');
                i += 2;
            }
            b'$' => {
                // math keeps its TeX
                let start = i + 1;
                let close = s[start..].find('$').map_or(len, |e| start + e);
                out.push_str(s[start..close].trim_matches('$'));
                i = (close + 1).min(len);
                while i < len && b[i] == b'$' {
                    i += 1;
                }
            }
            _ => {
                let ch = s[i..].chars().next().unwrap_or(' ');
                out.push(ch);
                i += ch.len_utf8();
            }
        }
    }
}

/// An accent's argument as plain text: `{…}` or one character.
fn plain_arg(s: &str, at: usize, depth: usize) -> (String, usize) {
    let b = s.as_bytes();
    let mut i = at;
    while i < b.len() && matches!(b[i], b' ' | b'\t') {
        i += 1;
    }
    if i >= b.len() {
        return (String::new(), i);
    }
    if b[i] == b'{' {
        let close = brace_end(b, i, b.len());
        let mut inner = String::new();
        if depth < MAX_DEPTH {
            plain_into(&s[i + 1..close], &mut inner, depth + 1);
        }
        return (inner, (close + 1).min(b.len()));
    }
    if b[i] == b'\\' {
        let mut k = i + 1;
        while k < b.len() && b[k].is_ascii_alphabetic() {
            k += 1;
        }
        if k > i + 1 {
            let name = &s[i + 1..k];
            return (text_symbol(name).unwrap_or("").to_string(), k);
        }
    }
    let ch = s[i..].chars().next().unwrap_or(' ');
    (ch.to_string(), i + ch.len_utf8())
}

/// The matching `}` of the `{` at `open` (escapes skipped), or `end`.
fn brace_end(b: &[u8], open: usize, end: usize) -> usize {
    let mut depth = 0usize;
    let mut i = open;
    while i < end {
        match b[i] {
            b'\\' => i += 1,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
        i += 1;
    }
    end
}

/// The `{…}` group at `i` of a string (column specs).
fn str_group(s: &str, i: usize) -> Option<(&str, usize)> {
    let b = s.as_bytes();
    let mut i = i;
    while i < b.len() && b[i] == b' ' {
        i += 1;
    }
    if i >= b.len() || b[i] != b'{' {
        return None;
    }
    let close = brace_end(b, i, b.len());
    Some((&s[i + 1..close], (close + 1).min(b.len())))
}

fn alph(n: u32, upper: bool) -> String {
    if (1..=26).contains(&n) {
        let base = if upper { b'A' } else { b'a' };
        char::from(base + (n - 1) as u8).to_string()
    } else {
        n.to_string()
    }
}

fn roman(n: u32, upper: bool) -> String {
    if n == 0 || n >= 4000 {
        return n.to_string();
    }
    const TABLE: &[(u32, &str)] = &[
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut n = n;
    let mut out = String::new();
    for &(value, digits) in TABLE {
        while n >= value {
            out.push_str(digits);
            n -= value;
        }
    }
    if upper { out.to_uppercase() } else { out }
}

/// `width=5cm` → CSS pixels; relative widths (`0.5\textwidth`) are `None`.
fn dim_px(value: &str) -> Option<u32> {
    let v = value.trim();
    let num_end = v
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(v.len());
    let n: f64 = v[..num_end].parse().ok()?;
    let factor = match v[num_end..].trim() {
        "px" => 1.0,
        "pt" | "bp" => 4.0 / 3.0,
        "pc" => 16.0,
        "cm" => 37.8,
        "mm" => 3.78,
        "in" => 96.0,
        _ => return None,
    };
    let px = (n * factor).round();
    (1.0..100_000.0).contains(&px).then_some(px as u32)
}

fn key_value<'s>(opts: &'s str, key: &str) -> Option<&'s str> {
    opts.split(',').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k.trim() == key).then(|| v.trim().trim_matches(|c| c == '{' || c == '}'))
    })
}

// ---------------------------------------------------------------------------
// Rich text building

#[derive(Clone, Debug, Default)]
struct Style {
    inline: InlineStyle,
    caps: bool,
    link: Option<String>,
}

impl Style {
    fn with(&self, f: impl FnOnce(&mut Style)) -> Style {
        let mut s = self.clone();
        f(&mut s);
        s
    }
}

pub(crate) fn push_rt(rt: &mut RichText, text: &str, style: InlineStyle, link: Option<&str>) {
    if text.is_empty() {
        return;
    }
    let start = rt.text.len();
    rt.text.push_str(text);
    let end = rt.text.len();
    if style == InlineStyle::default() && link.is_none() {
        return;
    }
    if let Some(last) = rt.spans.last_mut()
        && last.range.end == start
        && last.style == style
        && last.link.as_deref() == link
    {
        last.range.end = end;
        return;
    }
    rt.spans.push(Span {
        range: start..end,
        style,
        link: link.map(str::to_string),
    });
}

fn append_rich(dst: &mut RichText, src: RichText) {
    let offset = dst.text.len();
    dst.text.push_str(&src.text);
    dst.spans.extend(src.spans.into_iter().map(|mut s| {
        s.range = s.range.start + offset..s.range.end + offset;
        s
    }));
    dst.images.extend(src.images.into_iter().map(|mut i| {
        i.at += offset;
        i
    }));
    dst.maths.extend(src.maths.into_iter().map(|mut m| {
        m.at += offset;
        m
    }));
}

fn ends_in_space(rt: &RichText) -> bool {
    let len = rt.text.len();
    let object_last = rt.images.last().is_some_and(|i| i.at == len)
        || rt.maths.last().is_some_and(|m| m.at == len);
    !object_last && rt.text.ends_with([' ', '\n'])
}

fn trim_end(rt: &mut RichText) {
    let keep = rt
        .images
        .last()
        .map_or(0, |i| i.at)
        .max(rt.maths.last().map_or(0, |m| m.at));
    let trimmed = rt.text.trim_end_matches([' ', '\n']).len().max(keep);
    rt.text.truncate(trimmed);
    rt.spans.retain(|s| s.range.start < trimmed);
    if let Some(last) = rt.spans.last_mut() {
        last.range.end = last.range.end.min(trimmed);
    }
}

fn has_content(rt: &RichText) -> bool {
    !rt.images.is_empty() || !rt.maths.is_empty() || !rt.text.trim().is_empty()
}

/// Where a reader puts what it reads: finished blocks and the paragraph
/// being collected.
#[derive(Default)]
struct Sink {
    blocks: Vec<Block>,
    para: RichText,
    /// White space seen; becomes one space before the next text.
    space: bool,
    /// `\centering` / `\raggedleft` inside the environment.
    align: Option<Align>,
}

impl Sink {
    fn emit_space(&mut self) {
        if std::mem::take(&mut self.space) && !self.para.is_empty() && !ends_in_space(&self.para) {
            push_rt(&mut self.para, " ", InlineStyle::default(), None);
        }
    }

    fn text(&mut self, s: &str, style: &Style) {
        if s.is_empty() {
            return;
        }
        self.emit_space();
        if style.caps {
            push_rt(
                &mut self.para,
                &s.to_uppercase(),
                style.inline,
                style.link.as_deref(),
            );
        } else {
            push_rt(&mut self.para, s, style.inline, style.link.as_deref());
        }
    }

    fn brk(&mut self) {
        self.space = false;
        if !self.para.is_empty() {
            push_rt(&mut self.para, "\n", InlineStyle::default(), None);
        }
    }

    fn hard_space(&mut self) {
        self.space = false;
        if !self.para.is_empty() {
            push_rt(&mut self.para, " ", InlineStyle::default(), None);
        }
    }

    fn math(&mut self, tex: &str) {
        self.emit_space();
        let at = self.para.text.len();
        self.para.maths.push(InlineMath {
            at,
            tex: tex.trim().to_string(),
            display: false,
        });
    }

    fn image(&mut self, mut image: InlineImage) {
        self.emit_space();
        image.at = self.para.text.len();
        self.para.images.push(image);
    }

    fn flush(&mut self) {
        self.space = false;
        let mut para = std::mem::take(&mut self.para);
        trim_end(&mut para);
        if has_content(&para) {
            self.blocks.push(Block::Paragraph(para));
        }
    }

    fn block(&mut self, block: Block) {
        self.flush();
        self.blocks.push(block);
    }

    /// Everything collected as one line of text (cells, titles, labels).
    fn into_rich(mut self) -> RichText {
        self.flush();
        let mut out = RichText::default();
        for block in self.blocks {
            let rt = match block {
                Block::Paragraph(rt) | Block::Heading { text: rt, .. } => rt,
                Block::Math { tex, .. } => RichText {
                    maths: vec![InlineMath {
                        at: 0,
                        tex,
                        display: true,
                    }],
                    ..RichText::default()
                },
                other => RichText {
                    text: crate::markdown::plain_text(&[other]),
                    ..RichText::default()
                },
            };
            if !out.is_empty() && !rt.is_empty() {
                push_rt(&mut out, " ", InlineStyle::default(), None);
            }
            append_rich(&mut out, rt);
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Document state

#[derive(Clone, Debug, PartialEq, Eq)]
struct Label {
    /// `section`, `figure`, `equation`, `item`, … or a theorem's title.
    kind: String,
    number: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fmt {
    Arabic,
    Alph,
    Roman,
}

#[derive(Clone, Debug)]
struct Counter {
    value: u32,
    parent: Option<String>,
    fmt: Fmt,
}

#[derive(Clone, Debug)]
struct Theorem {
    title: String,
    /// `None`: unnumbered (`\newtheorem*`, `proof`).
    counter: Option<String>,
    italic: bool,
}

struct Macro {
    nargs: usize,
    default: Option<String>,
    body: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Float {
    Figure,
    Table,
    Sub,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListKind {
    Itemize,
    Enumerate,
    Description,
    Bibliography,
}

struct State {
    /// Labels from the first pass, for forward references.
    known: HashMap<String, Label>,
    labels: HashMap<String, Label>,
    unresolved: usize,
    /// What a `\label` here refers to.
    target: Option<Label>,
    macros: String,
    user: HashMap<String, Rc<Macro>>,
    aliases: HashMap<String, String>,
    budget: usize,
    title: Option<String>,
    author: Option<String>,
    date: Option<String>,
    theorems: HashMap<String, Theorem>,
    theorem_italic: bool,
    counters: HashMap<String, Counter>,
    chapters: bool,
    secnumdepth: i32,
    appendix: bool,
    float: Option<Float>,
    figure_alt: Option<String>,
    subfigure: u32,
    figure_captioned: bool,
    enum_depth: usize,
    footnotes: Vec<Vec<Block>>,
    depth: usize,
}

impl State {
    fn new(chapters: bool, known: HashMap<String, Label>) -> Self {
        let mut counters = HashMap::new();
        let mut add = |name: &str, parent: Option<&str>, fmt: Fmt| {
            counters.insert(
                name.to_string(),
                Counter {
                    value: 0,
                    parent: parent.map(str::to_string),
                    fmt,
                },
            );
        };
        let top = chapters.then_some("chapter");
        add("part", None, Fmt::Roman);
        add("chapter", None, Fmt::Arabic);
        add("section", top, Fmt::Arabic);
        add("subsection", Some("section"), Fmt::Arabic);
        add("subsubsection", Some("subsection"), Fmt::Arabic);
        add("paragraph", Some("subsubsection"), Fmt::Arabic);
        add("subparagraph", Some("paragraph"), Fmt::Arabic);
        add("equation", top, Fmt::Arabic);
        add("figure", top, Fmt::Arabic);
        add("table", top, Fmt::Arabic);
        let mut theorems = HashMap::new();
        for (name, title, italic) in [
            ("theorem", "Theorem", true),
            ("lemma", "Lemma", true),
            ("proposition", "Proposition", true),
            ("corollary", "Corollary", true),
            ("conjecture", "Conjecture", true),
            ("claim", "Claim", true),
            ("definition", "Definition", false),
            ("example", "Example", false),
            ("exercise", "Exercise", false),
            ("remark", "Remark", false),
            ("note", "Note", false),
        ] {
            counters.insert(
                name.to_string(),
                Counter {
                    value: 0,
                    parent: None,
                    fmt: Fmt::Arabic,
                },
            );
            theorems.insert(
                name.to_string(),
                Theorem {
                    title: title.to_string(),
                    counter: Some(name.to_string()),
                    italic,
                },
            );
        }
        Self {
            known,
            labels: HashMap::new(),
            unresolved: 0,
            target: None,
            macros: String::new(),
            user: HashMap::new(),
            aliases: HashMap::new(),
            budget: EXPANSION_BUDGET,
            title: None,
            author: None,
            date: None,
            theorems,
            theorem_italic: true,
            counters,
            chapters,
            secnumdepth: if chapters { 2 } else { 3 },
            appendix: false,
            float: None,
            figure_alt: None,
            subfigure: 0,
            figure_captioned: false,
            enum_depth: 0,
            footnotes: Vec::new(),
            depth: 0,
        }
    }

    fn step(&mut self, name: &str) {
        let counter = self
            .counters
            .entry(name.to_string())
            .or_insert_with(|| Counter {
                value: 0,
                parent: None,
                fmt: Fmt::Arabic,
            });
        counter.value = counter.value.saturating_add(1);
        self.reset_children(name, 0);
    }

    fn reset_children(&mut self, name: &str, depth: usize) {
        if depth > 8 {
            return;
        }
        let children: Vec<String> = self
            .counters
            .iter()
            .filter(|(child, c)| c.parent.as_deref() == Some(name) && child.as_str() != name)
            .map(|(child, _)| child.clone())
            .collect();
        for child in children {
            if let Some(c) = self.counters.get_mut(&child) {
                c.value = 0;
            }
            self.reset_children(&child, depth + 1);
        }
    }

    fn format(&self, name: &str) -> String {
        self.format_depth(name, 0)
    }

    fn format_depth(&self, name: &str, depth: usize) -> String {
        let Some(c) = self.counters.get(name) else {
            return "0".to_string();
        };
        let own = match c.fmt {
            Fmt::Arabic => c.value.to_string(),
            Fmt::Alph => alph(c.value, true),
            Fmt::Roman => roman(c.value, true),
        };
        match &c.parent {
            Some(parent) if depth < 8 && parent != name && self.counters.contains_key(parent) => {
                format!("{}.{own}", self.format_depth(parent, depth + 1))
            }
            _ => own,
        }
    }

    fn set_target(&mut self, kind: &str, number: String) {
        self.target = Some(Label {
            kind: kind.to_string(),
            number,
        });
    }

    fn record_label(&mut self, key: &str) {
        if let Some(target) = self.target.clone() {
            self.labels.insert(key.trim().to_string(), target);
        }
    }

    fn lookup(&mut self, key: &str) -> Option<Label> {
        let key = key.trim();
        let found = self
            .labels
            .get(key)
            .or_else(|| self.known.get(key))
            .cloned();
        if found.is_none() {
            self.unresolved += 1;
        }
        found
    }
}

// ---------------------------------------------------------------------------
// The reader

struct Reader<'a, 's> {
    s: &'a str,
    b: &'a [u8],
    st: &'s mut State,
    misses: Misses,
}

/// Failed searches, so that unclosed openers do not rescan the rest of
/// the source (quadratic time on broken input).
#[derive(Default)]
struct Misses {
    /// Environment name → (from, end): no `\end{name}` in between.
    env: HashMap<String, (usize, usize)>,
    /// `\)`, `\]`, `$$` → (from, stop, the largest range end it holds for).
    close: [Option<(usize, usize, usize)>; 3],
}

impl Misses {
    fn close_missed(&self, kind: usize, from: usize, end: usize) -> bool {
        self.close[kind].is_some_and(|(f, stop, limit)| from >= f && from < stop && end <= limit)
    }
}

const SECTION_COMMANDS: &[&str] = &[
    "part",
    "chapter",
    "section",
    "subsection",
    "subsubsection",
    "paragraph",
    "subparagraph",
];

/// Commands that print nothing and take no arguments.
const NO_OPS: &[&str] = &[
    "noindent",
    "indent",
    "raggedright",
    "RaggedRight",
    "small",
    "footnotesize",
    "scriptsize",
    "tiny",
    "normalsize",
    "large",
    "Large",
    "LARGE",
    "huge",
    "Huge",
    "selectfont",
    "newpage",
    "clearpage",
    "cleardoublepage",
    "smallskip",
    "medskip",
    "bigskip",
    "vfill",
    "vfil",
    "hss",
    "strut",
    "protect",
    "relax",
    "null",
    "leavevmode",
    "nobreak",
    "allowbreak",
    "sloppy",
    "fussy",
    "frenchspacing",
    "nonfrenchspacing",
    "frontmatter",
    "mainmatter",
    "backmatter",
    "makeatletter",
    "makeatother",
    "tableofcontents",
    "listoffigures",
    "listoftables",
    "printindex",
    "makeindex",
    "qedhere",
    "nonumber",
    "notag",
    "onecolumn",
    "flushbottom",
    "raggedbottom",
    "FloatBarrier",
    "justifying",
    "hline",
    "toprule",
    "midrule",
    "bottomrule",
    "hdashline",
    "endhead",
    "endfirsthead",
    "endfoot",
    "endlastfoot",
    "morecmidrules",
    "today",
    "and",
    "centering",
    "raggedleft",
    "displaystyle",
    "item",
];

/// Commands whose arguments are dropped, with their argument spec
/// (`s` star, `o` optional, `m` mandatory).
fn ignored_args(name: &str) -> Option<&'static str> {
    Some(match name {
        "usepackage" | "RequirePackage" | "documentclass" => "omo",
        "pagestyle" | "thispagestyle" | "pagenumbering" | "bibliographystyle" | "newlength"
        | "geometry" | "hypersetup" | "graphicspath" | "index" | "nocite" | "linespread"
        | "phantom" | "hphantom" | "vphantom" | "selectlanguage" | "lstset" | "usetikzlibrary"
        | "tikzset" | "pgfplotsset" | "sisetup" | "markright" | "addvspace" | "needspace"
        | "arrayrulecolor" | "thanks" | "institute" | "affiliation" | "email" | "keywords"
        | "color" | "pagecolor" | "includeonly" | "hyphenation" | "glsaddall" => "om",
        "addbibresource" | "captionsetup" | "setminted" | "rowcolor" | "cellcolor"
        | "columncolor" | "affil" => "om",
        "setlength" | "addtolength" | "settowidth" | "settoheight" | "fontsize" | "markboth"
        | "counterwithout" => "mm",
        "definecolor" | "addcontentsline" | "specialrule" => "mmm",
        "newcolumntype" => "mom",
        "vspace" | "hspace" | "enlargethispage" => "sm",
        "cline" | "label" => "m",
        "pagebreak" | "nopagebreak" | "linebreak" | "allowdisplaybreaks" | "twocolumn"
        | "addlinespace" => "o",
        "bibitem" => "om",
        "newenvironment" | "renewenvironment" => "smoomm",
        _ => return None,
    })
}

const KNOWN_WRAPPERS: &[&str] = &[
    "document",
    "minipage",
    "adjustbox",
    "small",
    "footnotesize",
    "scriptsize",
    "tiny",
    "normalsize",
    "large",
    "Large",
    "LARGE",
    "huge",
    "Huge",
    "subequations",
    "singlespace",
    "onehalfspace",
    "doublespace",
    "spacing",
    "landscape",
    "samepage",
    "multicols",
    "multicols*",
    "threeparttable",
    "refsection",
    "sloppypar",
    "titlepage",
    "otherlanguage",
    "otherlanguage*",
    "english",
    "hyphenrules",
    "raggedright",
    "RaggedRight",
    "algorithm",
    "algorithm*",
    "algorithmic",
    "algorithm2e",
    "frame",
    "block",
    "columns",
    "column",
    "appendices",
    "measuredfigure",
    "savenotes",
    "fullwidth",
    "group",
];

/// Picture languages: shown as their source.
const PICTURES: &[&str] = &[
    "tikzpicture",
    "tikzcd",
    "pgfpicture",
    "circuitikz",
    "forest",
    "pspicture",
    "picture",
    "axis",
    "dot2tex",
];

impl<'a, 's> Reader<'a, 's> {
    fn new(src: &'a str, st: &'s mut State) -> Self {
        Self {
            s: src,
            b: src.as_bytes(),
            st,
            misses: Misses::default(),
        }
    }

    fn slice(&self, range: Range<usize>) -> &'a str {
        self.s.get(range).unwrap_or("")
    }

    /// Read another source (a macro body, the stored title) with this
    /// document's state.
    fn sub<R>(&mut self, src: &str, f: impl FnOnce(&mut Reader<'_, '_>) -> R) -> R {
        let mut reader = Reader {
            s: src,
            b: src.as_bytes(),
            st: &mut *self.st,
            misses: Misses::default(),
        };
        f(&mut reader)
    }

    // -- scanning ----------------------------------------------------------

    fn char_len(&self, i: usize) -> usize {
        self.s
            .get(i..)
            .and_then(|r| r.chars().next())
            .map_or(1, char::len_utf8)
    }

    /// The command at the `\` at `at`: its name, the end of the name, and
    /// whether it is a control word (letters).
    fn read_name(&self, at: usize, end: usize) -> (&'a str, usize, bool) {
        let i = at + 1;
        if i >= end {
            return ("", end, false);
        }
        if self.b[i].is_ascii_alphabetic() {
            let mut j = i;
            while j < end && self.b[j].is_ascii_alphabetic() {
                j += 1;
            }
            (self.slice(i..j), j, true)
        } else {
            let j = (i + self.char_len(i)).min(end);
            (self.slice(i..j), j, false)
        }
    }

    fn blank_line_at(&self, nl: usize, end: usize) -> bool {
        let mut j = nl + 1;
        while j < end && matches!(self.b[j], b' ' | b'\t' | b'\r') {
            j += 1;
        }
        j < end && self.b[j] == b'\n'
    }

    /// Spaces after a control word, and its line end unless a blank line
    /// follows.
    fn skip_ws(&self, i: usize, end: usize) -> usize {
        let mut i = i;
        while i < end && matches!(self.b[i], b' ' | b'\t' | b'\r') {
            i += 1;
        }
        if i < end && self.b[i] == b'\n' && !self.blank_line_at(i, end) {
            i += 1;
            while i < end && matches!(self.b[i], b' ' | b'\t' | b'\r') {
                i += 1;
            }
        }
        i
    }

    fn match_brace(&self, open: usize, end: usize) -> usize {
        brace_end(self.b, open, end)
    }

    /// `{…}` after optional white space.
    fn group_arg(&self, i: usize, end: usize) -> Option<(Range<usize>, usize)> {
        let j = self.skip_ws(i, end);
        if j >= end || self.b[j] != b'{' {
            return None;
        }
        let close = self.match_brace(j, end);
        Some((j + 1..close, (close + 1).min(end)))
    }

    /// `{…}` or a single token (`\name` or one character).
    fn req_arg(&self, i: usize, end: usize) -> Option<(Range<usize>, usize)> {
        let j = self.skip_ws(i, end);
        if j >= end {
            return None;
        }
        match self.b[j] {
            b'{' => self.group_arg(j, end),
            b'}' => None,
            b'\\' => {
                let (_, after, _) = self.read_name(j, end);
                Some((j..after, after))
            }
            _ => {
                let after = (j + self.char_len(j)).min(end);
                Some((j..after, after))
            }
        }
    }

    /// `[…]` after optional white space, within the paragraph.
    fn opt_arg(&self, i: usize, end: usize) -> Option<(Range<usize>, usize)> {
        let j = self.skip_ws(i, end);
        if j >= end || self.b[j] != b'[' {
            return None;
        }
        self.bracket_from(j, end)
    }

    fn bracket_from(&self, open: usize, end: usize) -> Option<(Range<usize>, usize)> {
        let mut depth = 0usize;
        let mut k = open + 1;
        while k < end {
            match self.b[k] {
                b'\\' => k += 1,
                b'{' => depth += 1,
                b'}' => depth = depth.saturating_sub(1),
                b']' if depth == 0 => return Some((open + 1..k, k + 1)),
                b'\n' if self.blank_line_at(k, end) => return None,
                _ => {}
            }
            k += 1;
        }
        None
    }

    /// Arguments by spec: `s` star, `o` optional, `m` mandatory, `g` group.
    fn args(&self, i: usize, end: usize, spec: &str) -> (Vec<Option<Range<usize>>>, usize) {
        let mut i = i;
        let mut out = Vec::with_capacity(spec.len());
        for c in spec.chars() {
            let got = match c {
                's' => {
                    let j = self.skip_ws(i, end);
                    (j < end && self.b[j] == b'*').then(|| (j..j + 1, j + 1))
                }
                'o' => self.opt_arg(i, end),
                'm' => self.req_arg(i, end),
                _ => self.group_arg(i, end),
            };
            match got {
                Some((r, after)) => {
                    out.push(Some(r));
                    i = after;
                }
                None => out.push(None),
            }
        }
        (out, i)
    }

    fn arg_text(&self, arg: &Option<Range<usize>>) -> &'a str {
        arg.as_ref().map_or("", |r| self.slice(r.clone()))
    }

    /// `{name}` of `\begin` / `\end`.
    fn env_name(&self, i: usize, end: usize) -> Option<(&'a str, usize)> {
        env_name_at(self.s, self.skip_ws(i, end), end)
    }

    /// The raw end of a verbatim environment: (body end, after `\end{…}`).
    fn raw_end(&self, name: &str, from: usize, end: usize) -> (usize, usize) {
        let close = format!("\\end{{{name}}}");
        match self.slice(from..end).find(&close) {
            Some(e) => (from + e, from + e + close.len()),
            None => (end, end),
        }
    }

    /// The `\end{name}` matching an environment whose body starts at
    /// `from`: (body end, after it). Missing: the rest of the range.
    fn find_end(&mut self, name: &str, from: usize, end: usize) -> (usize, usize) {
        if self
            .misses
            .env
            .get(name)
            .is_some_and(|&(f, e)| from >= f && end <= e)
        {
            return (end, end);
        }
        let mut depth = 1usize;
        let mut saw_end = false;
        let mut i = from;
        while i < end {
            let Some(off) = self.b[i..end].iter().position(|&c| c == b'\\') else {
                break;
            };
            i += off;
            let (w, j, word) = self.read_name(i, end);
            if word && (w == "begin" || w == "end") {
                if let Some((n, after)) = self.env_name(j, end) {
                    if w == "begin" {
                        if is_verbatim(n) {
                            i = self.raw_end(n, after, end).1;
                            continue;
                        }
                        if n == name {
                            depth += 1;
                        }
                    } else if n == name {
                        saw_end = true;
                        depth -= 1;
                        if depth == 0 {
                            return (i, after);
                        }
                    }
                    i = after;
                    continue;
                }
            } else if word && (w == "verb" || w == "lstinline") {
                i = verb_extent(self.s, j, end).map_or(j, |v| v.1);
                continue;
            }
            i = j.max(i + 1);
        }
        if !saw_end {
            self.misses.env.insert(name.to_string(), (from, end));
        }
        (end, end)
    }

    /// Top-level (outside groups and nested environments) positions of the
    /// commands in `cmds`, of `&` (`amp`) and of row ends `\\` (`rows`):
    /// (start, end) of each hit, a row end including its `*` and `[…]`.
    fn top_hits(
        &self,
        range: Range<usize>,
        cmds: &[&str],
        amp: bool,
        rows: bool,
    ) -> Vec<(usize, usize)> {
        let end = range.end.min(self.b.len());
        let mut hits = Vec::new();
        let mut brace = 0usize;
        let mut env = 0usize;
        let mut i = range.start;
        while i < end {
            match self.b[i] {
                b'{' => {
                    brace += 1;
                    i += 1;
                }
                b'}' => {
                    brace = brace.saturating_sub(1);
                    i += 1;
                }
                b'&' => {
                    if amp && brace == 0 && env == 0 {
                        hits.push((i, i + 1));
                    }
                    i += 1;
                }
                b'\\' => {
                    let (w, j, word) = self.read_name(i, end);
                    let top = brace == 0 && env == 0;
                    if word {
                        match w {
                            "begin" => {
                                if let Some((n, after)) = self.env_name(j, end) {
                                    if is_verbatim(n) {
                                        i = self.raw_end(n, after, end).1;
                                    } else {
                                        env += 1;
                                        i = after;
                                    }
                                    continue;
                                }
                            }
                            "end" => {
                                env = env.saturating_sub(1);
                                if let Some((_, after)) = self.env_name(j, end) {
                                    i = after;
                                    continue;
                                }
                            }
                            "verb" | "lstinline" => {
                                i = verb_extent(self.s, j, end).map_or(j, |v| v.1);
                                continue;
                            }
                            _ => {
                                if top && (cmds.contains(&w) || (rows && w == "tabularnewline")) {
                                    hits.push((i, j));
                                }
                            }
                        }
                    } else if rows && top && w == "\\" {
                        let mut k = j;
                        if k < end && self.b[k] == b'*' {
                            k += 1;
                        }
                        if k < end
                            && self.b[k] == b'['
                            && let Some((_, after)) = self.bracket_from(k, end)
                        {
                            k = after;
                        }
                        hits.push((i, k));
                        i = k;
                        continue;
                    }
                    i = j.max(i + 1);
                }
                _ => i += 1,
            }
        }
        hits
    }

    // -- reading -----------------------------------------------------------

    fn run(&mut self, range: Range<usize>, sink: &mut Sink, style: &mut Style) {
        if self.st.depth >= MAX_DEPTH {
            let text = tex_to_plain(self.slice(range));
            sink.text(&text, style);
            return;
        }
        self.st.depth += 1;
        self.run_inner(range, sink, style);
        self.st.depth -= 1;
    }

    fn run_inner(&mut self, range: Range<usize>, sink: &mut Sink, style: &mut Style) {
        let end = range.end.min(self.b.len());
        let mut i = range.start;
        while i < end {
            let c = self.b[i];
            let literal = style.inline.code;
            match c {
                b'\\' => i = self.command(i, end, sink, style).max(i + 1),
                b'{' => {
                    let close = self.match_brace(i, end);
                    let mut inner = style.clone();
                    self.run(i + 1..close, sink, &mut inner);
                    i = (close + 1).min(end).max(i + 1);
                }
                b'}' | b'#' | b'&' => i += 1,
                b'$' => i = self.dollar(i, end, sink).max(i + 1),
                b'%' => {
                    while i < end && self.b[i] != b'\n' {
                        i += 1;
                    }
                }
                b'~' => {
                    sink.text("\u{a0}", style);
                    i += 1;
                }
                b'\n' => {
                    if self.blank_line_at(i, end) {
                        sink.flush();
                        while i < end && self.b[i].is_ascii_whitespace() {
                            i += 1;
                        }
                    } else {
                        sink.space = true;
                        i += 1;
                    }
                }
                b' ' | b'\t' | b'\r' => {
                    sink.space = true;
                    i += 1;
                }
                b'-' if !literal => {
                    let n = self.b[i..end.min(i + 3)]
                        .iter()
                        .take_while(|&&c| c == b'-')
                        .count();
                    sink.text(
                        match n {
                            1 => "-",
                            2 => "–",
                            _ => "—",
                        },
                        style,
                    );
                    i += n;
                }
                b'`' if !literal => {
                    if i + 1 < end && self.b[i + 1] == b'`' {
                        sink.text("“", style);
                        i += 2;
                    } else {
                        sink.text("‘", style);
                        i += 1;
                    }
                }
                b'\'' if !literal => {
                    if i + 1 < end && self.b[i + 1] == b'\'' {
                        sink.text("”", style);
                        i += 2;
                    } else {
                        sink.text("’", style);
                        i += 1;
                    }
                }
                _ => {
                    let start = i;
                    i += self.char_len(i);
                    while i < end
                        && !matches!(
                            self.b[i],
                            b'\\'
                                | b'{'
                                | b'}'
                                | b'#'
                                | b'&'
                                | b'$'
                                | b'%'
                                | b'~'
                                | b'\n'
                                | b' '
                                | b'\t'
                                | b'\r'
                                | b'-'
                                | b'`'
                                | b'\''
                        )
                    {
                        i += 1;
                    }
                    let i_clamped = i.min(end);
                    sink.text(self.slice(start..i_clamped), style);
                    i = i_clamped;
                }
            }
        }
    }

    /// Blocks of an environment body, wrapped when it was `\centering`.
    fn blocks_of(&mut self, range: Range<usize>, style: &Style) -> Vec<Block> {
        let mut sink = Sink::default();
        self.run(range, &mut sink, &mut style.clone());
        sink.flush();
        match sink.align {
            Some(align @ (Align::Center | Align::Right)) if !sink.blocks.is_empty() => {
                vec![Block::Aligned {
                    align,
                    blocks: sink.blocks,
                }]
            }
            _ => sink.blocks,
        }
    }

    fn inline(&mut self, range: Range<usize>, style: &Style) -> RichText {
        let mut sink = Sink::default();
        self.run(range, &mut sink, &mut style.clone());
        sink.into_rich()
    }

    fn inline_str(&mut self, src: &str, style: &Style) -> RichText {
        self.sub(src, |r| r.inline(0..src.len(), style))
    }

    // -- math --------------------------------------------------------------

    fn dollar(&mut self, i: usize, end: usize, sink: &mut Sink) -> usize {
        let b = self.b;
        if i + 1 < end && b[i + 1] == b'$' {
            let from = i + 2;
            let mut k = from;
            let mut miss = (from, end, end);
            if !self.misses.close_missed(2, from, end) {
                while k + 1 < end {
                    match b[k] {
                        b'\\' => k += 2,
                        b'$' if b[k + 1] == b'$' => {
                            let tex = self.slice(from..k);
                            self.display_math(tex, sink);
                            return k + 2;
                        }
                        b'\n' if self.blank_line_at(k, end) => {
                            miss = (from, k, usize::MAX);
                            break;
                        }
                        _ => k += 1,
                    }
                }
                self.misses.close[2] = Some(miss);
            }
            sink.text("$$", &Style::default());
            return from;
        }
        let mut k = i + 1;
        while k < end {
            match b[k] {
                b'\\' => k += 2,
                b'$' => {
                    sink.math(self.slice(i + 1..k));
                    return k + 1;
                }
                b'\n' if self.blank_line_at(k, end) => break,
                _ => k += 1,
            }
        }
        sink.text("$", &Style::default());
        i + 1
    }

    /// `\(…\)` / `\[…\]`: the end of the formula from `from`.
    /// Formulas end within their paragraph.
    fn find_close(&mut self, from: usize, end: usize, close: u8) -> Option<usize> {
        let kind = usize::from(close == b']');
        if self.misses.close_missed(kind, from, end) {
            return None;
        }
        let mut k = from;
        while k + 1 < end {
            match self.b[k] {
                b'\\' if self.b[k + 1] == close => return Some(k),
                b'\\' => k += 2,
                b'\n' if self.blank_line_at(k, end) => {
                    self.misses.close[kind] = Some((from, k, usize::MAX));
                    return None;
                }
                _ => k += 1,
            }
        }
        self.misses.close[kind] = Some((from, end, end));
        None
    }

    fn display_math(&mut self, tex: &str, sink: &mut Sink) {
        let (clean, info) = clean_math_row(tex);
        let number = info.tag.map(|t| format!("({t})"));
        if let Some(n) = &number {
            for key in &info.labels {
                self.st.labels.insert(
                    key.clone(),
                    Label {
                        kind: "equation".into(),
                        number: n.trim_matches(['(', ')']).to_string(),
                    },
                );
            }
        }
        sink.block(Block::Math {
            tex: clean.trim().to_string(),
            number,
        });
    }

    fn math_env(&mut self, name: &str, mut body: Range<usize>, sink: &mut Sink) {
        let star = name.ends_with('*');
        let base = name.trim_end_matches('*');
        let mut cols = String::new();
        if matches!(base, "alignat" | "xalignat" | "xxalignat") {
            let (a, after) = self.args(body.start, body.end, "g");
            cols = self.arg_text(&a[0]).trim().to_string();
            body.start = after;
        }
        let raw = self.slice(body).to_string();
        let per_row = matches!(
            base,
            "align" | "gather" | "flalign" | "alignat" | "xalignat" | "xxalignat" | "eqnarray"
        );
        let mut numbers: Vec<(String, bool)> = Vec::new();
        let mut cleaned = String::new();
        if per_row {
            let mut rows = split_math_rows(&raw);
            while rows.len() > 1
                && rows
                    .last()
                    .is_some_and(|(row, _)| clean_math_row(row).0.trim().is_empty())
            {
                rows.pop();
                if let Some(last) = rows.last_mut() {
                    last.1 = "";
                }
            }
            for (row, sep) in rows {
                let (mut clean, info) = clean_math_row(row);
                if base == "eqnarray" {
                    clean = drop_second_amp(&clean);
                }
                let number = self.row_number(&info, star, clean.trim().is_empty());
                if let Some((n, _)) = &number {
                    for key in &info.labels {
                        self.st.labels.insert(
                            key.clone(),
                            Label {
                                kind: "equation".into(),
                                number: n.clone(),
                            },
                        );
                    }
                }
                numbers.extend(number);
                cleaned.push_str(&clean);
                cleaned.push_str(sep);
            }
        } else {
            let (clean, info) = clean_math_row(&raw);
            let number = self.row_number(&info, star, false);
            if let Some((n, _)) = &number {
                for key in &info.labels {
                    self.st.labels.insert(
                        key.clone(),
                        Label {
                            kind: "equation".into(),
                            number: n.clone(),
                        },
                    );
                }
            }
            numbers.extend(number);
            cleaned = clean;
        }
        let number = match numbers.as_slice() {
            [] => None,
            [(n, _)] => Some(format!("({n})")),
            [(first, true), .., (last, true)] if numbers.iter().all(|(_, c)| *c) => {
                Some(format!("({first})–({last})"))
            }
            _ => Some(
                numbers
                    .iter()
                    .map(|(n, _)| format!("({n})"))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        };
        let body = cleaned.trim();
        let tex = match base {
            "align" | "flalign" | "eqnarray" => format!("\\begin{{aligned}}{body}\\end{{aligned}}"),
            "gather" | "multline" => format!("\\begin{{gathered}}{body}\\end{{gathered}}"),
            "alignat" | "xalignat" | "xxalignat" => {
                format!("\\begin{{alignedat}}{{{cols}}}{body}\\end{{alignedat}}")
            }
            _ => body.to_string(),
        };
        sink.block(Block::Math { tex, number });
    }

    /// A formula row's number: its `\tag`, else the next equation number
    /// unless starred, `\nonumber`ed or empty. `true`: from the counter.
    fn row_number(&mut self, info: &RowInfo, star: bool, blank: bool) -> Option<(String, bool)> {
        if let Some(tag) = &info.tag {
            return Some((tag.clone(), false));
        }
        if star || info.nonumber || blank {
            return None;
        }
        self.st.step("equation");
        Some((self.st.format("equation"), true))
    }

    // -- commands ----------------------------------------------------------

    fn command(&mut self, at: usize, end: usize, sink: &mut Sink, style: &mut Style) -> usize {
        let (name, after, word) = self.read_name(at, end);
        if !word {
            return self.symbol(name, after, end, sink, style);
        }
        match name {
            "verb" | "lstinline" => {
                return match verb_extent(self.s, after, end) {
                    Some((code, after)) => {
                        let text = self.slice(code);
                        sink.text(text, &style.with(|s| s.inline.code = true));
                        after
                    }
                    None => after,
                };
            }
            "mintinline" => {
                let (_, i) = self.args(after, end, "og");
                if let Some((code, after)) = verb_extent(self.s, i, end) {
                    let text = self.slice(code);
                    sink.text(text, &style.with(|s| s.inline.code = true));
                    return after;
                }
                return i;
            }
            _ => {}
        }
        let i = self.skip_ws(after, end);
        if let Some(m) = self.st.user.get(name).cloned() {
            return self.expand(&m, i, end, sink, style);
        }
        let aliased;
        let name = match self.st.aliases.get(name) {
            Some(a) => {
                aliased = a.clone();
                aliased.as_str()
            }
            None => name,
        };
        self.builtin(name, at, i, end, sink, style)
    }

    fn symbol(
        &mut self,
        name: &str,
        i: usize,
        end: usize,
        sink: &mut Sink,
        style: &mut Style,
    ) -> usize {
        if let Some(accent) = is_accent_symbol(name) {
            return self.accent(accent, i, end, sink, style);
        }
        match name {
            "\\" => {
                let mut k = i;
                if k < end && self.b[k] == b'*' {
                    k += 1;
                }
                if k < end
                    && self.b[k] == b'['
                    && let Some((_, after)) = self.bracket_from(k, end)
                {
                    k = after;
                }
                sink.brk();
                k
            }
            "&" | "%" | "$" | "#" | "_" | "{" | "}" => {
                sink.text(name, style);
                i
            }
            " " | "\n" | "\t" | "," | ";" | ":" | ">" => {
                sink.hard_space();
                i
            }
            "|" => {
                sink.text("‖", style);
                i
            }
            "(" => match self.find_close(i, end, b')') {
                Some(close) => {
                    sink.math(self.slice(i..close));
                    close + 2
                }
                None => i,
            },
            "[" => match self.find_close(i, end, b']') {
                Some(close) => {
                    let tex = self.slice(i..close);
                    self.display_math(tex, sink);
                    close + 2
                }
                None => i,
            },
            _ => i,
        }
    }

    fn accent(
        &mut self,
        accent: char,
        i: usize,
        end: usize,
        sink: &mut Sink,
        style: &Style,
    ) -> usize {
        let Some((arg, after)) = self.req_arg(i, end) else {
            return i;
        };
        let base = tex_to_plain(self.slice(arg));
        sink.text(&compose_accent(accent, &base), style);
        after
    }

    fn styled(
        &mut self,
        i: usize,
        end: usize,
        sink: &mut Sink,
        style: &Style,
        f: impl FnOnce(&mut Style),
    ) -> usize {
        let Some((arg, after)) = self.req_arg(i, end) else {
            return i;
        };
        let mut inner = style.with(f);
        self.run(arg, sink, &mut inner);
        after
    }

    fn builtin(
        &mut self,
        name: &str,
        at: usize,
        i: usize,
        end: usize,
        sink: &mut Sink,
        style: &mut Style,
    ) -> usize {
        if let Some(sym) = text_symbol(name) {
            sink.text(sym, style);
            return i;
        }
        if let Some(accent) = is_accent_word(name) {
            return self.accent(accent, i, end, sink, style);
        }
        if SECTION_COMMANDS.contains(&name) {
            return self.heading(name, i, end, sink, style);
        }
        match name {
            "centering" => {
                sink.align = Some(Align::Center);
                return i;
            }
            "raggedleft" => {
                sink.align = Some(Align::Right);
                return i;
            }
            "label" => {
                let (a, after) = self.args(i, end, "g");
                let key = self.arg_text(&a[0]).to_string();
                self.st.record_label(&key);
                return after;
            }
            _ => {}
        }
        if NO_OPS.contains(&name) {
            return i;
        }
        if let Some(spec) = ignored_args(name) {
            return self.args(i, end, spec).1;
        }
        match name {
            "par" => {
                sink.flush();
                i
            }
            "newline" => {
                sink.brk();
                i
            }
            "quad" | "qquad" | "enspace" | "enskip" | "thinspace" | "space" | "hfill" | "hfil"
            | "dotfill" | "hrulefill" => {
                sink.hard_space();
                i
            }
            "vskip" | "hskip" | "kern" | "vglue" | "hglue" | "mskip" => self.skip_dimen(i, end),
            "appendix" => {
                self.start_appendix();
                i
            }
            "maketitle" => {
                self.maketitle(sink);
                i
            }
            "title" | "author" | "date" => {
                let (a, after) = self.args(i, end, "om");
                let text = self.arg_text(&a[1]).to_string();
                match name {
                    "title" => self.st.title = Some(text),
                    "author" => self.st.author = Some(text),
                    _ => self.st.date = Some(text),
                }
                after
            }
            "textbf" => self.styled(i, end, sink, style, |s| s.inline.bold = true),
            "textit" | "textsl" => self.styled(i, end, sink, style, |s| s.inline.italic = true),
            "emph" => self.styled(i, end, sink, style, |s| s.inline.italic = !s.inline.italic),
            "texttt" => self.styled(i, end, sink, style, |s| s.inline.code = true),
            "textsc" | "MakeUppercase" | "uppercase" => {
                self.styled(i, end, sink, style, |s| s.caps = true)
            }
            "sout" | "st" | "xout" | "strike" => {
                self.styled(i, end, sink, style, |s| s.inline.strikethrough = true)
            }
            "textnormal" => self.styled(i, end, sink, style, |s| {
                s.inline = InlineStyle::default();
                s.caps = false;
            }),
            "textup" => self.styled(i, end, sink, style, |s| s.inline.italic = false),
            "textmd" => self.styled(i, end, sink, style, |s| s.inline.bold = false),
            "bfseries" | "bf" => {
                style.inline.bold = true;
                i
            }
            "itshape" | "it" | "slshape" | "sl" => {
                style.inline.italic = true;
                i
            }
            "em" => {
                style.inline.italic = !style.inline.italic;
                i
            }
            "ttfamily" | "tt" => {
                style.inline.code = true;
                i
            }
            "scshape" | "sc" => {
                style.caps = true;
                i
            }
            "normalfont" => {
                style.inline = InlineStyle::default();
                style.caps = false;
                i
            }
            "upshape" => {
                style.inline.italic = false;
                i
            }
            "mdseries" => {
                style.inline.bold = false;
                i
            }
            "rmfamily" | "rm" | "sffamily" | "sf" => i,
            "url" | "nolinkurl" => {
                let (a, after) = self.args(i, end, "g");
                let url = unescape_url(self.arg_text(&a[0]));
                let link = (name == "url").then(|| url.clone());
                sink.text(&url, &style.with(|s| s.link = link));
                after
            }
            "href" => {
                let (a, after) = self.args(i, end, "gm");
                let url = unescape_url(self.arg_text(&a[0]));
                if let Some(text) = a[1].clone() {
                    let mut inner = style.with(|s| s.link = Some(url));
                    self.run(text, sink, &mut inner);
                }
                after
            }
            "footnote" => self.footnote(i, end, sink, style, true),
            "footnotetext" => self.footnote(i, end, sink, style, false),
            "footnotemark" => self.args(i, end, "o").1,
            "cite" | "citep" | "citet" | "parencite" | "textcite" | "autocite" | "Cite"
            | "Citep" | "Citet" | "Parencite" | "Textcite" | "Autocite" | "citealp" | "citealt"
            | "citeauthor" | "citeyear" | "footcite" | "smartcite" | "supercite" | "cites" => {
                self.cite(i, end, sink)
            }
            "ref" | "autoref" | "cref" | "Cref" | "pageref" | "vref" | "nameref" | "eqref"
            | "Autoref" | "cpageref" | "subref" => self.reference(name, i, end, sink, style),
            "includegraphics" => self.graphic(i, end, sink, style),
            "caption" => self.caption(i, end, sink, style, None),
            "captionof" => {
                let (a, after) = self.args(i, end, "sg");
                let kind = match self.arg_text(&a[1]).trim() {
                    "table" => Float::Table,
                    _ => Float::Figure,
                };
                self.caption(after, end, sink, style, Some(kind))
            }
            "input" | "include" | "subfile" | "InputIfFileExists" => {
                let (a, after) = self.args(i, end, "g");
                let file = self.arg_text(&a[0]).trim().to_string();
                self.code_paragraph(&format!("input: {file}"), sink);
                after
            }
            "import" | "subimport" | "includefrom" | "subincludefrom" => {
                let (a, after) = self.args(i, end, "sgg");
                let file = format!(
                    "{}{}",
                    self.arg_text(&a[1]).trim(),
                    self.arg_text(&a[2]).trim()
                );
                self.code_paragraph(&format!("input: {file}"), sink);
                after
            }
            "bibliography" => {
                let (a, after) = self.args(i, end, "g");
                let files = self.arg_text(&a[0]).trim().to_string();
                self.code_paragraph(&format!("bibliography: {files}"), sink);
                after
            }
            "printbibliography" => {
                let (_, after) = self.args(i, end, "o");
                self.code_paragraph("bibliography", sink);
                after
            }
            "newcommand"
            | "renewcommand"
            | "providecommand"
            | "DeclareRobustCommand"
            | "def"
            | "gdef"
            | "edef"
            | "xdef"
            | "let"
            | "DeclareMathOperator" => self.define(name, at, i, end),
            "newtheorem" => self.newtheorem(i, end),
            "declaretheorem" => {
                let (a, after) = self.args(i, end, "og");
                let env = self.arg_text(&a[1]).trim().to_string();
                let opts = self.arg_text(&a[0]);
                let title = key_value(opts, "name").map_or_else(
                    || {
                        let mut c = env.chars();
                        c.next()
                            .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                            .unwrap_or_default()
                    },
                    str::to_string,
                );
                let counter = key_value(opts, "sibling")
                    .or_else(|| key_value(opts, "numberlike"))
                    .map_or_else(|| env.clone(), str::to_string);
                let parent = key_value(opts, "parent")
                    .or_else(|| key_value(opts, "numberwithin"))
                    .map(str::to_string);
                self.st.counters.entry(counter.clone()).or_insert(Counter {
                    value: 0,
                    parent,
                    fmt: Fmt::Arabic,
                });
                let italic = key_value(opts, "style").is_none_or(|s| s == "plain");
                self.st.theorems.insert(
                    env,
                    Theorem {
                        title,
                        counter: Some(counter),
                        italic,
                    },
                );
                after
            }
            "theoremstyle" => {
                let (a, after) = self.args(i, end, "g");
                self.st.theorem_italic = self.arg_text(&a[0]).trim() == "plain";
                after
            }
            "newcounter" => {
                let (a, after) = self.args(i, end, "go");
                let counter = self.arg_text(&a[0]).trim().to_string();
                let parent = a[1].clone().map(|r| self.slice(r).trim().to_string());
                self.st.counters.insert(
                    counter,
                    Counter {
                        value: 0,
                        parent,
                        fmt: Fmt::Arabic,
                    },
                );
                after
            }
            "setcounter" | "addtocounter" => {
                let (a, after) = self.args(i, end, "gg");
                let counter = self.arg_text(&a[0]).trim().to_string();
                let value: i64 = self.arg_text(&a[1]).trim().parse().unwrap_or(0);
                if counter == "secnumdepth" {
                    self.st.secnumdepth = value.clamp(-10, 10) as i32;
                } else if let Some(c) = self.st.counters.get_mut(&counter) {
                    let base = if name == "addtocounter" {
                        i64::from(c.value)
                    } else {
                        0
                    };
                    c.value = (base + value).clamp(0, i64::from(u32::MAX)) as u32;
                }
                after
            }
            "stepcounter" | "refstepcounter" => {
                let (a, after) = self.args(i, end, "g");
                let counter = self.arg_text(&a[0]).trim().to_string();
                self.st.step(&counter);
                if name == "refstepcounter" {
                    let number = self.st.format(&counter);
                    self.st.set_target(&counter, number);
                }
                after
            }
            "numberwithin" | "counterwithin" => {
                let (a, after) = self.args(i, end, "sgg");
                let counter = self.arg_text(&a[1]).trim().to_string();
                let parent = self.arg_text(&a[2]).trim().to_string();
                if let Some(c) = self.st.counters.get_mut(&counter) {
                    c.parent = Some(parent);
                }
                after
            }
            "begin" => self.environment(at, i, end, sink, style),
            "end" => self.env_name(i, end).map_or(i, |(_, after)| after),
            "hrule" => {
                sink.block(Block::Rule);
                i
            }
            "rule" => {
                let (a, after) = self.args(i, end, "ogg");
                let width = self.arg_text(&a[1]).trim();
                let zero = width
                    .trim_end_matches(|c: char| c.is_ascii_alphabetic())
                    .parse::<f64>()
                    .is_ok_and(|w| w == 0.0);
                if !zero {
                    sink.block(Block::Rule);
                }
                after
            }
            "ensuremath" => {
                let (a, after) = self.args(i, end, "g");
                sink.math(self.arg_text(&a[0]));
                after
            }
            "enquote" => {
                sink.text("“", style);
                let after = self.styled(i, end, sink, style, |_| {});
                sink.text("”", style);
                after
            }
            "SI" | "qty" => {
                let (a, after) = self.args(i, end, "omom");
                if let Some(value) = a[1].clone() {
                    let mut inner = style.clone();
                    self.run(value, sink, &mut inner);
                }
                sink.hard_space();
                if let Some(unit) = a[3].clone() {
                    let mut inner = style.clone();
                    self.run(unit, sink, &mut inner);
                }
                after
            }
            "cmidrule" => {
                let mut k = self.skip_ws(i, end);
                if k < end && self.b[k] == b'(' {
                    k = (k..end.min(k + 16))
                        .find(|&p| self.b[p] == b')')
                        .map_or(k, |p| p + 1);
                }
                self.args(k, end, "og").1
            }
            _ => self.unknown(i, end, sink, style),
        }
    }

    /// An unknown command: its directly following `[…]` / `{…}` arguments
    /// are read and the last braced one is shown.
    fn unknown(&mut self, i: usize, end: usize, sink: &mut Sink, style: &Style) -> usize {
        let mut k = i;
        if k < end && self.b[k] == b'*' {
            k += 1;
        }
        let mut last = None;
        while k < end {
            match self.b[k] {
                b'[' => match self.bracket_from(k, end) {
                    Some((_, after)) => k = after,
                    None => break,
                },
                b'{' => {
                    let close = self.match_brace(k, end);
                    last = Some(k + 1..close);
                    k = (close + 1).min(end);
                }
                _ => break,
            }
        }
        if let Some(arg) = last {
            let mut inner = style.clone();
            self.run(arg, sink, &mut inner);
        }
        k
    }

    fn skip_dimen(&self, i: usize, end: usize) -> usize {
        let mut k = self.skip_ws(i, end);
        if k < end && self.b[k] == b'\\' {
            return self.read_name(k, end).1;
        }
        if k < end && matches!(self.b[k], b'-' | b'+') {
            k += 1;
        }
        while k < end && (self.b[k].is_ascii_digit() || self.b[k] == b'.') {
            k += 1;
        }
        while k < end && self.b[k] == b' ' {
            k += 1;
        }
        let unit_end = (k..end.min(k + 2))
            .take_while(|&p| self.b[p].is_ascii_alphabetic())
            .count();
        k + unit_end
    }

    fn code_paragraph(&self, text: &str, sink: &mut Sink) {
        let mut rt = RichText::default();
        push_rt(
            &mut rt,
            text,
            InlineStyle {
                code: true,
                ..InlineStyle::default()
            },
            None,
        );
        sink.block(Block::Paragraph(rt));
    }

    fn expand(
        &mut self,
        m: &Macro,
        i: usize,
        end: usize,
        sink: &mut Sink,
        style: &mut Style,
    ) -> usize {
        let mut i = i;
        let mut args: Vec<String> = Vec::with_capacity(m.nargs);
        if m.nargs > 0 {
            if let Some(default) = &m.default {
                match self.opt_arg(i, end) {
                    Some((r, after)) => {
                        args.push(self.slice(r).to_string());
                        i = after;
                    }
                    None => args.push(default.clone()),
                }
            }
            while args.len() < m.nargs {
                match self.req_arg(i, end) {
                    Some((r, after)) => {
                        args.push(self.slice(r).to_string());
                        i = after;
                    }
                    None => args.push(String::new()),
                }
            }
        }
        let body = substitute(&m.body, &args);
        let cost = body.len().max(16);
        if cost > self.st.budget {
            self.st.budget = 0;
            return i;
        }
        self.st.budget -= cost;
        self.sub(&body, |r| r.run(0..body.len(), sink, style));
        i
    }

    fn define(&mut self, cmd: &str, at: usize, i: usize, end: usize) -> usize {
        let after = match cmd {
            "newcommand" | "renewcommand" | "providecommand" | "DeclareRobustCommand" => {
                let (a, after) = self.args(i, end, "smoog");
                let name = macro_name(self.arg_text(&a[1]));
                let nargs = self
                    .arg_text(&a[2])
                    .trim()
                    .parse::<usize>()
                    .unwrap_or(0)
                    .min(9);
                let default = a[3].clone().map(|r| self.slice(r).to_string());
                if let (Some(name), Some(body)) = (name, a[4].clone())
                    && !(cmd == "providecommand" && self.st.user.contains_key(&name))
                {
                    let body = self.slice(body).to_string();
                    self.st.user.insert(
                        name,
                        Rc::new(Macro {
                            nargs,
                            default,
                            body,
                        }),
                    );
                }
                after
            }
            "def" | "gdef" | "edef" | "xdef" => {
                let k = self.skip_ws(i, end);
                if k >= end || self.b[k] != b'\\' {
                    return i;
                }
                let (name, mut k, _) = self.read_name(k, end);
                let params_start = k;
                while k < end && self.b[k] != b'{' && k - params_start < 64 {
                    k += 1;
                }
                if k >= end || self.b[k] != b'{' {
                    return k.min(end);
                }
                let params = self.slice(params_start..k).trim();
                let close = self.match_brace(k, end);
                let body = self.slice(k + 1..close).to_string();
                let simple = params
                    .as_bytes()
                    .chunks(2)
                    .enumerate()
                    .all(|(n, p)| p.len() == 2 && p[0] == b'#' && p[1] == b'1' + n as u8);
                if simple && !name.contains('@') && !name.is_empty() {
                    self.st.user.insert(
                        name.to_string(),
                        Rc::new(Macro {
                            nargs: params.len() / 2,
                            default: None,
                            body,
                        }),
                    );
                }
                (close + 1).min(end)
            }
            "let" => {
                let k = self.skip_ws(i, end);
                if k >= end || self.b[k] != b'\\' {
                    return i;
                }
                let (name, mut k, _) = self.read_name(k, end);
                while k < end && matches!(self.b[k], b' ' | b'=') {
                    k += 1;
                }
                if k >= end || self.b[k] != b'\\' {
                    return k.min(end);
                }
                let (target, after, _) = self.read_name(k, end);
                if !name.is_empty() && !name.contains('@') {
                    if let Some(m) = self.st.user.get(target).cloned() {
                        self.st.user.insert(name.to_string(), m);
                    } else {
                        self.st.user.remove(name);
                        self.st.aliases.insert(name.to_string(), target.to_string());
                    }
                }
                after
            }
            _ => self.args(i, end, "sgg").1,
        };
        let definition = self.slice(at..after);
        if !definition.contains('@') {
            self.st.macros.push_str(definition.trim());
            self.st.macros.push('\n');
        }
        after
    }

    fn newtheorem(&mut self, i: usize, end: usize) -> usize {
        let (a, mut after) = self.args(i, end, "sgo");
        let star = a[0].is_some();
        let env = self.arg_text(&a[1]).trim().to_string();
        let shared = a[2].clone().map(|r| self.slice(r).trim().to_string());
        let Some((title, after_title)) = self.group_arg(after, end) else {
            return after;
        };
        after = after_title;
        let title = tex_to_plain(self.slice(title));
        let mut parent = None;
        if shared.is_none()
            && let Some((p, after_parent)) = self.opt_arg(after, end)
        {
            parent = Some(self.slice(p).trim().to_string());
            after = after_parent;
        }
        let counter = if star {
            None
        } else if let Some(shared) = shared {
            Some(
                self.st
                    .theorems
                    .get(&shared)
                    .and_then(|t| t.counter.clone())
                    .unwrap_or(shared),
            )
        } else {
            self.st.counters.insert(
                env.clone(),
                Counter {
                    value: 0,
                    parent,
                    fmt: Fmt::Arabic,
                },
            );
            Some(env.clone())
        };
        let italic = self.st.theorem_italic;
        if !env.is_empty() {
            self.st.theorems.insert(
                env,
                Theorem {
                    title,
                    counter,
                    italic,
                },
            );
        }
        after
    }

    fn heading(
        &mut self,
        cmd: &str,
        i: usize,
        end: usize,
        sink: &mut Sink,
        style: &Style,
    ) -> usize {
        let (a, after) = self.args(i, end, "sog");
        let Some(title) = a[2].clone() else {
            return after;
        };
        sink.flush();
        let (level, depth) = match cmd {
            "part" => (1, -1),
            "chapter" => (1, 0),
            "section" => (2, 1),
            "subsection" => (3, 2),
            "subsubsection" => (4, 3),
            "paragraph" => (5, 4),
            _ => (6, 5),
        };
        let numbered = a[0].is_none() && (depth == -1 || depth <= self.st.secnumdepth);
        let mut text = RichText::default();
        if numbered {
            self.st.step(cmd);
            let number = self.st.format(cmd);
            let top = if self.st.chapters {
                "chapter"
            } else {
                "section"
            };
            let kind = if self.st.appendix && cmd == top {
                "appendix"
            } else {
                cmd
            };
            self.st.set_target(kind, number.clone());
            let prefix = if cmd == "part" {
                format!("Part {number}: ")
            } else {
                format!("{number} ")
            };
            push_rt(&mut text, &prefix, style.inline, None);
        }
        let title = self.inline(title, style);
        append_rich(&mut text, title);
        text.text = text.text.replace('\n', " ");
        sink.blocks.push(Block::Heading { level, text });
        after
    }

    fn start_appendix(&mut self) {
        self.st.appendix = true;
        let top = if self.st.chapters {
            "chapter"
        } else {
            "section"
        };
        if let Some(c) = self.st.counters.get_mut(top) {
            c.value = 0;
            c.fmt = Fmt::Alph;
        }
    }

    fn maketitle(&mut self, sink: &mut Sink) {
        if let Some(title) = self.st.title.clone() {
            let mut text = self.inline_str(&title, &Style::default());
            text.text = text.text.replace('\n', " ");
            if has_content(&text) {
                sink.block(Block::Heading { level: 1, text });
            }
        }
        let mut para = RichText::default();
        if let Some(author) = self.st.author.clone() {
            for name in split_top_str(&author, "and") {
                for line in split_top_str(name, "\\") {
                    let rt = self.inline_str(line, &Style::default());
                    if !has_content(&rt) {
                        continue;
                    }
                    if !para.is_empty() {
                        push_rt(&mut para, ", ", InlineStyle::default(), None);
                    }
                    append_rich(&mut para, rt);
                }
            }
        }
        if let Some(date) = self.st.date.clone() {
            let rt = self.inline_str(&date, &Style::default());
            if has_content(&rt) {
                if !para.is_empty() {
                    push_rt(&mut para, "\n", InlineStyle::default(), None);
                }
                append_rich(&mut para, rt);
            }
        }
        if has_content(&para) {
            sink.block(Block::Paragraph(para));
        }
    }

    fn footnote(
        &mut self,
        i: usize,
        end: usize,
        sink: &mut Sink,
        style: &Style,
        mark: bool,
    ) -> usize {
        let (a, after) = self.args(i, end, "og");
        let Some(text) = a[1].clone() else {
            return after;
        };
        let slot = self.st.footnotes.len();
        self.st.footnotes.push(Vec::new());
        let blocks = self.blocks_of(text, &Style::default());
        if let Some(note) = self.st.footnotes.get_mut(slot) {
            *note = blocks;
        }
        if mark {
            let n = slot + 1;
            let link = format!("#fn-{n}");
            sink.text(&format!("[{n}]"), &style.with(|s| s.link = Some(link)));
        }
        after
    }

    fn cite(&mut self, i: usize, end: usize, sink: &mut Sink) -> usize {
        let (a, after) = self.args(i, end, "soog");
        let keys: Vec<String> = self
            .arg_text(&a[3])
            .split(',')
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
            .collect();
        let (pre, post) = match (&a[1], &a[2]) {
            (Some(pre), Some(post)) => (
                tex_to_plain(self.slice(pre.clone())),
                tex_to_plain(self.slice(post.clone())),
            ),
            (Some(post), None) => (String::new(), tex_to_plain(self.slice(post.clone()))),
            _ => (String::new(), String::new()),
        };
        let code = Style {
            inline: InlineStyle {
                code: true,
                ..InlineStyle::default()
            },
            caps: false,
            link: None,
        };
        sink.text("[", &code);
        if !pre.is_empty() {
            sink.text(&format!("{pre} "), &code);
        }
        for (n, key) in keys.iter().enumerate() {
            if n > 0 {
                sink.text(", ", &code);
            }
            let link = format!("cite:{key}");
            sink.text(key, &code.with(|s| s.link = Some(link)));
        }
        if !post.is_empty() {
            sink.text(&format!(", {post}"), &code);
        }
        sink.text("]", &code);
        after
    }

    fn reference(
        &mut self,
        cmd: &str,
        i: usize,
        end: usize,
        sink: &mut Sink,
        style: &Style,
    ) -> usize {
        let (a, after) = self.args(i, end, "sg");
        let arg = self.arg_text(&a[1]).to_string();
        let keys: Vec<&str> = if matches!(cmd, "cref" | "Cref" | "cpageref") {
            arg.split(',').collect()
        } else {
            vec![arg.as_str()]
        };
        let mut parts = Vec::new();
        for key in keys {
            let label = self.st.lookup(key);
            let number = label.as_ref().map_or("??", |l| l.number.as_str());
            let kind = label.as_ref().map_or("", |l| l.kind.as_str());
            parts.push(match cmd {
                "eqref" => format!("({number})"),
                "autoref" | "Autoref" => match ref_name(kind, RefForm::Auto) {
                    Some(n) => format!("{n} {number}"),
                    None => number.to_string(),
                },
                "cref" | "Cref" => {
                    let form = if cmd == "cref" {
                        RefForm::Lower
                    } else {
                        RefForm::Upper
                    };
                    let shown = if kind == "equation" {
                        format!("({number})")
                    } else {
                        number.to_string()
                    };
                    match ref_name(kind, form) {
                        Some(n) => format!("{n} {shown}"),
                        None => shown,
                    }
                }
                _ => number.to_string(),
            });
        }
        sink.text(&parts.join(", "), style);
        after
    }

    fn graphic(&mut self, i: usize, end: usize, sink: &mut Sink, style: &Style) -> usize {
        let (a, after) = self.args(i, end, "soog");
        let Some(path) = a[3].clone() else {
            return after;
        };
        let src = self.slice(path).trim().to_string();
        let opts = self.arg_text(&a[1]);
        let width = key_value(opts, "width").and_then(dim_px);
        let height = key_value(opts, "height").and_then(dim_px);
        let alt = self
            .st
            .figure_alt
            .clone()
            .filter(|a| !a.is_empty())
            .unwrap_or_else(|| src.clone());
        sink.image(InlineImage {
            at: 0,
            src,
            alt,
            width,
            height,
            link: style.link.clone(),
        });
        after
    }

    fn caption(
        &mut self,
        i: usize,
        end: usize,
        sink: &mut Sink,
        style: &Style,
        kind: Option<Float>,
    ) -> usize {
        let (a, after) = self.args(i, end, "sog");
        let Some(text) = a[2].clone() else {
            return after;
        };
        sink.flush();
        let numbered = a[0].is_none();
        let prefix = match (kind.or(self.st.float), numbered) {
            (Some(Float::Figure), true) => {
                self.st.step("figure");
                self.st.figure_captioned = true;
                let n = self.st.format("figure");
                self.st.set_target("figure", n.clone());
                format!("Figure {n}:")
            }
            (Some(Float::Table), true) => {
                self.st.step("table");
                let n = self.st.format("table");
                self.st.set_target("table", n.clone());
                format!("Table {n}:")
            }
            (Some(Float::Sub), true) => {
                self.st.subfigure += 1;
                let letter = alph(self.st.subfigure, false);
                let figure = self.st.counters.get("figure").map_or(0, |c| c.value)
                    + u32::from(!self.st.figure_captioned);
                self.st.set_target("figure", format!("{figure}{letter}"));
                format!("({letter})")
            }
            _ => String::new(),
        };
        let mut rt = RichText::default();
        if !prefix.is_empty() {
            push_rt(
                &mut rt,
                &prefix,
                InlineStyle {
                    bold: true,
                    ..style.inline
                },
                None,
            );
            push_rt(&mut rt, " ", InlineStyle::default(), None);
        }
        let caption = self.inline(text, style);
        append_rich(&mut rt, caption);
        sink.block(Block::Paragraph(rt));
        after
    }

    // -- environments ------------------------------------------------------

    fn environment(
        &mut self,
        begin_at: usize,
        i: usize,
        end: usize,
        sink: &mut Sink,
        style: &Style,
    ) -> usize {
        let Some((name, after_name)) = self.env_name(i, end) else {
            return i;
        };
        if is_verbatim(name) {
            return self.verbatim_env(name, after_name, end, sink);
        }
        let (body_end, after) = self.find_end(name, after_name, end);
        if PICTURES.contains(&name) {
            sink.block(Block::CodeBlock {
                language: Some("latex".to_string()),
                code: self.slice(begin_at..after).to_string(),
            });
            return after;
        }
        let saved_target = self.st.target.clone();
        let saved_float = self.st.float;
        self.env_body(name, after_name..body_end, sink, style);
        self.st.target = saved_target;
        self.st.float = saved_float;
        after
    }

    fn env_body(&mut self, name: &str, mut body: Range<usize>, sink: &mut Sink, style: &Style) {
        let base = name.trim_end_matches('*');
        match base {
            "itemize" | "compactitem" | "asparaitem" | "inparaitem" | "tablenotes" => {
                self.list(ListKind::Itemize, body, sink, style)
            }
            "enumerate" | "compactenum" | "asparaenum" | "inparaenum" => {
                self.list(ListKind::Enumerate, body, sink, style)
            }
            "description" | "compactdesc" | "asparadesc" => {
                self.list(ListKind::Description, body, sink, style)
            }
            "thebibliography" => {
                let (_, after) = self.args(body.start, body.end, "g");
                body.start = after;
                let title = if self.st.chapters {
                    "Bibliography"
                } else {
                    "References"
                };
                sink.block(Block::Heading {
                    level: 2,
                    text: RichText {
                        text: title.to_string(),
                        ..RichText::default()
                    },
                });
                self.list(ListKind::Bibliography, body, sink, style);
            }
            "quote" | "quotation" | "verse" => {
                let blocks = self.blocks_of(body, style);
                if !blocks.is_empty() {
                    sink.block(Block::BlockQuote(blocks));
                }
            }
            "abstract" => {
                let blocks = self.blocks_of(body, style);
                sink.block(Block::Heading {
                    level: 3,
                    text: RichText {
                        text: "Abstract".to_string(),
                        ..RichText::default()
                    },
                });
                sink.blocks.extend(blocks);
            }
            "center" | "flushleft" | "flushright" => {
                let align = match base {
                    "center" => Align::Center,
                    "flushleft" => Align::Left,
                    _ => Align::Right,
                };
                let blocks = self.blocks_of(body, style);
                if !blocks.is_empty() {
                    sink.block(Block::Aligned { align, blocks });
                }
            }
            "figure" | "wrapfigure" | "sidewaysfigure" | "SCfigure" | "marginfigure" => {
                self.float_env(Float::Figure, base, body, sink, style)
            }
            "table" | "wraptable" | "sidewaystable" | "margintable" => {
                self.float_env(Float::Table, base, body, sink, style)
            }
            "subfigure" | "subtable" => self.float_env(Float::Sub, base, body, sink, style),
            "tabular" | "tabularx" | "tabulary" | "longtable" | "array" | "tabu"
            | "supertabular" | "xtabular" => {
                let spec = match name {
                    "tabular*" | "tabularx" | "tabulary" => "gog",
                    _ => "og",
                };
                let (a, after) = self.args(body.start, body.end, spec);
                body.start = after;
                let cols = self.arg_text(a.last().unwrap_or(&None)).to_string();
                let saved = self.st.float;
                if base == "longtable" {
                    self.st.float = Some(Float::Table);
                }
                self.tabular(&cols, body, sink, style);
                self.st.float = saved;
            }
            "equation" | "align" | "gather" | "multline" | "flalign" | "alignat" | "xalignat"
            | "xxalignat" | "eqnarray" | "displaymath" | "dmath" => self.math_env(name, body, sink),
            "split" | "aligned" | "gathered" | "cases" | "matrix" | "pmatrix" | "bmatrix" => {
                let tex = format!("\\begin{{{name}}}{}\\end{{{name}}}", self.slice(body));
                sink.block(Block::Math { tex, number: None });
            }
            "math" => {
                let tex = self.slice(body);
                sink.math(tex);
            }
            "proof" => self.proof(body, sink, style),
            "filecontents" | "comment" => {}
            _ => {
                if let Some(def) = self.st.theorems.get(name).cloned() {
                    self.theorem(&def, body, sink, style);
                    return;
                }
                if KNOWN_WRAPPERS.contains(&name) {
                    let spec = match base {
                        "minipage" => "oog",
                        "multicols" | "adjustbox" | "otherlanguage" | "spacing" | "column"
                        | "block" => "g",
                        "algorithm" | "algorithmic" => "o",
                        _ => "",
                    };
                    let (_, after) = self.args(body.start, body.end, spec);
                    body.start = after;
                    if base == "appendices" {
                        self.start_appendix();
                    }
                    let blocks = self.blocks_of(body, style);
                    sink.flush();
                    sink.blocks.extend(blocks);
                    return;
                }
                if let Some((_, after)) = self.opt_arg(body.start, body.end) {
                    body.start = after;
                }
                let mut blocks = self.blocks_of(body, style);
                if blocks.is_empty() {
                    return;
                }
                let mut label = RichText::default();
                push_rt(
                    &mut label,
                    name,
                    InlineStyle {
                        code: true,
                        ..InlineStyle::default()
                    },
                    None,
                );
                match blocks.first_mut() {
                    Some(Block::Paragraph(first)) => {
                        push_rt(&mut label, " ", InlineStyle::default(), None);
                        append_rich(&mut label, std::mem::take(first));
                        *first = label;
                    }
                    _ => blocks.insert(0, Block::Paragraph(label)),
                }
                sink.flush();
                sink.blocks.extend(blocks);
            }
        }
    }

    fn verbatim_env(
        &mut self,
        name: &str,
        after_name: usize,
        end: usize,
        sink: &mut Sink,
    ) -> usize {
        let (body_end, after) = self.raw_end(name, after_name, end);
        let mut start = after_name;
        let mut language = None;
        match name {
            "filecontents" | "filecontents*" | "comment" => return after,
            "lstlisting" => {
                if start < body_end
                    && self.b[start] == b'['
                    && let Some((opts, a)) = self.bracket_from(start, body_end)
                {
                    language = key_value(self.slice(opts), "language").map(|l| {
                        let l = l.rsplit(']').next().unwrap_or(l);
                        l.trim().to_lowercase()
                    });
                    start = a;
                }
            }
            "minted" => {
                let (a, after_args) = self.args(start, body_end, "og");
                language = a[1].clone().map(|r| self.slice(r).trim().to_lowercase());
                start = after_args;
            }
            "Verbatim" | "Verbatim*" | "BVerbatim" | "LVerbatim" => {
                if start < body_end
                    && self.b[start] == b'['
                    && let Some((_, a)) = self.bracket_from(start, body_end)
                {
                    start = a;
                }
            }
            _ => {}
        }
        let mut code = self.slice(start..body_end);
        // the rest of the \begin line and the indent before \end
        if let Some(nl) = code.find('\n')
            && code[..nl].trim().is_empty()
        {
            code = &code[nl + 1..];
        }
        if let Some(nl) = code.rfind('\n')
            && code[nl..].trim().is_empty()
        {
            code = &code[..nl];
        }
        sink.block(Block::CodeBlock {
            language: language.filter(|l| !l.is_empty()),
            code: code.to_string(),
        });
        after
    }

    fn list(&mut self, kind: ListKind, mut body: Range<usize>, sink: &mut Sink, style: &Style) {
        let mut start = 1u64;
        if kind != ListKind::Bibliography
            && let Some((opts, after)) = self.opt_arg(body.start, body.end)
        {
            if let Some(s) = key_value(self.slice(opts), "start").and_then(|s| s.parse().ok()) {
                start = s;
            }
            body.start = after;
        }
        let names: &[&str] = if kind == ListKind::Bibliography {
            &["bibitem"]
        } else {
            &["item"]
        };
        let hits = self.top_hits(body.clone(), names, false, false);
        let numbered = matches!(kind, ListKind::Enumerate | ListKind::Bibliography);
        if numbered {
            self.st.enum_depth += 1;
        }
        let mut n = start.saturating_sub(1);
        let mut items = Vec::with_capacity(hits.len());
        for (k, &(_, after)) in hits.iter().enumerate() {
            let next = hits.get(k + 1).map_or(body.end, |h| h.0);
            let mut j = after;
            let mut label = None;
            if let Some((r, a)) = self.opt_arg(j, next) {
                label = Some(r);
                j = a;
            }
            if kind == ListKind::Bibliography
                && let Some((_, a)) = self.group_arg(j, next)
            {
                j = a;
            }
            let mut item = Sink::default();
            match label {
                Some(r) => {
                    let label_style = if kind == ListKind::Description {
                        style.with(|s| s.inline.bold = true)
                    } else {
                        style.clone()
                    };
                    let rt = self.inline(r, &label_style);
                    if has_content(&rt) {
                        item.para = rt;
                        item.space = true;
                    }
                }
                None if numbered => {
                    n += 1;
                    let shown = match self.st.enum_depth {
                        2 => alph(n as u32, false),
                        3 => roman(n as u32, false),
                        4 => alph(n as u32, true),
                        _ => n.to_string(),
                    };
                    self.st.set_target("item", shown);
                }
                None => {}
            }
            let mut inner = style.clone();
            self.run(j..next, &mut item, &mut inner);
            item.flush();
            items.push(ListItem {
                task: None,
                blocks: item.blocks,
            });
        }
        if numbered {
            self.st.enum_depth -= 1;
        }
        if !items.is_empty() {
            sink.block(Block::List {
                start: numbered.then_some(start),
                items,
            });
        }
    }

    fn float_env(
        &mut self,
        kind: Float,
        name: &str,
        mut body: Range<usize>,
        sink: &mut Sink,
        style: &Style,
    ) {
        let spec = match name {
            "wrapfigure" | "wraptable" => "ogog",
            "subfigure" | "subtable" => "og",
            _ => "o",
        };
        let (_, after) = self.args(body.start, body.end, spec);
        body.start = after;
        let saved_alt = self.st.figure_alt.take();
        if matches!(kind, Float::Figure | Float::Sub) {
            self.st.figure_alt = self.caption_text(body.clone());
        }
        if kind == Float::Figure {
            self.st.subfigure = 0;
            self.st.figure_captioned = false;
        }
        self.st.float = Some(kind);
        let blocks = self.blocks_of(body, style);
        sink.flush();
        sink.blocks.extend(blocks);
        self.st.figure_alt = saved_alt;
    }

    /// The plain text of the float's own `\caption`, for image alt texts.
    fn caption_text(&self, body: Range<usize>) -> Option<String> {
        let hits = self.top_hits(body.clone(), &["caption"], false, false);
        let &(_, after) = hits.first()?;
        let (a, _) = self.args(after, body.end, "sog");
        a[2].clone().map(|r| tex_to_plain(self.slice(r)))
    }

    fn theorem(&mut self, def: &Theorem, mut body: Range<usize>, sink: &mut Sink, style: &Style) {
        let title = self.opt_arg(body.start, body.end).map(|(r, after)| {
            body.start = after;
            r
        });
        let mut header = def.title.clone();
        if let Some(counter) = &def.counter {
            self.st.step(counter);
            let n = self.st.format(counter);
            header = format!("{header} {n}");
            self.st.set_target(&def.title, n);
        }
        let bold = style.with(|s| {
            s.inline = InlineStyle {
                bold: true,
                ..InlineStyle::default()
            }
        });
        let mut inner = Sink::default();
        inner.text(&header, &bold);
        if let Some(title) = title {
            inner.text(" (", &Style::default());
            let rt = self.inline(title, &Style::default());
            append_rich(&mut inner.para, rt);
            inner.text(")", &Style::default());
        }
        inner.text(".", &bold);
        inner.space = true;
        let mut body_style = style.with(|s| s.inline.italic = def.italic);
        self.run(body, &mut inner, &mut body_style);
        inner.flush();
        sink.block(Block::BlockQuote(inner.blocks));
    }

    fn proof(&mut self, mut body: Range<usize>, sink: &mut Sink, style: &Style) {
        let italic = Style {
            inline: InlineStyle {
                italic: true,
                ..InlineStyle::default()
            },
            ..Style::default()
        };
        let mut inner = Sink::default();
        match self.opt_arg(body.start, body.end) {
            Some((title, after)) => {
                body.start = after;
                let rt = self.inline(title, &italic);
                inner.para = rt;
                inner.text(".", &italic);
            }
            None => inner.text("Proof.", &italic),
        }
        inner.space = true;
        let mut body_style = style.clone();
        self.run(body, &mut inner, &mut body_style);
        if has_content(&inner.para) {
            inner.space = false;
            inner.text("\u{a0}∎", &Style::default());
            inner.flush();
        } else {
            inner.flush();
            match inner.blocks.last_mut() {
                Some(Block::Paragraph(last)) => {
                    push_rt(last, "\u{a0}∎", InlineStyle::default(), None);
                }
                _ => inner.blocks.push(Block::Paragraph(RichText {
                    text: "∎".to_string(),
                    ..RichText::default()
                })),
            }
        }
        sink.block(Block::BlockQuote(inner.blocks));
    }

    fn tabular(&mut self, spec: &str, body: Range<usize>, sink: &mut Sink, style: &Style) {
        let mut align = colspec(spec, 0);
        let breaks = self.top_hits(body.clone(), &[], false, true);
        let mut row_ranges = Vec::with_capacity(breaks.len() + 1);
        let mut start = body.start;
        for &(at, after) in &breaks {
            row_ranges.push(start..at);
            start = after;
        }
        row_ranges.push(start..body.end);
        let mut rows: Vec<Vec<RichText>> = Vec::new();
        for row in row_ranges {
            if self.slice(row.clone()).contains("\\caption") {
                let mut inner = style.clone();
                self.run(row, sink, &mut inner);
                continue;
            }
            let amps = self.top_hits(row.clone(), &[], true, false);
            let mut cells = Vec::with_capacity(amps.len() + 1);
            let mut cell_start = row.start;
            let mut bounds = Vec::with_capacity(amps.len() + 1);
            for &(at, after) in &amps {
                bounds.push(cell_start..at);
                cell_start = after;
            }
            bounds.push(cell_start..row.end);
            for cell in bounds {
                let (content, span) = self.cell_content(cell);
                cells.push(self.inline(content, style));
                for _ in 1..span {
                    cells.push(RichText::default());
                }
            }
            if cells.iter().any(has_content) {
                rows.push(cells);
            }
        }
        if rows.is_empty() {
            return;
        }
        let head = rows.remove(0);
        let columns = rows
            .iter()
            .map(Vec::len)
            .chain([head.len(), align.len()])
            .max()
            .unwrap_or(0);
        align.resize(columns, None);
        let pad = |mut row: Vec<RichText>| {
            row.resize(columns, RichText::default());
            row
        };
        let head = pad(head);
        let rows = rows.into_iter().map(pad).collect();
        sink.block(Block::Table { align, head, rows });
    }

    /// A cell's content after leading rules, and its column span
    /// (`\multicolumn`).
    fn cell_content(&self, cell: Range<usize>) -> (Range<usize>, usize) {
        let end = cell.end;
        let mut k = cell.start;
        loop {
            k = self.skip_ws(k, end);
            while k < end && self.b[k].is_ascii_whitespace() {
                k += 1;
            }
            if k >= end || self.b[k] != b'\\' {
                break;
            }
            let (name, after, word) = self.read_name(k, end);
            if !word {
                break;
            }
            match name {
                "hline" | "toprule" | "midrule" | "bottomrule" | "hdashline" | "endhead"
                | "endfirsthead" | "endfoot" | "endlastfoot" => k = after,
                "cline" | "rowcolor" => k = self.args(after, end, "og").1,
                "cmidrule" | "specialrule" | "addlinespace" => {
                    let mut p = self.skip_ws(after, end);
                    if p < end && self.b[p] == b'(' {
                        p = (p..end.min(p + 16))
                            .find(|&q| self.b[q] == b')')
                            .map_or(p, |q| q + 1);
                    }
                    k = self
                        .args(p, end, if name == "addlinespace" { "o" } else { "og" })
                        .1;
                }
                "multicolumn" => {
                    let (a, _) = self.args(after, end, "ggg");
                    let span = self
                        .arg_text(&a[0])
                        .trim()
                        .parse::<usize>()
                        .unwrap_or(1)
                        .clamp(1, 64);
                    return (a[2].clone().unwrap_or(k..k), span);
                }
                _ => break,
            }
        }
        (k.min(end)..end, 1)
    }
}

/// The alignment of each column of a `tabular` spec.
fn colspec(spec: &str, depth: usize) -> Vec<Option<Align>> {
    let mut out = Vec::new();
    let b = spec.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        i += 1;
        match c {
            b'l' => out.push(Some(Align::Left)),
            b'c' => out.push(Some(Align::Center)),
            b'r' => out.push(Some(Align::Right)),
            b'L' => out.push(Some(Align::Left)),
            b'C' => out.push(Some(Align::Center)),
            b'R' | b'S' => out.push(Some(Align::Right)),
            b'p' | b'm' | b'b' => {
                out.push(None);
                if let Some((_, after)) = str_group(spec, i) {
                    i = after;
                }
            }
            b'@' | b'!' | b'>' | b'<' => {
                if let Some((_, after)) = str_group(spec, i) {
                    i = after;
                }
            }
            b'*' => {
                let Some((count, after)) = str_group(spec, i) else {
                    continue;
                };
                let Some((inner, after)) = str_group(spec, after) else {
                    i = after;
                    continue;
                };
                i = after;
                let n = count.trim().parse::<usize>().unwrap_or(1).min(64);
                if depth < 4 {
                    let cols = colspec(inner, depth + 1);
                    for _ in 0..n {
                        out.extend(cols.iter().copied());
                    }
                }
            }
            b'{' => {
                if let Some((_, after)) = str_group(spec, i - 1) {
                    i = after;
                }
            }
            c if c.is_ascii_alphabetic() => out.push(None),
            _ => {}
        }
        if out.len() > 256 {
            break;
        }
    }
    out
}

/// Split a raw string at top-level `\name` (or `\\` for `"\\"`).
fn split_top_str<'t>(s: &'t str, name: &str) -> Vec<&'t str> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            b'\\' => {
                let rest = &s[i + 1..];
                let word = name.as_bytes().first().is_some_and(u8::is_ascii_alphabetic);
                let hit = rest.starts_with(name)
                    && (!word
                        || !rest
                            .as_bytes()
                            .get(name.len())
                            .is_some_and(u8::is_ascii_alphabetic));
                if depth == 0 && hit {
                    out.push(&s[start..i]);
                    i += 1 + name.len();
                    start = i;
                    continue;
                }
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    out.push(&s[start.min(s.len())..]);
    out
}

fn unescape_url(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    let mut chars = url.trim().chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\'
            && let Some(&next) = chars.peek()
            && !next.is_ascii_alphabetic()
        {
            out.push(next);
            chars.next();
            continue;
        }
        out.push(c);
    }
    out
}

/// `\name` or `{\name}` of a definition, without the backslash.
fn macro_name(raw: &str) -> Option<String> {
    let name = raw.trim().trim_matches(['{', '}']).trim();
    let name = name.strip_prefix('\\')?;
    (!name.is_empty() && !name.contains('@')).then(|| name.to_string())
}

fn substitute(body: &str, args: &[String]) -> String {
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '#' {
            match chars.peek().copied() {
                Some('#') => {
                    out.push('#');
                    chars.next();
                }
                Some(d @ '1'..='9') => {
                    chars.next();
                    let n = d as usize - '1' as usize;
                    if let Some(arg) = args.get(n) {
                        out.push_str(arg);
                    }
                }
                _ => out.push('#'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[derive(Clone, Copy)]
enum RefForm {
    Auto,
    Lower,
    Upper,
}

fn ref_name(kind: &str, form: RefForm) -> Option<String> {
    let (auto, lower, upper) = match kind {
        "" => return None,
        "section" | "subsection" | "subsubsection" | "paragraph" | "subparagraph" => {
            ("Section", "section", "Section")
        }
        "chapter" => ("Chapter", "chapter", "Chapter"),
        "part" => ("Part", "part", "Part"),
        "appendix" => ("Appendix", "appendix", "Appendix"),
        "equation" => ("Equation", "eq.", "Eq."),
        "figure" => ("Figure", "fig.", "Fig."),
        "table" => ("Table", "table", "Table"),
        "item" => ("Item", "item", "Item"),
        theorem => {
            return Some(match form {
                RefForm::Lower => theorem.to_lowercase(),
                _ => theorem.to_string(),
            });
        }
    };
    Some(
        match form {
            RefForm::Auto => auto,
            RefForm::Lower => lower,
            RefForm::Upper => upper,
        }
        .to_string(),
    )
}

// ---------------------------------------------------------------------------
// Formula rows

#[derive(Default)]
struct RowInfo {
    labels: Vec<String>,
    tag: Option<String>,
    nonumber: bool,
}

/// Split math at top-level `\\`: (row, separator after it).
fn split_math_rows(s: &str) -> Vec<(&str, &str)> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut brace = 0usize;
    let mut env = 0usize;
    let mut start = 0;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'{' => brace += 1,
            b'}' => brace = brace.saturating_sub(1),
            b'\\' => {
                let rest = &s[i + 1..];
                if rest.starts_with("begin") {
                    env += 1;
                } else if rest.starts_with("end") {
                    env = env.saturating_sub(1);
                } else if rest.starts_with('\\') && brace == 0 && env == 0 {
                    let mut k = i + 2;
                    if k < b.len() && b[k] == b'*' {
                        k += 1;
                    }
                    if k < b.len()
                        && b[k] == b'['
                        && let Some(close) = s[k..].find(']')
                    {
                        k += close + 1;
                    }
                    out.push((&s[start..i], &s[i..k]));
                    start = k;
                    i = k;
                    continue;
                }
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    out.push((&s[start.min(s.len())..], ""));
    out
}

/// Remove `\label{…}`, `\tag{…}`, `\nonumber` and `\notag` from a formula,
/// keeping what they said.
fn clean_math_row(row: &str) -> (String, RowInfo) {
    let b = row.as_bytes();
    let mut info = RowInfo::default();
    let mut out = String::with_capacity(row.len());
    let mut copied = 0;
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'\\' {
            i += 1;
            continue;
        }
        let mut k = i + 1;
        while k < b.len() && b[k].is_ascii_alphabetic() {
            k += 1;
        }
        if k == i + 1 {
            i += 2;
            continue;
        }
        let name = &row[i + 1..k];
        match name {
            "nonumber" | "notag" => {
                out.push_str(&row[copied..i]);
                info.nonumber = true;
                copied = k;
            }
            "label" | "tag" => {
                let mut p = k;
                if name == "tag" && p < b.len() && b[p] == b'*' {
                    p += 1;
                }
                if let Some((arg, after)) = str_group(row, p) {
                    out.push_str(&row[copied..i]);
                    if name == "label" {
                        info.labels.push(arg.trim().to_string());
                    } else {
                        info.tag = Some(arg.trim().to_string());
                    }
                    copied = after;
                    k = after;
                }
            }
            _ => {}
        }
        i = k;
    }
    out.push_str(&row[copied.min(row.len())..]);
    (out, info)
}

/// `a &=& b` (eqnarray) → `a &= b` (aligned).
fn drop_second_amp(row: &str) -> String {
    let b = row.as_bytes();
    let mut depth = 0usize;
    let mut seen = 0;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 1,
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            b'&' if depth == 0 => {
                seen += 1;
                if seen == 2 {
                    return format!("{}{}", &row[..i], &row[i + 1..]);
                }
            }
            _ => {}
        }
        i += 1;
    }
    row.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paragraphs(blocks: &[Block]) -> Vec<String> {
        blocks
            .iter()
            .filter_map(|b| match b {
                Block::Paragraph(rt) => Some(rt.text.clone()),
                _ => None,
            })
            .collect()
    }

    fn headings(blocks: &[Block]) -> Vec<(u8, String)> {
        blocks
            .iter()
            .filter_map(|b| match b {
                Block::Heading { level, text } => Some((*level, text.text.clone())),
                _ => None,
            })
            .collect()
    }

    fn maths(blocks: &[Block]) -> Vec<(String, Option<String>)> {
        blocks
            .iter()
            .filter_map(|b| match b {
                Block::Math { tex, number } => Some((tex.clone(), number.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn sections_numbering_and_refs() {
        let doc = parse(
            "\\documentclass{article}\n\\title{A \\emph{Study}}\\author{Ann \\and Bob}\n\
             \\date{2024}\n\\begin{document}\n\\maketitle\n\
             See Section~\\ref{sec:b} and \\autoref{sec:b1}.\n\
             \\section{Intro}\\label{sec:a}\nText.\n\\subsection{Sub}\n\
             \\section*{Unnumbered}\n\\section[Short]{Second}\\label{sec:b}\n\
             \\subsection{One}\\label{sec:b1}\n\\paragraph{Para} p\n\
             \\appendix\n\\section{Extra}\\label{app}\nIn \\ref{app}, \\cref{sec:a}.\n\
             \\ref{missing}\n\\end{document}",
        );
        assert_eq!(doc.title.as_deref(), Some("A Study"));
        assert_eq!(
            headings(&doc.blocks),
            vec![
                (1, "A Study".to_string()),
                (2, "1 Intro".to_string()),
                (3, "1.1 Sub".to_string()),
                (2, "Unnumbered".to_string()),
                (2, "2 Second".to_string()),
                (3, "2.1 One".to_string()),
                (5, "Para".to_string()),
                (2, "A Extra".to_string()),
            ]
        );
        let paras = paragraphs(&doc.blocks);
        assert_eq!(paras[0], "Ann, Bob\n2024");
        assert_eq!(paras[1], "See Section\u{a0}2 and Section 2.1.");
        assert!(
            paras.contains(&"In A, section 1. ??".to_string()),
            "{paras:?}"
        );
        // the title's emphasis survives
        let Block::Heading { text, .. } = &doc.blocks[0] else {
            panic!()
        };
        assert!(text.spans[0].style.italic);
    }

    #[test]
    fn chapters_number_sections_within() {
        let doc = parse(
            "\\chapter{One}\\section{A}\\begin{equation}x\\label{e}\\end{equation}\
             \\chapter{Two}\\section{B}\\eqref{e}",
        );
        assert_eq!(
            headings(&doc.blocks),
            vec![
                (1, "1 One".to_string()),
                (2, "1.1 A".to_string()),
                (1, "2 Two".to_string()),
                (2, "2.1 B".to_string()),
            ]
        );
        assert_eq!(maths(&doc.blocks)[0].1.as_deref(), Some("(1.1)"));
        assert_eq!(paragraphs(&doc.blocks), vec!["(1.1)"]);
    }

    #[test]
    fn paragraphs_quotes_dashes_accents() {
        let doc = parse(
            "First line\nsame paragraph --- with ``quotes'' and `single'.\n\n\
             Second: pages 1--2, caf\\'e, {\\\"o}, \\\"{u}, \\c{c}, \\v s, \\ss, \\'{\\i}, \
             \\H{o}, \\k{a}, \\r{a}, \\~n, \\=a, \\.z, \\u{a}, \\aa\\ \\AE.\\par Third \\& 50\\% \
             \\$5 \\#1 a\\_b \\{x\\}\\\\next\\newline last\\ldots{} \\LaTeX{} and \\TeX.",
        );
        let paras = paragraphs(&doc.blocks);
        assert_eq!(
            paras,
            vec![
                "First line same paragraph — with “quotes” and ‘single’.",
                "Second: pages 1–2, café, ö, ü, ç, š, ß, í, ő, ą, å, ñ, ā, ż, ă, å Æ.",
                "Third & 50% $5 #1 a_b {x}\nnext\nlast… LaTeX and TeX.",
            ]
        );
    }

    #[test]
    fn inline_styles_links_and_citations() {
        let doc = parse(
            "\\textbf{bold} {\\it it} \\emph{em} \\texttt{a--b} \\textsc{caps} \\sout{gone} \
             {\\bfseries x \\itshape y} z \\url{https://x.org/a\\_b} \\href{https://e.com}{site} \
             \\cite[p.~3]{knuth,lamport} \\citep{one} \\textcolor{red}{red} \\mbox{box}\\unknowncmd.",
        );
        let Block::Paragraph(rt) = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert_eq!(
            rt.text,
            "bold it em a--b CAPS gone x y z https://x.org/a_b site [knuth, lamport, p. 3] [one] red box."
        );
        let span = |text: &str| {
            let at = rt.text.find(text).unwrap();
            rt.spans
                .iter()
                .find(|s| s.range.start <= at && s.range.end > at)
                .cloned()
        };
        assert!(span("bold").unwrap().style.bold);
        assert!(span("it ").unwrap().style.italic);
        assert!(span("a--b").unwrap().style.code);
        assert!(span("gone").unwrap().style.strikethrough);
        let xy = span("y z").unwrap();
        assert!(xy.style.bold && xy.style.italic);
        assert!(span(" z").is_none());
        assert_eq!(
            span("https").unwrap().link.as_deref(),
            Some("https://x.org/a_b")
        );
        assert_eq!(span("site").unwrap().link.as_deref(), Some("https://e.com"));
        let cite = span("lamport").unwrap();
        assert!(cite.style.code);
        assert_eq!(cite.link.as_deref(), Some("cite:lamport"));
        assert_eq!(span("knuth").unwrap().link.as_deref(), Some("cite:knuth"));
    }

    #[test]
    fn math_inline_display_and_numbering() {
        let doc = parse(
            "Let $a^2$ and \\(b\\) be.\n\\[ x = 1 \\]\n$$ y $$\n\
             \\begin{equation}\\label{eq:one} E = mc^2 \\end{equation}\n\
             \\begin{equation*} f \\end{equation*}\n\
             \\begin{align}\n a &= b \\label{eq:a}\\\\\n c &= d \\nonumber \\\\\n e &= f\n\\end{align}\n\
             \\begin{align*} g &= h \\tag{T} \\end{align*}\n\
             \\begin{gather} p \\\\ q \\tag{Q}\\label{eq:q} \\end{gather}\n\
             \\begin{eqnarray} a &=& b \\end{eqnarray}\n\
             \\begin{alignat}{2} x &= y & z \\end{alignat}\n\
             See \\eqref{eq:one}, \\ref{eq:a}, \\eqref{eq:q} and \\eqref{eq:later}.\n\
             \\begin{equation}\\label{eq:later} z \\end{equation}",
        );
        let Block::Paragraph(first) = &doc.blocks[0] else {
            panic!()
        };
        assert_eq!(first.text, "Let  and  be.");
        assert_eq!(first.maths.len(), 2);
        assert_eq!((first.maths[0].at, first.maths[0].tex.as_str()), (4, "a^2"));
        assert_eq!((first.maths[1].at, first.maths[1].tex.as_str()), (9, "b"));
        let m = maths(&doc.blocks);
        assert_eq!(m[0], ("x = 1".to_string(), None));
        assert_eq!(m[1], ("y".to_string(), None));
        assert_eq!(m[2], ("E = mc^2".to_string(), Some("(1)".to_string())));
        assert_eq!(m[3], ("f".to_string(), None));
        assert!(m[4].0.starts_with("\\begin{aligned}"), "{}", m[4].0);
        assert!(!m[4].0.contains("label") && !m[4].0.contains("nonumber"));
        assert_eq!(m[4].1.as_deref(), Some("(2)–(3)"));
        assert_eq!(m[5].1.as_deref(), Some("(T)"));
        assert!(!m[5].0.contains("tag"));
        assert!(m[6].0.starts_with("\\begin{gathered}"));
        assert_eq!(m[6].1.as_deref(), Some("(4), (Q)"));
        assert_eq!(m[7].0, "\\begin{aligned}a &= b\\end{aligned}");
        assert_eq!(m[7].1.as_deref(), Some("(5)"));
        assert_eq!(m[8].0, "\\begin{alignedat}{2}x &= y & z\\end{alignedat}");
        let paras = paragraphs(&doc.blocks);
        assert!(
            paras.contains(&"See (1), 2, (Q) and (7).".to_string()),
            "{paras:?}"
        );
    }

    #[test]
    fn nested_lists() {
        let doc = parse(
            "\\begin{itemize}\n\\item One\n\\item Two\n  \\begin{enumerate}\n  \\item A\\label{it:a}\n  \
             \\item B\n  \\end{enumerate}\n\\item[*] Star\n\\end{itemize}\n\
             \\begin{description}\\item[Term] Meaning\\end{description}\n\
             \\begin{enumerate}[start=3]\\item c\\end{enumerate}",
        );
        let Block::List { start, items } = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert_eq!(*start, None);
        assert_eq!(items.len(), 3);
        assert_eq!(paragraphs(&items[0].blocks), vec!["One"]);
        let Block::List {
            start,
            items: inner,
        } = &items[1].blocks[1]
        else {
            panic!("{:?}", items[1].blocks)
        };
        assert_eq!(*start, Some(1));
        assert_eq!(paragraphs(&inner[1].blocks), vec!["B"]);
        assert_eq!(paragraphs(&items[2].blocks), vec!["* Star"]);
        let Block::List { items, .. } = &doc.blocks[1] else {
            panic!()
        };
        let Block::Paragraph(rt) = &items[0].blocks[0] else {
            panic!()
        };
        assert_eq!(rt.text, "Term Meaning");
        assert!(rt.spans[0].style.bold && rt.spans[0].range == (0..4));
        assert!(matches!(doc.blocks[2], Block::List { start: Some(3), .. }));
    }

    #[test]
    fn tabular_with_multicolumn() {
        let doc = parse(
            "\\begin{tabular}{|l|c|r|p{2cm}|}\n\\hline\nName & \\multicolumn{2}{c}{Both} & Note \\\\ \\hline\n\
             a & $x$ & \\textbf{3} & 4\\% \\\\[2pt]\n\\cline{1-2} \\multicolumn{3}{l}{wide} & z \\\\\n\
             \\hline\n\\end{tabular}",
        );
        let Block::Table { align, head, rows } = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert_eq!(
            align,
            &vec![
                Some(Align::Left),
                Some(Align::Center),
                Some(Align::Right),
                None
            ]
        );
        let texts = |row: &Vec<RichText>| row.iter().map(|c| c.text.clone()).collect::<Vec<_>>();
        assert_eq!(texts(head), vec!["Name", "Both", "", "Note"]);
        assert_eq!(rows.len(), 2);
        assert_eq!(texts(&rows[0]), vec!["a", "", "3", "4%"]);
        assert_eq!(rows[0][1].maths[0].tex, "x");
        assert!(rows[0][2].spans[0].style.bold);
        assert_eq!(texts(&rows[1]), vec!["wide", "", "", "z"]);
    }

    #[test]
    fn figures_and_tables_number_captions() {
        let doc = parse(
            "\\begin{figure}[h]\n\\centering\n\\includegraphics[width=2cm]{img/a.png}\n\
             \\caption{A \\emph{cat}.}\\label{fig:a}\n\\end{figure}\n\
             \\begin{table}\\caption{Data}\\label{tab:d}\\begin{tabular}{ll}a&b\\\\\\end{tabular}\\end{table}\n\
             \\begin{figure}\\includegraphics{b.pdf}\\caption{Second}\\label{fig:b}\\end{figure}\n\
             Figures \\ref{fig:a}, \\ref{fig:b} and Table~\\ref{tab:d}.",
        );
        let Block::Aligned { align, blocks } = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert_eq!(*align, Align::Center);
        let Block::Paragraph(img) = &blocks[0] else {
            panic!()
        };
        assert_eq!(img.images[0].src, "img/a.png");
        assert_eq!(img.images[0].alt, "A cat.");
        assert_eq!(img.images[0].width, Some(76));
        let Block::Paragraph(caption) = &blocks[1] else {
            panic!()
        };
        assert_eq!(caption.text, "Figure 1: A cat.");
        assert!(caption.spans[0].style.bold);
        assert_eq!(paragraphs(&doc.blocks)[0], "Table 1: Data");
        assert!(matches!(doc.blocks[2], Block::Table { .. }));
        let paras = paragraphs(&doc.blocks);
        assert!(paras.contains(&"Figure 2: Second".to_string()));
        assert_eq!(paras.last().unwrap(), "Figures 1, 2 and Table\u{a0}1.");
        let Block::Paragraph(img) = &doc.blocks[3] else {
            panic!("{:?}", doc.blocks[3])
        };
        assert_eq!(img.images[0].alt, "Second");
        assert_eq!(img.images[0].width, None);
    }

    #[test]
    fn theorems_share_counters() {
        let doc = parse(
            "\\newtheorem{thm}{Theorem}[section]\n\\newtheorem{lem}[thm]{Lemma}\n\
             \\theoremstyle{definition}\\newtheorem{defn}{Definition}\n\\newtheorem*{rem}{Remark}\n\
             \\begin{document}\\section{S}\n\
             \\begin{thm}[Main]\\label{t:main} Body.\\end{thm}\n\\begin{lem}Small.\\end{lem}\n\
             \\begin{defn}Def.\\end{defn}\\begin{rem}Note.\\end{rem}\n\
             \\begin{proof}Easy.\\end{proof}\n\\begin{lemma}Default.\\end{lemma}\n\
             By \\ref{t:main}.\\end{document}",
        );
        let quotes: Vec<&Vec<Block>> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::BlockQuote(inner) => Some(inner),
                _ => None,
            })
            .collect();
        let first = |q: &Vec<Block>| paragraphs(q)[0].clone();
        assert_eq!(first(quotes[0]), "Theorem 1.1 (Main). Body.");
        assert_eq!(first(quotes[1]), "Lemma 1.2. Small.");
        assert_eq!(first(quotes[2]), "Definition 1. Def.");
        assert_eq!(first(quotes[3]), "Remark. Note.");
        assert_eq!(first(quotes[4]), "Proof. Easy.\u{a0}∎");
        assert_eq!(first(quotes[5]), "Lemma 1. Default.");
        let Block::Paragraph(rt) = &quotes[0][0] else {
            panic!()
        };
        assert!(rt.spans[0].style.bold);
        assert!(rt.spans.last().unwrap().style.italic, "{rt:?}");
        let Block::Paragraph(def) = &quotes[2][0] else {
            panic!()
        };
        assert!(def.spans.iter().all(|s| !s.style.italic));
        assert_eq!(paragraphs(&doc.blocks).last().unwrap(), "By 1.1.");
    }

    #[test]
    fn footnotes_are_collected() {
        let doc = parse("Text\\footnote{First \\emph{note}.} and more\\footnote{Second}.");
        let Block::Paragraph(rt) = &doc.blocks[0] else {
            panic!()
        };
        assert_eq!(rt.text, "Text[1] and more[2].");
        assert_eq!(rt.spans[0].link.as_deref(), Some("#fn-1"));
        let Some(Block::Footnotes(notes)) = doc.blocks.last() else {
            panic!("{:?}", doc.blocks)
        };
        assert_eq!(paragraphs(&notes[0]), vec!["First note."]);
        assert_eq!(paragraphs(&notes[1]), vec!["Second"]);
    }

    #[test]
    fn verbatim_and_listings() {
        let doc = parse(
            "\\begin{verbatim}\n  50% \\section{x} {\n\\end{verbatim}\n\
             \\begin{lstlisting}[language=Python, caption=x]\nprint(1) # %\n\\end{lstlisting}\n\
             \\begin{minted}{Rust}\nfn main() {}\n\\end{minted}\n\
             Inline \\verb|a%b{| and \\lstinline{x}.",
        );
        assert_eq!(
            doc.blocks[0],
            Block::CodeBlock {
                language: None,
                code: "  50% \\section{x} {".to_string()
            }
        );
        assert_eq!(
            doc.blocks[1],
            Block::CodeBlock {
                language: Some("python".to_string()),
                code: "print(1) # %".to_string()
            }
        );
        assert_eq!(
            doc.blocks[2],
            Block::CodeBlock {
                language: Some("rust".to_string()),
                code: "fn main() {}".to_string()
            }
        );
        let Block::Paragraph(rt) = &doc.blocks[3] else {
            panic!()
        };
        assert_eq!(rt.text, "Inline a%b{ and x.");
        assert!(rt.spans.iter().all(|s| s.style.code));
    }

    #[test]
    fn comments_are_removed() {
        let doc = parse(
            "One % hidden\ntwo 100\\% sure%\nly.\n% whole line\n\n\
             \\begin{comment}\nGone \\section{No}\n\\end{comment}\nThree%\n\nFour",
        );
        assert_eq!(
            paragraphs(&doc.blocks),
            vec!["One two 100% surely.", "Three", "Four"]
        );
        assert!(headings(&doc.blocks).is_empty());
    }

    #[test]
    fn preamble_macros_and_expansion() {
        let doc = parse(
            "\\documentclass{article}\n\\usepackage[utf8]{inputenc}\n\
             \\newcommand{\\R}{\\mathbb{R}}\n\\newcommand\\name{Corvene}\n\
             \\newcommand{\\pair}[2]{(#1, #2)}\n\\renewcommand{\\vec}[1]{\\mathbf{#1}}\n\
             \\DeclareMathOperator*{\\argmax}{arg\\,max}\n\\def\\half{\\frac12}\n\
             \\makeatletter\\def\\@hidden{x}\\makeatother\n\
             \\begin{document}\nUse \\name{} on $\\R$, \\pair{a}{b}.\n\\end{document}",
        );
        assert!(doc.macros.contains("\\newcommand{\\R}{\\mathbb{R}}"));
        assert!(doc.macros.contains("\\newcommand{\\pair}[2]{(#1, #2)}"));
        assert!(
            doc.macros
                .contains("\\DeclareMathOperator*{\\argmax}{arg\\,max}")
        );
        assert!(doc.macros.contains("\\def\\half{\\frac12}"));
        assert!(!doc.macros.contains('@'));
        assert_eq!(doc.macros.lines().count(), 6);
        let Block::Paragraph(rt) = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert_eq!(rt.text, "Use Corvene on , (a, b).");
        assert_eq!(rt.maths[0].tex, "\\R");
        assert_eq!(doc.blocks.len(), 1);
    }

    #[test]
    fn quotes_alignment_abstract_inputs_and_unknown_envs() {
        let doc = parse(
            "\\begin{abstract}We study.\\end{abstract}\n\\begin{quote}Quoted.\\end{quote}\n\
             \\begin{flushright}Right\\end{flushright}\n\\input{chapters/intro}\n\\hrule\n\
             \\begin{minipage}{0.5\\textwidth}Inside\\end{minipage}\n\
             \\begin{mybox}[opt]Boxed\\end{mybox}\n\\begin{empty}\\end{empty}\n\
             \\begin{tikzpicture}\\draw (0,0) -- (1,1);\\end{tikzpicture}",
        );
        assert_eq!(headings(&doc.blocks), vec![(3, "Abstract".to_string())]);
        assert!(matches!(&doc.blocks[2], Block::BlockQuote(q) if paragraphs(q) == vec!["Quoted."]));
        assert!(matches!(
            &doc.blocks[3],
            Block::Aligned {
                align: Align::Right,
                ..
            }
        ));
        let paras = paragraphs(&doc.blocks);
        assert_eq!(
            paras,
            vec![
                "We study.",
                "input: chapters/intro",
                "Inside",
                "mybox Boxed"
            ]
        );
        assert!(doc.blocks.contains(&Block::Rule));
        let Some(Block::CodeBlock { language, code }) = doc.blocks.last() else {
            panic!()
        };
        assert_eq!(language.as_deref(), Some("latex"));
        assert!(code.starts_with("\\begin{tikzpicture}") && code.ends_with("\\end{tikzpicture}"));
    }

    #[test]
    fn malformed_input_never_panics() {
        for input in [
            "",
            "\\",
            "{",
            "}",
            "{{{{",
            "\\begin{itemize}\\item a",
            "\\end{itemize} x",
            "\\begin{",
            "\\begin{document}",
            "$x",
            "$$x",
            "\\[x",
            "\\(",
            "\\verb|x",
            "\\section",
            "\\section{",
            "\\textbf",
            "\\footnote{",
            "\\begin{tabular}{",
            "\\begin{tabular}{ll}a&b\\\\",
            "\\multicolumn{2}",
            "\\'",
            "\\c",
            "\\newcommand{\\a}{\\a}\\a",
            "\\def\\b{\\b\\b}\\b",
            "\\newcommand",
            "\\def",
            "\\let",
            "\\item[",
            "é\\é{é}\\'é",
            "\\begin{equation}\\label{",
            "\\begin{align}a\\\\\\tag{",
            "\\includegraphics[width=",
            "%",
            "\\cite[",
            "\\begin{lstlisting}[language=",
            "\\newtheorem{x}[y]{X}[",
            "\\ref{",
        ] {
            let _ = parse(input);
        }
        let doc = parse("\\begin{itemize}\\item a\n\nText {\\bf bold");
        assert!(matches!(doc.blocks[0], Block::List { .. }));
        let doc = parse(&"{".repeat(500));
        assert!(doc.blocks.len() <= 1);
        let doc = parse(&"\\begin{quote}".repeat(300));
        assert!(doc.blocks.len() <= 1);
    }

    #[test]
    fn big_input_is_fast() {
        let mut text = String::from("\\documentclass{article}\\begin{document}\n");
        let chunk = "\\section{Part}\\label{s}\nSome \\textbf{bold} text with $x^2$ and \
                     \\cite{a} plus \\ref{s}.\n\n\\begin{itemize}\\item one\\item two\\end{itemize}\n\
                     \\begin{align}a&=b\\\\c&=d\\end{align}\n\
                     \\begin{tabular}{lc}x&y\\\\z&w\\end{tabular}\n% comment\n";
        while text.len() < 1 << 20 {
            text.push_str(chunk);
        }
        text.push_str("\\end{document}");
        let started = std::time::Instant::now();
        let doc = parse(&text);
        assert!(doc.blocks.len() > 1000);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        let started = std::time::Instant::now();
        let _ = parse(&"{\\a ".repeat(100_000));
        let _ = parse(&"\\begin{x}".repeat(50_000));
        let _ = parse(&"$ \\[ \\( [".repeat(50_000));
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn plain_text_helper() {
        assert_eq!(
            tex_to_plain("{\\\"O}sterreich -- {\\'E}cole~\\&\\ co"),
            "Österreich – École & co"
        );
        assert_eq!(tex_to_plain("The {GPU} $\\alpha$"), "The GPU \\alpha");
        assert_eq!(compose_accent('d', "a"), "a\u{323}");
    }
}
