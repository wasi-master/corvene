//! Corvene `346-releases`: Repository › Releases…, the GitHub repository's
//! releases as a History mode, and Create Release… GitHub Desktop has no
//! releases at all (they are made on github.com).
//!
//! [`Dispatcher::show_releases`] swaps History's commit list for the list
//! ([`ReleasesViewState`]), read through the REST API with the account for
//! the repository's endpoint; the selected release's notes and assets take
//! the commit view's place. Create Release… (`Popup::CreateRelease`, from a
//! tagged commit's menu in History, a tag in the branch list or the view)
//! takes an existing tag or a new one at a commit, a title, notes and the
//! draft / pre-release switches. Generate release notes asks GitHub
//! (`generate-notes`, since the tag before the target) and, on a GitHub
//! Enterprise Server that cannot (before 3.5), lists the commits since that
//! tag from the local history instead ([`local_release_notes`]). A tag that
//! exists locally but is not on GitHub is pushed first; a new tag needs its
//! commit on GitHub, so the branch has to be pushed first.

use corvene_github::{ApiRelease, Client, NewRelease};
use corvene_models::{Commit, GitHubRepository, Section};
use tracing::info;

use crate::dispatcher::Dispatcher;
use crate::host::Host;
use crate::mco::Banner;
use crate::remote::spawn_bg;
use crate::state::{AppState, Popup, RepositoryState};

/// Pages of 50 releases read.
pub const RELEASE_PAGES: usize = 4;
/// Commits the local notes list at most.
pub const LOCAL_NOTES_LIMIT: usize = 500;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseAsset {
    pub name: String,
    pub size: u64,
    pub url: String,
    pub downloads: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseRow {
    pub id: u64,
    pub tag_name: String,
    /// The title; empty when GitHub shows the tag instead.
    pub name: String,
    pub body: String,
    pub html_url: String,
    pub draft: bool,
    pub prerelease: bool,
    pub author: String,
    /// ISO-8601.
    pub created_at: Option<String>,
    pub published_at: Option<String>,
    pub target: Option<String>,
    pub assets: Vec<ReleaseAsset>,
    pub tarball_url: Option<String>,
}

impl ReleaseRow {
    pub fn from_api(release: ApiRelease) -> Self {
        Self {
            id: release.id,
            tag_name: release.tag_name,
            name: release.name.unwrap_or_default(),
            body: release.body.unwrap_or_default(),
            html_url: release.html_url,
            draft: release.draft,
            prerelease: release.prerelease,
            author: release.author.map(|u| u.login).unwrap_or_default(),
            created_at: release.created_at,
            published_at: release.published_at,
            target: release.target_commitish,
            assets: release
                .assets
                .into_iter()
                .map(|a| ReleaseAsset {
                    name: a.name,
                    size: a.size,
                    url: a.browser_download_url,
                    downloads: a.download_count,
                })
                .collect(),
            tarball_url: release.tarball_url,
        }
    }

    /// The title, or the tag when there is none.
    pub fn display_name(&self) -> &str {
        if self.name.trim().is_empty() {
            &self.tag_name
        } else {
            &self.name
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ReleasesViewState {
    /// Newest first.
    pub rows: Vec<ReleaseRow>,
    /// The selected release's id.
    pub selected: Option<u64>,
    pub loading: bool,
    pub loaded: bool,
    pub reload_pending: bool,
    pub error: Option<String>,
    /// No account for the repository's endpoint.
    pub signed_out: bool,
    /// History's selection when the list opened, restored on close.
    pub saved_selection: Vec<String>,
    /// `355-github-links-in-app`: the release to select once the list has
    /// loaded (by tag).
    pub wanted_tag: Option<String>,
}

impl ReleasesViewState {
    /// The newest release that is neither a draft nor a pre-release: what
    /// GitHub marks Latest.
    pub fn latest_id(&self) -> Option<u64> {
        self.rows
            .iter()
            .find(|r| !r.draft && !r.prerelease)
            .map(|r| r.id)
    }

    pub fn find(&self, id: u64) -> Option<&ReleaseRow> {
        self.rows.iter().find(|r| r.id == id)
    }

    pub fn selected_row(&self) -> Option<&ReleaseRow> {
        self.find(self.selected?)
    }
}

/// The open list of the selected repository, `None` while the flag is off.
pub fn releases_of<'a>(s: &AppState, rs: &'a RepositoryState) -> Option<&'a ReleasesViewState> {
    if !s.flags.bool(crate::flags::ids::RELEASES) {
        return None;
    }
    rs.releases.as_ref()
}

/// What the Create Release dialog hands over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseDraft {
    pub tag: String,
    /// The tag does not exist locally: GitHub creates it at `target_sha`.
    pub new_tag: bool,
    /// The commit a new tag is made at (the current branch's tip when the
    /// dialog was opened without one).
    pub target_sha: Option<String>,
    pub title: String,
    pub body: String,
    pub prerelease: bool,
    pub draft: bool,
}

/// The notes Generate release notes produced, picked up by the dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedReleaseNotes {
    pub title: String,
    pub body: String,
    /// From the local history (GitHub could not generate them).
    pub local: bool,
}

/// Release notes from the local history: one line per commit, newest
/// first, as GitHub's "What's Changed" list reads without pull requests.
pub fn local_release_notes(commits: &[Commit]) -> String {
    if commits.is_empty() {
        return String::new();
    }
    let mut out = String::from("## What's Changed\n");
    for commit in commits {
        out.push_str("* ");
        out.push_str(commit.summary.trim());
        if !commit.author.name.trim().is_empty() {
            out.push_str(" by ");
            out.push_str(commit.author.name.trim());
        }
        out.push('\n');
    }
    out
}

impl Dispatcher {
    /// Repository › Releases…: History lists the releases.
    pub fn show_releases(id: u64, cx: &mut dyn Host) {
        if !Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::RELEASES)
        {
            return;
        }
        Self::show_section(id, Section::History, cx);
        let opened = Self::state(cx).update(cx, |s, cx| {
            let rs = s.repo_state_mut(id);
            if rs.releases.as_ref().is_some_and(|r| r.loaded || r.loading) {
                return false;
            }
            let saved = rs.selected_commits.clone();
            let releases = rs.releases.get_or_insert_with(Default::default);
            releases.saved_selection = saved;
            rs.selected_commits.clear();
            rs.selected_commit = None;
            rs.changeset = None;
            rs.commit_selected_file = None;
            rs.commit_diff = None;
            cx.notify();
            true
        });
        if opened {
            Self::close_blame(id, cx);
            Self::close_recent_activity(id, cx);
            Self::close_issues(id, cx);
            Self::close_tags(id, cx);
            Self::load_releases(id, cx);
        }
    }

    /// The header's close button and Escape: back to History, its
    /// selection as it was.
    pub fn close_releases(id: u64, cx: &mut dyn Host) {
        let saved = Self::state(cx).update(cx, |s, cx| {
            let rs = s.repo_states.get_mut(&id)?;
            let releases = rs.releases.take()?;
            rs.selected_commits.clear();
            rs.selected_commit = None;
            rs.changeset = None;
            rs.commit_selected_file = None;
            rs.commit_diff = None;
            cx.notify();
            Some(releases.saved_selection)
        });
        if let Some(saved) = saved
            && !saved.is_empty()
        {
            Self::select_commits(id, saved, cx);
        }
    }

    fn releases_target(id: u64, cx: &dyn Host) -> Option<GitHubRepository> {
        Self::state(cx)
            .read(cx)
            .repository(id)
            .and_then(|r| r.non_fork_github().cloned())
    }

    /// Read the list (opened, refreshed, a release created). One load at
    /// a time; one asked for meanwhile runs after it.
    pub fn load_releases(id: u64, cx: &mut dyn Host) {
        let gh = Self::releases_target(id, cx);
        let api = gh
            .as_ref()
            .and_then(|gh| Self::api_for_repository(id, gh, cx));
        let (sso_hint, error_details) = {
            let s = Self::state(cx).read(cx);
            (
                s.flags.bool(crate::flags::ids::API_SAML_SSO_HINT),
                s.flags.bool(crate::flags::ids::API_ERROR_DETAILS),
            )
        };
        let start = Self::state(cx).update(cx, |s, cx| {
            let Some(releases) = s.repo_state_mut(id).releases.as_mut() else {
                return false;
            };
            if releases.loading {
                releases.reload_pending = true;
                return false;
            }
            releases.signed_out = api.is_none() && releases.rows.is_empty();
            if api.is_none() {
                releases.loaded = true;
                cx.notify();
                return false;
            }
            releases.loading = true;
            releases.error = None;
            cx.notify();
            true
        });
        let (Some(gh), Some((endpoint, token, _))) = (gh, api.filter(|_| start)) else {
            return;
        };
        spawn_bg(
            cx,
            move || {
                Client::new(endpoint, token)
                    .with_sso_hint(sso_hint)
                    .with_error_details(error_details)
                    .releases(&gh.owner, &gh.name, RELEASE_PAGES)
            },
            move |result, cx| {
                let reload = Self::state(cx).update(cx, |s, cx| {
                    let Some(releases) = s.repo_state_mut(id).releases.as_mut() else {
                        return false;
                    };
                    releases.loading = false;
                    releases.loaded = true;
                    let reload = std::mem::take(&mut releases.reload_pending);
                    match result {
                        Ok(rows) => {
                            releases.rows = rows.into_iter().map(ReleaseRow::from_api).collect();
                            releases.error = None;
                            if releases
                                .selected
                                .is_some_and(|r| releases.find(r).is_none())
                            {
                                releases.selected = None;
                            }
                            if let Some(tag) = releases.wanted_tag.take()
                                && let Some(row) = releases.rows.iter().find(|r| r.tag_name == tag)
                            {
                                releases.selected = Some(row.id);
                            }
                            if releases.selected.is_none() {
                                releases.selected = releases.rows.first().map(|r| r.id);
                            }
                        }
                        Err(err) => releases.error = Some(err.to_string()),
                    }
                    cx.notify();
                    reload
                });
                if reload {
                    Self::load_releases(id, cx);
                }
            },
        );
    }

    /// `355-github-links-in-app`: the list with release `tag` selected (once
    /// loaded, when it is still loading).
    pub fn show_release_tag(id: u64, tag: String, cx: &mut dyn Host) {
        Self::show_releases(id, cx);
        Self::state(cx).update(cx, |s, cx| {
            let Some(releases) = s.repo_state_mut(id).releases.as_mut() else {
                return;
            };
            match releases.rows.iter().find(|r| r.tag_name == tag) {
                Some(row) if releases.loaded => releases.selected = Some(row.id),
                _ => releases.wanted_tag = Some(tag),
            }
            cx.notify();
        });
    }

    pub fn select_release(id: u64, release: Option<u64>, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            if let Some(releases) = s.repo_state_mut(id).releases.as_mut()
                && releases.selected != release
            {
                releases.selected = release;
                cx.notify();
            }
        });
    }

    /// Create Release… for `tag` (an existing one, or none for a new tag)
    /// at `sha` (the commit the menu was opened on, else the current
    /// branch's tip).
    pub fn show_create_release(
        id: u64,
        tag: Option<String>,
        sha: Option<String>,
        cx: &mut dyn Host,
    ) {
        if !Self::state(cx)
            .read(cx)
            .flags
            .bool(crate::flags::ids::RELEASES)
        {
            return;
        }
        let sha = sha.or_else(|| {
            Self::state(cx)
                .read(cx)
                .repo_states
                .get(&id)
                .and_then(|rs| rs.info.as_ref())
                .and_then(|info| info.current_branch())
                .and_then(|b| b.tip.clone())
        });
        Self::load_branch_list_tags(id, cx);
        Self::show_popup(Popup::CreateRelease { repo: id, tag, sha }, cx);
    }

    /// Generate release notes: GitHub's, since the tag before `target`
    /// (the one `tag` follows), or the local history's when the host cannot
    /// generate them. The dialog picks the result up from
    /// `RepositoryState::generated_release_notes`.
    pub fn generate_release_notes(id: u64, tag: String, target: Option<String>, cx: &mut dyn Host) {
        let Some(gh) = Self::releases_target(id, cx) else {
            return;
        };
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let api = Self::api_for_repository(id, &gh, cx);
        Self::state(cx).update(cx, |s, cx| {
            let rs = s.repo_state_mut(id);
            rs.generating_release_notes = true;
            rs.generated_release_notes = None;
            cx.notify();
        });
        spawn_bg(
            cx,
            move || {
                let target_ref = target.clone().unwrap_or_else(|| tag.clone());
                let previous = corvene_git::previous_tag(git.clone(), &workdir, &target_ref)
                    .ok()
                    .flatten();
                if let Some((endpoint, token, _)) = api {
                    let generated = Client::new(endpoint, token).generate_release_notes(
                        &gh.owner,
                        &gh.name,
                        &tag,
                        target.as_deref(),
                        previous.as_deref(),
                    );
                    match generated {
                        Ok(Some(notes)) => {
                            return Ok(GeneratedReleaseNotes {
                                title: notes.name,
                                body: notes.body,
                                local: false,
                            });
                        }
                        Ok(None) => info!("GitHub cannot generate notes here; using the history"),
                        Err(err) => return Err(err.to_string()),
                    }
                }
                let commits = match &previous {
                    Some(prev) => corvene_git::get_commits_in_range(
                        &workdir,
                        prev,
                        &target_ref,
                        LOCAL_NOTES_LIMIT,
                    ),
                    None => corvene_git::get_commits_in_range(
                        &workdir,
                        &format!("{target_ref}~{LOCAL_NOTES_LIMIT}"),
                        &target_ref,
                        LOCAL_NOTES_LIMIT,
                    )
                    .or_else(|_| {
                        corvene_git::get_commits(&workdir, &target_ref, 0, LOCAL_NOTES_LIMIT)
                    }),
                }
                .map_err(|e| e.to_string())?;
                Ok(GeneratedReleaseNotes {
                    title: tag,
                    body: local_release_notes(&commits),
                    local: true,
                })
            },
            move |result: Result<GeneratedReleaseNotes, String>, cx| {
                Self::state(cx).update(cx, |s, cx| {
                    let rs = s.repo_state_mut(id);
                    rs.generating_release_notes = false;
                    rs.generated_release_notes = Some(result);
                    cx.notify();
                });
            },
        );
    }

    /// The dialog took the generated notes.
    pub fn take_generated_release_notes(
        id: u64,
        cx: &mut dyn Host,
    ) -> Option<Result<GeneratedReleaseNotes, String>> {
        Self::state(cx).update(cx, |s, _| {
            s.repo_state_mut(id).generated_release_notes.take()
        })
    }

    /// Create Release › Create Release / Save Draft: push the tag when it
    /// is local only, make sure a new tag's commit is on the remote, then
    /// `POST /repos/{owner}/{name}/releases`; the list (when open) shows the
    /// new release and a banner links to it.
    pub fn create_release(id: u64, draft: ReleaseDraft, cx: &mut dyn Host) {
        let Some(gh) = Self::releases_target(id, cx) else {
            return;
        };
        let Some((endpoint, token, _)) = Self::api_for_repository(id, &gh, cx) else {
            Self::show_error(
                "Could not create release",
                format!(
                    "Sign in to {} first.",
                    corvene_github::Endpoint::from_api_base(&gh.endpoint).host()
                ),
                cx,
            );
            return;
        };
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let (remote, unpushed_tag, sso_hint, error_details) = {
            let s = Self::state(cx).read(cx);
            let repo = s.repository(id);
            let info = s.repo_states.get(&id).and_then(|rs| rs.info.as_ref());
            let remote = info.and_then(|info| {
                info.current_branch()
                    .and_then(|b| b.upstream_remote_name())
                    .and_then(|n| info.remotes.iter().find(|r| r.name == n))
                    .or_else(|| corvene_git::find_default_remote(&info.remotes))
                    .cloned()
            });
            (
                remote,
                !draft.new_tag && repo.is_some_and(|r| r.tags_to_push.contains(&draft.tag)),
                s.flags.bool(crate::flags::ids::API_SAML_SSO_HINT),
                s.flags.bool(crate::flags::ids::API_ERROR_DETAILS),
            )
        };
        let Some(remote) = remote else {
            Self::show_error(
                "Could not create release",
                "The repository has no remote to publish the tag to.",
                cx,
            );
            return;
        };
        Self::arm_credential_helper_for(id, &remote.url, cx);
        let askpass = Self::askpass_env_for(id, &remote.url, cx);
        Self::close_popup_if(|p| matches!(p, Popup::CreateRelease { .. }), cx);
        let tag = draft.tag.clone();
        spawn_bg(
            cx,
            move || {
                if unpushed_tag {
                    corvene_git::push_tag(
                        git.clone(),
                        &workdir,
                        &remote.name,
                        &draft.tag,
                        askpass.as_ref(),
                    )
                    .map_err(|e| format!("Could not push the tag {}: {e}", draft.tag))?;
                } else if draft.new_tag
                    && let Some(sha) = &draft.target_sha
                {
                    let on_remote =
                        corvene_git::remote_branches_containing(git.clone(), &workdir, sha)
                            .map(|branches| {
                                branches
                                    .iter()
                                    .any(|b| b.starts_with(&format!("{}/", remote.name)))
                            })
                            .unwrap_or(true);
                    if !on_remote {
                        return Err(format!(
                            "Push the branch first: {} is not on {} yet, so GitHub cannot tag it.",
                            &sha[..sha.len().min(7)],
                            remote.name
                        ));
                    }
                }
                let new = NewRelease {
                    tag_name: draft.tag.clone(),
                    target_commitish: draft.new_tag.then(|| draft.target_sha.clone()).flatten(),
                    name: Some(draft.title.trim().to_string()).filter(|t| !t.is_empty()),
                    body: draft.body,
                    draft: draft.draft,
                    prerelease: draft.prerelease,
                };
                Client::new(endpoint, token)
                    .with_sso_hint(sso_hint)
                    .with_error_details(error_details)
                    .create_release(&gh.owner, &gh.name, &new)
                    .map_err(|e| e.to_string())
            },
            move |result, cx| match result {
                Ok(release) => {
                    let row = ReleaseRow::from_api(release);
                    info!(id, tag = %row.tag_name, draft = row.draft, "release created");
                    if unpushed_tag {
                        Self::update_tags_to_push(id, cx, |tags| tags.retain(|t| t != &tag));
                    }
                    Self::set_banner(
                        Banner::ReleaseCreated {
                            name: row.display_name().to_string(),
                            html_url: row.html_url.clone(),
                            draft: row.draft,
                        },
                        cx,
                    );
                    Self::state(cx).update(cx, |s, cx| {
                        let rs = s.repo_state_mut(id);
                        if let Some(releases) = rs.releases.as_mut()
                            && releases.loaded
                        {
                            releases.rows.retain(|r| r.id != row.id);
                            releases.rows.insert(0, row.clone());
                            releases.selected = Some(row.id);
                            cx.notify();
                        }
                    });
                    // a new tag now exists on the remote; a pushed one is listed
                    Self::refresh_repository(id, cx);
                    Self::load_branch_list_tags(id, cx);
                }
                Err(err) => Self::show_error("Could not create release", err, cx),
            },
        );
    }

    pub fn open_release_on_github(id: u64, release: u64, cx: &mut dyn Host) {
        let url = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|rs| rs.releases.as_ref())
            .and_then(|r| r.find(release))
            .map(|r| r.html_url.clone());
        if let Some(url) = url {
            Self::open_url(&url, cx);
        }
    }
}

/// `CORVENE_POPUP=releases`: sample releases in the view, no API.
pub fn install_samples(id: u64, cx: &mut dyn Host) {
    let gh = crate::samples::github_repository(id, cx);
    let row = |id: u64, tag: &str, name: &str, draft: bool, prerelease: bool, ago: u64| {
        ReleaseRow {
            id,
            tag_name: tag.to_string(),
            name: name.to_string(),
            body: format!(
                "## What's Changed\n* Keep scroll position on refresh by @octocat in #{}\n* Show commit signature status by @mona in #{}\n\n**Full Changelog**: {}/compare/v0.0.9...{tag}",
                id * 3,
                id * 3 + 1,
                gh.html_url
            ),
            html_url: format!("{}/releases/tag/{tag}", gh.html_url),
            draft,
            prerelease,
            author: "octocat".to_string(),
            created_at: Some(crate::samples::iso_ago(ago)),
            published_at: (!draft).then(|| crate::samples::iso_ago(ago)),
            target: Some("main".to_string()),
            assets: if draft {
                Vec::new()
            } else {
                vec![
                    ReleaseAsset {
                        name: format!("Corvene-{}.dmg", tag.trim_start_matches('v')),
                        size: 24_117_248,
                        url: format!("{}/releases/download/{tag}/Corvene.dmg", gh.html_url),
                        downloads: 128,
                    },
                    ReleaseAsset {
                        name: "SHA256SUMS".to_string(),
                        size: 412,
                        url: format!("{}/releases/download/{tag}/SHA256SUMS", gh.html_url),
                        downloads: 9,
                    },
                ]
            },
            tarball_url: Some(format!("{}/archive/refs/tags/{tag}.tar.gz", gh.html_url)),
        }
    };
    let rows = vec![
        row(3, "v0.2.0", "", true, false, 3_600),
        row(2, "v0.1.1-beta.1", "Beta 1", false, true, 86_400 * 3),
        row(1, "v0.1.0", "First release", false, false, 86_400 * 20),
    ];
    Dispatcher::state(cx).update(cx, |s, cx| {
        let rs = s.repo_state_mut(id);
        rs.releases = Some(ReleasesViewState {
            selected: Some(1),
            rows,
            ..Default::default()
        });
        cx.notify();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(summary: &str, author: &str) -> Commit {
        let identity = corvene_models::CommitIdentity {
            name: author.to_string(),
            email: String::new(),
            seconds: 0,
            offset: 0,
        };
        Commit {
            sha: "0".repeat(40),
            summary: summary.to_string(),
            body: String::new(),
            author: identity.clone(),
            committer: identity,
            parents: Vec::new(),
            trailers: Vec::new(),
            tags: Vec::new(),
            signature: None,
        }
    }

    #[test]
    fn local_notes_list_commits() {
        assert_eq!(local_release_notes(&[]), "");
        let notes =
            local_release_notes(&[commit("Add dark mode ", "Mona"), commit("Fix crash", "")]);
        assert_eq!(
            notes,
            "## What's Changed\n* Add dark mode by Mona\n* Fix crash\n"
        );
    }

    #[test]
    fn latest_skips_drafts_and_prereleases() {
        let row = |id, draft, prerelease| ReleaseRow {
            id,
            tag_name: format!("v{id}"),
            name: String::new(),
            body: String::new(),
            html_url: String::new(),
            draft,
            prerelease,
            author: String::new(),
            created_at: None,
            published_at: None,
            target: None,
            assets: Vec::new(),
            tarball_url: None,
        };
        let state = ReleasesViewState {
            rows: vec![
                row(3, true, false),
                row(2, false, true),
                row(1, false, false),
            ],
            ..Default::default()
        };
        assert_eq!(state.latest_id(), Some(1));
        assert_eq!(state.find(3).map(|r| r.display_name()), Some("v3"));
    }
}
