//! Port of GitHub Desktop's `app/test/unit/git-store-test.ts`.
//!
//! Corvene has no `GitStore` (`lib/stores/git-store.ts`): its parts are the
//! `corvene_git` calls the `Dispatcher` makes and the `RepositoryState`
//! fields it fills. [`GitStore`] here routes each method the cases use to
//! the call Corvene makes for it:
//!
//! - `loadCommitBatch(commitish, skip)`: `Dispatcher::load_commits` reads
//!   `corvene_git::get_commits_with(path, "HEAD", skip,
//!   COMMIT_BATCH_SIZE, first_parent)`; GitHub Desktop has no first-parent
//!   history, so `first_parent` is false (the `807-history-first-parent`
//!   flag's GitHub Desktop value). The batch's shas are returned.
//! - `loadStatus()` / `tip`: `Dispatcher::refresh_repository` takes the tip
//!   from `corvene_git::open_repository(path)`.
//! - `discardChanges(files)`: `Dispatcher::discard_changes` calls
//!   `corvene_git::discard_changes(git, path, files, move_to_trash,
//!   clean_submodules, keep_untrashable)`; `clean_submodules` is the
//!   `720-discard-submodule-changes` flag (off for GitHub Desktop), and
//!   `keep_untrashable` only matters with the Trash (false here).
//!   GitHub Desktop moves every discarded file that is not deleted to the
//!   Trash through `shell.moveItemToTrash`, which the test shell
//!   (`helpers/test-app-shell.ts`) replaces with `unlink`; Corvene's
//!   `move_to_trash: false` deletes instead of trashing, which is what the
//!   test shell does (and keeps the user's Trash out of the tests).
//! - `undoCommit(commit)` / `commitMessage`: `Dispatcher::undo_commit`
//!   calls `corvene_core::git_store::undo_commit(git, path, commit)`, which
//!   undoes `HEAD` (every case undoes the `HEAD` commit) and returns the
//!   message the commit form gets back (`RepositoryState::commit_message`);
//!   [`GitStore::commit_message`] is that message as (summary,
//!   description).
//! - `loadLocalCommits(branch)` / `localCommitSHAs`: the refresh calls
//!   `corvene_git::most_recent_local_commit(path, branch, upstream)` for the
//!   current branch (none when there is no current branch); Corvene keeps
//!   only that first local commit (the undo bar's), so `localCommitSHAs`
//!   has at most one sha.
//! - `loadBranches()` / `allBranches`: the refresh reads every branch with
//!   `open_repository`; the merge of local and remote branches GitHub
//!   Desktop does in `mergeRemoteAndLocalBranches` is in the branch list,
//!   `corvene_ui::branch_list::group_branches`, so `allBranches` is its
//!   groups' branches with no default branch, no recent branches and no
//!   filter (everything lands in Other Branches). A branch's `upstream` is
//!   `Branch::upstream_short` (`origin/master`), its `type` is `kind`.
//! - `getRemotes(repository)` is `corvene_git::get_remotes`, `getCommit(repository,
//!   ref)` the first of `corvene_git::get_commits(path, ref, 0, 1)`.
//!
//! The status the cases read is `get_status_or_throw`, GitHub Desktop's
//! `getStatusOrThrow`.

use std::path::PathBuf;

use corvene_core::{Branch, BranchKind, Commit, FileStatusKind, Tip, WorkingDirectoryFileChange};
use corvene_test_support::{
    TestRepo, Tree, TreeEntry, clone_local_repository, exec, get_commit, get_status_or_throw, git,
    make_commit, setup_empty_repository, setup_fixture_repository, switch_to, write_file,
};
use corvene_ui::branch_list::group_branches;

/// GitHub Desktop's `GitStore` for one repository, as Corvene does each
/// part (see the module docs).
struct GitStore {
    path: PathBuf,
    commit_message: Option<(String, String)>,
    tip: Tip,
    all_branches: Vec<Branch>,
    local_commit_shas: Vec<String>,
}

impl GitStore {
    /// `new GitStore(repository, shell, new TestStatsStore())`.
    fn new(repository: &TestRepo) -> Self {
        Self {
            path: repository.path().to_path_buf(),
            commit_message: None,
            tip: Tip::Unknown,
            all_branches: Vec::new(),
            local_commit_shas: Vec::new(),
        }
    }

    /// `loadCommitBatch(commitish, skip)`: the shas of the batch, `None`
    /// when reading the history fails.
    fn load_commit_batch(&self, commitish: &str, skip: usize) -> Option<Vec<String>> {
        corvene_git::get_commits_with(
            &self.path,
            commitish,
            skip,
            corvene_git::COMMIT_BATCH_SIZE,
            false,
        )
        .ok()
        .map(|commits| commits.into_iter().map(|c| c.sha).collect())
    }

    /// `loadStatus()`: refresh the tip.
    fn load_status(&mut self) {
        self.tip = corvene_git::open_repository(&self.path)
            .unwrap_or_else(|err| panic!("open {}: {err}", self.path.display()))
            .tip;
    }

    /// `discardChanges(files)`. GitHub Desktop runs the reset and checkout
    /// in `performFailableOperation`, which reports an error and carries on
    /// (as `Dispatcher::discard_changes` shows it and refreshes), so a
    /// failure is printed and the case's status checks decide.
    fn discard_changes(&self, files: &[WorkingDirectoryFileChange]) {
        if let Err(err) =
            corvene_git::discard_changes(git(), &self.path, files, false, false, false)
        {
            eprintln!("discard changes failed (performFailableOperation): {err}");
        }
    }

    /// `undoCommit(commit)`: like `discardChanges`, an error is reported
    /// and swallowed (`performFailableOperation`, `Dispatcher::undo_commit`).
    fn undo_commit(&mut self, commit: &Commit) {
        match corvene_core::git_store::undo_commit(git(), &self.path, commit, false) {
            Ok(message) => {
                self.commit_message =
                    Some((message.summary, message.description.unwrap_or_default()));
            }
            Err(err) => eprintln!("undo commit failed (performFailableOperation): {err}"),
        }
    }

    /// `commitMessage`: the (summary, description) the commit form got back
    /// from the last undone commit.
    fn commit_message(&self) -> Option<(String, String)> {
        self.commit_message.clone()
    }

    /// `loadLocalCommits(branch)`.
    fn load_local_commits(&mut self, branch: Option<&Branch>) {
        self.local_commit_shas = branch
            .and_then(|branch| {
                corvene_git::most_recent_local_commit(
                    &self.path,
                    &branch.name,
                    branch.upstream.as_deref(),
                )
                .unwrap_or_else(|err| panic!("local commits of {}: {err}", branch.name))
            })
            .into_iter()
            .map(|c| c.sha)
            .collect();
    }

    /// `loadBranches()`.
    fn load_branches(&mut self) {
        let info = corvene_git::open_repository(&self.path)
            .unwrap_or_else(|err| panic!("open {}: {err}", self.path.display()));
        self.all_branches = group_branches(&info.branches, None, &[], "", false)
            .into_iter()
            .flat_map(|group| group.branches)
            .collect();
    }
}

// GHD: unit/git-store-test.ts › GitStore › loadCommitBatch › includes HEAD when loading commits
#[test]
fn includes_head_when_loading_commits() {
    let repo = setup_fixture_repository("repository-with-105-commits");
    let git_store = GitStore::new(&repo);

    let commits = git_store.load_commit_batch("HEAD", 0);

    let commits = commits.expect("commits !== null");
    assert_eq!(commits.len(), 100);
    assert_eq!(commits[0], "708a46eac512c7b2486da2247f116d11a100b611");
}

// GHD: unit/git-store-test.ts › GitStore › can discard changes from a repository
#[test]
fn can_discard_changes_from_a_repository() {
    let repo = setup_empty_repository();
    let mut git_store = GitStore::new(&repo);

    let readme_file = "README.md";
    let readme_file_path = repo.join(readme_file);

    write_file(&readme_file_path, "SOME WORDS GO HERE\n");

    let license_file = "LICENSE.md";
    let license_file_path = repo.join(license_file);

    write_file(&license_file_path, "SOME WORDS GO HERE\n");

    // commit the readme file but leave the license
    exec(["add", readme_file], repo.path());
    exec(["commit", "-m", "added readme file"], repo.path());

    write_file(&readme_file_path, "WRITING SOME NEW WORDS\n");
    // setup requires knowing about the current tip
    git_store.load_status();

    let status = get_status_or_throw(&repo);
    let files = status.files;

    assert_eq!(files.len(), 2);
    assert_eq!(files[0].path, "README.md");
    assert_eq!(files[0].status.kind, FileStatusKind::Modified);

    // discard the LICENSE.md file
    git_store.discard_changes(&[files[1].clone()]);

    let status = get_status_or_throw(&repo);
    let files = status.files;

    assert_eq!(files.len(), 1);
}

// GHD: unit/git-store-test.ts › GitStore › can discard a renamed file
#[test]
fn can_discard_a_renamed_file() {
    let repo = setup_empty_repository();
    let git_store = GitStore::new(&repo);

    let file = "README.md";
    let renamed_file = "NEW-README.md";
    let file_path = repo.join(file);

    write_file(&file_path, "SOME WORDS GO HERE\n");

    // commit the file, and then rename it
    exec(["add", file], repo.path());
    exec(["commit", "-m", "added file"], repo.path());
    exec(["mv", file, renamed_file], repo.path());

    let status_before_discard = get_status_or_throw(&repo);
    let files_to_discard = status_before_discard.files;

    // discard the renamed file
    git_store.discard_changes(&files_to_discard);

    let status = get_status_or_throw(&repo);
    let files = status.files;

    assert_eq!(files.len(), 0, "{files:?}");
}

/// The `undo first commit` describe's `commitMessage`.
const COMMIT_MESSAGE: &str = "added file";

/// The `undo first commit` describe's `setupRepo(t)`: `README.md`
/// committed as the only commit on `master`.
fn setup_repo() -> (TestRepo, Commit) {
    let repository = setup_empty_repository();

    let file = "README.md";
    let file_path = repository.join(file);

    write_file(&file_path, "SOME WORDS GO HERE\n");

    exec(["add", file], repository.path());
    exec(["commit", "-m", COMMIT_MESSAGE], repository.path());

    let first_commit = get_commit(&repository, "master");
    let first_commit = first_commit.expect("firstCommit !== null");
    assert_eq!(first_commit.parents.len(), 0);

    (repository, first_commit)
}

// GHD: unit/git-store-test.ts › GitStore › undo first commit › reports the repository is unborn
#[test]
fn reports_the_repository_is_unborn() {
    let (repository, first_commit) = setup_repo();
    let mut git_store = GitStore::new(&repository);

    git_store.load_status();
    assert!(
        matches!(git_store.tip, Tip::Valid { .. }),
        "{:?}",
        git_store.tip
    );

    git_store.undo_commit(&first_commit);

    let after = get_status_or_throw(&repository);
    assert!(after.current_tip.is_none(), "{:?}", after.current_tip);
}

// GHD: unit/git-store-test.ts › GitStore › undo first commit › pre-fills the commit message
#[test]
fn pre_fills_the_commit_message() {
    let (repository, first_commit) = setup_repo();

    let mut git_store = GitStore::new(&repository);

    git_store.undo_commit(&first_commit);

    let new_commit_message = git_store.commit_message();
    let new_commit_message = new_commit_message.expect("newCommitMessage !== null");
    assert_eq!(new_commit_message.0, COMMIT_MESSAGE);
}

// GHD: unit/git-store-test.ts › GitStore › undo first commit › clears the undo commit dialog
#[test]
fn clears_the_undo_commit_dialog() {
    let (repository, first_commit) = setup_repo();

    let mut git_store = GitStore::new(&repository);

    git_store.load_status();

    let Tip::Valid { branch } = git_store.tip.clone() else {
        panic!("tip as IValidBranch: {:?}", git_store.tip);
    };
    git_store.load_local_commits(Some(&branch));

    assert_eq!(git_store.local_commit_shas.len(), 1);

    git_store.undo_commit(&first_commit);

    git_store.load_status();
    assert!(
        matches!(git_store.tip, Tip::Unborn { .. }),
        "{:?}",
        git_store.tip
    );

    git_store.load_local_commits(None);

    assert_eq!(git_store.local_commit_shas.len(), 0);
}

// GHD: unit/git-store-test.ts › GitStore › undo first commit › has no staged files
#[test]
fn has_no_staged_files() {
    let (repository, first_commit) = setup_repo();

    let mut git_store = GitStore::new(&repository);

    git_store.load_status();

    let Tip::Valid { branch } = git_store.tip.clone() else {
        panic!("tip as IValidBranch: {:?}", git_store.tip);
    };
    git_store.load_local_commits(Some(&branch));

    assert_eq!(git_store.local_commit_shas.len(), 1);

    git_store.undo_commit(&first_commit);

    // compare the index state to some other tree-ish
    // 4b825dc642cb6eb9a060e54bf8d69288fbee4904 is the magic empty tree
    // if nothing is staged, this should return no entries
    let result = exec(
        [
            "diff-index",
            "--name-status",
            "-z",
            "4b825dc642cb6eb9a060e54bf8d69288fbee4904",
        ],
        repository.path(),
    );
    assert_eq!(result.stdout.len(), 0, "{:?}", result.stdout);
}

// GHD: unit/git-store-test.ts › GitStore › repository with HEAD file › can discard modified change cleanly
#[test]
fn can_discard_modified_change_cleanly() {
    let repo = setup_fixture_repository("repository-with-HEAD-file");
    let git_store = GitStore::new(&repo);

    let file = "README.md";
    let file_path = repo.join(file);

    write_file(&file_path, "SOME WORDS GO HERE\n");

    let status = get_status_or_throw(&repo);
    let files = status.files;
    assert_eq!(files.len(), 1);

    git_store.discard_changes(&[files[0].clone()]);

    let status = get_status_or_throw(&repo);
    let files = status.files;
    assert_eq!(files.len(), 0, "{files:?}");
}

/// The `loadBranches` describe's `setupRepositories(t)`: an upstream with
/// two commits on `master` and two more on `some-other-branch`, cloned
/// while on `master`.
fn setup_repositories() -> (TestRepo, TestRepo) {
    let upstream = setup_empty_repository();
    make_commit(
        &upstream,
        &Tree::with_message(
            "first commit",
            [TreeEntry::new("README.md", "some words go here")],
        ),
    );
    make_commit(
        &upstream,
        &Tree::with_message(
            "second commit",
            [TreeEntry::new(
                "README.md",
                "some words go here\nand some more words",
            )],
        ),
    );
    switch_to(&upstream, "some-other-branch");
    make_commit(
        &upstream,
        &Tree::with_message(
            "branch commit",
            [TreeEntry::new("README.md", "changing some words")],
        ),
    );
    make_commit(
        &upstream,
        &Tree::with_message(
            "second branch commit",
            [TreeEntry::new(
                "README.md",
                "and even more changing of words",
            )],
        ),
    );

    // move this repository back to `master` before cloning
    switch_to(&upstream, "master");

    let repository = clone_local_repository(&upstream);

    (upstream, repository)
}

// GHD: unit/git-store-test.ts › GitStore › loadBranches › has a remote defined
#[test]
fn has_a_remote_defined() {
    let (_upstream, repository) = setup_repositories();
    let remotes = corvene_git::get_remotes(git(), repository.path())
        .unwrap_or_else(|err| panic!("get remotes: {err}"));
    assert_eq!(remotes.len(), 1);
}

// GHD: unit/git-store-test.ts › GitStore › loadBranches › will merge a local and remote branch when tracking branch set
#[test]
fn will_merge_a_local_and_remote_branch_when_tracking_branch_set() {
    let (_upstream, repository) = setup_repositories();
    let mut git_store = GitStore::new(&repository);
    git_store.load_branches();

    assert_eq!(
        git_store.all_branches.len(),
        2,
        "{:?}",
        git_store.all_branches
    );

    let default_branch = git_store.all_branches.iter().find(|b| b.name == "master");
    let default_branch = default_branch.expect("defaultBranch !== undefined");
    assert_eq!(default_branch.upstream_short(), Some("origin/master"));

    let remote_branch = git_store
        .all_branches
        .iter()
        .find(|b| b.name == "origin/some-other-branch");
    let remote_branch = remote_branch.expect("remoteBranch !== undefined");
    assert_eq!(remote_branch.kind, BranchKind::Remote);
}

// GHD: unit/git-store-test.ts › GitStore › loadBranches › the tracking branch is not cleared when the remote branch is removed
#[test]
fn the_tracking_branch_is_not_cleared_when_the_remote_branch_is_removed() {
    let (upstream, repository) = setup_repositories();
    // checkout the other branch after cloning
    exec(["checkout", "some-other-branch"], repository.path());

    let mut git_store = GitStore::new(&repository);
    git_store.load_branches();

    let current_branch_before = git_store
        .all_branches
        .iter()
        .find(|b| b.name == "some-other-branch");
    let current_branch_before = current_branch_before.expect("currentBranchBefore !== undefined");
    assert_eq!(
        current_branch_before.upstream_short(),
        Some("origin/some-other-branch")
    );

    // delete the ref in the upstream branch
    exec(["branch", "-D", "some-other-branch"], upstream.path());

    // update the local repository state to remove the remote ref
    exec(["fetch", "--prune", "--all"], repository.path());
    git_store.load_branches();

    let current_branch_after = git_store
        .all_branches
        .iter()
        .find(|b| b.name == "some-other-branch");

    // ensure the tracking information is unchanged
    let current_branch_after = current_branch_after.expect("currentBranchAfter !== undefined");
    assert_eq!(
        current_branch_after.upstream_short(),
        Some("origin/some-other-branch")
    );
}
