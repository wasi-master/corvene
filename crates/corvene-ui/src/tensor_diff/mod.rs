//! Corvene `1340-tensor-diff`: a model weight file (safetensors, GGUF,
//! ONNX, TensorFlow Lite, HDF5 / Keras, PyTorch, NumPy, Core ML) shown as
//! its tensors and metadata.
//!
//! Top to bottom: a summary bar (format, how many tensors were added,
//! removed, changed and renamed, the file size before and after), a line
//! with the parameter and tensor counts and anything the reader skipped,
//! then a Tensors / Metadata switch with a filter box, the Show unchanged
//! toggle and Compare Values.
//!
//! - Tensors: every tensor grouped by its dotted or slashed name prefix
//!   (`model.layers.0.`), folding like a file tree, with its shape, element
//!   type, size and status; rows tinted like diff lines. Compare Values
//!   reads both versions' elements on a background thread
//!   (`corvene_core::inspect_diff::TensorValuesJob`) and adds the largest
//!   and RMS difference, the share of changed elements and the cosine
//!   similarity; a row opens to its statistics, histograms and largest
//!   differences.
//! - Metadata: the files' key/value metadata by section, changed entries
//!   highlighted.
//!
//! GitHub Desktop shows these files as binary (`app/src/ui/diff/index.tsx`
//! picks `app/src/ui/diff/binary-file.tsx`: "This binary file has
//! changed."); the diff header's switch goes back to that.

mod metadata;
mod model;
mod table;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use corvene_core::{Diff, Dispatcher};
use corvene_models::TensorFile;
use corvene_models::tensors::{
    MetaRow, TensorFormat, TensorRow, TensorValues, align, align_metadata,
};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::icons::{Octicon, octicon};
use crate::keyboard_nav::ControlFocus;
use crate::scrollbar::{gutter, scrollbar};
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, GhdTheme};
use crate::widgets::{GhdTooltip, IconButtonA11y, ListRowA11y};
use model::{Counts, Item, MetaItem, Shown, Tree};

/// What the background pass builds once per diff.
struct Data {
    previous: Option<Arc<TensorFile>>,
    current: Option<Arc<TensorFile>>,
    rows: Vec<TensorRow>,
    tree: Tree,
    counts: Counts,
    meta: Vec<MetaRow>,
    meta_changed: usize,
}

impl Data {
    /// Both versions exist (not an added or deleted file).
    fn both(&self) -> bool {
        self.previous.is_some() && self.current.is_some()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Tensors,
    Metadata,
}

/// A Compare Values run in progress.
struct Running {
    cancel: Arc<AtomicBool>,
    canceled: bool,
    tensors: usize,
    _task: Task<()>,
}

/// How the last Compare Values run ended.
enum Outcome {
    Done {
        compared: usize,
        changed: usize,
        identical: usize,
    },
    Canceled {
        compared: usize,
    },
    Failed(String),
}

/// The tensor diff view of one `Diff::Tensors`.
pub struct TensorDiffView {
    diff: Arc<Diff>,
    repo: u64,
    data: Option<Rc<Data>>,
    _build: Option<Task<()>>,
    tab: Tab,
    show_unchanged: bool,
    show_unchanged_meta: bool,
    filter: Option<Entity<InputState>>,
    filter_text: String,
    /// Folded group prefixes.
    collapsed: HashSet<String>,
    /// Rows opened to their details.
    expanded: HashSet<usize>,
    items: Rc<Vec<Item>>,
    list: ListState,
    meta_items: Rc<Vec<MetaItem>>,
    meta_list: ListState,
    /// Compared values by tensor name.
    values: Rc<HashMap<String, TensorValues>>,
    running: Option<Running>,
    outcome: Option<Outcome>,
}

fn sides(diff: &Diff) -> (Option<Arc<TensorFile>>, Option<Arc<TensorFile>>) {
    match diff {
        Diff::Tensors {
            previous, current, ..
        } => (previous.clone(), current.clone()),
        _ => (None, None),
    }
}

/// Replaces `old` with `new` in `list`, touching only the rows between
/// their common start and end so the scroll position stays.
fn splice<T: PartialEq>(list: &ListState, old: &[T], new: &[T]) {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let room = old.len().min(new.len()) - prefix;
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(room)
        .take_while(|(a, b)| a == b)
        .count();
    if prefix == old.len() && prefix == new.len() {
        return;
    }
    list.splice(prefix..old.len() - suffix, new.len() - prefix - suffix);
}

impl TensorDiffView {
    /// `diff` is a `Diff::Tensors` of a file in `context.repo`.
    pub fn new(
        diff: Arc<Diff>,
        context: crate::diff_view::InspectContext,
        cx: &mut Context<Self>,
    ) -> Self {
        let (previous, current) = sides(&diff);
        let build = cx.background_executor().spawn(async move {
            let rows = align(previous.as_deref(), current.as_deref());
            let tree = Tree::build(&rows);
            let counts = Counts::of(&rows);
            let meta = align_metadata(previous.as_deref(), current.as_deref());
            let meta_changed = meta.iter().filter(|m| !m.unchanged()).count();
            Data {
                previous,
                current,
                rows,
                tree,
                counts,
                meta,
                meta_changed,
            }
        });
        let _build = Some(cx.spawn(async move |this, cx| {
            let data = build.await;
            this.update(cx, |this, cx| this.loaded(data, cx)).ok();
        }));
        Self {
            diff,
            repo: context.repo,
            data: None,
            _build,
            tab: Tab::Tensors,
            show_unchanged: false,
            show_unchanged_meta: false,
            filter: None,
            filter_text: String::new(),
            collapsed: HashSet::new(),
            expanded: HashSet::new(),
            items: Rc::new(Vec::new()),
            list: ListState::new(0, ListAlignment::Top, zpx(400.)),
            meta_items: Rc::new(Vec::new()),
            meta_list: ListState::new(0, ListAlignment::Top, zpx(400.)),
            values: Rc::new(HashMap::new()),
            running: None,
            outcome: None,
        }
    }

    /// The diff shown.
    pub fn diff(&self) -> &Arc<Diff> {
        &self.diff
    }

    fn loaded(&mut self, data: Data, cx: &mut Context<Self>) {
        // unchanged rows show when nothing else would
        self.show_unchanged = data.counts.layout_changes() == 0;
        self.show_unchanged_meta = data.meta_changed == 0;
        if data.counts.total() == 0 && !data.meta.is_empty() {
            self.tab = Tab::Metadata;
        }
        self.data = Some(Rc::new(data));
        self.refresh(cx);
    }

    /// Rebuilds the listed rows after an option changed.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(data) = self.data.clone() else {
            return;
        };
        let filter = self.filter_text.trim().to_lowercase();
        let values = self.values.clone();
        let value_changed = |ix: usize| table::value_changed(&data.rows[ix], &values);
        let items = model::flatten(
            &data.tree,
            &data.rows,
            &Shown {
                show_unchanged: self.show_unchanged,
                collapsed: &self.collapsed,
                expanded: &self.expanded,
                filter: &filter,
                value_changed: &value_changed,
            },
        );
        splice(&self.list, &self.items, &items);
        self.items = Rc::new(items);
        let meta = model::flatten_metadata(&data.meta, self.show_unchanged_meta, &filter);
        splice(&self.meta_list, &self.meta_items, &meta);
        self.meta_items = Rc::new(meta);
        cx.notify();
    }

    fn toggle_group(&mut self, prefix: String, cx: &mut Context<Self>) {
        if !self.collapsed.remove(&prefix) {
            self.collapsed.insert(prefix);
        }
        self.refresh(cx);
    }

    fn toggle_row(&mut self, row: usize, cx: &mut Context<Self>) {
        if !self.expanded.remove(&row) {
            self.expanded.insert(row);
        }
        self.refresh(cx);
    }

    fn set_show_unchanged(&mut self, on: bool, cx: &mut Context<Self>) {
        match self.tab {
            Tab::Tensors => self.show_unchanged = on,
            Tab::Metadata => self.show_unchanged_meta = on,
        }
        self.refresh(cx);
    }

    fn ensure_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(filter) = &self.filter {
            return filter.clone();
        }
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
        cx.subscribe(&filter, |this, input, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                this.filter_text = input.read(cx).value().to_string();
                this.refresh(cx);
            }
        })
        .detach();
        self.filter = Some(filter.clone());
        filter
    }

    /// The names Compare Values reads: the listed tensors while filtering,
    /// else every tensor (an empty list).
    fn compare_names(&self) -> Vec<String> {
        let Some(data) = &self.data else {
            return Vec::new();
        };
        if self.filter_text.trim().is_empty() {
            return Vec::new();
        }
        let mut names = Vec::new();
        for item in self.items.iter() {
            if let Item::Tensor { row, .. } = item {
                names.extend(table::names(&data.rows[*row]));
            }
        }
        names.dedup();
        names
    }

    /// Starts reading the values of `names` (all tensors when empty).
    fn compare(&mut self, names: Vec<String>, cx: &mut Context<Self>) {
        self.stop();
        let Some(job) = Dispatcher::tensor_values_job(self.repo, cx) else {
            self.outcome = Some(Outcome::Failed("The repository could not be found.".into()));
            cx.notify();
            return;
        };
        let (previous, current) = sides(&self.diff);
        let tensors = if names.is_empty() {
            self.data.as_ref().map_or(0, |d| d.rows.len())
        } else {
            names.len()
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let work = cx
            .background_executor()
            .spawn(async move { job.run(previous.as_deref(), current.as_deref(), &names, &flag) });
        let task = cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| this.finished(result, cx)).ok();
        });
        self.outcome = None;
        self.running = Some(Running {
            cancel,
            canceled: false,
            tensors,
            _task: task,
        });
        cx.notify();
    }

    /// Stops a run and forgets it.
    fn stop(&mut self) {
        if let Some(run) = self.running.take() {
            run.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Stops a run but keeps what it read so far.
    fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(run) = &mut self.running {
            run.cancel.store(true, Ordering::Relaxed);
            run.canceled = true;
        }
        cx.notify();
    }

    fn finished(&mut self, result: Result<Vec<TensorValues>, String>, cx: &mut Context<Self>) {
        let canceled = self.running.as_ref().is_some_and(|r| r.canceled);
        match result {
            Ok(list) => {
                let compared = list.len();
                let (mut changed, mut identical) = (0, 0);
                let map = Rc::make_mut(&mut self.values);
                for v in list {
                    match &v.diff {
                        Some(d) if d.identical => identical += 1,
                        Some(_) => changed += 1,
                        None => {}
                    }
                    map.insert(v.name.clone(), v);
                }
                self.outcome = Some(if canceled {
                    Outcome::Canceled { compared }
                } else {
                    Outcome::Done {
                        compared,
                        changed,
                        identical,
                    }
                });
            }
            Err(e) => self.outcome = Some(Outcome::Failed(e)),
        }
        self.list.remeasure();
        self.refresh(cx);
        self.running = None;
    }
}

impl Drop for TensorDiffView {
    fn drop(&mut self) {
        self.stop();
    }
}

fn format_label(f: &TensorFile) -> String {
    if f.format == TensorFormat::Gguf
        && let Some((_, v)) = f.metadata.iter().find(|(k, _)| k == "GGUF version")
    {
        return format!("GGUF v{v}");
    }
    f.format.display_name().to_string()
}

fn bytes(n: u64) -> String {
    crate::format::format_bytes(i64::try_from(n).unwrap_or(i64::MAX), 1)
}

/// `a → b` when the sides differ, else one of them.
fn pair<T: PartialEq>(old: Option<T>, new: Option<T>, f: impl Fn(&T) -> String) -> String {
    match (old, new) {
        (Some(o), Some(n)) if o != n => format!("{} → {}", f(&o), f(&n)),
        (_, Some(n)) => f(&n),
        (Some(o), None) => f(&o),
        (None, None) => String::new(),
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!(
        "{} {}",
        crate::format::format_count(n as u64),
        if n == 1 { one } else { many }
    )
}

/// A coloured count in the summary bar.
fn count_pill(id: &'static str, text: String, color: Hsla, tooltip: String) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .px(SPACING_HALF())
        .text_size(FONT_SIZE_SM())
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color)
        .child(text)
        .ghd_tooltip(tooltip)
}

/// A square toggle button (the archive diff's).
fn toggle_button(
    id: &'static str,
    icon: Octicon,
    on: bool,
    tooltip: &'static str,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.ghd();
    div()
        .id(id)
        .icon_button_label(tooltip)
        .flex_none()
        .size(zpx(22.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(BORDER_RADIUS())
        .border_1()
        .border_color(if on {
            t.box_selected_active_background
        } else {
            t.box_border
        })
        .when(on, |d| d.bg(t.box_selected_active_background))
        .cursor_pointer()
        .hover(|d| d.bg(t.box_hover_background))
        .child(octicon(
            icon,
            if on {
                t.box_selected_active_text
            } else {
                t.text
            },
        ))
        .on_click(move |_, window, cx| on_click(window, cx))
        .control_focus(cx)
}

/// The Tensors / Metadata switch (the font diff's segmented control).
fn segmented(
    labels: [SharedString; 2],
    selected: usize,
    on_pick: impl Fn(usize, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.ghd();
    let on_pick = Rc::new(on_pick);
    div()
        .id("tensor-tabs")
        .role(Role::TabList)
        .flex_none()
        .flex()
        .flex_row()
        .h(zpx(24.))
        .p(zpx(2.))
        .gap(zpx(2.))
        .rounded(BORDER_RADIUS())
        .bg(t.box_alt_background)
        .border_1()
        .border_color(t.box_border)
        .children(labels.into_iter().enumerate().map(|(i, label)| {
            let on_pick = on_pick.clone();
            let is_selected = i == selected;
            let hover = t.box_hover_background;
            div()
                .id(("tensor-tab", i))
                .role(Role::Tab)
                .aria_label(label.clone())
                .aria_selected(is_selected)
                .px(SPACING())
                .flex()
                .items_center()
                .rounded(zpx(3.))
                .text_size(FONT_SIZE_SM())
                .whitespace_nowrap()
                .when(is_selected, |d| {
                    d.bg(t.tab_bar_active)
                        .text_color(t.box_selected_active_text)
                })
                .when(!is_selected, |d| {
                    d.text_color(t.text)
                        .cursor_pointer()
                        .hover(move |d| d.bg(hover))
                })
                .on_click(move |_, window, cx| on_pick(i, window, cx))
                .control_focus(cx)
                .child(label)
        }))
}

impl TensorDiffView {
    /// The summary bar: format, change counts, file sizes.
    fn summary_bar(&self, data: &Data, t: &GhdTheme) -> AnyElement {
        let c = data.counts;
        let mut pills: Vec<AnyElement> = Vec::new();
        let mut pill = |id, text: String, color, tip: String| {
            pills.push(count_pill(id, text, color, tip).into_any_element());
        };
        if c.added > 0 {
            pill(
                "tensor-added",
                format!("+{}", c.added),
                t.color_new,
                plural(c.added, "tensor added", "tensors added"),
            );
        }
        if c.removed > 0 {
            pill(
                "tensor-removed",
                format!("−{}", c.removed),
                t.color_deleted,
                plural(c.removed, "tensor removed", "tensors removed"),
            );
        }
        if c.changed > 0 {
            pill(
                "tensor-changed",
                format!("~{}", c.changed),
                t.color_modified,
                plural(
                    c.changed,
                    "tensor changed shape, type or size",
                    "tensors changed shape, type or size",
                ),
            );
        }
        if c.renamed > 0 {
            pill(
                "tensor-renamed",
                format!("→{}", c.renamed),
                t.color_renamed,
                plural(c.renamed, "tensor renamed", "tensors renamed"),
            );
        }
        if data.both() {
            let text = if c.layout_changes() == 0 {
                "No layout changes".to_string()
            } else if c.unchanged > 0 {
                format!(
                    "{} unchanged",
                    crate::format::format_count(c.unchanged as u64)
                )
            } else {
                String::new()
            };
            pills.push(
                div()
                    .flex_none()
                    .px(SPACING_HALF())
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(text)
                    .into_any_element(),
            );
        }
        let (old, new) = (data.previous.as_deref(), data.current.as_deref());
        let formats = pair(old.map(format_label), new.map(format_label), |s| s.clone());
        let sizes = match (old, new) {
            (Some(o), Some(n)) if o.file_size != n.file_size => {
                let (sign, delta) = if n.file_size >= o.file_size {
                    ("+", n.file_size - o.file_size)
                } else {
                    ("−", o.file_size - n.file_size)
                };
                format!(
                    "{} → {} ({sign}{})",
                    bytes(o.file_size),
                    bytes(n.file_size),
                    bytes(delta)
                )
            }
            _ => pair(old.map(|f| f.file_size), new.map(|f| f.file_size), |n| {
                bytes(*n)
            }),
        };
        div()
            .id("tensor-summary")
            .h(zpx(30.))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .px(SPACING())
            .bg(t.box_alt_background)
            .border_b_1()
            .border_color(t.diff_border)
            .overflow_hidden()
            .child(octicon(Octicon::Database, t.text_secondary))
            .child(
                div()
                    .id("tensor-format")
                    .flex_none()
                    .ml(SPACING_HALF())
                    .mr(SPACING_HALF())
                    .px(zpx(5.))
                    .rounded(BORDER_RADIUS())
                    .border_1()
                    .border_color(t.box_border)
                    .text_size(zpx(10.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.text_secondary)
                    .child(formats)
                    .ghd_tooltip("Model file format"),
            )
            .children(pills)
            .child(div().flex_1().min_w(SPACING()))
            .child(
                div()
                    .id("tensor-sizes")
                    .flex_none()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(sizes)
                    .ghd_tooltip("File size"),
            )
            .into_any_element()
    }

    /// Parameter, tensor and byte totals, then the readers' notes.
    fn stats_line(&self, data: &Data, t: &GhdTheme) -> AnyElement {
        let (old, new) = (data.previous.as_deref(), data.current.as_deref());
        let params = pair(old.map(|f| f.parameters), new.map(|f| f.parameters), |n| {
            model::human_count(*n)
        });
        let exact = pair(old.map(|f| f.parameters), new.map(|f| f.parameters), |n| {
            crate::format::format_count(*n)
        });
        let tensors = pair(
            old.map(|f| f.tensors.len()),
            new.map(|f| f.tensors.len()),
            |n| crate::format::format_count(*n as u64),
        );
        let tensor_bytes = pair(
            old.map(|f| f.tensor_bytes),
            new.map(|f| f.tensor_bytes),
            |n| bytes(*n),
        );
        let single = old.is_none() || new.is_none();
        let new_count = new.or(old).map_or(0, |f| f.tensors.len());
        let mut notes: Vec<String> = Vec::new();
        let old_notes = old.map_or(&[][..], |f| f.notes.as_slice());
        let new_notes = new.map_or(&[][..], |f| f.notes.as_slice());
        for n in new_notes {
            if single || old_notes.contains(n) {
                notes.push(n.clone());
            } else {
                notes.push(format!("After: {n}"));
            }
        }
        for n in old_notes {
            if single {
                notes.push(n.clone());
            } else if !new_notes.contains(n) {
                notes.push(format!("Before: {n}"));
            }
        }
        let sep = || div().flex_none().child("·");
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(zpx(2.))
            .px(SPACING())
            .py(zpx(5.))
            .border_b_1()
            .border_color(t.diff_border)
            .text_size(FONT_SIZE_SM())
            .text_color(t.text_secondary)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap_x(zpx(6.))
                    .child(
                        div()
                            .id("tensor-parameters")
                            .flex_none()
                            .child(format!("{params} parameters"))
                            .ghd_tooltip(format!("{exact} parameters")),
                    )
                    .child(sep())
                    .child(div().flex_none().child(format!(
                        "{tensors} {}",
                        if new_count == 1 { "tensor" } else { "tensors" }
                    )))
                    .child(sep())
                    .child(
                        div()
                            .flex_none()
                            .child(format!("{tensor_bytes} of tensor data")),
                    ),
            )
            .children(notes.into_iter().map(|n| {
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(zpx(4.))
                    .child(
                        div()
                            .flex_none()
                            .mt(zpx(1.))
                            .child(octicon(Octicon::Info, t.text_secondary).size(zpx(12.))),
                    )
                    .child(div().flex_1().min_w_0().child(n))
            }))
            .into_any_element()
    }

    /// The tab switch, filter, Show unchanged and Compare Values.
    fn toolbar(&mut self, data: &Data, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.ghd().clone();
        let tensors_label: SharedString = format!(
            "Tensors {}",
            crate::format::format_count(data.rows.len() as u64)
        )
        .into();
        let meta_label: SharedString = if data.meta_changed > 0 {
            format!(
                "Metadata · {} changed",
                crate::format::format_count(data.meta_changed as u64)
            )
            .into()
        } else {
            "Metadata".into()
        };
        let weak = cx.weak_entity();
        let tabs = segmented(
            [tensors_label, meta_label],
            match self.tab {
                Tab::Tensors => 0,
                Tab::Metadata => 1,
            },
            move |i, _, cx| {
                weak.update(cx, |this, cx| {
                    this.tab = if i == 0 { Tab::Tensors } else { Tab::Metadata };
                    cx.notify();
                })
                .ok();
            },
            cx,
        );

        let input = self.ensure_filter(window, cx);
        let escape_input = input.clone();
        let filter = div()
            .flex_shrink(1.)
            .min_w(zpx(90.))
            .w(zpx(180.))
            .on_key_down(move |ev, window, cx| {
                if ev.keystroke.key == "escape" && !escape_input.read(cx).value().is_empty() {
                    escape_input.update(cx, |i, cx| i.set_value("", window, cx));
                    cx.stop_propagation();
                }
            })
            .child(crate::widgets::filter_text_box(
                "tensor-filter",
                &input,
                Some(octicon(Octicon::Filter, t.text_secondary)),
                window,
                cx,
            ));

        let (show, has_unchanged) = match self.tab {
            Tab::Tensors => (self.show_unchanged, data.counts.unchanged > 0),
            Tab::Metadata => (
                self.show_unchanged_meta,
                data.meta.len() > data.meta_changed,
            ),
        };
        let weak = cx.weak_entity();
        let unchanged = (has_unchanged && data.both()).then(|| {
            toggle_button(
                "tensor-show-unchanged",
                if show {
                    Octicon::Eye
                } else {
                    Octicon::EyeClosed
                },
                show,
                if show {
                    "Hide unchanged"
                } else {
                    "Show unchanged"
                },
                move |_, cx| {
                    weak.update(cx, |this, cx| this.set_show_unchanged(!show, cx))
                        .ok();
                },
                cx,
            )
        });

        let compare = (self.tab == Tab::Tensors && !data.rows.is_empty()).then(|| {
            let weak = cx.weak_entity();
            match &self.running {
                Some(run) => div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING_HALF())
                    .child(
                        div()
                            .id("tensor-reading")
                            .a11y_live("Reading values")
                            .text_size(FONT_SIZE_SM())
                            .text_color(t.text_secondary)
                            .whitespace_nowrap()
                            .child(if run.canceled {
                                "Canceling…".to_string()
                            } else {
                                format!(
                                    "Reading values of {}…",
                                    plural(run.tensors, "tensor", "tensors")
                                )
                            }),
                    )
                    .child(
                        crate::widgets::button("tensor-compare-cancel", "Cancel", cx).on_click(
                            move |_, _, cx| {
                                weak.update(cx, |this, cx| this.cancel(cx)).ok();
                            },
                        ),
                    )
                    .into_any_element(),
                None => {
                    let filtered = !self.filter_text.trim().is_empty();
                    crate::widgets::button("tensor-compare", "Compare Values", cx)
                        .flex_none()
                        .ghd_tooltip(if filtered {
                            "Read the listed tensors' values and compare them"
                        } else {
                            "Read every tensor's values and compare them"
                        })
                        .on_click(move |_, _, cx| {
                            weak.update(cx, |this, cx| {
                                let names = this.compare_names();
                                this.compare(names, cx);
                            })
                            .ok();
                        })
                        .into_any_element()
                }
            }
        });

        div()
            .flex_none()
            .h(zpx(36.))
            .px(SPACING())
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING_HALF())
            .border_b_1()
            .border_color(t.box_border)
            .bg(t.box_background)
            .child(tabs)
            .child(div().flex_1().min_w(SPACING_HALF()))
            .child(filter)
            .children(unchanged)
            .children(compare)
            .into_any_element()
    }

    /// The outcome of Compare Values, or the unchanged-layout hint.
    fn status_line(&self, data: &Data, t: &GhdTheme) -> Option<AnyElement> {
        if self.tab != Tab::Tensors {
            return None;
        }
        let (icon, color, text) = match &self.outcome {
            Some(Outcome::Done {
                compared,
                changed,
                identical,
            }) => {
                let mut text = format!("Compared {}.", plural(*compared, "tensor", "tensors"));
                if *changed > 0 || *identical > 0 {
                    text.push_str(&format!(
                        " {} changed, {} identical.",
                        crate::format::format_count(*changed as u64),
                        crate::format::format_count(*identical as u64)
                    ));
                }
                let skipped = compared - changed - identical;
                if skipped > 0 && data.both() {
                    text.push_str(&format!(
                        " {} not compared.",
                        crate::format::format_count(skipped as u64)
                    ));
                }
                (Octicon::Check, t.text_secondary, text)
            }
            Some(Outcome::Canceled { compared }) => (
                Octicon::Stop,
                t.text_secondary,
                format!("Canceled after {}.", plural(*compared, "tensor", "tensors")),
            ),
            Some(Outcome::Failed(e)) => (
                Octicon::Alert,
                t.error,
                format!("Could not read the values. {e}"),
            ),
            None if data.both()
                && data.counts.layout_changes() == 0
                && self.running.is_none()
                && !data.rows.is_empty() =>
            {
                (
                    Octicon::Info,
                    t.text_secondary,
                    "The tensor layout did not change. Compare Values to see whether the \
                     weights did."
                        .to_string(),
                )
            }
            None => return None,
        };
        Some(
            div()
                .id("tensor-status")
                .a11y_live(text.clone())
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(SPACING_HALF())
                .px(SPACING())
                .py(zpx(5.))
                .border_b_1()
                .border_color(t.diff_border)
                .text_size(FONT_SIZE_SM())
                .text_color(color)
                .child(octicon(icon, color).flex_none())
                .child(div().flex_1().min_w_0().child(text))
                .into_any_element(),
        )
    }

    fn empty(&self, message: &'static str, show_link: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let weak = cx.weak_entity();
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(SPACING_HALF())
            .p(SPACING())
            .text_color(t.text_secondary)
            .child(message)
            .when(show_link, |d| {
                d.child(
                    crate::widgets::link_button("tensor-empty-show", "Show unchanged", cx)
                        .on_click(move |_, _, cx| {
                            weak.update(cx, |this, cx| this.set_show_unchanged(true, cx))
                                .ok();
                        }),
                )
            })
            .into_any_element()
    }

    fn tensors_body(&mut self, data: &Rc<Data>, cx: &mut Context<Self>) -> AnyElement {
        let filtering = !self.filter_text.trim().is_empty();
        if data.rows.is_empty() {
            return self.empty("This file has no tensors.", false, cx);
        }
        if self.items.is_empty() {
            return if filtering {
                self.empty("No tensors match the filter.", false, cx)
            } else {
                self.empty("No tensors changed.", !self.show_unchanged, cx)
            };
        }
        let width = self.list.viewport_bounds().size.width;
        let cols = table::Cols::fit(width, !self.values.is_empty(), data.both());
        let rows = Rc::new(table::Rows {
            data: data.clone(),
            items: self.items.clone(),
            values: self.values.clone(),
            cols,
            view: cx.weak_entity(),
            running: self.running.is_some(),
        });
        let t = cx.ghd();
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .child(table::header(&cols, t).pr(gutter(&self.list)))
            .child(
                list(self.list.clone(), move |ix, _window, cx| rows.row(ix, cx))
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .pb(SPACING())
                    .pr(gutter(&self.list)),
            )
            .child(scrollbar("tensor-scrollbar", self.list.clone()))
            .into_any_element()
    }

    fn metadata_body(&mut self, data: &Rc<Data>, cx: &mut Context<Self>) -> AnyElement {
        if data.meta.is_empty() {
            return self.empty("This file has no metadata.", false, cx);
        }
        if self.meta_items.is_empty() {
            return if self.filter_text.trim().is_empty() {
                self.empty("No metadata changed.", !self.show_unchanged_meta, cx)
            } else {
                self.empty("No metadata matches the filter.", false, cx)
            };
        }
        let rows = Rc::new(metadata::Rows {
            data: data.clone(),
            items: self.meta_items.clone(),
        });
        let t = cx.ghd();
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .child(metadata::header(data, t).pr(gutter(&self.meta_list)))
            .child(
                list(self.meta_list.clone(), move |ix, _window, cx| {
                    rows.row(ix, cx)
                })
                .flex_1()
                .min_h_0()
                .w_full()
                .pb(SPACING())
                .pr(gutter(&self.meta_list)),
            )
            .child(scrollbar("tensor-meta-scrollbar", self.meta_list.clone()))
            .into_any_element()
    }
}

impl Render for TensorDiffView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd().clone();
        let root = div()
            .id("tensor-diff")
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(t.background)
            .text_color(t.text)
            .text_size(FONT_SIZE());
        let Some(data) = self.data.clone() else {
            return root;
        };
        let summary = self.summary_bar(&data, &t);
        let stats = self.stats_line(&data, &t);
        let toolbar = self.toolbar(&data, window, cx);
        let status = self.status_line(&data, &t);
        let body = match self.tab {
            Tab::Tensors => self.tensors_body(&data, cx),
            Tab::Metadata => self.metadata_body(&data, cx),
        };
        root.child(summary)
            .child(stats)
            .child(toolbar)
            .children(status)
            .child(body)
    }
}
