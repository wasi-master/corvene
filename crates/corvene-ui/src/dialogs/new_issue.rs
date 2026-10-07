//! Corvene `345-issues`: New Issue…, a Corvene extra with no GitHub Desktop
//! counterpart (GHD's Create Issue on GitHub opens the browser's form).
//! A title, a Markdown description, and the repository's labels and
//! assignable users as two filterable checkbox lists (read through the API
//! when the dialog opens, `Dispatcher::load_issue_meta`). The description
//! has github.com's Write / Preview tabs. Create Issue
//! posts it (`Dispatcher::create_issue`); ⏎ in the title and ⌘⏎ in the
//! description submit too.

use std::collections::BTreeSet;

use corvene_core::{AppState, Dispatcher};
use corvene_github::NewIssue;
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::context_menu::mac_or;
use crate::dialog::{DialogButton, dialog};
use crate::github_list::label_chip;
use crate::icons::{Octicon, octicon, spin};
use crate::scrollbar::ScrollbarExt;
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::{checkbox, labeled, text_box};

pub struct NewIssueDialog {
    state: Entity<AppState>,
    repo: u64,
    title: Entity<InputState>,
    body: Entity<TextareaState>,
    label_filter: Entity<InputState>,
    assignee_filter: Entity<InputState>,
    labels: BTreeSet<String>,
    assignees: BTreeSet<String>,
    /// The description's Preview tab is showing.
    preview: bool,
}

impl NewIssueDialog {
    pub fn new(
        state: Entity<AppState>,
        repo: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let title = cx.new(|cx| InputState::new(window, cx).placeholder("Title"));
        let body = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Add a description…")
                .rows(6)
        });
        let label_filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter labels"));
        let assignee_filter =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter assignees"));
        for input in [&title, &label_filter, &assignee_filter] {
            cx.observe(input, |_, _, cx| cx.notify()).detach();
        }
        cx.observe(&body, |_, _, cx| cx.notify()).detach();
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let handle = title.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.subscribe(&title, |this, _, ev: &InputEvent, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.submit(cx);
            }
        })
        .detach();
        cx.subscribe(&body, |this, _, ev: &InputEvent, cx| {
            if let InputEvent::PressEnter {
                secondary: true, ..
            } = ev
            {
                this.submit(cx);
            }
        })
        .detach();
        Dispatcher::load_issue_meta(repo, cx);
        Self {
            state,
            repo,
            title,
            body,
            label_filter,
            assignee_filter,
            labels: BTreeSet::new(),
            assignees: BTreeSet::new(),
            preview: false,
        }
    }

    fn toggle_label(&mut self, name: String, cx: &mut Context<Self>) {
        if !self.labels.remove(&name) {
            self.labels.insert(name);
        }
        cx.notify();
    }

    fn toggle_assignee(&mut self, login: String, cx: &mut Context<Self>) {
        if !self.assignees.remove(&login) {
            self.assignees.insert(login);
        }
        cx.notify();
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let title = self.title.read(cx).value().trim().to_string();
        if title.is_empty() {
            return;
        }
        let new = NewIssue {
            title,
            body: self.body.read(cx).value().trim().to_string(),
            labels: self.labels.iter().cloned().collect(),
            assignees: self.assignees.iter().cloned().collect(),
        };
        Dispatcher::create_issue_via_api(self.repo, new, cx);
    }

    /// A 140 px checkbox list with its filter box above.
    #[allow(clippy::too_many_arguments)]
    fn pick_list(
        &self,
        key: &'static str,
        heading: &'static str,
        filter: &Entity<InputState>,
        items: Vec<(String, AnyElement)>,
        ticked: &BTreeSet<String>,
        loading: bool,
        on_toggle: impl Fn(String, &mut App) + Clone + 'static,
        window: &Window,
        cx: &Context<Self>,
    ) -> Div {
        let t = cx.ghd().clone();
        let hover_bg = t.list_item_hover_background;
        let empty = if loading {
            None
        } else if items.is_empty() {
            Some(format!("No {}", heading.to_lowercase()))
        } else {
            None
        };
        let list = div()
            .id(key)
            .h(zpx(140.))
            .overflow_y_scroll()
            .border_1()
            .border_color(t.box_border)
            .rounded(BORDER_RADIUS())
            .flex()
            .flex_col()
            .when(loading, |d| {
                d.child(
                    div()
                        .p(SPACING_HALF())
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(SPACING_HALF())
                        .text_color(t.text_secondary)
                        .child(spin(
                            octicon(Octicon::SyncClockwise, t.text_secondary),
                            SharedString::from(format!("{key}-spin")),
                        ))
                        .child("Loading…"),
                )
            })
            .when_some(empty, |d, text| {
                d.child(
                    div()
                        .p(SPACING_HALF())
                        .text_color(t.text_secondary)
                        .child(text),
                )
            })
            .children(items.into_iter().enumerate().map(|(ix, (name, label))| {
                let is_ticked = ticked.contains(&name);
                let on_toggle = on_toggle.clone();
                div()
                    .id((key, ix))
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING_HALF())
                    .px(SPACING_HALF())
                    .py(zpx(3.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover_bg))
                    .on_click(move |_, _, cx| on_toggle(name.clone(), cx))
                    .child(checkbox((key, 1000 + ix), is_ticked, false, cx))
                    .child(label)
            }))
            .with_scrollbar();
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(SPACING_THIRD())
            .child(div().child(heading))
            .child(crate::widgets::filter_text_box(
                SharedString::from(format!("{key}-filter")),
                filter,
                Some(octicon(Octicon::Search, t.text_secondary)),
                window,
                cx,
            ))
            .child(list)
    }
}

impl Render for NewIssueDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd().clone();
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let (labels, assignees, loading, repo_name, html_url) = {
            let s = self.state.read(cx);
            let issues = s
                .repo_states
                .get(&self.repo)
                .and_then(|rs| rs.issues.as_ref());
            (
                issues.map(|i| i.labels.clone()).unwrap_or_default(),
                issues.map(|i| i.assignees.clone()).unwrap_or_default(),
                issues.is_some_and(|i| i.meta_loading),
                s.repository(self.repo)
                    .and_then(|r| r.non_fork_github())
                    .map(|gh| gh.full_name())
                    .unwrap_or_default(),
                s.repository(self.repo)
                    .and_then(|r| r.non_fork_github())
                    .map(|gh| gh.html_url.clone()),
            )
        };
        let label_query = self.label_filter.read(cx).value().trim().to_string();
        let assignee_query = self.assignee_filter.read(cx).value().trim().to_string();
        let matches = |query: &str, text: &str| {
            query.is_empty() || corvene_core::filter::fuzzy_match(query, text).is_some()
        };
        let label_items: Vec<(String, AnyElement)> = labels
            .iter()
            .filter(|l| matches(&label_query, &l.name))
            .enumerate()
            .map(|(ix, l)| {
                (
                    l.name.clone(),
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(SPACING_HALF())
                        .child(label_chip(
                            ("new-issue-label-chip", ix),
                            &l.name,
                            &l.color,
                            false,
                            cx,
                        ))
                        .when_some(l.description.clone(), |d, desc| {
                            d.child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(FONT_SIZE_SM())
                                    .text_color(t.text_secondary)
                                    .child(desc),
                            )
                        })
                        .into_any_element(),
                )
            })
            .collect();
        let assignee_items: Vec<(String, AnyElement)> = assignees
            .iter()
            .filter(|a| matches(&assignee_query, a))
            .map(|a| {
                (
                    a.clone(),
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(SPACING_HALF())
                        .child(octicon(Octicon::Person, t.text_secondary).flex_none())
                        .child(div().min_w_0().truncate().child(a.clone()))
                        .into_any_element(),
                )
            })
            .collect();
        let this = cx.weak_entity();
        let toggle_label = {
            let this = this.clone();
            move |name: String, cx: &mut App| {
                this.update(cx, |d, cx| d.toggle_label(name, cx)).ok();
            }
        };
        let toggle_assignee = {
            let this = this.clone();
            move |login: String, cx: &mut App| {
                this.update(cx, |d, cx| d.toggle_assignee(login, cx)).ok();
            }
        };
        let labels_list = self.pick_list(
            "new-issue-labels",
            "Labels",
            &self.label_filter,
            label_items,
            &self.labels,
            loading,
            toggle_label,
            window,
            cx,
        );
        let assignees_list = self.pick_list(
            "new-issue-assignees",
            "Assignees",
            &self.assignee_filter,
            assignee_items,
            &self.assignees,
            loading,
            toggle_assignee,
            window,
            cx,
        );
        let title_empty = self.title.read(cx).value().trim().is_empty();
        let content = div()
            .w(crate::theme::fit_width(560.))
            .flex()
            .flex_col()
            .gap(SPACING())
            .when(!repo_name.is_empty(), |d| {
                d.child(
                    div()
                        .text_color(t.text_secondary)
                        .child(format!("A new issue in {repo_name}.")),
                )
            })
            .child(labeled(
                "Title",
                text_box("new-issue-title", &self.title, None, window, cx),
                cx,
            ))
            .child(labeled(
                "Description",
                crate::markdown_editor::markdown_editor(
                    "new-issue-body",
                    &self.body,
                    self.preview,
                    html_url.as_deref(),
                    zpx(110.),
                    {
                        let this = this.clone();
                        move |preview, _, cx| {
                            this.update(cx, |d, cx| {
                                d.preview = preview;
                                cx.notify();
                            })
                            .ok();
                        }
                    },
                    cx,
                ),
                cx,
            ))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(SPACING())
                    .child(labels_list)
                    .child(assignees_list),
            );
        dialog(
            "dialog-new-issue",
            mac_or("New Issue", "New issue"),
            content,
            vec![
                DialogButton {
                    id: "new-issue-cancel",
                    label: "Cancel".into(),
                    primary: false,
                    disabled: false,
                    on_click: Box::new(close),
                },
                DialogButton {
                    id: "new-issue-create",
                    label: mac_or("Create Issue", "Create issue").into(),
                    primary: true,
                    disabled: title_empty,
                    on_click: Box::new(move |_, cx| {
                        if !title_empty {
                            this.update(cx, |d, cx| d.submit(cx)).ok();
                        }
                    }),
                },
            ],
            close,
            window,
            cx,
        )
    }
}
