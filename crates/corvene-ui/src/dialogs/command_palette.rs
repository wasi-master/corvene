//! Corvene `624-command-palette` (desktop/desktop#22832): one box that runs
//! any menu item and switches repository or branch. GitHub Desktop has none;
//! its menus and foldouts each have their own filter.
//!
//! The actions come from the menu template (`app_menu::palette_commands`),
//! so there is no second list. What is enabled is decided before the popup
//! opens, because `menu_state` disables nearly everything under a popup.

use std::cell::RefCell;

use corvene_core::filter::fuzzy_score;
use corvene_core::{AppState, Dispatcher};
use gpui_kit::component::input::InputState;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::dialog::dialog;
use crate::filter_list::scroll_into_view;
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::text_box;

/// How many rows are drawn; the filter narrows the rest.
const MAX_ROWS: usize = 200;
const ROW_HEIGHT: f32 = 30.;
/// Most recent first; this session only.
const RECENT_LIMIT: usize = 20;

thread_local! {
    static RECENT: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static SEED: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Action,
    Repository,
    Branch,
}

enum Run {
    Action(Box<dyn Action>),
    Repository(u64),
    Branch { repo: u64, name: String },
}

struct Entry {
    source: Source,
    label: String,
    /// The menu it is in, or what kind of thing it is.
    detail: String,
    shortcut: Option<String>,
    run: Run,
}

impl Entry {
    fn key(&self) -> String {
        format!("{:?}:{}:{}", self.source, self.detail, self.label)
    }
}

/// What a query asks for: `>` actions, `@` branches, `#` repositories.
fn parse_query(query: &str) -> (Option<Source>, &str) {
    let query = query.trim_start();
    match query.chars().next() {
        Some('>') => (Some(Source::Action), query[1..].trim_start()),
        Some('@') => (Some(Source::Branch), query[1..].trim_start()),
        Some('#') => (Some(Source::Repository), query[1..].trim_start()),
        _ => (None, query),
    }
}

/// The rank of `key` among the recently used, if it is one.
fn recent_rank(key: &str, recent: &[String]) -> Option<usize> {
    recent.iter().position(|k| k == key)
}

/// Indices into `entries`, best first: recently used, then by score; a tie
/// keeps the menu order.
fn matches(entries: &[(Source, String, String)], query: &str, recent: &[String]) -> Vec<usize> {
    let (source, text) = parse_query(query);
    let mut hits: Vec<(usize, f32, usize)> = entries
        .iter()
        .enumerate()
        .filter(|(_, (kind, ..))| source.is_none_or(|s| s == *kind))
        // an empty box lists what the keyboard most often wants: actions
        .filter(|(_, (kind, ..))| source.is_some() || !text.is_empty() || *kind == Source::Action)
        .filter_map(|(ix, (_, label, key))| {
            let score = fuzzy_score(text, label)?;
            Some((ix, score, recent_rank(key, recent).unwrap_or(usize::MAX)))
        })
        .collect();
    hits.sort_by(|a, b| {
        a.2.cmp(&b.2)
            .then(b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal))
            .then(a.0.cmp(&b.0))
    });
    hits.into_iter().map(|(ix, ..)| ix).collect()
}

fn remember(key: String) {
    RECENT.with(|recent| {
        let mut recent = recent.borrow_mut();
        recent.retain(|k| *k != key);
        recent.insert(0, key);
        recent.truncate(RECENT_LIMIT);
    });
}

#[cfg(target_os = "macos")]
fn shortcut_text(keystroke: &Keystroke) -> String {
    keystroke.to_string()
}

#[cfg(not(target_os = "macos"))]
fn shortcut_text(keystroke: &Keystroke) -> String {
    crate::views_menu::accelerator_text(keystroke)
}

/// Collect what can be run now and open the palette.
pub fn open(window: &Window, cx: &mut App) {
    let commands = crate::app_menu::palette_commands(AppState::global(cx).read(cx));
    let mut entries = Vec::with_capacity(commands.len());
    for c in commands {
        if !window.is_action_available(&*c.action, cx) && !cx.is_action_available(&*c.action) {
            continue;
        }
        entries.push(Entry {
            source: Source::Action,
            shortcut: window
                .bindings_for_action(&*c.action)
                .last()
                .and_then(|binding| binding.keystrokes().first().cloned())
                .map(|k| shortcut_text(k.inner())),
            label: c.label,
            detail: c.menu,
            run: Run::Action(c.action),
        });
    }
    let state = AppState::global(cx).read(cx);
    let selected = state.selected;
    entries.extend(state.repositories.iter().map(|r| Entry {
        source: Source::Repository,
        label: r.name(),
        detail: "Repository".to_string(),
        shortcut: None,
        run: Run::Repository(r.id),
    }));
    if let Some((id, info)) =
        selected.and_then(|id| Some((id, state.repo_states.get(&id)?.info.as_ref()?)))
    {
        entries.extend(info.branches.iter().map(|b| Entry {
            source: Source::Branch,
            label: b.name.clone(),
            detail: "Branch".to_string(),
            shortcut: None,
            run: Run::Branch {
                repo: id,
                name: b.name.clone(),
            },
        }));
    }
    SEED.with(|seed| *seed.borrow_mut() = entries);
    Dispatcher::show_popup(corvene_core::Popup::CommandPalette, cx);
}

pub struct CommandPaletteDialog {
    entries: Vec<Entry>,
    /// Source, label and key of each entry, for [`matches`].
    keys: Vec<(Source, String, String)>,
    filter: Entity<InputState>,
    selected: usize,
    scroll: ScrollHandle,
}

impl CommandPaletteDialog {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let entries = SEED.with(|seed| std::mem::take(&mut *seed.borrow_mut()));
        let keys = entries
            .iter()
            .map(|e| (e.source, e.label.clone(), e.key()))
            .collect();
        let filter = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Run a command  > actions  @ branches  # repositories")
        });
        cx.observe(&filter, |this, _, cx| {
            this.selected = 0;
            cx.notify();
        })
        .detach();
        let handle = filter.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        Self {
            entries,
            keys,
            filter,
            selected: 0,
            scroll: ScrollHandle::new(),
        }
    }

    fn visible(&self, cx: &App) -> Vec<usize> {
        let query = self.filter.read(cx).value().to_string();
        let mut hits = RECENT.with(|recent| matches(&self.keys, &query, &recent.borrow()));
        hits.truncate(MAX_ROWS);
        hits
    }

    fn run(&mut self, entry: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.entries.get(entry) else {
            return;
        };
        remember(entry.key());
        Dispatcher::close_popup(cx);
        match &entry.run {
            // menu actions do nothing under a popup: it is closed above
            Run::Action(action) => window.dispatch_action(action.boxed_clone(), cx),
            Run::Repository(id) => Dispatcher::select_repository(*id, cx),
            Run::Branch { repo, name } => {
                Dispatcher::checkout_branch(*repo, name.clone(), None, cx)
            }
        }
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let hits = self.visible(cx);
        match ev.keystroke.key.as_str() {
            "down" | "up" if !hits.is_empty() => {
                let delta = if ev.keystroke.key == "down" { 1 } else { -1 };
                self.selected = crate::filter_list::wrap_step(self.selected, delta, hits.len());
                scroll_into_view(
                    &self.scroll,
                    zpx(ROW_HEIGHT) * self.selected as f32,
                    zpx(ROW_HEIGHT),
                );
            }
            "enter" => {
                if let Some(&entry) = hits.get(self.selected) {
                    self.run(entry, window, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for CommandPaletteDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd();
        let hits = self.visible(cx);
        self.selected = self.selected.min(hits.len().saturating_sub(1));
        let selected = self.selected;
        let rows = hits.iter().enumerate().map(|(row, &ix)| {
            let entry = &self.entries[ix];
            let hot = row == selected;
            let (text, minor) = if hot {
                (t.box_selected_active_text, t.box_selected_active_text)
            } else {
                (t.text, t.text_secondary)
            };
            div()
                .id(("palette-row", row))
                .flex_none()
                .h(zpx(ROW_HEIGHT))
                .px(SPACING())
                .flex()
                .flex_row()
                .items_center()
                .gap(SPACING())
                .cursor_pointer()
                .role(Role::ListItem)
                .aria_label(SharedString::from(entry.label.clone()))
                .aria_selected(hot)
                .when(hot, |d| d.bg(t.box_selected_active_background))
                .text_color(text)
                .on_click(cx.listener(move |this, _, window, cx| this.run(ix, window, cx)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(entry.label.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(FONT_SIZE_SM())
                        .text_color(minor)
                        .child(entry.detail.clone()),
                )
                .children(entry.shortcut.clone().map(|keys| {
                    div()
                        .flex_none()
                        .text_size(FONT_SIZE_SM())
                        .text_color(minor)
                        .child(keys)
                }))
                .into_any_element()
        });
        let list = div()
            .id("palette-list")
            .h(zpx(ROW_HEIGHT * 12.))
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .role(Role::List)
            .aria_label("Commands")
            .children(rows)
            .when(hits.is_empty(), |d| {
                d.child(
                    div()
                        .p(SPACING())
                        .text_color(t.text_secondary)
                        .child("Nothing matches."),
                )
            });
        let content = div()
            .w(crate::theme::fit_width(520.))
            .flex()
            .flex_col()
            .gap(SPACING())
            .capture_key_down(cx.listener(Self::on_key))
            .child(text_box("palette-filter", &self.filter, None, window, cx))
            .child(list);
        dialog(
            "command-palette",
            "Command Palette",
            content,
            Vec::new(),
            |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx),
            window,
            cx,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // gpui's prelude has a `test` macro of its own
    use core::prelude::v1::test;

    fn entries() -> Vec<(Source, String, String)> {
        [
            (Source::Action, "Pull", "a:pull"),
            (Source::Action, "Push", "a:push"),
            (Source::Action, "Fetch", "a:fetch"),
            (Source::Branch, "push-fix", "b:push-fix"),
            (Source::Repository, "pulsar", "r:pulsar"),
        ]
        .into_iter()
        .map(|(source, label, key)| (source, label.to_string(), key.to_string()))
        .collect()
    }

    #[test]
    fn prefixes_narrow_the_source() {
        assert_eq!(parse_query("> pu"), (Some(Source::Action), "pu"));
        assert_eq!(parse_query("@fix"), (Some(Source::Branch), "fix"));
        assert_eq!(parse_query("#x"), (Some(Source::Repository), "x"));
        assert_eq!(parse_query(" pu"), (None, "pu"));
        let e = entries();
        assert_eq!(matches(&e, "@", &[]), vec![3]);
        assert_eq!(matches(&e, "#pu", &[]), vec![4]);
        assert_eq!(matches(&e, ">fe", &[]), vec![2]);
    }

    #[test]
    fn an_empty_box_lists_actions_in_menu_order() {
        assert_eq!(matches(&entries(), "", &[]), vec![0, 1, 2]);
    }

    #[test]
    fn typing_searches_every_source() {
        let hits = matches(&entries(), "pu", &[]);
        assert_eq!(hits.len(), 4);
        assert!(hits.contains(&3) && hits.contains(&4));
        assert!(!hits.contains(&2));
    }

    #[test]
    fn recently_used_rank_first() {
        let recent = vec!["a:fetch".to_string(), "a:push".to_string()];
        assert_eq!(matches(&entries(), "", &recent), vec![2, 1, 0]);
    }
}
