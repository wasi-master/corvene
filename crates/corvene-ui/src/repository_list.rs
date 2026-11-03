//! Repository foldout: filter + "Add ▾", then grouped 29 px rows
//! (`ui/repositories-list/*.tsx`, `styles/ui/_repository-list.scss`).
//! The groups are GHD `groupRepositories` ([`group_repositories`]); the
//! filter then keeps each group's matches, as GHD `FilterList` does.
//!
//! Deviation (`291-recent-worktrees`): a Recent repository used in several
//! worktrees is listed once per worktree (named, dimmed); picking one
//! switches to that worktree.
//!
//! Deviation (`290-custom-repository-groups`): "Move to Group…" puts a
//! repository in a group of the user's naming, listed (by name) after
//! Pinned and Recent and before GHD's owner groups.
//!
//! Deviation (`288-dead-remote-indicator`): a repository whose last fetch
//! found no remote repository shows the alert icon and says so in its
//! tooltip (GHD `ui/repositories-list/repository-list-item.tsx` only marks
//! missing folders).

use std::collections::HashMap;

use corvene_core::{AheadBehind, AppState, Dispatcher, Popup, RepoIndicator, Repository};
use gpui_kit::component::input::InputState;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::actions::{FilterListPick, SelectNextFile, SelectPreviousFile};
use crate::context_menu::mac_or;
use crate::icons::{Octicon, RepositoryOrCloning, octicon};
use crate::scrollbar::ScrollbarExt;
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::GhdTooltip;
use crate::widgets::ListRowA11y;
use crate::widgets::button;

pub struct RepositoryFoldout {
    state: Entity<AppState>,
    filter: Entity<InputState>,
    add_menu_open: bool,
    /// Corvene (`207-repository-status-filter`): only repositories with
    /// uncommitted changes / commits to push or pull.
    only_changed: bool,
    only_ahead_behind: bool,
    /// Corvene (`208-repository-fork-filter`): only forks / only the rest.
    only_forks: bool,
    only_sources: bool,
    /// GHD `FilterList` keyboard selection: the row ↓ / ↑ moved to from
    /// the filter box (an index into the rows as shown, groups flattened).
    highlighted: Option<usize>,
    scroll: ScrollHandle,
    /// Shift+F10 / Menu: the highlighted (else the selected) row.
    menu_anchor: crate::context_menu::RowMenuAnchor,
}

struct Group {
    title: SharedString,
    /// GHD's `recent` group.
    recent: bool,
    /// Corvene (`266-collapsible-repository-groups`): the name the collapsed
    /// set stores, `None` when the header has no chevron (flag off, a
    /// filtered list, the flat result list).
    key: Option<String>,
    /// Hidden rows: `repos` is empty and the header shows a right chevron.
    collapsed: bool,
    /// Each repository with the char positions of its name the filter
    /// matched (`HighlightText`) and whether it needs its owner prefix
    /// (`needsDisambiguation`).
    repos: Vec<(Repository, Vec<usize>, bool)>,
}

/// `290-custom-repository-groups`: a custom group's name as typed and its
/// matching repositories (score, repository, matched name chars).
type CustomGroup = (String, Vec<(f32, Repository, Vec<usize>)>);

/// GHD `RepositoryListGroup` (`ui/repositories-list/group-repositories.ts`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepositoryListGroup {
    Recent,
    /// GitHub.com repositories of one owner (the login).
    Dotcom {
        owner: String,
    },
    /// The repositories of one GitHub Enterprise host.
    Enterprise {
        host: String,
    },
    Other,
}

impl RepositoryListGroup {
    /// GHD `getGroupKey`: unique, and the case-sensitive order of the keys
    /// is the order of the groups.
    pub fn key(&self) -> String {
        match self {
            Self::Recent => "0:recent".into(),
            Self::Dotcom { owner } => format!("1:dotcom:{owner}"),
            Self::Enterprise { host } => format!("2:enterprise:{host}"),
            Self::Other => "3:other".into(),
        }
    }

    /// GHD `getGroupLabel` (`repositories-list.tsx`): the group header.
    pub fn label(&self) -> &str {
        match self {
            Self::Recent => "Recent",
            Self::Dotcom { owner } => owner,
            Self::Enterprise { host } => host,
            Self::Other => "Other",
        }
    }
}

/// GHD `IRepositoryListItem`.
#[derive(Clone, Debug)]
pub struct RepositoryListItem {
    /// The texts the filter matches: the display title (alias, else name)
    /// and `nameOf` (`owner/name`, else the folder).
    pub text: Vec<String>,
    pub id: String,
    pub repository: Repository,
    /// The row shows its owner prefix (a duplicate title in an Enterprise
    /// group, or anywhere for the Recent group).
    pub needs_disambiguation: bool,
    pub ahead_behind: Option<AheadBehind>,
    pub changed_files_count: usize,
}

/// GHD `IFilterListGroup<IRepositoryListItem, RepositoryListGroup>`.
#[derive(Clone, Debug)]
pub struct RepositoryGroup {
    pub identifier: RepositoryListGroup,
    pub items: Vec<RepositoryListItem>,
}

/// GHD `recentRepositoriesThreshold`: more repositories than this get a
/// Recent group.
pub const RECENT_REPOSITORIES_THRESHOLD: usize = 7;

/// GHD `isDotCom` (`lib/endpoint-capabilities.ts`).
fn is_dot_com(endpoint: &str) -> bool {
    endpoint == "https://api.github.com" || {
        let host = corvene_core::host_of(endpoint);
        host == "api.github.com" || host == "github.com"
    }
}

/// GHD `getGroupForRepository`.
fn group_for_repository(repository: &Repository) -> RepositoryListGroup {
    match &repository.github {
        Some(gh) if is_dot_com(&gh.endpoint) => RepositoryListGroup::Dotcom {
            owner: gh.owner.clone(),
        },
        // `getHostForRepository`: the host of `getHTMLURL(endpoint)`, which
        // is the endpoint's scheme and host name alone
        Some(gh) => RepositoryListGroup::Enterprise {
            host: corvene_core::host_of(
                corvene_github::Endpoint::from_api_base(&gh.endpoint).host(),
            ),
        },
        None => RepositoryListGroup::Other,
    }
}

/// GHD `groupRepositories(repositories, localRepositoryStateLookup,
/// recentRepositories)`: the Recent group (with more than
/// [`RECENT_REPOSITORIES_THRESHOLD`] repositories), one group per GitHub.com
/// owner, one per Enterprise host, then Other, ordered by
/// [`RepositoryListGroup::key`]; each group's items sorted by title,
/// ignoring case.
pub fn group_repositories(
    repositories: &[Repository],
    local_repository_state_lookup: &HashMap<u64, RepoIndicator>,
    recent_repositories: &[u64],
) -> Vec<RepositoryGroup> {
    let include_recent = repositories.len() > RECENT_REPOSITORIES_THRESHOLD;
    let mut groups: Vec<(String, RepositoryListGroup, Vec<&Repository>)> = Vec::new();
    let mut add = |group: RepositoryListGroup, repository| {
        let key = group.key();
        match groups.iter_mut().find(|(k, _, _)| *k == key) {
            Some((_, _, repos)) => repos.push(repository),
            None => groups.push((key, group, vec![repository])),
        }
    };
    for repository in repositories {
        if include_recent && recent_repositories.contains(&repository.id) {
            add(RepositoryListGroup::Recent, repository);
        }
        add(group_for_repository(repository), repository);
    }
    groups.sort_by(|(x, _, _), (y, _, _)| x.cmp(y));

    // the titles of every group but Recent (its items are in another group
    // too), all together and per group
    let mut all_names: HashMap<String, usize> = HashMap::new();
    for (_, group, repos) in &groups {
        if *group != RepositoryListGroup::Recent {
            for r in repos {
                *all_names.entry(r.name()).or_default() += 1;
            }
        }
    }
    groups
        .iter()
        .map(|(_, group, repos)| {
            let mut group_names: HashMap<String, usize> = HashMap::new();
            if *group != RepositoryListGroup::Recent {
                for r in repos {
                    *group_names.entry(r.name()).or_default() += 1;
                }
            }
            let mut items: Vec<RepositoryListItem> = repos
                .iter()
                .map(|r| {
                    let title = r.name();
                    let state = local_repository_state_lookup.get(&r.id);
                    let needs_disambiguation = match group {
                        RepositoryListGroup::Enterprise { .. } => {
                            group_names.get(&title).copied().unwrap_or(0) > 1
                        }
                        RepositoryListGroup::Recent => {
                            all_names.get(&title).copied().unwrap_or(0) > 1
                        }
                        _ => false,
                    };
                    RepositoryListItem {
                        text: vec![title, corvene_core::name_of(r)],
                        id: r.id.to_string(),
                        repository: (*r).clone(),
                        needs_disambiguation,
                        ahead_behind: state.and_then(|s| s.ahead_behind),
                        changed_files_count: state.map_or(0, |s| s.changed_files),
                    }
                })
                .collect();
            // `caseInsensitiveCompare` of the titles (stable)
            items.sort_by_cached_key(|item| item.text[0].to_lowercase());
            RepositoryGroup {
                identifier: group.clone(),
                items,
            }
        })
        .collect()
}

/// What GHD `RepositoryListItem` draws in its `.name` element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositoryListItemName {
    /// The dimmed `.prefix` (`<owner>/`), when the row needs it.
    pub prefix: Option<String>,
    /// The whole text: the prefix, then the alias or name.
    pub text: String,
}

/// GHD `RepositoryListItem`'s name: the alias or name, after the GitHub
/// owner's `owner/` when `needs_disambiguation`.
pub fn repository_list_item_name(
    repository: &Repository,
    needs_disambiguation: bool,
) -> RepositoryListItemName {
    let prefix = repository
        .github
        .as_ref()
        .filter(|_| needs_disambiguation)
        .map(|gh| format!("{}/", gh.owner));
    RepositoryListItemName {
        text: format!("{}{}", prefix.as_deref().unwrap_or(""), repository.name()),
        prefix,
    }
}

/// What GHD `renderRepoIndicators` draws.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoIndicators {
    /// The `.ahead-behind` arrows (up, then down), `None` when nothing is
    /// ahead or behind.
    pub ahead_behind: Option<Vec<Octicon>>,
    /// The arrows' tooltip (`aheadBehindTooltip`).
    pub ahead_behind_tooltip: Option<String>,
    /// The `.change-indicator-wrapper` dot.
    pub changes: bool,
}

/// GHD `renderRepoIndicators` with `RepositoryListItem`'s `hasChanges`
/// (`changedFilesCount > 0`).
pub fn render_repo_indicators(
    ahead_behind: Option<AheadBehind>,
    changed_files_count: usize,
) -> RepoIndicators {
    render_repo_indicators_with(ahead_behind, changed_files_count, &|n| n.to_string())
}

/// [`render_repo_indicators`] with the tooltip's commit counts written by
/// `count` (`272-grouped-ahead-behind-counts`).
pub fn render_repo_indicators_with(
    ahead_behind: Option<AheadBehind>,
    changed_files_count: usize,
    count: &dyn Fn(u32) -> String,
) -> RepoIndicators {
    let ab = ahead_behind.filter(|ab| ab.ahead > 0 || ab.behind > 0);
    RepoIndicators {
        ahead_behind: ab.map(|ab| {
            [
                (ab.ahead > 0).then_some(Octicon::ArrowUp),
                (ab.behind > 0).then_some(Octicon::ArrowDown),
            ]
            .into_iter()
            .flatten()
            .collect()
        }),
        ahead_behind_tooltip: ab.map(|ab| {
            format!(
                "The currently checked out branch is{}{}{}its tracked branch.",
                if ab.behind > 0 {
                    format!(" {} behind ", commit_grammar(ab.behind, count))
                } else {
                    String::new()
                },
                if ab.behind > 0 && ab.ahead > 0 {
                    "and"
                } else {
                    ""
                },
                if ab.ahead > 0 {
                    format!(" {} ahead of ", commit_grammar(ab.ahead, count))
                } else {
                    String::new()
                },
            )
        }),
        changes: changed_files_count > 0,
    }
}

/// What GHD `RepositoryListItem.renderTooltip` shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositoryListItemTooltip {
    /// The bold GitHub full name, else the repository's name.
    pub full_name: String,
    /// The alias, in parentheses after it.
    pub alias: Option<String>,
    /// The second line.
    pub path: String,
}

/// GHD `RepositoryListItem.renderTooltip`.
pub fn repository_list_item_tooltip(repository: &Repository) -> RepositoryListItemTooltip {
    RepositoryListItemTooltip {
        // `gitHubRepo ? gitHubRepo.fullName : repo.name` (not the alias)
        full_name: corvene_core::name_of(repository),
        alias: repository.alias.clone(),
        path: repository.path.to_string_lossy().into_owned(),
    }
}

impl Group {
    /// A group that is not GHD's Recent one.
    fn new(
        title: impl Into<SharedString>,
        key: Option<String>,
        repos: Vec<(Repository, Vec<usize>, bool)>,
    ) -> Self {
        Self {
            title: title.into(),
            recent: false,
            key,
            collapsed: false,
            repos,
        }
    }
}

impl RepositoryFoldout {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        cx.observe(&filter, |this: &mut Self, _, cx| {
            this.highlighted = None;
            cx.notify()
        })
        .detach();
        let scroll = ScrollHandle::new();
        Self {
            state,
            filter,
            add_menu_open: false,
            only_changed: false,
            only_ahead_behind: false,
            only_forks: false,
            only_sources: false,
            highlighted: None,
            menu_anchor: crate::context_menu::RowMenuAnchor::new(&scroll),
            scroll,
        }
    }

    pub fn focus_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.highlighted = None;
        let handle = self.filter.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        // Corvene (`210-repository-filter-selects-text`): the remembered
        // filter text is selected, so typing replaces it
        if self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::REPOSITORY_FILTER_SELECTS_TEXT)
        {
            self.filter
                .update(cx, |input, cx| input.select_all(window, cx));
        }
    }

    /// GHD `FilterList`: ↓ / ↑ in the filter box move through the rows (↑
    /// from the filter starts at the last), wrapping at the ends.
    fn move_highlight(&mut self, delta: isize, cx: &mut Context<Self>) {
        let groups = self.groups(cx);
        let count: usize = groups.iter().map(|g| g.repos.len()).sum();
        if count == 0 {
            return;
        }
        // Corvene (`613-repository-list-starts-at-selected`): the first
        // arrow steps from the selected repository's (first) row
        let start = self.highlighted.or_else(|| {
            let s = self.state.read(cx);
            let selected = s.selected?;
            s.flags
                .bool(corvene_core::flags::ids::REPOSITORY_LIST_STARTS_AT_SELECTED)
                .then(|| {
                    groups
                        .iter()
                        .flat_map(|g| &g.repos)
                        .position(|(r, _, _)| r.id == selected)
                })
                .flatten()
        });
        let ix = match start {
            Some(ix) => crate::filter_list::wrap_step(ix, delta, count),
            None if delta < 0 => count - 1,
            None => 0,
        };
        self.highlighted = Some(ix);
        // rows and group headers are all `ROW_HEIGHT`: scroll the row in
        let mut headers = 0;
        let mut before = 0;
        for group in &groups {
            if !group.title.is_empty() {
                headers += 1;
            }
            if ix < before + group.repos.len() {
                break;
            }
            before += group.repos.len();
        }
        let top = ROW_HEIGHT() * (headers + ix) as f32;
        let bottom = top + ROW_HEIGHT();
        let height = self.scroll.bounds().size.height;
        let mut offset = self.scroll.offset();
        if top < -offset.y {
            offset.y = -top;
        } else if bottom > -offset.y + height {
            offset.y = height - bottom;
        }
        self.scroll.set_offset(offset);
        cx.notify();
    }

    /// Enter in the filter box: the highlighted row, else the first one.
    fn pick_highlighted(&mut self, cx: &mut Context<Self>) {
        let ix = self.highlighted.unwrap_or(0);
        let row = self
            .groups(cx)
            .into_iter()
            .flat_map(|g| g.repos)
            .nth(ix)
            .map(|(r, _, _)| r);
        if let Some(row) = row {
            select_row(&row, cx);
        }
    }

    /// GHD `groupRepositories` ([`group_repositories`]): Recent, one group
    /// per GitHub.com owner and per Enterprise host, then Other. A filter
    /// fuzzy-matches the name or `owner/name` and sorts each group best
    /// match first (`FilterList`'s `match`; ties keep the list order).
    fn groups(&self, cx: &App) -> Vec<Group> {
        let state = self.state.read(cx);
        let raw_query = self.filter.read(cx).value().trim().to_string();
        let query = raw_query.to_lowercase();
        // Corvene (`211-regex-repository-filter`): `/pattern/`
        let regex = state
            .flags
            .bool(corvene_core::flags::ids::REGEX_REPOSITORY_FILTER)
            .then(|| corvene_core::filter::regex_query(&raw_query))
            .flatten();
        // Corvene (`207-repository-status-filter`)
        let status_filter = state
            .flags
            .bool(corvene_core::flags::ids::REPOSITORY_STATUS_FILTER)
            && (self.only_changed || self.only_ahead_behind);
        // Corvene (`208-repository-fork-filter`)
        let fork_filter = state
            .flags
            .bool(corvene_core::flags::ids::REPOSITORY_FORK_FILTER)
            && (self.only_forks || self.only_sources);
        // the status / fork filters; the query is matched below
        let passes_filters = |r: &Repository| {
            (!status_filter || {
                let (ahead_behind, changed_files) = indicators(state, r.id);
                (self.only_changed && changed_files > 0)
                    || (self.only_ahead_behind && ahead_behind.is_some())
            }) && (!fork_filter || {
                let fork = r.github.as_ref().is_some_and(|gh| gh.fork);
                (self.only_forks && fork) || (self.only_sources && !fork)
            })
        };

        // Corvene (`266-collapsible-repository-groups`): chevrons on the
        // headers, except while filtering (then every group is expanded)
        let filtering = !query.is_empty() || status_filter || fork_filter;
        let collapsible = !filtering
            && state
                .flags
                .bool(corvene_core::flags::ids::COLLAPSIBLE_REPOSITORY_GROUPS);
        let key = |k: String| collapsible.then_some(k);

        // Corvene (`209-recent-repositories-count`; GHD remembers 3); the
        // Recent group is left out while a status or fork filter is on
        let recent: Vec<u64> = if status_filter || fork_filter {
            Vec::new()
        } else {
            let shown = usize::try_from(
                state
                    .flags
                    .number(corvene_core::flags::ids::RECENT_REPOSITORIES_COUNT),
            )
            .unwrap_or(3);
            state.recent.iter().take(shown).copied().collect()
        };
        let repositories: Vec<Repository> = state
            .repositories
            .iter()
            .filter(|r| passes_filters(r))
            .cloned()
            .collect();

        let mut groups: Vec<Group> = Vec::new();
        // Corvene (`267-pinned-repositories`): the pinned repositories, by
        // name, above Recent (they stay in their owner groups too)
        if !filtering
            && state
                .flags
                .bool(corvene_core::flags::ids::PINNED_REPOSITORIES)
        {
            let pinned: Vec<_> = state
                .sorted_repositories()
                .into_iter()
                .filter(|r| r.pinned)
                .map(|r| (r.clone(), Vec::new(), false))
                .collect();
            if !pinned.is_empty() {
                groups.push(Group::new("Pinned", key(":pinned".into()), pinned));
            }
        }
        // Corvene (`268-ungrouped-repository-list`): without a typed filter,
        // one alphabetical group instead of the owner, host and Other groups
        let ungrouped = query.is_empty()
            && state
                .flags
                .bool(corvene_core::flags::ids::UNGROUPED_REPOSITORY_LIST);
        let mut all: Vec<(Repository, Vec<usize>, bool)> = Vec::new();
        // Corvene (`290-custom-repository-groups`): the repositories moved to
        // a group of their own, by group name (ignoring case), placed after
        // Pinned and Recent; they leave their owner groups
        let custom_groups = state
            .flags
            .bool(corvene_core::flags::ids::CUSTOM_REPOSITORY_GROUPS);
        let mut custom: std::collections::BTreeMap<String, CustomGroup> =
            std::collections::BTreeMap::new();
        let mut leading = groups.len();
        for group in group_repositories(&repositories, &state.indicators, &recent) {
            // GHD `FilterList`: each group's matches, best first
            let mut hits: Vec<(f32, Repository, Vec<usize>, bool)> = Vec::new();
            for item in group.items {
                let hit = match &regex {
                    // a `/pattern/` filter matches the name, no bold chars
                    Some(re) => re.is_match(&item.text[0]).then(|| (1.0, Vec::new())),
                    None if query.is_empty() => Some((1.0, Vec::new())),
                    // GHD `match` over the texts `[title, nameOf(r)]`; only
                    // the title is highlighted, so an `owner/name` hit shows
                    // no bold chars
                    None => corvene_core::filter::match_keys(&query, &item.text)
                        .map(|(score, matches)| (score, matches.title)),
                };
                if let Some((score, positions)) = hit {
                    if let Some(name) = item.repository.group.clone().filter(|_| {
                        custom_groups && group.identifier != RepositoryListGroup::Recent
                    }) {
                        custom
                            .entry(name.to_lowercase())
                            .or_insert_with(|| (name, Vec::new()))
                            .1
                            .push((score, item.repository, positions));
                        continue;
                    }
                    hits.push((score, item.repository, positions, item.needs_disambiguation));
                }
            }
            if hits.is_empty() {
                continue;
            }
            if !query.is_empty() {
                // stable: ties keep the list order
                hits.sort_by(|a, b| b.0.total_cmp(&a.0));
            }
            // Corvene (`291-recent-worktrees`): a Recent repository used in
            // several worktrees gets a row per worktree, most recent first
            if group.identifier == RepositoryListGroup::Recent
                && state.flags.bool(corvene_core::flags::ids::RECENT_WORKTREES)
            {
                hits = hits
                    .into_iter()
                    .flat_map(|(score, repo, positions, d)| {
                        let paths = recent_worktree_paths(state, &repo);
                        if paths.len() < 2 {
                            return vec![(score, repo, positions, d)];
                        }
                        paths
                            .into_iter()
                            .map(|path| {
                                let mut row = repo.clone();
                                row.path = path;
                                (score, row, positions.clone(), d)
                            })
                            .collect()
                    })
                    .collect();
            }
            if ungrouped && group.identifier != RepositoryListGroup::Recent {
                all.extend(hits.into_iter().map(|(_, r, p, _)| (r, p, false)));
                continue;
            }
            let k = key(match &group.identifier {
                RepositoryListGroup::Recent => ":recent".to_string(),
                RepositoryListGroup::Dotcom { owner } => format!("owner:{owner}"),
                RepositoryListGroup::Enterprise { host } => format!("host:{host}"),
                RepositoryListGroup::Other => ":other".to_string(),
            });
            let recent = group.identifier == RepositoryListGroup::Recent;
            groups.push(Group {
                title: group.identifier.label().to_string().into(),
                recent,
                key: k,
                collapsed: false,
                repos: hits.into_iter().map(|(_, r, p, d)| (r, p, d)).collect(),
            });
            if recent {
                leading = groups.len();
            }
        }
        for (ix, (_, (name, mut hits))) in custom.into_iter().enumerate() {
            if query.is_empty() {
                hits.sort_by_cached_key(|(_, r, _)| r.name().to_lowercase());
            } else {
                // stable: ties keep the name order
                hits.sort_by(|a, b| b.0.total_cmp(&a.0));
            }
            // a name shown twice in the group gets its owner prefix
            let mut names: HashMap<String, usize> = HashMap::new();
            for (_, r, _) in &hits {
                *names.entry(r.name()).or_default() += 1;
            }
            let repos = hits
                .into_iter()
                .map(|(_, r, p)| {
                    let twice = names.get(&r.name()).copied().unwrap_or(0) > 1;
                    (r, p, twice)
                })
                .collect();
            let k = key(format!("group:{}", name.to_lowercase()));
            groups.insert(leading + ix, Group::new(name, k, repos));
        }
        if !all.is_empty() {
            // by name, ignoring case (stable); a name shown twice gets its
            // owner prefix
            all.sort_by_cached_key(|(r, _, _)| r.name().to_lowercase());
            let mut names: HashMap<String, usize> = HashMap::new();
            for (r, _, _) in &all {
                *names.entry(r.name()).or_default() += 1;
            }
            for (r, _, disambiguate) in &mut all {
                *disambiguate = names.get(&r.name()).copied().unwrap_or(0) > 1;
            }
            groups.push(Group::new("Repositories", key(":all".into()), all));
        }
        // Corvene (`212-flat-repository-results`): while a query is typed,
        // one list without group headers, best match first
        if !query.is_empty()
            && state
                .flags
                .bool(corvene_core::flags::ids::FLAT_REPOSITORY_RESULTS)
        {
            // the Recent group's repositories are in their own groups too
            let mut repos: Vec<(Repository, Vec<usize>, bool)> = groups
                .into_iter()
                .filter(|g| !g.recent)
                .flat_map(|g| g.repos)
                .collect();
            let score = |r: &Repository| {
                corvene_core::filter::fuzzy_score(&query, &r.name()).unwrap_or(0.0)
            };
            // stable: equal scores keep the grouped order
            repos.sort_by(|a, b| score(&b.0).total_cmp(&score(&a.0)));
            return vec![Group::new(SharedString::default(), None, repos)];
        }
        if collapsible {
            let collapsed = &state.settings.collapsed_repository_groups;
            for group in &mut groups {
                if group.key.as_ref().is_some_and(|k| collapsed.contains(k)) {
                    group.collapsed = true;
                    group.repos.clear();
                }
            }
        }
        groups
    }

    /// Corvene (`266-collapsible-repository-groups`): a header click hides
    /// or shows the group's rows.
    fn toggle_group(&mut self, key: String, cx: &mut Context<Self>) {
        self.highlighted = None;
        Dispatcher::update_settings(cx, |s| {
            let collapsed = &mut s.collapsed_repository_groups;
            match collapsed.iter().position(|k| *k == key) {
                Some(ix) => {
                    collapsed.remove(ix);
                }
                None => collapsed.push(key),
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn row(
        &self,
        repo: &Repository,
        matched: &[usize],
        needs_disambiguation: bool,
        selected: bool,
        highlighted: bool,
        detail: Option<String>,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let t = cx.ghd();
        let id = repo.id;
        // Corvene (`288-dead-remote-indicator`): the last fetch found no
        // remote repository
        let remote_not_found = {
            let s = self.state.read(cx);
            s.flags
                .bool(corvene_core::flags::ids::DEAD_REMOTE_INDICATOR)
                && s.repo_states.get(&id).is_some_and(|rs| rs.remote_not_found)
        };
        let icon = if remote_not_found {
            Octicon::Alert
        } else {
            crate::icons::icon_for_repository(RepositoryOrCloning::Repository(repo))
        };
        let hover_bg = t.list_item_hover_background;
        let behind_accent = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::REPOSITORY_LIST_BEHIND_ACCENT);
        let (ahead_behind, changed_files) = indicators(self.state.read(cx), id);
        let has_changes = changed_files > 0;
        // `272-grouped-ahead-behind-counts` in the arrows' tooltip
        let repo_indicators = render_repo_indicators_with(ahead_behind, changed_files, &|n| {
            crate::toolbar::ahead_behind_count(n, self.state.read(cx))
        });
        // Corvene (`270-repository-list-stash-icon`): a stash the app shows
        // (the branch's own or, with `728-show-latest-other-stash`, a
        // command-line one, or another branch's) for an opened repository,
        // else the indicator refresh
        let has_stash = {
            let s = self.state.read(cx);
            s.flags
                .bool(corvene_core::flags::ids::REPOSITORY_LIST_STASH_ICON)
                && s.repo_states
                    .get(&id)
                    .filter(|rs| rs.info.is_some())
                    .map(|rs| rs.stash.is_some() || !rs.stashed_branches.is_empty())
                    .or_else(|| s.indicators.get(&id).map(|i| i.has_stash))
                    .unwrap_or(false)
        };
        // Corvene (`214-repository-list-branch`): the checked-out branch
        // (the loaded state for an opened repository, else the background
        // indicator refresh) joins the dimmed detail
        let branch = {
            let s = self.state.read(cx);
            s.flags
                .bool(corvene_core::flags::ids::REPOSITORY_LIST_BRANCH)
                .then(|| {
                    s.repo_states
                        .get(&id)
                        .and_then(|rs| rs.info.as_ref())
                        .and_then(|i| i.current_branch())
                        .map(|b| b.name.clone())
                        .or_else(|| s.indicators.get(&id).and_then(|i| i.branch.clone()))
                })
                .flatten()
        };
        let detail = match (detail, branch) {
            (Some(folder), Some(branch)) => Some(format!("{folder} · {branch}")),
            (folder, branch) => folder.or(branch),
        };
        // GHD `RepositoryListItem` aria label: name, changes, ahead/behind
        let mut label = repo.name();
        if has_changes {
            label.push_str(", uncommitted changes");
        }
        let count = |n: u32| crate::toolbar::ahead_behind_count(n, self.state.read(cx));
        if let Some(ab) = ahead_behind {
            label.push_str(&format!(
                ", {} ahead, {} behind",
                count(ab.ahead),
                count(ab.behind)
            ));
        }
        let (badge_bg, badge_text) = if selected {
            (
                t.list_item_selected_badge_background,
                t.list_item_selected_badge_text,
            )
        } else {
            (t.list_item_badge_background, t.list_item_badge_text)
        };
        // `.prefix` and the flag-213 / 214 detail: `--text-secondary-color`,
        // the selected text colour in a selected row
        let dim = HighlightStyle {
            color: Some(if selected || highlighted {
                t.box_selected_text
            } else {
                t.text_secondary
            }),
            ..Default::default()
        };
        // Corvene (`291-recent-worktrees`): a Recent row for another
        // worktree of the repository than the one it is in now
        let worktree_row = self
            .state
            .read(cx)
            .repository(id)
            .is_some_and(|r| r.path != repo.path);
        let row_id: ElementId = if worktree_row {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            repo.path.hash(&mut hasher);
            ("repo-worktree-row", hasher.finish()).into()
        } else {
            ("repo-row", id).into()
        };
        let menu_color = if selected || highlighted {
            t.box_selected_text
        } else {
            t.text_secondary
        };
        div()
            .id(row_id)
            .group("repository-row")
            .a11y_row(label, selected)
            .h(ROW_HEIGHT())
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .px(SPACING())
            .cursor_pointer()
            .when(selected, |d| {
                d.bg(t.box_selected_background)
                    .text_color(t.box_selected_text)
            })
            // the keyboard row (GHD's focused-list selection)
            .when(highlighted, |d| {
                d.bg(t.box_selected_active_background)
                    .text_color(t.box_selected_active_text)
                    // `623-contrast-focus-ring`
                    .when_some(crate::keyboard_nav::contrast_row_outline(cx), |d, ring| {
                        d.shadow(ring)
                    })
            })
            // `.list-item:hover` outranks `.list-item.selected` (flag 104 keeps it)
            .when(
                !(selected && crate::widgets::selection_keeps_colour_on_hover(cx)),
                move |d| d.hover(move |s| s.bg(hover_bg)),
            )
            // `renderTooltip`: the GitHub full name (or name) in bold, the
            // alias in parentheses, then the path
            .tooltip({
                let tooltip = repository_list_item_tooltip(repo);
                let bold = 0..tooltip.full_name.len();
                let mut text = tooltip.full_name;
                if let Some(alias) = &tooltip.alias {
                    text.push_str(&format!(" ({alias})"));
                }
                text.push('\n');
                text.push_str(&tooltip.path);
                // Corvene (`273-fork-parent-in-tooltip`)
                if let Some(parent) = crate::toolbar::fork_parent(repo, self.state.read(cx)) {
                    text.push_str(&format!("\nFork of {parent}"));
                }
                if remote_not_found {
                    text.push_str(
                        "\nThe remote repository was not found: it may have been deleted or \
                         renamed, or you no longer have access to it.",
                    );
                }
                crate::widgets::rich_tooltip(text, bold)
            })
            .tooltip_show_delay(crate::widgets::TOOLTIP_DELAY)
            .on_click({
                let row = repo.clone();
                move |_, _, cx| select_row(&row, cx)
            })
            // Corvene (`426-drag-repository-out`): the folder drags out of
            // the window
            .when(!repo.missing && crate::repository_drag::enabled(cx), |d| {
                crate::repository_drag::draggable(d, repo.path.clone(), repo.name().into())
            })
            .on_mouse_down(MouseButton::Right, {
                let repo = repo.clone();
                move |ev: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    crate::native_menu::show_context_menu(
                        repository_menu_items(&repo, cx),
                        ev.position,
                        window,
                        cx,
                    );
                }
            })
            .child(octicon(icon, t.text).mr(SPACING_HALF()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(FONT_SIZE())
                    .when(repo.alias.is_some(), |d| d.italic())
                    .child({
                        // the `owner/` prefix (`needsDisambiguation`), the
                        // name with the filter's matched chars in bold
                        // (`HighlightText`), then Corvene's
                        // (`213-duplicate-names-show-path`) telling folders
                        let name = repository_list_item_name(repo, needs_disambiguation);
                        let prefix_len = name.prefix.as_ref().map_or(0, String::len);
                        let mut text = name.text;
                        let mut highlights: Vec<_> = bold_ranges(&text[prefix_len..], matched)
                            .into_iter()
                            .map(|(r, style)| (r.start + prefix_len..r.end + prefix_len, style))
                            .collect();
                        if prefix_len > 0 {
                            highlights.insert(0, (0..prefix_len, dim));
                        }
                        if let Some(detail) = detail {
                            let start = text.len() + 2;
                            text.push_str(&format!("  {detail}"));
                            highlights.push((start..text.len(), dim));
                        }
                        StyledText::new(text).with_highlights(highlights)
                    }),
            )
            // `.repo-indicators`: (Corvene's stash icon,) ahead / behind
            // arrows, then the changes dot
            .when(
                has_stash || repo_indicators.changes || repo_indicators.ahead_behind.is_some(),
                |d| {
                    d.child(
                        div()
                            .flex_none()
                            .mr(SPACING_HALF())
                            .flex()
                            .flex_row()
                            .items_center()
                            .when(has_stash, |d| {
                                d.child(
                                    div()
                                        .id(("repo-stash", id))
                                        .flex_none()
                                        .ml(SPACING_HALF())
                                        .child(octicon(
                                            Octicon::Stash,
                                            if selected || highlighted {
                                                t.box_selected_text
                                            } else {
                                                t.text_secondary
                                            },
                                        ))
                                        .ghd_tooltip("Stashed changes"),
                                )
                            })
                            .when_some(
                                repo_indicators
                                    .ahead_behind
                                    .zip(repo_indicators.ahead_behind_tooltip),
                                |d, (arrows, tooltip)| {
                                    // `renderAheadBehindIndicator`: arrows only, 12 px
                                    // tall (darwin; the base rule's 16 px elsewhere)
                                    d.child(
                                        div()
                                            .id(("repo-ahead-behind", id))
                                            .ghd_tooltip(tooltip)
                                            // as far from the stash glyph (1 px inset)
                                            // as the changes dot (5 px + its 4 px inset)
                                            .when(has_stash, |d| d.ml(zpx(8.)))
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .h(zpx(if cfg!(target_os = "macos") {
                                                12.
                                            } else {
                                                16.
                                            }))
                                            .px(zpx(6.))
                                            .rounded(zpx(8.))
                                            .bg(badge_bg)
                                            .children(arrows.into_iter().map(|arrow| {
                                                // flag `215-repository-list-behind-accent`:
                                                // commits to pull show in the success colour
                                                let color = if arrow == Octicon::ArrowDown
                                                    && behind_accent
                                                    && !selected
                                                {
                                                    t.status_success
                                                } else {
                                                    badge_text
                                                };
                                                octicon(arrow, color).size(zpx(12.))
                                            })),
                                    )
                                },
                            )
                            .when(repo_indicators.changes, |d| {
                                // `.change-indicator-wrapper`: 5 px in, at least 12 px wide
                                d.child(
                                    div()
                                        .id(("repo-changes", id))
                                        .ghd_tooltip(
                                            "There are uncommitted changes in this repository",
                                        )
                                        .ml(SPACING_HALF())
                                        .min_w(zpx(12.))
                                        .flex()
                                        .justify_center()
                                        .items_center()
                                        .child(octicon(Octicon::DotFill, t.tab_bar_active)),
                                )
                            }),
                    )
                },
            )
            // Shift+F10 / Menu
            .when(
                highlighted || (selected && self.highlighted.is_none()),
                |d| d.child(self.menu_anchor.track()),
            )
            // `621-context-menu-buttons`
            .when(crate::context_menu::row_menu_buttons(cx), |d| {
                d.child(crate::context_menu::row_menu_button(
                    "row-menu",
                    "repository-row",
                    selected || highlighted,
                    menu_color,
                ))
            })
    }

    /// Corvene (`207-repository-status-filter`): the filter options menu.
    fn open_filter_menu(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::context_menu::MenuItem;
        let this = cx.entity().downgrade();
        let toggle = move |pick: fn(&mut Self) -> &mut bool| {
            let this = this.clone();
            move |_: &mut Window, cx: &mut App| {
                this.update(cx, |f, cx| {
                    let flag = pick(f);
                    *flag = !*flag;
                    cx.notify();
                })
                .ok();
            }
        };
        let flags = &self.state.read(cx).flags;
        let mut items = Vec::new();
        if flags.bool(corvene_core::flags::ids::REPOSITORY_STATUS_FILTER) {
            items.extend([
                MenuItem::checkbox(
                    "Uncommitted changes",
                    self.only_changed,
                    toggle(|f| &mut f.only_changed),
                ),
                MenuItem::checkbox(
                    "Commits to push or pull",
                    self.only_ahead_behind,
                    toggle(|f| &mut f.only_ahead_behind),
                ),
            ]);
        }
        // Corvene (`208-repository-fork-filter`)
        if flags.bool(corvene_core::flags::ids::REPOSITORY_FORK_FILTER) {
            if !items.is_empty() {
                items.push(MenuItem::separator());
            }
            items.extend([
                MenuItem::checkbox("Forks", self.only_forks, toggle(|f| &mut f.only_forks)),
                MenuItem::checkbox(
                    "Not forks",
                    self.only_sources,
                    toggle(|f| &mut f.only_sources),
                ),
            ]);
        }
        crate::native_menu::show_context_menu(items, position, window, cx);
    }

    /// Corvene (`225-clone-prefills-filter`): the filter text that Add ›
    /// Clone Repository… puts in the clone dialog's filter box.
    fn clone_filter(&self, cx: &App) -> Option<String> {
        let text = self.filter.read(cx).value().trim().to_string();
        let enabled = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::CLONE_PREFILLS_FILTER);
        (enabled && !text.is_empty()).then_some(text)
    }

    fn add_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.ghd();
        let item = |id: &'static str, label: &'static str, on_click: fn(&mut Window, &mut App)| {
            let hover_bg = t.box_hover_background;
            div()
                .id(id)
                .h(ROW_HEIGHT())
                .px(SPACING())
                .flex()
                .items_center()
                .cursor_pointer()
                .hover(move |s| s.bg(hover_bg))
                .on_click(move |_, window, cx| on_click(window, cx))
                .child(label)
        };
        div()
            .id("add-menu")
            .absolute()
            .top(SPACING() + TEXT_FIELD_HEIGHT() + zpx(4.))
            .right(SPACING())
            .w(zpx(240.))
            .py(zpx(4.))
            .flex()
            .flex_col()
            .bg(t.box_background)
            .border_1()
            .border_color(t.box_border)
            .rounded(BORDER_RADIUS())
            .shadow_md()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(item(
                "add-clone",
                mac_or("Clone Repository…", "Clone repository…"),
                |_, cx| Dispatcher::show_popup(Popup::CloneRepository { url: None }, cx),
            ))
            .child(item(
                "add-create",
                mac_or("Create New Repository…", "Create new repository…"),
                |_, cx| Dispatcher::show_popup(Popup::CreateRepository { path: None }, cx),
            ))
            .child(item(
                "add-existing",
                mac_or("Add Existing Repository…", "Add existing repository…"),
                |_, cx| {
                    Dispatcher::close_foldout(cx);
                    Dispatcher::prompt_add_repository(cx);
                },
            ))
    }
}

/// GHD `generateRepositoryListContextMenu`.
fn repository_menu_items(repo: &Repository, cx: &App) -> Vec<crate::context_menu::MenuItem> {
    use crate::context_menu::{IS_MAC, MenuItem, labels, mac_or};
    let state = AppState::global(cx).read(cx);
    let (editor, shell) = (state.editor_label(), state.shell_label());
    let confirm = state.settings.confirm_repository_removal;
    let id = repo.id;
    let remote_page = Dispatcher::non_github_remote_web_url(id, cx).is_some();
    let missing = repo.missing;
    let path = repo.path.clone();
    let (name, copy_path, shell_path, reveal, editor_path) = (
        repo.name(),
        path.to_string_lossy().to_string(),
        path.clone(),
        path.clone(),
        path,
    );
    let verb = if repo.alias.is_some() {
        "Change"
    } else {
        "Create"
    };
    let alias_label = if IS_MAC {
        format!("{verb} Alias")
    } else {
        format!("{verb} alias")
    };
    let mut items = vec![MenuItem::new(alias_label, move |_, cx| {
        Dispatcher::close_foldout(cx);
        Dispatcher::show_popup(Popup::ChangeRepositoryAlias { repo: id }, cx)
    })];
    if repo.alias.is_some() {
        items.push(MenuItem::new(
            mac_or("Remove Alias", "Remove alias"),
            move |_, cx| Dispatcher::change_repository_alias(id, None, cx),
        ));
    }
    // Corvene (`267-pinned-repositories`)
    if state
        .flags
        .bool(corvene_core::flags::ids::PINNED_REPOSITORIES)
    {
        let pinned = repo.pinned;
        items.push(MenuItem::new(
            if pinned { "Unpin" } else { "Pin" },
            move |_, cx| Dispatcher::set_repository_pinned(id, !pinned, cx),
        ));
    }
    // Corvene (`290-custom-repository-groups`)
    if state
        .flags
        .bool(corvene_core::flags::ids::CUSTOM_REPOSITORY_GROUPS)
    {
        items.push(MenuItem::new(
            mac_or("Move to Group…", "Move to group…"),
            move |_, cx| {
                Dispatcher::close_foldout(cx);
                Dispatcher::show_popup(Popup::MoveRepositoryToGroup { repo: id }, cx)
            },
        ));
        if repo.group.is_some() {
            items.push(MenuItem::new(
                mac_or("Remove from Group", "Remove from group"),
                move |_, cx| Dispatcher::set_repository_group(id, None, cx),
            ));
        }
    }
    items.extend([
        // `buildWorktreeMenuItems` (worktree support is on)
        MenuItem::new(mac_or("Show Worktrees", "Show worktrees"), move |_, cx| {
            Dispatcher::select_repository(id, cx);
            Dispatcher::toggle_foldout(corvene_core::Foldout::Worktree, cx);
        }),
        MenuItem::new(
            mac_or("New Worktree…", "New worktree…"),
            move |_, cx| {
                Dispatcher::close_foldout(cx);
                Dispatcher::show_popup(
                    Popup::AddWorktree {
                        repo: id,
                        initial_branch_name: None,
                        initial_worktree_name: None,
                    },
                    cx,
                )
            },
        ),
        MenuItem::new(mac_or("Copy Repo Name", "Copy repo name"), move |_, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(name.clone()))
        }),
        MenuItem::new(mac_or("Copy Repo Path", "Copy repo path"), move |_, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(copy_path.clone()))
        }),
        MenuItem::separator(),
        // `262-view-on-remote`: "View on Remote" for other hosts
        MenuItem::new(
            if repo.github.is_none() && remote_page {
                mac_or("View on Remote", "View on remote")
            } else {
                "View on GitHub"
            },
            move |_, cx| Dispatcher::view_on_github(id, cx),
        )
        .enabled(repo.github.is_some() || remote_page),
        MenuItem::new(labels::open_in(&shell), move |_, cx| {
            Dispatcher::open_in_shell(&shell_path, cx)
        })
        .enabled(!missing),
        MenuItem::new(labels::REVEAL_IN_FILE_MANAGER, move |_, cx| {
            Dispatcher::show_repository(&reveal, cx)
        })
        .enabled(!missing),
        MenuItem::new(labels::open_in(&editor), move |_, cx| {
            Dispatcher::open_in_editor(editor_path.clone(), cx)
        })
        .enabled(!missing),
    ]);
    // Corvene (`524-open-repository-with-editor`)
    let editors = Dispatcher::menu_editors(state);
    if !editors.is_empty() {
        let items_for = |(label, editor): (String, corvene_core::EditorChoice)| {
            let path = repo.path.clone();
            MenuItem::new(label, move |_, cx| {
                Dispatcher::close_foldout(cx);
                Dispatcher::open_in_editor_with(path.clone(), editor.clone(), cx)
            })
        };
        items.push(
            MenuItem::submenu(
                mac_or("Open in Editor", "Open in editor"),
                editors.into_iter().map(items_for).collect(),
            )
            .enabled(!missing),
        );
    }
    // Corvene (`429-multiple-windows`, `430-repository-tabs`): the
    // repository next to this one (desktop/desktop#22427)
    let (new_window, new_tab) = (
        state.flags.bool(corvene_core::flags::ids::MULTIPLE_WINDOWS),
        state.flags.bool(corvene_core::flags::ids::REPOSITORY_TABS),
    );
    if new_window || new_tab {
        items.push(MenuItem::separator());
    }
    if new_tab {
        items.push(
            MenuItem::new(
                mac_or("Open in New Tab", "Open in new tab"),
                move |_, cx| {
                    Dispatcher::close_foldout(cx);
                    Dispatcher::open_tab(id, cx);
                },
            )
            .enabled(!missing),
        );
    }
    if new_window {
        items.push(
            MenuItem::new(
                mac_or("Open in New Window", "Open in new window"),
                move |_, cx| {
                    Dispatcher::close_foldout(cx);
                    crate::windows::open_repository_in_new_window(id, cx);
                },
            )
            .enabled(!missing),
        );
    }
    items.extend([
        MenuItem::separator(),
        MenuItem::new(
            if confirm { "Remove…" } else { "Remove" },
            move |_, cx| {
                Dispatcher::close_foldout(cx);
                Dispatcher::request_remove_repository(id, cx)
            },
        ),
    ]);
    // Corvene (`269-bulk-remove-repositories`)
    if state.repositories.len() > 1
        && state
            .flags
            .bool(corvene_core::flags::ids::BULK_REMOVE_REPOSITORIES)
    {
        items.push(MenuItem::new(
            mac_or("Remove Repositories…", "Remove repositories…"),
            move |_, cx| {
                Dispatcher::close_foldout(cx);
                Dispatcher::show_popup(Popup::RemoveRepositories { ticked: Some(id) }, cx)
            },
        ));
    }
    // Corvene (`216-remove-all-missing-repositories`): on a missing row,
    // remove every repository Corvene cannot find (without confirmation,
    // as GHD removes one missing repository)
    let missing_ids: Vec<u64> = state
        .repositories
        .iter()
        .filter(|r| r.missing)
        .map(|r| r.id)
        .collect();
    if missing
        && missing_ids.len() > 1
        && state
            .flags
            .bool(corvene_core::flags::ids::REMOVE_ALL_MISSING_REPOSITORIES)
    {
        items.push(MenuItem::new(
            if IS_MAC {
                format!("Remove All {} Missing Repositories", missing_ids.len())
            } else {
                format!("Remove all {} missing repositories", missing_ids.len())
            },
            move |_, cx| {
                Dispatcher::close_foldout(cx);
                for id in &missing_ids {
                    Dispatcher::remove_repository(*id, cx);
                }
            },
        ));
    }
    items
}

/// The Add button's items (`onNewRepositoryButtonClick`).
fn add_menu_items(clone_filter: Option<String>) -> Vec<crate::context_menu::MenuItem> {
    use crate::context_menu::{MenuItem, mac_or};
    vec![
        MenuItem::new(
            mac_or("Clone Repository…", "Clone repository…"),
            move |_, cx| {
                // Corvene (`225-clone-prefills-filter`)
                if let Some(text) = &clone_filter {
                    crate::dialogs::clone_repository::prefill_filter(text.clone());
                }
                Dispatcher::show_popup(Popup::CloneRepository { url: None }, cx)
            },
        ),
        MenuItem::new(
            mac_or("Create New Repository…", "Create new repository…"),
            |_, cx| Dispatcher::show_popup(Popup::CreateRepository { path: None }, cx),
        ),
        MenuItem::new(
            mac_or("Add Existing Repository…", "Add existing repository…"),
            |_, cx| {
                Dispatcher::close_foldout(cx);
                Dispatcher::prompt_add_repository(cx);
            },
        ),
    ]
}

impl Render for RepositoryFoldout {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd();
        let selected = self.state.read(cx).selected;
        let groups = self.groups(cx);
        let has_repos = !self.state.read(cx).repositories.is_empty();
        let add_open = self.add_menu_open;
        // the filter button: `207-repository-status-filter` or
        // `208-repository-fork-filter`
        let (status_filter, filtering) = {
            let flags = &self.state.read(cx).flags;
            let status = flags.bool(corvene_core::flags::ids::REPOSITORY_STATUS_FILTER);
            let fork = flags.bool(corvene_core::flags::ids::REPOSITORY_FORK_FILTER);
            (
                status || fork,
                (status && (self.only_changed || self.only_ahead_behind))
                    || (fork && (self.only_forks || self.only_sources)),
            )
        };
        let highlighted = self.highlighted;
        let mut row_ix = 0;
        let show_paths = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::DUPLICATE_NAMES_SHOW_PATH);

        div()
            .id("repository-list")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.add_menu_open {
                        this.add_menu_open = false;
                        cx.notify();
                    }
                }),
            )
            .child(
                // `.filter-field-row`: [🔍 Filter][Add ▾]
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING())
                    .p(SPACING())
                    .key_context("RepositoryFilter")
                    .on_action(
                        cx.listener(|this, _: &SelectNextFile, _, cx| this.move_highlight(1, cx)),
                    )
                    .on_action(cx.listener(|this, _: &SelectPreviousFile, _, cx| {
                        this.move_highlight(-1, cx)
                    }))
                    .on_action(
                        cx.listener(|this, _: &FilterListPick, _, cx| this.pick_highlighted(cx)),
                    )
                    .on_action(self.menu_anchor.action_handler())
                    .child(crate::widgets::filter_text_box(
                        "repo-filter",
                        &self.filter,
                        Some(octicon(Octicon::Search, t.text_secondary)),
                        window,
                        cx,
                    ))
                    // Corvene (`207-repository-status-filter`): a menu of
                    // status filters, blue while one is on
                    .when(status_filter, |d| {
                        d.child(
                            button("repository-filter-options", "", cx)
                                .flex_none()
                                .ghd_tooltip("Filter options")
                                .child(octicon(
                                    Octicon::Filter,
                                    if filtering {
                                        t.tab_bar_active
                                    } else {
                                        t.secondary_button_text
                                    },
                                ))
                                .on_click(cx.listener(|this, ev: &ClickEvent, window, cx| {
                                    cx.stop_propagation();
                                    this.open_filter_menu(ev.position(), window, cx);
                                })),
                        )
                    })
                    .child(
                        button("add-repository", "Add", cx)
                            .flex_none()
                            .gap(zpx(5.))
                            .child(
                                octicon(Octicon::TriangleDown, t.secondary_button_text)
                                    .size(zpx(12.)),
                            )
                            .on_click(cx.listener(|this, ev: &ClickEvent, window, cx| {
                                cx.stop_propagation();
                                // GHD `onNewRepositoryButtonClick`: a native
                                // contextual menu at the pointer
                                let clone_filter = this.clone_filter(cx);
                                crate::native_menu::show_context_menu(
                                    add_menu_items(clone_filter),
                                    ev.position(),
                                    window,
                                    cx,
                                );
                            })),
                    ),
            )
            .child(
                div()
                    .id("repository-list-scroll")
                    // a `List` node owning the repository rows
                    .role(Role::List)
                    .aria_label("Repositories")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .when(!has_repos, |d| {
                        d.child(
                            div()
                                .p(SPACING())
                                .text_color(t.text_secondary)
                                .child("No repositories yet. Use Add to get started."),
                        )
                    })
                    .when(has_repos && groups.is_empty(), |d| {
                        d.child(
                            div()
                                .p(SPACING())
                                .w_full()
                                .text_center()
                                .text_color(t.text_secondary)
                                .child("Sorry, I can't find that repository"),
                        )
                    })
                    .children(groups.into_iter().enumerate().map(|(group_ix, group)| {
                        let first = row_ix;
                        row_ix += group.repos.len();
                        let mut details = if show_paths {
                            duplicate_name_paths(group.repos.iter().map(|(r, _, _)| r))
                        } else {
                            HashMap::new()
                        };
                        // a repository can be listed under Recent and its
                        // owner: the group id keeps the rows' ids (and a11y
                        // nodes) unique
                        div()
                            .id(("repo-group", group_ix))
                            .flex()
                            .flex_col()
                            // a flat result list (`212-flat-repository-results`)
                            // has no header
                            .when(!group.title.is_empty(), |d| {
                                d.child(
                                    // `.filter-list-group-header`
                                    div()
                                        .id(("repo-group-header", group_ix))
                                        .h(ROW_HEIGHT())
                                        .pt(SPACING())
                                        .px(SPACING())
                                        .flex()
                                        .items_center()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_size(FONT_SIZE())
                                        .truncate()
                                        // Corvene (`266-collapsible-repository-groups`)
                                        .when_some(group.key.clone(), |d, key| {
                                            d.cursor_pointer()
                                                .child(
                                                    octicon(
                                                        if group.collapsed {
                                                            Octicon::ChevronRight
                                                        } else {
                                                            Octicon::ChevronDown
                                                        },
                                                        t.text_secondary,
                                                    )
                                                    .size(zpx(12.))
                                                    .mr(SPACING_HALF()),
                                                )
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.toggle_group(key.clone(), cx)
                                                }))
                                        })
                                        .child(group.title.clone()),
                                )
                            })
                            .children(group.repos.iter().enumerate().map(
                                |(ix, (repo, matched, needs_disambiguation))| {
                                    // `291-recent-worktrees`: a repository
                                    // listed once per worktree names it
                                    let state = self.state.read(cx);
                                    let current_path = state.repository(repo.id).map(|r| &r.path);
                                    let worktree_rows = group.recent
                                        && group
                                            .repos
                                            .iter()
                                            .filter(|(r, _, _)| r.id == repo.id)
                                            .count()
                                            > 1;
                                    // (the folder name, unless the row's name is that already)
                                    let detail = if worktree_rows {
                                        repo.path
                                            .file_name()
                                            .map(|n| n.to_string_lossy().into_owned())
                                            .filter(|folder| *folder != repo.name())
                                    } else {
                                        details.remove(&repo.id)
                                    };
                                    self.row(
                                        repo,
                                        matched,
                                        *needs_disambiguation,
                                        selected == Some(repo.id)
                                            && current_path == Some(&repo.path),
                                        highlighted == Some(first + ix),
                                        detail,
                                        cx,
                                    )
                                },
                            ))
                    }))
                    .with_scrollbar_handle(&self.scroll),
            )
            .when(add_open, |d| d.child(self.add_menu(cx)))
    }
}

/// Select a list row's repository; a `291-recent-worktrees` row of another
/// worktree also switches the repository to it.
fn select_row(row: &Repository, cx: &mut App) {
    let current = AppState::global(cx)
        .read(cx)
        .repository(row.id)
        .map(|r| r.path.clone());
    match current {
        Some(current) if current != row.path => {
            Dispatcher::select_recent_worktree(row.id, row.path.clone(), cx)
        }
        _ => Dispatcher::select_repository(row.id, cx),
    }
}

/// `291-recent-worktrees`: the worktrees `repo` was last used in (its
/// current one included), most recent first, at most three; worktrees the
/// last refresh no longer lists are left out.
fn recent_worktree_paths(state: &AppState, repo: &Repository) -> Vec<std::path::PathBuf> {
    let known = state
        .repo_states
        .get(&repo.id)
        .map(|rs| rs.worktrees.as_slice())
        .unwrap_or_default();
    let mut paths: Vec<std::path::PathBuf> = Vec::new();
    for (id, path) in &state.recent_worktrees {
        if *id != repo.id || paths.contains(path) {
            continue;
        }
        if *path != repo.path && !known.is_empty() && !known.iter().any(|w| w.path == *path) {
            continue;
        }
        paths.push(path.clone());
    }
    if !paths.contains(&repo.path) {
        paths.insert(0, repo.path.clone());
    }
    paths.truncate(3);
    paths
}

/// `HighlightText`'s bold ranges (bytes of `text`) for the matched char
/// `positions`, as `crate::autocompletion::highlighted` draws them.
#[doc(hidden)]
pub fn bold_ranges(
    text: &str,
    positions: &[usize],
) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
    let bold = HighlightStyle {
        font_weight: Some(FontWeight::BOLD),
        ..Default::default()
    };
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    for (ci, (bi, c)) in text.char_indices().enumerate() {
        if !positions.contains(&ci) {
            continue;
        }
        let end = bi + c.len_utf8();
        match ranges.last_mut() {
            Some(last) if last.end == bi => last.end = end,
            _ => ranges.push(bi..end),
        }
    }
    ranges.into_iter().map(|r| (r, bold)).collect()
}

/// The row's indicators: ahead / behind (when either is non-zero) and the
/// number of uncommitted changes, from the loaded state of a repository a
/// window shows, else the indicator refresh. A repository no window shows
/// keeps the state it last loaded, which goes stale (desktop/desktop#22997).
fn indicators(s: &AppState, id: u64) -> (Option<corvene_core::AheadBehind>, usize) {
    let indicator = s.indicators.get(&id);
    let rs = s
        .repo_states
        .get(&id)
        .filter(|_| indicator.is_none() || s.workspace_showing(id).is_some());
    let ab = rs
        .and_then(|r| r.ahead_behind)
        .or_else(|| indicator.and_then(|i| i.ahead_behind))
        .filter(|ab| ab.ahead > 0 || ab.behind > 0);
    let changed_files = rs
        .and_then(|r| r.status.as_deref())
        .map(|st| st.files.len())
        .or_else(|| indicator.map(|i| i.changed_files))
        .unwrap_or(0);
    (ab, changed_files)
}

/// Corvene (`213-duplicate-names-show-path`): for repositories whose names
/// repeat within `repos`, the trailing directories of their parent paths
/// that tell them apart (`fork-a` for `~/fork-a/app` beside `~/fork-b/app`).
fn duplicate_name_paths<'a>(
    repos: impl IntoIterator<Item = &'a Repository>,
) -> HashMap<u64, String> {
    let repos: Vec<&Repository> = repos.into_iter().collect();
    let parents = |r: &Repository| -> Vec<String> {
        r.path
            .parent()
            .map(|p| {
                p.components()
                    .rev()
                    .filter_map(|c| match c {
                        std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut out = HashMap::new();
    for &repo in &repos {
        let name = repo.name().to_lowercase();
        let others: Vec<Vec<String>> = repos
            .iter()
            .filter(|r| r.id != repo.id && r.name().to_lowercase() == name)
            .map(|r| parents(r))
            .collect();
        if others.is_empty() {
            continue;
        }
        let mine = parents(repo);
        for k in 1..=mine.len() {
            let unique = others.iter().all(|p| p.len() < k || p[..k] != mine[..k]);
            if unique || k == mine.len() {
                let shown: Vec<&str> = mine[..k].iter().rev().map(String::as_str).collect();
                out.insert(repo.id, shown.join("/"));
                break;
            }
        }
    }
    out
}

/// Corvene (`612-navigation-shortcuts`): the repositories in the list's
/// order without the Recent group ([`group_repositories`]: owner groups,
/// Enterprise hosts, then Other; by name within a group; by name alone with
/// `268-ungrouped-repository-list`), for ⇧⌘] / ⇧⌘[.
pub fn list_order(state: &AppState) -> Vec<u64> {
    let order: Vec<u64> = if state
        .flags
        .bool(corvene_core::flags::ids::UNGROUPED_REPOSITORY_LIST)
    {
        state.sorted_repositories().iter().map(|r| r.id).collect()
    } else {
        group_repositories(&state.repositories, &HashMap::new(), &[])
            .into_iter()
            .flat_map(|g| g.items)
            .map(|item| item.repository.id)
            .collect()
    };
    // `290-custom-repository-groups`: the custom groups come first, by name
    if !state
        .flags
        .bool(corvene_core::flags::ids::CUSTOM_REPOSITORY_GROUPS)
    {
        return order;
    }
    let group_of = |id: u64| {
        state
            .repository(id)
            .and_then(|r| r.group.as_ref())
            .map(|g| g.to_lowercase())
    };
    let mut grouped: Vec<(String, String, u64)> = order
        .iter()
        .filter_map(|&id| {
            let name = state.repository(id)?.name().to_lowercase();
            group_of(id).map(|g| (g, name, id))
        })
        .collect();
    grouped.sort();
    grouped
        .into_iter()
        .map(|(_, _, id)| id)
        .chain(order.iter().copied().filter(|&id| group_of(id).is_none()))
        .collect()
}

/// The repository `step` places after `current` in `order`, wrapping.
pub fn step_repository(order: &[u64], current: Option<u64>, step: isize) -> Option<u64> {
    if order.is_empty() {
        return None;
    }
    let n = order.len() as isize;
    let next = match current.and_then(|id| order.iter().position(|r| *r == id)) {
        Some(ix) => (ix as isize + step).rem_euclid(n),
        None if step < 0 => n - 1,
        None => 0,
    };
    order.get(next as usize).copied()
}

/// GHD `commitGrammar`: "1 commit" / "N commits" (N through `count`,
/// `272-grouped-ahead-behind-counts`).
fn commit_grammar(n: u32, count: &dyn Fn(u32) -> String) -> String {
    if n == 1 {
        "1 commit".to_string()
    } else {
        format!("{} commits", count(n))
    }
}

#[cfg(test)]
mod tests {
    use super::{duplicate_name_paths, step_repository};

    fn repo(id: u64, path: &str) -> corvene_core::Repository {
        corvene_core::Repository::new(id, path)
    }

    #[test]
    fn duplicate_names_get_the_parent_dirs_that_differ() {
        let repos = [
            repo(1, "/w/fork-a/app"),
            repo(2, "/w/fork-b/app"),
            repo(3, "/x/src/lib"),
            repo(4, "/y/src/lib"),
            repo(5, "/w/solo"),
        ];
        let paths = duplicate_name_paths(&repos);
        assert_eq!(paths.get(&1).map(String::as_str), Some("fork-a"));
        assert_eq!(paths.get(&2).map(String::as_str), Some("fork-b"));
        assert_eq!(paths.get(&3).map(String::as_str), Some("x/src"));
        assert_eq!(paths.get(&4).map(String::as_str), Some("y/src"));
        assert!(!paths.contains_key(&5));
    }

    #[test]
    fn enterprise_groups_are_named_by_host() {
        let github = |endpoint: &str| corvene_core::GitHubRepository {
            endpoint: endpoint.into(),
            owner: "o".into(),
            name: "n".into(),
            html_url: String::new(),
            clone_url: String::new(),
            default_branch: None,
            private: false,
            fork: false,
            parent: None,
            archived: false,
            permissions: None,
            allow_forking: None,
            node_id: None,
        };
        let group = |endpoint: &str| {
            let mut r = repo(1, "/w/n");
            r.github = Some(github(endpoint));
            super::group_for_repository(&r)
        };
        // `getHTMLURL` keeps the scheme and host name alone
        assert_eq!(
            group("https://GHE.example.com:8443/api/v3"),
            super::RepositoryListGroup::Enterprise {
                host: "ghe.example.com".into()
            }
        );
        assert_eq!(
            group("https://api.octo.ghe.com"),
            super::RepositoryListGroup::Enterprise {
                host: "octo.ghe.com".into()
            }
        );
        assert_eq!(
            group("https://api.github.com"),
            super::RepositoryListGroup::Dotcom { owner: "o".into() }
        );
    }

    #[test]
    fn steps_wrap_around_the_list() {
        let order = [3, 1, 2];
        assert_eq!(step_repository(&order, Some(1), 1), Some(2));
        assert_eq!(step_repository(&order, Some(2), 1), Some(3));
        assert_eq!(step_repository(&order, Some(3), -1), Some(2));
        assert_eq!(step_repository(&order, None, 1), Some(3));
        assert_eq!(step_repository(&order, None, -1), Some(2));
        assert_eq!(step_repository(&[], Some(1), 1), None);
    }
}
