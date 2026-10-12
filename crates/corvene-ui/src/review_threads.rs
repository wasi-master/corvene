//! Corvene `348-pull-request-review`: review threads inside the diff and
//! the box that writes a new comment (`corvene_core::pull_request_review`).
//! GitHub Desktop has no inline comments; the look follows github.com's
//! review threads on GHD's palette: a card under the line with one row per
//! comment (avatar, login, when, the body as Markdown), Reply, Resolve /
//! Unresolve, Edit / Delete for the viewer's own pending comments, and a
//! resolved thread folded to one line. A `+` on a hovered line (⇧-click
//! for a range) opens the composer, whose comment goes out at once ("Add
//! single comment") or into the pending review ("Start a review" / "Add
//! review comment").
//!
//! The widgets are plain elements: GitHub's state goes through the
//! dispatcher, the open reply boxes and the comment being edited belong to
//! the [`DiffView`] that draws the rows ([`ReviewUi`]).

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use corvene_core::markdown::Block;
use corvene_core::pull_request_review::{
    ComposeTarget, DiffSide, ReviewComment, ReviewFile, ReviewThread,
};
use corvene_core::review_anchor::{AnchoredThread, ThreadAnchor};
use corvene_core::{AppState, Dispatcher};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::diff_view::DiffView;
use crate::github_list::{date_text, pill};
use crate::icons::{Octicon, octicon};
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, GhdTheme, c, primer};
use crate::widgets::{avatar_image, avatar_lookup_url, button, link_button, primary_button};

/// The comment avatars.
fn avatar_size() -> Pixels {
    zpx(20.)
}

/// Everything the rows need to draw the threads, snapshotted per render.
pub struct ReviewUi {
    pub repo: u64,
    pub view: WeakEntity<DiffView>,
    pub threads: Rc<Vec<ReviewThread>>,
    pub bodies: Rc<HashMap<String, Arc<Vec<Block>>>>,
    /// The threads under each unified row.
    pub by_row: HashMap<usize, Vec<AnchoredThread>>,
    /// The composer: where, under which unified row, and its text.
    pub composer: Option<(ComposeTarget, usize, Entity<TextareaState>)>,
    /// Signed in, the pull request known to GitHub, no write in flight.
    pub can_write: bool,
    pub has_pending: bool,
    pub busy: Option<String>,
    pub error: Option<String>,
    pub show_resolved: bool,
    /// Resolved threads unfolded by hand.
    pub expanded: Rc<HashSet<String>>,
    pub reply_boxes: Rc<HashMap<String, Entity<TextareaState>>>,
    pub editing: Option<(String, Entity<TextareaState>)>,
    pub signed_out: bool,
}

impl ReviewUi {
    /// The rows that carry a thread or the composer.
    pub fn decorated_rows(&self) -> impl Iterator<Item = usize> + '_ {
        self.by_row
            .keys()
            .copied()
            .chain(self.composer.as_ref().map(|(_, row, _)| *row))
    }

    /// Whether `line` on `side` is inside the composer's range.
    pub fn in_composer_range(&self, side: DiffSide, line: Option<u32>) -> bool {
        match (&self.composer, line) {
            (Some((target, _, _)), Some(line)) => target.covers(side, line),
            _ => false,
        }
    }

    /// The threads and the composer under the unified row `ix`.
    pub fn decorations(&self, ix: usize, cx: &App) -> Option<AnyElement> {
        let threads = self.by_row.get(&ix);
        let composer = self.composer.as_ref().filter(|(_, row, _)| *row == ix);
        if threads.is_none() && composer.is_none() {
            return None;
        }
        let t = cx.ghd().clone();
        let mut column = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(SPACING_HALF())
            .px(SPACING())
            .py(SPACING_HALF())
            .bg(t.box_alt_background)
            .border_t_1()
            .border_b_1()
            .border_color(t.box_border)
            // a row's text is monospace; the threads are prose
            .font_family(crate::theme::ui_font())
            .text_size(FONT_SIZE())
            .line_height(zpx(18.))
            .text_color(t.text);
        if let Some(threads) = threads {
            for anchored in threads {
                if let Some(thread) = self.threads.get(anchored.thread) {
                    column = column.child(self.thread_card(thread, &anchored.anchor, cx));
                }
            }
        }
        if let Some((target, _, input)) = composer {
            column = column.child(self.composer_card(target, input, cx));
        }
        Some(column.into_any_element())
    }

    /// One thread as a card.
    pub fn thread_card(
        &self,
        thread: &ReviewThread,
        anchor: &ThreadAnchor,
        cx: &App,
    ) -> AnyElement {
        let t = cx.ghd().clone();
        let id = thread.id.clone();
        let folded = thread.is_resolved && !self.show_resolved && !self.expanded.contains(&id);
        let card = div()
            .id(SharedString::from(format!("thread-{id}")))
            .w_full()
            .max_w(zpx(900.))
            .flex()
            .flex_col()
            .rounded(BORDER_RADIUS())
            .border_1()
            .border_color(t.box_border)
            .bg(t.background)
            .overflow_hidden();
        if folded {
            let view = self.view.clone();
            let unfold_id = id.clone();
            let count = thread.comments.len();
            return card
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(SPACING_HALF())
                        .px(SPACING())
                        .py(SPACING_HALF())
                        .child(octicon(Octicon::CheckCircleFill, c(primer::PURPLE_500)).flex_none())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(t.text_secondary)
                                .child(format!(
                                    "{} resolved this conversation ({} comment{})",
                                    thread.resolved_by.as_deref().unwrap_or("Someone"),
                                    count,
                                    if count == 1 { "" } else { "s" }
                                )),
                        )
                        .child(
                            link_button(
                                SharedString::from(format!("thread-unfold-{id}")),
                                "Show",
                                cx,
                            )
                            .on_click(move |_, _, cx| {
                                view.update(cx, |this, cx| {
                                    this.toggle_thread_expanded(&unfold_id, cx)
                                })
                                .ok();
                            }),
                        ),
                )
                .into_any_element();
        }
        let mut card = card;
        // the header: Resolved / Outdated / Pending / moved
        let mut flags: Vec<AnyElement> = Vec::new();
        if thread.is_resolved {
            flags.push(pill("Resolved", c(primer::PURPLE_500)).into_any_element());
        }
        match anchor {
            ThreadAnchor::Outdated => {
                flags.push(pill("Outdated", t.text_secondary).into_any_element())
            }
            ThreadAnchor::Line { remapped: true, .. } => {
                flags.push(pill("Lines moved", t.text_secondary).into_any_element())
            }
            ThreadAnchor::File => flags.push(pill("File", t.text_secondary).into_any_element()),
            _ => {}
        }
        if thread.is_pending() {
            flags.push(pill("Pending", c(primer::YELLOW_700)).into_any_element());
        }
        if !flags.is_empty() || matches!(anchor, ThreadAnchor::Outdated) {
            let mut header = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(SPACING_HALF())
                .px(SPACING())
                .py(SPACING_HALF())
                .bg(t.box_alt_background)
                .border_b_1()
                .border_color(t.box_border)
                .children(flags);
            if matches!(anchor, ThreadAnchor::Outdated)
                && let Some(first) = thread.comments.first()
                && !first.diff_hunk.is_empty()
            {
                // the lines GitHub kept, since the diff no longer has them
                header = header.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(crate::theme::mono_font())
                        .text_size(FONT_SIZE_SM())
                        .text_color(t.text_secondary)
                        .child(
                            first
                                .diff_hunk
                                .lines()
                                .last()
                                .unwrap_or_default()
                                .to_string(),
                        ),
                );
            } else {
                header = header.child(div().flex_1());
            }
            if thread.is_resolved {
                let view = self.view.clone();
                let fold_id = id.clone();
                header = header.child(
                    link_button(SharedString::from(format!("thread-fold-{id}")), "Hide", cx)
                        .on_click(move |_, _, cx| {
                            view.update(cx, |this, cx| this.toggle_thread_expanded(&fold_id, cx))
                                .ok();
                        }),
                );
            }
            card = card.child(header);
        }
        for comment in &thread.comments {
            card = card.child(self.comment_row(thread, comment, &t, cx));
        }
        card = card.child(self.thread_footer(thread, &t, cx));
        card.into_any_element()
    }

    fn comment_row(
        &self,
        thread: &ReviewThread,
        comment: &ReviewComment,
        t: &GhdTheme,
        cx: &App,
    ) -> AnyElement {
        let login = comment
            .author
            .as_ref()
            .map(|a| a.login.clone())
            .unwrap_or_else(|| "ghost".to_string());
        let avatar = comment
            .author
            .as_ref()
            .and_then(|a| a.avatar_url.as_deref())
            .and_then(|url| avatar_lookup_url(url, cx));
        let when = date_text(Some(&comment.created_at));
        let editing = self
            .editing
            .as_ref()
            .filter(|(id, _)| *id == comment.id)
            .map(|(_, input)| input.clone());
        let mut meta = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING_HALF())
            .child(avatar_image(avatar, avatar_size(), cx))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(login))
            .child(
                div()
                    .text_color(t.text_secondary)
                    .child(if comment.pending {
                        "pending".to_string()
                    } else if when.is_empty() {
                        "commented".to_string()
                    } else {
                        format!("commented {when}")
                    }),
            )
            .child(div().flex_1());
        if comment.pending && self.can_write && editing.is_none() {
            let (repo, view) = (self.repo, self.view.clone());
            let (edit_id, edit_body) = (comment.id.clone(), comment.body.clone());
            let delete_id = comment.id.clone();
            meta = meta
                .child(
                    link_button(
                        SharedString::from(format!("edit-{}", comment.id)),
                        "Edit",
                        cx,
                    )
                    .text_size(FONT_SIZE_SM())
                    .on_click(move |_, window, cx| {
                        let input = cx.new(|cx| {
                            let mut s = TextareaState::new(window, cx).rows(3);
                            s.set_value(edit_body.clone(), window, cx);
                            s
                        });
                        window.focus(&input.read(cx).focus_handle(cx), cx);
                        view.update(cx, |this, cx| this.start_comment_edit(&edit_id, input, cx))
                            .ok();
                    }),
                )
                .child(
                    link_button(
                        SharedString::from(format!("delete-{}", comment.id)),
                        "Delete",
                        cx,
                    )
                    .text_size(FONT_SIZE_SM())
                    .on_click(move |_, _, cx| {
                        Dispatcher::delete_review_comment(repo, delete_id.clone(), cx);
                    }),
                );
        }
        let body: AnyElement = match editing {
            Some(input) => {
                let (repo, view) = (self.repo, self.view.clone());
                let comment_id = comment.id.clone();
                let input_for_save = input.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACING_HALF())
                    .child(Textarea::new(&input))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap(SPACING_HALF())
                            .child(
                                button(
                                    SharedString::from(format!("cancel-edit-{}", comment.id)),
                                    "Cancel",
                                    cx,
                                )
                                .on_click({
                                    let view = view.clone();
                                    move |_, _, cx| {
                                        view.update(cx, |this, cx| this.cancel_comment_edit(cx))
                                            .ok();
                                    }
                                }),
                            )
                            .child(
                                primary_button(
                                    SharedString::from(format!("save-edit-{}", comment.id)),
                                    "Save",
                                    !self.can_write,
                                    cx,
                                )
                                .on_click(move |_, _, cx| {
                                    let text = input_for_save.read(cx).value().to_string();
                                    view.update(cx, |this, cx| this.cancel_comment_edit(cx))
                                        .ok();
                                    Dispatcher::update_review_comment(
                                        repo,
                                        comment_id.clone(),
                                        text,
                                        cx,
                                    );
                                }),
                            ),
                    )
                    .into_any_element()
            }
            None => {
                let blocks = self
                    .bodies
                    .get(&comment.id)
                    .cloned()
                    .unwrap_or_else(|| Arc::new(corvene_core::markdown::parse(&comment.body)));
                if blocks.is_empty() {
                    div()
                        .text_color(t.text_secondary)
                        .child("(no text)")
                        .into_any_element()
                } else {
                    // `1333-markdown-math`: parsed again with math
                    match crate::markdown::github_options(cx) {
                        Some(_) => crate::markdown::github_markdown(
                            format!("comment-body-{}", comment.id),
                            &comment.body,
                            Some(&comment.url),
                            cx,
                        ),
                        None => crate::markdown::markdown(
                            format!("comment-body-{}", comment.id),
                            &blocks,
                            Some(&comment.url),
                            cx,
                        ),
                    }
                    .into_any_element()
                }
            }
        };
        let _ = thread;
        div()
            .flex()
            .flex_col()
            .gap(SPACING_HALF())
            .px(SPACING())
            .py(SPACING_HALF())
            .border_b_1()
            .border_color(t.box_border)
            .child(meta)
            .child(div().pl(avatar_size() + SPACING_HALF()).child(body))
            .into_any_element()
    }

    /// Reply box or its button, Resolve / Unresolve.
    fn thread_footer(&self, thread: &ReviewThread, t: &GhdTheme, cx: &App) -> AnyElement {
        let id = thread.id.clone();
        let repo = self.repo;
        let mut footer = div()
            .flex()
            .flex_col()
            .gap(SPACING_HALF())
            .px(SPACING())
            .py(SPACING_HALF())
            .bg(t.box_alt_background);
        if self.signed_out {
            return footer
                .child(
                    div()
                        .text_color(t.text_secondary)
                        .text_size(FONT_SIZE_SM())
                        .child("Sign in to reply."),
                )
                .into_any_element();
        }
        match self.reply_boxes.get(&id) {
            Some(input) => {
                let pending_label = if self.has_pending {
                    "Add review comment"
                } else {
                    "Start a review"
                };
                let view = self.view.clone();
                let cancel_id = id.clone();
                let single_input = input.clone();
                let single_id = id.clone();
                let review_input = input.clone();
                let review_id = id.clone();
                let view_single = self.view.clone();
                let view_review = self.view.clone();
                footer = footer.child(Textarea::new(input)).child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap(SPACING_HALF())
                        .child(
                            button(
                                SharedString::from(format!("reply-cancel-{id}")),
                                "Cancel",
                                cx,
                            )
                            .on_click(move |_, _, cx| {
                                view.update(cx, |this, cx| this.close_reply_box(&cancel_id, cx))
                                    .ok();
                            }),
                        )
                        .child(
                            button(
                                SharedString::from(format!("reply-single-{id}")),
                                "Add single comment",
                                cx,
                            )
                            .on_click(move |_, _, cx| {
                                let text = single_input.read(cx).value().to_string();
                                view_single
                                    .update(cx, |this, cx| this.close_reply_box(&single_id, cx))
                                    .ok();
                                Dispatcher::reply_to_review_thread(
                                    repo,
                                    single_id.clone(),
                                    text,
                                    false,
                                    cx,
                                );
                            }),
                        )
                        .child(
                            primary_button(
                                SharedString::from(format!("reply-review-{id}")),
                                pending_label,
                                !self.can_write,
                                cx,
                            )
                            .on_click(move |_, _, cx| {
                                let text = review_input.read(cx).value().to_string();
                                view_review
                                    .update(cx, |this, cx| this.close_reply_box(&review_id, cx))
                                    .ok();
                                Dispatcher::reply_to_review_thread(
                                    repo,
                                    review_id.clone(),
                                    text,
                                    true,
                                    cx,
                                );
                            }),
                        ),
                );
            }
            None => {
                let mut row = div().flex().flex_row().items_center().gap(SPACING_HALF());
                if thread.viewer_can_reply || thread.comments.is_empty() {
                    let view = self.view.clone();
                    let reply_id = id.clone();
                    row = row.child(
                        button(SharedString::from(format!("reply-{id}")), "Reply…", cx).on_click(
                            move |_, window, cx| {
                                let input = cx.new(|cx| {
                                    TextareaState::new(window, cx)
                                        .placeholder("Leave a reply")
                                        .rows(3)
                                });
                                window.focus(&input.read(cx).focus_handle(cx), cx);
                                view.update(cx, |this, cx| {
                                    this.open_reply_box(&reply_id, input, cx)
                                })
                                .ok();
                            },
                        ),
                    );
                }
                row = row.child(div().flex_1());
                if !thread.is_pending()
                    && (thread.viewer_can_resolve || thread.viewer_can_unresolve)
                {
                    let resolve_id = id.clone();
                    let resolved = thread.is_resolved;
                    let label = if resolved {
                        "Unresolve conversation"
                    } else {
                        "Resolve conversation"
                    };
                    let can = if resolved {
                        thread.viewer_can_unresolve
                    } else {
                        thread.viewer_can_resolve
                    } && self.can_write;
                    let mut b = button(SharedString::from(format!("resolve-{id}")), label, cx);
                    if can {
                        b = b.on_click(move |_, _, cx| {
                            Dispatcher::set_review_thread_resolved(
                                repo,
                                resolve_id.clone(),
                                !resolved,
                                cx,
                            );
                        });
                    } else {
                        b = b.opacity(0.6);
                    }
                    row = row.child(b);
                }
                footer = footer.child(row);
            }
        }
        footer.into_any_element()
    }

    /// The new-comment box under its line.
    fn composer_card(
        &self,
        target: &ComposeTarget,
        input: &Entity<TextareaState>,
        cx: &App,
    ) -> AnyElement {
        let t = cx.ghd().clone();
        let repo = self.repo;
        let (start, end) = target.range();
        let side = match target.side {
            DiffSide::Left => " (old side)",
            DiffSide::Right => "",
        };
        let title = if start == end {
            format!("Comment on line {end}{side}")
        } else {
            format!("Comment on lines {start} to {end}{side}")
        };
        let pending_label = if self.has_pending {
            "Add review comment"
        } else {
            "Start a review"
        };
        let single_input = input.clone();
        let review_input = input.clone();
        let mut card = div()
            .id("review-composer")
            .w_full()
            .max_w(zpx(900.))
            .flex()
            .flex_col()
            .gap(SPACING_HALF())
            .p(SPACING())
            .rounded(BORDER_RADIUS())
            .border_1()
            .border_color(t.accent)
            .bg(t.background)
            .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
            .child(Textarea::new(input));
        if let Some(error) = &self.error {
            card = card.child(crate::widgets::input_error(error.clone(), cx));
        }
        if let Some(busy) = &self.busy {
            card = card.child(
                div()
                    .text_color(t.text_secondary)
                    .text_size(FONT_SIZE_SM())
                    .child(busy.clone()),
            );
        }
        card.child(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .gap(SPACING_HALF())
                .child(
                    button("composer-cancel", "Cancel", cx).on_click(move |_, _, cx| {
                        Dispatcher::set_review_composer(repo, None, cx);
                    }),
                )
                .child(
                    button("composer-single", "Add single comment", cx).on_click(
                        move |_, _, cx| {
                            let text = single_input.read(cx).value().to_string();
                            Dispatcher::add_review_comment(repo, text, true, cx);
                        },
                    ),
                )
                .child(
                    primary_button("composer-review", pending_label, !self.can_write, cx).on_click(
                        move |_, _, cx| {
                            let text = review_input.read(cx).value().to_string();
                            Dispatcher::add_review_comment(repo, text, false, cx);
                        },
                    ),
                ),
        )
        .into_any_element()
    }

    /// The threads the diff has no row for (outdated, on the file), above
    /// the rows, folded to a line until opened.
    pub fn unplaced_panel(&self, file: &ReviewFile, open: bool, cx: &App) -> Option<AnyElement> {
        let t = cx.ghd().clone();
        let unplaced: Vec<&AnchoredThread> = file.unplaced().collect();
        if unplaced.is_empty() {
            return None;
        }
        let outdated = unplaced
            .iter()
            .filter(|a| a.anchor == ThreadAnchor::Outdated)
            .count();
        let on_file = unplaced.len() - outdated;
        let mut what = Vec::new();
        if outdated > 0 {
            what.push(format!(
                "{outdated} outdated conversation{}",
                if outdated == 1 { "" } else { "s" }
            ));
        }
        if on_file > 0 {
            what.push(format!(
                "{on_file} conversation{} on the file",
                if on_file == 1 { "" } else { "s" }
            ));
        }
        let view = self.view.clone();
        let mut panel = div()
            .flex_none()
            .flex()
            .flex_col()
            .bg(t.box_alt_background)
            .border_b_1()
            .border_color(t.box_border)
            .font_family(crate::theme::ui_font())
            .text_size(FONT_SIZE())
            .text_color(t.text)
            .child(
                div()
                    .id("review-unplaced-toggle")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING_HALF())
                    .px(SPACING())
                    .h(zpx(29.))
                    .cursor_pointer()
                    .on_click(move |_, _, cx| {
                        view.update(cx, |this, cx| this.toggle_unplaced_threads(cx))
                            .ok();
                    })
                    .child(octicon(
                        if open {
                            Octicon::ChevronDown
                        } else {
                            Octicon::ChevronRight
                        },
                        t.text_secondary,
                    ))
                    .child(octicon(Octicon::Comment, t.text_secondary))
                    .child(div().child(what.join(", "))),
            );
        if open {
            let mut list = div()
                .flex()
                .flex_col()
                .gap(SPACING_HALF())
                .px(SPACING())
                .pb(SPACING());
            for anchored in unplaced {
                if let Some(thread) = self.threads.get(anchored.thread) {
                    list = list.child(self.thread_card(thread, &anchored.anchor, cx));
                }
            }
            panel = panel.child(list);
        }
        Some(panel.into_any_element())
    }
}

/// The `+` that opens the composer on a line, shown while the row is
/// hovered (`group`); ⇧-click extends the composer's range to the line.
pub fn add_comment_button(
    repo: u64,
    path: &str,
    side: DiffSide,
    line: u32,
    current: Option<&ComposeTarget>,
    group: SharedString,
    cx: &App,
) -> AnyElement {
    let t = cx.ghd();
    let path = path.to_string();
    let current = current.cloned();
    div()
        .id(("add-comment", line as usize))
        .absolute()
        .left(zpx(1.))
        .top(zpx(2.))
        .size(zpx(16.))
        .rounded(zpx(3.))
        .bg(t.accent)
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        // opacity, not visibility: a hidden element registers no click
        // listeners, and nothing guarantees a frame between the hover
        // that would show it and the press
        .opacity(0.)
        .group_hover(group, |s| s.opacity(1.))
        .child(octicon(Octicon::Plus, c(primer::WHITE)).size(zpx(12.)))
        .on_click(move |ev, _, cx| {
            cx.stop_propagation();
            let target = match &current {
                Some(c) if ev.modifiers().shift && c.side == side && c.path == path => {
                    let (start, end) = c.range();
                    ComposeTarget {
                        path: path.clone(),
                        side,
                        line: end.max(line),
                        start: Some(start.min(line)),
                    }
                }
                _ => ComposeTarget {
                    path: path.clone(),
                    side,
                    line,
                    start: None,
                },
            };
            Dispatcher::set_review_composer(repo, Some(target), cx);
        })
        .into_any_element()
}

/// The thread counter of a sidebar row: unresolved (and pending) threads.
pub fn thread_badge(
    unresolved: usize,
    total: usize,
    pending: usize,
    cx: &App,
) -> Option<AnyElement> {
    if total == 0 {
        return None;
    }
    let t = cx.ghd();
    let color = if pending > 0 {
        c(primer::YELLOW_700)
    } else if unresolved > 0 {
        t.text
    } else {
        t.text_secondary
    };
    Some(
        div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(zpx(2.))
            .text_size(FONT_SIZE_SM())
            .text_color(color)
            .child(octicon(Octicon::Comment, color).size(zpx(12.)))
            .child(if unresolved > 0 {
                unresolved.to_string()
            } else {
                total.to_string()
            })
            .into_any_element(),
    )
}

/// The state read for the rows, from the repository's review.
pub fn review_state_of(
    s: &AppState,
    repo: u64,
) -> Option<&corvene_core::pull_request_review::PullRequestReviewState> {
    let rs = s.repo_states.get(&repo)?;
    corvene_core::pull_request_review::review_of(s, rs)
}
