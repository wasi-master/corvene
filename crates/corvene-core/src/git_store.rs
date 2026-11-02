//! Parts of GHD's `GitStore` (`app/src/lib/stores/git-store.ts`) that need
//! no gpui; the `Dispatcher` runs them on the background executor.

use std::path::Path;
use std::sync::Arc;

use corvene_git::{GitBinary, GitError};
use corvene_models::Commit;

use crate::state::CommitMessage;

/// GHD `GitStore.undoCommit(commit)` for the `HEAD` commit: undo it
/// (`corvene_git::undo_last_commit`: `git reset --mixed` to its parent, or
/// GHD's `undoFirstCommit` for a root commit) and return the message the
/// commit form gets back (GHD's `commitMessage`): the commit's summary and
/// body.
///
/// Corvene leaves out GHD's `restoreCoAuthorsFromCommit`, which for a
/// repository on GitHub moves the message's `Co-authored-by` trailers to
/// the co-author field instead (`.docs/TODO.md`).
pub fn undo_commit(
    git: Arc<GitBinary>,
    workdir: &Path,
    commit: &Commit,
    github: bool,
) -> Result<CommitMessage, GitError> {
    corvene_git::undo_last_commit(git, workdir)?;
    // GHD `restoreCoAuthorsFromCommit`: in a GitHub repository the trailers
    // become co-author tokens ([`restored_co_authors`])
    let body = if github && !commit.co_authors().is_empty() {
        commit.body_no_co_authors().trim_end().to_string()
    } else {
        commit.body.clone()
    };
    Ok(CommitMessage::new(commit.summary.clone(), Some(body)))
}

/// GHD `loadCommitAndCoAuthors`: `commit`'s co-authors as the form's tokens.
pub fn restored_co_authors(commit: &Commit) -> Vec<corvene_models::Author> {
    commit
        .co_authors()
        .into_iter()
        .map(|author| corvene_models::Author::Known {
            name: author.name,
            email: author.email,
            username: None,
        })
        .collect()
}

/// The name of GHD's `findDefaultRemote`
/// (`lib/stores/helpers/find-default-remote.ts`,
/// `corvene_git::find_default_remote`): `origin`, else the first remote.
pub fn default_remote_name(info: &corvene_models::RepositoryInfo) -> Option<&str> {
    corvene_git::find_default_remote(&info.remotes).map(|r| r.name.as_str())
}

/// GHD `GitStore.loadBranches`' `findDefaultBranch(repository, branches,
/// defaultRemoteName)`: the default branch's name, from the default
/// remote's `HEAD` or `init.defaultBranch`. `Dispatcher::refresh_repository`
/// runs the same steps with its git processes in parallel.
pub fn load_default_branch(
    git: Arc<GitBinary>,
    info: &corvene_models::RepositoryInfo,
) -> Option<String> {
    let remote = default_remote_name(info);
    let head = remote.and_then(|remote| {
        corvene_git::remote_head(git.clone(), &info.workdir, remote)
            .ok()
            .flatten()
    });
    let configured = corvene_git::configured_default_branch(git);
    corvene_git::find_default_branch(&info.branches, remote, head.as_deref(), &configured)
        .map(|b| b.name.clone())
}
