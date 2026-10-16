//! Opening a conflicted file: the computed merge laid over the file git
//! left in the working tree.
//!
//! The diff3 is rendered the way git would have written it, that text is
//! line-diffed against the working file, and each chunk takes the working
//! bytes its rendered lines map to. Every conflict block in the working
//! file then becomes a conflict chunk: the computed chunks it covers are
//! merged into it, and if its sides are not those chunks' lines its own
//! sides are kept ([`crate::Bodies`]). A computed conflict with no block
//! was resolved before the editor opened it (by hand, another tool or
//! rerere) and is classified by what its bytes hold.
//!
//! Markers that cannot be read in full, or a file too large to diff, give a
//! document of conflict blocks alone ([`DocMode::MarkersOnly`]).

use std::ops::Range;

use similar::DiffOp;

use crate::diff3::{Diff3Options, diff_ops};
use crate::live::infer_resolution;
use crate::markers::{self, Block, render_block};
use crate::text::{Interner, Text, same_body};
use crate::{
    AutoResolution, Bodies, ChunkKind, ConflictStyle, DocMode, Eol, Labels, MergeChunk,
    MergeDocument, Resolution, diff3, token, ws,
};

/// Files with more lines than this open with their conflict blocks only.
pub const MAX_LINES: u32 = 50_000;
/// Files larger than this open with their conflict blocks only.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;

/// A conflicted file's versions, as text in the index's line ends.
#[derive(Clone, Debug, Default)]
pub struct MergeInput<'a> {
    /// Stage 1; `None` when both sides added the file.
    pub base: Option<&'a str>,
    /// Stage 2.
    pub ours: &'a str,
    /// Stage 3.
    pub theirs: &'a str,
    /// The working file.
    pub working: &'a str,
    pub style: ConflictStyle,
    pub marker_size: usize,
    /// Fallback labels for rendered blocks; the working file's own win.
    pub labels: Labels,
    pub ignore_whitespace: bool,
}

pub fn reconcile(input: &MergeInput) -> MergeDocument {
    let size = if input.marker_size == 0 {
        markers::DEFAULT_MARKER_SIZE
    } else {
        input.marker_size
    };
    let base = Text::new(input.base.unwrap_or(""));
    let ours = Text::new(input.ours);
    let theirs = Text::new(input.theirs);
    let working = Text::new(input.working);
    let eol = Eol::detect(if input.working.is_empty() {
        input.ours
    } else {
        input.working
    });
    let too_big = [&base, &ours, &theirs, &working]
        .iter()
        .any(|t| t.len() > MAX_BYTES || t.line_count() > MAX_LINES);
    let parsed = markers::parse(input.working, size);
    let blocks = match &parsed {
        Ok(blocks) => blocks.clone(),
        Err(_) => markers::parse_lenient(input.working, size),
    };
    let labels = blocks
        .first()
        .map(|b| b.labels.clone().or(&input.labels))
        .unwrap_or_else(|| input.labels.clone());
    let mut doc = MergeDocument {
        base,
        ours,
        theirs,
        has_base: input.base.is_some(),
        chunks: Vec::new(),
        style: input.style,
        marker_size: size,
        labels,
        eol,
        mode: DocMode::Full,
    };
    if parsed.is_err() || too_big {
        doc.mode = DocMode::MarkersOnly;
        doc.chunks = blocks
            .iter()
            .map(|b| block_chunk(&doc, &working, b, !too_big))
            .collect();
    } else {
        doc.chunks = full_chunks(&doc, &working, &blocks, input.ignore_whitespace);
    }
    for (i, chunk) in doc.chunks.iter_mut().enumerate() {
        chunk.id = i as u32;
    }
    let autos: Vec<_> = doc
        .chunks
        .iter()
        .map(|c| (c.resolution == Resolution::Unresolved).then(|| auto_resolution(&doc, c)))
        .collect();
    for (chunk, auto) in doc.chunks.iter_mut().zip(autos) {
        chunk.auto = auto.flatten();
    }
    doc
}

/// What the magic wand would put in place of a conflict.
pub(crate) fn auto_resolution(doc: &MergeDocument, chunk: &MergeChunk) -> Option<AutoResolution> {
    if chunk.kind != ChunkKind::Conflict {
        return None;
    }
    let base = (doc.has_base || chunk.bodies.as_ref().is_some_and(|b| b.base.is_some()))
        .then(|| doc.side_text(chunk, crate::Side::Base));
    let ours = doc.side_text(chunk, crate::Side::Ours);
    let theirs = doc.side_text(chunk, crate::Side::Theirs);
    if let Some((resolution, text)) = ws::auto_resolve(base.unwrap_or(""), ours, theirs) {
        // Without a base only "the same but for spacing" is safe.
        if base.is_some() || ws::normalize(ours) == ws::normalize(theirs) {
            return Some(AutoResolution { resolution, text });
        }
    }
    let text = token::merge(base?, ours, theirs)?;
    Some(AutoResolution {
        resolution: Resolution::AutoToken,
        text,
    })
}

fn full_chunks(
    doc: &MergeDocument,
    working: &Text,
    blocks: &[Block],
    ignore_whitespace: bool,
) -> Vec<MergeChunk> {
    let options = Diff3Options {
        style: doc.style,
        ignore_whitespace,
    };
    let raw = diff3(&doc.base, &doc.ours, &doc.theirs, &options);

    // The merge as git would have written it, and each chunk's lines in it.
    let mut rendered = String::new();
    let mut spans = Vec::with_capacity(raw.len());
    for c in &raw {
        let start = rendered.len();
        match c.kind {
            ChunkKind::TheirsOnly => rendered.push_str(doc.theirs.slice(c.theirs.clone())),
            ChunkKind::Conflict => rendered.push_str(&render_block(
                doc.ours.slice(c.ours.clone()),
                doc.style
                    .shows_base()
                    .then(|| doc.base.slice(c.base.clone())),
                doc.theirs.slice(c.theirs.clone()),
                &doc.labels,
                doc.marker_size,
                doc.eol,
            )),
            _ => rendered.push_str(doc.ours.slice(c.ours.clone())),
        }
        spans.push(start..rendered.len());
    }
    let rendered = Text::new(rendered);
    let map = LineMap::new(&rendered, working);
    let last = raw.len().saturating_sub(1);
    let mut chunks: Vec<MergeChunk> = raw
        .iter()
        .zip(&spans)
        .enumerate()
        .map(|(i, (c, span))| {
            let start = if i == 0 {
                0
            } else {
                map.map(rendered.line_of_byte(span.start))
            };
            let end = if i == last {
                working.line_count()
            } else {
                map.map(rendered.line_of_byte(span.end))
            };
            MergeChunk {
                id: 0,
                kind: c.kind,
                base: c.base.clone(),
                ours: c.ours.clone(),
                theirs: c.theirs.clone(),
                result: working.byte_range(start..end.max(start)),
                resolution: Resolution::Custom,
                auto: None,
                bodies: None,
            }
        })
        .collect();

    let mut owned = vec![false; chunks.len()];
    for block in blocks {
        place_block(doc, working, block, &mut chunks, &mut owned);
    }
    for (chunk, owned) in chunks.iter_mut().zip(&owned) {
        if *owned {
            chunk.resolution = Resolution::Unresolved;
        } else {
            let content = &working.as_str()[chunk.result.clone()];
            chunk.resolution = infer_resolution(doc, chunk, content);
        }
    }
    chunks
}

fn overlaps(chunk: &Range<usize>, block: &Range<usize>) -> bool {
    if chunk.is_empty() {
        block.start <= chunk.start && chunk.start < block.end
    } else {
        chunk.start < block.end && block.start < chunk.end
    }
}

/// Makes `block` one conflict chunk out of the chunks it covers.
fn place_block(
    doc: &MergeDocument,
    working: &Text,
    block: &Block,
    chunks: &mut Vec<MergeChunk>,
    owned: &mut Vec<bool>,
) {
    let hits: Vec<usize> = (0..chunks.len())
        .filter(|&i| !owned[i] && overlaps(&chunks[i].result, &block.range))
        .collect();
    let (Some(&first), Some(&last)) = (hits.first(), hits.last()) else {
        let chunk = block_chunk(doc, working, block, true);
        let at = chunks.partition_point(|c| c.result.start <= block.range.start);
        chunks.insert(at, chunk);
        owned.insert(at, true);
        return;
    };
    // Unchanged lines on either side of the block stay outside it.
    let (mut first, mut last) = (first, last);
    if chunks[last].kind == ChunkKind::Unchanged && chunks[last].result.end > block.range.end {
        let after = split_unchanged(working, &mut chunks[last], block.range.end, false);
        chunks.insert(last + 1, after);
        owned.insert(last + 1, false);
    }
    if chunks[first].kind == ChunkKind::Unchanged && chunks[first].result.start < block.range.start
    {
        let inside = split_unchanged(working, &mut chunks[first], block.range.start, true);
        chunks.insert(first + 1, inside);
        owned.insert(first + 1, false);
        first += 1;
        last += 1;
    }
    let merged: Vec<MergeChunk> = chunks.drain(first..=last).collect();
    owned.drain(first..=last);
    let any_change = merged.iter().any(|c| c.kind.is_change());
    let span = |f: fn(&MergeChunk) -> &Range<u32>| {
        let start = merged.first().map_or(0, |c| f(c).start);
        let end = merged.last().map_or(start, |c| f(c).end);
        start..end.max(start)
    };
    let mut chunk = MergeChunk {
        id: 0,
        kind: ChunkKind::Conflict,
        base: span(|c| &c.base),
        ours: span(|c| &c.ours),
        theirs: span(|c| &c.theirs),
        result: block.range.clone(),
        resolution: Resolution::Unresolved,
        auto: None,
        bodies: None,
    };
    let text = working.as_str();
    let sides_match = any_change
        && same_body(block.ours(text), doc.ours.slice(chunk.ours.clone()))
        && same_body(block.theirs(text), doc.theirs.slice(chunk.theirs.clone()));
    if !sides_match {
        chunk.bodies = Some(bodies(block, text));
    }
    chunks.insert(first, chunk);
    owned.insert(first, true);
}

/// Cuts an unchanged chunk at a working line start; returns the part
/// after it. The part on the far side of the block (before it when
/// `clean_head`, else after it) holds no markers, so its working lines are
/// its lines on every side.
fn split_unchanged(
    working: &Text,
    chunk: &mut MergeChunk,
    at: usize,
    clean_head: bool,
) -> MergeChunk {
    let start_line = working.line_of_byte(chunk.result.start);
    let end_line = working.line_of_byte(chunk.result.end);
    let at_line = working.line_of_byte(at).clamp(start_line, end_line);
    let cut = |r: &Range<u32>| -> u32 {
        let n = if clean_head {
            at_line - start_line
        } else {
            end_line - at_line
        };
        let n = n.min(r.len() as u32);
        if clean_head { r.start + n } else { r.end - n }
    };
    let (b, o, t) = (cut(&chunk.base), cut(&chunk.ours), cut(&chunk.theirs));
    let at = working.line_start(at_line);
    let mut after = chunk.clone();
    chunk.base.end = b;
    chunk.ours.end = o;
    chunk.theirs.end = t;
    chunk.result.end = at;
    after.base.start = b;
    after.ours.start = o;
    after.theirs.start = t;
    after.result.start = at;
    after
}

fn bodies(block: &Block, text: &str) -> Bodies {
    Bodies {
        ours: block.ours(text).to_owned(),
        base: block.base(text).map(str::to_owned),
        theirs: block.theirs(text).to_owned(),
    }
}

/// A conflict chunk made from a block alone, its side lines found by
/// searching each version when `search`.
fn block_chunk(doc: &MergeDocument, working: &Text, block: &Block, search: bool) -> MergeChunk {
    let text = working.as_str();
    let find = |side: &Text, body: Option<&str>| match body {
        Some(body) if search => find_lines(side, body).unwrap_or(0..0),
        _ => 0..0,
    };
    MergeChunk {
        id: 0,
        kind: ChunkKind::Conflict,
        base: find(&doc.base, block.base(text)),
        ours: find(&doc.ours, Some(block.ours(text))),
        theirs: find(&doc.theirs, Some(block.theirs(text))),
        result: block.range.clone(),
        resolution: Resolution::Unresolved,
        auto: None,
        bodies: Some(bodies(block, text)),
    }
}

/// Where `body`'s lines are in `text`, if they are there whole.
fn find_lines(text: &Text, body: &str) -> Option<Range<u32>> {
    if body.is_empty() {
        return None;
    }
    let trimmed = body
        .strip_suffix("\r\n")
        .or_else(|| body.strip_suffix('\n'))
        .unwrap_or(body);
    for needle in [body, trimmed] {
        let hay = text.as_str();
        let mut from = 0;
        while let Some(off) = hay[from..].find(needle) {
            let start = from + off;
            let end = start + needle.len();
            let start_line = text.line_of_byte(start);
            let at_line_start = text.line_start(start_line) == start;
            let end_line = text.line_of_byte(end);
            let at_line_end = end == hay.len() || text.line_start(end_line) == end;
            if at_line_start && at_line_end {
                return Some(start_line..end_line);
            }
            from = start + 1;
            while !hay.is_char_boundary(from) {
                from += 1;
            }
        }
    }
    None
}

/// Maps line boundaries of one text onto another through their line diff.
struct LineMap {
    /// Matched line pairs, ascending.
    pairs: Vec<(u32, u32)>,
    to_len: u32,
}

impl LineMap {
    fn new(from: &Text, to: &Text) -> Self {
        let mut interner = Interner::default();
        let a = interner.ids(from.lines());
        let b = interner.ids(to.lines());
        let mut pairs = Vec::new();
        for op in diff_ops(&a, &b) {
            if let DiffOp::Equal {
                old_index,
                new_index,
                len,
            } = op
            {
                pairs.extend((0..len).map(|i| ((old_index + i) as u32, (new_index + i) as u32)));
            }
        }
        Self {
            pairs,
            to_len: to.line_count(),
        }
    }

    /// The boundary before line `line` of `from`, in `to`.
    fn map(&self, line: u32) -> u32 {
        let i = self.pairs.partition_point(|&(r, _)| r < line);
        let (lo_r, lo_w) = if i == 0 {
            (0, 0)
        } else {
            let (r, w) = self.pairs[i - 1];
            (r + 1, w + 1)
        };
        let hi_w = self.pairs.get(i).map_or(self.to_len, |&(_, w)| w);
        (lo_w + line.saturating_sub(lo_r)).min(hi_w).max(lo_w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Side;

    fn input<'a>(
        base: &'a str,
        ours: &'a str,
        theirs: &'a str,
        working: &'a str,
    ) -> MergeInput<'a> {
        MergeInput {
            base: Some(base),
            ours,
            theirs,
            working,
            marker_size: 7,
            labels: Labels {
                ours: Some("HEAD".into()),
                theirs: Some("topic".into()),
                ..Labels::default()
            },
            ..MergeInput::default()
        }
    }

    const BASE: &str = "a\nb\nc\nd\ne\nf\ng\n";
    const OURS: &str = "A\nb\nc\nX\ne\nf\ng\n";
    const THEIRS: &str = "a\nb\nc\nY\ne\nf\nG\n";
    const GIT: &str = "A\nb\nc\n<<<<<<< HEAD\nX\n=======\nY\n>>>>>>> topic\ne\nf\nG\n";

    #[test]
    fn pristine_file_matches_every_chunk() {
        let doc = reconcile(&input(BASE, OURS, THEIRS, GIT));
        assert_eq!(doc.mode, DocMode::Full);
        let kinds: Vec<_> = doc.chunks.iter().map(|c| c.kind).collect();
        use ChunkKind::*;
        assert_eq!(
            kinds,
            [OursOnly, Unchanged, Conflict, Unchanged, TheirsOnly]
        );
        let conflict = &doc.chunks[2];
        assert_eq!(conflict.resolution, Resolution::Unresolved);
        assert_eq!(
            &GIT[conflict.result.clone()],
            "<<<<<<< HEAD\nX\n=======\nY\n>>>>>>> topic\n"
        );
        assert!(conflict.bodies.is_none());
        assert_eq!(doc.side_text(conflict, Side::Ours), "X\n");
        assert_eq!(doc.chunks[0].resolution, Resolution::Ours);
        assert_eq!(doc.chunks[4].resolution, Resolution::Theirs);
        assert_eq!(&GIT[doc.chunks[4].result.clone()], "G\n");
    }

    #[test]
    fn a_conflict_resolved_outside_is_classified() {
        let working = "A\nb\nc\nY\ne\nf\nG\n";
        let doc = reconcile(&input(BASE, OURS, THEIRS, working));
        let conflict = doc.conflicts().next().expect("conflict chunk");
        assert_eq!(conflict.resolution, Resolution::Theirs);
        assert_eq!(&working[conflict.result.clone()], "Y\n");
        let edited = "A\nb\nc\nZZ\ne\nf\nG\n";
        let doc = reconcile(&input(BASE, OURS, THEIRS, edited));
        let conflict = doc.conflicts().next().expect("conflict chunk");
        assert_eq!(conflict.resolution, Resolution::Custom);
        assert_eq!(&edited[conflict.result.clone()], "ZZ\n");
    }

    #[test]
    fn a_block_git_made_differently_keeps_its_sides() {
        // git conflicted lines the diff3 here merged cleanly
        let working = "<<<<<<< HEAD\nA\n=======\na\n>>>>>>> topic\nb\nc\nX\ne\nf\nG\n";
        let doc = reconcile(&input(BASE, OURS, THEIRS, working));
        let unresolved: Vec<_> = doc
            .chunks
            .iter()
            .filter(|c| c.resolution == Resolution::Unresolved)
            .collect();
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].ours, 0..1);
        assert_eq!(doc.side_text(unresolved[0], Side::Theirs), "a\n");
    }

    #[test]
    fn a_block_inside_unchanged_lines_is_cut_out() {
        let working = "A\nb\n<<<<<<< HEAD\nc\n=======\nC\n>>>>>>> topic\nX\ne\nf\nG\n";
        let doc = reconcile(&input(BASE, OURS, "a\nb\nc\nX\ne\nf\nG\n", working));
        let block = doc
            .chunks
            .iter()
            .find(|c| c.resolution == Resolution::Unresolved)
            .expect("block chunk");
        assert_eq!(doc.side_text(block, Side::Theirs), "C\n");
        assert_eq!(
            &working[block.result.clone()],
            "<<<<<<< HEAD\nc\n=======\nC\n>>>>>>> topic\n"
        );
        let before = &doc.chunks[doc
            .chunks
            .iter()
            .position(|c| c.id == block.id)
            .expect("id")
            - 1];
        assert_eq!(&working[before.result.clone()], "b\n");
    }

    #[test]
    fn diff3_style_blocks() {
        let mut i = input(BASE, OURS, THEIRS, "");
        i.style = ConflictStyle::Diff3;
        let working =
            "A\nb\nc\n<<<<<<< HEAD\nX\n||||||| base\nd\n=======\nY\n>>>>>>> topic\ne\nf\nG\n";
        i.working = working;
        let doc = reconcile(&i);
        let conflict = doc.conflicts().next().expect("conflict");
        assert_eq!(conflict.resolution, Resolution::Unresolved);
        assert!(conflict.bodies.is_none());
        assert_eq!(doc.labels.base.as_deref(), Some("base"));
    }

    #[test]
    fn add_add_has_an_empty_base() {
        let mut i = input(
            "",
            "x\n",
            "y\n",
            "<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> topic\n",
        );
        i.base = None;
        let doc = reconcile(&i);
        assert!(!doc.has_base);
        assert_eq!(doc.chunks.len(), 1);
        assert_eq!(doc.chunks[0].resolution, Resolution::Unresolved);
    }

    #[test]
    fn malformed_markers_fall_back_to_blocks_only() {
        let working = "<<<<<<< HEAD\nX\n=======\nY\n>>>>>>> topic\n<<<<<<< HEAD\nunclosed\n";
        let doc = reconcile(&input(BASE, OURS, THEIRS, working));
        assert_eq!(doc.mode, DocMode::MarkersOnly);
        assert_eq!(doc.chunks.len(), 1);
        assert_eq!(doc.chunks[0].ours, 3..4);
    }

    #[test]
    fn crlf_working_file() {
        let base = "a\r\nb\r\nc\r\n";
        let ours = "a\r\nX\r\nc\r\n";
        let theirs = "a\r\nY\r\nc\r\n";
        let working = "a\r\n<<<<<<< HEAD\r\nX\r\n=======\r\nY\r\n>>>>>>> topic\r\nc\r\n";
        let doc = reconcile(&input(base, ours, theirs, working));
        assert_eq!(doc.eol, Eol::CrLf);
        let conflict = doc.conflicts().next().expect("conflict");
        assert_eq!(conflict.resolution, Resolution::Unresolved);
        assert!(conflict.bodies.is_none());
    }

    #[test]
    fn auto_resolutions_are_offered() {
        let base = "let total = price * count;\n";
        let ours = "let total = cost * count;\n";
        let theirs = "let total = price * amount;\n";
        let working = format!("<<<<<<< HEAD\n{ours}=======\n{theirs}>>>>>>> topic\n");
        let doc = reconcile(&input(base, ours, theirs, &working));
        let auto = doc.chunks[0].auto.as_ref().expect("auto");
        assert_eq!(auto.resolution, Resolution::AutoToken);
        assert_eq!(auto.text, "let total = cost * amount;\n");
    }
}
