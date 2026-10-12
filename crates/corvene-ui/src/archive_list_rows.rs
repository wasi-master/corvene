//! Corvene `1328-archive-entries-in-file-list`: a changed archive in the
//! Changes, History and stash file lists gets a "▸ entries" toggle that
//! lists its changed entries under it, like a folder; clicking one selects
//! the archive and shows that entry in the archive view
//! (`1323-archive-diff`). GitHub Desktop lists an archive as one file
//! (`app/src/ui/changes/changed-file.tsx`, `app/src/ui/history/
//! committed-file-item.tsx`).
//!
//! The lists keep their own row models: [`ArchiveSplice`] maps the rows
//! drawn (a list's rows with each expanded archive's entries after its
//! row) back to the list's rows, for drawing and for scrolling to a row.
//! The entries are reached with the mouse; the archive view has the
//! keyboard navigation.

use std::rc::Rc;

use corvene_core::archive_browse::{ArchiveSurface, listing, nested_in_lists};
use corvene_core::{AppState, ArchiveEntryChange, ArchiveEntryStatus, Dispatcher, RepositoryState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::diff_view::{archive_entry_kind, status_icon};
use crate::icons::{Octicon, octicon};
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::{BORDER_RADIUS, FONT_SIZE_SM, ROW_HEIGHT, SPACING, SPACING_HALF, zpx};
use crate::widgets::{GhdTooltip, IconButtonA11y, ListRowA11y};

/// Entries listed under one archive; the rest are a "more" row.
const MAX_CHILDREN: usize = 200;

/// Selects the archive's row in its list.
pub type SelectFile = Rc<dyn Fn(&mut Window, &mut App)>;

/// One row under an expanded archive.
#[derive(Clone, Debug)]
pub enum ChildRow {
    Loading,
    Entry(Box<ArchiveEntryChange>),
    /// Entries past [`MAX_CHILDREN`].
    More(usize),
    /// Why nothing is listed.
    Note(String),
}

/// The rows under one expanded archive.
#[derive(Debug)]
pub struct ChildList {
    /// The archive's repository path.
    pub outer: String,
    /// The archive row's depth in a tree (`None` in a flat list).
    pub depth: Option<usize>,
    pub rows: Vec<ChildRow>,
}

/// What a drawn row is.
pub enum DisplayRow<'a> {
    /// The list's own row.
    Base(usize),
    /// Row `.1` under an expanded archive.
    Child(&'a ChildList, usize),
}

/// The rows a list draws: its own, with expanded archives' entries after
/// their rows.
#[derive(Debug, Default)]
pub struct ArchiveSplice {
    /// (the archive's row, its entries), by row.
    inserts: Vec<(usize, Rc<ChildList>)>,
    base: usize,
}

impl ArchiveSplice {
    /// No archive expanded: the list's rows as they are.
    pub fn plain(base: usize) -> Self {
        Self {
            inserts: Vec::new(),
            base,
        }
    }

    pub fn len(&self) -> usize {
        self.base
            + self
                .inserts
                .iter()
                .map(|(_, c)| c.rows.len())
                .sum::<usize>()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The drawn row `ix`.
    pub fn row(&self, ix: usize) -> Option<DisplayRow<'_>> {
        let mut extra = 0;
        for (row, children) in &self.inserts {
            let start = row + 1 + extra;
            if ix < start {
                break;
            }
            if ix < start + children.rows.len() {
                return Some(DisplayRow::Child(children, ix - start));
            }
            extra += children.rows.len();
        }
        let base = ix - extra;
        (base < self.base).then_some(DisplayRow::Base(base))
    }

    /// Where the list's row `base` is drawn.
    pub fn display_of(&self, base: usize) -> usize {
        base + self
            .inserts
            .iter()
            .take_while(|(row, _)| *row < base)
            .map(|(_, c)| c.rows.len())
            .sum::<usize>()
    }
}

/// The splice of a list with `base` rows on `surface`: `row_of` finds an
/// archive's row and depth. Returns the archives whose entries still have
/// to be read (the caller asks for them after the frame).
pub fn splice(
    s: &AppState,
    rs: &RepositoryState,
    surface: ArchiveSurface,
    base: usize,
    row_of: impl Fn(&str) -> Option<(usize, Option<usize>)>,
) -> (ArchiveSplice, Vec<String>) {
    if !nested_in_lists(s) || !rs.archives.expanded.iter().any(|(sf, _)| *sf == surface) {
        return (ArchiveSplice::plain(base), Vec::new());
    }
    let (unchanged, metadata) = (
        s.settings.archive_show_unchanged,
        s.settings.archive_show_metadata_only,
    );
    let mut inserts = Vec::new();
    let mut missing = Vec::new();
    for (_, path) in rs.archives.expanded.iter().filter(|(sf, _)| *sf == surface) {
        let Some((row, depth)) = row_of(path) else {
            continue;
        };
        let rows = match listing(rs, surface, path) {
            None => {
                missing.push(path.clone());
                vec![ChildRow::Loading]
            }
            Some(None) => vec![ChildRow::Loading],
            Some(Some(level)) => match &level.unreadable {
                Some(reason) => vec![ChildRow::Note(reason.clone())],
                None => {
                    let listed =
                        crate::archive_diff_view::listed_entries(level, unchanged, metadata);
                    if listed.is_empty() {
                        vec![ChildRow::Note("No entries changed".into())]
                    } else {
                        let mut rows: Vec<ChildRow> = listed
                            .iter()
                            .take(MAX_CHILDREN)
                            .map(|ix| ChildRow::Entry(Box::new(level.entries[*ix].clone())))
                            .collect();
                        if listed.len() > MAX_CHILDREN {
                            rows.push(ChildRow::More(listed.len() - MAX_CHILDREN));
                        }
                        rows
                    }
                }
            },
        };
        inserts.push((
            row,
            Rc::new(ChildList {
                outer: path.clone(),
                depth,
                rows,
            }),
        ));
    }
    inserts.sort_by_key(|(row, _)| *row);
    (ArchiveSplice { inserts, base }, missing)
}

/// Ask for the listings `splice` found missing, after this frame.
pub fn request_missing(id: u64, surface: ArchiveSurface, missing: Vec<String>, cx: &mut App) {
    for path in missing {
        cx.defer(move |cx| Dispatcher::ensure_archive_listing(id, surface, path, cx));
    }
}

/// Whether `path`'s row gets the toggle.
pub fn has_toggle(s: &AppState, path: &str) -> bool {
    nested_in_lists(s) && corvene_git::archive::archive_format(path).is_some()
}

/// The "▸ entries" toggle of an archive's row, before its status icon.
pub fn toggle(
    id: u64,
    surface: ArchiveSurface,
    path: &str,
    selected_text: Option<Hsla>,
    cx: &App,
) -> Option<AnyElement> {
    let s = AppState::try_global(cx)?.read(cx);
    if !has_toggle(s, path) {
        return None;
    }
    let rs = s.repo_states.get(&id)?;
    let expanded = rs.archives.is_expanded(surface, path);
    let t = cx.ghd();
    let color = selected_text.unwrap_or(t.text_secondary);
    let count = listing(rs, surface, path)
        .flatten()
        .filter(|l| l.unreadable.is_none())
        .map(|l| l.counts().content_changes());
    let label = match count {
        Some(n) => format!("{n}"),
        None => "entries".to_string(),
    };
    let path = path.to_string();
    Some(
        div()
            .id(SharedString::from(format!("archive-toggle-{path}")))
            .icon_button_label(if expanded {
                "Hide the archive's entries"
            } else {
                "Show the archive's entries"
            })
            .flex_none()
            .h(zpx(18.))
            .px(zpx(4.))
            .flex()
            .flex_row()
            .items_center()
            .gap(zpx(2.))
            .rounded(BORDER_RADIUS())
            .border_1()
            .border_color(t.box_border)
            .text_size(zpx(10.))
            .text_color(color)
            .cursor_pointer()
            .hover(|d| d.bg(t.box_hover_background))
            .child(
                octicon(
                    if expanded {
                        Octicon::ChevronDown
                    } else {
                        Octicon::ChevronRight
                    },
                    color,
                )
                .size(zpx(10.)),
            )
            .child(label)
            .ghd_tooltip(if expanded {
                "Hide the archive's entries"
            } else {
                "Show the archive's changed entries"
            })
            // a press here must not select the row underneath
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                Dispatcher::set_archive_expanded(id, surface, path.clone(), !expanded, cx);
            })
            .into_any_element(),
    )
}

/// Row `ix` of `list`. `file_selected`: the archive is the list's selected
/// file; `select_file` selects it.
#[allow(clippy::too_many_arguments)]
pub fn child_row(
    id: u64,
    surface: ArchiveSurface,
    list: &ChildList,
    ix: usize,
    file_selected: bool,
    list_focused: bool,
    select_file: SelectFile,
    cx: &App,
) -> AnyElement {
    let t = cx.ghd();
    let indent = crate::file_tree_rows::file_label_margin(list.depth.map_or(1, |d| d + 1));
    let base = div()
        .id(SharedString::from(format!(
            "archive-child-{}-{ix}",
            list.outer
        )))
        .w_full()
        .h(ROW_HEIGHT())
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(SPACING_HALF())
        .px(SPACING())
        .text_size(FONT_SIZE_SM());
    let Some(row) = list.rows.get(ix) else {
        return base.into_any_element();
    };
    let outer = list.outer.clone();
    match row {
        ChildRow::Loading => base
            .child(div().ml(indent).child(crate::icons::loading(
                SharedString::from(format!("archive-child-loading-{outer}")),
                t.text_secondary,
            )))
            .child(
                div()
                    .text_color(t.text_secondary)
                    .child("Reading the archive…"),
            )
            .into_any_element(),
        ChildRow::Note(text) => base
            .child(
                div()
                    .ml(indent)
                    .min_w_0()
                    .truncate()
                    .text_color(t.text_secondary)
                    .child(text.clone()),
            )
            .into_any_element(),
        ChildRow::More(n) => base
            .cursor_pointer()
            .hover(|d| d.bg(t.list_item_hover_background))
            .child(
                div()
                    .ml(indent)
                    .text_color(t.link)
                    .child(format!("{n} more, open the archive to see them")),
            )
            .on_click(move |_, window, cx| select_file(window, cx))
            .into_any_element(),
        ChildRow::Entry(change) => {
            let selected_entry = AppState::try_global(cx).and_then(|s| {
                s.read(cx)
                    .repo_states
                    .get(&id)?
                    .archives
                    .selected_entry(surface, &outer)
                    .map(str::to_string)
            });
            let is_selected =
                file_selected && selected_entry.as_deref() == Some(change.path.as_str());
            let text = match (is_selected, list_focused) {
                (true, true) => Some(t.box_selected_active_text),
                (true, false) => Some(t.box_selected_text),
                _ => None,
            };
            let (icon, color) = match change.status {
                ArchiveEntryStatus::MetadataOnly => (Octicon::Clock, t.text_secondary),
                ArchiveEntryStatus::Unchanged => (Octicon::DotFill, t.text_secondary),
                _ => status_icon(archive_entry_kind(change.status), t),
            };
            let icon_color = text.filter(|_| list_focused).unwrap_or(color);
            let quiet = !change.status.is_content_change();
            let hover_bg = t.list_item_hover_background;
            let entry = change.path.clone();
            let label = match &change.old_path {
                Some(old) => format!("{old} → {}", change.path),
                None => change.path.clone(),
            };
            base.a11y_row(format!("{label}, in {outer}"), is_selected)
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
                .when(!is_selected, move |d| d.hover(move |s| s.bg(hover_bg)))
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    Dispatcher::select_archive_entry_in_list(
                        id,
                        surface,
                        outer.clone(),
                        entry.clone(),
                        cx,
                    );
                    select_file(window, cx);
                })
                .child(div().ml(indent).flex_none().child(
                    octicon(Octicon::FileZip, text.unwrap_or(t.text_secondary)).size(zpx(12.)),
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(text.unwrap_or(t.text))
                        .child(label.clone()),
                )
                .ghd_tooltip(label)
                .child(octicon(icon, icon_color).size(zpx(12.)))
                .into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ArchiveSplice, ChildList, ChildRow, DisplayRow};
    use std::rc::Rc;

    fn list(n: usize) -> Rc<ChildList> {
        Rc::new(ChildList {
            outer: "a.zip".into(),
            depth: None,
            rows: vec![ChildRow::Loading; n],
        })
    }

    #[test]
    fn maps_drawn_rows_to_list_rows() {
        let splice = ArchiveSplice {
            inserts: vec![(1, list(2)), (3, list(1))],
            base: 5,
        };
        assert_eq!(splice.len(), 8);
        let kinds: Vec<String> = (0..8)
            .map(|ix| match splice.row(ix) {
                Some(DisplayRow::Base(b)) => format!("b{b}"),
                Some(DisplayRow::Child(_, c)) => format!("c{c}"),
                None => "-".into(),
            })
            .collect();
        assert_eq!(kinds, ["b0", "b1", "c0", "c1", "b2", "b3", "c0", "b4"]);
        assert_eq!(splice.display_of(0), 0);
        assert_eq!(splice.display_of(2), 4);
        assert_eq!(splice.display_of(4), 7);
        assert!(splice.row(8).is_none());
    }
}
