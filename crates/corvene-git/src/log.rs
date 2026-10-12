//! Commit history - GHD `lib/git/log.ts` (`getCommits`, `getChangedFiles`,
//! `getCommitDiff`). The walk is done in-process with gitoxide, building each
//! commit as GHD's `git log` format does: git's `%s` / `%b` for the summary
//! and body, `%(trailers:unfold,only)` for the trailers and `%D`'s order for
//! the tags. Changed files and per-file diffs come from the git CLI so
//! rename/copy detection matches GitHub Desktop exactly.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use corvene_models::{
    ChangesetData, Commit, CommitIdentity, CommittedFileChange, Diff, FileStatus, FileStatusKind,
    GitStatusEntry, SignatureKind,
};

use crate::detect::GitBinary;
use crate::error::{GitError, Result};
use crate::process::GitCommand;

/// GHD `CommitBatchSize`
pub const COMMIT_BATCH_SIZE: usize = 100;

fn identity(sig: gix::actor::SignatureRef<'_>) -> CommitIdentity {
    let time = sig.time().unwrap_or_default();
    CommitIdentity {
        name: sig.name.to_string(),
        email: sig.email.to_string(),
        seconds: time.seconds,
        offset: time.offset,
    }
}

/// GHD `getAllTags` (`lib/git/tag.ts`): every tag's short name
/// (`refs/tags/` stripped) and the object it points at, annotated tags
/// peeled to their commit (`git show-ref --tags -d`, read in-process).
/// Empty without tags.
pub fn get_all_tags(workdir: &Path) -> Result<HashMap<String, String>> {
    let repo = crate::handle::open(workdir)?;
    let refs = repo
        .references()
        .map_err(|e| GitError::Gix(e.to_string()))?;
    let mut tags = HashMap::new();
    for r in refs
        .tags()
        .map_err(|e| GitError::Gix(e.to_string()))?
        .flatten()
    {
        let name = r.name().shorten().to_string();
        if let Ok(id) = r.into_fully_peeled_id() {
            tags.insert(name, id.to_string());
        }
    }
    Ok(tags)
}

/// Every tag's short name ([`get_all_tags`]), sorted case-insensitively.
/// Feeds the compare list's Tags group (flag `825`).
pub fn tag_names(workdir: &Path) -> Result<Vec<String>> {
    let mut names: Vec<String> = get_all_tags(workdir)?.into_keys().collect();
    names.sort_by_key(|n| n.to_lowercase());
    Ok(names)
}

/// The tags of each commit in `git log`'s `%D` order: git prepends each
/// decoration while it walks the refs in name order, so a commit's tags
/// come in descending ref-name order (GHD `getCommits` keeps that order).
fn tags_by_commit(repo: &gix::Repository) -> HashMap<gix::ObjectId, Vec<String>> {
    let mut tags: HashMap<gix::ObjectId, Vec<String>> = HashMap::new();
    if let Ok(refs) = repo.references()
        && let Ok(iter) = refs.tags()
    {
        for r in iter.flatten() {
            let name = r.name().shorten().to_string();
            if let Ok(id) = r.into_fully_peeled_id() {
                tags.entry(id.detach()).or_default().push(name);
            }
        }
    }
    for names in tags.values_mut() {
        names.sort_by(|a, b| b.cmp(a));
    }
    tags
}

/// GHD `getCommits` cuts the summary and the body at 100 KiB.
const MESSAGE_FIELD_LIMIT: usize = 100 * 1024;

fn message_field(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(MESSAGE_FIELD_LIMIT)]).into_owned()
}

/// git's `%s` and `%b` of a raw commit message (`pretty.c`
/// `parse_commit_message` / `format_subject`): blank lines are skipped, the
/// subject is the first paragraph with each line's trailing whitespace
/// dropped and the lines joined by spaces, and the body is everything after
/// that paragraph and the blank lines below it, verbatim (so it keeps its
/// final newline).
fn subject_and_body(message: &[u8]) -> (String, String) {
    fn is_space(b: u8) -> bool {
        matches!(b, b' ' | b'\t' | b'\n' | b'\r')
    }
    // `get_one_line`: the line with its newline
    fn line_len(s: &[u8]) -> usize {
        s.iter()
            .position(|&b| b == b'\n')
            .map_or(s.len(), |i| i + 1)
    }
    // `is_blank_line`: the length without trailing whitespace
    fn trimmed_len(line: &[u8]) -> usize {
        line.iter()
            .rposition(|&b| !is_space(b))
            .map_or(0, |i| i + 1)
    }
    fn skip_blank_lines(mut s: &[u8]) -> &[u8] {
        loop {
            let n = line_len(s);
            if n == 0 || trimmed_len(&s[..n]) != 0 {
                return s;
            }
            s = &s[n..];
        }
    }
    let mut rest = skip_blank_lines(message);
    let mut subject: Vec<u8> = Vec::new();
    let mut first = true;
    loop {
        let n = line_len(rest);
        let line = &rest[..n];
        rest = &rest[n..];
        let len = trimmed_len(line);
        if n == 0 || len == 0 {
            break;
        }
        if !first {
            subject.push(b' ');
        }
        subject.extend_from_slice(&line[..len]);
        first = false;
    }
    (
        message_field(&subject),
        message_field(skip_blank_lines(rest)),
    )
}

/// One walked commit as GHD's `getCommits` builds it.
fn commit_from_walk(
    info: gix::revision::walk::Info<'_>,
    tags: &HashMap<gix::ObjectId, Vec<String>>,
) -> Result<Commit> {
    let commit = info.object().map_err(|e| GitError::Gix(e.to_string()))?;
    let parents = info.parent_ids.iter().map(|p| p.to_string()).collect();
    build_commit(info.id, &commit, parents, tags)
}

/// The commit `id` as GHD's `getCommits` builds it, looked up by id.
fn commit_from_id(
    repo: &gix::Repository,
    id: gix::ObjectId,
    tags: &HashMap<gix::ObjectId, Vec<String>>,
) -> Result<Commit> {
    let commit = repo
        .find_commit(id)
        .map_err(|e| GitError::Gix(e.to_string()))?;
    let parents = commit.parent_ids().map(|p| p.to_string()).collect();
    build_commit(id, &commit, parents, tags)
}

fn build_commit(
    id: gix::ObjectId,
    commit: &gix::Commit<'_>,
    parents: Vec<String>,
    tags: &HashMap<gix::ObjectId, Vec<String>>,
) -> Result<Commit> {
    let decoded = commit.decode().map_err(|e| GitError::Gix(e.to_string()))?;
    let (summary, body) = subject_and_body(decoded.message);
    // `%(trailers:unfold,only)` parsed by `parseRawUnfoldedTrailers`
    let trailers = decoded
        .message()
        .body()
        .map(|b| {
            b.trailers()
                .map(|t| {
                    (
                        t.token.to_string().trim().to_string(),
                        t.value.to_string().trim().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Commit {
        sha: id.to_string(),
        summary,
        body,
        author: identity(commit.author().map_err(|e| GitError::Gix(e.to_string()))?),
        committer: identity(
            commit
                .committer()
                .map_err(|e| GitError::Gix(e.to_string()))?,
        ),
        parents,
        trailers,
        tags: tags.get(&id).cloned().unwrap_or_default(),
        // `1214-commit-signatures`: the header is already decoded
        signature: decoded
            .extra_headers()
            .pgp_signature()
            .and_then(|sig| SignatureKind::from_armor(sig)),
    })
}

/// Commits reachable from `revision` (a ref name or sha), newest first,
/// `skip` then at most `limit` of them.
pub fn get_commits(
    workdir: &Path,
    revision: &str,
    skip: usize,
    limit: usize,
) -> Result<Vec<Commit>> {
    get_commits_with(workdir, revision, skip, limit, false)
}

/// [`get_commits`], following only first parents when `first_parent`
/// (`git log --first-parent`).
pub fn get_commits_with(
    workdir: &Path,
    revision: &str,
    skip: usize,
    limit: usize,
    first_parent: bool,
) -> Result<Vec<Commit>> {
    let repo = crate::handle::open(workdir)?;
    let Some(tip) = repo.rev_parse_single(revision).ok() else {
        return Ok(Vec::new());
    };
    walk_commits(&repo, vec![tip.detach()], skip, limit, first_parent)
}

/// Corvene `1213-commit-graph`: where History's "All branches" list starts,
/// HEAD's commit first, then the tips of the local and remote-tracking
/// branches (`git log HEAD --branches --remotes`), each commit once. Empty
/// on an unborn branch with no branches.
pub fn all_branch_tips(workdir: &Path) -> Result<Vec<String>> {
    let repo = crate::handle::open(workdir)?;
    let mut tips: Vec<String> = Vec::new();
    if let Ok(head) = repo.head_id() {
        tips.push(head.to_string());
    }
    let refs = repo
        .references()
        .map_err(|e| GitError::Gix(e.to_string()))?;
    let mut branches: Vec<(String, String)> = Vec::new();
    for iter in [refs.local_branches(), refs.remote_branches()] {
        let iter = iter.map_err(|e| GitError::Gix(e.to_string()))?;
        for r in iter.flatten() {
            // `refs/remotes/<remote>/HEAD` only repeats a branch
            if matches!(r.target(), gix::refs::TargetRef::Symbolic(_)) {
                continue;
            }
            let name = r.name().as_bstr().to_string();
            if let Ok(id) = r.into_fully_peeled_id() {
                branches.push((name, id.to_string()));
            }
        }
    }
    branches.sort();
    for (_, id) in branches {
        if !tips.contains(&id) {
            tips.push(id);
        }
    }
    Ok(tips)
}

/// [`get_commits_with`] walking from several `tips` (SHAs) at once, newest
/// first by commit date, each commit once.
pub fn get_commits_from(
    workdir: &Path,
    tips: &[String],
    skip: usize,
    limit: usize,
    first_parent: bool,
) -> Result<Vec<Commit>> {
    let repo = crate::handle::open(workdir)?;
    let tips: Vec<gix::ObjectId> = tips
        .iter()
        .filter_map(|t| gix::ObjectId::from_hex(t.as_bytes()).ok())
        .collect();
    if tips.is_empty() {
        return Ok(Vec::new());
    }
    walk_commits(&repo, tips, skip, limit, first_parent)
}

/// Corvene `1216-recent-activity`: the commits `shas` name, in that order,
/// each once. A SHA whose object is gone (garbage collected) or is not a
/// commit is left out.
pub fn commits_by_sha(workdir: &Path, shas: &[String]) -> Result<Vec<Commit>> {
    let repo = crate::handle::open(workdir)?;
    let tags = tags_by_commit(&repo);
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for sha in shas {
        let Ok(id) = gix::ObjectId::from_hex(sha.as_bytes()) else {
            continue;
        };
        if !seen.insert(id) {
            continue;
        }
        if let Ok(commit) = commit_from_id(&repo, id, &tags) {
            out.push(commit);
        }
    }
    Ok(out)
}

fn walk_commits(
    repo: &gix::Repository,
    tips: Vec<gix::ObjectId>,
    skip: usize,
    limit: usize,
    first_parent: bool,
) -> Result<Vec<Commit>> {
    let tags = tags_by_commit(repo);
    let mut walk = repo.rev_walk(tips);
    if first_parent {
        walk = walk.first_parent_only();
    }
    let walk = walk
        .sorting(gix::revision::walk::Sorting::ByCommitTime(
            gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
        ))
        .all()
        .map_err(|e| GitError::Gix(e.to_string()))?;
    let mut out = Vec::with_capacity(limit.min(COMMIT_BATCH_SIZE));
    for info in walk.skip(skip).take(limit) {
        let info = info.map_err(|e| GitError::Gix(e.to_string()))?;
        out.push(commit_from_walk(info, &tags)?);
    }
    Ok(out)
}

/// Corvene `886-history-search`: what a filtered History asks `git log`
/// for. `words` are matched in Rust ([`commit_matches_words`]); the rest
/// are `git log` limits.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HistoryQuery {
    /// Each must appear in the message, the author's name or their e-mail
    /// (any case).
    pub words: Vec<String>,
    /// `--author`, a fixed string matched in any case.
    pub author: Option<String>,
    /// Corvene `1220-history-search-terms`: `--committer`, matched like
    /// `author`.
    pub committer: Option<String>,
    /// `--before` / `--after`: a date git understands (`2024-05-01`,
    /// `2.weeks.ago`).
    pub before: Option<String>,
    pub after: Option<String>,
    /// `887-file-history` (or a `path:` term, `1220-history-search-terms`):
    /// the commits that touched this file or folder, following renames
    /// (`--follow -- <path>`).
    pub path: Option<String>,
}

impl HistoryQuery {
    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
            && self.author.is_none()
            && self.committer.is_none()
            && self.before.is_none()
            && self.after.is_none()
            && self.path.is_none()
    }

    /// A free word that looks like an abbreviated SHA (4 to 40 hex digits),
    /// when it is the only word.
    pub fn sha_prefix(&self) -> Option<&str> {
        match self.words.as_slice() {
            [word]
                if (4..=40).contains(&word.len())
                    && word.bytes().all(|b| b.is_ascii_hexdigit()) =>
            {
                Some(word)
            }
            _ => None,
        }
    }
}

/// One commit of a filtered History's `git log`, in its order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoggedCommit {
    pub sha: String,
    /// For a file history, the file's path in this commit (renames give
    /// the new name).
    pub path: Option<String>,
}

/// Corvene `886-history-search`: the commits reachable from `tip` that
/// match the query's `git log` limits, newest first (`git log -i -F
/// --format=%H [--author] [--committer] [--before] [--after] <tip> [--follow --name-only
/// -- <path>]`). Free words are left to [`filtered_history_page`].
pub fn filtered_history(
    git: Arc<GitBinary>,
    workdir: &Path,
    tip: &str,
    query: &HistoryQuery,
    first_parent: bool,
    cancel: Option<crate::process::CancelToken>,
) -> Result<Vec<LoggedCommit>> {
    let mut args: Vec<String> = vec![
        "-c".into(),
        "core.quotePath=false".into(),
        "log".into(),
        "--regexp-ignore-case".into(),
        "--fixed-strings".into(),
        "--format=%x00%H".into(),
    ];
    if first_parent {
        args.push("--first-parent".into());
    }
    if let Some(author) = &query.author {
        args.push(format!("--author={author}"));
    }
    if let Some(committer) = &query.committer {
        args.push(format!("--committer={committer}"));
    }
    if let Some(before) = &query.before {
        args.push(format!("--before={before}"));
    }
    if let Some(after) = &query.after {
        args.push(format!("--after={after}"));
    }
    if query.path.is_some() {
        args.push("--follow".into());
        args.push("--name-only".into());
    }
    args.push(tip.to_string());
    args.push("--".into());
    if let Some(path) = &query.path {
        args.push(path.clone());
    }
    let mut command = GitCommand::new(git).args(&args).current_dir(workdir);
    if let Some(cancel) = cancel {
        command = command.cancel_token(cancel);
    }
    let out = command.run()?;
    Ok(parse_filtered_history(&out.stdout))
}

/// [`filtered_history`]'s output: `\0<sha>` per commit, each followed by
/// the commit's file names when `--name-only` was asked for.
pub fn parse_filtered_history(stdout: &[u8]) -> Vec<LoggedCommit> {
    String::from_utf8_lossy(stdout)
        .split('\0')
        .filter_map(|entry| {
            let mut lines = entry.lines();
            let sha = lines.next()?.trim();
            (sha.len() >= 40 && sha.bytes().all(|b| b.is_ascii_hexdigit())).then(|| LoggedCommit {
                sha: sha.to_string(),
                path: lines.find(|l| !l.trim().is_empty()).map(str::to_string),
            })
        })
        .collect()
}

/// The commit `revision` names, as a full sha (`rev-parse <revision>^{commit}`).
pub fn resolve_commit(workdir: &Path, revision: &str) -> Result<Option<String>> {
    let repo = crate::handle::open(workdir)?;
    Ok(repo
        .rev_parse_single(revision)
        .ok()
        .and_then(|id| id.object().ok()?.peel_to_commit().ok())
        .map(|c| c.id.to_string()))
}

/// Corvene `886-history-search`: every word appears in the commit's
/// message, author name or e-mail, ignoring case. `words` are lower case.
pub fn commit_matches_words(commit: &Commit, words: &[String]) -> bool {
    if words.is_empty() {
        return true;
    }
    let haystack = haystack(
        &commit.summary,
        &commit.body,
        &commit.author.name,
        &commit.author.email,
    );
    words.iter().all(|w| haystack.contains(w.as_str()))
}

/// What [`commit_matches_words`] searches: the message, author name and
/// e-mail in lower case.
fn haystack(summary: &str, body: &str, name: &str, email: &str) -> String {
    format!("{summary}\n{body}\n{name}\n{email}").to_lowercase()
}

/// [`commit_matches_words`]'s text of commit `id`, without building the
/// whole [`Commit`].
fn commit_haystack(repo: &gix::Repository, id: gix::ObjectId) -> Result<Box<str>> {
    let commit = repo
        .find_commit(id)
        .map_err(|e| GitError::Gix(e.to_string()))?;
    let decoded = commit.decode().map_err(|e| GitError::Gix(e.to_string()))?;
    let (summary, body) = subject_and_body(decoded.message);
    let author = commit.author().map_err(|e| GitError::Gix(e.to_string()))?;
    Ok(haystack(
        &summary,
        &body,
        &author.name.to_string(),
        &author.email.to_string(),
    )
    .into_boxed_str())
}

/// Corvene `886-history-search`: a filtered History's `git log`
/// ([`filtered_history`]) with each commit's searchable text, read the
/// first time a page looks at the commit and kept, so searching the same
/// commits for other words reads no objects.
#[derive(Debug, Default)]
pub struct LoggedHistory {
    pub commits: Vec<LoggedCommit>,
    haystacks: std::sync::Mutex<Vec<Option<Box<str>>>>,
}

impl LoggedHistory {
    pub fn new(commits: Vec<LoggedCommit>) -> Self {
        let haystacks = std::sync::Mutex::new(vec![None; commits.len()]);
        Self { commits, haystacks }
    }

    pub fn len(&self) -> usize {
        self.commits.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commits.is_empty()
    }
}

/// One page of a filtered History: from `logged[start..]`, the commits that
/// match `words` ([`commit_matches_words`]) and are not in `skip`, until
/// `limit` of them are found. Returns them and where the next page starts.
/// Stops with [`GitError::Cancelled`] once `cancel` is cancelled.
pub fn filtered_history_page(
    workdir: &Path,
    logged: &LoggedHistory,
    start: usize,
    words: &[String],
    skip: &[String],
    limit: usize,
    cancel: Option<&crate::process::CancelToken>,
) -> Result<(Vec<Commit>, usize)> {
    let repo = crate::handle::open(workdir)?;
    let mut tags = None;
    let mut haystacks = logged
        .haystacks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut out = Vec::new();
    let mut next = start;
    for (ix, entry) in logged.commits.iter().enumerate().skip(start) {
        if out.len() >= limit {
            break;
        }
        if ix % 256 == 0 && cancel.is_some_and(|c| c.is_cancelled()) {
            return Err(GitError::Cancelled("history filter".into()));
        }
        next = ix + 1;
        if skip.contains(&entry.sha) {
            continue;
        }
        let Ok(id) = gix::ObjectId::from_hex(entry.sha.as_bytes()) else {
            continue;
        };
        if !words.is_empty() {
            let text = match &mut haystacks[ix] {
                Some(text) => text,
                slot => slot.insert(commit_haystack(&repo, id)?),
            };
            if !words.iter().all(|w| text.contains(w.as_str())) {
                continue;
            }
        }
        let tags = tags.get_or_insert_with(|| tags_by_commit(&repo));
        out.push(commit_from_id(&repo, id, tags)?);
    }
    Ok((out, next))
}

/// The commits whose sha starts with `prefix` among `logged` (at most
/// `limit`), for a SHA typed in History's filter box.
pub fn commits_with_sha_prefix(
    workdir: &Path,
    logged: &[LoggedCommit],
    prefix: &str,
    limit: usize,
) -> Result<Vec<Commit>> {
    let prefix = prefix.to_ascii_lowercase();
    let repo = crate::handle::open(workdir)?;
    let tags = tags_by_commit(&repo);
    logged
        .iter()
        .filter(|c| c.sha.starts_with(&prefix))
        .take(limit)
        .filter_map(|c| gix::ObjectId::from_hex(c.sha.as_bytes()).ok())
        .map(|id| commit_from_id(&repo, id, &tags))
        .collect()
}

/// The newest commit on the current branch that no remote has (GHD
/// `GitStore.loadLocalCommits`, first of `localCommitSHAs`): `upstream..branch`
/// when the branch tracks one, else `HEAD --not --remotes`. Feeds the changes
/// sidebar's "Committed … Undo" bar.
pub fn most_recent_local_commit(
    workdir: &Path,
    branch: &str,
    upstream: Option<&str>,
) -> Result<Option<Commit>> {
    if let Some(upstream) = upstream {
        return Ok(get_commits_in_range(workdir, upstream, branch, 1)?
            .into_iter()
            .next());
    }
    let repo = crate::handle::open(workdir)?;
    let Ok(head) = repo.rev_parse_single("HEAD") else {
        return Ok(None);
    };
    let mut hidden = Vec::new();
    if let Ok(refs) = repo.references()
        && let Ok(iter) = refs.remote_branches()
    {
        for r in iter.flatten() {
            if let Ok(id) = r.into_fully_peeled_id() {
                hidden.push(id.detach());
            }
        }
    }
    let walk = repo
        .rev_walk([head.detach()])
        .with_hidden(hidden)
        .sorting(gix::revision::walk::Sorting::ByCommitTime(
            gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
        ))
        .all()
        .map_err(|e| GitError::Gix(e.to_string()))?;
    let Some(info) = walk.take(1).next() else {
        return Ok(None);
    };
    let info = info.map_err(|e| GitError::Gix(e.to_string()))?;
    // the undo bar shows no tags
    Ok(Some(commit_from_walk(info, &HashMap::new())?))
}

/// The current branch's commits no remote has, newest first (GHD
/// `GitStore.loadLocalCommits` / `localCommitSHAs`): `upstream..branch` when
/// the branch tracks one, else `HEAD --not --remotes`. `skip` pages them
/// alongside the history list; at most `limit` come back. Feeds the history
/// rows' unpushed indicator.
pub fn local_commit_shas(
    workdir: &Path,
    branch: &str,
    upstream: Option<&str>,
    skip: usize,
    limit: usize,
) -> Result<Vec<String>> {
    let repo = crate::handle::open(workdir)?;
    let tip = match upstream {
        Some(_) => branch,
        None => "HEAD",
    };
    let Ok(tip) = repo.rev_parse_single(tip) else {
        return Ok(Vec::new());
    };
    let mut hidden = Vec::new();
    match upstream {
        // `revRange(branch.upstream, branch.name)`
        Some(upstream) => match repo.rev_parse_single(upstream) {
            Ok(id) => hidden.push(id.detach()),
            // the upstream is gone: nothing to hide behind
            Err(_) => return Ok(Vec::new()),
        },
        // `HEAD --not --remotes`
        None => {
            if let Ok(refs) = repo.references()
                && let Ok(iter) = refs.remote_branches()
            {
                for r in iter.flatten() {
                    if let Ok(id) = r.into_fully_peeled_id() {
                        hidden.push(id.detach());
                    }
                }
            }
        }
    }
    let walk = repo
        .rev_walk([tip.detach()])
        .with_hidden(hidden)
        .sorting(gix::revision::walk::Sorting::ByCommitTime(
            gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
        ))
        .all()
        .map_err(|e| GitError::Gix(e.to_string()))?;
    let mut out = Vec::new();
    for info in walk.skip(skip).take(limit) {
        let info = info.map_err(|e| GitError::Gix(e.to_string()))?;
        out.push(info.id.to_hex().to_string());
    }
    Ok(out)
}

/// Corvene `883-unpublished-commit-links`: the commits reachable from `tip`
/// but from no remote-tracking branch (`rev-list <tip> --not --remotes`),
/// newest first, at most `limit`. They are not on any remote, so links to
/// them on GitHub would be dead.
pub fn local_only_commits(
    git: Arc<GitBinary>,
    workdir: &Path,
    tip: &str,
    limit: usize,
) -> Result<Vec<String>> {
    let out = GitCommand::new(git)
        .args([
            "rev-list",
            &format!("--max-count={limit}"),
            tip,
            "--not",
            "--remotes",
        ])
        .current_dir(workdir)
        .run()?;
    Ok(out
        .stdout_string()?
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

/// Commits reachable from `to` but not from `from` (`from..to`), newest
/// first, at most `limit` (GHD `getCommits(repository, revRange(from, to))`).
pub fn get_commits_in_range(
    workdir: &Path,
    from: &str,
    to: &str,
    limit: usize,
) -> Result<Vec<Commit>> {
    let repo = crate::handle::open(workdir)?;
    let (Ok(from_id), Ok(to_id)) = (repo.rev_parse_single(from), repo.rev_parse_single(to)) else {
        return Ok(Vec::new());
    };
    let tags = tags_by_commit(&repo);
    let walk = repo
        .rev_walk([to_id.detach()])
        .with_hidden([from_id.detach()])
        .sorting(gix::revision::walk::Sorting::ByCommitTime(
            gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
        ))
        .all()
        .map_err(|e| GitError::Gix(e.to_string()))?;
    let mut out = Vec::new();
    for info in walk.take(limit) {
        let info = info.map_err(|e| GitError::Gix(e.to_string()))?;
        out.push(commit_from_walk(info, &tags)?);
    }
    Ok(out)
}

/// Corvene `1221-stacked-branch-refs`: the commits `git log <base>..HEAD`
/// lists and where `base` begins below them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StackRange {
    /// Newest first by commit date, as History lists them.
    pub shas: Vec<String>,
    /// The newest commit (by commit date) of HEAD's history that `base`
    /// has: the first History row after the range's commits begin to give
    /// way to `base`'s, its merge base with HEAD in a linear history.
    pub boundary: Option<String>,
}

/// Corvene `1221-stacked-branch-refs`: [`StackRange`] of `base..HEAD`.
/// `None` when either does not resolve or more than `limit` commits are in
/// the range (a branch that far from `base` is not a stack worth marking).
pub fn stack_range(workdir: &Path, base: &str, limit: usize) -> Result<Option<StackRange>> {
    let repo = crate::handle::open(workdir)?;
    let (Ok(head), Ok(base)) = (repo.rev_parse_single("HEAD"), repo.rev_parse_single(base)) else {
        return Ok(None);
    };
    let walk = repo
        .rev_walk([head.detach()])
        .with_hidden([base.detach()])
        .sorting(gix::revision::walk::Sorting::ByCommitTime(
            gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
        ))
        .all()
        .map_err(|e| GitError::Gix(e.to_string()))?;
    let mut in_range = std::collections::HashSet::new();
    let mut parents = Vec::new();
    let mut shas = Vec::new();
    for info in walk {
        let info = info.map_err(|e| GitError::Gix(e.to_string()))?;
        if shas.len() == limit {
            return Ok(None);
        }
        in_range.insert(info.id);
        parents.extend(info.parent_ids.iter().copied());
        shas.push(info.id.to_string());
    }
    let mut boundary: Option<(i64, gix::ObjectId)> = None;
    for parent in parents {
        if in_range.contains(&parent) || boundary.is_some_and(|(_, id)| id == parent) {
            continue;
        }
        let Ok(commit) = repo.find_commit(parent) else {
            continue;
        };
        let time = commit
            .committer()
            .ok()
            .and_then(|c| c.time().ok())
            .map(|t| t.seconds)
            .unwrap_or_default();
        if boundary.is_none_or(|(newest, _)| time > newest) {
            boundary = Some((time, parent));
        }
    }
    Ok(Some(StackRange {
        shas,
        boundary: boundary.map(|(_, id)| id.to_string()),
    }))
}

/// `getChangedFiles`: `log <sha> -C -M -m -1 --first-parent --raw --numstat -z`.
///
/// `in_process` reads them with gitoxide (`log_gix.rs`), git only when that
/// fails. Flag `907-in-process-commit-files`.
pub fn get_changed_files(
    git: Arc<GitBinary>,
    workdir: &Path,
    sha: &str,
    in_process: bool,
) -> Result<ChangesetData> {
    if in_process && let Some(data) = crate::log_gix::changed_files(workdir, sha, sha) {
        return Ok(data);
    }
    let out = GitCommand::new(git)
        .args([
            "log",
            sha,
            "-C",
            "-M",
            "-m",
            "-1",
            "--no-show-signature",
            "--first-parent",
            "--raw",
            "--format=format:",
            "--numstat",
            "-z",
            "--",
        ])
        .current_dir(workdir)
        .run()?;
    Ok(parse_raw_log_with_numstat(&out.stdout, sha))
}

/// Flag `907-in-process-commit-files`'s reader on its own: the files
/// [`get_changed_files`] (`oldest` and `newest` the same sha) or
/// [`get_commit_range_changed_files`] (the range's oldest and newest sha)
/// return, read by gitoxide, or `None` where those run git instead. For
/// tests that hold the two side by side.
pub fn get_changed_files_in_process(
    workdir: &Path,
    oldest: &str,
    newest: &str,
) -> Option<ChangesetData> {
    crate::log_gix::changed_files(workdir, oldest, newest)
}

/// git's file mode for a submodule (GHD `SubmoduleFileMode`).
const SUBMODULE_FILE_MODE: &str = "160000";

/// GHD `mapSubmoduleStatusFileModes` (`lib/git/log.ts`): a committed
/// submodule (file mode `160000`) is modified (`M`, both modes: its commit
/// changed), added (`A`) or deleted (`D`); anything else is not a submodule
/// change. `src` / `dst`: whether the old / new file mode is a submodule's
/// (`log_gix.rs` reads the modes from the trees).
pub(crate) fn map_submodule_status_file_modes(
    status: &str,
    src: bool,
    dst: bool,
) -> Option<corvene_models::SubmoduleStatus> {
    if src && dst && status == "M" {
        Some(corvene_models::SubmoduleStatus {
            commit_changed: true,
            ..Default::default()
        })
    } else if (src && status == "D") || (dst && status == "A") {
        Some(corvene_models::SubmoduleStatus::default())
    } else {
        None
    }
}

/// Parse `--raw --numstat -z` output (GHD `parseRawLogWithNumstat`).
///
/// Raw entries: `:<old mode> <new mode> <old sha> <new sha> <status>[score]` NUL
/// path [NUL path2 for renames/copies]; numstat entries: `added TAB deleted TAB`
/// path (renames: NUL old NUL new).
pub fn parse_raw_log_with_numstat(stdout: &[u8], sha: &str) -> ChangesetData {
    let text = String::from_utf8_lossy(stdout);
    let mut fields = text.split('\0').peekable();
    let mut data = ChangesetData::default();
    while let Some(field) = fields.next() {
        let field = field.trim_start_matches('\n');
        if field.is_empty() {
            continue;
        }
        if let Some(raw) = field.strip_prefix(':') {
            // ":100644 100644 5716ca5 db3c77d M" or "R100"
            let status = raw.split(' ').nth(4).unwrap_or("");
            // the status letter, then the score; a malformed entry may have
            // no status at all
            let (letter, score) = status.split_at(status.chars().next().map_or(0, char::len_utf8));
            let score = score.parse::<u8>().ok();
            let first = fields.next().unwrap_or("").to_string();
            let (path, old_path) = if matches!(letter, "R" | "C") {
                let second = fields.next().unwrap_or("").to_string();
                (second, Some(first))
            } else {
                (first, None)
            };
            let mut modes = raw.split(' ');
            let submodule_status = map_submodule_status_file_modes(
                status,
                modes.next() == Some(SUBMODULE_FILE_MODE),
                modes.next() == Some(SUBMODULE_FILE_MODE),
            );
            data.files.push(committed_file(
                letter,
                score,
                path,
                old_path,
                submodule_status,
                sha,
            ));
        } else {
            // numstat: "added\tdeleted\tpath" - "-" for binary files
            let mut parts = field.splitn(3, '\t');
            let added = parts.next().and_then(|n| n.parse::<u64>().ok());
            let deleted = parts.next().and_then(|n| n.parse::<u64>().ok());
            let path = parts.next().unwrap_or("");
            if path.is_empty() {
                // rename numstat: "added\tdeleted\t" NUL old NUL new
                fields.next();
                fields.next();
            }
            data.lines_added += added.unwrap_or(0);
            data.lines_deleted += deleted.unwrap_or(0);
        }
    }
    data
}

/// One `--raw` record as a file change: `letter` is git's status letter
/// (`A`, `D`, `M`, `T`, `R`, `C`, `U`), `submodule_status` what
/// [`map_submodule_status_file_modes`] made of its modes.
pub(crate) fn committed_file(
    letter: &str,
    score: Option<u8>,
    path: String,
    old_path: Option<String>,
    submodule_status: Option<corvene_models::SubmoduleStatus>,
    sha: &str,
) -> CommittedFileChange {
    let kind = match letter {
        "A" => FileStatusKind::New,
        "D" => FileStatusKind::Deleted,
        "R" => FileStatusKind::Renamed,
        "C" => FileStatusKind::Copied,
        "U" => FileStatusKind::Conflicted,
        _ => FileStatusKind::Modified,
    };
    let entry = match kind {
        FileStatusKind::New => GitStatusEntry::Added,
        FileStatusKind::Deleted => GitStatusEntry::Deleted,
        FileStatusKind::Renamed => GitStatusEntry::Renamed,
        FileStatusKind::Copied => GitStatusEntry::Copied,
        FileStatusKind::Conflicted => GitStatusEntry::Unmerged,
        _ => GitStatusEntry::Modified,
    };
    CommittedFileChange {
        path,
        old_path,
        status: FileStatus {
            kind,
            index: entry,
            working_tree: GitStatusEntry::Unchanged,
            score,
            code: letter.to_string(),
            submodule: submodule_status.is_some(),
            submodule_status,
            conflict_markers: None,
        },
        commitish: sha.to_string(),
    }
}

/// The empty tree, used as the parent of a root commit (GHD `NullTreeSHA`).
pub const NULL_TREE_SHA: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

pub(crate) fn is_bad_revision(err: &crate::error::GitError) -> bool {
    matches!(err, crate::error::GitError::Failed { stderr, .. }
        if stderr.contains("bad revision") || stderr.contains("unknown revision"))
}

/// `getCommitRangeChangedFiles`: files changed between `shas[0]^` and the
/// newest sha (`shas` oldest first). Falls back to the empty tree when the
/// oldest commit is a root commit. `in_process`: see [`get_changed_files`].
pub fn get_commit_range_changed_files(
    git: Arc<GitBinary>,
    workdir: &Path,
    shas: &[String],
    in_process: bool,
) -> Result<ChangesetData> {
    let (Some(oldest), Some(newest)) = (shas.first(), shas.last()) else {
        return Ok(ChangesetData::default());
    };
    if in_process && let Some(data) = crate::log_gix::changed_files(workdir, oldest, newest) {
        return Ok(data);
    }
    let run = |base: &str| {
        GitCommand::new(git.clone())
            .args([
                "diff",
                base,
                newest,
                "-C",
                "-M",
                "-z",
                "--raw",
                "--numstat",
                "--",
            ])
            .current_dir(workdir)
            .run()
    };
    let out = match run(&format!("{oldest}^")) {
        Ok(out) => out,
        Err(err) if is_bad_revision(&err) => run(NULL_TREE_SHA)?,
        Err(err) => return Err(err),
    };
    Ok(parse_raw_log_with_numstat(&out.stdout, newest))
}

/// `getCommitRangeDiff`: one file's patch across a range of commits.
pub fn commit_range_file_diff(
    git: Arc<GitBinary>,
    workdir: &Path,
    file: &CommittedFileChange,
    oldest: &str,
    newest: &str,
    hide_whitespace: bool,
) -> Result<Diff> {
    let run = |base: &str| {
        let mut args = vec!["diff", base, newest];
        if hide_whitespace {
            args.push("-w");
        }
        args.extend(["--patch-with-raw", "--format=", "-z", "--no-color", "--"]);
        let mut cmd = GitCommand::new(git.clone())
            .args(args)
            .current_dir(workdir)
            .arg(&file.path);
        if let Some(old) = &file.old_path {
            cmd = cmd.arg(old);
        }
        cmd.run()
    };
    let out = match run(&format!("{oldest}^")) {
        Ok(out) => out,
        Err(err) if is_bad_revision(&err) => run(NULL_TREE_SHA)?,
        Err(err) => return Err(err),
    };
    Ok(finish_committed_diff(
        git,
        workdir,
        file,
        newest,
        &format!("{oldest}^"),
        &out.stdout,
        hide_whitespace,
        true,
    ))
}

/// Shared tail of the committed-diff loaders: submodule and image diffs need
/// the blobs on both sides (`getImageDiff`, `buildSubmoduleDiff`), and so
/// does a UTF-16 text file git took for binary (Corvene `1306-utf16-diffs`,
/// [`crate::utf16`]; `utf16` false where `base` is not the patch's old
/// side, and `hide_whitespace` as the patch was made).
#[allow(clippy::too_many_arguments)]
fn finish_committed_diff(
    git: Arc<GitBinary>,
    workdir: &Path,
    file: &CommittedFileChange,
    newest: &str,
    base: &str,
    patch: &[u8],
    hide_whitespace: bool,
    utf16: bool,
) -> Diff {
    if file.status.submodule {
        return crate::diff::submodule_diff(
            git,
            workdir,
            &file.path,
            file.status.submodule_status.unwrap_or_default(),
            file.status.kind,
            patch,
        );
    }
    let previous_path = file.old_path.as_deref().unwrap_or(&file.path);
    match crate::diff::parse_raw_diff(patch) {
        Diff::Binary
            if utf16
                && crate::diff::binary_media_type(&file.path).is_none()
                && crate::table_file::table_format(&file.path).is_none()
                && crate::archive::archive_format(&file.path).is_none()
                && crate::utf16::decode_utf16() =>
        {
            use crate::utf16::Side;
            let new = if file.status.kind == FileStatusKind::Deleted {
                Side::Missing
            } else {
                Side::Blob(newest, &file.path)
            };
            let old = if file.status.kind.is_new_or_untracked() {
                Side::Missing
            } else {
                Side::Blob(base, previous_path)
            };
            crate::utf16::text_diff(git.clone(), workdir, new, old, hide_whitespace, patch, None)
                .unwrap_or(Diff::Binary)
        }
        diff if matches!(diff, Diff::Binary) || crate::diff::pdf_as_text(&file.path, &diff) => {
            // `1323-archive-diff`
            use crate::archive::SideRef;
            let new = if file.status.kind == FileStatusKind::Deleted {
                SideRef::Missing
            } else {
                SideRef::Blob(newest, &file.path)
            };
            let old = if file.status.kind.is_new_or_untracked() {
                SideRef::Missing
            } else {
                SideRef::Blob(base, previous_path)
            };
            if let Some(archive) =
                crate::archive::archive_diff(git.clone(), workdir, &file.path, new, old)
            {
                return archive;
            }
            crate::diff::image_diff(
                &file.path,
                file.status.kind,
                || crate::diff::blob_bytes(git.clone(), workdir, newest, &file.path).ok(),
                || crate::diff::blob_bytes(git.clone(), workdir, base, previous_path).ok(),
            )
        }
        other => other,
    }
}

/// `getCommitDiff`: the patch for one file of a commit.
pub fn commit_file_diff(
    git: Arc<GitBinary>,
    workdir: &Path,
    file: &CommittedFileChange,
    hide_whitespace: bool,
) -> Result<Diff> {
    committed_file_diff(git, workdir, file, hide_whitespace, false)
}

/// Corvene `797-stash-list`: [`commit_file_diff`] for a stash entry's file.
/// An untracked file's commitish is the entry's untracked-files commit, a
/// root commit, whose patch `git log` leaves out under `log.showRoot=false`
/// unless asked with `--root`.
pub fn stash_file_diff(
    git: Arc<GitBinary>,
    workdir: &Path,
    file: &CommittedFileChange,
    hide_whitespace: bool,
) -> Result<Diff> {
    committed_file_diff(git, workdir, file, hide_whitespace, true)
}

fn committed_file_diff(
    git: Arc<GitBinary>,
    workdir: &Path,
    file: &CommittedFileChange,
    hide_whitespace: bool,
    root: bool,
) -> Result<Diff> {
    let mut args = vec!["log", file.commitish.as_str()];
    if hide_whitespace {
        args.push("-w");
    }
    if root {
        args.push("--root");
    }
    args.extend([
        "-m",
        "-1",
        "--first-parent",
        "--no-show-signature",
        "--patch-with-raw",
        "-z",
        "--no-color",
        "--format=format:",
        "--",
    ]);
    let mut cmd = GitCommand::new(git.clone())
        .args(args)
        .current_dir(workdir)
        .arg(&file.path);
    if let Some(old) = &file.old_path {
        cmd = cmd.arg(old);
    }
    let out = cmd.run()?;
    Ok(finish_committed_diff(
        git,
        workdir,
        file,
        &file.commitish,
        &format!("{}^", file.commitish),
        &out.stdout,
        hide_whitespace,
        true,
    ))
}

/// Corvene `773-merge-remerge-diff`: the oldest git with `--remerge-diff`.
pub const REMERGE_DIFF_MIN_VERSION: (u32, u32) = (2, 36);

/// Corvene `773-merge-remerge-diff`: the files of merge commit `sha` that
/// differ from git's automatic re-merge of its parents, i.e. the conflict
/// resolutions and any other edit made in the merge (`show --remerge-diff
/// --format= --raw --numstat -z <sha>`). Empty for a clean merge.
pub fn remerge_changed_files(
    git: Arc<GitBinary>,
    workdir: &Path,
    sha: &str,
) -> Result<ChangesetData> {
    let out = GitCommand::new(git)
        .args([
            "show",
            sha,
            "--remerge-diff",
            "--no-show-signature",
            "--raw",
            "--format=format:",
            "--numstat",
            "-z",
            "--",
        ])
        .current_dir(workdir)
        .run()?;
    Ok(parse_raw_log_with_numstat(&out.stdout, sha))
}

/// Corvene `773-merge-remerge-diff`: one file of
/// [`remerge_changed_files`], from the re-merge (conflict markers and all)
/// to what the merge commit recorded.
pub fn remerge_file_diff(
    git: Arc<GitBinary>,
    workdir: &Path,
    file: &CommittedFileChange,
    hide_whitespace: bool,
) -> Result<Diff> {
    let mut args = vec!["show", file.commitish.as_str(), "--remerge-diff"];
    if hide_whitespace {
        args.push("-w");
    }
    args.extend([
        "--no-show-signature",
        "--patch-with-raw",
        "-z",
        "--no-color",
        "--format=format:",
        "--",
    ]);
    let out = GitCommand::new(git.clone())
        .args(args)
        .current_dir(workdir)
        .arg(&file.path)
        .run()?;
    Ok(finish_committed_diff(
        git,
        workdir,
        file,
        &file.commitish,
        &format!("{}^", file.commitish),
        &out.stdout,
        hide_whitespace,
        false,
    ))
}

/// Whether `commitish` names a commit the repository has (`git cat-file -e
/// <commitish>^{commit}`): a pull request head may not be fetched yet.
pub fn commit_exists(git: Arc<GitBinary>, workdir: &Path, commitish: &str) -> bool {
    GitCommand::new(git)
        .args(["cat-file", "-e", &format!("{commitish}^{{commit}}")])
        .current_dir(workdir)
        .allow_exit_code(1)
        .allow_exit_code(128)
        .run()
        .is_ok_and(|out| out.status.success())
}

/// `getMergeBase`: `None` when the two commits have unrelated histories
/// (exit code 1) or a ref cannot be found (128).
pub fn merge_base(git: Arc<GitBinary>, workdir: &Path, a: &str, b: &str) -> Result<Option<String>> {
    let out = GitCommand::new(git)
        .args(["merge-base", a, b])
        .current_dir(workdir)
        .allow_exit_code(1)
        .allow_exit_code(128)
        .run()?;
    if !out.status.success() {
        return Ok(None);
    }
    let sha = out.stdout_string()?.trim().to_string();
    Ok((!sha.is_empty()).then_some(sha))
}

/// `getBranchMergeBaseChangedFiles`: the files `compare` changed since it
/// diverged from `base` (`git diff --merge-base`), `None` without a merge
/// base. `newest` is the comparison branch's tip, recorded as the files'
/// commitish.
pub fn merge_base_changed_files(
    git: Arc<GitBinary>,
    workdir: &Path,
    base: &str,
    compare: &str,
    newest: &str,
) -> Result<Option<ChangesetData>> {
    range_changed_files(git, workdir, base, compare, true, newest)
}

/// `getBranchMergeBaseDiff`: one file's patch between the merge base of
/// `base`/`compare` and `compare`.
pub fn merge_base_file_diff(
    git: Arc<GitBinary>,
    workdir: &Path,
    file: &CommittedFileChange,
    base: &str,
    compare: &str,
    hide_whitespace: bool,
    newest: &str,
) -> Result<Diff> {
    range_file_diff(
        git,
        workdir,
        file,
        base,
        compare,
        true,
        hide_whitespace,
        newest,
    )
}

/// The files that changed between two commits: from their merge base to
/// `to` with `merge_base` (`git diff from...to`, `None` when the two have
/// no merge base), else straight from `from` to `to` (`git diff from to`).
/// `newest` is `to`'s commit, recorded as the files' commitish. Corvene
/// `1218-compare-refs` compares any two refs both ways; Preview Pull
/// Request always from the merge base.
pub fn range_changed_files(
    git: Arc<GitBinary>,
    workdir: &Path,
    from: &str,
    to: &str,
    merge_base: bool,
    newest: &str,
) -> Result<Option<ChangesetData>> {
    if merge_base && self::merge_base(git.clone(), workdir, from, to)?.is_none() {
        return Ok(None);
    }
    let mut args = vec!["diff"];
    if merge_base {
        args.push("--merge-base");
    }
    args.extend([from, to, "-C", "-M", "-z", "--raw", "--numstat", "--"]);
    let out = GitCommand::new(git).args(args).current_dir(workdir).run()?;
    Ok(Some(parse_raw_log_with_numstat(&out.stdout, newest)))
}

/// One file's patch of [`range_changed_files`]: from the merge base of
/// `from` and `to` with `merge_base`, else from `from`, to `to`.
#[allow(clippy::too_many_arguments)]
pub fn range_file_diff(
    git: Arc<GitBinary>,
    workdir: &Path,
    file: &CommittedFileChange,
    from: &str,
    to: &str,
    merge_base: bool,
    hide_whitespace: bool,
    newest: &str,
) -> Result<Diff> {
    let mut args = vec!["diff"];
    if merge_base {
        args.push("--merge-base");
    }
    args.extend([from, to]);
    if hide_whitespace {
        args.push("-w");
    }
    args.extend(["--patch-with-raw", "-z", "--no-color", "--"]);
    let mut cmd = GitCommand::new(git.clone())
        .args(args)
        .current_dir(workdir)
        .arg(&file.path);
    if let Some(old) = &file.old_path {
        cmd = cmd.arg(old);
    }
    let out = cmd.run()?;
    let old = if merge_base {
        self::merge_base(git.clone(), workdir, from, to)?.unwrap_or_else(|| newest.to_string())
    } else {
        from.to_string()
    };
    Ok(finish_committed_diff(
        git,
        workdir,
        file,
        newest,
        &old,
        &out.stdout,
        hide_whitespace,
        true,
    ))
}

/// The distinct authors (name, email) of the newest `limit` commits
/// reachable from HEAD, most recent first, one per email (case ignored).
/// Corvene `770-co-authors-from-history`: co-author suggestions for people
/// without a GitHub account. An unborn branch has none.
pub fn recent_authors(
    git: Arc<GitBinary>,
    workdir: &Path,
    limit: usize,
) -> Result<Vec<(String, String)>> {
    let out = GitCommand::new(git)
        .args([
            "log",
            "-n",
            &limit.to_string(),
            "--format=%an%x1f%ae",
            "HEAD",
            "--",
        ])
        .current_dir(workdir)
        .allow_exit_code(128)
        .run()?;
    if !out.status.success() {
        return Ok(Vec::new());
    }
    Ok(parse_recent_authors(&out.stdout_string()?))
}

/// `%an%x1f%ae` lines to distinct (name, email) pairs, first seen wins.
pub fn parse_recent_authors(text: &str) -> Vec<(String, String)> {
    let mut seen = std::collections::HashSet::new();
    text.lines()
        .filter_map(|line| line.split_once('\u{1f}'))
        .map(|(name, email)| (name.trim().to_string(), email.trim().to_string()))
        .filter(|(name, email)| !name.is_empty() && email.contains('@'))
        .filter(|(_, email)| seen.insert(email.to_lowercase()))
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn filtered_history_output_lists_shas() {
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let out = format!("\0{a}\n\0{b}\n\nsrc/x.rs\n");
        let logged = parse_filtered_history(out.as_bytes());
        assert_eq!(
            logged,
            vec![
                LoggedCommit { sha: a, path: None },
                LoggedCommit {
                    sha: b,
                    path: Some("src/x.rs".into())
                }
            ]
        );
    }

    #[test]
    fn words_match_message_or_author() {
        let author = CommitIdentity {
            name: "Mona Lisa".into(),
            email: "mona@example.com".into(),
            seconds: 0,
            offset: 0,
        };
        let commit = Commit {
            sha: "a".repeat(40),
            summary: "Fix the Login form".into(),
            body: "Closes #3\n".into(),
            author: author.clone(),
            committer: author,
            parents: Vec::new(),
            trailers: Vec::new(),
            tags: Vec::new(),
            signature: None,
        };
        let words = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(commit_matches_words(&commit, &words(&["login", "mona"])));
        assert!(commit_matches_words(&commit, &words(&["example.com"])));
        assert!(commit_matches_words(&commit, &words(&["#3"])));
        assert!(!commit_matches_words(&commit, &words(&["login", "hubot"])));
        assert!(commit_matches_words(&commit, &[]));
    }

    #[test]
    fn a_lone_hex_word_is_a_sha_prefix() {
        let query = |w: &[&str]| HistoryQuery {
            words: w.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        assert_eq!(query(&["a1b2"]).sha_prefix(), Some("a1b2"));
        assert_eq!(query(&["a1b"]).sha_prefix(), None);
        assert_eq!(query(&["fix"]).sha_prefix(), None);
        assert_eq!(query(&["a1b2", "c3d4"]).sha_prefix(), None);
    }

    #[test]
    fn recent_authors_are_distinct_by_email() {
        let text = "Ann\u{1f}ann@x.io\nBob\u{1f}bob@x.io\nAnn B\u{1f}ANN@x.io\nNo Mail\u{1f}\n";
        assert_eq!(
            parse_recent_authors(text),
            vec![
                ("Ann".to_string(), "ann@x.io".to_string()),
                ("Bob".to_string(), "bob@x.io".to_string()),
            ]
        );
    }

    use super::*;
    use std::process::Command;

    #[test]
    fn stack_range_lists_the_commits_after_base() {
        let (dir, _) = repo();
        let path = dir.path();
        let run = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(path)
                    .env("GIT_AUTHOR_NAME", "Ada")
                    .env("GIT_AUTHOR_EMAIL", "ada@example.com")
                    .env("GIT_COMMITTER_NAME", "Ada")
                    .env("GIT_COMMITTER_EMAIL", "ada@example.com")
                    .status()
                    .unwrap()
                    .success()
            )
        };
        let main_tip = get_commits(path, "main", 0, 1).unwrap()[0].sha.clone();
        run(&["checkout", "-q", "-b", "a"]);
        run(&["commit", "-q", "--allow-empty", "-m", "A"]);
        run(&["checkout", "-q", "-b", "b"]);
        run(&["commit", "-q", "--allow-empty", "-m", "B"]);
        let a = get_commits(path, "a", 0, 1).unwrap()[0].sha.clone();
        let b = get_commits(path, "b", 0, 1).unwrap()[0].sha.clone();
        let range = stack_range(path, "main", 10).unwrap().unwrap();
        let mut shas = range.shas.clone();
        shas.sort();
        let mut expected = vec![a.clone(), b];
        expected.sort();
        assert_eq!(shas, expected);
        assert_eq!(range.boundary.as_deref(), Some(main_tip.as_str()));
        // too long a range, or no such base: nothing
        assert_eq!(stack_range(path, "main", 1).unwrap(), None);
        assert_eq!(stack_range(path, "nope", 10).unwrap(), None);
        // HEAD on the base: an empty range
        run(&["checkout", "-q", "main"]);
        assert_eq!(
            stack_range(path, "a", 10).unwrap().unwrap().shas,
            Vec::<String>::new()
        );
    }

    #[test]
    fn local_only_commits_exclude_remote_ones() {
        let (dir, git) = repo();
        let all = local_only_commits(git.clone(), dir.path(), "HEAD", 100).unwrap();
        assert_eq!(all.len(), 2);
        // a remote-tracking ref at the first commit publishes it
        let status = Command::new("git")
            .args(["update-ref", "refs/remotes/origin/main", "HEAD~1"])
            .current_dir(dir.path())
            .status()
            .unwrap();
        assert!(status.success());
        let local = local_only_commits(git, dir.path(), "HEAD", 100).unwrap();
        assert_eq!(local, all[..1]);
    }

    #[test]
    fn local_commit_shas_follow_the_upstream_then_every_remote() {
        let (dir, _) = repo();
        let all = get_commits(dir.path(), "HEAD", 0, 10).unwrap();
        let run = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(dir.path())
                    .status()
                    .unwrap()
                    .success()
            )
        };
        // no upstream: `HEAD --not --remotes`
        let local = local_commit_shas(dir.path(), "main", None, 0, 10).unwrap();
        assert_eq!(local, [all[0].sha.clone(), all[1].sha.clone()]);
        run(&["update-ref", "refs/remotes/origin/main", "HEAD~1"]);
        let local = local_commit_shas(dir.path(), "main", None, 0, 10).unwrap();
        assert_eq!(local, [all[0].sha.clone()]);
        // with one: `upstream..branch`, whatever other remotes hold
        run(&["update-ref", "refs/remotes/other/main", "HEAD"]);
        let local =
            local_commit_shas(dir.path(), "main", Some("refs/remotes/origin/main"), 0, 10).unwrap();
        assert_eq!(local, [all[0].sha.clone()]);
        // paged like the history list
        let local = local_commit_shas(dir.path(), "main", None, 1, 10).unwrap();
        assert!(local.is_empty());
        // an upstream that is gone leaves nothing marked
        let local = local_commit_shas(dir.path(), "main", Some("refs/remotes/gone/main"), 0, 10);
        assert_eq!(local.unwrap(), Vec::<String>::new());
    }

    fn repo() -> (tempfile::TempDir, Arc<GitBinary>) {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(dir.path())
                    // explicit identity: another test sets GIT_AUTHOR_NAME process-wide
                    .env("GIT_AUTHOR_NAME", "Ada")
                    .env("GIT_AUTHOR_EMAIL", "ada@example.com")
                    .env("GIT_COMMITTER_NAME", "Ada")
                    .env("GIT_COMMITTER_EMAIL", "ada@example.com")
                    .env("GIT_AUTHOR_DATE", "2024-01-02T03:04:05+0000")
                    .env("GIT_COMMITTER_DATE", "2024-01-02T03:04:05+0000")
                    .status()
                    .unwrap()
                    .success()
            )
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "commit.gpgsign", "false"]);
        run(&["config", "user.name", "Ada"]);
        run(&["config", "user.email", "ada@example.com"]);
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "first\n\nbody line"]);
        std::fs::write(dir.path().join("a.txt"), "one\nTWO\nthree\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "b\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "second"]);
        run(&["tag", "v1"]);
        (dir, Arc::new(crate::find_git().unwrap()))
    }

    #[test]
    fn walks_commits_newest_first_with_tags() {
        let (dir, _) = repo();
        let commits = get_commits(dir.path(), "HEAD", 0, 10).unwrap();
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].summary, "second");
        assert_eq!(commits[0].tags, vec!["v1".to_string()]);
        assert_eq!(commits[1].summary, "first");
        // git's `%b` keeps the final newline
        assert_eq!(commits[1].body, "body line\n");
        assert_eq!(commits[1].author.name, "Ada");
        assert_eq!(commits[1].author.seconds, 1704164645);
        assert_eq!(commits[0].parents, vec![commits[1].sha.clone()]);
        assert_eq!(tag_names(dir.path()).unwrap(), ["v1"]);
        let page = get_commits(dir.path(), "HEAD", 1, 10).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].summary, "first");
    }

    #[test]
    fn first_parent_skips_merged_commits() {
        let (dir, _) = repo();
        let run = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(dir.path())
                    .status()
                    .unwrap()
                    .success()
            )
        };
        run(&["checkout", "-q", "-b", "topic"]);
        std::fs::write(dir.path().join("c.txt"), "c\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "topic work"]);
        run(&["checkout", "-q", "main"]);
        run(&["merge", "-q", "--no-ff", "-m", "merge topic", "topic"]);
        let all = get_commits(dir.path(), "HEAD", 0, 10).unwrap();
        assert!(all.iter().any(|c| c.summary == "topic work"));
        let first = get_commits_with(dir.path(), "HEAD", 0, 10, true).unwrap();
        let summaries: Vec<_> = first.iter().map(|c| c.summary.as_str()).collect();
        assert_eq!(summaries, ["merge topic", "second", "first"]);
    }

    #[test]
    fn all_branches_walk_from_every_tip() {
        let (dir, _) = repo();
        let run = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(dir.path())
                    .status()
                    .unwrap()
                    .success()
            )
        };
        run(&["checkout", "-q", "-b", "topic"]);
        std::fs::write(dir.path().join("c.txt"), "c\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "topic work"]);
        run(&["checkout", "-q", "main"]);
        run(&["update-ref", "refs/remotes/origin/main", "HEAD~1"]);
        run(&[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ]);
        let head = get_commits(dir.path(), "HEAD", 0, 1).unwrap().remove(0);
        let tips = all_branch_tips(dir.path()).unwrap();
        // HEAD first; main repeats it and origin/HEAD repeats origin/main
        assert_eq!(tips.len(), 3);
        assert_eq!(tips[0], head.sha);
        let all = get_commits_from(dir.path(), &tips, 0, 10, false).unwrap();
        let summaries: Vec<_> = all.iter().map(|c| c.summary.as_str()).collect();
        assert!(summaries.contains(&"topic work"));
        assert_eq!(summaries.len(), 3);
        let page = get_commits_from(dir.path(), &tips, 1, 1, false).unwrap();
        assert_eq!(page[0].sha, all[1].sha);
    }

    #[test]
    fn changed_files_and_diff() {
        let (dir, git) = repo();
        let head = get_commits(dir.path(), "HEAD", 0, 1).unwrap().remove(0);
        let data = get_changed_files(git.clone(), dir.path(), &head.sha, false).unwrap();
        let paths: Vec<_> = data.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["a.txt", "b.txt"]);
        assert_eq!(data.files[1].status.kind, FileStatusKind::New);
        assert_eq!((data.lines_added, data.lines_deleted), (3, 1));
        let diff = commit_file_diff(git, dir.path(), &data.files[0], false).unwrap();
        let Diff::Text { hunks, .. } = diff else {
            panic!("text diff expected")
        };
        assert_eq!(hunks.len(), 1);
    }

    #[test]
    fn subject_and_body_match_git_format() {
        let split = |m: &str| subject_and_body(m.as_bytes());
        assert_eq!(split("one\n"), ("one".into(), "".into()));
        assert_eq!(
            split("\n\none  \ntwo\n\n\nbody\n\nmore\n"),
            ("one two".into(), "body\n\nmore\n".into())
        );
        assert_eq!(split("one\r\n\r\nbody"), ("one".into(), "body".into()));
        assert_eq!(split(""), ("".into(), "".into()));
    }

    /// The in-process walk against GHD's `getCommits`, which reads `git log
    /// --date=raw --format=…%s…%b…%(trailers:unfold,only)…%D`: summary,
    /// body, trailers (split as `parseRawUnfoldedTrailers` does), tags
    /// (the `tag: ` entries of `%D`, in its order), parents and identities.
    #[test]
    fn walk_matches_ghd_log_format() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        let git = |args: &[&str], n: u32| {
            let date = format!("{} +0130", 1_700_000_000 + n * 60);
            let out = Command::new("git")
                .args(args)
                .current_dir(path)
                .env("GIT_AUTHOR_NAME", "Ada Lovelace")
                .env("GIT_AUTHOR_EMAIL", "ada@example.com")
                .env("GIT_COMMITTER_NAME", "Grace")
                .env("GIT_COMMITTER_EMAIL", "grace@example.com")
                .env("GIT_AUTHOR_DATE", &date)
                .env("GIT_COMMITTER_DATE", &date)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
            out.stdout
        };
        git(&["init", "-q", "-b", "main"], 0);
        git(&["config", "commit.gpgsign", "false"], 0);
        git(&["config", "tag.gpgsign", "false"], 0);
        let messages = [
            "one line",
            "subject\n\nbody\n",
            "  \n\nsubject over\ntwo lines  \n\n\n\nbody one\n\nbody two\n\n",
            "subject\r\n\r\nbody with crlf\r\n",
            "subject\n\nSigned-off-by: A <a@example.com>\nCo-authored-by: B <b@example.com>\n",
            "subject\n\nbody\n\nCo-Authored-By: X <x@example.com>\nLink: https://example.com/a:b\n",
            "subject\n\nNot a trailer: this\nis prose\n",
            "subject\n\nbody\n\nAcked-by : Spaced\nFolded-by: first\n  second\nEmpty:\n",
            "subject\n\n(cherry picked from commit abc)\nNote: x\n",
            "subject\n\nbody\n\nKey: value\nnot a trailer line\nmore prose\nand more\nSigned-off-by: Z <z@example.com>\n",
            "subject\n\nbody\n\nKey: value\nnot a trailer line\nmore prose\nand more\nlast\n",
            "subject\n\n# not a comment here\n\nToken: v\n",
            "",
        ];
        for (n, message) in messages.iter().enumerate() {
            let file = path.join(format!("m{n}"));
            std::fs::write(&file, message).unwrap();
            git(
                &[
                    "commit",
                    "-q",
                    "--allow-empty",
                    "--allow-empty-message",
                    "--cleanup=verbatim",
                    "-F",
                    file.to_str().unwrap(),
                ],
                n as u32 + 1,
            );
            std::fs::remove_file(&file).unwrap();
            match n {
                1 => {
                    git(&["tag", "light"], 0);
                    git(&["tag", "-a", "-m", "annotated", "zeta"], 0);
                    git(&["tag", "-a", "-m", "nested", "nested", "zeta"], 0);
                    git(&["tag", "with,comma"], 0);
                    git(&["branch", "other"], 0);
                }
                4 => {
                    git(&["tag", "Upper"], 0);
                }
                _ => {}
            }
        }
        let format = [
            "%H",
            "%s",
            "%b",
            "%an <%ae> %ad",
            "%cn <%ce> %cd",
            "%P",
            "%(trailers:unfold,only)",
            "%D",
        ]
        .join("%x00");
        let out = git(
            &[
                "log",
                "HEAD",
                "--date=raw",
                &format!("--format=format:{format}%x01"),
                "--no-show-signature",
                "--no-color",
                "--",
            ],
            0,
        );
        let identity = |field: &str| {
            let (name, rest) = field.split_once(" <").unwrap();
            let (email, date) = rest.split_once("> ").unwrap();
            let (seconds, offset) = date.split_once(' ').unwrap();
            let sign = if offset.starts_with('-') { -1 } else { 1 };
            let hours: i32 = offset[1..3].parse().unwrap();
            let minutes: i32 = offset[3..5].parse().unwrap();
            (
                name.to_string(),
                email.to_string(),
                seconds.parse::<i64>().unwrap(),
                sign * (hours * 3600 + minutes * 60),
            )
        };
        let expected: Vec<String> = String::from_utf8(out)
            .unwrap()
            .split('\x01')
            .map(|record| record.trim_start_matches('\n'))
            .filter(|record| !record.is_empty())
            .map(|record| {
                let f: Vec<&str> = record.split('\0').collect();
                let trailers: Vec<(String, String)> = f[6]
                    .split('\n')
                    .filter_map(|line| {
                        let ix = line.find(':').filter(|&ix| ix > 0)?;
                        Some((
                            line[..ix].trim().to_string(),
                            line[ix + 1..].trim().to_string(),
                        ))
                    })
                    .collect();
                let tags: Vec<&str> = f[7]
                    .split(", ")
                    .filter_map(|r| r.strip_prefix("tag: "))
                    .collect();
                let parents: Vec<&str> = f[5].split(' ').filter(|p| !p.is_empty()).collect();
                format!(
                    "{} {:?} {:?} {:?} {:?} {parents:?} {trailers:?} {tags:?}",
                    f[0],
                    f[1],
                    f[2],
                    identity(f[3]),
                    identity(f[4])
                )
            })
            .collect();
        let actual: Vec<String> = get_commits(path, "HEAD", 0, 100)
            .unwrap()
            .iter()
            .map(|c| {
                let id =
                    |i: &CommitIdentity| (i.name.clone(), i.email.clone(), i.seconds, i.offset);
                format!(
                    "{} {:?} {:?} {:?} {:?} {:?} {:?} {:?}",
                    c.sha,
                    c.summary,
                    c.body,
                    id(&c.author),
                    id(&c.committer),
                    c.parents,
                    c.trailers,
                    c.tags
                )
            })
            .collect();
        assert_eq!(actual.len(), messages.len());
        for (actual, expected) in actual.iter().zip(&expected) {
            assert_eq!(actual, expected);
        }
        assert_eq!(actual, expected);
    }

    #[test]
    fn parses_rename_raw_numstat() {
        let out = b":100644 100644 abc def R100\0old.txt\0new.txt\0\n0\t0\t\0old.txt\0new.txt\0";
        let data = parse_raw_log_with_numstat(out, "sha");
        assert_eq!(data.files.len(), 1);
        assert_eq!(data.files[0].path, "new.txt");
        assert_eq!(data.files[0].old_path.as_deref(), Some("old.txt"));
        assert_eq!(data.files[0].status.kind, FileStatusKind::Renamed);
        assert_eq!(data.files[0].status.score, Some(100));
    }

    #[test]
    fn a_raw_entry_without_a_status_does_not_panic() {
        let data = parse_raw_log_with_numstat(b":100644 100644 abc def\0a.txt\0", "sha");
        assert_eq!(data.files.len(), 1);
        assert_eq!(data.files[0].path, "a.txt");
        assert_eq!(data.files[0].status.kind, FileStatusKind::Modified);
        assert_eq!(data.files[0].status.score, None);
        let data = parse_raw_log_with_numstat(b":100644 100644 abc def \0a.txt\0", "sha");
        assert_eq!(data.files.len(), 1);
    }
}
