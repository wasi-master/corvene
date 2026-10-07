//! GitHub API + OAuth device flow. Blocking `ureq` calls;
//! run them on a background thread.

pub mod actions;
pub mod alive;
pub mod api;
pub mod auth;
pub mod endpoint;
pub mod error;
pub mod pull_request_events;
pub mod review;
pub mod signatures;

pub use api::{
    ApiBranch, ApiCheckSuite, ApiIdentity, ApiIssue, ApiIssueComment, ApiIssueUser, ApiLabel,
    ApiMentionableUser, ApiPullRequest, ApiPullRequestReview, ApiPullRequestReviewState,
    ApiPushControl, ApiReactions, ApiRefCheckRun, ApiRefStatus, ApiRelease, ApiReleaseAsset,
    ApiRepoRule, ApiRepoRuleset, ApiWorkflowJob, ApiWorkflowRun, Client, GeneratedNotes,
    ISSUE_COMMENTS_PAGE, IssueState, IssueUpdate, NewIssue, NewRelease, RepositoryCloneInfo,
    encode_path_component, get_next_page_path_from_link,
    get_next_page_path_with_increasing_page_size,
};
pub use endpoint::{Endpoint, EnterpriseAddressError};
pub use error::{ApiErrorBody, ApiErrorItem, GitHubError, Result};

/// OAuth App "Corvene" (public identifier; device flow needs no secret).
/// Override at build time with `CORVENE_GITHUB_CLIENT_ID`.
pub const CLIENT_ID: &str = match option_env!("CORVENE_GITHUB_CLIENT_ID") {
    Some(id) => id,
    None => "Ov23li8x4b9tpSBwDEOy",
};

/// Scopes GitHub Desktop requests, plus `read:user`/`user:email` for the account card.
pub const SCOPES: &str = "repo workflow read:user user:email";

/// Corvene (`350-ssh-key-helper`): the scope `POST /user/keys` needs, asked
/// for only when adding an SSH key.
pub const PUBLIC_KEY_SCOPE: &str = "write:public_key";

/// [`SCOPES`] and `extra` (a sign-in that asks for more), each once.
pub fn scopes_with(extra: &[String]) -> String {
    let mut scopes: Vec<&str> = SCOPES.split(' ').collect();
    for scope in extra {
        if !scopes.contains(&scope.as_str()) {
            scopes.push(scope);
        }
    }
    scopes.join(" ")
}

/// A scope list as GitHub writes it (`X-OAuth-Scopes`, the token answer's
/// `scope`): separated by commas or spaces.
pub fn parse_scopes(text: &str) -> Vec<String> {
    text.split([',', ' '])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether `scopes` allow adding an SSH key (`admin:public_key` includes
/// `write:public_key`).
pub fn allows_adding_ssh_keys(scopes: &[String]) -> bool {
    scopes
        .iter()
        .any(|s| s == PUBLIC_KEY_SCOPE || s == "admin:public_key")
}

/// The OAuth app's client secret for the browser flow's token exchange
/// (GHD `ClientSecret`); `None` when the build has none - the exchange then
/// relies on PKCE alone. Set with `CORVENE_GITHUB_CLIENT_SECRET` at build time.
pub const CLIENT_SECRET: Option<&str> = option_env!("CORVENE_GITHUB_CLIENT_SECRET");

/// OAuth apps registered on GitHub Enterprise hosts, baked in at build time
/// with `CORVENE_GHES_OAUTH` as `host=client_id[:client_secret],…`
/// (e.g. `ghe.corp=Iv1.0123456789abcdef`). A GHES server does not know
/// Corvene's github.com app, so signing in with OAuth there needs an app its
/// administrator registered; without one the host stays PAT-only.
pub const ENTERPRISE_OAUTH_APPS: Option<&str> = option_env!("CORVENE_GHES_OAUTH");

/// The OAuth app a sign-in authenticates as: Corvene's own on GitHub.com,
/// an administrator-registered one on a GitHub Enterprise host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthApp {
    pub client_id: String,
    /// Needed by the browser flow's token exchange unless the app accepts
    /// PKCE alone; the device flow never sends it.
    pub client_secret: Option<String>,
}

impl OAuthApp {
    /// Corvene's github.com app (`CLIENT_ID` / `CLIENT_SECRET`).
    pub fn dotcom() -> Self {
        Self {
            client_id: CLIENT_ID.to_string(),
            client_secret: CLIENT_SECRET.map(str::to_string),
        }
    }

    /// The app this build knows for `endpoint`: Corvene's on GitHub.com,
    /// the `CORVENE_GHES_OAUTH` entry for an Enterprise host.
    pub fn built_in(endpoint: &Endpoint) -> Option<Self> {
        if endpoint.is_dotcom() {
            return Some(Self::dotcom());
        }
        Self::from_spec(ENTERPRISE_OAUTH_APPS?, endpoint.host())
    }

    /// `host`'s entry in a `host=client_id[:client_secret],…` list.
    pub fn from_spec(spec: &str, host: &str) -> Option<Self> {
        spec.split(',').find_map(|entry| {
            let (entry_host, app) = entry.trim().split_once('=')?;
            if !entry_host.trim().eq_ignore_ascii_case(host) {
                return None;
            }
            let (client_id, client_secret) = match app.split_once(':') {
                Some((id, secret)) => (id.trim(), Some(secret.trim())),
                None => (app.trim(), None),
            };
            (!client_id.is_empty()).then(|| Self {
                client_id: client_id.to_string(),
                client_secret: client_secret.filter(|s| !s.is_empty()).map(str::to_string),
            })
        })
    }
}

pub const USER_AGENT: &str = concat!("Corvene/", env!("CARGO_PKG_VERSION"));

/// Plain GET of a small binary resource (avatars); 5 s per phase, no auth.
pub fn download(url: &str) -> Result<Vec<u8>> {
    let agent = corvene_platform::proxy::agent(
        ureq::Agent::config_builder()
            // the system proxy, PAC scripts and saved proxy credentials
            // (`corvene_platform::proxy`)
            .middleware(corvene_platform::proxy::per_request)
            .timeout_global(Some(std::time::Duration::from_secs(10)))
            .user_agent(USER_AGENT)
            .build(),
    );
    let mut response = agent.get(url).call()?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(GitHubError::api(status, url));
    }
    let bytes = response.body_mut().read_to_vec()?;
    Ok(bytes)
}

/// `GET /emojis` (no authentication needed on GitHub.com): emoji name → image URL.
pub fn public_emojis(endpoint: &Endpoint) -> Result<std::collections::HashMap<String, String>> {
    let agent = corvene_platform::proxy::agent(
        ureq::Agent::config_builder()
            // the system proxy, PAC scripts and saved proxy credentials
            // (`corvene_platform::proxy`)
            .middleware(corvene_platform::proxy::per_request)
            .timeout_global(Some(std::time::Duration::from_secs(15)))
            .user_agent(USER_AGENT)
            .build(),
    );
    let url = endpoint.api("emojis");
    let mut response = agent
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(GitHubError::api(status, url));
    }
    Ok(response.body_mut().read_json()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_scopes_once() {
        assert_eq!(scopes_with(&[]), SCOPES);
        assert_eq!(
            scopes_with(&["write:public_key".into(), "repo".into()]),
            "repo workflow read:user user:email write:public_key"
        );
    }

    #[test]
    fn reads_scope_lists() {
        let scopes = parse_scopes("repo, workflow,admin:public_key");
        assert_eq!(scopes, ["repo", "workflow", "admin:public_key"]);
        assert!(allows_adding_ssh_keys(&scopes));
        assert!(!allows_adding_ssh_keys(&parse_scopes("repo workflow")));
    }

    #[test]
    fn enterprise_oauth_spec() {
        let spec = "ghe.corp=Iv1.abc:s3cret, other.example = Ov23xyz ,bad";
        assert_eq!(
            OAuthApp::from_spec(spec, "GHE.corp"),
            Some(OAuthApp {
                client_id: "Iv1.abc".into(),
                client_secret: Some("s3cret".into()),
            })
        );
        assert_eq!(
            OAuthApp::from_spec(spec, "other.example"),
            Some(OAuthApp {
                client_id: "Ov23xyz".into(),
                client_secret: None,
            })
        );
        assert_eq!(OAuthApp::from_spec(spec, "bad"), None);
        assert_eq!(OAuthApp::from_spec("x.y=", "x.y"), None);
        assert_eq!(
            OAuthApp::built_in(&Endpoint::github_com()),
            Some(OAuthApp::dotcom())
        );
    }
}
