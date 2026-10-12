//! Corvene: the row standing for a run of unchanged rows in a rendered diff
//! (`corvene_core::fold`), drawn like the text diff's hunk header: click to
//! show the run.

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::diff_view::DIFF_LINE_HEIGHT;
use crate::icons::{Octicon, octicon};
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::IconButtonA11y;

/// A fold of `n` unchanged `noun`s (`noun` / `plural`); `tight` fits it
/// in one diff line (table rows), else it is padded like a block.
pub fn fold_row(
    id: impl Into<ElementId>,
    n: usize,
    (noun, plural): (&str, &str),
    tight: bool,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let t = cx.ghd();
    let hover = t.box_hover_background;
    let label = format!("{n} unchanged {}", if n == 1 { noun } else { plural });
    let button = div()
        .id(id.into())
        .a11y_button(format!("Show {label}"))
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap(SPACING_HALF())
        .bg(t.diff_hunk_background)
        .text_size(FONT_SIZE_SM())
        .text_color(t.text_secondary)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(octicon(Octicon::Unfold, t.text_secondary))
        .child(label)
        .on_click(move |_, window, cx| on_click(window, cx));
    if tight {
        button
            .h(DIFF_LINE_HEIGHT())
            .border_y_1()
            .border_color(t.diff_border)
            .into_any_element()
    } else {
        div()
            .w_full()
            .px(SPACING())
            .py(zpx(3.))
            .child(button.h(zpx(22.)).rounded(BORDER_RADIUS()))
            .into_any_element()
    }
}
