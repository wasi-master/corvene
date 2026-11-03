//! Corvene `348-pull-request-review`: Review changes…, the dialog that
//! submits the pending review (`corvene_core::pull_request_review`).
//! GitHub Desktop has no reviews; the layout is github.com's "Review
//! changes" popover as a GHD dialog: a summary text box, the verdict as
//! radio rows (Comment, Approve, Request changes) and Submit review, with
//! a Discard review link while comments are pending. ⌘⏎ in the box
//! submits too. Approving or requesting changes on one's own pull request
//! is refused by GitHub, so those rows are disabled for its author.

use corvene_core::pull_request_review::ReviewEvent;
use corvene_core::{AppState, Dispatcher};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::dialog::{DialogButton, dialog};
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::{link_button, radio_row, resizable_text_area};

pub struct SubmitPullRequestReviewDialog {
    state: Entity<AppState>,
    repo: u64,
    body: Entity<TextareaState>,
    event: ReviewEvent,
}

impl SubmitPullRequestReviewDialog {
    pub fn new(
        state: Entity<AppState>,
        repo: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let body = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Leave a comment")
                .rows(6)
        });
        cx.observe(&body, |_, _, cx| cx.notify()).detach();
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let handle = body.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.subscribe(&body, |this, _, ev: &InputEvent, cx| {
            if let InputEvent::PressEnter {
                secondary: true, ..
            } = ev
            {
                this.submit(cx);
            }
        })
        .detach();
        Self {
            state,
            repo,
            body,
            event: ReviewEvent::Comment,
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let body = self.body.read(cx).value().to_string();
        if self.event == ReviewEvent::Comment && body.trim().is_empty() && self.pending(cx) == 0 {
            return;
        }
        Dispatcher::submit_pull_request_review(self.repo, self.event, body, cx);
    }

    fn pending(&self, cx: &App) -> u64 {
        crate::review_threads::review_state_of(self.state.read(cx), self.repo)
            .map(|r| r.pending_count())
            .unwrap_or(0)
    }
}

impl Render for SubmitPullRequestReviewDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd().clone();
        let repo = self.repo;
        let (pending, own, busy, number, title) = {
            let s = self.state.read(cx);
            let review = crate::review_threads::review_state_of(s, repo);
            (
                review.map(|r| r.pending_count()).unwrap_or(0),
                review.is_some_and(|r| {
                    r.overview.as_ref().is_some_and(|o| {
                        o.author
                            .as_ref()
                            .is_some_and(|a| a.login.eq_ignore_ascii_case(&o.viewer_login))
                    })
                }),
                review.is_some_and(|r| r.busy.is_some()),
                review.map(|r| r.number()).unwrap_or(0),
                review
                    .map(|r| r.pull_request.title.clone())
                    .unwrap_or_default(),
            )
        };
        let body_empty = self.body.read(cx).value().trim().is_empty();
        let view = cx.entity().downgrade();
        let option = |id: &'static str,
                      event: ReviewEvent,
                      label: &'static str,
                      hint: &'static str,
                      disabled: bool| {
            let view = view.clone();
            let row = radio_row(
                id,
                self.event == event,
                div().flex().flex_col().child(div().child(label)).child(
                    div()
                        .text_size(FONT_SIZE_SM())
                        .text_color(t.text_secondary)
                        .child(hint),
                ),
                move |_, cx| {
                    if disabled {
                        return;
                    }
                    view.update(cx, |this, cx| {
                        this.event = event;
                        cx.notify();
                    })
                    .ok();
                },
                cx,
            );
            if disabled { row.opacity(0.6) } else { row }
        };
        let content = crate::dialog::own_enter(div())
            .flex()
            .flex_col()
            .gap(SPACING())
            .w(zpx(440.))
            .child(
                div()
                    .text_color(t.text_secondary)
                    .child(if pending > 0 {
                        format!(
                            "{pending} pending comment{} will be submitted with this review of #{number} {title}.",
                            if pending == 1 { "" } else { "s" }
                        )
                    } else {
                        format!("Review of #{number} {title}.")
                    }),
            )
            .child(resizable_text_area(
                "submit-review-body",
                Textarea::new(&self.body),
                cx,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACING_HALF())
                    .child(option(
                        "review-comment",
                        ReviewEvent::Comment,
                        "Comment",
                        "Submit general feedback without explicit approval.",
                        false,
                    ))
                    .child(option(
                        "review-approve",
                        ReviewEvent::Approve,
                        "Approve",
                        if own {
                            "Pull request authors can't approve their own pull request."
                        } else {
                            "Submit feedback and approve merging these changes."
                        },
                        own,
                    ))
                    .child(option(
                        "review-request-changes",
                        ReviewEvent::RequestChanges,
                        "Request changes",
                        if own {
                            "Pull request authors can't request changes on their own pull request."
                        } else {
                            "Submit feedback that must be addressed before merging."
                        },
                        own,
                    )),
            )
            .when(pending > 0, |d| {
                d.child(
                    link_button("discard-review", "Discard the pending review", cx)
                        .text_color(t.dialog_error)
                        .on_click(move |_, _, cx| Dispatcher::discard_pending_review(repo, cx)),
                )
            });
        let submit_disabled = busy
            || (self.event == ReviewEvent::Comment && body_empty && pending == 0)
            || (self.event != ReviewEvent::Comment && own);
        let view = cx.entity().downgrade();
        let buttons = vec![
            DialogButton {
                id: "submit-review",
                label: "Submit review".into(),
                primary: true,
                disabled: submit_disabled,
                on_click: Box::new(move |_, cx| {
                    view.update(cx, |this, cx| this.submit(cx)).ok();
                }),
            },
            DialogButton {
                id: "submit-review-cancel",
                label: "Cancel".into(),
                primary: false,
                disabled: false,
                on_click: Box::new(|_, cx| Dispatcher::close_popup(cx)),
            },
        ];
        dialog(
            "dialog-submit-pull-request-review",
            "Review changes",
            content,
            buttons,
            |_, cx| Dispatcher::close_popup(cx),
            window,
            cx,
        )
    }
}
