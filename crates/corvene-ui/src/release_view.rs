//! Corvene `346-releases`: the selected release, in the commit view's place
//! while Repository › Releases… is open (`corvene_core::releases`). GitHub
//! Desktop has no releases.
//!
//! The name with the Draft / Pre-release / Latest badge, the tag, author
//! and date, View on GitHub, then the notes as Markdown and the assets
//! (name, size, download count; a click downloads in the browser) with the
//! source archive.

use corvene_core::releases::ReleaseRow;
use corvene_core::{AppState, Dispatcher};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::github_list::{self, state_pill};
use crate::icons::{Octicon, octicon};
use crate::scrollbar::ScrollbarExt;
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::button;

pub struct ReleaseView {
    state: Entity<AppState>,
    shown: Option<u64>,
    scroll: ScrollHandle,
}

impl ReleaseView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            shown: None,
            scroll: ScrollHandle::new(),
        }
    }

    fn release(&self, id: u64, row: &ReleaseRow, latest: bool, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd().clone();
        let release = row.id;
        let badge = if row.draft {
            Some(state_pill("Draft", Octicon::Pencil, t.text_secondary, cx))
        } else if row.prerelease {
            Some(state_pill(
                "Pre-release",
                Octicon::Tag,
                github_list::prerelease_color(&t),
                cx,
            ))
        } else if latest {
            Some(state_pill("Latest", Octicon::Tag, t.pr_open_icon, cx))
        } else {
            None
        };
        let when =
            github_list::date_text(row.published_at.as_deref().or(row.created_at.as_deref()));
        let mut meta = String::new();
        if !row.author.is_empty() {
            meta.push_str(&row.author);
            meta.push_str(if row.draft {
                " drafted this"
            } else {
                " released this"
            });
        } else {
            meta.push_str(if row.draft { "Drafted" } else { "Released" });
        }
        if !when.is_empty() {
            meta.push(' ');
            meta.push_str(&when);
        }
        let body = if row.body.trim().is_empty() {
            div()
                .text_color(t.text_secondary)
                .child("No release notes.")
                .into_any_element()
        } else {
            crate::markdown::markdown(
                format!("release-body-{release}"),
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
                            .text_size(FONT_SIZE_LG())
                            .line_height(zpx(22.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(row.display_name().to_string()),
                    )
                    .child(
                        button("release-view-on-github", "View on GitHub", cx).on_click(
                            move |_, _, cx| Dispatcher::open_release_on_github(id, release, cx),
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
                    .children(badge)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(zpx(3.))
                            .text_size(FONT_SIZE())
                            .text_color(t.text_secondary)
                            .child(octicon(Octicon::Tag, t.text_secondary).size(zpx(12.)))
                            .child(row.tag_name.clone()),
                    )
                    .child(
                        div()
                            .text_size(FONT_SIZE())
                            .text_color(t.text_secondary)
                            .child(meta),
                    ),
            );
        let assets = (!row.assets.is_empty() || row.tarball_url.is_some()).then(|| {
            div()
                .mt(SPACING())
                .flex()
                .flex_col()
                .gap(SPACING_THIRD())
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .pb(SPACING_THIRD())
                        .child("Assets"),
                )
                .children(row.assets.iter().enumerate().map(|(ix, asset)| {
                    let url = asset.url.clone();
                    asset_row(
                        ("release-asset", ix),
                        Octicon::Package,
                        asset.name.clone(),
                        Some(format!(
                            "{} • {} download{}",
                            github_list::human_size(asset.size),
                            asset.downloads,
                            if asset.downloads == 1 { "" } else { "s" }
                        )),
                        move |cx| Dispatcher::open_url(&url, cx),
                        cx,
                    )
                }))
                .children(row.tarball_url.clone().map(|url| {
                    asset_row(
                        "release-source-tarball",
                        Octicon::FileCode,
                        "Source code (tar.gz)".to_string(),
                        None,
                        move |cx| Dispatcher::open_url(&url, cx),
                        cx,
                    )
                }))
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .child(header)
            .child(
                div()
                    .id("release-body-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .p(SPACING())
                    .child(div().max_w(zpx(760.)).child(body).children(assets))
                    .with_scrollbar(),
            )
            .into_any_element()
    }
}

/// One asset line: icon, name as a link, details.
fn asset_row(
    id: impl Into<ElementId>,
    icon: Octicon,
    name: String,
    details: Option<String>,
    open: impl Fn(&mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.ghd();
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap(SPACING_HALF())
        .px(SPACING_HALF())
        .py(SPACING_THIRD())
        .rounded(BORDER_RADIUS())
        .border_1()
        .border_color(t.box_border)
        .cursor_pointer()
        .hover(|d| d.bg(t.box_hover_background))
        .on_click(move |_, _, cx| open(cx))
        .child(octicon(icon, t.text_secondary).flex_none())
        .child(div().min_w_0().truncate().text_color(t.link).child(name))
        .child(div().flex_1())
        .when_some(details, |d, details| {
            d.child(
                div()
                    .flex_none()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(details),
            )
        })
}

impl Render for ReleaseView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = {
            let s = self.state.read(cx);
            s.selected.and_then(|id| {
                let rs = s.repo_states.get(&id)?;
                let releases = corvene_core::releases::releases_of(s, rs)?;
                let row = releases.selected_row().cloned();
                let latest = row
                    .as_ref()
                    .is_some_and(|r| releases.latest_id() == Some(r.id));
                Some((id, row, latest, releases.loaded))
            })
        };
        let t = cx.ghd().clone();
        let Some((id, row, latest, loaded)) = shown else {
            return div().size_full().bg(t.background).into_any_element();
        };
        let current = row.as_ref().map(|r| r.id);
        if current != self.shown {
            self.shown = current;
            self.scroll.set_offset(point(px(0.), px(0.)));
        }
        let content = match row {
            Some(row) => self.release(id, &row, latest, cx),
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(t.text_secondary)
                .child(if loaded {
                    "Select a release to see it here"
                } else {
                    ""
                })
                .into_any_element(),
        };
        div()
            .id("release-view")
            .size_full()
            .bg(t.background)
            .text_color(t.text)
            .child(content)
            .into_any_element()
    }
}
