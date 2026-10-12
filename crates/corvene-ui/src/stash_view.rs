//! Stash viewer - GHD `ui/stashing/{stash-diff-viewer,stash-diff-header}.tsx`
//! (`styles/ui/_stash-diff-viewer.scss`): "Stashed changes" header with
//! Restore / Discard, then a resizable file list beside the read-only diff.
//!
//! Deviation (`797-stash-list`): the viewer shows the entry picked in the
//! Stashes section under its own title, with Apply (keep the stash) between
//! Restore and Discard; GHD's always shows the branch's Desktop stash.
//! A `stash -u` entry's untracked files (its third parent) are listed as
//! new files (`corvene_git::stashed_files`); GHD's stashes never have one.
//!
//! Deviation (`1310-file-list-tree`): the file list can show the stash's
//! files as a folder tree (GHD `stash-diff-viewer.tsx` lists them flat).
//!
//! Deviation (`1315-discard-stash-file`): a file's context menu can take it
//! out of the stash (GHD's `FileList` here has no context menu).

use corvene_core::archive_browse::ArchiveSurface;
use corvene_core::file_tree::TreeRow;
use corvene_core::{AppState, CommittedFileChange, Dispatcher};
use gpui_kit::component::resizable::{
    ResizablePanelEvent, ResizableState, h_resizable, resizable_panel,
};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::actions::{
    ExtendSelectionDown, ExtendSelectionUp, SelectFirstFile, SelectLastFile, SelectNextFile,
    SelectPreviousFile,
};
use crate::diff_view::{DiffSource, DiffView, status_icon};
use crate::scrollbar::ScrollbarExt;
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::ListRowA11y;
use crate::widgets::{button, primary_button};

#[allow(non_snake_case)]
fn FILE_LIST_MIN() -> Pixels {
    zpx(100.)
}
#[allow(non_snake_case)]
fn FILE_LIST_MAX() -> Pixels {
    zpx(600.)
}

pub struct StashDiffViewer {
    state: Entity<AppState>,
    diff: Entity<DiffView>,
    resizable: Entity<ResizableState>,
    file_list_width: Pixels,
    /// The file list takes focus on click so ⌘9 / ⌘8 resize it.
    file_list_focus: FocusHandle,
    /// `file_list_focus` held focus at the last render (active selection colours).
    file_list_focused: bool,
    file_scroll: UniformListScrollHandle,
    /// `1310-file-list-tree`: the file list's folders.
    tree: crate::file_tree_rows::ListTree,
    /// `1328-archive-entries-in-file-list`: the rows drawn, as of the last
    /// render.
    archive_splice: std::rc::Rc<crate::archive_list_rows::ArchiveSplice>,
}

/// `1310-file-list-tree`: a tree over the stash's paths.
type StashTree = crate::file_tree_rows::PathTree;

impl StashDiffViewer {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let diff = cx.new(|cx| DiffView::new(state.clone(), DiffSource::Stash, cx));
        let file_list_width = zpx(state.read(cx).settings.commit_summary_width);
        let resizable = cx.new(|_| ResizableState::default());
        cx.subscribe(&resizable, |this, state, _: &ResizablePanelEvent, cx| {
            if let Some(width) = state.read(cx).sizes().first().copied()
                && width != this.file_list_width
            {
                this.file_list_width = width;
                cx.notify();
            }
        })
        .detach();
        Self {
            state,
            diff,
            resizable,
            file_list_width,
            file_list_focus: cx.focus_handle().tab_stop(true),
            file_list_focused: false,
            file_scroll: UniformListScrollHandle::new(),
            tree: Default::default(),
            archive_splice: Default::default(),
        }
    }

    /// `1310-file-list-tree`: the stash's files as a tree, while file lists
    /// show trees.
    fn file_tree(&self, cx: &App) -> Option<StashTree> {
        if !crate::file_tree_rows::tree_mode(cx) {
            return None;
        }
        let s = self.state.read(cx);
        let files = s
            .selected
            .and_then(|id| s.repo_states.get(&id)?.stash_files.as_deref())
            .unwrap_or(&[]);
        Some(self.tree.tree(files.iter().map(|f| f.path.as_str())))
    }

    /// `1310-file-list-tree`: the tree row the keyboard moves from.
    fn tree_current_row(&self, t: &StashTree, cx: &App) -> Option<usize> {
        if let Some(row) = self.tree.cursor_row(&t.2) {
            return Some(row);
        }
        let (_, _, current) = self.file_order(cx)?;
        t.2.row_of_item(current?)
    }

    /// `1310-file-list-tree`: select tree row `row`; a folder takes the
    /// cursor (the list selects one file), its only file the selection.
    fn select_tree_row(&mut self, t: &StashTree, row: usize, cx: &mut Context<Self>) {
        use corvene_core::file_tree::TreeRow;
        let Some(id) = self.state.read(cx).selected else {
            return;
        };
        match t.2.rows.get(row) {
            Some(TreeRow::File { item, .. }) => {
                self.tree.clear_cursor();
                if let Some(path) = t.0.get(*item) {
                    Dispatcher::select_stash_file(id, path.clone(), cx);
                }
            }
            Some(TreeRow::Folder { path, .. }) => {
                self.tree.set_cursor(path.clone());
                if let [only] = crate::file_tree_rows::row_paths(&t.2, row, &t.0).as_slice() {
                    Dispatcher::select_stash_file(id, only.clone(), cx);
                }
            }
            None => return,
        }
        self.file_scroll
            .scroll_to_item(self.archive_splice.display_of(row), ScrollStrategy::Nearest);
        cx.notify();
    }

    fn select_folder(&mut self, folder: &str, cx: &mut Context<Self>) {
        let Some(t) = self.file_tree(cx) else { return };
        let row = t.2.rows.iter().position(|r| {
            matches!(r, corvene_core::file_tree::TreeRow::Folder { path, .. } if path == folder)
        });
        if let Some(row) = row {
            self.select_tree_row(&t, row, cx);
        }
    }

    fn set_collapsed(&mut self, folders: Vec<String>, collapsed: bool, cx: &mut Context<Self>) {
        self.tree.collapse.set(folders, collapsed);
        cx.notify();
    }

    /// `1310-file-list-tree`: ← / → on the tree.
    fn tree_key(&mut self, right: bool, window: &mut Window, cx: &mut Context<Self>) {
        use crate::file_tree_rows::{LeftKey, RightKey};
        let Some(t) = self.file_tree(cx) else { return };
        let Some(row) = self.tree_current_row(&t, cx) else {
            return;
        };
        if right {
            match crate::file_tree_rows::right_key(&t.2, row) {
                Some(RightKey::Expand(folder)) => self.set_collapsed(vec![folder], false, cx),
                Some(RightKey::Select(row)) => self.select_tree_row(&t, row, cx),
                None => crate::file_tree_rows::focus_pane(true, window, cx),
            }
        } else {
            match crate::file_tree_rows::left_key(&t.2, row) {
                LeftKey::Collapse(folder) => self.set_collapsed(vec![folder], true, cx),
                LeftKey::Select(parent) => self.select_tree_row(&t, parent, cx),
                LeftKey::Nothing => crate::file_tree_rows::focus_pane(false, window, cx),
            }
        }
    }

    /// The repository, the stash's files in list order and the selected
    /// file's index.
    fn file_order(&self, cx: &App) -> Option<(u64, Vec<String>, Option<usize>)> {
        let s = self.state.read(cx);
        let id = s.selected?;
        let rs = s.repo_states.get(&id)?;
        let order: Vec<String> = rs
            .stash_files
            .as_ref()
            .map(|f| f.iter().map(|f| f.path.clone()).collect())
            .unwrap_or_default();
        let current = rs
            .stash_selected_file
            .as_ref()
            .and_then(|p| order.iter().position(|o| o == p));
        Some((id, order, current))
    }

    /// GHD `List.moveSelection` on the stash's `FileList` (↑ / ↓, and ⌥↓ /
    /// ⌥↑ from the diff; single selection, so ⇧↑ / ⇧↓ too): the file
    /// `delta` rows away, wrapping around the ends (GHD `List.moveSelection`), scrolled into view.
    pub fn select_relative(&mut self, delta: isize, cx: &mut Context<Self>) {
        // `1310-file-list-tree`: through the tree's rows
        if let Some(t) = self.file_tree(cx) {
            let current = self.tree_current_row(&t, cx);
            if let Some(row) =
                corvene_core::list_selection::step_index(t.2.rows.len(), current, delta)
            {
                self.select_tree_row(&t, row, cx);
            }
            return;
        }
        let Some((id, order, current)) = self.file_order(cx) else {
            return;
        };
        if let Some(ix) = corvene_core::list_selection::step_index(order.len(), current, delta) {
            self.select_index(id, &order, ix, cx);
        }
    }

    /// Home / End, ⌘↑ / ⌘↓: the first or last file.
    fn select_edge(&mut self, last: bool, cx: &mut Context<Self>) {
        if let Some(t) = self.file_tree(cx) {
            if let Some(end) = t.2.rows.len().checked_sub(1) {
                self.select_tree_row(&t, if last { end } else { 0 }, cx);
            }
            return;
        }
        let Some((id, order, _)) = self.file_order(cx) else {
            return;
        };
        if !order.is_empty() {
            let ix = if last { order.len() - 1 } else { 0 };
            self.select_index(id, &order, ix, cx);
        }
    }

    fn select_index(&mut self, id: u64, order: &[String], ix: usize, cx: &mut Context<Self>) {
        self.tree.clear_cursor();
        Dispatcher::select_stash_file(id, order[ix].clone(), cx);
        self.file_scroll
            .scroll_to_item(self.archive_splice.display_of(ix), ScrollStrategy::Nearest);
    }

    fn file_list(&mut self, id: u64, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.ghd().clone();
        let state = self.state.clone();
        let s = state.read(cx);
        let rs = s.repo_states.get(&id);
        let files: Vec<CommittedFileChange> =
            rs.and_then(|r| r.stash_files.clone()).unwrap_or_default();
        let selected = rs.and_then(|r| r.stash_selected_file.clone());
        // `1310-file-list-tree`
        let tree = self.file_tree(cx);
        let cursor_row = tree.as_ref().and_then(|t| self.tree.cursor_row(&t.2));
        let weak = cx.weak_entity();
        let focus = self.file_list_focus.clone();
        let count = tree.as_ref().map_or(files.len(), |t| t.2.rows.len());
        // `1328-archive-entries-in-file-list`
        let (splice, missing) = match rs {
            Some(rs) => {
                crate::archive_list_rows::splice(s, rs, ArchiveSurface::Stash, count, |path| {
                    match &tree {
                        Some(t) => {
                            let item = t.0.iter().position(|p| p == path)?;
                            let row = t.2.row_of_item(item)?;
                            matches!(t.2.rows.get(row), Some(TreeRow::File { .. }))
                                .then(|| (row, Some(t.2.rows[row].depth())))
                        }
                        None => files
                            .iter()
                            .position(|f| f.path == path)
                            .map(|ix| (ix, None)),
                    }
                })
            }
            None => (
                crate::archive_list_rows::ArchiveSplice::plain(count),
                Vec::new(),
            ),
        };
        crate::archive_list_rows::request_missing(id, ArchiveSurface::Stash, missing, cx);
        let splice = std::rc::Rc::new(splice);
        self.archive_splice = splice.clone();
        let count = splice.len();
        let files = std::rc::Rc::new(files);
        let scroll = self.file_scroll.clone();
        let focused = self.file_list_focused;
        div()
            .size_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(t.box_border)
            .child(
                // a `List` node owning the file rows
                div()
                    .id("stash-files")
                    .role(Role::List)
                    .aria_label("Changed files")
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        uniform_list("stash-file-rows", count, move |range, _, cx| {
                            range
                                .map(|ix| {
                                    let ix = match splice.row(ix) {
                                        Some(crate::archive_list_rows::DisplayRow::Base(b)) => b,
                                        Some(crate::archive_list_rows::DisplayRow::Child(
                                            list,
                                            c,
                                        )) => {
                                            let outer = list.outer.clone();
                                            let focus = focus.clone();
                                            return crate::archive_list_rows::child_row(
                                                id,
                                                ArchiveSurface::Stash,
                                                list,
                                                c,
                                                selected.as_deref() == Some(list.outer.as_str()),
                                                focused,
                                                std::rc::Rc::new(move |window, cx| {
                                                    window.focus(&focus, cx);
                                                    Dispatcher::select_stash_file(
                                                        id,
                                                        outer.clone(),
                                                        cx,
                                                    )
                                                }),
                                                cx,
                                            );
                                        }
                                        None => return div().into_any_element(),
                                    };
                                    if let Some(t) = &tree {
                                        return stash_tree_row(
                                            id,
                                            t,
                                            ix,
                                            &files,
                                            selected.as_deref(),
                                            cursor_row,
                                            focused,
                                            &focus,
                                            &weak,
                                            cx,
                                        );
                                    }
                                    let file = &files[ix];
                                    let is_selected =
                                        selected.as_deref() == Some(file.path.as_str());
                                    stash_file_row(id, file, is_selected, focused, None, None, cx)
                                })
                                .collect()
                        })
                        .flex_1()
                        .min_h_0()
                        .with_scrollbar_handle(&scroll),
                    ),
            )
            .into_any_element()
    }
}

/// `1310-file-list-tree`: row `ix` of the stash's file tree.
#[allow(clippy::too_many_arguments)]
fn stash_tree_row(
    id: u64,
    tree: &StashTree,
    ix: usize,
    files: &[CommittedFileChange],
    selected: Option<&str>,
    cursor_row: Option<usize>,
    list_focused: bool,
    focus: &FocusHandle,
    view: &WeakEntity<StashDiffViewer>,
    cx: &App,
) -> AnyElement {
    use corvene_core::file_tree::TreeRow;
    match tree.2.rows.get(ix) {
        Some(TreeRow::File { item, depth }) => {
            let Some(file) = files.get(*item) else {
                return div().into_any_element();
            };
            // the folder cursor stands for the selection
            let is_selected = cursor_row.is_none() && selected == Some(file.path.as_str());
            stash_file_row(
                id,
                file,
                is_selected,
                list_focused,
                Some(*depth),
                Some(view.clone()),
                cx,
            )
        }
        Some(row @ TreeRow::Folder { path, expanded, .. }) => {
            let folder = path.clone();
            let select = {
                let view = view.clone();
                let focus = focus.clone();
                let folder = folder.clone();
                move |window: &mut Window, cx: &mut App| {
                    window.focus(&focus, cx);
                    view.update(cx, |this, cx| this.select_folder(&folder, cx))
                        .ok();
                }
            };
            let toggle = {
                let view = view.clone();
                let folder = folder.clone();
                let expanded = *expanded;
                move |_: &mut Window, cx: &mut App| {
                    view.update(cx, |this, cx| {
                        this.set_collapsed(vec![folder.clone()], expanded, cx)
                    })
                    .ok();
                }
            };
            let menu = {
                let view = view.clone();
                let tree = tree.clone();
                move |position: Point<Pixels>, window: &mut Window, cx: &mut App| {
                    let Some(repo) = AppState::global(cx)
                        .read(cx)
                        .repository(id)
                        .map(|r| r.path.clone())
                    else {
                        return;
                    };
                    let view = view.clone();
                    let items = crate::file_tree_rows::plain_folder_menu(
                        &repo,
                        &folder,
                        crate::file_tree_rows::all_folders(&tree.0),
                        move |folders, collapsed, cx| {
                            view.update(cx, |this, cx| this.set_collapsed(folders, collapsed, cx))
                                .ok();
                        },
                    );
                    crate::native_menu::show_context_menu(items, position, window, cx);
                }
            };
            crate::file_tree_rows::plain_folder_row(
                SharedString::from(format!("stash-folder-{path}")),
                "stash-file-row",
                row,
                cursor_row == Some(ix),
                list_focused,
                select,
                toggle,
                menu,
                None,
                cx,
            )
        }
        None => div().into_any_element(),
    }
}

fn stash_file_row(
    id: u64,
    file: &CommittedFileChange,
    is_selected: bool,
    list_focused: bool,
    // `1310-file-list-tree`: the row's depth in the tree, and the view
    // whose folder cursor a click clears
    depth: Option<usize>,
    view: Option<WeakEntity<StashDiffViewer>>,
    cx: &App,
) -> AnyElement {
    let t = cx.ghd();
    let hover_bg = t.list_item_hover_background;
    let (icon, color) = status_icon(file.status.kind, t);
    // `.focus-within .list-item.selected`: the icon takes the row's colour
    let color = if is_selected && list_focused {
        t.box_selected_active_text
    } else {
        color
    };
    // `117-file-icons`
    let file_icon = crate::file_icons::file_icons(cx).map(|icons| {
        let color = match (is_selected, list_focused) {
            (true, true) => t.box_selected_active_text,
            (true, false) => t.box_selected_text,
            _ => t.text_secondary,
        };
        div()
            .flex_none()
            .when_some(depth, |d, depth| {
                d.ml(crate::file_tree_rows::file_label_margin(depth))
            })
            .child(crate::file_icons::file_icon(&icons, &file.path, color, cx))
    });
    let has_icon = file_icon.is_some();
    let path = file.path.clone();
    // `1315-discard-stash-file`
    let menu = corvene_core::AppState::global(cx)
        .read(cx)
        .flags
        .bool(corvene_core::flags::ids::DISCARD_STASH_FILE);
    let menu_path = file.path.clone();
    div()
        .id(SharedString::from(format!("stash-file-{}", file.path)))
        .group("stash-file-row")
        .a11y_row(
            format!(
                "{}, {}",
                file.path,
                crate::widgets::status_label(&file.status)
            ),
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
        .when(is_selected, |d| {
            if list_focused {
                d.bg(t.box_selected_active_background)
                    .text_color(t.box_selected_active_text)
            } else {
                d.bg(t.box_selected_background)
                    .text_color(t.box_selected_text)
            }
        })
        // `.list-item:hover` outranks `.list-item.selected` (flag 104 keeps it)
        .when(
            !(is_selected && (list_focused || crate::widgets::selection_keeps_colour_on_hover(cx))),
            move |d| d.hover(move |s| s.bg(hover_bg)),
        )
        .on_click(move |_, _, cx| {
            if let Some(view) = &view {
                view.update(cx, |this, _| this.tree.clear_cursor()).ok();
            }
            Dispatcher::select_stash_file(id, path.clone(), cx)
        })
        .when(menu, |d| {
            d.on_mouse_down(
                MouseButton::Right,
                move |ev: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    if !is_selected {
                        Dispatcher::select_stash_file(id, menu_path.clone(), cx);
                    }
                    open_stash_file_menu(id, &menu_path, ev.position, window, cx);
                },
            )
        })
        .children(file_icon)
        .child({
            // GHD `PathLabel`; `.list-item.selected .dirname` inherits the
            // row colour
            let (directory_color, arrow_color) = match (is_selected, list_focused) {
                (true, true) => (t.box_selected_active_text, t.box_selected_active_text),
                (true, false) => (t.box_selected_text, t.box_selected_text),
                _ => (t.text_secondary, t.text),
            };
            match depth {
                // `1310-file-list-tree`: the name under its folder
                Some(depth) => crate::path_label::path_label_element(
                    crate::file_tree_rows::tree_label(
                        &file.path,
                        file.status.kind,
                        file.old_path.as_deref(),
                    ),
                    Vec::new(),
                    directory_color,
                    arrow_color,
                )
                .when(!has_icon, |d| {
                    d.ml(crate::file_tree_rows::file_label_margin(depth))
                }),
                None => crate::path_label::path_label_element(
                    crate::path_label::path_label(
                        &file.path,
                        file.status.kind,
                        file.old_path.as_deref(),
                    ),
                    Vec::new(),
                    directory_color,
                    arrow_color,
                ),
            }
            .text_size(FONT_SIZE())
        })
        // `1328-archive-entries-in-file-list`
        .children(crate::archive_list_rows::toggle(
            id,
            ArchiveSurface::Stash,
            &file.path,
            (is_selected && list_focused).then_some(t.box_selected_active_text),
            cx,
        ))
        // `1319-trivial-change-icon`
        .child(crate::trivial_change::status_icon(
            icon,
            color,
            is_selected && list_focused,
            crate::trivial_change::lookup(
                id,
                crate::trivial_change::FileList::Stash,
                &file.path,
                cx,
            ),
            cx,
        ))
        // `621-context-menu-buttons`
        .when(menu && crate::context_menu::row_menu_buttons(cx), |d| {
            d.child(
                crate::context_menu::row_menu_button(
                    "row-menu",
                    "stash-file-row",
                    is_selected,
                    color,
                )
                .ml(SPACING_HALF()),
            )
        })
        .into_any_element()
}

/// `1315-discard-stash-file`: a stash file's context menu.
fn open_stash_file_menu(
    id: u64,
    path: &str,
    position: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    use crate::context_menu::{MenuItem, mac_or};
    let Some(repo) = AppState::global(cx)
        .read(cx)
        .repository(id)
        .map(|r| r.path.clone())
    else {
        return;
    };
    let full = repo.join(path).to_string_lossy().into_owned();
    let relative = path.to_string();
    let discard = path.to_string();
    let items = vec![
        MenuItem::new(
            mac_or("Discard from Stash", "Discard from stash"),
            move |_, cx| Dispatcher::discard_stash_file(id, discard.clone(), cx),
        ),
        MenuItem::separator(),
        MenuItem::new(mac_or("Copy File Path", "Copy file path"), move |_, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(full.clone()))
        }),
        MenuItem::new(
            mac_or("Copy Relative File Path", "Copy relative file path"),
            move |_, cx| cx.write_to_clipboard(ClipboardItem::new_string(relative.clone())),
        ),
    ];
    crate::native_menu::show_context_menu(items, position, window, cx);
}

impl Render for StashDiffViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.file_list_focused = self.file_list_focus.is_focused(window);
        let t = cx.ghd().clone();
        let id = self.state.read(cx).selected;
        let Some(id) = id else {
            return div().size_full().into_any_element();
        };
        // `797-stash-list`: the entry's own title, and Apply
        let (title, apply) = {
            let s = self.state.read(cx);
            let shown = s.selected_state().and_then(|rs| rs.shown_stash());
            if s.flags.bool(corvene_core::flags::ids::STASH_LIST) {
                (
                    shown.map_or_else(
                        || "Stashed changes".to_string(),
                        corvene_core::stash_list::stash_title,
                    ),
                    shown.map(|e| e.sha.clone()),
                )
            } else {
                ("Stashed changes".to_string(), None)
            }
        };
        let file_list = self.file_list(id, cx);
        div()
            .id("stash-diff-viewer")
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .bg(t.background)
            .child(
                // `.header`
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .px(SPACING_DOUBLE())
                    .py(zpx(30.))
                    .border_b_1()
                    .border_color(t.box_border)
                    .child(
                        div()
                            .mb(SPACING())
                            .text_size(zpx(32.))
                            .line_height(zpx(32.))
                            .font_weight(FontWeight::LIGHT)
                            .truncate()
                            .child(title),
                    )
                    .child(
                        div()
                            .mt(SPACING())
                            .flex()
                            .flex_row()
                            .items_center()
                            .child(
                                primary_button("stash-restore", "Restore", false, cx)
                                    .mr(SPACING())
                                    .on_click(move |_, _, cx| Dispatcher::pop_stash(id, cx)),
                            )
                            .when_some(apply, |d, sha| {
                                d.child(button("stash-apply", "Apply", cx).mr(SPACING()).on_click(
                                    move |_, _, cx| {
                                        Dispatcher::apply_stash_entry(id, sha.clone(), cx)
                                    },
                                ))
                            })
                            .child(
                                button("stash-discard", "Discard", cx)
                                    .mr(SPACING())
                                    .on_click(move |_, _, cx| {
                                        Dispatcher::request_drop_stash(id, cx)
                                    }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .ml(SPACING_HALF())
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .text_size(FONT_SIZE())
                                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Restore"))
                                    .child(
                                        "\u{a0}will move your stashed files to the Changes list.",
                                    ),
                            ),
                    ),
            )
            .child(
                h_resizable("stash-details")
                    .with_state(&self.resizable)
                    .with_handle_appearance(std::rc::Rc::new(|_, _, _| {
                        Some(div().into_any_element())
                    }))
                    .child(
                        resizable_panel()
                            .size(self.file_list_width)
                            .size_range(FILE_LIST_MIN()..FILE_LIST_MAX())
                            .child(
                                crate::active_resizable::active_resizable(
                                    "stash-file-list-resizable",
                                    &self.resizable,
                                    Some(&self.file_list_focus),
                                    crate::active_resizable::ResizableDescription::new(
                                        "Stash file list",
                                        FILE_LIST_MIN()..FILE_LIST_MAX(),
                                    ),
                                    file_list,
                                )
                                // `1310-file-list-tree`: ← / → fold the tree's folders
                                .key_context(if crate::file_tree_rows::tree_mode(cx) {
                                    "StashFileList FileTree"
                                } else {
                                    "StashFileList"
                                })
                                .on_action(cx.listener(
                                    |this, _: &crate::actions::CollapseFolder, window, cx| {
                                        this.tree_key(false, window, cx)
                                    },
                                ))
                                .on_action(cx.listener(
                                    |this, _: &crate::actions::ExpandFolder, window, cx| {
                                        this.tree_key(true, window, cx)
                                    },
                                ))
                                .on_action(cx.listener(|this, _: &SelectNextFile, _, cx| {
                                    this.select_relative(1, cx)
                                }))
                                .on_action(cx.listener(|this, _: &SelectPreviousFile, _, cx| {
                                    this.select_relative(-1, cx)
                                }))
                                .on_action(cx.listener(|this, _: &ExtendSelectionDown, _, cx| {
                                    this.select_relative(1, cx)
                                }))
                                .on_action(cx.listener(|this, _: &ExtendSelectionUp, _, cx| {
                                    this.select_relative(-1, cx)
                                }))
                                .on_action(cx.listener(|this, _: &SelectFirstFile, _, cx| {
                                    this.select_edge(false, cx)
                                }))
                                .on_action(cx.listener(
                                    |this, _: &SelectLastFile, _, cx| this.select_edge(true, cx),
                                )),
                            ),
                    )
                    .child(
                        resizable_panel().child(
                            div()
                                .size_full()
                                .flex()
                                .flex_col()
                                .min_h_0()
                                // GHD `StashDiffViewer` renders the
                                // `SeamlessDiffSwitcher` alone: no file
                                // header above the diff
                                .child(DiffView::embed(&self.diff)),
                        ),
                    ),
            )
            .into_any_element()
    }
}
