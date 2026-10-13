//! Forks - GHD `ui/forks/create-fork-dialog.tsx` (`CreateFork` popup),
//! `app-store.ts#_convertRepositoryToFork`, `_updateRepositoryWorkflowPreferences`,
//! `git-store.ts#addUpstreamRemoteIfNeeded` / `ensureUpstreamRemoteURL` /
//! `updateExistingUpstreamRemote`, and the `UpstreamAlreadyExists` popup's
//! Update / Ignore (`app-store.ts#_updateExistingUpstreamRemote`,
//! `_ignoreExistingUpstreamRemote`).
//!
//! The repository's API record, with the user's `permissions`, is refreshed
//! on selection and after signing in (`repositoryWithRefreshedGitHubRepository`)
//! and persisted with the repository. Without write access the commit form
//! suggests a fork and a push opens `CreateFork` before git runs (GHD runs the
//! push and offers the fork when it fails authentication,
//! `insufficientGitHubRepoPermissions`). Repositories whose permissions are
//! still unknown fall back to offering the fork after a push refused with
//! "Permission denied".
//!
//! Deviation (flag `fork-remotes-keep-ssh`): when the repository's remote is
//! SSH, the fork's origin and the `upstream` remote are set over SSH too
//! (GHD always uses the API's HTTPS `clone_url`).
//!
//! Deviation (flag `fork-offer-respects-allow-forking`): a repository whose
//! API record says `allow_forking: false` gets no fork suggestion, no
//! `CreateFork` before a push and no fork offer after a refused push; the
//! push runs (or fails) as usual.
//!
//! Deviation (flag `clear-lost-github-association`): when refreshing the
//! API record answers 404 (the repository was deleted, or the account lost
//! access), the stored GitHub association is dropped and the repository is
//! handled as a plain git repository (the next launch or sign-in matches its
//! remote again); GHD's `repositoryWithRefreshedGitHubRepository` keeps the
//! stale record forever.
//!
//! Deviation: a repository with a GitHub association keeps it as stored and
//! only one without is matched again, once a session when its remotes are
//! first read and after signing in. GHD matches the default remote on every
//! selection (`app-store.ts#matchGitHubRepository`).
//!
//! Deviation (flag `353-fork-tracks-upstream`, desktop#19457): a fork set up
//! to contribute to its parent (Choose Fork Settings, Repository Settings, or
//! a cloned fork getting its `upstream` remote) has its local default branch
//! track the parent's (`upstream/<default>`), so Pull brings the parent's
//! commits without syncing the fork first. `branch.<default>.pushRemote`
//! keeps its pushes, force pushes included, on the fork; that needs
//! `1109-remote-manager`, without which nothing changes. Only a default
//! branch tracking the fork's same-named branch with no commits the parent
//! lacks is switched, and choosing "For my own purposes" switches it back.
//! GHD leaves the default branch tracking the fork.

use crate::host::Host;
use corvene_github::Client;
use corvene_models::{
    BranchKind, ForkContributionTarget, GitHubRepository, Remote, clone_url_like_remote,
    url_matches_remote,
};
use tracing::{debug, info, warn};

use crate::dispatcher::Dispatcher;
use crate::remote::spawn_bg;
use crate::state::Popup;

/// `UpstreamRemoteName`
pub const UPSTREAM_REMOTE_NAME: &str = "upstream";

/// GHD `findUpstreamRemote(parent, remotes)`
/// (`lib/stores/helpers/find-upstream-remote.ts`): the remote named
/// `upstream` when it points at `parent` (`repositoryMatchesRemote`: its
/// `html_url` or `clone_url`, ignoring case and protocol).
pub fn find_upstream_remote<'a>(
    parent: &GitHubRepository,
    remotes: &'a [Remote],
) -> Option<&'a Remote> {
    let upstream = remotes.iter().find(|r| r.name == UPSTREAM_REMOTE_NAME)?;
    (url_matches_remote(&parent.html_url, &upstream.url)
        || url_matches_remote(&parent.clone_url, &upstream.url))
    .then_some(upstream)
}

/// GHD `getIgnoreExistingUpstreamRemoteKey` (a localStorage key there, a
/// store key here).
fn ignore_existing_upstream_key(id: u64) -> String {
    format!("repository/{id}/ignoreExistingUpstreamRemote")
}

impl Dispatcher {
    /// `repositoryWithRefreshedGitHubRepository`: re-read the repository's
    /// API record (parent, default branch, `permissions`) with the account
    /// for its endpoint and persist it. A failed request keeps what is stored.
    /// Corvene (`341-custom-autolinks`): the GitHub repository's autolinks
    /// (`GET repos/{owner}/{name}/autolinks`, admins only: anything else
    /// leaves none), read when it is selected.
    pub fn refresh_autolinks(id: u64, cx: &mut dyn Host) {
        if !Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::CUSTOM_AUTOLINKS)
        {
            return;
        }
        let Some(github) = Self::state(cx)
            .read(cx)
            .repository(id)
            .and_then(|r| r.github.clone())
        else {
            return;
        };
        // the API answers repository admins only
        if github
            .permissions
            .is_some_and(|p| p != corvene_models::RepositoryPermission::Admin)
        {
            return;
        }
        let Some((endpoint, token, _)) = Self::api_for_repository(id, &github, cx) else {
            return;
        };
        let (owner, name) = (github.owner.clone(), github.name.clone());
        spawn_bg(
            cx,
            move || Client::new(endpoint, token).autolinks(&owner, &name),
            move |result, cx| {
                let autolinks: Vec<corvene_models::Autolink> = match result {
                    Ok(list) => list
                        .unwrap_or_default()
                        .into_iter()
                        .map(|a| corvene_models::Autolink {
                            key_prefix: a.key_prefix,
                            url_template: a.url_template,
                            is_alphanumeric: a.is_alphanumeric,
                        })
                        .collect(),
                    Err(err) => {
                        debug!(id, %err, "could not read the repository's autolinks");
                        return;
                    }
                };
                Self::state(cx).update(cx, |s, cx| {
                    let rs = s.repo_state_mut(id);
                    if rs.api_autolinks != autolinks {
                        rs.api_autolinks = autolinks;
                        cx.notify();
                    }
                });
            },
        );
    }

    /// Corvene (`1211-issuetracker-links`): the repository's `.issuetracker`
    /// trackers as link rules, read when it is selected; a regex that does
    /// not compile is left out (and logged).
    pub fn load_issue_trackers(id: u64, cx: &mut dyn Host) {
        let s = Self::state(cx).read(cx);
        if !s.flags.bool(crate::flags::ids::ISSUETRACKER_LINKS) {
            return;
        }
        let (Some(git), Some(path)) = (s.git.clone(), s.repository(id).map(|r| r.path.clone()))
        else {
            return;
        };
        spawn_bg(
            cx,
            move || {
                corvene_git::issue_trackers(git, &path)
                    .into_iter()
                    .filter_map(|tracker| match regex::Regex::new(&tracker.regex) {
                        Ok(regex) => Some(crate::text_tokens::LinkRule::Pattern {
                            regex,
                            url: tracker.url,
                        }),
                        Err(err) => {
                            warn!(name = %tracker.name, %err, "ignoring an .issuetracker regex");
                            None
                        }
                    })
                    .collect::<Vec<_>>()
            },
            move |rules, cx| {
                Self::state(cx).update(cx, |s, cx| {
                    let rs = s.repo_state_mut(id);
                    if !(rs.issue_trackers.is_empty() && rules.is_empty()) {
                        rs.issue_trackers = rules;
                        cx.notify();
                    }
                });
            },
        );
    }

    pub fn refresh_github_repository(id: u64, cx: &mut dyn Host) {
        // `527-multiple-accounts`: a repository from before a second
        // account signed in finds its account first (and refreshes then)
        if Self::needs_account_lookup(Self::state(cx).read(cx), id) {
            return Self::resolve_repository_account(id, cx);
        }
        let stored = Self::state(cx)
            .read(cx)
            .repository(id)
            .and_then(|r| r.github.clone());
        // GHD matches the default remote on every refresh; Corvene keeps the
        // stored record and matches only a repository without one, once a
        // session (its remotes must have been read)
        let (github, matched) = match stored {
            Some(github) => (github, false),
            None => {
                let matched = Self::state(cx).update(cx, |s, _| {
                    let info = s.repo_states.get(&id)?.info.clone()?;
                    s.repo_state_mut(id).github_rematched = true;
                    Self::match_github_repository(s, &info)
                });
                match matched {
                    Some(github) => (github, true),
                    None => return,
                }
            }
        };
        let Some((endpoint, token, _)) = Self::api_for_repository(id, &github, cx) else {
            // signed out (`331-github-without-account`): the match is all
            // there is
            if matched {
                Self::set_matched_github_repository(id, github, cx);
            }
            return;
        };
        let (owner, name) = (github.owner.clone(), github.name.clone());
        let clear_lost = Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::CLEAR_LOST_GITHUB_ASSOCIATION);
        spawn_bg(
            cx,
            move || Client::new(endpoint, token).repository(&owner, &name),
            move |result, cx| match result {
                // GHD: a failed request still associates the skeleton
                // repository from the match
                Err(corvene_github::GitHubError::Api { status: 404, .. })
                    if matched && clear_lost => {}
                Err(err) if matched => {
                    warn!(id, %err, "could not read the matched GitHub repository");
                    Self::set_matched_github_repository(id, github, cx);
                }
                Ok(fresh) => {
                    let (changed, associated) = Self::state(cx).update(cx, |s, cx| {
                        let Some(repo) = s.repository(id).cloned() else {
                            return (false, false);
                        };
                        if repo.github.as_ref() == Some(&fresh) {
                            return (false, false);
                        }
                        info!(id, permissions = ?fresh.permissions, "refreshed GitHub repository");
                        let associated = repo.github.is_none();
                        // GHD `RepositoriesStore.setGitHubRepository`
                        s.repositories_store().set_github_repository(&repo, fresh);
                        cx.notify();
                        (true, associated)
                    });
                    if associated {
                        Self::github_repository_associated(id, cx);
                    } else if changed {
                        Self::refresh_branch_protection(id, cx);
                    }
                }
                Err(corvene_github::GitHubError::Api { status: 404, .. }) if clear_lost => {
                    Self::state(cx).update(cx, |s, cx| {
                        let Some(repo) = s.repositories.iter_mut().find(|r| r.id == id) else {
                            return;
                        };
                        // only the record that was asked about
                        if repo.github.as_ref() != Some(&github) {
                            return;
                        }
                        info!(id, repo = %github.full_name(), "GitHub repository gone, association cleared");
                        repo.github = None;
                        crate::dispatcher::persist_repositories(s);
                        cx.notify();
                    });
                }
                Err(err) => warn!(id, %err, "could not refresh the GitHub repository"),
            },
        );
    }

    /// GHD `matchGitHubRepository` (`app-store.ts`): the GitHub repository
    /// the default remote names, on the signed-in accounts' hosts plus
    /// github.com when signed out (`331-github-without-account`).
    pub(crate) fn match_github_repository(
        s: &crate::state::AppState,
        info: &corvene_models::RepositoryInfo,
    ) -> Option<GitHubRepository> {
        let wiki_not_github = s.flags.bool(crate::flags::ids::WIKI_NOT_GITHUB);
        let dotcom = s.flags.bool(crate::flags::ids::GITHUB_WITHOUT_ACCOUNT);
        let hosts = corvene_models::github_hosts(&s.accounts, dotcom);
        corvene_git::find_default_remote(&info.remotes)
            .and_then(|r| corvene_models::github_from_remote(&r.url, &hosts))
            .filter(|gh| !(wiki_not_github && gh.is_wiki()))
    }

    /// `upsertGitHubRepositoryFromMatch` + `setGitHubRepository`, unless
    /// the repository got a record meanwhile.
    fn set_matched_github_repository(id: u64, github: GitHubRepository, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            let Some(repo) = s.repository(id).cloned() else {
                return;
            };
            if repo.github.is_some() {
                return;
            }
            info!(id, repo = %github.full_name(), "matched GitHub repository");
            s.repositories_store().set_github_repository(&repo, github);
            cx.notify();
        });
        Self::github_repository_associated(id, cx);
    }

    /// What selecting the repository skipped while it had no GitHub
    /// repository.
    fn github_repository_associated(id: u64, cx: &mut dyn Host) {
        Self::refresh_branch_protection(id, cx);
        Self::ensure_pull_requests(id, cx);
        Self::refresh_autolinks(id, cx);
        Self::load_issue_trackers(id, cx);
        Self::restart_pull_request_updater(cx);
    }

    /// With `fork-offer-respects-allow-forking`, a repository whose owner
    /// disabled forking is never offered a fork (GHD offers it and the fork
    /// request fails).
    pub fn fork_offer_blocked(s: &crate::state::AppState, gh: &GitHubRepository) -> bool {
        gh.forking_disabled()
            && s.flags
                .bool(crate::flags::ids::FORK_OFFER_RESPECTS_ALLOW_FORKING)
    }

    /// `_showCreateForkDialog`: only with an account for the repository.
    pub fn show_create_fork_dialog(id: u64, cx: &mut dyn Host) {
        let ok = {
            let s = Self::state(cx).read(cx);
            s.repository(id)
                .and_then(|r| r.github.as_ref())
                .is_some_and(|gh| {
                    s.account_for(&gh.endpoint).is_some() && !Self::fork_offer_blocked(s, gh)
                })
        };
        if ok {
            Self::show_popup(Popup::CreateFork { repo: id }, cx);
        }
    }

    /// `CreateForkDialog.onSubmit` + `_convertRepositoryToFork`: fork through
    /// the API, point `origin` at the fork, keep the original as `upstream`,
    /// then ask how the fork will be used. `then` gets the API error, if any.
    pub fn create_fork(
        id: u64,
        then: impl FnOnce(Option<String>, &mut dyn Host) + 'static,
        cx: &mut dyn Host,
    ) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            then(Some("The repository is not available.".into()), cx);
            return;
        };
        let (github, remote) = {
            let s = Self::state(cx).read(cx);
            (
                s.repository(id).and_then(|r| r.github.clone()),
                Self::current_remote_in(s, id),
            )
        };
        let (Some(github), Some(remote)) = (github, remote) else {
            then(Some("The repository has no GitHub remote.".into()), cx);
            return;
        };
        let Some((endpoint, token, _)) = Self::api_for_repository(id, &github, cx) else {
            then(Some("You are not signed in to GitHub.".into()), cx);
            return;
        };
        let original = github.clone();
        let (keep_ssh, sso_hint) = {
            let flags = &Self::state(cx).read(cx).flags;
            (
                flags.bool(crate::flags::ids::FORK_REMOTES_KEEP_SSH),
                flags.bool(crate::flags::ids::API_SAML_SSO_HINT),
            )
        };
        spawn_bg(
            cx,
            move || -> Result<GitHubRepository, String> {
                let client = Client::new(endpoint, token).with_sso_hint(sso_hint);
                let mut fork = client
                    .fork_repository(&original.owner, &original.name)
                    .map_err(|err| err.to_string())?;
                if fork.parent.is_none() {
                    fork.parent = Some(Box::new(original.clone()));
                }
                // `setRemoteURL(defaultRemote, fork.clone_url)` then
                // `ensureUpstreamRemoteURL(originalUrl)`
                let fork_url = if keep_ssh {
                    clone_url_like_remote(&fork.clone_url, &remote.url)
                } else {
                    fork.clone_url.clone()
                };
                corvene_git::set_remote_url(git.clone(), &workdir, &remote.name, &fork_url)
                    .map_err(|err| err.to_string())?;
                if let Err(err) = corvene_git::add_remote(
                    git.clone(),
                    &workdir,
                    UPSTREAM_REMOTE_NAME,
                    &remote.url,
                ) && let Err(err) = corvene_git::set_remote_url(
                    git.clone(),
                    &workdir,
                    UPSTREAM_REMOTE_NAME,
                    &remote.url,
                )
                .map_err(|_| err)
                {
                    warn!(%err, "could not set the upstream remote");
                }
                Ok(fork)
            },
            move |result, cx| match result {
                Ok(fork) => {
                    info!(id, fork = %fork.full_name(), "repository converted to a fork");
                    Self::state(cx).update(cx, |s, cx| {
                        if let Some(repo) = s.repositories.iter_mut().find(|r| r.id == id) {
                            repo.github = Some(fork);
                            repo.fork_contribution_target = None;
                        }
                        crate::dispatcher::persist_repositories(s);
                        cx.notify();
                    });
                    Self::refresh_repository(id, cx);
                    then(None, cx);
                    // GHD's `CreateForkDialog` closes, then asks for the settings
                    Self::close_popups_where(|p| matches!(p, Popup::CreateFork { .. }), cx);
                    Self::show_popup(Popup::ChooseForkSettings { repo: id }, cx);
                }
                Err(err) => then(Some(err), cx),
            },
        );
    }

    /// `_updateRepositoryWorkflowPreferences({ forkContributionTarget })`
    pub fn set_fork_contribution_target(
        id: u64,
        target: ForkContributionTarget,
        cx: &mut dyn Host,
    ) {
        Self::state(cx).update(cx, |s, cx| {
            if let Some(repo) = s.repositories.iter_mut().find(|r| r.id == id) {
                repo.fork_contribution_target = Some(target);
            }
            crate::dispatcher::persist_repositories(s);
            cx.notify();
        });
        // the pull request / issue target changed with it
        Self::ensure_pull_requests(id, cx);
        // `353-fork-tracks-upstream`
        Self::track_parent_default_branch(id, target == ForkContributionTarget::Parent, cx);
    }

    /// `353-fork-tracks-upstream`: point the fork's local default branch at
    /// the parent's (`to_parent`, after fetching the parent) or back at the
    /// fork's. See the module docs.
    pub(crate) fn track_parent_default_branch(id: u64, to_parent: bool, cx: &mut dyn Host) {
        let plan = {
            let s = Self::state(cx).read(cx);
            if !s.flags.bool(crate::flags::ids::FORK_TRACKS_UPSTREAM)
                || !s.flags.bool(crate::flags::ids::REMOTE_MANAGER)
            {
                return;
            }
            let Some(gh) = s.repository(id).and_then(|r| r.github.as_ref()) else {
                return;
            };
            let Some(parent) = gh.parent.as_deref() else {
                return;
            };
            let Some(info) = s.repo_states.get(&id).and_then(|rs| rs.info.as_ref()) else {
                return;
            };
            let Some(default_branch) = parent.default_branch.clone() else {
                return;
            };
            let Some(upstream) = find_upstream_remote(parent, &info.remotes) else {
                return;
            };
            let Some(origin) = crate::git_store::default_remote_name(info) else {
                return;
            };
            if origin == upstream.name {
                return;
            }
            let Some(local) = info
                .branches
                .iter()
                .find(|b| b.kind == BranchKind::Local && b.name == default_branch)
            else {
                return;
            };
            let (from, to) = (
                format!("{origin}/{default_branch}"),
                format!("{}/{default_branch}", upstream.name),
            );
            let (from, to) = if to_parent { (from, to) } else { (to, from) };
            if local.upstream_short() != Some(from.as_str()) {
                return;
            }
            (
                default_branch,
                origin.to_string(),
                upstream.name.clone(),
                to,
            )
        };
        let (branch, origin, upstream, to) = plan;
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let push_remote_key = format!("branch.{branch}.pushRemote");
        if !to_parent {
            spawn_bg(
                cx,
                move || -> Result<(), corvene_git::GitError> {
                    // only the configuration this feature wrote is undone
                    let ours =
                        corvene_git::local_config_value(git.clone(), &workdir, &push_remote_key)
                            .is_some_and(|remote| remote == origin);
                    if !ours {
                        return Ok(());
                    }
                    corvene_git::set_upstream(git.clone(), &workdir, &branch, &to)?;
                    corvene_git::remove_local_config_value(git, &workdir, &push_remote_key)
                },
                move |result, cx| {
                    if let Err(err) = result {
                        warn!(id, %err, "could not track the fork's default branch again");
                    }
                    Self::refresh_repository(id, cx);
                },
            );
            return;
        }
        Self::fetch_remote_then(
            id,
            Some(&upstream),
            false,
            move |fetched, cx| {
                if !fetched {
                    return;
                }
                let Some((git, workdir)) = Self::repo_context(id, cx) else {
                    return;
                };
                spawn_bg(
                    cx,
                    move || -> Result<bool, corvene_git::GitError> {
                        let local = format!("refs/heads/{branch}");
                        let tip = corvene_git::merge_base(git.clone(), &workdir, &local, &local)?;
                        let base = corvene_git::merge_base(
                            git.clone(),
                            &workdir,
                            &local,
                            &format!("refs/remotes/{to}"),
                        )?;
                        // commits the parent lacks stay with the fork
                        if tip.is_none() || base != tip {
                            return Ok(false);
                        }
                        corvene_git::set_local_config_value(
                            git.clone(),
                            &workdir,
                            &push_remote_key,
                            &origin,
                        )?;
                        corvene_git::set_upstream(git, &workdir, &branch, &to)?;
                        Ok(true)
                    },
                    move |result, cx| {
                        match result {
                            Ok(true) => info!(id, "default branch tracks the parent"),
                            Ok(false) => {}
                            Err(err) => {
                                warn!(id, %err, "could not track the parent's default branch")
                            }
                        }
                        Self::refresh_repository(id, cx);
                    },
                );
            },
            cx,
        );
    }

    /// `addUpstreamRemoteIfNeeded`: a fork gets an `upstream` remote for its
    /// parent unless its `upstream` remote already points there
    /// ([`find_upstream_remote`]; with `fork-remotes-keep-ssh`, also at the
    /// parent's SSH URL) or the name is taken by something else.
    pub(crate) fn add_upstream_remote_if_needed(id: u64, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        // `getIgnoreExistingUpstreamRemote`: the user picked Ignore once
        let ignored = Self::state(cx)
            .read(cx)
            .store
            .get::<bool>(&ignore_existing_upstream_key(id))
            .ok()
            .flatten()
            .unwrap_or(false);
        if ignored {
            return;
        }
        let (parent, parent_url, remotes) = {
            let s = Self::state(cx).read(cx);
            let Some(parent) = s
                .repository(id)
                .and_then(|r| r.github.as_ref())
                .and_then(|gh| gh.parent.as_ref())
            else {
                return;
            };
            let Some(rs) = s.repo_states.get(&id) else {
                return;
            };
            if rs.upstream_checked {
                return;
            }
            (
                (**parent).clone(),
                Self::parent_remote_url(s, id, parent),
                rs.info
                    .as_ref()
                    .map(|i| i.remotes.clone())
                    .unwrap_or_default(),
            )
        };
        Self::state(cx).update(cx, |s, _| s.repo_state_mut(id).upstream_checked = true);
        // `parent_url` is the SSH URL `fork-remotes-keep-ssh` set (an SSH
        // host alias would not match the parent's URLs)
        let points_at_parent = find_upstream_remote(&parent, &remotes).is_some()
            || remotes
                .iter()
                .any(|r| r.name == UPSTREAM_REMOTE_NAME && url_matches_remote(&r.url, &parent_url));
        if points_at_parent {
            return;
        }
        if let Some(existing) = remotes.iter().find(|r| r.name == UPSTREAM_REMOTE_NAME) {
            // `UpstreamAlreadyExistsError` → `upstreamAlreadyExistsHandler`
            Self::show_popup(
                Popup::UpstreamAlreadyExists {
                    repo: id,
                    existing_url: existing.url.clone(),
                },
                cx,
            );
            return;
        }
        spawn_bg(
            cx,
            move || corvene_git::add_remote(git, &workdir, UPSTREAM_REMOTE_NAME, &parent_url),
            move |result, cx| match result {
                Ok(()) => {
                    // `353-fork-tracks-upstream`: a cloned fork set up here
                    let to_parent = Self::state(cx)
                        .read(cx)
                        .repository(id)
                        .is_some_and(|r| r.is_fork_contributing_to_parent());
                    if to_parent {
                        Self::state(cx).update(cx, |s, _| {
                            s.repo_state_mut(id).track_parent_after_refresh = true;
                        });
                    }
                    Self::refresh_repository(id, cx);
                }
                Err(err) => warn!(id, %err, "could not add the upstream remote"),
            },
        );
    }

    /// `UpstreamAlreadyExists` › Update (`updateExistingUpstreamRemote`):
    /// point `upstream` at the parent's clone URL.
    pub fn update_existing_upstream_remote(id: u64, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let parent_url = {
            let s = Self::state(cx).read(cx);
            s.repository(id)
                .and_then(|r| r.github.as_ref())
                .and_then(|gh| gh.parent.as_ref())
                .map(|p| Self::parent_remote_url(s, id, p))
        };
        let Some(parent_url) = parent_url else {
            return;
        };
        spawn_bg(
            cx,
            move || corvene_git::set_remote_url(git, &workdir, UPSTREAM_REMOTE_NAME, &parent_url),
            move |result, cx| {
                if let Err(err) = result {
                    Self::show_error("Could not update the upstream remote", &err, cx);
                }
                Self::refresh_repository(id, cx);
            },
        );
    }

    /// The URL the `upstream` remote gets for `parent`: its `clone_url`, or,
    /// with `fork-remotes-keep-ssh`, the same repository over the current
    /// remote's SSH host when that remote is SSH.
    pub fn parent_remote_url(
        s: &crate::state::AppState,
        id: u64,
        parent: &GitHubRepository,
    ) -> String {
        match Self::current_remote_in(s, id) {
            Some(remote) if s.flags.bool(crate::flags::ids::FORK_REMOTES_KEEP_SSH) => {
                clone_url_like_remote(&parent.clone_url, &remote.url)
            }
            _ => parent.clone_url.clone(),
        }
    }

    /// `UpstreamAlreadyExists` › Ignore: never check this repository again.
    pub fn ignore_existing_upstream_remote(id: u64, cx: &mut dyn Host) {
        let result = Self::state(cx)
            .read(cx)
            .store
            .set(&ignore_existing_upstream_key(id), &true);
        if let Err(err) = result {
            warn!(id, %err, "could not remember to ignore the upstream remote");
        }
    }
}
