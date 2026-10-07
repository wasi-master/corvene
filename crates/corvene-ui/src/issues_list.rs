//! Corvene `345-issues`: the History sidebar while Repository › Issues… is
//! open (`corvene_core::issues`). GitHub Desktop has no issues list.
//!
//! A header counts the issues and offers New Issue…, a refresh and Close.
//! Under it a filter box and the state picker (Open, Closed, Assigned to
//! me), then the repository's labels as chips that narrow the list to one
//! label. Each row shows the state icon, `#N title`, the issue's labels and
//! who opened it when, with its comment count. Selecting a row shows the
//! issue in the commit view's place ([`crate::issue_view`]); its context
//! menu creates a branch from it, opens it on GitHub or copies its URL.

use std::time::SystemTime;

use corvene_core::issues::{IssueFilter, IssueRow, IssuesViewState};
use corvene_core::{AppState, Dispatcher, Popup};
use gpui_kit::component::input::InputState;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::context_menu::{MenuItem, mac_or};
use crate::github_list::{self, header, header_button, label_chip};
use crate::icons::{Octicon, octicon};
use crate::scrollbar::ScrollbarExt;
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::GhdTooltip;

/// GHD `RowHeight` of the pull request list.
fn row_height() -> Pixels {
    zpx(47.)
}

pub struct IssuesList {
    state: Entity<AppState>,
    filter: Entity<InputState>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    /// The selected issue last scrolled into view.
    scrolled_to: Option<u64>,
    /// Focused since it opened (arrows and Escape work at once).
    focused: bool,
}

impl IssuesList {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
        cx.observe(&filter, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            filter,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle().tab_stop(true),
            scrolled_to: None,
            focused: false,
        }
    }

    /// The rows the list shows, after every filter.
    fn visible(&self, id: u64, cx: &App) -> Vec<IssueRow> {
        let s = self.state.read(cx);
        let Some(rs) = s.repo_states.get(&id) else {
            return Vec::new();
        };
        let Some(issues) = corvene_core::issues::issues_of(s, rs) else {
            return Vec::new();
        };
        let login = s
            .repository(id)
            .and_then(|r| r.non_fork_github())
            .and_then(|gh| s.account_for_repository_on(id, &gh.endpoint))
            .map(|a| a.login.clone());
        let query = self.filter.read(cx).value().to_string();
        issues
            .visible(login.as_deref(), &query)
            .into_iter()
            .cloned()
            .collect()
    }

    /// ↑ / ↓: the row above or below the selected one.
    fn move_selection(&mut self, id: u64, down: bool, cx: &mut Context<Self>) {
        let rows = self.visible(id, cx);
        if rows.is_empty() {
            return;
        }
        let selected = self
            .state
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|rs| rs.issues.as_ref())
            .and_then(|i| i.selected);
        let current = selected.and_then(|n| rows.iter().position(|r| r.number == n));
        let next = match (current, down) {
            (None, _) => 0,
            (Some(c), true) => (c + 1).min(rows.len() - 1),
            (Some(c), false) => c.saturating_sub(1),
        };
        Dispatcher::select_issue(id, Some(rows[next].number), cx);
    }

    /// `count`: the rows shown after every filter.
    fn header(&self, id: u64, issues: &IssuesViewState, count: usize, cx: &Context<Self>) -> Div {
        let t = cx.ghd();
        let subtitle = if issues.loaded && !issues.signed_out {
            Some(match issues.filter {
                IssueFilter::AssignedToMe => format!("{count} assigned to you"),
                filter => format!("{count} {}", filter.label().to_lowercase()),
            })
        } else {
            None
        };
        header(
            Octicon::IssueOpened,
            "Issues",
            subtitle,
            vec![
                header_button("issues-new", "New Issue", cx)
                    .child(octicon(Octicon::Plus, t.text))
                    .on_click(move |_, _, cx| {
                        Dispatcher::show_popup(Popup::NewIssue { repo: id }, cx)
                    })
                    .into_any_element(),
                header_button("issues-refresh", "Refresh Issues", cx)
                    .child(octicon(Octicon::Sync, t.text))
                    .on_click(move |_, _, cx| Dispatcher::load_issues(id, cx))
                    .into_any_element(),
                header_button("issues-close", "Close Issues", cx)
                    .child(octicon(Octicon::X, t.text))
                    .on_click(move |_, _, cx| Dispatcher::close_issues(id, cx))
                    .into_any_element(),
            ],
            cx,
        )
    }

    /// The filter box and the state picker.
    fn filters(
        &self,
        id: u64,
        issues: &IssuesViewState,
        window: &Window,
        cx: &Context<Self>,
    ) -> Div {
        let t = cx.ghd();
        let options: Vec<SharedString> = IssueFilter::ALL
            .iter()
            .map(|f| SharedString::from(f.label()))
            .collect();
        let selected = IssueFilter::ALL.iter().position(|f| *f == issues.filter);
        let on_select: crate::widgets::SelectHandler = std::rc::Rc::new(move |ix, _, cx| {
            if let Some(filter) = IssueFilter::ALL.get(ix) {
                Dispatcher::set_issue_filter(id, *filter, cx);
            }
        });
        div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING_HALF())
            .p(SPACING())
            .pb(SPACING_HALF())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(crate::widgets::filter_text_box(
                        "issues-filter",
                        &self.filter,
                        Some(octicon(Octicon::Search, t.text_secondary)),
                        window,
                        cx,
                    )),
            )
            .child(
                div()
                    .flex_none()
                    .w(zpx(128.))
                    .child(crate::widgets::select_button(
                        "issues-state",
                        issues.filter.label(),
                        options,
                        selected,
                        false,
                        on_select,
                        cx,
                    )),
            )
    }

    /// The label chips; the active one is ringed and clicking it again
    /// clears the filter.
    fn labels(&self, id: u64, issues: &IssuesViewState, cx: &Context<Self>) -> Option<Div> {
        let labels = issues.chip_labels();
        if labels.is_empty() {
            return None;
        }
        Some(
            div()
                .flex_none()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap(zpx(4.))
                .px(SPACING())
                .pb(SPACING_HALF())
                .children(labels.into_iter().enumerate().map(|(ix, label)| {
                    let selected = issues.label_filter.as_deref() == Some(label.name.as_str());
                    let name = label.name.clone();
                    let tooltip = label.description.clone().unwrap_or_else(|| name.clone());
                    label_chip(
                        ("issue-label-chip", ix),
                        &label.name,
                        &label.color,
                        selected,
                        cx,
                    )
                    .cursor_pointer()
                    .ghd_tooltip(tooltip)
                    .on_click(move |_, _, cx| {
                        let next = (!selected).then(|| name.clone());
                        Dispatcher::set_issue_label_filter(id, next, cx);
                    })
                })),
        )
    }
}

/// What a row's context menu acts on.
#[derive(Clone)]
struct RowTarget {
    number: u64,
    html_url: String,
}

fn row_menu(
    id: u64,
    target: RowTarget,
    position: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let RowTarget { number, html_url } = target;
    let items = vec![
        MenuItem::new(
            mac_or("Create Branch from Issue", "Create branch from issue"),
            move |_, cx| Dispatcher::create_branch_from_issue(id, number, cx),
        ),
        MenuItem::separator(),
        MenuItem::new(mac_or("View on GitHub", "View on GitHub"), move |_, cx| {
            Dispatcher::open_issue_on_github(id, number, cx)
        }),
        MenuItem::new(mac_or("Copy Issue URL", "Copy issue URL"), move |_, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(html_url.clone()))
        }),
    ];
    crate::native_menu::show_context_menu(items, position, window, cx);
}

/// `opened 3 days ago by octocat • 2 comments`.
fn byline(row: &IssueRow, now: SystemTime) -> String {
    let when = github_list::date_text_at(row.created_at.as_deref(), now);
    let mut text = if row.open {
        format!("#{} opened {when} by {}", row.number, row.author)
    } else {
        format!("#{} by {}, closed", row.number, row.author)
    };
    if row.comments > 0 {
        text.push_str(&format!(
            " • {} comment{}",
            row.comments,
            if row.comments == 1 { "" } else { "s" }
        ));
    }
    text
}

impl Render for IssuesList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let loaded = {
            let s = self.state.read(cx);
            s.selected.and_then(|id| {
                let rs = s.repo_states.get(&id)?;
                let issues = corvene_core::issues::issues_of(s, rs)?.clone();
                let target = github_list::sign_in_target(s, id);
                Some((id, issues, target))
            })
        };
        let Some((id, issues, (target_name, target_endpoint))) = loaded else {
            self.focused = false;
            return div().into_any_element();
        };
        if !self.focused {
            self.focused = true;
            self.scrolled_to = None;
            window.focus(&self.focus, cx);
        }
        let t = cx.ghd().clone();
        let rows = self.visible(id, cx);
        let header = self.header(id, &issues, rows.len(), cx);

        // keep the selected row on screen (keyboard, reloads)
        if issues.selected != self.scrolled_to {
            self.scrolled_to = issues.selected;
            if let Some(ix) = issues
                .selected
                .and_then(|n| rows.iter().position(|r| r.number == n))
            {
                self.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
            }
        }
        let list_focused = self.focus.contains_focused(window, cx);
        let body = if issues.signed_out {
            github_list::signed_out("issues", target_name, target_endpoint, cx).into_any_element()
        } else if !issues.loaded {
            github_list::loading("issues", "issues-spin", &t).into_any_element()
        } else if let Some(message) = issues.error.clone().filter(|_| issues.rows().is_empty()) {
            github_list::error(message, move |_, cx| Dispatcher::load_issues(id, cx), cx)
                .into_any_element()
        } else if rows.is_empty() {
            let text = if issues.rows().is_empty() {
                match issues.filter {
                    IssueFilter::Open => "No open issues",
                    IssueFilter::Closed => "No closed issues",
                    IssueFilter::AssignedToMe => "No issues assigned to you",
                }
            } else {
                "Sorry, I can't find that issue"
            };
            github_list::message(text, &t).into_any_element()
        } else {
            let now = SystemTime::now();
            let focus = self.focus.clone();
            let selected = issues.selected;
            let closed = github_list::closed_color(&t);
            uniform_list("issue-rows", rows.len(), move |range, _window, cx| {
                range
                    .map(|ix| {
                        let row = &rows[ix];
                        let is_selected = selected == Some(row.number);
                        let (bg, text, secondary) = if is_selected && list_focused {
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
                        } else {
                            (t.background, t.text, t.text_secondary)
                        };
                        let (icon, icon_color) = if row.open {
                            (Octicon::IssueOpened, t.pr_open_icon)
                        } else if row.not_planned() {
                            (Octicon::Skip, t.text_secondary)
                        } else {
                            (Octicon::IssueClosed, closed)
                        };
                        let icon_color = if is_selected && list_focused {
                            text
                        } else {
                            icon_color
                        };
                        let number = row.number;
                        let target = RowTarget {
                            number,
                            html_url: row.html_url.clone(),
                        };
                        let (focus, menu_focus) = (focus.clone(), focus.clone());
                        let labels: Vec<_> = row.labels.iter().take(4).cloned().collect();
                        div()
                            .id(("issue-row", ix))
                            .ghd_tooltip(row.title.clone())
                            .h(row_height())
                            .w_full()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(SPACING_HALF())
                            .pl(SPACING())
                            .pr(SPACING() + SPACING_HALF())
                            .bg(bg)
                            .border_b_1()
                            .border_color(t.box_border)
                            .on_click(move |_, window, cx| {
                                window.focus(&focus, cx);
                                Dispatcher::select_issue(id, Some(number), cx);
                            })
                            .on_mouse_down(MouseButton::Right, move |ev, window, cx| {
                                cx.stop_propagation();
                                window.focus(&menu_focus, cx);
                                Dispatcher::select_issue(id, Some(number), cx);
                                row_menu(id, target.clone(), ev.position, window, cx);
                            })
                            .child(octicon(icon, icon_color).flex_none())
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap(zpx(4.))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .truncate()
                                                    .text_size(FONT_SIZE())
                                                    .line_height(zpx(18.))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(text)
                                                    .child(row.title.clone()),
                                            )
                                            .child(
                                                div()
                                                    .flex_none()
                                                    .max_w(relative(0.5))
                                                    .overflow_hidden()
                                                    .flex()
                                                    .flex_row()
                                                    .gap(zpx(4.))
                                                    .children(labels.iter().enumerate().map(
                                                        |(lx, label)| {
                                                            label_chip(
                                                                ("issue-row-label", ix * 8 + lx),
                                                                &label.name,
                                                                &label.color,
                                                                false,
                                                                cx,
                                                            )
                                                            .h(zpx(16.))
                                                            .line_height(zpx(14.))
                                                        },
                                                    )),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .mt(zpx(2.))
                                            .min_w_0()
                                            .truncate()
                                            .text_size(FONT_SIZE_SM())
                                            .line_height(zpx(16.5))
                                            .text_color(secondary)
                                            .child(byline(row, now)),
                                    ),
                            )
                    })
                    .collect()
            })
            .track_scroll(&self.scroll)
            .flex_1()
            .min_h_0()
            .with_scrollbar_handle(&self.scroll)
            .into_any_element()
        };
        let filters = self.filters(id, &issues, window, cx);
        let labels = self.labels(id, &issues, cx);
        let error_line = issues
            .error
            .clone()
            .filter(|_| !issues.rows().is_empty())
            .map(|message| {
                div()
                    .flex_none()
                    .px(SPACING())
                    .pb(SPACING_HALF())
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.dialog_error)
                    .truncate()
                    .child(format!("Could not refresh: {message}"))
            });

        let view = cx.entity().downgrade();
        let t = cx.ghd();
        div()
            .id("issues-list")
            .track_focus(&self.focus)
            .on_key_down(move |ev, _, cx| {
                let down = match ev.keystroke.key.as_str() {
                    "escape" => {
                        cx.stop_propagation();
                        Dispatcher::close_issues(id, cx);
                        return;
                    }
                    "down" => true,
                    "up" => false,
                    _ => return,
                };
                cx.stop_propagation();
                view.update(cx, |this, cx| this.move_selection(id, down, cx))
                    .ok();
            })
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .bg(t.background)
            .child(header)
            .when(!issues.signed_out, |d| d.child(filters))
            .when_some(labels.filter(|_| !issues.signed_out), |d, labels| {
                d.child(labels)
            })
            .when_some(error_line, |d, line| d.child(line))
            .child(body)
            .into_any_element()
    }
}
