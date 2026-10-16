//! Multi-commit operations (GHD `IMultiCommitOperationState`,
//! `models/multi-commit-operation.ts`, `models/banner.ts` and the
//! `_rebase/_cherryPick/_squash/_reorderCommits/_mergeBranch` flows in
//! `app-store.ts` + `dispatcher.ts`): rebase, cherry-pick, squash, reorder
//! and merge, with the shared progress / conflicts / abort steps and the
//! banners shown when they finish.
//!
//! Corvene addition (`799-fixup-commits`): a fixup commit made from the
//! commit form offers to autosquash ([`Dispatcher::autosquash`], an
//! `McoDetail::Autosquash` run like a squash).
//! Deviation: while the repository is still conflicted, a new merge, rebase
//! or update from the default branch is refused with an explanation (GHD
//! starts it and shows git's error; `838-no-merge-while-conflicted`).
//! Deviation: with `878-conflicts-open-as-banner`, conflicts found by an
//! operation show the conflicts banner instead of opening the conflicts
//! dialog (GHD `startMultiCommitOperationConflictFlow` opens the dialog).
//! Deviation (`1206-explain-merge-abort-failure`): when `git merge --abort`
//! refuses because files changed after the merge started, the error names
//! them and says what to do ([`merge_abort_blocked_message`]); GHD shows
//! git's output.
//!
//! Deviation: [`derive_conflict_state`] drops the manual resolutions when
//! the kind of conflict changes between two status reads (a merge aborted
//! and a cherry-pick started outside Corvene); GHD
//! (`lib/stores/updates/changes-state.ts` `updateConflictState`) carries
//! them over, marking same-named files of the new operation resolved.
//!
//! The status refresh reopens the conflicts dialog when GHD's
//! `_triggerConflictsFlow` would ([`is_conflicts_flow`]), except that it
//! leaves an operation's open progress dialog alone (the operation moves it
//! to the conflicts step itself once git reports conflicts; GHD switches it
//! on any status read that finds the conflict state).

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::host::{AsyncCtx, Host};
use corvene_git::{CherryPickResult, RebaseResult};
use corvene_models::{
    Branch, BranchKind, Commit, CommitOneLine, ManualConflictResolution, McoProgress, Mergeability,
    MultiCommitOperationKind, Section, WorkingDirectoryFileChange, WorkingDirectoryStatus,
};
use tracing::{info, warn};

use crate::dispatcher::Dispatcher;
use crate::stacked_refs::{StackedBranches, StackedOp};
use crate::state::{Popup, RepositoryState, RetryAction};

/// `MultiCommitOperationStepKind`
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McoStep {
    ChooseBranch,
    WarnForcePush,
    ShowProgress,
    ShowConflicts,
    /// The conflicts dialog was dismissed; a banner offers to reopen it.
    HideConflicts,
    ConfirmAbort,
    /// Cherry-pick to a new branch (`CreateBranchStep`).
    CreateBranch {
        initial_name: String,
    },
}

/// `MultiCommitOperationDetail`
#[derive(Clone, Debug, PartialEq)]
pub enum McoDetail {
    Rebase {
        /// The branch chosen to rebase onto (`sourceBranch`).
        base_branch: Option<String>,
        commits: Vec<CommitOneLine>,
    },
    CherryPick {
        source_branch: Option<String>,
        branch_created: bool,
        commits: Vec<CommitOneLine>,
    },
    Squash {
        commits: Vec<Commit>,
        target_commit: Commit,
        last_retained_ref: Option<String>,
        message: String,
        /// Corvene `1221-stacked-branch-refs`: the other branches moved
        /// with their commits.
        update_refs: corvene_git::UpdateRefs,
    },
    Reorder {
        commits: Vec<Commit>,
        before_commit: Option<Commit>,
        last_retained_ref: Option<String>,
        /// Corvene `1221-stacked-branch-refs`
        update_refs: corvene_git::UpdateRefs,
    },
    /// Corvene `799-fixup-commits`: fold the branch's `fixup!` commits
    /// (`commits`) into the commits they fix. A squash to the user (its
    /// dialogs, banners and Undo); `count` is the commits involved.
    Autosquash {
        commits: Vec<Commit>,
        last_retained_ref: Option<String>,
        count: usize,
        /// Corvene `1221-stacked-branch-refs`
        update_refs: corvene_git::UpdateRefs,
    },
    Merge {
        squash: bool,
        source_branch: Option<String>,
    },
}

impl McoDetail {
    pub fn kind(&self) -> MultiCommitOperationKind {
        match self {
            McoDetail::Rebase { .. } => MultiCommitOperationKind::Rebase,
            McoDetail::CherryPick { .. } => MultiCommitOperationKind::CherryPick,
            McoDetail::Squash { .. } => MultiCommitOperationKind::Squash,
            McoDetail::Reorder { .. } => MultiCommitOperationKind::Reorder,
            McoDetail::Autosquash { .. } => MultiCommitOperationKind::Squash,
            McoDetail::Merge { .. } => MultiCommitOperationKind::Merge,
        }
    }
}

/// `MultiCommitOperationConflictState` minus the resolutions, which live in
/// `RepositoryState::conflict_state`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McoConflicts {
    pub our_branch: Option<String>,
    pub their_branch: Option<String>,
}

/// `IMultiCommitOperationState`
#[derive(Clone, Debug, PartialEq)]
pub struct MultiCommitOperation {
    pub step: McoStep,
    pub detail: McoDetail,
    pub progress: McoProgress,
    pub user_has_resolved_conflicts: bool,
    pub original_branch_tip: Option<String>,
    /// Cherry-pick: the branch commits are copied to; otherwise the current branch.
    pub target_branch: Option<String>,
    pub conflicts: McoConflicts,
}

impl MultiCommitOperation {
    pub fn kind(&self) -> MultiCommitOperationKind {
        self.detail.kind()
    }

    /// The operation is waiting on its conflicts: one of GHD's
    /// [`CONFLICT_STEPS`] or the dismissed conflicts dialog
    /// (`HideConflicts`, which `conflictSteps` leaves out). The status
    /// refresh ends such an operation once the conflicts are gone.
    pub fn in_conflict_step(&self) -> bool {
        self.step == McoStep::HideConflicts || CONFLICT_STEPS.contains(&self.step)
    }
}

/// GHD `conflictSteps` (`models/multi-commit-operation.ts`; Corvene has no
/// Copilot conflict steps).
pub const CONFLICT_STEPS: [McoStep; 2] = [McoStep::ShowConflicts, McoStep::ConfirmAbort];

/// GHD `isConflictsFlow` (`lib/multi-commit-operation.ts`): the
/// multi-commit operation dialog is open at one of [`CONFLICT_STEPS`].
pub fn is_conflicts_flow(
    is_multi_commit_operation_popup_open: bool,
    multi_commit_operation_state: Option<&MultiCommitOperation>,
) -> bool {
    is_multi_commit_operation_popup_open
        && multi_commit_operation_state.is_some_and(|m| CONFLICT_STEPS.contains(&m.step))
}

/// GHD `ChooseBranchStep` (`models/multi-commit-operation.ts`): what the
/// choose-branch step of a rebase or cherry-pick starts from. Corvene's
/// `McoStep::ChooseBranch` carries none of it; its dialog reads the
/// branches from the repository state.
#[derive(Clone, Debug, PartialEq)]
pub struct ChooseBranchStep {
    pub kind: McoStep,
    pub default_branch: Option<Branch>,
    pub current_branch: Branch,
    pub all_branches: Vec<Branch>,
    pub recent_branches: Vec<Branch>,
}

/// Why a multi-commit operation cannot start.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum McoError {
    #[error("Tip is not in a valid state, which is required to start the multi commit operation")]
    TipNotValid,
}

/// GHD `getMultiCommitOperationChooseBranchStep(state)`
/// (`lib/multi-commit-operation.ts`): the choose-branch step with the
/// current branch (from a valid tip), the default branch, every branch and
/// the recent ones (`branchesState`). An error when the tip is not a valid
/// branch (GHD throws).
pub fn get_multi_commit_operation_choose_branch_step(
    state: &RepositoryState,
) -> Result<ChooseBranchStep, McoError> {
    let info = state.info.as_ref().ok_or(McoError::TipNotValid)?;
    let current_branch = info.current_branch().ok_or(McoError::TipNotValid)?.clone();
    // a local branch of that name first, as `branchesState` holds them
    let find = |name: &str| {
        info.branches
            .iter()
            .filter(|b| b.name == name)
            .min_by_key(|b| b.kind != BranchKind::Local)
            .cloned()
    };
    Ok(ChooseBranchStep {
        kind: McoStep::ChooseBranch,
        default_branch: state.default_branch.as_deref().and_then(find),
        current_branch,
        all_branches: info.branches.clone(),
        recent_branches: state
            .recent_branches
            .iter()
            .filter_map(|name| find(name))
            .collect(),
    })
}

/// `IMultiCommitOperationUndoState` plus what the undo needs to know.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McoUndo {
    pub sha: String,
    pub branch: String,
    pub kind: MultiCommitOperationKind,
    pub count: usize,
    pub source_branch: Option<String>,
    pub branch_created: bool,
    /// Corvene `1221-stacked-branch-refs`: other branches the operation
    /// moved, as (full ref, the commit to put back).
    pub moved_branches: Vec<(String, String)>,
}

/// `ConflictState` kinds derived from the repository (`MERGE_HEAD`,
/// `.git/rebase-merge`, `CHERRY_PICK_HEAD`, `SQUASH_MSG`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConflictKind {
    Merge {
        current_branch: String,
        current_tip: String,
    },
    Rebase {
        /// The tip while the rebase is conflicted (`currentTip`).
        current_tip: String,
        target_branch: String,
        base_branch_tip: String,
        original_branch_tip: String,
    },
    CherryPick {
        target_branch: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictState {
    pub kind: ConflictKind,
    pub manual_resolutions: BTreeMap<String, ManualConflictResolution>,
}

/// GHD `updateConflictState` / `getConflictState`
/// (`lib/stores/updates/changes-state.ts`): a rebase (which needs the
/// current tip), else a cherry-pick (which needs the current branch), else a
/// merge (`MERGE_HEAD`, or `SQUASH_MSG` with conflicted files, on a branch
/// with a tip). The resolutions of `previous` are kept while the kind stays
/// the same (see the module doc).
pub fn derive_conflict_state(
    status: &WorkingDirectoryStatus,
    previous: Option<&ConflictState>,
) -> Option<ConflictState> {
    let kind = if let Some(rebase) = &status.rebase_internal_state {
        ConflictKind::Rebase {
            current_tip: status.current_tip.clone()?,
            target_branch: rebase.target_branch.clone(),
            base_branch_tip: rebase.base_branch_tip.clone(),
            original_branch_tip: rebase.original_branch_tip.clone(),
        }
    } else if status.cherry_pick_head_found {
        ConflictKind::CherryPick {
            target_branch: status.branch.clone()?,
        }
    } else if status.merge_head_found || (status.squash_msg_found && status.has_conflicts()) {
        ConflictKind::Merge {
            current_branch: status.branch.clone()?,
            current_tip: status.current_tip.clone()?,
        }
    } else {
        return None;
    };
    let same_kind = previous
        .map(|p| std::mem::discriminant(&p.kind) == std::mem::discriminant(&kind))
        .unwrap_or(false);
    Some(ConflictState {
        manual_resolutions: if same_kind {
            previous
                .map(|p| p.manual_resolutions.clone())
                .unwrap_or_default()
        } else {
            BTreeMap::new()
        },
        kind,
    })
}

/// `1206-explain-merge-abort-failure`: why `git merge --abort` refused,
/// naming the files that changed after the merge started (at most five).
pub fn merge_abort_blocked_message(paths: &[String]) -> String {
    let shown: Vec<&str> = paths.iter().take(5).map(String::as_str).collect();
    let names = match shown.as_slice() {
        [one] if paths.len() == 1 => one.to_string(),
        [rest @ .., last] if paths.len() == shown.len() => {
            format!("{} and {last}", rest.join(", "))
        }
        _ => format!(
            "{} and {} more",
            shown.join(", "),
            paths.len() - shown.len()
        ),
    };
    let them = if paths.len() == 1 { "it" } else { "them" };
    format!(
        "Could not abort the merge because {names} changed after the merge started. Discard or \
         stash the changes to {them}, then abort again."
    )
}

/// GHD `getSquashedCommitDescription(commits, squashOnto)`
/// (`lib/squash/squashed-commit-description.ts`): the squash dialog's
/// prefilled description. `squash_onto`'s body, then each of `commits`'
/// summary and body (joined by a blank line even when the body is empty),
/// all trimmed and without their `Co-Authored-By` trailers; blank parts
/// dropped, the rest joined by blank lines.
pub fn get_squashed_commit_description(commits: &[Commit], squash_onto: &Commit) -> String {
    let commit_messages = commits
        .iter()
        .map(|c| format!("{}\n\n{}", c.summary.trim(), c.body_no_co_authors().trim()));
    std::iter::once(squash_onto.body_no_co_authors().trim().to_string())
        .chain(commit_messages)
        .filter(|d| !d.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// GHD `getUnmergedFiles`
pub fn unmerged_files(status: &WorkingDirectoryStatus) -> Vec<&WorkingDirectoryFileChange> {
    status
        .files
        .iter()
        .filter(|f| f.status.is_conflicted())
        .collect()
}

/// GHD `getUniqueCoauthorsAsAuthors` (`lib/unique-coauthors-as-authors.ts`):
/// the `Co-Authored-By` co-authors of `commits` as known authors without a
/// login, the first of each name and email pair. GHD prefills the squash
/// dialog's co-authors with the squashed commits' ones; Corvene's squash
/// dialog has no co-authors field yet (`.docs/TODO.md`).
pub fn get_unique_coauthors_as_authors(commits: &[Commit]) -> Vec<corvene_models::Author> {
    let mut unique: Vec<corvene_models::GitAuthor> = Vec::new();
    for author in commits.iter().flat_map(Commit::co_authors) {
        if !unique
            .iter()
            .any(|a| a.email == author.email && a.name == author.name)
        {
            unique.push(author);
        }
    }
    unique
        .into_iter()
        .map(|a| corvene_models::Author::Known {
            name: a.name,
            email: a.email,
            username: None,
        })
        .collect()
}

/// GHD `getConflictedFiles`: unmerged files that still need work.
pub fn conflicted_files<'a>(
    status: &'a WorkingDirectoryStatus,
    resolutions: &BTreeMap<String, ManualConflictResolution>,
) -> Vec<&'a WorkingDirectoryFileChange> {
    unmerged_files(status)
        .into_iter()
        .filter(|f| {
            f.status
                .has_unresolved_conflicts(resolutions.get(&f.path).copied())
        })
        .collect()
}

/// GHD `getResolvedFiles`
pub fn resolved_files<'a>(
    status: &'a WorkingDirectoryStatus,
    resolutions: &BTreeMap<String, ManualConflictResolution>,
) -> Vec<&'a WorkingDirectoryFileChange> {
    unmerged_files(status)
        .into_iter()
        .filter(|f| {
            !f.status
                .has_unresolved_conflicts(resolutions.get(&f.path).copied())
        })
        .collect()
}

/// `Banner`
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Banner {
    SuccessfulMerge {
        our_branch: String,
        their_branch: Option<String>,
    },
    SuccessfulRebase {
        target_branch: String,
        base_branch: Option<String>,
    },
    BranchAlreadyUpToDate {
        our_branch: String,
        their_branch: Option<String>,
    },
    SuccessfulCherryPick {
        repo: u64,
        target_branch: String,
        count: usize,
        /// Offers Undo (not after `894-cherry-pick-into-worktree-branch`
        /// copied them in another worktree).
        undoable: bool,
    },
    CherryPickUndone {
        target_branch: String,
        count: usize,
    },
    SuccessfulSquash {
        repo: u64,
        count: usize,
    },
    SquashUndone {
        count: usize,
    },
    /// Corvene (`799-fixup-commits`): "Created a fixup commit for
    /// **{target}**" + Squash Now, which autosquashes the branch.
    FixupCommitted {
        repo: u64,
        target: String,
    },
    SuccessfulReorder {
        repo: u64,
        count: usize,
    },
    ReorderUndone {
        count: usize,
    },
    /// Corvene (`345-issues`): "Created issue **#N**" + View on GitHub.
    IssueCreated {
        number: u64,
        html_url: String,
    },
    /// Corvene (`348-pull-request-review`): "Submitted review of pull
    /// request **#N**" (approved / changes requested / commented) + View on
    /// GitHub.
    ReviewSubmitted {
        number: u64,
        event: corvene_github::review::ReviewEvent,
        html_url: String,
    },
    /// Corvene (`346-releases`): "Created release **{name}**" (or "Saved
    /// draft release") + View on GitHub.
    ReleaseCreated {
        name: String,
        html_url: String,
        draft: bool,
    },
    /// Corvene (`861-undo-delete-branch`): "Deleted branch **{branch}**" +
    /// Undo, which recreates it at `sha`.
    BranchDeleted {
        repo: u64,
        branch: String,
        sha: String,
    },
    /// Corvene (`861-undo-delete-branch`): after that Undo.
    BranchRestored {
        branch: String,
    },
    /// Corvene (`895-bulk-delete-branches` with `861-undo-delete-branch`):
    /// "Deleted N branches" + Undo, which recreates each (name, sha).
    BranchesDeleted {
        repo: u64,
        branches: Vec<(String, String)>,
    },
    /// After that Undo.
    BranchesRestored {
        count: usize,
    },
    /// Corvene (`797-stash-list`): "Discarded stash" + Undo, which stores
    /// the entry's commit `sha` again with its `message`.
    StashDropped {
        repo: u64,
        sha: String,
        message: String,
    },
    /// Corvene (`797-stash-list`): after that Undo.
    StashRestored,
    /// Corvene (`1315-discard-stash-file`): "Discarded <path> from the
    /// stash" + Undo, which puts stash commit `sha` back with `message` at
    /// `position`, in place of `replacing` (the rewritten entry; `None`
    /// when the discard dropped the entry).
    StashFileDiscarded {
        repo: u64,
        path: String,
        sha: String,
        message: String,
        position: usize,
        replacing: Option<String>,
    },
    /// Corvene (`1106-apply-patch`): "Committed N patches", "Applied the
    /// patch to N files", or with `conflicts` "… Resolve the conflicts in N
    /// files."
    PatchApplied {
        files: usize,
        commits: usize,
        conflicts: usize,
    },
    /// "Resolve conflicts to continue {description} **{branch}**."
    ConflictsFound {
        repo: u64,
        description: String,
        branch: Option<String>,
    },
    /// Corvene (`322-git-email-mismatch-banner`): after signing in, the
    /// global `user.email` is unset (`missing`) or not one of the account's
    /// addresses on `host`; links to Settings › Git.
    GitEmailMismatch {
        host: String,
        missing: bool,
    },
    /// Corvene (`287-repository-list-backup`): `count` entries of the stored
    /// repository list could not be read and were set aside, or (`raw`) the
    /// whole list was not readable and was backed up.
    RepositoriesUnreadable {
        count: usize,
        raw: bool,
    },
    /// Corvene (`294-follow-moved-repositories`): the folder of repository
    /// `name` was moved; Corvene follows it to `path`.
    RepositoryMoved {
        name: String,
        path: std::path::PathBuf,
    },
    /// Corvene (`287-repository-list-backup`): the store at `path` could not
    /// be opened, so this session uses a temporary one.
    TemporaryStore {
        path: std::path::PathBuf,
    },
    /// Corvene (`618-keymap-overrides`, `522-settings-file`): parts of the
    /// configuration file at `path` could not be used; `errors` say which.
    ConfigFileErrors {
        path: std::path::PathBuf,
        errors: Vec<String>,
    },
}

impl Banner {
    /// GHD banner timeouts: 5 s for plain successes, 15 s when there is an Undo.
    pub fn timeout(&self) -> Option<Duration> {
        match self {
            Banner::SuccessfulMerge { .. }
            | Banner::SuccessfulRebase { .. }
            | Banner::BranchAlreadyUpToDate { .. }
            | Banner::CherryPickUndone { .. }
            | Banner::SquashUndone { .. }
            | Banner::ReorderUndone { .. }
            | Banner::BranchRestored { .. }
            | Banner::BranchesRestored { .. }
            | Banner::StashRestored => Some(Duration::from_secs(5)),
            Banner::RepositoryMoved { .. } => Some(Duration::from_secs(15)),
            Banner::PatchApplied { conflicts, .. } => {
                (*conflicts == 0).then(|| Duration::from_secs(5))
            }
            Banner::SuccessfulCherryPick { .. }
            | Banner::SuccessfulSquash { .. }
            | Banner::SuccessfulReorder { .. }
            | Banner::FixupCommitted { .. }
            | Banner::BranchDeleted { .. }
            | Banner::BranchesDeleted { .. }
            | Banner::IssueCreated { .. }
            | Banner::ReviewSubmitted { .. }
            | Banner::ReleaseCreated { .. }
            | Banner::StashDropped { .. }
            | Banner::StashFileDiscarded { .. } => Some(Duration::from_secs(15)),
            Banner::ConflictsFound { .. }
            | Banner::GitEmailMismatch { .. }
            | Banner::RepositoriesUnreadable { .. }
            | Banner::TemporaryStore { .. }
            | Banner::ConfigFileErrors { .. } => None,
        }
    }

    /// `322-git-email-mismatch-banner`: the banner for a global
    /// `user.email` of `email` after signing in to `account`, if commits
    /// with it would not be linked to the account.
    pub fn for_git_email(account: &corvene_models::Account, email: Option<&str>) -> Option<Self> {
        let email = email.map(str::trim).filter(|e| !e.is_empty());
        match email {
            Some(email) if account.is_attributable_email(email) => None,
            _ => Some(Banner::GitEmailMismatch {
                host: account.host(),
                missing: email.is_none(),
            }),
        }
    }

    pub fn dismissable(&self) -> bool {
        !matches!(self, Banner::ConflictsFound { .. })
    }
}

/// Merge dialog preview (`mergeStatus` + commit count).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergePreview {
    pub branch: String,
    pub commits: u32,
    pub mergeability: Option<Mergeability>,
}

/// Rebase dialog preview (`RebasePreview`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RebasePreview {
    pub base_branch: String,
    /// The branch rebased onto `base_branch`; `None` for the current one
    /// (Corvene `1224-rebase-branch-onto-current` names another).
    pub target: Option<String>,
    /// Commits on the current branch that will be replayed (`commitsAhead`).
    pub commits_ahead: Vec<CommitOneLine>,
    /// Commits on the base branch missing from the current one (`commitsBehind`).
    pub behind: usize,
    pub valid: bool,
}

fn spawn_bg<T: Send + 'static>(
    cx: &mut dyn Host,
    work: impl FnOnce() -> T + Send + 'static,
    then: impl FnOnce(T, &mut dyn Host) + 'static,
) {
    let task = cx.background_executor().spawn(async move { work() });
    cx.spawn(async move |cx: &mut AsyncCtx| {
        let result = task.await;
        cx.update(|cx| then(result, cx));
    })
    .detach();
}

fn operation_description(kind: MultiCommitOperationKind) -> &'static str {
    match kind {
        MultiCommitOperationKind::Rebase => "rebasing",
        MultiCommitOperationKind::CherryPick => "cherry-picking onto",
        MultiCommitOperationKind::Squash => "squashing commits on",
        MultiCommitOperationKind::Reorder => "reordering commits on",
        MultiCommitOperationKind::Merge => "merge into",
    }
}

impl Dispatcher {
    /// Whether the repository is still in a conflicted merge, rebase or
    /// cherry-pick and `838-no-merge-while-conflicted` is on: the merge,
    /// rebase and update-from-default entry points then do not start another
    /// operation (GHD starts it and shows git's error).
    pub fn merge_blocked_by_conflicts(id: u64, cx: &dyn Host) -> bool {
        let s = Self::state(cx).read(cx);
        s.flags.bool(crate::flags::ids::NO_MERGE_WHILE_CONFLICTED)
            && s.repo_states
                .get(&id)
                .is_some_and(|r| r.conflict_state.is_some())
    }

    /// [`Self::merge_blocked_by_conflicts`], explaining why when it is.
    pub fn refuse_merge_while_conflicted(id: u64, cx: &mut dyn Host) -> bool {
        if !Self::merge_blocked_by_conflicts(id, cx) {
            return false;
        }
        Self::show_error(
            "Conflicts to resolve",
            "This repository is still in the middle of a merge, rebase or cherry-pick. Resolve \
             its conflicts and continue, or abort it, before starting another one.",
            cx,
        );
        true
    }

    // ---- banners ----

    /// `_setBanner`. Its view (`corvene_ui::banner::BannerView`, GHD
    /// `Banner`) focuses it and dismisses it `Banner::timeout()` after focus
    /// leaves it (`crate::banner_focus`).
    pub fn set_banner(banner: Banner, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            s.banner_nonce += 1;
            s.banner = Some(banner);
            cx.notify();
        });
    }

    /// GHD `Banner`'s dismissal timeout (`onDismissed`) for the banner shown
    /// as `nonce`: it goes unless another banner has replaced it.
    pub fn dismiss_banner(nonce: u64, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            if s.banner_nonce == nonce && s.banner.take().is_some() {
                cx.notify();
            }
        });
    }

    pub fn clear_banner(cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            if s.banner.take().is_some() {
                cx.notify();
            }
        });
    }

    fn clear_conflicts_banner(cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            if matches!(s.banner, Some(Banner::ConflictsFound { .. })) {
                s.banner = None;
                cx.notify();
            }
        });
    }

    // ---- operation state ----

    fn mco(id: u64, cx: &dyn Host) -> Option<MultiCommitOperation> {
        Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|r| r.mco.clone())
    }

    fn update_mco(id: u64, cx: &mut dyn Host, edit: impl FnOnce(&mut MultiCommitOperation)) {
        Self::state(cx).update(cx, |s, cx| {
            if let Some(mco) = s.repo_state_mut(id).mco.as_mut() {
                edit(mco);
                cx.notify();
            }
        });
    }

    pub fn set_mco_step(id: u64, step: McoStep, cx: &mut dyn Host) {
        Self::update_mco(id, cx, |m| m.step = step);
    }

    fn init_mco(
        id: u64,
        detail: McoDetail,
        target_branch: Option<String>,
        original_branch_tip: Option<String>,
        step: McoStep,
        cx: &mut dyn Host,
    ) {
        let (first_summary, total) = match &detail {
            McoDetail::Rebase { commits, .. } | McoDetail::CherryPick { commits, .. } => (
                commits
                    .first()
                    .map(|c| c.summary.clone())
                    .unwrap_or_default(),
                commits.len(),
            ),
            McoDetail::Squash { commits, .. }
            | McoDetail::Reorder { commits, .. }
            | McoDetail::Autosquash { commits, .. } => (
                commits
                    .first()
                    .map(|c| c.summary.clone())
                    .unwrap_or_default(),
                commits.len(),
            ),
            McoDetail::Merge { .. } => (String::new(), 0),
        };
        Self::state(cx).update(cx, |s, cx| {
            s.repo_state_mut(id).mco_flow += 1;
            s.repo_state_mut(id).mco = Some(MultiCommitOperation {
                step,
                detail,
                progress: McoProgress {
                    value: 0.,
                    position: 1,
                    total,
                    current_summary: first_summary,
                },
                user_has_resolved_conflicts: false,
                original_branch_tip,
                target_branch,
                conflicts: McoConflicts::default(),
            });
            cx.notify();
        });
    }

    fn show_mco_popup(id: u64, cx: &mut dyn Host) {
        let flow = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .map(|r| r.mco_flow)
            .unwrap_or(0);
        Self::show_popup(Popup::MultiCommitOperation { repo: id, flow }, cx);
    }

    /// GHD `areTherePopupsOfType(PopupType.MultiCommitOperation)`, for
    /// repository `id`.
    fn is_mco_popup(popups: &crate::popup_manager::PopupManager, id: u64) -> bool {
        popups
            .all_popups()
            .iter()
            .any(|p| matches!(p.popup, Popup::MultiCommitOperation { repo, .. } if repo == id))
    }

    fn close_mco_popup(id: u64, cx: &mut dyn Host) {
        Self::close_popups_where(
            |p| matches!(p, Popup::MultiCommitOperation { repo, .. } if *repo == id),
            cx,
        );
    }

    /// `_endMultiCommitOperation` (+ closing its dialog).
    pub fn end_mco(id: u64, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            s.repo_state_mut(id).mco = None;
            cx.notify();
        });
        Self::close_mco_popup(id, cx);
    }

    pub(crate) fn current_branch_and_tip_pub(
        id: u64,
        cx: &dyn Host,
    ) -> Option<(String, Option<String>)> {
        Self::current_branch_and_tip(id, cx)
    }

    fn current_branch_and_tip(id: u64, cx: &dyn Host) -> Option<(String, Option<String>)> {
        let s = Self::state(cx).read(cx);
        let branch = s.repo_states.get(&id)?.info.as_ref()?.current_branch()?;
        Some((branch.name.clone(), branch.tip.clone()))
    }

    /// [`Self::current_branch_and_tip`] for an operation the user started:
    /// with flag `822` a missing branch (detached HEAD, rebase in progress)
    /// shows an error titled `title` instead of doing nothing.
    fn current_branch_or_explain(
        id: u64,
        title: &str,
        cx: &mut dyn Host,
    ) -> Option<(String, Option<String>)> {
        let found = Self::current_branch_and_tip(id, cx);
        if found.is_none() {
            let message = {
                let s = Self::state(cx).read(cx);
                let rs = s.repo_states.get(&id);
                let loaded = rs.is_some_and(|r| r.info.is_some());
                let rebasing = rs
                    .and_then(|r| r.status.as_deref())
                    .is_some_and(|st| st.rebase_in_progress);
                (loaded && s.flags.bool(crate::flags::ids::NO_BRANCH_EXPLAINED)).then_some(
                    if rebasing {
                        "A rebase is in progress. Finish or abort it first."
                    } else {
                        "You are not on a branch (detached HEAD). Check out a branch first."
                    },
                )
            };
            if let Some(message) = message {
                Self::show_error(title, message, cx);
            }
        }
        found
    }

    /// Flag `834`: rebases keep `#` lines in commit messages.
    fn rebase_keeps_messages(cx: &dyn Host) -> bool {
        Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::REBASE_KEEPS_HASH_MESSAGES)
    }

    /// Flag `836`: cherry-picks keep `#` lines and drop git's conflict note.
    fn cherry_pick_keeps_messages(cx: &dyn Host) -> bool {
        Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::CHERRY_PICK_KEEPS_MESSAGES)
    }

    fn working_directory_files(id: u64, cx: &dyn Host) -> Vec<WorkingDirectoryFileChange> {
        Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|r| r.status.as_deref())
            .map(|st| st.files.clone())
            .unwrap_or_default()
    }

    fn manual_resolutions(id: u64, cx: &dyn Host) -> BTreeMap<String, ManualConflictResolution> {
        Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|r| r.conflict_state.as_ref())
            .map(|c| c.manual_resolutions.clone())
            .unwrap_or_default()
    }

    /// Store a fresh status (from an operation's background task) and derive
    /// the conflict state from it, like `_loadStatus`.
    fn apply_status(rs: &mut RepositoryState, mut status: WorkingDirectoryStatus) {
        status.sort_files();
        rs.conflict_state = derive_conflict_state(&status, rs.conflict_state.as_ref());
        rs.status = Some(Arc::new(status));
    }

    /// `_checkForUncommittedChanges`: rebase-style operations need a clean
    /// working directory; offer to stash and retry.
    fn blocked_by_local_changes(id: u64, retry: RetryAction, cx: &mut dyn Host) -> bool {
        let files: Vec<String> = Self::working_directory_files(id, cx)
            .into_iter()
            .map(|f| f.path)
            .collect();
        if files.is_empty() {
            return false;
        }
        Self::show_popup(
            Popup::LocalChangesOverwritten {
                repo: id,
                retry,
                files,
            },
            cx,
        );
        true
    }

    /// `LocalChangesOverwrittenDialog` › Stash Changes and Continue.
    pub fn stash_and_retry(id: u64, retry: RetryAction, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let Some((branch, _)) = Self::current_branch_and_tip(id, cx) else {
            return;
        };
        let guard = Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::STASH_PROTECTS_ASSUME_UNCHANGED);
        Self::close_popup(cx);
        spawn_bg(
            cx,
            move || {
                corvene_git::create_desktop_stash(git.clone(), &workdir, &branch, guard)
                    .and_then(|_| corvene_git::get_status(git, &workdir))
            },
            move |result, cx| match result {
                Ok(status) => {
                    Self::state(cx).update(cx, |s, cx| {
                        Self::apply_status(s.repo_state_mut(id), status);
                        cx.notify();
                    });
                    Self::perform_retry(id, retry, cx);
                }
                Err(err) => {
                    Self::end_mco(id, cx);
                    Self::show_error("Could not stash changes", &err, cx);
                }
            },
        );
    }

    /// `performRetry`
    pub fn perform_retry(id: u64, retry: RetryAction, cx: &mut dyn Host) {
        match retry {
            RetryAction::CherryPick { target } => Self::cherry_pick_to_branch(id, target, cx),
            RetryAction::CherryPickNewBranch { name, start_point } => {
                Self::cherry_pick_to_new_branch(id, name, start_point, cx)
            }
            RetryAction::Squash {
                to_squash,
                onto,
                message,
            } => Self::squash(id, to_squash, onto, message, false, cx),
            RetryAction::Reorder { to_move, before } => {
                Self::reorder_commits(id, to_move, before, false, cx)
            }
            RetryAction::Autosquash => Self::autosquash(id, false, cx),
            RetryAction::Push {
                force_with_lease,
                branch,
                up_to: Some(sha),
            } => Self::push_inner(id, force_with_lease, branch, Some(sha), |_, _| {}, cx),
            RetryAction::Push {
                force_with_lease,
                branch,
                up_to: None,
            } => Self::push(id, force_with_lease, branch, cx),
            RetryAction::Pull => Self::pull(id, cx),
            RetryAction::Fetch => Self::fetch(id, false, cx),
            RetryAction::Rebase { base, target } => {
                Self::start_rebase_of(id, base, target, false, cx)
            }
            RetryAction::PushToRemote { remote } => Self::push_to_remote(id, remote, cx),
        }
    }

    /// `warnAboutRemoteCommits`: the branch tracks a remote that already has
    /// commits the operation is about to rewrite.
    fn remote_commits_would_be_rewritten(
        git: Arc<corvene_git::GitBinary>,
        workdir: &std::path::Path,
        upstream: Option<&str>,
        oldest_commit_ref: Option<&str>,
    ) -> bool {
        let Some(upstream) = upstream else {
            return false;
        };
        let upstream_exists =
            corvene_git::commits_in_range(git.clone(), workdir, &format!("{upstream}^!"))
                .ok()
                .flatten()
                .is_some();
        if !upstream_exists {
            return false;
        }
        let Some(oldest) = oldest_commit_ref else {
            return true;
        };
        corvene_git::commits_between(git, workdir, oldest, upstream)
            .ok()
            .flatten()
            .is_some_and(|c| !c.is_empty())
    }

    // ---- shared result processing ----

    /// `completeMultiCommitOperation`: banner, force-push bookkeeping, refresh.
    fn complete_mco(id: u64, count: usize, cx: &mut dyn Host) {
        let Some(mco) = Self::mco(id, cx) else {
            return;
        };
        let current = Self::current_branch_and_tip(id, cx);
        if matches!(mco.detail, McoDetail::Squash { .. }) {
            Self::state(cx).update(cx, |s, _| s.repo_state_mut(id).squash_draft = None);
        }
        // flag `830`: find the rewritten commits again once history reloads
        let rewritten: Vec<(String, Option<i64>)> = match &mco.detail {
            McoDetail::Squash { message, .. } => {
                vec![(
                    message.lines().next().unwrap_or("").trim().to_string(),
                    None,
                )]
            }
            McoDetail::Reorder { commits, .. } => commits
                .iter()
                .map(|c| (c.summary.clone(), Some(c.author.seconds)))
                .collect(),
            _ => Vec::new(),
        };
        Self::state(cx).update(cx, |s, _| {
            let on = s.flags.bool(crate::flags::ids::SELECT_REWRITTEN_COMMITS);
            s.repo_state_mut(id).rewritten_selection = if on { rewritten } else { Vec::new() };
        });
        let banner = match &mco.detail {
            McoDetail::Squash { .. } | McoDetail::Autosquash { .. } => {
                Banner::SuccessfulSquash { repo: id, count }
            }
            McoDetail::Reorder { .. } => Banner::SuccessfulReorder { repo: id, count },
            McoDetail::CherryPick { .. } => Banner::SuccessfulCherryPick {
                repo: id,
                target_branch: mco.target_branch.clone().unwrap_or_default(),
                count,
                undoable: true,
            },
            McoDetail::Rebase { base_branch, .. } => Banner::SuccessfulRebase {
                target_branch: mco.target_branch.clone().unwrap_or_default(),
                base_branch: base_branch.clone(),
            },
            McoDetail::Merge { source_branch, .. } => Banner::SuccessfulMerge {
                our_branch: current.as_ref().map(|c| c.0.clone()).unwrap_or_default(),
                their_branch: source_branch.clone(),
            },
        };
        Self::set_banner(banner, cx);
        if mco.kind() != MultiCommitOperationKind::CherryPick
            && let Some((branch, Some(tip))) = current
            && let Some(original) = mco.original_branch_tip.clone()
            && original != tip
        {
            Self::state(cx).update(cx, |s, _| {
                s.repo_state_mut(id).force_push_branches.insert(branch, tip);
            });
        }
        Self::end_mco(id, cx);
        Self::refresh_repository(id, cx);
    }

    /// `startMultiCommitOperationConflictFlow`
    fn start_conflict_flow(id: u64, our: Option<String>, their: Option<String>, cx: &mut dyn Host) {
        let has_conflict_state = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .is_some_and(|r| r.conflict_state.is_some());
        if !has_conflict_state {
            warn!(id, "conflict flow requested without a conflict state");
            Self::end_mco(id, cx);
            Self::refresh_repository(id, cx);
            return;
        }
        Self::update_mco(id, cx, |m| {
            m.step = McoStep::ShowConflicts;
            m.conflicts = McoConflicts {
                our_branch: our,
                their_branch: their,
            };
        });
        Self::reveal_conflicts(id, cx);
        Self::refresh_repository(id, cx);
    }

    /// Show the conflicts step of `id`'s operation: the dialog, or with
    /// `878-conflicts-open-as-banner` the conflicts banner (as if the dialog
    /// had been closed).
    fn reveal_conflicts(id: u64, cx: &mut dyn Host) {
        if Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::CONFLICTS_OPEN_AS_BANNER)
        {
            Self::hide_conflicts(id, cx);
        } else {
            Self::show_mco_popup(id, cx);
        }
    }

    /// `processMultiCommitOperationRebaseResult`
    fn process_rebase_result(
        id: u64,
        result: RebaseResult,
        status: Option<WorkingDirectoryStatus>,
        count: usize,
        our: Option<String>,
        their: Option<String>,
        cx: &mut dyn Host,
    ) {
        if let Some(status) = status {
            Self::state(cx).update(cx, |s, cx| {
                Self::apply_status(s.repo_state_mut(id), status);
                cx.notify();
            });
        }
        match result {
            RebaseResult::CompletedWithoutError => {
                Self::show_section(id, Section::History, cx);
                Self::complete_mco(id, count, cx);
            }
            RebaseResult::AlreadyUpToDate => {
                let (our_branch, their_branch) = match Self::mco(id, cx).map(|m| m.detail) {
                    Some(McoDetail::Rebase { base_branch, .. }) => (
                        Self::current_branch_and_tip(id, cx)
                            .map(|c| c.0)
                            .unwrap_or_default(),
                        base_branch,
                    ),
                    _ => (our.unwrap_or_default(), their),
                };
                Self::set_banner(
                    Banner::BranchAlreadyUpToDate {
                        our_branch,
                        their_branch,
                    },
                    cx,
                );
                Self::end_mco(id, cx);
                Self::refresh_repository(id, cx);
            }
            RebaseResult::ConflictsEncountered => Self::start_conflict_flow(id, our, their, cx),
            RebaseResult::OutstandingFilesNotStaged | RebaseResult::Aborted => {
                Self::end_mco(id, cx);
                Self::refresh_repository(id, cx);
            }
            RebaseResult::Error(message) => {
                let kind = Self::mco(id, cx).map(|m| m.kind());
                // flag `828`: keep the squash message for the next try
                if let Some(McoDetail::Squash {
                    commits,
                    target_commit,
                    message: draft,
                    ..
                }) = Self::mco(id, cx).map(|m| m.detail)
                    && Self::state(cx)
                        .read(cx)
                        .flags
                        .bool(crate::flags::ids::SQUASH_KEEPS_DRAFT)
                {
                    let key = squash_draft_key(&target_commit.sha, commits.iter().map(|c| &c.sha));
                    Self::state(cx).update(cx, |s, _| {
                        s.repo_state_mut(id).squash_draft = Some((key, draft));
                    });
                }
                Self::end_mco(id, cx);
                Self::show_error(
                    format!("{} failed", kind.map(|k| k.label()).unwrap_or("Operation")),
                    message,
                    cx,
                );
                Self::refresh_repository(id, cx);
            }
        }
    }

    // ---- rebase ----

    /// Branch › Rebase Current Branch…: open the choose-branch step.
    pub fn start_rebase_flow(id: u64, cx: &mut dyn Host) {
        Self::start_rebase_flow_onto(id, None, cx);
    }

    /// The choose-branch step with `base_branch` already selected (the
    /// branch list's "Rebase Current Branch onto…",
    /// `856-branch-menu-rebase-onto`).
    pub fn start_rebase_flow_onto(id: u64, base_branch: Option<String>, cx: &mut dyn Host) {
        // `showRebaseDialog` → `getMultiCommitOperationChooseBranchStep`
        let step = {
            let s = Self::state(cx).read(cx);
            let Some(rs) = s.repo_states.get(&id) else {
                return;
            };
            get_multi_commit_operation_choose_branch_step(rs)
        };
        let (current, tip) = match step {
            Ok(step) => (step.current_branch.name, step.current_branch.tip),
            Err(err) => {
                warn!(id, %err, "cannot start the rebase flow");
                return;
            }
        };
        Self::state(cx).update(cx, |s, _| s.repo_state_mut(id).rebase_preview = None);
        if let Some(base) = &base_branch {
            Self::preview_rebase(id, base.clone(), cx);
        }
        Self::init_mco(
            id,
            McoDetail::Rebase {
                base_branch,
                commits: Vec::new(),
            },
            Some(current),
            tip,
            McoStep::ChooseBranch,
            cx,
        );
        Self::show_mco_popup(id, cx);
    }

    /// `updateRebasePreview`
    pub fn preview_rebase(id: u64, base_branch: String, cx: &mut dyn Host) {
        Self::preview_rebase_then(id, base_branch, None, |_, _| {}, cx);
    }

    /// Update from Default Branch with `pull.rebase` set
    /// (`859-update-from-default-rebases`): rebase the current branch onto
    /// `base_branch` without the choose-branch step.
    pub(crate) fn rebase_onto(id: u64, base_branch: String, cx: &mut dyn Host) {
        let Some((current, _)) = Self::current_branch_and_tip(id, cx) else {
            return;
        };
        Self::preview_rebase_then(
            id,
            base_branch.clone(),
            None,
            move |preview, cx| {
                if !preview.valid {
                    Self::show_error(
                        "Could not rebase",
                        format!("Unable to rebase {current} onto {base_branch}."),
                        cx,
                    );
                } else if preview.behind == 0 {
                    Self::set_banner(
                        Banner::BranchAlreadyUpToDate {
                            our_branch: current,
                            their_branch: Some(base_branch),
                        },
                        cx,
                    );
                } else {
                    Self::start_rebase(id, base_branch, false, cx);
                }
            },
            cx,
        );
    }

    /// [`Self::preview_rebase`] of `target` (`None`: the current branch),
    /// then `then(preview)` once it is stored.
    fn preview_rebase_then(
        id: u64,
        base_branch: String,
        target: Option<String>,
        then: impl FnOnce(RebasePreview, &mut dyn Host) + 'static,
        cx: &mut dyn Host,
    ) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let base = base_branch.clone();
        let tip = target.clone().unwrap_or_else(|| "HEAD".to_string());
        spawn_bg(
            cx,
            move || {
                let ahead = corvene_git::commits_between(git.clone(), &workdir, &base, &tip);
                let behind = corvene_git::commits_between(git, &workdir, &tip, &base);
                (ahead, behind)
            },
            move |(ahead, behind), cx| {
                let preview = match (ahead, behind) {
                    (Ok(Some(ahead)), Ok(Some(behind))) => RebasePreview {
                        base_branch: base_branch.clone(),
                        target,
                        commits_ahead: ahead,
                        behind: behind.len(),
                        valid: true,
                    },
                    _ => RebasePreview {
                        base_branch: base_branch.clone(),
                        target,
                        commits_ahead: Vec::new(),
                        behind: 0,
                        valid: false,
                    },
                };
                Self::state(cx).update(cx, |s, cx| {
                    s.repo_state_mut(id).rebase_preview = Some(preview.clone());
                    cx.notify();
                });
                then(preview, cx);
            },
        );
    }

    /// `startRebase`: warn about a force push when needed, then rebase.
    pub fn start_rebase(id: u64, base_branch: String, force_push_checked: bool, cx: &mut dyn Host) {
        Self::start_rebase_of(id, base_branch, None, force_push_checked, cx);
    }

    /// Corvene `1224-rebase-branch-onto-current`: the branch list's "Rebase
    /// <branch> onto <current>": `git rebase <current> <branch>`, which
    /// leaves `branch` checked out (GHD only rebases the current branch).
    pub fn rebase_branch_onto_current(id: u64, branch: String, cx: &mut dyn Host) {
        let Some((current, _)) = Self::current_branch_and_tip(id, cx) else {
            return;
        };
        if current == branch {
            return;
        }
        Self::preview_rebase_then(
            id,
            current.clone(),
            Some(branch.clone()),
            move |preview, cx| {
                if !preview.valid {
                    Self::show_error(
                        "Could not rebase",
                        format!("Unable to rebase {branch} onto {current}."),
                        cx,
                    );
                } else if preview.behind == 0 {
                    Self::set_banner(
                        Banner::BranchAlreadyUpToDate {
                            our_branch: branch,
                            their_branch: Some(current),
                        },
                        cx,
                    );
                } else {
                    Self::start_rebase_of(id, current, Some(branch), false, cx);
                }
            },
            cx,
        );
    }

    /// [`Self::start_rebase`] of `target`, the current branch when `None`.
    fn start_rebase_of(
        id: u64,
        base_branch: String,
        target: Option<String>,
        force_push_checked: bool,
        cx: &mut dyn Host,
    ) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let retry_target = target.clone();
        let Some((target, tip)) = (match target {
            Some(name) => {
                let found = Self::branch_by_name(id, &name, cx)
                    .filter(|b| b.kind == corvene_models::BranchKind::Local)
                    .map(|b| (name.clone(), b.tip));
                if found.is_none() {
                    Self::show_error(
                        "Could not rebase",
                        format!("The branch {name} no longer exists."),
                        cx,
                    );
                }
                found
            }
            None => Self::current_branch_and_tip(id, cx),
        }) else {
            return;
        };
        // flag `833`: git refuses to rebase over local changes; offer the
        // stash first, and rebase once it is made (GHD's retry forgets to)
        if Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::REBASE_STASH_AND_CONTINUE)
            && Self::blocked_by_local_changes(
                id,
                RetryAction::Rebase {
                    base: base_branch.clone(),
                    target: retry_target.clone(),
                },
                cx,
            )
        {
            return;
        }
        let commits = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|r| r.rebase_preview.as_ref())
            .filter(|p| p.base_branch == base_branch && p.target == retry_target)
            .map(|p| p.commits_ahead.clone())
            .unwrap_or_default();
        Self::init_mco(
            id,
            McoDetail::Rebase {
                base_branch: Some(base_branch.clone()),
                commits: commits.clone(),
            },
            Some(target.clone()),
            tip.clone(),
            McoStep::ShowProgress,
            cx,
        );
        Self::show_mco_popup(id, cx);
        let confirm = Self::state(cx).read(cx).settings.confirm_force_push;
        if confirm && !force_push_checked {
            // GHD warns when the branch being rewritten (the current one) is
            // published and its remote commits would be rewritten.
            let upstream = Self::branch_by_name(id, &target, cx).and_then(|b| b.upstream);
            let (git2, workdir2) = (git.clone(), workdir.clone());
            spawn_bg(
                cx,
                move || {
                    Self::remote_commits_would_be_rewritten(
                        git2,
                        &workdir2,
                        upstream.as_deref(),
                        tip.as_deref(),
                    )
                },
                move |warn, cx| {
                    if warn {
                        Self::set_mco_step(id, McoStep::WarnForcePush, cx);
                    } else {
                        Self::run_rebase(id, base_branch, target, commits, cx);
                    }
                },
            );
            return;
        }
        Self::run_rebase(id, base_branch, target, commits, cx);
    }

    fn run_rebase(
        id: u64,
        base_branch: String,
        target_branch: String,
        commits: Vec<CommitOneLine>,
        cx: &mut dyn Host,
    ) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        Self::set_mco_step(id, McoStep::ShowProgress, cx);
        info!(id, %base_branch, %target_branch, "starting rebase");
        let (base_for_result, target_for_result) = (base_branch.clone(), target_branch.clone());
        let keep_messages = Self::rebase_keeps_messages(cx);
        Self::run_with_progress(
            id,
            cx,
            move |on_progress| {
                let result = corvene_git::rebase(
                    git.clone(),
                    &workdir,
                    &base_branch,
                    &target_branch,
                    &commits,
                    keep_messages,
                    on_progress,
                );
                let status = corvene_git::get_status(git, &workdir).ok();
                (result, status)
            },
            move |(result, status), cx| {
                let count = Self::mco(id, cx)
                    .map(|m| match m.detail {
                        McoDetail::Rebase { commits, .. } => commits.len(),
                        _ => 0,
                    })
                    .unwrap_or(0);
                Self::process_rebase_result(
                    id,
                    result,
                    status,
                    count,
                    Some(base_for_result),
                    Some(target_for_result),
                    cx,
                );
            },
        );
    }

    /// Run a git operation that reports `McoProgress` on a background thread,
    /// mirroring it into the operation state as it arrives.
    fn run_with_progress<T: Send + 'static>(
        id: u64,
        cx: &mut dyn Host,
        work: impl FnOnce(&mut dyn FnMut(McoProgress)) -> T + Send + 'static,
        then: impl FnOnce(T, &mut dyn Host) + 'static,
    ) {
        let (tx, rx) = async_channel::unbounded::<McoProgress>();
        let task = cx.background_executor().spawn(async move {
            let mut report = |p: McoProgress| {
                let _ = tx.send_blocking(p);
            };
            work(&mut report)
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            while let Ok(progress) = rx.recv().await {
                cx.update(|cx| Self::update_mco(id, cx, |m| m.progress = progress));
            }
            let result = task.await;
            cx.update(|cx| then(result, cx));
        })
        .detach();
    }

    /// `WarnForcePushDialog` › Begin: remember the checkbox, run the operation.
    pub fn begin_after_force_push_warning(id: u64, ask_again: bool, cx: &mut dyn Host) {
        if !ask_again {
            Self::update_settings(cx, |s| s.confirm_force_push = false);
        }
        let Some(mco) = Self::mco(id, cx) else {
            return;
        };
        match mco.detail {
            McoDetail::Rebase {
                base_branch: Some(base),
                commits,
            } => {
                let target = mco.target_branch.clone().unwrap_or_default();
                Self::run_rebase(id, base, target, commits, cx);
            }
            // `1221-stacked-branch-refs`: the branches were asked about
            McoDetail::Squash {
                commits,
                target_commit,
                message,
                update_refs,
                ..
            } => Self::squash_with(
                id,
                commits.iter().map(|c| c.sha.clone()).collect(),
                target_commit.sha,
                message,
                true,
                StackedBranches::Decided(update_refs),
                cx,
            ),
            McoDetail::Reorder {
                commits,
                before_commit,
                update_refs,
                ..
            } => Self::reorder_commits_with(
                id,
                commits.iter().map(|c| c.sha.clone()).collect(),
                before_commit.map(|c| c.sha),
                true,
                StackedBranches::Decided(update_refs),
                cx,
            ),
            McoDetail::Autosquash { update_refs, .. } => {
                Self::autosquash_with(id, true, StackedBranches::Decided(update_refs), cx)
            }
            _ => Self::end_mco(id, cx),
        }
    }

    // ---- conflicts dialog ----

    /// `updateManualConflictResolution`
    pub fn set_manual_resolution(
        id: u64,
        path: String,
        resolution: Option<ManualConflictResolution>,
        cx: &mut dyn Host,
    ) {
        Self::state(cx).update(cx, |s, cx| {
            if let Some(conflict) = s.repo_state_mut(id).conflict_state.as_mut() {
                match resolution {
                    Some(r) => {
                        conflict.manual_resolutions.insert(path, r);
                    }
                    None => {
                        conflict.manual_resolutions.remove(&path);
                    }
                }
                cx.notify();
            }
        });
    }

    /// Conflicts dialog › Resolve All (flag `839`): `resolution` for every
    /// file that still has conflicts. Nothing is written until Continue, and
    /// each file keeps its Undo.
    pub fn set_all_manual_resolutions(
        id: u64,
        resolution: ManualConflictResolution,
        cx: &mut dyn Host,
    ) {
        Self::state(cx).update(cx, |s, cx| {
            let rs = s.repo_state_mut(id);
            let Some(status) = rs.status.as_deref() else {
                return;
            };
            let Some(conflict) = rs.conflict_state.as_mut() else {
                return;
            };
            let paths: Vec<String> = conflicted_files(status, &conflict.manual_resolutions)
                .into_iter()
                .map(|f| f.path.clone())
                .collect();
            for path in paths {
                conflict.manual_resolutions.insert(path, resolution);
            }
            cx.notify();
        });
    }

    /// Conflicts dialog › Open in Merge Tool (flag `842`): the user's
    /// `merge.tool` on one file; refresh once it closes.
    pub fn open_in_merge_tool(id: u64, path: String, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        spawn_bg(
            cx,
            move || corvene_git::open_merge_tool(git, &workdir, &path),
            move |result, cx| {
                if let Err(err) = result {
                    Self::show_error("Could not open the merge tool", &err, cx);
                }
                Self::refresh_repository(id, cx);
            },
        );
    }

    fn note_resolved_conflicts(id: u64, cx: &mut dyn Host) {
        let any_resolved = {
            let s = Self::state(cx).read(cx);
            let rs = s.repo_states.get(&id);
            match (
                rs.and_then(|r| r.status.as_deref()),
                rs.and_then(|r| r.conflict_state.as_ref()),
            ) {
                (Some(status), Some(conflict)) => {
                    !resolved_files(status, &conflict.manual_resolutions).is_empty()
                }
                _ => false,
            }
        };
        if any_resolved {
            Self::update_mco(id, cx, |m| m.user_has_resolved_conflicts = true);
        }
    }

    /// Conflicts dialog dismissed (`onInvokeConflictsDialogDismissed`): keep
    /// the operation, show the "Resolve conflicts to continue…" banner.
    pub fn hide_conflicts(id: u64, cx: &mut dyn Host) {
        Self::note_resolved_conflicts(id, cx);
        let Some(mco) = Self::mco(id, cx) else {
            Self::close_mco_popup(id, cx);
            return;
        };
        let branch = match mco.kind() {
            MultiCommitOperationKind::Merge => mco
                .conflicts
                .our_branch
                .clone()
                .or_else(|| Self::current_branch_and_tip(id, cx).map(|c| c.0)),
            _ => mco.target_branch.clone(),
        };
        Self::set_mco_step(id, McoStep::HideConflicts, cx);
        Self::close_mco_popup(id, cx);
        Self::set_banner(
            Banner::ConflictsFound {
                repo: id,
                description: operation_description(mco.kind()).to_string(),
                branch,
            },
            cx,
        );
    }

    /// Banner › View conflicts.
    pub fn show_conflicts(id: u64, cx: &mut dyn Host) {
        Self::clear_conflicts_banner(cx);
        if Self::mco(id, cx).is_none() {
            return;
        }
        Self::set_mco_step(id, McoStep::ShowConflicts, cx);
        Self::show_mco_popup(id, cx);
    }

    /// Conflicts dialog › Abort: confirm first when work would be lost.
    pub fn request_abort_mco(id: u64, cx: &mut dyn Host) {
        Self::note_resolved_conflicts(id, cx);
        let Some(mco) = Self::mco(id, cx) else {
            return;
        };
        if mco.user_has_resolved_conflicts {
            Self::set_mco_step(id, McoStep::ConfirmAbort, cx);
        } else {
            Self::abort_mco(id, cx);
        }
    }

    /// Confirm-abort dialog › Cancel.
    pub fn return_to_conflicts(id: u64, cx: &mut dyn Host) {
        Self::set_mco_step(id, McoStep::ShowConflicts, cx);
    }

    /// `onAbort` per operation kind.
    pub fn abort_mco(id: u64, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let Some(mco) = Self::mco(id, cx) else {
            return;
        };
        let tip = Self::current_branch_and_tip(id, cx).and_then(|c| c.1);
        Self::end_mco(id, cx);
        let source = match &mco.detail {
            McoDetail::CherryPick { source_branch, .. } => source_branch
                .as_deref()
                .and_then(|name| Self::branch_by_name(id, name, cx)),
            _ => None,
        };
        let detail = mco.detail.clone();
        // `1206-explain-merge-abort-failure`
        let explain = matches!(detail, McoDetail::Merge { squash: false, .. })
            && Self::state(cx)
                .read(cx)
                .flags
                .bool(crate::flags::ids::EXPLAIN_MERGE_ABORT_FAILURE);
        spawn_bg(
            cx,
            move || -> corvene_git::error::Result<()> {
                match detail {
                    McoDetail::Merge { squash: false, .. } => {
                        corvene_git::abort_merge(git, &workdir)
                    }
                    McoDetail::Merge { squash: true, .. } => match tip {
                        Some(tip) => corvene_git::abort_squash_merge(git, &workdir, &tip),
                        None => Ok(()),
                    },
                    McoDetail::Rebase { .. }
                    | McoDetail::Squash { .. }
                    | McoDetail::Reorder { .. }
                    | McoDetail::Autosquash { .. } => corvene_git::abort_rebase(git, &workdir),
                    McoDetail::CherryPick { .. } => {
                        corvene_git::abort_cherry_pick(git.clone(), &workdir)?;
                        if let Some(source) = source {
                            corvene_git::checkout_branch(git, &workdir, &source)?;
                        }
                        Ok(())
                    }
                }
            },
            move |result, cx| {
                if let Err(err) = result {
                    let blocked = match &err {
                        corvene_git::GitError::Failed { stderr, .. } if explain => {
                            corvene_git::merge_abort_blocked_paths(stderr)
                        }
                        _ => Vec::new(),
                    };
                    if blocked.is_empty() {
                        Self::show_error("Could not abort", &err, cx);
                    } else {
                        Self::show_error(
                            "Could not abort the merge",
                            merge_abort_blocked_message(&blocked),
                            cx,
                        );
                    }
                }
                Self::refresh_repository(id, cx);
            },
        );
    }

    /// Conflicts dialog › Continue.
    pub fn continue_after_conflicts(id: u64, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let Some(mco) = Self::mco(id, cx) else {
            return;
        };
        let files = Self::working_directory_files(id, cx);
        let resolutions = Self::manual_resolutions(id, cx);
        let (our, their) = (
            mco.conflicts.our_branch.clone(),
            mco.conflicts.their_branch.clone(),
        );
        let keep_messages = Self::rebase_keeps_messages(cx);
        let keep_pick_messages = Self::cherry_pick_keeps_messages(cx);
        match mco.detail.clone() {
            McoDetail::Merge {
                squash,
                source_branch,
            } => {
                let conflicted: Vec<WorkingDirectoryFileChange> = files
                    .into_iter()
                    .filter(|f| f.status.is_conflicted())
                    .collect();
                Self::set_mco_step(id, McoStep::ShowProgress, cx);
                spawn_bg(
                    cx,
                    move || {
                        corvene_git::create_merge_commit(git, &workdir, &conflicted, &resolutions)
                    },
                    move |result, cx| {
                        match result {
                            Ok(_) => {
                                let our_branch = Self::current_branch_and_tip(id, cx)
                                    .map(|c| c.0)
                                    .unwrap_or_default();
                                Self::set_banner(
                                    Banner::SuccessfulMerge {
                                        our_branch,
                                        their_branch: source_branch,
                                    },
                                    cx,
                                );
                                let _ = squash;
                            }
                            Err(err) => Self::show_error("Could not merge", &err, cx),
                        }
                        Self::end_mco(id, cx);
                        Self::show_section(id, Section::Changes, cx);
                        Self::refresh_repository(id, cx);
                    },
                );
            }
            McoDetail::Rebase { commits, .. } => {
                let count = commits.len();
                Self::set_mco_step(id, McoStep::ShowProgress, cx);
                Self::run_with_progress(
                    id,
                    cx,
                    move |on_progress| {
                        let result = corvene_git::continue_rebase(
                            git.clone(),
                            &workdir,
                            &files,
                            &resolutions,
                            &commits,
                            keep_messages,
                            None,
                            on_progress,
                        )
                        .unwrap_or_else(|e| RebaseResult::Error(e.to_string()));
                        let status = corvene_git::get_status(git, &workdir).ok();
                        (result, status)
                    },
                    move |(result, status), cx| {
                        Self::process_rebase_result(id, result, status, count, our, their, cx)
                    },
                );
            }
            detail @ (McoDetail::Squash { .. }
            | McoDetail::Reorder { .. }
            | McoDetail::Autosquash { .. }) => {
                // `892-squash-message-survives-conflicts`: the squash keeps
                // the message typed for it once it is continued
                let (commits, message) = match detail {
                    McoDetail::Squash {
                        commits, message, ..
                    } => (
                        commits,
                        Self::state(cx)
                            .read(cx)
                            .flags
                            .bool(crate::flags::ids::SQUASH_MESSAGE_SURVIVES_CONFLICTS)
                            .then_some(message),
                    ),
                    McoDetail::Reorder { commits, .. } => (commits, None),
                    McoDetail::Autosquash { commits, .. } => (commits, None),
                    _ => return,
                };
                let one_line: Vec<CommitOneLine> = commits
                    .iter()
                    .map(|c| CommitOneLine {
                        sha: c.sha.clone(),
                        summary: c.summary.clone(),
                    })
                    .collect();
                let count = match &mco.detail {
                    McoDetail::Autosquash { count, .. } => *count,
                    _ if mco.kind() == MultiCommitOperationKind::Squash => commits.len() + 1,
                    _ => commits.len(),
                };
                Self::set_mco_step(id, McoStep::ShowProgress, cx);
                Self::run_with_progress(
                    id,
                    cx,
                    move |on_progress| {
                        let result = match &message {
                            Some(message) => corvene_git::continue_squash_rebase(
                                git.clone(),
                                &workdir,
                                &files,
                                &resolutions,
                                &one_line,
                                keep_messages,
                                message,
                                on_progress,
                            ),
                            None => corvene_git::continue_rebase(
                                git.clone(),
                                &workdir,
                                &files,
                                &resolutions,
                                &one_line,
                                keep_messages,
                                None,
                                on_progress,
                            ),
                        }
                        .unwrap_or_else(|e| RebaseResult::Error(e.to_string()));
                        let status = corvene_git::get_status(git, &workdir).ok();
                        (result, status)
                    },
                    move |(result, status), cx| {
                        Self::process_rebase_result(id, result, status, count, our, their, cx)
                    },
                );
            }
            McoDetail::CherryPick {
                commits,
                source_branch,
                ..
            } => {
                Self::set_mco_step(id, McoStep::ShowProgress, cx);
                Self::run_with_progress(
                    id,
                    cx,
                    move |on_progress| {
                        let result = corvene_git::continue_cherry_pick(
                            git.clone(),
                            &workdir,
                            &files,
                            &resolutions,
                            keep_pick_messages,
                            on_progress,
                        )
                        .unwrap_or_else(|e| CherryPickResult::Error(e.to_string()));
                        let status = corvene_git::get_status(git, &workdir).ok();
                        (result, status)
                    },
                    move |(result, status), cx| {
                        Self::process_cherry_pick_result(
                            id,
                            result,
                            status,
                            commits,
                            source_branch,
                            cx,
                        )
                    },
                );
            }
        }
    }

    // ---- merge ----

    /// `_mergeBranch` (+ `initializeMergeOperation`).
    pub fn merge_branch(id: u64, branch: String, squash: bool, cx: &mut dyn Host) {
        Self::merge_branch_with_message(id, branch, squash, None, cx)
    }

    /// [`Self::merge_branch`]; a squash merge commits with `message` when
    /// given (flag `837`'s message fields).
    pub fn merge_branch_with_message(
        id: u64,
        branch: String,
        squash: bool,
        message: Option<String>,
        cx: &mut dyn Host,
    ) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let Some((current, tip)) = Self::current_branch_and_tip(id, cx) else {
            return;
        };
        Self::init_mco(
            id,
            McoDetail::Merge {
                squash,
                source_branch: Some(branch.clone()),
            },
            Some(current.clone()),
            tip,
            McoStep::ShowProgress,
            cx,
        );
        let name = branch.clone();
        let submodules = Self::submodule_update_plan(id, cx);
        // GHD `gitStore.merge(…, { onHookFailure })`
        let operation = format!("Merge {branch} into {current}");
        let hooks = crate::hooks::hook_ui(id, false, Some(operation), cx);
        let hook_callbacks = hooks.callbacks.clone();
        spawn_bg(
            cx,
            move || {
                let result = corvene_git::hooks::with_hook_callbacks(&hook_callbacks, || {
                    corvene_git::merge_branch_with_message(
                        git.clone(),
                        &workdir,
                        &branch,
                        squash,
                        message.as_deref(),
                    )
                });
                drop(hook_callbacks);
                // `263-submodules-follow-checkout`
                let submodule_error = match (&result, submodules) {
                    (Ok(corvene_git::MergeOutcome::Success), Some((skip, askpass))) => {
                        corvene_git::update_submodules(
                            git.clone(),
                            &workdir,
                            &skip,
                            askpass.as_ref(),
                        )
                        .err()
                    }
                    _ => None,
                };
                let status = corvene_git::get_status(git, &workdir).ok();
                (result, status, submodule_error)
            },
            move |(result, status, submodule_error), cx| {
                if let Some(err) = submodule_error {
                    Self::show_error("Could not update submodules", &err, cx);
                }
                if let Some(status) = status {
                    Self::state(cx).update(cx, |s, cx| {
                        Self::apply_status(s.repo_state_mut(id), status);
                        cx.notify();
                    });
                }
                // GHD: `if (aborted) { return this._refreshRepository(repository) }`
                if hooks.aborted() {
                    Self::end_mco(id, cx);
                    Self::refresh_repository(id, cx);
                    return;
                }
                match result {
                    Ok(corvene_git::MergeOutcome::Success) => {
                        Self::set_banner(
                            Banner::SuccessfulMerge {
                                our_branch: current,
                                their_branch: Some(name),
                            },
                            cx,
                        );
                        Self::end_mco(id, cx);
                    }
                    Ok(corvene_git::MergeOutcome::AlreadyUpToDate) => {
                        Self::set_banner(
                            Banner::BranchAlreadyUpToDate {
                                our_branch: current,
                                their_branch: Some(name),
                            },
                            cx,
                        );
                        Self::end_mco(id, cx);
                    }
                    Ok(corvene_git::MergeOutcome::Conflicts) => {
                        Self::start_conflict_flow(id, Some(current), Some(name), cx);
                    }
                    Ok(corvene_git::MergeOutcome::Failed(msg)) => {
                        Self::end_mco(id, cx);
                        Self::show_error("Could not merge", msg, cx);
                    }
                    Err(err) => {
                        Self::end_mco(id, cx);
                        Self::show_error("Could not merge", &err, cx);
                    }
                }
                Self::show_section(id, Section::Changes, cx);
                Self::refresh_repository(id, cx);
            },
        );
    }

    // ---- cherry-pick ----

    /// History › Cherry-pick Commit(s)…: open the choose-target-branch step.
    pub fn start_cherry_pick_flow(id: u64, shas: Vec<String>, cx: &mut dyn Host) {
        let Some((current, tip)) = Self::current_branch_or_explain(id, "Could not cherry-pick", cx)
        else {
            return;
        };
        let commits = Self::commits_oldest_first(id, &shas, cx);
        if commits.is_empty() {
            return;
        }
        // `893-cherry-pick-into-current-branch`: commits from the compare
        // view's Behind tab are on the compared branch, not this one, so it
        // is their source and the current branch can be the target
        let compared = {
            let s = Self::state(cx).read(cx);
            s.repo_states
                .get(&id)
                .filter(|_| {
                    s.flags
                        .bool(crate::flags::ids::CHERRY_PICK_INTO_CURRENT_BRANCH)
                })
                .and_then(|rs| match &rs.compare.form {
                    crate::compare::CompareForm::Branch {
                        branch,
                        mode: crate::compare::ComparisonMode::Behind,
                        ..
                    } => Some(branch.clone()),
                    _ => None,
                })
        };
        let current = compared.unwrap_or(current);
        Self::init_mco(
            id,
            McoDetail::CherryPick {
                source_branch: Some(current),
                branch_created: false,
                commits,
            },
            None,
            tip,
            McoStep::ChooseBranch,
            cx,
        );
        Self::show_mco_popup(id, cx);
    }

    /// `orderCommitsByHistory`: the given commits in log order, oldest first.
    fn commits_oldest_first(id: u64, shas: &[String], cx: &dyn Host) -> Vec<CommitOneLine> {
        let s = Self::state(cx).read(cx);
        let Some(rs) = s.repo_states.get(&id) else {
            return Vec::new();
        };
        let wanted: HashSet<&str> = shas.iter().map(String::as_str).collect();
        rs.visible_commits()
            .iter()
            .rev()
            .filter(|c| wanted.contains(c.sha.as_str()))
            .map(|c| CommitOneLine {
                sha: c.sha.clone(),
                summary: c.summary.clone(),
            })
            .collect()
    }

    /// Choose-target-branch › New Branch: switch to the create-branch step.
    pub fn cherry_pick_show_create_branch(id: u64, initial_name: String, cx: &mut dyn Host) {
        Self::update_mco(id, cx, |m| {
            if let McoDetail::CherryPick { branch_created, .. } = &mut m.detail {
                *branch_created = true;
            }
            m.step = McoStep::CreateBranch { initial_name };
        });
    }

    /// `startCherryPickWithBranchName`
    pub fn cherry_pick_to_new_branch(
        id: u64,
        name: String,
        start_point: Option<String>,
        cx: &mut dyn Host,
    ) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        if Self::blocked_by_local_changes(
            id,
            RetryAction::CherryPickNewBranch {
                name: name.clone(),
                start_point: start_point.clone(),
            },
            cx,
        ) {
            return;
        }
        Self::set_mco_step(id, McoStep::ShowProgress, cx);
        let branch = name.clone();
        // `1222-push-target-guard`
        let no_track = Self::create_branch_no_track(id, &name, start_point.as_deref(), cx);
        spawn_bg(
            cx,
            move || {
                corvene_git::create_branch(git, &workdir, &branch, start_point.as_deref(), no_track)
            },
            move |result, cx| match result {
                Ok(()) => {
                    Self::update_mco(id, cx, |m| {
                        if let McoDetail::CherryPick { branch_created, .. } = &mut m.detail {
                            *branch_created = true;
                        }
                    });
                    Self::cherry_pick_to_branch(id, name, cx);
                }
                Err(err) => {
                    Self::end_mco(id, cx);
                    Self::show_error("Could not create branch", &err, cx);
                }
            },
        );
    }

    /// `894-cherry-pick-into-worktree-branch`: the other worktree that has
    /// `branch` checked out, when the flag is on.
    fn worktree_with_branch(id: u64, branch: &str, cx: &dyn Host) -> Option<PathBuf> {
        let s = Self::state(cx).read(cx);
        if !s
            .flags
            .bool(crate::flags::ids::CHERRY_PICK_INTO_WORKTREE_BRANCH)
        {
            return None;
        }
        let rs = s.repo_states.get(&id)?;
        // the current branch is this worktree's
        if rs
            .info
            .as_ref()
            .and_then(|i| i.current_branch())
            .is_some_and(|b| b.name == branch)
        {
            return None;
        }
        let full = format!("refs/heads/{branch}");
        rs.worktrees
            .iter()
            .find(|w| w.branch.as_deref() == Some(full.as_str()))
            .map(|w| w.path.clone())
    }

    /// `894-cherry-pick-into-worktree-branch`: copy the commits onto
    /// `target` inside the worktree at `path`, which has it checked out
    /// (GHD's checkout of it fails). A worktree with changes is left alone;
    /// conflicts are aborted there and explained.
    fn cherry_pick_in_worktree(
        id: u64,
        target: String,
        path: PathBuf,
        commits: Vec<CommitOneLine>,
        cx: &mut dyn Host,
    ) {
        enum Outcome {
            Done,
            Dirty,
            Conflicts,
            Failed(String),
        }
        let Some((git, _)) = Self::repo_context(id, cx) else {
            return;
        };
        Self::update_mco(id, cx, |m| {
            m.target_branch = Some(target.clone());
            m.step = McoStep::ShowProgress;
        });
        Self::show_mco_popup(id, cx);
        let keep_messages = Self::cherry_pick_keeps_messages(cx);
        let count = commits.len();
        let worktree = path.clone();
        spawn_bg(
            cx,
            move || {
                match corvene_git::get_status(git.clone(), &worktree) {
                    Ok(status) if !status.files.is_empty() => return Outcome::Dirty,
                    Ok(_) => {}
                    Err(err) => return Outcome::Failed(err.to_string()),
                }
                match corvene_git::cherry_pick(
                    git.clone(),
                    &worktree,
                    &commits,
                    keep_messages,
                    |_| {},
                ) {
                    CherryPickResult::CompletedWithoutError => Outcome::Done,
                    CherryPickResult::ConflictsEncountered
                    | CherryPickResult::OutstandingFilesNotStaged => {
                        let _ = corvene_git::abort_cherry_pick(git, &worktree);
                        Outcome::Conflicts
                    }
                    CherryPickResult::UnableToStart => {
                        Outcome::Failed("The cherry-pick could not be started.".into())
                    }
                    CherryPickResult::Error(message) => {
                        if corvene_git::cherry_pick_head_found(&worktree) {
                            let _ = corvene_git::abort_cherry_pick(git, &worktree);
                        }
                        Outcome::Failed(message)
                    }
                }
            },
            move |outcome, cx| {
                Self::end_mco(id, cx);
                let place = path.display().to_string();
                match outcome {
                    Outcome::Done => {
                        Self::set_banner(
                            Banner::SuccessfulCherryPick {
                                repo: id,
                                target_branch: target,
                                count,
                                undoable: false,
                            },
                            cx,
                        );
                        // the worktree's own repository, when it is listed
                        let other = Self::state(cx)
                            .read(cx)
                            .repositories
                            .iter()
                            .find(|r| r.path == path)
                            .map(|r| r.id);
                        if let Some(other) = other {
                            Self::refresh_repository(other, cx);
                        }
                    }
                    Outcome::Dirty => Self::show_error(
                        "Could not cherry-pick",
                        format!(
                            "\"{target}\" is checked out in the worktree at {place}, which has \
                             uncommitted changes. Commit or stash them there, then cherry-pick \
                             again."
                        ),
                        cx,
                    ),
                    Outcome::Conflicts => Self::show_error(
                        "Could not cherry-pick",
                        format!(
                            "Copying the commits onto \"{target}\" in the worktree at {place} \
                             ran into conflicts, so it was undone there. Open that worktree and \
                             cherry-pick the commits there to resolve them."
                        ),
                        cx,
                    ),
                    Outcome::Failed(message) => Self::show_error(
                        "Could not cherry-pick",
                        format!("In the worktree at {place}: {message}"),
                        cx,
                    ),
                }
                Self::refresh_repository(id, cx);
            },
        );
    }

    /// `cherryPick`: check out the target branch, then copy the commits.
    pub fn cherry_pick_to_branch(id: u64, target_name: String, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let Some(mco) = Self::mco(id, cx) else {
            return;
        };
        let McoDetail::CherryPick {
            commits,
            source_branch,
            branch_created,
        } = mco.detail.clone()
        else {
            return;
        };
        if let Some(path) = Self::worktree_with_branch(id, &target_name, cx) {
            Self::cherry_pick_in_worktree(id, target_name, path, commits, cx);
            return;
        }
        Self::update_mco(id, cx, |m| {
            m.target_branch = Some(target_name.clone());
            m.step = McoStep::ShowProgress;
        });
        Self::show_mco_popup(id, cx);
        if Self::blocked_by_local_changes(
            id,
            RetryAction::CherryPick {
                target: target_name.clone(),
            },
            cx,
        ) {
            return;
        }
        // a freshly created branch may not be in the cached branch list yet
        let target = Self::branch_by_name(id, &target_name, cx).unwrap_or(corvene_models::Branch {
            name: target_name.clone(),
            kind: corvene_models::BranchKind::Local,
            full_name: format!("refs/heads/{target_name}"),
            tip: None,
            upstream: None,
            tip_time: None,
            tip_author: None,
            remote_name: None,
        });
        let local_name = target.name_without_remote().to_string();
        let count = commits.len();
        let keep_messages = Self::cherry_pick_keeps_messages(cx);
        let commits_for_result = commits.clone();
        // `893-cherry-pick-into-current-branch`: onto the branch checked out
        // here, nothing to check out
        let onto_current = target.kind == corvene_models::BranchKind::Local
            && Self::current_branch_and_tip(id, cx).is_some_and(|(name, _)| name == target.name);
        Self::run_with_progress(
            id,
            cx,
            move |on_progress| {
                if !onto_current
                    && let Err(err) = corvene_git::checkout_branch(git.clone(), &workdir, &target)
                {
                    return (CherryPickResult::Error(err.to_string()), None, None, false);
                }
                let undo_sha = corvene_git::head_sha(git.clone(), &workdir).ok();
                let result = corvene_git::cherry_pick(
                    git.clone(),
                    &workdir,
                    &commits,
                    keep_messages,
                    on_progress,
                );
                let status = corvene_git::get_status(git, &workdir).ok();
                (result, status, undo_sha, true)
            },
            move |(result, status, undo_sha, checked_out), cx| {
                if !checked_out {
                    Self::end_mco(id, cx);
                    if let CherryPickResult::Error(msg) = result {
                        Self::show_error("Could not check out the target branch", msg, cx);
                    }
                    Self::refresh_repository(id, cx);
                    return;
                }
                if let Some(sha) = undo_sha {
                    Self::state(cx).update(cx, |s, _| {
                        s.repo_state_mut(id).mco_undo = Some(McoUndo {
                            sha,
                            branch: local_name.clone(),
                            kind: MultiCommitOperationKind::CherryPick,
                            count,
                            source_branch: source_branch.clone(),
                            branch_created,
                            moved_branches: Vec::new(),
                        });
                    });
                }
                Self::update_mco(id, cx, |m| m.target_branch = Some(local_name.clone()));
                Self::process_cherry_pick_result(
                    id,
                    result,
                    status,
                    commits_for_result,
                    source_branch,
                    cx,
                );
            },
        );
    }

    /// `processCherryPickResult`
    fn process_cherry_pick_result(
        id: u64,
        result: CherryPickResult,
        status: Option<WorkingDirectoryStatus>,
        commits: Vec<CommitOneLine>,
        source_branch: Option<String>,
        cx: &mut dyn Host,
    ) {
        if let Some(status) = status {
            Self::state(cx).update(cx, |s, cx| {
                Self::apply_status(s.repo_state_mut(id), status);
                cx.notify();
            });
        }
        match result {
            CherryPickResult::CompletedWithoutError => {
                Self::show_section(id, Section::History, cx);
                Self::complete_mco(id, commits.len(), cx);
            }
            CherryPickResult::ConflictsEncountered => {
                let target = Self::mco(id, cx).and_then(|m| m.target_branch);
                Self::start_conflict_flow(id, target, source_branch, cx);
            }
            CherryPickResult::UnableToStart => {
                Self::end_mco(id, cx);
                Self::refresh_repository(id, cx);
            }
            CherryPickResult::OutstandingFilesNotStaged => {
                Self::end_mco(id, cx);
                Self::refresh_repository(id, cx);
            }
            CherryPickResult::Error(message) => {
                // clear the half-done cherry-pick and go back where the user was
                let Some((git, workdir)) = Self::repo_context(id, cx) else {
                    return;
                };
                let source = source_branch.and_then(|n| Self::branch_by_name(id, &n, cx));
                Self::end_mco(id, cx);
                Self::show_error("Could not cherry-pick", message, cx);
                spawn_bg(
                    cx,
                    move || {
                        if corvene_git::cherry_pick_head_found(&workdir) {
                            let _ = corvene_git::abort_cherry_pick(git.clone(), &workdir);
                        }
                        if let Some(source) = source {
                            let _ = corvene_git::checkout_branch(git, &workdir, &source);
                        }
                    },
                    move |_, cx| Self::refresh_repository(id, cx),
                );
            }
        }
    }

    // ---- squash / reorder ----

    /// GHD `getLastRetainedCommitRef`: `<oldest involved>^`, or `None` when
    /// the oldest involved commit is the root of the loaded history.
    fn last_retained_ref(rs: &RepositoryState, shas: &[String]) -> Option<Option<String>> {
        let indexes: Vec<usize> = shas
            .iter()
            .filter_map(|sha| rs.commits.iter().position(|c| &c.sha == sha))
            .collect();
        if indexes.len() != shas.len() {
            return None;
        }
        let max = *indexes.iter().max()?;
        // the oldest involved commit is the root of the (fully loaded) history: rebase --root
        Some(if max + 1 == rs.commits.len() && rs.commits_exhausted {
            None
        } else {
            Some(format!("{}^", rs.commits[max].sha))
        })
    }

    /// History › Squash N Commits… (and drag-onto-commit): ask for the
    /// combined message after checking for merge commits in the range.
    pub fn request_squash(id: u64, to_squash: Vec<String>, onto: String, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let to_squash: Vec<String> = to_squash.into_iter().filter(|s| s != &onto).collect();
        if to_squash.is_empty() {
            return;
        }
        let (last_retained, summary, description, count) = {
            let s = Self::state(cx).read(cx);
            let Some(rs) = s.repo_states.get(&id) else {
                return;
            };
            // flag `828`: a failed squash of the same commits left its message
            let draft = rs
                .squash_draft
                .as_ref()
                .filter(|(key, _)| *key == squash_draft_key(&onto, to_squash.iter()))
                .filter(|_| s.flags.bool(crate::flags::ids::SQUASH_KEEPS_DRAFT))
                .map(|(_, message)| split_message(message));
            let mut involved = to_squash.clone();
            involved.push(onto.clone());
            let Some(last_retained) = Self::last_retained_ref(rs, &involved) else {
                return;
            };
            let Some(onto_commit) = rs.commits.iter().find(|c| c.sha == onto) else {
                return;
            };
            // the squashed commits, oldest first
            let squashed: Vec<Commit> = rs
                .commits
                .iter()
                .rev()
                .filter(|c| to_squash.contains(&c.sha))
                .cloned()
                .collect();
            let (summary, description) = draft.unwrap_or_else(|| {
                (
                    onto_commit.summary.clone(),
                    get_squashed_commit_description(&squashed, onto_commit),
                )
            });
            (last_retained, summary, description, to_squash.len() + 1)
        };
        spawn_bg(
            cx,
            move || corvene_git::merge_commits_exist_after(git, &workdir, last_retained.as_deref()),
            move |has_merges, cx| {
                if has_merges.unwrap_or(false) {
                    Self::show_error(
                        "Unable to squash",
                        "Squashing replays all commits up to the last one required for the squash. A merge commit cannot exist among those commits.",
                        cx,
                    );
                    return;
                }
                Self::show_popup(
                    Popup::SquashCommitMessage {
                        repo: id,
                        to_squash,
                        onto,
                        summary,
                        description,
                        count,
                    },
                    cx,
                );
            },
        );
    }

    /// `squash`
    pub fn squash(
        id: u64,
        to_squash: Vec<String>,
        onto: String,
        message: String,
        force_push_checked: bool,
        cx: &mut dyn Host,
    ) {
        Self::squash_with(
            id,
            to_squash,
            onto,
            message,
            force_push_checked,
            StackedBranches::Ask,
            cx,
        );
    }

    /// [`Self::squash`] once `1221-stacked-branch-refs` knows what to do
    /// with other branches among the commits (else it asks first).
    pub(crate) fn squash_with(
        id: u64,
        to_squash: Vec<String>,
        onto: String,
        message: String,
        force_push_checked: bool,
        stacked: StackedBranches,
        cx: &mut dyn Host,
    ) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        // flag `829`: git stashes local changes around the squash instead
        let autostash = Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::SQUASH_AUTOSTASH)
            && !Self::working_directory_files(id, cx).is_empty();
        if !autostash
            && Self::blocked_by_local_changes(
                id,
                RetryAction::Squash {
                    to_squash: to_squash.clone(),
                    onto: onto.clone(),
                    message: message.clone(),
                },
                cx,
            )
        {
            return;
        }
        let Some((branch, tip)) = Self::current_branch_or_explain(id, "Could not squash", cx)
        else {
            return;
        };
        let (commits, target_commit, last_retained) = {
            let s = Self::state(cx).read(cx);
            let Some(rs) = s.repo_states.get(&id) else {
                return;
            };
            let commits: Vec<Commit> = rs
                .commits
                .iter()
                .filter(|c| to_squash.contains(&c.sha))
                .cloned()
                .collect();
            let Some(target) = rs.commits.iter().find(|c| c.sha == onto).cloned() else {
                return;
            };
            let mut involved = to_squash.clone();
            involved.push(onto.clone());
            let Some(last_retained) = Self::last_retained_ref(rs, &involved) else {
                return;
            };
            (commits, target, last_retained)
        };
        let Some(update_refs) = Self::stacked_update_refs(
            id,
            StackedOp::Squash {
                to_squash: to_squash.clone(),
                onto: onto.clone(),
                message: message.clone(),
                force_push_checked,
            },
            stacked,
            last_retained.as_deref(),
            cx,
        ) else {
            return;
        };
        Self::init_mco(
            id,
            McoDetail::Squash {
                commits: commits.clone(),
                target_commit: target_commit.clone(),
                last_retained_ref: last_retained.clone(),
                message: message.clone(),
                update_refs: update_refs.clone(),
            },
            Some(branch.clone()),
            tip.clone(),
            McoStep::ShowProgress,
            cx,
        );
        Self::show_mco_popup(id, cx);
        if let Some(tip) = tip.clone() {
            Self::state(cx).update(cx, |s, _| {
                s.repo_state_mut(id).mco_undo = Some(McoUndo {
                    sha: tip,
                    branch: branch.clone(),
                    kind: MultiCommitOperationKind::Squash,
                    count: commits.len() + 1,
                    source_branch: None,
                    branch_created: false,
                    moved_branches: crate::stacked_refs::moved_branches(&update_refs),
                });
            });
        }
        let count = commits.len() + 1;
        let options = corvene_git::RebaseOptions {
            keep_messages: Self::rebase_keeps_messages(cx),
            autostash,
            update_refs,
        };
        let run = move |cx: &mut dyn Host| {
            let (git, workdir) = (git.clone(), workdir.clone());
            let branch = branch.clone();
            let (commits, target_commit, last_retained, message) = (
                commits.clone(),
                target_commit.clone(),
                last_retained.clone(),
                message.clone(),
            );
            Self::run_with_progress(
                id,
                cx,
                move |on_progress| {
                    let stash_before = options
                        .autostash
                        .then(|| corvene_git::stash_tip(git.clone(), &workdir));
                    let result = corvene_git::squash(
                        git.clone(),
                        &workdir,
                        &commits,
                        &target_commit,
                        last_retained.as_deref(),
                        &message,
                        options,
                        on_progress,
                    );
                    // the autostash went back into the stash: reapplying it conflicted
                    let stash_kept = result == RebaseResult::CompletedWithoutError
                        && stash_before.is_some_and(|before| {
                            corvene_git::stash_tip(git.clone(), &workdir) != before
                        });
                    let status = corvene_git::get_status(git, &workdir).ok();
                    (result, status, stash_kept)
                },
                move |(result, status, stash_kept), cx| {
                    Self::process_rebase_result(
                        id,
                        result,
                        status,
                        count,
                        Some(branch),
                        Some("squash commit".into()),
                        cx,
                    );
                    if stash_kept {
                        Self::show_error(
                            "Local changes kept in the stash",
                            "The commits were squashed, but your local changes conflicted with the result. They are kept in git's stash as \"autostash\" (git stash list).",
                            cx,
                        );
                    }
                },
            );
        };
        Self::maybe_warn_force_push(
            id,
            force_push_checked,
            last_retained_for_warn(&Self::mco(id, cx)),
            run,
            cx,
        );
    }

    /// Shared "warn about force push, else run" step for squash / reorder.
    fn maybe_warn_force_push(
        id: u64,
        force_push_checked: bool,
        oldest_commit_ref: Option<String>,
        run: impl FnOnce(&mut dyn Host) + 'static,
        cx: &mut dyn Host,
    ) {
        let confirm = Self::state(cx).read(cx).settings.confirm_force_push;
        if !confirm || force_push_checked {
            run(cx);
            return;
        }
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let upstream = Self::current_branch_and_tip(id, cx)
            .and_then(|(name, _)| Self::branch_by_name(id, &name, cx))
            .and_then(|b| b.upstream);
        spawn_bg(
            cx,
            move || {
                Self::remote_commits_would_be_rewritten(
                    git,
                    &workdir,
                    upstream.as_deref(),
                    oldest_commit_ref.as_deref(),
                )
            },
            move |warn, cx| {
                if warn {
                    Self::set_mco_step(id, McoStep::WarnForcePush, cx);
                } else {
                    run(cx);
                }
            },
        );
    }

    /// `reorderCommits`: move `to_move` right before `before` (`None` = tip).
    pub fn reorder_commits(
        id: u64,
        to_move: Vec<String>,
        before: Option<String>,
        force_push_checked: bool,
        cx: &mut dyn Host,
    ) {
        Self::reorder_commits_with(
            id,
            to_move,
            before,
            force_push_checked,
            StackedBranches::Ask,
            cx,
        );
    }

    /// [`Self::reorder_commits`] once `1221-stacked-branch-refs` knows what
    /// to do with other branches among the commits (else it asks first).
    pub(crate) fn reorder_commits_with(
        id: u64,
        to_move: Vec<String>,
        before: Option<String>,
        force_push_checked: bool,
        stacked: StackedBranches,
        cx: &mut dyn Host,
    ) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        if to_move.is_empty() {
            return;
        }
        if Self::blocked_by_local_changes(
            id,
            RetryAction::Reorder {
                to_move: to_move.clone(),
                before: before.clone(),
            },
            cx,
        ) {
            return;
        }
        let Some((branch, tip)) = Self::current_branch_or_explain(id, "Could not reorder", cx)
        else {
            return;
        };
        let (commits, before_commit, last_retained) = {
            let s = Self::state(cx).read(cx);
            let Some(rs) = s.repo_states.get(&id) else {
                return;
            };
            let commits: Vec<Commit> = rs
                .commits
                .iter()
                .filter(|c| to_move.contains(&c.sha))
                .cloned()
                .collect();
            let before_commit = before
                .as_ref()
                .and_then(|sha| rs.commits.iter().find(|c| &c.sha == sha).cloned());
            let mut involved = to_move.clone();
            involved.extend(before.iter().cloned());
            let Some(last_retained) = Self::last_retained_ref(rs, &involved) else {
                return;
            };
            (commits, before_commit, last_retained)
        };
        let Some(update_refs) = Self::stacked_update_refs(
            id,
            StackedOp::Reorder {
                to_move: to_move.clone(),
                before: before.clone(),
                force_push_checked,
            },
            stacked,
            last_retained.as_deref(),
            cx,
        ) else {
            return;
        };
        Self::init_mco(
            id,
            McoDetail::Reorder {
                commits: commits.clone(),
                before_commit: before_commit.clone(),
                last_retained_ref: last_retained.clone(),
                update_refs: update_refs.clone(),
            },
            Some(branch.clone()),
            tip.clone(),
            McoStep::ShowProgress,
            cx,
        );
        Self::show_mco_popup(id, cx);
        if let Some(tip) = tip.clone() {
            Self::state(cx).update(cx, |s, _| {
                s.repo_state_mut(id).mco_undo = Some(McoUndo {
                    sha: tip,
                    branch: branch.clone(),
                    kind: MultiCommitOperationKind::Reorder,
                    count: commits.len(),
                    source_branch: None,
                    branch_created: false,
                    moved_branches: crate::stacked_refs::moved_branches(&update_refs),
                });
            });
        }
        let count = commits.len();
        let last_retained_for_run = last_retained.clone();
        let keep_messages = Self::rebase_keeps_messages(cx);
        let run = move |cx: &mut dyn Host| {
            let (git, workdir) = (git.clone(), workdir.clone());
            let branch = branch.clone();
            let (commits, before_commit, last_retained) = (
                commits.clone(),
                before_commit.clone(),
                last_retained_for_run.clone(),
            );
            Self::run_with_progress(
                id,
                cx,
                move |on_progress| {
                    let result = corvene_git::reorder(
                        git.clone(),
                        &workdir,
                        &commits,
                        before_commit.as_ref(),
                        last_retained.as_deref(),
                        corvene_git::RebaseOptions {
                            keep_messages,
                            autostash: false,
                            update_refs,
                        },
                        on_progress,
                    );
                    let status = corvene_git::get_status(git, &workdir).ok();
                    (result, status)
                },
                move |(result, status), cx| {
                    Self::process_rebase_result(
                        id,
                        result,
                        status,
                        count,
                        Some(branch),
                        Some("reorder commit".into()),
                        cx,
                    )
                },
            );
        };
        Self::maybe_warn_force_push(id, force_push_checked, last_retained, run, cx);
    }

    /// Corvene `799-fixup-commits`: commit the included changes as a fixup
    /// of `target` (`git commit --fixup`), then offer to squash it in.
    pub fn commit_fixup(id: u64, target: String, cx: &mut dyn Host) {
        Self::commit_with(
            id,
            String::new(),
            String::new(),
            crate::commit_checks::CommitChecks {
                fixup: Some(target),
                ..Default::default()
            },
            cx,
        );
    }

    /// Corvene `799-fixup-commits`: Squash Fixup Commits (and the fixup
    /// banner's Squash Now): fold every unpushed `fixup!` commit into the
    /// commit it fixes, run like a squash.
    pub fn autosquash(id: u64, force_push_checked: bool, cx: &mut dyn Host) {
        Self::autosquash_with(id, force_push_checked, StackedBranches::Ask, cx);
    }

    /// [`Self::autosquash`] once `1221-stacked-branch-refs` knows what to
    /// do with other branches among the commits (else it asks first).
    pub(crate) fn autosquash_with(
        id: u64,
        force_push_checked: bool,
        stacked: StackedBranches,
        cx: &mut dyn Host,
    ) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let autostash = Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::SQUASH_AUTOSTASH)
            && !Self::working_directory_files(id, cx).is_empty();
        if !autostash && Self::blocked_by_local_changes(id, RetryAction::Autosquash, cx) {
            return;
        }
        let Some((branch, tip)) = Self::current_branch_or_explain(id, "Could not squash", cx)
        else {
            return;
        };
        let (fixups, count, last_retained) = {
            let s = Self::state(cx).read(cx);
            let Some(rs) = s.repo_states.get(&id) else {
                return;
            };
            let pairs = pending_fixups(rs);
            if pairs.is_empty() {
                return;
            }
            let fixups: Vec<Commit> = rs
                .commits
                .iter()
                .filter(|c| pairs.iter().any(|(fixup, _)| *fixup == c.sha))
                .cloned()
                .collect();
            let mut involved: Vec<String> = Vec::new();
            for (fixup, target) in &pairs {
                for sha in [fixup, target] {
                    if !involved.contains(sha) {
                        involved.push(sha.clone());
                    }
                }
            }
            let Some(last_retained) = Self::last_retained_ref(rs, &involved) else {
                return;
            };
            (fixups, involved.len(), last_retained)
        };
        let Some(update_refs) = Self::stacked_update_refs(
            id,
            StackedOp::Autosquash { force_push_checked },
            stacked,
            last_retained.as_deref(),
            cx,
        ) else {
            return;
        };
        Self::init_mco(
            id,
            McoDetail::Autosquash {
                commits: fixups,
                last_retained_ref: last_retained.clone(),
                count,
                update_refs: update_refs.clone(),
            },
            Some(branch.clone()),
            tip.clone(),
            McoStep::ShowProgress,
            cx,
        );
        Self::show_mco_popup(id, cx);
        if let Some(tip) = tip.clone() {
            Self::state(cx).update(cx, |s, _| {
                s.repo_state_mut(id).mco_undo = Some(McoUndo {
                    sha: tip,
                    branch: branch.clone(),
                    kind: MultiCommitOperationKind::Squash,
                    count,
                    source_branch: None,
                    branch_created: false,
                    moved_branches: crate::stacked_refs::moved_branches(&update_refs),
                });
            });
        }
        let options = corvene_git::RebaseOptions {
            keep_messages: Self::rebase_keeps_messages(cx),
            autostash,
            update_refs,
        };
        let last_retained_for_run = last_retained.clone();
        let run = move |cx: &mut dyn Host| {
            let (git, workdir) = (git.clone(), workdir.clone());
            let branch = branch.clone();
            let last_retained = last_retained_for_run.clone();
            Self::run_with_progress(
                id,
                cx,
                move |on_progress| {
                    let (result, _) = corvene_git::autosquash(
                        git.clone(),
                        &workdir,
                        last_retained.as_deref(),
                        options,
                        on_progress,
                    );
                    let status = corvene_git::get_status(git, &workdir).ok();
                    (result, status)
                },
                move |(result, status), cx| {
                    Self::process_rebase_result(
                        id,
                        result,
                        status,
                        count,
                        Some(branch),
                        Some("fixup commit".into()),
                        cx,
                    )
                },
            );
        };
        Self::maybe_warn_force_push(id, force_push_checked, last_retained, run, cx);
    }

    /// Banner › Undo (`_undoMultiCommitOperation`).
    pub fn undo_mco(id: u64, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let Some(undo) = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|r| r.mco_undo.clone())
        else {
            return;
        };
        let dirty = !Self::working_directory_files(id, cx).is_empty();
        let Some((branch, _)) = Self::current_branch_and_tip(id, cx) else {
            return;
        };
        if dirty {
            Self::show_error(
                format!("Could not undo {}", undo.kind.lower()),
                "This would delete the local changes that exist on the branch.",
                cx,
            );
            return;
        }
        if branch != undo.branch {
            Self::show_error(
                format!("Could not undo {}", undo.kind.lower()),
                format!(
                    "You are no longer on the branch the {} occurred on.",
                    undo.kind.lower()
                ),
                cx,
            );
            return;
        }
        Self::clear_banner(cx);
        let source = undo
            .source_branch
            .as_deref()
            .and_then(|n| Self::branch_by_name(id, n, cx));
        let undo_for_bg = undo.clone();
        spawn_bg(
            cx,
            move || -> corvene_git::error::Result<()> {
                let u = undo_for_bg;
                if u.kind == MultiCommitOperationKind::CherryPick && u.branch_created {
                    if let Some(source) = &source {
                        corvene_git::checkout_branch(git.clone(), &workdir, source)?;
                    }
                    return corvene_git::delete_local_branch(git, &workdir, &u.branch);
                }
                corvene_git::reset_to(git.clone(), &workdir, corvene_git::ResetMode::Hard, &u.sha)?;
                // `1221-stacked-branch-refs`: and the branches it moved
                corvene_git::restore_branch_tips(git.clone(), &workdir, &u.moved_branches)?;
                if u.kind == MultiCommitOperationKind::CherryPick
                    && let Some(source) = &source
                {
                    corvene_git::checkout_branch(git, &workdir, source)?;
                }
                Ok(())
            },
            move |result, cx| {
                match result {
                    Ok(()) => {
                        let banner = match undo.kind {
                            MultiCommitOperationKind::Squash => {
                                Banner::SquashUndone { count: undo.count }
                            }
                            MultiCommitOperationKind::Reorder => {
                                Banner::ReorderUndone { count: undo.count }
                            }
                            MultiCommitOperationKind::CherryPick => Banner::CherryPickUndone {
                                target_branch: undo.branch.clone(),
                                count: undo.count,
                            },
                            _ => return,
                        };
                        Self::set_banner(banner, cx);
                        Self::state(cx).update(cx, |s, _| s.repo_state_mut(id).mco_undo = None);
                    }
                    Err(err) => Self::show_error("Could not undo", &err, cx),
                }
                Self::refresh_repository(id, cx);
            },
        );
    }

    // ---- refresh integration ----

    /// After every status load (`_loadStatus` → `updateConflictState`,
    /// `initializeMultiCommitOperationIfConflictsFound`, `_triggerConflictsFlow`,
    /// `clearConflictsFlowVisuals`).
    pub(crate) fn sync_conflicts(
        id: u64,
        rebase_snapshot: Option<corvene_git::RebaseSnapshot>,
        cherry_pick_snapshot: Option<corvene_git::CherryPickSnapshot>,
        merge_head_branches: Option<Vec<String>>,
        cx: &mut dyn Host,
    ) {
        let (conflict, mco, current, selected, mco_popup_open, banner_is_conflicts) = {
            let s = Self::state(cx).read(cx);
            let rs = s.repo_states.get(&id);
            (
                rs.and_then(|r| r.conflict_state.clone()),
                rs.and_then(|r| r.mco.clone()),
                rs.and_then(|r| r.info.as_ref())
                    .and_then(|i| i.current_branch())
                    .map(|b| (b.name.clone(), b.tip.clone())),
                s.selected == Some(id),
                Self::is_mco_popup(&s.popups, id),
                matches!(s.banner, Some(Banner::ConflictsFound { .. })),
            )
        };
        let Some(conflict) = conflict else {
            // conflicts went away (resolved elsewhere, or aborted): drop the visuals
            if let Some(mco) = mco
                && mco.in_conflict_step()
            {
                Self::end_mco(id, cx);
                Self::clear_conflicts_banner(cx);
            }
            return;
        };
        if mco.is_none() {
            // an operation started outside Corvene (or before a restart)
            let (detail, target, original_tip, progress, our, their) = match &conflict.kind {
                ConflictKind::Merge { current_branch, .. } => (
                    McoDetail::Merge {
                        squash: Self::state(cx)
                            .read(cx)
                            .repo_states
                            .get(&id)
                            .and_then(|r| r.status.as_deref())
                            .is_some_and(|st| st.squash_msg_found),
                        source_branch: None,
                    },
                    Some(current_branch.clone()),
                    current.as_ref().and_then(|c| c.1.clone()),
                    None,
                    Some(current_branch.clone()),
                    // `getMergeConflictsTheirBranch`: the one branch at
                    // `MERGE_HEAD`, if exactly one
                    match merge_head_branches.as_deref() {
                        Some([theirs]) => Some(theirs.clone()),
                        _ => None,
                    },
                ),
                ConflictKind::Rebase {
                    target_branch,
                    original_branch_tip,
                    ..
                } => {
                    let Some(snapshot) = rebase_snapshot else {
                        return;
                    };
                    // flag `835`: name the branch at `onto` (GHD shows none)
                    let base = snapshot.base_branch.filter(|_| {
                        Self::state(cx)
                            .read(cx)
                            .flags
                            .bool(crate::flags::ids::REBASE_BASE_NAME_RESOLVED)
                    });
                    (
                        McoDetail::Rebase {
                            base_branch: base.clone(),
                            commits: snapshot.commits,
                        },
                        Some(target_branch.clone()),
                        Some(original_branch_tip.clone()),
                        Some(snapshot.progress),
                        base,
                        Some(target_branch.clone()),
                    )
                }
                ConflictKind::CherryPick { target_branch } => {
                    let Some(snapshot) = cherry_pick_snapshot else {
                        return;
                    };
                    Self::state(cx).update(cx, |s, _| {
                        s.repo_state_mut(id).mco_undo = Some(McoUndo {
                            sha: snapshot.target_branch_undo_sha.clone(),
                            branch: target_branch.clone(),
                            kind: MultiCommitOperationKind::CherryPick,
                            count: snapshot.commits.len(),
                            source_branch: None,
                            branch_created: false,
                            moved_branches: Vec::new(),
                        });
                    });
                    (
                        McoDetail::CherryPick {
                            source_branch: None,
                            branch_created: false,
                            commits: snapshot.commits,
                        },
                        Some(target_branch.clone()),
                        None,
                        Some(snapshot.progress),
                        Some(target_branch.clone()),
                        None,
                    )
                }
            };
            Self::init_mco(id, detail, target, original_tip, McoStep::ShowConflicts, cx);
            Self::update_mco(id, cx, |m| {
                if let Some(p) = progress {
                    m.progress = p;
                }
                m.conflicts = McoConflicts {
                    our_branch: our,
                    their_branch: their,
                };
            });
            // GHD `_triggerConflictsFlow` (selected repository only, not
            // while the conflicts banner shows): the conflicts dialog goes
            // on the popup stack, above any dialog already open
            // (`878-conflicts-open-as-banner`: the banner instead)
            if selected && !mco_popup_open && !banner_is_conflicts {
                Self::reveal_conflicts(id, cx);
            }
            return;
        }
        // an operation we started: make sure the conflicts step is visible
        // (`_triggerConflictsFlow`), leaving the operation's own progress
        // dialog alone (see the module doc)
        if let Some(mco) = mco
            && selected
            && !banner_is_conflicts
            && !is_conflicts_flow(mco_popup_open, Some(&mco))
            && !(mco_popup_open && mco.step == McoStep::ShowProgress)
        {
            Self::set_mco_step(id, McoStep::ShowConflicts, cx);
            Self::reveal_conflicts(id, cx);
        }
    }
}

/// Flag `830`: the newest commit matching each (summary, author time) pair,
/// in the given order; empty unless every one is found.
pub(crate) fn find_rewritten(commits: &[Commit], wanted: &[(String, Option<i64>)]) -> Vec<String> {
    let found: Vec<String> = wanted
        .iter()
        .filter_map(|(summary, seconds)| {
            commits
                .iter()
                .find(|c| &c.summary == summary && seconds.is_none_or(|t| c.author.seconds == t))
                .map(|c| c.sha.clone())
        })
        .collect();
    if found.len() == wanted.len() {
        found
    } else {
        Vec::new()
    }
}

/// Flag `828`: the commits of a squash, order-independent.
fn squash_draft_key<'a>(onto: &str, squashed: impl Iterator<Item = &'a String>) -> Vec<String> {
    let mut key: Vec<String> = squashed.cloned().collect();
    key.sort();
    key.insert(0, onto.to_string());
    key
}

impl Dispatcher {
    /// `892-edit-commit-message`: History › Edit Commit Message… opens the
    /// squash message dialog on the commit's message (no commits to squash
    /// marks the edit), unless a merge commit follows it.
    pub fn request_edit_commit_message(id: u64, sha: String, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let Some(commit) = Self::commit_by_sha(id, &sha, cx) else {
            return;
        };
        let last_retained = commit.parents.first().cloned();
        let summary = commit.summary.clone();
        let description = commit.body.trim_end().to_string();
        spawn_bg(
            cx,
            move || corvene_git::merge_commits_exist_after(git, &workdir, last_retained.as_deref()),
            move |has_merges, cx| {
                if has_merges.unwrap_or(false) {
                    Self::show_error(
                        "Unable to edit the commit message",
                        "Editing a message replays every commit after it. A merge commit cannot exist among those commits.",
                        cx,
                    );
                    return;
                }
                Self::show_popup(
                    Popup::SquashCommitMessage {
                        repo: id,
                        to_squash: Vec::new(),
                        onto: sha,
                        summary,
                        description,
                        count: 0,
                    },
                    cx,
                );
            },
        );
    }

    /// `892-edit-commit-message`: give commit `sha` `message`: an amend of
    /// HEAD's message, else a `reword` interactive rebase; the rewritten
    /// commit stays selected.
    pub fn edit_commit_message(id: u64, sha: String, message: String, cx: &mut dyn Host) {
        Self::edit_commit_message_with(id, sha, message, StackedBranches::Ask, cx);
    }

    /// [`Self::edit_commit_message`] once `1221-stacked-branch-refs` knows
    /// what to do with other branches among the replayed commits (else it
    /// asks first). An amend of HEAD replays nothing.
    pub(crate) fn edit_commit_message_with(
        id: u64,
        sha: String,
        message: String,
        stacked: StackedBranches,
        cx: &mut dyn Host,
    ) {
        let (commit, is_head) = {
            let s = Self::state(cx).read(cx);
            let rs = s.repo_states.get(&id);
            (
                rs.and_then(|r| {
                    r.commits
                        .iter()
                        .chain(r.visible_commits())
                        .find(|c| c.sha == sha)
                        .cloned()
                }),
                rs.and_then(|r| r.commits.first())
                    .is_some_and(|c| c.sha == sha),
            )
        };
        let Some(commit) = commit else { return };
        let parent = commit.parents.first().cloned();
        let update_refs = if is_head {
            corvene_git::UpdateRefs::new()
        } else {
            let op = StackedOp::Reword {
                sha: sha.clone(),
                message: message.clone(),
            };
            match Self::stacked_update_refs(id, op, stacked, parent.as_deref(), cx) {
                Some(refs) => refs,
                None => return,
            }
        };
        let keep_messages = Self::rebase_keeps_messages(cx);
        let summary = split_message(&message).0;
        Self::state(cx).update(cx, |s, _| {
            s.repo_state_mut(id).rewritten_selection = vec![(summary, Some(commit.author.seconds))];
        });
        Self::run_history_op(
            id,
            "Could not edit the commit message",
            move |git, workdir| {
                if is_head {
                    corvene_git::reword_head(git, &workdir, &message)
                } else {
                    corvene_git::reword(
                        git,
                        &workdir,
                        &sha,
                        parent.as_deref(),
                        &message,
                        keep_messages,
                        &update_refs,
                    )
                }
            },
            cx,
        );
    }
}

/// A message's summary line and description (after the blank line).
/// Corvene `799-fixup-commits`: the current branch's unpushed `fixup!`
/// commits and the commits they fix ([`corvene_git::autosquash_pairs`] over
/// the loaded history), as `(fixup, target)` shas.
pub fn pending_fixups(rs: &RepositoryState) -> Vec<(String, String)> {
    if !rs.commits.iter().any(|c| {
        rs.local_commits.contains(&c.sha)
            && c.summary.starts_with(corvene_git::rebase_ops::FIXUP_PREFIX)
    }) {
        return Vec::new();
    }
    let oldest_first: Vec<CommitOneLine> = rs
        .commits
        .iter()
        .rev()
        .map(|c| CommitOneLine {
            sha: c.sha.clone(),
            summary: c.summary.clone(),
        })
        .collect();
    corvene_git::autosquash_pairs(&oldest_first)
        .into_iter()
        .filter(|(fixup, _)| rs.local_commits.contains(fixup))
        .collect()
}

/// Corvene `799-fixup-commits`: the commits Fixup Into offers, the current
/// branch's unpushed ones newest first (no merge commits, which a rebase
/// would flatten).
pub fn fixup_targets(rs: &RepositoryState) -> Vec<&Commit> {
    rs.commits
        .iter()
        .take_while(|c| rs.local_commits.contains(&c.sha))
        .filter(|c| {
            c.parents.len() <= 1 && !c.summary.starts_with(corvene_git::rebase_ops::FIXUP_PREFIX)
        })
        .collect()
}

fn split_message(message: &str) -> (String, String) {
    let (summary, rest) = message.split_once('\n').unwrap_or((message, ""));
    (summary.trim().to_string(), rest.trim().to_string())
}

fn last_retained_for_warn(mco: &Option<MultiCommitOperation>) -> Option<String> {
    match mco.as_ref().map(|m| &m.detail) {
        Some(McoDetail::Squash {
            last_retained_ref, ..
        })
        | Some(McoDetail::Reorder {
            last_retained_ref, ..
        })
        | Some(McoDetail::Autosquash {
            last_retained_ref, ..
        }) => last_retained_ref.clone(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_email_banner_only_for_unlinked_addresses() {
        let account = corvene_models::Account {
            endpoint: "https://api.github.com".into(),
            id: 7,
            login: "mona".into(),
            name: None,
            avatar_url: None,
            emails: vec!["Mona@Example.com".into()],
            scopes: Vec::new(),
            plan: None,
            private_primary_email: false,
            from_gh_cli: false,
        };
        assert_eq!(
            Banner::for_git_email(&account, Some("mona@example.com")),
            None
        );
        assert_eq!(
            Banner::for_git_email(&account, Some("7+mona@users.noreply.github.com")),
            None
        );
        assert_eq!(
            Banner::for_git_email(&account, Some("other@example.com")),
            Some(Banner::GitEmailMismatch {
                host: "github.com".into(),
                missing: false
            })
        );
        assert_eq!(
            Banner::for_git_email(&account, Some("  ")),
            Some(Banner::GitEmailMismatch {
                host: "github.com".into(),
                missing: true
            })
        );
    }

    #[test]
    fn rewritten_commits_are_found_by_summary_and_time() {
        let commit = |sha: &str, summary: &str, seconds: i64| {
            let who = corvene_models::CommitIdentity {
                name: "T".into(),
                email: "t@example.com".into(),
                seconds,
                offset: 0,
            };
            Commit {
                sha: sha.into(),
                summary: summary.into(),
                body: String::new(),
                author: who.clone(),
                committer: who,
                parents: Vec::new(),
                trailers: Vec::new(),
                tags: Vec::new(),
                signature: None,
            }
        };
        let log = [
            commit("n2", "b", 2),
            commit("n1", "a", 1),
            commit("old", "a", 5),
        ];
        let wanted = vec![("a".to_string(), Some(1)), ("b".to_string(), None)];
        assert_eq!(find_rewritten(&log, &wanted), ["n1", "n2"]);
        assert!(find_rewritten(&log, &[("c".to_string(), None)]).is_empty());
    }

    #[test]
    fn squash_drafts_match_the_same_commits() {
        let (a, b) = ("a".to_string(), "b".to_string());
        assert_eq!(
            squash_draft_key("o", [&a, &b].into_iter()),
            squash_draft_key("o", [&b, &a].into_iter())
        );
        assert_ne!(
            squash_draft_key("o", [&a].into_iter()),
            squash_draft_key("a", ["o".to_string()].iter())
        );
        assert_eq!(
            split_message("Title\n\nbody\nmore"),
            ("Title".to_string(), "body\nmore".to_string())
        );
        assert_eq!(split_message("Title"), ("Title".to_string(), String::new()));
    }

    #[test]
    fn merge_abort_message_names_the_files() {
        let one = merge_abort_blocked_message(&["b.txt".to_string()]);
        assert_eq!(
            one,
            "Could not abort the merge because b.txt changed after the merge started. Discard \
             or stash the changes to it, then abort again."
        );
        let two = merge_abort_blocked_message(&["a".to_string(), "b".to_string()]);
        assert!(
            two.contains("because a and b changed") && two.contains("to them"),
            "{two}"
        );
        let many: Vec<String> = (1..=7).map(|i| format!("f{i}")).collect();
        assert!(merge_abort_blocked_message(&many).contains("f1, f2, f3, f4, f5 and 2 more"));
    }

    #[test]
    fn conflict_state_follows_repository_markers() {
        let mut status = WorkingDirectoryStatus {
            branch: Some("main".into()),
            current_tip: Some("abc".into()),
            ..Default::default()
        };
        assert!(derive_conflict_state(&status, None).is_none());
        status.merge_head_found = true;
        let merge = derive_conflict_state(&status, None).unwrap();
        assert!(matches!(merge.kind, ConflictKind::Merge { .. }));
        // resolutions survive while the kind stays the same
        let mut with_resolution = merge.clone();
        with_resolution
            .manual_resolutions
            .insert("a.txt".into(), ManualConflictResolution::Ours);
        let again = derive_conflict_state(&status, Some(&with_resolution)).unwrap();
        assert_eq!(again.manual_resolutions.len(), 1);
        status.merge_head_found = false;
        status.cherry_pick_head_found = true;
        let pick = derive_conflict_state(&status, Some(&with_resolution)).unwrap();
        assert!(matches!(pick.kind, ConflictKind::CherryPick { .. }));
        assert!(pick.manual_resolutions.is_empty());
    }
}
