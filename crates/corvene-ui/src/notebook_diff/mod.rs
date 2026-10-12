//! Corvene `1331-notebook-diff`: a Jupyter notebook's diff drawn as the
//! notebook. Cells are paired as nbdime pairs them
//! (`corvene_core::notebook_diff`); code is highlighted with its changed
//! words marked, markdown is rendered (math typeset) with its changed
//! paragraphs tinted and marked, and outputs are drawn and compared: images
//! in the image diff (2-up, swipe, onion skin, difference), ANSI streams and
//! tracebacks in colour, pandas tables cell by cell, LaTeX typeset, HTML,
//! JSON and text. Moved cells show where they came from.
//!
//! The toolbar's chips show or hide what the default view leaves out
//! (execution counts, metadata, unchanged cells; outputs on demand), and
//! the layout follows the diff's unified / side-by-side setting. In the
//! Changes tab each changed cell has a checkbox that selects its lines of
//! git's JSON diff for the commit (`corvene_core::notebook_stage` keeps
//! the staged file valid JSON) and a Discard button.
//!
//! GitHub Desktop has no notebook diff: an `.ipynb` file is its JSON text
//! diff (`app/src/ui/diff/text-diff.tsx`; desktop#5279, desktop#12121,
//! desktop#17728). Find (⌘F) searches the JSON view only.

mod cell;
mod outputs;
mod source;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use corvene_core::notebook_diff::{
    self as nd, CellStatus, NotebookDiff, NotebookFilters, Row as NbRow,
};
use corvene_core::{Diff, DiffHunk, DiffSelection, Dispatcher, NotebookBlob};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::icons::{Octicon, octicon};
use crate::math::SharedMacros;
use crate::scrollbar::{gutter, scrollbar};
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::{GhdTooltip, IconButtonA11y};

/// Where the diff is shown: what staging needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotebookContext {
    pub repo: u64,
    pub path: String,
    /// The Changes tab (cells can be staged and discarded).
    pub working: bool,
}

/// What staging a cell needs: the text git's diff starts from, its hunks,
/// and each pair's lines in it.
struct Stage {
    repo: u64,
    path: String,
    old_text: Arc<str>,
    hunks: Arc<Vec<DiffHunk>>,
    /// Selection indices of each pair's lines (by pair index).
    pair_lines: Vec<Vec<u32>>,
    /// Lines outside every cell (notebook metadata, structure).
    outside: Vec<u32>,
}

/// View state that outlives a render: toggles and caches, shared with the
/// list's row callback.
#[derive(Default)]
pub(crate) struct UiState {
    /// Markdown cells showing their source instead of the rendering.
    pub md_source: HashSet<usize>,
    /// Pairs whose hidden changes were revealed.
    pub revealed: HashSet<usize>,
    /// The rendering picked for an output (pair, output pair) → index.
    pub output_choice: HashMap<(usize, usize), usize>,
    /// Outputs shown in full (pair, output pair).
    pub full_output: HashSet<(usize, usize)>,
    /// Folded unchanged lines opened: (pair, part, first line).
    pub open_lines: HashSet<(usize, u8, usize)>,
    /// Highlight spans by (extension, source) hash.
    pub highlights: HashMap<u64, Rc<Vec<Vec<corvene_highlight::Span>>>>,
    /// Decoded output images by payload hash (`None`: unreadable).
    pub images: HashMap<u64, Option<Arc<Image>>>,
    /// Image diffs of changed image outputs (pair, output pair).
    pub image_diffs: HashMap<(usize, usize), Entity<crate::image_diff::ImageDiff>>,
}

pub struct NotebookDiffView {
    diff: Arc<Diff>,
    aligned: Option<Rc<NotebookDiff>>,
    _align_task: Option<Task<()>>,
    rows: Rc<Vec<NbRow>>,
    /// The filters and folds `rows` were built for.
    rows_for: Option<(NotebookFilters, usize)>,
    expanded: HashSet<usize>,
    list: ListState,
    ui: Rc<RefCell<UiState>>,
    macros: SharedMacros,
    context: Option<NotebookContext>,
    stage: Option<Rc<Stage>>,
    /// A short message under the toolbar (a cell that cannot be staged
    /// alone).
    notice: Option<SharedString>,
}

/// The notebooks of a `Diff::Notebook`.
pub(crate) fn sides(diff: &Diff) -> (Option<&NotebookBlob>, Option<&NotebookBlob>) {
    match diff {
        Diff::Notebook {
            previous, current, ..
        } => (previous.as_deref(), current.as_deref()),
        _ => (None, None),
    }
}

/// Whether two notebook diffs hold the same files.
fn same_files(a: &Diff, b: &Diff) -> bool {
    let (ap, ac) = sides(a);
    let (bp, bc) = sides(b);
    let raw = |n: Option<&NotebookBlob>| n.map(|n| n.raw.clone());
    raw(ap) == raw(bp) && raw(ac) == raw(bc)
}

impl NotebookDiffView {
    /// `diff` is a `Diff::Notebook`.
    pub fn new(diff: Arc<Diff>, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            diff: diff.clone(),
            aligned: None,
            _align_task: None,
            rows: Rc::new(Vec::new()),
            rows_for: None,
            expanded: HashSet::new(),
            list: ListState::new(0, ListAlignment::Top, zpx(600.)),
            ui: Rc::new(RefCell::new(UiState::default())),
            macros: SharedMacros::default(),
            context: None,
            stage: None,
            notice: None,
        };
        this.set_diff(diff, cx);
        this
    }

    /// The notebook diff shown.
    pub fn diff(&self) -> &Arc<Diff> {
        &self.diff
    }

    /// A newer diff of the file: aligned again unless the files are the
    /// same (the view keeps its place and toggles).
    pub fn set_diff(&mut self, diff: Arc<Diff>, cx: &mut Context<Self>) {
        let same = self.aligned.is_some() && same_files(&self.diff, &diff);
        self.diff = diff.clone();
        if same {
            self.rebuild_stage();
            return;
        }
        // the pairs are the old diff's until the new alignment arrives
        self.stage = None;
        {
            let mut ui = self.ui.borrow_mut();
            ui.image_diffs.clear();
        }
        let (previous, current) = sides(&diff);
        self.macros = Arc::new(corvene_math::Macros::collect(&nd::math_macros_source(
            previous, current,
        )));
        let job = diff.clone();
        let task = cx.background_executor().spawn(async move {
            let (previous, current) = sides(&job);
            nd::diff(previous, current)
        });
        self._align_task = Some(cx.spawn(async move |this, cx| {
            let aligned = task.await;
            this.update(cx, |this, cx| {
                if Arc::ptr_eq(&this.diff, &diff) {
                    let first = this.aligned.is_none();
                    this.aligned = Some(Rc::new(aligned));
                    this.rows_for = None;
                    this.rebuild_stage();
                    if first {
                        this.list.scroll_to(ListOffset::default());
                    }
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// Where the view is shown (set by the diff view on every render).
    pub fn set_context(&mut self, context: Option<NotebookContext>) {
        if self.context != context {
            self.context = context;
            self.rebuild_stage();
        }
    }

    fn rebuild_stage(&mut self) {
        self.stage = None;
        let Some(ctx) = self.context.clone().filter(|c| c.working) else {
            return;
        };
        let Some(aligned) = self.aligned.clone() else {
            return;
        };
        let Diff::Notebook { source, .. } = &*self.diff else {
            return;
        };
        let Some(hunks) = source.hunks() else {
            return;
        };
        let (previous, current) = sides(&self.diff);
        let old_text: Arc<str> = previous.map_or_else(|| Arc::from(""), |p| p.raw.clone());
        let span = |nb: Option<&NotebookBlob>, i: Option<usize>| {
            i.and_then(|i| nb?.cells.get(i)).map(|c| c.lines)
        };
        let pair_lines = aligned
            .pairs
            .iter()
            .map(|p| {
                let old = match p.status {
                    // a move's lines are on both pairs; the moved cell takes
                    // them all
                    CellStatus::MovedAway { .. } => return Vec::new(),
                    _ => span(previous, p.previous),
                };
                corvene_core::notebook_stage::span_lines(hunks, old, span(current, p.current))
            })
            .collect();
        let spans = |nb: Option<&NotebookBlob>| -> Vec<(u32, u32)> {
            nb.map(|n| n.cells.iter().map(|c| c.lines).collect())
                .unwrap_or_default()
        };
        let outside =
            corvene_core::notebook_stage::outside_lines(hunks, &spans(previous), &spans(current));
        self.stage = Some(Rc::new(Stage {
            repo: ctx.repo,
            path: ctx.path,
            old_text,
            hunks: Arc::new(hunks.to_vec()),
            pair_lines,
            outside,
        }));
    }

    fn filters(cx: &App) -> NotebookFilters {
        corvene_core::AppState::try_global(cx)
            .map(|s| s.read(cx).settings.notebook_filters)
            .unwrap_or_default()
    }

    fn ensure_rows(&mut self, filters: NotebookFilters) {
        let Some(aligned) = self.aligned.clone() else {
            return;
        };
        let key = (filters, self.expanded.len());
        if self.rows_for == Some(key) {
            return;
        }
        self.rows_for = Some(key);
        let rows = nd::rows(&aligned, &filters, &self.expanded);
        self.list.reset(rows.len());
        self.rows = Rc::new(rows);
        if self.stage.is_none() {
            self.rebuild_stage();
        }
    }

    pub(crate) fn expand_fold(&mut self, start: usize, cx: &mut Context<Self>) {
        self.expanded.insert(start);
        self.rows_for = None;
        cx.notify();
    }

    /// Next or previous changed cell from the top of the view.
    fn step_change(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(aligned) = self.aligned.clone() else {
            return;
        };
        let filters = Self::filters(cx);
        let change = |r: &NbRow| match r {
            NbRow::Cell(i) => aligned.pairs[*i].is_change(&filters),
            NbRow::Header => true,
            NbRow::Folded { .. } => false,
        };
        let top = self.list.logical_scroll_top().item_ix;
        let rows = self.rows.clone();
        let found = if forward {
            (top + 1..rows.len()).find(|&i| change(&rows[i]))
        } else {
            (0..top).rev().find(|&i| change(&rows[i]))
        };
        if let Some(ix) = found {
            self.list.scroll_to(ListOffset {
                item_ix: ix,
                offset_in_item: px(0.),
            });
            cx.notify();
        }
    }

    /// A cell's checkbox: its lines selected for the commit, or not.
    pub(crate) fn toggle_pair(&mut self, pair: usize, cx: &mut Context<Self>) {
        let Some(stage) = self.stage.clone() else {
            return;
        };
        let lines = stage.pair_lines.get(pair).cloned().unwrap_or_default();
        self.select_lines(&stage, lines, cx);
    }

    /// The notebook-level checkbox (metadata and structure lines).
    pub(crate) fn toggle_outside(&mut self, cx: &mut Context<Self>) {
        let Some(stage) = self.stage.clone() else {
            return;
        };
        let lines = stage.outside.clone();
        self.select_lines(&stage, lines, cx);
    }

    fn select_lines(&mut self, stage: &Stage, lines: Vec<u32>, cx: &mut Context<Self>) {
        if lines.is_empty() {
            return;
        }
        let selection = file_selection(stage.repo, &stage.path, cx);
        let all = selection.as_ref().is_some_and(|s| {
            corvene_core::notebook_stage::lines_kind(s, &lines)
                == corvene_core::DiffSelectionType::All
        });
        let ok = Dispatcher::set_notebook_cell_selected(
            stage.repo,
            stage.path.clone(),
            stage.old_text.clone(),
            lines,
            !all,
            cx,
        );
        self.notice = (!ok).then(|| {
            SharedString::from(
                "This change cannot be committed on its own without breaking the notebook's \
                 JSON. Select the cell next to it too, or use the JSON view.",
            )
        });
        cx.notify();
    }

    /// Discard: the cell's change reversed in the working copy (after the
    /// usual confirmation).
    pub(crate) fn discard_pair(&mut self, pair: usize, cx: &mut Context<Self>) {
        let Some(stage) = self.stage.clone() else {
            return;
        };
        let lines = stage.pair_lines.get(pair).cloned().unwrap_or_default();
        if lines.is_empty() {
            return;
        }
        match corvene_core::notebook_stage::select_cell(
            &stage.old_text,
            &stage.hunks,
            &DiffSelection::none(),
            &lines,
            true,
        ) {
            Ok(selection) => {
                Dispatcher::request_discard_selection(
                    stage.repo,
                    stage.path.clone(),
                    selection,
                    cx,
                );
                self.notice = None;
            }
            Err(_) => {
                self.notice = Some(
                    "This change cannot be discarded on its own without breaking the notebook's \
                     JSON."
                        .into(),
                );
            }
        }
        cx.notify();
    }

    pub(crate) fn toggle_md_source(&mut self, pair: usize, cx: &mut Context<Self>) {
        {
            let mut ui = self.ui.borrow_mut();
            if !ui.md_source.remove(&pair) {
                ui.md_source.insert(pair);
            }
        }
        self.list.remeasure();
        cx.notify();
    }

    pub(crate) fn reveal(&mut self, pair: usize, cx: &mut Context<Self>) {
        self.ui.borrow_mut().revealed.insert(pair);
        self.list.remeasure();
        cx.notify();
    }

    pub(crate) fn pick_output(&mut self, key: (usize, usize), ix: usize, cx: &mut Context<Self>) {
        self.ui.borrow_mut().output_choice.insert(key, ix);
        self.list.remeasure();
        cx.notify();
    }

    pub(crate) fn show_full_output(&mut self, key: (usize, usize), cx: &mut Context<Self>) {
        self.ui.borrow_mut().full_output.insert(key);
        self.list.remeasure();
        cx.notify();
    }

    pub(crate) fn open_lines(&mut self, key: (usize, u8, usize), cx: &mut Context<Self>) {
        self.ui.borrow_mut().open_lines.insert(key);
        self.list.remeasure();
        cx.notify();
    }

    fn toolbar(&self, filters: NotebookFilters, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.ghd().clone();
        let summary = self.aligned.as_ref().map(|a| a.summary(&filters));
        let mut parts: Vec<String> = Vec::new();
        if let Some(s) = summary {
            let mut push = |n: usize, one: &str, many: &str| {
                if n > 0 {
                    parts.push(format!("{n} {}", if n == 1 { one } else { many }));
                }
            };
            push(s.modified, "cell changed", "cells changed");
            push(s.added, "added", "added");
            push(s.deleted, "removed", "removed");
            push(s.moved, "moved", "moved");
            push(s.outputs, "output changed", "outputs changed");
            push(s.hidden, "hidden change", "hidden changes");
            if parts.is_empty() {
                parts.push("No changes to the cells".into());
            }
        } else {
            parts.push("Comparing cells…".into());
        }
        let any_change = summary.is_some_and(|s| s.modified + s.added + s.deleted + s.moved > 0);
        let chip = |id: &'static str, label: &'static str, shown: bool, tip: &'static str| {
            let hover = t.box_hover_background;
            div()
                .id(id)
                .a11y_button(label)
                .flex_none()
                .h(zpx(19.))
                .px(SPACING_HALF())
                .flex()
                .flex_row()
                .items_center()
                .gap(zpx(3.))
                .rounded(zpx(10.))
                .border_1()
                .cursor_pointer()
                .border_color(if shown {
                    t.box_border_accent
                } else {
                    t.box_border
                })
                .when(shown, |d| {
                    d.bg(t.box_selected_active_background)
                        .text_color(t.box_selected_active_text)
                })
                .when(!shown, |d| {
                    d.text_color(t.text_secondary).hover(move |s| s.bg(hover))
                })
                .child(octicon(
                    if shown {
                        Octicon::Eye
                    } else {
                        Octicon::EyeClosed
                    },
                    if shown {
                        t.box_selected_active_text
                    } else {
                        t.text_secondary
                    },
                ))
                .child(label)
                .ghd_tooltip(tip)
        };
        let set = move |edit: fn(&mut NotebookFilters)| {
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                let mut f = Self::filters(cx);
                edit(&mut f);
                Dispatcher::set_notebook_filters(f, cx);
            }
        };
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
        let kernel = self.aligned.as_ref().and_then(|a| a.kernel.clone());
        div()
            .flex_none()
            .w_full()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(t.diff_border)
            .child(
                div()
                    .id("notebook-toolbar")
                    .w_full()
                    .px(SPACING())
                    .py(SPACING_HALF())
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(SPACING_HALF())
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(
                        div()
                            .flex_1()
                            .min_w(zpx(160.))
                            .truncate()
                            .child(parts.join(" · ")),
                    )
                    .child(
                        chip(
                            "notebook-show-counts",
                            "Execution counts",
                            !filters.hide_execution_counts,
                            "Show execution count changes",
                        )
                        .on_click(set(|f| f.hide_execution_counts = !f.hide_execution_counts)),
                    )
                    .child(
                        chip(
                            "notebook-show-metadata",
                            "Metadata",
                            !filters.hide_metadata,
                            "Show metadata changes",
                        )
                        .on_click(set(|f| f.hide_metadata = !f.hide_metadata)),
                    )
                    .child(
                        chip(
                            "notebook-show-outputs",
                            "Outputs",
                            !filters.hide_outputs,
                            "Show outputs",
                        )
                        .on_click(set(|f| f.hide_outputs = !f.hide_outputs)),
                    )
                    .child(
                        chip(
                            "notebook-show-unchanged",
                            "Unchanged cells",
                            !filters.collapse_unchanged,
                            "Show the cells that did not change",
                        )
                        .on_click(set(|f| f.collapse_unchanged = !f.collapse_unchanged)),
                    )
                    .child(
                        button(
                            "notebook-previous-change",
                            Octicon::ChevronUp,
                            "Previous Change",
                        )
                        .when(any_change, |d| {
                            d.on_click(cx.listener(|this, _, _, cx| this.step_change(false, cx)))
                        }),
                    )
                    .child(
                        button("notebook-next-change", Octicon::ChevronDown, "Next Change").when(
                            any_change,
                            |d| {
                                d.on_click(cx.listener(|this, _, _, cx| this.step_change(true, cx)))
                            },
                        ),
                    ),
            )
            .when_some(kernel, |d, (old, new)| {
                d.child(
                    div()
                        .w_full()
                        .px(SPACING())
                        .py(zpx(3.))
                        .text_size(FONT_SIZE_SM())
                        .bg(t.file_warning_background)
                        .text_color(t.file_warning)
                        .child(format!(
                            "Kernel changed: {} → {}",
                            old.as_deref().unwrap_or("none"),
                            new.as_deref().unwrap_or("none")
                        )),
                )
            })
            .when_some(self.notice.clone(), |d, notice| {
                d.child(
                    div()
                        .w_full()
                        .px(SPACING())
                        .py(zpx(3.))
                        .text_size(FONT_SIZE_SM())
                        .bg(t.file_warning_background)
                        .text_color(t.file_warning)
                        .child(notice),
                )
            })
            .into_any_element()
    }
}

/// A working file's line selection.
fn file_selection(repo: u64, path: &str, cx: &App) -> Option<DiffSelection> {
    let s = corvene_core::AppState::try_global(cx)?.read(cx);
    let rs = s.repo_states.get(&repo)?;
    rs.status
        .as_deref()?
        .files
        .iter()
        .find(|f| f.path == path)
        .map(|f| f.selection.clone())
}

/// What the list's rows draw from.
pub(crate) struct Shared {
    pub weak: WeakEntity<NotebookDiffView>,
    pub previous: Option<Arc<NotebookBlob>>,
    pub current: Option<Arc<NotebookBlob>>,
    pub aligned: Rc<NotebookDiff>,
    pub rows: Rc<Vec<NbRow>>,
    pub filters: NotebookFilters,
    pub split: bool,
    pub ui: Rc<RefCell<UiState>>,
    pub macros: SharedMacros,
    /// Staging: the pair's lines and the file's selection.
    stage: Option<Rc<Stage>>,
    selection: Option<DiffSelection>,
    /// The notebook's file extension for highlighting (`.py`).
    pub ext: String,
}

impl Shared {
    /// A pair's checkbox: `Some(Some(all))` / `Some(None)` (some lines),
    /// `None` without staging or lines.
    pub fn checkbox(&self, pair: usize) -> Option<Option<bool>> {
        let stage = self.stage.as_ref()?;
        let lines = stage.pair_lines.get(pair)?;
        if lines.is_empty() {
            return None;
        }
        let selection = self.selection.as_ref()?;
        Some(
            match corvene_core::notebook_stage::lines_kind(selection, lines) {
                corvene_core::DiffSelectionType::All => Some(true),
                corvene_core::DiffSelectionType::None => Some(false),
                corvene_core::DiffSelectionType::Partial => None,
            },
        )
    }

    fn outside_checkbox(&self) -> Option<Option<bool>> {
        let stage = self.stage.as_ref()?;
        if stage.outside.is_empty() {
            return None;
        }
        let selection = self.selection.as_ref()?;
        Some(
            match corvene_core::notebook_stage::lines_kind(selection, &stage.outside) {
                corvene_core::DiffSelectionType::All => Some(true),
                corvene_core::DiffSelectionType::None => Some(false),
                corvene_core::DiffSelectionType::Partial => None,
            },
        )
    }

    pub fn can_discard(&self) -> bool {
        self.stage.is_some()
    }

    fn row(&self, ix: usize, cx: &mut App) -> AnyElement {
        let Some(row) = self.rows.get(ix).cloned() else {
            return div().into_any_element();
        };
        match row {
            NbRow::Header => self.header_row(cx),
            NbRow::Folded { start, end } => self.folded_row(start, end, cx),
            NbRow::Cell(i) => cell::cell_row(self, i, cx),
        }
    }

    fn folded_row(&self, start: usize, end: usize, cx: &App) -> AnyElement {
        let t = cx.ghd();
        let weak = self.weak.clone();
        let hover = t.box_hover_background;
        let n = end - start;
        div()
            .w_full()
            .px(SPACING())
            .py(zpx(3.))
            .child(
                div()
                    .id(("notebook-fold", start))
                    .a11y_button(format!("Show {n} unchanged cells"))
                    .w_full()
                    .h(zpx(22.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .gap(SPACING_HALF())
                    .rounded(BORDER_RADIUS())
                    .bg(t.diff_hunk_background)
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .child(octicon(Octicon::Unfold, t.text_secondary))
                    .child(format!(
                        "{n} unchanged {}",
                        if n == 1 { "cell" } else { "cells" }
                    ))
                    .on_click(move |_, _, cx| {
                        weak.update(cx, |this, cx| this.expand_fold(start, cx)).ok();
                    }),
            )
            .into_any_element()
    }

    fn header_row(&self, cx: &mut App) -> AnyElement {
        let t = cx.ghd().clone();
        let mut lines: Vec<String> = Vec::new();
        if let Some((old, new)) = &self.aligned.language {
            lines.push(format!("Language: {old} → {new}"));
        }
        if let Some(((a, b), (c, d))) = self.aligned.format {
            lines.push(format!("nbformat {a}.{b} → {c}.{d}"));
        }
        let metadata = (self.aligned.metadata_changed && !self.filters.hide_metadata).then(|| {
            let old = self
                .previous
                .as_ref()
                .map(|n| nd::metadata_text(&n.metadata))
                .unwrap_or_default();
            let new = self
                .current
                .as_ref()
                .map(|n| nd::metadata_text(&n.metadata))
                .unwrap_or_default();
            source::text_diff_element(self, usize::MAX, 9, &old, &new, "json", cx)
        });
        let check = self.outside_checkbox();
        let weak = self.weak.clone();
        div()
            .w_full()
            .px(SPACING())
            .py(zpx(4.))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(zpx(4.))
                    .p(SPACING_HALF())
                    .rounded(zpx(6.))
                    .border_1()
                    .border_color(t.box_border)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(SPACING_HALF())
                            .text_size(FONT_SIZE_SM())
                            .font_weight(FontWeight::SEMIBOLD)
                            .children(check.map(|state| {
                                crate::widgets::checkbox_tristate(
                                    "notebook-outside-check",
                                    state,
                                    false,
                                    cx,
                                )
                                .on_click(move |_, _, cx| {
                                    weak.update(cx, |this, cx| this.toggle_outside(cx)).ok();
                                })
                            }))
                            .child("Notebook"),
                    )
                    .children(lines.into_iter().map(|l| {
                        div()
                            .text_size(FONT_SIZE_SM())
                            .text_color(t.text_secondary)
                            .child(l)
                    }))
                    .children(metadata),
            )
            .into_any_element()
    }
}

impl Render for NotebookDiffView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd().clone();
        let filters = Self::filters(cx);
        let split = corvene_core::AppState::try_global(cx)
            .is_some_and(|s| s.read(cx).settings.show_side_by_side_diff);
        self.ensure_rows(filters);
        let toolbar = self.toolbar(filters, cx);
        let body = match self.aligned.clone() {
            None => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(t.text_secondary)
                .child("Comparing the notebook's cells…")
                .into_any_element(),
            Some(aligned) => {
                let (previous, current) = match &*self.diff {
                    Diff::Notebook {
                        previous, current, ..
                    } => (previous.clone(), current.clone()),
                    _ => (None, None),
                };
                let ext = current
                    .as_ref()
                    .or(previous.as_ref())
                    .map(|n| n.file_extension.clone())
                    .unwrap_or_else(|| ".py".into());
                let selection = self
                    .stage
                    .as_ref()
                    .and_then(|s| file_selection(s.repo, &s.path, cx));
                let shared = Rc::new(Shared {
                    weak: cx.entity().downgrade(),
                    previous,
                    current,
                    aligned,
                    rows: self.rows.clone(),
                    filters,
                    split,
                    ui: self.ui.clone(),
                    macros: self.macros.clone(),
                    stage: self.stage.clone(),
                    selection,
                    ext,
                });
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(
                        list(self.list.clone(), move |ix, _window, cx| shared.row(ix, cx))
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .pt(SPACING_HALF())
                            .pb(SPACING())
                            .pr(gutter(&self.list)),
                    )
                    .child(scrollbar("notebook-scrollbar", self.list.clone()))
                    .into_any_element()
            }
        };
        div()
            .id("notebook-diff")
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(t.background)
            .text_color(t.text)
            .child(toolbar)
            .child(body)
    }
}
