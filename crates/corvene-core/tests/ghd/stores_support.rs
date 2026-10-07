//! Helpers shared by `corvene-core`'s ports of GitHub Desktop's store
//! tests (`unit/git-store-cache-test.ts`,
//! `unit/repository-state-cache-test.ts`).
//!
//! GitHub Desktop's per-repository caches (`GitStoreCache`,
//! `RepositoryStateCache`) are Corvene's `AppState::repo_states`, reached
//! through `AppState::repo_state_mut`. `AppState` normally comes from
//! `Dispatcher::init`, which needs a gpui `App`; [`app_state`] builds the
//! same value by hand: empty lists and maps, no git, the default settings
//! and the flags of the `github-desktop` preset, and a store in a
//! temporary directory.

use std::collections::{HashMap, HashSet};
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use corvene_core::AppState;
use corvene_core::flags::{EnvFlags, github_desktop_flags, github_desktop_overrides};
use corvene_store::Store;
use tempfile::TempDir;

/// An [`AppState`] and the temporary directory its store lives in.
pub struct TestAppState {
    state: AppState,
    _dir: TempDir,
}

impl Deref for TestAppState {
    type Target = AppState;

    fn deref(&self) -> &AppState {
        &self.state
    }
}

impl DerefMut for TestAppState {
    fn deref_mut(&mut self) -> &mut AppState {
        &mut self.state
    }
}

/// The `AppState` `Dispatcher::init` makes for an empty store when git is
/// not looked up, with the `github-desktop` flag preset.
pub fn app_state() -> TestAppState {
    let dir = corvene_test_support::create_temp_directory();
    let store = Store::open_in(dir.path()).expect("open the store");
    let flags = github_desktop_flags();
    let sign_in_accounts = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let state = AppState {
        store: Arc::new(store),
        settings: Default::default(),
        flag_overrides: github_desktop_overrides(),
        flags_env: EnvFlags::default(),
        flags: flags.clone(),
        flags_at_launch: flags,
        git: None,
        git_error: None,
        git_redetecting: false,
        repositories: Vec::new(),
        recent: Vec::new(),
        recent_worktrees: Vec::new(),
        workspaces: vec![corvene_core::WorkspaceState::new(
            corvene_core::WorkspaceId::FIRST,
            None,
        )],
        current: corvene_core::WorkspaceId::FIRST,
        focused: corvene_core::WorkspaceId::FIRST,
        next_workspace_id: 2,
        keymap_overrides: Default::default(),
        keymap_load_errors: Vec::new(),
        settings_overlay: Default::default(),
        settings_file_flags: Default::default(),
        repo_states: HashMap::new(),
        accounts: Vec::new(),
        cloning: Default::default(),
        ahead_behind: Default::default(),
        pruner_generations: HashMap::new(),
        shared_storage_move: None,
        pending_aliases: Vec::new(),
        pending_accounts: Vec::new(),
        account_probes: Default::default(),
        sign_in_store: corvene_core::sign_in::SignInStore::new(sign_in_accounts.clone()),
        sign_in_accounts,
        authentication: None,
        adding_account: false,
        extra_oauth_scopes: Vec::new(),
        ssh_key: Default::default(),
        watchers: HashMap::new(),
        watcher_generation: 0,
        indicators: HashMap::new(),
        generic_logins: HashMap::new(),
        proxy: Default::default(),
        enterprise_oauth_apps: HashMap::new(),
        avatars: HashMap::new(),
        api_repositories: HashMap::new(),
        api_repositories_loading: HashSet::new(),
        issues: HashMap::new(),
        mentionables: HashMap::new(),
        pull_requests: HashMap::new(),
        tutorial_announced: false,
        tutorial_step_override: None,
        commit_statuses: Default::default(),
        job_logs: Default::default(),
        actions: None,
        actions_watch: Default::default(),
        menu_bar_statuses: Default::default(),
        repo_rulesets: HashMap::new(),
        editors: Vec::new(),
        shells: Vec::new(),
        app_icons: HashMap::new(),
        global_git: None,
        repo_settings: None,
        pending_open_in_desktop: None,
        update: Default::default(),
        packs: Default::default(),
        extensions: Default::default(),
        alive: Default::default(),
        commit_drafts: HashMap::new(),
        commit_drafts_nonce: 0,
        excluded_files: HashMap::new(),
        excluded_files_restored: HashSet::new(),
        collapsed_folders: HashMap::new(),
        changelists: HashMap::new(),
        file_icon_theme: None,
        hosts: Default::default(),
        markdown_images: Default::default(),
    };
    TestAppState { state, _dir: dir }
}
