//! Corvene `1338-lockfile-diff`: a package manager lockfile's diff read as
//! dependency changes. A strip on top names the lockfile kind and counts the
//! upgraded, downgraded, added, removed and changed packages, with a filter
//! and a "Direct only" toggle; under it the packages are listed in groups
//! (Upgraded, Downgraded, Added, Removed, Changed), each folding on a click
//! of its header. A row shows the version step with the changed part of the
//! new version marked, a major / minor / patch badge, the dev / optional /
//! peer tags, a source move (registry to git), which packages need a
//! transitive one, and links to the registry page and the git comparison.
//! A checksum that changed under the same version is a warning row.
//!
//! GitHub Desktop shows lockfiles as a text diff only
//! (`app/src/ui/diff/index.tsx`); the diff header's Dependencies / Source
//! switch goes back to it.

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use corvene_core::{Diff, Dispatcher};
use corvene_models::lockfile::{
    Bump, ChangeKind, LockfileDiff, PackageChange, PackageSource, short_rev,
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

/// A package row's height (CSS px), dense like GitHub Desktop's lists.
const ROW: f32 = 28.;
/// A group header's height (CSS px).
const HEADER: f32 = 26.;
/// Names listed in a "required by" before "+N".
const REQUIRED_BY_SHOWN: usize = 2;
/// Names in the "required by" tooltip before "and N more".
const REQUIRED_BY_TOOLTIP: usize = 30;
/// How long a copied row shows its check mark.
const COPIED_FOR: Duration = Duration::from_secs(2);

const WARNING_TOOLTIP: &str = "The checksum changed but the version did not. The package may \
                               have been republished or tampered with.";

/// One row of the list.
#[derive(Clone, Copy)]
enum Row {
    /// A group's header: its kind, rows shown and rows in the group.
    Header {
        kind: ChangeKind,
        shown: usize,
        total: usize,
    },
    /// A package (index into `changes`).
    Item(usize),
}

/// The lockfile diff view.
pub struct LockfileDiffView {
    diff: Arc<Diff>,
    data: Arc<LockfileDiff>,
    /// Lower-case package names, for the filter.
    lower: Vec<String>,
    /// Package indexes per kind, in [`ChangeKind::ALL`] order.
    by_kind: [Vec<usize>; 5],
    /// Whether the lockfile says which packages are direct.
    any_direct: bool,
    rows: Rc<Vec<Row>>,
    list: ListState,
    collapsed: HashSet<ChangeKind>,
    direct_only: bool,
    filter: Option<Entity<InputState>>,
    filter_text: String,
    /// The group whose header is pinned over the top of the list.
    sticky: Option<ChangeKind>,
    /// The row last copied, while it shows its check mark.
    copied: Option<usize>,
    copy_generation: u64,
}

fn kind_index(kind: ChangeKind) -> usize {
    match kind {
        ChangeKind::Upgraded => 0,
        ChangeKind::Downgraded => 1,
        ChangeKind::Added => 2,
        ChangeKind::Removed => 3,
        ChangeKind::Changed => 4,
    }
}

fn kind_color(kind: ChangeKind, t: &GhdTheme) -> Hsla {
    match kind {
        ChangeKind::Upgraded => t.color_new,
        ChangeKind::Downgraded => t.color_modified,
        ChangeKind::Added => t.color_new,
        ChangeKind::Removed => t.color_deleted,
        ChangeKind::Changed => t.color_renamed,
    }
}

fn kind_icon(kind: ChangeKind) -> Octicon {
    match kind {
        ChangeKind::Upgraded => Octicon::ArrowUp,
        ChangeKind::Downgraded => Octicon::ArrowDown,
        ChangeKind::Added => Octicon::DiffAdded,
        ChangeKind::Removed => Octicon::DiffRemoved,
        ChangeKind::Changed => Octicon::DiffModified,
    }
}

fn bump_color(bump: Bump, t: &GhdTheme) -> Hsla {
    match bump {
        Bump::Major => t.color_deleted,
        Bump::Minor => t.color_modified,
        Bump::Patch => t.color_new,
        Bump::Prerelease => t.color_renamed,
        Bump::Other => t.text_secondary,
    }
}

fn bump_tooltip(bump: Bump) -> &'static str {
    match bump {
        Bump::Major => "Major version change: may break",
        Bump::Minor => "Minor version change: new features",
        Bump::Patch => "Patch version change: fixes",
        Bump::Prerelease => "Only the pre-release part changed",
        Bump::Other => "Versions that do not compare as numbers",
    }
}

/// Where `new` starts to differ from `old`, moved back to the start of
/// that version part (`1.2.3 → 1.4.0` marks `4.0`).
fn changed_from(old: &str, new: &str) -> usize {
    let common = old
        .char_indices()
        .zip(new.chars())
        .find(|((_, a), b)| a != b)
        .map_or(old.len().min(new.len()), |((ix, _), _)| ix);
    let common = if new.is_char_boundary(common) {
        common
    } else {
        0
    };
    new[..common]
        .rfind(['.', '-', '+', '_', '@'])
        .map_or(0, |ix| ix + 1)
}

/// A source in a few characters: "registry", "git 1a2b3c4", "path".
fn short_source(source: &PackageSource) -> String {
    match &source.rev {
        Some(rev) => format!("{} {}", source.kind.label(), short_rev(rev)),
        None => source.kind.label().to_string(),
    }
}

/// "name old → new", the row's copied text.
fn copy_text(change: &PackageChange) -> String {
    let version = change.version_label();
    if version.is_empty() {
        change.name.clone()
    } else {
        format!("{} {version}", change.name)
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!(
        "{} {}",
        crate::format::format_count(n as u64),
        if n == 1 { one } else { many }
    )
}

impl LockfileDiffView {
    /// `diff` is a `Diff::Lockfile`; any other diff shows as empty.
    pub fn new(
        diff: Arc<Diff>,
        context: crate::diff_view::InspectContext,
        cx: &mut Context<Self>,
    ) -> Self {
        let _ = context;
        let data = match &*diff {
            Diff::Lockfile { diff, .. } => diff.clone(),
            _ => Arc::new(LockfileDiff::default()),
        };
        let lower = data.changes.iter().map(|c| c.name.to_lowercase()).collect();
        let mut by_kind: [Vec<usize>; 5] = Default::default();
        for (ix, change) in data.changes.iter().enumerate() {
            by_kind[kind_index(change.kind)].push(ix);
        }
        let any_direct = data.changes.iter().any(|c| c.direct.is_some());
        let list = ListState::new(0, ListAlignment::Top, zpx(400.));
        let weak = cx.weak_entity();
        list.set_scroll_handler(move |_, _, cx| {
            weak.update(cx, |this, cx| {
                let sticky = this.sticky_kind();
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
            by_kind,
            any_direct,
            rows: Rc::new(Vec::new()),
            list,
            collapsed: HashSet::new(),
            direct_only: false,
            filter: None,
            filter_text: String::new(),
            sticky: None,
            copied: None,
            copy_generation: 0,
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
        let mut rows = Vec::new();
        for kind in ChangeKind::ALL {
            let all = &self.by_kind[kind_index(kind)];
            let shown: Vec<usize> = all
                .iter()
                .copied()
                .filter(|&ix| {
                    (!self.direct_only || self.data.changes[ix].direct == Some(true))
                        && (needle.is_empty() || self.lower[ix].contains(&needle))
                })
                .collect();
            if shown.is_empty() {
                continue;
            }
            rows.push(Row::Header {
                kind,
                shown: shown.len(),
                total: all.len(),
            });
            // a filter shows every match
            if needle.is_empty() && self.collapsed.contains(&kind) {
                continue;
            }
            rows.extend(shown.into_iter().map(Row::Item));
        }
        // keep the row at the top where it was (a folded row's group header)
        let top = self.list.logical_scroll_top();
        let anchor = self.rows.get(top.item_ix).copied();
        self.list.reset_with_uniform_height(rows.len(), zpx(ROW));
        let same = |a: &Row, b: &Row| match (a, b) {
            (Row::Header { kind: x, .. }, Row::Header { kind: y, .. }) => x == y,
            (Row::Item(x), Row::Item(y)) => x == y,
            _ => false,
        };
        if let Some(anchor) = anchor {
            let at = rows.iter().position(|r| same(r, &anchor));
            let at = at.map(|ix| (ix, top.offset_in_item)).or_else(|| {
                let Row::Item(item) = anchor else {
                    return None;
                };
                let kind = self.data.changes[item].kind;
                rows.iter()
                    .position(|r| matches!(r, Row::Header { kind: k, .. } if *k == kind))
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
        self.sticky = self.sticky_kind();
    }

    /// The group of the row at the top of the list, unless that row is the
    /// group's own header in full view.
    fn sticky_kind(&self) -> Option<ChangeKind> {
        let top = self.list.logical_scroll_top();
        match self.rows.get(top.item_ix)? {
            Row::Header { kind, .. } => (top.offset_in_item > px(0.)).then_some(*kind),
            Row::Item(ix) => Some(self.data.changes[*ix].kind),
        }
    }

    fn toggle_group(&mut self, kind: ChangeKind, cx: &mut Context<Self>) {
        if !self.collapsed.remove(&kind) {
            self.collapsed.insert(kind);
        }
        self.build_rows();
        cx.notify();
    }

    fn reveal_group(&mut self, kind: ChangeKind, cx: &mut Context<Self>) {
        if let Some(ix) = self
            .rows
            .iter()
            .position(|r| matches!(r, Row::Header { kind: k, .. } if *k == kind))
        {
            self.list.scroll_to(ListOffset {
                item_ix: ix,
                offset_in_item: px(0.),
            });
            self.sticky = self.sticky_kind();
            cx.notify();
        }
    }

    fn set_direct_only(&mut self, on: bool, cx: &mut Context<Self>) {
        self.direct_only = on;
        self.build_rows();
        cx.notify();
    }

    fn copy(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(change) = self.data.changes.get(ix) else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(copy_text(change)));
        self.copied = Some(ix);
        self.copy_generation += 1;
        let generation = self.copy_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_FOR).await;
            this.update(cx, |this, cx| {
                if this.copy_generation == generation {
                    this.copied = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn ensure_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(filter) = &self.filter {
            return filter.clone();
        }
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter packages"));
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

/// What a row draws from, shared with the list's row callback.
struct Rows {
    data: Arc<LockfileDiff>,
    rows: Rc<Vec<Row>>,
    collapsed: HashSet<ChangeKind>,
    filtering: bool,
    copied: Option<usize>,
    view: WeakEntity<LockfileDiffView>,
}

/// A small bordered label ("major", "dev").
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

/// A square icon button shown on a row's hover.
fn row_button(
    id: impl Into<ElementId>,
    icon: Octicon,
    label: &'static str,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.ghd();
    div()
        .id(id)
        .icon_button_label(label)
        .flex_none()
        .size(zpx(22.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(BORDER_RADIUS())
        .cursor_pointer()
        .hover(|d| d.bg(t.box_hover_background))
        .child(octicon(icon, t.text_secondary))
        .control_focus(cx)
}

impl Rows {
    fn row(&self, ix: usize, cx: &mut App) -> AnyElement {
        match self.rows.get(ix).copied() {
            Some(Row::Header { kind, shown, total }) => {
                self.header(("lockfile-group", ix), kind, shown, total, cx)
            }
            Some(Row::Item(item)) => self.item(ix, item, cx),
            None => div().into_any_element(),
        }
    }

    fn header(
        &self,
        id: impl Into<ElementId>,
        kind: ChangeKind,
        shown: usize,
        total: usize,
        cx: &App,
    ) -> AnyElement {
        let t = cx.ghd();
        let collapsed = !self.filtering && self.collapsed.contains(&kind);
        let view = self.view.clone();
        let count = if shown == total {
            crate::format::format_count(total as u64)
        } else {
            format!(
                "{} of {}",
                crate::format::format_count(shown as u64),
                crate::format::format_count(total as u64)
            )
        };
        let label = if collapsed {
            format!("Expand {}", kind.label())
        } else {
            format!("Collapse {}", kind.label())
        };
        div()
            .id(id)
            .a11y_button(label)
            .w_full()
            .h(zpx(HEADER))
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
            .child(octicon(kind_icon(kind), kind_color(kind, t)))
            .child(kind.label())
            .child(
                div()
                    .flex_none()
                    .px(zpx(6.))
                    .rounded_full()
                    .bg(t.list_item_badge_background)
                    .text_color(t.list_item_badge_text)
                    .text_size(zpx(10.))
                    .child(count),
            )
            .control_focus(cx)
            .when(!self.filtering, |d| {
                d.on_click(move |_, _, cx| {
                    view.update(cx, |this, cx| this.toggle_group(kind, cx)).ok();
                })
            })
            .into_any_element()
    }

    /// The version step: old, arrow, new with its changed part marked.
    fn version(&self, change: &PackageChange, t: &GhdTheme) -> Option<Div> {
        let mono = |d: Div| d.font_family(mono_font()).text_size(FONT_SIZE_SM());
        let color = kind_color(change.kind, t);
        let base = mono(
            div()
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .max_w(zpx(260.))
                .overflow_hidden(),
        );
        match (&change.old_version, &change.new_version) {
            (Some(old), Some(new)) if old != new => {
                let from = changed_from(old, new);
                Some(
                    base.child(div().text_color(t.text_secondary).child(old.clone()))
                        .child(div().px(zpx(4.)).text_color(t.text_secondary).child("→"))
                        .child(div().text_color(t.text).child(new[..from].to_string()))
                        .child(
                            div()
                                .text_color(color)
                                .font_weight(FontWeight::BOLD)
                                .child(new[from..].to_string()),
                        ),
                )
            }
            (_, Some(new)) => Some(base.text_color(t.text).child(new.clone())),
            (Some(old), None) => Some(
                base.text_color(t.text_secondary)
                    .line_through()
                    .child(old.clone()),
            ),
            (None, None) => None,
        }
    }

    fn item(&self, ix: usize, item: usize, cx: &mut App) -> AnyElement {
        let t = cx.ghd().clone();
        let Some(change) = self.data.changes.get(item) else {
            return div().into_any_element();
        };
        let warning = change.is_warning();
        let removed = change.kind == ChangeKind::Removed;
        let tooltip = copy_text(change);

        let icon = if warning {
            div()
                .id(("lockfile-warning", ix))
                .flex_none()
                .child(octicon(Octicon::Alert, t.file_warning))
                .ghd_tooltip(WARNING_TOOLTIP)
                .into_any_element()
        } else {
            octicon(kind_icon(change.kind), kind_color(change.kind, &t)).into_any_element()
        };

        let name = div()
            .id(("lockfile-name", ix))
            .flex_shrink(1.)
            .min_w(zpx(40.))
            .truncate()
            .font_weight(FontWeight::SEMIBOLD)
            .when(removed, |d| d.text_color(t.text_secondary).line_through())
            .child(change.name.clone())
            .ghd_tooltip(tooltip);

        let bump = change.bump.map(|b| {
            let color = bump_color(b, &t);
            chip(b.label(), color, color.opacity(0.5))
                .id(("lockfile-bump", ix))
                .ghd_tooltip(bump_tooltip(b))
        });

        let tags: Vec<AnyElement> = change
            .tags()
            .into_iter()
            .filter(|tag| *tag != "transitive")
            .map(|tag| chip(tag, t.text_secondary, t.box_border).into_any_element())
            .collect();

        let source = if change.source_changed() {
            let (old, new) = (
                change
                    .old_source
                    .as_ref()
                    .map(short_source)
                    .unwrap_or_default(),
                change
                    .new_source
                    .as_ref()
                    .map(short_source)
                    .unwrap_or_default(),
            );
            let full = format!(
                "{} → {}",
                change
                    .old_source
                    .as_ref()
                    .map(PackageSource::label)
                    .unwrap_or_default(),
                change
                    .new_source
                    .as_ref()
                    .map(PackageSource::label)
                    .unwrap_or_default()
            );
            Some(
                div()
                    .id(("lockfile-source", ix))
                    .flex_none()
                    .max_w(zpx(220.))
                    .truncate()
                    .font_family(mono_font())
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.color_renamed)
                    .child(format!("{old} → {new}"))
                    .ghd_tooltip(full),
            )
        } else {
            None
        };

        let checksum = (warning && !change.source_changed()).then(|| {
            div()
                .id(("lockfile-checksum", ix))
                .flex_none()
                .text_size(FONT_SIZE_SM())
                .text_color(t.file_warning)
                .child("checksum changed")
                .ghd_tooltip(WARNING_TOOLTIP)
        });

        let required_by =
            (change.direct != Some(true) && !change.required_by.is_empty()).then(|| {
                let by = &change.required_by;
                let mut text = by
                    .iter()
                    .take(REQUIRED_BY_SHOWN)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ");
                if by.len() > REQUIRED_BY_SHOWN {
                    text.push_str(&format!(", +{}", by.len() - REQUIRED_BY_SHOWN));
                }
                let mut full = by
                    .iter()
                    .take(REQUIRED_BY_TOOLTIP)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ");
                if by.len() > REQUIRED_BY_TOOLTIP {
                    full.push_str(&format!(" and {} more", by.len() - REQUIRED_BY_TOOLTIP));
                }
                div()
                    .id(("lockfile-required-by", ix))
                    .flex_shrink(4.)
                    .min_w_0()
                    .truncate()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(format!("required by {text}"))
                    .ghd_tooltip(format!("Required by {full}"))
            });

        let registry = change.registry_url();
        let compare = change.compare_url();
        let copied = self.copied == Some(item);
        let group = SharedString::from(format!("lockfile-row-{ix}"));
        let mut buttons = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(zpx(2.))
            .ml(SPACING_HALF())
            .opacity(if copied { 1. } else { 0. })
            .group_hover(group.clone(), |s| s.opacity(1.));
        if let Some(url) = compare.clone() {
            buttons = buttons.child(
                row_button(
                    ("lockfile-compare", ix),
                    Octicon::GitCompare,
                    "Compare revisions",
                    cx,
                )
                .on_click(move |_, _, cx| Dispatcher::open_url(&url, cx)),
            );
        }
        if let Some(url) = registry.clone() {
            buttons = buttons.child(
                row_button(
                    ("lockfile-open", ix),
                    Octicon::LinkExternal,
                    "Open package page",
                    cx,
                )
                .on_click(move |_, _, cx| Dispatcher::open_url(&url, cx)),
            );
        }
        let view = self.view.clone();
        buttons = buttons.child(
            row_button(
                ("lockfile-copy", ix),
                if copied {
                    Octicon::Check
                } else {
                    Octicon::Copy
                },
                if copied {
                    "Copied!"
                } else {
                    "Copy package change"
                },
                cx,
            )
            .on_click(move |_, _, cx| {
                view.update(cx, |this, cx| this.copy(item, cx)).ok();
            }),
        );

        let view = self.view.clone();
        let name_copy = change.name.clone();
        let menu_compare = compare;
        let menu_registry = registry;
        let version = self.version(change, &t);
        div()
            .id(("lockfile-row", ix))
            .group(group)
            .w_full()
            .h(zpx(ROW))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING_HALF())
            .pl(SPACING() + zpx(16.))
            .pr(SPACING_HALF())
            .border_b_1()
            .border_color(t.diff_border)
            .text_size(FONT_SIZE())
            .text_color(t.text)
            .when(warning, |d| d.bg(t.file_warning_background))
            .when(!warning, |d| {
                d.hover(|d| d.bg(t.list_item_hover_background))
            })
            .child(icon)
            .child(name)
            .children(version)
            .children(bump)
            .children(tags)
            .children(source)
            .children(checksum)
            .children(required_by)
            .child(div().flex_1())
            .child(buttons)
            .on_mouse_down(
                MouseButton::Right,
                move |ev: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    use crate::context_menu::MenuItem;
                    let mut items = Vec::new();
                    let view = view.clone();
                    items.push(MenuItem::new("Copy package change", move |_, cx| {
                        view.update(cx, |this, cx| this.copy(item, cx)).ok();
                    }));
                    let name = name_copy.clone();
                    items.push(MenuItem::new("Copy package name", move |_, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(name.clone()));
                    }));
                    if menu_registry.is_some() || menu_compare.is_some() {
                        items.push(MenuItem::separator());
                    }
                    if let Some(url) = menu_registry.clone() {
                        items.push(MenuItem::new("Open package page", move |_, cx| {
                            Dispatcher::open_url(&url, cx)
                        }));
                    }
                    if let Some(url) = menu_compare.clone() {
                        items.push(MenuItem::new("Compare revisions", move |_, cx| {
                            Dispatcher::open_url(&url, cx)
                        }));
                    }
                    crate::native_menu::show_context_menu(items, ev.position, window, cx);
                },
            )
            .into_any_element()
    }
}

impl LockfileDiffView {
    /// The notes about the file itself, format version change first.
    fn notes(&self) -> Vec<String> {
        let mut notes = Vec::new();
        if let Some(note) = self.data.format_version_note()
            && !self.data.notes.contains(&note)
        {
            notes.push(note);
        }
        notes.extend(self.data.notes.iter().cloned());
        notes
    }

    fn header_strip(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.ghd().clone();
        let counts = self.data.counts();
        let weak = cx.weak_entity();
        let pill = |kind: ChangeKind, n: usize, cx: &App| {
            let weak = weak.clone();
            let color = kind_color(kind, &t);
            div()
                .id(("lockfile-count", kind_index(kind)))
                .a11y_button(format!("{n} {}", kind.word()))
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(zpx(3.))
                .px(SPACING_HALF())
                .h(zpx(20.))
                .rounded(BORDER_RADIUS())
                .text_size(FONT_SIZE_SM())
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color)
                .cursor_pointer()
                .hover(|d| d.bg(t.box_hover_background))
                .child(octicon(kind_icon(kind), color))
                .child(format!(
                    "{} {}",
                    crate::format::format_count(n as u64),
                    kind.word()
                ))
                .ghd_tooltip(format!("Go to {}", kind.label()))
                .control_focus(cx)
                .on_click(move |_, _, cx| {
                    weak.update(cx, |this, cx| this.reveal_group(kind, cx)).ok();
                })
                .into_any_element()
        };
        let mut pills: Vec<AnyElement> = Vec::new();
        for (kind, n) in [
            (ChangeKind::Upgraded, counts.upgraded),
            (ChangeKind::Downgraded, counts.downgraded),
            (ChangeKind::Added, counts.added),
            (ChangeKind::Removed, counts.removed),
            (ChangeKind::Changed, counts.changed),
        ] {
            if n > 0 {
                pills.push(pill(kind, n, cx));
            }
        }
        if counts.integrity_warnings > 0 {
            pills.push(
                div()
                    .id("lockfile-warnings")
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(zpx(3.))
                    .px(SPACING_HALF())
                    .text_size(FONT_SIZE_SM())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.file_warning)
                    .child(octicon(Octicon::Alert, t.file_warning))
                    .child(plural(
                        counts.integrity_warnings,
                        "checksum warning",
                        "checksum warnings",
                    ))
                    .ghd_tooltip(WARNING_TOOLTIP)
                    .into_any_element(),
            );
        }
        if counts.unchanged > 0 {
            pills.push(
                div()
                    .flex_none()
                    .px(SPACING_HALF())
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(format!(
                        "{} unchanged",
                        crate::format::format_count(counts.unchanged as u64)
                    ))
                    .into_any_element(),
            );
        }

        let has_changes = !self.data.changes.is_empty();
        let filter = has_changes.then(|| {
            let input = self.ensure_filter(window, cx);
            let clear = input.clone();
            div()
                .flex_none()
                .w(zpx(170.))
                .on_key_down(move |ev: &KeyDownEvent, window, cx| {
                    if ev.keystroke.key == "escape" && !clear.read(cx).value().is_empty() {
                        cx.stop_propagation();
                        clear.update(cx, |input, cx| input.set_value("", window, cx));
                    }
                })
                .child(crate::widgets::filter_text_box(
                    "lockfile-filter",
                    &input,
                    Some(octicon(Octicon::Filter, t.text_secondary)),
                    window,
                    cx,
                ))
        });
        let direct_only = self.direct_only;
        let toggle = (has_changes && self.any_direct).then(|| {
            let weak = weak.clone();
            div()
                .id("lockfile-direct-only")
                .a11y_button("Direct only")
                .flex_none()
                .h(zpx(22.))
                .px(SPACING_HALF())
                .flex()
                .items_center()
                .rounded(BORDER_RADIUS())
                .border_1()
                .text_size(FONT_SIZE_SM())
                .cursor_pointer()
                .when(direct_only, |d| {
                    d.bg(t.box_selected_active_background)
                        .border_color(t.box_selected_active_background)
                        .text_color(t.box_selected_active_text)
                })
                .when(!direct_only, |d| {
                    d.border_color(t.box_border)
                        .text_color(t.text)
                        .hover(|d| d.bg(t.box_hover_background))
                })
                .child("Direct only")
                .ghd_tooltip(if direct_only {
                    "Show every package"
                } else {
                    "Show only the packages the project depends on itself"
                })
                .control_focus(cx)
                .on_click(move |_, _, cx| {
                    weak.update(cx, |this, cx| this.set_direct_only(!direct_only, cx))
                        .ok();
                })
        });

        let format = if self.data.format.is_empty() {
            "Lockfile".to_string()
        } else {
            self.data.format.clone()
        };
        let notes: Vec<AnyElement> = self
            .notes()
            .into_iter()
            .map(|note| {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING_HALF())
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(octicon(Octicon::Info, t.text_secondary))
                    .child(div().flex_1().min_w_0().truncate().child(note))
                    .into_any_element()
            })
            .collect();

        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(zpx(3.))
            .px(SPACING())
            .py(zpx(4.))
            .bg(t.box_alt_background)
            .border_b_1()
            .border_color(t.diff_border)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(SPACING_HALF())
                    .min_h(zpx(22.))
                    .child(octicon(Octicon::Package, t.text_secondary))
                    .child(
                        div()
                            .id("lockfile-format")
                            .flex_none()
                            .max_w(zpx(260.))
                            .truncate()
                            .text_size(FONT_SIZE_SM())
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.text)
                            .child(format!("{format} · {}", self.data.ecosystem.name()))
                            .ghd_tooltip(self.data.summary()),
                    )
                    .children(pills)
                    .child(div().flex_1())
                    .children(toggle)
                    .children(filter),
            )
            .children(notes)
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
            .child(octicon(Octicon::Package, t.text_secondary).size(zpx(24.)))
            .child(text)
            .into_any_element()
    }
}

impl Render for LockfileDiffView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd().clone();
        let header = self.header_strip(window, cx);
        let filtering = !self.filter_text.trim().is_empty();
        let shared = Rc::new(Rows {
            data: self.data.clone(),
            rows: self.rows.clone(),
            collapsed: self.collapsed.clone(),
            filtering,
            copied: self.copied,
            view: cx.weak_entity(),
        });
        let body = if self.data.changes.is_empty() {
            self.empty("Only the lockfile's formatting or metadata changed.", cx)
        } else if self.rows.is_empty() {
            self.empty(
                if filtering {
                    "No packages match the filter."
                } else {
                    "No direct dependencies changed."
                },
                cx,
            )
        } else {
            let sticky = self.sticky.and_then(|kind| {
                self.rows.iter().find_map(|r| match r {
                    Row::Header {
                        kind: k,
                        shown,
                        total,
                    } if *k == kind => {
                        Some(shared.header("lockfile-sticky", kind, *shown, *total, cx))
                    }
                    _ => None,
                })
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
                .child(scrollbar("lockfile-scrollbar", self.list.clone()))
                .into_any_element()
        };
        div()
            .id("lockfile-diff")
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
