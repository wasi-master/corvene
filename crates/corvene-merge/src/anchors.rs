//! Chunk ranges in the result, carried through edits.
//!
//! Fed every edit the editor makes, undo and redo included, as
//! `(start, old_end, new_end)` byte offsets. A boundary inside a replaced
//! stretch moves to its new end; one at the stretch's start stays, so text
//! typed where one chunk ends and the next begins joins the next (typing at
//! a line start edits that line).

use std::ops::Range;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Anchors {
    ranges: Vec<Range<usize>>,
}

fn shift(p: usize, start: usize, old_end: usize, new_end: usize) -> usize {
    if p <= start {
        p
    } else if p > old_end {
        p - old_end + new_end
    } else {
        new_end
    }
}

impl Anchors {
    pub fn new(ranges: Vec<Range<usize>>) -> Self {
        Self { ranges }
    }

    pub fn ranges(&self) -> &[Range<usize>] {
        &self.ranges
    }

    /// Starts over, after a reload.
    pub fn reset(&mut self, ranges: Vec<Range<usize>>) {
        self.ranges = ranges;
    }

    /// Bytes `start..old_end` became `start..new_end`.
    pub fn edit(&mut self, start: usize, old_end: usize, new_end: usize) {
        for r in &mut self.ranges {
            r.start = shift(r.start, start, old_end, new_end);
            r.end = shift(r.end, start, old_end, new_end).max(r.start);
        }
    }

    /// An edit made for one chunk (accepting a side): it applies as any
    /// edit, then the chunk owns exactly the new text and its neighbours
    /// give up any of it.
    pub fn edit_chunk(&mut self, chunk: usize, range: Range<usize>, new_len: usize) {
        self.edit(range.start, range.end, range.start + new_len);
        self.claim(chunk, range.start..range.start + new_len);
    }

    /// Gives `chunk` exactly `range`, trimming the others off it.
    pub fn claim(&mut self, chunk: usize, range: Range<usize>) {
        for (i, r) in self.ranges.iter_mut().enumerate() {
            if i == chunk {
                *r = range.clone();
            } else if i < chunk {
                r.end = r.end.min(range.start);
                r.start = r.start.min(r.end);
            } else {
                r.start = r.start.max(range.end);
                r.end = r.end.max(r.start);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_before_inside_and_after() {
        let mut a = Anchors::new(vec![0..4, 4..10, 10..12]);
        a.edit(5, 7, 9); // grow the middle chunk by two
        assert_eq!(a.ranges(), [0..4, 4..12, 12..14]);
        a.edit(4, 4, 6); // type at the middle chunk's start
        assert_eq!(a.ranges(), [0..4, 4..14, 14..16]);
        a.edit(2, 14, 3); // delete across chunks
        assert_eq!(a.ranges(), [0..3, 3..3, 3..5]);
    }

    #[test]
    fn claiming_after_an_insertion_into_an_empty_chunk() {
        let mut a = Anchors::new(vec![0..4, 4..4, 4..8]);
        a.edit_chunk(1, 4..4, 3);
        assert_eq!(a.ranges(), [0..4, 4..7, 7..11]);
    }

    #[test]
    fn edit_then_inverse_restores_ranges() {
        // random edits replayed backwards (undo) leave every range where
        // it was as long as no boundary was swallowed
        let start = vec![0..10, 10..20, 20..30];
        let mut a = Anchors::new(start.clone());
        let edits = [(12usize, 15usize, 19usize), (25, 25, 27), (11, 11, 12)];
        for &(s, o, n) in &edits {
            a.edit(s, o, n);
        }
        for &(s, o, n) in edits.iter().rev() {
            a.edit(s, n, o);
        }
        assert_eq!(a.ranges(), start.as_slice());
    }
}
