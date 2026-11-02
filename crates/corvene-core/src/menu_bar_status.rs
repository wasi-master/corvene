//! The menu bar status item (Corvene addition, flag
//! `428-menu-bar-status-item`, macOS): for each watched repository, the
//! current branch's ahead/behind counts and the latest check status of its
//! pushed tip, in a menu that opens a repository or a failing check. GHD
//! has no status item; its Dock icon shows nothing either.
//!
//! The data is what the app already refreshes: the repository indicators
//! pass ([`crate::remote`], every 15 minutes and on the sidebar's prompts)
//! also records each watched repository's branch, ahead/behind and CI ref
//! ([`WatchedRepoStatus`]), and the commit status store's 3 minute loop
//! keeps those refs subscribed ([`Dispatcher::touch_menu_bar_statuses`]).
//! No loop of its own. The selected repository's own state is fresher and
//! wins when it is loaded.
//!
//! Which repositories are watched is `Settings::menu_bar_repositories`
//! (Settings › Advanced › Menu bar). [`MenuBarModel`] is what the item
//! shows, pure and compared by value so the AppKit side only rebuilds on a
//! change ([`AppState::menu_bar_model`]).

use std::collections::HashMap;

use corvene_models::{
    AheadBehind, CheckConclusion, CheckStatus, GitHubRepository, RefCheck, RepositoryInfo,
    url_matches_remote,
};

use crate::dispatcher::Dispatcher;
use crate::host::Host;
use crate::state::AppState;

/// What the indicator pass records for a watched repository.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WatchedRepoStatus {
    pub branch: Option<String>,
    pub ahead_behind: Option<AheadBehind>,
    /// The GitHub repository and the pushed tip whose checks to show.
    pub ci_ref: Option<(GitHubRepository, String)>,
}

/// The CI ref of `info`'s current branch on `gh` (or its parent): the
/// upstream's tip on the remote the upstream belongs to, when that remote
/// is `gh`'s clone URL (`AppState::branch_ci_ref` for the selected
/// repository, and the indicator pass for watched ones).
pub fn ci_ref_of(
    info: &RepositoryInfo,
    gh: &GitHubRepository,
) -> Option<(GitHubRepository, String)> {
    let branch = info.current_branch()?;
    let upstream = branch.upstream_short()?;
    let remote_name = branch.upstream_remote_name()?;
    let remote = info.remotes.iter().find(|r| r.name == remote_name)?;
    let target = std::iter::once(gh)
        .chain(gh.parent.as_deref())
        .find(|g| url_matches_remote(&remote.url, &g.clone_url))?;
    let tip = info
        .branches
        .iter()
        .find(|b| b.kind == corvene_models::BranchKind::Remote && b.name == upstream)?
        .tip
        .clone()?;
    Some((target.clone(), tip))
}

/// A failing check of a watched repository's tip.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct FailingCheck {
    pub name: String,
    pub url: Option<String>,
}

/// The checks of a watched repository's tip, summarised.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct CheckSummary {
    pub status: CheckStatus,
    pub conclusion: Option<CheckConclusion>,
    pub total: usize,
    pub failed: usize,
    pub pending: usize,
    pub failing: Vec<FailingCheck>,
}

impl CheckSummary {
    fn of(checks: &[RefCheck]) -> Option<Self> {
        if checks.is_empty() {
            return None;
        }
        let combined = corvene_models::CombinedRefCheck::from_checks(checks.to_vec())?;
        let failing: Vec<FailingCheck> = checks
            .iter()
            .filter(|c| c.conclusion.is_some_and(|c| c.is_failing()))
            .map(|c| FailingCheck {
                name: c.name.clone(),
                url: c.html_url.clone(),
            })
            .collect();
        Some(Self {
            status: combined.status,
            conclusion: combined.conclusion,
            total: checks.len(),
            failed: failing.len(),
            pending: checks.iter().filter(|c| c.conclusion.is_none()).count(),
            failing,
        })
    }

    /// The menu's line for the checks.
    pub fn label(&self) -> String {
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        if self.failed > 0 {
            format!(
                "{} of {} check{} failed",
                self.failed,
                self.total,
                plural(self.total)
            )
        } else if self.pending > 0 {
            format!("{} check{} pending", self.pending, plural(self.pending))
        } else if self.conclusion.is_some_and(|c| {
            matches!(
                c,
                CheckConclusion::Success | CheckConclusion::Neutral | CheckConclusion::Skipped
            )
        }) {
            format!("All {} check{} passed", self.total, plural(self.total))
        } else {
            format!(
                "{} check{} {}",
                self.total,
                plural(self.total),
                CheckConclusion::adjective(self.conclusion).to_lowercase()
            )
        }
    }
}

/// One watched repository as the status item shows it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct MenuBarRepo {
    pub id: u64,
    pub name: String,
    pub branch: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// `None`: no ahead/behind (no upstream or not read yet).
    pub has_upstream: bool,
    /// `None`: no checks known (no GitHub repository, no pushed tip, or not
    /// fetched yet).
    pub checks: Option<CheckSummary>,
}

impl MenuBarRepo {
    /// The menu's line for the branch and its sync state.
    pub fn sync_label(&self) -> String {
        let branch = self.branch.as_deref().unwrap_or("detached");
        if !self.has_upstream {
            return format!("{branch} · no upstream");
        }
        match (self.ahead, self.behind) {
            (0, 0) => format!("{branch} · up to date"),
            (a, 0) => format!("{branch} · {a} ahead"),
            (0, b) => format!("{branch} · {b} behind"),
            (a, b) => format!("{branch} · {a} ahead, {b} behind"),
        }
    }
}

/// The worst state over the watched repositories, for the item's icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverallState {
    /// A check failed somewhere.
    Failing,
    /// Checks still running somewhere, none failed.
    Pending,
    /// Every known check passed.
    Passing,
    /// No checks known.
    NoChecks,
}

/// What the status item shows.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct MenuBarModel {
    pub repos: Vec<MenuBarRepo>,
}

impl MenuBarModel {
    pub fn overall(&self) -> OverallState {
        let mut state = OverallState::NoChecks;
        for repo in &self.repos {
            let Some(checks) = &repo.checks else { continue };
            let this = if checks.failed > 0 {
                OverallState::Failing
            } else if checks.pending > 0 || checks.conclusion.is_none() {
                OverallState::Pending
            } else {
                OverallState::Passing
            };
            state = match (state, this) {
                (OverallState::Failing, _) | (_, OverallState::Failing) => OverallState::Failing,
                (OverallState::Pending, _) | (_, OverallState::Pending) => OverallState::Pending,
                _ => OverallState::Passing,
            };
        }
        state
    }

    /// The item's title: the summed ahead/behind counts (`↑3 ↓1`), empty
    /// when everything is in sync.
    pub fn title(&self) -> String {
        let ahead: u32 = self.repos.iter().map(|r| r.ahead).sum();
        let behind: u32 = self.repos.iter().map(|r| r.behind).sum();
        let mut parts = Vec::new();
        if ahead > 0 {
            parts.push(format!("↑{ahead}"));
        }
        if behind > 0 {
            parts.push(format!("↓{behind}"));
        }
        parts.join(" ")
    }

    /// The item's accessible description.
    pub fn description(&self) -> String {
        let n = self.repos.len();
        let state = match self.overall() {
            OverallState::Failing => "checks failing",
            OverallState::Pending => "checks pending",
            OverallState::Passing => "checks passing",
            OverallState::NoChecks => "no checks",
        };
        format!(
            "Corvene: {n} watched repositor{}, {state}",
            if n == 1 { "y" } else { "ies" }
        )
    }
}

impl AppState {
    /// `428-menu-bar-status-item` is on and something is watched.
    pub fn menu_bar_enabled(&self) -> bool {
        cfg!(target_os = "macos")
            && self.flags.bool(crate::flags::ids::MENU_BAR_STATUS_ITEM)
            && !self.settings.menu_bar_repositories.is_empty()
    }

    /// The watched repositories in the sidebar's order (ids the settings
    /// name that are no longer added are skipped).
    pub fn watched_repositories(&self) -> Vec<u64> {
        self.repositories
            .iter()
            .filter(|r| self.settings.menu_bar_repositories.contains(&r.id))
            .map(|r| r.id)
            .collect()
    }

    /// The CI ref of a watched repository: the loaded state's when it is
    /// loaded (fresh after a push), else what the indicator pass recorded.
    pub fn menu_bar_ci_ref(&self, id: u64) -> Option<(GitHubRepository, String)> {
        let repo = self.repository(id)?;
        if let Some(info) = self.repo_states.get(&id).and_then(|rs| rs.info.as_ref())
            && let Some(gh) = repo.github.as_ref()
        {
            return ci_ref_of(info, gh);
        }
        self.menu_bar_statuses
            .get(&id)
            .and_then(|w| w.ci_ref.clone())
    }

    /// What the status item shows, `None` while it is off.
    pub fn menu_bar_model(&self) -> Option<MenuBarModel> {
        if !self.menu_bar_enabled() {
            return None;
        }
        Some(MenuBarModel {
            repos: self.watched_repositories_model(),
        })
    }

    /// The watched repositories as the status item and the widgets
    /// (`433-macos-widgets`) show them.
    pub fn watched_repositories_model(&self) -> Vec<MenuBarRepo> {
        self.watched_repositories()
            .into_iter()
            .filter_map(|id| {
                let repo = self.repository(id)?;
                let loaded = self.repo_states.get(&id).and_then(|rs| rs.info.as_ref());
                let watched = self.menu_bar_statuses.get(&id);
                let branch = loaded
                    .and_then(|i| i.current_branch().map(|b| b.name.clone()))
                    .or_else(|| watched.and_then(|w| w.branch.clone()))
                    .or_else(|| self.indicators.get(&id).and_then(|i| i.branch.clone()));
                let ahead_behind = self
                    .repo_states
                    .get(&id)
                    .filter(|rs| rs.info.is_some())
                    .and_then(|rs| rs.ahead_behind)
                    .or_else(|| watched.and_then(|w| w.ahead_behind))
                    .or_else(|| self.indicators.get(&id).and_then(|i| i.ahead_behind));
                let checks = self
                    .menu_bar_ci_ref(id)
                    .and_then(|(gh, sha)| self.commit_status(&gh, &sha).cloned())
                    .and_then(|c| CheckSummary::of(&c.checks));
                Some(MenuBarRepo {
                    id,
                    name: repo.name(),
                    branch,
                    ahead: ahead_behind.map(|ab| ab.ahead).unwrap_or(0),
                    behind: ahead_behind.map(|ab| ab.behind).unwrap_or(0),
                    has_upstream: ahead_behind.is_some(),
                    checks,
                })
            })
            .collect()
    }
}

/// `AppState::menu_bar_statuses`
pub type MenuBarStatuses = HashMap<u64, WatchedRepoStatus>;

impl Dispatcher {
    /// Settings › Advanced › Menu bar: watch or stop watching `id`.
    pub fn set_menu_bar_repository(id: u64, watched: bool, cx: &mut dyn Host) {
        Self::update_settings(cx, |s| {
            s.menu_bar_repositories.retain(|&r| r != id);
            if watched {
                s.menu_bar_repositories.push(id);
            }
        });
        if watched {
            // the indicator pass records the new repository's status
            Self::refresh_indicators_if_stale(cx);
        }
        Self::touch_menu_bar_statuses(cx);
    }

    /// Keep the watched repositories' CI refs subscribed in the commit
    /// status store (run by its refresh loop and after the indicator pass),
    /// and fetch any that are not there yet.
    pub fn touch_menu_bar_statuses(cx: &mut dyn Host) {
        let refs: Vec<(GitHubRepository, String)> = {
            let s = Self::state(cx).read(cx);
            if !s.menu_bar_enabled() && !s.widgets_enabled() {
                return;
            }
            s.watched_repositories()
                .into_iter()
                .filter_map(|id| s.menu_bar_ci_ref(id))
                .collect()
        };
        for (gh, sha) in refs {
            Self::touch_commit_status(&gh, &sha, None, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(name: &str, conclusion: Option<CheckConclusion>) -> RefCheck {
        RefCheck {
            id: 1,
            name: name.into(),
            description: String::new(),
            status: if conclusion.is_some() {
                CheckStatus::Completed
            } else {
                CheckStatus::InProgress
            },
            conclusion,
            app_name: String::new(),
            html_url: Some(format!("https://github.com/o/r/actions/runs/1/job/{name}")),
            check_suite_id: None,
            actions_workflow: None,
            job_steps: None,
        }
    }

    fn repo(name: &str, ahead: u32, behind: u32, checks: Option<CheckSummary>) -> MenuBarRepo {
        MenuBarRepo {
            id: 1,
            name: name.into(),
            branch: Some("main".into()),
            ahead,
            behind,
            has_upstream: true,
            checks,
        }
    }

    #[test]
    fn summarises_checks() {
        let s = CheckSummary::of(&[
            check("a", Some(CheckConclusion::Success)),
            check("b", Some(CheckConclusion::Failure)),
            check("c", None),
        ])
        .unwrap();
        assert_eq!(s.total, 3);
        assert_eq!(s.failed, 1);
        assert_eq!(s.pending, 1);
        assert_eq!(s.failing[0].name, "b");
        assert_eq!(s.label(), "1 of 3 checks failed");
        let s = CheckSummary::of(&[check("a", Some(CheckConclusion::Success))]).unwrap();
        assert_eq!(s.label(), "All 1 check passed");
        let s = CheckSummary::of(&[check("a", None), check("b", None)]).unwrap();
        assert_eq!(s.label(), "2 checks pending");
        assert!(CheckSummary::of(&[]).is_none());
    }

    #[test]
    fn overall_state_and_title() {
        let passing = CheckSummary::of(&[check("a", Some(CheckConclusion::Success))]);
        let failing = CheckSummary::of(&[check("a", Some(CheckConclusion::Failure))]);
        let pending = CheckSummary::of(&[check("a", None)]);
        let model = MenuBarModel {
            repos: vec![repo("x", 2, 0, passing.clone()), repo("y", 1, 3, None)],
        };
        assert_eq!(model.overall(), OverallState::Passing);
        assert_eq!(model.title(), "↑3 ↓3");
        let model = MenuBarModel {
            repos: vec![repo("x", 0, 0, pending), repo("y", 0, 0, failing)],
        };
        assert_eq!(model.overall(), OverallState::Failing);
        assert_eq!(model.title(), "");
        assert_eq!(
            model.description(),
            "Corvene: 2 watched repositories, checks failing"
        );
        assert_eq!(MenuBarModel::default().overall(), OverallState::NoChecks);
    }

    #[test]
    fn sync_labels() {
        assert_eq!(repo("x", 0, 0, None).sync_label(), "main · up to date");
        assert_eq!(
            repo("x", 2, 1, None).sync_label(),
            "main · 2 ahead, 1 behind"
        );
        let mut r = repo("x", 0, 0, None);
        r.has_upstream = false;
        assert_eq!(r.sync_label(), "main · no upstream");
    }
}
