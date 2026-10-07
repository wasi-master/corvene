//! Corvene `346-releases`: the History sidebar while Repository › Releases…
//! is open (`corvene_core::releases`). GitHub Desktop has no releases.
//!
//! A header counts the releases and offers New Release…, a refresh and
//! Close. Each row shows the release's name (the tag when it has none) with
//! Draft / Pre-release / Latest pills, then its tag, author and date.
//! Selecting a row shows the release in the commit view's place
//! ([`crate::release_view`]); its context menu opens it on GitHub or copies
//! the tag name.

use std::time::SystemTime;

use corvene_core::releases::ReleasesViewState;
use corvene_core::{AppState, Dispatcher};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::context_menu::{MenuItem, mac_or};
use crate::github_list::{self, header, header_button, pill};
use crate::icons::{Octicon, octicon};
use crate::scrollbar::ScrollbarExt;
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::GhdTooltip;

fn row_height() -> Pixels {
    zpx(47.)
}

pub struct ReleasesList {
    state: Entity<AppState>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    scrolled_to: Option<u64>,
    focused: bool,
}

impl ReleasesList {
    pub fn new(state: Entity<AppState>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle().tab_stop(true),
            scrolled_to: None,
            focused: false,
        }
    }

    fn move_selection(&mut self, id: u64, down: bool, cx: &mut Context<Self>) {
        let next = {
            let s = self.state.read(cx);
            let Some(r) = s.repo_states.get(&id).and_then(|rs| rs.releases.as_ref()) else {
                return;
            };
            if r.rows.is_empty() {
                return;
            }
            let current = r
                .selected
                .and_then(|sel| r.rows.iter().position(|x| x.id == sel));
            let next = match (current, down) {
                (None, _) => 0,
                (Some(c), true) => (c + 1).min(r.rows.len() - 1),
                (Some(c), false) => c.saturating_sub(1),
            };
            r.rows[next].id
        };
        Dispatcher::select_release(id, Some(next), cx);
    }

    fn header(&self, id: u64, releases: &ReleasesViewState, cx: &Context<Self>) -> Div {
        let t = cx.ghd();
        let subtitle = (releases.loaded && !releases.signed_out).then(|| {
            let n = releases.rows.len();
            format!("{n} release{}", if n == 1 { "" } else { "s" })
        });
        header(
            Octicon::Tag,
            "Releases",
            subtitle,
            vec![
                header_button("releases-new", "New Release", cx)
                    .child(octicon(Octicon::Plus, t.text))
                    .on_click(move |_, _, cx| Dispatcher::show_create_release(id, None, None, cx))
                    .into_any_element(),
                header_button("releases-refresh", "Refresh Releases", cx)
                    .child(octicon(Octicon::Sync, t.text))
                    .on_click(move |_, _, cx| Dispatcher::load_releases(id, cx))
                    .into_any_element(),
                header_button("releases-close", "Close Releases", cx)
                    .child(octicon(Octicon::X, t.text))
                    .on_click(move |_, _, cx| Dispatcher::close_releases(id, cx))
                    .into_any_element(),
            ],
            cx,
        )
    }
}

#[derive(Clone)]
struct RowTarget {
    release: u64,
    tag: String,
}

fn row_menu(
    id: u64,
    target: RowTarget,
    position: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let RowTarget { release, tag } = target;
    let items = vec![
        MenuItem::new(mac_or("View on GitHub", "View on GitHub"), move |_, cx| {
            Dispatcher::open_release_on_github(id, release, cx)
        }),
        MenuItem::separator(),
        MenuItem::new(mac_or("Copy Tag Name", "Copy tag name"), move |_, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(tag.clone()))
        }),
    ];
    crate::native_menu::show_context_menu(items, position, window, cx);
}

impl Render for ReleasesList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let loaded = {
            let s = self.state.read(cx);
            s.selected.and_then(|id| {
                let rs = s.repo_states.get(&id)?;
                let releases = corvene_core::releases::releases_of(s, rs)?.clone();
                let target = github_list::sign_in_target(s, id);
                Some((id, releases, target))
            })
        };
        let Some((id, releases, (target_name, target_endpoint))) = loaded else {
            self.focused = false;
            return div().into_any_element();
        };
        if !self.focused {
            self.focused = true;
            self.scrolled_to = None;
            window.focus(&self.focus, cx);
        }
        let t = cx.ghd().clone();
        let header = self.header(id, &releases, cx);
        if releases.selected != self.scrolled_to {
            self.scrolled_to = releases.selected;
            if let Some(ix) = releases
                .selected
                .and_then(|sel| releases.rows.iter().position(|r| r.id == sel))
            {
                self.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
            }
        }
        let list_focused = self.focus.contains_focused(window, cx);
        let body = if releases.signed_out {
            github_list::signed_out("releases", target_name, target_endpoint, cx).into_any_element()
        } else if !releases.loaded {
            github_list::loading("releases", "releases-spin", &t).into_any_element()
        } else if let Some(message) = releases.error.clone().filter(|_| releases.rows.is_empty()) {
            github_list::error(message, move |_, cx| Dispatcher::load_releases(id, cx), cx)
                .into_any_element()
        } else if releases.rows.is_empty() {
            github_list::message("No releases yet", &t).into_any_element()
        } else {
            let now = SystemTime::now();
            let focus = self.focus.clone();
            let latest = releases.latest_id();
            let rows = releases.rows.clone();
            let selected = releases.selected;
            let prerelease_color = github_list::prerelease_color(&t);
            uniform_list("release-rows", rows.len(), move |range, _window, _cx| {
                range
                    .map(|ix| {
                        let row = &rows[ix];
                        let is_selected = selected == Some(row.id);
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
                        let release = row.id;
                        let target = RowTarget {
                            release,
                            tag: row.tag_name.clone(),
                        };
                        let (focus, menu_focus) = (focus.clone(), focus.clone());
                        let when = github_list::date_text_at(
                            row.published_at.as_deref().or(row.created_at.as_deref()),
                            now,
                        );
                        let mut byline = row.tag_name.clone();
                        if !row.author.is_empty() {
                            byline.push_str(&format!(" • {}", row.author));
                        }
                        if !when.is_empty() {
                            byline.push_str(&format!(" • {when}"));
                        }
                        let (latest_color, draft_color, pre_color) = if is_selected && list_focused
                        {
                            (text, text, text)
                        } else {
                            (t.pr_open_icon, secondary, prerelease_color)
                        };
                        div()
                            .id(("release-row", ix))
                            .ghd_tooltip(row.display_name().to_string())
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
                                Dispatcher::select_release(id, Some(release), cx);
                            })
                            .on_mouse_down(MouseButton::Right, move |ev, window, cx| {
                                cx.stop_propagation();
                                window.focus(&menu_focus, cx);
                                Dispatcher::select_release(id, Some(release), cx);
                                row_menu(id, target.clone(), ev.position, window, cx);
                            })
                            .child(octicon(Octicon::Tag, secondary).flex_none())
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
                                            .gap(SPACING_HALF())
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .truncate()
                                                    .text_size(FONT_SIZE())
                                                    .line_height(zpx(18.))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(text)
                                                    .child(row.display_name().to_string()),
                                            )
                                            .when(row.draft, |d| {
                                                d.child(pill("Draft", draft_color))
                                            })
                                            .when(row.prerelease, |d| {
                                                d.child(pill("Pre-release", pre_color))
                                            })
                                            .when(latest == Some(row.id), |d| {
                                                d.child(pill("Latest", latest_color))
                                            }),
                                    )
                                    .child(
                                        div()
                                            .mt(zpx(2.))
                                            .min_w_0()
                                            .truncate()
                                            .text_size(FONT_SIZE_SM())
                                            .line_height(zpx(16.5))
                                            .text_color(secondary)
                                            .child(byline),
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
        let error_line = releases
            .error
            .clone()
            .filter(|_| !releases.rows.is_empty())
            .map(|message| {
                div()
                    .flex_none()
                    .px(SPACING())
                    .py(SPACING_HALF())
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.dialog_error)
                    .truncate()
                    .child(format!("Could not refresh: {message}"))
            });

        let view = cx.entity().downgrade();
        let t = cx.ghd();
        div()
            .id("releases-list")
            .track_focus(&self.focus)
            .on_key_down(move |ev, _, cx| {
                let down = match ev.keystroke.key.as_str() {
                    "escape" => {
                        cx.stop_propagation();
                        Dispatcher::close_releases(id, cx);
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
            .when_some(error_line, |d, line| d.child(line))
            .child(body)
            .into_any_element()
    }
}
