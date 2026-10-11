//! Corvene UI: a GPUI recreation of GitHub Desktop's chrome.
//!
//! Geometry and colours come from `.docs/ghd-ui-inventory.md` and
//! `.docs/ghd-theme-tokens.md`. Views never touch git, network or disk;
//! they dispatch actions that `corvene-core` handles.

pub mod actions;
pub mod active_resizable;
pub mod app_menu;
pub mod autocompletion;
pub mod banner;
pub mod bisect_bar;
pub mod blame_view;
pub mod branch_list;
pub mod changes;
pub mod ci_check_popover;
pub mod ci_status;
pub mod cloneable_repositories;
pub mod cloning_view;
pub mod code_owners;
pub mod commit_graph;
pub mod commit_message_avatar;
pub mod context_menu;
pub mod copy_button;
pub mod archive_diff_view;
pub mod archive_list_rows;
pub mod csv_table_diff;
pub mod document_diff;
pub mod dialog;
pub mod dialogs;
pub mod diff_expansion;
pub mod diff_view;
pub mod diff_view_rows;
pub mod file_icons;
pub mod file_tree_rows;
pub mod filter_list;
pub mod foldout;
pub mod font_diff;
pub mod format;
pub mod git_email_not_found_warning;
pub mod github_list;
pub mod history;
pub mod hook_results;
pub mod icons;
pub mod image_diff;
pub mod insights_view;
pub mod issue_view;
pub mod issues_list;
pub mod keyboard_nav;
pub mod keymap;
pub mod link_menu;
pub mod markdown;
pub mod markdown_editor;
#[cfg(not(target_os = "macos"))]
pub mod menu_bar;
pub mod missing_repository;
#[cfg_attr(not(target_os = "macos"), path = "native_menu_linux.rs")]
pub mod native_menu;
mod native_menu_common;
#[cfg(target_os = "macos")]
pub mod native_window;
pub mod no_changes;
pub mod no_repositories;
pub mod path_label;
pub mod path_text;
pub mod pdf_diff;
pub mod popover;
pub mod progress_bar;
pub mod pull_request_list;
pub mod pull_request_overview;
pub mod pull_request_review_list;
pub mod pull_request_review_view;
pub mod ref_compare_view;
pub mod reflog_list;
pub mod relative_time;
pub mod release_view;
pub mod releases_list;
pub mod repository_drag;
pub mod repository_list;
pub mod review_threads;
pub mod scrollbar;
pub mod selected_commit;
pub mod signature_badge;
pub mod stacked_diff_view;
pub mod stash_conflicts;
pub mod stash_list;
pub mod stash_view;
#[cfg(target_os = "macos")]
pub mod status_item;
pub mod tab_bar;
pub mod tab_strip;
pub mod tags_list;
pub mod terminal;
pub mod theme;
pub mod title_bar;
#[cfg(windows)]
pub mod title_bar_windows;
pub mod toolbar;
pub mod trivial_change;
pub mod tutorial_panel;
#[cfg(not(target_os = "macos"))]
pub mod views_menu;
pub mod welcome;
pub mod widgets;
pub mod windows;
pub mod workspace;
pub mod worktree_list;

use gpui_kit::App;

/// Install the theme global and keymap and start the relative times'
/// refresh. Call once after `gpui_kit::init`.
pub fn init(cx: &mut App, theme: theme::GhdTheme) {
    theme::init(cx, theme);
    keymap::install(cx);
    relative_time::start_refresh(cx);
}
