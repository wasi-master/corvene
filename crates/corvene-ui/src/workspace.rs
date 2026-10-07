//! Root view: title bar, toolbar, resizable sidebar + content, foldouts, dialogs.
//!
//! Deviation: `407-smaller-minimum-sizes` lowers the sidebar minimum from
//! GHD's 220 px (`ui/app.tsx` `sidebarWidth`) to 120 px.
//! Deviation: `617-section-switch-restores-commit-focus` gives the commit
//! summary or description back its focus when Changes is shown again after
//! another section (GHD `ui/repository.tsx` unmounts the commit form with
//! the tab, so its focus is lost).
//! `419-extra-zoom-inputs`: ⌘ / Ctrl + mouse wheel zooms; GHD only zooms
//! from the View menu's shortcuts (`main-process/menu/build-default-menu.ts`).
//! `115-sidebar-on-right`: the sidebar is the resizable group's second
//! panel, right of the diff (GHD `ui/repository.tsx` always renders it
//! first).

use std::cell::Cell;
use std::rc::Rc;

use corvene_core::{AppState, Dispatcher, Section, WorkspaceId};
use corvene_platform::editors::SETTINGS_LABEL;
use gpui_kit::component::resizable::{
    ResizablePanelEvent, ResizableState, h_resizable, resizable_panel,
};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::actions::{FocusNext, FocusPaneLeft, FocusPaneRight, FocusPrevious};
use crate::banner::{BannerView, banner_toast_frame, update_banner};
use crate::branch_list::BranchFoldout;
use crate::changes::ChangesSidebar;
use crate::ci_check_popover::CiCheckPopover;
use crate::cloning_view::cloning_view;
use crate::dialogs::DialogHost;
use crate::diff_view::{DiffSource, DiffView, diff_header};
use crate::foldout::{FoldoutPanels, foldout_layer};
use crate::history::HistorySidebar;
use crate::no_changes::{SuggestedAction, no_changes};
use crate::no_repositories::NoRepositoriesView;
use crate::repository_list::RepositoryFoldout;
use crate::selected_commit::SelectedCommitView;
use crate::stacked_diff_view::StackedDiffView;
use crate::stash_view::StashDiffViewer;
use crate::tab_bar::{TabModel, tab_bar};
use crate::tab_strip::tab_strip;
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::title_bar::{light_title_bar, title_bar, title_bar_with};
use crate::toolbar::{
    ToolbarResize, toolbar, toolbar_models, toolbar_widths, worktree_button_visible,
};
use crate::welcome::WelcomeView;
use crate::worktree_list::WorktreeFoldout;
use corvene_core::tutorial::TutorialStep;

/// Which `MissingRepository` variant replaces the repository view.
enum MissingRepository {
    Unsafe {
        id: u64,
        name: String,
        path: std::path::PathBuf,
        trusting: bool,
    },
    NotFound {
        id: u64,
        name: String,
        path: std::path::PathBuf,
        can_clone_again: bool,
    },
}

pub struct Workspace {
    focus_handle: FocusHandle,
    state: Entity<AppState>,
    /// The workspace this window shows (`crate::windows`,
    /// `corvene_core::workspace`).
    workspace_id: WorkspaceId,
    section: Section,
    sidebar_width: Pixels,
    resizable: Entity<ResizableState>,
    /// The compact (phone) layout, see `theme::compact`: the sidebar above
    /// the content instead of beside it, this tall.
    compact: bool,
    compact_sidebar_height: Pixels,
    /// The handle between them is being dragged: where the pointer went
    /// down and how tall the sidebar was then.
    compact_drag: Option<(Pixels, Pixels)>,
    /// The tallest the sidebar may get (the content keeps some room).
    compact_sidebar_max: Pixels,
    /// `#window-zoom-info`: the factor to show and when it was set
    /// (GHD `ZoomInfo`: 750 ms hold after a 100 ms transition).
    zoom_info: Option<(f32, std::time::Instant)>,
    zoom_info_nonce: u64,
    changes: Entity<ChangesSidebar>,
    history: Entity<HistorySidebar>,
    selected_commit: Entity<SelectedCommitView>,
    stash_view: Entity<StashDiffViewer>,
    /// `798-blame`: shown in place of the diff while a blame is open.
    blame_view: Entity<crate::blame_view::BlameView>,
    /// `345-issues` / `346-releases`: shown in place of the commit view
    /// while those lists are open.
    issue_view: Entity<crate::issue_view::IssueView>,
    /// `348-pull-request-review`: the overview or the file under review.
    review_view: Entity<crate::pull_request_review_view::PullRequestReviewView>,
    release_view: Entity<crate::release_view::ReleaseView>,
    /// `1218-compare-refs`: the combined diff of two compared refs.
    ref_compare_view: Entity<crate::ref_compare_view::RefCompareView>,
    /// `1110-repository-insights`: shown in place of the tab's content.
    insights_view: Entity<crate::insights_view::InsightsView>,
    /// The onboarding tutorial's right-hand panel.
    tutorial_panel: Entity<crate::tutorial_panel::TutorialPanel>,
    /// `1114-hook-results`: under the sidebar's tabs; sized by
    /// `hook_results_width`, which the sidebar keeps current.
    hook_results: Entity<crate::hook_results::HookResults>,
    hook_results_width: Rc<Cell<Pixels>>,
    repository_foldout: Entity<RepositoryFoldout>,
    branch_foldout: Entity<BranchFoldout>,
    worktree_foldout: Entity<WorktreeFoldout>,
    dialogs: Entity<DialogHost>,
    diff_view: Entity<DiffView>,
    /// `1311-stacked-diff`: the included or selected files' diffs in one list.
    stacked_view: Entity<StackedDiffView>,
    welcome: Option<Entity<WelcomeView>>,
    /// GHD `Banner`: the app's banner, its focus and dismissal.
    banner_view: Entity<BannerView>,
    no_repositories: Entity<NoRepositoriesView>,
    /// The branch button's PR badge rectangle (anchor of the CI popover).
    pr_badge_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// Resize handles of the worktree and branch buttons.
    toolbar_resize: Rc<ToolbarResize>,
    ci_popover: Entity<CiCheckPopover>,
    /// The open foldout as of the last state change (to focus its filter
    /// once when it opens).
    last_foldout: Option<corvene_core::Foldout>,
    /// Whether a popup was open at the last state change, to refocus the
    /// root when it closes.
    popup_was_open: bool,
    /// What had focus as the first popup opened: a closing `<dialog>` hands
    /// focus back to it (Chromium's "previously focused element"), so a
    /// clicked commit button keeps its `:focus` look after a hook dialog.
    focus_before_popup: Option<FocusHandle>,
    /// A tab click or View › Show Changes / History asked for the section's
    /// list to take focus at the next render (`603-focus-list-on-section-switch`).
    focus_section_list: bool,
    /// The launch has not placed focus yet (`604-launch-focuses-commit-summary`).
    launch_focus_pending: bool,
    /// History shows the diff alone (`801-history-review-mode`).
    review_mode: bool,
    /// The section as of the last render, and the commit field that had
    /// focus when Changes was left (`617-section-switch-restores-commit-focus`).
    rendered_section: Section,
    left_commit_field: Option<FocusHandle>,
    /// The side the sidebar was laid out on (`115-sidebar-on-right`): the
    /// panel group's sizes are dropped when it changes.
    sidebar_on_right: bool,
}

/// GHD `sidebarWidth` minimum (220 px), or 120 px with
/// `407-smaller-minimum-sizes`.
fn sidebar_min_width(state: &AppState) -> Pixels {
    if state
        .flags
        .bool(corvene_core::flags::ids::SMALLER_MINIMUM_SIZES)
    {
        zpx(120.)
    } else {
        SIDEBAR_MIN_WIDTH()
    }
}

impl Workspace {
    pub fn new(
        state: Entity<AppState>,
        workspace_id: WorkspaceId,
        sidebar_width: Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        // GHD: `ipcRenderer.on('focus')` → refreshRepository.
        // `_setAppFocusState` pauses the pull request updater while blurred.
        // `429-multiple-windows`: the active window's workspace is the one
        // window-level work (URLs, the menu bar) is for.
        cx.observe_window_activation(window, |this, window, cx| {
            let active = window.is_window_active();
            if active {
                Dispatcher::activate_workspace(this.workspace_id, cx);
            }
            Dispatcher::set_app_focus_state(active, cx);
            if active {
                Dispatcher::refresh_selected(cx);
            }
        })
        .detach();
        // When a dialog or foldout closes, put keyboard focus back on the root so
        // menu actions stay available (GPUI disables items whose action has no handler
        // in the focus path).
        cx.observe_in(&state, window, |this, state, window, cx| {
            // this window's workspace, not the focused one's
            let (popup_open, foldout) = {
                let s = state.read(cx);
                let ws = s.workspace(this.workspace_id);
                (
                    ws.is_some_and(|w| w.popups.current_popup().is_some()),
                    ws.and_then(|w| w.foldout),
                )
            };
            let overlay_open = popup_open || foldout.is_some();
            let popup_closed = this.popup_was_open && !popup_open;
            if !this.popup_was_open && popup_open {
                this.focus_before_popup = window.focused(cx);
            }
            this.popup_was_open = popup_open;
            let restore = if popup_closed {
                this.focus_before_popup.take()
            } else {
                None
            };
            // A closing dialog's focused field is still in the last frame;
            // once it is gone nothing would have focus and no shortcut would
            // match (`keymap::MENU` needs the `Workspace` context). A
            // foldout's filter box is caught by the focus-lost listener
            // below. Not `!contains_focused`: that is also true of a field
            // focused but not drawn yet (History's compare box after ⇧⌘B
            // from Changes), and the focus would be taken from it.
            // A handle whose element is gone by then loses focus again and
            // the focus-lost listener below takes it to the root.
            if !overlay_open && (popup_closed || window.focused(cx).is_none()) {
                window.focus(restore.as_ref().unwrap_or(&this.focus_handle), cx);
            }
            // GHD foldouts put the caret in their filter box when they open,
            // however they were opened (`FilterList` autoFocus)
            if foldout != this.last_foldout {
                if crate::toolbar::opened_by_button(cx) != foldout {
                    crate::toolbar::forget_opened_by_button(cx);
                }
                this.last_foldout = foldout;
                match foldout {
                    Some(corvene_core::Foldout::Repository) => this
                        .repository_foldout
                        .update(cx, |f, cx| f.focus_filter(window, cx)),
                    Some(corvene_core::Foldout::Branch) => this
                        .branch_foldout
                        .update(cx, |f, cx| f.focus_filter(window, cx)),
                    Some(corvene_core::Foldout::Worktree) => this
                        .worktree_foldout
                        .update(cx, |f, cx| f.focus_filter(window, cx)),
                    _ => {}
                }
            }
        })
        .detach();

        // Focus whose element went away (a foldout's filter box after
        // Escape, History's compare box blurring itself) leaves no key
        // context, and the menu shortcuts (`keymap::MENU`) need `Workspace`:
        // in GHD they are the window's accelerators and work whatever has
        // focus.
        cx.on_focus_lost(window, |this, window, cx| {
            window.focus(&this.focus_handle, cx);
        })
        .detach();

        let resizable = cx.new(|_| ResizableState::default());
        cx.subscribe(&resizable, |this, state, _: &ResizablePanelEvent, cx| {
            let sizes = state.read(cx).sizes();
            let sidebar = if this.sidebar_on_right {
                sizes.last()
            } else {
                sizes.first()
            };
            if let Some(width) = sidebar.copied()
                && width != this.sidebar_width
            {
                this.sidebar_width = width;
                Dispatcher::update_settings(cx, |s| s.sidebar_width = unzoom(width));
                cx.notify();
            }
        })
        .detach();

        let changes = cx.new(|cx| ChangesSidebar::new(state.clone(), window, cx));
        let history = cx.new(|cx| HistorySidebar::new(state.clone(), window, cx));
        let selected_commit = cx.new(|cx| SelectedCommitView::new(state.clone(), cx));
        let stash_view = cx.new(|cx| StashDiffViewer::new(state.clone(), cx));
        let blame_view = cx.new(|cx| crate::blame_view::BlameView::new(state.clone(), cx));
        let issue_view = cx.new(|cx| crate::issue_view::IssueView::new(state.clone(), window, cx));
        let review_view = cx.new(|cx| {
            crate::pull_request_review_view::PullRequestReviewView::new(state.clone(), cx)
        });
        let release_view = cx.new(|cx| crate::release_view::ReleaseView::new(state.clone(), cx));
        let ref_compare_view =
            cx.new(|cx| crate::ref_compare_view::RefCompareView::new(state.clone(), cx));
        let insights_view = cx.new(|cx| crate::insights_view::InsightsView::new(state.clone(), cx));
        let tutorial_panel =
            cx.new(|cx| crate::tutorial_panel::TutorialPanel::new(state.clone(), cx));
        let hook_results_width = Rc::new(Cell::new(px(250.)));
        let hook_results = cx.new(|cx| {
            crate::hook_results::HookResults::new(state.clone(), hook_results_width.clone(), cx)
        });
        let repository_foldout = cx.new(|cx| RepositoryFoldout::new(state.clone(), window, cx));
        let branch_foldout = cx.new(|cx| BranchFoldout::new(state.clone(), window, cx));
        let worktree_foldout = cx.new(|cx| WorktreeFoldout::new(state.clone(), window, cx));
        let diff_view = cx.new(|cx| DiffView::new(state.clone(), DiffSource::WorkingDirectory, cx));
        let stacked_view =
            cx.new(|cx| StackedDiffView::new(state.clone(), DiffSource::WorkingDirectory, cx));
        let dialogs = cx.new(|cx| DialogHost::new(state.clone(), cx));
        let pr_badge_bounds: Rc<Cell<Bounds<Pixels>>> = Rc::new(Cell::new(Bounds::default()));
        let ci_popover =
            cx.new(|cx| CiCheckPopover::new(state.clone(), pr_badge_bounds.clone(), cx));
        let welcome = (!state.read(cx).settings.welcome_completed)
            .then(|| cx.new(|cx| WelcomeView::new(state.clone(), window, cx)));
        let no_repositories = cx.new(|cx| NoRepositoriesView::new(state.clone(), window, cx));
        let banner_view = cx.new(|cx| BannerView::new(state.clone(), window, cx));
        window.focus(&focus_handle, cx);
        let sidebar_min = sidebar_min_width(state.read(cx));
        let sidebar_on_right = state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::SIDEBAR_ON_RIGHT);

        Self {
            focus_handle,
            state,
            workspace_id,
            section: Section::Changes,
            sidebar_width: sidebar_width.max(sidebar_min),
            resizable,
            compact: false,
            compact_sidebar_height: zpx(0.),
            compact_drag: None,
            compact_sidebar_max: zpx(2000.),
            zoom_info: None,
            zoom_info_nonce: 0,
            changes,
            history,
            selected_commit,
            stash_view,
            blame_view,
            issue_view,
            review_view,
            release_view,
            ref_compare_view,
            insights_view,
            tutorial_panel,
            hook_results,
            hook_results_width,
            repository_foldout,
            branch_foldout,
            worktree_foldout,
            pr_badge_bounds,
            toolbar_resize: Rc::new(ToolbarResize::default()),
            ci_popover,
            last_foldout: None,
            popup_was_open: false,
            focus_before_popup: None,
            focus_section_list: false,
            launch_focus_pending: true,
            review_mode: false,
            rendered_section: Section::Changes,
            left_commit_field: None,
            sidebar_on_right,
            dialogs,
            diff_view,
            stacked_view,
            welcome,
            banner_view,
            no_repositories,
        }
    }

    /// View › Go to Summary (`focusCommitSummary`).
    pub fn focus_commit_summary(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_section(Section::Changes, cx);
        self.changes
            .update(cx, |changes, cx| changes.focus_summary(window, cx));
    }

    /// Edit › Find: focus the changes filter (`selectAllInput` in GHD).
    pub fn focus_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.section() == Section::Changes {
            self.changes
                .update(cx, |changes, cx| changes.focus_filter(window, cx));
        }
    }

    /// View › Show/Hide Changes Filter.
    pub fn toggle_changes_filter(&mut self, cx: &mut Context<Self>) {
        self.changes
            .update(cx, |changes, cx| changes.toggle_filter(cx));
    }

    pub fn section(&self) -> Section {
        self.section
    }

    /// `View › Show Repository List` (⌘T): open the foldout and focus its filter.
    pub fn show_repository_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        Dispatcher::toggle_foldout(corvene_core::Foldout::Repository, cx);
        if self.state.read(cx).foldout == Some(corvene_core::Foldout::Repository) {
            self.repository_foldout
                .update(cx, |f, cx| f.focus_filter(window, cx));
        }
    }

    /// The branch foldout's filter text while it is open on the Branches tab
    /// (`847-new-branch-from-filter`: Branch › New Branch… prefills it).
    pub fn open_branch_filter(&self, cx: &App) -> Option<String> {
        let s = self.state.read(cx);
        (s.foldout == Some(corvene_core::Foldout::Branch)
            && s.branches_tab == corvene_core::BranchesTab::Branches)
            .then(|| self.branch_foldout.read(cx).filter_text(cx))
            .filter(|text| !text.is_empty())
    }

    /// `View › Show Branches List` (⌘B): open the foldout and focus its filter.
    pub fn show_branches_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        Dispatcher::toggle_foldout(corvene_core::Foldout::Branch, cx);
        if self.state.read(cx).foldout == Some(corvene_core::Foldout::Branch) {
            self.branch_foldout
                .update(cx, |f, cx| f.focus_filter(window, cx));
        }
    }

    /// `View › Show Worktrees List` (⌥⌘W): open the foldout and focus its filter.
    pub fn show_worktrees_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        Dispatcher::toggle_foldout(corvene_core::Foldout::Worktree, cx);
        if self.state.read(cx).foldout == Some(corvene_core::Foldout::Worktree) {
            self.worktree_foldout
                .update(cx, |f, cx| f.focus_filter(window, cx));
        }
    }

    /// Branch › Compare to Branch (⇧⌘B): History tab with the compare box focused.
    pub fn show_compare(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_section(Section::History, cx);
        Dispatcher::close_foldout(cx);
        self.history.update(cx, |h, cx| h.focus_compare(window, cx));
    }

    /// Corvene (`612-navigation-shortcuts`, ⌃⌘P): the branch foldout on
    /// its Pull Requests tab (the Branches list for a non-GitHub repository).
    pub fn show_pull_requests_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        Dispatcher::change_branches_tab(corvene_core::BranchesTab::PullRequests, cx);
        if self.state.read(cx).foldout != Some(corvene_core::Foldout::Branch) {
            Dispatcher::toggle_foldout(corvene_core::Foldout::Branch, cx);
        }
        self.branch_foldout
            .update(cx, |f, cx| f.focus_filter(window, cx));
    }

    /// Corvene (`612-navigation-shortcuts`, ⌘3): focus the diff on the right.
    pub fn focus_diff(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.section {
            Section::Changes => self.diff_view.update(cx, |d, cx| d.focus(window, cx)),
            Section::History => self
                .selected_commit
                .update(cx, |v, cx| v.focus_diff(window, cx)),
        }
    }

    /// Corvene (`619-arrow-keys-between-panes`): ← (`step` -1) / → move
    /// keyboard focus from the focused list or diff to the pane on that
    /// side: History's commit list, file list and diff, Changes' file list
    /// and diff. Nothing happens at either end or from anywhere else.
    pub fn focus_pane(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        let showing_stash = self
            .state
            .read(cx)
            .selected_state()
            .is_some_and(|rs| rs.showing_stash);
        let panes: Vec<FocusHandle> = match self.section {
            Section::Changes if showing_stash => return,
            Section::Changes => vec![
                self.changes.read(cx).list_focus_handle(),
                self.diff_view.read(cx).focus_handle(),
            ],
            Section::History => {
                let mut panes = Vec::new();
                if !self.review_mode_active(cx) {
                    panes.push(self.history.read(cx).list_focus_handle());
                }
                panes.extend(self.selected_commit.read(cx).pane_focus_handles(cx));
                panes
            }
        };
        let Some(ix) = panes.iter().position(|h| h.contains_focused(window, cx)) else {
            return;
        };
        if let Some(next) = ix.checked_add_signed(step).and_then(|ix| panes.get(ix)) {
            window.focus(next, cx);
        }
    }

    /// Corvene (`612-navigation-shortcuts`, ⌥↓ / ⌥↑ in the diff): the
    /// next / previous file of the section's file list, clamped at the ends.
    pub fn step_file(&mut self, delta: isize, cx: &mut Context<Self>) {
        let showing_stash = self
            .state
            .read(cx)
            .selected_state()
            .is_some_and(|rs| rs.showing_stash);
        match self.section {
            Section::Changes if showing_stash => self
                .stash_view
                .update(cx, |v, cx| v.select_relative(delta, cx)),
            Section::Changes => self
                .changes
                .update(cx, |changes, cx| changes.select_relative(delta, cx)),
            Section::History => self
                .selected_commit
                .update(cx, |v, cx| v.select_relative(delta, cx)),
        }
    }

    /// The user switched sections (a tab, ⌘1 / ⌘2, ⌃Tab). Corvene
    /// (`603-focus-list-on-section-switch`): the section's list takes focus,
    /// where GHD leaves it on the body.
    pub fn switch_section(&mut self, section: Section, cx: &mut Context<Self>) {
        self.set_section(section, cx);
        if self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::FOCUS_LIST_ON_SECTION_SWITCH)
        {
            self.focus_section_list = true;
            cx.notify();
        }
    }

    /// Corvene (`604-launch-focuses-commit-summary`): once the first
    /// repository's status has loaded after launch, the commit summary takes
    /// focus when there are changes to commit and nothing else is open.
    fn place_launch_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.launch_focus_pending {
            return;
        }
        let (loaded, has_changes, free) = {
            let s = self.state.read(cx);
            let rs = s.selected_state();
            (
                // no repository at launch: nothing to wait for
                s.selected.is_none() || rs.is_some_and(|rs| rs.status.is_some()),
                rs.is_some_and(|rs| rs.changed_files() > 0),
                s.popup().is_none() && s.foldout.is_none() && s.settings.welcome_completed,
            )
        };
        if !loaded {
            return;
        }
        self.launch_focus_pending = false;
        let enabled = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::LAUNCH_FOCUSES_COMMIT_SUMMARY);
        if enabled && has_changes && free && self.section == Section::Changes {
            self.changes
                .update(cx, |changes, cx| changes.focus_summary(window, cx));
        }
    }

    pub fn set_section(&mut self, section: Section, cx: &mut Context<Self>) {
        if self.section != section {
            self.section = section;
            if let Some(id) = self.state.read(cx).selected {
                Dispatcher::show_section(id, section, cx);
            }
            cx.notify();
        }
    }

    fn sidebar(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.ghd();
        let selected = match self.section {
            Section::Changes => 0,
            Section::History => 1,
        };
        let this = cx.entity();
        self.hook_results_width.set(self.sidebar_width);
        div()
            .id("repository-sidebar")
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .map(|d| {
                // the seam is on the diff's side
                if self.sidebar_on_right && !self.compact {
                    d.border_l_1()
                } else {
                    d.border_r_1()
                }
            })
            .border_color(t.box_border)
            .bg(t.background)
            .child(tab_bar(
                vec![
                    TabModel {
                        // Corvene (`727-stash-dot-on-changes-tab`): the
                        // branch has a stash, seen from History
                        dot: self.section == Section::History
                            && self
                                .state
                                .read(cx)
                                .flags
                                .bool(corvene_core::flags::ids::STASH_DOT_ON_CHANGES_TAB)
                            && self
                                .state
                                .read(cx)
                                .selected_state()
                                .is_some_and(|rs| rs.stash.is_some()),
                        id: "tab-changes",
                        label: "Changes".into(),
                        count: self
                            .state
                            .read(cx)
                            .selected_state()
                            .map(|rs| rs.changed_files())
                            .filter(|n| *n > 0)
                            .map(|n| crate::format::files_changed_badge(n).into()),
                    },
                    TabModel {
                        dot: false,
                        id: "tab-history",
                        label: "History".into(),
                        count: None,
                    },
                ],
                selected,
                move |ix, _, cx| {
                    this.update(cx, |ws, cx| {
                        ws.switch_section(
                            if ix == 0 {
                                Section::Changes
                            } else {
                                Section::History
                            },
                            cx,
                        )
                    });
                },
                cx,
            ))
            .child(self.hook_results.clone())
            .child(
                div().flex_1().min_h_0().child(match self.section {
                    // cached views (like the diffs): a sidebar re-renders when
                    // it changes, not for every frame of the diff beside it
                    Section::Changes => self
                        .changes
                        .clone()
                        .cached(StyleRefinement::default().size_full())
                        .into_any_element(),
                    Section::History => self
                        .history
                        .clone()
                        .cached(StyleRefinement::default().size_full())
                        .into_any_element(),
                }),
            )
    }

    fn content(&self, cx: &Context<Self>) -> AnyElement {
        let state = self.state.read(cx);
        let repo = state.selected_repository();
        let has_github = repo.and_then(|r| r.github.as_ref()).is_some();
        // flags 342-344: GitLab, Gitea or Bitbucket
        let hosted_kind = repo
            .and_then(|r| state.hosted_repository(r.id))
            .map(|h| h.kind.site_name(&h.repo.endpoint));
        let rs = state.selected_state();
        let selected_change = rs.and_then(|r| {
            let path = r.selected_file.as_ref()?;
            r.status
                .as_deref()?
                .files
                .iter()
                .find(|f| &f.path == path)
                .cloned()
        });
        let showing_stash = rs.is_some_and(|r| r.showing_stash);
        let multi_selected = rs.map(|r| r.selected_files.len()).unwrap_or(0);
        // `798-blame`: the Blame view replaces the tab's diff
        if rs
            .and_then(|r| r.blame.as_ref())
            .is_some_and(|b| b.section == self.section)
        {
            return self.blame_view.clone().into_any_element();
        }
        // `1110-repository-insights`: so does Insights
        if rs
            .and_then(|r| r.insights.as_ref())
            .is_some_and(|i| i.section == self.section)
            && corvene_core::insights::enabled(state)
        {
            return self.insights_view.clone().into_any_element();
        }
        // `1218-compare-refs`: the combined diff of two compared refs
        if self.section == Section::History
            && rs.is_some_and(corvene_core::ref_compare::showing_changes)
            && corvene_core::ref_compare::enabled(state)
        {
            return self.ref_compare_view.clone().into_any_element();
        }
        // `345-issues` / `346-releases`: the selected issue / release
        // replaces History's commit view
        if self.section == Section::History {
            // `348-pull-request-review`
            if rs.is_some_and(|r| corvene_core::pull_request_review::review_of(state, r).is_some())
            {
                return self.review_view.clone().into_any_element();
            }
            if rs.is_some_and(|r| {
                corvene_core::issues::issues_of(state, r).is_some_and(|i| i.loaded || i.loading)
            }) {
                return self.issue_view.clone().into_any_element();
            }
            if rs.is_some_and(|r| corvene_core::releases::releases_of(state, r).is_some()) {
                return self.release_view.clone().into_any_element();
            }
        }
        match self.section {
            Section::Changes if showing_stash => self.stash_view.clone().into_any_element(),
            // `1311-stacked-diff`: the included or selected files in one list
            Section::Changes
                if rs.is_some_and(|r| {
                    corvene_core::stacked_diff::working_stack(state, r).is_some()
                }) =>
            {
                StackedDiffView::embed(&self.stacked_view).into_any_element()
            }
            Section::Changes if multi_selected > 1 => {
                crate::no_changes::multiple_selection(multi_selected, cx).into_any_element()
            }
            Section::Changes if selected_change.is_some() => {
                let file = selected_change.unwrap();
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .min_h_0()
                    .child(diff_header(
                        &file.path,
                        file.status.kind,
                        file.old_path.as_deref(),
                        // `763-diff-header-mtime`
                        rs.and_then(|r| r.diff_file_modified.as_ref())
                            .filter(|(path, _)| *path == file.path)
                            .filter(|_| {
                                state
                                    .flags
                                    .bool(corvene_core::flags::ids::DIFF_HEADER_MTIME)
                            })
                            .map(|(_, at)| *at),
                        &self.diff_view,
                        cx,
                    ))
                    .child(DiffView::embed(&self.diff_view))
                    .into_any_element()
            }
            // `renderTutorialPane` in place of "No local changes"
            Section::Changes if state.selected_tutorial_step().is_valid() => {
                let step = state.selected_tutorial_step();
                if matches!(step, TutorialStep::AllDone | TutorialStep::Announced) {
                    div()
                        .size_full()
                        .relative()
                        .child(
                            canvas(
                                |_, _, _| {},
                                move |_, _, _, cx| {
                                    // `onTutorialCompletionAnnounced`, deferred:
                                    // it notifies AppState
                                    if step == TutorialStep::AllDone {
                                        cx.defer(|cx| {
                                            Dispatcher::mark_tutorial_completion_announced(cx)
                                        });
                                    }
                                },
                            )
                            .absolute()
                            .size_0(),
                        )
                        .child(crate::tutorial_panel::tutorial_done(cx))
                        .into_any_element()
                } else {
                    crate::tutorial_panel::tutorial_welcome(cx).into_any_element()
                }
            }
            Section::Changes => {
                let (repo_id, repo_path, editor_label, editor_available, shell_label) = {
                    let s = self.state.read(cx);
                    let repo = s.selected_repository();
                    (
                        repo.map(|r| r.id),
                        repo.map(|r| r.path.clone()),
                        s.editor_label(),
                        // `isExternalEditorAvailable`: `useCustomEditor ||
                        // selectedExternalEditor !== null`
                        s.custom_editor_in_use().is_some()
                            || s.settings.external_editor.is_some()
                            || !s.editors.is_empty(),
                        // flag `724-no-changes-open-in-shell` (Corvene
                        // addition; GHD `NoChanges` has no shell action)
                        s.flags
                            .bool(corvene_core::flags::ids::NO_CHANGES_OPEN_IN_SHELL)
                            .then(|| s.shell_label()),
                    )
                };
                let path = repo_path.clone().unwrap_or_default();
                let mut actions: Vec<SuggestedAction> = repo_id
                    .and_then(|id| crate::no_changes::primary_action(state, id))
                    .into_iter()
                    .collect();
                // Corvene (`772-editor-picker-dropdown`): the installed
                // editors; a pick becomes the editor (the repository's own
                // one when `518-per-repo-editor` set it) and opens
                let editor_menu = {
                    let s = self.state.read(cx);
                    let per_repo = repo_id.filter(|_| {
                        repo_path.as_deref().is_some_and(|p| {
                            s.repository_editor(p).is_some()
                                || s.repository_custom_editor(p).is_some()
                        })
                    });
                    // `523-custom-editor-list`: the custom editors too
                    let customs: Vec<(usize, corvene_core::CustomIntegration)> =
                        if s.flags.bool(corvene_core::flags::ids::CUSTOM_EDITOR_LIST) {
                            s.settings
                                .custom_editors()
                                .into_iter()
                                .enumerate()
                                .collect()
                        } else {
                            Vec::new()
                        };
                    (s.flags
                        .bool(corvene_core::flags::ids::EDITOR_PICKER_DROPDOWN)
                        && s.editors.len() + customs.len() > 1)
                        .then(|| {
                            s.editors
                                .iter()
                                .map(|e| {
                                    let name = e.name.clone();
                                    let path = path.clone();
                                    crate::context_menu::MenuItem::checkbox(
                                        e.name.clone(),
                                        e.name == editor_label,
                                        move |_, cx| {
                                            match per_repo {
                                                Some(id) => Dispatcher::set_repository_editor(
                                                    id,
                                                    Some(name.clone()),
                                                    cx,
                                                ),
                                                None => {
                                                    let name = name.clone();
                                                    Dispatcher::update_settings(cx, move |s| {
                                                        s.external_editor = Some(name);
                                                        s.use_custom_editor = false;
                                                    })
                                                }
                                            }
                                            Dispatcher::open_in_editor(path.clone(), cx);
                                        },
                                    )
                                    .icon(
                                        s.app_icons.get(&e.path).map(|icon| {
                                            crate::widgets::integration_icon(&e.path, icon)
                                        }),
                                    )
                                })
                                .chain(customs.iter().map(|(ix, custom)| {
                                    let (ix, path) = (*ix, path.clone());
                                    let label = custom.display_name(ix, true);
                                    // a repository keeps a copy (`518`)
                                    let own = corvene_core::RepoCustomEditor {
                                        path: custom.path.clone(),
                                        arguments: custom.arguments.clone(),
                                        name: label.clone(),
                                    };
                                    crate::context_menu::MenuItem::checkbox(
                                        label.clone(),
                                        label == editor_label,
                                        move |_, cx| {
                                            match per_repo {
                                                Some(id) => {
                                                    Dispatcher::set_repository_custom_editor(
                                                        id,
                                                        Some(own.clone()),
                                                        cx,
                                                    )
                                                }
                                                None => Dispatcher::update_settings(cx, move |s| {
                                                    s.use_custom_editor = true;
                                                    s.custom_editor_index = ix;
                                                }),
                                            }
                                            Dispatcher::open_in_editor(path.clone(), cx);
                                        },
                                    )
                                }))
                                .collect::<Vec<_>>()
                        })
                };
                if editor_available {
                    actions.push(SuggestedAction {
                        id: "suggested-editor",
                        on_click: std::rc::Rc::new({
                            let path = path.clone();
                            move |_, cx| Dispatcher::open_in_editor(path.clone(), cx)
                        }),
                        title: "Open the repository in your external editor".into(),
                        description: Some(format!("Select your editor in {SETTINGS_LABEL}").into()),
                        hint: "Repository menu or".into(),
                        keys: &["⌘", "⇧", "A"],
                        button_label: format!("Open in {editor_label}").into(),
                        primary: false,
                        menu: editor_menu,
                    });
                }
                actions.extend([SuggestedAction {
                    id: "suggested-finder",
                    on_click: std::rc::Rc::new({
                        let path = path.clone();
                        move |_, cx| Dispatcher::show_repository(&path, cx)
                    }),
                    // `getPlatformFileManagerName` and the menu item's label
                    title: if cfg!(windows) {
                        "View the files of your repository in Explorer"
                    } else {
                        crate::context_menu::mac_or(
                            "View the files of your repository in Finder",
                            "View the files of your repository in your File Manager",
                        )
                    }
                    .into(),
                    description: None,
                    hint: "Repository menu or".into(),
                    keys: &["⌘", "⇧", "F"],
                    button_label: if cfg!(windows) {
                        "Show in Explorer"
                    } else {
                        crate::context_menu::mac_or("Show in Finder", "Show in your File Manager")
                    }
                    .into(),
                    primary: false,
                    menu: None,
                }]);
                if let Some(shell) = shell_label {
                    actions.push(SuggestedAction {
                        id: "suggested-shell",
                        on_click: std::rc::Rc::new({
                            let path = path.clone();
                            move |_, cx| Dispatcher::open_in_shell(&path, cx)
                        }),
                        title: format!("Open the repository in {shell}").into(),
                        description: Some(format!("Select your shell in {SETTINGS_LABEL}").into()),
                        hint: "Repository menu or".into(),
                        keys: &["⌃", "`"],
                        button_label: format!("Open in {shell}").into(),
                        primary: false,
                        menu: None,
                    });
                }
                // `725-no-changes-view-pull-request`: GHD shows no remote
                // action while the branch has an open pull request
                let open_pr = repo_id
                    .filter(|_| {
                        state
                            .flags
                            .bool(corvene_core::flags::ids::NO_CHANGES_VIEW_PULL_REQUEST)
                    })
                    .and_then(|id| state.current_pull_request(id).map(|pr| (id, pr.clone())));
                if let Some((id, pr)) = open_pr {
                    // flags 342-344: "Merge request !12" on GitLab
                    let kind = pr.host_kind();
                    let noun = if kind.is_merge_request() {
                        "Merge request"
                    } else {
                        "Pull request"
                    };
                    actions.insert(
                        0,
                        SuggestedAction {
                            id: "suggested-view-pull-request",
                            on_click: std::rc::Rc::new(move |_, cx| {
                                Dispatcher::show_pull_request(id, cx)
                            }),
                            title: format!(
                                "{noun} {} is open for the current branch",
                                pr.number_label()
                            )
                            .into(),
                            description: Some(pr.title.into()),
                            hint: "Branch menu or".into(),
                            keys: &["⌘", "R"],
                            button_label: if kind.is_merge_request() {
                                crate::context_menu::mac_or(
                                    "View Merge Request",
                                    "View merge request",
                                )
                            } else {
                                crate::context_menu::mac_or(
                                    "View Pull Request",
                                    "View pull request",
                                )
                            }
                            .into(),
                            primary: true,
                            menu: None,
                        },
                    );
                }
                if has_github && let Some(id) = repo_id {
                    actions.push(SuggestedAction {
                        id: "suggested-github",
                        on_click: std::rc::Rc::new(move |_, cx| Dispatcher::view_on_github(id, cx)),
                        title: "Open the repository page on GitHub in your browser".into(),
                        description: None,
                        hint: "Repository menu or".into(),
                        keys: &["⌘", "⇧", "G"],
                        button_label: "View on GitHub".into(),
                        primary: false,
                        menu: None,
                    });
                }
                if let (Some(kind), Some(id)) = (hosted_kind, repo_id) {
                    actions.push(SuggestedAction {
                        id: "suggested-host",
                        on_click: std::rc::Rc::new(move |_, cx| Dispatcher::view_on_github(id, cx)),
                        title: format!("Open the repository page on {} in your browser", kind)
                            .into(),
                        description: None,
                        hint: "Repository menu or".into(),
                        keys: &["⌘", "⇧", "G"],
                        button_label: format!("View on {kind}").into(),
                        primary: false,
                        menu: None,
                    });
                }
                no_changes(actions, cx).into_any_element()
            }
            Section::History => self
                .selected_commit
                .clone()
                .cached(StyleRefinement::default().size_full())
                .into_any_element(),
        }
    }

    /// Corvene (`801-history-review-mode`): View › Toggle History Review
    /// Mode (⌃⌘S) hides the repository sidebar and the commit's file list
    /// in History, so the diff gets the whole width.
    pub fn toggle_review_mode(&mut self, cx: &mut Context<Self>) {
        self.review_mode = !self.review_mode;
        cx.notify();
    }

    fn review_mode_active(&self, cx: &App) -> bool {
        self.review_mode
            && self
                .state
                .read(cx)
                .flags
                .bool(corvene_core::flags::ids::HISTORY_REVIEW_MODE)
    }

    fn repository_view(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.ghd();
        let sidebar_min = sidebar_min_width(self.state.read(cx));
        if self.section == Section::History && self.review_mode_active(cx) {
            return div()
                .flex_1()
                .min_h_0()
                .w_full()
                .border_t_1()
                .border_color(t.box_border)
                .child(self.content(cx))
                .into_any_element();
        }
        if self.compact {
            // A phone: the list on top, what it selects below, the seam
            // between them a handle to drag. (The kit's resizable group
            // takes its drags from GPUI's drag and drop, which a vertical
            // group in this place never started.)
            let weak = cx.weak_entity();
            return div()
                .flex_1()
                .min_h_0()
                .w_full()
                .flex()
                .flex_col()
                .border_t_1()
                .border_color(t.box_border)
                .child(
                    div()
                        .flex_none()
                        .w_full()
                        .h(self.compact_sidebar_height)
                        .child(self.sidebar(cx)),
                )
                .child(
                    div()
                        .id("compact-split-handle")
                        .relative()
                        .flex_none()
                        .w_full()
                        .h(zpx(12.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor(CursorStyle::ResizeUpDown)
                        .bg(t.toolbar_background)
                        .border_y_1()
                        .border_color(t.box_border)
                        .child(crate::widgets::touch_drag_handle())
                        .child(
                            div()
                                .w(zpx(36.))
                                .h(zpx(4.))
                                .rounded(zpx(2.))
                                .bg(t.text_secondary),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                this.compact_drag =
                                    Some((event.position.y, this.compact_sidebar_height));
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        ),
                )
                .child(div().flex_1().min_h_0().w_full().child(self.content(cx)))
                .map(|d| {
                    // the pointer is followed anywhere in the window until
                    // the button is released (always listening: the press
                    // and the first movement can arrive within one frame)
                    d.child(
                        canvas(
                            |_, _, _| {},
                            move |_, _, window, _| {
                                let moved = weak.clone();
                                window.on_mouse_event(move |event: &MouseMoveEvent, _, _, cx| {
                                    moved
                                        .update(cx, |this, cx| {
                                            if let Some((from, height)) = this.compact_drag {
                                                this.compact_sidebar_height =
                                                    (height + event.position.y - from)
                                                        .clamp(zpx(120.), this.compact_sidebar_max);
                                                cx.notify();
                                            }
                                        })
                                        .ok();
                                });
                                let released = weak.clone();
                                window.on_mouse_event(move |_: &MouseUpEvent, _, _, cx| {
                                    released
                                        .update(cx, |this, cx| {
                                            if this.compact_drag.take().is_some() {
                                                cx.notify();
                                            }
                                        })
                                        .ok();
                                });
                            },
                        )
                        .absolute()
                        .size_0(),
                    )
                })
                .into_any_element();
        }
        let sidebar = resizable_panel()
            .size(self.sidebar_width)
            .size_range(sidebar_min..zpx(900.))
            .child(crate::active_resizable::active_resizable(
                "repository-sidebar-resizable",
                &self.resizable,
                None,
                crate::active_resizable::ResizableDescription::new(
                    "Repository sidebar",
                    sidebar_min..zpx(900.),
                )
                .last_panel(self.sidebar_on_right),
                self.sidebar(cx),
            ));
        let content = resizable_panel().child(self.content(cx));
        let group = h_resizable("repository")
            .with_state(&self.resizable)
            // GHD's 6 px handle is invisible; the sidebar's own border is the seam.
            .with_handle_appearance(std::rc::Rc::new(|_, _, _| {
                // (Android: a finger can drag it too)
                Some(
                    div()
                        .relative()
                        .size_full()
                        .child(crate::widgets::touch_drag_handle())
                        .into_any_element(),
                )
            }));
        // `115-sidebar-on-right`
        let group = if self.sidebar_on_right {
            group.child(content).child(sidebar)
        } else {
            group.child(sidebar).child(content)
        };
        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .border_t_1()
            .border_color(t.box_border)
            .child(group)
            .into_any_element()
    }

    /// `maybeRenderTutorialPanel`: the repository view with the tutorial
    /// panel on the right while a tutorial step is showing.
    fn repository_view_with_tutorial(&self, cx: &Context<Self>) -> AnyElement {
        if !self.state.read(cx).selected_tutorial_step().is_valid() {
            return self.repository_view(cx).into_any_element();
        }
        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_row()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(self.repository_view(cx)),
            )
            .child(self.tutorial_panel.clone())
            .into_any_element()
    }
}

/// GHD `NoRepositorySelected` (`app.tsx`): `.panel.blankslate` with "No
/// repository selected" when `selectedState` is null, which GHD reaches
/// only in passing; Corvene's File › New Window (`429-multiple-windows`)
/// shows it until a repository is picked.
fn no_repository_selected(cx: &App) -> AnyElement {
    let t = cx.ghd();
    div()
        .id("no-repository-selected")
        .flex_1()
        .min_h_0()
        .w_full()
        .border_t_1()
        .border_color(t.box_border)
        .flex()
        .items_center()
        .justify_center()
        .p(SPACING())
        .bg(t.box_alt_background)
        .text_color(t.text_secondary)
        .text_align(TextAlign::Center)
        .child("No repository selected")
        .into_any_element()
}

thread_local! {
    /// Trackpad pixels scrolled with ⌘ / Ctrl held since the last zoom step.
    static WHEEL_ZOOM_PIXELS: Cell<f32> = const { Cell::new(0.) };
}

/// `419-extra-zoom-inputs`: ⌘ / Ctrl + wheel zooms in (up) and out (down).
/// A capture-phase listener painted before the content, so the scroll views
/// under the pointer never see those wheel events. A mouse wheel notch is one
/// step; a trackpad steps every 40 px.
fn wheel_zoom_listener(workspace: WeakEntity<Workspace>) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |_, _, window, _| {
            window.on_mouse_event(move |event: &ScrollWheelEvent, phase, _, cx| {
                if phase != DispatchPhase::Capture || !event.modifiers.secondary() {
                    return;
                }
                cx.stop_propagation();
                let step = match event.delta {
                    ScrollDelta::Lines(lines) => lines.y.signum() as i32,
                    ScrollDelta::Pixels(pixels) => WHEEL_ZOOM_PIXELS.with(|acc| {
                        let total = acc.get() + f32::from(pixels.y);
                        if total.abs() >= 40. {
                            acc.set(0.);
                            total.signum() as i32
                        } else {
                            acc.set(total);
                            0
                        }
                    }),
                };
                if step != 0 {
                    workspace.update(cx, |w, cx| w.zoom(step, cx)).ok();
                }
            });
        },
    )
    .absolute()
    .size_0()
}

impl Workspace {
    /// View › Zoom In (+1) / Zoom Out (-1) / Reset Zoom (0): GHD's
    /// `zoom(ZoomDirection)` steps through `ZoomInFactors`, persists the
    /// factor (Electron keeps `zoomFactor`) and shows `#window-zoom-info`.
    pub fn zoom(&mut self, direction: i32, cx: &mut Context<Self>) {
        use crate::theme::sizes::{next_zoom_factor, set_zoom_factor, zoom_factor};
        let current = zoom_factor();
        let next = if direction == 0 {
            1.0
        } else {
            next_zoom_factor(current, direction)
        };
        set_zoom_factor(next);
        tracing::info!(from = current, to = next, "zoom changed");
        // sizes read the factor at render; the kit's font size and radius
        // are copied at apply time
        crate::theme::apply(cx.ghd().clone(), cx);
        Dispatcher::update_settings(cx, |s| s.window_zoom_factor = next);
        let settings_sidebar = self.state.read(cx).settings.sidebar_width;
        self.sidebar_width = zpx(settings_sidebar).max(sidebar_min_width(self.state.read(cx)));
        // the panel group keeps screen-pixel sizes: drop them so the next
        // layout takes the sidebar's zoomed width again
        self.resizable.update(cx, |state, _| state.clear());
        self.zoom_info_nonce += 1;
        let nonce = self.zoom_info_nonce;
        self.zoom_info = Some((next, std::time::Instant::now()));
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(850))
                .await;
            this.update(cx, |this, cx| {
                if this.zoom_info_nonce == nonce {
                    this.zoom_info = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.refresh_windows();
        cx.notify();
    }

    /// `#window-zoom-info` (`styles/ui/window/_zoom-info.scss`): a pill
    /// with the percentage, centred over the content, ignoring the mouse.
    fn zoom_info_overlay(&self, cx: &App) -> Option<AnyElement> {
        let (factor, _) = self.zoom_info?;
        let t = cx.ghd();
        Some(
            div()
                .id("window-zoom-info")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .p(SPACING())
                        .min_w(zpx(100.))
                        .rounded(zpx(100.))
                        .bg(t.tooltip_background)
                        .text_color(t.tooltip_text)
                        .text_size(FONT_SIZE_MD())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_align(TextAlign::Center)
                        .child(format!("{}%", (factor * 100.).round() as i32)),
                )
                .into_any_element(),
        )
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // `429-multiple-windows`: this frame reads this window's workspace
        // (`s.selected`, `s.foldout`, `s.popup()` in every child); the
        // focused one is current again once the frame is drawn
        Dispatcher::enter_workspace(self.workspace_id, cx);
        cx.defer(|cx| Dispatcher::leave_workspace(cx));
        // the dialog drawn this frame registers itself again
        crate::keyboard_nav::clear_dialog_trap(window);
        // The section is per repository (`repositoryState.selectedSection`), so
        // dispatcher-driven switches (Amend Commit…, undo) and repository
        // changes land here.
        if let Some(section) = self.state.read(cx).selected_state().map(|rs| rs.section)
            && section != self.section
        {
            self.section = section;
            // `427-back-forward-navigation`: Back / Forward to the other
            // section moves keyboard focus to its list, as a tab switch
            // with `603` does (the focused list or field is going away)
            let navigation = self
                .state
                .read(cx)
                .flags
                .bool(corvene_core::flags::ids::BACK_FORWARD_NAVIGATION);
            if navigation && self.focus_handle.contains_focused(window, cx) {
                self.focus_section_list = true;
            }
        }
        self.place_launch_focus(window, cx);
        // `115-sidebar-on-right` changed: the group's sizes belong to the
        // old panel order
        let sidebar_on_right = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::SIDEBAR_ON_RIGHT);
        if sidebar_on_right != self.sidebar_on_right {
            self.sidebar_on_right = sidebar_on_right;
            self.resizable.update(cx, |state, _| state.clear());
        }
        self.compact = crate::theme::compact(window);
        let page = crate::theme::page_size(window);
        crate::theme::set_compact_page_width(self.compact.then_some(page.width));
        self.compact_sidebar_max = (page.height - TOOLBAR_HEIGHT() - zpx(140.)).max(zpx(120.));
        if self.compact && self.compact_sidebar_height == zpx(0.) {
            // the list and the commit form get a little over half of what
            // the toolbar leaves
            self.compact_sidebar_height = ((page.height - TOOLBAR_HEIGHT()) * 0.55).max(zpx(160.));
        }
        let review = self.review_mode_active(cx);
        self.selected_commit
            .update(cx, |v, cx| v.set_file_list_hidden(review, cx));
        // Corvene (`617-section-switch-restores-commit-focus`): leaving
        // Changes remembers a focused summary / description, coming back
        // focuses it again (before the list `603` would focus)
        let restore_commit_focus = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::SECTION_SWITCH_RESTORES_COMMIT_FOCUS);
        let previous = std::mem::replace(&mut self.rendered_section, self.section);
        let mut restored = false;
        if restore_commit_focus && previous != self.section {
            if previous == Section::Changes {
                self.left_commit_field = self.changes.read(cx).focused_commit_field(window);
            } else if self.section == Section::Changes
                && let Some(handle) = self.left_commit_field.take()
            {
                window.focus(&handle, cx);
                restored = true;
            }
        }
        if std::mem::take(&mut self.focus_section_list) && !restored {
            let handle = match self.section {
                Section::Changes => self.changes.read(cx).list_focus_handle(),
                Section::History => self.history.read(cx).list_focus_handle(),
            };
            window.focus(&handle, cx);
        }
        let t = cx.ghd();
        let welcome_done = self.state.read(cx).settings.welcome_completed;
        // `inNoRepositoriesViewState`: a paused tutorial shows the blank slate
        let tutorial_paused = self.state.read(cx).selected_tutorial_step() == TutorialStep::Paused;
        if welcome_done {
            self.welcome = None;
        }
        // GHD `SelectionType.MissingRepository`: the unsafe variant when git
        // named an unsafe directory, else "Can't find"
        let missing_repository = {
            let state = self.state.read(cx);
            state.selected_repository().and_then(|repo| {
                let rs = state.repo_states.get(&repo.id);
                match rs.and_then(|rs| rs.unsafe_path.clone()) {
                    Some(path) => Some(MissingRepository::Unsafe {
                        id: repo.id,
                        name: repo.name(),
                        path,
                        trusting: rs.is_some_and(|rs| rs.trusting_path),
                    }),
                    None if repo.missing => Some(MissingRepository::NotFound {
                        id: repo.id,
                        name: repo.name(),
                        path: repo.path.clone(),
                        can_clone_again: repo
                            .github
                            .as_ref()
                            .is_some_and(|gh| !gh.clone_url.is_empty()),
                    }),
                    None => None,
                }
            })
        };
        let update_available = {
            let state = self.state.read(cx);
            if state.update.banner_visible {
                state.update.status.available().cloned().map(|u| {
                    let manager = match state.update.status {
                        corvene_core::UpdateStatus::AvailableViaHomebrew { manager, .. } => {
                            Some(manager)
                        }
                        _ => None,
                    };
                    (u, manager)
                })
            } else {
                None
            }
        };
        let (
            mut buttons,
            foldout,
            popup,
            (has_repos, no_selection, tabs),
            cloning,
            banner,
            worktree_button,
            ci_popover,
            (_, worktree_width, branch_width),
        ) = {
            let state = self.state.read(cx);
            let widths = toolbar_widths(
                state,
                crate::theme::page_size(window).width,
                self.sidebar_width,
                &self.toolbar_resize,
            );
            (
                toolbar_models(state, self.sidebar_width, widths, &self.pr_badge_bounds),
                state.foldout,
                state.popup().is_some(),
                (
                    !state.repositories.is_empty(),
                    // `429-multiple-windows`: a new window before a
                    // repository is picked (GHD `NoRepositorySelected`)
                    state.selected.is_none(),
                    // `430-repository-tabs`
                    state.flags.bool(corvene_core::flags::ids::REPOSITORY_TABS),
                ),
                state.cloning.latest().cloned(),
                state.banner.clone(),
                worktree_button_visible(state),
                state.show_ci_status_popover
                    && state.selected.is_some_and(|id| {
                        state.current_pull_request(id).is_some()
                            // `334-branch-ci-status`
                            || state.branch_ci_ref(id).is_some()
                    }),
                widths,
            )
        };

        if self.compact {
            // the toolbar's buttons share the width; nothing to resize
            let width = page.width / buttons.len().max(1) as f32;
            for button in &mut buttons {
                button.width = Some(width);
                button.resize = None;
            }
        }
        let compact = self.compact;

        // GHD `inNoRepositoriesViewState` (repositories.length === 0, or a
        // paused tutorial): no toolbar (`renderToolbar`) and, like the welcome
        // flow, the transparent `light-title-bar` laid over the content
        let blank_slate = tutorial_paused || (!has_repos && cloning.is_none());
        let clone_cancel = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::CLONE_CANCEL);
        let bare = self.welcome.is_some() || blank_slate;
        // `Popup` turns off the menu shortcuts (`keymap::MENU`) while a
        // dialog is open
        let mut key_context = KeyContext::default();
        key_context.add("Workspace");
        if self.state.read(cx).popup().is_some() {
            key_context.add("Popup");
        }
        let wheel_zoom = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::EXTRA_ZOOM_INPUTS);
        let banner_toast = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::BANNER_AS_TOAST);
        div()
            .id("workspace")
            .key_context(key_context)
            .track_focus(&self.focus_handle)
            // `619-arrow-keys-between-panes` (bound in the lists and the diff)
            .on_action(
                cx.listener(|this, _: &FocusPaneLeft, window, cx| this.focus_pane(-1, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &FocusPaneRight, window, cx| this.focus_pane(1, window, cx)),
            )
            .on_action(cx.listener(|this, _: &FocusNext, window, cx| {
                let modal = this.state.read(cx).popup().is_some();
                crate::keyboard_nav::move_focus(true, modal, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusPrevious, window, cx| {
                let modal = this.state.read(cx).popup().is_some();
                crate::keyboard_nav::move_focus(false, modal, window, cx);
            }))
            .relative()
            .when(wheel_zoom, |d| {
                d.child(wheel_zoom_listener(cx.entity().downgrade()))
            })
            .size_full()
            .flex()
            .flex_col()
            .bg(t.background)
            .text_color(t.text)
            .text_size(FONT_SIZE())
            .font_family(crate::theme::ui_font())
            // `430-repository-tabs`: the tab strip takes the title bar's
            // place (and adds one off macOS)
            .when(!bare && (cfg!(target_os = "macos") || tabs), |d| {
                if tabs {
                    d.child(title_bar_with(tab_strip(self.state.read(cx), cx), cx))
                } else {
                    d.child(title_bar(cx))
                }
            })
            .when_some(self.welcome.clone(), |d, welcome| {
                d.child(div().flex_1().min_h_0().w_full().child(welcome))
            })
            .when(!bare, |d| {
                d.child(toolbar(buttons, &self.toolbar_resize, cx))
            })
            // `1212-bisect`: under the toolbar while the repository bisects
            .when(
                !bare && cloning.is_none() && missing_repository.is_none(),
                |d| d.children(crate::bisect_bar::bisect_bar(self.state.read(cx), cx)),
            )
            .when(self.welcome.is_none() && !banner_toast, |d| {
                d.when(banner.is_some(), |d| d.child(self.banner_view.clone()))
            })
            // GHD shows the update banner only while no other banner is up
            .when(
                self.welcome.is_none() && banner.is_none() && !banner_toast,
                |d| {
                    d.when_some(update_available.as_ref(), |d, (update, manager)| {
                        d.child(update_banner(update, *manager, cx))
                    })
                },
            )
            .when(self.welcome.is_none(), |d| {
                d.child(if let Some(clone) = cloning.as_ref() {
                    div()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .border_t_1()
                        .border_color(t.box_border)
                        .child(cloning_view(clone, clone_cancel, cx))
                        .into_any_element()
                } else if let Some(missing) = missing_repository.as_ref() {
                    // GHD `SelectionType.MissingRepository` replaces the
                    // whole repository view
                    div()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .border_t_1()
                        .border_color(t.box_border)
                        .child(match missing {
                            MissingRepository::Unsafe {
                                id,
                                name,
                                path,
                                trusting,
                            } => crate::missing_repository::unsafe_repository_view(
                                *id, name, path, *trusting, cx,
                            )
                            .into_any_element(),
                            MissingRepository::NotFound {
                                id,
                                name,
                                path,
                                can_clone_again,
                            } => crate::missing_repository::missing_repository_view(
                                *id,
                                name,
                                path,
                                *can_clone_again,
                                cx,
                            )
                            .into_any_element(),
                        })
                        .into_any_element()
                } else if has_repos && !tutorial_paused && no_selection {
                    no_repository_selected(cx)
                } else if has_repos && !tutorial_paused {
                    self.repository_view_with_tutorial(cx)
                } else {
                    div()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .child(self.no_repositories.clone())
                        .into_any_element()
                })
            })
            .when(bare && cfg!(target_os = "macos"), |d| {
                d.child(light_title_bar())
            })
            // `422-banner-as-toast`: over the content, under the foldouts
            .when(self.welcome.is_none() && banner_toast, |d| {
                d.when(banner.is_some(), |d| {
                    d.child(banner_toast_frame(self.banner_view.clone(), cx))
                })
                .when(banner.is_none(), |d| {
                    d.when_some(update_available.as_ref(), |d, (update, manager)| {
                        d.child(banner_toast_frame(update_banner(update, *manager, cx), cx))
                    })
                })
            })
            .when_some(foldout, |d, foldout| {
                // the worktree button sits between the repository and branch buttons
                let shift = if worktree_button {
                    worktree_width
                } else {
                    zpx(0.)
                };
                // `foldoutStyleOverrides`: as wide as the resized button,
                // at least 365 px
                let foldout_width = |width: Pixels| width.max(zpx(365.));
                let (x, width) = match foldout {
                    // a phone: every foldout spans the window
                    _ if compact => (zpx(0.), page.width),
                    corvene_core::Foldout::Repository => (zpx(0.), self.sidebar_width),
                    corvene_core::Foldout::Worktree => {
                        (self.sidebar_width, foldout_width(worktree_width))
                    }
                    corvene_core::Foldout::Branch => {
                        (self.sidebar_width + shift, foldout_width(branch_width))
                    }
                    corvene_core::Foldout::PushPull => (
                        self.sidebar_width + shift + branch_width,
                        TOOLBAR_BUTTON_WIDTH(),
                    ),
                };
                d.child(foldout_layer(
                    foldout,
                    x,
                    width,
                    FoldoutPanels {
                        repository: &self.repository_foldout,
                        branch: &self.branch_foldout,
                        worktree: &self.worktree_foldout,
                    },
                    window,
                    cx,
                ))
            })
            .when(ci_popover, |d| d.child(self.ci_popover.clone()))
            .children(self.zoom_info_overlay(cx))
            .when(popup, |d| d.child(self.dialogs.clone()))
            // the open dialog's title is the window title (`dialog.rs`);
            // without one it is the app's again
            .when(!popup, |d| {
                d.child(crate::dialog::window_title(crate::dialog::APP_WINDOW_TITLE))
            })
    }
}
