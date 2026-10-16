//! Port of GitHub Desktop's `app/test/unit/repository-matching-test.ts`.
//!
//! - `matchGitHubRepository(accounts, remote)` (`lib/repository-matching.ts`,
//!   used when a repository is added or refreshed) is
//!   `corvene_models::github_from_remote(url, hosts)`, whose second
//!   argument names the web hosts of the signed-in accounts (github.com only
//!   when it is listed, as in GitHub Desktop);
//!   [`match_github_repository`] passes the accounts' hosts
//!   (`corvene_models::github_hosts(accounts, false)`, each `Account::host`).
//!   GitHub Desktop's result also carries the matching account, which no
//!   case reads.
//! - `urlMatchesRemote(url, remote)` is
//!   `corvene_models::url_matches_remote(url, &remote.url)`.
//! - `urlMatchesCloneURL(url, gitHubRepository)` has no Corvene function of
//!   its own: where GitHub Desktop calls it (`findRemoteBranchName`,
//!   `lib/stores/helpers/find-branch-name.ts`) Corvene calls
//!   `url_matches_remote(url, &gh.clone_url)` (`repo_rules::remote_branch_name`
//!   in `corvene-core`), which [`url_matches_clone_url`] does.
//! - GitHub Desktop's `null` HTML or clone URL (a `GitHubRepository` the API
//!   has not described, `cloneURL: null`) is an empty `String` in Corvene:
//!   `GitHubRepository::html_url` / `clone_url` are `String`s, left empty
//!   where nothing is known (`commit_status.rs` builds such a repository)
//!   and tested with `is_empty()` where GitHub Desktop tests `!== null`
//!   (the missing view's Clone Again, `Dispatcher::clone_again`). So
//!   `urlMatchesRemote(null, remote)` is `url_matches_remote("", ..)` and
//!   `repositoryWithoutCloneURL` has an empty `clone_url`.
//! - `new Account(login, endpoint, token, emails, avatarURL, id, name, plan)`
//!   is a `corvene_models::Account` with the same fields (the token lives in
//!   the keychain, not in the model).
//! - `gitHubRepoFixture` (`helpers/github-repo-builder.ts`) is
//!   `corvene_test_support::git_hub_repo_fixture`.

use corvene_models::{
    Account, GitHubRepository, Remote, github_from_remote, github_hosts, url_matches_remote,
};
use corvene_test_support::{GitHubRepoFixtureOptions, git_hub_repo_fixture};

/// `new Account(login, endpoint, '', [], '', 1, '', 'free')`, as every case
/// builds it.
fn account(login: &str, endpoint: &str) -> Account {
    Account {
        endpoint: endpoint.to_string(),
        id: 1,
        login: login.to_string(),
        name: Some(String::new()),
        avatar_url: Some(String::new()),
        emails: Vec::new(),
        scopes: Vec::new(),
        plan: Some("free".to_string()),
        private_primary_email: false,
        from_gh_cli: false,
    }
}

/// GitHub Desktop's `matchGitHubRepository(accounts, remote)`:
/// `github_from_remote` with the hosts of the signed-in accounts.
fn match_github_repository(accounts: &[Account], remote: &str) -> Option<GitHubRepository> {
    github_from_remote(remote, &github_hosts(accounts, false))
}

/// GitHub Desktop's `urlMatchesRemote(url, remote)`.
fn url_matches_remote_of(url: &str, remote: &Remote) -> bool {
    url_matches_remote(url, &remote.url)
}

/// GitHub Desktop's `urlMatchesCloneURL(url, gitHubRepository)`, as
/// Corvene's `remote_branch_name` asks it.
fn url_matches_clone_url(url: &str, github_repository: &GitHubRepository) -> bool {
    url_matches_remote(url, &github_repository.clone_url)
}

fn remote(url: &str) -> Remote {
    Remote {
        name: "origin".to_string(),
        url: url.to_string(),
    }
}

// GHD: unit/repository-matching-test.ts › repository-matching › matchGitHubRepository › matches HTTPS URLs
#[test]
fn matches_https_urls() {
    let accounts = [account("alovelace", "https://api.github.com")];
    let repo = match_github_repository(&accounts, "https://github.com/someuser/somerepo.git");
    let repo = repo.expect("repo !== null");
    assert_eq!(repo.name, "somerepo");
    assert_eq!(repo.owner, "someuser");
}

// GHD: unit/repository-matching-test.ts › repository-matching › matchGitHubRepository › matches HTTPS URLs without the git extension
#[test]
fn matches_https_urls_without_the_git_extension() {
    let accounts = [account("alovelace", "https://api.github.com")];
    let repo = match_github_repository(&accounts, "https://github.com/someuser/somerepo");
    let repo = repo.expect("repo !== null");
    assert_eq!(repo.name, "somerepo");
    assert_eq!(repo.owner, "someuser");
}

// GHD: unit/repository-matching-test.ts › repository-matching › matchGitHubRepository › matches git URLs
#[test]
fn matches_git_urls() {
    let accounts = [account("alovelace", "https://api.github.com")];
    let repo = match_github_repository(&accounts, "git:github.com/someuser/somerepo.git");
    let repo = repo.expect("repo !== null");
    assert_eq!(repo.name, "somerepo");
    assert_eq!(repo.owner, "someuser");
}

// GHD: unit/repository-matching-test.ts › repository-matching › matchGitHubRepository › matches SSH URLs
#[test]
fn matches_ssh_urls() {
    let accounts = [account("alovelace", "https://api.github.com")];
    let repo = match_github_repository(&accounts, "git@github.com:someuser/somerepo.git");
    let repo = repo.expect("repo !== null");
    assert_eq!(repo.name, "somerepo");
    assert_eq!(repo.owner, "someuser");
}

// GHD: unit/repository-matching-test.ts › repository-matching › matchGitHubRepository › doesn't match if there aren't any users with that endpoint
#[test]
fn doesnt_match_if_there_arent_any_users_with_that_endpoint() {
    let accounts = [account("alovelace", "https://github.babbageinc.com")];
    let repo = match_github_repository(&accounts, "https://github.com/someuser/somerepo.git");
    assert!(repo.is_none(), "{repo:?}");
}

// GHD: unit/repository-matching-test.ts › repository-matching › urlMatchesRemote › with HTTPS remote › does not match null
#[test]
fn with_https_remote_does_not_match_null() {
    let remote_with_suffix = remote("https://github.com/shiftkey/desktop.git");
    // `null`: a GitHub repository URL Corvene leaves empty (see the module docs)
    assert!(!url_matches_remote_of("", &remote_with_suffix));
}

// GHD: unit/repository-matching-test.ts › repository-matching › urlMatchesRemote › with HTTPS remote › matches cloneURL from API
#[test]
fn with_https_remote_matches_clone_url_from_api() {
    let remote_with_suffix = remote("https://github.com/shiftkey/desktop.git");
    let clone_url = "https://github.com/shiftkey/desktop.git";
    assert!(url_matches_remote_of(clone_url, &remote_with_suffix));
}

// GHD: unit/repository-matching-test.ts › repository-matching › urlMatchesRemote › with HTTPS remote › matches cloneURL from API with different casing
#[test]
fn with_https_remote_matches_clone_url_from_api_with_different_casing() {
    let remote_with_suffix = remote("https://github.com/shiftkey/desktop.git");
    let clone_url = "https://GITHUB.COM/SHIFTKEY/DESKTOP.git";
    assert!(url_matches_remote_of(clone_url, &remote_with_suffix));
}

// GHD: unit/repository-matching-test.ts › repository-matching › urlMatchesRemote › with HTTPS remote › matches cloneURL from API without suffix
#[test]
fn with_https_remote_matches_clone_url_from_api_without_suffix() {
    let remote = remote("https://github.com/shiftkey/desktop");
    let clone_url = "https://github.com/shiftkey/desktop.git";
    assert!(url_matches_remote_of(clone_url, &remote));
}

// GHD: unit/repository-matching-test.ts › repository-matching › urlMatchesRemote › with HTTPS remote › matches htmlURL from API
#[test]
fn with_https_remote_matches_html_url_from_api() {
    let remote_with_suffix = remote("https://github.com/shiftkey/desktop.git");
    let html_url = "https://github.com/shiftkey/desktop";
    assert!(url_matches_remote_of(html_url, &remote_with_suffix));
}

// GHD: unit/repository-matching-test.ts › repository-matching › urlMatchesRemote › with HTTPS remote › matches htmlURL from API with different casing
#[test]
fn with_https_remote_matches_html_url_from_api_with_different_casing() {
    let remote_with_suffix = remote("https://github.com/shiftkey/desktop.git");
    let html_url = "https://GITHUB.COM/SHIFTKEY/DESKTOP";
    assert!(url_matches_remote_of(html_url, &remote_with_suffix));
}

// GHD: unit/repository-matching-test.ts › repository-matching › urlMatchesRemote › with HTTPS remote › matches htmlURL from API without suffix
#[test]
fn with_https_remote_matches_html_url_from_api_without_suffix() {
    let remote = remote("https://github.com/shiftkey/desktop");
    let html_url = "https://github.com/shiftkey/desktop";
    assert!(url_matches_remote_of(html_url, &remote));
}

// GHD: unit/repository-matching-test.ts › repository-matching › urlMatchesRemote › with SSH remote › does not match null
#[test]
fn with_ssh_remote_does_not_match_null() {
    let remote = remote("git@github.com:shiftkey/desktop.git");
    // `null`: a GitHub repository URL Corvene leaves empty (see the module docs)
    assert!(!url_matches_remote_of("", &remote));
}

// GHD: unit/repository-matching-test.ts › repository-matching › urlMatchesRemote › with SSH remote › matches cloneURL from API
#[test]
fn with_ssh_remote_matches_clone_url_from_api() {
    let remote = remote("git@github.com:shiftkey/desktop.git");
    let clone_url = "https://github.com/shiftkey/desktop.git";
    assert!(url_matches_remote_of(clone_url, &remote));
}

// GHD: unit/repository-matching-test.ts › repository-matching › urlMatchesRemote › with SSH remote › matches htmlURL from API
#[test]
fn with_ssh_remote_matches_html_url_from_api() {
    let remote = remote("git@github.com:shiftkey/desktop.git");
    let html_url = "https://github.com/shiftkey/desktop";
    assert!(url_matches_remote_of(html_url, &remote));
}

/// The `cloneUrlMatches` describe's `repository`.
fn clone_url_matches_repository() -> GitHubRepository {
    git_hub_repo_fixture(GitHubRepoFixtureOptions {
        owner: "shiftkey",
        name: "desktop",
        is_private: Some(false),
        ..Default::default()
    })
}

// GHD: unit/repository-matching-test.ts › repository-matching › cloneUrlMatches › returns true for exact match
#[test]
fn returns_true_for_exact_match() {
    let repository = clone_url_matches_repository();
    assert!(url_matches_clone_url(
        "https://github.com/shiftkey/desktop.git",
        &repository
    ));
}

// GHD: unit/repository-matching-test.ts › repository-matching › cloneUrlMatches › returns true when URL doesn't have a .git suffix
#[test]
fn returns_true_when_url_doesnt_have_a_git_suffix() {
    let repository = clone_url_matches_repository();
    assert!(url_matches_clone_url(
        "https://github.com/shiftkey/desktop",
        &repository
    ));
}

// GHD: unit/repository-matching-test.ts › repository-matching › cloneUrlMatches › returns false when URL belongs to a different owner
#[test]
fn returns_false_when_url_belongs_to_a_different_owner() {
    let repository = clone_url_matches_repository();
    assert!(!url_matches_clone_url(
        "https://github.com/outofambit/desktop.git",
        &repository
    ));
}

/// The `cloneUrlMatches` describe's `repositoryWithoutCloneURL`: `dbID` 1,
/// `shiftkey/desktop` on `https://api.github.com/`, `cloneURL: null`
/// (Corvene: empty, see the module docs), not private, a fork, issues
/// enabled, not archived, no permissions and no parent.
fn repository_without_clone_url() -> GitHubRepository {
    GitHubRepository {
        endpoint: "https://api.github.com/".to_string(),
        owner: "shiftkey".to_string(),
        name: "desktop".to_string(),
        html_url: "https://github.com/shiftkey/desktop".to_string(),
        clone_url: String::new(),
        default_branch: None,
        private: false,
        fork: true,
        parent: None,
        archived: false,
        permissions: None,
        allow_forking: None,
        node_id: None,
    }
}

// GHD: unit/repository-matching-test.ts › repository-matching › cloneUrlMatches › returns false if GitHub repository does't have a cloneURL set
#[test]
fn returns_false_if_github_repository_doesnt_have_a_clone_url_set() {
    let repository_without_clone_url = repository_without_clone_url();
    assert!(!url_matches_clone_url(
        "https://github.com/shiftkey/desktop",
        &repository_without_clone_url
    ));
}
