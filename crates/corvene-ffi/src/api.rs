//! The Kotlin-visible interface.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use corvene_core::Dispatcher;
use corvene_core::host::{Host, LoopHandle, spawn_loop};
use corvene_core::persistence::{StoreExt, UncommittedChangesStrategy};

use crate::runtime::Services;
use crate::vm::{
    BannerVm, BranchesVm, ChangesVm, CloneableRepositoriesVm, CommitDetailVm, ConflictsVm,
    DesignStyleVm, DiffHeaderVm, DiffRowVm, FlagsVm, GlobalGitConfigVm, HistoryVm, McoVm, PopupVm,
    PullRequestsVm, RepoListVm, RepositorySettingsVm, ResolutionVm, SessionVm, SettingsVm, ThemeVm,
    banner, branches, changes, cloneable_repositories, commit_detail, commit_diff_rows, conflicts,
    diff_header, diff_rows, flags, global_git_config, history, mco, popup, pull_requests,
    repo_list, repository_settings, session, settings,
};

/// What the engine asks of the Android side. Called on the engine's
/// threads: post to the main looper before touching views.
#[uniffi::export(with_foreign)]
pub trait HostEvents: Send + Sync {
    /// The state changed; screens re-query. Coalesced per run-loop turn.
    fn state_changed(&self, version: u64);
    /// Open in the browser (Custom Tabs for the sign-in URLs).
    fn open_url(&self, url: String);
    /// A folder or file picker; answered with `Corvene::paths_picked`.
    fn pick_paths(&self, request: u64, directories: bool, multiple: bool, prompt: Option<String>);
    fn write_clipboard(&self, text: String);
    /// Open a file with another application, or (`reveal`) show its folder.
    fn open_path(&self, path: String, reveal: bool);
    fn toast(&self, message: String);
    fn show_notification(&self, identifier: String, title: String, body: String, payload: String);
    fn request_notification_permission(&self);
    fn request_all_files_access(&self);
    /// A network operation started or ended (foreground service).
    fn transfer_active(&self, active: bool);
    fn bring_to_front(&self);
    /// The engine wants the process to end (relaunch after a flag change).
    fn quit(&self);
    /// The share sheet for a file (`ACTION_SEND`).
    fn share_path(&self, path: String);
    /// A Termux session in `dir`, or `program` with `arguments` in one.
    fn open_termux(&self, dir: String);
    fn run_termux(&self, program: String, arguments: Vec<String>, dir: String);
    /// Opens `path` in the application `component` ("package/class") names,
    /// at `line` when it can.
    fn view_path_with(&self, path: String, component: String, line: Option<u32>);
    /// The applications that open a text file, as "label\tpackage/class".
    fn view_apps(&self) -> Vec<String>;
    fn package_installed(&self, package: String) -> bool;
    /// Start the process again (a flag that needs a restart was changed).
    fn relaunch(&self);
}

/// What the Android side knows at start-up that the engine asks for later.
#[derive(uniffi::Record, Clone, Debug, Default)]
pub struct HostInfo {
    pub has_all_files_access: bool,
    pub can_request_all_files_access: bool,
    pub allows_downloaded_code: bool,
    pub grammar_module_dir: Option<String>,
    pub notifications_allowed: Option<bool>,
    /// `Environment.getExternalStorageDirectory()` (`/storage/emulated/0`).
    pub shared_storage_dir: Option<String>,
}

/// The field is `reason`, not `message`: UniFFI's Kotlin error classes
/// already have a `message`.
#[derive(uniffi::Error, thiserror::Error, Debug)]
pub enum CoreError {
    #[error("{reason}")]
    Failed { reason: String },
}

/// The engine. One per process.
#[derive(uniffi::Object)]
pub struct Corvene {
    loop_: LoopHandle,
    services: Arc<Services>,
}

#[uniffi::export]
impl Corvene {
    /// Starts the engine: the process environment (`HOME`, the bundled
    /// git), the store under `files_dir`, the dispatcher on its own thread.
    /// `env` sets `CORVENE_*` variables (`CORVENE_FLAGS`, `CORVENE_LOG`)
    /// before anything reads them.
    #[uniffi::constructor]
    pub fn new(
        files_dir: String,
        env: HashMap<String, String>,
        info: HostInfo,
        events: Arc<dyn HostEvents>,
    ) -> Result<Arc<Self>, CoreError> {
        for (key, value) in &env {
            if key.starts_with("CORVENE_") {
                // SAFETY: before the engine spawns any thread
                unsafe { std::env::set_var(key, value) };
            }
        }
        let files = PathBuf::from(files_dir);
        #[cfg(target_os = "android")]
        {
            if !crate::jni::attached() {
                tracing::warn!("NativeContext.attach was not called: the Keystore is unavailable");
            }
            corvene_platform::android::prepare_environment(&files);
            corvene_platform::android::set_bridge(Box::new(crate::bridge::KotlinBridge {
                events: events.clone(),
                info: info.clone(),
            }));
        }
        #[cfg(not(target_os = "android"))]
        let _ = (&files, &info);
        crate::runtime::init_logging();
        Dispatcher::prefetch_git();
        #[cfg(target_os = "android")]
        corvene_core::askpass::serve_socket();

        let services = Arc::new(Services::new(events.clone()));
        let on_changed = {
            let events = events.clone();
            move |version| events.state_changed(version)
        };
        let loop_ = spawn_loop("corvene-main", services.clone(), on_changed).map_err(|err| {
            CoreError::Failed {
                reason: err.to_string(),
            }
        })?;
        let init = loop_.query_blocking(|host| -> Result<(), String> {
            let dir = corvene_platform::paths::app_support_dir();
            let store = corvene_store::Store::open_in(&dir).map_err(|err| err.to_string())?;
            let store = Arc::new(store);
            let settings = store.settings().unwrap_or_default();
            let flag_overrides = store.flags().unwrap_or_default();
            let (flags_env, flag_errors) = corvene_core::flags::env::from_env();
            for err in &flag_errors {
                tracing::warn!("{err}");
            }
            Dispatcher::init(store, settings, flag_overrides, flags_env, host);
            // what the desktop window does once it is up
            Dispatcher::start_background_tasks(host);
            Dispatcher::refresh_accounts(host);
            Dispatcher::start_pull_request_updater(host);
            Dispatcher::refresh_indicators(host);
            Ok(())
        });
        match init {
            Some(Ok(())) => {}
            Some(Err(reason)) => return Err(CoreError::Failed { reason }),
            None => {
                return Err(CoreError::Failed {
                    reason: "the engine thread ended during start-up".into(),
                });
            }
        }
        Ok(Arc::new(Corvene { loop_, services }))
    }

    /// Kotlin's answer to `HostEvents::pick_paths` (`None` = cancelled).
    pub fn paths_picked(&self, request: u64, paths: Option<Vec<String>>) {
        self.services.paths_picked(request, paths);
    }

    /// An `x-corvene://` URL or a GitHub URL the system handed over.
    pub fn app_url(&self, url: String) {
        self.loop_
            .post(move |host| Dispatcher::handle_app_url(&url, host));
    }

    /// The activity came to the front (GHD refreshes on window focus).
    pub fn focus(&self) {
        self.loop_.post(|host| Dispatcher::refresh_selected(host));
    }

    /// The application is on screen or not: periodic work (pull request
    /// updater, background fetch, indicators) runs only while it is.
    pub fn app_visible(&self, visible: bool) {
        self.loop_.post(move |host| {
            Dispatcher::set_app_focus_state(visible, host);
            if visible {
                Dispatcher::refresh_selected(host);
            }
        });
    }

    /// `true` once the constructor returned: the store is open and the
    /// state exists, so the first query answers at once.
    pub fn ready(&self) -> bool {
        true
    }

    pub fn select_repository(&self, id: u64) {
        self.loop_
            .post(move |host| Dispatcher::select_repository(id, host));
    }

    pub fn remove_repository(&self, id: u64) {
        self.loop_
            .post(move |host| Dispatcher::remove_repository(id, host));
    }

    pub fn add_repository(&self, path: String) {
        self.loop_
            .post(move |host| Dispatcher::add_repository(PathBuf::from(path), host));
    }

    pub fn refresh_indicators(&self) {
        self.loop_.post(|host| Dispatcher::refresh_indicators(host));
    }

    /// Bumps after every state change (the number `state_changed` sent).
    pub async fn version(&self) -> u64 {
        self.loop_.query(|host| host.version()).await
    }

    pub async fn repo_list(&self) -> RepoListVm {
        self.loop_.query(|host| repo_list(host.state_ref())).await
    }

    // ---- the Changes tab ----

    pub fn select_section(&self, repo: u64, history: bool) {
        self.loop_.post(move |host| {
            let section = if history {
                corvene_models::Section::History
            } else {
                corvene_models::Section::Changes
            };
            Dispatcher::show_section(repo, section, host)
        });
    }

    pub fn select_file(&self, repo: u64, path: String) {
        self.loop_
            .post(move |host| Dispatcher::select_file(repo, path, host));
    }

    pub fn toggle_file_included(&self, repo: u64, path: String) {
        self.loop_
            .post(move |host| Dispatcher::toggle_file_included(repo, path, host));
    }

    /// Toggles one add/delete line of `path` in or out of the next commit.
    pub fn toggle_diff_line(&self, repo: u64, path: String, line: u32) {
        self.loop_
            .post(move |host| Dispatcher::toggle_diff_line(repo, path, line, host));
    }

    pub fn commit(&self, repo: u64, summary: String, description: String) {
        // the Compose app has no `OversizedFiles` / `AddEmbeddedRepositories`
        // dialog yet: commit as before
        let checks = corvene_core::commit_checks::CommitChecks {
            allow_oversized: true,
            embedded: Some(Vec::new()),
            ..Default::default()
        };
        self.loop_
            .post(move |host| Dispatcher::commit_with(repo, summary, description, checks, host));
    }

    pub fn undo_commit(&self, repo: u64) {
        self.loop_
            .post(move |host| Dispatcher::undo_commit(repo, host));
    }

    pub fn discard_changes(&self, repo: u64, paths: Vec<String>) {
        self.loop_
            .post(move |host| Dispatcher::discard_changes(repo, paths, host));
    }

    pub fn refresh_repository(&self, repo: u64) {
        self.loop_
            .post(move |host| Dispatcher::refresh_repository(repo, host));
    }

    pub async fn changes(&self, repo: u64) -> Option<ChangesVm> {
        self.loop_
            .query(move |host| changes(host.state_ref(), repo))
            .await
    }

    pub async fn diff_header(&self, repo: u64) -> Option<DiffHeaderVm> {
        self.loop_
            .query(move |host| diff_header(host.state_ref(), repo))
            .await
    }

    pub async fn diff_rows(
        &self,
        repo: u64,
        generation: u64,
        start: u32,
        count: u32,
    ) -> Vec<DiffRowVm> {
        self.loop_
            .query(move |host| diff_rows(host.state_ref(), repo, generation, start, count))
            .await
    }

    // ---- the History tab ----

    pub fn load_more_commits(&self, repo: u64) {
        self.loop_
            .post(move |host| Dispatcher::load_commits(repo, true, host));
    }

    pub fn select_commit(&self, repo: u64, sha: String) {
        self.loop_
            .post(move |host| Dispatcher::select_commit(repo, sha, host));
    }

    pub fn select_commits(&self, repo: u64, shas: Vec<String>) {
        self.loop_
            .post(move |host| Dispatcher::select_commits(repo, shas, host));
    }

    pub fn select_commit_file(&self, repo: u64, path: String) {
        self.loop_
            .post(move |host| Dispatcher::select_commit_file(repo, path, host));
    }

    pub async fn history(&self, repo: u64, start: u32, count: u32) -> Option<HistoryVm> {
        self.loop_
            .query(move |host| history(host.state_ref(), repo, start, count))
            .await
    }

    pub async fn commit_detail(&self, repo: u64) -> Option<CommitDetailVm> {
        self.loop_
            .query(move |host| commit_detail(host.state_ref(), repo))
            .await
    }

    // ---- branches and the sync button ----

    /// `strategy`: `None` asks when there are uncommitted changes (GHD's
    /// dialog), `"stash"` or `"move"` decides.
    pub fn checkout_branch(&self, repo: u64, name: String, strategy: Option<String>) {
        let explicit = match strategy.as_deref() {
            Some("stash") => Some(UncommittedChangesStrategy::StashOnCurrentBranch),
            Some("move") => Some(UncommittedChangesStrategy::MoveToNewBranch),
            _ => None,
        };
        self.loop_
            .post(move |host| Dispatcher::checkout_branch(repo, name, explicit, host));
    }

    pub fn create_branch(&self, repo: u64, name: String, start_point: Option<String>) {
        self.loop_
            .post(move |host| Dispatcher::create_branch(repo, name, start_point, false, host));
    }

    pub fn rename_branch(&self, repo: u64, old: String, new: String) {
        self.loop_
            .post(move |host| Dispatcher::rename_branch(repo, old, new, host));
    }

    pub fn delete_branch(&self, repo: u64, name: String, include_remote: bool) {
        self.loop_
            .post(move |host| Dispatcher::delete_branch(repo, name, include_remote, host));
    }

    pub fn fetch(&self, repo: u64) {
        self.loop_
            .post(move |host| Dispatcher::fetch(repo, false, host));
    }

    pub fn pull(&self, repo: u64) {
        self.loop_.post(move |host| Dispatcher::pull(repo, host));
    }

    pub fn push(&self, repo: u64, force_with_lease: bool) {
        self.loop_
            .post(move |host| Dispatcher::push(repo, force_with_lease, None, host));
    }

    pub async fn branches(&self, repo: u64) -> Option<BranchesVm> {
        self.loop_
            .query(move |host| branches(host.state_ref(), repo))
            .await
    }

    // ---- dialogs and banners ----

    /// Dismisses the open dialog (Escape / Cancel).
    pub fn close_popup(&self) {
        self.loop_.post(|host| Dispatcher::close_popup(host));
    }

    pub async fn popup(&self) -> Option<PopupVm> {
        self.loop_.query(|host| popup(host.state_ref())).await
    }

    pub async fn banner(&self) -> Option<BannerVm> {
        self.loop_.query(|host| banner(host.state_ref())).await
    }

    // ---- settings ----

    pub fn set_design_style(&self, style: DesignStyleVm) {
        self.loop_.post(move |host| {
            Dispatcher::update_settings(host, |settings| settings.design_style = style.into())
        });
    }

    pub fn set_theme(&self, theme: ThemeVm) {
        self.loop_.post(move |host| {
            Dispatcher::update_settings(host, |settings| {
                settings.theme = match theme {
                    ThemeVm::Light => corvene_models::ThemeSetting::Light,
                    ThemeVm::Dark => corvene_models::ThemeSetting::Dark,
                    ThemeVm::System => corvene_models::ThemeSetting::System,
                    ThemeVm::HighContrast => corvene_models::ThemeSetting::HighContrast,
                }
            })
        });
    }

    pub async fn settings(&self) -> SettingsVm {
        self.loop_.query(|host| settings(host.state_ref())).await
    }

    // ---- the Changes tab, continued ----

    /// Toggles one of the file-list filter chips: `included`, `excluded`,
    /// `new`, `modified`, `deleted`, `renamed`.
    pub fn toggle_filter_option(&self, repo: u64, option: String) {
        use corvene_core::state::FilterOption;
        let option = match option.as_str() {
            "included" => FilterOption::IncludedInCommit,
            "excluded" => FilterOption::ExcludedFromCommit,
            "new" => FilterOption::NewFiles,
            "modified" => FilterOption::ModifiedFiles,
            "deleted" => FilterOption::DeletedFiles,
            "renamed" => FilterOption::RenamedFiles,
            _ => return,
        };
        self.loop_
            .post(move |host| Dispatcher::toggle_filter_option(repo, option, host));
    }

    pub fn clear_filter_options(&self, repo: u64) {
        self.loop_
            .post(move |host| Dispatcher::clear_filter_options(repo, host));
    }

    pub fn stash_all_changes(&self, repo: u64) {
        self.loop_
            .post(move |host| Dispatcher::stash_all_changes(repo, host));
    }

    /// Restores the branch's stash (GHD "Restore").
    pub fn pop_stash(&self, repo: u64) {
        self.loop_
            .post(move |host| Dispatcher::pop_stash(repo, host));
    }

    /// Drops the branch's stash (GHD "Discard").
    pub fn drop_stash(&self, repo: u64) {
        self.loop_
            .post(move |host| Dispatcher::drop_stash(repo, host));
    }

    pub fn ignore_files(&self, repo: u64, paths: Vec<String>) {
        self.loop_
            .post(move |host| Dispatcher::ignore_files(repo, paths, host));
    }

    /// "Amend last commit": the form takes the commit's message.
    pub fn start_amending(&self, repo: u64, sha: String) {
        self.loop_
            .post(move |host| Dispatcher::start_amending(repo, sha, host));
    }

    pub fn stop_amending(&self, repo: u64) {
        self.loop_
            .post(move |host| Dispatcher::stop_amending(repo, host));
    }

    /// The diff gear menu's "Hide whitespace changes" (Changes or History).
    pub fn set_hide_whitespace_in_diff(&self, history: bool, hide: bool) {
        self.loop_
            .post(move |host| Dispatcher::set_hide_whitespace_in_diff(history, hide, host));
    }

    // ---- sign-in, clone, new and the welcome flow ----

    /// Starts the sign-in the `307-sign-in-flow` flag picks (device code
    /// or browser) for GitHub.com, or for the enterprise host given.
    pub fn sign_in(&self, enterprise_host: Option<String>) {
        self.loop_.post(move |host| {
            let endpoint = match enterprise_host {
                Some(input) => match corvene_github::Endpoint::enterprise(&input, false) {
                    Some(endpoint) => endpoint,
                    None => return,
                },
                None => corvene_github::Endpoint::github_com(),
            };
            Dispatcher::begin_sign_in(endpoint, host)
        });
    }

    /// The device-code flow regardless of the flag.
    pub fn sign_in_device_flow(&self, enterprise_host: Option<String>) {
        self.loop_.post(move |host| {
            let endpoint = match enterprise_host {
                Some(input) => match corvene_github::Endpoint::enterprise(&input, false) {
                    Some(endpoint) => endpoint,
                    None => return,
                },
                None => corvene_github::Endpoint::github_com(),
            };
            Dispatcher::sign_in_device_flow(endpoint, host)
        });
    }

    /// A personal access token (enterprise, or the fallback).
    pub fn sign_in_with_token(&self, enterprise_host: Option<String>, token: String) {
        self.loop_.post(move |host| {
            let endpoint = match enterprise_host {
                Some(input) => match corvene_github::Endpoint::enterprise(&input, false) {
                    Some(endpoint) => endpoint,
                    None => return,
                },
                None => corvene_github::Endpoint::github_com(),
            };
            Dispatcher::sign_in_with_token(endpoint, token, host)
        });
    }

    pub fn cancel_sign_in(&self) {
        self.loop_.post(|host| Dispatcher::cancel_sign_in(host));
    }

    pub fn sign_out(&self, endpoint: String) {
        self.loop_
            .post(move |host| Dispatcher::sign_out(endpoint, host));
    }

    pub fn complete_welcome(&self) {
        self.loop_.post(|host| Dispatcher::complete_welcome(host));
    }

    /// Clones `url` into `path` (`depth` > 0 = shallow).
    pub fn clone_repository(&self, url: String, path: String, depth: u32) {
        self.loop_.post(move |host| {
            Dispatcher::clone_repository_with(
                url,
                PathBuf::from(path),
                None,
                (depth > 0).then_some(depth),
                host,
            )
        });
    }

    pub fn cancel_clone(&self) {
        self.loop_.post(|host| Dispatcher::cancel_clone(host));
    }

    pub fn create_repository(
        &self,
        path: String,
        name: String,
        description: Option<String>,
        readme: bool,
        gitignore: Option<String>,
        license: Option<String>,
    ) {
        self.loop_.post(move |host| {
            Dispatcher::create_repository(
                PathBuf::from(path),
                name,
                description,
                readme,
                gitignore,
                license,
                false,
                host,
            )
        });
    }

    pub async fn session(&self) -> SessionVm {
        self.loop_.query(|host| session(host.state_ref())).await
    }

    // ---- history actions and multi-commit operations ----

    pub fn revert_commit(&self, repo: u64, sha: String) {
        self.loop_
            .post(move |host| Dispatcher::revert_commit(repo, sha, host));
    }

    pub fn reset_to_commit(&self, repo: u64, sha: String) {
        self.loop_
            .post(move |host| Dispatcher::reset_to_commit(repo, sha, host));
    }

    pub fn checkout_commit(&self, repo: u64, sha: String) {
        self.loop_
            .post(move |host| Dispatcher::checkout_commit(repo, sha, host));
    }

    pub fn create_tag(&self, repo: u64, name: String, sha: String, message: String) {
        self.loop_
            .post(move |host| Dispatcher::create_tag(repo, name, sha, message, host));
    }

    /// `mode`: "behind" (their commits) or "ahead" (ours); `None` ends the comparison.
    pub fn compare_to_branch(&self, repo: u64, branch: Option<String>, mode: String) {
        self.loop_.post(move |host| match branch {
            Some(branch) => {
                let mode = if mode == "ahead" {
                    corvene_core::compare::ComparisonMode::Ahead
                } else {
                    corvene_core::compare::ComparisonMode::Behind
                };
                Dispatcher::compare_to_branch(repo, branch, mode, host)
            }
            None => Dispatcher::exit_compare(repo, host),
        });
    }

    pub fn merge_branch(&self, repo: u64, branch: String, squash: bool) {
        self.loop_
            .post(move |host| Dispatcher::merge_branch(repo, branch, squash, host));
    }

    pub fn start_rebase(&self, repo: u64, base_branch: String) {
        self.loop_
            .post(move |host| Dispatcher::start_rebase(repo, base_branch, false, host));
    }

    pub fn abort_mco(&self, repo: u64) {
        self.loop_
            .post(move |host| Dispatcher::abort_mco(repo, host));
    }

    pub fn reorder_commits(&self, repo: u64, to_move: Vec<String>, before: Option<String>) {
        self.loop_
            .post(move |host| Dispatcher::reorder_commits(repo, to_move, before, false, host));
    }

    // ---- the operation in flight and its conflicts ----

    pub async fn mco(&self, repo: u64) -> Option<McoVm> {
        self.loop_
            .query(move |host| mco(host.state_ref(), repo))
            .await
    }

    pub async fn conflicts(&self, repo: u64) -> Option<ConflictsVm> {
        self.loop_
            .query(move |host| conflicts(host.state_ref(), repo))
            .await
    }

    /// Moves the wizard to `step`: "choose-branch", "warn-force-push",
    /// "show-progress", "show-conflicts", "hide-conflicts", "confirm-abort".
    pub fn set_mco_step(&self, repo: u64, step: String) {
        use corvene_core::mco::McoStep;
        let step = match step.as_str() {
            "choose-branch" => McoStep::ChooseBranch,
            "warn-force-push" => McoStep::WarnForcePush,
            "show-progress" => McoStep::ShowProgress,
            "show-conflicts" => McoStep::ShowConflicts,
            "hide-conflicts" => McoStep::HideConflicts,
            "confirm-abort" => McoStep::ConfirmAbort,
            _ => return,
        };
        self.loop_
            .post(move |host| Dispatcher::set_mco_step(repo, step, host));
    }

    pub fn show_conflicts(&self, repo: u64) {
        self.loop_
            .post(move |host| Dispatcher::show_conflicts(repo, host));
    }

    pub fn hide_conflicts(&self, repo: u64) {
        self.loop_
            .post(move |host| Dispatcher::hide_conflicts(repo, host));
    }

    /// Ours / Theirs for a conflicted file; `None` goes back to manual.
    pub fn set_manual_resolution(&self, repo: u64, path: String, resolution: Option<ResolutionVm>) {
        let resolution = resolution.map(|r| match r {
            ResolutionVm::Ours => corvene_models::ManualConflictResolution::Ours,
            ResolutionVm::Theirs => corvene_models::ManualConflictResolution::Theirs,
        });
        self.loop_
            .post(move |host| Dispatcher::set_manual_resolution(repo, path, resolution, host));
    }

    pub fn continue_after_conflicts(&self, repo: u64) {
        self.loop_
            .post(move |host| Dispatcher::continue_after_conflicts(repo, host));
    }

    /// History › Cherry-pick…: starts the flow for the selected commits.
    pub fn start_cherry_pick(&self, repo: u64, shas: Vec<String>) {
        self.loop_
            .post(move |host| Dispatcher::start_cherry_pick_flow(repo, shas, host));
    }

    pub fn cherry_pick_to_branch(&self, repo: u64, target: String) {
        self.loop_
            .post(move |host| Dispatcher::cherry_pick_to_branch(repo, target, host));
    }

    pub fn cherry_pick_to_new_branch(&self, repo: u64, name: String, start_point: Option<String>) {
        self.loop_
            .post(move |host| Dispatcher::cherry_pick_to_new_branch(repo, name, start_point, host));
    }

    pub fn squash(&self, repo: u64, to_squash: Vec<String>, onto: String, message: String) {
        self.loop_
            .post(move |host| Dispatcher::squash(repo, to_squash, onto, message, false, host));
    }

    // ---- pull requests ----

    pub async fn pull_requests(&self, repo: u64) -> Option<PullRequestsVm> {
        self.loop_
            .query(move |host| pull_requests(host.state_ref(), repo))
            .await
    }

    pub fn checkout_pull_request(&self, repo: u64, number: u64) {
        self.loop_.post(move |host| {
            let pr = host
                .state_ref()
                .pull_requests_for(repo)
                .iter()
                .find(|pr| pr.number == number)
                .cloned();
            if let Some(pr) = pr {
                Dispatcher::checkout_pull_request(repo, pr, host);
            }
        });
    }

    // ---- git identity (Welcome › Configure Git, Settings › Git) ----

    /// Writes `user.name` / `user.email` to the global git config.
    pub fn set_global_identity(&self, name: String, email: String) {
        self.loop_
            .post(move |host| Dispatcher::set_global_identity(name, email, host));
    }

    // ---- more of the diff, dialogs that answer with data, publishing ----

    /// Rows of the selected commit's selected file (`CommitDetailVm.diff_generation`).
    pub async fn commit_diff_rows(
        &self,
        repo: u64,
        generation: u64,
        start: u32,
        count: u32,
    ) -> Vec<DiffRowVm> {
        self.loop_
            .query(move |host| commit_diff_rows(host.state_ref(), repo, generation, start, count))
            .await
    }

    /// Includes or excludes `len` lines of `path` from `from` (drag selection).
    pub fn set_diff_lines(&self, repo: u64, path: String, from: u32, len: u32, selected: bool) {
        self.loop_
            .post(move |host| Dispatcher::set_diff_lines(repo, path, from, len, selected, host));
    }

    /// `GenericGitAuthentication` › Save: stores the credentials and retries
    /// the operation the open dialog came from.
    pub fn submit_generic_auth(&self, username: String, password: String) {
        self.loop_.post(move |host| {
            let Some(corvene_core::state::Popup::GenericGitAuthentication {
                repo,
                host: git_host,
                retry,
                ..
            }) = host.state_ref().popup().cloned()
            else {
                return;
            };
            Dispatcher::close_popup(host);
            Dispatcher::save_generic_credentials(git_host, username, password, repo, retry, host);
        });
    }

    /// `ProxyAuthentication` › Save (`528-proxy-credentials`): keeps the
    /// proxy's credentials and retries what the open dialog came from.
    pub fn submit_proxy_auth(&self, username: String, password: String) {
        self.loop_.post(move |host| {
            let Some(corvene_core::state::Popup::ProxyAuthentication {
                proxy,
                server,
                retry,
                ..
            }) = host.state_ref().popup().cloned()
            else {
                return;
            };
            if server {
                Dispatcher::save_front_door_credentials(proxy, username, password, host);
            } else {
                Dispatcher::save_proxy_credentials(proxy, username, password, retry, host);
            }
        });
    }

    /// `LocalChangesOverwritten` › the retry button of the open dialog.
    pub fn retry_popup_action(&self) {
        self.loop_.post(move |host| {
            let Some(corvene_core::state::Popup::LocalChangesOverwritten { repo, retry, .. }) =
                host.state_ref().popup().cloned()
            else {
                return;
            };
            Dispatcher::close_popup(host);
            Dispatcher::perform_retry(repo, retry, host);
        });
    }

    /// Publishes the repository to GitHub under the account of `endpoint`.
    pub fn publish_repository(
        &self,
        repo: u64,
        name: String,
        description: String,
        private: bool,
        endpoint: String,
        org: Option<String>,
    ) {
        self.loop_.post(move |host| {
            let Some(account) = host
                .state_ref()
                .accounts
                .iter()
                .find(|a| a.endpoint == endpoint)
                .cloned()
            else {
                return;
            };
            Dispatcher::publish_repository(
                repo,
                name,
                description,
                private,
                account,
                org,
                None,
                host,
            );
        });
    }

    // ---- GHD's own dialogs, opened the way its menus do ----

    pub fn request_delete_branch(&self, repo: u64, name: String) {
        self.loop_.post(move |host| {
            Dispatcher::show_popup(
                corvene_core::state::Popup::DeleteBranch { repo, name },
                host,
            )
        });
    }

    pub fn request_rename_branch(&self, repo: u64, name: String) {
        self.loop_.post(move |host| {
            Dispatcher::show_popup(
                corvene_core::state::Popup::RenameBranch { repo, name },
                host,
            )
        });
    }

    pub fn request_create_branch(
        &self,
        repo: u64,
        target_sha: Option<String>,
        initial_name: String,
    ) {
        self.loop_.post(move |host| {
            Dispatcher::show_popup(
                corvene_core::state::Popup::CreateBranch {
                    repo,
                    target_sha,
                    initial_name,
                },
                host,
            )
        });
    }

    pub fn request_create_tag(&self, repo: u64, sha: String) {
        self.loop_.post(move |host| {
            Dispatcher::show_popup(corvene_core::state::Popup::CreateTag { repo, sha }, host)
        });
    }

    pub fn request_merge(&self, repo: u64, squash: bool) {
        self.loop_.post(move |host| {
            Dispatcher::show_popup(
                corvene_core::state::Popup::MergeBranch { repo, squash },
                host,
            )
        });
    }

    pub fn request_publish_repository(&self, repo: u64) {
        self.loop_.post(move |host| {
            Dispatcher::show_popup(corvene_core::state::Popup::PublishRepository { repo }, host)
        });
    }

    /// Respects "Confirm before checking out a commit".
    pub fn request_checkout_commit(&self, repo: u64, sha: String) {
        self.loop_
            .post(move |host| Dispatcher::request_checkout_commit(repo, sha, host));
    }

    pub fn request_reset_to_commit(&self, repo: u64, sha: String) {
        self.loop_
            .post(move |host| Dispatcher::request_reset_to_commit(repo, sha, host));
    }

    /// Pushes, or asks first when it would be a force push.
    pub fn confirm_or_force_push(&self, repo: u64) {
        self.loop_
            .post(move |host| Dispatcher::confirm_or_force_push(repo, host));
    }

    pub fn request_drop_stash(&self, repo: u64) {
        self.loop_
            .post(move |host| Dispatcher::request_drop_stash(repo, host));
    }

    /// Respects "Confirm before undoing a commit" and the tag warnings.
    pub fn request_undo_commit(&self, repo: u64) {
        self.loop_
            .post(move |host| Dispatcher::request_undo_commit(repo, host));
    }

    pub fn request_discard_changes(&self, repo: u64, paths: Vec<String>) {
        self.loop_
            .post(move |host| Dispatcher::request_discard_changes(repo, paths, host));
    }

    pub fn request_remove_repository(&self, repo: u64) {
        self.loop_
            .post(move |host| Dispatcher::request_remove_repository(repo, host));
    }

    /// Branch › Rebase current branch…: the ChooseBranch step, `base`
    /// preselected when given.
    pub fn start_rebase_flow(&self, repo: u64, base: Option<String>) {
        self.loop_
            .post(move |host| Dispatcher::start_rebase_flow_onto(repo, base, host));
    }

    /// WarnForcePush › Begin rebase.
    pub fn start_rebase_with(&self, repo: u64, base_branch: String, force_push_checked: bool) {
        self.loop_.post(move |host| {
            Dispatcher::start_rebase(repo, base_branch, force_push_checked, host)
        });
    }

    /// Leaves the operation wizard (the ChooseBranch step was dismissed).
    pub fn end_mco(&self, repo: u64) {
        self.loop_.post(move |host| Dispatcher::end_mco(repo, host));
    }

    pub fn request_abort_mco(&self, repo: u64) {
        self.loop_
            .post(move |host| Dispatcher::request_abort_mco(repo, host));
    }

    /// `LocalChangesOverwritten` › Stash changes and retry.
    pub fn stash_and_retry(&self) {
        self.loop_.post(move |host| {
            let Some(corvene_core::state::Popup::LocalChangesOverwritten { repo, retry, .. }) =
                host.state_ref().popup().cloned()
            else {
                return;
            };
            Dispatcher::close_popup(host);
            Dispatcher::stash_and_retry(repo, retry, host);
        });
    }

    /// The `BranchDeleted` banner's Undo (flag `861`).
    pub fn undo_delete_branch(&self) {
        self.loop_.post(move |host| {
            let Some(corvene_core::mco::Banner::BranchDeleted { repo, branch, sha }) =
                host.state_ref().banner.clone()
            else {
                return;
            };
            Dispatcher::restore_deleted_branch(repo, branch, sha, host);
        });
    }

    /// `StashAndSwitchBranch` › Discard my changes (flag `865`).
    pub fn discard_all_and_checkout(&self, repo: u64, branch: String) {
        self.loop_
            .post(move |host| Dispatcher::discard_all_and_checkout(repo, branch, host));
    }

    /// Branch › Update from default branch (merge, or rebase per flag `859`).
    pub fn update_from_default(&self, repo: u64) {
        self.loop_
            .post(move |host| Dispatcher::update_from_default_branch(repo, host));
    }

    /// `UpstreamAlreadyExists` › Update remote / Ignore.
    pub fn update_upstream_remote(&self, repo: u64, update: bool) {
        self.loop_.post(move |host| {
            if update {
                Dispatcher::update_existing_upstream_remote(repo, host)
            } else {
                Dispatcher::ignore_existing_upstream_remote(repo, host)
            }
        });
    }

    /// `CICheckRunRerun` › Re-run: the checks of the open dialog.
    pub fn rerun_checks(&self, failed_only: bool) {
        self.loop_.post(move |host| {
            let Some(corvene_core::state::Popup::CICheckRunRerun { github, checks, .. }) =
                host.state_ref().popup().cloned()
            else {
                return;
            };
            Dispatcher::close_popup(host);
            Dispatcher::rerequest_check_suites(github, checks, failed_only, |_, _| {}, host);
        });
    }

    // ---- settings by name, and the Flags screen ----

    /// Sets one setting: a boolean one with "true"/"false", a text one with
    /// its text (empty = none), `uncommitted_changes_strategy` with
    /// "ask" / "stash" / "move". Unknown keys are ignored.
    pub fn set_setting(&self, key: String, value: String) {
        self.loop_.post(move |host| {
            Dispatcher::update_settings(host, |s| {
                let b = value == "true";
                let text = (!value.is_empty()).then(|| value.clone());
                match key.as_str() {
                    "confirm_discard_changes" => s.confirm_discard_changes = b,
                    "confirm_discard_changes_permanently" => {
                        s.confirm_discard_changes_permanently = b
                    }
                    "confirm_checkout_commit" => s.confirm_checkout_commit = b,
                    "confirm_undo_commit" => s.confirm_undo_commit = b,
                    "confirm_discard_stash" => s.confirm_discard_stash = b,
                    "confirm_force_push" => s.confirm_force_push = b,
                    "confirm_repository_removal" => s.confirm_repository_removal = b,
                    "confirm_commit_filtered_changes" => s.confirm_commit_filtered_changes = b,
                    "notifications_enabled" => s.notifications_enabled = b,
                    "repository_indicators_enabled" => s.repository_indicators_enabled = b,
                    "hide_whitespace_in_changes_diff" => s.hide_whitespace_in_changes_diff = b,
                    "hide_whitespace_in_history_diff" => s.hide_whitespace_in_history_diff = b,
                    "show_diff_check_marks" => s.show_diff_check_marks = b,
                    "underline_links" => s.underline_links = b,
                    "show_commit_length_warning" => s.show_commit_length_warning = b,
                    "commit_spellcheck_enabled" => s.commit_spellcheck_enabled = b,
                    "history_first_parent" => s.history_first_parent = b,
                    "use_external_credential_helper" => s.use_external_credential_helper = b,
                    "uncommitted_changes_strategy" => {
                        use corvene_core::persistence::UncommittedChangesStrategy as U;
                        s.uncommitted_changes_strategy = match value.as_str() {
                            "stash" => U::StashOnCurrentBranch,
                            "move" => U::MoveToNewBranch,
                            _ => U::AskForConfirmation,
                        }
                    }
                    "external_editor" => s.external_editor = text,
                    "shell" => s.shell = text,
                    "clone_dir" => s.clone_dir = text.map(PathBuf::from),
                    _ => {}
                }
            })
        });
    }

    pub async fn flags(&self) -> FlagsVm {
        self.loop_.query(|host| flags(host.state_ref())).await
    }

    /// Sets a flag by slug from its text form; the error names what was wrong.
    pub fn set_flag_by_slug(&self, slug: String, value: String) -> Result<(), CoreError> {
        let def = corvene_core::flags::by_slug(&slug).ok_or_else(|| CoreError::Failed {
            reason: format!("no flag {slug}"),
        })?;
        let value = def
            .kind
            .parse(&value)
            .map_err(|reason| CoreError::Failed { reason })?;
        let id = def.id;
        let result = self
            .loop_
            .query_blocking(move |host| Dispatcher::set_flag(id, value, host))
            .unwrap_or_else(|| Err("the engine thread is gone".into()));
        result.map_err(|reason| CoreError::Failed { reason })
    }

    pub fn reset_flag(&self, slug: String) {
        let Some(def) = corvene_core::flags::by_slug(&slug) else {
            return;
        };
        let id = def.id;
        self.loop_
            .post(move |host| Dispatcher::reset_flag(id, host));
    }

    /// "github-desktop", "familiar", "corvene" or "max".
    pub fn apply_preset(&self, preset: String) {
        let Some(preset) = corvene_core::flags::Preset::parse(&preset) else {
            return;
        };
        self.loop_
            .post(move |host| Dispatcher::apply_preset(preset, host));
    }

    pub fn reset_all_flags(&self) {
        self.loop_.post(|host| Dispatcher::reset_all_flags(host));
    }

    /// Flags that need a restart: relaunch the process (the host quits).
    pub fn relaunch(&self) {
        self.loop_.post(|host| Dispatcher::relaunch(host));
    }

    // ---- repository settings and the global git config ----

    /// Loads the repository's settings (`tab`: "remote", "ignored", "git").
    pub fn open_repository_settings(&self, repo: u64, tab: String) {
        use corvene_core::state::RepositorySettingsTab;
        let tab = match tab.as_str() {
            "ignored" => RepositorySettingsTab::IgnoredFiles,
            "git" => RepositorySettingsTab::GitConfig,
            _ => RepositorySettingsTab::Remote,
        };
        self.loop_
            .post(move |host| Dispatcher::open_repository_settings(repo, tab, host));
    }

    pub async fn repository_settings(&self) -> Option<RepositorySettingsVm> {
        self.loop_
            .query(|host| repository_settings(host.state_ref()))
            .await
    }

    /// Saves what changed: the remote URL (with its name), the `.gitignore`
    /// text, the identity (`git_config_location` "global"/"local"), and
    /// `core.autocrlf` ("true"/"false"/"input"/"" for unset).
    #[allow(clippy::too_many_arguments)]
    pub fn save_repository_settings(
        &self,
        repo: u64,
        remote_name: Option<String>,
        remote_url: Option<String>,
        gitignore: Option<String>,
        git_config_location: Option<String>,
        name: Option<String>,
        email: Option<String>,
        autocrlf: Option<String>,
    ) {
        use corvene_core::integrations::RepositorySettingsSave;
        use corvene_core::state::GitConfigLocation;
        let save = RepositorySettingsSave {
            remote_url: remote_name.zip(remote_url),
            gitignore,
            git_config: git_config_location.map(|loc| {
                (
                    if loc == "local" {
                        GitConfigLocation::Local
                    } else {
                        GitConfigLocation::Global
                    },
                    name.unwrap_or_default(),
                    email.unwrap_or_default(),
                )
            }),
            autocrlf: autocrlf.map(|v| (!v.is_empty()).then_some(v)),
            ..Default::default()
        };
        self.loop_
            .post(move |host| Dispatcher::save_repository_settings(repo, save, host));
    }

    /// Loads the global git config for Settings › Git.
    pub fn load_global_git_config(&self) {
        self.loop_
            .post(|host| Dispatcher::load_global_git_config(host));
    }

    /// Opens `~/.gitconfig` in an editor (desktop Settings › Git › Edit).
    pub fn open_global_git_config(&self) {
        self.loop_
            .post(|host| Dispatcher::edit_global_git_config(host));
    }

    /// Settings › Git › Default branch name (`init.defaultBranch`).
    pub fn set_default_branch(&self, name: String) {
        self.loop_
            .post(move |host| Dispatcher::set_global_default_branch(name, host));
    }

    pub async fn global_git_config(&self) -> Option<GlobalGitConfigVm> {
        self.loop_
            .query(|host| global_git_config(host.state_ref()))
            .await
    }

    // ---- the Clone dialog's repository list, notifications, the worker ----

    /// Starts loading the account's repositories (cached in the store).
    pub fn load_cloneable_repositories(&self, endpoint: String) {
        self.loop_.post(move |host| {
            let Some(account) = host
                .state_ref()
                .accounts
                .iter()
                .find(|a| a.endpoint == endpoint)
                .cloned()
            else {
                return;
            };
            Dispatcher::load_api_repositories(account, host);
        });
    }

    pub async fn cloneable_repositories(&self, endpoint: String) -> CloneableRepositoriesVm {
        self.loop_
            .query(move |host| cloneable_repositories(host.state_ref(), &endpoint))
            .await
    }

    /// A tap on a notification the engine posted (`HostEvents::show_notification`).
    pub fn notification_clicked(&self, identifier: String, payload: Option<String>) {
        #[cfg(target_os = "android")]
        corvene_platform::notifications::clicked(
            corvene_platform::notifications::NotificationClick {
                identifier,
                payload,
            },
        );
        #[cfg(not(target_os = "android"))]
        let _ = (identifier, payload);
    }

    /// WorkManager's hourly tick while the app is alive: one fetch round.
    pub fn background_fetch(&self) {
        self.loop_
            .post(|host| Dispatcher::background_fetch_tick(host));
    }

    // ---- the rest the Android side asked for ----

    /// `user.name` / `user.email` from the global git config.
    pub async fn git_identity(&self) -> Option<GitIdentityVm> {
        self.loop_
            .query(|host| {
                let git = host.state_ref().git.clone()?;
                let identity = corvene_git::global_identity(git);
                Some(GitIdentityVm {
                    name: identity.name,
                    email: identity.email,
                })
            })
            .await
    }

    /// The bundled `.gitignore` templates, by name.
    pub fn gitignore_names(&self) -> Vec<String> {
        corvene_core::templates::gitignore_names()
    }

    /// The bundled licenses.
    pub fn licenses(&self) -> Vec<LicenseVm> {
        corvene_core::templates::licenses()
            .into_iter()
            .map(|l| LicenseVm {
                name: l.name,
                featured: l.featured,
                hidden: l.hidden,
            })
            .collect()
    }

    /// What changed at run time (all-files access granted, notifications
    /// allowed…): the engine's Android paths read it from here on.
    pub fn set_host_info(&self, info: HostInfo) {
        #[cfg(target_os = "android")]
        crate::bridge::set_host_info(info);
        #[cfg(not(target_os = "android"))]
        let _ = info;
    }

    /// A git network command is running (the worker waits for `false`).
    pub fn network_busy(&self) -> bool {
        #[cfg(target_os = "android")]
        {
            corvene_platform::android::network_busy()
        }
        #[cfg(not(target_os = "android"))]
        {
            false
        }
    }

    /// Stops the transfers the user can stop: the clone in progress.
    pub fn cancel_transfers(&self) {
        self.loop_.post(|host| Dispatcher::cancel_clone(host));
    }
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct GitIdentityVm {
    pub name: Option<String>,
    pub email: Option<String>,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct LicenseVm {
    pub name: String,
    pub featured: bool,
    pub hidden: bool,
}
