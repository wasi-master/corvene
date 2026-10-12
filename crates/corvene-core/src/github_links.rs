//! Corvene `355-github-links-in-app`: links to a repository's pages on
//! GitHub (in release notes, issues, pull request descriptions) that the
//! app can show itself when the repository is cloned: a release, a
//! comparison, a commit, an issue, a pull request (in the review view,
//! `348-pull-request-review`, when it is among the open ones; an issue
//! link to one shows it there too, as GitHub redirects it).
//!
//! GitHub Desktop opens every link in the browser
//! (`app/src/ui/lib/link-button.tsx`, `shell.openExternal`).

use crate::dispatcher::Dispatcher;
use crate::host::Host;
use crate::ref_compare::RefRange;
use crate::remote::spawn_bg;
use crate::state::AppState;
use corvene_models::PullRequest;

/// What a GitHub link points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitHubLinkTarget {
    /// `/releases/tag/<tag>`
    Release(String),
    /// `/compare/<base>...<head>` (`symmetric`) or `<base>..<head>`.
    Compare {
        base: String,
        head: String,
        symmetric: bool,
    },
    /// `/commit/<sha>`
    Commit(String),
    /// `/issues/<number>`
    Issue(u64),
    /// `/pull/<number>`
    PullRequest(u64),
}

/// A parsed link to a page of `owner/name` on `host`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubLink {
    pub host: String,
    pub owner: String,
    pub name: String,
    pub target: GitHubLinkTarget,
}

impl GitHubLink {
    /// What the in-app menu item says it opens.
    pub fn description(&self) -> String {
        match &self.target {
            GitHubLinkTarget::Release(tag) => format!("Release {tag}"),
            GitHubLinkTarget::Compare {
                base,
                head,
                symmetric,
            } => format!(
                "Comparison {base}{}{head}",
                if *symmetric { "..." } else { ".." }
            ),
            GitHubLinkTarget::Commit(sha) => format!("Commit {}", &sha[..sha.len().min(7)]),
            GitHubLinkTarget::Issue(number) => format!("Issue #{number}"),
            GitHubLinkTarget::PullRequest(number) => format!("Pull Request #{number}"),
        }
    }
}

/// Percent-decode a path segment (`%2F` in branch names and tags).
fn decode(segment: &str) -> String {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(&segment[i + 1..i + 3], 16)
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `https://<host>/<owner>/<name>/<page>…` → [`GitHubLink`], for the pages
/// the app can show. Query strings and fragments are ignored.
pub fn parse_github_link(url: &str) -> Option<GitHubLink> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let rest = rest.split(['?', '#']).next()?;
    let (host, path) = rest.split_once('/')?;
    let host = host.to_ascii_lowercase();
    let mut parts = path.trim_end_matches('/').splitn(4, '/');
    let owner = parts.next().filter(|s| !s.is_empty())?.to_string();
    let name = parts.next().filter(|s| !s.is_empty())?.to_string();
    let page = parts.next()?;
    let rest = parts.next().filter(|s| !s.is_empty())?;
    let target = match page {
        "releases" => GitHubLinkTarget::Release(decode(rest.strip_prefix("tag/")?)),
        "compare" => {
            // `owner:branch` heads come from forks: the branch is the part
            // after the colon
            let strip_owner = |r: &str| decode(r.rsplit(':').next().unwrap_or(r));
            if let Some((base, head)) = rest.split_once("...") {
                GitHubLinkTarget::Compare {
                    base: strip_owner(base),
                    head: strip_owner(head),
                    symmetric: true,
                }
            } else {
                let (base, head) = rest.split_once("..")?;
                GitHubLinkTarget::Compare {
                    base: strip_owner(base),
                    head: strip_owner(head),
                    symmetric: false,
                }
            }
        }
        "commit" => {
            let sha = rest.split('/').next()?;
            if sha.len() < 7 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            GitHubLinkTarget::Commit(sha.to_ascii_lowercase())
        }
        "issues" => GitHubLinkTarget::Issue(rest.split('/').next()?.parse().ok()?),
        "pull" => GitHubLinkTarget::PullRequest(rest.split('/').next()?.parse().ok()?),
        _ => return None,
    };
    if let GitHubLinkTarget::Compare { base, head, .. } = &target
        && (base.is_empty() || head.is_empty())
    {
        return None;
    }
    Some(GitHubLink {
        host,
        owner,
        name,
        target,
    })
}

/// The host of a repository page URL (`https://github.com/o/n`).
fn html_host(html_url: &str) -> Option<String> {
    let rest = html_url
        .strip_prefix("https://")
        .or_else(|| html_url.strip_prefix("http://"))?;
    Some(rest.split('/').next()?.to_ascii_lowercase())
}

/// The repository `link` belongs to among those added, the selected one
/// first. `None` when the flag is off, the page is one the app cannot show
/// with its flags, or the repository is not here.
pub fn repository_for_link(s: &AppState, link: &GitHubLink) -> Option<u64> {
    use crate::flags::ids;
    if !s.flags.bool(ids::GITHUB_LINKS_IN_APP) {
        return None;
    }
    let same = |gh: &corvene_models::GitHubRepository| {
        gh.owner.eq_ignore_ascii_case(&link.owner)
            && gh.name.eq_ignore_ascii_case(&link.name)
            && html_host(&gh.html_url).is_some_and(|h| h == link.host)
    };
    // a fork's issues and pull requests are its parent's
    let upstream = matches!(
        link.target,
        GitHubLinkTarget::Issue(_) | GitHubLinkTarget::PullRequest(_)
    );
    let matches = |id: u64| {
        let shown = match link.target {
            GitHubLinkTarget::Release(_) => s.flags.bool(ids::RELEASES),
            GitHubLinkTarget::Compare { .. } => crate::ref_compare::enabled(s),
            GitHubLinkTarget::Commit(_) => true,
            GitHubLinkTarget::Issue(number) => {
                s.flags.bool(ids::ISSUES) || reviewable_pull_request(s, id, number).is_some()
            }
            GitHubLinkTarget::PullRequest(number) => {
                reviewable_pull_request(s, id, number).is_some()
            }
        };
        shown
            && s.repository(id)
                .and_then(|r| r.github.as_ref())
                .is_some_and(|gh| same(gh) || (upstream && gh.parent.as_deref().is_some_and(same)))
    };
    s.selected
        .filter(|&id| matches(id))
        .or_else(|| s.repositories.iter().map(|r| r.id).find(|&id| matches(id)))
}

/// Open pull request `number` of repository `id` when the review view
/// (`348-pull-request-review`) can show it.
fn reviewable_pull_request(s: &AppState, id: u64, number: u64) -> Option<&PullRequest> {
    if !s.flags.bool(crate::flags::ids::PULL_REQUEST_REVIEW) {
        return None;
    }
    s.pull_requests_for(id)
        .iter()
        .find(|pr| pr.number == number)
}

/// What the in-app menu item for `link` in repository `id` says it opens:
/// an issue link to an open pull request names the pull request.
pub fn link_description(s: &AppState, id: u64, link: &GitHubLink) -> String {
    match link.target {
        GitHubLinkTarget::Issue(number) if reviewable_pull_request(s, id, number).is_some() => {
            format!("Pull Request #{number}")
        }
        _ => link.description(),
    }
}

impl Dispatcher {
    /// Show `link` in repository `id` (selected first): the release in
    /// Releases, the comparison in History, the commit in History, the issue
    /// in Issues. A comparison whose refs are not here opens in the browser.
    pub fn open_github_link(id: u64, link: GitHubLink, url: String, cx: &mut dyn Host) {
        if Self::state(cx).read(cx).selected != Some(id) {
            Self::select_repository(id, cx);
        }
        match link.target {
            GitHubLinkTarget::Release(tag) => Self::show_release_tag(id, tag, cx),
            GitHubLinkTarget::Commit(sha) => Self::reveal_commit(id, sha, cx),
            GitHubLinkTarget::Issue(number) | GitHubLinkTarget::PullRequest(number) => {
                let pr = reviewable_pull_request(Self::state(cx).read(cx), id, number).cloned();
                match pr {
                    Some(pr) => Self::review_pull_request(id, pr, cx),
                    None if matches!(link.target, GitHubLinkTarget::Issue(_)) => {
                        Self::show_issues(id, cx);
                        Self::select_issue(id, Some(number), cx);
                    }
                    None => Self::open_url(&url, cx),
                }
            }
            GitHubLinkTarget::Compare {
                base,
                head,
                symmetric,
            } => {
                let Some((_, workdir)) = Self::repo_context(id, cx) else {
                    return;
                };
                let remotes: Vec<String> = Self::state(cx)
                    .read(cx)
                    .repo_states
                    .get(&id)
                    .and_then(|rs| rs.info.as_ref())
                    .map(|i| {
                        // origin first: where a GitHub page's branches live
                        let mut names: Vec<String> =
                            i.remotes.iter().map(|r| r.name.clone()).collect();
                        names.sort_by_key(|n| n != "origin");
                        names
                    })
                    .unwrap_or_default();
                spawn_bg(
                    cx,
                    move || {
                        // a tag or local branch as is, else a remote's branch
                        let resolve = |name: &str| {
                            std::iter::once(name.to_string())
                                .chain(remotes.iter().map(|r| format!("{r}/{name}")))
                                .find(|rev| {
                                    corvene_git::resolve_commit(&workdir, rev)
                                        .ok()
                                        .flatten()
                                        .is_some()
                                })
                        };
                        Some((resolve(&base)?, resolve(&head)?))
                    },
                    move |refs, cx| match refs {
                        Some((base, head)) => {
                            let range = if symmetric {
                                RefRange::Symmetric
                            } else {
                                RefRange::Range
                            };
                            Self::compare_refs(id, base, head, range, false, cx)
                        }
                        None => Self::open_url(&url, cx),
                    },
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(url: &str) -> Option<GitHubLinkTarget> {
        parse_github_link(url).map(|l| l.target)
    }

    #[test]
    fn parses_pages() {
        let link =
            parse_github_link("https://github.com/wasi-master/wmkeyboard/releases/tag/v0.5.13")
                .unwrap();
        assert_eq!(link.host, "github.com");
        assert_eq!(
            (link.owner.as_str(), link.name.as_str()),
            ("wasi-master", "wmkeyboard")
        );
        assert_eq!(link.target, GitHubLinkTarget::Release("v0.5.13".into()));
        assert_eq!(
            target("https://github.com/o/n/compare/v0.5.13...v0.5.14"),
            Some(GitHubLinkTarget::Compare {
                base: "v0.5.13".into(),
                head: "v0.5.14".into(),
                symmetric: true
            })
        );
        assert_eq!(
            target("https://github.com/o/n/compare/main..fork:feature%2Fx"),
            Some(GitHubLinkTarget::Compare {
                base: "main".into(),
                head: "feature/x".into(),
                symmetric: false
            })
        );
        assert_eq!(
            target("https://github.com/o/n/commit/ABCDEF1234?diff=split#x"),
            Some(GitHubLinkTarget::Commit("abcdef1234".into()))
        );
        assert_eq!(
            target("https://github.com/o/n/issues/12#issuecomment-1"),
            Some(GitHubLinkTarget::Issue(12))
        );
        assert_eq!(
            target("https://github.com/o/n/pull/3/files"),
            Some(GitHubLinkTarget::PullRequest(3))
        );
    }

    #[test]
    fn ignores_other_links() {
        for url in [
            "https://github.com/o/n",
            "https://github.com/o/n/releases",
            "https://github.com/o/n/releases/latest",
            "https://github.com/o/n/pull/new",
            "https://github.com/o/n/commit/xyz",
            "https://github.com/o/n/compare/main",
            "https://github.com/o/n/issues/new",
            "mailto:a@b.c",
        ] {
            assert_eq!(target(url), None, "{url}");
        }
    }
}
