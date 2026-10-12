//! GitHub Desktop's keyboard shortcuts (`build-default-menu.ts`). Its
//! `CmdOrCtrl` is GPUI's `secondary` (⌘ on macOS, Ctrl elsewhere); the
//! macOS-only app and Window menu shortcuts are bound on macOS only.
//!
//! Corvene: some bindings depend on flags ([`KeymapFlags`]); [`sync`]
//! rebuilds the keymap when one changes, so the menu bar (which reads its
//! shortcuts from the keymap) must be rebuilt after it. `618-keymap-overrides`:
//! `keymap.json` ([`corvene_core::keymap_file`]) is applied last, over the
//! defaults, the flags' bindings and gpui-kit's.

use std::rc::Rc;

use corvene_core::flags::{Flags, ids};
use corvene_core::keymap_file::{KeymapOverrides, resolve_action_name};
use gpui_kit::{App, DummyKeyboardMapper, Global, KeyBinding, KeyBindingContextPredicate};

use crate::actions::*;

/// The flags that add or remove key bindings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeymapFlags {
    /// `605-cmd-backspace-discards-files`: ⌘⌫ in the changes list discards
    /// the selected files instead of removing the repository.
    pub discard_selected_files: bool,
    /// `606-open-file-shortcuts`: ⇧⌘A / ⌥⌘O in the changes and commit file
    /// lists open the selected file in the editor / default program.
    pub open_file_shortcuts: bool,
    /// `607-no-push-shortcut`: ⌘P does not push.
    pub no_push_shortcut: bool,
    /// `608-open-in-shell-alt-shortcut`: ⌥⌘T also opens the shell.
    pub open_in_shell_alt_shortcut: bool,
    /// `609-emacs-list-keys`: ⌃N / ⌃P move through the changes and history lists.
    pub emacs_list_keys: bool,
    /// `610-diff-mode-shortcut`: ⌥⌘S switches between unified and split diffs.
    pub diff_mode_shortcut: bool,
    /// `611-copy-path-shortcuts`: ⌥⌘C / ⇧⌥⌘C copy the selected files' paths.
    pub copy_path_shortcuts: bool,
    /// `612-navigation-shortcuts`: ⌃⌘P pull requests, ⇧⌘] / ⇧⌘[ next /
    /// previous repository, ⌘3 the diff, ⌥↓ / ⌥↑ files from the diff.
    pub navigation_shortcuts: bool,
    /// `801-history-review-mode`: ⌃⌘S hides History's lists.
    pub history_review_mode: bool,
    /// `419-extra-zoom-inputs`: ⌘+ / ⇧⌘= (and the keypad's +) zoom in too.
    pub extra_zoom_inputs: bool,
    /// `429-multiple-windows`: ⌥⌘N opens a new window.
    pub multiple_windows: bool,
    /// `430-repository-tabs`: ⌥⌘→ / ⌥⌘← step through the tabs.
    pub repository_tabs: bool,
    /// `427-back-forward-navigation`: ⌃- / ⌃⇧- (Alt+← / Alt+→ off macOS)
    /// go back / forward.
    pub back_forward: bool,
    /// `619-arrow-keys-between-panes`: ← / → in the lists and the diff move
    /// focus to the pane on that side.
    pub arrow_keys_between_panes: bool,
}

impl KeymapFlags {
    pub fn from_flags(flags: &Flags) -> Self {
        Self {
            discard_selected_files: flags.bool(ids::CMD_BACKSPACE_DISCARDS_FILES),
            open_file_shortcuts: flags.bool(ids::OPEN_FILE_SHORTCUTS),
            no_push_shortcut: flags.bool(ids::NO_PUSH_SHORTCUT),
            open_in_shell_alt_shortcut: flags.bool(ids::OPEN_IN_SHELL_ALT_SHORTCUT),
            emacs_list_keys: flags.bool(ids::EMACS_LIST_KEYS),
            diff_mode_shortcut: flags.bool(ids::DIFF_MODE_SHORTCUT),
            copy_path_shortcuts: flags.bool(ids::COPY_PATH_SHORTCUTS),
            navigation_shortcuts: flags.bool(ids::NAVIGATION_SHORTCUTS),
            history_review_mode: flags.bool(ids::HISTORY_REVIEW_MODE),
            extra_zoom_inputs: flags.bool(ids::EXTRA_ZOOM_INPUTS),
            back_forward: flags.bool(ids::BACK_FORWARD_NAVIGATION),
            multiple_windows: flags.bool(ids::MULTIPLE_WINDOWS),
            repository_tabs: flags.bool(ids::REPOSITORY_TABS),
            arrow_keys_between_panes: flags.bool(ids::ARROW_KEYS_BETWEEN_PANES),
        }
    }
}

/// Electron's `togglefullscreen` role: ⌃⌘F on macOS, F11 elsewhere.
const FULL_SCREEN: &str = if cfg!(target_os = "macos") {
    "ctrl-cmd-f"
} else {
    "f11"
};

/// Corvene's ⌃⌘ chords (`612-navigation-shortcuts`, `801-history-review-mode`):
/// off macOS Ctrl is already the command key, so they become Ctrl+Alt.
const CTRL_CMD_P: &str = if cfg!(target_os = "macos") {
    "ctrl-cmd-p"
} else {
    "ctrl-alt-p"
};
const CTRL_CMD_S: &str = if cfg!(target_os = "macos") {
    "ctrl-cmd-s"
} else {
    "ctrl-alt-s"
};

/// Context of the shortcuts for menu items GitHub Desktop disables while a
/// popup is open (`menu-update.ts` `getMenuState`). `Workspace` adds
/// `Popup` to its key context while one is; the key then goes to the dialog
/// (a text field's ⌘⌫, ⌃H…) instead of acting on the window behind it. The
/// handlers check too, for the menu bar's own key equivalents and clicks.
const MENU: Option<&str> = Some("!Popup");

/// The bindings other crates (gpui-kit's components) installed before ours,
/// kept so [`sync`] can rebuild the whole keymap.
struct InstalledKeymap {
    base: Vec<KeyBinding>,
    flags: KeymapFlags,
    overrides: KeymapOverrides,
}

impl Global for InstalledKeymap {}

pub fn install(cx: &mut App) {
    let base = cx.key_bindings().borrow().bindings().cloned().collect();
    let flags = KeymapFlags::default();
    cx.bind_keys(bindings(flags));
    cx.set_global(InstalledKeymap {
        base,
        flags,
        overrides: KeymapOverrides::default(),
    });
}

/// Rebinds the keymap when `flags` or `overrides` differ from the installed
/// ones. `Some` when it did (rebuild the menu bar so its shortcuts follow),
/// with what of `overrides` could not be used.
pub fn sync(flags: KeymapFlags, overrides: &KeymapOverrides, cx: &mut App) -> Option<Vec<String>> {
    let installed = cx.try_global::<InstalledKeymap>()?;
    if installed.flags == flags && installed.overrides == *overrides {
        return None;
    }
    let mut all = installed.base.clone();
    all.extend(bindings(flags));
    let errors = apply_overrides(&mut all, overrides, cx);
    cx.clear_key_bindings();
    cx.bind_keys(all);
    let installed = cx.global_mut::<InstalledKeymap>();
    installed.flags = flags;
    installed.overrides = overrides.clone();
    Some(errors)
}

/// `618-keymap-overrides`: each entry drops every binding of its action and,
/// with a keystroke, binds that keystroke in the contexts the action had (an
/// action without one is bound like a menu item's shortcut). Returns what
/// could not be used.
fn apply_overrides(
    bindings: &mut Vec<KeyBinding>,
    overrides: &KeymapOverrides,
    cx: &App,
) -> Vec<String> {
    let mut errors = Vec::new();
    let registered = cx.all_action_names().to_vec();
    for (name, keystrokes) in &overrides.entries {
        let Some(action_name) = resolve_action_name(name, &registered) else {
            errors.push(format!("there is no action named `{name}`"));
            continue;
        };
        let action = match cx.build_action(action_name, None) {
            Ok(action) => action,
            Err(err) => {
                errors.push(format!("`{name}` cannot have a shortcut ({err})"));
                continue;
            }
        };
        // a keystroke that does not parse leaves the action's shortcuts alone
        if let Some(keystrokes) = keystrokes
            && let Err(err) = KeyBinding::load(
                keystrokes,
                action.boxed_clone(),
                None,
                false,
                None,
                &DummyKeyboardMapper,
            )
        {
            errors.push(format!(
                "`{keystrokes}` for `{name}` is not a keystroke ({err})"
            ));
            continue;
        }
        let mut contexts: Vec<Option<Rc<KeyBindingContextPredicate>>> = Vec::new();
        bindings.retain(|binding| {
            if binding.action().name() != action_name {
                return true;
            }
            let predicate = binding.predicate();
            if !contexts.contains(&predicate) {
                contexts.push(predicate);
            }
            false
        });
        let Some(keystrokes) = keystrokes else {
            continue;
        };
        if contexts.is_empty() {
            contexts.push(
                MENU.and_then(|m| KeyBindingContextPredicate::parse(m).ok())
                    .map(Rc::new),
            );
        }
        bindings.extend(contexts.into_iter().filter_map(|context| {
            KeyBinding::load(
                keystrokes,
                action.boxed_clone(),
                context,
                false,
                None,
                &DummyKeyboardMapper,
            )
            .ok()
        }));
    }
    errors
}

/// GHD `List.onKeyDown` `isHomeKey` / `isEndKey`: ⌘↑ / ⌘↓ go to the first
/// / last row on macOS; elsewhere only Home / End do, and Ctrl+↑ / Ctrl+↓
/// are plain arrows (the `ArrowUp` / `ArrowDown` branch ignores Ctrl).
fn list_end_binding(up: bool, context: &'static str) -> KeyBinding {
    let key = if up { "secondary-up" } else { "secondary-down" };
    match (cfg!(target_os = "macos"), up) {
        (true, true) => KeyBinding::new(key, SelectFirstFile, Some(context)),
        (true, false) => KeyBinding::new(key, SelectLastFile, Some(context)),
        (false, true) => KeyBinding::new(key, SelectPreviousFile, Some(context)),
        (false, false) => KeyBinding::new(key, SelectNextFile, Some(context)),
    }
}

fn bindings(flags: KeymapFlags) -> Vec<KeyBinding> {
    let mut bindings = vec![
        KeyBinding::new("down", SelectNextFile, Some("ChangesList")),
        KeyBinding::new("up", SelectPreviousFile, Some("ChangesList")),
        KeyBinding::new("secondary-a", SelectAllFiles, Some("ChangesList")),
        // flag `810` + `1311-stacked-diff`: every file of the commit, stacked
        KeyBinding::new("secondary-a", SelectAllFiles, Some("CommitFileList")),
        // the diff's text selection (GHD `select-all` / the browser's copy)
        KeyBinding::new("secondary-a", SelectAll, Some("Diff")),
        KeyBinding::new("secondary-c", Copy, Some("Diff")),
        KeyBinding::new("shift-down", ExtendSelectionDown, Some("ChangesList")),
        KeyBinding::new("shift-up", ExtendSelectionUp, Some("ChangesList")),
        list_end_binding(true, "ChangesList"),
        list_end_binding(false, "ChangesList"),
        KeyBinding::new("home", SelectFirstFile, Some("ChangesList")),
        KeyBinding::new("end", SelectLastFile, Some("ChangesList")),
        KeyBinding::new("space", ToggleIncludeSelected, Some("ChangesList")),
        KeyBinding::new("down", SelectNextFile, Some("HistoryList")),
        KeyBinding::new("up", SelectPreviousFile, Some("HistoryList")),
        KeyBinding::new("shift-down", ExtendSelectionDown, Some("HistoryList")),
        KeyBinding::new("shift-up", ExtendSelectionUp, Some("HistoryList")),
        list_end_binding(true, "HistoryList"),
        list_end_binding(false, "HistoryList"),
        KeyBinding::new("home", SelectFirstFile, Some("HistoryList")),
        KeyBinding::new("end", SelectLastFile, Some("HistoryList")),
        KeyBinding::new("enter", ReorderConfirm, Some("HistoryList")),
        // GHD `List.handleKeyDown` in the commit, stash and pull request
        // file lists (`FileList`)
        KeyBinding::new("down", SelectNextFile, Some("CommitFileList")),
        KeyBinding::new("up", SelectPreviousFile, Some("CommitFileList")),
        KeyBinding::new("shift-down", ExtendSelectionDown, Some("CommitFileList")),
        KeyBinding::new("shift-up", ExtendSelectionUp, Some("CommitFileList")),
        list_end_binding(true, "CommitFileList"),
        list_end_binding(false, "CommitFileList"),
        KeyBinding::new("home", SelectFirstFile, Some("CommitFileList")),
        KeyBinding::new("end", SelectLastFile, Some("CommitFileList")),
        // `1323-archive-diff`: an archive's entry tree
        KeyBinding::new("down", SelectNextFile, Some("ArchiveEntryList")),
        KeyBinding::new("up", SelectPreviousFile, Some("ArchiveEntryList")),
        list_end_binding(true, "ArchiveEntryList"),
        list_end_binding(false, "ArchiveEntryList"),
        KeyBinding::new("home", SelectFirstFile, Some("ArchiveEntryList")),
        KeyBinding::new("end", SelectLastFile, Some("ArchiveEntryList")),
        KeyBinding::new("enter", OpenArchiveEntry, Some("ArchiveEntryList")),
        KeyBinding::new("backspace", ArchiveUp, Some("ArchiveEntryList")),
        KeyBinding::new("down", SelectNextFile, Some("StashFileList")),
        KeyBinding::new("up", SelectPreviousFile, Some("StashFileList")),
        KeyBinding::new("shift-down", ExtendSelectionDown, Some("StashFileList")),
        KeyBinding::new("shift-up", ExtendSelectionUp, Some("StashFileList")),
        list_end_binding(true, "StashFileList"),
        list_end_binding(false, "StashFileList"),
        KeyBinding::new("home", SelectFirstFile, Some("StashFileList")),
        KeyBinding::new("end", SelectLastFile, Some("StashFileList")),
        KeyBinding::new("down", SelectNextFile, Some("PullRequestFileList")),
        KeyBinding::new("up", SelectPreviousFile, Some("PullRequestFileList")),
        KeyBinding::new(
            "shift-down",
            ExtendSelectionDown,
            Some("PullRequestFileList"),
        ),
        KeyBinding::new("shift-up", ExtendSelectionUp, Some("PullRequestFileList")),
        list_end_binding(true, "PullRequestFileList"),
        list_end_binding(false, "PullRequestFileList"),
        KeyBinding::new("home", SelectFirstFile, Some("PullRequestFileList")),
        KeyBinding::new("end", SelectLastFile, Some("PullRequestFileList")),
        KeyBinding::new("enter", CompareSelect, Some("CompareFilter")),
        KeyBinding::new("escape", CompareClear, Some("CompareFilter")),
        KeyBinding::new("down", SelectNextFile, Some("CompareFilter")),
        KeyBinding::new("up", SelectPreviousFile, Some("CompareFilter")),
        KeyBinding::new("escape", ReorderCancel, Some("HistoryList")),
        // `886-history-search`: Esc clears the filter, ↓ moves to the list
        KeyBinding::new("escape", CompareClear, Some("HistoryFilter")),
        KeyBinding::new("down", SelectNextFile, Some("HistoryFilter")),
        // GHD `FilterList.onFilterKeyDown`: ↓ / ↑ / Enter from the filter box
        KeyBinding::new("down", SelectNextFile, Some("RepositoryFilter")),
        KeyBinding::new("up", SelectPreviousFile, Some("RepositoryFilter")),
        KeyBinding::new("enter", FilterListPick, Some("RepositoryFilter")),
        KeyBinding::new("down", SelectNextFile, Some("BranchFilter")),
        KeyBinding::new("up", SelectPreviousFile, Some("BranchFilter")),
        KeyBinding::new("enter", FilterListPick, Some("BranchFilter")),
        KeyBinding::new("down", SelectNextFile, Some("PullRequestFilter")),
        KeyBinding::new("up", SelectPreviousFile, Some("PullRequestFilter")),
        KeyBinding::new("enter", FilterListPick, Some("PullRequestFilter")),
        KeyBinding::new("down", SelectNextFile, Some("WorktreeFilter")),
        KeyBinding::new("up", SelectPreviousFile, Some("WorktreeFilter")),
        KeyBinding::new("enter", FilterListPick, Some("WorktreeFilter")),
        KeyBinding::new("secondary-,", OpenSettings, MENU),
        // ⌘⇧, - macOS delivers the shifted character, so the chord is `cmd-<`
        KeyBinding::new("secondary-<", OpenFlags, MENU),
        KeyBinding::new("secondary-q", Quit, None),
        // File
        KeyBinding::new("secondary-n", NewRepository, MENU),
        KeyBinding::new("secondary-o", AddLocalRepository, MENU),
        KeyBinding::new("shift-secondary-o", CloneRepository, MENU),
        // Edit
        KeyBinding::new("secondary-f", Find, None),
        // View
        KeyBinding::new("secondary-1", ShowChanges, MENU),
        KeyBinding::new("secondary-2", ShowHistory, MENU),
        KeyBinding::new("secondary-t", ShowRepositoryList, MENU),
        KeyBinding::new("secondary-b", ShowBranchesList, MENU),
        KeyBinding::new("alt-secondary-w", ShowWorktreesList, MENU),
        KeyBinding::new("secondary-g", GoToSummary, MENU),
        KeyBinding::new("ctrl-h", ToggleStashedChanges, MENU),
        KeyBinding::new("secondary-l", ToggleChangesFilter, MENU),
        KeyBinding::new(FULL_SCREEN, ToggleFullScreen, None),
        KeyBinding::new("secondary-0", ResetZoom, None),
        KeyBinding::new("secondary-=", ZoomIn, None),
        KeyBinding::new("secondary--", ZoomOut, None),
        KeyBinding::new("secondary-9", ExpandActiveResizable, None),
        KeyBinding::new("secondary-8", ContractActiveResizable, None),
        KeyBinding::new("ctrl-tab", ToggleSection, MENU),
        // Repository
        KeyBinding::new("shift-secondary-p", Pull, MENU),
        KeyBinding::new("shift-secondary-t", Fetch, MENU),
        KeyBinding::new("secondary-backspace", RemoveRepository, MENU),
        KeyBinding::new("shift-secondary-g", ViewOnGitHub, MENU),
        KeyBinding::new("ctrl-`", OpenInShell, MENU),
        KeyBinding::new("shift-secondary-f", ShowInFinder, MENU),
        KeyBinding::new("shift-secondary-a", OpenInEditor, MENU),
        KeyBinding::new("shift-alt-secondary-a", OpenWith, MENU),
        KeyBinding::new("secondary-i", CreateIssue, MENU),
        // Branch
        KeyBinding::new("shift-secondary-n", NewBranch, MENU),
        KeyBinding::new("shift-secondary-w", NewWorktree, MENU),
        KeyBinding::new("shift-secondary-r", RenameBranch, MENU),
        KeyBinding::new("shift-secondary-d", DeleteBranch, MENU),
        KeyBinding::new("shift-secondary-backspace", DiscardAllChanges, MENU),
        KeyBinding::new("shift-secondary-s", StashAllChanges, MENU),
        KeyBinding::new("shift-secondary-u", UpdateFromDefaultBranch, MENU),
        KeyBinding::new("shift-secondary-b", CompareToBranch, MENU),
        KeyBinding::new("shift-secondary-m", MergeIntoCurrentBranch, MENU),
        KeyBinding::new("shift-secondary-h", SquashAndMergeIntoCurrentBranch, MENU),
        KeyBinding::new("shift-secondary-e", RebaseCurrentBranch, MENU),
        KeyBinding::new("shift-secondary-c", CompareOnGitHub, MENU),
        KeyBinding::new("alt-secondary-b", ViewBranchOnGitHub, MENU),
        KeyBinding::new("alt-secondary-p", PreviewPullRequest, MENU),
        KeyBinding::new("secondary-r", CreatePullRequest, MENU),
        // GHD `onMacOSWindowKeyDown` / Chromium: Shift+F10 (and the Menu key
        // off macOS) open the selected row's context menu
        KeyBinding::new("shift-f10", OpenRowContextMenu, Some("ChangesList")),
        KeyBinding::new("shift-f10", OpenRowContextMenu, Some("HistoryList")),
        KeyBinding::new("shift-f10", OpenRowContextMenu, Some("CommitFileList")),
        KeyBinding::new("shift-f10", OpenRowContextMenu, Some("RepositoryFilter")),
        KeyBinding::new("shift-f10", OpenRowContextMenu, Some("BranchFilter")),
        KeyBinding::new("shift-f10", OpenRowContextMenu, Some("BranchList")),
        // In-app
        KeyBinding::new("secondary-enter", Commit, Some("CommitMessage")),
        KeyBinding::new("escape", CloseFoldout, None),
        // context-free, so a text field's own Tab (indent) does not win;
        // the commit form's autocompletion takes it first
        KeyBinding::new("tab", FocusNext, None),
        KeyBinding::new("shift-tab", FocusPrevious, None),
    ];
    // Corvene flags. Added last: at equal depth a later binding wins, and a
    // context-free binding counts as the deepest context.
    if flags.discard_selected_files {
        bindings.push(KeyBinding::new(
            "secondary-backspace",
            DiscardSelectedFiles,
            Some("ChangesList"),
        ));
    }
    if !flags.no_push_shortcut {
        bindings.push(KeyBinding::new("secondary-p", Push, MENU));
    }
    if flags.open_in_shell_alt_shortcut {
        bindings.push(KeyBinding::new("alt-secondary-t", OpenInShell, MENU));
    }
    if flags.emacs_list_keys {
        for context in ["ChangesList", "HistoryList"] {
            bindings.extend([
                KeyBinding::new("ctrl-n", SelectNextFile, Some(context)),
                KeyBinding::new("ctrl-p", SelectPreviousFile, Some(context)),
            ]);
        }
    }
    if flags.diff_mode_shortcut {
        bindings.push(KeyBinding::new(
            "alt-secondary-s",
            ToggleDiffDisplayMode,
            MENU,
        ));
    }
    if flags.copy_path_shortcuts {
        for context in ["ChangesList", "CommitFileList"] {
            bindings.extend([
                KeyBinding::new("alt-secondary-c", CopySelectedFilePaths, Some(context)),
                KeyBinding::new(
                    "shift-alt-secondary-c",
                    CopySelectedRelativeFilePaths,
                    Some(context),
                ),
            ]);
        }
    }
    if flags.navigation_shortcuts {
        bindings.extend([
            KeyBinding::new(CTRL_CMD_P, ShowPullRequestsList, MENU),
            // ⇧⌘] / ⇧⌘[: macOS delivers the shifted character
            KeyBinding::new("secondary-}", NextRepository, MENU),
            KeyBinding::new("secondary-{", PreviousRepository, MENU),
            KeyBinding::new("secondary-3", FocusDiff, MENU),
            KeyBinding::new("alt-down", SelectNextFileFromDiff, Some("Diff")),
            KeyBinding::new("alt-up", SelectPreviousFileFromDiff, Some("Diff")),
        ]);
    }
    if flags.history_review_mode {
        bindings.push(KeyBinding::new(CTRL_CMD_S, ToggleHistoryReviewMode, MENU));
    }
    if flags.arrow_keys_between_panes {
        // a text field inside one of them binds the arrows deeper
        for context in ["ChangesList", "HistoryList", "CommitFileList", "Diff"] {
            bindings.extend([
                KeyBinding::new("left", FocusPaneLeft, Some(context)),
                KeyBinding::new("right", FocusPaneRight, Some(context)),
            ]);
        }
    }
    if flags.back_forward {
        // Xcode's ⌃- / ⌃⇧-; off macOS Ctrl+- is Zoom Out, so the browsers'
        // Alt+← / Alt+→. macOS may deliver ⌃⇧- as the shifted character:
        // the menu shows the first binding
        if cfg!(target_os = "macos") {
            bindings.extend([
                KeyBinding::new("ctrl--", NavigateBack, MENU),
                KeyBinding::new("ctrl-shift--", NavigateForward, MENU),
                KeyBinding::new("ctrl-_", NavigateForward, MENU),
            ]);
        } else {
            bindings.extend([
                KeyBinding::new("alt-left", NavigateBack, MENU),
                KeyBinding::new("alt-right", NavigateForward, MENU),
            ]);
        }
    }
    if flags.multiple_windows {
        // ⇧⌘N is Branch › New Branch, so New Window takes ⌥⌘N
        bindings.push(KeyBinding::new("alt-secondary-n", NewWindow, MENU));
    }
    if flags.repository_tabs {
        bindings.extend([
            KeyBinding::new("alt-secondary-right", NextTab, MENU),
            KeyBinding::new("alt-secondary-left", PreviousTab, MENU),
        ]);
    }
    if flags.extra_zoom_inputs {
        // macOS delivers ⇧⌘= as `cmd-+`; the keypad's + and - arrive as the
        // plain characters (GPUI has no keypad key names), so ⌘- covers
        // keypad minus already
        bindings.extend([
            KeyBinding::new("secondary-+", ZoomIn, None),
            KeyBinding::new("shift-secondary-=", ZoomIn, None),
        ]);
    }
    if flags.open_file_shortcuts {
        for context in ["ChangesList", "CommitFileList"] {
            bindings.extend([
                KeyBinding::new("shift-secondary-a", OpenSelectedFileInEditor, Some(context)),
                KeyBinding::new(
                    "alt-secondary-o",
                    OpenSelectedFileWithDefaultProgram,
                    Some(context),
                ),
            ]);
        }
    }
    #[cfg(not(target_os = "macos"))]
    for context in [
        "ChangesList",
        "HistoryList",
        "CommitFileList",
        "RepositoryFilter",
        "BranchFilter",
        "BranchList",
    ] {
        bindings.push(KeyBinding::new("menu", OpenRowContextMenu, Some(context)));
    }
    // GHD `Dialog.onKeyDown`: CmdOrCtrl+W dismisses the dialog (on macOS
    // ⌘W is the Window menu's Close Window below)
    #[cfg(not(target_os = "macos"))]
    bindings.push(KeyBinding::new("secondary-w", CloseFoldout, Some("Popup")));
    // Electron's `quit` role on Windows: the accelerator File › Exit shows
    #[cfg(windows)]
    bindings.push(KeyBinding::new("alt-f4", Quit, None));
    #[cfg(target_os = "macos")]
    bindings.extend([
        // the app menu and the Window menu (macOS only in GHD)
        KeyBinding::new("secondary-h", Hide, None),
        KeyBinding::new("alt-secondary-h", HideOthers, None),
        KeyBinding::new("secondary-m", Minimize, None),
        KeyBinding::new("secondary-w", CloseWindow, None),
    ]);
    // `1310-file-list-tree`: a list showing a tree adds `FileTree` to its
    // context; last, so these win over `619-arrow-keys-between-panes`
    bindings.extend([
        KeyBinding::new("left", CollapseFolder, Some("FileTree")),
        KeyBinding::new("right", ExpandFolder, Some("FileTree")),
    ]);
    bindings
}
