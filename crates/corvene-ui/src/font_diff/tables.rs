//! The Tables tab: the sfnt table directory of both versions, then a
//! table's fields old next to new (every table read-fonts can walk, glyph
//! ids written as names), the `name` records, and the character coverage
//! per Unicode block.

use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use corvene_fonts::compare::{FaceDiff, Status};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::widgets::IconButtonA11y;

use super::{FontDiffView, codepoint_label, printable, status_color};
use crate::format::format_bytes;
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, GhdTheme};

/// Pseudo entries listed above the tables.
const NAMES: &str = "\u{0}names";
const COVERAGE: &str = "\u{0}coverage";

#[derive(Default)]
pub(super) struct TablesUi {
    selected: Option<String>,
    /// Show every field, not only changed ones.
    all_fields: bool,
    scroll: UniformListScrollHandle,
    /// (table, all, query, field indexes) of the last filtering.
    listed: Option<(String, bool, String, Rc<Vec<usize>>)>,
}

fn row_height() -> Pixels {
    zpx(22.)
}

impl FontDiffView {
    pub(super) fn tables(
        &mut self,
        face: &Arc<FaceDiff>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd().clone();
        let both = face.old.is_some() && face.new.is_some();
        let selected = {
            let ui = &mut self.face_ui().tables;
            if ui.selected.is_none() {
                // the first changed table, else the coverage
                ui.selected = Some(
                    face.tables
                        .iter()
                        .find(|r| r.status.is_change() && face.table_fields.contains_key(&r.tag))
                        .map(|r| r.tag.clone())
                        .unwrap_or_else(|| COVERAGE.to_string()),
                );
            }
            ui.selected.clone().unwrap_or_default()
        };
        let names_changed = face.summary.names_changed;
        let coverage_changed = face.summary.codepoints_added + face.summary.codepoints_removed;
        let mut entries: Vec<(String, String, Status, String)> = vec![
            (
                COVERAGE.to_string(),
                "Characters".to_string(),
                if coverage_changed > 0 && both {
                    Status::Modified
                } else {
                    Status::Unchanged
                },
                face.new
                    .as_ref()
                    .or(face.old.as_ref())
                    .map(|r| format!("{} code points", r.cmap.len()))
                    .unwrap_or_default(),
            ),
            (
                NAMES.to_string(),
                "Names".to_string(),
                if names_changed > 0 && both {
                    Status::Modified
                } else {
                    Status::Unchanged
                },
                format!("{} records", face.names.len()),
            ),
        ];
        for r in &face.tables {
            let size = match (r.old_len, r.new_len) {
                (Some(a), Some(b)) if a != b => {
                    let d = i64::from(b) - i64::from(a);
                    format!(
                        "{} ({}{})",
                        format_bytes(u64::from(b) as i64, 1),
                        if d > 0 { "+" } else { "−" },
                        format_bytes(d.unsigned_abs() as i64, 1)
                    )
                }
                (_, Some(b)) => format_bytes(u64::from(b) as i64, 1),
                (Some(a), None) => format_bytes(u64::from(a) as i64, 1),
                _ => String::new(),
            };
            entries.push((r.tag.clone(), r.tag.clone(), r.status, size));
        }
        let list =
            div()
                .id("font-table-list")
                .w(zpx(220.))
                .flex_none()
                .h_full()
                .overflow_y_scroll()
                .border_r_1()
                .border_color(t.box_border)
                .py(SPACING_HALF())
                .children(entries.into_iter().enumerate().map(
                    |(i, (key, label, status, detail))| {
                        let is_selected = key == selected;
                        let hover = t.box_hover_background;
                        div()
                            .id(("font-table", i))
                            .a11y_button(label.clone())
                            .h(zpx(26.))
                            .px(SPACING())
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(SPACING_HALF())
                            .cursor_pointer()
                            .when(is_selected, |d| {
                                d.bg(t.box_selected_active_background)
                                    .text_color(t.box_selected_active_text)
                            })
                            .when(!is_selected, |d| d.hover(move |d| d.bg(hover)))
                            .child(
                                super::status_dot(status, &t)
                                    .when(status == Status::Unchanged, |d| {
                                        d.bg(transparent_black())
                                    }),
                            )
                            .child(
                                div()
                                    .font_family(crate::theme::mono_font())
                                    .text_size(FONT_SIZE_SM())
                                    .child(label),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_size(zpx(10.))
                                    .text_color(if is_selected {
                                        t.box_selected_active_text
                                    } else {
                                        t.text_secondary
                                    })
                                    .child(detail),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let ui = &mut this.face_ui().tables;
                                ui.selected = Some(key.clone());
                                ui.listed = None;
                                cx.notify();
                            }))
                    },
                ));
        let body = match selected.as_str() {
            NAMES => self.name_records(face, both, &t, cx),
            COVERAGE => coverage(face, both, &t),
            tag => self.table_fields(face, tag, both, window, cx),
        };
        div()
            .size_full()
            .flex()
            .flex_row()
            .child(list)
            .child(div().flex_1().min_w_0().h_full().child(body))
            .into_any_element()
    }

    fn name_records(
        &mut self,
        face: &FaceDiff,
        both: bool,
        t: &GhdTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let all = self.face_ui().tables.all_fields || !both;
        let rows: Vec<_> = face
            .names
            .iter()
            .filter(|r| all || r.status().is_change())
            .cloned()
            .collect();
        div()
            .id("font-names")
            .size_full()
            .overflow_y_scroll()
            .p(SPACING())
            .flex()
            .flex_col()
            .child(self.all_toggle(both, cx))
            .when(rows.is_empty(), |d| {
                d.child(
                    div()
                        .text_color(t.text_secondary)
                        .child("No name record changed."),
                )
            })
            .children(rows.into_iter().enumerate().map(|(i, r)| {
                let status = r.status();
                let platform = match r.platform {
                    0 => "Unicode",
                    1 => "Mac",
                    3 => "Windows",
                    _ => "Other",
                };
                let value = |v: &Option<String>, bg: Option<Hsla>| {
                    div()
                        .flex_1()
                        .min_w_0()
                        .px(SPACING_HALF())
                        .py(zpx(2.))
                        .rounded(zpx(3.))
                        .when_some(bg, |d, bg| d.bg(bg))
                        .text_color(if v.is_some() {
                            t.text
                        } else {
                            t.text_secondary
                        })
                        .child(v.clone().unwrap_or_else(|| "—".into()))
                };
                div()
                    .id(("font-name-row", i))
                    .flex()
                    .flex_row()
                    .gap(SPACING_HALF())
                    .py(zpx(3.))
                    .border_b_1()
                    .border_color(t.box_border.opacity(0.4))
                    .child(
                        div()
                            .w(zpx(200.))
                            .flex_none()
                            .flex()
                            .flex_col()
                            .child(format!(
                                "{} · {}",
                                r.name_id,
                                corvene_fonts::report::name_id_label(r.name_id)
                            ))
                            .child(
                                div()
                                    .text_size(zpx(10.))
                                    .text_color(t.text_secondary)
                                    .child(format!("{platform} · {}", r.language_tag)),
                            ),
                    )
                    .map(|d| {
                        if status.is_change() && both {
                            d.child(value(&r.old, Some(t.diff_delete_background)))
                                .child(value(&r.new, Some(t.diff_add_background)))
                        } else {
                            d.child(value(if r.new.is_some() { &r.new } else { &r.old }, None))
                        }
                    })
            }))
            .into_any_element()
    }

    fn all_toggle(&mut self, both: bool, cx: &mut Context<Self>) -> AnyElement {
        if !both {
            return div().into_any_element();
        }
        let all = self.face_ui().tables.all_fields;
        let view = cx.entity().downgrade();
        div()
            .pb(SPACING_HALF())
            .child(crate::widgets::checkbox_row(
                "font-fields-all",
                all,
                "Show unchanged",
                move |on, _, cx| {
                    view.update(cx, |v, cx| {
                        let ui = &mut v.face_ui().tables;
                        ui.all_fields = on;
                        ui.listed = None;
                        cx.notify();
                    })
                    .ok();
                },
                cx,
            ))
            .into_any_element()
    }

    fn table_fields(
        &mut self,
        face: &Arc<FaceDiff>,
        tag: &str,
        both: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd().clone();
        let Some(fields) = face.table_fields.get(tag) else {
            let row = face.tables.iter().find(|r| r.tag == tag);
            let text = match row {
                Some(r) if r.status == Status::Modified => format!(
                    "{tag} changed ({} → {} bytes). Its contents are compared in the Glyphs tab or are opaque data.",
                    r.old_len.unwrap_or(0),
                    r.new_len.unwrap_or(0)
                ),
                _ => format!("{tag} is not listed field by field."),
            };
            return div()
                .p(SPACING())
                .text_color(t.text_secondary)
                .child(text)
                .into_any_element();
        };
        let query = self
            .inputs
            .as_ref()
            .map(|i| i.field_filter.read(cx).value().to_lowercase())
            .unwrap_or_default();
        let all = self.face_ui().tables.all_fields || !both;
        let rows = {
            let ui = &mut self.face_ui().tables;
            match &ui.listed {
                Some((k, a, q, rows)) if k == tag && *a == all && *q == query => rows.clone(),
                _ => {
                    let rows: Rc<Vec<usize>> = Rc::new(
                        fields
                            .iter()
                            .enumerate()
                            .filter(|(_, f)| all || f.status().is_change())
                            .filter(|(_, f)| {
                                query.is_empty() || f.label.to_lowercase().contains(&query)
                            })
                            .map(|(i, _)| i)
                            .collect(),
                    );
                    ui.listed = Some((tag.to_string(), all, query.clone(), rows.clone()));
                    rows
                }
            }
        };
        let truncated = face.table_fields_truncated.contains(tag);
        let scroll = self.face_ui().tables.scroll.clone();
        let face = face.clone();
        let tag_owned = tag.to_string();
        let changed = fields.iter().filter(|f| f.status().is_change()).count();
        let list: AnyElement = if rows.is_empty() {
            div()
                .p(SPACING())
                .text_color(t.text_secondary)
                .child(if changed == 0 && both {
                    "No field changed."
                } else {
                    "No fields match."
                })
                .into_any_element()
        } else {
            let t2 = t.clone();
            uniform_list(
                "font-table-fields",
                rows.len(),
                cx.processor(move |_, range: Range<usize>, _, _| {
                    let Some(fields) = face.table_fields.get(&tag_owned) else {
                        return Vec::new();
                    };
                    range
                        .filter_map(|i| {
                            let f = fields.get(*rows.get(i)?)?;
                            Some(field_line(i, f, both, &t2))
                        })
                        .collect()
                }),
            )
            .track_scroll(&scroll)
            .size_full()
            .into_any_element()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING())
                    .p(SPACING())
                    .border_b_1()
                    .border_color(t.box_border)
                    .children(self.inputs.as_ref().map(|i| {
                        div().w(zpx(240.)).child(crate::widgets::filter_text_box(
                            "font-field-filter",
                            &i.field_filter,
                            None,
                            window,
                            cx,
                        ))
                    }))
                    .child(self.all_toggle(both, cx))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(FONT_SIZE_SM())
                            .text_color(t.text_secondary)
                            .child(format!(
                                "{changed} of {} fields changed{}",
                                fields.len(),
                                if truncated {
                                    " (first 40,000 read)"
                                } else {
                                    ""
                                }
                            )),
                    ),
            )
            .child(div().flex_1().min_h_0().child(list))
            .into_any_element()
    }
}

/// One field: path, then the value, or old and new when it changed.
fn field_line(i: usize, f: &corvene_fonts::compare::Field, both: bool, t: &GhdTheme) -> AnyElement {
    let status = f.status();
    let changed = status.is_change() && both;
    let cell = |v: &Option<String>, bg: Option<Hsla>| {
        div()
            .flex_1()
            .min_w_0()
            .px(SPACING_HALF())
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .when_some(bg, |d, bg| d.bg(bg))
            .text_color(if v.is_some() {
                t.text
            } else {
                t.text_secondary
            })
            .child(v.clone().unwrap_or_else(|| "—".into()))
    };
    div()
        .id(("font-field", i))
        .h(row_height())
        .flex()
        .flex_row()
        .items_center()
        .gap(SPACING_HALF())
        .px(SPACING())
        .font_family(crate::theme::mono_font())
        .text_size(FONT_SIZE_SM())
        .border_b_1()
        .border_color(t.box_border.opacity(0.3))
        .child(
            div()
                .w(relative(0.42))
                .flex_none()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(if changed {
                    status_color(status, t)
                } else {
                    t.text_secondary
                })
                .child(f.label.clone()),
        )
        .map(|d| {
            if changed {
                d.child(cell(&f.old, Some(t.diff_delete_background)))
                    .child(cell(&f.new, Some(t.diff_add_background)))
            } else {
                d.child(cell(if f.new.is_some() { &f.new } else { &f.old }, None))
            }
        })
        .into_any_element()
}

/// Code points per Unicode block, with the ones added and removed.
fn coverage(face: &FaceDiff, both: bool, t: &GhdTheme) -> AnyElement {
    let chars = |cps: &[u32]| -> String {
        cps.iter()
            .take(200)
            .map(|&cp| {
                printable(cp)
                    .map(String::from)
                    .unwrap_or_else(|| codepoint_label(cp))
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    div()
        .id("font-coverage")
        .size_full()
        .overflow_y_scroll()
        .p(SPACING())
        .flex()
        .flex_col()
        .gap(zpx(2.))
        .children(face.coverage.iter().enumerate().map(|(i, b)| {
            let status = if both { b.status() } else { Status::Unchanged };
            let size = b.size() as f32;
            let fraction = |n: u32| (n as f32 / size).clamp(0., 1.);
            div()
                .id(("font-block", i))
                .flex()
                .flex_col()
                .py(zpx(4.))
                .border_b_1()
                .border_color(t.box_border.opacity(0.4))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(SPACING_HALF())
                        .child(
                            super::status_dot(status, t)
                                .when(status == Status::Unchanged, |d| d.bg(transparent_black())),
                        )
                        .child(div().w(zpx(240.)).flex_none().child(b.name))
                        .child(
                            div()
                                .w(zpx(130.))
                                .flex_none()
                                .text_size(zpx(10.))
                                .text_color(t.text_secondary)
                                .child(format!(
                                    "{}–{}",
                                    codepoint_label(b.start),
                                    codepoint_label(b.end)
                                )),
                        )
                        .child(
                            // coverage bars: old under new
                            div()
                                .w(zpx(160.))
                                .flex_none()
                                .flex()
                                .flex_col()
                                .gap(px(1.))
                                .when(both, |d| {
                                    d.child(
                                        div()
                                            .h(zpx(3.))
                                            .w(relative(fraction(b.old_count)))
                                            .bg(t.color_deleted.opacity(0.6)),
                                    )
                                })
                                .child(
                                    div()
                                        .h(zpx(3.))
                                        .w(relative(fraction(b.new_count.max(if both {
                                            0
                                        } else {
                                            b.old_count
                                        }))))
                                        .bg(t.focus),
                                ),
                        )
                        .child(
                            div()
                                .text_size(FONT_SIZE_SM())
                                .text_color(t.text_secondary)
                                .child(if both && b.old_count != b.new_count {
                                    format!("{} → {} of {}", b.old_count, b.new_count, b.size())
                                } else {
                                    format!("{} of {}", b.new_count.max(b.old_count), b.size())
                                }),
                        ),
                )
                .when(!b.added.is_empty(), |d| {
                    d.child(
                        div()
                            .pl(zpx(16.))
                            .text_color(t.color_new)
                            .child(format!("+ {}", chars(&b.added))),
                    )
                })
                .when(!b.removed.is_empty(), |d| {
                    d.child(
                        div()
                            .pl(zpx(16.))
                            .text_color(t.color_deleted)
                            .child(format!("− {}", chars(&b.removed))),
                    )
                })
                .when(!b.remapped.is_empty(), |d| {
                    d.child(
                        div()
                            .pl(zpx(16.))
                            .text_color(t.color_modified)
                            .child(format!(
                                "now drawn by another glyph: {}",
                                chars(&b.remapped)
                            )),
                    )
                })
        }))
        .into_any_element()
}
