//! Corvene addition (`774-stash-conflict-flow`, no GHD file): while files a
//! stash restore left conflicted are in the status (no merge, rebase or
//! cherry-pick explains them), the commit form gives way to a list of them,
//! as GHD's `ContinueRebase` does during a rebase. Each row says how many
//! conflicts remain, opens the file (editor, merge tool, default program,
//! Finder) and marks it resolved once no markers are left
//! (`Dispatcher::mark_stash_conflicts_resolved`, the index only). GHD shows
//! the files as conflicted and offers nothing to resolve them with.

use corvene_core::{AppState, Dispatcher, WorkingDirectoryFileChange};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::context_menu::{MenuItem, labels, mac_or};
use crate::icons::{Octicon, octicon};
use crate::scrollbar::ScrollbarExt;
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::{IconButtonA11y, button_disabled, small_button};

/// The rows shown before the list scrolls.
const VISIBLE_ROWS: f32 = 5.;

/// The block in place of the commit form, or `None` when the flag is off or
/// no such conflict is left.
pub fn stash_conflicts_block(state: &Entity<AppState>, cx: &App) -> Option<AnyElement> {
    let s = state.read(cx);
    if !s.flags.bool(corvene_core::flags::ids::STASH_CONFLICT_FLOW) {
        return None;
    }
    let id = s.selected?;
    let rs = s.repo_states.get(&id)?;
    let files: Vec<WorkingDirectoryFileChange> =
        rs.stash_conflicted_files().into_iter().cloned().collect();
    if files.is_empty() {
        return None;
    }
    let workdir = rs.info.as_ref()?.workdir.clone();
    let from_stash = rs.kept_stash_for(&workdir).is_some();
    let editor = s.editor_label();
    let t = cx.ghd();
    let title = if from_stash {
        mac_or(
            "Stash Restored with Conflicts",
            "Stash restored with conflicts",
        )
    } else {
        mac_or("Unresolved Conflicts", "Unresolved conflicts")
    };
    let hint = if from_stash {
        "Git kept the stash. Resolve each file, then mark it as resolved."
    } else {
        "Resolve each file, then mark it as resolved."
    };
    let rows = files.iter().map(|file| {
        let path = file.path.clone();
        let full = workdir.join(&file.path);
        let (text, color, resolvable) = match file.status.conflict_markers {
            Some(0) => ("No conflicts".to_string(), t.color_new, true),
            Some(markers) => {
                let conflicts = markers.div_ceil(3);
                (
                    if conflicts == 1 {
                        "1 conflict".to_string()
                    } else {
                        format!("{conflicts} conflicts")
                    },
                    t.color_conflicted,
                    false,
                )
            }
            None => ("Manual conflict".to_string(), t.color_conflicted, true),
        };
        let menu = {
            let (open, tool, default, reveal) =
                (full.clone(), path.clone(), full.clone(), full.clone());
            let editor = editor.clone();
            move |ev: &ClickEvent, window: &mut Window, cx: &mut App| {
                let (open, tool, default, reveal) =
                    (open.clone(), tool.clone(), default.clone(), reveal.clone());
                let items = vec![
                    MenuItem::new(labels::open_in(&editor), move |_, cx| {
                        Dispatcher::open_in_editor(open.clone(), cx)
                    }),
                    MenuItem::new(
                        mac_or("Open in Merge Tool", "Open in merge tool"),
                        move |_, cx| {
                            Dispatcher::open_stash_conflict_in_merge_tool(id, tool.clone(), cx)
                        },
                    ),
                    MenuItem::new(labels::OPEN_WITH_DEFAULT_PROGRAM, move |_, cx| {
                        cx.open_with_system(&default)
                    }),
                    MenuItem::new(labels::REVEAL_IN_FILE_MANAGER, move |_, cx| {
                        Dispatcher::show_in_finder(&reveal, cx)
                    }),
                ];
                let position = ev.mouse_position().unwrap_or_default();
                crate::native_menu::show_context_menu(items, position, window, cx);
            }
        };
        // `1361-merge-editor`
        let merge_editor = corvene_core::merge_editor::enabled(state.read(cx)).then(|| {
            let path = path.clone();
            small_button(
                SharedString::from(format!("stash-conflict-merge-{path}")),
                "Resolve",
                cx,
            )
            .on_click(move |_, _, cx| Dispatcher::resolve_in_app(id, path.clone(), cx))
        });
        let resolve_id = SharedString::from(format!("stash-conflict-resolve-{path}"));
        let resolve_label = mac_or("Mark as Resolved", "Mark as resolved");
        let resolve = if resolvable {
            let paths = vec![path.clone()];
            small_button(resolve_id, resolve_label, cx)
                .on_click(move |_, _, cx| {
                    Dispatcher::mark_stash_conflicts_resolved(id, paths.clone(), cx)
                })
                .into_any_element()
        } else {
            button_disabled(resolve_id, resolve_label, cx)
                .h(zpx(21.))
                .px(SPACING_HALF())
                .text_size(FONT_SIZE_SM())
                .into_any_element()
        };
        div()
            .id(SharedString::from(format!("stash-conflict-{path}")))
            .flex()
            .flex_col()
            .gap(zpx(2.))
            .py(SPACING_HALF())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING_HALF())
                    .child(octicon(Octicon::FileCode, t.text))
                    .child(div().flex_1().min_w_0().truncate().child(path.clone()))
                    .children(merge_editor),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING_HALF())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(FONT_SIZE_SM())
                            .text_color(color)
                            .child(text),
                    )
                    .child(
                        small_button(
                            SharedString::from(format!("stash-conflict-open-{path}")),
                            "",
                            cx,
                        )
                        .child(octicon(Octicon::KebabHorizontal, t.secondary_button_text))
                        .icon_button_label(mac_or("Open File", "Open file"))
                        .on_click(menu),
                    )
                    .child(resolve),
            )
    });
    let row_height = zpx(52.);
    let list = div()
        .id("stash-conflicts-list")
        .flex()
        .flex_col()
        .max_h(row_height * VISIBLE_ROWS)
        .overflow_y_scroll()
        .children(rows)
        .with_scrollbar();
    Some(
        div()
            .id("stash-conflicts")
            .flex_none()
            .flex()
            .flex_col()
            .p(SPACING())
            .gap(SPACING_HALF())
            .bg(t.box_alt_background)
            .border_t_1()
            .border_color(t.box_border)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING_HALF())
                    .child(octicon(Octicon::Alert, t.color_conflicted))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(title)),
            )
            .child(
                div()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(hint),
            )
            .child(list)
            .into_any_element(),
    )
}
