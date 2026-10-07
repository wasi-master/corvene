//! Corvene `345-issues` / `346-releases`: what the Issues and Releases
//! lists share (GitHub Desktop has neither): the 36 px header with its icon
//! buttons, the loading / empty / error / signed-out bodies, GitHub's label
//! chips and the small pills, dates as the pull request list shows them.

use std::time::SystemTime;

use corvene_core::{Dispatcher, Popup};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::icons::{Octicon, octicon, spin};
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, Appearance, GhdTheme};
use crate::widgets::IconButtonA11y;

/// A header's 19 px icon button.
pub fn header_button(id: impl Into<ElementId>, label: &str, cx: &App) -> Stateful<Div> {
    let t = cx.ghd();
    div()
        .id(id)
        .icon_button_label(label.to_string())
        .h(zpx(19.))
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
}

/// The list's header: an icon, the title, a dimmed subtitle, then the
/// buttons at the right (the close one last).
pub fn header(
    icon: Octicon,
    title: &'static str,
    subtitle: Option<String>,
    buttons: Vec<AnyElement>,
    cx: &App,
) -> Div {
    let t = cx.ghd();
    div()
        .h(zpx(36.))
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(SPACING_HALF())
        .pl(SPACING())
        .pr(SPACING_HALF())
        .bg(t.box_alt_background)
        .border_b_1()
        .border_color(t.box_border)
        .text_size(FONT_SIZE())
        .child(octicon(icon, t.text_secondary).flex_none())
        .child(
            div()
                .flex_none()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .when_some(subtitle, |d, subtitle| {
            d.child(
                div()
                    .min_w_0()
                    .flex_shrink(1.)
                    .truncate()
                    .text_color(t.text_secondary)
                    .child(subtitle),
            )
        })
        .child(div().flex_1())
        .children(buttons)
}

/// A centred secondary line (nothing loaded yet, nothing to list).
pub fn message(text: impl Into<SharedString>, t: &GhdTheme) -> Div {
    div()
        .flex_1()
        .flex()
        .items_start()
        .justify_center()
        .pt(SPACING() * 2.)
        .px(SPACING())
        .text_center()
        .text_color(t.text_secondary)
        .child(text.into())
}

/// "Loading …" with the spinning sync icon.
pub fn loading(what: &str, id: &'static str, t: &GhdTheme) -> Div {
    div()
        .flex_1()
        .flex()
        .flex_row()
        .items_start()
        .justify_center()
        .gap(SPACING_HALF())
        .pt(SPACING() * 2.)
        .text_color(t.text_secondary)
        .child(spin(octicon(Octicon::SyncClockwise, t.text_secondary), id))
        .child(format!("Loading {what}…"))
}

/// A failed load: the message and a Retry link.
pub fn error(message: String, retry: impl Fn(&mut Window, &mut App) + 'static, cx: &App) -> Div {
    let t = cx.ghd();
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .gap(SPACING_HALF())
        .pt(SPACING() * 2.)
        .px(SPACING())
        .text_center()
        .text_color(t.text_secondary)
        .child(div().text_color(t.dialog_error).child(message))
        .child(
            crate::widgets::link_button("github-list-retry", "Try again", cx)
                .on_click(move |_, window, cx| retry(window, cx)),
        )
}

/// The repository a signed-out list names and the API endpoint its sign-in
/// link is for: the matched GitHub repository, else what the origin remote
/// points at (a repository added while signed out has no match yet, which
/// left the name blank and the link on GitHub Enterprise).
pub fn sign_in_target(s: &corvene_core::AppState, id: u64) -> (String, String) {
    const DOTCOM: &str = "https://api.github.com";
    if let Some(gh) = s.repository(id).and_then(|r| r.non_fork_github()) {
        return (gh.full_name(), gh.endpoint.clone());
    }
    let info = s.repo_states.get(&id).and_then(|rs| rs.info.as_ref());
    let remote = info.and_then(|i| i.remote("origin").or(i.remotes.first()));
    match remote.and_then(|r| corvene_core::split_remote(&r.url)) {
        Some((host, path)) => {
            let path = path.trim_matches('/');
            let path = path.strip_suffix(".git").unwrap_or(path).to_string();
            let endpoint = if host.eq_ignore_ascii_case("github.com") {
                DOTCOM.to_string()
            } else {
                corvene_github::endpoint::get_enterprise_api_url(&format!("https://{host}"))
            };
            (path, endpoint)
        }
        None => (
            s.repository(id).map(|r| r.name()).unwrap_or_default(),
            DOTCOM.to_string(),
        ),
    }
}

/// No account for the repository's endpoint (after the pull request
/// list's `signed_out_pull_requests`): a sign-in link for it.
pub fn signed_out(what: &str, repository_name: String, endpoint: String, cx: &App) -> Div {
    let t = cx.ghd();
    let enterprise = endpoint != "https://api.github.com";
    let service = if enterprise {
        "GitHub Enterprise"
    } else {
        "GitHub.com"
    };
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .text_center()
        .p(SPACING())
        .pt(SPACING() * 2.)
        .text_size(FONT_SIZE())
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(format!("Sign in to see {what}")),
        )
        .child(
            div()
                .pb(SPACING())
                .flex()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_center()
                .gap(zpx(3.))
                .child(format!("{what} in", what = capitalize(what)))
                .child(crate::widgets::code_ref(repository_name, cx))
                .child(format!("are loaded with a {service} account.")),
        )
        .child(
            div().text_size(FONT_SIZE_SM()).text_color(t.text).child(
                crate::widgets::link_button(
                    "github-list-sign-in",
                    format!("Sign in to {service}"),
                    cx,
                )
                .on_click(move |_, _, cx| Dispatcher::show_popup(Popup::SignIn { enterprise }, cx)),
            ),
        )
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// GitHub's label colour (`rrggbb`), `None` for anything else.
pub fn label_color(hex: &str) -> Option<Hsla> {
    let hex = hex.trim().trim_start_matches('#');
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some(rgb(value).into())
}

/// Black or white text over `background`, by its luminance, as GitHub's
/// `IssueLabel` chooses.
fn label_text(background: Hsla) -> Hsla {
    let rgba: Rgba = background.into();
    let luminance = 0.2126 * rgba.r + 0.7152 * rgba.g + 0.0722 * rgba.b;
    if luminance > 0.55 {
        rgb(0x1f2328).into()
    } else {
        rgb(0xffffff).into()
    }
}

/// A label as GitHub draws it: a rounded pill in the label's colour. A
/// label without a usable colour gets the secondary text on a border.
/// `selected` (the active label filter) adds a focus-coloured ring.
pub fn label_chip(
    id: impl Into<ElementId>,
    name: &str,
    hex: &str,
    selected: bool,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.ghd();
    let chip = div()
        .id(id)
        .flex_none()
        .h(zpx(18.))
        .px(SPACING_HALF())
        .rounded(zpx(9.))
        .text_size(FONT_SIZE_SM())
        .line_height(zpx(16.))
        .flex()
        .items_center()
        .border_1()
        .child(div().max_w(zpx(160.)).truncate().child(name.to_string()));
    let chip = match label_color(hex) {
        Some(color) => {
            // dark mode: GitHub dims the fill and keeps the hue in the text
            if t.appearance == Appearance::Dark {
                chip.bg(color.opacity(0.18))
                    .border_color(color.opacity(0.4))
                    .text_color(color.blend(rgb(0xffffff).into()).opacity(1.))
            } else {
                chip.bg(color)
                    .border_color(color)
                    .text_color(label_text(color))
            }
        }
        None => chip.border_color(t.box_border).text_color(t.text_secondary),
    };
    chip.when(selected, |d| {
        d.border_color(t.focus).shadow(vec![BoxShadow {
            color: t.focus.opacity(0.4),
            offset: point(px(0.), px(0.)),
            blur_radius: px(0.),
            spread_radius: px(2.),
            inset: false,
        }])
    })
}

/// A small outlined pill after a row's text (Draft, Pre-release, Latest,
/// Open, Closed).
pub fn pill(text: impl Into<SharedString>, color: Hsla) -> Div {
    div()
        .flex_none()
        .h(zpx(15.))
        .px(SPACING_THIRD())
        .flex()
        .items_center()
        .rounded(BORDER_RADIUS())
        .border_1()
        .border_color(color)
        .text_color(color)
        .text_size(FONT_SIZE_SM())
        .line_height(zpx(13.))
        .child(text.into())
}

/// A filled state pill (the issue / release page's badge).
pub fn state_pill(text: impl Into<SharedString>, icon: Octicon, color: Hsla, cx: &App) -> Div {
    crate::widgets::pill(text, Some(icon), color, rgb(0xffffff).into(), cx)
}

/// The purple of a closed issue (Primer `done.fg`).
pub fn closed_color(t: &GhdTheme) -> Hsla {
    if t.appearance == Appearance::Dark {
        rgb(0xa371f7).into()
    } else {
        rgb(0x8250df).into()
    }
}

/// The yellow of a pre-release (Primer `attention.fg`).
pub fn prerelease_color(t: &GhdTheme) -> Hsla {
    if t.appearance == Appearance::Dark {
        rgb(0xd29922).into()
    } else {
        rgb(0x9a6700).into()
    }
}

/// An API timestamp as the pull request list shows dates: relative, or
/// absolute when Settings asks for it; empty when unreadable.
pub fn date_text(iso: Option<&str>) -> String {
    iso.and_then(corvene_core::parse_iso8601)
        .map(|t| {
            if crate::format::prefer_absolute_dates() {
                crate::format::format_date(t)
            } else {
                crate::relative_time::relative(t)
            }
        })
        .unwrap_or_default()
}

/// `relative_at` against `now` for list rows (one `now` per render).
pub fn date_text_at(iso: Option<&str>, now: SystemTime) -> String {
    iso.and_then(corvene_core::parse_iso8601)
        .map(|t| {
            if crate::format::prefer_absolute_dates() {
                crate::format::format_date(t)
            } else {
                crate::relative_time::relative_at(t, now)
            }
        })
        .unwrap_or_default()
}

/// `1.2 MB` for an asset's size.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024. && unit < UNITS.len() - 1 {
        size /= 1024.;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if size >= 100. {
        format!("{size:.0} {}", UNITS[unit])
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn parses_label_colours() {
        assert!(label_color("d73a4a").is_some());
        assert!(label_color("#D73A4A").is_some());
        assert!(label_color("").is_none());
        assert!(label_color("zzzzzz").is_none());
        assert!(label_color("fff").is_none());
    }

    #[::core::prelude::v1::test]
    fn sizes_read_well() {
        assert_eq!(human_size(412), "412 B");
        assert_eq!(human_size(24_117_248), "23.0 MB");
        assert_eq!(human_size(150 * 1024), "150 KB");
    }
}
