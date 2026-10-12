//! Git engine. Reads via `gix`, writes and network via the
//! `git` CLI so behaviour matches GitHub Desktop exactly.

pub mod archive;
pub mod bisect;
pub mod blame;
pub mod branch_ops;
pub mod clean;
pub mod commit;
pub mod commit_template;
pub mod config;
pub mod config_lock;
pub mod credential;
pub mod description;
pub mod detect;
pub mod diff;
pub mod error;
pub mod font_file;
pub mod git_errors;
pub mod handle;
pub mod history_ops;
pub mod hook_env;
pub mod hooks;
pub mod ignore;
pub mod index_lock;
pub mod lfs;
pub mod lfs_locks;
pub mod lfs_progress;
pub mod log;
mod log_gix;
pub mod ops;
pub mod partial_stash;
pub mod patch;
pub mod patch_import;
pub mod paths;
pub mod process;
pub mod proxy;
pub mod rebase_ops;
pub mod reflog;
pub mod refs;
pub mod remote_ops;

/// Windows: Corvene is a GUI program, so a console program it starts would
/// open a console window of its own. Every child gets this creation flag.
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
pub mod repo;
pub mod signature;
pub mod sparse;
#[cfg(any(target_os = "android", all(test, unix)))]
mod spawn;
pub mod ssh;
pub mod stash_file_discard;
pub mod stash_ops;
pub mod stats;
pub mod status;
mod status_gix;
pub mod submodule;
pub mod table_file;
pub mod terminal;
pub mod text_encoding;
pub mod trivial_change;
pub mod utf16;
pub mod worktree;

pub use bisect::{
    BisectRange, BisectVerdict, bisect_in_progress, bisect_mark, bisect_range, bisect_reset,
    bisect_start, bisect_state, estimate_steps,
};
pub use blame::{
    BlameCommit, BlameEvent, BlameOptions, BlameRange, IGNORE_REVS_FILE, UNCOMMITTED_SHA, blame,
};
pub use branch_ops::{
    BranchTracking, CheckoutOptions, DESKTOP_STASH_MARKER, MergeOutcome, SubmoduleUpdate,
    abort_merge, branch_created_from, branch_tracking, checkout_branch, checkout_branch_with,
    checkout_new_branch, commits_ahead, commits_not_in, configured_default_branch, create_branch,
    create_desktop_stash, create_tracking_branch, delete_local_branch, delete_remote_branch,
    delete_remote_branch_with, desktop_stash_message, drop_desktop_stash_entry,
    ensure_no_modified_assume_unchanged, find_default_branch, get_branch_checkouts,
    get_branches_pointed_at, get_last_desktop_stash_entry_for_branch, get_merged_branches,
    get_stashes, is_local_changes_overwritten, last_desktop_stash_entry_index, merge_branch,
    merge_branch_with_message, modified_assume_unchanged, move_branch_back, parse_recent_branches,
    pop_stash_entry, pop_stash_on_branch, recent_branches, remote_head, rename_branch,
    set_upstream, stashed_files, unset_upstream, update_submodules_after_checkout,
};
pub use clean::{clean_dry_run, clean_paths, parse_clean_dry_run, unquote_c_path};
pub use commit::{
    CommitAuthor, CommitOptions, RECEIVE_LIMIT, add_paths, assume_unchanged_paths, commit,
    commit_with_terminal_output, delete_worktree_paths, discard_changes, format_message, head_sha,
    hook_exists, large_file_paths, merge_trailers, parse_commit_author, parse_commit_sha,
    restore_mode_changes, set_assume_unchanged, stage_files, stage_files_with_progress,
    staged_mode_changes, undo_last_commit, unstage_all,
};
pub use config::{
    IssueTracker, add_safe_directory, boolean_config_value, branch_merge_base,
    global_boolean_config_value, global_config_path, global_config_value, global_config_values,
    issue_trackers, local_config_value, remove_global_config_value, remove_local_config_value,
    set_branch_merge_base, set_default_branch, set_global_config_value, set_local_config_value,
    still_unsafe,
};
pub use config_lock::delete_config_lock_file;
pub use credential::{CredentialError, format_credential, parse_credential};
pub use description::{DEFAULT_GIT_DESCRIPTION, get_git_description, write_git_description};
pub use detect::{
    GitBinary, GitVersion, find_git, find_git_prefetched, find_git_prefetched_preferring,
    prefetch_git, still_usable,
};
pub use diff::{
    COMMIT_LINES_MAX_FILES, CommitLines, DecodedImage, ImageDecoder, PDF_MAX_BYTES, PDF_MEDIA_TYPE,
    SVG_MEDIA_TYPE, binary_media_type, blob_bytes, blob_lines, file_lines, has_hidden_bidi_chars,
    image_diff, image_diff_as, is_pdf, is_svg, lines_made_by_commit, open_difftool,
    parse_line_endings_warning, parse_raw_diff, parse_raw_diff_with_warnings, parse_unified,
    read_partial_file, set_image_decoder, set_pdf_diffs, submodule_diff, whitespace_only_paths,
    working_directory_diff, working_directory_patch, working_file_lines,
};
pub use error::{GitError, bad_config_line, dubious_ownership_path, explain_bad_config};
pub use git_errors::{
    EnvironmentError, GitErrorDetails, GitFailure, KnownGitError, display_command,
    environment_error, files_that_would_be_overwritten, git_error_details, is_lfs_auth_failure,
    is_signing_failure, known_git_error, lfs_auth_failure_url, merge_abort_blocked_paths,
    parse_config_lock_file_path_from_error,
};
pub use history_ops::{
    ResetMode, TagInfo, checkout_commit, cherry_pick_no_commit, create_tag, delete_tag,
    format_patch_range, format_patches, list_tags, reset_paths, reset_to, revert_commit,
    revert_commits_no_commit, revert_file_in_commit,
};
pub use ignore::{
    IgnoreTarget, append_ignore_files, append_ignore_rules, append_ignore_rules_to,
    escape_gitignore_pattern, excludes_file, gitignore_dirs_above, ignore_and_untrack,
    read_gitignore, save_gitignore, tracked_paths,
};
pub use index_lock::{index_lock_path, remove_stale_index_lock};
pub use lfs_locks::{LfsLock, LfsLocks, lfs_lock, lfs_locks, lfs_unlock};
pub use lfs_progress::GitLfsProgressParser;
pub use log::{
    COMMIT_BATCH_SIZE, HistoryQuery, LoggedCommit, LoggedHistory, NULL_TREE_SHA,
    REMERGE_DIFF_MIN_VERSION, StackRange, all_branch_tips, commit_exists, commit_file_diff,
    commit_matches_words, commit_range_file_diff, commits_by_sha, commits_with_sha_prefix,
    filtered_history, filtered_history_page, get_all_tags, get_changed_files,
    get_changed_files_in_process, get_commit_range_changed_files, get_commits, get_commits_from,
    get_commits_in_range, get_commits_with, local_commit_shas, local_only_commits, merge_base,
    merge_base_changed_files, merge_base_file_diff, most_recent_local_commit,
    parse_filtered_history, parse_raw_log_with_numstat, parse_recent_authors, range_changed_files,
    range_file_diff, recent_authors, remerge_changed_files, remerge_file_diff, resolve_commit,
    stack_range, stash_file_diff, tag_names,
};
pub use ops::{
    CloneOptions, CloneProgress, CloneProgressParser, InitOptions, PathStatus, clone,
    clone_failed_in_submodule, clone_with_options, explain_open_failure, explain_stale_worktree,
    global_identity, init_repository, init_repository_with, is_clone_path_sensitive,
    normalize_clone_url, parse_clone_progress, path_status, readme_exists, refresh_index,
    repositories_inside, repository_name_from_url, root_path_status, set_global_identity,
};
pub use partial_stash::create_partial_stash;
pub use patch::{
    PatchOptions, apply_patch_to_index, discard_changes_from_selection, format_patch,
    format_patch_to_discard_changes, format_patch_to_discard_changes_with, format_patch_with,
    stage_partial_files, stage_partial_files_with, stage_partial_files_with_progress,
};
pub use patch_import::{
    PatchApply, PatchFile, PatchFileChange, PatchPreview, apply_mailbox, apply_patch,
    mailbox_subjects, preview_patch,
};
pub use paths::git_dir;
pub use process::{
    CancelToken, GitCommand, GitOutput, set_credential_helper, set_explain_missing_workdir,
    set_network_stall_timeout, with_cancel_token,
};
pub use proxy::{
    GitProxy, GitProxySource, effective_proxy, env_for_proxy, env_for_remote_operation,
    is_proxy_auth_failure, parse_pac_string, resolve_git_proxy, set_proxy_logins,
    set_system_proxy_resolver,
};
pub use rebase_ops::{
    CherryPickResult, CherryPickSnapshot, RebaseOptions, RebaseResult, RebaseSnapshot, UpdateRefs,
    abort_cherry_pick, abort_rebase, abort_squash_merge, autosquash, autosquash_pairs,
    binary_paths, cherry_pick, cherry_pick_head_found, cherry_pick_snapshot, commits_between,
    commits_in_range, conflict_marker_counts, continue_cherry_pick, continue_rebase,
    continue_squash_rebase, create_merge_commit, determine_mergeability, fixup_todo,
    merge_commits_exist_after, merge_head_set, merge_tree_conflicts, open_merge_tool,
    parse_merge_tree_names, rebase, rebase_head_set, rebase_internal_state, rebase_snapshot,
    reorder, restore_branch_tips, reword, reword_head, reword_todo, squash, squash_msg_set,
    stage_manual_conflict_resolution, stash_tip, with_update_refs,
};
pub use reflog::{
    CommitKind, RebaseStep, ReflogAction, ReflogEntry, parse_action as parse_reflog_action,
    reachable_shas, read_reflog,
};
pub use refs::{delete_ref, format_as_local_ref, get_symbolic_ref};
pub use remote_ops::{
    AskpassEnv, FastForward, FetchOptions, GitProgressEvent, ProgressLine, ProgressParser,
    PushProgress, PushSize, RemoteFailure, TrackingBranch, add_remote, classify_remote_failure,
    cloned_at, commits_not_on_remote, config_value, delete_remote_tag,
    fast_forward_branch_from_remote, fast_forward_branches, fast_forward_branches_with,
    fast_forward_if_only_behind, fast_forward_outcome, fast_forward_tracking_branches, fetch,
    fetch_refspec, fetch_remote_branch, fetch_tags, fetch_tags_to_push, fetch_with,
    fetch_with_prune_tags, files_not_tracked_by_lfs, find_default_remote,
    get_branches_differing_from_upstream, get_remotes, implicit_push_remote, install_lfs_hooks,
    is_stale_remote_ref_failure, is_tracked_by_lfs, is_using_lfs, is_using_lfs_by_attributes,
    last_fetched, lfs_available, lfs_hooks_installed, ls_remote_heads, parse_progress_line,
    previous_tag, prune_remote, pull, pull_merge_started, pull_with_rebase, push, push_size,
    push_tag, push_with_progress, remote_branches_containing, remote_failure, remote_head_resolves,
    remote_name_is_valid, remote_read_failure_cause, remote_repository_missing, remove_remote,
    rename_remote, set_remote_url, strip_vt_control_characters, track_in_lfs, update_remote_head,
    update_submodules, upstream_tip_in_reflog,
};
pub use repo::{
    ahead_behind, has_stash, main_worktree_path, open_repository, symmetric_ahead_behind,
    top_level_working_directory,
};
pub use sparse::{
    SparseCheckout, sparse_checkout, sparse_checkout_disable, sparse_checkout_enabled,
    sparse_checkout_set, tree_directories,
};
pub use ssh::{AddSshHostInfo, parse_add_ssh_host_prompt};
pub use stash_file_discard::{DiscardFromStash, discard_files_from_stash, put_back_stash_entry};
pub use stash_ops::{
    ADD_TO_STASH_MIN_VERSION, AddToStash, StashPop, StashPopOptions, add_to_desktop_stash,
    apply_stash_entry_with, create_branch_from_stash, create_desktop_stash_of_files,
    create_stash_with_message, mark_conflicts_resolved, pop_stash_entry_with, stash_entry,
    store_stash, unmerged_paths,
};
pub use stats::{
    Contributor, FileChurn, RepoStats, StatsScope, TOP_FILES, WeekBucket, repository_stats,
    stats_commit_count, week_start,
};
pub use status::{
    IgnoreSubmodules, LineStats, StatusOptions, get_status, get_status_in_process, get_status_with,
    map_status, parse_porcelain_v2, refresh_stale_index, working_directory_line_stats,
};
pub use submodule::{
    EmbeddedRepository, SubmoduleDetails, SubmoduleEntry, SubmoduleState,
    add_embedded_repositories, embedded_repositories, list_submodules, reset_submodule_paths,
    submodule_details, submodule_init, submodule_sync, submodule_update,
    update_submodules_after_operation,
};
pub use terminal::{
    TailStream, TerminalOutput, TerminalOutputCallback, TerminalOutputListener,
    TerminalOutputSubscriber, TerminalStream, Unsubscribe,
    create_multi_operation_terminal_output_callback, create_tail_stream, create_terminal_stream,
    push_terminal_chunk,
};
pub use worktree::{
    add_worktree, list_worktrees, list_worktrees_from_git_dir, move_worktree,
    parse_worktree_porcelain, remove_worktree, resolve_main_worktree_path,
};
