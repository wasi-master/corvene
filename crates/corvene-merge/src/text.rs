use std::collections::HashMap;
use std::ops::Range;

/// A version of a file with its line starts. A line keeps its terminator;
/// the last one may have none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Text {
    text: String,
    starts: Vec<usize>,
}

impl Text {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let starts = line_starts(&text);
        Self { text, starts }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn line_count(&self) -> u32 {
        self.starts.len() as u32
    }

    /// Byte where `line` starts; the text's length past the last line.
    pub fn line_start(&self, line: u32) -> usize {
        self.starts
            .get(line as usize)
            .copied()
            .unwrap_or(self.text.len())
    }

    pub fn byte_range(&self, lines: Range<u32>) -> Range<usize> {
        self.line_start(lines.start)..self.line_start(lines.end.max(lines.start))
    }

    pub fn slice(&self, lines: Range<u32>) -> &str {
        &self.text[self.byte_range(lines)]
    }

    pub fn line(&self, line: u32) -> &str {
        self.slice(line..line + 1)
    }

    pub fn lines(&self) -> impl Iterator<Item = &str> {
        self.text.split_inclusive('\n')
    }

    /// The line holding `byte` (the line count at the end).
    pub fn line_of_byte(&self, byte: usize) -> u32 {
        if byte >= self.text.len() {
            return self.line_count();
        }
        (self.starts.partition_point(|&s| s <= byte) as u32).saturating_sub(1)
    }
}

pub(crate) fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        starts.push(at);
        at += line.len();
    }
    starts
}

/// A line without its terminator.
pub(crate) fn strip_eol(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

/// `text` ending in a line end (unless empty).
pub(crate) fn with_eol(text: &str, eol: &str) -> String {
    let mut out = text.to_owned();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push_str(eol);
    }
    out
}

/// Whether two sides are the same text, one perhaps given a line end git
/// added before a marker.
pub(crate) fn same_body(body: &str, side: &str) -> bool {
    body == side
        || (!side.ends_with('\n')
            && body.len() > side.len()
            && body.starts_with(side)
            && matches!(&body[side.len()..], "\n" | "\r\n"))
}

/// Gives equal items equal ids so the diffs compare numbers.
#[derive(Default)]
pub(crate) struct Interner {
    ids: HashMap<String, u32>,
}

impl Interner {
    pub(crate) fn id(&mut self, key: &str) -> u32 {
        if let Some(&id) = self.ids.get(key) {
            return id;
        }
        let id = self.ids.len() as u32;
        self.ids.insert(key.to_owned(), id);
        id
    }

    pub(crate) fn ids<'a>(&mut self, items: impl Iterator<Item = &'a str>) -> Vec<u32> {
        items.map(|item| self.id(item)).collect()
    }
}
