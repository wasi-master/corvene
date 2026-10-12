//! Corvene `1360-compiled-summary`: a compiled file's diff read as
//! structured data. A strip on top names the file (old → new when its
//! format, file type or architecture changed) and counts the added,
//! removed and changed parts, with a "Show unchanged" toggle and a filter.
//! Under it a Header table lists the file's facts old → new (format,
//! architectures, entry point, UUID, minimum OS, flags, code signature;
//! a class file's version and access flags; a .pyc file's Python version),
//! then one table per kind of part (sections, linked libraries, symbols,
//! members, fields, methods, code objects, imports, exports, functions),
//! each folding on a click of its header. A row shows its status, name,
//! kind or type (and what it was), its size old → new with the delta, and
//! whether its contents changed under the same size.
//!
//! GitHub Desktop shows compiled files as binary files
//! (`app/src/ui/diff/index.tsx`); the diff header's Summary / Listing
//! switch goes to the decoded listing of `1350-compiled-code-diff`.

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use corvene_core::Diff;
use corvene_models::compiled::{
    CompiledDiff, CompiledFact, CompiledFamily, CompiledGroupKind, CompiledRow, CompiledStatus,
};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::icons::{Octicon, octicon};
use crate::keyboard_nav::ControlFocus;
use crate::scrollbar::{gutter, scrollbar};
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, GhdTheme, mono_font};
use crate::widgets::{GhdTooltip, IconButtonA11y};

/// A row's height (CSS px), dense like GitHub Desktop's lists.
const ROW: f32 = 26.;
/// The fact label column's width (CSS px).
const LABEL_W: f32 = 170.;
/// The size column's width (CSS px).
const SIZE_W: f32 = 210.;

/// Which table a header row folds.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Section {
    Facts,
    Group(usize),
}

/// One row of the list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Row {
    Header(Section),
    /// A fact (index into `facts`).
    Fact(usize),
    /// A part: group, row.
    Item(usize, usize),
    /// "N unchanged rows not listed" under a group.
    Hidden(usize),
}

/// The compiled file summary view.
pub struct CompiledDiffView {
    diff: Arc<Diff>,
    data: Arc<CompiledDiff>,
    /// Lower-case row names per group, for the filter.
    lower: Vec<Vec<String>>,
    rows: Rc<Vec<Row>>,
    list: ListState,
    collapsed: HashSet<Section>,
    show_unchanged: bool,
    filter: Option<Entity<InputState>>,
    filter_text: String,
    /// The table whose header is pinned over the top of the list.
    sticky: Option<Section>,
}

fn status_color(status: CompiledStatus, t: &GhdTheme) -> Hsla {
    match status {
        CompiledStatus::Added => t.color_new,
        CompiledStatus::Removed => t.color_deleted,
        CompiledStatus::Changed => t.color_modified,
        CompiledStatus::Unchanged => t.text_secondary,
    }
}

fn status_icon(status: CompiledStatus) -> Octicon {
    match status {
        CompiledStatus::Added => Octicon::DiffAdded,
        CompiledStatus::Removed => Octicon::DiffRemoved,
        CompiledStatus::Changed => Octicon::DiffModified,
        CompiledStatus::Unchanged => Octicon::DotFill,
    }
}

fn family_icon(family: CompiledFamily) -> Octicon {
    match family {
        CompiledFamily::Native => Octicon::FileBinary,
        CompiledFamily::JavaClass | CompiledFamily::PythonBytecode => Octicon::FileCode,
        CompiledFamily::WebAssembly => Octicon::Package,
    }
}

fn count(n: usize) -> String {
    crate::format::format_count(n as u64)
}

/// A byte count as "1,234 bytes".
fn bytes(n: u64) -> String {
    format!(
        "{} {}",
        crate::format::format_count(n),
        if n == 1 { "byte" } else { "bytes" }
    )
}

/// A signed delta ("+12", "−4").
fn signed(delta: i64) -> String {
    if delta >= 0 {
        format!("+{}", crate::format::format_count(delta as u64))
    } else {
        format!("−{}", crate::format::format_count(delta.unsigned_abs()))
    }
}

/// "name old → new" / "Added name", a row's copied text.
fn copy_text(row: &CompiledRow) -> String {
    let mut out = format!("{} {}", row.status.label(), row.name);
    if !row.detail.is_empty() {
        out.push_str(&format!(" ({})", row.detail));
    }
    match (row.old_size, row.new_size) {
        (Some(a), Some(b)) if a != b => out.push_str(&format!(", {a} → {b} bytes")),
        _ => {}
    }
    out
}

impl CompiledDiffView {
    /// `diff` is a `Diff::Converted` with a summary; any other diff shows
    /// as empty.
    pub fn new(
        diff: Arc<Diff>,
        context: crate::diff_view::InspectContext,
        cx: &mut Context<Self>,
    ) -> Self {
        let _ = context;
        let data = match &*diff {
            Diff::Converted {
                summary: Some(summary),
                ..
            } => summary.clone(),
            _ => Arc::new(CompiledDiff::default()),
        };
        let lower = data
            .groups
            .iter()
            .map(|g| g.rows.iter().map(|r| r.name.to_lowercase()).collect())
            .collect();
        let list = ListState::new(0, ListAlignment::Top, zpx(400.));
        let weak = cx.weak_entity();
        list.set_scroll_handler(move |_, _, cx| {
            weak.update(cx, |this, cx| {
                let sticky = this.sticky_section();
                if sticky != this.sticky {
                    this.sticky = sticky;
                    cx.notify();
                }
            })
            .ok();
        });
        let mut this = Self {
            diff,
            data,
            lower,
            rows: Rc::new(Vec::new()),
            list,
            collapsed: HashSet::new(),
            show_unchanged: false,
            filter: None,
            filter_text: String::new(),
            sticky: None,
        };
        this.build_rows();
        this
    }

    /// The diff shown.
    pub fn diff(&self) -> &Arc<Diff> {
        &self.diff
    }

    fn build_rows(&mut self) {
        let needle = self.filter_text.trim().to_lowercase();
        let filtering = !needle.is_empty();
        let mut rows = Vec::new();
        if !filtering && !self.data.facts.is_empty() {
            rows.push(Row::Header(Section::Facts));
            if !self.collapsed.contains(&Section::Facts) {
                rows.extend((0..self.data.facts.len()).map(Row::Fact));
            }
        }
        for (gix, group) in self.data.groups.iter().enumerate() {
            let shown: Vec<usize> = group
                .rows
                .iter()
                .enumerate()
                .filter(|(rix, r)| {
                    (self.show_unchanged || r.status != CompiledStatus::Unchanged)
                        && (!filtering || self.lower[gix][*rix].contains(&needle))
                })
                .map(|(rix, _)| rix)
                .collect();
            if filtering && shown.is_empty() {
                continue;
            }
            rows.push(Row::Header(Section::Group(gix)));
            // a filter shows every match
            if !filtering && self.collapsed.contains(&Section::Group(gix)) {
                continue;
            }
            rows.extend(shown.into_iter().map(|rix| Row::Item(gix, rix)));
            if self.show_unchanged && !filtering && group.unchanged_hidden > 0 {
                rows.push(Row::Hidden(gix));
            }
        }
        // keep the row at the top where it was (a folded row's header)
        let top = self.list.logical_scroll_top();
        let anchor = self.rows.get(top.item_ix).copied();
        self.list.reset_with_uniform_height(rows.len(), zpx(ROW));
        if let Some(anchor) = anchor {
            let at = rows
                .iter()
                .position(|r| *r == anchor)
                .map(|ix| (ix, top.offset_in_item))
                .or_else(|| {
                    let section = section_of(anchor)?;
                    rows.iter()
                        .position(|r| *r == Row::Header(section))
                        .map(|ix| (ix, px(0.)))
                });
            if let Some((item_ix, offset_in_item)) = at {
                self.list.scroll_to(ListOffset {
                    item_ix,
                    offset_in_item,
                });
            }
        }
        self.rows = Rc::new(rows);
        self.sticky = self.sticky_section();
    }

    /// The table of the row at the top of the list, unless that row is the
    /// table's own header in full view.
    fn sticky_section(&self) -> Option<Section> {
        let top = self.list.logical_scroll_top();
        match *self.rows.get(top.item_ix)? {
            Row::Header(section) => (top.offset_in_item > px(0.)).then_some(section),
            row => section_of(row),
        }
    }

    fn toggle(&mut self, section: Section, cx: &mut Context<Self>) {
        if !self.collapsed.remove(&section) {
            self.collapsed.insert(section);
        }
        self.build_rows();
        cx.notify();
    }

    fn set_show_unchanged(&mut self, on: bool, cx: &mut Context<Self>) {
        self.show_unchanged = on;
        self.build_rows();
        cx.notify();
    }

    fn ensure_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(filter) = &self.filter {
            return filter.clone();
        }
        let placeholder = match self.data.family {
            CompiledFamily::Native => "Filter symbols and sections",
            CompiledFamily::JavaClass => "Filter fields and methods",
            CompiledFamily::PythonBytecode => "Filter code objects",
            CompiledFamily::WebAssembly => "Filter imports, exports, functions",
        };
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        cx.subscribe(&filter, |this, input, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                let text = input.read(cx).value().to_string();
                if text != this.filter_text {
                    this.filter_text = text;
                    this.build_rows();
                    this.list.scroll_to(ListOffset::default());
                    cx.notify();
                }
            }
        })
        .detach();
        self.filter = Some(filter.clone());
        filter
    }
}

fn section_of(row: Row) -> Option<Section> {
    match row {
        Row::Header(section) => Some(section),
        Row::Fact(_) => Some(Section::Facts),
        Row::Item(gix, _) | Row::Hidden(gix) => Some(Section::Group(gix)),
    }
}

/// What a row draws from, shared with the list's row callback.
struct Rows {
    data: Arc<CompiledDiff>,
    rows: Rc<Vec<Row>>,
    collapsed: HashSet<Section>,
    filtering: bool,
    view: WeakEntity<CompiledDiffView>,
}

/// A small bordered label ("contents changed").
fn chip(text: impl Into<SharedString>, color: Hsla, border: Hsla) -> Div {
    div()
        .flex_none()
        .px(zpx(4.))
        .rounded(zpx(4.))
        .border_1()
        .border_color(border)
        .text_size(zpx(10.))
        .line_height(zpx(14.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .child(text.into())
}

/// A size delta chip, `None` when the size did not change.
fn delta_chip(delta: Option<i64>, t: &GhdTheme) -> Option<Div> {
    let delta = delta.filter(|d| *d != 0)?;
    let color = if delta > 0 {
        t.color_new
    } else {
        t.color_deleted
    };
    Some(chip(signed(delta), color, color.opacity(0.5)))
}

impl Rows {
    fn row(&self, ix: usize, cx: &mut App) -> AnyElement {
        match self.rows.get(ix).copied() {
            Some(Row::Header(section)) => self.header(("compiled-group", ix), section, cx),
            Some(Row::Fact(fix)) => self.fact(ix, fix, cx),
            Some(Row::Item(gix, rix)) => self.item(ix, gix, rix, cx),
            Some(Row::Hidden(gix)) => self.hidden(gix, cx),
            None => div().into_any_element(),
        }
    }

    fn header(&self, id: impl Into<ElementId>, section: Section, cx: &App) -> AnyElement {
        let t = cx.ghd();
        let collapsed = !self.filtering && self.collapsed.contains(&section);
        let view = self.view.clone();
        let (title, counts): (String, Vec<(CompiledStatus, usize)>) = match section {
            Section::Facts => {
                let changed = self
                    .data
                    .facts
                    .iter()
                    .filter(|f| f.status() != CompiledStatus::Unchanged)
                    .count();
                ("Header".into(), vec![(CompiledStatus::Changed, changed)])
            }
            Section::Group(gix) => {
                let group = &self.data.groups[gix];
                (
                    group.kind.title().into(),
                    [
                        CompiledStatus::Added,
                        CompiledStatus::Removed,
                        CompiledStatus::Changed,
                    ]
                    .into_iter()
                    .map(|s| (s, group.count(s)))
                    .collect(),
                )
            }
        };
        let total = match section {
            Section::Facts => self.data.facts.len(),
            Section::Group(gix) => {
                let g = &self.data.groups[gix];
                g.old_count.max(g.new_count)
            }
        };
        let mut chips: Vec<AnyElement> = counts
            .into_iter()
            .filter(|(_, n)| *n > 0)
            .map(|(status, n)| {
                let color = status_color(status, t);
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(zpx(2.))
                    .text_size(FONT_SIZE_SM())
                    .font_weight(FontWeight::NORMAL)
                    .text_color(color)
                    .child(octicon(status_icon(status), color))
                    .child(format!("{} {}", count(n), status.label().to_lowercase()))
                    .into_any_element()
            })
            .collect();
        if chips.is_empty() {
            chips.push(
                div()
                    .flex_none()
                    .text_size(FONT_SIZE_SM())
                    .font_weight(FontWeight::NORMAL)
                    .text_color(t.text_secondary)
                    .child("no changes")
                    .into_any_element(),
            );
        }
        let label = if collapsed {
            format!("Expand {title}")
        } else {
            format!("Collapse {title}")
        };
        div()
            .id(id)
            .a11y_button(label)
            .w_full()
            .h(zpx(ROW))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING_HALF())
            .px(SPACING())
            .bg(t.box_alt_background)
            .border_b_1()
            .border_color(t.diff_border)
            .text_size(FONT_SIZE_SM())
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(t.text)
            .cursor_pointer()
            .hover(|d| d.bg(t.box_hover_background))
            .child(octicon(
                if collapsed {
                    Octicon::ChevronRight
                } else {
                    Octicon::ChevronDown
                },
                t.text_secondary,
            ))
            .child(title)
            .child(
                div()
                    .flex_none()
                    .px(zpx(6.))
                    .rounded_full()
                    .bg(t.list_item_badge_background)
                    .text_color(t.list_item_badge_text)
                    .text_size(zpx(10.))
                    .child(count(total)),
            )
            .children(chips)
            .control_focus(cx)
            .when(!self.filtering, |d| {
                d.on_click(move |_, _, cx| {
                    view.update(cx, |this, cx| this.toggle(section, cx)).ok();
                })
            })
            .into_any_element()
    }

    /// A value cell: old → new, or the one value.
    fn values(
        id: impl Into<ElementId>,
        old: Option<&str>,
        new: Option<&str>,
        mono: bool,
        t: &GhdTheme,
    ) -> Stateful<Div> {
        let font = |d: Div| {
            if mono {
                d.font_family(mono_font()).text_size(FONT_SIZE_SM())
            } else {
                d
            }
        };
        let tooltip = match (old, new) {
            (Some(a), Some(b)) if a != b => format!("{a} → {b}"),
            (Some(a), None) => format!("{a} (removed)"),
            (_, Some(b)) => b.to_string(),
            (None, None) => String::new(),
        };
        let cell = div()
            .id(id)
            .flex_1()
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .gap(zpx(4.))
            .overflow_hidden();
        let cell = match (old, new) {
            (Some(a), Some(b)) if a != b => cell
                .child(
                    font(div())
                        .flex_shrink(1.)
                        .min_w(zpx(30.))
                        .truncate()
                        .text_color(t.text_secondary)
                        .line_through()
                        .child(a.to_string()),
                )
                .child(div().flex_none().text_color(t.text_secondary).child("→"))
                .child(
                    font(div())
                        .flex_shrink(1.)
                        .min_w(zpx(30.))
                        .truncate()
                        .text_color(t.text)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(b.to_string()),
                ),
            (Some(a), None) => cell.child(
                font(div())
                    .truncate()
                    .text_color(t.text_secondary)
                    .line_through()
                    .child(a.to_string()),
            ),
            (_, Some(b)) => cell.child(
                font(div())
                    .truncate()
                    .text_color(t.text)
                    .child(b.to_string()),
            ),
            (None, None) => cell,
        };
        cell.ghd_tooltip(tooltip)
    }

    fn fact(&self, ix: usize, fix: usize, cx: &App) -> AnyElement {
        let t = cx.ghd();
        let Some(fact) = self.data.facts.get(fix) else {
            return div().into_any_element();
        };
        let status = fact.status();
        let CompiledFact {
            label,
            old,
            new,
            delta,
            bytes: is_bytes,
        } = fact;
        let mono = matches!(
            label.split(" (").next().unwrap_or(""),
            "Entry point" | "UUID" | "Build ID" | "Source hash" | "Magic number"
        );
        div()
            .id(("compiled-fact", ix))
            .w_full()
            .h(zpx(ROW))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING_HALF())
            .pl(SPACING() + zpx(16.))
            .pr(SPACING())
            .border_b_1()
            .border_color(t.diff_border)
            .text_size(FONT_SIZE())
            .hover(|d| d.bg(t.list_item_hover_background))
            .child(
                div()
                    .flex_none()
                    .w(zpx(LABEL_W))
                    .truncate()
                    .text_color(t.text_secondary)
                    .child(label.clone()),
            )
            .child(Self::values(
                ("compiled-fact-value", ix),
                old.as_deref(),
                new.as_deref(),
                mono,
                t,
            ))
            .children(if *is_bytes {
                delta_chip(*delta, t)
            } else {
                delta.map(|d| chip(signed(d), t.text_secondary, t.box_border))
            })
            .when(status != CompiledStatus::Unchanged, |d| {
                d.child(chip(
                    status.label(),
                    status_color(status, t),
                    status_color(status, t).opacity(0.5),
                ))
            })
            .into_any_element()
    }

    fn size_cell(row: &CompiledRow, t: &GhdTheme) -> Div {
        let text = match (row.old_size, row.new_size) {
            (Some(a), Some(b)) if a != b => format!("{} → {}", bytes(a), bytes(b)),
            (Some(a), None) => bytes(a),
            (_, Some(b)) => bytes(b),
            (None, None) => String::new(),
        };
        div()
            .flex_none()
            .w(zpx(SIZE_W))
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .gap(zpx(4.))
            .child(
                div()
                    .flex_shrink(1.)
                    .min_w_0()
                    .truncate()
                    .font_family(mono_font())
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(text),
            )
            .children(delta_chip(row.size_delta(), t))
    }

    fn item(&self, ix: usize, gix: usize, rix: usize, cx: &App) -> AnyElement {
        let t = cx.ghd().clone();
        let Some(group) = self.data.groups.get(gix) else {
            return div().into_any_element();
        };
        let Some(row) = group.rows.get(rix) else {
            return div().into_any_element();
        };
        let kind = group.kind;
        let status = row.status;
        let removed = status == CompiledStatus::Removed;
        let unchanged = status == CompiledStatus::Unchanged;
        let mono = !matches!(
            kind,
            CompiledGroupKind::Libraries
                | CompiledGroupKind::RunPaths
                | CompiledGroupKind::Interfaces
        );
        let name = div()
            .id(("compiled-name", ix))
            .flex_shrink(1.)
            .min_w(zpx(60.))
            .truncate()
            .when(mono, |d| {
                d.font_family(mono_font()).text_size(FONT_SIZE_SM())
            })
            .when(!mono, |d| d.font_weight(FontWeight::SEMIBOLD))
            .text_color(if unchanged { t.text_secondary } else { t.text })
            .when(removed, |d| d.text_color(t.text_secondary).line_through())
            .child(row.name.clone())
            .ghd_tooltip(row.name.clone());
        let detail = (!row.detail.is_empty() || row.old_detail.is_some()).then(|| {
            let text = match &row.old_detail {
                Some(old) if old.is_empty() => format!("{} (was none)", row.detail),
                Some(old) => format!("{old} → {}", row.detail),
                None => row.detail.clone(),
            };
            div()
                .id(("compiled-detail", ix))
                .flex_shrink(4.)
                .min_w_0()
                .truncate()
                .text_size(FONT_SIZE_SM())
                .text_color(if row.old_detail.is_some() {
                    t.color_modified
                } else {
                    t.text_secondary
                })
                .child(text.clone())
                .ghd_tooltip(text)
        });
        let body = row.body_changed.then(|| {
            chip(
                kind.body_word(),
                t.color_renamed,
                t.color_renamed.opacity(0.5),
            )
        });
        let size = kind.has_size().then(|| Self::size_cell(row, &t));
        let copy = copy_text(row);
        let name_copy = row.name.clone();
        div()
            .id(("compiled-row", ix))
            .w_full()
            .h(zpx(ROW))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING_HALF())
            .pl(SPACING() + zpx(16.))
            .pr(SPACING())
            .border_b_1()
            .border_color(t.diff_border)
            .text_size(FONT_SIZE())
            .hover(|d| d.bg(t.list_item_hover_background))
            .child(octicon(status_icon(status), status_color(status, &t)))
            .child(name)
            .children(detail)
            .children(body)
            .child(div().flex_1())
            .children(size)
            .on_mouse_down(
                MouseButton::Right,
                move |ev: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    use crate::context_menu::MenuItem;
                    let copy = copy.clone();
                    let name = name_copy.clone();
                    let items = vec![
                        MenuItem::new("Copy name", move |_, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(name.clone()));
                        }),
                        MenuItem::new("Copy change", move |_, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                        }),
                    ];
                    crate::native_menu::show_context_menu(items, ev.position, window, cx);
                },
            )
            .into_any_element()
    }

    fn hidden(&self, gix: usize, cx: &App) -> AnyElement {
        let t = cx.ghd();
        let n = self.data.groups.get(gix).map_or(0, |g| g.unchanged_hidden);
        div()
            .w_full()
            .h(zpx(ROW))
            .flex_none()
            .flex()
            .items_center()
            .pl(SPACING() + zpx(16.))
            .border_b_1()
            .border_color(t.diff_border)
            .text_size(FONT_SIZE_SM())
            .text_color(t.text_secondary)
            .child(format!("{} more unchanged not listed", count(n)))
            .into_any_element()
    }
}

impl CompiledDiffView {
    fn header_strip(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.ghd().clone();
        let [added, removed, changed] = self.data.counts();
        let pills: Vec<AnyElement> = [
            (CompiledStatus::Added, added),
            (CompiledStatus::Removed, removed),
            (CompiledStatus::Changed, changed),
        ]
        .into_iter()
        .filter(|(_, n)| *n > 0)
        .map(|(status, n)| {
            let color = status_color(status, &t);
            div()
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(zpx(3.))
                .px(SPACING_HALF())
                .h(zpx(20.))
                .text_size(FONT_SIZE_SM())
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color)
                .child(octicon(status_icon(status), color))
                .child(format!("{} {}", count(n), status.label().to_lowercase()))
                .into_any_element()
        })
        .collect();
        let any_rows = self.data.groups.iter().any(|g| !g.rows.is_empty());
        let filter = any_rows.then(|| {
            let input = self.ensure_filter(window, cx);
            let clear = input.clone();
            div()
                .flex_none()
                .w(zpx(200.))
                .on_key_down(move |ev: &KeyDownEvent, window, cx| {
                    if ev.keystroke.key == "escape" && !clear.read(cx).value().is_empty() {
                        cx.stop_propagation();
                        clear.update(cx, |input, cx| input.set_value("", window, cx));
                    }
                })
                .child(crate::widgets::filter_text_box(
                    "compiled-filter",
                    &input,
                    Some(octicon(Octicon::Filter, t.text_secondary)),
                    window,
                    cx,
                ))
        });
        let show = self.show_unchanged;
        let weak = cx.weak_entity();
        let any_unchanged = self
            .data
            .groups
            .iter()
            .any(|g| g.count(CompiledStatus::Unchanged) > 0);
        let toggle = any_unchanged.then(|| {
            div()
                .id("compiled-show-unchanged")
                .a11y_button("Show unchanged")
                .flex_none()
                .h(zpx(22.))
                .px(SPACING_HALF())
                .flex()
                .items_center()
                .rounded(BORDER_RADIUS())
                .border_1()
                .text_size(FONT_SIZE_SM())
                .cursor_pointer()
                .when(show, |d| {
                    d.bg(t.box_selected_active_background)
                        .border_color(t.box_selected_active_background)
                        .text_color(t.box_selected_active_text)
                })
                .when(!show, |d| {
                    d.border_color(t.box_border)
                        .text_color(t.text)
                        .hover(|d| d.bg(t.box_hover_background))
                })
                .child("Show unchanged")
                .ghd_tooltip(if show {
                    "List only what changed"
                } else {
                    "List the parts that did not change too"
                })
                .control_focus(cx)
                .on_click(move |_, _, cx| {
                    weak.update(cx, |this, cx| this.set_show_unchanged(!show, cx))
                        .ok();
                })
        });
        let title = match (&self.data.old_title, &self.data.new_title) {
            (Some(a), Some(b)) if a != b => format!("{a} → {b}"),
            (_, Some(b)) => b.clone(),
            (Some(a), None) => a.clone(),
            (None, None) => "Compiled file".into(),
        };
        div()
            .flex_none()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(SPACING_HALF())
            .min_h(zpx(30.))
            .px(SPACING())
            .py(zpx(4.))
            .bg(t.box_alt_background)
            .border_b_1()
            .border_color(t.diff_border)
            .child(octicon(family_icon(self.data.family), t.text_secondary))
            .child(
                div()
                    .id("compiled-title")
                    .flex_shrink(1.)
                    .min_w(zpx(80.))
                    .max_w(zpx(420.))
                    .truncate()
                    .text_size(FONT_SIZE_SM())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.text)
                    .child(title.clone())
                    .ghd_tooltip(title),
            )
            .children(pills)
            .child(div().flex_1())
            .children(toggle)
            .children(filter)
            .into_any_element()
    }

    fn empty(&self, text: &'static str, cx: &App) -> AnyElement {
        let t = cx.ghd();
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(SPACING_HALF())
            .p(SPACING_DOUBLE())
            .text_color(t.text_secondary)
            .child(octicon(family_icon(self.data.family), t.text_secondary).size(zpx(24.)))
            .child(text)
            .into_any_element()
    }
}

impl Render for CompiledDiffView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd().clone();
        let header = self.header_strip(window, cx);
        let filtering = !self.filter_text.trim().is_empty();
        let shared = Rc::new(Rows {
            data: self.data.clone(),
            rows: self.rows.clone(),
            collapsed: self.collapsed.clone(),
            filtering,
            view: cx.weak_entity(),
        });
        let body = if self.rows.is_empty() {
            self.empty(
                if filtering {
                    "Nothing matches the filter."
                } else {
                    "The file could not be compared."
                },
                cx,
            )
        } else {
            let sticky = self.sticky.and_then(|section| {
                self.rows
                    .contains(&Row::Header(section))
                    .then(|| shared.header("compiled-sticky", section, cx))
            });
            let rows = shared.clone();
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(
                    list(self.list.clone(), move |ix, _window, cx| rows.row(ix, cx))
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .pr(gutter(&self.list)),
                )
                .children(sticky.map(|h| {
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right(gutter(&self.list))
                        .child(h)
                }))
                .child(scrollbar("compiled-scrollbar", self.list.clone()))
                .into_any_element()
        };
        div()
            .id("compiled-diff")
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(t.background)
            .text_color(t.text)
            .text_size(FONT_SIZE())
            .child(header)
            .child(body)
    }
}
