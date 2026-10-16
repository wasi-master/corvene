//! A line diff3 shaped like git's `xdl_merge` (`xdiff/xmerge.c`).
//!
//! A base line that both sides kept splits the merge; a stretch between two
//! such lines that both sides changed is a conflict, so changes to adjacent
//! base lines conflict as in git. Identical changes merge (`XDL_MERGE_EAGER`).
//! In the merge style a conflict is refined the way `xdl_refine_conflicts`
//! does at `XDL_MERGE_ZEALOUS`, which `ll_xdl_merge` asks for: the two sides
//! are diffed and the lines they share leave the conflict. diff3 shows the
//! base, so git stays at `XDL_MERGE_EAGER` there; zdiff3 only moves a shared
//! prefix and suffix out (`xdl_refine_zdiff3_conflicts`).
//!
//! The line diff is Myers from `similar`, as git's default; where the two
//! pick different but equally short scripts the chunks can differ from
//! git's, which is why the working file's markers stay the truth
//! (`reconcile`).

use std::ops::Range;
use std::time::{Duration, Instant};

use similar::{Algorithm, DiffOp};

use crate::text::{Interner, Text};
use crate::{ChunkKind, ConflictStyle, ws};

/// Gives up on an optimal line diff after this long (similar's deadline).
const DEADLINE: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Diff3Options {
    pub style: ConflictStyle,
    /// Lines that differ only in whitespace count as equal.
    pub ignore_whitespace: bool,
}

/// A stretch of a three-way merge, in lines of each version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diff3Chunk {
    pub kind: ChunkKind,
    pub base: Range<u32>,
    pub ours: Range<u32>,
    pub theirs: Range<u32>,
}

/// Splits a merge of three versions into chunks covering every line of
/// each.
pub fn diff3(base: &Text, ours: &Text, theirs: &Text, options: &Diff3Options) -> Vec<Diff3Chunk> {
    let mut interner = Interner::default();
    let mut ids = |text: &Text| -> Vec<u32> {
        if options.ignore_whitespace {
            text.lines()
                .map(|l| interner.id(&ws::line_key(l)))
                .collect()
        } else {
            interner.ids(text.lines())
        }
    };
    let (b, o, t) = (ids(base), ids(ours), ids(theirs));
    diff3_ids(&b, &o, &t, options.style)
        .into_iter()
        .map(|c| Diff3Chunk {
            kind: c.kind,
            base: c.base.start as u32..c.base.end as u32,
            ours: c.ours.start as u32..c.ours.end as u32,
            theirs: c.theirs.start as u32..c.theirs.end as u32,
        })
        .collect()
}

/// The merged text when nothing conflicts, else the number of conflicts.
pub fn merge_clean(base: &str, ours: &str, theirs: &str) -> Result<String, usize> {
    let (base, ours, theirs) = (Text::new(base), Text::new(ours), Text::new(theirs));
    let chunks = diff3(&base, &ours, &theirs, &Diff3Options::default());
    let conflicts = chunks
        .iter()
        .filter(|c| c.kind == ChunkKind::Conflict)
        .count();
    if conflicts > 0 {
        return Err(conflicts);
    }
    Ok(chunks
        .iter()
        .map(|c| match c.kind {
            ChunkKind::TheirsOnly => theirs.slice(c.theirs.clone()),
            _ => ours.slice(c.ours.clone()),
        })
        .collect())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RawChunk {
    pub(crate) kind: ChunkKind,
    pub(crate) base: Range<usize>,
    pub(crate) ours: Range<usize>,
    pub(crate) theirs: Range<usize>,
}

pub(crate) fn diff_ops(old: &[u32], new: &[u32]) -> Vec<DiffOp> {
    similar::capture_diff_slices_deadline(
        Algorithm::Myers,
        old,
        new,
        Some(Instant::now() + DEADLINE),
    )
}

/// For each item of `a`, the item of `b` the diff pairs it with.
fn matches(a: &[u32], b: &[u32]) -> Vec<Option<usize>> {
    let mut out = vec![None; a.len()];
    for op in diff_ops(a, b) {
        if let DiffOp::Equal {
            old_index,
            new_index,
            len,
        } = op
        {
            for i in 0..len {
                out[old_index + i] = Some(new_index + i);
            }
        }
    }
    out
}

/// The diff3 of three id sequences (lines or tokens), refined for `style`.
pub(crate) fn diff3_ids(
    base: &[u32],
    ours: &[u32],
    theirs: &[u32],
    style: ConflictStyle,
) -> Vec<RawChunk> {
    let to_ours = matches(base, ours);
    let to_theirs = matches(base, theirs);
    let (mut b, mut o, mut t) = (0, 0, 0);
    let mut out = Vec::new();
    loop {
        let start = (b, o, t);
        while b < base.len() && to_ours[b] == Some(o) && to_theirs[b] == Some(t) {
            b += 1;
            o += 1;
            t += 1;
        }
        if b > start.0 {
            out.push(RawChunk {
                kind: ChunkKind::Unchanged,
                base: start.0..b,
                ours: start.1..o,
                theirs: start.2..t,
            });
        }
        if b == base.len() && o == ours.len() && t == theirs.len() {
            break;
        }
        // The next base line both sides kept ends the changed stretch.
        let mut next = b;
        let (next_o, next_t) = loop {
            if next == base.len() {
                break (ours.len(), theirs.len());
            }
            if let (Some(x), Some(y)) = (to_ours[next], to_theirs[next])
                && x >= o
                && y >= t
            {
                break (x, y);
            }
            next += 1;
        };
        let (bs, os, ts) = (&base[b..next], &ours[o..next_o], &theirs[t..next_t]);
        let kind = match (os == bs, ts == bs) {
            (true, true) => ChunkKind::Unchanged,
            (true, false) => ChunkKind::TheirsOnly,
            (false, true) => ChunkKind::OursOnly,
            (false, false) if os == ts => ChunkKind::BothSame,
            (false, false) => ChunkKind::Conflict,
        };
        let chunk = RawChunk {
            kind,
            base: b..next,
            ours: o..next_o,
            theirs: t..next_t,
        };
        if kind == ChunkKind::Conflict {
            out.extend(refine(chunk, ours, theirs, style));
        } else {
            out.push(chunk);
        }
        (b, o, t) = (next, next_o, next_t);
    }
    coalesce(out)
}

/// Splits a conflict into the parts the sides share and the parts they
/// don't, as git does for `style`.
fn refine(chunk: RawChunk, ours: &[u32], theirs: &[u32], style: ConflictStyle) -> Vec<RawChunk> {
    let (os, ts) = (&ours[chunk.ours.clone()], &theirs[chunk.theirs.clone()]);
    // git: "no sense refining a conflict when one side is empty"
    if os.is_empty() || ts.is_empty() {
        return vec![chunk];
    }
    match style {
        ConflictStyle::Diff3 => vec![chunk],
        ConflictStyle::ZDiff3 => trim_shared(chunk, os, ts),
        ConflictStyle::Merge => split_shared(chunk, os, ts),
    }
}

/// zdiff3: a shared prefix and suffix leave the conflict, which keeps the
/// whole base.
fn trim_shared(chunk: RawChunk, os: &[u32], ts: &[u32]) -> Vec<RawChunk> {
    let prefix = os.iter().zip(ts).take_while(|(a, b)| a == b).count();
    let room = os.len().min(ts.len()) - prefix;
    let suffix = os[prefix..]
        .iter()
        .rev()
        .zip(ts[prefix..].iter().rev())
        .take(room)
        .take_while(|(a, b)| a == b)
        .count();
    let (o, t) = (chunk.ours.start, chunk.theirs.start);
    let (oe, te) = (chunk.ours.end, chunk.theirs.end);
    let mut out = Vec::new();
    if prefix > 0 {
        out.push(RawChunk {
            kind: ChunkKind::BothSame,
            base: chunk.base.start..chunk.base.start,
            ours: o..o + prefix,
            theirs: t..t + prefix,
        });
    }
    out.push(RawChunk {
        kind: ChunkKind::Conflict,
        base: chunk.base.clone(),
        ours: o + prefix..oe - suffix,
        theirs: t + prefix..te - suffix,
    });
    if suffix > 0 {
        out.push(RawChunk {
            kind: ChunkKind::BothSame,
            base: chunk.base.end..chunk.base.end,
            ours: oe - suffix..oe,
            theirs: te - suffix..te,
        });
    }
    out
}

/// The merge style: every run of lines the sides share leaves the
/// conflict. The first conflict left keeps the base lines.
fn split_shared(chunk: RawChunk, os: &[u32], ts: &[u32]) -> Vec<RawChunk> {
    let (o, t) = (chunk.ours.start, chunk.theirs.start);
    let mut out: Vec<RawChunk> = Vec::new();
    let mut base_used = false;
    let mut pending: Option<(Range<usize>, Range<usize>)> = None;
    let flush = |pending: &mut Option<(Range<usize>, Range<usize>)>,
                 out: &mut Vec<RawChunk>,
                 base_used: &mut bool| {
        if let Some((ours, theirs)) = pending.take() {
            let base = if *base_used {
                chunk.base.end..chunk.base.end
            } else {
                *base_used = true;
                chunk.base.clone()
            };
            out.push(RawChunk {
                kind: ChunkKind::Conflict,
                base,
                ours,
                theirs,
            });
        }
    };
    for op in diff_ops(os, ts) {
        let (old, new) = (op.old_range(), op.new_range());
        let (old, new) = (o + old.start..o + old.end, t + new.start..t + new.end);
        if let DiffOp::Equal { .. } = op {
            flush(&mut pending, &mut out, &mut base_used);
            let at = if base_used {
                chunk.base.end
            } else {
                chunk.base.start
            };
            out.push(RawChunk {
                kind: ChunkKind::BothSame,
                base: at..at,
                ours: old,
                theirs: new,
            });
        } else {
            pending = Some(match pending {
                Some((po, pt)) => (po.start..old.end, pt.start..new.end),
                None => (old, new),
            });
        }
    }
    flush(&mut pending, &mut out, &mut base_used);
    if !base_used {
        // Only reachable if the sides were equal after all.
        return vec![RawChunk {
            kind: ChunkKind::BothSame,
            ..chunk
        }];
    }
    out
}

/// Joins neighbours of the same non-conflict kind.
fn coalesce(chunks: Vec<RawChunk>) -> Vec<RawChunk> {
    let mut out: Vec<RawChunk> = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        if let Some(last) = out.last_mut()
            && last.kind == chunk.kind
            && chunk.kind != ChunkKind::Conflict
            && last.base.end == chunk.base.start
        {
            last.base.end = chunk.base.end;
            last.ours.end = chunk.ours.end;
            last.theirs.end = chunk.theirs.end;
            continue;
        }
        out.push(chunk);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(base: &str, ours: &str, theirs: &str, style: ConflictStyle) -> Vec<ChunkKind> {
        let opts = Diff3Options {
            style,
            ..Default::default()
        };
        diff3(
            &Text::new(base),
            &Text::new(ours),
            &Text::new(theirs),
            &opts,
        )
        .into_iter()
        .map(|c| c.kind)
        .collect()
    }

    use ChunkKind::*;
    const M: ConflictStyle = ConflictStyle::Merge;

    #[test]
    fn clean_merge_of_separate_changes() {
        let base = "a\nb\nc\nd\ne\n";
        let ours = "A\nb\nc\nd\ne\n";
        let theirs = "a\nb\nc\nd\nE\n";
        assert_eq!(
            merge_clean(base, ours, theirs).as_deref(),
            Ok("A\nb\nc\nd\nE\n")
        );
        assert_eq!(
            kinds(base, ours, theirs, M),
            [OursOnly, Unchanged, TheirsOnly]
        );
    }

    #[test]
    fn identical_changes_merge() {
        assert_eq!(
            kinds("a\nb\nc\n", "a\nX\nc\n", "a\nX\nc\n", M),
            [Unchanged, BothSame, Unchanged]
        );
    }

    #[test]
    fn overlapping_changes_conflict() {
        assert_eq!(
            kinds("a\nb\nc\n", "a\nX\nc\n", "a\nY\nc\n", M),
            [Unchanged, Conflict, Unchanged]
        );
    }

    #[test]
    fn adjacent_changes_conflict_like_git() {
        // git: changes to neighbouring base lines overlap in xdl_merge
        assert_eq!(
            merge_clean("a\nb\nc\nd\n", "a\nB\nc\nd\n", "a\nb\nC\nd\n"),
            Err(1)
        );
        // one kept line between them is enough
        assert!(merge_clean("a\nb\nc\nd\ne\n", "a\nB\nc\nd\ne\n", "a\nb\nc\nD\ne\n").is_ok());
    }

    #[test]
    fn delete_against_modify_conflicts() {
        assert_eq!(
            kinds("a\nb\nc\n", "a\nc\n", "a\nB\nc\n", M),
            [Unchanged, Conflict, Unchanged]
        );
    }

    #[test]
    fn both_add_with_empty_base() {
        assert_eq!(kinds("", "x\n", "y\n", M), [Conflict]);
        assert_eq!(kinds("", "x\n", "x\n", M), [BothSame]);
    }

    #[test]
    fn insertions_at_the_same_place_conflict() {
        assert_eq!(
            kinds("a\nb\n", "a\nX\nb\n", "a\nY\nb\n", M),
            [Unchanged, Conflict, Unchanged]
        );
    }

    #[test]
    fn missing_final_newline_is_a_change() {
        assert_eq!(
            merge_clean("a\nb\n", "a\nb", "a\nb\n").as_deref(),
            Ok("a\nb")
        );
    }

    #[test]
    fn merge_style_splits_shared_lines_out() {
        let base = "a\nb\nz\n";
        let ours = "a\nX\nsame\nP\nz\n";
        let theirs = "a\nY\nsame\nQ\nz\n";
        assert_eq!(
            kinds(base, ours, theirs, M),
            [Unchanged, Conflict, BothSame, Conflict, Unchanged]
        );
        assert_eq!(
            kinds(base, ours, theirs, ConflictStyle::Diff3),
            [Unchanged, Conflict, Unchanged]
        );
    }

    #[test]
    fn zdiff3_trims_shared_prefix_and_suffix() {
        let base = "a\nb\nz\n";
        let ours = "a\nhead\nX\ntail\nz\n";
        let theirs = "a\nhead\nY\ntail\nz\n";
        let opts = Diff3Options {
            style: ConflictStyle::ZDiff3,
            ..Default::default()
        };
        let chunks = diff3(
            &Text::new(base),
            &Text::new(ours),
            &Text::new(theirs),
            &opts,
        );
        let k: Vec<_> = chunks.iter().map(|c| c.kind).collect();
        assert_eq!(k, [Unchanged, BothSame, Conflict, BothSame, Unchanged]);
        assert_eq!(chunks[2].base, 1..2);
        assert_eq!(chunks[2].ours, 2..3);
    }

    #[test]
    fn whitespace_only_differences_can_be_ignored() {
        let opts = Diff3Options {
            ignore_whitespace: true,
            ..Default::default()
        };
        let chunks = diff3(
            &Text::new("a\nb\nc\n"),
            &Text::new("a\n  b\nc\n"),
            &Text::new("a\nb\nC\n"),
            &opts,
        );
        assert!(chunks.iter().all(|c| c.kind != Conflict));
    }

    #[test]
    fn chunks_cover_every_line() {
        let (b, o, t) = (
            Text::new("1\n2\n3\n4\n5\n6\n"),
            Text::new("1\nx\n3\n4\n6\ny\n"),
            Text::new("0\n1\n2\n3\nq\n5\n6\n"),
        );
        let chunks = diff3(&b, &o, &t, &Diff3Options::default());
        for (side, total) in [
            (0, b.line_count()),
            (1, o.line_count()),
            (2, t.line_count()),
        ] {
            let mut at = 0;
            for c in &chunks {
                let r = [&c.base, &c.ours, &c.theirs][side];
                assert_eq!(r.start, at);
                at = r.end;
            }
            assert_eq!(at, total);
        }
    }
}
