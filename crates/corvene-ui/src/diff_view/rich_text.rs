//! Corvene `1331-notebook-diff` / `1332-latex-diff`: the diff view's side
//! of notebook and LaTeX diffs. The diff's `source` is git's own text diff;
//! the header's switch shows the rendered view (an entity of its own), a
//! derived text (a notebook's Script view) or that `source` (JSON / Text,
//! stageable line by line like any text diff).
//!
//! GitHub Desktop shows both as text diffs (`app/src/ui/diff/index.tsx`).
//!
//! Corvene `1334-markdown-preview-diff`: a Markdown file's Source /
//! Preview switch works the same way (Source is git's diff; Preview the
//! LaTeX view over the Markdown blocks).

use std::cell::RefCell;
use std::sync::Arc;

use corvene_core::{
    Diff, Dispatcher, LatexDiffView as LatexView, MarkdownDiffView as MarkdownView,
    NotebookDiffView as NotebookView,
};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{DiffSource, DiffView, PdfText, Snapshot, view_switch_n};
use crate::latex_diff::LatexDiff;
use crate::notebook_diff::{NotebookContext, NotebookDiffView};

/// Flipped into the diff generation of the replacement views, so their rows
/// are built apart from each other's.
const JSON_GENERATION: u64 = 1 << 60;
const SCRIPT_GENERATION: u64 = 1 << 59;
const LATEX_PAGES_GENERATION: u64 = 1 << 58;

/// A LaTeX diff and its pages (or why they could not be drawn).
type PagesOf = (Arc<Diff>, Result<Arc<Diff>, String>);

/// The diff view's state for notebook and LaTeX diffs.
#[derive(Default)]
pub(super) struct RichPane {
    /// The notebook or LaTeX diff last shown (any view).
    source: Option<Arc<Diff>>,
    notebook: Option<Entity<NotebookDiffView>>,
    latex: Option<Entity<LatexDiff>>,
    /// git's diff of the shown file as its own `Arc` (for the JSON / Text
    /// view), with the diff it came from.
    text: RefCell<Option<(Arc<Diff>, Arc<Diff>)>>,
    /// A notebook's Script view.
    script: Option<(Arc<Diff>, PdfText)>,
    _script_task: Option<Task<()>>,
    /// A LaTeX document's pages.
    pages: Option<PagesOf>,
    _pages_task: Option<Task<()>>,
    pages_running: bool,
}

/// A notebook or LaTeX diff.
pub(super) fn is_rich(diff: &Diff) -> bool {
    matches!(
        diff,
        Diff::Notebook { .. } | Diff::Latex { .. } | Diff::Markdown { .. }
    )
}

fn source_of(diff: &Diff) -> Option<&Diff> {
    match diff {
        Diff::Notebook { source, .. }
        | Diff::Latex { source, .. }
        | Diff::Markdown { source, .. } => Some(source),
        _ => None,
    }
}

/// `1332-latex-diff`: whether a TeX engine can draw a document's pages
/// (and the PDF page view to show them with).
fn latex_pages_available(s: &corvene_core::AppState) -> bool {
    s.flags.bool(corvene_core::flags::ids::PDF_DIFF)
        && corvene_core::latex_pages::engine().is_some()
}

/// The view each kind shows (Pages falls back to Rendered).
fn effective(diff: &Diff, s: &corvene_core::AppState) -> Effective {
    match diff {
        Diff::Notebook { .. } if !s.flags.bool(corvene_core::flags::ids::NOTEBOOK_DIFF) => {
            Effective::Text
        }
        Diff::Notebook { .. } => match s.settings.notebook_diff_view {
            NotebookView::Notebook => Effective::Rendered,
            NotebookView::Script => Effective::Script,
            NotebookView::Json => Effective::Text,
        },
        Diff::Latex { .. } if !s.flags.bool(corvene_core::flags::ids::LATEX_DIFF) => {
            Effective::Text
        }
        Diff::Latex { kind, .. } => match s.settings.latex_diff_view {
            LatexView::Rendered => Effective::Rendered,
            LatexView::Text => Effective::Text,
            LatexView::Pages
                if *kind == corvene_core::LatexKind::Document && latex_pages_available(s) =>
            {
                Effective::Pages
            }
            LatexView::Pages => Effective::Rendered,
        },
        Diff::Markdown { .. }
            if !s
                .flags
                .bool(corvene_core::flags::ids::MARKDOWN_PREVIEW_DIFF) =>
        {
            Effective::Text
        }
        Diff::Markdown { .. } => match s.settings.markdown_diff_view {
            MarkdownView::Source => Effective::Text,
            MarkdownView::Preview => Effective::Rendered,
        },
        _ => Effective::Rendered,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Effective {
    Rendered,
    Text,
    Script,
    Pages,
}

/// The header's switch while the diff is a notebook or LaTeX diff.
pub(super) fn rich_switch(view: &Entity<DiffView>, cx: &App) -> Option<impl IntoElement + use<>> {
    let s = AppState::try_global(cx)?.read(cx);
    let source = view.read(cx).rich.source.clone()?;
    if view.read(cx).stacked_path.is_some() {
        return None;
    }
    match &*source {
        Diff::Notebook { .. } => {
            if !s.flags.bool(corvene_core::flags::ids::NOTEBOOK_DIFF) {
                return None;
            }
            let values = [
                NotebookView::Notebook,
                NotebookView::Script,
                NotebookView::Json,
            ];
            let selected = values
                .iter()
                .position(|v| *v == s.settings.notebook_diff_view)
                .unwrap_or(0);
            Some(view_switch_n(
                "notebook-view-switch",
                &[
                    "notebook-view-cells",
                    "notebook-view-script",
                    "notebook-view-json",
                ],
                &["Notebook", "Script", "JSON"],
                selected,
                move |ix, cx| Dispatcher::set_notebook_diff_view(values[ix], cx),
                cx,
            ))
        }
        Diff::Latex { kind, .. } => {
            if !s.flags.bool(corvene_core::flags::ids::LATEX_DIFF) {
                return None;
            }
            let mut values = vec![LatexView::Rendered, LatexView::Text];
            let mut labels = vec!["Rendered", "Text"];
            let mut ids = vec!["latex-view-rendered", "latex-view-text"];
            if *kind == corvene_core::LatexKind::Document && latex_pages_available(s) {
                values.push(LatexView::Pages);
                labels.push("Pages");
                ids.push("latex-view-pages");
            }
            let current = match effective(&source, s) {
                Effective::Text => LatexView::Text,
                Effective::Pages => LatexView::Pages,
                _ => LatexView::Rendered,
            };
            let selected = values.iter().position(|v| *v == current).unwrap_or(0);
            Some(view_switch_n(
                "latex-view-switch",
                &ids,
                &labels,
                selected,
                move |ix, cx| Dispatcher::set_latex_diff_view(values[ix], cx),
                cx,
            ))
        }
        Diff::Markdown { .. } => {
            if !s
                .flags
                .bool(corvene_core::flags::ids::MARKDOWN_PREVIEW_DIFF)
            {
                return None;
            }
            let values = [MarkdownView::Source, MarkdownView::Preview];
            let selected = values
                .iter()
                .position(|v| *v == s.settings.markdown_diff_view)
                .unwrap_or(0);
            Some(view_switch_n(
                "markdown-view-switch",
                &["markdown-view-source", "markdown-view-preview"],
                &["Source", "Preview"],
                selected,
                move |ix, cx| Dispatcher::set_markdown_diff_view(values[ix], cx),
                cx,
            ))
        }
        _ => None,
    }
}

use corvene_core::AppState;

impl DiffView {
    /// The view that replaces the notebook or LaTeX diff `diff` (with the
    /// generation bit it is keyed by, and whether the file's lines still
    /// apply), once ready; `None` for the rendered view.
    pub(super) fn rich_view_of(
        &self,
        diff: &Arc<Diff>,
        cx: &App,
    ) -> Option<(Arc<Diff>, u64, bool)> {
        if !is_rich(diff) {
            return None;
        }
        let s = self.state.read(cx);
        // a stack shows the file's own diff
        let view = if self.stacked_path.is_some() {
            Effective::Text
        } else {
            effective(diff, s)
        };
        match view {
            Effective::Rendered => None,
            Effective::Text => {
                let mut cache = self.rich.text.borrow_mut();
                let fresh = cache
                    .as_ref()
                    .is_some_and(|(from, _)| Arc::ptr_eq(from, diff));
                if !fresh {
                    let text = Arc::new(source_of(diff)?.clone());
                    *cache = Some((diff.clone(), text));
                }
                cache
                    .as_ref()
                    .map(|(_, text)| (text.clone(), JSON_GENERATION, true))
            }
            Effective::Script => match self.rich.script.as_ref()? {
                (from, PdfText::Ready(text)) if Arc::ptr_eq(from, diff) => {
                    Some((text.clone(), SCRIPT_GENERATION, false))
                }
                _ => None,
            },
            Effective::Pages => match self.rich.pages.as_ref()? {
                (from, Ok(pages)) if Arc::ptr_eq(from, diff) => {
                    Some((pages.clone(), LATEX_PAGES_GENERATION, false))
                }
                _ => None,
            },
        }
    }

    /// Starts the Script view or the pages of the shown diff when asked.
    pub(super) fn request_rich_view(&mut self, snap: &Snapshot, cx: &mut Context<Self>) {
        self.rich.source = snap.rich_source.clone();
        let Some(source) = snap.rich_source.clone() else {
            return;
        };
        if self.stacked_path.is_some() {
            return;
        }
        let fresh = |s: Option<&Arc<Diff>>| s.is_some_and(|s| Arc::ptr_eq(s, &source));
        match effective(&source, self.state.read(cx)) {
            Effective::Script if !fresh(self.rich.script.as_ref().map(|(s, _)| s)) => {
                self.rich.script = Some((source.clone(), PdfText::Reading));
                let job = source.clone();
                let task = cx.background_executor().spawn(async move {
                    let Diff::Notebook {
                        previous, current, ..
                    } = &*job
                    else {
                        return None;
                    };
                    Some(corvene_core::notebook_diff::script::script_diff(
                        previous.as_deref(),
                        current.as_deref(),
                    ))
                });
                self.rich._script_task = Some(cx.spawn(async move |this, cx| {
                    let diff = task.await;
                    this.update(cx, |this, cx| {
                        if this
                            .rich
                            .script
                            .as_ref()
                            .is_some_and(|(s, _)| Arc::ptr_eq(s, &source))
                        {
                            this.rich.script = Some((
                                source,
                                diff.map_or(PdfText::Unreadable, |d| PdfText::Ready(Arc::new(d))),
                            ));
                            cx.notify();
                        }
                    })
                    .ok();
                }));
            }
            Effective::Pages
                if !fresh(self.rich.pages.as_ref().map(|(s, _)| s)) && !self.rich.pages_running =>
            {
                self.rich.pages_running = true;
                let job = source.clone();
                let (repo_path, path, kind) =
                    (snap.repo_path.clone(), snap.path.clone(), snap.kind);
                let task = cx.background_executor().spawn(async move {
                    corvene_core::latex_pages::pages_diff(&repo_path, &path, kind, &job)
                });
                self.rich._pages_task = Some(cx.spawn(async move |this, cx| {
                    let pages = task.await;
                    this.update(cx, |this, cx| {
                        this.rich.pages_running = false;
                        this.rich.pages = Some((source, pages.map(Arc::new)));
                        cx.notify();
                    })
                    .ok();
                }));
            }
            _ => {}
        }
    }

    /// Makes (or hands the newer diff to) the rendered view.
    pub(super) fn load_rich(&mut self, snap: &Snapshot, cx: &mut Context<Self>) {
        let Some(source) = snap.rich_source.clone() else {
            self.rich.notebook = None;
            self.rich.latex = None;
            return;
        };
        match &*source {
            Diff::Notebook { .. } => {
                self.rich.latex = None;
                match self.rich.notebook.clone() {
                    Some(view) => {
                        if !Arc::ptr_eq(view.read(cx).diff(), &source) {
                            view.update(cx, |v, cx| v.set_diff(source, cx));
                        }
                    }
                    None => {
                        self.rich.notebook = Some(cx.new(|cx| NotebookDiffView::new(source, cx)));
                    }
                }
            }
            Diff::Latex { .. } => {
                self.rich.notebook = None;
                let keep = self
                    .rich
                    .latex
                    .as_ref()
                    .is_some_and(|v| Arc::ptr_eq(v.read(cx).diff(), &source));
                if !keep {
                    self.rich.latex = Some(cx.new(|cx| LatexDiff::new(source, cx)));
                }
            }
            Diff::Markdown { .. } => {
                self.rich.notebook = None;
                let keep = self
                    .rich
                    .latex
                    .as_ref()
                    .is_some_and(|v| Arc::ptr_eq(v.read(cx).diff(), &source));
                if !keep {
                    let path = snap.path.clone();
                    self.rich.latex = Some(cx.new(|cx| LatexDiff::markdown(source, &path, cx)));
                }
            }
            _ => {}
        }
    }

    /// The rendered view, or where the Script view or pages stand while
    /// they are not ready.
    pub(super) fn rich_panel(&mut self, snap: &Snapshot, cx: &mut Context<Self>) -> AnyElement {
        if self.stacked_path.is_some() {
            return self.panel("This file changed. Select it alone to see it rendered.", cx);
        }
        let Some(source) = snap.rich_source.clone() else {
            return self.panel("This file has changed.", cx);
        };
        match effective(&source, self.state.read(cx)) {
            Effective::Rendered => match &*source {
                Diff::Notebook { .. } => match self.rich.notebook.clone() {
                    Some(view) => {
                        let context = Some(NotebookContext {
                            repo: snap.repo,
                            path: snap.path.clone(),
                            working: self.source == DiffSource::WorkingDirectory,
                        });
                        view.update(cx, |v, _| v.set_context(context));
                        view.into_any_element()
                    }
                    None => self.panel("Reading the notebook…", cx),
                },
                _ => match self.rich.latex.clone() {
                    Some(view) => view.into_any_element(),
                    None => self.panel("Reading the document…", cx),
                },
            },
            Effective::Script => match self.rich.script.as_ref().map(|(_, t)| t) {
                Some(PdfText::Unreadable) => self.panel("The notebook could not be read.", cx),
                _ => self.panel("Writing the notebook as a script…", cx),
            },
            Effective::Pages => match self.rich.pages.as_ref() {
                Some((from, Err(reason))) if Arc::ptr_eq(from, &source) => {
                    let reason = reason.clone();
                    self.panel(reason, cx)
                }
                _ => self.panel("Compiling both versions of the document…", cx),
            },
            Effective::Text => self.panel("This file has changed.", cx),
        }
    }
}
