//! `Dispatcher`: the only thing that mutates `AppState`. Every method is
//! callable from the UI with an `&mut dyn Host`; git and disk work runs on the
//! background executor and results are applied on the main thread.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::host::{AsyncCtx, Host, StateHandle};
use corvene_git::{GitError, InitOptions, find_git, open_repository};
use corvene_store::Store;
use tracing::{error, info, warn};

use crate::persistence::{Settings, StoreExt, UncommittedChangesStrategy};
use crate::sign_in::{ResultCallback, SignInResult, SignInStore};
use crate::state::{
    AppState, AuthenticationFlow, AuthenticationStep, CloneState, ErrorMessage, Foldout,
    LastCommit, Popup, RepositoryState, RetryAction,
};
use corvene_models::{DiffSelectionType, Section, WorkingDirectoryFileChange, github_from_remote};
use std::sync::atomic::{AtomicBool, Ordering};

const RECENT_REPOSITORIES_LENGTH: usize = 3;

pub struct Dispatcher;

impl Dispatcher {
    /// Load persisted state, create the global entity, kick off git detection
    /// and a refresh of the selected repository.
    pub fn init(
        store: Arc<Store>,
        settings: Settings,
        flag_overrides: crate::flags::FlagOverrides,
        flags_env: crate::flags::EnvFlags,
        cx: &mut dyn Host,
    ) -> StateHandle {
        let flags = crate::flags::Flags::resolve(&flag_overrides, &flags_env);
        // Corvene (`287-repository-list-backup`): entries that do not decode
        // are set aside instead of the whole list loading empty (and the
        // next save overwriting it)
        let mut unreadable_banner = None;
        let mut repositories = if flags.bool(crate::flags::ids::REPOSITORY_LIST_BACKUP) {
            match store.repositories_keeping_unreadable() {
                Ok(loaded) => {
                    if loaded.raw_backup.is_some() || loaded.newly_unreadable > 0 {
                        unreadable_banner = Some(crate::mco::Banner::RepositoriesUnreadable {
                            count: loaded.newly_unreadable,
                            raw: loaded.raw_backup.is_some(),
                        });
                    }
                    loaded.repositories
                }
                Err(err) => {
                    error!(?err, "could not load repositories");
                    Vec::new()
                }
            }
        } else {
            store.repositories().unwrap_or_else(|err| {
                error!(?err, "could not load repositories");
                Vec::new()
            })
        };
        if flags.bool(crate::flags::ids::WIKI_NOT_GITHUB) {
            for repo in &mut repositories {
                if repo.github.as_ref().is_some_and(|gh| gh.is_wiki()) {
                    repo.github = None;
                }
            }
        }
        let recent = store.recent_repositories().unwrap_or_default();
        let mut recent_worktrees = store.recent_worktrees().unwrap_or_default();
        recent_worktrees.retain(|(id, _)| repositories.iter().any(|r| r.id == *id));
        let selected = store
            .selected_repository()
            .ok()
            .flatten()
            .filter(|id| repositories.iter().any(|r| r.id == *id))
            .or_else(|| recent.first().copied())
            .or_else(|| repositories.first().map(|r| r.id));
        // `AccountsStore.loadFromStore` sorts GitHub.com first
        let accounts = crate::accounts::sort_accounts(store.accounts().unwrap_or_default());
        let sign_in_accounts = std::rc::Rc::new(std::cell::RefCell::new(accounts.clone()));
        let generic_logins = store.generic_logins().unwrap_or_default();
        let proxy = crate::proxy::ProxyState::load(&store);
        let enterprise_oauth_apps = store.enterprise_oauth_apps().unwrap_or_default();

        // Synchronous: a few `git --version` probes (~10 ms), started on a
        // thread at the top of `main`. Avoids racing launch-time operations
        // against an async detection.
        // `516-git-executable`: a chosen git goes first
        let preferred = configured_git(
            flags.text(crate::flags::ids::GIT_EXECUTABLE),
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from),
        );
        let mut popups = crate::popup_manager::PopupManager::new();
        let (git, git_error) =
            match corvene_git::find_git_prefetched_preferring(preferred.as_deref()) {
                Ok(bin) => (Some(Arc::new(bin)), None),
                Err(err) => {
                    warn!(%err, "git not usable");
                    popups.add_popup(Popup::InstallGit {
                        reason: err.to_string(),
                    });
                    (None, Some(err.to_string()))
                }
            };
        // Corvene (`271-persist-repository-indicators`): last launch's
        // indicators until the first refresh
        let indicators = if flags.bool(crate::flags::ids::PERSIST_REPOSITORY_INDICATORS)
            && settings.repository_indicators_enabled
        {
            let mut saved = store.repository_indicators().unwrap_or_default();
            saved.retain(|id, _| repositories.iter().any(|r| r.id == *id && !r.missing));
            saved
        } else {
            std::collections::HashMap::new()
        };
        // Corvene (`766-persist-commit-drafts`): last session's drafts
        let commit_drafts = if flags.bool(crate::flags::ids::PERSIST_COMMIT_DRAFTS) {
            let mut saved = store.commit_drafts().unwrap_or_default();
            saved.retain(|id, _| repositories.iter().any(|r| r.id == *id));
            saved
        } else {
            std::collections::HashMap::new()
        };
        // Corvene (`767-persist-file-selection`): last session's unticked files
        let excluded_files = if flags.bool(crate::flags::ids::PERSIST_FILE_SELECTION) {
            let mut saved = store.excluded_files().unwrap_or_default();
            saved.retain(|id, _| repositories.iter().any(|r| r.id == *id));
            saved
        } else {
            std::collections::HashMap::new()
        };
        // Corvene (`1310-file-list-tree`): the changes trees' collapsed folders
        let collapsed_folders = {
            let mut saved = store.collapsed_folders().unwrap_or_default();
            saved.retain(|id, _| repositories.iter().any(|r| r.id == *id));
            saved
                .into_iter()
                .map(|(id, folders)| (id, std::sync::Arc::new(folders.into_iter().collect())))
                .collect()
        };
        // Corvene (`1313-changelists`): each repository's changelists
        let changelists = {
            let mut saved = store.changelists().unwrap_or_default();
            saved.retain(|id, _| repositories.iter().any(|r| r.id == *id));
            saved
                .into_iter()
                .map(|(id, lists)| (id, std::sync::Arc::new(lists)))
                .collect()
        };
        // `876-git-spawn-error-details`
        corvene_git::set_explain_missing_workdir(
            flags.bool(crate::flags::ids::GIT_SPAWN_ERROR_DETAILS),
        );
        // `789-non-utf8-diffs`
        corvene_git::text_encoding::set_decode_legacy_text(
            flags.bool(crate::flags::ids::NON_UTF8_DIFFS),
        );
        // `1306-utf16-diffs`
        corvene_git::utf16::set_decode_utf16(flags.bool(crate::flags::ids::UTF16_DIFFS));
        let hosts = crate::hosts::HostsState::load(&store);
        let (workspaces, next_workspace_id) =
            crate::workspace::restored_workspaces(&store, &flags, &repositories, selected, popups);
        let first_workspace = workspaces[0].id;
        cx.install_state(AppState {
            store,
            settings,
            flag_overrides,
            flags_env,
            flags: flags.clone(),
            flags_at_launch: flags,
            git,
            git_error,
            git_redetecting: false,
            repositories,
            recent,
            recent_worktrees,
            workspaces,
            current: first_workspace,
            focused: first_workspace,
            next_workspace_id,
            keymap_overrides: Default::default(),
            keymap_load_errors: Vec::new(),
            settings_overlay: Default::default(),
            settings_file_flags: Default::default(),
            repo_states: Default::default(),
            accounts,
            cloning: Default::default(),
            ahead_behind: Default::default(),
            pruner_generations: std::collections::HashMap::new(),
            shared_storage_move: None,
            pending_aliases: Vec::new(),
            pending_accounts: Vec::new(),
            account_probes: Default::default(),
            sign_in_store: SignInStore::new(sign_in_accounts.clone()),
            sign_in_accounts,
            authentication: None,
            adding_account: false,
            extra_oauth_scopes: Vec::new(),
            ssh_key: Default::default(),
            watchers: std::collections::HashMap::new(),
            watcher_generation: 0,
            indicators,
            generic_logins,
            proxy,
            enterprise_oauth_apps,
            avatars: std::collections::HashMap::new(),
            markdown_images: std::collections::HashMap::new(),
            api_repositories: std::collections::HashMap::new(),
            api_repositories_loading: std::collections::HashSet::new(),
            pull_requests: std::collections::HashMap::new(),
            tutorial_announced: false,
            tutorial_step_override: None,
            commit_statuses: crate::commit_status::CommitStatusStore::default(),
            job_logs: crate::job_log::JobLogStore::default(),
            actions: None,
            actions_watch: Default::default(),
            menu_bar_statuses: Default::default(),
            repo_rulesets: std::collections::HashMap::new(),
            issues: std::collections::HashMap::new(),
            mentionables: std::collections::HashMap::new(),
            editors: Vec::new(),
            shells: Vec::new(),
            app_icons: std::collections::HashMap::new(),
            global_git: None,
            repo_settings: None,
            pending_open_in_desktop: None,
            update: crate::updater::UpdateState::default(),
            packs: crate::packs::PacksState::default(),
            extensions: crate::extensions::ExtensionsState::default(),
            alive: crate::alive::AliveState::default(),
            commit_drafts,
            commit_drafts_nonce: 0,
            excluded_files,
            excluded_files_restored: std::collections::HashSet::new(),
            collapsed_folders,
            changelists,
            file_icon_theme: None,
            hosts,
        });
        let state = StateHandle;
        cx.background_executor()
            .spawn(async { corvene_highlight::prewarm() })
            .detach();
        Self::detect_integrations(cx);
        Self::refresh_hook_env(cx);

        for id in state.read(cx).visible_repositories() {
            Self::refresh_repository(id, cx);
            Self::start_background_pruner(id, cx);
            Self::start_watching(id, cx);
        }
        if let Some(banner) = unreadable_banner {
            Self::set_banner(banner, cx);
        }
        Self::sync_repository_list_file(cx);
        state
    }

    /// Corvene (`425-cli-list-repositories`): the repository list files
    /// follow the flag (written now, or removed).
    pub(crate) fn sync_repository_list_file(cx: &mut dyn Host) {
        let s = Self::state(cx).read(cx);
        let Some(dir) = s.store.path().parent() else {
            return;
        };
        crate::repository_list_file::set_enabled(
            s.flags.bool(crate::flags::ids::CLI_LIST_REPOSITORIES),
            dir,
            Some((&s.repositories, s.workspaces[0].selected)),
        );
    }

    /// Watch the repository's worktree; each debounced change triggers a
    /// refresh. `202-fs-watcher` turns this off (GHD only refreshes on focus
    /// and after its own actions); `904-fs-watcher-debounce-ms` is the wait.
    pub fn start_watching(id: u64, cx: &mut dyn Host) {
        let state = Self::state(cx);
        let (path, debounce, leading) = {
            let s = state.read(cx);
            if s.watchers.contains_key(&id) || !s.flags.bool(crate::flags::ids::FS_WATCHER) {
                return;
            }
            let Some(repo) = s.repository(id) else {
                return;
            };
            let debounce = s.flags.number(crate::flags::ids::FS_WATCHER_DEBOUNCE_MS);
            (
                repo.path.clone(),
                std::time::Duration::from_millis(debounce.max(0) as u64),
                s.flags.bool(crate::flags::ids::FS_WATCHER_LEADING_EDGE),
            )
        };
        match crate::watcher::watch(path.clone(), debounce, leading) {
            Ok((watcher, rx)) => {
                let generation = state.update(cx, |s, _| {
                    s.watcher_generation += 1;
                    s.watchers.insert(id, (s.watcher_generation, watcher));
                    s.watcher_generation
                });
                cx.spawn(async move |cx: &mut AsyncCtx| {
                    while let Ok(changed_at) = rx.recv().await {
                        let (still_watched, seen) = state.read_with(cx, |s, _| {
                            (
                                s.watchers.get(&id).is_some_and(|(g, _)| *g == generation),
                                // a refresh that started after the change
                                // (the one after Corvene's own git command)
                                // already saw it
                                s.repo_states
                                    .get(&id)
                                    .and_then(|rs| rs.refresh_started)
                                    .is_some_and(|started| started > changed_at),
                            )
                        });
                        if !still_watched {
                            break;
                        }
                        if !seen {
                            cx.update(|cx| Self::refresh_repository(id, cx));
                        }
                    }
                })
                .detach();
            }
            Err(err) => warn!(?err, path = %path.display(), "could not watch repository"),
        }
    }

    /// Drop the watchers of repositories no window shows any more.
    pub(crate) fn stop_unwatched(cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, _| {
            let visible = s.visible_repositories();
            s.watchers.retain(|id, _| visible.contains(id));
        });
    }

    /// Start looking for git on a thread; the first thing `main` does
    /// ([`corvene_git::prefetch_git`]).
    pub fn prefetch_git() {
        corvene_git::prefetch_git();
    }

    /// Window focus (GHD `focus` IPC). A refresh that started a moment ago
    /// already sees what focus would: at launch the window activates while
    /// the first refresh runs, which queued a second full refresh.
    pub fn refresh_selected(cx: &mut dyn Host) {
        let s = Self::state(cx).read(cx);
        let Some(id) = s.selected else { return };
        let fresh = s.repo_states.get(&id).is_some_and(|rs| {
            rs.loading
                && rs
                    .refresh_started
                    .is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(250))
        });
        if !fresh {
            Self::refresh_repository(id, cx);
        }
    }

    pub(crate) fn state(_cx: &dyn Host) -> StateHandle {
        StateHandle
    }

    /// Find git on a background thread, then refresh the selected repository.
    pub fn detect_git(cx: &mut dyn Host) {
        let task = cx.background_executor().spawn(async move { find_git() });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let result = task.await;
            cx.update(|cx| {
                let state = Self::state(cx);
                state.update(cx, |s, cx| {
                    match result {
                        Ok(bin) => {
                            s.git = Some(Arc::new(bin));
                            s.git_error = None;
                        }
                        Err(err) => {
                            warn!(%err, "git not usable");
                            s.git = None;
                            s.git_error = Some(err.to_string());
                            show_popup_in(
                                s,
                                Popup::InstallGit {
                                    reason: err.to_string(),
                                },
                            );
                        }
                    }
                    cx.notify();
                });
                if let Some(id) = state.read(cx).selected {
                    Self::refresh_repository(id, cx);
                }
            });
        })
        .detach();
    }

    /// Corvene: Apple's `/usr/bin/git` turns into the install stub when the
    /// Command Line Tools are removed while Corvene runs (a macOS update does
    /// it), failing every git command from then on. When the git in use is
    /// that stub ([`corvene_git::still_usable`]), look for git again and
    /// refresh; `failed` is the error that gave it away, shown again saying
    /// which git Corvene uses now. Returns whether it looked.
    pub fn replace_stale_git(failed: Option<(String, ErrorMessage)>, cx: &mut dyn Host) -> bool {
        let stale = {
            let s = Self::state(cx).read(cx);
            !s.git_redetecting
                && s.git
                    .as_ref()
                    .is_some_and(|git| !corvene_git::still_usable(git))
        };
        if !stale {
            return false;
        }
        Self::state(cx).update(cx, |s, _| s.git_redetecting = true);
        warn!("the Command Line Tools are gone; looking for git again");
        let task = cx.background_executor().spawn(async move { find_git() });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let result = task.await;
            cx.update(|cx| {
                let state = Self::state(cx);
                state.update(cx, |s, cx| {
                    s.git_redetecting = false;
                    match result {
                        Ok(bin) => {
                            info!(path = %bin.path.display(), "switched git");
                            if let Some((title, failed)) = failed {
                                show_popup_in(
                                    s,
                                    Popup::Error {
                                        title,
                                        message: format!(
                                            "Apple's Command Line Tools were removed while \
                                             Corvene was running, which broke the Git at \
                                             /usr/bin/git. Corvene now uses the Git at {}; \
                                             try again.",
                                            bin.path.display()
                                        ),
                                        git: failed.git,
                                    },
                                );
                            }
                            s.git = Some(Arc::new(bin));
                            s.git_error = None;
                        }
                        Err(err) => {
                            warn!(%err, "git not usable");
                            s.git = None;
                            s.git_error = Some(err.to_string());
                            show_popup_in(
                                s,
                                Popup::InstallGit {
                                    reason: err.to_string(),
                                },
                            );
                        }
                    }
                    cx.notify();
                });
                if let Some(id) = state.read(cx).selected {
                    Self::refresh_repository(id, cx);
                }
            });
        })
        .detach();
        true
    }

    // ---- foldouts / popups ----

    pub fn toggle_foldout(foldout: Foldout, cx: &mut dyn Host) {
        let opened = Self::state(cx).update(cx, |s, cx| {
            s.foldout = if s.foldout == Some(foldout) {
                None
            } else {
                Some(foldout)
            };
            s.show_ci_status_popover = false;
            cx.notify();
            s.foldout == Some(foldout)
        });
        // opening the branch list is a good moment to look for new pull requests
        if opened
            && foldout == Foldout::Branch
            && let Some(id) = Self::state(cx).read(cx).selected
        {
            Self::refresh_pull_requests(id, false, cx);
        }
        // `217-prompt-indicator-refresh`: the repository list shows fresh
        // indicators (GHD waits for the 15-minute updater)
        if opened
            && foldout == Foldout::Repository
            && Self::state(cx)
                .read(cx)
                .flags
                .bool(crate::flags::ids::PROMPT_INDICATOR_REFRESH)
        {
            Self::refresh_indicators_if_stale(cx);
        }
    }

    pub fn close_foldout(cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            if s.foldout.take().is_some() {
                cx.notify();
            }
        });
    }

    /// GHD `_showPopup`: put `popup` on the popup stack (see
    /// [`show_popup_in`]).
    pub fn show_popup(popup: Popup, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            // `345-issues`: a Create Branch dialog is for an issue only when
            // `create_branch_from_issue` says so right after this
            if let Popup::CreateBranch { repo, .. } = &popup
                && let Some(rs) = s.repo_states.get_mut(repo)
            {
                rs.pending_issue_link = None;
            }
            show_popup_in(s, popup);
            cx.notify();
        });
    }

    /// `265-remove-stale-index-lock`: the error dialog's "Remove Lock File":
    /// delete `lock` unless a git process is running in the repository.
    pub fn remove_index_lock(lock: PathBuf, cx: &mut dyn Host) {
        Self::close_popup(cx);
        let (selected, workdir) = {
            let s = Self::state(cx).read(cx);
            let repo = s.selected_repository();
            (
                repo.map(|r| r.id),
                repo.map(|r| r.path.clone())
                    // `<workdir>/.git/index.lock`
                    .or_else(|| lock.parent()?.parent().map(Path::to_path_buf)),
            )
        };
        let Some(workdir) = workdir else {
            return;
        };
        let task = cx
            .background_executor()
            .spawn(async move { corvene_git::remove_stale_index_lock(&lock, &workdir) });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let result = task.await;
            cx.update(|cx| {
                if let Err(err) = result {
                    Self::show_error("Could not remove the lock file", &err, cx);
                }
                if let Some(id) = selected {
                    Self::refresh_repository(id, cx);
                }
            });
        })
        .detach();
    }

    /// GHD `ConfigLockFileExists.onDeleteLockFile`
    /// (`ui/lib/config-lock-file-exists.tsx`), from the error of a Git
    /// configuration save: close the error and delete `lock` in the
    /// background (one already gone counts as deleted), so the save can be
    /// tried again; a failure shows its own error (GHD `postError`).
    pub fn delete_config_lock_file(lock: PathBuf, cx: &mut dyn Host) {
        Self::close_popup(cx);
        let task = cx
            .background_executor()
            .spawn(async move { corvene_git::delete_config_lock_file(&lock) });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            if let Err(err) = task.await {
                cx.update(|cx| {
                    Self::show_error("Could not delete the lock file", err.to_string(), cx)
                });
            }
        })
        .detach();
    }

    /// GHD `_closePopup()`: close the popup on top of the stack, the one
    /// shown; the one below it (if any) shows again.
    pub fn close_popup(cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            if let Some(popup) = s.popups.current_popup().cloned() {
                s.popups.remove_popup(popup.clone());
                closed_popup(s, &popup.popup);
                cx.notify();
            }
        });
    }

    /// GHD `_closePopup(popupType)`: when the popup shown is of the type
    /// `is` picks, close every popup of its type.
    pub fn close_popup_if(is: impl Fn(&Popup) -> bool, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            let Some(current) = s.popups.current_popup().cloned() else {
                return;
            };
            if !is(&current.popup) {
                return;
            }
            s.popups.remove_popup_by_type(current.popup_type());
            closed_popup(s, &current.popup);
            cx.notify();
        });
    }

    /// Close every popup `is` picks, wherever it is in the stack: what a
    /// GHD dialog's own `onDismissed` (`closePopupById`) does once its work
    /// is done, for callers that do not keep the popup's id.
    pub fn close_popups_where(is: impl Fn(&Popup) -> bool, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            let closing: Vec<crate::popup_manager::StackedPopup> = s
                .popups
                .all_popups()
                .iter()
                .filter(|p| is(&p.popup))
                .cloned()
                .collect();
            if closing.is_empty() {
                return;
            }
            for popup in closing {
                s.popups.remove_popup(popup.clone());
                closed_popup(s, &popup.popup);
            }
            cx.notify();
        });
    }

    /// GHD `_closePopupById`: close the popup with that stack id, wherever
    /// it is in the stack.
    pub fn close_popup_by_id(popup_id: u64, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            let Some(popup) = s
                .popups
                .all_popups()
                .iter()
                .find(|p| p.id == Some(popup_id))
                .cloned()
            else {
                return;
            };
            s.popups.remove_popup_by_id(popup_id);
            closed_popup(s, &popup.popup);
            cx.notify();
        });
    }

    /// The re-authorization prompts' "Sign in" / "Continue in browser": open
    /// the sign-in dialog and run `retry` in repository `id` once it succeeds.
    /// GHD `beginBrowserBasedSignIn(endpoint, resultCallback)` from the
    /// SAML and workflow prompts: the callback performs `retry` once the
    /// sign-in succeeds (`performRetry`).
    pub fn sign_in_then_retry(
        enterprise: bool,
        id: u64,
        retry: Option<RetryAction>,
        cx: &mut dyn Host,
    ) {
        let async_cx = cx.async_ctx();
        let mut retry = retry;
        let callback: ResultCallback = Box::new(move |result| {
            if let (SignInResult::Success { .. }, Some(retry)) = (&result, retry.take()) {
                // the store calls back in the middle of an `AppState` update
                async_cx
                    .spawn(async move |cx: &mut AsyncCtx| {
                        cx.update(|cx| Self::perform_retry(id, retry, cx));
                    })
                    .detach();
            }
        });
        Self::show_sign_in_dialog(enterprise, Some(callback), cx);
    }

    /// GHD `showDotComSignInDialog` / `showEnterpriseSignInDialog`: begin
    /// the sign-in store's flow and show the SignIn dialog.
    pub fn show_sign_in_dialog(
        enterprise: bool,
        result_callback: Option<ResultCallback>,
        cx: &mut dyn Host,
    ) {
        Self::state(cx).update(cx, |s, cx| {
            begin_sign_in_store(s, enterprise, result_callback);
            add_popup_in(s, Popup::SignIn { enterprise });
            cx.notify();
        });
    }

    /// Corvene (`527-multiple-accounts`): Settings › Accounts › Add
    /// account. The sign-in dialog for another account beside the signed-in
    /// ones; the browser shows GitHub's account picker, and the same
    /// account coming back again is refused with a hint.
    pub fn show_add_account_dialog(enterprise: bool, cx: &mut dyn Host) {
        Self::show_sign_in_dialog(enterprise, None, cx);
        Self::state(cx).update(cx, |s, _| s.adding_account = true);
    }

    /// GHD `setSignInEndpoint`: the Enterprise address step's Continue.
    pub fn set_sign_in_endpoint(url: String, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            s.sign_in_store.allow_plain_http =
                s.flags.bool(crate::flags::ids::ENTERPRISE_PLAIN_HTTP);
            s.sign_in_store.allow_multiple = s.multiple_accounts();
            sign_in_store(s).set_endpoint(&url);
            cx.notify();
        });
    }

    pub fn show_error(
        title: impl Into<String>,
        message: impl Into<ErrorMessage>,
        cx: &mut dyn Host,
    ) {
        let title = title.into();
        let message = message.into();
        let full = message.full_text();
        // Apple's git broke under us: switch git, then say so instead
        if matches!(
            corvene_git::environment_error(&full),
            Some(
                corvene_git::EnvironmentError::MissingCommandLineTools
                    | corvene_git::EnvironmentError::InvalidDeveloperPath
            )
        ) && Self::replace_stale_git(Some((title.clone(), message.clone())), cx)
        {
            return;
        }
        // any git call refused for an unsafe repository switches that
        // repository to the "Trust Repository" view instead
        if let Some(path) = corvene_git::dubious_ownership_path(&full)
            && Self::mark_unsafe_repository(path, cx)
        {
            return;
        }
        // Corvene (`265-remove-stale-index-lock`): a left-over index.lock can
        // be removed from the error (GHD shows git's words only)
        if Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::REMOVE_STALE_INDEX_LOCK)
            && let Some(lock) = corvene_git::index_lock_path(&full)
        {
            Self::show_popup(
                Popup::IndexLockExists {
                    title,
                    message: full,
                    lock,
                },
                cx,
            );
            return;
        }
        let ErrorMessage { text: message, git } = message;
        Self::show_popup(
            Popup::Error {
                title,
                message,
                git,
            },
            cx,
        );
    }

    /// The repository git named as unsafe (by path, else the selected one)
    /// shows the unsafe view; `false` when there is no such repository.
    fn mark_unsafe_repository(path: PathBuf, cx: &mut dyn Host) -> bool {
        Self::state(cx).update(cx, |s, cx| {
            let id = s
                .repositories
                .iter()
                .find(|r| same_path(&r.path, &path) || path.starts_with(&r.path))
                .map(|r| r.id)
                .or(s.selected);
            let Some(id) = id else {
                return false;
            };
            info!(id, path = %path.display(), "git considers the repository unsafe");
            s.repo_state_mut(id).unsafe_path = Some(path);
            if let Some(repo) = s.repositories.iter_mut().find(|r| r.id == id) {
                repo.missing = true;
            }
            cx.notify();
            true
        })
    }

    /// GHD `MissingRepository.onTrustDirectory`: `addSafeDirectory` for the
    /// path git named, then look at the repository again. Deviation
    /// (`279-explain-trust-failure`): when git still refuses the path, an
    /// error explains why and shows the value git suggests; GHD silently
    /// shows the Trust Repository view again.
    pub fn trust_repository(id: u64, cx: &mut dyn Host) {
        let state = Self::state(cx);
        let (git, path) = {
            let s = state.read(cx);
            (
                s.git.clone(),
                s.repo_states.get(&id).and_then(|rs| rs.unsafe_path.clone()),
            )
        };
        let (Some(git), Some(path)) = (git, path) else {
            return;
        };
        state.update(cx, |s, cx| {
            s.repo_state_mut(id).trusting_path = true;
            cx.notify();
        });
        let explain = state
            .read(cx)
            .flags
            .bool(crate::flags::ids::EXPLAIN_TRUST_FAILURE);
        crate::remote::spawn_bg(
            cx,
            move || {
                corvene_git::add_safe_directory(git.clone(), &path)?;
                // `explain-trust-failure`: git may still refuse the path
                // (network shares, WSL and UNC paths are compared by the
                // form git sees, not the one it printed)
                let still = if explain {
                    corvene_git::still_unsafe(git, &path)
                } else {
                    None
                };
                Ok::<_, corvene_git::GitError>(still.map(|suggested| (path, suggested)))
            },
            move |result, cx| {
                Self::state(cx).update(cx, |s, cx| {
                    s.repo_state_mut(id).trusting_path = false;
                    cx.notify();
                });
                match result {
                    Err(err) => Self::show_error("Could not trust the repository", &err, cx),
                    Ok(Some((path, suggested))) => Self::show_error(
                        "Could not trust the repository",
                        trust_failure_message(&path, suggested.as_deref()),
                        cx,
                    ),
                    Ok(None) => {}
                }
                Self::refresh_repository(id, cx);
            },
        );
    }

    /// GHD `_relocateRepository` (the missing view's "Locate…"): pick a
    /// directory and point the entry at the repository there
    /// (`RepositoriesStore::update_repository_path`). The entry stays missing
    /// until the refresh reads it; an unsafe repository then gets the "Trust
    /// Repository" view. The main worktree is resolved again from the new
    /// location (the recorded one belongs to the old one), by gitoxide, so
    /// also for an unsafe repository, where GHD clears it.
    pub fn relocate_repository(id: u64, cx: &mut dyn Host) {
        Self::pick_directory("Locate", cx, move |picked, cx| {
            let Some(picked) = picked else {
                return;
            };
            crate::remote::spawn_bg(
                cx,
                move || {
                    // `getRepositoryType`: a subdirectory resolves to its
                    // repository's top level; bare repositories are refused
                    let workdir = corvene_git::top_level_working_directory(&picked);
                    // GHD `findMainWorktreePath(topLevelWorkingDirectory)`
                    let main = workdir.as_deref().and_then(corvene_git::main_worktree_path);
                    (picked, workdir, main)
                },
                move |(picked, workdir, main), cx| {
                    let Some(workdir) = workdir else {
                        // GHD `getInvalidRepoPathsMessage` for one path
                        Self::show_error(
                            "Error",
                            format!("{} isn't a Git repository.", picked.display()),
                            cx,
                        );
                        return;
                    };
                    let changed = Self::state(cx).update(cx, |s, cx| {
                        let Some(repo) = s.repository(id).cloned() else {
                            return false;
                        };
                        info!(id, path = %workdir.display(), "relocated repository");
                        s.repositories_store().update_repository_path(
                            &repo,
                            &workdir,
                            main.as_deref(),
                            repo.missing,
                        );
                        let rs = s.repo_state_mut(id);
                        rs.unsafe_path = None;
                        rs.worktrees.clear();
                        // force the file watcher onto the new directory
                        s.watchers.remove(&id);
                        cx.notify();
                        true
                    });
                    if changed {
                        Self::refresh_repository(id, cx);
                        Self::start_watching(id, cx);
                    }
                },
            );
        });
    }

    /// GHD `_cloneAgain` (the missing view's "Clone Again"): clone the
    /// GitHub repository back into the entry's path; adding the finished
    /// clone selects the existing entry, whose refresh clears `missing`.
    pub fn clone_again(id: u64, cx: &mut dyn Host) {
        let Some((url, path)) = Self::state(cx).read(cx).repository(id).and_then(|r| {
            let gh = r.github.as_ref().filter(|gh| !gh.clone_url.is_empty())?;
            Some((gh.clone_url.clone(), r.path.clone()))
        }) else {
            return;
        };
        Self::clone_repository(url, path, None, cx);
    }

    // ---- repositories ----

    /// Validate `path` is a git repository (background), then add + select it.
    /// Existing entries for the same path are selected instead of duplicated.
    pub fn add_repository(path: PathBuf, cx: &mut dyn Host) {
        Self::add_repository_then(path, cx, |_, _| {});
    }

    /// [`Dispatcher::add_repository`], then `then` with the repository's id
    /// once it is added (or found) and selected.
    pub fn add_repository_then(
        path: PathBuf,
        cx: &mut dyn Host,
        then: impl FnOnce(u64, &mut dyn Host) + 'static,
    ) {
        // GHD stores `Path.resolve(path)`: absolute, `.`/`..` folded lexically
        let path = resolve_path(&path);
        let state = Self::state(cx);
        // `224-alias-when-adding`
        let alias = Self::take_pending_alias(&path, cx);
        // `527-multiple-accounts`
        let account = Self::take_pending_account(&path, cx);
        let then = move |id: u64, cx: &mut dyn Host| {
            if let Some(alias) = alias {
                Self::change_repository_alias(id, Some(alias), cx);
            }
            Self::settle_repository_account(id, account, cx);
            then(id, cx);
        };
        if let Some(existing) = state
            .read(cx)
            .repositories
            .iter()
            .find(|r| same_path(&r.path, &path))
        {
            let id = existing.id;
            Self::select_repository(id, cx);
            then(id, cx);
            return;
        }
        let git = state.read(cx).git.clone();
        // Corvene (`875-explain-bad-config`): a repository git cannot open
        // because of a broken config file names the file and line
        let explain = state
            .read(cx)
            .flags
            .bool(crate::flags::ids::EXPLAIN_BAD_CONFIG);
        // Corvene (`280-stale-core-worktree-hint`): a `core.worktree` that
        // points at a folder that is gone is named instead of adding a
        // repository that shows up missing
        let stale_worktree_hint = state
            .read(cx)
            .flags
            .bool(crate::flags::ids::STALE_CORE_WORKTREE_HINT);
        let probe = cx.background_executor().spawn(async move {
            // GHD `_addRepositories`: a subdirectory of a working directory
            // adds the repository (`getRepositoryType`'s
            // `topLevelWorkingDirectory`)
            let path = corvene_git::top_level_working_directory(&path).unwrap_or(path);
            #[cfg(target_os = "android")]
            if let Some(git) = git.clone() {
                android_prepare_repository(git, &path);
            }
            open_repository(&path)
                .map_err(|err| {
                    if explain
                        && !matches!(err, GitError::NotARepository(_))
                        && let Some(text) =
                            git.and_then(|git| corvene_git::explain_open_failure(git, &path))
                    {
                        GitError::Gix(text)
                    } else {
                        err
                    }
                })
                .and_then(|info| {
                    match corvene_git::explain_stale_worktree(&path, &info.workdir)
                        .filter(|_| stale_worktree_hint)
                    {
                        Some(text) => Err(GitError::Gix(text)),
                        None => Ok((path, info)),
                    }
                })
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let result = probe.await;
            cx.update(|cx| match result {
                Ok((path, info)) => {
                    let state = Self::state(cx);
                    let (id, added) = state.update(cx, |s, cx| {
                        // GHD `addRepository` hands back the entry already at
                        // that path (a subdirectory of a listed repository
                        // was picked)
                        if let Some(existing) = s.repositories_store().find_by_path(&info.workdir) {
                            return (existing.id, false);
                        }
                        let wiki_not_github = s.flags.bool(crate::flags::ids::WIKI_NOT_GITHUB);
                        // GHD `matchGitHubRepository`: the signed-in accounts'
                        // hosts, plus github.com when signed out
                        // (`331-github-without-account`)
                        let dotcom = s.flags.bool(crate::flags::ids::GITHUB_WITHOUT_ACCOUNT);
                        let hosts = corvene_models::github_hosts(&s.accounts, dotcom);
                        let github = info
                            .remote("origin")
                            .and_then(|r| github_from_remote(&r.url, &hosts))
                            .filter(|gh| !(wiki_not_github && gh.is_wiki()));
                        let mut repositories = s.repositories_store();
                        let repo = repositories.add_repository(&info.workdir);
                        if let Some(github) = github {
                            repositories.set_github_repository(&repo, github);
                        }
                        let id = repo.id;
                        let repo_state = s.repo_state_mut(id);
                        repo_state.info = Some(info);
                        repo_state.last_refresh = Some(Instant::now());
                        info!(id, path = %path.display(), "added repository");
                        cx.notify();
                        (id, true)
                    });
                    Self::select_repository(id, cx);
                    if added {
                        Self::apply_account_commit_email(id, cx);
                    }
                    then(id, cx);
                }
                Err(GitError::NotARepository(path)) => Self::show_error(
                    "Not a git repository",
                    format!(
                        "The directory {} does not appear to be a Git repository.",
                        path.display()
                    ),
                    cx,
                ),
                Err(err) => Self::show_error("Could not add repository", &err, cx),
            });
        })
        .detach();
    }

    pub fn select_repository(id: u64, cx: &mut dyn Host) {
        let state = Self::state(cx);
        let changed = state.update(cx, |s, cx| {
            if s.repository(id).is_none() {
                return false;
            }
            if s.selected != Some(id) {
                s.record_navigation();
            }
            // `430-repository-tabs`: the tab strip follows
            if s.flags.bool(crate::flags::ids::REPOSITORY_TABS) {
                s.select_tab(id);
            } else {
                s.selected = Some(id);
            }
            s.recent.retain(|r| *r != id);
            s.recent.insert(0, id);
            // `209-recent-repositories-count` shows up to that many; at
            // least GHD's 3 are kept (removing the selected repository
            // falls back to the most recent one)
            let shown =
                usize::try_from(s.flags.number(crate::flags::ids::RECENT_REPOSITORIES_COUNT))
                    .unwrap_or(RECENT_REPOSITORIES_LENGTH);
            s.recent.truncate(shown.max(RECENT_REPOSITORIES_LENGTH));
            s.record_recent_worktree(id);
            s.foldout = None;
            // flag `218-close-dialogs-on-repository-switch` (Corvene addition,
            // desktop/desktop#9847): a dialog bound to another repository
            // closes; an operation in progress (conflicts, a credentials
            // prompt) keeps its dialog
            if s.flags
                .bool(crate::flags::ids::CLOSE_DIALOGS_ON_REPOSITORY_SWITCH)
            {
                let other_repository: Vec<u64> = s
                    .popups
                    .all_popups()
                    .iter()
                    .filter(|p| {
                        p.popup.repository().is_some_and(|repo| repo != id)
                            && !matches!(
                                p.popup,
                                Popup::MultiCommitOperation { .. }
                                    | Popup::GenericGitAuthentication { .. }
                            )
                    })
                    .filter_map(|p| p.id)
                    .collect();
                for popup_id in other_repository {
                    s.popups.remove_popup_by_id(popup_id);
                }
            }
            crate::workspace::persist_workspaces(s);
            let _ = s.store.save_recent_repositories(&s.recent);
            cx.notify();
            true
        });
        if changed {
            Self::stop_unwatched(cx);
            Self::refresh_repository(id, cx);
            Self::start_background_pruner(id, cx);
            Self::start_watching(id, cx);
            Self::check_lfs(id, cx);
            Self::ensure_pull_requests(id, cx);
            Self::refresh_github_repository(id, cx);
            Self::hosted_repository_changed(id, cx);
            Self::refresh_autolinks(id, cx);
            Self::load_issue_trackers(id, cx);
            Self::resume_tutorial_on_other_repository(id, cx);
            Self::restart_pull_request_updater(cx);
        }
    }

    /// Corvene (`427-back-forward-navigation`): View › Back (`back`) or
    /// Forward, to the repository and section shown before (after).
    pub fn navigate(back: bool, cx: &mut dyn Host) {
        let state = Self::state(cx);
        let target = state.update(cx, |s, cx| {
            if !s.flags.bool(crate::flags::ids::BACK_FORWARD_NAVIGATION) {
                return None;
            }
            let current = s.navigation_entry();
            let repositories: Vec<u64> = s.repositories.iter().map(|r| r.id).collect();
            let target = s
                .navigation
                .step(back, current, |id| repositories.contains(&id));
            // the menu items' enabled state follows
            cx.notify();
            target
        });
        let Some(target) = target else {
            return;
        };
        state.update(cx, |s, _| s.navigation.replaying = true);
        if state.read(cx).selected != Some(target.repository) {
            Self::select_repository(target.repository, cx);
        }
        Self::show_section(target.repository, target.section, cx);
        state.update(cx, |s, _| s.navigation.replaying = false);
    }

    pub fn remove_repository(id: u64, cx: &mut dyn Host) {
        let state = Self::state(cx);
        let moved_to = state.update(cx, |s, cx| {
            s.repositories.retain(|r| r.id != id);
            s.recent.retain(|r| *r != id);
            s.forget_recent_worktrees(id, None);
            s.remove_repo_state(id);
            s.watchers.remove(&id);
            let fallback = s
                .recent
                .first()
                .copied()
                .or_else(|| s.repositories.first().map(|r| r.id));
            // every window showing it moves on (GHD
            // `updateRepositorySelectionAfterRepositoriesChanged`)
            let mut moved_to = Vec::new();
            for w in &mut s.workspaces {
                w.forget_repository(id);
                if w.selected == Some(id) {
                    w.selected = fallback;
                    if let Some(next) = fallback {
                        if !w.tabs.is_empty() && !w.tabs.contains(&next) {
                            w.tabs.push(next);
                        }
                        moved_to.push(next);
                    }
                }
            }
            persist_repositories(s);
            let _ = s.store.save_recent_repositories(&s.recent);
            crate::workspace::persist_workspaces(s);
            cx.notify();
            moved_to
        });
        for next in moved_to {
            Self::refresh_repository(next, cx);
            // selecting the next repository through `_selectRepository`
            // starts its branch pruner
            Self::start_background_pruner(next, cx);
            Self::start_watching(next, cx);
        }
        Self::restart_pull_request_updater(cx);
    }

    /// GHD `startBackgroundPruner` (`_selectRepositoryRefreshTasks`, which
    /// stops the previous repository's first): prune the repository's merged
    /// branches (`crate::branch_pruner`) once the refresh selecting it
    /// started has finished, then every `BACKGROUND_PRUNE_MINIMUM_INTERVAL`
    /// while it stays selected.
    pub(crate) fn start_background_pruner(id: u64, cx: &mut dyn Host) {
        let state = Self::state(cx);
        let (generation, loading) = state.update(cx, |s, _| {
            let generation = s.pruner_generations.entry(id).or_insert(0);
            *generation += 1;
            let generation = *generation;
            let rs = s.repo_state_mut(id);
            rs.prune_after_refresh = rs.loading;
            (generation, rs.loading)
        });
        if !loading {
            Self::prune_branches(id, cx);
        }
        cx.spawn(async move |cx: &mut AsyncCtx| {
            loop {
                cx.background_executor()
                    .timer(crate::branch_pruner::BACKGROUND_PRUNE_MINIMUM_INTERVAL)
                    .await;
                let current = state.read_with(cx, |s, _| {
                    s.pruner_generations.get(&id) == Some(&generation)
                        && s.visible_repositories().contains(&id)
                });
                if !current {
                    break;
                }
                cx.update(|cx| Self::prune_branches(id, cx));
            }
        })
        .detach();
    }

    /// GHD `BranchPruner.pruneLocalBranches` for a GitHub repository, at
    /// most once a day: the last prune date is stored first, the pruning
    /// runs on the background executor with the default branch and branches
    /// of the last refresh, and the repository is refreshed when it got to
    /// its end (GHD `onPruneCompleted`).
    fn prune_branches(id: u64, cx: &mut dyn Host) {
        let now = std::time::SystemTime::now();
        let prepared = Self::state(cx).update(cx, |s, _| {
            let git = s.git.clone()?;
            let repo = s.repository(id).filter(|r| !r.missing)?.clone();
            let github = repo.github.clone()?;
            let rs = s.repo_states.get(&id)?;
            let branches = rs.info.as_ref()?.branches.clone();
            let default_branch = rs.default_branch.clone();
            let last = crate::repositories_store::last_prune_date(&s.store, &github);
            if !crate::branch_pruner::is_due(last, now) {
                return None;
            }
            // GHD updates the last prune date first thing after checking it
            crate::repositories_store::update_last_prune_date(&s.store, &github, now);
            Some((git, repo, branches, default_branch, last))
        });
        let Some((git, repo, branches, default_branch, last)) = prepared else {
            return;
        };
        crate::remote::spawn_bg(
            cx,
            move || {
                crate::branch_pruner::BranchPruner::new(
                    git,
                    &repo,
                    &branches,
                    default_branch.as_deref(),
                    last,
                )
                .prune_local_branches(crate::branch_pruner::PruneOptions::DEFAULT, now)
            },
            move |run, cx| {
                if run.completed {
                    Self::refresh_repository(id, cx);
                }
            },
        );
    }

    /// GHD `_refreshRepository`: re-read tip/branches/remotes, ahead/behind
    /// and working-directory status; then reload the selected diff.
    pub fn refresh_repository(id: u64, cx: &mut dyn Host) {
        // the refresh runs again once a working git is found
        if Self::replace_stale_git(None, cx) {
            return;
        }
        let state = Self::state(cx);
        let (
            path,
            git,
            line_counts,
            status_options,
            recent_count,
            (other_stash, stash_list),
            track_branches,
            clone_counts_as_fetch,
            detect_rewrite,
            refresh_stale_index,
            shared_fetch_head,
            read_parent,
            read_message_hooks,
            read_implicit_upstream,
            read_signing,
            (read_sparse, read_lfs),
            (read_push_target, read_stacked),
        ) = {
            let s = state.read(cx);
            let Some(repo) = s.repository(id) else {
                return;
            };
            (
                repo.path.clone(),
                s.git.clone(),
                s.flags.bool(crate::flags::ids::CHANGES_LINE_COUNTS),
                corvene_git::StatusOptions {
                    respect_show_untracked_files: s
                        .flags
                        .bool(crate::flags::ids::RESPECT_SHOW_UNTRACKED_FILES),
                    ignore_submodules: match s.flags.text(crate::flags::ids::IGNORE_SUBMODULES) {
                        "dirty" => corvene_git::IgnoreSubmodules::Dirty,
                        "all" => corvene_git::IgnoreSubmodules::All,
                        _ => corvene_git::IgnoreSubmodules::AsConfigured,
                    },
                    in_process: s.flags.bool(crate::flags::ids::IN_PROCESS_STATUS),
                    lfs_conflicts_manual: s
                        .flags
                        .bool(crate::flags::ids::LFS_CONFLICTS_PICK_A_SIDE),
                    worktree_renames: s.flags.bool(crate::flags::ids::WORKTREE_RENAME_DETECTION),
                },
                // GHD `RecentBranchesLimit` is 5
                usize::try_from(s.flags.number(crate::flags::ids::RECENT_BRANCHES_COUNT))
                    .unwrap_or(5),
                (
                    s.flags.bool(crate::flags::ids::SHOW_LATEST_OTHER_STASH),
                    s.flags.bool(crate::flags::ids::STASH_LIST),
                ),
                s.flags.bool(crate::flags::ids::BRANCH_UPSTREAM_GONE)
                    || s.flags.bool(crate::flags::ids::BRANCH_LIST_AHEAD_BEHIND),
                s.flags.bool(crate::flags::ids::CLONE_COUNTS_AS_FETCH),
                s.flags
                    .bool(crate::flags::ids::FORCE_PUSH_AFTER_OUTSIDE_REWRITE),
                s.flags.bool(crate::flags::ids::REFRESH_STALE_INDEX),
                s.flags
                    .bool(crate::flags::ids::WORKTREE_SHARED_LAST_FETCHED),
                s.flags.bool(crate::flags::ids::UPDATE_FROM_PARENT_BRANCH),
                s.flags
                    .bool(crate::flags::ids::MESSAGE_RULES_DEFER_TO_HOOKS),
                s.flags
                    .bool(crate::flags::ids::IMPLICIT_UPSTREAM_PUSH_DEFAULT),
                s.flags.bool(crate::flags::ids::COMMIT_SIGNING),
                (
                    s.flags.bool(crate::flags::ids::SPARSE_CHECKOUT),
                    s.flags.bool(crate::flags::ids::LFS_LOCKS),
                ),
                (
                    s.flags.bool(crate::flags::ids::REMOTE_MANAGER),
                    s.flags.bool(crate::flags::ids::STACKED_BRANCH_REFS),
                ),
            )
        };
        // Corvene `1319-trivial-change-icon`
        let trivial_working = state
            .read(cx)
            .flags
            .bool(crate::flags::ids::TRIVIAL_CHANGE_ICON);
        // GHD `_refreshRepository`: a path that is gone may be a deleted
        // linked worktree; fall back to its main worktree before giving up
        if !path.exists() {
            // Corvene (`294-follow-moved-repositories`): the folder may have
            // been moved
            if !Self::follow_moved_repository(id, path.clone(), cx) {
                Self::recover_missing_worktree(id, path, cx);
            }
            return;
        }
        let started = Instant::now();
        // the status the new one is merged with off the main thread
        // (`changes_state::merge_changed_files`), and whether that merge
        // drops partial selections
        let starting = state.update(cx, |s, _| {
            let rs = s.repo_state_mut(id);
            if rs.loading {
                rs.refresh_pending = true;
                return None;
            }
            rs.loading = true;
            rs.refresh_started = Some(started);
            // no notify: a refresh that changes nothing re-renders nothing
            Some((rs.status.clone(), rs.clear_partial_state))
        });
        let Some((previous_status, merged_clearing)) = starting else {
            return;
        };
        let merged_from = previous_status.clone();
        // `708-changes-busy-indicator`: the spinner shows once a refresh has
        // taken this long
        if state
            .read(cx)
            .flags
            .bool(crate::flags::ids::CHANGES_BUSY_INDICATOR)
        {
            cx.spawn(async move |cx: &mut AsyncCtx| {
                cx.background_executor()
                    .timer(crate::state::BUSY_INDICATOR_DELAY)
                    .await;
                cx.update(|cx| {
                    Self::state(cx).update(cx, |s, cx| {
                        if s.repo_states.get(&id).is_some_and(|rs| rs.loading) {
                            cx.notify();
                        }
                    })
                });
            })
            .detach();
        }
        // the branch and worktrees are ready long before a big tree's
        // status: they are shown as soon as they are read
        let (early_tx, early_rx) = async_channel::bounded(1);
        let work = cx.background_executor().spawn(async move {
            let result = (|| {
                let Some(git) = git else {
                    return Ok::<_, GitError>((open_repository(&path)?, None, None, None));
                };
                // Everything runs at once, each git process on its own
                // thread: a refresh takes as long as its slowest part (`git
                // status` on a large tree) instead of the sum of about ten
                // processes. GHD runs these one after another.
                std::thread::scope(|scope| {
                    let path = path.as_path();
                    let previous = previous_status.as_deref();
                    let status = spawn_git(scope, &git, move |git| {
                        let started = Instant::now();
                        let status =
                            corvene_git::get_status_with(git.clone(), path, status_options)
                                // GHD `updateChangedFiles`' merge, here: it
                                // copies and sorts every file
                                .map(|status| {
                                    crate::changes_state::merge_changed_files(
                                        status,
                                        previous,
                                        merged_clearing,
                                    )
                                });
                        // `903-refresh-stale-index`: a slow status is most often
                        // one re-reading files whose stat data went stale
                        if refresh_stale_index
                            && status.is_ok()
                            && started.elapsed() > std::time::Duration::from_millis(500)
                        {
                            let path = path.to_path_buf();
                            std::thread::spawn(move || {
                                corvene_git::refresh_stale_index(git, &path)
                            });
                        }
                        status
                    });
                    let recent = spawn_git(scope, &git, move |git| {
                        corvene_git::recent_branches(git, path, recent_count).unwrap_or_default()
                    });
                    let stashes = spawn_git(scope, &git, move |git| {
                        corvene_git::get_stashes(git, path).unwrap_or_default()
                    });
                    let tracking = track_branches.then(|| {
                        spawn_git(scope, &git, move |git| {
                            corvene_git::branch_tracking(git, path).unwrap_or_default()
                        })
                    });
                    let pull_with_rebase = spawn_git(scope, &git, move |git| {
                        corvene_git::pull_with_rebase(git, path)
                    });
                    // `526-commit-signing`: git signs the commits made here
                    let signs_commits = read_signing.then(|| {
                        spawn_git(scope, &git, move |git| {
                            corvene_git::boolean_config_value(git, path, "commit.gpgsign", false)
                                .unwrap_or(false)
                        })
                    });
                    // `1112-sparse-checkout`: the Changes tab's banner
                    let sparse = read_sparse.then(|| {
                        spawn_git(scope, &git, move |git| {
                            corvene_git::sparse_checkout(git, path)
                                .ok()
                                .filter(|sparse| sparse.enabled)
                                .map(|sparse| crate::sparse_checkout::SparseSummary {
                                    cone: sparse.cone,
                                    patterns: sparse.patterns.len(),
                                })
                        })
                    });
                    // `1113-lfs-locks`: locks are only asked for in a
                    // repository that uses LFS
                    let uses_lfs = read_lfs.then(|| {
                        spawn_git(scope, &git, move |git| {
                            corvene_git::is_using_lfs_by_attributes(git, path)
                        })
                    });
                    // `340-message-rules-defer-to-hooks`
                    let message_hook = read_message_hooks.then(|| {
                        spawn_git(scope, &git, move |git| {
                            corvene_git::hook_exists(
                                git,
                                path,
                                &["prepare-commit-msg", "commit-msg"],
                            )
                        })
                    });
                    let worktrees = spawn_git(scope, &git, move |git| {
                        corvene_git::list_worktrees(git, path).unwrap_or_default()
                    });
                    let configured = spawn_git(scope, &git, corvene_git::configured_default_branch);
                    let info = open_repository(path)?;
                    let worktrees = join(worktrees);
                    let _ = early_tx.try_send((info.clone(), worktrees.clone()));
                    let remote = crate::git_store::default_remote_name(&info).map(str::to_string);
                    let head = remote.clone().map(|remote| {
                        let workdir = info.workdir.clone();
                        spawn_git(scope, &git, move |git| {
                            corvene_git::remote_head(git, &workdir, &remote)
                                .ok()
                                .flatten()
                        })
                    });
                    let last_local_commit = info.current_branch().and_then(|b| {
                        corvene_git::most_recent_local_commit(
                            &info.workdir,
                            &b.name,
                            b.upstream.as_deref(),
                        )
                        .ok()
                        .flatten()
                    });
                    let mut status = join(status)?;
                    if trivial_working {
                        status.trivial = corvene_git::trivial_change::classify_working(
                            &info.workdir,
                            &status.files,
                        );
                    }
                    // `status --branch` reports the same counts `rev-list
                    // --left-right --count branch...upstream` would
                    let ahead_behind = info
                        .current_branch()
                        .filter(|b| b.upstream.is_some())
                        .and(status.ahead_behind);
                    let line_stats = (line_counts && !status.files.is_empty()).then(|| {
                        corvene_git::working_directory_line_stats(
                            git.clone(),
                            &info.workdir,
                            &status,
                        )
                        .unwrap_or_default()
                    });
                    let rebase_snapshot = status
                        .rebase_internal_state
                        .is_some()
                        .then(|| corvene_git::rebase_snapshot(git.clone(), &info.workdir))
                        .flatten();
                    let cherry_pick_snapshot = status
                        .cherry_pick_head_found
                        .then(|| corvene_git::cherry_pick_snapshot(git.clone(), &info.workdir))
                        .flatten();
                    // GHD `getMergeConflictsTheirBranch`: the local branches
                    // at `MERGE_HEAD` (not for a squash merge)
                    let merge_head_branches = (status.merge_head_found && !status.squash_msg_found)
                        .then(|| {
                            corvene_git::get_branches_pointed_at(
                                git.clone(),
                                &info.workdir,
                                "MERGE_HEAD",
                            )
                            .ok()
                            .flatten()
                        })
                        .flatten();
                    // `257`: what a pull would bring in
                    let incoming_commits = info
                        .current_branch()
                        .and_then(|b| b.upstream.as_deref())
                        .filter(|_| ahead_behind.is_some_and(|ab| ab.behind > 0))
                        .and_then(|upstream| {
                            corvene_git::get_commits_in_range(
                                &info.workdir,
                                "HEAD",
                                upstream,
                                crate::state::INCOMING_COMMITS_LIMIT,
                            )
                            .ok()
                        })
                        .map(|commits| commits.into_iter().map(|c| c.summary).collect())
                        .unwrap_or_default();
                    // `260-force-push-after-outside-rewrite`
                    let upstream_rewritten = detect_rewrite
                        && ahead_behind.is_some_and(|ab| ab.ahead > 0 && ab.behind > 0)
                        && info.current_branch().is_some_and(|b| {
                            b.upstream.as_deref().is_some_and(|upstream| {
                                corvene_git::upstream_tip_in_reflog(
                                    git.clone(),
                                    &info.workdir,
                                    &b.name,
                                    upstream,
                                )
                            })
                        });
                    // `1103-implicit-upstream-push-default`: a branch without
                    // an upstream that `push.default=current` pushes to the
                    // same-named branch of the remote Corvene pushes to
                    let implicit_upstream = info
                        .current_branch()
                        .filter(|b| read_implicit_upstream && b.upstream.is_none())
                        .and_then(|b| {
                            let remote = remote.as_deref()?;
                            let push_remote = corvene_git::implicit_push_remote(
                                git.clone(),
                                &info.workdir,
                                &b.name,
                                remote,
                            )?;
                            if push_remote != remote {
                                return None;
                            }
                            let tracking = format!("refs/remotes/{remote}/{}", b.name);
                            info.branches.iter().find(|r| r.full_name == tracking)?;
                            let ab = corvene_git::symmetric_ahead_behind(
                                git.clone(),
                                &info.workdir,
                                &b.full_name,
                                &tracking,
                            )
                            .ok()
                            .flatten()?;
                            Some((format!("{remote}/{}", b.name), ab))
                        });
                    // `1109-remote-manager`: a push remote other than the
                    // upstream's
                    let push_target =
                        info.current_branch()
                            .filter(|_| read_push_target)
                            .and_then(|b| {
                                crate::remote_manager::read_push_target(
                                    git.clone(),
                                    &info.workdir,
                                    b,
                                    &info.remotes,
                                    &info.branches,
                                    remote.as_deref(),
                                )
                            });
                    // `1202-update-from-parent-branch`
                    let update_parent = info
                        .current_branch()
                        .filter(|_| read_parent)
                        .and_then(|b| {
                            corvene_git::branch_merge_base(git.clone(), &info.workdir, &b.name)
                                .filter(|parent| *parent != b.name)
                        })
                        .filter(|parent| info.branches.iter().any(|b| b.name == *parent));
                    let head = head.and_then(join);
                    let configured = join(configured);
                    let default_branch = corvene_git::find_default_branch(
                        &info.branches,
                        remote.as_deref(),
                        head.as_deref(),
                        &configured,
                    )
                    .map(|b| b.name.clone());
                    // `1221-stacked-branch-refs`: History's marks
                    let stacked_refs = read_stacked
                        .then(|| {
                            crate::stacked_refs::read(
                                &info.workdir,
                                &info,
                                default_branch.as_deref(),
                            )
                        })
                        .flatten()
                        .map(Arc::new);
                    let (mut stashes, stash_count) = join(stashes);
                    let current = info.current_branch().map(|b| b.name.clone());
                    let stashed_branches =
                        stashes.iter().filter_map(|s| s.branch.clone()).collect();
                    // Corvene (`797-stash-list`): every entry, for the list
                    let all_stashes = if stash_list {
                        stashes.clone()
                    } else {
                        Vec::new()
                    };
                    // `getLastDesktopStashEntryForBranch`
                    let desktop_stash = current.as_deref().and_then(|current| {
                        corvene_git::last_desktop_stash_entry_index(&stashes, current)
                    });
                    // Corvene (`728-show-latest-other-stash`): without one of
                    // its own, the branch shows the newest stash that no
                    // Desktop made (`git stash` on the command line)
                    let stash = desktop_stash
                        .or_else(|| {
                            other_stash
                                .then(|| stashes.iter().position(|s| s.branch.is_none()))
                                .flatten()
                        })
                        .map(|i| stashes.swap_remove(i));
                    let extras = RefreshExtras {
                        line_stats: line_stats.unwrap_or_default(),
                        // `852-branch-upstream-gone`, `853-branch-list-ahead-behind`
                        branch_tracking: tracking.map(join).unwrap_or_default(),
                        incoming_commits,
                        recent_branches: join(recent),
                        default_branch,
                        stacked_refs,
                        stash,
                        stash_count,
                        stashed_branches,
                        stashes: all_stashes,
                        rebase_snapshot,
                        cherry_pick_snapshot,
                        merge_head_branches,
                        // `253-clone-counts-as-fetch`: a clone writes no
                        // FETCH_HEAD, so GHD says "never fetched" until the
                        // first fetch
                        // `874-worktree-shared-last-fetched`: a linked
                        // worktree also counts the main FETCH_HEAD
                        last_fetched: corvene_git::last_fetched(&info.workdir, shared_fetch_head)
                            .or_else(|| {
                                clone_counts_as_fetch
                                    .then(|| corvene_git::cloned_at(&info.workdir))
                                    .flatten()
                            }),
                        pull_with_rebase: join(pull_with_rebase),
                        commit_message_hook: message_hook.is_some_and(join),
                        signs_commits: signs_commits.is_some_and(join),
                        sparse_checkout: sparse.and_then(join),
                        uses_lfs: uses_lfs.is_some_and(join),
                        implicit_upstream,
                        push_target,
                        worktrees,
                        upstream_rewritten,
                        update_parent,
                        last_local_commit: last_local_commit.map(|c| crate::state::LastCommit {
                            at: std::time::UNIX_EPOCH
                                + std::time::Duration::from_secs(c.author.seconds.max(0) as u64),
                            sha: c.sha,
                            summary: c.summary,
                        }),
                    };
                    // an unchanged status keeps the previous allocation, so
                    // the swap below is a pointer compare and views keep
                    // their caches (100,000 files take a while to compare
                    // and re-filter)
                    let status = match previous_status.as_ref() {
                        Some(previous) if **previous == status => previous.clone(),
                        _ => Arc::new(status),
                    };
                    Ok((info, ahead_behind, Some(status), Some(extras)))
                })
            })();
            // git refuses to run in an unsafe repository; gitoxide can still
            // read where its main worktree is (GHD `mainWorktreePath`)
            let unsafe_main = result
                .as_ref()
                .err()
                .and_then(GitError::unsafe_repository_path)
                .and_then(|_| corvene_git::main_worktree_path(&path));
            (result, unsafe_main)
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let Ok((mut info, worktrees)) = early_rx.recv().await else {
                return;
            };
            cx.update(|cx| {
                Self::state(cx).update(cx, |s, cx| {
                    let slash_remotes = s.flags.bool(crate::flags::ids::REMOTE_NAMES_WITH_SLASHES);
                    let rs = s.repo_state_mut(id);
                    if rs.refresh_started != Some(started) {
                        return;
                    }
                    if !slash_remotes {
                        forget_remote_names(&mut info);
                    }
                    let mut changed = set(&mut rs.info, Some(info));
                    changed |= set(&mut rs.worktrees, worktrees);
                    if changed {
                        cx.notify();
                    }
                })
            });
        })
        .detach();
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let (result, unsafe_main) = work.await;
            cx.update(|cx| {
                // a worktree switch abandons the running refresh (it read
                // the old directory) and starts its own
                let superseded = Self::state(cx)
                    .read(cx)
                    .repo_states
                    .get(&id)
                    .is_none_or(|rs| rs.refresh_started != Some(started));
                if superseded {
                    return;
                }
                let snapshots = result
                    .as_ref()
                    .ok()
                    .and_then(|(_, _, _, extras)| extras.as_ref())
                    .map(|e| {
                        (
                            e.rebase_snapshot.clone(),
                            e.cherry_pick_snapshot.clone(),
                            e.merge_head_branches.clone(),
                        )
                    });
                let selected_file = Self::state(cx).update(cx, |s, cx| {
                    let slash_remotes = s.flags.bool(crate::flags::ids::REMOTE_NAMES_WITH_SLASHES);
                    let exclude_untracked = s
                        .flags
                        .bool(crate::flags::ids::NEW_UNTRACKED_FILES_EXCLUDED);
                    // `767-persist-file-selection`: last session's unticked
                    // files, for this repository's first status
                    let restore_excluded = (!s.excluded_files_restored.contains(&id))
                        .then(|| s.excluded_files.get(&id).cloned())
                        .flatten();
                    let mut status_applied = false;
                    // `1313-changelists`: paths changed since the last status
                    // join the active list, so they are only looked for with one
                    let capture_new_paths = crate::changelists::enabled(s)
                        && s.changelists
                            .get(&id)
                            .is_some_and(|l| l.active_list().is_some());
                    let mut new_paths: Vec<String> = Vec::new();
                    let repo_state: &mut RepositoryState = s.repo_state_mut(id);
                    repo_state.loading = false;
                    repo_state.last_refresh = Some(Instant::now());
                    // the busy spinner may be showing (`708`)
                    let mut changed = repo_state
                        .refresh_started
                        .is_some_and(|t| t.elapsed() >= crate::state::BUSY_INDICATOR_DELAY);
                    let mut selected = None;
                    let mut main_worktree = None;
                    match result {
                        Ok((mut info, ahead_behind, status, extras)) => {
                            if !slash_remotes {
                                forget_remote_names(&mut info);
                            }
                            changed |= set(&mut repo_state.info, Some(info));
                            changed |= set(&mut repo_state.ahead_behind, ahead_behind);
                            changed |= set(&mut repo_state.error, None);
                            if let Some(extras) = extras {
                                changed |=
                                    set(&mut repo_state.line_stats, Arc::new(extras.line_stats));
                                changed |= set(
                                    &mut repo_state.branch_tracking,
                                    Arc::new(extras.branch_tracking),
                                );
                                changed |=
                                    set(&mut repo_state.recent_branches, extras.recent_branches);
                                changed |=
                                    set(&mut repo_state.default_branch, extras.default_branch);
                                changed |= set(&mut repo_state.stacked_refs, extras.stacked_refs);
                                changed |= set(&mut repo_state.stash, extras.stash);
                                changed |= set(&mut repo_state.stash_count, extras.stash_count);
                                changed |=
                                    set(&mut repo_state.stashed_branches, extras.stashed_branches);
                                changed |= set(&mut repo_state.stashes, extras.stashes);
                                changed |= set(&mut repo_state.last_fetched, extras.last_fetched);
                                changed |=
                                    set(&mut repo_state.pull_with_rebase, extras.pull_with_rebase);
                                changed |= set(
                                    &mut repo_state.commit_message_hook,
                                    extras.commit_message_hook,
                                );
                                changed |= set(&mut repo_state.signs_commits, extras.signs_commits);
                                changed |=
                                    set(&mut repo_state.sparse_checkout, extras.sparse_checkout);
                                changed |= set(&mut repo_state.uses_lfs, extras.uses_lfs);
                                changed |= set(
                                    &mut repo_state.implicit_upstream,
                                    extras.implicit_upstream,
                                );
                                changed |= set(&mut repo_state.push_target, extras.push_target);
                                changed |= set(&mut repo_state.worktrees, extras.worktrees);
                                changed |= set(
                                    &mut repo_state.upstream_rewritten,
                                    extras.upstream_rewritten,
                                );
                                // GHD `mostRecentLocalCommit`: the undo bar
                                // follows the branch's unpushed commits
                                changed |=
                                    set(&mut repo_state.last_commit, extras.last_local_commit);
                                changed |=
                                    set(&mut repo_state.incoming_commits, extras.incoming_commits);
                                changed |= set(&mut repo_state.update_parent, extras.update_parent);
                                // `mainWorktreePath` bookkeeping for the
                                // missing-worktree fallback (applied below)
                                main_worktree = repo_state
                                    .worktrees
                                    .iter()
                                    .find(|w| w.kind == corvene_models::WorktreeType::Main)
                                    .map(|w| w.path.clone());
                                // `797-stash-list`: an entry that is gone is no
                                // longer shown
                                if repo_state.viewed_stash.as_ref().is_some_and(|sha| {
                                    !repo_state.stashes.iter().any(|s| &s.sha == sha)
                                }) {
                                    changed |= set(&mut repo_state.viewed_stash, None);
                                }
                                let stash_sha = repo_state.shown_stash().map(|s| s.sha.clone());
                                if repo_state.stash_files_sha != stash_sha {
                                    changed |= set(&mut repo_state.stash_files, None);
                                    changed |= set(&mut repo_state.stash_files_sha, None);
                                }
                                if repo_state.shown_stash().is_none() {
                                    changed |= set(&mut repo_state.showing_stash, false);
                                    changed |= set(&mut repo_state.stash_diff, None);
                                }
                            }
                            if let Some(mut status) = status {
                                let same = repo_state
                                    .status
                                    .as_ref()
                                    .is_some_and(|s| Arc::ptr_eq(s, &status));
                                if exclude_untracked && !same {
                                    exclude_new_untracked(
                                        Arc::make_mut(&mut status),
                                        repo_state.status.as_deref(),
                                    );
                                }
                                if capture_new_paths
                                    && !same
                                    && let Some(previous) = repo_state.status.as_deref()
                                {
                                    let known: std::collections::HashSet<&str> =
                                        previous.files.iter().map(|f| f.path.as_str()).collect();
                                    new_paths = status
                                        .files
                                        .iter()
                                        .filter(|f| !known.contains(f.path.as_str()))
                                        .map(|f| f.path.clone())
                                        .collect();
                                }
                                // `767-persist-file-selection`
                                if let Some(excluded) = &restore_excluded {
                                    crate::drafts::apply_excluded(
                                        Arc::make_mut(&mut status),
                                        excluded,
                                    );
                                }
                                status_applied = true;
                                let conflict_state = crate::mco::derive_conflict_state(
                                    &status,
                                    repo_state.conflict_state.as_ref(),
                                );
                                // GHD `updateChangedFiles`: selections carried
                                // over, sorted, the selection and diff kept
                                // while their files are still changed
                                let clear_partial_state =
                                    std::mem::take(&mut repo_state.clear_partial_state);
                                changed |= crate::changes_state::apply_merged_files(
                                    repo_state,
                                    status,
                                    merged_from.as_ref(),
                                    merged_clearing,
                                    clear_partial_state,
                                );
                                selected = repo_state.selected_file.clone();
                                changed |= set(&mut repo_state.conflict_state, conflict_state);
                            }
                            changed |= set(&mut repo_state.unsafe_path, None);
                            if let Some(repo) = s.repositories.iter_mut().find(|r| r.id == id) {
                                changed |= set(&mut repo.missing, false);
                            }
                        }
                        // GHD `getRepositoryType` → `unsafe`: the repository is
                        // shown as missing with the "Trust Repository" view
                        Err(err) if err.unsafe_repository_path().is_some() => {
                            changed = true;
                            let unsafe_path = err.unsafe_repository_path();
                            info!(id, path = ?unsafe_path, "git considers the repository unsafe");
                            repo_state.unsafe_path = unsafe_path;
                            repo_state.error = Some(err.to_string());
                            if let Some(repo) = s.repositories.iter_mut().find(|r| r.id == id) {
                                repo.missing = true;
                            }
                            main_worktree = unsafe_main;
                        }
                        Err(GitError::NotARepository(_)) => {
                            changed = true;
                            repo_state.error = Some("repository is missing".into());
                            if let Some(repo) = s.repositories.iter_mut().find(|r| r.id == id) {
                                repo.missing = true;
                            }
                        }
                        Err(err) => {
                            changed = true;
                            warn!(id, %err, "refresh failed");
                            repo_state.error = Some(err.to_string());
                        }
                    }
                    if status_applied {
                        s.excluded_files_restored.insert(id);
                        crate::drafts::note_excluded(s, id, cx);
                        Self::capture_new_changelist_paths(s, id, new_paths, cx);
                    }
                    if let Some(main) = main_worktree
                        && let Some(repo) = s.repositories.iter_mut().find(|r| r.id == id)
                        && repo.main_worktree_path.as_ref() != Some(&main)
                    {
                        repo.main_worktree_path = Some(main);
                        persist_repositories(s);
                        changed = true;
                    }
                    // GHD re-renders after every refresh; an unchanged
                    // repository (most focus and watcher refreshes) needs no
                    // frame at all
                    if changed {
                        cx.notify();
                    }
                    selected
                });
                if selected_file.is_some() {
                    Self::load_diff(id, cx);
                }
                // `1311-stacked-diff`: the stacked files' diffs too
                Self::refresh_stacked_working(id, cx);
                // GHD `GitStore.loadFilesForCurrentStashEntry`, run with every
                // stash entry load (the no-changes "View stash" card counts them)
                Self::load_stash_files(id, cx);
                Self::load_whitespace_only_files(id, cx);
                if let Some((rebase_snapshot, cherry_pick_snapshot, merge_head_branches)) =
                    snapshots
                {
                    Self::sync_conflicts(
                        id,
                        rebase_snapshot,
                        cherry_pick_snapshot,
                        merge_head_branches,
                        cx,
                    );
                }
                Self::load_commits(id, false, cx);
                Self::refresh_compare(id, cx);
                // `294-follow-moved-repositories`
                Self::remember_repository_location(id, cx);
                Self::subscribe_current_pull_request_status(id, cx);
                // flags 342-344: a remote on GitLab, Gitea or Bitbucket
                // (both throttled)
                Self::hosted_repository_changed(id, cx);
                Self::add_upstream_remote_if_needed(id, cx);
                // `353-fork-tracks-upstream`
                let track_parent = Self::state(cx).update(cx, |s, _| {
                    let rs = s.repo_state_mut(id);
                    // a refresh started before the remote was added waits
                    let has_upstream = rs.info.as_ref().is_some_and(|i| {
                        i.remotes
                            .iter()
                            .any(|r| r.name == crate::forks::UPSTREAM_REMOTE_NAME)
                    });
                    has_upstream && std::mem::take(&mut rs.track_parent_after_refresh)
                });
                if track_parent {
                    Self::track_parent_default_branch(id, true, cx);
                }
                Self::refresh_branch_protection(id, cx);
                // `1113-lfs-locks` (throttled)
                Self::refresh_lfs_locks(id, false, cx);
                // `1314-code-owners`
                Self::refresh_code_owners(id, cx);
                let (rerun, prune) = Self::state(cx).update(cx, |s, _| {
                    let rs = s.repo_state_mut(id);
                    (
                        std::mem::take(&mut rs.refresh_pending),
                        std::mem::take(&mut rs.prune_after_refresh),
                    )
                });
                if prune {
                    Self::prune_branches(id, cx);
                }
                if rerun {
                    Self::refresh_repository(id, cx);
                }
            });
        })
        .detach();
    }

    // ---- changes list ----

    /// GHD `_changeChangesSelection`: select a file and load its diff.
    pub fn select_file(id: u64, path: String, cx: &mut dyn Host) {
        let changed = Self::state(cx).update(cx, |s, cx| {
            let rs = s.repo_state_mut(id);
            // GHD `_selectWorkingDirectoryFiles`: picking a file leaves the stash
            let left_stash = rs.leave_stash();
            let same_anchor = rs.selected_file.as_deref() == Some(path.as_str());
            let single = rs.selected_files.len() == 1 && rs.selected_files[0] == path;
            if same_anchor && single {
                if left_stash {
                    cx.notify();
                }
                return false;
            }
            rs.selected_files = vec![path.clone()];
            rs.selected_file = Some(path);
            if !same_anchor {
                rs.diff = None;
            }
            cx.notify();
            !same_anchor
        });
        if changed {
            // `798-blame`: another file was picked
            Self::close_blame(id, cx);
            // `1110-repository-insights`: so does Insights
            Self::close_insights(id, cx);
            Self::load_diff(id, cx);
        }
    }

    /// ⌘-click (`SelectionSource` toggle): add or remove one path.
    pub fn toggle_file_selection(id: u64, path: String, cx: &mut dyn Host) {
        let changed = Self::state(cx).update(cx, |s, cx| {
            let rs = s.repo_state_mut(id);
            rs.leave_stash();
            let before = rs.selected_file.clone();
            if let Some(pos) = rs.selected_files.iter().position(|p| *p == path) {
                rs.selected_files.remove(pos);
                if rs.selected_file.as_deref() == Some(path.as_str()) {
                    rs.selected_file = rs.selected_files.last().cloned();
                }
            } else {
                rs.selected_files.push(path.clone());
                rs.selected_file = Some(path);
            }
            if rs.selected_file != before {
                rs.diff = None;
            }
            cx.notify();
            rs.selected_file != before
        });
        if changed {
            Self::load_diff(id, cx);
        }
    }

    /// ⇧-click: select the visible range between the anchor and `path`.
    pub fn extend_file_selection(id: u64, path: String, order: Vec<String>, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            // `707-shift-click-keeps-selection`
            let keep = s.flags.bool(crate::flags::ids::SHIFT_CLICK_KEEPS_SELECTION);
            let rs = s.repo_state_mut(id);
            if rs.leave_stash() {
                cx.notify();
            }
            let anchor = rs.selected_file.clone().unwrap_or_else(|| path.clone());
            if keep {
                if let Some(selection) = crate::list_selection::extend_keeping(
                    &order,
                    &anchor,
                    &rs.selected_files,
                    &path,
                ) {
                    rs.selected_files = selection;
                    if rs.selected_file.is_none() {
                        rs.selected_file = Some(path);
                    }
                    cx.notify();
                }
                return;
            }
            let (Some(a), Some(b)) = (
                order.iter().position(|p| *p == anchor),
                order.iter().position(|p| *p == path),
            ) else {
                return;
            };
            // ordered anchor → clicked row, so ⇧-arrows continue from the click
            rs.selected_files = crate::list_selection::selection_between(&order, a, b);
            if rs.selected_file.is_none() {
                rs.selected_file = Some(path);
            }
            cx.notify();
        });
    }

    /// ⇧↑ / ⇧↓ (GHD `List.addSelection`): grow or shrink the range between the
    /// anchor and its moving end by one visible row. With no anchor it moves
    /// the plain selection like an unmodified arrow key.
    pub fn extend_file_selection_by(id: u64, delta: isize, order: Vec<String>, cx: &mut dyn Host) {
        let first = Self::state(cx).update(cx, |s, cx| {
            let rs = s.repo_state_mut(id);
            if rs.leave_stash() {
                cx.notify();
            }
            let anchor = rs
                .selected_file
                .clone()
                .filter(|p| order.contains(p))
                .or_else(|| rs.selected_files.first().cloned())
                .filter(|p| order.contains(p));
            let Some(anchor) = anchor else {
                return order.first().cloned();
            };
            if let Some(range) =
                crate::list_selection::extend_selection(&order, &anchor, &rs.selected_files, delta)
            {
                rs.selected_files = range;
                cx.notify();
            }
            None
        });
        if let Some(path) = first {
            Self::select_file(id, path, cx);
        }
    }

    /// ⌘A in the list: every visible file.
    pub fn select_all_files(id: u64, order: Vec<String>, cx: &mut dyn Host) {
        let changed = Self::state(cx).update(cx, |s, cx| {
            let rs = s.repo_state_mut(id);
            if order.is_empty() {
                return false;
            }
            rs.leave_stash();
            let before = rs.selected_file.clone();
            if !rs.selected_file.as_ref().is_some_and(|p| order.contains(p)) {
                rs.selected_file = order.first().cloned();
            }
            rs.selected_files = order;
            cx.notify();
            rs.selected_file != before
        });
        if changed {
            Self::load_diff(id, cx);
        }
    }

    /// `749-binary-diff-as-text`: "Show diff anyway" on a binary file
    /// reloads its diff with `git diff --text`.
    /// `794-svg-image-diff`: whether `path`'s diff shows as images.
    pub fn svg_shown_as_image(s: &AppState, rs: &RepositoryState, path: &str) -> bool {
        corvene_git::is_svg(path)
            && rs.svg_as_image.contains(path)
            && s.flags.bool(crate::flags::ids::SVG_IMAGE_DIFF)
    }

    /// `794-svg-image-diff`: show `path` (an SVG file) as images, or as text
    /// again, in Changes and History.
    pub fn set_svg_as_image(id: u64, path: String, as_image: bool, cx: &mut dyn Host) {
        let (working, commit) = Self::state(cx).update(cx, |s, cx| {
            let rs = s.repo_state_mut(id);
            let changed = if as_image {
                rs.svg_as_image.insert(path.clone())
            } else {
                rs.svg_as_image.remove(&path)
            };
            cx.notify();
            (
                changed && rs.selected_file.as_deref() == Some(path.as_str()),
                changed && rs.commit_selected_file.as_deref() == Some(path.as_str()),
            )
        });
        if working {
            Self::load_diff(id, cx);
        }
        if commit {
            Self::load_commit_diff(id, cx);
        }
        // `1311-stacked-diff`: the file's entry in a stack
        Self::reload_stacked_diff(id, &path, cx);
    }

    pub fn show_binary_diff_as_text(id: u64, cx: &mut dyn Host) {
        let path = Self::state(cx).update(cx, |s, _| {
            let rs = s.repo_state_mut(id);
            rs.diff_as_text = rs.selected_file.clone();
            rs.diff_as_text.clone()
        });
        Self::load_diff(id, cx);
        if let Some(path) = path {
            Self::reload_stacked_diff(id, &path, cx);
        }
    }

    /// `749-binary-diff-as-text` for a file of a stack (`1311-stacked-diff`):
    /// the file need not be the selected one.
    pub fn show_binary_diff_as_text_for(id: u64, path: String, cx: &mut dyn Host) {
        let selected = Self::state(cx).update(cx, |s, _| {
            let rs = s.repo_state_mut(id);
            rs.diff_as_text = Some(path.clone());
            rs.selected_file.as_deref() == Some(path.as_str())
        });
        if selected {
            Self::load_diff(id, cx);
        }
        Self::reload_stacked_diff(id, &path, cx);
    }

    pub fn load_diff(id: u64, cx: &mut dyn Host) {
        Self::load_file_modified(id, cx);
        let state = Self::state(cx);
        let (git, workdir, file, options, head) = {
            let s = state.read(cx);
            let Some(git) = s.git.clone() else { return };
            let Some(rs) = s.repo_states.get(&id) else {
                return;
            };
            let Some(info) = rs.info.as_ref() else { return };
            let Some(path) = rs.selected_file.as_ref() else {
                return;
            };
            let Some(status) = rs.status.as_deref() else {
                return;
            };
            let Some(file) = status.files.iter().find(|f| &f.path == path).cloned() else {
                return;
            };
            (
                git,
                info.workdir.clone(),
                file,
                WorkingDiffOptions::of(s, rs, path),
                status.current_tip.clone(),
            )
        };
        let path = file.path.clone();
        let stamp =
            crate::diff_cache::working_stamp(&workdir, &file, head.as_deref(), options.key());
        // a diff computed from the same file, status and HEAD: shown in this
        // same frame, no git process
        if let Some(loaded) = stamp
            .as_ref()
            .and_then(|stamp| crate::diff_cache::working_diff(&workdir, &path, stamp))
        {
            // `764-cancel-stale-diffs`
            if let Some(previous) = state.update(cx, |s, _| s.repo_state_mut(id).diff_cancel.take())
            {
                previous.cancel();
            }
            Self::apply_working_diff(id, &path, loaded, cx);
            Self::prefetch_working_diffs(id, cx);
            return;
        }
        // `764-cancel-stale-diffs`: the diff still running for the previous
        // selection (or refresh) is stopped instead of finishing unseen
        let cancel_stale = state
            .read(cx)
            .flags
            .bool(crate::flags::ids::CANCEL_STALE_DIFFS);
        let cancel = cancel_stale.then(corvene_git::CancelToken::new);
        state.update(cx, |s, cx| {
            let rs = s.repo_state_mut(id);
            rs.diff_loading = true;
            if let Some(previous) = std::mem::replace(&mut rs.diff_cancel, cancel.clone()) {
                previous.cancel();
            }
            cx.notify();
        });
        let work = cx.background_executor().spawn(async move {
            let loaded = compute_working_diff(git, &workdir, &file, options, cancel.as_ref());
            // a stopped diff is incomplete: neither cached nor shown
            if cancel
                .as_ref()
                .is_some_and(corvene_git::CancelToken::is_cancelled)
            {
                return None;
            }
            if let Some(stamp) = stamp {
                crate::diff_cache::store_working_diff(&workdir, &file.path, stamp, loaded.clone());
            }
            Some(loaded)
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let Some(loaded) = work.await else { return };
            cx.update(|cx| {
                Self::apply_working_diff(id, &path, loaded, cx);
                Self::prefetch_working_diffs(id, cx);
            });
        })
        .detach();
    }

    /// Show a loaded working-directory diff of `path` (dropped when another
    /// file was selected meanwhile).
    fn apply_working_diff(id: u64, path: &str, loaded: LoadedDiff, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            let follow_lines = s.flags.bool(crate::flags::ids::SELECTION_FOLLOWS_LINES);
            let rs = s.repo_state_mut(id);
            // ignore stale results
            if rs.selected_file.as_deref() != Some(path) {
                return;
            }
            let mut changed = set(&mut rs.diff_loading, false);
            if replace_diff(
                (
                    &mut rs.diff,
                    &mut rs.diff_contents,
                    &mut rs.diff_old_contents,
                ),
                loaded,
            ) {
                rs.diff_generation += 1;
                changed = true;
            }
            // GHD `updateChangesWorkingDirectoryDiff`: bound the file's
            // selection to the lines that exist in this diff.
            let selectable: std::collections::BTreeSet<u32> = match rs.diff.as_deref() {
                Some(
                    corvene_models::Diff::Text { hunks, .. }
                    | corvene_models::Diff::LargeText { hunks, .. },
                ) => hunks
                    .iter()
                    .flat_map(|h| {
                        h.lines.iter().enumerate().filter_map(move |(i, l)| {
                            matches!(
                                l.kind,
                                corvene_models::DiffLineKind::Add
                                    | corvene_models::DiffLineKind::Delete
                            )
                            .then_some(h.unified_diff_start + i as u32)
                        })
                    })
                    .collect(),
                _ => Default::default(),
            };
            // Corvene (`790-selection-follows-lines`): a partial selection
            // made on another diff of the file moves with its lines
            let new_hunks = rs.diff.as_deref().and_then(|d| d.hunks());
            let basis = rs.selection_bases.get(path).cloned();
            // `794-svg-image-diff`: the image view of a text file keeps the
            // line selection made on its text
            let svg_image = corvene_git::is_svg(path)
                && matches!(rs.diff.as_deref(), Some(corvene_models::Diff::Image { .. }));
            // `796-lfs-text-diff`: lines of the contents, not of the pointers
            // the selection is staged against
            let svg_image = svg_image
                || rs
                    .diff
                    .as_deref()
                    .and_then(|d| d.warnings())
                    .is_some_and(|w| w.lfs_contents);
            // usually unchanged: only then copy the shared status
            let update = rs.status.as_deref().filter(|_| !svg_image).and_then(|st| {
                let i = st.files.iter().position(|f| f.path == path)?;
                let current = &st.files[i].selection;
                let carried = match (follow_lines, basis.as_deref(), new_hunks) {
                    (true, Some(basis), Some(new_hunks))
                        if current.kind() == DiffSelectionType::Partial =>
                    {
                        basis
                            .hunks()
                            .map(|old| crate::line_selection::carry_over(current, old, new_hunks))
                    }
                    _ => None,
                };
                let selection =
                    carried.unwrap_or_else(|| current.with_selectable_lines(selectable));
                (selection != *current).then_some((i, selection))
            });
            if let Some((i, selection)) = update
                && let Some(st) = rs.status.as_mut().map(Arc::make_mut)
            {
                st.files[i].selection = selection;
                changed = true;
            }
            if follow_lines {
                let partial = rs.status.as_deref().is_some_and(|st| {
                    st.files
                        .iter()
                        .any(|f| f.path == path && f.selection.kind() == DiffSelectionType::Partial)
                });
                match rs.diff.clone() {
                    Some(diff) if partial => {
                        rs.selection_bases.insert(path.to_string(), diff);
                    }
                    _ => {
                        rs.selection_bases.remove(path);
                    }
                }
            }
            // the same diff again (a refresh): nothing to draw
            if changed {
                cx.notify();
            }
        });
        Self::load_diff_tool(id, cx);
    }

    /// `763-diff-header-mtime`: the selected file's modification time for
    /// the Changes diff header (re-read whenever its diff is loaded).
    fn load_file_modified(id: u64, cx: &mut dyn Host) {
        let s = Self::state(cx).read(cx);
        if !s.flags.bool(crate::flags::ids::DIFF_HEADER_MTIME) {
            return;
        }
        let Some(rs) = s.repo_states.get(&id) else {
            return;
        };
        let (Some(info), Some(path)) = (rs.info.as_ref(), rs.selected_file.clone()) else {
            return;
        };
        let full = info.workdir.join(&path);
        crate::remote::spawn_bg(
            cx,
            move || {
                std::fs::symlink_metadata(full)
                    .and_then(|m| m.modified())
                    .ok()
            },
            move |modified, cx| {
                Self::state(cx).update(cx, |s, cx| {
                    let rs = s.repo_state_mut(id);
                    let next = modified.map(|at| (path, at));
                    if rs.diff_file_modified != next {
                        rs.diff_file_modified = next;
                        cx.notify();
                    }
                });
            },
        );
    }

    /// `762-too-large-diff-escape-hatch`: read `diff.tool` once the selected
    /// file's diff is too large to show, for "Open in External Diff Tool".
    fn load_diff_tool(id: u64, cx: &mut dyn Host) {
        let s = Self::state(cx).read(cx);
        let Some(rs) = s.repo_states.get(&id) else {
            return;
        };
        if !matches!(rs.diff.as_deref(), Some(corvene_models::Diff::TooLarge))
            || !s.flags.bool(crate::flags::ids::TOO_LARGE_DIFF_ESCAPE_HATCH)
        {
            return;
        }
        let (Some(git), Some(info)) = (s.git.clone(), rs.info.as_ref()) else {
            return;
        };
        let workdir = info.workdir.clone();
        crate::remote::spawn_bg(
            cx,
            move || corvene_git::config_value(git, &workdir, "diff.tool"),
            move |tool, cx| {
                Self::state(cx).update(cx, |s, cx| {
                    let rs = s.repo_state_mut(id);
                    if rs.diff_tool != tool {
                        rs.diff_tool = tool;
                        cx.notify();
                    }
                });
            },
        );
    }

    /// `762-too-large-diff-escape-hatch`: the selected file in the configured
    /// `diff.tool` (`git difftool -y`).
    pub fn open_in_diff_tool(id: u64, cx: &mut dyn Host) {
        let s = Self::state(cx).read(cx);
        let Some(git) = s.git.clone() else { return };
        let Some(rs) = s.repo_states.get(&id) else {
            return;
        };
        let (Some(info), Some(path), Some(status)) = (
            rs.info.as_ref(),
            rs.selected_file.as_ref(),
            rs.status.as_ref(),
        ) else {
            return;
        };
        let Some(file) = status.files.iter().find(|f| &f.path == path).cloned() else {
            return;
        };
        let workdir = info.workdir.clone();
        crate::remote::spawn_bg(
            cx,
            move || corvene_git::open_difftool(git, &workdir, &file),
            |result, cx| {
                if let Err(err) = result {
                    Self::show_error("Could not open the diff tool", err.to_string(), cx);
                }
            },
        );
    }

    /// `901-prefetch-diffs`: compute the diffs of the files next to the
    /// selected one in the background, so moving the selection finds them
    /// in [`crate::diff_cache`].
    fn prefetch_working_diffs(id: u64, cx: &mut dyn Host) {
        let s = Self::state(cx).read(cx);
        if !s.flags.bool(crate::flags::ids::PREFETCH_DIFFS) {
            return;
        }
        let (Some(git), Some(rs)) = (s.git.clone(), s.repo_states.get(&id)) else {
            return;
        };
        let (Some(info), Some(status), Some(selected)) = (
            rs.info.as_ref(),
            rs.status.as_deref(),
            rs.selected_file.as_ref(),
        ) else {
            return;
        };
        let Some(ix) = status.files.iter().position(|f| &f.path == selected) else {
            return;
        };
        let workdir = info.workdir.clone();
        let head = status.current_tip.clone();
        // nearest first: next, previous, then two away
        let neighbours: Vec<(WorkingDirectoryFileChange, WorkingDiffOptions)> = [1isize, -1, 2, -2]
            .into_iter()
            .filter_map(|d| status.files.get(ix.checked_add_signed(d)?))
            .map(|f| (f.clone(), WorkingDiffOptions::of(s, rs, &f.path)))
            .collect();
        cx.background_executor()
            .spawn(async move {
                for (file, options) in neighbours {
                    let Some(stamp) = crate::diff_cache::working_stamp(
                        &workdir,
                        &file,
                        head.as_deref(),
                        options.key(),
                    ) else {
                        continue;
                    };
                    if crate::diff_cache::working_diff(&workdir, &file.path, &stamp).is_some() {
                        continue;
                    }
                    let loaded = compute_working_diff(git.clone(), &workdir, &file, options, None);
                    crate::diff_cache::store_working_diff(&workdir, &file.path, stamp, loaded);
                }
            })
            .detach();
    }

    // ---- history (GHD `_loadHistory`, `_loadNextCommitBatch`, `_changeCommitSelection`) ----

    /// Flag `807`: whether History lists first parents only.
    pub fn history_first_parent(s: &AppState) -> bool {
        s.settings.history_first_parent && s.flags.bool(crate::flags::ids::HISTORY_FIRST_PARENT)
    }

    /// Flag `807`: switch History to first parents only (or back) and
    /// reload the selected repository's list.
    pub fn set_history_first_parent(on: bool, cx: &mut dyn Host) {
        Self::update_settings(cx, |s| s.history_first_parent = on);
        if let Some(id) = Self::state(cx).read(cx).selected {
            Self::load_commits(id, false, cx);
            Self::refresh_history_filter(id, true, cx);
        }
    }

    /// Flag `1213`: History lists every branch's commits.
    pub fn history_all_branches(s: &AppState) -> bool {
        s.settings.history_all_branches && s.flags.bool(crate::flags::ids::COMMIT_GRAPH)
    }

    /// Flag `1213`: switch History to All branches (or back) and load the
    /// selected repository's list.
    pub fn set_history_all_branches(on: bool, cx: &mut dyn Host) {
        Self::update_settings(cx, |s| s.history_all_branches = on);
        if on {
            for id in Self::state(cx).read(cx).visible_repositories() {
                Self::load_all_branches(id, false, cx);
            }
        } else {
            Self::clear_all_branches(cx);
        }
    }

    /// Drops every repository's All branches list (History shows HEAD's).
    pub fn clear_all_branches(cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            let mut cleared = false;
            for rs in s.repo_states.values_mut() {
                cleared |= rs.all_branches.take().is_some();
            }
            if cleared {
                cx.notify();
            }
        });
    }

    /// Flag `1213`: the first page of the All branches list (tips read
    /// again), or the next one when `more`.
    pub fn load_all_branches(id: u64, more: bool, cx: &mut dyn Host) {
        let state = Self::state(cx);
        let job = {
            let s = state.read(cx);
            let Some(rs) = s.repo_states.get(&id) else {
                return;
            };
            let Some(info) = rs.info.as_ref() else { return };
            let all = rs.all_branches.as_ref();
            if all.is_some_and(|a| a.loading) {
                None
            } else if more && !all.is_some_and(|a| a.loaded && !a.exhausted) {
                return;
            } else {
                Some((
                    info.workdir.clone(),
                    more.then(|| all.map(|a| a.tips.clone()).unwrap_or_default()),
                    if more {
                        all.map_or(0, |a| a.commits.len())
                    } else {
                        0
                    },
                    Self::history_first_parent(s),
                ))
            }
        };
        let Some((workdir, tips, skip, first_parent)) = job else {
            // a reload asked for while a page loads runs after it
            if !more {
                state.update(cx, |s, _| {
                    if let Some(a) = s.repo_state_mut(id).all_branches.as_mut() {
                        a.reload_pending = true;
                    }
                });
            }
            return;
        };
        state.update(cx, |s, _| {
            s.repo_state_mut(id)
                .all_branches
                .get_or_insert_with(Default::default)
                .loading = true;
        });
        let task = cx.background_executor().spawn(async move {
            let tips = match tips {
                Some(tips) => tips,
                None => corvene_git::all_branch_tips(&workdir).unwrap_or_default(),
            };
            let commits = corvene_git::get_commits_from(
                &workdir,
                &tips,
                skip,
                corvene_git::COMMIT_BATCH_SIZE,
                first_parent,
            );
            (tips, commits)
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let (tips, result) = task.await;
            cx.update(|cx| {
                let reload = Self::state(cx).update(cx, |s, cx| {
                    // switched off meanwhile
                    let Some(all) = s.repo_state_mut(id).all_branches.as_mut() else {
                        return false;
                    };
                    all.loading = false;
                    match result {
                        Ok(batch) => {
                            let full = batch.len() == corvene_git::COMMIT_BATCH_SIZE;
                            if more {
                                all.exhausted = !full;
                                all.commits.extend(batch);
                            } else if all.loaded
                                && all.tips == tips
                                && all.commits.len() >= batch.len()
                                && all.commits[..batch.len()] == batch[..]
                                && (full || all.commits.len() == batch.len())
                            {
                                // a refresh changed nothing: the pages
                                // scrolled in stay
                            } else {
                                all.exhausted = !full;
                                all.commits = batch;
                                all.tips = tips;
                            }
                            all.loaded = true;
                            cx.notify();
                        }
                        Err(err) => warn!(id, %err, "all branches history failed"),
                    }
                    std::mem::take(&mut all.reload_pending)
                });
                if reload {
                    Self::load_all_branches(id, false, cx);
                }
            });
        })
        .detach();
    }

    /// Load the first page of HEAD's history, or the next one when `more`.
    pub fn load_commits(id: u64, more: bool, cx: &mut dyn Host) {
        // `1213`: a reload of History reloads All branches too
        if !more && Self::history_all_branches(Self::state(cx).read(cx)) {
            Self::load_all_branches(id, false, cx);
        }
        // `1110-repository-insights`: the numbers may be out of date now
        if !more {
            Self::check_insights_stale(id, cx);
        }
        // `1216-recent-activity`: and the reflog while it is listed
        if !more
            && Self::state(cx)
                .read(cx)
                .repo_states
                .get(&id)
                .is_some_and(|rs| rs.reflog.is_some())
        {
            Self::load_reflog(id, cx);
        }
        // `1219-tag-manager`: and the tags
        if !more
            && Self::state(cx)
                .read(cx)
                .repo_states
                .get(&id)
                .is_some_and(|rs| rs.tags_view.is_some())
        {
            Self::load_tags(id, cx);
        }
        let state = Self::state(cx);
        // `885-history-load-race`: a reload asked for while a page loads
        // runs once that page is in (GHD drops it), and the next page
        // continues from the tip the list was loaded from, not HEAD
        let race_fix = state
            .read(cx)
            .flags
            .bool(crate::flags::ids::HISTORY_LOAD_RACE);
        let loading = state
            .read(cx)
            .repo_states
            .get(&id)
            .is_some_and(|rs| rs.commits_loading);
        if loading {
            if !more && race_fix {
                state.update(cx, |s, _| {
                    s.repo_state_mut(id).commits_reload_pending = true
                });
            }
            return;
        }
        let (workdir, revision, skip, first_parent, local) = {
            let s = state.read(cx);
            let Some(rs) = s.repo_states.get(&id) else {
                return;
            };
            if more && rs.commits_exhausted {
                return;
            }
            let Some(info) = rs.info.as_ref() else { return };
            let revision = match rs.commits.first() {
                Some(tip) if more && race_fix => tip.sha.clone(),
                // `1212-bisect`: from the bad commit, so the range stays in view
                _ => crate::bisect::history_tip(s, rs).unwrap_or_else(|| "HEAD".to_string()),
            };
            // GHD `loadLocalCommits`: the page reset reloads them, and a
            // further page only when the list's last commit is local (there
            // may be more local ones below it)
            let branch = info.current_branch();
            let local = branch.filter(|_| !info.remotes.is_empty()).and_then(|b| {
                let more_local = rs
                    .commits
                    .last()
                    .is_some_and(|c| rs.local_commits.contains(&c.sha));
                (!more || more_local).then(|| {
                    (
                        b.name.clone(),
                        b.upstream.clone(),
                        if more { rs.local_commits.len() } else { 0 },
                    )
                })
            });
            (
                info.workdir.clone(),
                revision,
                if more { rs.commits.len() } else { 0 },
                Self::history_first_parent(s),
                local,
            )
        };
        // `883-unpublished-commit-links`: which of them no remote has
        let unpublished_git = {
            let s = state.read(cx);
            s.flags
                .bool(crate::flags::ids::UNPUBLISHED_COMMIT_LINKS)
                .then(|| s.git.clone())
                .flatten()
        };
        state.update(cx, |s, _| s.repo_state_mut(id).commits_loading = true);
        let task = cx.background_executor().spawn(async move {
            let unpublished = unpublished_git.and_then(|git| {
                corvene_git::local_only_commits(git, &workdir, "HEAD", UNPUBLISHED_COMMITS_LIMIT)
                    .ok()
                    .filter(|shas| shas.len() < UNPUBLISHED_COMMITS_LIMIT)
                    .map(|shas| shas.into_iter().collect::<std::collections::HashSet<_>>())
            });
            let local = local.map(|(branch, upstream, local_skip)| {
                corvene_git::local_commit_shas(
                    &workdir,
                    &branch,
                    upstream.as_deref(),
                    local_skip,
                    corvene_git::COMMIT_BATCH_SIZE,
                )
                .unwrap_or_default()
            });
            let commits = corvene_git::get_commits_with(
                &workdir,
                &revision,
                skip,
                corvene_git::COMMIT_BATCH_SIZE,
                first_parent,
            );
            (commits, unpublished, local)
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let (result, unpublished, local) = task.await;
            cx.update(|cx| {
                let mut rewritten = Vec::new();
                let mut reload = false;
                let reselect = Self::state(cx).update(cx, |s, cx| {
                    let rs = s.repo_state_mut(id);
                    rs.commits_loading = false;
                    reload = std::mem::take(&mut rs.commits_reload_pending);
                    let mut changed = true;
                    if rs.unpublished_commits != unpublished {
                        rs.unpublished_commits = unpublished;
                        cx.notify();
                    }
                    // GHD `localCommitSHAs`: replaced on a page reset,
                    // extended when a further page brought more local commits
                    match local {
                        Some(shas) if more => {
                            let before = rs.local_commits.len();
                            rs.local_commits.extend(shas);
                            if rs.local_commits.len() != before {
                                cx.notify();
                            }
                        }
                        Some(shas) => {
                            let shas: std::collections::HashSet<String> =
                                shas.into_iter().collect();
                            if rs.local_commits != shas {
                                rs.local_commits = shas;
                                cx.notify();
                            }
                        }
                        // no branch or no remote: nothing is unpushed
                        None if !more && !rs.local_commits.is_empty() => {
                            rs.local_commits.clear();
                            cx.notify();
                        }
                        None => {}
                    }
                    match result {
                        Ok(batch) => {
                            if more {
                                rs.commits_exhausted = batch.len() < corvene_git::COMMIT_BATCH_SIZE;
                                rs.commits.extend(batch);
                            } else if rs.commits == batch
                                || (batch.len() == corvene_git::COMMIT_BATCH_SIZE
                                    && rs.commits.len() >= batch.len()
                                    && rs.commits[..batch.len()] == batch[..])
                            {
                                // a refresh reloads the first page: unchanged,
                                // the pages scrolled in after it stay
                                changed = false;
                            } else {
                                rs.commits_exhausted = batch.len() < corvene_git::COMMIT_BATCH_SIZE;
                                rs.commits = batch;
                            }
                            // the filter's selection is not in the plain list
                            let missing = !rs.compare.is_comparing()
                                && !rs.history_filter.is_active()
                                && !rs.showing_all_branches()
                                && rs
                                    .selected_commits
                                    .iter()
                                    .any(|sha| !rs.commits.iter().any(|c| &c.sha == sha));
                            // flag `830`: a squash / reorder rewrote the
                            // selection; pick the new commits instead
                            if !more {
                                let wanted = std::mem::take(&mut rs.rewritten_selection);
                                if missing {
                                    rewritten = crate::mco::find_rewritten(&rs.commits, &wanted);
                                }
                            }
                            if missing {
                                changed = true;
                                rs.selected_commit = None;
                                rs.selected_commits.clear();
                                rs.shas_in_diff.clear();
                                rs.changeset = None;
                                rs.commit_selected_file = None;
                                rs.commit_diff = None;
                            }
                        }
                        Err(err) => warn!(id, %err, "history failed"),
                    }
                    if changed {
                        cx.notify();
                    }
                    // GHD `updateOrSelectFirstCommit`: with nothing (left)
                    // selected, the newest commit becomes the selection
                    let comparing = rs.compare.is_comparing() || rs.history_filter.is_active();
                    if !more && !comparing && rs.selected_commits.is_empty() {
                        Err(rs.commits.first().map(|c| c.sha.clone()))
                    } else {
                        Ok(!more && !rs.selected_commits.is_empty() && !comparing)
                    }
                });
                if !rewritten.is_empty() {
                    Self::select_commits(id, rewritten, cx);
                } else {
                    match reselect {
                        Err(Some(first)) => Self::select_commits(id, vec![first], cx),
                        Ok(true) => Self::load_changeset(id, cx),
                        _ => {}
                    }
                }
                if !more {
                    Self::reveal_first_bad_commit(id, cx);
                }
                if reload {
                    Self::load_commits(id, false, cx);
                } else if !more {
                    Self::refresh_history_filter(id, false, cx);
                }
            });
        })
        .detach();
    }

    pub fn select_commit(id: u64, sha: String, cx: &mut dyn Host) {
        Self::select_commits(id, vec![sha], cx);
    }

    /// GHD `_changeCommitSelection`: `shas` in click order.
    pub fn select_commits(id: u64, shas: Vec<String>, cx: &mut dyn Host) {
        let changed = Self::state(cx).update(cx, |s, cx| {
            let rs = s.repo_state_mut(id);
            if rs.selected_commits == shas {
                return false;
            }
            let indexes: Vec<usize> = shas
                .iter()
                .filter_map(|sha| rs.visible_commits().iter().position(|c| &c.sha == sha))
                .collect();
            let mut sorted = indexes.clone();
            sorted.sort_unstable();
            let contiguous = sorted.windows(2).all(|w| w[1] == w[0] + 1);
            rs.commits_contiguous = contiguous;
            rs.selected_commit = shas.first().cloned();
            rs.selected_commits = shas.clone();
            rs.shas_in_diff = Self::shas_in_diff(rs, contiguous);
            rs.changeset = None;
            rs.commit_selected_file = None;
            rs.commit_diff = None;
            cx.notify();
            true
        });
        if changed {
            // `798-blame`: another commit was picked
            Self::close_blame(id, cx);
            // `1110-repository-insights`: so does Insights
            Self::close_insights(id, cx);
            Self::load_changeset(id, cx);
        }
    }

    /// ⌘-click: add or remove one commit from the selection.
    pub fn toggle_commit_selection(id: u64, sha: String, cx: &mut dyn Host) {
        let mut shas = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .map(|r| r.selected_commits.clone())
            .unwrap_or_default();
        if let Some(pos) = shas.iter().position(|s| s == &sha) {
            if shas.len() > 1 {
                shas.remove(pos);
            }
        } else {
            shas.push(sha);
        }
        Self::select_commits(id, shas, cx);
    }

    /// ⇧-click: select everything between the anchor and `sha`.
    pub fn extend_commit_selection(id: u64, sha: String, cx: &mut dyn Host) {
        let shas = {
            let s = Self::state(cx).read(cx);
            let Some(rs) = s.repo_states.get(&id) else {
                return;
            };
            let anchor = rs.selected_commit.clone().unwrap_or_else(|| sha.clone());
            let commits = rs.visible_commits();
            let a = commits.iter().position(|c| c.sha == anchor);
            let b = commits.iter().position(|c| c.sha == sha);
            let (Some(a), Some(b)) = (a, b) else {
                return;
            };
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            let mut shas = vec![anchor.clone()];
            shas.extend(
                commits[lo..=hi]
                    .iter()
                    .map(|c| c.sha.clone())
                    .filter(|s| s != &anchor),
            );
            shas
        };
        Self::select_commits(id, shas, cx);
    }

    /// GHD `getShasInDiff`: walk parents from the newest selected commit.
    fn shas_in_diff(rs: &RepositoryState, contiguous: bool) -> Vec<String> {
        if rs.selected_commits.len() <= 1 || !contiguous {
            return rs.selected_commits.clone();
        }
        let ordered = Self::ordered_selection(rs);
        let selected: std::collections::HashSet<&str> =
            ordered.iter().map(String::as_str).collect();
        let mut in_diff: Vec<String> = Vec::new();
        let mut stack: Vec<String> = ordered.last().cloned().into_iter().collect();
        while let Some(sha) = stack.pop() {
            if in_diff.contains(&sha) {
                continue;
            }
            in_diff.push(sha.clone());
            if let Some(commit) = rs.visible_commits().iter().find(|c| c.sha == sha) {
                for parent in &commit.parents {
                    if selected.contains(parent.as_str()) && !in_diff.contains(parent) {
                        stack.push(parent.clone());
                    }
                }
            }
        }
        in_diff
    }

    /// `orderShasByHistory`: the selection oldest first, in the list History
    /// shows (`compareState.commitSHAs`).
    pub fn ordered_selection(rs: &RepositoryState) -> Vec<String> {
        let mut with_index: Vec<(usize, &String)> = rs
            .selected_commits
            .iter()
            .filter_map(|sha| {
                rs.visible_commits()
                    .iter()
                    .position(|c| &c.sha == sha)
                    .map(|i| (i, sha))
            })
            .collect();
        with_index.sort_by_key(|b| std::cmp::Reverse(b.0));
        with_index.into_iter().map(|(_, sha)| sha.clone()).collect()
    }

    /// `_loadChangedFilesForCurrentSelection`
    fn load_changeset(id: u64, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        // `1314-code-owners`: the owners as of the (first) selected commit
        if let Some(sha) = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|rs| rs.selected_commit.clone())
        {
            Self::load_code_owners_at(id, sha, false, cx);
        }
        let (ordered, contiguous) = {
            let s = Self::state(cx).read(cx);
            let Some(rs) = s.repo_states.get(&id) else {
                return;
            };
            (Self::ordered_selection(rs), rs.commits_contiguous)
        };
        let in_process = Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::IN_PROCESS_COMMIT_FILES);
        if ordered.is_empty() || (ordered.len() > 1 && !contiguous) {
            return;
        }
        // `773-merge-remerge-diff`: a merge's conflict resolutions only
        let remerge = {
            let s = Self::state(cx).read(cx);
            s.repo_states
                .get(&id)
                .is_some_and(|rs| Self::remerge_applies(s, rs))
        };
        if remerge {
            let key = ordered.clone();
            let task = cx.background_executor().spawn(async move {
                corvene_git::remerge_changed_files(git, &workdir, &ordered[0]).map(Arc::new)
            });
            cx.spawn(async move |cx: &mut AsyncCtx| {
                let result = task.await;
                cx.update(|cx| Self::apply_changeset(id, &key, true, result, 0, cx));
            })
            .detach();
            return;
        }
        // Corvene (`793-hide-whitespace-only-files`): while whitespace is
        // hidden, the files with whitespace changes only are left out
        let hide_whitespace_only = {
            let s = Self::state(cx).read(cx);
            s.flags.bool(crate::flags::ids::HIDE_WHITESPACE_ONLY_FILES)
                && s.settings.hide_whitespace_in_history_diff
        };
        // Corvene `1319-trivial-change-icon`
        let trivial = Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::TRIVIAL_CHANGE_ICON);
        // a commit's files never change: shown in this same frame
        if !hide_whitespace_only
            && let Some(data) = crate::diff_cache::changeset(&workdir, &ordered)
            && (!trivial || data.trivial_read)
        {
            Self::apply_changeset(id, &ordered, false, Ok(data), 0, cx);
            return;
        }
        let key = ordered.clone();
        let task = cx.background_executor().spawn(async move {
            let data = match crate::diff_cache::changeset(&workdir, &ordered) {
                Some(data) if trivial => with_trivial_changes(&workdir, &ordered, data),
                Some(data) => data,
                None => {
                    let data =
                        compute_changeset(git.clone(), &workdir, &ordered, in_process, trivial)?;
                    crate::diff_cache::store_changeset(&workdir, &ordered, data.clone());
                    data
                }
            };
            if !hide_whitespace_only {
                return Ok((data, 0));
            }
            Ok(without_whitespace_only_files(git, &workdir, &ordered, data))
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let (result, hidden) = match task.await {
                Ok((data, hidden)) => (Ok(data), hidden),
                Err(err) => (Err(err), 0),
            };
            cx.update(|cx| Self::apply_changeset(id, &key, false, result, hidden, cx));
        })
        .detach();
    }

    /// `773-merge-remerge-diff`: History can show only the selected merge's
    /// conflict resolutions (one merge commit selected, git 2.36 or newer).
    pub fn remerge_available(s: &AppState, rs: &RepositoryState) -> bool {
        let git_ok = s.git.as_ref().is_some_and(|git| {
            (git.version.major, git.version.minor) >= corvene_git::REMERGE_DIFF_MIN_VERSION
        });
        git_ok
            && s.flags.bool(crate::flags::ids::MERGE_REMERGE_DIFF)
            && rs.selected_commits.len() == 1
            && rs
                .visible_commits()
                .iter()
                .find(|c| Some(&c.sha) == rs.selected_commits.first())
                .is_some_and(|c| c.parents.len() > 1)
    }

    /// `773-merge-remerge-diff`: the toggle is on and applies to the selection.
    pub fn remerge_applies(s: &AppState, rs: &RepositoryState) -> bool {
        rs.remerge_diff && Self::remerge_available(s, rs)
    }

    /// `773-merge-remerge-diff`: show the selected merge's conflict
    /// resolutions only (or all of its changes again).
    pub fn set_remerge_diff(id: u64, on: bool, cx: &mut dyn Host) {
        let changed = Self::state(cx).update(cx, |s, cx| {
            let rs = s.repo_state_mut(id);
            if rs.remerge_diff == on {
                return false;
            }
            rs.remerge_diff = on;
            rs.changeset = None;
            rs.commit_diff = None;
            cx.notify();
            true
        });
        if changed {
            Self::load_changeset(id, cx);
        }
    }

    fn apply_changeset(
        id: u64,
        key: &[String],
        remerge: bool,
        result: corvene_git::error::Result<Arc<corvene_models::ChangesetData>>,
        whitespace_hidden: usize,
        cx: &mut dyn Host,
    ) {
        let load = Self::state(cx).update(cx, |s, cx| {
            let applies = s
                .repo_states
                .get(&id)
                .is_some_and(|rs| Self::remerge_applies(s, rs));
            let rs = s.repo_state_mut(id);
            if Self::ordered_selection(rs) != key || applies != remerge {
                return false;
            }
            match result {
                Ok(data) => {
                    // `887-file-history`: the file under its name in the commit
                    let history_file = key
                        .iter()
                        .rev()
                        .find_map(|sha| rs.history_filter.file_path_at(sha))
                        .filter(|p| data.files.iter().any(|f| f.path == *p))
                        .map(str::to_string);
                    // keep the file selection when the same path is still there
                    let keep = rs
                        .commit_selected_file
                        .as_ref()
                        .filter(|p| data.files.iter().any(|f| &f.path == *p))
                        .cloned();
                    let file = history_file
                        .or(keep)
                        .or_else(|| data.files.first().map(|f| f.path.clone()));
                    let mut changed = set(&mut rs.commit_selected_file, file);
                    changed |= set(&mut rs.changeset_whitespace_hidden, whitespace_hidden);
                    if rs.changeset.as_ref() != Some(&*data) {
                        rs.changeset = Some(Arc::unwrap_or_clone(data));
                        changed = true;
                    }
                    if changed {
                        cx.notify();
                    }
                }
                Err(err) => {
                    warn!(id, %err, "changed files failed");
                    cx.notify();
                }
            }
            true
        });
        if load {
            Self::load_commit_diff(id, cx);
        }
    }

    pub fn select_commit_file(id: u64, path: String, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            s.repo_state_mut(id).commit_selected_file = Some(path);
            cx.notify();
        });
        Self::load_commit_diff(id, cx);
    }

    pub(crate) fn load_commit_diff(id: u64, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let Some(file) = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|rs| {
                let path = rs.commit_selected_file.as_ref()?;
                rs.changeset
                    .as_ref()?
                    .files
                    .iter()
                    .find(|f| &f.path == path)
                    .cloned()
            })
        else {
            return;
        };
        let ordered = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .map(Self::ordered_selection)
            .unwrap_or_default();
        let hide_whitespace = Self::state(cx)
            .read(cx)
            .settings
            .hide_whitespace_in_history_diff;
        // `773-merge-remerge-diff`: from the re-merge to the recorded merge
        let remerge = {
            let s = Self::state(cx).read(cx);
            s.repo_states
                .get(&id)
                .is_some_and(|rs| Self::remerge_applies(s, rs))
        };
        if remerge {
            let key = (ordered, file.path.clone());
            let task = cx.background_executor().spawn(async move {
                crate::stacked_diff::remerge_diff(git, &workdir, &file, hide_whitespace)
            });
            cx.spawn(async move |cx: &mut AsyncCtx| {
                let loaded = task.await;
                cx.update(|cx| Self::apply_commit_diff(id, &key.0, &key.1, true, loaded, cx));
            })
            .detach();
            return;
        }
        // `794-svg-image-diff`: the SVG before and after, as images
        let svg_image = {
            let s = Self::state(cx).read(cx);
            s.repo_states
                .get(&id)
                .is_some_and(|rs| Self::svg_shown_as_image(s, rs, &file.path))
        };
        if svg_image {
            let key = (ordered.clone(), file.path.clone());
            let task = cx.background_executor().spawn(async move {
                crate::stacked_diff::svg_image_diff(git, &workdir, &ordered, &file)
            });
            cx.spawn(async move |cx: &mut AsyncCtx| {
                let loaded = task.await;
                cx.update(|cx| Self::apply_commit_diff(id, &key.0, &key.1, false, loaded, cx));
            })
            .detach();
            return;
        }
        if let Some(loaded) =
            crate::diff_cache::commit_diff(&workdir, &ordered, &file.path, hide_whitespace)
        {
            Self::apply_commit_diff(id, &ordered, &file.path, false, loaded, cx);
            Self::prefetch_commit_diffs(id, cx);
            return;
        }
        let key = (ordered.clone(), file.path.clone());
        let diff_options = CommitDiffOptions::of(Self::state(cx).read(cx));
        let task = cx.background_executor().spawn(async move {
            let loaded = compute_commit_diff(git, &workdir, &ordered, &file, diff_options);
            crate::diff_cache::store_commit_diff(
                &workdir,
                &ordered,
                &file.path,
                hide_whitespace,
                loaded.clone(),
            );
            loaded
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let loaded = task.await;
            cx.update(|cx| {
                Self::apply_commit_diff(id, &key.0, &key.1, false, loaded, cx);
                Self::prefetch_commit_diffs(id, cx);
            });
        })
        .detach();
    }

    fn apply_commit_diff(
        id: u64,
        shas: &[String],
        path: &str,
        remerge: bool,
        loaded: LoadedDiff,
        cx: &mut dyn Host,
    ) {
        Self::state(cx).update(cx, |s, cx| {
            let applies = s
                .repo_states
                .get(&id)
                .is_some_and(|rs| Self::remerge_applies(s, rs));
            let rs = s.repo_state_mut(id);
            if Self::ordered_selection(rs) != shas
                || rs.commit_selected_file.as_deref() != Some(path)
                || applies != remerge
            {
                return;
            }
            if replace_diff(
                (
                    &mut rs.commit_diff,
                    &mut rs.commit_diff_contents,
                    &mut rs.commit_diff_old_contents,
                ),
                loaded,
            ) {
                rs.commit_diff_generation += 1;
                cx.notify();
            }
        });
    }

    /// `901-prefetch-diffs`: the changed files and first diff of the
    /// commits next to the selected one, computed in the background.
    fn prefetch_commit_diffs(id: u64, cx: &mut dyn Host) {
        let s = Self::state(cx).read(cx);
        if !s.flags.bool(crate::flags::ids::PREFETCH_DIFFS) {
            return;
        }
        let hide_whitespace = s.settings.hide_whitespace_in_history_diff;
        let diff_options = CommitDiffOptions::of(s);
        let in_process = s.flags.bool(crate::flags::ids::IN_PROCESS_COMMIT_FILES);
        let trivial = s.flags.bool(crate::flags::ids::TRIVIAL_CHANGE_ICON);
        let (Some(git), Some(rs)) = (s.git.clone(), s.repo_states.get(&id)) else {
            return;
        };
        let Some(workdir) = rs.info.as_ref().map(|i| i.workdir.clone()) else {
            return;
        };
        // only while History shows (a refresh reselects the newest commit
        // behind the Changes tab too)
        if rs.selected_commits.len() != 1 || rs.section != Section::History {
            return;
        }
        let commits = rs.visible_commits();
        let Some(ix) = rs
            .selected_commit
            .as_ref()
            .and_then(|sha| commits.iter().position(|c| &c.sha == sha))
        else {
            return;
        };
        let neighbours: Vec<String> = [1isize, -1, 2, 3]
            .into_iter()
            .filter_map(|d| commits.get(ix.checked_add_signed(d)?))
            .map(|c| c.sha.clone())
            .collect();
        cx.background_executor()
            .spawn(async move {
                for sha in neighbours {
                    let shas = [sha];
                    let data = match crate::diff_cache::changeset(&workdir, &shas) {
                        Some(data) => data,
                        None => {
                            let Ok(data) = compute_changeset(
                                git.clone(),
                                &workdir,
                                &shas,
                                in_process,
                                trivial,
                            ) else {
                                continue;
                            };
                            crate::diff_cache::store_changeset(&workdir, &shas, data.clone());
                            data
                        }
                    };
                    // the file a selection there shows first
                    let Some(file) = data.files.first() else {
                        continue;
                    };
                    if crate::diff_cache::commit_diff(&workdir, &shas, &file.path, hide_whitespace)
                        .is_none()
                    {
                        let loaded =
                            compute_commit_diff(git.clone(), &workdir, &shas, file, diff_options);
                        crate::diff_cache::store_commit_diff(
                            &workdir,
                            &shas,
                            &file.path,
                            hide_whitespace,
                            loaded,
                        );
                    }
                }
            })
            .detach();
    }

    // ---- history operations (`_revertCommit`, `_resetToCommit`, `_checkoutCommit`, tags, amend) ----

    pub(crate) fn run_history_op(
        id: u64,
        error_title: &'static str,
        op: impl FnOnce(
            std::sync::Arc<corvene_git::GitBinary>,
            PathBuf,
        ) -> corvene_git::error::Result<()>
        + Send
        + 'static,
        cx: &mut dyn Host,
    ) {
        Self::run_history_op_then(id, error_title, op, |_| {}, cx);
    }

    /// [`Self::run_history_op`], then `then` when `op` succeeded.
    pub(crate) fn run_history_op_then(
        id: u64,
        error_title: &'static str,
        op: impl FnOnce(
            std::sync::Arc<corvene_git::GitBinary>,
            PathBuf,
        ) -> corvene_git::error::Result<()>
        + Send
        + 'static,
        then: impl FnOnce(&mut dyn Host) + 'static,
        cx: &mut dyn Host,
    ) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let task = cx
            .background_executor()
            .spawn(async move { op(git, workdir) });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let result = task.await;
            cx.update(|cx| {
                let ok = result.is_ok();
                if let Err(err) = result {
                    Self::show_error(error_title, &err, cx);
                }
                Self::refresh_repository(id, cx);
                if ok {
                    then(cx);
                }
            });
        })
        .detach();
    }

    /// Flag `814`: undo one file's changes from commit `sha` in the working
    /// tree (`corvene_git::revert_file_in_commit`), then refresh.
    pub fn revert_file_in_commit(
        id: u64,
        sha: String,
        path: String,
        old_path: Option<String>,
        cx: &mut dyn Host,
    ) {
        Self::run_history_op(
            id,
            "Could not revert the file",
            move |git, workdir| {
                corvene_git::revert_file_in_commit(git, &workdir, &sha, &path, old_path.as_deref())
            },
            cx,
        );
    }

    pub(crate) fn working_directory_dirty(id: u64, cx: &dyn Host) -> bool {
        Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|rs| rs.status.as_deref())
            .is_some_and(|st| !st.files.is_empty())
    }

    pub(crate) fn commit_by_sha(
        id: u64,
        sha: &str,
        cx: &dyn Host,
    ) -> Option<corvene_models::Commit> {
        Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)?
            .commits
            .iter()
            .find(|c| c.sha == sha)
            .cloned()
    }

    /// `Revert Changes in Commit`
    pub fn revert_commit(id: u64, sha: String, cx: &mut dyn Host) {
        let is_merge = Self::commit_by_sha(id, &sha, cx).is_some_and(|c| c.is_merge());
        Self::run_history_op(
            id,
            "Could not revert commit",
            move |git, workdir| corvene_git::revert_commit(git, &workdir, &sha, is_merge),
            cx,
        );
    }

    /// Corvene addition (flag `815`): revert `shas` without committing, the
    /// newest first, and show Changes with the result staged. Needs a clean
    /// working directory, so a conflict can roll everything back.
    pub fn revert_commits_without_committing(id: u64, mut shas: Vec<String>, cx: &mut dyn Host) {
        const TITLE: &str = "Could not revert changes";
        if Self::working_directory_dirty(id, cx) {
            Self::show_error(
                TITLE,
                "Commit or stash your changes before reverting without committing.",
                cx,
            );
            return;
        }
        let any_merge = shas
            .iter()
            .any(|sha| Self::commit_by_sha(id, sha, cx).is_some_and(|c| c.is_merge()));
        if let Some(rs) = Self::state(cx).read(cx).repo_states.get(&id) {
            shas.sort_by_key(|sha| {
                rs.commits
                    .iter()
                    .position(|c| &c.sha == sha)
                    .unwrap_or(usize::MAX)
            });
        }
        Self::show_section(id, Section::Changes, cx);
        Self::run_history_op(
            id,
            TITLE,
            move |git, workdir| {
                corvene_git::revert_commits_no_commit(git, &workdir, &shas, any_merge)
            },
            cx,
        );
    }

    /// `shas` sorted oldest first by their place in the loaded history.
    fn oldest_first(id: u64, mut shas: Vec<String>, cx: &dyn Host) -> Vec<String> {
        if let Some(rs) = Self::state(cx).read(cx).repo_states.get(&id) {
            shas.sort_by_key(|sha| {
                std::cmp::Reverse(rs.commits.iter().position(|c| &c.sha == sha))
            });
        }
        shas
    }

    /// Corvene addition (flag `820`): apply `shas` to the current branch
    /// without committing (oldest first) and show Changes with the result
    /// staged. Needs a clean working directory, so a conflict can roll back.
    pub fn cherry_pick_without_committing(id: u64, shas: Vec<String>, cx: &mut dyn Host) {
        const TITLE: &str = "Could not cherry-pick";
        if Self::working_directory_dirty(id, cx) {
            Self::show_error(
                TITLE,
                "Commit or stash your changes before cherry-picking without committing.",
                cx,
            );
            return;
        }
        let any_merge = shas
            .iter()
            .any(|sha| Self::commit_by_sha(id, sha, cx).is_some_and(|c| c.is_merge()));
        let shas = Self::oldest_first(id, shas, cx);
        Self::show_section(id, Section::Changes, cx);
        Self::run_history_op(
            id,
            TITLE,
            move |git, workdir| corvene_git::cherry_pick_no_commit(git, &workdir, &shas, any_merge),
            cx,
        );
    }

    /// Corvene addition (flag `821`): `git format-patch` each of `shas`
    /// (oldest first) into `dir`, then reveal the first patch in Finder.
    pub fn create_patch_files(id: u64, shas: Vec<String>, dir: PathBuf, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let shas = Self::oldest_first(id, shas, cx);
        let task = cx
            .background_executor()
            .spawn(async move { corvene_git::format_patches(git, &workdir, &shas, &dir) });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let result = task.await;
            cx.update(|cx| match result {
                Ok(files) => {
                    if let Some(first) = files.first() {
                        cx.reveal_path(first);
                    }
                }
                Err(err) => Self::show_error("Could not create patch files", &err, cx),
            });
        })
        .detach();
    }

    /// `Reset to Commit…`: warn first when the working directory is dirty.
    pub fn request_reset_to_commit(id: u64, sha: String, cx: &mut dyn Host) {
        if Self::working_directory_dirty(id, cx) {
            Self::show_popup(Popup::ResetToCommit { repo: id, sha }, cx);
        } else {
            Self::reset_to_commit(id, sha, cx);
        }
    }

    pub fn reset_to_commit(id: u64, sha: String, cx: &mut dyn Host) {
        Self::reset_to_commit_with(id, sha, corvene_git::ResetMode::Mixed, cx);
    }

    /// `reset [--soft | --hard] <sha>` (`888-reset-modes` picks the mode;
    /// GHD always resets mixed).
    pub fn reset_to_commit_with(
        id: u64,
        sha: String,
        mode: corvene_git::ResetMode,
        cx: &mut dyn Host,
    ) {
        Self::show_section(id, Section::Changes, cx);
        Self::run_history_op(
            id,
            "Could not reset to commit",
            move |git, workdir| corvene_git::reset_to(git, &workdir, mode, &sha),
            cx,
        );
    }

    /// `888-reset-modes`: Reset to Commit › Soft keeps every change (the
    /// commits' ones staged), so it needs no warning.
    pub fn soft_reset_to_commit(id: u64, sha: String, cx: &mut dyn Host) {
        Self::reset_to_commit_with(id, sha, corvene_git::ResetMode::Soft, cx);
    }

    /// `888-reset-modes`: Reset to Commit › Hard. Counts the commits it
    /// drops, then confirms naming them and the uncommitted changes it
    /// discards (the `261-reset-to-remote` dialog).
    pub fn request_hard_reset_to_commit(id: u64, sha: String, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let (branch, files) = {
            let s = Self::state(cx).read(cx);
            let rs = s.repo_states.get(&id);
            let branch = rs
                .and_then(|r| r.info.as_ref())
                .map(|info| match info.current_branch() {
                    Some(b) => b.name.clone(),
                    None => "HEAD".to_string(),
                })
                .unwrap_or_else(|| "HEAD".to_string());
            // `reset --hard` leaves untracked files alone
            let files: Vec<String> = rs
                .and_then(|r| r.status.as_deref())
                .map(|st| {
                    st.files
                        .iter()
                        .filter(|f| f.status.kind != corvene_models::FileStatusKind::Untracked)
                        .map(|f| f.path.clone())
                        .collect()
                })
                .unwrap_or_default();
            (branch, files)
        };
        let target = sha.clone();
        let task = cx.background_executor().spawn(async move {
            corvene_git::commits_ahead(git, &workdir, &target, "HEAD").unwrap_or(0)
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let ahead = task.await;
            cx.update(|cx| {
                Self::show_popup(
                    Popup::ResetToRemote {
                        repo: id,
                        branch,
                        upstream: sha[..sha.len().min(7)].to_string(),
                        ahead: ahead as usize,
                        dirty: !files.is_empty(),
                        commit: Some(sha),
                        files,
                    },
                    cx,
                )
            });
        })
        .detach();
    }

    /// The push/pull foldout's "Reset to <upstream>" (`261-reset-to-remote`;
    /// GHD has no such command): confirm, then [`Self::reset_to_remote`].
    pub fn request_reset_to_remote(id: u64, cx: &mut dyn Host) {
        let popup = {
            let s = Self::state(cx).read(cx);
            if !s.flags.bool(crate::flags::ids::RESET_TO_REMOTE) {
                return;
            }
            let rs = s.repo_states.get(&id);
            let branch = rs
                .and_then(|r| r.info.as_ref())
                .and_then(|i| i.current_branch());
            let Some((branch, upstream)) =
                branch.and_then(|b| Some((b.name.clone(), b.upstream_short()?.to_string())))
            else {
                return;
            };
            Popup::ResetToRemote {
                repo: id,
                branch,
                upstream,
                ahead: rs
                    .and_then(|r| r.ahead_behind)
                    .map_or(0, |ab| ab.ahead as usize),
                dirty: Self::working_directory_dirty(id, cx),
                commit: None,
                files: Vec::new(),
            }
        };
        Self::show_popup(popup, cx);
    }

    /// `reset --hard <upstream>`: the branch matches its upstream; local
    /// commits stay reachable through the reflog.
    pub fn reset_to_remote(id: u64, upstream: String, cx: &mut dyn Host) {
        Self::show_section(id, Section::Changes, cx);
        Self::run_history_op(
            id,
            "Could not reset to the remote",
            move |git, workdir| {
                corvene_git::reset_to(
                    git,
                    &workdir,
                    corvene_git::ResetMode::Hard,
                    &format!("refs/remotes/{upstream}"),
                )
            },
            cx,
        );
    }

    /// `Checkout Commit`: confirm unless the user opted out.
    pub fn request_checkout_commit(id: u64, sha: String, cx: &mut dyn Host) {
        if Self::state(cx).read(cx).settings.confirm_checkout_commit {
            Self::show_popup(Popup::CheckoutCommit { repo: id, sha }, cx);
        } else {
            Self::checkout_commit(id, sha, cx);
        }
    }

    pub fn checkout_commit(id: u64, sha: String, cx: &mut dyn Host) {
        Self::run_history_op(
            id,
            "Could not checkout commit",
            move |git, workdir| corvene_git::checkout_commit(git, &workdir, &sha),
            cx,
        );
    }

    /// GHD `_createTag`: the new tag joins `tagsToPush`.
    /// `message` is empty unless flag `823` shows the Message field.
    pub fn create_tag(id: u64, name: String, sha: String, message: String, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let tag = name.clone();
        let task = cx
            .background_executor()
            .spawn(async move { corvene_git::create_tag(git, &workdir, &name, &sha, &message) });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let result = task.await;
            cx.update(|cx| {
                match result {
                    Ok(()) => Self::update_tags_to_push(id, cx, |tags| {
                        if !tags.contains(&tag) {
                            tags.push(tag);
                        }
                    }),
                    Err(err) => Self::show_error("Could not create tag", &err, cx),
                }
                Self::refresh_repository(id, cx);
            });
        })
        .detach();
    }

    /// GHD `_deleteTag` (only unpushed tags are offered): it leaves `tagsToPush`.
    pub fn delete_tag(id: u64, name: String, cx: &mut dyn Host) {
        let tag = name.clone();
        Self::update_tags_to_push(id, cx, |tags| tags.retain(|t| *t != tag));
        Self::run_history_op(
            id,
            "Could not delete tag",
            move |git, workdir| corvene_git::delete_tag(git, &workdir, &name),
            cx,
        );
    }

    /// `224-alias-when-adding`: give the repository at `path` this alias once
    /// it is added (by New / Add / Clone); an empty alias does nothing.
    pub fn alias_when_added(path: &Path, alias: String, cx: &mut dyn Host) {
        let alias = alias.trim().to_string();
        if alias.is_empty() {
            return;
        }
        let path = resolve_path(path);
        Self::state(cx).update(cx, |s, _| {
            s.pending_aliases.retain(|(p, _)| !same_path(p, &path));
            s.pending_aliases.push((path, alias));
        });
    }

    /// Remove and return the alias waiting for `path` (see
    /// [`Dispatcher::alias_when_added`]).
    fn take_pending_alias(path: &Path, cx: &mut dyn Host) -> Option<String> {
        Self::state(cx).update(cx, |s, _| {
            let ix = s
                .pending_aliases
                .iter()
                .position(|(p, _)| same_path(p, path))?;
            Some(s.pending_aliases.remove(ix).1)
        })
    }

    /// GHD `changeRepositoryAlias` / `removeRepositoryAlias` (`None`).
    pub fn change_repository_alias(id: u64, alias: Option<String>, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            if let Some(repo) = s.repositories.iter_mut().find(|r| r.id == id) {
                repo.alias = alias.filter(|a| !a.is_empty());
                persist_repositories(s);
                cx.notify();
            }
        });
    }

    /// Corvene (`267-pinned-repositories`): pin or unpin a repository.
    pub fn set_repository_pinned(id: u64, pinned: bool, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            if let Some(repo) = s.repositories.iter_mut().find(|r| r.id == id) {
                repo.pinned = pinned;
                persist_repositories(s);
                cx.notify();
            }
        });
    }

    /// Corvene (`290-custom-repository-groups`): move a repository to the
    /// repository list group `group` (trimmed; `None` or empty = back to its
    /// owner's group).
    pub fn set_repository_group(id: u64, group: Option<String>, cx: &mut dyn Host) {
        let group = group
            .map(|g| g.trim().to_string())
            .filter(|g| !g.is_empty());
        Self::state(cx).update(cx, |s, cx| {
            if let Some(repo) = s.repositories.iter_mut().find(|r| r.id == id)
                && repo.group != group
            {
                repo.group = group;
                persist_repositories(s);
                cx.notify();
            }
        });
    }

    /// Corvene (`897-pinned-branches`): pin or unpin a branch of a repository.
    pub fn set_branch_pinned(id: u64, branch: String, pinned: bool, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            if let Some(repo) = s.repositories.iter_mut().find(|r| r.id == id) {
                repo.pinned_branches.retain(|b| *b != branch);
                if pinned {
                    repo.pinned_branches.push(branch);
                }
                persist_repositories(s);
                cx.notify();
            }
        });
    }

    /// `518-per-repo-editor`: the repository's own external editor (`None`:
    /// the one in Settings).
    pub fn set_repository_editor(id: u64, editor: Option<String>, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            if let Some(repo) = s.repositories.iter_mut().find(|r| r.id == id) {
                repo.editor = editor;
                repo.custom_editor = None;
                persist_repositories(s);
                cx.notify();
            }
        });
    }

    /// Corvene (`525-account-commit-email`): a repository just cloned or
    /// added that belongs to an account with a commit email in Settings ›
    /// Accounts gets it as its local `user.email`, unless it has one.
    pub(crate) fn apply_account_commit_email(id: u64, cx: &mut dyn Host) {
        let s = Self::state(cx).read(cx);
        if !s.flags.bool(crate::flags::ids::ACCOUNT_COMMIT_EMAIL) {
            return;
        }
        // `527-multiple-accounts`: once its account is known
        if Self::needs_account_lookup(s, id) {
            return;
        }
        let Some(repo) = s.repository(id) else {
            return;
        };
        let email = s
            .account_for_repository(id)
            .and_then(|account| {
                s.settings
                    .account_commit_emails
                    .get(&account_commit_email_key(account))
            })
            .map(|email| email.trim().to_string())
            .filter(|email| !email.is_empty());
        let (Some(email), Some(git)) = (email, s.git.clone()) else {
            return;
        };
        let path = repo.path.clone();
        crate::remote::spawn_bg(
            cx,
            move || {
                if corvene_git::local_config_value(git.clone(), &path, "user.email").is_some() {
                    return Ok(false);
                }
                corvene_git::set_local_config_value(git, &path, "user.email", &email).map(|_| true)
            },
            move |result, _| match result {
                Ok(true) => info!(id, "set the account's commit email"),
                Ok(false) => {}
                Err(err) => warn!(id, %err, "could not set the account's commit email"),
            },
        );
    }

    /// Corvene (`341-custom-autolinks`): Repository Settings › Autolinks.
    pub fn set_repository_autolinks(
        id: u64,
        autolinks: Vec<corvene_models::Autolink>,
        cx: &mut dyn Host,
    ) {
        Self::state(cx).update(cx, |s, cx| {
            if let Some(repo) = s.repositories.iter_mut().find(|r| r.id == id)
                && repo.autolinks != autolinks
            {
                repo.autolinks = autolinks;
                persist_repositories(s);
                cx.notify();
            }
        });
    }

    /// `518-per-repo-editor`: a custom editor for repository `id` (in place
    /// of an installed one).
    pub fn set_repository_custom_editor(
        id: u64,
        editor: Option<corvene_models::RepoCustomEditor>,
        cx: &mut dyn Host,
    ) {
        Self::state(cx).update(cx, |s, cx| {
            if let Some(repo) = s.repositories.iter_mut().find(|r| r.id == id) {
                repo.custom_editor = editor;
                repo.editor = None;
                persist_repositories(s);
                cx.notify();
            }
        });
    }

    /// Repository Settings › Remote's credential helper checkbox
    /// (`1102-repository-credential-helper`).
    pub fn set_repository_credential_helper(id: u64, on: bool, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            if let Some(repo) = s.repositories.iter_mut().find(|r| r.id == id)
                && repo.use_credential_helper != on
            {
                repo.use_credential_helper = on;
                persist_repositories(s);
                cx.notify();
            }
        });
    }

    /// Edit the repository's persisted `tagsToPush` (`storeTagsToPush`).
    pub(crate) fn update_tags_to_push(
        id: u64,
        cx: &mut dyn Host,
        edit: impl FnOnce(&mut Vec<String>),
    ) {
        Self::state(cx).update(cx, |s, cx| {
            if let Some(repo) = s.repositories.iter_mut().find(|r| r.id == id) {
                edit(&mut repo.tags_to_push);
                persist_repositories(s);
                cx.notify();
            }
        });
    }

    /// `Undo Commit…` from history: warn about the commit's tags (flag
    /// `819`), then about local changes.
    pub fn request_undo_commit(id: u64, cx: &mut dyn Host) {
        let tags = Self::undo_warning_tags(id, cx);
        if !tags.is_empty() {
            return Self::show_popup(
                Popup::WarnTaggedCommitBeforeUndo {
                    repo: id,
                    tags,
                    warn_local: true,
                },
                cx,
            );
        }
        Self::request_undo_commit_after_tags(id, cx);
    }

    /// The Changes view's Undo button: the tag warning (flag `819`) only.
    pub fn request_undo_last_commit(id: u64, cx: &mut dyn Host) {
        let tags = Self::undo_warning_tags(id, cx);
        if tags.is_empty() {
            return Self::undo_commit(id, cx);
        }
        Self::show_popup(
            Popup::WarnTaggedCommitBeforeUndo {
                repo: id,
                tags,
                warn_local: false,
            },
            cx,
        );
    }

    /// Flag `819`: HEAD's tags, empty when the flag is off.
    fn undo_warning_tags(id: u64, cx: &dyn Host) -> Vec<String> {
        {
            let s = Self::state(cx).read(cx);
            s.flags
                .bool(crate::flags::ids::WARN_UNDO_TAGGED_COMMIT)
                .then(|| {
                    // the history list starts at HEAD
                    let commit = s.repo_states.get(&id)?.commits.first()?;
                    Some(commit.tags.clone())
                })
                .flatten()
                .unwrap_or_default()
        }
    }

    /// The local-changes half of [`Self::request_undo_commit`].
    pub fn request_undo_commit_after_tags(id: u64, cx: &mut dyn Host) {
        let (confirm, overlap_only, in_process) = {
            let s = Self::state(cx).read(cx);
            (
                s.settings.confirm_undo_commit,
                s.flags.bool(crate::flags::ids::UNDO_WARNS_ONLY_ON_OVERLAP),
                s.flags.bool(crate::flags::ids::IN_PROCESS_COMMIT_FILES),
            )
        };
        if !(confirm && Self::working_directory_dirty(id, cx)) {
            return Self::undo_commit(id, cx);
        }
        // `818`: warn only when the commit touches a file with local changes
        let context = Self::repo_context(id, cx).filter(|_| overlap_only);
        let Some((git, workdir)) = context else {
            return Self::show_popup(Popup::WarnLocalChangesBeforeUndo { repo: id }, cx);
        };
        let local: Vec<String> = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|rs| rs.status.as_deref())
            .map(|st| {
                st.files
                    .iter()
                    .flat_map(|f| std::iter::once(f.path.clone()).chain(f.old_path.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let task = cx.background_executor().spawn(async move {
            corvene_git::get_changed_files(git, &workdir, "HEAD", in_process)
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let changed = task.await;
            cx.update(|cx| {
                let overlap = match changed {
                    Ok(changeset) => paths_overlap(
                        changeset
                            .files
                            .iter()
                            .flat_map(|f| std::iter::once(&f.path).chain(f.old_path.as_ref())),
                        &local,
                    ),
                    Err(_) => true,
                };
                if overlap {
                    Self::show_popup(Popup::WarnLocalChangesBeforeUndo { repo: id }, cx);
                } else {
                    Self::undo_commit(id, cx);
                }
            });
        })
        .detach();
    }

    /// Amend Commit…: warn about the commit's tags first (flag `819`; GHD
    /// amends silently and the tags stay on the replaced commit).
    pub fn request_start_amending(id: u64, sha: String, cx: &mut dyn Host) {
        let tags = {
            let s = Self::state(cx).read(cx);
            s.flags
                .bool(crate::flags::ids::WARN_UNDO_TAGGED_COMMIT)
                .then(|| Self::commit_by_sha(id, &sha, cx).map(|c| c.tags))
                .flatten()
                .unwrap_or_default()
        };
        if tags.is_empty() {
            return Self::start_amending(id, sha, cx);
        }
        Self::show_popup(
            Popup::WarnTaggedCommitBeforeAmend {
                repo: id,
                sha,
                tags,
            },
            cx,
        );
    }

    /// `_startAmendingRepository`: switch to Changes and load the message.
    pub fn start_amending(id: u64, sha: String, cx: &mut dyn Host) {
        let Some(commit) = Self::commit_by_sha(id, &sha, cx) else {
            return;
        };
        Self::show_section(id, Section::Changes, cx);
        Self::state(cx).update(cx, |s, cx| {
            let rs = s.repo_state_mut(id);
            rs.commit_to_amend = Some(commit);
            rs.amend_nonce += 1;
            rs.amend_author = None;
            cx.notify();
        });
    }

    /// `_stopAmendingRepository`
    pub fn stop_amending(id: u64, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            let rs = s.repo_state_mut(id);
            rs.commit_to_amend = None;
            rs.amend_author = None;
            cx.notify();
        });
    }

    /// Corvene `783-amend-author`: the author the commit being amended
    /// gets (`None` keeps its own).
    pub fn set_amend_author(id: u64, author: Option<corvene_git::CommitAuthor>, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            let rs = s.repo_state_mut(id);
            if rs.amend_author != author {
                rs.amend_author = author;
                cx.notify();
            }
        });
    }

    pub fn show_section(id: u64, section: Section, cx: &mut dyn Host) {
        // `798-blame`: the view belongs to the tab it was opened in
        Self::close_blame_unless(id, |b| b.section == section, cx);
        // `1110-repository-insights`: so does Insights
        Self::close_insights_unless(id, |i| i.section == section, cx);
        Self::state(cx).update(cx, |s, cx| {
            if s.selected == Some(id) && s.repo_state_mut(id).section != section {
                s.record_navigation();
            }
            let rs = s.repo_state_mut(id);
            if rs.section != section {
                rs.section = section;
                cx.notify();
            }
        });
        if section == Section::History {
            Self::prefetch_commit_diffs(id, cx);
        }
    }

    // ---- branches (`_createBranch`, `_checkoutBranch`, rename/delete, merge, stash) ----

    pub(crate) fn branch_by_name(
        id: u64,
        name: &str,
        cx: &dyn Host,
    ) -> Option<corvene_models::Branch> {
        let s = Self::state(cx).read(cx);
        let branches = &s.repo_states.get(&id)?.info.as_ref()?.branches;
        branches
            .iter()
            .find(|b| b.name == name && b.kind == corvene_models::BranchKind::Local)
            .or_else(|| branches.iter().find(|b| b.name == name))
            .cloned()
    }

    /// `_createBranch` then checkout (GHD always checks the new branch out).
    pub fn create_branch(
        id: u64,
        name: String,
        start_point: Option<String>,
        unborn: bool,
        cx: &mut dyn Host,
    ) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let branch_name = name.clone();
        let branch_done = name.clone();
        let checkout_options = Self::checkout_options(id, cx);
        // `1202-update-from-parent-branch`: a branch started from another
        // branch than the default one remembers it (VS Code's key)
        let parent = {
            let s = Self::state(cx).read(cx);
            let rs = s.repo_states.get(&id);
            let branches = rs
                .and_then(|r| r.info.as_ref())
                .map(|i| i.branches.as_slice())
                .unwrap_or_default();
            let default = rs.and_then(|r| r.default_branch.as_deref());
            let default_upstream = default
                .and_then(|d| {
                    branches
                        .iter()
                        .find(|b| b.name == d && b.kind == corvene_models::BranchKind::Local)
                })
                .and_then(|b| b.upstream_short());
            start_point
                .clone()
                .filter(|_| s.flags.bool(crate::flags::ids::UPDATE_FROM_PARENT_BRANCH))
                .filter(|sp| branches.iter().any(|b| b.name == *sp))
                .filter(|sp| Some(sp.as_str()) != default && Some(sp.as_str()) != default_upstream)
        };
        // `1222-push-target-guard`: not tracking a remote branch of another
        // name, which Push would then push to
        let no_track = Self::create_branch_no_track(id, &name, start_point.as_deref(), cx);
        // `1114-hook-results`: post-checkout's outcome goes to the hook list
        let hooks = crate::hooks::hook_ui(id, false, Some(format!("Switch to {name}")), cx);
        let task = cx.background_executor().spawn(async move {
            if unborn {
                return corvene_git::checkout_new_branch(git, &workdir, &name);
            }
            corvene_git::create_branch(
                git.clone(),
                &workdir,
                &name,
                start_point.as_deref(),
                no_track,
            )?;
            if let Some(parent) = &parent
                && let Err(err) =
                    corvene_git::set_branch_merge_base(git.clone(), &workdir, &name, parent)
            {
                warn!(%err, "could not record the branch's parent");
            }
            let branch = corvene_models::Branch {
                name: name.clone(),
                kind: corvene_models::BranchKind::Local,
                full_name: format!("refs/heads/{name}"),
                tip: None,
                upstream: None,
                tip_time: None,
                tip_author: None,
                remote_name: None,
            };
            corvene_git::hooks::with_hook_callbacks(&hooks.callbacks, || {
                corvene_git::checkout_branch_with(git, &workdir, &branch, &checkout_options)
            })
        });
        Self::state(cx).update(cx, |s, cx| {
            s.repo_state_mut(id).checkout_target = Some(branch_name);
            cx.notify();
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let result = task.await;
            cx.update(|cx| {
                Self::state(cx).update(cx, |s, _| s.repo_state_mut(id).checkout_target = None);
                match result {
                    // `345-issues`: a branch made from an issue remembers it
                    Ok(()) => Self::record_issue_branch(id, &branch_done, cx),
                    Err(err) => {
                        Self::clear_pending_issue_link(id, cx);
                        Self::show_error("Could not create branch", &err, cx);
                    }
                }
                Self::show_section(id, Section::Changes, cx);
                Self::refresh_repository(id, cx);
            });
        })
        .detach();
    }

    /// `_checkoutBranch`: apply the uncommitted-changes strategy (asking via
    /// `StashAndSwitchBranch` / `ConfirmOverwriteStash` when needed).
    pub fn checkout_branch(
        id: u64,
        name: String,
        explicit: Option<UncommittedChangesStrategy>,
        cx: &mut dyn Host,
    ) {
        let Some(mut branch) = Self::branch_by_name(id, &name, cx) else {
            return;
        };
        let mut name = name;
        // Corvene (`866-remote-checkout-uses-local`): a remote branch whose
        // short name is already a local branch checks the local one out
        // (GHD runs `checkout -b`, which fails with "already exists")
        if branch.kind == corvene_models::BranchKind::Remote
            && Self::state(cx)
                .read(cx)
                .flags
                .bool(crate::flags::ids::REMOTE_CHECKOUT_USES_LOCAL)
            && let Some(local) = Self::branch_by_name(id, branch.name_without_remote(), cx)
                .filter(|b| b.kind == corvene_models::BranchKind::Local)
        {
            name = local.name.clone();
            branch = local;
        }
        let (has_changes, has_stash, tip_valid, current, setting) = {
            let s = Self::state(cx).read(cx);
            let rs = s.repo_states.get(&id);
            let info = rs.and_then(|r| r.info.as_ref());
            (
                rs.and_then(|r| r.status.as_deref())
                    .is_some_and(|st| !st.files.is_empty()),
                rs.is_some_and(|r| r.desktop_stash().is_some()),
                info.is_some_and(|i| matches!(i.tip, corvene_models::Tip::Valid { .. })),
                info.and_then(|i| i.current_branch())
                    .map(|b| b.name.clone()),
                s.settings.uncommitted_changes_strategy,
            )
        };
        if tip_valid && current.as_deref() == Some(branch.name.as_str()) {
            return;
        }
        let mut strategy = explicit.unwrap_or(setting);
        if explicit.is_none()
            && strategy == UncommittedChangesStrategy::StashOnCurrentBranch
            && has_changes
            && has_stash
        {
            Self::show_popup(
                Popup::ConfirmOverwriteStash {
                    repo: id,
                    branch: Some(name),
                },
                cx,
            );
            return;
        }
        if !tip_valid {
            strategy = UncommittedChangesStrategy::MoveToNewBranch;
        }
        if strategy == UncommittedChangesStrategy::AskForConfirmation && has_changes {
            // `1207-switch-warns-target-behind`
            Self::load_switch_target_behind(id, &branch, cx);
            Self::show_popup(
                Popup::StashAndSwitchBranch {
                    repo: id,
                    branch: name,
                },
                cx,
            );
            return;
        }
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let previous_stash = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|r| r.desktop_stash())
            .map(|s| s.sha.clone());
        let target = branch.name.clone();
        // Corvene (`869-stash-protects-assume-unchanged`): no stash while it
        // would reset assume-unchanged files with local changes
        let guard = Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::STASH_PROTECTS_ASSUME_UNCHANGED);
        // GHD 3.6.6 `checkoutBranch` updates every submodule after the
        // checkout; Corvene runs that below, after the stash steps, and
        // reports a failure on its own while the checkout stands
        let submodule_options = Self::checkout_options(id, cx);
        let checkout_options = corvene_git::CheckoutOptions {
            submodules: corvene_git::SubmoduleUpdate::None,
            ..Default::default()
        };
        let git_for_submodules = git.clone();
        let workdir_for_submodules = workdir.clone();
        // Corvene (`729-pop-stash-on-return`): coming back to a branch with
        // a clean working directory restores the stash left on it (GHD keeps
        // it until Restore is clicked)
        let pop_stash_for = (Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::POP_STASH_ON_RETURN)
            && (!has_changes
                || (strategy == UncommittedChangesStrategy::StashOnCurrentBranch
                    && current.is_some())))
        .then(|| (git.clone(), branch.name_without_remote().to_string()));
        // `774-stash-conflict-flow`: a conflicted pop keeps the entry
        let pop_options = Self::stash_pop_options(cx);
        let kept_workdir = workdir.clone();
        // `1114-hook-results`: post-checkout's outcome goes to the hook list
        let hooks = crate::hooks::hook_ui(
            id,
            false,
            Some(format!("Switch to {}", branch.name_without_remote())),
            cx,
        );
        let task = cx.background_executor().spawn(async move {
            let mut kept: Option<(corvene_models::StashEntry, Vec<String>)> = None;
            // `1313-changelists`: the stash `729` restored, for its lists
            let mut popped: Option<String> = None;
            let result =
                corvene_git::hooks::with_hook_callbacks(&hooks.callbacks, || match strategy {
                    UncommittedChangesStrategy::StashOnCurrentBranch => {
                        if let Some(current) = current.as_deref()
                            && has_changes
                        {
                            if guard {
                                corvene_git::ensure_no_modified_assume_unchanged(
                                    git.clone(),
                                    &workdir,
                                )?;
                            }
                            // `createStashAndDropPreviousEntry`: the old entry
                            // goes once the new one is made
                            if corvene_git::create_desktop_stash(
                                git.clone(),
                                &workdir,
                                current,
                                false,
                            )? && let Some(old) = previous_stash
                            {
                                let _ = corvene_git::drop_desktop_stash_entry(
                                    git.clone(),
                                    &workdir,
                                    &old,
                                );
                            }
                        }
                        corvene_git::checkout_branch_with(git, &workdir, &branch, &checkout_options)
                    }
                    _ => {
                        // `checkoutAndBringChanges`: plain checkout, else stash → checkout → pop
                        match corvene_git::checkout_branch_with(
                            git.clone(),
                            &workdir,
                            &branch,
                            &checkout_options,
                        ) {
                            Ok(()) => Ok(()),
                            Err(err) if corvene_git::is_local_changes_overwritten(&err) => {
                                let target = branch.name_without_remote().to_string();
                                if !corvene_git::create_desktop_stash(
                                    git.clone(),
                                    &workdir,
                                    &target,
                                    guard,
                                )? {
                                    return Err(err);
                                }
                                corvene_git::checkout_branch_with(
                                    git.clone(),
                                    &workdir,
                                    &branch,
                                    &checkout_options,
                                )?;
                                if let Some(entry) =
                                    corvene_git::get_last_desktop_stash_entry_for_branch(
                                        git.clone(),
                                        &workdir,
                                        &target,
                                    )?
                                {
                                    let pop = corvene_git::pop_stash_entry_with(
                                        git.clone(),
                                        &workdir,
                                        &entry.sha,
                                        pop_options,
                                    )?;
                                    kept = Self::kept_after_pop(git.clone(), &workdir, &entry, pop);
                                }
                                Ok(())
                            }
                            Err(err) => Err(err),
                        }
                    }
                });
            let submodule_error = result.as_ref().ok().and_then(|()| {
                corvene_git::update_submodules_after_checkout(
                    git_for_submodules,
                    &workdir_for_submodules,
                    &submodule_options,
                )
                .err()
            });
            let pop_error = match (&result, pop_stash_for) {
                (Ok(()), Some((git, target))) => {
                    corvene_git::get_last_desktop_stash_entry_for_branch(
                        git.clone(),
                        &workdir_for_submodules,
                        &target,
                    )
                    .and_then(|entry| match entry {
                        Some(entry) => corvene_git::pop_stash_entry_with(
                            git.clone(),
                            &workdir_for_submodules,
                            &entry.sha,
                            pop_options,
                        )
                        .map(|pop| {
                            popped = Some(entry.sha.clone());
                            kept = Self::kept_after_pop(git, &workdir_for_submodules, &entry, pop);
                        }),
                        None => Ok(()),
                    })
                    .err()
                }
                _ => None,
            };
            (result, submodule_error, pop_error, kept, popped)
        });
        Self::state(cx).update(cx, |s, cx| {
            s.repo_state_mut(id).checkout_target = Some(target);
            s.foldout = None;
            cx.notify();
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let (result, submodule_error, pop_error, kept, popped) = task.await;
            cx.update(|cx| {
                Self::state(cx).update(cx, |s, _| s.repo_state_mut(id).checkout_target = None);
                if let Err(err) = result {
                    Self::show_error("Could not switch branch", &err, cx);
                }
                if let Some(sha) = popped {
                    Self::note_changelist_stash_fate(
                        id,
                        &sha,
                        crate::changelists::StashFate::Restored,
                        cx,
                    );
                }
                if let Some(err) = submodule_error {
                    Self::show_error("Could not update submodules", &err, cx);
                }
                if let Some(err) = pop_error {
                    Self::show_error("Could not restore stash", &err, cx);
                }
                Self::note_stash_pop(id, kept_workdir, kept, cx);
                Self::show_section(id, Section::Changes, cx);
                Self::refresh_repository(id, cx);
            });
        })
        .detach();
    }

    /// `263-submodules-follow-checkout`: when the flag is on, the submodules
    /// to leave alone after a checkout or merge (those the Changes list shows
    /// changed now) and the askpass environment for cloning new ones.
    pub(crate) fn submodule_update_plan(
        id: u64,
        cx: &dyn Host,
    ) -> Option<(Vec<String>, Option<corvene_git::AskpassEnv>)> {
        let s = Self::state(cx).read(cx);
        if !s.flags.bool(crate::flags::ids::SUBMODULES_FOLLOW_CHECKOUT) {
            return None;
        }
        let skip = s
            .repo_states
            .get(&id)
            .and_then(|r| r.status.as_deref())
            .map(|st| {
                st.files
                    .iter()
                    .filter(|f| f.status.submodule)
                    .map(|f| f.path.clone())
                    .collect()
            })
            .unwrap_or_default();
        Some((skip, Self::askpass_env_for_repository(id, cx)))
    }

    /// How a branch checkout's submodules follow it: GHD 3.6.6
    /// `checkoutBranch` updates every one with the askpass environment
    /// (`envForRemoteOperation`); with `263-submodules-follow-checkout` on,
    /// the changed ones are spared ([`Self::submodule_update_plan`]).
    pub(crate) fn checkout_options(id: u64, cx: &dyn Host) -> corvene_git::CheckoutOptions {
        match Self::submodule_update_plan(id, cx) {
            Some((skip, askpass)) => corvene_git::CheckoutOptions {
                submodules: corvene_git::SubmoduleUpdate::AllExcept(skip),
                askpass,
                ..Default::default()
            },
            None => corvene_git::CheckoutOptions {
                askpass: Self::askpass_env_for_repository(id, cx),
                ..Default::default()
            },
        }
    }

    pub fn rename_branch(id: u64, old: String, new: String, cx: &mut dyn Host) {
        let (from, to) = (old.clone(), new.clone());
        Self::run_history_op_then(
            id,
            "Could not rename branch",
            move |git, workdir| corvene_git::rename_branch(git, &workdir, &old, &new),
            // `897-pinned-branches`: a pinned branch stays pinned
            move |cx| {
                Self::state(cx).update(cx, |s, _| {
                    if let Some(repo) = s.repositories.iter_mut().find(|r| r.id == id)
                        && let Some(pin) = repo.pinned_branches.iter_mut().find(|b| **b == from)
                    {
                        *pin = to;
                        persist_repositories(s);
                    }
                })
            },
            cx,
        );
    }

    /// `_deleteBranch`: checks out the default branch first when deleting the
    /// current one, like GHD.
    pub fn delete_branch(id: u64, name: String, include_remote: bool, cx: &mut dyn Host) {
        let Some(branch) = Self::branch_by_name(id, &name, cx) else {
            return;
        };
        let (is_current, default_branch) = {
            let s = Self::state(cx).read(cx);
            let rs = s.repo_states.get(&id);
            (
                rs.and_then(|r| r.info.as_ref())
                    .and_then(|i| i.current_branch())
                    .is_some_and(|b| b.name == name),
                rs.and_then(|r| r.default_branch.clone()),
            )
        };
        let default = if is_current {
            default_branch.and_then(|d| Self::branch_by_name(id, &d, cx))
        } else {
            None
        };
        // Corvene (`861-undo-delete-branch`): a deleted local branch can be
        // recreated from the banner (GHD has no undo)
        let undo = (branch.kind == corvene_models::BranchKind::Local
            && Self::state(cx)
                .read(cx)
                .flags
                .bool(crate::flags::ids::UNDO_DELETE_BRANCH))
        .then(|| branch.tip.clone())
        .flatten()
        .map(|sha| crate::mco::Banner::BranchDeleted {
            repo: id,
            branch: branch.name.clone(),
            sha,
        });
        let (fetch_after, explain_worktrees, qualified) = {
            let flags = &Self::state(cx).read(cx).flags;
            (
                flags.bool(crate::flags::ids::FETCH_AFTER_DELETING_CURRENT_BRANCH),
                flags.bool(crate::flags::ids::EXPLAIN_BRANCH_IN_OTHER_WORKTREE),
                flags.bool(crate::flags::ids::QUALIFIED_PUSH_REFSPECS),
            )
        };
        // Corvene (`867-qualified-push-refspecs`): `:refs/heads/<name>`, so a
        // remote tag of the same name does not make the deletion ambiguous
        let remote_ref = move |name: &str| match qualified {
            true => format!("refs/heads/{name}"),
            false => name.to_string(),
        };
        // Corvene (`862-fetch-after-deleting-current-branch`): the default
        // branch this worktree switched to is brought up to date, so a merged
        // pull request shows up in it (GHD does not fetch)
        let fetch_remote = default
            .as_ref()
            .filter(|_| fetch_after)
            .map(|d| d.upstream_remote_name().map(str::to_owned));
        let deleted = branch.name.clone();
        // GHD runs the checkout as a failable operation of its own: a failed
        // submodule update after it is reported and the branch still deleted
        let submodule_options = default.is_some().then(|| Self::checkout_options(id, cx));
        let submodule_error =
            std::sync::Arc::new(std::sync::Mutex::new(None::<corvene_git::GitError>));
        let submodule_error_bg = submodule_error.clone();
        // GHD `deleteRemoteBranch` pushes with `envForRemoteOperation(remote.url)`:
        // the signed-in accounts' credentials
        let remote_name = match branch.kind {
            corvene_models::BranchKind::Local => branch
                .upstream_remote_name()
                .filter(|_| include_remote)
                .map(str::to_string),
            corvene_models::BranchKind::Remote => Some(
                branch
                    .name
                    .split_once('/')
                    .map_or("origin", |(remote, _)| remote)
                    .to_string(),
            ),
        };
        let askpass = remote_name.and_then(|name| {
            let url = Self::state(cx)
                .read(cx)
                .repo_states
                .get(&id)
                .and_then(|r| r.info.as_ref())
                .and_then(|i| i.remotes.iter().find(|r| r.name == name))
                .map(|r| r.url.clone());
            match url {
                Some(url) => {
                    Self::arm_credential_helper_for(id, &url, cx);
                    Self::askpass_env_for(id, &url, cx)
                }
                None => Self::askpass_env_for_repository(id, cx),
            }
        });
        Self::run_history_op_then(
            id,
            "Could not delete branch",
            move |git, workdir| {
                // Corvene (`863-explain-branch-in-other-worktree`): git's
                // refusal to use a branch another worktree has checked out
                // says what to do instead (GHD shows git's words)
                let explain = |err: corvene_git::GitError, switching: bool| match err
                    .branch_in_other_worktree()
                {
                    Some((other, path)) if explain_worktrees => {
                        corvene_git::GitError::Gix(if switching {
                            format!(
                                "\"{deleted}\" is checked out here, and the default branch \
                                     \"{other}\" cannot be switched to because it is checked out \
                                     in the worktree at {}. Switch to another branch, then \
                                     delete \"{deleted}\".",
                                path.display()
                            )
                        } else {
                            format!(
                                "\"{other}\" is checked out in the worktree at {}. Switch \
                                     that worktree to another branch (or remove it), then delete \
                                     \"{other}\".",
                                path.display()
                            )
                        })
                    }
                    _ => err,
                };
                if let Some(default) = default {
                    let checkout_options = corvene_git::CheckoutOptions {
                        submodules: corvene_git::SubmoduleUpdate::None,
                        ..Default::default()
                    };
                    corvene_git::checkout_branch_with(
                        git.clone(),
                        &workdir,
                        &default,
                        &checkout_options,
                    )
                    .map_err(|err| explain(err, true))?;
                    if let Some(options) = &submodule_options
                        && let Err(err) = corvene_git::update_submodules_after_checkout(
                            git.clone(),
                            &workdir,
                            options,
                        )
                        && let Ok(mut slot) = submodule_error_bg.lock()
                    {
                        *slot = Some(err);
                    }
                }
                match branch.kind {
                    corvene_models::BranchKind::Local => {
                        corvene_git::delete_local_branch(git.clone(), &workdir, &branch.name)
                            .map_err(|err| explain(err, false))?;
                        if include_remote
                            && let (Some(remote), Some(remote_branch)) = (
                                branch.upstream_remote_name(),
                                branch.upstream_without_remote(),
                            )
                        {
                            corvene_git::delete_remote_branch_with(
                                git,
                                &workdir,
                                remote,
                                &remote_ref(remote_branch),
                                askpass.as_ref(),
                            )?;
                        }
                        Ok(())
                    }
                    corvene_models::BranchKind::Remote => {
                        let (remote, remote_branch) = branch
                            .name
                            .split_once('/')
                            .unwrap_or(("origin", &branch.name));
                        corvene_git::delete_remote_branch_with(
                            git,
                            &workdir,
                            remote,
                            &remote_ref(remote_branch),
                            askpass.as_ref(),
                        )
                    }
                }
            },
            move |cx| {
                let submodule_error = submodule_error.lock().ok().and_then(|mut slot| slot.take());
                if let Some(err) = submodule_error {
                    Self::show_error("Could not update submodules", &err, cx);
                }
                if let Some(banner) = undo {
                    Self::set_banner(banner, cx);
                }
                if let Some(remote) = fetch_remote {
                    Self::fetch_remote_then(id, remote.as_deref(), true, |_, _| {}, cx);
                }
            },
            cx,
        );
    }

    /// The "Deleted branch" banner's Undo (`861-undo-delete-branch`):
    /// recreate `branch` at the commit it pointed at.
    pub fn restore_deleted_branch(id: u64, branch: String, sha: String, cx: &mut dyn Host) {
        let restored = branch.clone();
        Self::run_history_op_then(
            id,
            "Could not restore branch",
            move |git, workdir| {
                corvene_git::create_branch(git, &workdir, &branch, Some(&sha), true)
            },
            move |cx| Self::set_banner(crate::mco::Banner::BranchRestored { branch: restored }, cx),
            cx,
        );
    }

    /// Delete Branch dialog warnings (`860-delete-branch-warnings`): commits
    /// only this branch has, and a stash recorded for it.
    pub fn preview_delete_branch(id: u64, name: String, cx: &mut dyn Host) {
        let Some(branch) = Self::branch_by_name(id, &name, cx) else {
            return;
        };
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        // the default branch (local and upstream) and the branch's upstream,
        // where those refs exist
        let bases: Vec<(String, String)> = {
            let s = Self::state(cx).read(cx);
            let rs = s.repo_states.get(&id);
            let branches = rs
                .and_then(|r| r.info.as_ref())
                .map(|i| i.branches.as_slice())
                .unwrap_or_default();
            let default = rs.and_then(|r| r.default_branch.as_deref()).and_then(|d| {
                branches
                    .iter()
                    .find(|b| b.name == d && b.kind == corvene_models::BranchKind::Local)
            });
            let mut full_names: Vec<&str> = Vec::new();
            if let Some(default) = default {
                full_names.push(&default.full_name);
                full_names.extend(default.upstream.as_deref());
            }
            full_names.extend(branch.upstream.as_deref());
            full_names.dedup();
            full_names
                .into_iter()
                .filter(|f| *f != branch.full_name)
                .filter_map(|f| {
                    branches
                        .iter()
                        .find(|b| b.full_name == f)
                        .map(|b| (b.full_name.clone(), b.name.clone()))
                })
                .collect()
        };
        let local = branch.kind == corvene_models::BranchKind::Local;
        let task = cx.background_executor().spawn(async move {
            let unmerged = if bases.is_empty() {
                0
            } else {
                let refs: Vec<String> = bases.iter().map(|(f, _)| f.clone()).collect();
                corvene_git::commits_not_in(git.clone(), &workdir, &branch.full_name, &refs)
                    .unwrap_or(0)
            };
            let has_stash = local
                && corvene_git::get_last_desktop_stash_entry_for_branch(
                    git,
                    &workdir,
                    &branch.name,
                )
                .is_ok_and(|entry| entry.is_some());
            crate::state::DeleteBranchPreview {
                branch: branch.name,
                unmerged_commits: unmerged,
                compared_to: bases.into_iter().map(|(_, short)| short).collect(),
                has_stash,
            }
        });
        Self::state(cx).update(cx, |s, _| s.repo_state_mut(id).delete_branch_preview = None);
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let preview = task.await;
            cx.update(|cx| {
                Self::state(cx).update(cx, |s, cx| {
                    s.repo_state_mut(id).delete_branch_preview = Some(preview);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Merge dialog preview: how many commits `branch` would bring in.
    pub fn preview_merge(id: u64, branch: String, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let current = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|r| r.info.as_ref())
            .and_then(|i| i.current_branch())
            .map(|b| b.name.clone());
        let Some(current) = current else { return };
        let name = branch.clone();
        let task = cx.background_executor().spawn(async move {
            let count =
                corvene_git::commits_ahead(git.clone(), &workdir, &current, &branch).unwrap_or(0);
            let mergeability =
                corvene_git::determine_mergeability(git, &workdir, &current, &branch).ok();
            (count, mergeability)
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let (count, mergeability) = task.await;
            cx.update(|cx| {
                Self::state(cx).update(cx, |s, cx| {
                    s.repo_state_mut(id).merge_preview = Some(crate::mco::MergePreview {
                        branch: name,
                        commits: count,
                        mergeability,
                    });
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Branch › Update from Default Branch: merge the default branch in.
    ///
    /// Deviation (`858-update-from-default-fetches`): GHD merges the local
    /// default branch as it is (`app/src/ui/app.tsx`
    /// `updateBranchWithContributionTargetBranch`), which may be behind its
    /// remote; with the flag on, its remote is fetched first and the
    /// remote-tracking branch is merged.
    ///
    /// Deviation (`1202-update-from-parent-branch`): a branch created from
    /// another branch than the default one is updated from that branch
    /// (`branch.<name>.vscode-merge-base`, VS Code's key) while it exists.
    pub fn update_from_default_branch(id: u64, cx: &mut dyn Host) {
        let (default, fetch_first) = {
            let s = Self::state(cx).read(cx);
            let rs = s.repo_states.get(&id);
            (
                // `1202-update-from-parent-branch`: the branch this one was
                // created from (GHD: always the default branch)
                rs.and_then(|r| {
                    r.update_parent
                        .clone()
                        .filter(|_| s.flags.bool(crate::flags::ids::UPDATE_FROM_PARENT_BRANCH))
                })
                .or_else(|| rs.and_then(|r| r.default_branch.clone())),
                s.flags.bool(crate::flags::ids::UPDATE_FROM_DEFAULT_FETCHES),
            )
        };
        let Some(default) = default else { return };
        let tracking = fetch_first
            .then(|| Self::branch_by_name(id, &default, cx))
            .flatten()
            .and_then(|b| match b.kind {
                corvene_models::BranchKind::Local => Some((
                    b.upstream_remote_name()?.to_owned(),
                    b.upstream_short()?.to_owned(),
                )),
                corvene_models::BranchKind::Remote => {
                    let remote = b
                        .remote_name
                        .clone()
                        .or_else(|| b.name.split_once('/').map(|(r, _)| r.to_owned()))?;
                    Some((remote, b.name))
                }
            })
            // the remote-tracking branch must exist to be merged
            .filter(|(_, short)| Self::branch_by_name(id, short, cx).is_some());
        match tracking {
            Some((remote, short)) => Self::fetch_remote_then(
                id,
                Some(&remote),
                false,
                move |fetched, cx| {
                    if fetched {
                        Self::update_from_branch(id, short, cx);
                    }
                },
                cx,
            ),
            None => Self::update_from_branch(id, default, cx),
        }
    }

    /// Bring `branch` into the current branch for Update from Default Branch.
    ///
    /// Deviation (`859-update-from-default-rebases`): with `pull.rebase` set
    /// the current branch is rebased onto it (GHD always merges).
    fn update_from_branch(id: u64, branch: String, cx: &mut dyn Host) {
        let rebase = {
            let s = Self::state(cx).read(cx);
            s.flags.bool(crate::flags::ids::UPDATE_FROM_DEFAULT_REBASES)
                && s.repo_states.get(&id).is_some_and(|r| r.pull_with_rebase)
        };
        if rebase {
            Self::rebase_onto(id, branch, cx);
        } else {
            Self::merge_branch(id, branch, false, cx);
        }
    }

    /// Branch › Stash All Changes (`createStashForCurrentBranch`): asks
    /// `ConfirmOverwriteStash` first when the branch has a stash.
    pub fn stash_all_changes(id: u64, cx: &mut dyn Host) {
        Self::create_stash_for_current_branch(id, true, cx);
    }

    /// GHD `_createStashForCurrentBranch(repository, showConfirmationDialog)`:
    /// stash every change on the current branch, then drop the branch's
    /// previous stash (`createStashAndDropPreviousEntry`).
    pub fn create_stash_for_current_branch(id: u64, show_confirmation: bool, cx: &mut dyn Host) {
        let current = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|r| r.info.as_ref())
            .and_then(|i| i.current_branch())
            .map(|b| b.name.clone());
        let Some(current) = current else { return };
        let previous = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|r| r.desktop_stash())
            .map(|s| s.sha.clone());
        if show_confirmation && previous.is_some() {
            Self::show_popup(
                Popup::ConfirmOverwriteStash {
                    repo: id,
                    branch: None,
                },
                cx,
            );
            return;
        }
        let guard = Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::STASH_PROTECTS_ASSUME_UNCHANGED);
        Self::run_history_op(
            id,
            "Could not stash changes",
            move |git, workdir| {
                if guard {
                    corvene_git::ensure_no_modified_assume_unchanged(git.clone(), &workdir)?;
                }
                if corvene_git::create_desktop_stash(git.clone(), &workdir, &current, false)?
                    && let Some(old) = previous
                {
                    let _ = corvene_git::drop_desktop_stash_entry(git, &workdir, &old);
                }
                Ok(())
            },
            cx,
        );
    }

    // ---- stash viewer (`_selectStashedFile`, `popStash`, `dropStash`) ----

    /// The "Stashed Changes" row / View › Toggle Stashed Changes.
    pub fn toggle_stash_view(id: u64, cx: &mut dyn Host) {
        let show = Self::state(cx).update(cx, |s, cx| {
            let rs = s.repo_state_mut(id);
            let was_showing = rs.leave_stash();
            cx.notify();
            rs.showing_stash = !was_showing && rs.stash.is_some();
            rs.showing_stash
        });
        if show {
            // GHD `_selectStashedFile` without a file: the first one
            let loaded = Self::state(cx).update(cx, |s, _| {
                let rs = s.repo_state_mut(id);
                let first = rs
                    .stash_files
                    .as_ref()
                    .map(|f| f.first().map(|f| f.path.clone()));
                let loaded = first.is_some();
                if let Some(first) = first {
                    rs.stash_selected_file = first;
                }
                loaded
            });
            if loaded {
                Self::load_stash_diff(id, cx);
            } else {
                Self::load_stash_files(id, cx);
            }
        }
    }

    /// GHD `GitStore.loadFilesForCurrentStashEntry`: the current stash's
    /// files, once per stash; the first is selected and, while the stash
    /// is showing, its diff loaded.
    pub(crate) fn load_stash_files(id: u64, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let Some(sha) = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|r| r.shown_stash())
            .map(|s| s.sha.clone())
        else {
            return;
        };
        let started = Self::state(cx).update(cx, |s, _| {
            let rs = s.repo_state_mut(id);
            if rs.stash_files_sha.as_ref() == Some(&sha) {
                return false;
            }
            rs.stash_files_sha = Some(sha.clone());
            true
        });
        if !started {
            return;
        }
        let sha_for_task = sha.clone();
        // Corvene `1319-trivial-change-icon`
        let trivial = Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::TRIVIAL_CHANGE_ICON);
        let task = cx.background_executor().spawn(async move {
            corvene_git::stashed_files(git, &workdir, &sha_for_task).map(|mut data| {
                if !trivial {
                    return data;
                }
                data.trivial = corvene_git::trivial_change::classify_committed(
                    &workdir,
                    &format!("{sha_for_task}^1"),
                    &sha_for_task,
                    &data.files,
                );
                data
            })
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let result = task.await;
            cx.update(|cx| {
                let load = Self::state(cx).update(cx, |s, cx| {
                    let rs = s.repo_state_mut(id);
                    if rs.shown_stash().map(|s| &s.sha) != Some(&sha) {
                        return false;
                    }
                    match result {
                        Ok(data) => {
                            rs.stash_selected_file = data.files.first().map(|f| f.path.clone());
                            rs.stash_trivial = data.trivial;
                            rs.stash_files = Some(data.files);
                        }
                        Err(err) => {
                            warn!(id, %err, "stashed files failed");
                            rs.stash_files_sha = None;
                        }
                    }
                    cx.notify();
                    rs.showing_stash
                });
                if load {
                    Self::load_stash_diff(id, cx);
                }
            });
        })
        .detach();
    }

    pub fn select_stash_file(id: u64, path: String, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            s.repo_state_mut(id).stash_selected_file = Some(path);
            cx.notify();
        });
        Self::load_stash_diff(id, cx);
    }

    pub(crate) fn load_stash_diff(id: u64, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        // An untracked file of a `stash -u` entry carries the entry's
        // untracked-files commit as its commitish (`stashed_files`), so its
        // contents are read from that commit, not the stash commit.
        let Some((stash_sha, file)) =
            Self::state(cx)
                .read(cx)
                .repo_states
                .get(&id)
                .and_then(|rs| {
                    let path = rs.stash_selected_file.as_ref()?;
                    let file = rs
                        .stash_files
                        .as_ref()?
                        .iter()
                        .find(|f| &f.path == path)
                        .cloned()?;
                    Some((rs.shown_stash()?.sha.clone(), file))
                })
        else {
            return;
        };
        let key = (stash_sha, file.path.clone());
        let hide_whitespace = Self::state(cx)
            .read(cx)
            .settings
            .hide_whitespace_in_history_diff;
        let task = cx.background_executor().spawn(async move {
            let diff = corvene_git::stash_file_diff(git.clone(), &workdir, &file, hide_whitespace);
            let contents = (file.status.kind != corvene_models::FileStatusKind::Deleted)
                .then(|| {
                    corvene_git::blob_lines(git.clone(), &workdir, &file.commitish, &file.path)
                })
                .flatten();
            let old = (!matches!(
                file.status.kind,
                corvene_models::FileStatusKind::New | corvene_models::FileStatusKind::Untracked
            ))
            .then(|| {
                let parent = format!("{}^", file.commitish);
                let old_path = file.old_path.as_deref().unwrap_or(&file.path);
                corvene_git::blob_lines(git, &workdir, &parent, old_path)
            })
            .flatten();
            (diff, (contents, old))
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let (result, (contents, old)) = task.await;
            cx.update(|cx| {
                Self::state(cx).update(cx, |s, cx| {
                    let rs = s.repo_state_mut(id);
                    if rs.shown_stash().map(|s| s.sha.as_str()) != Some(key.0.as_str())
                        || rs.stash_selected_file.as_deref() != Some(key.1.as_str())
                    {
                        return;
                    }
                    let diff = result.unwrap_or_else(|err| {
                        warn!(id, %err, "stash diff failed");
                        corvene_models::Diff::Empty
                    });
                    if replace_diff(
                        (
                            &mut rs.stash_diff,
                            &mut rs.stash_diff_contents,
                            &mut rs.stash_diff_old_contents,
                        ),
                        (Arc::new(diff), contents.map(Arc::new), old.map(Arc::new)),
                    ) {
                        rs.stash_diff_generation += 1;
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Restore: `git stash pop`, then the files show up in Changes.
    pub fn pop_stash(id: u64, cx: &mut dyn Host) {
        let s = Self::state(cx).read(cx);
        let Some(rs) = s.repo_states.get(&id) else {
            return;
        };
        let Some(stash) = rs.shown_stash().cloned() else {
            return;
        };
        // Corvene (`868-stash-restore-checks-branch`): the stash is popped by
        // its commit and only while its branch is still checked out (GHD pops
        // `stash@{n}` from the last refresh, which may be another branch's);
        // an entry picked in the stash list (`797-stash-list`) is restored
        // onto whichever branch is checked out
        let check_branch = s.flags.bool(crate::flags::ids::STASH_RESTORE_CHECKS_BRANCH)
            && rs.viewed_stash.is_none();
        // `774-stash-conflict-flow` / `775-stash-restore-unstages-new-files`
        Self::pop_stash_with_options(id, stash, check_branch, cx);
    }

    /// Discard: confirm unless the user opted out (`askForConfirmationOnDiscardStash`).
    pub fn request_drop_stash(id: u64, cx: &mut dyn Host) {
        if Self::state(cx).read(cx).settings.confirm_discard_stash {
            Self::show_popup(Popup::ConfirmDiscardStash { repo: id }, cx);
        } else {
            Self::drop_stash(id, cx);
        }
    }

    pub fn drop_stash(id: u64, cx: &mut dyn Host) {
        let s = Self::state(cx).read(cx);
        let Some(stash) = s
            .repo_states
            .get(&id)
            .and_then(|r| r.shown_stash())
            .cloned()
        else {
            return;
        };
        // `797-stash-list`: with an Undo banner
        if s.flags.bool(crate::flags::ids::STASH_LIST) {
            Self::discard_stash_entry(id, stash, cx);
            return;
        }
        let sha = stash.sha;
        let dropped = sha.clone();
        Self::run_history_op_then(
            id,
            "Could not discard stash",
            move |git, workdir| corvene_git::drop_desktop_stash_entry(git, &workdir, &sha),
            move |cx| {
                Self::note_changelist_stash_fate(
                    id,
                    &dropped,
                    crate::changelists::StashFate::Dropped,
                    cx,
                )
            },
            cx,
        );
    }

    pub fn set_commit_summary_expanded(id: u64, expanded: bool, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            s.repo_state_mut(id).commit_summary_expanded = expanded;
            cx.notify();
        });
    }

    /// Toggle the include checkbox of one file (`_changeFileIncluded`).
    pub fn toggle_file_included(id: u64, path: String, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            if let Some(status) = s.repo_state_mut(id).status.as_mut().map(Arc::make_mut) {
                if let Some(f) = status.files.iter_mut().find(|f| f.path == path) {
                    // GHD: an indeterminate checkbox click checks it (Partial -> All)
                    f.selection = if f.selection.kind() == DiffSelectionType::All {
                        f.selection.select_none()
                    } else {
                        f.selection.select_all()
                    };
                }
                crate::drafts::note_excluded(s, id, cx);
                cx.notify();
            }
        });
    }

    /// Gutter click: toggle one diff line (`withToggleLineSelection`).
    pub fn toggle_diff_line(id: u64, path: String, line: u32, cx: &mut dyn Host) {
        Self::update_selection(id, &path, cx, |sel| sel.toggled(line));
    }

    /// Hunk handle / drag selection: mark `len` lines from `from`.
    pub fn set_diff_lines(
        id: u64,
        path: String,
        from: u32,
        len: u32,
        selected: bool,
        cx: &mut dyn Host,
    ) {
        Self::update_selection(id, &path, cx, |sel| sel.with_range(from, len, selected));
    }

    fn update_selection(
        id: u64,
        path: &str,
        cx: &mut dyn Host,
        edit: impl FnOnce(&corvene_models::DiffSelection) -> corvene_models::DiffSelection,
    ) {
        Self::state(cx).update(cx, |s, cx| {
            if let Some(f) = s
                .repo_state_mut(id)
                .status
                .as_mut()
                .map(Arc::make_mut)
                .and_then(|st| st.files.iter_mut().find(|f| f.path == path))
            {
                f.selection = edit(&f.selection);
                let partial = f.selection.kind() == DiffSelectionType::Partial;
                // `790-selection-follows-lines`: the lines are the shown diff's
                if s.flags.bool(crate::flags::ids::SELECTION_FOLLOWS_LINES) {
                    let rs = s.repo_state_mut(id);
                    match rs.diff.clone() {
                        Some(diff) if partial && rs.selected_file.as_deref() == Some(path) => {
                            rs.selection_bases.insert(path.to_string(), diff);
                        }
                        _ => {
                            rs.selection_bases.remove(path);
                        }
                    }
                }
                crate::drafts::note_excluded(s, id, cx);
                cx.notify();
            }
        });
    }

    /// GHD `onIncludeChanged(files, include)`: the header checkbox applies to
    /// the files currently visible through the filter.
    pub fn set_files_included(id: u64, paths: Vec<String>, include: bool, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            if let Some(status) = s.repo_state_mut(id).status.as_mut().map(Arc::make_mut) {
                // a set: the header checkbox passes every visible path
                let paths: std::collections::HashSet<&str> =
                    paths.iter().map(String::as_str).collect();
                for f in status
                    .files
                    .iter_mut()
                    .filter(|f| paths.contains(f.path.as_str()))
                {
                    f.selection = if include {
                        f.selection.select_all()
                    } else {
                        f.selection.select_none()
                    };
                }
                crate::drafts::note_excluded(s, id, cx);
                cx.notify();
            }
        });
    }

    /// Filter Options popover checkbox.
    pub fn toggle_filter_option(id: u64, option: crate::state::FilterOption, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            let f = &mut s.repo_state_mut(id).file_list_filter;
            let on = !f.get(option);
            f.set(option, on);
            cx.notify();
        });
    }

    /// "Clear filters" (the text box is cleared by the view).
    pub fn clear_filter_options(id: u64, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            s.repo_state_mut(id).file_list_filter = Default::default();
            cx.notify();
        });
    }

    /// Header checkbox (`_changeIncludeAllFiles`).
    pub fn toggle_include_all(id: u64, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            if let Some(status) = s.repo_state_mut(id).status.as_mut().map(Arc::make_mut) {
                let select_all = status.include_all() != Some(true);
                for f in &mut status.files {
                    f.selection = if select_all {
                        f.selection.select_all()
                    } else {
                        f.selection.select_none()
                    };
                }
                crate::drafts::note_excluded(s, id, cx);
                cx.notify();
            }
        });
    }

    /// Native folder picker → `add_repository` (GHD `AddRepository` shortcut).
    pub fn prompt_add_repository(cx: &mut dyn Host) {
        let receiver = cx.prompt_for_paths(crate::host::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(
                if cfg!(target_os = "macos") {
                    "Add Repository"
                } else {
                    "Add repository"
                }
                .into(),
            ),
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let picked = match receiver.await {
                Some(paths) => paths.into_iter().next(),
                _ => None,
            };
            if let Some(path) = picked {
                cx.update(|cx| Self::add_repository(path, cx));
            }
        })
        .detach();
    }

    // ---- create / clone ----

    /// `221-add-license`: write the named license template to `LICENSE` in
    /// the repository's worktree, filled in like Create a New Repository
    /// does. An existing license file is never replaced.
    pub fn add_license(repo: u64, license: String, cx: &mut dyn Host) {
        let state = Self::state(cx);
        let (Some(git), Some(repository)) = (
            state.read(cx).git.clone(),
            state.read(cx).repository(repo).cloned(),
        ) else {
            return;
        };
        let Some(body) = crate::templates::licenses()
            .into_iter()
            .find(|l| l.name == license)
            .map(|l| l.body)
        else {
            return;
        };
        Self::close_popup(cx);
        let dir = repository.path.clone();
        let project = corvene_models::dir_name(&dir);
        let task = cx.background_executor().spawn(async move {
            let identity = corvene_git::global_identity(git);
            let year = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| 1970 + d.as_secs() / 31_556_952)
                .unwrap_or(1970);
            let text = crate::templates::render_license(
                &body,
                &crate::templates::LicenseFields {
                    fullname: identity.name.unwrap_or_default(),
                    email: identity.email.unwrap_or_default(),
                    project,
                    description: String::new(),
                    year: year.to_string(),
                },
            );
            crate::templates::write_license(&dir, &text)
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let result = task.await;
            cx.update(|cx| match result {
                Ok(_) => Self::refresh_repository(repo, cx),
                Err(message) => Self::show_error("Couldn't add the license", &message, cx),
            })
        })
        .detach();
    }

    /// GHD `CreateRepository` dialog submit: `git init` (+ README commit), then add.
    /// `keep_existing` leaves files already in the folder alone
    /// (`220-create-repository-in-folder`).
    #[allow(clippy::too_many_arguments)]
    pub fn create_repository(
        path: PathBuf,
        name: String,
        description: Option<String>,
        readme: bool,
        gitignore: Option<String>,
        license: Option<String>,
        keep_existing: bool,
        cx: &mut dyn Host,
    ) {
        let state = Self::state(cx);
        let Some(git) = state.read(cx).git.clone() else {
            Self::show_error("Git is not available", "Install git and retry.", cx);
            return;
        };
        Self::close_popup(cx);
        let gitignore_text = gitignore
            .as_deref()
            .and_then(crate::templates::gitignore_text);
        let license_body = license.as_deref().and_then(|name| {
            crate::templates::licenses()
                .into_iter()
                .find(|l| l.name == name)
                .map(|l| l.body)
        });
        let failed_path = path.clone();
        // Corvene (`286-initial-commit-skips-large-files`)
        let large_file_limit = state
            .read(cx)
            .flags
            .bool(crate::flags::ids::INITIAL_COMMIT_SKIPS_LARGE_FILES)
            .then_some(corvene_git::RECEIVE_LIMIT);
        let task = cx.background_executor().spawn(async move {
            let default_branch = corvene_git::configured_default_branch(git.clone());
            let license_text = license_body.map(|body| {
                let identity = corvene_git::global_identity(git.clone());
                let year = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| 1970 + d.as_secs() / 31_556_952)
                    .unwrap_or(1970);
                crate::templates::render_license(
                    &body,
                    &crate::templates::LicenseFields {
                        fullname: identity.name.unwrap_or_default(),
                        email: identity.email.unwrap_or_default(),
                        project: name.clone(),
                        description: String::new(),
                        year: year.to_string(),
                    },
                )
            });
            corvene_git::init_repository_with(
                git,
                InitOptions {
                    path,
                    default_branch: Some(default_branch),
                    description,
                    readme,
                    gitignore: gitignore_text,
                    license: license_text,
                    git_attributes: Some(crate::templates::GIT_ATTRIBUTES.to_string()),
                    keep_existing,
                },
                large_file_limit,
            )
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let result = task.await;
            cx.update(|cx| match result {
                Ok((path, left_out)) => {
                    Self::add_repository(path, cx);
                    if !left_out.is_empty() {
                        Self::show_error(
                            "Large files left out of the initial commit",
                            large_files_left_out_message(&left_out),
                            cx,
                        );
                    }
                }
                Err(err) => {
                    Self::take_pending_alias(&failed_path, cx);
                    Self::show_error("Could not create repository", &err, cx)
                }
            });
        })
        .detach();
    }

    /// GHD `cloneRepository`: streams progress into `AppState::cloning` (GHD
    /// `CloningRepositoriesStore.clone`),
    /// adds the repository when done.
    pub fn clone_repository(
        url: String,
        path: PathBuf,
        default_branch: Option<String>,
        cx: &mut dyn Host,
    ) {
        Self::clone_repository_with(url, path, default_branch, None, cx);
    }

    /// `clone_repository`, `depth` making a shallow clone
    /// (`233-shallow-clone`, the Clone dialog's checkbox).
    pub fn clone_repository_with(
        url: String,
        path: PathBuf,
        default_branch: Option<String>,
        depth: Option<u32>,
        cx: &mut dyn Host,
    ) {
        if Self::state(cx).read(cx).git.is_none() {
            Self::show_error("Git is not available", "Install git and retry.", cx);
            return;
        }
        Self::close_popup(cx);
        let (failed_url, failed_path) = (url.clone(), path.clone());
        let retry = crate::proxy::ProxyRetry::Clone {
            url: url.clone(),
            path: path.clone(),
            default_branch: default_branch.clone(),
            depth,
        };
        Self::start_clone(
            url,
            path,
            default_branch,
            depth,
            None,
            move |outcome, cx| match outcome {
                CloneOutcome::Added | CloneOutcome::Cancelled => {}
                CloneOutcome::SubmodulesFailed(err) => Self::show_error(
                    "Some submodules could not be cloned",
                    submodules_failed_message(&err),
                    cx,
                ),
                CloneOutcome::Failed(err) => {
                    if !Self::clone_proxy_login(&failed_url, &err, retry, cx) {
                        Self::show_clone_error(failed_url, failed_path, err, cx)
                    }
                }
            },
            cx,
        );
    }

    /// `528-proxy-credentials`: a clone the proxy refused with 407 asks for
    /// the proxy's credentials (then `retry`); false when it was something
    /// else or the flag is off.
    fn clone_proxy_login(
        url: &str,
        err: &GitError,
        retry: crate::proxy::ProxyRetry,
        cx: &mut dyn Host,
    ) -> bool {
        let s = Self::state(cx).read(cx);
        let Some(git) = s.git.clone() else {
            return false;
        };
        if corvene_git::remote_failure(err)
            != corvene_git::RemoteFailure::ProxyAuthenticationRequired
            || !s.flags.bool(crate::flags::ids::PROXY_CREDENTIALS)
        {
            return false;
        }
        Self::git_proxy_auth_required(git, None, url.to_string(), retry, true, cx);
        true
    }

    /// A clone failed: `235-clone-failure-keeps-input` reopens the dialog,
    /// else GHD's "Clone failed" error.
    fn show_clone_error(url: String, path: PathBuf, err: GitError, cx: &mut dyn Host) {
        let flags = &Self::state(cx).read(cx).flags;
        // `255-plain-language-remote-errors`
        let explanation = flags
            .bool(crate::flags::ids::PLAIN_LANGUAGE_REMOTE_ERRORS)
            .then(|| crate::push_errors::plain_clone_error(&err, &path))
            .flatten();
        // `235-clone-failure-keeps-input`: back to the dialog
        if flags.bool(crate::flags::ids::CLONE_FAILURE_KEEPS_INPUT) {
            let error = match explanation {
                Some(explanation) => format!("{explanation}\n\n{err}"),
                None => err.to_string(),
            };
            Self::show_popup(Popup::CloneRepositoryRetry { url, path, error }, cx)
        } else {
            Self::show_error(
                "Clone failed",
                ErrorMessage::explained(&err, explanation),
                cx,
            )
        }
    }

    /// Corvene (`293-clone-multiple`): clone `items` (what the Clone
    /// dialog's URL field would hold, and the destination) one after the
    /// other, each resolved like a single clone (`resolve_clone_info`); the
    /// cloning view says "Cloning 2 of 5". Each finished clone is added as it
    /// completes, Cancel stops the queue, and the failures are reported
    /// together at the end. GHD clones one repository at a time.
    pub fn clone_repositories(
        items: Vec<(String, PathBuf)>,
        prefer_ssh: bool,
        depth: Option<u32>,
        cx: &mut dyn Host,
    ) {
        if Self::state(cx).read(cx).git.is_none() {
            Self::show_error("Git is not available", "Install git and retry.", cx);
            return;
        }
        Self::close_popup(cx);
        Self::clone_queue_step(
            std::rc::Rc::new(items),
            0,
            prefer_ssh,
            depth,
            Vec::new(),
            cx,
        );
    }

    fn clone_queue_step(
        items: std::rc::Rc<Vec<(String, PathBuf)>>,
        ix: usize,
        prefer_ssh: bool,
        depth: Option<u32>,
        mut failures: Vec<String>,
        cx: &mut dyn Host,
    ) {
        let Some((input, path)) = items.get(ix).cloned() else {
            if !failures.is_empty() {
                Self::show_error(
                    "Some repositories could not be cloned",
                    failures.join("\n\n"),
                    cx,
                );
            }
            return;
        };
        let total = items.len();
        let name = corvene_git::repository_name_from_url(&input).unwrap_or_else(|| input.clone());
        Self::resolve_clone_info(
            input,
            prefer_ssh,
            move |resolved, cx| {
                let info = match resolved {
                    Ok(info) => info,
                    Err(message) => {
                        Self::take_pending_alias(&path, cx);
                        failures.push(format!("{name}: {message}"));
                        return Self::clone_queue_step(
                            items,
                            ix + 1,
                            prefer_ssh,
                            depth,
                            failures,
                            cx,
                        );
                    }
                };
                // `527-multiple-accounts`: the account that found it, unless
                // the dialog named one
                if let Some(login) = info.account.clone()
                    && Self::pending_account(&path, cx).is_none()
                {
                    Self::account_when_added(&path, login, cx);
                }
                let clone_url = info.url.clone();
                Self::start_clone(
                    info.url,
                    path,
                    info.default_branch,
                    depth,
                    Some((ix + 1, total)),
                    move |outcome, cx| {
                        match outcome {
                            CloneOutcome::Added => {}
                            // the rest of the queue stops too
                            CloneOutcome::Cancelled => {
                                if !failures.is_empty() {
                                    Self::show_error(
                                        "Some repositories could not be cloned",
                                        failures.join("\n\n"),
                                        cx,
                                    );
                                }
                                return;
                            }
                            CloneOutcome::SubmodulesFailed(_) => failures.push(format!(
                                "{name}: cloned, but some of its submodules could not be cloned"
                            )),
                            CloneOutcome::Failed(err) => {
                                // `528-proxy-credentials`: ask, the queue goes on
                                Self::clone_proxy_login(
                                    &clone_url,
                                    &err,
                                    crate::proxy::ProxyRetry::None,
                                    cx,
                                );
                                failures.push(format!("{name}: {err}"))
                            }
                        }
                        Self::clone_queue_step(items, ix + 1, prefer_ssh, depth, failures, cx);
                    },
                    cx,
                );
            },
            cx,
        );
    }

    /// Run one clone: progress into `AppState::cloning` (`queue`: its place
    /// in a `293-clone-multiple` queue), the repository added when it is
    /// done, then `then` with the outcome.
    fn start_clone(
        url: String,
        path: PathBuf,
        default_branch: Option<String>,
        depth: Option<u32>,
        queue: Option<(usize, usize)>,
        then: impl FnOnce(CloneOutcome, &mut dyn Host) + 'static,
        cx: &mut dyn Host,
    ) {
        let state = Self::state(cx);
        let Some(git) = state.read(cx).git.clone() else {
            Self::show_error("Git is not available", "Install git and retry.", cx);
            return;
        };
        // GHD `new CloningRepository(path, url)`
        let mut clone = CloneState::new(path.clone(), url.clone());
        clone.description = "Cloning…".into();
        clone.queue = queue;
        let clone_id = clone.id;
        let cancel = clone.cancel.clone();
        state.update(cx, |s, cx| {
            s.cloning.push(clone);
            cx.notify();
        });

        // GHD `envForRemoteOperation`: the signed-in accounts' credentials
        // (and the stalled-transfer timeout of `network-stall-timeout`);
        // `527-multiple-accounts`: the account the clone was picked from
        Self::arm_credential_helper(&url, cx);
        let askpass = match Self::pending_account(&path, cx) {
            Some(login) => Self::askpass_env_preferring(&url, &login, cx),
            None => Self::askpass_env(cx),
        };
        let options = corvene_git::CloneOptions {
            default_branch,
            depth,
            askpass,
            // `281-clone-updating-files-step`
            updating_files_step: state
                .read(cx)
                .flags
                .bool(crate::flags::ids::CLONE_UPDATING_FILES_STEP),
            ..corvene_git::CloneOptions::default()
        };
        let (tx, rx) = std::sync::mpsc::channel::<corvene_git::CloneProgress>();
        let clone_path = path.clone();
        let clone_url = url.clone();
        let task = cx.background_executor().spawn(async move {
            let result = corvene_git::clone_with_options(
                git,
                &clone_url,
                &clone_path,
                &options,
                Some(cancel),
                |p| {
                    let _ = tx.send(p);
                },
            );
            // `285-clone-keeps-repo-on-submodule-failure`
            let kept = result
                .as_ref()
                .is_err_and(|err| corvene_git::clone_failed_in_submodule(&clone_path, err));
            (result, kept)
        });
        // Progress pump: poll the channel on the foreground at ~30 Hz while cloning.
        let pump_state = state;
        cx.spawn(async move |cx: &mut AsyncCtx| {
            loop {
                let mut latest = None;
                while let Ok(p) = rx.try_recv() {
                    latest = Some(p);
                }
                if let Some(p) = latest {
                    let done = pump_state.update(cx, |s, cx| {
                        let updated = s.cloning.update_progress(clone_id, p.description, p.value);
                        if updated {
                            cx.notify();
                        }
                        !updated
                    });
                    if done {
                        break;
                    }
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(33))
                    .await;
                if pump_state.read_with(cx, |s, _| s.cloning.get(clone_id).is_none()) {
                    break;
                }
            }
        })
        .detach();

        cx.spawn(async move |cx: &mut AsyncCtx| {
            let (result, kept) = task.await;
            cx.update(|cx| {
                Self::state(cx).update(cx, |s, cx| {
                    s.cloning.remove_id(clone_id);
                    cx.notify();
                });
                // Corvene (`285-clone-keeps-repo-on-submodule-failure`): the
                // repository was cloned and only a submodule failed; GHD
                // drops it
                let keep = kept
                    && Self::state(cx)
                        .read(cx)
                        .flags
                        .bool(crate::flags::ids::CLONE_KEEPS_REPO_ON_SUBMODULE_FAILURE);
                let outcome = match result {
                    Err(err) if keep => {
                        Self::add_repository_then(path, cx, Self::resume_open_in_desktop);
                        CloneOutcome::SubmodulesFailed(err)
                    }
                    // an `openRepo` URL waiting for this clone continues
                    Ok(()) => {
                        Self::add_repository_then(path, cx, Self::resume_open_in_desktop);
                        CloneOutcome::Added
                    }
                    // git removed what it created
                    Err(corvene_git::GitError::Cancelled(_)) => {
                        info!("clone cancelled");
                        Self::take_pending_alias(&path, cx);
                        Self::take_pending_account(&path, cx);
                        CloneOutcome::Cancelled
                    }
                    Err(err) => {
                        Self::take_pending_alias(&path, cx);
                        Self::take_pending_account(&path, cx);
                        CloneOutcome::Failed(err)
                    }
                };
                then(outcome, cx);
            });
        })
        .detach();
    }

    /// Stop the running clone (`234-clone-cancel`; GHD cannot: removing the
    /// cloning repository leaves `git clone` running). git removes the
    /// directory it created; the view says "Cancelling…" until it exits.
    pub fn cancel_clone(cx: &mut dyn Host) {
        if !Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::CLONE_CANCEL)
        {
            return;
        }
        Self::state(cx).update(cx, |s, cx| {
            // the clone the content area shows
            if let Some(id) = s.cloning.latest().map(|c| c.id)
                && s.cloning.cancel(id)
            {
                cx.notify();
            }
        });
    }

    /// Open a link. With a `511-browser` application set, web links open in
    /// it (`open -a <app> <url>`); other schemes keep the system handler.
    pub fn open_url(url: &str, cx: &mut dyn Host) {
        let browser = Self::state(cx)
            .read(cx)
            .flags
            .text(crate::flags::ids::BROWSER)
            .trim()
            .to_string();
        let web = url.starts_with("https://") || url.starts_with("http://");
        if browser.is_empty() || !web {
            cx.open_url(url);
            return;
        }
        if let Err(err) = corvene_platform::apps::open_with_app(Path::new(&browser), Path::new(url))
        {
            warn!(%browser, %err, "opening a link in the chosen browser failed");
            cx.open_url(url);
        }
    }

    /// Android: the system page where the user grants "All files access",
    /// so repositories on shared storage can be used in place. Nothing
    /// elsewhere.
    pub fn request_all_files_access(cx: &mut dyn Host) {
        let _ = cx;
        #[cfg(target_os = "android")]
        if let Some(bridge) = corvene_platform::android::bridge() {
            bridge.request_all_files_access();
        }
    }

    /// Native folder picker → `Some(path)` on the foreground.
    pub fn pick_directory(
        prompt: &str,
        cx: &mut dyn Host,
        on_pick: impl FnOnce(Option<PathBuf>, &mut dyn Host) + 'static,
    ) {
        let receiver = cx.prompt_for_paths(crate::host::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(prompt.to_string()),
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let picked = match receiver.await {
                Some(paths) => paths.into_iter().next(),
                _ => None,
            };
            cx.update(|cx| on_pick(picked, cx));
        })
        .detach();
    }

    // ---- commit / undo / discard (GHD `_commitIncludedChanges`, `_undoCommit`, `_discardChanges`) ----

    pub(crate) fn repo_context(
        id: u64,
        cx: &dyn Host,
    ) -> Option<(Arc<corvene_git::GitBinary>, std::path::PathBuf)> {
        let s = Self::state(cx).read(cx);
        let git = s.git.clone()?;
        let workdir = s.repo_states.get(&id)?.info.as_ref()?.workdir.clone();
        Some((git, workdir))
    }

    /// The commit form's commit (GHD `onCreateCommit`), after the checks of
    /// [`Self::commit_with`].
    pub fn commit(id: u64, summary: String, description: String, cx: &mut dyn Host) {
        Self::commit_with(
            id,
            summary,
            description,
            crate::commit_checks::CommitChecks::default(),
            cx,
        );
    }

    /// GHD `commitIncludedChanges`: commit the included changes, once
    /// [`Self::commit_with`]'s checks passed.
    pub(crate) fn create_commit(
        id: u64,
        summary: String,
        description: String,
        checks: crate::commit_checks::CommitChecks,
        cx: &mut dyn Host,
    ) {
        let embedded = checks.embedded.unwrap_or_default();
        // Corvene (`787-commit-to-new-branch`): publish and open a pull
        // request once the commit landed
        let publish =
            checks.after == crate::new_branch_flows::AfterCommit::PublishAndOpenPullRequest;
        // Corvene (`799-fixup-commits`): `--fixup` of this commit, and its
        // summary for the undo bar and the banner
        let fixup = checks.fixup.clone().and_then(|sha| {
            let s = Self::state(cx).read(cx);
            let summary = s
                .repo_states
                .get(&id)?
                .commits
                .iter()
                .find(|c| c.sha == sha)?
                .summary
                .clone();
            Some((sha, summary))
        });
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        // Corvene (`1313-changelists`): only the active list's ticked files
        let files: Vec<_> = Self::committed_files(Self::state(cx).read(cx), id);
        // the files that leave their changelists: a partly committed file
        // is still changed
        let committed_paths: Vec<String> = files
            .iter()
            .filter(|f| f.selection.kind() == DiffSelectionType::All)
            .map(|f| f.path.clone())
            .collect();
        // every changed file goes in whole: `reset -- .` would only unstage
        // what `update-index` stages again (one index rewrite fewer, ~120 ms
        // on a 50,000-file index). Not with an index entry the list leaves
        // out (added, then deleted from disk), which only the reset drops.
        let restages_everything = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|r| r.status.as_deref())
            .is_some_and(|st| {
                !st.has_conflicts()
                    && !st.hidden_index_entries
                    && files.len() == st.files.len()
                    && st
                        .files
                        .iter()
                        .all(|f| f.selection.kind() == DiffSelectionType::All)
            });
        let options = Self::state(cx)
            .read(cx)
            .repository(id)
            .map(|r| r.commit_options)
            .unwrap_or_default();
        let amend = fixup.is_none()
            && Self::state(cx)
                .read(cx)
                .repo_states
                .get(&id)
                .is_some_and(|rs| rs.commit_to_amend.is_some());
        // Corvene (`783-amend-author`): the author field of an amend
        let author = {
            let s = Self::state(cx).read(cx);
            s.repo_states
                .get(&id)
                .filter(|_| amend && s.flags.bool(crate::flags::ids::AMEND_AUTHOR))
                .and_then(|rs| rs.amend_author.clone())
        };
        if (summary.trim().is_empty() && fixup.is_none())
            || (files.is_empty() && !options.allow_empty_commit && !amend)
        {
            return;
        }
        // GHD `withIsCommitting`
        Self::state(cx).update(cx, |s, cx| {
            let rs = s.repo_state_mut(id);
            rs.committing = true;
            rs.hook_progress = None;
            rs.commit_output = None;
            rs.commit_progress = None;
            cx.notify();
        });
        // GHD `onHookProgress` / `onHookFailure` / `onTerminalOutputAvailable`
        // `1114-hook-results` names it "Commit to main" in the hook list
        let operation = if amend {
            "Amend last commit".to_string()
        } else {
            crate::hooks::current_branch_name(id, cx)
                .map_or_else(|| "Commit".to_string(), |b| format!("Commit to {b}"))
        };
        let hooks = crate::hooks::hook_ui(id, true, Some(operation), cx);
        // Corvene (`1307-commit-progress`): the staged files counted on the
        // commit button, the fully-included ones first
        let mut progress = Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::COMMIT_PROGRESS)
            .then(|| crate::commit_progress::commit_progress_ui(id, files.len(), cx));
        let whole_files = files
            .iter()
            .filter(|f| f.selection.kind() == DiffSelectionType::All)
            .count();
        let hook_callbacks = hooks.callbacks.clone();
        let output_sink = crate::hooks::CommitOutputSink::new();
        let output_tx = output_sink.sender();
        output_sink.listen(id, cx);
        // Corvene (`1302-wrap-commit-body`)
        let description = if Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::WRAP_COMMIT_BODY)
        {
            crate::commit_message::wrap_body(&description, crate::commit_message::BODY_WIDTH)
        } else {
            description
        };
        let message = corvene_git::format_message(&summary, &description);
        // GHD `getCoAuthorTrailers`: known co-authors become `Co-Authored-By` trailers
        let trailers: Vec<(String, String)> = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .filter(|rs| rs.show_co_authored_by && fixup.is_none())
            .map(|rs| {
                rs.co_authors
                    .iter()
                    .filter_map(|a| a.trailer_value())
                    .map(|v| ("Co-Authored-By".to_string(), v))
                    .collect()
            })
            .unwrap_or_default();
        // Corvene (`736-commit-and-push`): push once the commit succeeded;
        // not after an amend, whose rewritten tip may need a force push
        let push_after = options.push_after_commit
            && !amend
            && fixup.is_none()
            && Self::state(cx)
                .read(cx)
                .flags
                .bool(crate::flags::ids::COMMIT_AND_PUSH);
        // `259-amend-force-push-if-pushed`: the amended commit and the
        // upstream, to check that the rewritten commit had been pushed
        let pushed_check = {
            let s = Self::state(cx).read(cx);
            (amend && s.flags.bool(crate::flags::ids::AMEND_FORCE_PUSH_IF_PUSHED)).then(|| {
                let rs = s.repo_states.get(&id)?;
                let old = rs.commit_to_amend.as_ref()?.sha.clone();
                let upstream = rs.info.as_ref()?.current_branch()?.upstream.clone();
                Some((old, upstream))
            })
        };
        let summary_for_bar = match &fixup {
            Some((_, target)) => format!("{}{target}", corvene_git::rebase_ops::FIXUP_PREFIX),
            None => summary.trim().to_string(),
        };
        let fixup_sha = fixup.as_ref().map(|(sha, _)| sha.clone());
        // Corvene (`781-keep-staged-mode-changes`): executable bits staged
        // with `update-index --chmod` while `core.fileMode` is false
        let keep_modes = !restages_everything
            && Self::state(cx)
                .read(cx)
                .flags
                .bool(crate::flags::ids::KEEP_STAGED_MODE_CHANGES);
        let patch_options = Self::patch_options(cx);
        let task = cx.background_executor().spawn(async move {
            corvene_git::hook_env::reload_if_uncached();
            // GHD recommends a force push after every amend; the flag only
            // does when the amended commit is on the upstream
            let rewrites_pushed = match &pushed_check {
                None => true,
                Some(None) | Some(Some((_, None))) => false,
                Some(Some((old, Some(upstream)))) => {
                    corvene_git::merge_base(git.clone(), &workdir, old, upstream)
                        .ok()
                        .flatten()
                        .is_some_and(|base| base == *old)
                }
            };
            let message = corvene_git::merge_trailers(git.clone(), &workdir, &message, &trailers)?;
            let modes = if keep_modes {
                corvene_git::staged_mode_changes(git.clone(), &workdir).unwrap_or_else(|err| {
                    warn!(%err, "could not read the staged mode changes");
                    Vec::new()
                })
            } else {
                Vec::new()
            };
            if let Some(progress) = progress.as_mut() {
                progress.staged(0);
            }
            if !restages_everything {
                corvene_git::unstage_all(git.clone(), &workdir)?;
            }
            // Corvene (`785-embedded-repo-commit`): the nested repositories
            // as submodules or pointers (`update-index` skips their `Sub/`)
            corvene_git::add_embedded_repositories(git.clone(), &workdir, &embedded)?;
            match progress.as_mut() {
                Some(progress) => {
                    corvene_git::stage_files_with_progress(
                        git.clone(),
                        &workdir,
                        &files,
                        &mut |staged| progress.staged(staged),
                    )?;
                    corvene_git::stage_partial_files_with_progress(
                        git.clone(),
                        &workdir,
                        &files,
                        patch_options,
                        &mut |staged| progress.staged(whole_files + staged),
                    )?;
                }
                None => {
                    corvene_git::stage_files(git.clone(), &workdir, &files)?;
                    corvene_git::stage_partial_files_with(
                        git.clone(),
                        &workdir,
                        &files,
                        patch_options,
                    )?;
                }
            }
            if !modes.is_empty() {
                // the files going into the commit (a deleted one has no mode)
                let staged: std::collections::HashSet<&str> = files
                    .iter()
                    .filter(|f| f.status.kind != corvene_models::FileStatusKind::Deleted)
                    .map(|f| f.path.as_str())
                    .collect();
                let modes: Vec<(String, bool)> = modes
                    .into_iter()
                    .filter(|(path, _)| staged.contains(path.as_str()))
                    .collect();
                corvene_git::restore_mode_changes(git.clone(), &workdir, &modes)?;
            }
            if let Some(progress) = progress.as_mut() {
                progress.writing();
            }
            let on_output = crate::hooks::commit_output_callback(output_tx);
            corvene_git::hooks::with_hook_callbacks(&hook_callbacks, || {
                corvene_git::commit_with_terminal_output(
                    git.clone(),
                    &workdir,
                    &message,
                    &corvene_git::CommitOptions {
                        amend,
                        no_verify: options.skip_commit_hooks,
                        signoff: options.sign_off_commits,
                        allow_empty: options.allow_empty_commit,
                        author,
                        fixup: fixup_sha,
                    },
                    Some(&on_output),
                )
            })?;
            // `commit` returns git's abbreviated sha (GHD `parseCommitSHA`);
            // the undo bar and the force-push list keep the full one
            corvene_git::head_sha(git, &workdir).map(|sha| (sha, rewrites_pushed))
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let result = task.await;
            // GHD: `.catch(err => (aborted ? undefined : Promise.reject(err)))`
            let aborted = hooks.aborted();
            drop(hooks);
            cx.update(|cx| {
                Self::state(cx).update(cx, |s, cx| {
                    let rs = s.repo_state_mut(id);
                    rs.committing = false;
                    rs.hook_progress = None;
                    rs.commit_output = None;
                    rs.commit_progress = None;
                    if let Ok((sha, rewrites_pushed)) = &result {
                        // GHD `_addBranchToForcePushList`: an amended tip
                        // makes "Force push" the recommended action.
                        if amend && *rewrites_pushed {
                            let branch = rs
                                .info
                                .as_ref()
                                .and_then(|i| i.current_branch())
                                .map(|b| b.name_without_remote().to_string());
                            if let Some(branch) = branch {
                                rs.force_push_branches.insert(branch, sha.clone());
                            }
                        }
                        // GHD: no undo bar after an amend
                        rs.last_commit = (!amend).then(|| LastCommit {
                            sha: sha.clone(),
                            summary: summary_for_bar.clone(),
                            at: std::time::SystemTime::now(),
                        });
                        rs.commit_to_amend = None;
                        rs.amend_author = None;
                        // a fixup leaves the message typed so far alone
                        if fixup.is_none() {
                            rs.co_authors.clear();
                            rs.commit_nonce += 1;
                        }
                        // GHD `refreshChangesSection({ clearPartialState:
                        // true })`: what stays partially selected after a
                        // partial commit starts unselected
                        rs.clear_partial_state = true;
                    }
                    cx.notify();
                });
                let committed = result.is_ok();
                // `1313-changelists`: committed files leave their lists
                if committed {
                    Self::forget_changelist_paths(id, &committed_paths, cx);
                }
                if committed && let Some((_, target)) = &fixup {
                    Self::set_banner(
                        crate::mco::Banner::FixupCommitted {
                            repo: id,
                            target: target.clone(),
                        },
                        cx,
                    );
                }
                if let Err(err) = result
                    && !aborted
                {
                    // Corvene (`526-commit-signing`): a signing failure says
                    // so, git's words below
                    let signing = Self::state(cx)
                        .read(cx)
                        .flags
                        .bool(crate::flags::ids::COMMIT_SIGNING)
                        && err
                            .failure()
                            .is_some_and(|f| corvene_git::is_signing_failure(&f.output));
                    let message = if signing {
                        ErrorMessage::explained(
                            &err,
                            Some(
                                "Git could not sign the commit. Check your signing key or turn \
                                 off commit signing."
                                    .to_string(),
                            ),
                        )
                    } else {
                        ErrorMessage::from(&err)
                    };
                    Self::show_error("Could not commit", message, cx);
                }
                Self::refresh_repository(id, cx);
                let branch = Self::state(cx)
                    .read(cx)
                    .repo_states
                    .get(&id)
                    .and_then(|rs| rs.info.as_ref())
                    .and_then(|i| i.current_branch())
                    .map(|b| b.name.clone());
                if committed
                    && publish
                    && let Some(branch) = branch
                {
                    Self::publish_and_open_pull_request(id, branch, cx);
                } else if committed && push_after {
                    Self::push(id, false, None, cx);
                }
            });
        })
        .detach();
    }

    /// Commit form gear menu (`onUpdateCommitOptions`), persisted with the repository.
    pub fn update_commit_options(
        id: u64,
        edit: impl FnOnce(&mut corvene_models::RepoCommitOptions),
        cx: &mut dyn Host,
    ) {
        Self::state(cx).update(cx, |s, cx| {
            if let Some(repo) = s.repositories.iter_mut().find(|r| r.id == id) {
                edit(&mut repo.commit_options);
                persist_repositories(s);
                cx.notify();
            }
        });
    }

    /// GHD `undoCommit` (`GitStore.undoCommit`) for the `HEAD` commit: the
    /// commit form gets its message back.
    pub fn undo_commit(id: u64, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        // Corvene (`1208-undo-restores-line-selection`): the diffs are read
        // the way the Changes tab reads them, so the line numbers match;
        // not while whitespace is hidden, which turns line selection off
        let restore = {
            let s = Self::state(cx).read(cx);
            (s.flags
                .bool(crate::flags::ids::UNDO_RESTORES_LINE_SELECTION)
                && !s.settings.hide_whitespace_in_changes_diff)
                .then(|| s.flags.bool(crate::flags::ids::RENAMED_DIFF_AGAINST_HEAD))
        };
        let task = cx.background_executor().spawn(async move {
            let head = corvene_git::get_commits(&workdir, "HEAD", 0, 1)?
                .into_iter()
                .next();
            let message = match &head {
                Some(commit) => {
                    crate::git_store::undo_commit(git.clone(), &workdir, commit).map(Some)
                }
                None => corvene_git::undo_last_commit(git.clone(), &workdir).map(|()| None),
            }?;
            let restored = match (restore, &head) {
                (Some(renamed_against_head), Some(commit)) => {
                    restored_selections(git, &workdir, commit, renamed_against_head).unwrap_or_else(
                        |err| {
                            warn!(%err, "could not read the undone commit's lines");
                            Vec::new()
                        },
                    )
                }
                _ => Vec::new(),
            };
            Ok::<_, corvene_git::GitError>((message, restored))
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let result = task.await;
            let (result, restored) = match result {
                Ok((message, restored)) => (Ok(message), restored),
                Err(err) => (Err(err), Vec::new()),
            };
            cx.update(|cx| {
                Self::state(cx).update(cx, |s, cx| {
                    let follow_lines = s.flags.bool(crate::flags::ids::SELECTION_FOLLOWS_LINES);
                    let rs = s.repo_state_mut(id);
                    rs.last_commit = None;
                    if let Ok(Some(message)) = &result {
                        rs.commit_message = message.clone();
                        rs.commit_message_nonce += 1;
                    }
                    rs.restored_selections.clear();
                    for (path, selection, diff) in restored {
                        // `790`: the diff those lines are positions in
                        if follow_lines && selection.kind() == DiffSelectionType::Partial {
                            rs.selection_bases.insert(path.clone(), diff);
                        }
                        rs.restored_selections.insert(path, selection);
                    }
                    cx.notify();
                });
                if let Err(err) = result {
                    Self::show_error("Could not undo commit", &err, cx);
                }
                Self::refresh_repository(id, cx);
            });
        })
        .detach();
    }

    /// Discard the given paths' changes (after the confirmation prompt).
    pub fn discard_changes(id: u64, paths: Vec<String>, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let files: Vec<_> = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|r| r.status.as_deref())
            .map(|st| {
                // Discard All passes every path: a set, not a search per file
                let paths: std::collections::HashSet<&str> =
                    paths.iter().map(String::as_str).collect();
                st.files
                    .iter()
                    .filter(|f| paths.contains(f.path.as_str()))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if files.is_empty() {
            return;
        }
        let flags = &Self::state(cx).read(cx).flags;
        let clean_submodules = flags.bool(crate::flags::ids::DISCARD_SUBMODULE_CHANGES);
        let move_to_trash = !flags.bool(crate::flags::ids::DISCARD_SKIPS_TRASH);
        // GHD `askForConfirmationOnDiscardChangesPermanently`
        let keep_untrashable = Self::state(cx)
            .read(cx)
            .settings
            .confirm_discard_changes_permanently;
        let discarded: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
        Self::state(cx).update(cx, |s, cx| {
            s.repo_state_mut(id).discarding = true;
            cx.notify();
        });
        let task = cx.background_executor().spawn(async move {
            corvene_git::discard_changes(
                git,
                &workdir,
                &files,
                move_to_trash,
                clean_submodules,
                keep_untrashable,
            )
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let result = task.await;
            cx.update(|cx| {
                Self::state(cx).update(cx, |s, cx| {
                    s.repo_state_mut(id).discarding = false;
                    cx.notify();
                });
                match result {
                    Err(err) => Self::show_error("Could not discard changes", err.to_string(), cx),
                    Ok(untrashable) => {
                        // `1313-changelists`: discarded files leave their
                        // lists (not the ones the Trash refused, still changed)
                        let gone: Vec<String> = discarded
                            .iter()
                            .filter(|p| !untrashable.contains(p))
                            .cloned()
                            .collect();
                        Self::forget_changelist_paths(id, &gone, cx);
                        Self::confirm_delete_untrashable(id, untrashable, cx)
                    }
                }
                Self::refresh_repository(id, cx);
            });
        })
        .detach();
    }

    /// GHD `DiscardChangesError` → `DiscardChangesRetry`: files the Trash
    /// refused keep their changes until the user agrees to lose them.
    fn confirm_delete_untrashable(id: u64, paths: Vec<String>, cx: &mut dyn Host) {
        if !paths.is_empty() {
            Self::show_popup(Popup::ConfirmDeleteUntrashable { repo: id, paths }, cx);
        }
    }

    /// `DiscardChangesRetry` › Permanently Discard Changes: discard `paths`
    /// again without the Trash (GHD `discardChanges(files, false, true)`).
    pub fn delete_untrashable(id: u64, paths: Vec<String>, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let (files, clean_submodules) = {
            let s = Self::state(cx).read(cx);
            let files: Vec<_> = s
                .repo_states
                .get(&id)
                .and_then(|r| r.status.as_deref())
                .map(|st| {
                    st.files
                        .iter()
                        .filter(|f| paths.contains(&f.path))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            (
                files,
                s.flags.bool(crate::flags::ids::DISCARD_SUBMODULE_CHANGES),
            )
        };
        let task = cx.background_executor().spawn(async move {
            if files.is_empty() {
                // gone from the status meanwhile: delete what is left
                corvene_git::delete_worktree_paths(&workdir, &paths)
            } else {
                corvene_git::discard_changes(git, &workdir, &files, false, clean_submodules, false)
                    .map(|_| ())
            }
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let result = task.await;
            cx.update(|cx| {
                if let Err(err) = result {
                    Self::show_error("Could not discard changes", &err, cx);
                }
                Self::refresh_repository(id, cx);
            });
        })
        .detach();
    }

    /// Switch Branch › "Discard my changes" (`865-switch-branch-discard`):
    /// discard every change (new files to the Trash), then check `branch`
    /// out. A failed discard stops before the checkout.
    pub fn discard_all_and_checkout(id: u64, branch: String, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let files: Vec<_> = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|r| r.status.as_deref())
            .map(|st| st.files.clone())
            .unwrap_or_default();
        let flags = &Self::state(cx).read(cx).flags;
        let clean_submodules = flags.bool(crate::flags::ids::DISCARD_SUBMODULE_CHANGES);
        let move_to_trash = !flags.bool(crate::flags::ids::DISCARD_SKIPS_TRASH);
        // GHD `askForConfirmationOnDiscardChangesPermanently`
        let keep_untrashable = Self::state(cx)
            .read(cx)
            .settings
            .confirm_discard_changes_permanently;
        let task = cx.background_executor().spawn(async move {
            if files.is_empty() {
                Ok(Vec::new())
            } else {
                corvene_git::discard_changes(
                    git,
                    &workdir,
                    &files,
                    move_to_trash,
                    clean_submodules,
                    keep_untrashable,
                )
            }
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let result = task.await;
            cx.update(|cx| match result {
                Err(err) => {
                    Self::show_error("Could not discard changes", &err, cx);
                    Self::refresh_repository(id, cx);
                }
                // `DiscardChangesRetry`: files are left, so no checkout yet
                Ok(untrashable) if !untrashable.is_empty() => {
                    Self::confirm_delete_untrashable(id, untrashable, cx);
                    Self::refresh_repository(id, cx);
                }
                // nothing is left to stash, and `MoveToNewBranch` never
                // touches the existing stash
                Ok(_) => Self::checkout_branch(
                    id,
                    branch,
                    Some(UncommittedChangesStrategy::MoveToNewBranch),
                    cx,
                ),
            });
        })
        .detach();
    }

    /// GHD `onDiscardChangesFromFiles`: confirm first unless the user opted out.
    pub fn request_discard_changes(id: u64, paths: Vec<String>, cx: &mut dyn Host) {
        if paths.is_empty() {
            return;
        }
        let (confirm, total) = {
            let s = Self::state(cx).read(cx);
            let total = s
                .repo_states
                .get(&id)
                .and_then(|r| r.status.as_deref())
                .map(|st| st.files.len())
                .unwrap_or(0);
            (s.settings.confirm_discard_changes, total)
        };
        let all = paths.len() == total;
        // `723-discard-confirm-snooze`: never for Discard All
        if confirm && (all || !Self::discard_confirm_snoozed(id, cx)) {
            Self::show_popup(
                Popup::DiscardChanges {
                    repo: id,
                    paths,
                    all,
                },
                cx,
            );
        } else {
            Self::discard_changes(id, paths, cx);
        }
    }

    /// Corvene `723-discard-confirm-snooze`: the confirmation is snoozed for
    /// this repository.
    fn discard_confirm_snoozed(id: u64, cx: &dyn Host) -> bool {
        let s = Self::state(cx).read(cx);
        s.flags.number(crate::flags::ids::DISCARD_CONFIRM_SNOOZE) > 0
            && s.repo_states
                .get(&id)
                .and_then(|r| r.discard_confirm_snoozed_until)
                .is_some_and(|until| Instant::now() < until)
    }

    /// Corvene `723-discard-confirm-snooze`: skip the confirmation for the
    /// flag's number of minutes.
    pub fn snooze_discard_confirm(id: u64, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, _| {
            let minutes = s.flags.number(crate::flags::ids::DISCARD_CONFIRM_SNOOZE);
            let Ok(minutes) = u64::try_from(minutes) else {
                return;
            };
            s.repo_state_mut(id).discard_confirm_snoozed_until =
                Some(Instant::now() + std::time::Duration::from_secs(minutes * 60));
        });
    }

    /// The flags that change the partial patches git applies.
    fn patch_options(cx: &mut dyn Host) -> corvene_git::PatchOptions {
        Self::patch_options_of(&Self::state(cx).read(cx).flags)
    }

    pub(crate) fn patch_options_of(flags: &crate::flags::Flags) -> corvene_git::PatchOptions {
        corvene_git::PatchOptions {
            exact_hunk_starts: flags.bool(crate::flags::ids::PARTIAL_COMMIT_HUNK_POSITIONS),
            raw_lines: flags.bool(crate::flags::ids::NON_UTF8_DIFFS),
        }
    }

    /// GHD `onDiscardChangesFromSelection` (diff gutter menu): confirm first
    /// unless the user opted out.
    pub fn request_discard_selection(
        id: u64,
        path: String,
        selection: corvene_models::DiffSelection,
        cx: &mut dyn Host,
    ) {
        if Self::state(cx).read(cx).settings.confirm_discard_changes
            && !Self::discard_confirm_snoozed(id, cx)
        {
            Self::show_popup(
                Popup::ConfirmDiscardSelection {
                    repo: id,
                    path,
                    selection,
                },
                cx,
            );
        } else {
            Self::discard_selection(id, path, selection, cx);
        }
    }

    /// `discardChangesFromSelection`: reverse-apply the selected lines of the
    /// current (unexpanded) diff to the working copy. A UTF-16 file's
    /// (`1306-utf16-diffs`) is rewritten without them instead.
    pub fn discard_selection(
        id: u64,
        path: String,
        selection: corvene_models::DiffSelection,
        cx: &mut dyn Host,
    ) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        enum Discard {
            Patch(Vec<u8>),
            Utf16(Arc<corvene_models::Diff>),
        }
        let discard = {
            let s = Self::state(cx).read(cx);
            let Some(rs) = s.repo_states.get(&id) else {
                return;
            };
            if rs.selected_file.as_deref() != Some(path.as_str()) {
                return;
            }
            let Some(diff) = rs.diff.as_ref() else {
                return;
            };
            let Some(hunks) = diff.hunks() else {
                return;
            };
            if diff.warnings().is_some_and(|w| w.utf16.is_some()) {
                Some(Discard::Utf16(diff.clone()))
            } else {
                corvene_git::format_patch_to_discard_changes_with(
                    &path,
                    hunks,
                    &selection,
                    Self::patch_options_of(&s.flags),
                )
                .map(Discard::Patch)
            }
        };
        let Some(discard) = discard else { return };
        crate::remote::spawn_bg(
            cx,
            move || {
                match discard {
                    Discard::Patch(patch) => {
                        corvene_git::discard_changes_from_selection(git, &workdir, &patch)
                    }
                    Discard::Utf16(diff) => corvene_git::utf16::discard_selection(
                        git, &workdir, &path, &diff, &selection,
                    ),
                }
                .map_err(|e| e.to_string())
            },
            move |result, cx| {
                if let Err(err) = result {
                    Self::show_error("Could not discard changes", err, cx);
                }
                Self::refresh_repository(id, cx);
            },
        );
    }

    /// Corvene `793-hide-whitespace-only-files`: while Changes hides
    /// whitespace, which changed files have whitespace changes only (the
    /// list leaves them out; they are still committed).
    fn load_whitespace_only_files(id: u64, cx: &mut dyn Host) {
        let wanted = {
            let s = Self::state(cx).read(cx);
            s.flags.bool(crate::flags::ids::HIDE_WHITESPACE_ONLY_FILES)
                && s.settings.hide_whitespace_in_changes_diff
                && s.repo_states
                    .get(&id)
                    .and_then(|rs| rs.status.as_deref())
                    .is_some_and(|st| !st.files.is_empty())
        };
        if !wanted {
            Self::state(cx).update(cx, |s, cx| {
                if s.repo_state_mut(id).whitespace_only_files.take().is_some() {
                    cx.notify();
                }
            });
            return;
        }
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        crate::remote::spawn_bg(
            cx,
            move || {
                corvene_git::whitespace_only_paths(git, &workdir, &["HEAD"]).unwrap_or_default()
            },
            move |paths, cx| {
                Self::state(cx).update(cx, |s, cx| {
                    let rs = s.repo_state_mut(id);
                    let next = (!paths.is_empty()).then(|| Arc::new(paths));
                    if rs.whitespace_only_files != next {
                        rs.whitespace_only_files = next;
                        cx.notify();
                    }
                });
            },
        );
    }

    /// Diff Settings › Hide Whitespace Changes, per tab
    /// (`_setHideWhitespaceInChangesDiff` / `…HistoryDiff`); reloads the diff.
    /// In Changes it refreshes the status with partial selections cleared
    /// (GHD `refreshChangesSection({ clearPartialState: true })`): the line
    /// numbers they name belong to the other diff.
    pub fn set_hide_whitespace_in_diff(history: bool, hide: bool, cx: &mut dyn Host) {
        Self::update_settings(cx, |s| {
            if history {
                s.hide_whitespace_in_history_diff = hide;
            } else {
                s.hide_whitespace_in_changes_diff = hide;
            }
        });
        for id in Self::state(cx).read(cx).visible_repositories() {
            if history {
                // `793-hide-whitespace-only-files`: the list depends on it
                if Self::state(cx)
                    .read(cx)
                    .flags
                    .bool(crate::flags::ids::HIDE_WHITESPACE_ONLY_FILES)
                {
                    Self::load_changeset(id, cx);
                }
                Self::load_commit_diff(id, cx);
                Self::load_stash_diff(id, cx);
                // `1311-stacked-diff`: the stacked diffs read the setting too
                Self::clear_stacked_diffs(id, cx);
            } else {
                Self::state(cx).update(cx, |s, _| {
                    s.repo_state_mut(id).clear_partial_state = true;
                });
                Self::load_diff(id, cx);
                Self::clear_stacked_diffs(id, cx);
                Self::refresh_repository(id, cx);
            }
        }
    }

    /// Diff Settings › Diff display (`_setShowSideBySideDiff`).
    pub fn set_show_side_by_side_diff(show: bool, cx: &mut dyn Host) {
        Self::update_settings(cx, |s| s.show_side_by_side_diff = show);
    }

    /// Corvene `1304-diff-no-wrap`: View › Wrap Diff Lines and Diff
    /// Settings › Wrap Long Lines.
    pub fn set_diff_wrap_lines(wrap: bool, cx: &mut dyn Host) {
        Self::update_settings(cx, |s| s.diff_wrap_lines = wrap);
    }

    /// Corvene `1316-diff-moved-lines`: Diff Settings › Mark Moved Lines.
    pub fn set_diff_mark_moved_lines(on: bool, cx: &mut dyn Host) {
        Self::update_settings(cx, |s| s.diff_mark_moved_lines = on);
    }

    /// Corvene `1317-diff-stylistic-changes`: Diff Settings › Mark
    /// Stylistic Changes.
    pub fn set_diff_mark_stylistic_changes(on: bool, cx: &mut dyn Host) {
        Self::update_settings(cx, |s| s.diff_mark_stylistic_changes = on);
    }

    /// Corvene `1318-csv-table-diff`: the diff header's Table/Text switch.
    pub fn set_csv_table_diff(on: bool, cx: &mut dyn Host) {
        Self::update_settings(cx, |s| s.csv_table_diff = on);
    }

    /// Modified-image diff tab (`_changeImageDiffType`).
    pub fn set_image_diff_type(kind: corvene_models::ImageDiffType, cx: &mut dyn Host) {
        Self::update_settings(cx, |s| s.image_diff_type = kind);
    }

    /// `onOpenSubmodule`: add the submodule as a repository of its own.
    pub fn open_submodule(path: PathBuf, cx: &mut dyn Host) {
        Self::add_repository(path, cx);
    }

    /// "Ignore File / Folder" menu items: paths are escaped before writing.
    pub fn ignore_files(id: u64, paths: Vec<String>, cx: &mut dyn Host) {
        let patterns = paths
            .iter()
            .map(|p| corvene_git::escape_gitignore_pattern(p))
            .collect();
        Self::ignore_patterns(id, patterns, cx);
    }

    /// Corvene `714-copy-diff`: the changes of `paths` as a patch on the
    /// clipboard (`corvene_git::working_directory_patch`).
    pub fn copy_diff(id: u64, paths: Vec<String>, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let (files, base) = {
            let s = Self::state(cx).read(cx);
            let Some(rs) = s.repo_states.get(&id) else {
                return;
            };
            let files: Vec<_> = rs
                .status
                .as_deref()
                .map(|st| {
                    let paths: std::collections::HashSet<&str> =
                        paths.iter().map(String::as_str).collect();
                    st.files
                        .iter()
                        .filter(|f| paths.contains(f.path.as_str()))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            let unborn = rs
                .info
                .as_ref()
                .is_some_and(|i| matches!(i.tip, corvene_models::Tip::Unborn { .. }));
            let base = if unborn {
                corvene_git::NULL_TREE_SHA
            } else {
                "HEAD"
            };
            (files, base)
        };
        if files.is_empty() {
            return;
        }
        let task = cx.background_executor().spawn(async move {
            corvene_git::working_directory_patch(git, &workdir, &files, base)
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let result = task.await;
            cx.update(|cx| match result {
                Ok(patch) => cx.write_to_clipboard(patch),
                Err(err) => Self::show_error("Could not copy the diff", &err, cx),
            });
        })
        .detach();
    }

    /// Corvene `715-assume-unchanged`: mark `paths` assume-unchanged, or with
    /// `paths: None` clear the mark from every file that has it; then refresh.
    pub fn set_assume_unchanged(
        id: u64,
        paths: Option<Vec<String>>,
        assume: bool,
        cx: &mut dyn Host,
    ) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let task = cx.background_executor().spawn(async move {
            let paths = match paths {
                Some(paths) => paths,
                None => corvene_git::assume_unchanged_paths(git.clone(), &workdir)?,
            };
            corvene_git::set_assume_unchanged(git, &workdir, &paths, assume)
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let result = task.await;
            cx.update(|cx| {
                if let Err(err) = result {
                    Self::show_error("Could not update the index", &err, cx);
                }
                Self::refresh_repository(id, cx);
            });
        })
        .detach();
    }

    /// Append raw patterns (e.g. `*.log`) to the root `.gitignore`, then refresh.
    pub fn ignore_patterns(id: u64, patterns: Vec<String>, cx: &mut dyn Host) {
        let Some((_git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let skip_existing = Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::IGNORE_SKIPS_EXISTING_RULES);
        let task = cx.background_executor().spawn(async move {
            corvene_git::append_ignore_rules(&workdir, &patterns, skip_existing)
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let result = task.await;
            cx.update(|cx| {
                if let Err(err) = result {
                    Self::show_error("Could not update .gitignore", &err, cx);
                }
                Self::refresh_repository(id, cx);
            });
        })
        .detach();
    }

    /// "Ignore File In" (flag `ignore-file-targets`): ignore one path from
    /// another ignore file than the root `.gitignore`, then refresh.
    pub fn ignore_file_in(
        id: u64,
        path: String,
        target: corvene_git::IgnoreTarget,
        cx: &mut dyn Host,
    ) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let skip_existing = Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::IGNORE_SKIPS_EXISTING_RULES);
        let patterns = vec![target.pattern_for(&path)];
        let task = cx.background_executor().spawn(async move {
            corvene_git::append_ignore_rules_to(git, &workdir, &target, &patterns, skip_existing)
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let result = task.await;
            cx.update(|cx| {
                if let Err(err) = result {
                    Self::show_error("Could not update the ignore file", &err, cx);
                }
                Self::refresh_repository(id, cx);
            });
        })
        .detach();
    }

    /// Repository Settings › Ignored Files › "Edit global ignore file" (flag
    /// `edit-global-ignore-file`): create the excludes file if needed and
    /// open it in the external editor.
    pub fn edit_global_ignore_file(id: u64, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let task = cx.background_executor().spawn(async move {
            let path = corvene_git::excludes_file(git, &workdir)
                .ok_or_else(|| "core.excludesFile is unset and HOME is unknown".to_string())?;
            if !path.exists() {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                std::fs::write(&path, "").map_err(|e| e.to_string())?;
            }
            Ok::<_, String>(path)
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let result = task.await;
            cx.update(|cx| match result {
                Ok(path) => Self::open_in_editor(path, cx),
                Err(err) => Self::show_error("Could not open the global ignore file", err, cx),
            });
        })
        .detach();
    }

    // ---- sign-in (GHD `SignInStore`) ----

    pub(crate) fn set_sign_in_step(step: AuthenticationStep, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            if let AuthenticationStep::Error(message) = &step {
                // GHD's `onAuthError`: the Authentication step shows it
                sign_in_store(s).authentication_failed(message.clone());
            }
            if let Some(si) = s.authentication.as_mut() {
                si.step = step;
                cx.notify();
            }
        });
    }

    /// GHD `authenticateWithBrowser`, before the browser opens: the sign-in
    /// store moves to Authentication (loading), and the account an
    /// ExistingAccountWarning named is signed out first. Every
    /// authentication flow starts here.
    pub(crate) fn start_authentication(cx: &mut dyn Host) {
        let failed = Self::state(cx).update(cx, |s, cx| {
            let existing = match sign_in_store(s).authenticate_with_browser() {
                Ok(existing) => existing,
                Err(err) => {
                    // a flow started without the sign-in dialog
                    info!(%err, "authentication outside the sign-in store");
                    return None;
                }
            };
            cx.notify();
            crate::accounts::remove_account(
                &s.store,
                &mut s.accounts,
                &existing?,
                &crate::accounts::Keychain,
            )
            .err()
        });
        if let Some(err) = failed {
            Self::show_error("Could not sign out", err.to_string(), cx);
        }
    }

    /// OAuth device flow against GitHub.com, or a GitHub Enterprise host
    /// with an OAuth app (`oauth_client_id`). Runs on its own thread;
    /// progress is pumped to the foreground.
    pub fn sign_in_device_flow(endpoint: corvene_github::Endpoint, cx: &mut dyn Host) {
        Self::start_authentication(cx);
        let client_id = Self::oauth_client_id(&endpoint, cx);
        // `350-ssh-key-helper` may ask for more
        let scopes = corvene_github::scopes_with(&Self::state(cx).read(cx).extra_oauth_scopes);
        let cancel = Arc::new(AtomicBool::new(false));
        Self::state(cx).update(cx, |s, cx| {
            if let Some(existing) = s.authentication.as_ref() {
                existing.cancel.store(true, Ordering::SeqCst);
            }
            s.authentication = Some(AuthenticationFlow {
                endpoint: endpoint.api_base.clone(),
                step: AuthenticationStep::Requesting,
                web_flow: None,
                cancel: cancel.clone(),
            });
            cx.notify();
        });

        enum Msg {
            Code(corvene_github::auth::DeviceCode),
            Token { token: String, scopes: Vec<String> },
            Failed(String),
        }
        let (tx, rx) = std::sync::mpsc::channel::<Msg>();
        let worker_endpoint = endpoint.clone();
        let worker_cancel = cancel.clone();
        std::thread::Builder::new()
            .name("device-flow".into())
            .spawn(move || {
                let Some(client_id) = client_id else {
                    let _ = tx.send(Msg::Failed(crate::web_flow::no_oauth_app_message(
                        &worker_endpoint,
                    )));
                    return;
                };
                let code = match corvene_github::auth::request_device_code(
                    &worker_endpoint,
                    &client_id,
                    &scopes,
                ) {
                    Ok(code) => code,
                    Err(err) => {
                        let _ = tx.send(Msg::Failed(err.to_string()));
                        return;
                    }
                };
                let mut interval = code.poll_interval();
                let deadline = std::time::Instant::now()
                    + std::time::Duration::from_secs(code.expires_in.max(60));
                let device_code = code.device_code.clone();
                let _ = tx.send(Msg::Code(code));
                loop {
                    if worker_cancel.load(Ordering::SeqCst) {
                        return;
                    }
                    if std::time::Instant::now() > deadline {
                        let _ =
                            tx.send(Msg::Failed("the sign-in request expired; try again".into()));
                        return;
                    }
                    std::thread::sleep(interval);
                    if worker_cancel.load(Ordering::SeqCst) {
                        return;
                    }
                    match corvene_github::auth::poll_token(
                        &worker_endpoint,
                        &client_id,
                        &device_code,
                    ) {
                        Ok(corvene_github::auth::PollOutcome::Pending) => {}
                        Ok(corvene_github::auth::PollOutcome::SlowDown) => {
                            interval += std::time::Duration::from_secs(5);
                        }
                        Ok(corvene_github::auth::PollOutcome::Token { token, scopes }) => {
                            let _ = tx.send(Msg::Token { token, scopes });
                            return;
                        }
                        // Android: the browser is in front while the code is
                        // typed, and requests of an app in the background
                        // can fail (no network for it on some devices);
                        // keep asking until the code expires
                        #[cfg(target_os = "android")]
                        Err(corvene_github::GitHubError::Http(err)) => {
                            warn!(%err, "sign-in poll failed; retrying");
                        }
                        Err(err) => {
                            let _ = tx.send(Msg::Failed(err.to_string()));
                            return;
                        }
                    }
                }
            })
            .ok();

        cx.spawn(async move |cx: &mut AsyncCtx| {
            loop {
                if cancel.load(Ordering::SeqCst) {
                    break;
                }
                let mut finished = false;
                while let Ok(msg) = rx.try_recv() {
                    match msg {
                        Msg::Code(code) => {
                            let uri = code.verification_uri.clone();
                            cx.update(|cx| {
                                Self::set_sign_in_step(
                                    AuthenticationStep::DeviceCode {
                                        user_code: code.user_code.clone(),
                                        verification_uri: uri.clone(),
                                    },
                                    cx,
                                );
                                // Android: the browser covers the dialog,
                                // so the code goes along on the clipboard
                                #[cfg(target_os = "android")]
                                {
                                    cx.write_to_clipboard(code.user_code.clone());
                                    corvene_platform::android::toast(&format!(
                                        "Code {} copied. Paste it on the page.",
                                        code.user_code
                                    ));
                                }
                                Self::open_url(&uri, cx);
                            });
                        }
                        Msg::Token { token, scopes } => {
                            let endpoint = endpoint.clone();
                            cx.update(|cx| Self::finish_sign_in(endpoint, token, scopes, cx));
                            finished = true;
                        }
                        Msg::Failed(err) => {
                            cx.update(|cx| {
                                Self::set_sign_in_step(AuthenticationStep::Error(err), cx)
                            });
                            finished = true;
                        }
                    }
                }
                if finished {
                    break;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(250))
                    .await;
            }
        })
        .detach();
    }

    /// The OAuth client ID a sign-in on `endpoint` uses: the one entered for
    /// that GitHub Enterprise host, else the build's (`OAuthApp::built_in`).
    /// `None`: a GHES host nobody registered an app for (PAT only).
    pub fn oauth_client_id(endpoint: &corvene_github::Endpoint, cx: &dyn Host) -> Option<String> {
        if !endpoint.is_dotcom()
            && let Some(id) = Self::state(cx)
                .read(cx)
                .enterprise_oauth_apps
                .get(&endpoint.host().to_ascii_lowercase())
        {
            return Some(id.clone());
        }
        corvene_github::OAuthApp::built_in(endpoint).map(|app| app.client_id)
    }

    /// Remember the OAuth app a GitHub Enterprise host signs in with. An
    /// empty `client_id` forgets the host's entry (and its secret); a
    /// non-empty `client_secret` replaces the keychain's, an empty one keeps
    /// what is there.
    pub fn set_enterprise_oauth_app(
        endpoint: &corvene_github::Endpoint,
        client_id: String,
        client_secret: String,
        cx: &mut dyn Host,
    ) {
        if endpoint.is_dotcom() {
            return;
        }
        let host = endpoint.host().to_ascii_lowercase();
        let client_id = client_id.trim().to_string();
        let client_secret = client_secret.trim().to_string();
        let previous = Self::state(cx).update(cx, |s, cx| {
            let previous = if client_id.is_empty() {
                s.enterprise_oauth_apps.remove(&host)
            } else {
                s.enterprise_oauth_apps
                    .insert(host.clone(), client_id.clone())
            };
            if let Err(err) = s.store.save_enterprise_oauth_apps(&s.enterprise_oauth_apps) {
                error!(?err, "could not save the Enterprise OAuth apps");
            }
            cx.notify();
            previous
        });
        cx.background_executor()
            .spawn(async move {
                use corvene_platform::keychain;
                if let Some(old) = previous.filter(|old| *old != client_id) {
                    let _ = keychain::delete_oauth_client_secret(&host, &old);
                }
                if !client_id.is_empty()
                    && !client_secret.is_empty()
                    && let Err(err) =
                        keychain::store_oauth_client_secret(&host, &client_id, &client_secret)
                {
                    error!(?err, "could not store the OAuth client secret");
                }
            })
            .detach();
    }

    /// Personal access token (GHES, or the fallback link on GitHub.com).
    pub fn sign_in_with_token(
        endpoint: corvene_github::Endpoint,
        token: String,
        cx: &mut dyn Host,
    ) {
        Self::start_authentication(cx);
        Self::state(cx).update(cx, |s, cx| {
            s.authentication = Some(AuthenticationFlow {
                endpoint: endpoint.api_base.clone(),
                step: AuthenticationStep::Verifying,
                web_flow: None,
                cancel: Arc::new(AtomicBool::new(false)),
            });
            cx.notify();
        });
        Self::finish_sign_in(endpoint, token, Vec::new(), cx);
    }

    /// `finish_sign_in` for the browser flow (`web_flow.rs`).
    pub(crate) fn finish_sign_in_public(
        endpoint: corvene_github::Endpoint,
        token: String,
        scopes: Vec<String>,
        cx: &mut dyn Host,
    ) {
        Self::finish_sign_in(endpoint, token, scopes, cx);
    }

    /// Token → account (background), then GHD's `_addAccount`
    /// (`AccountsStore.addAccount`: keychain + store) and the sign-in
    /// store's Success, which closes the dialog (GHD's `SignIn` dismisses
    /// itself at Success: `resetSignInState` + `closePopup`).
    fn finish_sign_in(
        endpoint: corvene_github::Endpoint,
        token: String,
        scopes: Vec<String>,
        cx: &mut dyn Host,
    ) {
        Self::set_sign_in_step(AuthenticationStep::Verifying, cx);
        let task = cx.background_executor().spawn({
            let endpoint = endpoint.clone();
            let token = token.clone();
            async move { corvene_github::Client::new(endpoint, token).current_user(scopes) }
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let result = task.await;
            cx.update(|cx| {
                let account = match result {
                    Ok(account) => account,
                    Err(err) => {
                        Self::set_sign_in_step(AuthenticationStep::Error(err.to_string()), cx);
                        return;
                    }
                };
                // `527-multiple-accounts`: Add account came back with an
                // account that is signed in already (the browser was
                // signed in to it)
                let again = {
                    let s = Self::state(cx).read(cx);
                    (s.multiple_accounts()
                        && s.adding_account
                        && s.account_with_login(&account.endpoint, &account.login)
                            .is_some())
                    .then(|| already_signed_in_hint(&account))
                };
                if let Some(hint) = again {
                    Self::set_sign_in_step(AuthenticationStep::Error(hint), cx);
                    return;
                }
                let added = Self::state(cx).update(cx, |s, cx| {
                    let multiple = s.multiple_accounts();
                    let added = crate::accounts::add_account(
                        &s.store,
                        &mut s.accounts,
                        account,
                        &token,
                        &crate::accounts::Keychain,
                        multiple,
                    );
                    if let Ok(account) = &added {
                        info!(login = %account.login, endpoint = %account.endpoint, "signed in");
                        s.authentication = None;
                        sign_in_store(s).authentication_succeeded(account.clone());
                        close_sign_in_popups(s);
                        cx.notify();
                    }
                    added
                });
                match added {
                    Ok(account) => Self::check_git_email_after_sign_in(account, cx),
                    Err(err) => {
                        Self::set_sign_in_step(AuthenticationStep::Error(err.to_string()), cx);
                        return;
                    }
                }
                // `refreshSelectedRepositoryAfterAccountChange`
                for id in Self::state(cx).read(cx).visible_repositories() {
                    Self::refresh_github_repository(id, cx);
                }
            });
        })
        .detach();
    }

    /// `322-git-email-mismatch-banner`: after signing in outside the Welcome
    /// flow (which asks for the identity itself), a banner when the global
    /// `user.email` is unset or would not link commits to `account`. Reads
    /// only; Settings › Git changes it.
    fn check_git_email_after_sign_in(account: corvene_models::Account, cx: &mut dyn Host) {
        let (enabled, git) = {
            let s = Self::state(cx).read(cx);
            (
                s.flags.bool(crate::flags::ids::GIT_EMAIL_MISMATCH_BANNER)
                    && s.settings.welcome_completed,
                s.git.clone(),
            )
        };
        let Some(git) = git.filter(|_| enabled) else {
            return;
        };
        let task = cx
            .background_executor()
            .spawn(async move { corvene_git::global_config_value(git, "user.email") });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let email = task.await;
            if let Some(banner) = crate::mco::Banner::for_git_email(&account, email.as_deref()) {
                cx.update(|cx| Self::set_banner(banner, cx));
            }
        })
        .detach();
    }

    /// `AccountsStore.refresh` at launch: re-read every account's profile
    /// (name, avatar, e-mails, plan). A failure keeps the stored account.
    pub fn refresh_accounts(cx: &mut dyn Host) {
        let accounts = Self::state(cx).read(cx).accounts.clone();
        for account in accounts {
            let task = cx.background_executor().spawn(async move {
                let token = corvene_platform::keychain::token(&account.host(), &account.login)
                    .ok()
                    .flatten()?;
                let endpoint = corvene_github::Endpoint::from_api_base(&account.endpoint);
                let client = corvene_github::Client::new(endpoint, token);
                match client.current_user(account.scopes.clone()) {
                    Ok(updated) if updated.login == account.login => Some(updated),
                    Ok(_) => None,
                    Err(err) => {
                        warn!(%err, login = %account.login, "could not refresh account");
                        None
                    }
                }
            });
            cx.spawn(async move |cx: &mut AsyncCtx| {
                let Some(updated) = task.await else {
                    return;
                };
                cx.update(|cx| {
                    Self::state(cx).update(cx, |s, cx| {
                        let Some(slot) = s
                            .accounts
                            .iter_mut()
                            .find(|a| a.endpoint == updated.endpoint && a.login == updated.login)
                        else {
                            return;
                        };
                        *slot = updated;
                        if let Err(err) = s.store.save_accounts(&s.accounts) {
                            error!(?err, "could not save accounts");
                        }
                        cx.notify();
                    });
                });
            })
            .detach();
        }
    }

    /// Stop the authentication flow in progress (the sign-in store's
    /// Authentication step stays, no longer loading).
    pub fn cancel_sign_in(cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            if cancel_authentication(s) {
                sign_in_store(s).authentication_stopped();
                cx.notify();
            }
        });
    }

    /// GHD `removeAccount` for the account signed in to `endpoint`.
    pub fn sign_out(endpoint: String, cx: &mut dyn Host) {
        let login = Self::state(cx)
            .read(cx)
            .account_for(&endpoint)
            .map(|a| a.login.clone());
        if let Some(login) = login {
            Self::sign_out_account(endpoint, login, cx);
        }
    }

    /// GHD `removeAccount` for `login`'s account on `endpoint`; the
    /// endpoint's other accounts stay signed in (`527-multiple-accounts`).
    /// Repositories that used it keep its login, so they use it again
    /// after signing back in, and the endpoint's first account meanwhile.
    pub fn sign_out_account(endpoint: String, login: String, cx: &mut dyn Host) {
        let failed = Self::state(cx).update(cx, |s, cx| {
            let account = s.account_with_login(&endpoint, &login).cloned()?;
            let removed = crate::accounts::remove_account(
                &s.store,
                &mut s.accounts,
                &account,
                &crate::accounts::Keychain,
            );
            cx.notify();
            removed.err()
        });
        if let Some(err) = failed {
            Self::show_error("Could not sign out", err.to_string(), cx);
        }
    }

    // ---- welcome / identity ----

    /// `git config --global user.name/user.email` (GHD `ConfigureGitUser` save).
    pub fn set_global_identity(name: String, email: String, cx: &mut dyn Host) {
        let Some(git) = Self::state(cx).read(cx).git.clone() else {
            return;
        };
        let task = cx
            .background_executor()
            .spawn(async move { corvene_git::set_global_identity(git, &name, &email) });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            if let Err(err) = task.await {
                cx.update(|cx| Self::show_error("Could not save Git identity", &err, cx));
            }
        })
        .detach();
    }

    /// `_endWelcomeFlow` → `markWelcomeFlowComplete`.
    pub fn complete_welcome(cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            s.settings.welcome_completed = true;
            crate::persistence::mark_welcome_flow_complete(&s.store);
            cx.notify();
        });
    }

    /// `onHighlightShas`: dim every history row except `shas` (empty = none).
    pub fn set_highlighted_shas(id: u64, shas: Vec<String>, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            let rs = s.repo_state_mut(id);
            if rs.highlighted_shas != shas {
                rs.highlighted_shas = shas;
                cx.notify();
            }
        });
    }

    /// `dragAndDropManager.emitEnterDropTarget` / `emitLeaveDropTarget`.
    pub fn set_drag_target(target: Option<crate::state::DropTarget>, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            if s.drag_target != target {
                s.drag_target = target;
                cx.notify();
            }
        });
    }

    // ---- settings ----

    /// Corvene (`618-keymap-overrides`): read `keymap.json` (nothing while
    /// the flag is off); the main loop hands it to the keymap. Problems
    /// reading it show a banner, once per distinct set.
    pub fn load_keymap_overrides(cx: &mut dyn Host) {
        let on = Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::KEYMAP_OVERRIDES);
        let path = crate::keymap_file::path();
        let (overrides, errors) = if on {
            crate::keymap_file::load(&path)
        } else {
            Default::default()
        };
        let report = Self::state(cx).update(cx, |s, cx| {
            if s.keymap_overrides != overrides {
                s.keymap_overrides = overrides;
                cx.notify();
            }
            let new = s.keymap_load_errors != errors;
            s.keymap_load_errors = errors.clone();
            new && !errors.is_empty()
        });
        if report {
            Self::report_config_file_errors(path, errors, cx);
        }
    }

    /// Corvene (`522-settings-file`): what `settings_file::apply_at_launch`
    /// took from the file, and what of it could not be used (a banner).
    pub fn set_settings_file(
        overlay: crate::settings_file::SettingsOverlay,
        flags: crate::flags::EnvFlags,
        errors: Vec<String>,
        cx: &mut dyn Host,
    ) {
        Self::state(cx).update(cx, |s, _| {
            s.settings_overlay = overlay;
            s.settings_file_flags = flags;
        });
        if !errors.is_empty() {
            Self::report_config_file_errors(corvene_platform::paths::settings_file(), errors, cx);
        }
    }

    /// Corvene (`522-settings-file`): Settings › Advanced › "Export
    /// Settings…" writes the settings in effect and the flags spec to `path`.
    pub fn export_settings(path: PathBuf, cx: &mut dyn Host) {
        let text = {
            let s = Self::state(cx).read(cx);
            crate::settings_file::export(
                &s.settings,
                Some(&crate::flags::env::render(&s.flag_overrides)),
            )
        };
        crate::remote::spawn_bg(
            cx,
            move || {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, text)
            },
            |result, cx| {
                if let Err(err) = result {
                    Self::show_error("Could not export settings", err.to_string(), cx);
                }
            },
        );
    }

    /// Corvene (`618-keymap-overrides`, `522-settings-file`): a banner
    /// naming the configuration file and what in it could not be used.
    pub fn report_config_file_errors(path: PathBuf, errors: Vec<String>, cx: &mut dyn Host) {
        for err in &errors {
            warn!(path = %path.display(), "{err}");
        }
        Self::set_banner(crate::mco::Banner::ConfigFileErrors { path, errors }, cx);
    }

    /// A failed save is logged; with `report-settings-save-errors` it is also
    /// shown (once while that error popup is up), where GitHub Desktop's
    /// `setItem` failures go unnoticed (desktop/desktop#5046).
    pub fn update_settings(cx: &mut dyn Host, edit: impl FnOnce(&mut Settings)) {
        const TITLE: &str = "Could not save settings";
        let failed = Self::state(cx).update(cx, |s, cx| {
            edit(&mut s.settings);
            cx.notify();
            // `522-settings-file`: what the file set is not saved
            let stored = s.settings_overlay.stored_form(&s.settings);
            let err = s.store.save_settings(&stored).err()?;
            error!(?err, "could not save settings");
            let shown = s
                .popups
                .all_popups()
                .iter()
                .any(|p| matches!(&p.popup, Popup::Error { title, .. } if title == TITLE));
            (s.flags.bool(crate::flags::ids::REPORT_SETTINGS_SAVE_ERRORS) && !shown)
                .then(|| err.to_string())
        });
        if let Some(message) = failed {
            Self::show_error(TITLE, message, cx);
        }
        // `117-file-icons`: a new icon theme choice loads (no-op otherwise)
        Self::load_file_icon_theme(false, cx);
    }
}

/// GHD `_showPopup` (`PopupManager.addPopup`) on `s`.
///
/// Deviation: a popup of a type already on the stack replaces that one in
/// place (it keeps its id and its place in the stack), where GitHub
/// Desktop leaves the stack alone (`app/src/lib/popup-manager.ts`
/// `addPopup`): Corvene keeps a dialog's data in its `Popup` value, so
/// showing the dialog again is how that data changes (GitHub Desktop
/// changes it with `updatePopup` or in the repository state). Error popups
/// repeat, as in GitHub Desktop.
pub(crate) fn show_popup_in(s: &mut AppState, popup: Popup) {
    // GHD `showDotComSignInDialog` / `showEnterpriseSignInDialog`
    if let Popup::SignIn { enterprise } = popup {
        begin_sign_in_store(s, enterprise, None);
    }
    add_popup_in(s, popup);
}

/// [`show_popup_in`] without its side effects. A popup bound to a
/// repository another window shows (a push error, a conflicts dialog that
/// finished after the user moved on) opens in that window
/// (`429-multiple-windows`).
fn add_popup_in(s: &mut AppState, popup: Popup) {
    let elsewhere = popup
        .repository()
        .filter(|repo| s.selected != Some(*repo))
        .and_then(|repo| s.workspace_showing(repo));
    if let Some(workspace) = elsewhere {
        let before = std::mem::replace(&mut s.current, workspace);
        add_popup_here(s, popup);
        s.current = before;
    } else {
        add_popup_here(s, popup);
    }
}

fn add_popup_here(s: &mut AppState, popup: Popup) {
    if !popup.is_error()
        && let Some(existing) = s
            .popups
            .all_popups()
            .iter()
            .find(|p| p.popup_type() == popup.popup_type())
    {
        let id = existing.id;
        s.popups
            .update_popup(crate::popup_manager::StackedPopup { id, popup });
        return;
    }
    s.popups.add_popup(popup);
}

/// What closing `popup` ends: a closed sign-in dialog stops its
/// authentication flow and resets the sign-in store (GHD's `SignIn`
/// `onDismissed` → `resetSignInState`, whose result callback hears
/// `Cancelled`).
fn closed_popup(s: &mut AppState, popup: &Popup) {
    if matches!(popup, Popup::SignIn { .. }) {
        cancel_authentication(s);
        sign_in_store(s).reset();
        s.extra_oauth_scopes.clear();
        s.adding_account = false;
    }
    // GHD `HookFailed.onDismissed`: `resolve('abort')` (a no-op once the
    // dialog answered)
    if let Popup::HookFailed { reply, .. } = popup {
        reply.send(corvene_git::hooks::HookFailureResolution::Abort);
    }
}

/// The sign-in store with its accounts brought up to date first (GHD's
/// follows `AccountsStore.onDidUpdate`).
fn sign_in_store(s: &mut AppState) -> &mut SignInStore {
    s.sign_in_accounts.replace(s.accounts.clone());
    &mut s.sign_in_store
}

/// GHD `beginDotComSignIn` / `beginEnterpriseSignIn`.
fn begin_sign_in_store(s: &mut AppState, enterprise: bool, callback: Option<ResultCallback>) {
    cancel_authentication(s);
    s.sign_in_store.allow_multiple = s.multiple_accounts();
    let store = sign_in_store(s);
    if enterprise {
        store.begin_enterprise_sign_in(callback);
    } else {
        store.begin_dot_com_sign_in(callback);
    }
}

/// Stops the authentication flow in progress; `false` when there is none.
fn cancel_authentication(s: &mut AppState) -> bool {
    let Some(flow) = s.authentication.take() else {
        return false;
    };
    flow.cancel.store(true, Ordering::SeqCst);
    true
}

/// The sign-in succeeded: the SignIn dialog closes and the store resets
/// (GHD's `SignIn` dismisses itself at the Success step).
fn close_sign_in_popups(s: &mut AppState) {
    let open: Vec<u64> = s
        .popups
        .all_popups()
        .iter()
        .filter(|p| matches!(p.popup, Popup::SignIn { .. }))
        .filter_map(|p| p.id)
        .collect();
    for popup_id in open {
        s.popups.remove_popup_by_id(popup_id);
    }
    sign_in_store(s).reset();
    s.adding_account = false;
}

/// `527-multiple-accounts`: what Add account says when the browser signed
/// in an account that is signed in already.
fn already_signed_in_hint(account: &corvene_models::Account) -> String {
    format!(
        "You are already signed in as @{} on {}. To add another account, sign in to it \
         on {} in your browser first (or pick it in the account picker there), then try \
         again.",
        account.login,
        account.friendly_endpoint(),
        account.friendly_endpoint(),
    )
}

/// Corvene (`525-account-commit-email`): the key of `account`'s commit
/// email in `Settings::account_commit_emails`.
pub fn account_commit_email_key(account: &corvene_models::Account) -> String {
    account.key()
}

pub(crate) fn persist_repositories(s: &mut AppState) {
    if let Err(err) = s.store.save_repositories(&s.repositories) {
        error!(?err, "could not save repositories");
    }
}

/// `explain-trust-failure`: why Trust Repository did not help, with the
/// `safe.directory` value git suggests (else the path as git printed it).
fn trust_failure_message(path: &Path, suggested: Option<&str>) -> String {
    let path = path.display().to_string();
    let value = suggested.unwrap_or(&path);
    format!(
        "{path} was added to the safe.directory list in your global Git config, but Git still \
         does not trust it. This happens when the folder is on a network share, a WSL or UNC \
         path or a file system that does not record its owner, because Git compares the path \
         in the form it sees, not the one it printed.\n\nAdd the value Git suggests instead:\n\n\
         git config --global --add safe.directory '{value}'\n\nor, if you trust every \
         repository on this computer, use '*' as the value."
    )
}

/// How a clone ended ([`Dispatcher::start_clone`]).
enum CloneOutcome {
    /// Cloned and added.
    Added,
    /// `285-clone-keeps-repo-on-submodule-failure`: cloned and added, a
    /// submodule failed.
    SubmodulesFailed(GitError),
    /// Stopped (`234-clone-cancel`); git removed what it created.
    Cancelled,
    Failed(GitError),
}

/// Corvene (`285-clone-keeps-repo-on-submodule-failure`): the error after a
/// clone whose submodules failed.
fn submodules_failed_message(err: &GitError) -> ErrorMessage {
    ErrorMessage::explained(
        err,
        Some(
            "The repository was cloned and added, but some of its submodules could not be \
             cloned. Fix their URLs or access and run git submodule update --init --recursive \
             in the repository."
                .to_string(),
        ),
    )
}

/// Corvene (`286-initial-commit-skips-large-files`): the notice naming the
/// files Create Repository left out of the initial commit.
fn large_files_left_out_message(paths: &[String]) -> String {
    const SHOWN: usize = 10;
    let (them, are) = if paths.len() == 1 {
        ("This file is", "it stays")
    } else {
        ("These files are", "they stay")
    };
    let mut list: Vec<String> = paths.iter().take(SHOWN).map(|p| format!("• {p}")).collect();
    if paths.len() > SHOWN {
        list.push(format!("and {} more", paths.len() - SHOWN));
    }
    format!(
        "{them} larger than GitHub's 100 MB limit, so {are} in the folder, \
         uncommitted, instead of going into the initial commit:\n\n{}\n\n\
         Track them with Git LFS or add them to .gitignore.",
        list.join("\n")
    )
}

pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Branch/stash facts gathered during `refresh_repository`.
struct RefreshExtras {
    line_stats: std::collections::HashMap<String, corvene_git::LineStats>,
    branch_tracking: std::collections::HashMap<String, corvene_git::BranchTracking>,
    recent_branches: Vec<String>,
    default_branch: Option<String>,
    /// `1221-stacked-branch-refs`
    stacked_refs: Option<Arc<crate::stacked_refs::StackedRefs>>,
    stash: Option<corvene_models::StashEntry>,
    stash_count: usize,
    stashed_branches: Vec<String>,
    stashes: Vec<corvene_models::StashEntry>,
    rebase_snapshot: Option<corvene_git::RebaseSnapshot>,
    cherry_pick_snapshot: Option<corvene_git::CherryPickSnapshot>,
    /// The local branches at `MERGE_HEAD` (`getBranchesPointedAt`).
    merge_head_branches: Option<Vec<String>>,
    last_fetched: Option<std::time::SystemTime>,
    pull_with_rebase: bool,
    /// `340-message-rules-defer-to-hooks`: git runs a `prepare-commit-msg`
    /// or `commit-msg` hook on a commit.
    commit_message_hook: bool,
    /// `526-commit-signing`: the effective `commit.gpgsign`.
    signs_commits: bool,
    /// `1112-sparse-checkout`: while sparse checkout is on.
    sparse_checkout: Option<crate::sparse_checkout::SparseSummary>,
    /// `1113-lfs-locks`: `.gitattributes` has `filter=lfs`.
    uses_lfs: bool,
    /// `1103-implicit-upstream-push-default`
    implicit_upstream: Option<(String, corvene_models::AheadBehind)>,
    /// `1109-remote-manager`
    push_target: Option<crate::remote_manager::PushTarget>,
    worktrees: Vec<corvene_models::WorktreeEntry>,
    upstream_rewritten: bool,
    /// `1202-update-from-parent-branch`: the branch the current one was
    /// created from, when it still exists.
    update_parent: Option<String>,
    last_local_commit: Option<crate::state::LastCommit>,
    incoming_commits: Vec<String>,
}

/// GHD parses a branch's remote name up to the first `/`
/// (`remote-names-with-slashes` off): drop the names matched against the
/// configured remotes so [`corvene_models::Branch`] falls back to that split.
pub(crate) fn forget_remote_names(info: &mut corvene_models::RepositoryInfo) {
    for branch in &mut info.branches {
        branch.remote_name = None;
    }
    if let corvene_models::Tip::Valid { branch } = &mut info.tip {
        branch.remote_name = None;
    }
}

/// The options a working-directory diff depends on.
#[derive(Clone, Copy, Debug)]
pub(crate) struct WorkingDiffOptions {
    hide_whitespace: bool,
    renamed_against_head: bool,
    symlinks_as_links: bool,
    as_text: bool,
    /// `794-svg-image-diff`
    svg_as_image: bool,
    /// `795-lfs-image-previews`
    lfs_images: bool,
    /// `796-lfs-text-diff`
    lfs_text: bool,
}

impl WorkingDiffOptions {
    pub(crate) fn of(s: &AppState, rs: &RepositoryState, path: &str) -> Self {
        Self {
            hide_whitespace: s.settings.hide_whitespace_in_changes_diff,
            renamed_against_head: s.flags.bool(crate::flags::ids::RENAMED_DIFF_AGAINST_HEAD),
            symlinks_as_links: s.flags.bool(crate::flags::ids::SYMLINK_CONTENTS),
            as_text: rs.diff_as_text.as_deref() == Some(path)
                && s.flags.bool(crate::flags::ids::BINARY_DIFF_AS_TEXT),
            svg_as_image: Dispatcher::svg_shown_as_image(s, rs, path),
            lfs_images: s.flags.bool(crate::flags::ids::LFS_IMAGE_PREVIEWS),
            lfs_text: s.flags.bool(crate::flags::ids::LFS_TEXT_DIFF),
        }
    }

    pub(crate) fn key(self) -> [bool; 7] {
        [
            self.hide_whitespace,
            self.renamed_against_head,
            self.symlinks_as_links,
            self.as_text,
            self.svg_as_image,
            self.lfs_images,
            self.lfs_text,
        ]
    }
}

/// Corvene `1208-undo-restores-line-selection`, after `commit` was undone:
/// for each file it changed that is listed now, the selection of the lines
/// it made (`renamed_against_head` as the Changes tab diffs) and the diff
/// it names lines of. Nothing for a root commit (its files become
/// untracked, all selected) or a huge one.
fn restored_selections(
    git: Arc<corvene_git::GitBinary>,
    workdir: &Path,
    commit: &corvene_models::Commit,
    renamed_against_head: bool,
) -> corvene_git::error::Result<
    Vec<(
        String,
        corvene_models::DiffSelection,
        Arc<corvene_models::Diff>,
    )>,
> {
    let Some(parent) = commit.parents.first() else {
        return Ok(Vec::new());
    };
    let Some(made) = corvene_git::lines_made_by_commit(git.clone(), workdir, parent, &commit.sha)?
    else {
        return Ok(Vec::new());
    };
    let status = corvene_git::get_status(git.clone(), workdir)?;
    let mut out = Vec::new();
    for file in status.files.iter().filter(|f| !f.status.submodule) {
        let Some(lines) = made.get(&file.path) else {
            continue;
        };
        let diff = corvene_git::working_directory_diff(
            git.clone(),
            workdir,
            file,
            false,
            renamed_against_head,
            false,
            None,
        )?;
        let Some(hunks) = diff.hunks() else { continue };
        let selection = crate::line_selection::selection_from_commit(hunks, lines);
        out.push((file.path.clone(), selection, Arc::new(diff)));
    }
    Ok(out)
}

/// `file`'s diff against `HEAD` with the working copy and the committed
/// contents (hunk expansion, highlighting). Blocking.
pub(crate) fn compute_working_diff(
    git: Arc<corvene_git::GitBinary>,
    workdir: &Path,
    file: &WorkingDirectoryFileChange,
    options: WorkingDiffOptions,
    cancel: Option<&corvene_git::CancelToken>,
) -> LoadedDiff {
    // the old side is read in-process meanwhile
    std::thread::scope(|scope| {
        // GHD `getOldFileContent`: what is committed (`HEAD`), not the index
        let old = (!matches!(
            file.status.kind,
            corvene_models::FileStatusKind::New | corvene_models::FileStatusKind::Untracked
        ))
        .then(|| {
            let git = git.clone();
            scope.spawn(move || {
                let old_path = file.old_path.as_deref().unwrap_or(&file.path);
                corvene_git::blob_lines(git, workdir, "HEAD", old_path)
            })
        });
        if options.svg_as_image {
            // `794-svg-image-diff`: the committed and working images, no lines
            let previous_path = file.old_path.as_deref().unwrap_or(&file.path);
            let diff = corvene_git::image_diff_as(
                corvene_git::SVG_MEDIA_TYPE,
                file.status.kind,
                || std::fs::read(workdir.join(&file.path)).ok(),
                || corvene_git::blob_bytes(git.clone(), workdir, "HEAD", previous_path).ok(),
            );
            return (Arc::new(diff), None, None);
        }
        let diff = corvene_git::working_directory_diff(
            git.clone(),
            workdir,
            file,
            options.hide_whitespace,
            options.renamed_against_head,
            options.as_text,
            cancel,
        )
        .unwrap_or_else(|err| {
            if !matches!(err, corvene_git::GitError::Cancelled(_)) {
                warn!(%err, "diff failed");
            }
            corvene_models::Diff::Empty
        });
        // `795-lfs-image-previews`: an LFS image's pointers become its images
        let diff = if options.lfs_images {
            let previous_path = file.old_path.as_deref().unwrap_or(&file.path);
            corvene_git::lfs::resolve_lfs_images(
                git.clone(),
                workdir,
                &file.path,
                file.status.kind,
                diff,
                || std::fs::read(workdir.join(&file.path)).ok(),
                || corvene_git::blob_bytes(git.clone(), workdir, "HEAD", previous_path).ok(),
            )
        } else {
            diff
        };
        // `796-lfs-text-diff`: another LFS file's contents instead of its pointers
        let diff = if options.lfs_text {
            let previous_path = file.old_path.as_deref().unwrap_or(&file.path);
            corvene_git::lfs::resolve_lfs_text(
                git.clone(),
                workdir,
                file.status.kind,
                diff,
                || Some(corvene_git::lfs::LfsSide::File(workdir.join(&file.path))),
                || {
                    corvene_git::blob_bytes(git.clone(), workdir, "HEAD", previous_path)
                        .ok()
                        .map(corvene_git::lfs::LfsSide::Blob)
                },
            )
        } else {
            diff
        };
        // GHD `fileContents.newContents`: the working copy, for hunk expansion.
        let contents = (file.status.kind != corvene_models::FileStatusKind::Deleted)
            .then(|| {
                corvene_git::working_file_lines(workdir, &file.path, options.symlinks_as_links)
            })
            .flatten();
        let old = old.and_then(join);
        (Arc::new(diff), contents.map(Arc::new), old.map(Arc::new))
    })
}

/// `883-unpublished-commit-links`: with this many local-only commits or more
/// none is marked (links stay as in GHD).
const UNPUBLISHED_COMMITS_LIMIT: usize = 10_000;

/// `793-hide-whitespace-only-files`: `data` (the files of `ordered`, oldest
/// first) without the files whose changes are whitespace only, and how
/// many those were. Unchanged when git cannot tell.
fn without_whitespace_only_files(
    git: Arc<corvene_git::GitBinary>,
    workdir: &Path,
    ordered: &[String],
    data: Arc<corvene_models::ChangesetData>,
) -> (Arc<corvene_models::ChangesetData>, usize) {
    let (Some(oldest), Some(newest)) = (ordered.first(), ordered.last()) else {
        return (data, 0);
    };
    let base = format!("{oldest}^");
    let hidden = corvene_git::whitespace_only_paths(git.clone(), workdir, &[&base, newest])
        // a root commit: against the empty tree
        .or_else(|_| {
            corvene_git::whitespace_only_paths(git, workdir, &[corvene_git::NULL_TREE_SHA, newest])
        })
        .unwrap_or_default();
    if hidden.is_empty() {
        return (data, 0);
    }
    let mut data = Arc::unwrap_or_clone(data);
    let before = data.files.len();
    data.files.retain(|f| !hidden.contains(&f.path));
    let count = before - data.files.len();
    (Arc::new(data), count)
}

/// The changed files of one commit or of a contiguous range (oldest first).
/// `in_process`: flag `907-in-process-commit-files`.
/// Corvene `1319-trivial-change-icon` (`trivial`): the files' trivial
/// changes are read with them.
fn compute_changeset(
    git: Arc<corvene_git::GitBinary>,
    workdir: &Path,
    ordered: &[String],
    in_process: bool,
    trivial: bool,
) -> corvene_git::error::Result<Arc<corvene_models::ChangesetData>> {
    let mut data = if ordered.len() > 1 {
        corvene_git::get_commit_range_changed_files(git, workdir, ordered, in_process)
    } else {
        corvene_git::get_changed_files(git, workdir, &ordered[0], in_process)
    }?;
    if trivial {
        read_trivial_changes(workdir, ordered, &mut data);
    }
    Ok(Arc::new(data))
}

/// Corvene `1319-trivial-change-icon`: classify the files of `ordered`
/// (one commit or a range, oldest first) against the oldest one's parent.
fn read_trivial_changes(
    workdir: &Path,
    ordered: &[String],
    data: &mut corvene_models::ChangesetData,
) {
    if let (Some(oldest), Some(newest)) = (ordered.first(), ordered.last()) {
        data.trivial = corvene_git::trivial_change::classify_committed(
            workdir,
            &format!("{oldest}^"),
            newest,
            &data.files,
        );
    }
    data.trivial_read = true;
}

/// Corvene `1319-trivial-change-icon`: a cached changeset loaded while the
/// flag was off, classified now (and cached again).
fn with_trivial_changes(
    workdir: &Path,
    ordered: &[String],
    data: Arc<corvene_models::ChangesetData>,
) -> Arc<corvene_models::ChangesetData> {
    if data.trivial_read {
        return data;
    }
    let mut data = Arc::unwrap_or_clone(data);
    read_trivial_changes(workdir, ordered, &mut data);
    let data = Arc::new(data);
    crate::diff_cache::store_changeset(workdir, ordered, data.clone());
    data
}

/// What a committed file's diff depends on besides the commits.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CommitDiffOptions {
    hide_whitespace: bool,
    /// `795-lfs-image-previews`
    lfs_images: bool,
    /// `796-lfs-text-diff`
    lfs_text: bool,
}

impl CommitDiffOptions {
    pub(crate) fn of(s: &AppState) -> Self {
        Self {
            hide_whitespace: s.settings.hide_whitespace_in_history_diff,
            lfs_images: s.flags.bool(crate::flags::ids::LFS_IMAGE_PREVIEWS),
            lfs_text: s.flags.bool(crate::flags::ids::LFS_TEXT_DIFF),
        }
    }
}

/// `file`'s diff in `ordered` (one commit or a range, oldest first) with its
/// new and old contents; the three parts are read in parallel. Blocking.
pub(crate) fn compute_commit_diff(
    git: Arc<corvene_git::GitBinary>,
    workdir: &Path,
    ordered: &[String],
    file: &corvene_models::CommittedFileChange,
    options: CommitDiffOptions,
) -> LoadedDiff {
    let hide_whitespace = options.hide_whitespace;
    let newest = match ordered {
        [_, .., newest] => newest.clone(),
        _ => file.commitish.clone(),
    };
    // GHD `parentCommitish`: the parent of the oldest selected commit
    let oldest = ordered
        .first()
        .cloned()
        .unwrap_or_else(|| file.commitish.clone());
    std::thread::scope(|scope| {
        let contents = (file.status.kind != corvene_models::FileStatusKind::Deleted).then(|| {
            let (git, newest) = (git.clone(), newest.as_str());
            scope.spawn(move || corvene_git::blob_lines(git, workdir, newest, &file.path))
        });
        let old = (!matches!(
            file.status.kind,
            corvene_models::FileStatusKind::New | corvene_models::FileStatusKind::Untracked
        ))
        .then(|| {
            let git = git.clone();
            let parent = format!("{oldest}^");
            scope.spawn(move || {
                let old_path = file.old_path.as_deref().unwrap_or(&file.path);
                corvene_git::blob_lines(git, workdir, &parent, old_path)
            })
        });
        let diff = match ordered {
            [oldest, .., newest] => corvene_git::commit_range_file_diff(
                git.clone(),
                workdir,
                file,
                oldest,
                newest,
                hide_whitespace,
            ),
            _ => corvene_git::commit_file_diff(git.clone(), workdir, file, hide_whitespace),
        }
        .unwrap_or_else(|err| {
            warn!(%err, "commit diff failed");
            corvene_models::Diff::Empty
        });
        // `795-lfs-image-previews`: an LFS image's pointers become its images
        let diff = if options.lfs_images {
            let previous_path = file.old_path.as_deref().unwrap_or(&file.path);
            corvene_git::lfs::resolve_lfs_images(
                git.clone(),
                workdir,
                &file.path,
                file.status.kind,
                diff,
                || corvene_git::blob_bytes(git.clone(), workdir, &newest, &file.path).ok(),
                || {
                    corvene_git::blob_bytes(
                        git.clone(),
                        workdir,
                        &format!("{oldest}^"),
                        previous_path,
                    )
                    .ok()
                },
            )
        } else {
            diff
        };
        // `796-lfs-text-diff`: another LFS file's contents instead of its pointers
        let diff = if options.lfs_text {
            let previous_path = file.old_path.as_deref().unwrap_or(&file.path);
            let blob = |rev: &str, path: &str| {
                corvene_git::blob_bytes(git.clone(), workdir, rev, path)
                    .ok()
                    .map(corvene_git::lfs::LfsSide::Blob)
            };
            corvene_git::lfs::resolve_lfs_text(
                git.clone(),
                workdir,
                file.status.kind,
                diff,
                || blob(&newest, &file.path),
                || blob(&format!("{oldest}^"), previous_path),
            )
        } else {
            diff
        };
        (
            Arc::new(diff),
            contents.and_then(join).map(Arc::new),
            old.and_then(join).map(Arc::new),
        )
    })
}

/// Assign `value` to `slot`; whether that changed it.
fn set<T: PartialEq>(slot: &mut T, value: T) -> bool {
    if *slot == value {
        return false;
    }
    *slot = value;
    true
}

/// Run `f` with its own handle on `git` on a thread of `scope`.
fn spawn_git<'scope, T: Send + 'scope>(
    scope: &'scope std::thread::Scope<'scope, '_>,
    git: &Arc<corvene_git::GitBinary>,
    f: impl FnOnce(Arc<corvene_git::GitBinary>) -> T + Send + 'scope,
) -> std::thread::ScopedJoinHandle<'scope, T> {
    let git = git.clone();
    scope.spawn(move || f(git))
}

/// A scoped thread's result; its panic continues on this thread.
fn join<T>(handle: std::thread::ScopedJoinHandle<'_, T>) -> T {
    handle
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

/// `709-new-untracked-files-excluded`: untracked files that were not listed
/// before start left out of the next commit (GHD includes every new file).
fn exclude_new_untracked(
    status: &mut corvene_models::WorkingDirectoryStatus,
    previous: Option<&corvene_models::WorkingDirectoryStatus>,
) {
    let known: std::collections::HashSet<&str> = previous
        .map(|p| p.files.iter().map(|f| f.path.as_str()).collect())
        .unwrap_or_default();
    for file in &mut status.files {
        if file.status.kind == corvene_models::FileStatusKind::Untracked
            && !known.contains(file.path.as_str())
        {
            file.selection = corvene_models::DiffSelection::none();
        }
    }
}

/// Node's `path.resolve(path)`: made absolute against the current directory,
/// with `.` and `..` components folded lexically (symlinks untouched).
pub(crate) fn resolve_path(path: &std::path::Path) -> PathBuf {
    use std::path::Component;
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// A loaded diff with the new and old file contents (hunk expansion,
/// highlighting), shared with the diff caches.
pub(crate) type LoadedDiff = (
    Arc<corvene_models::Diff>,
    Option<Arc<Vec<String>>>,
    Option<Arc<Vec<String>>>,
);

type DiffSlots<'a> = (
    &'a mut Option<Arc<corvene_models::Diff>>,
    &'a mut Option<Arc<Vec<String>>>,
    &'a mut Option<Arc<Vec<String>>>,
);

/// Store a freshly loaded diff and its file contents; `true` when any of
/// them changed. The diff view starts over (rows, highlighting, text
/// selection, expanded hunks) when its generation moves, and every refresh
/// (window focus, the filesystem watcher, after git commands) reloads the
/// selected file's diff, which is usually the same as before: callers only
/// bump the generation on `true`.
pub(crate) fn replace_diff(
    (diff, contents, old_contents): DiffSlots<'_>,
    (new_diff, new_contents, new_old): LoadedDiff,
) -> bool {
    fn same<T: PartialEq>(current: &Option<Arc<T>>, new: &Option<Arc<T>>) -> bool {
        match (current, new) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b) || a == b,
            (None, None) => true,
            _ => false,
        }
    }
    if diff
        .as_ref()
        .is_some_and(|d| Arc::ptr_eq(d, &new_diff) || **d == *new_diff)
        && same(contents, &new_contents)
        && same(old_contents, &new_old)
    {
        return false;
    }
    *diff = Some(new_diff);
    *contents = new_contents;
    *old_contents = new_old;
    true
}

/// Whether any of `committed` is among `local` (flag `818`).
fn paths_overlap<'a>(mut committed: impl Iterator<Item = &'a String>, local: &[String]) -> bool {
    let local: std::collections::HashSet<&str> = local.iter().map(String::as_str).collect();
    committed.any(|p| local.contains(p.as_str()))
}

#[cfg(test)]
mod trust_failure_tests {
    use super::trust_failure_message;
    use std::path::Path;

    #[test]
    fn suggests_the_value_git_names() {
        let message = trust_failure_message(
            Path::new("//server/share/repo"),
            Some("%(prefix)///server/share/repo"),
        );
        assert!(message.contains("safe.directory '%(prefix)///server/share/repo'"));
        // never mistaken for git's own refusal (that re-opens the trust view)
        assert!(corvene_git::dubious_ownership_path(&message).is_none());
        let message = trust_failure_message(Path::new("/mnt/c/repo"), None);
        assert!(message.contains("safe.directory '/mnt/c/repo'"));
    }
}

#[cfg(test)]
mod paths_overlap_tests {
    use super::paths_overlap;

    #[test]
    fn overlap_needs_a_shared_path() {
        let local = vec!["a.txt".to_string(), "old.txt".to_string()];
        let committed = ["b.txt".to_string(), "c.txt".to_string()];
        assert!(!paths_overlap(committed.iter(), &local));
        let committed = ["b.txt".to_string(), "old.txt".to_string()];
        assert!(paths_overlap(committed.iter(), &local));
        assert!(!paths_overlap([].iter(), &local));
    }
}

#[cfg(test)]
mod resolve_path_tests {
    use super::resolve_path;
    use std::path::Path;

    #[test]
    fn folds_dot_segments() {
        #[cfg(not(windows))]
        assert_eq!(
            resolve_path(Path::new("/a/b/../c/./d")),
            Path::new("/a/c/d").to_path_buf()
        );
        #[cfg(windows)]
        assert_eq!(
            resolve_path(Path::new(r"C:\a\b\..\c\.\d")),
            Path::new(r"C:\a\c\d").to_path_buf()
        );
        assert!(resolve_path(Path::new("x/../y")).is_absolute());
    }
}

#[cfg(test)]
mod exclude_new_untracked_tests {
    use super::exclude_new_untracked;
    use corvene_models::DiffSelectionType;

    #[test]
    fn only_newly_listed_untracked_files_are_excluded() {
        let before = corvene_git::parse_porcelain_v2(b"? old.txt\0");
        let mut after = corvene_git::parse_porcelain_v2(
            b"1 .M N... 100644 100644 100644 aaa bbb tracked.rs\0? old.txt\0? new.txt\0",
        );
        exclude_new_untracked(&mut after, Some(&before));
        let kind = |path: &str| {
            after
                .files
                .iter()
                .find(|f| f.path == path)
                .map(|f| f.selection.kind())
        };
        assert_eq!(kind("tracked.rs"), Some(DiffSelectionType::All));
        assert_eq!(kind("old.txt"), Some(DiffSelectionType::All));
        assert_eq!(kind("new.txt"), Some(DiffSelectionType::None));

        let mut first = corvene_git::parse_porcelain_v2(b"? a.txt\0");
        exclude_new_untracked(&mut first, None);
        assert_eq!(first.files[0].selection.kind(), DiffSelectionType::None);
    }
}

#[cfg(test)]
mod replace_diff_tests {
    use super::replace_diff;
    use std::sync::Arc;

    #[test]
    fn an_identical_reload_keeps_the_generation_and_the_contents() {
        let lines = || Some(Arc::new(vec!["fn main() {}".to_string()]));
        let empty = || Arc::new(corvene_models::Diff::Empty);
        let (mut diff, mut contents, mut old) = (None, None, None);
        assert!(replace_diff(
            (&mut diff, &mut contents, &mut old),
            (empty(), lines(), None),
        ));
        let kept = contents.clone().unwrap();
        assert!(!replace_diff(
            (&mut diff, &mut contents, &mut old),
            (empty(), lines(), None),
        ));
        // the same allocation, so the view's highlight cache still matches
        assert!(Arc::ptr_eq(&kept, contents.as_ref().unwrap()));
        assert!(replace_diff(
            (&mut diff, &mut contents, &mut old),
            (empty(), Some(Arc::new(vec!["fn main() { }".into()])), None),
        ));
        assert!(replace_diff(
            (&mut diff, &mut contents, &mut old),
            (
                empty(),
                Some(Arc::new(vec!["fn main() { }".into()])),
                lines()
            ),
        ));
    }
}

/// Android: what a repository outside the app-private filesystem needs
/// before git will work in it. Shared storage belongs to another user id
/// (git's "dubious ownership"), keeps neither file modes nor symbolic
/// links and folds case; a folder imported through the Storage Access
/// Framework arrived without its file modes. Failures are left for the
/// commands that follow to report.
#[cfg(target_os = "android")]
pub(crate) fn android_prepare_repository(git: Arc<corvene_git::GitBinary>, path: &Path) {
    if !path.join(".git").exists() {
        return;
    }
    let imported = corvene_platform::android::take_imported(path);
    if corvene_platform::android::is_shared_storage(path) {
        let trusted = corvene_git::global_config_values(git.clone(), "safe.directory");
        if !trusted.iter().any(|dir| Path::new(dir) == path) {
            let _ = corvene_git::add_safe_directory(git.clone(), path);
        }
        let _ = corvene_git::set_local_config_value(git.clone(), path, "core.symlinks", "false");
        let _ = corvene_git::set_local_config_value(git.clone(), path, "core.filemode", "false");
        if crate::shared_storage::folds_case(&path.join(".git")) {
            let _ = corvene_git::set_local_config_value(git, path, "core.ignorecase", "true");
        }
    } else if imported {
        let _ = corvene_git::set_local_config_value(git, path, "core.filemode", "false");
    }
}

/// `516-git-executable`: the flag's path, `~/` expanded against `home`;
/// `None` when empty.
fn configured_git(text: &str, home: Option<PathBuf>) -> Option<PathBuf> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    match (text.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => Some(home.join(rest)),
        _ => Some(PathBuf::from(text)),
    }
}

#[cfg(test)]
mod configured_git_tests {
    use super::*;

    #[test]
    fn expands_home_and_ignores_empty() {
        let home = Some(PathBuf::from("/Users/mona"));
        assert_eq!(configured_git("  ", home.clone()), None);
        assert_eq!(
            configured_git("~/bin/git", home.clone()),
            Some(PathBuf::from("/Users/mona/bin/git"))
        );
        assert_eq!(
            configured_git(" /opt/git/bin/git ", home),
            Some(PathBuf::from("/opt/git/bin/git"))
        );
    }
}
