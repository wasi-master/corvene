//! Corvene `798-blame`: the Blame view, shown in place of the diff of the
//! tab it was opened from (`corvene_core::blame`). GitHub Desktop has no
//! blame.
//!
//! A header names the file and revision with Back (after "Blame Previous
//! Revision"), the options menu and Close. Below it every line of the file,
//! syntax highlighted like the diff, with a gutter cell per run of lines
//! from one commit: short SHA, author and age on its first line, the
//! summary on the second. Hovering a run highlights it and shows the
//! summary; clicking selects the commit in History; its context menu can
//! blame the revision before it. Rows are virtualized, so a large file
//! draws only what is on screen while git is still attributing lines.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use corvene_core::blame::BlameState;
use corvene_core::{AppState, Dispatcher};
use corvene_git::BlameOptions;
use corvene_highlight::Span;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::context_menu::MenuItem;
use crate::diff_view::DIFF_LINE_HEIGHT;
use crate::diff_view_rows::{expand_tabs, line_number_width, spans_for_row, token_color};
use crate::icons::{Octicon, octicon};
use crate::scrollbar::ScrollbarExt;
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, GhdTheme, mono_font};
use crate::widgets::{GhdTooltip, IconButtonA11y};

/// The gutter's width in CSS px.
const GUTTER_WIDTH: f32 = 300.;

/// What the highlighted lines were computed from.
#[derive(Clone, PartialEq)]
struct HighlightKey {
    path: String,
    /// The lines' allocation and count.
    contents: (usize, usize),
    engine: (corvene_highlight::Engine, u64),
}

pub struct BlameView {
    state: Entity<AppState>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    /// Syntax spans per line of the shown contents.
    spans: Option<(HighlightKey, Arc<Vec<Vec<Span>>>)>,
    highlighting: Option<(HighlightKey, Task<()>)>,
    /// The load whose target line was scrolled to.
    scrolled_for: Option<u64>,
    /// The commit (index) whose lines the pointer is over.
    hovered: Option<u32>,
}

impl BlameView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle().tab_stop(true),
            spans: None,
            highlighting: None,
            scrolled_for: None,
            hovered: None,
        }
    }

    /// Highlight `lines` in the background, once per file and highlighter
    /// (lines past `MAX_HIGHLIGHT_BYTES` stay plain, as in the diff).
    fn highlight(&mut self, path: &str, lines: &Arc<Vec<String>>, cx: &mut Context<Self>) {
        let engine = crate::diff_view::highlight_engine(self.state.read(cx));
        let key = HighlightKey {
            path: path.to_string(),
            contents: (Arc::as_ptr(lines) as usize, lines.len()),
            engine,
        };
        if self.spans.as_ref().is_some_and(|(k, _)| *k == key)
            || self.highlighting.as_ref().is_some_and(|(k, _)| *k == key)
        {
            return;
        }
        let (lines, path) = (lines.clone(), path.to_string());
        let work = cx.background_executor().spawn(async move {
            corvene_highlight::highlight_lines_with(
                engine.0,
                &path,
                lines.iter().map(String::as_str),
            )
        });
        let task_key = key.clone();
        let task = cx.spawn(async move |this, cx| {
            let spans = work.await;
            this.update(cx, |this, cx| {
                this.highlighting = None;
                this.spans = spans.map(|s| (task_key, Arc::new(s)));
                cx.notify();
            })
            .ok();
        });
        self.highlighting = Some((key, task));
    }

    fn header(&self, id: u64, blame: &BlameState, cx: &Context<Self>) -> Div {
        let t = cx.ghd();
        let revision = match (&blame.target.rev, &blame.commit) {
            (None, _) => "Working tree".to_string(),
            (Some(_), Some(sha)) => format!("at {}", &sha[..sha.len().min(7)]),
            (Some(rev), None) => format!("at {rev}"),
        };
        let progress = blame.loading.then(|| {
            let (attributed, total) = (blame.result.attributed(), blame.result.lines.len());
            match (attributed * 100).checked_div(total) {
                Some(percent) => (
                    format!("Blaming… {percent}%"),
                    Some(attributed as f32 / total as f32),
                ),
                None => ("Blaming…".to_string(), None),
            }
        });
        let options = self
            .state
            .read(cx)
            .repo_states
            .get(&id)
            .map(|rs| rs.blame_options);
        let button = |el: Stateful<Div>| {
            el.h(zpx(19.))
                .px(SPACING_HALF())
                .flex()
                .flex_row()
                .items_center()
                .gap(zpx(2.))
                .rounded(BORDER_RADIUS())
                .cursor_pointer()
                .text_size(FONT_SIZE_SM())
                .text_color(t.text)
                .hover(|d| d.bg(t.box_hover_background))
        };
        div()
            .h(zpx(30.))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING_HALF())
            .px(SPACING())
            .bg(t.box_alt_background)
            .border_b_1()
            .border_color(t.diff_border)
            .text_size(FONT_SIZE())
            .when(!blame.back.is_empty(), |d| {
                d.child(
                    button(div().id("blame-back"))
                        .a11y_button("Back")
                        .child("‹ Back")
                        .on_click(move |_, _, cx| Dispatcher::blame_back(id, cx)),
                )
            })
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Blame"),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_shrink(1.)
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(blame.target.path.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(t.text_secondary)
                    .child(revision),
            )
            .child(div().flex_1())
            .children(progress.map(|(text, fraction)| {
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING_HALF())
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .children(fraction.map(|f| {
                        crate::progress_bar::progress_bar(
                            format!("blame-{id}"),
                            f,
                            t.link,
                            zpx(80.),
                        )
                    }))
                    .child(text)
            }))
            .children(options.map(|options| {
                button(div().id("blame-options"))
                    .icon_button_label("Blame Options")
                    .child(octicon(Octicon::Gear, t.text))
                    .child(octicon(Octicon::TriangleDown, t.text))
                    .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                        cx.stop_propagation();
                        options_menu(id, options, ev.position, window, cx);
                    })
            }))
            .child(
                button(div().id("blame-close"))
                    .icon_button_label("Close Blame")
                    .child(octicon(Octicon::X, t.text))
                    .on_click(move |_, _, cx| Dispatcher::close_blame(id, cx)),
            )
    }

    fn message(text: String, t: &GhdTheme) -> Div {
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_color(t.text_secondary)
            .child(text)
    }
}

/// The options as a menu of checkboxes; a change blames the file again.
fn options_menu(
    id: u64,
    options: BlameOptions,
    position: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let item = |label: &str, checked: bool, set: fn(&mut BlameOptions, bool)| {
        MenuItem::checkbox(label.to_string(), checked, move |_, cx| {
            let mut next = options;
            set(&mut next, !checked);
            Dispatcher::set_blame_options(id, next, cx);
        })
    };
    let items = vec![
        item("Ignore Whitespace", options.ignore_whitespace, |o, v| {
            o.ignore_whitespace = v
        }),
        item("Detect Moved Lines", options.detect_moves, |o, v| {
            o.detect_moves = v
        }),
        item(
            "Detect Lines Copied from Other Files",
            options.detect_copies,
            |o, v| o.detect_copies = v,
        ),
        MenuItem::separator(),
        item(
            "Skip Revisions in .git-blame-ignore-revs",
            options.ignore_revs_file,
            |o, v| o.ignore_revs_file = v,
        ),
    ];
    crate::native_menu::show_context_menu(items, position, window, cx);
}

/// A gutter cell's context menu.
#[allow(clippy::too_many_arguments)]
fn gutter_menu(
    id: u64,
    line: usize,
    top_line: u32,
    sha: String,
    has_previous: bool,
    position: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let uncommitted = sha == corvene_git::UNCOMMITTED_SHA;
    let show = sha.clone();
    let items = vec![
        MenuItem::new(
            if uncommitted {
                "Show in Changes"
            } else {
                "Show in History"
            },
            move |_, cx| Dispatcher::show_blamed_commit(id, show.clone(), cx),
        ),
        MenuItem::new("Blame Previous Revision", move |_, cx| {
            Dispatcher::blame_previous_revision(id, line, top_line, cx)
        })
        .enabled(has_previous),
        MenuItem::separator(),
        MenuItem::new("Copy SHA", move |_, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(sha.clone()))
        })
        .enabled(!uncommitted),
    ];
    crate::native_menu::show_context_menu(items, position, window, cx);
}

/// The first row on screen.
fn top_line(scroll: &UniformListScrollHandle) -> u32 {
    let offset = -f32::from(scroll.0.borrow().base_handle.offset().y);
    (offset / f32::from(DIFF_LINE_HEIGHT())).max(0.) as u32
}

/// Highlight styles for `spans` over `text`.
fn highlights(spans: &[Span], t: &GhdTheme) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
    spans
        .iter()
        .map(|s| {
            (
                s.range.clone(),
                HighlightStyle {
                    color: Some(token_color(s.class, t)),
                    ..HighlightStyle::default()
                },
            )
        })
        .collect()
}

impl Render for BlameView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.state.read(cx);
        let Some(id) = s.selected else {
            return div().into_any_element();
        };
        let Some(blame) = s.repo_states.get(&id).and_then(|rs| rs.blame.clone()) else {
            return div().into_any_element();
        };
        let text_size = match s.flags.number(corvene_core::flags::ids::DIFF_FONT_SIZE) {
            0 => FONT_SIZE_SM(),
            size => zpx(size.clamp(9, 16) as f32),
        };
        let t = cx.ghd().clone();
        // a new blame: focus for Escape, then its target line
        if self.scrolled_for != Some(blame.load_id()) && blame.contents.is_some() {
            self.scrolled_for = Some(blame.load_id());
            self.hovered = None;
            let line = blame.target.line.unwrap_or(0) as usize;
            self.scroll.scroll_to_item(line, ScrollStrategy::Top);
            window.focus(&self.focus, cx);
        }

        let blame_for_header = blame.clone();
        let body = match (&blame.message, &blame.contents) {
            (Some(message), _) => Self::message(message.clone(), &t).into_any_element(),
            (None, None) => Self::message("Loading…".to_string(), &t).into_any_element(),
            (None, Some(lines)) => {
                let lines = lines.clone();
                self.highlight(&blame.target.path, &lines, cx);
                let spans = self
                    .spans
                    .as_ref()
                    .filter(|(k, _)| k.contents == (Arc::as_ptr(&lines) as usize, lines.len()))
                    .map(|(_, s)| s.clone());
                self.rows(id, blame, lines, spans, text_size, &t, cx)
                    .into_any_element()
            }
        };

        let header = self.header(id, &blame_for_header, cx);
        div()
            .id("blame-view")
            .track_focus(&self.focus)
            .on_key_down(move |ev, _, cx| {
                if ev.keystroke.key == "escape" {
                    cx.stop_propagation();
                    Dispatcher::close_blame(id, cx);
                }
            })
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .bg(t.background)
            .child(header)
            .child(body)
            .into_any_element()
    }
}

impl BlameView {
    #[allow(clippy::too_many_arguments)]
    fn rows(
        &self,
        id: u64,
        blame: BlameState,
        lines: Arc<Vec<String>>,
        spans: Option<Arc<Vec<Vec<Span>>>>,
        text_size: Pixels,
        t: &GhdTheme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let count = lines.len();
        let number_width = zpx(line_number_width(count as u32));
        let longest = lines
            .iter()
            .enumerate()
            .max_by_key(|(_, l)| l.len() + l.matches('\t').count() * 3)
            .map(|(ix, _)| ix);
        let hovered = self.hovered;
        let view = cx.entity().downgrade();
        let scroll = self.scroll.clone();
        let text_color = t.diff_text;
        let t = t.clone();
        let blame = Arc::new(blame);
        let now = SystemTime::now();
        uniform_list("blame-rows", count, move |range, _window, cx| {
            range
                .map(|ix| {
                    let line = blame.result.lines.get(ix).copied().flatten();
                    let commit = blame.result.commit_of(ix);
                    let starts = blame.starts_group(ix);
                    let second = ix > 0 && !starts && blame.starts_group(ix - 1);
                    let in_hover = line.is_some_and(|l| Some(l.commit) == hovered);
                    let gutter = gutter_cell(
                        ix,
                        commit,
                        starts,
                        second,
                        blame.result.file_of(ix),
                        &blame.target.path,
                        now,
                        &t,
                        cx,
                    );
                    let gutter = {
                        let view = view.clone();
                        let commit_ix = line.map(|l| l.commit);
                        let sha = commit.map(|c| c.sha.clone());
                        let previous = commit.is_some_and(|c| c.previous.is_some());
                        let scroll = scroll.clone();
                        gutter
                            .when(in_hover, |d| d.bg(t.box_hover_background))
                            .on_hover(move |hovering, _, cx| {
                                let next = if *hovering { commit_ix } else { None };
                                view.update(cx, |this, cx| {
                                    if this.hovered != next
                                        && (*hovering || this.hovered == commit_ix)
                                    {
                                        this.hovered = next;
                                        cx.notify();
                                    }
                                })
                                .ok();
                            })
                            .when_some(sha, |d, sha| {
                                let menu_sha = sha.clone();
                                d.cursor_pointer()
                                    .on_click(move |_, _, cx| {
                                        Dispatcher::show_blamed_commit(id, sha.clone(), cx)
                                    })
                                    .on_mouse_down(MouseButton::Right, move |ev, window, cx| {
                                        cx.stop_propagation();
                                        let top = top_line(&scroll);
                                        gutter_menu(
                                            id,
                                            ix,
                                            top,
                                            menu_sha.clone(),
                                            previous,
                                            ev.position,
                                            window,
                                            cx,
                                        );
                                    })
                            })
                    };
                    let raw = &lines[ix];
                    let text = expand_tabs(raw);
                    let styles = spans
                        .as_ref()
                        .and_then(|s| s.get(ix))
                        .map(|s| highlights(&spans_for_row(s.clone(), raw, &text), &t))
                        .unwrap_or_default();
                    div()
                        .id(("blame-row", ix))
                        .min_w_full()
                        .h(DIFF_LINE_HEIGHT())
                        .flex()
                        .flex_row()
                        .when(starts && ix > 0, |d| {
                            d.border_t_1().border_color(t.diff_border)
                        })
                        .child(gutter)
                        .child(
                            div()
                                .flex_none()
                                .w(number_width)
                                .h_full()
                                .flex()
                                .justify_end()
                                .px(SPACING_HALF())
                                .bg(t.diff_gutter_background)
                                .border_r_1()
                                .border_color(t.diff_border)
                                .text_color(t.diff_line_number)
                                .child((ix + 1).to_string()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .pl(SPACING())
                                .pr(SPACING())
                                .whitespace_nowrap()
                                .child(
                                    StyledText::new(SharedString::from(text))
                                        .with_highlights(styles),
                                ),
                        )
                })
                .collect()
        })
        .track_scroll(&self.scroll)
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .with_width_from_item(longest)
        .flex_1()
        .min_h_0()
        .font_family(mono_font())
        .text_size(text_size)
        .line_height(DIFF_LINE_HEIGHT())
        .text_color(text_color)
        .with_scrollbar_handle(&self.scroll)
    }
}

/// The gutter of row `ix`: commit, author and age on a run's first line,
/// its summary on the second, empty below (or while git has not reached
/// the line).
#[allow(clippy::too_many_arguments)]
fn gutter_cell(
    ix: usize,
    commit: Option<&corvene_git::BlameCommit>,
    starts: bool,
    second: bool,
    file: Option<&str>,
    path: &str,
    now: SystemTime,
    t: &GhdTheme,
    cx: &mut App,
) -> Stateful<Div> {
    let cell = div()
        .id(("blame-gutter", ix))
        .flex_none()
        .w(zpx(GUTTER_WIDTH))
        .h_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(SPACING_HALF())
        .px(SPACING_HALF())
        .bg(t.diff_gutter_background)
        .border_r_1()
        .border_color(t.diff_border)
        .font_family(crate::theme::ui_font())
        .text_size(FONT_SIZE_SM())
        .overflow_hidden();
    let Some(commit) = commit else {
        return cell;
    };
    let uncommitted = commit.is_uncommitted();
    // the summary on hover, with the old path across a rename
    let mut tip = if uncommitted {
        "Not committed yet".to_string()
    } else {
        format!(
            "{}\n{} · {}",
            commit.summary,
            commit.author_name,
            commit.short_sha()
        )
    };
    if let Some(file) = file.filter(|f| *f != path && !uncommitted) {
        tip.push_str(&format!("\nas {file}"));
    }
    let cell = cell.ghd_tooltip(tip);
    if starts {
        if uncommitted {
            return cell.child(
                div()
                    .text_color(t.text_secondary)
                    .italic()
                    .child("Not committed yet"),
            );
        }
        Dispatcher::request_avatar_for_email(&commit.author_email, cx);
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(commit.author_time.max(0) as u64);
        let age = if at > now {
            crate::format::format_date(at)
        } else {
            crate::relative_time::relative(at)
        };
        return cell
            .child(crate::widgets::author_avatar(
                &commit.author_name,
                &commit.author_email,
                zpx(14.),
                cx,
            ))
            .child(
                div()
                    .flex_none()
                    .font_family(mono_font())
                    .text_color(t.text_secondary)
                    .child(commit.short_sha().to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_color(t.text)
                    .child(commit.author_name.clone()),
            )
            .child(div().flex_none().text_color(t.text_secondary).child(age));
    }
    if second && !uncommitted {
        return cell.child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .text_color(t.text_secondary)
                .child(commit.summary.clone()),
        );
    }
    cell
}
