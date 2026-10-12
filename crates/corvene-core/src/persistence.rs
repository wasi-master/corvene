//! Typed accessors over the generic `corvene_store::Store`.
//!
//! GHD keeps each preference in its own `localStorage` key and reads it with
//! a fallback (`lib/local-storage.ts` `getBoolean` and friends), so one
//! unreadable value only resets that preference: [`StoreExt::settings`] does
//! the same for the fields of the stored `settings` record
//! ([`settings_from_value`]).
//!
//! Accounts load as in GHD's `AccountsStore.loadFromStore`
//! (`lib/stores/accounts-store.ts`): a `*.ghe.com` account still on the
//! `/api/v3` endpoint is moved to the `api.` subdomain and the migrated list
//! is saved (`getMigratedGHEAccounts`). The GitHub repositories of the
//! repository list get the same move when they load (GHD matches them to
//! the accounts again on selection, which Corvene does not do yet), so they
//! keep matching their account.
//!
//! Deviation ([`StoreExt::repositories_keeping_unreadable`], flag
//! `287-repository-list-backup`): an entry of the repository list this
//! version cannot read is moved aside (`repositories.unreadable`) instead of
//! the whole list loading empty and the next save overwriting it, and comes
//! back once a version reads it; [`backup_on_version_change`] copies the
//! store file when the app version changes. GHD's repository list lives in
//! IndexedDB (`lib/databases/repositories-database.ts`) with no backup.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use corvene_store::{Result, Store};
use serde::{Deserialize, Serialize};
use tracing::warn;

use corvene_models::{Account, Repository, SyntaxHighlighter, ThemeSetting};

/// User settings persisted across launches (subset of GHD's preferences).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeSetting,
    /// Corvene: the Android app's design language (flag `113-design-style`).
    pub design_style: corvene_models::DesignStyle,
    pub sidebar_width: f32,
    /// History file-list width (`commitSummaryWidth`, default 250).
    pub commit_summary_width: f32,
    /// Corvene `114-resizable-commit-message`: the commit description box's
    /// height in CSS px; `None` is GHD's 80.
    pub commit_description_height: Option<f32>,
    /// Resized toolbar buttons (`branch-dropdown-width`,
    /// `worktree-dropdown-width`); `None` is the 230 px default.
    pub branch_dropdown_width: Option<f32>,
    pub worktree_dropdown_width: Option<f32>,
    /// Settings › Advanced › "Save crash reports locally" (Corvene addition).
    pub save_crash_reports: bool,
    /// When Corvene last started (seconds since the epoch): crash reports
    /// newer than this are from the previous session.
    pub last_launched_at: Option<u64>,
    /// GHD `last-successful-update-check` (seconds since the epoch).
    #[serde(default)]
    pub last_successful_update_check: Option<u64>,
    /// View › Zoom (Electron's `zoomFactor`, GHD `ZoomInFactors` steps).
    #[serde(default = "default_zoom")]
    pub window_zoom_factor: f32,
    /// Tutorial assessor state (GHD `tutorial-install-editor-skipped`,
    /// `tutorial-pull-request-step-complete`, `tutorial-paused`).
    pub tutorial_install_editor_skipped: bool,
    pub tutorial_pull_request_step_complete: bool,
    pub tutorial_paused: bool,
    pub clone_dir: Option<PathBuf>,
    /// GHD `hasShownWelcomeFlow`.
    pub welcome_completed: bool,
    /// GHD `askForConfirmationOnDiscardChanges`.
    pub confirm_discard_changes: bool,
    /// Corvene `785-embedded-repo-commit`: "Do not show this message again"
    /// on the note about nested repositories without an `origin`.
    pub hide_embedded_repository_note: bool,
    /// GHD `askForConfirmationOnCheckoutCommit`.
    pub confirm_checkout_commit: bool,
    /// GHD `askForConfirmationOnUndoCommit`.
    pub confirm_undo_commit: bool,
    /// Flag `807`: History lists first parents only (`git log --first-parent`).
    pub history_first_parent: bool,
    /// Flag `1213`: History lists every branch's commits ("All branches").
    pub history_all_branches: bool,
    /// GHD `uncommittedChangesStrategy` ("If I have changes and I switch branches…").
    pub uncommitted_changes_strategy: UncommittedChangesStrategy,
    /// GHD `askForConfirmationOnDiscardStash`.
    pub confirm_discard_stash: bool,
    /// GHD `confirmWorktreeRemoval` (Prompts › Removing worktrees).
    pub confirm_worktree_removal: bool,
    /// GHD `confirmCommitMessageOverride` (Prompts › Overriding commit
    /// message with generated message). Corvene generates no commit messages:
    /// kept only for the checkbox flag `512-copilot-prompt-omitted` shows.
    #[serde(default = "default_true")]
    pub confirm_commit_message_override: bool,
    /// GHD `askToMoveToApplicationsFolder` ("Do not show this message again"
    /// in the Move to Applications prompt clears it).
    pub ask_to_move_to_applications_folder: bool,
    /// GHD `askForConfirmationOnForcePush`.
    #[serde(default = "default_true")]
    pub confirm_force_push: bool,
    /// GHD `externalEditor`: the friendly name of the selected editor, `None`
    /// = first installed one.
    #[serde(default)]
    pub external_editor: Option<String>,
    /// GHD `shell`: the label of the selected shell, `None` = Terminal.
    #[serde(default)]
    pub shell: Option<String>,
    /// GHD `notificationsEnabled`.
    #[serde(default = "default_true")]
    pub notifications_enabled: bool,
    /// GHD `confirmRepoRemoval`.
    #[serde(default = "default_true")]
    pub confirm_repository_removal: bool,
    /// GHD `askForConfirmationOnDiscardChangesPermanently`.
    #[serde(default = "default_true")]
    pub confirm_discard_changes_permanently: bool,
    /// GHD `askForConfirmationOnCommitFilteredChanges`.
    #[serde(default = "default_true")]
    pub confirm_commit_filtered_changes: bool,
    /// GHD `showCommitLengthWarning`.
    #[serde(default = "default_true")]
    pub show_commit_length_warning: bool,
    /// GHD `commitSpellcheckEnabled` (toggled from the commit form's context menu).
    pub commit_spellcheck_enabled: bool,
    /// GHD `repositoryIndicatorsEnabled` (Advanced › Background updates).
    #[serde(default = "default_true")]
    pub repository_indicators_enabled: bool,
    /// Corvene (`428-menu-bar-status-item`): the repositories (ids) the menu
    /// bar status item watches.
    #[serde(default)]
    pub menu_bar_repositories: Vec<u64>,
    /// Corvene (`354-pull-request-event-notifications`): which pull request
    /// events of any repository become notifications (Notifications tab).
    #[serde(default)]
    pub pull_request_event_notifications: PullRequestEventNotifications,
    /// Corvene (`354`): what a click on one of those notifications opens.
    #[serde(default)]
    pub pull_request_notification_click: NotificationClickAction,
    /// Corvene (`1314-code-owners`): how a file row shows its owners
    /// (Settings › Appearance › Code owners).
    #[serde(default)]
    pub code_owners_display: CodeOwnersDisplay,
    /// GHD `useExternalCredentialHelper` (Git Credential Manager).
    #[serde(default)]
    pub use_external_credential_helper: bool,
    /// GHD `underlineLinks` (Accessibility), on by default
    /// (`underlineLinksDefault = true`).
    #[serde(default = "default_true")]
    pub underline_links: bool,
    /// GHD `showDiffCheckMarks` (Accessibility).
    #[serde(default = "default_true")]
    pub show_diff_check_marks: bool,
    /// Diff Settings › Hide Whitespace Changes (`hideWhitespaceInChangesDiff`).
    pub hide_whitespace_in_changes_diff: bool,
    /// Same for the History tab (`hideWhitespaceInHistoryDiff`).
    pub hide_whitespace_in_history_diff: bool,
    /// Same for the Preview Pull Request dialog (`hideWhitespaceInPullRequestDiff`).
    #[serde(default)]
    pub hide_whitespace_in_pull_request_diff: bool,
    /// Preview Pull Request file-list width (`pullRequestFileListWidth`, default 250).
    #[serde(default = "default_file_list_width")]
    pub pull_request_file_list_width: f32,
    /// Diff Settings › Diff display › Split (`showSideBySideDiff`).
    pub show_side_by_side_diff: bool,
    /// Corvene `1304-diff-no-wrap`: View › Wrap Diff Lines (on: long
    /// lines wrap, GHD's only behaviour).
    #[serde(default = "default_true")]
    pub diff_wrap_lines: bool,
    /// Corvene `1316-diff-moved-lines`: Diff Settings › Mark Moved Lines.
    #[serde(default = "default_true")]
    pub diff_mark_moved_lines: bool,
    /// Corvene `1317-diff-stylistic-changes`: Diff Settings › Mark
    /// Stylistic Changes.
    #[serde(default = "default_true")]
    pub diff_mark_stylistic_changes: bool,
    /// Corvene `1318-csv-table-diff`: CSV and TSV diffs open as a table
    /// (off: as text). The diff header's Table/Text switch flips it.
    #[serde(default = "default_true")]
    pub csv_table_diff: bool,
    /// Corvene `1322-pdf-diff`: PDF diffs show the text on their pages
    /// (off: the pages). The diff header's Pages/Text switch flips it.
    #[serde(default)]
    pub pdf_text_diff: bool,
    /// Corvene `794-svg-image-diff`: SVG diffs show the text (off: the
    /// drawings). The diff header's Text/Image switch flips it.
    #[serde(default)]
    pub svg_text_diff: bool,
    /// Corvene `1323-archive-diff`: the archive view lists unchanged
    /// entries too.
    #[serde(default)]
    pub archive_show_unchanged: bool,
    /// Corvene `1323-archive-diff`: the archive view lists entries whose
    /// only change is their permissions, time, owner or compression.
    #[serde(default)]
    pub archive_show_metadata_only: bool,
    /// Corvene `1323-archive-diff`: the archive view shows every changed
    /// entry's diff in one list instead of the selected one's.
    #[serde(default)]
    pub archive_stacked: bool,
    /// Corvene `1323-archive-diff`: width of the archive view's entry tree.
    #[serde(default = "default_file_list_width")]
    pub archive_tree_width: f32,
    /// Corvene `1330-font-source-preview`: font source diffs show the text
    /// (off: the glyphs). The diff header's Text / Glyphs switch flips it.
    #[serde(default)]
    pub font_source_text_diff: bool,
    /// Corvene `1324-document-diff`: how Word, PowerPoint, OpenDocument,
    /// EPUB and RTF diffs are shown (the diff header's switch).
    #[serde(default)]
    pub document_diff_view: DocumentDiffView,
    /// Corvene `1331-notebook-diff`: how notebooks are shown (the diff
    /// header's Notebook / Script / JSON switch).
    #[serde(default)]
    pub notebook_diff_view: NotebookDiffView,
    /// Corvene `1331-notebook-diff`: the notebook view's filter menu.
    #[serde(default)]
    pub notebook_filters: crate::notebook_diff::NotebookFilters,
    /// Corvene `1332-latex-diff`: how LaTeX and BibTeX files are shown (the
    /// diff header's Rendered / Text / Pages switch).
    #[serde(default)]
    pub latex_diff_view: LatexDiffView,
    /// Corvene `1334-markdown-preview-diff`: how Markdown files are shown
    /// (the diff header's Source / Preview switch).
    #[serde(default)]
    pub markdown_diff_view: MarkdownDiffView,
    /// Corvene `1310-file-list-tree`: View › Show Changes as Tree (file
    /// lists show folders; off: GHD's flat list).
    #[serde(default)]
    pub file_list_tree: bool,
    /// Corvene `117-file-icons`: Appearance › File icons
    /// (`corvene_core::file_icons`).
    #[serde(default = "default_file_icon_theme")]
    pub file_icon_theme: String,
    /// Corvene `1311-stacked-diff`: the Changes tab shows the included
    /// files' diffs stacked in one list.
    #[serde(default)]
    pub stacked_diff_changes: bool,
    /// Corvene `1311-stacked-diff`: History shows every file of the
    /// selected commit stacked in one list.
    #[serde(default)]
    pub stacked_diff_history: bool,
    /// Last chosen tab of a modified-image diff (`imageDiffType`).
    pub image_diff_type: corvene_models::ImageDiffType,
    /// Corvene `1329-font-diff`: the font viewer's last tab.
    #[serde(default)]
    pub font_diff_view: corvene_models::FontDiffView,
    /// Corvene `1335-video-diff`: a modified video's last tab.
    #[serde(default)]
    pub video_diff_mode: corvene_models::VideoDiffMode,
    /// Corvene `1336-audio-diff`: a modified sound's last tab.
    #[serde(default)]
    pub audio_diff_mode: corvene_models::AudioDiffMode,
    /// Corvene `1335-video-diff` / `1336-audio-diff`: the players' volume,
    /// 0..1.
    pub media_volume: f32,
    #[serde(default)]
    pub media_muted: bool,
    /// Corvene `1329-font-diff`: the specimen's own sample text (empty: the
    /// chosen preset).
    #[serde(default)]
    pub font_specimen_text: String,
    /// Corvene `1329-font-diff`: specimen size, points.
    #[serde(default = "default_font_specimen_size")]
    pub font_specimen_size: f32,
    /// Corvene `1329-font-diff`: glyph tile size, points.
    #[serde(default = "default_font_glyph_tile")]
    pub font_glyph_tile: f32,
    /// GHD `tabSize` for diffs (Appearance › Diff).
    #[serde(default = "default_tab_size")]
    pub tab_size: u32,
    /// Appearance › Syntax highlighting (Corvene addition, flag
    /// `105-tree-sitter-highlighting`).
    #[serde(default)]
    pub syntax_highlighter: SyntaxHighlighter,
    /// Appearance › Formatting (`dateFormat`, date-fns pattern).
    #[serde(default = "default_date_format")]
    pub date_format: String,
    /// `timeFormat`
    #[serde(default = "default_time_format")]
    pub time_format: String,
    /// `numberFormat` key: `<thousands separator>|<decimal separator>`.
    #[serde(default = "default_number_format")]
    pub number_format: String,
    /// `preferAbsoluteDates`
    #[serde(default)]
    pub prefer_absolute_dates: bool,
    /// Git › Hooks: `enableGitHookEnv` / `cacheGitHookEnv`.
    #[serde(default)]
    pub enable_git_hook_env: bool,
    #[serde(default = "default_true")]
    pub cache_git_hook_env: bool,
    /// Integrations › custom editor / shell (`customEditor`, `useCustomEditor`…).
    #[serde(default)]
    pub custom_editor: Option<CustomIntegration>,
    #[serde(default)]
    pub use_custom_editor: bool,
    /// Corvene (`523-custom-editor-list`): the custom editors after
    /// `custom_editor` (GHD's only one), and which of them all is used.
    #[serde(default)]
    pub more_custom_editors: Vec<CustomIntegration>,
    #[serde(default)]
    pub custom_editor_index: usize,
    #[serde(default)]
    pub custom_shell: Option<CustomIntegration>,
    #[serde(default)]
    pub use_custom_shell: bool,
    /// Flag `266-collapsible-repository-groups`: the repository list groups
    /// the user collapsed (`Group::key` in `corvene-ui`'s repository list).
    #[serde(default)]
    pub collapsed_repository_groups: Vec<String>,
    /// Corvene `292-clone-owner-picker`: the owner the clone lists show per
    /// account (`<endpoint>|<login>` → owner login); missing = all owners.
    pub clone_owner_filter: HashMap<String, String>,
    /// Corvene `525-account-commit-email`: the email a repository of an
    /// account (`<endpoint>|<login>`) gets as its local `user.email` when it
    /// is cloned or added without one.
    #[serde(default)]
    pub account_commit_emails: HashMap<String, String>,
}

/// GHD `ICustomIntegration`: an executable (or macOS app bundle) plus its
/// arguments; `%TARGET_PATH%` in the arguments is replaced by the repository path.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomIntegration {
    pub path: String,
    pub arguments: String,
    /// Set when `path` is a `.app` bundle (launched through `open -b`).
    #[serde(default)]
    pub bundle_id: Option<String>,
    /// The custom editor's name in "Open in …" labels (flag
    /// `custom-editor-name`; not in GHD). Empty: "Custom Editor".
    #[serde(default)]
    pub name: String,
}

impl CustomIntegration {
    /// The custom editor's name in menus: its own (`508-custom-editor-name`,
    /// `523-custom-editor-list`), else "Custom Editor" (numbered after the
    /// first in a list).
    pub fn display_name(&self, index: usize, named: bool) -> String {
        let name = self.name.trim();
        if named && !name.is_empty() {
            name.to_string()
        } else if index == 0 {
            "Custom Editor".to_string()
        } else {
            format!("Custom Editor {}", index + 1)
        }
    }
}

impl Settings {
    /// Corvene (`523-custom-editor-list`): every custom editor, GHD's
    /// `custom_editor` first.
    pub fn custom_editors(&self) -> Vec<CustomIntegration> {
        self.custom_editor
            .iter()
            .chain(&self.more_custom_editors)
            .cloned()
            .collect()
    }

    /// Store `editors` as the custom editors (the first in
    /// `custom_editor`), keeping the chosen one in range.
    pub fn set_custom_editors(&mut self, mut editors: Vec<CustomIntegration>) {
        self.custom_editor = (!editors.is_empty()).then(|| editors.remove(0));
        self.more_custom_editors = editors;
        let count = self.custom_editors().len();
        self.custom_editor_index = self.custom_editor_index.min(count.saturating_sub(1));
    }

    /// The custom editor that "Open in …" uses while `use_custom_editor`
    /// is set: GHD's one, or with `list` (`523-custom-editor-list`) the
    /// chosen one of the list.
    pub fn chosen_custom_editor(&self, list: bool) -> Option<&CustomIntegration> {
        if list && self.custom_editor_index > 0 {
            self.more_custom_editors
                .get(self.custom_editor_index - 1)
                .or(self.custom_editor.as_ref())
        } else {
            self.custom_editor.as_ref()
        }
    }
}

fn default_date_format() -> String {
    DEFAULT_DATE_FORMAT.to_string()
}
fn default_time_format() -> String {
    default_time_format_for(locale_country().as_deref())
}
fn default_number_format() -> String {
    default_number_format_for(locale_country().as_deref())
}

/// The OS locale's country, read once.
fn locale_country() -> Option<String> {
    static COUNTRY: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    COUNTRY
        .get_or_init(corvene_platform::locale::country_code)
        .clone()
}

/// GHD `defaultDateFormat`: the same everywhere.
pub const DEFAULT_DATE_FORMAT: &str = "MMM d, yyyy";
/// GHD's en-US defaults (used when the locale is unknown).
pub const DEFAULT_TIME_FORMAT: &str = "h:mm aaa";
pub const DEFAULT_NUMBER_FORMAT: &str = ",|.";

/// GHD `twelveHourCountries`.
const TWELVE_HOUR_COUNTRIES: &[&str] = &[
    "GB", "IE", "US", "CA", "AU", "NZ", "ZA", "IN", "PK", "BD", "PH", "MX", "CO",
];
/// GHD `decimalPointCountries`.
const DECIMAL_POINT_COUNTRIES: &[&str] = &[
    "AU", "BS", "BD", "BW", "AI", "AG", "BB", "BM", "VG", "KY", "DM", "GD", "JM", "MS", "KN", "LC",
    "VC", "TT", "TC", "GY", "BZ", "KH", "CA", "CN", "CY", "DO", "EG", "SV", "ET", "GH", "GT", "HN",
    "HK", "IN", "IE", "IL", "JP", "JO", "KE", "KP", "KR", "LY", "LI", "MO", "MY", "MV", "MT", "MX",
    "MM", "NA", "NP", "NZ", "NI", "NG", "PK", "PA", "PH", "RW", "QA", "SA", "SG", "SO", "LK", "CH",
    "SY", "TW", "TZ", "TH", "UG", "AE", "GB", "US",
];
const COMMA_GROUPING_COUNTRIES: &[&str] = &["US", "GB", "TH"];
const SPACE_GROUPING_COUNTRIES: &[&str] = &["CA", "DK", "FI", "SE", "FR", "DE"];
const DOT_GROUPING_COUNTRIES: &[&str] = &["IT", "NO", "ES"];

/// GHD `defaultTimeFormat`: 12-hour in the countries that use it (and when
/// the locale is unknown), 24-hour elsewhere.
pub fn default_time_format_for(country: Option<&str>) -> String {
    match country {
        None => DEFAULT_TIME_FORMAT.to_string(),
        Some(c) if TWELVE_HOUR_COUNTRIES.contains(&c) => DEFAULT_TIME_FORMAT.to_string(),
        Some(_) => "HH:mm".to_string(),
    }
}

/// GHD `defaultNumberFormat` (`thousands|decimal`).
pub fn default_number_format_for(country: Option<&str>) -> String {
    let Some(c) = country else {
        return DEFAULT_NUMBER_FORMAT.to_string();
    };
    let decimal = if DECIMAL_POINT_COUNTRIES.contains(&c) {
        "."
    } else {
        ","
    };
    let thousands = if COMMA_GROUPING_COUNTRIES.contains(&c) {
        ","
    } else if SPACE_GROUPING_COUNTRIES.contains(&c) {
        " "
    } else if DOT_GROUPING_COUNTRIES.contains(&c) {
        "."
    } else {
        ""
    };
    format!("{thousands}|{decimal}")
}

fn default_file_list_width() -> f32 {
    250.0
}

fn default_file_icon_theme() -> String {
    crate::file_icons::NONE.to_string()
}

fn default_tab_size() -> u32 {
    TAB_SIZE_DEFAULT
}

/// GHD `tabSizeDefault`.
pub const TAB_SIZE_DEFAULT: u32 = 4;

fn default_font_specimen_size() -> f32 {
    48.0
}

fn default_font_glyph_tile() -> f32 {
    72.0
}

fn default_true() -> bool {
    true
}

fn default_zoom() -> f32 {
    1.0
}

/// Corvene (`354-pull-request-event-notifications`): the pull request
/// events Settings › Notifications turns on, each on by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PullRequestEventNotifications {
    /// A review was asked of you or one of your teams.
    pub review_requested: bool,
    /// Your pull request was approved or got changes requested.
    pub reviews: bool,
    /// Someone else merged your pull request.
    pub merged: bool,
    /// You or one of your teams was mentioned in a pull request.
    pub mentions: bool,
}

impl Default for PullRequestEventNotifications {
    fn default() -> Self {
        Self {
            review_requested: true,
            reviews: true,
            merged: true,
            mentions: true,
        }
    }
}

impl PullRequestEventNotifications {
    pub fn any(&self) -> bool {
        self.review_requested || self.reviews || self.merged || self.mentions
    }
}

/// Corvene (`354`): where a click on a pull request event notification
/// goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotificationClickAction {
    /// The repository in Corvene when it is listed (its review view with
    /// `348-pull-request-review`), else the pull request on GitHub.
    #[default]
    OpenInCorvene,
    /// Always the pull request on GitHub.
    OpenOnGitHub,
}

/// Corvene `1324-document-diff`: the diff header's Rendered / Text / Pages
/// switch of a document file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DocumentDiffView {
    /// The blocks laid out as in a text editor.
    #[default]
    Rendered,
    /// A line diff of the extracted text (git's diff of an RTF file).
    Text,
    /// `1325-document-pages`: the pages LibreOffice draws.
    Pages,
}

/// Corvene `1331-notebook-diff`: the diff header's Notebook / Script /
/// JSON switch of a notebook.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotebookDiffView {
    /// Cells, rendered.
    #[default]
    Notebook,
    /// A line diff of the notebook written as a py:percent script.
    Script,
    /// Git's diff of the JSON (stageable line by line).
    Json,
}

/// Corvene `1332-latex-diff`: the diff header's Rendered / Text / Pages
/// switch of a LaTeX or BibTeX file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LatexDiffView {
    #[default]
    Rendered,
    /// Git's own diff.
    Text,
    /// Both versions compiled to PDF and compared page by page.
    Pages,
}

/// Corvene `1334-markdown-preview-diff`: the diff header's Source /
/// Preview switch of a Markdown file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarkdownDiffView {
    /// Git's own diff (GHD's only view).
    #[default]
    Source,
    /// Both versions rendered, the changed blocks marked.
    Preview,
}

/// Corvene (`1314-code-owners`): a file row's owners as text or an icon.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodeOwnersDisplay {
    /// The first owner and "+N" for more.
    #[default]
    Label,
    /// A shield icon; the owners are in its tooltip.
    Icon,
}

/// GHD `UncommittedChangesStrategy`
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum UncommittedChangesStrategy {
    #[default]
    AskForConfirmation,
    StashOnCurrentBranch,
    MoveToNewBranch,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeSetting::System,
            design_style: corvene_models::DesignStyle::default(),
            sidebar_width: 250.0,
            branch_dropdown_width: None,
            worktree_dropdown_width: None,
            save_crash_reports: false,
            last_launched_at: None,
            last_successful_update_check: None,
            window_zoom_factor: 1.0,
            tutorial_install_editor_skipped: false,
            tutorial_pull_request_step_complete: false,
            tutorial_paused: false,
            commit_summary_width: 250.0,
            commit_description_height: None,
            clone_dir: None,
            welcome_completed: false,
            confirm_discard_changes: true,
            hide_embedded_repository_note: false,
            confirm_checkout_commit: true,
            confirm_undo_commit: true,
            history_first_parent: false,
            history_all_branches: false,
            uncommitted_changes_strategy: UncommittedChangesStrategy::default(),
            confirm_discard_stash: true,
            confirm_worktree_removal: true,
            confirm_commit_message_override: true,
            ask_to_move_to_applications_folder: true,
            confirm_force_push: true,
            external_editor: None,
            shell: None,
            notifications_enabled: true,
            confirm_repository_removal: true,
            confirm_discard_changes_permanently: true,
            confirm_commit_filtered_changes: true,
            show_commit_length_warning: true,
            commit_spellcheck_enabled: true,
            repository_indicators_enabled: true,
            menu_bar_repositories: Vec::new(),
            pull_request_event_notifications: PullRequestEventNotifications::default(),
            pull_request_notification_click: NotificationClickAction::default(),
            code_owners_display: CodeOwnersDisplay::default(),
            use_external_credential_helper: false,
            underline_links: true,
            show_diff_check_marks: true,
            hide_whitespace_in_changes_diff: false,
            hide_whitespace_in_history_diff: false,
            hide_whitespace_in_pull_request_diff: false,
            pull_request_file_list_width: 250.0,
            show_side_by_side_diff: false,
            diff_wrap_lines: true,
            diff_mark_moved_lines: true,
            diff_mark_stylistic_changes: true,
            csv_table_diff: true,
            pdf_text_diff: false,
            svg_text_diff: false,
            archive_show_unchanged: false,
            archive_show_metadata_only: false,
            archive_stacked: false,
            archive_tree_width: 250.0,
            font_source_text_diff: false,
            document_diff_view: DocumentDiffView::Rendered,
            notebook_diff_view: NotebookDiffView::Notebook,
            notebook_filters: crate::notebook_diff::NotebookFilters::default(),
            latex_diff_view: LatexDiffView::Rendered,
            markdown_diff_view: MarkdownDiffView::Source,
            file_list_tree: false,
            file_icon_theme: default_file_icon_theme(),
            stacked_diff_changes: false,
            stacked_diff_history: false,
            image_diff_type: corvene_models::ImageDiffType::TwoUp,
            font_diff_view: corvene_models::FontDiffView::Overview,
            video_diff_mode: corvene_models::VideoDiffMode::TwoUp,
            audio_diff_mode: corvene_models::AudioDiffMode::Stacked,
            media_volume: 1.,
            media_muted: false,
            font_specimen_text: String::new(),
            font_specimen_size: default_font_specimen_size(),
            font_glyph_tile: default_font_glyph_tile(),
            tab_size: TAB_SIZE_DEFAULT,
            syntax_highlighter: SyntaxHighlighter::GitHubDesktop,
            date_format: default_date_format(),
            time_format: default_time_format(),
            number_format: default_number_format(),
            prefer_absolute_dates: false,
            enable_git_hook_env: false,
            cache_git_hook_env: true,
            custom_editor: None,
            use_custom_editor: false,
            more_custom_editors: Vec::new(),
            custom_editor_index: 0,
            custom_shell: None,
            use_custom_shell: false,
            collapsed_repository_groups: Vec::new(),
            clone_owner_filter: HashMap::new(),
            account_commit_emails: HashMap::new(),
        }
    }
}

/// The stored `settings` record as [`Settings`]: a field whose value cannot
/// be read (`"welcome_completed": "a"`) falls back to its default and the
/// other fields are kept, as GHD's per-key `getBoolean` / `getNumber` reads
/// do; a record that is not an object gives the defaults.
pub fn settings_from_value(value: serde_json::Value) -> Settings {
    if let Ok(settings) = serde_json::from_value::<Settings>(value.clone()) {
        return settings;
    }
    let serde_json::Value::Object(stored) = value else {
        warn!("ignoring unreadable settings");
        return Settings::default();
    };
    // every field has a default, so each key can be tried on its own
    let readable: serde_json::Map<String, serde_json::Value> = stored
        .into_iter()
        .filter(|(key, value)| {
            let single = serde_json::Value::Object(serde_json::Map::from_iter([(
                key.clone(),
                value.clone(),
            )]));
            let ok = serde_json::from_value::<Settings>(single).is_ok();
            if !ok {
                warn!(key, "ignoring an unreadable setting");
            }
            ok
        })
        .collect();
    serde_json::from_value(serde_json::Value::Object(readable)).unwrap_or_default()
}

/// GHD `markWelcomeFlowComplete` (`lib/welcome.ts`): record in the stored
/// settings that the welcome flow was shown (`Settings::welcome_completed`,
/// GHD's `has-shown-welcome-flow`). A failure is logged.
pub fn mark_welcome_flow_complete(store: &Store) {
    let saved = store.settings().and_then(|mut settings| {
        settings.welcome_completed = true;
        store.save_settings(&settings)
    });
    if let Err(err) = saved {
        warn!(%err, "could not record that the welcome flow was shown");
    }
}

/// GHD `getMigratedGHEAccounts`: `accounts` with every `*.ghe.com`
/// endpoint whose host does not start with `api.` moved to
/// `https://api.<host>/` (`getEnterpriseAPIURL`), or `None` when none had
/// to move.
pub fn migrated_ghe_accounts(accounts: &[Account]) -> Option<Vec<Account>> {
    let mut migrated = false;
    let accounts = accounts
        .iter()
        .map(|account| {
            let mut account = account.clone();
            if let Some(endpoint) = migrated_ghe_endpoint(&account.endpoint) {
                account.endpoint = endpoint;
                migrated = true;
            }
            account
        })
        .collect();
    migrated.then_some(accounts)
}

/// `endpoint` moved as in [`migrated_ghe_accounts`], or `None` when it
/// stays.
fn migrated_ghe_endpoint(endpoint: &str) -> Option<String> {
    // `URL.hostname` is lowercased
    let host = corvene_github::endpoint::endpoint_host(endpoint)?.to_ascii_lowercase();
    (corvene_github::endpoint::is_ghe(endpoint) && !host.starts_with("api."))
        .then(|| corvene_github::endpoint::get_enterprise_api_url(endpoint))
}

/// `repositories` whose GitHub repository (or its parent) is on a
/// `*.ghe.com` `/api/v3` endpoint, moved as in [`migrated_ghe_accounts`];
/// `None` when none had to move.
pub fn migrated_ghe_repositories(repositories: &[Repository]) -> Option<Vec<Repository>> {
    fn migrate(gh: &mut corvene_models::GitHubRepository) -> bool {
        let mut migrated = false;
        if let Some(endpoint) = migrated_ghe_endpoint(&gh.endpoint) {
            gh.endpoint = endpoint;
            migrated = true;
        }
        if let Some(parent) = gh.parent.as_mut() {
            migrated |= migrate(parent);
        }
        migrated
    }
    let mut migrated = false;
    let repositories = repositories
        .iter()
        .map(|repository| {
            let mut repository = repository.clone();
            if let Some(gh) = repository.github.as_mut() {
                migrated |= migrate(gh);
            }
            repository
        })
        .collect();
    migrated.then_some(repositories)
}

/// The user's switches for language extensions (`crate::extensions`),
/// kept apart from the extension folders so a reinstall keeps them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageExtensionsPrefs {
    /// extension id → its switches
    #[serde(default)]
    pub switches: HashMap<String, ExtensionSwitches>,
    /// file suffixes whose "no syntax highlighting" hint was dismissed
    #[serde(default)]
    pub dismissed_suffixes: BTreeSet<String>,
    /// when the offline extension index was last refreshed (unix seconds)
    #[serde(default)]
    pub index_refreshed_at: Option<u64>,
    /// grammar builds the user agreed to, as `repository@rev`
    #[serde(default)]
    pub build_consents: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionSwitches {
    pub enabled: bool,
    /// the default for the extension's languages
    pub prefer_over_builtin: bool,
    /// language id → its own answer, where it differs from the default
    #[serde(default)]
    pub languages: HashMap<String, bool>,
}

/// Where `287-repository-list-backup` keeps the repository list's entries
/// this version cannot read, as they were stored.
pub const UNREADABLE_REPOSITORIES_KEY: &str = "repositories.unreadable";

/// [`StoreExt::repositories_keeping_unreadable`]'s result.
#[derive(Clone, Debug, Default)]
pub struct LoadedRepositories {
    pub repositories: Vec<Repository>,
    /// Entries of the stored list that were just moved aside.
    pub newly_unreadable: usize,
    /// Entries moved aside earlier that this version reads again.
    pub restored: usize,
    /// The key the stored list was copied to because it was not JSON.
    pub raw_backup: Option<String>,
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Corvene (`287-repository-list-backup`): when the app version differs from
/// the one that last opened `store`, copy its file to
/// `corvene-<previous version>.redb.bak` next to it (replacing older
/// backups), so an update that loses data can be rolled back. Returns the
/// backup's path when one was made.
pub fn backup_on_version_change(store: &Store, version: &str) -> Option<PathBuf> {
    const KEY: &str = "meta.app_version";
    let previous: Option<String> = store.get(KEY).ok().flatten();
    if previous.as_deref() == Some(version) {
        return None;
    }
    let backup = previous.and_then(|previous| {
        let dir = store.path().parent()?;
        let name = format!("corvene-{previous}.redb.bak");
        // the newest backup only
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let file = entry.file_name().to_string_lossy().into_owned();
                if file.starts_with("corvene-") && file.ends_with(".redb.bak") && file != name {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        let backup = dir.join(name);
        match store.copy_to(&backup) {
            Ok(()) => Some(backup),
            Err(err) => {
                warn!(%err, "could not back up the store");
                None
            }
        }
    });
    if let Err(err) = store.set(KEY, version) {
        warn!(%err, "could not record the app version");
    }
    backup
}

/// Keys are namespaced strings; values JSON. Add a key here, never ad hoc.
pub trait StoreExt {
    fn settings(&self) -> Result<Settings>;
    fn save_settings(&self, settings: &Settings) -> Result<()>;

    /// Language extensions: enabled / preferred per extension, dismissed
    /// hints (`crate::extensions`).
    fn language_extensions(&self) -> Result<LanguageExtensionsPrefs>;
    fn save_language_extensions(&self, prefs: &LanguageExtensionsPrefs) -> Result<()>;

    /// The flags' preset + overrides (`corvene_core::flags`).
    fn flags(&self) -> Result<crate::flags::FlagOverrides>;
    fn save_flags(&self, flags: &crate::flags::FlagOverrides) -> Result<()>;

    fn repositories(&self) -> Result<Vec<Repository>>;
    /// `287-repository-list-backup`: [`StoreExt::repositories`], with the
    /// entries that do not decode moved to `repositories.unreadable` (and a
    /// list that is not JSON copied to `repositories.unreadable.raw.<secs>`)
    /// rather than failing; earlier moved entries that decode now come back.
    fn repositories_keeping_unreadable(&self) -> Result<LoadedRepositories>;
    fn save_repositories(&self, repos: &[Repository]) -> Result<()>;
    fn next_repository_id(&self) -> Result<u64>;

    fn recent_repositories(&self) -> Result<Vec<u64>>;
    fn save_recent_repositories(&self, ids: &[u64]) -> Result<()>;
    /// `291-recent-worktrees`: (repository id, worktree path), most recent
    /// first.
    fn recent_worktrees(&self) -> Result<Vec<(u64, PathBuf)>>;
    fn save_recent_worktrees(&self, entries: &[(u64, PathBuf)]) -> Result<()>;
    /// The repository list's indicators by repository id (flag
    /// `271-persist-repository-indicators`).
    fn repository_indicators(
        &self,
    ) -> Result<std::collections::HashMap<u64, crate::remote::RepoIndicator>>;
    fn save_repository_indicators(
        &self,
        indicators: &std::collections::HashMap<u64, crate::remote::RepoIndicator>,
    ) -> Result<()>;
    fn selected_repository(&self) -> Result<Option<u64>>;
    fn save_selected_repository(&self, id: Option<u64>) -> Result<()>;
    /// `429-multiple-windows` / `430-repository-tabs`: the windows' selected
    /// repositories and tabs (`crate::workspace`), the main window first.
    fn workspaces(&self) -> Result<Vec<crate::workspace::SavedWorkspace>>;
    fn save_workspaces(&self, workspaces: &[crate::workspace::SavedWorkspace]) -> Result<()>;
    /// Unfinished commit messages by repository id (flag
    /// `766-persist-commit-drafts`).
    fn commit_drafts(&self) -> Result<std::collections::HashMap<u64, crate::drafts::CommitDraft>>;
    fn save_commit_drafts(
        &self,
        drafts: &std::collections::HashMap<u64, crate::drafts::CommitDraft>,
    ) -> Result<()>;
    /// Unticked changed files by repository id (flag
    /// `767-persist-file-selection`).
    fn excluded_files(&self) -> Result<std::collections::HashMap<u64, Vec<String>>>;
    fn save_excluded_files(
        &self,
        excluded: &std::collections::HashMap<u64, Vec<String>>,
    ) -> Result<()>;
    /// Collapsed folders of the changes tree by repository id (flag
    /// `1310-file-list-tree`).
    fn collapsed_folders(&self) -> Result<std::collections::HashMap<u64, Vec<String>>>;
    fn save_collapsed_folders(
        &self,
        folders: &std::collections::HashMap<u64, Vec<String>>,
    ) -> Result<()>;
    /// Changelists by repository id (flag `1313-changelists`).
    fn changelists(
        &self,
    ) -> Result<std::collections::HashMap<u64, crate::changelists::Changelists>>;
    fn save_changelists(
        &self,
        lists: &std::collections::HashMap<u64, crate::changelists::Changelists>,
    ) -> Result<()>;

    fn accounts(&self) -> Result<Vec<Account>>;
    fn save_accounts(&self, accounts: &[Account]) -> Result<()>;
    /// Generic git server logins (host → username); passwords live in the keychain.
    fn generic_logins(&self) -> Result<std::collections::HashMap<String, String>>;
    /// `528-proxy-credentials`: the username saved per proxy `host:port`.
    fn proxy_logins(&self) -> Result<std::collections::HashMap<String, String>>;
    fn save_proxy_logins(&self, logins: &std::collections::HashMap<String, String>) -> Result<()>;
    fn save_generic_logins(&self, logins: &std::collections::HashMap<String, String>)
    -> Result<()>;
    /// OAuth app client IDs entered per GitHub Enterprise host (host →
    /// client ID); client secrets live in the keychain.
    fn enterprise_oauth_apps(&self) -> Result<std::collections::HashMap<String, String>>;
    fn save_enterprise_oauth_apps(
        &self,
        apps: &std::collections::HashMap<String, String>,
    ) -> Result<()>;
}

impl StoreExt for Store {
    fn settings(&self) -> Result<Settings> {
        Ok(self
            .get::<serde_json::Value>("settings")?
            .map(settings_from_value)
            .unwrap_or_default())
    }

    fn save_settings(&self, settings: &Settings) -> Result<()> {
        self.set("settings", settings)
    }

    fn language_extensions(&self) -> Result<LanguageExtensionsPrefs> {
        Ok(self.get("language_extensions")?.unwrap_or_default())
    }

    fn save_language_extensions(&self, prefs: &LanguageExtensionsPrefs) -> Result<()> {
        self.set("language_extensions", prefs)
    }

    fn flags(&self) -> Result<crate::flags::FlagOverrides> {
        Ok(self.get("flags")?.unwrap_or_default())
    }

    fn save_flags(&self, flags: &crate::flags::FlagOverrides) -> Result<()> {
        self.set("flags", flags)
    }

    fn repositories(&self) -> Result<Vec<Repository>> {
        let repositories: Vec<Repository> = self.get("repositories")?.unwrap_or_default();
        // see the module doc
        let Some(migrated) = migrated_ghe_repositories(&repositories) else {
            return Ok(repositories);
        };
        if let Err(err) = self.save_repositories(&migrated) {
            warn!(%err, "could not save the migrated GitHub Enterprise repositories");
        }
        Ok(migrated)
    }

    fn repositories_keeping_unreadable(&self) -> Result<LoadedRepositories> {
        let mut raw_backup = None;
        let stored: Option<serde_json::Value> = match self.get("repositories") {
            Ok(value) => value,
            Err(corvene_store::StoreError::Json(err)) => {
                warn!(%err, "the repository list is not JSON; backing it up");
                if let Some(bytes) = self.get_raw("repositories")? {
                    let key = format!("repositories.unreadable.raw.{}", unix_seconds());
                    self.set_raw(&key, &bytes)?;
                    raw_backup = Some(key);
                }
                None
            }
            Err(err) => return Err(err),
        };
        let mut repositories = Vec::new();
        let mut unreadable = Vec::new();
        match stored {
            Some(serde_json::Value::Array(entries)) => {
                for entry in entries {
                    match serde_json::from_value::<Repository>(entry.clone()) {
                        Ok(repo) => repositories.push(repo),
                        Err(err) => {
                            warn!(%err, "a repository list entry could not be read");
                            unreadable.push(entry);
                        }
                    }
                }
            }
            Some(other) => unreadable.push(other),
            None => {}
        }
        let newly_unreadable = unreadable.len();
        let mut kept: Vec<serde_json::Value> = self
            .get(UNREADABLE_REPOSITORIES_KEY)
            .ok()
            .flatten()
            .unwrap_or_default();
        let kept_before = kept.len();
        let mut restored = 0;
        kept.retain(
            |entry| match serde_json::from_value::<Repository>(entry.clone()) {
                Ok(repo) => {
                    if !repositories
                        .iter()
                        .any(|r| r.id == repo.id || r.path == repo.path)
                    {
                        repositories.push(repo);
                        restored += 1;
                    }
                    false
                }
                Err(_) => true,
            },
        );
        let changed = newly_unreadable > 0 || raw_backup.is_some() || kept.len() != kept_before;
        kept.extend(unreadable);
        if changed {
            self.set(UNREADABLE_REPOSITORIES_KEY, &kept)?;
            self.save_repositories(&repositories)?;
        }
        if let Some(migrated) = migrated_ghe_repositories(&repositories) {
            if let Err(err) = self.save_repositories(&migrated) {
                warn!(%err, "could not save the migrated GitHub Enterprise repositories");
            }
            repositories = migrated;
        }
        Ok(LoadedRepositories {
            repositories,
            newly_unreadable,
            restored,
            raw_backup,
        })
    }

    fn save_repositories(&self, repos: &[Repository]) -> Result<()> {
        self.set("repositories", repos)?;
        // `425-cli-list-repositories`
        if crate::repository_list_file::enabled()
            && let Some(dir) = self.path().parent()
        {
            let selected = self.selected_repository().ok().flatten();
            crate::repository_list_file::write(dir, repos, selected);
        }
        Ok(())
    }

    /// Monotonic, never reused.
    fn next_repository_id(&self) -> Result<u64> {
        let next: u64 = self.get("repositories.next_id")?.unwrap_or(1);
        self.set("repositories.next_id", &(next + 1))?;
        Ok(next)
    }

    /// Most recent first (GHD keeps 3).
    fn recent_repositories(&self) -> Result<Vec<u64>> {
        Ok(self.get("repositories.recent")?.unwrap_or_default())
    }

    fn save_recent_repositories(&self, ids: &[u64]) -> Result<()> {
        self.set("repositories.recent", ids)
    }

    fn recent_worktrees(&self) -> Result<Vec<(u64, PathBuf)>> {
        Ok(self
            .get("repositories.recent_worktrees")?
            .unwrap_or_default())
    }

    fn save_recent_worktrees(&self, entries: &[(u64, PathBuf)]) -> Result<()> {
        self.set("repositories.recent_worktrees", entries)
    }

    fn repository_indicators(
        &self,
    ) -> Result<std::collections::HashMap<u64, crate::remote::RepoIndicator>> {
        Ok(self.get("repositories.indicators")?.unwrap_or_default())
    }

    fn save_repository_indicators(
        &self,
        indicators: &std::collections::HashMap<u64, crate::remote::RepoIndicator>,
    ) -> Result<()> {
        self.set("repositories.indicators", indicators)
    }

    fn selected_repository(&self) -> Result<Option<u64>> {
        self.get("ui.selected_repository")
    }

    fn workspaces(&self) -> Result<Vec<crate::workspace::SavedWorkspace>> {
        Ok(self.get("ui.workspaces")?.unwrap_or_default())
    }

    fn save_workspaces(&self, workspaces: &[crate::workspace::SavedWorkspace]) -> Result<()> {
        self.set("ui.workspaces", &workspaces)
    }

    fn commit_drafts(&self) -> Result<std::collections::HashMap<u64, crate::drafts::CommitDraft>> {
        Ok(self.get("changes.commit_drafts")?.unwrap_or_default())
    }

    fn save_commit_drafts(
        &self,
        drafts: &std::collections::HashMap<u64, crate::drafts::CommitDraft>,
    ) -> Result<()> {
        self.set("changes.commit_drafts", drafts)
    }

    fn excluded_files(&self) -> Result<std::collections::HashMap<u64, Vec<String>>> {
        Ok(self.get("changes.excluded_files")?.unwrap_or_default())
    }

    fn save_excluded_files(
        &self,
        excluded: &std::collections::HashMap<u64, Vec<String>>,
    ) -> Result<()> {
        self.set("changes.excluded_files", excluded)
    }

    fn collapsed_folders(&self) -> Result<std::collections::HashMap<u64, Vec<String>>> {
        Ok(self.get("changes.collapsed_folders")?.unwrap_or_default())
    }

    fn save_collapsed_folders(
        &self,
        folders: &std::collections::HashMap<u64, Vec<String>>,
    ) -> Result<()> {
        self.set("changes.collapsed_folders", folders)
    }

    fn changelists(
        &self,
    ) -> Result<std::collections::HashMap<u64, crate::changelists::Changelists>> {
        Ok(self.get("changes.changelists")?.unwrap_or_default())
    }

    fn save_changelists(
        &self,
        lists: &std::collections::HashMap<u64, crate::changelists::Changelists>,
    ) -> Result<()> {
        self.set("changes.changelists", lists)
    }

    fn save_selected_repository(&self, id: Option<u64>) -> Result<()> {
        self.set("ui.selected_repository", &id)?;
        // `425-cli-list-repositories`
        if crate::repository_list_file::enabled()
            && let Some(dir) = self.path().parent()
        {
            let repos: Vec<Repository> =
                self.get("repositories").ok().flatten().unwrap_or_default();
            crate::repository_list_file::write(dir, &repos, id);
        }
        Ok(())
    }

    /// `AccountsStore.loadFromStore`: the stored accounts, `*.ghe.com`
    /// ones migrated (and saved) as in [`migrated_ghe_accounts`].
    fn accounts(&self) -> Result<Vec<Account>> {
        let accounts: Vec<Account> = self.get("accounts")?.unwrap_or_default();
        let Some(migrated) = migrated_ghe_accounts(&accounts) else {
            return Ok(accounts);
        };
        if let Err(err) = self.save_accounts(&migrated) {
            warn!(%err, "could not save the migrated GitHub Enterprise accounts");
        }
        Ok(migrated)
    }

    fn generic_logins(&self) -> Result<std::collections::HashMap<String, String>> {
        Ok(self.get("generic_git_logins")?.unwrap_or_default())
    }

    fn proxy_logins(&self) -> Result<std::collections::HashMap<String, String>> {
        Ok(self.get("proxy_logins")?.unwrap_or_default())
    }

    fn save_proxy_logins(&self, logins: &std::collections::HashMap<String, String>) -> Result<()> {
        self.set("proxy_logins", logins)
    }

    fn save_generic_logins(
        &self,
        logins: &std::collections::HashMap<String, String>,
    ) -> Result<()> {
        self.set("generic_git_logins", logins)
    }

    fn save_accounts(&self, accounts: &[Account]) -> Result<()> {
        self.set("accounts", accounts)
    }

    fn enterprise_oauth_apps(&self) -> Result<std::collections::HashMap<String, String>> {
        Ok(self.get("enterprise_oauth_apps")?.unwrap_or_default())
    }

    fn save_enterprise_oauth_apps(
        &self,
        apps: &std::collections::HashMap<String, String>,
    ) -> Result<()> {
        self.set("enterprise_oauth_apps", apps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_editor_list_keeps_ghds_entry_first() {
        let editor = |name: &str| CustomIntegration {
            path: format!("/bin/{name}"),
            name: name.to_string(),
            ..CustomIntegration::default()
        };
        // an old store: one custom editor, no list fields
        let mut settings: Settings = serde_json::from_value(serde_json::json!({
            "custom_editor": {"path": "/bin/a", "arguments": "", "name": "a"},
            "use_custom_editor": true,
        }))
        .unwrap();
        assert_eq!(settings.custom_editors(), vec![editor("a")]);
        settings.set_custom_editors(vec![editor("a"), editor("b"), editor("c")]);
        settings.custom_editor_index = 2;
        assert_eq!(settings.chosen_custom_editor(true), Some(&editor("c")));
        assert_eq!(settings.chosen_custom_editor(false), Some(&editor("a")));
        settings.set_custom_editors(vec![editor("b")]);
        assert_eq!(settings.custom_editor_index, 0);
        assert_eq!(settings.chosen_custom_editor(true), Some(&editor("b")));
        assert_eq!(editor("").display_name(1, true), "Custom Editor 2");
        assert_eq!(editor("vi").display_name(1, false), "Custom Editor 2");
        assert_eq!(editor("vi").display_name(0, true), "vi");
    }

    #[test]
    fn flags_round_trip() {
        use crate::flags::{FlagOverrides, Preset, Value};
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_in(dir.path()).unwrap();
        assert_eq!(store.flags().unwrap(), FlagOverrides::default());
        let mut flags = FlagOverrides {
            preset: Preset::Familiar,
            ..Default::default()
        };
        flags
            .overrides
            .insert("commit-templates".into(), Value::Bool(false));
        store.save_flags(&flags).unwrap();
        assert_eq!(store.flags().unwrap(), flags);
    }

    #[test]
    fn unreadable_settings_fall_back_one_by_one() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_in(dir.path()).unwrap();
        store
            .set(
                "settings",
                &serde_json::json!({ "welcome_completed": "a", "sidebar_width": 300.0 }),
            )
            .unwrap();
        let settings = store.settings().unwrap();
        assert!(!settings.welcome_completed);
        assert_eq!(settings.sidebar_width, 300.0);
        mark_welcome_flow_complete(&store);
        assert!(store.settings().unwrap().welcome_completed);
        assert_eq!(store.settings().unwrap().sidebar_width, 300.0);
    }

    #[test]
    fn ghe_com_endpoints_move_to_the_api_subdomain() {
        let account = Account {
            endpoint: "https://whatever.ghe.com/api/v3".into(),
            id: 1,
            login: "joan".into(),
            name: None,
            avatar_url: None,
            emails: Vec::new(),
            scopes: Vec::new(),
            plan: None,
            private_primary_email: false,
        };
        let migrated = migrated_ghe_accounts(std::slice::from_ref(&account)).unwrap();
        assert_eq!(migrated[0].endpoint, "https://api.whatever.ghe.com/");
        // the keychain key and remote matching keep the web host
        assert_eq!(migrated[0].host(), account.host());
        assert!(migrated_ghe_accounts(&migrated).is_none());
        let mut repository = Repository::new(1, "/r");
        repository.github = corvene_models::github_from_remote(
            "https://whatever.ghe.com/o/n.git",
            &["whatever.ghe.com".into()],
        );
        assert_eq!(
            repository.github.as_ref().map(|gh| gh.endpoint.as_str()),
            Some("https://api.whatever.ghe.com/")
        );
        if let Some(gh) = repository.github.as_mut() {
            gh.endpoint = "https://whatever.ghe.com/api/v3".into();
        }
        let moved = migrated_ghe_repositories(&[repository]).unwrap();
        assert_eq!(
            moved[0].github.as_ref().map(|gh| gh.endpoint.as_str()),
            Some("https://api.whatever.ghe.com/")
        );
    }

    #[test]
    fn settings_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_in(dir.path()).unwrap();
        assert_eq!(store.settings().unwrap().sidebar_width, 250.0);
        let s = Settings {
            theme: ThemeSetting::Dark,
            sidebar_width: 300.0,
            welcome_completed: true,
            confirm_discard_changes: false,
            confirm_force_push: false,
            external_editor: Some("Zed".into()),
            ..Settings::default()
        };
        store.save_settings(&s).unwrap();
        let back = store.settings().unwrap();
        assert_eq!(back.theme, ThemeSetting::Dark);
        assert_eq!(back.sidebar_width, 300.0);
        assert_eq!(back.external_editor.as_deref(), Some("Zed"));
        assert!(back.show_diff_check_marks && back.repository_indicators_enabled);
    }

    #[test]
    fn syntax_highlighter_spelling() {
        let json = serde_json::to_value(SyntaxHighlighter::GitHubDesktop).unwrap();
        assert_eq!(json, "github-desktop");
        let s = Settings {
            syntax_highlighter: SyntaxHighlighter::TreeSitterFallback,
            ..Settings::default()
        };
        let text = serde_json::to_string(&s).unwrap();
        assert!(
            text.contains(r#""syntax_highlighter":"tree-sitter-fallback""#),
            "{text}"
        );
        // settings saved before the field existed read as GitHub Desktop
        let mut old = serde_json::to_value(Settings::default()).unwrap();
        old.as_object_mut().unwrap().remove("syntax_highlighter");
        let back: Settings = serde_json::from_value(old).unwrap();
        assert_eq!(back.syntax_highlighter, SyntaxHighlighter::GitHubDesktop);
    }

    #[test]
    fn unreadable_repositories_are_kept_aside_and_come_back() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_in(dir.path()).unwrap();
        let good = Repository::new(1, "/tmp/a");
        let mut bad = serde_json::to_value(Repository::new(2, "/tmp/b")).unwrap();
        bad["pinned"] = serde_json::json!("not a bool");
        store
            .set(
                "repositories",
                &serde_json::json!([serde_json::to_value(&good).unwrap(), bad]),
            )
            .unwrap();
        assert!(store.repositories().is_err());
        let loaded = store.repositories_keeping_unreadable().unwrap();
        assert_eq!(loaded.repositories, vec![good.clone()]);
        assert_eq!(loaded.newly_unreadable, 1);
        // the readable list was saved; the entry waits under its own key
        assert_eq!(store.repositories().unwrap(), vec![good.clone()]);
        let kept: Vec<serde_json::Value> = store.get(UNREADABLE_REPOSITORIES_KEY).unwrap().unwrap();
        assert_eq!(kept.len(), 1);
        // a second launch moves nothing new
        let again = store.repositories_keeping_unreadable().unwrap();
        assert_eq!((again.newly_unreadable, again.restored), (0, 0));
        // a version that reads it gets it back
        let mut readable = kept[0].clone();
        readable["pinned"] = serde_json::json!(true);
        store
            .set(UNREADABLE_REPOSITORIES_KEY, &vec![readable])
            .unwrap();
        let back = store.repositories_keeping_unreadable().unwrap();
        assert_eq!(back.restored, 1);
        assert_eq!(back.repositories.len(), 2);
        let kept: Vec<serde_json::Value> = store.get(UNREADABLE_REPOSITORIES_KEY).unwrap().unwrap();
        assert!(kept.is_empty());
        // a list that is not JSON is copied as it was
        store.set_raw("repositories", b"{oops").unwrap();
        let loaded = store.repositories_keeping_unreadable().unwrap();
        assert!(loaded.repositories.is_empty());
        let key = loaded.raw_backup.unwrap();
        assert_eq!(store.get_raw(&key).unwrap().unwrap(), b"{oops");
        assert_eq!(store.repositories().unwrap(), Vec::new());
    }

    #[test]
    fn the_store_is_backed_up_when_the_version_changes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_in(dir.path()).unwrap();
        assert_eq!(backup_on_version_change(&store, "1.0.0"), None);
        assert_eq!(backup_on_version_change(&store, "1.0.0"), None);
        let backup = backup_on_version_change(&store, "1.1.0").unwrap();
        assert_eq!(backup, dir.path().join("corvene-1.0.0.redb.bak"));
        assert!(backup.exists());
        let newer = backup_on_version_change(&store, "1.2.0").unwrap();
        assert!(newer.exists() && !backup.exists());
    }

    #[test]
    fn repositories_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_in(dir.path()).unwrap();
        assert!(store.repositories().unwrap().is_empty());
        assert_eq!(store.next_repository_id().unwrap(), 1);
        assert_eq!(store.next_repository_id().unwrap(), 2);
        let repos = vec![Repository::new(1, "/tmp/a"), Repository::new(2, "/tmp/b")];
        store.save_repositories(&repos).unwrap();
        assert_eq!(store.repositories().unwrap(), repos);
        store.save_recent_repositories(&[2, 1]).unwrap();
        assert_eq!(store.recent_repositories().unwrap(), vec![2, 1]);
        store.save_selected_repository(Some(2)).unwrap();
        assert_eq!(store.selected_repository().unwrap(), Some(2));
    }

    #[test]
    fn repository_indicators_round_trip() {
        use crate::remote::RepoIndicator;
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_in(dir.path()).unwrap();
        assert!(store.repository_indicators().unwrap().is_empty());
        let mut indicators = std::collections::HashMap::new();
        indicators.insert(
            7,
            RepoIndicator {
                ahead_behind: Some(corvene_models::AheadBehind {
                    ahead: 2,
                    behind: 1,
                }),
                changed_files: 3,
                branch: Some("main".into()),
                has_stash: true,
            },
        );
        store.save_repository_indicators(&indicators).unwrap();
        assert_eq!(store.repository_indicators().unwrap(), indicators);
    }

    #[test]
    fn formatting_defaults_follow_the_country() {
        assert_eq!(default_time_format_for(None), "h:mm aaa");
        assert_eq!(default_time_format_for(Some("US")), "h:mm aaa");
        assert_eq!(default_time_format_for(Some("DE")), "HH:mm");
        assert_eq!(default_number_format_for(None), ",|.");
        assert_eq!(default_number_format_for(Some("US")), ",|.");
        assert_eq!(default_number_format_for(Some("DE")), " |,");
        assert_eq!(default_number_format_for(Some("IT")), ".|,");
        assert_eq!(default_number_format_for(Some("BR")), "|,");
    }
}
