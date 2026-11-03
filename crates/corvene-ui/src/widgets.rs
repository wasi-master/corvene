//! Small GHD-styled primitives: buttons, checkbox, counter badge, avatar, text box, kbd.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::Sizable;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::native_menu::NativeMenu;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::keyboard_nav::ControlFocus;
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, GhdTheme};

/// `.button-component` - secondary button (25 px, radius 6).
pub fn button(id: impl Into<ElementId>, label: impl Into<SharedString>, cx: &App) -> Stateful<Div> {
    let t = cx.ghd();
    let hover_bg = t.secondary_button_hover_background;
    let hover_border = t.secondary_button_hover_border;
    base_button(id, t)
        .bg(t.secondary_button_background)
        .border_color(t.secondary_button_border)
        .text_color(t.secondary_button_text)
        .hover(move |s| s.bg(hover_bg).border_color(hover_border))
        .child(label.into())
        .control_focus(cx)
}

/// `.button-component.small-button` - the secondary button at 21 px, 11 px
/// text, 5 px side padding (Undo in the changes sidebar, list row actions).
pub fn small_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    cx: &App,
) -> Stateful<Div> {
    button(id, label, cx)
        .h(zpx(21.))
        .px(SPACING_HALF())
        .text_size(FONT_SIZE_SM())
}

/// CSS `opacity: .6` on a button composites the whole button over what is
/// behind it; GPUI's `opacity` fades each primitive on its own, so a label
/// would blend with the already-faded fill. Disabled buttons therefore draw
/// each colour pre-blended over the page background.
pub fn faded(color: Hsla, backdrop: Hsla) -> Hsla {
    backdrop.blend(color.opacity(0.6))
}

/// `.button-component-primary` - blue primary button. Disabled = 60 % opacity
/// (`[aria-disabled=true]`), as a group.
pub fn primary_button(
    id: impl Into<ElementId>,
    label: impl IntoElement,
    disabled: bool,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.ghd();
    let hover_bg = t.button_hover_background;
    let (bg, text) = if disabled {
        (
            faded(t.button_background, t.background),
            faded(t.button_text, t.background),
        )
    } else {
        (t.button_background, t.button_text)
    };
    base_button(id, t)
        .bg(bg)
        .border_color(bg)
        .text_color(text)
        .when(disabled, |d| d.cursor_default())
        .when(!disabled, move |d| d.hover(move |s| s.bg(hover_bg)))
        // GHD's `aria-disabled` buttons stay focusable
        .control_focus(cx)
        .child(label)
}

/// [`button`] drawn disabled: its colours at 60 % over the page, no hover.
pub fn button_disabled(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.ghd();
    base_button(id, t)
        .bg(faded(t.secondary_button_background, t.background))
        .border_color(faded(t.secondary_button_border, t.background))
        .text_color(faded(t.secondary_button_text, t.background))
        .cursor_default()
        .child(label.into())
        .control_focus(cx)
}

fn base_button(id: impl Into<ElementId>, _t: &GhdTheme) -> Stateful<Div> {
    div()
        .id(id)
        .h(BUTTON_HEIGHT())
        .px(SPACING())
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .rounded(BORDER_RADIUS())
        .text_size(FONT_SIZE())
        .whitespace_nowrap()
        .cursor_pointer()
}

/// GHD `LinkButton`: link-coloured inline text, underlined on hover.
pub fn link_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.ghd();
    let hover = t.link_hover;
    // Settings › Accessibility › Underline links (`body.underline-links`).
    let underline =
        corvene_core::AppState::try_global(cx).is_some_and(|s| s.read(cx).settings.underline_links);
    let label: SharedString = label.into();
    div()
        .id(id)
        // a `Link` node VoiceOver can name and press
        .role(Role::Link)
        .aria_label(label.clone())
        .flex_none()
        .text_size(FONT_SIZE())
        .text_color(t.link)
        .cursor_pointer()
        .when(underline, |d| d.underline())
        .hover(move |s| s.text_color(hover).underline())
        .child(label)
        .control_focus(cx)
}

/// Shared handler for `select_button` choices (index of the picked option).
pub type SelectHandler = std::rc::Rc<dyn Fn(usize, &mut Window, &mut App)>;
/// Shared click handler.
pub type ClickAction = std::rc::Rc<dyn Fn(&mut Window, &mut App)>;

/// One piece of an inline paragraph.
pub enum Inline {
    Text(SharedString),
    Element(AnyElement),
}

impl From<&'static str> for Inline {
    fn from(s: &'static str) -> Self {
        Inline::Text(s.into())
    }
}

impl From<String> for Inline {
    fn from(s: String) -> Self {
        Inline::Text(s.into())
    }
}

impl From<AnyElement> for Inline {
    fn from(el: AnyElement) -> Self {
        Inline::Element(el)
    }
}

/// GHD `<p>` mixing text with `<LinkButton>` / `<Ref>` children. A flex-row
/// text child never shrinks in GPUI, so long sentences would overflow;
/// splitting the text into words gives real line wrapping around the inline
/// elements. Each word keeps the whitespace after it (and a part's leading
/// whitespace stays on its first word), so the gaps are the font's own space
/// advance at the inherited size, and text that touches an element with no
/// space (`"(" + chip`, `chip + "."`) stays attached. Words and elements are
/// [`InlineShift`]s sharing one [`InlineFlow`], which puts every word at its
/// exact advance like Chromium's inline layout.
pub fn paragraph(parts: Vec<Inline>) -> Div {
    let mut row = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .line_height(zpx(18.));
    let flow = Rc::new(Cell::new(InlineFlow::default()));
    let mut first = true;
    let mut push = |row: Div, child: InlineChild| {
        row.child(InlineShift::new(child, &flow, std::mem::take(&mut first)))
    };
    for part in parts {
        match part {
            Inline::Text(text) => {
                let mut start = 0;
                let mut in_word = false;
                let mut seen_word = false;
                for (i, ch) in text.char_indices() {
                    if ch.is_whitespace() {
                        if in_word {
                            in_word = false;
                        }
                    } else if !in_word {
                        // a new word: flush the previous word and its spaces
                        if seen_word {
                            let word = SharedString::from(text[start..i].to_string());
                            row = push(row, InlineChild::Word(word));
                            start = i;
                        }
                        in_word = true;
                        seen_word = true;
                    }
                }
                if start < text.len() {
                    let word = SharedString::from(text[start..].to_string());
                    row = push(row, InlineChild::Word(word));
                }
            }
            Inline::Element(el) => {
                row = push(row, InlineChild::Element(el));
            }
        }
    }
    row
}

/// The layout engine snaps every measured leaf to a whole device pixel
/// (words: to the nearest one), so a row of word boxes drifts by up to half
/// a pixel per word. The flow carries that drift along a line: each
/// element is moved left by the drift of the words before it on its line.
#[derive(Clone, Copy, Default)]
struct InlineFlow {
    /// Bottom of the line the previous element sat on.
    line_bottom: Pixels,
    /// Rounding accumulated by the words before on this line.
    drift: Pixels,
}

/// A [`paragraph`] child: a word (with its trailing spaces) shaped with the
/// inherited text style, its box its exact advance rounded up by the layout
/// engine only; or an element. Either is moved left by its line's
/// [`InlineFlow`] drift (at prepaint, so hit boxes move with it), and a word
/// adds its own rounding to the drift.
struct InlineShift {
    child: InlineChild,
    flow: Rc<Cell<InlineFlow>>,
    /// The paragraph's first child starts the flow afresh on every frame.
    first: bool,
}

enum InlineChild {
    Word(SharedString),
    Element(AnyElement),
}

impl InlineShift {
    fn new(child: InlineChild, flow: &Rc<Cell<InlineFlow>>, first: bool) -> Self {
        Self {
            child,
            flow: flow.clone(),
            first,
        }
    }
}

impl IntoElement for InlineShift {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for InlineShift {
    /// A word's shaped line, line height and layout node.
    type RequestLayoutState = Option<(ShapedLine, Pixels, LayoutId)>;
    /// A word's paint origin.
    type PrepaintState = Point<Pixels>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        match &mut self.child {
            InlineChild::Word(text) => {
                let style = window.text_style();
                let rem = window.rem_size();
                let font_size = style.font_size.to_pixels(rem);
                let line_height = style.line_height_in_pixels(rem);
                let run = style.to_run(text.len());
                let line = window
                    .text_system()
                    .shape_line(text.clone(), font_size, &[run], None);
                // Chromium breaks lines on exact widths. Off macOS layout
                // keeps measured widths to 1/64 px, so the word's own width
                // adds up along a line like Electron's; paint corrects the
                // snapping drift. (On macOS layout rounds every leaf up to a
                // device pixel, which its parity runs were tuned with.)
                let box_size = size(line.width, line_height);
                let layout_id =
                    window.request_measured_layout(Style::default(), move |_, _, _, _| box_size);
                (layout_id, Some((line, line_height, layout_id)))
            }
            InlineChild::Element(el) => (el.request_layout(window, cx), None),
        }
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        word: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Point<Pixels> {
        let mut flow = self.flow.get();
        // a box starting below the previous boxes of its line starts a new one
        if self.first || bounds.top() >= flow.line_bottom - px(0.5) {
            flow = InlineFlow {
                line_bottom: bounds.bottom(),
                drift: px(0.),
            };
        } else {
            flow.line_bottom = flow.line_bottom.max(bounds.bottom());
        }
        let offset = point(-flow.drift, px(0.));
        match (&mut self.child, word) {
            (InlineChild::Word(_), Some((line, _, layout_id))) => {
                flow.drift += bounds.size.width - line.width;
                // the fraction of a device pixel layout snapping took off
                // the box, as text elements paint (vendored gpui-pre)
                let unsnapped = window.unsnapped_layout_origin(*layout_id)
                    - window.layout_bounds(*layout_id).origin;
                self.flow.set(flow);
                return bounds.origin + offset + unsnapped;
            }
            (InlineChild::Element(el), _) => {
                window.with_element_offset(offset, |window| el.prepaint(window, cx));
            }
            _ => {}
        }
        self.flow.set(flow);
        bounds.origin + offset
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        word: &mut Self::RequestLayoutState,
        origin: &mut Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        match (&mut self.child, word) {
            (InlineChild::Word(_), Some((line, line_height, _))) => {
                // a failed glyph raster leaves the word blank, like `StyledText`
                let _ = line.paint(*origin, *line_height, TextAlign::Left, None, window, cx);
            }
            (InlineChild::Element(el), _) => el.paint(window, cx),
            _ => {}
        }
    }
}

/// `<Ref>`: inline monospace code on the alt background.
pub fn code_ref(text: impl Into<SharedString>, cx: &App) -> Div {
    let t = cx.ghd();
    // `.ref-component`: monospace on `--path-segment-background`, 6 px radius,
    // 3.33 px padding. The element is inline in GHD: its box is the 14 px
    // content area plus padding (20.7 px), painted over the 18 px line
    // without growing it - here 1.33 px beyond the line on each side.
    div()
        .font_family(crate::theme::mono_font())
        .px(SPACING_THIRD())
        .py(SPACING_THIRD() - zpx(2.))
        .my(zpx(2.) - SPACING_THIRD())
        .rounded(BORDER_RADIUS())
        .bg(t.path_segment_background)
        .child(text.into())
}

/// Dialog `h2` (`_dialog.scss`: 14 px semibold, 10 px below).
pub fn section_heading(text: impl Into<SharedString>, cx: &App) -> Div {
    let t = cx.ghd();
    div()
        .text_size(FONT_SIZE_MD())
        .line_height(zpx(21.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(t.text)
        .mb(SPACING())
        .child(text.into())
}

/// An emoji presentation sequence (`⚠️`): Chromium draws it with the
/// emoji font even where the UI font has a text glyph; GPUI's fallback
/// would take the text glyph, so off macOS the font is named.
pub fn emoji(text: &'static str) -> Div {
    let el = div().child(text);
    #[cfg(not(target_os = "macos"))]
    let el = el.font_family(SharedString::from(corvene_platform::fonts::emoji_family()));
    el
}

/// The colours of Chromium's `outline: auto` focus ring: the 2 px ring and
/// the 1 px line around it. Off macOS the ring is two-tone, `--focus-color`
/// inside a white line on a light page and white inside `#101010` on a dark
/// one; on macOS Chromium draws a single `--focus-color` ring, so both are
/// the focus colour.
pub fn focus_ring_colors(cx: &App) -> (Hsla, Hsla) {
    let t = cx.ghd();
    if cfg!(target_os = "macos") {
        return (t.focus, t.focus);
    }
    if t.background.l < 0.5 {
        (rgb(0xffffff).into(), rgb(0x101010).into())
    } else {
        (t.focus, rgb(0xffffff).into())
    }
}

/// Chromium's `outline: auto` focus ring in `--focus-color` for a
/// `.button-component` (`outline-offset` draws it 2 px outside the border):
/// absolutely placed in a `relative()` wrapper around the button. Off macOS
/// the ring is two-tone, a 1 px line around it (see
/// [`focus_ring_colors`]).
pub fn focus_ring(cx: &App) -> Div {
    // Windows: Chromium draws `outline: auto` in the system's focus colours
    // there, whatever `outline-color` says: a 2 px ring that contrasts with
    // the page (white on a dark one) inside a 1 px halo of the opposite
    #[cfg(windows)]
    if cfg!(windows) {
        let dark = cx.ghd().background.l < 0.5;
        let (ring, halo) = if dark {
            (rgb(0xffffff), rgb(0x101010))
        } else {
            (rgb(0x101010), rgb(0xffffff))
        };
        return div()
            .absolute()
            .top(zpx(-5.))
            .left(zpx(-5.))
            .right(zpx(-5.))
            .bottom(zpx(-5.))
            .border_1()
            .border_color(halo)
            .rounded(BORDER_RADIUS() + zpx(5.))
            .child(
                div()
                    .size_full()
                    .border_2()
                    .border_color(ring)
                    .rounded(BORDER_RADIUS() + zpx(4.)),
            );
    }
    div()
        .absolute()
        .top(zpx(-4.))
        .left(zpx(-4.))
        .right(zpx(-4.))
        .bottom(zpx(-4.))
        .border_2()
        .border_color(cx.ghd().focus)
        .rounded(BORDER_RADIUS() + zpx(4.))
}

/// `.settings-description`: 11 px secondary text, 10 px above.
pub fn settings_description(cx: &App) -> Div {
    let t = cx.ghd();
    div()
        .mt(SPACING())
        .text_size(FONT_SIZE_SM())
        .line_height(zpx(16.))
        .text_color(t.text_secondary)
}

/// `Checkbox` with its label (`.checkbox-component`): 13 px box, 5 px gap.
pub fn checkbox_row(
    id: &'static str,
    checked: bool,
    label: impl IntoElement,
    on_toggle: impl Fn(bool, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    checkbox_row_focus(id, checked, label, false, on_toggle, cx)
}

/// [`checkbox_row`] whose box may show Chromium's focus ring (a dialog's
/// autofocused first checkbox): 1 px gap, 2 px `--focus-color`, then a 1 px
/// dark halo.
pub fn checkbox_row_focus(
    id: &'static str,
    checked: bool,
    label: impl IntoElement,
    focused: bool,
    on_toggle: impl Fn(bool, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.ghd();
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap(SPACING_HALF())
        .cursor_pointer()
        .on_click(move |_, window, cx| on_toggle(!checked, window, cx))
        .control_focus(cx)
        .child(
            div()
                .relative()
                .flex_none()
                .when(focused, |d| {
                    d.child(
                        div()
                            .absolute()
                            .top(zpx(-4.))
                            .left(zpx(-4.))
                            .right(zpx(-4.))
                            .bottom(zpx(-4.))
                            .border_1()
                            .border_color(rgb(0x101010))
                            .rounded(zpx(6.))
                            .child(
                                div()
                                    .size_full()
                                    .border_2()
                                    .border_color(t.focus)
                                    .rounded(zpx(5.)),
                            ),
                    )
                })
                .child(checkbox(
                    ElementId::from(SharedString::from(format!("{id}-box"))),
                    checked,
                    false,
                    cx,
                )),
        )
        .child(div().flex_1().min_w_0().text_size(FONT_SIZE()).child(label))
}

/// Chromium's native `<input type="radio">` with GHD's `accent-color`:
/// 13 px circle, accent fill with a 5 px dot when selected.
pub fn radio(id: impl Into<ElementId>, selected: bool, cx: &App) -> Stateful<Div> {
    let t = cx.ghd();
    div()
        .id(id)
        .size(zpx(13.))
        .flex_none()
        .rounded_full()
        .border_1()
        .border_color(if selected { t.accent } else { t.control_border })
        .bg(if selected {
            t.accent
        } else {
            t.control_background
        })
        .flex()
        .items_center()
        .justify_center()
        .when(selected, |d| {
            d.child(div().size(zpx(5.)).rounded_full().bg(t.control_background))
        })
}

/// `RadioButton` row: radio + label, 5 px apart; rows 5 px apart.
pub fn radio_row(
    id: &'static str,
    selected: bool,
    label: impl IntoElement,
    on_select: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap(SPACING_HALF())
        .cursor_pointer()
        .on_click(move |_, window, cx| on_select(window, cx))
        .control_focus(cx)
        .child(radio(
            ElementId::from(SharedString::from(format!("{id}-radio"))),
            selected,
            cx,
        ))
        .child(div().flex_1().min_w_0().text_size(FONT_SIZE()).child(label))
}

thread_local! {
    /// [`app_icon`]'s answers: an image is decoded once.
    static APP_ICONS: std::cell::RefCell<std::collections::HashMap<String, Option<Arc<Image>>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

thread_local! {
    // Decoded integration icons by application path (flag `513`).
    static INTEGRATION_ICONS: std::cell::RefCell<std::collections::HashMap<std::path::PathBuf, Arc<Image>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// The GPUI image of an application icon the core loaded with the
/// integrations, decoded once per path.
pub fn integration_icon(
    path: &std::path::Path,
    icon: &corvene_platform::app_icons::AppIcon,
) -> Arc<Image> {
    INTEGRATION_ICONS.with(|icons| {
        icons
            .borrow_mut()
            .entry(path.to_path_buf())
            .or_insert_with(|| {
                let format = match icon.format {
                    corvene_platform::app_icons::IconFormat::Png => ImageFormat::Png,
                    corvene_platform::app_icons::IconFormat::Svg => ImageFormat::Svg,
                };
                Arc::new(Image::from_bytes(format, icon.bytes.clone()))
            })
            .clone()
    })
}

/// Android: the launcher icon of the application `key` names (a package, or
/// "package/class" for one activity), for a list of applications. `None`
/// elsewhere.
pub fn app_icon(key: &str) -> Option<Arc<Image>> {
    if !cfg!(target_os = "android") || key.is_empty() {
        return None;
    }
    APP_ICONS.with(|icons| {
        icons
            .borrow_mut()
            .entry(key.to_string())
            .or_insert_with(|| {
                corvene_platform::editors::app_icon(key)
                    .map(|png| Arc::new(Image::from_bytes(ImageFormat::Png, png)))
            })
            .clone()
    })
}

/// An entry of a `select_button_items` popup.
pub enum SelectItem {
    Option(SharedString),
    /// An option with a picture in front of its label ([`app_icon`]).
    IconOption(SharedString, Option<Arc<Image>>),
    /// `<option disabled>────</option>`: a native menu separator.
    Separator,
}

/// Native `<select>` as macOS renders it in GHD: a textboxish 25 px popup
/// button showing the current value with a ▾ caret; clicking opens a native
/// menu of `options` and calls `on_select(index)`.
pub fn select_button(
    id: impl Into<ElementId>,
    value: impl Into<SharedString>,
    options: Vec<SharedString>,
    selected: Option<usize>,
    disabled: bool,
    on_select: SelectHandler,
    cx: &App,
) -> Stateful<Div> {
    select_button_items(
        id,
        value,
        options.into_iter().map(SelectItem::Option).collect(),
        selected,
        disabled,
        on_select,
        cx,
    )
}

/// `select_button` with separators; `on_select` gets the index among the
/// `Option` items only.
pub fn select_button_items(
    id: impl Into<ElementId>,
    value: impl Into<SharedString>,
    items: Vec<SelectItem>,
    selected: Option<usize>,
    disabled: bool,
    on_select: SelectHandler,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.ghd();
    // `.select-component select`: contrast border, box background, no hover
    // style; `:disabled` takes the alt background and secondary text
    div()
        .id(id)
        .h(TEXT_FIELD_HEIGHT())
        .w_full()
        .min_w_0()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(SPACING_HALF())
        .pl(SPACING_HALF())
        .pr(zpx(3.5))
        .border_1()
        .rounded(BORDER_RADIUS())
        .bg(if disabled {
            t.box_alt_background
        } else {
            t.box_background
        })
        .border_color(t.box_border_contrast)
        .text_size(FONT_SIZE())
        .text_color(if disabled { t.text_secondary } else { t.text })
        .when(!disabled, |d| {
            d.cursor_pointer()
                .control_focus(cx)
                .on_click(move |ev: &ClickEvent, window, cx| {
                    let mut option_ix = 0;
                    let menu_items: Vec<crate::context_menu::MenuItem> = items
                        .iter()
                        .map(|item| match item {
                            SelectItem::Separator => crate::context_menu::MenuItem::separator(),
                            SelectItem::Option(label) | SelectItem::IconOption(label, _) => {
                                let on_select = on_select.clone();
                                let ix = option_ix;
                                option_ix += 1;
                                let icon = match item {
                                    SelectItem::IconOption(_, icon) => icon.clone(),
                                    _ => None,
                                };
                                crate::context_menu::MenuItem::checkbox(
                                    label.clone(),
                                    selected == Some(ix),
                                    move |window, cx| on_select(ix, window, cx),
                                )
                                .icon(icon)
                            }
                        })
                        .collect();
                    let position = ev.mouse_position().unwrap_or_default();
                    crate::native_menu::show_context_menu(menu_items, position, window, cx);
                })
        })
        .child(div().flex_1().min_w_0().truncate().child(value.into()))
        // Chromium's menulist chevron in the text colour
        .child(
            svg()
                .path("ui/select-chevron.svg")
                .flex_none()
                .w(zpx(9.))
                .h(zpx(5.3))
                .text_color(if disabled { t.text_secondary } else { t.text }),
        )
}

/// GHD `DialogError` (`.dialog-banner.dialog-error`): a full-width red band
/// under the dialog header. It cancels the content padding itself so it can
/// be the first child of any dialog content.
pub fn dialog_error_banner(message: impl Into<SharedString>, cx: &App) -> Div {
    let t = cx.ghd();
    div()
        .mx(zpx(-20.))
        .mt(zpx(-20.))
        .mb(SPACING_DOUBLE())
        .px(SPACING_DOUBLE())
        .py(SPACING())
        .bg(t.form_error_background)
        .border_t_1()
        .border_b_1()
        .border_color(t.form_error_border)
        .text_color(t.form_error_text)
        .text_size(FONT_SIZE())
        .line_height(zpx(18.))
        .child(message.into())
}

/// GHD `CallToAction`: text on the left, a ≥120 px primary button on the right.
pub fn call_to_action(
    id: &'static str,
    body: impl IntoElement,
    action_title: impl Into<SharedString>,
    on_action: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(SPACING())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(FONT_SIZE())
                .line_height(zpx(18.))
                .child(body),
        )
        .child(
            primary_button(id, action_title.into(), false, cx)
                .min_w(zpx(120.))
                .flex_none()
                .on_click(move |_, window, cx| on_action(window, cx)),
        )
}

/// 13 px checkbox. GHD renders a bare `<input type="checkbox">`, so this is
/// Chromium's native control (`ui/native_theme/native_theme_base.cc`
/// `PaintCheckbox`) with GHD's `accent-color` (`_globals.scss` `body`):
/// 2 px radius, 1 px border; checked = `accent` fill with the check drawn in
/// the control background colour; disabled = grey fill (GHD's "0 changed
/// files" state).
pub fn checkbox(
    id: impl Into<ElementId>,
    checked: bool,
    disabled: bool,
    cx: &App,
) -> Stateful<Div> {
    checkbox_tristate(id, Some(checked), disabled, cx)
}

/// `CheckboxValue::Mixed` (`None`) draws the accent fill with a dash, like
/// Chromium's indeterminate checkbox that GHD renders.
///
/// Hover/pressed tints of Chromium's native control are not reproduced.
pub fn checkbox_tristate(
    id: impl Into<ElementId>,
    value: Option<bool>,
    disabled: bool,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.ghd();
    let base = div()
        .id(id)
        .size(CHECKBOX_SIZE())
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(zpx(2.))
        .border_1()
        .when(!disabled, |d| d.cursor_pointer());
    let (fill, glyph) = match (value, disabled) {
        (Some(false), false) => {
            return base.bg(t.control_background).border_color(t.control_border);
        }
        (Some(false), true) => {
            return base
                .bg(t.control_disabled_background)
                .border_color(t.control_disabled_border);
        }
        (_, false) => (t.accent, t.control_background),
        (_, true) => (t.control_disabled_accent, t.control_disabled_glyph),
    };
    let path = if value.is_none() {
        "controls/checkbox-dash-13.svg"
    } else {
        "controls/checkbox-check-13.svg"
    };
    base.bg(fill).border_color(fill).child(
        svg()
            .path(path)
            .size(CHECKBOX_SIZE())
            .flex_none()
            .text_color(glyph),
    )
}

/// `.counter` pill used in the Changes tab and list rows.
pub fn counter(count: usize, cx: &App) -> Div {
    counter_text(count.to_string(), cx)
}

/// [`counter`] with its text given, such as the Changes tab's
/// [`crate::format::files_changed_badge`].
pub fn counter_text(text: impl Into<SharedString>, cx: &App) -> Div {
    let t = cx.ghd();
    div()
        .ml(zpx(4.))
        .px(zpx(5.))
        .py(zpx(2.))
        .rounded(zpx(20.))
        .bg(t.tab_bar_count_background)
        .text_color(t.tab_bar_count_text)
        .text_size(FONT_SIZE_XS())
        .font_weight(FontWeight::SEMIBOLD)
        .line_height(zpx(11.))
        .child(text.into())
}

/// `kbd` - one key cap: radius 6, base border, 1/2 px padding, min 16 px tall,
/// min-width 1.5em; [`kbd_group`] spaces a shortcut's caps.
pub fn kbd(key: impl Into<SharedString>, cx: &App) -> Div {
    let t = cx.ghd();
    div()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .min_h(zpx(16.))
        .min_w(zpx(18.))
        .px(zpx(2.))
        .py(zpx(1.))
        .rounded(BORDER_RADIUS())
        .border_1()
        .border_color(t.box_border_contrast.opacity(0.5))
        .bg(t.box_background)
        .text_size(FONT_SIZE())
        .line_height(zpx(12.))
        .text_color(t.text)
        .child(key.into())
}

/// GHD `getPlatformSpecificNameOrSymbolForModifier` for a key written the
/// macOS way: ⌘ (`CmdOrCtrl`) and ⌃ are Ctrl, ⇧ Shift and ⌥ Alt off macOS.
pub fn platform_key(key: &'static str) -> &'static str {
    if cfg!(target_os = "macos") {
        return key;
    }
    match key {
        "⌘" | "⌃" => "Ctrl",
        "⇧" => "Shift",
        "⌥" => "Alt",
        other => other,
    }
}

/// A row of key caps (GHD `KeyboardShortcut`), given the macOS way, e.g.
/// `["⌘", "⇧", "A"]`: 2 px apart on macOS (`_globals.scss` `kbd`, darwin);
/// elsewhere "Ctrl+Shift+A", the caps named ([`platform_key`]) and joined
/// by a `+`.
pub fn kbd_group(keys: &[&'static str], cx: &App) -> Div {
    kbd_group_sized(keys, FONT_SIZE(), cx)
}

/// The text of [`kbd_group`] (GHD `KeyboardShortcut`'s `textContent`):
/// `⌘⇧N` on macOS, `Ctrl+Shift+N` elsewhere, for the macOS `darwin_keys`.
pub fn keyboard_shortcut_text(darwin_keys: &[&'static str]) -> String {
    let keys: Vec<&str> = darwin_keys.iter().map(|k| platform_key(k)).collect();
    keys.join(if cfg!(target_os = "macos") { "" } else { "+" })
}

/// [`kbd_group`] in `size` text (the caps inherit it, as in a `.protip`).
pub fn kbd_group_sized(keys: &[&'static str], size: Pixels, cx: &App) -> Div {
    let row = div().flex().flex_row().items_center();
    if cfg!(target_os = "macos") {
        return row
            .gap(zpx(2.))
            .children(keys.iter().map(|k| kbd(*k, cx).text_size(size)));
    }
    row.text_size(size)
        .children(keys.iter().enumerate().flat_map(|(i, k)| {
            let plus = (i > 0).then(|| div().child("+").into_any_element());
            plus.into_iter()
                .chain([kbd(platform_key(k), cx).text_size(size).into_any_element()])
        }))
}

/// A 32 × 18 toggle switch on GHD tokens (no GHD equivalent; the Flags
/// dialog): the primary button's blue track when on, the control border
/// grey when off, a white 14 px knob either way (macOS switches), so both
/// states read at a glance in every theme. Callers add `.aria_label(..)`.
pub fn switch(
    id: impl Into<ElementId>,
    checked: bool,
    disabled: bool,
    on_toggle: impl Fn(bool, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.ghd();
    let white = gpui_kit::white();
    let (track, knob) = match (checked, disabled) {
        (true, false) => (t.button_background, white),
        (true, true) => (t.control_disabled_accent, t.control_disabled_glyph),
        (false, false) => (t.control_border, white),
        (false, true) => (t.control_disabled_border, t.control_disabled_background),
    };
    div()
        .id(id)
        .role(Role::Switch)
        .aria_toggled(if checked {
            Toggled::True
        } else {
            Toggled::False
        })
        .flex_none()
        .w(zpx(32.))
        .h(zpx(18.))
        .rounded(zpx(9.))
        .bg(track)
        .p(zpx(2.))
        .flex()
        .flex_row()
        .items_center()
        .when(checked, |d| d.justify_end())
        .when(disabled, |d| d.opacity(0.6))
        .when(!disabled, |d| {
            d.cursor_pointer()
                .on_click(move |_, window, cx| on_toggle(!checked, window, cx))
                .control_focus(cx)
        })
        .child(
            div()
                .size(zpx(14.))
                .rounded_full()
                .bg(knob)
                .shadow(vec![BoxShadow {
                    color: gpui_kit::black().opacity(0.25),
                    offset: point(zpx(0.), zpx(0.5)),
                    blur_radius: zpx(1.),
                    spread_radius: zpx(0.),
                    inset: false,
                }]),
        )
}

/// An 18 px pill with an optional 12 px icon (the Flags dialog's id,
/// restart, lock and unavailable markers).
pub fn pill(
    label: impl Into<SharedString>,
    icon: Option<crate::icons::Octicon>,
    background: Hsla,
    text: Hsla,
    cx: &App,
) -> Div {
    let _ = cx;
    div()
        .flex_none()
        .h(zpx(18.))
        .px(SPACING_HALF())
        .rounded(zpx(9.))
        .bg(background)
        .text_color(text)
        .text_size(FONT_SIZE_SM())
        .line_height(zpx(18.))
        .flex()
        .flex_row()
        .items_center()
        .gap(SPACING_THIRD())
        .when_some(icon, |d, icon| {
            d.child(crate::icons::octicon(icon, text).size(zpx(12.)))
        })
        .child(label.into())
}

/// GHD `textboxish` chrome around a gpui-kit `Input`: 25 px, contrast border,
/// radius 6, `box_background`, 0/5 px padding, blue border + 1 px halo on focus.
pub fn text_box(
    id: impl Into<ElementId>,
    state: &Entity<InputState>,
    prefix: Option<Svg>,
    window: &Window,
    cx: &App,
) -> Stateful<Div> {
    text_box_with_menu(id, state, prefix, None, window, cx)
}

/// [`text_box`] that can be disabled (dimmed, no typing).
pub fn text_box_opts(
    id: impl Into<ElementId>,
    state: &Entity<InputState>,
    prefix: Option<Svg>,
    disabled: bool,
    window: &Window,
    cx: &App,
) -> Stateful<Div> {
    text_box_impl(id, state, prefix, None, disabled, window, cx)
}

/// A 1 px CSS border as Chromium lays it out: whole device pixels, at least
/// one (1 device pixel, two thirds of a CSS pixel, at 150 %).
pub fn hairline(window: &Window) -> Pixels {
    let scale = window.scale_factor();
    px(scale.floor().max(1.) / scale)
}

thread_local! {
    /// The filter boxes whose ✕ emptied them, until they get a value again
    /// (GHD `TextBox` `valueCleared`).
    static CLEARED_BOXES: std::cell::RefCell<std::collections::HashSet<EntityId>> =
        Default::default();
}

/// GHD `TextBox` with `displayClearButton` (filter lists, the changes
/// filter, compare, diff search): the text stops 25 px before the end and a
/// 25 px ✕ button (`button.clear-button`, `--text-color`) clears the value
/// while there is one, after which an `AriaLiveContainer` announces "Input
/// cleared" until something is typed.
pub fn filter_text_box(
    id: impl Into<ElementId>,
    state: &Entity<InputState>,
    prefix: Option<Svg>,
    window: &Window,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.ghd();
    let has_value = !state.read(cx).value().is_empty();
    let clear_state = state.clone();
    let entity = state.entity_id();
    let cleared = CLEARED_BOXES.with(|c| {
        let mut c = c.borrow_mut();
        if has_value {
            c.remove(&entity);
        }
        c.contains(&entity)
    });
    text_box_with_menu(id, state, prefix, None, window, cx)
        .relative()
        // `padding-inline-end: var(--text-field-height)`, less the kit's 4 px
        .pr(TEXT_FIELD_HEIGHT() - zpx(4.))
        .when(cleared, |d| {
            d.child(
                div()
                    .id("input-cleared")
                    .absolute()
                    .size_0()
                    .overflow_hidden()
                    .a11y_live("Input cleared"),
            )
        })
        .when(has_value, |d| {
            d.child(
                div()
                    .id("clear-button")
                    .icon_button_label("Clear")
                    .absolute()
                    .right(zpx(0.))
                    .top(zpx(-1.))
                    .size(TEXT_FIELD_HEIGHT())
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(crate::icons::octicon(crate::icons::Octicon::X, t.text))
                    .on_click(move |_, window, cx| {
                        CLEARED_BOXES.with(|c| c.borrow_mut().insert(entity));
                        clear_state.update(cx, |input, cx| input.set_value("", window, cx));
                    }),
            )
        })
}

/// GHD `PasswordTextBox` (`ui/lib/password-text-box.tsx`): a [`text_box`]
/// over a masked `state` whose text stops `--spacing-triple` (30 px) before
/// the end, where a 30 px button (`.password-text-box button`: no border or
/// fill, `--text-secondary-color` icon) shows `eye-closed` while the value
/// is hidden and `eye` while it shows. Clicking flips the mask and puts focus
/// back in the field.
pub fn password_text_box(
    id: impl Into<ElementId>,
    state: &Entity<InputState>,
    window: &Window,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.ghd();
    let shown = !state.read(cx).presentation().is_masked();
    let toggle_state = state.clone();
    text_box_with_menu(id, state, None, None, window, cx)
        .relative()
        // `padding-right: var(--spacing-triple)`, less the kit's 4 px
        .pr(zpx(26.))
        .child(
            // a `Button` tooltip: north of the button by default
            with_directed_tooltip(
                div()
                    .id("toggle-password-visibility")
                    .a11y_button("Toggle password visibility"),
                "Toggle password visibility",
                TooltipDirection::North,
            )
            .aria_toggled(if shown { Toggled::True } else { Toggled::False })
            .absolute()
            .right(zpx(-1.))
            .top(zpx(-1.))
            .w(zpx(30.))
            .h(TEXT_FIELD_HEIGHT())
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .child(crate::icons::octicon(
                if shown {
                    crate::icons::Octicon::Eye
                } else {
                    crate::icons::Octicon::EyeClosed
                },
                t.text_secondary,
            ))
            .on_click(move |_, window, cx| {
                toggle_state.update(cx, |input, cx| {
                    input.toggle_masked(window, cx);
                    input.focus(window, cx);
                });
            }),
        )
}

/// A custom right-click menu for an input (the kit's native edit menu is
/// replaced wholesale, so builders add Cut/Copy/Paste themselves).
pub type InputMenuBuilder = std::rc::Rc<dyn Fn(NativeMenu, &mut Window, &mut App) -> NativeMenu>;

/// [`text_box`] with an optional context-menu builder.
pub fn text_box_with_menu(
    id: impl Into<ElementId>,
    state: &Entity<InputState>,
    prefix: Option<Svg>,
    menu: Option<InputMenuBuilder>,
    window: &Window,
    cx: &App,
) -> Stateful<Div> {
    text_box_impl(id, state, prefix, menu, false, window, cx)
}

#[allow(clippy::too_many_arguments)]
fn text_box_impl(
    id: impl Into<ElementId>,
    state: &Entity<InputState>,
    prefix: Option<Svg>,
    menu: Option<InputMenuBuilder>,
    disabled: bool,
    window: &Window,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.ghd();
    let focused = state.read(cx).focus_handle(cx).is_focused(window);
    div()
        .id(id)
        .when(disabled, |d| d.opacity(0.6))
        .h(TEXT_FIELD_HEIGHT())
        .w_full()
        .min_w_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(SPACING_HALF())
        // `input { padding: 0 5px }`: the kit's xsmall input pads 4 px on
        // each side itself, so the frame adds 1 px (and a prefix icon gets
        // its own 5 px)
        .pl(if prefix.is_some() {
            SPACING_HALF()
        } else {
            zpx(1.)
        })
        .pr(zpx(1.))
        .border_1()
        .rounded(BORDER_RADIUS())
        .bg(t.box_background)
        .border_color(if focused {
            t.focus
        } else {
            t.box_border_contrast
        })
        .when(focused, |d| {
            d.shadow(vec![BoxShadow {
                color: t.text_field_focus_shadow,
                offset: point(zpx(0.), zpx(0.)),
                blur_radius: zpx(0.),
                spread_radius: zpx(1.),
                inset: false,
            }])
        })
        .when_some(prefix, |d, icon| d.child(icon))
        .child(
            div()
                .flex_1()
                .min_w_0()
                // GHD inputs use the body font size (`--font-size`, 12 px);
                // the kit's `xsmall` would shrink text + placeholder to `text_xs`.
                .child({
                    let input = Input::new(state)
                        .appearance(false)
                        .xsmall()
                        .disabled(disabled)
                        .text_size(FONT_SIZE());
                    match menu {
                        Some(build) => {
                            input.context_menu(move |menu, window, cx| build(menu, window, cx))
                        }
                        None => input,
                    }
                }),
        )
}

/// `TextBox` label above a field (`.text-box-component > label`, 3.33 px gap).
pub fn labeled(label: impl Into<SharedString>, field: impl IntoElement, cx: &App) -> Div {
    let t = cx.ghd();
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(SPACING_THIRD())
        .child(
            div()
                .text_size(FONT_SIZE())
                .text_color(t.text)
                .child(label.into()),
        )
        .child(field)
}

/// `Avatar`: the cached image when resolved, else the grey placeholder.
pub fn avatar_image(path: Option<std::path::PathBuf>, size: Pixels, cx: &App) -> AnyElement {
    match path {
        Some(path) => img(path)
            .size(size)
            .flex_none()
            .rounded_full()
            .into_any_element(),
        None => avatar_placeholder(size, cx).into_any_element(),
    }
}

/// A commit author's avatar: the cached image, else (with
/// `112-initials-avatars`) their initials on a colour derived from the
/// e-mail, else GHD's placeholder.
pub fn author_avatar(name: &str, email: &str, size: Pixels, cx: &App) -> AnyElement {
    let path = avatar_lookup(email, cx);
    let initials_on = corvene_core::AppState::try_global(cx).is_some_and(|s| {
        s.read(cx)
            .flags
            .bool(corvene_core::flags::ids::INITIALS_AVATARS)
    });
    let text = corvene_core::initials(name, email);
    if path.is_some() || !initials_on || text.is_empty() {
        return avatar_image(path, size, cx);
    }
    let hue = corvene_core::initials_hue(email) as f32 / 360.;
    div()
        .size(size)
        .flex_none()
        .rounded_full()
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center()
        .bg(hsla(hue, 0.45, 0.42, 1.))
        .text_color(white())
        .text_size(size * if text.chars().count() > 1 { 0.42 } else { 0.55 })
        .font_weight(FontWeight::SEMIBOLD)
        .line_height(size)
        .child(text)
        .into_any_element()
}

/// GHD `MaxDisplayedAvatars`: stacked before the rest hide behind
/// `.avatar-more` until hover.
const MAX_DISPLAYED_AVATARS: usize = 3;

/// GHD `AvatarStack` (`--small`, `_avatar-stack.scss`) in front of `label`:
/// 16 px avatars overlapping by 11 px, the first on top, each with a 1 px
/// `--background-color` edge on its right. Hovering spreads them 1 px apart
/// and, past four users, swaps `.avatar-more` for the hidden ones. The stack
/// body is absolute over `label`, which keeps the stack's `min-width` as
/// left padding, so spread avatars paint over the text as in GHD.
pub fn avatar_stack(
    id: impl Into<ElementId>,
    users: &[corvene_core::AvatarUser],
    label: impl IntoElement,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.ghd();
    // the innermost group of a name wins, so every stack can share it
    let group = SharedString::from("avatar-stack");
    let more = users.len() > MAX_DISPLAYED_AVATARS + 1;
    let min_width = match users.len() {
        0 | 1 => 20.,
        2 => 25.,
        3 => 30.,
        _ => 40.,
    };
    // Each avatar advances by its 16 px width plus `margin-right` (-11 px,
    // 1 px on hover) and overflows its slot; the row runs right to left in
    // reverse so earlier avatars paint over later ones (GHD's z-indexes)
    let shadow = t.background;
    // each `.avatar-container` carries its own north tooltip (GHD `Avatar`
    // `TooltippedContent`), anchored to the 16 px avatar, not the advance
    let slot = |key: usize, user: &corvene_core::AvatarUser| {
        div()
            .id(("avatar", key))
            .flex_none()
            .h(zpx(16.))
            .w(zpx(5.))
            .group_hover(group.clone(), |s| s.w(zpx(17.)))
            .child(with_user_info_tooltip(
                // `.avatar-container { display: flex }`, which also puts the
                // tooltip's bounds probe at the avatar's top left
                div()
                    .id(("avatar-tooltip", key))
                    .flex()
                    .size(zpx(16.))
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left(zpx(1.))
                            .size(zpx(16.))
                            .rounded_full()
                            .bg(shadow),
                    )
                    .child(author_avatar(&user.name, &user.email, zpx(16.), cx)),
                user.name.clone(),
                user.email.clone(),
            ))
    };
    let mut slots: Vec<AnyElement> = Vec::new();
    for (i, user) in users.iter().enumerate() {
        if more && i == MAX_DISPLAYED_AVATARS - 1 {
            // `.avatar-more`: a 10 px sliver of stacked grey ellipses
            let ellipse = |w: f32, color: u32| {
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w(zpx(w))
                    .h(zpx(16.))
                    .rounded_full()
                    .bg(crate::theme::c(color))
            };
            slots.push(
                div()
                    .id("avatar-more")
                    .flex_none()
                    .relative()
                    .w(zpx(10.))
                    .h(zpx(16.))
                    .group_hover(group.clone(), |s| s.hidden())
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left(zpx(1.))
                            .w(zpx(10.))
                            .h(zpx(16.))
                            .rounded_full()
                            .bg(shadow),
                    )
                    .child(ellipse(10., crate::theme::primer::GRAY_100))
                    .child(ellipse(13., crate::theme::primer::GRAY_200))
                    .child(ellipse(14., crate::theme::primer::GRAY_300))
                    .into_any_element(),
            );
        }
        // `.avatar-container:nth-child(n + 5)` hides until hover
        let child_ix = i + 1 + usize::from(more && i >= MAX_DISPLAYED_AVATARS - 1);
        let hidden = child_ix >= 5;
        slots.push(
            slot(i, user)
                .when(hidden, |d| {
                    d.hidden()
                        .group_hover(group.clone(), |s| s.flex().w(zpx(17.)))
                })
                .into_any_element(),
        );
    }
    slots.reverse();
    div()
        .id(id)
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .min_w_0()
        .child(div().flex_1().min_w_0().pl(zpx(min_width)).child(label))
        .child(
            div()
                .group(group.clone())
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                // hovering anywhere over the stack spreads it, not just
                // over the 5 px advances
                .min_w(zpx(min_width))
                .flex()
                .flex_row_reverse()
                // physical left: `justify_end` is CSS `end`, not `flex-end`
                .justify_start()
                .items_center()
                .children(slots),
        )
}

/// Cached avatar for a commit e-mail (request it with
/// `Dispatcher::request_avatar_for_email` from a render with `&mut App`).
pub fn avatar_lookup(email: &str, cx: &App) -> Option<std::path::PathBuf> {
    corvene_core::AppState::try_global(cx)
        .and_then(|s| corvene_core::avatar_for_email(&s.read(cx).avatars, email))
}

/// Cached avatar for an account `avatar_url`.
pub fn avatar_lookup_url(url: &str, cx: &App) -> Option<std::path::PathBuf> {
    corvene_core::AppState::try_global(cx)
        .and_then(|s| corvene_core::avatar_for_url(&s.read(cx).avatars, url))
}

/// GHD `Avatar` without an image (none known, or it failed to load):
/// `DefaultAvatarSymbol`, the padded person octicon, as `svg.avatar` - the
/// secondary text colour on the alt box background, round.
pub fn avatar_placeholder(size: Pixels, cx: &App) -> Div {
    let t = cx.ghd();
    div()
        .size(size)
        .flex_none()
        .rounded_full()
        .overflow_hidden()
        .bg(t.box_alt_background)
        .child(
            svg()
                .path("ui/default-avatar.svg")
                .size(size)
                .text_color(t.text_secondary),
        )
}

/// `VerticalSegmentedControl` option (`.radio-button-component`): 10 px
/// padded label, a native radio (4 px down, 5 px either side), the bold title
/// and secondary description 5 px after it. The selected option holds the
/// radio's focus, so it takes `:has(input:focus)` - the hover colours, the
/// description included.
pub fn segmented_option(
    id: impl Into<ElementId>,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    selected: bool,
    first: bool,
    last: bool,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.ghd();
    let hover_bg = t.box_hover_background;
    let hover_text = t.box_hover_text;
    let id: ElementId = id.into();
    let radio_id = ElementId::from(SharedString::from(format!("{id}-radio")));
    let description_color = if selected {
        hover_text
    } else {
        t.text_secondary
    };
    div()
        .id(id)
        .w_full()
        .flex()
        .flex_row()
        .items_start()
        .p(SPACING())
        .border_1()
        .border_color(t.box_border)
        .when(!last, |d| d.border_b_0())
        .when(first, |d| d.rounded_t(BORDER_RADIUS()))
        .when(last, |d| d.rounded_b(BORDER_RADIUS()))
        .cursor_pointer()
        .when(selected, |d| d.bg(hover_bg).text_color(hover_text))
        .hover(move |s| s.bg(hover_bg).text_color(hover_text))
        .child(radio(radio_id, selected, cx).mx(SPACING_HALF()).mt(zpx(4.)))
        .child(
            div()
                .ml(SPACING_HALF())
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .line_height(zpx(18.))
                .child(div().font_weight(FontWeight::SEMIBOLD).child(title.into()))
                .child(
                    div()
                        .text_color(description_color)
                        .child(description.into()),
                ),
        )
}

/// GHD `.tool-tip-contents` (darwin): the small dark caption below a control.
struct TextTooltip {
    /// Lines separated by `\n`.
    text: SharedString,
    /// A `<strong>` byte range of `text`.
    bold: Option<std::ops::Range<usize>>,
    /// `direction` + the target's bounds; `None` anchors to the pointer.
    anchor: Option<(Bounds<Pixels>, TooltipDirection)>,
    /// Always the 300 px maximum wide, so changing text does not resize it.
    fixed_width: bool,
    /// GHD `.length-hint-tooltip`: no maximum width (`max-width: none`, the
    /// title `white-space: pre`) and the lines after the first at 0.7
    /// opacity (`.description`).
    titled: bool,
}

/// GHD `DefaultTooltipDelay` (`ui/lib/tooltip.tsx`).
pub const TOOLTIP_DELAY: std::time::Duration = std::time::Duration::from_millis(400);

/// GHD `TooltipDirection`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TooltipDirection {
    North,
    NorthEast,
    NorthWest,
    South,
    SouthEast,
    SouthWest,
    East,
    West,
}

/// GHD `getTooltipRectRelativeTo`: the tooltip's rect for `direction`
/// around `target`, leaving room for the 6 px arrow.
fn tooltip_rect(
    target: Bounds<Pixels>,
    direction: TooltipDirection,
    size: Size<Pixels>,
) -> Bounds<Pixels> {
    use TooltipDirection::*;
    let (tip_x, tip_w) = (px(10.), px(6.));
    let x_mid = target.left() + target.size.width / 2.;
    let y_top = target.top() - size.height;
    let y_mid = target.top() + target.size.height / 2. - size.height / 2.;
    let origin = match direction {
        NorthEast => point(x_mid - tip_x - tip_w, y_top - tip_w),
        North => point(x_mid - size.width / 2., y_top - tip_w),
        NorthWest => point(x_mid - size.width + tip_x + tip_w, y_top - tip_w),
        East => point(target.right() + tip_w, y_mid),
        SouthEast => point(x_mid - tip_x - tip_w, target.bottom() + tip_w),
        South => point(x_mid - size.width / 2., target.bottom() + tip_w),
        SouthWest => point(x_mid - size.width + tip_x + tip_w, target.bottom() + tip_w),
        West => point(target.left() - size.width - tip_w, y_mid),
    };
    Bounds::new(origin, size)
}

/// GHD `getDirection`: the desired direction when it fits, else the others
/// (its own side first); pointer-anchored tooltips try south-east, then
/// north-east. Falls back to south.
fn tooltip_direction(
    desired: Option<TooltipDirection>,
    target: Bounds<Pixels>,
    window: Bounds<Pixels>,
    size: Size<Pixels>,
) -> TooltipDirection {
    use TooltipDirection::*;
    let fits = |d: TooltipDirection| {
        let r = tooltip_rect(target, d, size);
        r.left() >= window.left()
            && r.top() >= window.top()
            && r.right() <= window.right()
            && r.bottom() <= window.bottom()
    };
    let all = [
        North, NorthEast, NorthWest, South, SouthEast, SouthWest, East, West,
    ];
    let mut order: Vec<TooltipDirection> = match desired {
        Some(d) if matches!(d, South | SouthEast | SouthWest) => {
            vec![d, South, SouthEast, SouthWest]
        }
        Some(d) if matches!(d, North | NorthEast | NorthWest) => {
            vec![d, North, NorthEast, NorthWest]
        }
        Some(d) => vec![d],
        None => vec![SouthEast, NorthEast],
    };
    for d in all {
        if !order.contains(&d) {
            order.push(d);
        }
    }
    order.into_iter().find(|d| fits(*d)).unwrap_or(South)
}

impl Render for TextTooltip {
    // GHD `Tooltip` without a `direction`: placed around a 20 px square at the
    // pointer (`mouseRect`), 300 px at most, 5 / 10 px padding, 11 px text,
    // a 6 px arrow pointing back at the pointer.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd();
        let mouse = window.mouse_position();
        let (target, desired) = match self.anchor {
            Some((bounds, direction)) => (bounds, Some(direction)),
            None => (
                Bounds::new(mouse - point(px(10.), px(10.)), size(px(20.), px(20.))),
                None,
            ),
        };
        let viewport = crate::theme::page_bounds(window);
        let font_size = FONT_SIZE_SM();
        let line_height = font_size * 1.5;
        let (pad_x, pad_y) = (SPACING(), SPACING_HALF());
        let max_text = if self.titled {
            viewport.size.width - pad_x * 2.
        } else {
            zpx(300.) - pad_x * 2.
        };
        let mut style = window.text_style();
        style.font_size = font_size.into();
        // Chromium's line breaking (`word-break: break-word`), not GPUI's
        let measure = |t: &str| -> Pixels {
            window
                .text_system()
                .shape_line(
                    SharedString::from(t.to_string()),
                    font_size,
                    &[style.to_run(t.len())],
                    None,
                )
                .width
        };
        let (display, breaks) = break_word_lines(&self.text, max_text, &measure);
        let widest = display
            .split('\n')
            .map(measure)
            .fold(px(0.), |a, b| a.max(b));
        let line_count = display.split('\n').count();
        let text_size = size(widest.min(max_text), line_height * line_count as f32);
        // the bold range moves right by the line breaks inserted before it
        let shift = |i: usize| i + breaks.iter().filter(|b| **b < i).count();
        let bold = self.bold.clone().map(|r| shift(r.start)..shift(r.end));
        let dim = self
            .titled
            .then(|| display.find('\n').map(|i| i + 1..display.len()))
            .flatten();
        let display: SharedString = display.into();
        let text_width = if self.fixed_width {
            max_text
        } else {
            text_size.width.ceil()
        };
        let box_size = size(text_width + pad_x * 2., text_size.height + pad_y * 2.);
        let direction = tooltip_direction(desired, target, viewport, box_size);
        let rect = tooltip_rect(target, direction, box_size);
        tooltip_frame(
            direction,
            rect,
            StyledText::new(display).with_highlights(
                bold.map(|range| {
                    (
                        range,
                        HighlightStyle {
                            font_weight: Some(FontWeight::BOLD),
                            ..Default::default()
                        },
                    )
                })
                .into_iter()
                .chain(dim.map(|range| {
                    (
                        range,
                        HighlightStyle {
                            fade_out: Some(0.3),
                            ..Default::default()
                        },
                    )
                })),
            ),
            t,
        )
    }
}

/// GHD `.tooltip` chrome for a box at `rect`: 5 / 10 px padding, 11 px
/// text, the shadow and the 6 px arrow on the side facing the target.
fn tooltip_frame(
    direction: TooltipDirection,
    rect: Bounds<Pixels>,
    content: impl IntoElement,
    t: &GhdTheme,
) -> impl IntoElement {
    use TooltipDirection::*;
    let box_size = rect.size;
    let font_size = FONT_SIZE_SM();
    let bg = t.tooltip_background;
    // `::before`: a 12 × 6 triangle outside the box
    let arrow = |path: &'static str, w: f32, h: f32| {
        svg()
            .path(path)
            .w(zpx(w))
            .h(zpx(h))
            .text_color(bg)
            .absolute()
    };
    let arrow = match direction {
        SouthEast => arrow("ui/tooltip-arrow-up.svg", 12., 6.)
            .top(zpx(-6.))
            .left(zpx(10.)),
        South => arrow("ui/tooltip-arrow-up.svg", 12., 6.)
            .top(zpx(-6.))
            .left(box_size.width / 2. - zpx(6.)),
        SouthWest => arrow("ui/tooltip-arrow-up.svg", 12., 6.)
            .top(zpx(-6.))
            .right(zpx(10.)),
        NorthEast => arrow("ui/tooltip-arrow-down.svg", 12., 6.)
            .bottom(zpx(-6.))
            .left(zpx(10.)),
        North => arrow("ui/tooltip-arrow-down.svg", 12., 6.)
            .bottom(zpx(-6.))
            .left(box_size.width / 2. - zpx(6.)),
        NorthWest => arrow("ui/tooltip-arrow-down.svg", 12., 6.)
            .bottom(zpx(-6.))
            .right(zpx(10.)),
        East => arrow("ui/tooltip-arrow-left.svg", 6., 12.)
            .left(zpx(-6.))
            .top(box_size.height / 2. - zpx(6.)),
        West => arrow("ui/tooltip-arrow-right.svg", 6., 12.)
            .right(zpx(-6.))
            .top(box_size.height / 2. - zpx(6.)),
    };
    anchored().position(rect.origin.map(|v| v.round())).child(
        div()
            .relative()
            .w(box_size.width)
            .px(SPACING())
            .py(SPACING_HALF())
            .rounded(BORDER_RADIUS())
            .bg(bg)
            .text_color(t.tooltip_text)
            .text_size(font_size)
            .line_height(font_size * 1.5)
            .shadow(vec![BoxShadow {
                color: t.tooltip_shadow,
                offset: point(zpx(0.), zpx(8.)),
                blur_radius: css_blur(24.),
                spread_radius: zpx(0.),
                inset: false,
            }])
            .child(arrow)
            .child(content),
    )
}

/// GHD `Avatar`'s own tooltip (`.tooltip.user-info`, `avatar.tsx`
/// `getTitle`): a 32 px avatar, 10 px, then the bold name over the e-mail,
/// with 5 px more padding above and below.
struct UserInfoTooltip {
    name: SharedString,
    email: SharedString,
    anchor: (Bounds<Pixels>, TooltipDirection),
}

impl Render for UserInfoTooltip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd();
        let (target, desired) = self.anchor;
        let viewport = crate::theme::page_bounds(window);
        let font_size = FONT_SIZE_SM();
        let line_height = font_size * 1.5;
        let avatar = zpx(32.);
        let max_text = zpx(300.) - SPACING() * 3. - avatar;
        let style = window.text_style();
        let window = &*window;
        let measure_with = |weight: FontWeight| {
            let mut run = style.clone();
            run.font_weight = weight;
            move |s: &str| -> Pixels {
                window
                    .text_system()
                    .shape_line(
                        SharedString::from(s.to_string()),
                        font_size,
                        &[run.to_run(s.len())],
                        None,
                    )
                    .width
            }
        };
        let lines = |text: &str, weight: FontWeight| {
            let measure = measure_with(weight);
            let (display, _) = break_word_lines(text, max_text, &measure);
            let widest = display
                .split('\n')
                .map(&measure)
                .fold(px(0.), |a, b| a.max(b));
            let count = display.split('\n').count();
            (display, widest, count)
        };
        let (name, name_w, name_lines) = lines(&self.name, FontWeight::BOLD);
        let (email, email_w, email_lines) = lines(&self.email, style.font_weight);
        let text_width = name_w.max(email_w).min(max_text).ceil();
        let text_height = line_height * (name_lines + email_lines) as f32;
        let content_height = text_height.max(avatar);
        let box_size = size(
            SPACING() * 3. + avatar + text_width,
            content_height + SPACING_HALF() * 4.,
        );
        let direction = tooltip_direction(Some(desired), target, viewport, box_size);
        let rect = tooltip_rect(target, direction, box_size);
        tooltip_frame(
            direction,
            rect,
            div()
                .flex()
                .flex_row()
                .py(SPACING_HALF())
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .mr(SPACING())
                        .child(author_avatar(&self.name, &self.email, avatar, cx)),
                )
                .child(
                    div()
                        .w(text_width)
                        .child(div().font_weight(FontWeight::BOLD).child(name))
                        .child(div().child(email)),
                ),
            t,
        )
    }
}

/// GHD `Avatar` with a tooltip: north of `el` after [`TOOLTIP_DELAY`], the
/// user-info card for a named user, else the e-mail as a caption.
pub fn with_user_info_tooltip(
    el: Stateful<Div>,
    name: impl Into<SharedString>,
    email: impl Into<SharedString>,
) -> Stateful<Div> {
    let (name, email): (SharedString, SharedString) = (name.into(), email.into());
    if name.is_empty() {
        let text = if email.is_empty() {
            SharedString::from("Unknown user")
        } else {
            email
        };
        return with_directed_tooltip(el, text, TooltipDirection::North);
    }
    let bounds = std::rc::Rc::new(std::cell::Cell::new(Bounds::default()));
    let probe = bounds.clone();
    el.relative()
        .child(
            canvas(move |b, _, _| probe.set(b), |_, _, _, _| {})
                .absolute()
                .size_full(),
        )
        .tooltip(move |_, cx| {
            cx.new(|_| UserInfoTooltip {
                name: name.clone(),
                email: email.clone(),
                anchor: (bounds.get(), TooltipDirection::North),
            })
            .into()
        })
        .tooltip_show_delay(TOOLTIP_DELAY)
}

/// Chromium's line breaking for tooltip text (`word-break: break-word` over
/// ICU's rules as they apply to paths and sentences): lines break after a
/// space or a hyphen, never after `/`, and a word that still overflows breaks
/// at the character that crosses `max`. Returns the text with `\n` at each
/// inserted break (trailing spaces dropped) and the byte offsets, in the
/// original text, where breaks were inserted.
fn break_word_lines(
    text: &str,
    max: Pixels,
    measure: &dyn Fn(&str) -> Pixels,
) -> (String, Vec<usize>) {
    let mut out = String::new();
    let mut breaks = Vec::new();
    let mut offset = 0;
    for (n, hard) in text.split('\n').enumerate() {
        if n > 0 {
            out.push('\n');
            offset += 1;
        }
        let mut start = 0;
        let mut last_break: Option<usize> = None;
        let chars: Vec<(usize, char)> = hard.char_indices().collect();
        let mut k = 0;
        while k < chars.len() {
            let (i, ch) = chars[k];
            let end = i + ch.len_utf8();
            if measure(&hard[start..end]) > max && end - start > ch.len_utf8() {
                let at = match last_break {
                    Some(b) if b > start => b,
                    _ => i,
                };
                out.push_str(hard[start..at].trim_end());
                out.push('\n');
                breaks.push(offset + at);
                start = at;
                while hard[start..].starts_with(' ') {
                    start += 1;
                }
                last_break = None;
                k = chars
                    .iter()
                    .position(|(j, _)| *j >= start)
                    .unwrap_or(chars.len());
                continue;
            }
            if ch == ' ' || ch == '-' {
                last_break = Some(end);
            }
            k += 1;
        }
        out.push_str(&hard[start..]);
        offset += hard.len();
    }
    (out, breaks)
}

/// GHD tooltips on any element: the caption tooltip after
/// [`TOOLTIP_DELAY`].
pub trait GhdTooltip: StatefulInteractiveElement + Sized {
    fn ghd_tooltip(self, text: impl Into<SharedString>) -> Self {
        let text: SharedString = text.into();
        self.tooltip(tooltip(text))
            .tooltip_show_delay(TOOLTIP_DELAY)
    }
}

impl<E: StatefulInteractiveElement> GhdTooltip for E {}

/// Accessibility for icon-only controls, after GHD's `ariaLabel` +
/// `tooltip` pairs: a `Button` node VoiceOver announces by `label`, and the
/// caption tooltip.
pub trait IconButtonA11y: StatefulInteractiveElement + Sized {
    /// `Button` role, accessible name and tooltip.
    fn icon_button_label(self, label: impl Into<SharedString>) -> Self {
        let label: SharedString = label.into();
        self.a11y_button(label.clone()).ghd_tooltip(label)
    }

    /// `Button` role and accessible name only (the tooltip is set elsewhere
    /// or differs from the name).
    fn a11y_button(self, label: impl Into<SharedString>) -> Self {
        self.role(Role::Button).aria_label(label)
    }
}

impl<E: StatefulInteractiveElement> IconButtonA11y for E {}

/// VoiceOver for list rows and banners (GHD `List` rows are `role="option"`
/// with `aria-selected`; `AriaLiveContainer` for banners).
pub trait ListRowA11y: StatefulInteractiveElement + Sized {
    /// A `Row` node (macOS `AXRow`, which carries the selection) named
    /// `label`, with its selected state.
    fn a11y_row(self, label: impl Into<SharedString>, selected: bool) -> Self {
        self.role(Role::Row)
            .aria_label(label)
            .aria_selected(selected)
    }

    /// A polite live region: VoiceOver announces `text` when the element
    /// appears or the text changes.
    fn a11y_live(self, text: impl Into<SharedString>) -> Self {
        let text: SharedString = text.into();
        let value = text.to_string();
        self.role(Role::Status)
            .aria_label(text)
            .a11y_synthetic_children(move |tree| {
                let node = tree.parent_node();
                node.set_live(accesskit::Live::Polite);
                node.set_value(value);
            })
    }
}

impl<E: StatefulInteractiveElement> ListRowA11y for E {}

/// A file status as VoiceOver reads it (GHD `mapStatus`, `lib/status.ts`):
/// a text conflict with no markers left reads "Resolved".
pub fn status_label(status: &corvene_core::FileStatus) -> &'static str {
    use corvene_core::FileStatusKind::*;
    match status.kind {
        New | Untracked => "New",
        Modified => "Modified",
        Deleted => "Deleted",
        Copied => "Copied",
        Renamed => "Renamed",
        Conflicted if status.conflict_markers == Some(0) => "Resolved",
        Conflicted => "Conflicted",
    }
}

/// A `.tooltip(...)` builder with GHD's caption look, anchored to the pointer.
pub fn tooltip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView {
    let text: SharedString = text.into();
    move |_, cx| {
        cx.new(|_| TextTooltip {
            text: text.clone(),
            bold: None,
            anchor: None,
            fixed_width: false,
            titled: false,
        })
        .into()
    }
}

/// [`tooltip`] with a bold byte range (`<strong>`); `\n` breaks lines.
pub fn rich_tooltip(
    text: impl Into<SharedString>,
    bold: std::ops::Range<usize>,
) -> impl Fn(&mut Window, &mut App) -> AnyView {
    let text: SharedString = text.into();
    move |_, cx| {
        cx.new(|_| TextTooltip {
            text: text.clone(),
            bold: Some(bold.clone()),
            anchor: None,
            fixed_width: false,
            titled: false,
        })
        .into()
    }
}

/// GHD `<Tooltip direction={…}>`: the caption tooltip placed around `el`
/// (a probe child records its bounds) after [`TOOLTIP_DELAY`].
pub fn with_directed_tooltip(
    el: Stateful<Div>,
    text: impl Into<SharedString>,
    direction: TooltipDirection,
) -> Stateful<Div> {
    with_directed_tooltip_delay(el, text, direction, TOOLTIP_DELAY)
}

/// [`with_directed_tooltip`] with its own delay (GHD shows a disabled
/// button's tooltip at once: `delay={disabled ? 0 : undefined}`).
pub fn with_directed_tooltip_delay(
    el: Stateful<Div>,
    text: impl Into<SharedString>,
    direction: TooltipDirection,
    delay: std::time::Duration,
) -> Stateful<Div> {
    directed_tooltip(el, text.into(), direction, delay, false, false)
}

/// GHD's `length-hint-tooltip` (`ui/changes/commit-message.tsx`): a title
/// line and a dimmed description below it, as wide as the title.
pub fn with_titled_tooltip(
    el: Stateful<Div>,
    title: &str,
    description: &str,
    direction: TooltipDirection,
    delay: std::time::Duration,
) -> Stateful<Div> {
    directed_tooltip(
        el,
        format!("{title}\n{description}").into(),
        direction,
        delay,
        false,
        true,
    )
}

/// [`with_directed_tooltip`] at the 300 px maximum width whatever the text,
/// for text that changes while it is shown (push / pull progress).
pub fn with_fixed_width_tooltip(
    el: Stateful<Div>,
    text: impl Into<SharedString>,
    direction: TooltipDirection,
) -> Stateful<Div> {
    directed_tooltip(el, text.into(), direction, TOOLTIP_DELAY, true, false)
}

/// [`with_directed_tooltip`] whose text follows the app state while it is
/// shown (`text` runs again whenever [`AppState`](corvene_core::AppState)
/// changes): for contents that load once the pointer is over the element
/// (`1101-push-size-tooltip`). A plain tooltip keeps the text it was built
/// with until it closes.
pub fn with_live_directed_tooltip(
    el: Stateful<Div>,
    text: impl Fn(&App) -> SharedString + 'static,
    direction: TooltipDirection,
) -> Stateful<Div> {
    let bounds = std::rc::Rc::new(std::cell::Cell::new(Bounds::default()));
    let probe = bounds.clone();
    let text = std::rc::Rc::new(text);
    el.relative()
        .child(
            canvas(move |b, _, _| probe.set(b), |_, _, _, _| {})
                .absolute()
                .size_full(),
        )
        .tooltip(move |_, cx| {
            let state = corvene_core::AppState::global(cx);
            let anchor = Some((bounds.get(), direction));
            let text = text.clone();
            cx.new(|cx| {
                let follow = text.clone();
                cx.observe(&state, move |this: &mut TextTooltip, _, cx| {
                    let text = follow(cx);
                    if this.text != text {
                        this.text = text;
                        cx.notify();
                    }
                })
                .detach();
                TextTooltip {
                    text: text(cx),
                    bold: None,
                    anchor,
                    fixed_width: false,
                    titled: false,
                }
            })
            .into()
        })
        .tooltip_show_delay(TOOLTIP_DELAY)
}

fn directed_tooltip(
    el: Stateful<Div>,
    text: SharedString,
    direction: TooltipDirection,
    delay: std::time::Duration,
    fixed_width: bool,
    titled: bool,
) -> Stateful<Div> {
    let bounds = std::rc::Rc::new(std::cell::Cell::new(Bounds::default()));
    let probe = bounds.clone();
    el.relative()
        .child(
            canvas(move |b, _, _| probe.set(b), |_, _, _, _| {})
                .absolute()
                .size_full(),
        )
        .tooltip(move |_, cx| {
            cx.new(|_| TextTooltip {
                text: text.clone(),
                bold: None,
                anchor: Some((bounds.get(), direction)),
                fixed_width,
                titled,
            })
            .into()
        })
        .tooltip_show_delay(delay)
}

/// GHD `.blankslate-image`: an `illustrations/<name>` picture, drawn in the
/// dark themes through `invert() grayscale(1) brightness(8) contrast(0.6)`,
/// which `tools/illustrations/darken.py` bakes into `illustrations/dark/`.
pub fn blankslate_image(name: &str, cx: &App) -> Img {
    if cx.ghd().appearance == crate::theme::Appearance::Dark {
        img(format!("illustrations/dark/{name}"))
    } else {
        img(format!("illustrations/{name}"))
    }
}

/// GHD `InputError` (`.input-description.input-description-error`): a 16 px
/// stop icon, 5 px, the message in 11 px `--input-error-text-color`.
pub fn input_error(message: impl Into<SharedString>, cx: &App) -> Div {
    let t = cx.ghd();
    div()
        .flex()
        .flex_row()
        .items_start()
        .text_size(FONT_SIZE_SM())
        .line_height(zpx(16.5))
        .text_color(t.input_error_text)
        .child(
            crate::icons::octicon(crate::icons::Octicon::Stop, t.input_error_text)
                .flex_none()
                .mr(SPACING_HALF()),
        )
        .child(div().flex_1().min_w_0().child(message.into()))
}

/// Flag `104-selection-keeps-colour-on-hover`: a hovered selected list row
/// keeps its selection colour (off: GHD's `.list-item:hover` wins).
pub fn selection_keeps_colour_on_hover(cx: &App) -> bool {
    corvene_core::AppState::try_global(cx).is_some_and(|s| {
        s.read(cx)
            .flags
            .bool(corvene_core::flags::ids::SELECTION_KEEPS_COLOUR_ON_HOVER)
    })
}

/// Flag `110-keyboard-hides-hover` off: hover styles and tooltips survive
/// typing for the elements the pointer hovered before, like Chromium's
/// `:hover` (vendored gpui-pre: in keyboard modality GPUI otherwise treats
/// every hitbox as unhovered). Corvene's widgets track `:focus-visible`
/// themselves, so nothing else reads the keyboard modality.
pub fn sync_hover_while_typing(cx: &App) {
    let hides = corvene_core::AppState::try_global(cx).is_none_or(|s| {
        s.read(cx)
            .flags
            .bool(corvene_core::flags::ids::KEYBOARD_HIDES_HOVER)
    });
    set_hover_persists_while_typing(!hides);
}

/// An inline `<Ref>` that wraps anywhere (`word-break: break-all`), drawn as
/// Chromium slices it: one chip per line fragment on the path-segment
/// background, 3.33 px padding and rounded corners only at the start of the
/// first fragment and the end of the last, each fragment's box (content area
/// plus padding) overhanging the line without growing it.
pub fn wrapped_ref(
    text: &str,
    font_size: Pixels,
    line_height: Pixels,
    max_width: Pixels,
    window: &Window,
    cx: &App,
) -> Div {
    let t = cx.ghd();
    let pad = SPACING_THIRD();
    let mut style = window.text_style();
    style.font_family = crate::theme::mono_font().into();
    style.font_size = font_size.into();
    let measure = |s: &str| -> Pixels {
        window
            .text_system()
            .shape_line(
                SharedString::from(s.to_string()),
                font_size,
                &[style.to_run(s.len())],
                None,
            )
            .width
    };
    // greedy break-all, the first fragment carrying the left padding
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        let lead = if lines.is_empty() { pad } else { px(0.) };
        let mut next = current.clone();
        next.push(ch);
        if !current.is_empty() && measure(&next) + lead > max_width {
            lines.push(std::mem::take(&mut current));
            current.push(ch);
        } else {
            current = next;
        }
    }
    lines.push(current);
    let last = lines.len() - 1;
    // the fragment box: the font's content area (~1.2 em) plus padding
    let box_h = font_size * 1.2 + pad * 2.;
    let overhang = (box_h - line_height) / 2.;
    div()
        .flex()
        .flex_col()
        .items_start()
        .font_family(crate::theme::mono_font())
        .text_size(font_size)
        .line_height(line_height)
        .children(lines.into_iter().enumerate().map(move |(i, line)| {
            div().h(line_height).flex().items_center().child(
                div()
                    .h(box_h)
                    .my(-overhang)
                    .flex()
                    .items_center()
                    .bg(t.path_segment_background)
                    .when(i == 0, |d| d.pl(pad).rounded_l(BORDER_RADIUS()))
                    .when(i == last, |d| d.pr(pad).rounded_r(BORDER_RADIUS()))
                    .child(line),
            )
        }))
}

/// Which dialog an [`android_storage_note`] is for.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StorageNote {
    /// Clone and Create: where the repository goes.
    NewRepository,
    /// Add Local Repository: what Choose… does.
    AddLocal,
}

/// Android only (`None` elsewhere): what the Local Path of a dialog means
/// there. Repositories live in Corvene's own storage, which uninstalling
/// the application deletes; a folder picked from elsewhere is imported, or
/// used in place once "All files access" is granted (builds that may ask
/// for it offer the link).
pub fn android_storage_note(note: StorageNote, cx: &App) -> Option<AnyElement> {
    #[cfg(not(target_os = "android"))]
    {
        let _ = (note, cx);
        None
    }
    #[cfg(target_os = "android")]
    {
        let t = cx.ghd();
        let bridge = corvene_platform::android::bridge();
        let in_place = bridge.is_some_and(|bridge| bridge.has_all_files_access());
        let can_ask =
            !in_place && bridge.is_some_and(|bridge| bridge.can_request_all_files_access());
        let text = match (note, in_place) {
            (StorageNote::NewRepository, false) => {
                "Corvene keeps repositories in its own storage. Uninstalling Corvene deletes \
                 them, so push what you want to keep."
            }
            (StorageNote::NewRepository, true) => {
                "Corvene keeps repositories in its own storage, which uninstalling Corvene \
                 deletes. A path under /storage/emulated/0 puts the repository on shared \
                 storage instead, where Termux and other apps reach it (without symbolic \
                 links and file modes)."
            }
            (StorageNote::AddLocal, false) => {
                "Choose… imports a repository: the folder is copied into Corvene's own storage, \
                 which uninstalling Corvene deletes."
            }
            (StorageNote::AddLocal, true) => {
                "A repository on shared storage is used in place (without symbolic links and \
                 file modes). One picked from another app is copied into Corvene's own storage."
            }
        };
        let mut parts: Vec<Inline> = vec![text.into()];
        if can_ask {
            parts.push(" ".into());
            parts.push(
                link_button(
                    "android-all-files-access",
                    match note {
                        StorageNote::AddLocal => {
                            "Allow \"All files access\" to use folders in place."
                        }
                        StorageNote::NewRepository => {
                            "Allow \"All files access\" to use shared storage, which Termux \
                             reaches too."
                        }
                    },
                    cx,
                )
                .on_click(|_, _, cx| corvene_core::Dispatcher::request_all_files_access(cx))
                .into_any_element()
                .into(),
            );
        }
        Some(
            paragraph(parts)
                .text_color(t.text_secondary)
                .into_any_element(),
        )
    }
}

/// Android: marks the element it fills (a dialog's backdrop, a menu, a
/// foldout) as covering the drag handles painted before it, so a tap on a
/// link over a hidden handle is not taken for a drag
/// (`corvene_platform::android::add_drag_occluder`). Nothing elsewhere.
pub fn touch_drag_occluder() -> impl IntoElement {
    // in the paint phase: what counts is the order things are painted in
    canvas(
        |_, _, _| {},
        |bounds, _, _, _| {
            #[cfg(target_os = "android")]
            corvene_platform::android::add_drag_occluder(
                bounds.origin.x.into(),
                bounds.origin.y.into(),
                (bounds.origin.x + bounds.size.width).into(),
                (bounds.origin.y + bounds.size.height).into(),
            );
            #[cfg(not(target_os = "android"))]
            let _ = bounds;
        },
    )
    .absolute()
    .size_full()
}

/// Android: marks the element it fills as a handle a finger can drag (see
/// `corvene_platform::android::add_drag_handle`); put it inside a resizable
/// group's handle. Nothing elsewhere: a mouse drags the handle as it is.
pub fn touch_drag_handle() -> impl IntoElement {
    // in the paint phase: what counts is the order things are painted in
    canvas(
        |_, _, _| {},
        |bounds, _, _, _| {
            #[cfg(target_os = "android")]
            corvene_platform::android::add_drag_handle(
                bounds.origin.x.into(),
                bounds.origin.y.into(),
                (bounds.origin.x + bounds.size.width).into(),
                (bounds.origin.y + bounds.size.height).into(),
            );
            #[cfg(not(target_os = "android"))]
            let _ = bounds;
        },
    )
    .absolute()
    .size_full()
}

/// Corvene (`116-resizable-dialog-text-areas`): heights of the dialogs'
/// multi-line boxes the user dragged (kept while Corvene runs), keyed by box,
/// and the drag in progress (box, pointer and height where it started).
struct TextAreaResize {
    heights: std::collections::HashMap<&'static str, Pixels>,
    laid_out: std::collections::HashMap<&'static str, Pixels>,
    drag: Option<(&'static str, Pixels, Pixels)>,
}

thread_local! {
    static TEXT_AREA_RESIZE: std::cell::RefCell<TextAreaResize> =
        std::cell::RefCell::new(TextAreaResize {
            heights: std::collections::HashMap::new(),
            laid_out: std::collections::HashMap::new(),
            drag: None,
        });
}

/// Corvene (`116-resizable-dialog-text-areas`): a dialog's multi-line text
/// box with a grip along its bottom edge that drags it taller or shorter
/// (at least 40 px, at most 70% of the window; the dialog grows with it).
/// GHD's text areas have a fixed number of rows (`rows` in
/// `app/src/ui/lib/text-area.tsx`); without the flag the box is unchanged.
pub fn resizable_text_area(
    key: &'static str,
    area: gpui_kit::component::input::Textarea,
    cx: &App,
) -> AnyElement {
    let on = corvene_core::AppState::try_global(cx).is_some_and(|s| {
        s.read(cx)
            .flags
            .bool(corvene_core::flags::ids::RESIZABLE_DIALOG_TEXT_AREAS)
    });
    if !on {
        return area.into_any_element();
    }
    let t = cx.ghd();
    let height = TEXT_AREA_RESIZE.with(|r| r.borrow().heights.get(key).copied());
    let area = match height {
        Some(height) => area.h(height),
        None => area,
    };
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .relative()
                .child(area)
                // the height the box has now: where a drag starts from
                .child(
                    canvas(
                        move |bounds, _, _| {
                            TEXT_AREA_RESIZE
                                .with(|r| r.borrow_mut().laid_out.insert(key, bounds.size.height));
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
                ),
        )
        .child(
            div()
                .id(SharedString::from(format!("{key}-resize-grip")))
                .h(zpx(8.))
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor(CursorStyle::ResizeUpDown)
                .child(
                    div()
                        .w(zpx(24.))
                        .h(zpx(3.))
                        .rounded(zpx(2.))
                        .bg(t.text_secondary.opacity(0.5)),
                )
                .on_mouse_down(MouseButton::Left, move |ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    TEXT_AREA_RESIZE.with(|r| {
                        let mut r = r.borrow_mut();
                        let from = r.laid_out.get(key).copied().unwrap_or(zpx(80.));
                        r.drag = Some((key, ev.position.y, from));
                    });
                })
                // the pointer is followed anywhere in the window until the
                // button is released (as the commit form's handle)
                .child(
                    canvas(
                        |_, _, _| {},
                        move |_, _, window, _| {
                            window.on_mouse_event(move |ev: &MouseMoveEvent, _, window, _| {
                                let max = (window.viewport_size().height * 0.7).max(zpx(40.));
                                let moved = TEXT_AREA_RESIZE.with(|r| {
                                    let mut r = r.borrow_mut();
                                    match r.drag {
                                        Some((dragged, from, height)) if dragged == key => {
                                            let height = (height + ev.position.y - from)
                                                .clamp(zpx(40.), max);
                                            r.heights.insert(key, height);
                                            true
                                        }
                                        _ => false,
                                    }
                                });
                                if moved {
                                    window.refresh();
                                }
                            });
                            window.on_mouse_event(move |_: &MouseUpEvent, _, _, _| {
                                TEXT_AREA_RESIZE.with(|r| {
                                    let mut r = r.borrow_mut();
                                    if r.drag.is_some_and(|(dragged, _, _)| dragged == key) {
                                        r.drag = None;
                                    }
                                });
                            });
                        },
                    )
                    .absolute()
                    .size_0(),
                ),
        )
        .into_any_element()
}

/// Corvene (`526-commit-signing`): "Sign commits", "Signing key" and the
/// SSH key choice (`commit.gpgsign`, `user.signingkey`, `gpg.format`) of
/// one Git config scope. `ids`: the two checkboxes' and the key box's.
#[allow(clippy::too_many_arguments)]
pub fn signing_fields(
    ids: (&'static str, &'static str, &'static str),
    sign: bool,
    ssh: bool,
    key: &Entity<InputState>,
    on_sign: impl Fn(bool, &mut Window, &mut App) + 'static,
    on_ssh: impl Fn(bool, &mut Window, &mut App) + 'static,
    window: &Window,
    cx: &App,
) -> Div {
    let t = cx.ghd();
    div()
        .flex()
        .flex_col()
        .gap(SPACING_HALF())
        .child(checkbox_row(ids.0, sign, "Sign commits", on_sign, cx))
        .child(labeled(
            "Signing key",
            text_box(ids.2, key, None, window, cx),
            cx,
        ))
        .child(checkbox_row(
            ids.1,
            ssh,
            "The key is an SSH key (gpg.format = ssh)",
            on_ssh,
            cx,
        ))
        .child(
            div()
                .text_size(FONT_SIZE_SM())
                .text_color(t.text_secondary)
                .child(
                    "A GPG key ID, or an SSH public key file or key. Git signs each commit \
                     with it while Sign commits is on.",
                ),
        )
}
