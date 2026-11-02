//! Electron's classic Linux menu bar (`shell/browser/ui/views/menu_bar.cc`,
//! `submenu_button.cc`), which GitHub Desktop shows on Linux instead of its
//! macOS title bar or Windows app menu (`app.tsx`: `renderTitlebar` returns
//! null, "Linux still uses the classic Electron menu").
//!
//! The items are the menu model `crates/corvene/src/menus.rs` installs with
//! `cx.set_menus` (GHD's non-darwin `build-default-menu.ts`), with `&`
//! mnemonics; the enabled state is GPUI's (an action without a live handler
//! is disabled, as in the macOS menu bar) combined with GHD's
//! `getMenuState` (`corvene_core::menu_state`); accelerators come from the
//! keymap. Dropdowns are `views_menu` popups.
//!
//! Measured from GitHub Desktop 3.6.6 on Linux (Adwaita): the bar is 28 px,
//! `#f6f5f4`, its buttons 6 px either side of the label, the open button
//! `#d7d7d6` over the top 27 px, the keyboard-focused one `#e8e7e6` with
//! the edge of Chromium's focus ring past its right side. Alt shows the
//! mnemonic underlines; Alt alone focuses the bar (← / → move, ↓ / ↑ /
//! Enter open, Escape leaves); Alt+letter opens that menu. Electron's zoom
//! applies to the page only, so the bar ignores the window zoom factor.

use std::rc::Rc;

use crate::app_menu::menu_id;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::views_menu::{self, Anchor, Entry, EntryKind, Palette, Source};

/// Android: as low and tight as the labels allow; a phone's screen has
/// little room for a desktop's menu bar.
pub const HEIGHT: f32 = if cfg!(target_os = "android") {
    22.
} else {
    28.
};
/// The open button's background covers the bar minus its last row.
#[cfg(not(windows))]
const OPEN_HEIGHT: f32 = HEIGHT - 1.;
#[cfg(not(windows))]
const BUTTON_PADDING: f32 = if cfg!(target_os = "android") { 5. } else { 6. };
/// Windows: the bar is GitHub Desktop's own app menu bar inside its title
/// bar (`crate::title_bar_windows`, `_app-menu-bar.scss`): the open button
/// is as tall as the bar, the labels have `var(--spacing)` either side.
#[cfg(windows)]
const OPEN_HEIGHT: f32 = HEIGHT;
#[cfg(windows)]
const BUTTON_PADDING: f32 = 10.;

/// Electron's role accelerators, for the Edit items whose actions the
/// text fields bind themselves.
fn role_accelerator(os_action: OsAction) -> &'static str {
    match os_action {
        OsAction::Undo => "Ctrl+Z",
        // Electron's `redo` role on Windows
        OsAction::Redo if cfg!(windows) => "Ctrl+Y",
        OsAction::Redo => "Ctrl+Shift+Z",
        OsAction::Cut => "Ctrl+X",
        OsAction::Copy => "Ctrl+C",
        OsAction::Paste => "Ctrl+V",
        OsAction::SelectAll => "Ctrl+A",
    }
}

struct Button {
    text: SharedString,
    mnemonic: Option<usize>,
    x: f32,
    width: f32,
}

pub struct MenuBar {
    /// The keyboard-focused button (Alt released alone, or ← / →).
    focused: Option<usize>,
    /// The button Escape left focused after a menu opened with the pointer
    /// (Windows): no access keys show for it.
    quiet: Option<usize>,
    /// Alt is down with no other key yet: underlines show, and releasing
    /// it focuses the bar.
    alt_alone: bool,
    buttons: Vec<Button>,
}

impl MenuBar {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let this = cx.entity().downgrade();
        views_menu::set_switch_menu(Rc::new(move |delta, cx| {
            let Some(bar) = this.upgrade() else {
                return;
            };
            let Some(next) = bar.read(cx).neighbour(delta, cx) else {
                return;
            };
            let Some(window) = cx.active_window() else {
                return;
            };
            window
                .update(cx, |_, window, cx| {
                    bar.update(cx, |bar, cx| bar.open(next, true, window, cx))
                })
                .ok();
        }));
        let bar = cx.entity().downgrade();
        cx.intercept_keystrokes(move |event, window, cx| {
            bar.update(cx, |bar, cx| bar.intercept(&event.keystroke, window, cx))
                .ok();
        })
        .detach();
        Self {
            focused: None,
            quiet: None,
            alt_alone: false,
            buttons: Vec::new(),
        }
    }

    fn menus(cx: &App) -> Vec<OwnedMenu> {
        cx.get_menus().unwrap_or_default()
    }

    /// Alt+letter, and the keys of a focused bar.
    fn intercept(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        self.alt_alone = false;
        // Windows: no app menu during the welcome flow
        #[cfg(windows)]
        if title_bar_mode(false, window, cx) == crate::title_bar_windows::Mode::Light {
            return;
        }
        match views_menu::last_key() {
            views_menu::LastKey::Activated => {
                // Chromium leaves the menu bar once an item runs
                self.unfocus(cx);
                return;
            }
            views_menu::LastKey::Consumed => return,
            views_menu::LastKey::Passed => {}
        }
        if views_menu::is_open(cx) {
            return;
        }
        let m = &keystroke.modifiers;
        let letter = keystroke
            .key
            .chars()
            .next()
            .filter(|c| keystroke.key.chars().count() == 1 && c.is_alphanumeric())
            .map(|c| c.to_ascii_lowercase());
        let by_mnemonic = |bar: &Self| {
            letter.and_then(|l| {
                bar.buttons
                    .iter()
                    .position(|b| views_menu::mnemonic_char(&b.text, b.mnemonic) == Some(l))
            })
        };
        if m.alt && !m.control && !m.platform {
            if let Some(index) = by_mnemonic(self) {
                self.open(index, true, window, cx);
                cx.stop_propagation();
            }
            return;
        }
        let Some(focused) = self.focused else {
            return;
        };
        if m.control || m.platform {
            self.unfocus(cx);
            return;
        }
        let count = self.buttons.len();
        match keystroke.key.as_str() {
            "left" if count > 0 => self.focused = Some((focused + count - 1) % count),
            "right" if count > 0 => self.focused = Some((focused + 1) % count),
            "down" | "up" | "enter" | "space" => self.open(focused, true, window, cx),
            "escape" => self.focused = None,
            _ => match by_mnemonic(self) {
                Some(index) => self.open(index, true, window, cx),
                None => self.focused = None,
            },
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn unfocus(&mut self, cx: &mut Context<Self>) {
        if self.focused.take().is_some() {
            cx.notify();
        }
    }

    /// ← / → with a menu open: the neighbouring menu's index.
    fn neighbour(&self, delta: isize, cx: &App) -> Option<usize> {
        let open = views_menu::open_menu_bar_index(cx)?;
        let count = self.buttons.len() as isize;
        (count > 0).then(|| (open as isize + delta).rem_euclid(count) as usize)
    }

    /// Open menu `index` under its button.
    fn open(&mut self, index: usize, keyboard: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = Self::menus(cx).into_iter().nth(index) else {
            return;
        };
        let Some(button) = self.buttons.get(index) else {
            return;
        };
        let entries = entries(&menu.items, window, cx);
        let origin = bar_origin();
        let rect = Bounds::new(
            point(origin.x + px(button.x), origin.y),
            size(px(button.width), px(OPEN_HEIGHT)),
        );
        // the bar keeps the keyboard position while its menu is open
        self.focused = keyboard.then_some(index);
        self.quiet = None;
        views_menu::take_escaped();
        let bar = cx.entity().downgrade();
        let on_close: views_menu::OnClose = Rc::new(move |cx| {
            let escaped = views_menu::take_escaped();
            bar.update(cx, |bar, cx| {
                if cfg!(windows) && escaped {
                    bar.focused = Some(index);
                    bar.quiet = (!keyboard).then_some(index);
                }
                cx.notify()
            })
            .ok();
        });
        views_menu::open(
            entries,
            Anchor::below(rect),
            Source::MenuBar(index),
            keyboard,
            Some(on_close),
            window,
            cx,
        );
        cx.notify();
    }

    /// The button under the window coordinate `x`.
    fn button_at(&self, x: f32) -> Option<usize> {
        let x = x - f32::from(bar_origin().x);
        self.buttons
            .iter()
            .position(|b| x >= b.x && x < b.x + b.width)
    }
}

/// The bar's top-left corner in the window: the window's own, except on
/// Android, where the shell keeps clear of the system bars.
fn bar_origin() -> Point<Pixels> {
    let area = crate::theme::safe_area();
    // Windows: the title bar's app icon comes first
    #[cfg(windows)]
    let area = {
        let mut area = area;
        area.left += px(crate::title_bar_windows::MENU_BAR_LEFT);
        area
    };
    point(area.left, area.top)
}

/// The views menu rows for a GPUI menu's items.
fn entries(items: &[OwnedMenuItem], window: &mut Window, cx: &mut App) -> Vec<Entry> {
    let state = corvene_core::AppState::try_global(cx).map(|s| {
        let state = s.read(cx);
        corvene_core::menu_state::menu_state(state)
    });
    let mut out: Vec<Entry> = Vec::new();
    for item in items {
        match item {
            OwnedMenuItem::Separator => {
                // no leading, trailing or doubled separators (hidden items)
                if out
                    .last()
                    .is_some_and(|e| !matches!(e.kind, EntryKind::Separator))
                {
                    out.push(Entry::separator());
                }
            }
            OwnedMenuItem::Submenu(menu) => {
                let (text, mnemonic) = views_menu::parse_mnemonic(&menu.name);
                out.push(Entry {
                    text: text.into(),
                    mnemonic,
                    accelerator: None,
                    enabled: !menu.disabled,
                    checked: None,
                    icon: None,
                    kind: EntryKind::Submenu(entries(&menu.items, window, cx)),
                });
            }
            OwnedMenuItem::SystemMenu(_) => {}
            OwnedMenuItem::Action {
                name,
                action,
                os_action,
                checked,
                disabled,
            } => {
                let (text, mnemonic) = views_menu::parse_mnemonic(name);
                let available =
                    window.is_action_available(&**action, cx) || cx.is_action_available(&**action);
                let ghd_enabled = match (menu_id(&**action), state.as_ref()) {
                    (Some(id), Some(state)) => state.get(&id).copied().unwrap_or(true),
                    _ => true,
                };
                let accelerator = window
                    .bindings_for_action(&**action)
                    .last()
                    .and_then(|binding| binding.keystrokes().first().cloned())
                    .map(|k| views_menu::accelerator_text(k.inner()))
                    .or_else(|| os_action.map(|a| role_accelerator(a).to_string()));
                let action = action.boxed_clone();
                out.push(Entry {
                    text: text.into(),
                    mnemonic,
                    accelerator: accelerator.map(Into::into),
                    enabled: !disabled && available && ghd_enabled,
                    checked: checked.then_some(true),
                    icon: None,
                    kind: EntryKind::Action(Rc::new(move |window, cx| {
                        window.dispatch_action(action.boxed_clone(), cx);
                    })),
                });
            }
        }
    }
    while out
        .last()
        .is_some_and(|e| matches!(e.kind, EntryKind::Separator))
    {
        out.pop();
    }
    out
}

impl Render for MenuBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Palette::for_view(window, cx);
        let font = Font {
            family: views_menu::font_family(),
            ..Font::default()
        };
        let font_size = px(views_menu::metrics::BAR_FONT_SIZE);
        let mut x = 0.;
        self.buttons = Self::menus(cx)
            .iter()
            .map(|menu| {
                let (text, mnemonic) = views_menu::parse_mnemonic(&menu.name);
                let run = TextRun {
                    len: text.len(),
                    font: font.clone(),
                    color: palette.bar_text,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let text_width = f32::from(
                    window
                        .text_system()
                        .shape_line(SharedString::from(text.clone()), font_size, &[run], None)
                        .width,
                );
                // (Windows: the buttons are HTML there, as wide as their text)
                let width = text_width + 2. * BUTTON_PADDING;
                let width = if cfg!(windows) { width } else { width.floor() };
                let button = Button {
                    text: text.into(),
                    mnemonic,
                    x,
                    width,
                };
                x += width;
                button
            })
            .collect();
        let open = views_menu::open_menu_bar_index(cx);
        let quiet = self.focused.is_some() && self.focused == self.quiet;
        let underline = self.alt_alone
            || (self.focused.is_some() && !quiet)
            || views_menu::opened_from_keyboard(cx);
        let buttons = self.buttons.iter().enumerate().map(|(ix, b)| {
            let is_open = open == Some(ix);
            let hot = !is_open && self.focused == Some(ix);
            let label = match b.mnemonic.filter(|_| underline) {
                Some(at) => {
                    views_menu::mnemonic_text(&b.text, at, palette.bar_text).into_any_element()
                }
                None => div().child(b.text.clone()).into_any_element(),
            };
            div()
                .absolute()
                .left(px(b.x))
                .top_0()
                .w(px(b.width))
                .h(px(HEIGHT))
                // Windows: `.toolbar-button > button:hover`
                .when(cfg!(windows) && !is_open && !hot, |d| {
                    d.hover(|style| style.bg(palette.bar_hot))
                })
                .when(is_open, |d| {
                    d.child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .w_full()
                            .h(px(OPEN_HEIGHT))
                            .rounded(px(1.))
                            .bg(palette.bar_open),
                    )
                })
                // Windows: Chromium's `outline: auto` ring, 2 px inside the
                // button (white in a black halo, the title bar being dark)
                .when(cfg!(windows) && hot, |d| {
                    d.bg(palette.bar_hot).child(
                        div()
                            .absolute()
                            .top(px(2.))
                            .left(px(2.))
                            .right(px(2.))
                            .bottom(px(3.))
                            .border_1()
                            .border_color(rgb(0x101010))
                            .rounded(px(4.))
                            .child(
                                div()
                                    .size_full()
                                    .border_2()
                                    .border_color(rgb(0xffffff))
                                    .rounded(px(3.)),
                            ),
                    )
                })
                .when(!cfg!(windows) && hot, |d| {
                    d.bg(palette.bar_hot).child(
                        // Chromium's focus ring: only its right edge shows
                        // past the button
                        div()
                            .absolute()
                            .top_0()
                            .left(px(b.width + 2.))
                            .w(px(2.))
                            .h(px(HEIGHT))
                            .bg(palette.focus_ring),
                    )
                })
                .child(
                    div()
                        .absolute()
                        .left(px(BUTTON_PADDING))
                        .top_0()
                        .h(px(HEIGHT))
                        .flex()
                        .items_center()
                        // Windows: the open button takes the pane's colours
                        .when(cfg!(windows) && is_open, |d| d.text_color(palette.text))
                        .child(label),
                )
        });
        let bar = div()
            .id("menu-bar")
            .relative()
            .w_full()
            .h(px(HEIGHT))
            .flex_none()
            .when(!cfg!(windows), |d| d.bg(palette.bar_background))
            .text_color(palette.bar_text)
            .font_family(views_menu::font_family())
            .text_size(font_size)
            .line_height(px(20.))
            .on_modifiers_changed(cx.listener(|this, event: &ModifiersChangedEvent, _, cx| {
                let alt_only = event.modifiers.alt
                    && !event.modifiers.control
                    && !event.modifiers.shift
                    && !event.modifiers.platform;
                if alt_only {
                    if !this.alt_alone {
                        this.alt_alone = true;
                        cx.notify();
                    }
                } else if this.alt_alone && !event.modifiers.alt {
                    // Alt released on its own: focus the bar, or leave it
                    this.alt_alone = false;
                    if views_menu::is_open(cx) {
                        views_menu::close_all(cx);
                        this.focused = None;
                    } else {
                        this.focused = if this.focused.is_some() {
                            None
                        } else {
                            Some(0)
                        };
                    }
                    cx.notify();
                } else if this.alt_alone {
                    this.alt_alone = false;
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    let Some(index) = this.button_at(f32::from(event.position.x)) else {
                        views_menu::close_all(cx);
                        this.unfocus(cx);
                        return;
                    };
                    if views_menu::open_menu_bar_index(cx) == Some(index) {
                        views_menu::close_all(cx);
                    } else {
                        this.open(index, false, window, cx);
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                // with a menu open, hovering another button opens its menu
                let Some(open) = views_menu::open_menu_bar_index(cx) else {
                    return;
                };
                if let Some(index) = this.button_at(f32::from(event.position.x))
                    && index != open
                {
                    this.open(index, false, window, cx);
                }
            }))
            .children(buttons);
        framed(bar, x, self.focused.is_some() || self.alt_alone, window, cx)
    }
}

/// Windows: how the title bar is shown (`app.tsx#renderTitlebar`): light
/// during the welcome flow; in full screen only while the app menu is in
/// use (`active`: the bar has the keyboard, or a menu is open).
#[cfg(windows)]
fn title_bar_mode(active: bool, window: &Window, cx: &App) -> crate::title_bar_windows::Mode {
    use crate::title_bar_windows::Mode;
    let welcome = corvene_core::AppState::try_global(cx)
        .is_some_and(|state| !state.read(cx).settings.welcome_completed);
    if welcome {
        Mode::Light
    } else if window.is_fullscreen() && !active && !views_menu::is_open(cx) {
        Mode::Hidden
    } else {
        Mode::Dark
    }
}

/// The bar as the window shows it: on Windows inside GitHub Desktop's title
/// bar (`crate::title_bar_windows`), `width` being the buttons' width.
#[cfg(windows)]
fn framed(bar: Stateful<Div>, width: f32, active: bool, window: &Window, cx: &App) -> AnyElement {
    let mode = title_bar_mode(active, window, cx);
    crate::title_bar_windows::title_bar(bar, width, mode, window)
}

#[cfg(not(windows))]
fn framed(
    bar: Stateful<Div>,
    _width: f32,
    _active: bool,
    _window: &Window,
    _cx: &App,
) -> AnyElement {
    bar.into_any_element()
}

/// The main window's root on Linux: the menu bar over the app, as Electron
/// draws its menu bar over the page. Presses outside an open menu close it
/// and go no further (Chromium's menus take the press that dismisses them).
pub struct MenuBarShell {
    menu_bar: Entity<MenuBar>,
    content: AnyView,
}

impl MenuBarShell {
    pub fn new(content: AnyView, cx: &mut Context<Self>) -> Self {
        let menu_bar = cx.new(MenuBar::new);
        // Windows: the title bar's mode and its dimming follow the app's
        // state (the welcome flow ending, a dialog opening)
        #[cfg(windows)]
        if let Some(state) = corvene_core::AppState::try_global(cx) {
            cx.observe(&state, |_, _, cx| cx.notify()).detach();
        }
        Self { menu_bar, content }
    }
}

impl Render for MenuBarShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let menu_bar = self.menu_bar.clone();
        crate::theme::update_safe_area(window);
        // the handles a finger can drag are collected again by this frame
        #[cfg(target_os = "android")]
        corvene_platform::android::clear_drag_handles();
        let shell = div().id("menu-bar-shell").size_full().flex().flex_col();
        // Android: stay clear of the system bars and the keyboard; the bar's
        // colour runs on under the status bar
        #[cfg(target_os = "android")]
        let shell = {
            let area = crate::theme::safe_area();
            shell
                .pt(area.top)
                .pl(area.left)
                .pr(area.right)
                .pb(area.bottom)
                .bg(Palette::for_window(window).bar_background)
        };
        shell
            .capture_any_mouse_down(move |event, _, cx| {
                let y = f32::from(event.position.y - bar_origin().y);
                // a press on a menu drawn in this window (Android) is the
                // menu's
                if views_menu::contains(event.position, cx) {
                    return;
                }
                // the bar handles presses on itself
                if y >= HEIGHT && views_menu::dismiss_on_outside_click(cx) {
                    cx.stop_propagation();
                }
                if y >= HEIGHT {
                    menu_bar.update(cx, |bar, cx| bar.unfocus(cx));
                }
            })
            .map(|shell| self.layout(shell, window, cx))
            .children(Self::menus_in_window(window, cx))
    }
}

impl MenuBarShell {
    /// The bar above the page.
    #[cfg(not(windows))]
    fn layout(&self, shell: Stateful<Div>, _window: &Window, _cx: &App) -> Stateful<Div> {
        shell.child(self.menu_bar.clone()).child(
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .child(self.content.clone()),
        )
    }

    /// Windows: the title bar above the page, or over it (the welcome
    /// flow's light bar; none in full screen), and the page dimmed under an
    /// open app menu (`#foldout-container .overlay`).
    #[cfg(windows)]
    fn layout(&self, shell: Stateful<Div>, window: &Window, cx: &App) -> Stateful<Div> {
        use crate::title_bar_windows::{self, Mode};
        let bar = self.menu_bar.read(cx);
        let mode = title_bar_mode(bar.focused.is_some() || bar.alt_alone, window, cx);
        title_bar_windows::set_mode(mode);
        let page = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(self.content.clone());
        // a dialog's backdrop lies over GHD's whole page, title bar included
        // (drawn here, not by the dialog, so the window controls stay usable)
        let dialog_open = corvene_core::AppState::try_global(cx)
            .is_some_and(|state| state.read(cx).popup().is_some());
        if mode == Mode::Dark {
            use crate::theme::ActiveGhdTheme;
            shell
                .child(self.menu_bar.clone())
                .when(dialog_open, |shell| {
                    shell.child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .w_full()
                            .h(px(title_bar_windows::HEIGHT))
                            .bg(cx.ghd().dialog_backdrop),
                    )
                })
                .child(page)
                .when(views_menu::app_menu_open(cx), |shell| {
                    shell.child(
                        div()
                            .absolute()
                            .top(px(title_bar_windows::HEIGHT))
                            .left_0()
                            .right_0()
                            .bottom_0()
                            // `--overlay-background-color`
                            .bg(cx.ghd().overlay),
                    )
                })
        } else {
            shell.relative().child(page).child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w_full()
                    .child(self.menu_bar.clone()),
            )
        }
    }
}

impl MenuBarShell {
    /// Android: the open menus, drawn over the page (an activity has no
    /// popup windows). Nothing elsewhere.
    fn menus_in_window(window: &Window, cx: &App) -> Vec<AnyElement> {
        #[cfg(any(target_os = "android", windows))]
        {
            views_menu::overlay(bar_origin().y + px(HEIGHT), window, cx)
        }
        #[cfg(not(any(target_os = "android", windows)))]
        {
            let _ = (window, cx);
            Vec::new()
        }
    }
}
