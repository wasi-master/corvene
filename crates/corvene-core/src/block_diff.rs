//! Two versions of a Markdown (or LaTeX, `crate::latex_doc`) document
//! aligned block by block, for rendered diffs (`1331-notebook-diff`,
//! `1332-latex-diff`): equal blocks pair up, and within a changed stretch a
//! block of the same kind whose words mostly match is the same block
//! edited, with its changed words marked.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;

use crate::markdown::{Block, plain_text};
use crate::notebook_diff::text::word_changes;

/// How a block changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockStatus {
    Same,
    Added,
    Deleted,
    Modified,
}

/// One aligned pair of blocks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockRow {
    pub previous: Option<usize>,
    pub current: Option<usize>,
    pub status: BlockStatus,
    /// Changed words of an edited paragraph or heading, in its text (old,
    /// new); empty for other blocks.
    pub marks: (Vec<Range<usize>>, Vec<Range<usize>>),
}

const LOOKAHEAD: usize = 16;
const SIMILAR: f32 = 0.45;
const MAX_COMPARED: usize = 4_000;

fn key(block: &Block) -> u64 {
    let mut h = DefaultHasher::new();
    format!("{block:?}").hash(&mut h);
    h.finish()
}

fn kind(block: &Block) -> u8 {
    match block {
        Block::Heading { .. } | Block::Paragraph(_) => 0,
        Block::CodeBlock { .. } => 1,
        Block::List { .. } => 2,
        Block::BlockQuote(_) | Block::Alert { .. } => 3,
        Block::Rule => 4,
        Block::Table { .. } => 5,
        Block::Details { .. } | Block::Aligned { .. } => 6,
        Block::Footnotes(_) => 7,
        Block::Math { .. } => 8,
    }
}

fn text_of(block: &Block) -> String {
    let mut t = plain_text(std::slice::from_ref(block));
    if t.len() > MAX_COMPARED {
        let mut end = MAX_COMPARED;
        while !t.is_char_boundary(end) {
            end -= 1;
        }
        t.truncate(end);
    }
    t
}

fn similar_blocks(a: &Block, b: &Block) -> bool {
    if kind(a) != kind(b) {
        return false;
    }
    let (x, y) = (text_of(a), text_of(b));
    if x.trim().is_empty() || y.trim().is_empty() {
        return matches!((a, b), (Block::Rule, Block::Rule));
    }
    similar::TextDiff::from_words(x.as_str(), y.as_str()).ratio() >= SIMILAR
}

/// The marks of an edited paragraph or heading.
fn marks(a: &Block, b: &Block) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let text = |block: &Block| match block {
        Block::Paragraph(t) | Block::Heading { text: t, .. } => Some(t.text.clone()),
        _ => None,
    };
    match (text(a), text(b)) {
        (Some(x), Some(y)) if x.len() <= MAX_COMPARED && y.len() <= MAX_COMPARED => {
            word_changes(&x, &y)
        }
        _ => (Vec::new(), Vec::new()),
    }
}

/// Aligns `old` and `new`.
pub fn block_diff(old: &[Block], new: &[Block]) -> Vec<BlockRow> {
    let ok: Vec<u64> = old.iter().map(key).collect();
    let nk: Vec<u64> = new.iter().map(key).collect();
    let row = |previous, current, status| BlockRow {
        previous,
        current,
        status,
        marks: (Vec::new(), Vec::new()),
    };
    let mut out = Vec::with_capacity(old.len().max(new.len()));
    let ops = similar::capture_diff_slices(similar::Algorithm::Myers, &ok, &nk);
    let mut ops = ops.into_iter().peekable();
    while let Some(op) = ops.next() {
        let (tag, o, n) = op.as_tag_tuple();
        if tag == similar::DiffTag::Equal {
            out.extend(
                o.zip(n)
                    .map(|(i, j)| row(Some(i), Some(j), BlockStatus::Same)),
            );
            continue;
        }
        let (mut olds, mut news): (Vec<usize>, Vec<usize>) = (o.collect(), n.collect());
        while let Some(next) = ops.peek() {
            let (tag, o, n) = next.as_tag_tuple();
            if tag == similar::DiffTag::Equal {
                break;
            }
            olds.extend(o);
            news.extend(n);
            ops.next();
        }
        let mut j = 0;
        for &i in &olds {
            let end = news.len().min(j + LOOKAHEAD);
            match (j..end).find(|&m| similar_blocks(&old[i], &new[news[m]])) {
                Some(m) => {
                    out.extend(
                        news[j..m]
                            .iter()
                            .map(|&k| row(None, Some(k), BlockStatus::Added)),
                    );
                    let mut r = row(Some(i), Some(news[m]), BlockStatus::Modified);
                    r.marks = marks(&old[i], &new[news[m]]);
                    out.push(r);
                    j = m + 1;
                }
                None => out.push(row(Some(i), None, BlockStatus::Deleted)),
            }
        }
        out.extend(
            news[j..]
                .iter()
                .map(|&k| row(None, Some(k), BlockStatus::Added)),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::{ParseOptions, parse_with};

    fn md(s: &str) -> Vec<Block> {
        parse_with(s, ParseOptions { math: true })
    }

    #[test]
    fn edited_paragraph_is_marked() {
        let old = md("# Title\n\nThe quick brown fox jumps.\n\nGone paragraph here.\n");
        let new = md("# Title\n\nThe quick red fox jumps.\n\n$$x^2$$\n");
        let rows = block_diff(&old, &new);
        let statuses: Vec<BlockStatus> = rows.iter().map(|r| r.status).collect();
        assert_eq!(
            statuses,
            vec![
                BlockStatus::Same,
                BlockStatus::Modified,
                BlockStatus::Deleted,
                BlockStatus::Added
            ]
        );
        let Block::Paragraph(t) = &new[1] else {
            panic!()
        };
        assert_eq!(&t.text[rows[1].marks.1[0].clone()], "red");
    }
}
