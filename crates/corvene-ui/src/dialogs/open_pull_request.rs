//! Preview Pull Request - GHD `ui/open-pull-request/{open-pull-request-dialog,
//! open-pull-request-header,pull-request-files-changed,pull-request-merge-status}.tsx`
//! with `ui/branches/branch-select.tsx` (`styles/ui/dialogs/_open-pull-request.scss`,
//! `_open-pull-request-header.scss`): a dialog filling the window minus
//! 80 px on every side; header "Merge N commits into [base ▾] from
//! <current>." with the lines added/removed, a resizable file list next to
//! the merge-base diff, the mergeability in the footer.
//!
//! Deviation (flag `pr-preview-empty-files-message`): when the branch has
//! commits but they add up to no file changes against the base, the dialog
//! says "No file changes between <base> and <current>." where GHD
//! (`open-pull-request-dialog.tsx#renderContent`) shows an empty file list
//! and a blank diff.
//!
//! Deviation (`1203-compare-branch-files`): opened from the compare view
//! (`PullRequestPreview::compare_only`) it is titled "Compare Branches",
//! reads "Comparing <base> with <current> (N commits)" and has only a
//! Close button.

use std::cell::Cell;
use std::rc::Rc;

use corvene_core::{
    AppState, Branch, BranchKind, CommittedFileChange, Dispatcher, MergeStatus, PreviewSlot,
    PullRequestPreview,
};
use gpui_kit::component::input::InputState;
use gpui_kit::component::resizable::{
    ResizablePanelEvent, ResizableState, h_resizable, resizable_panel,
};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::context_menu::mac_or;
use crate::widgets::GhdTooltip;
use crate::widgets::{IconButtonA11y, ListRowA11y};

use crate::actions::{
    ExtendSelectionDown, ExtendSelectionPageDown, ExtendSelectionPageUp, ExtendSelectionUp,
    SelectFirstFile, SelectLastFile, SelectNextFile, SelectPageDown, SelectPageUp,
    SelectPreviousFile,
};
use crate::branch_list::group_branches;
use crate::diff_view::{DiffSource, DiffView, diff_options_button, status_icon};
use crate::icons::{Octicon, octicon};
use crate::scrollbar::ScrollbarExt;
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, c, primer};
use crate::widgets::{button, code_ref, primary_button};

/// `pullRequestFileListWidth` constraints (`constrain(250, 100, 600)`).
#[allow(non_snake_case)]
fn FILE_LIST_MIN() -> Pixels {
    zpx(100.)
}
#[allow(non_snake_case)]
fn FILE_LIST_MAX() -> Pixels {
    zpx(600.)
}
/// `.dialog { max-width: calc(100% - var(--spacing-double) * 4) }`
#[allow(non_snake_case)]
fn DIALOG_MARGIN() -> Pixels {
    zpx(80.)
}

pub struct OpenPullRequestDialog {
    state: Entity<AppState>,
    repo: u64,
    diff: Entity<DiffView>,
    resizable: Entity<ResizableState>,
    file_list_width: Pixels,
    /// The file list takes focus on click so ⌘9 / ⌘8 resize it.
    file_list_focus: FocusHandle,
    /// `file_list_focus` held focus at the last render (active selection colours).
    file_list_focused: bool,
    file_scroll: ScrollHandle,
    /// `BranchSelect`: the base branch popover.
    base_select_open: bool,
    base_filter: Entity<InputState>,
    base_button_bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl OpenPullRequestDialog {
    pub fn new(
        state: Entity<AppState>,
        repo: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let diff = cx.new(|cx| DiffView::new(state.clone(), DiffSource::PullRequest, cx));
        let file_list_width = zpx(state.read(cx).settings.pull_request_file_list_width);
        let resizable = cx.new(|_| ResizableState::default());
        cx.subscribe(&resizable, |this, state, _: &ResizablePanelEvent, cx| {
            if let Some(width) = state.read(cx).sizes().first().copied()
                && width != this.file_list_width
            {
                this.file_list_width = width;
                Dispatcher::update_settings(cx, |s| s.pull_request_file_list_width = unzoom(width));
                cx.notify();
            }
        })
        .detach();
        let base_filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
        cx.observe(&base_filter, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            repo,
            diff,
            resizable,
            file_list_width,
            file_list_focus: cx.focus_handle().tab_stop(true),
            file_list_focused: false,
            file_scroll: ScrollHandle::new(),
            base_select_open: false,
            base_filter,
            base_button_bounds: Rc::new(Cell::new(Bounds::default())),
        }
    }

    fn preview(&self, cx: &App) -> Option<PullRequestPreview> {
        self.state
            .read(cx)
            .repo_states
            .get(&self.repo)
            .and_then(|rs| rs.pull_request_preview.clone())
    }

    /// `prBaseBranches` / `prRecentBaseBranches`: only branches on the remote
    /// the pull request targets (the parent's `upstream` for a fork
    /// contributing to it, else the default remote).
    fn base_branches(&self, cx: &App) -> (Vec<Branch>, Vec<String>, Option<String>) {
        let s = self.state.read(cx);
        let Some(rs) = s.repo_states.get(&self.repo) else {
            return (Vec::new(), Vec::new(), None);
        };
        let remote = if s
            .repository(self.repo)
            .is_some_and(|r| r.is_fork_contributing_to_parent())
        {
            Some("upstream".to_string())
        } else {
            Dispatcher::current_remote_in(s, self.repo).map(|r| r.name)
        };
        let Some(info) = rs.info.as_ref() else {
            return (Vec::new(), Vec::new(), None);
        };
        let on_remote = |b: &Branch| match b.kind {
            BranchKind::Local => b.upstream_remote_name() == remote.as_deref(),
            BranchKind::Remote => b
                .name
                .split_once('/')
                .map(|(r, _)| Some(r) == remote.as_deref())
                .unwrap_or(false),
        };
        let all: Vec<Branch> = info
            .branches
            .iter()
            .filter(|b| on_remote(b))
            .cloned()
            .collect();
        let recent: Vec<String> = rs
            .recent_branches
            .iter()
            .filter(|name| all.iter().any(|b| &b.name == *name))
            .cloned()
            .collect();
        (all, recent, rs.default_branch.clone())
    }

    fn close(&self, cx: &mut App) {
        Dispatcher::close_popup(cx);
        Dispatcher::close_pull_request_preview(self.repo, cx);
    }

    /// `onCreatePullRequest`
    fn submit(&self, preview: &PullRequestPreview, cx: &mut App) {
        let has_pr = self
            .state
            .read(cx)
            .current_pull_request(self.repo)
            .is_some();
        // close first: `PushBranchCommits` may replace this dialog
        self.close(cx);
        if has_pr {
            Dispatcher::show_pull_request(self.repo, cx);
        } else {
            // `352-remember-pr-base`
            Dispatcher::remember_pull_request_base(self.repo, preview.base_branch.as_deref(), cx);
            Dispatcher::create_pull_request_with_base(self.repo, preview.base_branch.clone(), cx);
        }
    }

    /// `BranchSelect` button + its popover.
    fn base_select(&self, preview: &PullRequestPreview, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let bounds = self.base_button_bounds.clone();
        let label = preview
            .base_branch
            .clone()
            .unwrap_or_else(|| "Choose a branch".to_string());
        let hover_bg = t.secondary_button_hover_background;
        div()
            .id("pr-base-select")
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING_HALF())
            .h(zpx(25.))
            .px(SPACING())
            .rounded(BORDER_RADIUS())
            .border_1()
            .border_color(t.secondary_button_border)
            .bg(t.secondary_button_background)
            .text_color(t.secondary_button_text)
            .cursor_pointer()
            .hover(move |s| s.bg(hover_bg))
            .on_click(cx.listener(|this, _, _, cx| {
                this.base_select_open = !this.base_select_open;
                cx.notify();
            }))
            .child(
                canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                    .absolute()
                    .inset_0(),
            )
            .child(octicon(Octicon::GitBranch, t.secondary_button_text))
            .child(
                div()
                    .max_w(zpx(200.))
                    .truncate()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(label),
            )
            .child(octicon(Octicon::TriangleDown, t.secondary_button_text))
            .into_any_element()
    }

    fn base_popover(&self, window: &Window, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let anchor = self.base_button_bounds.get();
        let viewport = window.viewport_size();
        let query = self.base_filter.read(cx).value().trim().to_string();
        let (branches, recent, default) = self.base_branches(cx);
        let groups = group_branches(
            &branches,
            default.as_deref(),
            &recent,
            &query,
            crate::branch_list::sort_by_date(cx),
        );
        let width = zpx(365.);
        let x = anchor
            .origin
            .x
            .min(viewport.width - width - zpx(8.))
            .max(zpx(8.));
        let y = anchor.origin.y + anchor.size.height + zpx(4.);
        let repo = self.repo;
        let hover_bg = t.box_selected_active_background;
        let hover_text = t.box_selected_active_text;
        deferred(
            anchored().position(point(zpx(0.), zpx(0.))).child(
                div()
                    .id("pr-base-select-layer")
                    .relative()
                    .w(viewport.width)
                    .h(viewport.height)
                    .child(
                        div()
                            .id("pr-base-select-overlay")
                            .absolute()
                            .inset_0()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.base_select_open = false;
                                    cx.notify();
                                }),
                            ),
                    )
                    .child(
                        div()
                            .id("pr-base-select-popover")
                            .absolute()
                            .left(x)
                            .top(y)
                            .w(width)
                            .max_h(zpx(400.))
                            .flex()
                            .flex_col()
                            .bg(t.box_background)
                            .text_color(t.text)
                            .border_1()
                            .border_color(t.box_border)
                            .rounded(BORDER_RADIUS())
                            .shadow_lg()
                            .overflow_hidden()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(div().flex_none().p(SPACING()).child(
                                crate::widgets::filter_text_box(
                                    "pr-base-filter",
                                    &self.base_filter,
                                    Some(octicon(Octicon::Search, t.text_secondary)),
                                    window,
                                    cx,
                                ),
                            ))
                            .child(if groups.is_empty() {
                                // `noBranchesMessage`
                                div()
                                    .p(SPACING())
                                    .text_size(FONT_SIZE())
                                    .text_color(t.text_secondary)
                                    .child(div().child("Sorry, I can't find that remote branch."))
                                    .child(div().child(
                                        "You can only open pull requests against remote branches.",
                                    ))
                                    .into_any_element()
                            } else {
                                div()
                                    .id("pr-base-branches")
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .flex()
                                    .flex_col()
                                    .children(groups.into_iter().map(|group| {
                                        div()
                                            .flex()
                                            .flex_col()
                                            .child(
                                                div()
                                                    .h(ROW_HEIGHT())
                                                    .pt(SPACING())
                                                    .px(SPACING())
                                                    .flex()
                                                    .items_center()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_size(FONT_SIZE())
                                                    .child(group.title),
                                            )
                                            .children(group.branches.into_iter().map(|b| {
                                                let name = b.name.clone();
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "pr-base-{}",
                                                        b.full_name
                                                    )))
                                                    .h(ROW_HEIGHT())
                                                    .flex()
                                                    .flex_row()
                                                    .items_center()
                                                    .gap(SPACING_HALF())
                                                    .px(SPACING())
                                                    .text_size(FONT_SIZE())
                                                    .cursor_pointer()
                                                    .hover(move |s| {
                                                        s.bg(hover_bg).text_color(hover_text)
                                                    })
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        this.base_select_open = false;
                                                        Dispatcher::update_pull_request_base_branch(
                                                            repo,
                                                            name.clone(),
                                                            cx,
                                                        );
                                                        cx.notify();
                                                    }))
                                                    .child(octicon(Octicon::GitBranch, t.text))
                                                    .child(div().min_w_0().truncate().child(b.name))
                                            }))
                                    }))
                                    .with_scrollbar()
                                    .into_any_element()
                            }),
                    ),
            ),
        )
        .with_priority(25)
        .into_any_element()
    }

    /// `FileList` of the changed files (250 px, resizable).
    /// The preview's files in list order and the selected file's index.
    fn file_order(&self, cx: &App) -> Option<(Vec<String>, Option<usize>)> {
        Some(range_file_order(&self.preview(cx)?))
    }

    /// GHD `List.moveSelection` on `PullRequestFilesChanged`'s `FileList`
    /// (↑ / ↓; single selection, so ⇧↑ / ⇧↓ too): the file `delta` rows
    /// away, wrapping around the ends (GHD `List.moveSelection`), scrolled into view.
    fn select_relative(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some((order, current)) = self.file_order(cx) else {
            return;
        };
        if let Some(ix) = corvene_core::list_selection::step_index(order.len(), current, delta) {
            self.select_index(&order, ix, cx);
        }
    }

    /// Home / End, ⌘↑ / ⌘↓: the first or last file.
    fn select_edge(&mut self, last: bool, cx: &mut Context<Self>) {
        let Some((order, _)) = self.file_order(cx) else {
            return;
        };
        if !order.is_empty() {
            let ix = if last { order.len() - 1 } else { 0 };
            self.select_index(&order, ix, cx);
        }
    }

    fn select_index(&mut self, order: &[String], ix: usize, cx: &mut Context<Self>) {
        Dispatcher::select_pull_request_file(self.repo, order[ix].clone(), cx);
        self.file_scroll.scroll_to_item(ix);
    }

    fn file_list(&self, preview: &PullRequestPreview, cx: &Context<Self>) -> AnyElement {
        range_file_list(
            "pr-file",
            self.repo,
            PreviewSlot::PullRequest,
            preview,
            self.file_list_focused,
            &self.file_scroll,
            cx,
        )
    }

    /// `PullRequestMergeStatus`
    fn merge_status(&self, status: Option<MergeStatus>, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let row = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(zpx(3.))
            .text_size(FONT_SIZE())
            .text_color(t.text_secondary);
        match status {
            None => row.into_any_element(),
            Some(MergeStatus::Loading) => row
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.text)
                        .child("Checking mergeability…"),
                )
                .child("Don’t worry, you can still create the pull request.")
                .into_any_element(),
            Some(MergeStatus::Invalid) => row
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.text)
                        .child("Error checking merge status."),
                )
                .child("Unable to merge unrelated histories in this repository")
                .into_any_element(),
            Some(MergeStatus::Clean) => row
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(zpx(3.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(c(primer::GREEN_500))
                        .child(octicon(Octicon::Check, c(primer::GREEN_500)))
                        .child("Able to merge."),
                )
                .child("These branches can be automatically merged.")
                .into_any_element(),
            Some(MergeStatus::Conflicts) => row
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(zpx(3.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(c(primer::RED_500))
                        .child(octicon(Octicon::X, c(primer::RED_500)))
                        .child("Can't automatically merge."),
                )
                .child("Don’t worry, you can still create the pull request.")
                .into_any_element(),
        }
    }

    /// `.open-pull-request-message`
    fn message(&self, title: &'static str, body: AnyElement, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_center()
            .p(SPACING_DOUBLE())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(SPACING_HALF())
                    .child(octicon(Octicon::GitPullRequest, t.text_secondary).size(zpx(32.)))
                    .child(
                        div()
                            .text_size(FONT_SIZE_MD())
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(body),
            )
            .into_any_element()
    }
}

impl Render for OpenPullRequestDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.file_list_focused = self.file_list_focus.is_focused(window);
        let t = cx.ghd();
        let Some(preview) = self.preview(cx) else {
            return div().into_any_element();
        };
        let viewport = crate::theme::page_size(window);
        let has_pr = self
            .state
            .read(cx)
            .current_pull_request(self.repo)
            .is_some();
        let enterprise = self
            .state
            .read(cx)
            .repository(self.repo)
            .and_then(|r| r.github.as_ref())
            .is_some_and(|gh| gh.endpoint != "https://api.github.com");
        let commit_count = preview.commit_shas.as_ref().map(|s| s.len()).unwrap_or(0);
        let (added, deleted) = preview
            .changeset
            .as_ref()
            .map(|c| (c.lines_added, c.lines_deleted))
            .unwrap_or((0, 0));
        let ok_disabled = preview.commit_shas.as_ref().is_none_or(|s| s.is_empty());
        let no_file_changes = preview
            .changeset
            .as_ref()
            .is_some_and(|c| c.files.is_empty())
            && self
                .state
                .read(cx)
                .flags
                .bool(corvene_core::flags::ids::PR_PREVIEW_EMPTY_FILES_MESSAGE);
        // `renderContent`: no base branch / no changes / files + diff
        let content: AnyElement = if preview.base_branch.is_none() {
            self.message(
                "Could not find a default branch to compare against.",
                div()
                    .child("Select a base branch above.")
                    .into_any_element(),
                cx,
            )
        } else if preview.commit_shas.as_ref().is_some_and(|s| s.is_empty()) {
            let base = preview.base_branch.clone().unwrap_or_default();
            let current = preview.current_branch.clone();
            let body = if preview.merge_status == Some(MergeStatus::Invalid) {
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .justify_center()
                    .gap(zpx(3.))
                    .child(code_ref(base, cx))
                    .child("and")
                    .child(code_ref(current, cx))
                    .child("are entirely different commit histories.")
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .justify_center()
                    .gap(zpx(3.))
                    .child(code_ref(base, cx))
                    .child("is up to date with all commits from")
                    .child(code_ref(current, cx))
                    .child(".")
                    .into_any_element()
            };
            self.message("There are no changes.", body, cx)
        } else if no_file_changes && preview.commit_shas.is_some() {
            let base = preview.base_branch.clone().unwrap_or_default();
            let current = preview.current_branch.clone();
            let body = div()
                .flex()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_center()
                .gap(zpx(3.))
                .child("No file changes between")
                .child(code_ref(base, cx))
                .child("and")
                .child(code_ref(current, cx))
                .child(".")
                .into_any_element();
            self.message("There are no file changes.", body, cx)
        } else if preview.commit_shas.is_none() {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(t.text_secondary)
                .child("Loading…")
                .into_any_element()
        } else {
            div()
                .size_full()
                .flex()
                .flex_col()
                .min_h_0()
                .child(
                    // `.files-changed-header`: the summary and `DiffOptions`
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .p(SPACING())
                        .border_1()
                        .border_color(t.box_border)
                        .rounded_t(BORDER_RADIUS())
                        .text_size(FONT_SIZE())
                        .child(div().flex_1().child("Showing changes from all commits"))
                        .child(diff_options_button(&self.diff, cx)),
                )
                // `1314-code-owners`: who owns the changed files, as the
                // base branch's CODEOWNERS says
                .children(
                    preview
                        .base_branch
                        .as_deref()
                        .filter(|_| !preview.compare_only)
                        .and_then(|base| {
                            crate::code_owners::summary(
                                self.repo,
                                base,
                                preview
                                    .changeset
                                    .iter()
                                    .flat_map(|c| c.files.iter().map(|f| f.path.as_str())),
                                cx,
                            )
                        })
                        .map(|row| {
                            row.flex_none()
                                .px(SPACING())
                                .py(SPACING_HALF())
                                .border_1()
                                .border_t_0()
                                .border_color(t.box_border)
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .border_1()
                        .border_t_0()
                        .border_color(t.box_border)
                        .rounded_b(BORDER_RADIUS())
                        .overflow_hidden()
                        .child(
                            h_resizable("pr-files-diff")
                                .with_state(&self.resizable)
                                .with_handle_appearance(Rc::new(|_, _, _| {
                                    Some(div().into_any_element())
                                }))
                                .child(
                                    resizable_panel()
                                        .size(self.file_list_width)
                                        .size_range(FILE_LIST_MIN()..FILE_LIST_MAX())
                                        .child(
                                            crate::active_resizable::active_resizable(
                                                "pr-file-list-resizable",
                                                &self.resizable,
                                                Some(&self.file_list_focus),
                                                crate::active_resizable::ResizableDescription::new(
                                                    "Pull request file list",
                                                    FILE_LIST_MIN()..FILE_LIST_MAX(),
                                                ),
                                                self.file_list(&preview, cx),
                                            )
                                            .key_context("PullRequestFileList")
                                            .on_action(cx.listener(
                                                |this, _: &SelectNextFile, _, cx| {
                                                    this.select_relative(1, cx)
                                                },
                                            ))
                                            .on_action(cx.listener(
                                                |this, _: &SelectPreviousFile, _, cx| {
                                                    this.select_relative(-1, cx)
                                                },
                                            ))
                                            .on_action(cx.listener(
                                                |this, _: &ExtendSelectionDown, _, cx| {
                                                    this.select_relative(1, cx)
                                                },
                                            ))
                                            .on_action(cx.listener(
                                                |this, _: &ExtendSelectionUp, _, cx| {
                                                    this.select_relative(-1, cx)
                                                },
                                            ))
                                            .on_action(cx.listener(
                                                |this, _: &SelectPageUp, _, cx| {
                                                    let page = crate::filter_list::page_rows_in(
                                                        &this.file_scroll,
                                                    );
                                                    this.select_relative(-page, cx)
                                                },
                                            ))
                                            .on_action(cx.listener(
                                                |this, _: &SelectPageDown, _, cx| {
                                                    let page = crate::filter_list::page_rows_in(
                                                        &this.file_scroll,
                                                    );
                                                    this.select_relative(page, cx)
                                                },
                                            ))
                                            .on_action(cx.listener(
                                                |this, _: &ExtendSelectionPageUp, _, cx| {
                                                    let page = crate::filter_list::page_rows_in(
                                                        &this.file_scroll,
                                                    );
                                                    this.select_relative(-page, cx)
                                                },
                                            ))
                                            .on_action(cx.listener(
                                                |this, _: &ExtendSelectionPageDown, _, cx| {
                                                    let page = crate::filter_list::page_rows_in(
                                                        &this.file_scroll,
                                                    );
                                                    this.select_relative(page, cx)
                                                },
                                            ))
                                            .on_action(cx.listener(
                                                |this, _: &SelectFirstFile, _, cx| {
                                                    this.select_edge(false, cx)
                                                },
                                            ))
                                            .on_action(cx.listener(
                                                |this, _: &SelectLastFile, _, cx| {
                                                    this.select_edge(true, cx)
                                                },
                                            )),
                                        ),
                                )
                                .child(
                                    resizable_panel().child(
                                        div()
                                            .size_full()
                                            .flex()
                                            .flex_col()
                                            .min_h_0()
                                            .child(self.diff.clone()),
                                    ),
                                ),
                        ),
                )
                .into_any_element()
        };
        let close = cx.listener(|this, _, _, cx| this.close(cx));
        let compare_only = preview.compare_only;
        let title = if compare_only {
            mac_or("Compare Branches", "Compare branches")
        } else {
            mac_or("Open a Pull Request", "Open a pull request")
        };
        let preview_for_submit = preview.clone();
        let ok_label = if has_pr {
            mac_or("View Pull Request", "View pull request")
        } else {
            mac_or("Create Pull Request", "Create pull request")
        };
        let ok_title = format!(
            "{} pull request on GitHub{}.",
            if has_pr { "View" } else { "Create" },
            if enterprise { " Enterprise" } else { "" }
        );
        let dialog = deferred(
            anchored().position(crate::theme::page_origin()).child(
                div()
                    .id("open-pull-request")
                    // modal: nothing underneath takes hover, clicks or wheel
                    .occlude()
                    .child(crate::widgets::touch_drag_occluder())
                    .w(viewport.width)
                    .h(viewport.height)
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(t.dialog_backdrop)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.close(cx)),
                    )
                    .child(
                        div()
                            .id("open-pull-request-box")
                            .role(Role::Dialog)
                            .aria_label(title)
                            .child(crate::dialog::window_title(title))
                            .w(viewport.width - DIALOG_MARGIN())
                            .h(viewport.height - DIALOG_MARGIN())
                            .flex()
                            .flex_col()
                            .rounded(BORDER_RADIUS())
                            .bg(t.background)
                            .text_color(t.text)
                            .border_1()
                            .border_color(t.box_border)
                            .shadow(vec![BoxShadow {
                                color: t.shadow,
                                offset: point(zpx(0.), zpx(2.)),
                                blur_radius: css_blur(7.),
                                spread_radius: zpx(0.),
                                inset: false,
                            }])
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(
                                // `OpenPullRequestDialogHeader`
                                div()
                                    .flex_none()
                                    .flex()
                                    .flex_col()
                                    .px(SPACING_DOUBLE())
                                    .pt(zpx(15.))
                                    .pb(SPACING())
                                    .border_b_1()
                                    .border_color(t.box_border)
                                    .child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .text_size(FONT_SIZE_MD())
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .child(title),
                                            )
                                            .child(
                                                div()
                                                    .id("open-pull-request-close")
                                                    .icon_button_label("Close")
                                                    .size(zpx(16.))
                                                    // `close-button` mixin
                                                    .mr(-SPACING())
                                                    .cursor_pointer()
                                                    .on_click(close)
                                                    .child(octicon(Octicon::X, t.text_secondary)),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .mt(SPACING())
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap(zpx(4.))
                                            .text_size(FONT_SIZE())
                                            .child(
                                                // `.base-branch-details`
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .flex()
                                                    .flex_row()
                                                    .items_center()
                                                    .flex_wrap()
                                                    .gap(zpx(4.))
                                                    .when(!compare_only, |d| {
                                                        d.child(format!(
                                                            "Merge {commit_count} commit{} into",
                                                            if commit_count == 1 {
                                                                ""
                                                            } else {
                                                                "s"
                                                            }
                                                        ))
                                                    })
                                                    .when(compare_only, |d| d.child("Comparing"))
                                                    .child(self.base_select(&preview, cx))
                                                    .child(if compare_only {
                                                        "with"
                                                    } else {
                                                        "from"
                                                    })
                                                    .child(code_ref(
                                                        preview.current_branch.clone(),
                                                        cx,
                                                    ))
                                                    .child(if compare_only {
                                                        format!(
                                                            "({commit_count} commit{})",
                                                            if commit_count == 1 {
                                                                ""
                                                            } else {
                                                                "s"
                                                            }
                                                        )
                                                    } else {
                                                        ".".to_string()
                                                    }),
                                            )
                                            .child(
                                                // `.lines-added-deleted`
                                                div()
                                                    .flex_none()
                                                    .flex()
                                                    .flex_row()
                                                    .text_size(FONT_SIZE_SM())
                                                    .child(
                                                        div()
                                                            .text_color(t.color_new)
                                                            .child(format!("{added} added lines")),
                                                    )
                                                    .child(", ")
                                                    .child(
                                                        div().text_color(t.color_deleted).child(
                                                            format!("{deleted} removed lines"),
                                                        ),
                                                    ),
                                            ),
                                    ),
                            )
                            .child(
                                // `.open-pull-request-content`
                                div()
                                    .flex_1()
                                    .min_h(zpx(200.))
                                    .min_h_0()
                                    .p(SPACING())
                                    .flex()
                                    .flex_col()
                                    .child(content),
                            )
                            .child(
                                // footer: merge status + buttons
                                div()
                                    .flex_none()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(SPACING())
                                    .px(SPACING_DOUBLE())
                                    .pb(SPACING_DOUBLE())
                                    .pt(SPACING())
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .child(self.merge_status(preview.merge_status, cx)),
                                    )
                                    .when(compare_only, |d| {
                                        d.child(
                                            button("open-pull-request-close-button", "Close", cx)
                                                .min_w(zpx(120.))
                                                .on_click(
                                                    cx.listener(|this, _, _, cx| this.close(cx)),
                                                ),
                                        )
                                    })
                                    .children(
                                        (!compare_only)
                                            .then(|| {
                                                crate::dialog::ok_cancel_order(vec![
                                                    button(
                                                        "open-pull-request-cancel",
                                                        "Cancel",
                                                        cx,
                                                    )
                                                    .min_w(zpx(120.))
                                                    .on_click(
                                                        cx.listener(|this, _, _, cx| {
                                                            this.close(cx)
                                                        }),
                                                    )
                                                    .into_any_element(),
                                                    primary_button(
                                                        "open-pull-request-ok",
                                                        "",
                                                        ok_disabled,
                                                        cx,
                                                    )
                                                    .min_w(zpx(120.))
                                                    .gap(SPACING_HALF())
                                                    .ghd_tooltip(ok_title)
                                                    .when(has_pr, |d| {
                                                        d.child(octicon(
                                                            Octicon::LinkExternal,
                                                            t.button_text,
                                                        ))
                                                    })
                                                    .child(ok_label)
                                                    .when(!ok_disabled, |d| {
                                                        d.on_click(cx.listener(
                                                            move |this, _, _, cx| {
                                                                this.submit(&preview_for_submit, cx)
                                                            },
                                                        ))
                                                    })
                                                    .into_any_element(),
                                                ])
                                            })
                                            .into_iter()
                                            .flatten(),
                                    ),
                            ),
                    ),
            ),
        )
        .with_priority(20)
        .into_any_element();
        // the base-branch popover sits above the dialog
        let popover = self.base_select_open.then(|| self.base_popover(window, cx));
        div().child(dialog).children(popover).into_any_element()
    }
}

/// The `1203` / `1217` file list of a [`PullRequestPreview`] in `slot`
/// (`PullRequestFilesChanged`'s `FileList`); rows select through
/// [`Dispatcher::select_preview_file`]. `id` prefixes the element ids.
pub(crate) fn range_file_list(
    id: &'static str,
    repo: u64,
    slot: PreviewSlot,
    preview: &PullRequestPreview,
    focused: bool,
    scroll: &ScrollHandle,
    cx: &App,
) -> AnyElement {
    let t = cx.ghd();
    let files: Vec<CommittedFileChange> = preview
        .changeset
        .as_ref()
        .map(|c| c.files.clone())
        .unwrap_or_default();
    let selected = preview.file.clone();
    let hover_bg = t.list_item_hover_background;
    // `1314-code-owners`: a pull request follows its base branch's file
    let owners_base = (slot == PreviewSlot::PullRequest && !preview.compare_only)
        .then(|| preview.base_branch.clone())
        .flatten();
    div()
        .size_full()
        .flex()
        .flex_col()
        .border_r_1()
        .border_color(t.box_border)
        .child(
            div()
                .id(SharedString::from(format!("{id}-rows")))
                .role(Role::List)
                .aria_label("Changed files")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .children(files.into_iter().map(|file| {
                    let is_selected = selected.as_deref() == Some(file.path.as_str());
                    let (icon, color) = status_icon(file.status.kind, t);
                    // `.focus-within .list-item.selected`: the icon
                    // takes the row's colour
                    let color = if is_selected && focused {
                        t.box_selected_active_text
                    } else {
                        color
                    };
                    let path = file.path.clone();
                    let code_owners = owners_base.as_deref().and_then(|base| {
                        crate::code_owners::row_owners(
                            Some(repo),
                            corvene_core::codeowners::CodeOwnersSource::Rev(base),
                            &file.path,
                            cx,
                        )
                    });
                    div()
                        .id(SharedString::from(format!("{id}-{}", file.path)))
                        .a11y_row(
                            format!(
                                "{}, {}",
                                file.path,
                                crate::widgets::status_label(&file.status)
                            ),
                            is_selected,
                        )
                        .w_full()
                        .h(ROW_HEIGHT())
                        .flex_none()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(SPACING_HALF())
                        .px(SPACING())
                        .cursor_pointer()
                        .when(is_selected, |d| {
                            if focused {
                                d.bg(t.box_selected_active_background)
                                    .text_color(t.box_selected_active_text)
                            } else {
                                d.bg(t.box_selected_background)
                                    .text_color(t.box_selected_text)
                            }
                        })
                        .when(!is_selected, move |d| d.hover(move |s| s.bg(hover_bg)))
                        .on_click(move |_, _, cx| {
                            Dispatcher::select_preview_file(repo, slot, path.clone(), cx)
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_row()
                                .text_size(FONT_SIZE())
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_color(match (is_selected, focused) {
                                            (true, true) => t.box_selected_active_text,
                                            (true, false) => t.box_selected_text,
                                            _ => t.text_secondary,
                                        })
                                        .child(crate::format::display_path(file.directory())),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .max_w_full()
                                        .truncate()
                                        .child(file.file_name().to_string()),
                                ),
                        )
                        .when_some(code_owners, |d, (owners, location)| {
                            d.child(crate::code_owners::badge(
                                format!("{id}-{}", file.path),
                                &owners,
                                &location,
                                (is_selected && focused).then_some(t.box_selected_active_text),
                                cx,
                            ))
                        })
                        .child(octicon(icon, color))
                }))
                .with_scrollbar_handle(scroll),
        )
        .into_any_element()
}

/// The files of a [`PullRequestPreview`] in order, and the selected one's index.
pub(crate) fn range_file_order(preview: &PullRequestPreview) -> (Vec<String>, Option<usize>) {
    let order: Vec<String> = preview
        .changeset
        .as_ref()
        .map(|c| c.files.iter().map(|f| f.path.clone()).collect())
        .unwrap_or_default();
    let current = preview
        .file
        .as_ref()
        .and_then(|p| order.iter().position(|o| o == p));
    (order, current)
}
