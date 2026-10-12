//! The Overview tab: a sample of each version, then what changed, then
//! every identity and metric field old next to new, then the variation
//! axes and named instances.

use std::sync::Arc;

use corvene_fonts::compare::{FaceDiff, Field, ItemRow, Status};
use corvene_fonts::render::{TextBlock, TextOptions, render_text};
use corvene_models::FontDiffView as Tab;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::widgets::IconButtonA11y;

use super::raster::{RasterKey, image_el, ink};
use super::{FontDiffView, GlyphFilter, status_color};
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, GhdTheme};
use corvene_core::Dispatcher;

/// The sample in each card.
const SAMPLE: &str = "Aa Gg Rr 0123 &?!";
/// Card sample width, points.
const CARD_WIDTH: f32 = 360.;

impl FontDiffView {
    /// A side's sample as an image.
    fn card_sample(
        &mut self,
        face: &FaceDiff,
        new_side: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd().clone();
        let report = if new_side {
            face.new.clone()
        } else {
            face.old.clone()
        };
        let (Some(report), Some(decoded)) = (report, self.decoded(new_side)) else {
            return div().into_any_element();
        };
        let scale = window.scale_factor();
        let color = t.text;
        let key = RasterKey::of((
            "overview-card",
            self.face,
            new_side,
            report.index,
            scale.to_bits(),
            color.to_rgb().r.to_bits(),
            color.to_rgb().g.to_bits(),
        ));
        let index = report.index;
        let ink = ink(color);
        let drawn = self.rasters.get(
            key,
            scale,
            move || {
                let font = decoded.face(index).ok()?;
                let layout = render_text(
                    &font,
                    &[TextBlock {
                        text: SAMPLE.to_string(),
                        size: 40. * scale,
                    }],
                    &TextOptions {
                        features: Vec::new(),
                        variations: Vec::new(),
                        width: CARD_WIDTH * scale,
                        line_height: 1.1,
                        block_gap: 0.,
                        padding: 0.,
                        ink,
                    },
                );
                Some(layout.raster)
            },
            cx,
        );
        match drawn {
            Some(d) => image_el(&d).into_any_element(),
            None => div().h(zpx(52.)).into_any_element(),
        }
    }

    fn card(
        &mut self,
        face: &FaceDiff,
        new_side: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd().clone();
        let report = if new_side {
            face.new.clone()
        } else {
            face.old.clone()
        };
        let both = face.old.is_some() && face.new.is_some();
        let heading = match (both, new_side) {
            (true, false) => "Before",
            (true, true) => "After",
            (false, true) => "Added",
            (false, false) => "Deleted",
        };
        let accent = match (both, new_side) {
            (true, _) => t.box_border,
            (false, true) => t.color_new,
            (false, false) => t.color_deleted,
        };
        let Some(report) = report else {
            return div().flex_1().into_any_element();
        };
        let decoded = self.decoded(new_side);
        let format = decoded.as_ref().map(|d| {
            let size = crate::format::format_bytes(d.file_len as i64, 1);
            if d.format.is_compressed() {
                format!(
                    "{} · {} ({} unpacked)",
                    d.format.label(),
                    size,
                    crate::format::format_bytes(d.decoded_len() as i64, 1)
                )
            } else {
                format!("{} · {}", report.outline.label(), size)
            }
        });
        let sample = self.card_sample(face, new_side, window, cx);
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(SPACING_HALF())
            .p(SPACING())
            .rounded(BORDER_RADIUS())
            .border_1()
            .border_color(accent)
            .bg(t.box_background)
            .overflow_hidden()
            .child(
                div()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(heading),
            )
            .child(sample)
            .child(
                div()
                    .text_size(FONT_SIZE_MD())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(report.title.clone()),
            )
            .children(format.map(|f| {
                div()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(f)
            }))
            .into_any_element()
    }

    pub(super) fn overview(
        &mut self,
        face: &Arc<FaceDiff>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd().clone();
        let both = face.old.is_some() && face.new.is_some();
        let old_card = self.card(face, false, window, cx);
        let new_card = self.card(face, true, window, cx);
        let changes = if both {
            what_changed(face, &t, cx)
        } else {
            Vec::new()
        };
        div()
            .id("font-overview")
            .size_full()
            .overflow_y_scroll()
            .p(SPACING())
            .flex()
            .flex_col()
            .gap(SPACING())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(SPACING())
                    .when(face.old.is_some(), |d| d.child(old_card))
                    .when(face.new.is_some(), |d| d.child(new_card)),
            )
            .when(!changes.is_empty(), |d| {
                d.child(section("What changed", &t))
                    .child(div().flex().flex_col().gap(zpx(2.)).children(changes))
            })
            .when(both && face.summary == Default::default(), |d| {
                d.child(div().text_color(t.text_secondary).child(
                    "The fonts draw the same; only bytes that do not show changed (see Tables).",
                ))
            })
            .child(section("Font", &t))
            .child(field_grid("font-overview-fields", &face.overview, both, &t))
            .when(!face.axes.is_empty(), |d| {
                d.child(section("Variation axes", &t)).child(item_grid(
                    "font-overview-axes",
                    &face.axes,
                    both,
                    &t,
                ))
            })
            .when(!face.instances.is_empty(), |d| {
                d.child(section("Named instances", &t)).child(item_grid(
                    "font-overview-instances",
                    &face.instances,
                    both,
                    &t,
                ))
            })
            .into_any_element()
    }
}

/// A section heading.
pub(super) fn section(title: &str, t: &GhdTheme) -> Div {
    div()
        .mt(SPACING_HALF())
        .pb(zpx(4.))
        .border_b_1()
        .border_color(t.box_border)
        .text_size(FONT_SIZE_SM())
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(t.text_secondary)
        .child(title.to_string())
}

/// One line of "What changed", opening `tab` when clicked.
fn change_line(
    id: (&'static str, usize),
    status: Status,
    text: String,
    tab: Tab,
    filter: Option<GlyphFilter>,
    t: &GhdTheme,
    cx: &mut Context<FontDiffView>,
) -> AnyElement {
    let hover = t.box_hover_background;
    div()
        .id(id)
        .a11y_button(text.clone())
        .flex()
        .flex_row()
        .items_center()
        .gap(SPACING_HALF())
        .px(SPACING_HALF())
        .py(zpx(3.))
        .rounded(BORDER_RADIUS())
        .cursor_pointer()
        .hover(move |d| d.bg(hover))
        .child(super::status_dot(status, t))
        .child(text)
        .on_click(cx.listener(move |this, _, _, cx| {
            if let Some(filter) = filter {
                this.show_glyphs(filter, cx);
            } else {
                Dispatcher::set_font_diff_view(tab, cx);
            }
        }))
        .into_any_element()
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// Sentences for what changed, most visible first.
fn what_changed(face: &FaceDiff, t: &GhdTheme, cx: &mut Context<FontDiffView>) -> Vec<AnyElement> {
    let s = face.summary;
    let mut out = Vec::new();
    let mut i = 0;
    let mut push = |status, text: String, tab, filter, cx: &mut Context<FontDiffView>| {
        out.push(change_line(
            ("font-change", i),
            status,
            text,
            tab,
            filter,
            t,
            cx,
        ));
        i += 1;
    };
    let names = |status: Status| -> String {
        let names: Vec<&str> = face
            .glyphs
            .iter()
            .filter(|g| g.status == status)
            .take(8)
            .map(|g| g.name.as_str())
            .collect();
        names.join(" ")
    };
    if s.glyphs_added > 0 {
        push(
            Status::Added,
            format!(
                "{} added: {}",
                plural(s.glyphs_added as usize, "glyph", "glyphs"),
                names(Status::Added)
            ),
            Tab::Glyphs,
            Some(GlyphFilter::Added),
            cx,
        );
    }
    if s.glyphs_removed > 0 {
        push(
            Status::Removed,
            format!(
                "{} removed: {}",
                plural(s.glyphs_removed as usize, "glyph", "glyphs"),
                names(Status::Removed)
            ),
            Tab::Glyphs,
            Some(GlyphFilter::Removed),
            cx,
        );
    }
    if s.glyphs_modified > 0 {
        push(
            Status::Modified,
            format!(
                "{} changed: {}",
                plural(s.glyphs_modified as usize, "glyph", "glyphs"),
                names(Status::Modified)
            ),
            Tab::Glyphs,
            Some(GlyphFilter::Changed),
            cx,
        );
    }
    if s.codepoints_added + s.codepoints_removed > 0 {
        push(
            Status::Modified,
            format!(
                "Character coverage: +{} −{} code points",
                s.codepoints_added, s.codepoints_removed
            ),
            Tab::Tables,
            None,
            cx,
        );
    }
    for f in face.features.iter().filter(|f| f.status.is_change()) {
        let what = match f.status {
            Status::Added => "added",
            Status::Removed => "removed",
            _ => "changed",
        };
        let name = f
            .ui_name
            .clone()
            .or_else(|| corvene_fonts::layout::registered_name(&f.tag).map(str::to_string))
            .map(|n| format!(" ({n})"))
            .unwrap_or_default();
        push(
            f.status,
            format!("{} feature {}{name} {what}", f.table.label(), f.tag),
            Tab::Features,
            None,
            cx,
        );
    }
    if s.kerning_changed > 0 {
        push(
            Status::Modified,
            format!(
                "Kerning: {} changed",
                plural(s.kerning_changed as usize, "pair", "pairs")
            ),
            Tab::Features,
            None,
            cx,
        );
    }
    for f in face.overview.iter().filter(|f| f.status().is_change()) {
        push(
            f.status(),
            format!(
                "{}: {} → {}",
                f.label,
                f.old.as_deref().unwrap_or("—"),
                f.new.as_deref().unwrap_or("—")
            ),
            Tab::Overview,
            None,
            cx,
        );
    }
    if s.instances_changed > 0 {
        push(
            Status::Modified,
            format!("Named instances: {} changed", s.instances_changed),
            Tab::Overview,
            None,
            cx,
        );
    }
    if s.names_changed > 0 {
        push(
            Status::Modified,
            format!(
                "name table: {}",
                plural(
                    s.names_changed as usize,
                    "record changed",
                    "records changed"
                )
            ),
            Tab::Tables,
            None,
            cx,
        );
    }
    out
}

/// Label | old | new rows; changed values tinted.
pub(super) fn field_grid(
    id: &'static str,
    fields: &[Field],
    both: bool,
    t: &GhdTheme,
) -> Stateful<Div> {
    div().id(id).flex().flex_col().children(
        fields
            .iter()
            .enumerate()
            .map(|(i, f)| field_row((id, i), f, both, t)),
    )
}

pub(super) fn field_row(
    id: (&'static str, usize),
    f: &Field,
    both: bool,
    t: &GhdTheme,
) -> Stateful<Div> {
    let status = f.status();
    let value = |v: &Option<String>, bg: Option<Hsla>, strike: bool| {
        div()
            .flex_1()
            .min_w_0()
            .px(SPACING_HALF())
            .py(zpx(2.))
            .rounded(zpx(3.))
            .when_some(bg, |d, bg| d.bg(bg))
            .when(strike, |d| d.line_through())
            .text_color(if v.is_some() {
                t.text
            } else {
                t.text_secondary
            })
            .child(v.clone().unwrap_or_else(|| "—".to_string()))
    };
    let changed = status.is_change() && both;
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_start()
        .gap(SPACING_HALF())
        .py(zpx(1.))
        .border_b_1()
        .border_color(t.box_border.opacity(0.4))
        .child(
            div()
                .w(zpx(170.))
                .flex_none()
                .py(zpx(2.))
                .text_color(t.text_secondary)
                .child(f.label.clone()),
        )
        .map(|d| {
            if !both {
                d.child(value(
                    if f.old.is_some() { &f.old } else { &f.new },
                    None,
                    false,
                ))
            } else if !changed {
                // the same on both sides: one value across both columns
                d.child(value(&f.new, None, false))
            } else {
                d.child(value(&f.old, Some(t.diff_delete_background), false))
                    .child(value(&f.new, Some(t.diff_add_background), false))
            }
        })
        .child(
            div().w(zpx(8.)).flex_none().pt(zpx(7.)).child(
                div()
                    .size(zpx(6.))
                    .rounded_full()
                    .when(changed, |d| d.bg(status_color(status, t))),
            ),
        )
}

/// Axes or instances: a heading per item with its fields.
fn item_grid(id: &'static str, items: &[ItemRow], both: bool, t: &GhdTheme) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_col()
        .gap(SPACING_HALF())
        .children(items.iter().enumerate().map(|(i, item)| {
            div()
                .id((id, i))
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(SPACING_HALF())
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(super::status_dot(item.status, t))
                        .child(item.name.clone())
                        .child(
                            div()
                                .text_size(FONT_SIZE_SM())
                                .text_color(status_color(item.status, t))
                                .child(super::status_label(item.status)),
                        ),
                )
                .child(field_grid(id, &item.fields, both, t))
        }))
}
