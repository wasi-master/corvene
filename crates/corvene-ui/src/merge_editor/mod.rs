//! Corvene `1361-merge-editor`: the merge editor, in place of the "must be
//! resolved via the command line" message (`app/src/ui/diff/index.tsx` in
//! GitHub Desktop, which has no merge editor).
//!
//! Ours, Result and Theirs side by side with ribbons between matching chunks
//! (an optional Base column), or Current and Incoming above the Result. Every
//! pane is a kit `EditorState`, the side panes read-only, soft wrap and
//! folding off so a line's y follows from its number. What each chunk holds is
//! read back from the Result's text ([`corvene_merge::scan`]), so undo needs
//! no bookkeeping; the anchors that carry chunk ranges through edits are fed
//! by the Result's highlighter ([`highlight`]). The kit's editor has no scroll
//! event: the pane the pointer last wheeled or pressed in drives the others,
//! whenever it notifies.
//!
//! Open files stay in [`MergeEditorView::open`] while their sessions live, so
//! switching files keeps undo.

mod chrome;
mod highlight;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use corvene_core::merge_editor::{MergeSession, session_of};
use corvene_core::persistence::MergeEditorLayout;
use corvene_core::{AppState, Dispatcher};
use corvene_merge::align::{AlignmentMap, Pane as AlignPane};
use corvene_merge::anchors::Anchors;
use corvene_merge::{ChunkKind, DocMode, LiveState, MergeDocument, Resolution, Side, Text};
use gpui_kit::component::input::{
    Editor, EditorState, InputEvent, RangeDecoration, RangeDecorationCollection,
    RangeDecorationStyle,
};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::actions::{
    MergeAcceptBoth, MergeAcceptOurs, MergeAcceptTheirs, MergeIgnoreConflict, MergeMarkResolved,
    MergeNextConflict, MergePreviousConflict, MergeSave,
};
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, GhdTheme, mono_font};

/// How long edits may pile up before the chunks are read again.
const SCAN_DELAY: Duration = Duration::from_millis(30);

/// `banner_focus` focuses a new banner after 200 ms.
const FOCUS_AFTER_BANNER: Duration = Duration::from_millis(300);

/// Lines kept above a conflict the editor jumps to.
const JUMP_CONTEXT_LINES: f32 = 5.;

/// A pane's slot in [`OpenFile::panes`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PaneId {
    Base,
    Ours,
    Result,
    Theirs,
}

impl PaneId {
    const ALL: [Self; 4] = [Self::Base, Self::Ours, Self::Result, Self::Theirs];

    fn index(self) -> usize {
        match self {
            Self::Base => 0,
            Self::Ours => 1,
            Self::Result => 2,
            Self::Theirs => 3,
        }
    }

    fn align(self) -> AlignPane {
        match self {
            Self::Base => AlignPane::Base,
            Self::Ours => AlignPane::Ours,
            Self::Result => AlignPane::Result,
            Self::Theirs => AlignPane::Theirs,
        }
    }

    fn side(self) -> Option<Side> {
        match self {
            Self::Base => Some(Side::Base),
            Self::Ours => Some(Side::Ours),
            Self::Result => None,
            Self::Theirs => Some(Side::Theirs),
        }
    }
}

/// The last frame's position of a pane's lines.
#[derive(Clone, Copy)]
pub(crate) struct PaneGeometry {
    /// The pane's top, in window coordinates.
    pub origin: Pixels,
    pub height: Pixels,
    /// The first line's y in window coordinates, scrolled.
    pub text_top: Pixels,
    pub scroll: Pixels,
    pub line_height: Pixels,
}

impl PaneGeometry {
    pub fn of(pane: &Pane, cx: &App) -> Option<Self> {
        Self::read(&pane.editor, pane.body.get(), cx)
    }

    pub fn read(editor: &Entity<EditorState>, bounds: Bounds<Pixels>, cx: &App) -> Option<Self> {
        let e = editor.read(cx);
        Some(Self {
            origin: bounds.origin.y,
            height: bounds.size.height,
            text_top: e.text_bounds()?.origin.y,
            scroll: e.scroll_offset().y,
            line_height: e.line_height()?,
        })
    }

    /// A line's top in window coordinates.
    pub fn line_y(&self, line: f32) -> Pixels {
        self.text_top + self.line_height * line
    }

    /// The fractional line at the pane's top.
    fn top_line(&self) -> f32 {
        -f32::from(self.scroll) / f32::from(self.line_height).max(1.)
    }

    /// The line at a window y.
    fn line_at(&self, y: Pixels) -> f32 {
        f32::from(y - self.text_top) / f32::from(self.line_height).max(1.)
    }
}

pub(crate) struct Pane {
    pub editor: Entity<EditorState>,
    /// The pane body's last bounds: what floats over it is placed from here
    /// (the editor's own bounds are inset by its padding).
    pub body: Rc<Cell<Bounds<Pixels>>>,
    fills: RangeDecorationCollection,
    last_scroll: Pixels,
}

/// A file with its editors.
pub(crate) struct OpenFile {
    pub generation: u64,
    pub doc: Arc<MergeDocument>,
    pub panes: [Pane; 4],
    pub anchors: Rc<RefCell<Anchors>>,
    pub live: LiveState,
    /// Each chunk's lines in the result.
    pub result_lines: Vec<std::ops::Range<u32>>,
    pub result_line_count: u32,
    align: AlignmentMap,
    /// The session's focus request last honoured.
    focus_request: u64,
    /// The pane that moves the others.
    driver: PaneId,
    scan: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl OpenFile {
    pub fn pane(&self, id: PaneId) -> &Pane {
        &self.panes[id.index()]
    }

    pub fn editor(&self, id: PaneId) -> &Entity<EditorState> {
        &self.pane(id).editor
    }
}

/// What the view shows now.
pub(crate) type FileKey = (u64, String);

pub struct MergeEditorView {
    pub(crate) state: Entity<AppState>,
    pub(crate) focus: FocusHandle,
    pub(crate) open: HashMap<FileKey, OpenFile>,
    /// The chunk and side the pointer is over, for the blame card.
    pub(crate) hovered: Option<(u32, Side)>,
}

fn editor_for(
    text: &str,
    readonly: bool,
    highlighter: highlight::MergeHighlighter,
    window: &mut Window,
    cx: &mut App,
) -> Entity<EditorState> {
    let highlighter = Rc::new(RefCell::new(Some(highlighter)));
    let text = text.to_string();
    cx.new(move |cx| {
        let mut editor = EditorState::new(window, cx)
            .soft_wrap(false)
            .folding(false)
            .auto_close(false)
            .line_number(true)
            .default_value(text);
        editor.set_readonly(readonly, cx);
        editor.set_highlighter_factory(
            Rc::new(move |_| {
                highlighter
                    .borrow_mut()
                    .take()
                    .map(|h| Box::new(h) as Box<dyn gpui_kit::component::input::InputHighlighter>)
            }),
            cx,
        );
        editor
    })
}

/// A chunk's lines in the result, from its byte range.
fn lines_of(text: &Text, range: &std::ops::Range<usize>) -> std::ops::Range<u32> {
    let start = text.line_of_byte(range.start);
    if range.is_empty() {
        return start..start;
    }
    let last = text.line_of_byte(range.end - 1);
    start..last + 1
}

/// A whole-lines byte range without its last line end, which would paint a
/// row of its own.
fn fill_range(text: &str, range: std::ops::Range<usize>) -> std::ops::Range<usize> {
    let end = if range.end > range.start && text.as_bytes().get(range.end - 1) == Some(&b'\n') {
        range.end - 1
    } else {
        range.end
    };
    range.start..end
}

/// A chunk's colour.
pub(crate) fn chunk_color(kind: ChunkKind, resolution: Resolution, t: &GhdTheme) -> Hsla {
    match (kind, resolution) {
        (ChunkKind::Conflict, Resolution::Unresolved) => t.color_conflicted,
        (ChunkKind::Conflict, _) => t.color_new,
        (ChunkKind::BothSame, _) => t.color_modified,
        _ => t.color_new,
    }
}

impl MergeEditorView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            focus: cx.focus_handle(),
            open: HashMap::new(),
            hovered: None,
        }
    }

    /// The selected repository and conflicted file.
    pub(crate) fn current_key(&self, cx: &App) -> Option<FileKey> {
        let s = self.state.read(cx);
        let id = s.selected?;
        let path = s.repo_states.get(&id)?.selected_file.clone()?;
        Some((id, path))
    }

    fn sync_open(&mut self, key: &FileKey, window: &mut Window, cx: &mut Context<Self>) {
        // files whose sessions ended
        let s = self.state.read(cx);
        self.open.retain(|(id, path), _| {
            s.repo_states
                .get(id)
                .is_some_and(|rs| rs.merge_editors.contains_key(path))
        });
        let engine = crate::diff_view::highlight_engine(s).0;
        let Some(session) = s
            .repo_states
            .get(&key.0)
            .and_then(|rs| session_of(rs, &key.1))
        else {
            return;
        };
        let Some(doc) = session.document.clone() else {
            self.open.remove(key);
            return;
        };
        let focus_request = session.focus_request;
        if let Some(file) = self
            .open
            .get_mut(key)
            .filter(|f| f.generation == session.generation)
        {
            if file.focus_request != focus_request {
                file.focus_request = focus_request;
                let handle = file.editor(PaneId::Result).read(cx).focus_handle(cx);
                // after the conflicts banner, which takes focus as it appears
                let key = key.clone();
                cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(FOCUS_AFTER_BANNER).await;
                    this.update_in(cx, |this, window, cx| {
                        window.focus(&handle, cx);
                        let first = this
                            .open
                            .get(&key)
                            .and_then(|f| f.live.unresolved_chunks().next().map(|c| c.id));
                        if let Some(first) = first {
                            this.jump_to_chunk(&key, first, window, cx);
                        }
                    })
                    .ok();
                })
                .detach();
            }
            return;
        }
        let generation = session.generation;
        let session_focus = session.focus_request;
        let text = session.text.clone();
        let theme = cx.ghd().clone();
        let anchors = Rc::new(RefCell::new(Anchors::new(doc.result_ranges())));
        let path = key.1.clone();
        let mut make = |text: &str, readonly: bool, tap: Option<Rc<RefCell<Anchors>>>| {
            let highlighter =
                highlight::MergeHighlighter::new(path.clone(), engine, theme.clone(), tap);
            let editor = editor_for(text, readonly, highlighter, window, cx);
            let fills = editor.update(cx, |e, cx| {
                e.create_range_decorations_collection(Vec::new(), cx)
            });
            Pane {
                editor,
                body: Rc::default(),
                fills,
                last_scroll: px(0.),
            }
        };
        let panes = [
            make(doc.base.as_str(), true, None),
            make(doc.ours.as_str(), true, None),
            make(&text, false, Some(anchors.clone())),
            make(doc.theirs.as_str(), true, None),
        ];
        let mut subscriptions = Vec::new();
        for id in PaneId::ALL {
            let key = key.clone();
            subscriptions.push(cx.observe(&panes[id.index()].editor, move |this, _, cx| {
                this.sync_scroll(&key, id, cx);
            }));
        }
        let event_key = key.clone();
        subscriptions.push(cx.subscribe_in(
            &panes[PaneId::Result.index()].editor,
            window,
            move |this, _, event: &InputEvent, window, cx| {
                this.on_result_event(&event_key, event, window, cx)
            },
        ));
        let file = OpenFile {
            generation,
            doc: doc.clone(),
            panes,
            anchors,
            live: LiveState::default(),
            result_lines: Vec::new(),
            result_line_count: 0,
            align: AlignmentMap::new([]),
            focus_request: session_focus,
            driver: PaneId::Result,
            scan: None,
            _subscriptions: subscriptions,
        };
        self.open.insert(key.clone(), file);
        self.rescan(key, cx);
        if let Some(file) = self.open.get(key) {
            let handle = file.editor(PaneId::Result).read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
    }

    fn on_result_event(
        &mut self,
        key: &FileKey,
        event: &InputEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                let Some(file) = self.open.get_mut(key) else {
                    return;
                };
                file.driver = PaneId::Result;
                let task_key = key.clone();
                file.scan = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(SCAN_DELAY).await;
                    this.update(cx, |this, cx| this.rescan(&task_key, cx)).ok();
                }));
            }
            InputEvent::Blur => Dispatcher::flush_merge_editor(key.0, &key.1, cx),
            InputEvent::PressEnter {
                secondary: true, ..
            } => self.mark_resolved(key, cx),
            _ => {}
        }
    }

    /// Reads every chunk back from the result's text and repaints what
    /// follows from it.
    fn rescan(&mut self, key: &FileKey, cx: &mut Context<Self>) {
        let t = cx.ghd().clone();
        let Some(file) = self.open.get_mut(key) else {
            return;
        };
        let value = file.editor(PaneId::Result).read(cx).value();
        let text: &str = &value;
        let doc = file.doc.clone();
        let live = corvene_merge::scan(&doc, text, file.anchors.borrow().ranges());
        let lines = Text::new(text);
        file.result_lines = live
            .chunks
            .iter()
            .map(|c| lines_of(&lines, &c.range))
            .collect();
        file.result_line_count = lines.line_count();
        file.align = AlignmentMap::from_doc(&doc, &file.result_lines, file.result_line_count);
        let mut fills: [Vec<RangeDecoration>; 4] = Default::default();
        let alpha = if t.is_dark() { 0.2 } else { 0.14 };
        for (chunk, state) in doc.chunks.iter().zip(&live.chunks) {
            if !chunk.kind.is_change() {
                continue;
            }
            let color = chunk_color(chunk.kind, state.resolution, &t).opacity(alpha);
            let fill = |range| {
                RangeDecoration::new(range)
                    .with_style(RangeDecorationStyle::Fill)
                    .with_color(color)
            };
            let result = fill_range(text, state.range.clone());
            if !result.is_empty() {
                fills[PaneId::Result.index()].push(fill(result));
            }
            if !chunk.has_ribbons() {
                continue;
            }
            for id in [PaneId::Base, PaneId::Ours, PaneId::Theirs] {
                let Some(side) = id.side() else { continue };
                let shown = doc.text(side);
                let range = fill_range(shown.as_str(), shown.byte_range(chunk.lines(side)));
                if !range.is_empty() {
                    fills[id.index()].push(fill(range));
                }
            }
        }
        let [base, ours, result, theirs] = fills;
        for (id, fills) in [
            (PaneId::Base, base),
            (PaneId::Ours, ours),
            (PaneId::Result, result),
            (PaneId::Theirs, theirs),
        ] {
            file.pane(id).fills.set(fills, cx);
        }
        file.live = live;
        Dispatcher::merge_text_changed(key.0, &key.1, text.to_string(), cx);
        cx.notify();
    }

    /// Moves the other panes when the driving pane scrolled.
    fn sync_scroll(&mut self, key: &FileKey, from: PaneId, cx: &mut Context<Self>) {
        let Some(file) = self.open.get_mut(key) else {
            return;
        };
        let now = file.editor(from).read(cx).scroll_offset().y;
        let changed = (now - file.pane(from).last_scroll).abs() >= px(0.5);
        if !changed {
            return;
        }
        file.panes[from.index()].last_scroll = now;
        if from == file.driver
            && let Some(geometry) = PaneGeometry::of(file.pane(from), cx)
        {
            let line = geometry.top_line() as f64;
            let targets: Vec<_> = PaneId::ALL
                .into_iter()
                .filter(|to| *to != from)
                .map(|to| (to, file.align.map(from.align(), line, to.align()) as f32))
                .collect();
            for (to, line) in targets {
                let y = -(geometry.line_height * line);
                let editor = file.editor(to).clone();
                editor.update(cx, |e, cx| {
                    let x = e.scroll_offset().x;
                    e.set_scroll_offset(point(x, y.min(px(0.))), cx);
                });
            }
        }
        cx.notify();
    }

    pub(crate) fn set_driver(&mut self, key: &FileKey, pane: PaneId) {
        if let Some(file) = self.open.get_mut(key) {
            file.driver = pane;
        }
    }

    /// Scrolls the result so a chunk is in view, the rest following.
    pub(crate) fn jump_to_chunk(
        &mut self,
        key: &FileKey,
        chunk: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(file) = self.open.get_mut(key) else {
            return;
        };
        file.driver = PaneId::Result;
        let Some(range) = file
            .live
            .chunks
            .get(chunk as usize)
            .map(|c| c.range.clone())
        else {
            return;
        };
        let line = file
            .result_lines
            .get(chunk as usize)
            .map(|l| l.start)
            .unwrap_or(0);
        let editor = file.editor(PaneId::Result).clone();
        editor.update(cx, |e, cx| {
            e.set_selected_range(range.start..range.start, cx);
            if let Some(lh) = e.line_height() {
                let x = e.scroll_offset().x;
                let y = -(lh * (line as f32 - JUMP_CONTEXT_LINES).max(0.));
                e.set_scroll_offset(point(x, y), cx);
            }
            e.focus(window, cx);
        });
    }

    /// The chunk the caret is in, or the next one left to resolve.
    fn caret_conflict(&self, key: &FileKey, cx: &App) -> Option<u32> {
        let file = self.open.get(key)?;
        let at = file.editor(PaneId::Result).read(cx).cursor();
        let conflicts = || {
            file.live
                .chunks
                .iter()
                .zip(&file.doc.chunks)
                .filter(|(_, c)| c.kind == ChunkKind::Conflict)
                .map(|(l, _)| l)
        };
        conflicts()
            .find(|c| c.range.start <= at && at <= c.range.end)
            .or_else(|| {
                conflicts().find(|c| c.range.start >= at && c.resolution == Resolution::Unresolved)
            })
            .or_else(|| conflicts().find(|c| c.resolution == Resolution::Unresolved))
            .map(|c| c.id)
    }

    /// F7 / ⇧F7: the next or previous unresolved conflict, wrapping.
    pub(crate) fn step_conflict(
        &mut self,
        key: &FileKey,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(file) = self.open.get(key) else {
            return;
        };
        let at = file.editor(PaneId::Result).read(cx).cursor();
        let unresolved: Vec<_> = file
            .live
            .unresolved_chunks()
            .map(|c| (c.id, c.range.start))
            .collect();
        let target = if forward {
            unresolved
                .iter()
                .find(|(_, start)| *start > at)
                .or(unresolved.first())
        } else {
            unresolved
                .iter()
                .rev()
                .find(|(_, start)| *start < at)
                .or(unresolved.last())
        };
        if let Some((id, _)) = target {
            self.jump_to_chunk(key, *id, window, cx);
        }
    }

    /// One edit, one undo step.
    pub(crate) fn accept(
        &mut self,
        key: &FileKey,
        chunk: u32,
        resolution: Resolution,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.rescan(key, cx);
        let Some(file) = self.open.get(key) else {
            return;
        };
        let editor = file.editor(PaneId::Result).clone();
        let value = editor.read(cx).value();
        let Some(live) = file.live.chunks.get(chunk as usize).cloned() else {
            return;
        };
        let Some(edit) = corvene_merge::resolve(&file.doc, &value, &live, resolution) else {
            return;
        };
        let anchors = file.anchors.clone();
        let new_len = edit.text.len();
        let start = edit.range.start;
        editor.update(cx, |e, cx| {
            e.set_selected_range(edit.range.clone(), cx);
            e.replace(edit.text, window, cx);
        });
        anchors
            .borrow_mut()
            .claim(chunk as usize, start..start + new_len);
        self.rescan(key, cx);
    }

    fn accept_at_caret(
        &mut self,
        resolution: Resolution,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = self.current_key(cx) else {
            return;
        };
        if let Some(chunk) = self.caret_conflict(&key, cx) {
            self.accept(&key, chunk, resolution, window, cx);
            self.step_conflict(&key, true, window, cx);
        }
    }

    /// The wand: every conflict with an automatic resolution, last first.
    pub(crate) fn resolve_automatically(
        &mut self,
        key: &FileKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(file) = self.open.get(key) else {
            return;
        };
        let autos: Vec<_> = file
            .live
            .unresolved_chunks()
            .filter_map(|l| {
                let auto = file.doc.chunks.get(l.id as usize)?.auto.as_ref()?;
                Some((l.id, auto.resolution))
            })
            .collect();
        for (chunk, resolution) in autos.into_iter().rev() {
            self.accept(key, chunk, resolution, window, cx);
        }
    }

    pub(crate) fn mark_resolved(&mut self, key: &FileKey, cx: &mut Context<Self>) {
        let unresolved = self
            .open
            .get(key)
            .map(|file| {
                let value = file.editor(PaneId::Result).read(cx).value();
                corvene_merge::scan(&file.doc, &value, file.anchors.borrow().ranges()).unresolved
            })
            .unwrap_or(0);
        if unresolved > 0 {
            return;
        }
        self.rescan(key, cx);
        Dispatcher::mark_merge_resolved(key.0, &key.1, cx);
    }

    /// The pane's body: the editor with whatever floats over it.
    fn pane_body(
        &self,
        key: &FileKey,
        id: PaneId,
        text_size: Pixels,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let wheel_key = key.clone();
        let down_key = key.clone();
        let move_key = key.clone();
        let pane = self.open[key].pane(id);
        let editor = pane.editor.clone();
        let body = pane.body.clone();
        div()
            .id(SharedString::from(format!("merge-pane-{id:?}")))
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .on_scroll_wheel(cx.listener(move |this, _, _, _| {
                this.set_driver(&wheel_key, id);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, _| {
                    this.set_driver(&down_key, id);
                }),
            )
            .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                this.hover_side(&move_key, id, ev.position.y, cx);
            }))
            .child(
                canvas(move |bounds, _, _| body.set(bounds), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .child(
                Editor::new(&editor)
                    .appearance(false)
                    .bordered(false)
                    .h_full()
                    .font_family(mono_font())
                    .text_size(text_size),
            )
    }

    /// Which chunk the pointer is over in a side pane.
    fn hover_side(&mut self, key: &FileKey, id: PaneId, y: Pixels, cx: &mut Context<Self>) {
        let hovered = id.side().and_then(|side| {
            let file = self.open.get(key)?;
            let geometry = PaneGeometry::of(file.pane(id), cx)?;
            let line = geometry.line_at(y).floor().max(0.) as u32;
            file.doc
                .chunks
                .iter()
                .find(|c| c.kind.is_change() && c.has_ribbons() && c.lines(side).contains(&line))
                .map(|c| (c.id, side))
        });
        if hovered == self.hovered {
            return;
        }
        self.hovered = hovered;
        if let Some((chunk, side)) = hovered {
            Dispatcher::load_chunk_blame(key.0, &key.1, chunk, side, cx);
        }
        cx.notify();
    }

    fn message(text: String, t: &GhdTheme) -> Div {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(t.text_secondary)
            .child(text)
    }

    fn body(
        &mut self,
        key: &FileKey,
        session: &MergeSession,
        layout: MergeEditorLayout,
        show_base: bool,
        text_size: Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let doc = self.open[key].doc.clone();
        let markers_only = doc.mode == DocMode::MarkersOnly;
        let result = self.result_column(key, session, layout, text_size, cx);
        if markers_only {
            return div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(result)
                .into_any_element();
        }
        let ours = self.side_column(key, PaneId::Ours, layout, text_size, cx);
        let theirs = self.side_column(key, PaneId::Theirs, layout, text_size, cx);
        match layout {
            MergeEditorLayout::ThreePane => {
                let mut row = div().flex_1().min_h_0().flex().flex_row();
                if show_base && doc.has_base {
                    let base = self.side_column(key, PaneId::Base, layout, text_size, cx);
                    row = row.child(base).child(chrome::thin_gutter(cx));
                }
                row.child(ours)
                    .child(self.ribbon_gutter(key, PaneId::Ours, PaneId::Result, cx))
                    .child(result)
                    .child(self.ribbon_gutter(key, PaneId::Theirs, PaneId::Result, cx))
                    .child(theirs)
                    .into_any_element()
            }
            MergeEditorLayout::Stacked => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_row()
                        .child(ours)
                        .child(chrome::thin_gutter(cx))
                        .child(theirs),
                )
                .child(div().h(zpx(1.)).flex_none().bg(cx.ghd().box_border))
                .child(result)
                .into_any_element(),
        }
    }
}

impl Focusable for MergeEditorView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for MergeEditorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd().clone();
        let Some(key) = self.current_key(cx) else {
            return div().size_full().into_any_element();
        };
        let (session, layout, show_base, text_size) = {
            let s = self.state.read(cx);
            let session = s
                .repo_states
                .get(&key.0)
                .and_then(|rs| session_of(rs, &key.1))
                .cloned();
            let size = match s.flags.number(corvene_core::flags::ids::DIFF_FONT_SIZE) {
                0 => FONT_SIZE_SM(),
                size => zpx(size.clamp(9, 16) as f32),
            };
            (
                session,
                s.settings.merge_editor_layout,
                s.settings.merge_editor_show_base,
                size,
            )
        };
        let Some(session) = session else {
            let open_key = key.clone();
            cx.defer(move |cx| Dispatcher::open_merge_editor(open_key.0, open_key.1, cx));
            return Self::message("Loading…".to_string(), &t).into_any_element();
        };
        self.sync_open(&key, window, cx);
        let content = if session.loading && session.generation == 0 {
            Self::message("Loading…".to_string(), &t).into_any_element()
        } else if let Some(message) = session.message.clone() {
            Self::message(message, &t).into_any_element()
        } else if session.pick.is_some() {
            chrome::side_picker(key.0, &session, self.state.read(cx), cx).into_any_element()
        } else if self.open.contains_key(&key) {
            self.body(&key, &session, layout, show_base, text_size, cx)
        } else {
            div().into_any_element()
        };
        let toolbar = self.toolbar(&key, &session, layout, show_base, cx);
        let banner = session
            .external_change
            .then(|| chrome::external_banner(key.0, &key.1, cx));
        let previous_key = key.clone();
        let next_key = key.clone();
        let mark_key = key.clone();
        let save_key = key.clone();
        div()
            .id("merge-editor")
            .key_context("MergeEditor")
            .track_focus(&self.focus)
            .on_action(cx.listener(move |this, _: &MergeNextConflict, window, cx| {
                this.step_conflict(&next_key, true, window, cx)
            }))
            .on_action(
                cx.listener(move |this, _: &MergePreviousConflict, window, cx| {
                    this.step_conflict(&previous_key, false, window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &MergeAcceptOurs, window, cx| {
                this.accept_at_caret(Resolution::Ours, window, cx)
            }))
            .on_action(cx.listener(|this, _: &MergeAcceptTheirs, window, cx| {
                this.accept_at_caret(Resolution::Theirs, window, cx)
            }))
            .on_action(cx.listener(|this, _: &MergeAcceptBoth, window, cx| {
                this.accept_at_caret(Resolution::OursThenTheirs, window, cx)
            }))
            .on_action(cx.listener(|this, _: &MergeIgnoreConflict, window, cx| {
                this.accept_at_caret(Resolution::Ignore, window, cx)
            }))
            .on_action(cx.listener(move |this, _: &MergeMarkResolved, _, cx| {
                this.mark_resolved(&mark_key, cx)
            }))
            .on_action(cx.listener(move |this, _: &MergeSave, _, cx| {
                this.rescan(&save_key, cx);
                Dispatcher::flush_merge_editor(save_key.0, &save_key.1, cx);
            }))
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .bg(t.background)
            .child(toolbar)
            .children(banner)
            .child(content)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{fill_range, lines_of};
    use corvene_merge::Text;

    #[test]
    fn chunk_lines_follow_their_bytes() {
        let text = Text::new("a\nb\nc\n");
        assert_eq!(lines_of(&text, &(0..2)), 0..1);
        assert_eq!(lines_of(&text, &(2..6)), 1..3);
        assert_eq!(lines_of(&text, &(4..4)), 2..2);
        assert_eq!(lines_of(&text, &(6..6)), 3..3);
    }

    #[test]
    fn fills_leave_the_last_line_end() {
        assert_eq!(fill_range("a\nb\n", 0..2), 0..1);
        assert_eq!(fill_range("a\nb", 2..3), 2..3);
        assert_eq!(fill_range("a\n", 1..1), 1..1);
    }
}
