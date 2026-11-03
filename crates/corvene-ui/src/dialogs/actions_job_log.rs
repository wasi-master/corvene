//! An Actions job's log in the app (Corvene addition, flag
//! `347-actions-job-logs`): opened from the check-run popover
//! ([`crate::ci_check_popover`]) for a job with steps. GHD has no such
//! dialog; its step rows link to the job's page on GitHub, which stays as
//! "Open on GitHub" here.
//!
//! Header: the check's status, name, workflow and duration, then a toolbar
//! with the search box (Enter / Shift-Enter step through the hits, as the
//! diff view's search), the hit count, Jump to failure (cycles through the
//! `##[error]` lines), Copy (the cleaned log) and Open on GitHub. The log is
//! a uniform-height list of lines, numbered, with `##[group]` lines bold,
//! `##[error]` lines on a red tint, `##[warning]` on a yellow one and the
//! search hits highlighted. Lines do not wrap (long ones are cut; Copy has
//! them whole). On open the list scrolls to the step clicked, else to the
//! first error.

use std::sync::Arc;

use corvene_core::job_log::{JobLog, JobLogState, LineKind, job_log_key};
use corvene_core::{AppState, Dispatcher, GitHubRepository, Popup, RefCheck};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::ci_status::ci_status;
use crate::copy_button::{COPIED_DURATION, CopyButton, copy_button_element};
use crate::dialog::{DialogButton, DialogFrame, dialog_with_frame};
use crate::icons::{Octicon, octicon, spin};
use crate::scrollbar::ScrollbarExt;
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, c, mono_font, primer};
use crate::widgets::{IconButtonA11y, button, filter_text_box, link_button};

/// One log line's height (12 px monospace, `line-height: 1.5`).
#[allow(non_snake_case)]
fn ROW_HEIGHT() -> Pixels {
    zpx(18.)
}

/// The log box's height.
#[allow(non_snake_case)]
fn LOG_HEIGHT() -> Pixels {
    zpx(440.)
}

/// Lines of context kept above the line jumped to.
const JUMP_CONTEXT: usize = 3;

pub struct ActionsJobLogDialog {
    state: Entity<AppState>,
    github: GitHubRepository,
    check: RefCheck,
    /// The step clicked (the API's step name); `None` jumps to the failure.
    step: Option<String>,
    search: Entity<InputState>,
    list: ListState,
    /// The log shown, once loaded (compared by pointer against the store).
    log: Option<Arc<JobLog>>,
    query: String,
    /// Lines matching `query`, and the one shown.
    hits: Vec<usize>,
    current_hit: Option<usize>,
    /// Which `##[error]` line Jump to failure goes to next.
    error_cursor: usize,
    copy: CopyButton,
}

impl ActionsJobLogDialog {
    pub fn new(
        state: Entity<AppState>,
        github: GitHubRepository,
        check: RefCheck,
        step: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search log"));
        cx.subscribe(&search, |this, input, ev: &InputEvent, cx| match ev {
            InputEvent::Change => {
                let text = input.read(cx).value().to_string();
                this.set_query(text, cx);
            }
            InputEvent::PressEnter { shift, .. } => this.step_hit(!*shift, cx),
            _ => {}
        })
        .detach();
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let handle = search.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        Self {
            state,
            copy: CopyButton::new(String::new(), format!("Copy the log of {}", check.name)),
            github,
            check,
            step,
            search,
            list: ListState::new(0, ListAlignment::Top, zpx(200.)),
            log: None,
            query: String::new(),
            hits: Vec::new(),
            current_hit: None,
            error_cursor: 0,
        }
    }

    fn close(cx: &mut App) {
        Dispatcher::close_popup_if(|p| matches!(p, Popup::ActionsJobLog { .. }), cx);
    }

    /// Pick up the store's log when it changed: reset the list, redo the
    /// search and scroll to the step or the failure.
    fn sync_log(&mut self, cx: &mut Context<Self>) {
        let key = job_log_key(&self.github, self.check.id);
        let loaded = match self.state.read(cx).job_logs.entries.get(&key) {
            Some(JobLogState::Loaded(log)) => Some(log.clone()),
            _ => None,
        };
        let same = match (&self.log, &loaded) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        self.log = loaded;
        let count = self.log.as_ref().map(|l| l.lines.len()).unwrap_or(0);
        self.list.reset_with_uniform_height(count, ROW_HEIGHT());
        self.error_cursor = 0;
        self.recompute_hits();
        if let Some(log) = &self.log {
            // the step's group line, else the first error (then the next
            // Jump to failure goes to the second)
            let target = log.jump_target(self.step.as_deref());
            if let Some(line) = target {
                if log.errors.first() == Some(&line) {
                    self.error_cursor = 1 % log.errors.len().max(1);
                }
                self.scroll_to_line(line);
            }
        }
        cx.notify();
    }

    fn scroll_to_line(&self, line: usize) {
        self.list.scroll_to(ListOffset {
            item_ix: line.saturating_sub(JUMP_CONTEXT),
            offset_in_item: px(0.),
        });
    }

    fn recompute_hits(&mut self) {
        self.hits = self
            .log
            .as_ref()
            .map(|log| log.search(&self.query))
            .unwrap_or_default();
        self.current_hit = (!self.hits.is_empty()).then_some(0);
    }

    fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        if self.query == query {
            return;
        }
        self.query = query;
        self.recompute_hits();
        if let Some(ix) = self.current_hit {
            self.scroll_to_line(self.hits[ix]);
        }
        cx.notify();
    }

    /// Enter / Shift-Enter: the next / previous hit, wrapping.
    fn step_hit(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(ix) = self.current_hit else { return };
        let n = self.hits.len();
        let next = if forward {
            (ix + 1) % n
        } else {
            (ix + n - 1) % n
        };
        self.current_hit = Some(next);
        self.scroll_to_line(self.hits[next]);
        cx.notify();
    }

    /// Jump to failure: the next `##[error]` line, wrapping.
    fn jump_to_failure(&mut self, cx: &mut Context<Self>) {
        let Some(log) = &self.log else { return };
        if log.errors.is_empty() {
            return;
        }
        let line = log.errors[self.error_cursor % log.errors.len()];
        self.error_cursor = (self.error_cursor + 1) % log.errors.len();
        self.scroll_to_line(line);
        cx.notify();
    }

    fn copy_log(&mut self, cx: &mut Context<Self>) {
        let Some(log) = &self.log else { return };
        self.copy = CopyButton::new(log.plain_text(), self.copy.accessible_name().to_string());
        let text = self.copy.click(std::time::Instant::now());
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        cx.notify();
        cx.spawn(async |this, cx| {
            cx.background_executor().timer(COPIED_DURATION).await;
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
    }

    /// The job's page on GitHub (the step's anchor when one was clicked).
    fn github_url(&self) -> String {
        let base = self.check.html_url.clone().unwrap_or_else(|| {
            format!(
                "{}/actions/runs/{}",
                self.github.html_url,
                self.check
                    .actions_workflow
                    .as_ref()
                    .map(|w| w.id)
                    .unwrap_or_default()
            )
        });
        let step_number = self.step.as_deref().and_then(|name| {
            self.check
                .job_steps
                .as_ref()?
                .iter()
                .find(|s| s.name == name)
                .map(|s| s.number)
        });
        match step_number {
            Some(number) => format!("{base}/#step:{number}:1"),
            None => base,
        }
    }

    /// The header: status, name, workflow and duration.
    fn header(&self, cx: &Context<Self>) -> Div {
        let t = cx.ghd();
        let workflow = self
            .check
            .actions_workflow
            .as_ref()
            .map(|w| format!("{} ({})", w.name, w.event));
        let duration = self.check.job_steps.as_ref().and_then(|steps| {
            let start = steps.iter().filter_map(|s| s.started_at.as_deref()).min()?;
            let end = steps
                .iter()
                .filter_map(|s| s.completed_at.as_deref())
                .max()?;
            corvene_core::check_duration_ms(Some(start), Some(end))
                .map(corvene_core::format_precise_duration)
        });
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING())
            .px(SPACING_DOUBLE())
            .pt(SPACING_DOUBLE())
            .pb(SPACING())
            .child(ci_status(self.check.status, self.check.conclusion))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.check.name.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(FONT_SIZE_SM())
                            .text_color(t.text_secondary)
                            .child(match (workflow, duration) {
                                (Some(w), Some(d)) => format!("{w} · {d}"),
                                (Some(w), None) => w,
                                (None, Some(d)) => d,
                                (None, None) => self.check.description.clone(),
                            }),
                    ),
            )
    }

    /// The toolbar: search, hit count, Jump to failure, Copy, Open on GitHub.
    fn toolbar(&self, window: &Window, cx: &Context<Self>) -> Div {
        let t = cx.ghd();
        let loaded = self.log.is_some();
        let errors = self.log.as_ref().map(|l| l.errors.len()).unwrap_or(0);
        let hit_count: Option<String> = if self.query.is_empty() {
            None
        } else if self.hits.is_empty() {
            Some("No matches".into())
        } else {
            Some(format!(
                "{} of {}",
                self.current_hit.map(|i| i + 1).unwrap_or(0),
                self.hits.len()
            ))
        };
        let entity = cx.entity().downgrade();
        let jump = entity.clone();
        let copy_entity = entity.clone();
        let url = self.github_url();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING())
            .px(SPACING_DOUBLE())
            .pb(SPACING())
            .child(div().flex_1().min_w_0().child(filter_text_box(
                "job-log-search",
                &self.search,
                Some(octicon(Octicon::Search, t.text_secondary)),
                window,
                cx,
            )))
            .when_some(hit_count, |d, count| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(FONT_SIZE_SM())
                        .text_color(t.text_secondary)
                        .child(count),
                )
            })
            .when(!self.hits.is_empty(), |d| {
                let (prev, next) = (entity.clone(), entity.clone());
                d.child(
                    small_icon_button("job-log-prev-hit", Octicon::ChevronUp, "Previous match", cx)
                        .on_click(move |_, _, cx| {
                            prev.update(cx, |this, cx| this.step_hit(false, cx)).ok();
                        }),
                )
                .child(
                    small_icon_button("job-log-next-hit", Octicon::ChevronDown, "Next match", cx)
                        .on_click(move |_, _, cx| {
                            next.update(cx, |this, cx| this.step_hit(true, cx)).ok();
                        }),
                )
            })
            .child(
                button("job-log-jump", "", cx)
                    .flex_none()
                    .gap(SPACING_HALF())
                    .when(errors == 0, |d| d.opacity(0.6))
                    .child(octicon(
                        Octicon::XCircleFill,
                        if errors == 0 {
                            t.secondary_button_text
                        } else {
                            c(primer::RED_500)
                        },
                    ))
                    .child("Jump to failure")
                    .on_click(move |_, _, cx| {
                        jump.update(cx, |this, cx| this.jump_to_failure(cx)).ok();
                    }),
            )
            .child(
                copy_button_element(
                    "job-log-copy",
                    &self.copy,
                    zpx(16.),
                    if loaded {
                        t.secondary_button_text
                    } else {
                        t.text_secondary_muted
                    },
                    move |_, _, cx| {
                        copy_entity.update(cx, |this, cx| this.copy_log(cx)).ok();
                    },
                )
                .flex_none()
                .h(zpx(25.))
                .px(SPACING())
                .flex()
                .items_center()
                .rounded(BORDER_RADIUS())
                .border_1()
                .border_color(t.secondary_button_border)
                .bg(t.secondary_button_background),
            )
            .child(
                link_button("job-log-open-github", "Open on GitHub", cx)
                    .flex_none()
                    .on_click(move |_, _, cx| Dispatcher::open_url(&url, cx)),
            )
    }

    fn line_row(&self, ix: usize, cx: &App) -> AnyElement {
        let t = cx.ghd();
        let Some(log) = &self.log else {
            return div().into_any_element();
        };
        let Some(line) = log.lines.get(ix) else {
            return div().into_any_element();
        };
        let is_current = self.current_hit.is_some_and(|h| self.hits[h] == ix);
        let is_hit = !is_current && self.hits.binary_search(&ix).is_ok();
        let (color, weight, tint): (Hsla, FontWeight, Option<Hsla>) = match line.kind {
            LineKind::Text => (t.text, FontWeight::NORMAL, None),
            LineKind::Group | LineKind::Section => (t.text, FontWeight::BOLD, None),
            LineKind::EndGroup => (t.text_secondary_muted, FontWeight::NORMAL, None),
            LineKind::Error => (
                c(primer::RED_500),
                FontWeight::SEMIBOLD,
                Some(c(primer::RED_500).opacity(0.12)),
            ),
            LineKind::Warning => (
                c(primer::YELLOW_700_DARKEN_10),
                FontWeight::NORMAL,
                Some(c(primer::YELLOW_700_DARKEN_10).opacity(0.12)),
            ),
            LineKind::Notice => (t.link, FontWeight::NORMAL, None),
            LineKind::Command => (t.text_secondary, FontWeight::NORMAL, None),
            LineKind::Debug => (t.text_secondary_muted, FontWeight::NORMAL, None),
        };
        let bg = if is_current {
            Some(t.accent.opacity(0.35))
        } else if is_hit {
            Some(c(primer::YELLOW_700_DARKEN_10).opacity(0.25))
        } else {
            tint
        };
        let text: AnyElement = if is_hit || is_current {
            // the first occurrence highlighted
            let lower = line.text.to_lowercase();
            let query = self.query.to_lowercase();
            match lower.find(&query) {
                Some(start) if !query.is_empty() => {
                    let end = start + query.len();
                    let (before, rest) = line.text.split_at(start.min(line.text.len()));
                    let (hit, after) = rest.split_at((end - start).min(rest.len()));
                    div()
                        .flex()
                        .flex_row()
                        .whitespace_nowrap()
                        .child(before.to_string())
                        .child(
                            div()
                                .bg(c(primer::YELLOW_700_DARKEN_10).opacity(0.6))
                                .rounded(zpx(2.))
                                .child(hit.to_string()),
                        )
                        .child(after.to_string())
                        .into_any_element()
                }
                _ => div().child(line.text.clone()).into_any_element(),
            }
        } else {
            div().child(line.text.clone()).into_any_element()
        };
        div()
            .h(ROW_HEIGHT())
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .when_some(bg, |d, bg| d.bg(bg))
            .child(
                div()
                    .flex_none()
                    .w(zpx(52.))
                    .pr(SPACING())
                    .text_right()
                    .text_color(t.text_secondary_muted)
                    .child((ix + 1).to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(color)
                    .font_weight(weight)
                    .child(text),
            )
            .into_any_element()
    }

    /// The log box: the lines, or the loading / failed state.
    fn body(&self, cx: &Context<Self>) -> Stateful<Div> {
        let t = cx.ghd();
        let key = job_log_key(&self.github, self.check.id);
        let state = self.state.read(cx).job_logs.entries.get(&key).cloned();
        let frame = div()
            .id("job-log-box")
            .mx(SPACING_DOUBLE())
            .mb(SPACING_DOUBLE())
            .h(LOG_HEIGHT())
            .border_1()
            .border_color(t.box_border)
            .rounded(BORDER_RADIUS())
            .bg(t.box_alt_background)
            .overflow_hidden();
        match (state, &self.log) {
            (Some(JobLogState::Loaded(_)), Some(_)) => {
                let weak = cx.entity().downgrade();
                frame.font_family(mono_font()).text_size(zpx(12.)).child(
                    div()
                        .id("job-log-lines")
                        .role(Role::List)
                        .aria_label(format!("Log of {}", self.check.name))
                        .size_full()
                        .child(
                            list(self.list.clone(), move |ix, _, cx| match weak.upgrade() {
                                Some(this) => this.read(cx).line_row(ix, cx),
                                None => div().into_any_element(),
                            })
                            .size_full(),
                        )
                        .with_scrollbar(),
                )
            }
            (Some(JobLogState::Failed(message)), _) => frame
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .p(SPACING_DOUBLE())
                .gap(SPACING())
                .text_center()
                .child(octicon(Octicon::Alert, t.text_secondary).size(zpx(24.)))
                .child(div().text_color(t.text_secondary).child(message)),
            _ => frame
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(SPACING())
                .child(spin(
                    octicon(Octicon::SyncClockwise, t.text_secondary).size(zpx(24.)),
                    "job-log-loading",
                ))
                .child(
                    div()
                        .text_color(t.text_secondary)
                        .child("Fetching the log…"),
                ),
        }
    }
}

/// A square transparent icon button for the toolbar.
fn small_icon_button(
    id: &'static str,
    icon: Octicon,
    label: &'static str,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.ghd();
    let hover_bg = t.box_selected_background;
    div()
        .id(id)
        .flex_none()
        .p(SPACING_HALF())
        .rounded(BORDER_RADIUS())
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
        .icon_button_label(label)
        .child(octicon(icon, t.text_secondary))
}

impl Render for ActionsJobLogDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_log(cx);
        let close = |_: &mut Window, cx: &mut App| Self::close(cx);
        let content = crate::dialog::own_enter(div())
            .flex()
            .flex_col()
            .child(self.header(cx))
            .child(self.toolbar(window, cx))
            .child(self.body(cx));
        dialog_with_frame(
            "actions-job-log",
            "Job log",
            content,
            vec![DialogButton {
                id: "job-log-close",
                label: "Close".into(),
                primary: true,
                disabled: false,
                on_click: Box::new(close),
            }],
            DialogFrame {
                content_padding: false,
                ..Default::default()
            },
            close,
            window,
            cx,
        )
    }
}
