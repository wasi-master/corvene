//! Corvene: runs of unchanged rows in a rendered diff (a table, a document's
//! blocks, a notebook's cells) folded into one row, as the text diff keeps
//! only a few context lines around each hunk. The view opens a fold by its
//! `start`.

use std::collections::HashSet;
use std::ops::Range;

/// A row of a folded view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FoldRow {
    /// Item `0` of the unfolded list.
    Item(usize),
    /// Unchanged items `start..end` as one row.
    Folded { start: usize, end: usize },
}

/// How much is kept around a change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FoldRule {
    /// Unchanged items kept next to each change.
    pub context: usize,
    /// Fewer unchanged items than this are not folded.
    pub min: usize,
}

impl FoldRule {
    /// Table rows and lines: the text diff's three lines of context.
    pub const ROWS: FoldRule = FoldRule { context: 3, min: 4 };
    /// Blocks and cells, each of which is tall.
    pub const BLOCKS: FoldRule = FoldRule { context: 1, min: 3 };
}

/// The rows to draw for items whose change flags are `changed`;
/// `expanded` holds the `start` of folds the user opened. Nothing folds
/// when nothing changed.
pub fn fold(changed: &[bool], rule: FoldRule, expanded: &HashSet<usize>) -> Vec<FoldRow> {
    let n = changed.len();
    if !changed.iter().any(|c| *c) {
        return (0..n).map(FoldRow::Item).collect();
    }
    let mut out = Vec::with_capacity(n.min(1024));
    let mut i = 0;
    while i < n {
        if changed[i] {
            out.push(FoldRow::Item(i));
            i += 1;
            continue;
        }
        let start = i;
        while i < n && !changed[i] {
            i += 1;
        }
        let end = i;
        let lead = if start > 0 { rule.context } else { 0 };
        let trail = if end < n { rule.context } else { 0 };
        let (fold_start, fold_end) = (start + lead, end.saturating_sub(trail));
        if fold_end <= fold_start
            || fold_end - fold_start < rule.min
            || expanded.contains(&fold_start)
        {
            out.extend((start..end).map(FoldRow::Item));
            continue;
        }
        out.extend((start..fold_start).map(FoldRow::Item));
        out.push(FoldRow::Folded {
            start: fold_start,
            end: fold_end,
        });
        out.extend((fold_end..end).map(FoldRow::Item));
    }
    out
}

/// The row of `rows` showing the first changed item, a little context
/// above it kept in view (`None`: no change).
pub fn first_change(rows: &[FoldRow], changed: &[bool], context: usize) -> Option<usize> {
    let ix = rows
        .iter()
        .position(|r| matches!(r, FoldRow::Item(i) if changed.get(*i).copied().unwrap_or(false)))?;
    // a fold above stays in view rather than half the context
    let above = rows[..ix]
        .iter()
        .rev()
        .take(context)
        .take_while(|r| matches!(r, FoldRow::Item(_)))
        .count();
    Some(ix - above.min(context))
}

/// The items a fold row stands for.
pub fn folded_range(row: &FoldRow) -> Option<Range<usize>> {
    match row {
        FoldRow::Folded { start, end } => Some(*start..*end),
        FoldRow::Item(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags(s: &str) -> Vec<bool> {
        s.chars().map(|c| c == 'x').collect()
    }

    #[test]
    fn folds_long_unchanged_runs() {
        let changed = flags("..........x..........");
        let rows = fold(&changed, FoldRule::ROWS, &HashSet::new());
        assert_eq!(rows[0], FoldRow::Folded { start: 0, end: 7 });
        assert_eq!(rows[4], FoldRow::Item(10));
        assert_eq!(rows.last(), Some(&FoldRow::Folded { start: 14, end: 21 }));
        assert_eq!(first_change(&rows, &changed, 1), Some(3));
        // opened, the run shows whole
        let open = fold(&changed, FoldRule::ROWS, &HashSet::from([0]));
        assert_eq!(open[0], FoldRow::Item(0));
    }

    #[test]
    fn short_runs_and_unchanged_files_stay() {
        let changed = flags("x.....x");
        assert_eq!(fold(&changed, FoldRule::ROWS, &HashSet::new()).len(), 7);
        assert_eq!(
            fold(&flags("...."), FoldRule::ROWS, &HashSet::new()).len(),
            4
        );
        assert_eq!(first_change(&[FoldRow::Item(0)], &[false], 3), None);
    }
}
