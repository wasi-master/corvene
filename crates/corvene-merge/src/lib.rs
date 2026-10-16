//! Three-way merges for the merge editor (Corvene `1361-merge-editor`).
//!
//! GitHub Desktop has no merge editor: conflicted files are resolved in an
//! external editor (`app/src/ui/lib/conflicts/unmerged-file.tsx`). This
//! crate is the model behind Corvene's: a line diff3 that follows git's
//! `xdl_merge` (adjacent changes conflict, identical changes merge, conflicts
//! are refined at `XDL_MERGE_ZEALOUS`), git's conflict markers in the
//! merge, diff3 and zdiff3 styles, and the reconciliation of a computed merge
//! with the file git left in the working tree.
//!
//! The working file's markers are the truth for what is unresolved: the
//! diff3 here never overrides git, it only supplies the chunks around and
//! between them and the matching lines of each side.
//!
//! Pure: no git, no disk, no UI.

pub mod align;
pub mod anchors;
mod diff3;
mod live;
pub mod markers;
mod reconcile;
mod text;
pub mod token;
pub mod ws;

use std::ops::Range;

pub use diff3::{Diff3Chunk, Diff3Options, diff3, merge_clean};
pub use live::{Edit, LiveChunk, LiveState, render_conflict, resolution_text, resolve, scan};
pub use reconcile::{MAX_BYTES, MAX_LINES, MergeInput, reconcile};
pub use text::Text;

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum MergeError {
    #[error("conflict marker inside a conflict at byte {0}")]
    Nested(usize),
    #[error("conflict marker out of place at byte {0}")]
    Misplaced(usize),
    #[error("conflict starting at byte {0} is not closed")]
    Unterminated(usize),
}

/// One of the three versions a merge combines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Base,
    Ours,
    Theirs,
}

/// How a stretch of the merge relates to its base.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChunkKind {
    /// The same in all three versions.
    Unchanged,
    /// Only ours changed it.
    OursOnly,
    /// Only theirs changed it.
    TheirsOnly,
    /// Both changed it the same way.
    BothSame,
    /// Both changed it differently.
    Conflict,
}

impl ChunkKind {
    pub fn is_change(self) -> bool {
        self != Self::Unchanged
    }
}

/// What a chunk's text in the result holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Resolution {
    /// Still a conflict block with markers.
    Unresolved,
    Ours,
    Theirs,
    OursThenTheirs,
    TheirsThenOurs,
    Base,
    /// Neither side: the chunk's text is dropped.
    Ignore,
    /// Edited by hand, or something none of the others match.
    Custom,
    /// The sides changed different words; both changes kept.
    AutoToken,
    /// The sides differ only in whitespace.
    AutoWhitespace,
}

/// git's `merge.conflictStyle`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ConflictStyle {
    #[default]
    Merge,
    Diff3,
    ZDiff3,
}

impl ConflictStyle {
    /// Reads a `merge.conflictStyle` value; anything unknown is git's
    /// default.
    pub fn from_config(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "diff3" => Self::Diff3,
            "zdiff3" => Self::ZDiff3,
            _ => Self::Merge,
        }
    }

    pub fn shows_base(self) -> bool {
        self != Self::Merge
    }
}

/// The text after each marker (`<<<<<<< HEAD`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Labels {
    pub ours: Option<String>,
    pub base: Option<String>,
    pub theirs: Option<String>,
}

impl Labels {
    /// Fills whatever this lacks from `other`.
    pub fn or(mut self, other: &Labels) -> Self {
        self.ours = self.ours.or_else(|| other.ours.clone());
        self.base = self.base.or_else(|| other.base.clone());
        self.theirs = self.theirs.or_else(|| other.theirs.clone());
        self
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Eol {
    #[default]
    Lf,
    CrLf,
}

impl Eol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
        }
    }

    /// CRLF when most line ends in `text` are CRLF.
    pub fn detect(text: &str) -> Self {
        let lf = text.matches('\n').count();
        let crlf = text.matches("\r\n").count();
        if crlf * 2 > lf { Self::CrLf } else { Self::Lf }
    }
}

/// An automatic resolution offered for a conflict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutoResolution {
    /// `AutoToken` or `AutoWhitespace`.
    pub resolution: Resolution,
    pub text: String,
}

/// A conflict block's sides when they are not the stage files' lines (a
/// block git made that the diff3 here did not).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bodies {
    pub ours: String,
    pub base: Option<String>,
    pub theirs: String,
}

/// A stretch of the merge, with its lines in each version and its bytes in
/// the working file when it was opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeChunk {
    pub id: u32,
    pub kind: ChunkKind,
    pub base: Range<u32>,
    pub ours: Range<u32>,
    pub theirs: Range<u32>,
    /// Bytes in the working file at open.
    pub result: Range<usize>,
    /// What those bytes held at open.
    pub resolution: Resolution,
    pub auto: Option<AutoResolution>,
    /// Set for a conflict block whose sides could not be matched to the
    /// stage files: its side ranges are a guess (or empty) and its sides are
    /// these bodies.
    pub bodies: Option<Bodies>,
}

impl MergeChunk {
    /// Whether the side panes can show where this chunk came from.
    pub fn has_ribbons(&self) -> bool {
        self.bodies.is_none()
    }

    pub fn lines(&self, side: Side) -> Range<u32> {
        match side {
            Side::Base => self.base.clone(),
            Side::Ours => self.ours.clone(),
            Side::Theirs => self.theirs.clone(),
        }
    }
}

/// How much of the merge editor a file gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DocMode {
    /// Side panes, ribbons and every chunk.
    Full,
    /// Only the result with its conflict blocks: the markers could not be
    /// read in full, or the file is too large to diff.
    MarkersOnly,
}

/// A conflicted file opened for merging.
#[derive(Clone, Debug)]
pub struct MergeDocument {
    pub base: Text,
    pub ours: Text,
    pub theirs: Text,
    /// False for a file both sides added (no stage 1).
    pub has_base: bool,
    pub chunks: Vec<MergeChunk>,
    pub style: ConflictStyle,
    pub marker_size: usize,
    pub labels: Labels,
    pub eol: Eol,
    pub mode: DocMode,
}

impl MergeDocument {
    pub fn text(&self, side: Side) -> &Text {
        match side {
            Side::Base => &self.base,
            Side::Ours => &self.ours,
            Side::Theirs => &self.theirs,
        }
    }

    /// A chunk's text in one version.
    pub fn side_text<'a>(&'a self, chunk: &'a MergeChunk, side: Side) -> &'a str {
        match (&chunk.bodies, side) {
            (Some(b), Side::Ours) => &b.ours,
            (Some(b), Side::Theirs) => &b.theirs,
            (Some(b), Side::Base) => b.base.as_deref().unwrap_or(""),
            (None, side) => self.text(side).slice(chunk.lines(side)),
        }
    }

    /// The initial result byte range of every chunk, to seed
    /// [`anchors::Anchors`].
    pub fn result_ranges(&self) -> Vec<Range<usize>> {
        self.chunks.iter().map(|c| c.result.clone()).collect()
    }

    pub fn conflicts(&self) -> impl Iterator<Item = &MergeChunk> {
        self.chunks.iter().filter(|c| c.kind == ChunkKind::Conflict)
    }
}
