//! Port of GitHub Desktop's `app/test/unit/ui/tab-bar-test.tsx` (the
//! keyboard case).
//!
//! GitHub Desktop's `TabBar` focuses the adjacent tab and calls
//! `onTabClicked` with its index when a tab gets ← / →. Corvene's tab bars
//! (`corvene_ui::tab_bar::tab_bar`) do the same from a tab's key handler
//! with `adjacent_tab`; moving the window's focus needs a drawn window, so
//! only the emitted index is checked here.

use corvene_ui::tab_bar::adjacent_tab;

// GHD: unit/ui/tab-bar-test.tsx › TabBar › moves focus and emits selection when navigating with the keyboard
#[test]
fn moves_focus_and_emits_selection_when_navigating_with_the_keyboard() {
    let clicks: Vec<usize> = adjacent_tab(0, 3, "right", false).into_iter().collect();
    assert_eq!(clicks, vec![1]);
}
