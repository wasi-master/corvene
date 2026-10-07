//! Modal dialog chrome (`styles/ui/_dialog.scss`): overlay, 400–600 px box,
//! 50 px header with close button, 20 px padded content, footer buttons.
//!
//! Accessibility: the box is a `Dialog` node labelled with its title, and the
//! open dialog's title becomes the window title (VoiceOver reads it as the
//! `AXWindow` title; the title bar itself is hidden) until the dialog closes
//! (`DialogHost` restores "Corvene"). Linux keeps the app name in its
//! visible title bar, as Electron does; AT-SPI reads the `Dialog` node.

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::widgets::IconButtonA11y;

use crate::icons::{Octicon, loading as loading_icon, octicon};
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, GhdTheme};

pub type ClickHandler = Box<dyn Fn(&mut Window, &mut App) + 'static>;

/// The window title when no dialog is open.
pub const APP_WINDOW_TITLE: &str = "Corvene";

/// Set the window title unless it already is `title` (macOS only); one
/// cache per window (`crate::windows`).
pub fn sync_window_title(title: &SharedString, window: &mut Window) {
    crate::windows::sync_window_title(title, window);
}

/// A zero-size element that makes `title` the window title while it renders
/// (dialog frames drawn outside `dialog()` add it themselves).
pub fn window_title(title: impl Into<SharedString>) -> impl IntoElement {
    let title: SharedString = title.into();
    canvas(
        move |_, window, _| sync_window_title(&title, window),
        |_, _, _, _| {},
    )
    .absolute()
    .size_0()
}

/// GHD `Dialog type`: warning/error dialogs show a 24 px icon left of the content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogKind {
    Normal,
    Warning,
    Error,
}

/// Per-dialog chrome variations.
pub struct DialogFrame {
    /// `.dialog-header`'s bottom border (`#choose-branch` has none).
    pub header_border: bool,
    /// `.dialog-content`'s 20 px padding (off for list dialogs whose content
    /// runs edge to edge).
    pub content_padding: bool,
    /// A footer drawn by the dialog itself instead of the button row.
    pub footer: Option<AnyElement>,
    /// GHD renders no `.dialog-header` for an untitled `Dialog` (About).
    pub show_header: bool,
    /// The primary button holds focus as the dialog opens (GHD focuses the
    /// submit button when nothing else asks for focus): its focus ring and
    /// `:focus` background show.
    pub focus_primary: bool,
    /// `focusCloseButtonOnOpen`: the header's close button holds focus.
    pub focus_close: bool,
}

impl Default for DialogFrame {
    fn default() -> Self {
        Self {
            header_border: true,
            content_padding: true,
            footer: None,
            show_header: true,
            focus_primary: false,
            focus_close: false,
        }
    }
}

/// GHD `OkCancelButtonGroup.renderButtons`: Cancel then OK on macOS, OK
/// then Cancel elsewhere. Dialogs list their footer buttons in the macOS
/// order; this puts them in the platform's.
pub fn ok_cancel_order<T>(mut buttons: Vec<T>) -> Vec<T> {
    if !cfg!(target_os = "macos") {
        buttons.reverse();
    }
    buttons
}

/// A destructive confirmation's button: GHD's bare `verb` ("Delete"), or
/// with `276-descriptive-confirm-buttons` the verb and its object ("Delete
/// Branch", sentence case off macOS).
pub fn confirm_label(
    verb: &'static str,
    mac: &'static str,
    other: &'static str,
    cx: &App,
) -> SharedString {
    let descriptive = corvene_core::AppState::global(cx)
        .read(cx)
        .flags
        .bool(corvene_core::flags::ids::DESCRIPTIVE_CONFIRM_BUTTONS);
    if descriptive {
        crate::context_menu::mac_or(mac, other).into()
    } else {
        verb.into()
    }
}

pub struct DialogButton {
    pub id: &'static str,
    pub label: SharedString,
    pub primary: bool,
    /// GHD `okButtonDisabled`: 60 % opacity, clicks ignored.
    pub disabled: bool,
    pub on_click: ClickHandler,
}

/// A group button's `type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonType {
    /// `type="submit"`: the default button, taken when the form is submitted
    /// with the keyboard (`DialogButton::primary`).
    Submit,
    /// `type="reset"`
    Reset,
    /// `type="button"`
    Button,
}

/// What a click on a group button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupEvent {
    /// `onOkButtonClick`
    OkButtonClick,
    /// `onCancelButtonClick`
    CancelButtonClick,
    /// The dialog's form gets `submit` (its affirmative action).
    Submit,
    /// The dialog's form gets `reset` (it is dismissed).
    Reset,
}

/// Which of the group's buttons a [`GroupButton`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupButtonRole {
    Ok,
    Cancel,
}

/// One button of an `OkCancelButtonGroup`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupButton {
    pub label: String,
    pub role: GroupButtonRole,
    pub button_type: ButtonType,
    /// `aria-describedby`
    pub aria_described_by: Option<String>,
}

/// GHD `IOkCancelButtonGroupProps` (the click callbacks are the
/// [`GroupEvent`]s).
pub struct OkCancelButtonGroupProps<'a> {
    pub destructive: bool,
    pub ok_button_text: &'a str,
    pub cancel_button_text: &'a str,
    pub ok_button_aria_described_by: Option<&'a str>,
    /// The text of each child.
    pub children: Vec<&'a str>,
}

/// What GHD `OkCancelButtonGroup` renders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OkCancelButtonGroupContent {
    /// The `destructive` class.
    pub destructive: bool,
    /// In the order they are drawn.
    pub buttons: Vec<GroupButton>,
    /// The children's text, after the buttons.
    pub children: Vec<String>,
}

impl OkCancelButtonGroupContent {
    /// A click on the button labelled `label`: what it sends. OK calls
    /// `onOkButtonClick` and submits the form, Cancel calls
    /// `onCancelButtonClick` and resets it; a destructive group flips its
    /// buttons' types (`onOkButtonClick` / `onCancelButtonClick` dispatch the
    /// flipped event), so the outcome is the same. Nothing for no such
    /// button.
    pub fn click(&self, label: &str) -> Vec<GroupEvent> {
        self.buttons
            .iter()
            .filter(|b| b.label == label)
            .flat_map(|b| match b.role {
                GroupButtonRole::Ok => [GroupEvent::OkButtonClick, GroupEvent::Submit],
                GroupButtonRole::Cancel => [GroupEvent::CancelButtonClick, GroupEvent::Reset],
            })
            .collect()
    }
}

/// GHD `OkCancelButtonGroup` (`ui/dialog/ok-cancel-button-group.tsx`): OK is
/// the `submit` button and Cancel `reset`, unless the group is destructive,
/// when Cancel is the `submit` (default) button and OK a plain button, so
/// that submitting with the keyboard takes the safe choice; Cancel then OK
/// on macOS, OK then Cancel elsewhere (`renderButtons`).
pub fn ok_cancel_button_group(props: OkCancelButtonGroupProps<'_>) -> OkCancelButtonGroupContent {
    let ok = GroupButton {
        label: props.ok_button_text.to_string(),
        role: GroupButtonRole::Ok,
        button_type: if props.destructive {
            ButtonType::Button
        } else {
            ButtonType::Submit
        },
        aria_described_by: props.ok_button_aria_described_by.map(str::to_string),
    };
    let cancel = GroupButton {
        label: props.cancel_button_text.to_string(),
        role: GroupButtonRole::Cancel,
        button_type: if props.destructive {
            ButtonType::Submit
        } else {
            ButtonType::Reset
        },
        aria_described_by: None,
    };
    OkCancelButtonGroupContent {
        destructive: props.destructive,
        buttons: ok_cancel_order(vec![cancel, ok]),
        children: props.children.into_iter().map(str::to_string).collect(),
    }
}

/// One footer button of an [`OkCancelButtonGroup`].
pub struct GroupButtonSpec {
    pub id: &'static str,
    pub label: SharedString,
    pub disabled: bool,
    pub on_click: ClickHandler,
}

/// GHD `OkCancelButtonGroup` as a dialog's footer buttons: the default
/// (`submit`) button of [`ok_cancel_button_group`] is the primary one (Cancel
/// for a destructive group).
pub struct OkCancelButtonGroup {
    pub destructive: bool,
    pub cancel: GroupButtonSpec,
    pub ok: GroupButtonSpec,
}

impl OkCancelButtonGroup {
    /// The buttons, Cancel then OK (the dialog puts them in the platform's
    /// order).
    pub fn into_buttons(self) -> Vec<DialogButton> {
        let OkCancelButtonGroup {
            destructive,
            cancel,
            ok,
        } = self;
        let content = ok_cancel_button_group(OkCancelButtonGroupProps {
            destructive,
            ok_button_text: &ok.label,
            cancel_button_text: &cancel.label,
            ok_button_aria_described_by: None,
            children: Vec::new(),
        });
        let primary = |role: GroupButtonRole| {
            content
                .buttons
                .iter()
                .any(|b| b.role == role && b.button_type == ButtonType::Submit)
        };
        let (cancel_primary, ok_primary) = (
            primary(GroupButtonRole::Cancel),
            primary(GroupButtonRole::Ok),
        );
        [(cancel, cancel_primary), (ok, ok_primary)]
            .into_iter()
            .map(|(b, primary)| DialogButton {
                id: b.id,
                label: b.label,
                primary,
                disabled: b.disabled,
                on_click: b.on_click,
            })
            .collect()
    }
}

/// GHD's per-dialog `width` rules (`dialog#<id> { width }` in the
/// stylesheets), by Corvene dialog id; other dialogs size to their content
/// between 400 and 600 px.
fn ghd_dialog_width(id: &str) -> Option<f32> {
    Some(match id {
        // `#app-error.raw-git-error`: room for 80-column git output
        "dialog-git-error" => 750.,
        "dialog-conflicts" | "create-fork" | "clone-repository" => 500.,
        // (`dialog#push-needs-pull-warning` is 450 px too, but
        // `PushNeedsPullWarning` never sets that id, so it sizes to its text)
        "dialog-confirm-abort"
        | "dialog-stash-and-switch"
        | "create-tutorial-repository-dialog"
        | "dialog-merge-branch"
        | "dialog-rebase-branch"
        | "dialog-cherry-pick"
        | "dialog-about"
        | "push-branch-commits"
        | "dialog-generic-git-auth"
        // Corvene (`528-proxy-credentials`), built like the one above
        | "dialog-proxy-auth"
        | "dialog-change-repository-alias"
        | "dialog-confirm-remove-repository" => 450.,
        "create-repository"
        | "dialog-create-branch"
        | "dialog-rename-branch"
        | "dialog-create-tag"
        | "sign-in"
        | "add-existing-repository"
        | "dialog-initialize-lfs" => 400.,
        "dialog-preferences" => 600.,
        // Corvene: Language Extensions (list + details, three tabs)
        "language-extensions" => 760.,
        // Corvene: an Actions job log (`347-actions-job-logs`), 100 columns
        "actions-job-log" => 800.,
        _ => return None,
    })
}

/// GHD's `max-width` rules above the 600 px default, for dialogs that
/// size to their content.
fn ghd_dialog_max_width(id: &str) -> Option<f32> {
    Some(match id {
        // "Make sure 80 cols fit comfortably" (`_commit_progress.scss`,
        // `_hook-failed.scss`)
        "commit-progress-dialog" | "hook-failed-dialog" => 800.,
        _ => return None,
    })
}

/// `421-larger-dialogs`: the dialogs whose lists grow with the window.
const LARGER_DIALOGS: &[&str] = &["dialog-conflicts"];

/// `421-larger-dialogs` is on.
pub fn larger_dialogs(cx: &App) -> bool {
    corvene_core::AppState::try_global(cx).is_some_and(|s| {
        s.read(cx)
            .flags
            .bool(corvene_core::flags::ids::LARGER_DIALOGS)
    })
}

/// `421-larger-dialogs`: 80 % of the window, at most 960 px.
pub fn larger_dialog_width(viewport: Size<Pixels>) -> Pixels {
    (viewport.width * 0.8).min(zpx(960.))
}

pub fn dialog(
    id: &'static str,
    title: impl Into<SharedString>,
    content: impl IntoElement,
    buttons: Vec<DialogButton>,
    on_close: impl Fn(&mut Window, &mut App) + Clone + 'static,
    window: &Window,
    cx: &App,
) -> impl IntoElement {
    dialog_with_kind(
        id,
        DialogKind::Normal,
        title,
        content,
        buttons,
        on_close,
        window,
        cx,
    )
}

/// `.dialog.warning` / `.dialog.error`: content gets `margin-left: 20px` and
/// `padding-left: 20px + 24px icon`, so the text starts 64 px from the edge.
fn dialog_content(
    kind: DialogKind,
    content: impl IntoElement,
    padding: bool,
    t: &GhdTheme,
) -> Stateful<Div> {
    // The padding sits on a wrapper inside the scroll container: taffy adds
    // a scroll container's own end padding to its scroll extent even when
    // the content's negative margins (the tabbed dialogs bleed to the edges
    // with `-20px`) already cover it, so every such dialog scrolled ~40 px
    // past its end.
    let base = div()
        .id("dialog-content")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .text_size(FONT_SIZE())
        .line_height(zpx(18.));
    let inner = div().when(padding, |d| d.p(SPACING_DOUBLE()));
    match kind {
        DialogKind::Normal if !padding => base.child(content),
        DialogKind::Normal => base.child(inner.child(content)),
        DialogKind::Warning | DialogKind::Error => {
            let color = if kind == DialogKind::Warning {
                t.dialog_warning
            } else {
                t.dialog_error
            };
            base.child(
                inner
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(SPACING_DOUBLE())
                    .child(
                        octicon(Octicon::Alert, color)
                            .size(zpx(24.))
                            .flex_none()
                            .mt(zpx(5.)),
                    )
                    .child(div().flex_1().min_w_0().child(content)),
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn dialog_with_kind(
    id: &'static str,
    kind: DialogKind,
    title: impl Into<SharedString>,
    content: impl IntoElement,
    buttons: Vec<DialogButton>,
    on_close: impl Fn(&mut Window, &mut App) + Clone + 'static,
    window: &Window,
    cx: &App,
) -> impl IntoElement {
    dialog_with_kind_opts(
        id, kind, true, title, content, buttons, on_close, window, cx,
    )
}

/// `dialog_with_kind` with GHD's `backdropDismissable` prop: with it off a
/// click outside the dialog does nothing.
#[allow(clippy::too_many_arguments)]
pub fn dialog_with_kind_opts(
    id: &'static str,
    kind: DialogKind,
    backdrop_dismissable: bool,
    title: impl Into<SharedString>,
    content: impl IntoElement,
    buttons: Vec<DialogButton>,
    on_close: impl Fn(&mut Window, &mut App) + Clone + 'static,
    window: &Window,
    cx: &App,
) -> impl IntoElement {
    let title: SharedString = title.into();
    dialog_impl(
        id,
        kind,
        false,
        backdrop_dismissable,
        div().child(title.clone()).into_any_element(),
        Some(title),
        content,
        None,
        buttons,
        DialogFrame::default(),
        on_close,
        window,
        cx,
    )
}

/// A dialog whose footer shows `footer_message` above the buttons (GHD
/// dialogs that put a `<div>` in `DialogFooter`, e.g. `#create-repo-path-msg`).
#[allow(clippy::too_many_arguments)]
pub fn dialog_with_footer_message(
    id: &'static str,
    title: impl Into<SharedString>,
    content: impl IntoElement,
    footer_message: Option<AnyElement>,
    buttons: Vec<DialogButton>,
    on_close: impl Fn(&mut Window, &mut App) + Clone + 'static,
    window: &Window,
    cx: &App,
) -> impl IntoElement {
    let title: SharedString = title.into();
    dialog_impl(
        id,
        DialogKind::Normal,
        false,
        true,
        div().child(title.clone()).into_any_element(),
        Some(title),
        content,
        footer_message,
        buttons,
        DialogFrame::default(),
        on_close,
        window,
        cx,
    )
}

/// [`dialog`] with chrome variations (`DialogFrame`).
#[allow(clippy::too_many_arguments)]
pub fn dialog_with_frame(
    id: &'static str,
    title: impl Into<SharedString>,
    content: impl IntoElement,
    buttons: Vec<DialogButton>,
    frame: DialogFrame,
    on_close: impl Fn(&mut Window, &mut App) + Clone + 'static,
    window: &Window,
    cx: &App,
) -> impl IntoElement {
    dialog_loading_framed(
        id, title, false, content, buttons, frame, on_close, window, cx,
    )
}

/// [`dialog_loading`] with chrome variations (`DialogFrame`).
#[allow(clippy::too_many_arguments)]
pub fn dialog_loading_framed(
    id: &'static str,
    title: impl Into<SharedString>,
    loading: bool,
    content: impl IntoElement,
    buttons: Vec<DialogButton>,
    frame: DialogFrame,
    on_close: impl Fn(&mut Window, &mut App) + Clone + 'static,
    window: &Window,
    cx: &App,
) -> impl IntoElement {
    let title: SharedString = title.into();
    dialog_impl(
        id,
        DialogKind::Normal,
        loading,
        true,
        div().child(title.clone()).into_any_element(),
        Some(title),
        content,
        None,
        buttons,
        frame,
        on_close,
        window,
        cx,
    )
}

/// A dialog with an element title and its own footer (`DialogFrame`), e.g.
/// `#choose-branch`.
#[allow(clippy::too_many_arguments)]
pub fn dialog_framed(
    id: &'static str,
    title: impl IntoElement,
    plain_title: impl Into<SharedString>,
    content: impl IntoElement,
    frame: DialogFrame,
    on_close: impl Fn(&mut Window, &mut App) + Clone + 'static,
    window: &Window,
    cx: &App,
) -> impl IntoElement {
    dialog_impl(
        id,
        DialogKind::Normal,
        false,
        true,
        title.into_any_element(),
        Some(plain_title.into()),
        content,
        None,
        Vec::new(),
        frame,
        on_close,
        window,
        cx,
    )
}

/// [`dialog_with_kind`] with chrome variations (`DialogFrame`).
#[allow(clippy::too_many_arguments)]
pub fn dialog_with_kind_framed(
    id: &'static str,
    kind: DialogKind,
    title: impl Into<SharedString>,
    content: impl IntoElement,
    buttons: Vec<DialogButton>,
    frame: DialogFrame,
    on_close: impl Fn(&mut Window, &mut App) + Clone + 'static,
    window: &Window,
    cx: &App,
) -> impl IntoElement {
    let title: SharedString = title.into();
    dialog_impl(
        id,
        kind,
        false,
        true,
        div().child(title.clone()).into_any_element(),
        Some(title),
        content,
        None,
        buttons,
        frame,
        on_close,
        window,
        cx,
    )
}

/// `Dialog loading={…}`: a spinner in the header while the dialog works.
#[allow(clippy::too_many_arguments)]
pub fn dialog_loading(
    id: &'static str,
    title: impl Into<SharedString>,
    loading: bool,
    content: impl IntoElement,
    buttons: Vec<DialogButton>,
    on_close: impl Fn(&mut Window, &mut App) + Clone + 'static,
    window: &Window,
    cx: &App,
) -> impl IntoElement {
    let title: SharedString = title.into();
    dialog_impl(
        id,
        DialogKind::Normal,
        loading,
        true,
        div().child(title.clone()).into_any_element(),
        Some(title),
        content,
        None,
        buttons,
        DialogFrame::default(),
        on_close,
        window,
        cx,
    )
}

/// A dialog whose title is an element (bold branch names inside the title);
/// `plain_title` is the same text for the window title and VoiceOver.
#[allow(clippy::too_many_arguments)]
pub fn dialog_with_title_element(
    id: &'static str,
    title: impl IntoElement,
    plain_title: impl Into<SharedString>,
    content: impl IntoElement,
    buttons: Vec<DialogButton>,
    on_close: impl Fn(&mut Window, &mut App) + Clone + 'static,
    window: &Window,
    cx: &App,
) -> impl IntoElement {
    dialog_impl(
        id,
        DialogKind::Normal,
        false,
        true,
        title.into_any_element(),
        Some(plain_title.into()),
        content,
        None,
        buttons,
        DialogFrame::default(),
        on_close,
        window,
        cx,
    )
}

#[allow(clippy::too_many_arguments)]
fn dialog_impl(
    id: &'static str,
    kind: DialogKind,
    loading: bool,
    backdrop_dismissable: bool,
    title: AnyElement,
    plain_title: Option<SharedString>,
    content: impl IntoElement,
    footer_message: Option<AnyElement>,
    buttons: Vec<DialogButton>,
    frame: DialogFrame,
    on_close: impl Fn(&mut Window, &mut App) + Clone + 'static,
    window: &Window,
    cx: &App,
) -> impl IntoElement {
    let t = cx.ghd();
    let close_for_overlay = on_close.clone();
    let DialogFrame {
        header_border,
        content_padding,
        footer,
        show_header,
        focus_primary,
        focus_close,
    } = frame;
    let viewport = crate::theme::page_size(window);
    let large_width = if id == "dialog-actions" {
        // Corvene `351-actions`: three panes, most of the window
        Some(crate::dialogs::actions_view::dialog_width(viewport))
    } else {
        (LARGER_DIALOGS.contains(&id) && larger_dialogs(cx))
            .then(|| larger_dialog_width(viewport).max(zpx(400.)))
    };
    // a phone: no dialog is wider than the window
    let widest = if crate::theme::compact(window) {
        viewport.width - zpx(16.)
    } else {
        Pixels::MAX
    };
    // Tab stays inside the dialog (`keyboard_nav`); once it moved focus to
    // one of the dialog's fields or buttons, the primary button's opening
    // ring goes
    let trap = crate::keyboard_nav::dialog_trap(id, window, cx);
    let focus_primary =
        focus_primary && !(trap.contains_focused(window, cx) && !trap.is_focused(window));
    deferred(
        anchored().position(crate::theme::page_origin()).child(
            div()
                .id(id)
                .track_focus(&trap)
                // modal: the views underneath get no hover, clicks or wheel
                // (GHD's `<dialog>` makes the rest of the page inert)
                .occlude()
                .child(crate::widgets::touch_drag_occluder())
                .w(viewport.width)
                .h(viewport.height)
                .flex()
                .items_center()
                .justify_center()
                // Windows: GHD's page includes its title bar, so a dialog is
                // centred in the whole window: half the bar's height higher
                // than in the page below it
                .when(cfg!(windows), |d| d.pb(crate::theme::page_top()))
                .bg(t.dialog_backdrop)
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    if backdrop_dismissable {
                        close_for_overlay(window, cx)
                    }
                })
                .child(
                    div()
                        .id("dialog-box")
                        .role(Role::Dialog)
                        .when_some(plain_title.clone(), |d, title| d.aria_label(title))
                        .children(plain_title.map(window_title))
                        .min_w(zpx(400.).min(widest))
                        // a dialog wider than GHD's 600 px cap says so in `ghd_dialog_width`
                        .max_w(
                            zpx(ghd_dialog_width(id)
                                .or(ghd_dialog_max_width(id))
                                .map_or(600., |w| w.max(600.)))
                            .max(large_width.unwrap_or_default())
                            .min(widest),
                        )
                        .when_some(large_width.or(ghd_dialog_width(id).map(zpx)), |d, w| {
                            d.w(w.min(widest))
                        })
                        // a `<dialog>` never outgrows the viewport; the content scrolls
                        .max_h(viewport.height)
                        .flex()
                        .flex_col()
                        .rounded(BORDER_RADIUS())
                        .bg(t.background)
                        .text_color(t.text)
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
                        .when(show_header, |d| {
                            d.child(
                                // header
                                div()
                                    .h(zpx(50.))
                                    .flex_none()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .px(SPACING_DOUBLE())
                                    .when(header_border, |d| {
                                        d.border_b_1().border_color(t.box_border)
                                    })
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_size(FONT_SIZE_MD())
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(title),
                                    )
                                    // `loading`: a spinning `syncClockwise` before the close button
                                    .when(loading, |d| {
                                        d.child(div().flex_none().mr(SPACING()).child(
                                            loading_icon("dialog-loading", t.text_secondary),
                                        ))
                                    })
                                    .child({
                                        let on_close = on_close.clone();
                                        div()
                                            .id("dialog-close")
                                            .relative()
                                            .icon_button_label("Close")
                                            .size(zpx(16.))
                                            // `close-button` mixin: `margin-right:
                                            // -10px` pulls it into the header's padding
                                            .mr(-SPACING())
                                            .cursor_pointer()
                                            .on_click(move |_, window, cx| on_close(window, cx))
                                            // Chromium's focus ring: 2 px out,
                                            // 2 px wide, a 1 px dark halo
                                            .when(focus_close, |d| {
                                                d.child(
                                                    div()
                                                        .absolute()
                                                        .top(zpx(-5.))
                                                        .left(zpx(-5.))
                                                        .right(zpx(-5.))
                                                        .bottom(zpx(-5.))
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
                                            .child(octicon(Octicon::X, t.text_secondary))
                                    }),
                            )
                        })
                        .child(dialog_content(kind, content, content_padding, t))
                        .children(footer)
                        .when(!buttons.is_empty(), |d| {
                            d.child(
                                // `.dialog-footer`: a top border, 20 px padding,
                                // an optional message 10 px above the buttons
                                // (`margin: -10px 0 10px`), buttons 5 px apart
                                div()
                                    .flex_none()
                                    .flex()
                                    .flex_col()
                                    .p(SPACING_DOUBLE())
                                    .border_t_1()
                                    .border_color(t.box_border)
                                    .text_size(FONT_SIZE())
                                    .line_height(zpx(18.))
                                    .children(footer_message.map(|message| {
                                        div()
                                            // a block takes the dialog's width
                                            // without widening it
                                            .w(zpx(0.))
                                            .min_w_full()
                                            .mt(-SPACING())
                                            .mb(SPACING())
                                            .child(message)
                                    }))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .justify_end()
                                            .gap(SPACING_HALF())
                                            .children(ok_cancel_order(buttons).into_iter().map(
                                                |b| {
                                                    let on_click = b.on_click;
                                                    let disabled = b.disabled;
                                                    if b.primary {
                                                        let button =
                                                            crate::widgets::primary_button(
                                                                b.id, b.label, disabled, cx,
                                                            )
                                                            .min_w(zpx(120.))
                                                            .when(focus_primary, |d| {
                                                                d.bg(t.button_hover_background)
                                                            })
                                                            .when(!disabled, |d| {
                                                                d.on_click(move |_, window, cx| {
                                                                    on_click(window, cx)
                                                                })
                                                            });
                                                        div()
                                                            .relative()
                                                            .when(focus_primary, |d| {
                                                                d.child(crate::widgets::focus_ring(
                                                                    cx,
                                                                ))
                                                            })
                                                            .child(button)
                                                            .into_any_element()
                                                    } else {
                                                        if disabled {
                                                            crate::widgets::button_disabled(
                                                                b.id, b.label, cx,
                                                            )
                                                        } else {
                                                            crate::widgets::button(
                                                                b.id, b.label, cx,
                                                            )
                                                        }
                                                        .min_w(zpx(120.))
                                                        .when(!disabled, |d| {
                                                            d.on_click(move |_, window, cx| {
                                                                on_click(window, cx)
                                                            })
                                                        })
                                                        .into_any_element()
                                                    }
                                                },
                                            )),
                                    ),
                            )
                        }),
                ),
        ),
    )
    .with_priority(20)
}
