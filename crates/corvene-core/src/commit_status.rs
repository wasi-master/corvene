//! CI status of refs - GHD `lib/stores/commit-status-store.ts` +
//! `lib/ci-checks/ci-checks.ts`: statuses and check runs of a ref are
//! fetched on demand, cached for a minute and refreshed every 3 minutes
//! while something still shows them.
//!
//! Deviation: GHD components subscribe on mount and unsubscribe on
//! unmount. Views here "touch" a key while they render it; a key nobody
//! touched for five minutes stops being refreshed.
//!
//! Deviation (`334-branch-ci-status`): a current branch without a pull
//! request subscribes its pushed tip (on the GitHub repository its upstream
//! belongs to), so the toolbar can show its checks; GHD shows checks only
//! for pull requests (`ui/branches/ci-status.tsx` via `PullRequestBadge`).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::host::{AsyncCtx, Host};
use corvene_github::{ApiRefCheckRun, Client};
use corvene_models::{
    CheckConclusion, CheckStatus, CombinedRefCheck, GitHubRepository, JobStep, PullRequest,
    RefCheck, WorkflowRun, check_duration_ms, check_short_description,
};

use crate::dispatcher::Dispatcher;
use crate::remote::spawn_bg;
use crate::state::AppState;

/// `BackgroundRefreshInterval`
const BACKGROUND_REFRESH_INTERVAL: Duration = Duration::from_secs(3 * 60);
/// `entryIsEligibleForRefresh`: older than a minute.
const ENTRY_MAX_AGE: Duration = Duration::from_secs(60);
/// `MaxConcurrentFetches`
const MAX_CONCURRENT_FETCHES: usize = 6;
/// `QuickLRU({ maxSize: 250 })`
const MAX_ENTRIES: usize = 250;

pub struct CommitStatusEntry {
    pub check: Option<CombinedRefCheck>,
    pub fetched_at: Instant,
}

/// `IRefStatusSubscription`
#[derive(Clone, Debug)]
pub struct CommitStatusSubscription {
    pub api_base: String,
    pub owner: String,
    pub name: String,
    pub git_ref: String,
    /// Set for the current PR: check runs get their Actions workflow and
    /// job steps (`getCheckRunActionsWorkflowRuns`).
    pub branch_name: Option<String>,
    pub last_seen: Instant,
}

#[derive(Default)]
pub struct CommitStatusStore {
    pub entries: HashMap<String, CommitStatusEntry>,
    pub subscriptions: HashMap<String, CommitStatusSubscription>,
    pub(crate) in_flight: HashSet<String>,
}

impl CommitStatusStore {
    /// Store a fresh result, evicting the oldest entry when full.
    pub(crate) fn insert_entry(&mut self, key: String, check: Option<CombinedRefCheck>) {
        if !self.entries.contains_key(&key)
            && self.entries.len() >= MAX_ENTRIES
            && let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.fetched_at)
                .map(|(k, _)| k.clone())
        {
            self.entries.remove(&oldest);
        }
        self.entries.insert(
            key,
            CommitStatusEntry {
                check,
                fetched_at: Instant::now(),
            },
        );
    }

    /// A failed refresh: try again only after the usual minute.
    pub(crate) fn touch_entry(&mut self, key: &str) {
        if let Some(entry) = self.entries.get_mut(key) {
            entry.fetched_at = Instant::now();
        }
    }
}

/// `getCacheKey`
pub fn status_key(gh: &GitHubRepository, git_ref: &str) -> String {
    format!(
        "{}/repos/{}/{}/commits/{}",
        gh.endpoint,
        gh.owner.to_lowercase(),
        gh.name.to_lowercase(),
        git_ref
    )
}

/// `getChecksForRef` (statuses + the latest check run of each name);
/// `None` when neither call answered. Also reports an authentication failure.
fn fetch_ref_checks_inner(
    client: &Client,
    owner: &str,
    name: &str,
    git_ref: &str,
    all_check_runs: bool,
) -> (Option<Vec<RefCheck>>, bool) {
    let statuses = client.combined_ref_status(owner, name, git_ref);
    let check_runs = client.ref_check_runs(owner, name, git_ref, all_check_runs);
    let auth_failed = statuses.as_ref().is_err_and(|e| e.is_token_invalidated())
        || check_runs.as_ref().is_err_and(|e| e.is_token_invalidated());
    let statuses = statuses.ok().flatten();
    let check_runs = check_runs.ok().flatten();
    if statuses.is_none() && check_runs.is_none() {
        return (None, auth_failed);
    }
    let mut checks: Vec<RefCheck> = Vec::new();
    if let Some(statuses) = statuses {
        checks.extend(statuses.statuses.into_iter().map(status_to_check));
    }
    if let Some(runs) = check_runs {
        checks.extend(
            latest_check_runs(runs.check_runs)
                .into_iter()
                .map(check_run_to_check),
        );
    }
    (Some(checks), auth_failed)
}

/// The checks of `git_ref` for the notification path (`getChecksForRef`).
pub(crate) fn fetch_ref_checks(
    client: &Client,
    owner: &str,
    name: &str,
    git_ref: &str,
    all_check_runs: bool,
) -> Option<Vec<RefCheck>> {
    fetch_ref_checks_inner(client, owner, name, git_ref, all_check_runs).0
}

/// `apiStatusToRefCheck`
fn status_to_check(item: corvene_github::api::ApiRefStatusItem) -> RefCheck {
    let (status, conclusion) = match item.state.as_str() {
        "success" => (CheckStatus::Completed, Some(CheckConclusion::Success)),
        "pending" => (CheckStatus::InProgress, None),
        _ => (CheckStatus::Completed, Some(CheckConclusion::Failure)),
    };
    RefCheck {
        id: item.id,
        name: item.context,
        description: check_short_description(status, conclusion, None),
        status,
        conclusion,
        app_name: String::new(),
        html_url: item.target_url,
        check_suite_id: None,
        actions_workflow: None,
        job_steps: None,
    }
}

/// `apiCheckRunToRefCheck`
fn check_run_to_check(run: ApiRefCheckRun) -> RefCheck {
    let duration = check_duration_ms(run.started_at.as_deref(), run.completed_at.as_deref());
    RefCheck {
        id: run.id,
        name: run.name,
        description: check_short_description(run.status, run.conclusion, duration),
        status: run.status,
        conclusion: run.conclusion,
        app_name: run.app.map(|a| a.name).unwrap_or_default(),
        check_suite_id: run.check_suite.map(|s| s.id),
        html_url: run.html_url,
        actions_workflow: None,
        job_steps: None,
    }
}

/// `getLatestCheckRunsById`: the newest check suite per check run id, kept
/// apart for push and pull-request runs.
fn latest_check_runs(runs: Vec<ApiRefCheckRun>) -> Vec<ApiRefCheckRun> {
    let mut latest: HashMap<(u64, bool), ApiRefCheckRun> = HashMap::new();
    for run in runs {
        let key = (run.id, !run.pull_requests.is_empty());
        let suite = run.check_suite.as_ref().map(|s| s.id).unwrap_or(0);
        match latest.get(&key) {
            Some(current) if current.check_suite.as_ref().map(|s| s.id).unwrap_or(0) >= suite => {}
            _ => {
                latest.insert(key, run);
            }
        }
    }
    let mut out: Vec<ApiRefCheckRun> = latest.into_values().collect();
    out.sort_by_key(|r| r.id);
    out
}

/// `getCheckRunsGroupedByActionWorkflowNameAndEvent` + `getCheckRunGroupNames`:
/// groups named after the workflow (with the event when several events
/// ran), "Code scanning results", then "Other" last.
pub fn group_check_runs(checks: &[RefCheck]) -> Vec<(String, Vec<RefCheck>)> {
    let events: HashSet<&str> = checks
        .iter()
        .filter_map(|c| c.actions_workflow.as_ref())
        .map(|w| w.event.trim())
        .filter(|e| !e.is_empty())
        .collect();
    let multiple_events = events.len() > 1;
    let mut groups: Vec<(String, Vec<RefCheck>)> = Vec::new();
    for check in checks {
        let mut group = check
            .actions_workflow
            .as_ref()
            .map(|w| w.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "Other".to_string());
        if multiple_events
            && let Some(w) = &check.actions_workflow
            && !w.event.trim().is_empty()
        {
            group = format!("{group} ({})", w.event);
        }
        if group == "Other" && check.app_name == "GitHub Code Scanning" {
            group = "Code scanning results".to_string();
        }
        match groups.iter_mut().find(|(name, _)| *name == group) {
            Some((_, items)) => items.push(check.clone()),
            None => groups.push((group, vec![check.clone()])),
        }
    }
    groups.sort_by(|(a, _), (b, _)| match (a.as_str(), b.as_str()) {
        ("Other", "Other") => std::cmp::Ordering::Equal,
        ("Other", _) => std::cmp::Ordering::Greater,
        (_, "Other") => std::cmp::Ordering::Less,
        _ => a.to_lowercase().cmp(&b.to_lowercase()),
    });
    for (_, items) in &mut groups {
        items.sort_by_key(|c| c.name.to_lowercase());
    }
    groups
}

/// `getCombinedStatusSummary`: `2 successful, 1 failed checks`.
pub fn combined_status_summary(conclusions: &[Option<CheckConclusion>], noun: &str) -> String {
    let mut order: Vec<Option<CheckConclusion>> = Vec::new();
    let mut counts: HashMap<Option<CheckConclusion>, usize> = HashMap::new();
    for c in conclusions {
        if !counts.contains_key(c) {
            order.push(*c);
        }
        *counts.entry(*c).or_default() += 1;
    }
    let parts: Vec<String> = order
        .iter()
        .map(|c| {
            format!(
                "{} {}",
                counts[c],
                CheckConclusion::adjective(*c).to_lowercase()
            )
        })
        .collect();
    let sentence = match parts.len() {
        0 => String::new(),
        1 => parts[0].clone(),
        2 => format!("{} and {}", parts[0], parts[1]),
        n => format!("{}, and {}", parts[..n - 1].join(", "), parts[n - 1]),
    };
    let plural = if conclusions.len() > 1 {
        format!("{noun}s")
    } else {
        noun.to_string()
    };
    format!("{sentence} {plural}")
}

/// `manuallySetChecksToPending`
pub fn set_checks_to_pending(
    cached: &[RefCheck],
    pending: &[RefCheck],
) -> Option<CombinedRefCheck> {
    let updated: Vec<RefCheck> = cached
        .iter()
        .map(|check| {
            if !pending.iter().any(|p| p.id == check.id) {
                return check.clone();
            }
            let mut check = check.clone();
            check.status = CheckStatus::InProgress;
            check.conclusion = None;
            if let Some(steps) = check.job_steps.as_mut() {
                for step in steps {
                    step.status = CheckStatus::InProgress;
                    step.conclusion = None;
                }
            }
            check
        })
        .collect();
    CombinedRefCheck::from_checks(updated)
}

impl AppState {
    /// `tryGetStatus`
    pub fn commit_status(&self, gh: &GitHubRepository, git_ref: &str) -> Option<&CombinedRefCheck> {
        self.commit_statuses
            .entries
            .get(&status_key(gh, git_ref))?
            .check
            .as_ref()
    }

    /// `334-branch-ci-status`: the GitHub repository and commit whose checks
    /// the toolbar shows for the current branch when it has no pull
    /// request: its upstream's tip, on the repository (or the fork parent)
    /// that upstream's remote points at.
    pub fn branch_ci_ref(&self, id: u64) -> Option<(GitHubRepository, String)> {
        if !self.flags.bool(crate::flags::ids::BRANCH_CI_STATUS)
            || self.current_pull_request(id).is_some()
        {
            return None;
        }
        // flags 342-344: a repository on GitLab, Gitea or Bitbucket too
        let hosted;
        let gh = match self.repository(id)?.github.as_ref() {
            Some(gh) => gh,
            None => {
                hosted = self.hosted_repository(id)?.repo;
                &hosted
            }
        };
        let info = self.repo_states.get(&id)?.info.as_ref()?;
        crate::menu_bar_status::ci_ref_of(info, gh)
    }

    /// [`Self::branch_ci_ref`]'s status icon, once it has checks.
    pub fn branch_ci_summary(&self, id: u64) -> Option<(CheckStatus, Option<CheckConclusion>)> {
        let (gh, sha) = self.branch_ci_ref(id)?;
        let check = self.commit_status(&gh, &sha)?;
        (!check.checks.is_empty()).then_some((check.status, check.conclusion))
    }

    /// The status icon of a pull request (its base repository + head ref).
    pub fn commit_status_summary(
        &self,
        pr: &PullRequest,
    ) -> Option<(CheckStatus, Option<CheckConclusion>)> {
        let base = pr.base.repository.as_ref()?;
        let check = self.commit_status(base, &pr.commit_ref())?;
        (!check.checks.is_empty()).then_some((check.status, check.conclusion))
    }
}

impl Dispatcher {
    /// `subscribe`: remember that `git_ref` of `gh` is on screen and fetch
    /// its status when the cache has nothing fresh. Never notifies, so it
    /// can be called while rendering.
    pub fn touch_commit_status(
        gh: &GitHubRepository,
        git_ref: &str,
        branch_name: Option<String>,
        cx: &mut dyn Host,
    ) {
        let key = status_key(gh, git_ref);
        let needs_fetch = Self::state(cx).update(cx, |s, _| {
            let store = &mut s.commit_statuses;
            let branch_changed = match store.subscriptions.get_mut(&key) {
                Some(sub) => {
                    sub.last_seen = Instant::now();
                    if branch_name.is_some() && sub.branch_name != branch_name {
                        sub.branch_name = branch_name.clone();
                        true
                    } else {
                        false
                    }
                }
                None => {
                    store.subscriptions.insert(
                        key.clone(),
                        CommitStatusSubscription {
                            api_base: gh.endpoint.clone(),
                            owner: gh.owner.clone(),
                            name: gh.name.clone(),
                            git_ref: git_ref.to_string(),
                            branch_name: branch_name.clone(),
                            last_seen: Instant::now(),
                        },
                    );
                    false
                }
            };
            branch_changed
                || store
                    .entries
                    .get(&key)
                    .is_none_or(|e| e.fetched_at.elapsed() > ENTRY_MAX_AGE)
        });
        if needs_fetch {
            Self::refresh_commit_status(key, cx);
        }
    }

    /// The current branch's pull request keeps its status subscribed with
    /// the branch name so the popover can show Actions job steps.
    pub fn subscribe_current_pull_request_status(id: u64, cx: &mut dyn Host) {
        let target = {
            let s = Self::state(cx).read(cx);
            s.current_pull_request(id).and_then(|pr| {
                let base = pr.base.repository.clone()?;
                let branch = s
                    .repo_states
                    .get(&id)
                    .and_then(|rs| rs.info.as_ref())
                    .and_then(|i| i.current_branch())
                    .map(|b| {
                        b.upstream_short()
                            .and_then(|u| u.split_once('/').map(|(_, n)| n.to_string()))
                            .unwrap_or_else(|| b.name.clone())
                    });
                Some((base, pr.commit_ref(), branch))
            })
        };
        if let Some((base, git_ref, branch)) = target {
            Self::touch_commit_status(&base, &git_ref, branch, cx);
            return;
        }
        // `334-branch-ci-status`: no pull request, the branch's pushed tip
        let branch = {
            let s = Self::state(cx).read(cx);
            s.branch_ci_ref(id).map(|(gh, sha)| {
                let name = s
                    .repo_states
                    .get(&id)
                    .and_then(|rs| rs.info.as_ref())
                    .and_then(|i| i.current_branch())
                    .and_then(|b| b.upstream_without_remote().map(str::to_string));
                (gh, sha, name)
            })
        };
        if let Some((gh, sha, name)) = branch {
            Self::touch_commit_status(&gh, &sha, name, cx);
        }
    }

    /// `startBackgroundRefresh`: refresh the eligible subscriptions every
    /// 3 minutes.
    pub fn start_commit_status_refresh(cx: &mut dyn Host) {
        cx.spawn(async move |cx: &mut AsyncCtx| {
            loop {
                cx.background_executor()
                    .timer(BACKGROUND_REFRESH_INTERVAL)
                    .await;
                if crate::pull_requests::android_in_background() {
                    continue;
                }
                cx.update(Self::refresh_eligible_commit_statuses);
            }
        })
        .detach();
    }

    /// `refreshEligibleSubscriptions`
    fn refresh_eligible_commit_statuses(cx: &mut dyn Host) {
        // `428-menu-bar-status-item`: the watched repositories' refs stay
        // subscribed through this loop (nobody renders them)
        Self::touch_menu_bar_statuses(cx);
        let keys: Vec<String> = Self::state(cx).update(cx, |s, _| {
            // `308-ci-status-idle-minutes`: a key nobody rendered for this
            // long stops refreshing (0: never, as GHD's mount / unmount)
            let idle_minutes = s.flags.number(crate::flags::ids::CI_STATUS_IDLE_MINUTES);
            let store = &mut s.commit_statuses;
            if idle_minutes > 0 {
                let max_idle = Duration::from_secs(idle_minutes as u64 * 60);
                store
                    .subscriptions
                    .retain(|_, sub| sub.last_seen.elapsed() < max_idle);
            }
            store
                .subscriptions
                .keys()
                .filter(|key| {
                    !store.in_flight.contains(*key)
                        && store
                            .entries
                            .get(*key)
                            .is_none_or(|e| e.fetched_at.elapsed() > ENTRY_MAX_AGE)
                })
                .cloned()
                .collect()
        });
        for key in keys {
            Self::refresh_commit_status(key, cx);
        }
    }

    /// `refreshSubscription`: statuses + check runs, then (for the
    /// current PR) the Actions workflows and job steps.
    fn refresh_commit_status(key: String, cx: &mut dyn Host) {
        let Some(sub) = Self::state(cx).update(cx, |s, _| {
            let store = &mut s.commit_statuses;
            if store.in_flight.contains(&key) || store.in_flight.len() >= MAX_CONCURRENT_FETCHES {
                return None;
            }
            let sub = store.subscriptions.get(&key)?.clone();
            store.in_flight.insert(key.clone());
            Some(sub)
        }) else {
            return;
        };
        // flags 342-344: a subscription on GitLab, Gitea or Bitbucket
        if Self::state(cx)
            .read(cx)
            .host_kind_of(&sub.api_base)
            .is_some()
        {
            return Self::refresh_host_commit_status(key, sub, cx);
        }
        let gh = GitHubRepository {
            endpoint: sub.api_base.clone(),
            owner: sub.owner.clone(),
            name: sub.name.clone(),
            html_url: String::new(),
            clone_url: String::new(),
            default_branch: None,
            private: false,
            fork: false,
            parent: None,
            archived: false,
            permissions: None,
            allow_forking: None,
            node_id: None,
        };
        let Some((endpoint, token, login)) = Self::api_for(&gh, cx) else {
            Self::state(cx).update(cx, |s, _| {
                s.commit_statuses.in_flight.remove(&key);
            });
            return;
        };
        let previous: Vec<RefCheck> = Self::state(cx)
            .read(cx)
            .commit_statuses
            .entries
            .get(&key)
            .and_then(|e| e.check.as_ref())
            .map(|c| c.checks.clone())
            .unwrap_or_default();
        let api_base = sub.api_base.clone();
        let all_check_runs = Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::ALL_CHECK_RUN_PAGES);
        let job_fallback = Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::ACTIONS_JOB_LOGS);
        spawn_bg(
            cx,
            move || {
                let client = Client::new(endpoint, token);
                let (owner, name, git_ref) = (&sub.owner, &sub.name, &sub.git_ref);
                let (checks, auth_failed) =
                    fetch_ref_checks_inner(&client, owner, name, git_ref, all_check_runs);
                let Some(mut checks) = checks else {
                    return (None, auth_failed, false);
                };
                if sub.branch_name.is_some() {
                    checks = with_actions_workflows(
                        &client,
                        owner,
                        name,
                        checks,
                        &previous,
                        job_fallback,
                    );
                }
                (CombinedRefCheck::from_checks(checks), auth_failed, true)
            },
            move |(check, auth_failed, changed), cx| {
                Self::state(cx).update(cx, |s, cx| {
                    let store = &mut s.commit_statuses;
                    store.in_flight.remove(&key);
                    if changed {
                        if store.entries.len() >= MAX_ENTRIES
                            && let Some(oldest) = store
                                .entries
                                .iter()
                                .min_by_key(|(_, e)| e.fetched_at)
                                .map(|(k, _)| k.clone())
                        {
                            store.entries.remove(&oldest);
                        }
                        store.entries.insert(
                            key.clone(),
                            CommitStatusEntry {
                                check,
                                fetched_at: Instant::now(),
                            },
                        );
                    } else if let Some(entry) = store.entries.get_mut(&key) {
                        entry.fetched_at = Instant::now();
                    }
                    cx.notify();
                });
                if auth_failed {
                    Self::token_invalidated(&api_base, &login, cx);
                }
            },
        );
    }

    /// `manualRefreshSubscription`: after a re-run request, show the checks
    /// as pending until the next refresh.
    pub fn set_commit_status_pending(
        gh: &GitHubRepository,
        git_ref: &str,
        pending: Vec<RefCheck>,
        cx: &mut dyn Host,
    ) {
        let key = status_key(gh, git_ref);
        Self::state(cx).update(cx, |s, cx| {
            let store = &mut s.commit_statuses;
            let Some(cached) = store.entries.get(&key).and_then(|e| e.check.clone()) else {
                return;
            };
            store.entries.insert(
                key,
                CommitStatusEntry {
                    check: set_checks_to_pending(&cached.checks, &pending),
                    fetched_at: Instant::now(),
                },
            );
            cx.notify();
        });
    }

    /// `setShowCIStatusPopover`
    pub fn set_show_ci_status_popover(show: bool, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            if s.show_ci_status_popover != show {
                s.show_ci_status_popover = show;
                cx.notify();
            }
        });
    }

    /// `rerequestCheckSuites`: one job → rerun it; otherwise the failed
    /// jobs of each workflow run (failed only) or the whole check suites.
    /// `then` runs on the main thread with whether every request succeeded.
    pub fn rerequest_check_suites(
        gh: GitHubRepository,
        checks: Vec<RefCheck>,
        failed_only: bool,
        then: impl FnOnce(bool, &mut dyn Host) + 'static,
        cx: &mut dyn Host,
    ) {
        let Some((endpoint, token, _)) = Self::api_for(&gh, cx) else {
            then(false, cx);
            return;
        };
        spawn_bg(
            cx,
            move || {
                let client = Client::new(endpoint, token);
                let (owner, name) = (gh.owner.as_str(), gh.name.as_str());
                if checks.len() == 1 && checks[0].actions_workflow.is_some() {
                    return client.rerun_job(owner, name, checks[0].id).unwrap_or(false);
                }
                let mut ok = true;
                let mut suites: HashSet<u64> = HashSet::new();
                let mut runs: HashSet<u64> = HashSet::new();
                for check in &checks {
                    if failed_only && let Some(w) = &check.actions_workflow {
                        runs.insert(w.id);
                        continue;
                    }
                    if let Some(id) = check.check_suite_id {
                        suites.insert(id);
                    }
                }
                for id in runs {
                    ok &= client.rerun_failed_jobs(owner, name, id).unwrap_or(false);
                }
                for id in suites {
                    ok &= client
                        .rerequest_check_suite(owner, name, id)
                        .unwrap_or(false);
                }
                ok
            },
            then,
        );
    }

    /// `CICheckRunRerunDialog.determineRerunnability`: which checks belong
    /// to a completed, re-requestable check suite younger than a month.
    pub fn determine_rerunnable_checks(
        gh: GitHubRepository,
        checks: Vec<RefCheck>,
        then: impl FnOnce(Vec<RefCheck>, Vec<RefCheck>, &mut dyn Host) + 'static,
        cx: &mut dyn Host,
    ) {
        let Some((endpoint, token, _)) = Self::api_for(&gh, cx) else {
            then(Vec::new(), checks, cx);
            return;
        };
        spawn_bg(
            cx,
            move || {
                let client = Client::new(endpoint, token);
                let ids: HashSet<u64> = checks.iter().filter_map(|c| c.check_suite_id).collect();
                // `offsetFromNow(-30, 'days')`
                let month_ago = crate::offset_from::offset_from(
                    std::time::SystemTime::now(),
                    -30,
                    crate::offset_from::Unit::Days,
                );
                let mut rerequestable: HashSet<u64> = HashSet::new();
                for id in ids {
                    if let Ok(Some(suite)) = client.check_suite(&gh.owner, &gh.name, id)
                        && suite.rerequestable
                        && suite.status == CheckStatus::Completed
                        && corvene_models::parse_iso8601(&suite.created_at)
                            .is_some_and(|t| t > month_ago)
                    {
                        rerequestable.insert(suite.id);
                    }
                }
                let (rerunnable, non_rerunnable): (Vec<RefCheck>, Vec<RefCheck>) =
                    checks.into_iter().partition(|c| {
                        c.check_suite_id
                            .is_some_and(|id| rerequestable.contains(&id))
                    });
                (rerunnable, non_rerunnable)
            },
            move |(rerunnable, non_rerunnable), cx| then(rerunnable, non_rerunnable, cx),
        );
    }
}

/// `getCheckRunActionsWorkflowRuns` (by check suite id, the dotcom path) +
/// `getLatestPRWorkflowRunsLogsForCheckRun`: attach the Actions workflow run
/// and job steps to each check run. The previous mapping is reused when
/// the set of check runs did not change (`getAndMapActionWorkflowRunsToCheckRuns`).
///
/// Deviation (`347-actions-job-logs`, `job_fallback`): GHD reads only the
/// first page of the run's latest attempt, so a job past it or from an
/// earlier attempt showed "There are no steps to display"; Corvene then
/// reads the one job.
fn with_actions_workflows(
    client: &Client,
    owner: &str,
    name: &str,
    checks: Vec<RefCheck>,
    previous: &[RefCheck],
    job_fallback: bool,
) -> Vec<RefCheck> {
    let same_set = !previous.is_empty()
        && previous.iter().any(|c| c.actions_workflow.is_some())
        && previous.len() == checks.len()
        && checks.iter().all(|c| previous.iter().any(|p| p.id == c.id));
    if same_set {
        return checks
            .into_iter()
            .map(|mut c| {
                if let Some(prev) = previous.iter().find(|p| p.id == c.id) {
                    c.actions_workflow = prev.actions_workflow.clone();
                    c.job_steps = prev.job_steps.clone();
                }
                c
            })
            .collect();
    }
    let mut suites: HashMap<u64, Option<WorkflowRun>> = HashMap::new();
    let mut jobs_cache: HashMap<u64, Option<corvene_github::api::ApiWorkflowJobs>> = HashMap::new();
    checks
        .into_iter()
        .map(|mut check| {
            let Some(suite_id) = check.check_suite_id else {
                return check;
            };
            let run = suites
                .entry(suite_id)
                .or_insert_with(|| {
                    client
                        .workflow_run_by_check_suite(owner, name, suite_id)
                        .ok()
                        .flatten()
                        .map(|r| WorkflowRun {
                            id: r.id,
                            workflow_id: r.workflow_id,
                            name: r.name,
                            event: r.event,
                            check_suite_id: r.check_suite_id,
                            created_at: r.created_at,
                        })
                })
                .clone();
            let Some(run) = run else {
                return check;
            };
            let jobs = jobs_cache
                .entry(run.id)
                .or_insert_with(|| client.workflow_run_jobs(owner, name, run.id).ok().flatten());
            let fallback;
            let mut job = jobs
                .as_ref()
                .and_then(|j| j.jobs.iter().find(|j| j.id == check.id));
            if job.is_none() && job_fallback {
                fallback = client.workflow_job(owner, name, check.id).ok().flatten();
                job = fallback.as_ref();
            }
            if let Some(job) = job {
                if job.html_url.is_some() {
                    check.html_url = job.html_url.clone();
                }
                check.job_steps = Some(
                    job.steps
                        .iter()
                        .map(|s| JobStep {
                            name: s.name.clone(),
                            number: s.number,
                            status: s.status,
                            conclusion: s.conclusion,
                            started_at: s.started_at.clone(),
                            completed_at: s.completed_at.clone(),
                        })
                        .collect(),
                );
            }
            check.actions_workflow = Some(run);
            check
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(id: u64, name: &str, workflow: Option<(&str, &str)>) -> RefCheck {
        RefCheck {
            id,
            name: name.into(),
            description: String::new(),
            status: CheckStatus::Completed,
            conclusion: Some(CheckConclusion::Success),
            app_name: String::new(),
            html_url: None,
            check_suite_id: Some(1),
            actions_workflow: workflow.map(|(name, event)| WorkflowRun {
                id: 1,
                workflow_id: 1,
                name: name.into(),
                event: event.into(),
                check_suite_id: Some(1),
                created_at: String::new(),
            }),
            job_steps: None,
        }
    }

    #[test]
    fn groups_by_workflow_with_other_last() {
        let groups = group_check_runs(&[
            check(1, "lint", None),
            check(2, "build", Some(("CI", "push"))),
            check(3, "test", Some(("CI", "pull_request"))),
        ]);
        let names: Vec<&str> = groups.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["CI (pull_request)", "CI (push)", "Other"]);
    }

    #[test]
    fn summarises_conclusions() {
        assert_eq!(
            combined_status_summary(
                &[
                    Some(CheckConclusion::Success),
                    Some(CheckConclusion::Success),
                    Some(CheckConclusion::Failure)
                ],
                "check"
            ),
            "2 successful and 1 failed checks"
        );
        assert_eq!(
            combined_status_summary(&[None], "step"),
            "1 in progress step"
        );
    }

    #[test]
    fn pending_override_clears_conclusions() {
        let cached = vec![check(1, "a", None), check(2, "b", None)];
        let combined = set_checks_to_pending(&cached, &cached[..1]).unwrap();
        assert_eq!(combined.checks[0].conclusion, None);
        assert_eq!(
            combined.checks[1].conclusion,
            Some(CheckConclusion::Success)
        );
        assert_eq!(combined.status, CheckStatus::InProgress);
    }
}
