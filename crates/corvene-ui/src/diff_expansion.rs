//! Hunk expansion (GHD `ui/diff/text-diff-expansion.ts`): fold-up / fold-down
//! handles in hunk headers pull unchanged lines from the new side of the file
//! into the diff, twenty at a time or the whole file. Expanded hunks are a
//! view-local copy of the model's hunks; each line remembers its index in the
//! original diff so selections and discard patches keep addressing the model.

use corvene_core::{DiffHunk, DiffLine, DiffLineKind};

/// GHD `DefaultDiffExpansionStep`.
pub const DEFAULT_DIFF_EXPANSION_STEP: u32 = 20;

/// GHD `DiffHunkExpansionType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HunkExpansionType {
    /// Not expandable (nothing above / expansion disabled).
    None,
    /// First hunk with lines above it.
    Up,
    /// The bottom dummy hunk: lines below the last real hunk.
    Down,
    /// Far from the previous hunk: expand down (into the gap) or up.
    Both,
    /// Within one step of the previous hunk: one handle fills the gap.
    Short,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpansionKind {
    Up,
    Down,
}

/// A diff line plus its absolute index in the original (unexpanded) diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XLine {
    pub line: DiffLine,
    pub original: Option<u32>,
}

/// GHD `DiffHunk` with `expansionType`; `lines[0]` is the header line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XHunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<XLine>,
    /// Absolute index of the header line in the expanded diff.
    pub unified_diff_start: u32,
    pub expansion: HunkExpansionType,
}

impl XHunk {
    pub fn unified_diff_end(&self) -> u32 {
        self.unified_diff_start + self.lines.len() as u32 - 1
    }

    fn is_dummy(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].line.kind == DiffLineKind::Hunk
    }
}

/// GHD `DiffHunkHeader.toDiffLineRepresentation`.
pub fn header_text(old_start: u32, old_lines: u32, new_start: u32, new_lines: u32) -> String {
    format!("@@ -{old_start},{old_lines} +{new_start},{new_lines} @@")
}

fn header_line(old_start: u32, old_lines: u32, new_start: u32, new_lines: u32) -> XLine {
    XLine {
        line: DiffLine {
            kind: DiffLineKind::Hunk,
            text: header_text(old_start, old_lines, new_start, new_lines),
            old_line: None,
            new_line: None,
            no_trailing_newline: false,
            carriage_return: false,
            raw: None,
        },
        original: None,
    }
}

/// GHD `getHunkHeaderExpansionType`.
pub fn expansion_type(
    index: usize,
    old_start: u32,
    new_start: u32,
    previous: Option<&XHunk>,
) -> HunkExpansionType {
    let distance_to_previous = previous
        .map(|p| old_start as i64 - p.old_start as i64 - p.old_lines as i64)
        .unwrap_or(i64::MAX);
    if index == 0 {
        if old_start > 1 && new_start > 1 {
            HunkExpansionType::Up
        } else {
            HunkExpansionType::None
        }
    } else if distance_to_previous <= DEFAULT_DIFF_EXPANSION_STEP as i64 {
        HunkExpansionType::Short
    } else {
        HunkExpansionType::Both
    }
}

/// The model's hunks as expandable hunks. With the new file contents known
/// (`new_line_count`) the headers get their expansion handles and a dummy
/// hunk at the bottom stands for the lines after the last hunk
/// (`getTextDiffWithBottomDummyHunk`).
pub fn from_hunks(hunks: &[DiffHunk], new_line_count: Option<usize>) -> Vec<XHunk> {
    let mut out: Vec<XHunk> = Vec::with_capacity(hunks.len() + 1);
    for (index, hunk) in hunks.iter().enumerate() {
        let expansion = match new_line_count {
            Some(_) => expansion_type(index, hunk.old_start, hunk.new_start, out.last()),
            None => HunkExpansionType::None,
        };
        out.push(XHunk {
            old_start: hunk.old_start,
            old_lines: hunk.old_lines,
            new_start: hunk.new_start,
            new_lines: hunk.new_lines,
            lines: hunk
                .lines
                .iter()
                .enumerate()
                .map(|(i, line)| XLine {
                    line: line.clone(),
                    original: Some(hunk.unified_diff_start + i as u32),
                })
                .collect(),
            unified_diff_start: hunk.unified_diff_start,
            expansion,
        });
    }
    if let (Some(count), Some(last)) = (new_line_count, out.last()) {
        let new_count = count as u32;
        let next_new = last.new_start + last.new_lines;
        if next_new <= new_count {
            let (adds, dels) =
                hunks
                    .iter()
                    .flat_map(|h| h.lines.iter())
                    .fold((0u32, 0u32), |(a, d), l| match l.kind {
                        DiffLineKind::Add => (a + 1, d),
                        DiffLineKind::Delete => (a, d + 1),
                        _ => (a, d),
                    });
            let old_count = (new_count + dels).saturating_sub(adds);
            let dummy_old_start = last.old_start + last.old_lines;
            let dummy = XHunk {
                old_start: dummy_old_start,
                old_lines: (old_count + 1).saturating_sub(dummy_old_start),
                new_start: next_new,
                new_lines: new_count + 1 - next_new,
                lines: vec![XLine {
                    line: DiffLine {
                        kind: DiffLineKind::Hunk,
                        text: String::new(),
                        old_line: None,
                        new_line: None,
                        no_trailing_newline: false,
                        carriage_return: false,
                        raw: None,
                    },
                    original: None,
                }],
                unified_diff_start: last.unified_diff_end() + 1,
                expansion: HunkExpansionType::Down,
            };
            out.push(dummy);
        }
    }
    out
}

/// GHD `mergeDiffHunks`.
fn merge(h1: &XHunk, h2: &XHunk) -> XHunk {
    let (old_start, old_lines, new_start, new_lines) = (
        h1.old_start,
        h1.old_lines + h2.old_lines,
        h1.new_start,
        h1.new_lines + h2.new_lines,
    );
    let mut lines = vec![header_line(old_start, old_lines, new_start, new_lines)];
    lines.extend(h1.lines.iter().skip(1).cloned());
    lines.extend(h2.lines.iter().skip(1).cloned());
    XHunk {
        old_start,
        old_lines,
        new_start,
        new_lines,
        lines,
        unified_diff_start: h1.unified_diff_start,
        expansion: h1.expansion,
    }
}

/// GHD `expandTextDiffHunk`: `None` when there is nothing to pull in.
pub fn expand_hunk(
    hunks: &[XHunk],
    index: usize,
    kind: ExpansionKind,
    contents: &[String],
    step: u32,
) -> Option<Vec<XHunk>> {
    let hunk = hunks.get(index)?;
    let is_up = kind == ExpansionKind::Up;
    let adjacent_index = if is_up && index > 0 {
        Some(index - 1)
    } else if !is_up && index + 1 < hunks.len() {
        Some(index + 1)
    } else {
        None
    };
    let adjacent = adjacent_index.map(|i| &hunks[i]);
    let is_adjacent_dummy = adjacent
        .zip(adjacent_index)
        .is_some_and(|(a, i)| !is_up && a.is_dummy() && i == hunks.len() - 1);

    let new_line_number = hunk.new_start as i64;
    let old_line_number = hunk.old_start as i64;
    let (mut from, mut to) = if is_up {
        (new_line_number - step as i64, new_line_number)
    } else {
        (
            new_line_number + hunk.new_lines as i64,
            new_line_number + hunk.new_lines as i64 + step as i64,
        )
    };
    let mut merge_with_adjacent = false;
    if let Some(adj) = adjacent {
        if is_up {
            let up_limit = (adj.new_start + adj.new_lines) as i64;
            from = from.max(up_limit);
            merge_with_adjacent = from == up_limit;
        } else if !is_adjacent_dummy {
            let down_limit = adj.new_start as i64;
            to = to.min(down_limit);
            merge_with_adjacent = to == down_limit;
        }
    }
    let slice_start = (from - 1).max(0) as usize;
    let slice_end = ((to - 1).max(0) as usize).min(contents.len());
    if slice_end <= slice_start {
        return None;
    }
    let new_lines = &contents[slice_start..slice_end];
    let count = new_lines.len() as u32;

    let expanded: Vec<XLine> = new_lines
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let i = i as u32;
            let (old_no, new_no) = if is_up {
                (
                    old_line_number - (count - i) as i64,
                    new_line_number - (count - i) as i64,
                )
            } else {
                (
                    old_line_number + hunk.old_lines as i64 + i as i64,
                    new_line_number + hunk.new_lines as i64 + i as i64,
                )
            };
            XLine {
                line: DiffLine {
                    kind: DiffLineKind::Context,
                    text: text.clone(),
                    old_line: Some(old_no.max(0) as u32),
                    new_line: Some(new_no.max(0) as u32),
                    no_trailing_newline: false,
                    carriage_return: false,
                    raw: None,
                },
                original: None,
            }
        })
        .collect();

    let (old_start, new_start) = if is_up {
        (hunk.old_start - count, hunk.new_start - count)
    } else {
        (hunk.old_start, hunk.new_start)
    };
    let old_lines = hunk.old_lines + count;
    let new_lines_count = hunk.new_lines + count;
    let mut lines = vec![header_line(
        old_start,
        old_lines,
        new_start,
        new_lines_count,
    )];
    if is_up {
        lines.extend(expanded);
        lines.extend(hunk.lines.iter().skip(1).cloned());
    } else {
        lines.extend(hunk.lines.iter().skip(1).cloned());
        lines.extend(expanded);
    }
    let mut new_diff_lines = lines.len() as i64 - hunk.lines.len() as i64;
    let previous = if index == 0 {
        None
    } else {
        Some(&hunks[index - 1])
    };
    let mut updated = XHunk {
        old_start,
        old_lines,
        new_start,
        new_lines: new_lines_count,
        lines,
        unified_diff_start: hunk.unified_diff_start,
        expansion: expansion_type(index, old_start, new_start, previous),
    };

    let (previous_end, following_start) = match adjacent {
        Some(adj) if merge_with_adjacent => {
            new_diff_lines -= 1;
            if is_up {
                updated = merge(adj, &updated);
                (index - 1, index + 1)
            } else {
                updated = merge(&updated, adj);
                (index, index + 2)
            }
        }
        _ => (index, index + 1),
    };
    let mut result: Vec<XHunk> = hunks[..previous_end].to_vec();
    let new_hunk_last_line = updated.new_start + updated.new_lines - 1;
    result.push(updated);
    if (new_hunk_last_line as usize) < contents.len() {
        let updated_ref = result.len() - 1;
        for (i, h) in hunks.iter().enumerate().skip(following_start) {
            let is_last_dummy = i == hunks.len() - 1 && h.is_dummy();
            let recompute = i == following_start && !is_last_dummy;
            let expansion = if recompute {
                expansion_type(
                    following_start,
                    h.old_start,
                    h.new_start,
                    Some(&result[updated_ref]),
                )
            } else {
                h.expansion
            };
            result.push(XHunk {
                unified_diff_start: (h.unified_diff_start as i64 + new_diff_lines).max(0) as u32,
                expansion,
                ..h.clone()
            });
        }
    }
    Some(result)
}

/// GHD `expandWholeTextDiff`: keep expanding the first hunk until one hunk
/// covers the whole file.
pub fn expand_whole(mut hunks: Vec<XHunk>, contents: &[String]) -> Option<Vec<XHunk>> {
    let step = contents.len().max(1) as u32;
    loop {
        let more =
            hunks.len() > 1 || (hunks.len() == 1 && hunks[0].expansion == HunkExpansionType::Up);
        if !more {
            return Some(hunks);
        }
        let kind = if hunks[0].expansion == HunkExpansionType::Up {
            ExpansionKind::Up
        } else {
            ExpansionKind::Down
        };
        hunks = expand_hunk(&hunks, 0, kind, contents, step)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(kind: DiffLineKind, text: &str, old: Option<u32>, new: Option<u32>) -> DiffLine {
        DiffLine {
            kind,
            text: text.into(),
            old_line: old,
            new_line: new,
            no_trailing_newline: false,
            carriage_return: false,
            raw: None,
        }
    }

    /// A 40-line file where line 21 changed: `@@ -18,7 +18,7 @@`.
    fn sample() -> (Vec<DiffHunk>, Vec<String>) {
        let contents: Vec<String> = (1..=40).map(|n| format!("line {n}")).collect();
        let mut lines = vec![line(DiffLineKind::Hunk, "@@ -18,7 +18,7 @@", None, None)];
        for n in 18..=20 {
            lines.push(line(
                DiffLineKind::Context,
                &format!("line {n}"),
                Some(n),
                Some(n),
            ));
        }
        lines.push(line(DiffLineKind::Delete, "old 21", Some(21), None));
        lines.push(line(DiffLineKind::Add, "line 21", None, Some(21)));
        for n in 22..=24 {
            lines.push(line(
                DiffLineKind::Context,
                &format!("line {n}"),
                Some(n),
                Some(n),
            ));
        }
        let hunk = DiffHunk {
            unified_diff_start: 0,
            header: "@@ -18,7 +18,7 @@".into(),
            old_start: 18,
            old_lines: 7,
            new_start: 18,
            new_lines: 7,
            lines,
        };
        (vec![hunk], contents)
    }

    #[test]
    fn adds_dummy_hunk_and_up_handle() {
        let (hunks, contents) = sample();
        let x = from_hunks(&hunks, Some(contents.len()));
        assert_eq!(x.len(), 2);
        assert_eq!(x[0].expansion, HunkExpansionType::Up);
        assert_eq!(x[1].expansion, HunkExpansionType::Down);
        assert_eq!((x[1].new_start, x[1].new_lines), (25, 16));
        assert_eq!(x[1].unified_diff_start, 9);
        // without contents nothing is expandable
        let plain = from_hunks(&hunks, None);
        assert_eq!(plain.len(), 1);
        assert_eq!(plain[0].expansion, HunkExpansionType::None);
        assert_eq!(plain[0].lines[3].original, Some(3));
    }

    #[test]
    fn expands_up_by_one_step_then_to_the_top() {
        let (hunks, contents) = sample();
        let x = from_hunks(&hunks, Some(contents.len()));
        let x = expand_hunk(&x, 0, ExpansionKind::Up, &contents, 20).unwrap();
        // 17 lines above, so the step is clipped and the hunk now starts at 1
        assert_eq!((x[0].old_start, x[0].new_start), (1, 1));
        assert_eq!(x[0].expansion, HunkExpansionType::None);
        assert_eq!(x[0].lines[1].line.text, "line 1");
        assert_eq!(x[0].lines[1].original, None);
        assert_eq!(x[0].lines[1].line.new_line, Some(1));
        // the changed line keeps its original index
        let changed = x[0]
            .lines
            .iter()
            .find(|l| l.line.kind == DiffLineKind::Add)
            .unwrap();
        assert_eq!(changed.original, Some(5));
        // the dummy hunk moved down by 17 lines
        assert_eq!(x[1].unified_diff_start, 9 + 17);
        assert!(expand_hunk(&x, 0, ExpansionKind::Up, &contents, 20).is_none());
    }

    #[test]
    fn expands_down_into_the_dummy_and_whole_file() {
        let (hunks, contents) = sample();
        let x = from_hunks(&hunks, Some(contents.len()));
        let x = expand_hunk(&x, 0, ExpansionKind::Down, &contents, 20).unwrap();
        // 25..=40 fit inside one step: the dummy hunk is consumed
        assert_eq!(x.len(), 1);
        assert_eq!(x[0].new_start + x[0].new_lines - 1, 40);
        let whole = expand_whole(from_hunks(&hunks, Some(contents.len())), &contents).unwrap();
        assert_eq!(whole.len(), 1);
        assert_eq!((whole[0].new_start, whole[0].new_lines), (1, 40));
        assert_eq!(whole[0].lines.len(), 1 + 40 + 1);
    }
}
