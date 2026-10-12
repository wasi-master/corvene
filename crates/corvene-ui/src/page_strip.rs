//! Corvene `1359-page-strip`: the pieces of the thumbnail strip beside a
//! page diff (`pdf_diff`: PDF files and the document and LaTeX Pages views)
//! and of the chapter list beside an EPUB's rendered diff
//! (`document_diff`). GitHub Desktop shows these files as binary
//! (`app/src/ui/diff/binary-file.tsx`, "This binary file has changed.").
//!
//! The strip is a resizable column like the archive view's entry tree
//! (`archive_diff_view.rs`); its width and the Changed only box are
//! settings shared by every page diff. A row's mark uses the file list's
//! status icons and colours (`diff_view::status_icon`); unchanged rows are
//! dimmed. The owner keeps the rows, the selection and the thumbnails and
//! binds the arrow keys under [`KEY_CONTEXT`].

use std::rc::Rc;
use std::sync::Arc;

use corvene_core::pdf::{PagePair, PageStatus};
use corvene_core::{AppState, Dispatcher};
use gpui_kit::component::resizable::{
    ResizablePanelEvent, ResizableState, h_resizable, resizable_panel,
};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::icons::{Octicon, octicon};
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, GhdTheme};
use crate::widgets::{GhdTooltip, ListRowA11y};

/// The key context of a focused strip (arrows step through its rows).
pub const KEY_CONTEXT: &str = "PageStrip";
const MIN_WIDTH: f32 = 100.;
const MAX_WIDTH: f32 = 400.;
/// The tallest a thumbnail is drawn (CSS px).
pub const THUMB_HEIGHT: f32 = 96.;
/// A thumbnail row: the page, its number line and padding (CSS px).
pub const THUMB_ROW_HEIGHT: f32 = THUMB_HEIGHT + 34.;
/// The longer side of a drawn thumbnail in pixels (sharp at 2x).
pub const THUMB_PX: u32 = 200;

/// Whether the strip shows (`1359-page-strip`).
pub fn enabled(cx: &App) -> bool {
    AppState::try_global(cx)
        .is_some_and(|s| s.read(cx).flags.bool(corvene_core::flags::ids::PAGE_STRIP))
}

/// Whether the strip lists only changed rows.
pub fn changed_only(cx: &App) -> bool {
    AppState::try_global(cx).is_some_and(|s| s.read(cx).settings.page_strip_changed_only)
}

fn width(cx: &App) -> Pixels {
    zpx(AppState::try_global(cx).map_or(160., |s| s.read(cx).settings.page_strip_width))
}

/// The strip's resizable split; drags save the width.
pub fn split_state<V: 'static>(cx: &mut Context<V>) -> Entity<ResizableState> {
    let resizable = cx.new(|_| ResizableState::default());
    cx.subscribe(&resizable, |_, state, _: &ResizablePanelEvent, cx| {
        if let Some(width) = state.read(cx).sizes().first().copied()
            && width != self::width(cx)
        {
            Dispatcher::set_page_strip_width(unzoom(width), cx);
            cx.notify();
        }
    })
    .detach();
    resizable
}

/// `strip` (left, resizable) beside `main`.
pub fn split(
    id: &'static str,
    resizable: &Entity<ResizableState>,
    strip: AnyElement,
    main: AnyElement,
    cx: &App,
) -> AnyElement {
    h_resizable(id)
        .with_state(resizable)
        .with_handle_appearance(Rc::new(|_, _, _| Some(div().into_any_element())))
        .child(
            resizable_panel()
                .size(width(cx))
                .size_range(zpx(MIN_WIDTH)..zpx(MAX_WIDTH))
                .child(strip),
        )
        .child(resizable_panel().child(main))
        .into_any_element()
}

/// The strip's frame: the Changed only box over `list`, focusable, with a
/// border on the right.
pub fn frame(
    id: &'static str,
    label: &'static str,
    focus: &FocusHandle,
    list: AnyElement,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.ghd();
    let on = changed_only(cx);
    div()
        .id(id)
        .track_focus(focus)
        .role(Role::List)
        .aria_label(label)
        .key_context(KEY_CONTEXT)
        .size_full()
        .min_h_0()
        .flex()
        .flex_col()
        .border_r_1()
        .border_color(t.box_border)
        .bg(t.box_alt_background)
        .child(
            div()
                .flex_none()
                .h(zpx(29.))
                .px(SPACING())
                .flex()
                .items_center()
                .border_b_1()
                .border_color(t.box_border)
                .text_size(FONT_SIZE_SM())
                .text_color(t.text)
                .child(crate::widgets::checkbox_row(
                    "page-strip-changed-only",
                    on,
                    "Changed only",
                    |on, _, cx| Dispatcher::set_page_strip_changed_only(on, cx),
                    cx,
                )),
        )
        .child(div().flex_1().min_h_0().flex().flex_col().child(list))
}

/// The mark of a row: its icon, colour and name (`None`: unchanged or not
/// compared yet).
pub fn status_mark(status: PageStatus, t: &GhdTheme) -> Option<(Octicon, Hsla, &'static str)> {
    match status {
        PageStatus::Added => Some((Octicon::DiffAdded, t.color_new, "Added")),
        PageStatus::Removed => Some((Octicon::DiffRemoved, t.color_deleted, "Removed")),
        PageStatus::Changed => Some((Octicon::DiffModified, t.color_modified, "Changed")),
        PageStatus::Same | PageStatus::Unknown => None,
    }
}

/// A pair's page numbers, old → new ("3" when both are page 3).
pub fn pair_label(pair: &PagePair) -> String {
    match (pair.previous, pair.current) {
        (Some(a), Some(b)) if a == b => format!("{}", a + 1),
        (Some(a), Some(b)) => format!("{} → {}", a + 1, b + 1),
        (None, Some(b)) => format!("– → {}", b + 1),
        (Some(a), None) => format!("{} → –", a + 1),
        (None, None) => String::new(),
    }
}

/// The rows a strip lists: every row, or with Changed only the rows not
/// known to be unchanged.
pub fn visible_rows(statuses: impl Iterator<Item = PageStatus>, changed_only: bool) -> Vec<usize> {
    statuses
        .enumerate()
        .filter(|(_, s)| !changed_only || *s != PageStatus::Same)
        .map(|(i, _)| i)
        .collect()
}

/// The row `step` rows away from `current` among `visible` (the first or
/// last when `current` is not listed).
pub fn step_row(visible: &[usize], current: usize, step: isize) -> Option<usize> {
    let at = match visible.iter().position(|&r| r == current) {
        Some(at) => at.checked_add_signed(step)?,
        None if step > 0 => visible.iter().position(|&r| r > current)?,
        None => visible.iter().rposition(|&r| r < current)?,
    };
    visible.get(at).copied()
}

/// How a row draws.
pub struct RowLook {
    pub selected: bool,
    pub focused: bool,
}

fn row_base(
    id: ElementId,
    label: &str,
    look: &RowLook,
    status: PageStatus,
    t: &GhdTheme,
) -> Stateful<Div> {
    let hover = t.list_item_hover_background;
    div()
        .id(id)
        .a11y_row(label.to_string(), look.selected)
        .w_full()
        .flex_none()
        .cursor_pointer()
        .when(status == PageStatus::Same && !look.selected, |d| {
            d.opacity(0.65)
        })
        .when(look.selected, |d| {
            if look.focused {
                d.bg(t.box_selected_active_background)
                    .text_color(t.box_selected_active_text)
            } else {
                d.bg(t.box_selected_background)
                    .text_color(t.box_selected_text)
            }
        })
        .when(!look.selected, move |d| d.hover(move |s| s.bg(hover)))
}

fn mark_icon(status: PageStatus, look: &RowLook, t: &GhdTheme) -> Option<AnyElement> {
    let (icon, color, tip) = status_mark(status, t)?;
    let color = if look.selected && look.focused {
        t.box_selected_active_text
    } else {
        color
    };
    Some(
        div()
            .id(tip)
            .flex_none()
            .child(octicon(icon, color))
            .ghd_tooltip(tip)
            .into_any_element(),
    )
}

/// A page row: the thumbnail (`None` while it is drawn) of `aspect`
/// (width / height), and the page numbers with the mark under it.
pub fn thumb_row(
    id: ElementId,
    pair: &PagePair,
    look: RowLook,
    thumb: Option<Arc<RenderImage>>,
    aspect: f32,
    t: &GhdTheme,
) -> Stateful<Div> {
    let label = pair_label(pair);
    let a11y = match status_mark(pair.status, t) {
        Some((_, _, name)) => format!("Page {label}, {}", name.to_lowercase()),
        None if pair.status == PageStatus::Same => format!("Page {label}, unchanged"),
        None => format!("Page {label}"),
    };
    let frame_color = status_mark(pair.status, t).map_or(t.box_border, |(_, c, _)| c);
    let height = THUMB_HEIGHT;
    let width = (height * aspect.clamp(0.2, 5.)).round();
    let page = div()
        .flex_none()
        .w(zpx(width))
        .h(zpx(height))
        .max_w_full()
        .overflow_hidden()
        .bg(gpui::white())
        .border_1()
        .border_color(frame_color)
        .when(
            pair.status != PageStatus::Same && pair.status != PageStatus::Unknown,
            |d| d.border_2(),
        )
        .children(thumb.map(|image| {
            img(ImageSource::Render(image))
                .size_full()
                .object_fit(ObjectFit::Contain)
        }));
    let removed = pair.status == PageStatus::Removed;
    row_base(id, &a11y, &look, pair.status, t)
        .h(zpx(THUMB_ROW_HEIGHT))
        .px(SPACING())
        .pt(zpx(6.))
        .flex()
        .flex_col()
        .items_center()
        .gap(zpx(4.))
        .child(page)
        .child(
            div()
                .flex_none()
                .max_w_full()
                .flex()
                .flex_row()
                .items_center()
                .gap(SPACING_HALF())
                .text_size(FONT_SIZE_SM())
                .children(mark_icon(pair.status, &look, t))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .when(removed, |d| d.line_through())
                        .child(label),
                ),
        )
}

/// A chapter row: the mark and the name.
pub fn name_row(
    id: ElementId,
    name: SharedString,
    status: PageStatus,
    look: RowLook,
    t: &GhdTheme,
) -> Stateful<Div> {
    let removed = status == PageStatus::Removed;
    let a11y = match status_mark(status, t) {
        Some((_, _, mark)) => format!("{name}, {}", mark.to_lowercase()),
        None => name.to_string(),
    };
    row_base(id, &a11y, &look, status, t)
        .h(ROW_HEIGHT())
        .px(SPACING())
        .flex()
        .flex_row()
        .items_center()
        .gap(SPACING_HALF())
        .text_size(FONT_SIZE())
        .child(
            div()
                .flex_none()
                .w(zpx(16.))
                .flex()
                .justify_center()
                .children(mark_icon(status, &look, t)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .when(removed, |d| d.line_through())
                .child(name),
        )
}

/// An empty strip's note.
pub fn empty_note(text: &'static str, t: &GhdTheme) -> AnyElement {
    div()
        .flex_1()
        .p(SPACING())
        .flex()
        .justify_center()
        .text_size(FONT_SIZE_SM())
        .text_color(t.text_secondary)
        .child(text)
        .into_any_element()
}

/// A PDF page's pixels (RGBA) as an image GPUI draws (BGRA).
pub fn render_image(width: u32, height: u32, mut rgba: Vec<u8>) -> Option<Arc<RenderImage>> {
    for p in rgba.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(width, height, rgba)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(previous: Option<usize>, current: Option<usize>) -> PagePair {
        PagePair {
            previous,
            current,
            status: PageStatus::Unknown,
        }
    }

    #[test]
    fn labels_old_and_new_numbers() {
        assert_eq!(pair_label(&pair(Some(2), Some(2))), "3");
        assert_eq!(pair_label(&pair(Some(1), Some(2))), "2 → 3");
        assert_eq!(pair_label(&pair(None, Some(2))), "– → 3");
        assert_eq!(pair_label(&pair(Some(4), None)), "5 → –");
    }

    #[test]
    fn changed_only_keeps_unknown_rows() {
        use PageStatus::*;
        let statuses = [Same, Changed, Unknown, Same, Added];
        assert_eq!(visible_rows(statuses.into_iter(), true), [1, 2, 4]);
        assert_eq!(visible_rows(statuses.into_iter(), false), [0, 1, 2, 3, 4]);
    }

    #[test]
    fn steps_through_listed_rows() {
        let visible = [1, 2, 4];
        assert_eq!(step_row(&visible, 2, 1), Some(4));
        assert_eq!(step_row(&visible, 4, 1), None);
        assert_eq!(step_row(&visible, 1, -1), None);
        // from a row the list hides
        assert_eq!(step_row(&visible, 3, 1), Some(4));
        assert_eq!(step_row(&visible, 3, -1), Some(2));
        assert_eq!(step_row(&visible, 0, -1), None);
    }
}
