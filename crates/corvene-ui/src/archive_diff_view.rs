//! Corvene `1323-archive-diff`: a changed archive shown as a folder. GitHub
//! Desktop shows an archive as a binary file (`app/src/ui/diff/
//! binary-file.tsx`, "This binary file has changed.").
//!
//! Layout, top to bottom:
//! - a summary bar: the format, how many entries were added, removed,
//!   modified and renamed, the archive's size before and after, a filter
//!   box and toggles for unchanged entries and the stacked view;
//! - a breadcrumb back out of archives opened inside the archive;
//! - a banner when a rebuilt archive holds the same contents (only times,
//!   permissions or compression changed);
//! - the entry tree (folders fold like the file lists' tree, `1310`) beside
//!   the selected entry: its header (status, path, what changed in its
//!   metadata, the Diff Settings gear, an actions menu) over its diff, a
//!   regular [`DiffView`] reading [`DiffSource::ArchiveEntry`].
//!
//! A single compressed file (`.gz`, …) skips the tree: its one file is
//! shown at once under a "decompressed from" bar.

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use corvene_core::archive_browse::{ArchiveBrowse, ArchiveSurface};
use corvene_core::file_tree::{FileTree, TreeRow};
use corvene_core::{
    AppState, ArchiveDiff, ArchiveEntryChange, ArchiveEntryKind, ArchiveEntryMeta,
    ArchiveEntryStatus, Diff, Dispatcher, FileStatusKind,
};
use corvene_git::archive::EntrySide;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::resizable::{
    ResizablePanelEvent, ResizableState, h_resizable, resizable_panel,
};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::actions::{
    ArchiveUp, CollapseFolder, ExpandFolder, OpenArchiveEntry, SelectFirstFile, SelectLastFile,
    SelectNextFile, SelectPreviousFile,
};
use crate::diff_view::{
    DiffSource, DiffView, archive_entry_kind, diff_options_button, status_icon,
};
use crate::icons::{Octicon, octicon};
use crate::scrollbar::ScrollbarExt;
use crate::stacked_diff_view::StackedDiffView;
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::{
    BORDER_RADIUS, FONT_SIZE, FONT_SIZE_SM, ROW_HEIGHT, SPACING, SPACING_DOUBLE, SPACING_HALF,
    unzoom, zpx,
};
use crate::widgets::{GhdTooltip, IconButtonA11y, ListRowA11y};

const TREE_MIN: f32 = 160.;
const TREE_MAX: f32 = 640.;
/// A filter box shows above this many entries.
const FILTER_FROM: usize = 12;

/// The entries shown and their tree, for one set of inputs.
struct EntryTree {
    /// Kept alive so its address is never another level's.
    level: Arc<ArchiveDiff>,
    show_unchanged: bool,
    show_metadata: bool,
    filter: String,
    collapsed_generation: u64,
    /// The entries listed (indexes into `level.entries`), in tree order of
    /// `tree.order`.
    items: Vec<usize>,
    tree: FileTree,
}

pub struct ArchiveDiffView {
    state: Entity<AppState>,
    surface: ArchiveSurface,
    entry_view: Entity<DiffView>,
    stacked: Entity<StackedDiffView>,
    resizable: Entity<ResizableState>,
    tree_width: Pixels,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    filter: Option<Entity<InputState>>,
    filter_text: String,
    /// Folded folders of the level shown (forgotten with the level).
    collapsed: HashSet<String>,
    collapsed_generation: u64,
    collapsed_level: Option<Arc<ArchiveDiff>>,
    /// A folder row the keyboard is on (entries are the selection itself).
    cursor_folder: Option<String>,
    tree: Option<Rc<EntryTree>>,
    /// The selection last scrolled to.
    scrolled_to: Option<String>,
}

/// Whether an entry is listed with these settings.
fn listed(e: &ArchiveEntryChange, level: &ArchiveDiff, unchanged: bool, metadata: bool) -> bool {
    // a folder entry with entries under it is the tree's folder row
    if e.is_dir() {
        let prefix = format!("{}/", e.path);
        let has_children = level
            .entries
            .binary_search_by(|x| x.path.as_str().cmp(prefix.as_str()))
            .map_or_else(
                |ix| {
                    level
                        .entries
                        .get(ix)
                        .is_some_and(|x| x.path.starts_with(&prefix))
                },
                |_| true,
            );
        if has_children {
            return false;
        }
    }
    match e.status {
        ArchiveEntryStatus::MetadataOnly => metadata || unchanged,
        ArchiveEntryStatus::Unchanged => unchanged,
        _ => true,
    }
}

/// The settings that decide which entries are listed.
fn list_settings(s: &AppState) -> (bool, bool) {
    (
        s.settings.archive_show_unchanged,
        s.settings.archive_show_metadata_only,
    )
}

/// The entries of `level` a file list or the view shows, in tree order.
pub fn listed_entries(level: &ArchiveDiff, unchanged: bool, metadata: bool) -> Vec<usize> {
    level
        .entries
        .iter()
        .enumerate()
        .filter(|(_, e)| listed(e, level, unchanged, metadata))
        .map(|(ix, _)| ix)
        .collect()
}

fn format_size(bytes: u64) -> String {
    crate::format::format_bytes(i64::try_from(bytes).unwrap_or(i64::MAX), 1)
}

/// `+1.2 KiB`, `−300 B`: how much an entry grew.
fn size_delta(e: &ArchiveEntryChange) -> Option<String> {
    let old = e.old.as_ref().map(|m| m.size);
    let new = e.new.as_ref().map(|m| m.size);
    if e.is_dir() {
        return None;
    }
    let (sign, amount) = match (old, new) {
        (None, Some(n)) => ("+", n),
        (Some(o), None) => ("−", o),
        (Some(o), Some(n)) if n > o => ("+", n - o),
        (Some(o), Some(n)) if o > n => ("−", o - n),
        (Some(_), Some(_)) if e.status == ArchiveEntryStatus::Modified => {
            return Some("±0 B".into());
        }
        _ => return None,
    };
    Some(format!("{sign}{}", format_size(amount)))
}

/// Unix permission bits as `rwxr-xr-x`.
fn mode_text(mode: u32) -> String {
    let mut out = String::with_capacity(9);
    for shift in [6, 3, 0] {
        let bits = (mode >> shift) & 7;
        out.push(if bits & 4 != 0 { 'r' } else { '-' });
        out.push(if bits & 2 != 0 { 'w' } else { '-' });
        out.push(if bits & 1 != 0 { 'x' } else { '-' });
    }
    format!("{out} ({mode:o})")
}

/// Days since 1970 as (year, month, day).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// An entry's time: zip's zone-less times as stored, others in local time.
fn time_text(meta: &ArchiveEntryMeta) -> Option<String> {
    let secs = meta.mtime?;
    if meta.mtime_naive {
        let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
        let rest = secs.rem_euclid(86_400);
        return Some(format!(
            "{y:04}-{m:02}-{d:02} {:02}:{:02}",
            rest / 3600,
            rest % 3600 / 60
        ));
    }
    let at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(u64::try_from(secs).ok()?);
    Some(crate::format::format_date_time(at))
}

fn kind_text(kind: &ArchiveEntryKind) -> String {
    match kind {
        ArchiveEntryKind::File => "File".into(),
        ArchiveEntryKind::Dir => "Folder".into(),
        ArchiveEntryKind::Symlink(_) => "Symbolic link".into(),
        ArchiveEntryKind::Hardlink(_) => "Hard link".into(),
        ArchiveEntryKind::Other => "Special file".into(),
    }
}

/// The rows of the metadata table: (property, before, after).
fn metadata_rows(e: &ArchiveEntryChange) -> Vec<(&'static str, Option<String>, Option<String>)> {
    let field = |f: &dyn Fn(&ArchiveEntryMeta) -> Option<String>| {
        (e.old.as_ref().and_then(f), e.new.as_ref().and_then(f))
    };
    let mut rows = Vec::new();
    let mut push = |name, (old, new): (Option<String>, Option<String>)| {
        if old.is_some() || new.is_some() {
            rows.push((name, old, new));
        }
    };
    push("Kind", field(&|m| Some(kind_text(&m.kind))));
    push(
        "Link target",
        field(&|m| match &m.kind {
            ArchiveEntryKind::Symlink(t) | ArchiveEntryKind::Hardlink(t) => Some(t.clone()),
            _ => None,
        }),
    );
    push(
        "Size",
        field(&|m| (m.kind == ArchiveEntryKind::File).then(|| format_size(m.size))),
    );
    push("Packed size", field(&|m| m.compressed.map(format_size)));
    push("Compression", field(&|m| m.method.clone()));
    push("Permissions", field(&|m| m.mode.map(mode_text)));
    push("Modified", field(&time_text));
    push("Owner", field(&|m| m.owner.clone()));
    push(
        "CRC-32",
        field(&|m| (m.kind == ArchiveEntryKind::File).then(|| format!("{:08x}", m.crc32))),
    );
    push(
        "Encrypted",
        field(&|m| m.encrypted.then(|| "Yes".to_string())),
    );
    rows
}

/// What changed besides the contents, for the entry header: `rw-r--r--
/// → rwxr-xr-x`, the time, the compression.
fn metadata_changes(e: &ArchiveEntryChange) -> Vec<String> {
    let (Some(old), Some(new)) = (&e.old, &e.new) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if old.mode != new.mode
        && let (Some(o), Some(n)) = (old.mode, new.mode)
    {
        out.push(format!("{:o} → {:o}", o, n));
    }
    if old.method != new.method
        && let (Some(o), Some(n)) = (&old.method, &new.method)
    {
        out.push(format!("{o} → {n}"));
    }
    if old.mtime != new.mtime {
        out.push("time changed".into());
    }
    if old.owner != new.owner {
        out.push("owner changed".into());
    }
    out
}

impl ArchiveDiffView {
    pub fn new(state: Entity<AppState>, surface: ArchiveSurface, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let source = DiffSource::ArchiveEntry(surface);
        let entry_view = cx.new(|cx| DiffView::new(state.clone(), source, cx));
        let stacked = cx.new(|cx| StackedDiffView::new(state.clone(), source, cx));
        let tree_width = zpx(state.read(cx).settings.archive_tree_width);
        let resizable = cx.new(|_| ResizableState::default());
        cx.subscribe(&resizable, |this, state, _: &ResizablePanelEvent, cx| {
            if let Some(width) = state.read(cx).sizes().first().copied()
                && width != this.tree_width
            {
                this.tree_width = width;
                Dispatcher::set_archive_tree_width(unzoom(width), cx);
                cx.notify();
            }
        })
        .detach();
        Self {
            state,
            surface,
            entry_view,
            stacked,
            resizable,
            tree_width,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle().tab_stop(true),
            filter: None,
            filter_text: String::new(),
            collapsed: HashSet::new(),
            collapsed_generation: 0,
            collapsed_level: None,
            cursor_folder: None,
            tree: None,
            scrolled_to: None,
        }
    }

    fn browse<'a>(&self, cx: &'a App) -> Option<(u64, &'a ArchiveBrowse)> {
        let s = self.state.read(cx);
        let id = s.selected?;
        let b = s.repo_states.get(&id)?.archives.open.get(&self.surface)?;
        Some((id, b))
    }

    /// The tree for the level shown, rebuilt when its inputs change.
    fn entry_tree(&mut self, level: &Arc<ArchiveDiff>, cx: &App) -> Rc<EntryTree> {
        if !self
            .collapsed_level
            .as_ref()
            .is_some_and(|l| Arc::ptr_eq(l, level))
        {
            // another level (or a reload): folds start over, unless the
            // same folders exist
            let keep: HashSet<String> = self
                .collapsed
                .iter()
                .filter(|f| {
                    let prefix = format!("{f}/");
                    level.entries.iter().any(|e| e.path.starts_with(&prefix))
                })
                .cloned()
                .collect();
            self.collapsed = keep;
            self.collapsed_generation += 1;
            self.collapsed_level = Some(level.clone());
        }
        let (unchanged, metadata) = list_settings(self.state.read(cx));
        if let Some(tree) = &self.tree
            && Arc::ptr_eq(&tree.level, level)
            && tree.show_unchanged == unchanged
            && tree.show_metadata == metadata
            && tree.filter == self.filter_text
            && tree.collapsed_generation == self.collapsed_generation
        {
            return tree.clone();
        }
        let needle = self.filter_text.to_lowercase();
        let items: Vec<usize> = listed_entries(level, unchanged, metadata)
            .into_iter()
            .filter(|ix| {
                needle.is_empty() || level.entries[*ix].path.to_lowercase().contains(&needle)
            })
            .collect();
        let paths: Vec<&str> = items
            .iter()
            .map(|ix| level.entries[*ix].path.as_str())
            .collect();
        let collapsed = &self.collapsed;
        // a filter shows every match
        let tree = FileTree::build(&paths, |f| needle.is_empty() && collapsed.contains(f));
        let tree = Rc::new(EntryTree {
            level: level.clone(),
            show_unchanged: unchanged,
            show_metadata: metadata,
            filter: self.filter_text.clone(),
            collapsed_generation: self.collapsed_generation,
            items,
            tree,
        });
        self.tree = Some(tree.clone());
        tree
    }

    fn toggle_folder(&mut self, folder: String, collapse: Option<bool>, cx: &mut Context<Self>) {
        let collapse = collapse.unwrap_or(!self.collapsed.contains(&folder));
        let changed = if collapse {
            self.collapsed.insert(folder)
        } else {
            self.collapsed.remove(&folder)
        };
        if changed {
            self.collapsed_generation += 1;
            cx.notify();
        }
    }

    fn select(&mut self, path: String, cx: &mut Context<Self>) {
        self.cursor_folder = None;
        if let Some((id, _)) = self.browse(cx) {
            Dispatcher::select_archive_entry(id, self.surface, path, cx);
        }
        cx.notify();
    }

    /// The row the keyboard moves from.
    fn current_row(&self, tree: &EntryTree, cx: &App) -> Option<usize> {
        if let Some(folder) = &self.cursor_folder
            && let Some(row) = tree
                .tree
                .rows
                .iter()
                .position(|r| matches!(r, TreeRow::Folder { path, .. } if path == folder))
        {
            return Some(row);
        }
        let (_, b) = self.browse(cx)?;
        let selected = b.selected.as_deref()?;
        let item = tree
            .items
            .iter()
            .position(|ix| tree.level.entries[*ix].path == selected)?;
        tree.tree.row_of_item(item)
    }

    fn select_row(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(tree) = self.tree.clone() else {
            return;
        };
        match tree.tree.rows.get(row) {
            Some(TreeRow::File { item, .. }) => {
                let path = tree.level.entries[tree.items[*item]].path.clone();
                self.select(path, cx);
            }
            Some(TreeRow::Folder { path, .. }) => {
                self.cursor_folder = Some(path.clone());
                cx.notify();
            }
            None => return,
        }
        self.scroll.scroll_to_item(row, ScrollStrategy::Nearest);
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(tree) = self.tree.clone() else {
            return;
        };
        let count = tree.tree.rows.len();
        if count == 0 {
            return;
        }
        let row = match self.current_row(&tree, cx) {
            Some(row) => row.saturating_add_signed(delta).min(count - 1),
            None if delta < 0 => count - 1,
            None => 0,
        };
        self.select_row(row, cx);
    }

    fn edge(&mut self, last: bool, cx: &mut Context<Self>) {
        let count = self.tree.as_ref().map_or(0, |t| t.tree.rows.len());
        if count > 0 {
            self.select_row(if last { count - 1 } else { 0 }, cx);
        }
    }

    fn tree_key(&mut self, right: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tree) = self.tree.clone() else {
            return;
        };
        let Some(row) = self.current_row(&tree, cx) else {
            return;
        };
        use crate::file_tree_rows::{LeftKey, RightKey, left_key, right_key};
        if right {
            match right_key(&tree.tree, row) {
                Some(RightKey::Expand(folder)) => self.toggle_folder(folder, Some(false), cx),
                Some(RightKey::Select(row)) => self.select_row(row, cx),
                None => crate::file_tree_rows::focus_pane(true, window, cx),
            }
        } else {
            match left_key(&tree.tree, row) {
                LeftKey::Collapse(folder) => self.toggle_folder(folder, Some(true), cx),
                LeftKey::Select(row) => self.select_row(row, cx),
                LeftKey::Nothing => crate::file_tree_rows::focus_pane(false, window, cx),
            }
        }
    }

    /// Enter: open the selected archive entry; on a folder, fold it.
    fn open_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(folder) = self.cursor_folder.clone() {
            self.toggle_folder(folder, None, cx);
            return;
        }
        let Some((id, b)) = self.browse(cx) else {
            return;
        };
        let Some(change) = b.selected_change() else {
            return;
        };
        if opens_as_archive(change, b) {
            Dispatcher::open_archive_entry(id, self.surface, change.path.clone(), cx);
        }
    }

    fn up(&mut self, levels: usize, cx: &mut Context<Self>) {
        if let Some((id, b)) = self.browse(cx)
            && !b.nested.is_empty()
        {
            Dispatcher::archive_up(id, self.surface, levels, cx);
        }
    }

    fn ensure_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(filter) = &self.filter {
            return filter.clone();
        }
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter entries"));
        cx.subscribe(&filter, |this, input, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                this.filter_text = input.read(cx).value().to_string();
                cx.notify();
            }
        })
        .detach();
        self.filter = Some(filter.clone());
        filter
    }
}

/// Whether an entry is an archive that opens as a level.
fn opens_as_archive(change: &ArchiveEntryChange, b: &ArchiveBrowse) -> bool {
    corvene_git::archive::archive_format(&change.path).is_some()
        && change.status.is_content_change()
        && change
            .meta()
            .is_some_and(|m| m.kind == ArchiveEntryKind::File)
        && b.nested.len() < corvene_git::archive::MAX_DEPTH
}

/// A pill in the summary bar.
fn count_pill(
    id: &'static str,
    text: String,
    color: Hsla,
    tooltip: String,
    cx: &App,
) -> Stateful<Div> {
    let _ = cx;
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

fn plural(n: usize, one: &str, many: &str) -> String {
    format!(
        "{} {}",
        crate::format::format_count(n as u64),
        if n == 1 { one } else { many }
    )
}

/// A square toggle button of the summary bar.
fn toggle_button(
    id: &'static str,
    icon: Octicon,
    on: bool,
    tooltip: &'static str,
    on_click: impl Fn(&mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.ghd();
    div()
        .id(id)
        .icon_button_label(tooltip)
        .flex_none()
        .ml(SPACING_HALF())
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
        .ghd_tooltip(tooltip)
        .on_click(move |_, _, cx| on_click(cx))
}

impl ArchiveDiffView {
    fn summary_bar(
        &mut self,
        id: u64,
        b: &ArchiveBrowse,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd().clone();
        let level = b.level().diff.clone();
        let counts = level.counts();
        let s = self.state.read(cx);
        let (show_unchanged, show_metadata) = list_settings(s);
        let stacked_on = s.settings.archive_stacked;
        let mut pills: Vec<AnyElement> = Vec::new();
        if counts.added > 0 {
            pills.push(
                count_pill(
                    "archive-added",
                    format!("+{}", counts.added),
                    t.color_new,
                    plural(counts.added, "entry added", "entries added"),
                    cx,
                )
                .into_any_element(),
            );
        }
        if counts.deleted > 0 {
            pills.push(
                count_pill(
                    "archive-deleted",
                    format!("−{}", counts.deleted),
                    t.color_deleted,
                    plural(counts.deleted, "entry removed", "entries removed"),
                    cx,
                )
                .into_any_element(),
            );
        }
        if counts.modified > 0 {
            pills.push(
                count_pill(
                    "archive-modified",
                    format!("~{}", counts.modified),
                    t.color_modified,
                    plural(counts.modified, "entry modified", "entries modified"),
                    cx,
                )
                .into_any_element(),
            );
        }
        if counts.renamed > 0 {
            pills.push(
                count_pill(
                    "archive-renamed",
                    format!("→{}", counts.renamed),
                    t.color_renamed,
                    plural(counts.renamed, "entry moved", "entries moved"),
                    cx,
                )
                .into_any_element(),
            );
        }
        if counts.content_changes() == 0 && level.single.is_none() {
            pills.push(
                div()
                    .px(SPACING_HALF())
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child("No content changes")
                    .into_any_element(),
            );
        }
        let metadata_toggle = (counts.metadata > 0).then(|| {
            div()
                .id("archive-metadata-toggle")
                .flex_none()
                .ml(SPACING_HALF())
                .px(SPACING_HALF())
                .rounded(BORDER_RADIUS())
                .text_size(FONT_SIZE_SM())
                .text_color(t.text_secondary)
                .cursor_pointer()
                .hover(|d| d.bg(t.box_hover_background))
                .when(show_metadata, |d| d.text_color(t.text))
                .child(format!(
                    "{} {}",
                    plural(counts.metadata, "entry", "entries"),
                    if show_metadata {
                        "with metadata changes (hide)"
                    } else {
                        "with metadata changes (show)"
                    }
                ))
                .ghd_tooltip(
                    "Entries whose contents are the same but whose time, permissions, owner or \
                     compression changed",
                )
                .on_click(move |_, _, cx| {
                    Dispatcher::set_archive_show_metadata_only(!show_metadata, cx)
                })
        });
        let sizes = match (&level.old, &level.new) {
            (Some(o), Some(n)) if o.bytes != n.bytes => {
                let (sign, delta) = if n.bytes >= o.bytes {
                    ("+", n.bytes - o.bytes)
                } else {
                    ("−", o.bytes - n.bytes)
                };
                format!(
                    "{} → {} ({sign}{})",
                    format_size(o.bytes),
                    format_size(n.bytes),
                    format_size(delta)
                )
            }
            (Some(o), Some(_)) => format_size(o.bytes),
            (None, Some(n)) => format_size(n.bytes),
            (Some(o), None) => format_size(o.bytes),
            (None, None) => String::new(),
        };
        let unpacked = level.new.as_ref().or(level.old.as_ref()).map(|s| {
            format!(
                "{} unpacked in {}",
                format_size(s.unpacked),
                plural(s.entry_count, "entry", "entries")
            )
        });
        let entries = level.entries.len();
        let filter = (entries > FILTER_FROM && level.single.is_none()).then(|| {
            let input = self.ensure_filter(window, cx);
            div()
                .flex_none()
                .w(zpx(170.))
                .ml(SPACING())
                .child(crate::widgets::filter_text_box(
                    "archive-filter",
                    &input,
                    Some(octicon(Octicon::Filter, t.text_secondary)),
                    window,
                    cx,
                ))
        });
        let many = level.single.is_none();
        div()
            .h(zpx(30.))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .px(SPACING())
            .bg(t.box_alt_background)
            .border_b_1()
            .border_color(t.diff_border)
            .child(octicon(Octicon::FileZip, t.text_secondary))
            .child(
                div()
                    .id("archive-format")
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
                    .child(level.format.label())
                    .ghd_tooltip(match &level.single {
                        Some(name) => format!("One compressed file, {name}"),
                        None => "Archive format".to_string(),
                    }),
            )
            .children(pills)
            .children(metadata_toggle)
            .child(div().flex_1())
            .child(
                div()
                    .id("archive-sizes")
                    .flex_none()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(sizes)
                    .when_some(unpacked, |d, unpacked| d.ghd_tooltip(unpacked)),
            )
            .children(filter)
            .when(many, |d| {
                d.child(toggle_button(
                    "archive-show-unchanged",
                    if show_unchanged {
                        Octicon::Eye
                    } else {
                        Octicon::EyeClosed
                    },
                    show_unchanged,
                    if show_unchanged {
                        "Hide unchanged entries"
                    } else {
                        "Show unchanged entries"
                    },
                    move |cx| Dispatcher::set_archive_show_unchanged(!show_unchanged, cx),
                    cx,
                ))
                .child(toggle_button(
                    "archive-stacked",
                    Octicon::Stack,
                    stacked_on,
                    if stacked_on {
                        "Show one entry at a time"
                    } else {
                        "Show every changed entry's diff in one list"
                    },
                    move |cx| Dispatcher::set_archive_stacked(!stacked_on, cx),
                    cx,
                ))
            })
            .id(("archive-summary", id as usize))
            .into_any_element()
    }

    fn breadcrumb(&self, b: &ArchiveBrowse, cx: &mut Context<Self>) -> Option<AnyElement> {
        if b.nested.is_empty() {
            return None;
        }
        let t = cx.ghd();
        let depth = b.nested.len();
        let weak = cx.weak_entity();
        let mut parts: Vec<AnyElement> = Vec::new();
        let back = weak.clone();
        parts.push(
            div()
                .id("archive-back")
                .a11y_button("Back to the containing archive")
                .flex_none()
                .mr(SPACING_HALF())
                .px(SPACING_HALF())
                .rounded(BORDER_RADIUS())
                .cursor_pointer()
                .hover(|d| d.bg(t.box_hover_background))
                .child("‹ Back")
                .ghd_tooltip("Back to the containing archive (⌫)")
                .on_click(move |_, _, cx| {
                    back.update(cx, |this, cx| this.up(1, cx)).ok();
                })
                .into_any_element(),
        );
        for (ix, level) in b.levels().enumerate() {
            let name = level
                .entry
                .rsplit('/')
                .next()
                .unwrap_or(&level.entry)
                .to_string();
            if ix > 0 {
                parts.push(
                    div()
                        .flex_none()
                        .px(SPACING_HALF())
                        .text_color(t.text_secondary)
                        .child("›")
                        .into_any_element(),
                );
            }
            let last = ix == depth;
            let weak = weak.clone();
            parts.push(
                div()
                    .id(("archive-crumb", ix))
                    .flex_none()
                    .px(zpx(3.))
                    .rounded(BORDER_RADIUS())
                    .when(last, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .when(!last, |d| {
                        d.cursor_pointer()
                            .text_color(t.link)
                            .hover(|d| d.bg(t.box_hover_background))
                            .on_click(move |_, _, cx| {
                                weak.update(cx, |this, cx| this.up(depth - ix, cx)).ok();
                            })
                    })
                    .ghd_tooltip(level.entry.clone())
                    .child(name)
                    .into_any_element(),
            );
        }
        Some(
            div()
                .h(zpx(26.))
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .px(SPACING())
                .border_b_1()
                .border_color(t.diff_border)
                .text_size(FONT_SIZE_SM())
                .text_color(t.text)
                .children(parts)
                .into_any_element(),
        )
    }

    /// The "rebuilt archive" banner and the cut-off note.
    fn notices(&self, level: &ArchiveDiff, cx: &App) -> Vec<AnyElement> {
        let t = cx.ghd();
        let counts = level.counts();
        let (_, show_metadata) = list_settings(self.state.read(cx));
        let mut out = Vec::new();
        let notice = |icon: Octicon, text: String| {
            div()
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(SPACING_HALF())
                .px(SPACING())
                .py(SPACING_HALF())
                .bg(t.box_alt_background)
                .border_b_1()
                .border_color(t.diff_border)
                .text_size(FONT_SIZE_SM())
                .text_color(t.text)
                .child(octicon(icon, t.text_secondary))
                .child(div().flex_1().min_w_0().child(text))
        };
        if counts.content_changes() == 0 && counts.metadata > 0 && level.single.is_none() {
            let mut banner = notice(
                Octicon::Info,
                format!(
                    "The contents are identical. Only the time, permissions, owner or compression \
                     of {} changed, so the archive was most likely rebuilt.",
                    plural(counts.metadata, "entry", "entries")
                ),
            );
            if !show_metadata {
                banner = banner.child(
                    crate::widgets::link_button("archive-show-metadata", "Show them", cx)
                        .on_click(|_, _, cx| Dispatcher::set_archive_show_metadata_only(true, cx)),
                );
            }
            out.push(banner.into_any_element());
        }
        if level.truncated {
            out.push(
                notice(
                    Octicon::Alert,
                    format!(
                        "Only the first {} entries and {} of unpacked contents are compared.",
                        crate::format::format_count(corvene_git::archive::MAX_ENTRIES as u64),
                        format_size(corvene_git::archive::MAX_UNPACKED)
                    ),
                )
                .into_any_element(),
            );
        }
        out
    }

    fn entry_tree_list(
        &mut self,
        id: u64,
        b: &ArchiveBrowse,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd().clone();
        let level = b.level().diff.clone();
        let tree = self.entry_tree(&level, cx);
        let focused = self.focus.contains_focused(window, cx);
        let selected = b.selected.clone();
        // keep the selection in view when it moved
        if selected != self.scrolled_to {
            self.scrolled_to = selected.clone();
            if let Some(row) = self.current_row(&tree, cx) {
                self.scroll.scroll_to_item(row, ScrollStrategy::Nearest);
            }
        }
        let cursor = self.cursor_folder.clone();
        let weak = cx.weak_entity();
        let focus = self.focus.clone();
        let surface = self.surface;
        let rows = tree.tree.rows.len();
        let body = if rows == 0 {
            let (unchanged, _) = list_settings(self.state.read(cx));
            let message = if !self.filter_text.is_empty() {
                "No entries match the filter."
            } else {
                "No entries changed."
            };
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(SPACING_HALF())
                .p(SPACING())
                .text_size(FONT_SIZE())
                .text_color(t.text_secondary)
                .child(message)
                .when(!unchanged && self.filter_text.is_empty(), |d| {
                    d.child(
                        crate::widgets::link_button(
                            "archive-empty-show",
                            "Show unchanged entries",
                            cx,
                        )
                        .on_click(|_, _, cx| Dispatcher::set_archive_show_unchanged(true, cx)),
                    )
                })
                .into_any_element()
        } else {
            let browse_nested = b.nested.len();
            uniform_list("archive-entry-rows", rows, move |range, _, cx| {
                range
                    .map(|ix| match &tree.tree.rows[ix] {
                        row @ TreeRow::Folder { path, .. } => {
                            let (select, toggle) = (weak.clone(), weak.clone());
                            let (folder, toggle_folder) = (path.clone(), path.clone());
                            let focus = focus.clone();
                            crate::file_tree_rows::plain_folder_row(
                                SharedString::from(format!("archive-folder-{path}")),
                                "archive-folder-row",
                                row,
                                cursor.as_deref() == Some(path.as_str()),
                                focused,
                                move |window, cx| {
                                    window.focus(&focus, cx);
                                    select
                                        .update(cx, |this, cx| {
                                            this.cursor_folder = Some(folder.clone());
                                            cx.notify();
                                        })
                                        .ok();
                                },
                                move |_, cx| {
                                    toggle
                                        .update(cx, |this, cx| {
                                            this.toggle_folder(toggle_folder.clone(), None, cx)
                                        })
                                        .ok();
                                },
                                |_, _, _| {},
                                None,
                                cx,
                            )
                        }
                        TreeRow::File { item, depth } => {
                            let change = &tree.level.entries[tree.items[*item]];
                            let is_selected = cursor.is_none()
                                && selected.as_deref() == Some(change.path.as_str());
                            entry_row(
                                id,
                                surface,
                                change,
                                *depth,
                                is_selected,
                                focused,
                                browse_nested,
                                &focus,
                                &weak,
                                cx,
                            )
                        }
                    })
                    .collect()
            })
            .flex_1()
            .min_h_0()
            .with_scrollbar_handle(&self.scroll)
            .into_any_element()
        };
        div()
            .id("archive-entries")
            .track_focus(&self.focus)
            .role(Role::List)
            .aria_label("Archive entries")
            .key_context("ArchiveEntryList FileTree")
            .on_action(cx.listener(|this, _: &SelectNextFile, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPreviousFile, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectFirstFile, _, cx| this.edge(false, cx)))
            .on_action(cx.listener(|this, _: &SelectLastFile, _, cx| this.edge(true, cx)))
            .on_action(
                cx.listener(|this, _: &CollapseFolder, window, cx| {
                    this.tree_key(false, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &ExpandFolder, window, cx| this.tree_key(true, window, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenArchiveEntry, _, cx| this.open_selected(cx)))
            .on_action(cx.listener(|this, _: &ArchiveUp, _, cx| this.up(1, cx)))
            .size_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(t.box_border)
            .child(body)
            .into_any_element()
    }

    /// The selected entry: its header over its diff.
    fn entry_pane(&self, id: u64, b: &ArchiveBrowse, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.ghd().clone();
        if let Some(opening) = &b.opening {
            return centered(
                vec![
                    crate::icons::loading("archive-opening", t.text_secondary),
                    div()
                        .child(format!(
                            "Opening {}…",
                            opening.rsplit('/').next().unwrap_or(opening)
                        ))
                        .into_any_element(),
                ],
                cx,
            );
        }
        let Some(change) = b.selected_change().cloned() else {
            return centered(
                vec![
                    div()
                        .child("Select an entry to see its changes.")
                        .into_any_element(),
                ],
                cx,
            );
        };
        let header = self.entry_header(id, b, &change, cx);
        let loaded = b.entry.as_ref().filter(|e| e.path == change.path);
        let plain_file = |m: &Option<ArchiveEntryMeta>| {
            m.as_ref().is_none_or(|m| m.kind == ArchiveEntryKind::File)
        };
        let body: AnyElement = if !change.status.is_content_change()
            || !plain_file(&change.old)
            || !plain_file(&change.new)
        {
            metadata_card(&change, cx)
        } else if opens_as_archive(&change, b) {
            nested_card(id, self.surface, &change, loaded.map(|e| &*e.diff), cx)
        } else if loaded.is_none() {
            centered(
                vec![crate::icons::loading(
                    "archive-entry-loading",
                    t.text_secondary,
                )],
                cx,
            )
        } else {
            DiffView::embed(&self.entry_view).into_any_element()
        };
        div()
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .child(header)
            .child(div().flex_1().min_h_0().flex().flex_col().child(body))
            .into_any_element()
    }

    fn entry_header(
        &self,
        id: u64,
        b: &ArchiveBrowse,
        change: &ArchiveEntryChange,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd().clone();
        let kind = archive_entry_kind(change.status);
        let (icon, color) = match change.status {
            ArchiveEntryStatus::MetadataOnly => (Octicon::Clock, t.text_secondary),
            ArchiveEntryStatus::Unchanged => (Octicon::DotFill, t.text_secondary),
            _ => status_icon(kind, &t),
        };
        let changes = metadata_changes(change);
        let menu = entry_menu_button(id, self.surface, change, b, cx);
        let gear = (change.status.is_content_change() && !opens_as_archive(change, b)).then(|| {
            div()
                .ml(SPACING_HALF())
                .mr(SPACING_HALF())
                .child(diff_options_button(&self.entry_view, cx))
        });
        let full_name = b.display_path(&change.path);
        div()
            .id("archive-entry-header")
            .h(zpx(30.))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .px(SPACING())
            .border_b_1()
            .border_color(t.diff_border)
            .child(
                crate::path_label::path_label_element(
                    crate::path_label::path_label(&change.path, kind, change.old_path.as_deref()),
                    Vec::new(),
                    t.text_secondary,
                    t.text,
                )
                .text_size(FONT_SIZE()),
            )
            .ghd_tooltip(full_name)
            .children((!changes.is_empty()).then(|| {
                div()
                    .flex_none()
                    .ml(SPACING())
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(changes.join(" · "))
            }))
            .children(size_delta(change).map(|delta| {
                div()
                    .flex_none()
                    .ml(SPACING())
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(delta)
            }))
            .child(div().flex_1())
            .children(crate::diff_view::csv_switch(&change.path, cx))
            .children(gear)
            .child(menu)
            .child(div().ml(SPACING_HALF()).child(octicon(icon, color)))
            .into_any_element()
    }
}

fn centered(children: Vec<AnyElement>, cx: &App) -> AnyElement {
    let t = cx.ghd();
    div()
        .flex_1()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(SPACING_HALF())
        .p(SPACING_DOUBLE())
        .text_size(FONT_SIZE())
        .text_color(t.text_secondary)
        .children(children)
        .into_any_element()
}

/// A table of what an entry is and was, for entries whose contents did
/// not change (or that are not files).
fn metadata_card(change: &ArchiveEntryChange, cx: &App) -> AnyElement {
    let t = cx.ghd();
    let title = match change.status {
        ArchiveEntryStatus::MetadataOnly => {
            "The contents are identical. Only the details below changed."
        }
        ArchiveEntryStatus::Unchanged => "This entry did not change.",
        ArchiveEntryStatus::Added => "This entry was added.",
        ArchiveEntryStatus::Deleted => "This entry was removed.",
        ArchiveEntryStatus::Renamed => "This entry moved without changing.",
        ArchiveEntryStatus::Modified => "This entry changed.",
    };
    let both = change.old.is_some() && change.new.is_some();
    let cell = |text: Option<String>, changed: bool, strong: bool| {
        div()
            .flex_1()
            .min_w_0()
            .px(SPACING_HALF())
            .truncate()
            .when(changed, |d| d.font_weight(FontWeight::SEMIBOLD))
            .text_color(if strong { t.text } else { t.text_secondary })
            .child(text.unwrap_or_else(|| "—".into()))
    };
    let rows = metadata_rows(change).into_iter().map(|(name, old, new)| {
        let changed = both && old != new;
        div()
            .flex()
            .flex_row()
            .py(zpx(3.))
            .border_b_1()
            .border_color(t.box_border)
            .when(changed, |d| d.bg(t.diff_hunk_background))
            .child(
                cell(Some(name.to_string()), false, false)
                    .flex_none()
                    .w(zpx(110.)),
            )
            .when(change.old.is_some(), |d| {
                d.child(cell(old, changed, !changed || !both))
            })
            .when(change.new.is_some(), |d| d.child(cell(new, changed, true)))
    });
    let header = div()
        .flex()
        .flex_row()
        .pb(zpx(3.))
        .border_b_1()
        .border_color(t.box_border)
        .text_color(t.text_secondary)
        .child(div().flex_none().w(zpx(110.)).px(SPACING_HALF()))
        .when(change.old.is_some(), |d| {
            d.child(
                div()
                    .flex_1()
                    .px(SPACING_HALF())
                    .child(if both { "Before" } else { "" }),
            )
        })
        .when(change.new.is_some(), |d| {
            d.child(
                div()
                    .flex_1()
                    .px(SPACING_HALF())
                    .child(if both { "After" } else { "" }),
            )
        });
    div()
        .id("archive-metadata-card")
        .flex_1()
        .overflow_y_scroll()
        .p(SPACING_DOUBLE())
        .child(
            div()
                .max_w(zpx(640.))
                .mx_auto()
                .flex()
                .flex_col()
                .gap(SPACING())
                .text_size(FONT_SIZE())
                .child(div().text_color(t.text).child(title))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .text_size(FONT_SIZE_SM())
                        .child(header)
                        .children(rows),
                ),
        )
        .into_any_element()
}

/// An archive inside the archive: what changed in it and how to open it.
fn nested_card(
    id: u64,
    surface: ArchiveSurface,
    change: &ArchiveEntryChange,
    loaded: Option<&Diff>,
    cx: &App,
) -> AnyElement {
    let t = cx.ghd();
    let summary = match loaded {
        Some(Diff::Archive(inner)) => {
            let c = inner.counts();
            let mut parts = Vec::new();
            if c.added > 0 {
                parts.push(format!("{} added", c.added));
            }
            if c.deleted > 0 {
                parts.push(format!("{} removed", c.deleted));
            }
            if c.modified > 0 {
                parts.push(format!("{} modified", c.modified));
            }
            if c.renamed > 0 {
                parts.push(format!("{} moved", c.renamed));
            }
            if parts.is_empty() {
                "No entries changed inside.".to_string()
            } else {
                format!("Entries {}.", parts.join(", "))
            }
        }
        Some(_) => "It could not be read as an archive.".to_string(),
        None => "Reading its entries…".to_string(),
    };
    let path = change.path.clone();
    let name = change
        .path
        .rsplit('/')
        .next()
        .unwrap_or(&change.path)
        .to_string();
    centered(
        vec![
            octicon(Octicon::FileZip, t.text_secondary)
                .size(zpx(32.))
                .into_any_element(),
            div()
                .text_color(t.text)
                .child(format!("{name} is an archive."))
                .into_any_element(),
            div().child(summary).into_any_element(),
            crate::widgets::button("archive-open-nested", "Open Archive", cx)
                .on_click(move |_, _, cx| {
                    Dispatcher::open_archive_entry(id, surface, path.clone(), cx)
                })
                .into_any_element(),
            div()
                .text_size(FONT_SIZE_SM())
                .child("or press Return in the entry list")
                .into_any_element(),
        ],
        cx,
    )
}

/// The entry header's "…" button with the entry's actions.
fn entry_menu_button(
    id: u64,
    surface: ArchiveSurface,
    change: &ArchiveEntryChange,
    b: &ArchiveBrowse,
    cx: &App,
) -> AnyElement {
    let t = cx.ghd();
    let change = change.clone();
    let full = b.display_path(&change.path);
    let nested = opens_as_archive(&change, b);
    div()
        .id("archive-entry-menu")
        .icon_button_label("Entry actions")
        .flex_none()
        .ml(SPACING_HALF())
        .px(zpx(3.))
        .rounded(BORDER_RADIUS())
        .cursor_pointer()
        .hover(|d| d.bg(t.box_hover_background))
        .child(octicon(Octicon::KebabHorizontal, t.text))
        .ghd_tooltip("Entry actions")
        .on_click(move |ev: &ClickEvent, window, cx| {
            let items = entry_menu(id, surface, &change, &full, nested, cx);
            crate::native_menu::show_context_menu(items, ev.position(), window, cx);
        })
        .into_any_element()
}

/// Copy, open, save and compare one entry.
fn entry_menu(
    id: u64,
    surface: ArchiveSurface,
    change: &ArchiveEntryChange,
    full: &str,
    nested: bool,
    cx: &App,
) -> Vec<crate::context_menu::MenuItem> {
    use crate::context_menu::{MenuItem, labels, mac_or};
    let file = |m: &Option<ArchiveEntryMeta>| {
        m.as_ref().is_some_and(|m| {
            matches!(
                m.kind,
                ArchiveEntryKind::File
                    | ArchiveEntryKind::Symlink(_)
                    | ArchiveEntryKind::Hardlink(_)
            )
        })
    };
    let (has_new, has_old) = (file(&change.new), file(&change.old));
    let s = AppState::global(cx).read(cx);
    let rs = s.repo_states.get(&id);
    let diff_tool = rs.and_then(|rs| rs.diff_tool.clone());
    let outer = rs
        .and_then(|rs| rs.archives.open.get(&surface))
        .map(|b| b.outer_path.clone());
    let repo_path = s.repository(id).map(|r| r.path.clone());
    let path = change.path.clone();
    let relative = change.path.clone();
    let full = full.to_string();
    let name = corvene_git::archive::safe_file_name(&change.path, EntrySide::New);
    let old_name = corvene_git::archive::safe_file_name(&change.path, EntrySide::Old);
    let save = move |side: EntrySide, name: String| {
        let path = path.clone();
        move |_: &mut Window, cx: &mut App| {
            let dir = std::env::var_os("HOME")
                .map(|h| std::path::PathBuf::from(h).join("Downloads"))
                .filter(|d| d.is_dir())
                .unwrap_or_else(std::env::temp_dir);
            let receiver = cx.prompt_for_new_path(&dir, Some(&name));
            let path = path.clone();
            cx.spawn(async move |cx| {
                if let Ok(Ok(Some(dest))) = receiver.await {
                    cx.update(|cx| {
                        Dispatcher::save_archive_entry(id, surface, path.clone(), side, dest, cx)
                    });
                }
            })
            .detach();
        }
    };
    let open = |side: EntrySide| {
        let path = change.path.clone();
        move |_: &mut Window, cx: &mut App| {
            Dispatcher::open_archive_entry_externally(id, surface, path.clone(), side, cx)
        }
    };
    let mut items = vec![
        MenuItem::new(mac_or("Copy Path", "Copy path"), move |_, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(relative.clone()))
        }),
        MenuItem::new(mac_or("Copy Full Path", "Copy full path"), move |_, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(full.clone()))
        }),
        MenuItem::separator(),
    ];
    if nested {
        let path = change.path.clone();
        items.push(MenuItem::new(
            mac_or("Open Archive", "Open archive"),
            move |_, cx| Dispatcher::open_archive_entry(id, surface, path.clone(), cx),
        ));
    }
    items.push(
        MenuItem::new(mac_or("Open Entry", "Open entry"), open(EntrySide::New)).enabled(has_new),
    );
    if has_old && change.status != ArchiveEntryStatus::Deleted {
        items.push(MenuItem::new(
            mac_or("Open Previous Version", "Open previous version"),
            open(EntrySide::Old),
        ));
    }
    let deleted = change.status == ArchiveEntryStatus::Deleted;
    items.push(MenuItem::separator());
    items.push(
        MenuItem::new(
            mac_or("Save Entry As…", "Save entry as…"),
            save(
                if deleted {
                    EntrySide::Old
                } else {
                    EntrySide::New
                },
                if deleted { old_name.clone() } else { name },
            ),
        )
        .enabled(has_new || has_old),
    );
    if has_old && !deleted {
        items.push(MenuItem::new(
            mac_or("Save Previous Version As…", "Save previous version as…"),
            save(EntrySide::Old, old_name),
        ));
    }
    let compare = change.path.clone();
    items.push(MenuItem::separator());
    items.push(
        MenuItem::new(
            match &diff_tool {
                Some(tool) => format!(
                    "{} ({tool})",
                    mac_or("Open in External Diff Tool", "Open in external diff tool")
                ),
                None => {
                    mac_or("Open in External Diff Tool", "Open in external diff tool").to_string()
                }
            },
            move |_, cx| {
                Dispatcher::open_archive_entry_in_diff_tool(id, surface, compare.clone(), cx)
            },
        )
        .enabled(diff_tool.is_some() && (has_new || has_old)),
    );
    if surface == ArchiveSurface::Changes
        && let (Some(repo), Some(outer)) = (repo_path, outer)
    {
        let full = repo.join(outer);
        if full.exists() {
            items.push(MenuItem::new(
                labels::REVEAL_IN_FILE_MANAGER,
                move |_, cx| Dispatcher::show_in_finder(&full, cx),
            ));
        }
    }
    items
}

/// One entry of the tree.
#[allow(clippy::too_many_arguments)]
fn entry_row(
    id: u64,
    surface: ArchiveSurface,
    change: &ArchiveEntryChange,
    depth: usize,
    is_selected: bool,
    list_focused: bool,
    nested_depth: usize,
    focus: &FocusHandle,
    view: &WeakEntity<ArchiveDiffView>,
    cx: &App,
) -> AnyElement {
    let t = cx.ghd();
    let hover_bg = t.list_item_hover_background;
    let kind = archive_entry_kind(change.status);
    let quiet = !change.status.is_content_change();
    let selected_text = match (is_selected, list_focused) {
        (true, true) => Some(t.box_selected_active_text),
        (true, false) => Some(t.box_selected_text),
        _ => None,
    };
    let (icon, color) = match change.status {
        ArchiveEntryStatus::MetadataOnly => (Octicon::Clock, t.text_secondary),
        ArchiveEntryStatus::Unchanged => (Octicon::DotFill, t.text_secondary),
        _ => status_icon(kind, t),
    };
    let color = selected_text.filter(|_| list_focused).unwrap_or(color);
    let secondary = selected_text.unwrap_or(t.text_secondary);
    let meta = change.meta();
    let is_archive = corvene_git::archive::archive_format(&change.path).is_some()
        && change.status.is_content_change()
        && nested_depth < corvene_git::archive::MAX_DEPTH;
    let badge = |icon: Octicon, tip: &'static str| {
        div()
            .id(SharedString::from(tip))
            .flex_none()
            .child(octicon(icon, secondary).size(zpx(12.)))
            .ghd_tooltip(tip)
    };
    let file_icon = crate::file_icons::file_icons(cx)
        .map(|icons| crate::file_icons::file_icon(&icons, &change.path, secondary, cx))
        .unwrap_or_else(|| {
            octicon(
                if is_archive {
                    Octicon::FileZip
                } else if matches!(meta.map(|m| &m.kind), Some(ArchiveEntryKind::Dir)) {
                    Octicon::FileDirectory
                } else {
                    Octicon::File
                },
                secondary,
            )
            .into_any_element()
        });
    let path = change.path.clone();
    let menu_path = change.path.clone();
    let open_path = change.path.clone();
    let label_kind = if change.status == ArchiveEntryStatus::Renamed {
        FileStatusKind::Renamed
    } else {
        FileStatusKind::Modified
    };
    div()
        .id(SharedString::from(format!("archive-entry-{}", change.path)))
        .a11y_row(
            format!("{}, {:?}", change.path, change.status).to_lowercase(),
            is_selected,
        )
        .w_full()
        .h(ROW_HEIGHT())
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(SPACING_HALF())
        .px(SPACING())
        .cursor_pointer()
        .when(quiet && !is_selected, |d| d.opacity(0.65))
        .when(is_selected, |d| {
            if list_focused {
                d.bg(t.box_selected_active_background)
                    .text_color(t.box_selected_active_text)
            } else {
                d.bg(t.box_selected_background)
                    .text_color(t.box_selected_text)
            }
        })
        .when(
            !(is_selected && (list_focused || crate::widgets::selection_keeps_colour_on_hover(cx))),
            move |d| d.hover(move |s| s.bg(hover_bg)),
        )
        .on_mouse_down(MouseButton::Left, {
            let focus = focus.clone();
            let view = view.clone();
            move |_, window, cx| {
                window.focus(&focus, cx);
                view.update(cx, |this, cx| this.select(path.clone(), cx))
                    .ok();
            }
        })
        .on_mouse_down(MouseButton::Right, {
            let focus = focus.clone();
            let view = view.clone();
            move |ev: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                window.focus(&focus, cx);
                view.update(cx, |this, cx| this.select(menu_path.clone(), cx))
                    .ok();
                let Some((full, change, nested)) = AppState::global(cx)
                    .read(cx)
                    .repo_states
                    .get(&id)
                    .and_then(|rs| rs.archives.open.get(&surface))
                    .and_then(|b| {
                        let change = b.level().diff.entry(&menu_path)?.clone();
                        let nested = opens_as_archive(&change, b);
                        Some((b.display_path(&menu_path), change, nested))
                    })
                else {
                    return;
                };
                let items = entry_menu(id, surface, &change, &full, nested, cx);
                crate::native_menu::show_context_menu(items, ev.position, window, cx);
            }
        })
        .on_click(move |ev: &ClickEvent, _, cx| {
            if ev.click_count() == 2 && is_archive {
                Dispatcher::open_archive_entry(id, surface, open_path.clone(), cx);
            }
        })
        .child(
            div()
                .flex_none()
                .ml(crate::file_tree_rows::file_label_margin(depth))
                .child(file_icon),
        )
        .child(
            crate::path_label::path_label_element(
                crate::file_tree_rows::tree_label(
                    &change.path,
                    label_kind,
                    change.old_path.as_deref(),
                ),
                Vec::new(),
                secondary,
                selected_text.unwrap_or(t.text),
            )
            .flex_1()
            .min_w_0(),
        )
        .children(
            meta.filter(|m| m.encrypted)
                .map(|_| badge(Octicon::Lock, "Encrypted")),
        )
        .children(
            meta.filter(|m| {
                matches!(
                    m.kind,
                    ArchiveEntryKind::Symlink(_) | ArchiveEntryKind::Hardlink(_)
                )
            })
            .map(|_| badge(Octicon::FileSymlinkFile, "Link")),
        )
        .children(size_delta(change).map(|delta| {
            div()
                .flex_none()
                .text_size(zpx(11.))
                .text_color(secondary)
                .child(delta)
        }))
        .children(is_archive.then(|| {
            badge(
                Octicon::ChevronRight,
                "Archive: press Return or double-click to open",
            )
        }))
        .child(octicon(icon, color))
        .into_any_element()
}

/// `1311-stacked-diff`: an archive in a stack of files: what changed in it.
pub fn stacked_summary(
    _repo: u64,
    path: &str,
    archive: &ArchiveDiff,
    _source: DiffSource,
    cx: &App,
) -> AnyElement {
    let t = cx.ghd();
    let counts = archive.counts();
    let changed: Vec<&ArchiveEntryChange> = archive
        .entries
        .iter()
        .filter(|e| e.status.is_content_change() && !e.is_dir())
        .collect();
    let shown = changed.iter().take(6).map(|e| {
        let (icon, color) = status_icon(archive_entry_kind(e.status), t);
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING_HALF())
            .child(octicon(icon, color).size(zpx(12.)))
            .child(div().truncate().child(e.path.clone()))
    });
    let more = changed.len().saturating_sub(6);
    let name = path.rsplit('/').next().unwrap_or(path);
    div()
        .flex_1()
        .flex()
        .flex_col()
        .gap(zpx(2.))
        .p(SPACING())
        .text_size(FONT_SIZE_SM())
        .text_color(t.text)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(SPACING_HALF())
                .text_color(t.text_secondary)
                .child(octicon(Octicon::FileZip, t.text_secondary))
                .child(format!(
                    "{name}: {} added, {} removed, {} modified, {} moved",
                    counts.added, counts.deleted, counts.modified, counts.renamed
                )),
        )
        .children(shown)
        .when(more > 0, |d| {
            d.child(div().text_color(t.text_secondary).child(format!(
                "and {more} more. Select only this file to browse its entries."
            )))
        })
        .into_any_element()
}

impl Render for ArchiveDiffView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd().clone();
        let state = self.state.clone();
        let s = state.read(cx);
        let Some(id) = s.selected else {
            return div().into_any_element();
        };
        let Some(b) = s
            .repo_states
            .get(&id)
            .and_then(|rs| rs.archives.open.get(&self.surface))
            .cloned()
        else {
            return centered(
                vec![crate::icons::loading("archive-loading", t.text_secondary)],
                cx,
            );
        };
        let stacked = s.settings.archive_stacked;
        // the hide-whitespace setting moved under the shown entries
        let hide_whitespace = self.surface.hide_whitespace(s);
        if b.entry
            .as_ref()
            .is_some_and(|e| e.hide_whitespace != hide_whitespace)
            || b.stacked
                .values()
                .any(|e| e.hide_whitespace != hide_whitespace)
        {
            let surface = self.surface;
            cx.defer(move |cx| Dispatcher::refresh_archive_entries(id, surface, cx));
        }
        let level = b.level().diff.clone();
        let summary = self.summary_bar(id, &b, window, cx);
        let breadcrumb = self.breadcrumb(&b, cx);
        let notices = self.notices(&level, cx);
        let body: AnyElement = if level.single.is_some() {
            self.entry_pane(id, &b, cx)
        } else if stacked {
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(self.stacked.clone())
                .into_any_element()
        } else {
            let list = self.entry_tree_list(id, &b, window, cx);
            let pane = self.entry_pane(id, &b, cx);
            h_resizable("archive-split")
                .with_state(&self.resizable)
                .with_handle_appearance(Rc::new(|_, _, _| Some(div().into_any_element())))
                .child(
                    resizable_panel()
                        .size(self.tree_width)
                        .size_range(zpx(TREE_MIN)..zpx(TREE_MAX))
                        .child(list),
                )
                .child(resizable_panel().child(pane))
                .into_any_element()
        };
        div()
            .id("archive-view")
            .key_context("ArchiveView")
            .on_action(cx.listener(|this, _: &ArchiveUp, _, cx| this.up(1, cx)))
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(t.background)
            .child(summary)
            .children(breadcrumb)
            .children(notices)
            .child(div().flex_1().min_h_0().flex().flex_col().child(body))
            .into_any_element()
    }
}
