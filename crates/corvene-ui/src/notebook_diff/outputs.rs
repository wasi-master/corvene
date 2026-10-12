//! A code cell's outputs: each drawn as its richest MIME type (chips pick
//! another), and a changed output compared the way its kind reads best:
//! text as a line diff, images in the image diff, tables cell by cell,
//! formulas and HTML old over new (or side by side).

use std::cell::RefCell;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;

use corvene_core::csv_diff::{ColumnStatus, RowKind, TableDiff};
use corvene_core::notebook_diff::CellStatus;
use corvene_core::notebook_diff::outputs::{
    OutputStatus, Render, Rendering, decode_base64, output_pairs, renderings, table_diff,
};
use corvene_core::{FileStatusKind, ImageBlob, NbCell, NbOutput, NbOutputKind};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{Shared, UiState};
use crate::icons::{Octicon, octicon};
use crate::markdown::{MarkdownOptions, markdown_with, parsed_math};
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, GhdTheme, mono_font};
use crate::widgets::{GhdTooltip, IconButtonA11y};

/// Lines of an output drawn before "Show all".
const CLIPPED_LINES: usize = 60;
/// Table rows drawn.
const MAX_TABLE_ROWS: usize = 300;
/// The tallest an output image is drawn.
const MAX_IMAGE_HEIGHT: f32 = 420.;
/// Terminal width for ANSI text (no wrapping inside the buffer).
const ANSI_COLS: usize = 1000;

fn hash_of(value: impl Hash) -> u64 {
    let mut h = DefaultHasher::new();
    value.hash(&mut h);
    h.finish()
}

/// A cache key for a payload held by a notebook blob: its address, length
/// and ends (hashing megabytes of base64 every frame would cost more).
pub(super) fn payload_key(kind: &str, bytes: &[u8]) -> u64 {
    let n = bytes.len();
    let head = &bytes[..n.min(64)];
    let tail = &bytes[n.saturating_sub(64)..];
    hash_of((kind, bytes.as_ptr() as usize, n, head, tail))
}

/// A base64 image payload as a GPUI image, cached.
pub(super) fn decode_image(
    ui: &Rc<RefCell<UiState>>,
    mime: &str,
    base64: &str,
) -> Option<Arc<Image>> {
    let key = payload_key(mime, base64.as_bytes());
    if let Some(hit) = ui.borrow().images.get(&key) {
        return hit.clone();
    }
    let format = match mime {
        "image/png" => Some(ImageFormat::Png),
        "image/jpeg" | "image/jpg" => Some(ImageFormat::Jpeg),
        "image/gif" => Some(ImageFormat::Gif),
        "image/webp" => Some(ImageFormat::Webp),
        "image/bmp" => Some(ImageFormat::Bmp),
        "image/svg+xml" => Some(ImageFormat::Svg),
        _ => None,
    };
    let bytes = if mime == "image/svg+xml" {
        // an SVG output is its markup, not base64
        Some(base64.as_bytes().to_vec())
    } else {
        decode_base64(base64)
    };
    let image = format
        .zip(bytes)
        .map(|(f, b)| Arc::new(Image::from_bytes(f, b)));
    ui.borrow_mut().images.insert(key, image.clone());
    image
}

/// An output's label: `Out[3]`, `stderr`, `Error`.
fn label(output: &NbOutput) -> String {
    match output.kind {
        NbOutputKind::Stream => output.name.clone(),
        NbOutputKind::Error => format!("{}: {}", output.name, output.evalue),
        NbOutputKind::ExecuteResult => output
            .execution_count
            .map_or_else(|| "Out".into(), |n| format!("Out[{n}]")),
        NbOutputKind::DisplayData => "Display".into(),
    }
}

pub(super) fn cell_outputs(
    shared: &Shared,
    ix: usize,
    old: Option<&NbCell>,
    new: Option<&NbCell>,
    status: CellStatus,
    cx: &mut App,
) -> AnyElement {
    let t = cx.ghd().clone();
    let empty: &[NbOutput] = &[];
    let old_outputs = match status {
        CellStatus::Added => empty,
        _ => old.map_or(empty, |c| c.outputs.as_slice()),
    };
    let new_outputs = match status {
        CellStatus::Deleted => empty,
        _ => new.map_or(empty, |c| c.outputs.as_slice()),
    };
    let pairs = output_pairs(old_outputs, new_outputs);
    let mut items: Vec<AnyElement> = Vec::new();
    for (k, pair) in pairs.iter().enumerate() {
        let old_out = pair.previous.and_then(|i| old_outputs.get(i));
        let new_out = pair.current.and_then(|i| new_outputs.get(i));
        let Some(shown) = new_out.or(old_out) else {
            continue;
        };
        let new_r = new_out.map(renderings).unwrap_or_default();
        let old_r = old_out.map(renderings).unwrap_or_default();
        let list = if new_r.is_empty() { &old_r } else { &new_r };
        let choice = shared
            .ui
            .borrow()
            .output_choice
            .get(&(ix, k))
            .copied()
            .unwrap_or(0)
            .min(list.len().saturating_sub(1));
        let mime = list.get(choice).map(|r| r.mime.clone()).unwrap_or_default();
        let pick = |rs: &[Rendering]| -> Option<Rendering> {
            rs.iter()
                .find(|r| r.mime == mime)
                .or_else(|| rs.first())
                .cloned()
        };
        let (old_pick, new_pick) = (pick(&old_r), pick(&new_r));

        // the output's label and its MIME chips
        let weak = shared.weak.clone();
        let chips = (list.len() > 1).then(|| {
            div()
                .flex()
                .flex_row()
                .gap(zpx(3.))
                .children(list.iter().enumerate().map(|(i, r)| {
                    let selected = i == choice;
                    let weak = weak.clone();
                    let hover = t.box_hover_background;
                    div()
                        .id(SharedString::from(format!("nb-mime-{ix}-{k}-{i}")))
                        .a11y_button(r.mime.clone())
                        .px(zpx(4.))
                        .rounded(zpx(8.))
                        .border_1()
                        .border_color(if selected {
                            t.box_border_accent
                        } else {
                            t.box_border
                        })
                        .when(selected, |d| {
                            d.bg(t.box_selected_active_background)
                                .text_color(t.box_selected_active_text)
                        })
                        .when(!selected, |d| {
                            d.cursor_pointer().hover(move |s| s.bg(hover)).on_click(
                                move |_, _, cx| {
                                    weak.update(cx, |this, cx| this.pick_output((ix, k), i, cx))
                                        .ok();
                                },
                            )
                        })
                        .child(r.mime.clone())
                }))
        });
        let status_label = match pair.status {
            OutputStatus::Same => None,
            OutputStatus::Added => Some(("added", t.diff_add_border)),
            OutputStatus::Deleted => Some(("removed", t.diff_delete_border)),
            OutputStatus::Modified => Some(("changed", t.color_modified)),
        };
        let head = div()
            .w_full()
            .px(SPACING_HALF())
            .pt(zpx(3.))
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING_HALF())
            .text_size(FONT_SIZE_XS())
            .text_color(t.text_secondary)
            .child(label(shown))
            .children(status_label.map(|(l, c)| div().text_color(c).child(l)))
            .child(div().flex_1())
            .children(chips);

        let full = shared.ui.borrow().full_output.contains(&(ix, k));
        let draw = |r: &Rendering, cx: &mut App| render_one(shared, ix, k, r, full, cx);
        let body: AnyElement = match (pair.status, &old_pick, &new_pick) {
            (OutputStatus::Same, _, Some(r)) | (OutputStatus::Added, _, Some(r)) => {
                let el = draw(r, cx);
                if pair.status == OutputStatus::Added {
                    tinted(el, true, &t)
                } else {
                    el
                }
            }
            (OutputStatus::Deleted, Some(r), _) | (OutputStatus::Same, Some(r), None) => {
                tinted(draw(r, cx), false, &t)
            }
            (OutputStatus::Modified, Some(a), Some(b)) => compare(shared, ix, k, a, b, full, cx),
            _ => div().into_any_element(),
        };
        items.push(div().w_full().child(head).child(body).into_any_element());
    }
    div()
        .w_full()
        .pb(zpx(3.))
        .border_t_1()
        .border_color(t.diff_border)
        .flex()
        .flex_col()
        .gap(zpx(2.))
        .children(items)
        .into_any_element()
}

/// A drawn output in an added or removed tint.
fn tinted(el: AnyElement, added: bool, t: &GhdTheme) -> AnyElement {
    let (bar, bg) = if added {
        (t.diff_add_border, t.diff_add_background)
    } else {
        (t.diff_delete_border, t.diff_delete_background)
    };
    div()
        .w_full()
        .flex()
        .flex_row()
        .bg(bg)
        .child(div().flex_none().w(zpx(3.)).bg(bar))
        .child(div().flex_1().min_w_0().child(el))
        .into_any_element()
}

/// Old and new drawn one over the other (side by side in split mode).
fn old_new(old: AnyElement, new: AnyElement, split: bool, t: &GhdTheme) -> AnyElement {
    let (old, new) = (tinted(old, false, t), tinted(new, true, t));
    if split {
        div()
            .w_full()
            .flex()
            .flex_row()
            .child(div().flex_1().min_w_0().child(old))
            .child(div().flex_1().min_w_0().child(new))
            .into_any_element()
    } else {
        div()
            .w_full()
            .flex()
            .flex_col()
            .child(old)
            .child(new)
            .into_any_element()
    }
}

fn has_escapes(text: &str) -> bool {
    text.contains('\x1b') || text.contains('\r')
}

fn compare(
    shared: &Shared,
    ix: usize,
    k: usize,
    a: &Rendering,
    b: &Rendering,
    full: bool,
    cx: &mut App,
) -> AnyElement {
    let t = cx.ghd().clone();
    let part = 16u8.saturating_add(k.min(200) as u8);
    let text_diff = |x: &str, y: &str, ext: &str, cx: &mut App| {
        super::source::text_diff_element(shared, ix, part, x, y, ext, cx)
    };
    match (&a.render, &b.render) {
        (Render::Ansi { text: x, .. }, Render::Ansi { text: y, .. })
            if !has_escapes(x) && !has_escapes(y) =>
        {
            text_diff(x, y, "txt", cx)
        }
        (Render::Text(x), Render::Text(y)) => text_diff(x, y, "txt", cx),
        (Render::Json(x), Render::Json(y)) => text_diff(x, y, "json", cx),
        (Render::Html(x), Render::Html(y)) if x.len() + y.len() < 64 * 1024 => {
            let rendered = old_new(
                html(shared, ix, k, 0, x, cx),
                html(shared, ix, k, 1, y, cx),
                shared.split,
                &t,
            );
            rendered
        }
        (
            Render::Image {
                mime: ma,
                base64: x,
            },
            Render::Image {
                mime: mb,
                base64: y,
            },
        ) => image_pair(shared, ix, k, (ma, x), (mb, y), cx),
        (Render::Svg(x), Render::Svg(y)) => image_pair(
            shared,
            ix,
            k,
            ("image/svg+xml", x),
            ("image/svg+xml", y),
            cx,
        ),
        (Render::Table(x), Render::Table(y)) => match table_diff(Some(x), Some(y)) {
            Some(diff) => table_view(&diff, &t),
            None => old_new(simple_table(x, &t), simple_table(y, &t), shared.split, &t),
        },
        _ => {
            let old = render_one(shared, ix, k, a, full, cx);
            let new = render_one(shared, ix, k, b, full, cx);
            old_new(old, new, shared.split, &t)
        }
    }
}

/// Two images in the image diff (2-up, swipe, onion skin, difference).
fn image_pair(
    shared: &Shared,
    ix: usize,
    k: usize,
    old: (&str, &str),
    new: (&str, &str),
    cx: &mut App,
) -> AnyElement {
    let blob = |(mime, data): (&str, &str)| -> Option<ImageBlob> {
        let bytes = if mime == "image/svg+xml" {
            data.as_bytes().to_vec()
        } else {
            decode_base64(data)?
        };
        Some(ImageBlob {
            bytes,
            media_type: if mime == "image/jpeg" {
                "image/jpg".into()
            } else {
                mime.to_string()
            },
            original: None,
        })
    };
    let existing = shared.ui.borrow().image_diffs.get(&(ix, k)).cloned();
    let view = match existing {
        Some(v) => v,
        None => {
            let (a, b) = (blob(old), blob(new));
            let v = cx.new(|cx| {
                crate::image_diff::ImageDiff::new(
                    a.as_ref(),
                    b.as_ref(),
                    FileStatusKind::Modified,
                    cx,
                )
            });
            shared
                .ui
                .borrow_mut()
                .image_diffs
                .insert((ix, k), v.clone());
            v
        }
    };
    div()
        .w_full()
        .h(zpx(MAX_IMAGE_HEIGHT))
        .flex()
        .flex_col()
        .child(view)
        .into_any_element()
}

fn html(shared: &Shared, ix: usize, k: usize, side: u8, text: &str, cx: &App) -> AnyElement {
    let blocks = corvene_core::markdown::parse(text);
    markdown_with(
        SharedString::from(format!("nb-html-{ix}-{k}-{side}")),
        &blocks,
        &MarkdownOptions {
            macros: Some(shared.macros.clone()),
            highlight_code: true,
            ..MarkdownOptions::default()
        },
        cx,
    )
    .px(SPACING())
    .py(zpx(3.))
    .into_any_element()
}

/// Lines of plain or ANSI text, clipped unless `full`.
fn text_lines(
    shared: &Shared,
    ix: usize,
    k: usize,
    text: &str,
    full: bool,
    stderr: bool,
    cx: &App,
) -> AnyElement {
    let t = cx.ghd();
    let styled: Vec<(String, Vec<(std::ops::Range<usize>, HighlightStyle)>)> = if has_escapes(text)
    {
        crate::terminal::ansi_lines(text, ANSI_COLS)
    } else {
        corvene_core::notebook_diff::text::lines(text)
            .into_iter()
            .map(|l| (l.to_string(), Vec::new()))
            .collect()
    };
    let total = styled.len();
    let shown = if full {
        total
    } else {
        total.min(CLIPPED_LINES)
    };
    let weak = shared.weak.clone();
    let hover = t.box_hover_background;
    div()
        .w_full()
        .px(SPACING())
        .py(zpx(2.))
        .when(stderr, |d| d.bg(t.diff_delete_background))
        .font_family(mono_font())
        .text_size(FONT_SIZE_SM())
        .line_height(relative(1.4))
        .children(styled.into_iter().take(shown).map(|(line, runs)| {
            let runs: Vec<_> = runs
                .into_iter()
                .filter(|(r, _)| r.end <= line.len())
                .collect();
            div()
                .min_h(zpx(14.))
                .child(StyledText::new(SharedString::from(line)).with_highlights(runs))
        }))
        .when(shown < total, |d| {
            d.child(
                div()
                    .id(SharedString::from(format!("nb-more-{ix}-{k}")))
                    .a11y_button("Show the whole output")
                    .mt(zpx(2.))
                    .text_color(t.link)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .child(format!("Show {} more lines", total - shown))
                    .on_click(move |_, _, cx| {
                        weak.update(cx, |this, cx| this.show_full_output((ix, k), cx))
                            .ok();
                    }),
            )
        })
        .into_any_element()
}

fn render_one(
    shared: &Shared,
    ix: usize,
    k: usize,
    r: &Rendering,
    full: bool,
    cx: &mut App,
) -> AnyElement {
    let t = cx.ghd().clone();
    match &r.render {
        Render::Ansi { text, stderr } => text_lines(shared, ix, k, text, full, *stderr, cx),
        Render::Text(text) => text_lines(shared, ix, k, text, full, false, cx),
        Render::Json(text) => {
            super::source::text_diff_element(shared, ix, 200, text, text, "json", cx)
        }
        Render::Image { mime, base64 } => match decode_image(&shared.ui, mime, base64) {
            Some(image) => div()
                .px(SPACING())
                .py(zpx(3.))
                .child(
                    img(image)
                        .max_w_full()
                        .max_h(zpx(MAX_IMAGE_HEIGHT))
                        .object_fit(ObjectFit::ScaleDown),
                )
                .into_any_element(),
            None => placeholder("Image that could not be read", mime, &t),
        },
        Render::Svg(svg) => match decode_image(&shared.ui, "image/svg+xml", svg) {
            Some(image) => div()
                .px(SPACING())
                .py(zpx(3.))
                .child(
                    img(image)
                        .max_w_full()
                        .max_h(zpx(MAX_IMAGE_HEIGHT))
                        .object_fit(ObjectFit::ScaleDown),
                )
                .into_any_element(),
            None => placeholder("Drawing that could not be read", "image/svg+xml", &t),
        },
        Render::Table(rows) => simple_table(rows, &t),
        Render::Html(text) => html(shared, ix, k, 2, text, cx),
        Render::Markdown(text) => markdown_with(
            SharedString::from(format!("nb-out-md-{ix}-{k}")),
            &parsed_math(text),
            &MarkdownOptions {
                macros: Some(shared.macros.clone()),
                highlight_code: true,
                ..MarkdownOptions::default()
            },
            cx,
        )
        .px(SPACING())
        .py(zpx(3.))
        .into_any_element(),
        Render::Latex(tex) => div()
            .px(SPACING())
            .child(crate::math::block(
                SharedString::from(format!("nb-tex-{ix}-{k}-{}", hash_of(tex))),
                tex,
                None,
                &shared.macros,
                zpx(12.),
                cx,
            ))
            .into_any_element(),
        Render::Pdf(data) => placeholder(
            &format!("PDF output, {} KB", data.len() * 3 / 4 / 1024),
            "application/pdf",
            &t,
        ),
        Render::Placeholder { mime, what } => placeholder(what, mime, &t),
    }
}

fn placeholder(what: &str, mime: &str, t: &GhdTheme) -> AnyElement {
    div()
        .mx(SPACING())
        .my(zpx(3.))
        .px(SPACING_HALF())
        .py(zpx(3.))
        .flex()
        .flex_row()
        .items_center()
        .gap(zpx(4.))
        .rounded(BORDER_RADIUS())
        .border_1()
        .border_color(t.box_border)
        .text_size(FONT_SIZE_SM())
        .text_color(t.text_secondary)
        .child(octicon(Octicon::Info, t.text_secondary))
        .child(format!("{what} ({mime}), not shown"))
        .into_any_element()
}

fn cell_box(text: String, header: bool, t: &GhdTheme) -> Div {
    div()
        .flex_1()
        .min_w(zpx(60.))
        .px(zpx(6.))
        .py(zpx(1.))
        .border_l_1()
        .border_color(t.diff_border)
        .when(header, |d| d.font_weight(FontWeight::SEMIBOLD))
        .truncate()
        .child(text)
}

/// A table as a grid, the first row as its header.
fn simple_table(rows: &[Vec<String>], t: &GhdTheme) -> AnyElement {
    div()
        .id("nb-table")
        .mx(SPACING())
        .my(zpx(3.))
        .max_w_full()
        .overflow_x_scroll()
        .border_1()
        .border_color(t.diff_border)
        .text_size(FONT_SIZE_SM())
        .flex()
        .flex_col()
        .children(
            rows.iter()
                .take(MAX_TABLE_ROWS + 1)
                .enumerate()
                .map(|(r, row)| {
                    div()
                        .flex()
                        .flex_row()
                        .when(r > 0, |d| d.border_t_1().border_color(t.diff_border))
                        .when(r == 0, |d| d.bg(t.diff_gutter_background))
                        .children(row.iter().map(|c| cell_box(c.clone(), r == 0, t)))
                }),
        )
        .when(rows.len() > MAX_TABLE_ROWS + 1, |d| {
            d.child(
                div()
                    .px(SPACING_HALF())
                    .text_color(t.text_secondary)
                    .child(format!("{} more rows", rows.len() - MAX_TABLE_ROWS - 1)),
            )
        })
        .into_any_element()
}

/// A table diff: added rows green, removed red, changed cells marked with
/// their old value struck through.
fn table_view(diff: &TableDiff, t: &GhdTheme) -> AnyElement {
    let header = div()
        .flex()
        .flex_row()
        .bg(t.diff_gutter_background)
        .children(diff.columns.iter().map(|c| {
            let name = match &c.old_name {
                Some(old) => format!("{old} → {}", c.name),
                None => c.name.clone(),
            };
            cell_box(name, true, t).map(|d| match c.status {
                ColumnStatus::Added => d.bg(t.diff_add_inner_background),
                ColumnStatus::Deleted => d.bg(t.diff_delete_inner_background).line_through(),
                ColumnStatus::Same => d,
            })
        }));
    let changed = diff.changed_rows();
    let rows = diff.rows.iter().take(MAX_TABLE_ROWS).map(|row| {
        let bg = match row.kind {
            RowKind::Added => Some(t.diff_add_background),
            RowKind::Deleted => Some(t.diff_delete_background),
            _ => None,
        };
        div()
            .flex()
            .flex_row()
            .border_t_1()
            .border_color(t.diff_border)
            .when_some(bg, |d, bg| d.bg(bg))
            .children((0..diff.columns.len()).map(|c| {
                let is_changed =
                    row.kind == RowKind::Modified && row.changed.get(c).copied().unwrap_or(false);
                if is_changed {
                    let old = row.old.get(c).cloned().unwrap_or_default();
                    let new = row.new.get(c).cloned().unwrap_or_default();
                    div()
                        .flex_1()
                        .min_w(zpx(60.))
                        .px(zpx(6.))
                        .py(zpx(1.))
                        .border_l_1()
                        .border_color(t.diff_border)
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .bg(t.diff_delete_inner_background)
                                .line_through()
                                .text_color(t.text_secondary)
                                .truncate()
                                .child(old),
                        )
                        .child(div().bg(t.diff_add_inner_background).truncate().child(new))
                } else {
                    cell_box(row.cell(c).to_string(), false, t)
                }
            }))
    });
    div()
        .id("nb-table-diff")
        .mx(SPACING())
        .my(zpx(3.))
        .max_w_full()
        .overflow_x_scroll()
        .border_1()
        .border_color(t.diff_border)
        .text_size(FONT_SIZE_SM())
        .flex()
        .flex_col()
        .child(header)
        .children(rows)
        .child(
            div()
                .px(SPACING_HALF())
                .border_t_1()
                .border_color(t.diff_border)
                .text_size(FONT_SIZE_XS())
                .text_color(t.text_secondary)
                .child(format!(
                    "{changed} of {} rows changed{}",
                    diff.rows.len(),
                    if diff.rows.len() > MAX_TABLE_ROWS {
                        format!(", first {MAX_TABLE_ROWS} shown")
                    } else {
                        String::new()
                    }
                )),
        )
        .ghd_tooltip("Table output compared cell by cell")
        .into_any_element()
}
