//! Corvene `354-pull-request-event-notifications` (desktop/desktop#21474):
//! the REST reads behind notifications for pull request events on any
//! repository of an account. The notifications API (`GET /notifications`,
//! polled with `If-Modified-Since` and `X-Poll-Interval`) says which pull
//! requests changed; the issue search is the fallback for tokens it
//! refuses. GHD only learns of pull request events from Alive
//! (`lib/stores/alive-store.ts`), which pushes reviews, comments and failed
//! checks of the user's own pull requests.

use serde::Deserialize;
use tracing::debug;

use crate::api::{ApiIssueComment, ApiOwner, ApiPullRequestReview, ApiPullRequestReviewState};
use crate::api::{Client, encode_path_component};
use crate::error::Result;

/// The notification threads read per poll (one page, newest first).
const THREADS_PER_POLL: u32 = 50;

/// One notification thread (`GET /notifications`).
#[derive(Debug, Clone, Deserialize)]
pub struct ApiNotificationThread {
    pub id: String,
    /// `review_requested`, `mention`, `team_mention`, `author`,
    /// `comment`, `state_change`, `subscribed`…
    pub reason: String,
    pub updated_at: String,
    pub subject: ApiNotificationSubject,
    pub repository: ApiNotificationRepository,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiNotificationSubject {
    pub title: String,
    /// The pull request's API URL.
    #[serde(default)]
    pub url: Option<String>,
    /// The API URL of the thread's latest comment (the pull request itself
    /// when it has none).
    #[serde(default)]
    pub latest_comment_url: Option<String>,
    /// `PullRequest`, `Issue`, `Commit`, `Release`…
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiNotificationRepository {
    pub name: String,
    pub owner: ApiOwner,
}

/// What one poll of the notifications API found.
#[derive(Debug, Clone, Default)]
pub struct NotificationsPoll {
    /// `None` when nothing changed since `If-Modified-Since` (a 304).
    pub threads: Option<Vec<ApiNotificationThread>>,
    /// `Last-Modified`: the next poll's `If-Modified-Since`.
    pub last_modified: Option<String>,
    /// `X-Poll-Interval`: the fewest seconds until the next poll.
    pub poll_interval: Option<u64>,
    /// `Date`: the server's clock, the watermark of the next poll.
    pub date: Option<String>,
}

/// One pull request of an issue search (`GET /search/issues`).
#[derive(Debug, Clone, Deserialize)]
pub struct ApiSearchPullRequest {
    pub number: u64,
    pub title: String,
    pub updated_at: String,
    /// `https://api.github.com/repos/{owner}/{name}`
    pub repository_url: String,
    pub user: ApiOwner,
    /// Corvene (`433-macos-widgets`)
    #[serde(default)]
    pub html_url: Option<String>,
}

impl ApiSearchPullRequest {
    /// The owner and name of the repository, from `repository_url`.
    pub fn repository(&self) -> Option<(String, String)> {
        let rest = self.repository_url.split("/repos/").nth(1)?;
        let (owner, name) = rest.split_once('/')?;
        Some((owner.to_string(), name.trim_end_matches('/').to_string()))
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ApiSearchResults<T> {
    #[serde(default = "Vec::new")]
    items: Vec<T>,
}

/// A review in a pull request's list: pending reviews have no
/// `submitted_at` yet.
#[derive(Debug, Clone, Deserialize)]
struct ApiListedReview {
    id: u64,
    user: Option<crate::api::ApiIdentity>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    submitted_at: Option<String>,
    state: ApiPullRequestReviewState,
}

/// The value of header `name` (case-insensitive) in `headers`.
pub fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

impl Client {
    /// `GET /notifications` (unread threads the account is subscribed to,
    /// newest first), conditional on `if_modified_since`.
    pub fn notifications(&self, if_modified_since: Option<&str>) -> Result<NotificationsPoll> {
        let (threads, headers) = self.get_json_if_modified_since::<Vec<ApiNotificationThread>>(
            &format!("notifications?per_page={THREADS_PER_POLL}"),
            if_modified_since,
        )?;
        Ok(NotificationsPoll {
            threads,
            last_modified: header_value(&headers, "Last-Modified").map(str::to_string),
            poll_interval: header_value(&headers, "X-Poll-Interval").and_then(|v| v.parse().ok()),
            date: header_value(&headers, "Date").map(str::to_string),
        })
    }

    /// `GET /search/issues?q=…` for pull requests, most recently updated
    /// first (one page of 50).
    pub fn search_pull_requests(&self, query: &str) -> Result<Vec<ApiSearchPullRequest>> {
        let q = query
            .split(' ')
            .map(encode_path_component)
            .collect::<Vec<_>>()
            .join("+");
        let results: ApiSearchResults<ApiSearchPullRequest> = self.get_json(&format!(
            "search/issues?q={q}&sort=updated&order=desc&per_page=50"
        ))?;
        Ok(results.items)
    }

    /// `GET /repos/{owner}/{name}/pulls/{number}/reviews`: the submitted
    /// reviews, oldest first (at most 100).
    pub fn pull_request_reviews(
        &self,
        owner: &str,
        name: &str,
        number: u64,
    ) -> Result<Vec<ApiPullRequestReview>> {
        let listed: Vec<ApiListedReview> = self.get_json(&format!(
            "repos/{}/{}/pulls/{number}/reviews?per_page=100",
            encode_path_component(owner),
            encode_path_component(name),
        ))?;
        Ok(listed
            .into_iter()
            .filter_map(|r| {
                Some(ApiPullRequestReview {
                    id: r.id,
                    user: r.user?,
                    body: r.body.unwrap_or_default(),
                    html_url: r.html_url,
                    submitted_at: r.submitted_at?,
                    state: r.state,
                })
            })
            .collect())
    }

    /// `GET /repos/{owner}/{name}/issues/{number}/comments?since=…`: the
    /// conversation comments updated since `since` (ISO-8601; at most 100).
    pub fn issue_comments_since(
        &self,
        owner: &str,
        name: &str,
        number: u64,
        since: &str,
    ) -> Result<Vec<ApiIssueComment>> {
        self.get_json(&format!(
            "repos/{}/{}/issues/{number}/comments?since={}&per_page=100",
            encode_path_component(owner),
            encode_path_component(name),
            encode_path_component(since),
        ))
    }

    /// The comment behind a notification's `latest_comment_url`: an issue
    /// or review comment of this client's endpoint (`None` for other
    /// hosts, the pull request itself or a comment that is gone).
    pub fn comment_at(&self, url: &str) -> Result<Option<ApiIssueComment>> {
        let Some(path) = self.api_path_of(url) else {
            return Ok(None);
        };
        if !path.contains("/comments/") {
            return Ok(None);
        }
        match self.get_json::<ApiIssueComment>(&path) {
            Ok(comment) => Ok(Some(comment)),
            Err(err) if err.is_token_invalidated() => Err(err),
            Err(err) => {
                debug!(%err, %url, "comment not available");
                Ok(None)
            }
        }
    }

    /// `url` relative to this client's API base, `None` when it points
    /// elsewhere (the token is never sent to another host).
    pub fn api_path_of(&self, url: &str) -> Option<String> {
        let base = self.endpoint().api("");
        url.strip_prefix(&base).map(str::to_string)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_notification_thread() {
        let json = r#"[{
            "id": "1",
            "reason": "review_requested",
            "updated_at": "2026-10-06T10:00:00Z",
            "unread": true,
            "subject": {
                "title": "Add a thing",
                "url": "https://api.github.com/repos/octo/hello/pulls/7",
                "latest_comment_url": null,
                "type": "PullRequest"
            },
            "repository": {"name": "hello", "full_name": "octo/hello", "owner": {"login": "octo"}}
        }]"#;
        let threads: Vec<ApiNotificationThread> = serde_json::from_str(json).unwrap();
        assert_eq!(threads[0].subject.kind, "PullRequest");
        assert_eq!(threads[0].repository.owner.login, "octo");
        assert!(threads[0].subject.latest_comment_url.is_none());
    }

    #[test]
    fn search_items_name_their_repository() {
        let item = ApiSearchPullRequest {
            html_url: None,
            number: 7,
            title: "t".into(),
            updated_at: String::new(),
            repository_url: "https://ghe.corp/api/v3/repos/octo/hello".into(),
            user: ApiOwner {
                login: "octo".into(),
            },
        };
        assert_eq!(item.repository(), Some(("octo".into(), "hello".into())));
    }

    #[test]
    fn api_paths_stay_on_the_endpoint() {
        let client = Client::new(
            crate::Endpoint::from_api_base("https://api.github.com"),
            String::new(),
        );
        assert_eq!(
            client
                .api_path_of("https://api.github.com/repos/o/r/issues/comments/5")
                .as_deref(),
            Some("repos/o/r/issues/comments/5")
        );
        assert_eq!(
            client.api_path_of("https://evil.example/repos/o/r/issues/comments/5"),
            None
        );
    }
}
