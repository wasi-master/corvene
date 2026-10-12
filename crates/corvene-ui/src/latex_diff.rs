//! Corvene `1332-latex-diff`: a LaTeX document or BibTeX bibliography's
//! diff drawn rendered. A `.tex` file is read into blocks
//! (`corvene_core::latex_doc`: numbered sections, typeset equations, lists,
//! tables, figures, theorems, footnotes, resolved references) and the two
//! versions are aligned block by block: added blocks tinted green, removed
//! ones red, and an edited block drawn once (`corvene_core::block_diff::
//! merge`): a paragraph or heading with its removed words struck through in
//! red before the added ones in green (each version with its own marks side
//! by side in split mode), a table under the same header as one table with
//! its added rows green, removed rows red and changed cells marked, and a
//! block quote, alert, `<details>` or list item as one container with only
//! the blocks inside it that changed tinted or marked. Other edited blocks
//! (code, formulas, a paragraph whose pictures changed) show old over new.
//! A `.bib` file shows each entry as
//! a formatted reference with the fields that changed.
//!
//! GitHub Desktop shows these files as text only (`app/src/ui/diff/
//! text-diff.tsx`); the header's switch keeps that view as Text.
//!
//! Corvene `1334-markdown-preview-diff`: a Markdown file's Preview is the
//! same view over `corvene_core::markdown_doc` blocks (math typeset with
//! `1333-markdown-math`).
//!
//! Both open scrolled to the first change (a Markdown preview switched to
//! from a scrolled Source view: to the block holding its top line), and
//! runs of unchanged blocks fold as the text diff's context does
//! (`corvene_core::fold`).

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use corvene_core::bibtex::{self, BibChange, BibEntry};
use corvene_core::block_diff::{BlockRow, BlockStatus, Change, Merged, Side, block_diff};
use corvene_core::fold::{FoldRow, FoldRule, fold};
use corvene_core::markdown::{Block, ListItem};
use corvene_core::{Diff, LatexBlob, LatexKind};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::icons::{Octicon, octicon};
use crate::markdown::{
    BlockTint, BlockTints, CodeLineTint, ImageResolver, MarkLayer, MarkdownOptions, TableRowTint,
    markdown_with,
};
use crate::math::SharedMacros;
use crate::scrollbar::{gutter, scrollbar};
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, GhdTheme};
use crate::widgets::{GhdTooltip, IconButtonA11y};

/// The widest a column of text gets, like a page.
const MAX_TEXT_WIDTH: f32 = 820.;

/// What the view shows once read.
enum Content {
    Document {
        old: Rc<Vec<Block>>,
        new: Rc<Vec<Block>>,
        rows: Rc<Vec<BlockRow>>,
        /// The line each new block ends on (Markdown only).
        new_lines: Rc<Vec<u32>>,
        macros: SharedMacros,
        title: Option<String>,
    },
    Bibliography {
        old: Rc<Vec<BibEntry>>,
        new: Rc<Vec<BibEntry>>,
        changes: Rc<Vec<BibChange>>,
    },
}

pub struct LatexDiff {
    diff: Arc<Diff>,
    content: Option<Rc<Content>>,
    _task: Option<Task<()>>,
    list: ListState,
    /// Each version's pictures, ready to draw.
    images: [ImageResolver; 2],
    /// A scroll asked for before the content was read: to the block
    /// holding this line of the new file, or (`None`) to the first change.
    reveal: Option<Option<u32>>,
    /// The list's rows: blocks (or entries) and folds of unchanged ones.
    view: Rc<Vec<FoldRow>>,
    /// The `start` of folds the user opened.
    expanded: HashSet<usize>,
    weak: WeakEntity<Self>,
}

fn sides(diff: &Diff) -> (Option<&LatexBlob>, Option<&LatexBlob>) {
    match diff {
        Diff::Latex {
            previous, current, ..
        }
        | Diff::Markdown {
            previous, current, ..
        } => (previous.as_deref(), current.as_deref()),
        _ => (None, None),
    }
}

/// What the background read hands back.
enum Read {
    Document {
        old: Vec<Block>,
        new: Vec<Block>,
        rows: Vec<BlockRow>,
        new_lines: Vec<u32>,
        macros: String,
        title: Option<String>,
    },
    Bibliography {
        old: Vec<BibEntry>,
        new: Vec<BibEntry>,
        changes: Vec<BibChange>,
    },
}

impl LatexDiff {
    /// `diff` is a `Diff::Latex`.
    pub fn new(diff: Arc<Diff>, cx: &mut Context<Self>) -> Self {
        Self::build(diff, false, cx)
    }

    /// `1334-markdown-preview-diff`: `diff` is a `Diff::Markdown` of the
    /// file at `path`.
    pub fn markdown(diff: Arc<Diff>, path: &str, cx: &mut Context<Self>) -> Self {
        Self::build(diff, corvene_core::markdown_doc::is_mdx(path), cx)
    }

    fn build(diff: Arc<Diff>, mdx: bool, cx: &mut Context<Self>) -> Self {
        let math = corvene_core::AppState::try_global(cx).is_some_and(|s| {
            s.read(cx)
                .flags
                .bool(corvene_core::flags::ids::MARKDOWN_MATH)
        });
        let job = diff.clone();
        let task = cx.background_executor().spawn(async move {
            let (previous, current) = sides(&job);
            let text = |b: Option<&LatexBlob>| b.map(|b| b.text.clone()).unwrap_or_default();
            let kind = match &*job {
                Diff::Latex { kind, .. } => *kind,
                Diff::Markdown { .. } => {
                    let parse =
                        |t: String| corvene_core::markdown_doc::parse_document_lines(&t, mdx, math);
                    let (old, _) = parse(text(previous));
                    let (new, new_lines) = parse(text(current));
                    let mut rows = block_diff(&old, &new);
                    // a picture redrawn under the same name changed its block
                    let bytes = |blob: Option<&LatexBlob>, src: &str| {
                        blob.and_then(|b| b.images.iter().find(|(n, _)| n == src))
                            .map(|(_, i)| i.bytes.clone())
                    };
                    for row in &mut rows {
                        let (BlockStatus::Same, Some(i), Some(j)) =
                            (row.status, row.previous, row.current)
                        else {
                            continue;
                        };
                        let srcs =
                            corvene_core::markdown::image_sources(std::slice::from_ref(&new[j]));
                        if srcs
                            .iter()
                            .any(|src| bytes(previous, src) != bytes(current, src))
                            && old.get(i).is_some()
                        {
                            row.status = BlockStatus::Modified;
                        }
                    }
                    return Read::Document {
                        old,
                        new,
                        rows,
                        new_lines,
                        macros: String::new(),
                        title: None,
                    };
                }
                _ => LatexKind::Document,
            };
            match kind {
                LatexKind::Document => {
                    let old = corvene_core::latex_doc::parse(&text(previous));
                    let new = corvene_core::latex_doc::parse(&text(current));
                    let rows = block_diff(&old.blocks, &new.blocks);
                    Read::Document {
                        macros: format!("{}\n{}", old.macros, new.macros),
                        title: new.title.or(old.title),
                        old: old.blocks,
                        new: new.blocks,
                        rows,
                        new_lines: Vec::new(),
                    }
                }
                LatexKind::Bibliography => {
                    let old = bibtex::parse(&text(previous));
                    let new = bibtex::parse(&text(current));
                    let changes = bibtex::diff(&old, &new);
                    Read::Bibliography { old, new, changes }
                }
            }
        });
        let _task = Some(cx.spawn(async move |this, cx| {
            let read = task.await;
            this.update(cx, |this, cx| {
                let content = match read {
                    Read::Document {
                        old,
                        new,
                        rows,
                        new_lines,
                        macros,
                        title,
                    } => Content::Document {
                        old: Rc::new(old),
                        new: Rc::new(new),
                        rows: Rc::new(rows),
                        new_lines: Rc::new(new_lines),
                        macros: Arc::new(corvene_math::Macros::collect(&macros)),
                        title,
                    },
                    Read::Bibliography { old, new, changes } => Content::Bibliography {
                        old: Rc::new(old),
                        new: Rc::new(new),
                        changes: Rc::new(changes),
                    },
                };
                this.content = Some(Rc::new(content));
                this.refold();
                this.apply_reveal();
                cx.notify();
            })
            .ok();
        }));
        let (previous, current) = sides(&diff);
        let images = [resolver(previous), resolver(current)];
        Self {
            diff,
            content: None,
            _task,
            list: ListState::new(0, ListAlignment::Top, zpx(600.)),
            images,
            reveal: Some(None),
            view: Rc::new(Vec::new()),
            expanded: HashSet::new(),
            weak: cx.weak_entity(),
        }
    }

    /// Rebuild the list's rows from the content and the opened folds.
    fn refold(&mut self) {
        let changed: Vec<bool> = (0..self.count()).map(|i| self.changed(i)).collect();
        self.view = Rc::new(fold(&changed, FoldRule::BLOCKS, &self.expanded));
        self.list.reset(self.view.len());
    }

    fn expand_fold(&mut self, start: usize, cx: &mut Context<Self>) {
        let top = self.list.logical_scroll_top();
        self.expanded.insert(start);
        self.refold();
        self.list.scroll_to(top);
        cx.notify();
    }

    /// The list row showing item `item` (the fold holding it, if folded).
    fn row_of(&self, item: usize) -> Option<usize> {
        self.view.iter().position(|r| match r {
            FoldRow::Item(i) => *i == item,
            FoldRow::Folded { start, end } => (*start..*end).contains(&item),
        })
    }

    /// Whether list row `ix` is a changed item.
    fn row_changed(&self, ix: usize) -> bool {
        matches!(self.view.get(ix), Some(FoldRow::Item(i)) if self.changed(*i))
    }

    /// Scroll to the block holding `line` of the new file (`None`: to the
    /// first change), now or once read.
    pub fn reveal(&mut self, line: Option<u32>, cx: &mut Context<Self>) {
        self.reveal = Some(line);
        self.apply_reveal();
        cx.notify();
    }

    fn apply_reveal(&mut self) {
        let Some(content) = self.content.clone() else {
            return;
        };
        let Some(line) = self.reveal.take() else {
            return;
        };
        let at_line = match (&*content, line) {
            (
                Content::Document {
                    rows, new_lines, ..
                },
                Some(line),
            ) => rows.iter().position(|r| {
                r.current
                    .and_then(|j| new_lines.get(j))
                    .is_some_and(|&end| end >= line)
            }),
            _ => None,
        };
        let at_line = at_line.and_then(|item| self.row_of(item));
        let first = || {
            let ix = (0..self.view.len()).find(|&i| self.row_changed(i))?;
            // the block of context above stays in view
            Some(match ix.checked_sub(1).map(|p| &self.view[p]) {
                Some(FoldRow::Item(_)) => ix - 1,
                _ => ix,
            })
        };
        let Some(ix) = at_line.or_else(first) else {
            return;
        };
        self.list.scroll_to(ListOffset {
            item_ix: ix,
            offset_in_item: px(0.),
        });
    }

    /// The LaTeX diff shown.
    pub fn diff(&self) -> &Arc<Diff> {
        &self.diff
    }

    fn changed(&self, ix: usize) -> bool {
        match self.content.as_deref() {
            Some(Content::Document { rows, .. }) => {
                rows.get(ix).is_some_and(|r| r.status != BlockStatus::Same)
            }
            Some(Content::Bibliography { changes, .. }) => changes
                .get(ix)
                .is_some_and(|c| !matches!(c, BibChange::Same(..))),
            None => false,
        }
    }

    fn count(&self) -> usize {
        match self.content.as_deref() {
            Some(Content::Document { rows, .. }) => rows.len(),
            Some(Content::Bibliography { changes, .. }) => changes.len(),
            None => 0,
        }
    }

    fn step_change(&mut self, forward: bool, cx: &mut Context<Self>) {
        let top = self.list.logical_scroll_top().item_ix;
        let n = self.view.len();
        let found = if forward {
            (top + 1..n).find(|&i| self.row_changed(i))
        } else {
            (0..top).rev().find(|&i| self.row_changed(i))
        };
        if let Some(ix) = found {
            self.list.scroll_to(ListOffset {
                item_ix: ix,
                offset_in_item: px(0.),
            });
            cx.notify();
        }
    }

    fn summary(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.ghd().clone();
        let (mut added, mut deleted, mut edited) = (0, 0, 0);
        let mut title = None;
        let unit = match self.content.as_deref() {
            Some(Content::Document {
                rows, title: tt, ..
            }) => {
                title = tt.clone();
                for r in rows.iter() {
                    match r.status {
                        BlockStatus::Added => added += 1,
                        BlockStatus::Deleted => deleted += 1,
                        BlockStatus::Modified => edited += 1,
                        BlockStatus::Same => {}
                    }
                }
                "blocks"
            }
            Some(Content::Bibliography { changes, .. }) => {
                for c in changes.iter() {
                    match c {
                        BibChange::Added(_) => added += 1,
                        BibChange::Removed(_) => deleted += 1,
                        BibChange::Changed { .. } => edited += 1,
                        BibChange::Same(..) => {}
                    }
                }
                "entries"
            }
            None => "",
        };
        let text = if self.content.is_none() {
            "Reading the document…".to_string()
        } else if added + deleted + edited == 0 {
            "The rendered text did not change (only markup, comments or spacing did)".to_string()
        } else {
            let mut parts = Vec::new();
            if edited > 0 {
                parts.push(format!("{edited} edited"));
            }
            if added > 0 {
                parts.push(format!("{added} added"));
            }
            if deleted > 0 {
                parts.push(format!("{deleted} removed"));
            }
            format!("{} {unit}", parts.join(" · "))
        };
        let any = added + deleted + edited > 0;
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
                .when(any, |d| d.cursor_pointer().hover(move |s| s.bg(hover)))
                .child(octicon(icon, if any { t.text } else { t.text_secondary }))
                .ghd_tooltip(label)
        };
        div()
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
            .children(title.map(|title| {
                div()
                    .flex_none()
                    .max_w(zpx(320.))
                    .truncate()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.text)
                    .child(title)
            }))
            .child(div().flex_1().min_w_0().truncate().child(text))
            .child(
                button(
                    "latex-previous-change",
                    Octicon::ChevronUp,
                    "Previous Change",
                )
                .when(any, |d| {
                    d.on_click(cx.listener(|this, _, _, cx| this.step_change(false, cx)))
                }),
            )
            .child(
                button("latex-next-change", Octicon::ChevronDown, "Next Change").when(any, |d| {
                    d.on_click(cx.listener(|this, _, _, cx| this.step_change(true, cx)))
                }),
            )
            .into_any_element()
    }
}

/// A tinted strip around a block.
fn strip(child: AnyElement, status: BlockStatus, t: &GhdTheme) -> Div {
    let (bar, bg) = match status {
        BlockStatus::Added => (t.diff_add_border, t.diff_add_background),
        BlockStatus::Deleted => (t.diff_delete_border, t.diff_delete_background),
        BlockStatus::Modified => (t.color_modified, gpui_kit::transparent_black()),
        BlockStatus::Same => (gpui_kit::transparent_black(), gpui_kit::transparent_black()),
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
                .child(child),
        )
}

/// What a path of a [`Merged`] block leads to.
enum Target<'a> {
    Block(&'a Block),
    Item(&'a ListItem),
}

fn resolve<'a>(block: &'a Block, path: &[usize]) -> Option<Target<'a>> {
    let Some((&first, rest)) = path.split_first() else {
        return Some(Target::Block(block));
    };
    match block {
        Block::BlockQuote(blocks)
        | Block::Alert { blocks, .. }
        | Block::Aligned { blocks, .. }
        | Block::Details { blocks, .. } => resolve(blocks.get(first)?, rest),
        Block::List { items, .. } => {
            let item = items.get(first)?;
            match rest.split_first() {
                None => Some(Target::Item(item)),
                Some((&k, rest)) => resolve(item.blocks.get(k)?, rest),
            }
        }
        _ => None,
    }
}

/// How the blocks inside an edited block drawn once are tinted.
fn merged_tints(merged: &Merged, side: Side, t: &GhdTheme) -> BlockTints {
    let mut tints = BlockTints::default();
    let layer = |ranges: &[std::ops::Range<usize>], bg, strike| MarkLayer {
        ranges: ranges.to_vec().into(),
        bg,
        strike,
    };
    for (path, change) in &merged.changes {
        let tint = match change {
            Change::Added => BlockTint {
                bg: Some(t.diff_add_background),
                bar: Some(t.diff_add_border),
                ..BlockTint::default()
            },
            Change::Deleted => BlockTint {
                bg: Some(t.diff_delete_background),
                bar: Some(t.diff_delete_border),
                ..BlockTint::default()
            },
            Change::Words { deleted, added } => BlockTint {
                marks: vec![
                    layer(deleted, t.diff_delete_inner_background, side == Side::Both),
                    layer(added, t.diff_add_inner_background, false),
                ],
                ..BlockTint::default()
            },
            Change::Lines(lines) => BlockTint {
                code_lines: Some(
                    lines
                        .iter()
                        .map(|(status, words)| {
                            let (bg, mark, strike) = match status {
                                BlockStatus::Added => {
                                    (t.diff_add_background, t.diff_add_inner_background, false)
                                }
                                BlockStatus::Deleted => (
                                    t.diff_delete_background,
                                    t.diff_delete_inner_background,
                                    side == Side::Both,
                                ),
                                _ => return CodeLineTint::default(),
                            };
                            CodeLineTint {
                                bg: Some(bg),
                                marks: (!words.is_empty()).then(|| layer(words, mark, strike)),
                            }
                        })
                        .collect(),
                ),
                ..BlockTint::default()
            },
            Change::Table(rows) => BlockTint {
                table_rows: Some(
                    rows.iter()
                        .map(|(status, cells)| match status {
                            BlockStatus::Added => TableRowTint {
                                row: Some(t.diff_add_background),
                                cell: Some(t.diff_add_inner_background),
                                cells: cells.clone(),
                            },
                            BlockStatus::Deleted => TableRowTint {
                                row: Some(t.diff_delete_background),
                                cell: Some(t.diff_delete_inner_background),
                                cells: cells.clone(),
                            },
                            _ => TableRowTint::default(),
                        })
                        .collect(),
                ),
                ..BlockTint::default()
            },
        };
        match resolve(&merged.block, path) {
            Some(Target::Block(block)) => tints.block(block, tint),
            Some(Target::Item(item)) => tints.item(item, tint),
            None => {}
        }
    }
    tints
}

/// An edited block drawn once (`side` of it), in a strip with the
/// modified bar. `options` are the side's (its pictures); for
/// [`Side::Both`] they should resolve both versions' pictures.
pub(crate) fn merged_block(
    id: SharedString,
    merged: &Merged,
    side: Side,
    options: MarkdownOptions,
    t: &GhdTheme,
    cx: &App,
) -> AnyElement {
    let options = MarkdownOptions {
        marks: None,
        table_rows: None,
        tints: Some(Rc::new(merged_tints(merged, side, t))),
        ..options
    };
    strip(
        markdown_with(id, std::slice::from_ref(&merged.block), &options, cx).into_any_element(),
        BlockStatus::Modified,
        t,
    )
    .into_any_element()
}

/// `first`'s pictures, then `second`'s.
pub(crate) fn either_images(first: &ImageResolver, second: &ImageResolver) -> ImageResolver {
    let (first, second) = (first.clone(), second.clone());
    Rc::new(move |src: &str| first(src).or_else(|| second(src)))
}

/// Pictures a version of the document names, drawn from its own bytes.
fn resolver(blob: Option<&LatexBlob>) -> ImageResolver {
    let images: Vec<(String, Arc<Image>)> = blob
        .map(|b| {
            b.images
                .iter()
                .filter_map(|(name, image)| {
                    let format = match image.media_type.as_str() {
                        "image/png" => ImageFormat::Png,
                        "image/jpg" | "image/jpeg" => ImageFormat::Jpeg,
                        "image/gif" => ImageFormat::Gif,
                        "image/webp" => ImageFormat::Webp,
                        "image/bmp" => ImageFormat::Bmp,
                        "image/svg+xml" => ImageFormat::Svg,
                        _ => return None,
                    };
                    Some((
                        name.clone(),
                        Arc::new(Image::from_bytes(format, image.bytes.clone())),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    Rc::new(move |src: &str| {
        images
            .iter()
            .find(|(name, _)| {
                name == src || name.trim_start_matches("./") == src.trim_start_matches("./")
            })
            .map(|(_, i)| i.clone())
    })
}

struct Rows {
    content: Rc<Content>,
    images: [ImageResolver; 2],
    split: bool,
    view: Rc<Vec<FoldRow>>,
    this: WeakEntity<LatexDiff>,
}

impl Rows {
    fn list_row(&self, ix: usize, cx: &App) -> AnyElement {
        match self.view.get(ix) {
            Some(FoldRow::Item(i)) => self.row(*i, cx),
            Some(FoldRow::Folded { start, end }) => {
                let (start, this) = (*start, self.this.clone());
                let noun = match &*self.content {
                    Content::Document { .. } => ("block", "blocks"),
                    Content::Bibliography { .. } => ("entry", "entries"),
                };
                crate::unchanged_fold::fold_row(
                    ("latex-fold", start),
                    end - start,
                    noun,
                    false,
                    move |_, cx| {
                        this.update(cx, |v, cx| v.expand_fold(start, cx)).ok();
                    },
                    cx,
                )
            }
            None => div().into_any_element(),
        }
    }

    fn row(&self, ix: usize, cx: &App) -> AnyElement {
        let t = cx.ghd();
        let body = match &*self.content {
            Content::Document {
                old,
                new,
                rows,
                macros,
                ..
            } => {
                let Some(row) = rows.get(ix) else {
                    return div().into_any_element();
                };
                let opts = |side: usize, marks: &[std::ops::Range<usize>]| MarkdownOptions {
                    macros: Some(macros.clone()),
                    marks: (!marks.is_empty()).then(|| {
                        let marks: Rc<[std::ops::Range<usize>]> = marks.to_vec().into();
                        let bg = if side == 0 {
                            t.diff_delete_inner_background
                        } else {
                            t.diff_add_inner_background
                        };
                        (marks, bg)
                    }),
                    images: Some(self.images[side].clone()),
                    highlight_code: true,
                    font_size: Some(13.),
                    ..MarkdownOptions::default()
                };
                let draw = |side: usize, block: &Block, status: BlockStatus, marks: &[_]| {
                    let id = SharedString::from(format!("latex-{ix}-{side}"));
                    strip(
                        markdown_with(id, std::slice::from_ref(block), &opts(side, marks), cx)
                            .into_any_element(),
                        status,
                        t,
                    )
                    .into_any_element()
                };
                let old_el = |status| {
                    row.previous
                        .and_then(|i| old.get(i))
                        .map(|b| draw(0, b, status, &row.marks.0))
                };
                let new_el = |status| {
                    row.current
                        .and_then(|i| new.get(i))
                        .map(|b| draw(1, b, status, &row.marks.1))
                };
                let column = |el: Option<AnyElement>| div().flex_1().min_w_0().children(el);
                // an edited block drawn once
                let merged = row.merged.as_deref().map(|views| {
                    let id = |side: &str| SharedString::from(format!("latex-{ix}-{side}"));
                    if self.split {
                        div()
                            .w_full()
                            .flex()
                            .flex_row()
                            .child(column(Some(merged_block(
                                id("old"),
                                &views.old,
                                Side::Old,
                                opts(0, &[]),
                                t,
                                cx,
                            ))))
                            .child(column(Some(merged_block(
                                id("new"),
                                &views.new,
                                Side::New,
                                opts(1, &[]),
                                t,
                                cx,
                            ))))
                    } else {
                        let opts = MarkdownOptions {
                            images: Some(either_images(&self.images[1], &self.images[0])),
                            ..opts(1, &[])
                        };
                        div().w_full().child(merged_block(
                            id("both"),
                            &views.both,
                            Side::Both,
                            opts,
                            t,
                            cx,
                        ))
                    }
                });
                match (merged, row.status, self.split) {
                    (Some(merged), ..) => merged,
                    (None, BlockStatus::Same, false) => {
                        div().w_full().children(new_el(BlockStatus::Same))
                    }
                    (None, BlockStatus::Same, true) => div()
                        .w_full()
                        .flex()
                        .flex_row()
                        .child(column(old_el(BlockStatus::Same)))
                        .child(column(new_el(BlockStatus::Same))),
                    (None, _, true) => div()
                        .w_full()
                        .flex()
                        .flex_row()
                        .child(column(old_el(BlockStatus::Deleted)))
                        .child(column(new_el(BlockStatus::Added))),
                    (None, _, false) => div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .children(old_el(BlockStatus::Deleted))
                        .children(new_el(BlockStatus::Added)),
                }
            }
            Content::Bibliography { old, new, changes } => {
                let Some(change) = changes.get(ix) else {
                    return div().into_any_element();
                };
                bib_row(ix, change, old, new, cx)
            }
        };
        div()
            .id(("latex-row", ix))
            .w_full()
            .flex()
            .justify_center()
            .py(zpx(3.))
            .child(
                div()
                    .w_full()
                    .when(!self.split, |d| d.max_w(zpx(MAX_TEXT_WIDTH)))
                    .child(body),
            )
            .into_any_element()
    }
}

/// An entry of a bibliography: its reference, and the fields that changed.
fn bib_row(ix: usize, change: &BibChange, old: &[BibEntry], new: &[BibEntry], cx: &App) -> Div {
    let t = cx.ghd();
    let (entry, status, fields) = match change {
        BibChange::Same(_, j) => (new.get(*j), BlockStatus::Same, None),
        BibChange::Added(j) => (new.get(*j), BlockStatus::Added, None),
        BibChange::Removed(i) => (old.get(*i), BlockStatus::Deleted, None),
        BibChange::Changed { new: j, fields, .. } => {
            (new.get(*j), BlockStatus::Modified, Some(fields))
        }
    };
    let Some(entry) = entry else {
        return div();
    };
    let reference = crate::markdown::rich_text(
        SharedString::from(format!("bib-{ix}")),
        &bibtex::reference(entry),
        cx,
    );
    let head = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(SPACING_HALF())
        .text_size(FONT_SIZE_XS())
        .text_color(t.text_secondary)
        .child(
            div()
                .font_family(crate::theme::mono_font())
                .child(format!("@{}{{{}}}", entry.kind, entry.key)),
        )
        .when(status == BlockStatus::Modified, |d| {
            d.child(div().text_color(t.color_modified).child("changed"))
        });
    let table = fields.filter(|f| !f.is_empty()).map(|fields| {
        div()
            .mt(zpx(4.))
            .w_full()
            .flex()
            .flex_col()
            .border_1()
            .border_color(t.diff_border)
            .text_size(FONT_SIZE_SM())
            .children(fields.iter().enumerate().map(|(k, f)| {
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .when(k > 0, |d| d.border_t_1().border_color(t.diff_border))
                    .child(
                        div()
                            .flex_none()
                            .w(zpx(90.))
                            .px(zpx(6.))
                            .bg(t.diff_gutter_background)
                            .text_color(t.text_secondary)
                            .child(f.name.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .children(f.old.as_ref().map(|v| {
                                div()
                                    .px(zpx(6.))
                                    .bg(t.diff_delete_background)
                                    .child(bibtex::display_field(v))
                            }))
                            .children(f.new.as_ref().map(|v| {
                                div()
                                    .px(zpx(6.))
                                    .bg(t.diff_add_background)
                                    .child(bibtex::display_field(v))
                            })),
                    )
            }))
    });
    strip(
        div()
            .flex()
            .flex_col()
            .gap(zpx(2.))
            .child(head)
            .child(div().text_size(FONT_SIZE()).child(reference))
            .children(table)
            .into_any_element(),
        match status {
            BlockStatus::Modified => BlockStatus::Same,
            s => s,
        },
        t,
    )
    .when(status == BlockStatus::Modified, |d| {
        d.border_l_2().border_color(t.color_modified)
    })
}

impl Render for LatexDiff {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd().clone();
        let split = corvene_core::AppState::try_global(cx)
            .is_some_and(|s| s.read(cx).settings.show_side_by_side_diff);
        let summary = self.summary(cx);
        let body = match self.content.clone() {
            None => div().flex_1().into_any_element(),
            Some(content) => {
                let shared = Rc::new(Rows {
                    content,
                    images: self.images.clone(),
                    split,
                    view: self.view.clone(),
                    this: self.weak.clone(),
                });
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(
                        list(self.list.clone(), move |ix, _window, cx| {
                            shared.list_row(ix, cx)
                        })
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .pt(SPACING_HALF())
                        .pb(SPACING())
                        .pr(gutter(&self.list)),
                    )
                    .child(scrollbar("latex-scrollbar", self.list.clone()))
                    .into_any_element()
            }
        };
        div()
            .id("latex-diff")
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(t.background)
            .text_color(t.text)
            .child(summary)
            .child(body)
    }
}
