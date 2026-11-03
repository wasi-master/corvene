//! Worktree foldout (GHD `ui/worktrees/worktree-list.tsx`, `worktree-list-item.tsx`,
//! `worktree-list-item-context-menu.ts`, `styles/ui/_worktrees.scss`): filter,
//! "New Worktree", the Main / Linked groups, and the context menus shared
//! with the toolbar button (`ui/toolbar/worktree-dropdown.tsx`).
//!
//! Corvene addition (flag `240-worktree-paths`): rows have a tooltip with the
//! name and full path, and the filter also matches the path, so worktrees
//! with the same folder name can be told apart.
//! Deviation: worktrees git reports `prunable` (directory deleted outside
//! git) are not listed (`243-hide-prunable-worktrees`); a new worktree's
//! default folder is the one holding the main worktree, not the clone
//! folder (`242-worktree-dir-beside-repository`).

use std::path::PathBuf;

use corvene_core::{AppState, Dispatcher, Popup, WorktreeEntry, WorktreeType};
use gpui_kit::component::input::InputState;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::actions::{FilterListPick, SelectNextFile, SelectPreviousFile};
use crate::context_menu::{ContextMenu, MenuItem, mac_or};
use crate::icons::{Octicon, octicon};
use crate::scrollbar::ScrollbarExt;
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::GhdTooltip;
use crate::widgets::ListRowA11y;
use crate::widgets::button;

/// GHD `RowHeight` of the worktree list.
#[allow(non_snake_case)]
fn WORKTREE_ROW_HEIGHT() -> Pixels {
    zpx(30.)
}

/// GHD `generateWorktreeContextMenuItems`.
pub fn worktree_menu_items(
    repo: u64,
    worktree: &WorktreeEntry,
    with_rename: bool,
    with_delete: bool,
) -> Vec<MenuItem> {
    let path = worktree.path.clone();
    let is_main = worktree.kind == WorktreeType::Main;
    let editable = !is_main && !worktree.is_locked;
    let name = worktree.display_name();
    let mut items = Vec::new();
    if with_rename {
        let p = path.clone();
        items.push(
            MenuItem::new("Rename…", move |_, cx| {
                Dispatcher::close_foldout(cx);
                Dispatcher::show_popup(
                    Popup::RenameWorktree {
                        repo,
                        path: p.clone(),
                    },
                    cx,
                );
            })
            .enabled(editable),
        );
    }
    items.push(MenuItem::new(
        mac_or("Copy Worktree Name", "Copy worktree name"),
        move |_, cx| cx.write_to_clipboard(ClipboardItem::new_string(name.clone())),
    ));
    let path_text = path.display().to_string();
    items.push(MenuItem::new(
        mac_or("Copy Worktree Path", "Copy worktree path"),
        move |_, cx| cx.write_to_clipboard(ClipboardItem::new_string(path_text.clone())),
    ));
    items.push(MenuItem::separator());
    if with_delete {
        items.push(
            MenuItem::new("Delete…", move |_, cx| {
                Dispatcher::close_foldout(cx);
                Dispatcher::request_delete_worktree(repo, path.clone(), cx);
            })
            .enabled(editable),
        );
    }
    items
}

/// The worktrees the foldout lists: with `243-hide-prunable-worktrees` not
/// those git reports `prunable` (their directory is gone; GHD lists them and
/// selecting one fails).
pub fn listed_worktrees(state: &AppState, worktrees: &[WorktreeEntry]) -> Vec<WorktreeEntry> {
    let hide = state
        .flags
        .bool(corvene_core::flags::ids::HIDE_PRUNABLE_WORKTREES);
    worktrees
        .iter()
        .filter(|w| !(hide && w.is_prunable))
        .cloned()
        .collect()
}

/// The worktree the repository currently points at.
pub fn current_worktree(state: &AppState, repo: u64) -> Option<WorktreeEntry> {
    let path = &state.repository(repo)?.path;
    state
        .repo_states
        .get(&repo)?
        .worktrees
        .iter()
        .find(|w| &w.path == path)
        .cloned()
}

/// GHD `WorktreeDropdown.onContextMenu`: right-click on the toolbar button.
pub fn toolbar_button_menu(position: Point<Pixels>, window: &mut Window, cx: &mut App) {
    let (repo, current) = {
        let s = AppState::global(cx).read(cx);
        let Some(id) = s.selected else { return };
        (id, current_worktree(s, id))
    };
    let Some(current) = current else { return };
    let is_main = current.kind == WorktreeType::Main;
    let mut items = vec![
        MenuItem::new(
            mac_or("New Worktree…", "New worktree…"),
            move |_, cx| {
                Dispatcher::close_foldout(cx);
                Dispatcher::show_popup(
                    Popup::AddWorktree {
                        repo,
                        initial_branch_name: None,
                        initial_worktree_name: None,
                    },
                    cx,
                );
            },
        ),
        MenuItem::separator(),
    ];
    items.extend(worktree_menu_items(repo, &current, false, !is_main));
    show_menu(items, position, window, cx);
}

fn show_menu(items: Vec<MenuItem>, position: Point<Pixels>, window: &mut Window, cx: &mut App) {
    crate::native_menu::show_context_menu(items, position, window, cx);
}

pub struct WorktreeFoldout {
    state: Entity<AppState>,
    filter: Entity<InputState>,
    context_menu: Option<Entity<ContextMenu>>,
    /// GHD `FilterList` keyboard selection: the row ↓ / ↑ moved to from
    /// the filter box (an index into the rows as shown, groups flattened).
    highlighted: Option<usize>,
    scroll: ScrollHandle,
    /// Shift+F10 / Menu on the keyboard row, else the current worktree
    menu_anchor: crate::context_menu::RowMenuAnchor,
}

impl WorktreeFoldout {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
        cx.observe(&filter, |this: &mut Self, _, cx| {
            this.highlighted = None;
            cx.notify()
        })
        .detach();
        let scroll = ScrollHandle::new();
        Self {
            state,
            filter,
            context_menu: None,
            highlighted: None,
            menu_anchor: crate::context_menu::RowMenuAnchor::new(&scroll),
            scroll,
        }
    }

    /// The Main / Linked groups (GHD `WorktreeList` groups) as shown for
    /// the filter text; empty groups are dropped.
    fn groups(&self, cx: &App) -> Vec<(&'static str, Vec<WorktreeEntry>)> {
        let query = self.filter.read(cx).value().trim().to_lowercase();
        let s = self.state.read(cx);
        let Some(id) = s.selected else {
            return Vec::new();
        };
        let worktrees = s
            .repo_states
            .get(&id)
            .map(|rs| listed_worktrees(s, &rs.worktrees))
            .unwrap_or_default();
        let match_path = s.flags.bool(corvene_core::flags::ids::WORKTREE_PATHS);
        let matches = |w: &WorktreeEntry| {
            query.is_empty()
                || w.display_name().to_lowercase().contains(&query)
                || (match_path && w.path.to_string_lossy().to_lowercase().contains(&query))
        };
        [
            ("Main Worktree", WorktreeType::Main),
            ("Linked Worktrees", WorktreeType::Linked),
        ]
        .into_iter()
        .map(|(label, kind)| {
            let items: Vec<WorktreeEntry> = worktrees
                .iter()
                .filter(|w| w.kind == kind && matches(w))
                .cloned()
                .collect();
            (label, items)
        })
        .filter(|(_, items)| !items.is_empty())
        .collect()
    }

    /// GHD `ui/lib/filter-list.tsx` `onFilterKeyDown` (`SectionFilterList`
    /// in `WorktreeList`): ↓ / ↑ in the filter box move through the rows
    /// (↑ from the filter starts at the last), clamped, skipping the group
    /// headers.
    fn move_highlight(&mut self, delta: isize, cx: &mut Context<Self>) {
        let sizes: Vec<usize> = self.groups(cx).iter().map(|(_, g)| g.len()).collect();
        let Some(ix) = crate::filter_list::step(self.highlighted, delta, sizes.iter().sum()) else {
            return;
        };
        self.highlighted = Some(ix);
        let top = crate::filter_list::row_top(&sizes, ix, ROW_HEIGHT(), WORKTREE_ROW_HEIGHT());
        crate::filter_list::scroll_into_view(&self.scroll, top, WORKTREE_ROW_HEIGHT());
        cx.notify();
    }

    /// GHD `ui/lib/filter-list.tsx` `onFilterKeyDown` (Enter): the
    /// highlighted worktree, else - with a filter typed - the first one,
    /// switched to as a click does (`WorktreeList.onItemClick`).
    fn pick_highlighted(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.state.read(cx).selected else {
            return;
        };
        let query_empty = self.filter.read(cx).value().trim().is_empty();
        let ix = self.highlighted.or_else(|| (!query_empty).then_some(0));
        let path = ix.and_then(|ix| {
            self.groups(cx)
                .into_iter()
                .flat_map(|(_, g)| g)
                .nth(ix)
                .map(|w| w.path)
        });
        if let Some(path) = path {
            Dispatcher::close_foldout(cx);
            Dispatcher::switch_worktree(id, path, cx);
        }
    }

    pub fn focus_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.highlighted = None;
        let handle = self.filter.read(cx).focus_handle(cx);
        handle.focus(window, cx);
        cx.notify();
    }

    fn open_item_menu(
        &mut self,
        repo: u64,
        worktree: &WorktreeEntry,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = worktree_menu_items(repo, worktree, true, true);
        crate::native_menu::show_context_menu(items, position, window, cx);
    }

    /// `.worktrees-list-item`: icon, name (match in bold), branch / sha.
    fn row(
        &self,
        repo: u64,
        worktree: &WorktreeEntry,
        current: bool,
        highlighted: bool,
        query: &str,
        cx: &Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd();
        let name = worktree.display_name();
        let description = worktree.description();
        // `.list-item:hover`: `--list-item-hover-background-color`, text unchanged
        let list_hover = t.list_item_hover_background;
        let path = worktree.path.clone();
        let show_path = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::WORKTREE_PATHS);
        let for_menu = worktree.clone();
        let title: AnyElement = match (!query.is_empty())
            .then(|| name.to_lowercase().find(&query.to_lowercase()))
            .flatten()
        {
            Some(ix) => {
                let end = (ix + query.len()).min(name.len());
                div()
                    .flex()
                    .flex_row()
                    .overflow_hidden()
                    .child(name[..ix].to_string())
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .child(name[ix..end].to_string()),
                    )
                    .child(name[end..].to_string())
                    .into_any_element()
            }
            None => div().truncate().child(name.clone()).into_any_element(),
        };
        div()
            .id(SharedString::from(format!("worktree-{}", path.display())))
            .a11y_row(format!("{name}, {description}"), current)
            .when(show_path, |d| {
                d.ghd_tooltip(format!("{name}\n{}", path.display()))
            })
            .h(WORKTREE_ROW_HEIGHT())
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .px(SPACING())
            .cursor_pointer()
            .text_size(FONT_SIZE())
            .hover(move |s| s.bg(list_hover))
            // the keyboard row (GHD's focused-list selection)
            .when(highlighted, |d| {
                d.bg(t.box_selected_active_background)
                    .text_color(t.box_selected_active_text)
                    // `623-contrast-focus-ring`
                    .when_some(crate::keyboard_nav::contrast_row_outline(cx), |d, ring| {
                        d.shadow(ring)
                    })
            })
            .on_click(move |_, _, cx| {
                Dispatcher::close_foldout(cx);
                Dispatcher::switch_worktree(repo, path.clone(), cx);
            })
            .when(
                highlighted || (current && self.highlighted.is_none()),
                |d| d.relative().child(self.menu_anchor.track()),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                    this.open_item_menu(repo, &for_menu, ev.position, window, cx)
                }),
            )
            .child(
                octicon(
                    if current {
                        Octicon::Check
                    } else {
                        Octicon::FileDirectory
                    },
                    if current || highlighted {
                        t.text
                    } else {
                        t.text_secondary
                    },
                )
                .flex_none()
                .mr(SPACING_HALF()),
            )
            .child(
                div()
                    .flex_grow(2.)
                    .min_w_0()
                    .max_w(gpui_kit::relative(0.65))
                    .mr(SPACING_HALF())
                    .child(title),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .mr(SPACING_HALF())
                    .text_right()
                    .whitespace_nowrap()
                    .truncate()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(description),
            )
            .into_any_element()
    }
}

impl Render for WorktreeFoldout {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd();
        let query = self.filter.read(cx).value().trim().to_string();
        let (id, current) = {
            let s = self.state.read(cx);
            let Some(id) = s.selected else {
                return div().into_any_element();
            };
            (id, current_worktree(s, id).map(|w| w.path))
        };
        let shown = self.groups(cx);
        let row_count: usize = shown.iter().map(|(_, g)| g.len()).sum();
        self.highlighted = self.highlighted.filter(|ix| *ix < row_count);
        let highlighted = self.highlighted;
        let mut row_ix = 0;
        let group_header = |label: &str| {
            // `.filter-list-group-header`
            div()
                .h(ROW_HEIGHT())
                .pt(SPACING())
                .px(SPACING())
                .flex()
                .items_center()
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(FONT_SIZE())
                .truncate()
                .child(label.to_string())
        };
        let mut groups: Vec<AnyElement> = Vec::new();
        for (label, items) in shown {
            let first = row_ix;
            row_ix += items.len();
            groups.push(
                div()
                    .flex()
                    .flex_col()
                    .child(group_header(label))
                    .children(items.iter().enumerate().map(|(ix, w)| {
                        let is_current = current.as_ref() == Some(&w.path);
                        self.row(
                            id,
                            w,
                            is_current,
                            highlighted == Some(first + ix),
                            &query,
                            cx,
                        )
                    }))
                    .into_any_element(),
            );
        }
        div()
            .id("worktrees-container")
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .child(
                // `.filter-field-row`: [🔍 Filter][New Worktree]
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING())
                    .p(SPACING())
                    .key_context("WorktreeFilter")
                    .on_action(self.menu_anchor.action_handler())
                    .on_action(
                        cx.listener(|this, _: &SelectNextFile, _, cx| this.move_highlight(1, cx)),
                    )
                    .on_action(cx.listener(|this, _: &SelectPreviousFile, _, cx| {
                        this.move_highlight(-1, cx)
                    }))
                    .on_action(
                        cx.listener(|this, _: &FilterListPick, _, cx| this.pick_highlighted(cx)),
                    )
                    .child(crate::widgets::filter_text_box(
                        "worktree-filter",
                        &self.filter,
                        Some(octicon(Octicon::Search, t.text_secondary)),
                        window,
                        cx,
                    ))
                    .child(
                        button("new-worktree", mac_or("New Worktree", "New worktree"), cx)
                            .flex_none()
                            .on_click(move |_, _, cx| {
                                Dispatcher::close_foldout(cx);
                                Dispatcher::show_popup(
                                    Popup::AddWorktree {
                                        repo: id,
                                        initial_branch_name: None,
                                        initial_worktree_name: None,
                                    },
                                    cx,
                                );
                            }),
                    ),
            )
            .child(if groups.is_empty() {
                // `.no-items-found`
                div()
                    .p(SPACING())
                    .text_center()
                    .text_size(FONT_SIZE())
                    .text_color(t.text_secondary)
                    .child("No worktrees found")
                    .into_any_element()
            } else {
                div()
                    .id("worktrees-list")
                    .role(Role::List)
                    .aria_label("Worktrees")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .children(groups)
                    .with_scrollbar_handle(&self.scroll)
                    .into_any_element()
            })
            .children(self.context_menu.clone())
            .into_any_element()
    }
}

/// Where a new worktree goes by default (GHD `RepositoryPath` → clone dir).
/// With `242-worktree-dir-beside-repository`: the folder holding the
/// repository's main worktree, so new worktrees become its siblings;
/// otherwise flag `241-worktree-location`'s template.
pub fn default_worktree_dir(state: &AppState, repo: u64) -> PathBuf {
    let beside = state
        .flags
        .bool(corvene_core::flags::ids::WORKTREE_DIR_BESIDE_REPOSITORY);
    let main = || {
        let main = state
            .repo_states
            .get(&repo)
            .and_then(|rs| rs.worktrees.iter().find(|w| w.kind == WorktreeType::Main))
            .map(|w| w.path.clone());
        main.or_else(|| state.repository(repo).map(|r| r.path.clone()))
    };
    if let Some(dir) = beside
        .then(main)
        .flatten()
        .and_then(|p| p.parent().map(PathBuf::from))
    {
        return dir;
    }
    let clone_dir = state.clone_dir();
    let name = state
        .repository(repo)
        .map(|r| match &r.github {
            Some(gh) => gh.name.clone(),
            None => r
                .main_worktree_path
                .as_ref()
                .unwrap_or(&r.path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        })
        .unwrap_or_default();
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    corvene_core::worktrees::worktree_location(
        state
            .flags
            .text(corvene_core::flags::ids::WORKTREE_LOCATION),
        &clone_dir,
        &name,
        home.as_deref(),
    )
}
