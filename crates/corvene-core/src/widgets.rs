//! Corvene `433-macos-widgets` (desktop/desktop#18139): desktop and
//! Notification Center widgets. GHD has none.
//!
//! The extension (`packaging/widgets`) is sandboxed and holds no token: the
//! app writes a JSON snapshot into the extension's own container and asks
//! WidgetKit to reload. App group containers are out, since a widget reading
//! one from a self-signed app is denied by TCC with no way to prompt. Pull
//! requests come from an issue search per account every few minutes; checks
//! are what the menu bar status item shows for the chosen repositories
//! ([`crate::menu_bar_status`]), written within a minute of the app knowing
//! them.
//!
//! With the flag off the extension is switched off with `pluginkit` so the
//! gallery does not offer it, and the snapshot is removed.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use corvene_models::Account;
use serde::Serialize;

use crate::dispatcher::Dispatcher;
use crate::host::{AsyncCtx, Host};
use crate::menu_bar_status::MenuBarRepo;
use crate::remote::spawn_bg;
use crate::state::AppState;

/// `packaging/widgets/Info.plist`
const WIDGET_BUNDLE_ID: &str = "com.wasimaster.corvene.widgets";
/// How often the snapshot is compared and, when changed, written.
const TICK: Duration = Duration::from_secs(60);
/// Pull requests are searched every this many ticks.
const SEARCH_EVERY_TICKS: u32 = 5;

/// A pull request an account's search found.
#[derive(Clone, Debug, PartialEq, Eq)]
struct FoundPullRequest {
    owner: String,
    name: String,
    number: u64,
    title: String,
    html_url: String,
    review: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WidgetPullRequest {
    pub title: String,
    /// `owner/name`
    pub repository: String,
    pub number: u64,
    /// In Corvene when the repository is added, else on GitHub.
    pub url: String,
    /// `review` or `assigned`
    pub reason: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WidgetRepository {
    pub name: String,
    pub branch: Option<String>,
    /// `success`, `failure`, `pending` or `none`
    pub state: &'static str,
    pub summary: String,
    pub url: String,
}

/// What the extension reads (`WidgetSnapshot` in `WidgetViews.swift`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WidgetSnapshot {
    pub generated_at: u64,
    pub signed_in: bool,
    pub review_count: usize,
    pub assigned_count: usize,
    pub pull_requests: Vec<WidgetPullRequest>,
    pub repositories: Vec<WidgetRepository>,
}

/// The last search's result, between ticks.
static FOUND: Mutex<Vec<FoundPullRequest>> = Mutex::new(Vec::new());
/// The last snapshot written.
static WRITTEN: Mutex<Option<WidgetSnapshot>> = Mutex::new(None);

/// Waiting for review first, each pull request once.
fn merge_found(
    review: Vec<FoundPullRequest>,
    assigned: Vec<FoundPullRequest>,
) -> Vec<FoundPullRequest> {
    let mut all = review;
    for pr in assigned {
        let known = all
            .iter()
            .any(|p| p.owner == pr.owner && p.name == pr.name && p.number == pr.number);
        if !known {
            all.push(pr);
        }
    }
    all
}

fn widget_repository(repo: &MenuBarRepo, url: String) -> WidgetRepository {
    let (state, summary) = match &repo.checks {
        None => ("none", "No checks".to_string()),
        Some(c) if c.failed > 0 => ("failure", c.label()),
        Some(c) if c.pending > 0 => ("pending", c.label()),
        Some(c) => ("success", c.label()),
    };
    WidgetRepository {
        name: repo.name.clone(),
        branch: repo.branch.clone(),
        state,
        summary,
        url,
    }
}

impl AppState {
    /// `433-macos-widgets` is on (only ever on macOS).
    pub fn widgets_enabled(&self) -> bool {
        cfg!(target_os = "macos") && self.flags.bool(crate::flags::ids::MACOS_WIDGETS)
    }

    fn widget_accounts(&self) -> Vec<Account> {
        self.accounts
            .iter()
            .filter(|a| self.host_kind_of(&a.endpoint).is_none())
            .cloned()
            .collect()
    }

    fn widget_pull_request(&self, pr: &FoundPullRequest) -> WidgetPullRequest {
        let added = self.repositories.iter().find_map(|r| {
            let gh = r.github.as_ref()?;
            std::iter::once(gh).chain(gh.parent.as_deref()).find(|g| {
                g.owner.eq_ignore_ascii_case(&pr.owner) && g.name.eq_ignore_ascii_case(&pr.name)
            })
        });
        WidgetPullRequest {
            title: pr.title.clone(),
            repository: format!("{}/{}", pr.owner, pr.name),
            number: pr.number,
            url: match added {
                Some(gh) => format!(
                    "{}://openRepo/{}?pr={}",
                    crate::app_url::URL_SCHEME,
                    gh.clone_url,
                    pr.number
                ),
                None => pr.html_url.clone(),
            },
            reason: if pr.review { "review" } else { "assigned" },
        }
    }

    fn widget_snapshot(&self, found: &[FoundPullRequest]) -> WidgetSnapshot {
        let repositories = self
            .watched_repositories_model()
            .iter()
            .map(|repo| {
                let url = self
                    .repository(repo.id)
                    .map(|r| crate::app_url::open_local_repo_url(&r.path))
                    .unwrap_or_default();
                widget_repository(repo, url)
            })
            .collect();
        WidgetSnapshot {
            generated_at: 0,
            signed_in: !self.widget_accounts().is_empty(),
            review_count: found.iter().filter(|p| p.review).count(),
            assigned_count: found.iter().filter(|p| !p.review).count(),
            pull_requests: found.iter().map(|p| self.widget_pull_request(p)).collect(),
            repositories,
        }
    }
}

/// `~/Library/Containers/<extension>/Data`, which the system makes the first
/// time the widget runs. Not ours to create.
fn container() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("HOME")?)
        .join("Library/Containers")
        .join(WIDGET_BUNDLE_ID)
        .join("Data");
    dir.is_dir().then_some(dir)
}

/// `corvene_reload_widgets` of the library in `Contents/Frameworks`. Nothing
/// outside a bundle built by `packaging/bundle.sh`.
#[cfg(target_os = "macos")]
fn reload_widgets() {
    let Some(path) = std::env::current_exe()
        .ok()
        .and_then(|exe| {
            Some(
                exe.parent()?
                    .parent()?
                    .join("Frameworks/libCorveneWidgetCenter.dylib"),
            )
        })
        .filter(|p| p.exists())
    else {
        return;
    };
    // SAFETY: our own library, whose one function takes and returns nothing
    // and is kept loaded for the process's life.
    unsafe {
        match libloading::Library::new(&path) {
            Ok(library) => {
                if let Ok(reload) =
                    library.get::<unsafe extern "C" fn()>(b"corvene_reload_widgets\0")
                {
                    reload();
                }
                std::mem::forget(library);
            }
            Err(err) => tracing::warn!(%err, "could not load the widget library"),
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn reload_widgets() {}

/// `pluginkit -e use|ignore`: whether the gallery offers the widgets. Only in
/// a bundle that has the extension.
fn set_extension_enabled(on: bool) {
    let in_bundle = std::env::current_exe().ok().is_some_and(|exe| {
        exe.parent()
            .and_then(|d| d.parent())
            .is_some_and(|c| c.join("PlugIns/CorveneWidgets.appex").exists())
    });
    if !in_bundle {
        return;
    }
    let election = if on { "use" } else { "ignore" };
    if let Err(err) = std::process::Command::new("/usr/bin/pluginkit")
        .args(["-e", election, "-i", WIDGET_BUNDLE_ID])
        .status()
    {
        tracing::warn!(%err, "pluginkit");
    }
}

impl Dispatcher {
    /// Launch: keep the snapshot fresh, or with the flag off make sure the
    /// extension and its snapshot are gone.
    pub fn start_widgets(cx: &mut dyn Host) {
        if !cfg!(target_os = "macos") {
            return;
        }
        let on = Self::state(cx).read(cx).widgets_enabled();
        cx.background_executor()
            .spawn(async move {
                set_extension_enabled(on);
                if !on && let Some(dir) = container() {
                    let _ = std::fs::remove_file(dir.join("snapshot.json"));
                }
            })
            .detach();
        if !on {
            return;
        }
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let mut tick = 0;
            loop {
                cx.update(|cx| {
                    if tick % SEARCH_EVERY_TICKS == 0 {
                        Self::search_widget_pull_requests(cx);
                    }
                    Self::touch_menu_bar_statuses(cx);
                    Self::write_widget_snapshot(cx);
                });
                tick = tick.wrapping_add(1);
                cx.background_executor().timer(TICK).await;
            }
        })
        .detach();
    }

    /// Control hook `widgets-refresh`, and every few minutes.
    pub fn search_widget_pull_requests(cx: &mut dyn Host) {
        let accounts = Self::state(cx).read(cx).widget_accounts();
        spawn_bg(
            cx,
            move || {
                let (mut review, mut assigned) = (Vec::new(), Vec::new());
                for account in &accounts {
                    let Some(client) = crate::alive::account_client(account) else {
                        continue;
                    };
                    let search = |query: &str, is_review: bool, out: &mut Vec<_>| {
                        // a failed search keeps nothing of this account until
                        // the next one; the widget shows what is left
                        let Ok(found) = client.search_pull_requests(query) else {
                            return;
                        };
                        out.extend(found.into_iter().filter_map(|pr| {
                            let (owner, name) = pr.repository()?;
                            Some(FoundPullRequest {
                                html_url: pr.html_url.unwrap_or_else(|| {
                                    format!("https://github.com/{owner}/{name}/pull/{}", pr.number)
                                }),
                                owner,
                                name,
                                number: pr.number,
                                title: pr.title,
                                review: is_review,
                            })
                        }));
                    };
                    search(
                        "is:open is:pr archived:false review-requested:@me",
                        true,
                        &mut review,
                    );
                    search(
                        "is:open is:pr archived:false assignee:@me",
                        false,
                        &mut assigned,
                    );
                }
                merge_found(review, assigned)
            },
            |found, cx| {
                if let Ok(mut last) = FOUND.lock() {
                    *last = found;
                }
                Self::write_widget_snapshot(cx);
            },
        );
    }

    /// Write the snapshot if it changed and there is a container, then have
    /// WidgetKit reload. A write that fails is tried again next tick.
    fn write_widget_snapshot(cx: &mut dyn Host) {
        let found = FOUND.lock().map(|f| f.clone()).unwrap_or_default();
        let snapshot = Self::state(cx).read(cx).widget_snapshot(&found);
        if WRITTEN.lock().is_ok_and(|w| w.as_ref() == Some(&snapshot)) {
            return;
        }
        cx.background_executor()
            .spawn(async move {
                let Some(dir) = container() else {
                    return;
                };
                let stamped = WidgetSnapshot {
                    generated_at: crate::autocomplete::now_secs(),
                    ..snapshot.clone()
                };
                let Ok(json) = serde_json::to_vec(&stamped) else {
                    return;
                };
                let tmp = dir.join("snapshot.json.tmp");
                let written = std::fs::write(&tmp, json)
                    .and_then(|()| std::fs::rename(&tmp, dir.join("snapshot.json")));
                match written {
                    Ok(()) => {
                        if let Ok(mut w) = WRITTEN.lock() {
                            *w = Some(snapshot);
                        }
                        reload_widgets();
                    }
                    Err(err) => tracing::warn!(%err, "could not write the widget snapshot"),
                }
            })
            .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(number: u64, review: bool) -> FoundPullRequest {
        FoundPullRequest {
            owner: "me".to_string(),
            name: "repo".to_string(),
            number,
            title: format!("pr {number}"),
            html_url: String::new(),
            review,
        }
    }

    #[test]
    fn review_requests_win_over_assignments() {
        let all = merge_found(
            vec![found(1, true), found(2, true)],
            vec![found(2, false), found(3, false)],
        );
        let numbers: Vec<(u64, bool)> = all.iter().map(|p| (p.number, p.review)).collect();
        assert_eq!(numbers, [(1, true), (2, true), (3, false)]);
    }

    #[test]
    fn a_repository_without_checks_says_so() {
        let repo = MenuBarRepo {
            id: 1,
            name: "repo".to_string(),
            branch: Some("main".to_string()),
            ahead: 0,
            behind: 0,
            has_upstream: true,
            checks: None,
        };
        let row = widget_repository(&repo, "url".to_string());
        assert_eq!((row.state, row.summary.as_str()), ("none", "No checks"));
    }

    /// The field names the extension decodes (`WidgetViews.swift`).
    #[test]
    fn the_snapshot_keeps_its_field_names() {
        let json = serde_json::to_value(WidgetSnapshot {
            generated_at: 1,
            signed_in: true,
            review_count: 0,
            assigned_count: 0,
            pull_requests: Vec::new(),
            repositories: Vec::new(),
        })
        .unwrap();
        for key in [
            "generated_at",
            "signed_in",
            "review_count",
            "assigned_count",
            "pull_requests",
            "repositories",
        ] {
            assert!(json.get(key).is_some(), "{key}");
        }
    }
}
