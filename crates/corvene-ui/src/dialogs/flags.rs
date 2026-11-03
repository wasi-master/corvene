//! Flags - a Corvene addition with no GitHub Desktop counterpart: a
//! chrome://flags-style dialog over `corvene_core::flags`. Every switchable
//! deviation from GHD is a row with a toggle, select, number or text
//! control; values apply at once (no Save), restart-required flags show a
//! Relaunch bar, presets are the base layer and overrides sit on top.
//! Opened from Corvene › Flags… (⌘⇧,), `CORVENE_POPUP=flags[:query]` and
//! `x-corvene://flags?q=`. Snapshot-verified only: the parity harness has
//! nothing to compare it with.
//!
//! Bug-fix flags (`Nature::BugFix`) are hidden from the list, the search and
//! every count unless "Show bug fixes" is ticked; the All / On / Off switch
//! narrows the list and the counts to flags that deviate from GitHub Desktop
//! ("on": any value but the GitHub Desktop preset's) or do not. Both last
//! for the dialog session and are display-only: presets, Reset all and
//! `CORVENE_FLAGS` still cover every flag.
//!
//! The Presets page lists the presets with their details and how many flags
//! each switches on. Reset all, and picking a preset while flags are
//! modified, ask first in a sheet over the dialog.
//!
//! The list is virtualized (`gpui::list`): only the rows on screen are built
//! each frame, so wheel scrolling stays at frame rate with hundreds of flags.
//!
//! ↑ / ↓ (from the search box too) move a keyboard cursor through the flags,
//! Enter in the search box switches its toggle or puts the caret in its
//! number or text field. A
//! category header with modified flags offers "Reset to preset" for them.

use std::collections::HashMap;
use std::rc::Rc;

use corvene_core::flags::{
    self, Category, FlagDef, FlagId, FlagOverrides, Kind, Nature, Preset, REGISTRY, Value,
};
use corvene_core::{AppState, Dispatcher};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::actions::{CloseFoldout, Find, SelectNextFile, SelectPreviousFile};
use crate::autocompletion::highlighted;
use crate::context_menu::MenuItem;
use crate::dialog::window_title;
use crate::icons::{Octicon, octicon};
use crate::scrollbar::{ScrollbarExt, gutter, scrollbar};
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, mono_font};
use crate::widgets::{
    GhdTooltip, IconButtonA11y, ListRowA11y, SelectHandler, button, checkbox_row, counter,
    dialog_error_banner, filter_text_box, link_button, pill, primary_button, radio, select_button,
    switch, text_box_opts,
};

/// The left navigation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Nav {
    All,
    Modified,
    Presets,
    Category(Category),
    Unavailable,
}

/// The All / On / Off switch in the toolbar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StateFilter {
    All,
    /// Flags whose value deviates from GitHub Desktop.
    On,
    /// Flags at GitHub Desktop's behaviour.
    Off,
}

impl StateFilter {
    const ALL: [StateFilter; 3] = [StateFilter::All, StateFilter::On, StateFilter::Off];

    fn label(self) -> &'static str {
        match self {
            StateFilter::All => "All",
            StateFilter::On => "On",
            StateFilter::Off => "Off",
        }
    }

    fn tooltip(self) -> &'static str {
        match self {
            StateFilter::All => "Show every flag",
            StateFilter::On => "Show the flags that differ from GitHub Desktop",
            StateFilter::Off => "Show the flags at GitHub Desktop's behaviour",
        }
    }
}

/// What the confirmation sheet asks about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Confirm {
    ResetAll,
    Preset(Preset),
}

/// Where the search matched, as char positions for `highlighted`.
#[derive(Clone, Debug, Default)]
struct Hit {
    title: Vec<usize>,
    ident: Vec<usize>,
    summary: Vec<usize>,
    /// The query is an upstream issue number of this flag.
    issue: bool,
}

/// One entry of the virtualized list.
#[derive(Clone, Debug)]
enum Item {
    /// A category heading with the number of rows under it.
    Header(Category, usize),
    Flag(&'static FlagDef, Hit),
}

impl Item {
    /// Identity for deciding whether the list has to be reset.
    fn key(&self) -> u32 {
        match self {
            Item::Header(category, _) => 0x1_0000 | u32::from(category.block()),
            Item::Flag(def, _) => u32::from(def.id.0),
        }
    }
}

/// Char positions of `needle` (already lowercased) inside `hay`.
fn substring_positions(hay: &str, needle: &str) -> Option<Vec<usize>> {
    let lower = hay.to_lowercase();
    let start = lower.find(needle)?;
    let first = lower[..start].chars().count();
    Some((first..first + needle.chars().count()).collect())
}

/// Search over the title, `id-slug`, summary and upstream numbers. `q` is
/// the trimmed, lowercased query.
fn matches(def: &FlagDef, q: &str) -> Option<Hit> {
    if q.is_empty() {
        return Some(Hit::default());
    }
    let mut hit = Hit::default();
    let mut any = false;
    if let Some(p) = substring_positions(def.title, q) {
        hit.title = p;
        any = true;
    }
    if let Some(p) = substring_positions(&def.ident(), q) {
        hit.ident = p;
        any = true;
    }
    if let Some(p) = substring_positions(def.summary, q) {
        hit.summary = p;
        any = true;
    }
    let digits = q.trim_start_matches('#');
    if !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && def
            .upstream
            .iter()
            .any(|u| u.number.to_string().contains(digits))
    {
        hit.issue = true;
        any = true;
    }
    any.then_some(hit)
}

/// What a Number / Text input shows for a value.
fn display(value: &Value) -> String {
    match value {
        Value::Bool(_) | Value::Number(_) => value.render(),
        Value::Text(s) => s.to_string(),
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn category_icon(category: Category) -> Octicon {
    match category {
        Category::Appearance => Octicon::Paintbrush,
        Category::Repository => Octicon::Repo,
        Category::GitHub => Octicon::GitPullRequest,
        Category::WindowAndMenus => Octicon::DeviceDesktop,
        Category::SettingsAndUpdates => Octicon::Gear,
        Category::Accessibility => Octicon::Accessibility,
        Category::ChangesAndDiffs => Octicon::FileDiff,
        Category::HistoryAndBranches => Octicon::History,
        Category::Performance => Octicon::Zap,
        Category::Experimental => Octicon::Telescope,
    }
}

pub struct FlagsDialog {
    state: Entity<AppState>,
    /// Routes ⌘F / Esc through the dialog while nothing inside has focus.
    focus_handle: FocusHandle,
    search: Entity<InputState>,
    /// One input per Number / Text flag, created up front (never in render).
    inputs: HashMap<u16, Entity<InputState>>,
    /// The value each input was last written from; a core value that differs
    /// (preset, reset, import, env) rewrites the input on the next observe.
    synced: HashMap<u16, Value>,
    /// Inputs whose text is not committable, with the message under them.
    errors: HashMap<u16, String>,
    nav: Nav,
    /// "Show bug fixes": list and count `Nature::BugFix` flags.
    show_bug_fixes: bool,
    state_filter: StateFilter,
    /// The open confirmation sheet.
    confirm: Option<Confirm>,
    /// "Paste JSON" outcome, shown under the toolbar.
    import_error: Option<String>,
    import_note: Option<String>,
    /// The virtualized flag list and the rows it currently holds.
    list: ListState,
    items: Rc<Vec<Item>>,
    item_keys: Vec<u32>,
    view_key: ViewKey,
    /// The stored layer the rows were measured with; any change (preset,
    /// reset, a toggle that adds a Reset link) remeasures them.
    measured_overrides: FlagOverrides,
    /// The zoom factor the rows were measured at.
    zoom_seen: f32,
    /// The flag ↑ / ↓ last moved to.
    cursor: Option<FlagId>,
}

impl FlagsDialog {
    pub fn new(
        state: Entity<AppState>,
        query: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search flags")
                .default_value(query.unwrap_or_default())
        });
        cx.observe(&search, |_, _, cx| cx.notify()).detach();
        // Enter in the search box: the cursor's flag (`activate_cursor`)
        cx.subscribe_in(&search, window, |this, _, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter {
                secondary: false,
                shift: false,
            } = ev
                && this.confirm.is_none()
            {
                this.activate_cursor(window, cx);
            }
        })
        .detach();

        let (editable, overrides): (Vec<(&'static FlagDef, Value)>, FlagOverrides) = {
            let s = state.read(cx);
            (
                REGISTRY
                    .iter()
                    .filter(|def| matches!(def.kind, Kind::Number { .. } | Kind::Text { .. }))
                    .map(|def| (def, s.flags.value(def.id).clone()))
                    .collect(),
                s.flag_overrides.clone(),
            )
        };
        let mut inputs = HashMap::new();
        let mut synced = HashMap::new();
        let errors = HashMap::new();
        for (def, value) in editable {
            let placeholder = match def.kind {
                Kind::Text { placeholder, .. } => placeholder,
                _ => "",
            };
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .default_value(display(&value))
            });
            let id = def.id.0;
            cx.subscribe(&input, move |this, _, ev: &InputEvent, cx| {
                this.on_input(id, ev, cx)
            })
            .detach();
            inputs.insert(id, input);
            synced.insert(id, value);
        }
        crate::windows::observe_state_in(&state, window, cx, |this, _, window, cx| {
            this.resync(window, cx);
            cx.notify();
        })
        .detach();

        let handle = search.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        Self {
            state,
            focus_handle,
            search,
            inputs,
            synced,
            errors,
            nav: Nav::All,
            show_bug_fixes: false,
            state_filter: StateFilter::All,
            confirm: None,
            import_error: None,
            import_note: None,
            // every row is measured on the first layout so the scrollbar
            // reflects the whole list
            list: ListState::new(0, ListAlignment::Top, zpx(400.)).measure_all(),
            items: Rc::new(Vec::new()),
            item_keys: Vec::new(),
            view_key: ViewKey::default(),
            measured_overrides: overrides,
            zoom_seen: crate::theme::sizes::zoom_factor(),
            cursor: None,
        }
    }

    /// ↑ / ↓: the keyboard cursor to the next or previous listed flag
    /// (from nothing, the first or last), scrolled into view.
    fn move_cursor(&mut self, delta: isize, cx: &mut Context<Self>) {
        let flags: Vec<(usize, FlagId)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(ix, item)| match item {
                Item::Flag(def, _) => Some((ix, def.id)),
                Item::Header(..) => None,
            })
            .collect();
        let current = self
            .cursor
            .and_then(|id| flags.iter().position(|(_, f)| *f == id));
        let Some(next) = crate::filter_list::step(current, delta, flags.len()) else {
            return;
        };
        let (ix, id) = flags[next];
        self.cursor = Some(id);
        self.list.scroll_to_reveal_item(ix);
        self.remeasure_flag(id.0);
        if let Some(old) = current.map(|c| flags[c].1) {
            self.remeasure_flag(old.0);
        }
        cx.notify();
    }

    /// Enter on the cursor's flag: a toggle flips, a number or text field
    /// takes the caret.
    fn activate_cursor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self
            .cursor
            .filter(|id| self.items.iter().any(|i| i.key() == u32::from(id.0)))
        else {
            return;
        };
        match flags::def(id).kind {
            Kind::Bool => {
                let on = matches!(self.state.read(cx).flags.value(id), Value::Bool(true));
                let _ = Dispatcher::set_flag(id, Value::Bool(!on), cx);
            }
            Kind::Number { .. } | Kind::Text { .. } => {
                if let Some(input) = self.inputs.get(&id.0) {
                    let handle = input.read(cx).focus_handle(cx);
                    window.focus(&handle, cx);
                }
            }
            Kind::Select { .. } => {}
        }
        cx.notify();
    }

    /// A Number / Text input changed: validate, and commit when it fits.
    fn on_input(&mut self, id: u16, ev: &InputEvent, cx: &mut Context<Self>) {
        if !matches!(ev, InputEvent::Change) {
            return;
        }
        let Some(input) = self.inputs.get(&id) else {
            return;
        };
        let text = input.read(cx).value().to_string();
        let def = flags::def(FlagId(id));
        let had_error = self.errors.contains_key(&id);
        match def.kind.parse(&text) {
            Ok(value) => {
                self.errors.remove(&id);
                if self.synced.get(&id) != Some(&value) {
                    self.synced.insert(id, value.clone());
                    if let Err(err) = Dispatcher::set_flag(FlagId(id), value, cx) {
                        self.errors.insert(id, err);
                    }
                }
            }
            Err(err) => {
                self.errors.insert(id, err);
            }
        }
        if had_error != self.errors.contains_key(&id) {
            // the message under the input changes the row's height
            self.remeasure_flag(id);
        }
        cx.notify();
    }

    fn remeasure_flag(&self, id: u16) {
        if let Some(ix) = self.item_keys.iter().position(|k| *k == u32::from(id)) {
            self.list.remeasure_items(ix..ix + 1);
        }
    }

    /// The app state changed: inputs whose flag changed elsewhere (preset,
    /// reset, import) show the new value, and rows are remeasured when the
    /// stored layer changed.
    fn resync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (current, overrides_changed): (Vec<(u16, Value)>, bool) = {
            let s = self.state.read(cx);
            (
                self.inputs
                    .keys()
                    .map(|id| (*id, s.flags.value(FlagId(*id)).clone()))
                    .collect(),
                s.flag_overrides != self.measured_overrides,
            )
        };
        if overrides_changed {
            self.measured_overrides = self.state.read(cx).flag_overrides.clone();
            self.list.remeasure();
        }
        for (id, value) in current {
            if self.synced.get(&id) == Some(&value) {
                continue;
            }
            if let Some(input) = self.inputs.get(&id) {
                let text = display(&value);
                input.update(cx, |s, cx| s.set_value(text, window, cx));
            }
            self.synced.insert(id, value);
            self.errors.remove(&id);
        }
    }

    fn in_nav(def: &FlagDef, nav: Nav, flags: &flags::Flags) -> bool {
        match nav {
            Nav::All | Nav::Presets => true,
            Nav::Modified => flags.is_overridden(def.id),
            Nav::Category(category) => def.category() == category,
            Nav::Unavailable => !flags.is_available(def.id),
        }
    }

    fn in_state(def: &FlagDef, filter: StateFilter, flags: &flags::Flags) -> bool {
        match filter {
            StateFilter::All => true,
            StateFilter::On => def.is_on(flags.value(def.id)),
            StateFilter::Off => !def.is_on(flags.value(def.id)),
        }
    }

    /// The rows to list, and how many flags would match the search but are
    /// hidden by "Show bug fixes" or the All / On / Off switch.
    fn visible_rows(&self, query: &str, cx: &App) -> (Vec<Item>, usize) {
        let q = query.trim().to_lowercase();
        let flags = &self.state.read(cx).flags;
        let grouped = matches!(self.nav, Nav::All | Nav::Modified | Nav::Unavailable);
        let mut flat: Vec<(&'static FlagDef, Hit)> = Vec::new();
        let mut hidden = 0;
        for def in REGISTRY {
            if !Self::in_nav(def, self.nav, flags) {
                continue;
            }
            let Some(hit) = matches(def, &q) else {
                continue;
            };
            if def.is_shown(self.show_bug_fixes) && Self::in_state(def, self.state_filter, flags) {
                flat.push((def, hit));
            } else {
                hidden += 1;
            }
        }
        // a category's overflow block (1100, …) joins its first block
        flat.sort_by_key(|(def, _)| Category::ALL.iter().position(|c| *c == def.category()));
        let mut items = Vec::with_capacity(flat.len() + Category::ALL.len());
        let mut ix = 0;
        while ix < flat.len() {
            let category = flat[ix].0.category();
            let run = flat[ix..]
                .iter()
                .take_while(|(def, _)| def.category() == category)
                .count();
            if grouped {
                items.push(Item::Header(category, run));
            }
            items.extend(
                flat[ix..ix + run]
                    .iter()
                    .map(|(def, hit)| Item::Flag(def, hit.clone())),
            );
            ix += run;
        }
        (items, hidden)
    }

    /// Hand the list its rows. A new view (navigation, search, filters)
    /// starts it over at the top; rows coming and going with the flags'
    /// values (a toggle under On / Off) are spliced so the scroll position
    /// stays.
    fn sync_list(&mut self, items: Vec<Item>, view: ViewKey) {
        let keys: Vec<u32> = items.iter().map(Item::key).collect();
        if view != self.view_key {
            self.view_key = view;
            self.list.reset(keys.len());
            self.item_keys = keys;
        } else if keys != self.item_keys {
            let old = &self.item_keys;
            let prefix = old.iter().zip(&keys).take_while(|(a, b)| a == b).count();
            let suffix = old[prefix..]
                .iter()
                .rev()
                .zip(keys[prefix..].iter().rev())
                .take_while(|(a, b)| a == b)
                .count();
            self.list
                .splice(prefix..old.len() - suffix, keys.len() - prefix - suffix);
            // a header's count changed with the rows under it
            self.list.remeasure();
            self.item_keys = keys;
        }
        self.items = Rc::new(items);
        let zoom = crate::theme::sizes::zoom_factor();
        if self.zoom_seen != zoom {
            self.zoom_seen = zoom;
            self.list.remeasure();
        }
    }

    fn paste_json(&mut self, cx: &mut Context<Self>) {
        self.import_error = None;
        self.import_note = None;
        match cx.read_from_clipboard().and_then(|item| item.text()) {
            None => self.import_error = Some("The clipboard has no text to import.".into()),
            Some(text) => match Dispatcher::import_flags_json(&text, cx) {
                Ok(report) if report.unknown.is_empty() => {}
                Ok(report) => {
                    self.import_note = Some(format!(
                        "Imported. Unknown flags were skipped: {}.",
                        report.unknown.join(", ")
                    ))
                }
                Err(err) => self.import_error = Some(format!("Could not import flags: {err}")),
            },
        }
        cx.notify();
    }

    /// Pick `preset`, asking first when flags are modified (picking a preset
    /// clears the overrides).
    fn request_preset(&mut self, preset: Preset, window: &mut Window, cx: &mut Context<Self>) {
        let custom = self.state.read(cx).flags.modified_count() > 0;
        if custom {
            self.open_confirm(Confirm::Preset(preset), window, cx);
        } else if self.state.read(cx).flags.preset() != preset {
            Dispatcher::apply_preset(preset, cx);
        }
    }

    fn open_confirm(&mut self, confirm: Confirm, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm = Some(confirm);
        // keys stop going to the search box while the sheet is up
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    fn run_confirm(&mut self, cx: &mut Context<Self>) {
        match self.confirm.take() {
            Some(Confirm::ResetAll) => Dispatcher::reset_all_flags(cx),
            Some(Confirm::Preset(preset)) => Dispatcher::apply_preset(preset, cx),
            None => {}
        }
        cx.notify();
    }

    fn close(cx: &mut App) {
        Dispatcher::close_popup(cx);
    }

    // ---- rendering ----

    fn nav_row(
        &self,
        nav: Nav,
        label: &'static str,
        icon: Octicon,
        trailing: NavTrailing,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd();
        let selected = self.nav == nav;
        let (text, icon_color, badge_bg, badge_fg, secondary) = if selected {
            (
                t.box_selected_active_text,
                t.box_selected_active_text,
                t.list_item_selected_active_badge_background,
                t.list_item_selected_active_badge_text,
                t.box_selected_active_text,
            )
        } else {
            (
                t.text,
                t.text_secondary,
                t.list_item_badge_background,
                t.list_item_badge_text,
                t.text_secondary,
            )
        };
        let aria = match &trailing {
            NavTrailing::Count(count) => format!("{label}, {count}"),
            NavTrailing::Text(text) => format!("{label}, {text}"),
        };
        div()
            .id(SharedString::from(format!("flags-nav-{label}")))
            .a11y_row(aria, selected)
            .h(ROW_HEIGHT())
            .mx(SPACING())
            .my(zpx(1.))
            .px(SPACING())
            .rounded(BORDER_RADIUS())
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING())
            .text_size(FONT_SIZE())
            .text_color(text)
            .cursor_pointer()
            .when(selected, |d| d.bg(t.box_selected_active_background))
            .when(!selected, |d| d.hover(|d| d.bg(t.tab_bar_hover_background)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.nav = nav;
                cx.notify();
            }))
            .child(octicon(icon, icon_color).size(ICON_SIZE()))
            .child(div().flex_1().min_w_0().truncate().child(label))
            .child(match trailing {
                NavTrailing::Count(count) => counter(count, cx)
                    .bg(badge_bg)
                    .text_color(badge_fg)
                    .into_any_element(),
                NavTrailing::Text(text) => div()
                    .flex_none()
                    .text_size(FONT_SIZE_SM())
                    .text_color(secondary)
                    .child(text)
                    .into_any_element(),
            })
            .into_any_element()
    }

    fn nav_separator(&self, cx: &App) -> AnyElement {
        let t = cx.ghd();
        div()
            .h(zpx(1.))
            .mx(SPACING_DOUBLE())
            .my(SPACING_HALF())
            .bg(t.box_border_contrast.opacity(0.5))
            .into_any_element()
    }

    fn group_header(category: Category, count: usize, modified: usize, cx: &App) -> AnyElement {
        let t = cx.ghd();
        div()
            .id(("flags-group", category.block() as usize))
            .role(Role::Heading)
            .aria_label(format!(
                "{}, {}",
                category.title(),
                plural(count, "flag", "flags")
            ))
            .w_full()
            .h(ROW_HEIGHT())
            .px(SPACING_DOUBLE())
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING())
            .bg(t.box_alt_background)
            .border_b_1()
            .border_color(t.box_border)
            .text_size(FONT_SIZE())
            .child(octicon(category_icon(category), t.text_secondary).size(ICON_SIZE()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.text)
                    .child(format!("{} · {}", category.block(), category.title())),
            )
            .when(modified > 0, |d| {
                d.child(
                    link_button(
                        SharedString::from(format!("flags-reset-group-{}", category.block())),
                        "Reset to preset",
                        cx,
                    )
                    .ghd_tooltip(format!(
                        "Put the {} modified {} back to the preset's values",
                        modified,
                        if modified == 1 { "flag" } else { "flags" }
                    ))
                    .text_size(FONT_SIZE_SM())
                    .on_click(move |_, _, cx| Dispatcher::reset_category_flags(category, cx)),
                )
            })
            .child(
                div()
                    .flex_none()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(plural(count, "flag", "flags")),
            )
            .into_any_element()
    }

    fn control(
        &self,
        def: &'static FlagDef,
        value: &Value,
        disabled: bool,
        window: &Window,
        cx: &App,
    ) -> Option<AnyElement> {
        let t = cx.ghd();
        let id = def.id;
        Some(match def.kind {
            Kind::Bool => switch(
                ("flag-switch", id.0 as usize),
                value.as_bool().unwrap_or(false),
                disabled,
                move |on, _, cx| {
                    let _ = Dispatcher::set_flag(id, Value::Bool(on), cx);
                },
                cx,
            )
            .aria_label(def.title)
            .into_any_element(),
            Kind::Select { options } => {
                let current = value.as_text().unwrap_or("");
                let selected = options.iter().position(|o| o.value == current);
                let labels: Vec<SharedString> = options
                    .iter()
                    .map(|o| {
                        if Value::text(o.value) == def.corvene {
                            format!("{} (default)", o.label).into()
                        } else {
                            o.label.into()
                        }
                    })
                    .collect();
                let shown = selected
                    .and_then(|ix| labels.get(ix).cloned())
                    .unwrap_or_else(|| current.to_string().into());
                let on_select: SelectHandler = Rc::new(move |ix, _, cx| {
                    if let Some(option) = options.get(ix) {
                        let _ = Dispatcher::set_flag(id, Value::text(option.value), cx);
                    }
                });
                div()
                    .w_full()
                    .child(select_button(
                        ("flag-select", id.0 as usize),
                        shown,
                        labels,
                        selected,
                        disabled,
                        on_select,
                        cx,
                    ))
                    .into_any_element()
            }
            Kind::Number { unit, .. } => {
                let input = self.inputs.get(&id.0)?;
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .gap(SPACING_HALF())
                    .child(div().w(zpx(100.)).child(text_box_opts(
                        ("flag-input", id.0 as usize),
                        input,
                        None,
                        disabled,
                        window,
                        cx,
                    )))
                    .when_some(unit, |d, unit| {
                        d.child(
                            div()
                                .text_size(FONT_SIZE_SM())
                                .text_color(t.text_secondary)
                                .child(unit),
                        )
                    })
                    .into_any_element()
            }
            // under the description, as wide as the text column allows
            Kind::Text { .. } => return None,
        })
    }

    fn flag_row(&self, def: &'static FlagDef, hit: &Hit, window: &Window, cx: &App) -> AnyElement {
        let t = cx.ghd();
        let id = def.id;
        let (value, overridden, env_locked, available, preset) = {
            let flags = &self.state.read(cx).flags;
            (
                flags.value(id).clone(),
                flags.is_overridden(id),
                flags.is_env_locked(id),
                flags.is_available(id),
                flags.preset(),
            )
        };
        let disabled = env_locked || !available;
        let unavailable_reason = match def.availability() {
            flags::Availability::Available => None,
            flags::Availability::BuiltIn(reason) => Some(reason),
        };
        let ident = def.ident();
        let on = def.is_on(&value);
        let mut aria = format!("{}, {ident}, {}", def.title, if on { "on" } else { "off" });
        if overridden {
            aria.push_str(", modified");
        }
        if let Some(reason) = unavailable_reason {
            aria.push_str(&format!(", unavailable in this build: {reason}"));
        } else if env_locked {
            let source = self.state.read(cx).flag_lock_source(Some(id));
            aria.push_str(&format!(", set by {source}"));
        }

        let title = div()
            .font_weight(FontWeight::SEMIBOLD)
            .text_size(FONT_SIZE_MD())
            .line_height(zpx(20.))
            .text_color(t.text)
            .child(highlighted(def.title, &hit.title));

        let mut meta = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(SPACING_HALF())
            .child(
                pill(
                    "",
                    None,
                    if hit.ident.is_empty() {
                        t.box_alt_background
                    } else {
                        t.box_border_accent.opacity(0.35)
                    },
                    t.text,
                    cx,
                )
                .border_1()
                .border_color(t.box_border_contrast.opacity(0.6))
                .font_family(mono_font())
                .child(highlighted(&ident, &hit.ident)),
            );
        if overridden {
            meta = meta.child(pill(
                format!(
                    "Modified ({} preset: {})",
                    preset.title(),
                    def.label_for(preset.value_of(def))
                ),
                Some(Octicon::Pencil),
                t.accent.opacity(0.18),
                t.text,
                cx,
            ));
        }
        if def.is_bug_fix() {
            meta = meta.child(pill(
                Nature::BugFix.label(),
                None,
                t.box_alt_background,
                t.text,
                cx,
            ));
        }
        if def.restart {
            meta = meta.child(pill(
                "Restart required",
                Some(Octicon::SyncClockwise),
                t.banner_warning_background,
                t.banner_warning_text,
                cx,
            ));
        }
        if env_locked {
            let source = self.state.read(cx).flag_lock_source(Some(id));
            meta = meta.child(pill(
                format!("Set by {source}"),
                Some(Octicon::Lock),
                t.box_alt_background,
                t.text,
                cx,
            ));
        }
        if unavailable_reason.is_some() {
            meta = meta.child(pill(
                "Unavailable",
                Some(Octicon::Stop),
                t.form_error_background,
                t.form_error_text,
                cx,
            ));
        }
        for (ix, upstream) in def.upstream.iter().enumerate() {
            let url = upstream.url();
            meta = meta.child(
                link_button(
                    SharedString::from(format!("flag-upstream-{}-{ix}", id.0)),
                    upstream.label(),
                    cx,
                )
                .text_size(FONT_SIZE_SM())
                .when(hit.issue, |d| d.font_weight(FontWeight::SEMIBOLD))
                .flex()
                .flex_row()
                .items_center()
                .gap(zpx(2.))
                .child(octicon(Octicon::LinkExternal, t.link).size(zpx(12.)))
                .on_click(move |_, _, cx| corvene_core::Dispatcher::open_url(&url, cx)),
            );
        }

        let text_input = match def.kind {
            Kind::Text { .. } => self.inputs.get(&id.0).map(|input| {
                div()
                    .max_w(zpx(420.))
                    .child(text_box_opts(
                        ("flag-input", id.0 as usize),
                        input,
                        None,
                        disabled,
                        window,
                        cx,
                    ))
                    .into_any_element()
            }),
            _ => None,
        };
        let error = self.errors.get(&id.0).cloned();

        let text_column = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(zpx(6.))
            .when(!available, |d| d.opacity(0.75))
            .child(title)
            .child(meta)
            .child(
                div()
                    .text_size(FONT_SIZE())
                    .line_height(zpx(18.))
                    .text_color(t.text)
                    .child(highlighted(def.summary, &hit.summary)),
            )
            .child(
                div()
                    .flex()
                    .map(|d| {
                        if crate::theme::compact(window) {
                            d.flex_col()
                        } else {
                            d.flex_row().gap(SPACING_HALF())
                        }
                    })
                    .items_start()
                    .text_size(FONT_SIZE_SM())
                    .line_height(zpx(16.))
                    .text_color(t.text_secondary)
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("GitHub Desktop:"),
                    )
                    .child(
                        div()
                            .map(|d| {
                                if crate::theme::compact(window) {
                                    d.w_full()
                                } else {
                                    d.flex_1().min_w_0()
                                }
                            })
                            .child(def.ghd_behaviour),
                    ),
            )
            .when_some(unavailable_reason, |d, reason| {
                d.child(
                    div()
                        .text_size(FONT_SIZE_SM())
                        .line_height(zpx(16.))
                        .text_color(t.error)
                        .child(format!("Unavailable in this build: {reason}")),
                )
            })
            .children(text_input)
            .when(text_input_error(def), |d| {
                d.when_some(error.clone(), |d, message| {
                    d.child(
                        div()
                            .text_size(FONT_SIZE_SM())
                            .text_color(t.error)
                            .child(message),
                    )
                })
            });

        // the compact (phone) layout has no room for a fixed column: a
        // switch takes its own width, a select goes under the text
        let compact = crate::theme::compact(window);
        let stacked = compact && matches!(def.kind, Kind::Select { .. });
        let control = self.control(def, &value, disabled, window, cx);
        let control_column = div()
            .flex_none()
            .map(|d| match def.kind {
                _ if stacked => d.w_full(),
                _ if compact => d,
                Kind::Select { .. } => d.w(zpx(200.)),
                _ => d.w(zpx(140.)),
            })
            .flex()
            .flex_col()
            .items_end()
            .gap(zpx(6.))
            .children(control)
            .when(!text_input_error(def), |d| {
                d.when_some(error, |d, message| {
                    d.child(
                        div()
                            .text_size(FONT_SIZE_SM())
                            .text_color(t.error)
                            .text_right()
                            .child(message),
                    )
                })
            })
            .when(overridden && !disabled, |d| {
                d.child(
                    link_button(
                        SharedString::from(format!("flag-reset-{}", id.0)),
                        "Reset",
                        cx,
                    )
                    .ghd_tooltip(format!("Back to the {} preset's value", preset.title()))
                    .text_size(FONT_SIZE_SM())
                    .on_click(move |_, _, cx| Dispatcher::reset_flag(id, cx)),
                )
            });

        div()
            .id(("flag", id.0 as usize))
            .role(Role::Group)
            .aria_label(aria)
            .relative()
            .w_full()
            .pl(SPACING_DOUBLE())
            .pr(SPACING_DOUBLE())
            .py(zpx(14.))
            .gap(if compact { SPACING() } else { SPACING_DOUBLE() })
            .flex()
            .map(|d| if stacked { d.flex_col() } else { d.flex_row() })
            .items_start()
            .border_b_1()
            .border_color(t.box_border_contrast.opacity(0.35))
            .when(overridden, |d| d.bg(t.accent.opacity(0.06)))
            .hover(|d| d.bg(t.list_item_hover_background))
            // the keyboard cursor (↑ / ↓)
            .when(self.cursor == Some(id), |d| {
                d.bg(t.box_selected_background)
                    .shadow(crate::keyboard_nav::inset_ring_shadows(cx))
            })
            .when(overridden, |d| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(zpx(3.))
                        .bg(t.accent),
                )
            })
            .child(text_column)
            .child(control_column)
            .into_any_element()
    }

    fn empty_state(&self, query: &str, hidden: usize, cx: &App) -> AnyElement {
        let t = cx.ghd();
        let mut message = if !query.trim().is_empty() {
            format!("No flags match “{}”.", query.trim())
        } else {
            match (self.nav, self.state_filter) {
                (_, StateFilter::On) => "No flags here are on.".to_string(),
                (_, StateFilter::Off) => "No flags here are off.".to_string(),
                (Nav::Modified, _) => "No flags differ from the preset.".to_string(),
                (Nav::Unavailable, _) => "Every flag is available in this build.".to_string(),
                _ => "No flags here yet.".to_string(),
            }
        };
        if hidden > 0 {
            let what = match (self.show_bug_fixes, self.state_filter) {
                (false, StateFilter::All) => "tick Show bug fixes",
                (true, _) => "switch the filter to All",
                (false, _) => "switch the filter to All or tick Show bug fixes",
            };
            message.push_str(&format!(
                " {} hidden: {what} to see {}.",
                plural(hidden, "flag is", "flags are"),
                if hidden == 1 { "it" } else { "them" },
            ));
        }
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p(SPACING_DOUBLE())
            .text_color(t.text_secondary)
            .text_center()
            .child(message)
            .into_any_element()
    }

    /// The All / On / Off switch: GHD-styled segmented buttons.
    fn state_switch(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.ghd().clone();
        let mut group = div()
            .id("flags-state-filter")
            .role(Role::RadioGroup)
            .aria_label("Show flags")
            .flex_none()
            .h(TEXT_FIELD_HEIGHT())
            .flex()
            .flex_row()
            .items_stretch()
            .border_1()
            .border_color(t.box_border_contrast)
            .rounded(BORDER_RADIUS())
            .overflow_hidden();
        for (ix, filter) in StateFilter::ALL.into_iter().enumerate() {
            let selected = self.state_filter == filter;
            let hover = t.secondary_button_hover_background;
            group = group.child(
                div()
                    .id(("flags-state", ix))
                    .role(Role::RadioButton)
                    .aria_label(filter.label())
                    .aria_toggled(if selected {
                        Toggled::True
                    } else {
                        Toggled::False
                    })
                    .ghd_tooltip(filter.tooltip())
                    .px(SPACING())
                    .flex()
                    .items_center()
                    .text_size(FONT_SIZE())
                    .when(ix > 0, |d| {
                        d.border_l_1().border_color(t.box_border_contrast)
                    })
                    .when(selected, |d| {
                        d.bg(t.box_selected_active_background)
                            .text_color(t.box_selected_active_text)
                            .font_weight(FontWeight::SEMIBOLD)
                    })
                    .when(!selected, |d| {
                        d.bg(t.box_background)
                            .text_color(t.text)
                            .cursor_pointer()
                            .hover(move |s| s.bg(hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state_filter = filter;
                                cx.notify();
                            }))
                    })
                    .child(filter.label()),
            );
        }
        group.into_any_element()
    }

    /// The Presets page: every preset with its details and numbers; a click
    /// picks it.
    fn presets_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.ghd().clone();
        let (current, custom, modified, values): (Preset, bool, usize, Vec<Value>) = {
            let flags = &self.state.read(cx).flags;
            (
                flags.preset(),
                flags.modified_count() > 0,
                flags.modified_count(),
                REGISTRY
                    .iter()
                    .map(|def| flags.value(def.id).clone())
                    .collect(),
            )
        };
        let from_env = self.state.read(cx).flags.preset_from_env();
        let total = REGISTRY.len();
        let bug_fixes = REGISTRY.iter().filter(|d| d.is_bug_fix()).count();

        let mut page = div()
            .id("flags-presets")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .p(SPACING_DOUBLE())
            .flex()
            .flex_col()
            .gap(SPACING())
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(FONT_SIZE_MD())
                    .text_color(t.text)
                    .child("Presets"),
            )
            .child(
                div()
                    .text_size(FONT_SIZE())
                    .line_height(zpx(18.))
                    .text_color(t.text_secondary)
                    .mb(SPACING())
                    .child(
                        "A preset sets every flag at once. Flags you change afterwards are \
                         listed under Modified and marked with a blue bar; picking a preset \
                         or Reset all puts them back to the preset's values.",
                    ),
            );
        if from_env {
            page = page.child(
                div()
                    .px(SPACING())
                    .py(SPACING_HALF())
                    .rounded(BORDER_RADIUS())
                    .bg(t.banner_warning_background)
                    .text_color(t.banner_warning_text)
                    .text_size(FONT_SIZE())
                    .child(format!(
                        "{} sets the preset for this session; relaunch without it to pick \
                         another.",
                        self.state.read(cx).flag_lock_source(None)
                    )),
            );
        }
        let weak = cx.weak_entity();
        for (ix, preset) in Preset::ALL.into_iter().enumerate() {
            let selected = preset == current;
            let on = REGISTRY
                .iter()
                .filter(|def| def.is_on(preset.value_of(def)))
                .count();
            let fixes_on = REGISTRY
                .iter()
                .filter(|def| def.is_bug_fix() && def.is_on(preset.value_of(def)))
                .count();
            let differs = REGISTRY
                .iter()
                .zip(&values)
                .filter(|(def, value)| preset.value_of(def) != *value)
                .count();
            let hover = t.box_hover_background;
            let mut stats = vec![
                format!("{on} of {total} flags on"),
                format!("{fixes_on} of {bug_fixes} bug fixes"),
            ];
            if selected && custom {
                stats.push(format!("{} modified", plural(modified, "flag", "flags")));
            } else if !selected {
                stats.push(if differs == 0 {
                    "same as your current values".to_string()
                } else {
                    format!("would change {}", plural(differs, "flag", "flags"))
                });
            }
            let weak = weak.clone();
            let enabled = !from_env && !(selected && !custom);
            page = page.child(
                div()
                    .id(("flags-preset-card", ix))
                    .role(Role::RadioButton)
                    .aria_label(format!("{}: {}", preset.title(), preset.description()))
                    .aria_toggled(if selected {
                        Toggled::True
                    } else {
                        Toggled::False
                    })
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(SPACING())
                    .p(SPACING())
                    .border_1()
                    .rounded(BORDER_RADIUS())
                    .border_color(if selected {
                        t.accent
                    } else {
                        t.box_border_contrast.opacity(0.6)
                    })
                    .when(selected, |d| d.bg(t.accent.opacity(0.08)))
                    .when(enabled, |d| {
                        d.cursor_pointer().hover(move |s| s.bg(hover)).on_click(
                            move |_, window, cx| {
                                weak.update(cx, |this, cx| this.request_preset(preset, window, cx))
                                    .ok();
                            },
                        )
                    })
                    .child(radio(("flags-preset-radio", ix), selected, cx).mt(zpx(3.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(zpx(4.))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(SPACING_HALF())
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_size(FONT_SIZE_MD())
                                            .text_color(t.text)
                                            .child(preset.title()),
                                    )
                                    .when(preset == Preset::default(), |d| {
                                        d.child(pill(
                                            "Default",
                                            None,
                                            t.box_alt_background,
                                            t.text,
                                            cx,
                                        ))
                                    })
                                    .when(selected, |d| {
                                        d.child(pill(
                                            if custom {
                                                "Current, modified"
                                            } else {
                                                "Current"
                                            },
                                            Some(Octicon::Check),
                                            t.accent.opacity(0.25),
                                            t.text,
                                            cx,
                                        ))
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(FONT_SIZE())
                                    .line_height(zpx(18.))
                                    .text_color(t.text)
                                    .child(preset.description()),
                            )
                            .child(
                                div()
                                    .text_size(FONT_SIZE())
                                    .line_height(zpx(18.))
                                    .text_color(t.text_secondary)
                                    .child(preset.details()),
                            )
                            .child(
                                div()
                                    .mt(zpx(2.))
                                    .text_size(FONT_SIZE_SM())
                                    .text_color(t.text_secondary)
                                    .child(stats.join(" · ")),
                            ),
                    ),
            );
        }
        page.with_scrollbar().into_any_element()
    }

    /// The confirmation sheet over the dialog (GHD's destructive-dialog
    /// layout: Cancel is the primary button).
    fn confirm_sheet(&self, confirm: Confirm, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.ghd().clone();
        let (preset, modified_names) = {
            let flags = &self.state.read(cx).flags;
            (
                flags.preset(),
                REGISTRY
                    .iter()
                    .filter(|def| flags.is_overridden(def.id))
                    .map(|def| def.title)
                    .collect::<Vec<_>>(),
            )
        };
        let n = modified_names.len();
        let (title, body, action): (String, String, String) = match confirm {
            Confirm::ResetAll => (
                "Reset all flags?".into(),
                format!(
                    "{} you changed will go back to the {} preset's values.",
                    plural(n, "flag", "flags"),
                    preset.title()
                ),
                "Reset All".into(),
            ),
            Confirm::Preset(to) => (
                format!("Switch to the {} preset?", to.title()),
                format!(
                    "Every flag takes the {} preset's value, and your changes to {} are \
                     discarded.",
                    to.title(),
                    plural(n, "flag", "flags")
                ),
                format!("Use {}", to.title()),
            ),
        };
        const SHOWN: usize = 6;
        let bullet = |text: String| {
            div()
                .flex()
                .flex_row()
                .gap(SPACING_HALF())
                .child(div().flex_none().child("•"))
                .child(div().flex_1().min_w_0().child(text))
        };
        let mut names: Vec<Div> = modified_names
            .iter()
            .take(SHOWN)
            .map(|s| bullet(s.to_string()))
            .collect();
        if n > SHOWN {
            names.push(div().child(format!("and {} more", n - SHOWN)));
        }
        let cancel = cx.listener(|this, _, _, cx| {
            this.confirm = None;
            cx.notify();
        });
        let cancel_backdrop = cx.listener(|this, _, _, cx| {
            this.confirm = None;
            cx.stop_propagation();
            cx.notify();
        });
        let run = cx.listener(|this, _, _, cx| this.run_confirm(cx));
        div()
            .id("flags-confirm-layer")
            .occlude()
            .child(crate::widgets::touch_drag_occluder())
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(t.dialog_backdrop)
            .on_mouse_down(MouseButton::Left, cancel_backdrop)
            .child(
                div()
                    .id("flags-confirm")
                    .role(Role::AlertDialog)
                    .aria_label(title.clone())
                    .w(crate::theme::fit_width(420.))
                    .flex()
                    .flex_col()
                    .rounded(BORDER_RADIUS())
                    .bg(t.background)
                    .text_color(t.text)
                    .text_size(FONT_SIZE())
                    .border_1()
                    .border_color(t.box_border)
                    .shadow(vec![BoxShadow {
                        color: t.shadow,
                        offset: point(zpx(0.), zpx(2.)),
                        blur_radius: css_blur(7.),
                        spread_radius: zpx(0.),
                        inset: false,
                    }])
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .h(zpx(50.))
                            .px(SPACING_DOUBLE())
                            .flex()
                            .items_center()
                            .border_b_1()
                            .border_color(t.box_border)
                            .text_size(FONT_SIZE_MD())
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .p(SPACING_DOUBLE())
                            .flex()
                            .flex_col()
                            .gap(SPACING())
                            .line_height(zpx(18.))
                            .child(body)
                            .when(!names.is_empty(), |d| {
                                d.child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .text_color(t.text_secondary)
                                        .children(names),
                                )
                            }),
                    )
                    .child(
                        div()
                            .p(SPACING_DOUBLE())
                            .border_t_1()
                            .border_color(t.box_border)
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap(SPACING_HALF())
                            .children(crate::dialog::ok_cancel_order(vec![
                                primary_button("flags-confirm-cancel", "Cancel", false, cx)
                                    .min_w(zpx(120.))
                                    .on_click(cancel),
                                button("flags-confirm-ok", action, cx)
                                    .min_w(zpx(120.))
                                    .on_click(run),
                            ])),
                    ),
            )
            .into_any_element()
    }
}

/// What the user chose to look at; a change starts the list over at the
/// top.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ViewKey {
    nav: Option<Nav>,
    state_filter: Option<StateFilter>,
    show_bug_fixes: bool,
    query: String,
}

/// Where a nav row's count sits: a badge, or a short text (the Presets row
/// names the current preset).
enum NavTrailing {
    Count(usize),
    Text(SharedString),
}

/// Text flags show their validation message under the input in the text
/// column; the others under the control.
fn text_input_error(def: &FlagDef) -> bool {
    matches!(def.kind, Kind::Text { .. })
}

impl Render for FlagsDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // a copy: the nav rows below need `cx` mutably for their listeners
        let t = cx.ghd().clone();
        let viewport = crate::theme::page_size(window);
        let width = (viewport.width - zpx(80.)).max(zpx(720.)).min(zpx(1000.));
        let height = (viewport.height - zpx(80.)).max(zpx(480.)).min(zpx(760.));

        let show_bug_fixes = self.show_bug_fixes;
        let state_filter = self.state_filter;
        let (preset, preset_from_env, all_modified, modified, restart_pending, nav_counts) = {
            let s = self.state.read(cx);
            let flags = &s.flags;
            // one pass: which navs each shown flag counts towards
            let mut all = 0;
            let mut modified_nav = 0;
            let mut unavailable = 0;
            let mut per_category = [0usize; Category::ALL.len()];
            for def in REGISTRY {
                if !def.is_shown(show_bug_fixes) || !Self::in_state(def, state_filter, flags) {
                    continue;
                }
                all += 1;
                if flags.is_overridden(def.id) {
                    modified_nav += 1;
                }
                if !flags.is_available(def.id) {
                    unavailable += 1;
                }
                if let Some(ix) = Category::ALL.iter().position(|c| *c == def.category()) {
                    per_category[ix] += 1;
                }
            }
            (
                flags.preset(),
                flags.preset_from_env(),
                flags.modified_count(),
                flags.modified_count_shown(show_bug_fixes),
                s.flags_restart_pending(),
                (all, modified_nav, unavailable, per_category),
            )
        };
        let (count_all, count_modified, count_unavailable, per_category) = nav_counts;
        let query = self.search.read(cx).value().to_string();
        // "Custom" and Reset all cover hidden bug fixes too; the counts do not
        let custom = all_modified > 0;
        let hidden_modified = all_modified - modified;

        // ---- header ----
        let header = div()
            .flex_none()
            .h(zpx(50.))
            .px(SPACING_DOUBLE())
            .flex()
            .flex_row()
            .items_center()
            .border_b_1()
            .border_color(t.box_border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .child(
                        div()
                            .text_size(FONT_SIZE_MD())
                            .line_height(zpx(18.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Flags"),
                    )
                    .child(
                        div()
                            .text_size(FONT_SIZE_SM())
                            .line_height(zpx(16.))
                            .text_color(t.text_secondary)
                            .truncate()
                            .child(
                                "Switches for the ways Corvene differs from GitHub Desktop. \
                                 Changes apply immediately.",
                            ),
                    ),
            )
            .child(
                div()
                    .id("flags-close")
                    .icon_button_label("Close")
                    .flex_none()
                    .size(ICON_SIZE())
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(octicon(Octicon::X, t.text_secondary))
                    .on_click(|_, _, cx| FlagsDialog::close(cx)),
            );

        // ---- toolbar ----
        let weak = cx.weak_entity();
        let more = button("flags-more", "", cx)
            .icon_button_label("More options")
            .px(SPACING_HALF())
            .child(octicon(Octicon::KebabHorizontal, t.secondary_button_text))
            .on_click(move |ev: &ClickEvent, window, cx| {
                let weak = weak.clone();
                let items = vec![
                    MenuItem::new("Copy as JSON", |_, cx| {
                        let json = Dispatcher::export_flags_json(cx);
                        cx.write_to_clipboard(ClipboardItem::new_string(json));
                    }),
                    MenuItem::new("Copy as CORVENE_FLAGS", |_, cx| {
                        let spec = Dispatcher::export_env_string(cx);
                        cx.write_to_clipboard(ClipboardItem::new_string(spec));
                    }),
                    MenuItem::separator(),
                    MenuItem::new("Paste JSON", move |_, cx| {
                        weak.update(cx, |this, cx| this.paste_json(cx)).ok();
                    }),
                ];
                let position = ev.mouse_position().unwrap_or_default();
                crate::native_menu::show_context_menu(items, position, window, cx);
            });
        let modified_label = (modified > 0 || hidden_modified > 0).then(|| {
            let mut label = format!("{modified} modified");
            if hidden_modified > 0 {
                label.push_str(&format!(
                    " (+{} hidden)",
                    plural(hidden_modified, "bug fix", "bug fixes")
                ));
            }
            label
        });
        let compact = crate::theme::compact(window);
        let toolbar = div()
            .flex_none()
            .map(|d| {
                if compact {
                    // the search box, the switch and the checkbox wrap
                    d.min_h(zpx(45.)).py(SPACING_HALF()).flex_wrap()
                } else {
                    d.h(zpx(45.))
                }
            })
            .px(SPACING_DOUBLE())
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING())
            .border_b_1()
            .border_color(t.box_border)
            .child(
                div()
                    .flex_1()
                    .min_w(zpx(160.))
                    .max_w(zpx(340.))
                    .child(filter_text_box(
                        "flags-search",
                        &self.search,
                        Some(octicon(Octicon::Search, t.text_secondary)),
                        window,
                        cx,
                    )),
            )
            .child(self.state_switch(cx))
            .child({
                let weak = cx.weak_entity();
                div().flex_none().child(checkbox_row(
                    "flags-show-bug-fixes",
                    show_bug_fixes,
                    "Show bug fixes",
                    move |on, _, cx| {
                        weak.update(cx, |this, cx| {
                            this.show_bug_fixes = on;
                            cx.notify();
                        })
                        .ok();
                    },
                    cx,
                ))
            })
            .when(!compact, |d| d.child(div().flex_1()))
            .when_some(modified_label, |d, label| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(FONT_SIZE_SM())
                        .text_color(t.text_secondary)
                        .child(label),
                )
            })
            .child(
                button("flags-reset-all", "Reset all", cx)
                    .flex_none()
                    .when(!custom, |d| d.opacity(0.5).cursor_default())
                    .when(custom, |d| {
                        d.on_click(cx.listener(|this, _, window, cx| {
                            this.open_confirm(Confirm::ResetAll, window, cx)
                        }))
                    }),
            )
            .child(more);

        // ---- navigation ----
        let mut nav = div()
            .id("flags-nav")
            .flex_none()
            .w(zpx(230.))
            .py(SPACING())
            .flex()
            .flex_col()
            .bg(t.box_alt_background)
            .border_r_1()
            .border_color(t.box_border)
            .role(Role::List)
            .aria_label("Flag categories")
            .child(self.nav_row(
                Nav::All,
                "All",
                Octicon::ListUnordered,
                NavTrailing::Count(count_all),
                cx,
            ))
            .child(self.nav_row(
                Nav::Modified,
                "Modified",
                Octicon::Pencil,
                NavTrailing::Count(count_modified),
                cx,
            ))
            .child(self.nav_row(
                Nav::Presets,
                "Presets",
                Octicon::Stack,
                NavTrailing::Text(if custom {
                    format!("{}*", preset.title()).into()
                } else {
                    preset.title().into()
                }),
                cx,
            ))
            .child(self.nav_separator(cx));
        for (ix, category) in Category::ALL.into_iter().enumerate() {
            let count = per_category[ix];
            if category == Category::Experimental && count == 0 {
                continue;
            }
            nav = nav.child(self.nav_row(
                Nav::Category(category),
                category.title(),
                category_icon(category),
                NavTrailing::Count(count),
                cx,
            ));
        }
        if count_unavailable > 0 {
            nav = nav.child(self.nav_separator(cx)).child(self.nav_row(
                Nav::Unavailable,
                "Unavailable",
                Octicon::Stop,
                NavTrailing::Count(count_unavailable),
                cx,
            ));
        }

        // ---- list ----
        let content: AnyElement = if self.nav == Nav::Presets {
            self.presets_page(cx)
        } else {
            let (items, hidden_matches) = self.visible_rows(&query, cx);
            let empty = items.is_empty();
            let view = ViewKey {
                nav: Some(self.nav),
                state_filter: Some(state_filter),
                show_bug_fixes,
                query: query.clone(),
            };
            self.sync_list(items, view);
            if empty {
                self.empty_state(&query, hidden_matches, cx)
            } else {
                let items = self.items.clone();
                let weak = cx.weak_entity();
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .relative()
                    .child(
                        div()
                            .id("flags-list")
                            .role(Role::List)
                            .aria_label("Flags")
                            .size_full()
                            .child(
                                list(self.list.clone(), move |ix, window, cx| {
                                    let Some(item) = items.get(ix) else {
                                        return div().into_any_element();
                                    };
                                    match item {
                                        Item::Header(category, count) => {
                                            let modified = {
                                                let flags = &AppState::global(cx).read(cx).flags;
                                                REGISTRY
                                                    .iter()
                                                    .filter(|d| {
                                                        d.category() == *category
                                                            && flags.is_overridden(d.id)
                                                            && !flags.is_env_locked(d.id)
                                                    })
                                                    .count()
                                            };
                                            FlagsDialog::group_header(
                                                *category, *count, modified, cx,
                                            )
                                        }
                                        Item::Flag(def, hit) => match weak.upgrade() {
                                            Some(this) => {
                                                this.read(cx).flag_row(def, hit, window, cx)
                                            }
                                            None => div().into_any_element(),
                                        },
                                    }
                                })
                                .size_full()
                                .pr(gutter(&self.list)),
                            ),
                    )
                    .child(scrollbar("flags-scrollbar", self.list.clone()))
                    .into_any_element()
            }
        };
        let body = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_row()
            .items_stretch()
            // a phone: the search box and the filter chips find flags; the
            // category list would leave the flags no room
            .when(!crate::theme::compact(window), |d| d.child(nav))
            .child(content);

        // ---- restart bar ----
        let restart_bar = (!restart_pending.is_empty()).then(|| {
            let names = restart_pending
                .iter()
                .map(|id| id.display())
                .collect::<Vec<_>>()
                .join(", ");
            let message = "Your changes will take effect the next time you relaunch Corvene.";
            div()
                .id("flags-restart-bar")
                .a11y_live(message)
                .flex_none()
                .h(zpx(40.))
                .px(SPACING_DOUBLE())
                .gap(SPACING())
                .flex()
                .flex_row()
                .items_center()
                .border_t_1()
                .border_color(t.box_border)
                .bg(t.banner_warning_background)
                .text_color(t.banner_warning_text)
                .text_size(FONT_SIZE())
                .child(octicon(Octicon::Alert, t.banner_warning_icon).size(ICON_SIZE()))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(format!("{message} ({names})")),
                )
                .child(
                    primary_button("flags-relaunch", "Relaunch", false, cx)
                        .min_w(zpx(100.))
                        .on_click(|_, _, cx| Dispatcher::relaunch(cx)),
                )
        });

        // ---- footer: the preset ----
        let preset_labels: Vec<SharedString> =
            Preset::ALL.iter().map(|p| p.title().into()).collect();
        let preset_ix = Preset::ALL.iter().position(|p| *p == preset);
        let preset_shown: SharedString = if custom {
            format!("{} (modified)", preset.title()).into()
        } else {
            preset.title().into()
        };
        let weak = cx.weak_entity();
        let on_preset: SelectHandler = Rc::new(move |ix, window, cx| {
            if let Some(preset) = Preset::ALL.get(ix).copied() {
                weak.update(cx, |this, cx| this.request_preset(preset, window, cx))
                    .ok();
            }
        });
        let compact_footer = crate::theme::compact(window);
        let footer = div()
            .flex_none()
            .p(SPACING_DOUBLE())
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING())
            .border_t_1()
            .border_color(t.box_border)
            .child(
                div()
                    .flex_none()
                    .text_size(FONT_SIZE())
                    .text_color(t.text)
                    .child("Preset"),
            )
            .child(
                div()
                    .map(|d| {
                        if compact_footer {
                            d.flex_1().min_w_0()
                        } else {
                            d.flex_none().w(zpx(180.))
                        }
                    })
                    .child(select_button(
                        "flags-preset",
                        preset_shown,
                        preset_labels,
                        preset_ix,
                        preset_from_env,
                        on_preset,
                        cx,
                    )),
            )
            .when(!compact_footer, |d| {
                d.child(
                    div()
                        .id("flags-preset-description")
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(FONT_SIZE_SM())
                        .text_color(t.text_secondary)
                        .child(if preset_from_env {
                            format!(
                                "{} Set by {}.",
                                preset.description(),
                                self.state.read(cx).flag_lock_source(None)
                            )
                        } else {
                            preset.description().to_string()
                        })
                        .when(self.nav != Nav::Presets, |d| {
                            d.cursor_pointer()
                                .ghd_tooltip("Compare the presets")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.nav = Nav::Presets;
                                    cx.notify();
                                }))
                        }),
                )
            })
            .child(
                primary_button("flags-done", "Done", false, cx)
                    .min_w(zpx(if compact_footer { 72. } else { 120. }))
                    .on_click(|_, _, cx| FlagsDialog::close(cx)),
            );

        let banner = self
            .import_error
            .clone()
            .map(|message| dialog_error_banner(message, cx).into_any_element())
            .or_else(|| {
                self.import_note.clone().map(|note| {
                    div()
                        .flex_none()
                        .px(SPACING_DOUBLE())
                        .py(SPACING_HALF())
                        .border_b_1()
                        .border_color(t.box_border)
                        .bg(t.banner_warning_background)
                        .text_color(t.banner_warning_text)
                        .text_size(FONT_SIZE_SM())
                        .child(note)
                        .into_any_element()
                })
            });

        let confirm = self.confirm.map(|c| self.confirm_sheet(c, cx));

        deferred(
            anchored().position(crate::theme::page_origin()).child(
                div()
                    .id("flags-overlay")
                    // modal: the views underneath get no hover, clicks or wheel
                    .occlude()
                    .child(crate::widgets::touch_drag_occluder())
                    .relative()
                    .w(viewport.width)
                    .h(viewport.height)
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(t.dialog_backdrop)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| FlagsDialog::close(cx))
                    .child(
                        div()
                            .id("flags-dialog")
                            .role(Role::Dialog)
                            .aria_label("Flags")
                            .child(window_title("Flags"))
                            .track_focus(&self.focus_handle)
                            .key_context("FlagsDialog")
                            .on_action(cx.listener(|this, _: &SelectNextFile, _, cx| {
                                if this.confirm.is_none() {
                                    this.move_cursor(1, cx);
                                }
                            }))
                            .on_action(cx.listener(|this, _: &SelectPreviousFile, _, cx| {
                                if this.confirm.is_none() {
                                    this.move_cursor(-1, cx);
                                }
                            }))
                            .on_action(cx.listener(|this, _: &Find, window, cx| {
                                if this.confirm.is_none() {
                                    let handle = this.search.read(cx).focus_handle(cx);
                                    window.focus(&handle, cx);
                                }
                                cx.stop_propagation();
                            }))
                            .on_action(cx.listener(|this, _: &CloseFoldout, _, cx| {
                                if this.confirm.take().is_some() {
                                    cx.notify();
                                } else {
                                    FlagsDialog::close(cx);
                                }
                                cx.stop_propagation();
                            }))
                            .w(width)
                            .h(height)
                            .flex()
                            .flex_col()
                            .rounded(BORDER_RADIUS())
                            .overflow_hidden()
                            .bg(t.background)
                            .text_color(t.text)
                            .text_size(FONT_SIZE())
                            .border_1()
                            .border_color(t.box_border)
                            .shadow(vec![BoxShadow {
                                color: t.shadow,
                                offset: point(zpx(0.), zpx(2.)),
                                blur_radius: css_blur(7.),
                                spread_radius: zpx(0.),
                                inset: false,
                            }])
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(header)
                            .child(toolbar)
                            .children(banner)
                            .child(body)
                            .children(restart_bar)
                            .child(footer),
                    )
                    .children(confirm),
            ),
        )
        .with_priority(20)
    }
}
