//! Syntax colours for a merge editor pane, worked out off the main thread by
//! the diff's highlighter. The result pane's adapter is also where every
//! edit (undo and redo included) reaches the chunk anchors.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use corvene_highlight::{Engine, TokenClass};
use corvene_merge::anchors::Anchors;
use gpui_kit::component::input::{
    EditorState, FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter, Rope,
};
use gpui_kit::*;

use crate::diff_view_rows::token_color;
use crate::theme::GhdTheme;

/// How long edits may pile up before highlighting again.
const HIGHLIGHT_DELAY: Duration = Duration::from_millis(60);

type Runs = Rc<RefCell<Vec<(Range<usize>, Hsla)>>>;

pub struct MergeHighlighter {
    path: String,
    engine: Engine,
    theme: GhdTheme,
    runs: Runs,
    anchors: Option<Rc<RefCell<Anchors>>>,
    task: Option<Task<()>>,
}

impl MergeHighlighter {
    pub fn new(
        path: String,
        engine: Engine,
        theme: GhdTheme,
        anchors: Option<Rc<RefCell<Anchors>>>,
    ) -> Self {
        Self {
            path,
            engine,
            theme,
            runs: Rc::default(),
            anchors,
            task: None,
        }
    }

    /// Runs after an edit keep their text until the next highlight.
    fn shift_runs(&self, edit: &InputEdit) {
        let move_past = |p: usize| {
            if p <= edit.start_byte {
                p
            } else if p >= edit.old_end_byte {
                p - edit.old_end_byte + edit.new_end_byte
            } else {
                edit.new_end_byte
            }
        };
        let mut runs = self.runs.borrow_mut();
        for (range, _) in runs.iter_mut() {
            range.start = move_past(range.start);
            range.end = move_past(range.end);
        }
        runs.retain(|(range, _)| !range.is_empty());
    }
}

/// Spans per line, as byte ranges of the whole text.
fn highlight(engine: Engine, path: &str, text: &str) -> Vec<(Range<usize>, TokenClass)> {
    let lines: Vec<&str> = text.split('\n').collect();
    let Some(spans) = corvene_highlight::highlight_lines_with(engine, path, lines.iter().copied())
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut at = 0;
    for (line, spans) in lines.iter().zip(spans) {
        for span in spans {
            out.push((span.range.start + at..span.range.end + at, span.class));
        }
        at += line.len() + 1;
    }
    out
}

impl InputHighlighter for MergeHighlighter {
    fn language(&self) -> SharedString {
        "merge".into()
    }

    fn update(
        &mut self,
        edit: Option<InputEdit>,
        text: &Rope,
        _folding: bool,
        _window: &mut Window,
        cx: &mut Context<EditorState>,
    ) {
        if let Some(edit) = &edit {
            if let Some(anchors) = &self.anchors {
                anchors
                    .borrow_mut()
                    .edit(edit.start_byte, edit.old_end_byte, edit.new_end_byte);
            }
            self.shift_runs(edit);
        }
        let text = text.to_string();
        let (engine, path) = (self.engine, self.path.clone());
        let (runs, theme) = (self.runs.clone(), self.theme.clone());
        let delayed = edit.is_some();
        self.task = Some(cx.spawn(async move |this, cx| {
            if delayed {
                cx.background_executor().timer(HIGHLIGHT_DELAY).await;
            }
            let spans = cx
                .background_executor()
                .spawn(async move { highlight(engine, &path, &text) })
                .await;
            *runs.borrow_mut() = spans
                .into_iter()
                .map(|(range, class)| (range, token_color(class, &theme)))
                .collect();
            this.update(cx, |editor, cx| editor.refresh(cx)).ok();
        }));
    }

    fn styles(
        &self,
        range: &Range<usize>,
        _resolver: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let runs = self.runs.borrow();
        let first = runs.partition_point(|(r, _)| r.end <= range.start);
        let mut out = Vec::new();
        let mut at = range.start;
        for (run, color) in &runs[first..] {
            if run.start >= range.end {
                break;
            }
            let start = run.start.max(at);
            let end = run.end.min(range.end);
            if start >= end {
                continue;
            }
            if at < start {
                out.push((at..start, HighlightStyle::default()));
            }
            out.push((
                start..end,
                HighlightStyle {
                    color: Some(*color),
                    ..HighlightStyle::default()
                },
            ));
            at = end;
        }
        if at < range.end {
            out.push((at..range.end, HighlightStyle::default()));
        }
        out
    }

    fn fold_ranges(&self, _text: &Rope) -> Vec<FoldRange> {
        Vec::new()
    }
}
