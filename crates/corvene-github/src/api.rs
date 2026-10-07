//! Minimal REST client: the calls core parity needs (`lib/api.ts`).
//!
//! Deviation (flag `api-saml-sso-hint`, `Client::with_sso_hint`): a 403
//! with `X-GitHub-SSO: required; url=…` adds "Re-authorize SSO for <org>"
//! and the URL to the error; GHD's `lib/api.ts` keeps GitHub's message only.

use std::time::Duration;

use corvene_models::{
    Account, BypassReason, CheckConclusion, CheckStatus, GitHubRepository, RepoRuleEnforced,
    RepositoryPermission, RuleOperator,
};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::USER_AGENT;
use crate::endpoint::Endpoint;
use crate::error::{ApiErrorBody, GitHubError, Result};

/// The most of a job log that is read (`347-actions-job-logs`); GitHub's own
/// viewer cuts off far below this.
const MAX_JOB_LOG_BYTES: u64 = 64 * 1024 * 1024;

/// The response headers the paging helpers read (GHD `Response.headers`):
/// `(name, value)` pairs, names matched without regard to case.
pub type ResponseHeaders = [(String, String)];

pub struct Client {
    agent: ureq::Agent,
    endpoint: Endpoint,
    token: String,
    /// Append the body's `errors[].message` to an error's message.
    error_details: bool,
    /// Flag `api-saml-sso-hint`: a 403 carrying `X-GitHub-SSO` says which
    /// organization needs its SSO authorization renewed, and where.
    sso_hint: bool,
}

/// Flag `api-saml-sso-hint`: the hint for an `X-GitHub-SSO: required;
/// url=<authorize url>` header, naming the organization when the URL is
/// `…/orgs/<org>/sso…`. Other values (`partial-results; organizations=…`)
/// give no hint.
pub fn sso_hint(header: &str) -> Option<String> {
    let mut parts = header.split(';').map(str::trim);
    if !parts.next()?.eq_ignore_ascii_case("required") {
        return None;
    }
    let url = parts.find_map(|p| p.strip_prefix("url="))?.trim();
    if url.is_empty() {
        return None;
    }
    let org = url
        .split_once("/orgs/")
        .and_then(|(_, rest)| rest.split(['/', '?']).next())
        .filter(|org| !org.is_empty());
    Some(match org {
        Some(org) => format!("Re-authorize SSO for {org}: {url}"),
        None => format!("Re-authorize SSO: {url}"),
    })
}

#[derive(Debug, Deserialize)]
struct ApiUser {
    id: u64,
    login: String,
    name: Option<String>,
    avatar_url: Option<String>,
    #[serde(default)]
    plan: Option<ApiPlan>,
}

#[derive(Debug, Deserialize)]
struct ApiPlan {
    name: String,
}

#[derive(Debug, Deserialize)]
struct ApiEmail {
    email: String,
    primary: bool,
    verified: bool,
    /// `public` / `private`; missing on older Enterprise versions.
    #[serde(default)]
    visibility: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiRepository {
    pub name: String,
    pub owner: ApiOwner,
    pub html_url: String,
    pub clone_url: String,
    #[serde(default)]
    pub ssh_url: Option<String>,
    pub default_branch: Option<String>,
    #[serde(default)]
    pub private: bool,
    #[serde(default)]
    pub fork: bool,
    pub parent: Option<Box<ApiRepository>>,
    #[serde(default)]
    pub pushed_at: Option<String>,
    #[serde(default)]
    pub archived: bool,
    /// Only on `GET /repos/{owner}/{name}` with a token (`IAPIRepositoryPermissions`).
    #[serde(default)]
    pub permissions: Option<ApiRepositoryPermissions>,
    /// `false` when the owner disabled forking.
    #[serde(default)]
    pub allow_forking: Option<bool>,
    /// GraphQL id, for `createLinkedBranch` (flag `345-issues`).
    #[serde(default)]
    pub node_id: Option<String>,
}

/// `IAPIRepositoryPermissions`
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct ApiRepositoryPermissions {
    #[serde(default)]
    pub admin: bool,
    /// aka write
    #[serde(default)]
    pub push: bool,
    /// aka read
    #[serde(default)]
    pub pull: bool,
}

impl ApiRepositoryPermissions {
    /// `getPermissionsString`
    pub fn permission(self) -> Option<RepositoryPermission> {
        if self.admin {
            Some(RepositoryPermission::Admin)
        } else if self.push {
            Some(RepositoryPermission::Write)
        } else if self.pull {
            Some(RepositoryPermission::Read)
        } else {
            None
        }
    }
}

/// A GitHub release (`GET /repos/{owner}/{repo}/releases/tags/{tag}`,
/// `GET /repos/{owner}/{repo}/releases`). Everything past `html_url`
/// defaults, so the app's own release-notes lookup keeps decoding.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiRelease {
    pub tag_name: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    /// ISO-8601.
    #[serde(default)]
    pub published_at: Option<String>,
    pub html_url: String,
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub author: Option<ApiIssueUser>,
    /// ISO-8601.
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub target_commitish: Option<String>,
    #[serde(default)]
    pub tarball_url: Option<String>,
    #[serde(default)]
    pub assets: Vec<ApiReleaseAsset>,
}

/// An uploaded release asset.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ApiReleaseAsset {
    pub name: String,
    #[serde(default)]
    pub size: u64,
    pub browser_download_url: String,
    #[serde(default)]
    pub download_count: u64,
}

/// `POST /repos/{owner}/{repo}/releases` body (flag `346-releases`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NewRelease {
    pub tag_name: String,
    /// The commit a tag that does not exist yet is created at; ignored
    /// for an existing tag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_commitish: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub body: String,
    pub draft: bool,
    pub prerelease: bool,
}

/// `POST /repos/{owner}/{repo}/releases/generate-notes` answer.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GeneratedNotes {
    pub name: String,
    pub body: String,
}

/// `POST /repos/{owner}/{repo}/issues` body (flag `345-issues`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NewIssue {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
}

/// A repository label (`GET /repos/{owner}/{repo}/labels`, `issue.labels`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ApiLabel {
    pub name: String,
    /// Six hex digits without the `#`.
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// The short user record on issues, releases and the assignees list.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ApiIssueUser {
    pub login: String,
    #[serde(default)]
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub html_url: Option<String>,
}

/// GHD `IAPIRepositoryCloneInfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryCloneInfo {
    pub url: String,
    pub default_branch: Option<String>,
}

/// `state` filter for [`Client::issues`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueState {
    Open,
    Closed,
    All,
}

impl IssueState {
    fn as_str(self) -> &'static str {
        match self {
            IssueState::Open => "open",
            IssueState::Closed => "closed",
            IssueState::All => "all",
        }
    }
}

/// `IAPIIssue` (+ the `pull_request` marker used to filter PRs out). The
/// fields past `pull_request` feed the Issues view (flag `345-issues`) and
/// default, since the autocomplete cache only persists the first four.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiIssue {
    pub number: u64,
    pub title: String,
    /// `open` | `closed`
    pub state: String,
    pub updated_at: String,
    #[serde(default)]
    pub pull_request: Option<serde_json::Value>,
    #[serde(default)]
    pub id: u64,
    /// GraphQL id, for `createLinkedBranch`.
    #[serde(default)]
    pub node_id: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub closed_at: Option<String>,
    #[serde(default)]
    pub user: Option<ApiIssueUser>,
    #[serde(default)]
    pub labels: Vec<ApiLabel>,
    #[serde(default)]
    pub assignees: Vec<ApiIssueUser>,
    #[serde(default)]
    pub comments: u64,
    /// `completed` | `not_planned` | `reopened`; absent on older GitHub
    /// Enterprise Server.
    #[serde(default)]
    pub state_reason: Option<String>,
}

/// `IAPIFullIdentity` (`GET /users/{login}`), also the `IAPIIdentity` on
/// reviews and comments.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ApiIdentity {
    pub id: u64,
    pub login: String,
    /// The profile page (`IAPIIdentity.html_url`).
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
    /// `type`: `User`, `Organization` or `Bot`.
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
}

/// `IAPIPullRequestReview.state`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ApiPullRequestReviewState {
    Approved,
    Dismissed,
    Pending,
    Commented,
    ChangesRequested,
}

/// `IAPIPullRequestReview` (`GET /repos/{owner}/{repo}/pulls/{n}/reviews/{id}`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ApiPullRequestReview {
    pub id: u64,
    pub user: ApiIdentity,
    /// Empty for a review without a summary comment.
    #[serde(default, deserialize_with = "null_as_empty")]
    pub body: String,
    pub html_url: String,
    pub submitted_at: String,
    pub state: ApiPullRequestReviewState,
}

/// `IAPIComment`: an issue comment on a pull request, or a review comment
/// (`GET /repos/{owner}/{repo}/issues/comments/{id}`,
/// `…/pulls/comments/{id}`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ApiIssueComment {
    pub id: u64,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub body: String,
    pub html_url: String,
    pub user: ApiIdentity,
    pub created_at: String,
}

/// `null` → `""` for bodies the API may send as `null`.
fn null_as_empty<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<String, D::Error> {
    Ok(Option::<String>::deserialize(d)?.unwrap_or_default())
}

/// `IAPIMentionableUser`.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiMentionableUser {
    pub login: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiOwner {
    pub login: String,
}

/// `IAPIPullRequestRef`
#[derive(Debug, Clone, Deserialize)]
pub struct ApiPullRequestRef {
    #[serde(rename = "ref")]
    pub ref_name: String,
    pub sha: String,
    /// `null` when the head repository was deleted.
    pub repo: Option<ApiRepository>,
}

/// `IAPIPullRequest`
#[derive(Debug, Clone, Deserialize)]
pub struct ApiPullRequest {
    pub number: u64,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub user: ApiOwner,
    pub head: ApiPullRequestRef,
    pub base: ApiPullRequestRef,
    #[serde(default)]
    pub body: Option<String>,
    /// `open` | `closed`
    pub state: String,
    #[serde(default)]
    pub draft: bool,
    /// Corvene: the assignees and the users asked for a review (the pull
    /// request list's filters).
    #[serde(default, deserialize_with = "null_as_empty_vec")]
    pub assignees: Vec<ApiOwner>,
    #[serde(default, deserialize_with = "null_as_empty_vec")]
    pub requested_reviewers: Vec<ApiOwner>,
    /// Corvene (`354-pull-request-event-notifications`): the teams asked
    /// for a review, when it was merged and by whom, and its page.
    #[serde(default, deserialize_with = "null_as_empty_vec")]
    pub requested_teams: Vec<ApiTeam>,
    #[serde(default)]
    pub merged_at: Option<String>,
    #[serde(default)]
    pub merged_by: Option<ApiOwner>,
    #[serde(default)]
    pub html_url: Option<String>,
}

/// A team asked for a review (`requested_teams[]`).
#[derive(Debug, Clone, Deserialize)]
pub struct ApiTeam {
    pub slug: String,
    #[serde(default)]
    pub name: String,
}

/// `null` → `[]` for lists the API may send as `null`.
fn null_as_empty_vec<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> std::result::Result<Vec<T>, D::Error> {
    Ok(Option::<Vec<T>>::deserialize(d)?.unwrap_or_default())
}

/// `IAPIRefStatusItem` (the legacy commit status API).
#[derive(Debug, Clone, Deserialize)]
pub struct ApiRefStatusItem {
    pub id: u64,
    /// `success` | `pending` | `failure` | `error`
    pub state: String,
    #[serde(default)]
    pub target_url: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub context: String,
}

/// `IAPIRefStatus`
#[derive(Debug, Clone, Deserialize)]
pub struct ApiRefStatus {
    pub state: String,
    #[serde(default)]
    pub total_count: u64,
    #[serde(default)]
    pub statuses: Vec<ApiRefStatusItem>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiCheckSuiteRef {
    pub id: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiCheckApp {
    #[serde(default)]
    pub name: String,
}

/// `IAPIRefCheckRun`
#[derive(Debug, Clone, Deserialize)]
pub struct ApiRefCheckRun {
    pub id: u64,
    pub status: CheckStatus,
    #[serde(default)]
    pub conclusion: Option<CheckConclusion>,
    pub name: String,
    #[serde(default)]
    pub check_suite: Option<ApiCheckSuiteRef>,
    #[serde(default)]
    pub app: Option<ApiCheckApp>,
    #[serde(default)]
    pub completed_at: Option<String>,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub pull_requests: Vec<serde_json::Value>,
}

/// `IAPIRefCheckRuns`
#[derive(Debug, Clone, Deserialize)]
pub struct ApiRefCheckRuns {
    #[serde(default)]
    pub total_count: u64,
    #[serde(default)]
    pub check_runs: Vec<ApiRefCheckRun>,
}

/// `IAPICheckSuite`
#[derive(Debug, Clone, Deserialize)]
pub struct ApiCheckSuite {
    pub id: u64,
    #[serde(default)]
    pub rerequestable: bool,
    #[serde(default)]
    pub runs_rerequestable: bool,
    pub status: CheckStatus,
    pub created_at: String,
}

/// `IAPIWorkflowRun`, with what the Actions view (Corvene `351-actions`)
/// reads besides GHD's fields.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiWorkflowRun {
    pub id: u64,
    pub workflow_id: u64,
    #[serde(default)]
    pub name: String,
    pub created_at: String,
    #[serde(default)]
    pub check_suite_id: Option<u64>,
    #[serde(default)]
    pub event: String,
    /// `queued`, `in_progress`, `completed`, `waiting`, `requested`, `pending`.
    #[serde(default)]
    pub status: Option<CheckStatus>,
    #[serde(default)]
    pub conclusion: Option<CheckConclusion>,
    #[serde(default)]
    pub head_branch: Option<String>,
    #[serde(default)]
    pub head_sha: String,
    #[serde(default)]
    pub actor: Option<ApiIssueUser>,
    #[serde(default)]
    pub triggering_actor: Option<ApiIssueUser>,
    #[serde(default)]
    pub run_number: u64,
    #[serde(default)]
    pub run_attempt: u64,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub run_started_at: Option<String>,
    /// The commit's or pull request's title GitHub lists the run under.
    #[serde(default)]
    pub display_title: Option<String>,
    /// The workflow file, `.github/workflows/ci.yml`.
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiWorkflowRuns {
    #[serde(default)]
    pub total_count: u64,
    #[serde(default)]
    pub workflow_runs: Vec<ApiWorkflowRun>,
}

/// `IAPIWorkflowJobStep`
#[derive(Debug, Clone, Deserialize)]
pub struct ApiWorkflowJobStep {
    pub name: String,
    pub number: u64,
    pub status: CheckStatus,
    #[serde(default)]
    pub conclusion: Option<CheckConclusion>,
    #[serde(default)]
    pub completed_at: Option<String>,
    #[serde(default)]
    pub started_at: Option<String>,
}

/// `IAPIWorkflowJob`
#[derive(Debug, Clone, Deserialize)]
pub struct ApiWorkflowJob {
    pub id: u64,
    pub name: String,
    pub status: CheckStatus,
    #[serde(default)]
    pub conclusion: Option<CheckConclusion>,
    #[serde(default)]
    pub steps: Vec<ApiWorkflowJobStep>,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiWorkflowJobs {
    #[serde(default)]
    pub total_count: u64,
    #[serde(default)]
    pub jobs: Vec<ApiWorkflowJob>,
}

/// `IAPIPushControl`
#[derive(Debug, Clone, Deserialize)]
pub struct ApiPushControl {
    #[serde(default)]
    pub required_status_checks: Vec<String>,
    #[serde(default)]
    pub required_approving_review_count: u64,
    #[serde(default = "default_true")]
    pub allow_actor: bool,
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub required_signatures: bool,
    #[serde(default)]
    pub required_linear_history: bool,
    #[serde(default = "default_true")]
    pub allow_deletions: bool,
    #[serde(default = "default_true")]
    pub allow_force_pushes: bool,
}

fn default_true() -> bool {
    true
}

impl ApiPushControl {
    /// A branch nobody can push to directly (`isBranchPushable` negated).
    pub fn is_pushable(&self) -> bool {
        self.allow_actor
            && self.required_status_checks.is_empty()
            && self.required_approving_review_count == 0
    }
}

impl Default for ApiPushControl {
    fn default() -> Self {
        Self {
            required_status_checks: Vec::new(),
            required_approving_review_count: 0,
            allow_actor: true,
            pattern: None,
            required_signatures: false,
            required_linear_history: false,
            allow_deletions: true,
            allow_force_pushes: true,
        }
    }
}

/// `IAPIRepoRuleMetadataParameters`
#[derive(Debug, Clone, Deserialize)]
pub struct ApiRepoRuleParameters {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub negate: bool,
    pub pattern: String,
    pub operator: RuleOperator,
}

/// `IAPIRepoRule`
#[derive(Debug, Clone, Deserialize)]
pub struct ApiRepoRule {
    pub ruleset_id: u64,
    /// `creation` | `update` | `required_signatures` | `commit_message_pattern` | …
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub parameters: Option<ApiRepoRuleParameters>,
}

/// `IAPISlimRepoRuleset`
#[derive(Debug, Clone, Deserialize)]
pub struct ApiSlimRepoRuleset {
    pub id: u64,
}

/// `IAPIRepoRuleset`
#[derive(Debug, Clone, Deserialize)]
pub struct ApiRepoRuleset {
    pub id: u64,
    /// `always` | `pull_requests_only` | `never`, and since 2025-09
    /// `exempt` (the ruleset is not evaluated for the user at all)
    #[serde(default)]
    pub current_user_can_bypass: Option<String>,
}

impl ApiRepoRuleset {
    /// GHD: `always` is a bypass, anything else enforces the rules.
    pub fn enforced(&self) -> RepoRuleEnforced {
        self.enforced_for(false)
    }

    /// [`Self::enforced`], or with `exempt_skips` (Corvene
    /// `338-ruleset-exempt-bypass`) [`RepoRuleEnforced::No`] for an `exempt`
    /// user, whose pushes GitHub does not check against the ruleset. GHD
    /// predates the mode and treats it as enforced (desktop#21347).
    pub fn enforced_for(&self, exempt_skips: bool) -> RepoRuleEnforced {
        match self.current_user_can_bypass.as_deref() {
            Some("always") => RepoRuleEnforced::Bypass,
            Some("exempt") if exempt_skips => RepoRuleEnforced::No,
            _ => RepoRuleEnforced::Yes,
        }
    }
}

/// `IAPICreatePushProtectionBypassResponse`
#[derive(Debug, Clone, Deserialize)]
pub struct ApiPushProtectionBypass {
    pub reason: String,
    #[serde(default)]
    pub expire_at: Option<String>,
    #[serde(default)]
    pub token_type: Option<String>,
}

/// `encodeURIComponent` for refs and branch names in API paths.
pub fn encode_path_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Corvene `341-custom-autolinks`: an entry of `GET
/// repos/{owner}/{name}/autolinks` (only repository admins may read them).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ApiAutolink {
    pub key_prefix: String,
    pub url_template: String,
    /// GitHub's default is alphanumeric.
    #[serde(default = "default_true")]
    pub is_alphanumeric: bool,
}

/// GHD `IAPIBranch` (`GET repos/{owner}/{name}/branches`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ApiBranch {
    /// The branch name on the remote (not a full ref).
    pub name: String,
    /// The branch has protection rules.
    #[serde(default)]
    pub protected: bool,
}

/// `headers.get(name)`: the first value of `name`, any case.
pub(crate) fn header<'a>(headers: &'a ResponseHeaders, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// The headers of a response as [`ResponseHeaders`] pairs.
/// A response's headers as `(name, value)` pairs.
pub(crate) type HeaderPairs = Vec<(String, String)>;

pub(crate) fn header_pairs(headers: &ureq::http::HeaderMap) -> HeaderPairs {
    headers
        .iter()
        .filter_map(|(name, value)| Some((name.to_string(), value.to_str().ok()?.to_string())))
        .collect()
}

/// GHD `getNextPagePathFromLink`: the path (and query) of the `rel="next"`
/// link in the `Link` header, `None` without one.
pub fn get_next_page_path_from_link(headers: &ResponseHeaders) -> Option<String> {
    let link = header(headers, "Link").filter(|l| !l.is_empty())?;
    for part in link.split(',') {
        // `<([^>]+)>; rel="([^"]+)"`
        let Some((_, rest)) = part.split_once('<') else {
            continue;
        };
        let Some((url, rest)) = rest.split_once('>') else {
            continue;
        };
        let Some(rel) = rest
            .strip_prefix("; rel=\"")
            .and_then(|r| r.split_once('"'))
            .map(|(rel, _)| rel)
        else {
            continue;
        };
        if !url.is_empty() && rel == "next" {
            return url_path(url);
        }
    }
    None
}

/// Node's `URL.parse(url).path`: path and query, without scheme, host and
/// fragment.
fn url_path(url: &str) -> Option<String> {
    let url = url.split('#').next().unwrap_or(url);
    let path = match url.split_once("://") {
        Some((_, rest)) => match rest.find(['/', '?']) {
            Some(i) if rest[i..].starts_with('?') => format!("/{}", &rest[i..]),
            Some(i) => rest[i..].to_string(),
            None => "/".to_string(),
        },
        None => url.to_string(),
    };
    (!path.is_empty()).then_some(path)
}

/// JavaScript's `parseInt(value, 10)`: the leading integer, `None` for NaN.
fn parse_int(value: &str) -> Option<i64> {
    let value = value.trim_start();
    let (sign, digits) = match value.as_bytes().first() {
        Some(b'-') => (-1, &value[1..]),
        Some(b'+') => (1, &value[1..]),
        _ => (1, value),
    };
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    digits[..end].parse::<i64>().ok().map(|n| sign * n)
}

/// GHD `getNextPagePathWithIncreasingPageSize`: the next link
/// ([`get_next_page_path_from_link`]), with `per_page` doubled (at most
/// 100) and `page` recomputed once the items received so far fill whole
/// pages of the doubled size, so a long list is read in fewer requests.
pub fn get_next_page_path_with_increasing_page_size(headers: &ResponseHeaders) -> Option<String> {
    let next_path = get_next_page_path_from_link(headers)?;
    let (pathname, query) = next_path.split_once('?').unwrap_or((&next_path, ""));
    let mut pairs: Vec<(&str, &str)> = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| pair.split_once('=').unwrap_or((pair, "")))
        .collect();
    // `typeof per_page === 'string'`: exactly one value
    let single = |key: &str| {
        let mut values = pairs.iter().filter(|(k, _)| *k == key).map(|(_, v)| *v);
        match (values.next(), values.next()) {
            (Some(value), None) => parse_int(value),
            _ => None,
        }
    };
    let (Some(page_size), Some(page_number)) = (single("per_page"), single("page")) else {
        return Some(next_path);
    };
    if page_size == 0 || page_number == 0 {
        return Some(next_path);
    }
    // the next page's link: what came before it has been received
    let (Some(received), Some(doubled)) = (
        (page_number - 1).checked_mul(page_size),
        page_size.checked_mul(2),
    ) else {
        return Some(next_path);
    };
    let next_page_size = doubled.min(100);
    if page_size == next_page_size || received % next_page_size != 0 {
        return Some(next_path);
    }
    let per_page = next_page_size.to_string();
    let page = (received / next_page_size + 1).to_string();
    for (key, value) in &mut pairs {
        match *key {
            "per_page" => *value = per_page.as_str(),
            "page" => *value = page.as_str(),
            _ => {}
        }
    }
    let query: Vec<String> = pairs.iter().map(|(k, v)| format!("{k}={v}")).collect();
    Some(format!("{pathname}?{}", query.join("&")))
}

impl Client {
    pub fn new(endpoint: Endpoint, token: impl Into<String>) -> Self {
        let agent = corvene_platform::proxy::agent(
            ureq::Agent::config_builder()
                // the system proxy, PAC scripts and saved proxy credentials
                // (`corvene_platform::proxy`)
                .middleware(corvene_platform::proxy::per_request)
                .timeout_global(Some(Duration::from_secs(30)))
                .http_status_as_error(false)
                .user_agent(USER_AGENT)
                .build(),
        );
        Self {
            agent,
            endpoint,
            token: token.into(),
            error_details: false,
            sso_hint: false,
        }
    }

    /// A 403 that needs SAML SSO authorization says so (flag
    /// `api-saml-sso-hint`; GHD shows GitHub's message only).
    pub fn with_sso_hint(mut self, on: bool) -> Self {
        self.sso_hint = on;
        self
    }

    /// Error messages show validation `errors` without a `message` as
    /// `field code`, and publishing to an organization reports GitHub's
    /// reasons (flag `api-error-details`; GHD drops those errors and
    /// replaces an organization's error with a generic hint).
    pub fn with_error_details(mut self, on: bool) -> Self {
        self.error_details = on;
        self
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    pub(crate) fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.get_json_accept(path, "application/vnd.github+json")
    }

    /// GHD `parsedResponse` for an error status: `APIError` with the parsed
    /// body, its `message` (or `API error <url>: <statusText> (<status>)`
    /// without one) and whether it means the token was revoked (GHD
    /// `ghRequest`).
    ///
    /// With `sso_hint` (flag `327-api-saml-sso-hint`) a 403's message names
    /// the SSO authorization its `X-GitHub-SSO` header asks for.
    pub(crate) fn api_error(
        &self,
        url: &str,
        mut response: ureq::http::Response<ureq::Body>,
    ) -> GitHubError {
        let status = response.status();
        let headers = response.headers();
        let token_invalidated = status == ureq::http::StatusCode::UNAUTHORIZED
            && headers.contains_key("X-GitHub-Request-Id")
            && !headers.contains_key("X-GitHub-OTP");
        let hint = if self.sso_hint && status == ureq::http::StatusCode::FORBIDDEN {
            headers
                .get("x-github-sso")
                .and_then(|v| v.to_str().ok())
                .and_then(sso_hint)
        } else {
            None
        };
        let body = response.body_mut().read_to_vec().unwrap_or_default();
        let api_error = ApiErrorBody::parse(&body);
        let message = api_error
            .as_ref()
            .and_then(|e| e.message(self.error_details))
            .unwrap_or_else(|| {
                format!(
                    "API error {url}: {} ({})",
                    status.canonical_reason().unwrap_or_default(),
                    status.as_u16()
                )
            });
        let message = match hint {
            Some(hint) => format!("{}. {hint}", message.trim_end_matches('.')),
            None => message,
        };
        GitHubError::Api {
            status: status.as_u16(),
            message,
            api_error,
            token_invalidated,
        }
    }

    /// `GET` with a specific `Accept` header (the preview APIs); an error
    /// status is an `Err`.
    pub(crate) fn get_response(
        &self,
        path: &str,
        accept: &str,
    ) -> Result<ureq::http::Response<ureq::Body>> {
        let url = self.endpoint.api(path);
        debug!(%url, "GET");
        let mut request = self
            .agent
            .get(&url)
            .header("Accept", accept)
            .header("X-GitHub-Api-Version", "2022-11-28");
        // an empty token is GHD's `Account.anonymous()`: no Authorization
        if !self.token.is_empty() {
            request = request.header("Authorization", &format!("Bearer {}", self.token));
        }
        let response = request.call()?;
        if !response.status().is_success() {
            return Err(self.api_error(&url, response));
        }
        Ok(response)
    }

    /// `GET` with a specific `Accept` header (the preview APIs).
    fn get_json_accept<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        accept: &str,
    ) -> Result<T> {
        let mut response = self.get_response(path, accept)?;
        Ok(response.body_mut().read_json()?)
    }

    /// GHD `fetchAll` (with `suppressErrors: false`): `path`, then each page
    /// `next_page` finds in the previous response's headers, while
    /// `keep_going(&items)` holds (asked only when there is a next page) and
    /// at most `max_pages` pages. An error on any page fails the call.
    fn fetch_all<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        max_pages: usize,
        next_page: fn(&ResponseHeaders) -> Option<String>,
        mut keep_going: impl FnMut(&[T]) -> bool,
    ) -> Result<Vec<T>> {
        let mut items: Vec<T> = Vec::new();
        let mut next = Some(path.to_string());
        let mut pages = 0;
        while let Some(path) = next.take() {
            let mut response = self.get_response(&path, "application/vnd.github+json")?;
            let headers = header_pairs(response.headers());
            if let Some(page) = response.body_mut().read_json::<Option<Vec<T>>>()? {
                items.extend(page);
            }
            pages += 1;
            next = next_page(&headers).filter(|_| pages < max_pages && keep_going(&items));
        }
        Ok(items)
    }

    /// `GET /user` (+ `/user/emails` when the scope allows) → `Account`.
    pub fn current_user(&self, scopes: Vec<String>) -> Result<Account> {
        let user: ApiUser = self.get_json("user")?;
        let (emails, private_primary_email) = match self.get_json::<Vec<ApiEmail>>("user/emails") {
            Ok(list) => {
                let mut list: Vec<ApiEmail> = list.into_iter().filter(|e| e.verified).collect();
                list.sort_by_key(|e| !e.primary);
                // GHD `isEmailPublic`: no visibility (older Enterprise) is public
                let private = list
                    .first()
                    .is_some_and(|e| e.primary && e.visibility.as_deref() == Some("private"));
                (list.into_iter().map(|e| e.email).collect(), private)
            }
            Err(err) => {
                debug!(?err, "could not read emails");
                (Vec::new(), false)
            }
        };
        Ok(Account {
            endpoint: self.endpoint.api_base.clone(),
            id: user.id,
            login: user.login,
            name: user.name,
            avatar_url: user.avatar_url,
            emails,
            scopes,
            plan: user.plan.map(|p| p.name),
            private_primary_email,
        })
    }

    /// Corvene (`350-ssh-key-helper`): the OAuth scopes of the token, from
    /// `GET /user`'s `X-OAuth-Scopes` header. `None` when the header is
    /// missing (a fine-grained token or a GitHub App's, whose permissions
    /// only show when a call is refused).
    pub fn token_scopes(&self) -> Result<Option<Vec<String>>> {
        let response = self.get_response("user", "application/vnd.github+json")?;
        Ok(response
            .headers()
            .get("x-oauth-scopes")
            .and_then(|v| v.to_str().ok())
            .map(crate::parse_scopes))
    }

    /// Corvene (`350-ssh-key-helper`): `POST /user/keys`, add `key` (an
    /// OpenSSH public key line) to the account's SSH keys as `title`. Needs
    /// the `write:public_key` scope (or `admin:public_key`); GitHub answers
    /// 422 when the key is already on an account.
    pub fn add_ssh_key(&self, title: &str, key: &str) -> Result<()> {
        let _: serde_json::Value = self.post_json(
            "user/keys",
            &serde_json::json!({ "title": title, "key": key }),
        )?;
        Ok(())
    }

    /// `GET` whose error answers mean "not available" rather than an error
    /// (`fetchCombinedRefStatus`, `fetchRefCheckRuns`…), except a revoked
    /// token, which the caller reports (GHD emits it from `ghRequest`).
    fn get_json_opt<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        accept: &str,
    ) -> Result<Option<T>> {
        match self.get_json_accept::<T>(path, accept) {
            Ok(value) => Ok(Some(value)),
            Err(err) if err.is_token_invalidated() => Err(err),
            Err(GitHubError::Api {
                status, message, ..
            }) => {
                debug!(status, %message, %path, "not available");
                Ok(None)
            }
            Err(err) => Err(err),
        }
    }

    /// `POST` without a body; `Ok(true)` for a 2xx answer.
    fn post_empty(&self, path: &str) -> Result<bool> {
        let url = self.endpoint.api(path);
        debug!(%url, "POST");
        let response = self
            .agent
            .post(&url)
            .header("Accept", "application/vnd.github+json")
            .header("Authorization", &format!("Bearer {}", self.token))
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send_empty()?;
        if response.status() == ureq::http::StatusCode::UNAUTHORIZED {
            return Err(self.api_error(&url, response));
        }
        Ok(response.status().is_success())
    }

    fn post_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T> {
        let url = self.endpoint.api(path);
        debug!(%url, "POST");
        let mut response = self
            .agent
            .post(&url)
            .header("Accept", "application/vnd.github+json")
            .header("Authorization", &format!("Bearer {}", self.token))
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send_json(body)?;
        if !response.status().is_success() {
            return Err(self.api_error(&url, response));
        }
        Ok(response.body_mut().read_json()?)
    }

    /// Corvene (`351-actions`): a conditional `GET` (`If-None-Match:
    /// <etag>`). `Ok(None)` when GitHub answers 304 Not Modified (which does
    /// not count against the rate limit), else the value and its `ETag`.
    pub(crate) fn get_json_if_none_match<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        etag: Option<&str>,
    ) -> Result<Option<(T, Option<String>)>> {
        let url = self.endpoint.api(path);
        debug!(%url, etag, "GET");
        let mut request = self
            .agent
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if !self.token.is_empty() {
            request = request.header("Authorization", &format!("Bearer {}", self.token));
        }
        if let Some(etag) = etag {
            request = request.header("If-None-Match", etag);
        }
        let mut response = request.call()?;
        if response.status() == ureq::http::StatusCode::NOT_MODIFIED {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(self.api_error(&url, response));
        }
        let etag = response
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        Ok(Some((response.body_mut().read_json()?, etag)))
    }

    /// Corvene (`354-pull-request-event-notifications`): a conditional
    /// `GET` on `If-Modified-Since` (the notifications API's polling
    /// contract). `None` for a 304, which costs no rate limit; the response
    /// headers come back either way (`Last-Modified`, `X-Poll-Interval`,
    /// `Date`).
    pub(crate) fn get_json_if_modified_since<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        since: Option<&str>,
    ) -> Result<(Option<T>, HeaderPairs)> {
        let url = self.endpoint.api(path);
        debug!(%url, since, "GET");
        let mut request = self
            .agent
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if !self.token.is_empty() {
            request = request.header("Authorization", &format!("Bearer {}", self.token));
        }
        if let Some(since) = since {
            request = request.header("If-Modified-Since", since);
        }
        let mut response = request.call()?;
        let headers = header_pairs(response.headers());
        if response.status() == ureq::http::StatusCode::NOT_MODIFIED {
            return Ok((None, headers));
        }
        if !response.status().is_success() {
            return Err(self.api_error(&url, response));
        }
        Ok((Some(response.body_mut().read_json()?), headers))
    }

    /// Corvene (`351-actions`): a `POST` (with `body`, else empty) or a
    /// `DELETE` whose success has no content to read (204, 201, 202). Unlike
    /// [`Self::post_empty`], every error status is an `Err`.
    pub(crate) fn send_no_content(
        &self,
        delete: bool,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<()> {
        let url = self.endpoint.api(path);
        debug!(%url, delete, "send");
        let auth = format!("Bearer {}", self.token);
        let response = if delete {
            self.agent
                .delete(&url)
                .header("Accept", "application/vnd.github+json")
                .header("Authorization", &auth)
                .header("X-GitHub-Api-Version", "2022-11-28")
                .call()?
        } else {
            let request = self
                .agent
                .post(&url)
                .header("Accept", "application/vnd.github+json")
                .header("Authorization", &auth)
                .header("X-GitHub-Api-Version", "2022-11-28");
            match body {
                Some(body) => request.send_json(body)?,
                None => request.send_empty()?,
            }
        };
        if !response.status().is_success() {
            return Err(self.api_error(&url, response));
        }
        Ok(())
    }

    /// `POST /graphql`: the query's `data`. A 200 answer whose `errors`
    /// left no `data` is an error with the first error's message.
    pub(crate) fn post_graphql<T: serde::de::DeserializeOwned>(
        &self,
        query: &str,
        variables: &serde_json::Value,
    ) -> Result<T> {
        self.post_graphql_with(query, variables, false)
    }

    /// [`Self::post_graphql`] for a mutation: an answer that carries
    /// `errors` is an error even when `data` came with it (a refused
    /// mutation answers `data: { field: null }` plus the reason).
    pub(crate) fn post_graphql_strict<T: serde::de::DeserializeOwned>(
        &self,
        query: &str,
        variables: &serde_json::Value,
    ) -> Result<T> {
        self.post_graphql_with(query, variables, true)
    }

    fn post_graphql_with<T: serde::de::DeserializeOwned>(
        &self,
        query: &str,
        variables: &serde_json::Value,
        strict: bool,
    ) -> Result<T> {
        #[derive(Deserialize)]
        struct Message {
            message: String,
        }
        #[derive(Deserialize)]
        struct Response<T> {
            data: Option<T>,
            #[serde(default)]
            errors: Vec<Message>,
        }
        let url = self.endpoint.graphql();
        debug!(%url, "POST");
        let mut response = self
            .agent
            .post(&url)
            .header("Authorization", &format!("Bearer {}", self.token))
            .send_json(serde_json::json!({ "query": query, "variables": variables }))?;
        if !response.status().is_success() {
            return Err(self.api_error(&url, response));
        }
        let body: Response<T> = response.body_mut().read_json()?;
        match (body.data, body.errors.into_iter().next()) {
            (Some(_), Some(error)) if strict => Err(GitHubError::api(200, error.message)),
            (Some(data), _) => Ok(data),
            (None, Some(error)) => Err(GitHubError::api(200, error.message)),
            (None, None) => Err(GitHubError::api(200, "GraphQL answered without data")),
        }
    }

    /// `GET /orgs/{org}/teams` (flag `publish-team`): the organization's
    /// teams the account can see, as `(id, name)` sorted by name.
    pub fn org_teams(&self, org: &str) -> Result<Vec<(u64, String)>> {
        #[derive(Deserialize)]
        struct Team {
            id: u64,
            name: String,
        }
        let teams: Vec<Team> = self.get_json(&format!("orgs/{org}/teams?per_page=100"))?;
        let mut teams: Vec<(u64, String)> = teams.into_iter().map(|t| (t.id, t.name)).collect();
        teams.sort_by_key(|(_, name)| name.to_lowercase());
        Ok(teams)
    }

    /// `GET /user/orgs`: organisations the user can publish to.
    pub fn user_orgs(&self) -> Result<Vec<String>> {
        #[derive(Deserialize)]
        struct Org {
            login: String,
        }
        let orgs: Vec<Org> = self.get_json("user/orgs?per_page=100")?;
        let mut logins: Vec<String> = orgs.into_iter().map(|o| o.login).collect();
        logins.sort_by_key(|l| l.to_lowercase());
        Ok(logins)
    }

    /// `POST /user/repos` or `/orgs/{org}/repos` (GHD `createRepository`).
    ///
    /// `team_id` (flag `publish-team`, organizations only) grants that team
    /// access to the new repository.
    pub fn create_repository(
        &self,
        org: Option<&str>,
        name: &str,
        description: &str,
        private: bool,
        team_id: Option<u64>,
    ) -> Result<GitHubRepository> {
        let path = match org {
            Some(org) => format!("orgs/{org}/repos"),
            None => "user/repos".to_string(),
        };
        let mut body = serde_json::json!({
            "name": name,
            "description": description,
            "private": private,
        });
        if let (Some(team_id), Some(_)) = (team_id, org) {
            body["team_id"] = serde_json::json!(team_id);
        }
        let repo: ApiRepository = self
            .post_json(&path, &body)
            .map_err(|err| match (err, org) {
                // GHD replaces an organization's API error with a hint
                (
                    GitHubError::Api {
                        status,
                        api_error,
                        token_invalidated,
                        ..
                    },
                    Some(org),
                ) if !self.error_details => GitHubError::Api {
                    status,
                    message: format!(
                        "Unable to create repository for organization '{org}'. Verify that the \
                     repository does not already exist and that you have permission to create a \
                     repository there."
                    ),
                    api_error,
                    token_invalidated,
                },
                (err, _) => err,
            })?;
        Ok(self.convert(repo))
    }

    /// `GET /repos/{owner}/{name}`.
    pub fn repository(&self, owner: &str, name: &str) -> Result<GitHubRepository> {
        let repo: ApiRepository = self.get_json(&format!("repos/{owner}/{name}"))?;
        Ok(self.convert(repo))
    }

    /// `fetchPushedAt`: when anything was last pushed to `owner/name`
    /// (`pushed_at`, ISO 8601), `None` when GitHub does not say.
    pub fn pushed_at(&self, owner: &str, name: &str) -> Result<Option<String>> {
        let repo: ApiRepository = self.get_json(&format!("repos/{owner}/{name}"))?;
        Ok(repo.pushed_at)
    }

    /// `fetchRepositoryCloneInfo`: the clone URL (SSH when `ssh`) and default
    /// branch of `owner/name`, `None` when the repository is not found (404,
    /// which GitHub also answers for private repositories the token can't see).
    pub fn repository_clone_info(
        &self,
        owner: &str,
        name: &str,
        ssh: bool,
    ) -> Result<Option<RepositoryCloneInfo>> {
        let path = format!(
            "repos/{}/{}",
            encode_path_component(owner),
            encode_path_component(name)
        );
        match self.get_json::<ApiRepository>(&path) {
            Ok(repo) => Ok(Some(RepositoryCloneInfo {
                url: match (ssh, repo.ssh_url) {
                    (true, Some(ssh_url)) => ssh_url,
                    _ => repo.clone_url,
                },
                default_branch: repo.default_branch,
            })),
            Err(GitHubError::Api { status: 404, .. }) => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// Corvene (`1223-checkout-from-fork`): the newest forks of
    /// `owner/name` (`GET /repos/{owner}/{name}/forks`, at most `pages` pages
    /// of 100).
    pub fn forks(&self, owner: &str, name: &str, pages: usize) -> Result<Vec<GitHubRepository>> {
        let path = format!(
            "repos/{}/{}/forks?sort=newest&per_page=100",
            encode_path_component(owner),
            encode_path_component(name)
        );
        let repos: Vec<ApiRepository> =
            self.fetch_all(&path, pages, get_next_page_path_from_link, |_| true)?;
        Ok(repos.into_iter().map(|r| self.convert(r)).collect())
    }

    /// `fetchPullRequestReview`: `GET /repos/{owner}/{name}/pulls/{number}/reviews/{id}`.
    pub fn pull_request_review(
        &self,
        owner: &str,
        name: &str,
        number: u64,
        review_id: &str,
    ) -> Result<Option<ApiPullRequestReview>> {
        self.get_json_opt(
            &format!(
                "repos/{}/{}/pulls/{number}/reviews/{}",
                encode_path_component(owner),
                encode_path_component(name),
                encode_path_component(review_id)
            ),
            "application/vnd.github+json",
        )
    }

    /// `fetchIssueComment`: `GET /repos/{owner}/{name}/issues/comments/{id}`.
    pub fn issue_comment(
        &self,
        owner: &str,
        name: &str,
        comment_id: &str,
    ) -> Result<Option<ApiIssueComment>> {
        self.get_json_opt(
            &format!(
                "repos/{}/{}/issues/comments/{}",
                encode_path_component(owner),
                encode_path_component(name),
                encode_path_component(comment_id)
            ),
            "application/vnd.github+json",
        )
    }

    /// `fetchPullRequestReviewComment`: `GET /repos/{owner}/{name}/pulls/comments/{id}`.
    pub fn pull_request_review_comment(
        &self,
        owner: &str,
        name: &str,
        comment_id: &str,
    ) -> Result<Option<ApiIssueComment>> {
        self.get_json_opt(
            &format!(
                "repos/{}/{}/pulls/comments/{}",
                encode_path_component(owner),
                encode_path_component(name),
                encode_path_component(comment_id)
            ),
            "application/vnd.github+json",
        )
    }

    /// `getAliveDesktopChannel`: `GET /desktop_internal/alive-channel`;
    /// `None` when the endpoint has no Alive service.
    pub fn alive_desktop_channel(&self) -> Result<Option<crate::alive::AliveChannel>> {
        self.get_json_opt(
            "desktop_internal/alive-channel",
            "application/vnd.github+json",
        )
    }

    /// `getAliveWebSocketURL`: `GET /alive_internal/websocket-url`; `None`
    /// on 404 (Alive disabled for the endpoint).
    pub fn alive_websocket_url(&self) -> Result<Option<String>> {
        Ok(self
            .get_json_opt::<crate::alive::AliveWebSocket>(
                "alive_internal/websocket-url",
                "application/vnd.github+json",
            )?
            .map(|ws| ws.url))
    }

    /// `GET /repos/{owner}/{name}/releases/tags/{tag}`; `None` when there is
    /// no such release.
    pub fn release_by_tag(&self, owner: &str, name: &str, tag: &str) -> Result<Option<ApiRelease>> {
        let path = format!(
            "repos/{}/{}/releases/tags/{}",
            encode_path_component(owner),
            encode_path_component(name),
            encode_path_component(tag)
        );
        match self.get_json::<ApiRelease>(&path) {
            Ok(release) => Ok(Some(release)),
            Err(GitHubError::Api { status: 404, .. }) => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// `GET /user/repos` (all pages, at most 20, newest pushed first) for
    /// the Clone dialog.
    pub fn user_repositories(&self) -> Result<Vec<GitHubRepository>> {
        let repos: Vec<ApiRepository> = self.fetch_all(
            "user/repos?per_page=100&sort=pushed&affiliation=owner,collaborator,organization_member",
            20,
            get_next_page_path_from_link,
            |_| true,
        )?;
        Ok(repos.into_iter().map(|r| self.convert(r)).collect())
    }

    /// `fetchIssues`: `GET /repos/{owner}/{name}/issues` (all pages). PRs are
    /// issues too, so anything carrying a `pull_request` key is dropped.
    /// `since` is an ISO-8601 timestamp; with it the API returns every issue
    /// updated at or after that moment (closed ones included, so callers can
    /// prune them).
    pub fn issues(
        &self,
        owner: &str,
        name: &str,
        state: IssueState,
        since: Option<&str>,
    ) -> Result<Vec<ApiIssue>> {
        let mut path = format!(
            "repos/{owner}/{name}/issues?state={}&per_page=100",
            state.as_str()
        );
        if let Some(since) = since {
            path.push_str("&since=");
            path.push_str(since);
        }
        let issues: Vec<ApiIssue> =
            self.fetch_all(&path, 20, get_next_page_path_from_link, |_| true)?;
        Ok(issues
            .into_iter()
            .filter(|i| i.pull_request.is_none())
            .collect())
    }

    /// Flag `345-issues`: the newest-updated closed issues, at most
    /// `max_pages` pages of 100 (`GET /repos/{owner}/{name}/issues?state=closed`),
    /// without the pull requests.
    pub fn closed_issues(
        &self,
        owner: &str,
        name: &str,
        max_pages: usize,
    ) -> Result<Vec<ApiIssue>> {
        let path = format!(
            "repos/{}/{}/issues?state=closed&sort=updated&direction=desc&per_page=100",
            encode_path_component(owner),
            encode_path_component(name)
        );
        let issues: Vec<ApiIssue> =
            self.fetch_all(&path, max_pages, get_next_page_path_from_link, |_| true)?;
        Ok(issues
            .into_iter()
            .filter(|i| i.pull_request.is_none())
            .collect())
    }

    /// Flag `345-issues`: `GET /repos/{owner}/{name}/labels` (at most 500).
    pub fn labels(&self, owner: &str, name: &str) -> Result<Vec<ApiLabel>> {
        let path = format!(
            "repos/{}/{}/labels?per_page=100",
            encode_path_component(owner),
            encode_path_component(name)
        );
        self.fetch_all(&path, 5, get_next_page_path_from_link, |_| true)
    }

    /// Flag `345-issues`: `GET /repos/{owner}/{name}/assignees` (at most
    /// 500), the users an issue can be assigned to.
    pub fn assignees(&self, owner: &str, name: &str) -> Result<Vec<ApiIssueUser>> {
        let path = format!(
            "repos/{}/{}/assignees?per_page=100",
            encode_path_component(owner),
            encode_path_component(name)
        );
        self.fetch_all(&path, 5, get_next_page_path_from_link, |_| true)
    }

    /// Flag `345-issues`: `POST /repos/{owner}/{name}/issues`.
    pub fn create_issue(&self, owner: &str, name: &str, new: &NewIssue) -> Result<ApiIssue> {
        let path = format!(
            "repos/{}/{}/issues",
            encode_path_component(owner),
            encode_path_component(name)
        );
        self.post_json(&path, &serde_json::to_value(new)?)
    }

    /// Flag `345-issues`: GitHub's own "create a branch for this issue"
    /// (GraphQL `createLinkedBranch`): the branch `name` is created at
    /// `oid` on GitHub and listed under the issue's Development section.
    /// `Ok(true)` when linked. Needs push access and GitHub Enterprise
    /// Server 3.7 or later; `oid` must already be on GitHub.
    pub fn create_linked_branch(
        &self,
        issue_node_id: &str,
        repository_node_id: &str,
        oid: &str,
        name: &str,
    ) -> Result<bool> {
        #[derive(Deserialize)]
        struct LinkedBranch {
            id: String,
        }
        #[derive(Deserialize)]
        struct Payload {
            #[serde(rename = "linkedBranch")]
            linked_branch: Option<LinkedBranch>,
        }
        #[derive(Deserialize)]
        struct Data {
            #[serde(rename = "createLinkedBranch")]
            create_linked_branch: Option<Payload>,
        }
        let data: Data = self.post_graphql(
            "mutation($issue: ID!, $repo: ID, $oid: GitObjectID!, $name: String) { \
             createLinkedBranch(input: {issueId: $issue, repositoryId: $repo, oid: $oid, name: $name}) \
             { linkedBranch { id } } }",
            &serde_json::json!({
                "issue": issue_node_id,
                "repo": repository_node_id,
                "oid": oid,
                "name": name,
            }),
        )?;
        Ok(data
            .create_linked_branch
            .and_then(|p| p.linked_branch)
            .is_some_and(|b| !b.id.is_empty()))
    }

    /// Flag `346-releases`: `GET /repos/{owner}/{name}/releases`, newest
    /// first, at most `max_pages` pages of 50. Drafts are included when the
    /// token can push.
    pub fn releases(&self, owner: &str, name: &str, max_pages: usize) -> Result<Vec<ApiRelease>> {
        let path = format!(
            "repos/{}/{}/releases?per_page=50",
            encode_path_component(owner),
            encode_path_component(name)
        );
        self.fetch_all(&path, max_pages, get_next_page_path_from_link, |_| true)
    }

    /// Flag `346-releases`: `POST /repos/{owner}/{name}/releases`.
    pub fn create_release(&self, owner: &str, name: &str, new: &NewRelease) -> Result<ApiRelease> {
        let path = format!(
            "repos/{}/{}/releases",
            encode_path_component(owner),
            encode_path_component(name)
        );
        self.post_json(&path, &serde_json::to_value(new)?)
    }

    /// Flag `346-releases`: `POST /repos/{owner}/{name}/releases/generate-notes`,
    /// GitHub's generated release notes for `tag_name` (created at
    /// `target_commitish` when it does not exist yet) since
    /// `previous_tag_name` (GitHub picks one without it). `Ok(None)` when
    /// the host cannot generate notes (GitHub Enterprise Server before 3.5
    /// answers 404; a tag GitHub cannot resolve, 422), so the caller can
    /// fall back to the local history.
    pub fn generate_release_notes(
        &self,
        owner: &str,
        name: &str,
        tag_name: &str,
        target_commitish: Option<&str>,
        previous_tag_name: Option<&str>,
    ) -> Result<Option<GeneratedNotes>> {
        let path = format!(
            "repos/{}/{}/releases/generate-notes",
            encode_path_component(owner),
            encode_path_component(name)
        );
        let mut body = serde_json::json!({ "tag_name": tag_name });
        if let Some(target) = target_commitish {
            body["target_commitish"] = serde_json::Value::String(target.to_string());
        }
        if let Some(previous) = previous_tag_name {
            body["previous_tag_name"] = serde_json::Value::String(previous.to_string());
        }
        match self.post_json::<GeneratedNotes>(&path, &body) {
            Ok(notes) => Ok(Some(notes)),
            Err(GitHubError::Api {
                status: 404 | 422, ..
            }) => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// `fetchUser`: `GET /users/{login}`; `None` when there is no such user.
    pub fn user(&self, login: &str) -> Result<Option<ApiIdentity>> {
        match self.get_json::<ApiIdentity>(&format!("users/{login}")) {
            Ok(user) => Ok(Some(user)),
            Err(GitHubError::Api { status: 404, .. }) => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// `fetchMentionables`: `GET /repos/{owner}/{name}/mentionables/users`
    /// (preview API; needs its own `Accept`). `None` when the repository has
    /// no mentionables endpoint (404 for repositories the token can't see).
    pub fn mentionables(&self, owner: &str, name: &str) -> Result<Option<Vec<ApiMentionableUser>>> {
        let url = self
            .endpoint
            .api(&format!("repos/{owner}/{name}/mentionables/users"));
        debug!(%url, "GET");
        let mut response = self
            .agent
            .get(&url)
            .header("Accept", "application/vnd.github.jerry-maguire-preview")
            .header("Authorization", &format!("Bearer {}", self.token))
            .call()?;
        match response.status().as_u16() {
            404 => Ok(None),
            200..=299 => Ok(Some(response.body_mut().read_json()?)),
            _ => Err(self.api_error(&url, response)),
        }
    }

    /// `fetchPullRequest`: `GET /repos/{owner}/{name}/pulls/{number}`.
    pub fn pull_request(&self, owner: &str, name: &str, number: u64) -> Result<ApiPullRequest> {
        self.get_json(&format!("repos/{owner}/{name}/pulls/{number}"))
    }

    /// Corvene (`336-request-reviewers`): `GET /repos/{owner}/{name}/collaborators`,
    /// the logins of the users who can be asked for a review (at most 10
    /// pages of 100).
    pub fn collaborators(&self, owner: &str, name: &str) -> Result<Vec<String>> {
        let users: Vec<ApiOwner> = self.fetch_all(
            &format!("repos/{owner}/{name}/collaborators?per_page=100"),
            10,
            get_next_page_path_from_link,
            |_| true,
        )?;
        Ok(users.into_iter().map(|u| u.login).collect())
    }

    /// Corvene (`1314-code-owners`): `GET /user/teams`, the user's teams as
    /// CODEOWNERS names them (`@org/slug`; at most 10 pages of 100).
    pub fn user_teams(&self) -> Result<Vec<String>> {
        #[derive(Deserialize)]
        struct Team {
            slug: String,
            organization: ApiOwner,
        }
        let teams: Vec<Team> = self.fetch_all(
            "user/teams?per_page=100",
            10,
            get_next_page_path_from_link,
            |_| true,
        )?;
        Ok(teams
            .into_iter()
            .map(|t| format!("@{}/{}", t.organization.login, t.slug))
            .collect())
    }

    /// Corvene (`336-request-reviewers`): ask `add` for a review of pull
    /// request `number` (`POST …/pulls/{number}/requested_reviewers`) and
    /// withdraw the request from `remove` (`DELETE`, same path). A refusal
    /// (403, 422: not a collaborator, the author…) is an `Err` with
    /// GitHub's message.
    pub fn set_requested_reviewers(
        &self,
        owner: &str,
        name: &str,
        number: u64,
        add: &[String],
        remove: &[String],
    ) -> Result<()> {
        let path = format!("repos/{owner}/{name}/pulls/{number}/requested_reviewers");
        if !add.is_empty() {
            let _: serde_json::Value =
                self.post_json(&path, &serde_json::json!({ "reviewers": add }))?;
        }
        if !remove.is_empty() {
            let url = self.endpoint.api(&path);
            debug!(%url, "DELETE");
            let response = self
                .agent
                .delete(&url)
                .header("Accept", "application/vnd.github+json")
                .header("Authorization", &format!("Bearer {}", self.token))
                .header("X-GitHub-Api-Version", "2022-11-28")
                .force_send_body()
                .send_json(serde_json::json!({ "reviewers": remove }))?;
            if !response.status().is_success() {
                return Err(self.api_error(&url, response));
            }
        }
        Ok(())
    }

    /// `fetchAllOpenPullRequests`: every open pull request (at most 50
    /// pages), newest page first.
    pub fn open_pull_requests(&self, owner: &str, name: &str) -> Result<Vec<ApiPullRequest>> {
        self.fetch_all(
            &format!("repos/{owner}/{name}/pulls?state=open&per_page=100"),
            50,
            get_next_page_path_from_link,
            |_| true,
        )
    }

    /// `fetchUpdatedPullRequests`: pull requests (open and closed) updated
    /// at or after `since` (ISO-8601), most recently updated first. Pages
    /// start at 10 pull requests and grow
    /// ([`get_next_page_path_with_increasing_page_size`]). `None` once
    /// `max_results` came back with more to read (`MaxResultsError`): the
    /// caller refetches the open list instead.
    pub fn pull_requests_updated_since(
        &self,
        owner: &str,
        name: &str,
        since: &str,
        max_results: usize,
    ) -> Result<Option<Vec<ApiPullRequest>>> {
        let mut over_max = false;
        let prs: Vec<ApiPullRequest> = self.fetch_all(
            &format!(
                "repos/{owner}/{name}/pulls?state=all&sort=updated&direction=desc&per_page=10"
            ),
            usize::MAX,
            get_next_page_path_with_increasing_page_size,
            |prs: &[ApiPullRequest]| {
                if prs.len() >= max_results {
                    over_max = true;
                    return false;
                }
                // sorted by `updated_at`, newest first: a last one updated
                // after `since` means there may be more
                prs.last()
                    .is_some_and(|last| last.updated_at.as_str() > since)
            },
        )?;
        if over_max {
            return Ok(None);
        }
        Ok(Some(
            prs.into_iter()
                .filter(|pr| pr.updated_at.as_str() >= since)
                .collect(),
        ))
    }

    /// GHD `fetchProtectedBranches`: `GET repos/{owner}/{name}/branches?protected=true`,
    /// `None` when the request fails or the server answers with an error.
    /// GHD feeds only a usage metric with it (`updateBranchProtectionsFromAPI`
    /// for `commitsToRepositoryWithBranchProtections`), which Corvene does
    /// not collect, so nothing calls it.
    pub fn fetch_protected_branches(&self, owner: &str, name: &str) -> Option<Vec<ApiBranch>> {
        match self.get_json(&format!("repos/{owner}/{name}/branches?protected=true")) {
            Ok(branches) => Some(branches),
            Err(err) => {
                debug!(%err, "[fetchProtectedBranches] unable to list protected branches");
                None
            }
        }
    }

    /// `fetchCombinedRefStatus`: `GET /repos/{o}/{n}/commits/{ref}/status`.
    pub fn combined_ref_status(
        &self,
        owner: &str,
        name: &str,
        git_ref: &str,
    ) -> Result<Option<ApiRefStatus>> {
        let safe = encode_path_component(git_ref);
        self.get_json_opt(
            &format!("repos/{owner}/{name}/commits/{safe}/status?per_page=100"),
            "application/vnd.github+json",
        )
    }

    /// `fetchRefCheckRuns`: `GET /repos/{o}/{n}/commits/{ref}/check-runs`.
    /// GHD reads the first 100 only; `all_pages` follows `page=` until
    /// `total_count` runs are read (at most 1,000, desktop/desktop#18101).
    pub fn ref_check_runs(
        &self,
        owner: &str,
        name: &str,
        git_ref: &str,
        all_pages: bool,
    ) -> Result<Option<ApiRefCheckRuns>> {
        let safe = encode_path_component(git_ref);
        let mut out: Option<ApiRefCheckRuns> = None;
        for page in 1..=10u32 {
            let mut path = format!("repos/{owner}/{name}/commits/{safe}/check-runs?per_page=100");
            if page > 1 {
                path.push_str(&format!("&page={page}"));
            }
            let batch: Option<ApiRefCheckRuns> =
                self.get_json_opt(&path, "application/vnd.github.antiope-preview+json")?;
            let Some(batch) = batch else {
                break;
            };
            let short = batch.check_runs.len() < 100;
            let runs = out.get_or_insert_with(|| ApiRefCheckRuns {
                total_count: batch.total_count,
                check_runs: Vec::new(),
            });
            runs.check_runs.extend(batch.check_runs);
            if !all_pages || short || runs.check_runs.len() as u64 >= runs.total_count {
                break;
            }
        }
        Ok(out)
    }

    /// `fetchPRActionWorkflowRunByCheckSuiteId`
    pub fn workflow_run_by_check_suite(
        &self,
        owner: &str,
        name: &str,
        check_suite_id: u64,
    ) -> Result<Option<ApiWorkflowRun>> {
        let runs: Option<ApiWorkflowRuns> = self.get_json_opt(
            &format!(
                "repos/{owner}/{name}/actions/runs?event=pull_request&check_suite_id={check_suite_id}"
            ),
            "application/vnd.github.antiope-preview+json",
        )?;
        Ok(runs.and_then(|r| r.workflow_runs.into_iter().next()))
    }

    /// `fetchPRWorkflowRunsByBranchName`
    pub fn workflow_runs_by_branch(
        &self,
        owner: &str,
        name: &str,
        branch: &str,
    ) -> Result<Option<ApiWorkflowRuns>> {
        let safe = encode_path_component(branch);
        self.get_json_opt(
            &format!("repos/{owner}/{name}/actions/runs?event=pull_request&branch={safe}"),
            "application/vnd.github.antiope-preview+json",
        )
    }

    /// `fetchWorkflowRunJobs`
    pub fn workflow_run_jobs(
        &self,
        owner: &str,
        name: &str,
        run_id: u64,
    ) -> Result<Option<ApiWorkflowJobs>> {
        self.get_json_opt(
            &format!("repos/{owner}/{name}/actions/runs/{run_id}/jobs"),
            "application/vnd.github.antiope-preview+json",
        )
    }

    /// `GET /repos/{owner}/{name}/actions/jobs/{job_id}`: one job with its
    /// steps (Corvene `347-actions-job-logs`, for a check run the run's job
    /// list left out: an older attempt, or past its first page).
    pub fn workflow_job(
        &self,
        owner: &str,
        name: &str,
        job_id: u64,
    ) -> Result<Option<ApiWorkflowJob>> {
        self.get_json_opt(
            &format!("repos/{owner}/{name}/actions/jobs/{job_id}"),
            "application/vnd.github+json",
        )
    }

    /// `fetchCheckSuite`
    pub fn check_suite(
        &self,
        owner: &str,
        name: &str,
        check_suite_id: u64,
    ) -> Result<Option<ApiCheckSuite>> {
        self.get_json_opt(
            &format!("repos/{owner}/{name}/check-suites/{check_suite_id}"),
            "application/vnd.github+json",
        )
    }

    /// `rerequestCheckSuite`
    pub fn rerequest_check_suite(&self, owner: &str, name: &str, id: u64) -> Result<bool> {
        self.post_empty(&format!("repos/{owner}/{name}/check-suites/{id}/rerequest"))
    }

    /// Corvene (`347-actions-job-logs`): `GET
    /// repos/{owner}/{name}/actions/jobs/{job_id}/logs`, the job's plain text
    /// log. GitHub answers with a redirect to a short-lived download URL,
    /// which the agent follows without the token (ureq drops `Authorization`
    /// on a cross-host redirect). `None` when the log is gone (expired or
    /// not an Actions job). Not in GHD, which only links to the job page.
    pub fn job_logs(&self, owner: &str, name: &str, job_id: u64) -> Result<Option<String>> {
        let path = format!("repos/{owner}/{name}/actions/jobs/{job_id}/logs");
        let mut response = match self.get_response(&path, "application/vnd.github+json") {
            Ok(response) => response,
            Err(err) if err.is_token_invalidated() => return Err(err),
            Err(GitHubError::Api {
                status, message, ..
            }) => {
                debug!(status, %message, %path, "no job log");
                return Ok(None);
            }
            Err(err) => return Err(err),
        };
        let text = response
            .body_mut()
            .with_config()
            .limit(MAX_JOB_LOG_BYTES)
            .read_to_string()?;
        Ok(Some(text))
    }

    /// `rerunJob`
    pub fn rerun_job(&self, owner: &str, name: &str, job_id: u64) -> Result<bool> {
        self.post_empty(&format!("repos/{owner}/{name}/actions/jobs/{job_id}/rerun"))
    }

    /// `rerunFailedJobs`
    pub fn rerun_failed_jobs(&self, owner: &str, name: &str, run_id: u64) -> Result<bool> {
        self.post_empty(&format!(
            "repos/{owner}/{name}/actions/runs/{run_id}/rerun-failed-jobs"
        ))
    }

    /// `forkRepository`: `POST /repos/{o}/{n}/forks` (202 with the fork).
    pub fn fork_repository(&self, owner: &str, name: &str) -> Result<GitHubRepository> {
        let repo: ApiRepository = self.post_json(
            &format!("repos/{owner}/{name}/forks"),
            &serde_json::json!({}),
        )?;
        Ok(self.convert(repo))
    }

    /// `fetchPushControl`: whether the branch takes direct pushes. The
    /// defaults (pushable) come back when the endpoint has no answer.
    pub fn push_control(&self, owner: &str, name: &str, branch: &str) -> Result<ApiPushControl> {
        let safe = encode_path_component(branch);
        Ok(self
            .get_json_opt(
                &format!("repos/{owner}/{name}/branches/{safe}/push_control"),
                "application/vnd.github.phandalin-preview",
            )?
            .unwrap_or_default())
    }

    /// `GET repos/{owner}/{name}/branches/{branch}`: the branch as GitHub
    /// knows it (its `protected` mark), `None` when it is not there (Corvene
    /// `339-protected-branch-bypass-note`).
    pub fn branch(&self, owner: &str, name: &str, branch: &str) -> Result<Option<ApiBranch>> {
        let safe = encode_path_component(branch);
        self.get_json_opt(
            &format!("repos/{owner}/{name}/branches/{safe}"),
            "application/vnd.github+json",
        )
    }

    /// Corvene `341-custom-autolinks`: the repository's autolink references;
    /// `None` when the account may not read them (not an admin) or GitHub
    /// has none to give.
    pub fn autolinks(&self, owner: &str, name: &str) -> Result<Option<Vec<ApiAutolink>>> {
        self.get_json_opt(
            &format!("repos/{owner}/{name}/autolinks"),
            "application/vnd.github+json",
        )
    }

    /// `fetchAllRepoRulesets`
    pub fn repo_rulesets(
        &self,
        owner: &str,
        name: &str,
    ) -> Result<Option<Vec<ApiSlimRepoRuleset>>> {
        self.get_json_opt(
            &format!("repos/{owner}/{name}/rulesets"),
            "application/vnd.github+json",
        )
    }

    /// `fetchRepoRuleset`
    pub fn repo_ruleset(&self, owner: &str, name: &str, id: u64) -> Result<Option<ApiRepoRuleset>> {
        self.get_json_opt(
            &format!("repos/{owner}/{name}/rulesets/{id}"),
            "application/vnd.github+json",
        )
    }

    /// `fetchRepoRulesForBranch`
    pub fn repo_rules_for_branch(
        &self,
        owner: &str,
        name: &str,
        branch: &str,
    ) -> Result<Vec<ApiRepoRule>> {
        let safe = encode_path_component(branch);
        Ok(self
            .get_json_opt(
                &format!("repos/{owner}/{name}/rules/branches/{safe}"),
                "application/vnd.github+json",
            )?
            .unwrap_or_default())
    }

    /// `createPushProtectionBypass`
    pub fn create_push_protection_bypass(
        &self,
        owner: &str,
        name: &str,
        reason: BypassReason,
        placeholder_id: &str,
    ) -> Result<ApiPushProtectionBypass> {
        self.post_json(
            &format!("repos/{owner}/{name}/secret-scanning/push-protection-bypasses"),
            &serde_json::json!({
                "reason": reason.as_str(),
                "placeholder_id": placeholder_id,
            }),
        )
    }

    /// The API repository → model conversion (endpoint-aware).
    pub fn convert(&self, repo: ApiRepository) -> GitHubRepository {
        GitHubRepository {
            endpoint: self.endpoint.api_base.clone(),
            owner: repo.owner.login,
            name: repo.name,
            html_url: repo.html_url,
            clone_url: repo.clone_url,
            default_branch: repo.default_branch,
            private: repo.private,
            fork: repo.fork,
            parent: repo.parent.map(|p| Box::new(self.convert(*p))),
            archived: repo.archived,
            permissions: repo.permissions.and_then(|p| p.permission()),
            allow_forking: repo.allow_forking,
            node_id: repo.node_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sso_hints() {
        assert_eq!(
            sso_hint("required; url=https://github.com/orgs/acme/sso?authorization_request=AB12")
                .as_deref(),
            Some(
                "Re-authorize SSO for acme: \
                 https://github.com/orgs/acme/sso?authorization_request=AB12"
            )
        );
        assert_eq!(
            sso_hint("required; url=https://ghe.corp/sso/start").as_deref(),
            Some("Re-authorize SSO: https://ghe.corp/sso/start")
        );
        assert_eq!(
            sso_hint("partial-results; organizations=21955855,20582480"),
            None
        );
        assert_eq!(sso_hint("required"), None);
    }

    #[test]
    fn error_details_follow_the_message() {
        let body = r#"{"message":"Repository creation failed.","errors":[{"resource":"Repository","code":"custom","field":"description","message":"description is too long (maximum is 350 characters)"},{"resource":"Repository","code":"invalid","field":"name"}]}"#;
        let parsed = ApiErrorBody::parse(body.as_bytes()).unwrap();
        // GHD `APIError`: `errors.map(e => e.message).join(', ')`
        assert_eq!(
            parsed.message(false).as_deref(),
            Some(
                "Repository creation failed. (description is too long (maximum is 350 \
                 characters), )"
            )
        );
        assert_eq!(
            parsed.message(true).as_deref(),
            Some(
                "Repository creation failed. (description is too long (maximum is 350 \
                 characters), name invalid)"
            )
        );
        let bare = ApiErrorBody::parse(br#"{"message":"Not Found"}"#).unwrap();
        assert_eq!(bare.message(true).as_deref(), Some("Not Found"));
        assert_eq!(bare.message(false).as_deref(), Some("Not Found"));
        let only_errors =
            ApiErrorBody::parse(br#"{"errors":[{"field":"name","code":"missing"}]}"#).unwrap();
        assert_eq!(only_errors.message(false), None);
        assert_eq!(only_errors.message(true).as_deref(), Some("name missing"));
        assert_eq!(ApiErrorBody::parse(b""), None);
        assert_eq!(ApiErrorBody::parse(b"null"), None);
        assert_eq!(
            ApiErrorBody::parse(br#"{"message":"x","errors":"odd"}"#),
            Some(ApiErrorBody {
                message: Some("x".into()),
                errors: Vec::new(),
            })
        );
    }

    #[test]
    fn next_page_links() {
        let headers = |link: &str| vec![("link".to_string(), link.to_string())];
        assert_eq!(
            get_next_page_path_from_link(&headers(
                "<https://api.github.com/repositories/1/pulls?per_page=10&page=2>; rel=\"next\", \
                 <https://api.github.com/repositories/1/pulls?per_page=10&page=9>; rel=\"last\""
            ))
            .as_deref(),
            Some("/repositories/1/pulls?per_page=10&page=2")
        );
        assert_eq!(
            get_next_page_path_from_link(&headers(
                "<https://ghe.corp/api/v3/user/repos?page=1>; rel=\"prev\""
            )),
            None
        );
        assert_eq!(
            get_next_page_path_with_increasing_page_size(&headers(
                "<https://ghe.corp/api/v3/repos/o/n/pulls?state=all&per_page=40&page=3>; rel=\"next\""
            ))
            .as_deref(),
            Some("/api/v3/repos/o/n/pulls?state=all&per_page=80&page=2")
        );
        assert_eq!(parse_int(" 12abc"), Some(12));
        assert_eq!(parse_int("x"), None);
    }

    #[test]
    fn pull_request_review_deserializes() {
        let json = r#"{
            "id": 80,
            "node_id": "MDE3OlB1bGxSZXF1ZXN0UmV2aWV3ODA=",
            "user": {
                "login": "octocat",
                "id": 1,
                "avatar_url": "https://github.com/images/error/octocat_happy.gif",
                "html_url": "https://github.com/octocat",
                "type": "User"
            },
            "body": "Here is the body for the review.",
            "state": "CHANGES_REQUESTED",
            "html_url": "https://github.com/octocat/Hello-World/pull/12#pullrequestreview-80",
            "submitted_at": "2019-11-17T17:43:43Z",
            "commit_id": "ecdd80bb57125d7ba9641ffaa4d7d2c19d3f3091"
        }"#;
        let review: ApiPullRequestReview = serde_json::from_str(json).expect("review");
        assert_eq!(review.id, 80);
        assert_eq!(review.state, ApiPullRequestReviewState::ChangesRequested);
        assert_eq!(review.user.login, "octocat");
        assert_eq!(
            review.user.html_url.as_deref(),
            Some("https://github.com/octocat")
        );
        assert_eq!(review.user.kind.as_deref(), Some("User"));
        assert_eq!(review.submitted_at, "2019-11-17T17:43:43Z");

        let approved = json
            .replace("CHANGES_REQUESTED", "APPROVED")
            .replace(r#""Here is the body for the review.""#, "null");
        let review: ApiPullRequestReview = serde_json::from_str(&approved).expect("approved");
        assert_eq!(review.state, ApiPullRequestReviewState::Approved);
        assert_eq!(review.body, "");
        for (raw, state) in [
            ("COMMENTED", ApiPullRequestReviewState::Commented),
            ("DISMISSED", ApiPullRequestReviewState::Dismissed),
            ("PENDING", ApiPullRequestReviewState::Pending),
        ] {
            let parsed: ApiPullRequestReviewState =
                serde_json::from_str(&format!("\"{raw}\"")).expect("state");
            assert_eq!(parsed, state);
        }
    }

    #[test]
    fn repository_permissions_map_like_get_permissions_string() {
        let perms = |json: &str| -> Option<RepositoryPermission> {
            serde_json::from_str::<ApiRepositoryPermissions>(json)
                .expect("permissions")
                .permission()
        };
        assert_eq!(
            perms(r#"{"admin":true,"push":true,"pull":true}"#),
            Some(RepositoryPermission::Admin)
        );
        assert_eq!(
            perms(r#"{"admin":false,"push":true,"pull":true,"maintain":true}"#),
            Some(RepositoryPermission::Write)
        );
        assert_eq!(
            perms(r#"{"admin":false,"push":false,"pull":true}"#),
            Some(RepositoryPermission::Read)
        );
        assert_eq!(perms(r#"{}"#), None);

        let repo: ApiRepository = serde_json::from_str(
            r#"{"name":"desktop","owner":{"login":"desktop"},"html_url":"https://github.com/desktop/desktop",
                "clone_url":"https://github.com/desktop/desktop.git","default_branch":"development",
                "permissions":{"admin":false,"push":false,"pull":true}}"#,
        )
        .expect("repository");
        let client = Client::new(crate::Endpoint::github_com(), "");
        let converted = client.convert(repo);
        assert_eq!(converted.permissions, Some(RepositoryPermission::Read));
        assert!(!converted.has_write_permission());
    }

    #[test]
    fn issue_comment_deserializes() {
        let json = r#"{
            "id": 1,
            "node_id": "MDEyOklzc3VlQ29tbWVudDE=",
            "url": "https://api.github.com/repos/octocat/Hello-World/issues/comments/1",
            "html_url": "https://github.com/octocat/Hello-World/issues/1347#issuecomment-1",
            "body": "Me too",
            "user": { "login": "octocat", "id": 1, "avatar_url": null, "html_url": "https://github.com/octocat", "type": "User" },
            "created_at": "2011-04-14T16:00:49Z",
            "updated_at": "2011-04-14T16:00:49Z",
            "author_association": "COLLABORATOR"
        }"#;
        let comment: ApiIssueComment = serde_json::from_str(json).expect("comment");
        assert_eq!(comment.id, 1);
        assert_eq!(comment.body, "Me too");
        assert_eq!(comment.user.login, "octocat");
        assert_eq!(comment.user.avatar_url, None);
        assert_eq!(comment.created_at, "2011-04-14T16:00:49Z");
        assert!(comment.html_url.ends_with("#issuecomment-1"));
    }
}
