//! Corvene `1110-repository-insights`: contributors, commits per week and
//! the most changed files of a branch (or every branch) over a date range,
//! read in one streamed `git log --numstat` pass that a [`CancelToken`]
//! stops. GHD has no counterpart.
//!
//! Merge commits are left out (`--no-merges`): their numstat repeats the
//! merged branch's lines, and GitHub's own insights skip them too. Renames
//! are not followed (`--no-renames`): a renamed file's churn starts anew
//! under its new path, and rename detection would cost more than the rest
//! of the pass on big histories.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::detect::GitBinary;
use crate::error::Result;
use crate::process::{CancelToken, GitCommand};

/// Which history the statistics cover.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum StatsScope {
    /// The current branch (`HEAD`).
    Head,
    /// Every local and remote-tracking branch (`--branches --remotes`).
    AllBranches,
    /// One branch, tag or commit.
    Ref(String),
}

/// One author's share (`git shortlog -sne` with the lines).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Contributor {
    /// The name of their newest commit (after `.mailmap`).
    pub name: String,
    pub email: String,
    pub commits: u64,
    pub additions: u64,
    pub deletions: u64,
    /// Their newest and oldest commit, seconds since the epoch.
    pub last: i64,
    pub first: i64,
}

/// Commits and lines in the week starting `start` (Monday 00:00 UTC).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WeekBucket {
    pub start: i64,
    pub commits: u64,
    pub additions: u64,
    pub deletions: u64,
}

/// One file's churn.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileChurn {
    pub path: String,
    pub additions: u64,
    pub deletions: u64,
    pub commits: u64,
    /// A binary file: changed in `commits` commits, no lines counted.
    pub binary: bool,
}

impl FileChurn {
    pub fn lines(&self) -> u64 {
        self.additions + self.deletions
    }
}

/// The statistics of one scope and range.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepoStats {
    pub commits: u64,
    pub additions: u64,
    pub deletions: u64,
    /// Most commits first.
    pub contributors: Vec<Contributor>,
    /// Every week from the first commit's (or the range start's) to the
    /// newest commit's (or the range end's), oldest first, empty weeks
    /// included.
    pub weeks: Vec<WeekBucket>,
    /// The files with the most changed lines, at most [`TOP_FILES`].
    pub files: Vec<FileChurn>,
    /// How many distinct files changed.
    pub files_total: usize,
    /// The oldest and newest commit read.
    pub first: Option<i64>,
    pub last: Option<i64>,
}

/// How many files [`RepoStats::files`] keeps.
pub const TOP_FILES: usize = 100;

const WEEK: i64 = 7 * 24 * 60 * 60;
/// 1970-01-01 was a Thursday: the Monday before it is 3 days earlier.
const MONDAY_OFFSET: i64 = 3 * 24 * 60 * 60;

/// The Monday 00:00 UTC starting the week of `seconds`.
pub fn week_start(seconds: i64) -> i64 {
    (seconds + MONDAY_OFFSET).div_euclid(WEEK) * WEEK - MONDAY_OFFSET
}

/// Folds `git log --numstat` output into [`RepoStats`], a line at a time.
#[derive(Default)]
pub struct StatsBuilder {
    commits: u64,
    additions: u64,
    deletions: u64,
    /// By lower-cased email.
    contributors: HashMap<String, Contributor>,
    weeks: HashMap<i64, WeekBucket>,
    files: HashMap<String, FileChurn>,
    first: Option<i64>,
    last: Option<i64>,
    /// The commit being read: its contributor key and week.
    current: Option<(String, i64)>,
}

/// Starts each commit's header line (`%x1e`).
const RECORD: char = '\u{1e}';
/// Separates the header's fields (`%x1f`).
const FIELD: char = '\u{1f}';

/// `--format` for [`StatsBuilder::push_line`]: sha, author time, author
/// name and email (both after `.mailmap`).
pub const STATS_FORMAT: &str = "--format=%x1e%H%x1f%at%x1f%aN%x1f%aE";

impl StatsBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn commits(&self) -> u64 {
        self.commits
    }

    /// One line of output: a commit header or one of its numstat lines.
    pub fn push_line(&mut self, line: &str) {
        if let Some(header) = line.strip_prefix(RECORD) {
            let mut fields = header.split(FIELD);
            let _sha = fields.next();
            let time = fields
                .next()
                .and_then(|t| t.trim().parse::<i64>().ok())
                .unwrap_or_default();
            let name = fields.next().unwrap_or_default().trim().to_string();
            let email = fields.next().unwrap_or_default().trim().to_string();
            self.push_commit(time, name, email);
            return;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            return;
        }
        let mut parts = line.splitn(3, '\t');
        let (Some(added), Some(deleted), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            return;
        };
        // `-\t-\tpath`: a binary file
        let binary = added == "-" || deleted == "-";
        let added = added.parse::<u64>().unwrap_or(0);
        let deleted = deleted.parse::<u64>().unwrap_or(0);
        self.push_file(path, added, deleted, binary);
    }

    fn push_commit(&mut self, time: i64, name: String, email: String) {
        self.commits += 1;
        self.first = Some(self.first.map_or(time, |f| f.min(time)));
        self.last = Some(self.last.map_or(time, |l| l.max(time)));
        let key = if email.is_empty() {
            name.to_lowercase()
        } else {
            email.to_lowercase()
        };
        let contributor = self
            .contributors
            .entry(key.clone())
            .or_insert_with(|| Contributor {
                name: name.clone(),
                email: email.clone(),
                first: time,
                last: time,
                ..Contributor::default()
            });
        contributor.commits += 1;
        // the newest commit names them
        if time >= contributor.last {
            contributor.last = time;
            if !name.is_empty() {
                contributor.name = name;
            }
        }
        contributor.first = contributor.first.min(time);
        let week = week_start(time);
        let bucket = self.weeks.entry(week).or_insert(WeekBucket {
            start: week,
            ..WeekBucket::default()
        });
        bucket.commits += 1;
        self.current = Some((key, week));
    }

    fn push_file(&mut self, path: &str, added: u64, deleted: u64, binary: bool) {
        let Some((key, week)) = &self.current else {
            return;
        };
        self.additions += added;
        self.deletions += deleted;
        if let Some(c) = self.contributors.get_mut(key) {
            c.additions += added;
            c.deletions += deleted;
        }
        if let Some(b) = self.weeks.get_mut(week) {
            b.additions += added;
            b.deletions += deleted;
        }
        let file = self
            .files
            .entry(path.to_string())
            .or_insert_with(|| FileChurn {
                path: path.to_string(),
                ..FileChurn::default()
            });
        file.additions += added;
        file.deletions += deleted;
        file.commits += 1;
        file.binary |= binary;
    }

    /// The statistics; the weeks run from `since` (or the first commit) to
    /// `until` (or the newest commit).
    pub fn finish(self, since: Option<i64>, until: Option<i64>) -> RepoStats {
        let mut contributors: Vec<Contributor> = self.contributors.into_values().collect();
        contributors.sort_by(|a, b| {
            b.commits
                .cmp(&a.commits)
                .then((b.additions + b.deletions).cmp(&(a.additions + a.deletions)))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        let files_total = self.files.len();
        let mut files: Vec<FileChurn> = self.files.into_values().collect();
        files.sort_by(|a, b| {
            b.lines()
                .cmp(&a.lines())
                .then(b.commits.cmp(&a.commits))
                .then_with(|| a.path.cmp(&b.path))
        });
        files.truncate(TOP_FILES);
        let from = since.or(self.first).map(week_start);
        let to = until.or(self.last).map(week_start);
        let weeks = match (from, to) {
            (Some(from), Some(to)) if from <= to => {
                let mut weeks = Vec::with_capacity(((to - from) / WEEK + 1) as usize);
                let mut start = from;
                while start <= to {
                    weeks.push(self.weeks.get(&start).copied().unwrap_or(WeekBucket {
                        start,
                        ..WeekBucket::default()
                    }));
                    start += WEEK;
                }
                weeks
            }
            _ => Vec::new(),
        };
        RepoStats {
            commits: self.commits,
            additions: self.additions,
            deletions: self.deletions,
            contributors,
            weeks,
            files,
            files_total,
            first: self.first,
            last: self.last,
        }
    }
}

/// How many commits [`repository_stats`] will read for the same scope and
/// `since` (`rev-list --count`, fast next to `log --numstat`): the total its
/// progress counts towards. 0 for an unborn branch or unknown revision.
pub fn stats_commit_count(
    git: Arc<GitBinary>,
    workdir: &Path,
    scope: &StatsScope,
    since: Option<i64>,
    cancel: &CancelToken,
) -> Result<u64> {
    let mut args: Vec<String> = vec!["rev-list".into(), "--count".into(), "--no-merges".into()];
    if let Some(since) = since {
        args.push(format!("--since=@{since}"));
    }
    match scope {
        StatsScope::Head => args.push("HEAD".into()),
        StatsScope::AllBranches => {
            args.push("--branches".into());
            args.push("--remotes".into());
        }
        StatsScope::Ref(name) => args.push(name.clone()),
    }
    args.push("--".into());
    let out = GitCommand::new(git)
        .args(args)
        .current_dir(workdir)
        .cancel_token(cancel.clone())
        .allow_exit_code(128)
        .run()?;
    if !out.status.success() {
        return Ok(0);
    }
    Ok(out.stdout_string()?.trim().parse().unwrap_or(0))
}

/// Reads the statistics of `scope` for the commits made since `since`
/// (seconds since the epoch; `None`: all of them). `progress` hears the
/// number of commits read so far, every few hundred commits. `cancel`
/// stops git, and the call then fails with [`crate::GitError::Cancelled`].
/// An unborn branch or an unknown ref has no statistics.
pub fn repository_stats(
    git: Arc<GitBinary>,
    workdir: &Path,
    scope: &StatsScope,
    since: Option<i64>,
    until: Option<i64>,
    cancel: &CancelToken,
    mut progress: impl FnMut(u64),
) -> Result<RepoStats> {
    let mut args: Vec<String> = [
        "-c",
        "core.quotePath=false",
        "log",
        "--no-merges",
        "--no-renames",
        "--numstat",
        "--no-show-signature",
        "--no-color",
        STATS_FORMAT,
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    if let Some(since) = since {
        args.push(format!("--since=@{since}"));
    }
    match scope {
        StatsScope::Head => args.push("HEAD".into()),
        StatsScope::AllBranches => {
            args.push("--branches".into());
            args.push("--remotes".into());
        }
        StatsScope::Ref(name) => args.push(name.clone()),
    }
    args.push("--".into());
    let mut builder = StatsBuilder::new();
    let mut reported = 0;
    let out = GitCommand::new(git)
        .args(args)
        .current_dir(workdir)
        .cancel_token(cancel.clone())
        .forget_streamed()
        // unborn branch / unknown revision
        .allow_exit_code(128)
        .run_streaming_stdout(|line| {
            builder.push_line(line);
            if builder.commits() >= reported + 250 {
                reported = builder.commits();
                progress(reported);
            }
        })?;
    if cancel.is_cancelled() {
        return Err(crate::GitError::Cancelled("git log --numstat".into()));
    }
    if !out.status.success() {
        return Ok(RepoStats::default());
    }
    Ok(builder.finish(since, until))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(time: i64, name: &str, email: &str) -> String {
        format!(
            "{RECORD}{}{FIELD}{time}{FIELD}{name}{FIELD}{email}",
            "a".repeat(40)
        )
    }

    #[test]
    fn weeks_start_on_monday() {
        // 2024-01-03 (a Wednesday) 12:00 UTC → Monday 2024-01-01
        assert_eq!(week_start(1_704_283_200), 1_704_067_200);
        // a Monday at midnight is its own week
        assert_eq!(week_start(1_704_067_200), 1_704_067_200);
        // before the epoch
        assert_eq!(week_start(-1), -MONDAY_OFFSET);
    }

    #[test]
    fn folds_commits_files_and_weeks() {
        let mut b = StatsBuilder::new();
        let monday = 1_704_067_200;
        for line in [
            header(monday + 3 * WEEK, "Ann B", "ann@example.com"),
            "".into(),
            "10\t2\tsrc/a.rs".into(),
            "-\t-\tlogo.png".into(),
            header(monday + 60, "Ann", "ANN@example.com"),
            "".into(),
            "1\t1\tsrc/a.rs".into(),
            header(monday + 120, "Bo", "bo@example.com"),
            "".into(),
            "5\t0\tREADME.md".into(),
            "0\t7\tsrc/a.rs".into(),
        ] {
            b.push_line(&line);
        }
        let stats = b.finish(None, None);
        assert_eq!(stats.commits, 3);
        assert_eq!((stats.additions, stats.deletions), (16, 10));
        // case-folded email: one contributor, named after the newest commit
        assert_eq!(stats.contributors.len(), 2);
        let ann = &stats.contributors[0];
        assert_eq!((ann.name.as_str(), ann.commits), ("Ann B", 2));
        assert_eq!((ann.additions, ann.deletions), (11, 3));
        assert_eq!(stats.contributors[1].name, "Bo");
        // four weeks, the two in between empty
        let commits: Vec<u64> = stats.weeks.iter().map(|w| w.commits).collect();
        assert_eq!(commits, vec![2, 0, 0, 1]);
        assert_eq!(stats.weeks[0].start, monday);
        assert_eq!(stats.weeks[0].additions, 6);
        // most changed lines first, binaries counted in commits
        let files: Vec<(&str, u64, u64)> = stats
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.lines(), f.commits))
            .collect();
        assert_eq!(
            files,
            vec![("src/a.rs", 21, 3), ("README.md", 5, 1), ("logo.png", 0, 1)]
        );
        assert!(stats.files[2].binary);
        assert_eq!(stats.files_total, 3);
    }

    #[test]
    fn range_pads_the_weeks() {
        let mut b = StatsBuilder::new();
        let monday = 1_704_067_200;
        b.push_line(&header(monday + WEEK, "A", "a@x"));
        let stats = b.finish(Some(monday - 1), Some(monday + 2 * WEEK + 5));
        assert_eq!(stats.weeks.len(), 4);
        assert_eq!(stats.weeks[0].start, monday - WEEK);
        assert_eq!(stats.weeks[2].commits, 1);
    }

    #[test]
    fn empty_history_has_no_weeks() {
        let stats = StatsBuilder::new().finish(None, None);
        assert_eq!(stats, RepoStats::default());
    }
}
