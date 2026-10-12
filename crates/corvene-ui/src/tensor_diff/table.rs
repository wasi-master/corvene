//! The Tensors tab: the column header, tensor and group rows, and the
//! details under an opened tensor.

use std::collections::HashMap;
use std::rc::Rc;

use corvene_models::tensors::{
    TensorChange, TensorFormat, TensorInfo, TensorRow, TensorStats, TensorValues, ValueDiff,
};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::model::{self, Item};
use super::{Data, TensorDiffView, bytes};
use crate::icons::{Octicon, octicon};
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, GhdTheme, mono_font};
use crate::widgets::{GhdTooltip, IconButtonA11y, ListRowA11y};

/// Column widths, CSS px.
const NAME_MIN: f32 = 160.;
const STATUS: f32 = 78.;
const SHAPE: f32 = 170.;
const DTYPE: f32 = 104.;
const SIZE: f32 = 128.;
const MAX_DIFF: f32 = 76.;
const RMS: f32 = 76.;
const CHANGED: f32 = 66.;
const COSINE: f32 = 76.;
/// Indent per tree level, CSS px.
const INDENT: f32 = 14.;
const ROW: f32 = 24.;

/// The columns shown for the list's width.
#[derive(Clone, Copy, Debug, Default)]
pub struct Cols {
    shape: bool,
    dtype: bool,
    size: bool,
    max_diff: bool,
    rms: bool,
    changed: bool,
    cosine: bool,
    status: bool,
}

impl Cols {
    /// Adds columns by priority while the name keeps [`NAME_MIN`].
    pub fn fit(width: Pixels, values: bool, both: bool) -> Cols {
        let width = if width <= px(1.) { zpx(1400.) } else { width };
        let mut room = width - zpx(NAME_MIN) - zpx(SPACING_UNITS);
        let mut cols = Cols::default();
        let mut take = |w: f32| {
            if room >= zpx(w) {
                room -= zpx(w);
                true
            } else {
                false
            }
        };
        cols.status = take(STATUS);
        cols.shape = take(SHAPE);
        cols.dtype = take(DTYPE);
        if values && both {
            cols.max_diff = take(MAX_DIFF);
            cols.cosine = take(COSINE);
        }
        cols.size = take(SIZE);
        if values && both {
            cols.changed = take(CHANGED);
            cols.rms = take(RMS);
        }
        cols
    }

    fn values(&self) -> bool {
        self.max_diff || self.rms || self.changed || self.cosine
    }

    fn values_width(&self) -> Pixels {
        let mut w = 0.;
        for (on, width) in [
            (self.max_diff, MAX_DIFF),
            (self.rms, RMS),
            (self.changed, CHANGED),
            (self.cosine, COSINE),
        ] {
            if on {
                w += width;
            }
        }
        zpx(w)
    }
}

/// Room for the row's side padding, CSS px.
const SPACING_UNITS: f32 = 20.;

/// The names a row's tensors go by (both, when renamed).
pub fn names(row: &TensorRow) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in [&row.new, &row.old].into_iter().flatten() {
        if !out.contains(&t.name) {
            out.push(t.name.clone());
        }
    }
    out
}

/// The element comparison of a row, once read.
fn value_diff<'a>(
    row: &TensorRow,
    values: &'a HashMap<String, TensorValues>,
) -> Option<&'a ValueDiff> {
    if row.change == TensorChange::Renamed {
        return None;
    }
    values.get(row.name()).and_then(|v| v.diff.as_ref())
}

/// Whether the row's values were compared and differ.
pub fn value_changed(row: &TensorRow, values: &HashMap<String, TensorValues>) -> bool {
    value_diff(row, values).is_some_and(|d| !d.identical)
}

/// The statistics of each side, once read.
fn side_stats<'a>(
    row: &TensorRow,
    values: &'a HashMap<String, TensorValues>,
) -> (Option<&'a TensorStats>, Option<&'a TensorStats>) {
    let get = |t: &Option<TensorInfo>| t.as_ref().and_then(|t| values.get(&t.name));
    (
        get(&row.old).and_then(|v| v.old.as_ref()),
        get(&row.new).and_then(|v| v.new.as_ref()),
    )
}

/// Why something about a row's values is missing.
fn value_notes(row: &TensorRow, values: &HashMap<String, TensorValues>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for n in names(row) {
        if let Some(note) = values.get(&n).and_then(|v| v.note.clone())
            && !out.contains(&note)
        {
            out.push(note);
        }
    }
    out
}

fn compared(row: &TensorRow, values: &HashMap<String, TensorValues>) -> bool {
    names(row).iter().any(|n| values.contains_key(n))
}

/// Background and accent of a row by its change.
fn tint(change: TensorChange, t: &GhdTheme) -> (Hsla, Option<(Hsla, &'static str)>) {
    match change {
        TensorChange::Added => (t.diff_add_background, Some((t.color_new, "Added"))),
        TensorChange::Removed => (t.diff_delete_background, Some((t.color_deleted, "Removed"))),
        TensorChange::Changed => (
            t.color_modified.opacity(0.10),
            Some((t.color_modified, "Changed")),
        ),
        TensorChange::Renamed => (
            t.color_renamed.opacity(0.08),
            Some((t.color_renamed, "Renamed")),
        ),
        TensorChange::Unchanged => (t.background, None),
    }
}

fn cell(width: f32) -> Div {
    div()
        .flex_none()
        .w(zpx(width))
        .pr(SPACING_HALF())
        .overflow_hidden()
        .whitespace_nowrap()
}

fn text_cell(width: f32, text: impl Into<SharedString>) -> Div {
    cell(width).child(div().truncate().child(text.into()))
}

/// The pinned column header.
pub fn header(cols: &Cols, t: &GhdTheme) -> Div {
    let head = |id: &'static str, width: f32, label: &'static str, tip: Option<&'static str>| {
        let d = div()
            .id(id)
            .flex_none()
            .w(zpx(width))
            .pr(SPACING_HALF())
            .truncate()
            .child(label);
        match tip {
            Some(tip) => d.ghd_tooltip(tip).into_any_element(),
            None => d.into_any_element(),
        }
    };
    let mut cells: Vec<AnyElement> = Vec::new();
    if cols.shape {
        cells.push(head("tensor-head-shape", SHAPE, "Shape", None));
    }
    if cols.dtype {
        cells.push(head(
            "tensor-head-dtype",
            DTYPE,
            "Type",
            Some("Element type"),
        ));
    }
    if cols.size {
        cells.push(head("tensor-head-size", SIZE, "Size", Some("Stored size")));
    }
    if cols.max_diff {
        cells.push(head(
            "tensor-head-max",
            MAX_DIFF,
            "Max |Δ|",
            Some("Largest absolute difference between the versions' elements"),
        ));
    }
    if cols.rms {
        cells.push(head(
            "tensor-head-rms",
            RMS,
            "RMS Δ",
            Some("Root mean square of the differences"),
        ));
    }
    if cols.changed {
        cells.push(head(
            "tensor-head-changed",
            CHANGED,
            "Changed",
            Some("Share of elements that changed"),
        ));
    }
    if cols.cosine {
        cells.push(head(
            "tensor-head-cosine",
            COSINE,
            "Cosine",
            Some("Cosine similarity of the two versions (1 means the same direction)"),
        ));
    }
    if cols.status {
        cells.push(head("tensor-head-status", STATUS, "Status", None));
    }
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
        .child(div().flex_1().min_w_0().truncate().child("Name"))
        .children(cells)
}

/// `[a, b] → [a, c]` with the changed dimensions in bold, or the shape.
fn shape_cell(row: &TensorRow, t: &GhdTheme) -> Div {
    let (Some(old), Some(new)) = (&row.old, &row.new) else {
        return text_cell(
            SHAPE,
            model::info(row)
                .map(TensorInfo::shape_label)
                .unwrap_or_default(),
        );
    };
    if !row.shape_changed {
        return text_cell(SHAPE, new.shape_label());
    }
    let (od, nd) = (model::dims(old), model::dims(new));
    let mut parts: Vec<AnyElement> = vec![
        div()
            .flex_none()
            .text_color(t.text_secondary)
            .line_through()
            .child(old.shape_label())
            .into_any_element(),
        div().flex_none().px(zpx(3.)).child("→").into_any_element(),
        div().flex_none().child("[").into_any_element(),
    ];
    for (i, d) in nd.iter().enumerate() {
        if i > 0 {
            parts.push(div().flex_none().child(", ").into_any_element());
        }
        let changed = od.get(i) != Some(d) || od.len() != nd.len();
        parts.push(
            div()
                .flex_none()
                .when(changed, |d| d.font_weight(FontWeight::BOLD))
                .child(d.clone())
                .into_any_element(),
        );
    }
    parts.push(div().flex_none().child("]").into_any_element());
    cell(SHAPE).child(
        div()
            .id("shape")
            .flex()
            .flex_row()
            .overflow_hidden()
            .children(parts)
            .ghd_tooltip(format!("{} → {}", old.shape_label(), new.shape_label())),
    )
}

/// `old → new` with the old value struck through, or the value.
fn change_cell(width: f32, old: Option<String>, new: Option<String>, t: &GhdTheme) -> Div {
    match (old, new) {
        (Some(o), Some(n)) if o != n => cell(width).child(
            div()
                .flex()
                .flex_row()
                .overflow_hidden()
                .child(
                    div()
                        .flex_none()
                        .text_color(t.text_secondary)
                        .line_through()
                        .child(o),
                )
                .child(div().flex_none().px(zpx(3.)).child("→"))
                .child(div().flex_none().font_weight(FontWeight::BOLD).child(n)),
        ),
        (o, n) => text_cell(width, n.or(o).unwrap_or_default()),
    }
}

/// What the rows of the list need.
pub struct Rows {
    pub data: Rc<Data>,
    pub items: Rc<Vec<Item>>,
    pub values: Rc<HashMap<String, TensorValues>>,
    pub cols: Cols,
    pub view: WeakEntity<TensorDiffView>,
    pub running: bool,
}

impl Rows {
    pub fn row(&self, ix: usize, cx: &mut App) -> AnyElement {
        match self.items.get(ix) {
            Some(Item::Group {
                node,
                depth,
                collapsed,
                shown,
                value_changes,
            }) => self.group(*node, *depth, *collapsed, *shown, *value_changes, cx),
            Some(Item::Tensor {
                row,
                depth,
                label_from,
                expanded,
            }) => self.tensor(*row, *depth, *label_from, *expanded, cx),
            Some(Item::Detail { row }) => self.detail(*row, cx),
            None => div().into_any_element(),
        }
    }

    fn group(
        &self,
        node: usize,
        depth: usize,
        collapsed: bool,
        shown: usize,
        value_changes: usize,
        cx: &mut App,
    ) -> AnyElement {
        let t = cx.ghd().clone();
        let n = &self.data.tree.nodes[node];
        let c = n.counts;
        let mut chips: Vec<AnyElement> = Vec::new();
        for (count, sign, color) in [
            (c.added, "+", t.color_new),
            (c.removed, "−", t.color_deleted),
            (c.changed, "~", t.color_modified),
            (c.renamed, "→", t.color_renamed),
        ] {
            if count > 0 {
                chips.push(
                    div()
                        .flex_none()
                        .text_color(color)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!(
                            "{sign}{}",
                            crate::format::format_count(count as u64)
                        ))
                        .into_any_element(),
                );
            }
        }
        if value_changes > 0 {
            chips.push(
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(zpx(3.))
                    .text_color(t.color_modified)
                    .child(dot(t.color_modified))
                    .child(format!(
                        "{} values",
                        crate::format::format_count(value_changes as u64)
                    ))
                    .into_any_element(),
            );
        }
        let prefix = n.prefix.clone();
        let view = self.view.clone();
        let hover = t.box_hover_background;
        let summary = format!(
            "{} · {}",
            if shown == c.total() {
                format!(
                    "{} {}",
                    crate::format::format_count(shown as u64),
                    if shown == 1 { "tensor" } else { "tensors" }
                )
            } else {
                format!(
                    "{} of {} tensors",
                    crate::format::format_count(shown as u64),
                    crate::format::format_count(c.total() as u64)
                )
            },
            model::human_count(n.parameters)
        );
        div()
            .id(("tensor-group", node))
            .a11y_row(n.prefix.clone(), false)
            .aria_expanded(!collapsed)
            .w_full()
            .h(zpx(ROW))
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING_HALF())
            .pl(SPACING() + zpx(INDENT * depth as f32))
            .pr(SPACING())
            .bg(t.box_alt_background)
            .border_b_1()
            .border_color(t.diff_border)
            .cursor_pointer()
            .hover(move |d| d.bg(hover))
            .on_click(move |_, _, cx| {
                let prefix = prefix.clone();
                view.update(cx, |this, cx| this.toggle_group(prefix, cx))
                    .ok();
            })
            .child(
                octicon(
                    if collapsed {
                        Octicon::TriangleRight
                    } else {
                        Octicon::TriangleDown
                    },
                    t.text_secondary,
                )
                .flex_none(),
            )
            .child(
                div()
                    .id(("tensor-group-label", node))
                    .flex_shrink(1.)
                    .min_w(zpx(40.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis_middle()
                    .font_family(mono_font())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(n.label.clone())
                    .ghd_tooltip(n.prefix.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(summary),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .gap(zpx(6.))
                    .text_size(FONT_SIZE_SM())
                    .children(chips),
            )
            .into_any_element()
    }

    fn tensor(
        &self,
        row_ix: usize,
        depth: usize,
        label_from: usize,
        expanded: bool,
        cx: &mut App,
    ) -> AnyElement {
        let t = cx.ghd().clone();
        let row = &self.data.rows[row_ix];
        let name = row.name().to_string();
        let label = name.get(label_from..).unwrap_or(&name).to_string();
        let (bg, accent) = tint(row.change, &t);
        let diff = value_diff(row, &self.values);
        let values_changed = diff.is_some_and(|d| !d.identical);
        let tip = match (&row.old, row.change) {
            (Some(old), TensorChange::Renamed) => format!("{name}\nRenamed from {}", old.name),
            _ => name.clone(),
        };
        let cols = self.cols;
        let dtype = |t: &Option<TensorInfo>| t.as_ref().map(|t| t.dtype.name());
        let size = |t: &Option<TensorInfo>| t.as_ref().map(|t| bytes(t.bytes));

        let name_cell = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .gap(zpx(4.))
            .pl(zpx(INDENT * depth as f32))
            .child(
                octicon(
                    if expanded {
                        Octicon::ChevronDown
                    } else {
                        Octicon::ChevronRight
                    },
                    t.text_secondary,
                )
                .size(zpx(12.))
                .flex_none(),
            )
            .when(values_changed, |d| d.child(dot(t.color_modified)))
            .child(
                div()
                    .id(("tensor-name", row_ix))
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .map(|d| {
                        if label_from == 0 {
                            d.text_ellipsis_start()
                        } else {
                            d.text_ellipsis_middle()
                        }
                    })
                    .font_family(mono_font())
                    .when(row.change == TensorChange::Removed, |d| {
                        d.text_color(t.diff_delete_text)
                    })
                    .child(label)
                    .ghd_tooltip(tip),
            );

        let mut cells: Vec<AnyElement> = Vec::new();
        if cols.shape {
            cells.push(shape_cell(row, &t).into_any_element());
        }
        if cols.dtype {
            cells.push(change_cell(DTYPE, dtype(&row.old), dtype(&row.new), &t).into_any_element());
        }
        if cols.size {
            cells.push(change_cell(SIZE, size(&row.old), size(&row.new), &t).into_any_element());
        }
        if cols.values() {
            cells.push(self.value_cells(row, diff, &t).into_any_element());
        }
        if cols.status {
            let status = match accent {
                Some((color, text)) => Some((color, text)),
                None if values_changed => Some((t.color_modified, "Values")),
                None => None,
            };
            cells.push(
                cell(STATUS)
                    .text_size(FONT_SIZE_SM())
                    .children(status.map(|(color, text)| {
                        div()
                            .text_color(color)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(text)
                    }))
                    .into_any_element(),
            );
        }

        let view = self.view.clone();
        div()
            .id(("tensor-row", row_ix))
            .a11y_row(name.clone(), expanded)
            .aria_expanded(expanded)
            .w_full()
            .h(zpx(ROW))
            .flex()
            .flex_row()
            .items_center()
            .pl(SPACING())
            .pr(SPACING_HALF())
            .bg(bg)
            .text_color(t.diff_text)
            .border_b_1()
            .border_color(t.diff_border)
            .when_some(accent, |d, (color, _)| d.border_l_2().border_color(color))
            .when(accent.is_none(), |d| {
                let hover = t.diff_hover_background;
                d.hover(move |d| d.bg(hover))
            })
            .cursor_pointer()
            .on_click(move |_, _, cx| {
                view.update(cx, |this, cx| this.toggle_row(row_ix, cx)).ok();
            })
            .child(name_cell)
            .children(cells)
            .into_any_element()
    }

    /// The value columns of a tensor row.
    fn value_cells(&self, row: &TensorRow, diff: Option<&ValueDiff>, t: &GhdTheme) -> Div {
        let cols = self.cols;
        let span = |content: Div| {
            div()
                .flex_none()
                .w(cols.values_width())
                .pr(SPACING_HALF())
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(FONT_SIZE_SM())
                .text_color(t.text_secondary)
                .child(content)
        };
        if let Some(d) = diff {
            if d.identical {
                return span(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(zpx(4.))
                        .child(octicon(Octicon::Check, t.text_secondary).size(zpx(12.)))
                        .child("Identical"),
                );
            }
            let count = self
                .values
                .get(row.name())
                .and_then(|v| v.new.as_ref().or(v.old.as_ref()))
                .map_or_else(|| model::info(row).map_or(0, |t| t.elements), |s| s.count);
            let mut out = div().flex_none().flex().flex_row().font_family(mono_font());
            if cols.max_diff {
                out = out.child(text_cell(MAX_DIFF, model::value(d.max_abs_diff)));
            }
            if cols.rms {
                out = out.child(text_cell(RMS, model::value(d.rms_diff)));
            }
            if cols.changed {
                out = out.child(text_cell(CHANGED, model::percent(d.changed, count)));
            }
            if cols.cosine {
                out = out.child(text_cell(
                    COSINE,
                    d.cosine.map_or("—".to_string(), model::cosine),
                ));
            }
            return out;
        }
        let (old, new) = side_stats(row, &self.values);
        let text = if let Some(s) = new.or(old)
            && (row.old.is_none() || row.new.is_none())
        {
            format!(
                "mean {} · std {}",
                model::value(s.mean),
                model::value(s.std)
            )
        } else if let Some(note) = value_notes(row, &self.values).into_iter().next() {
            note
        } else if compared(row, &self.values) {
            "Not compared".to_string()
        } else {
            String::new()
        };
        span(div().truncate().child(text))
    }

    /// The panel under an opened tensor: layout, statistics, histograms
    /// and the largest differences.
    fn detail(&self, row_ix: usize, cx: &mut App) -> AnyElement {
        let t = cx.ghd().clone();
        let row = &self.data.rows[row_ix];
        let both = row.old.is_some() && row.new.is_some();
        let name = row.name().to_string();
        let (old_stats, new_stats) = side_stats(row, &self.values);
        let diff = value_diff(row, &self.values);
        let notes = value_notes(row, &self.values);
        let depth = self
            .items
            .iter()
            .find_map(|i| match i {
                Item::Tensor { row, depth, .. } if *row == row_ix => Some(*depth),
                _ => None,
            })
            .unwrap_or(0);

        let label_w = zpx(96.);
        let heading = |text: &'static str| {
            div()
                .text_size(FONT_SIZE_SM())
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text_secondary)
                .child(text)
        };
        // a label column, then one value per side
        let kv = |label: &'static str, old: Option<String>, new: Option<String>| {
            let changed = both && old != new;
            let value = |v: Option<String>, emphasis: bool| {
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(mono_font())
                    .when(emphasis, |d| d.font_weight(FontWeight::BOLD))
                    .child(v.unwrap_or_else(|| "—".into()))
            };
            div()
                .flex()
                .flex_row()
                .gap(SPACING())
                .child(
                    div()
                        .flex_none()
                        .w(label_w)
                        .text_color(t.text_secondary)
                        .child(label),
                )
                .map(|d| {
                    if both {
                        d.child(
                            value(old, false).when(changed, |d| d.text_color(t.diff_delete_text)),
                        )
                        .child(value(new, changed).when(changed, |d| d.text_color(t.diff_add_text)))
                    } else {
                        d.child(value(new.or(old), false))
                    }
                })
        };
        let side_heads = both.then(|| {
            div()
                .flex()
                .flex_row()
                .gap(SPACING())
                .text_size(FONT_SIZE_SM())
                .text_color(t.text_secondary)
                .child(div().flex_none().w(label_w))
                .child(div().flex_1().child("Before"))
                .child(div().flex_1().child("After"))
        });
        let info =
            |f: &dyn Fn(&TensorInfo) -> String| (row.old.as_ref().map(f), row.new.as_ref().map(f));
        let (so, sn) = info(&|t| t.shape_label());
        let (do_, dn) = info(&|t| t.dtype.name());
        let (eo, en) = info(&|t| crate::format::format_count(t.elements));
        let (bo, bn) = info(&|t| bytes(t.bytes));
        let layout = div()
            .flex()
            .flex_col()
            .gap(zpx(2.))
            .children(side_heads)
            .child(kv("Shape", so, sn))
            .child(kv("Type", do_, dn))
            .child(kv("Elements", eo, en))
            .child(kv("Size", bo, bn));

        let stats = (old_stats.is_some() || new_stats.is_some()).then(|| {
            let both_stats = both;
            let f = |s: Option<&TensorStats>, g: &dyn Fn(&TensorStats) -> String| s.map(g);
            let kv2 = |label, g: &dyn Fn(&TensorStats) -> String| {
                kv(label, f(old_stats, g), f(new_stats, g))
            };
            let count = |n: u64| crate::format::format_count(n);
            let mut hist = div().flex().flex_row().gap(SPACING_DOUBLE()).pt(zpx(4.));
            for (label, s, color) in [
                ("Before", old_stats, t.color_deleted),
                ("After", new_stats, t.color_new),
            ] {
                if let Some(s) = s
                    && !s.histogram.is_empty()
                {
                    hist = hist.child(histogram(
                        if both_stats { Some(label) } else { None },
                        s,
                        color,
                        &t,
                    ));
                }
            }
            div()
                .flex()
                .flex_col()
                .gap(zpx(2.))
                .child(heading("Values"))
                .child(kv2("Min", &|s| model::value(s.min)))
                .child(kv2("Max", &|s| model::value(s.max)))
                .child(kv2("Mean", &|s| model::value(s.mean)))
                .child(kv2("Std", &|s| model::value(s.std)))
                .child(kv2("Zeros", &|s| count(s.zeros)))
                .child(kv2("NaN", &|s| count(s.nan)))
                .child(kv2("Inf", &|s| count(s.inf)))
                .child(kv2("Read", &|s| count(s.count)))
                .child(hist)
        });

        let difference = diff.map(|d| {
            let total = new_stats
                .or(old_stats)
                .map_or_else(|| model::info(row).map_or(0, |t| t.elements), |s| s.count);
            let summary = if d.identical {
                "Every element is identical.".to_string()
            } else {
                format!(
                    "{} of {} elements changed ({}). Max |Δ| {} · mean |Δ| {} · RMS Δ {}{}",
                    crate::format::format_count(d.changed),
                    crate::format::format_count(total),
                    model::percent(d.changed, total),
                    model::value(d.max_abs_diff),
                    model::value(d.mean_abs_diff),
                    model::value(d.rms_diff),
                    d.cosine
                        .map(|c| format!(" · cosine {}", model::cosine(c)))
                        .unwrap_or_default(),
                )
            };
            let format = self
                .data
                .current
                .as_ref()
                .or(self.data.previous.as_ref())
                .map(|f| f.format);
            let shape_of = row.new.as_ref().or(row.old.as_ref());
            let fastest_first =
                format == Some(TensorFormat::Gguf) || shape_of.is_some_and(|s| s.column_major);
            let samples = (!d.samples.is_empty()).then(|| {
                let col = |text: String, w: Option<Pixels>| {
                    div()
                        .map(|d| match w {
                            Some(w) => d.flex_none().w(w),
                            None => d.flex_1().min_w_0(),
                        })
                        .truncate()
                        .child(text)
                };
                let head = div()
                    .flex()
                    .flex_row()
                    .gap(SPACING())
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(col("Element".into(), Some(zpx(140.))))
                    .child(col("Before".into(), None))
                    .child(col("After".into(), None))
                    .child(col("Δ".into(), None));
                div()
                    .flex()
                    .flex_col()
                    .gap(zpx(1.))
                    .child(heading("Largest differences"))
                    .child(head)
                    .children(d.samples.iter().map(|s| {
                        let at = shape_of
                            .and_then(|i| model::coords(&i.shape, s.index, fastest_first))
                            .map(|c| {
                                format!(
                                    "[{}]",
                                    c.iter().map(u64::to_string).collect::<Vec<_>>().join(", ")
                                )
                            })
                            .unwrap_or_else(|| format!("#{}", s.index));
                        div()
                            .flex()
                            .flex_row()
                            .gap(SPACING())
                            .font_family(mono_font())
                            .child(col(at, Some(zpx(140.))))
                            .child(col(model::value(s.old), None).text_color(t.diff_delete_text))
                            .child(col(model::value(s.new), None).text_color(t.diff_add_text))
                            .child(col(model::value(s.new - s.old), None))
                    }))
            });
            div()
                .flex()
                .flex_col()
                .gap(SPACING_HALF())
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(zpx(4.))
                        .when(d.identical, |d| {
                            d.child(octicon(Octicon::Check, t.text_secondary).size(zpx(12.)))
                        })
                        .when(!d.identical, |d| d.child(dot(t.color_modified)))
                        .child(summary),
                )
                .children(samples)
        });

        let read_any = compared(row, &self.values);
        let ask = (!read_any).then(|| {
            let view = self.view.clone();
            let names = names(row);
            let button = if self.running {
                crate::widgets::button_disabled(
                    ("tensor-compare-one", row_ix),
                    "Compare Values",
                    cx,
                )
            } else {
                crate::widgets::button(("tensor-compare-one", row_ix), "Compare Values", cx)
                    .on_click(move |_, _, cx| {
                        let names = names.clone();
                        view.update(cx, |this, cx| this.compare(names, cx)).ok();
                    })
            };
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(SPACING())
                .child(
                    div()
                        .text_color(t.text_secondary)
                        .child("Compare this tensor's values to see its statistics."),
                )
                .child(button.flex_none())
        });

        let copy_name = name.clone();
        div()
            .id(("tensor-detail", row_ix))
            .w_full()
            .pl(SPACING() + zpx(INDENT * depth as f32 + 16.))
            .pr(SPACING())
            .py(SPACING_HALF())
            .border_b_1()
            .border_color(t.diff_border)
            .bg(t.background)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACING())
                    .p(SPACING())
                    .rounded(BORDER_RADIUS())
                    .border_1()
                    .border_color(t.box_border)
                    .bg(t.box_background)
                    .text_size(FONT_SIZE_SM())
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(SPACING_HALF())
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .font_family(mono_font())
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_size(FONT_SIZE())
                                    .child(name.clone()),
                            )
                            .child(
                                div()
                                    .id(("tensor-copy-name", row_ix))
                                    .icon_button_label("Copy name")
                                    .flex_none()
                                    .size(zpx(20.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(BORDER_RADIUS())
                                    .cursor_pointer()
                                    .hover(|d| d.bg(t.box_hover_background))
                                    .child(octicon(Octicon::Copy, t.text_secondary))
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            copy_name.clone(),
                                        ))
                                    }),
                            ),
                    )
                    .when_some(
                        row.old
                            .as_ref()
                            .filter(|_| row.change == TensorChange::Renamed),
                        |d, old| {
                            d.child(
                                div()
                                    .text_color(t.text_secondary)
                                    .child(format!("Renamed from {}", old.name)),
                            )
                        },
                    )
                    .child(layout)
                    .children(stats)
                    .children(difference)
                    .children(notes.into_iter().map(|n| {
                        div()
                            .flex()
                            .flex_row()
                            .items_start()
                            .gap(zpx(4.))
                            .text_color(t.text_secondary)
                            .child(
                                div()
                                    .flex_none()
                                    .mt(zpx(1.))
                                    .child(octicon(Octicon::Info, t.text_secondary).size(zpx(12.))),
                            )
                            .child(div().flex_1().min_w_0().child(n))
                    }))
                    .children(ask),
            )
            .into_any_element()
    }
}

fn dot(color: Hsla) -> Div {
    div().flex_none().size(zpx(6.)).rounded_full().bg(color)
}

/// 16 buckets from `min` to `max` as bars, with the range under them.
fn histogram(label: Option<&'static str>, s: &TensorStats, color: Hsla, t: &GhdTheme) -> Div {
    let top = s.histogram.iter().copied().max().unwrap_or(0).max(1) as f32;
    let bars = s.histogram.iter().map(|n| {
        let share = *n as f32 / top;
        div()
            .flex_1()
            .h(relative(share))
            .when(*n > 0, |d| d.min_h(px(1.)))
            .rounded_t(zpx(1.))
            .bg(color)
    });
    div()
        .flex_none()
        .w(zpx(176.))
        .flex()
        .flex_col()
        .gap(zpx(2.))
        .children(label.map(|l| div().text_color(t.text_secondary).child(l)))
        .child(
            div()
                .h(zpx(40.))
                .flex()
                .flex_row()
                .items_end()
                .gap(px(1.))
                .border_b_1()
                .border_color(t.box_border)
                .children(bars),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .justify_between()
                .text_size(FONT_SIZE_XS())
                .text_color(t.text_secondary)
                .font_family(mono_font())
                .child(model::value(s.min))
                .child(model::value(s.max)),
        )
}
