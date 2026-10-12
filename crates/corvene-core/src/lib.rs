//! Application state and the dispatcher that drives backends.
//! Mirrors GitHub Desktop's `AppStore` / `Dispatcher` / `RepositoryStateCache`.

pub mod accounts;
pub mod acknowledgements;
pub mod ahead_behind_store;
pub mod alive;
pub mod app_location;
pub mod app_url;
pub mod apply_patch;
pub mod archive_browse;
pub mod askpass;
pub mod autocomplete;
pub mod avatar_users;
pub mod avatars;
pub mod banner_focus;
pub mod bibtex;
pub mod bisect;
pub mod blame;
pub mod block_diff;
pub mod bookmarks;
pub mod branch_pruner;
pub mod branch_tags;
pub mod changelists;
pub mod changes_state;
pub mod clean_untracked;
pub mod clone_info;
pub mod cloning_repositories_store;
pub mod codeowners;
pub mod commit_checks;
pub mod commit_graph;
pub mod commit_message;
pub mod commit_progress;
pub mod commit_status;
pub mod compare;
pub mod crash_reports;
pub mod csv_diff;
pub mod delete_branches;
pub mod diff_cache;
pub mod diff_line_class;
pub mod dispatcher;
pub mod document_diff;
pub mod document_pages;
pub mod drafts;
pub mod emoji;
pub mod extensions;
pub mod file_icons;
pub mod file_tree;
pub mod filter;
pub mod flags;
pub mod fork_checkout;
pub mod forks;
pub mod ghd_import;
pub mod git_config_import;
pub mod git_store;
pub mod github_actions;
pub mod github_links;
#[cfg(any(target_os = "android", test))]
pub mod headless;
pub mod history_filter;
pub mod hooks;
pub mod host;
pub mod hosts;
pub mod image_decode;
pub mod insights;
pub mod integrations;
pub mod issues;
pub mod job_log;
pub mod keymap_file;
pub mod latex_doc;
pub mod latex_pages;
pub mod lfs_locks;
pub mod line_selection;
pub mod list_selection;
pub mod markdown;
pub mod markdown_doc;
pub mod markdown_filters;
pub mod markdown_images;
pub mod mco;
pub mod menu_bar_status;
pub mod menu_state;
pub mod navigation;
pub mod new_branch_flows;
pub mod notebook_diff;
pub mod notebook_stage;
pub mod notifications;
pub mod offset_from;
pub mod packs;
pub mod pdf;
pub mod persistence;
pub mod popup_manager;
pub mod portable_paths;
pub mod proxy;
pub mod pull_request_events;
pub mod pull_request_preview;
pub mod pull_request_review;
pub mod pull_requests;
pub mod push_errors;
pub mod push_target;
pub mod ref_compare;
pub mod reflog;
pub mod release_notes;
pub mod releases;
pub mod remote;
pub mod remote_manager;
pub mod repo_rules;
pub mod repositories_store;
pub mod repository_accounts;
pub mod repository_list_file;
pub mod review_anchor;
pub mod round;
pub mod samples;
pub mod settings_file;
pub mod shared_storage;
pub mod sign_in;
pub mod signatures;
pub mod sparse_checkout;
pub mod ssh_keys;
pub mod stacked_diff;
pub mod stacked_refs;
pub mod stash_flows;
pub mod stash_list;
pub mod state;
pub mod submodules;
pub mod tag_manager;
pub mod templates;
pub mod text_tokens;
pub mod toolbar_widths;
pub mod tutorial;
pub mod updater;
pub mod watcher;
pub mod web_flow;
pub mod workspace;
pub mod worktrees;

pub use alive::{AliveEventData, AliveState};
pub use autocomplete::{
    DEFAULT_MAX_HITS, Issue, IssueCache, IssueHit, MentionableCache, MentionableUser, Trigger,
    TriggerKind, find_trigger, folder_completions, issues_matching, users_matching,
};
pub use avatar_users::{AvatarUser, get_avatar_users_for_commit};
pub use avatars::{AvatarEntry, avatar_for_email, avatar_for_url, initials, initials_hue};
pub use commit_status::{CommitStatusStore, combined_status_summary, group_check_runs, status_key};
pub use compare::{CompareForm, CompareState, ComparisonMode};
pub use corvene_models::*;
pub use dispatcher::Dispatcher;
pub use emoji::CustomEmoji;
pub use flags::{FlagId, FlagOverrides, Flags};
pub use forks::UPSTREAM_REMOTE_NAME;
pub use host::{AsyncCtx, Ctx, Host, HostServices, StateCx, StateHandle};
pub use hosts::{HostSignIn, HostsState};
pub use integrations::{EditorChoice, PreferencesSave, RepositorySettingsSave};
pub use job_log::{
    JobLog, JobLogLine, JobLogState, JobLogStore, LineKind, job_log_key, parse_job_log,
};
pub use mco::{
    Banner, ConflictKind, ConflictState, McoConflicts, McoDetail, McoStep, McoUndo, MergePreview,
    MultiCommitOperation, RebasePreview, conflicted_files, get_unique_coauthors_as_authors,
    resolved_files, unmerged_files,
};
pub use packs::{OFFERED_PACKS, PackProgress, PacksState, offered_packs};
pub use persistence::DocumentDiffView;
pub use persistence::{
    CodeOwnersDisplay, CustomIntegration, DEFAULT_DATE_FORMAT, DEFAULT_NUMBER_FORMAT,
    DEFAULT_TIME_FORMAT, NotificationClickAction, PullRequestEventNotifications, Settings,
    StoreExt, TAB_SIZE_DEFAULT, UncommittedChangesStrategy,
};
pub use persistence::{LatexDiffView, MarkdownDiffView, NotebookDiffView};
pub use popup_manager::{AppError, PopupManager, PopupType, StackedPopup};
pub use pull_request_preview::{MergeStatus, PreviewSlot, PullRequestPreview};
pub use pull_requests::{
    BranchesTab, FORKED_REMOTE_PREFIX, PullRequestCache, PullRequestCaches, cache_key,
    find_associated_pull_request, fork_pull_request_remote_name,
};
pub use remote::{ForcePushState, PushPullKind, PushPullProgress, RepoIndicator, host_of};
pub use repo_rules::{append_trailers, failed_rules, rule_matches};
pub use shared_storage::{SharedStorageAccess, SharedStorageDestination};
pub use state::{
    AfterSharedStorageMove, AppState, AuthenticationFlow, AuthenticationStep, CloneState,
    CommitMessage, DropTarget, FileListFilter, FilterOption, Foldout, GitConfigLocation,
    GlobalGitConfig, LastCommit, Popup, PreferencesTab, RepositorySettingsData,
    RepositorySettingsTab, RepositoryState, RetryAction, SharedStorageMove, SharedStorageMoveStage,
    SharedStorageMoveState, SigningConfig, UnreachableCommitsTab,
};
pub use updater::{AvailableUpdate, PackageManager, UpdateState, UpdateStatus};
pub use workspace::{SavedWorkspace, WorkspaceId, WorkspaceState};
