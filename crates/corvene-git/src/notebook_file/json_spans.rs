//! Where each cell of a notebook sits in its file: a single pass over the
//! JSON text (strings and their escapes skipped, nesting tracked without
//! recursion) that finds the elements of the top-level `"cells"` array
//! (nbformat 4) or of `"worksheets"[0]."cells"` (nbformat 3).

/// The cell arrays a notebook's text holds, as byte ranges (`start..end`,
/// `end` exclusive) of their elements. A key that appears twice counts
/// once, the last time (as `serde_json` reads it).
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct CellSpans {
    /// Top-level `"cells"`.
    pub(crate) cells: Option<Vec<(usize, usize)>>,
    /// `"worksheets"[0]."cells"`.
    pub(crate) worksheet_cells: Option<Vec<(usize, usize)>>,
}

struct Scanner<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Scanner<'a> {
    fn err<T>(&self, what: &str) -> Result<T, String> {
        Err(format!("{what} at byte {}", self.i))
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.b.get(self.i) {
            if matches!(c, b' ' | b'\t' | b'\r' | b'\n') {
                self.i += 1;
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn expect(&mut self, c: u8) -> Result<(), String> {
        self.skip_ws();
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            self.err(&format!("expected `{}`", c as char))
        }
    }

    /// Skip a string starting at its opening quote; returns its raw
    /// contents (escapes kept).
    fn string(&mut self) -> Result<&'a [u8], String> {
        if self.peek() != Some(b'"') {
            return self.err("expected a string");
        }
        self.i += 1;
        let start = self.i;
        while let Some(&c) = self.b.get(self.i) {
            match c {
                b'"' => {
                    let raw = &self.b[start..self.i];
                    self.i += 1;
                    return Ok(raw);
                }
                b'\\' => self.i += 2,
                _ => self.i += 1,
            }
        }
        self.err("unterminated string")
    }

    /// A string key, decoded when it holds escapes.
    fn key(&mut self) -> Result<String, String> {
        let raw = self.string()?;
        if !raw.contains(&b'\\') {
            return Ok(String::from_utf8_lossy(raw).into_owned());
        }
        let mut quoted = Vec::with_capacity(raw.len() + 2);
        quoted.push(b'"');
        quoted.extend_from_slice(raw);
        quoted.push(b'"');
        serde_json::from_slice::<String>(&quoted).map_err(|e| e.to_string())
    }

    /// Skip one value of any kind (objects and arrays without recursion).
    fn skip_value(&mut self) -> Result<(), String> {
        self.skip_ws();
        match self.peek() {
            Some(b'"') => self.string().map(|_| ()),
            Some(b'{' | b'[') => {
                let mut depth = 0usize;
                while let Some(c) = self.peek() {
                    match c {
                        b'"' => {
                            self.string()?;
                            continue;
                        }
                        b'{' | b'[' => depth += 1,
                        b'}' | b']' => {
                            depth -= 1;
                            if depth == 0 {
                                self.i += 1;
                                return Ok(());
                            }
                        }
                        _ => {}
                    }
                    self.i += 1;
                }
                self.err("unterminated value")
            }
            Some(_) => {
                let start = self.i;
                while let Some(c) = self.peek() {
                    if matches!(c, b',' | b'}' | b']' | b' ' | b'\t' | b'\r' | b'\n') {
                        break;
                    }
                    self.i += 1;
                }
                if self.i == start {
                    self.err("expected a value")
                } else {
                    Ok(())
                }
            }
            None => self.err("unexpected end"),
        }
    }

    /// Walk the members of the object at the cursor, calling `member` with
    /// each key and the cursor on its value; `member` must consume the
    /// value.
    fn object(
        &mut self,
        mut member: impl FnMut(&mut Self, &str) -> Result<(), String>,
    ) -> Result<(), String> {
        self.expect(b'{')?;
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Ok(());
        }
        loop {
            self.skip_ws();
            let key = self.key()?;
            self.expect(b':')?;
            self.skip_ws();
            member(self, &key)?;
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(());
                }
                _ => return self.err("expected `,` or `}`"),
            }
        }
    }

    /// The byte ranges of the elements of the array at the cursor, or
    /// `None` (the value skipped) when it is not an array.
    fn elements(&mut self) -> Result<Option<Vec<(usize, usize)>>, String> {
        self.skip_ws();
        if self.peek() != Some(b'[') {
            self.skip_value()?;
            return Ok(None);
        }
        self.i += 1;
        let mut out = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            return Ok(Some(out));
        }
        loop {
            self.skip_ws();
            let start = self.i;
            self.skip_value()?;
            out.push((start, self.i));
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(Some(out));
                }
                _ => return self.err("expected `,` or `]`"),
            }
        }
    }
}

/// The cell arrays of a notebook's text.
pub(crate) fn cell_spans(text: &str) -> Result<CellSpans, String> {
    let mut s = Scanner {
        b: text.as_bytes(),
        i: 0,
    };
    let mut spans = CellSpans::default();
    s.object(|s, key| {
        match key {
            "cells" => spans.cells = s.elements()?,
            "worksheets" => spans.worksheet_cells = first_worksheet(s)?,
            _ => s.skip_value()?,
        }
        Ok(())
    })?;
    s.skip_ws();
    if s.i != s.b.len() {
        return s.err("trailing characters");
    }
    Ok(spans)
}

/// `"worksheets"`: the cells of its first element.
fn first_worksheet(s: &mut Scanner<'_>) -> Result<Option<Vec<(usize, usize)>>, String> {
    s.skip_ws();
    if s.peek() != Some(b'[') {
        s.skip_value()?;
        return Ok(None);
    }
    s.i += 1;
    s.skip_ws();
    if s.peek() == Some(b']') {
        s.i += 1;
        return Ok(None);
    }
    let mut cells = None;
    let mut first = true;
    loop {
        s.skip_ws();
        if first && s.peek() == Some(b'{') {
            s.object(|s, key| {
                if key == "cells" {
                    cells = s.elements()?;
                } else {
                    s.skip_value()?;
                }
                Ok(())
            })?;
        } else {
            s.skip_value()?;
        }
        first = false;
        s.skip_ws();
        match s.peek() {
            Some(b',') => s.i += 1,
            Some(b']') => {
                s.i += 1;
                return Ok(cells);
            }
            _ => return s.err("expected `,` or `]`"),
        }
    }
}

/// 1-based line numbers of the first and last byte of each range.
pub(crate) fn line_ranges(text: &str, spans: &[(usize, usize)]) -> Vec<(u32, u32)> {
    let newlines: Vec<usize> = text
        .bytes()
        .enumerate()
        .filter(|(_, b)| *b == b'\n')
        .map(|(i, _)| i)
        .collect();
    let line = |offset: usize| -> u32 {
        // newlines before `offset`, plus one
        let n = newlines.partition_point(|&nl| nl < offset);
        u32::try_from(n + 1).unwrap_or(u32::MAX)
    };
    spans
        .iter()
        .map(|&(start, end)| (line(start), line(end.saturating_sub(1).max(start))))
        .collect()
}
