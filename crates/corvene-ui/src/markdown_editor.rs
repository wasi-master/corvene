//! Corvene `345-issues`: a Markdown text area with github.com's "Write" /
//! "Preview" tabs (New Issue…'s description, the issue view's comment
//! box). Preview draws the text with [`crate::markdown`], links and
//! relative images resolved against the repository's page.

use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;

/// The editor box: the tab bar over the text area, or over the preview.
/// `on_select(preview)` switches tabs; switching to Write focuses the text.
pub fn markdown_editor(
    key: &'static str,
    state: &Entity<TextareaState>,
    preview: bool,
    base_href: Option<&str>,
    min_height: Pixels,
    on_select: impl Fn(bool, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> Div {
    let t = cx.ghd();
    let tab = |ix: usize, label: &'static str, selected: bool| {
        let on_select = on_select.clone();
        let state = state.clone();
        div()
            .id((key, ix))
            .px(zpx(12.))
            .py(zpx(5.))
            .mb(zpx(-1.))
            .rounded_t(BORDER_RADIUS())
            .border_1()
            .cursor_pointer()
            .text_size(FONT_SIZE())
            .map(|d| {
                if selected {
                    d.border_color(t.box_border)
                        .bg(t.box_background)
                        .text_color(t.text)
                } else {
                    d.border_color(transparent_black())
                        .text_color(t.text_secondary)
                        .hover(|s| s.text_color(t.text))
                }
            })
            .on_click(move |_, window, cx| {
                let to_preview = ix == 1;
                on_select(to_preview, window, cx);
                if !to_preview {
                    let handle = state.read(cx).focus_handle(cx);
                    window.focus(&handle, cx);
                }
            })
            .child(label)
    };
    let bar = div()
        .flex()
        .flex_row()
        .items_end()
        .gap(zpx(2.))
        .px(SPACING_HALF())
        .pt(SPACING_HALF())
        .bg(t.box_alt_background)
        .border_b_1()
        .border_color(t.box_border)
        .child(tab(0, "Write", !preview))
        .child(tab(1, "Preview", preview));
    let body = if preview {
        let text = state.read(cx).value().to_string();
        let content = if text.trim().is_empty() {
            div()
                .text_color(t.text_secondary)
                .child("Nothing to preview")
                .into_any_element()
        } else {
            // `1333-markdown-math`
            crate::markdown::github_markdown(
                SharedString::from(format!("{key}-preview")),
                &text,
                base_href,
                cx,
            )
            .into_any_element()
        };
        div()
            .id(SharedString::from(format!("{key}-preview-scroll")))
            .min_h(min_height)
            .max_h(zpx(320.))
            .overflow_y_scroll()
            .p(SPACING())
            .child(content)
            .into_any_element()
    } else {
        crate::widgets::resizable_text_area(
            key,
            Textarea::new(state)
                .appearance(false)
                .p(SPACING_HALF())
                .text_size(FONT_SIZE())
                .min_h(min_height),
            cx,
        )
    };
    div()
        .flex()
        .flex_col()
        .border_1()
        .border_color(t.box_border_contrast)
        .rounded(BORDER_RADIUS())
        .bg(t.box_background)
        .overflow_hidden()
        .child(bar)
        .child(body)
}
