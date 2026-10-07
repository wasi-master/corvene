//! Corvene `1110-repository-insights`: Repository › Insights…, in place of
//! the tab's content. A header with the scope (current branch, all
//! branches or one branch) and range pickers, then the totals, the commits
//! of every week as bars (hover one for its numbers), the contributors and
//! the most changed files. GitHub Desktop has no counterpart.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, SystemTime};

use corvene_core::insights::{InsightsRange, InsightsState};
use corvene_core::{AppState, BranchKind, Dispatcher};
use corvene_git::{RepoStats, StatsScope};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::format::{format_count, format_date};
use crate::icons::{Octicon, octicon};
use crate::scrollbar::ScrollbarExt;
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, GhdTheme};
use crate::widgets::{IconButtonA11y, select_button};

/// Files listed before "Show all".
const FILES_SHOWN: usize = 25;
/// Contributors listed before "Show all".
const CONTRIBUTORS_SHOWN: usize = 25;

pub struct InsightsView {
    state: Entity<AppState>,
    scroll: ScrollHandle,
    /// The week under the pointer.
    hovered_week: Option<usize>,
    chart_bounds: Rc<Cell<Bounds<Pixels>>>,
    all_files: bool,
    all_contributors: bool,
}

fn date(seconds: i64) -> String {
    format_date(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds.max(0) as u64))
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", format_count(n), if n == 1 { one } else { many })
}

impl InsightsView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            scroll: ScrollHandle::new(),
            hovered_week: None,
            chart_bounds: Rc::default(),
            all_files: false,
            all_contributors: false,
        }
    }

    fn header(&self, id: u64, insights: &InsightsState, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let s = self.state.read(cx);
        let rs = s.repo_states.get(&id);
        let current = rs
            .and_then(|rs| rs.info.as_ref())
            .and_then(|i| i.current_branch())
            .map(|b| b.name.clone());
        let mut branches: Vec<String> = rs
            .and_then(|rs| rs.info.as_ref())
            .map(|i| {
                i.branches
                    .iter()
                    .filter(|b| b.kind == BranchKind::Local)
                    .map(|b| b.name.clone())
                    .filter(|n| Some(n) != current.as_ref())
                    .collect()
            })
            .unwrap_or_default();
        branches.sort_by_key(|n| n.to_lowercase());
        let mut scopes: Vec<StatsScope> = vec![StatsScope::Head, StatsScope::AllBranches];
        scopes.extend(branches.into_iter().map(StatsScope::Ref));
        if let StatsScope::Ref(name) = &insights.scope
            && !scopes.contains(&insights.scope)
        {
            scopes.push(StatsScope::Ref(name.clone()));
        }
        let scope_label = |scope: &StatsScope| -> SharedString {
            match scope {
                StatsScope::Head => current
                    .as_ref()
                    .map(|c| format!("Current branch ({c})"))
                    .unwrap_or_else(|| "Current branch".into())
                    .into(),
                StatsScope::AllBranches => "All branches".into(),
                StatsScope::Ref(name) => name.clone().into(),
            }
        };
        let scope_ix = scopes.iter().position(|s| *s == insights.scope);
        let scope_options: Vec<SharedString> = scopes.iter().map(scope_label).collect();
        let range_ix = InsightsRange::ALL.iter().position(|r| *r == insights.range);
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
        let scopes = Rc::new(scopes);
        div()
            .flex_none()
            .min_h(zpx(36.))
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(SPACING())
            .px(SPACING())
            .py(SPACING_HALF())
            .border_b_1()
            .border_color(t.box_border)
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(FONT_SIZE())
                    .child("Insights"),
            )
            .child(div().w(zpx(220.)).child(select_button(
                "insights-scope",
                scope_label(&insights.scope),
                scope_options,
                scope_ix,
                false,
                Rc::new(move |ix, _, cx| {
                    if let Some(scope) = scopes.get(ix) {
                        Dispatcher::set_insights_scope(id, scope.clone(), cx);
                    }
                }),
                cx,
            )))
            .child(
                div().w(zpx(140.)).child(select_button(
                    "insights-range",
                    insights.range.label(),
                    InsightsRange::ALL
                        .iter()
                        .map(|r| SharedString::from(r.label()))
                        .collect(),
                    range_ix,
                    false,
                    Rc::new(move |ix, _, cx| {
                        if let Some(range) = InsightsRange::ALL.get(ix) {
                            Dispatcher::set_insights_range(id, *range, cx);
                        }
                    }),
                    cx,
                )),
            )
            .child(div().flex_1())
            .child(if insights.loading {
                button(div().id("insights-stop"))
                    .icon_button_label("Stop reading the history")
                    .child(octicon(Octicon::Stop, t.text))
                    .child("Stop")
                    .on_click(move |_, _, cx| Dispatcher::stop_insights(id, cx))
            } else {
                button(div().id("insights-refresh"))
                    .icon_button_label("Read the history again")
                    .child(octicon(Octicon::Sync, t.text))
                    .on_click(move |_, _, cx| Dispatcher::load_insights(id, cx))
            })
            .child(
                button(div().id("insights-close"))
                    .icon_button_label("Close Insights")
                    .child(octicon(Octicon::X, t.text))
                    .on_click(move |_, _, cx| Dispatcher::close_insights(id, cx)),
            )
            .into_any_element()
    }

    /// The line under the header: progress, a stop, an error or the
    /// numbers being out of date.
    fn status(&self, id: u64, insights: &InsightsState, cx: &Context<Self>) -> Option<AnyElement> {
        let t = cx.ghd();
        let total = insights.progress_total.filter(|_| insights.loading);
        let text: String = if insights.loading {
            match total {
                Some(total) if total > 0 => format!(
                    "Reading the history… {} of {}",
                    crate::format::format_count(insights.progress.min(total)),
                    plural(total, "commit", "commits")
                ),
                _ if insights.progress == 0 => "Reading the history…".into(),
                _ => format!(
                    "Reading the history… {} so far",
                    plural(insights.progress, "commit", "commits")
                ),
            }
        } else if let Some(error) = &insights.error {
            format!("Could not read the history: {error}")
        } else if insights.stopped {
            "Stopped before all commits were read.".into()
        } else if insights.stale {
            "There are new commits since these numbers were read.".into()
        } else {
            return None;
        };
        let refresh = !insights.loading;
        Some(
            div()
                .id("insights-status")
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(SPACING())
                .px(SPACING())
                .h(ROW_HEIGHT())
                .border_b_1()
                .border_color(t.box_border)
                .bg(t.box_alt_background)
                .text_size(FONT_SIZE())
                .text_color(t.text_secondary)
                .when(insights.loading, |d| {
                    d.child(crate::icons::loading("insights-loading", t.text_secondary))
                })
                .child(div().min_w_0().truncate().child(text))
                .children(total.filter(|&n| n > 0).map(|total| {
                    crate::progress_bar::progress_bar(
                        format!("insights-{id}"),
                        insights.progress as f32 / total as f32,
                        t.link,
                        zpx(120.),
                    )
                }))
                .child(div().flex_1())
                .when(refresh, |d| {
                    let link = t.link;
                    d.child(
                        div()
                            .id("insights-status-refresh")
                            .text_color(link)
                            .cursor_pointer()
                            .on_click(move |_, _, cx| Dispatcher::load_insights(id, cx))
                            .child("Refresh"),
                    )
                })
                .into_any_element(),
        )
    }

    fn section_title(title: &str, note: Option<String>, t: &GhdTheme) -> Div {
        div()
            .flex()
            .flex_row()
            .items_baseline()
            .gap(SPACING())
            .mt(SPACING_DOUBLE())
            .mb(SPACING())
            .child(
                div()
                    .text_size(FONT_SIZE_MD())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title.to_string()),
            )
            .children(note.map(|n| {
                div()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(n)
            }))
    }

    fn totals(stats: &RepoStats, t: &GhdTheme) -> Div {
        let tile = |value: String, label: &str, color: Hsla| {
            div()
                .flex_1()
                .min_w(zpx(110.))
                .p(SPACING())
                .border_1()
                .border_color(t.box_border)
                .rounded(BORDER_RADIUS())
                .flex()
                .flex_col()
                .gap(zpx(2.))
                .child(
                    div()
                        .text_size(FONT_SIZE_LG())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color)
                        .child(value),
                )
                .child(
                    div()
                        .text_size(FONT_SIZE_SM())
                        .text_color(t.text_secondary)
                        .child(label.to_string()),
                )
        };
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap(SPACING())
            .child(tile(format_count(stats.commits), "commits", t.text))
            .child(tile(
                format_count(stats.contributors.len() as u64),
                "contributors",
                t.text,
            ))
            .child(tile(
                format!("+{}", format_count(stats.additions)),
                "lines added",
                t.color_new,
            ))
            .child(tile(
                format!("−{}", format_count(stats.deletions)),
                "lines removed",
                t.color_deleted,
            ))
            .child(tile(
                format_count(stats.files_total as u64),
                "files changed",
                t.text,
            ))
    }

    /// Commits per week: one bar a week, scaled to the busiest.
    fn chart(&self, stats: &std::sync::Arc<RepoStats>, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd().clone();
        let weeks = stats.weeks.len();
        let max = stats.weeks.iter().map(|w| w.commits).max().unwrap_or(0);
        let hovered = self.hovered_week.filter(|ix| *ix < weeks);
        let bounds_cell = self.chart_bounds.clone();
        let paint_stats = stats.clone();
        let bar = t.color_new;
        let bar_hover = t.link;
        let grid = t.box_border;
        let chart = canvas(
            move |bounds, _, _| bounds_cell.set(bounds),
            move |bounds, _, window, _| {
                // the baseline
                window.paint_quad(fill(
                    Bounds::new(
                        point(bounds.origin.x, bounds.bottom() - px(1.)),
                        size(bounds.size.width, px(1.)),
                    ),
                    grid,
                ));
                let n = paint_stats.weeks.len();
                if n == 0 || max == 0 {
                    return;
                }
                let slot = bounds.size.width / n as f32;
                let gap = if slot > px(4.) { px(1.) } else { px(0.) };
                let width = (slot - gap).max(px(1.));
                let height = bounds.size.height - px(1.);
                for (ix, week) in paint_stats.weeks.iter().enumerate() {
                    if week.commits == 0 {
                        continue;
                    }
                    let h = (height * (week.commits as f32 / max as f32)).max(px(1.));
                    window.paint_quad(fill(
                        Bounds::new(
                            point(
                                bounds.origin.x + slot * ix as f32,
                                bounds.bottom() - px(1.) - h,
                            ),
                            size(width, h),
                        ),
                        if Some(ix) == hovered { bar_hover } else { bar },
                    ));
                }
            },
        )
        .size_full();
        let caption = match hovered.and_then(|ix| stats.weeks.get(ix)) {
            Some(week) => format!(
                "Week of {}: {}, +{} −{}",
                date(week.start),
                plural(week.commits, "commit", "commits"),
                format_count(week.additions),
                format_count(week.deletions)
            ),
            None => stats
                .weeks
                .iter()
                .max_by_key(|w| w.commits)
                .filter(|w| w.commits > 0)
                .map(|w| {
                    format!(
                        "Busiest: the week of {} ({})",
                        date(w.start),
                        plural(w.commits, "commit", "commits")
                    )
                })
                .unwrap_or_default(),
        };
        let bounds = self.chart_bounds.clone();
        div()
            .flex()
            .flex_col()
            .gap(SPACING_HALF())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(SPACING_HALF())
                    .child(
                        div()
                            .flex_none()
                            .w(zpx(36.))
                            .h(zpx(120.))
                            .flex()
                            .flex_col()
                            .justify_between()
                            .items_end()
                            .text_size(FONT_SIZE_SM())
                            .text_color(t.text_secondary)
                            .child(format_count(max))
                            .child("0"),
                    )
                    .child(
                        div()
                            .id("insights-chart")
                            .flex_1()
                            .min_w_0()
                            .h(zpx(120.))
                            .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                                let b = bounds.get();
                                let ix = (weeks > 0 && b.contains(&ev.position)).then(|| {
                                    let x = (ev.position.x - b.origin.x) / b.size.width;
                                    ((x * weeks as f32) as usize).min(weeks - 1)
                                });
                                if this.hovered_week != ix {
                                    this.hovered_week = ix;
                                    cx.notify();
                                }
                            }))
                            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                                if !hovered && this.hovered_week.is_some() {
                                    this.hovered_week = None;
                                    cx.notify();
                                }
                            }))
                            .child(chart),
                    ),
            )
            .child(
                div()
                    .ml(zpx(36.) + SPACING_HALF())
                    .flex()
                    .flex_row()
                    .justify_between()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(
                        stats
                            .weeks
                            .first()
                            .map(|w| date(w.start))
                            .unwrap_or_default(),
                    )
                    .child(
                        stats
                            .weeks
                            .last()
                            .map(|w| date(w.start))
                            .unwrap_or_default(),
                    ),
            )
            .child(
                div()
                    .ml(zpx(36.) + SPACING_HALF())
                    .min_h(zpx(16.))
                    .text_size(FONT_SIZE())
                    .child(caption),
            )
            .into_any_element()
    }

    /// A proportional bar in a table cell.
    fn share_bar(value: u64, max: u64, color: Hsla, t: &GhdTheme) -> Div {
        let share = if max == 0 {
            0.
        } else {
            value as f32 / max as f32
        };
        div()
            .w(zpx(120.))
            .flex_none()
            .h(zpx(6.))
            .rounded(zpx(3.))
            .bg(t.box_alt_background)
            .child(
                div()
                    .h_full()
                    .rounded(zpx(3.))
                    .w(relative(share.clamp(0., 1.)))
                    .bg(color),
            )
    }

    fn number_cell(text: String, width: f32, color: Hsla) -> Div {
        div()
            .flex_none()
            .w(zpx(width))
            .flex()
            .justify_end()
            .text_color(color)
            .child(text)
    }

    fn contributors(&self, stats: &RepoStats, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd().clone();
        let max = stats.contributors.first().map(|c| c.commits).unwrap_or(0);
        let shown = if self.all_contributors {
            stats.contributors.len()
        } else {
            CONTRIBUTORS_SHOWN
        };
        let rows = stats
            .contributors
            .iter()
            .take(shown)
            .enumerate()
            .map(|(ix, c)| {
                div()
                    .id(("insights-contributor", ix))
                    .h(ROW_HEIGHT() + zpx(6.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING())
                    .px(SPACING_HALF())
                    .border_b_1()
                    .border_color(t.box_border)
                    .text_size(FONT_SIZE())
                    .child(
                        div()
                            .flex_none()
                            .w(zpx(24.))
                            .text_color(t.text_secondary)
                            .child(format!("{}", ix + 1)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_row()
                            .items_baseline()
                            .gap(SPACING_HALF())
                            .child(
                                div()
                                    .flex_none()
                                    .max_w(relative(0.6))
                                    .truncate()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(c.name.clone()),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(FONT_SIZE_SM())
                                    .text_color(t.text_secondary)
                                    .child(c.email.clone()),
                            ),
                    )
                    .child(Self::share_bar(c.commits, max, t.link, &t))
                    .child(Self::number_cell(
                        plural(c.commits, "commit", "commits"),
                        90.,
                        t.text,
                    ))
                    .child(Self::number_cell(
                        format!("+{}", format_count(c.additions)),
                        70.,
                        t.color_new,
                    ))
                    .child(Self::number_cell(
                        format!("−{}", format_count(c.deletions)),
                        70.,
                        t.color_deleted,
                    ))
                    .child(
                        div()
                            .flex_none()
                            .w(zpx(170.))
                            .flex()
                            .justify_end()
                            .text_size(FONT_SIZE_SM())
                            .text_color(t.text_secondary)
                            .child(if c.first == c.last {
                                date(c.last)
                            } else {
                                format!("{} – {}", date(c.first), date(c.last))
                            }),
                    )
            });
        let more = stats.contributors.len() > CONTRIBUTORS_SHOWN;
        div()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(t.box_border)
            .children(rows)
            .when(more, |d| {
                d.child(self.show_all(
                    "insights-contributors-all",
                    self.all_contributors,
                    stats.contributors.len(),
                    |this| &mut this.all_contributors,
                    cx,
                ))
            })
            .into_any_element()
    }

    fn files(&self, stats: &RepoStats, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd().clone();
        let max = stats.files.first().map(|f| f.lines()).unwrap_or(0);
        let shown = if self.all_files {
            stats.files.len()
        } else {
            FILES_SHOWN
        };
        let rows = stats.files.iter().take(shown).enumerate().map(|(ix, f)| {
            let (dir, name) = match f.path.rfind('/') {
                Some(at) => (f.path[..=at].to_string(), f.path[at + 1..].to_string()),
                None => (String::new(), f.path.clone()),
            };
            div()
                .id(("insights-file", ix))
                .h(ROW_HEIGHT())
                .flex()
                .flex_row()
                .items_center()
                .gap(SPACING())
                .px(SPACING_HALF())
                .border_b_1()
                .border_color(t.box_border)
                .text_size(FONT_SIZE())
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_row()
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(t.text_secondary)
                                .child(crate::format::display_path(&dir)),
                        )
                        .child(div().flex_none().max_w_full().truncate().child(name)),
                )
                .child(Self::share_bar(f.lines(), max, t.color_modified, &t))
                .child(Self::number_cell(
                    if f.binary && f.lines() == 0 {
                        "binary".to_string()
                    } else {
                        plural(f.lines(), "line", "lines")
                    },
                    90.,
                    t.text,
                ))
                .child(Self::number_cell(
                    format!("+{}", format_count(f.additions)),
                    70.,
                    t.color_new,
                ))
                .child(Self::number_cell(
                    format!("−{}", format_count(f.deletions)),
                    70.,
                    t.color_deleted,
                ))
                .child(Self::number_cell(
                    plural(f.commits, "commit", "commits"),
                    90.,
                    t.text_secondary,
                ))
        });
        let more = stats.files.len() > FILES_SHOWN;
        div()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(t.box_border)
            .children(rows)
            .when(more, |d| {
                d.child(self.show_all(
                    "insights-files-all",
                    self.all_files,
                    stats.files.len(),
                    |this| &mut this.all_files,
                    cx,
                ))
            })
            .into_any_element()
    }

    fn show_all(
        &self,
        id: &'static str,
        all: bool,
        total: usize,
        field: fn(&mut Self) -> &mut bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd();
        div()
            .id(id)
            .mt(SPACING_HALF())
            .px(SPACING_HALF())
            .text_size(FONT_SIZE())
            .text_color(t.link)
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                let flag = field(this);
                *flag = !*flag;
                cx.notify();
            }))
            .child(if all {
                "Show fewer".to_string()
            } else {
                format!("Show all {total}")
            })
            .into_any_element()
    }
}

impl Render for InsightsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd().clone();
        let (id, insights) = {
            let s = self.state.read(cx);
            let Some(id) = s.selected else {
                return div().size_full().into_any_element();
            };
            let Some(insights) = s.repo_states.get(&id).and_then(|rs| rs.insights.clone()) else {
                return div().size_full().into_any_element();
            };
            (id, insights)
        };
        // numbers read for another scope or range are not shown
        let stats = insights.stats.clone().filter(|_| {
            insights
                .key
                .as_ref()
                .is_some_and(|k| k.scope == insights.scope && k.range == insights.range)
        });
        let body: AnyElement = match &stats {
            None => div()
                .flex_1()
                .flex()
                .items_start()
                .justify_center()
                .pt(SPACING_DOUBLE() * 2.)
                .text_color(t.text_secondary)
                .child(if insights.loading {
                    ""
                } else {
                    "No numbers yet."
                })
                .into_any_element(),
            Some(stats) if stats.commits == 0 => div()
                .flex_1()
                .flex()
                .items_start()
                .justify_center()
                .pt(SPACING_DOUBLE() * 2.)
                .text_color(t.text_secondary)
                .child("No commits in this range.")
                .into_any_element(),
            Some(stats) => {
                let span = match (stats.first, stats.last) {
                    (Some(first), Some(last)) => Some(format!("{} – {}", date(first), date(last))),
                    _ => None,
                };
                let took = insights
                    .took
                    .map(|d| format!("read in {:.1} s", d.as_secs_f32()));
                div()
                    .id("insights-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(
                        div()
                            .p(SPACING_DOUBLE())
                            .pt(SPACING())
                            .flex()
                            .flex_col()
                            .text_size(FONT_SIZE())
                            .child(Self::section_title(
                                "Overview",
                                [span, took]
                                    .into_iter()
                                    .flatten()
                                    .reduce(|a, b| format!("{a} · {b}")),
                                &t,
                            ))
                            .child(Self::totals(stats, &t))
                            .child(Self::section_title(
                                "Commits per week",
                                Some("Merge commits are not counted".into()),
                                &t,
                            ))
                            .child(self.chart(stats, cx))
                            .child(Self::section_title(
                                "Contributors",
                                Some(plural(stats.contributors.len() as u64, "person", "people")),
                                &t,
                            ))
                            .child(self.contributors(stats, cx))
                            .child(Self::section_title(
                                "Most changed files",
                                Some(format!(
                                    "By lines added and removed, of {}",
                                    plural(stats.files_total as u64, "file", "files")
                                )),
                                &t,
                            ))
                            .child(self.files(stats, cx)),
                    )
                    .with_scrollbar_handle(&self.scroll)
                    .into_any_element()
            }
        };
        div()
            .id("insights-view")
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .bg(t.background)
            .text_color(t.text)
            .child(self.header(id, &insights, cx))
            .children(self.status(id, &insights, cx))
            .child(body)
            .into_any_element()
    }
}
