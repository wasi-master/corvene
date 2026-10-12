//! The flag table. One entry per switchable deviation from GitHub Desktop or
//! Corvene-only extra; `.docs/deviations.md` names each flag next to
//! the behaviour it controls and `.docs/flags.md` is generated from
//! this file.
//!
//! Rules: the id's hundreds digit is the category block (a full block
//! continues in its category's overflow block: 1100 Repository, 1200 History
//! & branches, 1300 Changes & diffs), ids are never
//! reused (move a deleted flag's id and slug to [`RETIRED`]), "on" means the
//! Corvene deviation is active, and every preset gets an explicit value.
//! `max` is `corvene` plus the extras most people would want; an extra that
//! is a matter of taste or suits only some workflows (colour-blind colours,
//! compact rows, another list order, an extra confirmation) keeps the
//! Corvene value there too.
//!
//! Every entry also sets `nature`: [`Nature::BugFix`] when GitHub Desktop's
//! behaviour is plainly wrong and nearly everyone wants the fix,
//! [`Nature::Feature`] for a new capability, option or look (when in doubt,
//! Feature). It is an attribute, not a category: the flag keeps its numbered
//! block. The Flags dialog hides bug fixes unless "Show bug fixes" is ticked;
//! presets and `CORVENE_FLAGS` apply to them all the same.

use super::{Availability, FlagDef, FlagId, Kind, Nature, SelectOption, Upstream, Value};

/// Emits `ids::NAME` constants and `REGISTRY` from one table, so a const and
/// its definition cannot drift apart.
macro_rules! registry {
    ($( $(#[$m:meta])* $name:ident = $id:literal $slug:literal { $($field:ident : $value:expr),* $(,)? } ),* $(,)?) => {
        /// `FlagId` constants, one per flag (`ids::COMMIT_TEMPLATES`).
        pub mod ids {
            use super::FlagId;
            $( $(#[$m])* pub const $name: FlagId = FlagId($id); )*
        }

        pub static REGISTRY: &[FlagDef] = &[
            $( FlagDef { id: FlagId($id), slug: $slug, $($field: $value,)* } ),*
        ];
    };
}

fn available() -> Availability {
    Availability::Available
}

/// A Linux-only deviation: on macOS GHD already behaves this way.
fn linux_only() -> Availability {
    if cfg!(target_os = "macos") {
        Availability::BuiltIn("GitHub Desktop has this on macOS too.")
    } else {
        Availability::Available
    }
}

/// A macOS-only extra (file bookmarks, Finder drags).
fn macos_only() -> Availability {
    if cfg!(target_os = "macos") {
        Availability::Available
    } else {
        Availability::BuiltIn("Only the macOS version has this.")
    }
}

/// The menu bar status item is built for macOS only so far (Linux and
/// Windows trays can follow).
fn macos_status_item() -> Availability {
    if cfg!(target_os = "macos") {
        Availability::Available
    } else {
        Availability::BuiltIn("The status item is only built for macOS so far.")
    }
}

/// The system's sleep and wake notifications reach the app (not on Android,
/// where WorkManager runs the background fetch).
fn wake_events() -> Availability {
    if cfg!(target_os = "android") {
        Availability::BuiltIn("Android does not tell the app when the device wakes.")
    } else {
        Availability::Available
    }
}

/// Android's lists of applications always show their launcher icons.
fn android_built_in() -> Availability {
    if cfg!(target_os = "android") {
        Availability::BuiltIn("Android always shows the applications' icons.")
    } else {
        Availability::Available
    }
}

/// The wgpu renderer's switches: macOS and Windows draw with another one.
fn wgpu_renderer_only() -> Availability {
    if cfg!(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "android"
    )) {
        Availability::Available
    } else {
        Availability::BuiltIn("Only the Linux and Android renderer has this.")
    }
}

fn product_name(s: &str) -> Result<(), &'static str> {
    let s = s.trim();
    if s.is_empty() {
        Err("Enter a name")
    } else if s.chars().count() > 40 {
        Err("At most 40 characters")
    } else if s.contains(['\n', '\r']) {
        Err("One line only")
    } else {
        Ok(())
    }
}

fn worktree_location(s: &str) -> Result<(), &'static str> {
    let s = s.trim();
    if s.is_empty() {
        Err("Enter a location, e.g. {clone-dir}")
    } else if s.contains(['\n', '\r']) {
        Err("One line only")
    } else if !s.starts_with(['/', '~', '{']) {
        Err("Start with /, ~ or {clone-dir}")
    } else {
        Ok(())
    }
}

/// `845-branch-name-prefix`: empty (off) or a ref-name-safe prefix.
fn branch_name_prefix(s: &str) -> Result<(), &'static str> {
    if s.chars().count() > 60 {
        Err("At most 60 characters")
    } else if s
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\\"'".contains(c))
    {
        Err("No spaces or ~ ^ : ? * [ \\ quotes")
    } else if s.starts_with(['.', '/', '-']) || s.contains("..") || s.contains("//") {
        Err("Not a valid start of a branch name")
    } else {
        Ok(())
    }
}

/// `873-branch-name-forbidden-chars`: the characters, written together.
fn forbidden_branch_chars(s: &str) -> Result<(), &'static str> {
    if s.chars().count() > 40 {
        Err("At most 40 characters")
    } else if s.contains(['\n', '\r']) {
        Err("One line only")
    } else {
        Ok(())
    }
}

/// `228-clone-default-account`: logins separated by commas or spaces.
fn account_logins(s: &str) -> Result<(), &'static str> {
    if s.chars().count() > 200 {
        Err("At most 200 characters")
    } else if s
        .chars()
        .any(|c| !(c.is_ascii_alphanumeric() || "-_, ".contains(c)))
    {
        Err("Logins separated by commas or spaces")
    } else {
        Ok(())
    }
}

fn hide_globs(s: &str) -> Result<(), &'static str> {
    if s.chars().count() > 1000 {
        Err("At most 1000 characters")
    } else {
        Ok(())
    }
}

fn app_name(s: &str) -> Result<(), &'static str> {
    if s.chars().count() > 500 {
        Err("At most 500 characters")
    } else if s.contains(['\n', '\r']) {
        Err("One line only")
    } else {
        Ok(())
    }
}

/// `516-git-executable`: empty, or an absolute or `~/` path on one line.
fn git_path(s: &str) -> Result<(), &'static str> {
    let s = s.trim();
    if s.contains(['\n', '\r']) {
        Err("One line only")
    } else if s.chars().count() > 500 {
        Err("At most 500 characters")
    } else if !s.is_empty() && !s.starts_with("~/") && !std::path::Path::new(s).is_absolute() {
        Err("Enter a full path, e.g. /usr/local/bin/git")
    } else {
        Ok(())
    }
}

/// A comma-separated list of GitHub logins (`229-hidden-clone-owners`).
fn owner_list(s: &str) -> Result<(), &'static str> {
    if s.contains(['\n', '\r']) {
        Err("One line only")
    } else if s.chars().count() > 500 {
        Err("At most 500 characters")
    } else if s
        .split(',')
        .map(str::trim)
        .any(|o| o.contains(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_')))
    {
        Err("Logins separated by commas")
    } else {
        Ok(())
    }
}

const ON: Value = Value::Bool(true);
const OFF: Value = Value::Bool(false);

const QUICK_VIEW_WIDTHS: &[SelectOption] = &[
    SelectOption {
        value: "fixed-400",
        label: "Fixed 400 px",
    },
    SelectOption {
        value: "min-400",
        label: "At least 400 px",
    },
];

const BACKGROUND_FETCHES: &[SelectOption] = &[
    SelectOption {
        value: "off",
        label: "Off",
    },
    SelectOption {
        value: "github",
        label: "GitHub repositories",
    },
    SelectOption {
        value: "any",
        label: "Any repository with a remote",
    },
];

const DESIGN_STYLES: &[SelectOption] = &[
    SelectOption {
        value: "auto",
        label: "The style chosen in Settings › Appearance",
    },
    SelectOption {
        value: "github-mobile",
        label: "GitHub Mobile",
    },
    SelectOption {
        value: "github-desktop",
        label: "GitHub Desktop",
    },
    SelectOption {
        value: "material",
        label: "Material",
    },
];

const SIGN_IN_FLOWS: &[SelectOption] = &[
    SelectOption {
        value: "auto",
        label: "Browser when this build has a client secret",
    },
    SelectOption {
        value: "device",
        label: "One-time code (device flow)",
    },
    SelectOption {
        value: "browser",
        label: "Browser (web flow)",
    },
];

const WIDTH_SAVES: &[SelectOption] = &[
    SelectOption {
        value: "drag-end",
        label: "When the drag ends",
    },
    SelectOption {
        value: "every-move",
        label: "On every pointer move",
    },
];

const CHANGES_SORT_ORDERS: &[SelectOption] = &[
    SelectOption {
        value: "path",
        label: "Path",
    },
    SelectOption {
        value: "status",
        label: "Status, then path",
    },
    SelectOption {
        value: "name",
        label: "File name",
    },
    SelectOption {
        value: "order-file",
        label: "diff.orderFile",
    },
];
const CHANGES_FILTER_MATCHES: &[SelectOption] = &[
    SelectOption {
        value: "fuzzy",
        label: "Fuzzy",
    },
    SelectOption {
        value: "substring",
        label: "Contains the text",
    },
    SelectOption {
        value: "suffix",
        label: "Ends with the text",
    },
    SelectOption {
        value: "exact",
        label: "Exact path or file name",
    },
];
const IMAGE_DIFF_BACKGROUNDS: &[SelectOption] = &[
    SelectOption {
        value: "light",
        label: "Light checkerboard",
    },
    SelectOption {
        value: "dark",
        label: "Dark checkerboard",
    },
    SelectOption {
        value: "theme",
        label: "Follow the app theme",
    },
];
const IMAGE_DIFF_ALIGNMENTS: &[SelectOption] = &[
    SelectOption {
        value: "centre",
        label: "Centred",
    },
    SelectOption {
        value: "top-left",
        label: "Top left corners together",
    },
];

const IGNORE_SUBMODULE_MODES: &[SelectOption] = &[
    SelectOption {
        value: "configured",
        label: "As configured (submodule.<name>.ignore)",
    },
    SelectOption {
        value: "dirty",
        label: "Changes inside submodules",
    },
    SelectOption {
        value: "all",
        label: "All submodule changes",
    },
];

registry! {
    // ---- 100 Appearance ----

    /// Settings › Appearance › High Contrast and the macOS "Increase contrast" switch.
    HIGH_CONTRAST_THEME = 101 "high-contrast-theme" {
        title: "High Contrast theme",
        summary: "Settings › Appearance offers a High Contrast theme (GitHub Desktop's dark tokens in \
                  Primer's high-contrast palette), and the System theme switches to it while macOS's \
                  \"Increase contrast\" is on.",
        ghd_behaviour: "Light, Dark and System only; \"Increase contrast\" is ignored.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(4544)],
        code: &["crates/corvene/src/main.rs", "crates/corvene-ui/src/dialogs/preferences.rs"],
    },

    /// Chromium's smooth-scroll curve for mouse-wheel ticks.
    SMOOTH_WHEEL_SCROLLING = 102 "smooth-wheel-scrolling" {
        title: "Smooth wheel scrolling",
        summary: "Mouse-wheel ticks animate with Chromium's smooth-scroll curve.",
        ghd_behaviour: "Jumps 40 px per tick (Electron only animates when NSScrollAnimationEnabled is set).",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[],
        code: &["crates/corvene-ui/src/scrollbar.rs"],
    },

    /// The name the Welcome flow and the tutorial README call the app.
    PRODUCT_NAME = 103 "product-name" {
        title: "Product name in Welcome, blank slate, tutorial, Move to Applications and submodule copy",
        summary: "The name the Welcome flow, the no-repositories blank slate, the tutorial \
                  README and the tutorial repository's GitHub description, the Move to \
                  Applications dialog and the submodule diff's \"Open this submodule\" card \
                  use for the app.",
        ghd_behaviour: "\"GitHub Desktop\".",
        nature: Nature::Feature,
        kind: Kind::Text { placeholder: "Corvene", validate: product_name },
        corvene: Value::text("Corvene"), ghd: Value::text("GitHub Desktop"),
        familiar: Value::text("Corvene"), max: Value::text("Corvene"),
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-ui/src/welcome.rs", "crates/corvene-ui/src/no_repositories.rs", "crates/corvene-ui/src/tutorial_panel.rs", "crates/corvene-core/src/tutorial.rs", "crates/corvene-ui/src/dialogs/move_to_applications_folder.rs", "crates/corvene-ui/src/diff_view.rs"],
    },

    /// A hovered selected list row keeps its selection colour.
    SELECTION_KEEPS_COLOUR_ON_HOVER = 104 "selection-keeps-colour-on-hover" {
        title: "Selected rows keep their colour under the pointer",
        summary: "Hovering a selected row in a list (changed files, commit files, branches, \
                  repositories, pull requests, stash files, branch pickers) keeps the selection \
                  colour instead of swapping in the hover colour.",
        ghd_behaviour: "`.list-item:hover` outranks `.list-item.selected` in specificity, so a \
                        selected row in an unfocused list shows the hover colour and looks \
                        unselected while the pointer is on it.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-ui/src/widgets.rs", "crates/corvene-ui/src/changes.rs", "crates/corvene-ui/src/branch_list.rs"],
    },

    /// Settings › Appearance › Syntax highlighting (tree-sitter).
    TREE_SITTER_HIGHLIGHTING = 105 "tree-sitter-highlighting" {
        title: "Tree-sitter syntax highlighting",
        summary: "Settings › Appearance offers Syntax highlighting: GitHub Desktop's highlighter, \
                  tree-sitter for the languages it does not highlight, or tree-sitter wherever \
                  there is a grammar. The grammars download as an optional component.",
        ghd_behaviour: "CodeMirror 5 modes only; files in other languages get no colours.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[
            Upstream::issue(22015),
            Upstream::issue(19038),
            Upstream::issue(21385),
            Upstream::issue(22663),
            Upstream::issue(21106),
            Upstream::issue(19311),
        ],
        code: &[
            "crates/corvene-highlight/src/treesitter/mod.rs",
            "crates/corvene-ui/src/dialogs/preferences.rs",
            "crates/corvene-ui/src/diff_view.rs",
            "crates/corvene-core/src/packs.rs",
        ],
    },

    /// Relative dates in weeks and calendar months.
    CALENDAR_RELATIVE_DATES = 106 "calendar-relative-dates" {
        title: "Relative dates in weeks and calendar months",
        summary: "Past a week, relative dates count weeks (\"4 weeks ago\") until two calendar \
                  months have passed, then calendar months and years.",
        ghd_behaviour: "Days until 30, then days ÷ 30 rounded as months: a commit on the 1st is \
                        \"last month\" on the 31st.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20830), Upstream::issue(21903)],
        code: &["crates/corvene-ui/src/relative_time.rs", "crates/corvene/src/main.rs"],
    },

    /// Taller .gitignore and squash-message text areas.
    TALLER_TEXT_AREAS = 107 "taller-text-areas" {
        title: "Taller .gitignore and squash message boxes",
        summary: "Repository Settings › Ignored Files' .gitignore box is 260 px tall and the \
                  Squash dialog's description box shows 12 lines.",
        ghd_behaviour: "130 px for .gitignore, 6 lines for the squash description.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(11715), Upstream::issue(13018)],
        code: &["crates/corvene-ui/src/dialogs/repository_settings.rs", "crates/corvene-ui/src/dialogs/mco_dialogs.rs"],
    },

    /// Blue / orange diff colours for red-green colour blindness.
    COLOUR_BLIND_DIFF = 108 "colour-blind-diff" {
        title: "Colour-blind friendly diff colours",
        summary: "Diffs show added lines in blue and deleted lines in orange (after Primer's \
                  protanopia / deuteranopia themes) in the Light and Dark themes.",
        ghd_behaviour: "Pale green and pale red, which are hard to tell apart with red-green \
                        colour blindness.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(6795)],
        code: &["crates/corvene-ui/src/theme/mod.rs", "crates/corvene/src/main.rs"],
    },

    /// A light title bar and toolbar in the Light theme.
    LIGHT_TOOLBAR = 109 "light-toolbar" {
        title: "Light title bar and toolbar in the Light theme",
        summary: "With the Light theme the title bar and the toolbar (repository, branch and \
                  push / pull buttons) use light greys and dark text.",
        ghd_behaviour: "The title bar and toolbar stay dark in every theme.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22123), Upstream::issue(22470)],
        code: &["crates/corvene-ui/src/theme/mod.rs", "crates/corvene-ui/src/title_bar.rs", "crates/corvene/src/main.rs"],
    },

    /// Typing hides hover highlights and tooltips until the pointer moves.
    KEYBOARD_HIDES_HOVER = 110 "keyboard-hides-hover" {
        title: "Typing hides hover highlights",
        summary: "A key press clears the hover highlight and tooltip under a resting pointer \
                  until the pointer moves, so keyboard navigation isn't shadowed by the row \
                  under the mouse.",
        ghd_behaviour: "`:hover` and open tooltips stay on the element the pointer last moved \
                        over while typing; an element that appears under the resting pointer \
                        isn't hovered until it moves.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[],
        code: &["crates/corvene-ui/src/widgets.rs", "vendor/gpui-pre/src/window.rs", "vendor/gpui-pre/src/elements/div.rs"],
    },

    /// Settings › Appearance › Language extensions…
    LANGUAGE_EXTENSIONS = 111 "language-extensions" {
        title: "Language extensions",
        summary: "Settings › Appearance offers Language extensions…: grammars from VS Code, Zed, \
                  Pulsar / Atom, Sublime Text and TextMate packages add highlighting for languages \
                  Corvene does not know, from a file, a folder, a URL, a GitHub repository, the \
                  Open VSX, Zed and Pulsar registries, or the editors installed on this machine. \
                  An extension's grammar wins over the built-in one for its files unless told not to.",
        ghd_behaviour: "A fixed set of CodeMirror 5 modes; no way to add a language.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22015)],
        code: &["crates/corvene-extensions", "crates/corvene-core/src/extensions.rs", "crates/corvene-highlight/src/user.rs", "crates/corvene-ui/src/dialogs/language_extensions.rs"],
    },

    /// Initials instead of the grey placeholder avatar.
    INITIALS_AVATARS = 112 "initials-avatars" {
        title: "Initials for authors without an avatar",
        summary: "A commit author whose avatar is not (yet) loaded, offline for example, shows \
                  their initials on a colour that stays the same for their e-mail, in the \
                  history list and the commit details, instead of the grey person symbol.",
        ghd_behaviour: "The same grey person symbol for every author without an avatar.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(7256)],
        code: &["crates/corvene-ui/src/widgets.rs", "crates/corvene-core/src/avatars.rs"],
    },

    /// Which design language the Android app draws with.
    DESIGN_STYLE = 113 "design-style" {
        title: "Design style (Android)",
        summary: "The Android app draws as GitHub for Android (Primer colours over Material \
                  components, Inter, Octicons, a bottom navigation bar), as GitHub Desktop (its \
                  tokens, toolbar and density), or as Material 3 with the device's dynamic colours. \
                  Auto follows Settings › Appearance › Design style; a value here pins it for a \
                  session (screenshot tests, the parity harness).",
        ghd_behaviour: "No Android app.",
        nature: Nature::Feature,
        kind: Kind::Select { options: DESIGN_STYLES },
        corvene: Value::text("auto"), ghd: Value::text("github-desktop"),
        familiar: Value::text("auto"), max: Value::text("auto"),
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-ffi/src/vm/settings.rs", "android/core/design"],
    },

    /// Draggable height of the commit description box.
    RESIZABLE_COMMIT_MESSAGE = 114 "resizable-commit-message" {
        title: "Resizable commit description",
        summary: "Dragging the top edge of the commit form up or down makes the description \
                  box taller or shorter (the file list above gives up the room); the height is \
                  kept for every repository.",
        ghd_behaviour: "The description box is 80 px tall and scrolls.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(1646)],
        code: &["crates/corvene-ui/src/changes.rs", "crates/corvene-core/src/persistence.rs"],
    },

    /// The repository sidebar on the right of the diff.
    SIDEBAR_ON_RIGHT = 115 "sidebar-on-right" {
        title: "Sidebar on the right",
        summary: "The Changes / History sidebar sits on the right of the diff instead of the \
                  left; it keeps its width and is resized from its left edge. The toolbar stays \
                  as it is.",
        ghd_behaviour: "The sidebar is always on the left.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(18645)],
        code: &["crates/corvene-ui/src/workspace.rs", "crates/corvene-ui/src/active_resizable.rs"],
    },

    /// The dialogs' multi-line boxes can be dragged taller.
    RESIZABLE_DIALOG_TEXT_AREAS = 116 "resizable-dialog-text-areas" {
        title: "Resizable text boxes in dialogs",
        summary: "The multi-line boxes of the squash commit message, the squash and merge \
                  description, the tag message and Repository Settings' .gitignore have a grip \
                  along their bottom edge that drags them taller or shorter (up to 70% of the \
                  window); the dialog grows with them and the height is kept while Corvene runs.",
        ghd_behaviour: "Those boxes have a fixed number of rows and scroll.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(17317)],
        code: &["crates/corvene-ui/src/widgets.rs"],
    },

    /// File type icons in the file lists, from editor icon themes.
    FILE_ICONS = 117 "file-icons" {
        title: "File icons",
        summary: "Rows of the changes list, a commit's files and a stash's files (and the \
                  folder rows of their trees) start with an icon for the file's type. \
                  Settings › Appearance › File icons picks the built-in Octicons or a file icon \
                  theme from a VS Code or Zed extension (Material Icon Theme, vscode-icons, \
                  Catppuccin…), installed, imported from an editor or found on Open VSX and \
                  Zed's registry in Language Extensions. SVG, PNG and icon-font themes work.",
        ghd_behaviour: "File rows show no file type icon.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-extensions/src/icon_theme.rs", "crates/corvene-core/src/file_icons.rs", "crates/corvene-ui/src/file_icons.rs"],
    },

    // ---- 200 Repository ----

    /// `commit.template` prefills the commit description.
    COMMIT_TEMPLATES = 201 "commit-templates" {
        title: "Commit message templates",
        summary: "The repository's commit.template (comment lines stripped) prefills the description \
                  while the summary is empty, and comes back after every commit.",
        ghd_behaviour: "Ignores commit.template.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(8698)],
        code: &["crates/corvene-ui/src/changes.rs", "crates/corvene-git/src/commit_template.rs"],
    },

    /// FSEvents-driven refresh.
    FS_WATCHER = 202 "fs-watcher" {
        title: "Filesystem watcher",
        summary: "Refreshes the repository when files under the worktree or .git change, not only on \
                  window focus and after Corvene's own actions.",
        ghd_behaviour: "Refreshes on window focus and after its own actions only.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(22600), Upstream::issue(2790)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-core/src/watcher.rs"],
    },

    /// The clone dialog's `owner/name` 404.
    CLONE_SHORTHAND_NOT_FOUND = 204 "clone-shorthand-not-found" {
        title: "Clone: unknown owner/name shows an error",
        summary: "When every account answers 404 for an owner/name shorthand in the clone dialog, \
                  \"We couldn't find that repository\" is shown instead of starting the clone.",
        ghd_behaviour: "Hands the bare alias to git, which fails after a moment.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[],
        code: &["crates/corvene-core/src/clone_info.rs"],
    },

    /// Add Local Repository checks the path while typing.
    ADD_LOCAL_VALIDATES_WHILE_TYPING = 205 "add-local-validates-while-typing" {
        title: "Add Local Repository checks the path as you type",
        summary: "The \"does not appear to be a Git repository\" / bare-repository warning follows \
                  the Local Path field as it changes, and Add Repository is disabled until the \
                  path is a repository.",
        ghd_behaviour: "The path is only checked when Add Repository is pressed; the warning then \
                        stays, stale, while the path is edited.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-ui/src/dialogs/add_existing.rs"],
    },

    /// File › Import Repositories from GitHub Desktop….
    IMPORT_FROM_GITHUB_DESKTOP = 206 "import-from-github-desktop" {
        title: "Import repositories from GitHub Desktop",
        summary: "File › Import Repositories from GitHub Desktop… (and a button on the \
                  \"Let's get started!\" page when GitHub Desktop's data is on this Mac) reads \
                  GitHub Desktop's repository list and adds the repositories you pick, aliases \
                  included.",
        ghd_behaviour: "Not applicable: GitHub Desktop has nothing to import from.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-platform/src/ghd_import.rs", "crates/corvene-core/src/ghd_import.rs", "crates/corvene-ui/src/dialogs/import_github_desktop.rs"],
    },

    /// The repository list filters to repositories with changes or commits to push / pull.
    REPOSITORY_STATUS_FILTER = 207 "repository-status-filter" {
        title: "Repository list status filters",
        summary: "A filter button next to the repository list's filter box shows only \
                  repositories with uncommitted changes and / or commits to push or pull (the \
                  rows' indicators); the Recent group is left out while one is on.",
        ghd_behaviour: "The list filters by name only.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(18322), Upstream::issue(22693)],
        code: &["crates/corvene-ui/src/repository_list.rs"],
    },

    /// The repository list filters to forks or to the rest.
    REPOSITORY_FORK_FILTER = 208 "repository-fork-filter" {
        title: "Repository list fork filter",
        summary: "The repository list's filter button (added by this flag if 110 is off) offers \
                  Forks and Not forks, from the GitHub repository's fork flag.",
        ghd_behaviour: "The list filters by name only.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(15655)],
        code: &["crates/corvene-ui/src/repository_list.rs"],
    },

    /// How many recent repositories the repository list shows.
    RECENT_REPOSITORIES_COUNT = 209 "recent-repositories-count" {
        title: "Recent repositories shown",
        summary: "How many recently opened repositories the repository list shows in its Recent \
                  group (0 hides the group).",
        ghd_behaviour: "Always 3.",
        nature: Nature::Feature,
        kind: Kind::Number { min: 0, max: 20, unit: None },
        corvene: Value::Number(3), ghd: Value::Number(3),
        familiar: Value::Number(3), max: Value::Number(5),
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(15244), Upstream::issue(19828)],
        code: &["crates/corvene-ui/src/repository_list.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// Opening the repository list selects its remembered filter text.
    REPOSITORY_FILTER_SELECTS_TEXT = 210 "repository-filter-selects-text" {
        title: "Repository filter text is selected on open",
        summary: "Opening the repository list (⌘T or the toolbar button) selects the filter text \
                  it remembers, so typing replaces it.",
        ghd_behaviour: "The caret lands after the old text, which has to be deleted first.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(2652)],
        code: &["crates/corvene-ui/src/repository_list.rs"],
    },

    /// `/pattern/` in the repository filter is a regular expression.
    REGEX_REPOSITORY_FILTER = 211 "regex-repository-filter" {
        title: "Regular expressions in the repository filter",
        summary: "Typing /pattern/ in the repository list's filter matches names against a \
                  case-insensitive regular expression; any other text (or an invalid pattern) \
                  filters as before.",
        ghd_behaviour: "Plain text matching only.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(20745)],
        code: &["crates/corvene-ui/src/repository_list.rs", "crates/corvene-core/src/filter.rs"],
    },

    /// Filtering the repository list shows one ranked list.
    FLAT_REPOSITORY_RESULTS = 212 "flat-repository-results" {
        title: "Flat repository filter results",
        summary: "While text is typed in the repository list's filter, the matches form one list \
                  without owner groups, the best match (closest to the start of a word, fewest \
                  extra characters) first.",
        ghd_behaviour: "Matches stay in their owner groups, so the best match can sit far down.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(4860)],
        code: &["crates/corvene-ui/src/repository_list.rs"],
    },

    /// Same-named repositories in a group show the parent folders that differ.
    DUPLICATE_NAMES_SHOW_PATH = 213 "duplicate-names-show-path" {
        title: "Same-named repositories show their folder",
        summary: "When repositories in one group of the repository list share a name, each row \
                  adds, dimmed, the parent folders that tell them apart (fork-a beside fork-b).",
        ghd_behaviour: "Identical rows; only the tooltip's path tells them apart.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(15937)],
        code: &["crates/corvene-ui/src/repository_list.rs"],
    },

    /// Repository list rows name the checked-out branch.
    REPOSITORY_LIST_BRANCH = 214 "repository-list-branch" {
        title: "Branch names in the repository list",
        summary: "Each repository list row shows, dimmed after the name, the branch checked out \
                  in that repository (from the background indicator refresh).",
        ghd_behaviour: "Only the name and the change / ahead-behind indicators.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(8158)],
        code: &["crates/corvene-ui/src/repository_list.rs", "crates/corvene-core/src/remote.rs"],
    },

    /// The repository list's behind arrow in the success colour.
    REPOSITORY_LIST_BEHIND_ACCENT = 215 "repository-list-behind-accent" {
        title: "Repository list: green arrow for commits to pull",
        summary: "In the repository list, the down arrow (the branch is behind its upstream) is \
                  drawn in the success green instead of the badge text colour, except on the \
                  selected row.",
        ghd_behaviour: "Up and down arrows share the badge text colour.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(15005)],
        code: &["crates/corvene-ui/src/repository_list.rs"],
    },

    /// A missing repository's menu removes every missing repository.
    REMOVE_ALL_MISSING_REPOSITORIES = 216 "remove-all-missing-repositories" {
        title: "Remove all missing repositories",
        summary: "The context menu of a repository Corvene cannot find offers \"Remove All N \
                  Missing Repositories\" when several are missing; like removing one missing \
                  repository it only takes them off the list, without confirmation.",
        ghd_behaviour: "Missing repositories are removed one by one.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21151)],
        code: &["crates/corvene-ui/src/repository_list.rs"],
    },

    /// Repository list indicators refresh promptly.
    PROMPT_INDICATOR_REFRESH = 217 "prompt-indicator-refresh" {
        title: "Prompt repository indicator refresh",
        summary: "The repository list's changes and ahead/behind indicators are refreshed right \
                  after launch and whenever the repository list opens (at most once a minute), \
                  not only every 15 minutes.",
        ghd_behaviour: "The indicator updater starts on its delayed 15-minute cadence, so the list \
                        can show stale indicators after launch.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(22154)],
        code: &["crates/corvene-core/src/remote.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// Switching repositories closes dialogs bound to the previous one.
    CLOSE_DIALOGS_ON_REPOSITORY_SWITCH = 218 "close-dialogs-on-repository-switch" {
        title: "Switching repositories closes the previous repository's dialog",
        summary: "When another repository is selected (e.g. from a link or the command line \
                  tool) a dialog for the previous repository closes, except conflict and \
                  credential prompts of an operation in progress.",
        ghd_behaviour: "The dialog stays open over the newly selected repository.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(9847)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-core/src/state.rs"],
    },

    /// Create Repository warns before replacing an existing README.md.
    README_OVERWRITE_WARNING = 219 "readme-overwrite-warning" {
        title: "Create Repository warns about an existing README",
        summary: "With \"Initialize this repository with a README\" ticked and a README.md already \
                  in the folder, the Create a New Repository dialog warns that its content will be \
                  replaced.",
        ghd_behaviour: "The warning only exists in beta builds; release builds silently overwrite \
                        the README.md.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22471)],
        code: &["crates/corvene-ui/src/dialogs/create_repository.rs"],
    },

    /// Create a New Repository in the chosen folder itself.
    CREATE_REPOSITORY_IN_FOLDER = 220 "create-repository-in-folder" {
        title: "Create a repository in an existing folder",
        summary: "Create a New Repository shows \"Create the repository in this folder (no \
                  subfolder)\" under the local path: ticked, the Local Path folder itself becomes \
                  the repository, and a README.md, .gitignore or LICENSE already there is kept.",
        ghd_behaviour: "Always creates a <name> subfolder of the local path.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(11413)],
        code: &["crates/corvene-ui/src/dialogs/create_repository.rs", "crates/corvene-git/src/ops.rs"],
    },

    /// Repository › Add License….
    ADD_LICENSE = 221 "add-license" {
        title: "Add a license to a repository",
        summary: "Repository › Add License… writes one of the license templates Create a New \
                  Repository offers to LICENSE in the current repository (filled in with your \
                  Git name and the year). An existing license file is never replaced.",
        ghd_behaviour: "Licenses can only be added when creating a repository.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(12222)],
        code: &["crates/corvene-ui/src/app_menu.rs", "crates/corvene-ui/src/dialogs/add_license.rs", "crates/corvene-core/src/templates.rs"],
    },

    /// Add Local Repository › Choose… picks several folders.
    ADD_LOCAL_MULTIPLE = 222 "add-local-multiple" {
        title: "Add several local repositories at once",
        summary: "Add Local Repository's Choose… can select several folders; picking more than \
                  one adds every one that is a Git repository and lists the others.",
        ghd_behaviour: "One folder at a time.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(2978)],
        code: &["crates/corvene-ui/src/dialogs/add_existing.rs"],
    },

    /// Add Local Repository autocompletes folders.
    ADD_LOCAL_PATH_COMPLETION = 223 "add-local-path-completion" {
        title: "Folder completion in Add Local Repository",
        summary: "Typing a path (/… or ~/…) in Add Local Repository's Local Path lists the \
                  matching folders; ↑/↓ pick one, Enter or Tab completes it, Esc closes the list.",
        ghd_behaviour: "A plain text box.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(18303)],
        code: &["crates/corvene-ui/src/dialogs/add_existing.rs", "crates/corvene-ui/src/autocompletion.rs", "crates/corvene-core/src/autocomplete.rs"],
    },

    /// An Alias field in New / Add / Clone.
    ALIAS_WHEN_ADDING = 224 "alias-when-adding" {
        title: "Alias field when adding a repository",
        summary: "Create a New Repository, Add Local Repository and Clone a Repository have an \
                  optional Alias field; the repository shows under that name in the list once \
                  it is added.",
        ghd_behaviour: "An alias can only be set afterwards (Create Alias in the repository \
                        list).",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22505)],
        code: &["crates/corvene-ui/src/dialogs/add_existing.rs", "crates/corvene-ui/src/dialogs/create_repository.rs", "crates/corvene-ui/src/dialogs/clone_repository.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// Add › Clone Repository… carries the repository list's filter text.
    CLONE_PREFILLS_FILTER = 225 "clone-prefills-filter" {
        title: "Clone dialog takes the repository filter",
        summary: "Add › Clone Repository… in the repository list opens the clone dialog with the \
                  list's filter text in its GitHub tabs' filter box, so a repository that is not \
                  cloned yet can be found without typing its name again.",
        ghd_behaviour: "The clone dialog's filter starts empty.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20685)],
        code: &[
            "crates/corvene-ui/src/repository_list.rs",
            "crates/corvene-ui/src/dialogs/clone_repository.rs",
        ],
    },

    /// Clone over SSH by default.
    CLONE_PREFERS_SSH = 226 "clone-prefers-ssh" {
        title: "Clone over SSH",
        summary: "Clone a Repository clones repositories picked from the list and owner/name \
                  shorthands with their SSH URL (git@host:owner/name.git). An https:// URL typed \
                  on the URL tab is still cloned over HTTPS.",
        ghd_behaviour: "Clones over HTTPS unless an SSH URL is typed.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(19824)],
        code: &["crates/corvene-ui/src/dialogs/clone_repository.rs", "crates/corvene-core/src/clone_info.rs"],
    },

    /// The clone list's filter accepts repository URLs.
    CLONE_FILTER_ACCEPTS_URLS = 227 "clone-filter-accepts-urls" {
        title: "Clone: repository URLs in the list filter",
        summary: "A repository URL pasted into the repository filter of Clone a Repository (or \
                  the \"Let's get started!\" page) filters by its owner/name, so \
                  https://github.com/owner/name finds owner/name.",
        ghd_behaviour: "Fuzzy-matches the whole URL and finds no repository.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20942)],
        code: &["crates/corvene-ui/src/cloneable_repositories.rs"],
    },

    /// The clone account picker's default account.
    CLONE_DEFAULT_ACCOUNT = 228 "clone-default-account" {
        title: "Default account for cloning",
        summary: "With several accounts signed in, Clone a Repository (and the \"Let's get \
                  started!\" page) start on the first account whose login is in this list \
                  (comma-separated) instead of the first account signed in; empty for GitHub \
                  Desktop's order.",
        ghd_behaviour: "The account signed in first, every time the dialog opens.",
        nature: Nature::Feature,
        kind: Kind::Text { placeholder: "your-login", validate: account_logins },
        corvene: Value::text(""), ghd: Value::text(""),
        familiar: Value::text(""), max: Value::text(""),
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21743)],
        code: &["crates/corvene-ui/src/cloneable_repositories.rs", "crates/corvene-ui/src/dialogs/clone_repository.rs", "crates/corvene-ui/src/no_repositories.rs"],
    },

    /// Owners whose repositories the clone lists leave out.
    HIDDEN_CLONE_OWNERS = 229 "hidden-clone-owners" {
        title: "Owners hidden from the clone list",
        summary: "GitHub users or organizations (comma-separated logins) whose repositories the \
                  clone dialog and the blank slate's repository list leave out.",
        ghd_behaviour: "Every repository the account can access is listed.",
        nature: Nature::Feature,
        kind: Kind::Text { placeholder: "org-a, org-b", validate: owner_list },
        corvene: Value::text(""), ghd: Value::text(""),
        familiar: Value::text(""), max: Value::text(""),
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(11908)],
        code: &[
            "crates/corvene-ui/src/cloneable_repositories.rs",
            "crates/corvene-ui/src/dialogs/clone_repository.rs",
            "crates/corvene-ui/src/no_repositories.rs",
        ],
    },

    /// Clone paths mirror owner/name.
    CLONE_PATH_INCLUDES_OWNER = 230 "clone-path-includes-owner" {
        title: "Clone into an owner folder",
        summary: "The local path Clone a Repository suggests is <clone folder>/<owner>/<name> \
                  (for example GitHub/desktop/desktop), so repositories with the same name from \
                  different owners don't collide.",
        ghd_behaviour: "Suggests <clone folder>/<name>.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21293), Upstream::issue(5449)],
        code: &["crates/corvene-ui/src/dialogs/clone_repository.rs"],
    },

    /// Clone offers to add a repository already at the local path.
    CLONE_OFFER_ADD_EXISTING = 231 "clone-offer-add-existing" {
        title: "Clone: add a repository already at the destination",
        summary: "When Clone a Repository's local path is already a Git repository (usually an \
                  earlier clone), \"Add this repository instead?\" under the path adds it.",
        ghd_behaviour: "Only says the folder contains files; the repository has to be added \
                        through Add Local Repository.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(2956), Upstream::issue(3540)],
        code: &["crates/corvene-ui/src/dialogs/clone_repository.rs"],
    },

    /// Clone from a local folder.
    CLONE_LOCAL_SOURCES = 232 "clone-local-sources" {
        title: "Clone from a local folder",
        summary: "Clone a Repository's URL tab takes a local repository (/path, ~/path or a \
                  file:// URL): the local path is named after the folder, and a path without a \
                  Git repository is reported before cloning.",
        ghd_behaviour: "A path like /a/b is taken for the GitHub repository a/b; longer paths \
                        can't be cloned.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(2995)],
        code: &["crates/corvene-core/src/clone_info.rs", "crates/corvene-ui/src/dialogs/clone_repository.rs"],
    },

    /// Clone a Repository's "Shallow clone" checkbox.
    SHALLOW_CLONE = 233 "shallow-clone" {
        title: "Shallow clone option",
        summary: "Clone a Repository shows a \"Shallow clone\" checkbox under the local path; \
                  ticked, only the latest commit of the default branch is fetched \
                  (git clone --depth 1).",
        ghd_behaviour: "Always clones the full history.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21880)],
        code: &["crates/corvene-ui/src/dialogs/clone_repository.rs", "crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/ops.rs"],
    },

    /// The cloning view's Cancel button.
    CLONE_CANCEL = 234 "clone-cancel" {
        title: "Cancel a running clone",
        summary: "The cloning view has a Cancel button that stops `git clone`; git removes the \
                  directory it created.",
        ghd_behaviour: "No way to stop a clone: removing the cloning repository leaves the download \
                        running in the background.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21866), Upstream::issue(22478)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-ui/src/cloning_view.rs", "crates/corvene-git/src/process.rs"],
    },

    /// A failed clone reopens the clone dialog.
    CLONE_FAILURE_KEEPS_INPUT = 235 "clone-failure-keeps-input" {
        title: "Failed clone keeps the dialog's input",
        summary: "When a clone fails, Clone a Repository opens again with the same URL and \
                  local path and git's error at the top, ready to fix and retry.",
        ghd_behaviour: "Shows a \"Clone failed\" error dialog; the URL and path have to be \
                        entered again.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(8720)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-ui/src/dialogs/clone_repository.rs"],
    },

    /// Repository Settings › Remote shows the `upstream` remote.
    UPSTREAM_REMOTE_IN_SETTINGS = 236 "upstream-remote-in-settings" {
        title: "Repository Settings shows the upstream remote",
        summary: "Repository Settings › Remote shows the `upstream` remote's URL (read-only) \
                  under the primary remote's.",
        ghd_behaviour: "Only the primary remote.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(6877)],
        code: &["crates/corvene-ui/src/dialogs/repository_settings.rs"],
    },

    /// Repository Settings › Ignored Files offers the bundled templates.
    GITIGNORE_TEMPLATES = 237 "gitignore-templates" {
        title: "Repository Settings: .gitignore templates",
        summary: "Repository Settings › Ignored Files has an \"Add a template\" list (the \
                  Create a New Repository .gitignore templates) that fills an empty box or \
                  appends the template.",
        ghd_behaviour: "Templates only when creating a repository.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(2197)],
        code: &["crates/corvene-ui/src/dialogs/repository_settings.rs"],
    },

    /// Repository Settings › Ignored Files › Edit global ignore file.
    EDIT_GLOBAL_IGNORE_FILE = 238 "edit-global-ignore-file" {
        title: "Edit the global ignore file",
        summary: "Repository Settings › Ignored Files has an \"Edit global ignore file\" link that \
                  opens git's excludes file (core.excludesFile, else ~/.config/git/ignore, created \
                  when missing) in the external editor.",
        ghd_behaviour: "Only the repository's root .gitignore can be edited.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21951)],
        code: &["crates/corvene-ui/src/dialogs/repository_settings.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// Repository Settings › Git Config › Line endings (core.autocrlf).
    LINE_ENDINGS_SETTING = 239 "line-endings-setting" {
        title: "Line endings setting per repository",
        summary: "Repository Settings › Git Config adds \"Line endings (core.autocrlf)\": use the \
                  global config, or store true, input or false in the repository's own config. \
                  It applies to later checkouts and commits; files already checked out keep \
                  their line endings.",
        ghd_behaviour: "No line ending option; core.autocrlf has to be set on the command line.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(5230)],
        code: &["crates/corvene-ui/src/dialogs/repository_settings.rs", "crates/corvene-core/src/integrations.rs"],
    },

    /// Worktree rows show and match their path.
    WORKTREE_PATHS = 240 "worktree-paths" {
        title: "Worktree list shows and searches paths",
        summary: "Rows in the worktree list have a tooltip with the worktree's name and full \
                  path, and the filter also matches the path.",
        ghd_behaviour: "Only the folder name, truncated, and the filter matches only the name, \
                        so worktrees with the same folder name look alike.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22650), Upstream::issue(22946), Upstream::issue(22375)],
        code: &["crates/corvene-ui/src/worktree_list.rs"],
    },

    /// Default location for new worktrees.
    WORKTREE_LOCATION = 241 "worktree-location" {
        title: "Default worktree location",
        summary: "Where New Worktree puts worktrees by default: `{clone-dir}` is Settings' \
                  clone directory, `{repo}` the repository's name and a leading `~` the home \
                  folder (e.g. `~/code/worktrees/{repo}`).",
        ghd_behaviour: "Always the clone directory.",
        nature: Nature::Feature,
        kind: Kind::Text { placeholder: "{clone-dir}", validate: worktree_location },
        corvene: Value::text("{clone-dir}"), ghd: Value::text("{clone-dir}"),
        familiar: Value::text("{clone-dir}"), max: Value::text("{clone-dir}"),
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22308)],
        code: &["crates/corvene-ui/src/worktree_list.rs", "crates/corvene-core/src/worktrees.rs"],
    },

    /// New worktrees default to the repository's own folder.
    WORKTREE_DIR_BESIDE_REPOSITORY = 242 "worktree-dir-beside-repository" {
        title: "New worktrees beside the repository",
        summary: "Add Worktree's path starts in the folder that holds the repository's main \
                  worktree, so worktrees become its siblings.",
        ghd_behaviour: "Starts in the last clone folder, whatever repository the dialog is for.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22565)],
        code: &["crates/corvene-ui/src/worktree_list.rs"],
    },

    /// The worktree list leaves out prunable worktrees.
    HIDE_PRUNABLE_WORKTREES = 243 "hide-prunable-worktrees" {
        title: "Hide prunable worktrees",
        summary: "The worktree list leaves out worktrees git reports as prunable (their directory \
                  was deleted outside git), so they cannot be selected into an error.",
        ghd_behaviour: "Lists them until `git worktree prune` runs; selecting one shows \"does not \
                        appear to be a valid Git repository\".",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22605)],
        code: &["crates/corvene-ui/src/worktree_list.rs", "crates/corvene-ui/src/toolbar.rs"],
    },

    /// Which repositories the hourly background fetch covers.
    BACKGROUND_FETCH = 244 "background-fetch" {
        title: "Background fetch",
        summary: "Which selected repositories are fetched in the background every hour: none, \
                  GitHub repositories only, or any repository with a remote.",
        ghd_behaviour: "GitHub repositories only, with no way to turn it off.",
        nature: Nature::Feature,
        kind: Kind::Select { options: BACKGROUND_FETCHES },
        corvene: Value::text("github"), ghd: Value::text("github"),
        familiar: Value::text("github"), max: Value::text("any"),
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(10687), Upstream::issue(12474)],
        code: &["crates/corvene-core/src/remote.rs"],
    },

    /// Push, pull and fetch stay available during a background fetch.
    PUSH_DURING_BACKGROUND_FETCH = 245 "push-during-background-fetch" {
        title: "Push during a background fetch",
        summary: "The hourly background fetch runs without taking over the push/pull button, and a \
                  push, pull or fetch asked for meanwhile starts as soon as it finishes.",
        ghd_behaviour: "The button shows the background fetch's progress and is disabled; a push \
                        from the menu is ignored until the fetch is done.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(1011)],
        code: &["crates/corvene-core/src/remote.rs"],
    },

    /// The background fetch fast-forwards the current branch.
    BACKGROUND_FETCH_FAST_FORWARDS = 246 "background-fetch-fast-forwards" {
        title: "Background fetch pulls when safe",
        summary: "After a background fetch, the checked-out branch is fast-forwarded to its \
                  upstream when it is only behind, the working directory has no changes and no \
                  merge, rebase or cherry-pick is in progress. Anything else is left for Pull.",
        ghd_behaviour: "Only fetches; the branch stays behind until you pull.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(16586)],
        code: &["crates/corvene-core/src/remote.rs", "crates/corvene-git/src/remote_ops.rs"],
    },

    /// Repository › Fetch All Repositories.
    FETCH_ALL_REPOSITORIES = 247 "fetch-all-repositories" {
        title: "Repository › Fetch All Repositories",
        summary: "The Repository menu can fetch every repository in the list that has a remote, \
                  one after another; failures are listed in one error at the end.",
        ghd_behaviour: "Fetches the selected repository only; others wait for their background \
                        fetch after being selected.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(13700)],
        code: &["crates/corvene-ui/src/app_menu.rs", "crates/corvene-core/src/remote.rs"],
    },

    /// Fetch deletes local tags the remote no longer has.
    FETCH_PRUNE_TAGS = 248 "fetch-prune-tags" {
        title: "Fetch prunes deleted tags",
        summary: "Fetch (and the background fetch) passes --prune-tags, so tags deleted on the \
                  remote disappear locally. Local tags that were never pushed are deleted too, \
                  except while tags created in Corvene are waiting to be pushed.",
        ghd_behaviour: "Prunes branches only; a tag deleted on the remote stays forever.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(21022), Upstream::issue(22776)],
        code: &["crates/corvene-core/src/remote.rs", "crates/corvene-git/src/remote_ops.rs"],
    },

    /// Fetch passes `--write-commit-graph`.
    FETCH_WRITES_COMMIT_GRAPH = 249 "fetch-writes-commit-graph" {
        title: "Fetch updates the commit-graph",
        summary: "Fetch passes --write-commit-graph, so git extends the commit-graph file that \
                  speeds up history, ahead/behind and merge-base computations in big repositories.",
        ghd_behaviour: "Plain fetch; the commit-graph is only written by git's own maintenance.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(22045)],
        code: &["crates/corvene-core/src/remote.rs", "crates/corvene-git/src/remote_ops.rs"],
    },

    /// Fetch and pull leave submodules alone.
    SYNC_SKIPS_SUBMODULES = 250 "sync-skips-submodules" {
        title: "Fetch and pull skip submodules",
        summary: "Fetch and pull pass --no-recurse-submodules, so submodules are neither fetched \
                  nor updated; syncing them is left to you.",
        ghd_behaviour: "Fetch recurses into submodules on demand and pull always updates them, \
                        which fails the whole pull when a submodule has conflicts.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(15758)],
        code: &["crates/corvene-core/src/remote.rs", "crates/corvene-git/src/remote_ops.rs"],
    },

    /// Pull skips `remote set-head -a` when the remote HEAD is known.
    REMOTE_HEAD_ONCE = 251 "remote-head-once" {
        title: "Skip updating the remote HEAD",
        summary: "After a pull, `git remote set-head -a` (which lists every ref on the server) \
                  only runs when refs/remotes/<remote>/HEAD is missing or points at a branch that \
                  no longer exists.",
        ghd_behaviour: "Runs it after every pull, which takes minutes on repositories with \
                        hundreds of thousands of refs.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(22039)],
        code: &["crates/corvene-core/src/remote.rs", "crates/corvene-git/src/remote_ops.rs"],
    },

    /// Prune stale remote refs and retry a failed fetch or pull.
    PRUNE_STALE_REFS_AND_RETRY = 252 "prune-stale-refs-and-retry" {
        title: "Prune stale remote refs and retry",
        summary: "A fetch or pull that fails with \"cannot lock ref\" / \"unable to update local \
                  ref\" runs `git remote prune` and tries once more before showing an error.",
        ghd_behaviour: "Shows the error; the user has to run `git remote prune origin` in a \
                        terminal.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(11391)],
        code: &["crates/corvene-core/src/remote.rs", "crates/corvene-git/src/remote_ops.rs"],
    },

    /// "Last fetched" counts the clone.
    CLONE_COUNTS_AS_FETCH = 253 "clone-counts-as-fetch" {
        title: "Last fetched counts the clone",
        summary: "A repository that was cloned and not fetched since shows the clone's time as \
                  \"Last fetched\" (from HEAD's first reflog entry) instead of \"Never fetched\".",
        ghd_behaviour: "Reads FETCH_HEAD only, which a clone does not write, so a fresh clone \
                        says \"Never fetched\".",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(13401)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/remote_ops.rs"],
    },

    /// Fetch / pull / push give up on a stalled HTTP transfer.
    NETWORK_STALL_TIMEOUT = 254 "network-stall-timeout" {
        title: "Give up on stalled fetch, pull and push",
        summary: "Seconds an HTTPS fetch, pull, push or clone may transfer nothing before git \
                  aborts it with an error (GIT_HTTP_LOW_SPEED_LIMIT=1 and \
                  GIT_HTTP_LOW_SPEED_TIME). 0 waits forever. Does not apply to SSH remotes.",
        ghd_behaviour: "No limit: a stalled connection leaves the operation spinning until the app \
                        is restarted.",
        nature: Nature::Feature,
        kind: Kind::Number { min: 0, max: 3600, unit: Some("s") },
        corvene: Value::Number(0), ghd: Value::Number(0),
        familiar: Value::Number(0), max: Value::Number(60),
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(22863)],
        code: &["crates/corvene-core/src/remote.rs", "crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/process.rs"],
    },

    /// Plain-language text for two confusing git errors.
    PLAIN_LANGUAGE_REMOTE_ERRORS = 255 "plain-language-remote-errors" {
        title: "Plain-language remote errors",
        summary: "A pull whose upstream branch was deleted on the remote, a clone into a folder \
                  you may not write to, a remote failure whose real cause (out of memory, a lost \
                  connection) precedes \"Could not read from remote repository\", and a \
                  non-origin remote whose repository is gone explain what happened in a \
                  sentence before git's message.",
        ghd_behaviour: "Shows git's text only (\"Your configuration specifies to merge with the \
                        ref …\", \"Permission denied\"), and calls every \"Could not read from \
                        remote repository\" an SSH permission problem.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(1325), Upstream::issue(13187), Upstream::issue(22413), Upstream::issue(3715)],
        code: &["crates/corvene-core/src/push_errors.rs", "crates/corvene-core/src/remote.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// Remote names containing `/` are matched whole.
    REMOTE_NAMES_WITH_SLASHES = 256 "remote-names-with-slashes" {
        title: "Remote names with slashes",
        summary: "A remote branch's remote is found by matching the configured remote names, so \
                  a remote called team/fork gives team/fork/main the branch name main (checkout, \
                  push, pull requests and the branch list use it).",
        ghd_behaviour: "Takes everything before the first / as the remote name (team), so the \
                        branch becomes fork/main.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(3618)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/repo.rs", "crates/corvene-models/src/lib.rs"],
    },

    /// The Pull button's tooltip lists the incoming commits.
    PULL_TOOLTIP_LISTS_COMMITS = 257 "pull-tooltip-lists-commits" {
        title: "Pull button lists incoming commits",
        summary: "Hovering Pull shows the summaries of the commits it would bring in (up to ten, \
                  newest first, then how many more).",
        ghd_behaviour: "No tooltip; only the behind count.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(6753)],
        code: &["crates/corvene-ui/src/toolbar.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// A failed force push keeps Force Push recommended.
    FORCE_PUSH_KEPT_ON_FAILURE = 258 "force-push-kept-on-failure" {
        title: "Failed force push keeps Force Push",
        summary: "After a rebase, the branch's \"Force push\" recommendation is only cleared once a \
                  force push succeeds, so the button still offers it after a failed attempt.",
        ghd_behaviour: "Clears the recommendation before the push runs; after a failure the button \
                        offers a plain Push that git rejects.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(16352)],
        code: &["crates/corvene-core/src/remote.rs"],
    },

    /// Force push is recommended after an amend only when the amended commit was pushed.
    AMEND_FORCE_PUSH_IF_PUSHED = 259 "amend-force-push-if-pushed" {
        title: "Force push only after amending a pushed commit",
        summary: "After amending, the toolbar recommends Force push only when the amended commit \
                  is on the branch's upstream; amending a commit that was never pushed on a branch \
                  that is also behind offers Pull, as before the amend.",
        ghd_behaviour: "Every amend makes Force push the recommended action once the branch has \
                        diverged, even when the amended commit was never pushed.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20526)],
        code: &["crates/corvene-core/src/dispatcher.rs"],
    },

    /// Force push is recommended after a rewrite outside Corvene.
    FORCE_PUSH_AFTER_OUTSIDE_REWRITE = 260 "force-push-after-outside-rewrite" {
        title: "Suggest force push after an outside rewrite",
        summary: "When the branch is ahead of and behind its upstream and the upstream's tip is in \
                  the branch's reflog (pushed commits were amended, rebased or reset in another \
                  tool), the push/pull button recommends Force push instead of Pull.",
        ghd_behaviour: "Recommends a force push only after its own amend or rebase; otherwise it \
                        offers Pull, which merges the old commits back in or conflicts.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(9739)],
        code: &["crates/corvene-core/src/remote.rs", "crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/remote_ops.rs"],
    },

    /// The push/pull foldout's "Reset to <upstream>".
    RESET_TO_REMOTE = 261 "reset-to-remote" {
        title: "Reset to remote",
        summary: "While the current branch has commits its upstream lacks, the push/pull dropdown \
                  offers \"Reset to origin/…\": after a confirmation that names what is discarded, \
                  the branch and working directory are reset hard to the upstream.",
        ghd_behaviour: "No such command; resetting to the remote needs a terminal.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(16673)],
        code: &["crates/corvene-ui/src/foldout.rs", "crates/corvene-core/src/dispatcher.rs", "crates/corvene-ui/src/dialogs/history_dialogs.rs"],
    },

    /// View on GitHub opens other hosts' remotes too.
    VIEW_ON_REMOTE = 262 "view-on-remote" {
        title: "View on Remote for other hosts",
        summary: "Repository › View on GitHub opens the default remote's web page (as https://host/path) \
                  for a repository that is not on GitHub, and the repository list's context menu offers \
                  it as \"View on Remote\".",
        ghd_behaviour: "View on GitHub does nothing / is disabled for repositories not on GitHub.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(17846), Upstream::issue(20840)],
        code: &["crates/corvene-core/src/integrations.rs", "crates/corvene-ui/src/repository_list.rs"],
    },

    /// Submodules follow merges too, and changed ones are spared.
    SUBMODULES_FOLLOW_CHECKOUT = 263 "submodules-follow-checkout" {
        title: "Update submodules after merges, sparing changed ones",
        summary: "After a merge (including Update from Default Branch) submodules are checked \
                  out at the commits the branch records and new ones are cloned (git submodule \
                  update --init --recursive), as after switching branches. Submodules that \
                  showed changes beforehand are left alone, after a merge or a switch.",
        ghd_behaviour: "Updates every submodule after switching branches, changed ones included \
                        (their checked-out commit moves back), and none after a merge, so they \
                        show as changed and are easily committed back.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(18302), Upstream::issue(18673), Upstream::issue(9547)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-core/src/mco.rs", "crates/corvene-git/src/remote_ops.rs"],
    },

    /// Remove a left-over index.lock from the error dialog.
    REMOVE_STALE_INDEX_LOCK = 265 "remove-stale-index-lock" {
        title: "Remove a left-over index.lock",
        summary: "When git fails because .git/index.lock exists, the error explains it and offers \
                  Remove Lock File, which deletes the lock only when no Git process is running in \
                  the repository.",
        ghd_behaviour: "Shows git's error; the lock has to be deleted by hand.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(908)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/index_lock.rs", "crates/corvene-ui/src/dialogs/simple.rs"],
    },

    /// Repository list group headers collapse.
    COLLAPSIBLE_REPOSITORY_GROUPS = 266 "collapsible-repository-groups" {
        title: "Collapsible repository groups",
        summary: "The repository list's group headers (Recent, each owner, Other) get a chevron; \
                  clicking a header hides or shows its repositories and the collapsed groups are \
                  remembered. While the list is filtered every group is expanded, and the arrow \
                  keys skip hidden rows.",
        ghd_behaviour: "Groups are always expanded, so a long list of one owner's repositories has \
                        to be scrolled past.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[
            Upstream::issue(9910),
            Upstream::issue(20228),
            Upstream::issue(21908),
            Upstream::issue(14997),
        ],
        code: &["crates/corvene-ui/src/repository_list.rs", "crates/corvene-core/src/persistence.rs"],
    },

    /// Pin repositories to the top of the repository list.
    PINNED_REPOSITORIES = 267 "pinned-repositories" {
        title: "Pinned repositories",
        summary: "The repository list's context menu offers Pin and Unpin; pinned repositories are \
                  listed by name in a Pinned group above Recent (and stay in their owner groups). \
                  The group is hidden while the list is filtered.",
        ghd_behaviour: "Only the three most recently opened repositories are listed above the \
                        owner groups; there is no way to keep a repository at the top.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22751)],
        code: &["crates/corvene-ui/src/repository_list.rs", "crates/corvene-core/src/dispatcher.rs", "crates/corvene-models/src/lib.rs"],
    },

    /// One alphabetical repository list instead of owner groups.
    UNGROUPED_REPOSITORY_LIST = 268 "ungrouped-repository-list" {
        title: "Ungrouped repository list",
        summary: "Without filter text the repository list shows every repository in one \
                  alphabetical Repositories group after Recent, instead of a group per GitHub \
                  owner and Other. ⇧⌘] / ⇧⌘[ (612) follow the same order.",
        ghd_behaviour: "Repositories are always grouped by GitHub owner, with the rest under Other.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(11460)],
        code: &["crates/corvene-ui/src/repository_list.rs"],
    },

    /// Remove several repositories at once.
    BULK_REMOVE_REPOSITORIES = 269 "bulk-remove-repositories" {
        title: "Remove several repositories at once",
        summary: "File › Remove Repositories… and the repository list's context menu open a dialog \
                  listing every repository with a checkbox and a filter box; Remove takes the \
                  ticked ones out of Corvene (optionally moving their folders to the Trash, as \
                  the single Remove does).",
        ghd_behaviour: "Repositories are removed one at a time, each with its own confirmation.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20684), Upstream::issue(22135), Upstream::issue(22434)],
        code: &[
            "crates/corvene-ui/src/dialogs/remove_repositories.rs",
            "crates/corvene-ui/src/repository_list.rs",
            "crates/corvene/src/menus.rs",
            "crates/corvene/src/main.rs",
        ],
    },

    /// A stash icon on repositories with stashed changes.
    REPOSITORY_LIST_STASH_ICON = 270 "repository-list-stash-icon" {
        title: "Repository list shows stashes",
        summary: "Repositories with stashed changes show a stash icon in the repository list (from \
                  the opened repository's stashes, or the background indicator refresh for the \
                  others), like the branch list's stash icon (854). Only stashes Corvene can show \
                  count: its own on any branch, and command-line ones while 728 is on.",
        ghd_behaviour: "Nothing in the repository list tells which repositories hold stashed changes.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(15225)],
        code: &["crates/corvene-ui/src/repository_list.rs", "crates/corvene-core/src/remote.rs", "crates/corvene-git/src/repo.rs"],
    },

    /// Repository list indicators survive a restart.
    PERSIST_REPOSITORY_INDICATORS = 271 "persist-repository-indicators" {
        title: "Remember repository indicators",
        summary: "The repository list's ahead/behind arrows, changes dot (and branch and stash \
                  extras) are saved after each background refresh and shown at the next launch \
                  until the first refresh replaces them.",
        ghd_behaviour: "The indicators are kept in memory only, so after a launch the list shows \
                        none until the background refresh has visited every repository.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(5591)],
        code: &["crates/corvene-core/src/remote.rs", "crates/corvene-core/src/dispatcher.rs", "crates/corvene-core/src/persistence.rs"],
    },

    /// Ahead / behind counts with a thousands separator.
    GROUPED_AHEAD_BEHIND_COUNTS = 272 "grouped-ahead-behind-counts" {
        title: "Thousands separators in ahead/behind counts",
        summary: "The push/pull button's ahead/behind badge, its commits-to-pull tooltip and the \
                  repository list's ahead/behind tooltip and screen reader label write counts \
                  with the thousands separator from Appearance › Formatting (1,234).",
        ghd_behaviour: "Counts are plain digits (1234) whatever the number format.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(1245)],
        code: &["crates/corvene-ui/src/toolbar.rs", "crates/corvene-ui/src/repository_list.rs"],
    },

    /// Forks name their parent in the repository tooltips.
    FORK_PARENT_IN_TOOLTIP = 273 "fork-parent-in-tooltip" {
        title: "Tooltips name a fork's parent",
        summary: "For a forked GitHub repository, the Current Repository button's tooltip and the \
                  repository list row's tooltip end with \"Fork of owner/name\".",
        ghd_behaviour: "Only the fork icon tells a fork apart; its parent repository is not shown.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(16568)],
        code: &["crates/corvene-ui/src/toolbar.rs", "crates/corvene-ui/src/repository_list.rs"],
    },

    /// No "Publish repository" before the repository has loaded.
    NO_PUBLISH_BEFORE_LOAD = 274 "no-publish-before-load" {
        title: "No Publish repository while loading",
        summary: "Until a newly selected repository has been read, the push/pull button is a \
                  disabled blank button instead of \"Publish repository\", which a repository \
                  with a remote never needs.",
        ghd_behaviour: "Briefly offers \"Publish this repository to GitHub\" for every repository \
                        while it loads, including cloned ones.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(4107)],
        code: &["crates/corvene-ui/src/toolbar.rs"],
    },

    /// A friendlier branch button on a detached HEAD.
    DETACHED_HEAD_FRIENDLY = 275 "detached-head-friendly" {
        title: "Friendlier detached HEAD",
        summary: "On a detached HEAD the branch button names the tag HEAD is at (\"On v1.2.0\") \
                  instead of the short SHA when there is one, and its tooltip explains that \
                  no branch is checked out and new commits need a branch to be kept.",
        ghd_behaviour: "Shows \"On <short SHA>\" with the tooltip \"Currently on a detached HEAD\".",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(10857)],
        code: &["crates/corvene-ui/src/toolbar.rs"],
    },

    /// Destructive confirmations name what they remove.
    DESCRIPTIVE_CONFIRM_BUTTONS = 276 "descriptive-confirm-buttons" {
        title: "Descriptive confirmation buttons",
        summary: "The destructive button of the Remove Repository, Delete Branch, Delete Tag and \
                  Delete Worktree confirmations says what it does (\"Remove Repository\", \
                  \"Delete Branch\") instead of a bare \"Remove\" or \"Delete\".",
        ghd_behaviour: "The buttons read \"Remove\" and \"Delete\".",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(8591)],
        code: &[
            "crates/corvene-ui/src/dialog.rs",
            "crates/corvene-ui/src/dialogs/app_dialogs.rs",
            "crates/corvene-ui/src/dialogs/branch_dialogs.rs",
            "crates/corvene-ui/src/dialogs/history_dialogs.rs",
            "crates/corvene-ui/src/dialogs/worktree_dialogs.rs",
        ],
    },

    /// Bold filter matches in the clone list, like the other lists.
    CONSISTENT_FILTER_HIGHLIGHT = 277 "consistent-filter-highlight" {
        title: "Bold filter matches when cloning",
        summary: "In Clone a Repository's lists (and the signed-in blank slate) the characters \
                  matching the filter are bold, as in the branch and repository lists.",
        ghd_behaviour: "Black on bright yellow (an unstyled `<mark>`), unlike every other list.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(6898)],
        code: &["crates/corvene-ui/src/cloneable_repositories.rs"],
    },

    /// The background fetch also runs when GitHub saw a push.
    FETCH_ON_KNOWN_PUSH = 278 "fetch-on-known-push" {
        title: "Fetch when GitHub reports a push",
        summary: "Between the hourly background fetches, the selected GitHub repository is \
                  checked every five minutes and fetched as soon as GitHub says it was pushed \
                  to after the last fetch, so ahead/behind counts and the pull button catch up \
                  quickly.",
        ghd_behaviour: "Waits for the next hourly background fetch.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(22217)],
        code: &["crates/corvene-core/src/remote.rs", "crates/corvene-github/src/api.rs"],
    },

    /// Trust Repository explains when git still refuses the folder.
    EXPLAIN_TRUST_FAILURE = 279 "explain-trust-failure" {
        title: "Explain when Trust Repository does not help",
        summary: "When Trust Repository added the folder to safe.directory but Git still refuses \
                  it (network shares, WSL and UNC paths), an error explains why and shows the \
                  safe.directory value Git itself suggests.",
        ghd_behaviour: "Adds the folder and shows the Trust Repository view again with no \
                        explanation, so clicking it never helps.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(19451)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/config.rs"],
    },

    /// Adding a repository names a stale core.worktree.
    STALE_CORE_WORKTREE_HINT = 280 "stale-core-worktree-hint" {
        title: "Name a stale core.worktree when adding",
        summary: "Adding a folder whose repository's core.worktree setting points to a folder \
                  that no longer exists stops with an explanation and the \
                  `git config --unset core.worktree` command that fixes it.",
        ghd_behaviour: "Fails to add the repository (or adds one that shows up missing) without \
                        saying why.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(13654)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/ops.rs"],
    },

    /// Clone progress through git's "Updating files" step.
    CLONE_UPDATING_FILES_STEP = 281 "clone-updating-files-step" {
        title: "Clone progress covers checking out the files",
        summary: "The clone progress bar counts git's \"Updating files\" lines as the checkout \
                  step, so it keeps moving through the last fifth while the files are written.",
        ghd_behaviour: "Waits for \"Checking out files\", which current git no longer prints, so \
                        the bar stops at 80 % while the files are checked out.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/ops.rs", "crates/corvene-git/src/remote_ops.rs"],
    },

    /// Fast-forwarding after a fetch leaves out other worktrees' branches.
    FAST_FORWARD_SKIPS_WORKTREE_BRANCHES = 282 "fast-forward-skips-worktree-branches" {
        title: "Fast-forward branches checked out in other worktrees",
        summary: "After a fetch, pull or push the branches that are behind their upstream are \
                  fast-forwarded without the ones checked out in another worktree, which git \
                  refuses to move.",
        ghd_behaviour: "Hands git every branch that differs from its upstream; when one is \
                        checked out in another worktree git refuses the whole update, so no \
                        branch is fast-forwarded.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[],
        code: &["crates/corvene-core/src/remote.rs", "crates/corvene-git/src/remote_ops.rs"],
    },

    /// The changes can move to another worktree.
    MOVE_CHANGES_TO_WORKTREE = 283 "move-changes-to-worktree" {
        title: "Move changes to another worktree",
        summary: "Branch › Move Changes to Worktree… (also in the changes list's menu) picks \
                  another worktree of the repository that has no changes of its own: the \
                  changes are stashed here and restored there, and Corvene switches to it \
                  unless unticked. When restoring conflicts, the stash is kept and the \
                  conflicts are resolved in that worktree.",
        ghd_behaviour: "Switching worktrees leaves the changes where they are; moving them \
                        takes a stash and the command line.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22756)],
        code: &["crates/corvene-core/src/stash_flows.rs", "crates/corvene-ui/src/dialogs/worktree_dialogs.rs", "crates/corvene-ui/src/app_menu.rs", "crates/corvene-ui/src/changes.rs"],
    },

    /// Changes list › Open Submodule in Corvene.
    OPEN_SUBMODULE_FROM_CHANGES = 284 "open-submodule-from-changes" {
        title: "Open a submodule from the changes list",
        summary: "A changed submodule's menu in the changes list has \"Open Submodule in \
                  Corvene\", which adds it as a repository (if needed) and switches to it; \
                  double-clicking its row does the same.",
        ghd_behaviour: "Only the submodule's diff offers \"Open Repository\" (when its URL is \
                        known); double-clicking the row opens the folder in the editor.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20921)],
        code: &["crates/corvene-ui/src/changes.rs"],
    },

    /// A clone whose submodules fail is added anyway.
    CLONE_KEEPS_REPO_ON_SUBMODULE_FAILURE = 285 "clone-keeps-repo-on-submodule-failure" {
        title: "Keep a clone when a submodule fails",
        summary: "When cloning succeeds but one of the repository's submodules cannot be cloned \
                  (a moved or private submodule), the repository is added and selected anyway \
                  and the error, titled \"Some submodules could not be cloned\", says how to \
                  fetch them later.",
        ghd_behaviour: "The clone fails as a whole: the repository is not added, although git \
                        left it on disk, and adding it again means Add Local Repository.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(3242)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/ops.rs"],
    },

    /// Create Repository leaves files over 100 MB out of the first commit.
    INITIAL_COMMIT_SKIPS_LARGE_FILES = 286 "initial-commit-skips-large-files" {
        title: "Leave large files out of a new repository's first commit",
        summary: "Creating a repository in a folder with files over 100 MB (GitHub's limit) that \
                  Git LFS does not track leaves them out of the initial commit: they stay as \
                  uncommitted changes and a notice names them, so the repository can still be \
                  published.",
        ghd_behaviour: "Everything in the folder goes into the initial commit, and publishing \
                        the repository then fails on the large files.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(7849)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/ops.rs"],
    },

    /// Unreadable repository list entries are set aside, not lost.
    REPOSITORY_LIST_BACKUP = 287 "repository-list-backup" {
        title: "Keep the repository list safe",
        summary: "When entries of the stored repository list cannot be read (after going back to \
                  an older version, say), Corvene sets them aside and brings them back once a \
                  version that reads them starts, instead of loading an empty list and saving \
                  over it; a banner says so. A list that cannot be read at all is copied before \
                  a new one starts. On every update the data file is copied to \
                  corvene-<previous version>.redb.bak, and a banner says when the data could not \
                  be opened and nothing is being saved.",
        ghd_behaviour: "A repository list that cannot be read is lost; there is no backup.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: true, visible: true, availability: available,
        upstream: &[Upstream::issue(9954)],
        code: &["crates/corvene-core/src/persistence.rs", "crates/corvene-core/src/dispatcher.rs", "crates/corvene/src/main.rs", "crates/corvene-ui/src/banner.rs"],
    },

    /// Repositories whose remote is gone are marked and not fetched.
    DEAD_REMOTE_INDICATOR = 288 "dead-remote-indicator" {
        title: "Mark repositories whose remote is gone",
        summary: "When fetching a repository finds that its remote repository no longer exists \
                  (deleted, renamed or no longer accessible), its row in the repository list \
                  shows an alert icon with a tooltip saying so, and background fetches skip it \
                  until a fetch works again.",
        ghd_behaviour: "Background fetches keep failing silently and nothing in the list shows \
                        that the remote is gone.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(7521)],
        code: &["crates/corvene-core/src/remote.rs", "crates/corvene-ui/src/repository_list.rs"],
    },

    /// Add Local Repository offers the repositories inside a folder.
    ADD_REPOSITORIES_IN_FOLDER = 289 "add-repositories-in-folder" {
        title: "Add the repositories inside a folder",
        summary: "When Add Local Repository's path is a folder that is not a repository itself \
                  but holds some (up to two levels down), \"N repositories found inside. Add them \
                  all\" under the warning adds every one of them.",
        ghd_behaviour: "Only says the folder is not a Git repository and offers to create one \
                        there; each repository inside has to be added on its own.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20965)],
        code: &["crates/corvene-ui/src/dialogs/add_existing.rs", "crates/corvene-git/src/ops.rs"],
    },

    /// Repository list groups of the user's naming.
    CUSTOM_REPOSITORY_GROUPS = 290 "custom-repository-groups" {
        title: "Custom repository groups",
        summary: "The repository list's context menu has \"Move to Group…\", which puts the \
                  repository in a group you name (or one already in use) instead of its owner's \
                  group, and \"Remove from Group\". Custom groups are listed by name after Pinned \
                  and Recent, above the owner groups, and collapse like the others.",
        ghd_behaviour: "Repositories are grouped by GitHub owner or Enterprise host, then Other; \
                        the groups cannot be changed.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20328), Upstream::issue(21424)],
        code: &["crates/corvene-ui/src/repository_list.rs", "crates/corvene-ui/src/dialogs/move_repository_to_group.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// The Recent group lists the worktrees a repository was used in.
    RECENT_WORKTREES = 291 "recent-worktrees" {
        title: "Worktrees in the Recent group",
        summary: "A recently used repository that was open in several of its worktrees gets a \
                  row in the repository list's Recent group for each of them (up to three, most \
                  recent first), with the worktree's folder name dimmed after the name; picking \
                  one switches the repository to that worktree.",
        ghd_behaviour: "The Recent group lists each repository once and opens it in whichever \
                        worktree it was switched to last.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22376)],
        code: &["crates/corvene-ui/src/repository_list.rs", "crates/corvene-core/src/worktrees.rs", "crates/corvene-core/src/state.rs"],
    },

    /// The clone lists can show one owner's repositories.
    CLONE_OWNER_PICKER = 292 "clone-owner-picker" {
        title: "Clone: pick an owner",
        summary: "Clone a Repository's GitHub tabs and the \"Let's get started!\" page have an \
                  owner menu beside the filter box (All Owners, your account, each organization) \
                  that narrows the repository list to one owner's repositories; the choice is \
                  remembered per account.",
        ghd_behaviour: "Every repository the account can access is listed, grouped by owner; \
                        only the filter narrows it.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(18533)],
        code: &["crates/corvene-ui/src/cloneable_repositories.rs", "crates/corvene-ui/src/dialogs/clone_repository.rs", "crates/corvene-ui/src/no_repositories.rs"],
    },

    /// Clone several repositories from the clone lists at once.
    CLONE_MULTIPLE = 293 "clone-multiple" {
        title: "Clone several repositories at once",
        summary: "On Clone a Repository's GitHub tabs, ⌘-click adds repositories to the selection \
                  and ⇧-click picks a range. With more than one picked, the local path is a parent \
                  folder (each repository goes into its own folder in it, all checked before \
                  starting) and the repositories are cloned one after the other (\"Cloning 2 of \
                  5\"); each is added as it finishes, Cancel stops the rest, and failures are \
                  reported together.",
        ghd_behaviour: "One repository per clone.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(3849)],
        code: &["crates/corvene-ui/src/dialogs/clone_repository.rs", "crates/corvene-ui/src/cloneable_repositories.rs", "crates/corvene-core/src/dispatcher.rs", "crates/corvene-ui/src/cloning_view.rs"],
    },

    /// A repository whose folder was moved is followed to its new place.
    FOLLOW_MOVED_REPOSITORIES = 294 "follow-moved-repositories" {
        title: "Follow moved repositories",
        summary: "Corvene keeps a macOS bookmark of each repository's folder; when the folder is \
                  moved or renamed (on the same disk), the repository opens from its new place \
                  and a banner says where it is now, instead of showing \"Can't find\". A folder \
                  moved to the Trash or deleted is not followed.",
        ghd_behaviour: "The repository shows as missing; Locate… finds it again by hand.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: macos_only,
        upstream: &[Upstream::issue(19408)],
        code: &["crates/corvene-core/src/bookmarks.rs", "crates/corvene-platform/src/bookmarks.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// A running fetch, pull or push can be stopped.
    CANCEL_NETWORK_OPERATIONS = 295 "cancel-network-operations" {
        title: "Stop a fetch, pull or push",
        summary: "While a fetch, pull or push runs, the push/pull button has a Stop button in \
                  place of its ▾: git is stopped (with the processes it started) and the button \
                  says \"Cancelled\" for a moment. A push stopped while the server finishes it \
                  may still land; a pull can be stopped while it fetches, not once it merges \
                  or rebases.",
        ghd_behaviour: "The button is disabled until the operation ends, however long a stalled \
                        connection or a slow server hook takes.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(14095)],
        code: &["crates/corvene-core/src/remote.rs", "crates/corvene-git/src/process.rs", "crates/corvene-ui/src/toolbar.rs"],
    },

    /// Waking from sleep stops a hanging background fetch.
    CANCEL_FETCH_ON_WAKE = 296 "cancel-fetch-on-wake" {
        title: "Stop a stuck background fetch after sleep",
        summary: "When the computer wakes from sleep, a background fetch still running is \
                  stopped: its connection most likely died during sleep, and SSH or a stalled \
                  HTTPS transfer would otherwise leave \"Fetching origin\" spinning for good. \
                  The next background round fetches again; fetches you started are left \
                  alone.",
        ghd_behaviour: "The fetch can hang until the app is restarted, with the push/pull \
                        button stuck on \"Fetching origin\".",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: wake_events,
        upstream: &[Upstream::issue(19979)],
        code: &["crates/corvene-core/src/remote.rs", "crates/corvene/src/main.rs"],
    },

    /// Newer Commits on Remote offers to pull and push.
    PUSH_NEEDS_PULL_OFFERS_PULL = 297 "push-needs-pull-offers-pull" {
        title: "Pull and push when the remote moved on",
        summary: "The Newer Commits on Remote dialog (a push refused because the remote has \
                  commits the branch lacks) has a Pull and Push button next to Fetch: Corvene \
                  pulls, and pushes once the pull went through without conflicts. Conflicts \
                  open the usual conflicts flow and nothing is pushed.",
        ghd_behaviour: "Offers Fetch only; pulling and pushing again are two more clicks on the \
                        toolbar button.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(8090)],
        code: &["crates/corvene-ui/src/dialogs/remote_dialogs.rs", "crates/corvene-core/src/remote.rs"],
    },

    /// The hourly fetch is skipped while GitHub saw no push.
    BACKGROUND_FETCH_SKIPS_UNCHANGED = 298 "background-fetch-skips-unchanged" {
        title: "Skip background fetches with nothing new",
        summary: "Before the hourly background fetch of a GitHub repository (not a fork), \
                  Corvene asks GitHub when it was last pushed to and skips the fetch when that \
                  was before the last fetch. A real fetch still runs at least every six hours, \
                  as pull request refs, deleted branches and a fork's parent do not always \
                  show up there.",
        ghd_behaviour: "Runs git fetch every hour, also when nothing changed.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(1252)],
        code: &["crates/corvene-core/src/remote.rs"],
    },

    /// Repository › Pull All Repositories.
    PULL_ALL_REPOSITORIES = 299 "pull-all-repositories" {
        title: "Repository › Pull All Repositories",
        summary: "The Repository menu can pull every listed repository: each is fetched, and \
                  its checked-out branch is fast-forwarded when it is only behind its upstream \
                  and the working directory is clean. Nothing is merged or rebased, so nothing \
                  can conflict; repositories left as they were (diverged, with changes, in the \
                  middle of a merge, or failing) are listed in one dialog.",
        ghd_behaviour: "Repositories are pulled one at a time.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20525), Upstream::issue(21151)],
        code: &["crates/corvene-core/src/remote.rs", "crates/corvene-ui/src/app_menu.rs", "crates/corvene-git/src/remote_ops.rs"],
    },

    // ---- 300 GitHub ----

    /// The quick view's "opened … by author" line.
    PR_QUICK_VIEW_OPENED_BY = 301 "pr-quick-view-opened-by" {
        title: "Pull request quick view: \"opened by\" line",
        summary: "The pull request hover card shows the list item's \"opened N ago by author\" line \
                  next to the #N badge.",
        ghd_behaviour: "The card shows only the badge, the title and the body.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-ui/src/pull_request_list.rs"],
    },

    /// The quick view's width rule.
    PR_QUICK_VIEW_WIDTH = 302 "pr-quick-view-width" {
        title: "Pull request quick view width",
        summary: "The hover card's width: fixed at 400 px, or at least 400 px and growing with its \
                  content.",
        ghd_behaviour: "min-width: 400px.",
        nature: Nature::Feature,
        kind: Kind::Select { options: QUICK_VIEW_WIDTHS },
        corvene: Value::text("fixed-400"), ghd: Value::text("min-400"),
        familiar: Value::text("min-400"), max: Value::text("fixed-400"),
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-ui/src/pull_request_list.rs"],
    },

    /// Create Fork before a push git would refuse.
    FORK_BEFORE_PUSH = 303 "fork-before-push" {
        title: "Fork before pushing to a read-only repository",
        summary: "A push to a repository the account can only read opens the Create Fork dialog \
                  before git runs.",
        ghd_behaviour: "Runs the push and offers the fork after the authentication failure.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[],
        code: &["crates/corvene-core/src/remote.rs"],
    },

    /// Preview Pull Request's base survives Push Branch Commits.
    PUSH_BRANCH_COMMITS_KEEPS_BASE = 304 "push-branch-commits-keeps-base" {
        title: "Push Branch Commits keeps the chosen base",
        summary: "The base branch picked in Preview Pull Request survives the push that precedes \
                  the pull request.",
        ghd_behaviour: "Drops the chosen base and opens the compare page against the default branch.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[],
        code: &["crates/corvene-core/src/integrations.rs", "crates/corvene-ui/src/dialogs/push_branch_commits.rs"],
    },

    /// A failed Push Branch Commits keeps its error.
    PUSH_BRANCH_COMMITS_ERROR_STOPS = 305 "push-branch-commits-error-stops" {
        title: "Push Branch Commits stops on a failed push",
        summary: "A failed push leaves its error on screen.",
        ghd_behaviour: "Opens the compare page on GitHub anyway.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[],
        code: &["crates/corvene-core/src/integrations.rs", "crates/corvene-core/src/remote.rs"],
    },

    /// Dropping commits on a pull request explains why it could not start.
    CHERRY_PICK_PR_BRANCH_ERROR = 306 "cherry-pick-pr-branch-error" {
        title: "Cherry-pick onto a pull request explains failures",
        summary: "When commits are dropped on a pull request whose branch cannot be determined, \
                  the reason is shown.",
        ghd_behaviour: "Logs the reason and ends the operation silently.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[],
        code: &["crates/corvene-core/src/pull_requests.rs"],
    },

    /// Device flow or browser flow first.
    SIGN_IN_FLOW = 307 "sign-in-flow" {
        title: "Sign-in flow",
        summary: "How Sign in to GitHub.com starts: with a one-time code (device flow) or in the \
                  browser (web flow with PKCE, which GitHub refuses without a bundled client \
                  secret). Auto uses the browser on GitHub.com when the build has a client \
                  secret and the one-time code otherwise. The other flow stays one link away.",
        ghd_behaviour: "Browser flow only.",
        nature: Nature::Feature,
        kind: Kind::Select { options: SIGN_IN_FLOWS },
        corvene: Value::text("auto"), ghd: Value::text("browser"),
        familiar: Value::text("auto"), max: Value::text("auto"),
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(18749)],
        code: &["crates/corvene-ui/src/dialogs/sign_in.rs", "crates/corvene-ui/src/welcome.rs", "crates/corvene-core/src/flags/dispatch.rs"],
    },

    /// Check-run subscriptions idle out.
    CI_STATUS_IDLE_MINUTES = 308 "ci-status-idle-minutes" {
        title: "Check-run refresh idle timeout",
        summary: "A commit's check status stops refreshing this many minutes after nothing rendered \
                  it (0 keeps every status refreshing).",
        ghd_behaviour: "Subscribes on mount and unsubscribes on unmount, so nothing idles out.",
        nature: Nature::Feature,
        kind: Kind::Number { min: 0, max: 1440, unit: Some("min") },
        corvene: Value::Number(5), ghd: Value::Number(0),
        familiar: Value::Number(5), max: Value::Number(5),
        restart: false, visible: false, availability: available,
        upstream: &[],
        code: &["crates/corvene-core/src/commit_status.rs"],
    },

    /// Fork and pull-request remotes follow an SSH origin.
    FORK_REMOTES_KEEP_SSH = 309 "fork-remotes-keep-ssh" {
        title: "Fork remotes keep SSH",
        summary: "When the repository's remote is SSH, Create Fork's new origin, the upstream \
                  remote and the remote added to check out a pull request from a fork use SSH \
                  on the same host too.",
        ghd_behaviour: "Always uses the API's HTTPS clone URL, so SSH-only setups cannot fetch \
                        or push through those remotes.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(19074), Upstream::issue(9490)],
        code: &["crates/corvene-core/src/forks.rs", "crates/corvene-core/src/pull_requests.rs", "crates/corvene-ui/src/dialogs/fork_dialogs.rs"],
    },

    /// No fork offers where the owner disabled forking.
    FORK_OFFER_RESPECTS_ALLOW_FORKING = 310 "fork-offer-respects-allow-forking" {
        title: "No fork offer when forking is disabled",
        summary: "A read-only repository whose owner disabled forking gets no \"create a fork\" \
                  suggestion in the commit form and no Create Fork dialog around a push.",
        ghd_behaviour: "Offers the fork anyway; creating it then fails.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22156)],
        code: &["crates/corvene-core/src/forks.rs", "crates/corvene-core/src/remote.rs", "crates/corvene-ui/src/changes.rs"],
    },

    /// Publish errors name the failed validation.
    API_ERROR_DETAILS = 311 "api-error-details" {
        title: "Publish errors say what GitHub rejected",
        summary: "When publishing a repository fails, the error names every reason GitHub gave: \
                  a validation error without a message shows its field and code (\"name \
                  invalid\"), and publishing to an organization shows GitHub's error.",
        ghd_behaviour: "Lists only the reasons that have a message, leaving an empty item for \
                        the others (\"Repository creation failed. (…, )\"), and for an \
                        organization shows a hint to check its permissions instead.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(19465)],
        code: &["crates/corvene-core/src/remote.rs", "crates/corvene-github/src/api.rs"],
    },

    /// URL actions prefer the repository itself over a fork of it.
    EXACT_REPOSITORY_URL_FIRST = 312 "exact-repository-url-first" {
        title: "Open in Desktop prefers the repository over its forks",
        summary: "When an x-corvene://openRepo URL (Open with Desktop, a new branch from the web) \
                  names a repository that is added along with a fork of it, the repository \
                  itself is opened.",
        ghd_behaviour: "Opens whichever of them comes first in the list, often the fork.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(21379)],
        code: &["crates/corvene-core/src/app_url.rs"],
    },

    /// Wiki repositories are plain git repositories, not GitHub ones.
    WIKI_NOT_GITHUB = 313 "wiki-not-github" {
        title: "Wiki repositories are not treated as GitHub repositories",
        summary: "A repository whose origin is a GitHub wiki (owner/name.wiki) is handled as a \
                  plain git repository, so Corvene does not ask the API for its pull requests, \
                  issues, collaborators and checks, which do not exist. Applies at launch and \
                  when a repository is added.",
        ghd_behaviour: "Treats the wiki as a GitHub repository and every API request for it fails.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: true, visible: true, availability: available,
        upstream: &[Upstream::issue(2061)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-models/src/lib.rs"],
    },

    /// Plain-HTTP Enterprise servers.
    ENTERPRISE_PLAIN_HTTP = 314 "enterprise-plain-http" {
        title: "Allow plain-HTTP Enterprise servers",
        summary: "An Enterprise address typed with http:// stays on plain HTTP, for servers \
                  without TLS. The token then crosses the network unencrypted; an address \
                  without a scheme still uses HTTPS.",
        ghd_behaviour: "Refuses an http:// address with \"Unsupported protocol\" and connects \
                        over HTTPS only (plain HTTP was removed in 3.4.7).",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(20245)],
        code: &["crates/corvene-ui/src/dialogs/sign_in.rs", "crates/corvene-github/src/endpoint.rs"],
    },

    /// Pull Requests tab without an account.
    PULL_REQUESTS_SIGNED_OUT = 315 "pull-requests-signed-out" {
        title: "Pull Requests tab asks to sign in",
        summary: "Without an account for the repository's host, the empty Pull Requests tab \
                  says \"Sign in to see pull requests\" with a sign-in link, and its refresh \
                  button is disabled.",
        ghd_behaviour: "Shows \"You're all set!\" and a refresh button that does nothing.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22365), Upstream::issue(5354)],
        code: &["crates/corvene-ui/src/branch_list.rs", "crates/corvene-ui/src/pull_request_list.rs"],
    },

    /// Pull requests whose fork was deleted.
    PULL_REQUESTS_FROM_DELETED_FORKS = 316 "pull-requests-from-deleted-forks" {
        title: "Pull requests from deleted forks",
        summary: "Open pull requests whose head repository was deleted stay in the Pull \
                  Requests list and check out from the base repository's pull/N/head into pr/N.",
        ghd_behaviour: "Leaves them out of the list.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(14090)],
        code: &["crates/corvene-core/src/pull_requests.rs"],
    },

    /// Every page of a commit's check runs.
    ALL_CHECK_RUN_PAGES = 317 "all-check-run-pages" {
        title: "Read every page of check runs",
        summary: "A commit's check runs are read page by page until all of them are in (up to \
                  1,000), so the status and the checks list count every run.",
        ghd_behaviour: "Reads the first 100 check runs only.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(18101)],
        code: &["crates/corvene-core/src/commit_status.rs", "crates/corvene-core/src/alive.rs"],
    },

    /// No Re-run for read-only repositories.
    RERUN_NEEDS_PUSH_ACCESS = 318 "rerun-needs-push-access" {
        title: "Re-run checks needs push access",
        summary: "The check-run popover hides Re-run (and the per-job re-run) when the \
                  repository's permissions say the account can only read it.",
        ghd_behaviour: "Shows Re-run to everyone; for read-only accounts the request fails.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(14061)],
        code: &["crates/corvene-ui/src/ci_check_popover.rs"],
    },

    /// The check-run popover links to the pull request.
    CI_POPOVER_PULL_REQUEST_LINK = 319 "ci-popover-pull-request-link" {
        title: "Checks popover links to the pull request",
        summary: "The popover under the pull request badge ends its summary line with \
                  \"Open #N on GitHub\".",
        ghd_behaviour: "No way to open the pull request from the badge or its popover.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(15418)],
        code: &["crates/corvene-ui/src/ci_check_popover.rs"],
    },

    /// Full refresh of the `#` issue cache.
    ISSUES_FULL_REFRESH_HOURS = 320 "issues-full-refresh-hours" {
        title: "Issue suggestions: full refresh interval",
        summary: "Every this many hours the # issue suggestions fetch all open issues again, so \
                  deleted and transferred issues drop out (0 never does).",
        ghd_behaviour: "Only fetches issues updated since the newest cached one, so deleted or \
                        transferred issues are suggested forever.",
        nature: Nature::Feature,
        kind: Kind::Number { min: 0, max: 720, unit: Some("h") },
        corvene: Value::Number(24), ghd: Value::Number(0),
        familiar: Value::Number(24), max: Value::Number(24),
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(14124)],
        code: &["crates/corvene-core/src/autocomplete.rs"],
    },

    /// Repository › View Upstream on GitHub.
    VIEW_UPSTREAM_ON_GITHUB = 321 "view-upstream-on-github" {
        title: "Repository › View Upstream on GitHub",
        summary: "The Repository menu adds \"View Upstream on GitHub\", which opens a fork's \
                  parent repository.",
        ghd_behaviour: "Only View on GitHub (the fork itself).",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(13533)],
        code: &["crates/corvene-ui/src/app_menu.rs", "crates/corvene-core/src/integrations.rs"],
    },

    /// After sign-in, say when the Git email won't link commits.
    GIT_EMAIL_MISMATCH_BANNER = 322 "git-email-mismatch-banner" {
        title: "Git email check after signing in",
        summary: "Signing in to an account (outside the Welcome flow) checks the global Git \
                  email: when none is set, or it is not one of the account's addresses, a banner \
                  says commits won't be linked to the account and links to Settings › Git. \
                  Nothing is changed.",
        ghd_behaviour: "Says nothing at sign-in; only Settings › Git and the commit form warn \
                        about an email that doesn't match.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(14692)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-core/src/mco.rs", "crates/corvene-ui/src/banner.rs"],
    },

    /// A fork for its own work opens pull requests against itself.
    FORK_OWN_PR_TARGET = 323 "fork-own-pr-target" {
        title: "Own-purpose forks open pull requests on themselves",
        summary: "Create Pull Request in a fork set up \"for my own purposes\" opens GitHub's \
                  compare page with the fork's default branch (or the chosen base) as the base \
                  and the fork as both repositories.",
        ghd_behaviour: "Opens the fork's pull/new page with bare branch names, and GitHub \
                        proposes merging into the parent repository.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(15489)],
        code: &["crates/corvene-core/src/integrations.rs"],
    },

    /// Full refresh of the pull request cache.
    PULL_REQUESTS_FULL_REFRESH_HOURS = 324 "pull-requests-full-refresh-hours" {
        title: "Pull requests: full refresh interval",
        summary: "Every this many hours, and whenever the Pull Requests list's refresh button is \
                  clicked, all open pull requests are fetched again and replace the cached list, \
                  so deleted pull requests and those of removed or renamed repositories drop out \
                  (0 never does).",
        ghd_behaviour: "Only fetches pull requests updated since the newest cached one, so pull \
                        requests that disappeared stay in the list forever.",
        nature: Nature::BugFix,
        kind: Kind::Number { min: 0, max: 720, unit: Some("h") },
        corvene: Value::Number(24), ghd: Value::Number(0),
        familiar: Value::Number(24), max: Value::Number(24),
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(21124), Upstream::issue(21567)],
        code: &["crates/corvene-core/src/pull_requests.rs"],
    },

    /// Pull request and branch names match ignoring case as a fallback.
    PR_BRANCH_CASE_INSENSITIVE = 325 "pr-branch-case-insensitive" {
        title: "Match pull request branches ignoring case",
        summary: "When no branch matches a pull request's head branch exactly, a branch whose \
                  name differs only in case is used, so clicking the pull request switches to it \
                  and the current branch shows its pull request even after a case-insensitive \
                  file system folded the remote branch's name.",
        ghd_behaviour: "Compares names exactly, so such pull requests do not switch to their \
                        branch and are not shown as the current branch's.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(9463)],
        code: &["crates/corvene-core/src/pull_requests.rs"],
    },

    /// Preview Pull Request explains commits without file changes.
    PR_PREVIEW_EMPTY_FILES_MESSAGE = 326 "pr-preview-empty-files-message" {
        title: "Preview Pull Request says when there are no file changes",
        summary: "When the branch's commits add up to no file changes against the base branch, \
                  Preview Pull Request says \"No file changes between <base> and <branch>.\"",
        ghd_behaviour: "Shows an empty file list and a blank diff.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(17536)],
        code: &["crates/corvene-ui/src/dialogs/open_pull_request.rs"],
    },

    /// API errors that need SAML SSO say where to re-authorize.
    API_SAML_SSO_HINT = 327 "api-saml-sso-hint" {
        title: "Say when GitHub wants SSO re-authorization",
        summary: "When publishing a repository, creating a fork or creating the tutorial \
                  repository fails because an organization's SAML single sign-on authorization \
                  ran out, the error ends with \"Re-authorize SSO for <org>\" and the address \
                  GitHub gives for it.",
        ghd_behaviour: "Shows GitHub's message only, which does not say where to authorize.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(13668)],
        code: &["crates/corvene-github/src/api.rs", "crates/corvene-core/src/remote.rs", "crates/corvene-core/src/forks.rs", "crates/corvene-core/src/tutorial.rs"],
    },

    /// A repository GitHub no longer knows loses its GitHub association.
    CLEAR_LOST_GITHUB_ASSOCIATION = 328 "clear-lost-github-association" {
        title: "Forget GitHub repositories that are gone",
        summary: "When GitHub answers \"not found\" for a repository's details (it was deleted, \
                  or the account lost access), Corvene forgets that it is a GitHub repository and \
                  stops asking the API for its pull requests, issues and checks. Adding the \
                  repository again matches it anew.",
        ghd_behaviour: "Keeps the stale association forever, and every API request for the \
                        repository keeps failing.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(1144)],
        code: &["crates/corvene-core/src/forks.rs"],
    },

    /// Publish to an organization can grant a team access.
    PUBLISH_TEAM = 329 "publish-team" {
        title: "Publish: Team picker",
        summary: "Publishing a repository to an organization offers an optional Team picker \
                  listing the organization's teams; the picked team gets access to the new \
                  repository.",
        ghd_behaviour: "No team; access has to be granted on GitHub afterwards.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(826)],
        code: &["crates/corvene-ui/src/dialogs/remote_dialogs.rs", "crates/corvene-core/src/remote.rs", "crates/corvene-github/src/api.rs"],
    },

    /// Clone checks the local path before git runs.
    CLONE_PATH_VALIDATION = 330 "clone-path-validation" {
        title: "Clone checks the local path",
        summary: "The Clone dialog's local path expands a leading ~/ to the home folder, must be \
                  a full path, and a folder that does not exist yet must be creatable: Clone \
                  stays disabled with a reason when part of the path is a file or the location \
                  is not writable.",
        ghd_behaviour: "Only checks that the folder is empty; ~/ is taken literally and other bad \
                        paths fail inside git.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(13816)],
        code: &["crates/corvene-ui/src/dialogs/clone_repository.rs"],
    },

    /// github.com repositories without a GitHub.com account.
    GITHUB_WITHOUT_ACCOUNT = 331 "github-without-account" {
        title: "github.com repositories without an account",
        summary: "A repository whose origin is on github.com is a GitHub repository when it is \
                  added even without a GitHub.com account, so View on GitHub, the pull request \
                  links and the Pull Requests tab work.",
        ghd_behaviour: "Only the hosts of signed-in accounts count, so such a repository is a \
                        plain git repository.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-core/src/app_url.rs", "crates/corvene-models/src/lib.rs"],
    },

    /// The pull request compare URL names forks by owner only.
    PR_URL_OWNER_BRANCH_REFS = 332 "pr-url-owner-branch-refs" {
        title: "Compare URLs older GitHub Enterprise understands",
        summary: "Create Pull Request on a fork opens the compare page with owner:branch refs \
                  (octocat:main...me:feature) instead of owner:repository:branch, which older \
                  GitHub Enterprise Server versions do not understand. An owner has at most one \
                  fork in a network, so the short form names the same branches.",
        ghd_behaviour: "owner:repository:branch refs, for which older GitHub Enterprise Server \
                        compare pages say there is nothing to compare.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(16269)],
        code: &["crates/corvene-core/src/integrations.rs"],
    },

    /// Pull requests propose the branch the current one was created from as base.
    PR_BASE_FROM_BRANCH_ORIGIN = 333 "pr-base-from-branch-origin" {
        title: "Pull request base from where the branch started",
        summary: "Create Pull Request and Preview Pull Request propose as base the branch the \
                  current branch was created from (the branch recorded by \
                  update-from-parent-branch or VS Code, else the \"Created from\" entry of the \
                  branch's reflog) when it is another branch than the default one and exists on \
                  the remote; otherwise the default branch as before.",
        ghd_behaviour: "The default branch is always proposed, so a pull request for a branch \
                        stacked on another one targets the wrong branch unless changed by hand.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20580)],
        code: &["crates/corvene-core/src/integrations.rs", "crates/corvene-core/src/pull_request_preview.rs", "crates/corvene-git/src/branch_ops.rs"],
    },

    /// The current branch's checks beside its name when it has no pull request.
    BRANCH_CI_STATUS = 334 "branch-ci-status" {
        title: "CI status of branches without a pull request",
        summary: "When the current branch is pushed to a GitHub repository and has no pull \
                  request, the branch button shows the status of the checks that ran on its \
                  pushed commit (statuses and check runs) where the pull request badge would \
                  be; clicking it opens the check-run popover, with re-run.",
        ghd_behaviour: "Checks show only on the pull request badge, so a branch's CI status needs \
                        a pull request or the browser.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(10115)],
        code: &["crates/corvene-core/src/commit_status.rs", "crates/corvene-ui/src/toolbar.rs", "crates/corvene-ui/src/ci_check_popover.rs"],
    },

    /// Filters for the pull request list.
    PULL_REQUEST_LIST_FILTERS = 335 "pull-request-list-filters" {
        title: "Pull request list filters",
        summary: "A menu beside the Pull Requests filter box narrows the list to the pull \
                  requests created by you, those asking you for a review, or those assigned to \
                  you (requests to a team you are in are not counted). The choice is kept until \
                  Corvene quits.",
        ghd_behaviour: "The list can only be filtered by text.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(10966)],
        code: &["crates/corvene-ui/src/branch_list.rs", "crates/corvene-ui/src/pull_request_list.rs", "crates/corvene-core/src/pull_requests.rs", "crates/corvene-github/src/api.rs"],
    },

    /// Ask collaborators for a review of the current branch's pull request.
    REQUEST_REVIEWERS = 336 "request-reviewers" {
        title: "Request reviewers",
        summary: "Branch › Request Reviewers… (and the pull request badge's context menu) opens a \
                  filterable list of the repository's collaborators, the pull request's author \
                  left out and those already asked ticked; applying asks the newly ticked ones \
                  for a review and withdraws the request from the unticked ones. GitHub's refusal \
                  is shown as it comes.",
        ghd_behaviour: "Reviewers can only be requested on GitHub.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(10146)],
        code: &["crates/corvene-ui/src/dialogs/request_reviewers.rs", "crates/corvene-core/src/pull_requests.rs", "crates/corvene-github/src/api.rs", "crates/corvene-ui/src/app_menu.rs", "crates/corvene-ui/src/toolbar.rs"],
    },

    /// Pull request notifications for every listed repository.
    NOTIFICATIONS_ALL_REPOSITORIES = 337 "notifications-all-repositories" {
        title: "Notifications for every repository",
        summary: "Pull request notifications (reviews, comments, failed checks) are shown for \
                  every repository in the list, not only the selected one; the pull request is \
                  looked up on GitHub when that repository's list is not loaded. Clicking one \
                  switches to its repository.",
        ghd_behaviour: "Only events of the selected repository's pull requests become \
                        notifications.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(14688), Upstream::issue(19139)],
        code: &["crates/corvene-core/src/alive.rs", "crates/corvene-core/src/notifications.rs"],
    },

    /// A ruleset the user is exempt from does not block commits.
    RULESET_EXEMPT_BYPASS = 338 "ruleset-exempt-bypass" {
        title: "Respect ruleset exemptions",
        summary: "A repository ruleset whose bypass list makes you exempt (GitHub's \"Exempt\" \
                  bypass mode) is left out of the commit form's rule checks: no warning and no \
                  disabled Commit button for rules GitHub never applies to your pushes.",
        ghd_behaviour: "Only \"Always allow\" counts as a bypass; an exempt user gets the \
                        blocking \"will prevent pushing\" warning and cannot commit.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21159), Upstream::issue(21347)],
        code: &["crates/corvene-core/src/repo_rules.rs", "crates/corvene-github/src/api.rs"],
    },

    /// A protected branch that takes your pushes gets a note.
    PROTECTED_BRANCH_BYPASS_NOTE = 339 "protected-branch-bypass-note" {
        title: "Note when pushing past branch protection",
        summary: "When the current branch is protected on GitHub but still takes your direct \
                  pushes (you are an admin or on the bypass list, or its pull request rule \
                  needs no approvals), a note above the Commit button says so: \"main is a \
                  protected branch. Your push may bypass its rules.\" Committing stays \
                  possible.",
        ghd_behaviour: "Warns only about branches you cannot push to; a push that bypasses \
                        the protection goes through without a word.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20522), Upstream::issue(20547)],
        code: &["crates/corvene-core/src/repo_rules.rs", "crates/corvene-github/src/api.rs", "crates/corvene-ui/src/changes.rs"],
    },

    /// Commit message rules leave it to a commit message hook.
    MESSAGE_RULES_DEFER_TO_HOOKS = 340 "message-rules-defer-to-hooks" {
        title: "Commit message rules defer to hooks",
        summary: "When the repository has a prepare-commit-msg or commit-msg hook (core.hooksPath \
                  counts) and Bypass Commit Hooks is off, a summary that fails a commit message \
                  rule only warns: the hook may add the ticket number or prefix the rule asks \
                  for, so the Commit button stays enabled. GitHub still checks the pushed \
                  commits.",
        ghd_behaviour: "A failing commit message rule disables the Commit button, even when a \
                        hook would rewrite the message to pass it.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21328)],
        code: &["crates/corvene-ui/src/changes.rs", "crates/corvene-git/src/commit.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// GitHub autolinks and the repository's own ones link in commit messages.
    CUSTOM_AUTOLINKS = 341 "custom-autolinks" {
        title: "Autolinks in commit messages",
        summary: "References like TICKET-123 in commit messages link to their tracker: the \
                  GitHub repository's autolinks (read when the signed-in account is an admin of \
                  it) and the ones added in Repository Settings › Autolinks (a prefix and a URL \
                  with <num>), in GitHub and other repositories alike.",
        ghd_behaviour: "Only #123, @mentions and URLs are links; a repository's autolink \
                        references stay plain text.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(11417)],
        code: &["crates/corvene-core/src/text_tokens.rs", "crates/corvene-core/src/forks.rs", "crates/corvene-ui/src/dialogs/repository_settings.rs"],
    },

    /// GitLab accounts, merge requests and pipelines.
    GITLAB = 342 "gitlab" {
        title: "GitLab",
        summary: "Sign in to GitLab.com or a self-managed GitLab (browser or personal access \
                  token) in Settings › Accounts. A repository on GitLab gets the Merge Requests \
                  tab, Create Merge Request, the current branch's merge request with its pipeline \
                  in the toolbar and the checks popover, View on GitLab, and HTTPS credentials \
                  for git from the account. Public gitlab.com projects work without signing in.",
        ghd_behaviour: "Only GitHub.com and GitHub Enterprise; a GitLab repository is a plain \
                        repository and git asks for its credentials.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(7875), Upstream::issue(18588)],
        code: &["crates/corvene-hosts/src/gitlab.rs", "crates/corvene-core/src/hosts.rs", "crates/corvene-ui/src/dialogs/sign_in_host.rs"],
    },

    /// Gitea, Forgejo and Codeberg accounts, pull requests and statuses.
    GITEA = 343 "gitea" {
        title: "Gitea, Forgejo and Codeberg",
        summary: "Sign in to Codeberg or a self-hosted Gitea or Forgejo (browser or access \
                  token) in Settings › Accounts. A repository there gets the Pull Requests tab, \
                  Create Pull Request, the current branch's pull request with its commit \
                  statuses (Actions too) in the toolbar and the checks popover, View on the \
                  host, and HTTPS credentials for git. Public Codeberg repositories work without \
                  signing in.",
        ghd_behaviour: "Only GitHub.com and GitHub Enterprise.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-hosts/src/gitea.rs", "crates/corvene-core/src/hosts.rs", "crates/corvene-ui/src/dialogs/sign_in_host.rs"],
    },

    /// Bitbucket Cloud accounts, pull requests and build statuses.
    BITBUCKET = 344 "bitbucket" {
        title: "Bitbucket",
        summary: "Sign in to Bitbucket Cloud with an Atlassian API token in Settings › \
                  Accounts. A repository on bitbucket.org gets the Pull Requests tab, Create Pull \
                  Request, the current branch's pull request with its build statuses \
                  (Pipelines too) in the toolbar and the checks popover, View on Bitbucket, and \
                  HTTPS credentials for git. Public repositories work without signing in.",
        ghd_behaviour: "Only GitHub.com and GitHub Enterprise; git asks for Bitbucket's \
                        credentials and app passwords no longer work.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(14052)],
        code: &["crates/corvene-hosts/src/bitbucket.rs", "crates/corvene-core/src/hosts.rs", "crates/corvene-ui/src/dialogs/sign_in_host.rs"],
    },

    /// Repository › Issues…: the repository's issues in the app.
    ISSUES = 345 "issues" {
        title: "Issues",
        summary: "Repository › Issues… lists the GitHub repository's issues in place of History \
                  (open, closed or assigned to me, with their labels and a filter), shows the \
                  selected one with its description, and offers New Issue… (title, description, \
                  labels and assignees, created through the API) and Create Branch from the \
                  issue: the branch is named <number>-<title> and linked to the issue on GitHub \
                  once it is pushed (its description keeps the issue's URL meanwhile and Create \
                  Pull Request prefills \"Closes #<number>\").",
        ghd_behaviour: "Repository › Create Issue on GitHub opens the browser's new-issue form; \
                        issues only appear as # suggestions in the commit message.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-core/src/issues.rs", "crates/corvene-ui/src/issues_list.rs", "crates/corvene-ui/src/issue_view.rs", "crates/corvene-ui/src/dialogs/new_issue.rs", "crates/corvene-github/src/api.rs"],
    },

    /// Repository › Releases… and Create Release… from a tag.
    RELEASES = 346 "releases" {
        title: "Releases",
        summary: "Repository › Releases… lists the GitHub repository's releases in place of \
                  History (draft, pre-release and latest marked, with notes and assets), and \
                  Create Release… (from a tagged commit's menu in History, a tag in the branch \
                  list or the Releases view) publishes one: an existing or new tag, a title, \
                  notes generated by GitHub (from the local history on a GitHub Enterprise \
                  Server that cannot) and then edited, as a draft or pre-release. A tag not on \
                  GitHub yet is pushed first.",
        ghd_behaviour: "No releases: they are made on github.com.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(6648)],
        code: &["crates/corvene-core/src/releases.rs", "crates/corvene-ui/src/releases_list.rs", "crates/corvene-ui/src/release_view.rs", "crates/corvene-ui/src/dialogs/create_release.rs", "crates/corvene-github/src/api.rs"],
    },

    /// The check-run popover opens an Actions job's log in the app.
    ACTIONS_JOB_LOGS = 347 "actions-job-logs" {
        title: "Actions job logs",
        summary: "In the check-run popover a GitHub Actions job's steps open the job's log in a \
                  dialog (View log, or a click on a step): the log's text with its timestamps \
                  and colour codes removed, errors and groups marked, a search box, Jump to \
                  failure, Copy, and Open on GitHub for the job's page.",
        ghd_behaviour: "Every step links to its page on GitHub; logs are read in the browser.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-core/src/job_log.rs", "crates/corvene-ui/src/dialogs/actions_job_log.rs", "crates/corvene-ui/src/ci_check_popover.rs", "crates/corvene-github/src/api.rs"],
    },

    /// Review Pull Request: the pull request's files, overview and
    /// review threads in the app, with replies, resolving and a review to
    /// submit.
    PULL_REQUEST_REVIEW = 348 "pull-request-review" {
        title: "Pull request review",
        summary: "Branch › Review Pull Request… (or Review Pull Request in a row of the Pull \
                  Requests tab) shows the pull request in place of History: its changed files \
                  with their thread counts, an overview pane (description, reviewers, checks, \
                  labels and the timeline) and each file's diff with the review threads under \
                  the lines they belong to. Threads can be replied to, resolved and unresolved; \
                  a + button on a line (shift-click for a range) writes a comment, posted at once \
                  or into a pending review; Review changes… submits the pending review as a \
                  comment, an approval or a request for changes. The diff is the local branch \
                  tip when the pull request's branch is checked out, else GitHub's head (fetched \
                  when missing).",
        ghd_behaviour: "A pull request can only be checked out, previewed before it exists and \
                        opened in the browser; reviews happen on github.com.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20614)],
        code: &["crates/corvene-core/src/pull_request_review.rs", "crates/corvene-ui/src/pull_request_review_list.rs", "crates/corvene-ui/src/pull_request_overview.rs", "crates/corvene-ui/src/review_threads.rs", "crates/corvene-ui/src/dialogs/submit_pull_request_review.rs", "crates/corvene-github/src/review.rs"],
    },

    /// Review threads follow the lines across force-pushes and local
    /// commits.
    REVIEW_THREAD_REMAPPING = 349 "review-thread-remapping" {
        title: "Review threads follow the lines",
        summary: "When the diff shown is not the head GitHub has (local commits not pushed \
                  yet, or a branch behind a force-push), review threads are carried to the \
                  lines they belong to with a line map of the two versions of the file, and a \
                  thread github.com lists as outdated is placed again when the lines it was \
                  written on are still there. A new comment's line is carried back the same way.",
        ghd_behaviour: "Threads are shown only at the lines GitHub reports, and only while the \
                        diff shown is GitHub's head; the rest are listed as outdated.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-core/src/review_anchor.rs", "crates/corvene-core/src/pull_request_review.rs"],
    },

    /// Settings › Integrations creates an SSH key and adds it to GitHub.
    SSH_KEY_HELPER = 350 "ssh-key-helper" {
        title: "SSH key helper",
        summary: "Settings › Integrations shows the SSH key in ~/.ssh, or creates one (ed25519 \
                  with ssh-keygen, an optional passphrase), adds it to ssh-agent (on macOS with \
                  --apple-use-keychain, which keeps the passphrase in the keychain) and adds \
                  it to the GitHub account through the API. A sign-in that does not allow \
                  adding keys asks to sign in again for the write:public_key permission. An \
                  existing key is never replaced.",
        ghd_behaviour: "No help with SSH keys: they are made in a terminal and added on \
                        github.com.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(2579)],
        code: &["crates/corvene-platform/src/ssh_key.rs", "crates/corvene-core/src/ssh_keys.rs", "crates/corvene-ui/src/dialogs/preferences.rs", "crates/corvene-ui/src/dialogs/create_ssh_key.rs", "crates/corvene-github/src/api.rs"],
    },

    /// The GitHub repository's Actions: workflows, runs, jobs and logs,
    /// re-run, cancel and Run workflow, for cloned repositories and those
    /// in the clone list.
    ACTIONS = 351 "actions" {
        title: "GitHub Actions",
        summary: "Repository › Actions… (and View Actions… on a repository in the clone \
                  dialog's lists, so it works without cloning) shows the repository's \
                  workflows, their runs filtered by branch, status and event, and a run's \
                  jobs and steps, whose logs open in the app. Runs can be re-run (all or failed \
                  jobs), cancelled or have their logs deleted, a job re-run alone, and Run \
                  workflow starts a workflow_dispatch workflow on a branch or tag with a form \
                  built from the workflow file's inputs. The open view refreshes every 12 s \
                  while a run is queued or running, and a run started here posts a \
                  notification when it finishes.",
        ghd_behaviour: "Actions are only read to re-run a pull request's failed checks; \
                        everything else happens on github.com.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(17498)],
        code: &["crates/corvene-core/src/github_actions.rs", "crates/corvene-github/src/actions.rs", "crates/corvene-ui/src/dialogs/actions_view.rs", "crates/corvene-ui/src/dialogs/run_workflow.rs", "crates/corvene-ui/src/cloneable_repositories.rs"],
    },

    /// Preview Pull Request's base remembered per repository.
    REMEMBER_PR_BASE = 352 "remember-pr-base" {
        title: "Remember the pull request base",
        summary: "The base branch picked in Preview Pull Request is remembered for the \
                  repository. Create Pull Request and Preview Pull Request then propose it again \
                  (after the branch the current branch was created from, when that is known) \
                  while it still exists on the remote. Picking the default branch forgets it.",
        ghd_behaviour: "The default branch is proposed every time, so pull requests against a \
                        long-lived development branch need the base changed by hand each time.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(15806)],
        code: &["crates/corvene-core/src/integrations.rs", "crates/corvene-core/src/pull_request_preview.rs", "crates/corvene-ui/src/dialogs/open_pull_request.rs"],
    },

    /// A fork contributing to its parent follows the parent's default branch.
    FORK_TRACKS_UPSTREAM = 353 "fork-tracks-upstream" {
        title: "Fork's default branch follows the parent",
        summary: "When a fork is set up to contribute to its parent (Choose Fork Settings, \
                  Repository Settings, or a cloned fork getting its upstream remote), the parent \
                  is fetched and the local default branch tracks the parent's default branch, \
                  so Pull brings the parent's new commits without syncing the fork first. Its \
                  pushes still go to the fork (branch.<name>.pushRemote), and in such a fork a \
                  force push also goes to a branch's push remote. Only a default branch with no \
                  commits the parent lacks is switched; \"For my own purposes\" switches it \
                  back. Needs the remote manager.",
        ghd_behaviour: "The default branch keeps tracking the fork, which has to be updated from \
                        the parent and pushed before new branches start from the parent's latest \
                        commits.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(19457)],
        code: &["crates/corvene-core/src/forks.rs", "crates/corvene-core/src/remote.rs"],
    },

    /// Notifications for review requests, reviews, merges and mentions on
    /// any repository.
    PULL_REQUEST_EVENT_NOTIFICATIONS = 354 "pull-request-event-notifications" {
        title: "Pull request event notifications",
        summary: "Each signed-in account is polled for pull request events on any repository \
                  it can see (GitHub's notifications API, or the issue search when the token is \
                  refused it): a review asked of you or your team, an approval or changes \
                  requested on your pull request, your pull request merged by someone else, and \
                  a mention of you or your team. Settings › Notifications has a checkbox per \
                  event and a choice of what a click opens (the repository in Corvene when it \
                  is listed, or the pull request on GitHub). Works on GitHub Enterprise too.",
        ghd_behaviour: "Only Alive's reviews, comments and failed checks of your own pull \
                        requests in the selected repository are notified, and only on GitHub.com.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21474)],
        code: &["crates/corvene-core/src/pull_request_events.rs", "crates/corvene-github/src/pull_request_events.rs", "crates/corvene-core/src/notifications.rs", "crates/corvene-ui/src/dialogs/preferences.rs"],
    },

    /// Links to a cloned repository's releases, comparisons, commits and
    /// issues offer to open in the app.
    GITHUB_LINKS_IN_APP = 355 "github-links-in-app" {
        title: "Open GitHub links in the app",
        summary: "A click on a link in release notes, an issue or a pull request description \
                  that points at a release, a comparison, a commit or an issue of a repository \
                  in the list asks whether to show it in Corvene (the release in Releases, the \
                  comparison or the commit in History, the issue in Issues) or on GitHub.",
        ghd_behaviour: "Every link opens in the browser.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-core/src/github_links.rs", "crates/corvene-ui/src/markdown.rs"],
    },

    // ---- 400 Window & menus ----

    /// Help › Show Release Notes.
    RELEASE_NOTES_MENU_ITEM = 401 "release-notes-menu-item" {
        title: "Help › Show Release Notes",
        summary: "The Help menu can open the release notes of the running version at any time.",
        ghd_behaviour: "Shows release notes only right after an update.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-ui/src/app_menu.rs"],
    },

    /// About's architecture suffix and Source code link.
    ABOUT_EXTRAS = 402 "about-extras" {
        title: "About: architecture and source link",
        summary: "The About dialog shows the CPU architecture after the version and a Source code link.",
        ghd_behaviour: "The version only, and a Terms and Conditions link.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-ui/src/dialogs/app_dialogs.rs"],
    },

    /// The Move to Applications prompt's backdrop.
    MOVE_TO_APPLICATIONS_BACKDROP_DISMISS = 403 "move-to-applications-backdrop-dismiss" {
        title: "Move to Applications prompt closes on backdrop click",
        summary: "Clicking outside the \"Move to the Applications folder?\" prompt dismisses it.",
        ghd_behaviour: "The prompt only closes through its buttons (backdropDismissable=false).",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-ui/src/dialog.rs", "crates/corvene-ui/src/dialogs/move_to_applications_folder.rs"],
    },

    /// When a resized toolbar button's width is persisted.
    TOOLBAR_WIDTH_SAVE = 404 "toolbar-width-save" {
        title: "Toolbar button width is saved",
        summary: "When a resized toolbar button's width is written to the store.",
        ghd_behaviour: "localStorage on every pointer move.",
        nature: Nature::BugFix,
        kind: Kind::Select { options: WIDTH_SAVES },
        corvene: Value::text("drag-end"), ghd: Value::text("every-move"),
        familiar: Value::text("drag-end"), max: Value::text("drag-end"),
        restart: false, visible: false, availability: available,
        upstream: &[],
        code: &["crates/corvene-ui/src/toolbar.rs"],
    },

    /// Window › Corvene shows the hidden main window.
    WINDOW_MENU_MAIN_WINDOW = 405 "window-menu-main-window" {
        title: "Window menu lists the main window",
        summary: "The Window menu ends with \"Corvene\", which shows the main window again after \
                  ⌘W or the close button hid it.",
        ghd_behaviour: "The closed window is not listed; only the Dock icon brings it back.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(17647)],
        code: &["crates/corvene-ui/src/app_menu.rs", "crates/corvene/src/main.rs"],
    },

    /// `--hidden` launches with the window hidden.
    LAUNCH_HIDDEN = 406 "launch-hidden" {
        title: "Launch hidden with --hidden",
        summary: "Started with `--hidden` (`open -a Corvene --args --hidden`, e.g. from a login \
                  script), Corvene keeps its window hidden until the Dock icon or Window menu \
                  brings it back.",
        ghd_behaviour: "Always shows the window at launch.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(18925)],
        code: &["crates/corvene/src/main.rs"],
    },

    /// Smaller minimum window and sidebar sizes.
    SMALLER_MINIMUM_SIZES = 407 "smaller-minimum-sizes" {
        title: "Smaller minimum window and sidebar",
        summary: "The window can shrink to 600 × 400 and the repository sidebar to 120 px, for tiled \
                  and side-by-side layouts (the toolbar and lists clip below GitHub Desktop's sizes).",
        ghd_behaviour: "At least 960 × 660 for the window and 220 px for the sidebar.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: true, visible: true, availability: available,
        upstream: &[Upstream::issue(14286), Upstream::issue(22492), Upstream::issue(21368)],
        code: &["crates/corvene/src/main.rs", "crates/corvene-ui/src/workspace.rs"],
    },

    /// Open in editor / shell buttons in the toolbar.
    TOOLBAR_OPEN_BUTTONS = 408 "toolbar-open-buttons" {
        title: "Toolbar: Open in editor and shell buttons",
        summary: "Two icon buttons after Push / Pull open the repository in the external editor \
                  and in the shell.",
        ghd_behaviour: "Only the Repository menu, its shortcuts and the no-changes suggestions.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21171)],
        code: &["crates/corvene-ui/src/toolbar.rs"],
    },

    /// The repository button names the GitHub owner.
    OWNER_IN_REPOSITORY_BUTTON = 409 "owner-in-repository-button" {
        title: "Owner in the repository button",
        summary: "For a GitHub repository the Current Repository toolbar button's small line \
                  shows the owner (user or organization) instead of \"Current Repository\", so \
                  forks with the same name are told apart.",
        ghd_behaviour: "Always \"Current Repository\".",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(17252)],
        code: &["crates/corvene-ui/src/toolbar.rs"],
    },

    /// An aliased repository's name is italic in the toolbar too.
    ALIAS_ITALIC_IN_TOOLBAR = 410 "alias-italic-in-toolbar" {
        title: "Aliases are italic in the toolbar",
        summary: "The Current Repository button shows an aliased repository's name in italics, \
                  as the repository list does.",
        ghd_behaviour: "Italic in the repository list, upright in the toolbar.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(17770)],
        code: &["crates/corvene-ui/src/toolbar.rs"],
    },

    /// The push / pull progress tooltip keeps one width.
    STEADY_PROGRESS_TOOLTIP = 411 "steady-progress-tooltip" {
        title: "Steady push / pull progress tooltip",
        summary: "While a push, pull or fetch runs, the push / pull button's progress tooltip is \
                  always 300 px wide instead of resizing with every progress line.",
        ghd_behaviour: "The tooltip fits its text, so it jumps in size as the progress text changes.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(17429)],
        code: &["crates/corvene-ui/src/toolbar.rs", "crates/corvene-ui/src/widgets.rs"],
    },

    /// The branch button shows a running merge.
    MERGE_PROGRESS_IN_BRANCH_BUTTON = 412 "merge-progress-in-branch-button" {
        title: "Branch button shows a running merge",
        summary: "While a merge runs (Merge into…, Update from Default Branch) the toolbar's \
                  branch button spins and reads \"Merging <branch>\", as it does while \
                  switching branches.",
        ghd_behaviour: "The merge dialog closes and nothing shows until the merge finishes.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(6120), Upstream::issue(15996)],
        code: &["crates/corvene-ui/src/toolbar.rs"],
    },

    /// "Copy" button on error dialogs.
    ERROR_DIALOG_COPY = 413 "error-dialog-copy" {
        title: "Copy button on error dialogs",
        summary: "Error dialogs (a failed commit, push, checkout…) have a \"Copy\" button that puts \
                  the title and message on the clipboard; the text itself cannot be selected.",
        ghd_behaviour: "No way to copy the message (⌘C does nothing).",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(19198), Upstream::issue(22591), Upstream::issue(22913)],
        code: &["crates/corvene-ui/src/dialogs/simple.rs"],
    },

    /// File › Install command line tool… on Linux.
    LINUX_INSTALL_CLI = 414 "linux-install-cli" {
        title: "Install command line tool on Linux",
        summary: "File › Install command line tool… links the `corvene` command (`corvene open`, \
                  `corvene clone`) into ~/.local/bin, as the macOS app menu item does.",
        ghd_behaviour: "The command line tool can only be installed on macOS; Linux packages \
                        ship it on their own or not at all.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: linux_only,
        upstream: &[],
        code: &["crates/corvene-ui/src/app_menu.rs", "crates/corvene-platform/src/cli.rs"],
    },

    /// Git error dialogs show the command, its exit code and output.
    GIT_ERROR_DIALOG = 415 "git-error-dialog" {
        title: "Structured git error dialogs",
        summary: "When a git command fails, the error dialog leads with a plain sentence (GitHub \
                  Desktop's description where it has one, else git's first error line), lists \
                  what git named (files that would be overwritten or conflict, rejected refs, \
                  the server's messages, hints, the path or URL at fault), and keeps git's raw \
                  output in a monospace box under the command that ran (without the -c options \
                  Corvene adds) and its exit code, collapsed while the details stand in for it.",
        ghd_behaviour: "Shows its description alone for the errors it recognises; otherwise \
                        git's raw output in monospace, with no command or exit code.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-ui/src/dialogs/simple.rs", "crates/corvene-git/src/git_errors.rs"],
    },

    /// Open-repository URLs and the CLI can leave Corvene in the background.
    URL_BACKGROUND_OPEN = 416 "url-background-open" {
        title: "Open repositories in the background",
        summary: "An x-corvene://openRepo or openLocalRepo URL with ?background=1 (the command \
                  line tool's corvene --background …) selects the repository without bringing \
                  the window forward, so scripts and editor integrations can switch repositories \
                  quietly.",
        ghd_behaviour: "Every URL action and CLI command activates the app and shows its window.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(22150)],
        code: &["crates/corvene-core/src/app_url.rs", "packaging/corvene.sh", "packaging/linux/corvene.sh"],
    },

    /// The command line tool adds a repository without the dialog.
    CLI_ADD_REPOSITORY = 417 "cli-add-repository" {
        title: "Add repositories from the command line",
        summary: "corvene add [path] (macOS and Linux command line tool) adds the repository \
                  containing the path without the Add Local Repository dialog, or says it is not \
                  a Git repository. The request carries a token only the user's own shell can \
                  create, so x-corvene:// links from elsewhere still ask first.",
        ghd_behaviour: "The command line tool can only open a path; one Corvene doesn't list yet \
                        shows the Add Local Repository dialog.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(21260)],
        code: &[
            "crates/corvene-core/src/app_url.rs",
            "crates/corvene-platform/src/cli.rs",
            "packaging/corvene.sh",
            "packaging/linux/corvene.sh",
        ],
    },

    /// Quit asks first while work is running.
    CONFIRM_QUIT_WHILE_BUSY = 418 "confirm-quit-while-busy" {
        title: "Confirm quitting while busy",
        summary: "Quit (⌘Q or the menu) while a repository is being cloned, a push, pull or \
                  fetch runs, or an update downloads or installs asks \"Quit anyway?\" first. \
                  Quit again while it asks to quit at once.",
        ghd_behaviour: "Quits immediately and cuts the running operation short.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(12807), Upstream::issue(10559), Upstream::issue(18205)],
        code: &[
            "crates/corvene/src/main.rs",
            "crates/corvene-core/src/state.rs",
            "crates/corvene-ui/src/dialogs/confirm_quit.rs",
        ],
    },

    /// More ways to zoom.
    EXTRA_ZOOM_INPUTS = 419 "extra-zoom-inputs" {
        title: "More zoom shortcuts",
        summary: "⌘+ (⇧⌘=) and the keypad's + zoom in as ⌘= does, and ⌘ + mouse wheel (Ctrl \
                  on Windows and Linux) zooms in and out.",
        ghd_behaviour: "Only ⌘=, ⌘- and ⌘0 zoom.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(3913)],
        code: &["crates/corvene-ui/src/keymap.rs", "crates/corvene-ui/src/workspace.rs"],
    },

    /// The minimum window size never exceeds the screen.
    MIN_SIZE_FITS_DISPLAY = 420 "min-size-fits-display" {
        title: "Minimum window size fits the screen",
        summary: "On a display smaller than the minimum window size the minimum shrinks to the \
                  screen's usable area, so the window can still be resized and moved.",
        ghd_behaviour: "The 960 × 660 minimum holds even on smaller screens, which forces a \
                        window that covers or overflows the whole display.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: true, visible: true, availability: available,
        upstream: &[Upstream::issue(17669)],
        code: &["crates/corvene/src/main.rs"],
    },

    /// The release notes and conflicts dialogs grow with the window.
    LARGER_DIALOGS = 421 "larger-dialogs" {
        title: "Larger release notes and conflicts dialogs",
        summary: "The release notes and \"Resolve conflicts\" dialogs take up to 80 % of the \
                  window (at most 960 px wide), so long notes and many conflicted files need \
                  less scrolling.",
        ghd_behaviour: "Release notes are at most 800 × 500 px and the conflicts dialog 500 px \
                        wide with a 285 px file list, whatever the window size.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(6851)],
        code: &[
            "crates/corvene-ui/src/dialog.rs",
            "crates/corvene-ui/src/dialogs/release_notes.rs",
            "crates/corvene-ui/src/dialogs/mco_dialogs.rs",
        ],
    },

    /// Banners float over the content.
    BANNER_AS_TOAST = 422 "banner-as-toast" {
        title: "Banners as toasts",
        summary: "Success, conflict and update banners appear as a card in the bottom-right \
                  corner over the content instead of a strip under the toolbar, so the lists \
                  and the diff don't move when one appears or goes away.",
        ghd_behaviour: "A 30 px strip under the toolbar pushes everything down.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21433)],
        code: &["crates/corvene-ui/src/banner.rs", "crates/corvene-ui/src/workspace.rs"],
    },

    /// Edit › Undo Last Commit.
    UNDO_COMMIT_MENU_ITEM = 423 "undo-commit-menu-item" {
        title: "Edit › Undo Last Commit",
        summary: "The Edit menu has \"Undo Last Commit\", which does what the Undo button under \
                  the commit form does and is available exactly while that button shows.",
        ghd_behaviour: "Only the Undo button under the commit form and History's context menu.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(5725)],
        code: &["crates/corvene/src/menus.rs", "crates/corvene/src/main.rs"],
    },

    /// Open items stay enabled under the merge-conflicts dialog.
    CONFLICTS_DIALOG_KEEPS_OPEN_ITEMS = 424 "conflicts-dialog-keeps-open-items" {
        title: "Open items while resolving conflicts",
        summary: "While the merge-conflicts dialog is open, Show in Finder, Open in External \
                  Editor, Open in Terminal and View on GitHub stay enabled in the menus, to look \
                  at the conflicted files without closing the dialog.",
        ghd_behaviour: "Every menu item is disabled while the dialog is open.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(14236)],
        code: &["crates/corvene-core/src/menu_state.rs", "crates/corvene/src/main.rs"],
    },

    /// The command line tool lists the repositories and opens a tab.
    CLI_LIST_REPOSITORIES = 425 "cli-list-repositories" {
        title: "List repositories from the command line",
        summary: "Corvene keeps repositories.json and repositories.txt in its data folder up to \
                  date with the repository list (name, path, which one is selected), so scripts \
                  and launchers can read it while Corvene runs; corvene list [--json] prints it. \
                  corvene open [path] --changes or --history opens the repository on that tab.",
        ghd_behaviour: "The repository list cannot be read from outside, and the command line \
                        tool opens a repository on whatever tab it was on.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22779)],
        code: &[
            "crates/corvene-core/src/repository_list_file.rs",
            "crates/corvene-core/src/app_url.rs",
            "packaging/corvene.sh",
            "packaging/linux/corvene.sh",
            "packaging/windows/corvene.bat",
            "crates/corvene/src/cli_windows.rs",
        ],
    },

    /// The repository button and list rows drag the folder out.
    DRAG_REPOSITORY_OUT = 426 "drag-repository-out" {
        title: "Drag a repository out of the window",
        summary: "The toolbar's Current Repository button and the repository list's rows can be \
                  dragged out of the window as the repository's folder: onto Finder, the Dock, \
                  an editor or Terminal. Clicking them works as before.",
        ghd_behaviour: "Repositories cannot be dragged anywhere.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: macos_only,
        upstream: &[Upstream::issue(9411)],
        code: &["crates/corvene-ui/src/repository_drag.rs", "crates/corvene-ui/src/toolbar.rs", "crates/corvene-ui/src/repository_list.rs"],
    },

    /// View › Back / Forward through the repositories and sections shown.
    BACK_FORWARD_NAVIGATION = 427 "back-forward-navigation" {
        title: "Back and Forward",
        summary: "View › Back (⌃-) and Forward (⌃⇧-) step through the repositories and the \
                  Changes / History tabs shown before, like a browser's history (up to 50 steps; \
                  removed repositories are skipped). Off macOS, where Ctrl+- zooms out, they are \
                  Alt+Left and Alt+Right.",
        ghd_behaviour: "No navigation history: going back to the previous repository or tab means \
                        picking it again.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22775)],
        code: &["crates/corvene-core/src/navigation.rs", "crates/corvene-ui/src/app_menu.rs", "crates/corvene-ui/src/keymap.rs"],
    },

    /// A menu bar status item with the watched repositories' sync state and checks.
    MENU_BAR_STATUS_ITEM = 428 "menu-bar-status-item" {
        title: "Menu bar status item",
        summary: "A status item in the menu bar for the repositories ticked in Settings › \
                  Advanced › Menu bar: its icon is the worst state of their pushed tips' \
                  checks, its title the ahead/behind counts, and its menu lists each \
                  repository (opens it) with the branch's sync state, its checks and every \
                  failing check (opens it on GitHub). Fed by the repository indicators pass \
                  and the check status refresh; nothing polls on its own.",
        ghd_behaviour: "No status item; the Dock icon shows nothing either.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: macos_status_item,
        upstream: &[],
        code: &["crates/corvene-core/src/menu_bar_status.rs", "crates/corvene-ui/src/status_item.rs", "crates/corvene-ui/src/dialogs/preferences.rs", "crates/corvene-core/src/remote.rs"],
    },

    /// File › New Window, "Open in New Window" in the repository list.
    MULTIPLE_WINDOWS = 429 "multiple-windows" {
        title: "Multiple windows",
        summary: "File › New Window (⌥⌘N) opens another window, and a repository's context \
                  menu in the repository list has Open in New Window, so two repositories sit \
                  side by side. Each window has its own repository, foldouts, dialogs and \
                  Back / Forward history; the windows open again at the next launch. Closing a \
                  window that is not the last one closes it for good.",
        ghd_behaviour: "One window; switching repositories is the only way to see another.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(3606), Upstream::issue(22427), Upstream::issue(4340), Upstream::issue(12290)],
        code: &["crates/corvene-core/src/workspace.rs", "crates/corvene-ui/src/windows.rs", "crates/corvene/src/main.rs"],
    },

    /// A tab strip of repositories above the toolbar.
    REPOSITORY_TABS = 430 "repository-tabs" {
        title: "Repository tabs",
        summary: "A strip of tabs above the toolbar, one per open repository: the repository \
                  list's context menu has Open in New Tab, + opens the list, ⌥⌘→ / ⌥⌘← step \
                  through the tabs and ⌘W closes the current one while the window has more than \
                  one. Picking a repository from the list switches the current tab to it.",
        ghd_behaviour: "No tabs; the toolbar's repository button switches the one view.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20026), Upstream::issue(12290)],
        code: &["crates/corvene-ui/src/tab_strip.rs", "crates/corvene-core/src/workspace.rs"],
    },

    // ---- 500 Settings & updates ----

    /// Settings › Advanced › Save crash reports locally.
    CRASH_REPORTS = 501 "crash-reports" {
        title: "Save crash reports locally",
        summary: "Settings › Advanced offers \"Save crash reports locally\": a panic hook writes \
                  ~/Library/Logs/Corvene/crashes/ (Linux: ~/.local/state/corvene/crashes/) and \
                  the next launch lists new reports. Nothing is uploaded.",
        ghd_behaviour: "No local crash reports (GHD's crash reporter uploads to GitHub instead).",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-ui/src/dialogs/preferences.rs", "crates/corvene-core/src/crash_reports.rs"],
    },

    /// Background update-check errors stay in the log.
    QUIET_BACKGROUND_UPDATE_ERRORS = 503 "quiet-background-update-errors" {
        title: "Quiet background update checks",
        summary: "Errors from the automatic update checks are only logged; Check for Updates in \
                  About still shows them.",
        ghd_behaviour: "Posts every update error.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[],
        code: &["crates/corvene-core/src/updater.rs"],
    },

    /// Untagged release-note items keep their heading's kind.
    RELEASE_NOTES_HEADING_KINDS = 504 "release-notes-heading-kinds" {
        title: "Release notes: untagged items keep their heading's kind",
        summary: "Release-note items without a [Kind] tag are classified by their ## heading, and \
                  the notes' leading paragraph is shown.",
        ghd_behaviour: "Drops untagged items and the leading paragraph.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[],
        code: &["crates/corvene-core/src/release_notes.rs"],
    },

    /// A settings file that cannot be written is reported.
    REPORT_SETTINGS_SAVE_ERRORS = 505 "report-settings-save-errors" {
        title: "Report settings that could not be saved",
        summary: "When a changed setting cannot be written to disk, an error dialog says so \
                  (with the reason) instead of the change silently being lost on the next launch.",
        ghd_behaviour: "A failed save goes unnoticed; the setting reverts after a restart.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(5046)],
        code: &["crates/corvene-core/src/dispatcher.rs"],
    },

    /// No automatic update checks.
    NO_AUTOMATIC_UPDATE_CHECKS = 506 "no-automatic-update-checks" {
        title: "No automatic update checks",
        summary: "Corvene does not check for updates at launch or every four hours; Check for \
                  Updates in About still checks, downloads and installs on request.",
        ghd_behaviour: "Always checks at launch and every four hours and downloads what it finds.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(3410), Upstream::issue(22468)],
        code: &["crates/corvene-core/src/updater.rs"],
    },

    /// Editors GitHub Desktop does not detect.
    EXTRA_EDITORS = 507 "extra-editors" {
        title: "Detect more external editors",
        summary: "Settings › Integrations and Open in … also find editors GitHub Desktop 3.6.6 \
                  does not know: Antigravity (and on Linux Cursor and Windsurf; on Windows \
                  Microsoft Edit, opened in a console window, and gVim).",
        ghd_behaviour: "Only its own editor table.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22922), Upstream::issue(22638), Upstream::issue(12317)],
        code: &["crates/corvene-platform/src/editors.rs", "crates/corvene-platform/src/editors_windows.rs", "crates/corvene-core/src/integrations.rs", "crates/corvene-core/src/flags/dispatch.rs"],
    },

    /// A name for the custom editor.
    CUSTOM_EDITOR_NAME = 508 "custom-editor-name" {
        title: "Custom editor name",
        summary: "Settings › Integrations › Configure Custom Editor… has a Name box; menus then \
                  say \"Open in <name>\" instead of \"Open in Custom Editor\".",
        ghd_behaviour: "Always \"Custom Editor\".",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21376)],
        code: &["crates/corvene-ui/src/dialogs/preferences.rs", "crates/corvene-core/src/state.rs"],
    },

    /// VS Code opens the repository's workspace file.
    VSCODE_WORKSPACE_FILE = 509 "vscode-workspace-file" {
        title: "Open the VS Code workspace file",
        summary: "Opening the repository in Visual Studio Code (or VSCodium, Cursor, Windsurf) \
                  opens its `*.code-workspace` file when the repository's top folder has exactly \
                  one.",
        ghd_behaviour: "Always opens the folder.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(7007)],
        code: &["crates/corvene-core/src/integrations.rs", "crates/corvene-platform/src/editors.rs"],
    },

    /// Reveal in another file manager.
    FILE_MANAGER = 510 "file-manager" {
        title: "File manager",
        summary: "Application that Show in Finder and the Reveal in Finder items open the folder \
                  with (a file's parent folder), by name or path: `Path Finder`, \
                  `/Applications/ForkLift.app`. Empty uses Finder.",
        ghd_behaviour: "Always Finder.",
        nature: Nature::Feature,
        kind: Kind::Text { placeholder: "Path Finder", validate: app_name },
        corvene: Value::text(""), ghd: Value::text(""),
        familiar: Value::text(""), max: Value::text(""),
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(13812)],
        code: &["crates/corvene-core/src/integrations.rs"],
    },

    /// Open web links in a chosen browser.
    BROWSER = 511 "browser" {
        title: "Browser",
        summary: "Application that web links open in (GitHub pages, pull requests, sign-in, help \
                  links), by name or path: `Firefox`, `/Applications/Safari.app`. Empty uses the \
                  system's default browser.",
        ghd_behaviour: "Always the default browser.",
        nature: Nature::Feature,
        kind: Kind::Text { placeholder: "Firefox", validate: app_name },
        corvene: Value::text(""), ghd: Value::text(""),
        familiar: Value::text(""), max: Value::text(""),
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(21762)],
        code: &["crates/corvene-core/src/dispatcher.rs"],
    },

    /// Settings › Prompts leaves out the Copilot prompt.
    COPILOT_PROMPT_OMITTED = 512 "copilot-prompt-omitted" {
        title: "Settings › Prompts: no Copilot prompt",
        summary: "Settings › Prompts leaves out \"Overriding commit message with generated \
                  message\": Corvene has no Copilot commit message generation, so the \
                  checkbox would do nothing.",
        ghd_behaviour: "Lists the checkbox between \"Undo commit\" and \"Removing worktrees\".",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[],
        code: &["crates/corvene-ui/src/dialogs/preferences.rs"],
    },

    /// Application icons in the editor and shell menus.
    INTEGRATION_APP_ICONS = 513 "integration-app-icons" {
        title: "Application icons in Settings › Integrations",
        summary: "The External editor and Shell menus of Settings › Integrations show each \
                  application's icon in front of its name (macOS: the app's Finder icon; Linux: \
                  the icon of its desktop entry; Windows: the program's icon).",
        ghd_behaviour: "Names only.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: android_built_in,
        upstream: &[],
        code: &["crates/corvene-platform/src/app_icons.rs", "crates/corvene-core/src/integrations.rs", "crates/corvene-ui/src/dialogs/preferences.rs", "crates/corvene-ui/src/native_menu.rs"],
    },

    /// Settings › Advanced › Clone location.
    DEFAULT_CLONE_LOCATION = 514 "default-clone-location" {
        title: "Default clone location setting",
        summary: "Settings › Advanced › Clone location picks the folder Clone, New Repository \
                  and the tutorial suggest for new repositories (default ~/Documents/GitHub).",
        ghd_behaviour: "No setting; the folder of the last clone is suggested next time.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22630)],
        code: &["crates/corvene-ui/src/dialogs/preferences.rs"],
    },

    /// The default clone folder stays out of OneDrive.
    CLONE_DIR_AVOIDS_ONEDRIVE = 515 "clone-dir-avoids-onedrive" {
        title: "Default clone folder outside OneDrive",
        summary: "On Windows, when OneDrive syncs the Documents folder, repositories are cloned \
                  to %USERPROFILE%\\GitHub instead of Documents\\GitHub (syncing a repository's \
                  .git folder corrupts it and fills the OneDrive quota).",
        ghd_behaviour: "Always Documents\\GitHub, inside OneDrive when it backs up Documents.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: cfg!(windows), availability: available,
        upstream: &[Upstream::issue(13017)],
        code: &["crates/corvene-platform/src/lib.rs", "crates/corvene-core/src/state.rs"],
    },

    /// Which git to use.
    GIT_EXECUTABLE = 516 "git-executable" {
        title: "Git executable",
        summary: "Full path of the git Corvene runs (e.g. /opt/homebrew/bin/git or ~/bin/git), \
                  tried before CORVENE_GIT, PATH and the usual install locations. Empty finds \
                  git as usual; a path that can't be run or is too old is skipped.",
        ghd_behaviour: "Always its bundled git.",
        nature: Nature::Feature,
        kind: Kind::Text { placeholder: "/usr/local/bin/git", validate: git_path },
        corvene: Value::text(""), ghd: Value::text(""),
        familiar: Value::text(""), max: Value::text(""),
        restart: true, visible: true, availability: available,
        upstream: &[Upstream::issue(14222)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/detect.rs"],
    },

    /// Settings › Git path options.
    PATH_GIT_SETTINGS = 517 "path-git-settings" {
        title: "Settings › Git: path options",
        summary: "Settings › Git › Default branch also offers the global core.quotepath (show \
                  non-ASCII file names as they are in git's own output) and, on Windows, \
                  core.longpaths (paths over 260 characters).",
        ghd_behaviour: "Only through the global Git config file.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21157)],
        code: &["crates/corvene-ui/src/dialogs/preferences.rs", "crates/corvene-core/src/integrations.rs"],
    },

    /// An external editor per repository.
    PER_REPO_EDITOR = 518 "per-repo-editor" {
        title: "Editor per repository",
        summary: "Repository Settings has an Editor tab to pick the external editor this \
                  repository opens in (Open in External Editor, opening its files and the menus' \
                  \"Open in …\" labels); \"Use my default editor\" keeps the one in Settings.",
        ghd_behaviour: "One editor for every repository.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(12195)],
        code: &["crates/corvene-ui/src/dialogs/repository_settings.rs", "crates/corvene-core/src/integrations.rs", "crates/corvene-core/src/state.rs"],
    },

    /// Files open in their repository's editor window.
    OPEN_FILE_IN_REPOSITORY_WINDOW = 519 "open-file-in-repository-window" {
        title: "Open files in the repository's editor window",
        summary: "Opening a changed or committed file in VS Code (and its forks), Cursor, \
                  Windsurf, Zed or Sublime Text passes the repository folder along (`code <repo> \
                  <file>`), so the file opens in the window that has the repository open instead \
                  of a loose window or the last active one.",
        ghd_behaviour: "The file is handed to the editor alone.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22278)],
        code: &["crates/corvene-core/src/integrations.rs", "crates/corvene-platform/src/editors.rs"],
    },

    /// JetBrains IDEs registered under the 64-bit machine key.
    JETBRAINS_64BIT_HIVE = 520 "jetbrains-64bit-hive" {
        title: "Find JetBrains IDEs in the 64-bit registry",
        summary: "On Windows, JetBrains IDEs installed for all users are also looked for under \
                  the 64-bit machine uninstall key, where current installers register them.",
        ghd_behaviour: "Only checks the 32-bit machine key and the user key, so such IDEs are \
                        not detected.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(21990)],
        code: &["crates/corvene-platform/src/editors_windows.rs", "crates/corvene-platform/src/editors.rs", "crates/corvene-core/src/integrations.rs"],
    },

    /// Notepad++ shows a repository as a folder workspace.
    NOTEPADPP_FOLDER_WORKSPACE = 521 "notepadpp-folder-workspace" {
        title: "Notepad++ opens a repository as a folder workspace",
        summary: "Open in Notepad++ hands it the repository folder with \
                  -openFoldersAsWorkspace, so the folder shows in its Folder as Workspace panel.",
        ghd_behaviour: "Passes the folder alone, and Notepad++ opens every file in the \
                        repository, which can exhaust its memory in a large one.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(18960)],
        code: &["crates/corvene-platform/src/editors.rs", "crates/corvene-core/src/integrations.rs"],
    },

    /// `~/.config/corvene/settings.json` applied over the stored settings.
    SETTINGS_FILE = 522 "settings-file" {
        title: "Settings file",
        summary: "At launch the settings in ~/.config/corvene/settings.json (on Windows \
                  %APPDATA%\\corvene\\settings.json) override the stored ones, and its \"flags\" \
                  entry sets flags for the session like CORVENE_FLAGS; Corvene never writes the \
                  file nor saves what it set. Settings › Advanced › Export Settings… writes the \
                  current settings and flags in its format. Keys it cannot use are listed in a \
                  banner.",
        ghd_behaviour: "Settings live in the app's own storage only; there is no file to keep in \
                        dotfiles or to copy to another machine.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: true, visible: true, availability: available,
        upstream: &[Upstream::issue(22464)],
        code: &["crates/corvene-core/src/settings_file.rs", "crates/corvene/src/main.rs", "crates/corvene-ui/src/dialogs/preferences.rs"],
    },

    /// Several custom editors.
    CUSTOM_EDITOR_LIST = 523 "custom-editor-list" {
        title: "Several custom editors",
        summary: "Settings › Integrations keeps a list of custom editors (name, path and \
                  arguments; Add Custom Editor… and Remove Custom Editor): the editor menu lists \
                  each by name next to the installed editors, and so does the No Changes card's \
                  editor picker.",
        ghd_behaviour: "One custom editor (\"Configure Custom Editor…\").",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(19163)],
        code: &["crates/corvene-ui/src/dialogs/preferences.rs", "crates/corvene-core/src/persistence.rs", "crates/corvene-core/src/state.rs"],
    },

    /// Open a repository once in another editor.
    OPEN_REPOSITORY_WITH_EDITOR = 524 "open-repository-with-editor" {
        title: "Open in another editor",
        summary: "The repository list's context menu and the Repository menu get an \"Open in \
                  Editor\" submenu listing the installed editors (and the custom ones) that opens \
                  the repository in the one picked this once, without changing Settings. Shown \
                  when there are at least two editors.",
        ghd_behaviour: "Repositories open in the editor chosen in Settings; another one means \
                        changing that setting first.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(18737)],
        code: &["crates/corvene-core/src/integrations.rs", "crates/corvene-ui/src/app_menu.rs", "crates/corvene-ui/src/repository_list.rs"],
    },

    /// An account's commit email for the repositories cloned from it.
    ACCOUNT_COMMIT_EMAIL = 525 "account-commit-email" {
        title: "Commit email per account",
        summary: "Settings › Accounts has a Commit email box under each account: a repository of \
                  that account (an Enterprise one, say) that is cloned or added without its own \
                  user.email gets it in its local Git config, so work commits do not carry the \
                  personal global email.",
        ghd_behaviour: "Commits use the global Git email unless a repository sets its own in \
                        Repository Settings.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(2829)],
        code: &["crates/corvene-ui/src/dialogs/preferences.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// Commit signing: a hint, a plain error and the settings.
    COMMIT_SIGNING = 526 "commit-signing" {
        title: "Commit signing",
        summary: "When Git signs commits (commit.gpgsign), a lock under the commit button says \
                  \"Commits will be signed\"; a commit that fails because Git could not sign it \
                  says so above Git's output; and Settings › Git and Repository Settings › Git \
                  Config (local config) have Sign commits, Signing key and an SSH key choice \
                  (commit.gpgsign, user.signingkey, gpg.format).",
        ghd_behaviour: "Signing works through the Git configuration only: nothing shows that \
                        commits are signed, a signing failure shows Git's output alone and \
                        signing is set up outside the app.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(78), Upstream::issue(12147)],
        code: &["crates/corvene-ui/src/changes.rs", "crates/corvene-git/src/git_errors.rs", "crates/corvene-core/src/integrations.rs", "crates/corvene-ui/src/dialogs/preferences.rs", "crates/corvene-ui/src/dialogs/repository_settings.rs"],
    },

    /// Several accounts per host, and the account each repository uses.
    MULTIPLE_ACCOUNTS = 527 "multiple-accounts" {
        title: "Several accounts per host",
        summary: "Sign in to more than one account on GitHub.com or a GitHub Enterprise host \
                  (Settings › Accounts › Add account), say a personal and a work account. Each \
                  repository uses one of them for fetching, pushing, pull requests, checks and \
                  everything else from GitHub: the account that can push to it, picked when the \
                  repository is added or cloned (asked once when several can), and changed in \
                  Repository Settings › Remote or from the account button in the toolbar.",
        ghd_behaviour: "One account per host: signing in to another GitHub.com account signs \
                        the first one out.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(3707), Upstream::issue(21365)],
        code: &["crates/corvene-core/src/accounts.rs", "crates/corvene-core/src/state.rs", "crates/corvene-core/src/remote.rs", "crates/corvene-ui/src/dialogs/preferences.rs", "crates/corvene-ui/src/dialogs/repository_settings.rs", "crates/corvene-ui/src/dialogs/choose_repository_account.rs", "crates/corvene-ui/src/toolbar.rs"],
    },
    /// Proxies that want a username and password (desktop#10026).
    PROXY_CREDENTIALS = 528 "proxy-credentials" {
        title: "Sign in to proxies that ask for a password",
        summary: "When a proxy answers \"407 Proxy Authentication Required\" to a fetch, push, \
                  pull or clone, or to a request to GitHub, Corvene asks for the proxy's \
                  username and password, keeps them in the keychain and tries again. Git and \
                  Corvene's own requests both use them. Settings › Advanced shows the proxy in \
                  use and forgets a saved password.",
        ghd_behaviour: "Shows Git's \"CONNECT tunnel failed, response 407\" error, and requests to \
                        GitHub fail; the only way through is a proxy address with the password \
                        written into it.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(10026)],
        code: &["crates/corvene-core/src/proxy.rs", "crates/corvene-platform/src/proxy.rs", "crates/corvene-git/src/proxy.rs", "crates/corvene-core/src/askpass.rs", "crates/corvene-ui/src/dialogs/proxy_authentication.rs", "crates/corvene-ui/src/dialogs/preferences.rs"],
    },

    // ---- 600 Keyboard & accessibility ----

    /// ⌘9 / ⌘8 announce the width after the step.
    RESIZABLE_ANNOUNCES_NEW_WIDTH = 601 "resizable-announces-new-width" {
        title: "Expand / Contract Active Resizable announces the new width",
        summary: "⌘9 / ⌘8 announce the percentage of the width after the step.",
        ghd_behaviour: "Reads the width before applying the step, so the announced number lags one step.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[],
        code: &["crates/corvene-ui/src/active_resizable.rs"],
    },

    /// Repository Settings › Git Config always labels its email box.
    GIT_CONFIG_EMAIL_LABEL = 602 "git-config-email-label" {
        title: "Git Config's email box keeps its label",
        summary: "Settings › Git › Author and Repository Settings › Git Config show \"Email\" \
                  above the email text box whenever it stands alone.",
        ghd_behaviour: "The label disappears whenever the email isn't one of the signed-in \
                        accounts' addresses (always, when signed out), although the code means to \
                        hide it only under the account-email dropdown's \"Other\".",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-ui/src/dialogs/repository_settings.rs", "crates/corvene-ui/src/dialogs/preferences.rs"],
    },

    /// Switching to Changes or History focuses that section's list.
    FOCUS_LIST_ON_SECTION_SWITCH = 603 "focus-list-on-section-switch" {
        title: "Switching tabs focuses the list",
        summary: "Clicking the Changes or History tab, ⌘1 / ⌘2 and ⌃Tab put keyboard focus on \
                  the section's list, so the arrow keys work straight away.",
        ghd_behaviour: "Focus stays where it was (often the page body), and the list has to be \
                        tabbed to.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(535)],
        code: &["crates/corvene-ui/src/workspace.rs", "crates/corvene/src/main.rs"],
    },

    /// At launch the commit summary takes focus when there are changes.
    LAUNCH_FOCUSES_COMMIT_SUMMARY = 604 "launch-focuses-commit-summary" {
        title: "Launch focuses the commit summary",
        summary: "When Corvene opens on a repository with uncommitted changes, the caret starts \
                  in the commit summary, ready to type.",
        ghd_behaviour: "Nothing useful has focus at launch; the summary has to be clicked (or \
                        reached with ⌘G).",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20417)],
        code: &["crates/corvene-ui/src/workspace.rs"],
    },

    /// ⌘⌫ in the changes list discards the highlighted files.
    CMD_BACKSPACE_DISCARDS_FILES = 605 "cmd-backspace-discards-files" {
        title: "⌘⌫ in the changes list discards the selected files",
        summary: "With the changes list focused, ⌘⌫ discards the highlighted files (confirming \
                  as the context menu's Discard Changes does); elsewhere it still removes the \
                  repository.",
        ghd_behaviour: "⌘⌫ is Repository › Remove… everywhere, also in the changes list.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(11924), Upstream::issue(17680)],
        code: &["crates/corvene-ui/src/keymap.rs", "crates/corvene-ui/src/changes.rs"],
    },

    /// ⇧⌘A / ⌥⌘O open the file selected in a file list.
    OPEN_FILE_SHORTCUTS = 606 "open-file-shortcuts" {
        title: "Shortcuts that open the selected file",
        summary: "With the changes list or a commit's file list focused, ⇧⌘A opens the selected \
                  file in the external editor (instead of the repository) and ⌥⌘O opens it with \
                  its default program.",
        ghd_behaviour: "Only the file context menu opens a file; ⇧⌘A always opens the repository.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(13691), Upstream::issue(4655), Upstream::issue(20773)],
        code: &[
            "crates/corvene-ui/src/keymap.rs",
            "crates/corvene-ui/src/changes.rs",
            "crates/corvene-ui/src/selected_commit.rs",
        ],
    },

    /// Repository › Push has no ⌘P shortcut.
    NO_PUSH_SHORTCUT = 607 "no-push-shortcut" {
        title: "No ⌘P shortcut for Push",
        summary: "⌘P does nothing and Repository › Push shows no shortcut, so a stray ⌘P (print, \
                  quick open in an editor) cannot push.",
        ghd_behaviour: "⌘P pushes (or opens Force Push… after a rebase or amend).",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(14604)],
        code: &["crates/corvene-ui/src/keymap.rs"],
    },

    /// ⌥⌘T opens the repository in the shell, next to ⌃`.
    OPEN_IN_SHELL_ALT_SHORTCUT = 608 "open-in-shell-alt-shortcut" {
        title: "⌥⌘T also opens the shell",
        summary: "Repository › Open in <shell> also answers to ⌥⌘T, which every keyboard layout \
                  can type (⌃` is a dead key on German and other layouts).",
        ghd_behaviour: "Only ⌃`.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(3240)],
        code: &["crates/corvene-ui/src/keymap.rs"],
    },

    /// ⌃N / ⌃P move the selection in the changes and history lists.
    EMACS_LIST_KEYS = 609 "emacs-list-keys" {
        title: "⌃N / ⌃P move through lists",
        summary: "⌃N and ⌃P select the next and previous row of the changes and history lists, \
                  like ↓ and ↑ (the macOS text-system keys).",
        ghd_behaviour: "Only the arrow keys move the selection.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(7266)],
        code: &["crates/corvene-ui/src/keymap.rs"],
    },

    /// ⌥⌘S switches the diff between unified and split.
    DIFF_MODE_SHORTCUT = 610 "diff-mode-shortcut" {
        title: "⌥⌘S switches the diff display",
        summary: "⌥⌘S toggles Diff Settings between Unified and Split.",
        ghd_behaviour: "Only the diff settings popover changes it (three clicks).",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(15284)],
        code: &["crates/corvene-ui/src/keymap.rs", "crates/corvene/src/main.rs"],
    },

    /// ⌥⌘C / ⇧⌥⌘C copy the selected files' paths.
    COPY_PATH_SHORTCUTS = 611 "copy-path-shortcuts" {
        title: "Shortcuts that copy file paths",
        summary: "With the changes list or a commit's file list focused, ⌥⌘C copies the selected \
                  files' full paths and ⇧⌥⌘C their paths relative to the repository (VS Code's \
                  keys), one per line.",
        ghd_behaviour: "Only the file context menu copies paths.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(21810)],
        code: &[
            "crates/corvene-ui/src/keymap.rs",
            "crates/corvene-ui/src/changes.rs",
            "crates/corvene-ui/src/selected_commit.rs",
        ],
    },

    /// ⌃⌘P, ⇧⌘] / ⇧⌘[, ⌘3 and ⌥↓ / ⌥↑ in the diff.
    NAVIGATION_SHORTCUTS = 612 "navigation-shortcuts" {
        title: "More navigation shortcuts",
        summary: "View › Show Pull Requests List (⌃⌘P) opens the branch list on its Pull Requests \
                  tab; ⇧⌘] / ⇧⌘[ switch to the next / previous repository in the list's order; \
                  ⌘3 focuses the diff; ⌥↓ / ⌥↑ in the diff select the next / previous file.",
        ghd_behaviour: "None of these shortcuts; the pull request list, other repositories and the \
                        diff are reached with the mouse or by tabbing.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[
            Upstream::issue(16854),
            Upstream::issue(20115),
            Upstream::issue(20677),
            Upstream::issue(19935),
        ],
        code: &[
            "crates/corvene-ui/src/keymap.rs",
            "crates/corvene-ui/src/workspace.rs",
            "crates/corvene/src/main.rs",
            "crates/corvene-ui/src/app_menu.rs",
        ],
    },

    /// Arrow keys in the repository list start at the selected repository.
    REPOSITORY_LIST_STARTS_AT_SELECTED = 613 "repository-list-starts-at-selected" {
        title: "Repository list arrows start at the current repository",
        summary: "With no row highlighted yet, ↓ / ↑ in the repository list's filter box move to \
                  the row after / before the selected repository instead of the first / last row.",
        ghd_behaviour: "The first ↓ always goes to the top of the list (↑ to the bottom), however \
                        far down the current repository is.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(2650)],
        code: &["crates/corvene-ui/src/repository_list.rs"],
    },

    /// Honour the system's Reduce Motion setting.
    SYSTEM_REDUCE_MOTION = 614 "system-reduce-motion" {
        title: "Follow the system's Reduce Motion",
        summary: "With Reduce Motion on (macOS Accessibility › Display, Windows' Animation \
                  effects off, GNOME's Reduce Animation), spinners stand still and the mouse \
                  wheel scrolls without easing.",
        ghd_behaviour: "Animates regardless of the system setting.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(3318)],
        code: &["crates/corvene/src/main.rs", "crates/corvene-platform/src/accessibility.rs"],
    },

    /// Typing in the changes list goes to the commit summary.
    TYPE_TO_COMMIT_SUMMARY = 615 "type-to-commit-summary" {
        title: "Type in the changes list to write the summary",
        summary: "A letter, digit or other printable key pressed (without ⌘, ⌃ or ⌥) while the \
                  changes list has focus moves focus to the commit summary and types the \
                  character at its end. Space still ticks the selected files.",
        ghd_behaviour: "Printable keys do nothing in the changes list.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(15350)],
        code: &["crates/corvene-ui/src/changes.rs"],
    },

    /// → accepts the generated commit summary placeholder.
    ACCEPT_SUMMARY_PLACEHOLDER = 616 "accept-summary-placeholder" {
        title: "→ accepts the summary placeholder",
        summary: "With one file included, the empty commit summary shows a generated summary \
                  (\"Update README.md\"); pressing → there types it into the field, caret at the \
                  end, to extend it instead of retyping it.",
        ghd_behaviour: "The placeholder is only used as is when committing with an empty summary.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20563)],
        code: &["crates/corvene-ui/src/changes.rs"],
    },

    /// Back on Changes, the commit field that had focus gets it again.
    SECTION_SWITCH_RESTORES_COMMIT_FOCUS = 617 "section-switch-restores-commit-focus" {
        title: "Return to the commit message after History",
        summary: "Leaving the Changes tab while typing the commit summary or description (for \
                  History, with a tab, ⌘2 or ⌃Tab) and coming back puts the caret back in that \
                  field, instead of on the changes list or nowhere.",
        ghd_behaviour: "Focus is not restored when Changes is shown again.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(19390)],
        code: &["crates/corvene-ui/src/workspace.rs", "crates/corvene-ui/src/changes.rs"],
    },

    /// `keymap.json` changes or removes keyboard shortcuts.
    KEYMAP_OVERRIDES = 618 "keymap-overrides" {
        title: "Custom keyboard shortcuts",
        summary: "A keymap.json file in Corvene's data folder maps action names to keystrokes \
                  ({\"Push\": null, \"Pull\": \"cmd-shift-l\"}): null removes an action's \
                  shortcut, a keystroke replaces it, and the menus show the result. Read at launch \
                  and whenever Settings closes; names or keystrokes it cannot use are listed in a \
                  banner.",
        ghd_behaviour: "The keyboard shortcuts are fixed.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(19259)],
        code: &["crates/corvene-core/src/keymap_file.rs", "crates/corvene-ui/src/keymap.rs"],
    },

    /// ← / → move focus between the lists and the diff.
    ARROW_KEYS_BETWEEN_PANES = 619 "arrow-keys-between-panes" {
        title: "Arrow keys between panes",
        summary: "← and → in the History commit list, the file lists and the diff move keyboard \
                  focus to the pane on that side: commit list ⇄ files ⇄ diff in History, files ⇄ \
                  diff in Changes.",
        ghd_behaviour: "← and → do nothing there; Tab moves between the panes.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22222)],
        code: &["crates/corvene-ui/src/keymap.rs", "crates/corvene-ui/src/workspace.rs"],
    },

    /// ↑ / ↓ stop at the ends of the changes, commit and file lists.
    LISTS_STOP_AT_ENDS = 620 "lists-stop-at-ends" {
        title: "Lists stop at their ends",
        summary: "↓ on the last row and ↑ on the first row of the Changes file list, the History \
                  commit list and a commit's file list stay there instead of jumping to the other \
                  end.",
        ghd_behaviour: "The selection wraps around: ↓ on the last row selects the first.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22222)],
        code: &["crates/corvene-ui/src/filter_list.rs", "crates/corvene-ui/src/changes.rs", "crates/corvene-ui/src/history.rs", "crates/corvene-ui/src/selected_commit.rs"],
    },

    /// A "…" button on list rows opens their context menu.
    CONTEXT_MENU_BUTTONS = 621 "context-menu-buttons" {
        title: "Context menu buttons on rows",
        summary: "Rows of the repository, branch, changes, history and commit file lists show a \
                  \"…\" button at their right end while hovered or selected; it opens the row's \
                  context menu, so the menus can be found without a right-click.",
        ghd_behaviour: "The rows' menus open only with a right-click (or Shift+F10).",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(2718)],
        code: &["crates/corvene-ui/src/context_menu.rs"],
    },

    /// Tab follows macOS's Keyboard navigation setting.
    SYSTEM_KEYBOARD_NAVIGATION = 622 "system-keyboard-navigation" {
        title: "Follow the system's keyboard navigation setting",
        summary: "On macOS, Tab moves between text fields and lists only, unless System \
                  Settings › Keyboard › Keyboard navigation is on: then it also reaches \
                  buttons, links, checkboxes and selects, as in other Mac apps. The setting is \
                  read again whenever a window becomes active. Windows and Linux always tab \
                  through every control.",
        ghd_behaviour: "Tab reaches every button and link whatever the macOS setting says.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(4623)],
        code: &["crates/corvene-ui/src/keyboard_nav.rs", "crates/corvene-platform/src/accessibility.rs", "crates/corvene/src/main.rs"],
    },

    /// Focus rings stay visible in the high contrast theme.
    CONTRAST_FOCUS_RING = 623 "contrast-focus-ring" {
        title: "Visible keyboard focus in the high contrast theme",
        summary: "In the high contrast theme the keyboard focus ring and the row the arrow \
                  keys reach in the repository and branch lists (and the other dropdowns) are \
                  outlined in the text colour, so the focus shows without relying on the \
                  highlight's background. Applies on every platform.",
        ghd_behaviour: "Dropdown rows show focus only by a background colour, which a contrast \
                        theme can make invisible.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22949)],
        code: &["crates/corvene-ui/src/keyboard_nav.rs", "crates/corvene-ui/src/branch_list.rs", "crates/corvene-ui/src/repository_list.rs"],
    },

    // ---- 700 Changes & diffs ----

    /// Changes list: lines added / deleted per file and in total.
    CHANGES_LINE_COUNTS = 701 "changes-line-counts" {
        title: "Line counts in the Changes list",
        summary: "Each changed file shows the lines it adds and removes against the last commit \
                  (+N -M), and the \"N changed files\" header shows the totals of the listed \
                  files. Runs git diff --numstat on every refresh; untracked files over 1 MiB and \
                  binary files get no count.",
        ghd_behaviour: "No line counts for uncommitted changes (only the commit summary in History \
                        has them).",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[
            Upstream::issue(12914),
            Upstream::issue(14916),
            Upstream::issue(16024),
            Upstream::issue(16930),
            Upstream::issue(22403),
        ],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-ui/src/changes.rs", "crates/corvene-git/src/status.rs"],
    },

    /// File names without their directory in the changes list.
    CHANGES_FILE_NAMES_ONLY = 702 "changes-file-names-only" {
        title: "File names only in the changes list",
        summary: "Rows of the changes list show the file name alone, without its directory \
                  (the filter still matches the whole path).",
        ghd_behaviour: "Directory (dimmed) followed by the file name.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(14268), Upstream::issue(19016)],
        code: &["crates/corvene-ui/src/changes.rs"],
    },

    /// Order of the changes list.
    CHANGES_SORT_ORDER = 703 "changes-sort-order" {
        title: "Changes list order",
        summary: "How the changes list orders its files: by path, by status (conflicted, new, \
                  modified, renamed, deleted; path order within each), by file name, or by the \
                  repository's diff.orderFile (files by the first glob matching them, then path; \
                  unmatched files last; path order when it is unset). A filter text still ranks \
                  its matches best first.",
        ghd_behaviour: "Path order (git's).",
        nature: Nature::Feature,
        kind: Kind::Select { options: CHANGES_SORT_ORDERS },
        corvene: Value::text("path"), ghd: Value::text("path"),
        familiar: Value::text("path"), max: Value::text("path"),
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(4739), Upstream::issue(22622)],
        code: &["crates/corvene-ui/src/changes.rs", "crates/corvene-core/src/filter.rs", "crates/corvene-git/src/commit_template.rs"],
    },

    /// How the changes filter text matches.
    CHANGES_FILTER_MATCH = 704 "changes-filter-match" {
        title: "Changes filter matching",
        summary: "How the changes list's filter text matches a path: fuzzily (the letters in \
                  order), as a substring, as the end of the path (`.meta`), or as the exact path \
                  or file name. Case is ignored.",
        ghd_behaviour: "Fuzzy only.",
        nature: Nature::Feature,
        kind: Kind::Select { options: CHANGES_FILTER_MATCHES },
        corvene: Value::text("fuzzy"), ghd: Value::text("fuzzy"),
        familiar: Value::text("fuzzy"), max: Value::text("fuzzy"),
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20555)],
        code: &["crates/corvene-ui/src/changes.rs", "crates/corvene-core/src/filter.rs"],
    },

    /// "Renamed files" in the changes list's Filter Options.
    RENAMED_FILES_FILTER = 705 "renamed-files-filter" {
        title: "\"Renamed files\" filter option",
        summary: "The changes list's Filter Options popover has a sixth option, \"Renamed files\".",
        ghd_behaviour: "Included / excluded, new, modified and deleted files only.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21147)],
        code: &["crates/corvene-ui/src/changes.rs", "crates/corvene-core/src/filter.rs"],
    },

    /// Glob patterns hidden from the changes list.
    CHANGES_HIDE_GLOBS = 706 "changes-hide-globs" {
        title: "Hide files from the changes list",
        summary: "Changed files matching these comma-separated glob patterns (gitignore-like: \
                  `*.lock`, `node_modules`, `/docs/**`) are left out of the changes list, which then \
                  reads \"N of M changed files\". View only: hidden files are still included in \
                  commits. Empty hides nothing.",
        ghd_behaviour: "Lists every changed file.",
        nature: Nature::Feature,
        kind: Kind::Text { placeholder: "*.lock, node_modules", validate: hide_globs },
        corvene: Value::text(""), ghd: Value::text(""),
        familiar: Value::text(""), max: Value::text(""),
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(10093), Upstream::issue(20615), Upstream::issue(21242)],
        code: &["crates/corvene-core/src/filter.rs", "crates/corvene-ui/src/changes.rs"],
    },

    /// ⇧-click keeps ⌘-clicked rows.
    SHIFT_CLICK_KEEPS_SELECTION = 707 "shift-click-keeps-selection" {
        title: "⇧-click keeps ⌘-clicked files",
        summary: "In the changes list, ⇧-click replaces only the range from the last clicked \
                  file; files ⌘-clicked outside it stay selected, as in Finder.",
        ghd_behaviour: "⇧-click selects the range alone and drops the other selected files.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(16355)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-core/src/list_selection.rs"],
    },

    /// A spinner in the Changes header while discarding or refreshing.
    CHANGES_BUSY_INDICATOR = 708 "changes-busy-indicator" {
        title: "Changes list busy indicator",
        summary: "The \"N changed files\" row ends in a spinner while Discard Changes runs and \
                  while a status refresh has been running for more than 300 ms, so a slow \
                  discard or git status is visibly in progress.",
        ghd_behaviour: "Nothing shows that a discard or status refresh is still running.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(15297), Upstream::issue(1914)],
        code: &["crates/corvene-ui/src/changes.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// New untracked files start unticked in the Changes list.
    NEW_UNTRACKED_FILES_EXCLUDED = 709 "new-untracked-files-excluded" {
        title: "New untracked files start unticked",
        summary: "An untracked file that appears in the Changes list starts unticked, so it is \
                  only committed once it is ticked; tracked changes are still included.",
        ghd_behaviour: "Every new file is ticked and goes into the next commit.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(18774), Upstream::issue(21427)],
        code: &["crates/corvene-core/src/dispatcher.rs"],
    },

    /// `status.showUntrackedFiles=no` hides untracked files.
    RESPECT_SHOW_UNTRACKED_FILES = 710 "respect-show-untracked-files" {
        title: "Respect status.showUntrackedFiles",
        summary: "When the repository's git config sets status.showUntrackedFiles to no, the \
                  Changes list leaves untracked files out, as git status does (useful for a home \
                  directory or dotfiles repository). Untracked files then cannot be committed \
                  from Corvene until they are added with git.",
        ghd_behaviour: "Always lists every untracked file (--untracked-files=all).",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(3734)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/status.rs"],
    },

    /// `git status --ignore-submodules`.
    IGNORE_SUBMODULES = 711 "ignore-submodules" {
        title: "Hide submodule changes",
        summary: "What the Changes list leaves out about submodules: nothing beyond each \
                  submodule's own submodule.<name>.ignore setting, changes inside submodules (a \
                  new submodule commit is still listed), or submodules altogether (a new \
                  submodule commit can then not be committed from Corvene).",
        ghd_behaviour: "As configured: only submodule.<name>.ignore hides a submodule.",
        nature: Nature::Feature,
        kind: Kind::Select { options: IGNORE_SUBMODULE_MODES },
        corvene: Value::text("configured"), ghd: Value::text("configured"),
        familiar: Value::text("configured"), max: Value::text("configured"),
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20484)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/status.rs"],
    },

    /// Open in editor / default program acts on every selected file.
    OPEN_MULTIPLE_FILES = 712 "open-multiple-files" {
        title: "Open several files at once",
        summary: "With several changed files selected, \"Open in <editor>\" and \"Open with Default \
                  Program\" open all of them; the changes list's context menu gains \"Open All in \
                  <editor>\" and a history file's menu \"Open All Files of Commit in <editor>\". \
                  At most 25 files at a time.",
        ghd_behaviour: "Opens only the right-clicked file.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(16262), Upstream::issue(21374), Upstream::issue(15013)],
        code: &["crates/corvene-ui/src/changes.rs", "crates/corvene-ui/src/selected_commit.rs"],
    },

    /// "Open With…" in the changes list's file menu.
    OPEN_FILE_WITH = 713 "open-file-with" {
        title: "Open a changed file with…",
        summary: "The changes list's file menu has \"Open With…\" after \"Open with Default \
                  Program\": pick any application to open the file in.",
        ghd_behaviour: "Only the configured editor or the default program.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20166)],
        code: &["crates/corvene-ui/src/changes.rs", "crates/corvene-core/src/integrations.rs"],
    },

    /// "Copy Diff" in the changes list's file menu.
    COPY_DIFF = 714 "copy-diff" {
        title: "Copy Diff",
        summary: "The changes list's file context menu has \"Copy Diff\" (\"Copy Diff of Selected \
                  Files\" for a multi-selection): the working-directory changes of those files as \
                  a patch `git apply` takes, untracked files included.",
        ghd_behaviour: "No way to copy a diff.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(17746)],
        code: &["crates/corvene-ui/src/changes.rs", "crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/diff.rs"],
    },

    /// "Assume Unchanged" in the changes list's menus.
    ASSUME_UNCHANGED = 715 "assume-unchanged" {
        title: "Assume Unchanged",
        summary: "The changes list's file menu has \"Assume Unchanged\" (`git update-index \
                  --assume-unchanged`) for modified or deleted tracked files, which then leave \
                  the list; the list's own menu has \"Stop Assuming Files Unchanged\" to bring \
                  them all back.",
        ghd_behaviour: "No such items; only ignoring (which does not affect tracked files).",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22841)],
        code: &["crates/corvene-ui/src/changes.rs", "crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/commit.rs"],
    },

    /// Warn about paths Windows cannot check out.
    WINDOWS_INVALID_NAMES_WARNING = 716 "windows-invalid-names-warning" {
        title: "Warn about names invalid on Windows",
        summary: "The commit form warns when an included file's path is invalid on Windows (a \
                  reserved name like `CON` or `nul.txt`, a character such as `:` or `?`, or a name \
                  ending in a space or a dot). Committing stays possible.",
        ghd_behaviour: "Commits them silently; Windows clones then fail to check them out.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(19292)],
        code: &["crates/corvene-ui/src/changes.rs", "crates/corvene-core/src/portable_paths.rs"],
    },

    /// Ignore File / Folder / Extension skip rules already in .gitignore.
    IGNORE_SKIPS_EXISTING_RULES = 717 "ignore-skips-existing-rules" {
        title: "Ignore menu items don't duplicate .gitignore rules",
        summary: "\"Ignore File\", \"Ignore Folder\" and \"Ignore All .ext Files\" leave out \
                  patterns the root .gitignore already has as a line.",
        ghd_behaviour: "Appends the pattern again, so repeated use fills .gitignore with duplicate \
                        lines.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(2537)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/ignore.rs"],
    },

    /// Changed-file counts in the "Ignore All .x Files" items.
    IGNORE_MENU_COUNTS = 718 "ignore-menu-counts" {
        title: "Counts in \"Ignore All .x Files\"",
        summary: "The changes list's \"Ignore All .x Files\" context-menu items say how many \
                  changed files have that extension: \"Ignore All .png Files (170 Changed)\".",
        ghd_behaviour: "\"Ignore All .png Files (Add to .gitignore)\", no count.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(13789)],
        code: &["crates/corvene-ui/src/changes.rs"],
    },

    /// "Ignore File In" submenu.
    IGNORE_FILE_TARGETS = 719 "ignore-file-targets" {
        title: "Choose the ignore file",
        summary: "A changed file's context menu adds Ignore File In: the .gitignore of a folder \
                  above the file (anchored to that folder), .git/info/exclude (this clone only) or \
                  the global excludes file (core.excludesFile, else ~/.config/git/ignore; the file \
                  name is ignored in every repository).",
        ghd_behaviour: "Ignore items always write to the .gitignore at the repository root.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(12171), Upstream::issue(16028)],
        code: &["crates/corvene-ui/src/changes.rs", "crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/ignore.rs"],
    },

    /// Discarding a dirty submodule cleans inside it.
    DISCARD_SUBMODULE_CHANGES = 720 "discard-submodule-changes" {
        title: "Discard cleans changes inside submodules",
        summary: "Discarding a submodule that has changes inside checks out its modified files \
                  and moves its untracked files to the Trash, so the submodule is clean \
                  afterwards.",
        ghd_behaviour: "Edits to tracked files inside and a moved submodule commit are reset \
                        (git submodule update --force), but untracked files inside stay, so \
                        such a submodule stays in the list.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(10403)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/commit.rs"],
    },

    /// No Trash sentence when only submodules are discarded.
    DISCARD_SUBMODULE_NO_TRASH_HINT = 721 "discard-submodule-no-trash-hint" {
        title: "Discarding submodules does not mention the Trash",
        summary: "When every discarded entry is a submodule, the discard confirmation leaves out \
                  \"Changes can be restored by retrieving them from the Trash\": nothing is moved \
                  there.",
        ghd_behaviour: "Always promises the Trash.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(10402)],
        code: &["crates/corvene-ui/src/dialogs/discard_changes.rs"],
    },

    /// Discard deletes files instead of moving them to the Trash.
    DISCARD_SKIPS_TRASH = 722 "discard-skips-trash" {
        title: "Discard deletes instead of using the Trash",
        summary: "Discarding changes deletes new and untracked files (and untracked files inside \
                  a discarded submodule) permanently instead of moving them to the Trash; the \
                  confirmation says they cannot be restored.",
        ghd_behaviour: "Always moves discarded files to the Trash.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(10445)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-ui/src/dialogs/discard_changes.rs"],
    },

    /// Snooze the discard confirmation.
    DISCARD_CONFIRM_SNOOZE = 723 "discard-confirm-snooze" {
        title: "Snooze the discard confirmation",
        summary: "The Confirm Discard Changes dialog offers \"Do not show this message again for N \
                  minutes\": discarding in that repository then skips the confirmation for N \
                  minutes (this session; Discard All Changes still asks). 0 hides the option.",
        ghd_behaviour: "Only \"Do not show this message again\", for good.",
        nature: Nature::Feature,
        kind: Kind::Number { min: 0, max: 120, unit: Some("min") },
        corvene: Value::Number(0), ghd: Value::Number(0),
        familiar: Value::Number(0), max: Value::Number(10),
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20747)],
        code: &["crates/corvene-ui/src/dialogs/discard_changes.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// "No local changes" offers Open in <Shell>.
    NO_CHANGES_OPEN_IN_SHELL = 724 "no-changes-open-in-shell" {
        title: "No local changes: Open in shell",
        summary: "The \"No local changes\" view adds an \"Open the repository in <Shell>\" \
                  suggestion after Show in Finder.",
        ghd_behaviour: "Editor, Finder and GitHub suggestions only.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(12453)],
        code: &["crates/corvene-ui/src/workspace.rs"],
    },

    /// "No local changes" links the branch's open pull request.
    NO_CHANGES_VIEW_PULL_REQUEST = 725 "no-changes-view-pull-request" {
        title: "\"View Pull Request\" when there are no local changes",
        summary: "While the current branch has an open pull request, the \"No local changes\" view \
                  leads with a \"View Pull Request\" card naming its number and title, opening it \
                  on GitHub.",
        ghd_behaviour: "Shows no pull request action while one is open (only Create / Preview \
                        Pull Request for a branch without one).",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(19329)],
        code: &["crates/corvene-ui/src/workspace.rs"],
    },

    /// "No local changes" offers restoring the branch's stash.
    RESTORE_STASH_SUGGESTION = 726 "restore-stash-suggestion" {
        title: "Restore stash from No local changes",
        summary: "When the branch has stashed changes and nothing else is changed, the \"No local \
                  changes\" view starts with a \"Restore your stashed changes\" card whose Restore \
                  button brings them back in one click.",
        ghd_behaviour: "The stash has to be opened from the bottom of the Changes tab first.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(12864)],
        code: &["crates/corvene-ui/src/workspace.rs"],
    },

    /// A dot on the Changes tab while History shows and the branch has a stash.
    STASH_DOT_ON_CHANGES_TAB = 727 "stash-dot-on-changes-tab" {
        title: "Stash dot on the Changes tab",
        summary: "While the History tab is showing, the Changes tab has a blue dot when the \
                  current branch has stashed changes.",
        ghd_behaviour: "The stash is only visible at the bottom of the Changes tab.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(8589)],
        code: &["crates/corvene-ui/src/workspace.rs", "crates/corvene-ui/src/tab_bar.rs"],
    },

    /// Show the newest command-line stash when the branch has no Desktop stash.
    SHOW_LATEST_OTHER_STASH = 728 "show-latest-other-stash" {
        title: "Show stashes made outside Corvene",
        summary: "When the current branch has no stash of its own, the Changes list's Stashed \
                  Changes row shows the newest stash that GitHub Desktop or Corvene did not make \
                  (git stash on the command line), so it can be viewed, restored or discarded. \
                  Stashing from Corvene never replaces such a stash.",
        ghd_behaviour: "Only stashes named !!GitHub_Desktop<branch> are shown; others are invisible.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(17147)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-core/src/state.rs"],
    },

    /// Restore a branch's stash when switching back to it with no changes.
    POP_STASH_ON_RETURN = 729 "pop-stash-on-return" {
        title: "Restore a branch's stash when returning to it",
        summary: "Switching to a branch that has stashed changes restores them (git stash pop) \
                  when the working directory is clean after the switch, e.g. when the changes on \
                  the branch being left were stashed there. A pop that conflicts keeps the stash \
                  and shows the conflicts.",
        ghd_behaviour: "The stash stays until Stashed Changes › Restore is clicked.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(17682)],
        code: &["crates/corvene-core/src/dispatcher.rs"],
    },

    /// Commit form warning while HEAD is detached.
    DETACHED_HEAD_COMMIT_WARNING = 730 "detached-head-commit-warning" {
        title: "Warn when committing on a detached HEAD",
        summary: "While HEAD is detached the commit form shows a warning that the commit will not \
                  be on any branch, with a link to create one.",
        ghd_behaviour: "Commits on a detached HEAD without a word; the button reads \"Commit to\".",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(788)],
        code: &["crates/corvene-ui/src/changes.rs"],
    },

    /// ↑ / ↓ in an empty commit summary recall recent commit messages.
    RECALL_COMMIT_MESSAGES = 731 "recall-commit-messages" {
        title: "Recall recent commit messages with ↑ / ↓",
        summary: "In an empty commit form, ↑ in the summary fills in the summary and description \
                  of the latest commit on the branch; more ↑ go further back (merges and repeated \
                  summaries skipped), ↓ comes forward and past the newest empties the form again. \
                  Editing the text keeps it.",
        ghd_behaviour: "↑ / ↓ only move the caret.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(12927), Upstream::issue(20559), Upstream::issue(17525)],
        code: &["crates/corvene-ui/src/changes.rs", "crates/corvene-models/src/lib.rs"],
    },

    /// Confirmation before committing on the default branch.
    CONFIRM_COMMIT_TO_DEFAULT_BRANCH = 732 "confirm-commit-to-default-branch" {
        title: "Confirm commits to the default branch",
        summary: "Committing (not amending) while the default branch is checked out asks \
                  \"Commit to Default Branch\" first.",
        ghd_behaviour: "Commits to the default branch without asking.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21857)],
        code: &["crates/corvene-ui/src/changes.rs", "crates/corvene-ui/src/dialogs/confirm_commit_to_default_branch.rs"],
    },

    /// Hard 72-character limit on the commit summary.
    SUMMARY_MAX_LENGTH = 733 "summary-max-length" {
        title: "Limit the commit summary to 72 characters",
        summary: "The commit summary field takes at most 72 characters (GitHub truncates longer \
                  summaries); typing or pasting past the limit drops the excess, like an HTML \
                  maxlength.",
        ghd_behaviour: "No limit.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(18290)],
        code: &["crates/corvene-ui/src/changes.rs"],
    },

    /// "Committing as" line above the commit summary.
    COMMIT_AUTHOR_LINE = 734 "commit-author-line" {
        title: "Show the commit author",
        summary: "A line above the commit summary names the identity git resolved for this \
                  repository (`user.name` / `user.email`, `includeIf` included): \
                  \"Committing as Name <email>\".",
        ghd_behaviour: "Only the avatar, whose tooltip names the author.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21883)],
        code: &["crates/corvene-ui/src/changes.rs"],
    },

    /// "Name <email>" co-authors without a GitHub account.
    FREE_FORM_CO_AUTHORS = 735 "free-form-co-authors" {
        title: "Co-authors without a GitHub account",
        summary: "Typing \"Name <email>\" in the co-authors box adds that person as a co-author \
                  (a Co-Authored-By trailer) without looking them up on GitHub. To allow spaces \
                  in names, Space turns a typed word into a GitHub handle only when it starts \
                  with @; the suggestions list works as before.",
        ghd_behaviour: "Only GitHub users can be added; every word becomes a handle on Space.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(4308)],
        code: &["crates/corvene-ui/src/changes.rs", "crates/corvene-core/src/autocomplete.rs"],
    },

    /// Commit form gear › Push After Committing.
    COMMIT_AND_PUSH = 736 "commit-and-push" {
        title: "Push after committing",
        summary: "The commit form's gear menu adds Push After Committing (kept per repository); \
                  while it is ticked the button reads \"Commit and push to main\" and the branch \
                  is pushed (or published) once the commit, hooks included, succeeds. Push errors \
                  show as for the toolbar button; amended commits are not pushed.",
        ghd_behaviour: "Committing and pushing are separate steps.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21874), Upstream::issue(22742)],
        code: &["crates/corvene-ui/src/changes.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// Optional tag field in the commit form.
    COMMIT_TAG_FIELD = 737 "commit-tag-field" {
        title: "Tag field in the commit form",
        summary: "The commit form has a \"Tag (optional)\" field under the description; a name \
                  there tags the new commit once it is made (not when amending).",
        ghd_behaviour: "Tags are created from History after committing.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(16256)],
        code: &["crates/corvene-ui/src/changes.rs"],
    },

    /// Clear the drafted message after a matching outside commit.
    CLEAR_MESSAGE_AFTER_OUTSIDE_COMMIT = 738 "clear-message-after-outside-commit" {
        title: "Clear the draft after an outside commit",
        summary: "When a new commit appears on the branch (made on the command line or in another \
                  app) whose summary is the one drafted in the commit form, the form is cleared \
                  as after committing in Corvene.",
        ghd_behaviour: "The drafted message stays.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(5233)],
        code: &["crates/corvene-ui/src/changes.rs"],
    },

    /// Context menu on the "Committed … Undo" bar.
    UNDO_BAR_MENU = 739 "undo-bar-menu" {
        title: "Context menu on the undo bar",
        summary: "Right-clicking the \"Committed just now … Undo\" bar under the commit button \
                  offers Amend Commit…, Undo Commit…, Create Tag…, Copy SHA and View on GitHub \
                  for that commit.",
        ghd_behaviour: "No context menu there; those items live in History.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(12561), Upstream::issue(19938)],
        code: &["crates/corvene-ui/src/changes.rs"],
    },

    /// Spinner while a working-directory diff loads.
    DIFF_LOADING_INDICATOR = 740 "diff-loading-indicator" {
        title: "Diff loading spinner",
        summary: "When a changed file's diff takes more than 300 ms to compute (large files, slow \
                  filters), a spinner covers the diff pane until it is ready.",
        ghd_behaviour: "The previous diff (or an empty pane) stays up with no sign of work.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(1913)],
        code: &["crates/corvene-ui/src/diff_view.rs"],
    },

    /// The diff's "Open in <Editor> at Line N".
    DIFF_OPEN_IN_EDITOR_AT_LINE = 741 "diff-open-in-editor-at-line" {
        title: "Diff: Open in editor at a line",
        summary: "Right-clicking a line of a working-directory diff offers \"Open in <Editor> at \
                  Line N\" when the editor can jump to a line (VS Code and its forks, Sublime \
                  Text, Zed; on Android the Termux editors and Markor), and ⌥-clicking (Alt-click) \
                  a line's text opens it there directly.",
        ghd_behaviour: "The diff's context menu has no editor item; Open in <Editor> opens the \
                        file at its top.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(14476), Upstream::issue(20254)],
        code: &["crates/corvene-ui/src/diff_view.rs", "crates/corvene-platform/src/editors.rs", "crates/corvene-core/src/integrations.rs"],
    },

    /// "The file mode changed" for a mode-only diff.
    FILE_MODE_CHANGE_MESSAGE = 742 "file-mode-change-message" {
        title: "Say when only the file mode changed",
        summary: "A diff whose only change is the file mode (e.g. the executable bit) says \
                  \"The file mode changed from 100644 to 100755\".",
        ghd_behaviour: "\"No content changes found\", or \"Only whitespace changes found\" while \
                        whitespace changes are hidden.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(11685), Upstream::issue(557)],
        code: &["crates/corvene-git/src/diff.rs", "crates/corvene-ui/src/diff_view.rs"],
    },

    /// A renamed file's diff starts from HEAD.
    RENAMED_DIFF_AGAINST_HEAD = 743 "renamed-diff-against-head" {
        title: "Renamed files diff against the last commit",
        summary: "A renamed file's diff compares the old path in the last commit with the \
                  working copy, so edits staged outside Corvene show up too.",
        ghd_behaviour: "Compares the index with the working copy: a renamed file whose edits \
                        were staged (e.g. by `git add`) shows no changes.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(19142), Upstream::issue(5575)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/diff.rs"],
    },

    /// Split mode shows added and deleted files unified.
    UNIFIED_DIFF_FOR_ADDED_FILES = 744 "unified-diff-for-added-files" {
        title: "Added and deleted files use the unified layout",
        summary: "With Diff Settings › Split selected, a new or deleted file is still shown \
                  unified, across the whole width, instead of beside an empty column.",
        ghd_behaviour: "Split mode draws a new file in the right half next to an empty left \
                        half (a deleted one the other way round).",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(13763), Upstream::issue(16610)],
        code: &["crates/corvene-ui/src/diff_view.rs"],
    },

    /// A symbolic link's contents are its target path.
    SYMLINK_CONTENTS = 745 "symlink-contents" {
        title: "Symbolic links are not followed",
        summary: "Loading a changed symbolic link reads the path it points to, as git records \
                  it, instead of the file behind it.",
        ghd_behaviour: "Reads the file the link points to for hunk expansion, so a link to a \
                        pipe or device keeps the diff loading forever and a link to a huge file \
                        loads it whole.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(18620)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/diff.rs"],
    },

    /// Intra-line highlights end on grapheme boundaries.
    INTRA_LINE_GRAPHEMES = 746 "intra-line-graphemes" {
        title: "Intra-line highlights keep accents with their letters",
        summary: "The changed part of a modified line is widened to whole characters as \
                  people see them (grapheme clusters), so a combining accent is highlighted \
                  together with its letter.",
        ghd_behaviour: "Compares UTF-16 code units, so the highlight can cut a combining mark off \
                        its base character and the mark renders apart or disappears.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(11492)],
        code: &["crates/corvene-ui/src/diff_view.rs", "crates/corvene-ui/src/diff_view_rows.rs"],
    },

    /// The intra-line highlighting length cap.
    INTRA_LINE_MAX_LENGTH = 747 "intra-line-max-length" {
        title: "Longest line with intra-line highlighting",
        summary: "A modified line pair gets its changed characters highlighted only while both \
                  lines are shorter than this many bytes (0: no limit).",
        ghd_behaviour: "1024 (`MaxIntraLineDiffStringLength`), fixed; longer lines only show as \
                        wholly replaced.",
        nature: Nature::Feature,
        kind: Kind::Number { min: 0, max: 1_000_000, unit: Some("bytes") },
        corvene: Value::Number(1024), ghd: Value::Number(1024),
        familiar: Value::Number(1024), max: Value::Number(1024),
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22556)],
        code: &["crates/corvene-ui/src/diff_view.rs", "crates/corvene-ui/src/diff_view_rows.rs"],
    },

    /// Visible whitespace in diffs.
    DIFF_SHOW_WHITESPACE = 748 "diff-show-whitespace" {
        title: "Show whitespace in diffs",
        summary: "Diff lines mark every space with a faint dot and every tab with a faint line, \
                  so indentation and trailing whitespace changes can be told apart.",
        ghd_behaviour: "Whitespace is invisible.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(12974)],
        code: &["crates/corvene-ui/src/diff_view.rs", "crates/corvene-ui/src/diff_view_rows.rs"],
    },

    /// "Show the diff as text anyway" on binary files.
    BINARY_DIFF_AS_TEXT = 749 "binary-diff-as-text" {
        title: "Show binary files' diffs as text",
        summary: "A changed file git takes for binary (a stray NUL byte, an odd encoding) offers \
                  \"Show the diff as text anyway.\", which diffs it line by line with \
                  `git diff --text`. Its lines cannot be picked for a partial commit.",
        ghd_behaviour: "\"This binary file has changed.\" and a link to open it elsewhere.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(16855)],
        code: &["crates/corvene-ui/src/diff_view.rs", "crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/diff.rs"],
    },

    /// Diffs open with the whole file expanded.
    DIFF_EXPAND_WHOLE_FILE = 750 "diff-expand-whole-file" {
        title: "Expand the whole file in diffs",
        summary: "Every text diff opens as if \"Expand Whole File\" had been picked (files up \
                  to 20 000 lines; large diffs stay collapsed). \"Collapse Expanded Lines\" \
                  still collapses it.",
        ghd_behaviour: "Diffs open collapsed to their hunks; the expansion is per file and \
                        forgotten.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(16140), Upstream::issue(20548)],
        code: &["crates/corvene-ui/src/diff_view.rs"],
    },

    /// The diff's font size.
    DIFF_FONT_SIZE = 751 "diff-font-size" {
        title: "Diff font size",
        summary: "The size of the diff's monospace text, 9 to 16 pixels at 100 % zoom (0 \
                  keeps 11 px). Rows stay 20 px tall.",
        ghd_behaviour: "11 px, changed only by zooming the whole window.",
        nature: Nature::Feature,
        kind: Kind::Number { min: 0, max: 16, unit: Some("px") },
        corvene: Value::Number(0), ghd: Value::Number(0),
        familiar: Value::Number(0), max: Value::Number(0),
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22929)],
        code: &["crates/corvene-ui/src/diff_view.rs"],
    },

    /// Image diff borders go around the image.
    IMAGE_DIFF_BORDER_OUTSIDE = 752 "image-diff-border-outside" {
        title: "Image diff borders don't shrink the image",
        summary: "The coloured 1 px border of an image in the image diff is drawn around the \
                  image, which keeps its natural (or fitted) size.",
        ghd_behaviour: "The border is inside the image's box (`box-sizing: border-box`), so every \
                        image is drawn 2 px smaller than its size, blurring small images and \
                        pixel art.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(14469)],
        code: &["crates/corvene-ui/src/image_diff.rs"],
    },

    /// The image diff's checkerboard.
    IMAGE_DIFF_BACKGROUND = 753 "image-diff-background" {
        title: "Image diff background",
        summary: "The checkerboard behind images in the image diff: light, dark, or dark while \
                  the app theme is dark, so light and translucent images stay visible.",
        ghd_behaviour: "Always the light checkerboard.",
        nature: Nature::Feature,
        kind: Kind::Select { options: IMAGE_DIFF_BACKGROUNDS },
        corvene: Value::text("light"), ghd: Value::text("light"),
        familiar: Value::text("light"), max: Value::text("theme"),
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21092)],
        code: &["crates/corvene-ui/src/image_diff.rs"],
    },

    /// How Swipe / Onion Skin / Difference align images of different sizes.
    IMAGE_DIFF_ALIGNMENT = 754 "image-diff-alignment" {
        title: "Image diff alignment",
        summary: "Where Swipe, Onion Skin and Difference put two images of different sizes: \
                  centred on each other, or with their top left corners together (for layouts \
                  that grow to the right and down, such as UI snapshots).",
        ghd_behaviour: "Always centred.",
        nature: Nature::Feature,
        kind: Kind::Select { options: IMAGE_DIFF_ALIGNMENTS },
        corvene: Value::text("centre"), ghd: Value::text("centre"),
        familiar: Value::text("centre"), max: Value::text("centre"),
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(19385)],
        code: &["crates/corvene-ui/src/image_diff.rs"],
    },

    /// TGA images get an image diff.
    TGA_IMAGE_DIFF = 755 "tga-image-diff" {
        title: "Image diffs for TGA files",
        summary: "Changed `.tga` images (common in game assets) are shown in the image diff \
                  (2-up, Swipe, Onion Skin, Difference).",
        ghd_behaviour: "\"This binary file has changed.\"",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21970)],
        code: &["crates/corvene-ui/src/image_diff.rs", "crates/corvene-ui/src/diff_view.rs", "crates/corvene-models/src/lib.rs"],
    },

    /// A hint above a diff nothing highlights.
    MISSING_HIGHLIGHTING_HINT = 756 "missing-highlighting-hint" {
        title: "Hint for files without syntax highlighting",
        summary: "A diff of a file no grammar covers gets one line above it: \"No syntax \
                  highlighting for .foo\" with Find an extension…, which opens Language \
                  extensions with the registries' candidates for that suffix. A × dismisses the \
                  hint for that suffix.",
        ghd_behaviour: "Such a diff is shown without colours and without comment.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-ui/src/diff_view.rs", "crates/corvene-core/src/extensions.rs"],
    },

    /// Type changes (file to symbolic link) are parsed as two sections.
    TYPECHANGE_DIFF = 757 "typechange-diff" {
        title: "Type changes in the diff",
        summary: "When a file becomes a symbolic link (or the other way round), the diff says \
                  so above the rows and its lines can be neither selected nor expanded, since \
                  no partial commit can describe half of such a change. (The second file \
                  section's `---` / `+++` headers are never shown as content, flag or not.)",
        ghd_behaviour: "The diff parser fails on the second `diff --git` line and the diff keeps \
                        loading forever.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21975), Upstream::issue(12142)],
        code: &["crates/corvene-git/src/diff.rs", "crates/corvene-ui/src/diff_view.rs"],
    },

    /// The diff's row height (and the changes list's with it).
    DIFF_LINE_HEIGHT = 758 "diff-line-height" {
        title: "Diff line height",
        summary: "The height of the diff's rows, 14 to 32 pixels at 100 % zoom (0 keeps \
                  20 px), for a denser or roomier diff. The changes list's rows follow, 9 px \
                  taller (29 px with the default).",
        ghd_behaviour: "20 px diff rows and 29 px changes rows, changed only by zooming the \
                        whole window.",
        nature: Nature::Feature,
        kind: Kind::Number { min: 0, max: 32, unit: Some("px") },
        corvene: Value::Number(0), ghd: Value::Number(0),
        familiar: Value::Number(0), max: Value::Number(0),
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(19361), Upstream::issue(20480)],
        code: &["crates/corvene-ui/src/diff_view.rs", "crates/corvene-ui/src/changes.rs"],
    },

    /// The old-line-number column selects the whole hunk.
    WIDE_HUNK_HANDLE = 759 "wide-hunk-handle" {
        title: "Wide hunk handle",
        summary: "In the unified diff of the Changes tab, clicking the old line number column \
                  ticks or unticks the whole block of changes, like the thin handle strip to its \
                  left; the new line number column still selects single lines (and drags \
                  ranges).",
        ghd_behaviour: "Only the 16 px strip selects a block; both number columns select single \
                        lines.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20945)],
        code: &["crates/corvene-ui/src/diff_view_rows.rs", "crates/corvene-ui/src/diff_view.rs"],
    },

    /// Discard items in the diff text's context menu.
    DISCARD_FROM_TEXT_MENU = 760 "discard-from-text-menu" {
        title: "Discard lines from the text menu",
        summary: "Right-clicking the text of an added or removed line in the Changes tab's diff \
                  offers \"Discard Added Line\" (and \"Discard Added Lines\" for its whole block), \
                  the items the line number gutter has, below Copy and Select All. When the \
                  selected text spans several added or removed lines, \"Discard N Selected \
                  Lines\" discards exactly those.",
        ghd_behaviour: "Discarding lines needs a right-click on the line numbers; the text's menu \
                        has Copy, Select All and the expansion item only.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(14279), Upstream::issue(16415)],
        code: &["crates/corvene-ui/src/diff_view.rs", "crates/corvene-ui/src/diff_view_rows.rs"],
    },

    /// Small images are enlarged with sharp pixels.
    PIXELATED_SMALL_IMAGES = 761 "pixelated-small-images" {
        title: "Enlarge small images in image diffs",
        summary: "Images under 64 pixels (pixel art, icons) are shown at a whole multiple of \
                  their size, up to 16× and about 256 px, with sharp nearest-neighbour pixels \
                  (still shrunk to fit the pane). The footer keeps the real size.",
        ghd_behaviour: "Small images are drawn at their natural size, a few pixels across.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21531)],
        code: &["crates/corvene-ui/src/image_diff.rs"],
    },

    /// A way out of "The diff is too large to be displayed".
    TOO_LARGE_DIFF_ESCAPE_HATCH = 762 "too-large-diff-escape-hatch" {
        title: "Open diffs too large to show elsewhere",
        summary: "When a changed file's diff is too large to be displayed (over 70 MB), the \
                  pane offers \"Open in external diff tool\" (git difftool, when diff.tool is set \
                  in the Git configuration) and \"Open file in <Editor>\".",
        ghd_behaviour: "\"The diff is too large to be displayed.\" and nothing else.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(4053)],
        code: &["crates/corvene-ui/src/diff_view.rs", "crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/diff.rs"],
    },

    /// The working file's modification time in the Changes diff header.
    DIFF_HEADER_MTIME = 763 "diff-header-mtime" {
        title: "Modification time in the diff header",
        summary: "The Changes tab's diff header shows when the selected file was last modified \
                  (\"Modified 5 minutes ago\", the date and time in its tooltip).",
        ghd_behaviour: "The header shows the path only.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(18300)],
        code: &["crates/corvene-ui/src/diff_view.rs", "crates/corvene-ui/src/workspace.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// Stop computing a diff nobody will see.
    CANCEL_STALE_DIFFS = 764 "cancel-stale-diffs" {
        title: "Stop stale diffs",
        summary: "Selecting another changed file (or a refresh) stops the git diff still running \
                  for the previous selection, so moving quickly through large files does not \
                  queue up diffs that are thrown away.",
        ghd_behaviour: "Every started diff runs to completion and the stale ones are discarded.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(1915)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/diff.rs", "crates/corvene-git/src/process.rs"],
    },

    /// Highlighting for .jsonc, .slnx and MQL files.
    MORE_HIGHLIGHT_EXTENSIONS = 765 "more-highlight-extensions" {
        title: "Highlight more file types",
        summary: "GitHub Desktop's highlighter also colours `.jsonc` as JSON, `.slnx` (Visual \
                  Studio solutions) as XML and MetaQuotes `.mq4` / `.mq5` / `.mqh` sources as \
                  C++. (The tree-sitter grammars know these extensions either way.)",
        ghd_behaviour: "These files are not highlighted.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22663), Upstream::issue(20861)],
        code: &["crates/corvene-highlight/src/cm/modes/mod.rs", "crates/corvene/src/main.rs", "crates/corvene-grammars/src/grammars.rs"],
    },

    /// Commit messages survive switching repositories and restarts.
    PERSIST_COMMIT_DRAFTS = 766 "persist-commit-drafts" {
        title: "Keep commit message drafts",
        summary: "Each repository keeps its own unfinished commit summary and description: \
                  switching to another repository and back, or quitting and reopening Corvene, \
                  brings it back. A commit clears it.",
        ghd_behaviour: "Drafts are kept per repository in memory only and are gone after a \
                        restart.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(3664)],
        code: &["crates/corvene-core/src/drafts.rs", "crates/corvene-ui/src/changes.rs"],
    },

    /// Unticked files stay unticked after a restart.
    PERSIST_FILE_SELECTION = 767 "persist-file-selection" {
        title: "Remember unticked files",
        summary: "Files unticked in a repository's changes list are still unticked after \
                  quitting and reopening Corvene (whole files only; a partly selected file comes \
                  back fully ticked).",
        ghd_behaviour: "After a restart every changed file is ticked again.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(5870), Upstream::issue(17788)],
        code: &["crates/corvene-core/src/drafts.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// "Ignore with Pattern…" in the changes file menu.
    IGNORE_CUSTOM_PATTERN = 768 "ignore-custom-pattern" {
        title: "Ignore with a pattern",
        summary: "A changed file's context menu has \"Ignore with Pattern…\": a small dialog \
                  prefilled with the file's path where the pattern can be edited (`build/*.log`, \
                  `**/tmp/`) before it is added to the repository's .gitignore.",
        ghd_behaviour: "Only fixed choices: the file, one of its folders or its extension.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(10017)],
        code: &["crates/corvene-ui/src/changes.rs", "crates/corvene-ui/src/dialogs/ignore_with_pattern.rs"],
    },

    /// Refuse yourself and duplicates as co-authors.
    CO_AUTHOR_VALIDATION = 769 "co-author-validation" {
        title: "Check co-authors when adding them",
        summary: "Adding yourself (your account's login or emails, or `user.email`) or someone \
                  who is already a co-author removes what was typed and says why under the \
                  co-authors box, instead of adding a token that does nothing.",
        ghd_behaviour: "Your own handle is accepted (and ends up in a Co-authored-by trailer), \
                        and a co-author can be typed again as a second token.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21736)],
        code: &["crates/corvene-ui/src/changes.rs"],
    },

    /// Co-author suggestions from the repository's commit authors.
    CO_AUTHORS_FROM_HISTORY = 770 "co-authors-from-history" {
        title: "Suggest co-authors from history",
        summary: "The co-authors box also suggests the authors of the repository's last 500 \
                  commits (name and email, read once per session), for people without a GitHub \
                  account or outside the repository's collaborators; typing a name or email \
                  without @ suggests them too.",
        ghd_behaviour: "Only the GitHub repository's mentionable users are suggested, after @.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(9468)],
        code: &["crates/corvene-core/src/autocomplete.rs", "crates/corvene-ui/src/autocompletion.rs", "crates/corvene-ui/src/changes.rs", "crates/corvene-git/src/log.rs"],
    },

    /// The no-changes Pull card names a rebasing pull like the toolbar does.
    BLANK_SLATE_PULL_SAYS_REBASE = 771 "blank-slate-pull-says-rebase" {
        title: "\"Pull with rebase\" on the no-changes card",
        summary: "When `pull.rebase` is set, the \"Pull origin\" button of the no-changes view \
                  says \"Pull origin with rebase\", as the toolbar's pull button already does.",
        ghd_behaviour: "The card's button says \"Pull origin\" whatever the pull does.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(7101)],
        code: &["crates/corvene-ui/src/no_changes.rs"],
    },

    /// Pick the editor from the no-changes "Open in" card.
    EDITOR_PICKER_DROPDOWN = 772 "editor-picker-dropdown" {
        title: "Editor menu on the no-changes card",
        summary: "With more than one editor installed, a ▾ next to the no-changes view's \"Open \
                  in <editor>\" button lists them; picking one makes it the external editor (the \
                  repository's own one when it has one) and opens the repository in it.",
        ghd_behaviour: "The button opens the editor chosen in Settings only.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20330)],
        code: &["crates/corvene-ui/src/workspace.rs", "crates/corvene-ui/src/no_changes.rs"],
    },

    /// A merge commit can show only its conflict resolutions.
    MERGE_REMERGE_DIFF = 773 "merge-remerge-diff" {
        title: "Show a merge's conflict resolutions",
        summary: "When one merge commit is selected in History, a toggle at the end of its file \
                  list header switches to what the merge changed beyond git's automatic merge of \
                  its parents (git show --remerge-diff): the conflict resolutions and any other \
                  edit made while merging, or \"Merged cleanly\" when there are none. Needs git \
                  2.36 or newer.",
        ghd_behaviour: "A merge commit is diffed against its first parent only, so it lists \
                        everything the merged branch brought in.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(386)],
        code: &["crates/corvene-git/src/log.rs", "crates/corvene-core/src/dispatcher.rs", "crates/corvene-ui/src/selected_commit.rs"],
    },

    /// Conflicts left by restoring a stash can be resolved, and the stash is kept.
    STASH_CONFLICT_FLOW = 774 "stash-conflict-flow" {
        title: "Resolve conflicts from restoring a stash",
        summary: "When restoring a stash (Restore, bringing changes to another branch, or the \
                  restore on returning to a branch) conflicts with the files, git's kept copy of \
                  the stash stays and the commit form gives way to a list of the conflicted \
                  files: Open in your editor or merge tool, then Mark as Resolved (git reset on \
                  that file: the index only). Once every file is resolved Corvene asks whether \
                  to drop the stash or keep it.",
        ghd_behaviour: "The stash is dropped as if it had applied cleanly; the files stay \
                        conflicted in git's index with no way to mark them resolved, so a later \
                        pull fails even after the markers are gone.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(13542), Upstream::issue(11959)],
        code: &["crates/corvene-git/src/stash_ops.rs", "crates/corvene-core/src/stash_flows.rs", "crates/corvene-ui/src/stash_conflicts.rs"],
    },

    /// Files that were untracked when stashed come back untracked.
    STASH_RESTORE_UNSTAGES_NEW_FILES = 775 "stash-restore-unstages-new-files" {
        title: "Restored stashes leave new files untracked",
        summary: "A stash made by Corvene or GitHub Desktop includes the untracked files by \
                  adding them to git's index first. After a restore (Restore, bringing changes \
                  to another branch, returning to a branch) the files the stash added are \
                  unstaged again, so they are untracked as before and a branch whose .gitignore \
                  ignores them no longer lists them.",
        ghd_behaviour: "Restored files come back staged as new files, so they stay tracked even \
                        where .gitignore ignores them, until git reset is run.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(17883)],
        code: &["crates/corvene-git/src/stash_ops.rs", "crates/corvene-core/src/stash_flows.rs"],
    },

    /// Changes can be added to the branch's stash instead of replacing it.
    STASH_ADD_TO_EXISTING = 776 "stash-add-to-existing" {
        title: "Add changes to an existing stash",
        summary: "Overwrite Stash? (Stash All Changes, or switching branches while leaving the \
                  changes) has an Add to Stash button, and \"Unable to … when changes are \
                  present\" offers Add to Stash and Continue when the branch already has a \
                  stash. The stash is applied onto the current changes in a scratch copy first; \
                  when that is clean, one stash holding both replaces the old one. When the two \
                  change the same lines nothing is touched and Corvene names the files. Needs \
                  git 2.40 or newer.",
        ghd_behaviour: "A branch holds one stash: stashing again overwrites it, and the \
                        overwritten dialog of a pull or cherry-pick offers only Close.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(18539)],
        code: &["crates/corvene-git/src/stash_ops.rs", "crates/corvene-core/src/stash_flows.rs", "crates/corvene-ui/src/dialogs/branch_dialogs.rs", "crates/corvene-ui/src/dialogs/mco_dialogs.rs"],
    },

    /// The changes list can stash only the selected files.
    STASH_SELECTED_FILES = 777 "stash-selected-files" {
        title: "Stash selected files",
        summary: "A changed file's menu has Stash File, or Stash N Selected Files for a \
                  selection: those files (new ones included) go into the branch's stash and \
                  the other changes stay. The branch keeps one stash, so the item is disabled, \
                  saying why, while the branch already has one.",
        ghd_behaviour: "Only all changes can be stashed (Stash All Changes).",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(11531), Upstream::issue(14859)],
        code: &["crates/corvene-ui/src/changes.rs", "crates/corvene-core/src/stash_flows.rs", "crates/corvene-git/src/stash_ops.rs"],
    },

    /// "Unable to … when changes are present" can discard the files and go on.
    OVERWRITTEN_DISCARD_AND_CONTINUE = 778 "overwritten-discard-and-continue" {
        title: "Discard changes and continue",
        summary: "When a pull, cherry-pick, squash, reorder or rebase stops because of local \
                  changes, the dialog listing the files has Discard Changes and Continue: those \
                  files are discarded as Discard Changes does (new files go to the Trash) and \
                  the operation runs again.",
        ghd_behaviour: "The dialog offers Stash Changes and Continue (none when the branch \
                        already has a stash) or Close.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(17507)],
        code: &["crates/corvene-ui/src/dialogs/mco_dialogs.rs", "crates/corvene-core/src/stash_flows.rs"],
    },

    /// Programmatic commit message replacements keep the fields' undo history.
    UNDOABLE_COMMIT_MESSAGE_REPLACE = 779 "undoable-commit-message-replace" {
        title: "Undo a replaced commit message",
        summary: "When Corvene puts a message in the commit form (starting to amend a commit, \
                  Undo Commit, the commit template, emptying the form after a commit) the \
                  replacement is an edit like typing: ⌘Z / Ctrl+Z brings the text that was \
                  there back.",
        ghd_behaviour: "The fields' undo history is dropped, so a draft overwritten by Amend \
                        Commit is lost.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(17822)],
        code: &["crates/corvene-ui/src/changes.rs"],
    },

    /// Commit options gear › Amend Last Commit.
    AMEND_FROM_COMMIT_OPTIONS = 780 "amend-from-commit-options" {
        title: "Amend from the commit options",
        summary: "The commit form's gear menu has an \"Amend Last Commit\" checkbox: ticking it \
                  starts amending the latest commit as History's Amend Commit… does (with its \
                  warnings), unticking it stops amending.",
        ghd_behaviour: "Amending starts only from the latest commit's menu in History.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(17222)],
        code: &["crates/corvene-ui/src/changes.rs"],
    },

    /// Commits keep executable bits staged with `update-index --chmod`.
    KEEP_STAGED_MODE_CHANGES = 781 "keep-staged-mode-changes" {
        title: "Keep staged executable bits when committing",
        summary: "While core.fileMode is false (the default on Windows, where files have no \
                  executable bit), an executable bit set with git update-index --chmod=+x (or \
                  removed with -x) is put back into the index after Corvene restages the files, \
                  so the commit keeps it.",
        ghd_behaviour: "Committing unstages and restages every file from the working tree, \
                        which drops such a mode change.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(3870)],
        code: &["crates/corvene-git/src/commit.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// Commit or amend at a rebase's `edit` stop.
    COMMIT_DURING_REBASE_EDIT = 782 "commit-during-rebase-edit" {
        title: "Commit at a rebase's edit stop",
        summary: "When an interactive rebase stops at an edit (and nothing is conflicted), the \
                  changes list keeps the commit form, so the stopped commit can be amended or \
                  new commits made, with a \"Continue rebase\" button under it.",
        ghd_behaviour: "During any rebase the commit form gives way to a single \"Continue \
                        rebase\" button, so an edit stop can only be continued.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(10460)],
        code: &["crates/corvene-ui/src/changes.rs", "crates/corvene-git/src/status.rs"],
    },

    /// Amend mode can change the commit's author.
    AMEND_AUTHOR = 783 "amend-author" {
        title: "Change the author when amending",
        summary: "While amending, the commit form shows the commit's author as an editable \
                  \"Name <email>\" field (it replaces the 734 author line) with \"Reset to my \
                  identity\": a changed author is passed as git commit --amend --author, the \
                  reset as --reset-author (your identity and a new author date).",
        ghd_behaviour: "An amended commit keeps its author.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20591)],
        code: &["crates/corvene-ui/src/changes.rs", "crates/corvene-git/src/commit.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// Files Too Large › Track … in Git LFS.
    SUGGEST_LFS_TRACKING = 784 "suggest-lfs-tracking" {
        title: "Offer Git LFS for files that are too large",
        summary: "The Files Too Large warning before a commit has a \"Track *.ext in Git LFS\" \
                  button when Git LFS is installed: it tracks the files' extensions (the path \
                  of a file without one) with git lfs track, setting Git LFS up for the \
                  repository first if needed, and goes back to the commit form, where the files \
                  are committed as LFS pointers along with the .gitattributes change.",
        ghd_behaviour: "The warning links to an article about Git LFS; tracking the files is up \
                        to the command line.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(6055)],
        code: &["crates/corvene-ui/src/dialogs/oversized_files.rs", "crates/corvene-core/src/commit_checks.rs", "crates/corvene-git/src/remote_ops.rs"],
    },

    /// Commit nested git repositories as submodules.
    EMBEDDED_REPO_COMMIT = 785 "embedded-repo-commit" {
        title: "Commit nested repositories as submodules",
        summary: "When a commit includes an untracked folder that is a git repository of its \
                  own, Corvene asks first: one with an origin remote is added as a submodule \
                  (git submodule add with that URL, .gitmodules included), one without as a \
                  pointer to its current commit (with a note that can be hidden); then the \
                  commit goes on. Such a folder's menu also has \"Add as Submodule…\".",
        ghd_behaviour: "The folder is left out of the commit; when it is the only change, \
                        committing fails with nothing added.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(16819)],
        code: &["crates/corvene-core/src/commit_checks.rs", "crates/corvene-git/src/submodule.rs", "crates/corvene-ui/src/dialogs/add_embedded_repositories.rs", "crates/corvene-ui/src/changes.rs"],
    },

    /// The in-process status pairs deleted and untracked files as renames.
    WORKTREE_RENAME_DETECTION = 786 "worktree-rename-detection" {
        title: "Detect renames in the working tree",
        summary: "A tracked file deleted or moved on disk and a similar new file (at least 50 % \
                  alike) are listed as one renamed file, as git shows them once staged: its \
                  diff compares the old file with the new one and committing it records the \
                  rename. Needs 906-in-process-status (git status cannot pair them); a rename \
                  whose old file also has staged changes is listed as before. Pairing reads the \
                  untracked files, so it only runs while a tracked file is missing.",
        ghd_behaviour: "The old path is listed as deleted and the new one as a new file until \
                        they are committed.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(13588)],
        code: &["crates/corvene-git/src/status_gix.rs", "crates/corvene-git/src/diff.rs"],
    },

    /// Commit to a new branch / move the newest commits to one.
    COMMIT_TO_NEW_BRANCH = 787 "commit-to-new-branch" {
        title: "Commit to a new branch",
        summary: "The commit form's gear menu has \"Commit to New Branch…\": after a name, a \
                  branch is created at the current commit with the changes, they are committed \
                  there, the branch is published and its Create Pull Request page opens (GitHub \
                  repositories). Selecting the current branch's newest commits in History offers \
                  \"Create Branch from N Commits…\" (branch, publish, pull request), whose \
                  \"Remove the commits from <branch>\" box, offered only for commits that were \
                  never pushed, also moves the old branch back to before them.",
        ghd_behaviour: "Create the branch first (Branch › New Branch, bringing the changes), \
                        commit, publish, then Create Pull Request; commits made on the wrong \
                        branch are moved with the command line.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(14933)],
        code: &["crates/corvene-core/src/new_branch_flows.rs", "crates/corvene-ui/src/dialogs/branch_dialogs.rs", "crates/corvene-ui/src/changes.rs", "crates/corvene-ui/src/history.rs", "crates/corvene-git/src/branch_ops.rs"],
    },

    /// Partial commits place each hunk where it is in the index.
    PARTIAL_COMMIT_HUNK_POSITIONS = 788 "partial-commit-hunk-positions" {
        title: "Exact hunk positions in partial commits",
        summary: "When only some lines of a file are committed (or discarded), each hunk of the \
                  patch git applies says where it lands counting only the selected changes \
                  above it. Git starts looking for a hunk at that line, so a hunk whose \
                  surrounding lines also appear further down (blank lines, repeated blocks) \
                  is no longer applied there, committing or discarding the wrong lines.",
        ghd_behaviour: "The hunk position is taken from the full diff, which also counts the \
                        changes left out above it; with repeated surrounding lines the selected \
                        change can land at a later identical spot.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(12604)],
        code: &["crates/corvene-git/src/patch.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// Diffs of legacy-encoded files are decoded, and can be partly committed.
    NON_UTF8_DIFFS = 789 "non-utf8-diffs" {
        title: "Readable diffs of files that are not UTF-8",
        summary: "Text files kept in another encoding (windows-1252, ISO-8859-2, Shift_JIS…) \
                  are shown decoded in Changes, History and expanded diff lines, the encoding \
                  guessed from the file (windows-1252 when there is nothing to go on). \
                  Committing or discarding some of their lines writes the original bytes, so \
                  git accepts the patch. Files with a working-tree-encoding attribute are \
                  already shown right (git converts them).",
        ghd_behaviour: "Every non-UTF-8 character shows as a replacement character (\u{FFFD}), \
                        and committing or discarding selected lines of such a file fails \
                        because the patch no longer matches it.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(3923), Upstream::issue(5498)],
        code: &["crates/corvene-git/src/text_encoding.rs", "crates/corvene-git/src/diff.rs", "crates/corvene-git/src/patch.rs"],
    },

    /// A partial line selection follows its lines when the diff changes.
    SELECTION_FOLLOWS_LINES = 790 "selection-follows-lines" {
        title: "Line selection follows its lines",
        summary: "When a file with some lines ticked for the commit changes (edited above \
                  them, or some lines discarded), the ticks stay on the same lines: the \
                  changed lines of the old and new diff are matched by their text. Lines \
                  that are new take the file's default.",
        ghd_behaviour: "The selection is kept by row position in the diff, so after an edit or a \
                        discard above them the ticks land on other lines.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(4720), Upstream::issue(17614)],
        code: &["crates/corvene-core/src/line_selection.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// Intra-line highlights mark the changed words.
    WORD_INTRA_LINE_DIFF = 791 "word-intra-line-diff" {
        title: "Highlight changed words",
        summary: "In a modified line (a removed line paired with an added one), the darker \
                  highlight marks each changed word, found by a word diff of the two lines, \
                  so two small edits far apart in a line no longer highlight everything \
                  between them.",
        ghd_behaviour: "One highlight from the first to the last changed character of the line.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(2485), Upstream::issue(2700)],
        code: &["crates/corvene-ui/src/diff_view_rows.rs", "crates/corvene-ui/src/diff_view.rs"],
    },

    /// Unchanged lines keep their colours while whitespace is hidden.
    WHITESPACE_HIDDEN_HIGHLIGHT = 792 "whitespace-hidden-highlight" {
        title: "Right colours with whitespace hidden",
        summary: "While Hide Whitespace Changes is on, an unchanged line whose indentation \
                  changed is coloured from the file whose line git shows, so its syntax \
                  colours sit on the right characters.",
        ghd_behaviour: "The line is coloured from the other file's version, so the colours are \
                        shifted by the change in indentation.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(14920), Upstream::issue(21885)],
        code: &["crates/corvene-ui/src/diff_view.rs"],
    },

    /// Hide Whitespace Changes also hides files with whitespace changes only.
    HIDE_WHITESPACE_ONLY_FILES = 793 "hide-whitespace-only-files" {
        title: "Hide files with whitespace changes only",
        summary: "While Hide Whitespace Changes is on, files whose changes are all in \
                  whitespace (re-indented or with trailing spaces removed) are left out of the \
                  changes list and of a commit's file list in History, with a note \
                  \"N files hidden (whitespace only)\" under the list. In Changes they are \
                  still included in the commit.",
        ghd_behaviour: "Such files stay listed and show an empty diff.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(15980)],
        code: &["crates/corvene-git/src/diff.rs", "crates/corvene-core/src/dispatcher.rs", "crates/corvene-ui/src/changes.rs", "crates/corvene-ui/src/selected_commit.rs"],
    },

    /// SVG files can be shown as images.
    SVG_IMAGE_DIFF = 794 "svg-image-diff" {
        title: "SVG files as images",
        summary: "The diff header of an .svg file has a Text / Image switch: Image shows the \
                  old and new drawing with the image diff modes (2-up, Swipe, Onion Skin, \
                  Difference), drawn sharp at the size it shows at, with its text in the \
                  system fonts; Text the usual line diff. The choice is kept for every SVG \
                  file (Image at first); line selection keeps working on the text.",
        ghd_behaviour: "SVG files only show their text diff.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(11316)],
        code: &["crates/corvene-ui/src/image_diff.rs", "crates/corvene-ui/src/diff_view.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// Images stored in Git LFS show as images.
    LFS_IMAGE_PREVIEWS = 795 "lfs-image-previews" {
        title: "Image previews for files in Git LFS",
        summary: "An image stored in Git LFS shows as an image diff in Changes and History \
                  when its contents are downloaded (git-lfs keeps them in the repository's \
                  lfs folder), read without running git-lfs. When they are not downloaded, \
                  the pointer diff stays, with a note saying so.",
        ghd_behaviour: "The diff shows the LFS pointer files' text (version, oid and size \
                        lines) instead of the image.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(2981)],
        code: &["crates/corvene-git/src/lfs.rs", "crates/corvene-core/src/dispatcher.rs", "crates/corvene-ui/src/diff_view.rs"],
    },

    /// Text files stored in Git LFS diff their contents.
    LFS_TEXT_DIFF = 796 "lfs-text-diff" {
        title: "Diff the contents of text files in Git LFS",
        summary: "A text file stored in Git LFS shows the diff of its contents in Changes and \
                  History when they are downloaded, instead of the pointer files' text. Its \
                  lines cannot be selected one by one (the commit holds the pointer), only \
                  the whole file. Files over 16 MB, binary contents and contents that are \
                  not downloaded keep the pointer diff.",
        ghd_behaviour: "The diff shows the LFS pointer files (version, oid and size lines).",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(15136)],
        code: &["crates/corvene-git/src/lfs.rs", "crates/corvene-core/src/dispatcher.rs", "crates/corvene-ui/src/diff_view.rs"],
    },

    /// Every stash is listed and can be applied, restored, branched or discarded.
    STASH_LIST = 797 "stash-list" {
        title: "List every stash",
        summary: "The bottom of the changes list shows every stash of the repository (Stashes \
                  with a count, collapsible), including those made with git stash on the \
                  command line, each with its message, branch and age; Desktop's own stashes \
                  are marked. Clicking one shows it in the stash viewer. Its menu and the \
                  viewer offer Restore (apply and drop), Apply (keep the stash), Create Branch \
                  from Stash and Discard, which shows a Discarded stash banner with Undo for \
                  15 seconds. Stash All Changes with Message asks for a message and whether \
                  to include new files; such a stash is not the branch's own, so switching \
                  branches leaves it alone.",
        ghd_behaviour: "Only the current branch's Desktop stash is shown, as the Stashed Changes \
                        row, and it can only be restored or discarded.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(12699), Upstream::issue(21432)],
        code: &["crates/corvene-core/src/stash_list.rs", "crates/corvene-git/src/stash_ops.rs", "crates/corvene-ui/src/stash_list.rs", "crates/corvene-ui/src/stash_view.rs", "crates/corvene-ui/src/dialogs/stash_list_dialogs.rs"],
    },
    /// A Blame view: who last changed each line of a file.
    BLAME = 798 "blame" {
        title: "Blame",
        summary: "Blame in the context menu of a changed file and of a file in a commit, and \
                  a Blame button in the diff header, show the file with the commit that last \
                  changed each line: short SHA, author and age beside each run of lines from \
                  one commit, Not committed yet for local edits. The gutter fills in while git \
                  works. Hovering a commit shows its summary, clicking it selects the commit \
                  in History, and its menu can blame the revision before it to step back \
                  through a line's history. Options ignore whitespace, follow moved or copied \
                  lines and skip the revisions in .git-blame-ignore-revs.",
        ghd_behaviour: "There is no blame; it can only be seen on GitHub or with git.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(2310)],
        code: &["crates/corvene-git/src/blame.rs", "crates/corvene-core/src/blame.rs", "crates/corvene-ui/src/blame_view.rs"],
    },

    /// Fixup commits for unpushed commits, then autosquash.
    FIXUP_COMMITS = 799 "fixup-commits" {
        title: "Fixup commits",
        summary: "The commit options menu offers Fixup Into, listing the branch's unpushed \
                  commits: the included changes are committed with `git commit --fixup` for \
                  the commit picked, and a banner offers to squash them in right away. Squash \
                  Fixup Commits in the same menu folds every `fixup!` commit on the branch into \
                  the commit it fixes (`rebase --autosquash`), with the usual progress, \
                  conflict and Undo handling of a squash.",
        ghd_behaviour: "No fixup commits; a change to an older commit needs a new commit and a \
                        squash by drag and drop.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(12354)],
        code: &["crates/corvene-git/src/rebase_ops.rs", "crates/corvene-core/src/mco.rs", "crates/corvene-ui/src/changes.rs"],
    },

    // ---- 800 History & branches ----

    /// History review mode: the diff alone, full width.
    HISTORY_REVIEW_MODE = 801 "history-review-mode" {
        title: "History review mode",
        summary: "View › Toggle History Review Mode (⌃⌘S) hides the repository sidebar and the \
                  commit's file list in History so the diff gets the whole width; ⌥↓ / ⌥↑ \
                  (flag 614) still step through the files.",
        ghd_behaviour: "The sidebar and file list always take their width.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(8456)],
        code: &[
            "crates/corvene-ui/src/workspace.rs",
            "crates/corvene-ui/src/selected_commit.rs",
            "crates/corvene-ui/src/app_menu.rs",
        ],
    },

    /// Summary-only History rows.
    COMPACT_COMMIT_ROWS = 802 "compact-commit-rows" {
        title: "Compact History rows",
        summary: "History rows are 30 px tall and show the commit summary only, without the avatar, \
                  author and time line (the commit's details pane still has them).",
        ghd_behaviour: "50 px rows with an avatar + \"author • time\" line under the summary.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(15956)],
        code: &["crates/corvene-ui/src/history.rs"],
    },

    /// A mark on history rows whose commit has a description.
    COMMIT_BODY_INDICATOR = 803 "commit-body-indicator" {
        title: "Mark commits that have a description",
        summary: "A History row whose commit message has a description (extended body) shows a \
                  ⋯ mark after the summary.",
        ghd_behaviour: "Only the summary; the description is seen by selecting the commit.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20401)],
        code: &["crates/corvene-ui/src/history.rs"],
    },

    /// Inline code and autolinks in commit messages.
    COMMIT_MESSAGE_RICH_TEXT = 804 "commit-message-rich-text" {
        title: "Inline code and links in commit messages",
        summary: "The selected commit's title and description show `backtick` spans as inline code, \
                  link URLs inside punctuation and, in a GitHub repository, commit SHAs.",
        ghd_behaviour: "Backticks show literally; SHAs are plain text; a URL is linked only as a \
                        whole word.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(18104), Upstream::issue(7723)],
        code: &["crates/corvene-ui/src/selected_commit.rs", "crates/corvene-core/src/markdown.rs"],
    },

    /// The commit details show the date and link the SHA.
    COMMIT_DETAILS_EXTRAS = 805 "commit-details-extras" {
        title: "Commit date and SHA link in the commit details",
        summary: "The selected commit's details show the author date and time (the relative time on \
                  hover), and in a GitHub repository the SHA opens the commit on GitHub.",
        ghd_behaviour: "Author, SHA and line counts only; the SHA is plain text.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20715), Upstream::issue(3785)],
        code: &["crates/corvene-ui/src/selected_commit.rs"],
    },

    /// Tag tooltips in History.
    TAGS_TOOLTIP = 806 "tags-tooltip" {
        title: "Tag tooltips in History",
        summary: "Hovering a commit's tag pill in the History list, or the tag list in the commit's \
                  details, shows every tag, one per line.",
        ghd_behaviour: "The pill shows the first tag and a sliver for the rest; a truncated tag list \
                        cannot be read.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(9687)],
        code: &["crates/corvene-ui/src/history.rs", "crates/corvene-ui/src/selected_commit.rs"],
    },

    /// History's first-parent toggle.
    HISTORY_FIRST_PARENT = 807 "history-first-parent" {
        title: "First-parent History",
        summary: "A filter button before \"Select Branch to Compare…\" switches the History list to \
                  first parents only (git log --first-parent), hiding the commits that came in \
                  through merges. The choice is remembered.",
        ghd_behaviour: "History always lists every commit reachable from HEAD.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21414)],
        code: &["crates/corvene-ui/src/history.rs", "crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/log.rs"],
    },

    /// The history list scrolls to the top when the branch changes.
    HISTORY_SCROLLS_TO_TOP_ON_BRANCH_CHANGE = 808 "history-scrolls-to-top-on-branch-change" {
        title: "History scrolls to the top on branch change",
        summary: "Switching branch (or repository) scrolls the History list back to the newest \
                  commit.",
        ghd_behaviour: "The list keeps its scroll offset, so another branch's history opens \
                        somewhere in the middle.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20849), Upstream::issue(6706)],
        code: &["crates/corvene-ui/src/history.rs"],
    },

    /// History context menus: Copy Commit Title / Message / URL, Copy SHAs.
    HISTORY_COPY_ITEMS = 809 "history-copy-items" {
        title: "History: copy commit title, message, URL and SHAs",
        summary: "A commit's context menu adds Copy Commit Title, Copy Commit Message and (for \
                  GitHub repositories) Copy Commit URL next to Copy SHA; a multi-commit selection's \
                  menu adds Copy SHAs (newest first, one per line).",
        ghd_behaviour: "Copy SHA and Copy Tag only; nothing to copy for a multi-commit selection.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(12547), Upstream::issue(20853), Upstream::issue(7518), Upstream::issue(9791), Upstream::issue(21061)],
        code: &["crates/corvene-ui/src/history.rs"],
    },

    /// Multi-select in a commit's file list.
    COMMIT_FILES_MULTI_SELECT = 810 "commit-files-multi-select" {
        title: "Multi-select a commit's files",
        summary: "The History file list selects several files with ⌘-click and ⇧-click; right-clicking \
                  the selection offers Copy File Paths and Copy Relative File Paths (one per line) \
                  and opens the ones still on disk in the editor or with their default programs \
                  (up to 25). The diff shows the last clicked file.",
        ghd_behaviour: "One file at a time.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(15525), Upstream::issue(20467), Upstream::issue(15989)],
        code: &["crates/corvene-ui/src/selected_commit.rs"],
    },

    /// History › Open with Default Program opens the file as of the commit.
    OPEN_HISTORICAL_FILE = 811 "open-historical-file" {
        title: "History opens the commit's version of a file",
        summary: "Open with Default Program in a commit's file list opens the file as it is in \
                  that commit (a read-only copy in the temporary directory), not the working copy.",
        ghd_behaviour: "Opens the file in the working directory, whatever its current state.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(21117)],
        code: &["crates/corvene-ui/src/selected_commit.rs", "crates/corvene-core/src/integrations.rs"],
    },

    /// Copy path items for a commit's file that is gone from disk.
    COPY_PATH_OF_MISSING_FILE = 812 "copy-path-of-missing-file" {
        title: "Copy the path of a file missing on disk",
        summary: "In a commit's file list, a file that no longer exists on disk still offers Copy \
                  File Path and Copy Relative File Path under \"File Does Not Exist on Disk\".",
        ghd_behaviour: "Only the disabled \"File Does Not Exist on Disk\" item.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(18349)],
        code: &["crates/corvene-ui/src/selected_commit.rs"],
    },

    /// Line totals for a multi-commit selection.
    MULTI_COMMIT_LINE_TOTALS = 813 "multi-commit-line-totals" {
        title: "Line totals for a multi-commit selection",
        summary: "Selecting several commits shows the range's added and removed line totals next \
                  to \"Showing changes from N commits\".",
        ghd_behaviour: "Only the commit count.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(17869)],
        code: &["crates/corvene-ui/src/selected_commit.rs"],
    },

    /// Revert one file of a commit.
    REVERT_FILE_IN_COMMIT = 814 "revert-file-in-commit" {
        title: "Revert one file of a commit",
        summary: "A commit's file menu adds Revert Changes to This File: that file's changes from the \
                  commit are undone in the working directory (nothing is committed). A working file \
                  that has changed since in the same places is left alone and the error says so.",
        ghd_behaviour: "Only whole commits can be reverted.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(19207)],
        code: &["crates/corvene-ui/src/selected_commit.rs", "crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/history_ops.rs"],
    },

    /// Revert without committing, for one commit or a multi-selection.
    REVERT_WITHOUT_COMMITTING = 815 "revert-without-committing" {
        title: "Revert without committing",
        summary: "A commit's context menu adds Revert Changes in Commit Without Committing, and a \
                  multi-commit selection's menu Revert Changes in N Commits Without Committing: \
                  git revert --no-commit, newest first, leaves the combined inverse staged in \
                  Changes. Needs a clean working directory; a conflict rolls everything back.",
        ghd_behaviour: "Reverts one commit at a time, each as its own commit.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(17278), Upstream::issue(9967)],
        code: &["crates/corvene-ui/src/history.rs", "crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/history_ops.rs"],
    },

    /// History "Push Up to This Commit".
    PUSH_UP_TO_COMMIT = 816 "push-up-to-commit" {
        title: "Push up to a commit",
        summary: "A commit's context menu adds Push Up to This Commit, enabled on the current \
                  branch's unpushed commits: it pushes that commit (and the ones before it) to the \
                  upstream branch and keeps the newer ones local. Unpushed tags stay behind.",
        ghd_behaviour: "Push always pushes the whole branch.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(19238), Upstream::issue(20670)],
        code: &["crates/corvene-ui/src/history.rs", "crates/corvene-core/src/remote.rs"],
    },

    /// Checkout Commit on the branch tip.
    CHECKOUT_HEAD_COMMIT = 817 "checkout-head-commit" {
        title: "Checkout Commit on the latest commit",
        summary: "A commit's Checkout Commit is also enabled on the current branch's latest commit, \
                  detaching HEAD there.",
        ghd_behaviour: "Disabled on the latest commit, so HEAD cannot be detached at the branch tip.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22495)],
        code: &["crates/corvene-ui/src/history.rs"],
    },

    /// Undo Commit warns only when local changes touch the commit's files.
    UNDO_WARNS_ONLY_ON_OVERLAP = 818 "undo-warns-only-on-overlap" {
        title: "Undo Commit warns only about overlapping changes",
        summary: "The \"changes in progress\" warning before Undo Commit appears only when a file \
                  with local changes is one the commit touched.",
        ghd_behaviour: "Warns whenever there are any local changes, although undoing never loses \
                        changes to files the commit did not touch.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(18388)],
        code: &["crates/corvene-core/src/dispatcher.rs"],
    },

    /// Undo Commit and Amend Commit warn about the commit's tags.
    WARN_UNDO_TAGGED_COMMIT = 819 "warn-undo-tagged-commit" {
        title: "Warn before undoing or amending a tagged commit",
        summary: "Undo Commit and Amend Commit on a commit that has tags ask first: the tags \
                  would stay on a commit that is no longer on any branch.",
        ghd_behaviour: "Undoes and amends silently; the tags keep pointing at the orphaned \
                        commit.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(19844), Upstream::issue(17737)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-ui/src/dialogs/history_dialogs.rs", "crates/corvene-ui/src/changes.rs", "crates/corvene-ui/src/history.rs"],
    },

    /// History › Cherry-pick Without Committing.
    CHERRY_PICK_WITHOUT_COMMITTING = 820 "cherry-pick-without-committing" {
        title: "Cherry-pick without committing",
        summary: "A commit's menu (and a multi-commit selection's) adds Cherry-pick Commit Without \
                  Committing: the changes are applied to the current branch and left staged in \
                  Changes. Needs a clean working directory; a conflict undoes everything.",
        ghd_behaviour: "Cherry-picking always commits, onto a branch picked in a dialog.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21383)],
        code: &["crates/corvene-ui/src/history.rs", "crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/history_ops.rs"],
    },

    /// History › Create Patch File….
    CREATE_PATCH_FILES = 821 "create-patch-files" {
        title: "Create patch files from commits",
        summary: "A commit's menu (and a multi-commit selection's) adds Create Patch File…: after \
                  a folder is picked, git format-patch writes one numbered .patch file per commit \
                  there and the first is shown in Finder.",
        ghd_behaviour: "No way to export commits as patches.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20935)],
        code: &["crates/corvene-ui/src/history.rs", "crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/history_ops.rs"],
    },

    /// Cherry-pick / squash / reorder without a branch explain themselves.
    NO_BRANCH_EXPLAINED = 822 "no-branch-explained" {
        title: "Explain why cherry-pick, squash and reorder cannot start",
        summary: "Cherry-pick, squash and reorder on a detached HEAD or during a rebase show an \
                  error saying so.",
        ghd_behaviour: "Nothing happens.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(18715), Upstream::issue(20982)],
        code: &["crates/corvene-core/src/mco.rs"],
    },

    /// Create a Tag's Message field.
    TAG_MESSAGE = 823 "tag-message" {
        title: "Tag message",
        summary: "Create a Tag has an optional Message field; the annotated tag carries it as \
                  typed.",
        ghd_behaviour: "Annotated tags always get an empty message.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(12995), Upstream::issue(22890)],
        code: &["crates/corvene-ui/src/dialogs/history_dialogs.rs", "crates/corvene-git/src/history_ops.rs"],
    },

    /// ⌘⏎ submits Create a Tag.
    CMD_ENTER_SUBMITS_CREATE_TAG = 824 "cmd-enter-submits-create-tag" {
        title: "⌘⏎ creates the tag",
        summary: "In Create a Tag, ⌘⏎ creates the tag from the Message field as well as from Name \
                  (where ⏎ does too).",
        ghd_behaviour: "Only ⏎ in the Name field submits; ⌘⏎ does nothing.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(22740)],
        code: &["crates/corvene-ui/src/dialogs/history_dialogs.rs"],
    },

    /// Tags in the compare list.
    COMPARE_TAGS = 825 "compare-tags" {
        title: "Compare to a tag",
        summary: "Typing in \"Select Branch to Compare…\" also lists the matching tags, under Tags \
                  after the branches; picking one compares the current branch with it as with a \
                  branch.",
        ghd_behaviour: "Branches only.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(15702)],
        code: &["crates/corvene-ui/src/history.rs", "crates/corvene-core/src/compare.rs", "crates/corvene-git/src/log.rs"],
    },

    /// Delete pushed tags, optionally from the remote.
    DELETE_PUSHED_TAGS = 826 "delete-pushed-tags" {
        title: "Delete pushed tags",
        summary: "A commit's Delete tag items are enabled for every tag, not only ones created here \
                  and not pushed yet. Those others ask first, with an unticked option to delete the \
                  tag from the remote too (git push <remote> --delete).",
        ghd_behaviour: "Only tags created in the app and not yet pushed can be deleted.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(15858)],
        code: &["crates/corvene-ui/src/history.rs", "crates/corvene-ui/src/dialogs/history_dialogs.rs", "crates/corvene-core/src/remote.rs", "crates/corvene-git/src/remote_ops.rs"],
    },

    /// Squash message: keep only the target's.
    SQUASH_KEEP_TARGET_MESSAGE = 827 "squash-keep-target-message" {
        title: "Squash: use only the target commit's message",
        summary: "The squash message dialog has a \"Use only the target commit's message\" link \
                  that replaces the combined description with the summary and description of the \
                  commit squashed onto.",
        ghd_behaviour: "The combined message (target description plus every squashed commit's \
                        message) has to be trimmed by hand.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20507)],
        code: &["crates/corvene-ui/src/dialogs/mco_dialogs.rs", "crates/corvene-ui/src/dialogs/mod.rs"],
    },

    /// A failed squash keeps its message.
    SQUASH_KEEPS_DRAFT = 828 "squash-keeps-draft" {
        title: "Squash remembers its message after an error",
        summary: "When a squash fails, the message typed for it is kept: squashing the same \
                  commits again opens the dialog with that message instead of the combined one.",
        ghd_behaviour: "The message is lost; the next try starts from the combined messages again.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(20764)],
        code: &["crates/corvene-core/src/mco.rs", "crates/corvene-core/src/state.rs"],
    },

    /// Squash with local changes: git stashes them around it.
    SQUASH_AUTOSTASH = 829 "squash-autostash" {
        title: "Squash with uncommitted changes",
        summary: "Squashing with uncommitted changes runs the rebase with --autostash: git \
                  stashes the changes first and puts them back afterwards. If they no longer \
                  apply, they stay in git's stash and an error says so.",
        ghd_behaviour: "Asks to stash the changes first (Stash Changes and Continue) and leaves \
                        them stashed.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(12759)],
        code: &["crates/corvene-core/src/mco.rs", "crates/corvene-git/src/rebase_ops.rs"],
    },

    /// Squash / reorder keep the History selection.
    SELECT_REWRITTEN_COMMITS = 830 "select-rewritten-commits" {
        title: "History keeps the selection through squash and reorder",
        summary: "After a squash or reorder that rewrote the selected commits, History selects \
                  the squashed commit (or the moved commits) under their new SHAs.",
        ghd_behaviour: "The selection jumps to the newest commit.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(12549)],
        code: &["crates/corvene-core/src/mco.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// The Rebase dialog starts on the default branch.
    REBASE_PRESELECTS_DEFAULT_BRANCH = 831 "rebase-preselects-default-branch" {
        title: "Rebase dialog preselects the default branch",
        summary: "Branch › Rebase Current Branch… opens with the default branch selected and its \
                  preview shown, unless the default branch is the current one.",
        ghd_behaviour: "The current branch shows as selected, and a branch has to be picked first.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(17731)],
        code: &["crates/corvene-ui/src/dialogs/mco_dialogs.rs"],
    },

    /// Rebase onto `origin/main`.
    REBASE_ONTO_REMOTE_BRANCH = 832 "rebase-onto-remote-branch" {
        title: "Rebase onto a remote branch",
        summary: "The Rebase dialog's list ends with Remote Branches: the remote-tracking \
                  branches of local branches (origin/main next to main), so a branch can be \
                  rebased onto what was last fetched without checking out and pulling the local \
                  branch first.",
        ghd_behaviour: "A remote branch that has a local branch is not listed; only the local one \
                        can be picked.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(13994)],
        code: &["crates/corvene-ui/src/dialogs/mco_dialogs.rs", "crates/corvene-ui/src/branch_list.rs"],
    },

    /// Rebase with local changes: stash, then rebase.
    REBASE_STASH_AND_CONTINUE = 833 "rebase-stash-and-continue" {
        title: "Rebase: Stash Changes and Continue rebases",
        summary: "Rebasing with uncommitted changes first offers to stash them; Stash Changes and \
                  Continue stashes and then runs the rebase.",
        ghd_behaviour: "Stash Changes and Continue stashes the changes and stops; the rebase has to \
                        be started again.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(21904)],
        code: &["crates/corvene-core/src/mco.rs", "crates/corvene-core/src/state.rs"],
    },

    /// Rebase / squash / reorder keep `#` message lines.
    REBASE_KEEPS_HASH_MESSAGES = 834 "rebase-keeps-hash-messages" {
        title: "Rebase keeps commit messages that start with #",
        summary: "Rebase, squash and reorder keep commit message lines starting with # (a \
                  \"#123 Fix\" summary, say) when a commit stopped on conflicts is continued, and \
                  git's conflict notes stay out of the message.",
        ghd_behaviour: "Continuing after a conflict drops every line starting with #; a summary \
                        starting with # leaves the message empty and the rebase fails.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(16444)],
        code: &["crates/corvene-core/src/mco.rs", "crates/corvene-git/src/rebase_ops.rs"],
    },

    /// A rebase found in progress names its base branch.
    REBASE_BASE_NAME_RESOLVED = 835 "rebase-base-name-resolved" {
        title: "Rebase found in progress names its base branch",
        summary: "For a rebase that stopped on conflicts outside Corvene (or before a restart), the \
                  branch at the commit being rebased onto is looked up, so the conflicts dialog's \
                  choices read \"from main\" and the success banner names the base.",
        ghd_behaviour: "The base side is unnamed (\"Use the modified file\").",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(8113)],
        code: &["crates/corvene-core/src/mco.rs", "crates/corvene-git/src/rebase_ops.rs"],
    },

    /// Cherry-pick keeps the picked message after a conflict.
    CHERRY_PICK_KEEPS_MESSAGES = 836 "cherry-pick-keeps-messages" {
        title: "Cherry-pick keeps the commit message after a conflict",
        summary: "A cherry-picked commit that stopped on conflicts keeps its message as written \
                  when continued: lines starting with # stay, and git's \"Conflicts:\" note is \
                  not added.",
        ghd_behaviour: "Continuing can fail with \"Aborting commit due to empty commit message\"; \
                        lines starting with # are dropped.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(21685)],
        code: &["crates/corvene-core/src/mco.rs", "crates/corvene-git/src/rebase_ops.rs"],
    },

    /// Squash and merge asks for the commit message.
    SQUASH_MERGE_MESSAGE = 837 "squash-merge-message" {
        title: "Squash and merge: commit message",
        summary: "The Squash and Merge dialog has summary and description fields above its \
                  button. With a summary, the squashed commit gets that message; left empty, \
                  git's \"Squashed commit of the following\" list as before.",
        ghd_behaviour: "The squashed commit always gets git's \"Squashed commit of the following\" \
                        list of the merged commits.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21718)],
        code: &["crates/corvene-ui/src/dialogs/branch_dialogs.rs", "crates/corvene-core/src/mco.rs", "crates/corvene-git/src/branch_ops.rs"],
    },

    /// No new merge or rebase while the repository is conflicted.
    NO_MERGE_WHILE_CONFLICTED = 838 "no-merge-while-conflicted" {
        title: "No merge while conflicted",
        summary: "While a merge, rebase or cherry-pick still has conflicts, the History tab's merge \
                  button is disabled and Branch › Merge, Squash and Merge, Rebase and Update from \
                  Default Branch explain that it must be finished or aborted first.",
        ghd_behaviour: "Starts the new operation, which git refuses with an error.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(6429), Upstream::issue(6584)],
        code: &["crates/corvene-core/src/mco.rs", "crates/corvene-core/src/compare.rs", "crates/corvene/src/main.rs", "crates/corvene-ui/src/history.rs"],
    },

    /// Conflicts dialog › Resolve All ▾.
    RESOLVE_ALL_CONFLICTS = 839 "resolve-all-conflicts" {
        title: "Resolve all conflicts using one side",
        summary: "With two or more conflicted files, the conflicts dialog has a Resolve All menu \
                  that picks one branch's version for every file still in conflict. Like the \
                  per-file choice it is applied on Continue, and each file keeps its Undo.",
        ghd_behaviour: "One side is picked file by file.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(12377), Upstream::issue(15829), Upstream::issue(22516)],
        code: &["crates/corvene-ui/src/dialogs/mco_dialogs.rs", "crates/corvene-core/src/mco.rs"],
    },

    /// Conflicted file menu › Copy File Path.
    CONFLICT_MENU_COPY_PATHS = 840 "conflict-menu-copy-paths" {
        title: "Conflicts dialog: copy file paths",
        summary: "A conflicted file's ▾ menu in the conflicts dialog adds Copy File Path and Copy \
                  Relative File Path, as in the changes list.",
        ghd_behaviour: "Open with Default Program, Reveal in Finder and the resolution choices only.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22399)],
        code: &["crates/corvene-ui/src/dialogs/mco_dialogs.rs"],
    },

    /// Conflicts dialog names the stopped commit.
    CONFLICTS_SHOW_CURRENT_COMMIT = 841 "conflicts-show-current-commit" {
        title: "Conflicts dialog shows the stopped commit",
        summary: "While a rebase, squash or reorder waits on conflicts, the conflicts \
                  dialog starts with \"Commit N of M:\" and that commit's summary, as the \
                  progress dialog showed it.",
        ghd_behaviour: "The conflicts dialog does not say which commit is being applied.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(18796)],
        code: &["crates/corvene-ui/src/dialogs/mco_dialogs.rs"],
    },

    /// Conflicted file › Open in Merge Tool.
    OPEN_IN_MERGE_TOOL = 842 "open-in-merge-tool" {
        title: "Open conflicts in your merge tool",
        summary: "A conflicted file's ▾ menu in the conflicts dialog starts with Open in Merge \
                  Tool: git mergetool runs the tool set in git's merge.tool (Beyond Compare, \
                  kdiff3, …) on the file, and the list refreshes when it closes.",
        ghd_behaviour: "Only the editor, the default program or Finder.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(9609)],
        code: &["crates/corvene-ui/src/dialogs/mco_dialogs.rs", "crates/corvene-core/src/mco.rs", "crates/corvene-git/src/rebase_ops.rs"],
    },

    /// Create a Branch can start from any branch.
    CREATE_BRANCH_FROM_ANY_BRANCH = 843 "create-branch-from-any-branch" {
        title: "Create a branch from any branch",
        summary: "Create a Branch offers \"Other branch…\" next to the default and current \
                  branches, with a filterable list of every local and remote branch to start from.",
        ghd_behaviour: "Only the default branch or the current branch (and no choice at all while \
                        the default branch is checked out).",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(12459), Upstream::issue(20083)],
        code: &["crates/corvene-ui/src/dialogs/branch_dialogs.rs"],
    },

    /// Create a Branch starts from the current branch while there are changes.
    CREATE_BRANCH_WITH_CHANGES_FROM_CURRENT = 844 "create-branch-with-changes-from-current" {
        title: "New branch with uncommitted changes starts from the current branch",
        summary: "While the working directory has uncommitted changes, Create a Branch preselects \
                  the current branch as the starting point, so the changes brought along apply \
                  to the code they were written against.",
        ghd_behaviour: "Always preselects the default branch; bringing the changes onto it can \
                        conflict or appear to lose work.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(9670)],
        code: &["crates/corvene-ui/src/dialogs/branch_dialogs.rs"],
    },

    /// Create a Branch prefills a prefix.
    BRANCH_NAME_PREFIX = 845 "branch-name-prefix" {
        title: "Branch name prefix",
        summary: "Text Create a Branch puts in front of the suggested name (for example \
                  \"feature/\" or \"yourname/\"); empty for none.",
        ghd_behaviour: "No prefix.",
        nature: Nature::Feature,
        kind: Kind::Text { placeholder: "feature/", validate: branch_name_prefix },
        corvene: Value::text(""), ghd: Value::text(""),
        familiar: Value::text(""), max: Value::text(""),
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(14004)],
        code: &["crates/corvene-ui/src/dialogs/branch_dialogs.rs"],
    },

    /// Create / Rename Branch refuse `head` in any case.
    REJECT_HEAD_BRANCH_NAME = 846 "reject-head-branch-name" {
        title: "Branches can't be named \"head\"",
        summary: "Create a Branch and Rename Branch refuse \"head\" in any letter case, which on \
                  a case-insensitive file system names HEAD itself.",
        ghd_behaviour: "Creates the branch; HEAD ends up detached and the branch can't be published.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(13638)],
        code: &["crates/corvene-ui/src/dialogs/branch_dialogs.rs"],
    },

    /// Branch › New Branch… prefills the branch filter's text.
    NEW_BRANCH_FROM_FILTER = 847 "new-branch-from-filter" {
        title: "New Branch shortcut uses the branch filter",
        summary: "Branch › New Branch… (⌘⇧N) while the branch list is open prefills the name with \
                  its filter text, like the list's New Branch button.",
        ghd_behaviour: "The shortcut always opens Create a Branch with an empty name.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(5199)],
        code: &["crates/corvene/src/main.rs", "crates/corvene-ui/src/workspace.rs", "crates/corvene-ui/src/branch_list.rs"],
    },

    /// Other Branches sorted by last update.
    BRANCH_LIST_SORT_BY_DATE = 848 "branch-list-sort-by-date" {
        title: "Branch lists sort other branches by date",
        summary: "Other Branches in the branch list and the branch pickers (merge, rebase, \
                  compare, pull request base, new branch) are ordered by the tip commit's date, \
                  most recently updated first.",
        ghd_behaviour: "Sorted by name only.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(19903), Upstream::issue(21358), Upstream::issue(5155)],
        code: &["crates/corvene-ui/src/branch_list.rs"],
    },

    /// The branch filter ignores an `owner:` prefix.
    BRANCH_FILTER_STRIPS_OWNER = 849 "branch-filter-strips-owner" {
        title: "Branch filter understands owner:branch",
        summary: "Pasting GitHub's owner:branch form of a branch name into the branch list's filter \
                  finds the branch (the owner: part is ignored).",
        ghd_behaviour: "Filters for the whole text, so the branch is not found.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(7424)],
        code: &["crates/corvene-ui/src/branch_list.rs"],
    },

    /// Branch rows show where the branch lives.
    BRANCH_LIST_LOCAL_REMOTE_ICONS = 850 "branch-list-local-remote-icons" {
        title: "Branch list icons for local-only and remote branches",
        summary: "In the branch list a branch with no upstream (only on this computer) shows a \
                  desktop icon and a branch that exists only on the remote shows a server icon; \
                  tracked local branches keep the branch icon.",
        ghd_behaviour: "Every branch shows the same branch icon.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(17012), Upstream::issue(22019)],
        code: &["crates/corvene-ui/src/branch_list.rs"],
    },

    /// Branch list toggle: remote branches only.
    BRANCH_LIST_REMOTE_ONLY = 851 "branch-list-remote-only" {
        title: "Branch list can show only remote branches",
        summary: "A server button beside the branch list's filter narrows the list to the remote \
                  branches (including those checked out locally), in one Remote Branches group.",
        ghd_behaviour: "Remote branches are only listed, under Other Branches, when there is no \
                        local branch of the same name.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(14134)],
        code: &["crates/corvene-ui/src/branch_list.rs"],
    },

    /// Mark local branches whose upstream was deleted on the remote.
    BRANCH_UPSTREAM_GONE = 852 "branch-upstream-gone" {
        title: "Mark branches deleted on the remote",
        summary: "Local branches whose upstream branch was deleted on the remote (and pruned by \
                  a fetch) show a cloud icon after their name in the branch list, so merged \
                  branches are easy to spot and clean up.",
        ghd_behaviour: "Nothing tells such branches apart.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20897)],
        code: &["crates/corvene-ui/src/branch_list.rs", "crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/branch_ops.rs"],
    },

    /// Ahead/behind counts and "not published" in branch list rows.
    BRANCH_LIST_AHEAD_BEHIND = 853 "branch-list-ahead-behind" {
        title: "Push / pull state in the branch list",
        summary: "Local branch rows show how many commits they have to push and pull (\"2↑ 1↓\") \
                  against their upstream, or an upload icon when the branch was never published.",
        ghd_behaviour: "Only the current branch's state shows, on the toolbar's push / pull button.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(5330)],
        code: &["crates/corvene-ui/src/branch_list.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// A stash icon on branch rows that have a stash.
    BRANCH_LIST_STASH_ICON = 854 "branch-list-stash-icon" {
        title: "Stash icon in the branch list",
        summary: "Local branches with stashed changes (a GitHub Desktop or Corvene stash) show the \
                  stash icon after their name in the branch list.",
        ghd_behaviour: "A branch's stash is only visible after switching to it.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(17198)],
        code: &["crates/corvene-ui/src/branch_list.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// How many branches the branch list's Recent group shows.
    RECENT_BRANCHES_COUNT = 855 "recent-branches-count" {
        title: "Recent branches shown",
        summary: "How many recently checked-out branches the branch list shows in its Recent \
                  group (0 hides the group).",
        ghd_behaviour: "Always 5.",
        nature: Nature::Feature,
        kind: Kind::Number { min: 0, max: 50, unit: None },
        corvene: Value::Number(5), ghd: Value::Number(5),
        familiar: Value::Number(5), max: Value::Number(10),
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(14311)],
        code: &["crates/corvene-core/src/dispatcher.rs"],
    },

    /// Branch context menu: Rebase Current Branch onto <branch>….
    BRANCH_MENU_REBASE_ONTO = 856 "branch-menu-rebase-onto" {
        title: "Rebase onto a branch from the branch list",
        summary: "A branch's context menu in the branch list offers \"Rebase Current Branch onto \
                  <branch>…\", which opens the rebase dialog with that branch selected.",
        ghd_behaviour: "Rebasing starts from Branch › Rebase Current Branch… only.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21657)],
        code: &["crates/corvene-ui/src/branch_list.rs", "crates/corvene-core/src/mco.rs", "crates/corvene-ui/src/dialogs/mco_dialogs.rs"],
    },

    /// Branch context menu: fast-forward a branch that is not checked out.
    UPDATE_BRANCH_FROM_UPSTREAM = 857 "update-branch-from-upstream" {
        title: "Update a branch from its upstream",
        summary: "The branch list's context menu offers \"Update from origin/…\" on local branches \
                  that are not checked out: the branch is fast-forwarded to its upstream without \
                  switching to it (a diverged branch is left alone with an explanation).",
        ghd_behaviour: "No such command; the branch has to be checked out and pulled.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(19837)],
        code: &["crates/corvene-ui/src/branch_list.rs", "crates/corvene-core/src/remote.rs", "crates/corvene-git/src/remote_ops.rs"],
    },

    /// Update from Default Branch fetches and merges the remote-tracking branch.
    UPDATE_FROM_DEFAULT_FETCHES = 858 "update-from-default-fetches" {
        title: "Update from the default branch's remote",
        summary: "Branch › Update from Default Branch fetches the default branch's remote first \
                  and merges its remote-tracking branch (origin/main), so the latest commits on \
                  the remote are brought in even when the local default branch is behind.",
        ghd_behaviour: "Merges the local default branch as it is, which may be behind its remote.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(13709), Upstream::issue(19559), Upstream::issue(21545)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-core/src/remote.rs"],
    },

    /// Update from Default Branch rebases when pull.rebase is set.
    UPDATE_FROM_DEFAULT_REBASES = 859 "update-from-default-rebases" {
        title: "Update from Default Branch follows pull.rebase",
        summary: "When git config sets pull.rebase, Branch › Update from Default Branch rebases \
                  the current branch onto the default branch (with the usual force-push warning \
                  and conflict flow) instead of merging it in.",
        ghd_behaviour: "Always merges the default branch in.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(7956), Upstream::issue(16131)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-core/src/mco.rs"],
    },

    /// Delete Branch warns about unmerged commits and a stash.
    DELETE_BRANCH_WARNINGS = 860 "delete-branch-warnings" {
        title: "Delete Branch warns about unmerged commits and stashes",
        summary: "Delete Branch warns when the branch has commits that neither the default branch \
                  nor the branch's upstream contain, and when changes are stashed on it.",
        ghd_behaviour: "Only \"This action cannot be undone.\"",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(4214), Upstream::issue(13714)],
        code: &["crates/corvene-ui/src/dialogs/branch_dialogs.rs", "crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/branch_ops.rs"],
    },

    /// Undo banner after deleting a local branch.
    UNDO_DELETE_BRANCH = 861 "undo-delete-branch" {
        title: "Undo deleting a branch",
        summary: "Deleting a local branch shows a \"Deleted branch\" banner for 15 seconds whose \
                  Undo recreates the branch at the commit it pointed at (without its upstream; a \
                  branch deleted on the remote too stays deleted there).",
        ghd_behaviour: "Deleted branches can only be recovered from the reflog on the command line.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20750)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-core/src/mco.rs", "crates/corvene-ui/src/banner.rs"],
    },

    /// Fetch after deleting the checked-out branch.
    FETCH_AFTER_DELETING_CURRENT_BRANCH = 862 "fetch-after-deleting-current-branch" {
        title: "Fetch after deleting the current branch",
        summary: "Deleting the checked-out branch switches to the default branch and then fetches \
                  its remote in the background, so the commits of a just-merged pull request \
                  show up without a manual Fetch.",
        ghd_behaviour: "Switches to the default branch without fetching.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(15984)],
        code: &["crates/corvene-core/src/dispatcher.rs"],
    },

    /// A clear error when a branch is checked out in another worktree.
    EXPLAIN_BRANCH_IN_OTHER_WORKTREE = 863 "explain-branch-in-other-worktree" {
        title: "Explain branches checked out in another worktree",
        summary: "When deleting a branch fails because it, or the default branch Corvene would \
                  switch to, is checked out in another worktree, the error names that worktree \
                  and says what to switch first.",
        ghd_behaviour: "Shows git's \"used by worktree at\" errors, one after another.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22569)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/error.rs"],
    },

    /// Confirm before switching branch from the branch list.
    CONFIRM_BRANCH_SWITCH = 864 "confirm-branch-switch" {
        title: "Confirm before switching branches",
        summary: "Clicking a branch in the branch list asks \"Switch to <branch>?\" before \
                  checking it out.",
        ghd_behaviour: "Checks the branch out at once.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20410)],
        code: &["crates/corvene-ui/src/branch_list.rs", "crates/corvene-ui/src/dialogs/branch_dialogs.rs"],
    },

    /// Switch Branch › Discard my changes.
    SWITCH_BRANCH_DISCARD = 865 "switch-branch-discard" {
        title: "Switch Branch can discard changes",
        summary: "The Switch Branch dialog (shown for uncommitted changes) offers a third choice, \
                  \"Discard my changes\": its \"Discard Changes and Switch\" button discards \
                  every change (new files go to the Trash) and then switches.",
        ghd_behaviour: "Leave the changes in a stash or bring them along only.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(11491)],
        code: &["crates/corvene-ui/src/dialogs/branch_dialogs.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// Checking out a remote branch whose name is already a local branch.
    REMOTE_CHECKOUT_USES_LOCAL = 866 "remote-checkout-uses-local" {
        title: "Remote branches check out the local branch",
        summary: "Choosing a remote branch such as origin/foo while a local branch foo exists \
                  switches to the local foo (with the usual handling of uncommitted changes).",
        ghd_behaviour: "Tries to create foo again and fails with \"a branch named 'foo' already \
                        exists\".",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(4527)],
        code: &["crates/corvene-core/src/dispatcher.rs"],
    },

    /// Push and remote branch deletion name full refs.
    QUALIFIED_PUSH_REFSPECS = 867 "qualified-push-refspecs" {
        title: "Push branches by their full ref name",
        summary: "Push, Publish branch and deleting a branch on the remote name the branch as \
                  refs/heads/<name>, so a tag with the same name as the branch does not make \
                  them fail.",
        ghd_behaviour: "Pushes <name>:<name>; a tag called like the branch makes git stop with \
                        \"src refspec <name> matches more than one\".",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(7726)],
        code: &["crates/corvene-core/src/remote.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// Restore checks the stash still belongs to the checked-out branch.
    STASH_RESTORE_CHECKS_BRANCH = 868 "stash-restore-checks-branch" {
        title: "Restore stash checks the branch",
        summary: "Restore picks the stash by its commit and only while the branch it was made \
                  on is checked out; clicked during a branch switch, it stops with an error \
                  instead of applying the changes to the other branch.",
        ghd_behaviour: "Pops the stash entry as listed at the last refresh, even when a branch \
                        switch has just changed what is checked out.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(10651)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/branch_ops.rs"],
    },

    /// Stashing stops when it would reset assume-unchanged files.
    STASH_PROTECTS_ASSUME_UNCHANGED = 869 "stash-protects-assume-unchanged" {
        title: "Protect assume-unchanged files from stashing",
        summary: "Stashing (Stash All Changes, leaving changes on a branch, or Stash and \
                  Continue) stops with an explanation when a file marked assume-unchanged has \
                  local changes, because git would reset that file without saving it in the \
                  stash.",
        ghd_behaviour: "Stashes anyway; the assume-unchanged file's changes are lost.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20806)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-core/src/mco.rs", "crates/corvene-git/src/branch_ops.rs"],
    },

    /// Delete Branch names the remote branch it would delete.
    DELETE_REMOTE_NAMES_UPSTREAM = 870 "delete-remote-names-upstream" {
        title: "Delete Branch names the remote branch",
        summary: "The Delete Branch dialog's \"delete on the remote\" checkbox names the remote \
                  branch it would delete (for example origin/feature), and is not offered when \
                  that branch is the remote's default branch.",
        ghd_behaviour: "Says \"delete this branch on the remote\" and deletes the upstream, \
                        whatever its name: a local branch tracking origin/main deletes origin/main.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20638)],
        code: &["crates/corvene-ui/src/dialogs/branch_dialogs.rs"],
    },

    /// No "Will be saved as" for a name still being typed.
    BRANCH_NAME_TRAILING_SLASH_QUIET = 871 "branch-name-trailing-slash-quiet" {
        title: "Quiet branch name warning while typing a slash",
        summary: "A branch name box does not warn that the name will be changed while the only \
                  difference is a trailing / or . (as in feature/ on the way to feature/x).",
        ghd_behaviour: "Flashes \"Will be created as feature\" after each / typed, which reads \
                        as if slashes were not allowed.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(12275)],
        code: &["crates/corvene-ui/src/dialogs/branch_dialogs.rs", "crates/corvene-ui/src/dialogs/preferences.rs"],
    },

    /// Rename Branch opens with the name box focused.
    RENAME_BRANCH_FOCUSES_NAME = 872 "rename-branch-focuses-name" {
        title: "Rename Branch focuses the name",
        summary: "The Rename Branch dialog opens with the focus in the name box and the current \
                  name selected, so typing replaces it at once (as in Create a Branch).",
        ghd_behaviour: "Focuses the dialog's close button; the name box needs a click or Tab.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(17661)],
        code: &["crates/corvene-ui/src/dialogs/branch_dialogs.rs"],
    },

    /// More characters a branch name may not contain.
    BRANCH_NAME_FORBIDDEN_CHARS = 873 "branch-name-forbidden-chars" {
        title: "Forbidden branch name characters",
        summary: "Characters (written together, for example #&%) that Create a Branch, Rename \
                  Branch and the worktree dialogs replace with - in a branch name, along with \
                  those Git forbids; empty for none.",
        ghd_behaviour: "Only the characters Git forbids are replaced.",
        nature: Nature::Feature,
        kind: Kind::Text { placeholder: "#&%", validate: forbidden_branch_chars },
        corvene: Value::text(""), ghd: Value::text(""),
        familiar: Value::text(""), max: Value::text(""),
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22603)],
        code: &["crates/corvene-ui/src/dialogs/branch_dialogs.rs", "crates/corvene-ui/src/dialogs/worktree_dialogs.rs"],
    },

    /// A linked worktree's "Last fetched" counts the main repository's fetches.
    WORKTREE_SHARED_LAST_FETCHED = 874 "worktree-shared-last-fetched" {
        title: "Worktrees share the last fetch time",
        summary: "In a linked worktree, the Fetch button's \"Last fetched\" time also counts \
                  fetches made from the main worktree (they update the same remote branches), \
                  so it does not say \"Never fetched\" right after a fetch elsewhere.",
        ghd_behaviour: "Reads only the worktree's own FETCH_HEAD, so a linked worktree shows \
                        \"Never fetched\" until it fetches itself.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22520)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/remote_ops.rs", "crates/corvene-git/src/paths.rs"],
    },

    /// Broken config files are named, and a broken .gitmodules does not stop a fetch.
    EXPLAIN_BAD_CONFIG = 875 "explain-bad-config" {
        title: "Explain broken Git config files",
        summary: "Adding a repository whose .git/config git cannot read says which file and line \
                  to fix, and a fetch that fails because .gitmodules cannot be read (for example \
                  a merge conflict in it) is retried without submodules.",
        ghd_behaviour: "Says the folder is not a Git repository, and fetching fails with git's \
                        \"bad config line\" error until .gitmodules is fixed.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(6200), Upstream::issue(6534)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-core/src/remote.rs", "crates/corvene-git/src/remote_ops.rs", "crates/corvene-git/src/error.rs"],
    },

    /// A missing repository folder is named instead of "Not a directory".
    GIT_SPAWN_ERROR_DETAILS = 876 "git-spawn-error-details" {
        title: "Name a missing repository folder",
        summary: "When git cannot start because the repository's folder is gone or is a file, \
                  the error says which folder is missing instead of \"could not run git: Not a \
                  directory\".",
        ghd_behaviour: "Shows \"spawn ENOTDIR\" or a similar system error that reads as if Git \
                        were broken.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(9887)],
        code: &["crates/corvene-git/src/process.rs", "crates/corvene-git/src/error.rs", "crates/corvene-core/src/flags/dispatch.rs"],
    },

    /// Closing the conflicts dialog asks to abort or keep the operation.
    CONFLICTS_DIALOG_CLOSE_GUARD = 877 "conflicts-dialog-close-guard" {
        title: "Ask before closing the conflicts dialog",
        summary: "The conflicts dialog of a merge, rebase or cherry-pick says that closing it \
                  keeps the operation in progress (resumed from the banner), and closing it asks \
                  whether to abort the operation or keep it in progress.",
        ghd_behaviour: "Closing the dialog silently leaves the operation in progress behind a \
                        banner.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(16957)],
        code: &["crates/corvene-ui/src/dialogs/mco_dialogs.rs"],
    },

    /// Conflicts show the banner instead of opening the dialog.
    CONFLICTS_OPEN_AS_BANNER = 878 "conflicts-open-as-banner" {
        title: "Conflicts as a banner",
        summary: "When a merge, rebase or cherry-pick stops on conflicts, the \"Resolve \
                  conflicts\" banner shows instead of the conflicts dialog opening over the \
                  window; the banner's View conflicts opens it.",
        ghd_behaviour: "The conflicts dialog opens at once.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(16702)],
        code: &["crates/corvene-core/src/mco.rs"],
    },

    /// Links in commit messages end where github.com ends them.
    LINKIFY_TRAILING_PUNCTUATION = 879 "linkify-trailing-punctuation" {
        title: "Links leave out trailing punctuation",
        summary: "In commit messages a URL stops before trailing punctuation (`.` `,` `:` `!` \
                  `?` quotes) and closing brackets it did not open, and may follow an opening \
                  bracket; an issue reference is found before any closing punctuation, so \
                  `[#12]` and `#12:` link to issue 12, as on github.com.",
        ghd_behaviour: "A full stop after a URL is part of the link, a URL right after `(` is not \
                        linked, and `[#12]` or `#12:` are plain text.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(16373), Upstream::issue(16939)],
        code: &["crates/corvene-core/src/text_tokens.rs", "crates/corvene-ui/src/selected_commit.rs"],
    },

    /// `owner/repo#123` and `owner/repo@sha` in commit messages.
    CROSS_REPOSITORY_ISSUE_LINKS = 880 "cross-repository-issue-links" {
        title: "Links to other repositories in commit messages",
        summary: "In a GitHub repository's commit messages `owner/repo#123` is one link to issue \
                  123 of that repository and `owner/repo@<sha>` one link to that commit, as \
                  github.com links them.",
        ghd_behaviour: "`owner/repo` stays plain text and `#123` links to issue 123 of the current \
                        repository; `owner/repo@<sha>` is not linked.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(8161), Upstream::issue(8403), Upstream::issue(22311)],
        code: &["crates/corvene-core/src/text_tokens.rs", "crates/corvene-ui/src/selected_commit.rs"],
    },

    /// Issue titles in the tooltips of `#123` links.
    ISSUE_TITLE_TOOLTIPS = 881 "issue-title-tooltips" {
        title: "Issue titles on #123 links",
        summary: "Hovering a `#123` link in the selected commit's message shows \"#123 <issue \
                  title>\" for an open issue in the issue cache the commit box's # suggestions \
                  use (hovering loads it), else the link's URL.",
        ghd_behaviour: "The tooltip is the link's URL (older versions showed the commit title).",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(2257)],
        code: &["crates/corvene-ui/src/markdown.rs", "crates/corvene-ui/src/selected_commit.rs"],
    },

    /// The commit author's name links to their GitHub profile.
    COMMIT_AUTHOR_LINKS = 882 "commit-author-links" {
        title: "Link commit authors to their profiles",
        summary: "In a GitHub repository the selected commit's author name opens their GitHub \
                  profile when the login is known: a no-reply address, a signed-in account with \
                  that e-mail, or a collaborator from the commit box's @ suggestions.",
        ghd_behaviour: "The author name is plain text.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(3785)],
        code: &["crates/corvene-ui/src/selected_commit.rs", "crates/corvene-core/src/autocomplete.rs", "crates/corvene-models/src/lib.rs"],
    },

    /// No GitHub links for commits that are not on any remote.
    UNPUBLISHED_COMMIT_LINKS = 883 "unpublished-commit-links" {
        title: "No GitHub links for unpublished commits",
        summary: "\"View on GitHub\" (and Copy Commit URL and the commit details' SHA link) are \
                  disabled for a commit that no remote-tracking branch contains, such as one \
                  made on a detached HEAD or not pushed yet, since GitHub does not have it.",
        ghd_behaviour: "Every commit links to GitHub, which answers 404 for unpublished ones.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(1478)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/log.rs", "crates/corvene-ui/src/history.rs", "crates/corvene-ui/src/selected_commit.rs"],
    },

    /// Create a Tag says when the tag cannot be pushed.
    TAG_PUSH_PERMISSION_NOTE = 884 "tag-push-permission-note" {
        title: "Create a Tag notes a read-only repository",
        summary: "When GitHub says the account can only read the repository, Create a Tag adds \
                  \"You can't push this tag to <owner/name>\": the tag can be created but only \
                  exists on this computer.",
        ghd_behaviour: "Creates the tag without a word; pushing it later fails.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(9833)],
        code: &["crates/corvene-ui/src/dialogs/history_dialogs.rs"],
    },

    /// History reloads asked for during a page load are not dropped.
    HISTORY_LOAD_RACE = 885 "history-load-race" {
        title: "History keeps up with new commits",
        summary: "A History reload asked for while a page of commits is still loading (a refresh \
                  right after committing, switching first-parent mode) runs as soon as that page \
                  is in, and the next page continues from the tip the list was loaded from, so a \
                  commit made while scrolling neither duplicates rows nor hides the newest one.",
        ghd_behaviour: "A reload asked for during a load is dropped, so a new commit can be missing \
                        from History until the next refresh, and paging restarts from HEAD.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(13830), Upstream::issue(20687), Upstream::issue(22064)],
        code: &["crates/corvene-core/src/dispatcher.rs"],
    },

    /// A filter box above History searches the branch's commits.
    HISTORY_SEARCH = 886 "history-search" {
        title: "Search History",
        summary: "A \"Search commits\" box under \"Select Branch to Compare…\" searches the \
                  current branch's history in the background: words match the message, the \
                  author's name or e-mail (any case) and a lone hex word also an abbreviated SHA; \
                  author:<name>, before:<date> and after:<date> narrow it (dates like 2024-05-01 or \
                  2.weeks.ago). The matches replace the list until the box is cleared (Esc).",
        ghd_behaviour: "History cannot be searched; finding a commit means scrolling.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(7022), Upstream::issue(21407), Upstream::issue(22729), Upstream::issue(20102), Upstream::issue(20420), Upstream::issue(20335)],
        code: &["crates/corvene-core/src/history_filter.rs", "crates/corvene-git/src/log.rs", "crates/corvene-ui/src/history.rs"],
    },

    /// "Show History" on a file narrows History to that file.
    FILE_HISTORY = 887 "file-history" {
        title: "File history",
        summary: "\"Show History\" in the context menu of a changed file and of a file in a commit \
                  switches to History listing only the commits that touched it, following renames \
                  (git log --follow), under a removable \"History of <file>\" chip. Selecting one \
                  of them selects the file under the name it had in that commit.",
        ghd_behaviour: "A single file's history can only be seen on GitHub or with git.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(11990), Upstream::issue(21234)],
        code: &["crates/corvene-core/src/history_filter.rs", "crates/corvene-git/src/log.rs", "crates/corvene-ui/src/history.rs", "crates/corvene-ui/src/changes.rs", "crates/corvene-ui/src/selected_commit.rs"],
    },

    /// Reset to Commit offers Soft, Mixed and Hard.
    RESET_MODES = 888 "reset-modes" {
        title: "Soft, mixed and hard reset",
        summary: "History's Reset to Commit becomes a submenu: Soft keeps every change (the reset \
                  commits' changes staged), Mixed is GitHub Desktop's reset (changes kept unstaged, \
                  with its warning about changes in progress) and Hard discards everything after a \
                  confirmation that names the commits and the changed files it throws away.",
        ghd_behaviour: "Reset to Commit always runs a mixed reset (git reset <commit>).",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21418)],
        code: &["crates/corvene-ui/src/history.rs", "crates/corvene-core/src/dispatcher.rs", "crates/corvene-ui/src/dialogs/history_dialogs.rs"],
    },

    /// The compare view lists the files a merge would leave conflicted.
    COMPARE_SHOWS_CONFLICTS = 889 "compare-shows-conflicts" {
        title: "Compare lists conflicting files",
        summary: "Comparing to a branch checks in the background whether merging it would \
                  conflict (git merge-tree, nothing is touched) and, in both the Behind and the \
                  Ahead tab, shows \"N conflicting files\" above the commits, opening into the \
                  files' paths.",
        ghd_behaviour: "Only the Behind tab's merge button counts the conflicted files, without \
                        naming them.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(18672)],
        code: &["crates/corvene-core/src/compare.rs", "crates/corvene-git/src/rebase_ops.rs", "crates/corvene-ui/src/history.rs"],
    },

    /// History rows label branch tips.
    HISTORY_BRANCH_LABELS = 890 "history-branch-labels" {
        title: "Branch labels in History",
        summary: "Commit rows show a small outlined label (git branch icon and name, \"+N\" when \
                  several) where another local branch or the default branch's remote-tracking \
                  branch points, like git log --decorate, so History shows where the current \
                  branch started and which branches share its commits. Hovering lists them all.",
        ghd_behaviour: "Rows show tags only; nothing tells where other branches are.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(13547)],
        code: &["crates/corvene-ui/src/history.rs"],
    },

    /// A squash keeps its typed message after resolving conflicts.
    SQUASH_MESSAGE_SURVIVES_CONFLICTS = 891 "squash-message-survives-conflicts" {
        title: "Squash message survives conflicts",
        summary: "When a squash stops on a conflict, continuing it after the conflicts are \
                  resolved still gives the squashed commit the message typed in the Squash \
                  dialog; other commits the rebase replays keep their own messages.",
        ghd_behaviour: "After a conflict the squashed commit gets git's combined message of all \
                        the squashed commits and the typed one is lost.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(16129)],
        code: &["crates/corvene-core/src/mco.rs", "crates/corvene-git/src/rebase_ops.rs"],
    },

    /// History › Edit Commit Message… rewords an unpushed commit.
    EDIT_COMMIT_MESSAGE = 892 "edit-commit-message" {
        title: "Edit commit messages",
        summary: "The History context menu of a commit that is not pushed yet (and is no merge, \
                  with no merge after it) offers Edit Commit Message…: the squash message dialog \
                  opens on its message and Save rewrites it (an amend for the newest commit, else \
                  an interactive rebase that rewords it and replays the rest unchanged, local \
                  changes stashed meanwhile). The edited commit stays selected.",
        ghd_behaviour: "Only the newest commit's message can be changed, through Amend Commit.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(5219)],
        code: &["crates/corvene-ui/src/history.rs", "crates/corvene-core/src/mco.rs", "crates/corvene-git/src/rebase_ops.rs", "crates/corvene-ui/src/dialogs/mco_dialogs.rs"],
    },

    /// Commits from the compare view can be cherry-picked onto the current branch.
    CHERRY_PICK_INTO_CURRENT_BRANCH = 893 "cherry-pick-into-current-branch" {
        title: "Cherry-pick into the current branch",
        summary: "Cherry-picking commits listed in the compare view's Behind tab (they are on the \
                  compared branch) treats that branch as their source: the current branch can be \
                  picked as the target and they are copied onto it without a checkout. The single \
                  commit menu offers Cherry-pick Commit… there too.",
        ghd_behaviour: "The current branch cannot be chosen as the target, so commits seen while \
                        comparing cannot be brought into it from History.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(12688)],
        code: &["crates/corvene-core/src/mco.rs", "crates/corvene-ui/src/dialogs/mco_dialogs.rs", "crates/corvene-ui/src/history.rs"],
    },

    /// Cherry-pick onto a branch another worktree has checked out.
    CHERRY_PICK_INTO_WORKTREE_BRANCH = 894 "cherry-pick-into-worktree-branch" {
        title: "Cherry-pick into another worktree's branch",
        summary: "Choosing a target branch that another worktree has checked out copies the \
                  commits inside that worktree instead of failing on the checkout. It refuses \
                  when that worktree has uncommitted changes, and on a conflict the cherry-pick \
                  is undone there with a note to resolve it in that worktree.",
        ghd_behaviour: "The cherry-pick fails because git cannot check out a branch that another \
                        worktree uses.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22714)],
        code: &["crates/corvene-core/src/mco.rs", "crates/corvene-ui/src/banner.rs"],
    },

    /// Several local branches deleted at once from the branch list.
    BULK_DELETE_BRANCHES = 895 "bulk-delete-branches" {
        title: "Delete several branches at once",
        summary: "⌘-click and ⇧-click select several local branches in the branch list (not the \
                  current one) and their context menu offers Delete N Branches…: one confirmation \
                  lists them, marking the default branch, commits the default branch lacks, \
                  unpushed or unpublished commits, an upstream deleted on the remote and a last \
                  commit older than 30 days, with \"Also delete the remote branches\". The \
                  branches are deleted one after another and failures reported together; with \
                  undo-delete-branch on, one Undo recreates every deleted local branch.",
        ghd_behaviour: "Branches are deleted one at a time, each with its own confirmation.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(9824), Upstream::issue(13310), Upstream::issue(20603), Upstream::issue(20708)],
        code: &["crates/corvene-ui/src/branch_list.rs", "crates/corvene-ui/src/dialogs/delete_branches.rs", "crates/corvene-core/src/delete_branches.rs", "crates/corvene-ui/src/banner.rs"],
    },

    /// Branches deleted on the remote grouped at the end of the branch list.
    BRANCH_UPSTREAM_GONE_GROUP = 896 "branch-upstream-gone-group" {
        title: "Group branches deleted on the remote",
        summary: "Local branches whose upstream was deleted on the remote (and pruned by a fetch) \
                  move out of Recent Branches and Other Branches into a \"Deleted on Remote\" \
                  group at the end of the branch list, ready to be cleaned up. The current and \
                  the default branch stay where they are.",
        ghd_behaviour: "Such branches are listed among the others.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(11396)],
        code: &["crates/corvene-ui/src/branch_list.rs"],
    },

    /// Branches pinned to a group at the top of the branch list.
    PINNED_BRANCHES = 897 "pinned-branches" {
        title: "Pinned branches",
        summary: "The branch list's context menu offers Pin and Unpin; pinned branches (kept per \
                  repository) are listed in a Pinned group below the default branch and above \
                  Recent Branches instead of in their usual group. The group is hidden while the \
                  list is filtered.",
        ghd_behaviour: "Only the most recently checked out branches are listed above the others.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(15767)],
        code: &["crates/corvene-ui/src/branch_list.rs", "crates/corvene-core/src/dispatcher.rs", "crates/corvene-models/src/lib.rs"],
    },

    /// Other Branches grouped into folders by their prefix.
    BRANCH_LIST_FOLDERS = 898 "branch-list-folders" {
        title: "Branch folders",
        summary: "In the branch list's Other Branches, branches sharing the part of their name \
                  before the first / (feature/a, feature/b; origin/… for remote-only branches) \
                  are grouped under a collapsible folder row showing how many branches it holds. \
                  Folders start collapsed and remember being opened per repository until \
                  Corvene quits; the list is flat while filtered.",
        ghd_behaviour: "One flat list.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(18194)],
        code: &["crates/corvene-ui/src/branch_list.rs"],
    },

    /// Tags in the branch list's filter results, and Fetch All Tags.
    TAGS_IN_BRANCH_LIST = 899 "tags-in-branch-list" {
        title: "Tags in the branch list",
        summary: "Typing in the branch list's filter also lists the matching tags in a Tags \
                  group; choosing one checks out its commit (detached HEAD, after the usual \
                  confirmation). Repository › Fetch All Tags fetches every tag of the remote \
                  (git fetch --tags), not only those pointing into fetched history.",
        ghd_behaviour: "Tags cannot be checked out from the branch list, and fetching brings only \
                        the tags of fetched commits.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20299)],
        code: &["crates/corvene-ui/src/branch_list.rs", "crates/corvene-core/src/branch_tags.rs", "crates/corvene-git/src/remote_ops.rs", "crates/corvene-ui/src/app_menu.rs"],
    },

    // ---- 900 Performance ----

    /// Diffs of neighbouring files and commits computed ahead of time.
    PREFETCH_DIFFS = 901 "prefetch-diffs" {
        title: "Prefetch diffs",
        summary: "While a file or commit is shown, the diffs of the files and commits next to it \
                  are computed in the background, so moving the selection with the arrow keys \
                  shows the next diff at once. Costs a few extra git processes per selection.",
        ghd_behaviour: "Runs git for a diff when its file or commit is selected.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-core/src/diff_cache.rs"],
    },

    /// The watcher refreshes on the first change of a burst.
    FS_WATCHER_LEADING_EDGE = 902 "fs-watcher-leading-edge" {
        title: "Filesystem watcher: refresh at the first change",
        summary: "A change made outside Corvene (a save in the editor) refreshes at once instead of \
                  after the watcher's debounce; a burst of changes still ends with one refresh \
                  after it settles.",
        ghd_behaviour: "No watcher.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-core/src/watcher.rs"],
    },

    /// Save refreshed stat data after a slow status.
    REFRESH_STALE_INDEX = 903 "refresh-stale-index" {
        title: "Refresh the index after a slow status",
        summary: "When git status took over half a second, Corvene runs git update-index --refresh \
                  once (at most once a minute), so git stops re-reading files whose timestamps \
                  changed without their contents (a copied or restored checkout, a formatter): \
                  status on a 50,000-file tree went from 6 s to 0.2 s. It briefly holds \
                  index.lock, like any git command that writes the index.",
        ghd_behaviour: "Runs status without ever writing the index, so such a tree stays slow \
                        until a git command run elsewhere refreshes it.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-git/src/status.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// The watcher's debounce.
    FS_WATCHER_DEBOUNCE_MS = 904 "fs-watcher-debounce-ms" {
        title: "Filesystem watcher debounce",
        summary: "How long the watcher waits after the last change before refreshing.",
        ghd_behaviour: "No watcher.",
        nature: Nature::Feature,
        kind: Kind::Number { min: 50, max: 5000, unit: Some("ms") },
        corvene: Value::Number(300), ghd: Value::Number(300),
        familiar: Value::Number(300), max: Value::Number(300),
        restart: false, visible: false, availability: available,
        upstream: &[],
        code: &["crates/corvene-core/src/watcher.rs"],
    },

    /// The Git LFS check reads .gitattributes instead of `git lfs track`.
    LFS_DETECT_BY_ATTRIBUTES = 905 "lfs-detect-by-attributes" {
        title: "Fast Git LFS detection",
        summary: "Whether a newly added repository uses Git LFS is decided from its committed \
                  .gitattributes files (plus the root one and info/attributes) instead of \
                  `git lfs track`, which walks every directory, untracked ones included.",
        ghd_behaviour: "Runs `git lfs track --json`, which can take minutes in a worktree with \
                        many untracked files.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: false, availability: available,
        upstream: &[Upstream::issue(5198)],
        code: &["crates/corvene-core/src/remote.rs", "crates/corvene-git/src/remote_ops.rs"],
    },

    /// The working directory status read by gitoxide in-process.
    IN_PROCESS_STATUS = 906 "in-process-status" {
        title: "In-process status",
        summary: "The changes list is read by gitoxide inside Corvene instead of by a git status \
                  process: the same files, codes, upstream and ahead/behind counts, without \
                  starting git on every refresh (about a third faster on a typical repository, \
                  the same on a 50,000-file one). Corvene runs git as before whenever gitoxide \
                  cannot read the repository.",
        ghd_behaviour: "Runs `git status --porcelain=2` on every refresh.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-git/src/status_gix.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// A commit's changed files read by gitoxide in-process.
    IN_PROCESS_COMMIT_FILES = 907 "in-process-commit-files" {
        title: "In-process commit files",
        summary: "The changed files and line counts of a selected commit (or range of commits) \
                  are read by gitoxide inside Corvene instead of by a git log process, with git's \
                  rename and copy detection. Corvene runs git as before whenever gitoxide cannot \
                  read them.",
        ghd_behaviour: "Runs `git log --raw --numstat` (or `git diff` for a range) for every \
                        selected commit.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-git/src/log_gix.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// The wgpu renderer draws opaque quads first with a depth test.
    OPAQUE_DEPTH_PASS = 908 "opaque-depth-pass" {
        title: "Renderer: opaque depth pass",
        summary: "Linux and Android: the solid insides of opaque panels, rows and lines are drawn \
                  first, front to back, with a depth buffer, so everything they cover is skipped \
                  by the GPU instead of being shaded and blended underneath. The picture is the \
                  same pixel for pixel; mobile GPUs spend most of a frame on that overdraw. \
                  Not yet tried on a phone.",
        ghd_behaviour: "Chromium's compositor; Corvene's renderer otherwise blends every quad \
                        over the previous one.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: wgpu_renderer_only,
        upstream: &[],
        code: &["vendor/gpui-pre-wgpu/src/wgpu_renderer.rs", "crates/corvene/src/main.rs"],
    },

    /// The wgpu renderer redraws only what changed since the last frame.
    DAMAGE_SCISSOR = 909 "damage-scissor" {
        title: "Renderer: redraw only what changed",
        summary: "Linux and Android: a frame that differs from the last one in a small area (a \
                  blinking caret, a hovered row) redraws only that area into a kept copy of \
                  the window and copies it to the screen. Not yet tried on a phone.",
        ghd_behaviour: "Chromium's compositor; Corvene's renderer otherwise redraws the whole \
                        window every frame.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: wgpu_renderer_only,
        upstream: &[],
        code: &["vendor/gpui-pre-wgpu/src/wgpu_renderer.rs", "crates/corvene/src/main.rs"],
    },

    /// Store writes committed on a background thread.
    BACKGROUND_STORE_WRITES = 910 "background-store-writes" {
        title: "Save settings in the background",
        summary: "Changed settings, the selected repository and the other things Corvene \
                  remembers are saved to disk by a background thread, so the click or drag that \
                  changed them does not wait 5-35 ms for the disk to confirm the save. They are \
                  read back at once, saved in the order they were made, and the rest is saved \
                  before Corvene quits; a crash loses at most the last few milliseconds.",
        ghd_behaviour: "Settings live in Chromium's localStorage, which writes to disk in the \
                        background.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-store/src/lib.rs", "crates/corvene/src/main.rs"],
    },

    // ---- 1000 Experimental ----

    /// Compile tree-sitter grammars an extension names but Corvene lacks.
    BUILD_GRAMMARS_FROM_SOURCE = 1001 "build-grammars-from-source" {
        title: "Build tree-sitter grammars from source",
        summary: "A Zed or Pulsar extension whose tree-sitter grammar Corvene does not bundle \
                  offers Build grammar…: after a consent sheet naming the repository, the commit \
                  and the compiler, Corvene downloads the grammar's source and compiles its parser \
                  with the system C compiler (the Xcode Command Line Tools) into a library it \
                  then loads. A grammar's code runs inside Corvene; only sources you trust.",
        ghd_behaviour: "No tree-sitter.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-extensions/src/tsbuild", "crates/corvene-core/src/extensions.rs"],
    },

    // ---- 1100 Repository (overflow) ----

    /// The Push button's tooltip says how much the push sends.
    PUSH_SIZE_TOOLTIP = 1101 "push-size-tooltip" {
        title: "Push size in the Push tooltip",
        summary: "Hovering the Push button shows roughly how much the push sends: the size of \
                  the new commits' files and folders (before git compresses them), and the \
                  Git LFS files among them, e.g. \"≈ 12 MiB to push (10 MiB in Git LFS)\".",
        ghd_behaviour: "The Push button has no tooltip.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(18736)],
        code: &["crates/corvene-ui/src/toolbar.rs", "crates/corvene-core/src/remote.rs", "crates/corvene-git/src/remote_ops.rs"],
    },

    /// A repository can sign in through git's credential helper.
    REPOSITORY_CREDENTIAL_HELPER = 1102 "repository-credential-helper" {
        title: "Credential helper per repository",
        summary: "Repository Settings › Remote has \"Use Git Credential Manager for This \
                  Repository\": fetch, pull and push then sign in with git's credential helpers \
                  (Git Credential Manager, the keychain, or a login saved in Corvene) instead of \
                  the signed-in account of that host, e.g. to work as a second GitHub account \
                  or with a token a single repository needs.",
        ghd_behaviour: "Git Credential Manager (Settings › Advanced) is only used for hosts \
                        without a signed-in account; a github.com repository always uses the \
                        account.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20432)],
        code: &["crates/corvene-core/src/remote.rs", "crates/corvene-ui/src/dialogs/repository_settings.rs", "crates/corvene-models/src/lib.rs"],
    },

    /// push.default=current counts as an upstream.
    IMPLICIT_UPSTREAM_PUSH_DEFAULT = 1103 "implicit-upstream-push-default" {
        title: "Respect push.default=current",
        summary: "A branch with no upstream that git pushes to its same-named branch on the \
                  remote anyway (push.default=current, the branch already pushed with a plain \
                  git push) shows Push, Pull or Fetch with its ahead/behind counts against that \
                  branch instead of Publish branch. Pushing or pulling records it as the \
                  upstream, as publishing would.",
        ghd_behaviour: "Shows Publish branch, as if the branch were not on the remote.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(13737)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-core/src/remote.rs", "crates/corvene-ui/src/toolbar.rs", "crates/corvene-ui/src/no_changes.rs"],
    },

    /// A Git LFS server's login is asked for that server.
    LFS_SERVER_AUTHENTICATION = 1104 "lfs-server-authentication" {
        title: "Sign in to a separate Git LFS server",
        summary: "When a push, pull or fetch fails because git-lfs could not sign in to an \
                  LFS server other than the remote (lfs.url), the Authentication Failed dialog \
                  asks for that server's username and password; once saved, the retry and \
                  later operations sign in with them.",
        ghd_behaviour: "The failure shows as a sign-in problem with the remote itself, or as \
                        git-lfs' raw output, and the LFS server's login cannot be entered.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(11575)],
        code: &["crates/corvene-core/src/remote.rs", "crates/corvene-git/src/git_errors.rs"],
    },
    /// Repository › Clean Untracked Files…: `git clean` with a preview.
    CLEAN_UNTRACKED_FILES = 1105 "clean-untracked-files" {
        title: "Clean untracked files",
        summary: "Repository › Clean Untracked Files… (also in the changes list's menu) lists \
                  what git clean would delete, the untracked files and folders, each with a \
                  checkbox. Include ignored files adds what .gitignore hides, such as build \
                  output. The ticked paths are deleted for good, not moved to the Trash.",
        ghd_behaviour: "Untracked files are discarded one by one from the changes list (to the \
                        Trash) and ignored files cannot be removed at all.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(15485)],
        code: &["crates/corvene-core/src/clean_untracked.rs", "crates/corvene-git/src/clean.rs", "crates/corvene-ui/src/dialogs/clean_untracked_files.rs"],
    },
    /// Repository › Apply Patch from File… / from Clipboard.
    APPLY_PATCH = 1106 "apply-patch" {
        title: "Apply patches",
        summary: "Repository › Apply Patch › From File… and From Clipboard show the files a \
                  patch touches and whether it applies as it is, then apply it as \
                  uncommitted changes. A patch that does not apply cleanly goes in with a \
                  three-way merge, and its conflicts show in the changes list. A series made \
                  with git format-patch can instead become commits (git am); if one patch \
                  fails, nothing is committed.",
        ghd_behaviour: "Patches can be created (Create Patch File) but not applied.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(8703)],
        code: &["crates/corvene-core/src/apply_patch.rs", "crates/corvene-git/src/patch_import.rs", "crates/corvene-ui/src/dialogs/apply_patch.rs"],
    },
    /// Repository Settings › Remote manages every remote and the push target.
    REMOTE_MANAGER = 1109 "remote-manager" {
        title: "Manage remotes",
        summary: "Repository Settings › Remote lists every remote: add one, rename it (git \
                  remote rename), remove it or change its URL, all applied on Save. It also \
                  picks where pushes go: the repository's push remote (remote.pushDefault) \
                  and the current branch's own (branch.<name>.pushRemote). With a push remote \
                  other than the upstream's, Push sends the branch to the same-named branch \
                  there and counts the commits it does not have yet, while Pull still follows \
                  the upstream, and a branch without an upstream is published there. Branch › \
                  Push To ▸ marks that remote.",
        ghd_behaviour: "Only the primary remote's URL can be changed, and pushes always go to the \
                        upstream's remote whatever git's push settings say.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(3512), Upstream::issue(18154)],
        code: &["crates/corvene-core/src/remote_manager.rs", "crates/corvene-ui/src/dialogs/repository_settings.rs", "crates/corvene-git/src/remote_ops.rs", "crates/corvene-core/src/remote.rs"],
    },

    /// Repository › Insights…: contributors, commits per week, file churn.
    REPOSITORY_INSIGHTS = 1110 "repository-insights" {
        title: "Repository insights",
        summary: "Repository › Insights… shows, in place of the diff, who contributed to the \
                  current branch (or every branch, or any branch picked) and how much (commits, \
                  lines added and removed), the commits of every week as bars, and the files \
                  with the most changed lines, over the past week, month, 3 or 6 months, year \
                  or all time. Merge commits are not counted. The numbers are read in the \
                  background with git log, can be stopped, and are kept for the session until \
                  the branch moves.",
        ghd_behaviour: "No statistics: the contributors and activity of a repository are only \
                        on GitHub's Insights pages, for its default branch.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-git/src/stats.rs", "crates/corvene-core/src/insights.rs", "crates/corvene-ui/src/insights_view.rs"],
    },
    /// Repository › Submodules…: list, initialize, update and sync submodules.
    SUBMODULES = 1111 "submodules" {
        title: "Submodules panel",
        summary: "Repository › Submodules… lists every submodule with its path, URL, the \
                  commit the repository records and the one checked out, and whether it is \
                  initialized, up to date or at another commit. Submodules can be \
                  initialized, updated (optionally recursively) and synced one by one or all \
                  at once, and opened as repositories of their own.",
        ghd_behaviour: "Submodules are updated after a branch switch only; there is no list of \
                        them and no way to initialize, update or sync one.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(7523), Upstream::issue(20921)],
        code: &["crates/corvene-core/src/submodules.rs", "crates/corvene-git/src/submodule.rs", "crates/corvene-ui/src/dialogs/submodules.rs"],
    },
    /// Repository › Sparse Checkout…: pick the folders to check out.
    SPARSE_CHECKOUT = 1112 "sparse-checkout" {
        title: "Sparse checkout",
        summary: "Repository › Sparse Checkout… shows the repository's folders with \
                  checkboxes; only the ticked folders and the files at the top are checked \
                  out (git sparse-checkout in cone mode). It can also be turned off again. \
                  While it is on, the Changes tab says so.",
        ghd_behaviour: "Every file is checked out. A repository set up for sparse checkout on \
                        the command line works, but nothing shows that files are missing.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(12567), Upstream::issue(22835)],
        code: &["crates/corvene-core/src/sparse_checkout.rs", "crates/corvene-git/src/sparse.rs", "crates/corvene-ui/src/dialogs/sparse_checkout.rs"],
    },
    /// Git LFS file locks in the Changes and History file menus.
    LFS_LOCKS = 1113 "lfs-locks" {
        title: "Git LFS file locks",
        summary: "In a repository that uses Git LFS on a server with file locking, the file \
                  menus of Changes and History can lock and unlock files, and locked files \
                  show a lock and who holds it. Repository administrators can force a lock \
                  someone else holds open.",
        ghd_behaviour: "Locks can only be taken and released with git lfs on the command line; \
                        nothing shows which files are locked.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(8419), Upstream::issue(22494)],
        code: &["crates/corvene-core/src/lfs_locks.rs", "crates/corvene-git/src/lfs_locks.rs", "crates/corvene-ui/src/changes.rs", "crates/corvene-ui/src/selected_commit.rs"],
    },
    /// The outcome of hooks that cannot stop an operation, in the sidebar.
    HOOK_RESULTS = 1114 "hook-results" {
        title: "Hook results after pull, merge, commit and switch",
        summary: "With Settings › Git › Hooks on, the hooks that run after an operation \
                  (post-merge, post-rewrite, post-commit, post-checkout) report back: when one \
                  failed or printed something, a list under the Changes and History tabs names \
                  the operation and each hook with a check or an alert and how long it took. \
                  Clicking a hook shows its output. The list stays until it is closed or the \
                  next operation runs hooks. A failed post-checkout no longer fails the switch \
                  with an error.",
        ghd_behaviour: "A failed post-merge or post-rewrite after a pull or merge, and a failed \
                        post-commit, go unseen; whatever these hooks print is dropped. A failed \
                        post-checkout shows as an error although the branch was switched.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22476)],
        code: &["crates/corvene-git/src/hooks.rs", "crates/corvene-core/src/hooks.rs", "crates/corvene-ui/src/hook_results.rs"],
    },


    // ---- 1200 History & branches (overflow) ----

    /// Branch list rows name the author of the branch's newest commit.
    BRANCH_LIST_TIP_AUTHOR = 1201 "branch-list-tip-author" {
        title: "Last author in the branch list",
        summary: "Branch list rows show, dimmed after the date, the author of the branch's newest \
                  commit. Git does not record who created a branch, so this is the last person \
                  who committed to it (or whose commit it points at), not its creator.",
        ghd_behaviour: "Rows show the branch name and the date of its newest commit only.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20728)],
        code: &["crates/corvene-ui/src/branch_list.rs", "crates/corvene-git/src/repo.rs", "crates/corvene-models/src/lib.rs"],
    },

    /// Update from the branch the current branch was created from.
    UPDATE_FROM_PARENT_BRANCH = 1202 "update-from-parent-branch" {
        title: "Update from the parent branch",
        summary: "Creating a branch from another branch than the default one records that \
                  branch (as git config branch.<name>.vscode-merge-base, the key VS Code uses, \
                  which is read when VS Code set it). While the parent still exists, Branch › \
                  Update from Default Branch becomes \"Update from <parent>\" and merges it \
                  instead, the same way.",
        ghd_behaviour: "Update always merges the default branch, so a branch stacked on another \
                        one has to be merged with Merge into Current Branch.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21842)],
        code: &["crates/corvene-core/src/dispatcher.rs", "crates/corvene-core/src/menu_state.rs", "crates/corvene-ui/src/app_menu.rs", "crates/corvene-git/src/config.rs"],
    },

    /// The compare view lists the files that changed since the compared branch.
    COMPARE_BRANCH_FILES = 1203 "compare-branch-files" {
        title: "Changed files when comparing branches",
        summary: "The compare view (History, comparing to a branch) has a Show Changed Files row \
                  that opens the Preview Pull Request dialog, titled Compare Branches and \
                  without a pull request button, with the compared branch as its base: the files \
                  the current branch changed since the two diverged, with their diffs. It works \
                  in any repository, GitHub or not.",
        ghd_behaviour: "Comparing lists commits only; the files changed between two branches \
                        appear only in Preview Pull Request, for GitHub repositories.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(13458)],
        code: &["crates/corvene-ui/src/history.rs", "crates/corvene-core/src/pull_request_preview.rs", "crates/corvene-ui/src/dialogs/open_pull_request.rs"],
    },

    /// Another branch's stash can be restored onto the current branch.
    RESTORE_STASH_FROM_OTHER_BRANCH = 1204 "restore-stash-from-other-branch" {
        title: "Restore another branch's stash",
        summary: "In the branch list, the menu of a branch that has a stash has Restore Stash \
                  Here: with no local changes, the stash is restored onto the current branch \
                  and leaves the other branch. Conflicts are handled as for any restore.",
        ghd_behaviour: "A stash can only be restored on the branch it was made on, after \
                        switching to it.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(13156)],
        code: &["crates/corvene-ui/src/branch_list.rs", "crates/corvene-core/src/stash_flows.rs"],
    },

    /// Conflicted LFS files are resolved by picking a side.
    LFS_CONFLICTS_PICK_A_SIDE = 1205 "lfs-conflicts-pick-a-side" {
        title: "Pick a side for conflicted LFS files",
        summary: "A conflicted file stored in Git LFS (filter=lfs in .gitattributes) is a \
                  manual conflict in the conflicts dialog: Use the modified file from one \
                  branch or the other. Git merges the small LFS pointer files as text, so the \
                  conflict markers sit in the pointer and editing the file cannot resolve it.",
        ghd_behaviour: "The file is offered as a text conflict to open in the editor, which \
                        shows the pointer with conflict markers.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(7166)],
        code: &["crates/corvene-git/src/status.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// A merge that cannot be aborted says which files are in the way.
    EXPLAIN_MERGE_ABORT_FAILURE = 1206 "explain-merge-abort-failure" {
        title: "Explain why a merge cannot be aborted",
        summary: "When aborting a conflicted merge fails because files changed after the merge \
                  started (git's \"Entry … not uptodate. Cannot merge.\"), the error names those \
                  files and says to discard or stash their changes, then abort again.",
        ghd_behaviour: "Shows git's raw output.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(19973)],
        code: &["crates/corvene-core/src/mco.rs", "crates/corvene-git/src/git_errors.rs"],
    },

    /// Switch Branch warns that the branch the changes go to is behind.
    SWITCH_WARNS_TARGET_BEHIND = 1207 "switch-warns-target-behind" {
        title: "Warn before bringing changes to a branch that is behind",
        summary: "In the Switch Branch dialog, with \"Bring my changes\" chosen, a warning says \
                  when the branch is behind its upstream (as of the last fetch): \"<branch> is \
                  N commits behind <upstream>. Pull it before bringing your changes to avoid \
                  conflicts.\"",
        ghd_behaviour: "The changes are brought over without a word; pulling afterwards may \
                        conflict with them.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(18090)],
        code: &["crates/corvene-ui/src/dialogs/branch_dialogs.rs", "crates/corvene-core/src/stash_flows.rs"],
    },

    /// Undo Commit brings back which lines were in the commit.
    UNDO_RESTORES_LINE_SELECTION = 1208 "undo-restores-line-selection" {
        title: "Undo restores the line selection",
        summary: "After Undo (the undo bar or History's Undo Commit), the lines the commit \
                  had are the ticked ones: in a file only partly committed, the lines left \
                  out of the commit come back unticked, so the same commit can be made again \
                  after a fix to its message.",
        ghd_behaviour: "Every line of the undone files comes back ticked, including the ones \
                        that had been left out of the commit.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(19138)],
        code: &["crates/corvene-core/src/line_selection.rs", "crates/corvene-core/src/dispatcher.rs", "crates/corvene-git/src/diff.rs"],
    },

    /// Say when the current branch was deleted on the remote.
    CURRENT_BRANCH_DELETED_HINT = 1209 "current-branch-deleted-hint" {
        title: "Say when the current branch was deleted on the remote",
        summary: "When the current branch's upstream no longer exists on the remote (deleted \
                  there, e.g. after its pull request was merged, and pruned by a fetch), the \
                  push/pull button reads Publish branch, \"Deleted on origin\", \
                  with an alert icon, and a note under the Commit button says the same.",
        ghd_behaviour: "The button reads Fetch origin as if nothing were wrong, while clicking \
                        it pushes the branch again.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(13254)],
        code: &["crates/corvene-ui/src/toolbar.rs", "crates/corvene-ui/src/changes.rs", "crates/corvene-core/src/remote.rs"],
    },

    /// Branch › Push To ▸ and Fetch From ▸ another remote.
    PUSH_TO_OTHER_REMOTE = 1210 "push-to-other-remote" {
        title: "Push to and fetch from other remotes",
        summary: "In a repository with several remotes, the Branch menu has Push To ▸ and \
                  Fetch From ▸ submenus listing them: the current branch is pushed to the \
                  same-named branch of the remote picked, or that remote is fetched, while the \
                  branch's upstream stays what it was.",
        ghd_behaviour: "Push, pull and fetch use the branch's upstream remote (or origin) \
                        only.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20665)],
        code: &["crates/corvene-ui/src/app_menu.rs", "crates/corvene-core/src/remote.rs", "crates/corvene/src/main.rs"],
    },

    /// `.issuetracker` references link in commit messages.
    ISSUETRACKER_LINKS = 1211 "issuetracker-links" {
        title: ".issuetracker links",
        summary: "A repository's .issuetracker file ([issuetracker \"name\"] with a regex and a \
                  url using $1 for its groups, the format GitLens and Git Extensions read) turns \
                  the references it describes in commit messages into links, in any repository. \
                  Read when the repository is selected.",
        ghd_behaviour: "Only #123, @mentions and URLs are links in commit messages.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(19295)],
        code: &["crates/corvene-git/src/config.rs", "crates/corvene-core/src/forks.rs", "crates/corvene-core/src/text_tokens.rs"],
    },

    /// A guided `git bisect`: History marks, a bar with Good / Bad / Skip.
    BISECT = 1212 "bisect" {
        title: "Bisect",
        summary: "Find the commit that introduced a bug: mark a bad and a good commit from \
                  History's context menu (or Repository › Start Bisect with the current commit \
                  as bad), then answer Good, Bad or Skip for each commit Corvene checks out from \
                  a bar under the toolbar, which counts the steps left. History lists the range \
                  still in question with the marked commits labelled, and selects the first bad \
                  commit at the end; Stop Bisect returns to the branch. Uncommitted changes are \
                  stashed first, committing is off while bisecting, and a bisect started on the \
                  command line shows the same way.",
        ghd_behaviour: "No bisect: it has to be run on the command line, and History follows \
                        the detached HEAD it leaves.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-git/src/bisect.rs", "crates/corvene-core/src/bisect.rs", "crates/corvene-ui/src/bisect_bar.rs", "crates/corvene-ui/src/history.rs"],
    },

    /// Branch and merge lanes beside the History commits.
    COMMIT_GRAPH = 1213 "commit-graph" {
        title: "Commit graph",
        summary: "History draws a graph column at the left of the commits: a lane per line of \
                  development, a dot per commit, curves where branches fork and merge. The current \
                  branch's first-parent line is the thicker accent lane; past seven lanes the rest \
                  fold into a dotted column. A button before \"Select Branch to Compare…\" also \
                  lists every local and remote-tracking branch's commits (All branches), where \
                  commits cannot be squashed or reordered.",
        ghd_behaviour: "History is a flat list of the current branch's commits, so merges and \
                        parallel branches are not visible.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(1634), Upstream::issue(9452)],
        code: &["crates/corvene-core/src/commit_graph.rs", "crates/corvene-ui/src/commit_graph.rs", "crates/corvene-ui/src/history.rs", "crates/corvene-git/src/log.rs"],
    },

    /// Signature status on the selected commit and History rows.
    COMMIT_SIGNATURES = 1214 "commit-signatures" {
        title: "Commit signature status",
        summary: "A signed commit's header shows a Verified, Unverified, Bad signature or \
                  Can't verify badge (GPG, SSH and X.509) whose tooltip names the signer, key \
                  and why; History rows of signed commits get a small badge, coloured once the \
                  commit has been verified. Only the selected commit is checked with gpg or \
                  ssh-keygen, in the background and cached. In a GitHub repository a pushed \
                  commit shows GitHub's verdict, as github.com does, from one GraphQL query.",
        ghd_behaviour: "Commits are signed when git is configured to, but whether a commit is \
                        signed or its signature verifies is never shown.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(2339), Upstream::issue(8052)],
        code: &["crates/corvene-git/src/signature.rs", "crates/corvene-github/src/signatures.rs", "crates/corvene-core/src/signatures.rs", "crates/corvene-ui/src/signature_badge.rs", "crates/corvene-ui/src/selected_commit.rs", "crates/corvene-ui/src/history.rs"],
    },

    /// Verify the signatures of the History rows on screen too.
    VERIFY_VISIBLE_SIGNATURES = 1215 "verify-visible-signatures" {
        title: "Verify visible signatures",
        summary: "With commit signature status on, the History rows on screen are verified \
                  too, not just the selected commit: two gpg or ssh-keygen checks at a time \
                  locally, and in a GitHub repository one GraphQL query for up to 100 commits \
                  once scrolling stops.",
        ghd_behaviour: "No signature status at all.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(2339)],
        code: &["crates/corvene-core/src/signatures.rs", "crates/corvene-ui/src/history.rs"],
    },

    /// Repository › Recent Activity…: the reflog, to get lost commits back.
    RECENT_ACTIVITY = 1216 "recent-activity" {
        title: "Recent Activity",
        summary: "Repository › Recent Activity… lists what happened to HEAD (or the current \
                  branch) in plain words: commits, switches, resets, rebases (folded into one \
                  row unless Show Every Step is on), merges and pulls, newest first. Selecting \
                  a row shows the commit it left behind in the usual commit view; commits no \
                  branch or tag reaches any more are marked Unreachable and can be listed alone. \
                  Each row offers Create Branch Here, Reset Current Branch to Here (a hard \
                  reset, uncommitted changes stashed first) and Copy SHA, and a row that left a \
                  branch since deleted offers to restore it at the tip it had.",
        ghd_behaviour: "No reflog view: a commit lost to a reset or a rebase, or a deleted \
                        branch, can only be found with `git reflog` on the command line.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20750)],
        code: &["crates/corvene-git/src/reflog.rs", "crates/corvene-core/src/reflog.rs", "crates/corvene-ui/src/reflog_list.rs", "crates/corvene-ui/src/dialogs/reset_to_reflog_entry.rs"],
    },

    /// Branch › Compare…: any two refs, their commits and combined diff.
    COMPARE_REFS = 1218 "compare-refs" {
        title: "Compare any two refs",
        summary: "Branch › Compare… (and Compare with… in the menus of History commits and of \
                  the branch list) picks two branches, tags or commits. History then lists the \
                  commits of base..head, or with the ... toggle those only one side has, each \
                  marked with its side, and a Changed Files row shows the combined diff with \
                  its file list: straight from base to head for .., from their merge base for \
                  ..., as git diff reads the same range. Selecting a commit shows it as usual.",
        ghd_behaviour: "History compares the current branch with one other branch, commits only; \
                        the files changed between two branches show only in Preview Pull \
                        Request.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(19253)],
        code: &["crates/corvene-core/src/ref_compare.rs", "crates/corvene-core/src/pull_request_preview.rs", "crates/corvene-ui/src/dialogs/compare_refs.rs", "crates/corvene-ui/src/ref_compare_view.rs"],
    },

    /// Branch › Tags…: every tag, with checkout, push and delete.
    TAG_MANAGER = 1219 "tag-manager" {
        title: "Tag manager",
        summary: "Branch › Tags… lists every tag in place of History, newest first, with its \
                  date, its commit and the annotation's message, filtered as you type. \
                  Selecting a tag shows its commit. Each tag can be checked out (detached \
                  HEAD), pushed, deleted here, deleted from the remote, or its name copied; \
                  tags not pushed yet are marked.",
        ghd_behaviour: "Tags only show as labels on History's commits; the ones created in the \
                        app can be deleted until they are pushed.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(4829)],
        code: &["crates/corvene-git/src/history_ops.rs", "crates/corvene-core/src/tag_manager.rs", "crates/corvene-ui/src/tags_list.rs"],
    },

    /// `committer:` and `path:` terms in History's search box.
    HISTORY_SEARCH_TERMS = 1220 "history-search-terms" {
        title: "Search History by committer and path",
        summary: "History's \"Search commits\" box also understands committer:<name> (who \
                  committed, git log --committer) and path:<file or folder> (only commits that \
                  touched it, following renames like a file's history; selecting a commit \
                  selects the file). The box's tooltip lists every term.",
        ghd_behaviour: "History cannot be searched.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(22866)],
        code: &["crates/corvene-core/src/history_filter.rs", "crates/corvene-git/src/log.rs", "crates/corvene-ui/src/history.rs"],
    },

    /// Stacked branches marked in History and moved by squash / reorder.
    STACKED_BRANCH_REFS = 1221 "stacked-branch-refs" {
        title: "Stacked branches",
        summary: "History marks the current branch's commits the default branch does not have: \
                  a commit another local branch points at gets an accent line above it and the \
                  branch's label, and a line with the default branch's name shows where that \
                  branch begins. Squashing, reordering or editing a message over such commits \
                  first names those branches and offers to move them with their rewritten \
                  commits (git's update-ref, as rebase --update-refs does; a branch on a \
                  squashed commit lands on the combined one), which Undo puts back. With \
                  rebase.updateRefs set in git's config they move without asking.",
        ghd_behaviour: "Nothing marks other branches among the current branch's commits, and a \
                        squash or reorder leaves them on the old commits (even with \
                        rebase.updateRefs set).",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21256)],
        code: &["crates/corvene-core/src/stacked_refs.rs", "crates/corvene-git/src/rebase_ops.rs", "crates/corvene-ui/src/history.rs", "crates/corvene-ui/src/dialogs/warn_stacked_branches.rs"],
    },
    /// A branch that pushes to a differently named branch says so.
    PUSH_TARGET_GUARD = 1222 "push-target-guard" {
        title: "Pushes to a differently named branch",
        summary: "A branch whose upstream has another name (issue-134 tracking origin/main) \
                  shows where Push goes: the Push button reads \"To origin/main\" and the \
                  branch list shows \"→ origin/main\" on it. When that upstream is the \
                  remote's default branch, Push first asks whether to push to it or publish the \
                  branch under its own name. A new branch started from a remote branch no \
                  longer tracks it unless it has the same name.",
        ghd_behaviour: "Such a branch looks published and Push sends its commits to the \
                        upstream (git push origin issue-134:main) without asking; only the \
                        button's \"Push origin/main\" hints at it. A branch created from the \
                        default branch while only origin/main exists starts out tracking it.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21021)],
        code: &["crates/corvene-core/src/push_target.rs", "crates/corvene-core/src/remote.rs", "crates/corvene-core/src/dispatcher.rs", "crates/corvene-ui/src/toolbar.rs", "crates/corvene-ui/src/branch_list.rs", "crates/corvene-ui/src/dialogs/confirm_push_to_upstream.rs"],
    },

    /// Branch › Check Out from Fork…: a branch of any fork by its owner.
    CHECKOUT_FROM_FORK = 1223 "checkout-from-fork" {
        title: "Check out from a fork",
        summary: "Branch › Check Out from Fork… checks out a branch of another fork of the \
                  repository. Type its owner (alice), owner/name, owner:branch or paste its \
                  URL, or pick one of the suggested owners (open pull requests from forks and \
                  the newest forks on GitHub); its branches are listed to pick from. The fork \
                  gets a remote named after its owner (one already pointing at it is reused), \
                  only that branch is fetched, and it is checked out as owner/branch tracking \
                  it. The branch list offers it when an owner:branch filter matches nothing, \
                  and a fork's pull request offers its other branches from the context menu.",
        ghd_behaviour: "Branches of other forks are only reachable through their pull \
                        requests; anything else needs adding the remote on the command line.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(17939)],
        code: &["crates/corvene-core/src/fork_checkout.rs", "crates/corvene-ui/src/dialogs/checkout_from_fork.rs", "crates/corvene-git/src/remote_ops.rs"],
    },

    /// Rebase another branch onto the current one from the branch list.
    REBASE_BRANCH_ONTO_CURRENT = 1224 "rebase-branch-onto-current" {
        title: "Rebase a branch onto the current one",
        summary: "A local branch's menu in the branch list has \"Rebase <branch> onto Current \
                  Branch\", which replays that branch's commits on top of the branch you are on \
                  (git rebase <current> <branch>) with the usual progress, conflict handling, \
                  force push warning and Undo, and leaves the rebased branch checked out.",
        ghd_behaviour: "Only the current branch can be rebased, so bringing another branch up \
                        to date means switching to it first and rebasing it onto this one.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(15282)],
        code: &["crates/corvene-ui/src/branch_list.rs", "crates/corvene-core/src/mco.rs"],
    },

    /// The repo rules in the branch dialogs' rule message link to them.
    BRANCH_RULES_LINK = 1225 "branch-rules-link" {
        title: "Link to the rules that block a branch name",
        summary: "When Create Branch or Rename Branch says a name is restricted by repo rules, \
                  \"repo rules\" is a link to the repository's rulesets for that branch on \
                  GitHub, as in the commit form's rule warnings.",
        ghd_behaviour: "The message is plain text; the rules have to be found on GitHub by hand.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21824)],
        code: &["crates/corvene-ui/src/dialogs/branch_dialogs.rs"],
    },

    // ---- 1300 Changes & diffs (overflow) ----

    /// A Conventional Commits type menu next to the commit summary.
    CONVENTIONAL_COMMIT_TYPES = 1301 "conventional-commit-types" {
        title: "Conventional commit types",
        summary: "A type button before the commit summary opens a menu of Conventional \
                  Commits types (feat, fix, docs, style, refactor, perf, test, build, ci, \
                  chore, revert). Picking one puts `type: ` in front of the summary or \
                  replaces the type it has, keeping a scope and `!`; None removes the prefix. \
                  The button shows the summary's current type.",
        ghd_behaviour: "The summary is plain text; a type prefix has to be typed.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: OFF, ghd: OFF, familiar: OFF, max: OFF,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(1646)],
        code: &["crates/corvene-core/src/commit_message.rs", "crates/corvene-ui/src/changes.rs"],
    },

    /// The commit description is wrapped at 72 columns when committing.
    WRAP_COMMIT_BODY = 1302 "wrap-commit-body" {
        title: "Wrap the commit description at 72 characters",
        summary: "Committing breaks description lines longer than 72 characters at spaces, \
                  the width git and most tools expect. Line breaks you typed stay, list items \
                  and quotes continue under their text, and code (fenced or indented), table \
                  rows, trailers and long words such as URLs are left as they are.",
        ghd_behaviour: "The description is committed as typed, one long line per paragraph.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(62), Upstream::issue(1646)],
        code: &["crates/corvene-core/src/commit_message.rs", "crates/corvene-core/src/dispatcher.rs"],
    },
    /// Stash some lines: a hunk, a line or the checked changes.
    PARTIAL_STASH = 1303 "partial-stash" {
        title: "Stash lines",
        summary: "The diff's gutter menu has Stash Added Line, Stash Modified Lines and so on \
                  next to Discard, its text menu Stash N Selected Lines, and the changes list's \
                  menu Stash Checked Changes, which stashes the checked files and, in partly \
                  checked files, only the checked lines. New, deleted and renamed files go into the stash whole. \
                  With the stash list the result is a stash of its own beside the branch's; \
                  without it, it is the branch's stash, and the items are disabled while the \
                  branch has one. A stash restores over the changes left in the same files \
                  when the two do not touch the same lines.",
        ghd_behaviour: "Only all changes can be stashed (Stash All Changes).",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21882), Upstream::issue(21378)],
        code: &["crates/corvene-core/src/stash_flows.rs", "crates/corvene-git/src/partial_stash.rs", "crates/corvene-ui/src/diff_view.rs", "crates/corvene-ui/src/changes.rs"],
    },

    /// Diff lines can stay on one line and scroll sideways.
    DIFF_NO_WRAP = 1304 "diff-no-wrap" {
        title: "Diffs without line wrapping",
        summary: "View › Wrap Diff Lines and the diff's Diff Settings › Wrap Long Lines turn \
                  wrapping off: each line stays on one row and the text scrolls sideways \
                  (trackpad, shift-wheel or the horizontal scroll bar) under fixed line \
                  numbers. Line, hunk and text selection work as with wrapping.",
        ghd_behaviour: "Long diff lines always wrap.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(11052)],
        code: &["crates/corvene-ui/src/diff_view.rs", "crates/corvene-ui/src/diff_view_rows.rs", "crates/corvene-ui/src/app_menu.rs"],
    },

    /// The diff's ⌘F box shows a match count, arrows and a case toggle.
    DIFF_FIND_CONTROLS = 1305 "diff-find-controls" {
        title: "Find in diff controls",
        summary: "The diff's search box (⌘F) shows which match is selected out of how many \
                  (\"3 of 12\"), previous and next buttons, and an Aa button that makes the \
                  search case-sensitive. Enter, ⇧Enter and Esc work as before.",
        ghd_behaviour: "A bare text box: Enter and ⇧Enter move between matches with no count, \
                        and the search ignores case.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-ui/src/diff_view.rs", "crates/corvene-ui/src/diff_view_rows.rs"],
    },

    /// UTF-16 text files get a text diff, with line selection.
    UTF16_DIFFS = 1306 "utf16-diffs" {
        title: "Text diffs of UTF-16 files",
        summary: "Text files saved as UTF-16 (MetaEditor's .mq5 and .mqh files, Windows .reg \
                  and .rc files, some PowerShell scripts) show a highlighted text diff in \
                  Changes, History and comparisons instead of \"This binary file has \
                  changed.\" Lines can be selected for the commit, discarded, stashed and \
                  expanded as in any text file; what is written back stays UTF-16 with the \
                  file's byte order mark and line endings. Files with a byte order mark are \
                  recognized, and those without one when their bytes are clearly UTF-16.",
        ghd_behaviour: "git sees the zero bytes of UTF-16 text and calls the file binary, so \
                        only \"This binary file has changed.\" is shown and the file can only \
                        be committed or discarded whole.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20861)],
        code: &["crates/corvene-git/src/utf16.rs", "crates/corvene-git/src/diff.rs", "crates/corvene-git/src/log.rs", "crates/corvene-git/src/patch.rs", "crates/corvene-git/src/partial_stash.rs", "crates/corvene-core/src/dispatcher.rs"],
    },

    /// The commit button counts the files a commit stages.
    COMMIT_PROGRESS = 1307 "commit-progress" {
        title: "Commit progress on the commit button",
        summary: "A commit that takes more than a moment shows how far it got: the button \
                  reads \"Committing 1,234 of 5,000 files\" while git adds the files \
                  to the index, then \"Writing commit to main\" while git writes the commit \
                  and runs its hooks, with a thin bar along the bottom of the button. A \
                  commit that is done within 300 ms keeps the usual label.",
        ghd_behaviour: "The button says \"Committing 5000 files to main\" until git is done, \
                        however long that takes.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(19679)],
        code: &["crates/corvene-core/src/commit_progress.rs", "crates/corvene-git/src/commit.rs", "crates/corvene-core/src/dispatcher.rs", "crates/corvene-ui/src/changes.rs"],
    },

    /// Files Too Large › Add to .gitignore / Ignore and Untrack.
    IGNORE_OVERSIZED_FILES = 1308 "ignore-oversized-files" {
        title: "Ignore files that are too large",
        summary: "The Files Too Large warning before a commit has an \"Add to .gitignore\" \
                  button that appends the files' paths to the repository's .gitignore and goes \
                  back to the commit form. When some of them are already tracked, an \"Ignore \
                  and Untrack\" button also removes those from the index (git rm --cached), so \
                  the commit deletes them from the repository while the copies on disk stay.",
        ghd_behaviour: "The warning offers only Cancel and Commit Anyway; ignoring the files is \
                        up to the changes list's Ignore File menu item.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(18490)],
        code: &["crates/corvene-ui/src/dialogs/oversized_files.rs", "crates/corvene-core/src/commit_checks.rs", "crates/corvene-git/src/ignore.rs"],
    },

    /// The commit avatar's misattribution warning edits the name too.
    COMMIT_IDENTITY_UPDATE = 1309 "commit-identity-update" {
        title: "Fix your name and email from the commit warning",
        summary: "When the commit form's avatar warns that the commit will be misattributed, \
                  its popover has a Name box next to the account's emails, filled with the \
                  account's name. Update sets both. They go to the repository's own Git \
                  config when it already has a name or email there, else to the global one, \
                  and the warning says which config the email is in.",
        ghd_behaviour: "The popover changes only the email, always in the global Git config, \
                        so a repository with its own email keeps the warning.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(11760)],
        code: &["crates/corvene-ui/src/commit_message_avatar.rs", "crates/corvene-core/src/integrations.rs", "crates/corvene-git/src/repo.rs"],
    },
    /// Changes / History / stash file lists as a folder tree.
    FILE_LIST_TREE = 1310 "file-list-tree" {
        title: "Show changed files as a folder tree",
        summary: "A button at the end of the \"N changed files\" row, and View › Show \
                  Changes as Tree, switch the changes list, a commit's files in History and a \
                  stash's files between the flat list and a folder tree. Folder rows have a \
                  checkbox for every file inside (mixed when only some are included), a file \
                  count and their own menu. Clicking one selects its files; ← and → collapse \
                  and expand it. Collapsed folders are remembered per repository. The flat \
                  list stays the default.",
        ghd_behaviour: "Changed files are always one flat list with each file's directory \
                        before its name.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20624), Upstream::issue(20529)],
        code: &["crates/corvene-core/src/file_tree.rs", "crates/corvene-ui/src/file_tree_rows.rs", "crates/corvene-ui/src/changes.rs", "crates/corvene-ui/src/selected_commit.rs", "crates/corvene-ui/src/stash_view.rs"],
    },

    /// Every changed file's diff in one scrolling list.
    STACKED_DIFF = 1311 "stacked-diff" {
        title: "All files' diffs in one list",
        summary: "A stack button at the end of the \"N changed files\" header (Changes and \
                  History) shows every file's diff one under the other in one scrolling list, \
                  each under its own header, and remembers the choice per tab (also in \
                  Settings › Advanced). In Changes the stack holds the files included in the \
                  next commit, as a last look before committing; ⌘-click, ⇧-click or ⌘A in a \
                  file list stacks just the selected files. Clicking a file in the list \
                  scrolls to it, a header click folds that file, and the rows work as in the \
                  single-file diff (lines can still be included or excluded in Changes). \
                  Each file's diff loads as it scrolls into reach.",
        ghd_behaviour: "One file's diff at a time; several selected files show \"N files \
                        selected\".",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(5218), Upstream::issue(18184)],
        code: &["crates/corvene-ui/src/stacked_diff_view.rs", "crates/corvene-core/src/stacked_diff.rs", "crates/corvene-ui/src/diff_view.rs"],
    },

    /// ⌘F in the stacked list searches every file's diff.
    STACKED_DIFF_SEARCH = 1312 "stacked-diff-search" {
        title: "Search across the stacked diffs",
        summary: "With the files' diffs stacked in one list (1311-stacked-diff), ⌘F opens the \
                  diff's search box over the whole stack: Enter and ⇧Enter step through the \
                  matches of every file in order, wrapping, and the list scrolls to each one. \
                  Files whose diff has not loaded yet are loaded for the search, and with \
                  1305-diff-find-controls the count reads \"3 of 12\" across the files, with \
                  how many files are still loading; Aa matches case.",
        ghd_behaviour: "One file's diff at a time, so ⌘F searches one file.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20297)],
        code: &["crates/corvene-ui/src/stacked_diff_view.rs", "crates/corvene-ui/src/diff_view.rs"],
    },

    /// Named groups of changed files, committed or stashed one at a time.
    CHANGELISTS = 1313 "changelists" {
        title: "Changelists",
        summary: "Changed files can be grouped into named changelists: a file row's menu has \
                  Move to Changelist with New Changelist… (a name and an optional description) \
                  and the lists so far; the changes list then shows each list under its own \
                  header with a checkbox for its files, a count and a menu, and the files in \
                  no list under Changes. A header's Commit Only This List ticks its files, \
                  unticks the rest and puts the list's name and description into an empty \
                  commit form; Set as Active Changelist makes it the list new changes join \
                  and the only one the commit button commits (\"Commit <list> to <branch>\"), \
                  until Clear Active Changelist. Stash This List stashes the list's files \
                  (the stash list shows \"Stash of <list>\" and a restore brings a deleted \
                  list back), and the Switch Branch dialog can leave some lists stashed on \
                  the old branch while the rest travel. Files leave a list when they are \
                  committed or discarded; a list stays, empty, until it is deleted. Lists \
                  are saved per repository.",
        ghd_behaviour: "One flat list of changed files; the checkboxes alone pick the next \
                        commit, and a branch switch stashes or brings every change.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(16925)],
        code: &["crates/corvene-core/src/changelists.rs", "crates/corvene-ui/src/changes.rs", "crates/corvene-ui/src/dialogs/changelist_dialogs.rs", "crates/corvene-ui/src/dialogs/branch_dialogs.rs", "crates/corvene-ui/src/stash_list.rs"],
    },

    /// The CODEOWNERS owners of each changed file.
    CODE_OWNERS = 1314 "code-owners" {
        title: "Code owners",
        summary: "In a repository with a CODEOWNERS file (.github/, the root or docs/, read as \
                  GitHub does) each file in the Changes list, the selected commit's files and \
                  Preview Pull Request shows its owners: the first one and \"+N\", or a shield \
                  icon (Settings › Appearance), with every owner and the CODEOWNERS line in the \
                  tooltip. Owners that are you or one of your teams are highlighted. Changes \
                  follow the work tree's file, a commit its own, a pull request its base \
                  branch's, which also lists each owner and their file count above the files, \
                  other teams first.",
        ghd_behaviour: "Code owners only show on GitHub.com.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20909)],
        code: &["crates/corvene-core/src/codeowners.rs", "crates/corvene-ui/src/code_owners.rs", "crates/corvene-ui/src/changes.rs", "crates/corvene-ui/src/selected_commit.rs", "crates/corvene-ui/src/dialogs/open_pull_request.rs"],
    },
    /// Discard one file of a stash.
    DISCARD_STASH_FILE = 1315 "discard-stash-file" {
        title: "Discard a file from a stash",
        summary: "A file in the stash viewer has a menu with Discard from Stash (and Copy \
                  File Path), which takes the file out of the stash and leaves the rest of it \
                  where it was in the list. \
                  A Discarded banner offers Undo for 15 seconds. Discarding the stash's last \
                  file discards the stash.",
        ghd_behaviour: "A stash can only be restored or discarded whole.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-git/src/stash_file_discard.rs", "crates/corvene-core/src/stash_list.rs", "crates/corvene-ui/src/stash_view.rs", "crates/corvene-ui/src/banner.rs"],
    },
    /// Moved blocks of lines are marked apart from real removals and additions.
    DIFF_MOVED_LINES = 1316 "diff-moved-lines" {
        title: "Mark moved lines in diffs",
        summary: "Lines removed in one place and added back in another (like `git diff \
                  --color-moved`, indentation ignored, at least 20 letters or digits per block) \
                  keep their red or green but are faded, with a blue bar in the gutter that \
                  alternates shade between neighbouring blocks. Hovering the bar names the line \
                  the block moved from or to. Diff Settings › Mark Moved Lines turns it off.",
        ghd_behaviour: "A moved block shows as an unrelated removal and addition.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(17002)],
        code: &["crates/corvene-core/src/diff_line_class.rs", "crates/corvene-ui/src/diff_view.rs", "crates/corvene-ui/src/diff_view_rows.rs"],
    },
    /// Changes that only reformat code are marked apart from functional ones.
    DIFF_STYLISTIC_CHANGES = 1317 "diff-stylistic-changes" {
        title: "Mark stylistic changes in diffs",
        summary: "Changed lines whose code tokens stay the same once whitespace, line breaks, \
                  quote style, trailing commas and semicolons and parens around a whole value \
                  are set aside (a reformatter re-wrapping or re-indenting code) are faded \
                  with a grey gutter bar and no intra-line highlight, so the functional changes \
                  stand out. Diff Settings › Mark Stylistic Changes turns it off.",
        ghd_behaviour: "Every changed line looks the same. Hide Whitespace Changes only drops \
                        changes within a line and turns off line selection.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(20361)],
        code: &["crates/corvene-core/src/diff_line_class.rs", "crates/corvene-ui/src/diff_view.rs", "crates/corvene-ui/src/diff_view_rows.rs"],
    },
    /// CSV and TSV diffs as a table of cells.
    CSV_TABLE_DIFF = 1318 "csv-table-diff" {
        title: "CSV and TSV diffs as tables",
        summary: "A changed .csv or .tsv file opens as a table: the header row stays on top, \
                  added rows are green, removed rows red, and a modified row shows each changed \
                  cell's old value struck through above the new one. Columns are matched by \
                  name, added and removed ones are coloured in the header. The diff header's \
                  Text / Table switch goes back to the text diff (remembered for every such \
                  file). Files over 20,000 rows or that do not parse show the text diff.",
        ghd_behaviour: "CSV and TSV files only have the text diff.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(21518)],
        code: &["crates/corvene-core/src/csv_diff.rs", "crates/corvene-ui/src/csv_table_diff.rs", "crates/corvene-ui/src/diff_view.rs"],
    },
    /// A grey icon for files whose change is only their mode or patch metadata.
    TRIVIAL_CHANGE_ICON = 1319 "trivial-change-icon" {
        title: "Grey icon for mode-only and patch-metadata changes",
        summary: "In the Changes, History and stash file lists, a modified file whose only \
                  change is its mode (chmod) shows a grey modified icon, as does a .patch or \
                  .diff file whose only changed lines are patch metadata (index lines, hunk \
                  line numbers, the From commit id, the git version), which happens to a \
                  stack of patches when an earlier one is edited. The icon's tooltip says why.",
        ghd_behaviour: "Every modified file has the same yellow icon.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(17573)],
        code: &["crates/corvene-git/src/trivial_change.rs", "crates/corvene-ui/src/trivial_change.rs", "crates/corvene-core/src/dispatcher.rs"],
    },
    /// The image Difference view stays sharp at fractional scales.
    SHARP_IMAGE_DIFFERENCE = 1320 "sharp-image-difference" {
        title: "Sharp image Difference at any scale",
        summary: "The Difference view of a changed image is drawn at the exact device pixels \
                  it covers (shrunk by averaging, enlarged by repeating pixels, after the two \
                  images are blended), so a zoom level or a 1.5x display does not blur it or \
                  show unchanged edges as faint differences.",
        ghd_behaviour: "At a fractional device pixel ratio the Difference view is blurry and \
                        unchanged edges look changed.",
        nature: Nature::BugFix,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(19804)],
        code: &["crates/corvene-ui/src/image_diff.rs"],
    },
    /// Spreadsheet and database files diffed as tables.
    TABLE_FILE_DIFF = 1321 "table-file-diff" {
        title: "Spreadsheet and database diffs as tables",
        summary: "A changed Excel or OpenDocument spreadsheet (.xlsx, .xlsm, .xlsb, .xls, \
                  .ods), SQLite database (.sqlite, .db and similar) or dBASE table (.dbf) \
                  opens in the CSV table view instead of \"Binary file not shown\". Each sheet \
                  or database table is a tab above the table, marked when it was added, \
                  removed or changed. Files over 64 MB stay binary, and a sheet is cut off \
                  after 20,000 rows.",
        ghd_behaviour: "These files are binary, so the diff only says the file changed.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-git/src/table_file.rs", "crates/corvene-ui/src/diff_view.rs", "crates/corvene-ui/src/csv_table_diff.rs"],
    },

    /// PDF files diffed page by page.
    PDF_DIFF = 1322 "pdf-diff" {
        title: "PDF files as pages",
        summary: "A changed .pdf file shows its old and new pages with the image diff modes \
                  (2-up, Swipe, Onion Skin, Difference), one page at a time, with buttons for \
                  the previous, next and next changed page. The diff header's Pages / Text \
                  switch shows a line diff of the text on the pages instead (remembered for \
                  every PDF file). Encrypted or broken files and files over 64 MB stay binary.",
        ghd_behaviour: "PDF files show \"This binary file has changed.\"",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[Upstream::issue(5009), Upstream::issue(15875)],
        code: &["crates/corvene-core/src/pdf.rs", "crates/corvene-ui/src/pdf_diff.rs", "crates/corvene-ui/src/diff_view.rs", "crates/corvene-git/src/diff.rs"],
    },
    /// Archives diffed as folders of entries.
    ARCHIVE_DIFF = 1323 "archive-diff" {
        title: "Archive diffs as folders",
        summary: "A changed zip-style archive (.zip, .jar, .apk, .whl, .nupkg, .vsix, .epub \
                  and similar), tar archive (.tar, .tar.gz, .tgz, .tar.bz2, .tar.xz, .tar.zst) \
                  or compressed file (.gz, .bz2, .xz, .zst) opens as a folder instead of \
                  \"This binary file has changed.\": a tree of its entries marked added, \
                  removed, modified, renamed or changed only in permissions, times or \
                  compression, and each entry's own diff (text, image or an archive inside \
                  the archive, with a breadcrumb back). A banner says when a rebuilt archive \
                  holds the same contents. Entries can be stacked, saved, opened or sent to the \
                  external diff tool. Archives over 256 MB stay binary.",
        ghd_behaviour: "Archives are binary files: the diff only says the file changed.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-git/src/archive/mod.rs", "crates/corvene-core/src/archive_browse.rs", "crates/corvene-ui/src/archive_diff_view.rs"],
    },
    /// Archive entries listed under their archive in the file lists.
    ARCHIVE_ENTRIES_IN_FILE_LIST = 1328 "archive-entries-in-file-list" {
        title: "Archive entries in file lists",
        summary: "With archive diffs on, a changed archive in the Changes, History and stash \
                  file lists gets a disclosure arrow that lists its changed entries under it, \
                  like a folder. Clicking an entry shows the archive with that entry's diff.",
        ghd_behaviour: "An archive is one row like any other file.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-ui/src/file_tree_rows.rs", "crates/corvene-ui/src/changes.rs", "crates/corvene-ui/src/selected_commit.rs"],
    },

    /// Image diffs for HEIC, PSD, TIFF, camera RAW and other image files.
    MORE_IMAGE_FORMATS = 1326 "more-image-formats" {
        title: "More image formats in the image diff",
        summary: "Changed TIFF, PSD, ICNS, QOI, Netpbm, OpenEXR, Radiance HDR, DDS, farbfeld \
                  and gzipped SVG (.svgz) files show in the image diff (2-up, Swipe, Onion Skin, \
                  Difference) on every platform; HEIC/HEIF, AVIF, JPEG XL, JPEG 2000 and \
                  camera RAW files too where the system decodes them (macOS ImageIO, Windows \
                  WIC codecs). A file nothing decodes stays binary.",
        ghd_behaviour: "Only PNG, JPEG, GIF, ICO, WebP, BMP and AVIF files show as images; \
                        others show \"This binary file has changed.\"",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: OFF, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-platform/src/image_decode.rs", "crates/corvene-git/src/diff.rs", "crates/corvene-models/src/lib.rs", "crates/corvene-ui/src/image_diff.rs"],
    },

    /// Zoom and pan in the image diff.
    IMAGE_DIFF_ZOOM = 1327 "image-diff-zoom" {
        title: "Zoom in image diffs",
        summary: "The image diff has zoom buttons (out, fit, actual pixels, in); pinching, \
                  ⌘-scrolling or double-clicking zooms at the pointer and dragging or \
                  scrolling pans. Both sides of 2-up and the overlay modes zoom and pan \
                  together, and SVG drawings are redrawn sharp at every zoom level.",
        ghd_behaviour: "Images are always fitted to the diff area.",
        nature: Nature::Feature,
        kind: Kind::Bool,
        corvene: ON, ghd: OFF, familiar: ON, max: ON,
        restart: false, visible: true, availability: available,
        upstream: &[],
        code: &["crates/corvene-ui/src/image_diff.rs"],
    },

}

/// Ids and slugs that once existed; never reused.
pub const RETIRED: &[(u16, &str)] = &[
    // the syntax-extended pack it offered is compiled in
    (502, "optional-components"),
];

pub fn find(id: FlagId) -> Option<&'static FlagDef> {
    REGISTRY.iter().find(|def| def.id == id)
}

/// The definition of a registry id (a `FlagId` constant); unknown ids are a
/// registry bug and fall back to the first entry so callers stay total.
pub fn def(id: FlagId) -> &'static FlagDef {
    match find(id) {
        Some(def) => def,
        None => {
            debug_assert!(false, "unknown flag id {}", id.0);
            &REGISTRY[0]
        }
    }
}

pub fn by_slug(slug: &str) -> Option<&'static FlagDef> {
    REGISTRY.iter().find(|def| def.slug == slug)
}

/// Accepts `201-commit-templates`, `commit-templates`, `201` or `#201`.
pub fn lookup(key: &str) -> Option<&'static FlagDef> {
    let key = key.trim().trim_start_matches('#');
    REGISTRY
        .iter()
        .find(|def| def.slug == key || def.ident() == key || def.id.0.to_string() == key)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::flags::{Category, Preset};

    #[test]
    fn ids_are_unique_in_their_block_and_never_retired() {
        let mut seen = HashSet::new();
        for def in REGISTRY {
            assert!(seen.insert(def.id), "duplicate id {}", def.id.0);
            let category = Category::of_block(def.id.0 / 100)
                .unwrap_or_else(|| panic!("{} is outside every category block", def.id.0));
            let within = |block: u16| (block + 1..block + 100).contains(&def.id.0);
            assert!(
                within(category.block()) || category.overflow_block().is_some_and(within),
                "{} must be within its block",
                def.id.0
            );
            assert!(
                !RETIRED
                    .iter()
                    .any(|(id, slug)| *id == def.id.0 || *slug == def.slug),
                "{} reuses a retired id or slug",
                def.ident()
            );
        }
        assert!(
            REGISTRY.windows(2).all(|w| w[0].id < w[1].id),
            "registry must be sorted by id"
        );
    }

    #[test]
    fn slugs_are_unique_kebab_case() {
        let mut seen = HashSet::new();
        for def in REGISTRY {
            assert!(seen.insert(def.slug), "duplicate slug {}", def.slug);
            let ok = !def.slug.is_empty()
                && def.slug.split('-').all(|part| {
                    !part.is_empty()
                        && part
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
                });
            assert!(ok, "slug `{}` is not kebab-case", def.slug);
            assert!(
                !def.title.is_empty() && !def.summary.is_empty() && !def.ghd_behaviour.is_empty()
            );
        }
    }

    #[test]
    fn every_preset_value_fits_its_kind() {
        for def in REGISTRY {
            for preset in Preset::ALL {
                let value = def.value_for(preset);
                def.kind
                    .validate(value)
                    .unwrap_or_else(|err| panic!("{} {preset:?}: {err}", def.ident()));
            }
            if let Kind::Bool = def.kind {
                assert!(
                    def.corvene != Value::Bool(true) || def.max == Value::Bool(true),
                    "{}: on in Corvene, so on in Max",
                    def.ident()
                );
                assert_eq!(def.ghd, Value::Bool(false), "{}: GHD is off", def.ident());
            }
        }
    }

    #[test]
    fn lookup_accepts_every_key_form() {
        for key in [
            "201-commit-templates",
            "commit-templates",
            "201",
            "#201",
            " 201 ",
        ] {
            assert_eq!(
                lookup(key).map(|d| d.id),
                Some(ids::COMMIT_TEMPLATES),
                "{key}"
            );
        }
        assert!(lookup("201-something-else").is_none());
        assert!(lookup("999").is_none());
        assert_eq!(by_slug("fs-watcher").map(|d| d.id), Some(ids::FS_WATCHER));
        assert_eq!(def(ids::PRODUCT_NAME).ident(), "103-product-name");
        assert_eq!(ids::PRODUCT_NAME.to_string(), "103-product-name");
        assert_eq!(ids::PRODUCT_NAME.category(), Category::Appearance);
    }

    #[test]
    fn labels_for_selects() {
        let def = def(ids::PR_QUICK_VIEW_WIDTH);
        assert_eq!(def.label_for(&Value::text("min-400")), "At least 400 px");
        assert_eq!(def.label_for(&Value::text("other")), "other");
        assert_eq!(def.label_for(&Value::Bool(true)), "on");
    }

    #[test]
    fn branch_name_prefix_accepts_ref_safe_text() {
        for ok in ["", "feature/", "wasi-", "team/wasi/"] {
            assert!(branch_name_prefix(ok).is_ok(), "{ok}");
        }
        for bad in ["my feature/", "a:b", "/x", ".x", "a..b", "a//b", "x~"] {
            assert!(branch_name_prefix(bad).is_err(), "{bad}");
        }
    }
}
