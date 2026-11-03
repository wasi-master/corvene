//! Corvene `1218-compare-refs`: the combined diff of two compared refs, in
//! the commit view's place while History's "Changed Files" row is
//! selected. The header names the range and its line counts; below, the
//! `1203` Compare Branches dialog's resizable file list
//! ([`crate::dialogs::open_pull_request::range_file_list`]) next to the
//! diff. GitHub Desktop has no counterpart.

use std::rc::Rc;

use corvene_core::{AppState, Dispatcher, MergeStatus, PreviewSlot, PullRequestPreview};
use gpui_kit::component::resizable::{
    ResizablePanelEvent, ResizableState, h_resizable, resizable_panel,
};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::actions::{
    ExtendSelectionDown, ExtendSelectionPageDown, ExtendSelectionPageUp, ExtendSelectionUp,
    SelectFirstFile, SelectLastFile, SelectNextFile, SelectPageDown, SelectPageUp,
    SelectPreviousFile,
};
use crate::dialogs::open_pull_request::{range_file_list, range_file_order};
use crate::diff_view::{DiffSource, DiffView, diff_options_button};
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::code_ref;

#[allow(non_snake_case)]
fn FILE_LIST_MIN() -> Pixels {
    zpx(100.)
}
#[allow(non_snake_case)]
fn FILE_LIST_MAX() -> Pixels {
    zpx(600.)
}

pub struct RefCompareView {
    state: Entity<AppState>,
    diff: Entity<DiffView>,
    resizable: Entity<ResizableState>,
    file_list_width: Pixels,
    file_list_focus: FocusHandle,
    file_scroll: ScrollHandle,
}

impl RefCompareView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let diff = cx.new(|cx| DiffView::new(state.clone(), DiffSource::RefCompare, cx));
        // the Compare Branches dialog's width
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
        Self {
            state,
            diff,
            resizable,
            file_list_width,
            file_list_focus: cx.focus_handle().tab_stop(true),
            file_scroll: ScrollHandle::new(),
        }
    }

    fn preview(&self, cx: &App) -> Option<(u64, PullRequestPreview)> {
        let s = self.state.read(cx);
        let id = s.selected?;
        Some((id, s.repo_states.get(&id)?.ref_compare_changes.clone()?))
    }

    /// ↑ / ↓ (wrapping), Home / End.
    fn select_relative(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some((id, preview)) = self.preview(cx) else {
            return;
        };
        let (order, current) = range_file_order(&preview);
        if let Some(ix) = corvene_core::list_selection::step_index(order.len(), current, delta) {
            Dispatcher::select_preview_file(id, PreviewSlot::RefCompare, order[ix].clone(), cx);
            self.file_scroll.scroll_to_item(ix);
        }
    }

    fn select_edge(&mut self, last: bool, cx: &mut Context<Self>) {
        let Some((id, preview)) = self.preview(cx) else {
            return;
        };
        let (order, _) = range_file_order(&preview);
        if !order.is_empty() {
            let ix = if last { order.len() - 1 } else { 0 };
            Dispatcher::select_preview_file(id, PreviewSlot::RefCompare, order[ix].clone(), cx);
            self.file_scroll.scroll_to_item(ix);
        }
    }

    fn message(&self, body: AnyElement, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p(SPACING_DOUBLE())
            .text_size(FONT_SIZE())
            .text_color(t.text_secondary)
            .child(body)
            .into_any_element()
    }

    fn refs_line(
        &self,
        before: &'static str,
        base: String,
        middle: &'static str,
        head: String,
        after: &'static str,
        cx: &Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .justify_center()
            .gap(zpx(3.))
            .child(before)
            .child(code_ref(base, cx))
            .child(middle)
            .child(code_ref(head, cx))
            .child(after)
            .into_any_element()
    }
}

impl Render for RefCompareView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd().clone();
        let Some((id, preview)) = self.preview(cx) else {
            return div().size_full().into_any_element();
        };
        let range = self
            .state
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|rs| rs.compare.refs())
            .map(|(_, _, range)| range)
            .unwrap_or_default();
        let base = preview.base_branch.clone().unwrap_or_default();
        let head = preview.current_branch.clone();
        let (files, added, deleted) = preview
            .changeset
            .as_ref()
            .map(|c| (c.files.len(), c.lines_added, c.lines_deleted))
            .unwrap_or_default();
        let focused = self.file_list_focus.is_focused(window);
        let body: AnyElement = if preview.changeset.is_none() {
            self.message(div().child("Loading…").into_any_element(), cx)
        } else if preview.merge_status == Some(MergeStatus::Invalid) {
            let line = self.refs_line(
                "",
                base.clone(),
                "and",
                head.clone(),
                "are entirely different commit histories.",
                cx,
            );
            self.message(line, cx)
        } else if files == 0 {
            let line = self.refs_line(
                "No file changes between",
                base.clone(),
                "and",
                head.clone(),
                ".",
                cx,
            );
            self.message(line, cx)
        } else {
            h_resizable("ref-compare-files-diff")
                .with_state(&self.resizable)
                .with_handle_appearance(Rc::new(|_, _, _| Some(div().into_any_element())))
                .child(
                    resizable_panel()
                        .size(self.file_list_width)
                        .size_range(FILE_LIST_MIN()..FILE_LIST_MAX())
                        .child(
                            crate::active_resizable::active_resizable(
                                "ref-compare-file-list-resizable",
                                &self.resizable,
                                Some(&self.file_list_focus),
                                crate::active_resizable::ResizableDescription::new(
                                    "Compared file list",
                                    FILE_LIST_MIN()..FILE_LIST_MAX(),
                                ),
                                range_file_list(
                                    "ref-compare-file",
                                    id,
                                    PreviewSlot::RefCompare,
                                    &preview,
                                    focused,
                                    &self.file_scroll,
                                    cx,
                                ),
                            )
                            .key_context("PullRequestFileList")
                            .on_action(cx.listener(|this, _: &SelectNextFile, _, cx| {
                                this.select_relative(1, cx)
                            }))
                            .on_action(cx.listener(|this, _: &SelectPreviousFile, _, cx| {
                                this.select_relative(-1, cx)
                            }))
                            .on_action(cx.listener(|this, _: &ExtendSelectionDown, _, cx| {
                                this.select_relative(1, cx)
                            }))
                            .on_action(cx.listener(|this, _: &ExtendSelectionUp, _, cx| {
                                this.select_relative(-1, cx)
                            }))
                            .on_action(cx.listener(|this, _: &SelectPageUp, _, cx| {
                                let page = crate::filter_list::page_rows_in(&this.file_scroll);
                                this.select_relative(-page, cx)
                            }))
                            .on_action(cx.listener(|this, _: &SelectPageDown, _, cx| {
                                let page = crate::filter_list::page_rows_in(&this.file_scroll);
                                this.select_relative(page, cx)
                            }))
                            .on_action(cx.listener(|this, _: &ExtendSelectionPageUp, _, cx| {
                                let page = crate::filter_list::page_rows_in(&this.file_scroll);
                                this.select_relative(-page, cx)
                            }))
                            .on_action(cx.listener(|this, _: &ExtendSelectionPageDown, _, cx| {
                                let page = crate::filter_list::page_rows_in(&this.file_scroll);
                                this.select_relative(page, cx)
                            }))
                            .on_action(cx.listener(|this, _: &SelectFirstFile, _, cx| {
                                this.select_edge(false, cx)
                            }))
                            .on_action(cx.listener(
                                |this, _: &SelectLastFile, _, cx| this.select_edge(true, cx),
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
                )
                .into_any_element()
        };
        let summary = match range {
            corvene_core::ref_compare::RefRange::Range => "Changes from",
            corvene_core::ref_compare::RefRange::Symmetric => "Changes since the merge base of",
        };
        let middle = match range {
            corvene_core::ref_compare::RefRange::Range => "to",
            corvene_core::ref_compare::RefRange::Symmetric => "and",
        };
        div()
            .id("ref-compare-view")
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .bg(t.background)
            .child(
                // the commit view's header strip
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING())
                    .px(SPACING())
                    .py(SPACING_HALF())
                    .min_h(zpx(36.))
                    .border_b_1()
                    .border_color(t.box_border)
                    .text_size(FONT_SIZE())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .items_center()
                            .gap(zpx(3.))
                            .child(summary)
                            .child(code_ref(base, cx))
                            .child(middle)
                            .child(code_ref(head, cx)),
                    )
                    .when(preview.changeset.is_some(), |d| {
                        d.child(
                            div()
                                .flex_none()
                                .flex()
                                .flex_row()
                                .gap(SPACING_HALF())
                                .text_color(t.text_secondary)
                                .child(format!(
                                    "{files} changed {}",
                                    if files == 1 { "file" } else { "files" }
                                ))
                                .child(div().text_color(t.color_new).child(format!("+{added}")))
                                .child(
                                    div()
                                        .text_color(t.color_deleted)
                                        .child(format!("−{deleted}")),
                                ),
                        )
                    })
                    .child(diff_options_button(&self.diff, cx)),
            )
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }
}
