//! Corvene `345-issues`: the selected issue, in the commit view's place
//! while Repository › Issues… is open (`corvene_core::issues`). GitHub
//! Desktop has no issue view.
//!
//! The title with the state badge, who opened it when, the labels and
//! assignees, then the description as Markdown (links open on GitHub), and
//! Create Branch / View on GitHub at the top.

use corvene_core::issues::IssueRow;
use corvene_core::{AppState, Dispatcher};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::context_menu::mac_or;
use crate::github_list::{self, label_chip, state_pill};
use crate::icons::Octicon;
use crate::scrollbar::ScrollbarExt;
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::{button, primary_button};

pub struct IssueView {
    state: Entity<AppState>,
    /// The issue whose body was last laid out (resets the scroll).
    shown: Option<u64>,
    scroll: ScrollHandle,
}

impl IssueView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            shown: None,
            scroll: ScrollHandle::new(),
        }
    }

    fn issue(&self, id: u64, row: &IssueRow, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd().clone();
        let number = row.number;
        let (state_text, state_icon, state_color) = if row.open {
            ("Open", Octicon::IssueOpened, t.pr_open_icon)
        } else if row.not_planned() {
            ("Closed as not planned", Octicon::Skip, t.text_secondary)
        } else {
            (
                "Closed",
                Octicon::IssueClosed,
                github_list::closed_color(&t),
            )
        };
        let opened = github_list::date_text(row.created_at.as_deref());
        let mut meta = format!("{} opened this issue", row.author);
        if !opened.is_empty() {
            meta.push(' ');
            meta.push_str(&opened);
        }
        if row.comments > 0 {
            meta.push_str(&format!(
                " • {} comment{}",
                row.comments,
                if row.comments == 1 { "" } else { "s" }
            ));
        }
        let body = if row.body.trim().is_empty() {
            div()
                .text_color(t.text_secondary)
                .child("No description provided.")
                .into_any_element()
        } else {
            crate::markdown::markdown(
                format!("issue-body-{number}"),
                &crate::markdown::parsed(&row.body),
                Some(&row.html_url),
                cx,
            )
            .into_any_element()
        };
        let header = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(SPACING_HALF())
            .p(SPACING())
            .border_b_1()
            .border_color(t.box_border)
            .bg(t.box_alt_background)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(SPACING())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_row()
                            .items_baseline()
                            .gap(SPACING_HALF())
                            .text_size(FONT_SIZE_LG())
                            .line_height(zpx(22.))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_shrink(1.)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(row.title.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(t.text_secondary)
                                    .child(format!("#{number}")),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .flex_row()
                            .gap(SPACING_HALF())
                            .child(
                                primary_button(
                                    "issue-create-branch",
                                    mac_or("Create Branch", "Create branch"),
                                    false,
                                    cx,
                                )
                                .on_click(move |_, _, cx| {
                                    Dispatcher::create_branch_from_issue(id, number, cx)
                                }),
                            )
                            .child(
                                button("issue-view-on-github", "View on GitHub", cx).on_click(
                                    move |_, _, cx| {
                                        Dispatcher::open_issue_on_github(id, number, cx)
                                    },
                                ),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(SPACING_HALF())
                    .child(state_pill(state_text, state_icon, state_color, cx))
                    .child(
                        div()
                            .text_size(FONT_SIZE())
                            .text_color(t.text_secondary)
                            .child(meta),
                    ),
            )
            .when(!row.labels.is_empty() || !row.assignees.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap(zpx(4.))
                        .children(row.labels.iter().enumerate().map(|(ix, label)| {
                            label_chip(
                                ("issue-view-label", ix),
                                &label.name,
                                &label.color,
                                false,
                                cx,
                            )
                        }))
                        .when(!row.assignees.is_empty(), |d| {
                            d.child(
                                div()
                                    .ml(SPACING_HALF())
                                    .text_size(FONT_SIZE_SM())
                                    .text_color(t.text_secondary)
                                    .child(format!("Assigned to {}", row.assignees.join(", "))),
                            )
                        }),
                )
            });
        div()
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .child(header)
            .child(
                div()
                    .id("issue-body-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .p(SPACING())
                    .child(div().max_w(zpx(760.)).child(body))
                    .with_scrollbar(),
            )
            .into_any_element()
    }
}

impl Render for IssueView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = {
            let s = self.state.read(cx);
            s.selected.and_then(|id| {
                let rs = s.repo_states.get(&id)?;
                let issues = corvene_core::issues::issues_of(s, rs)?;
                Some((id, issues.selected_row().cloned(), issues.loaded))
            })
        };
        let t = cx.ghd().clone();
        let Some((id, row, loaded)) = shown else {
            return div().size_full().bg(t.background).into_any_element();
        };
        let current = row.as_ref().map(|r| r.number);
        if current != self.shown {
            self.shown = current;
            self.scroll.set_offset(point(px(0.), px(0.)));
        }
        let content = match row {
            Some(row) => self.issue(id, &row, cx),
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(t.text_secondary)
                .child(if loaded {
                    "Select an issue to see it here"
                } else {
                    ""
                })
                .into_any_element(),
        };
        div()
            .id("issue-view")
            .size_full()
            .bg(t.background)
            .text_color(t.text)
            .child(content)
            .into_any_element()
    }
}
