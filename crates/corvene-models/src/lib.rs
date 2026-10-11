//! Domain models, named after GitHub Desktop's `app/src/models/*`.
//! Leaf crate: no gpui, no git, no network.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

mod hosts;
mod signature;
pub use hosts::*;
pub use signature::*;

/// A repository known to Corvene (`models/repository.ts`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repository {
    pub id: u64,
    pub path: PathBuf,
    /// User-chosen display name (`ChangeRepositoryAlias`).
    #[serde(default)]
    pub alias: Option<String>,
    /// Set when the `origin` remote points at GitHub.com / GHES.
    #[serde(default)]
    pub github: Option<GitHubRepository>,
    /// The directory disappeared; GHD shows "Can't find" and offers Locate/Remove.
    #[serde(default)]
    pub missing: bool,
    /// Commit form gear menu (`CommitOptions`, persisted per repository).
    #[serde(default)]
    pub commit_options: RepoCommitOptions,
    /// `workflowPreferences.forkContributionTarget` (Repository Settings ›
    /// Fork Behavior); `None` = the GHD default (contribute to the parent).
    #[serde(default)]
    pub fork_contribution_target: Option<ForkContributionTarget>,
    /// GHD `mainWorktreePath`: the main worktree of the repository this
    /// entry belongs to, so a deleted linked worktree falls back to it.
    #[serde(default)]
    pub main_worktree_path: Option<PathBuf>,
    /// GHD `isTutorialRepository`: created by "Create a Tutorial Repository…".
    #[serde(default)]
    pub is_tutorial_repository: bool,
    /// GHD `tagsToPush` (`storeTagsToPush`): tags created in Corvene and not
    /// pushed yet. They go out with the next push and only they can be
    /// deleted from the history context menu.
    #[serde(default)]
    pub tags_to_push: Vec<String>,
    /// Corvene (flag `267-pinned-repositories`): listed in the repository
    /// list's Pinned group.
    #[serde(default)]
    pub pinned: bool,
    /// Corvene (flag `518-per-repo-editor`): the external editor (friendly
    /// name) this repository opens in; `None` = the one in Settings.
    #[serde(default)]
    pub editor: Option<String>,
    /// Corvene (flag `518-per-repo-editor`): a custom editor this repository
    /// opens in instead (`editor` is then `None`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_editor: Option<RepoCustomEditor>,
    /// Corvene (flag `897-pinned-branches`): branch names listed in the
    /// branch list's Pinned group.
    #[serde(default)]
    pub pinned_branches: Vec<String>,
    /// Corvene (flag `290-custom-repository-groups`): the repository list
    /// group the user moved this repository to (instead of its owner's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Corvene (flag `1102-repository-credential-helper`): remote operations
    /// sign in through git's credential helper instead of the account.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub use_credential_helper: bool,
    /// Corvene (flag `341-custom-autolinks`): links for references like
    /// `TICKET-123` in commit messages, set in Repository Settings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub autolinks: Vec<Autolink>,
    /// Corvene (flag `527-multiple-accounts`): the login of the signed-in
    /// account on the repository's host that this repository uses (git
    /// sign-in, API calls); `None` = the host's first account, as in GHD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Corvene (flag `352-remember-pr-base`): the base branch (without its
    /// remote) last picked in Preview Pull Request when it was not the
    /// default branch, proposed again for the next pull request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request_base: Option<String>,
}

/// Corvene (flag `341-custom-autolinks`): a GitHub autolink reference
/// (`key_prefix` + a reference → `url_template` with `<num>` replaced).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Autolink {
    pub key_prefix: String,
    pub url_template: String,
    /// Letters and `-` may follow the prefix too, not only digits.
    #[serde(default)]
    pub is_alphanumeric: bool,
}

/// Corvene (flag `518-per-repo-editor`): a repository's own custom editor,
/// an executable and its arguments (`%TARGET_PATH%` is the path opened).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoCustomEditor {
    pub path: String,
    pub arguments: String,
    /// Its name in "Open in …" labels (empty: "Custom Editor").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
}

/// GHD `ICommitOptions`: `skipCommitHooks`, `signOffCommits`, `allowEmptyCommit`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoCommitOptions {
    pub skip_commit_hooks: bool,
    pub sign_off_commits: bool,
    pub allow_empty_commit: bool,
    /// Corvene: push the branch once a commit succeeds (flag
    /// `736-commit-and-push`).
    #[serde(default)]
    pub push_after_commit: bool,
}

impl Repository {
    pub fn new(id: u64, path: impl Into<PathBuf>) -> Self {
        Self {
            id,
            path: path.into(),
            alias: None,
            github: None,
            missing: false,
            commit_options: RepoCommitOptions::default(),
            fork_contribution_target: None,
            main_worktree_path: None,
            is_tutorial_repository: false,
            tags_to_push: Vec::new(),
            pinned: false,
            editor: None,
            custom_editor: None,
            pinned_branches: Vec::new(),
            group: None,
            use_credential_helper: false,
            autolinks: Vec::new(),
            account: None,
            pull_request_base: None,
        }
    }

    /// Alias, else GitHub repo name, else the directory name (GHD
    /// `repository.alias ?? repository.name`, the list's display title).
    pub fn name(&self) -> String {
        if let Some(alias) = &self.alias {
            return alias.clone();
        }
        if let Some(gh) = &self.github {
            return gh.name.clone();
        }
        dir_name(&self.path)
    }
}

/// GHD `nameOf` (`models/repository.ts`): `owner/name` of the GitHub
/// repository, else the directory name; the alias plays no part.
pub fn name_of(repository: &Repository) -> String {
    match &repository.github {
        Some(gh) => gh.full_name(),
        None => dir_name(&repository.path),
    }
}

pub fn dir_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// A GitHub-hosted repository (`models/github-repository.ts`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubRepository {
    /// API base, e.g. `https://api.github.com`.
    pub endpoint: String,
    pub owner: String,
    pub name: String,
    pub html_url: String,
    pub clone_url: String,
    #[serde(default)]
    pub default_branch: Option<String>,
    #[serde(default)]
    pub private: bool,
    #[serde(default)]
    pub fork: bool,
    #[serde(default)]
    pub parent: Option<Box<GitHubRepository>>,
    /// Shown as an "Archived" badge in the clone list.
    #[serde(default)]
    pub archived: bool,
    /// The signed-in user's permission (`GitHubRepositoryPermission`), from
    /// `GET /repos/{owner}/{name}`; `None` when unknown.
    #[serde(default)]
    pub permissions: Option<RepositoryPermission>,
    /// The API's `allow_forking`; `None` when unknown (not in GHD's model).
    #[serde(default)]
    pub allow_forking: Option<bool>,
    /// The API's GraphQL `node_id`, for linking a branch to an issue
    /// (flag `345-issues`); `None` for records stored before it was read.
    #[serde(default)]
    pub node_id: Option<String>,
}

/// GHD `GitHubRepositoryPermission`
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepositoryPermission {
    Read,
    Write,
    Admin,
}

impl GitHubRepository {
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    /// The owner turned forking off (`allow_forking: false`).
    pub fn forking_disabled(&self) -> bool {
        self.allow_forking == Some(false)
    }

    /// A wiki repository (`owner/name.wiki`): it can be cloned, pushed and
    /// pulled, but the REST API knows nothing about it.
    pub fn is_wiki(&self) -> bool {
        self.name.ends_with(".wiki")
    }

    /// `hasWritePermission`: can the user push? Unknown permissions count
    /// as writable.
    pub fn has_write_permission(&self) -> bool {
        self.permissions != Some(RepositoryPermission::Read)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BranchKind {
    Local,
    Remote,
}

/// `models/branch.ts`
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Branch {
    /// Short name, e.g. `main` or `origin/main`.
    pub name: String,
    pub kind: BranchKind,
    /// Full ref name, e.g. `refs/heads/main`.
    pub full_name: String,
    /// Commit id the branch points at, if resolvable.
    pub tip: Option<String>,
    /// Upstream tracking branch full name (`refs/remotes/origin/main`), local branches only.
    pub upstream: Option<String>,
    /// Committer time of the tip commit (seconds since the epoch), for the
    /// relative dates in the branch list.
    #[serde(default)]
    pub tip_time: Option<i64>,
    /// The tip commit's author name, for the branch list (Corvene,
    /// `1201-branch-list-tip-author`).
    #[serde(default)]
    pub tip_author: Option<String>,
    /// The configured remote a remote branch belongs to (or, for a local
    /// branch, its upstream's), matched against the remote names so a name
    /// with slashes (`team/fork`) is kept whole. `None` falls back to GHD's
    /// split at the first `/` (flag `remote-names-with-slashes` off).
    #[serde(default)]
    pub remote_name: Option<String>,
}

impl Branch {
    /// `nameWithoutRemote`: `origin/feature` → `feature`.
    pub fn name_without_remote(&self) -> &str {
        match self.kind {
            BranchKind::Local => &self.name,
            BranchKind::Remote => self
                .remote_name
                .as_deref()
                .and_then(|remote| self.name.strip_prefix(remote)?.strip_prefix('/'))
                .or_else(|| remove_remote_prefix(&self.name))
                .unwrap_or(&self.name),
        }
    }

    /// `upstreamWithoutRemote`: the upstream's branch name without its
    /// remote (`refs/remotes/origin/feature` → `feature`), `None` without an
    /// upstream. Splits after [`Branch::upstream_remote_name`], so a remote
    /// name with slashes stays whole (flag `remote-names-with-slashes`).
    pub fn upstream_without_remote(&self) -> Option<&str> {
        let upstream = self.upstream_short()?;
        self.upstream_remote_name()
            .and_then(|remote| upstream.strip_prefix(remote)?.strip_prefix('/'))
            .or_else(|| remove_remote_prefix(upstream))
    }

    /// `upstreamRemoteName`
    pub fn upstream_remote_name(&self) -> Option<&str> {
        let upstream = self.upstream.as_deref()?;
        let rest = upstream.strip_prefix("refs/remotes/").unwrap_or(upstream);
        if let Some(remote) = self.remote_name.as_deref()
            && rest
                .strip_prefix(remote)
                .is_some_and(|r| r.starts_with('/'))
        {
            return Some(remote);
        }
        rest.split_once('/').map(|(remote, _)| remote)
    }

    /// The longest of `remotes` that `short` (`<remote>/<branch>`) starts
    /// with, for [`Branch::remote_name`].
    pub fn match_remote<'a>(short: &str, remotes: &'a [Remote]) -> Option<&'a str> {
        remotes
            .iter()
            .map(|r| r.name.as_str())
            .filter(|name| {
                short
                    .strip_prefix(name)
                    .is_some_and(|rest| rest.starts_with('/'))
            })
            .max_by_key(|name| name.len())
    }

    /// Short upstream name (`origin/main`).
    pub fn upstream_short(&self) -> Option<&str> {
        self.upstream
            .as_deref()
            .map(|u| u.strip_prefix("refs/remotes/").unwrap_or(u))
    }
}

/// GHD `removeRemotePrefix` (`lib/remove-remote-prefix.ts`): everything
/// after the first `/` (`origin/thing/my-branch` → `thing/my-branch`),
/// `None` without one.
pub fn remove_remote_prefix(name: &str) -> Option<&str> {
    name.split_once('/').map(|(_, rest)| rest)
}

/// `IStashEntry`
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StashEntry {
    /// `stash@{0}`
    pub name: String,
    pub sha: String,
    /// Branch recorded in a GitHub Desktop / Corvene stash message.
    pub branch: Option<String>,
    pub message: String,
    pub tree: String,
    pub parents: Vec<String>,
    /// Corvene (`797-stash-list`): when the entry was made, in seconds
    /// since the Unix epoch (0 when unknown).
    #[serde(default)]
    pub date: i64,
}

/// `models/tip.ts`
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tip {
    Unknown,
    Unborn { name: String },
    Detached { sha: String },
    Valid { branch: Branch },
}

impl Tip {
    pub fn branch_name(&self) -> Option<&str> {
        match self {
            Tip::Valid { branch } => Some(&branch.name),
            Tip::Unborn { name } => Some(name),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AheadBehind {
    pub ahead: u32,
    pub behind: u32,
}

/// `models/remote.ts`
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Remote {
    pub name: String,
    pub url: String,
}

/// A signed-in GitHub account (`models/account.ts`). The token lives in the
/// OS keychain, keyed by `endpoint` + `login`, never in the store.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// API base URL, e.g. `https://api.github.com` or `https://ghe.corp/api/v3`.
    pub endpoint: String,
    pub id: u64,
    pub login: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub emails: Vec<String>,
    /// OAuth scopes granted to the token.
    #[serde(default)]
    pub scopes: Vec<String>,
    /// `plan.name` of `/user` (`free`, `pro`, …); `None` until the account
    /// has been read from the API since this field was added.
    #[serde(default)]
    pub plan: Option<String>,
    /// The primary address (`emails[0]`) is set to private on GitHub
    /// (`visibility: "private"` in `/user/emails`).
    #[serde(default)]
    pub private_primary_email: bool,
}

impl Account {
    /// `endpoint|login`: one account among several on the same endpoint
    /// (flag `527-multiple-accounts`), as settings and caches key it.
    pub fn key(&self) -> String {
        format!("{}|{}", self.endpoint, self.login)
    }

    pub fn is_dotcom(&self) -> bool {
        self.endpoint == "https://api.github.com"
    }

    /// GHD `Account.friendlyEndpoint`: "GitHub.com", else the Enterprise host.
    pub fn friendly_endpoint(&self) -> String {
        if self.is_dotcom() {
            "GitHub.com".to_string()
        } else {
            self.host()
        }
    }

    /// GHD `lookupPreferredEmail`: the primary address when it is public,
    /// else the account's no-reply address when it is listed, else the first
    /// address; the no-reply address when the account has none.
    pub fn preferred_email(&self) -> String {
        let Some(first) = self.emails.first() else {
            return stealth_email(self.id, &self.login, &self.endpoint);
        };
        if !self.private_primary_email {
            return first.clone();
        }
        let legacy = legacy_stealth_email(&self.login, &self.endpoint);
        let suffix = &legacy[legacy.find('@').unwrap_or(0)..];
        self.emails
            .iter()
            .find(|e| e.to_lowercase().ends_with(suffix))
            .unwrap_or(first)
            .clone()
    }

    /// GHD `isAttributableEmailFor`: commits with `email` are linked to this
    /// account (a verified address or either no-reply form).
    pub fn is_attributable_email(&self, email: &str) -> bool {
        let needle = email.to_lowercase();
        self.emails.iter().any(|e| e.to_lowercase() == needle)
            || stealth_email(self.id, &self.login, &self.endpoint).to_lowercase() == needle
            || legacy_stealth_email(&self.login, &self.endpoint).to_lowercase() == needle
    }

    /// Host for keychain keys and remote matching (`github.com`, `ghe.corp`):
    /// the web host (GHD `getHTMLURL`), so a `*.ghe.com` account on the
    /// `api.` subdomain (`https://api.x.ghe.com/`) is `x.ghe.com`.
    pub fn host(&self) -> String {
        let trimmed = self
            .endpoint
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        let host = trimmed.split('/').next().unwrap_or(trimmed);
        if host == "api.github.com" {
            "github.com".to_string()
        } else if host.to_ascii_lowercase().ends_with(".ghe.com")
            && let Some(web) = host.strip_prefix("api.")
        {
            web.to_string()
        } else {
            host.to_string()
        }
    }
}

/// Author identity from git config (`models/commit-identity.ts`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Identity {
    pub name: Option<String>,
    pub email: Option<String>,
    /// The repository's own config sets `user.name` or `user.email` (GHD
    /// `isGitConfigLocal` in `ui/changes/commit-message-avatar.tsx`); false
    /// for the global identity.
    #[serde(default)]
    pub local: bool,
}

/// What Corvene knows about an opened repository (subset of GHD's
/// `IRepositoryState`, filled by `corvene-git`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryInfo {
    pub workdir: PathBuf,
    pub tip: Tip,
    pub branches: Vec<Branch>,
    pub remotes: Vec<Remote>,
    pub identity: Identity,
    pub ahead_behind: Option<AheadBehind>,
    /// `commit.template` contents with comment lines removed (`None` when
    /// unset, unreadable or empty).
    pub commit_template: Option<String>,
    /// The patterns of the `diff.orderFile` file (blank and `#` lines left
    /// out; empty when unset or unreadable), for the changes list's
    /// "order-file" order (Corvene flag `703-changes-sort-order`).
    #[serde(default)]
    pub diff_order: Vec<String>,
}

impl RepositoryInfo {
    pub fn current_branch(&self) -> Option<&Branch> {
        match &self.tip {
            Tip::Valid { branch } => Some(branch),
            _ => None,
        }
    }

    pub fn remote(&self, name: &str) -> Option<&Remote> {
        self.remotes.iter().find(|r| r.name == name)
    }
}

/// GHD `gitAuthorNameIsValid`: git strips "crud" characters from names and
/// refuses one that consists only of them (`ident.c`). Empty is valid.
pub fn git_author_name_is_valid(name: &str) -> bool {
    !(!name.is_empty()
        && name.chars().all(|c| {
            (c as u32) <= 0x20 || matches!(c, '.' | ',' | ':' | ';' | '<' | '>' | '"' | '\\' | '\'')
        }))
}

/// GHD `InvalidGitAuthorNameMessage`.
pub const INVALID_GIT_AUTHOR_NAME_MESSAGE: &str =
    "Name is invalid, it consists only of disallowed characters.";

/// GHD `matchGitHubRepository` (`lib/repository-matching.ts`): `owner/name`
/// and host of a remote URL whose host is one of `hosts`, the web hosts of
/// the signed-in accounts (GHD `getHTMLURL(account.endpoint)`; see
/// [`github_hosts`]). Handles every URL [`split_remote`] takes
/// (`https://github.com/o/n(.git)`, `git@github.com:o/n(.git)`,
/// `git:github.com/o/n`, `ssh://git@github.com/o/n`…). `github.com` only
/// matches when it is listed, as in GHD.
pub fn github_from_remote(url: &str, hosts: &[String]) -> Option<GitHubRepository> {
    let (host, path) = split_remote(url)?;
    if !hosts.iter().any(|h| h.eq_ignore_ascii_case(&host)) {
        return None;
    }
    let is_dotcom = host.eq_ignore_ascii_case("github.com");
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.splitn(2, '/');
    let owner = parts.next()?.to_string();
    let name = parts.next()?.trim_end_matches('/').to_string();
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return None;
    }
    // GHD `getEnterpriseAPIURL`: `*.ghe.com` serves its API on `api.`
    let endpoint = if is_dotcom {
        "https://api.github.com".to_string()
    } else if host.to_ascii_lowercase().ends_with(".ghe.com") {
        format!("https://api.{host}/")
    } else {
        format!("https://{host}/api/v3")
    };
    Some(GitHubRepository {
        endpoint,
        html_url: format!("https://{host}/{owner}/{name}"),
        clone_url: format!("https://{host}/{owner}/{name}.git"),
        owner,
        name,
        default_branch: None,
        private: false,
        fork: false,
        parent: None,
        archived: false,
        permissions: None,
        allow_forking: None,
        node_id: None,
    })
}

/// The hosts [`github_from_remote`] matches for these accounts: GHD
/// `matchGitHubRepository` matches the web host of each signed-in account
/// (`Account::host`). With `dotcom`, `github.com` is listed even without a
/// GitHub.com account (Corvene, flag `331-github-without-account`: a
/// github.com remote is a GitHub repository when signed out, so View on
/// GitHub, the pull request links and the signed-out Pull Requests tab
/// work; GHD leaves it a plain repository).
pub fn github_hosts(accounts: &[Account], dotcom: bool) -> Vec<String> {
    let mut hosts: Vec<String> = accounts.iter().map(Account::host).collect();
    if dotcom && !hosts.iter().any(|h| h.eq_ignore_ascii_case("github.com")) {
        hosts.push("github.com".to_string());
    }
    hosts
}

/// `(host, path)` of a remote URL (GHD `parseRemote`, `lib/remote-parsing.ts`):
/// `https://`, `http://`, `ssh://` and `git://` URLs, `git:host/path`, and
/// scp-style `git@host:path` (any user for `*.ghe.com` hosts, as GHD).
/// `None` for anything else.
///
/// Deviation: GHD reads owner and name out of the URL with its regexes, so
/// it takes `ssh://` URLs with the `git` user only, keeps a port in the
/// host and reads `git://host/o/n` as host `//host`; Corvene takes any
/// user, drops the port and returns the whole path for any host.
pub fn split_remote(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    for scheme in ["https://", "http://", "ssh://", "git://"] {
        if let Some(rest) = url.strip_prefix(scheme) {
            let rest = rest.split_once('@').map(|(_, r)| r).unwrap_or(rest);
            let (host, path) = rest.split_once('/')?;
            let host = host.split(':').next()?.to_string();
            return Some((host, path.to_string()));
        }
    }
    if let Some(rest) = url.strip_prefix("git:") {
        // git:host/owner/name
        let (host, path) = rest.split_once('/')?;
        return Some((host.to_string(), path.to_string()));
    }
    if let Some(rest) = url.strip_prefix("git@") {
        // git@host:owner/name
        let (host, path) = rest.split_once(':')?;
        return Some((host.to_string(), path.to_string()));
    }
    // user@host.ghe.com:owner/name
    let (prefix, path) = url.split_once(':')?;
    let (user, host) = prefix.rsplit_once('@')?;
    let ghe = host.len() > ".ghe.com".len() && host.to_ascii_lowercase().ends_with(".ghe.com");
    (!user.is_empty() && ghe && !prefix.contains('/')).then(|| (host.to_string(), path.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(emails: &[&str], private_primary_email: bool) -> Account {
        Account {
            endpoint: "https://api.github.com".into(),
            id: 583231,
            login: "octocat".into(),
            name: None,
            avatar_url: None,
            emails: emails.iter().map(|e| e.to_string()).collect(),
            scopes: Vec::new(),
            plan: None,
            private_primary_email,
        }
    }

    #[test]
    fn preferred_email_follows_lookup_preferred_email() {
        let noreply = "583231+octocat@users.noreply.github.com";
        assert_eq!(account(&[], false).preferred_email(), noreply);
        assert_eq!(
            account(&["mona@example.com", noreply], false).preferred_email(),
            "mona@example.com"
        );
        assert_eq!(
            account(&["mona@example.com", noreply], true).preferred_email(),
            noreply
        );
        assert_eq!(
            account(&["mona@example.com", "other@example.com"], true).preferred_email(),
            "mona@example.com"
        );
    }

    #[test]
    fn stealth_email_logins() {
        let dotcom = "https://api.github.com";
        assert_eq!(
            stealth_email_login("583231+OctoCat@users.noreply.github.com", dotcom).as_deref(),
            Some("OctoCat")
        );
        assert_eq!(
            stealth_email_login("octocat@users.noreply.github.com", dotcom).as_deref(),
            Some("octocat")
        );
        assert_eq!(
            stealth_email_login("1+dependabot[bot]@users.noreply.github.com", dotcom).as_deref(),
            Some("dependabot[bot]")
        );
        assert_eq!(
            stealth_email_login("1+mona@users.noreply.ghe.corp", "https://ghe.corp/api/v3")
                .as_deref(),
            Some("mona")
        );
        assert_eq!(stealth_email_login("mona@example.com", dotcom), None);
        assert_eq!(
            stealth_email_login("x+mona@users.noreply.github.com", dotcom),
            None
        );
        assert_eq!(
            stealth_email_login("@users.noreply.github.com", dotcom),
            None
        );
    }

    #[test]
    fn attributable_emails_include_both_noreply_forms() {
        let a = account(&["Mona@Example.com"], false);
        assert!(a.is_attributable_email("mona@example.com"));
        assert!(a.is_attributable_email("583231+octocat@users.noreply.github.com"));
        assert!(a.is_attributable_email("octocat@users.noreply.github.com"));
        assert!(!a.is_attributable_email("someone@example.com"));
    }

    fn dotcom() -> Vec<String> {
        github_hosts(&[], true)
    }

    #[test]
    fn parses_github_remotes() {
        for url in [
            "https://github.com/wasi-master/corvene.git",
            "https://github.com/wasi-master/corvene",
            "git@github.com:wasi-master/corvene.git",
            "ssh://git@github.com/wasi-master/corvene.git",
            "https://user@github.com/wasi-master/corvene/",
            "git:github.com/wasi-master/corvene.git",
        ] {
            let gh = github_from_remote(url, &dotcom()).unwrap_or_else(|| panic!("{url}"));
            assert_eq!(gh.owner, "wasi-master");
            assert_eq!(gh.name, "corvene");
            assert_eq!(gh.html_url, "https://github.com/wasi-master/corvene");
            assert_eq!(gh.endpoint, "https://api.github.com");
        }
    }

    #[test]
    fn ignores_non_github_and_ghes_without_config() {
        assert!(github_from_remote("https://gitlab.com/a/b.git", &dotcom()).is_none());
        assert!(github_from_remote("git@ghe.corp:a/b.git", &dotcom()).is_none());
        let gh = github_from_remote("git@ghe.corp:a/b.git", &["ghe.corp".into()]).unwrap();
        assert_eq!(gh.endpoint, "https://ghe.corp/api/v3");
        // GHD: only the hosts of signed-in accounts match
        assert!(github_from_remote("git@github.com:a/b.git", &[]).is_none());
        assert!(github_from_remote("git@github.com:a/b.git", &["ghe.corp".into()]).is_none());
    }

    #[test]
    fn splits_scp_style_remotes_like_parse_remote() {
        assert_eq!(
            split_remote("niik@niik.ghe.com:hubot/repo.git"),
            Some(("niik.ghe.com".into(), "hubot/repo.git".into()))
        );
        assert_eq!(split_remote("work@gh-alias:team/app"), None);
        assert_eq!(split_remote("@x.ghe.com:o/n"), None);
        assert_eq!(
            split_remote("git:github.com/hubot/repo/"),
            Some(("github.com".into(), "hubot/repo/".into()))
        );
        assert_eq!(
            split_remote("git://github.com/hubot/repo"),
            Some(("github.com".into(), "hubot/repo".into()))
        );
        assert_eq!(split_remote("/local/path"), None);
        assert_eq!(split_remote("C:\\repos\\x"), None);
    }

    #[test]
    fn github_hosts_add_dotcom_once() {
        let mut ghes = account(&[], false);
        ghes.endpoint = "https://ghe.corp/api/v3".into();
        assert_eq!(github_hosts(&[ghes.clone()], false), ["ghe.corp"]);
        assert_eq!(github_hosts(&[ghes], true), ["ghe.corp", "github.com"]);
        assert_eq!(github_hosts(&[account(&[], false)], true), ["github.com"]);
    }

    #[test]
    fn remote_names_with_slashes() {
        let remotes = vec![
            Remote {
                name: "team".into(),
                url: String::new(),
            },
            Remote {
                name: "team/fork".into(),
                url: String::new(),
            },
        ];
        assert_eq!(
            Branch::match_remote("team/fork/main", &remotes),
            Some("team/fork")
        );
        assert_eq!(Branch::match_remote("team/main", &remotes), Some("team"));
        assert_eq!(Branch::match_remote("other/main", &remotes), None);
        let mut remote = Branch {
            name: "team/fork/feature/x".into(),
            kind: BranchKind::Remote,
            full_name: "refs/remotes/team/fork/feature/x".into(),
            tip: None,
            upstream: None,
            tip_time: None,
            tip_author: None,
            remote_name: None,
        };
        // GHD: split at the first slash
        assert_eq!(remote.name_without_remote(), "fork/feature/x");
        remote.remote_name = Some("team/fork".into());
        assert_eq!(remote.name_without_remote(), "feature/x");
        let mut local = Branch {
            name: "feature/x".into(),
            kind: BranchKind::Local,
            full_name: "refs/heads/feature/x".into(),
            tip: None,
            upstream: Some("refs/remotes/team/fork/feature/x".into()),
            tip_time: None,
            tip_author: None,
            remote_name: None,
        };
        assert_eq!(local.upstream_remote_name(), Some("team"));
        local.remote_name = Some("team/fork".into());
        assert_eq!(local.upstream_remote_name(), Some("team/fork"));
    }

    #[test]
    fn detects_wiki_remotes() {
        let wiki = github_from_remote("https://github.com/o/n.wiki.git", &dotcom()).unwrap();
        assert!(wiki.is_wiki());
        assert!(
            !github_from_remote("git@github.com:o/n.git", &dotcom())
                .unwrap()
                .is_wiki()
        );
    }

    #[test]
    fn repository_name_precedence() {
        let mut repo = Repository::new(1, "/tmp/my-dir");
        assert_eq!(repo.name(), "my-dir");
        repo.github = github_from_remote("git@github.com:o/gh-name.git", &dotcom());
        assert_eq!(repo.name(), "gh-name");
        repo.alias = Some("Alias".into());
        assert_eq!(repo.name(), "Alias");
    }
}

/// Which sidebar section is shown (`RepositorySectionTab`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Section {
    #[default]
    Changes,
    History,
}

/// Settings › Appearance › Design style: which design language the Android
/// app draws with (the desktop app only stores it). Corvene addition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DesignStyle {
    /// GitHub for Android: Primer colours over Material components, Inter,
    /// Octicons, a bottom navigation bar.
    #[default]
    GitHubMobile,
    /// GitHub Desktop's look transplanted: its tokens, toolbar and density.
    GitHubDesktop,
    /// Material 3 with the device's dynamic colours.
    Material,
}

impl DesignStyle {
    pub fn as_str(self) -> &'static str {
        match self {
            DesignStyle::GitHubMobile => "github-mobile",
            DesignStyle::GitHubDesktop => "github-desktop",
            DesignStyle::Material => "material",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "github-mobile" => Some(DesignStyle::GitHubMobile),
            "github-desktop" => Some(DesignStyle::GitHubDesktop),
            "material" => Some(DesignStyle::Material),
            _ => None,
        }
    }
}

/// Settings › Appearance › Theme (`ApplicationTheme`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeSetting {
    Light,
    Dark,
    #[default]
    System,
    /// Corvene addition (GHD has no high contrast theme).
    HighContrast,
}

/// Settings › Appearance › Syntax highlighting (Corvene addition, offered by
/// flag `105-tree-sitter-highlighting`; GHD always runs its CodeMirror modes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum SyntaxHighlighter {
    /// GHD's CodeMirror modes, syntect for the rest
    #[default]
    #[serde(rename = "github-desktop")]
    GitHubDesktop,
    /// GHD's modes where it has one, tree-sitter for other languages
    #[serde(rename = "tree-sitter-fallback")]
    TreeSitterFallback,
    /// tree-sitter wherever it has a grammar
    #[serde(rename = "tree-sitter")]
    TreeSitter,
}

// ---- working directory status (`models/status.ts`) ----

/// `AppFileStatusKind`
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FileStatusKind {
    New,
    Modified,
    Deleted,
    Copied,
    Renamed,
    Conflicted,
    Untracked,
}

impl FileStatusKind {
    /// GHD's status octicon per kind (`ui/octicons/status.ts`).
    pub fn is_new_or_untracked(self) -> bool {
        matches!(self, FileStatusKind::New | FileStatusKind::Untracked)
    }
}

/// Which side of the index a change lives on (porcelain X / Y columns).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GitStatusEntry {
    Unchanged,
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    Unmerged,
    Untracked,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStatus {
    pub kind: FileStatusKind,
    pub index: GitStatusEntry,
    pub working_tree: GitStatusEntry,
    /// `R100`/`C90` similarity score for renames and copies.
    pub score: Option<u8>,
    /// Two-letter porcelain code (`.M`, `UU`, `??`), kept for conflict handling.
    pub code: String,
    pub submodule: bool,
    /// Porcelain v2 `S<c><m><u>` flags for submodules (GHD `SubmoduleStatus`).
    #[serde(default)]
    pub submodule_status: Option<SubmoduleStatus>,
    /// Conflicted text files: number of leftover `<<<<<<<`/`=======`/`>>>>>>>`
    /// markers (0 once resolved in an editor). `None` for binary / delete
    /// conflicts that need a manual "use ours / theirs" choice
    /// (GHD `ConflictsWithMarkers` vs `ManualConflict`).
    #[serde(default)]
    pub conflict_markers: Option<u32>,
}

impl FileStatus {
    /// GHD `isConflictedFile` (`lib/status.ts`)
    pub fn is_conflicted(&self) -> bool {
        self.kind == FileStatusKind::Conflicted
    }

    /// GHD `isConflictWithMarkers`
    pub fn is_text_conflict(&self) -> bool {
        self.is_conflicted() && self.conflict_markers.is_some()
    }

    /// GHD `isManualConflict`
    pub fn is_manual_conflict(&self) -> bool {
        self.is_conflicted() && self.conflict_markers.is_none()
    }

    /// GHD `CopiedOrRenamedFileStatus.renameIncludesModifications`
    /// (`models/status.ts`): a rename whose contents changed too. Status
    /// (`lib/git/status.ts` `convertToAppStatus`): modified in the working
    /// tree or a similarity score under 100; history (`lib/git/log.ts`
    /// `mapStatus`): `R<score>` other than `R100`, never a bare `R`. Always
    /// false for copies and every other kind.
    pub fn rename_includes_modifications(&self) -> bool {
        self.kind == FileStatusKind::Renamed
            && (self.working_tree == GitStatusEntry::Modified
                || self.score.is_some_and(|score| score < 100))
    }

    /// GHD `hasUnresolvedConflicts`: a manual choice resolves anything; text
    /// conflicts are resolved once no markers remain; binary conflicts never
    /// resolve on their own.
    pub fn has_unresolved_conflicts(&self, resolution: Option<ManualConflictResolution>) -> bool {
        if resolution.is_some() {
            return false;
        }
        match self.conflict_markers {
            Some(count) => count > 0,
            None => true,
        }
    }

    /// GHD `status.entry.us` for unmerged entries (`u XY`: X is our side).
    pub fn us(&self) -> GitStatusEntry {
        self.index
    }

    /// GHD `status.entry.them` (Y is their side).
    pub fn them(&self) -> GitStatusEntry {
        self.working_tree
    }
}

/// GHD `ManualConflictResolution`
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ManualConflictResolution {
    Ours,
    Theirs,
}

/// GHD `CommitOneLine`
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitOneLine {
    pub sha: String,
    pub summary: String,
}

/// GHD `MultiCommitOperationKind`; `label()` is the capitalised user-facing name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MultiCommitOperationKind {
    Rebase,
    CherryPick,
    Squash,
    Merge,
    Reorder,
}

impl MultiCommitOperationKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Rebase => "Rebase",
            Self::CherryPick => "Cherry-pick",
            Self::Squash => "Squash",
            Self::Merge => "Merge",
            Self::Reorder => "Reorder",
        }
    }

    pub fn lower(self) -> &'static str {
        match self {
            Self::Rebase => "rebase",
            Self::CherryPick => "cherry-pick",
            Self::Squash => "squash",
            Self::Merge => "merge",
            Self::Reorder => "reorder",
        }
    }
}

/// GHD `IMultiCommitOperationProgress`
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct McoProgress {
    /// 0..=1
    pub value: f32,
    /// 1-based index of the commit being applied.
    pub position: usize,
    pub total: usize,
    pub current_summary: String,
}

/// GHD `RebaseInternalState` (`.git/rebase-merge/{head-name,onto,orig-head}`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RebaseInternalState {
    /// The branch being rebased (`head-name` without `refs/heads/`).
    pub target_branch: String,
    /// `onto`: the commit the branch is replayed on top of.
    pub base_branch_tip: String,
    /// `orig-head`: the branch tip before the rebase started.
    pub original_branch_tip: String,
}

/// GHD `MergeTreeResult` / `ComputedAction` for a would-be merge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mergeability {
    Clean,
    Conflicts(u32),
    /// Unrelated histories.
    Invalid,
}

/// How much of a file is included in the next commit (`DiffSelectionType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffSelectionType {
    All,
    Partial,
    None,
}

/// GHD `DiffSelection`: a default state (all or none) plus the lines that
/// diverge from it. `selectable` bounds what "all" means once a diff is
/// loaded. Line indices are absolute positions in the unified diff (hunk
/// header lines included), i.e. `hunk.unified_diff_start + index`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffSelection {
    default_all: bool,
    diverging: BTreeSet<u32>,
    selectable: Option<BTreeSet<u32>>,
}

impl Default for DiffSelection {
    fn default() -> Self {
        Self::all()
    }
}

impl DiffSelection {
    pub fn all() -> Self {
        Self {
            default_all: true,
            diverging: BTreeSet::new(),
            selectable: None,
        }
    }

    pub fn none() -> Self {
        Self {
            default_all: false,
            diverging: BTreeSet::new(),
            selectable: None,
        }
    }

    fn default_kind(&self) -> DiffSelectionType {
        if self.default_all {
            DiffSelectionType::All
        } else {
            DiffSelectionType::None
        }
    }

    fn inverse_kind(&self) -> DiffSelectionType {
        if self.default_all {
            DiffSelectionType::None
        } else {
            DiffSelectionType::All
        }
    }

    /// `getSelectionType`
    pub fn kind(&self) -> DiffSelectionType {
        if self.diverging.is_empty() {
            return self.default_kind();
        }
        match &self.selectable {
            None => DiffSelectionType::Partial,
            Some(selectable) => {
                if selectable.len() == self.diverging.len()
                    && selectable.iter().all(|l| self.diverging.contains(l))
                {
                    self.inverse_kind()
                } else {
                    DiffSelectionType::Partial
                }
            }
        }
    }

    pub fn is_selected(&self, line: u32) -> bool {
        let diverges = self.diverging.contains(&line);
        if self.default_all {
            !diverges
        } else {
            diverges
        }
    }

    pub fn is_selectable(&self, line: u32) -> bool {
        self.selectable.as_ref().is_none_or(|s| s.contains(&line))
    }

    /// `isRangeSelected`: All / None / Partial for `len` lines from `from`.
    pub fn range_kind(&self, from: u32, len: u32) -> DiffSelectionType {
        if len == 0 {
            return DiffSelectionType::None;
        }
        let kind = self.kind();
        if kind != DiffSelectionType::Partial {
            return kind;
        }
        let first = self.is_selected(from);
        for line in from + 1..from + len {
            if self.is_selected(line) != first {
                return DiffSelectionType::Partial;
            }
        }
        if first {
            DiffSelectionType::All
        } else {
            DiffSelectionType::None
        }
    }

    pub fn with_line(&self, line: u32, selected: bool) -> Self {
        self.with_range(line, 1, selected)
    }

    /// `withRangeSelection`: mark `len` lines from `from` (only selectable ones).
    pub fn with_range(&self, from: u32, len: u32, selected: bool) -> Self {
        let kind = self.kind();
        let already = (kind == DiffSelectionType::All && selected)
            || (kind == DiffSelectionType::None && !selected);
        if already {
            return self.clone();
        }
        if kind == DiffSelectionType::Partial {
            let mut diverging = self.diverging.clone();
            let matches_default = self.default_all == selected;
            for line in from..from + len {
                if matches_default {
                    diverging.remove(&line);
                } else if self.is_selectable(line) {
                    diverging.insert(line);
                }
            }
            Self {
                default_all: self.default_all,
                diverging,
                selectable: self.selectable.clone(),
            }
        } else {
            // Re-base on the computed state so "all lines flipped" collapses.
            let diverging = (from..from + len)
                .filter(|l| self.is_selectable(*l))
                .collect();
            Self {
                default_all: kind == DiffSelectionType::All,
                diverging,
                selectable: self.selectable.clone(),
            }
        }
    }

    pub fn toggled(&self, line: u32) -> Self {
        self.with_line(line, !self.is_selected(line))
    }

    pub fn select_all(&self) -> Self {
        Self {
            default_all: true,
            diverging: BTreeSet::new(),
            selectable: self.selectable.clone(),
        }
    }

    pub fn select_none(&self) -> Self {
        Self {
            default_all: false,
            diverging: BTreeSet::new(),
            selectable: self.selectable.clone(),
        }
    }

    /// Whether lines outside the diverging ones are selected.
    pub fn defaults_to_selected(&self) -> bool {
        self.default_all
    }

    /// A selection of `selectable` lines that are all selected
    /// (`default_all`) or not, except the `diverging` ones (outside
    /// `selectable` they are dropped).
    pub fn from_parts(
        default_all: bool,
        diverging: BTreeSet<u32>,
        selectable: BTreeSet<u32>,
    ) -> Self {
        Self {
            default_all,
            diverging: diverging.intersection(&selectable).copied().collect(),
            selectable: Some(selectable),
        }
    }

    /// `withSelectableLines`: drops diverging lines that no longer exist.
    pub fn with_selectable_lines(&self, selectable: BTreeSet<u32>) -> Self {
        let diverging = self
            .diverging
            .iter()
            .copied()
            .filter(|l| selectable.contains(l))
            .collect();
        Self {
            default_all: self.default_all,
            diverging,
            selectable: Some(selectable),
        }
    }
}

/// `WorkingDirectoryFileChange`
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkingDirectoryFileChange {
    /// Repository-relative path (new path for renames).
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub selection: DiffSelection,
}

/// GHD `extract` (`ui/lib/path-text.tsx`): the file name and the directory
/// (with its trailing `/`) of a repository path. A trailing `/`, which git
/// gives an untracked nested repository (`some/submodule/path/`), is dropped
/// first.
pub fn file_name_and_directory(path: &str) -> (&str, &str) {
    let path = path.strip_suffix('/').unwrap_or(path);
    match path.rfind('/') {
        Some(ix) => (&path[ix + 1..], &path[..=ix]),
        None => (path, ""),
    }
}

impl WorkingDirectoryFileChange {
    pub fn file_name(&self) -> &str {
        file_name_and_directory(&self.path).0
    }

    pub fn directory(&self) -> &str {
        file_name_and_directory(&self.path).1
    }
}

/// `WorkingDirectoryStatus`
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WorkingDirectoryStatus {
    pub files: Vec<WorkingDirectoryFileChange>,
    /// Branch header info from `# branch.*` (None when detached/unborn).
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead_behind: Option<AheadBehind>,
    pub merge_head_found: bool,
    pub rebase_in_progress: bool,
    /// `# branch.oid`
    #[serde(default)]
    pub current_tip: Option<String>,
    /// `.git/CHERRY_PICK_HEAD` exists (`isCherryPickingHeadFound`).
    #[serde(default)]
    pub cherry_pick_head_found: bool,
    /// `.git/SQUASH_MSG` exists (a `merge --squash` that has not been committed).
    #[serde(default)]
    pub squash_msg_found: bool,
    #[serde(default)]
    pub rebase_internal_state: Option<RebaseInternalState>,
    /// The rebase stopped at an `edit` (`.git/rebase-merge/amend` exists:
    /// the commit was applied and can be amended), for Corvene's
    /// `782-commit-during-rebase-edit`.
    #[serde(default)]
    pub rebase_edit_stop: bool,
    /// The index has an entry `files` leaves out: GHD `buildStatusMap`
    /// skips a file added to the index and then deleted from the working
    /// directory. A commit must reset the index first (GHD `unstageAll`),
    /// or that file would be committed.
    #[serde(default)]
    pub hidden_index_entries: bool,
    /// A `git bisect` is in progress (Corvene `1212-bisect`).
    #[serde(default)]
    pub bisect: Option<BisectState>,
    /// Corvene `1319-trivial-change-icon`: the files (by path) whose change
    /// is trivial.
    #[serde(default)]
    pub trivial: std::collections::BTreeMap<String, TrivialChange>,
}

/// A `git bisect` in progress (Corvene `1212-bisect`; GitHub Desktop has no
/// bisect): what the git directory holds, however the bisect was started.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BisectState {
    /// `BISECT_START`: the branch (or commit) `git bisect reset` goes back to.
    pub start: String,
    /// `BISECT_TERMS`: the words for a bad and a good commit (`bad` and
    /// `good` unless the bisect was started with `--term-new` / `--term-old`).
    pub term_bad: String,
    pub term_good: String,
    /// `refs/bisect/<bad>`
    pub bad: Option<String>,
    /// `refs/bisect/<good>-*`
    pub good: Vec<String>,
    /// `refs/bisect/skip-*`
    pub skipped: Vec<String>,
    /// Once a bad and a good commit are known, every commit that may still
    /// be the first bad one, newest first, the bad commit included
    /// (`git rev-list <bad> --not <good>…` with the bisect's paths).
    pub candidates: Vec<String>,
}

/// Where a bisect stands ([`BisectState::phase`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BisectPhase {
    /// A bad or a good commit (or both) is still to be marked.
    Waiting { has_bad: bool, has_good: bool },
    /// git checked out a commit to test; `left` candidates remain.
    Testing { left: usize },
    /// The first bad commit.
    Found { sha: String },
    /// Every commit left was skipped: the first bad one is among `count`.
    OnlySkipped { count: usize },
    /// Nothing lies between the marks (the good commit is not an ancestor
    /// of the bad one).
    Empty,
}

impl BisectState {
    pub fn phase(&self) -> BisectPhase {
        let Some(bad) = self.bad.as_deref() else {
            return BisectPhase::Waiting {
                has_bad: false,
                has_good: !self.good.is_empty(),
            };
        };
        if self.good.is_empty() {
            return BisectPhase::Waiting {
                has_bad: true,
                has_good: false,
            };
        }
        match self.candidates.as_slice() {
            [] => BisectPhase::Empty,
            [only] => BisectPhase::Found { sha: only.clone() },
            all if all
                .iter()
                .all(|sha| sha == bad || self.skipped.contains(sha)) =>
            {
                BisectPhase::OnlySkipped { count: all.len() }
            }
            all => BisectPhase::Testing { left: all.len() },
        }
    }

    /// The start is a branch name, not a commit (a bisect started on a
    /// detached HEAD writes the sha).
    pub fn start_branch(&self) -> Option<&str> {
        let looks_like_sha =
            self.start.len() >= 40 && self.start.chars().all(|c| c.is_ascii_hexdigit());
        (!self.start.is_empty() && !looks_like_sha).then_some(self.start.as_str())
    }
}

impl WorkingDirectoryStatus {
    /// `includeAll` tri-state: Some(true) all, Some(false) none, None mixed.
    pub fn include_all(&self) -> Option<bool> {
        if self.files.is_empty() {
            return Some(true);
        }
        let all = self
            .files
            .iter()
            .all(|f| f.selection.kind() == DiffSelectionType::All);
        let none = self
            .files
            .iter()
            .all(|f| f.selection.kind() == DiffSelectionType::None);
        if all {
            Some(true)
        } else if none {
            Some(false)
        } else {
            None
        }
    }

    pub fn has_conflicts(&self) -> bool {
        self.files.iter().any(|f| f.status.is_conflicted())
    }

    /// GHD `updateChangedFiles` (`lib/stores/updates/changes-state.ts`):
    /// the changes list orders files by `caseInsensitiveCompare` of their
    /// paths, not in the order `git status` gives them (tracked before
    /// untracked).
    pub fn sort_files(&mut self) {
        self.files
            .sort_by_cached_key(|file| file.path.to_lowercase());
    }
}

// ---- history (`models/commit.ts`, `IChangesetData`) ----

/// `CommitIdentity`
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitIdentity {
    pub name: String,
    pub email: String,
    /// Seconds since the Unix epoch (author/committer time).
    pub seconds: i64,
    /// Time zone offset in seconds east of UTC.
    pub offset: i32,
}

impl CommitIdentity {
    pub fn date(&self) -> std::time::SystemTime {
        if self.seconds >= 0 {
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(self.seconds as u64)
        } else {
            std::time::UNIX_EPOCH
        }
    }
}

/// GHD `ITrailer` (`lib/git/interpret-trailers.ts`): a commit message
/// trailer as `(token, value)`, e.g. `("Co-Authored-By", "Name <email>")`.
pub type Trailer = (String, String);

/// GHD `isCoAuthoredByTrailer`: the token is `Co-Authored-By` (any case).
pub fn is_co_authored_by_trailer(trailer: &Trailer) -> bool {
    trailer.0.eq_ignore_ascii_case("co-authored-by")
}

/// GHD `GitAuthor` (`models/git-author.ts`): a `Name <email>` pair.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GitAuthor {
    pub name: String,
    pub email: String,
}

impl GitAuthor {
    /// GHD `GitAuthor.parse`: the regular expression `^(.*?)\s+<(.*?)>`, so
    /// the shortest name (one line) followed by whitespace, then the
    /// address up to the first `>` on its line; anything after that is
    /// ignored.
    pub fn parse(name_addr: &str) -> Option<GitAuthor> {
        // JavaScript's `.` stops at these line terminators
        let line_end = |c: char| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}');
        for (name_end, c) in name_addr
            .char_indices()
            .chain(std::iter::once((name_addr.len(), '\n')))
        {
            let rest = &name_addr[name_end..];
            let after_space = rest.trim_start_matches(char::is_whitespace);
            if after_space.len() < rest.len()
                && let Some(addr) = after_space.strip_prefix('<')
                && let Some(close) = addr.find(|c: char| c == '>' || line_end(c))
                && addr[close..].starts_with('>')
            {
                return Some(GitAuthor {
                    name: name_addr[..name_end].to_string(),
                    email: addr[..close].to_string(),
                });
            }
            if line_end(c) {
                break;
            }
        }
        None
    }
}

/// `Commit` (trimmed to what the history views need).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commit {
    pub sha: String,
    pub summary: String,
    /// git's `%b`: the message after the summary paragraph and the blank
    /// lines that follow it, verbatim (trailers and the final newline
    /// included).
    pub body: String,
    pub author: CommitIdentity,
    pub committer: CommitIdentity,
    pub parents: Vec<String>,
    /// The message's trailers, unfolded (git's `%(trailers:unfold,only)`).
    #[serde(default)]
    pub trailers: Vec<Trailer>,
    pub tags: Vec<String>,
    /// The `gpgsig` header's armour, `None` when unsigned
    /// (`1214-commit-signatures`).
    #[serde(default)]
    pub signature: Option<SignatureKind>,
}

impl Commit {
    pub fn short_sha(&self) -> &str {
        &self.sha[..self.sha.len().min(7)]
    }

    pub fn is_merge(&self) -> bool {
        self.parents.len() > 1
    }

    /// GHD `Commit.coAuthors` (`extractCoAuthors`): the `Co-Authored-By`
    /// trailers that parse as `Name <email>`.
    pub fn co_authors(&self) -> Vec<GitAuthor> {
        self.trailers
            .iter()
            .filter(|t| is_co_authored_by_trailer(t))
            .filter_map(|(_, value)| GitAuthor::parse(value))
            .collect()
    }

    /// GHD `Commit.bodyNoCoAuthors` (`trimCoAuthorsTrailers`): the body with
    /// the first `token: value` text of each `Co-Authored-By` trailer
    /// removed (its line break stays).
    pub fn body_no_co_authors(&self) -> String {
        let mut body = self.body.clone();
        for (token, value) in self
            .trailers
            .iter()
            .filter(|t| is_co_authored_by_trailer(t))
        {
            body = body.replacen(&format!("{token}: {value}"), "", 1);
        }
        body
    }
}

/// Corvene `731-recall-commit-messages`: up to `limit` (summary, description)
/// pairs of `commits` (newest first) for recalling into the commit form.
/// Merge commits and repeated summaries are skipped; `Co-authored-by`
/// trailers are dropped from the description.
pub fn recent_commit_messages(commits: &[Commit], limit: usize) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for c in commits.iter().filter(|c| !c.is_merge()) {
        if out.len() >= limit {
            break;
        }
        if c.summary.trim().is_empty() || out.iter().any(|(s, _)| *s == c.summary) {
            continue;
        }
        let body = c
            .body
            .lines()
            .filter(|l| !l.to_ascii_lowercase().starts_with("co-authored-by:"))
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string();
        out.push((c.summary.clone(), body));
    }
    out
}

/// Corvene `1319-trivial-change-icon` (desktop/desktop#17573): a modified
/// file whose change does not touch what it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrivialChange {
    /// Only the file mode changed (`chmod +x`), the contents are the same.
    ModeOnly,
    /// A `.patch` / `.diff` file whose only changed lines are patch
    /// metadata: `index` lines, hunk header line numbers, the `From <sha>`
    /// line and the git version under the signature.
    PatchMetadata,
}

/// `CommittedFileChange`
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommittedFileChange {
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub commitish: String,
}

/// `IChangesetData`: files plus line counts for one commit.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangesetData {
    pub files: Vec<CommittedFileChange>,
    pub lines_added: u64,
    pub lines_deleted: u64,
    /// Corvene `1319-trivial-change-icon`: the files (by path) whose change
    /// is trivial.
    #[serde(default)]
    pub trivial: std::collections::BTreeMap<String, TrivialChange>,
    /// Whether `trivial` was read (the flag was on when it was loaded).
    #[serde(default)]
    pub trivial_read: bool,
}

impl CommittedFileChange {
    pub fn file_name(&self) -> &str {
        file_name_and_directory(&self.path).0
    }

    pub fn directory(&self) -> &str {
        file_name_and_directory(&self.path).1
    }
}

// ---- diffs (`models/diff/*`) ----

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffLineKind {
    Context,
    Add,
    Delete,
    Hunk,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    /// Line text without the leading marker.
    pub text: String,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    /// `\ No newline at end of file` followed this line.
    pub no_trailing_newline: bool,
    /// The line's bytes (without the marker) when they are not UTF-8;
    /// `text` is then a rendition of them. Partial patches write these
    /// (Corvene `789-non-utf8-diffs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffHunk {
    /// Absolute index of this hunk's header line in the unified diff.
    #[serde(default)]
    pub unified_diff_start: u32,
    pub header: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

/// GHD `UnknownAuthor.state`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnknownAuthorState {
    Searching,
    Error,
}

/// GHD `Author` (`models/author.ts`): a co-author handle in the commit form.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Author {
    Known {
        name: String,
        email: String,
        username: Option<String>,
    },
    Unknown {
        username: String,
        state: UnknownAuthorState,
    },
}

impl Author {
    pub fn username(&self) -> Option<&str> {
        match self {
            Author::Known { username, .. } => username.as_deref(),
            Author::Unknown { username, .. } => Some(username),
        }
    }

    /// GHD `getDisplayTextForAuthor`: `@login`, or the name without a login.
    pub fn display_text(&self) -> String {
        match self {
            Author::Known {
                name,
                username: None,
                ..
            } => name.clone(),
            Author::Known {
                username: Some(u), ..
            }
            | Author::Unknown { username: u, .. } => format!("@{u}"),
        }
    }

    /// GHD `getFullTextForAuthor`: `@login (Name)`.
    pub fn full_text(&self) -> String {
        match self {
            Author::Known {
                name,
                username: Some(u),
                ..
            } => format!("@{u} ({name})"),
            _ => self.display_text(),
        }
    }

    /// The `Co-Authored-By` trailer value of a known author.
    pub fn trailer_value(&self) -> Option<String> {
        match self {
            Author::Known { name, email, .. } => Some(format!("{name} <{email}>")),
            Author::Unknown { .. } => None,
        }
    }
}

/// GHD `getLegacyStealthEmailForUser`: the no-reply address of a login.
pub fn legacy_stealth_email(login: &str, endpoint: &str) -> String {
    let host = if endpoint == "https://api.github.com" {
        "github.com".to_string()
    } else {
        endpoint
            .trim_start_matches("https://")
            .split('/')
            .next()
            .unwrap_or("github.com")
            .to_string()
    };
    format!("{login}@users.noreply.{host}")
}

/// GHD `getStealthEmailForUser`: `<id>+<login>@users.noreply.<host>`.
pub fn stealth_email(id: u64, login: &str, endpoint: &str) -> String {
    format!("{id}+{}", legacy_stealth_email(login, endpoint))
}

/// GHD `parseStealthEmail`: the login of a no-reply address of `endpoint`'s
/// host, either form.
pub fn stealth_email_login(email: &str, endpoint: &str) -> Option<String> {
    let suffix = legacy_stealth_email("", endpoint).to_ascii_lowercase();
    let lower = email.trim().to_ascii_lowercase();
    let local = lower.strip_suffix(&suffix)?;
    let login = match local.split_once('+') {
        Some((id, login)) if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) => login,
        Some(_) => return None,
        None => local,
    };
    let bare = login.strip_suffix("[bot]").unwrap_or(login);
    // ASCII lower-casing keeps byte offsets: the login as written
    let start = local.len() - login.len();
    (!bare.is_empty() && bare.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        .then(|| email.trim()[start..local.len()].to_string())
}

/// GHD `WorktreeType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorktreeType {
    Main,
    Linked,
}

/// GHD `WorktreeEntry` (`git worktree list --porcelain`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub head: String,
    /// Full ref (`refs/heads/main`) when checked out on a branch.
    pub branch: Option<String>,
    pub is_detached: bool,
    pub kind: WorktreeType,
    pub is_locked: bool,
    pub is_prunable: bool,
}

impl WorktreeEntry {
    /// GHD `getWorktreeDisplayName`: the folder name.
    pub fn display_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    /// GHD `getWorktreeDescription`: the branch, else the short SHA.
    pub fn description(&self) -> String {
        match &self.branch {
            Some(b) => b.strip_prefix("refs/heads/").unwrap_or(b).to_string(),
            None => self.head.chars().take(7).collect(),
        }
    }
}

/// GHD `SubmoduleStatus`: what changed inside a submodule.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmoduleStatus {
    pub commit_changed: bool,
    pub modified_changes: bool,
    pub untracked_changes: bool,
}

/// GHD `Image`: the bytes of one side of an image diff.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageBlob {
    pub bytes: Vec<u8>,
    /// `image/png`, `image/jpg`, …
    pub media_type: String,
}

/// GHD `ISubmoduleDiff`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmoduleDiff {
    pub path: String,
    pub full_path: PathBuf,
    /// `submodule.<path>.url` from the repository config.
    pub url: Option<String>,
    pub old_sha: Option<String>,
    pub new_sha: Option<String>,
    pub status: SubmoduleStatus,
}

/// GHD `ImageDiffType` (the tabs of a modified-image diff).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ImageDiffType {
    #[default]
    TwoUp,
    Swipe,
    OnionSkin,
    Difference,
}

/// GHD `imageFileExtensions` + `getMediaType`: `Some` when a binary file
/// can be shown as an image.
pub fn image_media_type(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "avif" => "image/avif",
        // Corvene `755-tga-image-diff` (the UI shows it as binary when off)
        "tga" => "image/x-tga",
        _ => return None,
    })
}

/// GHD `LineEndingsChange`: git's "CRLF will be replaced by LF" warning.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineEndingsChange {
    pub from: String,
    pub to: String,
}

/// Warnings shown above a text diff (`DiffContentsWarning`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffWarnings {
    /// GHD `hasHiddenBidiChars`: U+202A–U+202E / U+2066–U+2069 in the text.
    pub hidden_bidi: bool,
    pub line_endings: Option<LineEndingsChange>,
    /// The `old mode` / `new mode` of the patch header (e.g. `100644`,
    /// `100755`) when the file's mode changed. A mode-only change is a
    /// `Text` diff without hunks that carries this.
    #[serde(default)]
    pub mode_change: Option<(String, String)>,
    /// A type change (`T` in git's status, e.g. a file replaced by a symbolic
    /// link): git prints a deletion and an addition as two `diff --git`
    /// sections. Holds the old and new modes (`100644`, `120000`, ...).
    #[serde(default)]
    pub type_change: Option<(String, String)>,
    /// Corvene `795-lfs-image-previews`: an image stored in Git LFS whose
    /// contents are not downloaded, so its pointer files are diffed.
    #[serde(default)]
    pub lfs_not_downloaded: bool,
    /// Corvene `796-lfs-text-diff`: the diff compares the contents of a
    /// file stored in Git LFS, not the pointer files the index holds, so
    /// its lines cannot be selected.
    #[serde(default)]
    pub lfs_contents: bool,
    /// Corvene `1306-utf16-diffs`: the file is saved as UTF-16, which git
    /// takes for binary; the diff compares its decoded text.
    #[serde(default)]
    pub utf16: Option<Utf16Diff>,
    /// Corvene `1322-pdf-diff`: the diff compares the text extracted from
    /// the pages of a PDF file, so its lines cannot be selected.
    #[serde(default)]
    pub pdf_text: bool,
}

/// Corvene `1306-utf16-diffs`: how a UTF-16 text file is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Utf16Format {
    /// UTF-16BE (else UTF-16LE).
    pub big_endian: bool,
    /// The file starts with a byte order mark.
    pub bom: bool,
}

/// Corvene `1306-utf16-diffs`: a text diff of a UTF-16 file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Utf16Diff {
    pub format: Utf16Format,
    /// The blob the old side was read from (`HEAD:<path>`, `:<path>` for
    /// the index), so a line selection can be written back; `None` for a
    /// new file.
    pub old_source: Option<String>,
}

/// GHD `IDiff` (`DiffType`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Diff {
    Text {
        hunks: Vec<DiffHunk>,
        #[serde(default)]
        warnings: DiffWarnings,
    },
    /// `LargeText`: shown only after "Show Diff" (performance).
    LargeText {
        hunks: Vec<DiffHunk>,
        #[serde(default)]
        warnings: DiffWarnings,
    },
    Binary,
    /// `Image`: `previous` is missing for new files, `current` for deleted ones.
    Image {
        previous: Option<ImageBlob>,
        current: Option<ImageBlob>,
    },
    /// Nothing to show (e.g. empty file, mode-only change).
    Empty,
    /// `Unrenderable`: beyond what git would even hand over.
    TooLarge,
    Submodule(SubmoduleDiff),
}

impl Diff {
    pub fn line_count(&self) -> usize {
        match self {
            Diff::Text { hunks, .. } | Diff::LargeText { hunks, .. } => {
                hunks.iter().map(|h| h.lines.len() + 1).sum()
            }
            _ => 0,
        }
    }

    /// The hunks of a text diff (large ones included).
    pub fn hunks(&self) -> Option<&[DiffHunk]> {
        match self {
            Diff::Text { hunks, .. } | Diff::LargeText { hunks, .. } => Some(hunks),
            _ => None,
        }
    }

    /// The warnings of a text diff.
    pub fn warnings(&self) -> Option<&DiffWarnings> {
        match self {
            Diff::Text { warnings, .. } | Diff::LargeText { warnings, .. } => Some(warnings),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Pull requests (`models/pull-request.ts`)
// ---------------------------------------------------------------------------

/// `PullRequestRef`: a ref in a GitHub repository. `repository` is `None`
/// when the head repository was deleted after the pull request was opened.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestRef {
    pub ref_name: String,
    pub sha: String,
    pub repository: Option<GitHubRepository>,
}

/// `PullRequest`
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    /// ISO-8601 timestamps as the API sends them (`2024-01-31T12:00:00Z`).
    pub created_at: String,
    pub updated_at: String,
    pub head: PullRequestRef,
    pub base: PullRequestRef,
    /// The author's login.
    pub author: String,
    pub draft: bool,
    pub body: String,
    /// The assignees' logins (Corvene, `335-pull-request-list-filters`).
    #[serde(default)]
    pub assignees: Vec<String>,
    /// The users asked for a review (logins; team requests left out).
    #[serde(default)]
    pub requested_reviewers: Vec<String>,
}

impl PullRequest {
    /// `getPullRequestCommitRef`: the ref GitHub exposes for the PR head.
    pub fn commit_ref(&self) -> String {
        format!("refs/pull/{}/head", self.number)
    }

    /// `<base html url>/pull/<number>`
    pub fn html_url(&self) -> Option<String> {
        self.base
            .repository
            .as_ref()
            .map(|r| format!("{}/pull/{}", r.html_url, self.number))
    }
}

/// Parse an ISO-8601 UTC timestamp (`2024-01-31T12:00:00Z`, optional
/// fractional seconds) into a `SystemTime`.
pub fn parse_iso8601(value: &str) -> Option<std::time::SystemTime> {
    let value = value.trim().trim_end_matches('Z');
    let (date, time) = value.split_once('T')?;
    let mut d = date.split('-');
    let year: i64 = d.next()?.parse().ok()?;
    let month: u32 = d.next()?.parse().ok()?;
    let day: u32 = d.next()?.parse().ok()?;
    // a `+02:00` / `-0500` offset (Forgejo, Bitbucket) is taken out; GitHub
    // always sends UTC
    let (time, offset_secs) = match time.find(['+', '-']) {
        Some(ix) => {
            let (clock, offset) = time.split_at(ix);
            let sign = if offset.starts_with('-') { -1 } else { 1 };
            let digits: String = offset[1..].chars().filter(char::is_ascii_digit).collect();
            let hours: i64 = digits.get(..2).and_then(|h| h.parse().ok()).unwrap_or(0);
            let minutes: i64 = digits.get(2..4).and_then(|m| m.parse().ok()).unwrap_or(0);
            (clock, sign * (hours * 3600 + minutes * 60))
        }
        None => (time, 0),
    };
    let mut t = time.split(':');
    let hour: u64 = t.next()?.parse().ok()?;
    let minute: u64 = t.next()?.parse().ok()?;
    let second: u64 = t
        .next()
        .map(|s| s.split('.').next().unwrap_or("0"))
        .unwrap_or("0")
        .parse()
        .ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Howard Hinnant's days-from-civil.
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = (month as u64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe as i64 - 719_468;
    let secs = days * 86_400 + (hour * 3600 + minute * 60 + second) as i64 - offset_secs;
    if secs < 0 {
        return None;
    }
    Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs as u64))
}

/// `urlMatchesRemote` / `repositoryMatchesRemote`: same host and
/// `owner/name`, ignoring scheme, credentials, case and a `.git` suffix.
pub fn url_matches_remote(a: &str, b: &str) -> bool {
    fn key(url: &str) -> Option<(String, String)> {
        let (host, path) = split_remote(url)?;
        let path = path.trim_matches('/');
        let path = path.strip_suffix(".git").unwrap_or(path);
        Some((host.to_lowercase(), path.to_lowercase()))
    }
    match (key(a), key(b)) {
        (Some(a), Some(b)) => a == b,
        // Corvene: two local paths (a bare repository beside the working
        // copy, as the parity harness's fixture has) match when they are
        // the same path; GHD's `urlMatchesRemote` needs a host in both.
        (None, None) => {
            let local = |s: &str| {
                let s = s.trim().trim_end_matches('/');
                s.strip_suffix(".git").unwrap_or(s).to_string()
            };
            let (a, b) = (local(a), local(b));
            !a.is_empty() && a == b
        }
        _ => false,
    }
}

/// `clone_url` (an API `clone_url`, HTTPS) over `like`'s protocol: when
/// `like` is an SSH remote (`user@host:path` or `ssh://user@host/path`), the
/// same `owner/name` path on `like`'s SSH user and host (an SSH host alias
/// stays); otherwise `clone_url` unchanged. Not in GHD, which always adds
/// HTTPS remotes (desktop/desktop#19074, #9490).
pub fn clone_url_like_remote(clone_url: &str, like: &str) -> String {
    let Some((_, path)) = split_remote(clone_url) else {
        return clone_url.to_string();
    };
    let path = path.trim_start_matches('/');
    let like = like.trim();
    if let Some(rest) = like.strip_prefix("ssh://") {
        if let Some((authority, _)) = rest.split_once('/') {
            return format!("ssh://{authority}/{path}");
        }
    } else if !like.contains("://")
        && let Some((prefix, _)) = like.split_once(':')
        && prefix.contains('@')
        && !prefix.contains('/')
    {
        return format!("{prefix}:{path}");
    }
    clone_url.to_string()
}

// ---------------------------------------------------------------------------
// Forks (`models/workflow-preferences.ts`)
// ---------------------------------------------------------------------------

/// `ForkContributionTarget`: what the user contributes to with a fork.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForkContributionTarget {
    #[default]
    Parent,
    #[serde(rename = "self")]
    Own,
}

impl Repository {
    /// `getForkContributionTarget`
    pub fn fork_contribution_target(&self) -> ForkContributionTarget {
        self.fork_contribution_target.unwrap_or_default()
    }

    /// `isForkedRepositoryContributingToParent`
    pub fn is_fork_contributing_to_parent(&self) -> bool {
        self.github.as_ref().is_some_and(|gh| gh.parent.is_some())
            && self.fork_contribution_target() == ForkContributionTarget::Parent
    }

    /// `getNonForkGitHubRepository`: the parent when this fork contributes
    /// to it, else the repository itself.
    pub fn non_fork_github(&self) -> Option<&GitHubRepository> {
        let gh = self.github.as_ref()?;
        match (&gh.parent, self.fork_contribution_target()) {
            (Some(parent), ForkContributionTarget::Parent) => Some(parent),
            _ => Some(gh),
        }
    }
}

// ---------------------------------------------------------------------------
// CI checks (`lib/ci-checks/ci-checks.ts`, `lib/api.ts`)
// ---------------------------------------------------------------------------

/// `APICheckStatus`
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Queued,
    InProgress,
    Completed,
    /// Anything else the API sends (`waiting`, `requested`, `pending`).
    #[serde(other)]
    Pending,
}

/// `APICheckConclusion`
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckConclusion {
    ActionRequired,
    Cancelled,
    TimedOut,
    Failure,
    Neutral,
    Success,
    Skipped,
    Stale,
    #[serde(other)]
    Unknown,
}

impl CheckConclusion {
    /// `getCheckRunConclusionAdjective`
    pub fn adjective(conclusion: Option<CheckConclusion>) -> &'static str {
        match conclusion {
            None => "In progress",
            Some(CheckConclusion::ActionRequired) => "Action required",
            Some(CheckConclusion::Cancelled) => "Canceled",
            Some(CheckConclusion::TimedOut) => "Timed out",
            Some(CheckConclusion::Failure) => "Failed",
            Some(CheckConclusion::Neutral) | Some(CheckConclusion::Unknown) => "Neutral",
            Some(CheckConclusion::Success) => "Successful",
            Some(CheckConclusion::Skipped) => "Skipped",
            Some(CheckConclusion::Stale) => "Marked as stale",
        }
    }

    /// `FailingCheckConclusions`
    pub fn is_failing(self) -> bool {
        matches!(
            self,
            CheckConclusion::Failure
                | CheckConclusion::Cancelled
                | CheckConclusion::ActionRequired
                | CheckConclusion::TimedOut
        )
    }
}

/// `IAPIWorkflowRun` (the fields the check-run list needs).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowRun {
    pub id: u64,
    pub workflow_id: u64,
    pub name: String,
    #[serde(default)]
    pub event: String,
    pub check_suite_id: Option<u64>,
    pub created_at: String,
}

/// `IAPIWorkflowJobStep`
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobStep {
    pub name: String,
    pub number: u64,
    pub status: CheckStatus,
    pub conclusion: Option<CheckConclusion>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

/// `IRefCheck`: one status or check run of a ref.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefCheck {
    pub id: u64,
    pub name: String,
    pub description: String,
    pub status: CheckStatus,
    pub conclusion: Option<CheckConclusion>,
    pub app_name: String,
    pub html_url: Option<String>,
    pub check_suite_id: Option<u64>,
    pub actions_workflow: Option<WorkflowRun>,
    pub job_steps: Option<Vec<JobStep>>,
}

/// `ICombinedRefCheck`
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CombinedRefCheck {
    pub status: CheckStatus,
    pub conclusion: Option<CheckConclusion>,
    pub checks: Vec<RefCheck>,
}

impl RefCheck {
    /// `isIncomplete`
    pub fn is_incomplete(&self) -> bool {
        self.status == CheckStatus::Completed
            && matches!(
                self.conclusion,
                Some(CheckConclusion::TimedOut)
                    | Some(CheckConclusion::Stale)
                    | Some(CheckConclusion::Cancelled)
            )
    }

    /// `isFailure`
    pub fn is_failure(&self) -> bool {
        self.status == CheckStatus::Completed
            && matches!(
                self.conclusion,
                Some(CheckConclusion::Failure) | Some(CheckConclusion::ActionRequired)
            )
    }

    /// `isSuccess`
    pub fn is_success(&self) -> bool {
        self.status == CheckStatus::Completed
            && matches!(
                self.conclusion,
                Some(CheckConclusion::Success)
                    | Some(CheckConclusion::Neutral)
                    | Some(CheckConclusion::Skipped)
            )
    }
}

impl CombinedRefCheck {
    /// `createCombinedCheckFromChecks`
    pub fn from_checks(checks: Vec<RefCheck>) -> Option<Self> {
        if checks.is_empty() {
            return None;
        }
        if checks.len() == 1 {
            let (status, conclusion) = (checks[0].status, checks[0].conclusion);
            return Some(Self {
                status,
                conclusion,
                checks,
            });
        }
        if checks.iter().any(|c| c.is_incomplete() || c.is_failure()) {
            Some(Self {
                status: CheckStatus::Completed,
                conclusion: Some(CheckConclusion::Failure),
                checks,
            })
        } else if checks.iter().all(RefCheck::is_success) {
            Some(Self {
                status: CheckStatus::Completed,
                conclusion: Some(CheckConclusion::Success),
                checks,
            })
        } else {
            Some(Self {
                status: CheckStatus::InProgress,
                conclusion: None,
                checks,
            })
        }
    }
}

/// `formatPreciseDuration` (`lib/format-duration.ts`): `1h 2m 3s` of the
/// absolute value of `ms`. Every unit after the first whole one is shown,
/// zeros included (`1h 0m 0s`), and seconds always are (`0s`).
pub fn format_precise_duration(ms: i64) -> String {
    const UNITS: [(char, u64); 4] = [
        ('d', 86_400_000),
        ('h', 3_600_000),
        ('m', 60_000),
        ('s', 1000),
    ];
    let mut ms = ms.unsigned_abs();
    let mut parts: Vec<String> = Vec::new();
    for (unit, unit_ms) in UNITS {
        if !parts.is_empty() || ms >= unit_ms || unit == 's' {
            let qty = ms / unit_ms;
            ms -= qty * unit_ms;
            parts.push(format!("{qty}{unit}"));
        }
    }
    parts.join(" ")
}

/// `getCheckDurationInMilliseconds`: `completed_at` minus `started_at`,
/// negative when the check run completed before it started; `None` when
/// either is missing or unparsable (GHD `NaN`).
pub fn check_duration_ms(started_at: Option<&str>, completed_at: Option<&str>) -> Option<i64> {
    let millis = |t: std::time::SystemTime| {
        t.duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|d| i64::try_from(d.as_millis()).ok())
    };
    let start = millis(parse_iso8601(started_at?)?)?;
    let end = millis(parse_iso8601(completed_at?)?)?;
    Some(end - start)
}

/// `getCheckRunShortDescription`
pub fn check_short_description(
    status: CheckStatus,
    conclusion: Option<CheckConclusion>,
    duration_ms: Option<i64>,
) -> String {
    let Some(conclusion) = conclusion.filter(|_| status == CheckStatus::Completed) else {
        return "In progress".to_string();
    };
    let adjective = CheckConclusion::adjective(Some(conclusion));
    if matches!(
        conclusion,
        CheckConclusion::ActionRequired | CheckConclusion::Skipped | CheckConclusion::Stale
    ) {
        return adjective.to_string();
    }
    let preposition = if conclusion == CheckConclusion::Success {
        "in"
    } else {
        "after"
    };
    match duration_ms {
        Some(ms) if ms > 0 => format!("{adjective} {preposition} {}", format_precise_duration(ms)),
        _ => adjective.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Repository rules (`models/repo-rules.ts`)
// ---------------------------------------------------------------------------

/// `RepoRuleEnforced`: `false` | `true` | `'bypass'`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepoRuleEnforced {
    #[default]
    No,
    Yes,
    /// The rule applies but the current user may bypass it.
    Bypass,
}

impl RepoRuleEnforced {
    /// `info.x !== true ? enforced : true`: once enforced, stays enforced.
    pub fn combine(self, other: RepoRuleEnforced) -> RepoRuleEnforced {
        if self == RepoRuleEnforced::Yes {
            self
        } else {
            other
        }
    }
}

/// `APIRepoRuleMetadataOperator`
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleOperator {
    StartsWith,
    EndsWith,
    Contains,
    #[serde(rename = "regex")]
    RegexMatch,
}

/// `IRepoRulesMetadataRule` (the matcher is built from the pattern in core).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoRulesMetadataRule {
    pub enforced: RepoRuleEnforced,
    pub ruleset_id: u64,
    pub operator: RuleOperator,
    pub pattern: String,
    pub negate: bool,
}

impl RepoRulesMetadataRule {
    /// `toHumanDescription`: `must not start with "foo"`.
    pub fn human_description(&self) -> String {
        let mut description = String::from("must ");
        if self.negate {
            description.push_str("not ");
        }
        match self.operator {
            RuleOperator::RegexMatch => {
                description.push_str(&format!(
                    "match the regular expression \"{}\"",
                    self.pattern
                ));
            }
            RuleOperator::StartsWith => {
                description.push_str(&format!("start with \"{}\"", self.pattern))
            }
            RuleOperator::EndsWith => {
                description.push_str(&format!("end with \"{}\"", self.pattern))
            }
            RuleOperator::Contains => {
                description.push_str(&format!("contain \"{}\"", self.pattern))
            }
        }
        description
    }
}

/// `RepoRulesMetadataFailure`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoRulesMetadataFailure {
    pub description: String,
    pub ruleset_id: u64,
}

/// `RepoRulesMetadataStatus`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepoRulesMetadataStatus {
    Pass,
    Fail,
    Bypass,
}

/// `RepoRulesMetadataFailures`
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepoRulesMetadataFailures {
    pub failed: Vec<RepoRulesMetadataFailure>,
    pub bypassed: Vec<RepoRulesMetadataFailure>,
}

impl RepoRulesMetadataFailures {
    pub fn status(&self) -> RepoRulesMetadataStatus {
        if !self.failed.is_empty() {
            RepoRulesMetadataStatus::Fail
        } else if !self.bypassed.is_empty() {
            RepoRulesMetadataStatus::Bypass
        } else {
            RepoRulesMetadataStatus::Pass
        }
    }

    pub fn total(&self) -> usize {
        self.failed.len() + self.bypassed.len()
    }
}

/// `RepoRulesInfo`: what the rulesets of the current branch enforce.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepoRulesInfo {
    /// `update` / `required_deployments` / `required_status_checks`.
    pub basic_commit_warning: RepoRuleEnforced,
    pub creation_restricted: RepoRuleEnforced,
    pub signed_commits_required: RepoRuleEnforced,
    pub pull_request_required: RepoRuleEnforced,
    pub commit_message_patterns: Vec<RepoRulesMetadataRule>,
    pub commit_author_email_patterns: Vec<RepoRulesMetadataRule>,
    pub committer_email_patterns: Vec<RepoRulesMetadataRule>,
    pub branch_name_patterns: Vec<RepoRulesMetadataRule>,
}

// ---------------------------------------------------------------------------
// Secret scanning push protection (`ui/secret-scanning/`)
// ---------------------------------------------------------------------------

/// `ISecretLocation`
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretLocation {
    pub commit_sha: String,
    pub path: String,
    pub line_number: u64,
}

/// `ISecretScanResult`: one secret the server refused to accept.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretScanResult {
    /// The placeholder id at the end of the bypass URL.
    pub id: String,
    pub description: String,
    pub locations: Vec<SecretLocation>,
    pub bypass_url: String,
    /// Bypassing needs an admin's approval ("request an exemption").
    pub requires_approval: bool,
}

/// `BypassReason`
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BypassReason {
    FalsePositive,
    UsedInTests,
    WillFixLater,
}

impl BypassReason {
    pub fn as_str(self) -> &'static str {
        match self {
            BypassReason::FalsePositive => "false_positive",
            BypassReason::UsedInTests => "used_in_tests",
            BypassReason::WillFixLater => "will_fix_later",
        }
    }
}

#[cfg(test)]
mod github_layer_tests {
    use super::*;

    #[test]
    fn recent_commit_messages_skip_merges_and_repeats() {
        let id = CommitIdentity {
            name: String::new(),
            email: String::new(),
            seconds: 0,
            offset: 0,
        };
        let commit = |summary: &str, body: &str, parents: usize| Commit {
            sha: String::new(),
            summary: summary.to_string(),
            body: body.to_string(),
            author: id.clone(),
            committer: id.clone(),
            parents: vec![String::new(); parents],
            trailers: Vec::new(),
            tags: Vec::new(),
            signature: None,
        };
        let commits = [
            commit("Fix a", "Details\n\nCo-authored-by: X <x@y.z>", 1),
            commit("Merge b", "", 2),
            commit("Fix a", "older", 1),
            commit("Add c", "", 1),
            commit("Add d", "", 0),
        ];
        let got = recent_commit_messages(&commits, 2);
        assert_eq!(
            got,
            [
                ("Fix a".to_string(), "Details".to_string()),
                ("Add c".to_string(), String::new())
            ]
        );
        assert_eq!(recent_commit_messages(&commits, 10).len(), 3);
    }

    #[test]
    fn parses_iso_timestamps() {
        let t = parse_iso8601("1970-01-02T00:00:00Z").unwrap();
        assert_eq!(
            t.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
            86_400
        );
        let t = parse_iso8601("2024-03-01T12:30:15.5Z").unwrap();
        assert_eq!(
            t.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
            1_709_296_215
        );
        assert!(parse_iso8601("nope").is_none());
    }

    #[test]
    fn matches_remote_urls() {
        assert!(url_matches_remote(
            "https://github.com/Octocat/Hello-World.git",
            "git@github.com:octocat/hello-world"
        ));
        assert!(!url_matches_remote(
            "https://github.com/octocat/hello-world",
            "https://github.com/octocat/other"
        ));
        // two local paths: the same bare repository
        assert!(url_matches_remote("/tmp/fixture.git", "/tmp/fixture.git/"));
        assert!(!url_matches_remote("/tmp/fixture.git", "/tmp/other.git"));
        assert!(!url_matches_remote("", ""));
        assert!(!url_matches_remote(
            "/tmp/fixture.git",
            "https://github.com/octocat/hello-world"
        ));
    }

    #[test]
    fn clone_url_follows_the_remote_protocol() {
        let url = "https://ghe.corp/fork/app.git";
        assert_eq!(
            clone_url_like_remote(url, "git@ghe.corp:team/app.git"),
            "git@ghe.corp:fork/app.git"
        );
        assert_eq!(
            clone_url_like_remote(url, "work@gh-alias:team/app"),
            "work@gh-alias:fork/app.git"
        );
        assert_eq!(
            clone_url_like_remote(url, "ssh://git@ghe.corp:2222/team/app.git"),
            "ssh://git@ghe.corp:2222/fork/app.git"
        );
        assert_eq!(
            clone_url_like_remote(url, "https://ghe.corp/team/app.git"),
            url
        );
        assert_eq!(clone_url_like_remote(url, "/local/path"), url);
    }

    #[test]
    fn combines_checks() {
        let check = |conclusion| RefCheck {
            id: 1,
            name: "ci".into(),
            description: String::new(),
            status: CheckStatus::Completed,
            conclusion,
            app_name: String::new(),
            html_url: None,
            check_suite_id: None,
            actions_workflow: None,
            job_steps: None,
        };
        let combined = CombinedRefCheck::from_checks(vec![
            check(Some(CheckConclusion::Success)),
            check(Some(CheckConclusion::Skipped)),
        ])
        .unwrap();
        assert_eq!(combined.conclusion, Some(CheckConclusion::Success));
        let combined = CombinedRefCheck::from_checks(vec![
            check(Some(CheckConclusion::Success)),
            check(Some(CheckConclusion::Cancelled)),
        ])
        .unwrap();
        assert_eq!(combined.conclusion, Some(CheckConclusion::Failure));
        assert!(CombinedRefCheck::from_checks(vec![]).is_none());
        assert_eq!(
            check_short_description(
                CheckStatus::Completed,
                Some(CheckConclusion::Failure),
                Some(65_000)
            ),
            "Failed after 1m 5s"
        );
    }

    #[test]
    fn describes_rules() {
        let rule = RepoRulesMetadataRule {
            enforced: RepoRuleEnforced::Yes,
            ruleset_id: 1,
            operator: RuleOperator::StartsWith,
            pattern: "feat".into(),
            negate: true,
        };
        assert_eq!(rule.human_description(), "must not start with \"feat\"");
    }
}

#[cfg(test)]
mod commit_and_status_tests {
    use super::*;

    #[test]
    fn git_author_parse_follows_ghd_regex() {
        let author = |name: &str, email: &str| GitAuthor {
            name: name.to_string(),
            email: email.to_string(),
        };
        assert_eq!(
            GitAuthor::parse("Jane Q. Doe <jane@example.com>"),
            Some(author("Jane Q. Doe", "jane@example.com"))
        );
        // shortest name, address up to the first `>`, the rest ignored
        assert_eq!(GitAuthor::parse("a <b> <c>"), Some(author("a", "b")));
        assert_eq!(GitAuthor::parse(" <x@y>"), Some(author("", "x@y")));
        assert_eq!(GitAuthor::parse("Jane<jane@example.com>"), None);
        assert_eq!(GitAuthor::parse("Jane <jane@example.com"), None);
        assert_eq!(GitAuthor::parse("Ja\nne <j@x>"), None);
    }

    #[test]
    fn co_authors_and_body_without_them() {
        let id = CommitIdentity {
            name: String::new(),
            email: String::new(),
            seconds: 0,
            offset: 0,
        };
        let commit = Commit {
            sha: String::new(),
            summary: "s".into(),
            body: "desc\n\nCo-authored-by: A <a@x>\nSigned-off-by: B <b@x>\n".into(),
            author: id.clone(),
            committer: id,
            parents: Vec::new(),
            trailers: vec![
                ("Co-authored-by".into(), "A <a@x>".into()),
                ("Signed-off-by".into(), "B <b@x>".into()),
            ],
            tags: Vec::new(),
            signature: None,
        };
        assert_eq!(
            commit.co_authors(),
            [GitAuthor {
                name: "A".into(),
                email: "a@x".into()
            }]
        );
        assert_eq!(
            commit.body_no_co_authors(),
            "desc\n\n\nSigned-off-by: B <b@x>\n"
        );
    }

    #[test]
    fn rename_includes_modifications_per_ghd() {
        let status = |kind, working_tree, score| FileStatus {
            kind,
            index: GitStatusEntry::Renamed,
            working_tree,
            score,
            code: String::new(),
            submodule: false,
            submodule_status: None,
            conflict_markers: None,
        };
        use FileStatusKind::{Copied, Renamed};
        use GitStatusEntry::{Modified, Unchanged};
        assert!(!status(Renamed, Unchanged, Some(100)).rename_includes_modifications());
        assert!(!status(Renamed, Unchanged, None).rename_includes_modifications());
        assert!(status(Renamed, Unchanged, Some(85)).rename_includes_modifications());
        assert!(status(Renamed, Modified, Some(100)).rename_includes_modifications());
        assert!(!status(Copied, Modified, Some(50)).rename_includes_modifications());
    }

    #[test]
    fn file_name_and_directory_drop_a_trailing_slash() {
        assert_eq!(file_name_and_directory("a/b/c.txt"), ("c.txt", "a/b/"));
        assert_eq!(file_name_and_directory("c.txt"), ("c.txt", ""));
        assert_eq!(
            file_name_and_directory("some/submodule/path/"),
            ("path", "some/submodule/")
        );
    }
}
