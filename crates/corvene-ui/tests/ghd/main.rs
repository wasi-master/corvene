//! GitHub Desktop 3.6.6 unit tests ported to Corvene. Each module is one
//! GitHub Desktop test file (`app/test/unit/...`); `*_support` modules hold
//! helpers shared inside this crate. See `tools/ghd-tests/README.md`.

mod bytes;
mod create_branch;
mod diff_parser;
mod format_number;
mod git_apply;
mod git_store;
mod group_branches;
mod lists_support;
mod main_process_menu;
mod octicon;
mod path_text;
mod release_notes;
mod repositories_clone_grouping;
mod repositories_list_grouping;
mod sanitize_ref_name;
mod sanitized_repository_name;
mod section_list;
mod section_list_selection;
mod status_utils;
mod text_diff_expansion;
mod ui_banner_surfaces;
mod ui_branch_empty_states;
mod ui_branch_list_item;
mod ui_component_primitives;
mod ui_copy_button;
mod ui_dialog_action_wrappers;
mod ui_email_attribution_warning;
mod ui_layout_and_message_components;
mod ui_path_and_selection_surfaces;
mod ui_path_text_and_link_button;
mod ui_relative_time;
mod ui_repository_list_item;
mod ui_rulesets_and_publish_surfaces;
mod ui_small_action_and_dialog_surfaces;
mod ui_static_status_and_link_components;
mod ui_structural_and_text_components;
mod ui_tab_bar;
mod ui_tab_bar_item;
mod ui_tutorial_welcome_surfaces;
mod ui_visual_helper_surfaces;
mod ui_welcome_and_sign_in_wrappers;
