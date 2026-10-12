//! The diff of the selected file: a header the screen keeps, and rows in
//! windows for a lazy list. Rows carry the highlight spans
//! (`corvene_highlight`) as UTF-16 ranges so Kotlin builds its
//! `AnnotatedString` without re-tokenizing.

use std::sync::Arc;

use corvene_core::AppState;
use corvene_highlight::TokenClass;
use corvene_models::{Diff, DiffLineKind, DiffSelection};

/// What the selected file's diff is.
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffKindVm {
    /// No file selected, or the diff has not loaded yet.
    Loading,
    Text,
    /// A text diff over the size GHD shows at once ("Show Diff" first).
    LargeText,
    Binary,
    Image,
    Empty,
    TooLarge,
    Submodule,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct DiffHeaderVm {
    pub repo: u64,
    pub path: Option<String>,
    pub kind: DiffKindVm,
    /// Matches `ChangesVm::diff_generation`; rows are addressed by it.
    pub generation: u64,
    /// Hunk headers included.
    pub row_count: u32,
    pub hunk_count: u32,
    pub lines_added: u32,
    pub lines_deleted: u32,
    /// The whole file is included / partially / not at all.
    pub include: super::changes::IncludeVm,
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffRowKindVm {
    Hunk,
    Context,
    Add,
    Delete,
}

/// A highlighted run inside the row's text (UTF-16 indices).
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct SpanVm {
    pub start: u32,
    pub end: u32,
    pub class: TokenClassVm,
}

/// `corvene_highlight::TokenClass`, one to one.
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenClassVm {
    Variable,
    AltVariable,
    Keyword,
    Atom,
    String,
    Qualifier,
    Type,
    Comment,
    Tag,
    Attribute,
    Link,
    Header,
    Quote,
}

impl From<TokenClass> for TokenClassVm {
    fn from(class: TokenClass) -> Self {
        match class {
            TokenClass::Variable => TokenClassVm::Variable,
            TokenClass::AltVariable => TokenClassVm::AltVariable,
            TokenClass::Keyword => TokenClassVm::Keyword,
            TokenClass::Atom => TokenClassVm::Atom,
            TokenClass::String => TokenClassVm::String,
            TokenClass::Qualifier => TokenClassVm::Qualifier,
            TokenClass::Type => TokenClassVm::Type,
            TokenClass::Comment => TokenClassVm::Comment,
            TokenClass::Tag => TokenClassVm::Tag,
            TokenClass::Attribute => TokenClassVm::Attribute,
            TokenClass::Link => TokenClassVm::Link,
            TokenClass::Header => TokenClassVm::Header,
            TokenClass::Quote => TokenClassVm::Quote,
        }
    }
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct DiffRowVm {
    /// Index in the unified diff (hunk headers count), the key of the row.
    pub index: u32,
    pub kind: DiffRowKindVm,
    pub text: String,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    /// Add/Delete rows: this line is included in the next commit.
    pub selected: bool,
    pub no_trailing_newline: bool,
    pub spans: Vec<SpanVm>,
}

fn kind_of(diff: Option<&Arc<Diff>>, loading: bool) -> DiffKindVm {
    match diff.map(|d| d.as_ref()) {
        None if loading => DiffKindVm::Loading,
        None => DiffKindVm::Loading,
        Some(Diff::Text { .. }) => DiffKindVm::Text,
        Some(Diff::LargeText { .. }) => DiffKindVm::LargeText,
        // Corvene `1323-archive-diff`: the Android app shows archives as
        // binary files
        Some(Diff::Binary | Diff::Table { .. } | Diff::Archive(_)) => DiffKindVm::Binary,
        Some(Diff::Image { .. }) => DiffKindVm::Image,
        Some(Diff::Font { .. } | Diff::FontSource { .. }) => DiffKindVm::Binary,
        Some(Diff::Empty) => DiffKindVm::Empty,
        Some(Diff::TooLarge) => DiffKindVm::TooLarge,
        Some(Diff::Submodule(_)) => DiffKindVm::Submodule,
        // `1324-document-diff`: the Android app has no document view yet;
        // an RTF file shows git's own diff
        Some(Diff::Document {
            source: Some(_), ..
        }) => DiffKindVm::Text,
        Some(Diff::Document { .. }) => DiffKindVm::Binary,
        // `1331-notebook-diff`, `1332-latex-diff`: git's own diff
        Some(Diff::Notebook { .. } | Diff::Latex { .. }) => DiffKindVm::Text,
    }
}

pub fn diff_header(s: &AppState, repo: u64) -> Option<DiffHeaderVm> {
    let rs = s.repo_states.get(&repo)?;
    let diff = rs.diff.as_ref();
    let (rows, hunks, added, deleted) = diff
        .and_then(|d| d.hunks())
        .map(|hunks| {
            // a hunk's first line is its header
            let rows = hunks.iter().map(|h| h.lines.len()).sum::<usize>();
            let added = hunks
                .iter()
                .flat_map(|h| h.lines.iter())
                .filter(|l| l.kind == DiffLineKind::Add)
                .count();
            let deleted = hunks
                .iter()
                .flat_map(|h| h.lines.iter())
                .filter(|l| l.kind == DiffLineKind::Delete)
                .count();
            (rows, hunks.len(), added, deleted)
        })
        .unwrap_or((0, 0, 0, 0));
    let include = rs
        .status
        .as_ref()
        .and_then(|status| {
            status
                .files
                .iter()
                .find(|f| Some(&f.path) == rs.selected_file.as_ref())
        })
        .map(|f| f.selection.kind().into())
        .unwrap_or(super::changes::IncludeVm::All);
    Some(DiffHeaderVm {
        repo,
        path: rs.selected_file.clone(),
        kind: kind_of(diff, rs.diff_loading),
        generation: rs.diff_generation,
        row_count: u32::try_from(rows).unwrap_or(u32::MAX),
        hunk_count: u32::try_from(hunks).unwrap_or(u32::MAX),
        lines_added: u32::try_from(added).unwrap_or(u32::MAX),
        lines_deleted: u32::try_from(deleted).unwrap_or(u32::MAX),
        include,
    })
}

/// Rows `start..start+count` of the selected file's diff, or an empty list
/// when `generation` is stale (the screen re-queries the header first).
pub fn diff_rows(
    s: &AppState,
    repo: u64,
    generation: u64,
    start: u32,
    count: u32,
) -> Vec<DiffRowVm> {
    let Some(rs) = s.repo_states.get(&repo) else {
        return Vec::new();
    };
    if rs.diff_generation != generation {
        return Vec::new();
    }
    let Some(diff) = rs.diff.as_ref() else {
        return Vec::new();
    };
    let selection = rs
        .status
        .as_ref()
        .and_then(|status| {
            status
                .files
                .iter()
                .find(|f| Some(&f.path) == rs.selected_file.as_ref())
        })
        .map(|f| f.selection.clone())
        .unwrap_or_else(DiffSelection::all);
    rows_of(
        diff,
        Some(&selection),
        rs.selected_file.as_deref().unwrap_or(""),
        start,
        count,
    )
}

/// Rows of the selected commit's selected file (History tab); nothing is
/// selectable there.
pub fn commit_diff_rows(
    s: &AppState,
    repo: u64,
    generation: u64,
    start: u32,
    count: u32,
) -> Vec<DiffRowVm> {
    let Some(rs) = s.repo_states.get(&repo) else {
        return Vec::new();
    };
    if rs.commit_diff_generation != generation {
        return Vec::new();
    }
    let Some(diff) = rs.commit_diff.as_ref() else {
        return Vec::new();
    };
    rows_of(
        diff,
        None,
        rs.commit_selected_file.as_deref().unwrap_or(""),
        start,
        count,
    )
}

fn rows_of(
    diff: &Diff,
    selection: Option<&DiffSelection>,
    path: &str,
    start: u32,
    count: u32,
) -> Vec<DiffRowVm> {
    let Some(hunks) = diff.hunks() else {
        return Vec::new();
    };
    let start = start as usize;
    let end = start.saturating_add(count as usize);

    // the rows in the window; a hunk's first line is its header and the
    // unified-diff index of a line is what the selection counts
    let mut rows = Vec::with_capacity(count as usize);
    for hunk in hunks {
        for (i, line) in hunk.lines.iter().enumerate() {
            let index = hunk.unified_diff_start as usize + i;
            if index < start {
                continue;
            }
            if index >= end {
                break;
            }
            let kind = match line.kind {
                DiffLineKind::Context => DiffRowKindVm::Context,
                DiffLineKind::Add => DiffRowKindVm::Add,
                DiffLineKind::Delete => DiffRowKindVm::Delete,
                DiffLineKind::Hunk => DiffRowKindVm::Hunk,
            };
            let line_index = u32::try_from(index).unwrap_or(u32::MAX);
            rows.push(DiffRowVm {
                index: line_index,
                kind,
                text: if kind == DiffRowKindVm::Hunk {
                    hunk.header.clone()
                } else {
                    line.text.clone()
                },
                old_line: line.old_line,
                new_line: line.new_line,
                selected: matches!(kind, DiffRowKindVm::Add | DiffRowKindVm::Delete)
                    && selection.is_some_and(|sel| sel.is_selected(line_index)),
                no_trailing_newline: line.no_trailing_newline,
                spans: Vec::new(),
            });
        }
    }
    highlight(path, &mut rows);
    rows
}

/// Syntax spans for the window's rows, each row tokenized on its own (the
/// desktop highlights the file's lines the same way, one line at a time
/// through `highlight_lines`).
fn highlight(path: &str, rows: &mut [DiffRowVm]) {
    if path.is_empty() || rows.is_empty() {
        return;
    }
    let lines: Vec<&str> = rows.iter().map(|r| r.text.as_str()).collect();
    let Some(spans) = corvene_highlight::highlight_lines(path, lines.iter().copied()) else {
        return;
    };
    for (row, line_spans) in rows.iter_mut().zip(spans) {
        if row.kind == DiffRowKindVm::Hunk {
            continue;
        }
        // byte offsets → UTF-16 units
        let text = row.text.as_str();
        let utf16_at = |byte: usize| -> u32 {
            u32::try_from(text[..byte.min(text.len())].encode_utf16().count()).unwrap_or(u32::MAX)
        };
        row.spans = line_spans
            .into_iter()
            .map(|span| SpanVm {
                start: utf16_at(span.range.start),
                end: utf16_at(span.range.end),
                class: span.class.into(),
            })
            .collect();
    }
}
