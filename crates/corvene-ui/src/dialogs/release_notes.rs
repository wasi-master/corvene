//! GHD `ui/release-notes/release-notes-dialog.tsx`
//! (`styles/ui/dialogs/_release-notes.scss`): a 100 px header with the two
//! release-note illustrations and "Version X / date", the entries in one or
//! two columns, "View all release notes" on the left of the footer.
//!
//! Deviations: entries are plain text (GHD's `RichText` renders emoji, links
//! and `#123` references); the pretext goes through `crate::markdown` instead
//! of GHD's sandboxed Markdown webview. "Install and Restart" appears when
//! the notes are those of the update the self-updater has ready (GHD: when
//! the version differs from the running one). `421-larger-dialogs`: the
//! dialog grows to 80 % of the window (at most 960 px wide; GHD: 550 to
//! 800 px wide and 500 px high).

use corvene_core::release_notes::{RELEASE_NOTES_URL, ReleaseNote, ReleaseSummary};
use corvene_core::{AppState, Dispatcher, UpdateStatus};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::context_menu::mac_or;
use crate::widgets::IconButtonA11y;

use crate::icons::{Octicon, octicon};
use crate::scrollbar::ScrollbarExt;
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::{button, link_button, primary_button};

/// `--spacing-triple`
#[allow(non_snake_case)]
fn SPACING_TRIPLE() -> Pixels {
    zpx(30.)
}
/// `.dialog-header` height and the illustrations' height in it.
#[allow(non_snake_case)]
fn HEADER_HEIGHT() -> Pixels {
    zpx(100.)
}
#[allow(non_snake_case)]
fn ART_HEIGHT() -> Pixels {
    zpx(90.)
}

pub struct ReleaseNotesDialog {
    state: Entity<AppState>,
    summary: ReleaseSummary,
}

impl ReleaseNotesDialog {
    pub fn new(state: Entity<AppState>, summary: ReleaseSummary, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self { state, summary }
    }
}

/// `renderList`: a bold header and one line per entry.
fn section(header: &'static str, entries: &[ReleaseNote], cx: &App) -> Option<AnyElement> {
    let t = cx.ghd();
    if entries.is_empty() {
        return None;
    }
    Some(
        div()
            .my(SPACING_DOUBLE())
            .flex()
            .flex_col()
            .gap(SPACING_HALF())
            .child(
                div()
                    .text_size(zpx(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(header),
            )
            .children(
                entries
                    .iter()
                    .map(|e| div().text_color(t.text).child(e.message.clone())),
            )
            .into_any_element(),
    )
}

/// `.column`
fn column(children: Vec<Option<AnyElement>>) -> Div {
    div()
        .flex_1()
        .min_w(zpx(275.))
        .mr(zpx(15.))
        .flex()
        .flex_col()
        .children(children.into_iter().flatten())
}

impl Render for ReleaseNotesDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd();
        let r = &self.summary;
        let viewport = crate::theme::page_size(window);
        // `421-larger-dialogs`: width and height
        let large = crate::dialog::larger_dialogs(cx).then(|| {
            (
                crate::dialog::larger_dialog_width(viewport).max(zpx(550.)),
                (viewport.height * 0.8).max(zpx(500.)),
            )
        });
        let date = r
            .date_published
            .map(|d| crate::format::format_pattern("MMMM d, yyyy", &crate::format::local_time(d)));
        // `drawTwoColumnLayout` when there are both enhancements and bugfixes
        let columns: Vec<Div> = if !r.enhancements.is_empty() && !r.bugfixes.is_empty() {
            vec![
                column(vec![
                    section("Enhancements", &r.enhancements, cx),
                    section("Other", &r.other, cx),
                ]),
                column(vec![section("Bugfixes", &r.bugfixes, cx)]),
            ]
        } else {
            vec![column(vec![
                section("Bugfixes", &r.bugfixes, cx),
                section("Enhancements", &r.enhancements, cx),
                section("Other", &r.other, cx),
            ])]
        };
        let pretext = r.pretext.first().map(|p| {
            // `1333-markdown-math`
            crate::markdown::github_markdown("release-notes-pretext", &p.message, None, cx)
        });
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        // `renderButtons`: the ready update's notes get "Install and Restart"
        // (GHD's destructive OkCancelButtonGroup: Close stays the default)
        let can_install = matches!(
            &self.state.read(cx).update.status,
            UpdateStatus::Ready { update, .. } if update.version == r.latest_version
        );
        deferred(
            anchored().position(crate::theme::page_origin()).child(
                div()
                    .id("release-notes-overlay")
                    // modal: nothing underneath takes hover, clicks or wheel
                    .occlude()
                    .child(crate::widgets::touch_drag_occluder())
                    .w(viewport.width)
                    .h(viewport.height)
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(t.dialog_backdrop)
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| close(window, cx))
                    .child(
                        div()
                            .id("release-notes")
                            .role(Role::Dialog)
                            .aria_label(format!("Release notes for version {}", r.latest_version))
                            .child(crate::dialog::window_title("Release Notes"))
                            .min_w(zpx(550.))
                            .map(|d| match large {
                                Some((width, height)) => d.w(width).max_h(height),
                                None => d.max_w(zpx(800.)).max_h(zpx(500.)),
                            })
                            .flex()
                            .flex_col()
                            .rounded(BORDER_RADIUS())
                            .bg(t.background)
                            .text_color(t.text)
                            .text_size(FONT_SIZE())
                            .border_1()
                            .border_color(t.box_border)
                            .shadow(vec![BoxShadow {
                                color: t.shadow,
                                offset: point(zpx(0.), zpx(2.)),
                                blur_radius: css_blur(7.),
                                spread_radius: zpx(0.),
                                inset: false,
                            }])
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(
                                // `.dialog-header`
                                div()
                                    .relative()
                                    .h(HEADER_HEIGHT())
                                    .flex_none()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_center()
                                    .border_b_1()
                                    .border_color(t.box_border)
                                    .child(
                                        img("illustrations/release-note-header-left.svg")
                                            .absolute()
                                            .left(SPACING_DOUBLE())
                                            .top(SPACING_HALF())
                                            .h(ART_HEIGHT())
                                            .w(ART_HEIGHT() * (147. / 71.)),
                                    )
                                    .child(
                                        img("illustrations/release-note-header-right.svg")
                                            .absolute()
                                            .right(SPACING_DOUBLE())
                                            .top(SPACING_HALF())
                                            .h(ART_HEIGHT())
                                            .w(ART_HEIGHT() * (144. / 76.)),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_size(zpx(14.))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .child(format!("Version {}", r.latest_version)),
                                            )
                                            .children(date.map(|d| {
                                                div()
                                                    .text_size(zpx(11.))
                                                    .text_color(t.text_secondary)
                                                    .child(d)
                                            })),
                                    )
                                    .child(
                                        // `a.close`, top-aligned
                                        div()
                                            .id("release-notes-close")
                                            .absolute()
                                            .top(SPACING())
                                            .right(SPACING())
                                            .size(zpx(16.))
                                            .cursor_pointer()
                                            .icon_button_label("Close")
                                            .on_click(move |_, window, cx| close(window, cx))
                                            .child(octicon(Octicon::X, t.text_secondary)),
                                    ),
                            )
                            .child(
                                // `.dialog-content`
                                div()
                                    .id("release-notes-content")
                                    // the header and footer take the other 165 px
                                    .max_h(large.map_or(zpx(335.), |(_, h)| h - zpx(165.)))
                                    .overflow_y_scroll()
                                    .py(SPACING())
                                    .px(SPACING_TRIPLE())
                                    .line_height(zpx(18.))
                                    .children(pretext.map(|p| div().my(SPACING()).child(p)))
                                    .child(
                                        div().flex().flex_row().justify_around().children(columns),
                                    )
                                    .with_scrollbar(),
                            )
                            .child(
                                // `.dialog-footer`: space-between
                                div()
                                    .flex_none()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .gap(SPACING())
                                    .p(SPACING_DOUBLE())
                                    .border_t_1()
                                    .border_color(t.box_border)
                                    .child(
                                        link_button(
                                            "release-notes-all",
                                            "View all release notes",
                                            cx,
                                        )
                                        .on_click(
                                            |_, _, cx| {
                                                corvene_core::Dispatcher::open_url(
                                                    RELEASE_NOTES_URL,
                                                    cx,
                                                )
                                            },
                                        ),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap(SPACING())
                                            // `OkCancelButtonGroup`: Close is its
                                            // Cancel, Install its OK
                                            .children(crate::dialog::ok_cancel_order(
                                                std::iter::once(
                                                    primary_button(
                                                        "release-notes-ok",
                                                        "Close",
                                                        false,
                                                        cx,
                                                    )
                                                    .min_w(zpx(120.))
                                                    .on_click(move |_, window, cx| {
                                                        close(window, cx)
                                                    }),
                                                )
                                                .chain(can_install.then(|| {
                                                    button(
                                                        "release-notes-install",
                                                        mac_or(
                                                            "Install and Restart",
                                                            "Install and restart",
                                                        ),
                                                        cx,
                                                    )
                                                    .on_click(|_, _, cx| {
                                                        Dispatcher::install_update(cx)
                                                    })
                                                }))
                                                .collect(),
                                            )),
                                    ),
                            ),
                    ),
            ),
        )
        .with_priority(20)
    }
}
