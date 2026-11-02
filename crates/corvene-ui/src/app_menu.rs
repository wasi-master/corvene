//! The menu bar's template, mirroring GitHub Desktop 3.6.6's
//! `buildDefaultMenuTemplate` (`app/src/main-process/menu/build-default-menu.ts`)
//! over its `MenuLabelsEvent` (`app/src/models/menu-labels.ts`): native on
//! macOS, the model of the in-window menu bar (`menu_bar`, `views_menu`)
//! elsewhere. Items are GHD's, `__DARWIN__` branch on macOS and the other
//! one elsewhere (sentence case with `&` access keys).
//!
//! [`build_default_menu_template`] is pure, so the labels a state produces
//! can be checked without an `App`; [`build_default_menu`] turns the
//! template into the GPUI menus `crates/corvene/src/menus.rs` installs.
//! Keyboard shortcuts are shown from the keymap in [`crate::keymap`]. GPUI
//! disables any item whose action has no live handler, which matches GHD's
//! context-dependent enabling. The Repository menu's push item keeps the
//! `Push` action when it reads Force Push; the binary's handler force-pushes
//! whenever a force push is possible, which is GHD's `force-push` event.
//!
//! [`MenuLabelsEvent::of`] is GHD `updateMenuItemLabels` (`app-store.ts`),
//! except:
//! - `isChangesFilterVisible` stays at GHD's default (shown): whether the
//!   filter is shown lives in the Changes view (`changes.rs`), not in
//!   `AppState`.
//! - The editor and shell are named by `AppState::editor_label` /
//!   `shell_label`, which name a custom one (GHD's `null` reads "External
//!   Editor" / "Shell").
//!
//! Corvene additions ([`MenuExtras`], all off in the github-desktop preset):
//! "Flags…" (no GHD equivalent, always there), File › Import Repositories
//! from GitHub Desktop…, Repository › Fetch All Repositories, Repository ›
//! Pull All Repositories (flag `299-pull-all-repositories`), Repository ›
//! Fetch All Tags (flag `899-tags-in-branch-list`), Repository › Recent
//! Activity… (flag `1216-recent-activity`), Repository › Insights… (flag
//! `1110-repository-insights`), Branch › Compare… (flag `1218-compare-refs`),
//! Branch › Tags… (flag `1219-tag-manager`), Repository › Clean Untracked
//! Files… (flag `1105-clean-untracked-files`), Repository › Apply Patch ▸
//! (flag `1106-apply-patch`), Repository › Submodules… (flag
//! `1111-submodules`), Repository › Sparse Checkout… (flag
//! `1112-sparse-checkout`), Repository › Start
//! Bisect / Stop Bisecting (flag `1212-bisect`), Branch › Push To ▸ and
//! Fetch From ▸ with a repository's remotes when it has several (flag
//! `1210-push-to-other-remote`; the remote Push goes to is checked with
//! `1109-remote-manager`), Branch › Request
//! Reviewers… (flag `336-request-reviewers`), Repository ›
//! View Upstream on GitHub, Repository › Add License… ("A&dd license…" off
//! macOS: `Pu&ll` has the `l`), View › Show Pull Requests List and Toggle
//! History Review Mode, View › Back / Forward (flag
//! `427-back-forward-navigation`), View › Wrap Diff Lines (flag
//! `1304-diff-no-wrap`), File › Remove Repositories… (flag
//! `269-bulk-remove-repositories`), Edit › Undo Last Commit (flag
//! `423-undo-commit-menu-item`, enabled while the Changes tab's Undo bar
//! shows), Window › Corvene (shows the window hidden with ⌘W)
//! and Help › Show Release Notes; off macOS "Flags…" and "Install Command
//! Line Tool…" sit under File after "Options…" (GHD's Linux menu has no app
//! menu). On Android, Repository › Move to shared storage… follows Remove.

use corvene_core::AppState;
use corvene_core::flags::{Flags, ids};
use corvene_core::menu_state::MenuId;
use gpui_kit::{Action, Menu, MenuItem, OsAction, SystemMenuType};

use crate::actions::*;
use crate::keymap::KeymapFlags;

/// GHD `MenuLabelsEvent` (`models/menu-labels.ts`): what the menu's labels
/// depend on, plus Corvene's flag-dependent items. A change rebuilds the
/// menu bar. `Default` is GHD's defaults for the optional fields with
/// nothing selected and every Corvene addition off.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuLabelsEvent {
    /// GHD `selectedShell` (`None`: "Shell")
    pub selected_shell: Option<String>,
    /// GHD `selectedExternalEditor` (`None`: "External Editor")
    pub selected_external_editor: Option<String>,
    pub ask_for_confirmation_on_force_push: bool,
    pub ask_for_confirmation_on_repository_removal: bool,
    /// `findContributionTargetDefaultBranch`'s name (`None`: "Default Branch")
    pub contribution_target_default_branch: Option<String>,
    /// `getCurrentBranchForcePushState(…) !== NotAvailable`
    pub is_force_push_for_current_repository: bool,
    /// `branchesState.currentPullRequest !== null`
    pub has_current_pull_request: bool,
    /// The Changes list shows the stash (`ChangesSelectionKind.Stash`)
    pub is_stashed_changes_visible: bool,
    /// GHD: `changesState.stashEntry !== null` (see the module docs)
    pub ask_for_confirmation_when_stashing_all_changes: bool,
    pub is_changes_filter_visible: bool,
    /// Corvene (`1210-push-to-other-remote`): the remotes of Branch › Push
    /// To ▸ and Fetch From ▸ (`Dispatcher::menu_remotes`; empty: no menus).
    pub remotes: Vec<String>,
    /// Corvene (`1109-remote-manager`): the remote Push sends the current
    /// branch to, checked in Push To ▸ (`None` while the flag is off).
    pub push_remote: Option<String>,
    /// Corvene (`524-open-repository-with-editor`): the editors of
    /// Repository › Open in Editor ▸ (`Dispatcher::menu_editors`; empty: no
    /// menu).
    pub editors: Vec<String>,
    /// Corvene (`427-back-forward-navigation`): whether View › Back /
    /// Forward have a step.
    pub navigation: (bool, bool),
    /// Corvene's flag-dependent items.
    pub extras: MenuExtras,
    /// Corvene (flags 342-344): the selected repository is on GitLab,
    /// Gitea or Bitbucket, so its items say "View on GitLab", "Create
    /// Merge Request"…
    pub host: Option<(corvene_core::HostKind, &'static str)>,
}

impl Default for MenuLabelsEvent {
    fn default() -> Self {
        Self {
            selected_shell: None,
            selected_external_editor: None,
            ask_for_confirmation_on_force_push: false,
            ask_for_confirmation_on_repository_removal: false,
            contribution_target_default_branch: None,
            is_force_push_for_current_repository: false,
            has_current_pull_request: false,
            is_stashed_changes_visible: false,
            ask_for_confirmation_when_stashing_all_changes: true,
            is_changes_filter_visible: true,
            remotes: Vec::new(),
            push_remote: None,
            editors: Vec::new(),
            navigation: (false, false),
            extras: MenuExtras::default(),
            host: None,
        }
    }
}

/// The menu items Corvene's flags add (`Default`: none, the github-desktop
/// preset).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MenuExtras {
    /// Flag `401-release-notes-menu-item`.
    pub show_release_notes: bool,
    /// Flag `206-import-from-github-desktop`.
    pub show_import: bool,
    /// Flag `321-view-upstream-on-github`.
    pub show_view_upstream: bool,
    /// Flag `405-window-menu-main-window`.
    pub show_main_window: bool,
    /// Flag `429-multiple-windows`: File › New Window.
    pub show_new_window: bool,
    /// Flag `430-repository-tabs`: Window › Show Next / Previous Tab.
    pub tabs: bool,
    /// Flag `221-add-license`.
    pub show_add_license: bool,
    /// Flag `247-fetch-all-repositories`.
    pub fetch_all: bool,
    /// Flag `299-pull-all-repositories`.
    pub pull_all: bool,
    /// Flag `899-tags-in-branch-list`: Repository › Fetch All Tags.
    pub fetch_tags: bool,
    /// Flag `1212-bisect`: Repository › Start Bisect / Stop Bisecting,
    /// `Some(true)` while the repository bisects (set by
    /// [`MenuLabelsEvent::of`]).
    pub bisect: Option<bool>,
    /// Flag `1216-recent-activity`: Repository › Recent Activity….
    pub recent_activity: bool,
    /// Flag `1110-repository-insights`: Repository › Insights….
    pub insights: bool,
    /// Flag `1218-compare-refs`: Branch › Compare….
    pub compare_refs: bool,
    /// Flag `1219-tag-manager`: Branch › Tags….
    pub tags: bool,
    /// Flag `1223-checkout-from-fork`: Branch › Check Out from Fork….
    pub checkout_from_fork: bool,
    /// Flag `1105-clean-untracked-files`: Repository › Clean Untracked
    /// Files….
    pub clean_untracked: bool,
    /// Flag `1106-apply-patch`: Repository › Apply Patch ▸ From File… /
    /// From Clipboard.
    pub apply_patch: bool,
    /// Flag `1111-submodules`: Repository › Submodules….
    pub submodules: bool,
    /// Flag `1112-sparse-checkout`: Repository › Sparse Checkout….
    pub sparse_checkout: bool,
    /// Flag `345-issues`: Repository › Issues… and New Issue…, enabled
    /// for a GitHub repository that is not archived (set by
    /// [`MenuLabelsEvent::of`]).
    pub issues: Option<bool>,
    /// Flag `346-releases`: Repository › Releases… and Create Release…,
    /// enabled for a GitHub repository (set by [`MenuLabelsEvent::of`]).
    pub releases: Option<bool>,
    /// Flag `351-actions`: Repository › Actions…, enabled for a GitHub
    /// repository (set by [`MenuLabelsEvent::of`]).
    pub actions: Option<bool>,
    /// Flag `336-request-reviewers`: Branch › Request Reviewers… (while the
    /// branch has a pull request).
    pub request_reviewers: bool,
    /// Flag `348-pull-request-review`: Branch › Review Pull Request… (while
    /// the branch has a pull request).
    pub review_pull_request: bool,
    /// Flag `283-move-changes-to-worktree`: Branch › Move Changes to
    /// Worktree…, enabled with changes and another worktree (set by
    /// [`MenuLabelsEvent::of`]).
    pub move_changes_to_worktree: Option<bool>,
    /// Flag `797-stash-list`: Branch › Stash All Changes with Message…,
    /// enabled like Stash All Changes (set by [`MenuLabelsEvent::of`]).
    pub stash_with_message: Option<bool>,
    /// Flag `414-linux-install-cli` (the item is always there on macOS).
    pub install_cli: bool,
    /// Flag `269-bulk-remove-repositories`.
    pub show_remove_repositories: bool,
    /// Flag `423-undo-commit-menu-item`: Edit › Undo Last Commit, enabled
    /// while the Changes tab's Undo bar shows (set by
    /// [`MenuLabelsEvent::of`]).
    pub undo_last_commit: Option<bool>,
    /// Flag `1304-diff-no-wrap`: View › Wrap Diff Lines, checked while
    /// diff lines wrap (set by [`MenuLabelsEvent::of`]).
    pub diff_wrap_lines: Option<bool>,
    /// Flag `1310-file-list-tree`: View › Show Changes as Tree / as List,
    /// `true` while file lists show trees (set by [`MenuLabelsEvent::of`]).
    pub file_list_tree: Option<bool>,
    /// `431-repository-sidebar`: the sidebar is showing.
    pub repository_sidebar: Option<bool>,
    /// `432-history-file-list`: where the list is.
    pub history_file_list: Option<corvene_core::HistoryFileList>,
    /// Flags that add key bindings and their View menu items
    /// (`612-navigation-shortcuts`, `801-history-review-mode`).
    pub keymap: KeymapFlags,
}

impl MenuExtras {
    pub fn of(flags: &Flags) -> Self {
        Self {
            show_release_notes: flags.bool(ids::RELEASE_NOTES_MENU_ITEM),
            // GitHub Desktop does not run on Android
            show_import: !cfg!(target_os = "android")
                && flags.bool(ids::IMPORT_FROM_GITHUB_DESKTOP),
            show_view_upstream: flags.bool(ids::VIEW_UPSTREAM_ON_GITHUB),
            show_main_window: flags.bool(ids::WINDOW_MENU_MAIN_WINDOW),
            show_new_window: flags.bool(ids::MULTIPLE_WINDOWS),
            tabs: flags.bool(ids::REPOSITORY_TABS),
            show_add_license: flags.bool(ids::ADD_LICENSE),
            fetch_all: flags.bool(ids::FETCH_ALL_REPOSITORIES),
            pull_all: flags.bool(ids::PULL_ALL_REPOSITORIES),
            fetch_tags: flags.bool(ids::TAGS_IN_BRANCH_LIST),
            bisect: flags.bool(ids::BISECT).then_some(false),
            recent_activity: flags.bool(ids::RECENT_ACTIVITY),
            insights: flags.bool(ids::REPOSITORY_INSIGHTS),
            compare_refs: flags.bool(ids::COMPARE_REFS),
            tags: flags.bool(ids::TAG_MANAGER),
            checkout_from_fork: flags.bool(ids::CHECKOUT_FROM_FORK),
            clean_untracked: flags.bool(ids::CLEAN_UNTRACKED_FILES),
            apply_patch: flags.bool(ids::APPLY_PATCH),
            submodules: flags.bool(ids::SUBMODULES),
            sparse_checkout: flags.bool(ids::SPARSE_CHECKOUT),
            issues: flags.bool(ids::ISSUES).then_some(false),
            releases: flags.bool(ids::RELEASES).then_some(false),
            actions: flags.bool(ids::ACTIONS).then_some(false),
            request_reviewers: flags.bool(ids::REQUEST_REVIEWERS),
            review_pull_request: flags.bool(ids::PULL_REQUEST_REVIEW),
            move_changes_to_worktree: flags.bool(ids::MOVE_CHANGES_TO_WORKTREE).then_some(false),
            stash_with_message: flags.bool(ids::STASH_LIST).then_some(false),
            // Windows: the installer puts the command line tool on the PATH
            // (GHD has no menu item for it there either)
            install_cli: !cfg!(any(target_os = "android", windows))
                && flags.bool(ids::LINUX_INSTALL_CLI),
            show_remove_repositories: flags.bool(ids::BULK_REMOVE_REPOSITORIES),
            undo_last_commit: flags.bool(ids::UNDO_COMMIT_MENU_ITEM).then_some(false),
            diff_wrap_lines: flags.bool(ids::DIFF_NO_WRAP).then_some(true),
            file_list_tree: flags.bool(ids::FILE_LIST_TREE).then_some(false),
            repository_sidebar: flags.bool(ids::REPOSITORY_SIDEBAR).then_some(false),
            history_file_list: flags
                .bool(ids::HISTORY_FILE_LIST)
                .then_some(corvene_core::HistoryFileList::Beside),
            keymap: KeymapFlags::from_flags(flags),
        }
    }
}

impl MenuLabelsEvent {
    /// GHD `updateMenuLabelsForSelectedRepository` / `updateMenuItemLabels`
    /// (see the module docs for the differences).
    pub fn of(s: &AppState) -> Self {
        let mut extras = MenuExtras::of(&s.flags);
        // `423-undo-commit-menu-item`: while the Undo bar shows
        if extras.undo_last_commit.is_some() {
            extras.undo_last_commit = Some(
                s.selected_state()
                    .is_some_and(|rs| rs.last_commit.is_some()),
            );
        }
        // `797-stash-list`: like Stash All Changes (changes on a branch, no
        // conflicts)
        if extras.stash_with_message.is_some() {
            extras.stash_with_message = Some(s.selected_state().is_some_and(|rs| {
                rs.changed_files() > 0
                    && rs.info.as_ref().and_then(|i| i.current_branch()).is_some()
                    && rs.conflict_state.is_none()
            }));
        }
        // `345-issues` / `346-releases`: a GitHub repository (issues: not
        // an archived one, as Create Issue on GitHub)
        let github = s.selected_repository().and_then(|r| r.non_fork_github());
        if extras.issues.is_some() {
            extras.issues = Some(github.is_some_and(|gh| !gh.archived));
        }
        if extras.releases.is_some() {
            extras.releases = Some(github.is_some());
        }
        // `351-actions`: the repository's own Actions (a fork's too)
        if extras.actions.is_some() {
            extras.actions = Some(s.selected_repository().is_some_and(|r| r.github.is_some()));
        }
        // `1304-diff-no-wrap`: checked while lines wrap
        if extras.diff_wrap_lines.is_some() {
            extras.diff_wrap_lines = Some(s.settings.diff_wrap_lines);
        }
        // `1310-file-list-tree`
        if extras.file_list_tree.is_some() {
            extras.file_list_tree = Some(s.settings.file_list_tree);
        }
        if extras.repository_sidebar.is_some() {
            extras.repository_sidebar = Some(s.settings.repository_sidebar);
        }
        if extras.history_file_list.is_some() {
            extras.history_file_list = Some(s.settings.history_file_list);
        }
        // `1212-bisect`: whether the selected repository bisects
        if extras.bisect.is_some() {
            extras.bisect = Some(corvene_core::bisect::selected_bisect(s).is_some());
        }
        // `283-move-changes-to-worktree`: with changes and another worktree
        if extras.move_changes_to_worktree.is_some() {
            extras.move_changes_to_worktree = Some(s.selected_state().is_some_and(|rs| {
                rs.changed_files() > 0
                    && rs.worktrees.len() > 1
                    && rs.info.as_ref().and_then(|i| i.current_branch()).is_some()
            }));
        }
        let labels = Self {
            selected_shell: Some(s.shell_label()),
            selected_external_editor: Some(s.editor_label()),
            ask_for_confirmation_on_force_push: s.settings.confirm_force_push,
            ask_for_confirmation_on_repository_removal: s.settings.confirm_repository_removal,
            ask_for_confirmation_when_stashing_all_changes: false,
            navigation: (s.navigation.can_go(true), s.navigation.can_go(false)),
            extras,
            ..Self::default()
        };
        // a missing repository is not a `SelectionType.Repository`
        let Some(repository) = s.selected_repository().filter(|r| !r.missing) else {
            return labels;
        };
        let Some(rs) = s.repo_states.get(&repository.id) else {
            return labels;
        };
        let configured_default = s
            .global_git
            .as_ref()
            .map(|g| g.default_branch.as_str())
            .filter(|b| !b.is_empty())
            .unwrap_or("main");
        let branches = rs
            .info
            .as_ref()
            .map(|i| i.branches.as_slice())
            .unwrap_or(&[]);
        let contribution_target = corvene_core::menu_state::contribution_target_default_branch(
            repository,
            branches,
            rs.default_branch.as_deref(),
            configured_default,
        );
        // `1202-update-from-parent-branch`: "Update from <parent>"
        let contribution_target = rs
            .update_parent
            .as_deref()
            .filter(|_| s.flags.bool(ids::UPDATE_FROM_PARENT_BRANCH))
            .or(contribution_target);
        Self {
            contribution_target_default_branch: contribution_target.map(str::to_string),
            // the menu offers a force push whenever one is possible
            is_force_push_for_current_repository: corvene_core::Dispatcher::force_push_state_in(
                s,
                repository.id,
            ) != corvene_core::ForcePushState::NotAvailable,
            is_stashed_changes_visible: rs.showing_stash,
            is_changes_filter_visible: !s.changes_filter_hidden,
            has_current_pull_request: s.current_pull_request(repository.id).is_some(),
            host: s
                .hosted_repository(repository.id)
                .map(|h| (h.kind, h.kind.site_name(&h.repo.endpoint))),
            // `changesState.stashEntry !== null`
            ask_for_confirmation_when_stashing_all_changes: rs.desktop_stash().is_some(),
            remotes: corvene_core::Dispatcher::menu_remotes(s, repository.id),
            push_remote: s
                .flags
                .bool(ids::REMOTE_MANAGER)
                .then(|| corvene_core::Dispatcher::push_remote_name_in(s, repository.id))
                .flatten(),
            editors: corvene_core::Dispatcher::menu_editors(s)
                .into_iter()
                .map(|(label, _)| label)
                .collect(),
            ..labels
        }
    }
}

/// GHD's `Electron.MenuItemConstructorOptions`, the fields Corvene uses: a
/// separator, an item that runs an action, a submenu or a system menu.
#[derive(Default)]
pub struct MenuItemConstructorOptions {
    pub label: Option<String>,
    /// `type: 'separator'`
    pub separator: bool,
    /// `visible` (`None` is visible)
    pub visible: Option<bool>,
    pub submenu: Option<Vec<MenuItemConstructorOptions>>,
    /// `click` / `role`: the action the item dispatches.
    pub action: Option<Box<dyn Action>>,
    /// The `role` the OS knows (undo, copy, …).
    pub os_action: Option<OsAction>,
    /// `role: 'services'` and the like.
    pub system_menu: Option<SystemMenuType>,
    /// `enabled` (`None` is enabled)
    pub enabled: Option<bool>,
    /// `type: 'checkbox'`, `checked`
    pub checked: bool,
}

/// GHD's label for this platform: `__DARWIN__ ? mac : other`. Off macOS
/// labels are sentence case and carry `&` access keys (Electron's classic
/// menu bar underlines the letter after `&`).
const fn l(mac: &'static str, other: &'static str) -> &'static str {
    if cfg!(target_os = "macos") {
        mac
    } else {
        other
    }
}

/// The `__DARWIN__ ? mac : other` choice for built labels.
fn l_owned(mac: String, other: String) -> String {
    if cfg!(target_os = "macos") {
        mac
    } else {
        other
    }
}

fn item(label: impl Into<String>, action: impl Action) -> MenuItemConstructorOptions {
    MenuItemConstructorOptions {
        label: Some(label.into()),
        action: Some(Box::new(action)),
        ..Default::default()
    }
}

fn os_item(label: &str, action: impl Action, os_action: OsAction) -> MenuItemConstructorOptions {
    MenuItemConstructorOptions {
        os_action: Some(os_action),
        ..item(label, action)
    }
}

fn separator() -> MenuItemConstructorOptions {
    MenuItemConstructorOptions {
        separator: true,
        ..Default::default()
    }
}

fn submenu(label: &str, items: Vec<MenuItemConstructorOptions>) -> MenuItemConstructorOptions {
    MenuItemConstructorOptions {
        label: Some(label.to_string()),
        submenu: Some(items),
        ..Default::default()
    }
}

/// GHD `getPushLabel`.
fn push_label(
    is_force_push_for_current_repository: bool,
    ask_for_confirmation: bool,
) -> &'static str {
    if !is_force_push_for_current_repository {
        l("Push", "P&ush")
    } else if ask_for_confirmation {
        l("Force Push…", "Force P&ush…")
    } else {
        l("Force Push", "Force P&ush")
    }
}

/// GHD `getStashedChangesLabel`.
fn stashed_changes_label(is_stashed_changes_visible: bool) -> &'static str {
    if is_stashed_changes_visible {
        l("Hide Stashed Changes", "H&ide stashed changes")
    } else {
        l("Show Stashed Changes", "Sho&w stashed changes")
    }
}

/// GHD `buildDefaultMenuTemplate(labels)`: the top-level menus with their
/// items (see the module docs).
pub fn build_default_menu_template(labels: &MenuLabelsEvent) -> Vec<MenuItemConstructorOptions> {
    let extras = labels.extras;
    let contribution_target_default_branch = corvene_core::notifications::truncate_with_ellipsis(
        labels
            .contribution_target_default_branch
            .as_deref()
            .unwrap_or(l("Default Branch", "default branch")),
        25,
    );
    let remove_repo_label = if labels.ask_for_confirmation_on_repository_removal {
        l("Remove…", "&Remove…")
    } else {
        l("Remove", "&Remove")
    };
    // flags 342-344: the host's name and, on GitLab, "merge request"
    let (host, site) = labels
        .host
        .unwrap_or((corvene_core::HostKind::GitHub, "GitHub"));
    let hosted = |label: &'static str| -> String {
        let label = label.replace("GitHub", site);
        if host.is_merge_request() {
            label
                .replace("Pull Request", "Merge Request")
                .replace("&pull request", "&merge request")
                .replace("pull request", "merge request")
        } else {
            label
        }
    };
    let pull_request_label = if labels.has_current_pull_request {
        hosted(l(
            "View Pull Request on GitHub",
            "View &pull request on GitHub",
        ))
    } else {
        hosted(l("Create Pull Request", "Create &pull request"))
    };

    let mut template = Vec::new();

    if cfg!(target_os = "macos") {
        template.push(submenu(
            "Corvene",
            vec![
                item("About Corvene", About),
                separator(),
                item("Settings…", OpenSettings),
                item("Flags…", OpenFlags),
                item("Install Command Line Tool…", InstallCli),
                separator(),
                MenuItemConstructorOptions {
                    label: Some("Services".to_string()),
                    system_menu: Some(SystemMenuType::Services),
                    ..Default::default()
                },
                separator(),
                item("Hide Corvene", Hide),
                item("Hide Others", HideOthers),
                item("Show All", ShowAll),
                separator(),
                item("Quit Corvene", Quit),
            ],
        ));
    }

    let mut file_items = Vec::new();
    // Corvene (`429-multiple-windows`): the old native GitHub Desktop's
    // File › New Window (desktop/desktop#3606)
    if extras.show_new_window {
        file_items.extend([item(l("New Window", "New &window"), NewWindow), separator()]);
    }
    file_items.extend([
        item(l("New Repository…", "New &repository…"), NewRepository),
        separator(),
        item(
            l("Add Local Repository…", "Add &local repository…"),
            AddLocalRepository,
        ),
        item(
            l("Clone Repository…", "Clo&ne repository…"),
            CloneRepository,
        ),
    ]);
    if extras.show_import {
        file_items.push(item(
            l(
                "Import Repositories from GitHub Desktop…",
                "&Import repositories from GitHub Desktop…",
            ),
            ImportFromGitHubDesktop,
        ));
    }
    if extras.show_remove_repositories {
        file_items.extend([
            separator(),
            item(
                l("Remove Repositories…", "Remo&ve repositories…"),
                RemoveRepositories,
            ),
        ]);
    }
    if !cfg!(target_os = "macos") {
        file_items.extend([
            separator(),
            item("&Options…", OpenSettings),
            item("Fla&gs…", OpenFlags),
        ]);
        if extras.install_cli {
            file_items.push(item("Install command line &tool…", InstallCli));
        }
        // Android applications are left, not quit
        if !cfg!(target_os = "android") {
            file_items.extend([separator(), item("E&xit", Quit)]);
        }
    }
    template.push(submenu(l("File", "&File"), file_items));

    let mut edit_items = vec![
        os_item(l("Undo", "&Undo"), Undo, OsAction::Undo),
        os_item(l("Redo", "&Redo"), Redo, OsAction::Redo),
        separator(),
        os_item(l("Cut", "Cu&t"), Cut, OsAction::Cut),
        os_item(l("Copy", "&Copy"), Copy, OsAction::Copy),
        os_item(l("Paste", "&Paste"), Paste, OsAction::Paste),
        os_item(
            l("Select All", "Select &all"),
            SelectAll,
            OsAction::SelectAll,
        ),
        separator(),
        item(l("Find", "&Find"), Find),
    ];
    if let Some(enabled) = extras.undo_last_commit {
        edit_items.extend([
            separator(),
            MenuItemConstructorOptions {
                enabled: Some(enabled),
                ..item(l("Undo Last Commit", "Undo &last commit"), UndoLastCommit)
            },
        ]);
    }
    template.push(submenu(l("Edit", "&Edit"), edit_items));

    let mut view = vec![
        item(l("Show Changes", "&Changes"), ShowChanges),
        item(l("Show History", "&History"), ShowHistory),
        item(
            l("Show Repository List", "Repository &list"),
            ShowRepositoryList,
        ),
        item(l("Show Branches List", "&Branches list"), ShowBranchesList),
        item(
            l("Show Worktrees List", "Wor&ktrees list"),
            ShowWorktreesList,
        ),
    ];
    // Corvene (`612-navigation-shortcuts`)
    if extras.keymap.navigation_shortcuts {
        view.push(item(
            l("Show Pull Requests List", "&Pull requests list"),
            ShowPullRequestsList,
        ));
    }
    // Corvene (`624-command-palette`)
    if extras.keymap.command_palette {
        view.push(item(
            l("Command Palette…", "&Command palette…"),
            CommandPalette,
        ));
    }
    // Corvene (`801-history-review-mode`)
    if extras.keymap.history_review_mode {
        view.push(item(
            l("Toggle History Review Mode", "Toggle history &review mode"),
            ToggleHistoryReviewMode,
        ));
    }
    // Corvene (`427-back-forward-navigation`)
    if extras.keymap.back_forward {
        let (back, forward) = labels.navigation;
        view.extend([
            separator(),
            MenuItemConstructorOptions {
                enabled: Some(back),
                ..item(l("Back", "B&ack"), NavigateBack)
            },
            MenuItemConstructorOptions {
                enabled: Some(forward),
                ..item(l("Forward", "F&orward"), NavigateForward)
            },
        ]);
    }
    // Corvene (`1304-diff-no-wrap`)
    if let Some(wrap) = extras.diff_wrap_lines {
        view.extend([
            separator(),
            MenuItemConstructorOptions {
                checked: wrap,
                ..item(l("Wrap Diff Lines", "Wrap &diff lines"), ToggleDiffWordWrap)
            },
        ]);
    }
    let filter_verb = if labels.is_changes_filter_visible {
        "Hide"
    } else {
        "Show"
    };
    view.extend([
        separator(),
        item(l("Go to Summary", "Go to &Summary"), GoToSummary),
        item(
            stashed_changes_label(labels.is_stashed_changes_visible),
            ToggleStashedChanges,
        ),
        // GHD's non-macOS label really reads "… Toggle Changes Filter"
        item(
            l_owned(
                format!("{filter_verb} Changes Filter"),
                format!("{filter_verb} Toggle Chan&ges Filter"),
            ),
            ToggleChangesFilter,
        ),
    ]);
    // Corvene (`1310-file-list-tree`)
    if let Some(tree) = extras.file_list_tree {
        view.push(item(
            if tree {
                l("Show Changes as List", "Show chang&es as list")
            } else {
                l("Show Changes as Tree", "Show changes as &tree")
            },
            ToggleFileListTree,
        ));
    }
    // Corvene (`431-repository-sidebar`)
    if let Some(showing) = extras.repository_sidebar {
        view.push(item(
            if showing {
                l("Hide Repository Sidebar", "Hide repository s&idebar")
            } else {
                l("Show Repository Sidebar", "Show repository s&idebar")
            },
            ToggleRepositorySidebar,
        ));
    }
    // Corvene (`432-history-file-list`)
    if let Some(place) = extras.history_file_list {
        use corvene_core::HistoryFileList as List;
        let choice = |label: &str, action: Box<dyn Action>, at: List| MenuItemConstructorOptions {
            label: Some(label.to_string()),
            action: Some(action),
            checked: place == at,
            ..Default::default()
        };
        view.push(submenu(
            l("History File List", "History file &list"),
            vec![
                choice(
                    l("Beside the Diff", "&Beside the diff"),
                    Box::new(HistoryFileListBeside),
                    List::Beside,
                ),
                choice(
                    l("Above the Diff", "&Above the diff"),
                    Box::new(HistoryFileListAbove),
                    List::Above,
                ),
                choice(
                    l("Hidden", "&Hidden"),
                    Box::new(HistoryFileListHidden),
                    List::Hidden,
                ),
            ],
        ));
    }
    // Corvene's macOS menu sets Toggle Full Screen apart; GHD has no
    // separator before it (kept on macOS, GHD's layout elsewhere)
    if cfg!(target_os = "macos") {
        view.push(separator());
    }
    view.extend([
        item(
            l("Toggle Full Screen", "Toggle &full screen"),
            ToggleFullScreen,
        ),
        separator(),
        item(l("Reset Zoom", "Reset zoom"), ResetZoom),
        item(l("Zoom In", "Zoom in"), ZoomIn),
        item(l("Zoom Out", "Zoom out"), ZoomOut),
        item(
            l("Expand Active Resizable", "Expand active resizable"),
            ExpandActiveResizable,
        ),
        item(
            l("Contract Active Resizable", "Contract active resizable"),
            ContractActiveResizable,
        ),
    ]);
    // Corvene (`430-repository-tabs`): off macOS the Window menu does not
    // exist, so the tab items go here
    if extras.tabs && !cfg!(target_os = "macos") {
        view.extend([
            separator(),
            item("Show &next tab", NextTab),
            item("Show previo&us tab", PreviousTab),
        ]);
    }
    template.push(submenu(l("View", "&View"), view));

    let mut repository = vec![
        item(
            push_label(
                labels.is_force_push_for_current_repository,
                labels.ask_for_confirmation_on_force_push,
            ),
            Push,
        ),
        item(l("Pull", "Pu&ll"), Pull),
        item(l("Fetch", "&Fetch"), Fetch),
    ];
    if extras.fetch_tags {
        repository.push(item(l("Fetch All Tags", "Fetch all ta&gs"), FetchAllTags));
    }
    if extras.fetch_all {
        repository.push(item(
            l("Fetch All Repositories", "Fetch &all repositories"),
            FetchAllRepositories,
        ));
    }
    if extras.pull_all {
        repository.push(item(
            l("Pull All Repositories", "Pull all r&epositories"),
            PullAllRepositories,
        ));
    }
    repository.push(item(remove_repo_label, RemoveRepository));
    // Android: a repository in Corvene's own storage can move to shared
    // storage, where Termux reaches it (`corvene_core::shared_storage`)
    if cfg!(target_os = "android") {
        repository.push(item("&Move to shared storage…", MoveToSharedStorage));
    }
    repository.extend([
        separator(),
        item(hosted(l("View on GitHub", "&View on GitHub")), ViewOnGitHub),
    ]);
    if extras.show_view_upstream {
        repository.push(item(
            l("View Upstream on GitHub", "View upstream on GitHub"),
            ViewUpstreamOnGitHub,
        ));
    }
    let shell = labels.selected_shell.as_deref();
    let editor = labels.selected_external_editor.as_deref();
    repository.extend([
        item(
            l_owned(
                format!("Open in {}", shell.unwrap_or("Shell")),
                format!("O&pen in {}", shell.unwrap_or("shell")),
            ),
            OpenInShell,
        ),
        item(
            if cfg!(windows) {
                "Show in E&xplorer"
            } else {
                l("Show in Finder", "Show in your File Manager")
            },
            ShowInFinder,
        ),
        item(
            l_owned(
                format!("Open in {}", editor.unwrap_or("External Editor")),
                format!("&Open in {}", editor.unwrap_or("external editor")),
            ),
            OpenInEditor,
        ),
    ]);
    // Corvene (`524-open-repository-with-editor`)
    if !labels.editors.is_empty() {
        repository.push(submenu(
            l("Open in Editor", "Open i&n editor"),
            remote_items(&labels.editors, &OPEN_IN_CHOSEN_EDITOR),
        ));
    }
    repository.extend([
        item(l("Open With…", "Open &with…"), OpenWith),
        separator(),
        item(
            hosted(l("Create Issue on GitHub", "Create &issue on GitHub")),
            CreateIssue,
        ),
        separator(),
        item(l("New Worktree…", "New work&tree…"), NewWorktree),
        separator(),
    ]);
    // Corvene (`1105-clean-untracked-files`, `1106-apply-patch`)
    if extras.clean_untracked || extras.apply_patch {
        if extras.clean_untracked {
            repository.push(item(
                l("Clean Untracked Files…", "Clean untrac&ked files…"),
                CleanUntrackedFiles,
            ));
        }
        if extras.apply_patch {
            repository.push(submenu(
                l("Apply Patch", "Apply patc&h"),
                vec![
                    item(l("From File…", "From &file…"), ApplyPatchFromFile),
                    item(
                        l("From Clipboard", "From &clipboard"),
                        ApplyPatchFromClipboard,
                    ),
                ],
            ));
        }
        repository.push(separator());
    }
    // Corvene (`1111-submodules`, `1112-sparse-checkout`)
    if extras.submodules || extras.sparse_checkout {
        if extras.submodules {
            repository.push(item(l("Submodules…", "Sub&modules…"), ShowSubmodules));
        }
        if extras.sparse_checkout {
            repository.push(item(
                l("Sparse Checkout…", "Sparse checkout…"),
                ShowSparseCheckout,
            ));
        }
        repository.push(separator());
    }
    // Corvene (`345-issues`)
    if let Some(enabled) = extras.issues {
        repository.extend([
            MenuItemConstructorOptions {
                enabled: Some(enabled),
                ..item(l("New Issue…", "New issue…"), NewIssue)
            },
            MenuItemConstructorOptions {
                enabled: Some(enabled),
                ..item(l("Issues…", "Issues…"), ShowIssues)
            },
            separator(),
        ]);
    }
    // Corvene (`346-releases`)
    if let Some(enabled) = extras.releases {
        repository.extend([
            MenuItemConstructorOptions {
                enabled: Some(enabled),
                ..item(l("Create Release…", "Create release…"), CreateRelease)
            },
            MenuItemConstructorOptions {
                enabled: Some(enabled),
                ..item(l("Releases…", "Releases…"), ShowReleases)
            },
            separator(),
        ]);
    }
    // Corvene (`351-actions`)
    if let Some(enabled) = extras.actions {
        repository.extend([
            MenuItemConstructorOptions {
                enabled: Some(enabled),
                ..item(l("Actions…", "Actions…"), ShowActions)
            },
            separator(),
        ]);
    }
    // Corvene (`1216-recent-activity`, `1110-repository-insights`)
    if extras.recent_activity {
        repository.push(item(
            l("Recent Activity…", "Recent activit&y…"),
            ShowRecentActivity,
        ));
    }
    if extras.insights {
        repository.push(item(l("Insights…", "Insights…"), ShowInsights));
    }
    if extras.recent_activity || extras.insights {
        repository.push(separator());
    }
    // Corvene (`1212-bisect`)
    if let Some(bisecting) = extras.bisect {
        repository.extend([
            MenuItemConstructorOptions {
                enabled: Some(!bisecting),
                ..item(l("Start Bisect", "Start &bisect"), StartBisect)
            },
            MenuItemConstructorOptions {
                enabled: Some(bisecting),
                ..item(l("Stop Bisecting", "Stop bise&cting"), StopBisect)
            },
            separator(),
        ]);
    }
    if extras.show_add_license {
        repository.push(item(l("Add License…", "A&dd license…"), AddLicense));
    }
    repository.push(item(
        l("Repository Settings…", "Repository &settings…"),
        RepositorySettings,
    ));
    template.push(submenu(l("Repository", "&Repository"), repository));

    let stash_all_label = if labels.ask_for_confirmation_when_stashing_all_changes {
        l("Stash All Changes…", "&Stash all changes…")
    } else {
        l("Stash All Changes", "&Stash all changes")
    };
    let mut branch = vec![
        item(l("New Branch…", "New &branch…"), NewBranch),
        item(l("Rename…", "&Rename…"), RenameBranch),
        item(l("Delete…", "&Delete…"), DeleteBranch),
        separator(),
        item(
            l("Discard All Changes…", "Discard all changes…"),
            DiscardAllChanges,
        ),
        item(stash_all_label, StashAllChanges),
    ];
    if let Some(enabled) = extras.stash_with_message {
        branch.push(MenuItemConstructorOptions {
            enabled: Some(enabled),
            ..item(
                l(
                    "Stash All Changes with Message…",
                    "Stash all changes &with message…",
                ),
                StashAllChangesWithMessage,
            )
        });
    }
    if let Some(enabled) = extras.move_changes_to_worktree {
        branch.push(MenuItemConstructorOptions {
            enabled: Some(enabled),
            ..item(
                l("Move Changes to Worktree…", "Move changes to wor&ktree…"),
                MoveChangesToWorktree,
            )
        });
    }
    branch.extend([
        separator(),
        item(
            l_owned(
                format!("Update from {contribution_target_default_branch}"),
                format!("&Update from {contribution_target_default_branch}"),
            ),
            UpdateFromDefaultBranch,
        ),
        item(
            l("Compare to Branch", "&Compare to branch"),
            CompareToBranch,
        ),
    ]);
    // Corvene (`1218-compare-refs`)
    if extras.compare_refs {
        branch.push(item(l("Compare…", "Comp&are…"), CompareRefs));
    }
    branch.extend([
        item(
            l("Merge into Current Branch…", "&Merge into current branch…"),
            MergeIntoCurrentBranch,
        ),
        item(
            l(
                "Squash and Merge into Current Branch…",
                "Squas&h and merge into current branch…",
            ),
            SquashAndMergeIntoCurrentBranch,
        ),
        item(
            l("Rebase Current Branch…", "R&ebase current branch…"),
            RebaseCurrentBranch,
        ),
    ]);
    // Corvene (`1219-tag-manager`)
    if extras.tags {
        branch.push(item(l("Tags…", "Tags…"), ShowTags));
    }
    // Corvene (`1223-checkout-from-fork`)
    if extras.checkout_from_fork {
        branch.push(item(
            l("Check Out from Fork…", "Check &out from fork…"),
            CheckoutFromFork,
        ));
    }
    // Corvene (`1210-push-to-other-remote`)
    if !labels.remotes.is_empty() {
        branch.extend([
            submenu(
                l("Push To", "Push &to"),
                remote_items(&labels.remotes, &PUSH_TO_REMOTE)
                    .into_iter()
                    .zip(&labels.remotes)
                    .map(|(item, name)| MenuItemConstructorOptions {
                        // `1109-remote-manager`: where Push goes
                        checked: labels.push_remote.as_ref() == Some(name),
                        ..item
                    })
                    .collect(),
            ),
            submenu(
                l("Fetch From", "Fetch &from"),
                remote_items(&labels.remotes, &FETCH_FROM_REMOTE),
            ),
        ]);
    }
    branch.extend([
        separator(),
        item(
            hosted(l("Compare on GitHub", "Compare on &GitHub")),
            CompareOnGitHub,
        ),
        item(
            hosted(l("View Branch on GitHub", "View branch on GitHub")),
            ViewBranchOnGitHub,
        ),
    ]);
    // same as Toggle Full Screen: a macOS-only separator
    if cfg!(target_os = "macos") {
        branch.push(separator());
    }
    branch.extend([
        item(
            hosted(l("Preview Pull Request", "Preview pull request")),
            PreviewPullRequest,
        ),
        item(pull_request_label, CreatePullRequest),
    ]);
    if extras.request_reviewers && labels.has_current_pull_request {
        branch.push(item(
            l("Request Reviewers…", "Request re&viewers…"),
            RequestReviewers,
        ));
    }
    // Corvene (`348-pull-request-review`)
    if extras.review_pull_request && labels.has_current_pull_request {
        branch.push(item(
            l("Review Pull Request…", "Rev&iew pull request…"),
            ReviewPullRequest,
        ));
    }
    template.push(submenu(l("Branch", "&Branch"), branch));

    if cfg!(target_os = "macos") {
        template.push(submenu(
            "Window",
            window_items(extras.show_main_window, extras.tabs),
        ));
    }
    template.push(submenu(
        l("Help", "&Help"),
        help_items(extras.show_release_notes),
    ));
    template
}

/// `1210-push-to-other-remote`: the actions of Branch › Push To ▸'s items,
/// by index into [`MenuLabelsEvent::remotes`].
const PUSH_TO_REMOTE: [fn() -> Box<dyn Action>; 8] = [
    || Box::new(PushToRemote0),
    || Box::new(PushToRemote1),
    || Box::new(PushToRemote2),
    || Box::new(PushToRemote3),
    || Box::new(PushToRemote4),
    || Box::new(PushToRemote5),
    || Box::new(PushToRemote6),
    || Box::new(PushToRemote7),
];

/// Open in Editor ▸'s, as [`PUSH_TO_REMOTE`]
/// (`524-open-repository-with-editor`).
const OPEN_IN_CHOSEN_EDITOR: [fn() -> Box<dyn Action>; 8] = [
    || Box::new(OpenInChosenEditor0),
    || Box::new(OpenInChosenEditor1),
    || Box::new(OpenInChosenEditor2),
    || Box::new(OpenInChosenEditor3),
    || Box::new(OpenInChosenEditor4),
    || Box::new(OpenInChosenEditor5),
    || Box::new(OpenInChosenEditor6),
    || Box::new(OpenInChosenEditor7),
];

/// Fetch From ▸'s, as [`PUSH_TO_REMOTE`].
const FETCH_FROM_REMOTE: [fn() -> Box<dyn Action>; 8] = [
    || Box::new(FetchFromRemote0),
    || Box::new(FetchFromRemote1),
    || Box::new(FetchFromRemote2),
    || Box::new(FetchFromRemote3),
    || Box::new(FetchFromRemote4),
    || Box::new(FetchFromRemote5),
    || Box::new(FetchFromRemote6),
    || Box::new(FetchFromRemote7),
];

/// One item per remote (or editor) name (a `&` in a name is doubled for
/// the Windows and Linux access keys).
fn remote_items(
    remotes: &[String],
    actions: &[fn() -> Box<dyn Action>; 8],
) -> Vec<MenuItemConstructorOptions> {
    remotes
        .iter()
        .zip(actions)
        .map(|(name, action)| MenuItemConstructorOptions {
            label: Some(if cfg!(target_os = "macos") {
                name.clone()
            } else {
                name.replace('&', "&&")
            }),
            action: Some(action()),
            ..Default::default()
        })
        .collect()
}

/// Window menu; flag `405-window-menu-main-window` appends "Corvene", which
/// shows the main window again after ⌘W or the red close button.
fn window_items(show_main_window: bool, tabs: bool) -> Vec<MenuItemConstructorOptions> {
    let mut items = vec![
        item("Minimize", Minimize),
        item("Zoom", Zoom),
        item("Close Window", CloseWindow),
    ];
    // Corvene (`430-repository-tabs`): Safari's tab items
    if tabs {
        items.extend([
            separator(),
            item("Show Next Tab", NextTab),
            item("Show Previous Tab", PreviousTab),
        ]);
    }
    items.extend([separator(), item("Bring All to Front", BringAllToFront)]);
    if show_main_window {
        items.extend([separator(), item("Corvene", ShowMainWindow)]);
    }
    items
}

/// Help menu; debug builds append GHD's test items (`buildTestMenu`, only
/// "Show notification" so far, as "Show Test Notifications"). Off macOS
/// About closes the menu, after a separator (GHD's non-darwin `&Help`).
fn help_items(show_release_notes: bool) -> Vec<MenuItemConstructorOptions> {
    let mut items = vec![
        item(l("Report Issue…", "Report issue…"), ReportIssue),
        item(
            l("Contact GitHub Support…", "&Contact GitHub support…"),
            ContactSupport,
        ),
        item("Show User Guides", ShowUserGuides),
        item(
            l("Show Keyboard Shortcuts", "Show keyboard shortcuts"),
            ShowKeyboardShortcuts,
        ),
        item(
            if cfg!(windows) {
                "S&how logs in Explorer"
            } else {
                l("Show Logs in Finder", "S&how logs in your File Manager")
            },
            ShowLogs,
        ),
    ];
    if show_release_notes {
        items.extend([
            separator(),
            item(
                l("Show Release Notes", "Show release &notes"),
                ShowReleaseNotes,
            ),
        ]);
    }
    if cfg!(debug_assertions) {
        items.extend([
            separator(),
            item(
                l("Show Test Notifications", "Show test notifications"),
                ShowTestNotifications,
            ),
        ]);
    }
    if !cfg!(target_os = "macos") {
        items.extend([separator(), item("&About Corvene", About)]);
    }
    items
}

/// GHD `buildDefaultMenu` (`Menu.buildFromTemplate`): the template as GPUI
/// menus, invisible items left out.
pub fn build_default_menu(labels: &MenuLabelsEvent) -> Vec<Menu> {
    build_default_menu_template(labels)
        .into_iter()
        .filter(|top| top.visible != Some(false))
        .map(|top| {
            Menu::new(top.label.unwrap_or_default())
                .items(gpui_items(top.submenu.unwrap_or_default()))
        })
        .collect()
}

fn gpui_items(items: Vec<MenuItemConstructorOptions>) -> Vec<MenuItem> {
    items
        .into_iter()
        .filter(|i| i.visible != Some(false))
        .filter_map(|i| {
            let label = i.label.unwrap_or_default();
            if i.separator {
                Some(MenuItem::separator())
            } else if let Some(kind) = i.system_menu {
                Some(MenuItem::os_submenu(label, kind))
            } else if let Some(items) = i.submenu {
                Some(MenuItem::submenu(Menu::new(label).items(gpui_items(items))))
            } else {
                i.action.map(|action| MenuItem::Action {
                    name: label.into(),
                    action,
                    os_action: i.os_action,
                    checked: i.checked,
                    disabled: i.enabled == Some(false),
                })
            }
        })
        .collect()
}

/// GHD's menu id for one of Corvene's menu actions (`menu-update.ts` works
/// on ids).
pub(crate) fn menu_id(action: &dyn Action) -> Option<MenuId> {
    let name = action.name();
    let short = name.rsplit("::").next().unwrap_or(name);
    Some(match short {
        "RenameBranch" => MenuId::RenameBranch,
        "DeleteBranch" => MenuId::DeleteBranch,
        "DiscardAllChanges" => MenuId::DiscardAllChanges,
        "StashAllChanges" => MenuId::StashAllChanges,
        "OpenSettings" => MenuId::Preferences,
        "UpdateFromDefaultBranch" => MenuId::UpdateBranchWithContributionTargetBranch,
        "CompareToBranch" => MenuId::CompareToBranch,
        "MergeIntoCurrentBranch" => MenuId::MergeBranch,
        "RebaseCurrentBranch" => MenuId::RebaseBranch,
        "ViewOnGitHub" => MenuId::ViewRepositoryOnGithub,
        "CompareOnGitHub" => MenuId::CompareOnGithub,
        "ViewBranchOnGitHub" => MenuId::BranchOnGithub,
        "OpenInShell" => MenuId::OpenInShell,
        "Push" => MenuId::Push,
        "Pull" => MenuId::Pull,
        "Fetch" => MenuId::Fetch,
        "GoToSummary" => MenuId::GoToCommitMessage,
        "NewBranch" => MenuId::CreateBranch,
        "ShowChanges" => MenuId::ShowChanges,
        "ShowHistory" => MenuId::ShowHistory,
        "ShowRepositoryList" => MenuId::ShowRepositoryList,
        "ShowBranchesList" => MenuId::ShowBranchesList,
        "ShowInFinder" => MenuId::OpenWorkingDirectory,
        "RepositorySettings" => MenuId::ShowRepositorySettings,
        "OpenInEditor" => MenuId::OpenExternalEditor,
        "OpenWith" => MenuId::OpenWithExternalEditor,
        "RemoveRepository" => MenuId::RemoveRepository,
        "NewRepository" => MenuId::NewRepository,
        "AddLocalRepository" => MenuId::AddLocalRepository,
        "CloneRepository" => MenuId::CloneRepository,
        "About" => MenuId::About,
        "CreatePullRequest" => MenuId::CreatePullRequest,
        "PreviewPullRequest" => MenuId::PreviewPullRequest,
        "SquashAndMergeIntoCurrentBranch" => MenuId::SquashAndMergeBranch,
        "ToggleStashedChanges" => MenuId::ToggleStashedChanges,
        "NewWorktree" => MenuId::CreateWorktree,
        "ShowWorktreesList" => MenuId::ShowWorktreesList,
        "CreateIssue" => MenuId::CreateIssueInRepositoryOnGithub,
        "ToggleChangesFilter" => MenuId::ToggleChangesFilter,
        "ExpandActiveResizable" => MenuId::IncreaseActiveResizableWidth,
        "ContractActiveResizable" => MenuId::DecreaseActiveResizableWidth,
        _ => return None,
    })
}

/// A menu item the command palette can run (`624-command-palette`).
pub struct PaletteCommand {
    pub label: String,
    /// Its top-level menu, with any submenus: `Branch › Compare`.
    pub menu: String,
    pub action: Box<dyn Action>,
}

/// A label without its `&` access key.
fn plain_label(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    let mut chars = label.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '&' {
            out.push(c);
        } else if chars.peek() == Some(&'&') {
            chars.next();
            out.push('&');
        }
    }
    out
}

/// Every visible, enabled item of the menu template as it stands, without
/// the OS's own (Cut, Copy, …, which act on the focused text box) and the
/// palette's own item.
pub fn palette_commands(s: &AppState) -> Vec<PaletteCommand> {
    fn walk(
        items: Vec<MenuItemConstructorOptions>,
        path: &str,
        s: &AppState,
        out: &mut Vec<PaletteCommand>,
    ) {
        for item in items {
            if item.visible == Some(false) || item.enabled == Some(false) || item.separator {
                continue;
            }
            let label = plain_label(item.label.as_deref().unwrap_or(""));
            if let Some(children) = item.submenu {
                walk(children, &format!("{path} › {label}"), s, out);
                continue;
            }
            let Some(action) = item.action else {
                continue;
            };
            if item.os_action.is_some() || item.system_menu.is_some() {
                continue;
            }
            if action.partial_eq(&CommandPalette) {
                continue;
            }
            let on = menu_id(&*action).is_none_or(|id| corvene_core::menu_state::is_enabled(s, id));
            if on {
                out.push(PaletteCommand {
                    label,
                    menu: path.to_string(),
                    action,
                });
            }
        }
    }
    let mut out = Vec::new();
    for top in build_default_menu_template(&MenuLabelsEvent::of(s)) {
        if top.visible == Some(false) {
            continue;
        }
        let name = plain_label(top.label.as_deref().unwrap_or(""));
        walk(top.submenu.unwrap_or_default(), &name, s, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The access key after an unescaped `&`, lower-cased.
    fn access_key(label: &str) -> Option<char> {
        let mut chars = label.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '&' {
                match chars.next() {
                    Some('&') => {}
                    Some(key) => return Some(key.to_ascii_lowercase()),
                    None => return None,
                }
            }
        }
        None
    }

    fn duplicates(items: &[MenuItemConstructorOptions], path: &str, out: &mut Vec<String>) {
        let mut seen: Vec<(char, &str)> = Vec::new();
        for item in items
            .iter()
            .filter(|i| !i.separator && i.visible != Some(false))
        {
            let label = item.label.as_deref().unwrap_or("");
            if let Some(key) = access_key(label) {
                match seen.iter().find(|(k, _)| *k == key) {
                    Some((_, first)) => out.push(format!("{path}: {first} / {label}")),
                    None => seen.push((key, label)),
                }
            }
            if let Some(submenu) = &item.submenu {
                duplicates(submenu, &format!("{path} > {label}"), out);
            }
        }
    }

    /// Every Corvene addition on, with each label variant: no submenu
    /// repeats an access key (the GHD-preset case is GHD's own test).
    #[test]
    fn corvene_additions_keep_access_keys_unique() {
        for bits in 0..(1u32 << 5) {
            let labels = MenuLabelsEvent {
                selected_shell: Some("Terminal".into()),
                selected_external_editor: Some("Visual Studio Code".into()),
                is_stashed_changes_visible: bits & 1 != 0,
                has_current_pull_request: bits & 2 != 0,
                is_force_push_for_current_repository: bits & 4 != 0,
                ask_for_confirmation_on_repository_removal: bits & 8 != 0,
                is_changes_filter_visible: bits & 16 != 0,
                remotes: vec!["fork".into(), "origin".into()],
                editors: vec!["Zed".into(), "Vim".into()],
                extras: MenuExtras {
                    show_release_notes: true,
                    show_import: true,
                    show_view_upstream: true,
                    show_main_window: true,
                    show_new_window: true,
                    tabs: true,
                    show_add_license: true,
                    fetch_all: true,
                    pull_all: true,
                    fetch_tags: true,
                    bisect: Some(bits & 1 != 0),
                    recent_activity: true,
                    insights: true,
                    compare_refs: true,
                    tags: true,
                    checkout_from_fork: true,
                    clean_untracked: true,
                    apply_patch: true,
                    submodules: true,
                    sparse_checkout: true,
                    issues: Some(true),
                    releases: Some(true),
                    actions: Some(true),
                    request_reviewers: true,
                    review_pull_request: true,
                    move_changes_to_worktree: Some(true),
                    stash_with_message: Some(true),
                    install_cli: true,
                    show_remove_repositories: true,
                    undo_last_commit: Some(true),
                    diff_wrap_lines: Some(bits & 2 != 0),
                    file_list_tree: Some(bits & 2 != 0),
                    repository_sidebar: None,
                    history_file_list: None,
                    keymap: KeymapFlags {
                        navigation_shortcuts: true,
                        history_review_mode: true,
                        back_forward: true,
                        multiple_windows: true,
                        repository_tabs: true,
                        ..KeymapFlags::default()
                    },
                },
                ..MenuLabelsEvent::default()
            };
            let mut out = Vec::new();
            duplicates(&build_default_menu_template(&labels), "root", &mut out);
            assert_eq!(out, Vec::<String>::new());
        }
    }

    /// `1304-diff-no-wrap`: View › Wrap Diff Lines is checked while lines
    /// wrap, and absent without the flag.
    #[test]
    fn wrap_diff_lines_is_a_checked_view_item() {
        let wrap_item = |extras: MenuExtras| {
            let labels = MenuLabelsEvent {
                extras,
                ..MenuLabelsEvent::default()
            };
            build_default_menu_template(&labels)
                .iter()
                .filter_map(|m| m.submenu.as_ref())
                .flatten()
                .find(|i| i.label.as_deref() == Some(l("Wrap Diff Lines", "Wrap &diff lines")))
                .map(|i| i.checked)
        };
        assert_eq!(wrap_item(MenuExtras::default()), None);
        for wrap in [true, false] {
            let extras = MenuExtras {
                diff_wrap_lines: Some(wrap),
                ..MenuExtras::default()
            };
            assert_eq!(wrap_item(extras), Some(wrap));
        }
    }

    /// `1109-remote-manager`: Push To ▸ checks the remote Push goes to.
    #[test]
    fn push_to_checks_the_push_remote() {
        let labels = MenuLabelsEvent {
            remotes: vec!["fork".into(), "origin".into()],
            push_remote: Some("fork".into()),
            ..MenuLabelsEvent::default()
        };
        let template = build_default_menu_template(&labels);
        let push_to = template
            .iter()
            .filter_map(|m| m.submenu.as_ref())
            .flatten()
            .find(|i| i.label.as_deref() == Some(l("Push To", "Push &to")))
            .and_then(|i| i.submenu.as_ref())
            .expect("Push To");
        let checked: Vec<(Option<&str>, bool)> = push_to
            .iter()
            .map(|i| (i.label.as_deref(), i.checked))
            .collect();
        assert_eq!(checked, [(Some("fork"), true), (Some("origin"), false)]);
    }

    #[test]
    fn labels_follow_the_repository_state() {
        let labels = MenuLabelsEvent {
            is_force_push_for_current_repository: true,
            ask_for_confirmation_on_force_push: true,
            has_current_pull_request: true,
            is_stashed_changes_visible: true,
            contribution_target_default_branch: Some("a-very-long-default-branch-name".into()),
            ..MenuLabelsEvent::default()
        };
        let template = build_default_menu_template(&labels);
        let labels_of = |menu: &str| -> Vec<String> {
            template
                .iter()
                .find(|m| m.label.as_deref() == Some(menu))
                .and_then(|m| m.submenu.as_ref())
                .map(|items| items.iter().filter_map(|i| i.label.clone()).collect())
                .unwrap_or_default()
        };
        let repository = labels_of(l("Repository", "&Repository"));
        assert_eq!(repository[0], l("Force Push…", "Force P&ush…"));
        let branch = labels_of(l("Branch", "&Branch"));
        assert!(
            branch.contains(
                &l(
                    "View Pull Request on GitHub",
                    "View &pull request on GitHub"
                )
                .to_string()
            )
        );
        assert!(
            branch
                .iter()
                .any(|b| b.ends_with("from a-very-long-default-branc…"))
        );
        let view = labels_of(l("View", "&View"));
        assert!(view.contains(&l("Hide Stashed Changes", "H&ide stashed changes").to_string()));
    }
}
