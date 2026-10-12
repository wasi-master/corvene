//! BibTeX databases for Corvene `1332-latex-diff`: a `.bib` file is shown
//! as its formatted references next to the text diff, entries paired
//! across the two versions (by key, then by content for renamed keys) with
//! their changed fields. GHD shows `.bib` files as text only.
//!
//! [`parse`] is tolerant like BibTeX itself: text outside entries is a
//! comment, `@comment` and `@preamble` are skipped, `@string` macros (and
//! the standard month names) resolve, `#` concatenates, `crossref` fills
//! missing fields from the referenced entry, and a broken entry is dropped
//! without losing the ones after it.

use std::collections::HashMap;

use crate::latex_doc::{push_rt, tex_to_plain};
use crate::markdown::{InlineStyle, RichText};

/// One `@type{key, field = value, …}` entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BibEntry {
    /// Lowercased: `article`, `book`, `inproceedings`, …
    pub kind: String,
    pub key: String,
    /// Lowercased names with their values: outer braces / quotes removed,
    /// `@string` macros and `#` concatenation resolved, white space
    /// collapsed. In file order.
    pub fields: Vec<(String, String)>,
    /// 1-based line of the `@`.
    pub line: u32,
}

impl BibEntry {
    /// A field's value by (lowercase) name.
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// How an entry changed between two versions of a database. Indices are
/// into the old and new entry lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BibChange {
    Same(usize, usize),
    Added(usize),
    Removed(usize),
    /// Edited fields; a renamed key shows as the pseudo field `key`.
    Changed {
        old: usize,
        new: usize,
        fields: Vec<FieldChange>,
    },
}

/// One field of a [`BibChange::Changed`] entry; `None`: absent on that
/// side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldChange {
    pub name: String,
    pub old: Option<String>,
    pub new: Option<String>,
}

const MONTHS: [(&str, &str); 12] = [
    ("jan", "January"),
    ("feb", "February"),
    ("mar", "March"),
    ("apr", "April"),
    ("may", "May"),
    ("jun", "June"),
    ("jul", "July"),
    ("aug", "August"),
    ("sep", "September"),
    ("oct", "October"),
    ("nov", "November"),
    ("dec", "December"),
];

/// Read a BibTeX database. Never fails; broken entries are skipped.
pub fn parse(text: &str) -> Vec<BibEntry> {
    let mut parser = Parser {
        s: text,
        b: text.as_bytes(),
        pos: 0,
        strings: MONTHS
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    };
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let mut entries = Vec::new();
    while let Some(off) = text[parser.pos..].find('@') {
        let at = parser.pos + off;
        parser.pos = at + 1;
        match parser.item() {
            Some(Item::Entry(mut entry)) => {
                entry.line = u32::try_from(line_starts.partition_point(|&s| s <= at)).unwrap_or(0);
                entries.push(entry);
            }
            Some(Item::Skipped) => {}
            None => parser.pos = at + 1,
        }
    }
    resolve_crossrefs(&mut entries);
    entries
}

enum Item {
    Entry(BibEntry),
    Skipped,
}

struct Parser<'a> {
    s: &'a str,
    b: &'a [u8],
    pos: usize,
    strings: HashMap<String, String>,
}

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b':' | b'.' | b'+' | b'/' | b'\'')
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }

    fn ident(&mut self) -> &str {
        let start = self.pos;
        while self.peek().is_some_and(is_ident) {
            self.pos += 1;
        }
        &self.s[start..self.pos]
    }

    /// The matching `}` of the `{` at `open`.
    fn brace_end(&self, open: usize) -> Option<usize> {
        let mut depth = 0usize;
        for (i, &c) in self.b.iter().enumerate().skip(open) {
            match c {
                b'{' => depth += 1,
                b'}' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// After an `@`: an entry, a skipped `@comment` / `@preamble` /
    /// `@string`, or `None` when it is not an entry at all.
    fn item(&mut self) -> Option<Item> {
        self.skip_ws();
        let kind = self.ident().to_ascii_lowercase();
        if kind.is_empty() || !kind.bytes().all(|c| c.is_ascii_alphabetic()) {
            return None;
        }
        self.skip_ws();
        let close = match self.peek()? {
            b'{' => b'}',
            b'(' => b')',
            _ => return None,
        };
        let open = self.pos;
        self.pos += 1;
        match kind.as_str() {
            "comment" | "preamble" => {
                let end = if close == b'}' {
                    self.brace_end(open)?
                } else {
                    open + self.s[open..].find(')')?
                };
                self.pos = end + 1;
                Some(Item::Skipped)
            }
            "string" => {
                self.skip_ws();
                let name = self.ident().to_ascii_lowercase();
                self.skip_ws();
                if name.is_empty() || self.peek()? != b'=' {
                    return None;
                }
                self.pos += 1;
                let value = self.value()?;
                self.skip_ws();
                if self.peek() == Some(close) {
                    self.pos += 1;
                }
                self.strings.insert(name, value);
                Some(Item::Skipped)
            }
            _ => self.entry(kind, close).map(Item::Entry),
        }
    }

    fn entry(&mut self, kind: String, close: u8) -> Option<BibEntry> {
        self.skip_ws();
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|c| c != b',' && c != close && !c.is_ascii_whitespace())
        {
            self.pos += 1;
        }
        let key = self.s[start..self.pos].to_string();
        let mut entry = BibEntry {
            kind,
            key,
            fields: Vec::new(),
            line: 0,
        };
        loop {
            self.skip_ws();
            match self.peek() {
                // a missing closer: the entry ends where the next begins
                None | Some(b'@') => return Some(entry),
                Some(b',') => {
                    self.pos += 1;
                    continue;
                }
                Some(c) if c == close => {
                    self.pos += 1;
                    return Some(entry);
                }
                Some(_) => {}
            }
            let name = self.ident().to_ascii_lowercase();
            if name.is_empty() {
                return None;
            }
            self.skip_ws();
            if self.peek()? != b'=' {
                return None;
            }
            self.pos += 1;
            let value = self.value()?;
            if !entry.fields.iter().any(|(n, _)| *n == name) {
                entry.fields.push((name, value));
            }
        }
    }

    /// `{…}`, `"…"`, a number or a macro, joined by `#`.
    fn value(&mut self) -> Option<String> {
        let mut out = String::new();
        loop {
            self.skip_ws();
            match self.peek()? {
                b'{' => {
                    let end = self.brace_end(self.pos)?;
                    out.push_str(&self.s[self.pos + 1..end]);
                    self.pos = end + 1;
                }
                b'"' => {
                    let start = self.pos + 1;
                    let mut depth = 0usize;
                    let mut i = start;
                    loop {
                        match *self.b.get(i)? {
                            b'{' => depth += 1,
                            b'}' => depth = depth.saturating_sub(1),
                            b'"' if depth == 0 => break,
                            _ => {}
                        }
                        i += 1;
                    }
                    out.push_str(&self.s[start..i]);
                    self.pos = i + 1;
                }
                c if is_ident(c) => {
                    let word = self.ident().to_string();
                    if word.bytes().all(|c| c.is_ascii_digit()) {
                        out.push_str(&word);
                    } else {
                        match self.strings.get(&word.to_ascii_lowercase()) {
                            Some(v) => out.push_str(v),
                            None => out.push_str(&word),
                        }
                    }
                }
                _ => return None,
            }
            self.skip_ws();
            if self.peek() == Some(b'#') {
                self.pos += 1;
            } else {
                break;
            }
        }
        Some(out.split_whitespace().collect::<Vec<_>>().join(" "))
    }
}

/// Fields missing in an entry come from its `crossref` target (whose
/// `title` is the child's `booktitle`).
fn resolve_crossrefs(entries: &mut [BibEntry]) {
    let by_key: HashMap<String, usize> = entries
        .iter()
        .enumerate()
        .rev()
        .map(|(i, e)| (e.key.to_lowercase(), i))
        .collect();
    let mut inherited = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        let Some(target) = entry.field("crossref") else {
            continue;
        };
        let Some(&parent) = by_key.get(&target.trim().to_lowercase()) else {
            continue;
        };
        if parent == i {
            continue;
        }
        let mut add = Vec::new();
        for (name, value) in &entries[parent].fields {
            let name = if name == "title" && entry.field("booktitle").is_none() {
                "booktitle"
            } else {
                name.as_str()
            };
            if name != "crossref" && entry.field(name).is_none() {
                add.push((name.to_string(), value.clone()));
            }
        }
        inherited.push((i, add));
    }
    for (i, add) in inherited {
        if let Some(entry) = entries.get_mut(i) {
            for (name, value) in add {
                if entry.field(&name).is_none() {
                    entry.fields.push((name, value));
                }
            }
        }
    }
}

/// A field value without its TeX: braces go, accents compose (`{\"o}` →
/// ö), `\&` → &, `--` → –, `~` → space.
pub fn display_field(value: &str) -> String {
    tex_to_plain(value)
}

/// `Last, First` / `First Last` → `First Last`; `others` → `et al.`.
fn person(name: &str) -> String {
    let parts: Vec<&str> = split_top(name, ',').into_iter().map(str::trim).collect();
    let shown = match parts.as_slice() {
        [last, jr, first] => format!("{first} {last} {jr}"),
        [last, first] => format!("{first} {last}"),
        _ => name.trim().to_string(),
    };
    display_field(&shown)
}

/// Split at `sep` outside braces.
fn split_top(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            c if c == sep && depth == 0 => {
                out.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// Names of an `author` / `editor` list, split on ` and ` outside braces.
fn people(list: &str) -> Vec<String> {
    let b = list.as_bytes();
    let mut names = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            c if c.is_ascii_whitespace() && depth == 0 => {
                let rest = &b[i + 1..];
                if rest.len() > 4
                    && rest[..3].eq_ignore_ascii_case(b"and")
                    && rest[3].is_ascii_whitespace()
                {
                    names.push(&list[start..i]);
                    i += 4;
                    start = i;
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }
    names.push(&list[start..]);
    names
        .into_iter()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(|n| {
            if n.eq_ignore_ascii_case("others") {
                "et al.".to_string()
            } else {
                person(n)
            }
        })
        .collect()
}

fn join_people(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        _ if names.len() > 4 => format!("{} et al.", names[0]),
        [rest @ .., last] if last == "et al." => format!("{} et al.", rest.join(", ")),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Close a part of the reference with ". " unless it ends in punctuation.
fn end_sentence(rt: &mut RichText) {
    let trimmed = rt.text.trim_end();
    if trimmed.is_empty() {
        return;
    }
    if trimmed.ends_with(['.', '?', '!']) {
        push_rt(rt, " ", InlineStyle::default(), None);
    } else {
        push_rt(rt, ". ", InlineStyle::default(), None);
    }
}

/// A formatted reference, like GitHub's and Zotero's citations: "Ann Lee
/// and Bob Ray (2020). Title. *Journal* 12(3), 45–67. https://doi.org/…".
/// Book-like titles are italic; for articles and chapters the venue is.
pub fn reference(entry: &BibEntry) -> RichText {
    let plain = InlineStyle::default();
    let italic = InlineStyle {
        italic: true,
        ..InlineStyle::default()
    };
    let field = |name: &str| {
        entry
            .field(name)
            .map(display_field)
            .filter(|v| !v.is_empty())
    };
    let mut rt = RichText::default();
    let (names, editors) = match entry.field("author") {
        Some(a) => (people(a), false),
        None => (entry.field("editor").map(people).unwrap_or_default(), true),
    };
    let mut who = join_people(&names);
    if editors && !who.is_empty() {
        who.push_str(if names.len() > 1 { " (eds.)" } else { " (ed.)" });
    }
    push_rt(&mut rt, &who, plain, None);
    let year = field("year").or_else(|| field("date").map(|d| d.chars().take(4).collect()));
    if let Some(year) = year {
        let sep = if rt.text.is_empty() { "" } else { " " };
        push_rt(&mut rt, &format!("{sep}({year})"), plain, None);
    }
    end_sentence(&mut rt);
    let booklike = matches!(
        entry.kind.as_str(),
        "book"
            | "booklet"
            | "manual"
            | "phdthesis"
            | "mastersthesis"
            | "thesis"
            | "techreport"
            | "report"
            | "proceedings"
            | "collection"
            | "mvbook"
            | "unpublished"
    );
    if let Some(title) = field("title") {
        push_rt(&mut rt, &title, if booklike { italic } else { plain }, None);
        end_sentence(&mut rt);
    }
    let venue = ["journal", "journaltitle", "booktitle"]
        .iter()
        .find_map(|n| field(n))
        .or_else(|| {
            [
                "publisher",
                "school",
                "institution",
                "organization",
                "howpublished",
            ]
            .iter()
            .find_map(|n| field(n))
        });
    let mut venue_part = false;
    if let Some(venue) = venue {
        push_rt(&mut rt, &venue, italic, None);
        venue_part = true;
    }
    if let Some(volume) = field("volume") {
        let sep = if venue_part { " " } else { "" };
        push_rt(&mut rt, &format!("{sep}{volume}"), plain, None);
        venue_part = true;
    }
    if let Some(number) = field("number").or_else(|| field("issue")) {
        push_rt(&mut rt, &format!("({number})"), plain, None);
        venue_part = true;
    }
    if let Some(pages) = field("pages") {
        let pages = pages.replace('-', "–").replace("––", "–");
        let sep = if venue_part { ", " } else { "pp. " };
        push_rt(&mut rt, &format!("{sep}{pages}"), plain, None);
        venue_part = true;
    }
    if venue_part {
        end_sentence(&mut rt);
    }
    let link = match entry.field("doi").map(str::trim).filter(|d| !d.is_empty()) {
        Some(doi) => {
            let doi = doi
                .trim_start_matches("https://doi.org/")
                .trim_start_matches("http://dx.doi.org/")
                .trim_start_matches("doi:");
            Some(format!("https://doi.org/{doi}"))
        }
        None => entry
            .field("url")
            .map(|u| u.trim().replace("\\_", "_").replace("\\%", "%"))
            .filter(|u| !u.is_empty()),
    };
    if let Some(link) = link {
        push_rt(&mut rt, &link, plain, Some(&link));
    }
    let len = rt.text.trim_end().len();
    rt.text.truncate(len);
    rt.spans.retain(|s| s.range.start < len);
    if let Some(last) = rt.spans.last_mut() {
        last.range.end = last.range.end.min(len);
    }
    rt
}

/// The share of equal fields, for pairing renamed entries.
fn similarity(a: &BibEntry, b: &BibEntry) -> f64 {
    let most = a.fields.len().max(b.fields.len());
    if most == 0 {
        return 0.0;
    }
    let equal = a
        .fields
        .iter()
        .filter(|(n, v)| b.field(n) == Some(v.as_str()))
        .count();
    equal as f64 / most as f64
}

fn field_changes(a: &BibEntry, b: &BibEntry) -> Vec<FieldChange> {
    let mut changes = Vec::new();
    if a.key != b.key {
        changes.push(FieldChange {
            name: "key".to_string(),
            old: Some(a.key.clone()),
            new: Some(b.key.clone()),
        });
    }
    if a.kind != b.kind {
        changes.push(FieldChange {
            name: "type".to_string(),
            old: Some(a.kind.clone()),
            new: Some(b.kind.clone()),
        });
    }
    for (name, value) in &a.fields {
        match b.field(name) {
            Some(v) if v == value => {}
            other => changes.push(FieldChange {
                name: name.clone(),
                old: Some(value.clone()),
                new: other.map(str::to_string),
            }),
        }
    }
    for (name, value) in &b.fields {
        if a.field(name).is_none() {
            changes.push(FieldChange {
                name: name.clone(),
                old: None,
                new: Some(value.clone()),
            });
        }
    }
    changes
}

/// Pair two versions of a database: by key first, then renamed entries by
/// field similarity (≥ 70% equal fields). The result follows the new file,
/// with removed entries after the entry that followed their old neighbour.
pub fn diff(old: &[BibEntry], new: &[BibEntry]) -> Vec<BibChange> {
    let mut old_by_key: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, e) in old.iter().enumerate().rev() {
        old_by_key.entry(e.key.as_str()).or_default().push(i);
    }
    let mut pair_of_new: Vec<Option<usize>> = vec![None; new.len()];
    let mut old_used = vec![false; old.len()];
    for (j, e) in new.iter().enumerate() {
        if let Some(i) = old_by_key.get_mut(e.key.as_str()).and_then(Vec::pop) {
            pair_of_new[j] = Some(i);
            old_used[i] = true;
        }
    }
    // renamed keys: the most similar unpaired old entry
    for (j, e) in new.iter().enumerate() {
        if pair_of_new[j].is_some() {
            continue;
        }
        let mut best: Option<(usize, f64)> = None;
        for (i, o) in old.iter().enumerate() {
            if old_used[i] {
                continue;
            }
            let score = similarity(o, e);
            if score >= 0.7 && best.is_none_or(|(_, s)| score > s) {
                best = Some((i, score));
            }
        }
        if let Some((i, _)) = best {
            pair_of_new[j] = Some(i);
            old_used[i] = true;
        }
    }
    let mut new_of_old: Vec<Option<usize>> = vec![None; old.len()];
    for (j, i) in pair_of_new.iter().enumerate() {
        if let Some(i) = i {
            new_of_old[*i] = Some(j);
        }
    }
    // removed entries go after the new position of their nearest paired
    // predecessor in the old file
    let mut leading = Vec::new();
    let mut after: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in (0..old.len()).filter(|&i| !old_used[i]) {
        match (0..i).rev().find_map(|p| new_of_old[p]) {
            Some(j) => after.entry(j).or_default().push(i),
            None => leading.push(i),
        }
    }
    let mut out: Vec<BibChange> = leading.into_iter().map(BibChange::Removed).collect();
    for (j, pair) in pair_of_new.iter().enumerate() {
        out.push(match *pair {
            None => BibChange::Added(j),
            Some(i) => {
                let fields = field_changes(&old[i], &new[j]);
                if fields.is_empty() {
                    BibChange::Same(i, j)
                } else {
                    BibChange::Changed {
                        old: i,
                        new: j,
                        fields,
                    }
                }
            }
        });
        if let Some(removed) = after.remove(&j) {
            out.extend(removed.into_iter().map(BibChange::Removed));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
Free text is a comment.
@String{acm = "ACM"}
@string(conf = {Proc. of } # acm)
@comment{ignored @article{nope, title={x}} }
@preamble{"\newcommand{\noop}[1]{}"}

@Article{knuth84,
  author  = {Donald E. Knuth},
  title   = {Literate   {P}rogramming},
  journal = {The Computer Journal},
  year    = 1984,
  month   = may # " 1",
  volume  = "27", number = {2},
  pages   = {97--111},
  doi     = {10.1093/comjnl/27.2.97},
}

@inproceedings(lamport,
  author = "Lamport, Leslie and Doe, J. and others",
  title = "Time, {Clocks}",
  crossref = {proc78},
)

@proceedings{proc78, title = {Conf 78}, year = 1978, publisher = conf}
@book{broken, title = {Never closed
@misc{after, title = "Still found", note = {x}}
"#;

    #[test]
    fn parses_entries_strings_and_crossrefs() {
        let entries = parse(SAMPLE);
        let keys: Vec<&str> = entries.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, vec!["knuth84", "lamport", "proc78", "after"]);
        let knuth = &entries[0];
        assert_eq!(knuth.kind, "article");
        assert_eq!(knuth.line, 8);
        assert_eq!(knuth.field("title"), Some("Literate {P}rogramming"));
        assert_eq!(knuth.field("year"), Some("1984"));
        assert_eq!(knuth.field("month"), Some("May 1"));
        assert_eq!(knuth.field("volume"), Some("27"));
        assert_eq!(knuth.field("number"), Some("2"));
        let lamport = &entries[1];
        assert_eq!(lamport.kind, "inproceedings");
        assert_eq!(lamport.field("booktitle"), Some("Conf 78"));
        assert_eq!(lamport.field("year"), Some("1978"));
        assert_eq!(lamport.field("publisher"), Some("Proc. of ACM"));
        assert_eq!(lamport.field("title"), Some("Time, {Clocks}"));
        assert_eq!(entries[3].field("title"), Some("Still found"));
    }

    #[test]
    fn malformed_input_never_panics() {
        for input in [
            "@",
            "@article",
            "@article{",
            "@article{key",
            "@article{key,",
            "@article{key, title",
            "@article{key, title =",
            "@article{key, title = {",
            "@article{key, title = \"x",
            "@string{",
            "@string{x = }",
            "@comment(",
            "@@@{}{}",
            "}}}{{{",
            "@article{é, tïtle = {ü}}",
        ] {
            let _ = parse(input);
        }
        let entries = parse("@book{a, title={T}\n@book{b, title={U}}");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].field("title"), Some("T"));
    }

    #[test]
    fn display_strips_tex() {
        assert_eq!(
            display_field("{\\\"O}sterreich {\\&} M{\\\"u}ller"),
            "Österreich & Müller"
        );
        assert_eq!(display_field("pp.~1--2 {GPU}s"), "pp. 1–2 GPUs");
        assert_eq!(display_field("Caf\\'{e}"), "Café");
    }

    #[test]
    fn formats_references() {
        let entries = parse(SAMPLE);
        let knuth = reference(&entries[0]);
        assert_eq!(
            knuth.text,
            "Donald E. Knuth (1984). Literate Programming. The Computer Journal 27(2), 97–111. \
             https://doi.org/10.1093/comjnl/27.2.97"
        );
        let journal = knuth.text.find("The Computer").unwrap();
        assert!(
            knuth
                .spans
                .iter()
                .any(|s| s.range.start == journal && s.style.italic)
        );
        let link = knuth.spans.last().unwrap();
        assert_eq!(
            link.link.as_deref(),
            Some("https://doi.org/10.1093/comjnl/27.2.97")
        );
        assert_eq!(
            reference(&entries[1]).text,
            "Leslie Lamport, J. Doe et al. (1978). Time, Clocks. Conf 78."
        );
        let book = parse(
            "@book{b, editor={Ann A and Bob B}, title={A Book}, publisher={Pub}, \
             year=2000, url={https://x.org/a\\_b}}",
        );
        let book = reference(&book[0]);
        assert_eq!(
            book.text,
            "Ann A and Bob B (eds.) (2000). A Book. Pub. https://x.org/a_b"
        );
        assert!(book.spans[0].style.italic);
        let many = parse("@article{m, author={A and B and C and D and E}, title={T?}}");
        assert_eq!(reference(&many[0]).text, "A et al. T?");
    }

    fn entry(key: &str, fields: &[(&str, &str)]) -> BibEntry {
        BibEntry {
            kind: "article".to_string(),
            key: key.to_string(),
            fields: fields
                .iter()
                .map(|(n, v)| (n.to_string(), v.to_string()))
                .collect(),
            line: 1,
        }
    }

    #[test]
    fn diffs_entries() {
        let common = [
            ("author", "A"),
            ("title", "T"),
            ("year", "2000"),
            ("journal", "J"),
        ];
        let old = vec![
            entry("a", &[("title", "A")]),
            entry("gone", &[("title", "G")]),
            entry("b", &[("title", "B"), ("year", "1")]),
            entry("old-name", &common),
            entry("c", &[("title", "C")]),
        ];
        let new = vec![
            entry("c", &[("title", "C")]),
            entry("a", &[("title", "A")]),
            entry("b", &[("title", "B2"), ("note", "n")]),
            entry("new-name", &common),
            entry("fresh", &[("title", "F")]),
        ];
        let changes = diff(&old, &new);
        assert_eq!(
            changes,
            vec![
                BibChange::Same(4, 0),
                BibChange::Same(0, 1),
                BibChange::Removed(1),
                BibChange::Changed {
                    old: 2,
                    new: 2,
                    fields: vec![
                        FieldChange {
                            name: "title".into(),
                            old: Some("B".into()),
                            new: Some("B2".into())
                        },
                        FieldChange {
                            name: "year".into(),
                            old: Some("1".into()),
                            new: None
                        },
                        FieldChange {
                            name: "note".into(),
                            old: None,
                            new: Some("n".into())
                        },
                    ]
                },
                BibChange::Changed {
                    old: 3,
                    new: 3,
                    fields: vec![FieldChange {
                        name: "key".into(),
                        old: Some("old-name".into()),
                        new: Some("new-name".into())
                    }]
                },
                BibChange::Added(4),
            ]
        );
        let first_removed = diff(&[entry("x", &[]), entry("y", &[])], &[entry("y", &[])]);
        assert_eq!(
            first_removed,
            vec![BibChange::Removed(0), BibChange::Same(1, 0)]
        );
    }
}
