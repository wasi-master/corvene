//! Corvene `1324-document-diff`: two versions of a document side by side.
//! The slides or chapters are paired first (same title, else similar
//! text), then the blocks of a pair (same block, else a block of the same
//! kind whose words mostly match, which is shown as edited).
//!
//! GitHub Desktop has no document diff: Word, PowerPoint, OpenDocument and
//! EPUB files are binary (`app/src/lib/git/diff.ts` `getDiffType`).

use std::hash::{DefaultHasher, Hash, Hasher};

use corvene_models::{Diff, DocBlob, DocBlock, DocSection, ImageBlob};

/// How a slide, chapter or block changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Same,
    Added,
    Deleted,
    Modified,
}

/// One aligned pair: indexes into the previous and current version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pair {
    pub previous: Option<usize>,
    pub current: Option<usize>,
    pub status: Status,
}

/// Blocks compared word by word up to this many bytes of text.
const MAX_COMPARED_TEXT: usize = 4_000;
/// An edited block is looked for this many blocks ahead.
const LOOKAHEAD: usize = 16;
/// Words in common for two blocks to be the same block edited.
const SIMILAR: f32 = 0.5;

/// The slides or chapters of both versions, paired. A text document's one
/// section pairs with the other's.
pub fn section_list(previous: Option<&DocBlob>, current: Option<&DocBlob>) -> Vec<Pair> {
    fn sections(d: Option<&DocBlob>) -> &[DocSection] {
        d.map_or(&[], |d| d.sections.as_slice())
    }
    fn images(d: Option<&DocBlob>) -> &[ImageBlob] {
        d.map_or(&[], |d| d.images.as_slice())
    }
    let (old, new) = (sections(previous), sections(current));
    let old_keys: Vec<u64> = old
        .iter()
        .map(|s| section_key(s, images(previous)))
        .collect();
    let new_keys: Vec<u64> = new
        .iter()
        .map(|s| section_key(s, images(current)))
        .collect();
    let mut pairs = align(&old_keys, &new_keys, |i, j| {
        let (a, b) = (&old[i], &new[j]);
        (!a.name.is_empty() && a.name == b.name) || similar(&section_text(a), &section_text(b))
    });
    // a text document: its body is the same section, changed or not
    if let [a, b] = pairs.as_slice()
        && a.current.is_none()
        && b.previous.is_none()
        && [previous, current]
            .into_iter()
            .flatten()
            .all(|d| d.kind == corvene_models::DocKind::Text)
    {
        pairs = vec![Pair {
            previous: a.previous,
            current: b.current,
            status: Status::Modified,
        }];
    }
    pairs
}

/// The blocks of a section pair, aligned (`previous` / `current` index
/// `old` / `new`).
pub fn block_rows(old: (&[DocBlock], &[ImageBlob]), new: (&[DocBlock], &[ImageBlob])) -> Vec<Pair> {
    let old_keys: Vec<u64> = old.0.iter().map(|b| block_key(b, old.1)).collect();
    let new_keys: Vec<u64> = new.0.iter().map(|b| block_key(b, new.1)).collect();
    align(&old_keys, &new_keys, |i, j| {
        let (a, b) = (&old.0[i], &new.0[j]);
        same_kind(a, b) && similar(&block_text(a), &block_text(b))
    })
}

/// The text of a block, for comparing and for word marks: a heading's or
/// paragraph's runs, a table's cells (tab and line separated), a picture's
/// description.
pub fn block_text(block: &DocBlock) -> String {
    match block {
        DocBlock::Heading { runs, .. } | DocBlock::Paragraph { runs, .. } => {
            DocBlock::runs_text(runs)
        }
        DocBlock::Table { rows } => rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|c| DocBlock::runs_text(c))
                    .collect::<Vec<_>>()
                    .join("\t")
            })
            .collect::<Vec<_>>()
            .join("\n"),
        DocBlock::Image { alt, .. } => alt.clone(),
        DocBlock::Code(text) => text.clone(),
        DocBlock::Break => String::new(),
    }
}

/// The Text view: a line diff of both versions' extracted text, marked
/// `DiffWarnings::document_text`. `Empty` when the text is the same.
pub fn text_diff(previous: Option<&DocBlob>, current: Option<&DocBlob>) -> Diff {
    let old = previous.map(DocBlob::to_text);
    let new = current.map(DocBlob::to_text);
    match crate::pdf::line_diff(old.as_deref(), new.as_deref()) {
        Diff::Text {
            hunks,
            mut warnings,
        } => {
            warnings.document_text = true;
            Diff::Text { hunks, warnings }
        }
        Diff::LargeText {
            hunks,
            mut warnings,
        } => {
            warnings.document_text = true;
            Diff::LargeText { hunks, warnings }
        }
        other => other,
    }
}

/// Pairs equal keys, and within each changed stretch the items `similar`
/// says are one item edited (in order, looking a few items ahead).
fn align(old: &[u64], new: &[u64], similar: impl Fn(usize, usize) -> bool) -> Vec<Pair> {
    let mut out = Vec::with_capacity(old.len().max(new.len()));
    let pair = |previous, current, status| Pair {
        previous,
        current,
        status,
    };
    let ops = similar::capture_diff_slices(similar::Algorithm::Myers, old, new);
    let mut ops = ops.into_iter().peekable();
    while let Some(op) = ops.next() {
        let (tag, o, n) = op.as_tag_tuple();
        if tag == similar::DiffTag::Equal {
            out.extend(o.zip(n).map(|(i, j)| pair(Some(i), Some(j), Status::Same)));
            continue;
        }
        // a change stretch: this op and the changed ones after it
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
            match (j..end).find(|&m| similar(i, news[m])) {
                Some(m) => {
                    out.extend(
                        news[j..m]
                            .iter()
                            .map(|&k| pair(None, Some(k), Status::Added)),
                    );
                    out.push(pair(Some(i), Some(news[m]), Status::Modified));
                    j = m + 1;
                }
                None => out.push(pair(Some(i), None, Status::Deleted)),
            }
        }
        out.extend(
            news[j..]
                .iter()
                .map(|&k| pair(None, Some(k), Status::Added)),
        );
    }
    out
}

fn similar(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let clip = |s: &str| {
        let mut end = s.len().min(MAX_COMPARED_TEXT);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s[..end].to_string()
    };
    let (a, b) = (clip(a), clip(b));
    similar::TextDiff::from_words(&a, &b).ratio() >= SIMILAR
}

fn same_kind(a: &DocBlock, b: &DocBlock) -> bool {
    use DocBlock::*;
    matches!(
        (a, b),
        (Heading { .. }, Heading { .. })
            | (Paragraph { .. }, Paragraph { .. })
            | (Heading { .. }, Paragraph { .. })
            | (Paragraph { .. }, Heading { .. })
            | (Table { .. }, Table { .. })
            | (Image { .. }, Image { .. })
            | (Code(_), Code(_))
    )
}

/// A block's identity: its content, a picture by its bytes (the index
/// differs between versions).
fn block_key(block: &DocBlock, images: &[ImageBlob]) -> u64 {
    let mut h = DefaultHasher::new();
    match block {
        DocBlock::Image { index, alt } => {
            "image".hash(&mut h);
            alt.hash(&mut h);
            index
                .and_then(|i| images.get(i))
                .map(|i| &i.bytes)
                .hash(&mut h);
        }
        // a list item renumbered by an item added or removed above it is
        // the same item
        DocBlock::Paragraph {
            runs,
            list: Some(mark),
            quote,
        } => {
            "item".hash(&mut h);
            runs.hash(&mut h);
            (mark.level, mark.ordered, quote).hash(&mut h);
        }
        other => other.hash(&mut h),
    }
    h.finish()
}

fn section_key(section: &DocSection, images: &[ImageBlob]) -> u64 {
    let mut h = DefaultHasher::new();
    section.name.hash(&mut h);
    section.hidden.hash(&mut h);
    for b in section.blocks.iter().chain(&section.notes) {
        block_key(b, images).hash(&mut h);
    }
    h.finish()
}

fn section_text(section: &DocSection) -> String {
    let mut out = String::new();
    for b in &section.blocks {
        if out.len() > MAX_COMPARED_TEXT {
            break;
        }
        out.push_str(&block_text(b));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use corvene_models::{DocKind, DocRun};

    use super::*;

    fn para(text: &str) -> DocBlock {
        DocBlock::Paragraph {
            runs: vec![DocRun::plain(text)],
            list: None,
            quote: false,
        }
    }

    fn blob(kind: DocKind, sections: Vec<(&str, Vec<DocBlock>)>) -> DocBlob {
        DocBlob {
            kind,
            sections: sections
                .into_iter()
                .map(|(name, blocks)| DocSection {
                    name: name.into(),
                    blocks,
                    ..DocSection::default()
                })
                .collect(),
            images: Vec::new(),
            truncated: false,
            file: Default::default(),
        }
    }

    fn statuses(pairs: &[Pair]) -> Vec<Status> {
        pairs.iter().map(|p| p.status).collect()
    }

    #[test]
    fn edited_blocks_pair_and_others_are_added_or_deleted() {
        let old = [
            para("The quick brown fox jumps over the lazy dog"),
            para("Gone entirely"),
            para("Kept"),
        ];
        let new = [
            para("Brand new"),
            para("The quick brown fox leaps over the lazy dog"),
            para("Kept"),
            para("Tail"),
        ];
        let rows = block_rows((&old, &[]), (&new, &[]));
        use Status::*;
        assert_eq!(statuses(&rows), [Added, Modified, Deleted, Same, Added]);
        assert_eq!(rows[1].previous, Some(0));
        assert_eq!(rows[1].current, Some(1));
    }

    #[test]
    fn renumbered_list_items_stay_the_same() {
        let item = |text: &str, number| DocBlock::Paragraph {
            runs: vec![DocRun::plain(text)],
            list: Some(corvene_models::ListMark {
                level: 0,
                ordered: true,
                number,
            }),
            quote: false,
        };
        let old = [item("a", 1), item("b", 2), item("c", 3)];
        let new = [item("a", 1), item("c", 2)];
        use Status::*;
        assert_eq!(statuses(&block_rows((&old, &[]), (&new, &[]))), [Same, Deleted, Same]);
    }

    #[test]
    fn slides_pair_by_title() {
        let old = blob(
            DocKind::Presentation,
            vec![("Intro", vec![para("a b c")]), ("Plan", vec![para("x")])],
        );
        let new = blob(
            DocKind::Presentation,
            vec![
                ("New", vec![para("zzz")]),
                ("Intro", vec![para("totally different")]),
                ("Plan", vec![para("x")]),
            ],
        );
        use Status::*;
        assert_eq!(
            statuses(&section_list(Some(&old), Some(&new))),
            [Added, Modified, Same]
        );
    }

    #[test]
    fn text_documents_always_pair() {
        let old = blob(DocKind::Text, vec![("", vec![para("one")])]);
        let new = blob(DocKind::Text, vec![("", vec![para("two")])]);
        let list = section_list(Some(&old), Some(&new));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].status, Status::Modified);
        assert_eq!(section_list(None, Some(&new))[0].status, Status::Added);
    }

    #[test]
    fn text_view_marks_its_warning() {
        let old = blob(DocKind::Text, vec![("", vec![para("one")])]);
        let new = blob(DocKind::Text, vec![("", vec![para("two")])]);
        let diff = text_diff(Some(&old), Some(&new));
        assert!(diff.warnings().unwrap().document_text);
        assert_eq!(text_diff(Some(&old), Some(&old)), Diff::Empty);
    }
}
