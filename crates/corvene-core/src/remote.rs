//! Remote operations - GHD `app-store.ts` `performFetch` / `performPull` /
//! `performPush` / `_publishRepository`, `dispatcher.ts` `confirmOrForcePush`,
//! the `BackgroundFetcher` (every hour, at least 5 minutes apart) and the
//! `RepositoryIndicatorUpdater` (every 15 minutes), plus the Git LFS
//! initialisation prompt (`InitializeLFS`).
//!
//! Deviations (flags): a failed force push keeps the "Force push"
//! recommendation (`258-force-push-kept-on-failure`; GHD clears it first).
//! The background fetch can be off or cover any remote
//! (`244-background-fetch`; GHD: GitHub repositories only).
//! Fetch can prune tags deleted on the remote (`248-fetch-prune-tags`).
//! The LFS check can read `.gitattributes` instead of running
//! `git lfs track` (`905-lfs-detect-by-attributes`).
//! The background fetch can run without progress in the push/pull button,
//! and a push, pull or fetch asked for meanwhile waits for it
//! (`245-push-during-background-fetch`; GHD disables the button).
//! A local branch that is not checked out can be fast-forwarded from its
//! upstream (`857-update-branch-from-upstream`).
//! Repository › Fetch All Repositories fetches every listed repository
//! (`247-fetch-all-repositories`).
//! Indicators refresh right after launch and on opening the repository list
//! (`217-prompt-indicator-refresh`; GHD waits for the 15-minute updater).
//! A pull skips `remote set-head -a` while the remote's HEAD resolves
//! (`251-remote-head-once`; GHD runs it after every pull).
//! Fetch can extend the commit-graph (`249-fetch-writes-commit-graph`).
//! Fetch and pull can leave submodules alone (`250-sync-skips-submodules`).
//! The background fetch can fast-forward a clean branch that is only behind
//! (`246-background-fetch-fast-forwards`).
//! Force push is also recommended after a rewrite outside Corvene
//! (`260-force-push-after-outside-rewrite`).
//! A fetch or pull blocked by a stale remote-tracking ref prunes the remote
//! and retries once (`252-prune-stale-refs-and-retry`).
//! Between the hourly background fetches the selected GitHub repository is
//! fetched as soon as the API's `pushed_at` is newer than its last fetch
//! (`278-fetch-on-known-push`; GHD `background-fetcher.ts` waits for its
//! hourly schedule, so a push made elsewhere shows up to an hour late).
//! Branch › Push To ▸ and Fetch From ▸ reach the repository's other
//! remotes, the upstream left as it is (`1210-push-to-other-remote`).
//! A branch without an upstream that `push.default=current` pushes to the
//! same-named remote branch shows Push / Pull against that branch instead of
//! Publish (`1103-implicit-upstream-push-default`; GHD reads only the
//! configured upstream).
//! A Git LFS server that wants a login of its own gets the login dialog for
//! its host (`1104-lfs-server-authentication`).
//! A repository can sign in through git's credential helper instead of the
//! account (`1102-repository-credential-helper`; GHD
//! `useExternalCredentialHelper` covers hosts without an account only).
//! The push button's tooltip can say how much a push sends
//! (`1101-push-size-tooltip`).
//! Repository › Pull All Repositories fetches every repository and
//! fast-forwards the branches that are only behind
//! (`299-pull-all-repositories`).
//! The hourly background fetch of a GitHub repository can be skipped while
//! GitHub says nothing was pushed since the last one, with a real fetch at
//! least every six hours (`298-background-fetch-skips-unchanged`; GHD
//! fetches every hour).
//! The Newer Commits on Remote dialog can pull and push in one go
//! (`297-push-needs-pull-offers-pull`).
//! A running fetch, push or pull (until it merges) can be stopped from the
//! push/pull button (`295-cancel-network-operations`; GHD `push-pull-button.tsx`
//! only disables itself), and waking from sleep stops a background fetch
//! left hanging on a dead connection (`296-cancel-fetch-on-wake`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use crate::host::{AsyncCtx, Host};
use corvene_git::{AskpassEnv, RemoteFailure};
use corvene_models::{Account, AheadBehind, Remote, Tip};
use tracing::{debug, info, warn};

use crate::dispatcher::Dispatcher;
use crate::persistence::StoreExt;
use crate::state::{ErrorMessage, Popup, RetryAction};

/// What [`Dispatcher::tag_on_remote`] does with a tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TagOnRemote {
    Push,
    /// From the remote, then locally (flag `826`).
    Delete,
    /// From the remote only (`1219-tag-manager`).
    DeleteRemoteOnly,
}

/// GHD `app-store.ts` progress title after a fetch/pull/push
/// (`Refreshing ${__DARWIN__ ? 'Repository' : 'repository'}`).
const REFRESHING_REPOSITORY: &str = if cfg!(target_os = "macos") {
    "Refreshing Repository"
} else {
    "Refreshing repository"
};

/// GHD `Progress` for the push/pull button.
#[derive(Clone, Debug, PartialEq)]
pub struct PushPullProgress {
    pub kind: PushPullKind,
    pub title: String,
    pub description: Option<String>,
    /// 0..=1
    pub value: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushOutcome {
    /// git pushed.
    Pushed,
    /// git ran and failed (the error dialog is up).
    Failed,
    /// Nothing was pushed: no remote (Publish opened), a fork was offered,
    /// an unborn / detached tip, or another network operation is running.
    NotAttempted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushPullKind {
    Push,
    Pull,
    Fetch,
    Generic,
}

/// Corvene (`295-cancel-network-operations`, `296-cancel-fetch-on-wake`):
/// what stops the repository's running fetch, pull or push.
#[derive(Clone, Debug)]
pub struct NetworkCancel {
    pub token: corvene_git::CancelToken,
    pub kind: PushPullKind,
    /// A background fetch, the only kind waking from sleep stops.
    pub background: bool,
    /// When it started: a pull that went on to merge cannot be stopped.
    pub started: SystemTime,
}

/// Corvene (`1101-push-size-tooltip`): the push size worked out for one
/// branch tip and upstream tip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushSizeEstimate {
    pub head: String,
    pub upstream: String,
    /// `None` while it is being worked out (or when that failed).
    pub size: Option<corvene_git::PushSize>,
}

/// How long the push/pull button says "Cancelled" after a stop.
pub const CANCELLED_NOTE: Duration = Duration::from_secs(5);

/// Run `f` with `token` stopping the git commands it starts.
fn cancellable<T>(token: Option<&corvene_git::CancelToken>, f: impl FnOnce() -> T) -> T {
    match token {
        Some(token) => corvene_git::with_cancel_token(token, f),
        None => f(),
    }
}

/// Corvene (`271-persist-repository-indicators`): keep the indicators for
/// the next launch, so the repository list is not blank until the first
/// refresh. GHD keeps them in memory only (`RepositoryIndicatorUpdater`).
fn save_indicators(s: &crate::state::AppState) {
    if s.flags
        .bool(crate::flags::ids::PERSIST_REPOSITORY_INDICATORS)
        && let Err(err) = s.store.save_repository_indicators(&s.indicators)
    {
        warn!(?err, "could not save repository indicators");
    }
}

/// Sidebar indicators (`ILocalRepositoryState`); saved between launches
/// with `271-persist-repository-indicators`.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct RepoIndicator {
    pub ahead_behind: Option<AheadBehind>,
    pub changed_files: usize,
    /// The checked-out branch, for `214-repository-list-branch`.
    pub branch: Option<String>,
    /// `refs/stash` exists, for `270-repository-list-stash-icon`.
    pub has_stash: bool,
}

/// GHD `ForcePushBranchState`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForcePushState {
    NotAvailable,
    Available,
    Recommended,
}

pub(crate) const BACKGROUND_FETCH_INTERVAL: Duration = Duration::from_secs(60 * 60);
const BACKGROUND_FETCH_MINIMUM: Duration = Duration::from_secs(5 * 60);
/// `298-background-fetch-skips-unchanged`: a real background fetch runs at
/// least this often, whatever GitHub says.
const FORCED_FETCH_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const INDICATOR_REFRESH_INTERVAL: Duration = Duration::from_secs(15 * 60);
const INDICATOR_REFRESH_MINIMUM: Duration = Duration::from_secs(60);

thread_local! {
    /// When [`Dispatcher::refresh_indicators`] last started (main thread).
    static LAST_INDICATOR_REFRESH: std::cell::Cell<Option<Instant>> =
        const { std::cell::Cell::new(None) };
}

pub(crate) fn spawn_bg<T: Send + 'static>(
    cx: &mut dyn Host,
    work: impl FnOnce() -> T + Send + 'static,
    then: impl FnOnce(T, &mut dyn Host) + 'static,
) {
    let task = cx.background_executor().spawn(async move { work() });
    cx.spawn(async move |cx: &mut AsyncCtx| {
        let result = task.await;
        cx.update(|cx| then(result, cx));
    })
    .detach();
}

impl Dispatcher {
    // ---- helpers ----

    /// Settings › Advanced › Use Git Credential Manager: only for remotes that
    /// are not GitHub (GHD `useExternalCredentialHelper`). Also arms the
    /// stalled-transfer timeout of flag `network-stall-timeout` (0 = none).
    pub(crate) fn arm_credential_helper(remote_url: &str, cx: &dyn Host) {
        let s = Self::state(cx).read(cx);
        let host = host_of(remote_url);
        let github = host == "github.com" || s.accounts.iter().any(|a| a.host() == host);
        corvene_git::set_credential_helper(s.settings.use_external_credential_helper && !github);
        corvene_git::set_network_stall_timeout(
            u32::try_from(s.flags.number(crate::flags::ids::NETWORK_STALL_TIMEOUT)).unwrap_or(0),
        );
    }

    /// `GIT_ASKPASS` environment: one login per host from the signed-in
    /// accounts and the generic credentials the user saved.
    pub(crate) fn askpass_env(cx: &dyn Host) -> Option<AskpassEnv> {
        Self::askpass_env_with(None, None, cx)
    }

    /// [`Self::askpass_env`] without the signed-in accounts of `except`'s
    /// host, and with `preferred` (host, login) answering for its host when
    /// that host has several accounts (`527-multiple-accounts`).
    fn askpass_env_with(
        except: Option<&str>,
        preferred: Option<(&str, &str)>,
        cx: &dyn Host,
    ) -> Option<AskpassEnv> {
        let s = Self::state(cx).read(cx);
        let mut logins: Vec<String> = crate::accounts::askpass_pairs(
            s.accounts
                .iter()
                .filter(|a| except.is_none_or(|host| a.host() != host))
                .map(|a| (a.host(), a.login.clone(), a.login.clone())),
            preferred,
        );
        // flags 342-344: GitLab, Gitea and Bitbucket accounts
        logins.extend(
            s.host_askpass_logins(preferred)
                .into_iter()
                .filter(|pair| except.is_none_or(|host| !pair.starts_with(&format!("{host}=")))),
        );
        logins.extend(
            s.generic_logins
                .iter()
                .map(|(host, user)| format!("{host}={user}")),
        );
        let gh_cli = s.accounts.iter().any(|a| a.from_gh_cli);
        AskpassEnv::current_exe(logins.join(";")).map(|env| AskpassEnv { gh_cli, ..env })
    }

    /// Corvene (`1102-repository-credential-helper`): repository `id` signs
    /// in through git's credential helper (Repository Settings › Remote).
    pub(crate) fn uses_credential_helper(s: &crate::state::AppState, id: u64) -> bool {
        s.flags
            .bool(crate::flags::ids::REPOSITORY_CREDENTIAL_HELPER)
            && s.repository(id).is_some_and(|r| r.use_credential_helper)
    }

    /// [`Self::arm_credential_helper`] for a remote of repository `id`: a
    /// repository that signs in through the credential helper gets it, as a
    /// host without an account does (`1102-repository-credential-helper`).
    pub(crate) fn arm_credential_helper_for(id: u64, remote_url: &str, cx: &dyn Host) {
        Self::arm_credential_helper(remote_url, cx);
        if Self::uses_credential_helper(Self::state(cx).read(cx), id) {
            corvene_git::set_credential_helper(true);
        }
    }

    /// [`Self::askpass_env`] for a remote of repository `id`: a repository
    /// that signs in through the credential helper leaves the account of the
    /// remote's host out, so the helper (or a login saved in Corvene for
    /// that host) answers (`1102-repository-credential-helper`).
    ///
    /// The account the repository uses (`527-multiple-accounts`) answers
    /// for the remote's host.
    pub(crate) fn askpass_env_for(id: u64, remote_url: &str, cx: &dyn Host) -> Option<AskpassEnv> {
        let host = host_of(remote_url);
        let (helper, login) = {
            let s = Self::state(cx).read(cx);
            let login = s
                .repository(id)
                .and_then(|r| r.account.clone())
                .filter(|_| s.multiple_accounts());
            (Self::uses_credential_helper(s, id), login)
        };
        let preferred = login.as_deref().map(|login| (host.as_str(), login));
        if helper {
            Self::askpass_env_with(Some(&host), preferred, cx)
        } else {
            Self::askpass_env_with(None, preferred, cx)
        }
    }

    /// [`Self::askpass_env`] with `login`'s account answering for `url`'s
    /// host (`527-multiple-accounts`: a clone from that account).
    pub(crate) fn askpass_env_preferring(
        url: &str,
        login: &str,
        cx: &dyn Host,
    ) -> Option<AskpassEnv> {
        let host = host_of(url);
        Self::askpass_env_with(None, Some((host.as_str(), login)), cx)
    }

    /// [`Self::askpass_env_for`] the current remote of repository `id` (its
    /// GitHub repository's clone URL without one): for git commands that
    /// reach the repository's host outside a fetch or push (LFS locks,
    /// submodules, pull request refs).
    pub(crate) fn askpass_env_for_repository(id: u64, cx: &dyn Host) -> Option<AskpassEnv> {
        let url = {
            let s = Self::state(cx).read(cx);
            Self::current_remote_in(s, id).map(|r| r.url).or_else(|| {
                s.repository(id)
                    .and_then(|r| r.github.as_ref())
                    .map(|gh| gh.clone_url.clone())
            })
        };
        match url {
            Some(url) => Self::askpass_env_for(id, &url, cx),
            None => Self::askpass_env(cx),
        }
    }

    /// GHD `currentRemote`: the branch's upstream remote, else `origin`, else
    /// the first remote.
    pub fn current_remote(id: u64, cx: &dyn Host) -> Option<Remote> {
        Self::current_remote_in(Self::state(cx).read(cx), id)
    }

    pub fn current_remote_in(s: &crate::state::AppState, id: u64) -> Option<Remote> {
        let info = s.repo_states.get(&id)?.info.as_ref()?;
        let upstream_remote = info
            .current_branch()
            .and_then(|b| b.upstream_remote_name().map(str::to_string));
        upstream_remote
            .and_then(|name| info.remotes.iter().find(|r| r.name == name))
            .or_else(|| corvene_git::find_default_remote(&info.remotes))
            .cloned()
    }

    /// Flag `826`: delete a tag that may have been pushed - from `remote`
    /// first (`push --delete`, so a failure keeps the local tag), then
    /// locally; with `remote` `None` only locally.
    pub fn delete_pushed_tag(id: u64, tag: String, remote: Option<Remote>, cx: &mut dyn Host) {
        match remote {
            Some(remote) => Self::tag_on_remote(id, tag, remote, TagOnRemote::Delete, cx),
            None => Self::delete_tag(id, tag, cx),
        }
    }

    /// Corvene (`1219-tag-manager`): delete `tag` from `remote` only (the
    /// local tag stays), as [`Self::delete_pushed_tag`] does first.
    pub fn delete_remote_tag_only(id: u64, tag: String, remote: Remote, cx: &mut dyn Host) {
        Self::tag_on_remote(id, tag, remote, TagOnRemote::DeleteRemoteOnly, cx)
    }

    /// Corvene (`1219-tag-manager`): push `tag` alone to the current
    /// remote (`git push <remote> refs/tags/<tag>`); it leaves the tags to
    /// push.
    pub fn push_single_tag(id: u64, tag: String, cx: &mut dyn Host) {
        let Some(remote) = Self::current_remote(id, cx) else {
            Self::show_popup(Popup::PublishRepository { repo: id }, cx);
            return;
        };
        Self::tag_on_remote(id, tag, remote, TagOnRemote::Push, cx)
    }

    /// One tag pushed to or deleted from `remote`, with the push/pull
    /// button's progress.
    fn tag_on_remote(id: u64, tag: String, remote: Remote, op: TagOnRemote, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        if !Self::begin_network(id, cx) {
            return;
        }
        Self::arm_credential_helper_for(id, &remote.url, cx);
        let askpass = Self::askpass_env_for(id, &remote.url, cx);
        let title = match op {
            TagOnRemote::Push => format!("Pushing tag {tag} to {}", remote.name),
            _ => format!("Deleting tag {tag} from {}", remote.name),
        };
        Self::set_progress(
            id,
            Some(PushPullProgress {
                kind: PushPullKind::Push,
                title,
                description: None,
                value: 0.,
            }),
            cx,
        );
        let tag_for_task = tag.clone();
        Self::run_network(
            id,
            cx,
            move |_| match op {
                TagOnRemote::Push => corvene_git::push_tag(
                    git,
                    &workdir,
                    &remote.name,
                    &tag_for_task,
                    askpass.as_ref(),
                ),
                _ => corvene_git::delete_remote_tag(
                    git,
                    &workdir,
                    &remote.name,
                    &tag_for_task,
                    askpass.as_ref(),
                ),
            },
            move |result, cx| match (result, op) {
                (Ok(()), TagOnRemote::Delete) => Self::delete_tag(id, tag, cx),
                (Ok(()), TagOnRemote::Push) => {
                    Self::update_tags_to_push(id, cx, |tags| tags.retain(|t| *t != tag));
                    Self::tags_changed(id, cx);
                }
                (Ok(()), TagOnRemote::DeleteRemoteOnly) => Self::tags_changed(id, cx),
                (Err(err), TagOnRemote::Push) => Self::show_error("Could not push tag", &err, cx),
                (Err(err), _) => Self::show_error("Could not delete tag", &err, cx),
            },
        );
    }

    /// GHD `getCurrentBranchForcePushState`
    pub fn force_push_state(id: u64, cx: &dyn Host) -> ForcePushState {
        Self::force_push_state_in(Self::state(cx).read(cx), id)
    }

    pub fn force_push_state_in(s: &crate::state::AppState, id: u64) -> ForcePushState {
        let Some(rs) = s.repo_states.get(&id) else {
            return ForcePushState::NotAvailable;
        };
        let Some(ab) = rs.ahead_behind else {
            return ForcePushState::NotAvailable;
        };
        if ab.ahead == 0 || ab.behind == 0 {
            return ForcePushState::NotAvailable;
        }
        let recommended = rs
            .info
            .as_ref()
            .and_then(|i| i.current_branch())
            .is_some_and(|b| rs.force_push_branches.get(b.name_without_remote()) == b.tip.as_ref());
        // `260-force-push-after-outside-rewrite`: GHD recommends a force
        // push only after its own amend or rebase
        let rewritten_outside = rs.upstream_rewritten
            && s.flags
                .bool(crate::flags::ids::FORCE_PUSH_AFTER_OUTSIDE_REWRITE);
        if recommended || rewritten_outside {
            ForcePushState::Recommended
        } else {
            ForcePushState::Available
        }
    }

    fn set_progress(id: u64, progress: Option<PushPullProgress>, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            s.repo_state_mut(id).push_pull_progress = progress;
            cx.notify();
        });
    }

    /// GHD `withPushPullFetch`: one network operation at a time per repository.
    fn begin_network(id: u64, cx: &mut dyn Host) -> bool {
        Self::state(cx).update(cx, |s, cx| {
            let rs = s.repo_state_mut(id);
            if rs.push_pull_in_progress {
                return false;
            }
            rs.push_pull_in_progress = true;
            cx.notify();
            true
        })
    }

    /// `245-push-during-background-fetch`: a push, pull or fetch asked for
    /// while a background fetch runs waits for it (GHD disables the button
    /// and drops the request).
    fn behind_background_fetch(id: u64, cx: &dyn Host) -> bool {
        Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .is_some_and(|r| r.push_pull_in_progress && r.quiet_background_fetch)
    }

    /// Run `then` once the repository's network operation finished.
    fn after_network(id: u64, cx: &mut dyn Host, then: impl FnOnce(&mut dyn Host) + 'static) {
        cx.spawn(async move |cx: &mut AsyncCtx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                let busy = cx.update(|cx| {
                    Self::state(cx)
                        .read(cx)
                        .repo_states
                        .get(&id)
                        .is_some_and(|r| r.push_pull_in_progress)
                });
                if !busy {
                    break;
                }
            }
            cx.update(then);
        })
        .detach();
    }

    fn end_network(id: u64, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            let rs = s.repo_state_mut(id);
            rs.push_pull_in_progress = false;
            rs.quiet_background_fetch = false;
            rs.push_pull_progress = None;
            rs.network_cancel = None;
            cx.notify();
        });
    }

    /// The token that stops the network operation just begun, kept in the
    /// repository state while `295-cancel-network-operations` or
    /// `296-cancel-fetch-on-wake` is on (`None`: GHD, nothing stops it).
    fn network_cancel_token(
        id: u64,
        kind: PushPullKind,
        background: bool,
        cx: &mut dyn Host,
    ) -> Option<corvene_git::CancelToken> {
        let flags = &Self::state(cx).read(cx).flags;
        if !flags.bool(crate::flags::ids::CANCEL_NETWORK_OPERATIONS)
            && !flags.bool(crate::flags::ids::CANCEL_FETCH_ON_WAKE)
        {
            return None;
        }
        let token = corvene_git::CancelToken::new();
        let cancel = NetworkCancel {
            token: token.clone(),
            kind,
            background,
            started: SystemTime::now(),
        };
        Self::state(cx).update(cx, |s, _| {
            s.repo_state_mut(id).network_cancel = Some(cancel);
        });
        Some(token)
    }

    /// `295-cancel-network-operations`: the push/pull button offers Stop
    /// (a fetch, pull or push showing its progress, not yet stopped).
    pub fn network_cancellable(s: &crate::state::AppState, id: u64) -> bool {
        s.flags.bool(crate::flags::ids::CANCEL_NETWORK_OPERATIONS)
            && s.repo_states.get(&id).is_some_and(|rs| {
                !rs.quiet_background_fetch
                    && rs
                        .network_cancel
                        .as_ref()
                        .is_some_and(|c| !c.token.is_cancelled())
            })
    }

    /// The push/pull button's Stop (`295-cancel-network-operations`): stop
    /// the running fetch, push or pull. A pull that already merges or
    /// rebases is left to finish.
    pub fn cancel_network(id: u64, cx: &mut dyn Host) {
        let (cancel, workdir) = {
            let s = Self::state(cx).read(cx);
            if !Self::network_cancellable(s, id) {
                return;
            }
            (
                s.repo_states
                    .get(&id)
                    .and_then(|r| r.network_cancel.clone()),
                s.repository(id).map(|r| r.path.clone()),
            )
        };
        let Some(cancel) = cancel else { return };
        if cancel.kind == PushPullKind::Pull
            && workdir.is_none_or(|dir| corvene_git::pull_merge_started(&dir, cancel.started))
        {
            Self::show_error(
                "Could not stop the pull",
                "The pull has already started merging or rebasing, which must not be \
                 interrupted. It will finish shortly.",
                cx,
            );
            return;
        }
        info!(id, kind = ?cancel.kind, "stopping network operation");
        cancel.token.cancel();
        Self::state(cx).update(cx, |s, cx| {
            s.repo_state_mut(id).network_cancelled_at = Some(Instant::now());
            cx.notify();
        });
        // the button's "Cancelled" goes again
        cx.spawn(async move |cx: &mut AsyncCtx| {
            cx.background_executor().timer(CANCELLED_NOTE).await;
            cx.update(|cx| Self::state(cx).update(cx, |_, cx| cx.notify()));
        })
        .detach();
    }

    /// `296-cancel-fetch-on-wake`: the system woke from sleep. A background
    /// fetch still running then most likely waits on a connection that died
    /// during sleep (SSH and stalled HTTPS never time out): stop it, and the
    /// next background round fetches again. Fetches the user started keep
    /// running and fail as they would.
    pub fn system_woke(cx: &mut dyn Host) {
        let s = Self::state(cx).read(cx);
        if !s.flags.bool(crate::flags::ids::CANCEL_FETCH_ON_WAKE) {
            return;
        }
        for (id, rs) in &s.repo_states {
            if let Some(cancel) = rs.network_cancel.as_ref().filter(|c| c.background) {
                info!(id, "stopping a background fetch after wake");
                cancel.token.cancel();
            }
        }
    }

    /// Run a network operation on a background thread, mirroring progress
    /// into the push/pull button, then handle the outcome.
    fn run_network<T: Send + 'static>(
        id: u64,
        cx: &mut dyn Host,
        work: impl FnOnce(&mut dyn FnMut(PushPullProgress)) -> T + Send + 'static,
        then: impl FnOnce(T, &mut dyn Host) + 'static,
    ) {
        let (tx, rx) = async_channel::unbounded::<PushPullProgress>();
        let task = cx.background_executor().spawn(async move {
            let mut report = |p: PushPullProgress| {
                let _ = tx.send_blocking(p);
            };
            work(&mut report)
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            while let Ok(progress) = rx.recv().await {
                cx.update(|cx| Self::set_progress(id, Some(progress), cx));
            }
            let result = task.await;
            cx.update(|cx| {
                Self::end_network(id, cx);
                then(result, cx);
            });
        })
        .detach();
    }

    /// The remote-specific error dialogs (`pushNeedsPullHandler`,
    /// `gitAuthenticationErrorHandler`); everything else is a plain error.
    fn handle_remote_error(
        id: u64,
        title: &str,
        err: corvene_git::GitError,
        remote_url: String,
        retry: RetryAction,
        background: bool,
        cx: &mut dyn Host,
    ) {
        // `295-cancel-network-operations` / `296-cancel-fetch-on-wake`:
        // stopped on purpose
        if matches!(err, corvene_git::GitError::Cancelled(_)) {
            info!(id, "remote operation stopped");
            return;
        }
        // `528-proxy-credentials`: the proxy wants a username and password
        // (background fetches ask too, until the prompt is cancelled)
        if corvene_git::remote_failure(&err) == RemoteFailure::ProxyAuthenticationRequired
            && Self::state(cx)
                .read(cx)
                .flags
                .bool(crate::flags::ids::PROXY_CREDENTIALS)
            && let Some((git, workdir)) = Self::repo_context(id, cx)
        {
            warn!(id, %err, "the proxy wants credentials");
            Self::git_proxy_auth_required(
                git,
                Some(workdir),
                remote_url,
                crate::proxy::ProxyRetry::Remote {
                    repo: id,
                    action: retry,
                },
                !background,
                cx,
            );
            return;
        }
        if background {
            warn!(id, %err, "background remote operation failed");
            return;
        }
        let stderr = match &err {
            corvene_git::GitError::Failed { stderr, .. } => stderr.clone(),
            _ => String::new(),
        };
        let github = Self::state(cx)
            .read(cx)
            .repository(id)
            .and_then(|r| r.github.clone());
        // `localChangesOverwrittenHandler`: a pull that would clobber
        // uncommitted changes offers to stash them and pull again
        if matches!(retry, RetryAction::Pull)
            && err.known().is_some_and(|k| k.is_blocked_by_local_changes())
        {
            Self::show_popup(
                Popup::LocalChangesOverwritten {
                    repo: id,
                    retry,
                    files: corvene_git::files_that_would_be_overwritten(&stderr),
                },
                cx,
            );
            return;
        }
        // Corvene (`1104-lfs-server-authentication`): git-lfs could not sign
        // in to its server; the login is asked for that server's host (GHD
        // shows git-lfs' output, or a login dialog for the remote's host)
        if corvene_git::is_lfs_auth_failure(&stderr)
            && Self::state(cx)
                .read(cx)
                .flags
                .bool(crate::flags::ids::LFS_SERVER_AUTHENTICATION)
        {
            return Self::ask_lfs_login(id, title.to_string(), err, &stderr, remote_url, retry, cx);
        }
        match corvene_git::remote_failure(&err) {
            RemoteFailure::PushNotFastForward => {
                Self::show_popup(Popup::PushNeedsPull { repo: id }, cx);
            }
            // `secretScanningPushProtectionErrorHandler`
            RemoteFailure::PushWithSecretDetected => {
                let secrets = crate::push_errors::secret_scan_results(
                    &crate::push_errors::remote_message(&stderr),
                );
                if secrets.is_empty() {
                    Self::show_error(title, &err, cx);
                } else {
                    Self::show_popup(
                        Popup::PushProtectionError {
                            repo: id,
                            secrets,
                            bypassed: Vec::new(),
                        },
                        cx,
                    );
                }
            }
            // `refusedWorkflowUpdate`
            RemoteFailure::MissingWorkflowScope if github.is_some() => {
                match crate::push_errors::rejected_workflow_path(&stderr) {
                    Some(rejected_path) => Self::show_popup(
                        Popup::PushRejectedDueToMissingWorkflowScope {
                            repo: id,
                            rejected_path,
                        },
                        cx,
                    ),
                    None => Self::show_error(title, &err, cx),
                }
            }
            // `samlReauthRequired`
            RemoteFailure::SamlReauthRequired if github.is_some() => {
                let organization = crate::push_errors::saml_organization(
                    &crate::push_errors::remote_message(&stderr),
                );
                match (organization, github) {
                    (Some(organization), Some(gh)) => Self::show_popup(
                        Popup::SAMLReauthRequired {
                            repo: id,
                            organization,
                            endpoint: gh.endpoint,
                            retry: Some(retry),
                        },
                        cx,
                    ),
                    _ => Self::show_error(title, &err, cx),
                }
            }
            // `insufficientGitHubRepoPermissions`: offer a fork. With
            // `303-fork-before-push` known read-only repositories never get
            // here (see `push_then`); this covers repositories whose
            // permissions were never fetched, and every read-only one when
            // the flag is off (GHD's path).
            RemoteFailure::PermissionDenied
                if matches!(retry, RetryAction::Push { .. })
                    && github.as_ref().is_some_and(|gh| {
                        let s = Self::state(cx).read(cx);
                        (gh.permissions.is_none() || !gh.has_write_permission())
                            && s.account_for(&gh.endpoint).is_some()
                            && !Self::fork_offer_blocked(s, gh)
                    }) =>
            {
                Self::show_create_fork_dialog(id, cx);
            }
            RemoteFailure::AuthenticationFailed => {
                let host = host_of(&remote_url);
                let username = {
                    let s = Self::state(cx).read(cx);
                    s.accounts
                        .iter()
                        .find(|a| a.host() == host)
                        .map(|a| a.login.clone())
                        .or_else(|| s.generic_logins.get(&host).cloned())
                };
                Self::show_popup(
                    Popup::GenericGitAuthentication {
                        repo: id,
                        remote_url,
                        host,
                        username,
                        retry,
                    },
                    cx,
                );
            }
            failure => {
                // `255-plain-language-remote-errors`: say what went wrong
                // before git's message
                let s = Self::state(cx).read(cx);
                let plain = s
                    .flags
                    .bool(crate::flags::ids::PLAIN_LANGUAGE_REMOTE_ERRORS)
                    .then(|| {
                        crate::push_errors::plain_remote_error(&err).or_else(|| {
                            // a non-origin remote (a fork's parent) that is gone
                            let remote = s
                                .repo_states
                                .get(&id)?
                                .info
                                .as_ref()?
                                .remotes
                                .iter()
                                .find(|r| r.url == remote_url && r.name != "origin")?;
                            (failure == RemoteFailure::RepositoryNotFound).then(|| {
                                crate::push_errors::plain_missing_remote_repository(
                                    &remote.name,
                                    &remote.url,
                                    &err,
                                )
                            })
                        })
                    })
                    .flatten();
                Self::show_error(title, ErrorMessage::explained(&err, plain), cx)
            }
        }
    }

    /// `1104-lfs-server-authentication`: the Authentication Failed dialog for
    /// the Git LFS server a remote operation could not sign in to, named by
    /// git-lfs' output or else by `git lfs env`; the plain error when it
    /// cannot be told.
    fn ask_lfs_login(
        id: u64,
        title: String,
        err: corvene_git::GitError,
        stderr: &str,
        remote_url: String,
        retry: RetryAction,
        cx: &mut dyn Host,
    ) {
        let show = move |lfs_url: Option<String>, cx: &mut dyn Host| {
            let host = lfs_url.as_deref().map(host_of).unwrap_or_default();
            let Some(lfs_url) = lfs_url.filter(|_| !host.is_empty()) else {
                return Self::show_error(title.as_str(), &err, cx);
            };
            let username = Self::state(cx).read(cx).generic_logins.get(&host).cloned();
            Self::show_popup(
                Popup::GenericGitAuthentication {
                    repo: id,
                    remote_url: lfs_url,
                    host,
                    username,
                    retry,
                },
                cx,
            );
        };
        if let Some(url) = corvene_git::lfs_auth_failure_url(stderr) {
            return show(Some(url), cx);
        }
        let remote = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|r| r.info.as_ref())
            .and_then(|i| i.remotes.iter().find(|r| r.url == remote_url))
            .map(|r| r.name.clone())
            .unwrap_or_else(|| "origin".to_string());
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return show(None, cx);
        };
        spawn_bg(
            cx,
            move || corvene_git::lfs::lfs_endpoint(git, &workdir, &remote),
            show,
        );
    }

    // ---- fetch ----

    /// `252-prune-stale-refs-and-retry`: when a fetch or pull failed because
    /// a stale remote-tracking ref blocks a new one, run `git remote prune`
    /// and say to try once more (`retry` is cleared). GHD shows the error.
    fn prune_before_retry<T>(
        retry: &mut bool,
        result: &Result<T, corvene_git::GitError>,
        git: &std::sync::Arc<corvene_git::GitBinary>,
        workdir: &std::path::Path,
        remote: &str,
        askpass: Option<&AskpassEnv>,
    ) -> bool {
        if !std::mem::take(retry)
            || !result
                .as_ref()
                .is_err_and(corvene_git::is_stale_remote_ref_failure)
        {
            return false;
        }
        info!(remote, "stale remote-tracking ref; pruning and retrying");
        corvene_git::prune_remote(git.clone(), workdir, remote, askpass).is_ok()
    }

    /// The Corvene additions to a fetch of repository `id`.
    fn fetch_options(s: &crate::state::AppState, id: u64) -> corvene_git::FetchOptions {
        corvene_git::FetchOptions {
            // `248-fetch-prune-tags`: drop tags deleted on the remote, but
            // never while tags created here wait to be pushed (they would
            // be lost)
            prune_tags: s.flags.bool(crate::flags::ids::FETCH_PRUNE_TAGS)
                && s.repository(id).is_some_and(|r| r.tags_to_push.is_empty()),
            // `1120-fetch-updates-moved-tags`: same guard, a tag created
            // here and not pushed yet must not be overwritten
            update_moved_tags: s.flags.bool(crate::flags::ids::FETCH_UPDATES_MOVED_TAGS)
                && s.repository(id).is_some_and(|r| r.tags_to_push.is_empty()),
            // `249-fetch-writes-commit-graph`
            write_commit_graph: s.flags.bool(crate::flags::ids::FETCH_WRITES_COMMIT_GRAPH),
            // `250-sync-skips-submodules`
            skip_submodules: s.flags.bool(crate::flags::ids::SYNC_SKIPS_SUBMODULES),
            // `875-explain-bad-config`
            retry_bad_gitmodules: s.flags.bool(crate::flags::ids::EXPLAIN_BAD_CONFIG),
        }
    }

    /// `_fetch(FetchType::UserInitiatedTask | BackgroundTask)`
    pub fn fetch(id: u64, background: bool, cx: &mut dyn Host) {
        Self::fetch_remote_then(id, None, background, |_, _| {}, cx);
    }

    /// `fetch` from `remote` (default: the current branch's remote), then
    /// `then(fetched)` once it finished or did not start.
    pub fn fetch_remote_then(
        id: u64,
        remote: Option<&str>,
        background: bool,
        then: impl FnOnce(bool, &mut dyn Host) + 'static,
        cx: &mut dyn Host,
    ) {
        if !background && Self::behind_background_fetch(id, cx) {
            let remote = remote.map(str::to_string);
            return Self::after_network(id, cx, move |cx| {
                Self::fetch_remote_then(id, remote.as_deref(), false, then, cx)
            });
        }
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return then(false, cx);
        };
        let remote = match remote {
            Some(name) => Self::state(cx)
                .read(cx)
                .repo_states
                .get(&id)
                .and_then(|r| r.info.as_ref())
                .and_then(|i| i.remotes.iter().find(|r| r.name == name))
                .cloned(),
            None => Self::current_remote(id, cx),
        };
        let Some(remote) = remote else {
            return then(false, cx);
        };
        if !Self::begin_network(id, cx) {
            return then(false, cx);
        }
        // `245-push-during-background-fetch`: the background fetch leaves the
        // push/pull button alone
        let quiet = background
            && Self::state(cx)
                .read(cx)
                .flags
                .bool(crate::flags::ids::PUSH_DURING_BACKGROUND_FETCH);
        Self::arm_credential_helper_for(id, &remote.url, cx);
        let askpass = Self::askpass_env_for(id, &remote.url, cx).map(|env| AskpassEnv {
            no_ui: background,
            ..env
        });
        // `282-fast-forward-skips-worktree-branches`
        let skip_worktree_branches = Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::FAST_FORWARD_SKIPS_WORKTREE_BRANCHES);
        let title = format!("Fetching {}", remote.name);
        if quiet {
            Self::state(cx).update(cx, |s, _| {
                s.repo_state_mut(id).quiet_background_fetch = true;
            });
        } else {
            Self::set_progress(
                id,
                Some(PushPullProgress {
                    kind: PushPullKind::Fetch,
                    title: title.clone(),
                    description: None,
                    value: 0.,
                }),
                cx,
            );
        }
        let remote_name = remote.name.clone();
        let remote_url = remote.url.clone();
        let options = Self::fetch_options(Self::state(cx).read(cx), id);
        let prune_retry = Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::PRUNE_STALE_REFS_AND_RETRY);
        let fast_forward_current = background
            && Self::state(cx)
                .read(cx)
                .flags
                .bool(crate::flags::ids::BACKGROUND_FETCH_FAST_FORWARDS);
        let cancel = Self::network_cancel_token(id, PushPullKind::Fetch, background, cx);
        Self::run_network(
            id,
            cx,
            move |report| {
                cancellable(cancel.as_ref(), || {
                    let mut report = |progress| {
                        if !quiet {
                            report(progress)
                        }
                    };
                    let mut retry = prune_retry;
                    let result = loop {
                        let result = corvene_git::fetch_with(
                            git.clone(),
                            &workdir,
                            &remote_name,
                            options,
                            askpass.as_ref(),
                            &mut |value, text| {
                                report(PushPullProgress {
                                    kind: PushPullKind::Fetch,
                                    title: title.clone(),
                                    description: Some(text),
                                    value: value * 0.9,
                                })
                            },
                        );
                        if !Self::prune_before_retry(
                            &mut retry,
                            &result,
                            &git,
                            &workdir,
                            &remote_name,
                            askpass.as_ref(),
                        ) {
                            break result;
                        }
                    };
                    if result.is_ok() {
                        report(PushPullProgress {
                            kind: PushPullKind::Generic,
                            title: REFRESHING_REPOSITORY.into(),
                            description: Some("Fast-forwarding branches".into()),
                            value: 0.9,
                        });
                        let _ = corvene_git::fast_forward_branches_with(
                            git.clone(),
                            &workdir,
                            skip_worktree_branches,
                        );
                        // `246-background-fetch-fast-forwards`: a clean branch
                        // that is only behind catches up (GHD leaves it for Pull)
                        if fast_forward_current {
                            match corvene_git::fast_forward_if_only_behind(git, &workdir) {
                                Ok(true) => info!(id, "fast-forwarded after background fetch"),
                                Ok(false) => {}
                                Err(err) => warn!(id, %err, "fast-forward after fetch failed"),
                            }
                        }
                    }
                    result
                })
            },
            move |result, cx| {
                let fetched = result.is_ok();
                // `356-stacked-pull-requests`: a lower pull request may have
                // merged (the pull request cache refreshes at most every two minutes)
                if fetched {
                    Self::check_stack_base(id, cx);
                }
                Self::note_remote_not_found(id, result.as_ref().err(), cx);
                if let Err(err) = result {
                    Self::handle_remote_error(
                        id,
                        "Could not fetch",
                        err,
                        remote_url,
                        RetryAction::Fetch,
                        background,
                        cx,
                    );
                }
                Self::refresh_repository(id, cx);
                then(fetched, cx);
            },
        );
    }

    /// Corvene (`288-dead-remote-indicator`): a fetch that found no remote
    /// repository marks the repository; one that worked clears the mark.
    fn note_remote_not_found(id: u64, err: Option<&corvene_git::GitError>, cx: &mut dyn Host) {
        let not_found = err.is_some_and(corvene_git::remote_repository_missing);
        Self::state(cx).update(cx, |s, cx| {
            let on = s.flags.bool(crate::flags::ids::DEAD_REMOTE_INDICATOR);
            let mark = on && not_found;
            if (err.is_none() || mark) && s.repo_state_mut(id).remote_not_found != mark {
                s.repo_state_mut(id).remote_not_found = mark;
                cx.notify();
            }
        });
    }

    /// Repository › Fetch All Repositories (`247-fetch-all-repositories`;
    /// GHD has none): fetch every listed repository with a remote, one at a
    /// time on a background thread, skipping those with a network operation
    /// running; failures are collected into one error.
    pub fn fetch_all_repositories(cx: &mut dyn Host) {
        if Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::FETCH_ALL_REPOSITORIES)
        {
            Self::sync_all_repositories(false, cx);
        }
    }

    /// Repository › Pull All Repositories (`299-pull-all-repositories`; GHD
    /// has none): Fetch All Repositories, then each checked-out branch that
    /// is only behind its upstream, in a clean working directory, is
    /// fast-forwarded. Nothing is merged or rebased, so nothing can conflict;
    /// the repositories left as they were are listed in one dialog.
    pub fn pull_all_repositories(cx: &mut dyn Host) {
        if Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::PULL_ALL_REPOSITORIES)
        {
            Self::sync_all_repositories(true, cx);
        }
    }

    /// Fetch (and with `pull` fast-forward) every listed repository.
    fn sync_all_repositories(pull: bool, cx: &mut dyn Host) {
        // one Fetch / Pull All at a time
        static RUNNING: AtomicBool = AtomicBool::new(false);
        let (git, repos, busy, use_helper, github_hosts, selected) = {
            let s = Self::state(cx).read(cx);
            let Some(git) = s.git.clone() else { return };
            let use_helper = s.settings.use_external_credential_helper;
            let in_progress = |id: u64| {
                s.repo_states
                    .get(&id)
                    .is_some_and(|rs| rs.push_pull_in_progress)
            };
            let listed = s.repositories.iter().filter(|r| !r.missing);
            let busy: Vec<String> = listed
                .clone()
                .filter(|r| in_progress(r.id))
                .map(|r| r.name())
                .collect();
            let repos: Vec<_> = listed
                .filter(|r| !in_progress(r.id))
                .map(|r| {
                    (
                        r.id,
                        r.name(),
                        r.path.clone(),
                        Self::fetch_options(s, r.id),
                        // `1102-repository-credential-helper`
                        Self::uses_credential_helper(s, r.id),
                        // `527-multiple-accounts`
                        r.account.clone().filter(|_| s.multiple_accounts()),
                    )
                })
                .collect();
            let github_hosts: Vec<String> = std::iter::once("github.com".to_string())
                .chain(s.accounts.iter().map(|a| a.host()))
                .collect();
            (
                git,
                repos,
                busy,
                use_helper,
                github_hosts,
                s.visible_repositories(),
            )
        };
        if RUNNING.swap(true, Ordering::SeqCst) {
            return;
        }
        let askpass = Self::askpass_env(cx);
        // `1102-repository-credential-helper`: the logins without the
        // accounts, for the repositories that sign in through the helper
        let accounts: Vec<String> = Self::state(cx)
            .read(cx)
            .accounts
            .iter()
            .map(|a| format!("{}={}", a.host(), a.login))
            .collect();
        // `282-fast-forward-skips-worktree-branches`
        let skip_worktree_branches = Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::FAST_FORWARD_SKIPS_WORKTREE_BRANCHES);
        spawn_bg(
            cx,
            move || {
                let mut failures = Vec::new();
                // `299-pull-all-repositories`: the repositories left alone
                let mut summary = PullAllSummary::default();
                // `288-dead-remote-indicator`: (id, fetch error or `None`)
                let mut outcomes = Vec::new();
                for (id, name, path, options, own_helper, login) in repos {
                    let Ok(info) = corvene_git::open_repository(&path) else {
                        continue;
                    };
                    let upstream_remote = info
                        .current_branch()
                        .and_then(|b| b.upstream_remote_name().map(str::to_string));
                    let Some(remote) = upstream_remote
                        .and_then(|n| info.remotes.iter().find(|r| r.name == n))
                        .or_else(|| corvene_git::find_default_remote(&info.remotes))
                        .cloned()
                    else {
                        continue;
                    };
                    let host = host_of(&remote.url);
                    corvene_git::set_credential_helper(
                        own_helper || (use_helper && !github_hosts.contains(&host)),
                    );
                    let own_askpass = askpass.as_ref().and_then(|a| {
                        if own_helper {
                            Some(without_accounts_of(a, &accounts, &host))
                        } else {
                            login
                                .as_deref()
                                .map(|login| with_account_of(a, &accounts, &host, login))
                        }
                    });
                    match corvene_git::fetch_with(
                        git.clone(),
                        &info.workdir,
                        &remote.name,
                        options,
                        own_askpass.as_ref().or(askpass.as_ref()),
                        &mut |_, _| {},
                    ) {
                        Ok(()) => {
                            let _ = corvene_git::fast_forward_branches_with(
                                git.clone(),
                                &info.workdir,
                                skip_worktree_branches,
                            );
                            outcomes.push((id, None));
                            if pull {
                                summary.add(
                                    name,
                                    corvene_git::fast_forward_outcome(git.clone(), &info.workdir),
                                );
                            }
                        }
                        Err(err) => {
                            failures.push(format!("{name}: {err}"));
                            outcomes.push((id, Some(err)));
                        }
                    }
                }
                (failures, summary, outcomes)
            },
            move |(failures, mut summary, outcomes), cx| {
                RUNNING.store(false, Ordering::SeqCst);
                for (id, err) in outcomes {
                    Self::note_remote_not_found(id, err.as_ref(), cx);
                }
                if pull {
                    summary.busy = busy;
                    summary.failed.extend(failures);
                    if let Some(message) = summary.message() {
                        Self::show_error("Some repositories were not pulled", message, cx);
                    }
                } else if !failures.is_empty() {
                    Self::show_error(
                        "Could not fetch all repositories",
                        failures.join("\n\n"),
                        cx,
                    );
                }
                for id in selected {
                    Self::refresh_repository(id, cx);
                }
                Self::refresh_indicators(cx);
            },
        );
    }

    // ---- pull ----

    /// `_pull`
    pub fn pull(id: u64, cx: &mut dyn Host) {
        Self::pull_then(id, |_, _| {}, cx);
    }

    /// The Newer Commits on Remote dialog's "Pull and Push"
    /// (`297-push-needs-pull-offers-pull`; GHD `push-needs-pull-warning.tsx`
    /// offers Fetch only): pull, then push when the pull went through without
    /// conflicts. A conflicted pull ends in the usual conflicts flow and
    /// pushes nothing.
    pub fn pull_and_push(id: u64, cx: &mut dyn Host) {
        if !Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::PUSH_NEEDS_PULL_OFFERS_PULL)
        {
            return;
        }
        Self::pull_then(
            id,
            move |pulled, cx| {
                if pulled {
                    Self::push(id, false, None, cx);
                }
            },
            cx,
        );
    }

    /// `pull`, then `then(pulled)` once it finished (or did not start);
    /// `pulled` is false after any error, conflicts included.
    pub fn pull_then(id: u64, then: impl FnOnce(bool, &mut dyn Host) + 'static, cx: &mut dyn Host) {
        if Self::behind_background_fetch(id, cx) {
            return Self::after_network(id, cx, move |cx| Self::pull_then(id, then, cx));
        }
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return then(false, cx);
        };
        let Some(remote) = Self::current_remote(id, cx) else {
            Self::show_error("Could not pull", "The repository has no remotes.", cx);
            return then(false, cx);
        };
        let tip = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|r| r.info.as_ref())
            .map(|i| i.tip.clone());
        match tip {
            Some(Tip::Unborn { .. }) => {
                Self::show_error("Could not pull", "The current branch is unborn.", cx);
                return then(false, cx);
            }
            Some(Tip::Detached { .. }) => {
                Self::show_error(
                    "Could not pull",
                    "The current repository is in a detached HEAD state.",
                    cx,
                );
                return then(false, cx);
            }
            _ => {}
        }
        if !Self::begin_network(id, cx) {
            return then(false, cx);
        }
        Self::arm_credential_helper_for(id, &remote.url, cx);
        let askpass = Self::askpass_env_for(id, &remote.url, cx);
        // `282-fast-forward-skips-worktree-branches`
        let skip_worktree_branches = Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::FAST_FORWARD_SKIPS_WORKTREE_BRANCHES);
        let title = format!("Pulling {}", remote.name);
        let keep_remote_head = Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::REMOTE_HEAD_ONCE);
        let skip_submodules = Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::SYNC_SKIPS_SUBMODULES);
        let prune_retry = Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::PRUNE_STALE_REFS_AND_RETRY);
        Self::set_progress(
            id,
            Some(PushPullProgress {
                kind: PushPullKind::Pull,
                title: title.clone(),
                description: None,
                value: 0.,
            }),
            cx,
        );
        let remote_name = remote.name.clone();
        let remote_url = remote.url.clone();
        // `1103-implicit-upstream-push-default`: the pull records the
        // implicit upstream first (`git pull <remote>` needs one), as a push
        // would with `--set-upstream`
        let record_upstream = {
            let s = Self::state(cx).read(cx);
            let branch = s
                .repo_states
                .get(&id)
                .and_then(|r| r.info.as_ref())
                .and_then(|i| i.current_branch())
                .filter(|b| b.upstream.is_none())
                .map(|b| b.name.clone());
            branch.zip(Self::implicit_upstream_in(s, id).map(|(name, _)| name))
        };
        let cancel = Self::network_cancel_token(id, PushPullKind::Pull, false, cx);
        // GHD `pullRepo(…, { onHookFailure })`
        let hooks = crate::hooks::hook_ui(id, false, Some(format!("Pull {}", remote.name)), cx);
        let hook_callbacks = hooks.callbacks.clone();
        Self::run_network(
            id,
            cx,
            move |report| {
                cancellable(cancel.as_ref(), || {
                    if let Some((branch, upstream)) = &record_upstream
                        && let Err(err) =
                            corvene_git::set_upstream(git.clone(), &workdir, branch, upstream)
                    {
                        return (Err(err), None);
                    }
                    let mut retry = prune_retry;
                    let result = loop {
                        let result =
                            corvene_git::hooks::with_hook_callbacks(&hook_callbacks, || {
                                corvene_git::pull(
                                    git.clone(),
                                    &workdir,
                                    &remote_name,
                                    skip_submodules,
                                    askpass.as_ref(),
                                    &mut |value, text| {
                                        report(PushPullProgress {
                                            kind: PushPullKind::Pull,
                                            title: title.clone(),
                                            description: Some(text),
                                            value: value * 0.6,
                                        })
                                    },
                                )
                            });
                        if !Self::prune_before_retry(
                            &mut retry,
                            &result,
                            &git,
                            &workdir,
                            &remote_name,
                            askpass.as_ref(),
                        ) {
                            break result;
                        }
                    };
                    drop(hook_callbacks);
                    // `251-remote-head-once`: `set-head -a` asks the server for
                    // every ref, which takes minutes on huge repositories; skip
                    // it while the remote's HEAD already resolves
                    if result.is_ok()
                        && !(keep_remote_head
                            && corvene_git::remote_head_resolves(
                                git.clone(),
                                &workdir,
                                &remote_name,
                            ))
                    {
                        let _ = corvene_git::update_remote_head(
                            git.clone(),
                            &workdir,
                            &remote_name,
                            askpass.as_ref(),
                        );
                    }
                    if result.is_ok() {
                        report(PushPullProgress {
                            kind: PushPullKind::Generic,
                            title: REFRESHING_REPOSITORY.into(),
                            description: Some("Fast-forwarding branches".into()),
                            value: 0.9,
                        });
                        let _ = corvene_git::fast_forward_branches_with(
                            git.clone(),
                            &workdir,
                            skip_worktree_branches,
                        );
                    }
                    let status = corvene_git::get_status(git, &workdir).ok();
                    (result, status)
                })
            },
            move |(result, status), cx| {
                let pulled = result.is_ok();
                if let Some(mut status) = status {
                    status.sort_files();
                    Self::state(cx).update(cx, |s, cx| {
                        let rs = s.repo_state_mut(id);
                        rs.conflict_state =
                            crate::mco::derive_conflict_state(&status, rs.conflict_state.as_ref());
                        rs.status = Some(std::sync::Arc::new(status));
                        cx.notify();
                    });
                }
                if let Err(err) = result {
                    // merge / rebase conflicts from a pull show the conflicts
                    // dialog through the refresh (`mergeConflictHandler`)
                    let conflicted = Self::state(cx)
                        .read(cx)
                        .repo_states
                        .get(&id)
                        .is_some_and(|r| r.conflict_state.is_some());
                    // aborted in the HookFailed dialog: no error
                    if !conflicted && !hooks.aborted() {
                        Self::handle_remote_error(
                            id,
                            "Could not pull",
                            err,
                            remote_url,
                            RetryAction::Pull,
                            false,
                            cx,
                        );
                    }
                }
                Self::refresh_repository(id, cx);
                // a pull that left conflicts failed (`pulled` is false then)
                then(pulled, cx);
            },
        );
    }

    // ---- update a branch from its upstream ----

    /// The branch list's "Update from <upstream>" (`857-update-branch-from-upstream`;
    /// GHD has none): fast-forward a local branch that is not checked out.
    pub fn update_branch_from_upstream(id: u64, name: String, cx: &mut dyn Host) {
        let target = {
            let s = Self::state(cx).read(cx);
            if !s.flags.bool(crate::flags::ids::UPDATE_BRANCH_FROM_UPSTREAM) {
                return;
            }
            let info = s.repo_states.get(&id).and_then(|r| r.info.as_ref());
            info.and_then(|info| {
                let branch = info
                    .branches
                    .iter()
                    .find(|b| b.name == name && b.kind == corvene_models::BranchKind::Local)?;
                if info.current_branch().is_some_and(|c| c.name == name) {
                    return None;
                }
                let remote = branch.upstream_remote_name()?;
                let remote_branch = branch
                    .upstream_short()?
                    .strip_prefix(remote)?
                    .strip_prefix('/')?
                    .to_string();
                let url = info.remotes.iter().find(|r| r.name == remote)?.url.clone();
                Some((
                    remote.to_string(),
                    remote_branch,
                    url,
                    branch.upstream_short()?.to_string(),
                ))
            })
        };
        let Some((remote, remote_branch, remote_url, upstream)) = target else {
            return;
        };
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        if !Self::begin_network(id, cx) {
            return;
        }
        Self::arm_credential_helper_for(id, &remote_url, cx);
        let askpass = Self::askpass_env_for(id, &remote_url, cx);
        Self::set_progress(
            id,
            Some(PushPullProgress {
                kind: PushPullKind::Fetch,
                title: format!("Updating {name} from {upstream}"),
                description: None,
                value: 0.,
            }),
            cx,
        );
        let local = name.clone();
        Self::run_network(
            id,
            cx,
            move |_| {
                corvene_git::fast_forward_branch_from_remote(
                    git,
                    &workdir,
                    &remote,
                    &remote_branch,
                    &local,
                    askpass.as_ref(),
                )
            },
            move |result, cx| {
                if let Err(err) = result {
                    let title = "Could not update branch";
                    if corvene_git::remote_failure(&err) == RemoteFailure::PushNotFastForward {
                        Self::show_error(
                            title,
                            format!(
                                "{name} has commits that are not on {upstream}, so it cannot be \
                                 fast-forwarded. Check it out and pull instead."
                            ),
                            cx,
                        );
                    } else {
                        Self::handle_remote_error(
                            id,
                            title,
                            err,
                            remote_url,
                            RetryAction::Fetch,
                            false,
                            cx,
                        );
                    }
                }
                Self::refresh_repository(id, cx);
            },
        );
    }

    // ---- push ----

    /// `_push` (+ `performPush`): publish the branch when it has no upstream.
    pub fn push(id: u64, force_with_lease: bool, branch: Option<String>, cx: &mut dyn Host) {
        Self::push_then(id, force_with_lease, branch, |_, _| {}, cx);
    }

    /// `push`, then `then(outcome)` once it finished (or did not start).
    pub fn push_then(
        id: u64,
        force_with_lease: bool,
        branch: Option<String>,
        then: impl FnOnce(PushOutcome, &mut dyn Host) + 'static,
        cx: &mut dyn Host,
    ) {
        Self::push_inner(id, force_with_lease, branch, None, then, cx);
    }

    /// Corvene addition (flag `816`, history "Push Up to This Commit"):
    /// push the current branch's upstream only up to `sha`,
    /// `push <remote> <sha>:refs/heads/<upstream branch>`. No force, so a
    /// commit that is not ahead of the upstream is refused by git; unpushed
    /// tags stay behind (they may point past `sha`).
    pub fn push_up_to(id: u64, sha: String, cx: &mut dyn Host) {
        Self::push_inner(id, false, None, Some(sha), |_, _| {}, cx);
    }

    pub(crate) fn push_inner(
        id: u64,
        force_with_lease: bool,
        branch: Option<String>,
        up_to: Option<String>,
        then: impl FnOnce(PushOutcome, &mut dyn Host) + 'static,
        cx: &mut dyn Host,
    ) {
        if Self::behind_background_fetch(id, cx) {
            return Self::after_network(id, cx, move |cx| {
                Self::push_inner(id, force_with_lease, branch, up_to, then, cx)
            });
        }
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return then(PushOutcome::NotAttempted, cx);
        };
        let Some(remote) = Self::current_remote(id, cx) else {
            Self::show_popup(Popup::PublishRepository { repo: id }, cx);
            return then(PushOutcome::NotAttempted, cx);
        };
        // Corvene (`1109-remote-manager`): the current branch goes to its push
        // remote when that is not the upstream's (GHD always pushes to the
        // upstream's remote, desktop#18154); a partial push and a force push
        // (of a branch rewritten against its upstream) stay upstream, except
        // with `353-fork-tracks-upstream`, whose fork default branch follows
        // the parent and must never be force pushed there
        let push_target = {
            let s = Self::state(cx).read(cx);
            let force_follows = s.flags.bool(crate::flags::ids::FORK_TRACKS_UPSTREAM)
                && s.repository(id)
                    .is_some_and(|r| r.is_fork_contributing_to_parent());
            let current = s
                .repo_states
                .get(&id)
                .and_then(|r| r.info.as_ref())
                .and_then(|i| i.current_branch())
                .map(|b| b.name.clone());
            Self::push_target_in(s, id)
                .filter(|_| {
                    up_to.is_none()
                        && (!force_with_lease || force_follows)
                        && (branch.is_none() || branch == current)
                })
                .and_then(|t| {
                    s.repo_states
                        .get(&id)?
                        .info
                        .as_ref()?
                        .remotes
                        .iter()
                        .find(|r| r.name == t.remote)
                        .cloned()
                })
        };
        let remote = push_target.clone().unwrap_or(remote);
        // no write access: suggest a fork before git runs (GHD pushes and
        // offers it after the auth failure, `insufficientGitHubRepoPermissions`;
        // `303-fork-before-push` off takes that path)
        let read_only = {
            let s = Self::state(cx).read(cx);
            s.flags.bool(crate::flags::ids::FORK_BEFORE_PUSH)
                && s.repository(id)
                    .and_then(|r| r.github.as_ref())
                    .is_some_and(|gh| {
                        !gh.has_write_permission()
                            && s.account_for(&gh.endpoint).is_some()
                            && !Self::fork_offer_blocked(s, gh)
                    })
        };
        if read_only {
            Self::show_create_fork_dialog(id, cx);
            return then(PushOutcome::NotAttempted, cx);
        }
        let (branch, tip_error) = {
            let s = Self::state(cx).read(cx);
            let info = s.repo_states.get(&id).and_then(|r| r.info.as_ref());
            match branch {
                Some(name) => (
                    info.and_then(|i| i.branches.iter().find(|b| b.name == name).cloned()),
                    None,
                ),
                None => match info.map(|i| &i.tip) {
                    Some(Tip::Valid { branch }) => (Some(branch.clone()), None),
                    Some(Tip::Unborn { .. }) => (None, Some("The current branch is unborn.")),
                    Some(Tip::Detached { .. }) => (
                        None,
                        Some("The current repository is in a detached HEAD state."),
                    ),
                    _ => (None, None),
                },
            }
        };
        if let Some(message) = tip_error {
            Self::show_error("Could not push", message, cx);
            return then(PushOutcome::NotAttempted, cx);
        }
        let Some(branch) = branch else {
            return then(PushOutcome::NotAttempted, cx);
        };
        if up_to.is_some() && branch.upstream.is_none() {
            Self::show_error(
                "Could not push",
                "The current branch has not been published yet.",
                cx,
            );
            return then(PushOutcome::NotAttempted, cx);
        }
        // Corvene (`1222-push-target-guard`): to a differently named
        // upstream that is the remote's default branch only once confirmed
        if push_target.is_none()
            && Self::push_needs_target_confirmation(
                id,
                &branch,
                force_with_lease,
                up_to.clone(),
                cx,
            )
        {
            return then(PushOutcome::NotAttempted, cx);
        }
        if !Self::begin_network(id, cx) {
            return then(PushOutcome::NotAttempted, cx);
        }
        // GHD clears the "force push recommended" mark before the push runs,
        // so a failed force push leaves a plain Push button (desktop#16352);
        // `258-force-push-kept-on-failure` clears it after success only
        let keep_force_push_on_failure = Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::FORCE_PUSH_KEPT_ON_FAILURE);
        let force_push_branch = branch.name_without_remote().to_string();
        if force_with_lease && !keep_force_push_on_failure {
            Self::state(cx).update(cx, |s, _| {
                s.repo_state_mut(id)
                    .force_push_branches
                    .remove(&force_push_branch);
            });
        }
        Self::arm_credential_helper_for(id, &remote.url, cx);
        let askpass = Self::askpass_env_for(id, &remote.url, cx);
        // `282-fast-forward-skips-worktree-branches`
        let skip_worktree_branches = Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::FAST_FORWARD_SKIPS_WORKTREE_BRANCHES);
        let remote_name = branch
            .upstream_remote_name()
            .filter(|_| push_target.is_none())
            .map(str::to_string)
            .unwrap_or_else(|| remote.name.clone());
        let title = format!("Pushing to {remote_name}");
        Self::set_progress(
            id,
            Some(PushPullProgress {
                kind: PushPullKind::Push,
                title: title.clone(),
                description: None,
                value: 0.,
            }),
            cx,
        );
        // Corvene (`867-qualified-push-refspecs`): full ref names, so a tag
        // named like the branch does not make the refspec ambiguous (GHD
        // pushes `name:name`)
        let qualified = Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::QUALIFIED_PUSH_REFSPECS);
        let local = up_to.clone().unwrap_or_else(|| match qualified {
            true => format!("refs/heads/{}", branch.name),
            false => branch.name.clone(),
        });
        let pushed_branch = up_to.is_none().then(|| branch.name.clone());
        // `1109-remote-manager`: to the push remote, the same-named branch
        // (git's `push.default=simple` / `current`); without an upstream it
        // is published there
        let remote_branch = match &push_target {
            Some(_) if branch.upstream.is_some() => Some(match qualified {
                true => format!("refs/heads/{}", branch.name),
                false => branch.name.clone(),
            }),
            Some(_) => None,
            None => branch
                .upstream_short()
                .and_then(|u| u.split_once('/').map(|(_, b)| b.to_string()))
                .map(|b| match up_to.is_some() || qualified {
                    true => format!("refs/heads/{b}"),
                    false => b,
                }),
        };
        let remote_url = remote.url.clone();
        // GHD `pushRepo(…, gitStore.tagsToPush)`: unpushed tags ride along
        // (not on a partial push: they may point past its commit)
        let tags: Vec<String> = Self::state(cx)
            .read(cx)
            .repository(id)
            .filter(|_| up_to.is_none())
            .map(|r| r.tags_to_push.clone())
            .unwrap_or_default();
        let pushed_tags = !tags.is_empty();
        // GHD's plain fetch after a push, plus `249` / `250`
        let fetch_options = corvene_git::FetchOptions {
            prune_tags: false,
            update_moved_tags: false,
            ..Self::fetch_options(Self::state(cx).read(cx), id)
        };
        let retry = RetryAction::Push {
            force_with_lease,
            branch: Some(branch.name.clone()),
            up_to,
        };
        let cancel = Self::network_cancel_token(id, PushPullKind::Push, false, cx);
        // GHD `onHookFailure: this.onHookFailure(() => (aborted = true))`
        let hooks = crate::hooks::hook_ui(id, false, None, cx);
        let hook_callbacks = hooks.callbacks.clone();
        Self::run_network(
            id,
            cx,
            move |report| {
                cancellable(cancel.as_ref(), || {
                    let result = corvene_git::hooks::with_hook_callbacks(&hook_callbacks, || {
                        corvene_git::push(
                            git.clone(),
                            &workdir,
                            &remote_name,
                            &local,
                            remote_branch.as_deref(),
                            &tags,
                            force_with_lease,
                            askpass.as_ref(),
                            &mut |value, text| {
                                report(PushPullProgress {
                                    kind: PushPullKind::Push,
                                    title: title.clone(),
                                    description: Some(text),
                                    value: value * 0.65,
                                })
                            },
                        )
                    });
                    drop(hook_callbacks);
                    if result.is_ok() {
                        report(PushPullProgress {
                            kind: PushPullKind::Fetch,
                            title: format!("Fetching {remote_name}"),
                            description: None,
                            value: 0.65,
                        });
                        let _ = corvene_git::fetch_with(
                            git.clone(),
                            &workdir,
                            &remote_name,
                            fetch_options,
                            askpass.as_ref(),
                            &mut |value, text| {
                                report(PushPullProgress {
                                    kind: PushPullKind::Fetch,
                                    title: format!("Fetching {remote_name}"),
                                    description: Some(text),
                                    value: 0.65 + value * 0.25,
                                })
                            },
                        );
                        report(PushPullProgress {
                            kind: PushPullKind::Generic,
                            title: REFRESHING_REPOSITORY.into(),
                            description: Some("Fast-forwarding branches".into()),
                            value: 0.9,
                        });
                        let _ = corvene_git::fast_forward_branches_with(
                            git,
                            &workdir,
                            skip_worktree_branches,
                        );
                    }
                    result
                })
            },
            move |result, cx| {
                let pushed = result.is_ok();
                if pushed && force_with_lease && keep_force_push_on_failure {
                    Self::state(cx).update(cx, |s, _| {
                        s.repo_state_mut(id)
                            .force_push_branches
                            .remove(&force_push_branch);
                    });
                }
                // `clearTagsToPush` once the push went through
                if pushed && pushed_tags {
                    Self::update_tags_to_push(id, cx, Vec::clear);
                }
                // `345-issues`: a branch made from an issue is linked to it
                // on GitHub once it is there
                if pushed
                    && let Some(branch) = pushed_branch.clone()
                    && Self::state(cx)
                        .read(cx)
                        .flags
                        .bool(crate::flags::ids::ISSUES)
                {
                    Self::link_pushed_branch_to_issue(id, branch, cx);
                }
                // aborted in the HookFailed dialog: no error
                if let Err(err) = result
                    && !hooks.aborted()
                {
                    Self::handle_remote_error(
                        id,
                        "Could not push",
                        err,
                        remote_url,
                        retry,
                        false,
                        cx,
                    );
                }
                Self::refresh_repository(id, cx);
                then(
                    if pushed {
                        PushOutcome::Pushed
                    } else {
                        PushOutcome::Failed
                    },
                    cx,
                );
            },
        );
    }

    /// Corvene (`1209-current-branch-deleted-hint`): the remote the current
    /// branch's upstream was deleted from (its remote-tracking branch is
    /// gone, as `git branch -vv` says "[gone]"), while the flag is on.
    pub fn current_upstream_gone(s: &crate::state::AppState, id: u64) -> Option<String> {
        if !s.flags.bool(crate::flags::ids::CURRENT_BRANCH_DELETED_HINT) {
            return None;
        }
        let info = s.repo_states.get(&id)?.info.as_ref()?;
        let branch = info.current_branch()?;
        let upstream = branch.upstream.as_deref()?;
        if info.branches.iter().any(|b| b.full_name == upstream) {
            return None;
        }
        branch.upstream_remote_name().map(str::to_string)
    }

    /// Corvene (`1103-implicit-upstream-push-default`): the current branch's
    /// implicit upstream (`origin/feature`) and ahead/behind counts against
    /// it, while the flag is on.
    pub fn implicit_upstream_in(
        s: &crate::state::AppState,
        id: u64,
    ) -> Option<(String, AheadBehind)> {
        s.flags
            .bool(crate::flags::ids::IMPLICIT_UPSTREAM_PUSH_DEFAULT)
            .then(|| s.repo_states.get(&id)?.implicit_upstream.clone())
            .flatten()
    }

    /// Corvene (`1109-remote-manager`): the current branch's push remote
    /// while it is not the upstream's (or, without an upstream, the
    /// default remote), while the flag is on.
    pub fn push_target_in(
        s: &crate::state::AppState,
        id: u64,
    ) -> Option<&crate::remote_manager::PushTarget> {
        s.flags
            .bool(crate::flags::ids::REMOTE_MANAGER)
            .then(|| s.repo_states.get(&id)?.push_target.as_ref())
            .flatten()
    }

    /// The remote a push of the current branch goes to: its push remote
    /// (`1109-remote-manager`), else the upstream's ([`Self::current_remote`]).
    /// Branch › Push To ▸ checks it.
    pub fn push_remote_name_in(s: &crate::state::AppState, id: u64) -> Option<String> {
        Self::push_target_in(s, id)
            .map(|t| t.remote.clone())
            .or_else(|| Self::current_remote_in(s, id).map(|r| r.name))
    }

    // ---- push to / fetch from another remote (`1210-push-to-other-remote`) ----

    /// The remotes Branch › Push To ▸ and Fetch From ▸ list (at most eight,
    /// in the repository's order): none while the flag is off or the
    /// repository has one remote.
    pub fn menu_remotes(s: &crate::state::AppState, id: u64) -> Vec<String> {
        if !s.flags.bool(crate::flags::ids::PUSH_TO_OTHER_REMOTE) {
            return Vec::new();
        }
        let remotes = s
            .repo_states
            .get(&id)
            .and_then(|r| r.info.as_ref())
            .map(|i| i.remotes.as_slice())
            .unwrap_or_default();
        if remotes.len() < 2 {
            return Vec::new();
        }
        remotes.iter().take(8).map(|r| r.name.clone()).collect()
    }

    /// Branch › Fetch From ▸ `remote`.
    pub fn fetch_from_remote(id: u64, remote: String, cx: &mut dyn Host) {
        if Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::PUSH_TO_OTHER_REMOTE)
        {
            Self::fetch_remote_then(id, Some(&remote), false, |_, _| {}, cx);
        }
    }

    /// Branch › Push To ▸ `remote` (GHD pushes to the upstream's remote
    /// only): the current branch to the same-named branch of `remote`,
    /// `git push <remote> refs/heads/<b>:refs/heads/<b>` without
    /// `--set-upstream`, so the upstream stays what it was. No tags, no
    /// force.
    pub fn push_to_remote(id: u64, remote_name: String, cx: &mut dyn Host) {
        if !Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::PUSH_TO_OTHER_REMOTE)
        {
            return;
        }
        if Self::behind_background_fetch(id, cx) {
            return Self::after_network(id, cx, move |cx| {
                Self::push_to_remote(id, remote_name, cx)
            });
        }
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let (remote, branch) = {
            let s = Self::state(cx).read(cx);
            let info = s.repo_states.get(&id).and_then(|r| r.info.as_ref());
            (
                info.and_then(|i| i.remotes.iter().find(|r| r.name == remote_name))
                    .cloned(),
                info.and_then(|i| match &i.tip {
                    Tip::Valid { branch } => Some(branch.name.clone()),
                    _ => None,
                }),
            )
        };
        let (Some(remote), Some(branch)) = (remote, branch) else {
            Self::show_error(
                "Could not push",
                "Pushing to another remote needs a checked-out branch.",
                cx,
            );
            return;
        };
        if !Self::begin_network(id, cx) {
            return;
        }
        Self::arm_credential_helper_for(id, &remote.url, cx);
        let askpass = Self::askpass_env_for(id, &remote.url, cx);
        // `867-qualified-push-refspecs`
        let refspec = if Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::QUALIFIED_PUSH_REFSPECS)
        {
            format!("refs/heads/{branch}")
        } else {
            branch.clone()
        };
        let title = format!("Pushing to {}", remote.name);
        Self::set_progress(
            id,
            Some(PushPullProgress {
                kind: PushPullKind::Push,
                title: title.clone(),
                description: None,
                value: 0.,
            }),
            cx,
        );
        let fetch_options = corvene_git::FetchOptions {
            prune_tags: false,
            update_moved_tags: false,
            ..Self::fetch_options(Self::state(cx).read(cx), id)
        };
        let cancel = Self::network_cancel_token(id, PushPullKind::Push, false, cx);
        let remote_url = remote.url.clone();
        let remote_name = remote.name.clone();
        let retry = RetryAction::PushToRemote {
            remote: remote_name.clone(),
        };
        let hooks = crate::hooks::hook_ui(id, false, None, cx);
        let hook_callbacks = hooks.callbacks.clone();
        Self::run_network(
            id,
            cx,
            move |report| {
                cancellable(cancel.as_ref(), || {
                    let result = corvene_git::hooks::with_hook_callbacks(&hook_callbacks, || {
                        corvene_git::push(
                            git.clone(),
                            &workdir,
                            &remote_name,
                            &refspec,
                            Some(&refspec),
                            &[],
                            false,
                            askpass.as_ref(),
                            &mut |value, text| {
                                report(PushPullProgress {
                                    kind: PushPullKind::Push,
                                    title: title.clone(),
                                    description: Some(text),
                                    value: value * 0.8,
                                })
                            },
                        )
                    });
                    drop(hook_callbacks);
                    if result.is_ok() {
                        // the remote-tracking branch catches up
                        let _ = corvene_git::fetch_with(
                            git,
                            &workdir,
                            &remote_name,
                            fetch_options,
                            askpass.as_ref(),
                            &mut |value, text| {
                                report(PushPullProgress {
                                    kind: PushPullKind::Fetch,
                                    title: format!("Fetching {remote_name}"),
                                    description: Some(text),
                                    value: 0.8 + value * 0.2,
                                })
                            },
                        );
                    }
                    result
                })
            },
            move |result, cx| {
                if let Err(err) = result
                    && !hooks.aborted()
                {
                    Self::handle_remote_error(
                        id,
                        "Could not push",
                        err,
                        remote_url,
                        retry,
                        false,
                        cx,
                    );
                }
                Self::refresh_repository(id, cx);
            },
        );
    }

    /// The toolbar button's main click (`PushPullButton.renderButton`).
    pub fn push_pull_action(id: u64, cx: &mut dyn Host) {
        let (has_remote, tip, upstream, ab) = {
            let s = Self::state(cx).read(cx);
            let rs = s.repo_states.get(&id);
            let info = rs.and_then(|r| r.info.as_ref());
            // `1103-implicit-upstream-push-default`: the same-named branch a
            // plain `git push` updates counts as the upstream (a push still
            // records it with `--set-upstream`)
            let implicit = Self::implicit_upstream_in(s, id);
            let upstream = info
                .and_then(|i| i.current_branch())
                .and_then(|b| b.upstream.clone())
                .or_else(|| implicit.as_ref().map(|(name, _)| name.clone()));
            let ab = rs
                .and_then(|r| r.ahead_behind)
                .or_else(|| implicit.map(|(_, ab)| ab));
            (
                info.is_some_and(|i| !i.remotes.is_empty()),
                info.map(|i| i.tip.clone()),
                upstream.clone(),
                Self::with_push_target(s, id, upstream.is_some(), ab),
            )
        };
        if !has_remote {
            Self::show_popup(Popup::PublishRepository { repo: id }, cx);
            return;
        }
        match tip {
            Some(Tip::Unborn { .. }) => return Self::fetch(id, false, cx),
            Some(Tip::Detached { .. }) | Some(Tip::Unknown) | None => return,
            Some(Tip::Valid { .. }) => {}
        }
        if upstream.is_none() {
            return Self::push(id, false, None, cx);
        }
        match ab {
            Some(ab) if ab.ahead == 0 && ab.behind == 0 => Self::fetch(id, false, cx),
            // a rewritten branch is force-pushed to its upstream, not to
            // another push remote (`1109-remote-manager`)
            _ if Self::force_push_state(id, cx) == ForcePushState::Recommended
                && Self::push_target_in(Self::state(cx).read(cx), id).is_none() =>
            {
                Self::confirm_or_force_push(id, cx)
            }
            Some(ab) if ab.behind > 0 => Self::pull(id, cx),
            _ => Self::push(id, false, None, cx),
        }
    }

    /// Corvene (`1109-remote-manager`): the toolbar's counts with a push
    /// remote other than the upstream's: ahead of the push remote's branch,
    /// behind the upstream.
    pub fn with_push_target(
        s: &crate::state::AppState,
        id: u64,
        has_upstream: bool,
        ab: Option<AheadBehind>,
    ) -> Option<AheadBehind> {
        match (Self::push_target_in(s, id), ab) {
            (Some(target), Some(ab)) if has_upstream => Some(AheadBehind {
                ahead: target.ahead,
                behind: ab.behind,
            }),
            (_, ab) => ab,
        }
    }

    /// `confirmOrForcePush`
    pub fn confirm_or_force_push(id: u64, cx: &mut dyn Host) {
        let upstream = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|r| r.info.as_ref())
            .and_then(|i| i.current_branch())
            .and_then(|b| b.upstream_short().map(str::to_string));
        let Some(upstream) = upstream else {
            warn!(id, "no upstream branch to force push");
            return;
        };
        if Self::state(cx).read(cx).settings.confirm_force_push {
            Self::show_popup(
                Popup::ConfirmForcePush {
                    repo: id,
                    upstream_branch: upstream,
                },
                cx,
            );
        } else {
            Self::push(id, true, None, cx);
        }
    }

    // ---- push size (`1101-push-size-tooltip`) ----

    /// The current branch's tip and its upstream's, the key of a
    /// [`PushSizeEstimate`].
    fn push_size_key(s: &crate::state::AppState, id: u64) -> Option<(String, String)> {
        let info = s.repo_states.get(&id)?.info.as_ref()?;
        let branch = info.current_branch()?;
        let upstream = branch.upstream.as_deref()?;
        let upstream_tip = info
            .branches
            .iter()
            .find(|b| b.full_name == upstream)?
            .tip
            .clone()?;
        Some((branch.tip.clone()?, upstream_tip))
    }

    /// The push button's size note: `None` while the flag is off or the
    /// branch has no upstream, `Some(None)` until the size is known.
    pub fn push_size_for(
        s: &crate::state::AppState,
        id: u64,
    ) -> Option<Option<corvene_git::PushSize>> {
        if !s.flags.bool(crate::flags::ids::PUSH_SIZE_TOOLTIP) {
            return None;
        }
        let (head, upstream) = Self::push_size_key(s, id)?;
        let estimate = s.repo_states.get(&id)?.push_size.as_ref();
        Some(
            estimate
                .filter(|e| e.head == head && e.upstream == upstream)
                .and_then(|e| e.size),
        )
    }

    /// Work out the push size for the current tips (on hovering the push
    /// button), unless it is known or being worked out already.
    pub fn load_push_size(id: u64, cx: &mut dyn Host) {
        let key = {
            let s = Self::state(cx).read(cx);
            if !s.flags.bool(crate::flags::ids::PUSH_SIZE_TOOLTIP) {
                return;
            }
            let Some(key) = Self::push_size_key(s, id) else {
                return;
            };
            let current = s.repo_states.get(&id).and_then(|r| r.push_size.as_ref());
            if current.is_some_and(|e| (&e.head, &e.upstream) == (&key.0, &key.1)) {
                return;
            }
            key
        };
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let (head, upstream) = key;
        Self::state(cx).update(cx, |s, _| {
            s.repo_state_mut(id).push_size = Some(PushSizeEstimate {
                head: head.clone(),
                upstream: upstream.clone(),
                size: None,
            });
        });
        spawn_bg(
            cx,
            move || corvene_git::push_size(git, &workdir),
            move |result, cx| {
                let size = match result {
                    Ok(size) => size,
                    Err(err) => {
                        debug!(id, %err, "could not work out the push size");
                        return;
                    }
                };
                Self::state(cx).update(cx, |s, cx| {
                    let rs = s.repo_state_mut(id);
                    if let Some(estimate) = rs
                        .push_size
                        .as_mut()
                        .filter(|e| e.head == head && e.upstream == upstream)
                    {
                        estimate.size = Some(size);
                        cx.notify();
                    }
                });
            },
        );
    }

    // ---- publish ----

    /// `Publish.componentDidMount`: the repository's description
    /// (`getGitDescription`, `""` when it has none or git's default text),
    /// read in the background, for the Publish Repository dialog to prefill.
    pub fn git_description(
        id: u64,
        then: impl FnOnce(String, &mut dyn Host) + 'static,
        cx: &mut dyn Host,
    ) {
        let Some(path) = Self::state(cx)
            .read(cx)
            .repository(id)
            .map(|r| r.path.clone())
        else {
            return then(String::new(), cx);
        };
        spawn_bg(cx, move || corvene_git::get_git_description(&path), then);
    }

    /// `_publishRepository`: create the GitHub repository, add `origin`, push.
    /// `team_id`: flag `329-publish-team`.
    #[allow(clippy::too_many_arguments)]
    pub fn publish_repository(
        id: u64,
        name: String,
        description: String,
        private: bool,
        account: Account,
        org: Option<String>,
        team_id: Option<u64>,
        cx: &mut dyn Host,
    ) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let Some(token) = corvene_platform::keychain::token(&account.host(), &account.login)
            .ok()
            .flatten()
        else {
            Self::show_error(
                "Could not publish",
                "The account's token is missing from the keychain. Sign in again.",
                cx,
            );
            return;
        };
        Self::state(cx).update(cx, |s, cx| {
            s.repo_state_mut(id).publishing = true;
            cx.notify();
        });
        let endpoint = corvene_github::Endpoint::from_api_base(&account.endpoint);
        // `527-multiple-accounts`: the repository uses the account it was
        // published with
        let login = account.login.clone();
        let (error_details, sso_hint) = {
            let flags = &Self::state(cx).read(cx).flags;
            (
                flags.bool(crate::flags::ids::API_ERROR_DETAILS),
                flags.bool(crate::flags::ids::API_SAML_SSO_HINT),
            )
        };
        spawn_bg(
            cx,
            move || {
                let client = corvene_github::Client::new(endpoint, token)
                    .with_error_details(error_details)
                    .with_sso_hint(sso_hint);
                let repo = client
                    .create_repository(org.as_deref(), &name, &description, private, team_id)
                    .map_err(|e| e.to_string())?;
                corvene_git::add_remote(git, &workdir, "origin", &repo.clone_url)
                    .map_err(|e| e.to_string())?;
                Ok::<_, String>(repo)
            },
            move |result, cx| {
                Self::state(cx).update(cx, |s, cx| {
                    s.repo_state_mut(id).publishing = false;
                    cx.notify();
                });
                match result {
                    Ok(repo) => {
                        info!(id, name = %repo.name, "published repository");
                        Self::state(cx).update(cx, |s, cx| {
                            let multiple = s.multiple_accounts();
                            if let Some(r) = s.repositories.iter_mut().find(|r| r.id == id) {
                                r.github = Some(repo);
                                if multiple {
                                    r.account = Some(login);
                                }
                            }
                            let _ = s.store.save_repositories(&s.repositories);
                            cx.notify();
                        });
                        // the dialog, wherever it is in the popup stack
                        Self::close_popups_where(
                            |p| matches!(p, Popup::PublishRepository { .. }),
                            cx,
                        );
                        Self::refresh_repository(id, cx);
                        // push the current branch (and set its upstream)
                        Self::push_after_publish(id, cx);
                    }
                    Err(message) => Self::show_error("Could not publish repository", message, cx),
                }
            },
        );
    }

    fn push_after_publish(id: u64, cx: &mut dyn Host) {
        // the remote list is refreshed asynchronously; push once it is there
        cx.spawn(async move |cx: &mut AsyncCtx| {
            for _ in 0..20 {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let ready = cx.update(|cx| {
                    Self::state(cx)
                        .read(cx)
                        .repo_states
                        .get(&id)
                        .and_then(|r| r.info.as_ref())
                        .is_some_and(|i| !i.remotes.is_empty() && !r_loading(cx, id))
                });
                if ready {
                    cx.update(|cx| Self::push(id, false, None, cx));
                    return;
                }
            }
        })
        .detach();
    }

    // ---- generic credentials ----

    /// `GenericGitAuthentication` › Save: keychain + retry the operation.
    pub fn save_generic_credentials(
        host: String,
        username: String,
        password: String,
        id: u64,
        retry: RetryAction,
        cx: &mut dyn Host,
    ) {
        if let Err(err) =
            corvene_platform::keychain::store_generic_password(&host, &username, &password)
        {
            Self::show_error("Could not save credentials", err.to_string(), cx);
            return;
        }
        Self::state(cx).update(cx, |s, _| {
            s.generic_logins.insert(host.clone(), username.clone());
            let _ = s.store.save_generic_logins(&s.generic_logins);
        });
        Self::close_popup(cx);
        Self::perform_retry(id, retry, cx);
    }

    // ---- LFS ----

    /// GHD `_addRepositories` › `InitializeLFS`: offer to install the hooks
    /// when the repository tracks paths with LFS but has no hooks yet.
    pub fn check_lfs(id: u64, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let already_asked = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .is_some_and(|r| r.lfs_checked);
        if already_asked {
            return;
        }
        Self::state(cx).update(cx, |s, _| s.repo_state_mut(id).lfs_checked = true);
        // `905-lfs-detect-by-attributes`: read the .gitattributes files instead
        // of `git lfs track`, which walks the whole worktree
        let by_attributes = Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::LFS_DETECT_BY_ATTRIBUTES);
        spawn_bg(
            cx,
            move || {
                if by_attributes {
                    corvene_git::is_using_lfs_by_attributes(git.clone(), &workdir)
                        && !corvene_git::lfs_hooks_installed(&workdir)
                        && corvene_git::lfs_available(git)
                } else {
                    corvene_git::lfs_available(git.clone())
                        && corvene_git::is_using_lfs(git, &workdir)
                        && !corvene_git::lfs_hooks_installed(&workdir)
                }
            },
            move |needs_init, cx| {
                if needs_init && Self::state(cx).read(cx).popup().is_none() {
                    Self::show_popup(Popup::InitializeLFS { repos: vec![id] }, cx);
                }
            },
        );
    }

    /// `_installLFSHooks`
    pub fn install_lfs_hooks(repos: Vec<u64>, cx: &mut dyn Host) {
        let contexts: Vec<_> = repos
            .iter()
            .filter_map(|id| Self::repo_context(*id, cx))
            .collect();
        spawn_bg(
            cx,
            move || {
                let mut errors = Vec::new();
                for (git, workdir) in contexts {
                    if let Err(err) = corvene_git::install_lfs_hooks(git, &workdir) {
                        errors.push(err.to_string());
                    }
                }
                errors
            },
            move |errors, cx| {
                if !errors.is_empty() {
                    Self::show_error("Could not initialize Git LFS", errors.join("\n"), cx);
                }
            },
        );
    }

    // ---- background fetch + indicators ----

    /// Start the periodic background fetch and sidebar indicator refresh
    /// (`BackgroundFetcher`, `RepositoryIndicatorUpdater`). Call once.
    pub fn start_background_tasks(cx: &mut dyn Host) {
        Self::start_proxy(cx);
        // the API's proxy falls back to git's `http.proxy` (GHD's requests
        // go through Chromium's proxy resolution, `corvene_platform::proxy`)
        if let Some(git) = Self::state(cx).read(cx).git.clone() {
            spawn_bg(
                cx,
                move || corvene_git::global_config_value(git, "http.proxy"),
                |proxy, _| corvene_platform::proxy::set_git_http_proxy(proxy),
            );
        }
        // Android: WorkManager wakes the process about once an hour, also
        // while its timers are frozen in the background
        #[cfg(target_os = "android")]
        {
            corvene_git::process::set_network_observer(corvene_platform::android::network_command);
            let (tx, rx) = async_channel::unbounded::<()>();
            corvene_platform::android::set_background_fetch_handler(move || {
                let _ = tx.try_send(());
            });
            cx.spawn(async move |cx: &mut AsyncCtx| {
                while rx.recv().await.is_ok() {
                    info!("background fetch woken by WorkManager");
                    cx.update(Self::background_fetch_tick);
                }
            })
            .detach();
        }
        cx.spawn(async move |cx: &mut AsyncCtx| {
            // skew the first run so several instances do not sync up
            cx.background_executor()
                .timer(Duration::from_secs(20))
                .await;
            loop {
                // in the background WorkManager drives the fetch (above)
                if !crate::pull_requests::android_in_background() {
                    cx.update(Self::background_fetch_tick);
                }
                cx.background_executor()
                    .timer(BACKGROUND_FETCH_MINIMUM)
                    .await;
            }
        })
        .detach();
        // `217-prompt-indicator-refresh`: the first indicator refresh runs
        // right after launch (GHD's updater starts on its delayed cadence)
        // Android: starting git twice per listed repository right at launch
        // competes with drawing the first frames, so the delayed cadence
        let first_indicators = if !cfg!(target_os = "android")
            && Self::state(cx)
                .read(cx)
                .flags
                .bool(crate::flags::ids::PROMPT_INDICATOR_REFRESH)
        {
            Duration::from_secs(1)
        } else {
            Duration::from_secs(45)
        };
        cx.spawn(async move |cx: &mut AsyncCtx| {
            cx.background_executor().timer(first_indicators).await;
            loop {
                if !crate::pull_requests::android_in_background() {
                    cx.update(Self::refresh_indicators);
                }
                cx.background_executor()
                    .timer(INDICATOR_REFRESH_INTERVAL)
                    .await;
            }
        })
        .detach();
    }

    /// Fetch the selected GitHub repository (see `244-background-fetch`) when
    /// its last fetch is older than the interval (`shouldBackgroundFetch`).
    /// One background fetch round (the hourly timer, WorkManager on Android).
    pub fn background_fetch_tick(cx: &mut dyn Host) {
        for id in Self::state(cx).read(cx).visible_repositories() {
            Self::background_fetch_repository(id, cx);
        }
    }

    /// [`Self::background_fetch_tick`] for one visible repository.
    fn background_fetch_repository(id: u64, cx: &mut dyn Host) {
        let (id, last_fetched, busy, known_push, skip_unchanged) = {
            let s = Self::state(cx).read(cx);
            let Some(repo) = s.repository(id) else { return };
            // `278-fetch-on-known-push`: between the hourly fetches, ask the
            // API whether the repository was pushed to since the last one
            let known_push = repo
                .github
                .clone()
                .filter(|_| s.flags.bool(crate::flags::ids::FETCH_ON_KNOWN_PUSH));
            // `298-background-fetch-skips-unchanged`: the hourly fetch of a
            // GitHub repository that is not a fork asks the API first
            let skip_unchanged = repo.github.clone().filter(|gh| {
                !gh.fork
                    && s.flags
                        .bool(crate::flags::ids::BACKGROUND_FETCH_SKIPS_UNCHANGED)
            });
            // GHD fetches GitHub repositories only; `244-background-fetch`
            // can also turn it off or extend it to any remote
            let fetch = match s.flags.text(crate::flags::ids::BACKGROUND_FETCH) {
                "off" => false,
                "any" => true,
                _ => repo.github.is_some(),
            };
            if !fetch {
                return;
            }
            let rs = s.repo_states.get(&id);
            // `288-dead-remote-indicator`: not until a fetch works again
            if rs.is_some_and(|r| r.remote_not_found)
                && s.flags.bool(crate::flags::ids::DEAD_REMOTE_INDICATOR)
            {
                return;
            }
            (
                id,
                rs.and_then(|r| r.last_fetched),
                rs.is_some_and(|r| r.push_pull_in_progress || r.mco.is_some()),
                known_push,
                skip_unchanged,
            )
        };
        if busy {
            return;
        }
        let age = last_fetched.map(|at| {
            SystemTime::now()
                .duration_since(at)
                .unwrap_or(Duration::MAX)
        });
        let due = age.is_none_or(|age| age >= BACKGROUND_FETCH_INTERVAL);
        if due {
            // `298-background-fetch-skips-unchanged`: nothing was pushed since
            // the last fetch, so the fetch would bring nothing; a real fetch
            // still runs every few hours (a fork's parent, pull request refs
            // and deleted branches do not move `pushed_at`)
            if let (Some(github), Some(last_fetched)) = (skip_unchanged, last_fetched)
                && age.is_some_and(|age| age < FORCED_FETCH_INTERVAL)
                && let Some(api) = Self::api_for_repository(id, &github, cx)
            {
                return Self::fetch_if_pushed(id, github, api, last_fetched, true, cx);
            }
            info!(id, "background fetch");
            Self::fetch(id, true, cx);
            return;
        }
        let (Some(github), Some(last_fetched)) = (known_push, last_fetched) else {
            return;
        };
        let Some(api) = Self::api_for_repository(id, &github, cx) else {
            return;
        };
        Self::fetch_if_pushed(id, github, api, last_fetched, false, cx);
    }

    /// Ask GitHub when `github` was last pushed to and fetch repository `id`
    /// when that was after `last_fetched` (`278-fetch-on-known-push`) - or,
    /// with `due` (`298-background-fetch-skips-unchanged`, the hourly fetch),
    /// unless it clearly was not: a failed or empty answer fetches too.
    fn fetch_if_pushed(
        id: u64,
        github: corvene_models::GitHubRepository,
        (endpoint, token, _): (corvene_github::Endpoint, String, String),
        last_fetched: SystemTime,
        due: bool,
        cx: &mut dyn Host,
    ) {
        spawn_bg(
            cx,
            move || {
                corvene_github::Client::new(endpoint, token)
                    .pushed_at(&github.owner, &github.name)
                    .map_err(|err| err.to_string())
            },
            move |result, cx| {
                let pushed_at = match result {
                    Ok(pushed_at) => pushed_at.as_deref().and_then(corvene_models::parse_iso8601),
                    Err(err) => {
                        debug!(id, %err, "could not read when the repository was pushed to");
                        None
                    }
                };
                // still shown, not fetched meanwhile, and nothing running
                let still_stale = {
                    let s = Self::state(cx).read(cx);
                    let rs = s.repo_states.get(&id);
                    s.visible_repositories().contains(&id)
                        && rs.and_then(|r| r.last_fetched) == Some(last_fetched)
                        && !rs.is_some_and(|r| r.push_pull_in_progress || r.mco.is_some())
                };
                if !still_stale {
                    return;
                }
                if pushed_after_fetch(pushed_at, last_fetched) {
                    info!(id, "background fetch after a push seen on GitHub");
                    Self::fetch(id, true, cx);
                } else if due && pushed_at.is_none() {
                    info!(
                        id,
                        "background fetch (GitHub did not say when it was pushed to)"
                    );
                    Self::fetch(id, true, cx);
                } else if due {
                    debug!(
                        id,
                        "background fetch skipped: nothing pushed since the last fetch"
                    );
                }
            },
        );
    }

    /// [`Self::refresh_indicators`] unless indicators were refreshed less
    /// than a minute ago (`217-prompt-indicator-refresh`, on opening the
    /// repository list).
    pub fn refresh_indicators_if_stale(cx: &mut dyn Host) {
        let fresh = LAST_INDICATOR_REFRESH
            .with(|last| last.get())
            .is_some_and(|at| at.elapsed() < INDICATOR_REFRESH_MINIMUM);
        if !fresh {
            Self::refresh_indicators(cx);
        }
    }

    /// `refreshIndicatorForRepository` for every repository: changed files
    /// and ahead/behind, shown in the repository list.
    pub fn refresh_indicators(cx: &mut dyn Host) {
        LAST_INDICATOR_REFRESH.with(|last| last.set(Some(Instant::now())));
        let (enabled, watched) = {
            let s = Self::state(cx).read(cx);
            (
                s.settings.repository_indicators_enabled,
                // `428-menu-bar-status-item`: the watched repositories get
                // their branch, ahead/behind and CI ref in the same pass,
                // with the sidebar's icons off too
                if s.menu_bar_enabled() {
                    s.watched_repositories()
                } else {
                    Vec::new()
                },
            )
        };
        if !enabled {
            Self::state(cx).update(cx, |s, cx| {
                if !s.indicators.is_empty() {
                    s.indicators.clear();
                    save_indicators(s);
                    cx.notify();
                }
            });
            if watched.is_empty() {
                return;
            }
        }
        let (git, repos, stash_icon, other_stash) = {
            let s = Self::state(cx).read(cx);
            let Some(git) = s.git.clone() else { return };
            (
                git,
                s.repositories
                    .iter()
                    .filter(|r| !r.missing && (enabled || watched.contains(&r.id)))
                    .map(|r| (r.id, r.path.clone(), r.github.clone()))
                    .collect::<Vec<_>>(),
                s.flags.bool(crate::flags::ids::REPOSITORY_LIST_STASH_ICON),
                s.flags.bool(crate::flags::ids::SHOW_LATEST_OTHER_STASH),
            )
        };
        spawn_bg(
            cx,
            move || {
                let mut out: HashMap<u64, RepoIndicator> = HashMap::new();
                let mut statuses: crate::menu_bar_status::MenuBarStatuses = HashMap::new();
                for (id, path, github) in repos {
                    let Ok(info) = corvene_git::open_repository(&path) else {
                        continue;
                    };
                    let ahead_behind = info.current_branch().and_then(|b| {
                        corvene_git::ahead_behind(git.clone(), &info.workdir, b)
                            .ok()
                            .flatten()
                    });
                    let branch = info.current_branch().map(|b| b.name.clone());
                    if watched.contains(&id) {
                        statuses.insert(
                            id,
                            crate::menu_bar_status::WatchedRepoStatus {
                                branch: branch.clone(),
                                ahead_behind,
                                ci_ref: github
                                    .as_ref()
                                    .and_then(|gh| crate::menu_bar_status::ci_ref_of(&info, gh)),
                            },
                        );
                    }
                    if !enabled {
                        continue;
                    }
                    let changed = corvene_git::get_status(git.clone(), &info.workdir)
                        .map(|st| st.files.len())
                        .unwrap_or(0);
                    let has_stash =
                        stash_icon && corvene_git::has_stash(&info.workdir, other_stash);
                    out.insert(
                        id,
                        RepoIndicator {
                            ahead_behind,
                            changed_files: changed,
                            branch,
                            has_stash,
                        },
                    );
                }
                (out, statuses)
            },
            move |(indicators, statuses), cx| {
                Self::state(cx).update(cx, |s, cx| {
                    if enabled {
                        s.indicators = indicators;
                        save_indicators(s);
                    }
                    s.menu_bar_statuses = statuses;
                    cx.notify();
                });
                Self::touch_menu_bar_statuses(cx);
            },
        );
    }

    /// Clone dialog: fetch the account's repositories (`ApiRepositoriesStore.loadRepositories`).
    pub fn load_api_repositories(account: Account, cx: &mut dyn Host) {
        // per account (`Account::key`): an endpoint can have several
        // (`527-multiple-accounts`)
        let key = account.key();
        let already = Self::state(cx).update(cx, |s, cx| {
            if s.api_repositories_loading.contains(&key) {
                return true;
            }
            // `ApiRepositoriesStore`: the last list shows right away while
            // the fresh one loads (kept under the endpoint before 527)
            if !s.api_repositories.contains_key(&key)
                && let Ok(Some(cached)) = s
                    .store
                    .get::<Vec<corvene_models::GitHubRepository>>(&format!(
                        "api-repositories:{key}"
                    ))
                    .and_then(|found| match found {
                        Some(found) => Ok(Some(found)),
                        None => s
                            .store
                            .get(&format!("api-repositories:{}", account.endpoint)),
                    })
            {
                s.api_repositories.insert(key.clone(), cached);
            }
            s.api_repositories_loading.insert(key.clone());
            cx.notify();
            false
        });
        if already {
            return;
        }
        let Some(token) = corvene_platform::keychain::token(&account.host(), &account.login)
            .ok()
            .flatten()
        else {
            Self::state(cx).update(cx, |s, cx| {
                s.api_repositories_loading.remove(&key);
                cx.notify();
            });
            return;
        };
        let api = corvene_github::Endpoint::from_api_base(&account.endpoint);
        spawn_bg(
            cx,
            move || {
                corvene_github::Client::new(api, token)
                    .user_repositories()
                    .map_err(|e| e.to_string())
            },
            move |result, cx| {
                Self::state(cx).update(cx, |s, cx| {
                    s.api_repositories_loading.remove(&key);
                    match result {
                        Ok(repos) => {
                            if let Err(err) =
                                s.store.set(&format!("api-repositories:{key}"), &repos)
                            {
                                warn!(%err, "could not cache the repository list");
                            }
                            s.api_repositories.insert(key.clone(), repos);
                        }
                        Err(err) => warn!(%err, "could not load repositories"),
                    }
                    cx.notify();
                });
            },
        );
    }

    /// GHD `CreateFork` is out of scope; a plain "no write access" hint.
    pub fn remote_url_host(url: &str) -> String {
        host_of(url)
    }
}

/// `299-pull-all-repositories`: the repositories Pull All left as they
/// were, by reason.
#[derive(Debug, Default)]
struct PullAllSummary {
    diverged: Vec<String>,
    local_changes: Vec<String>,
    in_progress: Vec<String>,
    /// Another network operation was running.
    busy: Vec<String>,
    /// "name: error" (the fetch or the fast-forward failed).
    failed: Vec<String>,
}

impl PullAllSummary {
    fn add(
        &mut self,
        name: String,
        outcome: Result<corvene_git::FastForward, corvene_git::GitError>,
    ) {
        use corvene_git::FastForward;
        match outcome {
            Ok(FastForward::Diverged) => self.diverged.push(name),
            Ok(FastForward::LocalChanges) => self.local_changes.push(name),
            Ok(FastForward::OperationInProgress) => self.in_progress.push(name),
            Ok(FastForward::Done | FastForward::UpToDate | FastForward::NoUpstream) => {}
            Err(err) => self.failed.push(format!("{name}: {err}")),
        }
    }

    /// The dialog's text, `None` when every repository was pulled.
    fn message(&self) -> Option<String> {
        let sections = [
            (
                "Both the branch and its upstream have new commits; pull these one at a time:",
                &self.diverged,
            ),
            (
                "Uncommitted changes; commit or stash them, then pull:",
                &self.local_changes,
            ),
            (
                "A merge, rebase or cherry-pick is in progress:",
                &self.in_progress,
            ),
            ("Another fetch, pull or push was running:", &self.busy),
            ("Could not be fetched or fast-forwarded:", &self.failed),
        ];
        let text: Vec<String> = sections
            .iter()
            .filter(|(_, names)| !names.is_empty())
            .map(|(title, names)| {
                let lines: Vec<String> = names.iter().map(|n| format!("• {n}")).collect();
                format!("{title}\n{}", lines.join("\n"))
            })
            .collect();
        (!text.is_empty()).then(|| text.join("\n\n"))
    }
}

fn r_loading(cx: &dyn Host, id: u64) -> bool {
    Dispatcher::state(cx)
        .read(cx)
        .repo_states
        .get(&id)
        .is_some_and(|r| r.loading)
}

/// `1102-repository-credential-helper`: `askpass` without the logins of
/// `accounts` (`host=login`) for `host`.
fn without_accounts_of(askpass: &AskpassEnv, accounts: &[String], host: &str) -> AskpassEnv {
    let logins: Vec<&str> = askpass
        .logins
        .split(';')
        .filter(|login| {
            !(login.split_once('=').is_some_and(|(h, _)| h == host)
                && accounts.iter().any(|a| a == login))
        })
        .collect();
    AskpassEnv {
        program: askpass.program.clone(),
        logins: logins.join(";"),
        gh_cli: askpass.gh_cli,
        no_ui: askpass.no_ui,
    }
}

/// `527-multiple-accounts`: `askpass` answering for `host` with `login`,
/// the account a repository uses, when that is one of `accounts`
/// (`host=login` pairs of every signed-in account).
fn with_account_of(
    askpass: &AskpassEnv,
    accounts: &[String],
    host: &str,
    login: &str,
) -> AskpassEnv {
    let pair = format!("{host}={login}");
    let known = accounts.iter().any(|a| a.eq_ignore_ascii_case(&pair));
    let logins: Vec<&str> = askpass
        .logins
        .split(';')
        .map(|l| {
            if known
                && l.split_once('=').is_some_and(|(h, _)| h == host)
                && accounts.iter().any(|a| a == l)
            {
                pair.as_str()
            } else {
                l
            }
        })
        .collect();
    AskpassEnv {
        program: askpass.program.clone(),
        logins: logins.join(";"),
        gh_cli: askpass.gh_cli,
        no_ui: askpass.no_ui,
    }
}

/// `github.com` from `https://github.com/a/b.git` or `git@github.com:a/b.git`.
pub fn host_of(url: &str) -> String {
    let without_scheme = url.split("://").nth(1).unwrap_or(url);
    let without_user = without_scheme
        .split_once('@')
        .map(|(_, h)| h)
        .unwrap_or(without_scheme);
    without_user
        .split(['/', ':'])
        .next()
        .unwrap_or(without_user)
        .to_lowercase()
}

/// `278-fetch-on-known-push`: GitHub saw a push after the last fetch
/// (`pushed_at` has a one-second resolution, so the same second counts).
fn pushed_after_fetch(pushed_at: Option<SystemTime>, last_fetched: SystemTime) -> bool {
    pushed_at.is_some_and(|pushed| pushed + Duration::from_secs(1) > last_fetched)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_pushes_after_the_last_fetch() {
        let fetched = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        assert!(!pushed_after_fetch(None, fetched));
        assert!(!pushed_after_fetch(
            Some(fetched - Duration::from_secs(60)),
            fetched
        ));
        assert!(pushed_after_fetch(Some(fetched), fetched));
        assert!(pushed_after_fetch(
            Some(fetched + Duration::from_secs(60)),
            fetched
        ));
    }

    #[test]
    fn pull_all_summary_lists_what_was_left() {
        use corvene_git::FastForward;
        let mut summary = PullAllSummary::default();
        summary.add("a".into(), Ok(FastForward::Done));
        summary.add("b".into(), Ok(FastForward::UpToDate));
        assert_eq!(summary.message(), None);
        summary.add("c".into(), Ok(FastForward::Diverged));
        summary.add("d".into(), Ok(FastForward::LocalChanges));
        summary.failed.push("e: boom".into());
        let message = summary.message().unwrap();
        assert!(message.contains("one at a time:\n• c"));
        assert!(message.contains("then pull:\n• d"));
        assert!(message.ends_with("fast-forwarded:\n• e: boom"));
    }

    #[test]
    fn credential_helper_repositories_drop_the_account_login() {
        let askpass = AskpassEnv {
            program: "/x".into(),
            logins: "github.com=octocat;ghe.corp=me;github.com=saved".into(),
            gh_cli: false,
            no_ui: false,
        };
        let accounts = ["github.com=octocat".to_string(), "ghe.corp=me".to_string()];
        let env = without_accounts_of(&askpass, &accounts, "github.com");
        assert_eq!(env.logins, "ghe.corp=me;github.com=saved");
    }

    #[test]
    fn pull_all_answers_with_the_repository_s_account() {
        let askpass = AskpassEnv {
            program: "/x".into(),
            logins: "github.com=me;ghe.corp=work".into(),
            gh_cli: false,
            no_ui: false,
        };
        let accounts = [
            "github.com=me".to_string(),
            "ghe.corp=work".to_string(),
            "github.com=me-at-work".to_string(),
        ];
        let env = with_account_of(&askpass, &accounts, "github.com", "me-at-work");
        assert_eq!(env.logins, "github.com=me-at-work;ghe.corp=work");
        // a login that is not signed in changes nothing
        let env = with_account_of(&askpass, &accounts, "github.com", "gone");
        assert_eq!(env.logins, askpass.logins);
    }

    #[test]
    fn host_of_urls() {
        assert_eq!(host_of("https://github.com/a/b.git"), "github.com");
        assert_eq!(host_of("git@GitHub.com:a/b.git"), "github.com");
        assert_eq!(host_of("https://user@ghe.corp:8443/a/b"), "ghe.corp");
    }
}
