//! Corvene `1110-repository-insights`: Repository › Insights…, the
//! contributors, commits per week and most changed files of a branch (or
//! every branch) over a date range, in place of the tab's content. GitHub
//! Desktop has no counterpart (GitHub's own Insights › Contributors and
//! Code frequency pages show the like for the default branch).
//!
//! A load first resolves the scope's tips (fast), then looks in
//! [`RepositoryState::insights_cache`], keyed by scope, range, the tips
//! and the day the range ends: a hit shows at once, so the numbers of a
//! HEAD are read once. A miss runs [`corvene_git::repository_stats`] on a
//! background thread; the number of commits read so far reaches the view
//! as it goes, and Stop, another scope or range, closing the view or
//! leaving the tab kill git. When History reloads with other tips the view
//! says the numbers are out of date instead of reading them again on its own.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use corvene_git::{CancelToken, RepoStats, StatsScope};
use corvene_models::Section;
use tracing::warn;

use crate::dispatcher::Dispatcher;
use crate::host::{AsyncCtx, Host};
use crate::remote::spawn_bg;
use crate::state::AppState;

/// How many finished results a repository keeps.
const CACHE_SIZE: usize = 8;

const DAY: i64 = 24 * 60 * 60;

/// The date range.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum InsightsRange {
    Week,
    Month,
    ThreeMonths,
    SixMonths,
    #[default]
    Year,
    AllTime,
}

impl InsightsRange {
    pub const ALL: [Self; 6] = [
        Self::Week,
        Self::Month,
        Self::ThreeMonths,
        Self::SixMonths,
        Self::Year,
        Self::AllTime,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Week => "Past week",
            Self::Month => "Past month",
            Self::ThreeMonths => "Past 3 months",
            Self::SixMonths => "Past 6 months",
            Self::Year => "Past year",
            Self::AllTime => "All time",
        }
    }

    fn days(self) -> Option<i64> {
        match self {
            Self::Week => Some(7),
            Self::Month => Some(30),
            Self::ThreeMonths => Some(91),
            Self::SixMonths => Some(182),
            Self::Year => Some(365),
            Self::AllTime => None,
        }
    }

    /// The range's start for a range ending on `today` (a day's start).
    pub fn since(self, today: i64) -> Option<i64> {
        self.days().map(|days| today - (days - 1) * DAY)
    }
}

/// What a cached result was read for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InsightsKey {
    pub scope: StatsScope,
    pub range: InsightsRange,
    /// The scope's tips (HEAD's commit, or a hash of every branch tip).
    pub tips: String,
    /// The start of the day the range ends on (a range moves with the days).
    pub today: i64,
}

/// The open Insights view.
#[derive(Clone, Debug)]
pub struct InsightsState {
    /// The tab it was opened in.
    pub section: Section,
    pub scope: StatsScope,
    pub range: InsightsRange,
    pub loading: bool,
    /// Commits read so far while loading.
    pub progress: u64,
    /// How many commits the load reads, once counted.
    pub progress_total: Option<u64>,
    pub stats: Option<Arc<RepoStats>>,
    /// What [`Self::stats`] was read for.
    pub key: Option<InsightsKey>,
    /// The last load failed.
    pub error: Option<String>,
    /// Stop was pressed before the numbers were in.
    pub stopped: bool,
    /// The tips moved since [`Self::stats`] was read.
    pub stale: bool,
    /// How long reading took (`None`: from the cache).
    pub took: Option<Duration>,
    /// Identifies the running load; late results of older ones are dropped.
    pub load: u64,
    pub cancel: CancelToken,
}

impl InsightsState {
    fn new(section: Section, scope: StatsScope, range: InsightsRange) -> Self {
        Self {
            section,
            scope,
            range,
            loading: false,
            progress: 0,
            progress_total: None,
            stats: None,
            key: None,
            error: None,
            stopped: false,
            stale: false,
            took: None,
            load: 0,
            cancel: CancelToken::new(),
        }
    }
}

/// The flag is on.
pub fn enabled(s: &AppState) -> bool {
    s.flags.bool(crate::flags::ids::REPOSITORY_INSIGHTS)
}

fn today() -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    now.div_euclid(DAY) * DAY
}

/// The scope's tips, for the cache key.
fn scope_tips(workdir: &std::path::Path, scope: &StatsScope) -> Result<String, String> {
    use std::hash::{Hash, Hasher};
    match scope {
        StatsScope::Head => corvene_git::resolve_commit(workdir, "HEAD")
            .map(Option::unwrap_or_default)
            .map_err(|err| err.to_string()),
        StatsScope::Ref(name) => corvene_git::resolve_commit(workdir, name)
            .map_err(|err| err.to_string())?
            .ok_or_else(|| format!("{name} is not a branch, tag or commit.")),
        StatsScope::AllBranches => {
            let mut tips = corvene_git::all_branch_tips(workdir).map_err(|err| err.to_string())?;
            tips.sort();
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            tips.hash(&mut hasher);
            Ok(format!("{}:{:016x}", tips.len(), hasher.finish()))
        }
    }
}

static NEXT_LOAD: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

enum InsightsMessage {
    Total(u64),
    Progress(u64),
    Done(Result<RepoStats, String>, Duration),
}

impl Dispatcher {
    /// Repository › Insights…: open the view in the current tab (the
    /// scope and range it had last time), or read it again.
    pub fn show_insights(id: u64, cx: &mut dyn Host) {
        if !enabled(Self::state(cx).read(cx)) {
            return;
        }
        Self::close_blame(id, cx);
        Self::state(cx).update(cx, |s, cx| {
            let rs = s.repo_state_mut(id);
            let section = rs.section;
            match &mut rs.insights {
                Some(open) => open.section = section,
                None => {
                    rs.insights = Some(InsightsState::new(
                        section,
                        StatsScope::Head,
                        InsightsRange::default(),
                    ));
                }
            }
            cx.notify();
        });
        Self::load_insights(id, cx);
    }

    /// The scope picker.
    pub fn set_insights_scope(id: u64, scope: StatsScope, cx: &mut dyn Host) {
        Self::update_insights(id, cx, |i| i.scope = scope);
    }

    /// The range picker.
    pub fn set_insights_range(id: u64, range: InsightsRange, cx: &mut dyn Host) {
        Self::update_insights(id, cx, |i| i.range = range);
    }

    fn update_insights(id: u64, cx: &mut dyn Host, f: impl FnOnce(&mut InsightsState)) {
        let open = Self::state(cx).update(cx, |s, _| {
            let Some(insights) = s.repo_state_mut(id).insights.as_mut() else {
                return false;
            };
            f(insights);
            true
        });
        if open {
            Self::load_insights(id, cx);
        }
    }

    /// Read the statistics of the view's scope and range (from the cache
    /// when the tips have not moved).
    pub fn load_insights(id: u64, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let load = NEXT_LOAD.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let Some((scope, range, cancel)) = Self::state(cx).update(cx, |s, cx| {
            let insights = s.repo_state_mut(id).insights.as_mut()?;
            insights.cancel.cancel();
            insights.cancel = CancelToken::new();
            insights.load = load;
            insights.loading = true;
            insights.progress = 0;
            insights.progress_total = None;
            insights.error = None;
            insights.stopped = false;
            cx.notify();
            Some((
                insights.scope.clone(),
                insights.range,
                insights.cancel.clone(),
            ))
        }) else {
            return;
        };
        let today = today();
        let key_scope = scope.clone();
        let tips_dir = workdir.clone();
        spawn_bg(
            cx,
            move || scope_tips(&tips_dir, &key_scope),
            move |tips, cx| {
                let tips = match tips {
                    Ok(tips) => tips,
                    Err(message) => {
                        Self::finish_insights(id, load, None, Err(message), None, cx);
                        return;
                    }
                };
                let key = InsightsKey {
                    scope: scope.clone(),
                    range,
                    tips,
                    today,
                };
                let cached = Self::state(cx).update(cx, |s, _| {
                    let rs = s.repo_state_mut(id);
                    let at = rs.insights_cache.iter().position(|(k, _)| *k == key)?;
                    // most recently used last
                    let entry = rs.insights_cache.remove(at);
                    let stats = entry.1.clone();
                    rs.insights_cache.push(entry);
                    Some(stats)
                });
                if let Some(stats) = cached {
                    Self::finish_insights(id, load, Some(key), Ok(stats), None, cx);
                    return;
                }
                Self::read_insights(id, load, key, git, workdir, cancel, cx);
            },
        );
    }

    /// The background pass of a cache miss.
    #[allow(clippy::too_many_arguments)]
    fn read_insights(
        id: u64,
        load: u64,
        key: InsightsKey,
        git: Arc<corvene_git::GitBinary>,
        workdir: std::path::PathBuf,
        cancel: CancelToken,
        cx: &mut dyn Host,
    ) {
        let since = key.range.since(key.today);
        // the weeks run up to now
        let until = key.today + DAY - 1;
        let scope = key.scope.clone();
        let (tx, rx) = async_channel::unbounded::<InsightsMessage>();
        let task = cx.background_executor().spawn(async move {
            let started = Instant::now();
            if let Ok(total) =
                corvene_git::stats_commit_count(git.clone(), &workdir, &scope, since, &cancel)
            {
                let _ = tx.send_blocking(InsightsMessage::Total(total));
            }
            let result = corvene_git::repository_stats(
                git,
                &workdir,
                &scope,
                since,
                Some(until),
                &cancel,
                |read| {
                    let _ = tx.send_blocking(InsightsMessage::Progress(read));
                },
            )
            .map_err(|err| err.to_string());
            if !cancel.is_cancelled() {
                let _ = tx.send_blocking(InsightsMessage::Done(result, started.elapsed()));
            }
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            while let Ok(first) = rx.recv().await {
                // progress that queued up while the main thread was busy
                // collapses into the newest (the total is applied on the way)
                let mut last = first;
                while let Ok(more) = rx.try_recv() {
                    if let InsightsMessage::Total(total) = last {
                        Self::set_insights_total(id, load, total, cx);
                    }
                    last = more;
                }
                match last {
                    InsightsMessage::Total(total) => Self::set_insights_total(id, load, total, cx),
                    InsightsMessage::Progress(read) => cx.update(|cx| {
                        Self::state(cx).update(cx, |s, cx| {
                            if let Some(i) = s.repo_state_mut(id).insights.as_mut()
                                && i.load == load
                            {
                                i.progress = read;
                                cx.notify();
                            }
                        })
                    }),
                    InsightsMessage::Done(result, took) => {
                        let key = key.clone();
                        cx.update(|cx| {
                            Self::finish_insights(
                                id,
                                load,
                                Some(key),
                                result.map(Arc::new),
                                Some(took),
                                cx,
                            )
                        });
                        break;
                    }
                }
            }
            task.await;
        })
        .detach();
    }

    fn set_insights_total(id: u64, load: u64, total: u64, cx: &mut AsyncCtx) {
        cx.update(|cx| {
            Self::state(cx).update(cx, |s, cx| {
                if let Some(i) = s.repo_state_mut(id).insights.as_mut()
                    && i.load == load
                {
                    i.progress_total = Some(total);
                    cx.notify();
                }
            })
        })
    }

    fn finish_insights(
        id: u64,
        load: u64,
        key: Option<InsightsKey>,
        result: Result<Arc<RepoStats>, String>,
        took: Option<Duration>,
        cx: &mut dyn Host,
    ) {
        Self::state(cx).update(cx, |s, cx| {
            let rs = s.repo_state_mut(id);
            let Some(insights) = rs.insights.as_mut().filter(|i| i.load == load) else {
                return;
            };
            insights.loading = false;
            match result {
                Ok(stats) => {
                    insights.stats = Some(stats.clone());
                    insights.key = key.clone();
                    insights.stale = false;
                    insights.took = took;
                    if let Some(key) = key
                        && took.is_some()
                    {
                        rs.insights_cache.retain(|(k, _)| *k != key);
                        rs.insights_cache.push((key, stats));
                        if rs.insights_cache.len() > CACHE_SIZE {
                            rs.insights_cache.remove(0);
                        }
                    }
                }
                Err(message) => {
                    warn!(id, %message, "insights failed");
                    insights.error = Some(message);
                }
            }
            cx.notify();
        });
    }

    /// Stop: kill git, keep what was shown before.
    pub fn stop_insights(id: u64, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            if let Some(i) = s.repo_state_mut(id).insights.as_mut()
                && i.loading
            {
                i.cancel.cancel();
                i.loading = false;
                i.stopped = true;
                // late messages of the stopped load are dropped
                i.load = 0;
                cx.notify();
            }
        });
    }

    /// The header's close button and Escape.
    pub fn close_insights(id: u64, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            if let Some(rs) = s.repo_states.get_mut(&id)
                && let Some(insights) = rs.insights.take()
            {
                insights.cancel.cancel();
                cx.notify();
            }
        });
    }

    /// Close the view unless `keep` says it belongs to what is shown.
    pub(crate) fn close_insights_unless(
        id: u64,
        keep: impl Fn(&InsightsState) -> bool,
        cx: &mut dyn Host,
    ) {
        let open = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|rs| rs.insights.as_ref())
            .is_some_and(|i| !keep(i));
        if open {
            Self::close_insights(id, cx);
        }
    }

    /// History reloaded: mark the numbers out of date when the tips moved.
    pub(crate) fn check_insights_stale(id: u64, cx: &mut dyn Host) {
        let Some((key, load)) = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|rs| {
                let i = rs.insights.as_ref().filter(|i| !i.loading && !i.stale)?;
                Some((i.key.clone()?, i.load))
            })
        else {
            return;
        };
        let Some((_, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let scope = key.scope.clone();
        spawn_bg(
            cx,
            move || scope_tips(&workdir, &scope),
            move |tips, cx| {
                let moved = tips.map(|t| t != key.tips).unwrap_or(true);
                if !moved {
                    return;
                }
                Self::state(cx).update(cx, |s, cx| {
                    if let Some(i) = s.repo_state_mut(id).insights.as_mut()
                        && i.load == load
                        && !i.loading
                    {
                        i.stale = true;
                        cx.notify();
                    }
                });
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn ranges_end_today() {
        let today = 20_000 * DAY;
        assert_eq!(InsightsRange::Week.since(today), Some(today - 6 * DAY));
        assert_eq!(InsightsRange::Year.since(today), Some(today - 364 * DAY));
        assert_eq!(InsightsRange::AllTime.since(today), None);
    }
}
