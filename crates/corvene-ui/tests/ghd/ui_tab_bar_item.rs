//! Port of GitHub Desktop's `app/test/unit/ui/tab-bar-item-test.tsx`
//! (the keyboard cases).
//!
//! GitHub Desktop's `TabBarItem.onKeyDown` asks its `TabBar` for the
//! previous tab on ←, the next on → (↑ / ↓ in a vertical bar) and ignores
//! other keys. Corvene's tabs (`corvene_ui::tab_bar`) resolve the key with
//! `adjacent_tab(index, count, key, vertical)`, which also does the bar's
//! wrap-around (`TabBar.onSelectAdjacentTab`); the rendered bar has three
//! tabs and the item is the third (`index: 2`).

use corvene_ui::tab_bar::adjacent_tab;

// GHD: unit/ui/tab-bar-item-test.tsx › TabBarItem › selects adjacent tabs with horizontal arrow keys
#[test]
fn selects_adjacent_tabs_with_horizontal_arrow_keys() {
    let selections: Vec<usize> = ["left", "right", "enter"]
        .into_iter()
        .filter_map(|key| adjacent_tab(2, 3, key, false))
        .collect();
    // previous of 2 is 1, next of 2 wraps to 0; Enter selects nothing
    assert_eq!(selections, vec![1, 0]);
}

// GHD: unit/ui/tab-bar-item-test.tsx › TabBarItem › selects adjacent tabs with vertical arrow keys when rendered vertically
#[test]
fn selects_adjacent_tabs_with_vertical_arrow_keys_when_rendered_vertically() {
    let selections: Vec<usize> = ["up", "down", "left"]
        .into_iter()
        .filter_map(|key| adjacent_tab(2, 3, key, true))
        .collect();
    assert_eq!(selections, vec![1, 0]);
}
