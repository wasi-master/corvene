//! Two versions of a Markdown (or LaTeX, `crate::latex_doc`) document
//! aligned block by block, for rendered diffs (`1331-notebook-diff`,
//! `1332-latex-diff`, `1334-markdown-preview-diff`): equal blocks pair up,
//! and within a changed stretch a block of the same kind whose words mostly
//! match is the same block edited.
//!
//! An edited block is drawn once where it can be ([`merge`]): a paragraph
//! or heading as its new text with the removed words put back before the
//! words that replaced them, a table under the same header as one table
//! with its rows tinted, and a container (block quote, alert, `<details>`,
//! list item, aligned block) as one container whose own blocks are aligned
//! the same way, so an edit inside a note marks only the words that
//! changed. Word diffs run under a deadline, so a long paragraph stays fast.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::time::{Duration, Instant};

use crate::markdown::{Block, InlineImage, InlineMath, ListItem, RichText, plain_text};
use crate::notebook_diff::text::words;

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
    /// An edited block drawn once ([`merge`]); `None` when it is drawn as
    /// its old version over its new one.
    pub merged: Option<Box<MergedViews>>,
}

/// How a block inside a [`Merged`] block changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    Added,
    Deleted,
    /// An edited paragraph, heading or `<details>` summary: the ranges of
    /// its drawn text that were removed and added.
    Words {
        deleted: Vec<Range<usize>>,
        added: Vec<Range<usize>>,
    },
    /// An edited table: how each drawn body row changed (`Same`, `Added`
    /// or `Deleted`) and its changed cells, by column.
    Table(Vec<(BlockStatus, Vec<bool>)>),
}

/// An edited block drawn once: the block, and the blocks in it that
/// changed, each by its path from it (an index into a container's blocks,
/// or into a list's items and then into the item's blocks; an empty path
/// is the block itself).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Merged {
    pub block: Block,
    pub changes: Vec<(Vec<usize>, Change)>,
}

/// Which version a [`Merged`] block shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// The old version, with what was removed marked.
    Old,
    /// The new version, with what was added marked.
    New,
    /// Both at once: what was removed put back next to what was added.
    Both,
}

/// An edited block drawn once, for the unified layout (`both`) and each
/// column of the side-by-side one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergedViews {
    pub both: Merged,
    pub old: Merged,
    pub new: Merged,
}

const LOOKAHEAD: usize = 16;
const SIMILAR: f32 = 0.45;
/// The most text of a block compared to find its edited version.
const MAX_COMPARED: usize = 4_000;
/// How long one word diff may take before it settles for a coarser one.
const WORD_DIFF_DEADLINE: Duration = Duration::from_millis(40);

fn key(block: &Block) -> u64 {
    let mut h = DefaultHasher::new();
    format!("{block:?}").hash(&mut h);
    h.finish()
}

fn item_key(item: &ListItem) -> u64 {
    let mut h = DefaultHasher::new();
    format!("{item:?}").hash(&mut h);
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

fn truncated(mut t: String) -> String {
    if t.len() > MAX_COMPARED {
        let mut end = MAX_COMPARED;
        while !t.is_char_boundary(end) {
            end -= 1;
        }
        t.truncate(end);
    }
    t
}

/// The word tokens of `a` and `b` and the diff between them.
fn word_diff(a: &str, b: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>, Vec<similar::DiffOp>) {
    let (wa, wb) = (words(a), words(b));
    let ta: Vec<&str> = wa.iter().map(|r| &a[r.clone()]).collect();
    let tb: Vec<&str> = wb.iter().map(|r| &b[r.clone()]).collect();
    let ops = similar::capture_diff_slices_deadline(
        similar::Algorithm::Myers,
        &ta,
        &tb,
        Some(Instant::now() + WORD_DIFF_DEADLINE),
    );
    (wa, wb, ops)
}

/// How much of `a` and `b` is the same, by words (0…1).
fn similarity(a: &str, b: &str) -> f32 {
    let (wa, wb, ops) = word_diff(a, b);
    if wa.is_empty() && wb.is_empty() {
        return 1.;
    }
    let same: usize = ops
        .iter()
        .filter(|op| op.tag() == similar::DiffTag::Equal)
        .map(|op| op.old_range().len())
        .sum();
    2. * same as f32 / (wa.len() + wb.len()) as f32
}

fn similar_text(x: &str, y: &str) -> bool {
    similarity(&truncated(x.to_string()), &truncated(y.to_string())) >= SIMILAR
}

fn similar_blocks(a: &Block, b: &Block) -> bool {
    if kind(a) != kind(b) {
        return false;
    }
    // a table under the same header is the same table, however many rows
    // changed
    if table_diff(a, b).is_some() {
        return true;
    }
    let (x, y) = (
        plain_text(std::slice::from_ref(a)),
        plain_text(std::slice::from_ref(b)),
    );
    if x.trim().is_empty() || y.trim().is_empty() {
        return matches!((a, b), (Block::Rule, Block::Rule));
    }
    similar_text(&x, &y)
}

fn similar_items(a: &ListItem, b: &ListItem) -> bool {
    if a.task != b.task {
        return false;
    }
    let (x, y) = (plain_text(&a.blocks), plain_text(&b.blocks));
    if x.trim().is_empty() || y.trim().is_empty() {
        return false;
    }
    similar_text(&x, &y)
}

/// Byte ranges of the words that differ between `a` and `b` (white space
/// alone between two changes joins them).
pub fn word_marks(a: &str, b: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let (wa, wb, ops) = word_diff(a, b);
    let (mut ra, mut rb) = (Vec::new(), Vec::new());
    for op in &ops {
        let (tag, o, n) = op.as_tag_tuple();
        if tag == similar::DiffTag::Equal {
            continue;
        }
        if let Some(r) = token_span(&wa, o) {
            ra.push(r);
        }
        if let Some(r) = token_span(&wb, n) {
            rb.push(r);
        }
    }
    (tidy(a, ra), tidy(b, rb))
}

/// The bytes tokens `range` of `tokens` cover.
fn token_span(tokens: &[Range<usize>], range: Range<usize>) -> Option<Range<usize>> {
    (!range.is_empty()).then(|| tokens[range.start].start..tokens[range.end - 1].end)
}

/// Marks of only white space dropped, and marks with only white space
/// between them joined.
fn tidy(text: &str, marks: Vec<Range<usize>>) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    for r in marks {
        if text[r.clone()].trim().is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if text[last.end..r.start].trim().is_empty() => last.end = r.end,
            _ => out.push(r),
        }
    }
    out
}

/// The marks of an edited paragraph or heading.
fn marks(a: &Block, b: &Block) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let text = |block: &Block| match block {
        Block::Paragraph(t) | Block::Heading { text: t, .. } => Some(t.text.clone()),
        _ => None,
    };
    match (text(a), text(b)) {
        (Some(x), Some(y)) => word_marks(&x, &y),
        _ => (Vec::new(), Vec::new()),
    }
}

/// One row of an edited table: a body row of either version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableRowDiff {
    pub previous: Option<usize>,
    pub current: Option<usize>,
    /// `Same`, `Deleted` or `Added`.
    pub status: BlockStatus,
    /// The cells that differ from the row it replaced (or that replaced
    /// it), by column; empty when it was not paired with one.
    pub cells: Vec<bool>,
}

/// The body rows of two versions of a table, aligned row by row (`None`
/// unless both are tables with the same header): an edited row is its old
/// row deleted, then its new row added, with the changed cells marked.
pub fn table_diff(old: &Block, new: &Block) -> Option<Vec<TableRowDiff>> {
    let (
        Block::Table {
            align: oa,
            head: oh,
            rows: or,
        },
        Block::Table {
            align: na,
            head: nh,
            rows: nr,
        },
    ) = (old, new)
    else {
        return None;
    };
    if oa.len() != na.len() || oh != nh {
        return None;
    }
    let row_key = |cells: &Vec<crate::markdown::RichText>| {
        let mut h = DefaultHasher::new();
        format!("{cells:?}").hash(&mut h);
        h.finish()
    };
    let ok: Vec<u64> = or.iter().map(row_key).collect();
    let nk: Vec<u64> = nr.iter().map(row_key).collect();
    let row = |previous, current, status, cells| TableRowDiff {
        previous,
        current,
        status,
        cells,
    };
    let mut out = Vec::with_capacity(or.len().max(nr.len()));
    for op in similar::capture_diff_slices(similar::Algorithm::Myers, &ok, &nk) {
        let (tag, o, n) = op.as_tag_tuple();
        if tag == similar::DiffTag::Equal {
            out.extend(
                o.zip(n)
                    .map(|(i, j)| row(Some(i), Some(j), BlockStatus::Same, Vec::new())),
            );
            continue;
        }
        let (olds, news): (Vec<usize>, Vec<usize>) = (o.collect(), n.collect());
        let paired = olds.len().min(news.len());
        for (&i, &j) in olds.iter().zip(&news) {
            let cells: Vec<bool> = (0..oa.len().max(1))
                .map(|c| or[i].get(c).map(|t| &t.text) != nr[j].get(c).map(|t| &t.text))
                .collect();
            out.push(row(Some(i), None, BlockStatus::Deleted, cells.clone()));
            out.push(row(None, Some(j), BlockStatus::Added, cells));
        }
        out.extend(
            olds[paired..]
                .iter()
                .map(|&i| row(Some(i), None, BlockStatus::Deleted, Vec::new())),
        );
        out.extend(
            news[paired..]
                .iter()
                .map(|&j| row(None, Some(j), BlockStatus::Added, Vec::new())),
        );
    }
    Some(out)
}

/// `old` and `new` aligned: equal items paired by `key`, and within a
/// changed stretch an item `similar` to one ahead of it paired as edited.
fn align<T>(
    old: &[T],
    new: &[T],
    key: impl Fn(&T) -> u64,
    similar: impl Fn(&T, &T) -> bool,
) -> Vec<(Option<usize>, Option<usize>, BlockStatus)> {
    let ok: Vec<u64> = old.iter().map(&key).collect();
    let nk: Vec<u64> = new.iter().map(&key).collect();
    let mut out = Vec::with_capacity(old.len().max(new.len()));
    let ops = similar::capture_diff_slices(similar::Algorithm::Myers, &ok, &nk);
    let mut ops = ops.into_iter().peekable();
    while let Some(op) = ops.next() {
        let (tag, o, n) = op.as_tag_tuple();
        if tag == similar::DiffTag::Equal {
            out.extend(o.zip(n).map(|(i, j)| (Some(i), Some(j), BlockStatus::Same)));
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
            match (j..end).find(|&m| similar(&old[i], &new[news[m]])) {
                Some(m) => {
                    out.extend(
                        news[j..m]
                            .iter()
                            .map(|&k| (None, Some(k), BlockStatus::Added)),
                    );
                    out.push((Some(i), Some(news[m]), BlockStatus::Modified));
                    j = m + 1;
                }
                None => out.push((Some(i), None, BlockStatus::Deleted)),
            }
        }
        out.extend(
            news[j..]
                .iter()
                .map(|&k| (None, Some(k), BlockStatus::Added)),
        );
    }
    out
}

/// Aligns `old` and `new`.
pub fn block_diff(old: &[Block], new: &[Block]) -> Vec<BlockRow> {
    align(old, new, key, similar_blocks)
        .into_iter()
        .map(|(previous, current, status)| {
            let mut row = BlockRow {
                previous,
                current,
                status,
                marks: (Vec::new(), Vec::new()),
                merged: None,
            };
            if let (BlockStatus::Modified, Some(i), Some(j)) = (status, previous, current) {
                row.marks = marks(&old[i], &new[j]);
                row.merged = merge_views(&old[i], &new[j]).map(Box::new);
            }
            row
        })
        .collect()
}

/// [`merge`] for each [`Side`].
pub fn merge_views(old: &Block, new: &Block) -> Option<MergedViews> {
    Some(MergedViews {
        both: merge(old, new, Side::Both)?,
        old: merge(old, new, Side::Old)?,
        new: merge(old, new, Side::New)?,
    })
}

/// `old` edited into `new` drawn as one block, or `None` when they are
/// not the same kind of block (or a paragraph whose pictures or formulas
/// changed, or only its styling).
pub fn merge(old: &Block, new: &Block, side: Side) -> Option<Merged> {
    let mut changes = Vec::new();
    let block = merge_block(old, new, side, &mut Vec::new(), &mut changes)?;
    Some(Merged { block, changes })
}

fn pick<'a, T>(side: Side, old: &'a T, new: &'a T) -> &'a T {
    if side == Side::Old { old } else { new }
}

type Changes = Vec<(Vec<usize>, Change)>;

/// How each drawn body row of a merged table changed, and its changed
/// cells.
type TableRows = Vec<(BlockStatus, Vec<bool>)>;

fn merge_block(
    old: &Block,
    new: &Block,
    side: Side,
    path: &mut Vec<usize>,
    out: &mut Changes,
) -> Option<Block> {
    let mark = out.len();
    let merged = match (old, new) {
        (Block::Paragraph(a), Block::Paragraph(b)) => {
            let (text, change) = merge_text(a, b, side)?;
            out.push((path.clone(), change));
            Some(Block::Paragraph(text))
        }
        (
            Block::Heading { level, text: a },
            Block::Heading {
                level: level_b,
                text: b,
            },
        ) if level == level_b => {
            let (text, change) = merge_text(a, b, side)?;
            out.push((path.clone(), change));
            Some(Block::Heading {
                level: *level,
                text,
            })
        }
        (Block::Table { .. }, Block::Table { .. }) => {
            let (block, rows) = merge_table(old, new, side)?;
            out.push((path.clone(), Change::Table(rows)));
            Some(block)
        }
        (Block::BlockQuote(a), Block::BlockQuote(b)) => {
            Some(Block::BlockQuote(merge_children(a, b, side, path, out)))
        }
        (
            Block::Alert { kind, blocks: a },
            Block::Alert {
                kind: kb,
                blocks: b,
            },
        ) if kind == kb => Some(Block::Alert {
            kind: *kind,
            blocks: merge_children(a, b, side, path, out),
        }),
        (
            Block::Aligned { align, blocks: a },
            Block::Aligned {
                align: ab,
                blocks: b,
            },
        ) if align == ab => Some(Block::Aligned {
            align: *align,
            blocks: merge_children(a, b, side, path, out),
        }),
        (
            Block::Details {
                open,
                summary: sa,
                blocks: a,
            },
            Block::Details {
                open: open_b,
                summary: sb,
                blocks: b,
            },
        ) => {
            let summary = if sa == sb {
                pick(side, sa, sb).clone()
            } else {
                let (text, change) = merge_text(sa, sb, side)?;
                out.push((path.clone(), change));
                text
            };
            Some(Block::Details {
                open: *pick(side, open, open_b),
                summary,
                blocks: merge_children(a, b, side, path, out),
            })
        }
        (
            Block::List { start, items: a },
            Block::List {
                start: sb,
                items: b,
            },
        ) if start.is_some() == sb.is_some() => Some(Block::List {
            start: *pick(side, start, sb),
            items: merge_items(a, b, side, path, out),
        }),
        _ => None,
    };
    if merged.is_none() {
        out.truncate(mark);
    }
    merged
}

/// A block of one version put in whole, tinted unless the side leaves it
/// out.
fn push_whole(
    blocks: &mut Vec<Block>,
    block: &Block,
    added: bool,
    side: Side,
    path: &mut Vec<usize>,
    out: &mut Changes,
) {
    if (added && side == Side::Old) || (!added && side == Side::New) {
        return;
    }
    path.push(blocks.len());
    out.push((
        path.clone(),
        if added {
            Change::Added
        } else {
            Change::Deleted
        },
    ));
    path.pop();
    blocks.push(block.clone());
}

fn merge_children(
    old: &[Block],
    new: &[Block],
    side: Side,
    path: &mut Vec<usize>,
    out: &mut Changes,
) -> Vec<Block> {
    let mut blocks = Vec::with_capacity(old.len().max(new.len()));
    for (i, j, status) in align(old, new, key, similar_blocks) {
        let (x, y) = (i.map(|i| &old[i]), j.map(|j| &new[j]));
        match (status, x, y) {
            (BlockStatus::Same, Some(x), Some(y)) => blocks.push(pick(side, x, y).clone()),
            (BlockStatus::Modified, Some(x), Some(y)) => {
                path.push(blocks.len());
                let merged = merge_block(x, y, side, path, out);
                path.pop();
                match merged {
                    Some(block) => blocks.push(block),
                    None => {
                        push_whole(&mut blocks, x, false, side, path, out);
                        push_whole(&mut blocks, y, true, side, path, out);
                    }
                }
            }
            (_, Some(x), None) => push_whole(&mut blocks, x, false, side, path, out),
            (_, None, Some(y)) => push_whole(&mut blocks, y, true, side, path, out),
            _ => {}
        }
    }
    blocks
}

fn merge_items(
    old: &[ListItem],
    new: &[ListItem],
    side: Side,
    path: &mut Vec<usize>,
    out: &mut Changes,
) -> Vec<ListItem> {
    let mut items: Vec<ListItem> = Vec::with_capacity(old.len().max(new.len()));
    let mut whole =
        |items: &mut Vec<ListItem>, item: &ListItem, added: bool, path: &mut Vec<usize>| {
            if (added && side == Side::Old) || (!added && side == Side::New) {
                return;
            }
            path.push(items.len());
            out.push((
                path.clone(),
                if added {
                    Change::Added
                } else {
                    Change::Deleted
                },
            ));
            path.pop();
            items.push(item.clone());
        };
    // `whole` borrows `out`, so the edited items' changes are gathered
    // here and appended after
    let mut inner: Changes = Vec::new();
    for (i, j, status) in align(old, new, item_key, similar_items) {
        let (x, y) = (i.map(|i| &old[i]), j.map(|j| &new[j]));
        match (status, x, y) {
            (BlockStatus::Same, Some(x), Some(y)) => items.push(pick(side, x, y).clone()),
            (BlockStatus::Modified, Some(x), Some(y)) => {
                path.push(items.len());
                let blocks = merge_children(&x.blocks, &y.blocks, side, path, &mut inner);
                path.pop();
                items.push(ListItem {
                    task: pick(side, x, y).task,
                    blocks,
                });
            }
            (_, Some(x), None) => whole(&mut items, x, false, path),
            (_, None, Some(y)) => whole(&mut items, y, true, path),
            _ => {}
        }
    }
    out.append(&mut inner);
    items
}

/// An edited text drawn once, with its change.
fn merge_text(a: &RichText, b: &RichText, side: Side) -> Option<(RichText, Change)> {
    // only the styling or a link changed, or a picture or formula: drawn
    // as both versions
    let same_images = |x: &[InlineImage], y: &[InlineImage]| {
        x.len() == y.len()
            && x.iter().zip(y).all(|(p, q)| {
                (&p.src, &p.alt, p.width, p.height) == (&q.src, &q.alt, q.width, q.height)
            })
    };
    let same_maths = |x: &[InlineMath], y: &[InlineMath]| {
        x.len() == y.len()
            && x.iter()
                .zip(y)
                .all(|(p, q)| (&p.tex, p.display) == (&q.tex, q.display))
    };
    if a.text == b.text || !same_images(&a.images, &b.images) || !same_maths(&a.maths, &b.maths) {
        return None;
    }
    Some(match side {
        Side::Old => {
            let (deleted, _) = word_marks(&a.text, &b.text);
            (
                a.clone(),
                Change::Words {
                    deleted,
                    added: Vec::new(),
                },
            )
        }
        Side::New => {
            let (_, added) = word_marks(&a.text, &b.text);
            (
                b.clone(),
                Change::Words {
                    deleted: Vec::new(),
                    added,
                },
            )
        }
        Side::Both => {
            let (text, deleted, added) = merged_text(a, b);
            (text, Change::Words { deleted, added })
        }
    })
}

/// `b` with the words of `a` it dropped put back before the words that
/// replaced them, and the ranges of both.
fn merged_text(a: &RichText, b: &RichText) -> (RichText, Vec<Range<usize>>, Vec<Range<usize>>) {
    let (wa, wb, ops) = word_diff(&a.text, &b.text);
    let mut out = RichText::default();
    let (mut deleted, mut added) = (Vec::new(), Vec::new());
    // (position in `b`, bytes of `a` put in before it so far)
    let mut shifts: Vec<(usize, usize)> = Vec::new();
    let mut put_in = 0;
    let copy = |out: &mut RichText, from: &RichText, range: Range<usize>| {
        let piece = from.slice(range);
        let at = out.text.len();
        out.text.push_str(&piece.text);
        out.spans.extend(piece.spans.into_iter().map(|mut s| {
            s.range = s.range.start + at..s.range.end + at;
            s
        }));
        at..out.text.len()
    };
    let b_at = |token: usize| wb.get(token).map_or(b.text.len(), |r| r.start);
    for op in &ops {
        let (tag, o, n) = op.as_tag_tuple();
        let at = b_at(n.start);
        let old = token_span(&wa, o);
        let new = token_span(&wb, n);
        if tag == similar::DiffTag::Equal {
            if let Some(r) = new {
                copy(&mut out, b, r);
            }
            continue;
        }
        if let Some(r) = old {
            let r = copy(&mut out, a, r);
            put_in += r.len();
            shifts.push((at, put_in));
            deleted.push(r);
        }
        if let Some(r) = new {
            added.push(copy(&mut out, b, r));
        }
    }
    // pictures and formulas (the same in both) stay where `b` has them
    let shift = |at: usize| {
        shifts
            .iter()
            .take_while(|(p, _)| *p <= at)
            .last()
            .map_or(0, |(_, s)| *s)
    };
    out.images = b
        .images
        .iter()
        .map(|i| InlineImage {
            at: i.at + shift(i.at),
            ..i.clone()
        })
        .collect();
    out.maths = b
        .maths
        .iter()
        .map(|m| InlineMath {
            at: m.at + shift(m.at),
            ..m.clone()
        })
        .collect();
    let deleted = tidy(&out.text, deleted);
    let added = tidy(&out.text, added);
    (out, deleted, added)
}

/// An edited table under the same header as one table: both versions'
/// body rows merged ([`Side::Both`]: the old row of an edited pair before
/// its new one), or one version's.
fn merge_table(old: &Block, new: &Block, side: Side) -> Option<(Block, TableRows)> {
    let diff = table_diff(old, new)?;
    let (
        Block::Table {
            align,
            head,
            rows: old_rows,
        },
        Block::Table { rows: new_rows, .. },
    ) = (old, new)
    else {
        return None;
    };
    let mut rows = Vec::with_capacity(diff.len());
    let mut tints = Vec::with_capacity(diff.len());
    for r in &diff {
        let previous = r.previous.and_then(|i| old_rows.get(i));
        let current = r.current.and_then(|j| new_rows.get(j));
        let cells = match side {
            Side::Both => current.or(previous),
            Side::Old => previous,
            Side::New => current,
        };
        let Some(cells) = cells else {
            continue;
        };
        rows.push(cells.clone());
        tints.push((r.status, r.cells.clone()));
    }
    let table = Block::Table {
        align: align.clone(),
        head: head.clone(),
        rows,
    };
    Some((table, tints))
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

    #[test]
    fn table_rows_are_aligned() {
        let old = md("| A | B |\n|---|---|\n| 1 | one |\n| 2 | two |\n| 3 | three |\n");
        let new =
            md("| A | B |\n|---|---|\n| 1 | one |\n| 2 | TWO |\n| 3 | three |\n| 4 | four |\n");
        let rows = block_diff(&old, &new);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, BlockStatus::Modified);
        let table = table_diff(&old[0], &new[0]).unwrap();
        let shape: Vec<_> = table
            .iter()
            .map(|r| (r.previous, r.current, r.status))
            .collect();
        assert_eq!(
            shape,
            vec![
                (Some(0), Some(0), BlockStatus::Same),
                (Some(1), None, BlockStatus::Deleted),
                (None, Some(1), BlockStatus::Added),
                (Some(2), Some(2), BlockStatus::Same),
                (None, Some(3), BlockStatus::Added),
            ]
        );
        assert_eq!(table[1].cells, vec![false, true]);
        assert!(table[4].cells.is_empty());
    }

    #[test]
    fn retitled_table_is_not_aligned() {
        let old = md("| A | B |\n|---|---|\n| 1 | 2 |\n");
        let new = md("| A | C |\n|---|---|\n| 1 | 2 |\n");
        assert!(table_diff(&old[0], &new[0]).is_none());
    }

    fn words_of(merged: &Merged) -> (&Block, Vec<String>, Vec<String>) {
        let (path, Change::Words { deleted, added }) = &merged.changes[0] else {
            panic!("{:?}", merged.changes)
        };
        let mut block = &merged.block;
        for &i in path {
            block = match block {
                Block::BlockQuote(b) | Block::Alert { blocks: b, .. } => &b[i],
                Block::Details { blocks: b, .. } => &b[i],
                _ => panic!(),
            };
        }
        let text = match block {
            Block::Paragraph(t) | Block::Heading { text: t, .. } => t.text.clone(),
            Block::Details { summary, .. } => summary.text.clone(),
            _ => panic!("{block:?}"),
        };
        let cut = |v: &Vec<Range<usize>>| v.iter().map(|r| text[r.clone()].to_string()).collect();
        (block, cut(deleted), cut(added))
    }

    #[test]
    fn edited_paragraph_is_drawn_once() {
        let old = md("The quick brown fox jumps.\n");
        let new = md("The quick red fox jumps.\n");
        let rows = block_diff(&old, &new);
        let views = rows[0].merged.as_deref().unwrap();
        let (block, deleted, added) = words_of(&views.both);
        assert_eq!(deleted, vec!["brown"]);
        assert_eq!(added, vec!["red"]);
        let Block::Paragraph(t) = block else { panic!() };
        assert_eq!(t.text, "The quick brownred fox jumps.");
        // each column keeps its own text and marks
        let (_, deleted, added) = words_of(&views.old);
        assert_eq!((deleted, added.len()), (vec!["brown".to_string()], 0));
        let (block, deleted, added) = words_of(&views.new);
        assert_eq!((deleted.len(), added), (0, vec!["red".to_string()]));
        assert_eq!(block, &new[0]);
    }

    #[test]
    fn merged_text_keeps_styles_and_pictures() {
        let old = md("A sink ![pic](a.png) works.\n");
        let new = md("A **kitchen** sink ![pic](a.png) works well.\n");
        let merged = merge(&old[0], &new[0], Side::Both).unwrap();
        let Block::Paragraph(t) = &merged.block else {
            panic!()
        };
        let bold = t.spans.iter().find(|s| s.style.bold).unwrap();
        assert_eq!(&t.text[bold.range.clone()], "kitchen");
        let Block::Paragraph(n) = &new[0] else {
            panic!()
        };
        assert_eq!(t.images[0].at, n.images[0].at);
        // a picture that changed is drawn as both versions
        let other = md("A sink ![pic](b.png) works.\n");
        assert!(merge(&old[0], &other[0], Side::Both).is_none());
    }

    #[test]
    fn edited_alert_marks_only_its_words() {
        let old = md("> [!NOTE]\n> Notes render as GitHub alerts.\n>\n> Second paragraph.\n");
        let new = md(
            "> [!NOTE]\n> Notes render as GitHub alerts, with an icon.\n>\n> Second paragraph.\n>\n> Added one.\n",
        );
        let rows = block_diff(&old, &new);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, BlockStatus::Modified);
        let both = &rows[0].merged.as_deref().unwrap().both;
        let Block::Alert { kind, blocks } = &both.block else {
            panic!("{:?}", both.block)
        };
        assert_eq!(*kind, crate::markdown::AlertKind::Note);
        assert_eq!(blocks.len(), 3);
        let (_, deleted, added) = words_of(both);
        assert!(deleted.is_empty());
        assert_eq!(added, vec![", with an icon"]);
        assert_eq!(both.changes[1], (vec![2], Change::Added));
        // another kind is another alert
        let tip = md("> [!TIP]\n> Notes render as GitHub alerts.\n>\n> Second paragraph.\n");
        assert!(merge(&old[0], &tip[0], Side::Both).is_none());
    }

    #[test]
    fn edited_quote_and_details_recurse() {
        let old = md("> One two three four.\n>\n> Kept.\n");
        let new = md("> One two five four.\n>\n> Kept.\n");
        let both = merge(&old[0], &new[0], Side::Both).unwrap();
        let (_, deleted, added) = words_of(&both);
        assert_eq!(
            (deleted, added),
            (vec!["three".into()], vec!["five".into()])
        );
        assert_eq!(both.changes.len(), 1);

        let old =
            md("<details>\n<summary>More info</summary>\n\nHidden text here.\n\n</details>\n");
        let new =
            md("<details>\n<summary>More info</summary>\n\nHidden text there.\n\n</details>\n");
        let both = merge(&old[0], &new[0], Side::Both).unwrap();
        assert_eq!(both.changes.len(), 1, "{:?}", both.changes);
        assert_eq!(both.changes[0].0, vec![0]);
    }

    #[test]
    fn edited_list_item_recurses() {
        let old =
            md("- Item one\n\n  Nested text stays.\n\n  More nested words here.\n- [ ] Task\n");
        let new =
            md("- Item one\n\n  Nested text stays.\n\n  More nested words there.\n- [x] Task\n");
        let both = merge(&old[0], &new[0], Side::Both).unwrap();
        let Block::List { items, .. } = &both.block else {
            panic!()
        };
        // the ticked task is its old item removed and its new one added
        assert_eq!(items.len(), 3);
        let paths: Vec<(Vec<usize>, bool)> = both
            .changes
            .iter()
            .map(|(p, c)| (p.clone(), matches!(c, Change::Words { .. })))
            .collect();
        assert!(paths.contains(&(vec![0, 2], true)), "{paths:?}");
        assert!(paths.contains(&(vec![1], false)));
        assert!(paths.contains(&(vec![2], false)));
    }

    #[test]
    fn long_paragraph_marks_only_the_edit() {
        let lines = |edited: bool| -> String {
            (1..=2_000)
                .map(|n| {
                    if edited && n == 1_470 {
                        format!("Paragraph {n} (edited far down): Lorem ipsum dolor sit amet.\n")
                    } else {
                        format!("Paragraph {n}: Lorem ipsum dolor sit amet.\n")
                    }
                })
                .collect()
        };
        let (old, new) = (md(&lines(false)), md(&lines(true)));
        let started = Instant::now();
        let rows = block_diff(&old, &new);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, BlockStatus::Modified);
        assert_eq!(rows[0].marks.1.len(), 1);
        let (_, deleted, added) = words_of(&rows[0].merged.as_deref().unwrap().both);
        assert!(deleted.is_empty());
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].trim(), "(edited far down)");
    }

    #[test]
    fn edited_table_is_merged() {
        let old = md("| A | B |\n|---|---|\n| 1 | one |\n");
        let new = md("| A | B |\n|---|---|\n| 1 | uno |\n");
        let both = merge(&old[0], &new[0], Side::Both).unwrap();
        let Block::Table { rows, .. } = &both.block else {
            panic!()
        };
        assert_eq!(rows.len(), 2);
        assert_eq!(
            both.changes,
            vec![(
                Vec::new(),
                Change::Table(vec![
                    (BlockStatus::Deleted, vec![false, true]),
                    (BlockStatus::Added, vec![false, true])
                ])
            )]
        );
    }
}
