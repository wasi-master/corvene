//! Keyboard selection shared by the foldouts' filter lists (GHD
//! `ui/lib/filter-list.tsx` `onFilterKeyDown`): ↓ / ↑ in the filter box move
//! a highlighted row through the list, Enter picks it.
//!
//! Deviation: GHD moves the focus into the list on ↓ / ↑; Corvene keeps the
//! caret in the filter box and highlights the row, so typing keeps
//! filtering.

use gpui_kit::{Pixels, ScrollHandle, UniformListScrollHandle};

/// GHD `ui/lib/filter-list.tsx` `onFilterKeyDown` (ArrowDown / ArrowUp): the
/// next highlighted row of `count` rows. ↓ from the filter starts at the
/// first row, ↑ at the last; further moves wrap around like the list's own
/// (`List.moveSelection`). [`step_selectable`] with every row selectable.
pub fn step(current: Option<usize>, delta: isize, count: usize) -> Option<usize> {
    step_selectable(current, delta, count, |_| true)
}

/// [`step`] over the rows `can_select` accepts (GHD `canSelectRow`), with
/// `corvene_core::list_selection::find_next_selectable_row` (GHD
/// `findNextSelectableRow`): from the filter box ↓ starts at the first row
/// and ↑ at the last; a highlighted row stays when no other row can be
/// selected. Only the sign of `delta` counts.
pub fn step_selectable(
    current: Option<usize>,
    delta: isize,
    count: usize,
    can_select: impl Fn(usize) -> bool,
) -> Option<usize> {
    use corvene_core::list_selection::find_next_selectable_row;
    match current {
        Some(ix) if ix < count => {
            find_next_selectable_row(count, Some(ix), delta, true, can_select).or(Some(ix))
        }
        // from the filter box: the first row down, the last one up
        _ => find_next_selectable_row(count, None, delta, true, can_select),
    }
}

/// GHD `List.moveSelection` (`findNextSelectableRow`, `wrap` defaults to
/// true): ↓ on the last row goes to the first, ↑ on the first to the last.
/// Extending a range (`addSelection`, `wrap: false`) clamps instead, and so
/// does a move of more than one row: a page (`moveSelectionByPage`) or
/// Home / End.
pub fn wrap_step(ix: usize, delta: isize, count: usize) -> usize {
    if count == 0 {
        return 0;
    }
    if delta.unsigned_abs() > 1 {
        return (ix as isize + delta).clamp(0, count as isize - 1) as usize;
    }
    (ix as isize + delta).rem_euclid(count as isize) as usize
}

/// PageUp / PageDown in a uniform list (GHD `List.findNextPageSelectableRow`):
/// the rows one page moves from the selected row, the rows that fit in the
/// list's height less the one it starts on. `0` before the first layout
/// (GHD: no height yet, no move). A list too short for two rows moves one
/// row, as ↓ / ↑ do.
pub fn page_rows(scroll: &UniformListScrollHandle, count: usize) -> isize {
    let Some(size) = scroll.0.borrow().last_item_size else {
        return 0;
    };
    if count == 0 {
        return 0;
    }
    rows_per_page(size.item.height, size.contents.height / count as f32)
}

/// [`page_rows`] for a scrolled column of equal rows (its children).
pub fn page_rows_in(scroll: &ScrollHandle) -> isize {
    scroll.bounds_for_item(0).map_or(0, |row| {
        rows_per_page(scroll.bounds().size.height, row.size.height)
    })
}

fn rows_per_page(height: Pixels, row: Pixels) -> isize {
    if row <= Pixels::ZERO {
        return 0;
    }
    ((height / row).floor() as isize - 1).max(1)
}

/// [`wrap_step`], or with `stop_at_ends` (Corvene `620-lists-stop-at-ends`)
/// clamped: ↓ on the last row and ↑ on the first stay put.
pub fn list_step(ix: usize, delta: isize, count: usize, stop_at_ends: bool) -> usize {
    if stop_at_ends {
        (ix as isize + delta).clamp(0, count.saturating_sub(1) as isize) as usize
    } else {
        wrap_step(ix, delta, count)
    }
}

/// The top of selectable row `ix` in a list of groups of `group_sizes`
/// rows, each group preceded by a `header` tall header (group headers are
/// not selectable, `canSelectRow`): the groups above it, its own header and
/// [`row_offset_in_section`].
pub fn row_top(group_sizes: &[usize], ix: usize, header: Pixels, row: Pixels) -> Pixels {
    let mut top = Pixels::ZERO;
    let mut before = 0;
    for &size in group_sizes {
        if size == 0 {
            continue;
        }
        top += header;
        if ix < before + size {
            return top + row_offset_in_section(ix - before, |_| row);
        }
        top += row * size as f32;
        before += size;
    }
    top + row * (ix - before) as f32
}

/// GHD `getRowOffsetInSection(rowHeight, indexPath)`
/// (`ui/lib/list/section-list.tsx`): the top of `row` inside its section,
/// the heights (`row_height(row)`) of the rows above it added up.
pub fn row_offset_in_section(row: usize, row_height: impl Fn(usize) -> Pixels) -> Pixels {
    (0..row)
        .map(row_height)
        .fold(Pixels::ZERO, |sum, h| sum + h)
}

/// Scroll `handle` so the content span `top..top + height` is visible.
pub fn scroll_into_view(handle: &ScrollHandle, top: Pixels, height: Pixels) {
    let bottom = top + height;
    let viewport = handle.bounds().size.height;
    let mut offset = handle.offset();
    if top < -offset.y {
        offset.y = -top;
    } else if bottom > -offset.y + viewport {
        offset.y = viewport - bottom;
    }
    handle.set_offset(offset);
}

#[cfg(test)]
mod tests {
    use super::{list_step, row_offset_in_section, row_top, step, step_selectable};
    use gpui_kit::px;

    #[test]
    fn list_step_wraps_or_stops_at_the_ends() {
        assert_eq!(list_step(2, 1, 3, false), 0);
        assert_eq!(list_step(0, -1, 3, false), 2);
        assert_eq!(list_step(2, 1, 3, true), 2);
        assert_eq!(list_step(0, -1, 3, true), 0);
        assert_eq!(list_step(1, 1, 3, true), 2);
        assert_eq!(list_step(0, 1, 0, true), 0);
        // a page and Home / End clamp even when ↓ / ↑ wrap
        assert_eq!(list_step(1, 5, 3, false), 2);
        assert_eq!(list_step(1, -(1 << 30), 3, false), 0);
    }

    #[test]
    fn step_starts_at_the_ends_and_wraps() {
        assert_eq!(step(None, 1, 0), None);
        assert_eq!(step(None, 1, 3), Some(0));
        assert_eq!(step(None, -1, 3), Some(2));
        assert_eq!(step(Some(2), 1, 3), Some(0));
        assert_eq!(step(Some(0), -1, 3), Some(2));
        assert_eq!(step(Some(1), 1, 3), Some(2));
        // the only row stays highlighted
        assert_eq!(step(Some(0), 1, 1), Some(0));
    }

    #[test]
    fn step_skips_unselectable_rows() {
        assert_eq!(step_selectable(None, 1, 3, |ix| ix != 0), Some(1));
        assert_eq!(step_selectable(None, -1, 3, |ix| ix != 2), Some(1));
        assert_eq!(step_selectable(Some(1), 1, 3, |ix| ix == 1), Some(1));
        assert_eq!(
            row_offset_in_section(2, |r| px([30., 46., 30.][r])),
            px(76.)
        );
    }

    #[test]
    fn row_top_counts_the_headers_above() {
        let (h, r) = (px(10.), px(30.));
        assert_eq!(row_top(&[1, 2, 3], 0, h, r), px(10.));
        assert_eq!(row_top(&[1, 2, 3], 1, h, r), px(20.) + px(30.));
        assert_eq!(row_top(&[1, 2, 3], 3, h, r), px(30.) + px(90.));
        // an empty group has no header
        assert_eq!(row_top(&[0, 2], 1, h, r), px(10.) + px(30.));
    }
}
