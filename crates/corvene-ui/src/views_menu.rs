//! Chromium views menus as Electron shows them on Linux: the menu bar's
//! dropdowns (`shell/browser/ui/views/menu_bar.cc`) and context menus
//! (`Menu.popup`, GHD `showContextualMenu`). Each level is its own popup
//! window (`WindowKind::AnchoredPopup`: an override-redirect window on X11,
//! an `xdg_popup` on Wayland), so a menu can extend past the main window as
//! Chromium's does.
//!
//! Geometry and colours are Chromium's `MenuConfig` (`menu_config.cc`,
//! `menu_item_view.cc`, `submenu_view.cc`) measured from GitHub Desktop
//! 3.6.6 on Linux (Electron 42, GTK Adwaita): 32 px items (6 px margins
//! around a 20 px line), 17 px separators with the rule at +8, 4 px above
//! and below the items, labels at x 20, accelerators right-aligned 21 px
//! from the edge, Noto Sans 10 pt. Without a compositing window manager
//! Chromium draws the menu square and without a shadow (Xvfb, openbox);
//! Corvene does the same everywhere.
//!
//! Keyboard: the main window keeps the keyboard focus (Chromium grabs it;
//! GPUI's popups on X11 do not), and [`install`]'s keystroke interceptor
//! routes keys to the open menu: ↑ / ↓ / Home / End move, → opens a
//! submenu (or the next menu bar menu), ← closes one (or goes to the
//! previous menu), Enter / Space activate, Escape closes a level, a
//! mnemonic letter activates its item. A click outside the menus, or the
//! main window losing focus, closes them.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::popup::{PopupAnchor, PopupGravity};
#[cfg(not(target_os = "android"))]
use gpui_kit::popup::{PopupConstraintAdjustment, PopupOptions};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::context_menu::{MenuItem, MenuItemKind};

/// Chromium's `MenuConfig` and the measured GitHub Desktop menus.
pub mod metrics {
    /// Menu text (`style::CONTEXT_MENU`, `STYLE_BODY_3`): the 10 pt UI font
    /// one pixel larger (measured: "Select all" is 59 px of ink in GHD).
    pub const FONT_SIZE: f32 = 13.333 + 1.;
    /// The menu bar's buttons use the UI font as is (10 pt at 96 dpi,
    /// rounded to whole pixels).
    #[cfg(not(windows))]
    pub const BAR_FONT_SIZE: f32 = 13.;
    /// Windows: GitHub Desktop's own menu bar, in its `--font-size`.
    #[cfg(windows)]
    pub const BAR_FONT_SIZE: f32 = 12.;
    pub const ITEM_HEIGHT: f32 = 32.;
    pub const SEPARATOR_HEIGHT: f32 = 17.;
    /// Where the separator's 1 px rule sits inside its 17 px.
    pub const SEPARATOR_RULE: f32 = 8.;
    /// Space above the first and below the last item.
    /// [`ITEM_HEIGHT`], lower in a short window (a phone on its side, where
    /// a menu of GHD's is taller than the screen).
    pub fn item_height() -> f32 {
        if crate::theme::short() {
            26.
        } else {
            ITEM_HEIGHT
        }
    }
    /// [`SEPARATOR_HEIGHT`], see [`item_height`].
    pub fn separator_height() -> f32 {
        if crate::theme::short() {
            9.
        } else {
            SEPARATOR_HEIGHT
        }
    }
    pub const VERTICAL_INSET: f32 = 4.;
    pub const LABEL_START: f32 = 20.;
    /// Right of the widest label (fitted with the `&` quirk in `measure`).
    pub const TRAILING: f32 = 20.;
    /// Accelerator column: the widest accelerator plus this (fitted to the
    /// GHD menus: Edit 197 px, Branch 474 px).
    pub const ACCELERATOR_PADDING: f32 = 10.;
    /// Text sits a pixel above the row's centre.
    pub const TEXT_RAISE: f32 = 1.;
    /// From the accelerators' right edge to the menu's.
    pub const ACCELERATOR_RIGHT: f32 = 21.;
    /// A submenu arrow: `arrow_size` + `arrow_to_edge_padding`.
    pub const ARROW_COLUMN: f32 = 24.;
    /// A check mark column in front of the labels of a menu with checkboxes.
    pub const CHECK_COLUMN: f32 = 24.;
    /// A picture in front of the labels (`Entry::icon`) and the column it
    /// takes in a menu that has any.
    pub const ICON_SIZE: f32 = 16.;
    pub const ICON_COLUMN: f32 = 24.;
    /// `MenuConfig::show_delay`: hovering a submenu item opens it after this.
    pub const SUBMENU_DELAY_MS: u64 = 400;
}

/// The measurements a menu is laid out with: Chromium's ([`metrics`]) or,
/// for the app menu on Windows, GitHub Desktop's own (`_app-menu.scss`):
/// the app menu is part of its page there, not a Chromium menu.
#[derive(Clone, Copy, Debug)]
struct Style {
    font_size: f32,
    item_height: f32,
    separator_height: f32,
    separator_rule: f32,
    top_inset: f32,
    bottom_inset: f32,
    label_start: f32,
    trailing: f32,
    accelerator_padding: f32,
    text_raise: f32,
    accelerator_right: f32,
    arrow_column: f32,
    check_column: f32,
    /// From the submenu arrow's 16 px box to the menu's right edge.
    arrow_right: f32,
    submenu_delay_ms: u64,
    /// GitHub Desktop's app menu panes rather than a Chromium menu.
    app_menu: bool,
}

impl Style {
    const CHROMIUM: Style = Style {
        font_size: metrics::FONT_SIZE,
        item_height: metrics::ITEM_HEIGHT,
        separator_height: metrics::SEPARATOR_HEIGHT,
        separator_rule: metrics::SEPARATOR_RULE,
        top_inset: metrics::VERTICAL_INSET,
        bottom_inset: metrics::VERTICAL_INSET,
        label_start: metrics::LABEL_START,
        trailing: metrics::TRAILING,
        accelerator_padding: metrics::ACCELERATOR_PADDING,
        text_raise: metrics::TEXT_RAISE,
        accelerator_right: metrics::ACCELERATOR_RIGHT,
        arrow_column: metrics::ARROW_COLUMN,
        check_column: metrics::CHECK_COLUMN,
        arrow_right: 8.,
        submenu_delay_ms: metrics::SUBMENU_DELAY_MS,
        app_menu: false,
    };

    /// Chromium's menus on Windows (the context menus), measured from
    /// GitHub Desktop 3.6.6 at 150 %: Segoe UI at 9 pt, 28 px items between
    /// 17 px separators, 12 px above the first and below the last item, the
    /// submenu arrow's ink ending 24 px from the edge.
    #[cfg(windows)]
    const CHROMIUM_WINDOWS: Style = Style {
        font_size: 12.,
        item_height: 28.,
        top_inset: 12.,
        bottom_inset: 12.,
        text_raise: 0.,
        arrow_right: 23.,
        ..Self::CHROMIUM
    };

    /// `.menu-pane` / `.menu-item` (`_app-menu.scss`): 30 px items in the
    /// app's 12 px font, the label `--spacing-double` in and `--spacing`
    /// before the accelerator, which ends `--spacing` from the edge; an
    /// `hr` is 1 px between the browser's 0.5 em margins; the pane has
    /// `--spacing-half` below its last item. `expandCollapseTimeout` is
    /// 300 ms (`app-menu.tsx`).
    #[cfg(windows)]
    const APP_MENU: Style = Style {
        font_size: 12.,
        item_height: 30.,
        separator_height: 13.,
        separator_rule: 6.,
        top_inset: 0.,
        bottom_inset: 5.,
        label_start: 20.,
        trailing: 10.,
        accelerator_padding: 10.,
        text_raise: 0.,
        accelerator_right: 10.,
        arrow_column: 12.,
        check_column: 0.,
        arrow_right: 10.,
        submenu_delay_ms: 300,
        app_menu: true,
    };

    /// The style of the open session (one at a time).
    fn current() -> Style {
        #[cfg(windows)]
        {
            if APP_MENU_SESSION.get() {
                Self::APP_MENU
            } else {
                Self::CHROMIUM_WINDOWS
            }
        }
        // (lower rows in a short window: `metrics::item_height`)
        #[cfg(not(windows))]
        {
            let separator_height = metrics::separator_height();
            Style {
                item_height: metrics::item_height(),
                separator_height,
                separator_rule: (separator_height / 2.).floor(),
                ..Self::CHROMIUM
            }
        }
    }

    /// How far below its parent's top a submenu starts: Chromium lines its
    /// first item up with the parent row (at `row_top`); GitHub Desktop's
    /// panes stand side by side.
    #[cfg(any(target_os = "android", windows))]
    fn submenu_offset(self, row_top: f32) -> f32 {
        if self.app_menu {
            0.
        } else {
            row_top - self.top_inset
        }
    }
}

#[cfg(windows)]
thread_local! {
    /// Whether the open session is the app menu ([`Style::current`]).
    static APP_MENU_SESSION: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the open menu is GitHub Desktop's app menu on Windows, under
/// which the page is dimmed (`#foldout-container .overlay`).
#[cfg(windows)]
pub fn app_menu_open(cx: &App) -> bool {
    is_open(cx) && APP_MENU_SESSION.get()
}

/// The GTK theme's menu colours Chromium draws with (Adwaita).
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub background: Hsla,
    pub text: Hsla,
    pub accelerator: Hsla,
    pub disabled: Hsla,
    pub separator: Hsla,
    pub highlight: Hsla,
    pub highlight_text: Hsla,
    /// The menu bar (`MenuBar`, `SubmenuButton`).
    pub bar_background: Hsla,
    pub bar_text: Hsla,
    /// A bar button whose menu is open.
    pub bar_open: Hsla,
    /// The keyboard-focused bar button.
    pub bar_hot: Hsla,
    pub focus_ring: Hsla,
}

impl Palette {
    /// Adwaita (light), measured from GitHub Desktop 3.6.6 under Xvfb.
    pub fn light() -> Self {
        Self {
            background: rgb(0xffffff).into(),
            text: rgb(0x2e3436).into(),
            accelerator: rgb(0x8c9091).into(),
            disabled: rgb(0x929595).into(),
            separator: rgb(0xe6e6e6).into(),
            highlight: rgb(0x3584e4).into(),
            highlight_text: rgb(0xffffff).into(),
            bar_background: rgb(0xf6f5f4).into(),
            bar_text: rgb(0x2e3436).into(),
            bar_open: rgb(0xd7d7d6).into(),
            bar_hot: rgb(0xe8e7e6).into(),
            focus_ring: rgb(0x0b57d0).into(),
        }
    }

    /// Adwaita dark, for a desktop that prefers dark (Chromium follows the
    /// GTK theme, not GitHub Desktop's own theme).
    pub fn dark() -> Self {
        Self {
            background: rgb(0x2b2b2b).into(),
            text: rgb(0xeeeeec).into(),
            accelerator: rgb(0x9c9c9a).into(),
            disabled: rgb(0x919190).into(),
            separator: rgb(0x3e3e3e).into(),
            highlight: rgb(0x3584e4).into(),
            highlight_text: rgb(0xffffff).into(),
            bar_background: rgb(0x303030).into(),
            bar_text: rgb(0xeeeeec).into(),
            bar_open: rgb(0x4a4a4a).into(),
            bar_hot: rgb(0x3d3d3d).into(),
            focus_ring: rgb(0x7ab4ff).into(),
        }
    }

    /// GitHub Desktop's app menu on Windows (`_app-menu.scss`,
    /// `_app-menu-bar.scss`): the bar's buttons are toolbar buttons on the
    /// title bar, the panes take the app's background and text colours and
    /// `--box-selected-active-*` for the selected item.
    /// The colours of Corvene's theme (not the desktop's, which Chromium's
    /// own menus follow on Linux).
    #[cfg(windows)]
    pub fn for_view(window: &Window, cx: &App) -> Self {
        if Style::current().app_menu || !is_open(cx) {
            return Self::for_view_app(cx);
        }
        // a context menu is Chromium's, light or dark as the app's theme is
        // (GitHub Desktop sets Electron's `nativeTheme.themeSource`), in the
        // colours measured from GitHub Desktop 3.6.6: a grey highlight, not
        // the accent colour
        let _ = window;
        let dark = Self::for_view_app(cx).background.l < 0.5;
        let (background, text, minor, disabled, separator, highlight) = if dark {
            (0x1f1f1f, 0xe3e3e3, 0x9aa0a6, 0x9aa0a6, 0x5e5e5e, 0x363636)
        } else {
            (0xffffff, 0x1f1f1f, 0x5f6368, 0x5f6368, 0xd3e3fd, 0xf2f2f2)
        };
        Self {
            background: rgb(background).into(),
            text: rgb(text).into(),
            accelerator: rgb(minor).into(),
            disabled: rgb(disabled).into(),
            separator: rgb(separator).into(),
            highlight: rgb(highlight).into(),
            highlight_text: rgb(text).into(),
            ..Self::for_view_app(cx)
        }
    }

    /// The app menu's palette: Corvene's theme.
    #[cfg(windows)]
    pub fn for_view_app(cx: &App) -> Self {
        use crate::theme::ActiveGhdTheme;
        let theme = cx.ghd();
        // measured from GitHub Desktop 3.6.6: the pane and the open button
        // are the page's colour in the light theme and `$gray-800` in the
        // dark one, where the separators are `$gray-600`
        let (pane, separator): (Hsla, Hsla) = if theme.background.l < 0.5 {
            (rgb(0x2f363d).into(), rgb(0x586069).into())
        } else {
            (theme.toolbar_button_active_background, theme.box_border)
        };
        Self {
            background: pane,
            text: theme.text,
            accelerator: theme.text_secondary,
            // `.menu-item.disabled { opacity: 0.3 }`
            disabled: theme.text.opacity(0.3),
            separator,
            highlight: theme.box_selected_active_background,
            highlight_text: theme.box_selected_active_text,
            bar_background: rgb(0x24292e).into(),
            bar_text: rgb(0xffffff).into(),
            bar_open: pane,
            bar_hot: rgb(0x2f363d).into(),
            focus_ring: transparent_black(),
        }
    }

    /// The palette a menu or the menu bar draws with.
    #[cfg(not(windows))]
    pub fn for_view(window: &Window, _cx: &App) -> Self {
        Self::for_window(window)
    }

    pub fn for_window(window: &Window) -> Self {
        match window.appearance() {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
            _ => Self::light(),
        }
    }
}

/// The font Chromium's menus use: the desktop UI font (fontconfig
/// `sans-serif`, Noto Sans on Ubuntu).
pub fn font_family() -> SharedString {
    crate::theme::ui_font()
}

/// `&File` → ("File", Some(0)); `&&` is a literal ampersand.
pub fn parse_mnemonic(label: &str) -> (String, Option<usize>) {
    let mut text = String::with_capacity(label.len());
    let mut mnemonic = None;
    let mut chars = label.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '&' {
            match chars.peek() {
                Some('&') => {
                    chars.next();
                    text.push('&');
                }
                Some(_) if mnemonic.is_none() => mnemonic = Some(text.len()),
                _ => {}
            }
        } else {
            text.push(c);
        }
    }
    (text, mnemonic)
}

/// The mnemonic's character, lowercased.
pub fn mnemonic_char(text: &str, mnemonic: Option<usize>) -> Option<char> {
    text.get(mnemonic?..)?
        .chars()
        .next()
        .map(|c| c.to_ascii_lowercase())
}

pub type EntryAction = Rc<dyn Fn(&mut Window, &mut App)>;
/// Runs when a menu session ends.
pub type OnClose = Rc<dyn Fn(&mut App)>;

#[derive(Clone)]
pub enum EntryKind {
    Action(EntryAction),
    Submenu(Vec<Entry>),
    Separator,
}

/// One row of a views menu.
#[derive(Clone)]
pub struct Entry {
    pub text: SharedString,
    pub mnemonic: Option<usize>,
    pub accelerator: Option<SharedString>,
    pub enabled: bool,
    pub checked: Option<bool>,
    /// A picture in front of the label (`MenuItem::icon`).
    pub icon: Option<std::sync::Arc<Image>>,
    pub kind: EntryKind,
}

impl Entry {
    pub fn separator() -> Self {
        Self {
            text: SharedString::default(),
            mnemonic: None,
            accelerator: None,
            enabled: false,
            checked: None,
            icon: None,
            kind: EntryKind::Separator,
        }
    }

    fn is_separator(&self) -> bool {
        matches!(self.kind, EntryKind::Separator)
    }

    fn selectable(&self) -> bool {
        self.enabled && !self.is_separator()
    }

    /// A context menu item (`context_menu::MenuItem`); its label may carry
    /// an `&` mnemonic.
    pub fn from_menu_item(item: &MenuItem) -> Self {
        let (text, mnemonic) = parse_mnemonic(&item.label);
        Self {
            text: text.into(),
            mnemonic,
            accelerator: None,
            enabled: item.enabled,
            checked: item.checked,
            icon: item.icon.clone(),
            kind: match &item.kind {
                MenuItemKind::Separator => EntryKind::Separator,
                MenuItemKind::Action(action) => EntryKind::Action(action.clone()),
                MenuItemKind::Submenu(items) => {
                    EntryKind::Submenu(items.iter().map(Self::from_menu_item).collect())
                }
            },
        }
    }
}

/// Where a session came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Context,
    /// The menu bar's menu at this index.
    MenuBar(usize),
}

struct Level {
    entries: Vec<Entry>,
    highlighted: Option<usize>,
    #[cfg(not(any(target_os = "android", windows)))]
    window: WindowHandle<MenuLevelView>,
    /// Android: an activity has one window, so a level is a view drawn over
    /// the page ([`overlay`]) at `bounds` (window coordinates).
    #[cfg(any(target_os = "android", windows))]
    view: Entity<MenuLevelView>,
    #[cfg(any(target_os = "android", windows))]
    bounds: Bounds<Pixels>,
    /// Windows: a context menu's level is a window of its own showing
    /// `view` (Chromium's menus reach past the window they belong to), and
    /// `bounds` is that window's; the app menu's levels have none, they are
    /// part of GitHub Desktop's page.
    #[cfg(windows)]
    popup: Option<WindowHandle<MenuLevelView>>,
    /// Top of each row inside the menu.
    row_tops: Vec<f32>,
}

/// The open menus. One session at a time, like Chromium's `MenuController`.
struct Session {
    owner: AnyWindowHandle,
    source: Source,
    levels: Vec<Level>,
    /// Opened from the keyboard: the first item starts highlighted.
    keyboard: bool,
    submenu_timer: Option<Task<()>>,
    _auto_dismiss: Option<Task<()>>,
}

#[derive(Default)]
struct Menus {
    session: Option<Session>,
    /// Called when the session ends (the menu bar redraws its buttons).
    on_close: Option<OnClose>,
}

impl Global for Menus {}

/// Register the keystroke interceptor that drives open menus from the
/// keyboard. Once, at startup.
pub fn install(cx: &mut App) {
    cx.set_global(Menus::default());
    cx.intercept_keystrokes(|event, _window, cx| {
        LAST_KEY.with(|k| *k.borrow_mut() = LastKey::Passed);
        let consumed = is_open(cx) && handle_key(&event.keystroke, cx);
        LAST_KEY.with(|k| {
            let mut k = k.borrow_mut();
            // `activate` sets `Activated` while the key is handled
            if consumed && *k == LastKey::Passed {
                *k = LastKey::Consumed;
            }
        });
        if consumed {
            cx.stop_propagation();
        }
    })
    .detach();
}

/// What the open menu did with the latest keystroke; the menu bar's own
/// interceptor runs after this one and must not act on it again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LastKey {
    Passed,
    Consumed,
    /// The key ran an item (the menus are closed).
    Activated,
}

thread_local! {
    static LAST_KEY: std::cell::RefCell<LastKey> = const { std::cell::RefCell::new(LastKey::Passed) };
}

/// What the menus did with the keystroke being dispatched.
pub fn last_key() -> LastKey {
    LAST_KEY.with(|k| *k.borrow())
}

/// Whether any menu is open.
pub fn is_open(cx: &App) -> bool {
    cx.try_global::<Menus>()
        .is_some_and(|m| m.session.is_some())
}

/// The menu bar menu that is open, if any.
pub fn open_menu_bar_index(cx: &App) -> Option<usize> {
    let menus = cx.try_global::<Menus>()?;
    match menus.session.as_ref()?.source {
        Source::MenuBar(index) => Some(index),
        Source::Context => None,
    }
}

/// Whether the menus were opened (or are driven) from the keyboard.
pub fn opened_from_keyboard(cx: &App) -> bool {
    cx.try_global::<Menus>()
        .and_then(|m| m.session.as_ref())
        .is_some_and(|s| s.keyboard)
}

/// Where a menu opens, in the owner window's coordinates.
#[derive(Clone, Copy, Debug)]
pub struct Anchor {
    pub rect: Bounds<Pixels>,
    pub anchor: PopupAnchor,
    pub gravity: PopupGravity,
}

impl Anchor {
    /// A context menu: its top-left corner at the pointer.
    pub fn point(position: Point<Pixels>) -> Self {
        Self {
            rect: Bounds::new(position, size(px(0.), px(0.))),
            anchor: PopupAnchor::TopLeft,
            gravity: PopupGravity::BottomRight,
        }
    }

    /// A dropdown under a menu bar button.
    pub fn below(rect: Bounds<Pixels>) -> Self {
        Self {
            rect,
            anchor: PopupAnchor::BottomLeft,
            gravity: PopupGravity::BottomRight,
        }
    }
}

/// Open `entries` as a new session (closing any other), owned by `window`.
/// `on_close` runs when the session ends.
pub fn open(
    entries: Vec<Entry>,
    anchor: Anchor,
    source: Source,
    keyboard: bool,
    on_close: Option<OnClose>,
    window: &mut Window,
    cx: &mut App,
) {
    close_all(cx);
    #[cfg(windows)]
    APP_MENU_SESSION.set(matches!(source, Source::MenuBar(_)));
    let owner = window.window_handle();
    let Some(level) = open_level(entries, anchor, owner, keyboard, window, cx) else {
        return;
    };
    // (Windows: the app menu is part of GitHub Desktop's page, which a
    // parity scenario opens, captures and closes itself; only the context
    // menus are closed for it)
    let hold = crate::native_menu_common::auto_dismiss()
        .filter(|_| !cfg!(windows) || matches!(source, Source::Context));
    let auto_dismiss = hold.map(|hold| {
        cx.spawn(async move |cx| {
            cx.background_executor().timer(hold).await;
            cx.update(close_all);
        })
    });
    let menus = cx.global_mut::<Menus>();
    menus.on_close = on_close;
    menus.session = Some(Session {
        owner,
        source,
        levels: vec![level],
        keyboard,
        submenu_timer: None,
        _auto_dismiss: auto_dismiss,
    });
    // the level's first frame ran before its entries were stored
    refresh_levels(cx);
}

/// Row tops and the menu's size for `entries`, measured in `window`.
fn measure(entries: &[Entry], window: &Window) -> (Vec<f32>, Size<Pixels>) {
    let s = Style::current();
    let font = Font {
        family: font_family(),
        ..Font::default()
    };
    let width_of = |text: &str| -> f32 {
        if text.is_empty() {
            return 0.;
        }
        let run = TextRun {
            len: text.len(),
            font: font.clone(),
            color: black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        f32::from(
            window
                .text_system()
                .shape_line(
                    SharedString::from(text.to_string()),
                    px(s.font_size),
                    &[run],
                    None,
                )
                .width,
        )
    };
    let has_check = entries.iter().any(|e| e.checked.is_some());
    let mut label_max = 0f32;
    let mut minor_max = 0f32;
    let mut row_max = 0f32;
    let mut row_tops = Vec::with_capacity(entries.len());
    let mut y = s.top_inset;
    for entry in entries {
        row_tops.push(y);
        if entry.is_separator() {
            y += s.separator_height;
            continue;
        }
        y += s.item_height;
        // Chromium measures `title_` with its `&` prefix still in it
        // (`MenuItemView::CalculateDimensions`; it is only stripped when
        // drawn), so a label with a mnemonic reserves an ampersand more
        let ampersand = if entry.mnemonic.is_some() && !s.app_menu {
            width_of("&")
        } else {
            0.
        };
        label_max = label_max.max(width_of(&entry.text) + ampersand);
        let accelerator = entry
            .accelerator
            .as_ref()
            .map(|a| width_of(a))
            .unwrap_or(0.);
        let arrow = if matches!(entry.kind, EntryKind::Submenu(_)) {
            s.arrow_column
        } else {
            0.
        };
        minor_max = minor_max.max(accelerator + arrow);
        // GitHub Desktop's app menu rows are flex boxes: a pane is as wide
        // as its widest row, not as its widest label plus widest accelerator
        let minor = accelerator + arrow;
        row_max = row_max.max(
            width_of(&entry.text)
                + if minor > 0. {
                    minor + s.accelerator_padding
                } else {
                    0.
                },
        );
    }
    if s.app_menu {
        let width = (s.label_start + row_max + s.trailing).ceil();
        return (row_tops, size(px(width), px(y + s.bottom_inset)));
    }
    let has_icon = entries.iter().any(|e| e.icon.is_some());
    let label_start = s.label_start
        + if has_check { s.check_column } else { 0. }
        + if has_icon { metrics::ICON_COLUMN } else { 0. };
    let minor = if minor_max > 0. {
        minor_max + s.accelerator_padding
    } else {
        0.
    };
    let width = (label_start + label_max.ceil() + s.trailing + minor.ceil()).ceil();
    (row_tops, size(px(width), px(y + s.bottom_inset)))
}

fn open_level(
    entries: Vec<Entry>,
    anchor: Anchor,
    owner: AnyWindowHandle,
    keyboard: bool,
    window: &mut Window,
    cx: &mut App,
) -> Option<Level> {
    let (row_tops, menu_size) = measure(&entries, window);
    let level_index = cx
        .try_global::<Menus>()
        .and_then(|m| m.session.as_ref())
        .map_or(0, |s| s.levels.len());
    // (GitHub Desktop's app menu selects a pane's first item however the
    // menu was opened; a submenu's only from the keyboard)
    let first = keyboard || (cfg!(windows) && Style::current().app_menu && level_index == 0);
    let highlighted = first
        .then(|| entries.iter().position(Entry::selectable))
        .flatten();
    #[cfg(any(target_os = "android", windows))]
    {
        let _ = owner;
        #[cfg(windows)]
        if !Style::current().app_menu {
            let view = cx.new(|_| MenuLevelView {
                level: level_index,
                scroll: ScrollHandle::new(),
            });
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    menu_size,
                ))),
                titlebar: None,
                focus: false,
                show: true,
                kind: WindowKind::AnchoredPopup(PopupOptions {
                    parent: owner,
                    anchor_rect: anchor.rect,
                    anchor: anchor.anchor,
                    gravity: anchor.gravity,
                    constraint_adjustment: PopupConstraintAdjustment::FLIP_X
                        | PopupConstraintAdjustment::FLIP_Y
                        | PopupConstraintAdjustment::SLIDE_X
                        | PopupConstraintAdjustment::SLIDE_Y,
                    offset: point(px(0.), px(0.)),
                    grab: false,
                }),
                is_movable: false,
                is_resizable: false,
                is_minimizable: false,
                window_background: WindowBackgroundAppearance::Opaque,
                app_id: Some(corvene_platform::BUNDLE_ID.into()),
                ..Default::default()
            };
            let root = view.clone();
            return match cx.open_window(options, move |_, _| root) {
                Ok(popup) => Some(Level {
                    entries,
                    highlighted,
                    view,
                    bounds: Bounds::new(point(px(0.), px(0.)), menu_size),
                    popup: Some(popup),
                    row_tops,
                }),
                Err(err) => {
                    tracing::warn!(%err, "could not open a menu window");
                    None
                }
            };
        }
        // a menu taller than the screen scrolls
        let area = crate::theme::safe_area();
        let room = window.viewport_size().height - area.top - area.bottom;
        let menu_size = size(menu_size.width, menu_size.height.min(room));
        let bounds = Bounds::new(place(anchor, menu_size, window), menu_size);
        let view = cx.new(|_| MenuLevelView {
            level: level_index,
            scroll: ScrollHandle::new(),
        });
        window.refresh();
        Some(Level {
            entries,
            highlighted,
            view,
            bounds,
            #[cfg(windows)]
            popup: None,
            row_tops,
        })
    }
    #[cfg(not(any(target_os = "android", windows)))]
    {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(0.), px(0.)),
                menu_size,
            ))),
            titlebar: None,
            focus: false,
            show: true,
            kind: WindowKind::AnchoredPopup(PopupOptions {
                parent: owner,
                anchor_rect: anchor.rect,
                anchor: anchor.anchor,
                gravity: anchor.gravity,
                constraint_adjustment: PopupConstraintAdjustment::FLIP_X
                    | PopupConstraintAdjustment::FLIP_Y
                    | PopupConstraintAdjustment::SLIDE_X
                    | PopupConstraintAdjustment::SLIDE_Y,
                offset: point(px(0.), px(0.)),
                grab: false,
            }),
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            window_background: WindowBackgroundAppearance::Opaque,
            app_id: Some(corvene_platform::BUNDLE_ID.into()),
            ..Default::default()
        };
        match cx.open_window(options, |_, cx| {
            cx.new(|_| MenuLevelView {
                level: level_index,
                #[cfg(any(target_os = "android", windows))]
                scroll: ScrollHandle::new(),
            })
        }) {
            Ok(handle) => Some(Level {
                entries,
                highlighted,
                window: handle,
                row_tops,
            }),
            Err(err) => {
                tracing::warn!(%err, "could not open a menu window");
                None
            }
        }
    }
}

/// Android: where a level goes in the window. What the window system does
/// for a popup window on the desktop: under (or beside) its anchor, kept
/// inside the part of the window the system bars leave free.
#[cfg(any(target_os = "android", windows))]
fn place(anchor: Anchor, menu_size: Size<Pixels>, window: &Window) -> Point<Pixels> {
    let area = crate::theme::safe_area();
    let viewport = window.viewport_size();
    let (right, bottom) = (viewport.width - area.right, viewport.height - area.bottom);
    let rect = anchor.rect;
    let beside = matches!(anchor.anchor, PopupAnchor::TopRight);
    let mut origin = match anchor.anchor {
        PopupAnchor::BottomLeft => point(rect.left(), rect.bottom()),
        PopupAnchor::TopRight => point(rect.right(), rect.top()),
        _ => rect.origin,
    };
    if origin.x + menu_size.width > right {
        // a submenu flips to its parent's other side, a menu slides in
        origin.x = if beside {
            rect.left() - menu_size.width
        } else {
            right - menu_size.width
        };
    }
    if origin.y + menu_size.height > bottom {
        origin.y = bottom - menu_size.height;
    }
    point(origin.x.max(area.left), origin.y.max(area.top))
}

/// Android: the open levels as overlays for the window's root
/// (`menu_bar::MenuBarShell`), topmost last.
#[cfg(any(target_os = "android", windows))]
pub fn overlay(bar_bottom: Pixels, window: &Window, cx: &App) -> Vec<AnyElement> {
    let Some(session) = cx.try_global::<Menus>().and_then(|m| m.session.as_ref()) else {
        return Vec::new();
    };
    // Android: a press outside the menus closes them. The shell's own
    // listener does not hear one on a dialog or a foldout (they occlude
    // it), so a sheet under the menus takes it: the whole window, less the
    // menu bar when the menu is one of its own (a press there opens the
    // next menu).
    #[cfg(target_os = "android")]
    let backdrop = {
        let top = match session.source {
            Source::Context => px(0.),
            Source::MenuBar(_) => bar_bottom,
        };
        let viewport = window.viewport_size();
        Some(
            deferred(
                anchored().position(point(px(0.), top)).child(
                    div()
                        .id("views-menu-backdrop")
                        .w(viewport.width)
                        .h(viewport.height - top)
                        .occlude()
                        .child(crate::widgets::touch_drag_occluder())
                        .on_any_mouse_down(|_, _, cx| {
                            cx.stop_propagation();
                            close_all(cx);
                        }),
                ),
            )
            .with_priority(999)
            .into_any_element(),
        )
    };
    #[cfg(not(target_os = "android"))]
    let backdrop = {
        let _ = (bar_bottom, window);
        None
    };
    let levels = session
        .levels
        .iter()
        .enumerate()
        .filter(|(_, level)| in_window(level))
        .map(|(index, level)| {
            deferred(
                anchored()
                    .position(level.bounds.origin)
                    .snap_to_window()
                    .child(
                        div()
                            .w(level.bounds.size.width)
                            .h(level.bounds.size.height)
                            .occlude()
                            .child(crate::widgets::touch_drag_occluder())
                            .map(|pane| {
                                // Windows: the app menu's panes are flat,
                                // each after the first with a divider on its
                                // left
                                #[cfg(windows)]
                                {
                                    pane.when(index > 0, |pane| {
                                        pane.border_l_1()
                                            .border_color(Palette::for_view_app(cx).separator)
                                    })
                                }
                                #[cfg(not(windows))]
                                {
                                    pane.shadow_md()
                                }
                            })
                            .child(level.view.clone()),
                    ),
            )
            .with_priority(1000 + index)
            .into_any_element()
        });
    backdrop.into_iter().chain(levels).collect()
}

/// Whether a level is drawn in its owner's window ([`overlay`]) rather than
/// in a window of its own.
#[cfg(any(target_os = "android", windows))]
fn in_window(level: &Level) -> bool {
    #[cfg(windows)]
    {
        level.popup.is_none()
    }
    #[cfg(not(windows))]
    {
        let _ = level;
        true
    }
}

/// Whether `position` (window coordinates) is on an open menu drawn in the
/// window itself (Android); menus in their own windows never are.
pub fn contains(position: Point<Pixels>, cx: &App) -> bool {
    #[cfg(any(target_os = "android", windows))]
    {
        cx.try_global::<Menus>()
            .and_then(|m| m.session.as_ref())
            .is_some_and(|s| {
                s.levels
                    .iter()
                    .any(|l| in_window(l) && l.bounds.contains(&position))
            })
    }
    #[cfg(not(any(target_os = "android", windows)))]
    {
        let _ = (position, cx);
        false
    }
}

/// Deferred: a click on a menu's row reaches here from inside that window's
/// own update, where updating it again fails and the window would stay open,
/// blank until the next menu draws in it (corvene#20).
fn remove_windows(levels: Vec<Level>, cx: &mut App) {
    #[cfg(not(any(target_os = "android", windows)))]
    cx.defer(move |cx| {
        for level in levels {
            level
                .window
                .update(cx, |_, window, _| window.remove_window())
                .ok();
        }
    });
    #[cfg(any(target_os = "android", windows))]
    {
        #[cfg(windows)]
        {
            let popups: Vec<_> = levels.iter().filter_map(|level| level.popup).collect();
            cx.defer(move |cx| {
                for popup in popups {
                    popup.update(cx, |_, window, _| window.remove_window()).ok();
                }
            });
        }
        drop(levels);
        cx.refresh_windows();
    }
}

thread_local! {
    /// Set when Escape closed the last level.
    static ESCAPED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether Escape closed the menus (once): GitHub Desktop's app menu then
/// leaves the keyboard focus on the menu's button.
pub fn take_escaped() -> bool {
    ESCAPED.with(|escaped| escaped.take())
}

/// Close every open menu.
pub fn close_all(cx: &mut App) {
    let Some(menus) = cx.try_global::<Menus>() else {
        return;
    };
    if menus.session.is_none() {
        return;
    }
    let menus = cx.global_mut::<Menus>();
    let session = menus.session.take();
    let on_close = menus.on_close.take();
    if let Some(session) = session {
        let owner = session.owner;
        remove_windows(session.levels, cx);
        owner.update(cx, |_, window, _| window.refresh()).ok();
    }
    if let Some(on_close) = on_close {
        // the owner (the menu bar) may be mid-update: it opens the next
        // menu, which closes this one
        cx.defer(move |cx| on_close(cx));
    }
}

/// Close the levels past `keep` (a submenu's parent stays).
fn close_after(keep: usize, cx: &mut App) {
    let removed = {
        let Some(session) = cx.global_mut::<Menus>().session.as_mut() else {
            return;
        };
        session.submenu_timer = None;
        if session.levels.len() <= keep + 1 {
            return;
        }
        session.levels.split_off(keep + 1)
    };
    remove_windows(removed, cx);
    refresh_levels(cx);
}

fn refresh_levels(cx: &mut App) {
    #[cfg(any(target_os = "android", windows))]
    cx.refresh_windows();
    #[cfg(not(any(target_os = "android", windows)))]
    refresh_level_windows(cx);
}

#[cfg(not(any(target_os = "android", windows)))]
fn refresh_level_windows(cx: &mut App) {
    let windows: Vec<_> = cx
        .try_global::<Menus>()
        .and_then(|m| m.session.as_ref())
        .map(|s| s.levels.iter().map(|l| l.window).collect())
        .unwrap_or_default();
    for handle in windows {
        handle.update(cx, |_, window, _| window.refresh()).ok();
    }
}

fn set_highlight(level: usize, row: Option<usize>, cx: &mut App) {
    if let Some(l) = cx
        .global_mut::<Menus>()
        .session
        .as_mut()
        .and_then(|s| s.levels.get_mut(level))
    {
        if l.highlighted == row {
            return;
        }
        l.highlighted = row;
    }
    refresh_levels(cx);
}

/// Open the submenu of `row` in `level` (closing deeper ones).
#[cfg(not(any(target_os = "android", windows)))]
fn open_submenu(level: usize, row: usize, keyboard: bool, cx: &mut App) {
    close_after(level, cx);
    let Some((entries, rect, parent_window, owner)) = (|| {
        let session = cx.try_global::<Menus>()?.session.as_ref()?;
        let l = session.levels.get(level)?;
        let EntryKind::Submenu(children) = &l.entries.get(row)?.kind else {
            return None;
        };
        let top = *l.row_tops.get(row)?;
        Some((children.clone(), top, l.window, session.owner))
    })() else {
        return;
    };
    let _ = owner;
    let parent: AnyWindowHandle = parent_window.into();
    let opened = parent_window.update(cx, |_, window, cx| {
        let width = window.bounds().size.width;
        // Chromium lines the submenu's first item up with its parent item
        let anchor = Anchor {
            rect: Bounds::new(
                point(px(0.), px(rect - metrics::VERTICAL_INSET)),
                size(width, px(metrics::item_height())),
            ),
            anchor: PopupAnchor::TopRight,
            gravity: PopupGravity::BottomRight,
        };
        open_level(entries, anchor, parent, keyboard, window, cx)
    });
    if let Ok(Some(level)) = opened
        && let Some(session) = cx.global_mut::<Menus>().session.as_mut()
    {
        session.levels.push(level);
    }
    refresh_levels(cx);
}

/// Android: the submenu beside its row, in the owner window.
#[cfg(any(target_os = "android", windows))]
fn open_submenu(level: usize, row: usize, keyboard: bool, cx: &mut App) {
    close_after(level, cx);
    let Some((entries, anchor, owner)) = (|| {
        let session = cx.try_global::<Menus>()?.session.as_ref()?;
        let l = session.levels.get(level)?;
        let EntryKind::Submenu(children) = &l.entries.get(row)?.kind else {
            return None;
        };
        let top = *l.row_tops.get(row)?;
        // Chromium lines the submenu's first item up with its parent item
        let anchor = Anchor {
            rect: Bounds::new(
                point(
                    l.bounds.origin.x,
                    l.bounds.origin.y
                        + l.view.read(cx).scroll.offset().y
                        + px(Style::current().submenu_offset(top)),
                ),
                size(l.bounds.size.width, px(Style::current().item_height)),
            ),
            anchor: PopupAnchor::TopRight,
            gravity: PopupGravity::BottomRight,
        };
        // Windows: a submenu of a menu window is anchored to that window
        #[cfg(windows)]
        let owner = l.popup.map_or(session.owner, AnyWindowHandle::from);
        #[cfg(not(windows))]
        let owner = session.owner;
        Some((children.clone(), anchor, owner))
    })() else {
        return;
    };
    let opened = owner.update(cx, |_, window, cx| {
        open_level(entries, anchor, owner, keyboard, window, cx)
    });
    if let Ok(Some(level)) = opened
        && let Some(session) = cx.global_mut::<Menus>().session.as_mut()
    {
        session.levels.push(level);
    }
    refresh_levels(cx);
}

/// Run the entry at `row` of `level`: open its submenu or close every menu
/// and run its action in the owner window.
fn activate(level: usize, row: usize, keyboard: bool, cx: &mut App) {
    let Some((entry, owner)) = cx
        .try_global::<Menus>()
        .and_then(|m| m.session.as_ref())
        .and_then(|s| Some((s.levels.get(level)?.entries.get(row)?.clone(), s.owner)))
    else {
        return;
    };
    if !entry.selectable() {
        return;
    }
    match entry.kind {
        #[cfg(not(any(target_os = "android", windows)))]
        EntryKind::Submenu(_) => open_submenu(level, row, keyboard, cx),
        // Android: a tap opens it (there is no hover), from inside the
        // owner window's update, which cannot nest
        #[cfg(any(target_os = "android", windows))]
        EntryKind::Submenu(_) => cx.defer(move |cx| open_submenu(level, row, keyboard, cx)),
        EntryKind::Action(action) => {
            if keyboard {
                LAST_KEY.with(|k| *k.borrow_mut() = LastKey::Activated);
            }
            close_all(cx);
            // deferred: a key reaches here from inside the owner window's
            // update (the keystroke interceptor), which cannot nest
            cx.defer(move |cx| {
                owner.update(cx, |_, window, cx| action(window, cx)).ok();
            });
        }
        EntryKind::Separator => {}
    }
}

/// The next selectable row after (or before) `from`, wrapping around
/// (`arrow_key_selection_wraps`).
fn step(entries: &[Entry], from: Option<usize>, forward: bool) -> Option<usize> {
    let n = entries.len();
    if n == 0 {
        return None;
    }
    let start = match (from, forward) {
        (Some(i), _) => i,
        (None, true) => n - 1,
        (None, false) => 0,
    };
    (1..=n)
        .map(|k| {
            if forward {
                (start + k) % n
            } else {
                (start + n - k % n) % n
            }
        })
        .find(|&i| entries[i].selectable())
}

/// Requests from the keyboard that need the menu bar (switching menus).
pub type SwitchMenu = Rc<dyn Fn(isize, &mut App)>;

thread_local! {
    static SWITCH_MENU: std::cell::RefCell<Option<SwitchMenu>> = const { std::cell::RefCell::new(None) };
}

/// The menu bar's handler for ← / → at the top level.
pub fn set_switch_menu(handler: SwitchMenu) {
    SWITCH_MENU.with(|s| *s.borrow_mut() = Some(handler));
}

/// A key while a menu is open; true when the menu used it.
fn handle_key(keystroke: &Keystroke, cx: &mut App) -> bool {
    let Some((depth, entries, highlighted, source)) = cx
        .try_global::<Menus>()
        .and_then(|m| m.session.as_ref())
        .and_then(|s| {
            let last = s.levels.last()?;
            Some((
                s.levels.len() - 1,
                last.entries.clone(),
                last.highlighted,
                s.source,
            ))
        })
    else {
        return false;
    };
    let m = &keystroke.modifiers;
    if m.control || m.platform || m.function {
        // shortcuts close the menu and go on to the window
        close_all(cx);
        return false;
    }
    if let Some(session) = cx.global_mut::<Menus>().session.as_mut() {
        session.keyboard = true;
    }
    match keystroke.key.as_str() {
        "down" => set_highlight(depth, step(&entries, highlighted, true), cx),
        "up" => set_highlight(depth, step(&entries, highlighted, false), cx),
        "home" => set_highlight(depth, step(&entries, None, true), cx),
        "end" => set_highlight(depth, step(&entries, None, false), cx),
        "right" => match highlighted.map(|i| &entries[i].kind) {
            Some(EntryKind::Submenu(_)) if highlighted.is_some_and(|i| entries[i].enabled) => {
                if let Some(row) = highlighted {
                    open_submenu(depth, row, true, cx);
                }
            }
            _ => {
                if let Source::MenuBar(_) = source {
                    switch_menu(1, cx);
                }
            }
        },
        "left" => {
            if depth > 0 {
                close_after(depth - 1, cx);
            } else if let Source::MenuBar(_) = source {
                switch_menu(-1, cx);
            }
        }
        "enter" | "space" => {
            if let Some(row) = highlighted {
                activate(depth, row, true, cx);
            }
        }
        "escape" => {
            if depth > 0 {
                close_after(depth - 1, cx);
            } else {
                ESCAPED.with(|escaped| escaped.set(true));
                close_all(cx);
            }
        }
        "tab" => {}
        _ => {
            // mnemonics (Chromium: a unique match activates, several cycle)
            let typed = keystroke
                .key_char
                .as_deref()
                .unwrap_or(&keystroke.key)
                .chars()
                .next()
                .map(|c| c.to_ascii_lowercase());
            let Some(typed) = typed.filter(|c| c.is_alphanumeric()) else {
                return true;
            };
            let matches: Vec<usize> = entries
                .iter()
                .enumerate()
                .filter(|(_, e)| {
                    e.selectable() && mnemonic_char(&e.text, e.mnemonic) == Some(typed)
                })
                .map(|(i, _)| i)
                .collect();
            match matches.as_slice() {
                [] => {}
                [only] => activate(depth, *only, true, cx),
                several => {
                    let next = several
                        .iter()
                        .copied()
                        .find(|&i| highlighted.is_none_or(|h| i > h))
                        .unwrap_or(several[0]);
                    set_highlight(depth, Some(next), cx);
                }
            }
        }
    }
    true
}

fn switch_menu(delta: isize, cx: &mut App) {
    let handler = SWITCH_MENU.with(|s| s.borrow().clone());
    if let Some(handler) = handler {
        // the menu bar opens the next menu in its window, which the key's
        // dispatch is still updating
        cx.defer(move |cx| handler(delta, cx));
    }
}

/// The owner window saw a mouse press outside every menu (the menu bar
/// handles its own buttons first).
pub fn dismiss_on_outside_click(cx: &mut App) -> bool {
    if is_open(cx) {
        close_all(cx);
        true
    } else {
        false
    }
}

/// One level's window.
pub struct MenuLevelView {
    level: usize,
    /// Android: a menu taller than the window scrolls.
    #[cfg(any(target_os = "android", windows))]
    scroll: ScrollHandle,
}

impl MenuLevelView {
    fn row_at(&self, y: f32, cx: &App) -> Option<usize> {
        let session = cx.try_global::<Menus>()?.session.as_ref()?;
        let level = session.levels.get(self.level)?;
        // Android: pointer positions are the window's, not the menu's
        #[cfg(any(target_os = "android", windows))]
        let y = y - f32::from(level.bounds.origin.y + self.scroll.offset().y);
        level
            .row_tops
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, top)| {
                let height = if level.entries[i].is_separator() {
                    Style::current().separator_height
                } else {
                    Style::current().item_height
                };
                (y >= *top && y < top + height).then_some(i)
            })
    }

    fn hover(&mut self, y: f32, cx: &mut Context<Self>) {
        let level = self.level;
        let row = self.row_at(y, cx);
        let entry = row.and_then(|r| {
            cx.try_global::<Menus>()?
                .session
                .as_ref()?
                .levels
                .get(level)?
                .entries
                .get(r)
                .cloned()
        });
        let selectable = entry.as_ref().is_some_and(Entry::selectable);
        let current = cx
            .try_global::<Menus>()
            .and_then(|m| m.session.as_ref())
            .and_then(|s| s.levels.get(level))
            .and_then(|l| l.highlighted);
        let target = if selectable { row } else { None };
        if target == current {
            return;
        }
        set_highlight(level, target, cx);
        // a submenu opens after `show_delay`; hovering elsewhere closes the
        // open one after the same delay
        let delay = Duration::from_millis(Style::current().submenu_delay_ms);
        let is_submenu = entry
            .as_ref()
            .is_some_and(|e| selectable && matches!(e.kind, EntryKind::Submenu(_)));
        let task = cx.spawn(async move |_, cx| {
            cx.background_executor().timer(delay).await;
            cx.update(|cx| {
                if is_submenu {
                    if let Some(row) = target {
                        open_submenu(level, row, false, cx);
                    }
                } else {
                    close_after(level, cx);
                }
            });
        });
        if let Some(session) = cx.global_mut::<Menus>().session.as_mut() {
            session.submenu_timer = Some(task);
        }
    }
}

impl Render for MenuLevelView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = Style::current();
        let palette = Palette::for_view(window, cx);
        let Some((entries, highlighted)) = cx
            .try_global::<Menus>()
            .and_then(|m| m.session.as_ref())
            .and_then(|s| {
                let l = s.levels.get(self.level)?;
                Some((l.entries.clone(), l.highlighted))
            })
        else {
            return div().id("views-menu");
        };
        let has_check = entries.iter().any(|e| e.checked.is_some());
        let icon_start = s.label_start + if has_check { s.check_column } else { 0. };
        let has_icon = entries.iter().any(|e| e.icon.is_some());
        let label_start = icon_start + if has_icon { metrics::ICON_COLUMN } else { 0. };
        let keyboard = opened_from_keyboard(cx);
        let rows = entries.into_iter().enumerate().map(move |(ix, entry)| {
            if entry.is_separator() {
                return div()
                    .h(px(s.separator_height))
                    .w_full()
                    .flex_none()
                    .relative()
                    .child(
                        div()
                            .absolute()
                            .top(px(s.separator_rule))
                            .left_0()
                            .w_full()
                            .h(px(1.))
                            .bg(palette.separator),
                    )
                    .into_any_element();
            }
            let hot = highlighted == Some(ix);
            let (text, minor) = if !entry.enabled {
                (palette.disabled, palette.disabled)
            } else if hot {
                (palette.highlight_text, palette.highlight_text)
            } else {
                (palette.text, palette.accelerator)
            };
            // Chromium's Linux menus show no mnemonic underlines, even when
            // opened from the keyboard (measured in GitHub Desktop)
            // (GitHub Desktop's own app menu on Windows underlines its
            // access keys when it is driven from the keyboard)
            let label_element = match entry.mnemonic.filter(|_| s.app_menu && keyboard) {
                Some(at) => mnemonic_text(&entry.text, at, text).into_any_element(),
                None => div().child(entry.text.clone()).into_any_element(),
            };
            div()
                .h(px(s.item_height))
                .w_full()
                .flex_none()
                .relative()
                .flex()
                .items_center()
                .when(hot, |d| d.bg(palette.highlight))
                .text_color(text)
                .when_some(entry.checked.filter(|c| *c), |d, _| {
                    d.child(
                        div()
                            .absolute()
                            // `.menu-item.checked .icon`: centred in the
                            // space before the label
                            .left(px(if s.app_menu { 2. } else { s.label_start }))
                            .top(px(-s.text_raise))
                            .h(px(s.item_height))
                            .flex()
                            .items_center()
                            .child(
                                svg()
                                    .path("octicons/check-16.svg")
                                    .w(px(16.))
                                    .h(px(16.))
                                    .text_color(text),
                            ),
                    )
                })
                .when_some(entry.icon.clone(), |d, icon| {
                    d.child(
                        img(icon)
                            .absolute()
                            .left(px(icon_start))
                            .top(px((s.item_height - metrics::ICON_SIZE) / 2.))
                            .size(px(metrics::ICON_SIZE))
                            .when(!entry.enabled, |i| i.opacity(0.5)),
                    )
                })
                .child(
                    div()
                        .absolute()
                        .left(px(label_start))
                        .top(px(-s.text_raise))
                        .h(px(s.item_height))
                        .flex()
                        .items_center()
                        .child(label_element),
                )
                .when_some(entry.accelerator.clone(), |d, accelerator| {
                    let arrow = matches!(entry.kind, EntryKind::Submenu(_));
                    d.child(
                        div()
                            .absolute()
                            .right(px(
                                s.accelerator_right + if arrow { s.arrow_column } else { 0. }
                            ))
                            .top(px(-s.text_raise))
                            .h(px(s.item_height))
                            .flex()
                            .items_center()
                            .text_color(minor)
                            .child(accelerator),
                    )
                })
                .when(matches!(entry.kind, EntryKind::Submenu(_)), |d| {
                    d.child(
                        div()
                            .absolute()
                            .right(px(s.arrow_right))
                            .top(px(-s.text_raise))
                            .h(px(s.item_height))
                            .flex()
                            .items_center()
                            .child(if s.app_menu {
                                // `.submenu-arrow`: 12 px high, 70 % opaque
                                svg()
                                    .path("octicons/triangle-right-16.svg")
                                    .w(px(12.))
                                    .h(px(12.))
                                    .text_color(text.opacity(0.7))
                            } else {
                                svg()
                                    .path("octicons/chevron-right-16.svg")
                                    .w(px(16.))
                                    .h(px(16.))
                                    .text_color(text)
                            }),
                    )
                })
                .into_any_element()
        });
        div()
            .id("views-menu")
            .size_full()
            .pt(px(s.top_inset))
            .flex()
            .flex_col()
            .bg(palette.background)
            .font_family(font_family())
            .text_size(px(s.font_size))
            .line_height(px(20.))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                this.hover(f32::from(event.position.y), cx);
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                // leaving the menu drops the highlight unless a submenu is open
                if !*hovered {
                    let level = this.level;
                    let deeper = cx
                        .try_global::<Menus>()
                        .and_then(|m| m.session.as_ref())
                        .is_some_and(|s| s.levels.len() > level + 1);
                    if !deeper {
                        set_highlight(level, None, cx);
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _, cx| {
                    if let Some(row) = this.row_at(f32::from(event.position.y), cx) {
                        activate(this.level, row, false, cx);
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, event: &MouseUpEvent, _, cx| {
                    if let Some(row) = this.row_at(f32::from(event.position.y), cx) {
                        activate(this.level, row, false, cx);
                    }
                }),
            )
            .children(rows)
            .map(|menu| {
                #[cfg(any(target_os = "android", windows))]
                let menu = menu.overflow_y_scroll().track_scroll(&self.scroll);
                menu
            })
    }
}

/// `text` with the character at byte `at` underlined (Chromium's
/// mnemonic underline: the text colour, 1 px, two pixels below the
/// baseline).
pub fn mnemonic_text(text: &str, at: usize, color: Hsla) -> impl IntoElement {
    let end = text[at..]
        .char_indices()
        .nth(1)
        .map_or(text.len(), |(i, _)| at + i);
    StyledText::new(text.to_string()).with_highlights([(
        at..end,
        HighlightStyle {
            underline: Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(color),
                wavy: false,
            }),
            ..Default::default()
        },
    )])
}

/// Format a GPUI keystroke the way Chromium labels accelerators on Linux
/// (`Accelerator::GetShortcutText`): `Alt+Ctrl+Shift+Key`, keys by their
/// Chromium names (`Comma`, `Backspace`, `F11`).
pub fn accelerator_text(keystroke: &Keystroke) -> String {
    // Windows: the app menu shows GitHub Desktop's own accelerator strings
    // (`friendlyAcceleratorText` of `CmdOrCtrl+Shift+Alt+<key>`), where a
    // key is the character it types (`Ctrl+,`, `Ctrl+=`)
    #[cfg(windows)]
    if cfg!(windows) {
        let m = &keystroke.modifiers;
        let mut out = String::new();
        for (held, name) in [(m.control, "Ctrl+"), (m.shift, "Shift+"), (m.alt, "Alt+")] {
            if held {
                out.push_str(name);
            }
        }
        let key = keystroke.key.as_str();
        out.push_str(&match key {
            "backspace" => "Backspace".to_string(),
            "delete" => "Delete".to_string(),
            "enter" => "Enter".to_string(),
            "escape" => "Esc".to_string(),
            "space" => "Space".to_string(),
            "tab" => "Tab".to_string(),
            "up" | "down" | "left" | "right" | "home" | "end" => {
                let mut name = key.to_string();
                name[..1].make_ascii_uppercase();
                name
            }
            key => key.to_uppercase(),
        });
        return out;
    }
    let m = &keystroke.modifiers;
    let mut out = String::new();
    if m.alt {
        out.push_str("Alt+");
    }
    if m.control {
        out.push_str("Ctrl+");
    }
    if m.platform {
        out.push_str("Super+");
    }
    if m.shift {
        out.push_str("Shift+");
    }
    // GPUI names a shifted punctuation key by the character it types;
    // Chromium names the key
    let (shifted, key) = match keystroke.key.as_str() {
        "<" => (true, ","),
        ">" => (true, "."),
        "{" => (true, "["),
        "}" => (true, "]"),
        "?" => (true, "/"),
        "+" => (true, "="),
        "_" => (true, "-"),
        other => (false, other),
    };
    if shifted && !m.shift {
        out.push_str("Shift+");
    }
    let name = match key {
        "," => "Comma".to_string(),
        "." => "Period".to_string(),
        "backspace" => "Backspace".to_string(),
        "delete" => "Delete".to_string(),
        "enter" => "Enter".to_string(),
        "escape" => "Esc".to_string(),
        "space" => "Space".to_string(),
        "tab" => "Tab".to_string(),
        "up" => "Up".to_string(),
        "down" => "Down".to_string(),
        "left" => "Left".to_string(),
        "right" => "Right".to_string(),
        "home" => "Home".to_string(),
        "end" => "End".to_string(),
        "pageup" => "Page Up".to_string(),
        "pagedown" => "Page Down".to_string(),
        k if k.len() > 1 && k.starts_with('f') && k[1..].chars().all(|c| c.is_ascii_digit()) => {
            k.to_uppercase()
        }
        k => k.to_uppercase(),
    };
    out.push_str(&name);
    out
}

/// A context menu as Electron pops it on Linux: at `position` in `window`.
pub fn show_context_menu(
    items: &[MenuItem],
    position: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let entries = items.iter().map(Entry::from_menu_item).collect();
    open(
        entries,
        Anchor::point(position),
        Source::Context,
        false,
        None,
        window,
        cx,
    );
}

/// The open context or menu bar menu's rows, for the parity harness.
pub fn pick(label: &str, cx: &mut App) -> bool {
    let found = cx
        .try_global::<Menus>()
        .and_then(|m| m.session.as_ref())
        .and_then(|s| {
            s.levels.iter().enumerate().find_map(|(li, l)| {
                l.entries
                    .iter()
                    .position(|e| e.text.as_ref() == label && e.selectable())
                    .map(|row| (li, row))
            })
        });
    match found {
        Some((level, row)) => {
            activate(level, row, false, cx);
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    // not `super::*`: gpui's `test` attribute would shadow the standard one
    use super::{Entry, EntryKind, accelerator_text, mnemonic_char, parse_mnemonic, step};
    use gpui_kit::Keystroke;
    use std::rc::Rc;

    #[test]
    fn parses_mnemonics() {
        assert_eq!(parse_mnemonic("&File"), ("File".into(), Some(0)));
        assert_eq!(
            parse_mnemonic("Clo&ne repository…"),
            ("Clone repository…".into(), Some(3))
        );
        assert_eq!(
            parse_mnemonic("Fish && chips"),
            ("Fish & chips".into(), None)
        );
        assert_eq!(parse_mnemonic("Plain"), ("Plain".into(), None));
        assert_eq!(mnemonic_char("Clone", Some(3)), Some('n'));
    }

    #[test]
    #[cfg(windows)]
    fn formats_accelerators_like_github_desktop() {
        let k = |s: &str| accelerator_text(&Keystroke::parse(s).unwrap());
        assert_eq!(k("ctrl-,"), "Ctrl+,");
        assert_eq!(k("ctrl-shift-backspace"), "Ctrl+Shift+Backspace");
        assert_eq!(k("ctrl-shift-alt-a"), "Ctrl+Shift+Alt+A");
        assert_eq!(k("ctrl-`"), "Ctrl+`");
        assert_eq!(k("alt-f4"), "Alt+F4");
        assert_eq!(k("f11"), "F11");
    }

    #[test]
    #[cfg(not(windows))]
    fn formats_accelerators_like_chromium() {
        let k = |s: &str| accelerator_text(&Keystroke::parse(s).unwrap());
        assert_eq!(k("ctrl-,"), "Ctrl+Comma");
        assert_eq!(k("ctrl-shift-backspace"), "Ctrl+Shift+Backspace");
        assert_eq!(k("ctrl-alt-shift-a"), "Alt+Ctrl+Shift+A");
        assert_eq!(k("ctrl-alt-w"), "Alt+Ctrl+W");
        assert_eq!(k("f11"), "F11");
        assert_eq!(k("ctrl-`"), "Ctrl+`");
        assert_eq!(k("ctrl-="), "Ctrl+=");
        assert_eq!(k("ctrl--"), "Ctrl+-");
        assert_eq!(k("ctrl-<"), "Ctrl+Shift+Comma");
    }

    fn entry(enabled: bool) -> Entry {
        Entry {
            text: "x".into(),
            mnemonic: None,
            accelerator: None,
            enabled,
            checked: None,
            icon: None,
            kind: EntryKind::Action(Rc::new(|_, _| {})),
        }
    }

    #[test]
    fn keyboard_steps_skip_disabled_rows_and_wrap() {
        let entries = vec![entry(false), entry(true), Entry::separator(), entry(true)];
        assert_eq!(step(&entries, None, true), Some(1));
        assert_eq!(step(&entries, Some(1), true), Some(3));
        assert_eq!(step(&entries, Some(3), true), Some(1));
        assert_eq!(step(&entries, Some(1), false), Some(3));
        assert_eq!(step(&entries, None, false), Some(3));
    }
}
