//! Menu item enabledness - GHD `app/src/lib/menu-update.ts` (`getMenuState`
//! and its builders) over the ids of `app/src/models/menu-ids.ts`.
//!
//! The decision is split in two: [`MenuInputs::from_app_state`] reads the
//! [`AppState`] into the handful of facts GHD's builders look at, and
//! [`menu_state_for`] turns those facts into the enabled state of each menu
//! id, so the rules can be tested without an `AppState`.
//!
//! Differences in the inputs (GHD `IAppState` → Corvene):
//! - `windowState !== 'hidden'`: `AppState` does not know whether the main
//!   window is hidden (⌘W), so the window counts as open.
//! - `resizablePaneActive` is focus state that lives in the UI
//!   (`corvene-ui/src/active_resizable.rs`), not in `AppState`; the two
//!   resizable ids are left undecided unless the caller sets
//!   [`MenuInputs::resizable_pane_active`].
//! - `CloningRepository` selections do not exist: a clone in progress lives
//!   in `AppState::cloning` and is never the selected repository.
//! - `branchesState.upstreamDefaultBranch` is not kept by Corvene; it is
//!   derived here from the branch list (see
//!   [`contribution_target_default_branch`]).
//! - `gitHubRepository.issuesEnabled` is not in Corvene's model, so it counts
//!   as unknown (GHD: `!== false`, i.e. enabled); only `archived` disables.
//! - `enableWorktreeSupport()` is `true` in GHD 3.6.6 and worktrees are
//!   always on in Corvene, so the worktree ids are never forced off.
//!
//! Deviation (`424-conflicts-dialog-keeps-open-items`): while the
//! merge-conflicts dialog is open, Show in Finder, Open in Editor, Open in
//! Shell and View on GitHub keep their usual state (GHD disables every item
//! while any popup is open).

use std::collections::HashMap;

use corvene_models::{Branch, BranchKind, Repository, Tip, WorkingDirectoryStatus};

use crate::forks::UPSTREAM_REMOTE_NAME;
use crate::mco::{ConflictKind, ConflictState};
use crate::state::{AppState, RepositoryState};

/// GHD `MenuIDs` (`app/src/models/menu-ids.ts`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MenuId {
    RenameBranch,
    DeleteBranch,
    DiscardAllChanges,
    StashAllChanges,
    Preferences,
    UpdateBranchWithContributionTargetBranch,
    MergeBranch,
    SquashAndMergeBranch,
    RebaseBranch,
    ViewRepositoryOnGithub,
    CompareOnGithub,
    BranchOnGithub,
    OpenInShell,
    Push,
    Pull,
    Fetch,
    Branch,
    Repository,
    GoToCommitMessage,
    CreateBranch,
    ShowChanges,
    ShowHistory,
    ShowRepositoryList,
    ShowBranchesList,
    OpenWorkingDirectory,
    ShowRepositorySettings,
    OpenExternalEditor,
    OpenWithExternalEditor,
    RemoveRepository,
    NewRepository,
    AddLocalRepository,
    CloneRepository,
    About,
    CreatePullRequest,
    CompareToBranch,
    ToggleStashedChanges,
    CreateIssueInRepositoryOnGithub,
    CreateWorktree,
    ShowWorktreesList,
    PreviewPullRequest,
    DecreaseActiveResizableWidth,
    IncreaseActiveResizableWidth,
    ToggleChangesFilter,
}

impl MenuId {
    /// Every `MenuIDs` member, in `menu-ids.ts` order.
    pub const ALL: [MenuId; 43] = [
        Self::RenameBranch,
        Self::DeleteBranch,
        Self::DiscardAllChanges,
        Self::StashAllChanges,
        Self::Preferences,
        Self::UpdateBranchWithContributionTargetBranch,
        Self::MergeBranch,
        Self::SquashAndMergeBranch,
        Self::RebaseBranch,
        Self::ViewRepositoryOnGithub,
        Self::CompareOnGithub,
        Self::BranchOnGithub,
        Self::OpenInShell,
        Self::Push,
        Self::Pull,
        Self::Fetch,
        Self::Branch,
        Self::Repository,
        Self::GoToCommitMessage,
        Self::CreateBranch,
        Self::ShowChanges,
        Self::ShowHistory,
        Self::ShowRepositoryList,
        Self::ShowBranchesList,
        Self::OpenWorkingDirectory,
        Self::ShowRepositorySettings,
        Self::OpenExternalEditor,
        Self::OpenWithExternalEditor,
        Self::RemoveRepository,
        Self::NewRepository,
        Self::AddLocalRepository,
        Self::CloneRepository,
        Self::About,
        Self::CreatePullRequest,
        Self::CompareToBranch,
        Self::ToggleStashedChanges,
        Self::CreateIssueInRepositoryOnGithub,
        Self::CreateWorktree,
        Self::ShowWorktreesList,
        Self::PreviewPullRequest,
        Self::DecreaseActiveResizableWidth,
        Self::IncreaseActiveResizableWidth,
        Self::ToggleChangesFilter,
    ];

    /// GHD's id string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RenameBranch => "rename-branch",
            Self::DeleteBranch => "delete-branch",
            Self::DiscardAllChanges => "discard-all-changes",
            Self::StashAllChanges => "stash-all-changes",
            Self::Preferences => "preferences",
            Self::UpdateBranchWithContributionTargetBranch => {
                "update-branch-with-contribution-target-branch"
            }
            Self::MergeBranch => "merge-branch",
            Self::SquashAndMergeBranch => "squash-and-merge-branch",
            Self::RebaseBranch => "rebase-branch",
            Self::ViewRepositoryOnGithub => "view-repository-on-github",
            Self::CompareOnGithub => "compare-on-github",
            Self::BranchOnGithub => "branch-on-github",
            Self::OpenInShell => "open-in-shell",
            Self::Push => "push",
            Self::Pull => "pull",
            Self::Fetch => "fetch",
            Self::Branch => "branch",
            Self::Repository => "repository",
            Self::GoToCommitMessage => "go-to-commit-message",
            Self::CreateBranch => "create-branch",
            Self::ShowChanges => "show-changes",
            Self::ShowHistory => "show-history",
            Self::ShowRepositoryList => "show-repository-list",
            Self::ShowBranchesList => "show-branches-list",
            Self::OpenWorkingDirectory => "open-working-directory",
            Self::ShowRepositorySettings => "show-repository-settings",
            Self::OpenExternalEditor => "open-external-editor",
            Self::OpenWithExternalEditor => "open-with-external-editor",
            Self::RemoveRepository => "remove-repository",
            Self::NewRepository => "new-repository",
            Self::AddLocalRepository => "add-local-repository",
            Self::CloneRepository => "clone-repository",
            Self::About => "about",
            Self::CreatePullRequest => "create-pull-request",
            Self::CompareToBranch => "compare-to-branch",
            Self::ToggleStashedChanges => "toggle-stashed-changes",
            Self::CreateIssueInRepositoryOnGithub => "create-issue-in-repository-on-github",
            Self::CreateWorktree => "create-worktree",
            Self::ShowWorktreesList => "show-worktrees-list",
            Self::PreviewPullRequest => "preview-pull-request",
            Self::DecreaseActiveResizableWidth => "decrease-active-resizable-width",
            Self::IncreaseActiveResizableWidth => "increase-active-resizable-width",
            Self::ToggleChangesFilter => "toggle-changes-filter",
        }
    }

    /// The id for GHD's id string.
    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.as_str() == id)
    }
}

/// GHD `allMenuIds`: the ids a popup disables and `getAllMenusEnabledBuilder`
/// enables.
pub const ALL_MENU_IDS: [MenuId; 39] = [
    MenuId::RenameBranch,
    MenuId::DeleteBranch,
    MenuId::DiscardAllChanges,
    MenuId::StashAllChanges,
    MenuId::Preferences,
    MenuId::UpdateBranchWithContributionTargetBranch,
    MenuId::CompareToBranch,
    MenuId::MergeBranch,
    MenuId::RebaseBranch,
    MenuId::ViewRepositoryOnGithub,
    MenuId::CompareOnGithub,
    MenuId::BranchOnGithub,
    MenuId::OpenInShell,
    MenuId::Push,
    MenuId::Pull,
    MenuId::Fetch,
    MenuId::Branch,
    MenuId::Repository,
    MenuId::GoToCommitMessage,
    MenuId::CreateBranch,
    MenuId::ShowChanges,
    MenuId::ShowHistory,
    MenuId::ShowRepositoryList,
    MenuId::ShowBranchesList,
    MenuId::OpenWorkingDirectory,
    MenuId::ShowRepositorySettings,
    MenuId::OpenExternalEditor,
    MenuId::OpenWithExternalEditor,
    MenuId::RemoveRepository,
    MenuId::NewRepository,
    MenuId::AddLocalRepository,
    MenuId::CloneRepository,
    MenuId::About,
    MenuId::CreatePullRequest,
    MenuId::PreviewPullRequest,
    MenuId::SquashAndMergeBranch,
    MenuId::ToggleStashedChanges,
    MenuId::CreateWorktree,
    MenuId::ShowWorktreesList,
];

/// `getRepositoryMenuBuilder`'s `repositoryScopedIDs`: enabled exactly when a
/// repository is active.
const REPOSITORY_SCOPED_IDS: [MenuId; 16] = [
    MenuId::Branch,
    MenuId::Repository,
    MenuId::RemoveRepository,
    MenuId::OpenInShell,
    MenuId::OpenWorkingDirectory,
    MenuId::ShowRepositorySettings,
    MenuId::GoToCommitMessage,
    MenuId::ShowChanges,
    MenuId::ShowHistory,
    MenuId::ShowBranchesList,
    MenuId::ShowWorktreesList,
    MenuId::OpenExternalEditor,
    MenuId::OpenWithExternalEditor,
    MenuId::CompareToBranch,
    MenuId::ToggleChangesFilter,
    MenuId::CreateWorktree,
];

/// `getInWelcomeFlowBuilder`'s `welcomeScopedIds`.
const WELCOME_SCOPED_IDS: [MenuId; 5] = [
    MenuId::NewRepository,
    MenuId::AddLocalRepository,
    MenuId::CloneRepository,
    MenuId::Preferences,
    MenuId::About,
];

/// `getNoRepositoriesBuilder`'s `noRepositoriesDisabledIds`.
const NO_REPOSITORIES_DISABLED_IDS: [MenuId; 1] = [MenuId::ShowRepositoryList];

/// GHD `SelectionType` of the selected repository (`Repository` or
/// `MissingRepository`; Corvene never selects a cloning repository).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionKind {
    Repository,
    MissingRepository,
}

/// The selected repository as `getRepositoryMenuBuilder` sees it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectedRepository {
    pub missing: bool,
    /// `repository.gitHubRepository` is set.
    pub has_github_repository: bool,
    /// GHD `isRepositoryHostedOnGitHub`.
    pub hosted_on_github: bool,
    /// GHD `getRepoIssuesEnabled`.
    pub repo_issues_enabled: bool,
    /// The branch and working directory facts; only read for a repository
    /// that is not missing (GHD leaves them `false` otherwise).
    pub facts: RepositoryFacts,
}

impl SelectedRepository {
    pub fn kind(&self) -> SelectionKind {
        if self.missing {
            SelectionKind::MissingRepository
        } else {
            SelectionKind::Repository
        }
    }
}

/// GHD `TipState` without its payload.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TipKind {
    #[default]
    Unknown,
    Unborn,
    Detached,
    Valid,
}

impl From<&Tip> for TipKind {
    fn from(tip: &Tip) -> Self {
        match tip {
            Tip::Unknown => Self::Unknown,
            Tip::Unborn { .. } => Self::Unborn,
            Tip::Detached { .. } => Self::Detached,
            Tip::Valid { .. } => Self::Valid,
        }
    }
}

/// The locals `getRepositoryMenuBuilder` computes for a selected (not
/// missing) repository.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RepositoryFacts {
    pub on_non_default_branch: bool,
    pub on_branch: bool,
    pub on_detached_head: bool,
    pub has_changed_files: bool,
    pub has_conflicts: bool,
    pub has_published_branch: bool,
    pub network_action_in_progress: bool,
    pub has_remote: bool,
    pub tip_state_is_unknown: bool,
    pub branch_is_unborn: bool,
    pub rebase_in_progress: bool,
    pub branch_has_stash_entry: bool,
    pub on_contribution_target_default_branch: bool,
    pub has_contribution_target_default_branch: bool,
}

/// What a selected repository's state holds, for [`RepositoryFacts::derive`].
#[derive(Clone, Copy, Debug)]
pub struct RepositorySnapshot<'a> {
    /// `branchesState.tip`
    pub tip: &'a Tip,
    /// `branchesState.defaultBranch`'s name
    pub default_branch: Option<&'a str>,
    /// `findContributionTargetDefaultBranch`'s name
    pub contribution_target: Option<&'a str>,
    /// `changesState.stashEntry !== null`
    pub has_stash_entry: bool,
    /// `isPushPullFetchInProgress`
    pub push_pull_fetch_in_progress: bool,
    /// `remote !== null`
    pub has_remote: bool,
    /// `changesState.conflictState`
    pub conflict_state: Option<&'a ConflictState>,
    /// `changesState.workingDirectory`
    pub working_directory: Option<&'a WorkingDirectoryStatus>,
}

impl RepositoryFacts {
    /// The `selectedState.type === SelectionType.Repository` block of GHD
    /// `getRepositoryMenuBuilder`.
    pub fn derive(s: &RepositorySnapshot<'_>) -> Self {
        let tip = TipKind::from(s.tip);
        let mut facts = Self {
            on_branch: tip == TipKind::Valid,
            on_detached_head: tip == TipKind::Detached,
            tip_state_is_unknown: tip == TipKind::Unknown,
            branch_is_unborn: tip == TipKind::Unborn,
            has_contribution_target_default_branch: s.contribution_target.is_some(),
            ..Self::default()
        };

        // If we are on the default branch, on an unborn branch or on a
        // detached HEAD there's not much we can do.
        if let Tip::Valid { branch } = s.tip {
            facts.on_contribution_target_default_branch =
                s.contribution_target == Some(branch.name.as_str());
            facts.on_non_default_branch = match s.default_branch {
                Some(default) => branch.name != default,
                None => true,
            };
            facts.has_published_branch = branch.upstream.is_some();
            facts.branch_has_stash_entry = s.has_stash_entry;
        } else {
            facts.on_non_default_branch = true;
        }

        facts.network_action_in_progress = s.push_pull_fetch_in_progress;
        facts.has_remote = s.has_remote;
        facts.rebase_in_progress = s
            .conflict_state
            .is_some_and(|c| matches!(c.kind, ConflictKind::Rebase { .. }));
        // GHD `hasConflictedFiles`
        facts.has_conflicts = s.conflict_state.is_some()
            || s.working_directory
                .is_some_and(WorkingDirectoryStatus::has_conflicts);
        facts.has_changed_files = s.working_directory.is_some_and(|w| !w.files.is_empty());
        facts
    }
}

/// Everything GHD `getMenuState` reads from `IAppState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuInputs {
    /// `currentPopup !== null`
    pub popup_open: bool,
    /// The open popup is the merge-conflicts dialog and
    /// `424-conflicts-dialog-keeps-open-items` is on.
    pub conflicts_dialog_open: bool,
    /// `windowState !== 'hidden'`
    pub window_open: bool,
    /// `showWelcomeFlow`
    pub show_welcome_flow: bool,
    /// `repositories.length`
    pub repository_count: usize,
    /// `resizablePaneActive`; `None` leaves the two resizable ids undecided.
    pub resizable_pane_active: Option<bool>,
    /// `selectedState` for a repository (`None`: nothing selected).
    pub selected: Option<SelectedRepository>,
}

impl MenuInputs {
    /// Read `state` the way GHD's builders read `IAppState`.
    pub fn from_app_state(state: &AppState) -> Self {
        let configured_default = state
            .global_git
            .as_ref()
            .map(|g| g.default_branch.as_str())
            .filter(|b| !b.is_empty())
            .unwrap_or("main");
        let selected = state.selected_repository().map(|repository| {
            let facts = if repository.missing {
                RepositoryFacts::default()
            } else {
                repository_facts(
                    repository,
                    state.repo_states.get(&repository.id),
                    configured_default,
                )
            };
            SelectedRepository {
                missing: repository.missing,
                has_github_repository: repository.github.is_some(),
                // flags 342-344: a GitLab, Gitea or Bitbucket repository gets
                // the same items (View on, Compare, Create Pull Request…)
                hosted_on_github: is_repository_hosted_on_github(repository)
                    || state.hosted_repository(repository.id).is_some(),
                repo_issues_enabled: repo_issues_enabled(repository)
                    || state
                        .hosted_repository(repository.id)
                        .is_some_and(|h| !h.repo.archived),
                facts,
            }
        });
        // `424-conflicts-dialog-keeps-open-items`
        let conflicts_dialog_open = state
            .flags
            .bool(crate::flags::ids::CONFLICTS_DIALOG_KEEPS_OPEN_ITEMS)
            && match state.popup() {
                Some(crate::state::Popup::MultiCommitOperation { repo, .. }) => state
                    .repo_states
                    .get(repo)
                    .and_then(|rs| rs.mco.as_ref())
                    .is_some_and(|m| m.step == crate::mco::McoStep::ShowConflicts),
                _ => false,
            };
        Self {
            popup_open: state.popups.is_a_popup_open(),
            conflicts_dialog_open,
            // `AppState` has no hidden-window state (⌘W hides the window in
            // the UI layer only): treat the window as open.
            window_open: true,
            show_welcome_flow: !state.settings.welcome_completed,
            repository_count: state.repositories.len(),
            resizable_pane_active: None,
            selected,
        }
    }
}

/// [`RepositoryFacts`] for a selected repository and its state. A repository
/// without a state yet (or not read yet) is GHD's initial state: tip
/// `Unknown`, no remote, no changes.
fn repository_facts(
    repository: &Repository,
    repo_state: Option<&RepositoryState>,
    configured_default: &str,
) -> RepositoryFacts {
    let info = repo_state.and_then(|r| r.info.as_ref());
    let tip = info.map(|i| &i.tip).unwrap_or(&Tip::Unknown);
    let branches = info.map(|i| i.branches.as_slice()).unwrap_or(&[]);
    let default_branch = repo_state.and_then(|r| r.default_branch.as_deref());
    RepositoryFacts::derive(&RepositorySnapshot {
        tip,
        default_branch,
        // `1202-update-from-parent-branch`: Update from <parent>
        // (`update_parent` is only read while the flag is on)
        contribution_target: repo_state
            .and_then(|r| r.update_parent.as_deref())
            .or_else(|| {
                contribution_target_default_branch(
                    repository,
                    branches,
                    default_branch,
                    configured_default,
                )
            }),
        has_stash_entry: repo_state.is_some_and(|r| r.stash.is_some()),
        push_pull_fetch_in_progress: repo_state.is_some_and(|r| r.push_pull_in_progress),
        has_remote: info.is_some_and(|i| corvene_git::find_default_remote(&i.remotes).is_some()),
        conflict_state: repo_state.and_then(|r| r.conflict_state.as_ref()),
        working_directory: repo_state.and_then(|r| r.status.as_deref()),
    })
}

/// GHD `findContributionTargetDefaultBranch` (`lib/branch.ts`): for a GitHub
/// repository the upstream default branch if there is one, else the default
/// branch.
///
/// GHD keeps `upstreamDefaultBranch` in the git store
/// (`git-store.ts#refreshDefaultBranch`): for a fork contributing to its
/// parent, the `upstream` remote branch named after `upstream`'s remote HEAD,
/// else `getDefaultBranch()`. Corvene does not read `upstream`'s remote HEAD;
/// the parent's API `default_branch` stands in for it, then the configured
/// default branch.
pub fn contribution_target_default_branch<'a>(
    repository: &'a Repository,
    branches: &'a [Branch],
    default_branch: Option<&'a str>,
    configured_default: &'a str,
) -> Option<&'a str> {
    if repository.github.is_none() {
        return default_branch;
    }
    upstream_default_branch(repository, branches, configured_default).or(default_branch)
}

impl crate::state::AppState {
    /// GHD `branchesState.upstreamDefaultBranch`: `upstream/<default>` of a
    /// fork contributing to its parent.
    pub fn upstream_default_branch_of(&self, id: u64) -> Option<&Branch> {
        let rs = self.repo_states.get(&id)?;
        let branches = rs.info.as_ref()?.branches.as_slice();
        let name = upstream_default_branch(
            self.repository(id)?,
            branches,
            rs.default_branch.as_deref().unwrap_or("main"),
        )?;
        branches.iter().find(|b| b.name == name)
    }
}

/// GHD `git-store.ts#refreshDefaultBranch`'s `upstreamDefaultBranch`.
fn upstream_default_branch<'a>(
    repository: &'a Repository,
    branches: &'a [Branch],
    configured_default: &'a str,
) -> Option<&'a str> {
    // `getNonForkGitHubRepository(repository) === repository.gitHubRepository`
    if !repository.is_fork_contributing_to_parent() {
        return None;
    }
    let head = repository
        .github
        .as_ref()
        .and_then(|gh| gh.parent.as_ref())
        .and_then(|p| p.default_branch.as_deref())
        .unwrap_or(configured_default);
    branches
        .iter()
        .find(|b| {
            b.kind == BranchKind::Remote
                && remote_name_of(b) == Some(UPSTREAM_REMOTE_NAME)
                && b.name_without_remote() == head
        })
        .map(|b| b.name.as_str())
}

/// A remote branch's `remoteName`.
fn remote_name_of(branch: &Branch) -> Option<&str> {
    branch
        .remote_name
        .as_deref()
        .or_else(|| branch.name.split_once('/').map(|(remote, _)| remote))
}

/// GHD `isRepositoryHostedOnGitHub`: a GitHub repository with an HTML URL.
fn is_repository_hosted_on_github(repository: &Repository) -> bool {
    repository
        .github
        .as_ref()
        .is_some_and(|gh| !gh.html_url.is_empty())
}

/// GHD `getRepoIssuesEnabled`: issues are enabled on the parent of a fork,
/// else on the repository, and it is not archived. Corvene's model has no
/// `issuesEnabled`, which GHD treats as enabled unless it is `false`.
fn repo_issues_enabled(repository: &Repository) -> bool {
    repository
        .github
        .as_ref()
        .is_some_and(|gh| match &gh.parent {
            Some(parent) => !parent.archived,
            None => !gh.archived,
        })
}

/// GHD `MenuStateBuilder`: coalesces updates to menu items.
#[derive(Clone, Debug, Default)]
struct MenuStateBuilder {
    state: HashMap<MenuId, bool>,
}

impl MenuStateBuilder {
    fn enable(&mut self, id: MenuId) -> &mut Self {
        self.set_enabled(id, true)
    }

    fn disable(&mut self, id: MenuId) -> &mut Self {
        self.set_enabled(id, false)
    }

    fn set_enabled(&mut self, id: MenuId, enabled: bool) -> &mut Self {
        self.state.insert(id, enabled);
        self
    }

    /// Values in `other` replace those in `self`.
    fn merge(mut self, other: MenuStateBuilder) -> Self {
        self.state.extend(other.state);
        self
    }
}

/// GHD `getAllMenusDisabledBuilder`
fn all_menus_disabled_builder() -> MenuStateBuilder {
    let mut builder = MenuStateBuilder::default();
    for id in ALL_MENU_IDS {
        builder.disable(id);
    }
    builder
}

/// GHD `getAllMenusEnabledBuilder`
fn all_menus_enabled_builder() -> MenuStateBuilder {
    let mut builder = MenuStateBuilder::default();
    for id in ALL_MENU_IDS {
        builder.enable(id);
    }
    builder
}

/// GHD `getRepositoryMenuBuilder`
fn repository_menu_builder(inputs: &MenuInputs) -> MenuStateBuilder {
    let selected = inputs.selected;
    let is_hosted_on_github = selected.is_some_and(|s| s.hosted_on_github);
    let repo_issues_enabled = selected.is_some_and(|s| s.repo_issues_enabled);
    let repository = selected.filter(|s| s.kind() == SelectionKind::Repository);
    let repository_selected = repository.is_some();
    let f = repository.map(|s| s.facts).unwrap_or_default();
    let missing = selected.filter(|s| s.kind() == SelectionKind::MissingRepository);

    let mut b = MenuStateBuilder::default();
    let repository_active = inputs.window_open && repository_selected && !inputs.show_welcome_flow;

    if repository_active {
        for id in REPOSITORY_SCOPED_IDS {
            b.enable(id);
        }
        // `enableWorktreeSupport()` is always true (GHD 3.6.6 and Corvene),
        // so `show-worktrees-list` / `create-worktree` stay enabled.

        b.set_enabled(
            MenuId::RenameBranch,
            (f.on_non_default_branch || !f.has_published_branch)
                && !f.branch_is_unborn
                && !f.on_detached_head,
        );
        b.set_enabled(
            MenuId::DeleteBranch,
            f.on_non_default_branch && !f.branch_is_unborn && !f.on_detached_head,
        );
        b.set_enabled(
            MenuId::UpdateBranchWithContributionTargetBranch,
            f.on_branch
                && f.has_contribution_target_default_branch
                && !f.on_contribution_target_default_branch,
        );
        b.set_enabled(MenuId::MergeBranch, f.on_branch);
        b.set_enabled(MenuId::SquashAndMergeBranch, f.on_branch);
        b.set_enabled(MenuId::RebaseBranch, f.on_branch);
        b.set_enabled(
            MenuId::CompareOnGithub,
            is_hosted_on_github && f.has_published_branch,
        );
        b.set_enabled(
            MenuId::BranchOnGithub,
            is_hosted_on_github && f.has_published_branch,
        );
        b.set_enabled(MenuId::ViewRepositoryOnGithub, is_hosted_on_github);
        b.set_enabled(MenuId::CreateIssueInRepositoryOnGithub, repo_issues_enabled);
        b.set_enabled(
            MenuId::CreatePullRequest,
            is_hosted_on_github && !f.branch_is_unborn && !f.on_detached_head,
        );
        b.set_enabled(
            MenuId::PreviewPullRequest,
            !f.branch_is_unborn && !f.on_detached_head && is_hosted_on_github,
        );
        b.set_enabled(
            MenuId::Push,
            !f.branch_is_unborn && !f.on_detached_head && !f.network_action_in_progress,
        );
        b.set_enabled(
            MenuId::Pull,
            f.has_published_branch && !f.network_action_in_progress,
        );
        b.set_enabled(MenuId::Fetch, f.has_remote && !f.network_action_in_progress);
        b.set_enabled(
            MenuId::CreateBranch,
            !f.tip_state_is_unknown && !f.branch_is_unborn && !f.rebase_in_progress,
        );
        b.set_enabled(
            MenuId::DiscardAllChanges,
            repository_active && f.has_changed_files && !f.rebase_in_progress,
        );
        b.set_enabled(
            MenuId::StashAllChanges,
            f.has_changed_files && f.on_branch && !f.rebase_in_progress && !f.has_conflicts,
        );
        b.set_enabled(MenuId::CompareToBranch, !f.on_detached_head);
        b.set_enabled(MenuId::ToggleStashedChanges, f.branch_has_stash_entry);

        // Unreachable in GHD too (a missing repository is never active);
        // kept for parity.
        if missing.is_some() {
            b.disable(MenuId::OpenExternalEditor);
            b.disable(MenuId::OpenWithExternalEditor);
        }
    } else {
        for id in REPOSITORY_SCOPED_IDS {
            b.disable(id);
        }

        b.disable(MenuId::ViewRepositoryOnGithub);
        b.disable(MenuId::CreatePullRequest);
        b.disable(MenuId::PreviewPullRequest);
        if let Some(missing) = missing {
            if missing.has_github_repository {
                b.enable(MenuId::ViewRepositoryOnGithub);
            }
            b.enable(MenuId::RemoveRepository);
        }

        for id in [
            MenuId::CreateBranch,
            MenuId::RenameBranch,
            MenuId::DeleteBranch,
            MenuId::DiscardAllChanges,
            MenuId::StashAllChanges,
            MenuId::UpdateBranchWithContributionTargetBranch,
            MenuId::MergeBranch,
            MenuId::SquashAndMergeBranch,
            MenuId::RebaseBranch,
            MenuId::Push,
            MenuId::Pull,
            MenuId::Fetch,
            MenuId::CompareToBranch,
            MenuId::CompareOnGithub,
            MenuId::BranchOnGithub,
            MenuId::ToggleStashedChanges,
        ] {
            b.disable(id);
        }
    }

    b
}

/// GHD `getInWelcomeFlowBuilder`
fn in_welcome_flow_builder(in_welcome_flow: bool) -> MenuStateBuilder {
    let mut b = MenuStateBuilder::default();
    for id in WELCOME_SCOPED_IDS {
        b.set_enabled(id, !in_welcome_flow);
    }
    b
}

/// GHD `getNoRepositoriesBuilder`
fn no_repositories_builder(inputs: &MenuInputs) -> MenuStateBuilder {
    let mut b = MenuStateBuilder::default();
    if inputs.repository_count == 0 {
        for id in NO_REPOSITORIES_DISABLED_IDS {
            b.disable(id);
        }
    }
    b
}

/// GHD `getAppMenuBuilder`
fn app_menu_builder(inputs: &MenuInputs) -> MenuStateBuilder {
    let mut b = MenuStateBuilder::default();
    if let Some(enabled) = inputs.resizable_pane_active {
        b.set_enabled(MenuId::IncreaseActiveResizableWidth, enabled);
        b.set_enabled(MenuId::DecreaseActiveResizableWidth, enabled);
    }
    b
}

/// Corvene (`424-conflicts-dialog-keeps-open-items`): the items that stay as
/// usual while the merge-conflicts dialog is open, to look at the conflicted
/// files.
const CONFLICTS_DIALOG_KEPT_IDS: [MenuId; 4] = [
    MenuId::OpenWorkingDirectory,
    MenuId::OpenExternalEditor,
    MenuId::OpenInShell,
    MenuId::ViewRepositoryOnGithub,
];

/// GHD `getMenuState` over already-read inputs.
pub fn menu_state_for(inputs: &MenuInputs) -> HashMap<MenuId, bool> {
    if inputs.popup_open {
        let mut disabled = all_menus_disabled_builder();
        if inputs.conflicts_dialog_open {
            let usual = menu_state_for(&MenuInputs {
                popup_open: false,
                conflicts_dialog_open: false,
                ..*inputs
            });
            for id in CONFLICTS_DIALOG_KEPT_IDS {
                disabled.set_enabled(id, usual.get(&id).copied().unwrap_or(true));
            }
        }
        return disabled.state;
    }
    all_menus_enabled_builder()
        .merge(repository_menu_builder(inputs))
        .merge(app_menu_builder(inputs))
        .merge(in_welcome_flow_builder(inputs.show_welcome_flow))
        .merge(no_repositories_builder(inputs))
        .state
}

/// GHD `getMenuState`: the enabled state of every menu item id the state
/// decides; ids absent from the map keep their default (enabled).
pub fn menu_state(state: &AppState) -> HashMap<MenuId, bool> {
    menu_state_for(&MenuInputs::from_app_state(state))
}

/// Whether `id` is enabled in `state` (`menu_state`, defaulting to enabled).
pub fn is_enabled(state: &AppState, id: MenuId) -> bool {
    menu_state(state).get(&id).copied().unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use corvene_models::{
        DiffSelection, FileStatus, FileStatusKind, ForkContributionTarget, GitHubRepository,
        GitStatusEntry, WorkingDirectoryFileChange,
    };

    use super::*;

    fn branch(name: &str, kind: BranchKind, upstream: Option<&str>) -> Branch {
        Branch {
            name: name.into(),
            kind,
            full_name: match kind {
                BranchKind::Local => format!("refs/heads/{name}"),
                BranchKind::Remote => format!("refs/remotes/{name}"),
            },
            tip: None,
            upstream: upstream.map(str::to_string),
            tip_time: None,
            tip_author: None,
            remote_name: None,
        }
    }

    fn valid(name: &str, upstream: Option<&str>) -> Tip {
        Tip::Valid {
            branch: branch(name, BranchKind::Local, upstream),
        }
    }

    fn snapshot(tip: &Tip) -> RepositorySnapshot<'_> {
        RepositorySnapshot {
            tip,
            default_branch: Some("main"),
            contribution_target: Some("main"),
            has_stash_entry: false,
            push_pull_fetch_in_progress: false,
            has_remote: true,
            conflict_state: None,
            working_directory: None,
        }
    }

    fn github(name: &str) -> GitHubRepository {
        GitHubRepository {
            endpoint: "https://api.github.com".into(),
            owner: "octocat".into(),
            name: name.into(),
            html_url: format!("https://github.com/octocat/{name}"),
            clone_url: format!("https://github.com/octocat/{name}.git"),
            default_branch: Some("main".into()),
            private: false,
            fork: false,
            parent: None,
            archived: false,
            permissions: None,
            allow_forking: None,
            node_id: None,
        }
    }

    fn inputs(selected: Option<SelectedRepository>) -> MenuInputs {
        MenuInputs {
            popup_open: false,
            conflicts_dialog_open: false,
            window_open: true,
            show_welcome_flow: false,
            repository_count: 1,
            resizable_pane_active: None,
            selected,
        }
    }

    fn on_github(facts: RepositoryFacts) -> SelectedRepository {
        SelectedRepository {
            missing: false,
            has_github_repository: true,
            hosted_on_github: true,
            repo_issues_enabled: true,
            facts,
        }
    }

    fn modified_file(path: &str) -> WorkingDirectoryFileChange {
        WorkingDirectoryFileChange {
            path: path.to_string(),
            old_path: None,
            status: FileStatus {
                kind: FileStatusKind::Modified,
                index: GitStatusEntry::Unchanged,
                working_tree: GitStatusEntry::Modified,
                score: None,
                code: ".M".into(),
                submodule: false,
                submodule_status: None,
                conflict_markers: None,
            },
            selection: DiffSelection::all(),
        }
    }

    fn enabled(state: &HashMap<MenuId, bool>, id: MenuId) -> bool {
        state.get(&id).copied().unwrap_or(true)
    }

    #[test]
    fn ids_round_trip() {
        for id in MenuId::ALL {
            assert_eq!(MenuId::parse(id.as_str()), Some(id));
        }
        for id in ALL_MENU_IDS {
            assert!(MenuId::ALL.contains(&id));
        }
        assert_eq!(MenuId::parse("nope"), None);
    }

    #[test]
    fn popup_disables_every_menu_id() {
        let facts = RepositoryFacts::derive(&snapshot(&valid("feature", Some("x"))));
        let state = menu_state_for(&MenuInputs {
            popup_open: true,
            ..inputs(Some(on_github(facts)))
        });
        for id in ALL_MENU_IDS {
            assert_eq!(state.get(&id), Some(&false), "{}", id.as_str());
        }
        assert_eq!(state.len(), ALL_MENU_IDS.len());
    }

    #[test]
    fn conflicts_dialog_keeps_the_open_items() {
        let facts = RepositoryFacts::derive(&snapshot(&valid("feature", Some("x"))));
        let state = menu_state_for(&MenuInputs {
            popup_open: true,
            conflicts_dialog_open: true,
            ..inputs(Some(on_github(facts)))
        });
        for id in ALL_MENU_IDS {
            let kept = CONFLICTS_DIALOG_KEPT_IDS.contains(&id);
            assert_eq!(state.get(&id), Some(&kept), "{}", id.as_str());
        }
    }

    #[test]
    fn no_repository_selected() {
        let state = menu_state_for(&inputs(None));
        for id in REPOSITORY_SCOPED_IDS {
            assert!(!enabled(&state, id), "{}", id.as_str());
        }
        for id in [
            MenuId::Push,
            MenuId::Pull,
            MenuId::Fetch,
            MenuId::CreateBranch,
        ] {
            assert!(!enabled(&state, id));
        }
        for id in WELCOME_SCOPED_IDS {
            assert!(enabled(&state, id));
        }
        assert!(enabled(&state, MenuId::ShowRepositoryList));
    }

    #[test]
    fn missing_repository_with_github_repository() {
        let selected = SelectedRepository {
            missing: true,
            has_github_repository: true,
            hosted_on_github: true,
            repo_issues_enabled: true,
            facts: RepositoryFacts::default(),
        };
        let state = menu_state_for(&inputs(Some(selected)));
        assert!(enabled(&state, MenuId::ViewRepositoryOnGithub));
        assert!(enabled(&state, MenuId::RemoveRepository));
        assert!(!enabled(&state, MenuId::OpenExternalEditor));
        assert!(!enabled(&state, MenuId::ShowChanges));
        assert!(!enabled(&state, MenuId::CreatePullRequest));
        assert!(!enabled(&state, MenuId::Fetch));

        let local = SelectedRepository {
            has_github_repository: false,
            hosted_on_github: false,
            ..selected
        };
        let state = menu_state_for(&inputs(Some(local)));
        assert!(!enabled(&state, MenuId::ViewRepositoryOnGithub));
        assert!(enabled(&state, MenuId::RemoveRepository));
    }

    #[test]
    fn feature_branch_on_github() {
        let tip = valid("feature", Some("refs/remotes/origin/feature"));
        let state = menu_state_for(&inputs(Some(on_github(RepositoryFacts::derive(
            &snapshot(&tip),
        )))));
        for id in [
            MenuId::RenameBranch,
            MenuId::DeleteBranch,
            MenuId::UpdateBranchWithContributionTargetBranch,
            MenuId::MergeBranch,
            MenuId::CompareOnGithub,
            MenuId::CreatePullRequest,
            MenuId::Push,
            MenuId::Pull,
            MenuId::Fetch,
            MenuId::CreateBranch,
            MenuId::CreateWorktree,
            MenuId::ShowWorktreesList,
            MenuId::CreateIssueInRepositoryOnGithub,
        ] {
            assert!(enabled(&state, id), "{}", id.as_str());
        }
        // nothing to discard or stash
        assert!(!enabled(&state, MenuId::DiscardAllChanges));
        assert!(!enabled(&state, MenuId::StashAllChanges));
        assert!(!enabled(&state, MenuId::ToggleStashedChanges));
    }

    #[test]
    fn published_default_branch() {
        let tip = valid("main", Some("refs/remotes/origin/main"));
        let state = menu_state_for(&inputs(Some(on_github(RepositoryFacts::derive(
            &snapshot(&tip),
        )))));
        assert!(!enabled(&state, MenuId::RenameBranch));
        assert!(!enabled(&state, MenuId::DeleteBranch));
        assert!(!enabled(
            &state,
            MenuId::UpdateBranchWithContributionTargetBranch
        ));
        // an unpublished default branch can be renamed
        let tip = valid("main", None);
        let state = menu_state_for(&inputs(Some(on_github(RepositoryFacts::derive(
            &snapshot(&tip),
        )))));
        assert!(enabled(&state, MenuId::RenameBranch));
        assert!(!enabled(&state, MenuId::Pull));
        assert!(!enabled(&state, MenuId::CompareOnGithub));
    }

    #[test]
    fn detached_head() {
        let tip = Tip::Detached { sha: "abc".into() };
        let state = menu_state_for(&inputs(Some(on_github(RepositoryFacts::derive(
            &snapshot(&tip),
        )))));
        for id in [
            MenuId::RenameBranch,
            MenuId::DeleteBranch,
            MenuId::MergeBranch,
            MenuId::RebaseBranch,
            MenuId::CreatePullRequest,
            MenuId::PreviewPullRequest,
            MenuId::Push,
            MenuId::Pull,
            MenuId::CompareToBranch,
        ] {
            assert!(!enabled(&state, id), "{}", id.as_str());
        }
        assert!(enabled(&state, MenuId::CreateBranch));
        assert!(enabled(&state, MenuId::Fetch));
        assert!(enabled(&state, MenuId::ViewRepositoryOnGithub));
    }

    #[test]
    fn unborn_and_unknown_tips() {
        let tip = Tip::Unborn {
            name: "main".into(),
        };
        let state = menu_state_for(&inputs(Some(on_github(RepositoryFacts::derive(
            &snapshot(&tip),
        )))));
        for id in [
            MenuId::RenameBranch,
            MenuId::DeleteBranch,
            MenuId::CreateBranch,
            MenuId::Push,
            MenuId::CreatePullRequest,
            MenuId::MergeBranch,
        ] {
            assert!(!enabled(&state, id), "{}", id.as_str());
        }
        assert!(enabled(&state, MenuId::CompareToBranch));

        let state = menu_state_for(&inputs(Some(on_github(RepositoryFacts::derive(
            &snapshot(&Tip::Unknown),
        )))));
        assert!(!enabled(&state, MenuId::CreateBranch));
        assert!(enabled(&state, MenuId::Push));
    }

    #[test]
    fn network_action_in_progress() {
        let tip = valid("feature", Some("refs/remotes/origin/feature"));
        let facts = RepositoryFacts::derive(&RepositorySnapshot {
            push_pull_fetch_in_progress: true,
            ..snapshot(&tip)
        });
        let state = menu_state_for(&inputs(Some(on_github(facts))));
        assert!(!enabled(&state, MenuId::Push));
        assert!(!enabled(&state, MenuId::Pull));
        assert!(!enabled(&state, MenuId::Fetch));
        assert!(enabled(&state, MenuId::CreateBranch));
    }

    #[test]
    fn rebase_in_progress() {
        let tip = valid("feature", None);
        let conflict = ConflictState {
            kind: ConflictKind::Rebase {
                current_tip: "c".into(),
                target_branch: "feature".into(),
                base_branch_tip: "a".into(),
                original_branch_tip: "b".into(),
            },
            manual_resolutions: BTreeMap::new(),
        };
        let status = WorkingDirectoryStatus {
            files: vec![modified_file("a.txt")],
            ..WorkingDirectoryStatus::default()
        };
        let facts = RepositoryFacts::derive(&RepositorySnapshot {
            conflict_state: Some(&conflict),
            working_directory: Some(&status),
            ..snapshot(&tip)
        });
        assert!(facts.rebase_in_progress);
        assert!(facts.has_conflicts);
        let state = menu_state_for(&inputs(Some(on_github(facts))));
        assert!(!enabled(&state, MenuId::CreateBranch));
        assert!(!enabled(&state, MenuId::DiscardAllChanges));
        assert!(!enabled(&state, MenuId::StashAllChanges));

        // the same changes outside a rebase can be discarded and stashed
        let facts = RepositoryFacts::derive(&RepositorySnapshot {
            working_directory: Some(&status),
            has_stash_entry: true,
            ..snapshot(&tip)
        });
        let state = menu_state_for(&inputs(Some(on_github(facts))));
        assert!(enabled(&state, MenuId::DiscardAllChanges));
        assert!(enabled(&state, MenuId::StashAllChanges));
        assert!(enabled(&state, MenuId::ToggleStashedChanges));
    }

    #[test]
    fn welcome_flow() {
        let facts = RepositoryFacts::derive(&snapshot(&valid("feature", None)));
        let state = menu_state_for(&MenuInputs {
            show_welcome_flow: true,
            ..inputs(Some(on_github(facts)))
        });
        for id in WELCOME_SCOPED_IDS {
            assert!(!enabled(&state, id), "{}", id.as_str());
        }
        // the repository is not active behind the welcome flow
        assert!(!enabled(&state, MenuId::ShowChanges));
        assert!(!enabled(&state, MenuId::Push));
    }

    #[test]
    fn zero_repositories() {
        let state = menu_state_for(&MenuInputs {
            repository_count: 0,
            ..inputs(None)
        });
        assert!(!enabled(&state, MenuId::ShowRepositoryList));
        assert!(enabled(&state, MenuId::NewRepository));
    }

    #[test]
    fn hidden_window_deactivates_the_repository() {
        let facts = RepositoryFacts::derive(&snapshot(&valid("feature", None)));
        let state = menu_state_for(&MenuInputs {
            window_open: false,
            ..inputs(Some(on_github(facts)))
        });
        assert!(!enabled(&state, MenuId::ShowChanges));
        assert!(enabled(&state, MenuId::NewRepository));
    }

    #[test]
    fn resizable_width_items() {
        let state = menu_state_for(&inputs(None));
        assert!(!state.contains_key(&MenuId::IncreaseActiveResizableWidth));
        let state = menu_state_for(&MenuInputs {
            resizable_pane_active: Some(false),
            ..inputs(None)
        });
        assert!(!enabled(&state, MenuId::IncreaseActiveResizableWidth));
        assert!(!enabled(&state, MenuId::DecreaseActiveResizableWidth));
    }

    #[test]
    fn archived_parent_disables_issues() {
        let mut repository = Repository::new(1, "/tmp/r");
        let mut gh = github("fork");
        gh.parent = Some(Box::new(GitHubRepository {
            archived: true,
            ..github("parent")
        }));
        repository.github = Some(gh);
        assert!(!repo_issues_enabled(&repository));
        repository.github = Some(github("plain"));
        assert!(repo_issues_enabled(&repository));
        repository.github = None;
        assert!(!repo_issues_enabled(&repository));
    }

    #[test]
    fn contribution_target_of_a_fork_is_the_upstream_default() {
        let branches = vec![
            branch("main", BranchKind::Local, Some("refs/remotes/origin/main")),
            branch("origin/main", BranchKind::Remote, None),
            branch("upstream/trunk", BranchKind::Remote, None),
        ];
        let mut repository = Repository::new(1, "/tmp/r");
        // no GitHub repository: the default branch
        assert_eq!(
            contribution_target_default_branch(&repository, &branches, Some("main"), "main"),
            Some("main")
        );

        let mut gh = github("fork");
        gh.parent = Some(Box::new(GitHubRepository {
            default_branch: Some("trunk".into()),
            ..github("parent")
        }));
        repository.github = Some(gh);
        assert_eq!(
            contribution_target_default_branch(&repository, &branches, Some("main"), "main"),
            Some("upstream/trunk")
        );

        // contributing to the fork itself: the fork's default branch
        repository.fork_contribution_target = Some(ForkContributionTarget::Own);
        assert_eq!(
            contribution_target_default_branch(&repository, &branches, Some("main"), "main"),
            Some("main")
        );

        // on `main` of a fork contributing to its parent, updating from
        // `upstream/trunk` is offered (GHD compares branch names)
        let tip = valid("main", Some("refs/remotes/origin/main"));
        let facts = RepositoryFacts::derive(&RepositorySnapshot {
            contribution_target: Some("upstream/trunk"),
            ..snapshot(&tip)
        });
        assert!(facts.has_contribution_target_default_branch);
        assert!(!facts.on_contribution_target_default_branch);
    }
}
