//! One cell pair: the gutter (checkbox, `In [n]` prompt), the card header
//! (kind, how it changed, hidden changes, actions) and its sections.

use corvene_core::notebook_diff::{self as nd, CellStatus, NotebookFilters};
use corvene_core::{NbCell, NbCellKind};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::Shared;
use crate::icons::{Octicon, octicon};
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, GhdTheme, mono_font};
use crate::widgets::{GhdTooltip, IconButtonA11y};

/// The gutter's width (checkbox and prompt).
pub(super) const GUTTER: f32 = 74.;

fn kind_label(kind: NbCellKind) -> &'static str {
    match kind {
        NbCellKind::Code => "Code",
        NbCellKind::Markdown => "Markdown",
        NbCellKind::Raw => "Raw",
    }
}

/// The colour of a pair's border and badge.
fn status_color(status: CellStatus, t: &GhdTheme) -> (Hsla, Option<&'static str>) {
    match status {
        CellStatus::Same => (t.box_border, None),
        CellStatus::Added => (t.diff_add_border, Some("Added")),
        CellStatus::Deleted => (t.diff_delete_border, Some("Removed")),
        CellStatus::Modified => (t.color_modified, Some("Changed")),
        CellStatus::Moved { .. } => (t.color_renamed, Some("Moved")),
        CellStatus::MovedAway { .. } => (t.box_border, None),
    }
}

/// `[n]`, or `[ ]` for a cell never run.
fn prompt(count: Option<u64>) -> String {
    count.map_or_else(|| "[ ]".into(), |n| format!("[{n}]"))
}

pub(super) fn cell_row(shared: &Shared, ix: usize, cx: &mut App) -> AnyElement {
    let t = cx.ghd().clone();
    let pair = shared.aligned.pairs[ix];
    let old = pair
        .previous
        .and_then(|i| shared.previous.as_ref()?.cells.get(i).cloned());
    let new = pair
        .current
        .and_then(|i| shared.current.as_ref()?.cells.get(i).cloned());
    if let CellStatus::MovedAway { to } = pair.status {
        return moved_away_row(to, &t);
    }
    let Some(cell) = new.as_ref().or(old.as_ref()) else {
        return div().into_any_element();
    };
    let revealed = shared.ui.borrow().revealed.contains(&ix);
    let filters = if revealed {
        NotebookFilters {
            hide_execution_counts: false,
            hide_metadata: false,
            hide_outputs: false,
            ..shared.filters
        }
    } else {
        shared.filters
    };
    let (border, badge) = status_color(pair.status, &t);
    let changed = pair.is_change(&filters);
    let hidden = pair.changes.hidden(&filters);

    // the gutter: checkbox and prompt
    let check = shared.checkbox(ix);
    let weak = shared.weak.clone();
    let counts_shown = !filters.hide_execution_counts && pair.changes.execution_count;
    let prompt_text = match (cell.kind, &old, &new) {
        (NbCellKind::Code, Some(a), Some(b)) if counts_shown => {
            format!(
                "{}→{}",
                prompt(a.execution_count),
                prompt(b.execution_count)
            )
        }
        (NbCellKind::Code, _, _) => prompt(cell.execution_count),
        _ => String::new(),
    };
    let gutter = div()
        .flex_none()
        .w(zpx(GUTTER))
        .pt(zpx(5.))
        .pr(SPACING_HALF())
        .flex()
        .flex_row()
        .justify_end()
        .items_start()
        .gap(zpx(4.))
        .children(check.map(|state| {
            let weak = weak.clone();
            crate::widgets::checkbox_tristate(("notebook-cell-check", ix), state, false, cx)
                .ghd_tooltip("Include this cell's change in the commit")
                .on_click(move |_, _, cx| {
                    weak.update(cx, |this, cx| this.toggle_pair(ix, cx)).ok();
                })
        }))
        .child(
            div()
                .font_family(mono_font())
                .text_size(FONT_SIZE_SM())
                .text_color(if counts_shown {
                    t.color_modified
                } else {
                    t.text_secondary
                })
                .child(prompt_text),
        );

    // the card's header
    let mut tags: Vec<AnyElement> = Vec::new();
    tags.push(
        div()
            .text_color(t.text_secondary)
            .child(match (&old, &new) {
                (Some(a), Some(b)) if a.kind != b.kind => {
                    format!("{} → {}", kind_label(a.kind), kind_label(b.kind))
                }
                _ => kind_label(cell.kind).to_string(),
            })
            .into_any_element(),
    );
    if let Some(label) = badge.filter(|_| changed || pair.status != CellStatus::Modified) {
        tags.push(
            div()
                .px(zpx(5.))
                .rounded(zpx(8.))
                .border_1()
                .border_color(border)
                .text_color(border)
                .child(label)
                .into_any_element(),
        );
    }
    if let CellStatus::Moved { from } = pair.status {
        tags.push(
            div()
                .text_color(t.text_secondary)
                .child(format!("from cell {}", from + 1))
                .into_any_element(),
        );
    }
    if !hidden.is_empty() {
        let names: Vec<&str> = hidden.iter().map(|h| h.label()).collect();
        let weak = weak.clone();
        let hover = t.box_hover_background;
        tags.push(
            div()
                .id(("notebook-hidden", ix))
                .a11y_button("Show hidden changes")
                .flex()
                .flex_row()
                .items_center()
                .gap(zpx(3.))
                .px(zpx(4.))
                .rounded(BORDER_RADIUS())
                .cursor_pointer()
                .text_color(t.text_secondary)
                .hover(move |s| s.bg(hover))
                .child(div().size(zpx(6.)).rounded_full().bg(t.text_secondary))
                .child(format!("{} changed (hidden)", names.join(", ")))
                .ghd_tooltip("Show these changes for this cell")
                .on_click(move |_, _, cx| {
                    weak.update(cx, |this, cx| this.reveal(ix, cx)).ok();
                })
                .into_any_element(),
        );
    }
    let markdown = cell.kind == NbCellKind::Markdown;
    let md_source = shared.ui.borrow().md_source.contains(&ix);
    let action = |id: (&'static str, usize), icon: Octicon, label: &'static str| {
        let hover = t.box_hover_background;
        div()
            .id(id)
            .a11y_button(label)
            .flex_none()
            .p(zpx(2.))
            .rounded(BORDER_RADIUS())
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .child(octicon(icon, t.text_secondary))
            .ghd_tooltip(label)
    };
    let mut actions: Vec<AnyElement> = Vec::new();
    if markdown {
        let weak = weak.clone();
        actions.push(
            action(
                ("notebook-md-source", ix),
                if md_source {
                    Octicon::Eye
                } else {
                    Octicon::FileCode
                },
                if md_source {
                    "Show Rendered Markdown"
                } else {
                    "Show Markdown Source"
                },
            )
            .on_click(move |_, _, cx| {
                weak.update(cx, |this, cx| this.toggle_md_source(ix, cx))
                    .ok();
            })
            .into_any_element(),
        );
    }
    let copy = cell.source.clone();
    actions.push(
        action(("notebook-copy", ix), Octicon::Copy, "Copy Cell Source")
            .on_click(move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
            })
            .into_any_element(),
    );
    if shared.can_discard() && check.is_some() {
        let weak = weak.clone();
        actions.push(
            action(
                ("notebook-discard", ix),
                Octicon::Trash,
                "Discard Cell Changes…",
            )
            .on_click(move |_, _, cx| {
                weak.update(cx, |this, cx| this.discard_pair(ix, cx)).ok();
            })
            .into_any_element(),
        );
    }
    let header = div()
        .w_full()
        .px(SPACING_HALF())
        .py(zpx(2.))
        .flex()
        .flex_row()
        .items_center()
        .gap(SPACING_HALF())
        .text_size(FONT_SIZE_XS())
        .border_b_1()
        .border_color(t.diff_border)
        .bg(t.diff_gutter_background)
        .children(tags)
        .child(div().flex_1())
        .children(actions);

    // the sections
    let mut sections: Vec<AnyElement> = Vec::new();
    sections.push(super::source::cell_source(
        shared,
        ix,
        old.as_ref(),
        new.as_ref(),
        md_source,
        pair.status,
        cx,
    ));
    let has_outputs = [&old, &new]
        .into_iter()
        .flatten()
        .any(|c| !c.outputs.is_empty());
    if has_outputs && !filters.hide_outputs {
        sections.push(super::outputs::cell_outputs(
            shared,
            ix,
            old.as_ref(),
            new.as_ref(),
            pair.status,
            cx,
        ));
    }
    if pair.changes.metadata && !filters.hide_metadata {
        let text = |c: &Option<NbCell>| {
            c.as_ref()
                .map(|c| nd::metadata_text(&c.metadata))
                .unwrap_or_default()
        };
        sections.push(
            div()
                .w_full()
                .border_t_1()
                .border_color(t.diff_border)
                .child(
                    div()
                        .px(SPACING_HALF())
                        .pt(zpx(2.))
                        .text_size(FONT_SIZE_XS())
                        .text_color(t.text_secondary)
                        .child("Metadata"),
                )
                .child(super::source::text_diff_element(
                    shared,
                    ix,
                    8,
                    &text(&old),
                    &text(&new),
                    "json",
                    cx,
                ))
                .into_any_element(),
        );
    }
    let bar = match pair.status {
        CellStatus::Same => gpui_kit::transparent_black(),
        _ if !changed => gpui_kit::transparent_black(),
        _ => border,
    };
    div()
        .w_full()
        .pr(SPACING())
        .py(zpx(3.))
        .flex()
        .flex_row()
        .child(gutter)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_row()
                .rounded(zpx(6.))
                .border_1()
                .border_color(if changed { border } else { t.box_border })
                .overflow_hidden()
                .bg(t.background)
                .when(pair.status == CellStatus::Same || !changed, |d| {
                    d.opacity(0.92)
                })
                .child(div().flex_none().w(zpx(3.)).bg(bar))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(header)
                        .children(sections),
                ),
        )
        .into_any_element()
}

fn moved_away_row(to: usize, t: &GhdTheme) -> AnyElement {
    div()
        .w_full()
        .pr(SPACING())
        .py(zpx(1.))
        .flex()
        .flex_row()
        .child(div().flex_none().w(zpx(GUTTER)))
        .child(
            div()
                .flex_1()
                .h(zpx(20.))
                .px(SPACING_HALF())
                .flex()
                .flex_row()
                .items_center()
                .gap(SPACING_HALF())
                .rounded(zpx(6.))
                .border_1()
                .border_dashed()
                .border_color(t.color_renamed)
                .text_size(FONT_SIZE_XS())
                .text_color(t.text_secondary)
                .child(octicon(Octicon::ArrowDown, t.color_renamed))
                .child(format!("A cell moved from here to cell {}", to + 1)),
        )
        .into_any_element()
}
