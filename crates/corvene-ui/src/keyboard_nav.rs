//! Tab / Shift+Tab focus traversal, as Chromium gives GHD: text fields and
//! lists are tab stops, and so are buttons, links, checkboxes, radio
//! buttons, switches and select buttons ([`ControlFocus`]), which take
//! focus and show a ring while it came from the keyboard (`:focus-visible`;
//! Space or Enter presses them). An open dialog keeps Tab inside it, as
//! `<dialog>` makes the rest of the page inert.
//!
//! Deviation (`622-system-keyboard-navigation`): on macOS the controls are
//! tab stops only with System Settings › Keyboard › "Keyboard navigation"
//! on, as in AppKit apps; text fields and lists always are. Electron ignores
//! the setting (desktop/desktop#4623).
//!
//! Deviation (`623-contrast-focus-ring`): in the high contrast theme the
//! ring and the keyboard row of a dropdown's list are drawn in the text
//! colour, which GHD leaves to colours a contrast theme replaces
//! (desktop/desktop#22949).

use std::cell::RefCell;
use std::collections::HashMap;

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;

/// Whether buttons and the like are tab stops (always without
/// `622-system-keyboard-navigation`).
struct ControlsReachable(bool);

impl Global for ControlsReachable {}

/// Set by the binary from the flag and the macOS setting; windows redraw.
pub fn set_controls_reachable(reachable: bool, cx: &mut App) {
    if cx
        .try_global::<ControlsReachable>()
        .is_some_and(|g| g.0 == reachable)
    {
        return;
    }
    cx.set_global(ControlsReachable(reachable));
    cx.refresh_windows();
}

/// Tab reaches buttons, links, checkboxes and selects.
pub fn controls_reachable(cx: &App) -> bool {
    cx.try_global::<ControlsReachable>().is_none_or(|g| g.0)
}

/// `623-contrast-focus-ring`: the ring colour, the text colour in the high
/// contrast theme.
pub fn contrast_focus(cx: &App) -> Option<Hsla> {
    let t = cx.ghd();
    let on = t.name == crate::theme::HIGH_CONTRAST_NAME
        && corvene_core::AppState::try_global(cx).is_some_and(|s| {
            s.read(cx)
                .flags
                .bool(corvene_core::flags::ids::CONTRAST_FOCUS_RING)
        });
    on.then_some(t.text)
}

/// Chromium's `outline: auto` 2 px outside the border (`outline-offset`),
/// as two box shadows: the page background in the gap, the ring around it.
pub fn focus_ring_shadows(cx: &App) -> Vec<BoxShadow> {
    let t = cx.ghd();
    let ring = contrast_focus(cx).unwrap_or(crate::widgets::focus_ring_colors(cx).0);
    let shadow = |color: Hsla, spread: f32| BoxShadow {
        color,
        offset: point(zpx(0.), zpx(0.)),
        blur_radius: zpx(0.),
        spread_radius: zpx(spread),
        inset: false,
    };
    // the first shadow paints on top
    vec![shadow(t.background, 2.), shadow(ring, 4.)]
}

/// The ring along the inside of an element that fills its bar (toolbar
/// buttons, tabs; Chromium's `outline-offset: -4px` leaves a 2 px gap,
/// which two inset shadows do not draw evenly in GPUI).
pub fn inset_ring_shadows(cx: &App) -> Vec<BoxShadow> {
    let ring = contrast_focus(cx).unwrap_or(crate::widgets::focus_ring_colors(cx).0);
    let shadow = |color: Hsla, spread: f32| BoxShadow {
        color,
        offset: point(zpx(0.), zpx(0.)),
        blur_radius: zpx(0.),
        spread_radius: zpx(spread),
        inset: true,
    };
    vec![shadow(ring, 2.)]
}

/// `623-contrast-focus-ring`: the keyboard row of a dropdown's list (the
/// one Enter picks) outlined 2 px inside in the text colour; `None` outside
/// the high contrast theme.
pub fn contrast_row_outline(cx: &App) -> Option<Vec<BoxShadow>> {
    contrast_focus(cx).map(|color| {
        vec![BoxShadow {
            color,
            offset: point(zpx(0.), zpx(0.)),
            blur_radius: zpx(0.),
            spread_radius: zpx(2.),
            inset: true,
        }]
    })
}

/// A control that Tab can reach (when [`controls_reachable`]): focusable, a
/// tab stop, with the focus ring while focused from the keyboard. Its
/// `on_click` runs for Space and Enter (GPUI's keyboard click).
pub trait ControlFocus: Sized {
    fn control_focus(self, cx: &App) -> Self;
    /// [`Self::control_focus`] with `focused` (the focus background and an
    /// [`inset_ring_shadows`] ring, say) instead of the outside ring.
    fn control_focus_styled(
        self,
        cx: &App,
        focused: impl FnOnce(StyleRefinement) -> StyleRefinement,
    ) -> Self;
}

impl ControlFocus for Stateful<Div> {
    fn control_focus(self, cx: &App) -> Self {
        if !controls_reachable(cx) {
            return self;
        }
        let ring = focus_ring_shadows(cx);
        self.control_focus_styled(cx, move |s| s.shadow(ring))
    }

    fn control_focus_styled(
        self,
        cx: &App,
        focused: impl FnOnce(StyleRefinement) -> StyleRefinement,
    ) -> Self {
        if !controls_reachable(cx) {
            return self;
        }
        self.focusable().tab_stop(true).focus_visible(focused)
    }
}

thread_local! {
    /// The topmost dialog drawn in each window this frame: its container's
    /// focus handle, which keeps Tab inside it.
    static MODAL: RefCell<HashMap<WindowId, FocusHandle>> = RefCell::default();
    /// One handle per dialog id, kept across frames.
    static DIALOG_HANDLES: RefCell<HashMap<(WindowId, SharedString), FocusHandle>> =
        RefCell::default();
    /// The dialogs [`claim_dialog_focus`] has run for since no popup was open.
    static CLAIMED: RefCell<std::collections::HashSet<(WindowId, SharedString)>> =
        RefCell::default();
}

/// The focus handle of dialog `id`'s container (the trap), registered as
/// the window's topmost dialog for this frame.
pub fn dialog_trap(id: &str, window: &Window, cx: &App) -> FocusHandle {
    let key = (
        window.window_handle().window_id(),
        SharedString::from(id.to_string()),
    );
    let handle = DIALOG_HANDLES.with(|h| {
        h.borrow_mut()
            .entry(key.clone())
            .or_insert_with(|| cx.focus_handle())
            .clone()
    });
    MODAL.with(|m| m.borrow_mut().insert(key.0, handle.clone()));
    handle
}

/// Once per opening, after the dialog's first frame: focus `trap` unless a
/// field of the dialog took focus, so Enter and Escape reach the dialog
/// rather than what was focused underneath. Only then can it be known: a
/// field focused as the dialog is built is not in the frame being drawn.
pub fn claim_dialog_focus(id: &str, trap: &FocusHandle, window: &mut Window, cx: &mut App) {
    let key = (
        window.window_handle().window_id(),
        SharedString::from(id.to_string()),
    );
    if !CLAIMED.with(|c| c.borrow_mut().insert(key)) {
        return;
    }
    let trap = trap.clone();
    window.defer(cx, move |window, cx| {
        if !trap.contains_focused(window, cx) {
            window.focus(&trap, cx);
        }
    });
}

/// Forget the window's dialog once no popup is open.
pub fn clear_dialog_trap(window: &Window) {
    let id = window.window_handle().window_id();
    MODAL.with(|m| m.borrow_mut().remove(&id));
    CLAIMED.with(|c| c.borrow_mut().retain(|(window, _)| *window != id));
}

/// Move focus to the next (or previous) tab stop; with a dialog open, the
/// next one inside it, wrapping around.
pub fn move_focus(forward: bool, modal_open: bool, window: &mut Window, cx: &mut App) {
    let step = |window: &mut Window, cx: &mut App| {
        if forward {
            window.focus_next(cx)
        } else {
            window.focus_prev(cx)
        }
    };
    let trap = modal_open
        .then(|| MODAL.with(|m| m.borrow().get(&window.window_handle().window_id()).cloned()))
        .flatten();
    let Some(trap) = trap else {
        step(window, cx);
        return;
    };
    if !trap.contains_focused(window, cx) {
        // from the container: forward is its first stop, back the last
        window.focus(&trap, cx);
    }
    let start = window.focused(cx);
    for _ in 0..512 {
        step(window, cx);
        let focused = window.focused(cx);
        if focused.as_ref() != Some(&trap) && trap.contains_focused(window, cx) {
            return;
        }
        if focused == start {
            return;
        }
    }
    // nothing in the dialog takes focus: keep it there anyway
    window.focus(&trap, cx);
}
