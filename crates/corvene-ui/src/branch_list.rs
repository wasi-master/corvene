//! Branch foldout - GHD `ui/branches/{branches-container,branch-list,
//! branch-list-item,group-branches,no-branches}.tsx`
//! (`styles/ui/_branches.scss`, `_no-branches.scss`, `_filter-list.scss`):
//! `[🔍 Filter][New Branch]`, groups Default Branch / Recent Branches /
//! Other Branches, 29 px rows (check or branch icon, name, relative date) and
//! the "Choose a branch to merge into <current>" footer. Remote branches a
//! local branch tracks are hidden behind it (GHD `mergeRemoteAndLocalBranches`,
//! `lib/stores/git-store.ts`). GitHub repositories
//! get the Branches / Pull Requests tab bar (`branches-container.tsx`); the
//! pull request rows come from `pull_request_list.rs`.
//!
//! Hovering a pull request row for 250 ms shows its quick view
//! (`onMouseEnterPullRequestListItem`); leaving the row hides it after 500 ms
//! unless the pointer reaches the quick view, and leaving the quick view
//! hides it at once (`onMouseLeavePullRequestQuickView`).
//!
//! Deviations: dates are the tip's committer date (GHD: author date); Other
//! Branches can be sorted newest first (`848-branch-list-sort-by-date`).
//! The filter ignores an `owner:` prefix (`849-branch-filter-strips-owner`).
//! Rows can tell local-only, tracked and remote-only branches apart by icon
//! (`850-branch-list-local-remote-icons`), and a filter-row toggle can
//! narrow the list to remote branches (`851-branch-list-remote-only`).
//! The context menu can start a rebase onto the branch
//! (`856-branch-menu-rebase-onto`).
//! Deviation (`852-branch-upstream-gone`): a local branch whose upstream was
//! deleted on the remote shows a cloud-offline icon after its name.
//! Deviation (`853-branch-list-ahead-behind`): local branch rows show their
//! commits to push / pull ("2↑ 1↓") or an upload icon when unpublished.
//! Deviation (`854-branch-list-stash-icon`): a local branch with a Desktop
//! stash shows the stash icon after its name (GHD `branch-list-item.tsx` does
//! not).
//! Deviation (`335-pull-request-list-filters`): a menu beside the Pull
//! Requests filter box narrows the list to the pull requests created by,
//! awaiting a review from or assigned to the signed-in user.
//! Deviation (`1201-branch-list-tip-author`): rows can name the newest
//! commit's author after the date.
//! Deviation (`899-tags-in-branch-list`): with a filter typed, matching tags
//! follow the branches in a Tags group; choosing one checks out its commit.
//! Deviation (`898-branch-list-folders`): Other Branches sharing a prefix
//! before the first `/` sit under collapsible folder rows (flat while
//! filtering).
//! Deviation (`897-pinned-branches`): Pin / Unpin in the context menu and a
//! Pinned group below the default branch (hidden while filtering).
//! Deviation (`896-branch-upstream-gone-group`): local branches whose
//! upstream was deleted on the remote are grouped last under "Deleted on
//! Remote" (GHD lists them among the others).
//! Deviation (`895-bulk-delete-branches`): ⌘-click / ⇧-click select several
//! local branches (GHD's list selects one row) and their context menu
//! deletes them together.
//! Deviation (`1204-restore-stash-from-other-branch`): the menu of a branch
//! with a stash has Restore Stash Here (GHD restores a stash only on its own
//! branch).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, UNIX_EPOCH};

use corvene_core::filter::fuzzy_score;
use corvene_core::{AppState, Branch, BranchKind, BranchesTab, Dispatcher, Popup, Tip};
use gpui_kit::component::input::InputState;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::actions::{FilterListPick, SelectNextFile, SelectPreviousFile};
use crate::context_menu::mac_or;
use crate::widgets::GhdTooltip;
use crate::widgets::IconButtonA11y;

use crate::icons::{Octicon, octicon, spin};
use crate::pull_request_list::{
    QUICK_VIEW_MAX_HEIGHT, matches_filter, no_pull_requests, pull_request_row, quick_view,
    quick_view_top, signed_out_pull_requests,
};
use crate::relative_time::relative_at;
use crate::scrollbar::ScrollbarExt;
use crate::tab_bar::{TabModel, tab_bar};
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::ListRowA11y;
use crate::widgets::button;

/// `.branches-container { width: 365px }`
#[allow(non_snake_case)]
pub fn BRANCH_FOLDOUT_WIDTH() -> Pixels {
    zpx(365.)
}

pub struct BranchFoldout {
    state: Entity<AppState>,
    filter: Entity<InputState>,
    /// The Pull Requests tab has its own filter text (`PullRequestList.filterText`).
    pr_filter: Entity<InputState>,
    /// `pullRequestBeingViewed`
    quick_view: Option<QuickView>,
    /// `pullRequestQuickViewTimerId` (dropping the task cancels it).
    quick_view_timer: Option<Task<()>>,
    /// Window-space bounds of the branches container and of each pull
    /// request row (by number), recorded while painting.
    container_bounds: Rc<Cell<Bounds<Pixels>>>,
    row_bounds: Rc<RefCell<HashMap<u64, Bounds<Pixels>>>>,
    /// The quick view card's height from the last paint (`quickViewHeight`).
    quick_view_height: Rc<Cell<Pixels>>,
    /// The pointer is over the quick view (GPUI may report entering it
    /// before leaving the row, so the row's leave timer checks this).
    quick_view_hovered: bool,
    /// `FilterList` selection: the row last pressed or right-clicked (the
    /// current branch until then), and whether the list has keyboard focus.
    selected_row: Option<String>,
    /// The row drawn as selected this frame: the pressed row while it is
    /// visible, else the current branch, else - with a filter typed - the
    /// first match (`FilterList` moves the selection into the results).
    shown_selected: Option<String>,
    list_focus: FocusHandle,
    list_focused: bool,
    /// `851-branch-list-remote-only`: the list shows remote branches only.
    remote_only: bool,
    /// `895-bulk-delete-branches`: the local branches ⌘ / ⇧-clicked, in
    /// click order, and the row a ⇧-click extends from.
    multi_selected: Vec<String>,
    multi_anchor: Option<String>,
    /// `898-branch-list-folders`: the folders opened, per repository.
    expanded_folders: HashMap<u64, HashSet<String>>,
    /// GHD `FilterList` keyboard selection: the branch row ↓ / ↑ moved to
    /// from the filter box (an index into the rows as shown, groups
    /// flattened); while set it is the list's selection.
    highlighted: Option<usize>,
    scroll: UniformListScrollHandle,
    /// Shift+F10 / Menu: the selected (or highlighted) branch row.
    menu_anchor: crate::context_menu::RowMenuAnchor,
    /// The same for the Pull Requests tab.
    pr_highlighted: Option<usize>,
    /// `335-pull-request-list-filters`, kept while Corvene runs.
    pr_list_filter: crate::pull_request_list::PullRequestListFilter,
    pr_scroll: ScrollHandle,
}

/// The pull request whose quick view is shown, its parsed body, and the
/// hovered row's top.
struct QuickView {
    pr: corvene_core::PullRequest,
    body: Vec<corvene_core::markdown::Block>,
    row_top: Pixels,
}

/// `onMouseEnterPullRequestListItem` delay and the leave grace period.
const QUICK_VIEW_SHOW_DELAY: Duration = Duration::from_millis(250);
const QUICK_VIEW_HIDE_DELAY: Duration = Duration::from_millis(500);

pub struct BranchGroup {
    pub title: &'static str,
    pub branches: Vec<Branch>,
}

/// What GHD `BranchListItem` (`ui/branches/branch-list-item.tsx`) shows:
/// the icon (check for the current branch), the name and the author date
/// (`RelativeTime` with `onlyRelative`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchListItemContent {
    pub icon: Octicon,
    pub name: String,
    pub author_date: Option<String>,
}

/// GHD `BranchListItem`'s content for a branch row while the clock reads
/// `now`.
pub fn branch_list_item(
    name: &str,
    is_current_branch: bool,
    author_date: Option<std::time::SystemTime>,
    now: std::time::SystemTime,
) -> BranchListItemContent {
    BranchListItemContent {
        icon: if is_current_branch {
            Octicon::Check
        } else {
            Octicon::GitBranch
        },
        name: name.to_string(),
        author_date: author_date.map(|date| relative_at(date, now)),
    }
}

/// What a `NoBranches` button does (`onCreateNewBranch`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoBranchesAction {
    CreateNewBranch,
}

/// What GHD `NoBranches` (`ui/branches/no-branches.tsx`) shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoBranchesContent {
    /// `canCreateNewBranch`: whether the blank-slate image is drawn, the
    /// title, the subtitle, the create button (its label and action) and
    /// the ProTip's text (its shortcut included).
    CreateBranch {
        blankslate_image: bool,
        title: String,
        subtitle: String,
        button: (String, NoBranchesAction),
        protip: String,
    },
    /// Otherwise: `noBranchesMessage`, or the title.
    Message(String),
}

impl NoBranchesContent {
    /// The title, or the message of the variant without a button.
    pub fn title(&self) -> &str {
        match self {
            Self::CreateBranch { title, .. } => title,
            Self::Message(message) => message,
        }
    }
}

const NO_BRANCHES_TITLE: &str = "Sorry, I can't find that branch";
/// The ProTip around its `KeyboardShortcut` (`darwinKeys`).
const NO_BRANCHES_PROTIP: (&str, [&str; 3], &str) = (
    "ProTip! Press ",
    ["⌘", "⇧", "N"],
    " to quickly create a new branch from anywhere within the app",
);

/// GHD `NoBranches` with `canCreateNewBranch` and `noBranchesMessage`.
pub fn no_branches(
    can_create_new_branch: bool,
    no_branches_message: Option<&str>,
) -> NoBranchesContent {
    if !can_create_new_branch {
        return NoBranchesContent::Message(
            no_branches_message.unwrap_or(NO_BRANCHES_TITLE).to_string(),
        );
    }
    let (lead, keys, tail) = NO_BRANCHES_PROTIP;
    NoBranchesContent::CreateBranch {
        blankslate_image: true,
        title: NO_BRANCHES_TITLE.into(),
        subtitle: "Do you want to create a new branch instead?".into(),
        button: (
            mac_or("Create New Branch", "Create new branch").into(),
            NoBranchesAction::CreateNewBranch,
        ),
        protip: format!(
            "{lead}{}{tail}",
            crate::widgets::keyboard_shortcut_text(&keys)
        ),
    }
}

/// Flag `1223-checkout-from-fork`: the owner (or `owner/name`) and branch
/// of an `owner:branch` filter (GitHub's copy-branch-name format).
fn fork_branch_query(query: &str, cx: &App) -> Option<(String, String)> {
    let parsed = corvene_core::fork_checkout::parse_fork_input(query)?;
    let branch = parsed.branch.filter(|_| parsed.url.is_none())?;
    let repo = match parsed.name {
        Some(name) => format!("{}/{name}", parsed.owner),
        None => parsed.owner,
    };
    AppState::global(cx)
        .read(cx)
        .flags
        .bool(corvene_core::flags::ids::CHECKOUT_FROM_FORK)
        .then_some((repo, branch))
}

/// Flag `849-branch-filter-strips-owner`: `owner:branch` (GitHub's
/// copy-branch-name format) filters by `branch`. `:` can't appear in a ref
/// name, so nothing that could match is lost.
fn strip_owner_prefix(query: &str, cx: &App) -> String {
    match query.split_once(':') {
        Some((owner, branch))
            if !owner.is_empty()
                && !branch.trim().is_empty()
                && AppState::global(cx)
                    .read(cx)
                    .flags
                    .bool(corvene_core::flags::ids::BRANCH_FILTER_STRIPS_OWNER) =>
        {
            branch.trim().to_string()
        }
        _ => query.to_string(),
    }
}

/// Flag `851-branch-list-remote-only`: every remote branch matching `query`
/// (those with a local counterpart too), in one "Remote Branches" group.
fn remote_group(branches: &[Branch], query: &str, cx: &App) -> Vec<BranchGroup> {
    let mut remote: Vec<Branch> = branches
        .iter()
        .filter(|b| b.kind == BranchKind::Remote)
        .filter(|b| query.is_empty() || fuzzy_score(query, &b.name).is_some())
        .cloned()
        .collect();
    remote.sort_by_key(|b| b.name.to_lowercase());
    if sort_by_date(cx) {
        remote.sort_by_key(|b| std::cmp::Reverse(b.tip_time.unwrap_or(0)));
    }
    if remote.is_empty() {
        Vec::new()
    } else {
        vec![BranchGroup {
            title: "Remote Branches",
            branches: remote,
        }]
    }
}

/// GHD `BranchesContainer.onBranchItemClick` (a click, or Enter in the
/// filter box): close the foldout and check the branch out, or ask first
/// (`864-confirm-branch-switch`).
fn checkout_branch_row(id: u64, name: String, current: bool, cx: &mut App) {
    Dispatcher::close_foldout(cx);
    if AppState::global(cx)
        .read(cx)
        .flags
        .bool(corvene_core::flags::ids::CONFIRM_BRANCH_SWITCH)
        && !current
    {
        Dispatcher::show_popup(
            Popup::ConfirmSwitchBranch {
                repo: id,
                branch: name,
            },
            cx,
        );
        return;
    }
    Dispatcher::checkout_branch(id, name, None, cx)
}

/// Flag `899-tags-in-branch-list` (+ `346-releases`): a tag row's menu:
/// Create Release… for the tag, Copy Tag Name, and Delete Tag… as the
/// History commit menu offers it (unpushed tags at once, the others after
/// the `826` confirmation).
fn tag_row_menu(
    id: u64,
    tag: String,
    sha: String,
    position: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    use crate::context_menu::MenuItem;
    let (releases, unpushed, delete_pushed) = {
        let s = AppState::global(cx).read(cx);
        (
            s.flags.bool(corvene_core::flags::ids::RELEASES)
                && s.repository(id).and_then(|r| r.non_fork_github()).is_some(),
            s.repository(id)
                .is_some_and(|r| r.tags_to_push.contains(&tag)),
            s.flags.bool(corvene_core::flags::ids::DELETE_PUSHED_TAGS),
        )
    };
    let mut items = Vec::new();
    if releases {
        let (tag, sha) = (tag.clone(), sha.clone());
        items.push(MenuItem::new(
            mac_or("Create Release…", "Create release…"),
            move |_, cx| {
                Dispatcher::show_create_release(id, Some(tag.clone()), Some(sha.clone()), cx)
            },
        ));
        items.push(MenuItem::separator());
    }
    // Corvene (`1218-compare-refs`): this tag against the current branch
    if corvene_core::ref_compare::enabled(AppState::global(cx).read(cx)) {
        let (base, head) = (tag.clone(), crate::history::current_ref(id, cx));
        items.push(MenuItem::new("Compare with…", move |_, cx| {
            Dispatcher::show_compare_refs(id, Some(base.clone()), head.clone(), cx)
        }));
        items.push(MenuItem::separator());
    }
    let copied = tag.clone();
    items.push(MenuItem::new(
        mac_or("Copy Tag Name", "Copy tag name"),
        move |_, cx| cx.write_to_clipboard(ClipboardItem::new_string(copied.clone())),
    ));
    items.push(MenuItem::separator());
    let deleted = tag.clone();
    items.push(
        MenuItem::new(mac_or("Delete Tag…", "Delete tag…"), move |_, cx| {
            Dispatcher::request_delete_tag(id, deleted.clone(), cx)
        })
        .enabled(unpushed || delete_pushed),
    );
    crate::native_menu::show_context_menu(items, position, window, cx);
}

/// Flag `899-tags-in-branch-list`: a Tags group row is a tag dressed as a
/// branch (`refs/tags/<name>`, its commit as the tip); its commit.
fn tag_commit(branch: &Branch) -> Option<&str> {
    branch
        .full_name
        .starts_with("refs/tags/")
        .then_some(branch.tip.as_deref())
        .flatten()
}

/// A click (or Enter) on a row: a tag's commit is checked out (after the
/// detached HEAD confirmation), a branch as [`checkout_branch_row`] does.
fn pick_row(id: u64, branch: &Branch, current: bool, cx: &mut App) {
    match tag_commit(branch) {
        Some(sha) => {
            Dispatcher::close_foldout(cx);
            Dispatcher::request_checkout_commit(id, sha.to_string(), cx);
        }
        None => checkout_branch_row(id, branch.name.clone(), current, cx),
    }
}

/// Flag `899-tags-in-branch-list`: the tags matching `query`, as branch
/// rows ([`tag_commit`]) in a Tags group.
fn tags_group(tags: &[(String, String)], query: &str) -> Option<BranchGroup> {
    let branches: Vec<Branch> = tags
        .iter()
        .filter(|(name, _)| fuzzy_score(query, name).is_some())
        .map(|(name, sha)| Branch {
            name: name.clone(),
            kind: BranchKind::Remote,
            full_name: format!("refs/tags/{name}"),
            tip: Some(sha.clone()),
            upstream: None,
            tip_time: None,
            tip_author: None,
            remote_name: None,
        })
        .collect();
    (!branches.is_empty()).then_some(BranchGroup {
        title: "Tags",
        branches,
    })
}

/// Flag `848-branch-list-sort-by-date`: Other Branches newest first.
pub fn sort_by_date(cx: &App) -> bool {
    AppState::try_global(cx).is_some_and(|s| {
        s.read(cx)
            .flags
            .bool(corvene_core::flags::ids::BRANCH_LIST_SORT_BY_DATE)
    })
}

/// GHD `groupBranches` over the merged local + remote-only branch list.
/// `newest_first` orders Other Branches by the tip's committer date (newest
/// first, then by name) instead of by name alone.
pub fn group_branches(
    branches: &[Branch],
    default_branch: Option<&str>,
    recent: &[String],
    query: &str,
    newest_first: bool,
) -> Vec<BranchGroup> {
    let query = query.trim();
    let matches = |b: &Branch| query.is_empty() || fuzzy_score(query, &b.name).is_some();
    let mut all = merge_remote_and_local_branches(branches);
    all.sort_by_key(|b| b.name.to_lowercase());

    let mut groups = Vec::new();
    if let Some(default) = default_branch.and_then(|d| all.iter().find(|b| b.name == d))
        && matches(default)
    {
        groups.push(BranchGroup {
            title: mac_or("Default Branch", "Default branch"),
            branches: vec![default.clone()],
        });
    }
    let recent_branches: Vec<Branch> = recent
        .iter()
        .filter(|n| Some(n.as_str()) != default_branch)
        .filter_map(|n| {
            all.iter()
                .find(|b| b.name == *n && b.kind == BranchKind::Local)
        })
        .filter(|b| matches(b))
        .cloned()
        .collect();
    if !recent_branches.is_empty() {
        groups.push(BranchGroup {
            title: mac_or("Recent Branches", "Recent branches"),
            branches: recent_branches,
        });
    }
    let mut other: Vec<Branch> = all
        .iter()
        .filter(|b| Some(b.name.as_str()) != default_branch)
        .filter(|b| !recent.contains(&b.name) || b.kind == BranchKind::Remote)
        .filter(|b| matches(b))
        .cloned()
        .collect();
    if newest_first {
        // stable: equal dates keep the name order
        other.sort_by_key(|b| std::cmp::Reverse(b.tip_time.unwrap_or(0)));
    }
    if !other.is_empty() {
        groups.push(BranchGroup {
            title: mac_or("Other Branches", "Other branches"),
            branches: other,
        });
    }
    groups
}

/// A folder row of [`fold_branches`]: drawn in group `group` before its
/// visible branch `before` (or after the last), holding `count` branches,
/// listed right after it while `expanded`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchFolder {
    pub group: usize,
    pub before: usize,
    pub name: String,
    pub count: usize,
    pub expanded: bool,
}

/// Flag `898-branch-list-folders`: in the Other Branches group, branches
/// sharing a prefix before the first `/` (two or more of them) move under a
/// folder placed where the first of them was; a folder not in `expanded`
/// hides them. Returns the groups as shown and the folder rows.
pub fn fold_branches(
    mut groups: Vec<BranchGroup>,
    expanded: &HashSet<String>,
) -> (Vec<BranchGroup>, Vec<BranchFolder>) {
    let other = mac_or("Other Branches", "Other branches");
    let mut folders = Vec::new();
    for (g, group) in groups.iter_mut().enumerate() {
        if group.title != other {
            continue;
        }
        let prefix = |b: &Branch| {
            b.name
                .split_once('/')
                .map(|(p, _)| p.to_string())
                .filter(|p| !p.is_empty())
        };
        let mut counts: HashMap<String, usize> = HashMap::new();
        for b in &group.branches {
            if let Some(p) = prefix(b) {
                *counts.entry(p).or_default() += 1;
            }
        }
        let branches = std::mem::take(&mut group.branches);
        let mut emitted: HashSet<String> = HashSet::new();
        for b in &branches {
            match prefix(b).filter(|p| counts.get(p).is_some_and(|n| *n > 1)) {
                Some(p) => {
                    if !emitted.insert(p.clone()) {
                        continue;
                    }
                    let open = expanded.contains(&p);
                    let members: Vec<Branch> = branches
                        .iter()
                        .filter(|m| prefix(m).as_deref() == Some(p.as_str()))
                        .cloned()
                        .collect();
                    folders.push(BranchFolder {
                        group: g,
                        before: group.branches.len(),
                        count: members.len(),
                        name: p,
                        expanded: open,
                    });
                    if open {
                        group.branches.extend(members);
                    }
                }
                None => group.branches.push(b.clone()),
            }
        }
    }
    (groups, folders)
}

/// One row of the branch list: a group header, a folder (an index into the
/// folders) or a branch (its group and index there, its keyboard row and
/// whether it is inside an open folder).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListItem {
    Header(usize),
    Folder(usize),
    Branch {
        group: usize,
        ix: usize,
        row: usize,
        nested: bool,
    },
}

fn list_items(groups: &[BranchGroup], folders: &[BranchFolder]) -> Vec<ListItem> {
    let mut items = Vec::new();
    let mut row = 0;
    for (g, group) in groups.iter().enumerate() {
        items.push(ListItem::Header(g));
        let here: Vec<(usize, &BranchFolder)> = folders
            .iter()
            .enumerate()
            .filter(|(_, f)| f.group == g)
            .collect();
        for ix in 0..=group.branches.len() {
            for (f, _) in here.iter().filter(|(_, f)| f.before == ix) {
                items.push(ListItem::Folder(*f));
            }
            if ix == group.branches.len() {
                break;
            }
            let nested = here
                .iter()
                .any(|(_, f)| f.expanded && f.before <= ix && ix < f.before + f.count);
            items.push(ListItem::Branch {
                group: g,
                ix,
                row,
                nested,
            });
            row += 1;
        }
    }
    items
}

/// Flag `897-pinned-branches`: moves the branches named in `pinned` (in
/// that order) out of Recent and Other into a Pinned group after the
/// Default Branch group. The default branch stays in its own group.
pub fn group_pinned(groups: Vec<BranchGroup>, pinned: &[String]) -> Vec<BranchGroup> {
    let default_title = mac_or("Default Branch", "Default branch");
    let mut found: Vec<Branch> = Vec::new();
    let mut out: Vec<BranchGroup> = groups
        .into_iter()
        .map(|mut g| {
            if g.title != default_title {
                let (pin, rest): (Vec<Branch>, Vec<Branch>) = g
                    .branches
                    .into_iter()
                    .partition(|b| pinned.contains(&b.name));
                found.extend(pin);
                g.branches = rest;
            }
            g
        })
        .filter(|g| !g.branches.is_empty())
        .collect();
    let mut ordered: Vec<Branch> = Vec::new();
    for name in pinned {
        if let Some(ix) = found.iter().position(|b| &b.name == name) {
            ordered.push(found.remove(ix));
        }
    }
    if !ordered.is_empty() {
        let at = usize::from(out.first().is_some_and(|g| g.title == default_title));
        out.insert(
            at,
            BranchGroup {
                title: "Pinned",
                branches: ordered,
            },
        );
    }
    out
}

/// Flag `896-branch-upstream-gone-group`: moves the local branches whose
/// upstream is gone (configured, but its remote-tracking branch deleted)
/// out of their groups into a "Deleted on Remote" group at the end; the
/// branches named in `keep` (the current and the default branch) stay.
pub fn group_upstream_gone(
    groups: Vec<BranchGroup>,
    all: &[Branch],
    keep: &[&str],
) -> Vec<BranchGroup> {
    let gone = |b: &Branch| {
        b.kind == BranchKind::Local
            && !keep.contains(&b.name.as_str())
            && corvene_core::delete_branches::upstream_gone(b, all)
    };
    let mut moved: Vec<Branch> = Vec::new();
    let mut out: Vec<BranchGroup> = groups
        .into_iter()
        .map(|mut g| {
            let (gone, rest): (Vec<Branch>, Vec<Branch>) =
                g.branches.into_iter().partition(|b| gone(b));
            moved.extend(gone);
            g.branches = rest;
            g
        })
        .filter(|g| !g.branches.is_empty())
        .collect();
    if !moved.is_empty() {
        moved.sort_by_key(|b| b.name.to_lowercase());
        moved.dedup_by(|a, b| a.name == b.name);
        out.push(BranchGroup {
            title: mac_or("Deleted on Remote", "Deleted on remote"),
            branches: moved,
        });
    }
    out
}

/// GHD `mergeRemoteAndLocalBranches` (`lib/stores/git-store.ts`, its
/// `allBranches`): the local branches, then the remote branches no local
/// branch tracks.
pub fn merge_remote_and_local_branches(branches: &[Branch]) -> Vec<Branch> {
    let mut all: Vec<Branch> = branches
        .iter()
        .filter(|b| b.kind == BranchKind::Local)
        .cloned()
        .collect();
    let tracked = tracked_upstreams(branches);
    all.extend(
        branches
            .iter()
            .filter(|b| b.kind == BranchKind::Remote && !tracked.contains(b.name.as_str()))
            .cloned(),
    );
    all
}

/// The remote branches a local branch in `branches` tracks (GHD's
/// `upstreamBranchesAdded`: the local branches' `upstream`, compared with
/// the remote branches' names).
fn tracked_upstreams(branches: &[Branch]) -> std::collections::HashSet<&str> {
    branches
        .iter()
        .filter(|b| b.kind == BranchKind::Local)
        .filter_map(|b| b.upstream_short())
        .collect()
}

/// Remote-tracking branches that [`group_branches`] hides behind the local
/// branch tracking them (`origin/main` behind `main`), for the rebase list
/// (flag `832`): rebasing onto the fetched remote needs no pull of the
/// local one.
pub fn remote_counterparts(branches: &[Branch], query: &str) -> Option<BranchGroup> {
    let query = query.trim();
    let tracked = tracked_upstreams(branches);
    let mut remotes: Vec<Branch> = branches
        .iter()
        .filter(|b| b.kind == BranchKind::Remote && !b.name.ends_with("/HEAD"))
        .filter(|b| tracked.contains(b.name.as_str()))
        .filter(|b| query.is_empty() || fuzzy_score(query, &b.name).is_some())
        .cloned()
        .collect();
    remotes.sort_by_key(|b| b.name.to_lowercase());
    (!remotes.is_empty()).then_some(BranchGroup {
        title: "Remote Branches",
        branches: remotes,
    })
}

impl BranchFoldout {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
        let pr_filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        cx.observe(&filter, |this: &mut Self, _, cx| {
            this.highlighted = None;
            cx.notify()
        })
        .detach();
        cx.observe(&pr_filter, |this: &mut Self, _, cx| {
            this.pr_highlighted = None;
            cx.notify()
        })
        .detach();
        let scroll = UniformListScrollHandle::new();
        Self {
            state,
            filter,
            pr_filter,
            quick_view: None,
            quick_view_timer: None,
            container_bounds: Rc::new(Cell::new(Bounds::default())),
            row_bounds: Rc::new(RefCell::new(HashMap::new())),
            quick_view_height: Rc::new(Cell::new(QUICK_VIEW_MAX_HEIGHT())),
            quick_view_hovered: false,
            selected_row: None,
            shown_selected: None,
            list_focus: cx.focus_handle().tab_stop(true),
            list_focused: false,
            remote_only: false,
            multi_selected: Vec::new(),
            multi_anchor: None,
            expanded_folders: HashMap::new(),
            highlighted: None,
            menu_anchor: crate::context_menu::RowMenuAnchor::for_uniform_list(&scroll),
            scroll,
            pr_highlighted: None,
            pr_list_filter: Default::default(),
            pr_scroll: ScrollHandle::new(),
        }
    }

    /// The Branches tab's groups as the list shows them.
    fn branch_groups(&self, cx: &App) -> Vec<BranchGroup> {
        self.folded_groups(cx).0
    }

    /// [`Self::branch_groups`] and the folder rows (`898-branch-list-folders`,
    /// not while filtering).
    fn folded_groups(&self, cx: &App) -> (Vec<BranchGroup>, Vec<BranchFolder>) {
        let groups = self.unfolded_groups(cx);
        let s = self.state.read(cx);
        let folders = s.flags.bool(corvene_core::flags::ids::BRANCH_LIST_FOLDERS)
            && !(self.remote_only
                && s.flags
                    .bool(corvene_core::flags::ids::BRANCH_LIST_REMOTE_ONLY))
            && self.filter_text(cx).is_empty();
        match s.selected.filter(|_| folders) {
            Some(id) => {
                let none = HashSet::new();
                fold_branches(groups, self.expanded_folders.get(&id).unwrap_or(&none))
            }
            None => (groups, Vec::new()),
        }
    }

    /// `898-branch-list-folders`: open or close a folder row.
    fn toggle_folder(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(id) = self.state.read(cx).selected else {
            return;
        };
        let open = self.expanded_folders.entry(id).or_default();
        if !open.remove(&name) {
            open.insert(name);
        }
        self.highlighted = None;
        cx.notify();
    }

    fn unfolded_groups(&self, cx: &App) -> Vec<BranchGroup> {
        let query = self.filter_text(cx);
        let s = self.state.read(cx);
        let remote_only = self.remote_only
            && s.flags
                .bool(corvene_core::flags::ids::BRANCH_LIST_REMOTE_ONLY);
        let Some(rs) = s.selected.and_then(|id| s.repo_states.get(&id)) else {
            return Vec::new();
        };
        let Some(info) = rs.info.as_ref() else {
            return Vec::new();
        };
        if remote_only {
            remote_group(&info.branches, &query, cx)
        } else {
            let mut groups = group_branches(
                &info.branches,
                rs.default_branch.as_deref(),
                &rs.recent_branches,
                &query,
                sort_by_date(cx),
            );
            // `897-pinned-branches`, not while filtering
            let pinned = s
                .selected
                .and_then(|id| s.repository(id))
                .map(|r| r.pinned_branches.as_slice())
                .filter(|p| {
                    !p.is_empty()
                        && query.is_empty()
                        && s.flags.bool(corvene_core::flags::ids::PINNED_BRANCHES)
                })
                .unwrap_or_default();
            if !pinned.is_empty() {
                groups = group_pinned(groups, pinned);
            }
            // `899-tags-in-branch-list`: after the branches
            if !query.is_empty()
                && s.flags.bool(corvene_core::flags::ids::TAGS_IN_BRANCH_LIST)
                && let Some(tags) = rs.branch_list_tags.as_deref()
            {
                groups.extend(tags_group(tags, &query));
            }
            if s.flags
                .bool(corvene_core::flags::ids::BRANCH_UPSTREAM_GONE_GROUP)
            {
                let current = info.current_branch().map(|b| b.name.as_str());
                let keep: Vec<&str> = current
                    .into_iter()
                    .chain(rs.default_branch.as_deref())
                    .chain(pinned.iter().map(String::as_str))
                    .collect();
                group_upstream_gone(groups, &info.branches, &keep)
            } else {
                groups
            }
        }
    }

    /// The Pull Requests tab's rows as the list shows them.
    fn pull_request_items(&self, id: u64, cx: &App) -> Vec<corvene_core::PullRequest> {
        let query = self.pr_filter.read(cx).value().trim().to_string();
        let s = self.state.read(cx);
        let (filter, login) = self.pr_list_filter(id, cx);
        s.pull_requests_for(id)
            .iter()
            .filter(|pr| matches_filter(pr, &query))
            .filter(|pr| filter.matches(pr, login.as_deref()))
            .cloned()
            .collect()
    }

    /// `335-pull-request-list-filters`: the list's filter (All while the
    /// flag is off) and the signed-in login it compares with.
    fn pr_list_filter(
        &self,
        id: u64,
        cx: &App,
    ) -> (
        crate::pull_request_list::PullRequestListFilter,
        Option<String>,
    ) {
        let s = self.state.read(cx);
        if !s
            .flags
            .bool(corvene_core::flags::ids::PULL_REQUEST_LIST_FILTERS)
        {
            return (Default::default(), None);
        }
        let login = s
            .repository(id)
            .and_then(|r| r.non_fork_github())
            .and_then(|gh| s.account_for_repository_on(id, &gh.endpoint))
            .map(|a| a.login.clone())
            // flags 342-344: the host account's login
            .or_else(|| {
                s.pull_request_repository(id)
                    .and_then(|gh| s.host_account_for_repository(id, &gh.endpoint))
                    .map(|a| a.login.clone())
            });
        (self.pr_list_filter, login)
    }

    /// GHD `ui/lib/filter-list.tsx` `onFilterKeyDown`: ↓ / ↑ in the
    /// Branches filter box move through the branch rows (↑ from the filter
    /// starts at the last), clamped, skipping the group headers.
    fn move_highlight(&mut self, delta: isize, cx: &mut Context<Self>) {
        let (groups, folders) = self.folded_groups(cx);
        let total = groups.iter().map(|g| g.branches.len()).sum();
        let Some(ix) = crate::filter_list::step(self.highlighted, delta, total) else {
            return;
        };
        self.highlighted = Some(ix);
        // the list is uniform (headers, folders and `.branches-list-item`
        // rows are all 30 px): its item index counts the rows above
        let item = list_items(&groups, &folders)
            .iter()
            .position(|item| matches!(item, ListItem::Branch { row, .. } if *row == ix))
            .unwrap_or(ix);
        self.scroll.scroll_to_item(item, ScrollStrategy::Top);
        cx.notify();
    }

    /// GHD `ui/lib/filter-list.tsx` `onFilterKeyDown` (Enter) and
    /// `onEnterPressed`: Enter in the Branches filter box checks out the
    /// highlighted branch, else - with a filter typed - the first one, as a
    /// click does; with a filter that matches nothing it opens Create
    /// Branch with the text (`onEnterPressedWithoutFilteredItems`).
    fn pick_highlighted(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.state.read(cx).selected else {
            return;
        };
        let query = self.filter_text(cx);
        let branches: Vec<Branch> = self
            .branch_groups(cx)
            .into_iter()
            .flat_map(|g| g.branches)
            .collect();
        let ix = self
            .highlighted
            .or_else(|| (!query.is_empty()).then_some(0));
        if let Some(branch) = ix.and_then(|ix| branches.get(ix)) {
            let current = {
                let s = self.state.read(cx);
                s.repo_states
                    .get(&id)
                    .and_then(|rs| rs.info.as_ref())
                    .and_then(|i| i.current_branch())
                    .is_some_and(|b| b.name == branch.name)
            };
            pick_row(id, branch, current, cx);
        } else if branches.is_empty() && !query.is_empty() {
            Dispatcher::close_foldout(cx);
            Dispatcher::show_popup(
                Popup::CreateBranch {
                    repo: id,
                    target_sha: None,
                    initial_name: query,
                },
                cx,
            );
        }
    }

    /// GHD `ui/lib/filter-list.tsx` `onFilterKeyDown` in the Pull Requests
    /// filter box (`SectionFilterList`): ↓ / ↑ move through the rows.
    fn move_pr_highlight(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(id) = self.state.read(cx).selected else {
            return;
        };
        let count = self.pull_request_items(id, cx).len();
        let Some(ix) = crate::filter_list::step(self.pr_highlighted, delta, count) else {
            return;
        };
        self.pr_highlighted = Some(ix);
        // one "Pull requests in …" header, then 47 px rows
        let top = crate::filter_list::row_top(
            &[count],
            ix,
            ROW_HEIGHT(),
            crate::pull_request_list::PR_ROW_HEIGHT(),
        );
        crate::filter_list::scroll_into_view(
            &self.pr_scroll,
            top,
            crate::pull_request_list::PR_ROW_HEIGHT(),
        );
        cx.notify();
    }

    /// GHD `ui/lib/filter-list.tsx` `onFilterKeyDown` (Enter): the
    /// highlighted pull request, else - with a filter typed - the first,
    /// checked out as a click does (`PullRequestList.onItemClick`).
    fn pick_pr_highlighted(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.state.read(cx).selected else {
            return;
        };
        let query_empty = self.pr_filter.read(cx).value().trim().is_empty();
        let ix = self.pr_highlighted.or_else(|| (!query_empty).then_some(0));
        if let Some(pr) = ix.and_then(|ix| self.pull_request_items(id, cx).into_iter().nth(ix)) {
            Dispatcher::close_foldout(cx);
            Dispatcher::checkout_pull_request(id, pr, cx);
        }
    }

    /// `onMouseEnterPullRequestListItem` / `onMouseLeavePullRequestListItem`
    fn hover_pull_request(
        &mut self,
        pr: corvene_core::PullRequest,
        hovered: bool,
        cx: &mut Context<Self>,
    ) {
        self.quick_view_timer = None;
        if hovered {
            self.quick_view_hovered = false;
            if self.quick_view.take().is_some() {
                cx.notify();
            }
            let number = pr.number;
            self.quick_view_timer = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(QUICK_VIEW_SHOW_DELAY).await;
                this.update(cx, |this, cx| {
                    let row_top = this
                        .row_bounds
                        .borrow()
                        .get(&number)
                        .map(|b| b.origin.y)
                        .unwrap_or_default();
                    let body = if pr.body.trim().is_empty() {
                        "_No description provided._"
                    } else {
                        pr.body.as_str()
                    };
                    this.quick_view = Some(QuickView {
                        body: corvene_core::markdown::parse(body),
                        pr,
                        row_top,
                    });
                    this.quick_view_timer = None;
                    cx.notify();
                })
                .ok();
            }));
        } else if !self.quick_view_hovered {
            self.quick_view_timer = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(QUICK_VIEW_HIDE_DELAY).await;
                this.update(cx, |this, cx| {
                    if this.quick_view_hovered {
                        return;
                    }
                    this.quick_view = None;
                    this.quick_view_timer = None;
                    cx.notify();
                })
                .ok();
            }));
        }
    }

    /// `onMouseEnterPullRequestQuickView` / `onMouseLeavePullRequestQuickView`
    fn hover_quick_view(&mut self, hovered: bool, cx: &mut Context<Self>) {
        self.quick_view_timer = None;
        self.quick_view_hovered = hovered;
        if !hovered {
            self.quick_view = None;
            cx.notify();
        }
    }

    /// The quick view, placed right of the foldout next to the hovered row.
    fn render_quick_view(&self, window: &Window, cx: &Context<Self>) -> Option<AnyElement> {
        let view = self.quick_view.as_ref()?;
        let container = self.container_bounds.get();
        let height = self.quick_view_height.get();
        let top = quick_view_top(
            view.row_top,
            container.origin.y,
            crate::theme::page_size(window).height,
            height,
        );
        let pointer_top = view.row_top - container.origin.y - top
            + crate::pull_request_list::PR_ROW_HEIGHT() / 2.
            + zpx(1.);
        let status = self.state.read(cx).commit_status_summary(&view.pr);
        let entity = cx.entity().downgrade();
        Some(
            div()
                .absolute()
                .left(container.size.width)
                .top(top)
                .child(
                    quick_view(
                        &view.pr,
                        &view.body,
                        status,
                        pointer_top,
                        self.quick_view_height.clone(),
                        cx,
                    )
                    .on_hover(move |hovered, _, cx| {
                        entity
                            .update(cx, |this, cx| this.hover_quick_view(*hovered, cx))
                            .ok();
                    }),
                )
                .into_any_element(),
        )
    }

    /// The Branches tab's filter text (`847-new-branch-from-filter`), with
    /// an `owner:` prefix stripped as the list does.
    pub fn filter_text(&self, cx: &App) -> String {
        strip_owner_prefix(self.filter.read(cx).value().trim(), cx)
    }

    pub fn focus_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // a freshly opened list selects the current branch again
        self.selected_row = None;
        self.multi_selected.clear();
        self.multi_anchor = None;
        self.highlighted = None;
        self.pr_highlighted = None;
        // `899-tags-in-branch-list`: the tags as they are now
        let tags = {
            let s = self.state.read(cx);
            s.selected
                .filter(|_| s.flags.bool(corvene_core::flags::ids::TAGS_IN_BRANCH_LIST))
        };
        if let Some(id) = tags {
            Dispatcher::load_branch_list_tags(id, cx);
        }
        let input = if self.pull_requests_tab_shown(cx) {
            &self.pr_filter
        } else {
            &self.filter
        };
        let handle = input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    /// The tab bar only exists for GitHub repositories (and, flags
    /// 342-344, GitLab, Gitea and Bitbucket ones).
    fn is_github(&self, cx: &App) -> bool {
        let s = self.state.read(cx);
        s.selected
            .and_then(|id| s.repository(id))
            .is_some_and(|r| r.github.is_some())
            || s.selected.and_then(|id| s.hosted_repository(id)).is_some()
    }

    /// GitLab calls them merge requests (flag `342-gitlab`).
    fn merge_requests(&self, cx: &App) -> bool {
        let s = self.state.read(cx);
        s.selected
            .and_then(|id| s.hosted_repository(id))
            .is_some_and(|h| h.kind.is_merge_request())
    }

    fn pull_requests_tab_shown(&self, cx: &App) -> bool {
        self.is_github(cx) && self.state.read(cx).branches_tab == BranchesTab::PullRequests
    }

    /// `PullRequestList`: filter row, group header, rows or the blank slate.
    fn pull_requests_tab(&self, id: u64, window: &Window, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let query = self.pr_filter.read(cx).value().trim().to_string();
        let s = self.state.read(cx);
        let loading = s.pull_requests_loading(id);
        let repository_name = s
            .pull_request_repository(id)
            .map(|gh| gh.full_name())
            .unwrap_or_default();
        // `pull-requests-signed-out`: no account for the endpoint, so the
        // list cannot load (GHD shows "You're all set!" and a refresh
        // button that does nothing)
        let signed_out_endpoint = s
            .flags
            .bool(corvene_core::flags::ids::PULL_REQUESTS_SIGNED_OUT)
            .then(|| s.repository(id).and_then(|r| r.non_fork_github()))
            .flatten()
            .filter(|gh| s.account_for(&gh.endpoint).is_none())
            .map(|gh| gh.endpoint.clone());
        let signed_out = signed_out_endpoint.is_some();
        let current = s.current_pull_request(id).map(|pr| pr.number);
        let rs = s.repo_states.get(&id);
        let on_default_branch = rs.is_some_and(|rs| {
            rs.default_branch.is_some()
                && rs.default_branch.as_deref()
                    == rs
                        .info
                        .as_ref()
                        .and_then(|i| i.current_branch())
                        .map(|b| b.name.as_str())
        });
        let all = s.pull_requests_for(id);
        let items = self.pull_request_items(id, cx);
        // `335-pull-request-list-filters`
        let filters_on = s
            .flags
            .bool(corvene_core::flags::ids::PULL_REQUEST_LIST_FILTERS);
        let list_filter = self.pr_list_filter(id, cx).0;
        let highlighted = self.pr_highlighted.filter(|ix| *ix < items.len());
        let highlight_bg = t.box_selected_active_background;
        let highlight_text = t.box_selected_active_text;
        let local_name = s.repository(id).map(|r| r.name()).unwrap_or_default();
        let rows: Vec<AnyElement> = items
            .iter()
            .enumerate()
            .map(|(ix, pr)| {
                let status = s.commit_status_summary(pr);
                let entity = cx.entity().downgrade();
                let hovered_pr = pr.clone();
                let row_bounds = self.row_bounds.clone();
                let number = pr.number;
                pull_request_row(
                    id,
                    pr,
                    current == Some(pr.number),
                    status,
                    local_name.clone(),
                    cx,
                )
                // the keyboard row (GHD's focused-list selection)
                .when(highlighted == Some(ix), |d| {
                    d.bg(highlight_bg)
                        .text_color(highlight_text)
                        // `623-contrast-focus-ring`
                        .when_some(crate::keyboard_nav::contrast_row_outline(cx), |d, ring| {
                            d.shadow(ring)
                        })
                })
                .relative()
                .on_hover(move |hovered, _, cx| {
                    let pr = hovered_pr.clone();
                    entity
                        .update(cx, |this, cx| this.hover_pull_request(pr, *hovered, cx))
                        .ok();
                })
                .child(
                    canvas(
                        move |b, _, _| {
                            row_bounds.borrow_mut().insert(number, b);
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                )
                .into_any_element()
            })
            .collect();
        div()
            .id("pull-request-list")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                // `.filter-field-row` with `renderPostFilter` (the refresh button)
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING())
                    .p(SPACING())
                    .pb(SPACING_HALF())
                    .key_context("PullRequestFilter")
                    .on_action(
                        cx.listener(|this, _: &SelectNextFile, _, cx| {
                            this.move_pr_highlight(1, cx)
                        }),
                    )
                    .on_action(cx.listener(|this, _: &SelectPreviousFile, _, cx| {
                        this.move_pr_highlight(-1, cx)
                    }))
                    .on_action(
                        cx.listener(|this, _: &FilterListPick, _, cx| this.pick_pr_highlighted(cx)),
                    )
                    .child(crate::widgets::filter_text_box(
                        "pull-request-filter",
                        &self.pr_filter,
                        Some(octicon(Octicon::Search, t.text_secondary)),
                        window,
                        cx,
                    ))
                    .when(filters_on, |d| {
                        let entity = cx.entity().downgrade();
                        let filtered = list_filter != Default::default();
                        d.child(
                            button("pull-request-list-filter", "", cx)
                                .flex_none()
                                .px(SPACING_HALF())
                                .gap(zpx(2.))
                                .when(filtered, |d| d.bg(t.box_selected_background))
                                .icon_button_label(list_filter.label())
                                .on_click(move |ev: &ClickEvent, window, cx| {
                                    let position = ev.mouse_position().unwrap_or_default();
                                    let items =
                                        crate::pull_request_list::PullRequestListFilter::ALL
                                            .into_iter()
                                            .map(|filter| {
                                                let entity = entity.clone();
                                                crate::context_menu::MenuItem::checkbox(
                                                    filter.label(),
                                                    filter == list_filter,
                                                    move |_, cx| {
                                                        entity
                                                            .update(cx, |this, cx| {
                                                                this.pr_list_filter = filter;
                                                                this.pr_highlighted = None;
                                                                cx.notify();
                                                            })
                                                            .ok();
                                                    },
                                                )
                                            })
                                            .collect();
                                    crate::native_menu::show_context_menu(
                                        items, position, window, cx,
                                    );
                                })
                                .child(octicon(Octicon::Filter, t.secondary_button_text))
                                .child(octicon(Octicon::TriangleDown, t.secondary_button_text)),
                        )
                    })
                    .child(
                        button("pull-request-refresh", "", cx)
                            .flex_none()
                            .px(SPACING_HALF())
                            .when(loading, |d| d.opacity(0.6))
                            .when(signed_out, |d| d.opacity(0.6).cursor_default())
                            .icon_button_label("Refresh the list of pull requests")
                            .on_click(move |_, _, cx| {
                                if !loading && !signed_out {
                                    Dispatcher::refresh_pull_requests(id, true, cx)
                                }
                            })
                            .child({
                                let icon = octicon(Octicon::SyncClockwise, t.secondary_button_text);
                                if loading {
                                    spin(icon, "pull-request-refresh-spin")
                                } else {
                                    icon.into_any_element()
                                }
                            }),
                    ),
            )
            .child(
                if let Some(endpoint) = signed_out_endpoint.filter(|_| rows.is_empty()) {
                    signed_out_pull_requests(repository_name, endpoint, cx)
                } else if rows.is_empty() {
                    no_pull_requests(
                        id,
                        repository_name,
                        !query.is_empty() || list_filter != Default::default(),
                        loading && all.is_empty(),
                        on_default_branch,
                        cx,
                    )
                } else {
                    div()
                        .id("pull-request-rows")
                        .role(Role::List)
                        .aria_label("Pull requests")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .child(
                            // `.filter-list-group-header`
                            div()
                                .h(ROW_HEIGHT())
                                .pt(SPACING())
                                .px(SPACING())
                                .flex()
                                .items_center()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(FONT_SIZE())
                                .truncate()
                                .child(if self.merge_requests(cx) {
                                    format!("Merge requests in {repository_name}")
                                } else {
                                    format!("Pull requests in {repository_name}")
                                }),
                        )
                        .children(rows)
                        .with_scrollbar_handle(&self.pr_scroll)
                        .into_any_element()
                },
            )
            .into_any_element()
    }

    /// `highlighted`: the filter box's keyboard row (GHD `FilterList`);
    /// `stashed`: the branch has a Desktop stash (`854-branch-list-stash-icon`);
    /// `tracking`: its upstream state (`852-branch-upstream-gone`).
    #[allow(clippy::too_many_arguments)]
    fn row(
        &self,
        id: u64,
        branch: &Branch,
        nested: bool,
        current: bool,
        highlighted: bool,
        stashed: bool,
        tracking: Option<corvene_git::BranchTracking>,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        // `853-branch-list-ahead-behind`: unpublished, or commits to push / pull
        let ahead_behind = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::BRANCH_LIST_AHEAD_BEHIND);
        let sync_state = (ahead_behind && branch.kind == BranchKind::Local)
            .then(|| match (&branch.upstream, tracking) {
                (None, _) => Some(("Not published".to_string(), None)),
                (Some(_), Some(t)) if !t.gone && (t.ahead > 0 || t.behind > 0) => Some((
                    format!("{} to push, {} to pull", t.ahead, t.behind),
                    Some((t.ahead, t.behind)),
                )),
                _ => None,
            })
            .flatten();
        // `1222-push-target-guard`: a differently named upstream, shown
        let push_target = {
            let s = self.state.read(cx);
            Dispatcher::push_target_guard(s)
                .then(|| {
                    corvene_core::push_target::mismatch_of(
                        branch,
                        Dispatcher::default_upstream_in(s, id).as_deref(),
                    )
                })
                .flatten()
        };
        let t = cx.ghd();
        let name = branch.name.clone();
        let is_tag = tag_commit(branch).is_some();
        // `1201-branch-list-tip-author`
        let author = branch.tip_author.clone().filter(|_| {
            self.state
                .read(cx)
                .flags
                .bool(corvene_core::flags::ids::BRANCH_LIST_TIP_AUTHOR)
        });
        // the keyboard row (GHD `FilterList` moves its selection, focusing
        // the list) replaces the pointer / current-branch selection
        let keyboard = self.highlighted.is_some();
        let selected = if keyboard {
            highlighted
        } else if !self.multi_selected.is_empty() {
            // `895-bulk-delete-branches`
            self.multi_selected.contains(&branch.name)
        } else {
            self.shown_selected.as_deref() == Some(branch.name.as_str())
        };
        let focused = keyboard || self.list_focused;
        let item = branch_list_item(
            &branch.name,
            current,
            branch
                .tip_time
                .filter(|s| *s > 0)
                .map(|s| UNIX_EPOCH + Duration::from_secs(s as u64)),
            std::time::SystemTime::now(),
        );
        let date = item.author_date.clone();
        // `.list-item:hover`: `--list-item-hover-background-color`, text unchanged
        let list_hover = t.list_item_hover_background;
        let hover_bg = t.box_selected_active_background;
        let hover_text = t.box_selected_active_text;
        let branch_name_for_target = branch.name.clone();
        let distinguish_remote = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::BRANCH_LIST_LOCAL_REMOTE_ICONS);
        div()
            .id(SharedString::from(format!("branch-{}", branch.full_name)))
            .group("branch-row")
            .a11y_row(
                match (&date, &author) {
                    (Some(date), Some(author)) => format!("{}, {date} by {author}", branch.name),
                    (Some(date), None) => format!("{}, {date}", branch.name),
                    (None, _) => branch.name.clone(),
                },
                current,
            )
            // `.branches-list-item`: 30 px rows
            .h(zpx(30.))
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .px(SPACING())
            // `898-branch-list-folders`: under the folder's icon
            .when(nested, |d| d.pl(SPACING() + zpx(20.)))
            .cursor_pointer()
            // GHD `List.onRowMouseDown`: pressing (or right-clicking) a row
            // selects it and focuses the list; the click then checks it out
            .on_mouse_down(
                MouseButton::Left,
                cx.listener({
                    let name = branch.name.clone();
                    move |this, _, window, cx| this.select_row(name.clone(), window, cx)
                }),
            )
            .when(selected && focused, |d| {
                d.bg(t.box_selected_active_background)
                    .text_color(t.box_selected_active_text)
                    .when_some(crate::keyboard_nav::contrast_row_outline(cx), |d, ring| {
                        d.shadow(ring)
                    })
            })
            .when(selected && !focused, |d| {
                d.bg(t.box_selected_background)
                    .text_color(t.box_selected_text)
            })
            .when(
                !(selected && (focused || crate::widgets::selection_keeps_colour_on_hover(cx))),
                move |d| d.hover(move |s| s.bg(list_hover)),
            )
            .when(!current && !is_tag, move |d| {
                let target_name = branch_name_for_target.clone();
                d.drag_over::<crate::history::CommitDrag>(move |s, _, _, _| {
                    s.bg(hover_bg).text_color(hover_text)
                })
                // `emitEnterDropTarget({ type: Branch })` → "Copy to <branch>" tooltip
                .on_drag_move::<crate::history::CommitDrag>(move |ev, _, cx| {
                    if ev.bounds.contains(&ev.event.position) {
                        Dispatcher::set_drag_target(
                            Some(corvene_core::DropTarget::Branch(target_name.clone())),
                            cx,
                        );
                    }
                })
            })
            .on_click(cx.listener({
                let local = branch.kind == BranchKind::Local;
                let picked = branch.clone();
                move |this, ev: &ClickEvent, _, cx| {
                    // `895-bulk-delete-branches`: ⌘ / ⇧-click select
                    let m = ev.modifiers();
                    if (m.secondary() || m.shift)
                        && this
                            .state
                            .read(cx)
                            .flags
                            .bool(corvene_core::flags::ids::BULK_DELETE_BRANCHES)
                    {
                        if local && !current {
                            this.extend_multi_selection(name.clone(), m.shift, cx);
                        }
                        return;
                    }
                    this.multi_selected.clear();
                    pick_row(id, &picked, current, cx)
                }
            }))
            // GHD `generateBranchContextMenuItems`
            .on_mouse_down(MouseButton::Right, {
                let branch = branch.clone();
                let this = cx.entity().downgrade();
                move |ev: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    // `899-tags-in-branch-list`: a tag row's menu (`346`)
                    if let Some(sha) = tag_commit(&branch) {
                        tag_row_menu(
                            id,
                            branch.name.clone(),
                            sha.to_string(),
                            ev.position,
                            window,
                            cx,
                        );
                        return;
                    }
                    use crate::context_menu::{IS_MAC, MenuItem, mac_or};
                    // `895-bulk-delete-branches`: the menu of a multi-selection
                    let multi = this
                        .update(cx, |this, cx| {
                            if this.multi_selected.len() > 1
                                && this.multi_selected.contains(&branch.name)
                            {
                                cx.notify();
                                Some(this.multi_selected.clone())
                            } else {
                                this.multi_selected.clear();
                                this.multi_anchor = None;
                                this.select_row(branch.name.clone(), window, cx);
                                None
                            }
                        })
                        .ok()
                        .flatten();
                    if let Some(names) = multi {
                        let count = names.len();
                        let items = vec![MenuItem::new(
                            if IS_MAC {
                                format!("Delete {count} Branches…")
                            } else {
                                format!("Delete {count} branches…")
                            },
                            move |_, cx| {
                                Dispatcher::close_foldout(cx);
                                Dispatcher::show_popup(
                                    Popup::DeleteBranches {
                                        repo: id,
                                        names: names.clone(),
                                    },
                                    cx,
                                )
                            },
                        )];
                        crate::native_menu::show_context_menu(items, ev.position, window, cx);
                        return;
                    }
                    let local = branch.kind == BranchKind::Local;
                    // `856-branch-menu-rebase-onto`
                    let rebase_onto = AppState::global(cx)
                        .read(cx)
                        .flags
                        .bool(corvene_core::flags::ids::BRANCH_MENU_REBASE_ONTO)
                        .then(|| branch.name.clone());
                    // `1224-rebase-branch-onto-current`
                    let rebase_this = (local
                        && AppState::global(cx)
                            .read(cx)
                            .flags
                            .bool(corvene_core::flags::ids::REBASE_BRANCH_ONTO_CURRENT))
                    .then(|| branch.name.clone());
                    let can_rebase = !current && {
                        let s = AppState::global(cx).read(cx);
                        s.repo_states.get(&id).is_some_and(|r| {
                            r.mco.is_none()
                                && r.info
                                    .as_ref()
                                    .is_some_and(|i| matches!(i.tip, Tip::Valid { .. }))
                        })
                    };
                    let (rename, copy, worktree, delete) = (
                        branch.name.clone(),
                        branch.name.clone(),
                        branch.name.clone(),
                        branch.name.clone(),
                    );
                    // Corvene addition (`857-update-branch-from-upstream`)
                    let update = (local && !current)
                        .then(|| branch.upstream_short())
                        .flatten()
                        .filter(|_| {
                            corvene_core::AppState::global(cx)
                                .read(cx)
                                .flags
                                .bool(corvene_core::flags::ids::UPDATE_BRANCH_FROM_UPSTREAM)
                        })
                        .map(|upstream| {
                            let name = branch.name.clone();
                            MenuItem::new(format!("Update from {upstream}"), move |_, cx| {
                                Dispatcher::close_foldout(cx);
                                Dispatcher::update_branch_from_upstream(id, name.clone(), cx)
                            })
                        });
                    // Corvene (`1204-restore-stash-from-other-branch`): another
                    // branch's stash onto this one, with a clean working directory
                    let restore_stash = {
                        let s = AppState::global(cx).read(cx);
                        (local
                            && !current
                            && s.flags
                                .bool(corvene_core::flags::ids::RESTORE_STASH_FROM_OTHER_BRANCH))
                        .then(|| s.repo_states.get(&id))
                        .flatten()
                        .filter(|r| r.stashed_branches.contains(&branch.name))
                        .map(|r| r.changed_files() == 0)
                    }
                    .map(|clean| {
                        let name = branch.name.clone();
                        MenuItem::new(
                            mac_or("Restore Stash Here", "Restore stash here"),
                            move |_, cx| {
                                Dispatcher::close_foldout(cx);
                                Dispatcher::restore_stash_from_branch(id, name.clone(), cx)
                            },
                        )
                        .enabled(clean)
                    });
                    // Corvene (`897-pinned-branches`)
                    let pin = {
                        let s = AppState::global(cx).read(cx);
                        s.flags
                            .bool(corvene_core::flags::ids::PINNED_BRANCHES)
                            .then(|| {
                                s.repository(id)
                                    .is_some_and(|r| r.pinned_branches.contains(&branch.name))
                            })
                    }
                    .map(|pinned| {
                        let name = branch.name.clone();
                        MenuItem::new(if pinned { "Unpin" } else { "Pin" }, move |_, cx| {
                            Dispatcher::set_branch_pinned(id, name.clone(), !pinned, cx)
                        })
                    });
                    let mut items = vec![
                        MenuItem::new("Rename…", move |_, cx| {
                            Dispatcher::close_foldout(cx);
                            Dispatcher::show_popup(
                                Popup::RenameBranch {
                                    repo: id,
                                    name: rename.clone(),
                                },
                                cx,
                            )
                        })
                        .enabled(local),
                        MenuItem::new(
                            mac_or("Copy Branch Name", "Copy branch name"),
                            move |_, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                            },
                        ),
                        MenuItem::new(
                            mac_or("Checkout in New Worktree…", "Checkout in new worktree…"),
                            move |_, cx| {
                                Dispatcher::close_foldout(cx);
                                Dispatcher::show_popup(
                                    Popup::AddWorktree {
                                        repo: id,
                                        initial_branch_name: Some(worktree.clone()),
                                        initial_worktree_name: None,
                                    },
                                    cx,
                                )
                            },
                        ),
                        MenuItem::separator(),
                    ];
                    // Corvene (`1218-compare-refs`): this branch against the
                    // current one
                    if corvene_core::ref_compare::enabled(AppState::global(cx).read(cx)) {
                        let base = branch.name.clone();
                        let head = crate::history::current_ref(id, cx).filter(|_| !current);
                        items.push(MenuItem::new("Compare with…", move |_, cx| {
                            Dispatcher::show_compare_refs(id, Some(base.clone()), head.clone(), cx)
                        }));
                        items.push(MenuItem::separator());
                    }
                    if let Some(base) = rebase_onto.clone() {
                        items.push(
                            MenuItem::new(
                                if IS_MAC {
                                    format!("Rebase Current Branch onto {base}…")
                                } else {
                                    format!("Rebase current branch onto {base}…")
                                },
                                move |_, cx| {
                                    Dispatcher::close_foldout(cx);
                                    Dispatcher::start_rebase_flow_onto(id, Some(base.clone()), cx);
                                },
                            )
                            .enabled(can_rebase),
                        );
                    }
                    if let Some(name) = rebase_this.clone() {
                        items.push(
                            MenuItem::new(
                                if IS_MAC {
                                    format!("Rebase {name} onto Current Branch")
                                } else {
                                    format!("Rebase {name} onto current branch")
                                },
                                move |_, cx| {
                                    Dispatcher::close_foldout(cx);
                                    Dispatcher::rebase_branch_onto_current(id, name.clone(), cx);
                                },
                            )
                            .enabled(can_rebase),
                        );
                    }
                    if rebase_onto.is_some() || rebase_this.is_some() {
                        items.push(MenuItem::separator());
                    }
                    // GHD `onViewPullRequestOnGitHub`, before Checkout in New
                    // Worktree…
                    let pull_request = AppState::global(cx)
                        .read(cx)
                        .pull_request_of_branch(id, &branch)
                        .cloned();
                    if let Some(pr) = pull_request {
                        let at = items
                            .iter()
                            .position(|item| item.label.contains("orktree"))
                            .unwrap_or(items.len());
                        items.insert(
                            at,
                            MenuItem::new("View Pull Request on GitHub", move |_, cx| {
                                Dispatcher::close_foldout(cx);
                                Dispatcher::open_pull_request(&pr, cx);
                            }),
                        );
                    }
                    items.extend([MenuItem::new("Delete…", move |_, cx| {
                        Dispatcher::close_foldout(cx);
                        Dispatcher::show_popup(
                            Popup::DeleteBranch {
                                repo: id,
                                name: delete.clone(),
                            },
                            cx,
                        )
                    })]);
                    if let Some(update) = update {
                        items.insert(2, update);
                    }
                    if let Some(restore_stash) = restore_stash {
                        let at = items
                            .iter()
                            .position(|i| {
                                matches!(i.kind, crate::context_menu::MenuItemKind::Separator)
                            })
                            .unwrap_or(items.len());
                        items.insert(at, restore_stash);
                    }
                    if let Some(pin) = pin {
                        let at = items
                            .iter()
                            .position(|i| {
                                matches!(i.kind, crate::context_menu::MenuItemKind::Separator)
                            })
                            .unwrap_or(items.len());
                        items.insert(at, pin);
                    }
                    crate::native_menu::show_context_menu(items, ev.position, window, cx);
                }
            })
            .on_drop({
                // `startCherryPickWithBranch`: drop commits on a branch to copy them there
                let target = branch.name.clone();
                move |drag: &crate::history::CommitDrag, _, cx| {
                    Dispatcher::set_drag_target(None, cx);
                    if current || is_tag || drag.repo != id {
                        return;
                    }
                    Dispatcher::close_foldout(cx);
                    Dispatcher::start_cherry_pick_flow(id, drag.shas.clone(), cx);
                    Dispatcher::cherry_pick_to_branch(id, target.clone(), cx);
                }
            })
            .child(
                octicon(
                    if is_tag {
                        Octicon::Tag
                    } else if distinguish_remote && !current {
                        // `850-branch-list-local-remote-icons`
                        match (branch.kind, branch.upstream.is_some()) {
                            (BranchKind::Remote, _) => Octicon::Server,
                            (BranchKind::Local, false) => Octicon::DeviceDesktop,
                            (BranchKind::Local, true) => Octicon::GitBranch,
                        }
                    } else {
                        item.icon
                    },
                    t.text,
                )
                .mr(SPACING_HALF()),
            )
            .child(
                div()
                    .flex_grow(2.)
                    .min_w_0()
                    .max_w(gpui_kit::relative(0.65))
                    .mr(SPACING_HALF())
                    .truncate()
                    .text_size(FONT_SIZE())
                    .child(item.name),
            )
            .when_some(push_target, |d, m| {
                d.child(
                    div()
                        .id(SharedString::from(format!(
                            "branch-push-target-{}",
                            branch.full_name
                        )))
                        .flex_shrink(1.)
                        .min_w_0()
                        .mr(SPACING_HALF())
                        .truncate()
                        .text_size(FONT_SIZE_SM())
                        .when(!selected, |d| d.text_color(t.text_secondary))
                        .child(format!("→ {}", m.upstream))
                        .ghd_tooltip(m.describe()),
                )
            })
            .when_some(sync_state, |d, (tooltip, counts)| {
                let content = match counts {
                    None => div()
                        .flex()
                        .child(octicon(Octicon::Upload, t.text_secondary))
                        .into_any_element(),
                    Some((ahead, behind)) => div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(zpx(2.))
                        .text_size(FONT_SIZE_SM())
                        .when(!selected, |d| d.text_color(t.text_secondary))
                        .when(ahead > 0, |d| d.child(format!("{ahead}↑")))
                        .when(behind > 0, |d| d.child(format!("{behind}↓")))
                        .into_any_element(),
                };
                d.child(
                    div()
                        .id(SharedString::from(format!(
                            "branch-sync-{}",
                            branch.full_name
                        )))
                        .flex_none()
                        .mr(SPACING_HALF())
                        .child(content)
                        .ghd_tooltip(tooltip),
                )
            })
            .when(tracking.is_some_and(|t| t.gone), |d| {
                d.child(
                    div()
                        .id(SharedString::from(format!(
                            "branch-gone-{}",
                            branch.full_name
                        )))
                        .flex_none()
                        .mr(SPACING_HALF())
                        .child(octicon(Octicon::CloudOffline, t.text_secondary))
                        .ghd_tooltip("Deleted on the remote"),
                )
            })
            .when(stashed, |d| {
                d.child(
                    div()
                        .id(SharedString::from(format!(
                            "branch-stash-{}",
                            branch.full_name
                        )))
                        .flex_none()
                        .mr(SPACING_HALF())
                        .child(octicon(Octicon::Stash, t.text_secondary))
                        .ghd_tooltip("Stashed changes"),
                )
            })
            .when_some(date, |d, date| {
                d.child(
                    div()
                        .flex_1()
                        .mr(SPACING_HALF())
                        .text_right()
                        .whitespace_nowrap()
                        .text_size(FONT_SIZE_SM())
                        .line_height(zpx(16.5))
                        // the selected row's date takes the row colour
                        .when(!selected, |d| d.text_color(t.text_secondary))
                        .child(date),
                )
            })
            .when_some(author, |d, author| {
                d.child(
                    div()
                        .flex_none()
                        .max_w(zpx(110.))
                        .mr(SPACING_HALF())
                        .truncate()
                        .text_size(FONT_SIZE_SM())
                        .line_height(zpx(16.5))
                        .when(!selected, |d| d.text_color(t.text_secondary))
                        .child(format!("by {author}")),
                )
            })
            // Shift+F10 / Menu
            .when(selected, |d| d.child(self.menu_anchor.track()))
            // `621-context-menu-buttons` (a tag row has no menu)
            .when(!is_tag && crate::context_menu::row_menu_buttons(cx), |d| {
                d.child(crate::context_menu::row_menu_button(
                    "row-menu",
                    "branch-row",
                    selected,
                    if selected && focused {
                        t.box_selected_active_text
                    } else {
                        t.text_secondary
                    },
                ))
            })
    }

    /// `895-bulk-delete-branches`: ⌘-click toggles `name` in the
    /// multi-selection, ⇧-click adds the local branches shown between the
    /// last clicked row and `name`. The current branch is never selected.
    fn extend_multi_selection(&mut self, name: String, range: bool, cx: &mut Context<Self>) {
        let current = {
            let s = self.state.read(cx);
            s.selected
                .and_then(|id| s.repo_states.get(&id))
                .and_then(|rs| rs.info.as_ref())
                .and_then(|i| i.current_branch())
                .map(|b| b.name.clone())
        };
        let shown: Vec<String> = self
            .branch_groups(cx)
            .into_iter()
            .flat_map(|g| g.branches)
            .filter(|b| b.kind == BranchKind::Local && Some(&b.name) != current.as_ref())
            .map(|b| b.name)
            .collect();
        let anchor = self
            .multi_anchor
            .as_ref()
            .and_then(|a| shown.iter().position(|n| n == a));
        match (range, anchor, shown.iter().position(|n| *n == name)) {
            (true, Some(from), Some(to)) => {
                for n in &shown[from.min(to)..=from.max(to)] {
                    if !self.multi_selected.contains(n) {
                        self.multi_selected.push(n.clone());
                    }
                }
            }
            _ => {
                if let Some(ix) = self.multi_selected.iter().position(|n| *n == name) {
                    self.multi_selected.remove(ix);
                } else {
                    self.multi_selected.push(name.clone());
                }
                self.multi_anchor = Some(name);
            }
        }
        cx.notify();
    }

    fn select_row(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.selected_row = Some(name);
        self.highlighted = None;
        window.focus(&self.list_focus, cx);
        cx.notify();
    }

    /// `NoBranches`: shown when the filter matches nothing. `.no-branches`
    /// in a resizable `.branches-container`: 365 px wide, 10 px margin and
    /// padding, the illustration at full width (`.foldout .blankslate-image`).
    fn no_branches(&self, id: u64, query: String, cx: &Context<Self>) -> AnyElement {
        let container = div()
            .flex_none()
            .w(zpx(365.))
            .mx_auto()
            .my(SPACING())
            .p(SPACING())
            .flex()
            .flex_col()
            .items_center()
            .text_center()
            .text_size(FONT_SIZE())
            .line_height(zpx(18.));
        let NoBranchesContent::CreateBranch {
            blankslate_image,
            title,
            subtitle,
            button: (label, NoBranchesAction::CreateNewBranch),
            ..
        } = no_branches(true, None)
        else {
            return container.into_any_element();
        };
        let (lead, keys, tail) = NO_BRANCHES_PROTIP;
        container
            // 257 × 85 at the 345 px content width
            .when(blankslate_image, |d| {
                d.child(
                    crate::widgets::blankslate_image("empty-no-branches.svg", cx)
                        .w(zpx(345.))
                        .h(zpx(345. * 85. / 257.)),
                )
            })
            .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
            .child(
                div()
                    .mx(SPACING_DOUBLE())
                    .text_size(FONT_SIZE_SM())
                    .line_height(zpx(16.5))
                    .child(subtitle),
            )
            .child(
                crate::widgets::primary_button("no-branches-create", label, false, cx)
                    .m(SPACING_DOUBLE())
                    .self_stretch()
                    .on_click(move |_, _, cx| {
                        Dispatcher::close_foldout(cx);
                        Dispatcher::show_popup(
                            Popup::CreateBranch {
                                repo: id,
                                target_sha: None,
                                initial_name: query.clone(),
                            },
                            cx,
                        )
                    }),
            )
            // Corvene (`1223-checkout-from-fork`): `owner:branch` that no
            // branch matches checks it out from that owner's fork
            .when_some(
                fork_branch_query(&self.filter.read(cx).value(), cx),
                |d, (owner, branch)| {
                    d.child(
                        crate::widgets::button(
                            "no-branches-checkout-from-fork",
                            if crate::context_menu::IS_MAC {
                                format!("Check Out {owner}:{branch} from Fork…")
                            } else {
                                format!("Check out {owner}:{branch} from fork…")
                            },
                            cx,
                        )
                        .mx(SPACING_DOUBLE())
                        .mb(SPACING_DOUBLE())
                        .self_stretch()
                        .on_click(move |_, _, cx| {
                            Dispatcher::show_checkout_from_fork(
                                id,
                                Some(owner.clone()),
                                Some(branch.clone()),
                                cx,
                            )
                        }),
                    )
                },
            )
            .child(
                // `.protip` with a `KeyboardShortcut` (⌘⇧N) in the sentence
                crate::widgets::paragraph(vec![
                    lead.into(),
                    // `kbd` inherits the 11 px `.protip` text
                    crate::widgets::kbd_group_sized(&keys, FONT_SIZE_SM(), cx)
                        .into_any_element()
                        .into(),
                    tail.into(),
                ])
                .justify_center()
                .px(SPACING() * 3.)
                .text_size(FONT_SIZE_SM())
                .line_height(zpx(16.5)),
            )
            .into_any_element()
    }
}

impl Render for BranchFoldout {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd();
        self.list_focused = self.list_focus.is_focused(window);
        let query = strip_owner_prefix(self.filter.read(cx).value().trim(), cx);
        let remote_toggle = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::BRANCH_LIST_REMOTE_ONLY);
        let remote_only = remote_toggle && self.remote_only;
        let (groups, folders) = self.folded_groups(cx);
        let (id, current, tip_valid, stashed, tracking) = {
            let s = self.state.read(cx);
            let id = s.selected;
            let rs = id.and_then(|id| s.repo_states.get(&id));
            let stashed = rs
                .filter(|_| {
                    s.flags
                        .bool(corvene_core::flags::ids::BRANCH_LIST_STASH_ICON)
                })
                .map(|rs| rs.stashed_branches.clone())
                .unwrap_or_default();
            let tracking = rs.map(|rs| rs.branch_tracking.clone()).unwrap_or_default();
            let info = rs.and_then(|r| r.info.as_ref());
            let current = info
                .and_then(|i| i.current_branch())
                .map(|b| b.name.clone());
            let tip_valid = info.is_some_and(|i| matches!(i.tip, Tip::Valid { .. }));
            (id, current, tip_valid, stashed, tracking)
        };
        let row_count: usize = groups.iter().map(|g| g.branches.len()).sum();
        self.highlighted = self.highlighted.filter(|ix| *ix < row_count);
        let highlighted = self.highlighted;
        let Some(id) = id else {
            return div().into_any_element();
        };
        let visible = |name: &str| {
            groups
                .iter()
                .any(|g| g.branches.iter().any(|b| b.name == name))
        };
        self.shown_selected = match (&self.selected_row, &current) {
            (Some(row), _) if visible(row) => Some(row.clone()),
            (None, Some(cur)) if visible(cur) => Some(cur.clone()),
            _ if !query.is_empty() => groups
                .iter()
                .flat_map(|g| g.branches.first())
                .next()
                .map(|b| b.name.clone()),
            _ => None,
        };
        let query_for_new = query.clone();
        let is_github = self.is_github(cx);
        let tab = if is_github {
            self.state.read(cx).branches_tab
        } else {
            BranchesTab::Branches
        };
        let open_prs = self.state.read(cx).pull_requests_for(id).len();
        if tab == BranchesTab::PullRequests {
            // `CIStatus.subscribe` for every row on screen
            let refs: Vec<(corvene_core::GitHubRepository, String)> = self
                .pull_request_items(id, cx)
                .iter()
                .filter_map(|pr| pr.base.repository.clone().map(|r| (r, pr.commit_ref())))
                .collect();
            for (base, git_ref) in refs {
                Dispatcher::touch_commit_status(&base, &git_ref, None, cx);
            }
            let container_bounds = self.container_bounds.clone();
            return div()
                .id("branches-container")
                .relative()
                .size_full()
                .flex()
                .flex_col()
                .min_h_0()
                .child(
                    canvas(move |b, _, _| container_bounds.set(b), |_, _, _, _| {})
                        .absolute()
                        .inset_0(),
                )
                .child(self.tab_bar(open_prs, tab, cx))
                .child(self.pull_requests_tab(id, window, cx))
                .children(self.merge_button_row(id, current.filter(|_| tip_valid), cx))
                .children(self.render_quick_view(window, cx))
                .into_any_element();
        }
        div()
            .id("branches-container")
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .when(is_github, |d| d.child(self.tab_bar(open_prs, tab, cx)))
            .child(
                // `.filter-field-row`: [🔍 Filter][New Branch]
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING())
                    .p(SPACING())
                    .key_context("BranchFilter")
                    .on_action(self.menu_anchor.action_handler())
                    .on_action(
                        cx.listener(|this, _: &SelectNextFile, _, cx| this.move_highlight(1, cx)),
                    )
                    .on_action(cx.listener(|this, _: &SelectPreviousFile, _, cx| {
                        this.move_highlight(-1, cx)
                    }))
                    .on_action(
                        cx.listener(|this, _: &FilterListPick, _, cx| this.pick_highlighted(cx)),
                    )
                    .child(crate::widgets::filter_text_box(
                        "branch-filter",
                        &self.filter,
                        Some(octicon(Octicon::Search, t.text_secondary)),
                        window,
                        cx,
                    ))
                    .when(remote_toggle, |d| {
                        d.child(
                            button("branch-remote-only", "", cx)
                                .flex_none()
                                .px(SPACING_HALF())
                                .when(remote_only, |d| d.bg(t.box_selected_background))
                                .icon_button_label(if remote_only {
                                    "Show all branches"
                                } else {
                                    "Show only remote branches"
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.remote_only = !this.remote_only;
                                    this.highlighted = None;
                                    cx.notify();
                                }))
                                .child(octicon(Octicon::Server, t.secondary_button_text)),
                        )
                    })
                    .child(
                        button("new-branch", mac_or("New Branch", "New branch"), cx)
                            .flex_none()
                            .on_click(move |_, _, cx| {
                                Dispatcher::close_foldout(cx);
                                Dispatcher::show_popup(
                                    Popup::CreateBranch {
                                        repo: id,
                                        target_sha: None,
                                        initial_name: query_for_new.clone(),
                                    },
                                    cx,
                                )
                            }),
                    ),
            )
            .child(if groups.is_empty() {
                // the filter list keeps growing; `.no-branches` sits at its top
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(self.no_branches(id, query, cx))
                    .into_any_element()
            } else {
                // one uniform list of group headers and rows (both 30 px), so
                // only the rows on screen are built: the whole list was built
                // every frame (and every keystroke in the filter) before
                // (a branch row counts only branches: the keyboard
                // highlight's index)
                let items = list_items(&groups, &folders);
                let count = items.len();
                let groups = std::rc::Rc::new(groups);
                let list_hover = t.list_item_hover_background;
                let secondary = t.text_secondary;
                let icon_colour = t.text;
                let current = current.clone();
                div()
                    .id("branches-list")
                    .key_context("BranchList")
                    .on_action(self.menu_anchor.action_handler())
                    .track_focus(&self.list_focus)
                    .role(Role::List)
                    .aria_label("Branches")
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        uniform_list(
                            "branches-list-rows",
                            count,
                            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                                range
                                    .map(|ix| match items[ix] {
                                        // `.filter-list-group-header`
                                        ListItem::Header(g) => div()
                                            .h(zpx(30.))
                                            .px(SPACING())
                                            .flex()
                                            .items_center()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_size(FONT_SIZE())
                                            .child(groups[g].title)
                                            .into_any_element(),
                                        // `898-branch-list-folders`
                                        ListItem::Folder(f) => {
                                            let folder = &folders[f];
                                            let name = folder.name.clone();
                                            div()
                                                .id(SharedString::from(format!(
                                                    "branch-folder-{}",
                                                    folder.name
                                                )))
                                                .a11y_row(
                                                    format!(
                                                        "{} folder, {} branches",
                                                        folder.name, folder.count
                                                    ),
                                                    false,
                                                )
                                                .h(zpx(30.))
                                                .w_full()
                                                .px(SPACING())
                                                .flex()
                                                .flex_row()
                                                .items_center()
                                                .gap(SPACING_HALF())
                                                .cursor_pointer()
                                                .hover(move |s| s.bg(list_hover))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.toggle_folder(name.clone(), cx)
                                                }))
                                                .child(octicon(
                                                    if folder.expanded {
                                                        Octicon::ChevronDown
                                                    } else {
                                                        Octicon::ChevronRight
                                                    },
                                                    secondary,
                                                ))
                                                .child(octicon(Octicon::FileDirectory, icon_colour))
                                                .child(
                                                    div()
                                                        .min_w_0()
                                                        .truncate()
                                                        .text_size(FONT_SIZE())
                                                        .child(folder.name.clone()),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(FONT_SIZE_SM())
                                                        .text_color(secondary)
                                                        .child(folder.count.to_string()),
                                                )
                                                .into_any_element()
                                        }
                                        ListItem::Branch {
                                            group: g,
                                            ix: b,
                                            row,
                                            nested,
                                        } => {
                                            let b = &groups[g].branches[b];
                                            this.row(
                                                id,
                                                b,
                                                nested,
                                                current.as_deref() == Some(b.name.as_str()),
                                                highlighted == Some(row),
                                                b.kind == BranchKind::Local
                                                    && stashed.contains(&b.name),
                                                (b.kind == BranchKind::Local)
                                                    .then(|| tracking.get(&b.name).copied())
                                                    .flatten(),
                                                cx,
                                            )
                                            .into_any_element()
                                        }
                                    })
                                    .collect()
                            }),
                        )
                        .flex_1()
                        .min_h_0()
                        .with_scrollbar_handle(&self.scroll),
                    )
                    .into_any_element()
            })
            .children(self.merge_button_row(id, current.filter(|_| tip_valid), cx))
            .into_any_element()
    }
}

impl BranchFoldout {
    /// `renderTabBar`: Branches | Pull Requests (with the open count bubble).
    fn tab_bar(&self, open_prs: usize, tab: BranchesTab, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        div()
            .flex_none()
            .border_t_1()
            .border_color(t.box_border)
            .child(tab_bar(
                vec![
                    TabModel {
                        dot: false,
                        id: "branches-tab",
                        label: "Branches".into(),
                        count: None,
                    },
                    TabModel {
                        dot: false,
                        id: "pull-requests-tab",
                        label: if self.merge_requests(cx) {
                            mac_or("Merge Requests", "Merge requests")
                        } else {
                            mac_or("Pull Requests", "Pull requests")
                        }
                        .into(),
                        count: (open_prs > 0).then(|| open_prs.to_string().into()),
                    },
                ],
                match tab {
                    BranchesTab::Branches => 0,
                    BranchesTab::PullRequests => 1,
                },
                |ix, _, cx| {
                    Dispatcher::change_branches_tab(
                        if ix == 0 {
                            BranchesTab::Branches
                        } else {
                            BranchesTab::PullRequests
                        },
                        cx,
                    )
                },
                cx,
            ))
            .into_any_element()
    }

    /// `.merge-button-row`: "Choose a branch to merge into <current>".
    fn merge_button_row(
        &self,
        id: u64,
        current: Option<String>,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let t = cx.ghd();
        let current = current?;
        Some(
            div()
                .flex_none()
                .p(SPACING())
                .border_t_1()
                .border_color(t.box_border)
                .child(
                    button("merge-into-current", "", cx)
                        .w_full()
                        .justify_center()
                        .gap(SPACING_HALF())
                        .child(octicon(Octicon::GitMerge, t.secondary_button_text))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .child("Choose a branch to merge into\u{a0}")
                                .child(div().font_weight(FontWeight::SEMIBOLD).child(current)),
                        )
                        .on_click(move |_, _, cx| {
                            Dispatcher::close_foldout(cx);
                            Dispatcher::show_popup(
                                Popup::MergeBranch {
                                    repo: id,
                                    squash: false,
                                },
                                cx,
                            )
                        }),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(name: &str, upstream: Option<&str>) -> Branch {
        Branch {
            name: name.into(),
            kind: BranchKind::Local,
            full_name: format!("refs/heads/{name}"),
            tip: None,
            upstream: upstream.map(|u| format!("refs/remotes/origin/{u}")),
            tip_time: None,
            tip_author: None,
            remote_name: None,
        }
    }

    fn remote(name: &str) -> Branch {
        Branch {
            name: format!("origin/{name}"),
            kind: BranchKind::Remote,
            full_name: format!("refs/remotes/origin/{name}"),
            tip: None,
            upstream: None,
            tip_time: None,
            tip_author: None,
            remote_name: Some("origin".into()),
        }
    }

    #[::core::prelude::v1::test]
    fn folders_group_shared_prefixes_where_the_first_member_was() {
        let all = vec![
            local("main", None),
            local("alpha", None),
            local("feature/a", None),
            local("feature/b", None),
            local("solo/x", None),
            local("zeta", None),
        ];
        let groups = group_branches(&all, Some("main"), &[], "", false);
        let names = |g: &BranchGroup| {
            g.branches
                .iter()
                .map(|b| b.name.clone())
                .collect::<Vec<_>>()
        };

        let (closed, folders) = fold_branches(groups, &HashSet::new());
        assert_eq!(names(&closed[1]), ["alpha", "solo/x", "zeta"]);
        assert_eq!(
            folders,
            [BranchFolder {
                group: 1,
                before: 1,
                name: "feature".into(),
                count: 2,
                expanded: false,
            }]
        );
        let items = list_items(&closed, &folders);
        assert_eq!(items[2], ListItem::Header(1));
        assert_eq!(items[4], ListItem::Folder(0));
        assert!(matches!(
            items[5],
            ListItem::Branch {
                row: 2,
                nested: false,
                ..
            }
        ));

        let groups = group_branches(&all, Some("main"), &[], "", false);
        let (open, folders) = fold_branches(groups, &HashSet::from(["feature".to_string()]));
        assert_eq!(
            names(&open[1]),
            ["alpha", "feature/a", "feature/b", "solo/x", "zeta"]
        );
        let items = list_items(&open, &folders);
        assert!(matches!(
            items[5],
            ListItem::Branch {
                ix: 1,
                nested: true,
                ..
            }
        ));
        assert!(matches!(
            items[7],
            ListItem::Branch {
                ix: 3,
                nested: false,
                ..
            }
        ));
    }

    #[::core::prelude::v1::test]
    fn pinned_branches_follow_the_default_branch_in_pin_order() {
        let all = vec![
            local("main", None),
            local("a", None),
            local("b", None),
            local("c", None),
        ];
        let groups = group_branches(&all, Some("main"), &["a".into()], "", false);
        let groups = group_pinned(
            groups,
            &["c".into(), "a".into(), "main".into(), "gone".into()],
        );
        let names = |g: &BranchGroup| {
            g.branches
                .iter()
                .map(|b| b.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(groups.len(), 3);
        assert_eq!(names(&groups[0]), ["main"]);
        assert_eq!(groups[1].title, "Pinned");
        assert_eq!(names(&groups[1]), ["c", "a"]);
        assert_eq!(names(&groups[2]), ["b"]);
    }

    #[::core::prelude::v1::test]
    fn gone_branches_move_to_their_own_group_last() {
        let all = vec![
            local("main", Some("main")),
            local("current", Some("current")),
            local("kept", Some("kept")),
            local("merged", Some("merged")),
            local("unpublished", None),
            remote("main"),
            remote("kept"),
        ];
        let groups = group_branches(&all, Some("main"), &["merged".into()], "", false);
        let groups = group_upstream_gone(groups, &all, &["current", "main"]);
        let titles: Vec<&str> = groups.iter().map(|g| g.title).collect();
        assert_eq!(titles.len(), 3);
        assert_eq!(titles[0], mac_or("Default Branch", "Default branch"));
        assert_eq!(titles[2], mac_or("Deleted on Remote", "Deleted on remote"));
        let names = |g: &BranchGroup| {
            g.branches
                .iter()
                .map(|b| b.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&groups[1]), ["current", "kept", "unpublished"]);
        assert_eq!(names(&groups[2]), ["merged"]);
    }
}
