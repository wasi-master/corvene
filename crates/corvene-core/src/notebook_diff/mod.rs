//! Corvene `1331-notebook-diff`: two versions of a Jupyter notebook as
//! cells. Cells are paired the way nbdime pairs them: by their nbformat 4.5
//! `id` when both versions have unique ids, else by equal source, and within
//! a changed stretch by similar source (an edited cell). A cell that left
//! one place and turns up at another is shown as moved, not as a deletion
//! and an addition.
//!
//! Each pair records what changed (source, type, execution count, outputs,
//! metadata, attachments) so the view can hide the noise a user asks it to
//! hide (execution counts and metadata by default, desktop#17728) and still
//! say that something was hidden.
//!
//! GitHub Desktop has no notebook diff: an `.ipynb` file is its JSON text
//! diff (`app/src/lib/git/diff.ts`; desktop#5279, desktop#12121).

pub mod outputs;
pub mod script;
#[cfg(test)]
mod tests;
pub mod text;

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};

use corvene_models::{NbCell, NbCellKind, NbOutput, NotebookBlob};
use serde::{Deserialize, Serialize};

pub use outputs::{OutputPair, output_pairs};

/// How a cell changed between the two versions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellStatus {
    Same,
    Added,
    Deleted,
    Modified,
    /// The cell now sits here; it was cell `from` (0-based, old version).
    /// It may be edited too ([`CellPair::changes`]).
    Moved {
        from: usize,
    },
    /// The old place of a cell that moved to cell `to` (0-based, new
    /// version).
    MovedAway {
        to: usize,
    },
}

/// What differs within a pair of cells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellChanges {
    pub source: bool,
    pub kind: bool,
    pub execution_count: bool,
    /// Outputs differ other than in their execution counts.
    pub outputs: bool,
    pub metadata: bool,
    pub attachments: bool,
}

impl CellChanges {
    pub fn any(&self) -> bool {
        self.source
            || self.kind
            || self.execution_count
            || self.outputs
            || self.metadata
            || self.attachments
    }

    /// Whether a change shows with `filters` on.
    pub fn visible(&self, filters: &NotebookFilters) -> bool {
        self.source
            || self.kind
            || self.attachments
            || (self.execution_count && !filters.hide_execution_counts)
            || (self.outputs && !filters.hide_outputs)
            || (self.metadata && !filters.hide_metadata)
    }

    /// Changes `filters` hide.
    pub fn hidden(&self, filters: &NotebookFilters) -> Vec<HiddenChange> {
        let mut out = Vec::new();
        if self.execution_count && filters.hide_execution_counts {
            out.push(HiddenChange::ExecutionCount);
        }
        if self.outputs && filters.hide_outputs {
            out.push(HiddenChange::Outputs);
        }
        if self.metadata && filters.hide_metadata {
            out.push(HiddenChange::Metadata);
        }
        out
    }
}

/// A change kind the filters can hide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HiddenChange {
    ExecutionCount,
    Outputs,
    Metadata,
}

impl HiddenChange {
    pub fn label(self) -> &'static str {
        match self {
            HiddenChange::ExecutionCount => "execution count",
            HiddenChange::Outputs => "outputs",
            HiddenChange::Metadata => "metadata",
        }
    }
}

/// One row of the alignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellPair {
    /// Index into the old version's cells.
    pub previous: Option<usize>,
    /// Index into the new version's cells.
    pub current: Option<usize>,
    pub status: CellStatus,
    pub changes: CellChanges,
}

impl CellPair {
    /// Shows as changed with `filters` on (added, deleted and moved cells
    /// always do).
    pub fn is_change(&self, filters: &NotebookFilters) -> bool {
        match self.status {
            CellStatus::Same => false,
            CellStatus::Modified => self.changes.visible(filters),
            _ => true,
        }
    }
}

/// The noise filters of the notebook view (persisted settings).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotebookFilters {
    pub hide_execution_counts: bool,
    pub hide_metadata: bool,
    pub hide_outputs: bool,
    pub collapse_unchanged: bool,
}

impl Default for NotebookFilters {
    fn default() -> Self {
        Self {
            hide_execution_counts: true,
            hide_metadata: true,
            hide_outputs: false,
            collapse_unchanged: true,
        }
    }
}

/// Counts for the header.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub added: usize,
    pub deleted: usize,
    pub modified: usize,
    pub moved: usize,
    /// Pairs whose outputs changed.
    pub outputs: usize,
    /// Changes the filters hide (cells whose only changes are hidden).
    pub hidden: usize,
}

/// The aligned notebook.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NotebookDiff {
    pub pairs: Vec<CellPair>,
    /// Notebook-level metadata differs.
    pub metadata_changed: bool,
    /// The kernel's display name, when it changed.
    pub kernel: Option<(Option<String>, Option<String>)>,
    /// The language, when it changed.
    pub language: Option<(String, String)>,
    /// `nbformat.nbformat_minor`, when it changed.
    pub format: Option<((u32, u32), (u32, u32))>,
}

impl NotebookDiff {
    pub fn summary(&self, filters: &NotebookFilters) -> Summary {
        let mut s = Summary::default();
        for p in &self.pairs {
            match p.status {
                CellStatus::Added => s.added += 1,
                CellStatus::Deleted => s.deleted += 1,
                CellStatus::Moved { .. } => s.moved += 1,
                CellStatus::Modified if p.changes.visible(filters) => s.modified += 1,
                CellStatus::Modified => s.hidden += 1,
                CellStatus::Same | CellStatus::MovedAway { .. } => {}
            }
            if p.changes.outputs && !filters.hide_outputs {
                s.outputs += 1;
            }
        }
        if self.metadata_changed && filters.hide_metadata {
            s.hidden += 1;
        }
        s
    }

    /// Indexes of the pairs that show as changes, for next / previous.
    pub fn change_indices(&self, filters: &NotebookFilters) -> Vec<usize> {
        self.pairs
            .iter()
            .enumerate()
            .filter(|(_, p)| p.is_change(filters))
            .map(|(i, _)| i)
            .collect()
    }
}

/// A row of the view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    /// The notebook's own metadata / kernel / format changes.
    Header,
    Cell(usize),
    /// Unchanged pairs `start..end` folded into one row.
    Folded {
        start: usize,
        end: usize,
    },
}

/// Unchanged cells kept around a change when folding.
const FOLD_CONTEXT: usize = 1;
/// Fewer unchanged cells than this are not folded.
const FOLD_MIN: usize = 3;

/// The rows to draw: `expanded` holds the `start` of folds the user opened.
pub fn rows(diff: &NotebookDiff, filters: &NotebookFilters, expanded: &HashSet<usize>) -> Vec<Row> {
    let mut out = Vec::with_capacity(diff.pairs.len() + 1);
    let header = diff.kernel.is_some()
        || diff.language.is_some()
        || diff.format.is_some()
        || (diff.metadata_changed && !filters.hide_metadata);
    if header {
        out.push(Row::Header);
    }
    if !filters.collapse_unchanged {
        out.extend((0..diff.pairs.len()).map(Row::Cell));
        return out;
    }
    let change: Vec<bool> = diff.pairs.iter().map(|p| p.is_change(filters)).collect();
    let any_change = change.iter().any(|c| *c);
    let mut i = 0;
    while i < diff.pairs.len() {
        if change[i] {
            out.push(Row::Cell(i));
            i += 1;
            continue;
        }
        let start = i;
        while i < diff.pairs.len() && !change[i] {
            i += 1;
        }
        let end = i;
        // keep a cell of context next to each change
        let lead = if start > 0 { FOLD_CONTEXT } else { 0 };
        let trail = if end < diff.pairs.len() {
            FOLD_CONTEXT
        } else {
            0
        };
        let (fold_start, fold_end) = if any_change {
            (start + lead, end.saturating_sub(trail))
        } else {
            (start, end)
        };
        if fold_end <= fold_start
            || fold_end - fold_start < FOLD_MIN
            || expanded.contains(&fold_start)
        {
            out.extend((start..end).map(Row::Cell));
            continue;
        }
        out.extend((start..fold_start).map(Row::Cell));
        out.push(Row::Folded {
            start: fold_start,
            end: fold_end,
        });
        out.extend((fold_end..end).map(Row::Cell));
    }
    out
}

/// Bytes of source compared for similarity.
const MAX_COMPARED: usize = 4_000;
/// An edited cell is looked for this many cells ahead.
const LOOKAHEAD: usize = 24;
/// Similarity for two cells to be one cell edited.
const SIMILAR_CODE: f32 = 0.5;
const SIMILAR_TEXT: f32 = 0.6;
/// Similarity for an added and a deleted cell to be one cell moved.
const MOVED: f32 = 0.9;
/// Larger notebooks skip similarity pairing inside big change stretches.
const MAX_STRETCH_PAIRS: usize = 250_000;

/// Aligns two versions of a notebook (`None` for a side that does not
/// exist).
pub fn diff(previous: Option<&NotebookBlob>, current: Option<&NotebookBlob>) -> NotebookDiff {
    let empty: &[NbCell] = &[];
    let old = previous.map_or(empty, |n| n.cells.as_slice());
    let new = current.map_or(empty, |n| n.cells.as_slice());
    let mut pairs = align(old, new);
    detect_moves(old, new, &mut pairs);
    for p in &mut pairs {
        if let (Some(i), Some(j)) = (p.previous, p.current) {
            p.changes = cell_changes(&old[i], &new[j]);
            if p.status == CellStatus::Same && p.changes.any() {
                p.status = CellStatus::Modified;
            } else if p.status == CellStatus::Modified && !p.changes.any() {
                p.status = CellStatus::Same;
            }
        }
    }
    let (kernel, language, format, metadata_changed) = match (previous, current) {
        (Some(a), Some(b)) => (
            (a.kernel != b.kernel).then(|| (a.kernel.clone(), b.kernel.clone())),
            (!a.language.eq_ignore_ascii_case(&b.language))
                .then(|| (a.language.clone(), b.language.clone())),
            (a.format != b.format).then_some((a.format, b.format)),
            a.metadata != b.metadata,
        ),
        _ => (None, None, None, false),
    };
    NotebookDiff {
        pairs,
        metadata_changed,
        kernel,
        language,
        format,
    }
}

/// What differs between two cells.
pub fn cell_changes(a: &NbCell, b: &NbCell) -> CellChanges {
    CellChanges {
        source: a.source != b.source,
        kind: a.kind != b.kind,
        execution_count: a.execution_count != b.execution_count
            || outputs_counts(&a.outputs) != outputs_counts(&b.outputs),
        outputs: !outputs_equal(&a.outputs, &b.outputs),
        metadata: a.metadata != b.metadata,
        attachments: a.attachments != b.attachments,
    }
}

fn outputs_counts(outputs: &[NbOutput]) -> Vec<Option<u64>> {
    outputs.iter().map(|o| o.execution_count).collect()
}

/// Outputs are equal but for `execute_result` counts.
fn outputs_equal(a: &[NbOutput], b: &[NbOutput]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| outputs::output_key(x) == outputs::output_key(y))
}

fn hash_of(value: impl Hash) -> u64 {
    let mut h = DefaultHasher::new();
    value.hash(&mut h);
    h.finish()
}

/// Unique non-empty ids on every cell of both sides.
fn ids_usable(old: &[NbCell], new: &[NbCell]) -> bool {
    let unique = |cells: &[NbCell]| {
        let mut seen = HashSet::new();
        !cells.is_empty()
            && cells.iter().all(|c| {
                c.id.as_deref()
                    .is_some_and(|id| !id.is_empty() && seen.insert(id))
            })
    };
    unique(old) && unique(new)
}

fn align(old: &[NbCell], new: &[NbCell]) -> Vec<CellPair> {
    let by_id = ids_usable(old, new);
    let key = |c: &NbCell| {
        if by_id {
            hash_of(("id", &c.id))
        } else {
            hash_of((c.kind, &c.source))
        }
    };
    let old_keys: Vec<u64> = old.iter().map(key).collect();
    let new_keys: Vec<u64> = new.iter().map(key).collect();
    let pair = |previous, current, status| CellPair {
        previous,
        current,
        status,
        changes: CellChanges::default(),
    };
    let mut out = Vec::with_capacity(old.len().max(new.len()));
    let ops = similar::capture_diff_slices(similar::Algorithm::Myers, &old_keys, &new_keys);
    let mut ops = ops.into_iter().peekable();
    while let Some(op) = ops.next() {
        let (tag, o, n) = op.as_tag_tuple();
        if tag == similar::DiffTag::Equal {
            out.extend(
                o.zip(n)
                    .map(|(i, j)| pair(Some(i), Some(j), CellStatus::Same)),
            );
            continue;
        }
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
        // with ids, a stretch never holds the same cell twice: only pair
        // by similarity when ids are not used
        let fuzzy = !by_id && olds.len() * news.len() <= MAX_STRETCH_PAIRS;
        let mut j = 0;
        for &i in &olds {
            let end = news.len().min(j + LOOKAHEAD);
            let hit = if fuzzy {
                (j..end).find(|&m| similar_cells(&old[i], &new[news[m]]))
            } else {
                None
            };
            match hit {
                Some(m) => {
                    out.extend(
                        news[j..m]
                            .iter()
                            .map(|&k| pair(None, Some(k), CellStatus::Added)),
                    );
                    out.push(pair(Some(i), Some(news[m]), CellStatus::Modified));
                    j = m + 1;
                }
                None => out.push(pair(Some(i), None, CellStatus::Deleted)),
            }
        }
        out.extend(
            news[j..]
                .iter()
                .map(|&k| pair(None, Some(k), CellStatus::Added)),
        );
    }
    out
}

fn clip(s: &str) -> &str {
    let mut end = s.len().min(MAX_COMPARED);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Source similarity in 0…1 (line based for code, word based for prose).
pub fn similarity(a: &NbCell, b: &NbCell) -> f32 {
    let (x, y) = (clip(&a.source), clip(&b.source));
    if x.trim().is_empty() && y.trim().is_empty() {
        return if a.outputs == b.outputs { 1.0 } else { 0.0 };
    }
    if x.trim().is_empty() || y.trim().is_empty() {
        return 0.0;
    }
    match a.kind {
        NbCellKind::Code => {
            let lines = similar::TextDiff::from_lines(x, y).ratio();
            let chars = similar::TextDiff::from_chars(x, y).ratio();
            lines.max(chars * 0.9)
        }
        _ => similar::TextDiff::from_words(x, y).ratio(),
    }
}

fn similar_cells(a: &NbCell, b: &NbCell) -> bool {
    if a.kind != b.kind {
        // a cell switched between code and markdown keeps its text
        return a.source.trim() == b.source.trim() && !a.source.trim().is_empty();
    }
    let threshold = match a.kind {
        NbCellKind::Code => SIMILAR_CODE,
        _ => SIMILAR_TEXT,
    };
    similarity(a, b) >= threshold
}

/// Turns an added and a deleted cell that are the same cell into a move.
fn detect_moves(old: &[NbCell], new: &[NbCell], pairs: &mut [CellPair]) {
    let deleted: Vec<usize> = pairs
        .iter()
        .enumerate()
        .filter(|(_, p)| p.status == CellStatus::Deleted)
        .map(|(k, _)| k)
        .collect();
    let added: Vec<usize> = pairs
        .iter()
        .enumerate()
        .filter(|(_, p)| p.status == CellStatus::Added)
        .map(|(k, _)| k)
        .collect();
    if deleted.is_empty() || added.is_empty() {
        return;
    }
    // exact matches first: same id, then same kind and source
    let mut by_key: HashMap<u64, Vec<usize>> = HashMap::new();
    let key = |c: &NbCell| match &c.id {
        Some(id) if !id.is_empty() => hash_of(("id", id)),
        _ => hash_of((c.kind, &c.source)),
    };
    for &d in &deleted {
        if let Some(i) = pairs[d].previous
            && !old[i].source.trim().is_empty()
        {
            by_key.entry(key(&old[i])).or_default().push(d);
        }
    }
    let mut taken: HashSet<usize> = HashSet::new();
    let mut matches: Vec<(usize, usize)> = Vec::new();
    for &a in &added {
        let Some(j) = pairs[a].current else { continue };
        if let Some(list) = by_key.get_mut(&key(&new[j]))
            && let Some(pos) = list.iter().position(|d| !taken.contains(d))
        {
            let d = list.remove(pos);
            taken.insert(d);
            matches.push((d, a));
        }
    }
    // then near-identical sources, when the notebook is small enough
    if deleted.len() * added.len() <= 40_000 {
        let matched_added: HashSet<usize> = matches.iter().map(|m| m.1).collect();
        for &a in &added {
            if matched_added.contains(&a) {
                continue;
            }
            let Some(j) = pairs[a].current else { continue };
            let best = deleted
                .iter()
                .filter(|d| !taken.contains(d))
                .filter_map(|&d| {
                    let i = pairs[d].previous?;
                    (old[i].kind == new[j].kind).then(|| (d, similarity(&old[i], &new[j])))
                })
                .filter(|(_, s)| *s >= MOVED)
                .max_by(|x, y| x.1.total_cmp(&y.1));
            if let Some((d, _)) = best {
                taken.insert(d);
                matches.push((d, a));
            }
        }
    }
    for (d, a) in matches {
        let (Some(i), Some(j)) = (pairs[d].previous, pairs[a].current) else {
            continue;
        };
        pairs[a] = CellPair {
            previous: Some(i),
            current: Some(j),
            status: CellStatus::Moved { from: i },
            changes: CellChanges::default(),
        };
        pairs[d] = CellPair {
            previous: Some(i),
            current: None,
            status: CellStatus::MovedAway { to: j },
            changes: CellChanges::default(),
        };
    }
}

/// The macro definitions of all markdown cells of both versions, for the
/// math engine (MathJax in Jupyter keeps a cell's `\newcommand` for the
/// cells after it).
pub fn math_macros_source(
    previous: Option<&NotebookBlob>,
    current: Option<&NotebookBlob>,
) -> String {
    let mut out = String::new();
    for nb in [previous, current].into_iter().flatten() {
        for cell in &nb.cells {
            if cell.kind == NbCellKind::Markdown
                && (cell.source.contains("\\newcommand")
                    || cell.source.contains("\\renewcommand")
                    || cell.source.contains("\\def")
                    || cell.source.contains("\\DeclareMathOperator"))
            {
                out.push_str(&cell.source);
                out.push('\n');
            }
        }
    }
    out
}

/// The metadata of a cell or notebook as `key: value` lines, for a text
/// diff of it.
pub fn metadata_text(metadata: &[(String, String)]) -> String {
    let mut out = String::new();
    for (key, value) in metadata {
        out.push_str(key);
        out.push_str(": ");
        out.push_str(value);
        out.push('\n');
    }
    out
}

/// Jupyter-written keys that change on every run (timings, UI state).
pub fn is_noise_key(key: &str) -> bool {
    matches!(
        key,
        "execution"
            | "ExecuteTime"
            | "collapsed"
            | "scrolled"
            | "papermill"
            | "vscode"
            | "jupyter"
            | "editable"
            | "deletable"
            | "trusted"
            | "pycharm"
            | "id"
    )
}
