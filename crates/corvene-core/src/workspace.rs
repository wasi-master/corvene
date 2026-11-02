//! Workspaces: the state of one window (Corvene `429-multiple-windows`,
//! `430-repository-tabs`; design in `.docs/multiple-windows.md`). GitHub
//! Desktop has one window, so its `AppState` holds the selected repository,
//! the open foldout, the popup stack, the banner and the drag target as
//! single slots. Here those live in a [`WorkspaceState`], one per window,
//! and [`AppState`](crate::AppState) derefs to the *current* one, so the
//! dispatcher and the views keep reading `s.selected`, `s.foldout` and
//! `s.popup()` for the window their code runs for.
//!
//! Two cursors on `AppState`: `focused` is the workspace of the window last
//! active (window-level work such as URL routing and the menu bar resolve
//! to it), `current` is the one code is running for right now. Outside a
//! window's render `current == focused`; `Workspace::render` moves it to its
//! own workspace for the frame and the observer wrappers in
//! `corvene_ui::windows` do the same for a view's state observers.
//! With both flags off there is exactly one workspace and the cursors never
//! move, which is GitHub Desktop's behaviour.

use serde::{Deserialize, Serialize};
use tracing::info;

use corvene_models::Repository;
use corvene_store::Store;

use crate::dispatcher::Dispatcher;
use crate::flags::Flags;
use crate::host::Host;
use crate::mco::Banner;
use crate::navigation::NavigationHistory;
use crate::popup_manager::PopupManager;
use crate::pull_requests::BranchesTab;
use crate::state::{DropTarget, Foldout};

/// A workspace's identity for the session; never reused once closed.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default,
)]
pub struct WorkspaceId(pub u32);

impl WorkspaceId {
    /// The workspace the main window shows.
    pub const FIRST: WorkspaceId = WorkspaceId(1);
}

impl std::fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// What one window shows: GHD's `selectedState.repository`, `currentFoldout`,
/// the `PopupManager`, `currentBanner`, the navigation history and the
/// smaller per-window slots.
#[derive(Clone, Debug)]
pub struct WorkspaceState {
    pub id: WorkspaceId,
    pub selected: Option<u64>,
    /// `430-repository-tabs`: the repositories open as tabs, in strip
    /// order; `selected` is one of them while the flag is on.
    pub tabs: Vec<u64>,
    pub foldout: Option<Foldout>,
    /// The open popups (GHD `PopupManager`); `AppState::popup` is the one
    /// shown.
    pub popups: PopupManager,
    /// `currentBanner`
    pub banner: Option<Banner>,
    pub banner_nonce: u64,
    /// Corvene (`427-back-forward-navigation`): View › Back / Forward.
    pub navigation: NavigationHistory,
    /// `selectedBranchesTab`
    pub branches_tab: BranchesTab,
    /// View › Show Changes Filter: hidden in this window.
    pub changes_filter_hidden: bool,
    /// `showCIStatusPopover`: the check-run popover under the PR badge.
    pub show_ci_status_popover: bool,
    /// `dragAndDropManager` drop target during a commit drag.
    pub drag_target: Option<DropTarget>,
}

impl WorkspaceState {
    pub fn new(id: WorkspaceId, selected: Option<u64>) -> Self {
        Self {
            id,
            selected,
            tabs: selected.into_iter().collect(),
            foldout: None,
            popups: PopupManager::new(),
            banner: None,
            banner_nonce: 0,
            navigation: NavigationHistory::default(),
            branches_tab: BranchesTab::Branches,
            changes_filter_hidden: false,
            show_ci_status_popover: false,
            drag_target: None,
        }
    }

    /// What `ui.workspaces` keeps of this workspace.
    pub fn saved(&self) -> SavedWorkspace {
        SavedWorkspace {
            selected: self.selected,
            tabs: self.tabs.clone(),
        }
    }

    /// `430-repository-tabs`: make `repo` the selected tab. A repository
    /// already open as a tab is switched to; otherwise it takes the place
    /// of the tab that was selected (GHD's switch inside the one view), or
    /// is appended when there is none.
    pub fn select_tab(&mut self, repo: u64) {
        if !self.tabs.contains(&repo) {
            match self
                .selected
                .and_then(|current| self.tabs.iter().position(|t| *t == current))
            {
                Some(index) => self.tabs[index] = repo,
                None => self.tabs.push(repo),
            }
        }
        self.selected = Some(repo);
    }

    /// `430-repository-tabs`: a tab for `repo` after the selected one (or
    /// at the end) unless it is open already; it is not selected.
    pub fn add_tab(&mut self, repo: u64) {
        if self.tabs.contains(&repo) {
            return;
        }
        let at = self
            .selected
            .and_then(|current| self.tabs.iter().position(|t| *t == current))
            .map_or(self.tabs.len(), |index| index + 1);
        self.tabs.insert(at, repo);
    }

    /// `430-repository-tabs`: close the tab of `repo`; when it was the
    /// selected one the right neighbour, then the left, takes over. Returns
    /// the repository now selected when the selection moved.
    pub fn close_tab(&mut self, repo: u64) -> Option<Option<u64>> {
        let index = self.tabs.iter().position(|t| *t == repo)?;
        self.tabs.remove(index);
        if self.selected != Some(repo) {
            return None;
        }
        let next = self.tabs.get(index).or_else(|| self.tabs.last()).copied();
        self.selected = next;
        Some(next)
    }

    /// `430-repository-tabs`: the tab `steps` to the right (negative: left)
    /// of the selected one, wrapping around.
    pub fn stepped_tab(&self, steps: isize) -> Option<u64> {
        let len = self.tabs.len();
        if len < 2 {
            return None;
        }
        let current = self
            .selected
            .and_then(|current| self.tabs.iter().position(|t| *t == current))?;
        let next = (current as isize + steps).rem_euclid(len as isize) as usize;
        self.tabs.get(next).copied()
    }

    /// Drop every trace of a removed repository.
    pub fn forget_repository(&mut self, repo: u64) {
        self.tabs.retain(|t| *t != repo);
        self.navigation.forget(repo);
    }
}

/// The workspaces a launch starts with: the first shows `selected` (GHD's
/// `ui.selected_repository`, validated by the caller) and carries `popups`;
/// with `430-repository-tabs` its tabs come back, with
/// `429-multiple-windows` the other windows do too, each only with
/// repositories that still exist. Returns the list and the next free id.
pub(crate) fn restored_workspaces(
    store: &Store,
    flags: &Flags,
    repositories: &[Repository],
    selected: Option<u64>,
    popups: PopupManager,
) -> (Vec<WorkspaceState>, u32) {
    use crate::persistence::StoreExt;
    let multiple = flags.bool(crate::flags::ids::MULTIPLE_WINDOWS);
    let tabs = flags.bool(crate::flags::ids::REPOSITORY_TABS);
    let exists = |id: &u64| repositories.iter().any(|r| r.id == *id);
    let saved = if multiple || tabs {
        store.workspaces().unwrap_or_default()
    } else {
        Vec::new()
    };
    let mut first = WorkspaceState::new(WorkspaceId::FIRST, selected);
    first.popups = popups;
    if tabs && let Some(saved_first) = saved.first() {
        first.tabs = saved_first.tabs.iter().copied().filter(exists).collect();
        if let Some(selected) = selected
            && !first.tabs.contains(&selected)
        {
            first.tabs.push(selected);
        }
    }
    let mut workspaces = vec![first];
    let mut next_id = WorkspaceId::FIRST.0 + 1;
    if multiple {
        for saved in saved.iter().skip(1) {
            let selected = saved.selected.filter(exists);
            let saved_tabs: Vec<u64> = if tabs {
                saved.tabs.iter().copied().filter(exists).collect()
            } else {
                Vec::new()
            };
            // a window left empty is not worth bringing back
            if selected.is_none() && saved_tabs.is_empty() {
                continue;
            }
            let mut workspace = WorkspaceState::new(WorkspaceId(next_id), selected);
            next_id += 1;
            if !saved_tabs.is_empty() {
                workspace.tabs = saved_tabs;
                if let Some(selected) = selected
                    && !workspace.tabs.contains(&selected)
                {
                    workspace.tabs.push(selected);
                }
            } else if let Some(selected) = selected
                && tabs
            {
                workspace.tabs = vec![selected];
            }
            workspaces.push(workspace);
        }
    }
    if workspaces.len() > 1 {
        info!(windows = workspaces.len(), "restoring windows");
    }
    (workspaces, next_id)
}

/// Save the selection: `ui.selected_repository` is the first workspace's
/// (what a launch with the flags off, or GitHub Desktop's model, sees) and
/// `ui.workspaces` the whole list while a flag keeps it.
pub(crate) fn persist_workspaces(s: &crate::AppState) {
    use crate::persistence::StoreExt;
    let _ = s.store.save_selected_repository(s.workspaces[0].selected);
    if s.persists_workspaces() {
        let _ = s.store.save_workspaces(&s.saved_workspaces());
    }
}

impl Dispatcher {
    /// The window of workspace `id` became active: window-level work
    /// (URLs, notifications, the menu bar) now means this one.
    pub fn activate_workspace(id: WorkspaceId, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            if s.workspace(id).is_none() || (s.focused == id && s.current == id) {
                return;
            }
            s.focused = id;
            s.current = id;
            cx.notify();
        });
    }

    /// Run the code that follows for workspace `id` (a window's frame, a
    /// view's observer); [`Self::leave_workspace`] ends it. Nothing is
    /// notified.
    pub fn enter_workspace(id: WorkspaceId, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, _| {
            if s.workspace(id).is_some() {
                s.current = id;
            }
        });
    }

    /// Back to the focused workspace, where code runs for between frames.
    pub fn leave_workspace(cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, _| s.current = s.focused);
    }

    /// `429-multiple-windows`: a new workspace showing `selected` (none:
    /// File › New Window's empty window); the caller opens its window.
    pub fn open_workspace(selected: Option<u64>, cx: &mut dyn Host) -> WorkspaceId {
        let id = Self::state(cx).update(cx, |s, cx| {
            let tabs = if s.flags.bool(crate::flags::ids::REPOSITORY_TABS) {
                selected.into_iter().collect()
            } else {
                Vec::new()
            };
            let id = s.add_workspace(selected, tabs);
            persist_workspaces(s);
            cx.notify();
            id
        });
        if let Some(repo) = selected {
            Self::refresh_repository(repo, cx);
            Self::start_background_pruner(repo, cx);
            Self::start_watching(repo, cx);
            Self::restart_pull_request_updater(cx);
        }
        id
    }

    /// `429-multiple-windows`: "Open in New Window". `Err` names the
    /// workspace whose window shows the repository already (activate it).
    pub fn open_in_new_workspace(repo: u64, cx: &mut dyn Host) -> Result<WorkspaceId, WorkspaceId> {
        if let Some(showing) = Self::state(cx).read(cx).workspace_showing(repo) {
            return Err(showing);
        }
        Ok(Self::open_workspace(Some(repo), cx))
    }

    /// `429-multiple-windows`: the window of workspace `id` closed. The last
    /// workspace stays (its window hides, as GitHub Desktop's does).
    pub fn close_workspace(id: WorkspaceId, cx: &mut dyn Host) -> bool {
        let removed = Self::state(cx).update(cx, |s, cx| {
            let removed = s.remove_workspace(id);
            if removed {
                persist_workspaces(s);
                cx.notify();
            }
            removed
        });
        if removed {
            Self::stop_unwatched(cx);
            Self::restart_pull_request_updater(cx);
        }
        removed
    }

    /// `430-repository-tabs`: "Open in New Tab": a tab for `repo` in the
    /// current workspace, selected.
    pub fn open_tab(repo: u64, cx: &mut dyn Host) {
        let known = Self::state(cx).update(cx, |s, cx| {
            if s.repository(repo).is_none() {
                return false;
            }
            s.add_tab(repo);
            cx.notify();
            true
        });
        if known {
            Self::select_repository(repo, cx);
        }
    }

    /// `430-repository-tabs`: close the tab of `repo` (the selected one
    /// when `None`); the neighbour takes over. False when the workspace has
    /// no other tab (the caller closes the window instead).
    pub fn close_tab(repo: Option<u64>, cx: &mut dyn Host) -> bool {
        let next = Self::state(cx).update(cx, |s, cx| {
            let repo = repo.or(s.selected)?;
            if s.tabs.len() < 2 {
                return None;
            }
            let moved = s.close_tab(repo);
            persist_workspaces(s);
            cx.notify();
            Some(moved.flatten())
        });
        match next {
            None => false,
            Some(None) => {
                Self::stop_unwatched(cx);
                true
            }
            Some(Some(next)) => {
                Self::select_repository(next, cx);
                true
            }
        }
    }

    /// `430-repository-tabs`: the tab to the right (`forward`) or left.
    pub fn step_tab(forward: bool, cx: &mut dyn Host) {
        let next = Self::state(cx)
            .read(cx)
            .stepped_tab(if forward { 1 } else { -1 });
        if let Some(next) = next {
            Self::select_repository(next, cx);
        }
    }

    /// `430-repository-tabs`: a tab's "Open in New Window" moves it out of
    /// the current workspace (when it has another tab to show) into a new
    /// one. `Err` as in [`Self::open_in_new_workspace`].
    pub fn move_tab_to_new_workspace(
        repo: u64,
        cx: &mut dyn Host,
    ) -> Result<WorkspaceId, WorkspaceId> {
        let can_leave = {
            let s = Self::state(cx).read(cx);
            s.tabs.len() > 1 && s.tabs.contains(&repo)
        };
        if can_leave {
            Self::close_tab(Some(repo), cx);
        }
        Self::open_in_new_workspace(repo, cx)
    }
}

/// One entry of the stored `ui.workspaces` list.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedWorkspace {
    pub selected: Option<u64>,
    #[serde(default)]
    pub tabs: Vec<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(tabs: &[u64], selected: Option<u64>) -> WorkspaceState {
        let mut w = WorkspaceState::new(WorkspaceId(1), selected);
        w.tabs = tabs.to_vec();
        w
    }

    #[test]
    fn select_tab_replaces_the_selected_tab_or_switches() {
        let mut w = ws(&[1, 2, 3], Some(2));
        w.select_tab(9);
        assert_eq!((w.tabs.clone(), w.selected), (vec![1, 9, 3], Some(9)));
        w.select_tab(3);
        assert_eq!((w.tabs.clone(), w.selected), (vec![1, 9, 3], Some(3)));
        let mut empty = ws(&[], None);
        empty.select_tab(4);
        assert_eq!((empty.tabs.clone(), empty.selected), (vec![4], Some(4)));
    }

    #[test]
    fn add_tab_goes_after_the_selected_one_once() {
        let mut w = ws(&[1, 2, 3], Some(1));
        w.add_tab(7);
        w.add_tab(7);
        assert_eq!(w.tabs, vec![1, 7, 2, 3]);
        assert_eq!(w.selected, Some(1));
    }

    #[test]
    fn close_tab_prefers_the_right_neighbour() {
        let mut w = ws(&[1, 2, 3], Some(2));
        assert_eq!(w.close_tab(2), Some(Some(3)));
        assert_eq!(w.tabs, vec![1, 3]);
        assert_eq!(w.close_tab(3), Some(Some(1)));
        assert_eq!(w.close_tab(5), None);
        assert_eq!(w.close_tab(1), Some(None));
        assert!(w.tabs.is_empty());
    }

    #[test]
    fn stepping_wraps() {
        let w = ws(&[1, 2, 3], Some(3));
        assert_eq!(w.stepped_tab(1), Some(1));
        assert_eq!(w.stepped_tab(-1), Some(2));
        assert_eq!(ws(&[1], Some(1)).stepped_tab(1), None);
    }
}
