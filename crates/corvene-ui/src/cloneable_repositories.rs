//! `CloneableRepositoryFilterList` (`ui/clone-repository/cloneable-repository-filter-list.tsx`,
//! `group-repositories.ts`) and `AccountPicker` (`ui/account-picker.tsx`,
//! `styles/ui/_account-picker.scss`), shared by Clone a Repository's account
//! tabs and the signed-in blank slate (`no-repositories-view.tsx`).
//!
//! Deviation (`277-consistent-filter-highlight`): the filter's matched
//! characters can be bold like in the branch and repository lists instead of
//! GHD's black-on-yellow `<mark>`.
//!
//! Deviation (`227-clone-filter-accepts-urls`): a repository URL pasted
//! into the filter (`https://github.com/owner/name`, `git@host:owner/name.git`,
//! a browser URL deeper into the repository) filters as `owner/name`; GHD
//! fuzzy-matches the whole URL and finds nothing.
//!
//! Deviation (`228-clone-default-account`): the account picker can start
//! on a chosen account instead of the first one signed in.
//!
//! Deviation (`292-clone-owner-picker`): an owner menu beside the filter
//! narrows the list to one user's or organization's repositories,
//! remembered per account.
//!
//! Callers own the state (filter text box, selected clone URL, picked
//! account, popover open) and wrap the pieces in their own layout; the two
//! places differ only in insets and the list's frame.

use std::cell::Cell;
use std::rc::Rc;

use corvene_core::{Account, Dispatcher, GitHubRepository};
use gpui_kit::component::input::InputState;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::context_menu::mac_or;
use crate::icons::{Octicon, octicon};
use crate::scrollbar::ScrollbarExt;
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::{
    GhdTooltip, IconButtonA11y, avatar_image, avatar_lookup_url, button, link_button,
};

/// GHD `cloneable-repository-filter-list.tsx` group title for the
/// signed-in user's own repositories.
#[doc(hidden)]
pub const YOUR_REPOSITORIES: &str = mac_or("Your Repositories", "Your repositories");

/// `RowHeight` of the cloneable repository list.
#[allow(non_snake_case)]
pub fn LIST_ROW_HEIGHT() -> Pixels {
    zpx(31.)
}

/// `AccountPicker` `rowHeight`.
#[allow(non_snake_case)]
fn ACCOUNT_ROW_HEIGHT() -> Pixels {
    zpx(47.)
}

/// A picked repository (a list row clicked, with the click's modifiers).
pub type OnRepository = Rc<dyn Fn(&GitHubRepository, Modifiers, &mut Window, &mut App)>;
/// A picked account (a popover row clicked).
pub type OnAccount = Rc<dyn Fn(&Account, &mut Window, &mut App)>;
/// The popover closed.
pub type OnClose = Rc<dyn Fn(&mut Window, &mut App)>;

/// One row of the flattened, filtered repository list; an item carries
/// the char positions the filter matched.
#[derive(Clone)]
pub enum CloneRow {
    Header(String),
    Item(GitHubRepository, Vec<usize>),
}

/// Corvene (`229-hidden-clone-owners`): the owners whose repositories the
/// clone lists leave out (the flag's comma-separated logins, lower-cased).
pub fn hidden_owners(cx: &App) -> Vec<String> {
    corvene_core::AppState::global(cx)
        .read(cx)
        .flags
        .text(corvene_core::flags::ids::HIDDEN_CLONE_OWNERS)
        .split(',')
        .map(|o| o.trim().to_lowercase())
        .filter(|o| !o.is_empty())
        .collect()
}

/// `repos` without those of `hidden` owners (see [`hidden_owners`]).
pub fn without_hidden_owners(
    repos: &[GitHubRepository],
    hidden: &[String],
) -> Vec<GitHubRepository> {
    repos
        .iter()
        .filter(|r| !hidden.contains(&r.owner.to_lowercase()))
        .cloned()
        .collect()
}

/// The repositories the clone lists show for `account`: without the
/// `229-hidden-clone-owners`, and only the owner `292-clone-owner-picker`
/// picked for the account.
pub fn visible_repositories(
    repos: &[GitHubRepository],
    account: &Account,
    cx: &App,
) -> Vec<GitHubRepository> {
    let mut repos = without_hidden_owners(repos, &hidden_owners(cx));
    if let Some(owner) = picked_owner(&repos, account, cx) {
        repos.retain(|r| r.owner == owner);
    }
    repos
}

/// `292-clone-owner-picker`: the settings key of an account's pick.
fn owner_key(account: &Account) -> String {
    format!("{}|{}", account.endpoint, account.login)
}

/// `292-clone-owner-picker`: the owner picked for `account`, while it still
/// has repositories in `repos`.
fn picked_owner(repos: &[GitHubRepository], account: &Account, cx: &App) -> Option<String> {
    let s = corvene_core::AppState::global(cx).read(cx);
    if !s.flags.bool(corvene_core::flags::ids::CLONE_OWNER_PICKER) {
        return None;
    }
    let owner = s.settings.clone_owner_filter.get(&owner_key(account))?;
    repos
        .iter()
        .any(|r| r.owner == *owner)
        .then(|| owner.clone())
}

/// `292-clone-owner-picker`: the owners of `repos` (hidden ones left out),
/// the account's own login first, then by name ignoring case.
pub fn owners(repos: &[GitHubRepository], login: &str) -> Vec<String> {
    let mut owners: Vec<String> = repos.iter().map(|r| r.owner.clone()).collect();
    owners.sort_by_key(|o| (!o.eq_ignore_ascii_case(login), o.to_lowercase()));
    owners.dedup();
    owners
}

/// `292-clone-owner-picker`: "All Owners ▾" (or the picked owner) beside
/// the filter box; a menu of the account's owners narrows the list to one.
/// `None` with the flag off or fewer than two owners.
pub fn owner_picker(
    id: &'static str,
    account: &Account,
    repos: &[GitHubRepository],
    cx: &App,
) -> Option<Stateful<Div>> {
    let flags = &corvene_core::AppState::global(cx).read(cx).flags;
    if !flags.bool(corvene_core::flags::ids::CLONE_OWNER_PICKER) {
        return None;
    }
    let shown = without_hidden_owners(repos, &hidden_owners(cx));
    let owners = owners(&shown, &account.login);
    if owners.len() < 2 {
        return None;
    }
    let picked = picked_owner(&shown, account, cx);
    let t = cx.ghd();
    let key = owner_key(account);
    let label: SharedString = picked
        .clone()
        .unwrap_or_else(|| mac_or("All Owners", "All owners").to_string())
        .into();
    Some(
        button(id, "", cx)
            .flex_none()
            .gap(zpx(5.))
            .max_w(zpx(160.))
            .ghd_tooltip("Show the repositories of one owner")
            .child(div().min_w_0().truncate().child(label))
            .child(octicon(Octicon::TriangleDown, t.secondary_button_text).size(zpx(12.)))
            .on_click(move |ev: &ClickEvent, window, cx| {
                use crate::context_menu::MenuItem;
                let pick = |owner: Option<String>| {
                    let key = key.clone();
                    move |_: &mut Window, cx: &mut App| {
                        let key = key.clone();
                        let owner = owner.clone();
                        Dispatcher::update_settings(cx, move |s| match owner {
                            Some(owner) => {
                                s.clone_owner_filter.insert(key, owner);
                            }
                            None => {
                                s.clone_owner_filter.remove(&key);
                            }
                        });
                    }
                };
                let mut items = vec![
                    MenuItem::checkbox(
                        mac_or("All Owners", "All owners"),
                        picked.is_none(),
                        pick(None),
                    ),
                    MenuItem::separator(),
                ];
                items.extend(owners.iter().map(|owner| {
                    MenuItem::checkbox(
                        owner.clone(),
                        picked.as_deref() == Some(owner.as_str()),
                        pick(Some(owner.clone())),
                    )
                }));
                crate::native_menu::show_context_menu(items, ev.position(), window, cx);
            }),
    )
}

/// `groupRepositories` + `SectionFilterList`'s filter: "Your Repositories"
/// first, then one group per owner login, groups and items in GHD's
/// `compare` order (plain `<`: capitals first, owners differing in case stay
/// apart). A filter keeps the fuzzy matches of `owner/name`
/// (`lib/fuzzy-find.ts`), best first within each group.
pub fn group_rows(repos: &[GitHubRepository], login: &str, query: &str) -> Vec<CloneRow> {
    let query = query.trim();
    let mut mine: Vec<&GitHubRepository> = Vec::new();
    let mut others: std::collections::BTreeMap<String, Vec<&GitHubRepository>> =
        std::collections::BTreeMap::new();
    for repo in repos {
        if repo.owner.eq_ignore_ascii_case(login) {
            mine.push(repo);
        } else {
            others.entry(repo.owner.clone()).or_default().push(repo);
        }
    }
    let mut rows = Vec::new();
    let mut push_group = |title: String, mut items: Vec<&GitHubRepository>| {
        items.sort_by(|a, b| a.name.cmp(&b.name));
        let mut hits: Vec<(f32, &GitHubRepository, Vec<usize>)> = items
            .into_iter()
            .filter_map(|r| {
                corvene_core::filter::fuzzy_match(query, &r.full_name())
                    .map(|(score, positions)| (score, r, positions))
            })
            .collect();
        if hits.is_empty() {
            return;
        }
        // `match` sorts by descending score; ties keep the name order
        hits.sort_by(|a, b| b.0.total_cmp(&a.0));
        rows.push(CloneRow::Header(title));
        rows.extend(
            hits.into_iter()
                .map(|(_, r, positions)| CloneRow::Item(r.clone(), positions)),
        );
    };
    push_group(YOUR_REPOSITORIES.to_string(), mine);
    for (owner, items) in others {
        push_group(owner, items);
    }
    rows
}

/// The query `group_rows` filters with: the typed filter, or with
/// `227-clone-filter-accepts-urls` a pasted URL's `owner/name`.
pub fn filter_query(query: &str, cx: &App) -> String {
    let on = corvene_core::AppState::global(cx)
        .read(cx)
        .flags
        .bool(corvene_core::flags::ids::CLONE_FILTER_ACCEPTS_URLS);
    if on {
        url_as_full_name(query).unwrap_or_else(|| query.to_string())
    } else {
        query.to_string()
    }
}

/// `owner/name` of a remote or browser URL (the first two path segments,
/// `.git` stripped); `None` for anything that is not such a URL.
pub fn url_as_full_name(query: &str) -> Option<String> {
    let (_, path) = corvene_core::split_remote(query)?;
    let mut parts = path.split(['/', '?', '#']).filter(|p| !p.is_empty());
    let owner = parts.next()?;
    let name = parts.next()?;
    let name = name.strip_suffix(".git").unwrap_or(name);
    (!name.is_empty()).then(|| format!("{owner}/{name}"))
}

/// `228-clone-default-account`: the account the picker starts on before one
/// is picked, the first of `accounts` whose login is in the flag's list,
/// else the first account (GHD).
pub fn default_account<'a>(accounts: &'a [Account], cx: &App) -> Option<&'a Account> {
    let logins = corvene_core::AppState::global(cx)
        .read(cx)
        .flags
        .text(corvene_core::flags::ids::CLONE_DEFAULT_ACCOUNT)
        .to_string();
    preferred_account(accounts, &logins).or_else(|| accounts.first())
}

/// The first of `accounts` whose login is one of the comma- or
/// space-separated `logins` (in `logins` order, case-insensitive).
pub fn preferred_account<'a>(accounts: &'a [Account], logins: &str) -> Option<&'a Account> {
    logins
        .split([',', ' '])
        .filter(|l| !l.is_empty())
        .find_map(|login| {
            accounts
                .iter()
                .find(|a| a.login.eq_ignore_ascii_case(login))
        })
}

/// `createStateUpdate` + `onSelectionChanged { kind: 'filter' }`: with a
/// filter, a selection that is filtered out moves to the first match (or to
/// nothing). `Some(new)` when the selection changes.
pub fn filtered_selection(
    rows: &[CloneRow],
    query: &str,
    selected: Option<&str>,
) -> Option<Option<GitHubRepository>> {
    if query.trim().is_empty() {
        return None;
    }
    let visible = |url: &str| {
        rows.iter()
            .any(|r| matches!(r, CloneRow::Item(repo, _) if repo.clone_url == url))
    };
    if selected.is_some_and(visible) {
        return None;
    }
    let first = rows.iter().find_map(|r| match r {
        CloneRow::Item(repo, _) => Some(repo.clone()),
        CloneRow::Header(_) => None,
    });
    (first.as_ref().map(|r| r.clone_url.as_str()) != selected).then_some(first)
}

/// How a caller lays the rows out.
#[derive(Clone, Copy)]
pub struct ListStyle {
    /// Horizontal padding of headers and items, CSS px.
    pub inset: f32,
    /// Group headers in the small secondary text (the Clone dialog) rather
    /// than the body text (`#no-repositories`).
    pub small_headers: bool,
    /// A CSS `zoom` on the list (`sizes::with_zoom`), 1 for none.
    pub zoom: f32,
    /// The list has focus: the selection takes the active colour, else the
    /// inactive one (`.list-item.selected` outside `.focus-within`).
    pub focused: bool,
}

/// `renderNoItems`: loading, no match for the filter, or an empty account;
/// centred in the list, or at its top (`centred: false`, the blank slate).
pub fn no_items(
    id: &'static str,
    account: &Account,
    loading: bool,
    loaded: bool,
    query: &str,
    centred: bool,
    cx: &App,
) -> AnyElement {
    let t = cx.ghd();
    let friendly = account.friendly_endpoint();
    let message: AnyElement = if loading && !loaded {
        div()
            .text_color(t.text_secondary)
            .child(format!("Loading repositories from {friendly}…"))
            .into_any_element()
    } else if !query.is_empty() {
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .justify_center()
            .child("Sorry, I can't find any repository matching\u{a0}")
            .child(
                div()
                    .font_family(crate::theme::mono_font())
                    .px(zpx(3.))
                    .rounded(zpx(3.))
                    .bg(t.box_alt_background)
                    .child(query.to_string()),
            )
            .into_any_element()
    } else {
        let account_for_link = account.clone();
        crate::widgets::paragraph(vec![
            "Looks like there are no repositories for ".into(),
            div()
                .font_family(crate::theme::mono_font())
                .px(zpx(3.))
                .rounded(zpx(3.))
                .bg(t.box_alt_background)
                .child(account.login.clone())
                .into_any_element()
                .into(),
            format!(" on {friendly}. ").into(),
            link_button(id, "Refresh this list", cx)
                .on_click(move |_, _, cx| {
                    Dispatcher::load_api_repositories(account_for_link.clone(), cx)
                })
                .into_any_element()
                .into(),
            " if you've created a repository recently.".into(),
        ])
        .justify_center()
        .into_any_element()
    };
    // at the top, the message sits where GHD's `.no-items` margin and the
    // `Ref` box's line put it
    div()
        .size_full()
        .flex()
        .justify_center()
        .map(|d| {
            if centred {
                d.items_center().p(SPACING_DOUBLE())
            } else {
                d.items_start().px(SPACING_DOUBLE()).pt(zpx(29.))
            }
        })
        .text_size(FONT_SIZE())
        .text_align(TextAlign::Center)
        .child(message)
        .into_any_element()
}

/// The `SectionFilterList` rows: group headers and repositories (icon,
/// highlighted `owner/name`, "Archived" badge); clicking one selects it.
/// `selected`: the clone URLs drawn selected (several with
/// `293-clone-multiple`).
pub fn repository_list(
    id: &'static str,
    rows: Rc<Vec<CloneRow>>,
    selected: Vec<String>,
    style: ListStyle,
    on_select: OnRepository,
) -> AnyElement {
    uniform_list(id, rows.len(), move |range, _window, cx| {
        // `277-consistent-filter-highlight`
        let bold_matches = corvene_core::AppState::try_global(cx).is_some_and(|s| {
            s.read(cx)
                .flags
                .bool(corvene_core::flags::ids::CONSISTENT_FILTER_HIGHLIGHT)
        });
        // `351-actions`: a row's menu opens its Actions without cloning
        let actions = corvene_core::AppState::try_global(cx)
            .is_some_and(|s| s.read(cx).flags.bool(corvene_core::flags::ids::ACTIONS));
        with_zoom(style.zoom, || {
            let t = cx.ghd();
            range
                .map(|ix| match &rows[ix] {
                    CloneRow::Header(title) => div()
                        .id(ix)
                        .w_full()
                        .h(LIST_ROW_HEIGHT())
                        .px(zpx(style.inset))
                        .flex()
                        .items_center()
                        .font_weight(FontWeight::SEMIBOLD)
                        .map(|d| {
                            if style.small_headers {
                                d.text_size(FONT_SIZE_SM()).text_color(t.text_secondary)
                            } else {
                                d.text_size(FONT_SIZE()).text_color(t.text)
                            }
                        })
                        .child(title.clone())
                        .into_any_element(),
                    CloneRow::Item(repo, positions) => {
                        let is_selected = selected.contains(&repo.clone_url);
                        let (selected_bg, selected_fg) = if style.focused {
                            (t.box_selected_active_background, t.box_selected_active_text)
                        } else {
                            (t.box_selected_background, t.box_selected_text)
                        };
                        let icon = if repo.private {
                            Octicon::Lock
                        } else if repo.fork {
                            Octicon::RepoForked
                        } else {
                            Octicon::Repo
                        };
                        let on_select = on_select.clone();
                        let repo_for_click = repo.clone();
                        let hover_bg = t.list_item_hover_background;
                        let text = repo.full_name();
                        div()
                            .id(ix)
                            .w_full()
                            .h(LIST_ROW_HEIGHT())
                            .px(zpx(style.inset))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(SPACING_HALF())
                            .cursor_pointer()
                            .text_size(FONT_SIZE())
                            .when(is_selected, |d| d.bg(selected_bg).text_color(selected_fg))
                            // the selection keeps its colour under the pointer
                            // (GHD's list has focus after the click)
                            .when(!is_selected, move |d| d.hover(move |s| s.bg(hover_bg)))
                            .on_click(move |ev: &ClickEvent, window, cx| {
                                on_select(&repo_for_click, ev.modifiers(), window, cx)
                            })
                            .when(actions, |d| {
                                let repo = repo.clone();
                                d.on_mouse_down(MouseButton::Right, move |ev, window, cx| {
                                    cx.stop_propagation();
                                    row_menu(&repo, ev.position, window, cx);
                                })
                            })
                            .child(octicon(
                                icon,
                                if is_selected { selected_fg } else { t.text },
                            ))
                            .child(div().flex_1().min_w_0().truncate().child(if bold_matches {
                                crate::autocompletion::highlighted(&text, positions)
                            } else {
                                highlighted(&text, positions)
                            }))
                            .when(repo.archived, |d| {
                                // `.archived` badge
                                d.child(
                                    div()
                                        .flex_none()
                                        .ml(SPACING_HALF())
                                        .px(zpx(3.))
                                        .py(zpx(1.))
                                        .rounded(BORDER_RADIUS())
                                        .border_1()
                                        .border_color(t.box_border_contrast)
                                        .text_size(FONT_SIZE_XS())
                                        .child("ARCHIVED"),
                                )
                            })
                            .into_any_element()
                    }
                })
                .collect()
        })
    })
    .size_full()
    .with_scrollbar()
    .into_any_element()
}

/// Corvene (`351-actions`): a repository row's context menu, View
/// Actions… (the repository need not be cloned) and View on GitHub. GHD's
/// clone list has no row menu.
fn row_menu(repo: &GitHubRepository, position: Point<Pixels>, window: &mut Window, cx: &mut App) {
    use crate::context_menu::{MenuItem, mac_or};
    let (actions, url) = (repo.clone(), repo.html_url.clone());
    let items = vec![
        MenuItem::new(
            mac_or("View Actions…", "View actions…"),
            move |_, cx| Dispatcher::show_actions(actions.clone(), None, None, cx),
        ),
        MenuItem::new(mac_or("View on GitHub", "View on GitHub"), move |_, cx| {
            Dispatcher::open_url(&url, cx)
        }),
    ];
    crate::native_menu::show_context_menu(items, position, window, cx);
}

/// `renderPostFilter`: the refresh button beside the filter box, dimmed
/// and inert while the list loads.
pub fn refresh_button(
    id: &'static str,
    account: &Account,
    loading: bool,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.ghd();
    let account = account.clone();
    button(id, "", cx)
        .icon_button_label("Refresh the list of repositories")
        .flex_none()
        .px(SPACING_HALF())
        .when(loading, |d| d.opacity(0.6))
        .on_click(move |_, _, cx| {
            if !loading {
                Dispatcher::load_api_repositories(account.clone(), cx)
            }
        })
        .child(octicon(Octicon::Sync, t.secondary_button_text))
}

/// `HighlightText`: the matched chars in a `<mark>`, which this list
/// leaves at Chromium's defaults (black on yellow).
pub fn highlighted(text: &str, positions: &[usize]) -> StyledText {
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
    let mark = HighlightStyle {
        color: Some(gpui_kit::black()),
        background_color: Some(rgb(0xffff00).into()),
        ..Default::default()
    };
    StyledText::new(text.to_string()).with_highlights(ranges.into_iter().map(|r| (r, mark)))
}

/// `AccountPicker` state a caller keeps.
pub struct AccountPickerState {
    pub open: bool,
    pub filter: Entity<InputState>,
    /// The button's bounds, the popover's anchor.
    pub button_bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl AccountPickerState {
    pub fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            open: false,
            filter: cx.new(|cx| InputState::new(window, cx).placeholder("Filter")),
            button_bounds: Rc::new(Cell::new(Bounds::default())),
        }
    }
}

/// `.account-picker-row`: "Account" over the `PopoverDropdown` button,
/// "@login - host".
pub fn account_picker(
    id: &'static str,
    account: &Account,
    picker: &AccountPickerState,
    on_toggle: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> Div {
    let t = cx.ghd();
    let bounds = picker.button_bounds.clone();
    let hover_bg = t.secondary_button_hover_background;
    div()
        .flex()
        .flex_col()
        .w_full()
        .child(
            div()
                .mb(SPACING_THIRD())
                .text_size(FONT_SIZE())
                .line_height(zpx(18.))
                .child("Account"),
        )
        .child(
            div()
                .id(id)
                .relative()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .gap(SPACING_HALF())
                .h(zpx(25.))
                .px(SPACING_HALF())
                .rounded(BORDER_RADIUS())
                .border_1()
                .border_color(t.secondary_button_border)
                .bg(t.secondary_button_background)
                .text_color(t.secondary_button_text)
                .text_size(FONT_SIZE())
                .cursor_pointer()
                .hover(move |s| s.bg(hover_bg))
                .on_click(move |_, window, cx| on_toggle(window, cx))
                .child(
                    canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                        .absolute()
                        .inset_0(),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .flex()
                        .flex_row()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(format!("@{}", account.login)),
                        )
                        .child(format!("\u{a0}-\u{a0}{}", account.friendly_endpoint())),
                )
                .child(octicon(Octicon::TriangleDown, t.secondary_button_text)),
        )
}

/// Where the popover goes relative to the picker button.
#[derive(Clone, Copy)]
pub struct PopoverPlacement {
    /// GHD positions the popover from the button's viewport rect as CSS px
    /// inside a `zoom`ed view, so under `#no-repositories { zoom }` both
    /// land `zoom` times further out; 1 elsewhere.
    pub scale: f32,
    /// CSS px between the button and the popover.
    pub gap: f32,
    /// 500 px tall however few accounts there are (else 200–500 px).
    pub fixed_height: bool,
    /// Placed and drawn as GHD's balloon (`crate::popover`), with `scale`
    /// as the page zoom; else at `gap` under the button without a tip.
    pub balloon: bool,
}

/// `.popover-dropdown-popover` with the account `SectionFilterList`, under
/// the picker button; `accounts` are the choices, `current` is ticked.
#[allow(clippy::too_many_arguments)]
pub fn account_popover(
    id_prefix: &'static str,
    picker: &AccountPickerState,
    placement: PopoverPlacement,
    accounts: Vec<Account>,
    current: Option<&Account>,
    on_pick: OnAccount,
    on_close: OnClose,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let t = cx.ghd();
    let anchor = picker.button_bounds.get();
    let viewport = window.viewport_size();
    let width = zpx(365.);
    let x = (anchor.origin.x * placement.scale)
        .min(viewport.width - width - zpx(8.))
        .max(zpx(8.));
    let y = (anchor.origin.y + anchor.size.height + zpx(placement.gap) / placement.scale)
        * placement.scale;
    let query = picker.filter.read(cx).value().trim().to_lowercase();
    let accounts: Vec<Account> = accounts
        .into_iter()
        .filter(|a| {
            query.is_empty()
                || corvene_core::filter::fuzzy_score(&query, &a.login).is_some()
                || corvene_core::filter::fuzzy_score(&query, &a.endpoint).is_some()
        })
        .collect();
    // the popover's filter has focus: the current account shows the
    // inactive selection
    let selected_bg = t.box_selected_background;
    let selected_text = t.box_selected_text;
    let hover_bg = t.list_item_hover_background;
    let id = |suffix: &str| SharedString::from(format!("{id_prefix}-{suffix}"));
    let close_overlay = on_close.clone();
    let body = div()
        .id(id("popover"))
        .w(width)
        .map(|d| {
            if placement.fixed_height {
                d.h(zpx(500.))
            } else {
                d.min_h(zpx(200.)).max_h(zpx(500.))
            }
        })
        .flex()
        .flex_col()
        // `.popover-component { background: var(--background-color) }`
        .bg(t.background)
        .text_color(t.text)
        .text_size(FONT_SIZE())
        .border_1()
        .border_color(t.box_border)
        .rounded(BORDER_RADIUS())
        .map(|d| {
            if placement.balloon {
                // `--base-box-shadow`
                d.shadow(vec![BoxShadow {
                    color: t.shadow,
                    offset: point(zpx(0.), zpx(2.)),
                    blur_radius: css_blur(7.),
                    spread_radius: zpx(0.),
                    inset: false,
                }])
            } else {
                d.shadow_lg()
            }
        })
        .overflow_hidden()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        // `.popover-dropdown-header`
        .child(
            div()
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(SPACING())
                .p(SPACING())
                .border_b_1()
                .border_color(t.box_border)
                .child(
                    div()
                        .flex_1()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Choose an account"),
                )
                .child(
                    div()
                        .id(id("close"))
                        .cursor_pointer()
                        .icon_button_label("Close")
                        .on_click(move |_, window, cx| on_close(window, cx))
                        .child(octicon(Octicon::X, t.text_secondary)),
                ),
        )
        .child(
            div()
                .flex_none()
                .mt(SPACING())
                .mx(SPACING())
                .mb(SPACING_HALF())
                .child(crate::widgets::filter_text_box(
                    id("filter"),
                    &picker.filter,
                    Some(octicon(Octicon::Search, t.text_secondary)),
                    window,
                    cx,
                )),
        )
        .child(
            div()
                .id(id("list"))
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .children(accounts.into_iter().map(|account| {
                    let is_selected = current.is_some_and(|c| {
                        c.endpoint == account.endpoint && c.login == account.login
                    });
                    let avatar = account
                        .avatar_url
                        .as_deref()
                        .and_then(|url| avatar_lookup_url(url, cx));
                    let (fg, secondary) = if is_selected {
                        (selected_text, selected_text)
                    } else {
                        (t.text, t.text_secondary)
                    };
                    let on_pick = on_pick.clone();
                    let picked = account.clone();
                    div()
                        .id(SharedString::from(format!(
                            "{id_prefix}-{}@{}",
                            account.login, account.endpoint
                        )))
                        .h(ACCOUNT_ROW_HEIGHT())
                        .flex_none()
                        .flex()
                        .flex_row()
                        .items_center()
                        .px(SPACING())
                        .cursor_pointer()
                        .text_color(fg)
                        .when(is_selected, |d| d.bg(selected_bg))
                        .when(!is_selected, move |d| d.hover(move |s| s.bg(hover_bg)))
                        .on_click(move |_, window, cx| on_pick(&picked, window, cx))
                        .child(avatar_image(avatar, zpx(32.), cx))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .mx(SPACING())
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .truncate()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(format!("@{}", account.login)),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .font_weight(FontWeight::LIGHT)
                                        .text_size(FONT_SIZE_SM())
                                        .text_color(secondary)
                                        .child(account.friendly_endpoint()),
                                ),
                        )
                }))
                .with_scrollbar(),
        );
    // a window-sized layer under the popover closes it on any click outside
    let layer = div()
        .id(id("layer"))
        .relative()
        .w(viewport.width)
        .h(viewport.height)
        .child(
            div()
                .id(id("overlay"))
                .absolute()
                .inset_0()
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    close_overlay(window, cx)
                }),
        );
    if placement.balloon {
        div()
            .child(
                deferred(anchored().position(point(zpx(0.), zpx(0.))).child(layer))
                    .with_priority(25),
            )
            .child(
                crate::popover::balloon_popover_zoomed(
                    anchor,
                    crate::popover::PopoverAnchorPosition::BottomLeft,
                    placement.scale,
                    body,
                    cx,
                )
                .with_priority(26),
            )
            .into_any_element()
    } else {
        deferred(
            anchored()
                .position(point(zpx(0.), zpx(0.)))
                .child(layer.child(div().absolute().left(x).top(y).child(body))),
        )
        .with_priority(25)
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(owner: &str, name: &str) -> GitHubRepository {
        GitHubRepository {
            endpoint: "https://api.github.com".into(),
            owner: owner.into(),
            name: name.into(),
            html_url: format!("https://github.com/{owner}/{name}"),
            clone_url: format!("https://github.com/{owner}/{name}.git"),
            default_branch: None,
            private: false,
            fork: false,
            parent: None,
            archived: false,
            permissions: None,
            allow_forking: None,
            node_id: None,
        }
    }

    fn labels(rows: &[CloneRow]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                CloneRow::Header(h) => format!("# {h}"),
                CloneRow::Item(r, _) => r.full_name(),
            })
            .collect()
    }

    #[::core::prelude::v1::test]
    fn hidden_owners_are_left_out_ignoring_case() {
        let repos = vec![
            repo("octocat", "a"),
            repo("BigCorp", "b"),
            repo("other", "c"),
        ];
        let kept = super::without_hidden_owners(&repos, &["bigcorp".to_string()]);
        let names: Vec<&str> = kept.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["a", "c"]);
    }

    #[::core::prelude::v1::test]
    fn groups_own_repositories_first_then_owners() {
        let repos = vec![
            repo("zed", "zed"),
            repo("Octocat", "spoon"),
            repo("desktop", "desktop"),
            repo("octocat", "git"),
            repo("octocat", "Hello"),
            repo("Desktop", "dugite"),
        ];
        let yours = format!("# {YOUR_REPOSITORIES}");
        assert_eq!(
            labels(&group_rows(&repos, "octocat", "")),
            [
                yours.as_str(),
                "octocat/Hello",
                "octocat/git",
                "Octocat/spoon",
                "# Desktop",
                "Desktop/dugite",
                "# desktop",
                "desktop/desktop",
                "# zed",
                "zed/zed"
            ]
        );
        assert_eq!(
            labels(&group_rows(&repos, "octocat", " DESK ")),
            [
                "# Desktop",
                "Desktop/dugite",
                "# desktop",
                "desktop/desktop"
            ]
        );
    }

    #[::core::prelude::v1::test]
    fn preferred_account_follows_the_login_list() {
        let account = |login: &str| Account {
            endpoint: "https://api.github.com".into(),
            id: 1,
            login: login.into(),
            name: None,
            avatar_url: None,
            emails: Vec::new(),
            scopes: Vec::new(),
            plan: None,
            private_primary_email: false,
            from_gh_cli: false,
        };
        let accounts = vec![account("first"), account("work"), account("other")];
        let login = |logins: &str| preferred_account(&accounts, logins).map(|a| a.login.as_str());
        assert_eq!(login(""), None);
        assert_eq!(login("Work"), Some("work"));
        assert_eq!(login("missing, other work"), Some("other"));
        assert_eq!(login("missing"), None);
    }

    #[::core::prelude::v1::test]
    fn urls_filter_as_owner_and_name() {
        for url in [
            "https://github.com/octocat/Hello",
            "https://github.com/octocat/Hello.git",
            "https://github.com/octocat/Hello/tree/main/src",
            "git@github.com:octocat/Hello.git",
            " https://github.com/octocat/Hello/ ",
        ] {
            assert_eq!(
                url_as_full_name(url).as_deref(),
                Some("octocat/Hello"),
                "{url}"
            );
        }
        assert_eq!(url_as_full_name("octocat/Hello"), None);
        assert_eq!(url_as_full_name("https://github.com/octocat"), None);
        assert_eq!(url_as_full_name("hello"), None);
    }

    #[::core::prelude::v1::test]
    fn filtering_moves_a_hidden_selection_to_the_first_match() {
        let repos = vec![repo("octocat", "Hello"), repo("desktop", "desktop")];
        let url = |r: &str| format!("https://github.com/{r}.git");
        let hello = url("octocat/Hello");
        let rows = group_rows(&repos, "octocat", "desk");
        let moved = filtered_selection(&rows, "desk", Some(&hello));
        assert_eq!(
            moved.map(|s| s.map(|r| r.full_name())),
            Some(Some("desktop/desktop".to_string()))
        );
        let rows = group_rows(&repos, "octocat", "");
        assert!(filtered_selection(&rows, "", Some(&hello)).is_none());
        let rows = group_rows(&repos, "octocat", "zzz");
        assert_eq!(
            filtered_selection(&rows, "zzz", Some(&hello)).map(|s| s.is_none()),
            Some(true)
        );
        assert!(filtered_selection(&rows, "zzz", None).is_none());
    }
}
