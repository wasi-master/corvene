//! Autocompletion popup: `:emoji:`, `#issue` and `@user` in the commit form,
//! branch names in Add Worktree (GHD
//! `ui/autocompletion/autocompleting-text-input.tsx`, the providers next to
//! it incl. `branch-autocompletion-provider.tsx`, and
//! `styles/ui/_autocompletion.scss`).
//!
//! The matching lives in `corvene_core::autocomplete` / `corvene_core::emoji`;
//! this module owns the popup state, its rendering and the wrap-around
//! keyboard selection (`ui/lib/list/selection.ts` `findNextSelectableRow`).

use std::ops::Range;
use std::rc::Rc;

use corvene_core::emoji::EmojiHit;
use corvene_core::{
    AppState, DEFAULT_MAX_HITS, Dispatcher, GitHubRepository, IssueHit, MentionableUser,
    TriggerKind, find_trigger, issues_matching, users_matching,
};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::scrollbar::{gutter, scrollbar};
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::{avatar_image, avatar_lookup_url};

/// GHD `RowHeight`.
#[allow(non_snake_case)]
fn ROW_HEIGHT() -> Pixels {
    zpx(29.)
}
/// GHD `DefaultPopupHeight`.
#[allow(non_snake_case)]
fn MAX_HEIGHT() -> Pixels {
    zpx(100.)
}

/// One row of the popup (`IEmojiHit` / `IIssueHit` / `UserHit`).
#[derive(Clone, Debug)]
pub enum Hit {
    Emoji(EmojiHit),
    Issue(IssueHit),
    User(MentionableUser),
    /// GHD `unknown-user`: a handle nobody in the mentionables matched
    /// (co-author input only; looked up on the API once added).
    UnknownUser(String),
    /// Corvene (`770-co-authors-from-history`): an author of a recent
    /// commit, offered as a co-author without a GitHub account.
    Author {
        name: String,
        email: String,
    },
    /// GHD `IBranchHit`: a branch name and its matched char positions.
    Branch {
        name: String,
        highlight: Vec<usize>,
    },
    /// `223-add-local-path-completion`: a folder completing a typed path.
    Folder {
        completion: String,
        name: String,
    },
}

impl Hit {
    /// GHD `getCompletionText`.
    pub fn completion_text(&self) -> String {
        match self {
            Hit::Emoji(e) => e.key.clone(),
            Hit::Issue(i) => format!("#{}", i.number),
            Hit::User(u) => format!("@{}", u.login),
            Hit::UnknownUser(name) => format!("@{name}"),
            Hit::Author { name, email } => format!("{name} <{email}>"),
            Hit::Branch { name, .. } => name.clone(),
            Hit::Folder { completion, .. } => completion.clone(),
        }
    }
}

/// GHD `CoAuthorAutocompletionProvider.getAutocompletionItems`: mentionable
/// users matching `filter` (minus the ones already added) plus the typed
/// handle as an unknown user when nothing matches it exactly.
pub fn co_author_hits(
    filter: &str,
    github: &GitHubRepository,
    exclude: &[String],
    cx: &mut App,
) -> Vec<Hit> {
    Dispatcher::refresh_mentionables(github, cx);
    let own = Dispatcher::own_login_for(github, cx);
    let Some(state) = AppState::try_global(cx) else {
        return Vec::new();
    };
    let s = state.read(cx);
    let key = corvene_core::autocomplete::cache_key(github);
    let users = s
        .mentionables
        .get(&key)
        .map(|c| c.users.as_slice())
        .unwrap_or(&[]);
    let mut hits: Vec<Hit> = users_matching(users, filter, own.as_deref(), DEFAULT_MAX_HITS)
        .into_iter()
        .filter(|u| !exclude.iter().any(|e| e.eq_ignore_ascii_case(&u.login)))
        .map(Hit::User)
        .collect();
    // Corvene (`770-co-authors-from-history`)
    let user_emails: Vec<String> = hits
        .iter()
        .filter_map(|h| match h {
            Hit::User(u) => u.email.clone(),
            _ => None,
        })
        .collect();
    let room = DEFAULT_MAX_HITS.saturating_sub(hits.len()).max(3);
    hits.extend(history_author_hits(filter, exclude, &user_emails, room, cx));
    if !filter.is_empty() {
        let exact = hits.iter().any(|h| match h {
            Hit::User(u) => u.login.eq_ignore_ascii_case(filter),
            _ => false,
        });
        if !exact {
            hits.push(Hit::UnknownUser(filter.to_string()));
        }
    }
    hits
}

/// Corvene (`770-co-authors-from-history`): authors of the selected
/// repository's recent commits matching `filter`, leaving out co-author ids
/// in `exclude` (free-form co-authors are their email), `also_exclude` and
/// the committer (`user.email`). Starts reading the authors the first time.
pub fn history_author_hits(
    filter: &str,
    exclude: &[String],
    also_exclude: &[String],
    max_hits: usize,
    cx: &mut App,
) -> Vec<Hit> {
    let Some(state) = AppState::try_global(cx) else {
        return Vec::new();
    };
    let (id, on) = {
        let s = state.read(cx);
        (
            s.selected,
            s.flags
                .bool(corvene_core::flags::ids::CO_AUTHORS_FROM_HISTORY),
        )
    };
    let Some(id) = id.filter(|_| on) else {
        return Vec::new();
    };
    Dispatcher::load_recent_authors(id, cx);
    let s = state.read(cx);
    let Some(rs) = s.repo_states.get(&id) else {
        return Vec::new();
    };
    let Some(authors) = rs.recent_authors.as_ref() else {
        return Vec::new();
    };
    let mut excluded: Vec<String> = exclude.iter().chain(also_exclude).cloned().collect();
    if let Some(own) = rs.info.as_ref().and_then(|i| i.identity.email.clone()) {
        excluded.push(own);
    }
    corvene_core::autocomplete::authors_matching(authors, filter, &excluded, max_hits)
        .into_iter()
        .map(|(name, email)| Hit::Author { name, email })
        .collect()
}

/// GHD `IAutocompletionState`.
pub struct Autocompletion {
    pub kind: TriggerKind,
    /// Byte range of the filter text; the trigger character sits just before it.
    pub range: Range<usize>,
    pub hits: Vec<Hit>,
    /// Nothing is selected until the user presses ↑/↓ (GHD `selectedItem: null`).
    pub selected: Option<usize>,
    pub scroll: UniformListScrollHandle,
}

impl Autocompletion {
    /// ↑/↓ with wrap-around (`findNextSelectableRow`, `wrap: true`).
    pub fn move_selection(&mut self, delta: i64) {
        let n = self.hits.len() as i64;
        if n == 0 {
            return;
        }
        let next = match self.selected {
            None if delta > 0 => 0,
            None => n - 1,
            Some(ix) => (ix as i64 + delta).rem_euclid(n),
        } as usize;
        self.selected = Some(next);
        self.scroll.scroll_to_item(next, ScrollStrategy::Nearest);
    }
}

/// GHD `attemptAutocompletion` with only the `BranchAutocompletionProvider`:
/// the whole of `text` filters `branches`. `None` when nothing matches.
pub fn attempt_branch(text: &str, branches: &[String]) -> Option<Autocompletion> {
    let hits: Vec<Hit> = corvene_core::filter::branch_matches(text, branches)
        .into_iter()
        .map(|(name, highlight)| Hit::Branch { name, highlight })
        .collect();
    if hits.is_empty() {
        return None;
    }
    Some(Autocompletion {
        kind: TriggerKind::Branch,
        range: 0..text.len(),
        hits,
        selected: None,
        scroll: UniformListScrollHandle::new(),
    })
}

/// `223-add-local-path-completion`: the whole of `text` is a path whose
/// last segment filters the folders next to it. `None` when nothing matches.
pub fn attempt_path(text: &str) -> Option<Autocompletion> {
    let hits: Vec<Hit> = corvene_core::folder_completions(text, DEFAULT_MAX_HITS)
        .into_iter()
        .map(|(completion, name)| Hit::Folder { completion, name })
        .collect();
    if hits.is_empty() {
        return None;
    }
    Some(Autocompletion {
        kind: TriggerKind::Path,
        range: 0..text.len(),
        hits,
        selected: None,
        scroll: UniformListScrollHandle::new(),
    })
}

/// GHD `attemptAutocompletion` over the commit-message providers. Issue and
/// user lookups also kick off the throttled cache refreshes.
pub fn attempt(
    text: &str,
    caret: usize,
    github: Option<&GitHubRepository>,
    cx: &mut App,
) -> Option<Autocompletion> {
    let trigger = find_trigger(text, caret)?;
    let hits: Vec<Hit> = match trigger.kind {
        TriggerKind::Emoji => corvene_core::emoji::matches(&trigger.text, DEFAULT_MAX_HITS)
            .into_iter()
            .map(Hit::Emoji)
            .collect(),
        TriggerKind::Issue => {
            let gh = github?;
            Dispatcher::refresh_issues(gh, cx);
            let state = AppState::try_global(cx)?;
            let s = state.read(cx);
            let key = corvene_core::autocomplete::cache_key(gh);
            let issues = s
                .issues
                .get(&key)
                .map(|c| c.issues.as_slice())
                .unwrap_or(&[]);
            issues_matching(issues, &trigger.text, DEFAULT_MAX_HITS)
                .into_iter()
                .map(Hit::Issue)
                .collect()
        }
        // never produced by `find_trigger` (see `attempt_branch`)
        TriggerKind::Branch | TriggerKind::Path => return None,
        TriggerKind::User => {
            let gh = github?;
            Dispatcher::refresh_mentionables(gh, cx);
            let own = Dispatcher::own_login_for(gh, cx);
            let state = AppState::try_global(cx)?;
            let s = state.read(cx);
            let key = corvene_core::autocomplete::cache_key(gh);
            let users = s
                .mentionables
                .get(&key)
                .map(|c| c.users.as_slice())
                .unwrap_or(&[]);
            users_matching(users, &trigger.text, own.as_deref(), DEFAULT_MAX_HITS)
                .into_iter()
                .map(Hit::User)
                .collect()
        }
    };
    if hits.is_empty() {
        return None;
    }
    Some(Autocompletion {
        kind: trigger.kind,
        range: trigger.range,
        hits,
        selected: None,
        scroll: UniformListScrollHandle::new(),
    })
}

pub type PickHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// GHD `renderAutocompletions`: a `Popover` anchored at the caret's
/// bottom-left, `RowHeight` rows, at most `DefaultPopupHeight` tall.
pub fn popup(
    ac: &Autocompletion,
    anchor: Point<Pixels>,
    on_pick: PickHandler,
    cx: &App,
) -> AnyElement {
    popup_with_priority(ac, anchor, on_pick, 3, cx)
}

/// [`popup`] inside a dialog: drawn above the dialog's deferred layer
/// (`dialog::dialog` uses priority 20).
pub fn dialog_popup(
    ac: &Autocompletion,
    anchor: Point<Pixels>,
    on_pick: PickHandler,
    cx: &App,
) -> AnyElement {
    popup_with_priority(ac, anchor, on_pick, 30, cx)
}

fn popup_with_priority(
    ac: &Autocompletion,
    anchor: Point<Pixels>,
    on_pick: PickHandler,
    priority: usize,
    cx: &App,
) -> AnyElement {
    let t = cx.ghd();
    // `.autocompletion-popup` widths per provider kind
    let width = match ac.kind {
        TriggerKind::Emoji => zpx(200.),
        TriggerKind::User => zpx(220.),
        TriggerKind::Issue => zpx(300.),
        // `.autocompletion-popup` default
        TriggerKind::Branch | TriggerKind::Path => zpx(250.),
    };
    let n = ac.hits.len();
    let height = (ROW_HEIGHT() * n as f32).min(MAX_HEIGHT());
    let hits = Rc::new(ac.hits.clone());
    let selected = ac.selected;
    deferred(
        anchored()
            .position(anchor)
            .snap_to_window_with_margin(zpx(8.))
            .child(
                div()
                    .id("autocompletion-popup")
                    .occlude()
                    .child(crate::widgets::touch_drag_occluder())
                    .w(width)
                    .h(height)
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .rounded(BORDER_RADIUS())
                    .bg(t.background)
                    .shadow(vec![BoxShadow {
                        color: hsla(0., 0., 0., 0.3),
                        offset: point(zpx(0.), zpx(0.)),
                        blur_radius: css_blur(8.),
                        spread_radius: zpx(0.),
                        inset: false,
                    }])
                    .child(
                        uniform_list("autocompletion-rows", n, move |range, _window, cx| {
                            range
                                .map(|ix| {
                                    row(ix, &hits[ix], selected == Some(ix), on_pick.clone(), cx)
                                })
                                .collect::<Vec<_>>()
                        })
                        .track_scroll(&ac.scroll)
                        .size_full()
                        .pr(gutter(&ac.scroll)),
                    )
                    .child(scrollbar("autocompletion-scrollbar", ac.scroll.clone())),
            ),
    )
    .with_priority(priority)
    .into_any_element()
}

/// `.autocompletion-item` + the provider's `renderItem`.
fn row(ix: usize, hit: &Hit, selected: bool, on_pick: PickHandler, cx: &mut App) -> AnyElement {
    let t = cx.ghd();
    let (fg, secondary) = if selected {
        (t.box_selected_active_text, t.box_selected_active_text)
    } else {
        (t.text, t.text_secondary)
    };
    let mut d = div()
        .id(("autocompletion-row", ix))
        .h(ROW_HEIGHT())
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .px(SPACING())
        .text_size(FONT_SIZE())
        .text_color(fg)
        .cursor_pointer()
        .when(ix > 0, |d| {
            d.border_t_1().border_color(if selected {
                t.box_selected_active_background
            } else {
                t.box_border
            })
        })
        .when(selected, |d| d.bg(t.box_selected_active_background))
        // GHD's list selects on mouse down and inserts on click; inserting on
        // mouse down keeps the popup alive across the field's blur.
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            on_pick(ix, window, cx)
        });
    d = match hit {
        Hit::Emoji(e) => {
            // `.emoji`: 20 px icon cell at 15 px, then the alias with the
            // matched substring in bold (`<mark>`), colons stripped.
            let alias = e.key.trim_matches(':').to_string();
            let title: AnyElement = if e.match_length == 0 {
                div().truncate().child(alias).into_any_element()
            } else {
                let start = e.match_start.saturating_sub(1).min(alias.len());
                let end = (start + e.match_length).min(alias.len());
                div()
                    .flex()
                    .flex_row()
                    .overflow_hidden()
                    .child(alias[..start].to_string())
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .child(alias[start..end].to_string()),
                    )
                    .child(alias[end..].to_string())
                    .into_any_element()
            };
            let icon: AnyElement = match &e.image {
                // GitHub's image-only emoji (`:shipit:`) from the cache
                Some(path) => img(path.clone())
                    .size(zpx(20.))
                    .object_fit(ObjectFit::Contain)
                    .into_any_element(),
                None => div()
                    .text_size(zpx(15.))
                    .child(e.emoji.clone())
                    .into_any_element(),
            };
            d.child(
                div()
                    .size(zpx(20.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .mr(SPACING_HALF())
                    .child(icon),
            )
            .child(div().flex_1().min_w_0().overflow_hidden().child(title))
        }
        Hit::UnknownUser(name) => d
            // `.user.unknown`
            .child(
                div()
                    .flex_none()
                    .max_w_full()
                    .truncate()
                    .font_weight(FontWeight::SEMIBOLD)
                    .italic()
                    // GHD 3.6.8 (desktop/desktop#21820): room for the italic
                    // overhang the clip would cut
                    .pr(zpx(2.))
                    .mr(SPACING_HALF())
                    .child(format!("@{name}")),
            )
            .child(
                // GHD 3.6.8: `white-space: nowrap` without the ellipsis, so
                // the description is not clipped mid-glyph
                div()
                    .flex_1()
                    .min_w_0()
                    .whitespace_nowrap()
                    .italic()
                    .text_size(FONT_SIZE_SM())
                    .text_color(secondary)
                    .child("Search for user"),
            ),
        Hit::Author { name, email } => {
            Dispatcher::request_avatar_for_email(email, cx);
            let avatar = crate::widgets::avatar_lookup(email, cx);
            d.child(
                div()
                    .mr(SPACING_HALF())
                    .child(avatar_image(avatar, zpx(16.), cx)),
            )
            .child(
                div()
                    .flex_none()
                    .max_w_full()
                    .truncate()
                    .font_weight(FontWeight::SEMIBOLD)
                    .mr(SPACING_HALF())
                    .child(name.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(secondary)
                    .child(email.clone()),
            )
        }
        Hit::Branch { name, highlight } => d
            // `.branch`: git-branch octicon, then the name with `<mark>` hits
            .child(
                crate::icons::octicon(crate::icons::Octicon::GitBranch, fg)
                    .flex_none()
                    .mr(SPACING_HALF()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(highlighted(name, highlight)),
            ),
        Hit::Folder { name, .. } => d
            .child(
                crate::icons::octicon(crate::icons::Octicon::FileDirectory, fg)
                    .flex_none()
                    .mr(SPACING_HALF()),
            )
            .child(div().flex_1().min_w_0().truncate().child(name.clone())),
        Hit::Issue(i) => d
            .gap(zpx(4.))
            .child(
                div()
                    .flex_none()
                    .text_color(secondary)
                    .child(format!("#{}", i.number)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(i.title.clone()),
            ),
        Hit::User(u) => {
            let avatar = u.avatar_url.as_deref().and_then(|url| {
                Dispatcher::request_avatar_url(url, cx);
                avatar_lookup_url(url, cx)
            });
            d.child(
                div()
                    .mr(SPACING_HALF())
                    .child(avatar_image(avatar, zpx(16.), cx)),
            )
            .child(
                div()
                    .flex_none()
                    .max_w_full()
                    .truncate()
                    .font_weight(FontWeight::SEMIBOLD)
                    .mr(SPACING_HALF())
                    .child(u.login.clone()),
            )
            .when_some(u.name.clone(), |d, name| {
                d.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(secondary)
                        .child(name),
                )
            })
        }
    };
    d.into_any_element()
}

/// GHD `HighlightText`: the chars at `positions` in bold.
pub(crate) fn highlighted(text: &str, positions: &[usize]) -> StyledText {
    let mut ranges: Vec<Range<usize>> = Vec::new();
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
    let bold = HighlightStyle {
        font_weight: Some(FontWeight::BOLD),
        ..Default::default()
    };
    StyledText::new(text.to_string()).with_highlights(ranges.into_iter().map(|r| (r, bold)))
}
