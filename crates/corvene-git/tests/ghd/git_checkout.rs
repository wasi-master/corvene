//! Port of GitHub Desktop's `app/test/unit/git/checkout-test.ts`.
//!
//! Corvene equivalents:
//!
//! - `checkoutBranch(repository, branch, currentRemote, progressCallback,
//!   allowFileProtocol)` (`lib/git/checkout.ts`) is
//!   `corvene_git::checkout_branch(git, path, branch)` with GitHub Desktop's
//!   defaults, or `corvene_git::checkout_branch_with(git, path, branch,
//!   &CheckoutOptions { allow_file_protocol, .. })`; neither takes a remote,
//!   and the progress callback is a scope around the call
//!   (`corvene_git::op_progress::with_progress`). Both update the
//!   submodules afterwards, as GitHub Desktop 3.6.6 does. An `Err` is a rejection; its message is
//!   [`git_error_message`].
//! - `getBranches(repository, ...prefixes)` is [`get_branches`]
//!   (`corvene_git::open_repository(..).branches`).
//! - `createBranch(repository, name, null)` is
//!   `corvene_git::create_branch(git, path, name, None, false)`.
//! - `new GitStore(repository, …).loadStatus()` then `store.tip`: the tip
//!   Corvene's repository refresh reads, `corvene_git::open_repository(..)
//!   .tip` ([`load_tip`]). `TipState.Valid` is `Tip::Valid`,
//!   `BranchType.Local` `BranchKind::Local`, `branch.upstreamRemoteName`
//!   `Branch::upstream_remote_name()`.
//! - GitHub Desktop's `Branch` object literal is a `corvene_models::Branch`:
//!   `ref: ''` is an empty `full_name`, `tip: { sha: '' }` is no tip.

use corvene_git::{CheckoutOptions, GitError};
use corvene_models::{Branch, BranchKind, Tip};
use corvene_test_support::{
    TestRepo, exec, get_branches, get_status_or_throw, git, git_error_message, load_tip,
    setup_empty_repository, setup_fixture_repository,
    setup_repository_with_uninitialized_submodule,
};

use crate::index_support::regex_test;

/// `checkoutBranch(repository, branch, null)`.
fn checkout_branch(repository: &TestRepo, branch: &Branch) -> Result<(), GitError> {
    corvene_git::checkout_branch(git(), repository.path(), branch)
}

// GHD: unit/git/checkout-test.ts › git/checkout › throws when invalid characters are used for branch name
#[test]
fn throws_when_invalid_characters_are_used_for_branch_name() {
    let repository = setup_empty_repository();

    let branch = Branch {
        name: "..".to_string(),
        kind: BranchKind::Local,
        full_name: String::new(),
        tip: None,
        upstream: None,
        tip_time: None,
        tip_author: None,
        remote_name: None,
    };

    let err = checkout_branch(&repository, &branch).expect_err("checkoutBranch rejects");
    let message = git_error_message(&err);
    assert!(
        regex_test("fatal: invalid reference: ..\n", &message),
        "{message:?}"
    );
}

// GHD: unit/git/checkout-test.ts › git/checkout › can checkout a valid branch name in an existing repository
#[test]
fn can_checkout_a_valid_branch_name_in_an_existing_repository() {
    let repository = setup_fixture_repository("repo-with-many-refs");

    let branches = get_branches(
        repository.path(),
        &["refs/heads/commit-with-long-description"],
    );

    if branches.is_empty() {
        panic!("Could not find branch: commit-with-long-description");
    }

    checkout_branch(&repository, &branches[0]).expect("checkoutBranch");

    let tip = load_tip(&repository);

    let Tip::Valid { branch } = tip else {
        panic!("expected TipState.Valid, got {tip:?}");
    };

    assert_eq!(branch.name, "commit-with-long-description");
}

// GHD: unit/git/checkout-test.ts › git/checkout › can checkout a branch when it exists on multiple remotes
#[test]
fn can_checkout_a_branch_when_it_exists_on_multiple_remotes() {
    let repository = setup_fixture_repository("checkout-test-cases");

    let expected_branch = "first";
    let first_remote = "first-remote";
    let second_remote = "second-remote";

    let branches = get_branches(repository.path(), &[]);
    let first_branch = format!("{first_remote}/{expected_branch}");
    let Some(first_remote_branch) = branches.iter().find(|b| b.name == first_branch) else {
        panic!("Could not find branch: '{first_branch}'");
    };

    let second_branch = format!("{second_remote}/{expected_branch}");
    if !branches.iter().any(|b| b.name == second_branch) {
        panic!("Could not find branch: '{second_branch}'");
    }

    checkout_branch(&repository, first_remote_branch).expect("checkoutBranch");

    let tip = load_tip(&repository);

    let Tip::Valid { branch } = tip else {
        panic!("expected TipState.Valid, got {tip:?}");
    };

    assert_eq!(branch.name, expected_branch);
    assert_eq!(branch.kind, BranchKind::Local);
    assert_eq!(branch.upstream_remote_name(), Some("first-remote"));
}

// GHD: unit/git/checkout-test.ts › git/checkout › will fail when an existing branch matches the remote branch
#[test]
fn will_fail_when_an_existing_branch_matches_the_remote_branch() {
    let repository = setup_fixture_repository("checkout-test-cases");

    let expected_branch = "first";
    let first_remote = "first-remote";

    let branches = get_branches(repository.path(), &[]);
    let first_branch = format!("{first_remote}/{expected_branch}");
    let Some(remote_branch) = branches.iter().find(|b| b.name == first_branch) else {
        panic!("Could not find branch: '{first_branch}'");
    };

    corvene_git::create_branch(git(), repository.path(), expected_branch, None, false)
        .expect("createBranch");

    let err = checkout_branch(&repository, remote_branch).expect_err("checkoutBranch rejects");
    let message = git_error_message(&err);
    assert!(
        regex_test("A branch with that name already exists.", &message),
        "{message:?}"
    );
}

// GHD: unit/git/checkout-test.ts › git/checkout › with submodules › updates a changed submodule reference
#[test]
fn updates_a_changed_submodule_reference() {
    let repository = setup_fixture_repository("test-submodule-checkouts");
    let path = repository.path();

    // put the repository into a known good state
    exec(["checkout", "master", "-f", "--recurse-submodules"], path);

    let branches = get_branches(path, &[]);
    let Some(dev_branch) = branches.iter().find(|b| b.name == "dev") else {
        panic!("Could not find branch: 'dev'");
    };

    checkout_branch(&repository, dev_branch).expect("checkoutBranch");

    let status = get_status_or_throw(&repository);
    assert_eq!(status.files.len(), 0, "{:?}", status.files);
}

// GHD: unit/git/checkout-test.ts › git/checkout › with submodules › initializes an uninitialized submodule when checking out a branch
#[test]
fn initializes_an_uninitialized_submodule_when_checking_out_a_branch() {
    let repository = setup_repository_with_uninitialized_submodule();

    let branches = get_branches(repository.path(), &[]);
    let Some(branch_with_submodule) = branches.iter().find(|b| b.name != "master") else {
        panic!("Could not find branch other than 'master'");
    };

    // `checkoutBranch(repository, branch, null, undefined, true)`: the
    // submodule's source is a local path
    corvene_git::checkout_branch_with(
        git(),
        repository.path(),
        branch_with_submodule,
        &CheckoutOptions {
            allow_file_protocol: true,
            ..Default::default()
        },
    )
    .expect("checkoutBranch");

    // Verify we're on the correct branch
    let status_output = exec(["status"], repository.path());
    assert!(
        status_output
            .stdout
            .contains(&format!("On branch {}", branch_with_submodule.name)),
        "{}",
        status_output.stdout
    );

    // Verify the submodule is initialized and has the correct commits
    let submodule_path = repository.join("test-submodule");
    let submodule_git_path = submodule_path.join(".git");

    // Check that submodule .git exists (either as file or directory)
    let submodule_git_exists = submodule_git_path.exists();
    assert!(
        submodule_git_exists,
        "Submodule .git should exist after checkout"
    );

    // Verify submodule has two commits
    let submodule_log = exec(["log", "--oneline"], &submodule_path);
    assert_eq!(submodule_log.stdout.trim().split('\n').count(), 2);
}
