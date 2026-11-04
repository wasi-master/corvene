//! Corvene `798-blame`: who last changed each line of a file. GitHub
//! Desktop has no blame view.
//!
//! `git blame --incremental` streams one entry per group of lines as soon
//! as git attributes it, so a large file's gutter fills in while git keeps
//! working. git applies `blame.ignoreRevsFile` from the configuration
//! itself; [`BlameOptions::ignore_revs_file`] adds the repository's
//! `.git-blame-ignore-revs` when nothing is configured, as GitHub.com does.

use std::path::Path;
use std::sync::Arc;

use crate::detect::GitBinary;
use crate::error::{GitError, Result};
use crate::process::{CancelToken, GitCommand};

/// The SHA git reports for lines that are not committed yet.
pub const UNCOMMITTED_SHA: &str = "0000000000000000000000000000000000000000";

/// The file GitHub.com reads revisions to skip from.
pub const IGNORE_REVS_FILE: &str = ".git-blame-ignore-revs";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlameOptions {
    /// `-w`: whitespace-only changes do not claim a line.
    pub ignore_whitespace: bool,
    /// `-M`: lines moved within the file keep their original commit.
    pub detect_moves: bool,
    /// `-C`: lines moved or copied from other files of the same commit
    /// keep their original commit.
    pub detect_copies: bool,
    /// Pass `.git-blame-ignore-revs` when it exists and the configuration
    /// names no `blame.ignoreRevsFile`.
    pub ignore_revs_file: bool,
    /// `-L <first>,<last>`: only these lines, 1-based and inclusive
    /// (Corvene `1361-merge-editor`'s chunk blame).
    pub line_range: Option<(u32, u32)>,
}

impl Default for BlameOptions {
    fn default() -> Self {
        Self {
            ignore_whitespace: false,
            detect_moves: false,
            detect_copies: false,
            ignore_revs_file: true,
            line_range: None,
        }
    }
}

/// A commit that claims lines, as `git blame --porcelain` describes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlameCommit {
    pub sha: String,
    pub author_name: String,
    pub author_email: String,
    /// Seconds since the epoch.
    pub author_time: i64,
    pub summary: String,
    /// The commit is the root (or the edge of a shallow history), so its
    /// lines may be older still.
    pub boundary: bool,
    /// The parent and the file's path there: where "Blame previous
    /// revision" continues. `None` for a file the commit added.
    pub previous: Option<(String, String)>,
}

impl BlameCommit {
    pub fn is_uncommitted(&self) -> bool {
        self.sha == UNCOMMITTED_SHA
    }

    pub fn short_sha(&self) -> &str {
        &self.sha[..self.sha.len().min(7)]
    }
}

/// `count` lines starting at `final_line` (1-based, in the blamed file)
/// come from `orig_line` of `filename` in `sha`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameRange {
    pub sha: String,
    pub orig_line: u32,
    pub final_line: u32,
    pub count: u32,
    /// The file's path in `sha` (differs from the blamed path across a
    /// rename).
    pub filename: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlameEvent {
    /// A commit's first range follows; sent once per commit.
    Commit(BlameCommit),
    Range(BlameRange),
}

/// Parses `git blame --incremental` line by line.
#[derive(Debug, Default)]
pub struct IncrementalParser {
    range: Option<BlameRange>,
    commit: Option<BlameCommit>,
    seen: std::collections::HashSet<String>,
}

impl IncrementalParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one output line; an entry ends with its `filename` line.
    pub fn feed(&mut self, line: &str, mut emit: impl FnMut(BlameEvent)) {
        if let Some(range) = &mut self.range {
            let (key, value) = line.split_once(' ').unwrap_or((line, ""));
            if let Some(commit) = &mut self.commit {
                match key {
                    "author" => commit.author_name = value.to_string(),
                    "author-mail" => {
                        commit.author_email = value
                            .trim_start_matches('<')
                            .trim_end_matches('>')
                            .to_string()
                    }
                    "author-time" => commit.author_time = value.parse().unwrap_or_default(),
                    "summary" => commit.summary = value.to_string(),
                    "boundary" => commit.boundary = true,
                    "previous" => {
                        commit.previous = value
                            .split_once(' ')
                            .map(|(sha, path)| (sha.to_string(), path.to_string()))
                    }
                    _ => {}
                }
            }
            if key == "filename" {
                range.filename = value.to_string();
                if let Some(commit) = self.commit.take() {
                    emit(BlameEvent::Commit(commit));
                }
                if let Some(range) = self.range.take() {
                    emit(BlameEvent::Range(range));
                }
            }
            return;
        }
        let mut parts = line.split(' ');
        let (Some(sha), Some(orig), Some(fin), Some(count)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return;
        };
        let (Ok(orig_line), Ok(final_line), Ok(count)) = (orig.parse(), fin.parse(), count.parse())
        else {
            return;
        };
        if sha.len() < 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
            return;
        }
        if self.seen.insert(sha.to_string()) {
            self.commit = Some(BlameCommit {
                sha: sha.to_string(),
                ..BlameCommit::default()
            });
        }
        self.range = Some(BlameRange {
            sha: sha.to_string(),
            orig_line,
            final_line,
            count,
            filename: String::new(),
        });
    }
}

/// Any configuration scope sets `blame.ignoreRevsFile` (git reads it then).
fn configures_ignore_revs_file(workdir: &Path) -> bool {
    crate::handle::open(workdir).is_ok_and(|repo| {
        repo.config_snapshot()
            .string("blame.ignoreRevsFile")
            .is_some()
    })
}

/// Blame `path` at `rev`, or in the working tree when `rev` is `None`
/// (uncommitted lines come as [`UNCOMMITTED_SHA`]). Ranges arrive in the
/// order git settles them, not in line order. A working file HEAD does
/// not have yet ends without events: every line is uncommitted.
pub fn blame(
    git: Arc<GitBinary>,
    workdir: &Path,
    rev: Option<&str>,
    path: &str,
    options: BlameOptions,
    cancel: Option<CancelToken>,
    mut on_event: impl FnMut(BlameEvent),
) -> Result<()> {
    // GHD 3.6.7 (desktop/desktop#22862): a revision starting with `-` as its
    // sha; blame takes no `--end-of-options` before a revision
    let resolved = match rev {
        Some(rev) => Some(crate::diff::resolve_blob_revision(
            git.clone(),
            workdir,
            rev,
        )?),
        None => None,
    };
    let mut cmd = GitCommand::new(git)
        .args(["blame", "--incremental"])
        .current_dir(workdir);
    if options.ignore_whitespace {
        cmd = cmd.arg("-w");
    }
    if options.detect_moves {
        cmd = cmd.arg("-M");
    }
    if options.detect_copies {
        cmd = cmd.arg("-C");
    }
    if options.ignore_revs_file
        && workdir.join(IGNORE_REVS_FILE).is_file()
        && !configures_ignore_revs_file(workdir)
    {
        cmd = cmd.args(["--ignore-revs-file", IGNORE_REVS_FILE]);
    }
    if let Some((first, last)) = options.line_range {
        cmd = cmd.args(["-L".to_string(), format!("{first},{last}")]);
    }
    if let Some(rev) = &resolved {
        cmd = cmd.arg(rev.as_ref());
    }
    cmd = cmd.args(["--", path]);
    if let Some(cancel) = cancel {
        cmd = cmd.cancel_token(cancel);
    }
    let mut parser = IncrementalParser::new();
    match cmd.run_streaming_stdout(|line| parser.feed(line, &mut on_event)) {
        Ok(_) => Ok(()),
        // a new file: nothing in HEAD to blame against
        Err(GitError::Failed { stderr, .. })
            if rev.is_none() && stderr.contains("no such path") =>
        {
            Ok(())
        }
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    const SAMPLE: &str = "\
1111111111111111111111111111111111111111 1 1 2
author Ann
author-mail <ann@example.com>
author-time 1700000000
author-tz +0000
committer Ann
committer-mail <ann@example.com>
committer-time 1700000000
committer-tz +0000
summary First commit
boundary
filename old.txt
2222222222222222222222222222222222222222 3 3 1
author Bob
author-mail <bob@example.com>
author-time 1700000100
author-tz +0000
summary Second
previous 1111111111111111111111111111111111111111 old.txt
filename new.txt
1111111111111111111111111111111111111111 4 4 1
filename old.txt
";

    #[test]
    fn parses_incremental_output() {
        let mut parser = IncrementalParser::new();
        let mut events = Vec::new();
        for line in SAMPLE.lines() {
            parser.feed(line, |e| events.push(e));
        }
        assert_eq!(events.len(), 5);
        let BlameEvent::Commit(first) = &events[0] else {
            panic!("{events:?}")
        };
        assert_eq!(first.author_name, "Ann");
        assert_eq!(first.author_email, "ann@example.com");
        assert_eq!(first.author_time, 1_700_000_000);
        assert_eq!(first.summary, "First commit");
        assert!(first.boundary);
        assert_eq!(first.previous, None);
        assert_eq!(
            events[1],
            BlameEvent::Range(BlameRange {
                sha: first.sha.clone(),
                orig_line: 1,
                final_line: 1,
                count: 2,
                filename: "old.txt".into(),
            })
        );
        let BlameEvent::Commit(second) = &events[2] else {
            panic!("{events:?}")
        };
        assert_eq!(
            second.previous,
            Some((first.sha.clone(), "old.txt".to_string()))
        );
        // a commit already described is not sent again
        assert!(matches!(&events[4], BlameEvent::Range(r) if r.final_line == 4));
    }

    fn repo() -> (tempfile::TempDir, Arc<GitBinary>) {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(dir.path())
                    .env("GIT_AUTHOR_NAME", "T")
                    .env("GIT_AUTHOR_EMAIL", "t@example.com")
                    .env("GIT_COMMITTER_NAME", "T")
                    .env("GIT_COMMITTER_EMAIL", "t@example.com")
                    .status()
                    .unwrap()
                    .success()
            )
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "core.autocrlf", "false"]);
        run(&["config", "commit.gpgsign", "false"]);
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "first"]);
        run(&["mv", "a.txt", "b.txt"]);
        run(&["commit", "-q", "-m", "rename"]);
        std::fs::write(dir.path().join("b.txt"), "one\nTWO\n").unwrap();
        run(&["commit", "-q", "-am", "second"]);
        (dir, Arc::new(crate::find_git().unwrap()))
    }

    fn blame_lines(
        git: Arc<GitBinary>,
        dir: &Path,
        rev: Option<&str>,
        path: &str,
    ) -> Vec<(u32, String, String)> {
        let mut commits = std::collections::HashMap::new();
        let mut lines = Vec::new();
        blame(
            git,
            dir,
            rev,
            path,
            BlameOptions::default(),
            None,
            |e| match e {
                BlameEvent::Commit(c) => {
                    commits.insert(c.sha.clone(), c.summary.clone());
                }
                BlameEvent::Range(r) => {
                    for i in 0..r.count {
                        lines.push((
                            r.final_line + i,
                            commits[&r.sha].clone(),
                            r.filename.clone(),
                        ));
                    }
                }
            },
        )
        .unwrap();
        lines.sort();
        lines
    }

    #[test]
    fn follows_renames_and_marks_uncommitted_lines() {
        let (dir, git) = repo();
        assert_eq!(
            blame_lines(git.clone(), dir.path(), Some("HEAD"), "b.txt"),
            vec![
                (1, "first".into(), "a.txt".into()),
                (2, "second".into(), "b.txt".into())
            ]
        );
        std::fs::write(dir.path().join("b.txt"), "zero\none\nTWO\n").unwrap();
        let working = blame_lines(git.clone(), dir.path(), None, "b.txt");
        assert_eq!(working[0].0, 1);
        assert_eq!(working.len(), 3);
        // a file HEAD does not have: no ranges, every line uncommitted
        std::fs::write(dir.path().join("new.txt"), "x\n").unwrap();
        assert!(blame_lines(git, dir.path(), None, "new.txt").is_empty());
    }

    #[test]
    fn cancelled_blame_stops() {
        let (dir, git) = repo();
        let token = CancelToken::new();
        token.cancel();
        let result = blame(
            git,
            dir.path(),
            Some("HEAD"),
            "b.txt",
            BlameOptions::default(),
            Some(token),
            |_| {},
        );
        assert!(matches!(result, Err(GitError::Cancelled(_))));
    }
}
