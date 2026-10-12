//! Corvene `1331-notebook-diff`: staging a notebook by cell. A cell's
//! checkbox stands for the lines of git's JSON diff inside the cell (its
//! `{` … `}` span on either side), so committing, partial stashing and
//! discarding keep working on the file's real diff.
//!
//! Selecting only some of a JSON file's changed lines can leave it broken
//! (an added cell after the last one changes that cell's `}` to `},`). The
//! staged text is therefore built and parsed before a selection is used;
//! when it does not parse, the structural lines (`}`, `},`, `]`, …) next to
//! the cell's lines are taken along, then the whole change around it, and
//! if it still does not parse the cell cannot be staged on its own.
//!
//! GitHub Desktop stages a notebook only by its JSON lines.

use corvene_models::{DiffHunk, DiffLineKind, DiffSelection, DiffSelectionType};

/// Why a cell's change cannot be staged on its own.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StageError {
    #[error("the notebook would not be valid JSON with only this change staged")]
    Invalid,
}

/// Selection indices of the changed lines of `hunks` that fall inside a
/// cell: an old line within `old` or a new line within `new` (1-based,
/// inclusive spans).
pub fn span_lines(
    hunks: &[DiffHunk],
    old: Option<(u32, u32)>,
    new: Option<(u32, u32)>,
) -> Vec<u32> {
    let within = |n: Option<u32>, span: Option<(u32, u32)>| match (n, span) {
        (Some(n), Some((a, b))) => n >= a && n <= b,
        _ => false,
    };
    changed(hunks)
        .filter(|(_, l)| match l.kind {
            DiffLineKind::Delete => within(l.old_line, old),
            DiffLineKind::Add => within(l.new_line, new),
            _ => false,
        })
        .map(|(abs, _)| abs)
        .collect()
}

/// Changed lines outside every cell span: the notebook's own metadata and
/// the file's structure.
pub fn outside_lines(
    hunks: &[DiffHunk],
    old_spans: &[(u32, u32)],
    new_spans: &[(u32, u32)],
) -> Vec<u32> {
    let inside = |n: Option<u32>, spans: &[(u32, u32)]| {
        n.is_some_and(|n| spans.iter().any(|&(a, b)| n >= a && n <= b))
    };
    changed(hunks)
        .filter(|(_, l)| match l.kind {
            DiffLineKind::Delete => !inside(l.old_line, old_spans),
            DiffLineKind::Add => !inside(l.new_line, new_spans),
            _ => false,
        })
        .map(|(abs, _)| abs)
        .collect()
}

fn changed(hunks: &[DiffHunk]) -> impl Iterator<Item = (u32, &corvene_models::DiffLine)> {
    hunks.iter().flat_map(|h| {
        h.lines
            .iter()
            .enumerate()
            .filter(|(_, l)| matches!(l.kind, DiffLineKind::Add | DiffLineKind::Delete))
            .map(move |(i, l)| (h.unified_diff_start + i as u32, l))
    })
}

/// The state of a cell's checkbox: all, none or some of its lines.
pub fn lines_kind(selection: &DiffSelection, lines: &[u32]) -> DiffSelectionType {
    let mut any = false;
    let mut all = true;
    for &l in lines {
        if selection.is_selected(l) {
            any = true;
        } else {
            all = false;
        }
    }
    match (any, all) {
        (_, true) if !lines.is_empty() => DiffSelectionType::All,
        (false, _) => DiffSelectionType::None,
        _ => DiffSelectionType::Partial,
    }
}

/// `selection` with `lines` set to `selected`.
pub fn with_lines(selection: &DiffSelection, lines: &[u32], selected: bool) -> DiffSelection {
    let mut sorted = lines.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut out = selection.clone();
    let mut run: Option<(u32, u32)> = None;
    for line in sorted {
        run = match run {
            Some((start, len)) if start + len == line => Some((start, len + 1)),
            Some((start, len)) => {
                out = out.with_range(start, len, selected);
                Some((line, 1))
            }
            None => Some((line, 1)),
        };
    }
    if let Some((start, len)) = run {
        out = out.with_range(start, len, selected);
    }
    out
}

/// The file text after applying the selected lines of `hunks` to `old`
/// (what `git apply --cached` of the partial patch leaves in the index).
pub fn staged_text(old: &str, hunks: &[DiffHunk], selection: &DiffSelection) -> String {
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let mut out = String::with_capacity(old.len());
    let mut next_old = 1u32; // next old line to copy
    for h in hunks {
        let start = h.old_start.max(1);
        while next_old < start && (next_old as usize) <= old_lines.len() {
            out.push_str(old_lines[next_old as usize - 1]);
            next_old += 1;
        }
        for (i, l) in h.lines.iter().enumerate() {
            let abs = h.unified_diff_start + i as u32;
            let newline = if l.no_trailing_newline { "" } else { "\n" };
            match l.kind {
                DiffLineKind::Context => {
                    if let Some(text) = old_lines.get(next_old as usize - 1) {
                        out.push_str(text);
                    }
                    next_old += 1;
                }
                DiffLineKind::Delete => {
                    if !selection.is_selected(abs) {
                        out.push_str(&l.text);
                        out.push_str(newline);
                    }
                    next_old += 1;
                }
                DiffLineKind::Add => {
                    if selection.is_selected(abs) {
                        out.push_str(&l.text);
                        out.push_str(newline);
                    }
                }
                _ => {}
            }
        }
    }
    while (next_old as usize) <= old_lines.len() {
        out.push_str(old_lines[next_old as usize - 1]);
        next_old += 1;
    }
    out
}

fn parses(text: &str) -> bool {
    text.trim().is_empty() || serde_json::from_str::<serde::de::IgnoredAny>(text).is_ok()
}

fn structural(text: &str) -> bool {
    let t = text.trim();
    !t.is_empty() && t.chars().all(|c| matches!(c, '{' | '}' | '[' | ']' | ','))
}

/// `selection` with a cell's `lines` set to `selected`, widened until the
/// staged notebook parses (`old` is the text the diff's old side is).
pub fn select_cell(
    old: &str,
    hunks: &[DiffHunk],
    selection: &DiffSelection,
    lines: &[u32],
    selected: bool,
) -> Result<DiffSelection, StageError> {
    let candidate = with_lines(selection, lines, selected);
    if parses(&staged_text(old, hunks, &candidate)) {
        return Ok(candidate);
    }
    // the structural lines touching the cell's lines in the same change run
    let runs = change_runs(hunks);
    let mut wider: Vec<u32> = lines.to_vec();
    for run in &runs {
        if !run.iter().any(|(abs, _)| lines.contains(abs)) {
            continue;
        }
        let positions: Vec<usize> = run
            .iter()
            .enumerate()
            .filter(|(_, (abs, _))| lines.contains(abs))
            .map(|(k, _)| k)
            .collect();
        for &k in &positions {
            for dir in [-1i64, 1] {
                let mut p = k as i64 + dir;
                while p >= 0 && (p as usize) < run.len() {
                    let (abs, text) = &run[p as usize];
                    if lines.contains(abs) {
                        p += dir;
                        continue;
                    }
                    if !structural(text) {
                        break;
                    }
                    wider.push(*abs);
                    p += dir;
                }
            }
        }
    }
    let candidate = with_lines(selection, &wider, selected);
    if parses(&staged_text(old, hunks, &candidate)) {
        return Ok(candidate);
    }
    // the whole change around the cell
    for run in &runs {
        if run.iter().any(|(abs, _)| lines.contains(abs)) {
            wider.extend(run.iter().map(|(abs, _)| *abs));
        }
    }
    let candidate = with_lines(selection, &wider, selected);
    if parses(&staged_text(old, hunks, &candidate)) {
        return Ok(candidate);
    }
    Err(StageError::Invalid)
}

/// Runs of consecutive changed lines (selection index, text).
fn change_runs(hunks: &[DiffHunk]) -> Vec<Vec<(u32, &str)>> {
    let mut out: Vec<Vec<(u32, &str)>> = Vec::new();
    for h in hunks {
        let mut run: Vec<(u32, &str)> = Vec::new();
        for (i, l) in h.lines.iter().enumerate() {
            if matches!(l.kind, DiffLineKind::Add | DiffLineKind::Delete) {
                run.push((h.unified_diff_start + i as u32, l.text.as_str()));
            } else if !run.is_empty() {
                out.push(std::mem::take(&mut run));
            }
        }
        if !run.is_empty() {
            out.push(run);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunks(old: &str, new: &str) -> Vec<DiffHunk> {
        match crate::pdf::line_diff(Some(old), Some(new)) {
            corvene_models::Diff::Text { hunks, .. } => hunks,
            other => panic!("{other:?}"),
        }
    }

    const OLD: &str = "{\n \"cells\": [\n  {\n   \"cell_type\": \"code\",\n   \"source\": [\"a\"]\n  },\n  {\n   \"cell_type\": \"code\",\n   \"source\": [\"b\"]\n  }\n ],\n \"nbformat\": 4\n}\n";

    #[test]
    fn appended_cell_takes_the_comma_along() {
        let new = OLD.replace(
            "   \"source\": [\"b\"]\n  }\n ],",
            "   \"source\": [\"b\"]\n  },\n  {\n   \"cell_type\": \"markdown\",\n   \"source\": [\"c\"]\n  }\n ],",
        );
        let h = hunks(OLD, &new);
        // the new cell: lines 11-14 of the new file
        let lines = span_lines(&h, None, Some((11, 14)));
        assert!(!lines.is_empty());
        let none = DiffSelection::none();
        // alone it breaks the JSON
        assert!(!parses(&staged_text(
            OLD,
            &h,
            &with_lines(&none, &lines, true)
        )));
        let sel = select_cell(OLD, &h, &none, &lines, true).unwrap();
        let staged = staged_text(OLD, &h, &sel);
        assert!(parses(&staged), "{staged}");
        assert_eq!(staged, new);
        assert_eq!(lines_kind(&sel, &lines), DiffSelectionType::All);
    }

    #[test]
    fn edited_cell_stages_alone() {
        let new = OLD
            .replace("[\"a\"]", "[\"a2\"]")
            .replace("[\"b\"]", "[\"b2\"]");
        let h = hunks(OLD, &new);
        let first = span_lines(&h, Some((3, 6)), Some((3, 6)));
        assert_eq!(first.len(), 2);
        let sel = select_cell(OLD, &h, &DiffSelection::none(), &first, true).unwrap();
        let staged = staged_text(OLD, &h, &sel);
        assert!(staged.contains("a2") && !staged.contains("b2"));
        // unstaging it again
        let all = DiffSelection::all();
        let sel = select_cell(OLD, &h, &all, &first, false).unwrap();
        let staged = staged_text(OLD, &h, &sel);
        assert!(!staged.contains("a2") && staged.contains("b2"));
    }

    #[test]
    fn full_selection_reproduces_new_text() {
        let new = OLD.replace("\"nbformat\": 4", "\"nbformat\": 5");
        let h = hunks(OLD, &new);
        assert_eq!(staged_text(OLD, &h, &DiffSelection::all()), new);
        assert_eq!(staged_text(OLD, &h, &DiffSelection::none()), OLD);
        let outside = outside_lines(&h, &[(3, 6), (7, 10)], &[(3, 6), (7, 10)]);
        assert_eq!(outside.len(), 2);
    }
}
