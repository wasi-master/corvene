//! Corvene `1318-csv-table-diff` (desktop/desktop#21518): a CSV or TSV
//! file's diff as a table. The header row stays on top, every row of the
//! file is listed (added green, removed red), and a modified row shows each
//! changed cell's old value struck through above the new one. Columns the
//! change added or removed are coloured in the header. The cells scroll
//! sideways together under fixed line numbers (the `1304-diff-no-wrap`
//! scroll, [`HScroll`]).
//!
//! GitHub Desktop shows these files as a text diff only
//! (`app/src/ui/diff/text-diff.tsx`); the diff header's Text / Table switch
//! goes back to it.

use std::rc::Rc;

use corvene_core::csv_diff::{ColumnStatus, RowKind, SheetEntry, SheetStatus, TableDiff, TableRow};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::diff_view::DIFF_LINE_HEIGHT;
use crate::diff_view_rows::{HScroll, line_number_width};
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, GhdTheme};
use crate::widgets::{GhdTooltip, IconButtonA11y};

/// Cell widths are measured in characters within these bounds.
const MIN_CELL_CHARS: usize = 3;
const MAX_CELL_CHARS: usize = 40;
/// Horizontal padding inside a cell, each side (CSS px).
const CELL_PADDING: f32 = 6.;

/// A table ready to render: the diff plus each column's width.
pub struct CsvTable {
    pub diff: Rc<TableDiff>,
    pub widths: Vec<Pixels>,
    /// One line-number column (CSS px, see [`line_number_width`]).
    pub number_width: f32,
    /// `1321-table-file-diff`: rows past the read limit were left out.
    pub truncated: bool,
}

impl CsvTable {
    /// Size the columns for `column`, the advance of one character.
    pub fn new(diff: Rc<TableDiff>, column: Pixels) -> CsvTable {
        let mut chars: Vec<usize> = diff
            .columns
            .iter()
            .map(|c| c.name.chars().count())
            .collect();
        for row in &diff.rows {
            for (ix, width) in chars.iter_mut().enumerate() {
                let len = |side: &Vec<String>| side.get(ix).map_or(0, |s| s.chars().count());
                *width = (*width).max(len(&row.old)).max(len(&row.new));
            }
        }
        let widths = chars
            .into_iter()
            .map(|n| {
                column * n.clamp(MIN_CELL_CHARS, MAX_CELL_CHARS) as f32
                    + zpx(2. * CELL_PADDING)
                    + px(1.)
            })
            .collect();
        let max_line = diff
            .rows
            .iter()
            .flat_map(|r| [r.old_line, r.new_line])
            .flatten()
            .max()
            .unwrap_or(1);
        CsvTable {
            diff,
            widths,
            number_width: line_number_width(max_line),
            truncated: false,
        }
    }

    /// The cells' total width.
    pub fn width(&self) -> Pixels {
        self.widths.iter().fold(Pixels::ZERO, |a, w| a + *w)
    }
}

/// A cell's text on one line (line breaks inside quoted fields shown as ⏎).
fn one_line(text: &str) -> SharedString {
    if text.contains('\n') {
        text.replace("\r\n", "\n").replace('\n', "⏎").into()
    } else {
        SharedString::from(text.to_string())
    }
}

fn cell(width: Pixels, t: &GhdTheme) -> Div {
    div()
        .flex_none()
        .w(width)
        .h_full()
        .px(zpx(CELL_PADDING))
        .border_r_1()
        .border_color(t.diff_border)
        .flex()
        .flex_col()
        .justify_center()
        .overflow_hidden()
}

fn cell_text(text: &str) -> Div {
    div().w_full().truncate().child(one_line(text))
}

/// The line-number gutter: old and new columns.
fn gutter(
    table: &CsvTable,
    old: Option<u32>,
    new: Option<u32>,
    bg: Hsla,
    border: Hsla,
    t: &GhdTheme,
) -> Div {
    let number = |n: Option<u32>| {
        div()
            .w(zpx(table.number_width))
            .flex_none()
            .flex()
            .justify_end()
            .items_start()
            .px(SPACING_HALF())
            .child(n.map(|n| n.to_string()).unwrap_or_default())
    };
    div()
        .flex_none()
        .flex()
        .flex_row()
        .bg(bg)
        .border_r_1()
        .border_color(border)
        .text_color(t.diff_line_number)
        .child(number(old).border_r_1().border_color(border))
        .child(number(new))
}

/// The cells of a row, clipped and moved by the shared sideways scroll.
fn scrolled(h: &HScroll, cells: Vec<AnyElement>) -> Div {
    let clip = h.clone();
    div()
        .relative()
        .flex_1()
        .min_w_0()
        .h_full()
        .overflow_hidden()
        .child(
            canvas(move |b, _, _| clip.record_clip(b), |_, _, _, _| {})
                .absolute()
                .inset_0(),
        )
        .child(
            div()
                .flex_none()
                .h_full()
                .flex()
                .flex_row()
                .ml(-h.offset())
                .children(cells),
        )
}

/// The pinned header row.
pub fn header_row(table: &CsvTable, h: &HScroll, cx: &App) -> AnyElement {
    let t = cx.ghd();
    let cells: Vec<AnyElement> = table
        .diff
        .columns
        .iter()
        .zip(&table.widths)
        .enumerate()
        .map(|(ix, (column, width))| {
            let (bg, fg) = match column.status {
                ColumnStatus::Same => (t.diff_hunk_background, t.diff_text),
                ColumnStatus::Added => (t.diff_add_inner_background, t.diff_add_text),
                ColumnStatus::Deleted => (t.diff_delete_inner_background, t.diff_delete_text),
            };
            let tip = match (column.status, &column.old_name) {
                (ColumnStatus::Added, _) => Some("Column added".to_string()),
                (ColumnStatus::Deleted, _) => Some("Column removed".to_string()),
                (_, Some(old)) => Some(format!("Renamed from \u{201c}{old}\u{201d}")),
                _ => None,
            };
            let mut c = cell(*width, t)
                .id(("csv-header", ix))
                .bg(bg)
                .text_color(fg)
                .font_weight(FontWeight::BOLD)
                .child(
                    cell_text(&column.name)
                        .when(column.status == ColumnStatus::Deleted, |d| d.line_through()),
                );
            if let Some(tip) = tip {
                c = c.ghd_tooltip(tip);
            }
            c.into_any_element()
        })
        .collect();
    let (old, new) = table.diff.header_lines;
    div()
        .id("csv-header-row")
        .flex_none()
        .w_full()
        .h(DIFF_LINE_HEIGHT())
        .flex()
        .flex_row()
        .border_b_1()
        .border_color(t.diff_border)
        .bg(t.diff_hunk_background)
        .child(gutter(
            table,
            old,
            new,
            t.diff_hunk_gutter_background,
            t.diff_border,
            t,
        ))
        .child(scrolled(h, cells))
        .into_any_element()
}

/// One data row of the table.
pub fn table_row(table: &CsvTable, ix: usize, row: &TableRow, h: &HScroll, cx: &App) -> AnyElement {
    let t = cx.ghd();
    let (row_bg, row_fg, gutter_bg, gutter_border) = match row.kind {
        RowKind::Added => (
            t.diff_add_background,
            t.diff_add_text,
            t.diff_add_gutter_background,
            t.diff_add_border,
        ),
        RowKind::Deleted => (
            t.diff_delete_background,
            t.diff_delete_text,
            t.diff_delete_gutter_background,
            t.diff_delete_border,
        ),
        RowKind::Same | RowKind::Modified => (
            t.background,
            t.diff_text,
            t.diff_gutter_background,
            t.diff_border,
        ),
    };
    let modified = row.kind == RowKind::Modified;
    let height = if modified {
        DIFF_LINE_HEIGHT() * 2.
    } else {
        DIFF_LINE_HEIGHT()
    };
    let cells: Vec<AnyElement> = table
        .diff
        .columns
        .iter()
        .zip(&table.widths)
        .enumerate()
        .map(|(c, (column, width))| {
            let changed = row.changed.get(c).copied().unwrap_or(false);
            let old = row.old.get(c).map(String::as_str).unwrap_or("");
            let new = row.new.get(c).map(String::as_str).unwrap_or("");
            let base = cell(*width, t);
            if modified && changed {
                // old struck through above new; a column the change added
                // or removed has only one side
                let line = |text: &str, bg: Hsla, struck: bool| {
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .px(zpx(CELL_PADDING))
                        .mx(-zpx(CELL_PADDING))
                        .bg(bg)
                        .child(cell_text(text).when(struck, |d| d.line_through()))
                };
                let mut d = base.justify_start();
                if column.status != ColumnStatus::Added {
                    d = d.child(line(old, t.diff_delete_inner_background, true));
                }
                if column.status != ColumnStatus::Deleted {
                    d = d.child(line(new, t.diff_add_inner_background, false));
                }
                return d.into_any_element();
            }
            let text = match row.kind {
                RowKind::Deleted => old,
                _ if column.status == ColumnStatus::Deleted => old,
                _ => row.cell(c),
            };
            base.when(column.status == ColumnStatus::Deleted, |d| {
                d.text_color(t.diff_alt_text).line_through()
            })
            .child(cell_text(text))
            .into_any_element()
        })
        .collect();
    div()
        .id(("csv-row", ix))
        .w_full()
        .h(height)
        .flex_none()
        .flex()
        .flex_row()
        .bg(row_bg)
        .text_color(row_fg)
        .border_b_1()
        .border_color(t.diff_border)
        .child(gutter(
            table,
            row.old_line,
            row.new_line,
            gutter_bg,
            gutter_border,
            t,
        ))
        .child(scrolled(h, cells))
        .into_any_element()
}

/// Corvene `1321-table-file-diff`: the sheets (or database tables) of a
/// spreadsheet or database diff as tabs above the table, each marked when
/// the change added, removed or edited it.
pub fn sheet_tabs(
    sheets: &[SheetEntry],
    selected: &str,
    on_pick: impl Fn(&str, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.ghd();
    let on_pick = Rc::new(on_pick);
    let tabs = sheets.iter().enumerate().map(|(ix, sheet)| {
        let is_selected = sheet.name == selected;
        let (mark, tip) = match sheet.status {
            SheetStatus::Same => (None, None),
            SheetStatus::Added => (Some(t.color_new), Some("Added")),
            SheetStatus::Deleted => (Some(t.color_deleted), Some("Removed")),
            SheetStatus::Modified => (Some(t.color_modified), Some("Changed")),
        };
        let name = sheet.name.clone();
        let on_pick = on_pick.clone();
        let hover = t.box_hover_background;
        div()
            .id(("csv-sheet", ix))
            .a11y_button(sheet.name.clone())
            .flex_none()
            .h_full()
            .px(SPACING())
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING_HALF())
            .border_r_1()
            .border_color(t.diff_border)
            .when(sheet.status == SheetStatus::Deleted, |d| d.line_through())
            .when(is_selected, |d| {
                d.bg(t.box_selected_active_background)
                    .text_color(t.box_selected_active_text)
            })
            .when(!is_selected, |d| {
                d.cursor_pointer()
                    .text_color(t.text)
                    .hover(move |d| d.bg(hover))
                    .on_click(move |_, _, cx| on_pick(&name, cx))
            })
            .children(mark.map(|color| div().flex_none().size(zpx(6.)).rounded_full().bg(color)))
            .child(sheet.name.clone())
            .when_some(tip, |d, tip| d.ghd_tooltip(tip))
    });
    div()
        .id("csv-sheets")
        .flex_none()
        .w_full()
        .h(zpx(24.))
        .flex()
        .flex_row()
        .overflow_x_scroll()
        .border_b_1()
        .border_color(t.diff_border)
        .bg(t.diff_gutter_background)
        .text_size(FONT_SIZE_SM())
        .children(tabs)
}
