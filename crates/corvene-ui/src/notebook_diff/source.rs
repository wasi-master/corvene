//! A cell's source: code (and markdown shown as source) as a line diff with
//! syntax colours and changed words marked; markdown rendered block by
//! block with changed blocks tinted. Also the text diffs of metadata and
//! text outputs.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use corvene_core::block_diff::{BlockStatus, block_diff};
use corvene_core::notebook_diff::CellStatus;
use corvene_core::notebook_diff::text::{
    LineKind, Segment, TextLine, line_diff, segments, split_rows, whole,
};
use corvene_core::{NbCell, NbCellKind, NbData};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::Shared;
use crate::icons::{Octicon, octicon};
use crate::markdown::{ImageResolver, MarkdownOptions, markdown_with, parsed_math};
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, GhdTheme, mono_font};
use crate::widgets::IconButtonA11y;

/// Unchanged lines kept next to a change.
const CONTEXT: usize = 3;
/// Lines drawn per text before "Show N more".
pub(super) const MAX_LINES: usize = 400;

fn hash_of(value: impl Hash) -> u64 {
    let mut h = DefaultHasher::new();
    value.hash(&mut h);
    h.finish()
}

pub(super) fn cell_source(
    shared: &Shared,
    ix: usize,
    old: Option<&NbCell>,
    new: Option<&NbCell>,
    md_source: bool,
    status: CellStatus,
    cx: &mut App,
) -> AnyElement {
    let t = cx.ghd().clone();
    let Some(cell) = new.or(old) else {
        return div().into_any_element();
    };
    let old_src = old.map_or("", |c| c.source.as_str());
    let new_src = new.map_or("", |c| c.source.as_str());
    if old_src.trim().is_empty() && new_src.trim().is_empty() {
        return div()
            .px(SPACING())
            .py(zpx(3.))
            .text_size(FONT_SIZE_SM())
            .text_color(t.text_secondary)
            .child("Empty cell")
            .into_any_element();
    }
    let (old_src, new_src) = match status {
        CellStatus::Added => ("", new_src),
        CellStatus::Deleted => (old_src, ""),
        _ => (old_src, new_src),
    };
    match cell.kind {
        NbCellKind::Markdown if !md_source => {
            rendered_markdown(shared, ix, old, new, old_src, new_src, status, cx)
        }
        NbCellKind::Markdown => text_diff_element(shared, ix, 0, old_src, new_src, "md", cx),
        NbCellKind::Raw => text_diff_element(shared, ix, 0, old_src, new_src, "txt", cx),
        NbCellKind::Code => {
            let ext = shared.ext.trim_start_matches('.').to_string();
            text_diff_element(shared, ix, 0, old_src, new_src, &ext, cx)
        }
    }
}

/// Highlight spans of a whole text, cached.
fn spans(shared: &Shared, ext: &str, text: &str) -> Option<Rc<Vec<Vec<corvene_highlight::Span>>>> {
    if text.is_empty() || text.len() > 256 * 1024 {
        return None;
    }
    let key = hash_of((ext, text));
    if let Some(hit) = shared.ui.borrow().highlights.get(&key) {
        return Some(hit.clone());
    }
    let lines = corvene_core::notebook_diff::text::lines(text);
    let spans = corvene_highlight::highlight_lines(&format!("cell.{ext}"), lines.iter().copied())?;
    let spans = Rc::new(spans);
    shared.ui.borrow_mut().highlights.insert(key, spans.clone());
    Some(spans)
}

/// A line as styled text: syntax colours, then the changed words.
fn styled_line(
    line: &TextLine,
    spans: Option<&[corvene_highlight::Span]>,
    t: &GhdTheme,
) -> StyledText {
    let text = line.text.replace('\t', "    ");
    // tabs widen the text: keep ranges valid by only colouring tab-free lines
    let tabs = text.len() != line.text.len();
    let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    if !tabs && let Some(spans) = spans {
        for s in spans {
            if s.range.end <= text.len() {
                highlights.push((
                    s.range.clone(),
                    HighlightStyle {
                        color: Some(crate::diff_view_rows::token_color(s.class, t)),
                        ..HighlightStyle::default()
                    },
                ));
            }
        }
    }
    if !tabs && !line.marks.is_empty() {
        let bg = match line.kind {
            LineKind::Deleted => t.diff_delete_inner_background,
            _ => t.diff_add_inner_background,
        };
        highlights = overlay(&text, highlights, &line.marks, bg);
    }
    StyledText::new(SharedString::from(text)).with_highlights(highlights)
}

fn overlay(
    text: &str,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    marks: &[Range<usize>],
    bg: Hsla,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut cuts: Vec<usize> = highlights
        .iter()
        .flat_map(|(r, _)| [r.start, r.end])
        .chain(marks.iter().flat_map(|m| [m.start, m.end]))
        .map(|c| c.min(text.len()))
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = Vec::new();
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a >= b || !text.is_char_boundary(a) || !text.is_char_boundary(b) {
            continue;
        }
        let base = highlights
            .iter()
            .find(|(r, _)| r.start <= a && b <= r.end)
            .map(|(_, h)| *h);
        let marked = marks.iter().any(|m| m.start <= a && b <= m.end);
        let mut h = base.unwrap_or_default();
        if marked {
            h.background_color = Some(bg);
        }
        if base.is_some() || marked {
            out.push((a..b, h));
        }
    }
    out
}

/// A line diff of two texts (equal texts show plainly), folded around its
/// changes. `part` keys the folds the user opened (0 source, 8 metadata,
/// 9 notebook metadata, 16+ outputs).
pub(super) fn text_diff_element(
    shared: &Shared,
    ix: usize,
    part: u8,
    old: &str,
    new: &str,
    ext: &str,
    cx: &mut App,
) -> AnyElement {
    let t = cx.ghd().clone();
    let lines = if old == new {
        whole(new, LineKind::Context)
    } else if old.is_empty() {
        whole(new, LineKind::Added)
    } else if new.is_empty() {
        whole(old, LineKind::Deleted)
    } else {
        line_diff(old, new)
    };
    let old_spans = spans(shared, ext, old);
    let new_spans = spans(shared, ext, new);
    let span_of = |line: &TextLine| -> Option<Vec<corvene_highlight::Span>> {
        match line.kind {
            LineKind::Deleted => old_spans
                .as_ref()
                .and_then(|s| s.get(line.old? as usize - 1).cloned()),
            _ => new_spans
                .as_ref()
                .and_then(|s| s.get(line.new? as usize - 1).cloned()),
        }
    };
    let open = shared.ui.borrow().open_lines.clone();
    let segs = segments(&lines, CONTEXT);
    let weak = shared.weak.clone();
    let mut rows: Vec<AnyElement> = Vec::new();
    let mut drawn = 0usize;
    let split = shared.split && old != new && !old.is_empty() && !new.is_empty();
    for seg in segs {
        let range = match seg {
            Segment::Lines(r) => r,
            Segment::Folded { lines: r } if open.contains(&(ix, part, r.start)) => r,
            Segment::Folded { lines: r } => {
                let start = r.start;
                let n = r.len();
                let weak = weak.clone();
                let hover = t.box_hover_background;
                rows.push(
                    div()
                        .id(("notebook-lines-fold", hash_of((ix, part, start)) as usize))
                        .a11y_button(format!("Show {n} unchanged lines"))
                        .w_full()
                        .h(zpx(18.))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(SPACING_HALF())
                        .px(SPACING_HALF())
                        .bg(t.diff_hunk_background)
                        .text_color(t.text_secondary)
                        .text_size(FONT_SIZE_XS())
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover))
                        .child(octicon(Octicon::Unfold, t.text_secondary))
                        .child(format!(
                            "{n} unchanged {}",
                            if n == 1 { "line" } else { "lines" }
                        ))
                        .on_click(move |_, _, cx| {
                            weak.update(cx, |this, cx| this.open_lines((ix, part, start), cx))
                                .ok();
                        })
                        .into_any_element(),
                );
                continue;
            }
        };
        let slice = &lines[range.clone()];
        if split {
            for (a, b) in split_rows(slice) {
                if drawn >= MAX_LINES {
                    break;
                }
                drawn += 1;
                let half = |k: Option<usize>, left: bool| {
                    let line = k.map(|k| &slice[k]);
                    let (bg, number) = match line {
                        Some(l) if l.kind == LineKind::Deleted => (t.diff_delete_background, l.old),
                        Some(l) if l.kind == LineKind::Added => (t.diff_add_background, l.new),
                        Some(l) => (
                            gpui_kit::transparent_black(),
                            if left { l.old } else { l.new },
                        ),
                        None => (t.diff_gutter_background, None),
                    };
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_row()
                        .bg(bg)
                        .child(line_number(number, &t))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .pl(zpx(4.))
                                .children(line.map(|l| styled_line(l, span_of(l).as_deref(), &t))),
                        )
                };
                rows.push(
                    div()
                        .w_full()
                        .flex()
                        .flex_row()
                        .child(half(a, true))
                        .child(div().flex_none().w(px(1.)).bg(t.diff_border))
                        .child(half(b, false))
                        .into_any_element(),
                );
            }
        } else {
            for line in slice {
                if drawn >= MAX_LINES {
                    break;
                }
                drawn += 1;
                let (bg, sign) = match line.kind {
                    LineKind::Added => (t.diff_add_background, "+"),
                    LineKind::Deleted => (t.diff_delete_background, "-"),
                    LineKind::Context => (gpui_kit::transparent_black(), " "),
                };
                let changes = old != new;
                rows.push(
                    div()
                        .w_full()
                        .flex()
                        .flex_row()
                        .bg(bg)
                        .when(changes, |d| d.child(line_number(line.old, &t)))
                        .child(line_number(line.new.or(line.old), &t))
                        .when(changes, |d| {
                            d.child(
                                div()
                                    .flex_none()
                                    .w(zpx(12.))
                                    .text_color(t.text_secondary)
                                    .child(sign),
                            )
                        })
                        .child(div().flex_1().min_w_0().pl(zpx(4.)).child(styled_line(
                            line,
                            span_of(line).as_deref(),
                            &t,
                        )))
                        .into_any_element(),
                );
            }
        }
    }
    let more = lines.len().saturating_sub(drawn);
    div()
        .w_full()
        .py(zpx(3.))
        .flex()
        .flex_col()
        .font_family(mono_font())
        .text_size(FONT_SIZE_SM())
        .line_height(relative(1.45))
        .children(rows)
        .when(more > 0 && drawn >= MAX_LINES, |d| {
            d.child(
                div()
                    .px(SPACING())
                    .text_color(t.text_secondary)
                    .child(format!("… {more} more lines (see the JSON view)")),
            )
        })
        .into_any_element()
}

fn line_number(n: Option<u32>, t: &GhdTheme) -> Div {
    div()
        .flex_none()
        .w(zpx(30.))
        .pr(zpx(4.))
        .flex()
        .justify_end()
        .text_color(t.text_secondary)
        .text_size(FONT_SIZE_XS())
        .children(n.map(|n| SharedString::from(n.to_string())))
}

/// Pictures of a markdown cell: its attachments and the repository images
/// the notebook read for it.
fn resolver(shared: &Shared, cell: Option<&NbCell>, current: bool) -> ImageResolver {
    let attachments: Vec<(String, String, String)> = cell
        .map(|c| {
            c.attachments
                .iter()
                .filter_map(|a| {
                    let m = a.data.iter().find(|m| m.mime.starts_with("image/"))?;
                    match &m.data {
                        NbData::Base64(b) | NbData::Text(b) => {
                            Some((a.name.clone(), m.mime.clone(), b.clone()))
                        }
                        NbData::Json(_) => None,
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let nb = if current {
        shared.current.clone()
    } else {
        shared.previous.clone()
    };
    let ui = shared.ui.clone();
    Rc::new(move |src: &str| {
        if let Some(name) = src.strip_prefix("attachment:") {
            let (_, mime, data) = attachments.iter().find(|(n, _, _)| n == name)?;
            return super::outputs::decode_image(&ui, mime, data);
        }
        let blob = nb
            .as_ref()?
            .linked_images
            .iter()
            .find(|(p, _)| p == src)
            .map(|(_, b)| b)?;
        let key = super::outputs::payload_key("linked", &blob.bytes);
        if let Some(hit) = ui.borrow().images.get(&key) {
            return hit.clone();
        }
        let format = match blob.media_type.as_str() {
            "image/png" => ImageFormat::Png,
            "image/jpg" | "image/jpeg" => ImageFormat::Jpeg,
            "image/gif" => ImageFormat::Gif,
            "image/webp" => ImageFormat::Webp,
            "image/bmp" => ImageFormat::Bmp,
            "image/svg+xml" => ImageFormat::Svg,
            _ => return None,
        };
        let image = Some(Arc::new(Image::from_bytes(format, blob.bytes.clone())));
        ui.borrow_mut().images.insert(key, image.clone());
        image
    })
}

/// A block tint: the bar on the left and the background.
fn strip(child: AnyElement, status: BlockStatus, t: &GhdTheme) -> Div {
    let (bar, bg) = match status {
        BlockStatus::Added => (t.diff_add_border, t.diff_add_background),
        BlockStatus::Deleted => (t.diff_delete_border, t.diff_delete_background),
        _ => (gpui_kit::transparent_black(), gpui_kit::transparent_black()),
    };
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_row()
        .bg(bg)
        .child(div().flex_none().w(zpx(3.)).bg(bar))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .px(SPACING())
                .py(zpx(3.))
                .child(child),
        )
}

#[allow(clippy::too_many_arguments)]
fn rendered_markdown(
    shared: &Shared,
    ix: usize,
    old: Option<&NbCell>,
    new: Option<&NbCell>,
    old_src: &str,
    new_src: &str,
    status: CellStatus,
    cx: &mut App,
) -> AnyElement {
    let t = cx.ghd().clone();
    let old_blocks = parsed_math(old_src);
    let new_blocks = parsed_math(new_src);
    let rows = block_diff(&old_blocks, &new_blocks);
    let options = |images: ImageResolver, marks: &[Range<usize>], deleted: bool| MarkdownOptions {
        macros: Some(shared.macros.clone()),
        marks: (!marks.is_empty()).then(|| {
            let marks: Rc<[Range<usize>]> = marks.to_vec().into();
            let bg = if deleted {
                t.diff_delete_inner_background
            } else {
                t.diff_add_inner_background
            };
            (marks, bg)
        }),
        images: Some(images),
        highlight_code: true,
        font_size: Some(12.),
        ..MarkdownOptions::default()
    };
    let old_images = resolver(shared, old, false);
    let new_images = resolver(shared, new, true);
    let unchanged_cell = matches!(status, CellStatus::Same);
    let mut out: Vec<AnyElement> = Vec::new();
    for (k, row) in rows.iter().enumerate() {
        let id = |side: &str| SharedString::from(format!("nb-md-{ix}-{k}-{side}"));
        let old_el = |status| {
            row.previous.and_then(|i| old_blocks.get(i)).map(|b| {
                let opts = options(old_images.clone(), &row.marks.0, true);
                strip(
                    markdown_with(id("old"), std::slice::from_ref(b), &opts, cx).into_any_element(),
                    status,
                    &t,
                )
                .into_any_element()
            })
        };
        let new_el = |status| {
            row.current.and_then(|i| new_blocks.get(i)).map(|b| {
                let opts = options(new_images.clone(), &row.marks.1, false);
                strip(
                    markdown_with(id("new"), std::slice::from_ref(b), &opts, cx).into_any_element(),
                    status,
                    &t,
                )
                .into_any_element()
            })
        };
        let column = |el: Option<AnyElement>| div().flex_1().min_w_0().children(el);
        let element = match (row.status, shared.split && !unchanged_cell) {
            (BlockStatus::Same, false) => div().w_full().children(new_el(BlockStatus::Same)),
            (BlockStatus::Same, true) => div()
                .w_full()
                .flex()
                .flex_row()
                .child(column(old_el(BlockStatus::Same)))
                .child(column(new_el(BlockStatus::Same))),
            (_, true) => div()
                .w_full()
                .flex()
                .flex_row()
                .child(column(old_el(BlockStatus::Deleted)))
                .child(column(new_el(BlockStatus::Added))),
            (_, false) => div()
                .w_full()
                .flex()
                .flex_col()
                .children(old_el(BlockStatus::Deleted))
                .children(new_el(BlockStatus::Added)),
        };
        out.push(element.into_any_element());
    }
    div()
        .w_full()
        .py(zpx(4.))
        .flex()
        .flex_col()
        .gap(zpx(6.))
        .children(out)
        .into_any_element()
}
