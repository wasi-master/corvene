//! Corvene `345-issues`: the selected issue, in the commit view's place
//! while Repository › Issues… is open (`corvene_core::issues`). GitHub
//! Desktop has no issue view.
//!
//! Laid out after github.com's issue page: the title with Create Branch,
//! View on GitHub and a "…" menu (Copy Issue Link, Lock / Unlock
//! Conversation), the state badge and who opened it when, the labels and
//! assignees (each with a picker of the repository's labels / assignable
//! users), then the conversation: the description and every comment as a
//! box with the author's avatar, the time and the Markdown body, their
//! reactions underneath, "Load more comments" when GitHub has more pages,
//! and a comment box with Write / Preview tabs, "Comment" and "Close with
//! comment" (as completed or not planned) / "Reopen with comment".

use corvene_core::issues::{IssueComment, IssueConversation, IssueRow, IssueStateChange};
use corvene_core::{AppState, Dispatcher};
use gpui_kit::component::input::TextareaState;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::context_menu::{MenuItem, mac_or};
use crate::github_list::{self, label_chip, state_pill};
use crate::icons::{Octicon, octicon, spin};
use crate::scrollbar::ScrollbarExt;
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, GhdTheme};
use crate::widgets::{button, primary_button};

pub struct IssueView {
    state: Entity<AppState>,
    /// The issue whose body was last laid out (resets the scroll).
    shown: Option<u64>,
    scroll: ScrollHandle,
    comment: Entity<TextareaState>,
    /// The comment box's Preview tab is showing.
    preview: bool,
    /// The conversation's `posted` count last seen (a new one clears the
    /// comment box).
    posted: (u64, u64),
    /// The repository's labels and assignees were asked for.
    meta_requested: bool,
}

/// What the view needs of the selected issue, read once per frame.
struct Shown {
    id: u64,
    row: IssueRow,
    conversation: IssueConversation,
    labels: Vec<corvene_core::issues::IssueLabel>,
    assignees: Vec<String>,
    meta_loaded: bool,
}

fn avatar_size() -> Pixels {
    zpx(20.)
}

impl IssueView {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let comment = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Leave a comment")
                .rows(5)
        });
        cx.observe(&comment, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            shown: None,
            scroll: ScrollHandle::new(),
            comment,
            preview: false,
            posted: (0, 0),
            meta_requested: false,
        }
    }

    fn header(&self, s: &Shown, cx: &mut Context<Self>) -> Div {
        let t = cx.ghd().clone();
        let (id, row) = (s.id, &s.row);
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
        let html_url = row.html_url.clone();
        let locked = row.locked;
        let more = button("issue-more", "", cx)
            .px(SPACING_HALF())
            .child(octicon(Octicon::KebabHorizontal, t.text))
            .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                let url = html_url.clone();
                let items = vec![
                    MenuItem::new(
                        mac_or("Copy Issue Link", "Copy issue link"),
                        move |_, cx| cx.write_to_clipboard(ClipboardItem::new_string(url.clone())),
                    ),
                    MenuItem::new(
                        if locked {
                            mac_or("Unlock Conversation", "Unlock conversation")
                        } else {
                            mac_or("Lock Conversation", "Lock conversation")
                        },
                        move |_, cx| Dispatcher::set_issue_locked(id, number, !locked, cx),
                    ),
                    MenuItem::separator(),
                    MenuItem::new(
                        mac_or("Open in Browser", "Open in browser"),
                        move |_, cx| Dispatcher::open_issue_on_github(id, number, cx),
                    ),
                ];
                crate::native_menu::show_context_menu(items, ev.position, window, cx);
            });
        div()
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
                            )
                            .child(more),
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
                    )
                    .when(row.locked, |d| {
                        d.child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(zpx(4.))
                                .text_size(FONT_SIZE())
                                .text_color(t.text_secondary)
                                .child(octicon(Octicon::Lock, t.text_secondary))
                                .child("Locked"),
                        )
                    }),
            )
            .child(self.meta_row(s, cx))
    }

    /// Labels and assignees, each behind its picker button.
    fn meta_row(&self, s: &Shown, cx: &mut Context<Self>) -> Div {
        let t = cx.ghd().clone();
        let (id, row) = (s.id, &s.row);
        let number = row.number;
        // the repository's labels, and any the issue has that it lacks
        let mut all_labels: Vec<String> = s.labels.iter().map(|l| l.name.clone()).collect();
        for label in &row.labels {
            if !all_labels.contains(&label.name) {
                all_labels.push(label.name.clone());
            }
        }
        let current_labels: Vec<String> = row.labels.iter().map(|l| l.name.clone()).collect();
        let mut all_assignees = s.assignees.clone();
        for login in &row.assignees {
            if !all_assignees.iter().any(|a| a.eq_ignore_ascii_case(login)) {
                all_assignees.push(login.clone());
            }
        }
        let current_assignees = row.assignees.clone();
        let meta_loaded = s.meta_loaded;
        let picker = |key: &'static str,
                      title: &'static str,
                      all: Vec<String>,
                      current: Vec<String>,
                      labels: bool| {
            small_picker_button(key, title, &t, cx).on_mouse_down(
                MouseButton::Left,
                move |ev, window, cx| {
                    let mut items: Vec<MenuItem> = all
                        .iter()
                        .map(|name| {
                            let checked = current.contains(name);
                            let mut next: Vec<String> =
                                current.iter().filter(|c| *c != name).cloned().collect();
                            if !checked {
                                next.push(name.clone());
                            }
                            MenuItem::checkbox(name.clone(), checked, move |_, cx| {
                                if labels {
                                    Dispatcher::set_issue_labels(id, number, next.clone(), cx)
                                } else {
                                    Dispatcher::set_issue_assignees(id, number, next.clone(), cx)
                                }
                            })
                        })
                        .collect();
                    if items.is_empty() {
                        items.push(
                            MenuItem::new(
                                if meta_loaded {
                                    if labels {
                                        "No labels"
                                    } else {
                                        "No one to assign"
                                    }
                                } else {
                                    "Loading…"
                                },
                                |_, _| {},
                            )
                            .enabled(false),
                        );
                    }
                    crate::native_menu::show_context_menu(items, ev.position, window, cx);
                },
            )
        };
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(zpx(4.))
            .child(picker(
                "issue-labels-picker",
                "Labels",
                all_labels,
                current_labels,
                true,
            ))
            .when(row.labels.is_empty(), |d| {
                d.child(
                    div()
                        .text_size(FONT_SIZE_SM())
                        .text_color(t.text_secondary)
                        .child("None yet"),
                )
            })
            .children(row.labels.iter().enumerate().map(|(ix, label)| {
                label_chip(
                    ("issue-view-label", ix),
                    &label.name,
                    &label.color,
                    false,
                    cx,
                )
            }))
            .child(div().w(SPACING()))
            .child(picker(
                "issue-assignees-picker",
                "Assignees",
                all_assignees,
                current_assignees,
                false,
            ))
            .child(
                div()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(if row.assignees.is_empty() {
                        "No one assigned".to_string()
                    } else {
                        row.assignees.join(", ")
                    }),
            )
    }

    fn conversation(&self, s: &Shown, cx: &mut Context<Self>) -> Div {
        let t = cx.ghd().clone();
        let (id, row, c) = (s.id, &s.row, &s.conversation);
        let number = row.number;
        let mut col = div().flex().flex_col().gap(SPACING()).max_w(zpx(760.));
        let body = if row.body.trim().is_empty() {
            div()
                .text_color(t.text_secondary)
                .italic()
                .child("No description provided.")
                .into_any_element()
        } else {
            // `1333-markdown-math`
            crate::markdown::github_markdown(
                format!("issue-body-{number}"),
                &row.body,
                Some(&row.html_url),
                cx,
            )
            .into_any_element()
        };
        col = col.child(comment_box(
            ("issue-body-box", number),
            &row.author,
            row.author_avatar.as_deref(),
            "opened",
            row.created_at.as_deref(),
            body,
            &row.reactions,
            cx,
        ));
        for comment in &c.comments {
            col = col.child(self.comment(number, &row.html_url, comment, cx));
        }
        if c.loading {
            col = col.child(status_line(
                Some(spin(
                    octicon(Octicon::SyncClockwise, t.text_secondary),
                    "issue-comments-spin",
                )),
                "Loading comments…",
                &t,
            ));
        } else if let Some(err) = &c.error {
            col = col.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACING_HALF())
                    .items_start()
                    .child(status_line(
                        Some(octicon(Octicon::Alert, t.text_secondary).into_any_element()),
                        &format!("Could not load the comments: {err}"),
                        &t,
                    ))
                    .child(button("issue-comments-retry", "Retry", cx).on_click(
                        move |_, _, cx| Dispatcher::load_issue_conversation(id, number, cx),
                    )),
            );
        }
        if c.next_page.is_some() && !c.loading {
            let label = if c.loading_more {
                "Loading…".to_string()
            } else {
                let left = row.comments.saturating_sub(c.comments.len() as u64);
                if left > 0 {
                    format!("Load more comments ({left} hidden)")
                } else {
                    "Load more comments".to_string()
                }
            };
            col = col.child(
                div().flex().justify_center().child(
                    button("issue-comments-more", label, cx)
                        .on_click(move |_, _, cx| Dispatcher::load_more_issue_comments(id, cx)),
                ),
            );
        }
        if row.locked {
            col = col.child(status_line(
                Some(octicon(Octicon::Lock, t.text_secondary).into_any_element()),
                "This conversation has been locked and limited to collaborators.",
                &t,
            ));
        }
        col.child(self.composer(s, cx))
    }

    fn comment(
        &self,
        number: u64,
        base: &str,
        comment: &IssueComment,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let body = if comment.body.trim().is_empty() {
            div()
                .italic()
                .text_color(cx.ghd().text_secondary)
                .child("No description provided.")
                .into_any_element()
        } else {
            crate::markdown::github_markdown(
                format!("issue-{number}-comment-{}", comment.id),
                &comment.body,
                Some(base),
                cx,
            )
            .into_any_element()
        };
        comment_box(
            ("issue-comment-box", comment.id),
            &comment.author,
            comment.avatar_url.as_deref(),
            "commented",
            Some(&comment.created_at),
            body,
            &comment.reactions,
            cx,
        )
        .into_any_element()
    }

    /// The comment box and its buttons.
    fn composer(&self, s: &Shown, cx: &mut Context<Self>) -> Div {
        let t = cx.ghd().clone();
        let (id, row) = (s.id, &s.row);
        let number = row.number;
        let busy = s.conversation.busy;
        let text = self.comment.read(cx).value().to_string();
        let empty = text.trim().is_empty();
        let this = cx.weak_entity();
        let editor = crate::markdown_editor::markdown_editor(
            "issue-comment",
            &self.comment,
            self.preview,
            Some(&row.html_url),
            zpx(90.),
            move |preview, _, cx| {
                this.update(cx, |v, cx| {
                    v.preview = preview;
                    cx.notify();
                })
                .ok();
            },
            cx,
        );
        let comment_input = self.comment.clone();
        let take_text = move |cx: &App| comment_input.read(cx).value().to_string();
        let state_button = if row.open {
            let take_text = take_text.clone();
            button(
                "issue-close",
                if empty {
                    mac_or("Close Issue", "Close issue")
                } else {
                    mac_or("Close with Comment", "Close with comment")
                },
                cx,
            )
            .gap(zpx(4.))
            .child(octicon(Octicon::TriangleDown, t.text).size(zpx(12.)))
            .when(busy, |b| b.opacity(0.5))
            .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                if busy {
                    return;
                }
                let close = |change: IssueStateChange| {
                    let take_text = take_text.clone();
                    move |_: &mut Window, cx: &mut App| {
                        let text = take_text(cx);
                        Dispatcher::comment_on_issue(id, number, text, Some(change), cx)
                    }
                };
                let items = vec![
                    MenuItem::new(
                        mac_or("Close as Completed", "Close as completed"),
                        close(IssueStateChange::CloseCompleted),
                    ),
                    MenuItem::new(
                        mac_or("Close as Not Planned", "Close as not planned"),
                        close(IssueStateChange::CloseNotPlanned),
                    ),
                ];
                crate::native_menu::show_context_menu(items, ev.position, window, cx);
            })
        } else {
            let take_text = take_text.clone();
            button(
                "issue-reopen",
                if empty {
                    mac_or("Reopen Issue", "Reopen issue")
                } else {
                    mac_or("Reopen with Comment", "Reopen with comment")
                },
                cx,
            )
            .when(busy, |b| b.opacity(0.5))
            .on_click(move |_, _, cx| {
                if !busy {
                    let text = take_text(cx);
                    Dispatcher::comment_on_issue(
                        id,
                        number,
                        text,
                        Some(IssueStateChange::Reopen),
                        cx,
                    )
                }
            })
        };
        let can_comment = !empty && !busy;
        div()
            .flex()
            .flex_col()
            .gap(SPACING_HALF())
            .mt(SPACING_HALF())
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Add a comment"),
            )
            .child(editor)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .items_center()
                    .gap(SPACING_HALF())
                    .when(busy, |d| {
                        d.child(spin(
                            octicon(Octicon::SyncClockwise, t.text_secondary),
                            "issue-write-spin",
                        ))
                    })
                    .child(state_button)
                    .child(
                        primary_button("issue-comment-post", "Comment", !can_comment, cx).on_click(
                            move |_, _, cx| {
                                if can_comment {
                                    let text = take_text(cx);
                                    Dispatcher::comment_on_issue(id, number, text, None, cx)
                                }
                            },
                        ),
                    ),
            )
    }
}

/// A small secondary button with a gear, opening a picker.
fn small_picker_button(
    key: &'static str,
    title: &'static str,
    t: &GhdTheme,
    cx: &App,
) -> Stateful<Div> {
    crate::widgets::small_button(key, title, cx)
        .gap(zpx(4.))
        .child(octicon(Octicon::Gear, t.text_secondary).size(zpx(12.)))
}

/// A line of muted text with an icon (loading, errors, locked).
fn status_line(icon: Option<AnyElement>, text: &str, t: &GhdTheme) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(SPACING_HALF())
        .text_color(t.text_secondary)
        .children(icon)
        .child(text.to_string())
}

/// github.com's timeline comment: a bordered box whose header (on the
/// subtle canvas) names the author and when, the Markdown body, and the
/// reaction counts under it.
#[allow(clippy::too_many_arguments)]
fn comment_box(
    id: impl Into<ElementId>,
    author: &str,
    avatar_url: Option<&str>,
    verb: &str,
    when: Option<&str>,
    body: AnyElement,
    reactions: &[(String, u64)],
    cx: &mut Context<IssueView>,
) -> Stateful<Div> {
    let t = cx.ghd().clone();
    let avatar = match avatar_url {
        Some(url) => {
            let path = crate::widgets::avatar_lookup_url(url, cx);
            if path.is_none() {
                let url = url.to_string();
                cx.defer(move |cx| Dispatcher::request_avatar_url(&url, cx));
            }
            crate::widgets::avatar_image(path, avatar_size(), cx)
        }
        None => crate::widgets::avatar_image(None, avatar_size(), cx),
    };
    let when = github_list::date_text(when);
    div()
        .id(id)
        .flex()
        .flex_col()
        .border_1()
        .border_color(t.box_border)
        .rounded(BORDER_RADIUS())
        .overflow_hidden()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(SPACING_HALF())
                .px(SPACING())
                .py(zpx(6.))
                .bg(t.box_alt_background)
                .border_b_1()
                .border_color(t.box_border)
                .text_size(FONT_SIZE())
                .child(avatar)
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(author.to_string()),
                )
                .child(
                    div()
                        .text_color(t.text_secondary)
                        .child(if when.is_empty() {
                            verb.to_string()
                        } else {
                            format!("{verb} {when}")
                        }),
                ),
        )
        .child(div().p(SPACING()).child(body))
        .when(!reactions.is_empty(), |d| {
            d.child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap(zpx(4.))
                    .px(SPACING())
                    .pb(SPACING())
                    .children(reactions.iter().map(|(emoji, count)| {
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(zpx(4.))
                            .h(zpx(22.))
                            .px(zpx(8.))
                            .rounded(zpx(11.))
                            .border_1()
                            .border_color(t.box_border)
                            .bg(t.box_alt_background)
                            .text_size(FONT_SIZE_SM())
                            .child(emoji.clone())
                            .child(div().text_color(t.text_secondary).child(count.to_string()))
                    })),
            )
        })
}

impl Render for IssueView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = {
            let s = self.state.read(cx);
            s.selected.and_then(|id| {
                let rs = s.repo_states.get(&id)?;
                let issues = corvene_core::issues::issues_of(s, rs)?;
                let row = issues.selected_row().cloned();
                let shown = row.map(|row| Shown {
                    id,
                    conversation: issues.selected_conversation().cloned().unwrap_or_else(|| {
                        IssueConversation {
                            number: row.number,
                            ..IssueConversation::default()
                        }
                    }),
                    row,
                    labels: issues.labels.clone(),
                    assignees: issues.assignees.clone(),
                    meta_loaded: issues.meta_loaded,
                });
                Some((
                    id,
                    shown,
                    issues.loaded,
                    issues.meta_loaded || issues.meta_loading,
                ))
            })
        };
        let t = cx.ghd().clone();
        let Some((id, shown, loaded, meta_asked)) = shown else {
            return div().size_full().bg(t.background).into_any_element();
        };
        if !meta_asked && !self.meta_requested {
            self.meta_requested = true;
            cx.defer(move |cx| Dispatcher::load_issue_meta(id, cx));
        }
        let current = shown.as_ref().map(|s| s.row.number);
        if current != self.shown {
            self.shown = current;
            self.preview = false;
            self.scroll.set_offset(point(px(0.), px(0.)));
        }
        if let Some(s) = &shown {
            // a posted comment empties the box
            let posted = (s.row.number, s.conversation.posted);
            if posted.0 == self.posted.0 && posted.1 > self.posted.1 {
                self.comment.update(cx, |c, cx| c.set_value("", window, cx));
                self.preview = false;
            }
            self.posted = posted;
        }
        let content = match &shown {
            Some(s) => div()
                .size_full()
                .flex()
                .flex_col()
                .min_h_0()
                .child(self.header(s, cx))
                .child(
                    div()
                        .id("issue-body-scroll")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .p(SPACING())
                        .child(self.conversation(s, cx))
                        .with_scrollbar(),
                )
                .into_any_element(),
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
