//! History sidebar - GHD `ui/history/compare.tsx` + `commit-list.tsx` +
//! `commit-list-item.tsx` (`styles/ui/history/_history.scss`,
//! `_commit-list.scss`, `drag-elements/_commit-drag-element.scss`):
//! "Select Branch to Compare…" box, then 50 px commit rows (bold summary;
//! `AvatarStack` + "authors • time" byline, `CommitAttribution`; tag badges). The
//! list is virtualized and pages in `COMMIT_BATCH_SIZE` commits as it
//! scrolls. Commits multi-select
//! with ⌘/⇧-click, drag to squash onto another commit, to reorder (drop
//! between rows) or to cherry-pick onto a branch in the branch foldout, and
//! "Reorder Commit" starts the keyboard insertion mode (↑/↓, ⏎, Esc).
//! A row the current branch has not pushed, or one carrying a tag still to
//! push, gets the arrow-up badge (`localCommitSHAs` / `tagsToPush`; nothing
//! without a remote). Drop tooltips ("Copy to …", "Squash N commits") show
//! under the drag element at once (GHD waits 1.5 s on macOS), in the darwin
//! title-tooltip look on macOS and the bordered base look elsewhere.
//!
//! Deviations (`.docs/deviations.md` › History): the commit menus
//! add Copy Commit Title / Message / URL and Copy SHAs (flag `809`); the
//! list scrolls back to the top when the branch changes (flag `808`); Revert
//! Changes in Commit(s) Without Committing (flag `815`); Push Up to This
//! Commit (flag `816`); a commit with a description gets a mark after its
//! summary (flag `803`); compact rows drop the author line (flag `802`); the
//! tag pill's tooltip lists every tag (flag `806`); Checkout Commit works on
//! the branch tip (flag `817`); a toggle before the compare box lists first
//! parents only (flag `807`); the compare list offers matching tags (flag
//! `825`); pushed tags can be deleted after a confirmation (flag `826`);
//! Cherry-pick Without Committing (flag `820`); Create Patch File(s) (flag
//! `821`); a filter box under the compare box searches the history (flag
//! `886`, `corvene_core::history_filter`); a file's history shows as a
//! removable chip there (flag `887`); the compare view lists the files a
//! merge of the compared branch would leave conflicted (flag `889`); rows
//! label local branch tips and the default branch's remote tip (flag `890`);
//! Edit Commit Message… rewords an unpushed commit (flag `892`); a commit of
//! the compare view's Behind tab can be cherry-picked onto the current
//! branch (flag `893`); the current branch's newest commits can go to a new
//! branch (Create Branch from N Commits…, flag `787`); a graph column draws
//! the branch lanes and a toggle lists every branch's commits (flag `1213`,
//! `crate::commit_graph`).

use std::rc::Rc;

use corvene_core::{
    AppState, Commit, ComparisonMode, Dispatcher, DropTarget, Mergeability,
    MultiCommitOperationKind, Popup,
};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::actions::{
    CompareClear, CompareSelect, ExtendSelectionDown, ExtendSelectionUp, ReorderCancel,
    ReorderConfirm, ReorderMoveDown, ReorderMoveUp, SelectFirstFile, SelectLastFile,
    SelectNextFile, SelectPreviousFile,
};
use crate::branch_list::group_branches;
use crate::context_menu::{ContextMenu, IS_MAC, MenuItem, mac_or};
use crate::icons::{Octicon, octicon};
use crate::relative_time::relative;
use crate::scrollbar::ScrollbarExt;
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::{GhdTooltip, IconButtonA11y, ListRowA11y};
use crate::widgets::{avatar_stack, kbd, primary_button};

/// `RowHeight` in `commit-list.tsx`
#[allow(non_snake_case)]
pub fn COMMIT_ROW_HEIGHT() -> Pixels {
    zpx(50.)
}

/// `802`: summary-only commit rows, 30 px tall.
fn compact_rows(cx: &App) -> bool {
    corvene_core::AppState::try_global(cx).is_some_and(|s| {
        s.read(cx)
            .flags
            .bool(corvene_core::flags::ids::COMPACT_COMMIT_ROWS)
    })
}

/// The commit row height: [`COMMIT_ROW_HEIGHT`], or 30 px for compact rows.
pub fn commit_row_height(cx: &App) -> Pixels {
    if compact_rows(cx) {
        zpx(30.)
    } else {
        COMMIT_ROW_HEIGHT()
    }
}

/// GHD `CommitDragData`: what a commit drag carries (drop targets in the
/// branch foldout and toolbar read it too).
#[derive(Clone, Debug)]
pub struct CommitDrag {
    pub repo: u64,
    /// The dragged selection, click order.
    pub shas: Vec<String>,
    /// The commit the drag started on, rendered in the drag element.
    pub commit: Commit,
}

/// `CommitDragElement`: the row that follows the cursor, with a red count
/// badge for multi-commit drags.
pub struct CommitDragElement {
    drag: CommitDrag,
    state: Entity<AppState>,
    /// Where the pointer grabbed the row; GPUI draws the drag view that far
    /// up and left of the pointer.
    grab: Point<Pixels>,
}

impl CommitDragElement {
    fn new(drag: CommitDrag, grab: Point<Pixels>, cx: &mut Context<Self>) -> Self {
        let state = AppState::global(cx);
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self { drag, state, grab }
    }

    /// `renderDragToolTip`: what a drop would do at the current target.
    fn tooltip(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let t = cx.ghd();
        let target = self.state.read(cx).drag_target.clone()?;
        let mac = cfg!(target_os = "macos");
        let content: AnyElement = match target {
            DropTarget::Branch(name) => div()
                .flex()
                .flex_row()
                // `copyToPlus`: a plus octicon leads off macOS
                .when(!mac, |d| {
                    d.items_center().child(octicon(Octicon::Plus, t.text))
                })
                .child("Copy to")
                .child(
                    div()
                        .ml(SPACING_THIRD())
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(name),
                )
                .into_any_element(),
            DropTarget::Commit => div()
                .child(format!("Squash {} commits", self.drag.shas.len() + 1))
                .into_any_element(),
            DropTarget::InsertionPoint { count } => div()
                .child(format!(
                    "Move {} here",
                    if count == 1 { "commit" } else { "commits" }
                ))
                .into_any_element(),
        };
        if !mac {
            // `.tool-tip-contents` base rule: a bordered box 35 px under the
            // element's bottom, in the page's text
            return Some(
                div()
                    .absolute()
                    .left_0()
                    .bottom(zpx(-35.))
                    .py(SPACING_THIRD())
                    .px(SPACING_HALF())
                    .border_1()
                    .border_color(t.box_border)
                    .bg(t.background)
                    .text_color(t.text)
                    .text_size(FONT_SIZE())
                    .line_height(zpx(18.))
                    .whitespace_nowrap()
                    .child(content)
                    .into_any_element(),
            );
        }
        Some(
            // `.tool-tip-contents` (darwin): title-tooltip look under the box
            div()
                .absolute()
                .left_0()
                .bottom(zpx(-25.))
                .px(SPACING_THIRD())
                .py(zpx(1.))
                .rounded(zpx(1.))
                .bg(t.tooltip_background)
                .text_color(t.tooltip_text)
                .text_size(FONT_SIZE_SM())
                .whitespace_nowrap()
                .shadow(vec![BoxShadow {
                    color: t.shadow,
                    offset: point(zpx(0.), zpx(1.)),
                    blur_radius: css_blur(3.),
                    spread_radius: zpx(0.),
                    inset: false,
                }])
                .child(content)
                .into_any_element(),
        )
    }
}

impl Render for CommitDragElement {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd();
        let count = self.drag.shas.len();
        let tooltip = self.tooltip(cx);
        div()
            .relative()
            .w(zpx(300.))
            .h(commit_row_height(cx))
            .map(|d| {
                if cfg!(target_os = "macos") {
                    d.mt(zpx(22.))
                } else {
                    // GHD `Draggable.verticalOffset` (15 px off macOS): the
                    // element's top left corner 15 px below the pointer
                    d.ml(self.grab.x).mt(self.grab.y + zpx(15.))
                }
            })
            .children(tooltip)
            .child(
                div()
                    .size_full()
                    .bg(t.background)
                    .text_color(t.text)
                    .border_t_1()
                    .border_l_1()
                    .border_color(t.box_border)
                    .shadow(vec![BoxShadow {
                        color: t.box_border,
                        offset: point(zpx(2.), zpx(1.)),
                        blur_radius: css_blur(1.),
                        spread_radius: zpx(0.),
                        inset: false,
                    }])
                    .overflow_hidden()
                    .child(commit_row_contents(
                        &self.drag.commit,
                        t.text,
                        t.text_secondary,
                        None,
                        cx,
                    )),
            )
            .when(count > 1, |d| {
                d.child(
                    div()
                        .absolute()
                        .top(zpx(-22.))
                        .left(zpx(20.))
                        .size(zpx(18.))
                        .rounded_full()
                        .bg(rgb(0xd73a49))
                        .text_color(white())
                        .text_size(FONT_SIZE_SM())
                        .font_weight(FontWeight::MEDIUM)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(count.to_string()),
                )
            })
    }
}

/// Where a dragged commit would land on a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DropHint {
    /// Squash the dragged commits onto this row's commit.
    Squash(usize),
    /// Insert the dragged commits before row `n` (`n == count` = after the last).
    InsertAt(usize),
}

pub struct HistorySidebar {
    state: Entity<AppState>,
    compare: Entity<InputState>,
    /// `886-history-search`: the filter box under the compare box.
    filter: Entity<InputState>,
    list_focus: FocusHandle,
    context_menu: Option<Entity<ContextMenu>>,
    /// Live drop target while a commit drag is over the list.
    drop_hint: Option<DropHint>,
    /// Keyboard reorder mode (`keyboardReorderData`): shas + insertion row.
    reorder: Option<(Vec<String>, usize)>,
    /// Focus edge detection for the compare box (`onTextBoxFocused`).
    compare_was_focused: bool,
    /// Keyboard-focused branch in the compare list (`focusedBranch`).
    focused_branch: Option<String>,
    /// Merge call to action dropdown choice (`selectedOperation`).
    merge_option: MultiCommitOperationKind,
    /// `889-compare-shows-conflicts`: the conflicted file list is open.
    conflicts_expanded: bool,
    list_scroll: UniformListScrollHandle,
    /// Shift+F10 / Menu: the selected commit's row.
    menu_anchor: crate::context_menu::RowMenuAnchor,
    /// Repository and tip (branch name or detached sha) the list last showed;
    /// a change scrolls it back to the top (flag `808`).
    shown_tip: Option<(u64, String)>,
    /// `798-blame`: the last commit picked from a Blame gutter that was
    /// scrolled to (repository, `reveal_commit` count).
    revealed: Option<(u64, u64)>,
    /// `1213-commit-graph`: the lanes of the listed commits, extended as
    /// pages load.
    graph: Rc<std::cell::RefCell<corvene_core::commit_graph::CommitGraph>>,
    /// `1216-recent-activity`: shown instead while Recent Activity is open.
    reflog: Entity<crate::reflog_list::ReflogList>,
    /// `345-issues` / `346-releases`: shown instead while those are open.
    issues: Entity<crate::issues_list::IssuesList>,
    /// `348-pull-request-review`: the pull request's files while under review.
    review: Entity<crate::pull_request_review_list::PullRequestReviewList>,
    releases: Entity<crate::releases_list::ReleasesList>,
    /// `1219-tag-manager`: shown instead while Branch › Tags… is open.
    tags: Entity<crate::tags_list::TagsList>,
}

impl HistorySidebar {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let compare = cx.new(|cx| {
            InputState::new(window, cx).placeholder(mac_or(
                "Select Branch to Compare…",
                "Select branch to compare…",
            ))
        });
        // GHD `compareState.filterText`: the box edits the repository's
        // filter text, and shows it when it changes elsewhere (a
        // comparison, back to the history, another repository)
        cx.subscribe(&compare, |this: &mut Self, input, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change)
                && let Some(id) = this.state.read(cx).selected
            {
                let text = input.read(cx).value().to_string();
                Dispatcher::set_compare_filter_text(id, text, cx);
            }
        })
        .detach();
        crate::windows::observe_state_in(&state, window, cx, |this, state, window, cx| {
            let filter_text = state
                .read(cx)
                .selected_state()
                .map(|rs| rs.compare.filter_text.clone())
                .unwrap_or_default();
            if this.compare.read(cx).value() != filter_text.as_str() {
                this.compare
                    .update(cx, |input, cx| input.set_value(filter_text, window, cx));
            }
        })
        .detach();
        // `886`: the box edits the repository's history filter and follows
        // it when it changes elsewhere (another repository, cleared)
        let filter = cx.new(|cx| {
            InputState::new(window, cx).placeholder(mac_or("Search Commits", "Search commits"))
        });
        cx.subscribe(&filter, |this: &mut Self, input, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change)
                && let Some(id) = this.state.read(cx).selected
            {
                let text = input.read(cx).value().to_string();
                Dispatcher::set_history_filter_text(id, text, cx);
            }
        })
        .detach();
        crate::windows::observe_state_in(&state, window, cx, |this, state, window, cx| {
            let text = state
                .read(cx)
                .selected_state()
                .map(|rs| rs.history_filter.text.clone())
                .unwrap_or_default();
            if this.filter.read(cx).value() != text.as_str() {
                this.filter
                    .update(cx, |input, cx| input.set_value(text, window, cx));
            }
        })
        .detach();
        let list_scroll = UniformListScrollHandle::new();
        let state_for_reflog = state.clone();
        Self {
            state,
            compare,
            filter,
            list_focus: cx.focus_handle().tab_stop(true),
            context_menu: None,
            drop_hint: None,
            reorder: None,
            compare_was_focused: false,
            focused_branch: None,
            merge_option: MultiCommitOperationKind::Merge,
            conflicts_expanded: false,
            menu_anchor: crate::context_menu::RowMenuAnchor::for_uniform_list(&list_scroll),
            list_scroll,
            shown_tip: None,
            revealed: None,
            graph: Default::default(),
            issues: cx.new(|cx| {
                crate::issues_list::IssuesList::new(state_for_reflog.clone(), window, cx)
            }),
            review: cx.new(|cx| {
                crate::pull_request_review_list::PullRequestReviewList::new(
                    state_for_reflog.clone(),
                    cx,
                )
            }),
            releases: cx.new(|cx| {
                crate::releases_list::ReleasesList::new(state_for_reflog.clone(), window, cx)
            }),
            tags: cx
                .new(|cx| crate::tags_list::TagsList::new(state_for_reflog.clone(), window, cx)),
            reflog: cx.new(|cx| crate::reflog_list::ReflogList::new(state_for_reflog, cx)),
        }
    }

    /// `886-history-search`: the filter box under the compare box, in the
    /// compare form's look.
    fn filter_row(
        &self,
        search: bool,
        file: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd().clone();
        let focused = self.filter.read(cx).focus_handle(cx).is_focused(window);
        let id = self.state.read(cx).selected;
        div()
            .id("history-filter-form")
            .key_context("HistoryFilter")
            .on_action(cx.listener(|this, _: &CompareClear, window, cx| {
                if let Some(id) = this.state.read(cx).selected {
                    Dispatcher::set_history_filter_text(id, String::new(), cx);
                }
                this.filter
                    .update(cx, |input, cx| input.set_value("", window, cx));
            }))
            .on_action(cx.listener(|this, _: &SelectNextFile, window, cx| {
                window.focus(&this.list_focus, cx);
            }))
            .flex_none()
            .flex()
            .flex_col()
            .gap(SPACING_HALF())
            .px(SPACING_HALF())
            .pb(SPACING_HALF())
            .bg(t.box_alt_background)
            .border_b_1()
            .border_color(t.box_border)
            .when_some(file, |d, file| {
                // `887`: "History of <file>", × goes back to the whole history
                let name = file.rsplit('/').next().unwrap_or(&file).to_string();
                d.child(
                    div()
                        .id("file-history-chip")
                        .ghd_tooltip(file.clone())
                        .h(zpx(22.))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(SPACING_HALF())
                        .pl(SPACING_HALF())
                        .rounded(BORDER_RADIUS())
                        .border_1()
                        .border_color(t.box_border)
                        .bg(t.background)
                        .text_size(FONT_SIZE())
                        .child(octicon(Octicon::History, t.text_secondary))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(format!("History of {name}")),
                        )
                        .child(
                            div()
                                .id("file-history-clear")
                                .icon_button_label("Show the whole history")
                                .size(zpx(20.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .child(octicon(Octicon::X, t.text))
                                .on_click(move |_, _, cx| {
                                    if let Some(id) = id {
                                        Dispatcher::clear_file_history(id, cx);
                                    }
                                }),
                        ),
                )
            })
            .when(search, |d| d.child(self.filter_box(focused, window, cx)))
            .into_any_element()
    }

    /// `886`: the filter text box itself.
    fn filter_box(&self, focused: bool, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.ghd().clone();
        // `1220-history-search-terms`: committer: and path: too
        let terms = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::HISTORY_SEARCH_TERMS);
        let tooltip = if terms {
            "Words match the message, author or SHA. Narrow with author:name, \
             committer:name, path:file-or-folder, before:date or after:date."
        } else {
            "Words match the message, author or SHA. Narrow with author:name, \
             before:date or after:date."
        };
        div()
            .id("history-filter-box")
            .ghd_tooltip(tooltip)
            .child(
                crate::widgets::filter_text_box(
                    "history-filter",
                    &self.filter,
                    Some(octicon(Octicon::Search, t.text).size(zpx(12.))),
                    window,
                    cx,
                )
                .h(zpx(25.) + 2. * crate::widgets::hairline(window))
                .pl(zpx(7.))
                .gap(zpx(1.))
                .when(!focused, |d| d.border_color(t.box_border)),
            )
            .into_any_element()
    }

    /// Corvene (`603-focus-list-on-section-switch`).
    pub fn list_focus_handle(&self) -> FocusHandle {
        self.list_focus.clone()
    }

    /// Branch › Compare to Branch: focus the compare box, which opens the list.
    pub fn focus_compare(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.compare.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        if let Some(id) = self.state.read(cx).selected {
            Dispatcher::set_compare_branch_list_visible(id, true, cx);
        }
    }

    fn compare_query(&self, cx: &App) -> String {
        self.compare.read(cx).value().trim().to_string()
    }

    /// Branches offered for comparison: everything but the current branch
    /// (`_initializeCompare`), grouped like the branch foldout.
    fn compare_groups(&self, id: u64, cx: &App) -> Vec<crate::branch_list::BranchGroup> {
        let query = self.compare_query(cx);
        let s = self.state.read(cx);
        let Some(rs) = s.repo_states.get(&id) else {
            return Vec::new();
        };
        let Some(info) = rs.info.as_ref() else {
            return Vec::new();
        };
        let current = info.current_branch().map(|b| b.name.clone());
        let branches: Vec<corvene_core::Branch> = info
            .branches
            .iter()
            .filter(|b| Some(&b.name) != current.as_ref())
            .cloned()
            .collect();
        let default = rs
            .default_branch
            .as_deref()
            .filter(|d| Some(*d) != current.as_deref());
        let recent: Vec<String> = rs
            .recent_branches
            .iter()
            .filter(|r| Some(*r) != current.as_ref())
            .cloned()
            .collect();
        let mut groups = group_branches(
            &branches,
            default,
            &recent,
            &query,
            crate::branch_list::sort_by_date(cx),
        );
        // `825`: tags matching the filter, after the branches
        if !query.is_empty() && s.flags.bool(corvene_core::flags::ids::COMPARE_TAGS) {
            let tags: Vec<corvene_core::Branch> = rs
                .compare
                .tags
                .iter()
                .filter(|t| corvene_core::filter::fuzzy_score(&query, t).is_some())
                .map(|t| corvene_core::Branch {
                    name: t.clone(),
                    kind: corvene_core::BranchKind::Local,
                    full_name: format!("refs/tags/{t}"),
                    tip: None,
                    upstream: None,
                    tip_time: None,
                    tip_author: None,
                    remote_name: None,
                })
                .collect();
            if !tags.is_empty() {
                groups.push(crate::branch_list::BranchGroup {
                    title: "Tags",
                    branches: tags,
                });
            }
        }
        groups
    }

    fn compare_branch_names(&self, id: u64, cx: &App) -> Vec<String> {
        self.compare_groups(id, cx)
            .into_iter()
            .flat_map(|g| g.branches.into_iter().map(|b| b.name))
            .collect()
    }

    /// Enter in the compare box (`onBranchFilterKeyDown`).
    fn compare_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.state.read(cx).selected else {
            return;
        };
        let names = self.compare_branch_names(id, cx);
        let pick = self
            .focused_branch
            .clone()
            .filter(|b| names.contains(b))
            .or_else(|| names.first().cloned());
        match pick {
            Some(branch) if !self.compare_query(cx).is_empty() || self.focused_branch.is_some() => {
                self.choose_compare_branch(id, branch, window, cx);
            }
            _ => {
                Dispatcher::exit_compare(id, cx);
                Dispatcher::set_compare_branch_list_visible(id, false, cx);
                window.blur(cx);
            }
        }
    }

    /// Escape / clear (`handleEscape`): back to the history.
    /// GHD `onBranchFilterBlur`: hide the list; an empty filter with nothing
    /// chosen drops back to the plain history.
    fn compare_blur(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.state.read(cx).selected else {
            return;
        };
        self.focused_branch = None;
        Dispatcher::set_compare_branch_list_visible(id, false, cx);
        if self.compare_query(cx).is_empty() {
            Dispatcher::exit_compare(id, cx);
        }
        window.blur(cx);
        cx.notify();
    }

    fn compare_clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.state.read(cx).selected else {
            return;
        };
        self.focused_branch = None;
        self.compare.update(cx, |s, cx| s.set_value("", window, cx));
        Dispatcher::exit_compare(id, cx);
        Dispatcher::set_compare_branch_list_visible(id, false, cx);
        window.blur(cx);
        cx.notify();
    }

    fn choose_compare_branch(
        &mut self,
        id: u64,
        branch: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focused_branch = None;
        self.compare
            .update(cx, |s, cx| s.set_value(branch.clone(), window, cx));
        Dispatcher::compare_to_branch(id, branch, ComparisonMode::Behind, cx);
        window.blur(cx);
        cx.notify();
    }

    fn move_focused_branch(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(id) = self.state.read(cx).selected else {
            return;
        };
        let names = self.compare_branch_names(id, cx);
        if names.is_empty() {
            return;
        }
        let current = self
            .focused_branch
            .as_ref()
            .and_then(|b| names.iter().position(|n| n == b));
        let next = match current {
            Some(ix) => crate::filter_list::wrap_step(ix, delta, names.len()),
            None => 0,
        };
        self.focused_branch = Some(names[next].clone());
        cx.notify();
    }

    /// `CompareBranchListItem` rows inside `BranchList`.
    fn compare_branch_list(&self, id: u64, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let groups = self.compare_groups(id, cx);
        let counts = self
            .state
            .read(cx)
            .repo_states
            .get(&id)
            .map(|r| r.compare.branch_counts.clone())
            .unwrap_or_default();
        if groups.is_empty() {
            return div()
                .flex_1()
                .pt(SPACING_DOUBLE())
                .flex()
                .justify_center()
                .text_size(FONT_SIZE())
                .text_color(t.text_secondary)
                // GHD `NoBranches` without `canCreateNewBranch`
                .child(
                    crate::branch_list::no_branches(false, None)
                        .title()
                        .to_string(),
                )
                .into_any_element();
        }
        let focused = self.focused_branch.clone();
        let weak = cx.weak_entity();
        // `.list-item:hover`: `--list-item-hover-background-color`, text unchanged
        let list_hover = t.list_item_hover_background;
        div()
            .id("compare-branch-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .children(groups.into_iter().map(|group| {
                let counts = counts.clone();
                let focused = focused.clone();
                let weak = weak.clone();
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
                    .children(group.branches.into_iter().map(move |b| {
                        let is_focused = focused.as_deref() == Some(b.name.as_str());
                        let name = b.name.clone();
                        let weak = weak.clone();
                        let ab = counts.get(&b.name).cloned();
                        div()
                            .id(SharedString::from(format!(
                                "compare-branch-{}",
                                b.full_name
                            )))
                            .h(ROW_HEIGHT())
                            .w_full()
                            .flex()
                            .flex_row()
                            .items_center()
                            .px(SPACING())
                            .cursor_pointer()
                            .when(is_focused, |d| {
                                d.bg(t.box_selected_active_background)
                                    .text_color(t.box_selected_active_text)
                            })
                            .when(!is_focused, move |d| d.hover(move |s| s.bg(list_hover)))
                            .on_click(move |_, window, cx| {
                                let name = name.clone();
                                weak.update(cx, |this, cx| {
                                    this.choose_compare_branch(id, name, window, cx)
                                })
                                .ok();
                            })
                            .child(
                                octicon(
                                    if b.full_name.starts_with("refs/tags/") {
                                        Octicon::Tag
                                    } else {
                                        Octicon::GitBranch
                                    },
                                    t.text,
                                )
                                .mr(SPACING_HALF()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(FONT_SIZE())
                                    .child(b.name.clone()),
                            )
                            .when_some(ab, |d, ab| {
                                // `.branch-commit-counter`: behind ↓, ahead ↑
                                let counter = |n: u32, icon: Octicon| {
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .child(n.to_string())
                                        .child(octicon(icon, t.text_secondary).size(zpx(10.)))
                                };
                                d.child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(SPACING_HALF())
                                        .text_size(FONT_SIZE_SM())
                                        .text_color(t.text_secondary)
                                        .child(counter(ab.behind, Octicon::ArrowDown))
                                        .child(counter(ab.ahead, Octicon::ArrowUp)),
                                )
                            })
                    }))
            }))
            .with_scrollbar()
            .into_any_element()
    }

    /// Behind (N) | Ahead (M) tabs (`#compare-view .tab-bar`).
    fn compare_tabs(
        &self,
        id: u64,
        mode: ComparisonMode,
        ahead: u32,
        behind: u32,
        cx: &Context<Self>,
    ) -> Div {
        let t = cx.ghd();
        let tab = |label: String, selected: bool, first: bool, target: ComparisonMode| {
            let hover_bg = t.tab_bar_hover_background;
            div()
                .id(if first {
                    "compare-tab-behind"
                } else {
                    "compare-tab-ahead"
                })
                .flex_1()
                .h(zpx(25.))
                .flex()
                .items_center()
                .justify_center()
                .text_size(FONT_SIZE())
                .border_1()
                .border_color(if selected {
                    t.box_border_accent
                } else {
                    t.box_border
                })
                .when(first, |d| d.rounded_l(BORDER_RADIUS()))
                .when(!first, |d| d.rounded_r(BORDER_RADIUS()).ml(zpx(-1.)))
                .bg(if selected {
                    t.box_selected_active_background
                } else {
                    t.tab_bar_background
                })
                .text_color(if selected {
                    t.box_selected_active_text
                } else {
                    t.text
                })
                .cursor_pointer()
                .when(!selected, move |d| d.hover(move |s| s.bg(hover_bg)))
                .on_click(move |_, _, cx| Dispatcher::set_comparison_mode(id, target, cx))
                .child(label)
        };
        div()
            .flex_none()
            .flex()
            .flex_row()
            .p(SPACING_HALF())
            .border_b_1()
            .border_color(t.box_border)
            .child(tab(
                format!("Behind ({behind})"),
                mode == ComparisonMode::Behind,
                true,
                ComparisonMode::Behind,
            ))
            .child(tab(
                format!("Ahead ({ahead})"),
                mode == ComparisonMode::Ahead,
                false,
                ComparisonMode::Ahead,
            ))
    }

    /// `1218-compare-refs`: the `..` / `...` tabs (with their commit
    /// counts) and the swap button.
    fn ref_compare_tabs(
        &self,
        id: u64,
        range: corvene_core::ref_compare::RefRange,
        base_only: u32,
        head_only: u32,
        cx: &Context<Self>,
    ) -> Div {
        use corvene_core::ref_compare::RefRange;
        let t = cx.ghd();
        let tab = |id_str: &'static str,
                   label: String,
                   tip: &'static str,
                   target: RefRange,
                   first: bool| {
            let selected = range == target;
            let hover_bg = t.tab_bar_hover_background;
            div()
                .id(id_str)
                .a11y_button(label.clone())
                .ghd_tooltip(tip)
                .flex_1()
                .h(zpx(25.))
                .flex()
                .items_center()
                .justify_center()
                .text_size(FONT_SIZE())
                .border_1()
                .border_color(if selected {
                    t.box_border_accent
                } else {
                    t.box_border
                })
                .when(first, |d| d.rounded_l(BORDER_RADIUS()))
                .when(!first, |d| d.rounded_r(BORDER_RADIUS()).ml(zpx(-1.)))
                .bg(if selected {
                    t.box_selected_active_background
                } else {
                    t.tab_bar_background
                })
                .text_color(if selected {
                    t.box_selected_active_text
                } else {
                    t.text
                })
                .cursor_pointer()
                .when(!selected, move |d| d.hover(move |s| s.bg(hover_bg)))
                .on_click(move |_, _, cx| Dispatcher::set_ref_compare_range(id, target, cx))
                .child(label)
        };
        let hover_bg = t.tab_bar_hover_background;
        div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING_HALF())
            .p(SPACING_HALF())
            .border_b_1()
            .border_color(t.box_border)
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_row()
                    .child(tab(
                        "ref-compare-range",
                        format!(".. ({head_only})"),
                        "Commits only the second ref has (base..head)",
                        RefRange::Range,
                        true,
                    ))
                    .child(tab(
                        "ref-compare-symmetric",
                        format!("... ({})", base_only + head_only),
                        "Commits only one of the refs has (base...head)",
                        RefRange::Symmetric,
                        false,
                    )),
            )
            .child(
                div()
                    .id("ref-compare-swap")
                    .icon_button_label("Swap the refs")
                    .flex_none()
                    .size(zpx(25.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(BORDER_RADIUS())
                    .border_1()
                    .border_color(t.box_border)
                    .bg(t.tab_bar_background)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover_bg))
                    .on_click(move |_, _, cx| Dispatcher::swap_compare_refs(id, cx))
                    .child(octicon(Octicon::GitCompare, t.text)),
            )
            // the way back to the commit list (Escape does the same; the
            // empty branch field shows no clear button to find)
            .child(
                div()
                    .id("ref-compare-close")
                    .icon_button_label("Back to History")
                    .flex_none()
                    .size(zpx(25.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(BORDER_RADIUS())
                    .border_1()
                    .border_color(t.box_border)
                    .bg(t.tab_bar_background)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover_bg))
                    .on_click(cx.listener(|this, _, window, cx| this.compare_clear(window, cx)))
                    .child(octicon(Octicon::X, t.text)),
            )
    }

    /// `1218-compare-refs`: the row that shows the combined diff, selected
    /// while it shows.
    fn ref_compare_changed_files(&self, id: u64, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let (selected, files) = {
            let s = self.state.read(cx);
            let rs = s.repo_states.get(&id);
            (
                rs.is_some_and(corvene_core::ref_compare::showing_changes),
                rs.and_then(|rs| rs.ref_compare_changes.as_ref())
                    .and_then(|p| p.changeset.as_ref())
                    .map(|c| c.files.len()),
            )
        };
        let focused = selected;
        let (bg, text, secondary) = if focused {
            (
                t.box_selected_active_background,
                t.box_selected_active_text,
                t.box_selected_active_text,
            )
        } else {
            (t.background, t.text, t.text_secondary)
        };
        let label = mac_or("Changed Files", "Changed files");
        let hover_bg = t.list_item_hover_background;
        div()
            .id("ref-compare-changed-files")
            .a11y_row(label, selected)
            .flex_none()
            .h(ROW_HEIGHT())
            .px(SPACING())
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING_HALF())
            .border_b_1()
            .border_color(t.box_border)
            .bg(bg)
            .text_color(text)
            .text_size(FONT_SIZE())
            .cursor_pointer()
            .when(!selected, move |d| d.hover(move |s| s.bg(hover_bg)))
            .on_click(move |_, _, cx| Dispatcher::show_ref_compare_changes(id, cx))
            .child(octicon(Octicon::FileDiff, secondary))
            .child(div().flex_1().child(label))
            .children(files.map(|n| {
                div()
                    .flex_none()
                    .text_color(secondary)
                    .child(format!("{n} {}", if n == 1 { "file" } else { "files" }))
            }))
            .into_any_element()
    }

    /// `889-compare-shows-conflicts`: "N conflicting files", opening into
    /// their paths.
    fn compare_conflicts(&self, id: u64, cx: &mut Context<Self>) -> Option<AnyElement> {
        let files = {
            let s = self.state.read(cx);
            if !s
                .flags
                .bool(corvene_core::flags::ids::COMPARE_SHOWS_CONFLICTS)
            {
                return None;
            }
            s.repo_states.get(&id)?.compare.conflicted_files.clone()
        };
        if files.is_empty() {
            return None;
        }
        let t = cx.ghd().clone();
        let expanded = self.conflicts_expanded;
        let n = files.len();
        Some(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .border_b_1()
                .border_color(t.box_border)
                .text_size(FONT_SIZE())
                .child(
                    div()
                        .id("compare-conflicts")
                        .a11y_button(format!(
                            "{n} conflicting {}",
                            if n == 1 { "file" } else { "files" }
                        ))
                        .h(ROW_HEIGHT())
                        .px(SPACING())
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(SPACING_HALF())
                        .cursor_pointer()
                        .hover(|s| s.bg(t.list_item_hover_background))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.conflicts_expanded = !this.conflicts_expanded;
                            cx.notify();
                        }))
                        .child(octicon(Octicon::Alert, t.color_modified))
                        .child(div().flex_1().child(format!(
                            "{n} conflicting {}",
                            if n == 1 { "file" } else { "files" }
                        )))
                        .child(octicon(
                            if expanded {
                                Octicon::ChevronDown
                            } else {
                                Octicon::ChevronRight
                            },
                            t.text_secondary,
                        )),
                )
                .when(expanded, |d| {
                    d.child(
                        div()
                            .id("compare-conflict-files")
                            .max_h(zpx(150.))
                            .overflow_y_scroll()
                            .pb(SPACING_HALF())
                            .children(files.into_iter().map(|file| {
                                div()
                                    .pl(SPACING_DOUBLE() + SPACING())
                                    .pr(SPACING())
                                    .h(zpx(22.))
                                    .flex()
                                    .items_center()
                                    .text_color(t.text_secondary)
                                    .child(div().min_w_0().truncate().child(file))
                            })),
                    )
                })
                .into_any_element(),
        )
    }

    /// `1203-compare-branch-files`: "Show Changed Files" opens the changes
    /// of the current branch since it diverged from the compared one.
    fn compare_changed_files(
        &self,
        id: u64,
        branch: &str,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        if !self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::COMPARE_BRANCH_FILES)
        {
            return None;
        }
        let t = cx.ghd();
        let label = mac_or("Show Changed Files", "Show changed files");
        let branch = branch.to_string();
        Some(
            div()
                .id("compare-changed-files")
                .a11y_button(label)
                .flex_none()
                .h(ROW_HEIGHT())
                .px(SPACING())
                .flex()
                .flex_row()
                .items_center()
                .gap(SPACING_HALF())
                .border_b_1()
                .border_color(t.box_border)
                .text_size(FONT_SIZE())
                .cursor_pointer()
                .hover(|s| s.bg(t.list_item_hover_background))
                .on_click(move |_, _, cx| Dispatcher::compare_branch_files(id, branch.clone(), cx))
                .child(octicon(Octicon::FileDiff, t.text_secondary))
                .child(div().flex_1().child(label))
                .into_any_element(),
        )
    }

    /// `MergeCallToActionWithConflicts` (`.merge-cta`).
    fn merge_cta(
        &self,
        id: u64,
        branch: &str,
        behind: u32,
        merge_status: Option<Mergeability>,
        cx: &Context<Self>,
    ) -> Div {
        let t = cx.ghd();
        let current = self
            .state
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|r| r.info.as_ref())
            .and_then(|i| i.current_branch())
            .map(|b| b.name.clone())
            .unwrap_or_default();
        let op = self.merge_option;
        let bold = |text: String| div().font_weight(FontWeight::SEMIBOLD).child(text);
        let row = || div().flex().flex_row().flex_wrap().items_center();
        let plural = if behind == 1 { "commit" } else { "commits" };
        let rebase_message = || {
            row()
                .child("This will update\u{a0}")
                .child(bold(current.clone()))
                .child("\u{a0}by applying its commits on top of\u{a0}")
                .child(bold(branch.to_string()))
                .into_any_element()
        };
        let (icon, color, message): (Octicon, Hsla, Option<AnyElement>) = if behind == 0 {
            (Octicon::Check, t.color_new, None)
        } else if op == MultiCommitOperationKind::Rebase {
            (Octicon::Check, t.color_new, Some(rebase_message()))
        } else {
            match merge_status {
                None => (
                    Octicon::DotFill,
                    t.color_modified,
                    Some(
                        div()
                            .child(format!(
                                "Checking for ability to {} automatically…",
                                op.lower()
                            ))
                            .into_any_element(),
                    ),
                ),
                Some(Mergeability::Invalid) => (
                    Octicon::X,
                    t.color_deleted,
                    Some(
                        div()
                            .child("Unable to merge unrelated histories in this repository")
                            .into_any_element(),
                    ),
                ),
                Some(Mergeability::Conflicts(n)) => (
                    Octicon::Alert,
                    t.color_modified,
                    Some(
                        row()
                            .child("There will be\u{a0}")
                            .child(bold(format!(
                                "{n} conflicted {}",
                                if n == 1 { "file" } else { "files" }
                            )))
                            .child("\u{a0}when merging\u{a0}")
                            .child(bold(branch.to_string()))
                            .child("\u{a0}into\u{a0}")
                            .child(bold(current.clone()))
                            .into_any_element(),
                    ),
                ),
                Some(Mergeability::Clean) => (
                    Octicon::Check,
                    t.color_new,
                    Some(
                        row()
                            .child("This will merge\u{a0}")
                            .child(bold(format!("{behind} {plural}")))
                            .child("\u{a0}from\u{a0}")
                            .child(bold(branch.to_string()))
                            .child("\u{a0}into\u{a0}")
                            .child(bold(current.clone()))
                            .into_any_element(),
                    ),
                ),
            }
        };
        // `838-no-merge-while-conflicted`: no second merge while conflicted
        let disabled = behind == 0
            || merge_status == Some(Mergeability::Invalid)
            || Dispatcher::merge_blocked_by_conflicts(id, cx);
        let label = match op {
            MultiCommitOperationKind::Squash => "Squash and merge",
            MultiCommitOperationKind::Rebase => "Rebase",
            _ => "Create a merge commit",
        };
        let weak = cx.weak_entity();
        div()
            .flex_none()
            .flex()
            .flex_col()
            .p(SPACING())
            .border_t_1()
            .border_color(t.box_border)
            .text_size(FONT_SIZE_SM())
            .when_some(message, |d, message| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .mb(SPACING())
                        .child(
                            div()
                                .relative()
                                .w_full()
                                .h(zpx(20.))
                                .flex()
                                .justify_center()
                                .child(
                                    div()
                                        .absolute()
                                        .left_0()
                                        .right_0()
                                        .top(zpx(10.))
                                        .h(zpx(1.))
                                        .bg(t.box_border),
                                )
                                .child(
                                    div()
                                        .px(SPACING_HALF())
                                        .bg(t.background)
                                        .child(octicon(icon, color)),
                                ),
                        )
                        .child(
                            div()
                                .mt(SPACING_HALF())
                                .text_center()
                                .text_color(t.text_secondary)
                                .child(message),
                        ),
                )
            })
            .child(
                // `DropdownSelectButton`
                div()
                    .flex()
                    .flex_row()
                    .child(
                        primary_button("compare-merge", label, disabled, cx)
                            .flex_1()
                            .rounded_tr(zpx(0.))
                            .rounded_br(zpx(0.))
                            .on_click(move |_, _, cx| {
                                if !disabled {
                                    Dispatcher::compare_merge_action(id, op, cx);
                                }
                            }),
                    )
                    .child(
                        primary_button("compare-merge-options", "", disabled, cx)
                            .px(SPACING_HALF())
                            .rounded_tl(zpx(0.))
                            .rounded_bl(zpx(0.))
                            .ml(zpx(1.))
                            .child(octicon(Octicon::TriangleDown, white()))
                            .on_click(move |ev: &ClickEvent, window, cx| {
                                if disabled {
                                    return;
                                }
                                let position = ev.mouse_position().unwrap_or_default();
                                let options = [
                                    (MultiCommitOperationKind::Merge, "Create a merge commit"),
                                    (MultiCommitOperationKind::Squash, "Squash and merge"),
                                    (MultiCommitOperationKind::Rebase, "Rebase"),
                                ];
                                let items: Vec<MenuItem> = options
                                    .iter()
                                    .map(|(kind, label)| {
                                        let kind = *kind;
                                        let weak = weak.clone();
                                        MenuItem::checkbox(*label, kind == op, move |_, cx| {
                                            weak.update(cx, |this, cx| {
                                                this.merge_option = kind;
                                                cx.notify();
                                            })
                                            .ok();
                                        })
                                    })
                                    .collect();
                                crate::native_menu::show_context_menu(items, position, window, cx);
                            }),
                    ),
            )
    }

    fn open_menu(
        &mut self,
        items: Vec<MenuItem>,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // GHD's Electron `Menu.popup`: an NSMenu on macOS, a views menu on Linux
        crate::native_menu::show_context_menu(items, position, window, cx);
    }

    fn selection(&self, id: u64, cx: &App) -> Vec<String> {
        self.state
            .read(cx)
            .repo_states
            .get(&id)
            .map(|r| r.selected_commits.clone())
            .unwrap_or_default()
    }

    fn mco_in_progress(&self, id: u64, cx: &App) -> bool {
        self.state
            .read(cx)
            .repo_states
            .get(&id)
            .is_some_and(|r| r.mco.is_some())
    }

    /// `onRowContextMenu`
    fn open_row_menu(
        &mut self,
        commit: Commit,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.reorder.is_some() {
            return;
        }
        let Some(id) = self.state.read(cx).selected else {
            return;
        };
        let selection = self.selection(id, cx);
        if selection.len() > 1 && selection.contains(&commit.sha) {
            self.open_multi_commit_menu(id, commit, selection, position, window, cx);
        } else {
            self.open_commit_menu(id, commit, position, window, cx);
        }
    }

    /// `getContextMenuMultipleCommits`
    fn open_multi_commit_menu(
        &mut self,
        id: u64,
        commit: Commit,
        selection: Vec<String>,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let count = selection.len();
        let busy = self.mco_in_progress(id, cx);
        // `886`: squashing and reordering need the plain list's neighbours
        let comparing = self.state.read(cx).repo_states.get(&id).is_some_and(|r| {
            r.compare.is_comparing() || r.history_filter.is_active() || r.showing_all_branches()
        });
        let (copy_items, revert_no_commit, pick_no_commit, patches) = {
            let flags = &self.state.read(cx).flags;
            (
                flags.bool(corvene_core::flags::ids::HISTORY_COPY_ITEMS),
                flags.bool(corvene_core::flags::ids::REVERT_WITHOUT_COMMITTING),
                flags.bool(corvene_core::flags::ids::CHERRY_PICK_WITHOUT_COMMITTING),
                flags.bool(corvene_core::flags::ids::CREATE_PATCH_FILES),
            )
        };
        // `809`: newest first, whatever the click order
        let shas_text = {
            let s = self.state.read(cx);
            let mut shas = selection.clone();
            if let Some(rs) = s.repo_states.get(&id) {
                shas.sort_by_key(|sha| {
                    rs.visible_commits()
                        .iter()
                        .position(|c| &c.sha == sha)
                        .unwrap_or(usize::MAX)
                });
            }
            shas.join("\n")
        };
        let weak = cx.weak_entity();
        let (s1, s2, s3, s4) = (
            selection.clone(),
            selection.clone(),
            selection.clone(),
            selection.clone(),
        );
        let (s5, s6, s7) = (selection.clone(), selection.clone(), selection);
        let onto = commit.sha.clone();
        let mut items = vec![
            MenuItem::new(
                if IS_MAC {
                    format!("Cherry-pick {count} Commits…")
                } else {
                    format!("Cherry-pick {count} commits…")
                },
                move |_, cx| Dispatcher::start_cherry_pick_flow(id, s1.clone(), cx),
            )
            .enabled(!busy),
            MenuItem::new(
                if IS_MAC {
                    format!("Squash {count} Commits…")
                } else {
                    format!("Squash {count} commits…")
                },
                move |_, cx| Dispatcher::request_squash(id, s2.clone(), onto.clone(), cx),
            )
            .enabled(!busy && !comparing),
            MenuItem::new(
                if IS_MAC {
                    format!("Reorder {count} Commits…")
                } else {
                    format!("Reorder {count} commits…")
                },
                move |_, cx| {
                    weak.update(cx, |this, cx| {
                        this.start_keyboard_reorder(id, s3.clone(), cx)
                    })
                    .ok();
                },
            )
            .enabled(!busy && !comparing),
        ];
        if revert_no_commit {
            // `815`: newest first, staged, not committed
            items.push(
                MenuItem::new(
                    if IS_MAC {
                        format!("Revert Changes in {count} Commits Without Committing")
                    } else {
                        format!("Revert changes in {count} commits without committing")
                    },
                    move |_, cx| Dispatcher::revert_commits_without_committing(id, s4.clone(), cx),
                )
                .enabled(!busy && !comparing),
            );
        }
        if pick_no_commit {
            // `820`: onto the current branch, staged, not committed
            items.push(
                MenuItem::new(
                    if IS_MAC {
                        format!("Cherry-pick {count} Commits Without Committing")
                    } else {
                        format!("Cherry-pick {count} commits without committing")
                    },
                    move |_, cx| Dispatcher::cherry_pick_without_committing(id, s5.clone(), cx),
                )
                .enabled(!busy),
            );
        }
        if patches {
            // `821`
            items.push(MenuItem::new(
                if IS_MAC {
                    format!("Create {count} Patch Files…")
                } else {
                    format!("Create {count} patch files…")
                },
                move |_, cx| create_patch_files(id, s6.clone(), cx),
            ));
        }
        // `787-commit-to-new-branch`: the current branch's newest commits
        let from_commits = {
            let s = self.state.read(cx);
            s.flags
                .bool(corvene_core::flags::ids::COMMIT_TO_NEW_BRANCH)
                .then(|| {
                    let rs = s.repo_states.get(&id)?;
                    corvene_core::new_branch_flows::from_commits_plan(rs, &s7)
                })
                .flatten()
        };
        if let Some(plan) = from_commits {
            items.push(MenuItem::separator());
            items.push(
                MenuItem::new(
                    if IS_MAC {
                        format!("Create Branch from {count} Commits…")
                    } else {
                        format!("Create branch from {count} commits…")
                    },
                    move |_, cx| {
                        Dispatcher::show_popup(
                            Popup::CreateBranchFromCommits {
                                repo: id,
                                plan: plan.clone(),
                            },
                            cx,
                        )
                    },
                )
                .enabled(!busy),
            );
        }
        if copy_items {
            items.push(MenuItem::separator());
            items.push(MenuItem::new("Copy SHAs", move |_, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(shas_text.clone()))
            }));
        }
        self.open_menu(items, position, window, cx);
    }

    /// GHD `getContextMenuForSingleCommit`.
    fn open_commit_menu(
        &mut self,
        id: u64,
        commit: Commit,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (html_url, is_head, busy, copy_items, revert_no_commit, unpushed, checkout_head) = {
            let s = self.state.read(cx);
            let rs = s.repo_states.get(&id);
            // `883-unpublished-commit-links`: no remote has the commit, so
            // there is nothing to view on GitHub
            let unpublished = rs
                .and_then(|r| r.unpublished_commits.as_ref())
                .is_some_and(|shas| shas.contains(&commit.sha));
            let html_url = s
                .repository(id)
                .and_then(|r| r.github.as_ref())
                .filter(|_| !unpublished)
                .map(|g| g.html_url.clone());
            let comparing = rs.is_some_and(|r| r.compare.is_comparing());
            // `1212-bisect`: History lists from the bad commit, not HEAD
            let bisecting = rs.is_some_and(|r| corvene_core::bisect::history_tip(s, r).is_some());
            let is_head = !comparing
                && !bisecting
                && rs
                    .and_then(|rs| rs.commits.first())
                    .map(|c| c.sha == commit.sha)
                    .unwrap_or(false);
            (
                html_url,
                is_head,
                rs.is_some_and(|r| r.mco.is_some()) || comparing,
                s.flags.bool(corvene_core::flags::ids::HISTORY_COPY_ITEMS),
                s.flags
                    .bool(corvene_core::flags::ids::REVERT_WITHOUT_COMMITTING),
                // `816`: one of the current branch's commits its upstream lacks
                s.flags
                    .bool(corvene_core::flags::ids::PUSH_UP_TO_COMMIT)
                    .then(|| {
                        let tracked = rs
                            .and_then(|r| r.info.as_ref())
                            .and_then(|i| i.current_branch())
                            .is_some_and(|b| b.upstream.is_some());
                        let ahead = rs
                            .filter(|_| tracked)
                            .and_then(|r| r.ahead_behind)
                            .map_or(0, |ab| ab.ahead as usize);
                        !comparing
                            && rs
                                .and_then(|r| r.commits.iter().position(|c| c.sha == commit.sha))
                                .is_some_and(|ix| ix < ahead)
                    }),
                // `817`: the branch tip can be checked out (detaching HEAD)
                s.flags.bool(corvene_core::flags::ids::CHECKOUT_HEAD_COMMIT)
                    && rs
                        .and_then(|r| r.info.as_ref())
                        .and_then(|i| i.current_branch())
                        .is_some(),
            )
        };
        // `886`: no reordering among filtered rows (nor all branches', `1213`)
        let filtering = self
            .state
            .read(cx)
            .repo_states
            .get(&id)
            .is_some_and(|r| r.history_filter.is_active() || r.showing_all_branches());
        let reset_modes = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::RESET_MODES);
        // `892`: an unpushed non-merge commit with no merge above it
        let edit_message = {
            let s = self.state.read(cx);
            s.flags
                .bool(corvene_core::flags::ids::EDIT_COMMIT_MESSAGE)
                .then(|| {
                    let rs = s.repo_states.get(&id)?;
                    let ix = rs.commits.iter().position(|c| c.sha == commit.sha)?;
                    let tracked = rs
                        .info
                        .as_ref()
                        .and_then(|i| i.current_branch())
                        .is_some_and(|b| b.upstream.is_some());
                    let unpushed = if tracked {
                        ix < rs.ahead_behind.map_or(0, |ab| ab.ahead as usize)
                    } else {
                        rs.unpublished_commits
                            .as_ref()
                            .is_some_and(|shas| shas.contains(&commit.sha))
                    };
                    Some(
                        unpushed
                            && commit.parents.len() <= 1
                            && !rs.commits[..ix].iter().any(|c| c.parents.len() > 1),
                    )
                })
                .map(|enabled| enabled.unwrap_or(false))
        };
        Dispatcher::select_commit(id, commit.sha.clone(), cx);
        let sha = commit.sha.clone();
        let weak = cx.weak_entity();
        let mut items = Vec::new();
        if is_head {
            items.push(MenuItem::new(
                mac_or("Amend Commit…", "Amend commit…"),
                {
                    let sha = sha.clone();
                    move |_, cx| Dispatcher::request_start_amending(id, sha.clone(), cx)
                },
            ));
            items.push(MenuItem::new(
                mac_or("Undo Commit…", "Undo commit…"),
                move |_, cx| Dispatcher::request_undo_commit(id, cx),
            ));
        }
        if let Some(enabled) = edit_message {
            items.push(
                MenuItem::new(mac_or("Edit Commit Message…", "Edit commit message…"), {
                    let sha = sha.clone();
                    move |_, cx| Dispatcher::request_edit_commit_message(id, sha.clone(), cx)
                })
                .enabled(enabled && !busy),
            );
        }
        // `888-reset-modes`: Soft / Mixed / Hard instead of GHD's mixed reset
        let reset = if reset_modes {
            let item = |label: &'static str, mode: corvene_git::ResetMode| {
                let sha = sha.clone();
                MenuItem::new(label, move |_, cx| match mode {
                    corvene_git::ResetMode::Soft => {
                        Dispatcher::soft_reset_to_commit(id, sha.clone(), cx)
                    }
                    corvene_git::ResetMode::Mixed => {
                        Dispatcher::request_reset_to_commit(id, sha.clone(), cx)
                    }
                    corvene_git::ResetMode::Hard => {
                        Dispatcher::request_hard_reset_to_commit(id, sha.clone(), cx)
                    }
                })
            };
            MenuItem::submenu(
                mac_or("Reset to Commit", "Reset to commit"),
                vec![
                    item(
                        mac_or("Soft (Keep Changes Staged)", "Soft (keep changes staged)"),
                        corvene_git::ResetMode::Soft,
                    ),
                    item(
                        mac_or(
                            "Mixed (Keep Changes Unstaged)…",
                            "Mixed (keep changes unstaged)…",
                        ),
                        corvene_git::ResetMode::Mixed,
                    ),
                    item(
                        mac_or("Hard (Discard Changes)…", "Hard (discard changes)…"),
                        corvene_git::ResetMode::Hard,
                    ),
                ],
            )
            .enabled(!is_head)
        } else {
            MenuItem::new(mac_or("Reset to Commit…", "Reset to commit…"), {
                let sha = sha.clone();
                move |_, cx| Dispatcher::request_reset_to_commit(id, sha.clone(), cx)
            })
            .enabled(!is_head)
        };
        items.extend([
            reset,
            MenuItem::new(mac_or("Checkout Commit", "Checkout commit"), {
                let sha = sha.clone();
                move |_, cx| Dispatcher::request_checkout_commit(id, sha.clone(), cx)
            })
            .enabled(!is_head || checkout_head),
            MenuItem::new(mac_or("Reorder Commit", "Reorder commit"), {
                let sha = sha.clone();
                move |_, cx| {
                    weak.update(cx, |this, cx| {
                        this.start_keyboard_reorder(id, vec![sha.clone()], cx)
                    })
                    .ok();
                }
            })
            .enabled(!busy && !filtering),
            MenuItem::new(
                mac_or("Revert Changes in Commit", "Revert changes in commit"),
                {
                    let sha = sha.clone();
                    move |_, cx| Dispatcher::revert_commit(id, sha.clone(), cx)
                },
            ),
        ]);
        if revert_no_commit {
            items.push(MenuItem::new(
                mac_or(
                    "Revert Changes in Commit Without Committing",
                    "Revert changes in commit without committing",
                ),
                {
                    let sha = sha.clone();
                    move |_, cx| {
                        Dispatcher::revert_commits_without_committing(id, vec![sha.clone()], cx)
                    }
                },
            ));
        }
        // `1212-bisect`
        if let Some(bisect) = crate::bisect_bar::bisect_menu(id, &sha, self.state.read(cx)) {
            items.push(bisect);
        }
        items.extend([
            MenuItem::separator(),
            MenuItem::new(
                mac_or("Create Branch from Commit", "Create branch from commit"),
                {
                    let sha = sha.clone();
                    move |_, cx| {
                        Dispatcher::show_popup(
                            Popup::CreateBranch {
                                repo: id,
                                target_sha: Some(sha.clone()),
                                initial_name: String::new(),
                            },
                            cx,
                        )
                    }
                },
            ),
            MenuItem::new("Create Tag…", {
                let sha = sha.clone();
                move |_, cx| {
                    Dispatcher::show_popup(
                        Popup::CreateTag {
                            repo: id,
                            sha: sha.clone(),
                        },
                        cx,
                    )
                }
            }),
        ]);
        if !commit.tags.is_empty() {
            items.push(MenuItem::separator());
            // GHD `getDeleteTagsMenuItem`: only tags still in `tagsToPush`
            // (created here, not pushed) can be deleted
            let (unpushed, delete_pushed) = {
                let s = self.state.read(cx);
                (
                    s.repository(id)
                        .map(|r| r.tags_to_push.clone())
                        .unwrap_or_default(),
                    s.flags.bool(corvene_core::flags::ids::DELETE_PUSHED_TAGS),
                )
            };
            // `826`: the others after a confirmation that can include the remote
            let delete = move |tag: &String| {
                let is_unpushed = unpushed.contains(tag);
                let tag = tag.clone();
                (
                    is_unpushed || delete_pushed,
                    move |_: &mut Window, cx: &mut App| {
                        Dispatcher::request_delete_tag(id, tag.clone(), cx)
                    },
                )
            };
            if commit.tags.len() == 1 {
                let tag = commit.tags[0].clone();
                let (enabled, action) = delete(&tag);
                items.push(MenuItem::new(format!("Delete tag {tag}"), action).enabled(enabled));
            } else {
                let entries = commit
                    .tags
                    .iter()
                    .map(|tag| {
                        let (enabled, action) = delete(tag);
                        MenuItem::new(tag.clone(), action).enabled(enabled)
                    })
                    .collect();
                items.push(MenuItem::submenu("Delete tag…", entries));
            }
        }
        // `346-releases`: a release for one of the commit's tags, or a new
        // tag at the commit
        let releases = {
            let s = self.state.read(cx);
            s.flags.bool(corvene_core::flags::ids::RELEASES)
                && s.repository(id).and_then(|r| r.non_fork_github()).is_some()
        };
        if releases {
            let sha = sha.clone();
            let create = move |tag: Option<String>| {
                let sha = sha.clone();
                move |_: &mut Window, cx: &mut App| {
                    Dispatcher::show_create_release(id, tag.clone(), Some(sha.clone()), cx)
                }
            };
            items.push(MenuItem::separator());
            match commit.tags.as_slice() {
                [] => items.push(MenuItem::new(
                    mac_or("Create Release…", "Create release…"),
                    create(None),
                )),
                [tag] => items.push(MenuItem::new(
                    format!(
                        "{} {tag}…",
                        mac_or("Create Release for", "Create release for")
                    ),
                    create(Some(tag.clone())),
                )),
                tags => items.push(MenuItem::submenu(
                    mac_or("Create Release…", "Create release…"),
                    tags.iter()
                        .map(|tag| MenuItem::new(tag.clone(), create(Some(tag.clone()))))
                        .collect(),
                )),
            }
        }
        // `1218-compare-refs`: this commit against the current branch
        if corvene_core::ref_compare::enabled(self.state.read(cx)) {
            let head = current_ref(id, cx);
            let base = commit.short_sha().to_string();
            items.push(MenuItem::separator());
            items.push(MenuItem::new("Compare with…", move |_, cx| {
                Dispatcher::show_compare_refs(id, Some(base.clone()), head.clone(), cx)
            }));
        }
        // `893`: a commit the current branch is behind can come over
        let pick_from_compare = {
            let s = self.state.read(cx);
            s.flags
                .bool(corvene_core::flags::ids::CHERRY_PICK_INTO_CURRENT_BRANCH)
                && s.repo_states.get(&id).is_some_and(|r| {
                    r.mco.is_none()
                        && matches!(
                            r.compare.form,
                            corvene_core::CompareForm::Branch {
                                mode: ComparisonMode::Behind,
                                ..
                            }
                        )
                })
        };
        items.push(
            MenuItem::new(mac_or("Cherry-pick Commit…", "Cherry-pick commit…"), {
                let sha = sha.clone();
                move |_, cx| Dispatcher::start_cherry_pick_flow(id, vec![sha.clone()], cx)
            })
            .enabled(!busy || pick_from_compare),
        );
        let (pick_no_commit, patches) = {
            let flags = &self.state.read(cx).flags;
            (
                flags.bool(corvene_core::flags::ids::CHERRY_PICK_WITHOUT_COMMITTING),
                flags.bool(corvene_core::flags::ids::CREATE_PATCH_FILES),
            )
        };
        if pick_no_commit {
            // `820`: onto the current branch (not the HEAD commit itself)
            items.push(
                MenuItem::new(
                    mac_or(
                        "Cherry-pick Commit Without Committing",
                        "Cherry-pick commit without committing",
                    ),
                    {
                        let sha = sha.clone();
                        move |_, cx| {
                            Dispatcher::cherry_pick_without_committing(id, vec![sha.clone()], cx)
                        }
                    },
                )
                .enabled(!busy && !is_head),
            );
        }
        if patches {
            // `821`
            items.push(MenuItem::new(
                mac_or("Create Patch File…", "Create patch file…"),
                {
                    let sha = sha.clone();
                    move |_, cx| create_patch_files(id, vec![sha.clone()], cx)
                },
            ));
        }
        if let Some(unpushed) = unpushed {
            items.push(
                MenuItem::new(
                    mac_or("Push Up to This Commit", "Push up to this commit"),
                    {
                        let sha = sha.clone();
                        move |_, cx| Dispatcher::push_up_to(id, sha.clone(), cx)
                    },
                )
                .enabled(unpushed && !busy),
            );
        }
        items.extend([
            MenuItem::separator(),
            MenuItem::new("Copy SHA", {
                let sha = sha.clone();
                move |_, cx| cx.write_to_clipboard(ClipboardItem::new_string(sha.clone()))
            }),
        ]);
        let commit_url = html_url.clone().map(|u| format!("{u}/commit/{sha}"));
        if copy_items {
            // `809`: the title, the full message and the GitHub URL
            let title = commit.summary.clone();
            // the body is git's `%b`, ending in a newline
            let message = if commit.body.trim().is_empty() {
                commit.summary.clone()
            } else {
                format!("{}\n\n{}", commit.summary, commit.body.trim_end())
            };
            items.push(MenuItem::new(
                mac_or("Copy Commit Title", "Copy commit title"),
                move |_, cx| cx.write_to_clipboard(ClipboardItem::new_string(title.clone())),
            ));
            items.push(MenuItem::new(
                mac_or("Copy Commit Message", "Copy commit message"),
                move |_, cx| cx.write_to_clipboard(ClipboardItem::new_string(message.clone())),
            ));
            items.push(
                MenuItem::new(mac_or("Copy Commit URL", "Copy commit URL"), {
                    let url = commit_url.clone();
                    move |_, cx| {
                        if let Some(url) = &url {
                            cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                        }
                    }
                })
                .enabled(commit_url.is_some()),
            );
        }
        let tags = commit.tags.join(" ");
        items.push(
            MenuItem::new(
                if commit.tags.len() > 1 {
                    mac_or("Copy Tags", "Copy tags")
                } else {
                    mac_or("Copy Tag", "Copy tag")
                },
                move |_, cx| cx.write_to_clipboard(ClipboardItem::new_string(tags.clone())),
            )
            .enabled(!commit.tags.is_empty()),
        );
        items.push(
            MenuItem::new("View on GitHub", {
                let url = commit_url;
                move |_, cx| {
                    if let Some(url) = &url {
                        Dispatcher::open_url(url, cx);
                    }
                }
            })
            .enabled(html_url.is_some()),
        );
        self.open_menu(items, position, window, cx);
    }

    // ---- keyboard reorder (`onKeyboardReorder`, list keyboard insertion) ----

    fn start_keyboard_reorder(&mut self, id: u64, shas: Vec<String>, cx: &mut Context<Self>) {
        let first = {
            let s = self.state.read(cx);
            s.repo_states
                .get(&id)
                .and_then(|r| {
                    shas.iter()
                        .filter_map(|sha| r.commits.iter().position(|c| &c.sha == sha))
                        .min()
                })
                .unwrap_or(0)
        };
        Dispatcher::select_commits(id, shas.clone(), cx);
        self.reorder = Some((shas, first));
        cx.notify();
    }

    fn cancel_keyboard_reorder(&mut self, cx: &mut Context<Self>) {
        if self.reorder.take().is_some() {
            cx.notify();
        }
    }

    fn move_insertion(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self
            .state
            .read(cx)
            .selected_state()
            .map(|r| r.commits.len())
            .unwrap_or(0);
        if let Some((_, insertion)) = self.reorder.as_mut() {
            let next = (*insertion as isize + delta).clamp(0, count as isize) as usize;
            *insertion = next;
            cx.notify();
        }
    }

    fn confirm_keyboard_reorder(&mut self, cx: &mut Context<Self>) {
        let Some((shas, insertion)) = self.reorder.take() else {
            return;
        };
        cx.notify();
        let Some(id) = self.state.read(cx).selected else {
            return;
        };
        self.drop_insertion(id, shas, insertion, cx);
    }

    /// `onDropDataInsertion`: the dragged commits go right before row
    /// `insertion` (the commit above becomes `beforeCommit`).
    fn drop_insertion(&self, id: u64, shas: Vec<String>, insertion: usize, cx: &mut App) {
        let commits: Vec<String> = self
            .state
            .read(cx)
            .repo_states
            .get(&id)
            .map(|r| r.commits.iter().map(|c| c.sha.clone()).collect())
            .unwrap_or_default();
        if commits.is_empty() || insertion > commits.len() {
            return;
        }
        let mut indexes: Vec<usize> = shas
            .iter()
            .filter_map(|sha| commits.iter().position(|c| c == sha))
            .collect();
        indexes.sort_unstable();
        let contiguous = indexes.windows(2).all(|w| w[1] == w[0] + 1);
        if contiguous && let Some(&first) = indexes.first() {
            let base = insertion.checked_sub(1);
            let above_themselves =
                (base.is_none() && first == 0) || base == Some(first.wrapping_sub(1));
            let within_themselves = base.is_some_and(|b| indexes.contains(&b));
            if above_themselves || within_themselves {
                return;
            }
        }
        let before = insertion.checked_sub(1).map(|i| commits[i].clone());
        Dispatcher::reorder_commits(id, shas, before, false, cx);
    }

    /// `.reorder-commits-hint-popover`
    fn reorder_hint(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.ghd();
        div()
            .absolute()
            .top(SPACING_HALF())
            .left(SPACING_HALF())
            .right(SPACING_HALF())
            .p(SPACING())
            .rounded(BORDER_RADIUS())
            .bg(t.background)
            .border_1()
            .border_color(t.box_border)
            .shadow(vec![BoxShadow {
                color: t.shadow,
                offset: point(zpx(0.), zpx(2.)),
                blur_radius: css_blur(7.),
                spread_radius: zpx(0.),
                inset: false,
            }])
            .text_size(FONT_SIZE())
            .flex()
            .flex_col()
            .gap(SPACING_HALF())
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(mac_or("Reorder Commits", "Reorder commits")),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(zpx(3.))
                    .child("Use")
                    .child(kbd("↑", cx))
                    .child(kbd("↓", cx))
                    .child("to choose a new location."),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(zpx(3.))
                    .child("Press")
                    .child(kbd("⏎", cx))
                    .child("to confirm."),
            )
    }

    fn commit_list(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd();
        let s = self.state.read(cx);
        let Some(id) = s.selected else {
            return div().flex_1().into_any_element();
        };
        let rs = s.repo_states.get(&id);
        let comparing = rs.is_some_and(|r| r.compare.is_comparing());
        let commits: Rc<Vec<Commit>> =
            Rc::new(rs.map(|r| r.visible_commits().clone()).unwrap_or_default());
        let selected: Rc<Vec<String>> =
            Rc::new(rs.map(|r| r.selected_commits.clone()).unwrap_or_default());
        // Shift+F10 / Menu
        let anchor_sha = rs.and_then(|r| r.selected_commit.clone());
        let menu_anchor = self.menu_anchor.clone();
        let highlighted: Rc<Vec<String>> =
            Rc::new(rs.map(|r| r.highlighted_shas.clone()).unwrap_or_default());
        // `886`: the filter's matches page in from its own search
        let filter = rs
            .map(|r| &r.history_filter)
            .filter(|f| !comparing && f.is_active());
        let filtering = filter.is_some();
        // `1213`: every branch's commits page in from their own walk
        let all_branches = rs.and_then(|r| {
            r.showing_all_branches()
                .then_some(r.all_branches.as_ref())
                .flatten()
        });
        let exhausted = comparing
            || match (filter, all_branches) {
                (Some(filter), _) => filter.exhausted,
                (None, Some(all)) => all.exhausted,
                (None, None) => rs.map(|r| r.commits_exhausted).unwrap_or(true),
            };
        let loaded = rs.map(|r| r.info.is_some()).unwrap_or(false);
        let draggable = rs.is_some_and(|r| r.mco.is_none()) && self.reorder.is_none() && !comparing;
        // filtered rows can be dragged onto a branch, not squashed or reordered
        let droppable = draggable && !filtering && all_branches.is_none();
        // `1213`: the graph of the plain list (or all branches)
        let graph =
            (s.flags.bool(corvene_core::flags::ids::COMMIT_GRAPH) && !comparing && !filtering)
                .then(|| {
                    let head = all_branches.and_then(|a| a.tips.first().map(String::as_str));
                    let mut graph = self.graph.borrow_mut();
                    graph.sync(&commits, head, Dispatcher::history_first_parent(s));
                    (
                        self.graph.clone(),
                        crate::commit_graph::graph_columns(graph.width()),
                        crate::commit_graph::palette(cx),
                    )
                });
        let paging_all = all_branches.is_some();
        // GHD `localCommitSHAs` / `tagsToPush`: the unpushed indicator, never
        // without a remote (`isLocalRepository`)
        let has_remote = rs
            .and_then(|r| r.info.as_ref())
            .is_some_and(|i| !i.remotes.is_empty());
        let local: Rc<std::collections::HashSet<String>> = Rc::new(
            rs.filter(|_| has_remote)
                .map(|r| r.local_commits.clone())
                .unwrap_or_default(),
        );
        let tags_to_push: Rc<std::collections::HashSet<String>> = Rc::new(
            s.repository(id)
                .filter(|_| has_remote)
                .map(|r| r.tags_to_push.iter().cloned().collect())
                .unwrap_or_default(),
        );
        // `890`: branch tips label their rows
        let labels: Rc<std::collections::HashMap<String, Vec<String>>> = Rc::new(
            rs.filter(|_| {
                s.flags
                    .bool(corvene_core::flags::ids::HISTORY_BRANCH_LABELS)
            })
            .map(branch_labels)
            .unwrap_or_default(),
        );
        // `1212-bisect`: marked commits and the range still in question
        let bisect = rs.and_then(|r| crate::bisect_bar::BisectRows::of(s, r));
        // `1221-stacked-branch-refs`: in the plain list, other branches'
        // tips among the commits the default branch lacks, and where it begins
        let stacked = rs
            .filter(|_| {
                s.flags.bool(corvene_core::flags::ids::STACKED_BRANCH_REFS)
                    && !comparing
                    && !filtering
                    && all_branches.is_none()
                    && bisect.is_none()
            })
            .and_then(|r| r.stacked_refs.clone());
        // `1218-compare-refs`: `base...head` marks each row with its side
        let sides: Option<(
            SharedString,
            SharedString,
            Rc<std::collections::HashSet<String>>,
        )> = rs.and_then(|r| match r.compare.refs()? {
            (base, head, corvene_core::ref_compare::RefRange::Symmetric) => Some((
                SharedString::from(base.to_string()),
                SharedString::from(head.to_string()),
                Rc::new(r.compare.base_only_shas.clone()),
            )),
            _ => None,
        });
        if commits.is_empty() {
            let compare_loading = rs.is_some_and(|r| r.compare.loading);
            let filter_message = filter.map(|filter| {
                if filter.loading || !filter.searched {
                    String::new()
                } else {
                    "No commits match".to_string()
                }
            });
            let message: String = match rs.map(|r| &r.compare.form) {
                _ if filter_message.is_some() => filter_message.unwrap_or_default(),
                Some(corvene_core::CompareForm::Branch { branch, mode, .. })
                    if !compare_loading =>
                {
                    match mode {
                        ComparisonMode::Ahead => {
                            format!("The compared branch ({branch}) is up to date with your branch")
                        }
                        ComparisonMode::Behind => {
                            format!("Your branch is up to date with the compared branch ({branch})")
                        }
                    }
                }
                // `1218-compare-refs`
                Some(corvene_core::CompareForm::Refs {
                    base, head, range, ..
                }) if !compare_loading => match range {
                    corvene_core::ref_compare::RefRange::Range => {
                        format!("{head} has no commits that {base} does not have")
                    }
                    corvene_core::ref_compare::RefRange::Symmetric => {
                        format!("{base} and {head} have the same commits")
                    }
                },
                _ if loaded && exhausted && !compare_loading => "No history".to_string(),
                _ => String::new(),
            };
            return div()
                .flex_1()
                .flex()
                .items_start()
                .justify_center()
                .p(SPACING_DOUBLE())
                .text_center()
                .text_color(t.text_secondary)
                .child(message)
                .into_any_element();
        }
        // `808`: another branch (or repository) starts at the newest commit
        let tip = rs
            .and_then(|r| r.info.as_ref())
            .map(|info| match &info.tip {
                corvene_core::Tip::Detached { sha } => sha.clone(),
                tip => tip.branch_name().unwrap_or_default().to_string(),
            });
        let scroll_to_top = s
            .flags
            .bool(corvene_core::flags::ids::HISTORY_SCROLLS_TO_TOP_ON_BRANCH_CHANGE);
        if let Some(tip) = tip {
            let key = Some((id, tip));
            if self.shown_tip != key {
                if scroll_to_top && self.shown_tip.is_some() {
                    self.list_scroll.scroll_to_item(0, ScrollStrategy::Top);
                }
                self.shown_tip = key;
            }
        }
        // `798-blame`: a commit picked in the Blame gutter is scrolled to
        if let Some((n, Some(sha))) = rs.map(|r| &r.reveal_commit)
            && self.revealed != Some((id, *n))
            && let Some(ix) = commits.iter().position(|c| &c.sha == sha)
        {
            self.revealed = Some((id, *n));
            self.list_scroll.scroll_to_item(ix, ScrollStrategy::Center);
        }
        let weak = cx.weak_entity();
        let list_focus = self.list_focus.clone();
        let count = commits.len();
        // GHD keeps the active (blue) selection while the row's context menu is open
        let menu_open = self.context_menu.is_some();
        let drop_hint = self.drop_hint;
        let reorder = self.reorder.clone();
        let in_reorder = reorder.is_some();
        div()
            .id("commit-list")
            // GHD `ariaLabel="Commits"` on the list
            .role(Role::List)
            .aria_label("Commits")
            .key_context("HistoryList")
            .on_action(self.menu_anchor.action_handler())
            .track_focus(&self.list_focus)
            .on_action(cx.listener(|this, _: &ReorderMoveUp, _, cx| this.move_insertion(-1, cx)))
            .on_action(cx.listener(|this, _: &ReorderMoveDown, _, cx| this.move_insertion(1, cx)))
            .on_action(
                cx.listener(|this, _: &ReorderConfirm, _, cx| this.confirm_keyboard_reorder(cx)),
            )
            .on_action(
                cx.listener(|this, _: &ReorderCancel, _, cx| this.cancel_keyboard_reorder(cx)),
            )
            .on_action(cx.listener(move |this, _: &SelectNextFile, _, cx| {
                if this.reorder.is_some() {
                    this.move_insertion(1, cx);
                } else {
                    this.step_selection(id, 1, cx);
                }
            }))
            .on_action(cx.listener(move |this, _: &SelectPreviousFile, _, cx| {
                if this.reorder.is_some() {
                    this.move_insertion(-1, cx);
                } else {
                    this.step_selection(id, -1, cx);
                }
            }))
            // ⌘↑ / ⌘↓ (GHD isHomeKey / isEndKey), ⇧↑ / ⇧↓ (`addSelection`)
            .on_action(cx.listener(move |this, _: &SelectFirstFile, _, cx| {
                this.step_selection(id, -(1 << 30), cx)
            }))
            .on_action(cx.listener(move |this, _: &SelectLastFile, _, cx| {
                this.step_selection(id, 1 << 30, cx)
            }))
            .on_action(cx.listener(move |this, _: &ExtendSelectionDown, _, cx| {
                this.extend_selection(id, 1, cx)
            }))
            .on_action(cx.listener(move |this, _: &ExtendSelectionUp, _, cx| {
                this.extend_selection(id, -1, cx)
            }))
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                uniform_list("commit-list-rows", count, move |range, window, cx| {
                    // GHD `onScroll` → `loadNextCommitBatch` near the end of the list
                    if !exhausted && range.end + 20 >= count {
                        if filtering {
                            Dispatcher::load_more_history_filter(id, cx);
                        } else if paging_all {
                            Dispatcher::load_all_branches(id, true, cx);
                        } else {
                            Dispatcher::load_commits(id, true, cx);
                        }
                    }
                    let focused = list_focus.is_focused(window) || menu_open;
                    // `1215-verify-visible-signatures`
                    let verify_signatures = crate::signature_badge::verify_rows(cx);
                    range
                        .map(|ix| {
                            let commit = &commits[ix];
                            let is_selected = selected.contains(&commit.sha);
                            let dimmed =
                                !highlighted.is_empty() && !highlighted.contains(&commit.sha);
                            request_commit_avatars(commit, cx);
                            if verify_signatures {
                                crate::signature_badge::touch(
                                    commit,
                                    corvene_core::signatures::Priority::Row,
                                    cx,
                                );
                            }
                            let insertion_here = match (&reorder, drop_hint) {
                                (Some((_, at)), _) => Some(*at),
                                (None, Some(DropHint::InsertAt(at))) => Some(at),
                                _ => None,
                            };
                            let labelled = labels
                                .get(&commit.sha)
                                .map(Vec::as_slice)
                                .unwrap_or_default();
                            let stack = stacked
                                .as_deref()
                                .and_then(|st| stack_mark(st, &commit.sha, labelled));
                            let row_hint = RowHint {
                                squash_target: drop_hint == Some(DropHint::Squash(ix)),
                                // `1221`: not over the drop's insertion line
                                stack_line: stack.is_some() && ix > 0 && insertion_here != Some(ix),
                                line_above: insertion_here == Some(ix),
                                line_below: insertion_here == Some(ix + 1) && ix + 1 == count,
                                keyboard_selected: in_reorder && is_selected,
                                bisect: bisect.as_ref().map(|b| BisectHint {
                                    mark: b.mark(&commit.sha),
                                    in_range: b.in_range(&commit.sha),
                                    rows: b.clone(),
                                }),
                                side: sides.as_ref().map(|(base, head, base_only)| {
                                    if base_only.contains(&commit.sha) {
                                        (base.clone(), true)
                                    } else {
                                        (head.clone(), false)
                                    }
                                }),
                            };
                            commit_row(
                                id,
                                ix,
                                commit,
                                is_selected,
                                focused && !in_reorder,
                                draggable,
                                droppable,
                                match &stack {
                                    Some((names, tooltip)) => RowBranches {
                                        names,
                                        tooltip: tooltip.as_ref(),
                                    },
                                    None => RowBranches {
                                        names: labelled,
                                        tooltip: None,
                                    },
                                },
                                unpushed_indicator(commit, &local, &tags_to_push),
                                selected.clone(),
                                row_hint,
                                dimmed,
                                weak.clone(),
                                list_focus.clone(),
                                (anchor_sha.as_ref() == Some(&commit.sha)).then_some(&menu_anchor),
                                graph.clone(),
                                cx,
                            )
                        })
                        .collect()
                })
                .flex_1()
                .min_h_0()
                .with_scrollbar_handle(&self.list_scroll),
            )
            .when(in_reorder, |d| d.child(self.reorder_hint(cx)))
            .into_any_element()
    }

    /// ↑/↓ moves a single-row selection (`onSelectedRowChanged`).
    fn step_selection(&self, id: u64, delta: isize, cx: &mut App) {
        let next = {
            let s = self.state.read(cx);
            let Some(rs) = s.repo_states.get(&id) else {
                return;
            };
            let commits = rs.visible_commits();
            let current = rs
                .selected_commit
                .as_ref()
                .and_then(|sha| commits.iter().position(|c| &c.sha == sha));
            // GHD `List.moveSelection` wraps around the ends (Corvene
            // `620-lists-stop-at-ends`: stops there)
            let stop = s.flags.bool(corvene_core::flags::ids::LISTS_STOP_AT_ENDS);
            let ix = match current {
                Some(ix) => crate::filter_list::list_step(ix, delta, commits.len(), stop),
                None => 0,
            };
            commits.get(ix).map(|c| c.sha.clone())
        };
        if let Some(sha) = next {
            Dispatcher::select_commit(id, sha, cx);
        }
    }

    /// ⇧↑ / ⇧↓: extend the multi-selection from its moving end (GHD
    /// `List.addSelection`).
    fn extend_selection(&self, id: u64, delta: isize, cx: &mut App) {
        let next = {
            let s = self.state.read(cx);
            let Some(rs) = s.repo_states.get(&id) else {
                return;
            };
            let commits = rs.visible_commits();
            let end = rs
                .selected_commits
                .last()
                .or(rs.selected_commit.as_ref())
                .and_then(|sha| commits.iter().position(|c| &c.sha == sha));
            let Some(end) = end else { return };
            let ix = (end as isize + delta).clamp(0, commits.len() as isize - 1) as usize;
            commits.get(ix).map(|c| c.sha.clone())
        };
        if let Some(sha) = next {
            Dispatcher::extend_commit_selection(id, sha, cx);
        }
    }

    /// Drag-move over a row: pick squash (middle) or insertion (edges).
    fn update_drop_hint(&mut self, hint: Option<DropHint>, cx: &mut Context<Self>) {
        if self.drop_hint != hint {
            self.drop_hint = hint;
            cx.notify();
        }
    }

    /// Drop on a row: squash or reorder, per the last hint.
    fn drop_on_row(&mut self, id: u64, row: usize, drag: &CommitDrag, cx: &mut Context<Self>) {
        let hint = self.drop_hint.take();
        Dispatcher::set_drag_target(None, cx);
        cx.notify();
        if drag.repo != id {
            return;
        }
        let commits: Vec<String> = self
            .state
            .read(cx)
            .repo_states
            .get(&id)
            .map(|r| r.commits.iter().map(|c| c.sha.clone()).collect())
            .unwrap_or_default();
        match hint {
            Some(DropHint::Squash(target)) if target == row => {
                let Some(onto) = commits.get(row) else { return };
                if drag.shas.iter().all(|s| s == onto) {
                    return;
                }
                Dispatcher::request_squash(id, drag.shas.clone(), onto.clone(), cx);
            }
            Some(DropHint::InsertAt(at)) => self.drop_insertion(id, drag.shas.clone(), at, cx),
            _ => {}
        }
    }
}

/// What GHD `CommitAttribution` (`ui/lib/commit-attribution.tsx`) renders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitAttribution {
    /// The `.commit-attribution-component` text.
    pub text: String,
    /// The `.author` spans' texts (none for "N people").
    pub authors: Vec<String>,
}

/// GHD `CommitAttribution`: one or two authors by name ("Mona", "Mona,
/// Hubot"), more counted ("3 people").
pub fn commit_attribution(avatar_users: &[&corvene_core::AvatarUser]) -> CommitAttribution {
    match avatar_users {
        [one] => CommitAttribution {
            text: one.name.clone(),
            authors: vec![one.name.clone()],
        },
        [first, second] => CommitAttribution {
            text: format!("{}, {}", first.name, second.name),
            authors: vec![first.name.clone(), second.name.clone()],
        },
        users => CommitAttribution {
            text: format!("{} people", users.len()),
            authors: Vec::new(),
        },
    }
}

/// `CommitAttribution` of `commit` in the selected repository
/// (`getAvatarUsersForCommit` with its GitHub repository).
pub(crate) fn commit_attribution_for(commit: &Commit, cx: &App) -> CommitAttribution {
    commit_attribution(&avatar_users_for(commit, cx).iter().collect::<Vec<_>>())
}

/// `getAvatarUsersForCommit` of `commit` with the selected repository's
/// GitHub repository: the author, co-authors and a differing committer.
pub(crate) fn avatar_users_for(commit: &Commit, cx: &App) -> Vec<corvene_core::AvatarUser> {
    let github = AppState::try_global(cx).and_then(|s| {
        let s = s.read(cx);
        s.selected_repository().and_then(|r| r.github.clone())
    });
    corvene_core::get_avatar_users_for_commit(github.as_ref(), commit)
}

/// Fetches the avatars of everyone in `commit`'s `AvatarStack`.
pub(crate) fn request_commit_avatars(commit: &Commit, cx: &mut App) {
    for user in avatar_users_for(commit, cx) {
        Dispatcher::request_avatar_for_email(&user.email, cx);
    }
}

#[derive(Clone, Default)]
struct RowHint {
    squash_target: bool,
    /// `1221-stacked-branch-refs`: the separator above a stacked branch's
    /// tip, or above the row where the default branch begins.
    stack_line: bool,
    line_above: bool,
    line_below: bool,
    keyboard_selected: bool,
    /// `1212-bisect`
    bisect: Option<BisectHint>,
    /// `1218-compare-refs` (`base...head`): the ref only it is on, and
    /// whether that is the base.
    side: Option<(SharedString, bool)>,
}

/// `1212-bisect`: a row's mark and whether it is still in question.
#[derive(Clone)]
struct BisectHint {
    mark: Option<corvene_core::bisect::BisectMark>,
    in_range: bool,
    rows: Rc<crate::bisect_bar::BisectRows>,
}

/// `.commit .info` + tag indicators, shared with the drag element. `badge`
/// overrides the tag pill's (background, text) colours on selected rows.
pub(crate) fn commit_row_contents(
    commit: &Commit,
    text: Hsla,
    secondary: Hsla,
    badge: Option<(Hsla, Hsla)>,
    cx: &App,
) -> Div {
    commit_row_contents_with(
        commit,
        text,
        secondary,
        badge,
        RowBranches::default(),
        None,
        cx,
    )
}

/// GHD `showUnpushedIndicator` / `getUnpushedIndicatorTitle`: the commit is
/// one of the current branch's that no remote has (`localCommitSHAs`), or it
/// carries a tag still to push (`tagsToPush`). The tooltip names which.
/// Nothing shows without a remote (`isLocalRepository`).
fn unpushed_indicator(
    commit: &Commit,
    local: &std::collections::HashSet<String>,
    tags_to_push: &std::collections::HashSet<String>,
) -> Option<SharedString> {
    if local.contains(&commit.sha) {
        return Some("This commit has not been pushed to the remote repository".into());
    }
    let tags = commit
        .tags
        .iter()
        .filter(|t| tags_to_push.contains(*t))
        .count();
    (tags > 0).then(|| {
        SharedString::from(format!(
            "This commit has {tags} tag{} to push",
            if tags > 1 { "s" } else { "" }
        ))
    })
}

/// `890-history-branch-labels`: the branches whose tips label commit rows,
/// by tip sha: local branches but the current one (which is the top row),
/// and the default branch's remote-tracking branch, which shows where the
/// current branch left it (`git log --decorate`).
fn branch_labels(
    rs: &corvene_core::RepositoryState,
) -> std::collections::HashMap<String, Vec<String>> {
    let mut labels: std::collections::HashMap<String, Vec<String>> = Default::default();
    let Some(info) = rs.info.as_ref() else {
        return labels;
    };
    let current = info.current_branch().map(|b| b.full_name.clone());
    let default_upstream = rs.default_branch.as_deref().and_then(|name| {
        info.branches
            .iter()
            .find(|b| b.name == name && b.kind == corvene_core::BranchKind::Local)
            .and_then(|b| b.upstream.clone())
    });
    for b in &info.branches {
        let wanted = match b.kind {
            corvene_core::BranchKind::Local => Some(&b.full_name) != current.as_ref(),
            _ => Some(&b.full_name) == default_upstream.as_ref(),
        };
        if wanted && let Some(tip) = &b.tip {
            labels.entry(tip.clone()).or_default().push(b.name.clone());
        }
    }
    labels
}

/// `890` / `1221`: the branch names labelling a row, and the label's
/// tooltip when it says more than the names.
#[derive(Clone, Copy, Default)]
struct RowBranches<'a> {
    names: &'a [String],
    tooltip: Option<&'a SharedString>,
}

/// `1221-stacked-branch-refs`: a row's branch names (the stacked branches
/// whose tip it is, or the default branch where it begins, then the
/// `890` labels it has besides) and the label's tooltip, when the row is
/// marked.
fn stack_mark(
    st: &corvene_core::stacked_refs::StackedRefs,
    sha: &str,
    labelled: &[String],
) -> Option<(Vec<String>, Option<SharedString>)> {
    let (mut names, tooltip) = match st.tips.get(sha) {
        Some(tips) => (tips.clone(), None),
        None if st.boundary.as_deref() == Some(sha) => {
            // the default branch moved on since this branch left it
            let tooltip = (!st.base_at_boundary).then(|| {
                let mut lines = vec![format!("{} begins here (it has moved on since)", st.base)];
                lines.extend(labelled.iter().filter(|l| **l != st.base).cloned());
                SharedString::from(lines.join("\n"))
            });
            (vec![st.base.clone()], tooltip)
        }
        None => return None,
    };
    for label in labelled {
        if !names.contains(label) {
            names.push(label.clone());
        }
    }
    Some((names, tooltip))
}

/// [`commit_row_contents`] with `890-history-branch-labels`' branch names
/// and the unpushed indicator's tooltip.
fn commit_row_contents_with(
    commit: &Commit,
    text: Hsla,
    secondary: Hsla,
    badge: Option<(Hsla, Hsla)>,
    branches: RowBranches<'_>,
    unpushed: Option<SharedString>,
    cx: &App,
) -> Div {
    let t = cx.ghd();
    let (branches, branches_tooltip) = (branches.names, branches.tooltip);
    let (badge_bg, badge_text) =
        badge.unwrap_or((t.list_item_badge_background, t.list_item_badge_text));
    let summary = if commit.summary.is_empty() {
        "Empty commit message".to_string()
    } else {
        // GHD `RichText`: emoji shortcodes as emoji
        corvene_core::text_tokens::with_emoji(&commit.summary)
    };
    let empty = commit.summary.is_empty();
    // `803`: a mark after the summary when the commit has a description
    let body_mark = !commit.body.trim().is_empty()
        && corvene_core::AppState::try_global(cx).is_some_and(|s| {
            s.read(cx)
                .flags
                .bool(corvene_core::flags::ids::COMMIT_BODY_INDICATOR)
        });
    let compact = compact_rows(cx);
    // `.byline`: `CommitAttribution`, then the relative time
    let users = avatar_users_for(commit, cx);
    let byline = format!(
        "{} • {}",
        commit_attribution(&users.iter().collect::<Vec<_>>()).text,
        relative(commit.author.date())
    );
    div()
        .size_full()
        .flex()
        .flex_row()
        .items_center()
        .pl(SPACING())
        .pr(SPACING() + SPACING_HALF())
        .text_color(text)
        .child(
            // `.info { margin-top: -4px }`: 18 px summary, 3 px gap, the
            // 16.5 px byline in 11 px text after the 20 px avatar stack
            div()
                .flex_1()
                .min_w(zpx(50.))
                .when(!compact, |d| d.mt(zpx(-4.)))
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .child(
                            div()
                                .min_w_0()
                                .text_size(FONT_SIZE())
                                .line_height(zpx(18.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .truncate()
                                .when(empty, |d| d.text_color(secondary))
                                .child(summary),
                        )
                        .when(body_mark, |d| {
                            d.child(
                                div()
                                    .flex_none()
                                    .ml(SPACING_HALF())
                                    .child(octicon(Octicon::KebabHorizontal, secondary)),
                            )
                        }),
                )
                .when(!compact, |d| {
                    d.child(
                        // `.description`: `AvatarStack`, then the byline
                        avatar_stack(
                            "avatar-stack",
                            &users,
                            div()
                                .truncate()
                                .text_size(FONT_SIZE_SM())
                                .line_height(zpx(16.5))
                                .text_color(secondary)
                                .child(byline),
                            cx,
                        )
                        .mt(zpx(3.)),
                    )
                }),
        )
        // `890`: the first branch as an outlined pill, "+N" for more
        .when(!branches.is_empty(), |d| {
            let more = branches.len() - 1;
            d.child(
                div()
                    .id(SharedString::from(format!("branches-{}", commit.sha)))
                    .ghd_tooltip(
                        branches_tooltip
                            .cloned()
                            .unwrap_or_else(|| branches.join("\n").into()),
                    )
                    .ml(SPACING())
                    .h(zpx(16.))
                    .max_w(gpui_kit::relative(0.5))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(zpx(2.))
                    .px(SPACING_HALF())
                    .rounded(BORDER_RADIUS())
                    .border_1()
                    .border_color(badge_bg)
                    .text_color(secondary)
                    .text_size(FONT_SIZE_SM())
                    .line_height(zpx(14.))
                    .child(
                        octicon(Octicon::GitBranch, secondary)
                            .size(zpx(10.))
                            .flex_none(),
                    )
                    .child(div().min_w_0().truncate().child(branches[0].clone()))
                    .when(more > 0, |d| {
                        d.child(div().flex_none().child(format!("+{more}")))
                    }),
            )
        })
        // `.commit-indicators .tag-indicator`: the first tag as a 16 px pill
        // (5 px padding, 6 px radius, no icon); more tags peek out behind it
        // as a 10 px tab (`.tag-indicator-more`), 5 px past the pill and cut
        // off by the pill's 1 px `box-shadow` in the background colour. The
        // tab is absolute: a negative margin pushed the whole pill past the
        // row's end, where the list clipped it
        .when(!commit.tags.is_empty(), |d| {
            let more = commit.tags.len() > 1;
            let pill = div()
                .relative()
                .ml(SPACING())
                .h(zpx(16.))
                .max_w(gpui_kit::relative(0.5))
                .when(more, |d| d.pr(SPACING_HALF()))
                .flex()
                .flex_row()
                .text_color(badge_text)
                .text_size(FONT_SIZE())
                .line_height(zpx(16.))
                .when(more, |d| {
                    d.child(
                        div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .w(SPACING())
                            .h(zpx(16.))
                            .rounded_r(BORDER_RADIUS())
                            .bg(badge_bg),
                    )
                })
                .child(
                    div()
                        .min_w_0()
                        .px(SPACING_HALF())
                        .h(zpx(16.))
                        .rounded(BORDER_RADIUS())
                        .bg(badge_bg)
                        .when(more, |d| {
                            d.shadow(vec![BoxShadow {
                                color: t.background,
                                offset: point(zpx(1.), zpx(0.)),
                                blur_radius: zpx(0.),
                                spread_radius: zpx(0.),
                                inset: false,
                            }])
                        })
                        .truncate()
                        .child(commit.tags[0].clone()),
                );
            // `806`: hovering the pill lists every tag
            if tags_tooltip(cx) {
                d.child(
                    pill.id(SharedString::from(format!("tags-{}", commit.sha)))
                        .ghd_tooltip(commit.tags.join("\n")),
                )
            } else {
                d.child(pill)
            }
        })
        // `.commit-indicators .unpushed-indicator`: a 16 px arrow-up badge
        // (5 px padding, 8 px radius) after the tag pill, 5 px from it and
        // 15 px from the summary when there is none
        .when_some(unpushed, |d, tooltip| {
            d.child(
                div()
                    .id(SharedString::from(format!("unpushed-{}", commit.sha)))
                    .ghd_tooltip(tooltip)
                    .flex_none()
                    .ml(if commit.tags.is_empty() {
                        SPACING() + SPACING_HALF()
                    } else {
                        SPACING_HALF()
                    })
                    .h(zpx(16.))
                    .px(SPACING_HALF())
                    .rounded(zpx(8.))
                    .bg(badge_bg)
                    .flex()
                    .items_center()
                    .child(octicon(Octicon::ArrowUp, badge_text)),
            )
        })
        // `1214-commit-signatures`: a signed commit's shield, last
        .when_some(
            crate::signature_badge::enabled(cx)
                .then(|| {
                    let on_accent = badge
                        .is_some_and(|(bg, _)| bg == t.list_item_selected_active_badge_background);
                    crate::signature_badge::row_icon(commit, on_accent.then_some(text), cx)
                })
                .flatten(),
            |d, icon| d.child(icon),
        )
}

/// `806`: the commit's tags as a tooltip on the tag pill and the details' tag list.
pub(crate) fn tags_tooltip(cx: &App) -> bool {
    corvene_core::AppState::try_global(cx).is_some_and(|s| {
        s.read(cx)
            .flags
            .bool(corvene_core::flags::ids::TAGS_TOOLTIP)
    })
}

/// Flag `807`: a 27 px toggle before the compare box that lists first
/// parents only; accent-coloured with a dot while on (the Changes filter
/// button's `.active` look), disabled while comparing.
fn first_parent_button(on: bool, comparing: bool, cx: &App) -> AnyElement {
    let label = if comparing {
        "First-parent history does not apply to a comparison"
    } else if on {
        "Showing first-parent commits only"
    } else {
        "Show first-parent commits only"
    };
    history_toggle(
        "history-first-parent",
        Octicon::Filter,
        label,
        on,
        comparing,
        |on, cx| Dispatcher::set_history_first_parent(on, cx),
        cx,
    )
}

/// Flag `1213`: the All branches toggle after the first-parent one, in
/// its look.
fn all_branches_button(on: bool, comparing: bool, cx: &App) -> AnyElement {
    let label = if comparing {
        "All branches does not apply to a comparison"
    } else if on {
        "Showing the commits of all branches"
    } else {
        "Show the commits of all branches"
    };
    history_toggle(
        "history-all-branches",
        Octicon::GitMerge,
        label,
        on,
        comparing,
        |on, cx| Dispatcher::set_history_all_branches(on, cx),
        cx,
    )
}

/// A 27 px toggle of the compare form's row.
fn history_toggle(
    id: &'static str,
    icon: Octicon,
    label: &'static str,
    on: bool,
    comparing: bool,
    toggle: fn(bool, &mut App),
    cx: &App,
) -> AnyElement {
    let t = cx.ghd();
    div()
        .id(id)
        .icon_button_label(label)
        .relative()
        .size(zpx(27.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(t.secondary_button_border)
        .rounded(BORDER_RADIUS())
        .bg(t.secondary_button_background)
        .when(comparing, |d| d.opacity(0.6))
        .when(!comparing, |d| {
            d.cursor_pointer().on_click(move |_, _, cx| toggle(!on, cx))
        })
        .child(octicon(
            icon,
            if on {
                t.box_selected_active_background
            } else {
                t.secondary_button_text
            },
        ))
        .when(on, |d| {
            d.child(
                div()
                    .absolute()
                    .top(zpx(3.))
                    .right(zpx(3.))
                    .size(zpx(5.))
                    .rounded_full()
                    .bg(t.box_selected_active_background),
            )
        })
        .into_any_element()
}

/// `1213`: the graph, its column count and lane colours for a row.
type GraphColumn = (
    Rc<std::cell::RefCell<corvene_core::commit_graph::CommitGraph>>,
    usize,
    [Hsla; corvene_core::commit_graph::PALETTE_LEN as usize],
);

/// `CommitListItem`
#[allow(clippy::too_many_arguments)]
fn commit_row(
    id: u64,
    ix: usize,
    commit: &Commit,
    is_selected: bool,
    list_focused: bool,
    draggable: bool,
    droppable: bool,
    branches: RowBranches<'_>,
    unpushed: Option<SharedString>,
    selection: Rc<Vec<String>>,
    hint: RowHint,
    dimmed: bool,
    weak: WeakEntity<HistorySidebar>,
    list_focus: FocusHandle,
    menu_anchor: Option<&crate::context_menu::RowMenuAnchor>,
    graph: Option<GraphColumn>,
    cx: &App,
) -> AnyElement {
    let t = cx.ghd();
    let sha = commit.sha.clone();
    let (bg, text, secondary) = if hint.keyboard_selected {
        (
            t.box_selected_background,
            t.box_selected_text,
            t.box_selected_text,
        )
    } else if is_selected && list_focused {
        (
            t.box_selected_active_background,
            t.box_selected_active_text,
            t.box_selected_active_text,
        )
    } else if is_selected {
        (
            t.box_selected_background,
            t.box_selected_text,
            t.box_selected_text,
        )
    } else if hint.squash_target {
        (t.list_item_hover_background, t.text, t.text_secondary)
    } else {
        (t.background, t.text, t.text_secondary)
    };
    // `#commit-list .list-item.selected .commit .tag-name`
    let badge = match (is_selected && !hint.keyboard_selected, list_focused) {
        (true, true) => Some((
            t.list_item_selected_active_badge_background,
            t.list_item_selected_active_badge_text,
        )),
        (true, false) => Some((
            t.list_item_selected_badge_background,
            t.list_item_selected_badge_text,
        )),
        _ => None,
    };
    let commit_for_menu = commit.clone();
    let commit_for_drag = commit.clone();
    let drag_shas: Vec<String> = if selection.contains(&commit.sha) && !selection.is_empty() {
        selection.as_ref().clone()
    } else {
        vec![commit.sha.clone()]
    };
    let weak_for_move = weak.clone();
    let weak_for_drop = weak.clone();
    let line = t.box_selected_active_background;
    div()
        .id(SharedString::from(format!("commit-{}", commit.sha)))
        .a11y_row(
            format!(
                "{}, {}, {}{}",
                if commit.summary.is_empty() {
                    "Empty commit message"
                } else {
                    commit.summary.as_str()
                },
                // the byline's `CommitAttribution`
                commit_attribution_for(commit, cx).text,
                relative(commit.author.date()),
                // `1221-stacked-branch-refs`: the branches starting here
                if hint.stack_line && !branches.names.is_empty() {
                    format!(", {}", branches.names.join(", "))
                } else {
                    String::new()
                }
            ),
            is_selected,
        )
        .relative()
        .w_full()
        .h(commit_row_height(cx))
        .flex_none()
        .bg(bg)
        // `.has-highlighted-commits .list-item:not(.highlighted) { opacity: 30% }`
        .when(dimmed, |d| d.opacity(0.3))
        .border_b_1()
        .border_color(t.box_border)
        .cursor_pointer()
        // no hover colour: `.commit` paints `--background-color` over the
        // list item's hover background in GHD
        .on_click({
            let sha = sha.clone();
            let list_focus = list_focus.clone();
            move |ev: &ClickEvent, window, cx| {
                window.focus(&list_focus, cx);
                let modifiers = ev.modifiers();
                if modifiers.secondary() {
                    Dispatcher::toggle_commit_selection(id, sha.clone(), cx);
                } else if modifiers.shift {
                    Dispatcher::extend_commit_selection(id, sha.clone(), cx);
                } else {
                    Dispatcher::select_commit(id, sha.clone(), cx);
                }
            }
        })
        // GHD `List.onRowMouseDown`: plain presses select at once (not on
        // release), unless the row is part of a multi-selection being dragged
        .on_mouse_down(MouseButton::Left, {
            let sha = sha.clone();
            let list_focus = list_focus.clone();
            let multi = selection.len() > 1 && selection.contains(&sha);
            move |ev: &MouseDownEvent, window, cx| {
                let m = ev.modifiers;
                if m.secondary() || m.shift || m.control || multi {
                    return;
                }
                window.focus(&list_focus, cx);
                if !is_selected {
                    Dispatcher::select_commit(id, sha.clone(), cx);
                }
            }
        })
        .on_mouse_down(MouseButton::Right, {
            let weak = weak.clone();
            let sha = sha.clone();
            move |ev: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                // a right-click selects the commit unless it is selected
                if !is_selected && !selection.contains(&sha) {
                    Dispatcher::select_commit(id, sha.clone(), cx);
                }
                let position = ev.position;
                let commit = commit_for_menu.clone();
                window.focus(&list_focus, cx);
                weak.update(cx, |this, cx| {
                    this.open_row_menu(commit, position, window, cx)
                })
                .ok();
            }
        })
        .when(draggable, |d| {
            d.on_drag(
                CommitDrag {
                    repo: id,
                    shas: drag_shas,
                    commit: commit_for_drag,
                },
                |drag, grab, _, cx| {
                    let drag = drag.clone();
                    cx.new(|cx| CommitDragElement::new(drag, grab, cx))
                },
            )
        })
        .when(droppable, |d| {
            d.on_drag_move::<CommitDrag>(move |ev, _, cx| {
                let bounds = ev.bounds;
                let pos = ev.event.position;
                let hint = if bounds.contains(&pos) {
                    let rel = (pos.y - bounds.origin.y) / bounds.size.height;
                    if rel < 0.25 {
                        Some(DropHint::InsertAt(ix))
                    } else if rel > 0.75 {
                        Some(DropHint::InsertAt(ix + 1))
                    } else {
                        Some(DropHint::Squash(ix))
                    }
                } else {
                    None
                };
                if hint.is_some() {
                    let target = match hint {
                        Some(DropHint::Squash(_)) => Some(DropTarget::Commit),
                        Some(DropHint::InsertAt(_)) => Some(DropTarget::InsertionPoint {
                            count: ev.drag(cx).shas.len(),
                        }),
                        None => None,
                    };
                    Dispatcher::set_drag_target(target, cx);
                    weak_for_move
                        .update(cx, |this, cx| this.update_drop_hint(hint, cx))
                        .ok();
                }
            })
            .on_drop(move |drag: &CommitDrag, _, cx| {
                weak_for_drop
                    .update(cx, |this, cx| this.drop_on_row(id, ix, drag, cx))
                    .ok();
            })
        })
        .map(|d| {
            let contents =
                commit_row_contents_with(commit, text, secondary, badge, branches, unpushed, cx)
                    // `1212-bisect`: the mark after the tags
                    .children(hint.bisect.as_ref().and_then(|b| {
                        let selected = (is_selected || hint.keyboard_selected).then_some(text);
                        Some(b.rows.pill(b.mark?, selected, cx))
                    }))
                    // `1218-compare-refs`: the side of a `base...head` row
                    .children(hint.side.as_ref().map(|(name, base)| {
                        let selected = (is_selected || hint.keyboard_selected).then_some(text);
                        side_pill(name.clone(), *base, selected, cx)
                    }));
            // `1213`: the graph column before the contents (painted in
            // the row, the contents moved over: no extra layout per row)
            let (contents, graph) = match graph {
                Some((graph, columns, palette)) => (
                    contents.pl(crate::commit_graph::column_width(columns) + SPACING()),
                    Some(crate::commit_graph::graph_cell(
                        graph,
                        ix,
                        columns,
                        crate::commit_graph::GraphStyle {
                            palette,
                            accent: (is_selected && list_focused && !hint.keyboard_selected)
                                .then_some(text),
                            background: bg,
                            overflow: secondary,
                            height: commit_row_height(cx),
                        },
                    )),
                ),
                None => (contents, None),
            };
            let d = d.children(graph);
            // `621-context-menu-buttons`: after the contents
            if crate::context_menu::row_menu_buttons(cx) {
                d.group("commit-row")
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(div().flex_1().min_w_0().h_full().child(contents))
                    .child(
                        crate::context_menu::row_menu_button(
                            "row-menu",
                            "commit-row",
                            is_selected,
                            secondary,
                        )
                        .mr(SPACING_HALF()),
                    )
            } else {
                d.child(contents)
            }
        })
        // `1212-bisect`: a bar down the left of the commits still in question
        .when(hint.bisect.as_ref().is_some_and(|b| b.in_range), |d| {
            d.child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left_0()
                    .w(zpx(3.))
                    .bg(t.color_modified),
            )
        })
        // `1221-stacked-branch-refs`: a thick accent line between a stacked
        // branch (or the default branch) and the newer commits above it
        .when(hint.stack_line, |d| {
            let color = if is_selected && list_focused && !hint.keyboard_selected {
                text
            } else {
                t.box_border_accent
            };
            d.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(zpx(3.))
                    .bg(color),
            )
        })
        .when(hint.line_above, |d| {
            d.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(zpx(2.))
                    .bg(line),
            )
        })
        .when(hint.line_below, |d| {
            d.child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .h(zpx(2.))
                    .bg(line),
            )
        })
        .when_some(menu_anchor, |d, anchor| d.child(anchor.track()))
        .into_any_element()
}

impl Render for HistorySidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // drop hints only live while a drag is in flight
        if !cx.has_active_drag() {
            if self.drop_hint.is_some() {
                self.drop_hint = None;
            }
            if self.state.read(cx).drag_target.is_some() {
                Dispatcher::set_drag_target(None, cx);
            }
        }
        // `1216-recent-activity`: the reflog replaces the whole sidebar
        let reflog_open = {
            let s = self.state.read(cx);
            s.selected_state()
                .is_some_and(|rs| corvene_core::reflog::recent_activity_of(s, rs).is_some())
        };
        if reflog_open {
            return self.reflog.clone().into_any_element();
        }
        // `348-pull-request-review`: so does a pull request under review
        let review_open = {
            let s = self.state.read(cx);
            s.selected_state()
                .is_some_and(|rs| corvene_core::pull_request_review::review_of(s, rs).is_some())
        };
        if review_open {
            return self.review.clone().into_any_element();
        }
        // `1219-tag-manager`: so do the tags
        let tags_open = {
            let s = self.state.read(cx);
            s.selected_state()
                .is_some_and(|rs| corvene_core::tag_manager::tags_of(s, rs).is_some())
        };
        if tags_open {
            return self.tags.clone().into_any_element();
        }
        // `345-issues` / `346-releases`: so do the issues and the releases
        let (issues_open, releases_open) = {
            let s = self.state.read(cx);
            let rs = s.selected_state();
            (
                rs.is_some_and(|rs| {
                    corvene_core::issues::issues_of(s, rs).is_some_and(|i| i.loaded || i.loading)
                }),
                rs.is_some_and(|rs| corvene_core::releases::releases_of(s, rs).is_some()),
            )
        };
        if issues_open {
            return self.issues.clone().into_any_element();
        }
        if releases_open {
            return self.releases.clone().into_any_element();
        }
        let (id, show_list, form, merge_status) = {
            let s = self.state.read(cx);
            let id = s.selected;
            let rs = id.and_then(|id| s.repo_states.get(&id));
            (
                id,
                rs.is_some_and(|r| r.compare.show_branch_list),
                rs.map(|r| r.compare.form.clone()),
                rs.and_then(|r| r.compare.merge_status),
            )
        };
        // `onTextBoxFocused` → show the branch list
        let focused = self.compare.read(cx).focus_handle(cx).is_focused(window);
        if focused
            && !self.compare_was_focused
            && let Some(id) = id
        {
            Dispatcher::set_compare_branch_list_visible(id, true, cx);
        }
        self.compare_was_focused = focused;
        let body: AnyElement = match (id, show_list, form) {
            (Some(id), true, _) => self.compare_branch_list(id, cx),
            (
                Some(id),
                false,
                Some(corvene_core::CompareForm::Branch {
                    branch,
                    mode,
                    ahead_behind,
                }),
            ) => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(self.compare_tabs(id, mode, ahead_behind.ahead, ahead_behind.behind, cx))
                .children(self.compare_conflicts(id, cx))
                .children(self.compare_changed_files(id, &branch, cx))
                .child(self.commit_list(cx))
                .when(mode == ComparisonMode::Behind, |d| {
                    d.child(self.merge_cta(id, &branch, ahead_behind.behind, merge_status, cx))
                })
                .into_any_element(),
            // `1218-compare-refs`
            (
                Some(id),
                false,
                Some(corvene_core::CompareForm::Refs {
                    range,
                    base_only,
                    head_only,
                    ..
                }),
            ) => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(self.ref_compare_tabs(id, range, base_only, head_only, cx))
                .child(self.ref_compare_changed_files(id, cx))
                .child(self.commit_list(cx))
                .into_any_element(),
            _ => self.commit_list(cx).into_any_element(),
        };
        let t = cx.ghd().clone();
        // `807`: the first-parent toggle before the compare box
        // `1213`: the All branches toggle after it, its list loaded or dropped
        let all_branches_toggle = {
            let s = self.state.read(cx);
            let rs = s.selected.and_then(|id| s.repo_states.get(&id));
            let on = Dispatcher::history_all_branches(s);
            let stale = !on && s.repo_states.values().any(|rs| rs.all_branches.is_some());
            let missing = on && rs.is_some_and(|rs| rs.info.is_some() && rs.all_branches.is_none());
            if stale {
                Dispatcher::clear_all_branches(cx);
            } else if missing && let Some(id) = id {
                Dispatcher::load_all_branches(id, false, cx);
            }
            let s = self.state.read(cx);
            s.flags
                .bool(corvene_core::flags::ids::COMMIT_GRAPH)
                .then_some(s.settings.history_all_branches)
        };
        let (first_parent_toggle, comparing, search, filter_active, file_history, file) = {
            let s = self.state.read(cx);
            let rs = s.selected.and_then(|id| s.repo_states.get(&id));
            (
                s.flags
                    .bool(corvene_core::flags::ids::HISTORY_FIRST_PARENT)
                    .then_some(s.settings.history_first_parent),
                rs.is_some_and(|rs| rs.compare.is_comparing()),
                s.flags.bool(corvene_core::flags::ids::HISTORY_SEARCH),
                rs.is_some_and(|rs| !rs.history_filter.text.is_empty()),
                s.flags.bool(corvene_core::flags::ids::FILE_HISTORY),
                rs.and_then(|rs| rs.history_filter.path.clone()),
            )
        };
        // `886` / `887`: switched off while filtering, History goes back to normal
        if let Some(id) = id {
            if !search && filter_active {
                Dispatcher::set_history_filter_text(id, String::new(), cx);
            }
            if !file_history && file.is_some() {
                Dispatcher::clear_file_history(id, cx);
            }
        }
        let file = file.filter(|_| file_history);
        let filter_row = ((search || file.is_some()) && !comparing && !show_list && id.is_some())
            .then(|| self.filter_row(search, file, window, cx));
        div()
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            // `onBranchFilterBlur`: clicking anywhere else closes the branch list.
            .when(show_list, |d| {
                d.on_mouse_down_out(
                    cx.listener(|this, _, window, cx| this.compare_blur(window, cx)),
                )
            })
            .child(
                // `#compare-view .compare-form`
                div()
                    .id("compare-form")
                    .key_context("CompareFilter")
                    .on_action(cx.listener(|this, _: &CompareSelect, window, cx| {
                        this.compare_select(window, cx)
                    }))
                    .on_action(cx.listener(|this, _: &CompareClear, window, cx| {
                        this.compare_clear(window, cx)
                    }))
                    .on_action(cx.listener(|this, _: &SelectNextFile, _, cx| {
                        this.move_focused_branch(1, cx)
                    }))
                    .on_action(cx.listener(|this, _: &SelectPreviousFile, _, cx| {
                        this.move_focused_branch(-1, cx)
                    }))
                    .flex_none()
                    .p(SPACING_HALF())
                    .bg(t.box_alt_background)
                    .when(filter_row.is_none(), |d| d.border_b_1())
                    .border_color(t.box_border)
                    .when_some(first_parent_toggle, |d, on| {
                        d.flex()
                            .flex_row()
                            .gap(SPACING_HALF())
                            .child(first_parent_button(on, comparing, cx))
                    })
                    .when_some(all_branches_toggle, |d, on| {
                        d.flex()
                            .flex_row()
                            .gap(SPACING_HALF())
                            .child(all_branches_button(on, comparing, cx))
                    })
                    .child({
                        // `FancyTextBox`: 27 px, `--box-border-color` frame,
                        // a 9 px branch glyph 7 px in, the text at 27 px
                        let focused = self.compare.read(cx).focus_handle(cx).is_focused(window);
                        div().flex_1().min_w_0().child(
                            crate::widgets::filter_text_box(
                                "compare-branch",
                                &self.compare,
                                Some(octicon(Octicon::GitBranch, t.text).size(zpx(9.))),
                                window,
                                cx,
                            )
                            // (25 px inside two borders, which Chromium
                            // lays out in whole device pixels)
                            .h(zpx(25.) + 2. * crate::widgets::hairline(window))
                            .pl(zpx(7.))
                            .gap(zpx(1.))
                            .when(!focused, |d| d.border_color(t.box_border)),
                        )
                    }),
            )
            .children(filter_row)
            .child(body)
            .children(self.context_menu.clone())
            .into_any_element()
    }
}

/// `1218-compare-refs`: the current branch's name (`HEAD` when detached),
/// the Compare… dialog's other side.
pub fn current_ref(id: u64, cx: &App) -> Option<String> {
    let s = AppState::global(cx).read(cx);
    let info = s.repo_states.get(&id)?.info.as_ref()?;
    Some(
        info.current_branch()
            .map(|b| b.name.clone())
            .unwrap_or_else(|| "HEAD".to_string()),
    )
}

/// `1218-compare-refs`: the ref a `base...head` commit is only on, in the
/// colours of a diff's removed (base) and added (head) lines.
fn side_pill(name: SharedString, base: bool, selected: Option<Hsla>, cx: &App) -> Div {
    let t = cx.ghd();
    let color = selected.unwrap_or(if base { t.color_deleted } else { t.color_new });
    div()
        .flex_none()
        .ml(SPACING())
        .max_w(zpx(110.))
        .h(zpx(16.))
        .px(SPACING_HALF())
        .flex()
        .items_center()
        .rounded(BORDER_RADIUS())
        .border_1()
        .border_color(color)
        .text_color(color)
        .text_size(FONT_SIZE_SM())
        .line_height(zpx(14.))
        .child(div().min_w_0().truncate().child(name))
}

/// Flag `821`: ask for a folder, then write the patches there.
fn create_patch_files(id: u64, shas: Vec<String>, cx: &mut App) {
    let receiver = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some("Save Patches".into()),
    });
    cx.spawn(async move |cx: &mut AsyncApp| {
        if let Ok(Ok(Some(paths))) = receiver.await
            && let Some(dir) = paths.into_iter().next()
        {
            cx.update(|cx| Dispatcher::create_patch_files(id, shas, dir, cx));
        }
    })
    .detach();
}
