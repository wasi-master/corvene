//! The state of a merge as the result is edited.
//!
//! Nothing is stored per chunk beyond its range in the result (kept by
//! [`crate::anchors::Anchors`]): what a chunk holds is read back from the
//! text each time, so undo and hand edits need no bookkeeping. A conflict
//! block in the text is assigned to the conflict chunk whose range it
//! overlaps most; blocks no chunk owns (typed in, or pasted) still count
//! as unresolved.

use std::ops::Range;

use crate::markers::{Block, parse_lenient, render_block};
use crate::text::{same_body, with_eol};
use crate::{ChunkKind, MergeChunk, MergeDocument, Resolution, Side};

/// A replacement in the result text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveChunk {
    pub id: u32,
    /// Bytes in the result now.
    pub range: Range<usize>,
    pub resolution: Resolution,
    /// The conflict block it still is.
    pub block: Option<Block>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveState {
    /// One per document chunk, in order.
    pub chunks: Vec<LiveChunk>,
    /// Blocks no chunk owns.
    pub stray: Vec<Block>,
    /// Conflict blocks left in the text.
    pub unresolved: usize,
}

impl LiveState {
    /// Conflict chunks left, in order.
    pub fn unresolved_chunks(&self) -> impl Iterator<Item = &LiveChunk> {
        self.chunks
            .iter()
            .filter(|c| c.resolution == Resolution::Unresolved)
    }
}

fn overlap(chunk: &Range<usize>, block: &Range<usize>) -> Option<usize> {
    if chunk.is_empty() {
        return (block.start <= chunk.start && chunk.start < block.end).then_some(0);
    }
    let start = chunk.start.max(block.start);
    let end = chunk.end.min(block.end);
    (start < end).then(|| end - start)
}

fn clamp(range: &Range<usize>, text: &str) -> Range<usize> {
    let floor = |mut i: usize| {
        i = i.min(text.len());
        while !text.is_char_boundary(i) {
            i -= 1;
        }
        i
    };
    let start = floor(range.start);
    start..floor(range.end).max(start)
}

/// Reads every chunk back from `text`, given each chunk's current range.
pub fn scan(doc: &MergeDocument, text: &str, ranges: &[Range<usize>]) -> LiveState {
    let blocks = parse_lenient(text, doc.marker_size);
    let ranges: Vec<_> = doc
        .chunks
        .iter()
        .enumerate()
        .map(|(i, c)| clamp(ranges.get(i).unwrap_or(&c.result), text))
        .collect();
    let mut owned: Vec<Option<Block>> = vec![None; doc.chunks.len()];
    let mut stray = Vec::new();
    let unresolved = blocks.len();
    for block in blocks {
        let best = doc
            .chunks
            .iter()
            .enumerate()
            .filter(|(i, c)| c.kind == ChunkKind::Conflict && owned[*i].is_none())
            .filter_map(|(i, _)| overlap(&ranges[i], &block.range).map(|n| (i, n)))
            .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)));
        match best {
            Some((i, _)) => owned[i] = Some(block),
            None => stray.push(block),
        }
    }
    let chunks = doc
        .chunks
        .iter()
        .zip(ranges)
        .zip(owned)
        .map(|((chunk, range), block)| match block {
            Some(block) => LiveChunk {
                id: chunk.id,
                range: range.start.min(block.range.start)..range.end.max(block.range.end),
                resolution: Resolution::Unresolved,
                block: Some(block),
            },
            None => LiveChunk {
                id: chunk.id,
                resolution: infer_resolution(doc, chunk, &text[range.clone()]),
                range,
                block: None,
            },
        })
        .collect();
    LiveState {
        chunks,
        stray,
        unresolved,
    }
}

fn joined(first: &str, second: &str, eol: &str) -> String {
    if first.is_empty() {
        return second.to_owned();
    }
    with_eol(first, eol) + second
}

/// What `content` holds of a chunk.
pub(crate) fn infer_resolution(
    doc: &MergeDocument,
    chunk: &MergeChunk,
    content: &str,
) -> Resolution {
    let ours = doc.side_text(chunk, Side::Ours);
    let theirs = doc.side_text(chunk, Side::Theirs);
    let base = doc.side_text(chunk, Side::Base);
    let eol = doc.eol.as_str();
    match chunk.kind {
        ChunkKind::Conflict => {
            if content == ours {
                Resolution::Ours
            } else if content == theirs {
                Resolution::Theirs
            } else if content.is_empty() {
                Resolution::Ignore
            } else if content == joined(ours, theirs, eol) {
                Resolution::OursThenTheirs
            } else if content == joined(theirs, ours, eol) {
                Resolution::TheirsThenOurs
            } else if doc.has_base && content == base {
                Resolution::Base
            } else if let Some(auto) = chunk.auto.as_ref().filter(|a| a.text == content) {
                auto.resolution
            } else {
                Resolution::Custom
            }
        }
        ChunkKind::OursOnly | ChunkKind::BothSame if content == ours => Resolution::Ours,
        ChunkKind::TheirsOnly if content == theirs => Resolution::Theirs,
        _ if content == base => Resolution::Base,
        _ => Resolution::Custom,
    }
}

/// A chunk's sides as the result should get them: a live block's own
/// bodies unless they are the stage files' lines.
fn sides<'a>(
    doc: &'a MergeDocument,
    chunk: &'a MergeChunk,
    live: Option<(&Block, &'a str)>,
) -> (&'a str, &'a str, &'a str) {
    let ours = doc.side_text(chunk, Side::Ours);
    let theirs = doc.side_text(chunk, Side::Theirs);
    let base = doc.side_text(chunk, Side::Base);
    let Some((block, text)) = live else {
        return (ours, base, theirs);
    };
    let pick = |body: &'a str, side: &'a str| if same_body(body, side) { side } else { body };
    (
        pick(block.ours(text), ours),
        block.base(text).map_or(base, |b| pick(b, base)),
        pick(block.theirs(text), theirs),
    )
}

/// The text a chunk takes for `resolution`; `None` for `Custom`.
pub fn resolution_text(
    doc: &MergeDocument,
    chunk: &MergeChunk,
    block: Option<(&Block, &str)>,
    resolution: Resolution,
) -> Option<String> {
    let (ours, base, theirs) = sides(doc, chunk, block);
    let eol = doc.eol.as_str();
    let conflict = chunk.kind == ChunkKind::Conflict;
    Some(match resolution {
        Resolution::Unresolved => render_conflict(doc, chunk),
        Resolution::Ours => ours.to_owned(),
        Resolution::Theirs => theirs.to_owned(),
        Resolution::OursThenTheirs => joined(ours, theirs, eol),
        Resolution::TheirsThenOurs => joined(theirs, ours, eol),
        Resolution::Base => base.to_owned(),
        Resolution::Ignore if conflict => String::new(),
        Resolution::Ignore => base.to_owned(),
        Resolution::AutoToken | Resolution::AutoWhitespace => chunk.auto.as_ref()?.text.clone(),
        Resolution::Custom => return None,
    })
}

/// The edit that gives a chunk `resolution`.
pub fn resolve(
    doc: &MergeDocument,
    text: &str,
    live: &LiveChunk,
    resolution: Resolution,
) -> Option<Edit> {
    let chunk = doc.chunks.get(live.id as usize)?;
    let block = live.block.as_ref().map(|b| (b, text));
    let new = resolution_text(doc, chunk, block, resolution)?;
    (text.get(live.range.clone())? != new).then(|| Edit {
        range: live.range.clone(),
        text: new,
    })
}

/// A chunk as a conflict block again.
pub fn render_conflict(doc: &MergeDocument, chunk: &MergeChunk) -> String {
    let base = (doc.style.shows_base() && (doc.has_base || chunk.bodies.is_some()))
        .then(|| doc.side_text(chunk, Side::Base));
    render_block(
        doc.side_text(chunk, Side::Ours),
        base,
        doc.side_text(chunk, Side::Theirs),
        &doc.labels,
        doc.marker_size,
        doc.eol,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anchors::Anchors;
    use crate::{Labels, MergeInput, reconcile};

    const BASE: &str = "a\nb\nc\nd\ne\nf\ng\n";
    const OURS: &str = "A\nb\nc\nX\ne\nf\ng\n";
    const THEIRS: &str = "a\nb\nc\nY\ne\nf\nG\n";
    const GIT: &str = "A\nb\nc\n<<<<<<< HEAD\nX\n=======\nY\n>>>>>>> topic\ne\nf\nG\n";

    fn doc() -> MergeDocument {
        reconcile(&MergeInput {
            base: Some(BASE),
            ours: OURS,
            theirs: THEIRS,
            working: GIT,
            marker_size: 7,
            labels: Labels::default(),
            ..MergeInput::default()
        })
    }

    fn apply(text: &mut String, anchors: &mut Anchors, id: u32, edit: &Edit) {
        text.replace_range(edit.range.clone(), &edit.text);
        anchors.edit_chunk(id as usize, edit.range.clone(), edit.text.len());
    }

    #[test]
    fn accepting_each_resolution() {
        let doc = doc();
        for (res, want) in [
            (Resolution::Ours, "X\n"),
            (Resolution::Theirs, "Y\n"),
            (Resolution::OursThenTheirs, "X\nY\n"),
            (Resolution::TheirsThenOurs, "Y\nX\n"),
            (Resolution::Base, "d\n"),
            (Resolution::Ignore, ""),
        ] {
            let mut text = GIT.to_owned();
            let mut anchors = Anchors::new(doc.result_ranges());
            let live = scan(&doc, &text, anchors.ranges());
            assert_eq!(live.unresolved, 1);
            let conflict = live
                .unresolved_chunks()
                .next()
                .expect("one conflict")
                .clone();
            let edit = resolve(&doc, &text, &conflict, res).expect("edit");
            apply(&mut text, &mut anchors, conflict.id, &edit);
            assert_eq!(text, format!("A\nb\nc\n{want}e\nf\nG\n"));
            let live = scan(&doc, &text, anchors.ranges());
            assert_eq!(live.unresolved, 0);
            assert_eq!(live.chunks[conflict.id as usize].resolution, res);
            assert_eq!(live.chunks[4].resolution, Resolution::Theirs, "{res:?}");
        }
    }

    #[test]
    fn undo_brings_the_conflict_back() {
        let doc = doc();
        let mut text = GIT.to_owned();
        let mut anchors = Anchors::new(doc.result_ranges());
        let live = scan(&doc, &text, anchors.ranges());
        let conflict = live.unresolved_chunks().next().expect("conflict").clone();
        let edit = resolve(&doc, &text, &conflict, Resolution::Ours).expect("edit");
        let old = text[edit.range.clone()].to_owned();
        apply(&mut text, &mut anchors, conflict.id, &edit);
        // the editor's undo replays the inverse edit through the same tap
        let undo = edit.range.start..edit.range.start + edit.text.len();
        text.replace_range(undo.clone(), &old);
        anchors.edit(undo.start, undo.end, undo.start + old.len());
        assert_eq!(text, GIT);
        let live = scan(&doc, &text, anchors.ranges());
        assert_eq!(live.unresolved, 1);
        assert_eq!(
            live.chunks[conflict.id as usize].resolution,
            Resolution::Unresolved
        );
    }

    #[test]
    fn reset_and_hand_edits() {
        let doc = doc();
        let mut text = GIT.to_owned();
        let mut anchors = Anchors::new(doc.result_ranges());
        let id = doc.conflicts().next().expect("conflict").id;
        let live = scan(&doc, &text, anchors.ranges());
        let edit =
            resolve(&doc, &text, &live.chunks[id as usize], Resolution::Theirs).expect("edit");
        apply(&mut text, &mut anchors, id, &edit);
        // typing inside the resolved chunk makes it custom
        let at = anchors.ranges()[id as usize].start + 1;
        text.insert_str(at, "yy");
        anchors.edit(at, at, at + 2);
        let live = scan(&doc, &text, anchors.ranges());
        assert_eq!(live.chunks[id as usize].resolution, Resolution::Custom);
        let edit = resolve(
            &doc,
            &text,
            &live.chunks[id as usize],
            Resolution::Unresolved,
        )
        .expect("edit");
        apply(&mut text, &mut anchors, id, &edit);
        assert_eq!(text, GIT);
        assert_eq!(scan(&doc, &text, anchors.ranges()).unresolved, 1);
    }

    #[test]
    fn a_typed_block_is_stray() {
        let doc = doc();
        let text = format!("{GIT}<<<<<<< a\nq\n=======\nr\n>>>>>>> b\n");
        let anchors = Anchors::new(doc.result_ranges());
        let live = scan(&doc, &text, anchors.ranges());
        assert_eq!(live.unresolved, 2);
        assert_eq!(live.stray.len(), 1);
    }
}
