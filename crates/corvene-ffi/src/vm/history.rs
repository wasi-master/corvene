//! The History tab: the commit list in windows, the selected commit's
//! files, and the selected file's diff header (rows come through
//! `diff_rows` with `commit = true`).

use corvene_core::AppState;
use corvene_models::{Diff, DiffLineKind};

use super::changes::FileStatusVm;
use super::diff::DiffKindVm;

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct CommitVm {
    pub sha: String,
    pub summary: String,
    pub author_name: String,
    pub author_email: String,
    /// Unix seconds of the author date.
    pub authored_at: i64,
    pub tags: Vec<String>,
    pub selected: bool,
    pub is_merge: bool,
}

/// The branch comparison on the History tab (GHD `ICompareBranch`).
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct CompareVm {
    pub branch: String,
    /// "behind" = their commits, "ahead" = ours.
    pub mode: String,
    pub ahead: u32,
    pub behind: u32,
    /// "clean", "conflicts", "invalid" or "loading".
    pub merge_status: String,
    pub merge_conflicts: u32,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct HistoryVm {
    pub repo: u64,
    pub compare: Option<CompareVm>,
    pub loading: bool,
    /// No more commits past the ones loaded (`load_more` is a no-op).
    pub exhausted: bool,
    pub total_loaded: u32,
    pub selected: Vec<String>,
    /// The window asked for.
    pub start: u32,
    pub commits: Vec<CommitVm>,
}

pub fn history(s: &AppState, repo: u64, start: u32, count: u32) -> Option<HistoryVm> {
    let rs = s.repo_states.get(&repo)?;
    let all = rs.visible_commits();
    let commits = all
        .iter()
        .skip(start as usize)
        .take(count as usize)
        .map(|c| CommitVm {
            sha: c.sha.clone(),
            summary: c.summary.clone(),
            author_name: c.author.name.clone(),
            author_email: c.author.email.clone(),
            authored_at: c.author.seconds,
            tags: c.tags.clone(),
            selected: rs.selected_commits.iter().any(|sha| sha == &c.sha)
                || rs.selected_commit.as_deref() == Some(&c.sha),
            is_merge: c.parents.len() > 1,
        })
        .collect();
    let compare = match &rs.compare.form {
        corvene_core::compare::CompareForm::Branch {
            branch,
            mode,
            ahead_behind,
        } => {
            let (merge_status, merge_conflicts) = match rs.compare.merge_status {
                Some(corvene_models::Mergeability::Clean) => ("clean", 0),
                Some(corvene_models::Mergeability::Conflicts(n)) => ("conflicts", n),
                Some(corvene_models::Mergeability::Invalid) => ("invalid", 0),
                None => ("loading", 0),
            };
            Some(CompareVm {
                branch: branch.clone(),
                mode: match mode {
                    corvene_core::compare::ComparisonMode::Behind => "behind".into(),
                    corvene_core::compare::ComparisonMode::Ahead => "ahead".into(),
                },
                ahead: ahead_behind.ahead,
                behind: ahead_behind.behind,
                merge_status: merge_status.into(),
                merge_conflicts,
            })
        }
        // `1218-compare-refs` is not offered on Android
        corvene_core::compare::CompareForm::History
        | corvene_core::compare::CompareForm::Refs { .. } => None,
    };
    Some(HistoryVm {
        repo,
        compare,
        loading: rs.commits_loading,
        exhausted: rs.commits_exhausted,
        total_loaded: u32::try_from(all.len()).unwrap_or(u32::MAX),
        selected: if rs.selected_commits.is_empty() {
            rs.selected_commit.iter().cloned().collect()
        } else {
            rs.selected_commits.clone()
        },
        start,
        commits,
    })
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct CommitFileVm {
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatusVm,
    pub selected: bool,
}

/// The selected commit (or range) on the History tab.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct CommitDetailVm {
    pub repo: u64,
    pub shas: Vec<String>,
    pub summary: String,
    pub body: String,
    pub author_name: String,
    pub author_email: String,
    pub authored_at: i64,
    pub files: Vec<CommitFileVm>,
    pub lines_added: u64,
    pub lines_deleted: u64,
    pub selected_file: Option<String>,
    /// The selected file's diff: kind and generation for `diff_rows`.
    pub diff_kind: DiffKindVm,
    pub diff_generation: u64,
    pub diff_row_count: u32,
    pub diff_lines_added: u32,
    pub diff_lines_deleted: u32,
}

pub fn commit_detail(s: &AppState, repo: u64) -> Option<CommitDetailVm> {
    let rs = s.repo_states.get(&repo)?;
    let shas: Vec<String> = if rs.selected_commits.is_empty() {
        rs.selected_commit.iter().cloned().collect()
    } else {
        rs.selected_commits.clone()
    };
    let first = shas.first()?;
    let commit = rs.visible_commits().iter().find(|c| &c.sha == first)?;
    let files = rs
        .changeset
        .as_ref()
        .map(|cs| {
            cs.files
                .iter()
                .map(|f| CommitFileVm {
                    path: f.path.clone(),
                    old_path: f.old_path.clone(),
                    status: f.status.kind.into(),
                    selected: rs.commit_selected_file.as_deref() == Some(&f.path),
                })
                .collect()
        })
        .unwrap_or_default();
    let diff = rs.commit_diff.as_ref().map(|d| d.as_ref());
    let (rows, added, deleted) = diff
        .and_then(|d| d.hunks())
        .map(|hunks| {
            let rows = hunks.iter().map(|h| h.lines.len()).sum::<usize>();
            let lines = hunks.iter().flat_map(|h| h.lines.iter());
            let added = lines
                .clone()
                .filter(|l| l.kind == DiffLineKind::Add)
                .count();
            let deleted = lines.filter(|l| l.kind == DiffLineKind::Delete).count();
            (rows, added, deleted)
        })
        .unwrap_or((0, 0, 0));
    let diff_kind = match diff {
        None => DiffKindVm::Loading,
        Some(Diff::Text { .. }) => DiffKindVm::Text,
        Some(Diff::LargeText { .. }) => DiffKindVm::LargeText,
        Some(Diff::Binary | Diff::Table { .. }) => DiffKindVm::Binary,
        Some(Diff::Image { .. }) => DiffKindVm::Image,
        Some(Diff::Empty) => DiffKindVm::Empty,
        Some(Diff::TooLarge) => DiffKindVm::TooLarge,
        Some(Diff::Submodule(_)) => DiffKindVm::Submodule,
    };
    Some(CommitDetailVm {
        repo,
        shas,
        summary: commit.summary.clone(),
        body: commit.body.clone(),
        author_name: commit.author.name.clone(),
        author_email: commit.author.email.clone(),
        authored_at: commit.author.seconds,
        files,
        lines_added: rs.changeset.as_ref().map(|c| c.lines_added).unwrap_or(0),
        lines_deleted: rs.changeset.as_ref().map(|c| c.lines_deleted).unwrap_or(0),
        selected_file: rs.commit_selected_file.clone(),
        diff_kind,
        diff_generation: rs.commit_diff_generation,
        diff_row_count: u32::try_from(rows).unwrap_or(u32::MAX),
        diff_lines_added: u32::try_from(added).unwrap_or(u32::MAX),
        diff_lines_deleted: u32::try_from(deleted).unwrap_or(u32::MAX),
    })
}
