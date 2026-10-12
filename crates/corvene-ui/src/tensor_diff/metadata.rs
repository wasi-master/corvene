//! The Metadata tab: each key with its value before and after, grouped by
//! section, changed entries highlighted.

use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::Data;
use super::model::MetaItem;
use crate::icons::{Octicon, octicon};
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, GhdTheme, mono_font};
use crate::widgets::{GhdTooltip, IconButtonA11y};

const ROW: f32 = 24.;
/// The key column's share of the row.
const KEY_SHARE: f32 = 0.32;
/// Longest tooltip text.
const TOOLTIP_CHARS: usize = 1500;

/// The column header: Key, then Before / After (or Value for a file that
/// exists on one side).
pub fn header(data: &Data, t: &GhdTheme) -> Div {
    let both = data.both();
    div()
        .flex_none()
        .w_full()
        .h(zpx(ROW))
        .flex()
        .flex_row()
        .items_center()
        .pl(SPACING())
        .pr(SPACING_HALF())
        .bg(t.diff_hunk_background)
        .border_b_1()
        .border_color(t.diff_border)
        .text_size(FONT_SIZE_SM())
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(t.text_secondary)
        .child(div().flex_none().w(relative(KEY_SHARE)).child("Key"))
        .when(both, |d| {
            d.child(div().flex_1().min_w_0().px(zpx(6.)).child("Before"))
                .child(div().flex_1().min_w_0().px(zpx(6.)).child("After"))
        })
        .when(!both, |d| {
            d.child(div().flex_1().min_w_0().px(zpx(6.)).child("Value"))
        })
}

fn one_line(text: &str) -> String {
    if text.contains('\n') {
        text.replace("\r\n", "\n").replace('\n', "⏎")
    } else {
        text.to_string()
    }
}

fn clip(text: &str) -> String {
    if text.chars().count() <= TOOLTIP_CHARS {
        text.to_string()
    } else {
        let mut s: String = text.chars().take(TOOLTIP_CHARS).collect();
        s.push('…');
        s
    }
}

/// What the rows of the list need.
pub struct Rows {
    pub data: Rc<Data>,
    pub items: Rc<Vec<MetaItem>>,
}

impl Rows {
    pub fn row(&self, ix: usize, cx: &mut App) -> AnyElement {
        let t = cx.ghd().clone();
        match self.items.get(ix) {
            Some(MetaItem::Section { title, changed }) => {
                let title = if title.is_empty() {
                    "File".to_string()
                } else {
                    title.clone()
                };
                div()
                    .w_full()
                    .h(zpx(ROW + 4.))
                    .flex()
                    .flex_row()
                    .items_end()
                    .gap(SPACING_HALF())
                    .px(SPACING())
                    .pb(zpx(4.))
                    .border_b_1()
                    .border_color(t.diff_border)
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                    .when(*changed > 0, |d| {
                        d.child(
                            div()
                                .text_size(FONT_SIZE_SM())
                                .text_color(t.color_modified)
                                .child(format!(
                                    "{} changed",
                                    crate::format::format_count(*changed as u64)
                                )),
                        )
                    })
                    .into_any_element()
            }
            Some(MetaItem::Entry(e)) => self.entry(*e, &t),
            None => div().into_any_element(),
        }
    }

    fn entry(&self, e: usize, t: &GhdTheme) -> AnyElement {
        let Some(m) = self.data.meta.get(e) else {
            return div().into_any_element();
        };
        let both = self.data.both();
        let (row_bg, old_bg, new_bg) = match (&m.old, &m.new) {
            _ if !both => (t.background, None, None),
            (None, Some(_)) => (t.diff_add_background, None, None),
            (Some(_), None) => (t.diff_delete_background, None, None),
            (Some(o), Some(n)) if o != n => (
                t.background,
                Some(t.diff_delete_inner_background),
                Some(t.diff_add_inner_background),
            ),
            _ => (t.background, None, None),
        };
        let group: SharedString = format!("tensor-meta-{e}").into();
        let value = |side: usize, v: &Option<String>, bg: Option<Hsla>| {
            let text = v.clone().unwrap_or_default();
            let shown = one_line(&text);
            let copy = text.clone();
            div()
                .id(("tensor-meta-value", e * 2 + side))
                .relative()
                .flex_1()
                .min_w_0()
                .h_full()
                .px(zpx(6.))
                .flex()
                .items_center()
                .when_some(bg, |d, bg| d.bg(bg))
                .font_family(mono_font())
                .child(div().w_full().truncate().child(shown))
                .when(!text.is_empty(), |d| {
                    d.ghd_tooltip(clip(&text)).child(
                        div()
                            .id(("tensor-meta-copy", e * 2 + side))
                            .icon_button_label("Copy value")
                            .absolute()
                            .right(zpx(2.))
                            .top(zpx(2.))
                            .size(zpx(ROW - 5.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(BORDER_RADIUS())
                            .bg(t.box_background)
                            .border_1()
                            .border_color(t.box_border)
                            .cursor_pointer()
                            .invisible()
                            .group_hover(group.clone(), |s| s.visible())
                            .child(octicon(Octicon::Copy, t.text_secondary).size(zpx(12.)))
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                            }),
                    )
                })
        };
        div()
            .id(("tensor-meta-row", e))
            .group(group.clone())
            .w_full()
            .h(zpx(ROW))
            .flex()
            .flex_row()
            .items_center()
            .pl(SPACING())
            .pr(SPACING_HALF())
            .bg(row_bg)
            .text_color(t.diff_text)
            .border_b_1()
            .border_color(t.diff_border)
            .child(
                div()
                    .id(("tensor-meta-key", e))
                    .flex_none()
                    .w(relative(KEY_SHARE))
                    .pr(SPACING_HALF())
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis_middle()
                    .font_family(mono_font())
                    .when(m.new.is_none() && both, |d| d.line_through())
                    .child(m.key.clone())
                    .ghd_tooltip(m.key.clone()),
            )
            .when(both, |d| {
                d.child(value(0, &m.old, old_bg))
                    .child(value(1, &m.new, new_bg))
            })
            .when(!both, |d| {
                d.child(value(
                    1,
                    if m.new.is_some() { &m.new } else { &m.old },
                    None,
                ))
            })
            .into_any_element()
    }
}
