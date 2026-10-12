//! Line diffs of small texts (a cell's source, a text output, metadata)
//! with the changed words of edited lines marked, as the diff view marks
//! them (`diff_view_rows.rs` `word_changes`, flag 791).

use std::ops::Range;

/// What a line of a [`line_diff`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Deleted,
}

/// One line of a text diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextLine {
    pub kind: LineKind,
    /// 1-based line numbers on each side.
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
    /// Byte ranges of `text` that changed (edited lines only).
    pub marks: Vec<Range<usize>>,
}

/// Texts longer than this are compared by line only (no word marks).
const MAX_MARKED_LINE: usize = 2_000;
/// Lines diffed at most; past it the texts are shown as replaced.
const MAX_LINES: usize = 20_000;
const DIFF_BUDGET: std::time::Duration = std::time::Duration::from_millis(200);

/// Every line of both texts in order (context, deleted and added), with
/// word marks on deleted / added lines that are one line edited.
pub fn line_diff(old: &str, new: &str) -> Vec<TextLine> {
    let old_lines: Vec<&str> = lines(old);
    let new_lines: Vec<&str> = lines(new);
    let mut out = Vec::with_capacity(old_lines.len().max(new_lines.len()));
    if old_lines.len() + new_lines.len() > MAX_LINES {
        push_replaced(&mut out, &old_lines, &new_lines);
        return out;
    }
    let ops = similar::capture_diff_slices_deadline(
        similar::Algorithm::Myers,
        &old_lines,
        &new_lines,
        std::time::Instant::now().checked_add(DIFF_BUDGET),
    );
    let mut deleted: Vec<usize> = Vec::new();
    let mut added: Vec<usize> = Vec::new();
    let flush = |out: &mut Vec<TextLine>, deleted: &mut Vec<usize>, added: &mut Vec<usize>| {
        let first_del = out.len();
        for &i in deleted.iter() {
            out.push(TextLine {
                kind: LineKind::Deleted,
                old: Some(i as u32 + 1),
                new: None,
                text: old_lines[i].to_string(),
                marks: Vec::new(),
            });
        }
        let first_add = out.len();
        for &j in added.iter() {
            out.push(TextLine {
                kind: LineKind::Added,
                old: None,
                new: Some(j as u32 + 1),
                text: new_lines[j].to_string(),
                marks: Vec::new(),
            });
        }
        // pair the k-th deleted with the k-th added line for word marks
        for k in 0..deleted.len().min(added.len()) {
            let (a, b) = (first_del + k, first_add + k);
            let (x, y) = (&out[a].text, &out[b].text);
            if x.len() > MAX_MARKED_LINE || y.len() > MAX_MARKED_LINE {
                continue;
            }
            if similar::TextDiff::from_chars(x.as_str(), y.as_str()).ratio() < 0.35 {
                continue;
            }
            let (ma, mb) = word_changes(x, y);
            out[a].marks = ma;
            out[b].marks = mb;
        }
        deleted.clear();
        added.clear();
    };
    for op in ops {
        let (tag, o, n) = op.as_tag_tuple();
        match tag {
            similar::DiffTag::Equal => {
                flush(&mut out, &mut deleted, &mut added);
                for (i, j) in o.zip(n) {
                    out.push(TextLine {
                        kind: LineKind::Context,
                        old: Some(i as u32 + 1),
                        new: Some(j as u32 + 1),
                        text: old_lines[i].to_string(),
                        marks: Vec::new(),
                    });
                }
            }
            similar::DiffTag::Delete => deleted.extend(o),
            similar::DiffTag::Insert => added.extend(n),
            similar::DiffTag::Replace => {
                deleted.extend(o);
                added.extend(n);
            }
        }
    }
    flush(&mut out, &mut deleted, &mut added);
    out
}

/// All lines of one text as context (an unchanged cell) or as added /
/// deleted (a cell only one side has).
pub fn whole(text: &str, kind: LineKind) -> Vec<TextLine> {
    lines(text)
        .into_iter()
        .enumerate()
        .map(|(i, l)| TextLine {
            kind,
            old: (kind != LineKind::Added).then_some(i as u32 + 1),
            new: (kind != LineKind::Deleted).then_some(i as u32 + 1),
            text: l.to_string(),
            marks: Vec::new(),
        })
        .collect()
}

fn push_replaced(out: &mut Vec<TextLine>, old: &[&str], new: &[&str]) {
    for (i, l) in old.iter().enumerate() {
        out.push(TextLine {
            kind: LineKind::Deleted,
            old: Some(i as u32 + 1),
            new: None,
            text: l.to_string(),
            marks: Vec::new(),
        });
    }
    for (j, l) in new.iter().enumerate() {
        out.push(TextLine {
            kind: LineKind::Added,
            old: None,
            new: Some(j as u32 + 1),
            text: l.to_string(),
            marks: Vec::new(),
        });
    }
}

/// The lines of a text: a trailing newline does not start an empty line,
/// an empty text has no lines.
pub fn lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    let text = text.strip_suffix('\n').unwrap_or(text);
    text.split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect()
}

/// Rows for a side-by-side view: each pairs a deleted line with the added
/// line it was edited into (indexes into `lines`), context lines with
/// themselves.
pub fn split_rows(lines: &[TextLine]) -> Vec<(Option<usize>, Option<usize>)> {
    let mut out = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        match lines[i].kind {
            LineKind::Context => {
                out.push((Some(i), Some(i)));
                i += 1;
            }
            _ => {
                let start = i;
                while i < lines.len() && lines[i].kind == LineKind::Deleted {
                    i += 1;
                }
                let dels: Vec<usize> = (start..i).collect();
                let add_start = i;
                while i < lines.len() && lines[i].kind == LineKind::Added {
                    i += 1;
                }
                let adds: Vec<usize> = (add_start..i).collect();
                for k in 0..dels.len().max(adds.len()) {
                    out.push((dels.get(k).copied(), adds.get(k).copied()));
                }
            }
        }
    }
    out
}

/// A run of lines to show, or `count` unchanged lines folded away.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment {
    Lines(Range<usize>),
    Folded { lines: Range<usize> },
}

/// Folds long runs of context lines, keeping `context` lines next to each
/// change. A text without changes is not folded.
pub fn segments(lines: &[TextLine], context: usize) -> Vec<Segment> {
    let changed: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.kind != LineKind::Context)
        .map(|(i, _)| i)
        .collect();
    if changed.is_empty() {
        return vec![Segment::Lines(0..lines.len())];
    }
    let mut keep = vec![false; lines.len()];
    for &c in &changed {
        let lo = c.saturating_sub(context);
        let hi = (c + context + 1).min(lines.len());
        keep[lo..hi].iter_mut().for_each(|k| *k = true);
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let start = i;
        let k = keep[i];
        while i < lines.len() && keep[i] == k {
            i += 1;
        }
        // a fold of one or two lines is not worth the row
        if k || i - start <= 2 {
            match out.last_mut() {
                Some(Segment::Lines(r)) if r.end == start => r.end = i,
                _ => out.push(Segment::Lines(start..i)),
            }
        } else {
            out.push(Segment::Folded { lines: start..i });
        }
    }
    out
}

/// Byte ranges of the words that differ between two edited lines.
pub fn word_changes(a: &str, b: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let wa = words(a);
    let wb = words(b);
    let ta: Vec<&str> = wa.iter().map(|r| &a[r.clone()]).collect();
    let tb: Vec<&str> = wb.iter().map(|r| &b[r.clone()]).collect();
    let mut ra: Vec<Range<usize>> = Vec::new();
    let mut rb: Vec<Range<usize>> = Vec::new();
    let push = |v: &mut Vec<Range<usize>>, r: Range<usize>| match v.last_mut() {
        Some(last) if last.end == r.start => last.end = r.end,
        _ => v.push(r),
    };
    for op in similar::capture_diff_slices(similar::Algorithm::Myers, &ta, &tb) {
        let (tag, o, n) = op.as_tag_tuple();
        if tag == similar::DiffTag::Equal {
            continue;
        }
        for i in o {
            push(&mut ra, wa[i].clone());
        }
        for j in n {
            push(&mut rb, wb[j].clone());
        }
    }
    // white space alone between marks joins them; marks of only white
    // space are dropped
    let tidy = |text: &str, v: Vec<Range<usize>>| -> Vec<Range<usize>> {
        let mut out: Vec<Range<usize>> = Vec::new();
        for r in v {
            if text[r.clone()].trim().is_empty() {
                continue;
            }
            match out.last_mut() {
                Some(last) if text[last.end..r.start].trim().is_empty() => last.end = r.end,
                _ => out.push(r),
            }
        }
        out
    };
    (tidy(a, ra), tidy(b, rb))
}

/// Word, white space and punctuation tokens.
fn words(s: &str) -> Vec<Range<usize>> {
    #[derive(PartialEq, Clone, Copy)]
    enum Class {
        Word,
        Space,
        Other,
    }
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            Class::Word
        } else if c.is_whitespace() {
            Class::Space
        } else {
            Class::Other
        }
    };
    let mut out = Vec::new();
    let mut start = 0;
    let mut prev: Option<Class> = None;
    for (i, c) in s.char_indices() {
        let k = class(c);
        // punctuation is one token per character
        if prev.is_some() && (Some(k) != prev || k == Class::Other) {
            out.push(start..i);
            start = i;
        }
        prev = Some(k);
    }
    if start < s.len() {
        out.push(start..s.len());
    }
    out
}
