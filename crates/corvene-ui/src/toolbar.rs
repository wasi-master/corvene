//! `#desktop-app-toolbar`: Repository / Branch / Push-Pull buttons.
//! Geometry from `styles/ui/toolbar/{_toolbar,_button,_dropdown}.scss`.
//!
//! The worktree and branch buttons are resizable (`enableResizingToolbarButtons`,
//! `ui/resizable/resizable.tsx`, `_resizable.scss`): a 6 px handle straddles
//! their right edge; dragging sets the width within
//! `corvene_core::toolbar_widths`, double-clicking resets it to 230 px. The
//! width is saved when the drag ends (GHD writes it on every move).
//!
//! The branch button also shows a running merge ("Merging <branch>",
//! `412-merge-progress-in-branch-button`); GHD shows only checkouts.
//! Deviation (`.docs/deviations.md` › History, flag `257`): the Pull
//! button's tooltip lists the incoming commits' summaries (GHD
//! `push-pull-button.tsx` has none).
//! While a fetch, pull or push runs, a Stop button takes the ▾'s place
//! (`295-cancel-network-operations`; GHD only disables the button).
//! A branch whose upstream was deleted on the remote shows Publish branch
//! with an alert (`1209-current-branch-deleted-hint`; GHD shows Fetch).
//! A branch that `push.default=current` pushes to a same-named remote branch
//! shows Push / Pull / Fetch instead of Publish branch
//! (`1103-implicit-upstream-push-default`).
//! The Push button's tooltip can say roughly how much the push sends
//! (`1101-push-size-tooltip`; GHD has no tooltip there).

use std::cell::Cell;
use std::rc::Rc;

use corvene_core::toolbar_widths::{ConstrainedWidth, ToolbarWidths};
use corvene_core::{AheadBehind, AppState, Dispatcher, Foldout, Tip};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::keyboard_nav::ControlFocus;
use crate::widgets::IconButtonA11y;

use crate::context_menu::mac_or;
use crate::icons::{Octicon, octicon, spin};
use crate::relative_time::relative;
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;

/// One toolbar button, derived from `AppState` by [`toolbar_models`].
pub struct ToolbarButtonModel {
    pub id: &'static str,
    pub icon: Octicon,
    /// Small secondary line ("Current Repository").
    pub description: SharedString,
    /// Bold main line (repository / branch name).
    pub title: SharedString,
    /// Corvene (`410-alias-italic-in-toolbar`): an aliased repository's
    /// name is italic, as in the repository list.
    pub title_italic: bool,
    pub width: Option<Pixels>,
    /// Opens this foldout when clicked; `None` = plain action button.
    pub foldout: Option<Foldout>,
    pub open: bool,
    pub disabled: bool,
    pub badge: Option<AheadBehind>,
    /// Push/pull button: the main click runs the network action instead of a foldout.
    pub push_pull: bool,
    /// Show the 39 px ▾ button that opens `Foldout::PushPull`.
    pub arrow: bool,
    /// `progressValue`: fill the button background up to this fraction.
    pub progress: Option<f32>,
    /// `iconClassName = 'spin'` (checkout / network action in progress).
    pub spin: bool,
    /// `PullRequestBadge` on the branch button (`#N` + CI status).
    pub pr_badge: Option<PrBadge>,
    /// Resizable worktree / branch button and its width constraints.
    pub resize: Option<(ResizeTarget, ConstrainedWidth)>,
    /// GHD `ToolbarButton` `tooltip`, shown south of the button.
    pub tooltip: Option<SharedString>,
    /// The tooltip keeps its maximum width while its text changes (flag
    /// `411-steady-progress-tooltip`).
    pub tooltip_fixed_width: bool,
    /// Corvene (`426-drag-repository-out`): the folder (and name) the button
    /// drags out of the window.
    pub drag: Option<(std::path::PathBuf, SharedString)>,
    /// Corvene (`295-cancel-network-operations`): the push/pull button shows
    /// a Stop button for the running operation.
    pub cancel: bool,
    /// Corvene (`1101-push-size-tooltip`): hovering works out the push size
    /// its tooltip names.
    pub load_push_size: bool,
}

/// Which toolbar button a resize handle belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResizeTarget {
    Worktree,
    Branch,
}

#[derive(Clone, Copy)]
struct ResizeDrag {
    target: ResizeTarget,
    start_x: Pixels,
    start_width: f32,
    constraint: ConstrainedWidth,
}

/// Drag state shared by the toolbar's resize handles (owned by the workspace).
#[derive(Default)]
pub struct ToolbarResize {
    drag: Cell<Option<ResizeDrag>>,
    /// The width being dragged to; saved to the settings on mouse up.
    live: Cell<Option<(ResizeTarget, f32)>>,
}

impl ToolbarResize {
    /// The width a button shows while it is being dragged.
    pub fn live_width(&self, target: ResizeTarget) -> Option<f32> {
        self.live
            .get()
            .filter(|(t, _)| *t == target)
            .map(|(_, w)| w)
    }
}

/// The toolbar button showing `:focus-visible`: Escape closing a foldout
/// gives focus back to the button that opened it (GHD's `ToolbarDropdown`
/// keeps focus on its `<button>`, and a key moved it there), until a mouse
/// press anywhere moves focus on.
#[derive(Default)]
pub struct ToolbarFocusVisible(pub Option<Foldout>);

impl Global for ToolbarFocusVisible {}

/// The open foldout when a click on its toolbar button opened it. GHD's
/// dropdown focus trap gives focus back to what had it when the foldout
/// opened: the clicked button, or for a shortcut or menu item whatever
/// had focus before (no ring on the button).
#[derive(Default)]
struct OpenedByButton(Option<Foldout>);

impl Global for OpenedByButton {}

/// The open foldout, if its toolbar button opened it.
pub fn opened_by_button(cx: &App) -> Option<Foldout> {
    cx.try_global::<OpenedByButton>().and_then(|o| o.0)
}

/// The foldout closed (or another one replaced it).
pub fn forget_opened_by_button(cx: &mut App) {
    if opened_by_button(cx).is_some() {
        cx.set_global(OpenedByButton(None));
    }
}

/// Mark `foldout`'s button (or none) as keyboard-focused.
pub fn set_focus_visible(foldout: Option<Foldout>, cx: &mut App) {
    cx.set_global(ToolbarFocusVisible(foldout));
    cx.refresh_windows();
}

fn focus_visible(cx: &App) -> Option<Foldout> {
    cx.try_global::<ToolbarFocusVisible>().and_then(|f| f.0)
}

/// The worktree and branch button widths for this window
/// (`updateResizableConstraints`), with a drag in progress applied.
pub fn toolbar_widths(
    state: &AppState,
    window_width: Pixels,
    sidebar_width: Pixels,
    resize: &ToolbarResize,
) -> (ToolbarWidths, Pixels, Pixels) {
    // the constraint math is in CSS pixels (GHD); widths persist unzoomed
    let widths = corvene_core::toolbar_widths::toolbar_widths(
        unzoom(window_width),
        unzoom(sidebar_width),
        worktree_button_visible(state),
        state.settings.worktree_dropdown_width,
        state.settings.branch_dropdown_width,
    );
    let worktree = resize
        .live_width(ResizeTarget::Worktree)
        .unwrap_or_else(|| widths.worktree.clamped());
    let branch = resize
        .live_width(ResizeTarget::Branch)
        .unwrap_or_else(|| widths.branch.clamped());
    (widths, zpx(worktree), zpx(branch))
}

/// `renderPullRequestInfo`
pub struct PrBadge {
    /// `None`: the current branch's own checks (`334-branch-ci-status`).
    /// `#12`, or `!12` on GitLab (flag `342-gitlab`).
    pub number: Option<String>,
    /// `2 of 3` (`356-stacked-pull-requests`).
    pub stack_position: Option<String>,
    /// On GitHub: the badge has GitHub's reviewer menu.
    pub github: bool,
    pub status: Option<(
        corvene_core::CheckStatus,
        Option<corvene_core::CheckConclusion>,
    )>,
    /// Window-space rectangle of the badge, for the check-run popover.
    pub bounds: Rc<Cell<Bounds<Pixels>>>,
}

/// GHD `renderWorktreeToolbarButton`: only with linked worktrees, or while
/// the foldout is open (so it can be reached from the menu).
pub fn worktree_button_visible(state: &AppState) -> bool {
    let has_linked = state
        .selected_state()
        .is_some_and(|rs| crate::worktree_list::listed_worktrees(state, &rs.worktrees).len() > 1);
    state.selected.is_some() && (has_linked || state.foldout == Some(Foldout::Worktree))
}

/// GHD `Toolbar` render: repository, worktree, branch, push/pull - from the app state.
pub fn toolbar_models(
    state: &AppState,
    sidebar_width: Pixels,
    (widths, worktree_width, branch_width): (ToolbarWidths, Pixels, Pixels),
    pr_badge_bounds: &Rc<Cell<Bounds<Pixels>>>,
) -> Vec<ToolbarButtonModel> {
    let repo = state.selected_repository();
    let repo_state = state.selected_state();
    let info = repo_state.and_then(|s| s.info.as_ref());

    // `WorktreeDropdown`: title = current worktree folder, else the repository name
    let worktree = worktree_button_visible(state).then(|| {
        let title: SharedString = repo
            .and_then(|r| crate::worktree_list::current_worktree(state, r.id))
            .map(|w| w.display_name())
            .or_else(|| repo.map(|r| r.name()))
            .unwrap_or_default()
            .into();
        let worktree_tooltip = (state.foldout != Some(Foldout::Worktree))
            .then(|| format!("Current worktree is {title}").into());
        ToolbarButtonModel {
            id: "toolbar-worktree",
            title_italic: false,
            icon: Octicon::FileDirectory,
            description: mac_or("Current Worktree", "Current worktree").into(),
            title,
            width: Some(worktree_width),
            foldout: Some(Foldout::Worktree),
            open: state.foldout == Some(Foldout::Worktree),
            disabled: false,
            badge: None,
            push_pull: false,
            arrow: false,
            progress: None,
            spin: false,
            pr_badge: None,
            resize: Some((ResizeTarget::Worktree, widths.worktree)),
            tooltip: worktree_tooltip,
            tooltip_fixed_width: false,
            drag: None,
            cancel: false,
            load_push_size: false,
        }
    });

    let repository = ToolbarButtonModel {
        id: "toolbar-repository",
        title_italic: repo.is_some_and(|r| r.alias.is_some())
            && state
                .flags
                .bool(corvene_core::flags::ids::ALIAS_ITALIC_IN_TOOLBAR),
        icon: repo.map_or(Octicon::Repo, |r| {
            crate::icons::icon_for_repository(crate::icons::RepositoryOrCloning::Repository(r))
        }),
        // Corvene (`409-owner-in-repository-button`): a GitHub repository's
        // owner in place of "Current Repository"
        description: match repo.and_then(|r| r.github.as_ref()) {
            Some(gh)
                if state
                    .flags
                    .bool(corvene_core::flags::ids::OWNER_IN_REPOSITORY_BUTTON) =>
            {
                gh.owner.clone().into()
            }
            _ => mac_or("Current Repository", "Current repository").into(),
        },
        title: repo
            .map(|r| r.name().into())
            .unwrap_or_else(|| mac_or("Select a Repository", "Select a repository").into()),
        width: Some(sidebar_width),
        foldout: Some(Foldout::Repository),
        open: state.foldout == Some(Foldout::Repository),
        disabled: false,
        badge: None,
        push_pull: false,
        arrow: false,
        progress: None,
        spin: false,
        pr_badge: None,
        resize: None,
        // `repository && !isOpen ? repository.path : undefined`; Corvene
        // (`273-fork-parent-in-tooltip`) adds "Fork of owner/name"
        tooltip: repo
            .filter(|_| state.foldout != Some(Foldout::Repository))
            .map(|r| {
                let mut text = r.path.to_string_lossy().into_owned();
                if let Some(parent) = fork_parent(r, state) {
                    text.push_str(&format!("\nFork of {parent}"));
                }
                text.into()
            }),
        tooltip_fixed_width: false,
        // Corvene (`426-drag-repository-out`)
        drag: repo
            .filter(|r| {
                !r.missing
                    && cfg!(target_os = "macos")
                    && state
                        .flags
                        .bool(corvene_core::flags::ids::DRAG_REPOSITORY_OUT)
            })
            .map(|r| (r.path.clone(), r.name().into())),
        cancel: false,
        load_push_size: false,
    };

    // `currentPullRequest`: the icon becomes the PR icon and the badge shows
    let current_pr = repo.and_then(|r| state.current_pull_request(r.id));
    let pr_badge = current_pr
        .map(|pr| PrBadge {
            number: Some(pr.number_label()),
            stack_position: repo
                .and_then(|r| state.stack_position(r.id, pr))
                .map(|p| p.label()),
            github: pr.host_kind() == corvene_core::HostKind::GitHub,
            status: state.commit_status_summary(pr),
            bounds: pr_badge_bounds.clone(),
        })
        .or_else(|| {
            // Corvene (`334-branch-ci-status`): no pull request, the checks
            // of the branch's pushed tip
            let status = repo.and_then(|r| state.branch_ci_summary(r.id))?;
            Some(PrBadge {
                number: None,
                stack_position: None,
                github: false,
                status: Some(status),
                bounds: pr_badge_bounds.clone(),
            })
        });
    // Corvene (`275-detached-head-friendly`): the tag HEAD sits on, from the
    // loaded history
    let detached_friendly = state
        .flags
        .bool(corvene_core::flags::ids::DETACHED_HEAD_FRIENDLY);
    let head_tag = |sha: &str| -> Option<String> {
        if !detached_friendly {
            return None;
        }
        repo_state?
            .commits
            .iter()
            .find(|c| c.sha == sha)
            .and_then(|c| c.tags.first().cloned())
    };
    let (branch_icon, branch_desc, branch_title): (Octicon, &str, SharedString) =
        match info.map(|i| &i.tip) {
            Some(Tip::Valid { branch }) => (
                if current_pr.is_some() {
                    Octicon::GitPullRequest
                } else {
                    Octicon::GitBranch
                },
                mac_or("Current Branch", "Current branch"),
                branch.name.clone().into(),
            ),
            Some(Tip::Unborn { name }) => (
                Octicon::GitBranch,
                mac_or("Current Branch", "Current branch"),
                name.clone().into(),
            ),
            Some(Tip::Detached { sha }) => (
                Octicon::GitCommit,
                "Detached HEAD",
                format!(
                    "On {}",
                    head_tag(sha).unwrap_or_else(|| sha.chars().take(7).collect())
                )
                .into(),
            ),
            _ => (
                Octicon::GitBranch,
                mac_or("Current Branch", "Current branch"),
                "".into(),
            ),
        };
    // `checkoutProgress`: title = target branch, description = "Switching to Branch"
    let switching_to = repo_state.and_then(|s| s.checkout_target.clone());
    let checkout_progress = repo_state
        .and_then(|s| s.checkout_progress.clone())
        .filter(|_| switching_to.is_some());
    let switching_to_tooltip = switching_to.clone();
    // `412-merge-progress-in-branch-button`: a running merge (Merge into…,
    // Update from Default Branch) spins the button, "Merging <branch>"
    let merging_from = repo_state
        .and_then(|s| s.mco.as_ref())
        .filter(|m| {
            m.step == corvene_core::McoStep::ShowProgress
                && state
                    .flags
                    .bool(corvene_core::flags::ids::MERGE_PROGRESS_IN_BRANCH_BUTTON)
        })
        .and_then(|m| match &m.detail {
            corvene_core::McoDetail::Merge { source_branch, .. } => {
                Some(source_branch.clone().unwrap_or_default())
            }
            _ => None,
        })
        .filter(|_| switching_to.is_none());
    let switching = switching_to.is_some() || merging_from.is_some();
    let (branch_icon, branch_desc, branch_title): (Octicon, SharedString, SharedString) =
        match (switching_to, &merging_from) {
            // `checkoutProgress.description`, with the percent once there is one
            (Some(target), _) => (
                Octicon::SyncClockwise,
                match checkout_progress.as_ref() {
                    Some(p) if p.value > 0. => {
                        format!("{} ({}%)", p.description, (p.value * 100.).round()).into()
                    }
                    Some(p) => p.description.clone().into(),
                    None => mac_or("Switching to Branch", "Switching to branch").into(),
                },
                SharedString::from(target),
            ),
            (None, Some(source)) => (
                Octicon::SyncClockwise,
                format!("Merging {source}").into(),
                branch_title,
            ),
            (None, None) => (branch_icon, branch_desc.into(), branch_title),
        };
    // `BranchDropdown` tooltip (none while open)
    let branch_tooltip: Option<SharedString> = match (&switching_to_tooltip, info.map(|i| &i.tip)) {
        _ if state.foldout == Some(Foldout::Branch) => None,
        (Some(target), _) => Some(format!("Checking out {target}").into()),
        (None, _) if merging_from.is_some() => Some(
            format!(
                "Merging {} into {branch_title}",
                merging_from.as_deref().unwrap_or_default()
            )
            .into(),
        ),
        (None, Some(Tip::Valid { branch })) => Some(branch.name.clone().into()),
        (None, Some(Tip::Unborn { name })) => Some(format!("Current branch is {name}").into()),
        (None, Some(Tip::Detached { .. })) if detached_friendly => Some(
            format!(
                "Currently on a detached HEAD at {}\nNot on any branch. Create a branch to keep \
                 new commits.",
                branch_title.trim_start_matches("On ")
            )
            .into(),
        ),
        (None, Some(Tip::Detached { .. })) => Some("Currently on a detached HEAD".into()),
        _ => None,
    };
    let branch = ToolbarButtonModel {
        id: "toolbar-branch",
        title_italic: false,
        icon: branch_icon,
        description: branch_desc,
        title: branch_title.clone(),
        width: Some(branch_width),
        foldout: Some(Foldout::Branch),
        open: state.foldout == Some(Foldout::Branch),
        disabled: repo.is_none(),
        badge: None,
        push_pull: false,
        arrow: false,
        // `progressValue={checkoutProgress.value}`
        progress: checkout_progress.as_ref().map(|p| p.value as f32),
        spin: switching,
        pr_badge,
        resize: Some((ResizeTarget::Branch, widths.branch)),
        tooltip: branch_tooltip,
        tooltip_fixed_width: false,
        drag: None,
        cancel: false,
        load_push_size: false,
    };

    // Push/Pull (`PushPullButton.renderButton`)
    let has_remote = info.map(|i| !i.remotes.is_empty()).unwrap_or(false);
    let remote_name = repo
        .and_then(|r| Dispatcher::current_remote_in(state, r.id))
        .map(|r| r.name)
        .unwrap_or_else(|| "origin".to_string());
    // GHD `app.tsx`: a branch tracking a differently named branch names its
    // upstream ("Push origin/main"); `1222-push-target-guard` says it in the
    // description and tooltip instead
    let mismatch = repo.and_then(|r| Dispatcher::upstream_mismatch_in(state, r.id));
    let guard = Dispatcher::push_target_guard(state);
    let remote_name = match &mismatch {
        Some(m) if !guard => m.upstream.clone(),
        _ => remote_name,
    };
    let mismatch = mismatch.filter(|_| guard);
    let mismatch_tooltip =
        |m: &corvene_core::push_target::UpstreamMismatch| -> SharedString { m.describe().into() };
    // Corvene (`1103-implicit-upstream-push-default`): the same-named branch
    // `push.default=current` pushes to stands in for a missing upstream
    let implicit = repo.and_then(|r| Dispatcher::implicit_upstream_in(state, r.id));
    let upstream = info
        .and_then(|i| i.current_branch())
        .and_then(|b| b.upstream.clone())
        .or_else(|| implicit.as_ref().map(|(name, _)| name.clone()));
    let ab = repo_state
        .and_then(|s| s.ahead_behind)
        .or_else(|| implicit.map(|(_, ab)| ab));
    // Corvene (`1109-remote-manager`): a push remote other than the
    // upstream's takes the pushes; Pull and Fetch stay with the upstream
    let push_target = repo.and_then(|r| Dispatcher::push_target_in(state, r.id));
    let ab = repo.and_then(|r| Dispatcher::with_push_target(state, r.id, upstream.is_some(), ab));
    let push_remote_name = push_target
        .map(|t| t.remote.clone())
        .unwrap_or_else(|| remote_name.clone());
    let last_fetched: SharedString = match repo_state.and_then(|s| s.last_fetched) {
        Some(at) => format!("Last fetched {}", relative(at)).into(),
        None => "Never fetched".into(),
    };
    let progress = repo_state.and_then(|s| s.push_pull_progress.clone());
    let is_github = repo.is_some_and(|r| r.github.is_some());
    let force_push = repo
        .map(|r| Dispatcher::force_push_state_in(state, r.id))
        .unwrap_or(corvene_core::ForcePushState::NotAvailable);
    let pull_with_rebase = repo_state.is_some_and(|s| s.pull_with_rebase);
    let rebase_in_progress = repo_state
        .and_then(|s| s.status.as_deref())
        .is_some_and(|st| st.rebase_in_progress);
    let base = ToolbarButtonModel {
        id: "toolbar-push-pull",
        title_italic: false,
        icon: Octicon::SyncClockwise,
        description: "".into(),
        title: "".into(),
        width: Some(TOOLBAR_BUTTON_WIDTH()),
        foldout: None,
        open: state.foldout == Some(Foldout::PushPull),
        disabled: false,
        badge: None,
        push_pull: true,
        arrow: false,
        progress: None,
        spin: false,
        pr_badge: None,
        resize: None,
        tooltip: None,
        tooltip_fixed_width: false,
        drag: None,
        cancel: false,
        load_push_size: false,
    };
    let revert_progress = repo_state.and_then(|s| s.revert_progress.clone());
    let push_pull = if repo.is_none() {
        ToolbarButtonModel {
            disabled: true,
            ..base
        }
    } else if let Some(p) = revert_progress {
        // GHD `RevertProgress`, drawn instead of the push / pull button
        ToolbarButtonModel {
            icon: Octicon::SyncClockwise,
            title: "Reverting…".into(),
            description: if p.title.is_empty() {
                "Hang on…".into()
            } else {
                p.title.into()
            },
            disabled: true,
            progress: Some(p.value as f32),
            spin: true,
            ..base
        }
    } else if let Some(p) = progress {
        ToolbarButtonModel {
            icon: Octicon::SyncClockwise,
            description: p
                .description
                .clone()
                .unwrap_or_else(|| "Hang on…".to_string())
                .into(),
            title: p.title.clone().into(),
            // `tooltip={progress.description}`
            tooltip: p.description.clone().map(Into::into),
            tooltip_fixed_width: state
                .flags
                .bool(corvene_core::flags::ids::STEADY_PROGRESS_TOOLTIP),
            disabled: true,
            progress: Some(p.value),
            spin: true,
            // `295-cancel-network-operations`
            cancel: repo.is_some_and(|r| Dispatcher::network_cancellable(state, r.id)),
            ..base
        }
    } else if info.is_none()
        && state
            .flags
            .bool(corvene_core::flags::ids::NO_PUBLISH_BEFORE_LOAD)
    {
        // Corvene (`274-no-publish-before-load`): the remotes are unknown
        // until the repository is read, so no "Publish repository" yet
        ToolbarButtonModel {
            disabled: true,
            ..base
        }
    } else if !has_remote {
        ToolbarButtonModel {
            icon: Octicon::Upload,
            description: "Publish this repository to GitHub".into(),
            title: "Publish repository".into(),
            ..base
        }
    } else {
        match info.map(|i| &i.tip) {
            Some(Tip::Unborn { .. }) => ToolbarButtonModel {
                icon: Octicon::SyncClockwise,
                description: last_fetched,
                title: format!("Fetch {remote_name}").into(),
                ..base
            },
            Some(Tip::Detached { .. }) | Some(Tip::Unknown) | None => ToolbarButtonModel {
                icon: Octicon::Upload,
                description: if rebase_in_progress {
                    "Rebase in progress".into()
                } else {
                    "Cannot publish detached HEAD".into()
                },
                title: "Publish branch".into(),
                disabled: true,
                ..base
            },
            // Corvene (`1209-current-branch-deleted-hint`): the upstream was
            // deleted on the remote (GHD shows Fetch, while a click pushes)
            Some(Tip::Valid { branch })
                if let Some(gone) =
                    repo.and_then(|r| Dispatcher::current_upstream_gone(state, r.id)) =>
            {
                ToolbarButtonModel {
                    icon: Octicon::Alert,
                    description: format!("Deleted on {gone}").into(),
                    title: "Publish branch".into(),
                    tooltip: Some(
                        format!(
                            "{} was deleted on {gone}. Publishing pushes it there again.",
                            branch.name
                        )
                        .into(),
                    ),
                    arrow: true,
                    ..base
                }
            }
            Some(Tip::Valid { .. }) if upstream.is_none() => ToolbarButtonModel {
                icon: Octicon::Upload,
                description: if let Some(target) = push_target {
                    format!("Publish this branch to {}", target.remote).into()
                } else if is_github {
                    "Publish this branch to GitHub".into()
                } else {
                    "Publish this branch to the remote".into()
                },
                title: "Publish branch".into(),
                arrow: true,
                ..base
            },
            Some(Tip::Valid { .. }) => {
                let ab = ab.unwrap_or_default();
                // `1222-push-target-guard`: where a push of this branch goes
                let target = mismatch.as_ref().filter(|_| push_target.is_none());
                let push_description: SharedString = match target {
                    Some(m) => format!("To {}", m.upstream).into(),
                    None => last_fetched.clone(),
                };
                if ab.ahead == 0 && ab.behind == 0 {
                    ToolbarButtonModel {
                        icon: Octicon::SyncClockwise,
                        description: last_fetched,
                        title: format!("Fetch {remote_name}").into(),
                        tooltip: target.map(mismatch_tooltip),
                        ..base
                    }
                } else if force_push == corvene_core::ForcePushState::Recommended
                    && push_target.is_none()
                {
                    ToolbarButtonModel {
                        icon: Octicon::ArrowUp,
                        tooltip: target.map(mismatch_tooltip),
                        description: push_description,
                        title: format!("Force push {remote_name}").into(),
                        badge: Some(ab),
                        arrow: true,
                        ..base
                    }
                } else if ab.behind > 0 {
                    ToolbarButtonModel {
                        tooltip: incoming_tooltip(ab.behind, repo_state, state),
                        icon: Octicon::ArrowDown,
                        description: last_fetched,
                        title: if pull_with_rebase {
                            format!("Pull {remote_name} with rebase").into()
                        } else {
                            format!("Pull {remote_name}").into()
                        },
                        badge: Some(ab),
                        arrow: true,
                        ..base
                    }
                } else {
                    // Corvene (`1101-push-size-tooltip`)
                    let push_size = repo.and_then(|r| Dispatcher::push_size_for(state, r.id));
                    ToolbarButtonModel {
                        icon: Octicon::ArrowUp,
                        description: push_description,
                        title: format!("Push {push_remote_name}").into(),
                        badge: Some(ab),
                        arrow: true,
                        load_push_size: push_size.is_some() && target.is_none(),
                        tooltip: match target {
                            Some(m) => Some(mismatch_tooltip(m)),
                            None => push_size.map(push_size_tooltip),
                        },
                        ..base
                    }
                }
            }
        }
    };

    // `295-cancel-network-operations`: right after a Stop the button says so
    let mut push_pull = push_pull;
    if !push_pull.disabled
        && repo_state
            .and_then(|s| s.network_cancelled_at)
            .is_some_and(|at| at.elapsed() < corvene_core::remote::CANCELLED_NOTE)
    {
        push_pull.description = "Cancelled".into();
    }

    let mut buttons = vec![repository];
    // the worktree, branch and push/pull buttons need a
    // `SelectionType.Repository`: a missing repository has none of them
    if repo.is_some_and(|r| r.missing) {
        return buttons;
    }
    buttons.extend(worktree);
    buttons.push(branch);
    buttons.push(push_pull);
    buttons
}

/// `1101-push-size-tooltip`: "≈ 1.2 MiB to push (340 MiB in Git LFS)".
fn push_size_tooltip(size: Option<corvene_git::PushSize>) -> SharedString {
    let Some(size) = size else {
        return "Working out how much to push…".into();
    };
    let bytes = |n: u64| crate::format::format_bytes(i64::try_from(n).unwrap_or(i64::MAX), 1);
    let total = format!(
        "≈ {} to push",
        bytes(size.bytes.saturating_add(size.lfs_bytes))
    );
    if size.lfs_files == 0 {
        total.into()
    } else {
        format!("{total} ({} in Git LFS)", bytes(size.lfs_bytes)).into()
    }
}

/// Flag `257`: the Pull button's tooltip lists the incoming commits.
/// Corvene (`273-fork-parent-in-tooltip`): the parent's `owner/name` of a
/// forked GitHub repository, for the repository tooltips.
pub(crate) fn fork_parent(repo: &corvene_core::Repository, state: &AppState) -> Option<String> {
    let gh = repo.github.as_ref().filter(|gh| gh.fork)?;
    let parent = gh.parent.as_ref()?;
    state
        .flags
        .bool(corvene_core::flags::ids::FORK_PARENT_IN_TOOLTIP)
        .then(|| parent.full_name())
}

/// An ahead / behind count: with the thousands separator under
/// `272-grouped-ahead-behind-counts`, else GHD's plain digits.
pub(crate) fn ahead_behind_count(n: u32, state: &AppState) -> String {
    if state
        .flags
        .bool(corvene_core::flags::ids::GROUPED_AHEAD_BEHIND_COUNTS)
    {
        crate::format::format_count(u64::from(n))
    } else {
        n.to_string()
    }
}

fn incoming_tooltip(
    behind: u32,
    repo_state: Option<&corvene_core::RepositoryState>,
    state: &AppState,
) -> Option<SharedString> {
    if !state
        .flags
        .bool(corvene_core::flags::ids::PULL_TOOLTIP_LISTS_COMMITS)
    {
        return None;
    }
    let summaries = &repo_state?.incoming_commits;
    if summaries.is_empty() {
        return None;
    }
    let mut lines = vec![if behind == 1 {
        "1 commit to pull:".to_string()
    } else {
        format!("{} commits to pull:", ahead_behind_count(behind, state))
    }];
    lines.extend(summaries.iter().map(|s| format!("• {s}")));
    let more = behind as usize - summaries.len().min(behind as usize);
    if more > 0 {
        lines.push(format!("…and {more} more"));
    }
    Some(lines.join("\n").into())
}

pub fn toolbar_button(
    model: ToolbarButtonModel,
    resize_state: &Rc<ToolbarResize>,
    cx: &App,
) -> AnyElement {
    let t = cx.ghd();
    let resize = model.resize.zip(model.width);
    // the push/pull button with its ▾ (the compact layout narrows it)
    let split_width = model.width.unwrap_or_else(TOOLBAR_BUTTON_WIDTH);
    let hover_bg = t.toolbar_button_hover_background;
    let hover_text = t.toolbar_button_hover_text;
    let (bg, text, secondary) = if model.open {
        (
            t.toolbar_button_active_background,
            t.toolbar_button_active_text,
            t.text_secondary,
        )
    } else {
        (
            t.toolbar_background,
            t.toolbar_text,
            t.toolbar_text_secondary,
        )
    };
    let foldout = model.foldout;
    let disabled = model.disabled;
    let push_pull = model.push_pull;
    let arrow = model.arrow;
    let progress = model.progress;
    let arrow_open = model.open && push_pull;
    let (arrow_bg, arrow_text) = if arrow_open {
        (
            t.toolbar_button_active_background,
            t.toolbar_button_active_text,
        )
    } else {
        (t.toolbar_background, t.toolbar_text)
    };
    // `.toolbar-button > button:focus-visible`: the focus background
    // (`--toolbar-button-focus-background-color`, gray-800 like the hover
    // one) and Chromium's `outline: auto` ring 4 px inside
    let ring = !model.open && foldout.is_some() && focus_visible(cx) == foldout;
    let bg = if ring { hover_bg } else { bg };
    let bg = if push_pull { t.toolbar_background } else { bg };
    let text = if push_pull { t.toolbar_text } else { text };
    let secondary = if push_pull {
        t.toolbar_text_secondary
    } else {
        secondary
    };
    let button = div()
        .id(model.id)
        .h(TOOLBAR_BUTTON_HEIGHT())
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .p(SPACING())
        .border_r_1()
        .border_color(t.toolbar_button_border)
        .bg(bg)
        .text_color(text)
        .overflow_hidden()
        .when(disabled, |d| d.opacity(0.6))
        .relative()
        .when(!disabled && (!model.open || push_pull), move |d| {
            d.cursor_pointer()
                .hover(move |s| s.bg(hover_bg).text_color(hover_text))
        })
        .when(!disabled, move |d| {
            // Tab reaches it (`keyboard_nav`): GHD's focus background and
            // a ring inside
            let ring = crate::keyboard_nav::inset_ring_shadows(cx);
            d.control_focus_styled(cx, move |s| s.bg(hover_bg).shadow(ring))
                .on_click(move |_, _, cx| {
                    if push_pull {
                        if let Some(id) = corvene_core::AppState::global(cx).read(cx).selected {
                            Dispatcher::close_foldout(cx);
                            Dispatcher::push_pull_action(id, cx);
                        }
                    } else if let Some(foldout) = foldout {
                        Dispatcher::toggle_foldout(foldout, cx);
                        let open =
                            corvene_core::AppState::global(cx).read(cx).foldout == Some(foldout);
                        cx.set_global(OpenedByButton(open.then_some(foldout)));
                    }
                })
        })
        .when_some(progress, |d, value| {
            // `.progress`: fills the button from the left while an operation runs
            d.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(gpui_kit::relative(value.clamp(0., 1.)))
                    .bg(t.toolbar_button_progress),
            )
        })
        .when(foldout == Some(Foldout::Worktree), |d| {
            // `WorktreeDropdown.onContextMenu`
            d.on_mouse_down(MouseButton::Right, |ev, window, cx| {
                crate::worktree_list::toolbar_button_menu(ev.position, window, cx)
            })
        })
        .when(foldout == Some(Foldout::Branch), |d| {
            // GHD `onDragEnter` on the branch dropdown: dragging commits over
            // it opens the list so they can be dropped on a branch.
            d.on_drag_move::<crate::history::CommitDrag>(move |ev, _, cx| {
                if ev.bounds.contains(&ev.event.position)
                    && corvene_core::AppState::global(cx).read(cx).foldout != Some(Foldout::Branch)
                {
                    Dispatcher::toggle_foldout(Foldout::Branch, cx);
                }
            })
        })
        .when(ring, |d| d.child(focus_visible_ring(cx)))
        // Corvene (`1101-push-size-tooltip`)
        .when(model.load_push_size, |d| {
            d.on_hover(|hovered, _, cx| {
                if *hovered && let Some(id) = corvene_core::AppState::global(cx).read(cx).selected {
                    Dispatcher::load_push_size(id, cx);
                }
            })
        })
        // Corvene (`426-drag-repository-out`)
        .when_some(model.drag, |d, (path, name)| {
            crate::repository_drag::draggable(d, path, name)
        })
        .when_some(model.width, |d, w| d.w(w))
        .when(model.width.is_none(), |d| d.flex_1().min_w_0())
        .child(if model.spin {
            div()
                .flex_none()
                .mr(SPACING())
                .child(spin(octicon(model.icon, text), "toolbar-button-spin"))
                .into_any_element()
        } else {
            octicon(model.icon, text).mr(SPACING()).into_any_element()
        })
        .child({
            // `.description` / `.title` keep `line-height: normal`: 13 px
            // and 14 px boxes for SF at 11 / 12 px (Segoe UI, measured in
            // GitHub Desktop at 150 %: 22 and 24 device pixels)
            let description = div()
                .text_size(FONT_SIZE_SM())
                .line_height(zpx(if cfg!(windows) { 44. / 3. } else { 13. }))
                .text_color(secondary)
                .truncate()
                .child(model.description);
            let title = div()
                .text_size(FONT_SIZE())
                .line_height(zpx(if cfg!(windows) { 16. } else { 14. }))
                .font_weight(FontWeight::SEMIBOLD)
                .truncate()
                .when(model.title_italic, |d| d.italic())
                .child(model.title);
            let text = div().flex().flex_col().flex_1().min_w_0().mr(SPACING());
            // GHD `ToolbarButtonStyle.Subtitle` (push-pull button): title
            // first; `Standard` (the dropdowns): description first
            if model.push_pull {
                text.child(title).child(description)
            } else {
                text.child(description).child(title)
            }
        })
        .when_some(model.pr_badge, |d, badge| {
            // `.pr-badge`: 22 px tall, `#N` + the CI status; clickable once
            // a status is known (opens the check-run popover)
            let clickable = badge.status.is_some();
            let bounds = badge.bounds.clone();
            let badge_bg = if model.open {
                gpui_kit::transparent_black()
            } else {
                t.toolbar_background
            };
            d.child(
                div()
                    .id("pr-badge")
                    .relative()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .h(zpx(22.))
                    .px(SPACING_HALF())
                    .mr(SPACING())
                    .rounded(BORDER_RADIUS())
                    .border_1()
                    .border_color(t.toolbar_badge_background)
                    .bg(badge_bg)
                    .when(clickable, |d| {
                        d.cursor_pointer().hover(move |s| s.bg(hover_bg)).on_click(
                            move |_, _, cx| {
                                let show = corvene_core::AppState::global(cx)
                                    .read(cx)
                                    .show_ci_status_popover;
                                Dispatcher::set_show_ci_status_popover(!show, cx);
                                cx.stop_propagation();
                            },
                        )
                    })
                    // Corvene (`336-request-reviewers`): the badge's menu
                    .when(badge.number.is_some() && badge.github, |d| {
                        d.on_mouse_down(MouseButton::Right, |ev: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            let s = corvene_core::AppState::global(cx).read(cx);
                            let Some(id) = s.selected.filter(|_| {
                                s.flags.bool(corvene_core::flags::ids::REQUEST_REVIEWERS)
                            }) else {
                                return;
                            };
                            let items = vec![crate::context_menu::MenuItem::new(
                                mac_or("Request Reviewers…", "Request reviewers…"),
                                move |_, cx| Dispatcher::show_request_reviewers(id, cx),
                            )];
                            crate::native_menu::show_context_menu(items, ev.position, window, cx);
                        })
                    })
                    .child(
                        canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                            .absolute()
                            .inset_0(),
                    )
                    .when_some(badge.number.clone(), |d, number| {
                        d.child(
                            div()
                                .text_size(FONT_SIZE_SM())
                                .line_height(zpx(22.))
                                .child(number),
                        )
                    })
                    // `356-stacked-pull-requests`
                    .when_some(badge.stack_position.clone(), |d, position| {
                        d.child(
                            div()
                                .ml(SPACING_HALF())
                                .text_size(FONT_SIZE_XS())
                                .child(position),
                        )
                    })
                    .when_some(badge.status, |d, (status, conclusion)| {
                        d.child(
                            crate::ci_status::ci_status(status, conclusion)
                                .when(badge.number.is_some(), |d| d.ml(SPACING_HALF())),
                        )
                    }),
            )
        })
        .when_some(model.badge, |d, ab| {
            // `.ahead-behind` pill: 13 px tall (darwin; elsewhere no height
            // is set and the 16 px octicons make it 16), radius 8, 9 px text
            d.child(
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(zpx(2.))
                    .px(zpx(5.))
                    .h(zpx(if cfg!(target_os = "macos") { 13. } else { 16. }))
                    .mr(SPACING_HALF())
                    .rounded(zpx(8.))
                    .bg(t.toolbar_badge_background)
                    .text_size(FONT_SIZE_XS())
                    .line_height(zpx(11.))
                    // GHD `formatCompactNumber` (1.2k)
                    .when(ab.ahead > 0, |d| {
                        d.child(crate::format::format_compact_number(
                            f64::from(ab.ahead),
                            &Default::default(),
                        ))
                        .child(octicon(Octicon::ArrowUp, text).size(zpx(9.)))
                    })
                    .when(ab.behind > 0, |d| {
                        d.child(crate::format::format_compact_number(
                            f64::from(ab.behind),
                            &Default::default(),
                        ))
                        .child(octicon(Octicon::ArrowDown, text).size(zpx(9.)))
                    }),
            )
        })
        .when(foldout.is_some(), |d| {
            d.child(octicon(Octicon::TriangleDown, text).when(model.open, |s| {
                s.with_transformation(Transformation::rotate(Radians(std::f32::consts::PI)))
            }))
        });
    let button = match model.tooltip {
        // Corvene (`1101-push-size-tooltip`): the size arrives after hovering
        Some(_) if model.load_push_size => crate::widgets::with_live_directed_tooltip(
            button,
            |cx| {
                let s = corvene_core::AppState::global(cx).read(cx);
                let size = s.selected.and_then(|id| Dispatcher::push_size_for(s, id));
                push_size_tooltip(size.flatten())
            },
            crate::widgets::TooltipDirection::South,
        ),
        Some(tip) if model.tooltip_fixed_width => crate::widgets::with_fixed_width_tooltip(
            button,
            tip,
            crate::widgets::TooltipDirection::South,
        ),
        Some(tip) => crate::widgets::with_directed_tooltip(
            button,
            tip,
            crate::widgets::TooltipDirection::South,
        ),
        None => button,
    };
    if let Some(((target, constraint), width)) = resize {
        // `.resizable-component` + `.resize-handle` (6 px, `right: -3px`)
        let state = resize_state.clone();
        return div()
            .relative()
            .flex_none()
            .w(width)
            .child(button.w_full())
            .child(
                div()
                    .id(match target {
                        ResizeTarget::Worktree => "toolbar-worktree-resize-handle",
                        ResizeTarget::Branch => "toolbar-branch-resize-handle",
                    })
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right(zpx(-3.))
                    .w(zpx(6.))
                    .occlude()
                    .child(crate::widgets::touch_drag_occluder())
                    .cursor(CursorStyle::ResizeLeftRight)
                    .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                        cx.stop_propagation();
                        if ev.click_count >= 2 {
                            // `onReset`
                            state.drag.set(None);
                            state.live.set(None);
                            Dispatcher::update_settings(cx, |s| match target {
                                ResizeTarget::Worktree => s.worktree_dropdown_width = None,
                                ResizeTarget::Branch => s.branch_dropdown_width = None,
                            });
                            return;
                        }
                        state.drag.set(Some(ResizeDrag {
                            target,
                            start_x: ev.position.x,
                            start_width: unzoom(width),
                            constraint,
                        }));
                        window.refresh();
                    }),
            )
            .into_any_element();
    }
    if model.cancel {
        // Corvene (`295-cancel-network-operations`): Stop in place of the ▾
        let stop = div()
            .id("toolbar-push-pull-cancel")
            // the label is the tooltip too
            .icon_button_label("Stop")
            .h(TOOLBAR_BUTTON_HEIGHT())
            .w(TOOLBAR_ARROW_WIDTH())
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .border_r_1()
            .border_color(t.toolbar_button_border)
            .bg(t.toolbar_background)
            .text_color(t.toolbar_text)
            .cursor_pointer()
            .hover(move |s| s.bg(hover_bg).text_color(hover_text))
            .on_click(|_, _, cx| {
                if let Some(id) = corvene_core::AppState::global(cx).read(cx).selected {
                    Dispatcher::cancel_network(id, cx);
                }
            })
            .child(octicon(Octicon::X, t.toolbar_text));
        return div()
            .flex()
            .flex_row()
            .flex_none()
            .w(split_width)
            .child(button.w(split_width - TOOLBAR_ARROW_WIDTH()))
            .child(stop)
            .into_any_element();
    }
    if !arrow {
        return button.into_any_element();
    }
    // `ToolbarDropdownStyle.MultiOption`: a 39 px ▾ button next to the main one
    div()
        .flex()
        .flex_row()
        .flex_none()
        .w(split_width)
        .child(button.w(split_width - TOOLBAR_ARROW_WIDTH()))
        .child(
            div()
                .id("toolbar-push-pull-arrow")
                .icon_button_label("Push, pull, fetch options")
                .h(TOOLBAR_BUTTON_HEIGHT())
                .w(TOOLBAR_ARROW_WIDTH())
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .border_r_1()
                .border_color(t.toolbar_button_border)
                .bg(arrow_bg)
                .text_color(arrow_text)
                .cursor_pointer()
                .when(!arrow_open, move |d| {
                    d.hover(move |s| s.bg(hover_bg).text_color(hover_text))
                })
                .on_click(|_, _, cx| Dispatcher::toggle_foldout(Foldout::PushPull, cx))
                .child(octicon(Octicon::TriangleDown, arrow_text)),
        )
        .into_any_element()
}

/// Flag `408-toolbar-open-buttons` (Corvene addition, desktop/desktop#21171):
/// icon buttons after Push / Pull that open the repository in the external
/// editor and the shell, like Repository › Open in … (GHD has no such
/// toolbar buttons).
fn open_in_buttons(cx: &App) -> Vec<AnyElement> {
    let s = corvene_core::AppState::global(cx).read(cx);
    if !s.flags.bool(corvene_core::flags::ids::TOOLBAR_OPEN_BUTTONS) {
        return Vec::new();
    }
    let Some(path) = s.selected_repository().map(|r| r.path.clone()) else {
        return Vec::new();
    };
    let (editor, shell) = (s.editor_label(), s.shell_label());
    let t = cx.ghd();
    let (hover_bg, hover_text) = (
        t.toolbar_button_hover_background,
        t.toolbar_button_hover_text,
    );
    let button = |id: &'static str, icon: Octicon, label: String| {
        crate::widgets::with_directed_tooltip(
            div()
                .id(id)
                .h(TOOLBAR_BUTTON_HEIGHT())
                .w(TOOLBAR_BUTTON_HEIGHT())
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .border_r_1()
                .border_color(t.toolbar_button_border)
                .text_color(t.toolbar_text)
                .cursor_pointer()
                .hover(move |s| s.bg(hover_bg).text_color(hover_text))
                .child(octicon(icon, t.toolbar_text)),
            label,
            crate::widgets::TooltipDirection::South,
        )
    };
    let editor_path = path.clone();
    vec![
        button(
            "toolbar-open-in-editor",
            Octicon::FileCode,
            format!("Open in {editor}"),
        )
        .on_click(move |_, _, cx| Dispatcher::open_in_editor(editor_path.clone(), cx))
        .into_any_element(),
        button(
            "toolbar-open-in-shell",
            Octicon::Terminal,
            format!("Open in {shell}"),
        )
        .on_click(move |_, _, cx| Dispatcher::open_in_shell(&path, cx))
        .into_any_element(),
    ]
}

/// Corvene (`527-multiple-accounts`): at the toolbar's right end, the
/// avatar of the account the selected repository uses when its host has
/// several; a click lists them to switch (Repository Settings › Remote has
/// the same choice) and offers Add Account. GHD's toolbar has no account.
fn account_button(cx: &App) -> Option<AnyElement> {
    let s = corvene_core::AppState::global(cx).read(cx);
    if !s.multiple_accounts() {
        return None;
    }
    let repo = s.selected_repository()?;
    let endpoint = repo.github.as_ref()?.endpoint.clone();
    let accounts: Vec<corvene_core::Account> =
        s.accounts_for(&endpoint).into_iter().cloned().collect();
    if accounts.len() < 2 {
        return None;
    }
    let current = s.account_for_repository(repo.id)?.clone();
    let id = repo.id;
    let t = cx.ghd();
    let (hover_bg, hover_text) = (
        t.toolbar_button_hover_background,
        t.toolbar_button_hover_text,
    );
    let enterprise = !current.is_dotcom();
    let current_login = current.login.clone();
    let button = div()
        .id("toolbar-account")
        .h(TOOLBAR_BUTTON_HEIGHT())
        .w(TOOLBAR_BUTTON_HEIGHT())
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .border_l_1()
        .border_color(t.toolbar_button_border)
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg).text_color(hover_text))
        .child(crate::widgets::avatar_image(
            current
                .avatar_url
                .as_deref()
                .and_then(|u| crate::widgets::avatar_lookup_url(u, cx)),
            zpx(20.),
            cx,
        ))
        .on_click(move |ev, window, cx| {
            let mut items: Vec<crate::context_menu::MenuItem> = accounts
                .iter()
                .map(|a| {
                    let login = a.login.clone();
                    let label = match &a.name {
                        Some(name) if name != &a.login => format!("@{} ({name})", a.login),
                        _ => format!("@{}", a.login),
                    };
                    crate::context_menu::MenuItem::checkbox(
                        label,
                        a.login == current_login,
                        move |_, cx| {
                            Dispatcher::set_repository_account(id, Some(login.clone()), cx)
                        },
                    )
                })
                .collect();
            items.push(crate::context_menu::MenuItem::separator());
            items.push(crate::context_menu::MenuItem::new(
                mac_or("Add Account…", "Add account…"),
                move |_, cx| Dispatcher::show_add_account_dialog(enterprise, cx),
            ));
            crate::native_menu::show_context_menu(items, ev.position(), window, cx);
        });
    Some(
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_row()
            .justify_end()
            .child(crate::widgets::with_directed_tooltip(
                button,
                format!(
                    "This repository uses @{} on {}",
                    current.login,
                    current.friendly_endpoint()
                ),
                crate::widgets::TooltipDirection::South,
            ))
            .into_any_element(),
    )
}

/// The toolbar row: 50 px tall including its 1 px bottom border.
pub fn toolbar(
    buttons: Vec<ToolbarButtonModel>,
    resize: &Rc<ToolbarResize>,
    cx: &App,
) -> impl IntoElement {
    let t = cx.ghd();
    let dragging = resize.drag.get().is_some();
    let listeners = resize.clone();
    div()
        .id("toolbar")
        .w_full()
        .h(TOOLBAR_HEIGHT())
        .flex_none()
        .flex()
        .flex_row()
        .min_w_0()
        .bg(t.toolbar_background)
        .border_b_1()
        .border_color(t.toolbar_border)
        .text_color(t.toolbar_text)
        .children(buttons.into_iter().map(|b| toolbar_button(b, resize, cx)))
        .children(open_in_buttons(cx))
        .children(account_button(cx))
        .when(focus_visible(cx).is_some(), |d| {
            // a mouse press anywhere moves focus off the button
            d.child(canvas(
                |_, _, _| {},
                |_, _, window, _| {
                    window.on_mouse_event(|_: &MouseDownEvent, phase, _, cx| {
                        if phase == DispatchPhase::Capture {
                            set_focus_visible(None, cx);
                        }
                    });
                },
            ))
        })
        .when(dragging, |d| {
            // `handleDragMove` / `handleDragStop` on the document
            d.child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        window.set_window_cursor_style(CursorStyle::ResizeLeftRight);
                        let state = listeners.clone();
                        window.on_mouse_event(move |ev: &MouseMoveEvent, _, window, cx| {
                            if let Some(drag) = state.drag.get() {
                                let width = drag
                                    .constraint
                                    .clamp(drag.start_width + unzoom(ev.position.x - drag.start_x));
                                state.live.set(Some((drag.target, width)));
                                // `404-toolbar-width-save`: GHD writes localStorage
                                // on every move; Corvene waits for the drop
                                let every_move = corvene_core::AppState::global(cx)
                                    .read(cx)
                                    .flags
                                    .text(corvene_core::flags::ids::TOOLBAR_WIDTH_SAVE)
                                    == "every-move";
                                if every_move {
                                    Dispatcher::update_settings(cx, |s| match drag.target {
                                        ResizeTarget::Worktree => {
                                            s.worktree_dropdown_width = Some(width)
                                        }
                                        ResizeTarget::Branch => {
                                            s.branch_dropdown_width = Some(width)
                                        }
                                    });
                                }
                                window.refresh();
                            }
                        });
                        let state = listeners.clone();
                        window.on_mouse_event(move |_: &MouseUpEvent, _, window, cx| {
                            if state.drag.take().is_none() {
                                return;
                            }
                            if let Some((target, width)) = state.live.take() {
                                Dispatcher::update_settings(cx, |s| match target {
                                    ResizeTarget::Worktree => {
                                        s.worktree_dropdown_width = Some(width)
                                    }
                                    ResizeTarget::Branch => s.branch_dropdown_width = Some(width),
                                });
                            }
                            window.refresh();
                        });
                    },
                )
                .absolute()
                .size_0(),
            )
        })
}

/// Chromium's `outline: auto` with `outline-offset: -4px` on Linux: a 1 px
/// line 2 px inside the button around the 2 px ring
/// ([`crate::widgets::focus_ring_colors`]).
fn focus_visible_ring(cx: &App) -> Div {
    let (inner, outer) = crate::widgets::focus_ring_colors(cx);
    div()
        .absolute()
        .inset(zpx(2.))
        .border_1()
        .border_color(outer)
        .rounded(zpx(4.))
        .child(
            div()
                .absolute()
                .inset_0()
                .border_2()
                .border_color(inner)
                .rounded(zpx(3.)),
        )
}
