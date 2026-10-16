//! git's conflict markers (`xdiff/xmerge.c` `fill_conflict_hunk`).
//!
//! A marker is exactly `size` copies of its character (7 unless the
//! `conflict-marker-size` attribute says otherwise), then a space and a
//! label or the line end. Longer runs are content: git writes the inner
//! conflicts of a virtual merge base with longer markers.

use std::ops::Range;

use crate::text::{strip_eol, with_eol};
use crate::{Eol, Labels, MergeError};

pub const DEFAULT_MARKER_SIZE: usize = 7;

/// A conflict block in a text, as byte ranges into it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// From the start of the `<<<<<<<` line to the end of the `>>>>>>>`
    /// line.
    pub range: Range<usize>,
    pub ours: Range<usize>,
    /// Present in the diff3 and zdiff3 styles.
    pub base: Option<Range<usize>>,
    pub theirs: Range<usize>,
    pub labels: Labels,
}

impl Block {
    pub fn ours<'a>(&self, text: &'a str) -> &'a str {
        &text[self.ours.clone()]
    }

    pub fn theirs<'a>(&self, text: &'a str) -> &'a str {
        &text[self.theirs.clone()]
    }

    pub fn base<'a>(&self, text: &'a str) -> Option<&'a str> {
        self.base.clone().map(|r| &text[r])
    }
}

/// The label after a marker of `ch`, `Some(None)` for a bare marker.
fn marker(line: &str, ch: u8, size: usize) -> Option<Option<&str>> {
    let body = strip_eol(line);
    let bytes = body.as_bytes();
    if bytes.len() < size || !bytes[..size].iter().all(|&b| b == ch) {
        return None;
    }
    let rest = &body[size..];
    if rest.is_empty() {
        Some(None)
    } else if let Some(label) = rest.strip_prefix(' ') {
        Some(Some(label))
    } else {
        None
    }
}

#[derive(Default)]
struct Pending {
    start: usize,
    ours_start: usize,
    ours_end: Option<usize>,
    base_start: Option<usize>,
    base_end: Option<usize>,
    theirs_start: Option<usize>,
    labels: Labels,
}

/// Every conflict block, or an error at the first marker out of place.
pub fn parse(text: &str, size: usize) -> Result<Vec<Block>, MergeError> {
    parse_impl(text, size, true)
}

/// Every well-formed conflict block; stray and unclosed markers are left as
/// content.
pub fn parse_lenient(text: &str, size: usize) -> Vec<Block> {
    parse_impl(text, size, false).unwrap_or_default()
}

fn parse_impl(text: &str, size: usize, strict: bool) -> Result<Vec<Block>, MergeError> {
    let size = size.max(1);
    let mut out = Vec::new();
    let mut pending: Option<Pending> = None;
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let (off, end) = (at, at + line.len());
        at = end;
        let first = line.as_bytes().first().copied();
        if !matches!(first, Some(b'<' | b'|' | b'=' | b'>')) {
            continue;
        }
        if let Some(label) = marker(line, b'<', size) {
            if pending.is_some() && strict {
                return Err(MergeError::Nested(off));
            }
            pending = Some(Pending {
                start: off,
                ours_start: end,
                labels: Labels {
                    ours: label.map(str::to_owned),
                    ..Labels::default()
                },
                ..Pending::default()
            });
            continue;
        }
        let Some(p) = pending.as_mut() else {
            continue;
        };
        let in_ours = p.ours_end.is_none() && p.theirs_start.is_none();
        let in_base = p.base_start.is_some() && p.theirs_start.is_none();
        let in_theirs = p.theirs_start.is_some();
        if let Some(label) = marker(line, b'|', size) {
            if in_ours {
                p.ours_end = Some(off);
                p.base_start = Some(end);
                p.labels.base = label.map(str::to_owned);
            } else if strict {
                return Err(MergeError::Misplaced(off));
            }
        } else if marker(line, b'=', size).is_some() {
            if in_ours {
                p.ours_end = Some(off);
                p.theirs_start = Some(end);
            } else if in_base {
                p.base_end = Some(off);
                p.theirs_start = Some(end);
            } else if strict {
                return Err(MergeError::Misplaced(off));
            }
        } else if let Some(label) = marker(line, b'>', size) {
            if in_theirs {
                if let Some(p) = pending.take() {
                    let theirs_start = p.theirs_start.unwrap_or(off);
                    let mut labels = p.labels;
                    labels.theirs = label.map(str::to_owned);
                    out.push(Block {
                        range: p.start..end,
                        ours: p.ours_start..p.ours_end.unwrap_or(p.ours_start),
                        base: p.base_start.map(|s| s..p.base_end.unwrap_or(s)),
                        theirs: theirs_start..off,
                        labels,
                    });
                }
            } else if strict {
                return Err(MergeError::Misplaced(off));
            }
        }
    }
    match pending {
        Some(p) if strict => Err(MergeError::Unterminated(p.start)),
        _ => Ok(out),
    }
}

/// A conflict block as git writes it. Sides without a final line end get
/// one, as git's `xdl_recs_copy` does with `add_nl`.
pub fn render_block(
    ours: &str,
    base: Option<&str>,
    theirs: &str,
    labels: &Labels,
    size: usize,
    eol: Eol,
) -> String {
    let eol = eol.as_str();
    let line = |ch: char, label: Option<&str>| {
        let mut s: String = std::iter::repeat_n(ch, size.max(1)).collect();
        if let Some(label) = label.filter(|l| !l.is_empty()) {
            s.push(' ');
            s.push_str(label);
        }
        s.push_str(eol);
        s
    };
    let mut out = line('<', labels.ours.as_deref());
    out.push_str(&with_eol(ours, eol));
    if let Some(base) = base {
        out.push_str(&line('|', labels.base.as_deref()));
        out.push_str(&with_eol(base, eol));
    }
    out.push_str(&line('=', None));
    out.push_str(&with_eol(theirs, eol));
    out.push_str(&line('>', labels.theirs.as_deref()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels() -> Labels {
        Labels {
            ours: Some("HEAD".into()),
            base: Some("merged common ancestors".into()),
            theirs: Some("feature/new thing".into()),
        }
    }

    #[test]
    fn round_trips_every_style() {
        for base in [None, Some("b\n")] {
            let block = render_block("o\n", base, "t\n", &labels(), 7, Eol::Lf);
            let text = format!("x\n{block}y\n");
            let blocks = parse(&text, 7).expect("parses");
            assert_eq!(blocks.len(), 1);
            let b = &blocks[0];
            assert_eq!(b.ours(&text), "o\n");
            assert_eq!(b.theirs(&text), "t\n");
            assert_eq!(b.base(&text), base);
            assert_eq!(&text[b.range.clone()], block);
            let mut want = labels();
            if base.is_none() {
                want.base = None;
            }
            assert_eq!(b.labels, want);
        }
    }

    #[test]
    fn custom_marker_size_and_longer_inner_markers() {
        let inner = "<<<<<<<<< inner\na\n=========\nb\n>>>>>>>>> inner\n";
        let text = render_block(inner, None, "t\n", &Labels::default(), 7, Eol::Lf);
        let blocks = parse(&text, 7).expect("parses");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].ours(&text), inner);
        let ten = render_block("o\n", None, "t\n", &Labels::default(), 10, Eol::Lf);
        assert!(ten.starts_with("<<<<<<<<<<\n"));
        assert_eq!(parse(&ten, 10).map(|b| b.len()), Ok(1));
        assert_eq!(parse(&ten, 7).map(|b| b.len()), Ok(0));
    }

    #[test]
    fn crlf_and_missing_final_newline() {
        let text = render_block("o", None, "t", &labels(), 7, Eol::CrLf);
        assert!(text.contains("o\r\n=======\r\nt\r\n>>>>>>> feature/new thing\r\n"));
        let blocks = parse(&text, 7).expect("parses");
        assert_eq!(blocks[0].ours(&text), "o\r\n");
    }

    #[test]
    fn empty_sides() {
        let text = render_block("", None, "t\n", &Labels::default(), 7, Eol::Lf);
        let b = &parse(&text, 7).expect("parses")[0];
        assert_eq!(b.ours(&text), "");
        assert_eq!(b.theirs(&text), "t\n");
    }

    #[test]
    fn malformed_markers() {
        assert_eq!(
            parse("<<<<<<< a\nx\n=======\n", 7),
            Err(MergeError::Unterminated(0))
        );
        assert_eq!(
            parse("<<<<<<< a\n<<<<<<< b\n", 7),
            Err(MergeError::Nested(10))
        );
        assert!(parse("<<<<<<< a\nx\n=======\ny\n=======\n", 7).is_err());
        // stray markers outside a block are content
        assert_eq!(parse("=======\n>>>>>>> x\n", 7), Ok(vec![]));
        let lenient = parse_lenient("<<<<<<< a\nx\n<<<<<<< b\no\n=======\nt\n>>>>>>> c\n", 7);
        assert_eq!(lenient.len(), 1);
        assert_eq!(lenient[0].range.start, 12);
    }
}
