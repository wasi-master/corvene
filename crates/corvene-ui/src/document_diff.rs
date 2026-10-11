//! Corvene `1324-document-diff`: a Word, PowerPoint, OpenDocument, EPUB or
//! RTF file's diff drawn as the document. Headings, formatted text, lists,
//! tables and pictures are laid out as a text editor would; an added block
//! is tinted green, a removed one red, and an edited one shows its old
//! version (removed words marked) over its new one (added words marked),
//! or beside it in the side-by-side layout. Presentations get a tab per
//! slide (speaker notes under the slide), books a tab per chapter.
//!
//! GitHub Desktop shows these files as binary (`app/src/ui/diff/
//! binary-file.tsx`); the diff header's switch also offers their text as a
//! line diff.

use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use corvene_core::document_diff::{Pair, Status, block_rows, block_text, section_list};
use corvene_core::{Diff, DocBlob, DocBlock, DocKind, DocRun, ImageBlob};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::diff_view_rows::word_changes;
use crate::icons::{Octicon, octicon};
use crate::scrollbar::{gutter, scrollbar};
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, GhdTheme};
use crate::widgets::{GhdTooltip, IconButtonA11y};

/// The widest a column of text gets (CSS px), like a page.
const MAX_TEXT_WIDTH: f32 = 820.;
/// A list level's indent (CSS px).
const LIST_INDENT: f32 = 20.;
/// The tallest a picture is drawn (CSS px).
const MAX_IMAGE_HEIGHT: f32 = 360.;

/// An edited block's changed word ranges: (old text, new text).
type WordChanges = (Vec<Range<usize>>, Vec<Range<usize>>);

/// One row of the list.
#[derive(Clone)]
enum Row {
    /// "Speaker notes", or a note that nothing changed.
    Label(SharedString),
    Block {
        pair: Pair,
        /// A block of the section's notes.
        notes: bool,
        /// An edited block's changed word ranges (old text, new text).
        words: Rc<WordChanges>,
    },
}

pub struct DocumentDiff {
    diff: Arc<Diff>,
    sections: Vec<Pair>,
    /// The shown section (index into `sections`).
    section: usize,
    rows: Rc<Vec<Row>>,
    list: ListState,
    /// The pictures of (previous, current), ready to draw.
    images: Rc<[Vec<Option<Arc<Image>>>; 2]>,
}

impl DocumentDiff {
    /// `diff` is a `Diff::Document`.
    pub fn new(diff: Arc<Diff>, _cx: &mut Context<Self>) -> Self {
        let (previous, current) = sides(&diff);
        let sections = section_list(previous, current);
        let section = sections
            .iter()
            .position(|s| s.status != Status::Same)
            .unwrap_or(0);
        let images = Rc::new([
            previous.map(|d| images_of(&d.images)).unwrap_or_default(),
            current.map(|d| images_of(&d.images)).unwrap_or_default(),
        ]);
        let mut this = Self {
            diff,
            sections,
            section,
            rows: Rc::new(Vec::new()),
            list: ListState::new(0, ListAlignment::Top, zpx(400.)),
            images,
        };
        this.build_rows();
        this
    }

    /// The document diff shown.
    pub fn diff(&self) -> &Arc<Diff> {
        &self.diff
    }

    fn blobs(&self) -> (Option<&DocBlob>, Option<&DocBlob>) {
        sides(&self.diff)
    }

    fn build_rows(&mut self) {
        let mut rows = Vec::new();
        let (previous, current) = self.blobs();
        if let Some(pair) = self.sections.get(self.section).copied() {
            let old = pair.previous.and_then(|i| previous?.sections.get(i));
            let new = pair.current.and_then(|i| current?.sections.get(i));
            let (old_images, new_images) = (images_slice(previous), images_slice(current));
            let add = |rows: &mut Vec<Row>,
                       old_blocks: &[DocBlock],
                       new_blocks: &[DocBlock],
                       notes: bool| {
                for p in block_rows((old_blocks, old_images), (new_blocks, new_images)) {
                    let words = match (p.status, p.previous, p.current) {
                        (Status::Modified, Some(a), Some(b)) => {
                            word_changes(&block_text(&old_blocks[a]), &block_text(&new_blocks[b]))
                        }
                        _ => (Vec::new(), Vec::new()),
                    };
                    rows.push(Row::Block {
                        pair: p,
                        notes,
                        words: Rc::new(words),
                    });
                }
            };
            let empty: &[DocBlock] = &[];
            add(
                &mut rows,
                old.map_or(empty, |s| &s.blocks),
                new.map_or(empty, |s| &s.blocks),
                false,
            );
            let (old_notes, new_notes) = (
                old.map_or(empty, |s| &s.notes),
                new.map_or(empty, |s| &s.notes),
            );
            if !old_notes.is_empty() || !new_notes.is_empty() {
                rows.push(Row::Label("Speaker notes".into()));
                add(&mut rows, old_notes, new_notes, true);
            }
        }
        if rows.is_empty() {
            rows.push(Row::Label(
                "This part of the document has no text or pictures.".into(),
            ));
        }
        if self.sections.iter().all(|s| s.status == Status::Same) {
            rows.insert(
                0,
                Row::Label(
                    "The text and pictures did not change; only the formatting or other parts \
                     of the file did."
                        .into(),
                ),
            );
        }
        self.list.reset(rows.len());
        self.rows = Rc::new(rows);
    }

    fn select_section(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.sections.len() && ix != self.section {
            self.section = ix;
            self.build_rows();
            cx.notify();
        }
    }

    /// The next (or previous) changed row of the shown section, wrapping
    /// into the next changed section.
    fn step_change(&mut self, forward: bool, cx: &mut Context<Self>) {
        let changed = |r: &Row| matches!(r, Row::Block { pair, .. } if pair.status != Status::Same);
        let top = self.list.logical_scroll_top().item_ix;
        let rows = self.rows.clone();
        let found = if forward {
            (top + 1..rows.len()).find(|&i| changed(&rows[i]))
        } else {
            (0..top).rev().find(|&i| changed(&rows[i]))
        };
        if let Some(ix) = found {
            self.list.scroll_to_reveal_item(ix);
            cx.notify();
            return;
        }
        let n = self.sections.len();
        let next = (1..n)
            .map(|k| {
                if forward {
                    (self.section + k) % n
                } else {
                    (self.section + n - k) % n
                }
            })
            .find(|&i| self.sections[i].status != Status::Same);
        if let Some(next) = next {
            self.select_section(next, cx);
            let rows = self.rows.clone();
            let first = if forward {
                rows.iter().position(changed)
            } else {
                rows.iter().rposition(changed)
            };
            if let Some(ix) = first {
                self.list.scroll_to_reveal_item(ix);
            }
        }
    }

    fn section_name(&self, pair: &Pair) -> String {
        let (previous, current) = self.blobs();
        let (blob, ix) = match (pair.current, pair.previous) {
            (Some(i), _) => (current, i),
            (None, Some(i)) => (previous, i),
            _ => return String::new(),
        };
        let Some(blob) = blob else {
            return String::new();
        };
        let label = blob.section_label(ix).unwrap_or_default();
        let name = blob.sections.get(ix).map_or("", |s| s.name.as_str());
        let hidden = blob.sections.get(ix).is_some_and(|s| s.hidden);
        let mut out = if name.is_empty() {
            label
        } else if label.is_empty() {
            name.to_string()
        } else {
            format!("{label}: {name}")
        };
        if out.chars().count() > 48 {
            out = out.chars().take(47).collect::<String>() + "…";
        }
        if hidden {
            out.push_str(" (hidden)");
        }
        out
    }

    fn tabs(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (previous, current) = self.blobs();
        let kind = current.or(previous)?.kind;
        if kind == DocKind::Text {
            return None;
        }
        let t = cx.ghd().clone();
        let tabs = self.sections.iter().enumerate().map(|(ix, pair)| {
            let selected = ix == self.section;
            let (mark, tip) = match pair.status {
                Status::Same => (None, None),
                Status::Added => (Some(t.color_new), Some("Added")),
                Status::Deleted => (Some(t.color_deleted), Some("Removed")),
                Status::Modified => (Some(t.color_modified), Some("Changed")),
            };
            let hover = t.box_hover_background;
            div()
                .id(("document-section", ix))
                .a11y_button(self.section_name(pair))
                .flex_none()
                .h_full()
                .px(SPACING())
                .flex()
                .flex_row()
                .items_center()
                .gap(SPACING_HALF())
                .border_r_1()
                .border_color(t.diff_border)
                .when(pair.status == Status::Deleted, |d| d.line_through())
                .when(selected, |d| {
                    d.bg(t.box_selected_active_background)
                        .text_color(t.box_selected_active_text)
                })
                .when(!selected, |d| {
                    d.cursor_pointer()
                        .text_color(t.text)
                        .hover(move |d| d.bg(hover))
                        .on_click(cx.listener(move |this, _, _, cx| this.select_section(ix, cx)))
                })
                .children(
                    mark.map(|color| div().flex_none().size(zpx(6.)).rounded_full().bg(color)),
                )
                .child(self.section_name(pair))
                .when_some(tip, |d, tip| d.ghd_tooltip(tip))
        });
        Some(
            div()
                .id("document-sections")
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
                .into_any_element(),
        )
    }

    /// What changed, and the change buttons.
    fn summary(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.ghd().clone();
        let count = |status: Status| {
            self.rows
                .iter()
                .filter(|r| matches!(r, Row::Block { pair, .. } if pair.status == status))
                .count()
        };
        let (added, deleted, edited) = (
            count(Status::Added),
            count(Status::Deleted),
            count(Status::Modified),
        );
        let (previous, current) = self.blobs();
        let kind = current.or(previous).map_or(DocKind::Text, |d| d.kind);
        let changed_sections = self
            .sections
            .iter()
            .filter(|s| s.status != Status::Same)
            .count();
        let unit = match kind {
            DocKind::Text => None,
            DocKind::Presentation => Some(("slide", "slides")),
            DocKind::Book => Some(("chapter", "chapters")),
        };
        let mut parts: Vec<String> = Vec::new();
        if let Some((one, many)) = unit {
            parts.push(format!(
                "{changed_sections} of {} {} changed",
                self.sections.len(),
                if self.sections.len() == 1 { one } else { many }
            ));
        }
        let blocks = |n: usize, what: &str| format!("{n} {what}");
        if added > 0 {
            parts.push(blocks(added, "added"));
        }
        if deleted > 0 {
            parts.push(blocks(deleted, "removed"));
        }
        if edited > 0 {
            parts.push(blocks(edited, "edited"));
        }
        let truncated = [previous, current]
            .into_iter()
            .flatten()
            .any(|d| d.truncated);
        let any_change = changed_sections > 0;
        let button = |id: &'static str, icon: Octicon, label: &'static str| {
            let hover = t.box_hover_background;
            div()
                .id(id)
                .a11y_button(label)
                .flex_none()
                .h(zpx(19.))
                .px(SPACING_HALF())
                .flex()
                .items_center()
                .rounded(BORDER_RADIUS())
                .border_1()
                .border_color(t.box_border)
                .when(any_change, |d| {
                    d.cursor_pointer().hover(move |s| s.bg(hover))
                })
                .child(octicon(
                    icon,
                    if any_change { t.text } else { t.text_secondary },
                ))
                .ghd_tooltip(label)
        };
        div()
            .id("document-summary")
            .flex_none()
            .w_full()
            .px(SPACING())
            .py(SPACING_HALF())
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING())
            .border_b_1()
            .border_color(t.diff_border)
            .text_size(FONT_SIZE_SM())
            .text_color(t.text_secondary)
            .child(div().flex_1().min_w_0().truncate().child(parts.join(" · ")))
            .when(truncated, |d| {
                d.child(div().flex_none().text_color(t.file_warning).child(format!(
                    "Only the first {} blocks are shown",
                    corvene_git::document_file::MAX_BLOCKS
                )))
            })
            .child(
                button(
                    "document-previous-change",
                    Octicon::ChevronUp,
                    "Previous Change",
                )
                .when(any_change, |d| {
                    d.on_click(cx.listener(|this, _, _, cx| this.step_change(false, cx)))
                }),
            )
            .child(
                button("document-next-change", Octicon::ChevronDown, "Next Change")
                    .when(any_change, |d| {
                        d.on_click(cx.listener(|this, _, _, cx| this.step_change(true, cx)))
                    }),
            )
            .into_any_element()
    }
}

fn images_slice(d: Option<&DocBlob>) -> &[ImageBlob] {
    d.map_or(&[], |d| d.images.as_slice())
}

fn sides(diff: &Diff) -> (Option<&DocBlob>, Option<&DocBlob>) {
    match diff {
        Diff::Document {
            previous, current, ..
        } => (previous.as_ref(), current.as_ref()),
        _ => (None, None),
    }
}

fn images_of(blobs: &[ImageBlob]) -> Vec<Option<Arc<Image>>> {
    blobs
        .iter()
        .map(|b| {
            let format = match b.media_type.as_str() {
                "image/png" => ImageFormat::Png,
                "image/jpg" | "image/jpeg" => ImageFormat::Jpeg,
                "image/gif" => ImageFormat::Gif,
                "image/webp" => ImageFormat::Webp,
                "image/bmp" => ImageFormat::Bmp,
                _ => return None,
            };
            Some(Arc::new(Image::from_bytes(format, b.bytes.clone())))
        })
        .collect()
}

/// How a block is drawn within its row.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tint {
    None,
    Added,
    Removed,
}

/// The pieces a block needs to draw.
#[derive(Clone, Copy)]
struct Draw<'a> {
    t: &'a GhdTheme,
    images: &'a [Option<Arc<Image>>],
    /// Changed word ranges in the block's text, and the tint they get.
    words: &'a [Range<usize>],
    tint: Tint,
    /// The other version of an edited table (changed cells are tinted).
    other: Option<&'a DocBlock>,
}

impl Draw<'_> {
    fn mark_bg(&self) -> Hsla {
        match self.tint {
            Tint::Removed => self.t.diff_delete_inner_background,
            _ => self.t.diff_add_inner_background,
        }
    }

    /// Runs as styled text, the changed words highlighted.
    fn text(&self, runs: &[DocRun]) -> StyledText {
        let mut text = String::new();
        let mut spans: Vec<(Range<usize>, &DocRun)> = Vec::new();
        for run in runs {
            let start = text.len();
            // a tab is one byte too, so the word ranges still fit
            text.push_str(&run.text.replace('\t', " "));
            spans.push((start..text.len(), run));
        }
        let mut cuts: Vec<usize> = spans.iter().flat_map(|(r, _)| [r.start, r.end]).collect();
        for w in self.words {
            cuts.push(w.start.min(text.len()));
            cuts.push(w.end.min(text.len()));
        }
        cuts.sort_unstable();
        cuts.dedup();
        let mut highlights = Vec::new();
        let mut fonts: Vec<(Range<usize>, SharedString)> = Vec::new();
        for pair in cuts.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if a >= b || !text.is_char_boundary(a) || !text.is_char_boundary(b) {
                continue;
            }
            let Some((_, run)) = spans.iter().find(|(r, _)| r.start <= a && a < r.end) else {
                continue;
            };
            let marked = self.words.iter().any(|w| w.start <= a && b <= w.end);
            let s = run.style;
            let link = run.link.is_some();
            highlights.push((
                a..b,
                HighlightStyle {
                    color: link.then_some(self.t.link),
                    font_weight: s.bold.then_some(FontWeight::BOLD),
                    font_style: s.italic.then_some(FontStyle::Italic),
                    background_color: if marked {
                        Some(self.mark_bg())
                    } else if s.code {
                        Some(self.t.md_neutral_muted)
                    } else {
                        None
                    },
                    underline: (s.underline || link).then_some(UnderlineStyle {
                        thickness: zpx(1.),
                        color: None,
                        wavy: false,
                    }),
                    strikethrough: s.strike.then_some(StrikethroughStyle {
                        thickness: zpx(1.),
                        color: None,
                    }),
                    fade_out: None,
                },
            ));
            if s.code {
                fonts.push((a..b, crate::theme::mono_font().into()));
            }
        }
        StyledText::new(SharedString::from(text))
            .with_highlights(highlights)
            .with_font_family_overrides(fonts)
    }

    fn block(&self, block: &DocBlock) -> AnyElement {
        let t = self.t;
        match block {
            DocBlock::Heading { level, runs } => {
                let scale = match level {
                    1 => 1.7,
                    2 => 1.4,
                    3 => 1.2,
                    _ => 1.05,
                };
                div()
                    .w_full()
                    .pt(zpx(if *level <= 2 { 6. } else { 2. }))
                    .text_size(FONT_SIZE() * scale)
                    .line_height(relative(1.3))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.text(runs))
                    .into_any_element()
            }
            DocBlock::Paragraph { runs, list, quote } => {
                let body = div().flex_1().min_w_0().child(self.text(runs));
                let row = match list {
                    Some(mark) => {
                        let marker = if mark.ordered {
                            format!("{}.", mark.number)
                        } else {
                            ["•", "◦", "▪"][usize::from(mark.level) % 3].to_string()
                        };
                        div()
                            .w_full()
                            .flex()
                            .flex_row()
                            .pl(zpx(LIST_INDENT * f32::from(mark.level)))
                            .child(
                                div()
                                    .flex_none()
                                    .w(zpx(LIST_INDENT + 6.))
                                    .text_color(t.text_secondary)
                                    .child(marker),
                            )
                            .child(body)
                    }
                    None => div().w_full().child(body),
                };
                row.when(*quote, |d| {
                    d.pl(SPACING())
                        .border_l_2()
                        .border_color(t.md_border_default)
                        .text_color(t.text_secondary)
                })
                .into_any_element()
            }
            DocBlock::Table { rows } => {
                let other_rows = match self.other {
                    Some(DocBlock::Table { rows }) => Some(rows),
                    _ => None,
                };
                let columns = rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
                let tint_bg = self.mark_bg();
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .border_1()
                    .border_color(t.md_border_default)
                    .text_size(FONT_SIZE_SM())
                    .children(rows.iter().enumerate().map(|(r, row)| {
                        div()
                            .w_full()
                            .flex()
                            .flex_row()
                            .when(r > 0, |d| d.border_t_1().border_color(t.md_border_default))
                            .children((0..columns).map(|c| {
                                let cell = row.get(c);
                                let changed = other_rows.is_some()
                                    && other_rows
                                        .and_then(|o| o.get(r))
                                        .and_then(|o| o.get(c))
                                        .map(|o| DocBlock::runs_text(o))
                                        != cell.map(|c| DocBlock::runs_text(c));
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .px(SPACING_HALF())
                                    .py(zpx(2.))
                                    .when(c > 0, |d| {
                                        d.border_l_1().border_color(t.md_border_default)
                                    })
                                    .when(r == 0, |d| d.font_weight(FontWeight::SEMIBOLD))
                                    .when(changed, |d| d.bg(tint_bg))
                                    .children(cell.map(|runs| {
                                        Draw {
                                            words: &[],
                                            other: None,
                                            ..*self
                                        }
                                        .text(runs)
                                    }))
                            }))
                    }))
                    .into_any_element()
            }
            DocBlock::Image { index, alt } => {
                let image = index.and_then(|i| self.images.get(i).cloned().flatten());
                match image {
                    Some(image) => div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap(zpx(2.))
                        .child(
                            img(image)
                                .max_w_full()
                                .max_h(zpx(MAX_IMAGE_HEIGHT))
                                .object_fit(ObjectFit::Contain),
                        )
                        .when(!alt.is_empty(), |d| {
                            d.child(
                                div()
                                    .text_size(FONT_SIZE_SM())
                                    .text_color(t.text_secondary)
                                    .child(alt.clone()),
                            )
                        })
                        .into_any_element(),
                    None => div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(zpx(4.))
                        .px(SPACING())
                        .py(SPACING_HALF())
                        .rounded(BORDER_RADIUS())
                        .border_1()
                        .border_color(t.md_border_default)
                        .text_color(t.text_secondary)
                        .child(octicon(Octicon::Image, t.text_secondary))
                        .child(if alt.is_empty() {
                            SharedString::from("Picture")
                        } else {
                            SharedString::from(alt.clone())
                        })
                        .into_any_element(),
                }
            }
            DocBlock::Code(text) => div()
                .w_full()
                .px(SPACING())
                .py(SPACING_HALF())
                .rounded(BORDER_RADIUS())
                .bg(t.md_neutral_muted)
                .font_family(crate::theme::mono_font())
                .text_size(FONT_SIZE_SM())
                .child(
                    Draw {
                        other: None,
                        ..*self
                    }
                    .text(&[DocRun::plain(text.clone())]),
                )
                .into_any_element(),
            DocBlock::Break => div()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .gap(SPACING())
                .text_size(FONT_SIZE_XS())
                .text_color(t.text_secondary)
                .child(div().flex_1().h(px(1.)).bg(t.md_border_default))
                .child("Page break")
                .child(div().flex_1().h(px(1.)).bg(t.md_border_default))
                .into_any_element(),
        }
        .into_any_element()
    }
}

/// A tinted strip around a block: the bar on the left and the background.
fn strip(child: AnyElement, tint: Tint, notes: bool, t: &GhdTheme) -> Div {
    let (bar, bg) = match tint {
        Tint::None => (gpui_kit::transparent_black(), gpui_kit::transparent_black()),
        Tint::Added => (t.diff_add_border, t.diff_add_background),
        Tint::Removed => (t.diff_delete_border, t.diff_delete_background),
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
                .py(zpx(4.))
                .when(notes, |d| d.text_size(FONT_SIZE_SM()))
                .child(child),
        )
}

/// What the list's rows draw from, shared with its item callback.
struct Rows {
    diff: Arc<Diff>,
    rows: Rc<Vec<Row>>,
    section: Option<Pair>,
    images: Rc<[Vec<Option<Arc<Image>>>; 2]>,
}

impl Rows {
    fn row(&self, ix: usize, side_by_side: bool, cx: &App) -> AnyElement {
        let t = cx.ghd();
        let Some(row) = self.rows.get(ix) else {
            return div().into_any_element();
        };
        let (pair, notes, words) = match row {
            Row::Label(text) => {
                return div()
                    .w_full()
                    .px(SPACING())
                    .pt(SPACING())
                    .pb(zpx(4.))
                    .text_size(FONT_SIZE_SM())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.text_secondary)
                    .child(text.clone())
                    .into_any_element();
            }
            Row::Block { pair, notes, words } => (*pair, *notes, words.clone()),
        };
        let section = self.section;
        let (previous, current) = sides(&self.diff);
        fn block_of(
            blob: Option<&DocBlob>,
            sec: Option<usize>,
            i: Option<usize>,
            notes: bool,
        ) -> Option<&DocBlock> {
            let s = blob?.sections.get(sec?)?;
            let blocks = if notes { &s.notes } else { &s.blocks };
            blocks.get(i?)
        }
        let old = block_of(
            previous,
            section.and_then(|s| s.previous),
            pair.previous,
            notes,
        );
        let new = block_of(
            current,
            section.and_then(|s| s.current),
            pair.current,
            notes,
        );
        let draw = |side: usize, block: &DocBlock, tint: Tint, words: &[Range<usize>], other| {
            let d = Draw {
                t,
                images: &self.images[side],
                words,
                tint,
                other,
            };
            strip(d.block(block), tint, notes, t).into_any_element()
        };
        let (old_words, new_words) = (&words.0[..], &words.1[..]);
        let old_el = |tint| old.map(|b| draw(0, b, tint, old_words, new));
        let new_el = |tint| new.map(|b| draw(1, b, tint, new_words, old));
        let column = |el: Option<AnyElement>| div().flex_1().min_w_0().children(el);
        let body = match (pair.status, side_by_side) {
            (Status::Same, false) => div().w_full().children(new_el(Tint::None)),
            (Status::Same, true) => div()
                .w_full()
                .flex()
                .flex_row()
                .child(column(old_el(Tint::None)))
                .child(column(new_el(Tint::None))),
            (_, true) => div()
                .w_full()
                .flex()
                .flex_row()
                .child(column(old_el(Tint::Removed)))
                .child(column(new_el(Tint::Added))),
            (_, false) => div()
                .w_full()
                .flex()
                .flex_col()
                .children(old_el(Tint::Removed))
                .children(new_el(Tint::Added)),
        };
        div()
            .id(("document-row", ix))
            .w_full()
            .flex()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .when(!side_by_side, |d| d.max_w(zpx(MAX_TEXT_WIDTH)))
                    .child(body),
            )
            .into_any_element()
    }
}

impl DocumentDiff {
    pub fn render_with(&mut self, side_by_side: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.ghd().clone();
        let shared = Rc::new(Rows {
            diff: self.diff.clone(),
            rows: self.rows.clone(),
            section: self.sections.get(self.section).copied(),
            images: self.images.clone(),
        });
        let tabs = self.tabs(cx);
        let summary = self.summary(cx);
        div()
            .id("document-diff")
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(t.background)
            .text_color(t.text)
            .text_size(FONT_SIZE() * 1.1)
            .line_height(relative(1.45))
            .children(tabs)
            .child(summary)
            .child(
                list(self.list.clone(), move |ix, _window, cx| {
                    shared.row(ix, side_by_side, cx)
                })
                .flex_1()
                .min_h_0()
                .w_full()
                .pt(SPACING_HALF())
                .pb(SPACING())
                .pr(gutter(&self.list)),
            )
            .child(scrollbar("document-scrollbar", self.list.clone()))
            .into_any_element()
    }
}

impl Render for DocumentDiff {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let side_by_side = corvene_core::AppState::try_global(cx)
            .is_some_and(|s| s.read(cx).settings.show_side_by_side_diff);
        self.render_with(side_by_side, cx)
    }
}
