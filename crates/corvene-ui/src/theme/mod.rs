//! GitHub Desktop's theme tokens as a GPUI global, plus sizes.
//!
//! Token names follow `app/styles/_variables.scss` / `themes/_dark.scss` with
//! the `--` prefix and `-color` suffix dropped. See
//! `.docs/ghd-theme-tokens.md`.

mod ghd_dark;
mod ghd_high_contrast;
/// The high contrast theme's `GhdTheme::name`.
pub const HIGH_CONTRAST_NAME: &str = ghd_high_contrast::NAME;
mod ghd_light;
pub mod primer;

use gpui_kit::component::theme::{Theme as KitTheme, ThemeMode};
use gpui_kit::{App, Global, Hsla, Pixels, Rgba, px, rgb};

/// Which built-in theme is active.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Light,
    Dark,
}

/// GHD's `_variables.scss` sizes, scaled by the window zoom factor
/// (View › Zoom In / Zoom Out / Reset Zoom; Electron's page zoom in GHD).
/// Every size in `corvene-ui` goes through [`sizes::zpx`], so the whole
/// layout follows one factor.
pub mod sizes {
    use std::cell::Cell;

    use gpui_kit::{Pixels, px};

    thread_local! {
        static ZOOM: Cell<f32> = const { Cell::new(1.0) };
    }

    /// GHD `ZoomInFactors`: the supported steps, ascending.
    pub const ZOOM_FACTORS: [f32; 10] = [0.67, 0.75, 0.8, 0.9, 1., 1.1, 1.25, 1.5, 1.75, 2.];

    /// The current window zoom factor (1 = 100 %).
    pub fn zoom_factor() -> f32 {
        ZOOM.with(|z| z.get())
    }

    /// Set the zoom factor; callers re-render (`cx.refresh_windows()`).
    pub fn set_zoom_factor(factor: f32) {
        ZOOM.with(|z| z.set(factor.clamp(0.25, 5.)));
    }

    /// Build `f` with the zoom factor multiplied by `extra`: a CSS `zoom` on
    /// one subtree (`#no-repositories { zoom: 1.2 }`). Sizes are resolved
    /// when elements are built, so deferred builders (a `uniform_list`'s row
    /// closure) wrap their own body too.
    pub fn with_zoom<R>(extra: f32, f: impl FnOnce() -> R) -> R {
        struct Restore(f32);
        impl Drop for Restore {
            fn drop(&mut self) {
                let previous = self.0;
                ZOOM.with(|z| z.set(previous));
            }
        }
        let _restore = Restore(zoom_factor());
        ZOOM.with(|z| z.set(z.get() * extra));
        f()
    }

    /// `v` CSS pixels at the current zoom.
    #[inline]
    /// A CSS `box-shadow` blur length as GPUI's shadow `blur_radius`: CSS
    /// blurs with a Gaussian of σ = blur / 2, GPUI takes σ itself.
    pub fn css_blur(v: f32) -> Pixels {
        zpx(v / 2.)
    }

    pub fn zpx(v: f32) -> Pixels {
        px(v * zoom_factor())
    }

    /// Screen pixels back to CSS pixels (persisted widths).
    #[inline]
    pub fn unzoom(v: Pixels) -> f32 {
        f32::from(v) / zoom_factor()
    }

    /// The next step for `direction` (+1 in, -1 out) from `current`, snapping
    /// a stray factor to the closest step first (GHD `findClosestValue`).
    pub fn next_zoom_factor(current: f32, direction: i32) -> f32 {
        let closest = ZOOM_FACTORS
            .iter()
            .copied()
            .min_by(|a, b| (a - current).abs().total_cmp(&(b - current).abs()))
            .unwrap_or(1.);
        let next = if direction > 0 {
            ZOOM_FACTORS.iter().copied().find(|f| *f > closest)
        } else {
            ZOOM_FACTORS.iter().rev().copied().find(|f| *f < closest)
        };
        next.unwrap_or(closest)
    }

    /// The macOS title-bar strip; GHD has none on Linux (the menu bar sits
    /// above the page there, outside the workspace).
    #[allow(non_snake_case)]
    pub fn TITLE_BAR_HEIGHT() -> Pixels {
        if cfg!(target_os = "macos") {
            zpx(32.)
        } else {
            zpx(0.)
        }
    }
    #[allow(non_snake_case)]
    pub fn TOOLBAR_HEIGHT() -> Pixels {
        // a short window (a phone on its side) has no 50 px to give
        zpx(if super::short() { 38. } else { 50. })
    }
    #[allow(non_snake_case)]
    pub fn TOOLBAR_BUTTON_HEIGHT() -> Pixels {
        TOOLBAR_HEIGHT() - zpx(1.)
    }
    #[allow(non_snake_case)]
    pub fn TOOLBAR_BUTTON_WIDTH() -> Pixels {
        zpx(230.)
    }
    #[allow(non_snake_case)]
    pub fn TOOLBAR_ARROW_WIDTH() -> Pixels {
        zpx(39.)
    }
    #[allow(non_snake_case)]
    pub fn TAB_BAR_HEIGHT() -> Pixels {
        zpx(29.)
    }
    #[allow(non_snake_case)]
    pub fn ROW_HEIGHT() -> Pixels {
        zpx(29.)
    }
    #[allow(non_snake_case)]
    pub fn SIDEBAR_DEFAULT_WIDTH() -> Pixels {
        zpx(250.)
    }
    #[allow(non_snake_case)]
    pub fn SIDEBAR_MIN_WIDTH() -> Pixels {
        zpx(220.)
    }
    #[allow(non_snake_case)]
    pub fn RESIZE_HANDLE_WIDTH() -> Pixels {
        zpx(6.)
    }
    #[allow(non_snake_case)]
    pub fn BUTTON_HEIGHT() -> Pixels {
        zpx(25.)
    }
    #[allow(non_snake_case)]
    pub fn TEXT_FIELD_HEIGHT() -> Pixels {
        zpx(25.)
    }
    #[allow(non_snake_case)]
    pub fn BORDER_RADIUS() -> Pixels {
        zpx(6.)
    }
    #[allow(non_snake_case)]
    pub fn OUTLINED_BORDER_RADIUS() -> Pixels {
        zpx(3.)
    }
    #[allow(non_snake_case)]
    pub fn SPACING() -> Pixels {
        zpx(10.)
    }
    #[allow(non_snake_case)]
    pub fn SPACING_HALF() -> Pixels {
        zpx(5.)
    }
    #[allow(non_snake_case)]
    pub fn SPACING_THIRD() -> Pixels {
        zpx(3.33)
    }
    #[allow(non_snake_case)]
    pub fn SPACING_DOUBLE() -> Pixels {
        zpx(20.)
    }
    #[allow(non_snake_case)]
    pub fn FONT_SIZE() -> Pixels {
        zpx(12.)
    }
    #[allow(non_snake_case)]
    pub fn FONT_SIZE_XS() -> Pixels {
        zpx(9.)
    }
    #[allow(non_snake_case)]
    pub fn FONT_SIZE_SM() -> Pixels {
        zpx(11.)
    }
    #[allow(non_snake_case)]
    pub fn FONT_SIZE_MD() -> Pixels {
        zpx(14.)
    }
    #[allow(non_snake_case)]
    pub fn FONT_SIZE_LG() -> Pixels {
        zpx(28.)
    }
    #[allow(non_snake_case)]
    pub fn AVATAR_SIZE() -> Pixels {
        zpx(25.)
    }
    #[allow(non_snake_case)]
    pub fn ICON_SIZE() -> Pixels {
        zpx(16.)
    }
    #[allow(non_snake_case)]
    pub fn CHECKBOX_SIZE() -> Pixels {
        zpx(13.)
    }
    #[allow(non_snake_case)]
    pub fn DIFF_LINE_NUMBER_WIDTH() -> Pixels {
        zpx(50.)
    }

    #[cfg(test)]
    mod tests {
        #[::core::prelude::v1::test]
        fn zoom_steps_follow_ghd() {
            use super::next_zoom_factor;
            assert_eq!(next_zoom_factor(1.0, 1), 1.1);
            assert_eq!(next_zoom_factor(1.0, -1), 0.9);
            assert_eq!(next_zoom_factor(2.0, 1), 2.0);
            assert_eq!(next_zoom_factor(0.67, -1), 0.67);
            // a stray factor snaps to the closest step first
            assert_eq!(next_zoom_factor(1.24, 1), 1.5);
            assert_eq!(next_zoom_factor(1.24, -1), 1.1);
        }
    }
}

/// What the system covers at the window's edges: nothing on the desktop.
/// Android draws the status and navigation bars over the window and opens
/// the on-screen keyboard above it; `menu_bar::MenuBarShell` keeps clear of
/// them and records the edges here on every frame.
pub fn safe_area() -> gpui_kit::Edges<gpui_kit::Pixels> {
    #[cfg(target_os = "android")]
    {
        SAFE_AREA.with(|area| area.get())
    }
    #[cfg(not(target_os = "android"))]
    {
        gpui_kit::Edges::default()
    }
}

/// Below this window height (logical pixels) the touch layout saves rows:
/// a lower toolbar, a one-line description box, lower menu items.
pub const SHORT_HEIGHT: f32 = 480.;

thread_local! {
    static SHORT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the phone layouts may be used at all: on Android, and with
/// `CORVENE_COMPACT=1` elsewhere (to work on them without a device).
fn phone_layouts() -> bool {
    static ALLOWED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ALLOWED.get_or_init(|| {
        cfg!(target_os = "android") || std::env::var_os("CORVENE_COMPACT").is_some()
    })
}

/// Whether the window is short (a phone held sideways), as of the last
/// [`update_safe_area`]. GHD's window is at least 660 px tall. Android only,
/// like [`compact`].
pub fn short() -> bool {
    SHORT.with(|short| short.get())
}

#[cfg(target_os = "android")]
thread_local! {
    static SAFE_AREA: std::cell::Cell<gpui_kit::Edges<gpui_kit::Pixels>> =
        std::cell::Cell::new(gpui_kit::Edges::default());
}

/// Reads [`safe_area`] from the window (Android; a no-op elsewhere).
pub fn update_safe_area(window: &gpui_kit::Window) {
    SHORT.with(|short| {
        short.set(phone_layouts() && window.viewport_size().height < gpui_kit::px(SHORT_HEIGHT))
    });
    #[cfg(target_os = "android")]
    {
        let viewport = window.viewport_size();
        let visible = window.fully_visible_bounds();
        SAFE_AREA.with(|area| {
            area.set(gpui_kit::Edges {
                top: visible.top(),
                left: visible.left(),
                right: viewport.width - visible.right(),
                bottom: viewport.height - visible.bottom(),
            })
        });
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = window;
    }
}

/// Where GHD's page starts in the window: below Electron's menu bar on
/// Linux (`crate::menu_bar`), at the top on macOS. Android: the menu bar
/// sits under the status bar.
pub fn page_top() -> gpui_kit::Pixels {
    #[cfg(target_os = "macos")]
    {
        gpui_kit::px(0.)
    }
    // Windows: below the title bar, unless it lies over the page (the
    // welcome flow) or is hidden (full screen)
    #[cfg(windows)]
    {
        gpui_kit::px(crate::title_bar_windows::page_top())
    }
    #[cfg(not(any(target_os = "macos", target_os = "android", windows)))]
    {
        gpui_kit::px(crate::menu_bar::HEIGHT)
    }
    #[cfg(target_os = "android")]
    {
        safe_area().top + gpui_kit::px(crate::menu_bar::HEIGHT)
    }
}

/// GHD's viewport (`100vh`, `innerHeight`): the window's content minus
/// the menu bar on Linux (Android: and what the system covers).
pub fn page_size(window: &gpui_kit::Window) -> gpui_kit::Size<gpui_kit::Pixels> {
    let viewport = window.viewport_size();
    #[cfg(target_os = "android")]
    let viewport = {
        let area = safe_area();
        gpui_kit::size(
            viewport.width - area.left - area.right,
            viewport.height - area.bottom,
        )
    };
    gpui_kit::size(viewport.width, viewport.height - page_top())
}

/// Below this page width (logical pixels) Android lays the window out for a
/// phone: one column instead of GHD's sidebar beside the content.
pub const COMPACT_WIDTH: f32 = 600.;

/// Whether the window gets the compact (phone) layout. GHD has none: its
/// window cannot be narrower than 960 px. Android only; `CORVENE_COMPACT=1`
/// turns it on elsewhere, to work on it without a device.
pub fn compact(window: &gpui_kit::Window) -> bool {
    phone_layouts() && window.viewport_size().width < gpui_kit::px(COMPACT_WIDTH)
}

thread_local! {
    /// The page width while the compact layout is on (set every frame by
    /// the workspace), for the fixed widths GHD's dialogs have.
    static COMPACT_PAGE_WIDTH: std::cell::Cell<Option<gpui_kit::Pixels>> =
        const { std::cell::Cell::new(None) };
}

pub(crate) fn set_compact_page_width(width: Option<gpui_kit::Pixels>) {
    COMPACT_PAGE_WIDTH.with(|cell| cell.set(width));
}

/// A width from GHD's stylesheet (`width: 450px` of a dialog's content),
/// narrowed in the compact layout to what a dialog's content has there: the
/// page less the dialog's margin, border and padding.
pub fn fit_width(width: f32) -> gpui_kit::Pixels {
    let width = sizes::zpx(width);
    match COMPACT_PAGE_WIDTH.with(|cell| cell.get()) {
        Some(page) => width.min(page - sizes::zpx(58.)),
        None => width,
    }
}

/// [`fit_width`] for content that cancels the dialog's padding with negative
/// margins (Settings, Repository Settings).
pub fn fit_bleed_width(width: f32) -> gpui_kit::Pixels {
    let width = sizes::zpx(width);
    match COMPACT_PAGE_WIDTH.with(|cell| cell.get()) {
        Some(page) => width.min(page - sizes::zpx(18.)),
        None => width,
    }
}

/// Chromium's `line-height: normal` (and an inline box's content area) for
/// the UI font at `size`: the font's ascent plus descent, each rounded to a
/// pixel. SF gives about 1.19 em, Noto Sans 1.36 em, so boxes GHD sizes by
/// it (a button, an inline span) grow off macOS.
pub fn normal_line_height(size: gpui_kit::Pixels, cx: &gpui_kit::App) -> gpui_kit::Pixels {
    let text = cx.text_system();
    let id = text.resolve_font(&gpui_kit::font(ui_font()));
    let ascent = f32::from(text.ascent(id, size)).round();
    let descent = f32::from(text.descent(id, size)).abs().round();
    gpui_kit::px(ascent + descent)
}

/// The page's top-left corner in window coordinates: where a full-page
/// overlay (`anchored()` positions are window coordinates) starts.
pub fn page_origin() -> gpui_kit::Point<gpui_kit::Pixels> {
    #[cfg(not(target_os = "android"))]
    {
        gpui_kit::point(gpui_kit::px(0.), page_top())
    }
    #[cfg(target_os = "android")]
    {
        gpui_kit::point(safe_area().left, page_top())
    }
}

/// [`page_size`] where it sits in window coordinates.
pub fn page_bounds(window: &gpui_kit::Window) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    gpui_kit::Bounds::new(page_origin(), page_size(window))
}

pub const UI_FONT: &str = ".SystemUIFont";

/// The UI font as GPUI should be asked for it: the `.SystemUIFont` sentinel
/// on macOS (the SF system font); elsewhere the family GHD's `system-ui`
/// resolves to in Chromium (`corvene_platform::fonts::ghd_ui_family`, Noto
/// Sans on Ubuntu). GPUI's Linux sentinel is "IBM Plex Sans", rarely
/// installed, and its fallback loses the bold and semibold faces.
pub fn ui_font() -> gpui_kit::SharedString {
    #[cfg(target_os = "macos")]
    {
        UI_FONT.into()
    }
    #[cfg(not(target_os = "macos"))]
    {
        static FAMILY: std::sync::OnceLock<gpui_kit::SharedString> = std::sync::OnceLock::new();
        FAMILY
            .get_or_init(|| corvene_platform::fonts::ghd_ui_family().into())
            .clone()
    }
}
/// The macOS system font by its real family name. gpui-kit only enumerates
/// every installed font (≈1.5 s) when its theme font is left at the
/// `.SystemUIFont` sentinel, so the kit theme is seeded with this instead.
pub const KIT_UI_FONT: &str = ".AppleSystemUIFont";
/// The kit's monospace family. Anything but gpui-base's default ("Menlo")
/// keeps the kit from enumerating installed fonts at startup.
pub const KIT_MONO_FONT: &str = ".AppleSystemUIFontMonospaced";

/// gpui-kit's UI font: the real macOS system family name (see
/// [`KIT_UI_FONT`]), or [`ui_font`] elsewhere.
fn kit_ui_font() -> gpui_kit::SharedString {
    if cfg!(target_os = "macos") {
        KIT_UI_FONT.into()
    } else {
        ui_font()
    }
}

/// gpui-kit's monospace font: the macOS sentinel, or [`mono_font`]
/// elsewhere (the sentinel is unknown off macOS).
fn kit_mono_font() -> gpui_kit::SharedString {
    if cfg!(target_os = "macos") {
        KIT_MONO_FONT.into()
    } else {
        mono_font().into()
    }
}

static MONO_FAMILY: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();

/// Diff and code text: what GHD's monospace stack resolves to on this Mac
/// (`corvene_platform::fonts::ghd_monospace_family`, set by the binary at
/// startup), Menlo until then.
pub fn mono_font() -> &'static str {
    MONO_FAMILY.get().copied().unwrap_or("Menlo")
}

pub fn set_mono_font(family: &'static str) {
    let _ = MONO_FAMILY.set(family);
}

/// Font features for text under a CSS `zoom` (GHD's `#no-repositories {
/// zoom: 1.2 }`): Chromium lays that text out at the unzoomed size
/// and scales it, so SF's size-dependent tracking stays the unzoomed size's
/// (vendored gpui-pre-macos reads the private `czom` tag, zoom × 100).
pub fn css_zoom_features(zoom: f32) -> gpui_kit::FontFeatures {
    gpui_kit::FontFeatures(std::sync::Arc::new(vec![(
        "czom".into(),
        (zoom * 100.).round() as u32,
    )]))
}

/// GitHub Desktop colour tokens.
#[derive(Clone, Debug)]
pub struct GhdTheme {
    pub name: &'static str,
    pub appearance: Appearance,
    /// The title bar is drawn light (flag `109-light-toolbar`); GHD's is
    /// always the dark gradient.
    pub light_title_bar: bool,

    // Text + surfaces
    pub text: Hsla,
    pub text_secondary: Hsla,
    pub text_secondary_muted: Hsla,
    pub background: Hsla,
    pub box_background: Hsla,
    pub box_alt_background: Hsla,
    /// `--tip-box-background-color` / `--tip-box-border-color` (blank-slate ProTip).
    pub tip_box_background: Hsla,
    pub tip_box_border: Hsla,
    /// `--path-segment-background` (`Ref` path chips).
    pub path_segment_background: Hsla,
    pub box_border: Hsla,
    pub box_border_contrast: Hsla,
    pub box_border_accent: Hsla,
    pub box_selected_background: Hsla,
    pub box_selected_text: Hsla,
    pub box_selected_active_background: Hsla,
    pub box_selected_active_text: Hsla,
    pub box_hover_background: Hsla,
    pub box_hover_text: Hsla,
    pub box_placeholder: Hsla,
    pub shadow: Hsla,
    pub overlay: Hsla,
    /// `::backdrop` of a modal `<dialog>`: rgba(0,0,0,.4) in every theme, since
    /// the pseudo-element does not inherit `body.theme-dark`'s variables.
    pub dialog_backdrop: Hsla,

    // Buttons + links
    pub button_background: Hsla,
    pub button_hover_background: Hsla,
    pub button_text: Hsla,
    pub button_focus_border: Hsla,
    pub secondary_button_background: Hsla,
    pub secondary_button_border: Hsla,
    pub secondary_button_hover_background: Hsla,
    pub secondary_button_hover_border: Hsla,
    pub secondary_button_text: Hsla,
    pub link: Hsla,
    pub link_hover: Hsla,

    // Toolbar
    pub toolbar_background: Hsla,
    pub toolbar_border: Hsla,
    pub toolbar_text: Hsla,
    pub toolbar_text_secondary: Hsla,
    pub toolbar_button_border: Hsla,
    pub toolbar_button_hover_background: Hsla,
    pub toolbar_button_hover_text: Hsla,
    pub toolbar_button_active_background: Hsla,
    pub toolbar_button_active_text: Hsla,
    pub toolbar_button_progress: Hsla,
    pub toolbar_badge_background: Hsla,
    pub toolbar_badge_active_background: Hsla,
    pub toolbar_dropdown_text_warning: Hsla,

    // Tab bar + lists
    pub tab_bar_active: Hsla,
    pub tab_bar_background: Hsla,
    pub tab_bar_hover_background: Hsla,
    pub tab_bar_count_text: Hsla,
    pub tab_bar_count_background: Hsla,
    /// `--pr-open-icon-color` / `--pr-draft-icon-color`
    pub pr_open_icon: Hsla,
    pub pr_draft_icon: Hsla,
    pub list_item_hover_background: Hsla,
    pub list_item_badge_text: Hsla,
    pub list_item_badge_background: Hsla,
    pub list_item_selected_badge_text: Hsla,
    pub list_item_selected_badge_background: Hsla,
    pub list_item_selected_active_badge_text: Hsla,
    pub list_item_selected_active_badge_background: Hsla,
    pub scroll_bar_thumb: Hsla,
    pub scroll_bar_thumb_active: Hsla,

    // Focus + accents
    pub focus: Hsla,
    pub accent: Hsla,
    pub text_field_focus_shadow: Hsla,
    pub primary_suggested_action_background: Hsla,
    pub primary_suggested_action_border: Hsla,
    pub suggested_action_icon: Hsla,

    // Native form controls. GHD leaves `<input type="checkbox">` to Chromium
    // (`native_theme_base.cc`, light/dark scheme) and only sets `accent-color`
    // (`accent` above); these are Chromium's own control colours.
    /// Unchecked box fill; also the check/dash colour on an `accent` fill.
    pub control_background: Hsla,
    /// Unchecked box 1 px border.
    pub control_border: Hsla,
    pub control_disabled_background: Hsla,
    pub control_disabled_border: Hsla,
    /// Checked/indeterminate fill while disabled.
    pub control_disabled_accent: Hsla,
    /// Check/dash colour on the disabled fill.
    pub control_disabled_glyph: Hsla,

    // File status colours
    pub color_new: Hsla,
    pub color_deleted: Hsla,
    pub color_modified: Hsla,
    pub color_renamed: Hsla,
    pub color_conflicted: Hsla,

    // Commit form
    pub co_author_tag_background: Hsla,
    pub co_author_tag_border: Hsla,
    pub input_icon_warning: Hsla,
    pub input_icon_error: Hsla,
    /// `--commit-warning-badge-background-color` / `-border-color`: the
    /// commit form avatar's warning badge.
    pub commit_warning_badge_background: Hsla,
    pub commit_warning_badge_border: Hsla,
    pub input_icon_hover_background: Hsla,

    // Dialogs + status
    pub error: Hsla,
    pub form_error_background: Hsla,
    pub form_error_border: Hsla,
    pub form_error_text: Hsla,
    /// `--input-error-text-color` (`InputError` under a field).
    pub input_error_text: Hsla,
    pub dialog_warning: Hsla,
    pub dialog_information: Hsla,
    pub dialog_error: Hsla,
    pub dialog_banner_success_background: Hsla,
    pub dialog_banner_success_border: Hsla,
    pub dialog_banner_success_text: Hsla,
    /// Selected text in the diff and in text boxes (GHD sets no
    /// `::selection`, so Chromium paints macOS's
    /// selectedTextBackgroundColor with the blue accent).
    pub text_selection_background: Hsla,
    /// `--banner-warning-*`: the update banner (`#update-available`).
    pub banner_warning_background: Hsla,
    pub banner_warning_text: Hsla,
    pub banner_warning_link: Hsla,
    pub banner_warning_icon: Hsla,
    pub status_pending: Hsla,
    pub status_error: Hsla,
    pub status_success: Hsla,
    pub tooltip_background: Hsla,
    pub tooltip_text: Hsla,
    /// `--tooltip-shadow-color` (`0 8px 24px`).
    pub tooltip_shadow: Hsla,

    // Native (NSMenu-like) context menus - Corvene addition, GHD uses real NSMenus
    pub menu_background: Hsla,
    pub menu_border: Hsla,
    pub menu_text: Hsla,
    pub menu_text_disabled: Hsla,
    pub menu_highlight: Hsla,
    pub menu_highlight_text: Hsla,

    // Syntax highlighting (`--syntax-*-color`, `.cm-s-default` in `_diff.scss`)
    pub syntax_variable: Hsla,
    pub syntax_alt_variable: Hsla,
    pub syntax_keyword: Hsla,
    pub syntax_atom: Hsla,
    pub syntax_string: Hsla,
    pub syntax_qualifier: Hsla,
    pub syntax_type: Hsla,
    pub syntax_comment: Hsla,
    pub syntax_tag: Hsla,
    pub syntax_attribute: Hsla,
    pub syntax_link: Hsla,
    pub syntax_header: Hsla,
    pub syntax_quote: Hsla,

    // Diff
    pub diff_text: Hsla,
    pub diff_alt_text: Hsla,
    pub diff_border: Hsla,
    pub diff_gutter: Hsla,
    pub diff_gutter_background: Hsla,
    pub diff_line_number: Hsla,
    pub diff_selected_background: Hsla,
    pub diff_selected_border: Hsla,
    pub diff_selected_gutter: Hsla,
    pub diff_selected_text: Hsla,
    pub diff_add_background: Hsla,
    pub diff_add_border: Hsla,
    pub diff_add_gutter: Hsla,
    pub diff_add_gutter_background: Hsla,
    pub diff_add_inner_background: Hsla,
    pub diff_add_text: Hsla,
    pub diff_delete_background: Hsla,
    pub diff_delete_border: Hsla,
    pub diff_delete_gutter: Hsla,
    pub diff_delete_gutter_background: Hsla,
    pub diff_delete_inner_background: Hsla,
    pub diff_delete_text: Hsla,
    pub diff_hunk_background: Hsla,
    pub diff_hunk_border: Hsla,
    pub diff_hunk_gutter: Hsla,
    pub diff_hunk_gutter_background: Hsla,
    pub diff_hunk_text: Hsla,
    pub diff_hover_background: Hsla,
    pub diff_hover_border: Hsla,
    pub diff_hover_gutter: Hsla,
    pub diff_hover_text: Hsla,
    pub diff_add_hover_background: Hsla,
    pub diff_add_hover_border: Hsla,
    pub diff_add_hover_gutter: Hsla,
    pub diff_add_hover_text: Hsla,
    pub diff_delete_hover_background: Hsla,
    pub diff_delete_hover_border: Hsla,
    pub diff_delete_hover_gutter: Hsla,
    pub diff_delete_hover_text: Hsla,
    pub diff_empty_row_background: Hsla,
    pub diff_empty_row_gutter_background: Hsla,
    /// Corvene `1316-diff-moved-lines`: the gutter bar of a moved block
    /// and of the block next to it (zebra).
    pub diff_moved_bar: Hsla,
    pub diff_moved_bar_alternate: Hsla,
    /// Corvene `1317-diff-stylistic-changes`: the gutter bar of a line
    /// whose change is only whitespace or formatting.
    pub diff_stylistic_bar: Hsla,
    /// `--file-warning-*`: the bidi / line-endings notice above a diff.
    pub file_warning_background: Hsla,
    pub file_warning: Hsla,
    pub file_warning_border: Hsla,
    pub diff_empty_hunk_handle: Hsla,

    // Markdown (`--md-*-color`, `static/common/markdown.css`)
    pub md_border_default: Hsla,
    pub md_border_muted: Hsla,
    pub md_canvas_subtle: Hsla,
    pub md_fg_muted: Hsla,
    pub md_neutral_muted: Hsla,
    pub md_accent_fg: Hsla,

    // Pull request reviews (`--pr-*-icon-*-color`)
    pub pr_timeline_line: Hsla,
    pub pr_changes_requested_icon: Hsla,
    pub pr_changes_requested_icon_background: Hsla,
    pub pr_approved_icon: Hsla,
    pub pr_approved_icon_background: Hsla,
    pub pr_commented_icon: Hsla,
    pub pr_commented_icon_background: Hsla,
}

impl Global for GhdTheme {}

/// Flag-driven changes to a palette, applied by [`GhdTheme::with_variants`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThemeVariants {
    /// Flag `108-colour-blind-diff`: blue additions, orange deletions.
    pub colour_blind_diff: bool,
    /// Flag `109-light-toolbar`: the Light theme's title bar and toolbar
    /// are light too.
    pub light_toolbar: bool,
}

impl ThemeVariants {
    pub fn of(flags: &corvene_core::Flags) -> Self {
        Self {
            colour_blind_diff: flags.bool(corvene_core::flags::ids::COLOUR_BLIND_DIFF),
            light_toolbar: flags.bool(corvene_core::flags::ids::LIGHT_TOOLBAR),
        }
    }
}

impl GhdTheme {
    /// The palette with the flagged variants applied. High Contrast keeps
    /// its own diff colours.
    pub fn with_variants(mut self, variants: ThemeVariants) -> Self {
        if variants.colour_blind_diff && self.name != "High Contrast" {
            self.colour_blind_diff();
        }
        if variants.light_toolbar && !self.is_dark() {
            self.light_toolbar();
        }
        self
    }

    /// Corvene addition (desktop/desktop#22123, #22470): Primer light greys
    /// for the title bar and toolbar instead of GHD's dark chrome
    /// (`app/styles/themes/_light.scss` `--toolbar-*`).
    fn light_toolbar(&mut self) {
        self.light_title_bar = true;
        self.toolbar_background = c(0xf6f8fa);
        self.toolbar_border = c(0xd0d7de);
        self.toolbar_text = c(0x24292e);
        self.toolbar_text_secondary = c(0x586069);
        self.toolbar_button_border = c(0xd0d7de);
        self.toolbar_button_hover_background = c(0xeaeef2);
        self.toolbar_button_hover_text = c(0x24292e);
        self.toolbar_button_active_background = c(0xffffff);
        self.toolbar_button_active_text = c(0x24292e);
        self.toolbar_button_progress = c(0xdde3ea);
        self.toolbar_badge_background = c(0xd1d5da);
        self.toolbar_badge_active_background = c(0xe1e4e8);
    }

    /// Corvene addition (desktop/desktop#6795): additions in blue and
    /// deletions in orange, after Primer's protanopia / deuteranopia themes,
    /// so the two differ in hue and lightness for red-green colour blindness.
    fn colour_blind_diff(&mut self) {
        // (background, gutter background, border / gutter, inner, hover
        // background, hover border / gutter, text)
        let (add, delete) = if self.is_dark() {
            (
                (
                    0x0f2b47, 0x0b2139, 0x1f4b7a, 0x1f6feb, 0x0c2d6b, 0x1158c7, 0xe1e4e8,
                ),
                (
                    0x3a2211, 0x2e1b0c, 0x5c3316, 0xbd561d, 0x762d0a, 0x9b4215, 0xe1e4e8,
                ),
            )
        } else {
            (
                (
                    0xddf4ff, 0xb6e3ff, 0x9cd7ff, 0x9fd4ff, 0xb6e3ff, 0x80ccff, 0x24292e,
                ),
                (
                    0xfff1e5, 0xffd8b5, 0xffc799, 0xffc796, 0xffd8b5, 0xffb77c, 0x24292e,
                ),
            )
        };
        self.diff_add_background = c(add.0);
        self.diff_add_gutter_background = c(add.1);
        self.diff_add_border = c(add.2);
        self.diff_add_gutter = c(add.2);
        self.diff_add_inner_background = c(add.3);
        self.diff_add_hover_background = c(add.4);
        self.diff_add_hover_border = c(add.5);
        self.diff_add_hover_gutter = c(add.5);
        self.diff_add_text = c(add.6);
        self.diff_add_hover_text = c(add.6);
        self.diff_delete_background = c(delete.0);
        self.diff_delete_gutter_background = c(delete.1);
        self.diff_delete_border = c(delete.2);
        self.diff_delete_gutter = c(delete.2);
        self.diff_delete_inner_background = c(delete.3);
        self.diff_delete_hover_background = c(delete.4);
        self.diff_delete_hover_border = c(delete.5);
        self.diff_delete_hover_gutter = c(delete.5);
        self.diff_delete_text = c(delete.6);
        self.diff_delete_hover_text = c(delete.6);
    }

    pub fn light() -> Self {
        ghd_light::theme()
    }

    pub fn dark() -> Self {
        ghd_dark::theme()
    }

    /// Corvene addition: GHD's dark tokens in Primer's high contrast palette.
    pub fn high_contrast() -> Self {
        ghd_high_contrast::theme()
    }

    pub fn for_appearance(appearance: Appearance) -> Self {
        match appearance {
            Appearance::Light => Self::light(),
            Appearance::Dark => Self::dark(),
        }
    }

    /// The palette for Settings › Appearance › Theme: System follows the
    /// system appearance, and "Increase contrast" picks High Contrast.
    pub fn for_setting(
        setting: corvene_core::ThemeSetting,
        system_dark: bool,
        increase_contrast: bool,
    ) -> Self {
        use corvene_core::ThemeSetting;
        match setting {
            ThemeSetting::Light => Self::light(),
            ThemeSetting::Dark => Self::dark(),
            ThemeSetting::HighContrast => Self::high_contrast(),
            ThemeSetting::System if increase_contrast => Self::high_contrast(),
            ThemeSetting::System if system_dark => Self::dark(),
            ThemeSetting::System => Self::light(),
        }
    }

    pub fn is_dark(&self) -> bool {
        self.appearance == Appearance::Dark
    }
}

/// `cx.ghd()` - read the active GitHub Desktop theme.
pub trait ActiveGhdTheme {
    fn ghd(&self) -> &GhdTheme;
}

impl ActiveGhdTheme for App {
    fn ghd(&self) -> &GhdTheme {
        self.global::<GhdTheme>()
    }
}

/// Call before `gpui_kit::init`: pre-create the kit theme with explicit font
/// families so `Theme::change` skips its installed-font enumeration.
#[allow(clippy::field_reassign_with_default)] // kit Theme has private fields
pub fn preseed_kit_theme(cx: &mut App) {
    if cx.has_global::<KitTheme>() {
        return;
    }
    let mut theme = KitTheme::default();
    theme.font_family = kit_ui_font();
    theme.mono_font_family = kit_mono_font();
    cx.set_global(theme);
}

/// Install the theme global and align gpui-kit's theme (used by Input,
/// Textarea, Scrollbar, Popover…) with GHD's palette, fonts and radius.
pub fn init(cx: &mut App, theme: GhdTheme) {
    apply(theme, cx);
}

pub fn apply(theme: GhdTheme, cx: &mut App) {
    let mode = if theme.is_dark() {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    KitTheme::change(mode, None, cx);
    // `update`, not `global_mut`: it carries the colours into the kit's
    // tokens and its Base projection, which paints the kit's scrollbars (a
    // Textarea's); edited in place they kept the stock palette's track and
    // thumb in dark mode
    KitTheme::update(cx, |kit| {
        kit.font_family = kit_ui_font();
        kit.font_size = sizes::FONT_SIZE();
        kit.mono_font_family = kit_mono_font();
        kit.mono_font_size = sizes::FONT_SIZE();
        kit.radius = sizes::BORDER_RADIUS();
        kit.radius_lg = sizes::BORDER_RADIUS();
        kit.shadow = false;
        kit.focus_ring = false;

        let c = &mut kit.colors;
        c.background = theme.background;
        c.foreground = theme.text;
        c.border = theme.box_border_contrast;
        // gpui-kit draws the Input border with `input`; GHD's textboxish uses the contrast border.
        c.input = theme.box_border_contrast;
        c.muted = theme.box_alt_background;
        c.muted_foreground = theme.text_secondary;
        c.primary = theme.button_background;
        c.primary_hover = theme.button_hover_background;
        c.primary_active = theme.button_hover_background;
        c.primary_foreground = theme.button_text;
        c.secondary = theme.secondary_button_background;
        c.secondary_hover = theme.secondary_button_hover_background;
        c.secondary_active = theme.secondary_button_hover_background;
        c.secondary_foreground = theme.secondary_button_text;
        c.accent = theme.box_hover_background;
        c.accent_foreground = theme.box_hover_text;
        c.ring = theme.focus;
        c.caret = theme.text;
        c.selection = theme.text_selection_background;
        c.link = theme.link;
        c.link_hover = theme.link_hover;
        c.link_active = theme.link_hover;
        c.popover = theme.box_background;
        c.popover_foreground = theme.text;
        c.list = theme.background;
        c.list_hover = theme.list_item_hover_background;
        c.list_active = theme.box_selected_background;
        c.list_active_border = theme.box_selected_background;
        c.list_even = theme.background;
        c.list_head = theme.box_alt_background;
        c.scrollbar = theme.background;
        c.scrollbar_thumb = theme.scroll_bar_thumb;
        c.scrollbar_thumb_hover = theme.scroll_bar_thumb_active;
        c.sidebar = theme.background;
        c.sidebar_foreground = theme.text;
        c.sidebar_border = theme.box_border;
        c.title_bar = theme.toolbar_background;
        c.title_bar_border = theme.toolbar_border;
        c.tab_bar = theme.tab_bar_background;
        c.tab = theme.tab_bar_background;
        c.tab_active = theme.tab_bar_background;
        c.tab_foreground = theme.text;
        c.tab_active_foreground = theme.text;
        c.danger = theme.error;
        c.danger_foreground = theme.button_text;
        c.success = theme.status_success;
        c.warning = theme.dialog_warning;
        c.info = theme.dialog_information;
        c.overlay = theme.overlay;
        c.skeleton = theme.box_alt_background;
        c.drag_border = theme.focus;
        c.drop_target = theme.text_field_focus_shadow;
    });
    cx.set_global(theme);
}

/// Hex → Hsla.
pub(crate) fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

/// Hex + alpha → Hsla.
pub(crate) fn ca(hex: u32, alpha: f32) -> Hsla {
    let rgba: Rgba = rgb(hex);
    let mut hsla: Hsla = rgba.into();
    hsla.a = alpha;
    hsla
}

/// Convenience for `px` in theme code.
#[allow(dead_code)]
pub(crate) const fn p(v: f32) -> Pixels {
    px(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use corvene_core::ThemeSetting;

    #[test]
    fn system_follows_increase_contrast() {
        let name = |setting, dark, contrast| GhdTheme::for_setting(setting, dark, contrast).name;
        assert_eq!(
            name(ThemeSetting::System, false, false),
            GhdTheme::light().name
        );
        assert_eq!(
            name(ThemeSetting::System, true, false),
            GhdTheme::dark().name
        );
        assert_eq!(name(ThemeSetting::System, false, true), "High Contrast");
        assert_eq!(name(ThemeSetting::Dark, true, true), GhdTheme::dark().name);
        assert_eq!(
            name(ThemeSetting::HighContrast, false, false),
            "High Contrast"
        );
        assert!(GhdTheme::high_contrast().is_dark());
    }
}
