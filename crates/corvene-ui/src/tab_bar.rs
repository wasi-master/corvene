//! `.tab-bar.tabs` - the Changes | History switcher (29 px).
//! Tabs and bar share `--background-color`; hover uses `--tab-bar-hover-background-color`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::widgets::IconButtonA11y;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::icons::{Octicon, octicon};
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::counter_text;

thread_local! {
    /// One focus handle per tab id, so the arrow keys can focus the tab
    /// beside the focused one (GHD `TabBar.tabRefsByIndex`).
    static TAB_HANDLES: RefCell<HashMap<&'static str, FocusHandle>> = RefCell::default();
}

fn tab_handle(id: &'static str, cx: &App) -> FocusHandle {
    TAB_HANDLES.with(|h| {
        h.borrow_mut()
            .entry(id)
            .or_insert_with(|| cx.focus_handle())
            .clone()
    })
}

/// GHD `TabBarItem.onKeyDown` + `TabBar.onSelectAdjacentTab`: the tab that
/// `key` moves to from tab `ix` of `count`, ← / → in a horizontal bar and
/// ↑ / ↓ in a vertical one, wrapping around the ends.
pub fn adjacent_tab(ix: usize, count: usize, key: &str, vertical: bool) -> Option<usize> {
    let (previous, next) = if vertical {
        ("up", "down")
    } else {
        ("left", "right")
    };
    if count == 0 {
        return None;
    }
    if key == previous {
        Some((ix + count - 1) % count)
    } else if key == next {
        Some((ix + 1) % count)
    } else {
        None
    }
}

/// A tab's keyboard focus (`keyboard_nav`): only the selected tab is a tab
/// stop (GHD `tabIndex={selected ? undefined : -1}`), and the arrow keys
/// focus and select the adjacent tab ([`adjacent_tab`]).
#[allow(clippy::too_many_arguments)]
fn tab_focus(
    d: Stateful<Div>,
    ids: Rc<[&'static str]>,
    ix: usize,
    selected: bool,
    vertical: bool,
    on_select: impl Fn(usize, &mut Window, &mut App) + 'static,
    focused: impl FnOnce(StyleRefinement) -> StyleRefinement,
    cx: &App,
) -> Stateful<Div> {
    if !crate::keyboard_nav::controls_reachable(cx) {
        return d;
    }
    d.track_focus(&tab_handle(ids[ix], cx).tab_stop(selected))
        .focus_visible(focused)
        .on_key_down(move |ev: &KeyDownEvent, window, cx| {
            if ev.keystroke.modifiers.modified() {
                return;
            }
            if let Some(next) = adjacent_tab(ix, ids.len(), &ev.keystroke.key, vertical) {
                cx.stop_propagation();
                window.focus(&tab_handle(ids[next], cx), cx);
                on_select(next, window, cx);
            }
        })
}

/// `.tab-bar.vertical` item: 16 px icon + label.
pub struct VerticalTab {
    pub id: &'static str,
    pub label: SharedString,
    pub icon: Octicon,
}

/// Settings / Repository Settings side navigation (`.tab-bar.vertical`):
/// column with 10 px vertical padding; items ≥150 px wide, 10 px padding,
/// 3.33 px × 20 px margins, 6 px radius; selected = blue with white text,
/// otherwise the icon is `$gray-500` and hover tints the row.
pub fn vertical_tab_bar(
    tabs: Vec<VerticalTab>,
    selected: usize,
    on_select: impl Fn(usize, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> impl IntoElement {
    vertical_tab_bar_sized(tabs, selected, false, on_select, cx)
}

/// [`vertical_tab_bar`]; `icons_only` for the compact (phone) layout, where
/// the labels would leave the tab's content no room.
pub fn vertical_tab_bar_sized(
    tabs: Vec<VerticalTab>,
    selected: usize,
    icons_only: bool,
    on_select: impl Fn(usize, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> impl IntoElement {
    let t = cx.ghd();
    let icon_muted = crate::theme::c(crate::theme::primer::GRAY_500);
    let ids: Rc<[&'static str]> = tabs.iter().map(|tab| tab.id).collect();
    div()
        .id("vertical-tab-bar")
        .flex_none()
        .flex()
        .flex_col()
        .py(SPACING())
        .children(tabs.into_iter().enumerate().map(|(ix, tab)| {
            let is_selected = ix == selected;
            let on_select = on_select.clone();
            let hover_bg = t.tab_bar_hover_background;
            let (bg, text, icon) = if is_selected {
                (
                    t.tab_bar_active,
                    t.box_selected_active_text,
                    t.box_selected_active_text,
                )
            } else {
                (t.background, t.text, icon_muted)
            };
            let d = div()
                .id(tab.id)
                .when(!icons_only, |d| d.min_w(zpx(150.)))
                .when(icons_only, |d| d.icon_button_label(tab.label.clone()))
                .my(SPACING_THIRD())
                .mx(if icons_only {
                    SPACING_HALF()
                } else {
                    SPACING_DOUBLE()
                })
                .p(SPACING())
                .rounded(BORDER_RADIUS())
                .flex()
                .flex_row()
                .items_center()
                .bg(bg)
                .text_color(text)
                .text_size(FONT_SIZE())
                // a `<button>` at `line-height: normal`: the label span is
                // 16 px tall with SF (36 px items), 17 px with Noto Sans
                .line_height(if cfg!(target_os = "macos") {
                    zpx(16.)
                } else {
                    crate::theme::normal_line_height(FONT_SIZE(), cx)
                })
                .cursor_pointer()
                .when(!is_selected, move |d| d.hover(move |s| s.bg(hover_bg)))
                .on_click({
                    let on_select = on_select.clone();
                    move |_, window, cx| on_select(ix, window, cx)
                })
                .child(octicon(tab.icon, icon).when(!icons_only, |d| d.mr(SPACING())))
                .when(!icons_only, |d| d.child(tab.label));
            let ring = crate::keyboard_nav::focus_ring_shadows(cx);
            tab_focus(
                d,
                ids.clone(),
                ix,
                is_selected,
                true,
                on_select,
                move |s| s.shadow(ring),
                cx,
            )
        }))
}

pub struct TabModel {
    pub id: &'static str,
    pub label: SharedString,
    /// The `.counter` text after the label.
    pub count: Option<SharedString>,
    /// Corvene (`727-stash-dot-on-changes-tab`): a blue dot after the label.
    pub dot: bool,
}

pub fn tab_bar(
    tabs: Vec<TabModel>,
    selected: usize,
    on_select: impl Fn(usize, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> impl IntoElement {
    tab_bar_focus(tabs, selected, false, on_select, cx)
}

/// [`tab_bar`] whose selected tab may show its `:focus-visible` ring (a
/// dialog's first focusable element): Chromium's 2 px outline drawn 4 px
/// inside the tab (`.tab-bar.tabs .tab-bar-item:focus-visible`).
pub fn tab_bar_focus(
    tabs: Vec<TabModel>,
    selected: usize,
    focus_ring: bool,
    on_select: impl Fn(usize, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> impl IntoElement {
    let t = cx.ghd();
    let count = tabs.len();
    let ids: Rc<[&'static str]> = tabs.iter().map(|tab| tab.id).collect();
    div()
        .id("tab-bar")
        .w_full()
        .h(TAB_BAR_HEIGHT())
        .flex_none()
        .flex()
        .flex_row()
        .bg(t.background)
        .children(tabs.into_iter().enumerate().map(|(ix, tab)| {
            let is_selected = ix == selected;
            let is_last = ix + 1 == count;
            let on_select = on_select.clone();
            let hover_bg = t.tab_bar_hover_background;
            div()
                .id(tab.id)
                .relative()
                .flex_1()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(t.background)
                .border_b_1()
                .border_color(t.box_border)
                .when(!is_last, |d| d.border_r_1())
                .text_color(t.text)
                .text_size(FONT_SIZE())
                .cursor_pointer()
                .hover(move |s| s.bg(hover_bg))
                .map(|d| {
                    let ring = crate::keyboard_nav::inset_ring_shadows(cx);
                    let on_select = on_select.clone();
                    tab_focus(
                        d,
                        ids.clone(),
                        ix,
                        is_selected,
                        false,
                        on_select,
                        move |s| s.shadow(ring),
                        cx,
                    )
                })
                .on_click(move |_, window, cx| on_select(ix, window, cx))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .child(tab.label)
                        .when_some(tab.count, |d, n| d.child(counter_text(n, cx)))
                        .when(tab.dot, |d| {
                            d.child(
                                octicon(Octicon::DotFill, t.tab_bar_active)
                                    .size(zpx(12.))
                                    .ml(SPACING_HALF()),
                            )
                        }),
                )
                .when(is_selected && focus_ring, |d| {
                    d.child(
                        div()
                            .absolute()
                            .top(zpx(2.))
                            .bottom(zpx(2.))
                            .left(zpx(2.))
                            .right(zpx(2.))
                            .border_2()
                            .border_color(t.focus)
                            .rounded(zpx(3.)),
                    )
                })
                .when(is_selected, |d| {
                    // `box-shadow: inset 0 -3px 0 var(--tab-bar-active-color)`
                    d.child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .h(zpx(3.))
                            .bg(t.tab_bar_active),
                    )
                })
        }))
}

#[cfg(test)]
mod tests {
    use super::adjacent_tab;

    #[test]
    fn arrows_select_the_adjacent_tab_and_wrap() {
        assert_eq!(adjacent_tab(0, 3, "right", false), Some(1));
        assert_eq!(adjacent_tab(2, 3, "right", false), Some(0));
        assert_eq!(adjacent_tab(0, 3, "left", false), Some(2));
        // a vertical bar takes ↑ / ↓ and leaves ← / → alone
        assert_eq!(adjacent_tab(0, 3, "down", true), Some(1));
        assert_eq!(adjacent_tab(0, 3, "up", true), Some(2));
        assert_eq!(adjacent_tab(0, 3, "right", true), None);
        assert_eq!(adjacent_tab(0, 3, "down", false), None);
        assert_eq!(adjacent_tab(0, 0, "right", false), None);
    }
}
