//! Keyboard range selection for multi-select lists - GHD
//! `ui/lib/list/list.tsx` (`addSelection`, `handleKeyDown` with `shiftKey`)
//! and `ui/lib/list/selection.ts` (`createSelectionBetween`).
//!
//! GHD keeps the selected rows ordered from the selection origin
//! (`selectedRows[0]`) to the row that moves (`selectedRows.at(-1)`). Corvene
//! stores paths in the same order; the origin is the repository state's
//! `selected_file` (the ⇧-click anchor), the moving end is the last path.
//!
//! Deviation: [`extend_keeping`] keeps ⌘-clicked rows on ⇧-click
//! (`707-shift-click-keeps-selection`).
//!
//! Deviation: ↑ in a list with no selected row selects the last selectable
//! row ([`find_next_selectable_row`] with `row: None`); GHD's
//! `List.moveSelection` passes `row: -1`, from which `findNextSelectableRow`
//! starts at the last row and steps once more, so it selects the
//! second-to-last one. (GHD's filter box ↑, `FilterList.onKeyDown`, passes
//! `row: 0` instead and lands on the last row, as Corvene does.)

/// `createSelectionBetween`: the rows from `from` to `to` inclusive, in the
/// direction of travel (so `from` comes first).
pub fn selection_between(order: &[String], from: usize, to: usize) -> Vec<String> {
    if order.is_empty() {
        return Vec::new();
    }
    let last = order.len() - 1;
    let (from, to) = (from.min(last), to.min(last));
    if from <= to {
        order[from..=to].to_vec()
    } else {
        order[to..=from].iter().rev().cloned().collect()
    }
}

/// GHD `findNextSelectableRow(rowCount, { direction, row, wrap },
/// canSelectRow)` (`ui/lib/list/selection.ts`): the next row in the
/// direction of `delta` (its sign: up when negative) from `row` that
/// `can_select_row` accepts, wrapping around the ends when `wrap` is set.
/// `row: None` (or a row outside the list) is GHD's `-1`, no selected row or
/// the filter box above the list: ↓ starts at the first row, ↑ at the last
/// (GHD: the second-to-last, see the module doc). `None` for an empty list,
/// when no other row can be selected, or (without `wrap`) at the edge.
///
/// A `delta` of more than one row is a page (PageUp / PageDown) or Home /
/// End: [`find_next_page_selectable_row`], which never wraps.
pub fn find_next_selectable_row(
    row_count: usize,
    row: Option<usize>,
    delta: isize,
    wrap: bool,
    can_select_row: impl Fn(usize) -> bool,
) -> Option<usize> {
    if row_count == 0 {
        return None;
    }
    if delta.unsigned_abs() > 1 {
        let from = row.filter(|&r| r < row_count);
        let next =
            find_next_page_selectable_row(row_count, from.unwrap_or(0), delta, can_select_row);
        return (Some(next) != from).then_some(next);
    }
    let len = row_count as isize;
    let up = delta < 0;
    let step = if up { -1 } else { 1 };
    let given = row.filter(|&r| r < row_count).map(|r| r as isize);
    // a row outside the list starts past the end the move comes from (GHD's
    // special case for ↓ from row -1; Corvene's for ↑, see the module doc),
    // so the first step lands on the first / last row
    let mut current = given.unwrap_or(if up { len } else { -1 });
    for _ in 0..row_count {
        current += step;
        if current >= len {
            if !wrap {
                break;
            }
            current = 0;
        } else if current < 0 {
            if !wrap {
                break;
            }
            current = len - 1;
        }
        if Some(current) != given && can_select_row(current as usize) {
            return Some(current as usize);
        }
    }
    None
}

/// GHD `List.findNextPageSelectableRow`: the farthest row `can_select_row`
/// accepts within `delta` rows of `from` (up when negative), clamped to the
/// ends; `from` when there is none. The caller turns the list's height into
/// `delta` (`moveSelectionByPage` / `addSelectionByPage` go from the last
/// selected row, GHD's `selectedRows.at(-1) ?? 0`).
pub fn find_next_page_selectable_row(
    row_count: usize,
    from: usize,
    delta: isize,
    can_select_row: impl Fn(usize) -> bool,
) -> usize {
    if row_count == 0 {
        return from;
    }
    let target = (from as isize + delta).clamp(0, row_count as isize - 1) as usize;
    if target >= from {
        (from..=target).rev().find(|&r| can_select_row(r))
    } else {
        (target..=from).find(|&r| can_select_row(r))
    }
    .unwrap_or(from)
}

/// `moveSelection` (`findNextSelectableRow`, which wraps by default) on a
/// list whose rows are all selectable: the row after `current` in the
/// direction of `delta`, wrapping around the ends; with nothing selected, ↓
/// starts at the first row and ↑ at the last. `None` for an empty list or
/// when `current` is the only row.
pub fn step_index(len: usize, current: Option<usize>, delta: isize) -> Option<usize> {
    find_next_selectable_row(len, current, delta, true, |_| true)
}

/// `addSelection`: move the end of the selection one row up (`delta < 0`) or
/// down and select everything between the origin and the new end. `None` when
/// nothing changes (the end is already at the edge, or `anchor` is not
/// visible). Without a wrap: GHD passes `wrap: false`.
pub fn extend_selection(
    order: &[String],
    anchor: &str,
    selected: &[String],
    delta: isize,
) -> Option<Vec<String>> {
    let origin = order.iter().position(|p| p == anchor)?;
    let end = selected
        .last()
        .and_then(|last| order.iter().position(|p| p == last))
        .unwrap_or(origin);
    let next = find_next_selectable_row(order.len(), Some(end), delta, false, |_| true)?;
    Some(selection_between(order, origin, next))
}

/// ⇧-click that keeps ⌘-clicked rows outside the range (Corvene
/// `707-shift-click-keeps-selection`, like Finder): the previous range from
/// `anchor` to the moving end (the last selected path) is replaced by the one
/// from `anchor` to `to`; other selected rows stay, before the range so the
/// moving end is still last. `None` when `anchor` or `to` is not visible.
pub fn extend_keeping(
    order: &[String],
    anchor: &str,
    selected: &[String],
    to: &str,
) -> Option<Vec<String>> {
    let origin = order.iter().position(|p| p == anchor)?;
    let target = order.iter().position(|p| p == to)?;
    let end = selected
        .last()
        .and_then(|last| order.iter().position(|p| p == last))
        .unwrap_or(origin);
    let previous = selection_between(order, origin, end);
    let range = selection_between(order, origin, target);
    let dropped: std::collections::HashSet<&str> =
        previous.iter().chain(&range).map(String::as_str).collect();
    let mut out: Vec<String> = selected
        .iter()
        .filter(|p| !dropped.contains(p.as_str()))
        .cloned()
        .collect();
    out.extend(range);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_index_clamps_and_starts_at_the_top() {
        assert_eq!(step_index(0, None, 1), None);
        assert_eq!(step_index(3, None, 1), Some(0));
        assert_eq!(step_index(3, None, -1), Some(2));
        assert_eq!(step_index(3, Some(0), 1), Some(1));
        // GHD wraps around the ends
        assert_eq!(step_index(3, Some(2), 1), Some(0));
        assert_eq!(step_index(3, Some(0), -1), Some(2));
        // the only row stays selected (GHD returns null)
        assert_eq!(step_index(1, Some(0), 1), None);
        assert_eq!(step_index(1, None, -1), Some(0));
    }

    #[test]
    fn next_selectable_row_skips_rows_and_stops_without_wrap() {
        let not_first = |row: usize| row != 0;
        assert_eq!(
            find_next_selectable_row(5, None, 1, true, not_first),
            Some(1)
        );
        assert_eq!(
            find_next_selectable_row(5, Some(1), -1, true, not_first),
            Some(4)
        );
        assert_eq!(
            find_next_selectable_row(5, Some(4), 1, false, |_| true),
            None
        );
        assert_eq!(
            find_next_selectable_row(5, Some(0), -1, false, |_| true),
            None
        );
        assert_eq!(
            find_next_selectable_row(5, Some(2), 1, true, |_| false),
            None
        );
        assert_eq!(find_next_selectable_row(0, None, 1, true, |_| true), None);
    }

    #[test]
    fn a_page_clamps_and_lands_on_the_farthest_selectable_row() {
        assert_eq!(
            find_next_selectable_row(10, Some(2), 4, true, |_| true),
            Some(6)
        );
        // never wraps, Home / End included
        assert_eq!(
            find_next_selectable_row(10, Some(8), 4, true, |_| true),
            Some(9)
        );
        assert_eq!(
            find_next_selectable_row(10, Some(9), 4, true, |_| true),
            None
        );
        assert_eq!(
            find_next_selectable_row(10, Some(5), -(1 << 30), true, |_| true),
            Some(0)
        );
        // GHD `selectedRows.at(-1) ?? 0`
        assert_eq!(
            find_next_selectable_row(10, None, 3, true, |_| true),
            Some(3)
        );
        let even = |row: usize| row.is_multiple_of(2);
        assert_eq!(find_next_page_selectable_row(10, 2, 3, even), 4);
        assert_eq!(find_next_page_selectable_row(10, 4, -3, even), 2);
        assert_eq!(find_next_page_selectable_row(10, 4, 1, |r| r == 4), 4);
    }

    #[test]
    fn extend_by_a_page() {
        let o = order();
        assert_eq!(
            extend_selection(&o, "a", &v(&["a"]), 3),
            Some(v(&["a", "b", "c", "d"]))
        );
        assert_eq!(
            extend_selection(&o, "c", &v(&["c"]), -9),
            Some(v(&["c", "b", "a"]))
        );
        assert_eq!(extend_selection(&o, "e", &v(&["e"]), 3), None);
    }

    fn order() -> Vec<String> {
        ["a", "b", "c", "d", "e"].map(String::from).to_vec()
    }

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn between_keeps_direction() {
        assert_eq!(selection_between(&order(), 1, 3), v(&["b", "c", "d"]));
        assert_eq!(selection_between(&order(), 3, 1), v(&["d", "c", "b"]));
        assert_eq!(selection_between(&order(), 2, 2), v(&["c"]));
    }

    #[test]
    fn extend_down_then_back_up() {
        let o = order();
        let s = extend_selection(&o, "b", &v(&["b"]), 1).unwrap();
        assert_eq!(s, v(&["b", "c"]));
        let s = extend_selection(&o, "b", &s, 1).unwrap();
        assert_eq!(s, v(&["b", "c", "d"]));
        let s = extend_selection(&o, "b", &s, -1).unwrap();
        assert_eq!(s, v(&["b", "c"]));
        let s = extend_selection(&o, "b", &s, -1).unwrap();
        assert_eq!(s, v(&["b"]));
        // past the origin the range grows the other way
        let s = extend_selection(&o, "b", &s, -1).unwrap();
        assert_eq!(s, v(&["b", "a"]));
    }

    #[test]
    fn extend_stops_at_edges() {
        let o = order();
        assert_eq!(extend_selection(&o, "a", &v(&["a"]), -1), None);
        assert_eq!(extend_selection(&o, "e", &v(&["e"]), 1), None);
    }

    #[test]
    fn extend_from_toggled_selection_uses_last_path_as_end() {
        // ⌘-click selections are unordered; the last toggled path is the end
        let o = order();
        let s = extend_selection(&o, "d", &v(&["a", "d"]), 1).unwrap();
        assert_eq!(s, v(&["d", "e"]));
    }

    #[test]
    fn shift_click_keeps_toggled_rows() {
        let o = order();
        // click a, ⌘-click c, ⇧-click e: a stays, c..e selected
        let s = extend_keeping(&o, "c", &v(&["a", "c"]), "e").unwrap();
        assert_eq!(s, v(&["a", "c", "d", "e"]));
        // ⇧-click d instead: the previous range c..e shrinks to c..d
        let s = extend_keeping(&o, "c", &s, "d").unwrap();
        assert_eq!(s, v(&["a", "c", "d"]));
        // ⇧-click above the anchor: the range flips, a is part of it
        let s = extend_keeping(&o, "c", &s, "a").unwrap();
        assert_eq!(s, v(&["c", "b", "a"]));
        assert_eq!(extend_keeping(&o, "z", &s, "a"), None);
    }

    #[test]
    fn hidden_anchor_is_ignored() {
        assert_eq!(extend_selection(&order(), "z", &v(&["z"]), 1), None);
    }
}
