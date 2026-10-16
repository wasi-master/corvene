//! Line correspondence between the merge editor's panes, for scroll sync
//! and the ribbons between panes (Meld's and JetBrains' approach): chunk
//! boundaries line up exactly and lines in between map linearly.

use std::ops::Range;

use crate::MergeDocument;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pane {
    Base,
    Ours,
    Result,
    Theirs,
}

impl Pane {
    fn col(self) -> usize {
        match self {
            Self::Base => 0,
            Self::Ours => 1,
            Self::Result => 2,
            Self::Theirs => 3,
        }
    }
}

/// Matching line boundaries of the four panes, ascending in each.
#[derive(Clone, Debug, PartialEq)]
pub struct AlignmentMap {
    rows: Vec<[f64; 4]>,
}

impl AlignmentMap {
    /// From boundary rows `[base, ours, result, theirs]`. Rows are made
    /// ascending in every pane.
    pub fn new(rows: impl IntoIterator<Item = [u32; 4]>) -> Self {
        let mut out: Vec<[f64; 4]> = vec![[0.; 4]];
        for row in rows {
            let prev = out.last().copied().unwrap_or([0.; 4]);
            let mut next = [0.; 4];
            for c in 0..4 {
                next[c] = (row[c] as f64).max(prev[c]);
            }
            if next != prev {
                out.push(next);
            }
        }
        Self { rows: out }
    }

    /// Every ribboned chunk's start and end, given each chunk's lines in
    /// the result now and the result's line count.
    pub fn from_doc(doc: &MergeDocument, result: &[Range<u32>], result_lines: u32) -> Self {
        let mut rows = Vec::with_capacity(doc.chunks.len() * 2 + 1);
        for (chunk, r) in doc.chunks.iter().zip(result) {
            if !chunk.has_ribbons() {
                continue;
            }
            rows.push([
                chunk.base.start,
                chunk.ours.start,
                r.start,
                chunk.theirs.start,
            ]);
            rows.push([chunk.base.end, chunk.ours.end, r.end, chunk.theirs.end]);
        }
        rows.push([
            doc.base.line_count(),
            doc.ours.line_count(),
            result_lines,
            doc.theirs.line_count(),
        ]);
        Self::new(rows)
    }

    pub fn rows(&self) -> &[[f64; 4]] {
        &self.rows
    }

    /// The line (fractional) of `to` beside line `line` of `from`.
    pub fn map(&self, from: Pane, line: f64, to: Pane) -> f64 {
        let (f, t) = (from.col(), to.col());
        let k = self
            .rows
            .partition_point(|r| r[f] <= line)
            .saturating_sub(1);
        let lo = self.rows[k];
        let Some(hi) = self.rows.get(k + 1) else {
            return lo[t] + (line - lo[f]).max(0.);
        };
        let span = hi[f] - lo[f];
        if span <= 0. {
            return lo[t];
        }
        lo[t] + (line - lo[f]) / span * (hi[t] - lo[t])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_through_chunks() {
        // a 2-line ours chunk where theirs has 6 lines
        let map = AlignmentMap::new([[10, 10, 10, 10], [12, 12, 12, 16], [40, 40, 40, 44]]);
        assert_eq!(map.map(Pane::Ours, 5., Pane::Theirs), 5.);
        assert_eq!(map.map(Pane::Ours, 11., Pane::Theirs), 13.);
        assert_eq!(map.map(Pane::Theirs, 13., Pane::Ours), 11.);
        assert_eq!(map.map(Pane::Ours, 20., Pane::Theirs), 24.);
        assert_eq!(map.map(Pane::Ours, 50., Pane::Theirs), 54.);
    }

    #[test]
    fn an_empty_side_jumps() {
        let map = AlignmentMap::new([[3, 3, 3, 3], [3, 3, 3, 8], [9, 9, 9, 14]]);
        assert_eq!(map.map(Pane::Ours, 3., Pane::Theirs), 8.);
        assert_eq!(map.map(Pane::Theirs, 5., Pane::Ours), 3.);
    }
}
