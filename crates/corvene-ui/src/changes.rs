//! Changes sidebar: filter header, "N changed files" row, file list, commit form.
//! `styles/ui/changes/{_changes-list,_commit-message}.scss`.
//!
//! Deviations (GHD `app/src/ui/changes/commit-message.tsx`):
//! - while committing, the summary and description get the read-only
//!   background but keep their text colour and still take typing (GHD's
//!   `readOnly={isCommitting}` dims the text and refuses it).
//! - a detached HEAD gets a commit warning (`730-detached-head-commit-warning`).
//! - committing is off while the repository bisects, with a warning that
//!   offers to stop (`1212-bisect`).
//! - Open in editor / default program act on every selected file, and the
//!   list menu has "Open All in <editor>" (`712-open-multiple-files`).
//! - files matching the `706-changes-hide-globs` patterns are left out of the
//!   list (they are still committed).
//! - while whitespace is hidden, files with whitespace changes only can be
//!   left out too, with a note under the list (`793-hide-whitespace-only-files`).
//! - ↑ / ↓ in an empty summary recall recent commit messages
//!   (`731-recall-commit-messages`).
//! - committing on the default branch asks first
//!   (`732-confirm-commit-to-default-branch`).
//! - the summary can be capped at 72 characters (`733-summary-max-length`).
//! - the commit options menu has Fixup Into ▸ an unpushed commit and Squash
//!   Fixup Commits (`799-fixup-commits`).
//! - a type button before the summary sets its Conventional Commits prefix
//!   (`1301-conventional-commit-types`).
//! - "Ignore All .x Files" items give the number of changed .x files
//!   (`718-ignore-menu-counts`).
//! - "Copy Diff" puts the selected files' changes on the clipboard as a patch
//!   (`714-copy-diff`).
//! - the Filter Options popover has "Renamed files" (`705-renamed-files-filter`).
//! - rows can show the file name without its directory
//!   (`702-changes-file-names-only`).
//! - the list can be ordered by status, file name or `diff.orderFile`
//!   (`703-changes-sort-order`).
//! - the filter text can match as a substring, suffix or exact name
//!   (`704-changes-filter-match`).
//! - a "Committing as Name <email>" line can sit above the summary
//!   (`734-commit-author-line`).
//! - included paths Windows cannot check out get a warning
//!   (`716-windows-invalid-names-warning`).
//! - the file menu can mark files assume-unchanged, the list menu clears the
//!   marks (`715-assume-unchanged`).
//! - a commit made outside Corvene with the drafted summary clears the draft
//!   (`738-clear-message-after-outside-commit`).
//! - the undo bar has a commit context menu (`739-undo-bar-menu`).
//! - an optional tag field tags the new commit (`737-commit-tag-field`).
//! - a single file's menu has "Open With…" (`713-open-file-with`).
//! - rows show "+N -M" before the status icon and the header the totals
//!   (GHD `changes-list.tsx` has none; flag `changes-line-counts`).
//! - a single file's menu adds "Ignore File In" (a nearer `.gitignore`,
//!   `info/exclude`, the global excludes file; flag `ignore-file-targets`).
//! - "Name <email>" in the co-authors box adds a co-author without a GitHub
//!   account (`735-free-form-co-authors`).
//! - co-author suggestions include recent commit authors, also for a name
//!   typed without @ (`770-co-authors-from-history`).
//! - the "N changed files" row ends in a spinner while Discard Changes runs
//!   or a status refresh is slow (`708-changes-busy-indicator`).
//! - rows follow the diff's row height, 9 px taller (`758-diff-line-height`).
//! - adding yourself or a second token for the same co-author is refused
//!   with a hint under the co-authors box (`769-co-author-validation`).
//! - each repository keeps its commit message, also across restarts
//!   (`766-persist-commit-drafts`).
//! - typing a character in the file list types it into the summary
//!   (`615-type-to-commit-summary`).
//! - → in the empty summary types the generated placeholder
//!   (`616-accept-summary-placeholder`).
//! - a single file's menu has "Ignore with Pattern…", a dialog to edit the
//!   pattern before it is added to `.gitignore` (`768-ignore-custom-pattern`).
//! - a file menu can stash the selected files (`777-stash-selected-files`).
//! - the Stashes section lists every stash in place of the Stashed Changes
//!   row, and the list menu has Stash All Changes with Message
//!   (`797-stash-list`, `crate::stash_list`).
//! - the list menu can move the changes to another worktree
//!   (`283-move-changes-to-worktree`).
//! - conflicts left by restoring a stash replace the commit form with a list
//!   of the files to resolve (`774-stash-conflict-flow`,
//!   `crate::stash_conflicts`).
//! - a message the amend, Undo Commit, the commit template or a commit puts
//!   in the form can be taken back with ⌘Z
//!   (`779-undoable-commit-message-replace`).
//! - the commit options gear has "Amend Last Commit"
//!   (`780-amend-from-commit-options`).
//! - a submodule's menu has "Open Submodule in Corvene", which a double-click
//!   does too (`284-open-submodule-from-changes`).
//! - at a rebase's `edit` stop without conflicts the commit form stays, with
//!   "Continue rebase" under it (`782-commit-during-rebase-edit`; GHD
//!   `continue-rebase.tsx` replaces the form during any rebase).
//! - while amending, the commit's author is an editable `Name <email>` with
//!   "Reset to my identity" (`783-amend-author`).
//! - the commit form's top edge drags the description box taller
//!   (`114-resizable-commit-message`).
//! - an untracked folder that is a repository has "Add as Submodule…"
//!   (`785-embedded-repo-commit`, `corvene_core::commit_checks`).
//! - the commit options gear has "Commit to New Branch…"
//!   (`787-commit-to-new-branch`, `corvene_core::new_branch_flows`).
//! - with a `prepare-commit-msg` / `commit-msg` hook, a commit message that
//!   fails the repository rules warns instead of blocking the commit
//!   (`340-message-rules-defer-to-hooks`; GHD `commit-message.tsx`
//!   `hasRepoRuleFailure` blocks it).
//! - a note under the commit button says when the branch's upstream was
//!   deleted on the remote (`1209-current-branch-deleted-hint`), and a lock
//!   that git signs the commit (`commit.gpgsign`, `526-commit-signing`).
//! - a protected branch that takes the user's pushes gets a note above the
//!   commit button (`339-protected-branch-bypass-note`; GHD `commit-warning`
//!   shows the protected warning only for unpushable branches).
//! - a commit that takes a while counts its staged files on the button
//!   ("Committing 1,234 of 5,000 files", then "Writing commit to main") over
//!   a thin bar (`1307-commit-progress`, `corvene_core::commit_progress`;
//!   GHD says "Committing 5000 files to main" until git is done).
//! - the list can show the files as a folder tree with folder checkboxes,
//!   folder selection and ← / → folding (`1310-file-list-tree`,
//!   `crate::file_tree_rows`; GHD `filter-changes-list.tsx` lists them flat).

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use corvene_core::archive_browse::ArchiveSurface;
use corvene_core::changelists::{Changelists, UNGROUPED};
use corvene_core::commit_progress::{CommitPhase, CommitProgress};
use corvene_core::filter::{no_results_message, option_count};
use corvene_core::{
    AppState, Author, DiffSelectionType, Dispatcher, FileListFilter, FileStatusKind, FilterOption,
    Foldout, Popup, RepoRuleEnforced, RepoRulesMetadataFailures, RepoRulesMetadataStatus, Tip,
    UnknownAuthorState, WorkingDirectoryFileChange, failed_rules, legacy_stealth_email,
};
use gpui_kit::component::Sizable;
use gpui_kit::component::input::{
    Copy, Cut, Enter, Escape, IndentInline, InlineToken, InputEvent, InputState, MoveDown,
    MoveRight, MoveUp, Paste, Redo, SelectAll, Textarea, TextareaState, Undo,
};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::widgets::GhdTooltip;
use crate::widgets::IconButtonA11y;
use crate::widgets::ListRowA11y;

use crate::actions::{
    Commit, DiscardSelectedFiles, ExtendSelectionDown, ExtendSelectionUp, OpenSelectedFileInEditor,
    OpenSelectedFileWithDefaultProgram, SelectAllFiles, SelectFirstFile, SelectLastFile,
    SelectNextFile, SelectPreviousFile, SpellAddToDictionary, SpellSuggestion0, SpellSuggestion1,
    SpellSuggestion2, SpellSuggestion3, SpellSuggestion4, ToggleCoAuthors, ToggleCommitSpellcheck,
    ToggleIncludeSelected,
};
use crate::autocompletion::{self, Autocompletion, Hit, PickHandler};
use crate::context_menu::{ContextMenu, IS_MAC, MenuItem, labels, mac_or};
use crate::diff_view::status_icon;
use crate::icons::{Octicon, octicon, spin};
use crate::relative_time::relative;
use crate::scrollbar::ScrollbarExt;
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::{
    InputMenuBuilder, button, checkbox, checkbox_tristate, primary_button, text_box_with_menu,
};

/// `712-open-multiple-files`: the most files one "Open …" item launches.
pub(crate) const MAX_BULK_OPEN: usize = 25;

/// How long a `769-co-author-validation` hint stays.
const CO_AUTHOR_HINT_DURATION: std::time::Duration = std::time::Duration::from_secs(4);

/// GHD `MaxTagNameLength` (`737-commit-tag-field`).
const MAX_TAG_NAME_LENGTH: usize = 245;

/// `114-resizable-commit-message`: the description box keeps two lines.
#[allow(non_snake_case)]
fn DESCRIPTION_MIN_HEIGHT() -> Pixels {
    zpx(40.)
}

/// Which commit-form field an autocompletion / spellcheck result belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommitField {
    Summary,
    Description,
    /// GHD `AuthorInput`: the co-author token field under the description.
    CoAuthors,
}

/// The commit button's focus handle, for GHD `App.onPopupDismissed`: closing
/// the Committing changes dialog after the commit moves focus back to the
/// button (see [`focus_commit_button`]).
#[derive(Default)]
struct CommitButtonFocus(Vec<(WindowId, FocusHandle)>);

impl Global for CommitButtonFocus {}

/// GHD `App.onPopupDismissed`: the Committing changes dialog closed once the
/// commit is done hands focus back to the commit button (its button under
/// the commit button is gone by then).
pub fn focus_commit_button(window: &mut Window, cx: &mut App) {
    let id = window.window_handle().window_id();
    let focus = cx
        .try_global::<CommitButtonFocus>()
        .and_then(|f| f.0.iter().find(|(w, _)| *w == id))
        .map(|(_, focus)| focus.clone());
    if let Some(focus) = focus {
        window.focus(&focus, cx);
    }
}

/// Window-space rectangles of a field's misspellings (see `summary_rects`).
type RectCache = Rc<RefCell<Vec<Option<Bounds<Pixels>>>>>;

/// One misspelled word of a commit-form field.
struct Misspelling {
    range: Range<usize>,
    word: String,
}

/// The misspelled word under the last right-click and its suggestions; the
/// context menu's items are index actions (`SpellSuggestionN`).
struct PendingSpell {
    field: CommitField,
    range: Range<usize>,
    word: String,
    suggestions: Vec<String>,
}

/// A co-author's token id: the lower-cased login, or the lower-cased email
/// of a `735-free-form-co-authors` author without one.
fn co_author_id(author: &Author) -> Option<String> {
    match author {
        Author::Known {
            username: None,
            email,
            ..
        } => Some(email.to_lowercase()),
        _ => author.username().map(str::to_lowercase),
    }
}

pub struct ChangesSidebar {
    filter: Entity<InputState>,
    summary: Entity<InputState>,
    description: Entity<TextareaState>,
    /// `AuthorInput`: authors are inline tokens (id = login, lower-cased).
    co_authors: Entity<TextareaState>,
    state: Entity<AppState>,
    seen_commit_nonce: u64,
    /// `737-commit-tag-field`: the optional tag field, and the tag to create
    /// once the repository's commit (nonce past the stored one) lands.
    tag: Entity<InputState>,
    pending_tag: Option<(u64, u64, String)>,
    /// Repository and newest commit last seen
    /// (`738-clear-message-after-outside-commit`).
    seen_head: (Option<u64>, Option<String>),
    seen_amend_nonce: u64,
    /// Repository and `commit_message_nonce` last seen: a new message from
    /// the dispatcher (Undo Commit) goes into the form.
    seen_commit_message: (Option<u64>, u64),
    /// Repository and `commit.template` text the form was last prefilled for.
    seen_template: (Option<u64>, Option<String>),
    context_menu: Option<Entity<ContextMenu>>,
    /// GHD `ChangesListFilterOptions` popover.
    filter_popover_open: bool,
    filter_button_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// Focus target for arrow-key navigation of the list.
    list_focus: FocusHandle,
    /// The include-all checkbox takes focus when pressed (it is outside the
    /// list, so the list's selection turns inactive, as in GHD).
    check_all_focus: FocusHandle,
    /// Keeps the row an arrow key moved to in view (`scrollRowToVisible`).
    list_scroll: UniformListScrollHandle,
    /// `1328-archive-entries-in-file-list`: the rows drawn, as of the last
    /// render (the list renders from `&self`).
    archive_splice: std::cell::RefCell<std::rc::Rc<crate::archive_list_rows::ArchiveSplice>>,
    /// Shift+F10 / Menu: the selected file's row.
    menu_anchor: crate::context_menu::RowMenuAnchor,
    /// View › Hide Changes Filter (`isChangesFilterVisible`).
    filter_visible: bool,
    /// GHD `AutocompletingTextInput` state for whichever field has the popup.
    autocomplete: Option<(CommitField, Autocompletion)>,
    /// Misspelled words per field (`NSSpellChecker`), refreshed on change.
    summary_misspelled: Vec<Misspelling>,
    description_misspelled: Vec<Misspelling>,
    /// Window-space rectangles of those words, written by the overlay's
    /// prepaint. The context-menu builder runs while the kit holds the input
    /// entity, so it must read these instead of the input.
    summary_rects: RectCache,
    description_rects: RectCache,
    summary_focus: FocusHandle,
    description_focus: FocusHandle,
    /// The commit options gear; inside the description box, so focusing it
    /// lights the box's `:focus-within` border like in GHD.
    commit_options_focus: FocusHandle,
    co_authors_focus: FocusHandle,
    /// The commit button: a click focuses it (`<button>`), and it keeps the
    /// `:focus` background while disabled during and after the commit.
    commit_button_focus: FocusHandle,
    pending_spell: Option<PendingSpell>,
    /// A handle typed with a trailing space, turned into a token on the next
    /// render (needs a window).
    pending_author: Option<(Range<usize>, Author)>,
    /// `isRuleFailurePopoverOpen`: the commit-message rule failures popover.
    rule_failure_popover_open: bool,
    /// GHD `CommitMessageAvatar`: the avatar button and its popovers.
    avatar: Entity<crate::commit_message_avatar::CommitMessageAvatar>,
    rule_hint_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// `731-recall-commit-messages`: index into the recent messages the form
    /// shows; `None` once the user edits it.
    recalled: Option<usize>,
    /// `769-co-author-validation`: why the last co-author was not added, and
    /// when (shown under the co-authors box for a few seconds).
    co_author_hint: Option<(SharedString, std::time::Instant)>,
    /// The summary's placeholder as last set (`getPlaceholderMessage`).
    summary_placeholder: SharedString,
    /// `766-persist-commit-drafts`: the repository the form's text belongs to.
    draft_repo: Option<u64>,
    /// `114-resizable-commit-message`: the drag of the handle over the
    /// commit form (pointer y and description height when it started) and
    /// the height it set, until it is saved on release.
    description_drag: Option<(Pixels, Pixels)>,
    dragged_description_height: Option<Pixels>,
    /// `783-amend-author`: the amended commit's author as `Name <email>`, and
    /// that text as loaded (an unchanged field keeps the commit's author).
    amend_author: Entity<InputState>,
    amend_author_original: String,
    /// `779-undoable-commit-message-replace`: the text last put in a field
    /// by [`Self::replace_message_field`]; its change event opens no
    /// autocompletion.
    programmatic_text: Option<(CommitField, String)>,
    /// The filtered list, rebuilt only when the status or a filter changes
    /// (every render and scroll frame reads it; 100,000 changed files are
    /// too many to filter and copy per frame).
    visible_cache: RefCell<Option<Rc<VisibleData>>>,
    /// The selected paths as a set for the rows, while the selection is large.
    selected_cache: RefCell<Option<Rc<SelectedPaths>>>,
    /// `716-windows-invalid-names-warning` for the current status.
    windows_names_cache: RefCell<Option<(std::sync::Weak<Status>, WindowsNames)>>,
    /// `797-stash-list`: the Stashes section is collapsed.
    stash_list_collapsed: bool,
    /// `1310-file-list-tree` / `1313-changelists`: the visible files as
    /// rows (a section per changelist, folder trees), rebuilt with them or
    /// when a folder or a group collapses.
    layout_cache: RefCell<Option<Rc<LayoutData>>>,
    /// `1310-file-list-tree`: the folder row (its section and path) whose
    /// files the user selected.
    tree_cursor: Option<(usize, String)>,
    /// `1313-changelists`: the group header row (its section) whose files
    /// the user selected.
    group_cursor: Option<usize>,
}

/// Included files Windows cannot check out: how many, and the first one
/// with the reason.
type WindowsNames = Option<(usize, String, &'static str)>;

type Status = corvene_core::WorkingDirectoryStatus;
type LineStatsMap = std::collections::HashMap<String, corvene_git::LineStats>;

/// What [`VisibleData`] was built from.
#[derive(Clone, PartialEq)]
struct VisibleKey {
    status: Option<usize>,
    text: String,
    /// View › Show Changes Filter: while hidden, neither the text nor the
    /// options filter (GHD `applyFilters`).
    show_filter: bool,
    filter: FileListFilter,
    hide: String,
    order: String,
    /// `703-changes-sort-order` "order-file": the `diff.orderFile`
    /// patterns (empty for any other order).
    order_file: Vec<String>,
    mode: String,
    /// `793-hide-whitespace-only-files`: the files left out (by address).
    whitespace_only: Option<usize>,
    /// `1313-changelists`: the repository's lists (by address).
    lists: Option<usize>,
}

/// The cached part of [`VisibleFiles`]. It keeps the status only weakly:
/// a strong reference would make every checkbox click copy all files
/// (`Arc::make_mut`), while a weak one still pins the pointer the key holds.
struct VisibleData {
    key: VisibleKey,
    /// Pins the allocation `key.status` points at (never read).
    _status: Option<std::sync::Weak<Status>>,
    /// Pins `key.lists` (never read).
    _lists: Option<std::sync::Arc<Changelists>>,
    /// In list order; grouped by changelist (`groups`) when there are lists.
    indices: Vec<usize>,
    /// `1313-changelists`: the groups `indices` fall in, in order, the
    /// files in no list last. `None` without lists.
    groups: Option<Vec<GroupInfo>>,
    /// `1313-changelists`: the name of the one list whose changed files are
    /// exactly what the next commit takes ("Commit <list> to main").
    single_list: Option<SharedString>,
    /// `getCheckAllValue` of the visible files (the active list's with one).
    include_all: Option<bool>,
    /// Visible files whose checkbox counts (every one, or the active list's).
    checkable: usize,
    /// Files included in the commit, and whether one of them is filtered out
    /// (GHD `isCommittingFileHiddenByFilter`).
    included: usize,
    included_hidden: bool,
    /// `793-hide-whitespace-only-files`: listed files left out.
    whitespace_hidden: usize,
    /// `changes-line-counts` totals for one line-stats map.
    line_totals: RefCell<Option<(std::sync::Weak<LineStatsMap>, corvene_git::LineStats)>>,
}

/// The files passing the text + option filters, in list order, as indices
/// into the shared status, with what the header derives from them. Lives
/// for one render: listeners must not keep it (see [`VisibleData`]).
pub(crate) struct VisibleFiles {
    status: Option<std::sync::Arc<Status>>,
    data: Rc<VisibleData>,
}

impl VisibleData {
    fn build(
        key: VisibleKey,
        status: Option<&std::sync::Arc<Status>>,
        whitespace_only: Option<&std::collections::HashSet<String>>,
        lists: Option<&std::sync::Arc<Changelists>>,
    ) -> Self {
        let files = status.map_or(&[][..], |s| &s.files[..]);
        // `706-changes-hide-globs`
        let hide = corvene_core::filter::hide_patterns(&key.hide);
        // `703-changes-sort-order`
        let order = corvene_core::filter::sorted_indices_with(files, &key.order, &key.order_file);
        // `704-changes-filter-match`
        let mut indices = corvene_core::filter::filtered_indices(
            files,
            order.as_deref(),
            &key.text,
            key.show_filter,
            &key.filter,
            &hide,
            &key.mode,
        );
        // `793-hide-whitespace-only-files`
        let mut whitespace_hidden = 0;
        if let Some(paths) = whitespace_only {
            let before = indices.len();
            indices.retain(|&i| !paths.contains(&files[i].path));
            whitespace_hidden = before - indices.len();
        }
        // `1313-changelists`: the visible files by group, the lists in
        // their order and the files in no list last
        let lists = lists.filter(|l| !l.is_empty());
        let mut groups = None;
        if let Some(lists) = lists {
            let mut shown = vec![false; files.len()];
            for &i in &indices {
                shown[i] = true;
            }
            let mut group_of = vec![corvene_core::changelists::UNGROUPED; files.len()];
            let mut totals: std::collections::HashMap<u64, usize> =
                std::collections::HashMap::new();
            for (i, f) in files.iter().enumerate() {
                group_of[i] = lists.group_of(&f.path);
                *totals.entry(group_of[i]).or_default() += 1;
            }
            let mut ordered = Vec::with_capacity(indices.len());
            let mut infos = Vec::with_capacity(lists.lists.len() + 1);
            let active = lists.active_list().map(|l| l.id);
            let mut push = |id: u64, name: SharedString, description: String| {
                let start = ordered.len();
                ordered.extend(indices.iter().copied().filter(|&i| group_of[i] == id));
                infos.push(GroupInfo {
                    id,
                    name,
                    description,
                    active: active == Some(id),
                    checkable: active.is_none_or(|a| a == id),
                    collapsed: lists.collapsed.contains(&id),
                    range: start..ordered.len(),
                    total: totals.get(&id).copied().unwrap_or(0),
                });
            };
            for list in &lists.lists {
                push(list.id, list.name.clone().into(), list.description.clone());
            }
            push(
                corvene_core::changelists::UNGROUPED,
                "Changes".into(),
                String::new(),
            );
            indices = ordered;
            groups = Some(infos);
        }
        let kind = |i: &usize| files[*i].selection.kind();
        // what the commit takes: every ticked file, or the active list's
        let is_included = |f: &WorkingDirectoryFileChange| {
            corvene_core::changelists::committed(lists.map(|l| &**l), f)
        };
        let checkable_index = |i: &usize| lists.is_none_or(|l| l.commits_path(&files[*i].path));
        // `getCheckAllValue`: the box reflects only the files passing the
        // filter (and in the active list, with one)
        let checkable_indices: Vec<usize> =
            indices.iter().copied().filter(checkable_index).collect();
        let include_all = if checkable_indices
            .iter()
            .all(|i| kind(i) == DiffSelectionType::All)
        {
            Some(true)
        } else if checkable_indices
            .iter()
            .all(|i| kind(i) == DiffSelectionType::None)
        {
            Some(false)
        } else {
            None
        };
        let included = files.iter().filter(|f| is_included(f)).count();
        let included_hidden = indices.len() != files.len() && {
            let mut shown = vec![false; files.len()];
            for &i in &indices {
                shown[i] = true;
            }
            files
                .iter()
                .zip(&shown)
                .any(|(f, shown)| !shown && is_included(f))
        };
        // `1313-changelists`: "Commit <list> to main" when the commit takes
        // exactly one list's changed files
        let single_list = lists.filter(|_| included > 0).and_then(|lists| {
            let committed: Vec<&str> = files
                .iter()
                .filter(|f| is_included(f))
                .map(|f| f.path.as_str())
                .collect();
            let list = lists
                .lists
                .iter()
                .find(|l| committed.iter().all(|p| l.paths.contains(*p)))?;
            let changed = files
                .iter()
                .filter(|f| list.paths.contains(&f.path))
                .count();
            (changed == committed.len()).then(|| SharedString::from(list.name.clone()))
        });
        Self {
            key,
            _status: status.map(std::sync::Arc::downgrade),
            _lists: lists.cloned(),
            indices,
            groups,
            single_list,
            include_all,
            checkable: checkable_indices.len(),
            included,
            included_hidden,
            whitespace_hidden,
            line_totals: RefCell::new(None),
        }
    }
}

impl VisibleFiles {
    fn files(&self) -> &[WorkingDirectoryFileChange] {
        self.status.as_deref().map_or(&[][..], |s| &s.files[..])
    }

    fn len(&self) -> usize {
        self.data.indices.len()
    }

    fn is_empty(&self) -> bool {
        self.data.indices.is_empty()
    }

    /// Every changed file, filtered or not.
    fn total(&self) -> usize {
        self.files().len()
    }

    /// `793-hide-whitespace-only-files`: files left out for whitespace-only
    /// changes.
    fn whitespace_hidden(&self) -> usize {
        self.data.whitespace_hidden
    }

    fn include_all(&self) -> Option<bool> {
        self.data.include_all
    }

    fn get(&self, ix: usize) -> Option<&WorkingDirectoryFileChange> {
        self.data.indices.get(ix).map(|&i| &self.files()[i])
    }

    fn iter(&self) -> impl Iterator<Item = &WorkingDirectoryFileChange> {
        let files = self.files();
        self.data.indices.iter().map(move |&i| &files[i])
    }

    fn paths(&self) -> Vec<String> {
        self.iter().map(|f| f.path.clone()).collect()
    }

    /// `1313-changelists`: whether visible file `ix` goes into the next
    /// commit when ticked (no active list, or in it).
    fn checkable(&self, ix: usize) -> bool {
        match (&self.data._lists, self.get(ix)) {
            (Some(lists), Some(file)) => lists.commits_path(&file.path),
            _ => true,
        }
    }

    /// The paths the header checkbox ticks: the visible files whose
    /// checkbox counts.
    fn checkable_paths(&self) -> Vec<String> {
        (0..self.len())
            .filter(|&ix| self.checkable(ix))
            .filter_map(|ix| self.get(ix))
            .map(|f| f.path.clone())
            .collect()
    }

    /// `1313-changelists`: the groups, when there are lists.
    fn groups(&self) -> Option<&[GroupInfo]> {
        self.data.groups.as_deref()
    }

    fn position(&self, path: &str) -> Option<usize> {
        self.iter().position(|f| f.path == path)
    }

    /// Lines added / deleted by the visible files.
    fn line_totals(&self, stats: &std::sync::Arc<LineStatsMap>) -> corvene_git::LineStats {
        if let Some((map, totals)) = &*self.data.line_totals.borrow()
            && std::ptr::eq(map.as_ptr(), std::sync::Arc::as_ptr(stats))
        {
            return *totals;
        }
        let totals = self.iter().filter_map(|f| stats.get(&f.path)).fold(
            corvene_git::LineStats::default(),
            |acc, s| corvene_git::LineStats {
                added: acc.added + s.added,
                deleted: acc.deleted + s.deleted,
            },
        );
        *self.data.line_totals.borrow_mut() = Some((std::sync::Arc::downgrade(stats), totals));
        totals
    }
}

/// `1313-changelists`: one group of the changes list: a changelist's
/// visible files, or (last) the files in no list.
struct GroupInfo {
    /// The changelist's id, `UNGROUPED` for files in none.
    id: u64,
    name: SharedString,
    description: String,
    active: bool,
    /// Whether its files' checkboxes count (no active list, or this is it).
    checkable: bool,
    /// Collapsed by the user (a filter text still expands it).
    collapsed: bool,
    /// Its visible files: `VisibleData::indices[range]`.
    range: Range<usize>,
    /// Its changed files, filtered or not.
    total: usize,
}

/// `1310-file-list-tree` / `1313-changelists`: the rows of the changes
/// list, built from the visible files: one section per group (with a header
/// row when there are changelists), its files flat or as a folder tree.
struct LayoutData {
    /// Built from these visible files (held, so the pointer stays theirs).
    visible: Rc<VisibleData>,
    /// The collapsed folders' set, by address (held in `_collapsed`).
    collapsed: Option<usize>,
    _collapsed: Option<std::sync::Arc<std::collections::BTreeSet<String>>>,
    expand_all: bool,
    tree_mode: bool,
    sections: Vec<Section>,
    rows: usize,
    /// Each row's checkbox: its file's, or `getCheckAllValue` of a folder's
    /// or a group's files.
    include: Vec<Option<bool>>,
}

/// One group's rows.
struct Section {
    /// Its group (`None`: no changelists, every file in this one section).
    group: Option<usize>,
    /// The header row.
    header: Option<usize>,
    /// The first row after the header, and how many follow (0 collapsed).
    first: usize,
    body: usize,
    /// Its files' visible indices start here (tree items are relative).
    offset: usize,
    /// How many visible files it has.
    len: usize,
    /// The group is collapsed (no body rows).
    collapsed: bool,
    /// Its files as a tree, in tree mode.
    tree: Option<corvene_core::file_tree::FileTree>,
}

/// What a row of the changes list is.
#[derive(Clone, Debug, PartialEq)]
enum RowRef {
    /// A group's header (the section).
    Header(usize),
    /// A file (its visible index), at `depth` in a tree.
    File {
        visible: usize,
        depth: Option<usize>,
    },
    /// A folder row of a section's tree (the tree's row).
    Folder { section: usize, row: usize },
}

impl LayoutData {
    /// Anything beyond a flat list of files: trees or group headers.
    fn structured(&self) -> bool {
        self.tree_mode || self.sections.iter().any(|s| s.header.is_some())
    }

    fn section_of_row(&self, ix: usize) -> Option<usize> {
        self.sections
            .iter()
            .rposition(|s| s.header.unwrap_or(s.first) <= ix)
    }

    fn section_of_visible(&self, visible: usize) -> Option<usize> {
        self.sections
            .iter()
            .rposition(|s| s.offset <= visible && visible < s.offset + s.len)
    }

    fn row(&self, ix: usize) -> Option<RowRef> {
        use corvene_core::file_tree::TreeRow;
        let section = self.section_of_row(ix)?;
        let s = &self.sections[section];
        if s.header == Some(ix) {
            return Some(RowRef::Header(section));
        }
        let local = ix.checked_sub(s.first)?;
        if local >= s.body {
            return None;
        }
        match &s.tree {
            Some(tree) => match tree.rows.get(local)? {
                TreeRow::File { item, depth } => Some(RowRef::File {
                    visible: s.offset + item,
                    depth: Some(*depth),
                }),
                TreeRow::Folder { .. } => Some(RowRef::Folder {
                    section,
                    row: local,
                }),
            },
            None => Some(RowRef::File {
                visible: s.offset + local,
                depth: None,
            }),
        }
    }

    /// The row showing visible file `visible` (`None` inside a collapsed
    /// folder or group).
    fn row_of_visible(&self, visible: usize) -> Option<usize> {
        let s = &self.sections[self.section_of_visible(visible)?];
        if s.body == 0 {
            return None;
        }
        match &s.tree {
            Some(tree) => tree.row_of_item(visible - s.offset).map(|r| s.first + r),
            None => Some(s.first + visible - s.offset),
        }
    }

    /// The visible files `ix` stands for: a header's group, a folder's
    /// files, or the file.
    fn row_items(&self, ix: usize) -> Vec<usize> {
        match self.row(ix) {
            Some(RowRef::Header(section)) => {
                let s = &self.sections[section];
                (s.offset..s.offset + s.len).collect()
            }
            Some(RowRef::Folder { section, row }) => {
                let s = &self.sections[section];
                s.tree
                    .as_ref()
                    .map(|t| t.row_items(row).iter().map(|&i| s.offset + i).collect())
                    .unwrap_or_default()
            }
            Some(RowRef::File { visible, .. }) => vec![visible],
            None => Vec::new(),
        }
    }

    /// Every visible file in row order, collapsed ones included (the order
    /// a ⇧-click range follows).
    fn order(&self) -> Vec<usize> {
        let mut order = Vec::with_capacity(self.visible.indices.len());
        for s in &self.sections {
            match &s.tree {
                Some(tree) => order.extend(tree.order.iter().map(|&i| s.offset + i)),
                None => order.extend(s.offset..s.offset + s.len),
            }
        }
        order
    }

    /// The files on shown rows, in row order (⇧↑ / ⇧↓ steps).
    fn visible_items(&self) -> Vec<usize> {
        let mut items = Vec::new();
        for s in self.sections.iter().filter(|s| s.body > 0) {
            match &s.tree {
                Some(tree) => items.extend(tree.visible_items().into_iter().map(|i| s.offset + i)),
                None => items.extend(s.offset..s.offset + s.len),
            }
        }
        items
    }

    /// The row of folder `folder` in section `section`.
    fn folder_row(&self, section: usize, folder: &str) -> Option<usize> {
        use corvene_core::file_tree::TreeRow;
        let s = self.sections.get(section)?;
        let local = s
            .tree
            .as_ref()?
            .rows
            .iter()
            .position(|r| matches!(r, TreeRow::Folder { path, .. } if path == folder))?;
        Some(s.first + local)
    }
}

/// A file's checkbox value.
fn include_value(file: &WorkingDirectoryFileChange) -> Option<bool> {
    match file.selection.kind() {
        DiffSelectionType::All => Some(true),
        DiffSelectionType::None => Some(false),
        DiffSelectionType::Partial => None,
    }
}

/// `selectedFileIDs` for the rows: a set once there are more than a few
/// (⌘A over 100,000 files would make every row search them all).
pub(crate) struct SelectedPaths {
    list: Vec<String>,
    set: Option<std::collections::HashSet<String>>,
}

impl SelectedPaths {
    fn new(list: Vec<String>) -> Self {
        let set = (list.len() > 16).then(|| list.iter().cloned().collect());
        Self { list, set }
    }

    fn contains(&self, path: &str) -> bool {
        match &self.set {
            Some(set) => set.contains(path),
            None => self.list.iter().any(|p| p == path),
        }
    }
}

/// GHD `RepoRulesetsForBranchLink`
/// (`ui/repository-rules/repo-rulesets-for-branch-link.tsx`): the rulesets
/// page for `branch`, `None` (the children without a link) when the
/// repository or the branch is missing.
pub fn repo_rulesets_for_branch_link(
    repository: Option<&corvene_core::GitHubRepository>,
    branch: Option<&str>,
) -> Option<String> {
    let (repository, branch) = (repository?, branch.filter(|b| !b.is_empty())?);
    Some(format!(
        "{}/rules/?ref={}",
        repository.html_url,
        corvene_core::integrations::encode_component(&format!("refs/heads/{branch}"))
    ))
}

/// GHD `RepoRulesetLink` (`ui/repository-rules/repo-ruleset-link.tsx`): the
/// page of the ruleset `ruleset_id`.
pub fn repo_ruleset_link(repository: &corvene_core::GitHubRepository, ruleset_id: u64) -> String {
    format!("{}/rules/{ruleset_id}", repository.html_url)
}

/// What the repository rules say about the commit being written
/// (`renderBranchProtectionsRepoRulesCommitWarning` inputs).
struct RulesSnapshot {
    github: corvene_core::GitHubRepository,
    branch: Option<String>,
    /// `aheadBehind === null`: the branch is unpublished.
    unpublished: bool,
    protected: bool,
    /// `339-protected-branch-bypass-note`: protected, but the user's pushes
    /// go through.
    protection_bypassed: bool,
    info: corvene_core::RepoRulesInfo,
    message_failures: RepoRulesMetadataFailures,
    /// `340-message-rules-defer-to-hooks`: a commit message hook runs on the
    /// commit and may make the message pass, so a failure does not block.
    message_hook: bool,
    author_failures: RepoRulesMetadataFailures,
    branch_failures: RepoRulesMetadataFailures,
}

impl ChangesSidebar {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        // Clear the form after a successful commit (GHD resets `commitMessage`).
        crate::windows::observe_state_in(&state, window, cx, |this, state, window, cx| {
            let nonce = state
                .read(cx)
                .selected_state()
                .map(|rs| rs.commit_nonce)
                .unwrap_or(0);
            // `737-commit-tag-field`: tag the commit that just landed
            if let Some((repo, before, _)) = &this.pending_tag {
                let landed = state
                    .read(cx)
                    .repo_states
                    .get(repo)
                    .filter(|rs| rs.commit_nonce > *before)
                    .and_then(|rs| rs.last_commit.as_ref())
                    .map(|c| c.sha.clone());
                if let Some(sha) = landed
                    && let Some((repo, _, name)) = this.pending_tag.take()
                {
                    Dispatcher::create_tag(repo, name, sha, String::new(), cx);
                }
            }
            // Corvene (`766-persist-commit-drafts`): each repository keeps its
            // own message; another repository's commits never clear it
            let selected = state.read(cx).selected;
            if selected != this.draft_repo
                && state
                    .read(cx)
                    .flags
                    .bool(corvene_core::flags::ids::PERSIST_COMMIT_DRAFTS)
            {
                let first = this.draft_repo.is_none();
                if let Some(previous) = std::mem::replace(&mut this.draft_repo, selected) {
                    this.save_draft(previous, cx);
                }
                let draft = selected.and_then(|id| state.read(cx).commit_drafts.get(&id).cloned());
                // the first repository keeps what the form has without a draft
                if draft.is_some() || !first {
                    let draft = draft.unwrap_or_default();
                    this.summary
                        .update(cx, |s, cx| s.set_value(draft.summary, window, cx));
                    this.description
                        .update(cx, |s, cx| s.set_value(draft.description, window, cx));
                    this.recalled = None;
                    this.refresh_spelling(CommitField::Summary, cx);
                    this.refresh_spelling(CommitField::Description, cx);
                }
                this.seen_commit_nonce = nonce;
            }
            if nonce != this.seen_commit_nonce {
                this.seen_commit_nonce = nonce;
                this.clear_form(window, cx);
            }
            // `738-clear-message-after-outside-commit`: a new HEAD commit made
            // elsewhere with the drafted summary clears the draft
            let head = {
                let s = state.read(cx);
                let rs = s.selected_state();
                let head = rs.and_then(|rs| rs.commits.first());
                (
                    s.selected,
                    head.map(|c| c.sha.clone()),
                    head.map(|c| c.summary.clone()),
                    s.flags
                        .bool(corvene_core::flags::ids::CLEAR_MESSAGE_AFTER_OUTSIDE_COMMIT),
                )
            };
            let (repo, sha, summary, clear_outside) = head;
            let previous = std::mem::replace(&mut this.seen_head, (repo, sha.clone()));
            if clear_outside
                && previous.0 == repo
                && previous.1.is_some()
                && previous.1 != sha
                && let Some(summary) = summary
            {
                let draft = this.summary.read(cx).value().trim().to_string();
                if !draft.is_empty() && draft == summary.trim() {
                    this.clear_form(window, cx);
                }
            }
            let template = {
                let s = state.read(cx);
                (
                    s.selected,
                    s.selected_state()
                        .and_then(|rs| rs.info.as_ref())
                        .and_then(|i| i.commit_template.clone())
                        // `201-commit-templates` off: as if there were none
                        .filter(|_| s.flags.bool(corvene_core::flags::ids::COMMIT_TEMPLATES)),
                )
            };
            if template != this.seen_template {
                let previous = std::mem::replace(&mut this.seen_template, template.clone()).1;
                this.apply_commit_template(previous, template.1, window, cx);
            }
            // GHD `prepareToAmendCommit`: load the commit's message into the form.
            let (amend_nonce, to_amend) = state
                .read(cx)
                .selected_state()
                .map(|rs| (rs.amend_nonce, rs.commit_to_amend.clone()))
                .unwrap_or((0, None));
            if amend_nonce != this.seen_amend_nonce {
                this.seen_amend_nonce = amend_nonce;
                if let Some(commit) = to_amend {
                    this.replace_message_field(
                        CommitField::Summary,
                        commit.summary.clone(),
                        window,
                        cx,
                    );
                    this.replace_message_field(
                        CommitField::Description,
                        commit.body.clone(),
                        window,
                        cx,
                    );
                    // `783-amend-author`
                    this.amend_author_original =
                        format!("{} <{}>", commit.author.name, commit.author.email);
                    let original = this.amend_author_original.clone();
                    this.amend_author
                        .update(cx, |s, cx| s.set_value(original, window, cx));
                    this.refresh_spelling(CommitField::Summary, cx);
                    this.refresh_spelling(CommitField::Description, cx);
                    cx.notify();
                }
            }
            // GHD `commitMessage` (after `undoCommit`): load it into the form
            let (repo, message_nonce, message) = {
                let s = state.read(cx);
                let rs = s.selected_state();
                (
                    s.selected,
                    rs.map_or(0, |rs| rs.commit_message_nonce),
                    rs.map(|rs| rs.commit_message.clone()),
                )
            };
            let seen = std::mem::replace(&mut this.seen_commit_message, (repo, message_nonce));
            if seen.0 == repo
                && seen.1 != message_nonce
                && let Some(message) = message
            {
                this.replace_message_field(CommitField::Summary, message.summary, window, cx);
                this.replace_message_field(
                    CommitField::Description,
                    message.description.unwrap_or_default(),
                    window,
                    cx,
                );
                this.refresh_spelling(CommitField::Summary, cx);
                this.refresh_spelling(CommitField::Description, cx);
                cx.notify();
            }
        })
        .detach();
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
        cx.observe(&filter, |this, _, cx| {
            this.select_first_visible_if_hidden(cx);
            cx.notify()
        })
        .detach();
        let summary = cx.new(|cx| InputState::new(window, cx).placeholder("Summary (required)"));
        let tag = cx.new(|cx| InputState::new(window, cx).placeholder("Tag (optional)"));
        let description = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(4)
                .placeholder("Description")
        });
        // `AuthorInput` wraps its tokens and grows with them
        let co_authors = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 6)
                .placeholder("@username")
        });
        cx.subscribe(&co_authors, |this, _, ev: &InputEvent, cx| {
            this.on_input_event(CommitField::CoAuthors, ev, cx)
        })
        .detach();
        let summary_focus = summary.read(cx).focus_handle(cx);
        let description_focus = description.read(cx).focus_handle(cx);
        let co_authors_focus = co_authors.read(cx).focus_handle(cx);
        let commit_button_focus = cx.focus_handle();
        {
            // one commit button per window (`429-multiple-windows`)
            let id = window.window_handle().window_id();
            let registry = cx.default_global::<CommitButtonFocus>();
            registry.0.retain(|(w, _)| *w != id);
            registry.0.push((id, commit_button_focus.clone()));
        }
        cx.subscribe(&summary, |this, _, ev: &InputEvent, cx| {
            this.on_input_event(CommitField::Summary, ev, cx)
        })
        .detach();
        // `733-summary-max-length`: like `maxlength`, the part of an edit
        // that goes past the limit is dropped
        cx.subscribe_in(
            &summary,
            window,
            |this, summary, ev: &InputEvent, window, cx| {
                if !matches!(ev, InputEvent::Change)
                    || !this
                        .state
                        .read(cx)
                        .flags
                        .bool(corvene_core::flags::ids::SUMMARY_MAX_LENGTH)
                {
                    return;
                }
                let (text, caret) = {
                    let s = summary.read(cx);
                    (s.value().to_string(), s.cursor())
                };
                if let Some(excess) = summary_overflow(&text, caret, SUMMARY_MAX_CHARS) {
                    summary.update(cx, |s, cx| {
                        s.set_selected_range(excess, cx);
                        s.replace("", window, cx);
                    });
                }
            },
        )
        .detach();
        cx.subscribe(&description, |this, _, ev: &InputEvent, cx| {
            this.on_input_event(CommitField::Description, ev, cx)
        })
        .detach();
        // `783-amend-author`
        let amend_author = cx.new(|cx| InputState::new(window, cx).placeholder("Name <email>"));
        cx.subscribe(&amend_author, |this, input, ev: &InputEvent, cx| {
            if !matches!(ev, InputEvent::Change) {
                return;
            }
            let Some(id) = this.state.read(cx).selected else {
                return;
            };
            let text = input.read(cx).value().to_string();
            let author = (text.trim() != this.amend_author_original)
                .then(|| corvene_git::parse_commit_author(&text).ok())
                .flatten()
                .map(|(name, email)| corvene_git::CommitAuthor::Given { name, email });
            Dispatcher::set_amend_author(id, author, cx);
        })
        .detach();
        let list_scroll = UniformListScrollHandle::new();
        let avatar = {
            let state = state.clone();
            cx.new(|cx| crate::commit_message_avatar::CommitMessageAvatar::new(state, window, cx))
        };
        Self {
            filter,
            summary,
            description,
            co_authors,
            state,
            seen_commit_nonce: 0,
            seen_head: (None, None),
            tag,
            pending_tag: None,
            seen_amend_nonce: 0,
            seen_commit_message: (None, 0),
            seen_template: (None, None),
            context_menu: None,
            filter_popover_open: false,
            filter_button_bounds: Rc::new(Cell::new(Bounds::default())),
            list_focus: cx.focus_handle().tab_stop(true),
            check_all_focus: cx.focus_handle(),
            menu_anchor: crate::context_menu::RowMenuAnchor::for_uniform_list(&list_scroll),
            list_scroll,
            archive_splice: Default::default(),
            filter_visible: true,
            autocomplete: None,
            summary_misspelled: Vec::new(),
            description_misspelled: Vec::new(),
            summary_rects: Rc::new(RefCell::new(Vec::new())),
            description_rects: Rc::new(RefCell::new(Vec::new())),
            summary_focus,
            description_focus,
            commit_options_focus: cx.focus_handle(),
            co_authors_focus,
            commit_button_focus,
            pending_spell: None,
            pending_author: None,
            rule_failure_popover_open: false,
            avatar,
            rule_hint_bounds: Rc::new(Cell::new(Bounds::default())),
            recalled: None,
            co_author_hint: None,
            summary_placeholder: "Summary (required)".into(),
            draft_repo: None,
            programmatic_text: None,
            amend_author,
            amend_author_original: String::new(),
            description_drag: None,
            dragged_description_height: None,
            visible_cache: RefCell::new(None),
            selected_cache: RefCell::new(None),
            windows_names_cache: RefCell::new(None),
            stash_list_collapsed: false,
            layout_cache: RefCell::new(None),
            tree_cursor: None,
            group_cursor: None,
        }
    }

    // ---- autocompletion + spellcheck (GHD `AutocompletingTextInput`) ----

    fn on_input_event(&mut self, field: CommitField, ev: &InputEvent, cx: &mut Context<Self>) {
        if matches!(ev, InputEvent::Change) && field != CommitField::CoAuthors {
            self.recalled = None;
            if let Some(id) = self.draft_repo
                && self.state.read(cx).selected == Some(id)
            {
                self.save_draft(id, cx);
            }
        }
        match ev {
            InputEvent::Change if field == CommitField::CoAuthors => {
                self.sync_co_authors(cx);
                self.open_autocomplete(field, cx);
            }
            InputEvent::Change => {
                self.refresh_spelling(field, cx);
                // `779-undoable-commit-message-replace`: a replaced message is
                // not typed text
                let programmatic = self.programmatic_text.take().filter(|(f, _)| *f == field);
                if programmatic
                    .is_none_or(|(_, text)| text != self.field_text_and_caret(field, cx).0)
                {
                    self.open_autocomplete(field, cx);
                }
            }
            InputEvent::Blur if self.autocomplete.as_ref().is_some_and(|(f, _)| *f == field) => {
                self.autocomplete = None;
                cx.notify();
            }
            _ => {}
        }
    }

    /// `766-persist-commit-drafts`: the form's message is repository `id`'s
    /// draft (none when untouched).
    fn save_draft(&self, id: u64, cx: &mut Context<Self>) {
        let draft = corvene_core::drafts::CommitDraft::normalized(
            &self.summary.read(cx).value(),
            &self.description.read(cx).value(),
            self.seen_template.1.as_deref(),
        );
        Dispatcher::set_commit_draft(id, draft, cx);
    }

    fn field_text_and_caret(&self, field: CommitField, cx: &App) -> (String, usize) {
        match field {
            CommitField::Summary => {
                let s = self.summary.read(cx);
                (s.value().to_string(), s.cursor())
            }
            CommitField::Description => {
                let s = self.description.read(cx);
                (s.value().to_string(), s.cursor())
            }
            CommitField::CoAuthors => {
                let s = self.co_authors.read(cx);
                (s.value().to_string(), s.cursor())
            }
        }
    }

    fn field_focus_handle(&self, field: CommitField) -> FocusHandle {
        match field {
            CommitField::Summary => self.summary_focus.clone(),
            CommitField::Description => self.description_focus.clone(),
            CommitField::CoAuthors => self.co_authors_focus.clone(),
        }
    }

    /// Replace a byte range of a field's text and put the caret after it.
    fn replace_range(
        &mut self,
        field: CommitField,
        range: Range<usize>,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match field {
            CommitField::Summary => self.summary.update(cx, |s, cx| {
                s.set_selected_range(range, cx);
                s.replace(text, window, cx);
            }),
            CommitField::Description => self.description.update(cx, |s, cx| {
                s.set_selected_range(range, cx);
                s.replace(text, window, cx);
            }),
            CommitField::CoAuthors => self.co_authors.update(cx, |s, cx| {
                s.set_selected_range(range, cx);
                s.replace(text, window, cx);
            }),
        }
        let handle = self.field_focus_handle(field);
        window.focus(&handle, cx);
        self.refresh_spelling(field, cx);
        cx.notify();
    }

    /// GHD `open`: re-run the providers against the text at the caret.
    fn open_autocomplete(&mut self, field: CommitField, cx: &mut Context<Self>) {
        let (text, caret) = self.field_text_and_caret(field, cx);
        let github = {
            let s = self.state.read(cx);
            s.selected
                .and_then(|id| s.repository(id))
                .and_then(|r| r.github.clone())
        };
        if field == CommitField::CoAuthors {
            // the free text after the last token, `@handle`, caret at the end
            self.autocomplete = None;
            let free_start = self.co_author_free_start(cx).min(text.len());
            let free = &text[free_start..];
            let trimmed = free.trim_start();
            if let (Some(gh), Some(rest)) = (github.as_ref(), trimmed.strip_prefix('@'))
                && caret == text.len()
            {
                let start = free_start + (free.len() - trimmed.len()) + 1;
                let exclude = self.co_author_logins(cx);
                let hits = autocompletion::co_author_hits(&rest.to_lowercase(), gh, &exclude, cx);
                if !hits.is_empty() {
                    self.autocomplete = Some((
                        field,
                        Autocompletion {
                            kind: corvene_core::TriggerKind::User,
                            range: start..text.len(),
                            hits,
                            selected: None,
                            scroll: UniformListScrollHandle::new(),
                        },
                    ));
                }
            } else if github.is_some()
                && caret == text.len()
                && !trimmed.is_empty()
                && !trimmed.starts_with('@')
                && !trimmed.contains('<')
            {
                // Corvene (`770-co-authors-from-history`): a typed name or
                // email suggests recent commit authors without the @
                let exclude = self.co_author_logins(cx);
                let hits = autocompletion::history_author_hits(
                    &trimmed.to_lowercase(),
                    &exclude,
                    &[],
                    corvene_core::DEFAULT_MAX_HITS,
                    cx,
                );
                if !hits.is_empty() {
                    // the insert replaces from one byte before `range` (the
                    // trigger character there is none, so start one later)
                    let start = free_start + (free.len() - trimmed.len()) + 1;
                    self.autocomplete = Some((
                        field,
                        Autocompletion {
                            kind: corvene_core::TriggerKind::User,
                            range: start..text.len(),
                            hits,
                            selected: None,
                            scroll: UniformListScrollHandle::new(),
                        },
                    ));
                }
            }
            cx.notify();
            return;
        }
        self.autocomplete =
            autocompletion::attempt(&text, caret, github.as_ref(), cx).map(|ac| (field, ac));
        cx.notify();
    }

    // ---- co-authors (GHD `AuthorInput`) ----

    /// Byte offset where the free text after the last author token starts.
    fn co_author_free_start(&self, cx: &App) -> usize {
        self.co_authors
            .read(cx)
            .tokens()
            .last()
            .map(|t| t.range().end)
            .unwrap_or(0)
    }

    fn co_author_logins(&self, cx: &App) -> Vec<String> {
        self.state
            .read(cx)
            .selected_state()
            .map(|rs| rs.co_authors.iter().filter_map(co_author_id).collect())
            .unwrap_or_default()
    }

    /// GHD `onAuthorsUpdated` after the tokens changed (backspace removed one),
    /// plus "Space at the end of the text adds the typed handle".
    fn sync_co_authors(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.state.read(cx).selected else {
            return;
        };
        let ids: Vec<String> = self
            .co_authors
            .read(cx)
            .tokens()
            .iter()
            .map(|t| t.token().id().to_string())
            .collect();
        let current = self
            .state
            .read(cx)
            .selected_state()
            .map(|rs| rs.co_authors.clone())
            .unwrap_or_default();
        let kept: Vec<Author> = ids
            .iter()
            .filter_map(|id| {
                current
                    .iter()
                    .find(|a| co_author_id(a).as_ref() == Some(id))
                    .cloned()
            })
            .collect();
        if kept != current {
            Dispatcher::set_co_authors(id, kept, cx);
        }
        // `onInputKeyDown`: Space at the end turns the typed handle into an author
        let (text, caret) = self.field_text_and_caret(CommitField::CoAuthors, cx);
        let free_start = self.co_author_free_start(cx).min(text.len());
        let free = &text[free_start..];
        // Corvene (`735-free-form-co-authors`): "Name <email>" becomes a
        // co-author without a GitHub account, and Space only turns a word
        // typed with @ into a handle, so a name can be typed with spaces
        let free_form = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::FREE_FORM_CO_AUTHORS);
        if free_form
            && caret == text.len()
            && free.trim_end().ends_with('>')
            && let Some((name, email)) = corvene_core::autocomplete::parse_co_author_address(free)
        {
            self.pending_author = Some((
                free_start..text.len(),
                Author::Known {
                    name,
                    email,
                    username: None,
                },
            ));
            cx.notify();
        } else if caret == text.len()
            && free.ends_with(' ')
            && (!free_form || free.trim_start().starts_with('@'))
        {
            let handle = free.trim().trim_start_matches('@').to_string();
            if !handle.is_empty() && !handle.contains(char::is_whitespace) {
                let author = Author::Unknown {
                    username: handle,
                    state: UnknownAuthorState::Searching,
                };
                self.pending_author = Some((free_start..text.len(), author));
                cx.notify();
            }
        }
    }

    /// Add an author as a token replacing `range` (GHD `onAutocompleteItemSelected`).
    fn add_co_author(
        &mut self,
        range: Range<usize>,
        author: Author,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.state.read(cx).selected else {
            return;
        };
        let Some(login) = co_author_id(&author) else {
            return;
        };
        let already = self
            .co_author_logins(cx)
            .iter()
            .any(|u| u.eq_ignore_ascii_case(&login));
        // Corvene (`769-co-author-validation`): neither the commit's own
        // author nor a second token for the same co-author; the typed text
        // goes and a hint says why
        if self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::CO_AUTHOR_VALIDATION)
        {
            let hint = if self.is_own_author(&author, cx) {
                Some("You are the author of this commit already".to_string())
            } else if already {
                Some(format!("{} is already a co-author", author.display_text()))
            } else {
                None
            };
            if let Some(hint) = hint {
                self.co_authors.update(cx, |s, cx| {
                    s.set_selected_range(range, cx);
                    s.replace("", window, cx);
                });
                self.show_co_author_hint(hint, cx);
                return;
            }
            self.co_author_hint = None;
        }
        let token = InlineToken::new(login.clone(), author.display_text())
            .with_label(author.display_text());
        let ok = self
            .co_authors
            .update(cx, |s, cx| {
                s.replace_range_with_token(range, token, window, cx)
            })
            .is_ok();
        if !ok || already {
            return;
        }
        let mut authors = self
            .state
            .read(cx)
            .selected_state()
            .map(|rs| rs.co_authors.clone())
            .unwrap_or_default();
        authors.push(author.clone());
        Dispatcher::set_co_authors(id, authors, cx);
        if let (Author::Unknown { username, .. }, Some(gh)) = (
            &author,
            self.state
                .read(cx)
                .repository(id)
                .and_then(|r| r.github.clone()),
        ) {
            Dispatcher::resolve_unknown_author(id, &gh, username.clone(), cx);
        }
        let handle = self.co_authors_focus.clone();
        window.focus(&handle, cx);
        cx.notify();
    }

    /// `769-co-author-validation`: the author is the one committing: their
    /// account (login or one of its emails) or `user.email`.
    fn is_own_author(&self, author: &Author, cx: &App) -> bool {
        let s = self.state.read(cx);
        let identity_email = s
            .selected_state()
            .and_then(|rs| rs.info.as_ref())
            .and_then(|i| i.identity.email.clone());
        let account = s.selected.and_then(|id| s.account_for_repository(id));
        let login = author.username();
        let email = match author {
            Author::Known { email, .. } => Some(email.as_str()),
            Author::Unknown { .. } => None,
        };
        let same_login =
            login.is_some_and(|l| account.is_some_and(|a| a.login.eq_ignore_ascii_case(l)));
        let same_email = email.is_some_and(|e| {
            identity_email
                .as_deref()
                .is_some_and(|i| i.eq_ignore_ascii_case(e))
                || account.is_some_and(|a| a.emails.iter().any(|m| m.eq_ignore_ascii_case(e)))
        });
        same_login || same_email
    }

    /// `769-co-author-validation`: show `hint` under the co-authors box for
    /// a few seconds.
    fn show_co_author_hint(&mut self, hint: String, cx: &mut Context<Self>) {
        let at = std::time::Instant::now();
        self.co_author_hint = Some((hint.into(), at));
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(CO_AUTHOR_HINT_DURATION)
                .await;
            this.update(cx, |this, cx| {
                if this.co_author_hint.as_ref().is_some_and(|(_, t)| *t == at) {
                    this.co_author_hint = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The "Add Co-Authors" / "Remove Co-Authors" toggle.
    fn toggle_co_authors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.state.read(cx).selected else {
            return;
        };
        let show = self
            .state
            .read(cx)
            .selected_state()
            .is_some_and(|rs| rs.show_co_authored_by);
        Dispatcher::set_show_co_authored_by(id, !show, cx);
        if !show {
            let handle = self.co_authors_focus.clone();
            window.focus(&handle, cx);
        }
    }

    /// Unknown handles still in the list (`onConfirmCommitWithUnknownCoAuthors`).
    fn unknown_co_authors(&self, cx: &App) -> Vec<String> {
        self.state
            .read(cx)
            .selected_state()
            .filter(|rs| rs.show_co_authored_by)
            .map(|rs| {
                rs.co_authors
                    .iter()
                    .filter_map(|a| match a {
                        Author::Unknown { username, .. } => Some(username.clone()),
                        Author::Known { .. } => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `.author-input-component`: the label, the author tokens and the
    /// `@username` box, attached under the description container.
    fn co_author_input(&self, window: &Window, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let authors: Rc<Vec<Author>> = Rc::new(
            self.state
                .read(cx)
                .selected_state()
                .map(|rs| rs.co_authors.clone())
                .unwrap_or_default(),
        );
        let focused = self.co_authors_focus.is_focused(window);
        let (tag_bg, tag_border, error_bg, error_border, error_text, sel_bg, sel_text, text) = (
            t.co_author_tag_background,
            t.co_author_tag_border,
            t.form_error_background,
            t.form_error_border,
            t.form_error_text,
            t.box_selected_active_background,
            t.box_selected_active_text,
            t.text,
        );
        div()
            .id("co-author-input")
            .flex()
            .flex_row()
            .items_start()
            .min_h(TEXT_FIELD_HEIGHT())
            .px(SPACING_HALF())
            .py(zpx(2.))
            .gap(zpx(2.))
            .border_1()
            .border_t_0()
            .rounded_b(BORDER_RADIUS())
            .bg(t.box_background)
            .border_color(if focused {
                t.focus
            } else {
                t.box_border_contrast
            })
            .cursor_text()
            .text_size(FONT_SIZE())
            .child(
                div()
                    .flex_none()
                    .h(TEXT_FIELD_HEIGHT() - zpx(6.))
                    .flex()
                    .items_center()
                    .text_color(t.text_secondary)
                    .child("Co-Authors "),
            )
            .child(
                div().flex_1().min_w(zpx(80.)).child(
                    Textarea::new(&self.co_authors)
                        .appearance(false)
                        .xsmall()
                        .token(move |ctx, _window, cx| {
                            // `.handle`: known / progress / error / focused
                            let id = ctx.token().id().to_string();
                            let author = authors
                                .iter()
                                .find(|a| co_author_id(a).as_deref() == Some(id.as_str()));
                            let unknown = match author {
                                Some(Author::Unknown { state, .. }) => Some(*state),
                                _ => None,
                            };
                            let (bg, border, fg) = if ctx.is_selected() {
                                (sel_bg, tag_border, sel_text)
                            } else {
                                match unknown {
                                    Some(UnknownAuthorState::Error) => {
                                        (error_bg, error_border, error_text)
                                    }
                                    Some(UnknownAuthorState::Searching) => {
                                        (gpui_kit::transparent_black(), tag_border, text)
                                    }
                                    None => (tag_bg, tag_border, text),
                                }
                            };
                            let title = match unknown {
                                Some(UnknownAuthorState::Error) => {
                                    Some(format!("Could not find user with username {}", id))
                                }
                                Some(UnknownAuthorState::Searching) => {
                                    Some(format!("Searching for @{id}"))
                                }
                                None => author.map(|a| a.full_text()),
                            };
                            let _ = cx;
                            div()
                                .id(SharedString::from(format!("co-author-{id}")))
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(zpx(3.))
                                .px(zpx(2.))
                                .mx(zpx(2.))
                                .rounded(BORDER_RADIUS())
                                .border_1()
                                .border_color(border)
                                .bg(bg)
                                .text_color(fg)
                                .cursor_pointer()
                                .when_some(title, |d, title| d.ghd_tooltip(title))
                                .child(ctx.token().label().clone())
                                .when(unknown == Some(UnknownAuthorState::Searching), |d| {
                                    d.child(spin(
                                        octicon(Octicon::SyncClockwise, fg).size(zpx(9.)),
                                        "co-author-searching",
                                    ))
                                })
                                .when(unknown == Some(UnknownAuthorState::Error), |d| {
                                    d.child(octicon(Octicon::Stop, fg).size(zpx(9.)))
                                })
                        }),
                ),
            )
            .into_any_element()
    }

    /// ↑/↓ while the popup is open; `false` lets the key reach the field.
    fn autocomplete_move(&mut self, delta: i64, cx: &mut Context<Self>) -> bool {
        match self.autocomplete.as_mut() {
            Some((_, ac)) => {
                ac.move_selection(delta);
                cx.notify();
                true
            }
            None => false,
        }
    }

    /// Enter / Tab: insert the highlighted item (GHD `insertCompletion`).
    fn autocomplete_accept(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match self.autocomplete.as_ref().and_then(|(_, ac)| ac.selected) {
            Some(ix) => {
                self.autocomplete_insert(ix, window, cx);
                true
            }
            None => false,
        }
    }

    fn autocomplete_insert(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((field, ac)) = self.autocomplete.take() else {
            return;
        };
        let Some(hit) = ac.hits.get(ix) else {
            return;
        };
        // The trigger character sits right before the filter text; GHD
        // appends its `completionSuffix` (a space).
        let range = ac.range.start.saturating_sub(1)..ac.range.end;
        if field == CommitField::CoAuthors {
            let endpoint = self
                .state
                .read(cx)
                .selected
                .and_then(|id| self.state.read(cx).repository(id).cloned())
                .and_then(|r| r.github.map(|gh| gh.endpoint))
                .unwrap_or_else(|| "https://api.github.com".to_string());
            let author = match hit {
                Hit::User(u) => Author::Known {
                    name: u.name.clone().unwrap_or_else(|| u.login.clone()),
                    email: u
                        .email
                        .clone()
                        .filter(|e| !e.is_empty())
                        .unwrap_or_else(|| legacy_stealth_email(&u.login, &endpoint)),
                    username: Some(u.login.clone()),
                },
                Hit::UnknownUser(name) => Author::Unknown {
                    username: name.clone(),
                    state: UnknownAuthorState::Searching,
                },
                // `770-co-authors-from-history`
                Hit::Author { name, email } => Author::Known {
                    name: name.clone(),
                    email: email.clone(),
                    username: None,
                },
                _ => return,
            };
            self.add_co_author(range, author, window, cx);
            return;
        }
        let text = format!("{} ", hit.completion_text());
        self.replace_range(field, range, text, window, cx);
    }

    fn refresh_spelling(&mut self, field: CommitField, cx: &mut Context<Self>) {
        let enabled = self.state.read(cx).settings.commit_spellcheck_enabled;
        let items = if enabled {
            let (text, _) = self.field_text_and_caret(field, cx);
            corvene_platform::spell::misspelled_ranges(&text)
                .into_iter()
                .map(|range| Misspelling {
                    word: text.get(range.clone()).unwrap_or("").to_string(),
                    range,
                })
                .collect()
        } else {
            Vec::new()
        };
        match field {
            CommitField::Summary => {
                self.summary_misspelled = items;
                self.summary_rects.borrow_mut().clear();
            }
            CommitField::Description => {
                self.description_misspelled = items;
                self.description_rects.borrow_mut().clear();
            }
            CommitField::CoAuthors => {}
        }
        cx.notify();
    }

    /// Window-space rectangle of a byte range in a field, scroll included.
    fn range_rect(
        &self,
        field: CommitField,
        range: &Range<usize>,
        cx: &App,
    ) -> Option<Bounds<Pixels>> {
        let (bounds, scroll) = match field {
            CommitField::Summary => {
                let s = self.summary.read(cx);
                (s.range_to_bounds(range)?, s.scroll_offset())
            }
            CommitField::Description => {
                let s = self.description.read(cx);
                (s.range_to_bounds(range)?, s.scroll_offset())
            }
            CommitField::CoAuthors => return None,
        };
        Some(Bounds {
            origin: point(bounds.origin.x + scroll.x, bounds.origin.y),
            size: bounds.size,
        })
    }

    /// The misspelled word under `position`, from the last painted
    /// rectangles (no input reads: see `summary_rects`).
    fn misspelled_at(
        &self,
        field: CommitField,
        position: Point<Pixels>,
    ) -> Option<(Range<usize>, String)> {
        let (items, rects) = match field {
            CommitField::Summary => (&self.summary_misspelled, self.summary_rects.borrow()),
            CommitField::Description => (
                &self.description_misspelled,
                self.description_rects.borrow(),
            ),
            CommitField::CoAuthors => return None,
        };
        items
            .iter()
            .zip(rects.iter())
            .find(|(_, rect)| rect.is_some_and(|rect| rect.contains(&position)))
            .map(|(m, _)| (m.range.clone(), m.word.clone()))
    }

    /// Red dotted underline beneath each misspelled word (Chromium's marker).
    fn spell_overlay(&self, field: CommitField, cx: &Context<Self>) -> Option<AnyElement> {
        let (ranges, rects_cell): (Vec<Range<usize>>, RectCache) = match field {
            CommitField::Summary => (
                self.summary_misspelled
                    .iter()
                    .map(|m| m.range.clone())
                    .collect(),
                self.summary_rects.clone(),
            ),
            CommitField::Description => (
                self.description_misspelled
                    .iter()
                    .map(|m| m.range.clone())
                    .collect(),
                self.description_rects.clone(),
            ),
            CommitField::CoAuthors => return None,
        };
        if ranges.is_empty() {
            return None;
        }
        let color = cx.ghd().error;
        let weak = cx.weak_entity();
        Some(
            canvas(
                move |_, _, cx| {
                    let rects: Vec<Option<Bounds<Pixels>>> = weak
                        .upgrade()
                        .map(|this| {
                            let this = this.read(cx);
                            ranges
                                .iter()
                                .map(|r| this.range_rect(field, r, cx))
                                .collect()
                        })
                        .unwrap_or_default();
                    *rects_cell.borrow_mut() = rects.clone();
                    rects
                },
                move |bounds, rects, window, _| {
                    window.with_content_mask(Some(ContentMask { bounds }), |window| {
                        for rect in rects.into_iter().flatten() {
                            let y = rect.bottom() - zpx(3.);
                            let mut x = rect.left();
                            while x < rect.right() {
                                window.paint_quad(fill(
                                    Bounds::new(point(x, y), size(zpx(2.), zpx(2.))),
                                    color,
                                ));
                                x += zpx(4.);
                            }
                        }
                    });
                },
            )
            .absolute()
            .inset_0()
            .into_any_element(),
        )
    }

    /// GHD `onAutocompletingInputContextMenu` + Chromium's spelling items:
    /// suggestions, Add to Dictionary, the edit menu, the spellcheck toggle.
    ///
    /// The kit invokes the builder while it holds the input entity, so nothing
    /// in here may read the `InputState` (that panics inside AppKit's event
    /// handler and aborts the process).
    fn input_menu(&self, field: CommitField, cx: &Context<Self>) -> InputMenuBuilder {
        let weak = cx.weak_entity();
        let focus = self.field_focus_handle(field);
        Rc::new(move |mut menu, window, cx| {
            let mut suggestions: Option<Vec<String>> = None;
            let mut enabled = true;
            let mut co_authors: Option<(&'static str, bool)> = None;
            // the menu's actions dispatch through the focused element
            window.focus(&focus, cx);
            let updated = weak.update(cx, |this, cx| {
                this.pending_spell = None;
                enabled = this.state.read(cx).settings.commit_spellcheck_enabled;
                // `getAddRemoveCoAuthorsMenuItem`
                let s = this.state.read(cx);
                if let Some(id) = s.selected
                    && s.repository(id).is_some_and(|r| r.github.is_some())
                {
                    let rs = s.selected_state();
                    let show = rs.is_some_and(|rs| rs.show_co_authored_by);
                    let committing = rs.is_some_and(|rs| rs.committing);
                    co_authors = Some((
                        if show {
                            mac_or("Remove Co-Authors", "Remove co-authors")
                        } else {
                            mac_or("Add Co-Authors", "Add co-authors")
                        },
                        !committing,
                    ));
                }
                let position = window.mouse_position();
                tracing::debug!(
                    ?position,
                    rects = ?match field {
                        CommitField::Summary => this.summary_rects.borrow().clone(),
                        CommitField::Description => this.description_rects.borrow().clone(),
                        CommitField::CoAuthors => Vec::new(),
                    },
                    "commit input context menu"
                );
                if let Some((range, word)) = this.misspelled_at(field, position) {
                    let guesses = corvene_platform::spell::guesses(&word);
                    this.pending_spell = Some(PendingSpell {
                        field,
                        range,
                        word,
                        suggestions: guesses.clone(),
                    });
                    suggestions = Some(guesses);
                }
            });
            if updated.is_err() {
                tracing::warn!("commit input context menu: sidebar entity unavailable");
            }
            if let Some((label, enabled)) = co_authors {
                menu = menu
                    .menu_with_disabled(label, !enabled, Box::new(ToggleCoAuthors))
                    .separator();
            }
            if let Some(guesses) = suggestions {
                if guesses.is_empty() {
                    menu = menu.menu_with_disabled(
                        "No Guesses Found",
                        true,
                        Box::new(SpellSuggestion0),
                    );
                }
                for (ix, guess) in guesses.into_iter().enumerate() {
                    let action: Box<dyn Action> = match ix {
                        0 => Box::new(SpellSuggestion0),
                        1 => Box::new(SpellSuggestion1),
                        2 => Box::new(SpellSuggestion2),
                        3 => Box::new(SpellSuggestion3),
                        _ => Box::new(SpellSuggestion4),
                    };
                    menu = menu.menu(guess, action);
                }
                menu = menu
                    .menu("Add to Dictionary", Box::new(SpellAddToDictionary))
                    .separator();
            }
            menu.menu("Undo", Box::new(Undo))
                .menu("Redo", Box::new(Redo))
                .separator()
                .menu("Cut", Box::new(Cut))
                .menu("Copy", Box::new(Copy))
                .menu("Paste", Box::new(Paste))
                .menu("Select All", Box::new(SelectAll))
                .separator()
                .menu(
                    if enabled {
                        mac_or("Disable Commit Spellcheck", "Disable commit spellcheck")
                    } else {
                        mac_or("Enable Commit Spellcheck", "Enable commit spellcheck")
                    },
                    Box::new(ToggleCommitSpellcheck),
                )
        })
    }

    fn apply_spell_suggestion(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_spell.take() else {
            return;
        };
        let Some(word) = pending.suggestions.get(ix).cloned() else {
            return;
        };
        self.replace_range(pending.field, pending.range, word, window, cx);
    }

    fn add_to_dictionary(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_spell.take() else {
            return;
        };
        corvene_platform::spell::learn_word(&pending.word);
        self.refresh_spelling(CommitField::Summary, cx);
        self.refresh_spelling(CommitField::Description, cx);
    }

    fn toggle_spellcheck(&mut self, cx: &mut Context<Self>) {
        Dispatcher::update_settings(cx, |s| {
            s.commit_spellcheck_enabled = !s.commit_spellcheck_enabled
        });
        self.refresh_spelling(CommitField::Summary, cx);
        self.refresh_spelling(CommitField::Description, cx);
    }

    /// `731-recall-commit-messages`: ↑ (`delta` 1, older) / ↓ (-1, newer) in
    /// the summary field, shell-history style. Starts only from an untouched
    /// form (summary empty, description empty or the commit template); ↓ past
    /// the newest message restores the untouched form. Returns whether the
    /// key was used.
    fn recall_message(
        &mut self,
        delta: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let messages = {
            let s = self.state.read(cx);
            if !s
                .flags
                .bool(corvene_core::flags::ids::RECALL_COMMIT_MESSAGES)
            {
                return false;
            }
            let Some(rs) = s.selected_state() else {
                return false;
            };
            if rs.commit_to_amend.is_some() {
                return false;
            }
            corvene_core::recent_commit_messages(&rs.commits, 50)
        };
        let template = self.seen_template.1.clone().unwrap_or_default();
        let next = match self.recalled {
            Some(ix) => ix as isize + delta,
            None => {
                let untouched = self.summary.read(cx).value().is_empty() && {
                    let d = self.description.read(cx).value();
                    d.is_empty() || AsRef::<str>::as_ref(&d) == template
                };
                if delta < 0 || !untouched {
                    return false;
                }
                0
            }
        };
        let (summary, description) = if next < 0 {
            self.recalled = None;
            (String::new(), template)
        } else {
            let Some(message) = messages.get(next as usize) else {
                // past the oldest: stay, but keep the key
                return self.recalled.is_some();
            };
            self.recalled = Some(next as usize);
            message.clone()
        };
        self.summary
            .update(cx, |s, cx| s.set_value(summary, window, cx));
        self.description
            .update(cx, |s, cx| s.set_value(description, window, cx));
        self.refresh_spelling(CommitField::Summary, cx);
        self.refresh_spelling(CommitField::Description, cx);
        cx.notify();
        true
    }

    /// View › Go to Summary.
    /// Prefill the description with the repository's `commit.template`
    /// (`corvene_git::commit_template`) while the form is untouched: summary
    /// empty and the description empty or still holding the `previous`
    /// template text.
    /// Empty the commit form (GHD resets `commitMessage` after a commit); the
    /// next commit starts from the template again.
    fn clear_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_message_field(CommitField::Summary, String::new(), window, cx);
        self.replace_message_field(CommitField::Description, String::new(), window, cx);
        self.co_authors
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.tag.update(cx, |s, cx| s.set_value("", window, cx));
        self.summary_misspelled.clear();
        self.description_misspelled.clear();
        self.autocomplete = None;
        self.recalled = None;
        let template = self.seen_template.1.clone();
        self.apply_commit_template(None, template, window, cx);
        cx.notify();
    }

    fn apply_commit_template(
        &mut self,
        previous: Option<String>,
        template: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.summary.read(cx).value().is_empty() {
            return;
        }
        let description = self.description.read(cx).value().to_string();
        let untouched = description.is_empty() || previous.as_deref() == Some(description.as_str());
        if !untouched {
            return;
        }
        let text = template.unwrap_or_default();
        if text == description {
            return;
        }
        self.replace_message_field(CommitField::Description, text, window, cx);
        self.refresh_spelling(CommitField::Description, cx);
        cx.notify();
    }

    /// Put `text` in the summary or description, as the amend, Undo Commit,
    /// the commit template and the cleared form after a commit do. GHD sets
    /// the field's value, which drops its undo history, as `set_value` does;
    /// `779-undoable-commit-message-replace` makes it an edit ⌘Z takes back.
    fn replace_message_field(
        &mut self,
        field: CommitField,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let undoable = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::UNDOABLE_COMMIT_MESSAGE_REPLACE);
        if undoable {
            if self.field_text_and_caret(field, cx).0 == text {
                return;
            }
            self.programmatic_text = Some((field, text.clone()));
        }
        match field {
            CommitField::Summary => self.summary.update(cx, |s, cx| {
                if undoable {
                    s.replace_all(text, window, cx)
                } else {
                    s.set_value(text, window, cx)
                }
            }),
            CommitField::Description => self.description.update(cx, |s, cx| {
                if undoable {
                    s.replace_all(text, window, cx)
                } else {
                    s.set_value(text, window, cx)
                }
            }),
            CommitField::CoAuthors => self
                .co_authors
                .update(cx, |s, cx| s.set_value(text, window, cx)),
        }
    }

    pub fn focus_summary(&self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.summary.read(cx).focus_handle(cx);
        handle.focus(window, cx);
        cx.notify();
    }

    /// The summary or description field when it has focus
    /// (`617-section-switch-restores-commit-focus`).
    pub fn focused_commit_field(&self, window: &Window) -> Option<FocusHandle> {
        [&self.summary_focus, &self.description_focus]
            .into_iter()
            .find(|h| h.is_focused(window))
            .cloned()
    }

    /// Corvene (`603-focus-list-on-section-switch`).
    pub fn list_focus_handle(&self) -> FocusHandle {
        self.list_focus.clone()
    }

    /// Edit › Find.
    pub fn focus_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter_visible = true;
        let handle = self.filter.read(cx).focus_handle(cx);
        handle.focus(window, cx);
        cx.notify();
    }

    /// View › Show / Hide Changes Filter.
    pub fn toggle_filter(&mut self, cx: &mut Context<Self>) {
        self.filter_visible = !self.filter_visible;
        cx.notify();
    }

    /// Arrow keys move the selection through the visible files.
    pub(crate) fn select_relative(&mut self, delta: isize, cx: &mut Context<Self>) {
        let files = self.visible(cx);
        if files.is_empty() {
            return;
        }
        // `1310-file-list-tree` / `1313-changelists`: through the rows,
        // folders and group headers included
        let layout = self.layout(cx);
        if layout.structured() {
            let count = layout.rows;
            if count == 0 {
                return;
            }
            let stop = self
                .state
                .read(cx)
                .flags
                .bool(corvene_core::flags::ids::LISTS_STOP_AT_ENDS);
            let row = match self.current_row(&layout, cx) {
                // Home / End
                _ if delta.unsigned_abs() >= count.max(2) => {
                    if delta < 0 {
                        0
                    } else {
                        count - 1
                    }
                }
                Some(row) => crate::filter_list::list_step(row, delta, count, stop),
                None => 0,
            };
            self.select_row(row, cx);
            return;
        }
        let (id, current) = {
            let s = self.state.read(cx);
            let Some(id) = s.selected else { return };
            // GHD `moveSelection` starts from the last selected row: the
            // moving end of a ⇧-arrow range
            (
                id,
                s.selected_state().and_then(|rs| {
                    rs.selected_files
                        .last()
                        .or(rs.selected_file.as_ref())
                        .cloned()
                }),
            )
        };
        // GHD `List.moveSelection` wraps around the ends (Corvene
        // `620-lists-stop-at-ends`: stops there)
        let stop = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::LISTS_STOP_AT_ENDS);
        let index = current
            .and_then(|p| files.position(&p))
            .map(|i| crate::filter_list::list_step(i, delta, files.len(), stop))
            .unwrap_or(0);
        let Some(file) = files.get(index) else {
            return;
        };
        Dispatcher::select_file(id, file.path.clone(), cx);
        self.list_scroll.scroll_to_item(
            self.archive_splice.borrow().display_of(index),
            ScrollStrategy::Nearest,
        );
    }

    /// The repository and the highlighted files (the selection, else the
    /// selected file), for keyboard actions on the list.
    fn highlighted_files(&self, cx: &App) -> Option<(u64, Vec<String>)> {
        let s = self.state.read(cx);
        let id = s.selected?;
        let rs = s.selected_state()?;
        let mut paths = rs.selected_files.clone();
        if paths.is_empty()
            && let Some(one) = rs.selected_file.clone()
        {
            paths.push(one);
        }
        (!paths.is_empty()).then_some((id, paths))
    }

    /// Corvene (`605-cmd-backspace-discards-files`): ⌘⌫ discards the
    /// highlighted files, confirming as the context menu's Discard Changes
    /// does (GHD binds ⌘⌫ to Repository › Remove everywhere).
    fn discard_highlighted(&mut self, cx: &mut Context<Self>) {
        let committing = self
            .state
            .read(cx)
            .selected_state()
            .is_some_and(|rs| rs.committing);
        if committing {
            return;
        }
        if let Some((id, paths)) = self.highlighted_files(cx) {
            Dispatcher::request_discard_changes(id, paths, cx);
        }
    }

    /// Corvene (`606-open-file-shortcuts`): the first highlighted file on
    /// disk (a deleted file has nothing to open, as in the context menu).
    fn highlighted_file_on_disk(&self, cx: &App) -> Option<PathBuf> {
        let (id, paths) = self.highlighted_files(cx)?;
        let s = self.state.read(cx);
        let status = s.repo_states.get(&id)?.status.as_deref()?;
        let first = paths.first()?;
        let file = status.files.iter().find(|f| &f.path == first)?;
        (file.status.kind != FileStatusKind::Deleted)
            .then(|| s.repository(id).map(|r| r.path.join(first)))
            .flatten()
    }

    /// Corvene (`611-copy-path-shortcuts`): the context menu's Copy Paths /
    /// Copy Relative Paths for the highlighted files, one per line.
    fn copy_highlighted_paths(&self, absolute: bool, cx: &mut Context<Self>) {
        let Some((id, paths)) = self.highlighted_files(cx) else {
            return;
        };
        let text = match self.state.read(cx).repository(id) {
            Some(repo) if absolute => paths
                .iter()
                .map(|p| repo.path.join(p).to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("\n"),
            Some(_) => paths.join("\n"),
            None => return,
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    /// Space (GHD `onToggleInclude` for the row's `onKeyDown`): include the
    /// highlighted files, or exclude them when they are all included.
    fn toggle_include_selected(&mut self, cx: &mut Context<Self>) {
        let (id, paths, all_included) = {
            let s = self.state.read(cx);
            let Some(id) = s.selected else { return };
            let Some(rs) = s.selected_state() else { return };
            let mut paths = rs.selected_files.clone();
            if paths.is_empty()
                && let Some(one) = rs.selected_file.clone()
            {
                paths.push(one);
            }
            let Some(status) = rs.status.as_deref() else {
                return;
            };
            // `1313-changelists`: files outside the active list stay as they are
            if let Some(lists) = corvene_core::changelists::of(s, id) {
                paths.retain(|p| lists.commits_path(p));
            }
            let wanted: std::collections::HashSet<&str> =
                paths.iter().map(String::as_str).collect();
            let all_included = status
                .files
                .iter()
                .filter(|f| wanted.contains(f.path.as_str()))
                .all(|f| f.selection.kind() == corvene_core::DiffSelectionType::All);
            (id, paths, all_included)
        };
        if paths.is_empty() {
            return;
        }
        Dispatcher::set_files_included(id, paths, !all_included, cx);
    }

    /// ⇧↑ / ⇧↓: extend the range selection (GHD `List.addSelection`).
    fn extend_relative(&mut self, delta: isize, cx: &mut Context<Self>) {
        // `1310-file-list-tree` / `1313-changelists`: the file rows shown
        let layout = self.layout(cx);
        let visible = self.visible(cx);
        let order: Vec<String> = if layout.structured() {
            layout
                .visible_items()
                .into_iter()
                .filter_map(|i| visible.get(i))
                .map(|f| f.path.clone())
                .collect()
        } else {
            visible.paths()
        };
        self.tree_cursor = None;
        self.group_cursor = None;
        let Some(id) = self.state.read(cx).selected else {
            return;
        };
        Dispatcher::extend_file_selection_by(id, delta, order.clone(), cx);
        let end = self
            .state
            .read(cx)
            .selected_state()
            .and_then(|rs| rs.selected_files.last().cloned());
        let index = if layout.structured() {
            end.and_then(|p| visible.position(&p))
                .and_then(|i| layout.row_of_visible(i))
        } else {
            end.and_then(|p| order.iter().position(|o| *o == p))
        };
        if let Some(index) = index {
            self.list_scroll.scroll_to_item(
                self.archive_splice.borrow().display_of(index),
                ScrollStrategy::Nearest,
            );
        }
    }

    /// Commit form gear: GHD's native checkbox menu (`onCommitOptionsButtonClick`).
    fn open_commit_options_menu(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (id, options, push_option, amend_option) = {
            let s = self.state.read(cx);
            let Some(id) = s.selected else { return };
            // `780-amend-from-commit-options`: (amending, the HEAD commit
            // to amend when there is one)
            let amend_option = s
                .flags
                .bool(corvene_core::flags::ids::AMEND_FROM_COMMIT_OPTIONS)
                .then(|| {
                    let rs = s.selected_state();
                    let head = rs
                        .filter(|rs| {
                            !matches!(rs.info.as_ref().map(|i| &i.tip), Some(Tip::Unborn { .. }))
                        })
                        .and_then(|rs| rs.commits.first())
                        .map(|c| c.sha.clone());
                    (rs.is_some_and(|rs| rs.commit_to_amend.is_some()), head)
                });
            (
                id,
                s.repository(id)
                    .map(|r| r.commit_options)
                    .unwrap_or_default(),
                s.flags.bool(corvene_core::flags::ids::COMMIT_AND_PUSH),
                amend_option,
            )
        };
        let mut items = vec![
            MenuItem::checkbox(
                mac_or("Bypass Commit Hooks", "Bypass Commit hooks"),
                options.skip_commit_hooks,
                move |_, cx| {
                    Dispatcher::update_commit_options(
                        id,
                        |o| o.skip_commit_hooks = !o.skip_commit_hooks,
                        cx,
                    )
                },
            ),
            MenuItem::checkbox(
                mac_or("Add Signed-off-by Trailer", "Add Signed-off-by trailer"),
                options.sign_off_commits,
                move |_, cx| {
                    Dispatcher::update_commit_options(
                        id,
                        |o| o.sign_off_commits = !o.sign_off_commits,
                        cx,
                    )
                },
            ),
            MenuItem::checkbox(
                mac_or("Allow Empty Commit", "Allow empty commit"),
                options.allow_empty_commit,
                move |_, cx| {
                    Dispatcher::update_commit_options(
                        id,
                        |o| o.allow_empty_commit = !o.allow_empty_commit,
                        cx,
                    )
                },
            ),
        ];
        // Corvene: `736-commit-and-push`
        if push_option {
            items.push(MenuItem::checkbox(
                mac_or("Push After Committing", "Push after committing"),
                options.push_after_commit,
                move |_, cx| {
                    Dispatcher::update_commit_options(
                        id,
                        |o| o.push_after_commit = !o.push_after_commit,
                        cx,
                    )
                },
            ));
        }
        // Corvene: `787-commit-to-new-branch`, the changes committed on a new
        // branch that is then published with a pull request
        let new_branch = {
            let s = self.state.read(cx);
            s.flags
                .bool(corvene_core::flags::ids::COMMIT_TO_NEW_BRANCH)
                .then(|| {
                    s.selected_state()
                        .is_some_and(|rs| rs.commit_to_amend.is_none() && rs.mco.is_none())
                        && !self.commit_disabled(cx)
                })
        };
        if let Some(enabled) = new_branch {
            let summary = self.summary_or_placeholder(cx);
            let description = self.description.read(cx).value().to_string();
            items.push(MenuItem::separator());
            items.push(
                MenuItem::new(
                    mac_or("Commit to New Branch…", "Commit to new branch…"),
                    move |_, cx| {
                        Dispatcher::show_popup(
                            Popup::CommitToNewBranch {
                                repo: id,
                                summary: summary.clone(),
                                description: description.clone(),
                            },
                            cx,
                        )
                    },
                )
                .enabled(enabled),
            );
        }
        // Corvene: `780-amend-from-commit-options`, History's Amend Commit…
        // (the same warnings) as a toggle
        if let Some((amending, head)) = amend_option {
            let enabled = amending || head.is_some();
            items.push(MenuItem::separator());
            items.push(
                MenuItem::checkbox(
                    mac_or("Amend Last Commit", "Amend last commit"),
                    amending,
                    move |_, cx| match (amending, head.clone()) {
                        (true, _) => Dispatcher::stop_amending(id, cx),
                        (false, Some(sha)) => Dispatcher::request_start_amending(id, sha, cx),
                        (false, None) => {}
                    },
                )
                .enabled(enabled),
            );
        }
        // Corvene: `799-fixup-commits`, Fixup Into ▸ an unpushed commit and
        // Squash Fixup Commits
        let included = self.visible(cx).data.included;
        let fixups = {
            let s = self.state.read(cx);
            s.flags
                .bool(corvene_core::flags::ids::FIXUP_COMMITS)
                .then(|| {
                    let rs = s.selected_state()?;
                    let targets: Vec<(String, String)> = corvene_core::mco::fixup_targets(rs)
                        .into_iter()
                        .map(|c| (c.sha.clone(), c.summary.clone()))
                        .collect();
                    let idle = rs.commit_to_amend.is_none() && rs.mco.is_none() && !rs.committing;
                    let pending = !corvene_core::mco::pending_fixups(rs).is_empty();
                    Some((targets, idle, pending))
                })
                .flatten()
        };
        if let Some((targets, idle, pending)) = fixups {
            let can_commit = idle && included > 0;
            let fixup_items: Vec<MenuItem> = targets
                .into_iter()
                .map(|(sha, summary)| {
                    let short: String = sha.chars().take(7).collect();
                    MenuItem::new(format!("{short} {summary}"), move |_, cx| {
                        Dispatcher::commit_fixup(id, sha.clone(), cx)
                    })
                })
                .collect();
            let has_targets = !fixup_items.is_empty();
            items.push(MenuItem::separator());
            items.push(
                MenuItem::submenu(mac_or("Fixup Into", "Fixup into"), fixup_items)
                    .enabled(can_commit && has_targets),
            );
            items.push(
                MenuItem::new(
                    mac_or("Squash Fixup Commits", "Squash fixup commits"),
                    move |_, cx| Dispatcher::autosquash(id, false, cx),
                )
                .enabled(idle && pending),
            );
        }
        self.open_menu(items, position, window, cx);
    }

    /// Corvene (`1301-conventional-commit-types`): the type menu before the
    /// summary; a pick rewrites the summary's `type: ` prefix.
    fn open_conventional_type_menu(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use corvene_core::commit_message::{
            CONVENTIONAL_TYPES, conventional_type, with_conventional_type,
        };
        let summary = self.summary.read(cx).value().to_string();
        let current = conventional_type(&summary);
        let weak = cx.weak_entity();
        let pick = move |ty: Option<&'static str>| {
            let weak = weak.clone();
            move |window: &mut Window, cx: &mut App| {
                weak.update(cx, |this, cx| {
                    let summary = this.summary.read(cx).value().to_string();
                    let text = with_conventional_type(&summary, ty);
                    this.replace_message_field(CommitField::Summary, text, window, cx);
                    this.focus_summary(window, cx);
                })
                .ok();
            }
        };
        let mut items = vec![
            MenuItem::checkbox("None", current.is_none(), pick(None)),
            MenuItem::separator(),
        ];
        items.extend(CONVENTIONAL_TYPES.iter().map(|(ty, what)| {
            MenuItem::checkbox(
                format!("{ty}: {what}"),
                current == Some(*ty),
                pick(Some(*ty)),
            )
        }));
        self.open_menu(items, position, window, cx);
    }

    /// Corvene (`1301-conventional-commit-types`): the button before the
    /// summary showing its type.
    fn conventional_type_button(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let t = cx.ghd();
        if !self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::CONVENTIONAL_COMMIT_TYPES)
        {
            return None;
        }
        let summary = self.summary.read(cx).value().to_string();
        let current = corvene_core::commit_message::conventional_type(&summary);
        let (label, color) = match current {
            Some(ty) => (ty, t.text),
            None => ("type", t.text_secondary),
        };
        Some(
            div()
                .id("conventional-type-button")
                .flex_none()
                .h(TEXT_FIELD_HEIGHT())
                .px(zpx(6.))
                .flex()
                .flex_row()
                .items_center()
                .gap(zpx(2.))
                .border_1()
                .border_color(t.box_border_contrast)
                .rounded(BORDER_RADIUS())
                .bg(t.box_background)
                .cursor_pointer()
                .text_size(FONT_SIZE())
                .text_color(color)
                .hover(|s| s.bg(t.box_hover_background))
                .a11y_button("Commit type")
                .ghd_tooltip("Commit type")
                .on_click(cx.listener(|this, ev: &ClickEvent, window, cx| {
                    this.open_conventional_type_menu(ev.position(), window, cx)
                }))
                .child(label)
                .child(octicon(Octicon::TriangleDown, t.text_secondary))
                .into_any_element(),
        )
    }

    /// GHD `renderSummaryLengthHint`: a light bulb at the end of the summary
    /// past [`IDEAL_SUMMARY_LENGTH`](corvene_core::commit_message::IDEAL_SUMMARY_LENGTH)
    /// characters, with Settings › Prompts › "Show commit length warning"
    /// on (the repository rule hint takes its place when there is one).
    fn summary_length_hint(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let t = cx.ghd();
        if !self.state.read(cx).settings.show_commit_length_warning {
            return None;
        }
        let summary = self.summary.read(cx).value().to_string();
        if summary.chars().count() <= corvene_core::commit_message::IDEAL_SUMMARY_LENGTH {
            return None;
        }
        let hint = div()
            .id("length-hint")
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .a11y_button("Open Summary Length Info")
            .child(octicon(Octicon::LightBulb, t.text).size(zpx(12.)));
        // the tooltip wrapper positions its element relatively: place a box
        Some(
            div()
                .absolute()
                .top(zpx(2.))
                .right(zpx(2.))
                .w(zpx(16.))
                .h(TEXT_FIELD_HEIGHT() - zpx(4.))
                .child(crate::widgets::with_titled_tooltip(
                    hint,
                    "Great commit summaries contain fewer than 50 characters",
                    "Place extra information in the description field.",
                    crate::widgets::TooltipDirection::North,
                    std::time::Duration::ZERO,
                ))
                .into_any_element(),
        )
    }

    /// Files that pass the text + option filters (cached until the status
    /// or a filter changes).
    fn visible(&self, cx: &App) -> Rc<VisibleFiles> {
        let s = self.state.read(cx);
        let rs = s.selected_state();
        let status = rs.and_then(|rs| rs.status.clone());
        // `793-hide-whitespace-only-files` (set only while it applies)
        let whitespace_only = rs.and_then(|rs| rs.whitespace_only_files.clone());
        // `1313-changelists`
        let lists = s
            .selected
            .and_then(|id| corvene_core::changelists::of(s, id));
        let order = s.flags.text(corvene_core::flags::ids::CHANGES_SORT_ORDER);
        // `diff.orderFile` (path order when unset)
        let order_file = match order {
            "order-file" => rs
                .and_then(|rs| rs.info.as_ref())
                .map(|i| i.diff_order.clone())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let key = VisibleKey {
            status: status.as_ref().map(|s| std::sync::Arc::as_ptr(s) as usize),
            text: self.filter.read(cx).value().to_string(),
            show_filter: self.filter_visible,
            filter: rs.map(|rs| rs.file_list_filter).unwrap_or_default(),
            hide: s
                .flags
                .text(corvene_core::flags::ids::CHANGES_HIDE_GLOBS)
                .to_string(),
            order: order.to_string(),
            order_file,
            mode: s
                .flags
                .text(corvene_core::flags::ids::CHANGES_FILTER_MATCH)
                .to_string(),
            whitespace_only: whitespace_only
                .as_ref()
                .map(|w| std::sync::Arc::as_ptr(w) as usize),
            lists: lists.as_ref().map(|l| std::sync::Arc::as_ptr(l) as usize),
        };
        let cached = self
            .visible_cache
            .borrow()
            .as_ref()
            .filter(|data| data.key == key)
            .cloned();
        let data = cached.unwrap_or_else(|| {
            let data = Rc::new(VisibleData::build(
                key,
                status.as_ref(),
                whitespace_only.as_deref(),
                lists.as_ref(),
            ));
            *self.visible_cache.borrow_mut() = Some(data.clone());
            data
        });
        Rc::new(VisibleFiles { status, data })
    }

    /// `1310-file-list-tree` / `1313-changelists`: the visible files as
    /// rows: a section per group, each a folder tree while file lists show
    /// trees.
    fn layout(&self, cx: &App) -> Rc<LayoutData> {
        use corvene_core::file_tree::{FileTree, TreeRow};
        let tree_mode = crate::file_tree_rows::tree_mode(cx);
        let visible = self.visible(cx);
        let s = self.state.read(cx);
        let collapsed = tree_mode
            .then(|| {
                s.selected
                    .and_then(|id| s.collapsed_folders.get(&id))
                    .cloned()
            })
            .flatten();
        // a filter shows every match, collapsed or not
        let expand_all = !self.filter.read(cx).value().trim().is_empty();
        if let Some(hit) = self.layout_cache.borrow().as_ref().filter(|l| {
            Rc::ptr_eq(&l.visible, &visible.data)
                && l.collapsed
                    == collapsed
                        .as_ref()
                        .map(|c| std::sync::Arc::as_ptr(c) as usize)
                && l.expand_all == expand_all
                && l.tree_mode == tree_mode
        }) {
            return hit.clone();
        }
        let groups: Vec<(Option<usize>, Range<usize>, bool)> = match visible.groups() {
            Some(groups) => groups
                .iter()
                .enumerate()
                .map(|(g, info)| (Some(g), info.range.clone(), info.collapsed && !expand_all))
                .collect(),
            None => vec![(None, 0..visible.len(), false)],
        };
        let mut sections = Vec::with_capacity(groups.len());
        let mut include = Vec::with_capacity(visible.len() + groups.len());
        let mut rows = 0;
        for (group, range, group_collapsed) in groups {
            let header = group.map(|_| {
                let row = rows;
                rows += 1;
                include.push(crate::file_tree_rows::check_value(
                    range
                        .clone()
                        .filter_map(|i| visible.get(i))
                        .map(include_value),
                ));
                row
            });
            let first = rows;
            let tree = (tree_mode && !group_collapsed).then(|| {
                let paths: Vec<&str> = range
                    .clone()
                    .filter_map(|i| visible.get(i))
                    .map(|f| f.path.as_str())
                    .collect();
                FileTree::build(&paths, |folder| {
                    !expand_all && collapsed.as_ref().is_some_and(|c| c.contains(folder))
                })
            });
            let body = if group_collapsed {
                0
            } else if let Some(tree) = &tree {
                tree.rows.len()
            } else {
                range.len()
            };
            if let Some(tree) = &tree {
                include.extend(tree.rows.iter().enumerate().map(|(row, r)| {
                    match r {
                        TreeRow::File { item, .. } => {
                            visible.get(range.start + item).and_then(include_value)
                        }
                        TreeRow::Folder { .. } => crate::file_tree_rows::check_value(
                            tree.row_items(row)
                                .iter()
                                .filter_map(|&i| visible.get(range.start + i))
                                .map(include_value),
                        ),
                    }
                }));
            } else if body > 0 {
                include.extend(
                    range
                        .clone()
                        .map(|i| visible.get(i).and_then(include_value)),
                );
            }
            rows += body;
            sections.push(Section {
                group,
                header,
                first,
                body,
                offset: range.start,
                len: range.len(),
                collapsed: group_collapsed,
                tree,
            });
        }
        let data = Rc::new(LayoutData {
            visible: visible.data.clone(),
            collapsed: collapsed
                .as_ref()
                .map(|c| std::sync::Arc::as_ptr(c) as usize),
            _collapsed: collapsed,
            expand_all,
            tree_mode,
            sections,
            rows,
            include,
        });
        *self.layout_cache.borrow_mut() = Some(data.clone());
        data
    }

    /// `1310-file-list-tree`: the order a ⇧-click range follows (the rows'
    /// while they are structured, collapsed folders' files included).
    fn selection_order(&self, cx: &App) -> Vec<String> {
        let visible = self.visible(cx);
        let layout = self.layout(cx);
        if layout.structured() {
            layout
                .order()
                .iter()
                .filter_map(|&i| visible.get(i))
                .map(|f| f.path.clone())
                .collect()
        } else {
            visible.paths()
        }
    }

    /// The files of row `row` (a folder's or a group's all).
    fn row_files(&self, layout: &LayoutData, row: usize, cx: &App) -> Vec<String> {
        let visible = self.visible(cx);
        layout
            .row_items(row)
            .iter()
            .filter_map(|&i| visible.get(i))
            .map(|f| f.path.clone())
            .collect()
    }

    /// `1310-file-list-tree`: the files of the folder row for `folder` in
    /// section `section`.
    fn folder_files(&self, section: usize, folder: &str, cx: &App) -> Vec<String> {
        let layout = self.layout(cx);
        layout
            .folder_row(section, folder)
            .map(|row| self.row_files(&layout, row, cx))
            .unwrap_or_default()
    }

    /// `1313-changelists`: the files of the group in section `section`.
    fn section_files(&self, section: usize, cx: &App) -> Vec<String> {
        let layout = self.layout(cx);
        layout
            .sections
            .get(section)
            .and_then(|s| s.header)
            .map(|row| self.row_files(&layout, row, cx))
            .unwrap_or_default()
    }

    /// Select row `row` (a folder or a group header selects its files) and
    /// keep it in view.
    fn select_row(&mut self, row: usize, cx: &mut Context<Self>) {
        use corvene_core::file_tree::TreeRow;
        let layout = self.layout(cx);
        let Some(id) = self.state.read(cx).selected else {
            return;
        };
        match layout.row(row) {
            Some(RowRef::File { visible, .. }) => {
                let Some(path) = self.visible(cx).get(visible).map(|f| f.path.clone()) else {
                    return;
                };
                self.tree_cursor = None;
                self.group_cursor = None;
                Dispatcher::select_file(id, path, cx);
            }
            Some(RowRef::Folder {
                section,
                row: local,
            }) => {
                let Some(TreeRow::Folder { path, .. }) = layout.sections[section]
                    .tree
                    .as_ref()
                    .and_then(|t| t.rows.get(local))
                else {
                    return;
                };
                let files = self.row_files(&layout, row, cx);
                self.tree_cursor = Some((section, path.clone()));
                self.group_cursor = None;
                Dispatcher::select_all_files(id, files, cx);
                cx.notify();
            }
            Some(RowRef::Header(section)) => {
                let files = self.row_files(&layout, row, cx);
                self.tree_cursor = None;
                self.group_cursor = Some(section);
                Dispatcher::select_all_files(id, files, cx);
                cx.notify();
            }
            None => return,
        }
        self.list_scroll.scroll_to_item(
            self.archive_splice.borrow().display_of(row),
            ScrollStrategy::Nearest,
        );
    }

    /// The row the keyboard moves from: the group header or folder row the
    /// user put the cursor on while its files are still the selection, else
    /// the last selected file's row.
    fn current_row(&self, layout: &LayoutData, cx: &App) -> Option<usize> {
        let s = self.state.read(cx);
        let rs = s.selected_state()?;
        let n = rs.selected_files.len();
        if let Some(section) = self.group_cursor
            && let Some(sec) = layout.sections.get(section)
            && let Some(header) = sec.header
            && (sec.len == n || sec.len == 0)
        {
            return Some(header);
        }
        if let Some((section, folder)) = &self.tree_cursor
            && let Some(sec) = layout.sections.get(*section)
            && let Some(tree) = &sec.tree
            && let Some(local) = crate::file_tree_rows::current_row(tree, Some(folder), n, None)
        {
            return Some(sec.first + local);
        }
        let last = rs.selected_files.last().or(rs.selected_file.as_ref());
        let item = last.and_then(|p| self.visible(cx).position(p))?;
        layout.row_of_visible(item)
    }

    /// `1310-file-list-tree`: collapse or expand `folders`.
    fn set_collapsed(&mut self, folders: Vec<String>, collapsed: bool, cx: &mut Context<Self>) {
        if let Some(id) = self.state.read(cx).selected {
            Dispatcher::set_folders_collapsed(id, folders, collapsed, cx);
        }
    }

    /// `1313-changelists`: collapse or expand group `group`.
    fn set_group_collapsed(&mut self, group: u64, collapsed: bool, cx: &mut Context<Self>) {
        if let Some(id) = self.state.read(cx).selected {
            Dispatcher::set_changelist_collapsed(id, group, collapsed, cx);
        }
    }

    /// The group id of section `section`'s header, when it has one.
    fn section_group(&self, layout: &LayoutData, section: usize) -> Option<u64> {
        let g = layout.sections.get(section)?.group?;
        layout.visible.groups.as_ref()?.get(g).map(|info| info.id)
    }

    /// `1310-file-list-tree`: ← collapses the folder or goes to its parent
    /// (`1313-changelists`: or collapses the group, or goes to its header).
    fn tree_left(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::file_tree_rows::LeftKey;
        let layout = self.layout(cx);
        if !layout.structured() {
            return;
        }
        let Some(row) = self.current_row(&layout, cx) else {
            return;
        };
        let Some(section) = layout.section_of_row(row) else {
            return;
        };
        let sec = &layout.sections[section];
        if sec.header == Some(row) {
            match self.section_group(&layout, section) {
                Some(group) if !sec.collapsed => self.set_group_collapsed(group, true, cx),
                _ => crate::file_tree_rows::focus_pane(false, window, cx),
            }
            return;
        }
        let key = sec.tree.as_ref().map_or(LeftKey::Nothing, |tree| {
            crate::file_tree_rows::left_key(tree, row - sec.first)
        });
        let (header, first) = (sec.header, sec.first);
        match key {
            LeftKey::Collapse(folder) => self.set_collapsed(vec![folder], true, cx),
            LeftKey::Select(parent) => self.select_row(first + parent, cx),
            LeftKey::Nothing => match header {
                Some(header) => self.select_row(header, cx),
                None => crate::file_tree_rows::focus_pane(false, window, cx),
            },
        }
    }

    /// `1310-file-list-tree`: → expands the folder or goes into it; on a file
    /// it does what → does in the flat list (`1313-changelists`: expands a
    /// group or goes to its first row).
    fn tree_right(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::file_tree_rows::RightKey;
        let layout = self.layout(cx);
        if !layout.structured() {
            return;
        }
        let Some(row) = self.current_row(&layout, cx) else {
            crate::file_tree_rows::focus_pane(true, window, cx);
            return;
        };
        let Some(section) = layout.section_of_row(row) else {
            return;
        };
        let sec = &layout.sections[section];
        if sec.header == Some(row) {
            if sec.collapsed {
                if let Some(group) = self.section_group(&layout, section) {
                    self.set_group_collapsed(group, false, cx);
                }
            } else if sec.body > 0 {
                let first = sec.first;
                self.select_row(first, cx);
            } else {
                crate::file_tree_rows::focus_pane(true, window, cx);
            }
            return;
        }
        let key = sec
            .tree
            .as_ref()
            .and_then(|tree| crate::file_tree_rows::right_key(tree, row - sec.first));
        let first = sec.first;
        match key {
            Some(RightKey::Expand(folder)) => self.set_collapsed(vec![folder], false, cx),
            Some(RightKey::Select(local)) => self.select_row(first + local, cx),
            None => crate::file_tree_rows::focus_pane(true, window, cx),
        }
    }

    /// The selection for the rows (cached while it does not change).
    fn selected_paths(&self, cx: &App) -> Rc<SelectedPaths> {
        let s = self.state.read(cx);
        let list = s
            .selected_state()
            .map_or(&[][..], |rs| &rs.selected_files[..]);
        if let Some(cached) = self.selected_cache.borrow().as_ref()
            && cached.list == list
        {
            return cached.clone();
        }
        let selected = Rc::new(SelectedPaths::new(list.to_vec()));
        *self.selected_cache.borrow_mut() = Some(selected.clone());
        selected
    }

    /// GHD `createStateUpdate` (augmented-filter-list.tsx): when a non-empty
    /// filter hides every selected file, the first visible file is selected.
    fn select_first_visible_if_hidden(&self, cx: &mut App) {
        if self.filter.read(cx).value().trim().is_empty() {
            return;
        }
        let visible = self.visible(cx);
        let selected = self.selected_paths(cx);
        let Some(id) = self.state.read(cx).selected else {
            return;
        };
        let hidden = !visible.iter().any(|f| selected.contains(&f.path));
        if hidden && let Some(first) = visible.get(0) {
            Dispatcher::select_file(id, first.path.clone(), cx);
        }
    }

    /// GHD `isCommittingFileHiddenByFilter`: a text filter or filter option
    /// is active, not every file is listed, and a file included in the
    /// commit is among the hidden ones. Returns the included count.
    fn committing_hidden_files(&self, cx: &App) -> Option<usize> {
        let text = self.filter.read(cx).value().to_string();
        let filter = self.filter_options(cx);
        if !corvene_core::filter::has_active_filters(&text, &filter) {
            return None;
        }
        let visible = self.visible(cx);
        visible
            .data
            .included_hidden
            .then_some(visible.data.included)
    }

    /// `.hidden-changes-warning` between the list and the commit form.
    /// Corvene (`1112-sparse-checkout`): "Sparse checkout is on" over the
    /// list, with Edit… and Turn Off.
    fn sparse_checkout_banner(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        let s = self.state.read(cx);
        if !s.flags.bool(corvene_core::flags::ids::SPARSE_CHECKOUT) {
            return None;
        }
        let id = s.selected?;
        let sparse = s.selected_state()?.sparse_checkout?;
        let t = cx.ghd();
        let detail = match (sparse.cone, sparse.patterns) {
            (true, 0) => "Only the files at the top".to_string(),
            (true, 1) => "1 folder checked out".to_string(),
            (true, n) => format!("{n} folders checked out"),
            (false, 1) => "1 pattern".to_string(),
            (false, n) => format!("{n} patterns"),
        };
        Some(
            div()
                .id("sparse-checkout-banner")
                .flex_none()
                .flex()
                .flex_col()
                .py(SPACING_HALF())
                .px(SPACING())
                .bg(t.box_alt_background)
                .border_b_1()
                .border_color(t.box_border)
                .text_size(FONT_SIZE())
                .line_height(zpx(18.))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .child(
                            octicon(Octicon::FileDirectory, t.text_secondary)
                                .flex_none()
                                .mr(SPACING_HALF()),
                        )
                        .child(div().min_w_0().truncate().child("Sparse checkout is on")),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap_x(SPACING())
                        .child(div().text_color(t.text_secondary).child(detail))
                        .child(
                            crate::widgets::link_button("sparse-checkout-edit", "Edit…", cx)
                                .on_click(move |_, _, cx| Dispatcher::show_sparse_checkout(id, cx)),
                        )
                        .child(
                            crate::widgets::link_button(
                                "sparse-checkout-off",
                                mac_or("Turn Off", "Turn off"),
                                cx,
                            )
                            .on_click(move |_, _, cx| Dispatcher::disable_sparse_checkout(id, cx)),
                        ),
                ),
        )
    }

    fn hidden_changes_warning(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        let count = self.committing_hidden_files(cx)?;
        let id = self.state.read(cx).selected?;
        let t = cx.ghd();
        let filter = self.filter.clone();
        Some(
            div()
                .id("hidden-changes-warning")
                .flex_none()
                .flex()
                .flex_col()
                .py(SPACING_HALF())
                .px(SPACING())
                .bg(t.file_warning_background)
                .border_t_1()
                .border_b_1()
                .border_color(t.file_warning_border)
                .text_size(FONT_SIZE())
                .line_height(zpx(18.))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .child(octicon(Octicon::Alert, t.file_warning).mr(SPACING_HALF()))
                        .child("Hidden changes will be committed."),
                )
                .child(
                    // `showFilesToBeCommitted`: clear the filters, then list
                    // only what is included in the commit
                    crate::widgets::link_button(
                        "hidden-changes-adjust",
                        format!("Adjust the filters to see all {count} changes"),
                        cx,
                    )
                    .on_click(move |_, window, cx| {
                        filter.update(cx, |input, cx| input.set_value("", window, cx));
                        Dispatcher::clear_filter_options(id, cx);
                        Dispatcher::toggle_filter_option(
                            id,
                            corvene_core::FilterOption::IncludedInCommit,
                            cx,
                        );
                    }),
                ),
        )
    }

    fn filter_options(&self, cx: &App) -> FileListFilter {
        self.state
            .read(cx)
            .selected_state()
            .map(|rs| rs.file_list_filter)
            .unwrap_or_default()
    }

    /// `ChangesListFilterOptions` popover: header, five option checkboxes,
    /// "Clear filters" when anything is active. Anchored under the button.
    fn filter_popover(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        if !self.filter_popover_open {
            return None;
        }
        let t = cx.ghd();
        let s = self.state.read(cx);
        let id = s.selected?;
        let status = s.selected_state().and_then(|rs| rs.status.clone());
        let files = status.as_deref().map_or(&[][..], |st| &st.files[..]);
        let filter = self.filter_options(cx);
        let active =
            corvene_core::filter::has_active_filters(&self.filter.read(cx).value(), &filter);
        // `705-renamed-files-filter` (kept while active, to be cleared)
        let renamed_option =
            s.flags.bool(corvene_core::flags::ids::RENAMED_FILES_FILTER) || filter.renamed;
        let bounds = self.filter_button_bounds.get();
        let close =
            |this: &mut Self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>| {
                this.filter_popover_open = false;
                cx.notify();
            };
        let option_row = |option: FilterOption, label: &str| {
            let checked = filter.get(option);
            let count = option_count(option, files);
            div()
                .id(SharedString::from(format!("filter-opt-{label}")))
                .flex()
                .flex_row()
                .items_center()
                .gap(SPACING_HALF())
                // `.checkbox-component`: one 18 px line
                .h(zpx(18.))
                .cursor_pointer()
                // GHD closes the popover after every option change
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.filter_popover_open = false;
                    Dispatcher::toggle_filter_option(id, option, cx)
                }))
                .child(checkbox(
                    SharedString::from(format!("filter-check-{label}")),
                    checked,
                    false,
                    cx,
                ))
                .child(
                    div()
                        .text_size(FONT_SIZE())
                        .child(format!("{label} ({count})")),
                )
        };
        // a window-sized layer under the balloon closes it on any click
        // outside (`onMousedownOutside`)
        let overlay = deferred(
            anchored().position(point(zpx(0.), zpx(0.))).child(
                div()
                    .id("filter-popover-overlay")
                    .relative()
                    .size_full()
                    .on_mouse_down(MouseButton::Left, cx.listener(close))
                    .on_mouse_down(MouseButton::Right, cx.listener(close)),
            ),
        )
        .with_priority(25);
        Some(
            div().child(overlay).child(
                crate::popover::balloon_popover(
                    bounds,
                    crate::popover::PopoverAnchorPosition::BottomRight,
                    crate::popover::popover_component(cx)
                        .id("filter-popover")
                        .occlude()
                        // `.filter-popover { min-width: 200px }`,
                        // `.popover-content { padding: var(--spacing)
                        // var(--spacing) 0 var(--spacing) }`
                        .min_w(zpx(200.))
                        .px(SPACING())
                        .pt(SPACING())
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .child(
                                    // `h3` (UA style: 1.17em, bold)
                                    div()
                                        .text_size(FONT_SIZE() * 1.17)
                                        .line_height(FONT_SIZE() * 1.17 * 1.5)
                                        .font_weight(FontWeight::BOLD)
                                        .child("Filter Options"),
                                )
                                .child(
                                    div()
                                        .id("filter-popover-close")
                                        .icon_button_label("Close")
                                        .size(zpx(16.))
                                        .cursor_pointer()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.filter_popover_open = false;
                                            cx.notify();
                                        }))
                                        .child(octicon(Octicon::X, t.text_secondary)),
                                ),
                        )
                        .child(
                            div()
                                .my(SPACING())
                                .flex()
                                .flex_col()
                                .child(option_row(
                                    FilterOption::IncludedInCommit,
                                    "Included in commit",
                                ))
                                .child(option_row(
                                    FilterOption::ExcludedFromCommit,
                                    "Excluded from commit",
                                ))
                                .child(option_row(FilterOption::NewFiles, "New files"))
                                .child(option_row(FilterOption::ModifiedFiles, "Modified files"))
                                .child(option_row(FilterOption::DeletedFiles, "Deleted files"))
                                .when(renamed_option, |d| {
                                    d.child(option_row(FilterOption::RenamedFiles, "Renamed files"))
                                }),
                        )
                        .when(active, |d| {
                            d.child(div().pt(SPACING_HALF()).pb(SPACING()).child(
                                button("filter-clear", "Clear filters", cx).on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        this.filter.update(cx, |s, cx| s.set_value("", window, cx));
                                        this.filter_popover_open = false;
                                        Dispatcher::clear_filter_options(id, cx);
                                    },
                                )),
                            ))
                        }),
                    cx,
                )
                .with_priority(26),
            ),
        )
    }

    fn open_menu(
        &mut self,
        items: Vec<MenuItem>,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // GHD's Electron `Menu.popup`: an NSMenu on macOS, a views menu on Linux
        crate::native_menu::show_context_menu(items, position, window, cx);
    }

    /// `1310-file-list-tree`: select the folder row for `folder` in section
    /// `section`.
    fn select_folder(&mut self, section: usize, folder: &str, cx: &mut Context<Self>) {
        if let Some(row) = self.layout(cx).folder_row(section, folder) {
            self.select_row(row, cx);
        }
    }

    /// `1310-file-list-tree`: a folder row's menu; its items act on every
    /// file inside.
    fn open_folder_menu(
        &mut self,
        section: usize,
        folder: String,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (id, confirm, repo_path, stash_files, rebase_conflict, on_disk) = {
            let s = self.state.read(cx);
            let Some(id) = s.selected else { return };
            let Some(rs) = s.selected_state() else { return };
            if rs.committing {
                return;
            }
            let Some(repo) = s.repository(id) else { return };
            let stash_files = s
                .flags
                .bool(corvene_core::flags::ids::STASH_SELECTED_FILES)
                .then(|| {
                    (
                        rs.desktop_stash().is_some(),
                        rs.info.as_ref().and_then(|i| i.current_branch()).is_some()
                            && rs.conflict_state.is_none()
                            && !rs.status.as_deref().is_some_and(|st| {
                                st.files.iter().any(|f| f.status.is_conflicted())
                            }),
                    )
                });
            // a folder whose files are all deleted is gone from disk
            let prefix = format!("{folder}/");
            let on_disk = rs.status.as_deref().is_some_and(|st| {
                st.files.iter().any(|f| {
                    f.path.starts_with(&prefix) && f.status.kind != FileStatusKind::Deleted
                })
            });
            (
                id,
                s.settings.confirm_discard_changes,
                repo.path.clone(),
                stash_files,
                rs.conflict_state.is_some(),
                on_disk,
            )
        };
        let paths = self.folder_files(section, &folder, cx);
        if paths.is_empty() {
            return;
        }
        // `1313-changelists`
        let changelists = corvene_core::changelists::for_menu(self.state.read(cx), id);
        let ellipsis = if confirm { "…" } else { "" };
        let mut items = Vec::new();
        let discard = paths.clone();
        items.push(
            MenuItem::new(
                format!(
                    "{}{ellipsis}",
                    mac_or("Discard Changes in Folder", "Discard changes in folder")
                ),
                move |_, cx| Dispatcher::request_discard_changes(id, discard.clone(), cx),
            )
            .enabled(!rebase_conflict),
        );
        if let Some((has_stash, can_stash)) = stash_files {
            let to_stash = paths.clone();
            items.push(
                MenuItem::new(
                    mac_or("Stash Files in Folder", "Stash files in folder"),
                    move |_, cx| Dispatcher::stash_selected_files(id, to_stash.clone(), cx),
                )
                .enabled(can_stash && !has_stash),
            );
        }
        items.push(MenuItem::separator());
        let pattern = format!("/{folder}");
        items.push(
            MenuItem::new(
                mac_or(
                    "Ignore Folder (Add to .gitignore)",
                    "Ignore folder (add to .gitignore)",
                ),
                move |_, cx| Dispatcher::ignore_files(id, vec![pattern.clone()], cx),
            )
            .enabled(!rebase_conflict),
        );
        items.push(MenuItem::separator());
        let include = paths.clone();
        items.push(MenuItem::new(
            mac_or("Include Files in Folder", "Include files in folder"),
            move |_, cx| Dispatcher::set_files_included(id, include.clone(), true, cx),
        ));
        let exclude = paths.clone();
        items.push(MenuItem::new(
            mac_or("Exclude Files in Folder", "Exclude files in folder"),
            move |_, cx| Dispatcher::set_files_included(id, exclude.clone(), false, cx),
        ));
        // `1313-changelists`
        if let Some(lists) = changelists.as_deref() {
            items.push(changelist_submenu(id, paths.clone(), lists));
        }
        items.push(MenuItem::separator());
        let full = repo_path.join(&folder);
        let absolute = full.to_string_lossy().into_owned();
        items.push(MenuItem::new(
            mac_or("Copy Folder Path", "Copy folder path"),
            move |_, cx| cx.write_to_clipboard(ClipboardItem::new_string(absolute.clone())),
        ));
        let relative = folder.clone();
        items.push(MenuItem::new(
            mac_or("Copy Relative Folder Path", "Copy relative folder path"),
            move |_, cx| cx.write_to_clipboard(ClipboardItem::new_string(relative.clone())),
        ));
        items.push(
            MenuItem::new(labels::REVEAL_IN_FILE_MANAGER, move |_, cx| {
                Dispatcher::show_in_finder(&full, cx)
            })
            .enabled(on_disk),
        );
        items.push(MenuItem::separator());
        let all = {
            let visible = self.visible(cx);
            let paths: Vec<&str> = visible.iter().map(|f| f.path.as_str()).collect();
            corvene_core::file_tree::FileTree::folder_paths(&paths)
        };
        let expand = all.clone();
        items.push(MenuItem::new(
            mac_or("Expand All", "Expand all"),
            move |_, cx| Dispatcher::set_folders_collapsed(id, expand.clone(), false, cx),
        ));
        items.push(MenuItem::new(
            mac_or("Collapse All", "Collapse all"),
            move |_, cx| Dispatcher::set_folders_collapsed(id, all.clone(), true, cx),
        ));
        self.open_menu(items, position, window, cx);
    }

    /// `1313-changelists`: a group header's menu.
    fn open_group_menu(
        &mut self,
        section: usize,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let layout = self.layout(cx);
        let Some(info) = layout
            .sections
            .get(section)
            .and_then(|s| s.group)
            .and_then(|g| layout.visible.groups.as_ref()?.get(g))
        else {
            return;
        };
        let (list_id, name, description, active, checkable, has_files) = (
            info.id,
            info.name.clone(),
            info.description.clone(),
            info.active,
            info.checkable,
            info.total > 0,
        );
        let files = self.section_files(section, cx);
        let visible_files = !files.is_empty();
        let (id, stash, all_included) = {
            let s = self.state.read(cx);
            let Some(id) = s.selected else { return };
            let Some(rs) = s.selected_state() else { return };
            if rs.committing {
                return;
            }
            let wanted: std::collections::HashSet<&str> =
                files.iter().map(String::as_str).collect();
            let all_included = rs.status.as_deref().is_some_and(|st| {
                st.files
                    .iter()
                    .filter(|f| wanted.contains(f.path.as_str()))
                    .all(|f| f.selection.kind() == DiffSelectionType::All)
            });
            (
                id,
                corvene_core::stash_flows::PartialStash::of(s, rs),
                all_included,
            )
        };
        let named = list_id != UNGROUPED;
        let mut items = Vec::new();
        if named {
            let weak = cx.weak_entity();
            let (summary, body) = (name.to_string(), description.clone());
            items.push(
                MenuItem::new(
                    mac_or("Commit Only This List", "Commit only this list"),
                    move |window, cx| {
                        Dispatcher::check_only_changelist(id, list_id, cx);
                        weak.update(cx, |this, cx| {
                            this.prefill_commit_message(&summary, &body, window, cx)
                        })
                        .ok();
                    },
                )
                .enabled(has_files),
            );
            items.push(MenuItem::checkbox(
                mac_or("Active Changelist", "Active changelist"),
                active,
                move |_, cx| {
                    Dispatcher::set_active_changelist(id, (!active).then_some(list_id), cx)
                },
            ));
            items.push(MenuItem::separator());
        }
        let include_label = match (all_included, named) {
            (true, true) => mac_or("Exclude Files in List", "Exclude files in list"),
            (false, true) => mac_or("Include Files in List", "Include files in list"),
            (true, false) => mac_or("Exclude Files in Group", "Exclude files in group"),
            (false, false) => mac_or("Include Files in Group", "Include files in group"),
        };
        let to_include = files.clone();
        items.push(
            MenuItem::new(include_label, move |_, cx| {
                Dispatcher::set_files_included(id, to_include.clone(), !all_included, cx)
            })
            .enabled(visible_files && checkable),
        );
        if named {
            if let Some(stash) = stash {
                let label = mac_or("Stash This List", "Stash this list");
                let label = if stash == corvene_core::stash_flows::PartialStash::Blocked {
                    format!(
                        "{label}{}",
                        mac_or(
                            " (Restore or Discard the Stash First)",
                            " (restore or discard the stash first)"
                        )
                    )
                } else {
                    label.to_string()
                };
                items.push(
                    MenuItem::new(label, move |_, cx| {
                        Dispatcher::stash_changelist(id, list_id, cx)
                    })
                    .enabled(
                        has_files && stash != corvene_core::stash_flows::PartialStash::Blocked,
                    ),
                );
            }
            items.push(MenuItem::separator());
            items.push(MenuItem::new(
                mac_or("Edit Changelist…", "Edit changelist…"),
                move |_, cx| {
                    Dispatcher::show_popup(
                        Popup::NewChangelist {
                            repo: id,
                            paths: Vec::new(),
                            edit: Some(list_id),
                        },
                        cx,
                    )
                },
            ));
            items.push(MenuItem::new(
                mac_or("Delete Changelist", "Delete changelist"),
                move |_, cx| Dispatcher::delete_changelist(id, list_id, cx),
            ));
        } else {
            let to_list = files.clone();
            items.push(
                MenuItem::new(
                    mac_or(
                        "New Changelist from These Files…",
                        "New changelist from these files…",
                    ),
                    move |_, cx| {
                        Dispatcher::show_popup(
                            Popup::NewChangelist {
                                repo: id,
                                paths: to_list.clone(),
                                edit: None,
                            },
                            cx,
                        )
                    },
                )
                .enabled(visible_files),
            );
        }
        self.open_menu(items, position, window, cx);
    }

    /// `1313-changelists` › Commit Only This List: an empty summary takes
    /// the list's name and an empty description its description; the
    /// summary gets focus.
    fn prefill_commit_message(
        &mut self,
        name: &str,
        description: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.summary.read(cx).value().trim().is_empty() {
            self.replace_message_field(CommitField::Summary, name.to_string(), window, cx);
            self.refresh_spelling(CommitField::Summary, cx);
        }
        if !description.is_empty()
            && self
                .field_text_and_caret(CommitField::Description, cx)
                .0
                .trim()
                .is_empty()
        {
            self.replace_message_field(
                CommitField::Description,
                description.to_string(),
                window,
                cx,
            );
            self.refresh_spelling(CommitField::Description, cx);
        }
        self.focus_summary(window, cx);
    }

    /// GHD `onItemContextMenu`: the default menu, or the reduced one while a
    /// rebase is stopped on conflicts (`getRebaseContextMenu`). Right-clicking
    /// inside the current selection applies the items to every selected file.
    fn open_file_menu(
        &mut self,
        file: WorkingDirectoryFileChange,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (
            id,
            confirm,
            repo_path,
            selected_files,
            rebase_conflict,
            status_files,
            open_many,
            ignore_counts,
            copy_diff,
            assume_unchanged,
            open_file_with,
            stash_files,
        ) = {
            let s = self.state.read(cx);
            let Some(id) = s.selected else { return };
            let Some(rs) = s.selected_state() else { return };
            if rs.committing {
                return;
            }
            let Some(repo) = s.repository(id) else { return };
            // `777-stash-selected-files`: (branch has a stash, can stash at
            // all: a branch and no conflicts)
            let stash_files = s
                .flags
                .bool(corvene_core::flags::ids::STASH_SELECTED_FILES)
                .then(|| {
                    (
                        rs.desktop_stash().is_some(),
                        rs.info.as_ref().and_then(|i| i.current_branch()).is_some()
                            && rs.conflict_state.is_none()
                            && !rs.status.as_deref().is_some_and(|st| {
                                st.files.iter().any(|f| f.status.is_conflicted())
                            }),
                    )
                });
            (
                id,
                s.settings.confirm_discard_changes,
                repo.path.clone(),
                rs.selected_files.clone(),
                rs.conflict_state
                    .as_ref()
                    .is_some_and(|c| matches!(c.kind, corvene_core::ConflictKind::Rebase { .. })),
                rs.status
                    .as_deref()
                    .map(|st| st.files.clone())
                    .unwrap_or_default(),
                s.flags.bool(corvene_core::flags::ids::OPEN_MULTIPLE_FILES),
                s.flags.bool(corvene_core::flags::ids::IGNORE_MENU_COUNTS),
                s.flags.bool(corvene_core::flags::ids::COPY_DIFF),
                s.flags.bool(corvene_core::flags::ids::ASSUME_UNCHANGED),
                s.flags.bool(corvene_core::flags::ids::OPEN_FILE_WITH),
                stash_files,
            )
        };
        let path = file.path.clone();
        let full = repo_path.join(&path);
        let deleted = file.status.kind == FileStatusKind::Deleted;
        let editor_label = self.state.read(cx).editor_label();
        // `1313-changelists`
        let changelists = corvene_core::changelists::for_menu(self.state.read(cx), id);
        let discard_item = |paths: Vec<String>| {
            // `getDiscardChangesMenuItemLabel`
            let label = match paths.len() {
                1 => mac_or("Discard Changes", "Discard changes").to_string(),
                n if IS_MAC => format!("Discard {n} Selected Changes"),
                n => format!("Discard {n} selected changes"),
            };
            let label = if confirm {
                format!("{label}…")
            } else {
                label
            };
            MenuItem::new(label, move |_, cx| {
                Dispatcher::request_discard_changes(id, paths.clone(), cx)
            })
        };
        let copy_items = |files: Vec<WorkingDirectoryFileChange>| {
            let multi = files.len() > 1;
            let absolute = files
                .iter()
                .map(|f| repo_path.join(&f.path).to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("\n");
            let relative = files
                .iter()
                .map(|f| f.path.clone())
                .collect::<Vec<_>>()
                .join("\n");
            vec![
                MenuItem::new(
                    if multi {
                        labels::COPY_SELECTED_PATHS
                    } else {
                        labels::COPY_FILE_PATH
                    },
                    move |_, cx| cx.write_to_clipboard(ClipboardItem::new_string(absolute.clone())),
                ),
                MenuItem::new(
                    if multi {
                        labels::COPY_SELECTED_RELATIVE_PATHS
                    } else {
                        labels::COPY_RELATIVE_FILE_PATH
                    },
                    move |_, cx| cx.write_to_clipboard(ClipboardItem::new_string(relative.clone())),
                ),
            ]
        };
        let open_items = |full: PathBuf, deleted: bool| {
            let reveal = full.clone();
            let editor = full.clone();
            let default = full;
            let mut items = vec![
                MenuItem::new(labels::REVEAL_IN_FILE_MANAGER, move |_, cx| {
                    Dispatcher::show_in_finder(&reveal, cx)
                })
                .enabled(!deleted),
                MenuItem::new(labels::open_in(&editor_label), move |_, cx| {
                    Dispatcher::open_in_editor(editor.clone(), cx)
                })
                .enabled(!deleted),
                MenuItem::new(labels::OPEN_WITH_DEFAULT_PROGRAM, {
                    let default = default.clone();
                    move |_, cx| cx.open_with_system(&default)
                })
                .enabled(!deleted),
            ];
            // Android: the share sheet
            #[cfg(target_os = "android")]
            let share = default.clone();
            // `713-open-file-with`
            if open_file_with {
                items.push(
                    MenuItem::new(mac_or("Open With…", "Open with…"), move |_, cx| {
                        Dispatcher::open_with(default.clone(), cx)
                    })
                    .enabled(!deleted),
                );
            }
            #[cfg(target_os = "android")]
            items.push(
                MenuItem::new("Share…", move |_, cx| {
                    Dispatcher::share_file(share.clone(), cx)
                })
                .enabled(!deleted),
            );
            items
        };

        if rebase_conflict {
            let mut items = Vec::new();
            if file.status.kind == FileStatusKind::Untracked {
                items.push(discard_item(vec![path.clone()]));
                items.push(MenuItem::separator());
            }
            items.extend(copy_items(vec![file.clone()]));
            items.push(MenuItem::separator());
            items.extend(open_items(full, deleted));
            self.open_menu(items, position, window, cx);
            return;
        }

        // `718-ignore-menu-counts`: changed files per extension
        let extension_count = |ext: &str| {
            status_files
                .iter()
                .filter(|f| {
                    Path::new(&f.path)
                        .extension()
                        .is_some_and(|e| format!(".{}", e.to_string_lossy()) == ext)
                })
                .count()
        };
        // `getDefaultContextMenu`
        let targets: Vec<WorkingDirectoryFileChange> = if selected_files.contains(&path) {
            let selected: std::collections::HashSet<&str> =
                selected_files.iter().map(String::as_str).collect();
            status_files
                .iter()
                .filter(|f| selected.contains(f.path.as_str()))
                .cloned()
                .collect()
        } else {
            vec![file.clone()]
        };
        let paths: Vec<String> = targets.iter().map(|f| f.path.clone()).collect();
        let mut items = vec![discard_item(paths.clone())];
        // `777-stash-selected-files`: only while the branch has no stash
        if let Some((has_stash, can_stash)) = stash_files {
            let label = match (paths.len(), IS_MAC) {
                (1, _) => mac_or("Stash File", "Stash file").to_string(),
                (n, true) => format!("Stash {n} Selected Files"),
                (n, false) => format!("Stash {n} selected files"),
            };
            let label = if has_stash {
                let why = mac_or(
                    " (Restore or Discard the Stash First)",
                    " (restore or discard the stash first)",
                );
                format!("{label}{why}")
            } else {
                label
            };
            let to_stash = paths.clone();
            items.push(
                MenuItem::new(label, move |_, cx| {
                    Dispatcher::stash_selected_files(id, to_stash.clone(), cx)
                })
                .enabled(can_stash && !has_stash),
            );
        }
        // `1313-changelists`: Move to Changelist ▸
        if let Some(lists) = changelists.as_deref() {
            items.push(MenuItem::separator());
            items.push(changelist_submenu(id, paths.clone(), lists));
        }
        // `785-embedded-repo-commit`: status lists an untracked folder as
        // one entry only when it is a repository of its own
        if paths.len() == 1
            && file.status.kind == FileStatusKind::Untracked
            && path.ends_with('/')
            && self
                .state
                .read(cx)
                .flags
                .bool(corvene_core::flags::ids::EMBEDDED_REPO_COMMIT)
        {
            let folder = path.clone();
            items.push(MenuItem::new(
                mac_or("Add as Submodule…", "Add as submodule…"),
                move |_, cx| Dispatcher::request_add_embedded_repository(id, folder.clone(), cx),
            ));
        }
        items.push(MenuItem::separator());
        if paths.len() == 1 {
            let is_gitignore = file.file_name() == ".gitignore";
            items.push(
                MenuItem::new(
                    mac_or(
                        "Ignore File (Add to .gitignore)",
                        "Ignore file (add to .gitignore)",
                    ),
                    {
                        let p = path.clone();
                        move |_, cx| Dispatcher::ignore_files(id, vec![p.clone()], cx)
                    },
                )
                .enabled(!is_gitignore),
            );
            let parents: Vec<&str> = {
                let mut components: Vec<&str> = path.split('/').collect();
                components.pop();
                components
            };
            if !parents.is_empty() {
                let folders: Vec<MenuItem> = (0..parents.len())
                    .map(|index| {
                        let label = format!("/{}", parents[..parents.len() - index].join("/"));
                        let pattern = label.clone();
                        MenuItem::new(label, move |_, cx| {
                            Dispatcher::ignore_files(id, vec![pattern.clone()], cx)
                        })
                    })
                    .collect();
                items.push(
                    MenuItem::submenu(
                        mac_or(
                            "Ignore Folder (Add to .gitignore)",
                            "Ignore folder (add to .gitignore)",
                        ),
                        folders,
                    )
                    .enabled(!is_gitignore),
                );
            }
            // Corvene: ignore from a nearer .gitignore, info/exclude or the
            // global excludes file (flag `ignore-file-targets`)
            if self
                .state
                .read(cx)
                .flags
                .bool(corvene_core::flags::ids::IGNORE_FILE_TARGETS)
            {
                use corvene_git::IgnoreTarget;
                let mut targets: Vec<(String, IgnoreTarget)> =
                    corvene_git::gitignore_dirs_above(&repo_path, &path)
                        .into_iter()
                        .map(|dir| (format!("{dir}/.gitignore"), IgnoreTarget::Directory(dir)))
                        .collect();
                targets.push((
                    mac_or(
                        ".git/info/exclude (This Repository Only)",
                        ".git/info/exclude (this repository only)",
                    )
                    .into(),
                    IgnoreTarget::InfoExclude,
                ));
                targets.push((
                    mac_or(
                        "Global Ignore File (All Repositories)",
                        "Global ignore file (all repositories)",
                    )
                    .into(),
                    IgnoreTarget::ExcludesFile,
                ));
                let entries: Vec<MenuItem> = targets
                    .into_iter()
                    .map(|(label, target)| {
                        let p = path.clone();
                        MenuItem::new(label, move |_, cx| {
                            Dispatcher::ignore_file_in(id, p.clone(), target.clone(), cx)
                        })
                    })
                    .collect();
                items.push(
                    MenuItem::submenu(mac_or("Ignore File In", "Ignore file in"), entries)
                        .enabled(!is_gitignore),
                );
            }
            // Corvene (`768-ignore-custom-pattern`): edit the pattern first
            if self
                .state
                .read(cx)
                .flags
                .bool(corvene_core::flags::ids::IGNORE_CUSTOM_PATTERN)
            {
                let pattern = corvene_git::escape_gitignore_pattern(&path);
                items.push(
                    MenuItem::new(
                        mac_or("Ignore with Pattern…", "Ignore with pattern…"),
                        move |_, cx| {
                            Dispatcher::show_popup(
                                Popup::IgnoreWithPattern {
                                    repo: id,
                                    pattern: pattern.clone(),
                                },
                                cx,
                            )
                        },
                    )
                    .enabled(!is_gitignore),
                );
            }
        } else {
            let ignorable: Vec<String> = paths
                .iter()
                .filter(|p| !p.ends_with(".gitignore"))
                .cloned()
                .collect();
            let enabled = !ignorable.is_empty();
            items.push(
                MenuItem::new(
                    if IS_MAC {
                        format!("Ignore {} Selected Files (Add to .gitignore)", paths.len())
                    } else {
                        format!("Ignore {} selected files (add to .gitignore)", paths.len())
                    },
                    move |_, cx| Dispatcher::ignore_files(id, ignorable.clone(), cx),
                )
                .enabled(enabled),
            );
        }
        // "Five menu items should be enough for everyone"
        let mut extensions: Vec<String> = Vec::new();
        for p in &paths {
            if let Some(ext) = Path::new(p)
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                && !extensions.contains(&ext)
            {
                extensions.push(ext);
            }
        }
        for ext in extensions.into_iter().take(5) {
            let pattern = format!("*{ext}");
            let label = match (ignore_counts, IS_MAC) {
                (true, true) => {
                    let n = extension_count(&ext);
                    format!("Ignore All {ext} Files ({n} Changed) (Add to .gitignore)")
                }
                (true, false) => {
                    let n = extension_count(&ext);
                    format!("Ignore all {ext} files ({n} changed) (add to .gitignore)")
                }
                (false, true) => format!("Ignore All {ext} Files (Add to .gitignore)"),
                (false, false) => format!("Ignore all {ext} files (add to .gitignore)"),
            };
            items.push(MenuItem::new(label, move |_, cx| {
                Dispatcher::ignore_patterns(id, vec![pattern.clone()], cx)
            }));
        }
        // `715-assume-unchanged`: tracked files only (the index must know them)
        if assume_unchanged {
            let tracked = targets.iter().all(|f| {
                matches!(
                    f.status.kind,
                    FileStatusKind::Modified | FileStatusKind::Deleted
                )
            });
            let assume = paths.clone();
            items.push(
                MenuItem::new(
                    match (paths.len(), IS_MAC) {
                        (1, _) => mac_or("Assume Unchanged", "Assume unchanged").to_string(),
                        (n, true) => format!("Assume {n} Selected Files Unchanged"),
                        (n, false) => format!("Assume {n} selected files unchanged"),
                    },
                    move |_, cx| {
                        Dispatcher::set_assume_unchanged(id, Some(assume.clone()), true, cx)
                    },
                )
                .enabled(tracked),
            );
        }
        // `1113-lfs-locks`
        let lock_items = lfs_lock_items(self.state.read(cx), id, &paths);
        if !lock_items.is_empty() {
            items.push(MenuItem::separator());
            items.extend(lock_items);
        }
        if paths.len() > 1 {
            items.push(MenuItem::separator());
            let include = paths.clone();
            items.push(MenuItem::new(
                mac_or("Include Selected Files", "Include selected files"),
                move |_, cx| Dispatcher::set_files_included(id, include.clone(), true, cx),
            ));
            let exclude = paths.clone();
            items.push(MenuItem::new(
                mac_or("Exclude Selected Files", "Exclude selected files"),
                move |_, cx| Dispatcher::set_files_included(id, exclude.clone(), false, cx),
            ));
        }
        items.push(MenuItem::separator());
        // `714-copy-diff`
        let copy_diff_item = copy_diff.then(|| {
            let paths = paths.clone();
            MenuItem::new(
                if paths.len() > 1 {
                    mac_or("Copy Diff of Selected Files", "Copy diff of selected files")
                } else {
                    mac_or("Copy Diff", "Copy diff")
                },
                move |_, cx| Dispatcher::copy_diff(id, paths.clone(), cx),
            )
        });
        if open_many && targets.len() > 1 {
            // `712-open-multiple-files`: the open items act on the selection
            let existing: Vec<PathBuf> = targets
                .iter()
                .filter(|f| f.status.kind != FileStatusKind::Deleted)
                .map(|f| repo_path.join(&f.path))
                .collect();
            items.extend(copy_items(targets));
            items.extend(copy_diff_item);
            items.push(MenuItem::separator());
            let reveal = full.clone();
            items.push(
                MenuItem::new(labels::REVEAL_IN_FILE_MANAGER, move |_, cx| {
                    Dispatcher::show_in_finder(&reveal, cx)
                })
                .enabled(!deleted),
            );
            items.extend(open_many_items(&existing, &editor_label));
        } else {
            items.extend(copy_items(targets));
            items.extend(copy_diff_item);
            items.push(MenuItem::separator());
            items.extend(open_items(full.clone(), deleted));
            // `284-open-submodule-from-changes`: the submodule as a repository
            if file.status.submodule
                && self
                    .state
                    .read(cx)
                    .flags
                    .bool(corvene_core::flags::ids::OPEN_SUBMODULE_FROM_CHANGES)
            {
                let name = self.state.read(cx).product_name().to_string();
                let label = if IS_MAC {
                    format!("Open Submodule in {name}")
                } else {
                    format!("Open submodule in {name}")
                };
                items.push(
                    MenuItem::new(label, move |_, cx| {
                        Dispatcher::open_submodule(full.clone(), cx)
                    })
                    .enabled(!deleted),
                );
            }
            // `887-file-history`: History narrowed to the file (as HEAD names it)
            if self
                .state
                .read(cx)
                .flags
                .bool(corvene_core::flags::ids::FILE_HISTORY)
            {
                let in_head = file
                    .old_path
                    .clone()
                    .filter(|_| file.status.kind == FileStatusKind::Renamed)
                    .unwrap_or(path.clone());
                items.push(MenuItem::separator());
                items.push(
                    MenuItem::new(mac_or("Show History", "Show history"), move |_, cx| {
                        Dispatcher::show_file_history(id, in_head.clone(), cx)
                    })
                    .enabled(!file.status.kind.is_new_or_untracked()),
                );
            }
            // `798-blame`: who last changed each line (`HEAD`'s copy of a
            // deleted file)
            let flags = &self.state.read(cx).flags;
            if flags.bool(corvene_core::flags::ids::BLAME) {
                if !flags.bool(corvene_core::flags::ids::FILE_HISTORY) {
                    items.push(MenuItem::separator());
                }
                let path = path.clone();
                items.push(
                    MenuItem::new("Blame", move |_, cx| {
                        Dispatcher::show_blame_for_change(id, path.clone(), cx)
                    })
                    .enabled(!file.status.kind.is_new_or_untracked()),
                );
            }
        }
        self.open_menu(items, position, window, cx);
    }

    /// A double-clicked row: GHD `onOpenItemInExternalEditor` (nothing for a
    /// deleted file, which GHD fails to open). A submodule opens as a
    /// repository with `284-open-submodule-from-changes`.
    fn open_row(&mut self, path: &str, cx: &mut Context<Self>) {
        let (target, submodule) = {
            let s = self.state.read(cx);
            let Some(id) = s.selected else { return };
            let Some(repo) = s.repository(id) else { return };
            let Some(file) = s
                .selected_state()
                .and_then(|rs| rs.status.as_deref())
                .and_then(|st| st.files.iter().find(|f| f.path == path))
            else {
                return;
            };
            let submodule = file.status.submodule
                && s.flags
                    .bool(corvene_core::flags::ids::OPEN_SUBMODULE_FROM_CHANGES);
            (
                (file.status.kind != FileStatusKind::Deleted)
                    .then(|| repo.path.join(path.trim_end_matches('/'))),
                submodule,
            )
        };
        match target {
            Some(full) if submodule => Dispatcher::open_submodule(full, cx),
            Some(full) => Dispatcher::open_in_editor(full, cx),
            None => {}
        }
    }

    /// GHD `onContextMenu` on the list itself: Discard All / Stash All.
    fn open_list_menu(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let stash_list = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::STASH_LIST);
        // `1303-partial-stash` (with whether any change is checked) and
        // `1105-clean-untracked-files`
        let (partial_stash, clean_untracked) = {
            let s = self.state.read(cx);
            (
                s.selected_state().and_then(|rs| {
                    let stash = corvene_core::stash_flows::PartialStash::of(s, rs)?;
                    let checked = rs.status.as_deref().is_some_and(|st| {
                        st.files
                            .iter()
                            .any(|f| f.selection.kind() != DiffSelectionType::None)
                    });
                    Some((stash, checked))
                }),
                s.flags
                    .bool(corvene_core::flags::ids::CLEAN_UNTRACKED_FILES),
            )
        };
        let (id, confirm, paths, openable, assume_unchanged, has_stash, can_stash, move_changes) = {
            let s = self.state.read(cx);
            let Some(id) = s.selected else { return };
            let Some(rs) = s.selected_state() else { return };
            if rs.committing {
                return;
            }
            // GHD: a branch, and no conflicts (`hasConflictedFiles` too)
            let can_stash = rs.info.as_ref().and_then(|i| i.current_branch()).is_some()
                && rs.conflict_state.is_none()
                && !rs
                    .status
                    .as_deref()
                    .is_some_and(|st| st.files.iter().any(|f| f.status.is_conflicted()));
            let paths: Vec<String> = rs
                .status
                .as_deref()
                .map(|st| st.files.iter().map(|f| f.path.clone()).collect())
                .unwrap_or_default();
            // `712-open-multiple-files`: every changed file still on disk
            let openable: Option<Vec<PathBuf>> = s
                .flags
                .bool(corvene_core::flags::ids::OPEN_MULTIPLE_FILES)
                .then(|| {
                    let root = s.repository(id).map(|r| r.path.clone()).unwrap_or_default();
                    rs.status
                        .as_deref()
                        .map(|st| {
                            st.files
                                .iter()
                                .filter(|f| f.status.kind != FileStatusKind::Deleted)
                                .map(|f| root.join(&f.path))
                                .collect()
                        })
                        .unwrap_or_default()
                });
            (
                id,
                s.settings.confirm_discard_changes,
                paths,
                openable,
                s.flags.bool(corvene_core::flags::ids::ASSUME_UNCHANGED),
                rs.desktop_stash().is_some(),
                can_stash,
                // `283-move-changes-to-worktree`: enabled with another worktree
                s.flags
                    .bool(corvene_core::flags::ids::MOVE_CHANGES_TO_WORKTREE)
                    .then_some(rs.worktrees.len() > 1),
            )
        };
        let has_changes = !paths.is_empty();
        let editor_label = self.state.read(cx).editor_label();
        let mut items = vec![
            MenuItem::new(
                if confirm {
                    mac_or("Discard All Changes…", "Discard all changes…")
                } else {
                    mac_or("Discard All Changes", "Discard all changes")
                },
                move |_, cx| Dispatcher::request_discard_changes(id, paths.clone(), cx),
            )
            .enabled(has_changes),
            MenuItem::new(
                if has_stash {
                    mac_or("Stash All Changes…", "Stash all changes…")
                } else {
                    mac_or("Stash All Changes", "Stash all changes")
                },
                move |_, cx| Dispatcher::stash_all_changes(id, cx),
            )
            .enabled(has_changes && can_stash),
        ];
        // `797-stash-list`
        if stash_list {
            items.push(
                MenuItem::new(
                    mac_or(
                        "Stash All Changes with Message…",
                        "Stash all changes with message…",
                    ),
                    move |_, cx| Dispatcher::request_stash_with_message(id, cx),
                )
                .enabled(has_changes && can_stash),
            );
        }
        if let Some((stash, checked)) = partial_stash {
            let label = mac_or("Stash Checked Changes", "Stash checked changes");
            let label = if stash == corvene_core::stash_flows::PartialStash::Blocked {
                let why = mac_or(
                    " (Restore or Discard the Stash First)",
                    " (restore or discard the stash first)",
                );
                format!("{label}{why}")
            } else {
                label.to_string()
            };
            items.push(
                MenuItem::new(label, move |_, cx| {
                    Dispatcher::stash_checked_changes(id, cx)
                })
                .enabled(checked && stash != corvene_core::stash_flows::PartialStash::Blocked),
            );
        }
        if let Some(other_worktree) = move_changes {
            items.push(
                MenuItem::new(
                    mac_or("Move Changes to Worktree…", "Move changes to worktree…"),
                    move |_, cx| Dispatcher::show_move_changes_to_worktree(id, cx),
                )
                .enabled(has_changes && can_stash && other_worktree),
            );
        }
        // `1313-changelists`
        if corvene_core::changelists::enabled(self.state.read(cx)) {
            items.push(MenuItem::separator());
            items.push(MenuItem::new(
                mac_or("New Changelist…", "New changelist…"),
                move |_, cx| {
                    Dispatcher::show_popup(
                        Popup::NewChangelist {
                            repo: id,
                            paths: Vec::new(),
                            edit: None,
                        },
                        cx,
                    )
                },
            ));
        }
        if clean_untracked {
            items.push(MenuItem::separator());
            items.push(MenuItem::new(
                mac_or("Clean Untracked Files…", "Clean untracked files…"),
                move |_, cx| Dispatcher::show_clean_untracked_files(id, cx),
            ));
        }
        if let Some(files) = openable {
            items.push(MenuItem::separator());
            items.push(open_all_in_editor_item(
                if IS_MAC {
                    format!("Open All in {editor_label}")
                } else {
                    format!("Open all in {editor_label}")
                },
                files,
            ));
        }
        if assume_unchanged {
            // `715-assume-unchanged`: the way back for files the list no
            // longer shows
            items.push(MenuItem::separator());
            items.push(MenuItem::new(
                mac_or(
                    "Stop Assuming Files Unchanged",
                    "Stop assuming files unchanged",
                ),
                move |_, cx| Dispatcher::set_assume_unchanged(id, None, false, cx),
            ));
        }
        self.open_menu(items, position, window, cx);
    }

    /// `737-commit-tag-field`: the repository's commit nonce and the trimmed
    /// tag name, when the field is shown and filled in with a valid length.
    fn tag_to_create(&self, cx: &App) -> Option<(u64, String)> {
        let s = self.state.read(cx);
        if !s.flags.bool(corvene_core::flags::ids::COMMIT_TAG_FIELD) {
            return None;
        }
        let rs = s.selected_state()?;
        if rs.commit_to_amend.is_some() {
            return None;
        }
        let name = self.tag.read(cx).value().trim().to_string();
        (!name.is_empty() && name.len() <= MAX_TAG_NAME_LENGTH).then_some((rs.commit_nonce, name))
    }

    fn do_commit(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.state.read(cx).selected else {
            return;
        };
        let summary = self.summary_or_placeholder(cx);
        let description = self.description.read(cx).value().to_string();
        let unknown = self.unknown_co_authors(cx);
        // `737-commit-tag-field`
        self.pending_tag = self
            .tag_to_create(cx)
            .map(|(nonce, name)| (id, nonce, name));
        // `732-confirm-commit-to-default-branch` (not when amending)
        let default_branch = {
            let s = self.state.read(cx);
            s.selected_state()
                .filter(|_| {
                    s.flags
                        .bool(corvene_core::flags::ids::CONFIRM_COMMIT_TO_DEFAULT_BRANCH)
                })
                .filter(|rs| rs.commit_to_amend.is_none())
                .and_then(|rs| {
                    let current = rs.info.as_ref()?.current_branch()?.name.clone();
                    (rs.default_branch.as_deref() == Some(current.as_str())).then_some(current)
                })
        };
        if let Some(branch) = default_branch {
            Dispatcher::show_popup(
                Popup::ConfirmCommitToDefaultBranch {
                    repo: id,
                    branch,
                    summary,
                    description,
                    unknown_co_authors: unknown,
                },
                cx,
            );
            return;
        }
        if !unknown.is_empty() {
            Dispatcher::show_popup(
                Popup::UnknownAuthors {
                    repo: id,
                    usernames: unknown,
                    summary,
                    description,
                },
                cx,
            );
            return;
        }
        Dispatcher::commit(id, summary, description, cx);
    }

    /// `.filtered-changes-list .header`: filter row + check-all row.
    fn header(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.ghd();
        div()
            .flex_none()
            .flex()
            .flex_col()
            .px(SPACING())
            .py(SPACING_HALF())
            .bg(t.box_alt_background)
            .border_b_1()
            .border_color(t.box_border)
            .when(self.filter_visible, |d| {
                d.child(
                    // Filter row: [Filter Options ▾][Filter…] as a joined button group
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .h(TEXT_FIELD_HEIGHT())
                        .child({
                            let active_count = self.filter_options(cx).count_active();
                            let active = active_count > 0;
                            // `buttonTextLabel`
                            let filter_label = if active {
                                format!("Filter Options ({active_count} applied)")
                            } else {
                                "Filter Options".to_string()
                            };
                            let bounds_cell = self.filter_button_bounds.clone();
                            let secondary = t.text_secondary;
                            // a `Button` tooltip: north of the button by default
                            crate::widgets::with_directed_tooltip(
                                div().id("filter-options").a11y_button(filter_label.clone()),
                                filter_label,
                                crate::widgets::TooltipDirection::North,
                            )
                            .group("filter-options")
                            .relative()
                            .h(TEXT_FIELD_HEIGHT())
                            .w(zpx(48.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(zpx(2.))
                            .border_1()
                            .border_color(t.secondary_button_border)
                            .rounded_l(BORDER_RADIUS())
                            .bg(t.secondary_button_background)
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.filter_popover_open = !this.filter_popover_open;
                                cx.notify();
                            }))
                            .child(
                                canvas(move |b, _, _| bounds_cell.set(b), |_, _, _, _| {})
                                    .absolute()
                                    .size_full(),
                            )
                            // `.active span:first-child { color: box-selected-active-background }`
                            // `.filter-button:hover { color: var(--text-secondary-color) }`
                            .child(
                                octicon(
                                    Octicon::Filter,
                                    if active {
                                        t.box_selected_active_background
                                    } else {
                                        t.secondary_button_text
                                    },
                                )
                                .when(!active, |i| {
                                    i.group_hover("filter-options", move |s| {
                                        s.text_color(secondary)
                                    })
                                }),
                            )
                            .child(
                                octicon(Octicon::TriangleDown, t.secondary_button_text)
                                    .size(zpx(12.))
                                    .group_hover("filter-options", move |s| {
                                        s.text_color(secondary)
                                    }),
                            )
                            // `.active-badge`: 5 px dot with a 1 px ring, right 18 / top 4
                            .when(active, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .top(zpx(4.))
                                        .right(zpx(18.))
                                        .p(zpx(1.))
                                        .rounded_full()
                                        .bg(t.secondary_button_background)
                                        .child(
                                            div()
                                                .size(zpx(5.))
                                                .rounded_full()
                                                .bg(t.box_selected_active_background),
                                        ),
                                )
                            })
                        })
                        .child(
                            crate::widgets::filter_text_box(
                                "changes-filter",
                                &self.filter,
                                None,
                                window,
                                cx,
                            )
                            .rounded_l(zpx(0.))
                            .border_l_0(),
                        ),
                )
            })
            .child(
                // "☑ N changed files": `.checkbox-container` 18 px tall, 5 px
                // below the filter row; the box's `margin-right: 7px`
                div()
                    .h(zpx(18.))
                    .when(self.filter_visible, |d| d.mt(SPACING_HALF()))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(zpx(7.))
                    // GHD shows the include-all box checked but disabled when there is nothing to commit.
                    .child({
                        let visible = self.visible(cx);
                        let repo_id = self.state.read(cx).selected;
                        // `1313-changelists`: the active list's files only
                        let include_all = if visible.data.checkable == 0 {
                            Some(true)
                        } else {
                            visible.include_all()
                        };
                        let disabled = visible.total() == 0 || visible.data.checkable == 0;
                        let include = include_all != Some(true);
                        let focus = self.check_all_focus.clone();
                        checkbox_tristate("check-all", include_all, disabled, cx)
                            .track_focus(&self.check_all_focus)
                            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                window.focus(&focus, cx)
                            })
                            .when_some(repo_id.filter(|_| !disabled), |d, id| {
                                // the paths are read on click: a listener
                                // holding the list would pin the status
                                let weak = cx.weak_entity();
                                d.on_click(move |_, _, cx| {
                                    if let Some(this) = weak.upgrade() {
                                        let paths = this.read(cx).visible(cx).checkable_paths();
                                        Dispatcher::set_files_included(id, paths, include, cx)
                                    }
                                })
                            })
                    })
                    .child({
                        let visible = self.visible(cx);
                        div()
                            .text_size(FONT_SIZE())
                            .truncate()
                            .child(changed_files_label(visible.len(), visible.total()))
                    })
                    .when_some(self.line_stats(cx), |d, stats| {
                        let totals = self.visible(cx).line_totals(&stats);
                        d.child(div().flex_1())
                            .child(line_stats_label(totals, None, t))
                    })
                    .children(
                        self.busy_indicator(cx)
                            .map(|spinner| div().ml_auto().flex_none().child(spinner)),
                    )
                    // `1310-file-list-tree`
                    .when(crate::file_tree_rows::tree_available(cx), |d| {
                        d.child(crate::file_tree_rows::view_toggle_button(cx))
                    })
                    // `1311-stacked-diff`: the stack button at the very end,
                    // beside the tree button when there is one
                    .when(crate::stacked_diff_view::enabled(cx), |d| {
                        let on = self.state.read(cx).settings.stacked_diff_changes;
                        let beside_tree = crate::file_tree_rows::tree_available(cx);
                        d.child(
                            crate::stacked_diff_view::stack_toggle(false, on, cx)
                                .size(zpx(18.))
                                .rounded(zpx(3.))
                                .map(|d| {
                                    if beside_tree {
                                        d.ml(zpx(6.))
                                    } else {
                                        d.ml_auto()
                                    }
                                }),
                        )
                    }),
            )
    }

    /// `708-changes-busy-indicator`: a spinner at the end of the "N changed
    /// files" row while Discard Changes runs, or once a status refresh has
    /// taken [`corvene_core::state::BUSY_INDICATOR_DELAY`] (GHD shows neither).
    fn busy_indicator(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let s = self.state.read(cx);
        let (discarding, refreshing_since) = s
            .selected_state()
            .filter(|_| {
                s.flags
                    .bool(corvene_core::flags::ids::CHANGES_BUSY_INDICATOR)
            })
            .map_or((false, None), |rs| {
                (rs.discarding, rs.refresh_started.filter(|_| rs.loading))
            });
        let color = cx.ghd().text_secondary;
        if discarding {
            return Some(crate::icons::loading("changes-busy-spinner", color));
        }
        // the dispatcher notifies once a refresh has run this long
        refreshing_since
            .filter(|since| since.elapsed() >= corvene_core::state::BUSY_INDICATOR_DELAY)
            .map(|_| crate::icons::loading("changes-busy-spinner", color))
    }

    /// Per-file line counts while flag `changes-line-counts` is on.
    fn line_stats(
        &self,
        cx: &App,
    ) -> Option<std::sync::Arc<std::collections::HashMap<String, corvene_git::LineStats>>> {
        let s = self.state.read(cx);
        if !s.flags.bool(corvene_core::flags::ids::CHANGES_LINE_COUNTS) {
            return None;
        }
        s.selected_state().map(|rs| rs.line_stats.clone())
    }

    fn branch_name(&self, cx: &App) -> SharedString {
        let rs = self.state.read(cx).selected_state();
        rs.and_then(|s| s.info.as_ref())
            .and_then(|i| match &i.tip {
                Tip::Valid { branch } => Some(branch.name.clone()),
                Tip::Unborn { name } => Some(name.clone()),
                // `782-commit-during-rebase-edit`: the branch being rebased
                // (the form only shows during a rebase with the flag)
                Tip::Detached { .. } => match rs?.conflict_state.as_ref()?.kind {
                    corvene_core::ConflictKind::Rebase {
                        ref target_branch, ..
                    } => Some(target_branch.clone()),
                    _ => None,
                },
                Tip::Unknown => None,
            })
            .unwrap_or_default()
            .into()
    }

    /// `ChangesList`: 29 px rows - checkbox, dimmed directory + bold name, status icon.
    /// `ChangesList`: 29 px rows - checkbox, dimmed directory + bold name,
    /// status icon. Virtualized with `uniform_list` (GHD uses react-virtualized).
    fn list(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.ghd();
        // `.list:focus-within .list-item.selected`: the active selection colours
        let list_focused = self.list_focus.is_focused(window);
        let s = self.state.read(cx);
        let repo_id = s.selected;
        let files = self.visible(cx);
        let empty_message = if files.is_empty() {
            no_results_message(&self.filter.read(cx).value(), &self.filter_options(cx))
        } else {
            None
        };
        let selected = self.selected_paths(cx);
        // Shift+F10 / Menu: the row the arrows move from
        let anchor_path = selected.list.last().cloned();
        let menu_anchor = self.menu_anchor.clone();
        // `1310-file-list-tree` / `1313-changelists`
        let layout = self.layout(cx);
        let anchor_row = self.current_row(&layout, cx);
        let tree_cursor = self.tree_cursor.clone();
        let group_cursor = self.group_cursor;
        let rows = layout.rows;
        // `1328-archive-entries-in-file-list` (expanding reads the
        // listing, and every refresh re-reads it)
        let splice = std::rc::Rc::new(match (repo_id, s.selected_state()) {
            (Some(_), Some(rs)) => {
                crate::archive_list_rows::splice(s, rs, ArchiveSurface::Changes, rows, |path| {
                    let visible = files.position(path)?;
                    let row = layout.row_of_visible(visible)?;
                    match layout.row(row)? {
                        RowRef::File { depth, .. } => Some((row, depth)),
                        _ => None,
                    }
                })
                .0
            }
            _ => crate::archive_list_rows::ArchiveSplice::plain(rows),
        });
        *self.archive_splice.borrow_mut() = splice.clone();
        let rows = splice.len();
        let query: SharedString = self.filter.read(cx).value().trim().to_string().into();
        let weak = cx.weak_entity();
        let list_focus = self.list_focus.clone();
        let line_stats = self.line_stats(cx);
        // `ariaLabelledBy="changes-list-check-all-label"`: the header's text
        let label = changed_files_label(files.len(), files.total());
        let whitespace_hidden = files.whitespace_hidden();
        div()
            .id("changes-list")
            .role(Role::List)
            .aria_label(label)
            .flex_1()
            .min_h(zpx(if crate::theme::short() { 58. } else { 100. }))
            .bg(t.background)
            .flex()
            .flex_col()
            .when_some(empty_message, |d, message| {
                d.child(
                    div()
                        .p(SPACING_DOUBLE())
                        .text_size(FONT_SIZE())
                        .text_color(t.text_secondary)
                        .child(message),
                )
            })
            .child(
                uniform_list("changes-list-rows", rows, move |range, _, cx| {
                    use corvene_core::file_tree::TreeRow;
                    let query = query.clone();
                    range
                        .filter_map(|ix| {
                            let ix = match splice.row(ix)? {
                                crate::archive_list_rows::DisplayRow::Base(b) => b,
                                crate::archive_list_rows::DisplayRow::Child(list, c) => {
                                    let id = repo_id?;
                                    let outer = list.outer.clone();
                                    let (focus, weak) = (list_focus.clone(), weak.clone());
                                    return Some(crate::archive_list_rows::child_row(
                                        id,
                                        ArchiveSurface::Changes,
                                        list,
                                        c,
                                        selected.list.len() == 1 && selected.contains(&list.outer),
                                        list_focused,
                                        std::rc::Rc::new(move |window, cx| {
                                            window.focus(&focus, cx);
                                            weak.update(cx, |this, _| {
                                                this.tree_cursor = None;
                                                this.group_cursor = None;
                                            })
                                            .ok();
                                            Dispatcher::select_file(id, outer.clone(), cx);
                                        }),
                                        cx,
                                    ));
                                }
                            };
                            let anchor = (anchor_row == Some(ix)).then_some(&menu_anchor);
                            match layout.row(ix)? {
                                RowRef::File { visible, depth } => {
                                    let file = files.get(visible)?;
                                    let anchor = if layout.structured() {
                                        anchor
                                    } else {
                                        (anchor_path.as_ref() == Some(&file.path))
                                            .then_some(&menu_anchor)
                                    };
                                    Some(file_row(
                                        file,
                                        line_stats
                                            .as_ref()
                                            .and_then(|m| m.get(&file.path))
                                            .copied(),
                                        selected.contains(&file.path),
                                        list_focused,
                                        &query,
                                        repo_id,
                                        weak.clone(),
                                        list_focus.clone(),
                                        anchor,
                                        depth,
                                        files.checkable(visible),
                                        cx,
                                    ))
                                }
                                RowRef::Folder { section, row } => {
                                    let sec = &layout.sections[section];
                                    let tree = sec.tree.as_ref()?;
                                    let TreeRow::Folder {
                                        path,
                                        label,
                                        depth,
                                        items,
                                        expanded,
                                    } = tree.rows.get(row)?
                                    else {
                                        return None;
                                    };
                                    let first =
                                        tree.order.get(items.start).map(|&i| sec.offset + i);
                                    let is_selected = tree_cursor
                                        .as_ref()
                                        .is_some_and(|(s, p)| *s == section && p == path)
                                        && selected.list.len() == items.len()
                                        && first
                                            .and_then(|i| files.get(i))
                                            .is_some_and(|f| selected.contains(&f.path));
                                    let checkable = sec
                                        .group
                                        .and_then(|g| files.groups()?.get(g))
                                        .is_none_or(|info| info.checkable);
                                    Some(folder_row(
                                        FolderRow {
                                            section,
                                            path,
                                            label,
                                            depth: *depth,
                                            count: items.len(),
                                            expanded: *expanded,
                                            include: layout
                                                .include
                                                .get(ix)
                                                .copied()
                                                .unwrap_or(Some(false)),
                                            checkable,
                                        },
                                        is_selected,
                                        list_focused,
                                        repo_id,
                                        weak.clone(),
                                        list_focus.clone(),
                                        anchor,
                                        cx,
                                    ))
                                }
                                RowRef::Header(section) => {
                                    let sec = &layout.sections[section];
                                    let info = files.groups()?.get(sec.group?)?;
                                    let is_selected = group_cursor == Some(section)
                                        && (info.range.is_empty()
                                            || (selected.list.len() == info.range.len()
                                                && files
                                                    .get(info.range.start)
                                                    .is_some_and(|f| selected.contains(&f.path))));
                                    Some(group_row(
                                        GroupRow {
                                            section,
                                            info,
                                            include: layout
                                                .include
                                                .get(ix)
                                                .copied()
                                                .unwrap_or(Some(false)),
                                            expanded: !sec.collapsed,
                                        },
                                        is_selected,
                                        list_focused,
                                        repo_id,
                                        weak.clone(),
                                        list_focus.clone(),
                                        anchor,
                                        cx,
                                    ))
                                }
                            }
                        })
                        .collect()
                })
                .flex_1()
                .min_h_0()
                .with_scrollbar_handle(&self.list_scroll),
            )
            .when(whitespace_hidden > 0, |d| {
                d.child(whitespace_hidden_note(whitespace_hidden, cx))
            })
    }

    /// `797-stash-list`: the Stashes section (every stash), else GHD's
    /// Stashed Changes row.
    fn stash_section(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let s = self.state.read(cx);
        if !s.flags.bool(corvene_core::flags::ids::STASH_LIST) {
            return self.stash_button(cx).map(IntoElement::into_any_element);
        }
        let id = s.selected?;
        let rs = s.selected_state()?;
        crate::stash_list::stash_list(
            id,
            rs,
            self.stash_list_collapsed,
            cx.listener(|this, _, _, cx| {
                this.stash_list_collapsed = !this.stash_list_collapsed;
                cx.notify();
            }),
            cx,
        )
    }

    /// `.stashed-changes-button`: shown when the current branch has a stash.
    fn stash_button(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        let t = cx.ghd();
        let s = self.state.read(cx);
        let id = s.selected?;
        let rs = s.selected_state()?;
        rs.stash.as_ref()?;
        let showing = rs.showing_stash;
        let hover_bg = t.box_selected_background;
        let (bg, text, icon) = if showing {
            (
                t.box_selected_active_background,
                t.box_selected_active_text,
                t.box_selected_active_text,
            )
        } else {
            (
                t.secondary_button_background,
                t.secondary_button_text,
                t.color_modified,
            )
        };
        Some(
            div()
                .id("stashed-changes-button")
                .flex_none()
                .w_full()
                .min_h(ROW_HEIGHT())
                .px(SPACING())
                .flex()
                .flex_row()
                .items_center()
                .border_t_1()
                .border_color(t.box_border)
                .bg(bg)
                .text_color(text)
                .text_size(FONT_SIZE())
                .cursor_pointer()
                .when(!showing, move |d| d.hover(move |s| s.bg(hover_bg)))
                .on_click(move |_, _, cx| Dispatcher::toggle_stash_view(id, cx))
                .child(octicon(Octicon::Stash, icon))
                .child(
                    div()
                        .flex_1()
                        .mx(SPACING_HALF())
                        .truncate()
                        .child("Stashed Changes"),
                )
                .child(octicon(Octicon::ChevronRight, text)),
        )
    }

    /// The repository rules that apply to the commit form, `None` for
    /// repositories without a GitHub remote.
    fn rules_snapshot(&self, cx: &App) -> Option<RulesSnapshot> {
        let s = self.state.read(cx);
        let id = s.selected?;
        let github = s.repository(id)?.github.as_ref()?;
        let rs = s.repo_states.get(&id)?;
        let info = rs.repo_rules.clone();
        let branch = rs
            .info
            .as_ref()
            .and_then(|i| i.current_branch())
            .map(|b| b.name.clone());
        // `formatCommitMessage`: summary, blank line, description, trailers
        let summary = self.summary_or_placeholder(cx).trim().to_string();
        let description = self.description.read(cx).value().trim().to_string();
        let message = if description.is_empty() {
            format!("{summary}\n")
        } else {
            format!("{summary}\n\n{description}\n")
        };
        // `getCoAuthorTrailers`, merged like `mergeTrailers`
        let trailers: Vec<(String, String)> = if rs.show_co_authored_by {
            rs.co_authors
                .iter()
                .filter_map(|a| a.trailer_value())
                .map(|v| ("Co-Authored-By".to_string(), v))
                .collect()
        } else {
            Vec::new()
        };
        let message = corvene_core::append_trailers(&message, &trailers);
        let message_failures = if summary.is_empty() {
            RepoRulesMetadataFailures::default()
        } else {
            failed_rules(&info.commit_message_patterns, &message)
        };
        let author_failures = rs
            .info
            .as_ref()
            .and_then(|i| i.identity.email.as_deref())
            .map(|email| failed_rules(&info.commit_author_email_patterns, email))
            .unwrap_or_default();
        let branch_failures = branch
            .as_deref()
            .map(|b| failed_rules(&info.branch_name_patterns, b))
            .unwrap_or_default();
        Some(RulesSnapshot {
            github: github.clone(),
            branch,
            unpublished: rs.ahead_behind.is_none(),
            protected: rs.current_branch_protected,
            protection_bypassed: rs.current_branch_protection_bypassed
                && s.flags
                    .bool(corvene_core::flags::ids::PROTECTED_BRANCH_BYPASS_NOTE),
            info,
            message_failures,
            message_hook: rs.commit_message_hook
                && s.flags
                    .bool(corvene_core::flags::ids::MESSAGE_RULES_DEFER_TO_HOOKS)
                && !s
                    .repository(id)
                    .is_some_and(|r| r.commit_options.skip_commit_hooks),
            author_failures,
            branch_failures,
        })
    }

    /// `hasRepoRuleFailure`
    fn has_repo_rule_failure(&self, cx: &App) -> bool {
        let Some(rules) = self.rules_snapshot(cx) else {
            return false;
        };
        rules.info.basic_commit_warning == RepoRuleEnforced::Yes
            || rules.info.signed_commits_required == RepoRuleEnforced::Yes
            || rules.info.pull_request_required == RepoRuleEnforced::Yes
            || (rules.message_failures.status() == RepoRulesMetadataStatus::Fail
                && !rules.message_hook)
            || rules.author_failures.status() == RepoRulesMetadataStatus::Fail
            || (rules.unpublished
                && (rules.info.creation_restricted == RepoRuleEnforced::Yes
                    || rules.branch_failures.status() == RepoRulesMetadataStatus::Fail))
    }

    /// `CommitWarning`: an icon centred on a rule above a centred message.
    fn commit_warning(
        &self,
        icon: Octicon,
        color: Hsla,
        message: AnyElement,
        cx: &App,
    ) -> AnyElement {
        let t = cx.ghd();
        div()
            .flex_none()
            .flex()
            .flex_col()
            .mb(SPACING())
            .bg(t.box_alt_background)
            .child(
                div()
                    .relative()
                    .h(zpx(20.))
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(zpx(10.))
                            .h(zpx(1.))
                            .bg(t.box_border),
                    )
                    .child(
                        div()
                            .px(SPACING_HALF())
                            .bg(t.box_alt_background)
                            .child(octicon(icon, color)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .justify_center()
                    .gap(zpx(3.))
                    .text_size(FONT_SIZE())
                    .text_color(t.text_secondary)
                    .child(message),
            )
            .into_any_element()
    }

    /// `showNoWriteAccess` (with changed files): "You don't have write access
    /// to <repo>. Want to create a fork?", shown before any branch warning.
    fn no_write_access_warning(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let t = cx.ghd();
        let (id, name) = {
            let s = self.state.read(cx);
            let id = s.selected?;
            let repo = s.repository(id)?;
            let github = repo.github.as_ref()?;
            let files = s.selected_state()?.status.as_deref()?.files.len();
            if github.has_write_permission()
                || files == 0
                || Dispatcher::fork_offer_blocked(s, github)
            {
                return None;
            }
            (id, repo.name())
        };
        Some(
            self.commit_warning(
                Octicon::Alert,
                t.dialog_warning,
                crate::widgets::paragraph(vec![
                    "You don't have write access to ".into(),
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.text)
                        .child(name)
                        .into_any_element()
                        .into(),
                    ". Want to ".into(),
                    crate::widgets::link_button("commit-warning-create-fork", "create a fork", cx)
                        .on_click(move |_, _, cx| Dispatcher::show_create_fork_dialog(id, cx))
                        .into_any_element()
                        .into(),
                    "?".into(),
                ])
                .justify_center()
                .into_any_element(),
                cx,
            ),
        )
    }

    /// Corvene `1212-bisect`: committing is off while the repository
    /// bisects (HEAD is a detached commit under test).
    fn bisecting(&self, cx: &App) -> bool {
        corvene_core::bisect::selected_bisect(self.state.read(cx)).is_some()
    }

    /// `1212-bisect`: the `CommitWarning` saying why committing is off.
    fn bisect_warning(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if !self.bisecting(cx) {
            return None;
        }
        let t = cx.ghd();
        let id = self.state.read(cx).selected?;
        Some(
            self.commit_warning(
                Octicon::Info,
                t.dialog_information,
                crate::widgets::paragraph(vec![
                    "You're bisecting, so committing is off until you ".into(),
                    crate::widgets::link_button("commit-warning-stop-bisect", "stop bisecting", cx)
                        .on_click(move |_, _, cx| Dispatcher::stop_bisect(id, cx))
                        .into_any_element()
                        .into(),
                    ".".into(),
                ])
                .justify_center()
                .into_any_element(),
                cx,
            ),
        )
    }

    /// Corvene addition (`730-detached-head-commit-warning`): a `CommitWarning`
    /// while HEAD is detached, since the commit lands on no branch.
    fn detached_head_warning(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let t = cx.ghd();
        let (id, sha) = {
            let s = self.state.read(cx);
            if !s
                .flags
                .bool(corvene_core::flags::ids::DETACHED_HEAD_COMMIT_WARNING)
            {
                return None;
            }
            let id = s.selected?;
            let rs = s.selected_state()?;
            // amending, or a rebase's own detached HEAD
            // (`782-commit-during-rebase-edit`)
            if rs.commit_to_amend.is_some() || rs.conflict_state.is_some() {
                return None;
            }
            match &rs.info.as_ref()?.tip {
                Tip::Detached { sha } => (id, sha.clone()),
                _ => return None,
            }
        };
        Some(
            self.commit_warning(
                Octicon::Alert,
                t.dialog_warning,
                crate::widgets::paragraph(vec![
                    "You're not on a branch (detached HEAD). This commit won't belong to any \
                     branch unless you "
                        .into(),
                    crate::widgets::link_button(
                        "commit-warning-detached-create-branch",
                        "create a branch",
                        cx,
                    )
                    .on_click(move |_, _, cx| {
                        Dispatcher::show_popup(
                            Popup::CreateBranch {
                                repo: id,
                                target_sha: Some(sha.clone()),
                                initial_name: String::new(),
                            },
                            cx,
                        )
                    })
                    .into_any_element()
                    .into(),
                    ".".into(),
                ])
                .justify_center()
                .into_any_element(),
                cx,
            ),
        )
    }

    /// `716-windows-invalid-names-warning`: included (not deleted) files whose
    /// path Windows rejects.
    fn windows_names_warning(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let t = cx.ghd();
        let s = self.state.read(cx);
        if !s
            .flags
            .bool(corvene_core::flags::ids::WINDOWS_INVALID_NAMES_WARNING)
        {
            return None;
        }
        let status = s.selected_state()?.status.as_ref()?;
        // checked once per status, not per frame
        let cached = self
            .windows_names_cache
            .borrow()
            .as_ref()
            .filter(|(seen, _)| std::ptr::eq(seen.as_ptr(), std::sync::Arc::as_ptr(status)))
            .map(|(_, names)| names.clone());
        let names = cached.unwrap_or_else(|| {
            let mut bad = status
                .files
                .iter()
                .filter(|f| {
                    f.status.kind != FileStatusKind::Deleted
                        && f.selection.kind() != DiffSelectionType::None
                })
                .filter_map(|f| {
                    corvene_core::portable_paths::windows_invalid_reason(&f.path)
                        .map(|why| (f.path.as_str(), why))
                });
            let names = bad
                .next()
                .map(|(path, why)| (1 + bad.count(), path.to_string(), why));
            *self.windows_names_cache.borrow_mut() =
                Some((std::sync::Arc::downgrade(status), names.clone()));
            names
        });
        let (count, path, why) = names?;
        let message = match count {
            1 => format!("\"{path}\" {why}, so it can't be checked out on Windows."),
            n => format!(
                "{n} files can't be checked out on Windows: \"{path}\" {why}, among others."
            ),
        };
        Some(self.commit_warning(
            Octicon::Alert,
            t.dialog_warning,
            div().child(message).into_any_element(),
            cx,
        ))
    }

    /// `783-amend-author`: the author field shows while amending.
    fn amending_with_author_field(&self, cx: &App) -> bool {
        let s = self.state.read(cx);
        s.flags.bool(corvene_core::flags::ids::AMEND_AUTHOR)
            && s.selected_state()
                .is_some_and(|rs| rs.commit_to_amend.is_some())
    }

    /// `783-amend-author`: the author field holds no valid `Name <email>`.
    fn amend_author_invalid(&self, cx: &App) -> bool {
        self.amending_with_author_field(cx)
            && corvene_git::parse_commit_author(&self.amend_author.read(cx).value()).is_err()
    }

    /// `783-amend-author`: while amending, the amended commit's author as an
    /// editable `Name <email>` and "Reset to my identity" (`--reset-author`).
    fn amend_author_field(&self, window: &Window, cx: &Context<Self>) -> Option<AnyElement> {
        if !self.amending_with_author_field(cx) {
            return None;
        }
        let t = cx.ghd();
        let s = self.state.read(cx);
        let id = s.selected?;
        let rs = s.selected_state()?;
        let identity = rs.info.as_ref().and_then(|i| {
            Some(format!(
                "{} <{}>",
                i.identity.name.as_deref()?,
                i.identity.email.as_deref()?
            ))
        });
        let reset = rs.amend_author == Some(corvene_git::CommitAuthor::ResetToCommitter);
        let invalid = self.amend_author_invalid(cx);
        let input = self.amend_author.clone();
        Some(
            div()
                .id("amend-author")
                .flex()
                .flex_col()
                .mb(SPACING_HALF())
                // "Author" and the reset link, then the field full width
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap(SPACING_HALF())
                        .mb(zpx(2.))
                        .text_size(FONT_SIZE_SM())
                        .child(
                            div()
                                .flex_none()
                                .text_color(t.text_secondary)
                                .child("Author"),
                        )
                        .when_some(identity.filter(|_| !reset), |d, identity| {
                            d.child(
                                crate::widgets::link_button(
                                    "amend-author-reset",
                                    mac_or("Reset to My Identity", "Reset to my identity"),
                                    cx,
                                )
                                .flex_none()
                                .text_size(FONT_SIZE_SM())
                                .on_click(move |_, window, cx| {
                                    input.update(cx, |s, cx| {
                                        s.set_value(identity.clone(), window, cx)
                                    });
                                    Dispatcher::set_amend_author(
                                        id,
                                        Some(corvene_git::CommitAuthor::ResetToCommitter),
                                        cx,
                                    );
                                }),
                            )
                        }),
                )
                .child(crate::widgets::text_box(
                    "amend-author-input",
                    &self.amend_author,
                    None,
                    window,
                    cx,
                ))
                .when(invalid, |d| {
                    d.child(
                        div()
                            .pt(zpx(2.))
                            .text_size(FONT_SIZE_SM())
                            .text_color(t.dialog_error)
                            .child("Enter the author as Name <email>"),
                    )
                })
                .into_any_element(),
        )
    }

    /// `734-commit-author-line`: the identity the next commit is made with.
    fn author_line(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let t = cx.ghd();
        let s = self.state.read(cx);
        if !s.flags.bool(corvene_core::flags::ids::COMMIT_AUTHOR_LINE) {
            return None;
        }
        let identity = &s.selected_state()?.info.as_ref()?.identity;
        let text = match (identity.name.as_deref(), identity.email.as_deref()) {
            (Some(name), Some(email)) => format!("Committing as {name} <{email}>"),
            (Some(name), None) => format!("Committing as {name} (no user.email)"),
            (None, Some(email)) => format!("Committing as <{email}> (no user.name)"),
            (None, None) => "No commit author configured (user.name / user.email)".to_string(),
        };
        Some(
            div()
                .id("commit-author-line")
                .mb(SPACING_HALF())
                .text_size(FONT_SIZE_SM())
                .text_color(t.text_secondary)
                .truncate()
                .ghd_tooltip(text.clone())
                .child(text)
                .into_any_element(),
        )
    }

    /// `renderBranchProtectionsRepoRulesCommitWarning`
    fn branch_protection_warning(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let t = cx.ghd();
        let rules = self.rules_snapshot(cx)?;
        let branch = rules.branch.clone()?;
        let bold = |text: String| {
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text)
                .child(text)
                .into_any_element()
        };
        let switch_link = || {
            crate::widgets::link_button("commit-warning-switch", "switch branches", cx)
                .on_click(|_, _, cx| Dispatcher::toggle_foldout(Foldout::Branch, cx))
                .into_any_element()
        };
        let rulesets_link = |label: &'static str| {
            let url = repo_rulesets_for_branch_link(Some(&rules.github), Some(&branch))
                .unwrap_or_default();
            crate::widgets::link_button("commit-warning-rulesets", label, cx)
                .on_click(move |_, _, cx| corvene_core::Dispatcher::open_url(&url, cx))
                .into_any_element()
        };
        let message = |parts: Vec<AnyElement>| {
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .justify_center()
                .gap(zpx(3.))
                .children(parts)
                .into_any_element()
        };
        if rules.protected {
            return Some(self.commit_warning(
                Octicon::Alert,
                t.dialog_warning,
                message(vec![
                    bold(branch.clone()),
                    div().child("is a protected branch. Want to").into_any_element(),
                    switch_link(),
                    div().child("?").into_any_element(),
                ]),
                cx,
            ));
        }
        // which rule warning to show: enforced ones first, then bypassable
        let publish = if rules.unpublished {
            if rules.info.creation_restricted == RepoRuleEnforced::Yes
                || rules.branch_failures.status() == RepoRulesMetadataStatus::Fail
            {
                RepoRuleEnforced::Yes
            } else if rules.info.creation_restricted == RepoRuleEnforced::Bypass
                || rules.branch_failures.status() == RepoRulesMetadataStatus::Bypass
            {
                RepoRuleEnforced::Bypass
            } else {
                RepoRuleEnforced::No
            }
        } else {
            RepoRuleEnforced::No
        };
        let statuses = [
            ("publish", publish),
            ("signing", rules.info.signed_commits_required),
            ("basic", rules.info.basic_commit_warning),
        ];
        let warning = statuses
            .iter()
            .find(|(_, e)| *e == RepoRuleEnforced::Yes)
            .or_else(|| {
                statuses
                    .iter()
                    .find(|(_, e)| *e == RepoRuleEnforced::Bypass)
            })
            .copied();
        // Corvene (`339-protected-branch-bypass-note`): a protected branch
        // that takes the user's pushes gets a note, not a block (GHD says
        // nothing)
        let Some(warning) = warning else {
            if !rules.protection_bypassed {
                return None;
            }
            return Some(self.commit_warning(
                Octicon::Alert,
                t.dialog_warning,
                message(vec![
                    bold(branch.clone()),
                    div()
                        .child("is a protected branch. Your push may bypass its rules.")
                        .into_any_element(),
                ]),
                cx,
            ));
        };
        let can_bypass = warning.1 == RepoRuleEnforced::Bypass;
        let (icon, color) = if can_bypass {
            (Octicon::Alert, t.dialog_warning)
        } else {
            (Octicon::Stop, t.dialog_error)
        };
        let bypass_tail = || {
            if can_bypass {
                vec![
                    div()
                        .child(", but you can bypass them. Proceed with caution!")
                        .into_any_element(),
                ]
            } else {
                vec![
                    div().child(". Want to").into_any_element(),
                    switch_link(),
                    div().child("?").into_any_element(),
                ]
            }
        };
        let parts = match warning.0 {
            "publish" => {
                let mut parts = vec![
                    div().child("The branch name").into_any_element(),
                    bold(branch.clone()),
                    div().child("fails").into_any_element(),
                    rulesets_link("one or more rules"),
                    div()
                        .child(format!(
                            "that {} prevent it from being published",
                            if can_bypass { "would" } else { "will" }
                        ))
                        .into_any_element(),
                ];
                parts.extend(bypass_tail());
                parts
            }
            "signing" => vec![
                rulesets_link("One or more rules"),
                div().child("apply to the branch").into_any_element(),
                bold(branch.clone()),
                div()
                    .child(format!(
                        "that require signed commits{}",
                        if can_bypass {
                            ", but you can bypass them. Proceed with caution!"
                        } else {
                            "."
                        }
                    ))
                    .into_any_element(),
                crate::widgets::link_button(
                    "commit-warning-signing-docs",
                    "Learn more about commit signing.",
                    cx,
                )
                .on_click(|_, _, cx| {
                    corvene_core::Dispatcher::open_url("https://docs.github.com/authentication/managing-commit-signature-verification/signing-commits", cx)
                })
                .into_any_element(),
            ],
            _ => {
                let mut parts = vec![
                    rulesets_link("One or more rules"),
                    div().child("apply to the branch").into_any_element(),
                    bold(branch.clone()),
                    div()
                        .child(format!(
                            "that {} prevent pushing",
                            if can_bypass { "would" } else { "will" }
                        ))
                        .into_any_element(),
                ];
                parts.extend(bypass_tail());
                parts
            }
        };
        Some(self.commit_warning(icon, color, message(parts), cx))
    }

    /// `renderRepoRuleCommitMessageFailureHint`: the stop / alert button at
    /// the end of the summary box.
    fn rule_failure_hint(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let t = cx.ghd();
        let rules = self.rules_snapshot(cx)?;
        let status = rules.message_failures.status();
        if status == RepoRulesMetadataStatus::Pass {
            return None;
        }
        let can_bypass = status == RepoRulesMetadataStatus::Bypass;
        // `340-message-rules-defer-to-hooks`: a warning, not a stop
        let hook_may_fix = !can_bypass && rules.message_hook;
        let bounds = self.rule_hint_bounds.clone();
        Some(
            div()
                .id("commit-message-failure-hint")
                .absolute()
                .right(zpx(6.))
                .top(zpx(4.))
                .cursor_pointer()
                .ghd_tooltip(if can_bypass {
                    "Warning: Commit message fails repository rules, but you can bypass them. View details."
                } else if hook_may_fix {
                    "Warning: Commit message fails repository rules, but a commit message hook may fix this. View details."
                } else {
                    "Error: Commit message fails repository rules. View details."
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    this.rule_failure_popover_open = !this.rule_failure_popover_open;
                    cx.notify();
                }))
                .child(
                    canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                        .absolute()
                        .inset_0(),
                )
                .child(if can_bypass || hook_may_fix {
                    octicon(Octicon::Alert, t.dialog_warning)
                } else {
                    octicon(Octicon::Stop, t.dialog_error)
                })
                .into_any_element(),
        )
    }

    /// `renderRuleFailurePopover` + `RepoRulesMetadataFailureList`.
    fn rule_failure_popover(&self, window: &Window, cx: &Context<Self>) -> Option<AnyElement> {
        let t = cx.ghd();
        let rules = self.rules_snapshot(cx)?;
        let branch = rules.branch.clone()?;
        let failures = &rules.message_failures;
        if failures.status() == RepoRulesMetadataStatus::Pass {
            return None;
        }
        let anchor = self.rule_hint_bounds.get();
        let viewport = window.viewport_size();
        let width = zpx(360.);
        let x =
            (anchor.origin.x + anchor.size.width + zpx(8.)).min(viewport.width - width - zpx(8.));
        let y = (anchor.origin.y - zpx(20.)).max(zpx(8.));
        let total = failures.total();
        let end_text = if failures.status() == RepoRulesMetadataStatus::Bypass {
            format!(
                ", but you can bypass {}. Proceed with caution!",
                if total == 1 { "it" } else { "them" }
            )
        } else if rules.message_hook {
            // `340-message-rules-defer-to-hooks`
            ". The repository's commit message hook runs on the commit and may fix this."
                .to_string()
        } else {
            ".".to_string()
        };
        let all_url =
            repo_rulesets_for_branch_link(Some(&rules.github), Some(&branch)).unwrap_or_default();
        let github = rules.github.clone();
        let list = |label: &'static str, items: &[corvene_core::RepoRulesMetadataFailure]| {
            if items.is_empty() {
                return None;
            }
            Some(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("{label} {}:", mac_or("Rules", "rules"))),
                    )
                    .children(items.iter().enumerate().map(|(ix, f)| {
                        let url = repo_ruleset_link(&github, f.ruleset_id);
                        div()
                            .flex()
                            .flex_row()
                            .gap(SPACING_HALF())
                            .pl(SPACING_DOUBLE())
                            .child("•")
                            .child(
                                crate::widgets::link_button(
                                    SharedString::from(format!("rule-{label}-{ix}")),
                                    f.description.clone(),
                                    cx,
                                )
                                .on_click(move |_, _, cx| {
                                    corvene_core::Dispatcher::open_url(&url, cx)
                                }),
                            )
                    })),
            )
        };
        Some(
            deferred(
                anchored().position(point(zpx(0.), zpx(0.))).child(
                    div()
                        .id("rule-failure-layer")
                        .relative()
                        .w(viewport.width)
                        .h(viewport.height)
                        .child(
                            div()
                                .id("rule-failure-overlay")
                                .absolute()
                                .inset_0()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.rule_failure_popover_open = false;
                                        cx.notify();
                                    }),
                                ),
                        )
                        .child(
                            div()
                                .id("rule-failure-popover")
                                .absolute()
                                .left(x)
                                .top(y)
                                .w(width)
                                .min_h(zpx(200.))
                                .p(SPACING())
                                .flex()
                                .flex_col()
                                .gap(SPACING())
                                .bg(t.box_background)
                                .text_color(t.text)
                                .text_size(FONT_SIZE())
                                .border_1()
                                .border_color(t.box_border)
                                .rounded(BORDER_RADIUS())
                                .shadow_lg()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .child(
                                    div()
                                        .text_size(FONT_SIZE_MD())
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(mac_or(
                                            "Commit Message Rule Failures",
                                            "Commit message rule failures",
                                        )),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .flex_wrap()
                                        .gap(zpx(3.))
                                        .child(format!(
                                            "This commit message fails {total} rule{}{end_text}",
                                            if total > 1 { "s" } else { "" }
                                        ))
                                        .child(
                                            crate::widgets::link_button(
                                                "rule-failure-all",
                                                "View all rulesets for this branch.",
                                                cx,
                                            )
                                            .on_click(
                                                move |_, _, cx| {
                                                    corvene_core::Dispatcher::open_url(&all_url, cx)
                                                },
                                            ),
                                        ),
                                )
                                .children(list("Failed", &failures.failed))
                                .children(list("Bypassed", &failures.bypassed)),
                        ),
                ),
            )
            .with_priority(12)
            .into_any_element(),
        )
    }

    /// GHD `getPlaceholderMessage` when `prepopulateCommitSummary` (exactly
    /// one file included, not the tutorial repository): "Create x", "Delete
    /// x" or "Update x", which an empty summary commits with.
    fn generated_summary(&self, cx: &App) -> Option<String> {
        let s = self.state.read(cx);
        if s.selected_repository()?.is_tutorial_repository {
            return None;
        }
        let status = s.selected_state()?.status.as_ref()?;
        let mut included = status
            .files
            .iter()
            .filter(|f| f.selection.kind() != DiffSelectionType::None);
        let file = included.next()?;
        if included.next().is_some() {
            return None;
        }
        let name = file.path.rsplit('/').next().unwrap_or(&file.path);
        Some(match file.status.kind {
            FileStatusKind::New | FileStatusKind::Untracked => format!("Create {name}"),
            FileStatusKind::Deleted => format!("Delete {name}"),
            _ => format!("Update {name}"),
        })
    }

    /// GHD `summaryOrPlaceholder`.
    fn summary_or_placeholder(&self, cx: &App) -> String {
        let summary = self.summary.read(cx).value().to_string();
        if summary.is_empty() {
            self.generated_summary(cx).unwrap_or(summary)
        } else {
            summary
        }
    }

    /// Corvene (`615-type-to-commit-summary`): a printable key without
    /// modifiers in the focused file list goes to the end of the commit
    /// summary, which takes focus.
    fn type_into_summary(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let m = &ev.keystroke.modifiers;
        if m.control || m.alt || m.platform || m.function || !self.list_focus.is_focused(window) {
            return false;
        }
        let Some(text) = ev.keystroke.key_char.clone() else {
            return false;
        };
        if text.is_empty() || text.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return false;
        }
        {
            let s = self.state.read(cx);
            if !s
                .flags
                .bool(corvene_core::flags::ids::TYPE_TO_COMMIT_SUMMARY)
            {
                return false;
            }
            // the commit form is there (not committing, no rebase to continue)
            let Some(rs) = s.selected_state() else {
                return false;
            };
            if rs.committing || rs.conflict_state.is_some() {
                return false;
            }
        }
        self.summary.update(cx, |s, cx| {
            let end = s.value().len();
            s.set_selected_range(end..end, cx);
            s.replace(text, window, cx);
        });
        let handle = self.summary_focus.clone();
        window.focus(&handle, cx);
        cx.notify();
        true
    }

    /// Corvene (`616-accept-summary-placeholder`): → in the empty summary
    /// types the generated placeholder, caret at the end.
    fn accept_summary_placeholder(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.summary_focus.is_focused(window)
            || !self.summary.read(cx).value().is_empty()
            || !self
                .state
                .read(cx)
                .flags
                .bool(corvene_core::flags::ids::ACCEPT_SUMMARY_PLACEHOLDER)
        {
            return false;
        }
        let Some(text) = self.generated_summary(cx) else {
            return false;
        };
        self.summary.update(cx, |s, cx| s.replace(text, window, cx));
        true
    }

    fn commit_disabled(&self, cx: &App) -> bool {
        let s = self.state.read(cx);
        let rs = s.selected_state();
        let any_included = self.visible(cx).data.included > 0;
        let committing = rs.map(|r| r.committing).unwrap_or(false);
        let allow_empty = s
            .selected
            .and_then(|id| s.repository(id))
            .map(|r| r.commit_options.allow_empty_commit)
            .unwrap_or(false);
        let amending = rs.is_some_and(|r| r.commit_to_amend.is_some());
        self.summary_or_placeholder(cx).trim().is_empty()
            || (!any_included && !allow_empty && !amending)
            || committing
            || self.has_repo_rule_failure(cx)
            || self.amend_author_invalid(cx)
            || self.bisecting(cx)
    }

    /// GHD `getButtonTooltip` for a disabled commit button.
    fn commit_disabled_tooltip(&self, cx: &App) -> Option<&'static str> {
        let s = self.state.read(cx);
        let rs = s.selected_state();
        let visible = self.visible(cx);
        let any_available = visible.total() > 0;
        let any_included = visible.data.included > 0;
        let allow_empty = s
            .selected
            .and_then(|id| s.repository(id))
            .is_some_and(|r| r.commit_options.allow_empty_commit);
        if self.bisecting(cx) {
            Some("Stop bisecting to commit")
        } else if self.summary_or_placeholder(cx).trim().is_empty() {
            Some("A commit summary is required to commit")
        } else if !any_included && any_available && !allow_empty {
            Some("Select one or more files to commit")
        } else if rs.is_some_and(|r| r.committing) {
            Some("Committing changes…")
        } else if self.amend_author_invalid(cx) {
            Some("Enter the author as Name <email>")
        } else {
            None
        }
    }

    /// `CommitWarning` with the information icon: "Your changes will modify
    /// your most recent commit. Stop amending to make these changes as a new commit."
    fn amend_notice(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        let t = cx.ghd();
        let s = self.state.read(cx);
        let id = s.selected?;
        s.selected_state()?.commit_to_amend.as_ref()?;
        Some(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .mb(SPACING())
                .bg(t.box_alt_background)
                .child(
                    // `.warning-icon-container`: icon centred on a rule
                    div()
                        .relative()
                        .h(zpx(20.))
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .right_0()
                                .top(zpx(10.))
                                .h(zpx(1.))
                                .bg(t.box_border),
                        )
                        .child(
                            div()
                                .px(SPACING_HALF())
                                .bg(t.box_alt_background)
                                .child(octicon(Octicon::Info, t.dialog_information)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .justify_center()
                        .text_size(FONT_SIZE())
                        .text_color(t.text_secondary)
                        .child("Your changes will modify your\u{a0}")
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.text)
                                .child("most recent commit"),
                        )
                        .child(".\u{a0}")
                        .child(
                            div()
                                .id("stop-amending")
                                .text_color(t.link)
                                .cursor_pointer()
                                .on_click(move |_, _, cx| Dispatcher::stop_amending(id, cx))
                                .child("Stop amending"),
                        )
                        .child("\u{a0}to make these changes as a new commit."),
                ),
        )
    }

    /// `#undo-commit`: "Committed N ago / summary" + Undo, after a commit.
    /// `739-undo-bar-menu`: the History commit menu's items for HEAD that
    /// make sense here.
    fn open_undo_bar_menu(
        &mut self,
        id: u64,
        sha: String,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let html_url = self
            .state
            .read(cx)
            .repository(id)
            .and_then(|r| r.github.as_ref())
            .map(|g| format!("{}/commit/{sha}", g.html_url));
        let mut items = vec![
            MenuItem::new(mac_or("Amend Commit…", "Amend commit…"), {
                let sha = sha.clone();
                move |_, cx| Dispatcher::request_start_amending(id, sha.clone(), cx)
            }),
            MenuItem::new(mac_or("Undo Commit…", "Undo commit…"), move |_, cx| {
                Dispatcher::request_undo_commit(id, cx)
            }),
            MenuItem::separator(),
            MenuItem::new("Create Tag…", {
                let sha = sha.clone();
                move |_, cx| {
                    Dispatcher::show_popup(
                        Popup::CreateTag {
                            repo: id,
                            sha: sha.clone(),
                        },
                        cx,
                    )
                }
            }),
            MenuItem::separator(),
            MenuItem::new("Copy SHA", move |_, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(sha.clone()))
            }),
        ];
        if let Some(url) = html_url {
            items.push(MenuItem::new("View on GitHub", move |_, cx| {
                Dispatcher::open_url(&url, cx)
            }));
        }
        self.open_menu(items, position, window, cx);
    }

    fn undo_bar(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        let t = cx.ghd();
        let s = self.state.read(cx);
        let id = s.selected?;
        let last = s.selected_state()?.last_commit.clone()?;
        let menu = s.flags.bool(corvene_core::flags::ids::UNDO_BAR_MENU);
        let sha = last.sha.clone();
        Some(
            div()
                .id("undo-commit-bar")
                // `739-undo-bar-menu`
                .when(menu, |d| {
                    d.on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.open_undo_bar_menu(id, sha.clone(), ev.position, window, cx)
                        }),
                    )
                })
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .mt(SPACING())
                .mx(zpx(-10.))
                .mb(zpx(-10.))
                .border_t_1()
                .border_color(t.box_border)
                .bg(t.box_alt_background)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .py(SPACING_HALF())
                        .pl(SPACING())
                        .pr(SPACING_HALF())
                        .text_size(FONT_SIZE_SM())
                        .line_height(zpx(16.5))
                        .child(
                            div()
                                .text_color(t.text_secondary)
                                .truncate()
                                .child(format!("Committed {}", relative(last.at))),
                        )
                        .child(
                            div()
                                .truncate()
                                .child(corvene_core::text_tokens::with_emoji(&last.summary)),
                        ),
                )
                .child(
                    div().p(SPACING()).pl(zpx(0.)).child(
                        crate::widgets::small_button("undo-commit", "Undo", cx)
                            .on_click(move |_, _, cx| Dispatcher::request_undo_last_commit(id, cx)),
                    ),
                ),
        )
    }

    /// `.commit-message-component`
    /// GHD `ContinueRebase` (`#continue-rebase`): while a rebase is stopped
    /// on conflicts the commit form gives way to a single "Continue rebase"
    /// button, enabled once every conflict is resolved.
    ///
    /// `782-commit-during-rebase-edit`: at an `edit` stop without conflicts
    /// the commit form stays and the button goes under it (`below_form`).
    fn continue_rebase(&self, below_form: bool, cx: &Context<Self>) -> Option<AnyElement> {
        let t = cx.ghd();
        let s = self.state.read(cx);
        let id = s.selected?;
        let rs = s.repo_states.get(&id)?;
        let conflict = rs.conflict_state.as_ref()?;
        if !matches!(conflict.kind, corvene_core::ConflictKind::Rebase { .. }) {
            return None;
        }
        let status = rs.status.as_deref()?;
        let conflicted = corvene_core::conflicted_files(status, &conflict.manual_resolutions).len();
        let edit_stop = conflicted == 0
            && status.rebase_edit_stop
            && s.flags
                .bool(corvene_core::flags::ids::COMMIT_DURING_REBASE_EDIT);
        if edit_stop != below_form {
            return None;
        }
        let untracked = status
            .files
            .iter()
            .any(|f| f.status.kind == FileStatusKind::Untracked);
        let in_progress = rs
            .mco
            .as_ref()
            .is_some_and(|m| m.step == corvene_core::McoStep::ShowProgress);
        let enabled = conflicted == 0 && !in_progress;
        let label = if in_progress {
            "Rebasing"
        } else {
            "Continue rebase"
        };
        // under the commit form the Commit button stays the primary one
        let button = match (below_form, enabled) {
            (false, _) => primary_button("continue-rebase-button", label, !enabled, cx),
            (true, true) => crate::widgets::button("continue-rebase-button", label, cx),
            (true, false) => crate::widgets::button_disabled("continue-rebase-button", label, cx),
        };
        Some(
            div()
                .id("continue-rebase")
                .flex_none()
                .flex()
                .flex_col()
                .p(SPACING())
                .when(below_form, |d| d.pt(zpx(0.)))
                .bg(t.box_alt_background)
                .when(!below_form, |d| d.border_t_1().border_color(t.box_border))
                .child(button.w_full().when(enabled, |d| {
                    d.on_click(move |_, _, cx| Dispatcher::continue_after_conflicts(id, cx))
                }))
                .when(untracked, |d| {
                    d.child(
                        div()
                            .pt(SPACING_HALF())
                            .text_align(TextAlign::Center)
                            .text_size(FONT_SIZE())
                            .child("Untracked files will be excluded"),
                    )
                })
                .into_any_element(),
        )
    }

    /// The description box's height: GHD's 80 px, or with
    /// `114-resizable-commit-message` the dragged / saved one.
    fn description_height(&self, cx: &App) -> Pixels {
        let s = self.state.read(cx);
        if !s
            .flags
            .bool(corvene_core::flags::ids::RESIZABLE_COMMIT_MESSAGE)
        {
            return zpx(80.);
        }
        self.dragged_description_height
            .unwrap_or_else(|| zpx(s.settings.commit_description_height.unwrap_or(80.)))
    }

    /// `114-resizable-commit-message`: a strip over the commit form's top
    /// edge that drags the description box taller or shorter (the file
    /// list above gives up or takes back the height); saved on release.
    fn description_resize_handle(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if crate::theme::short()
            || !self
                .state
                .read(cx)
                .flags
                .bool(corvene_core::flags::ids::RESIZABLE_COMMIT_MESSAGE)
        {
            return None;
        }
        let weak = cx.weak_entity();
        Some(
            div()
                .id("commit-message-resize-handle")
                .absolute()
                .top(zpx(-3.))
                .left_0()
                .right_0()
                .h(zpx(6.))
                .cursor(CursorStyle::ResizeUpDown)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        this.description_drag =
                            Some((event.position.y, this.description_height(cx)));
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                // the pointer is followed anywhere in the window until the
                // button is released (as `workspace.rs`'s compact split)
                .child(
                    canvas(
                        |_, _, _| {},
                        move |_, _, window, _| {
                            let moved = weak.clone();
                            window.on_mouse_event(move |event: &MouseMoveEvent, _, window, cx| {
                                let max = (window.viewport_size().height * 0.6)
                                    .max(DESCRIPTION_MIN_HEIGHT());
                                moved
                                    .update(cx, |this, cx| {
                                        if let Some((from, height)) = this.description_drag {
                                            this.dragged_description_height = Some(
                                                (height + from - event.position.y)
                                                    .clamp(DESCRIPTION_MIN_HEIGHT(), max),
                                            );
                                            cx.notify();
                                        }
                                    })
                                    .ok();
                            });
                            let released = weak.clone();
                            window.on_mouse_event(move |_: &MouseUpEvent, _, _, cx| {
                                released
                                    .update(cx, |this, cx| {
                                        if this.description_drag.take().is_some() {
                                            if let Some(height) = this.dragged_description_height {
                                                Dispatcher::update_settings(cx, |s| {
                                                    s.commit_description_height =
                                                        Some(unzoom(height))
                                                });
                                            }
                                            cx.notify();
                                        }
                                    })
                                    .ok();
                            });
                        },
                    )
                    .absolute()
                    .size_0(),
                )
                .into_any_element(),
        )
    }

    fn commit_form(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.ghd();
        // GHD: the repository rule hint, else the summary length hint
        let trailing_icon = self
            .rule_failure_hint(cx)
            .or_else(|| self.summary_length_hint(cx));
        let committing_now = self
            .state
            .read(cx)
            .selected_state()
            .is_some_and(|rs| rs.committing);
        let description_box_focused = self.description_focus.is_focused(window)
            || self.commit_options_focus.is_focused(window);
        let (is_github, co_authors_visible) = {
            let s = self.state.read(cx);
            let is_github = s
                .selected
                .and_then(|id| s.repository(id))
                .is_some_and(|r| r.github.is_some());
            let show = s.selected_state().is_some_and(|rs| rs.show_co_authored_by);
            (is_github, is_github && show)
        };
        // `737-commit-tag-field` (not while amending)
        let tag_field = {
            let s = self.state.read(cx);
            s.flags.bool(corvene_core::flags::ids::COMMIT_TAG_FIELD)
                && s.selected_state()
                    .is_some_and(|rs| rs.commit_to_amend.is_none())
        };
        // Autocompletion popup anchored at the caret's bottom-left.
        let popup = self.autocomplete.as_ref().and_then(|(field, ac)| {
            let (bounds, line_height) = match field {
                CommitField::Summary => self.summary.read(cx).cursor_layout()?,
                CommitField::Description => self.description.read(cx).cursor_layout()?,
                CommitField::CoAuthors => self.co_authors.read(cx).cursor_layout()?,
            };
            let anchor = point(bounds.origin.x, bounds.origin.y + line_height);
            let weak = cx.weak_entity();
            let on_pick: PickHandler = Rc::new(move |ix, window, cx| {
                weak.update(cx, |this, cx| this.autocomplete_insert(ix, window, cx))
                    .ok();
            });
            Some(autocompletion::popup(ac, anchor, on_pick, cx))
        });
        div()
            .id("commit-message")
            .key_context("CommitMessage")
            .on_action(cx.listener(|this, _: &Commit, _, cx| {
                if !this.commit_disabled(cx) {
                    this.do_commit(cx)
                }
            }))
            // Popup keys win over the field's own bindings (GHD `onKeyDown`).
            .capture_action(cx.listener(|this, _: &MoveUp, window, cx| {
                if this.autocomplete_move(-1, cx)
                    || (this.summary_focus.is_focused(window) && this.recall_message(1, window, cx))
                {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, window, cx| {
                if this.autocomplete_move(1, cx)
                    || (this.summary_focus.is_focused(window)
                        && this.recall_message(-1, window, cx))
                {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveRight, window, cx| {
                if this.accept_summary_placeholder(window, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, ev: &Enter, window, cx| {
                if !ev.secondary && !ev.shift && this.autocomplete_accept(window, cx) {
                    cx.stop_propagation();
                } else if this.co_authors_focus.is_focused(window) {
                    // the author field is one logical line, it only wraps
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                if this.autocomplete_accept(window, cx) {
                    cx.stop_propagation();
                }
            }))
            // Tab is bound to focus traversal before the field's indent
            .capture_action(
                cx.listener(|this, _: &crate::actions::FocusNext, window, cx| {
                    if this.autocomplete_accept(window, cx) {
                        cx.stop_propagation();
                    }
                }),
            )
            .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                if this.autocomplete.take().is_some() {
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|this, _: &SpellSuggestion0, window, cx| {
                this.apply_spell_suggestion(0, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SpellSuggestion1, window, cx| {
                this.apply_spell_suggestion(1, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SpellSuggestion2, window, cx| {
                this.apply_spell_suggestion(2, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SpellSuggestion3, window, cx| {
                this.apply_spell_suggestion(3, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SpellSuggestion4, window, cx| {
                this.apply_spell_suggestion(4, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &SpellAddToDictionary, _, cx| this.add_to_dictionary(cx)),
            )
            .on_action(
                cx.listener(|this, _: &ToggleCommitSpellcheck, _, cx| this.toggle_spellcheck(cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleCoAuthors, window, cx| {
                this.toggle_co_authors(window, cx)
            }))
            .children(popup)
            .relative()
            .children(self.description_resize_handle(cx))
            .flex_none()
            .flex()
            .flex_col()
            .p(SPACING())
            .bg(t.box_alt_background)
            // the hidden-changes warning overlaps this border
            // (`margin-bottom: -1px; z-index: 1`)
            .when(self.committing_hidden_files(cx).is_none(), |d| {
                d.border_t_1().border_color(t.box_border)
            })
            .children(
                self.amend_author_field(window, cx)
                    .or_else(|| self.author_line(cx)),
            )
            .child(
                // `.summary`: avatar + summary field
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING_HALF())
                    .mb(SPACING())
                    .child(self.avatar.clone())
                    .children(self.conventional_type_button(cx))
                    .child(
                        // the icon sits over the field's border box, outside
                        // its padding
                        div()
                            .relative()
                            .w_full()
                            .min_w_0()
                            .child(
                                text_box_with_menu(
                                    "commit-summary",
                                    &self.summary,
                                    None,
                                    Some(self.input_menu(CommitField::Summary, cx)),
                                    window,
                                    cx,
                                )
                                .relative()
                                // `readOnly={isCommitting}`: `textboxish-disabled-styles`
                                .when(committing_now, |d| d.bg(t.box_alt_background))
                                // `.with-trailing-icon input { padding-right:
                                // 20px }` (the kit's input pads 4 px itself)
                                .when(trailing_icon.is_some(), |d| d.pr(zpx(16.)))
                                .children(self.spell_overlay(CommitField::Summary, cx)),
                            )
                            .children(trailing_icon),
                    ),
            )
            .child(
                // `.description-focus-container`: textarea + action bar, with
                // the text field focus border + ring while anything inside
                // has focus (`:focus-within`)
                div()
                    .flex()
                    .flex_col()
                    .when(!co_authors_visible, |d| d.mb(SPACING()))
                    .border_1()
                    .border_color(if description_box_focused {
                        t.focus
                    } else {
                        t.box_border_contrast
                    })
                    .when(description_box_focused, |d| {
                        d.shadow(vec![BoxShadow {
                            color: t.text_field_focus_shadow,
                            offset: point(zpx(0.), zpx(0.)),
                            blur_radius: zpx(0.),
                            spread_radius: zpx(1.),
                            inset: false,
                        }])
                    })
                    .rounded_t(BORDER_RADIUS())
                    .when(!co_authors_visible, |d| d.rounded_b(BORDER_RADIUS()))
                    .bg(t.box_background)
                    // the read-only textarea and `.action-bar.disabled`
                    .when(committing_now, |d| d.bg(t.box_alt_background))
                    .overflow_hidden()
                    .child({
                        let menu = self.input_menu(CommitField::Description, cx);
                        div()
                            .relative()
                            .child(
                                Textarea::new(&self.description)
                                    .appearance(false)
                                    // `textarea { padding: 5px }`: xsmall pads
                                    // 4 px sideways and none vertically
                                    .xsmall()
                                    .pt(SPACING_HALF())
                                    .pb(SPACING_HALF())
                                    .pl(zpx(1.))
                                    .pr(zpx(1.))
                                    .text_size(FONT_SIZE())
                                    // a short window (a phone on its
                                    // side) keeps one line of it
                                    .h(if crate::theme::short() {
                                        zpx(22.)
                                    } else {
                                        self.description_height(cx)
                                    })
                                    .context_menu(move |m, window, cx| menu(m, window, cx)),
                            )
                            .children(self.spell_overlay(CommitField::Description, cx))
                    })
                    .child(
                        // `.action-bar`: add co-authors | commit options
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(SPACING_HALF())
                            // `.action-bar { padding: var(--spacing) }`
                            .p(SPACING())
                            .when(crate::theme::short(), |d| d.py(zpx(2.)))
                            .when(is_github, |d| {
                                // `.co-authors-toggle`
                                let toggle_label = if co_authors_visible {
                                    mac_or("Remove Co-Authors", "Remove co-authors")
                                } else {
                                    mac_or("Add Co-Authors", "Add co-authors")
                                };
                                let color = if co_authors_visible {
                                    t.link
                                } else {
                                    t.text_secondary
                                };
                                let hover = if co_authors_visible {
                                    t.link_hover
                                } else {
                                    t.text
                                };
                                d.child(
                                    div()
                                        .id("co-authors-toggle")
                                        .size(zpx(18.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .text_color(color)
                                        .hover(move |s| s.text_color(hover))
                                        .a11y_button(toggle_label)
                                        .ghd_tooltip(toggle_label)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.toggle_co_authors(window, cx)
                                        }))
                                        .child(octicon(Octicon::PersonAdd, color)),
                                )
                                .child(div().w(zpx(1.)).h(zpx(16.)).bg(t.box_border_contrast))
                            })
                            .child(
                                div()
                                    .id("commit-options-button")
                                    .track_focus(&self.commit_options_focus)
                                    .icon_button_label("Configure commit options")
                                    .w(zpx(18.))
                                    .h(zpx(17.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, ev: &ClickEvent, window, cx| {
                                        window.focus(&this.commit_options_focus, cx);
                                        this.open_commit_options_menu(ev.position(), window, cx)
                                    }))
                                    .child(octicon(Octicon::Gear, t.text_secondary)),
                            ),
                    ),
            )
            .when(co_authors_visible, |d| {
                d.child(
                    div()
                        .mb(SPACING())
                        .child(self.co_author_input(window, cx))
                        // `769-co-author-validation`
                        .when_some(self.co_author_hint.clone(), |d, (hint, _)| {
                            d.child(
                                div()
                                    .id("co-author-hint")
                                    .pt(zpx(2.))
                                    .text_size(FONT_SIZE_SM())
                                    .text_color(t.text_secondary)
                                    .child(hint),
                            )
                        }),
                )
            })
            .when(tag_field, |d| {
                d.child(div().mb(SPACING()).child(crate::widgets::text_box(
                    "commit-tag",
                    &self.tag,
                    None,
                    window,
                    cx,
                )))
            })
            .children(self.amend_notice(cx))
            .children(
                self.no_write_access_warning(cx)
                    .or_else(|| self.bisect_warning(cx))
                    .or_else(|| self.detached_head_warning(cx))
                    .or_else(|| self.branch_protection_warning(cx)),
            )
            .children(self.windows_names_warning(cx))
            .children(
                self.rule_failure_popover_open
                    .then(|| self.rule_failure_popover(window, cx))
                    .flatten(),
            )
            .child({
                let push_after = {
                    let s = self.state.read(cx);
                    s.flags.bool(corvene_core::flags::ids::COMMIT_AND_PUSH)
                        && s.selected
                            .and_then(|id| s.repository(id))
                            .is_some_and(|r| r.commit_options.push_after_commit)
                };
                let (included, single_list) = {
                    let visible = self.visible(cx);
                    (visible.data.included, visible.data.single_list.clone())
                };
                let (amending, committing, progress) = self
                    .state
                    .read(cx)
                    .selected_state()
                    .map(|r| {
                        (
                            r.commit_to_amend.is_some(),
                            r.committing,
                            r.commit_progress.filter(|_| r.committing),
                        )
                    })
                    .unwrap_or((false, false, None));
                // GHD `getFilesToBeCommittedButtonText`: "Commit 4 files to main"
                // (Corvene `1313-changelists`: "Commit <list> to main" when
                // the commit takes exactly one list's files)
                let files = match (&single_list, included) {
                    (Some(list), _) => format!("{list} "),
                    (None, 0) => String::new(),
                    (None, 1) => "1 file ".to_string(),
                    (None, n) => format!("{n} files "),
                };
                let files = if push_after {
                    format!("{files}and push ")
                } else {
                    files
                };
                // Corvene (`1307-commit-progress`): the staged files counted,
                // then git writing the commit
                let staging = progress
                    .filter(|p| p.phase == CommitPhase::Staging && p.total > 0)
                    .map(|p| committed_files_count(&p));
                let counting = staging.is_some();
                let writing = progress.is_some_and(|p| p.phase == CommitPhase::Writing);
                let label = if let Some(count) = staging {
                    // the count takes the place of "to <branch>" (as
                    // desktop/desktop#19679 suggests), which would not fit;
                    // a narrow sidebar cuts it short with an ellipsis
                    let verb = if amending { "Amending" } else { "Committing" };
                    div().flex().flex_row().min_w_0().child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(format!("{verb} {count}")),
                    )
                } else if amending {
                    div().flex().flex_row().child(if committing {
                        "Amending last commit"
                    } else {
                        "Amend last commit"
                    })
                } else {
                    div()
                        .flex()
                        .flex_row()
                        .gap(zpx(4.))
                        .child(if writing {
                            "Writing commit to".to_string()
                        } else if committing {
                            format!("Committing {files}to")
                        } else {
                            format!("Commit {files}to")
                        })
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(self.branch_name(cx)),
                        )
                };
                let disabled = self.commit_disabled(cx);
                let label_color = if disabled {
                    crate::widgets::faded(cx.ghd().button_text, cx.ghd().box_alt_background)
                } else {
                    cx.ghd().button_text
                };
                // GHD `<Loading />` before the text while committing
                let label = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .when(counting, |d| d.min_w_0())
                    .when(committing, |d| {
                        d.child(
                            div()
                                .when(counting, |d| d.flex_none())
                                .w(zpx(16.))
                                .h(zpx(12.))
                                .mr(zpx(5.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(crate::icons::spin(
                                    octicon(Octicon::SyncClockwise, label_color).size(zpx(12.)),
                                    "commit-spinner",
                                )),
                        )
                    })
                    .child(label);
                // `.button-component[type='submit']:focus`: the hover
                // background, also while `aria-disabled` keeps it focusable
                let focused = self.commit_button_focus.is_focused(window);
                let button = primary_button("commit", label, disabled, cx)
                    .track_focus(
                        &self
                            .commit_button_focus
                            .clone()
                            .tab_stop(crate::keyboard_nav::controls_reachable(cx)),
                    )
                    .when(focused, |d| d.bg(cx.ghd().button_hover_background))
                    // `opacity: 0.6` shows the commit form's box-alt
                    // background through, not the page's
                    .when(disabled, |d| {
                        let t = cx.ghd();
                        let fill = if focused {
                            t.button_hover_background
                        } else {
                            t.button_background
                        };
                        let border =
                            crate::widgets::faded(t.button_background, t.box_alt_background);
                        d.bg(crate::widgets::faded(fill, t.box_alt_background))
                            .border_color(border)
                            .text_color(crate::widgets::faded(t.button_text, t.box_alt_background))
                    })
                    .w_full()
                    // `1307-commit-progress`: a thin bar along the bottom
                    .when_some(progress, |d, progress| {
                        d.relative()
                            .child(commit_progress_bar(progress.fraction(), label_color))
                    })
                    // a `<button>` takes focus on mouse down, enabled or not
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            window.focus(&this.commit_button_focus, cx);
                        }),
                    )
                    .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                        let k = &ev.keystroke;
                        if (k.key == "enter" || k.key == "space")
                            && !k.modifiers.modified()
                            && !this.commit_disabled(cx)
                        {
                            cx.stop_propagation();
                            this.do_commit(cx)
                        }
                    }))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if !this.commit_disabled(cx) {
                            this.do_commit(cx)
                        }
                    }));
                // `Button tooltip`: north of the button, at once while disabled
                let button = match disabled.then(|| self.commit_disabled_tooltip(cx)).flatten() {
                    Some(tip) => crate::widgets::with_directed_tooltip_delay(
                        button,
                        tip,
                        crate::widgets::TooltipDirection::North,
                        std::time::Duration::ZERO,
                    ),
                    None => button,
                };
                // `:focus-visible`: the ring only after keyboard input
                div()
                    .relative()
                    .w_full()
                    .when(focused && window.last_input_was_keyboard(), |d| {
                        d.child(crate::widgets::focus_ring(cx))
                    })
                    .child(button)
            })
            .children(self.commit_progress(cx))
            .children(self.upstream_gone_note(cx))
            .children(self.signing_note(cx))
            .when_some(self.undo_bar(cx), |d, bar| d.child(bar))
    }

    /// GHD `renderCommitProgress`: under the commit button while a commit's
    /// hook runs (hooks interception), with Show commit progress (the
    /// Committing changes dialog) once git's output can be followed.
    fn commit_progress(&self, cx: &Context<Self>) -> Option<AnyElement> {
        use corvene_git::hooks::HookStatus;
        let t = cx.ghd();
        let s = self.state.read(cx);
        let rs = s.selected_state()?;
        let progress = rs.hook_progress.as_ref().filter(|_| rs.committing)?;
        let hook = &progress.hook_name;
        let text = match progress.status {
            HookStatus::Finished if hook == "pre-auto-gc" => "Optimizing repository…".to_string(),
            HookStatus::Started => format!("{hook} hook running…"),
            HookStatus::Finished => format!("{hook} hook finished"),
            HookStatus::Failed => format!("{hook} hook failed"),
        };
        let output = rs.commit_output.clone();
        let with_button = output.is_some();
        let border = t.secondary_button_border;
        let description = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .items_center()
            .px(SPACING())
            .border_1()
            .border_color(border)
            .when(with_button, |d| d.border_r_0())
            .rounded_l(BORDER_RADIUS())
            .when(!with_button, |d| d.rounded_r(BORDER_RADIUS()))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(text),
            );
        let button = output.map(|output| {
            let (bg, hover_bg, hover_border) = (
                t.secondary_button_background,
                t.secondary_button_hover_background,
                t.secondary_button_hover_border,
            );
            let button = div()
                .id("show-commit-progress")
                .flex_none()
                .w(zpx(30.))
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(bg)
                .border_1()
                .border_color(border)
                .rounded_r(BORDER_RADIUS())
                .cursor_pointer()
                .hover(move |s| s.bg(hover_bg).border_color(hover_border))
                .child(octicon(Octicon::Terminal, t.secondary_button_text).size(zpx(12.)))
                .on_click(move |_, _, cx| {
                    Dispatcher::show_popup(
                        corvene_core::Popup::CommitProgress {
                            output: output.clone(),
                        },
                        cx,
                    )
                });
            crate::widgets::with_directed_tooltip(
                button,
                "Show commit progress",
                crate::widgets::TooltipDirection::North,
            )
        });
        Some(
            div()
                .id("commit-progress")
                .mt(SPACING_HALF())
                .h(zpx(30.))
                .flex()
                .flex_row()
                .items_center()
                .child(description)
                .children(button)
                .into_any_element(),
        )
    }

    /// Corvene (`526-commit-signing`): under the commit button, git signs
    /// the commit (`commit.gpgsign` is on here).
    fn signing_note(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let t = cx.ghd();
        let s = self.state.read(cx);
        let signs = s.flags.bool(corvene_core::flags::ids::COMMIT_SIGNING)
            && s.selected_state().is_some_and(|rs| rs.signs_commits);
        signs.then(|| {
            div()
                .id("commit-signing-note")
                .mt(SPACING_HALF())
                .flex()
                .flex_row()
                .items_center()
                .gap(SPACING_HALF())
                .text_size(FONT_SIZE_SM())
                .text_color(t.text_secondary)
                .child(octicon(Octicon::Lock, t.text_secondary).flex_none())
                .child("Commits will be signed")
                .into_any_element()
        })
    }

    /// Corvene (`1209-current-branch-deleted-hint`): under the commit
    /// button, the current branch's upstream was deleted on the remote.
    fn upstream_gone_note(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let t = cx.ghd();
        let s = self.state.read(cx);
        let remote = Dispatcher::current_upstream_gone(s, s.selected?)?;
        let text = format!(
            "{} was deleted on {remote}. Pushing publishes it again.",
            self.branch_name(cx)
        );
        Some(
            div()
                .id("upstream-gone-note")
                .mt(SPACING_HALF())
                .flex()
                .flex_row()
                .items_start()
                .gap(SPACING_HALF())
                .text_size(FONT_SIZE_SM())
                .text_color(t.text_secondary)
                .child(octicon(Octicon::Alert, t.dialog_warning).flex_none())
                .child(div().flex_1().min_w_0().child(text))
                .into_any_element(),
        )
    }
}

impl Render for ChangesSidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some((range, author)) = self.pending_author.take() {
            self.add_co_author(range, author, window, cx);
        }
        // `getPlaceholderMessage`
        let placeholder: SharedString = self
            .generated_summary(cx)
            .map(SharedString::from)
            .unwrap_or_else(|| "Summary (required)".into());
        if self.summary_placeholder != placeholder {
            self.summary_placeholder = placeholder.clone();
            self.summary
                .update(cx, |s, cx| s.set_placeholder(placeholder, window, cx));
        }
        // `CommitMessageAvatar`: the committer's avatar next to the summary.
        let identity_email = self
            .state
            .read(cx)
            .selected_state()
            .and_then(|rs| rs.info.as_ref())
            .and_then(|i| i.identity.email.clone());
        if let Some(email) = identity_email {
            Dispatcher::request_avatar_for_email(&email, cx);
        }
        // a phone's sidebar can be lower than the commit form: the list
        // keeps two rows and the column scrolls
        let tight = crate::theme::compact(window) || crate::theme::short();
        div()
            .id("changes-sidebar")
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .when(tight, |d| d.overflow_y_scroll())
            // `1112-sparse-checkout`
            .children(self.sparse_checkout_banner(cx))
            .child(
                div()
                    .id("changes-list-container")
                    .track_focus(&self.list_focus)
                    // `1310-file-list-tree`: ← / → fold the tree's folders
                    // (`1313-changelists`: group headers fold with ← / → too)
                    .key_context(if self.layout(cx).structured() {
                        "ChangesList FileTree"
                    } else {
                        "ChangesList"
                    })
                    .on_action(cx.listener(
                        |this, _: &crate::actions::CollapseFolder, window, cx| {
                            this.tree_left(window, cx)
                        },
                    ))
                    .on_action(
                        cx.listener(|this, _: &crate::actions::ExpandFolder, window, cx| {
                            this.tree_right(window, cx)
                        }),
                    )
                    .on_action(self.menu_anchor.action_handler())
                    .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                        if this.type_into_summary(ev, window, cx) {
                            cx.stop_propagation();
                        }
                    }))
                    .on_action(
                        cx.listener(|this, _: &SelectNextFile, _, cx| this.select_relative(1, cx)),
                    )
                    .on_action(cx.listener(|this, _: &SelectPreviousFile, _, cx| {
                        this.select_relative(-1, cx)
                    }))
                    .on_action(cx.listener(|this, _: &ExtendSelectionDown, _, cx| {
                        this.extend_relative(1, cx)
                    }))
                    .on_action(cx.listener(|this, _: &ExtendSelectionUp, _, cx| {
                        this.extend_relative(-1, cx)
                    }))
                    // ⌘↑ / ⌘↓ (GHD `moveSelectionToLastSelectableRow`)
                    .on_action(cx.listener(|this, _: &SelectFirstFile, _, cx| {
                        this.select_relative(-(1 << 30), cx)
                    }))
                    .on_action(cx.listener(|this, _: &SelectLastFile, _, cx| {
                        this.select_relative(1 << 30, cx)
                    }))
                    // Space: "Select or deselect all highlighted files"
                    .on_action(cx.listener(|this, _: &ToggleIncludeSelected, _, cx| {
                        this.toggle_include_selected(cx)
                    }))
                    .on_action(cx.listener(|this, _: &DiscardSelectedFiles, _, cx| {
                        this.discard_highlighted(cx)
                    }))
                    .on_action(cx.listener(
                        |this, _: &crate::actions::CopySelectedFilePaths, _, cx| {
                            this.copy_highlighted_paths(true, cx)
                        },
                    ))
                    .on_action(cx.listener(
                        |this, _: &crate::actions::CopySelectedRelativeFilePaths, _, cx| {
                            this.copy_highlighted_paths(false, cx)
                        },
                    ))
                    .on_action(cx.listener(|this, _: &OpenSelectedFileInEditor, _, cx| {
                        if let Some(path) = this.highlighted_file_on_disk(cx) {
                            Dispatcher::open_in_editor(path, cx)
                        }
                    }))
                    .on_action(cx.listener(
                        |this, _: &OpenSelectedFileWithDefaultProgram, _, cx| {
                            if let Some(path) = this.highlighted_file_on_disk(cx) {
                                cx.open_with_system(&path)
                            }
                        },
                    ))
                    .on_action(cx.listener(|this, _: &SelectAllFiles, _, cx| {
                        let paths = this.visible(cx).paths();
                        if let Some(id) = this.state.read(cx).selected {
                            Dispatcher::select_all_files(id, paths, cx);
                        }
                    }))
                    .flex_1()
                    .map(|d| {
                        if tight {
                            d.min_h(zpx(124.))
                        } else {
                            d.min_h_0()
                        }
                    })
                    .flex()
                    .flex_col()
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                            this.open_list_menu(ev.position, window, cx)
                        }),
                    )
                    .child(self.header(window, cx))
                    .child(self.list(window, cx))
                    .children(self.stash_section(cx)),
            )
            .children(self.hidden_changes_warning(cx))
            .child(
                match self
                    .continue_rebase(false, cx)
                    .or_else(|| crate::stash_conflicts::stash_conflicts_block(&self.state, cx))
                {
                    Some(block) => block,
                    None => self.commit_form(window, cx).into_any_element(),
                },
            )
            // `782-commit-during-rebase-edit`
            .children(self.continue_rebase(true, cx))
            .children(self.context_menu.clone())
            .children(self.filter_popover(cx))
    }
}

/// `712-open-multiple-files`: "Open N Files in <editor>" / "… with Default
/// Program" for a multi-selection; disabled past [`MAX_BULK_OPEN`].
pub(crate) fn open_many_items(files: &[PathBuf], editor_label: &str) -> Vec<MenuItem> {
    let n = files.len();
    let enabled = (1..=MAX_BULK_OPEN).contains(&n);
    let default = files.to_vec();
    vec![
        open_all_in_editor_item(
            if IS_MAC {
                format!("Open {n} Files in {editor_label}")
            } else {
                format!("Open {n} files in {editor_label}")
            },
            files.to_vec(),
        ),
        MenuItem::new(
            if IS_MAC {
                format!("Open {n} Files with Default Program")
            } else {
                format!("Open {n} files with default program")
            },
            move |_, cx| {
                for f in &default {
                    cx.open_with_system(f)
                }
            },
        )
        .enabled(enabled),
    ]
}

/// An item opening every file in the editor, disabled when there are none or
/// more than [`MAX_BULK_OPEN`].
pub(crate) fn open_all_in_editor_item(label: String, files: Vec<PathBuf>) -> MenuItem {
    let enabled = (1..=MAX_BULK_OPEN).contains(&files.len());
    MenuItem::new(label, move |_, cx| {
        for f in &files {
            Dispatcher::open_in_editor(f.clone(), cx)
        }
    })
    .enabled(enabled)
}

/// `733-summary-max-length`: GitHub truncates longer summaries.
const SUMMARY_MAX_CHARS: usize = 72;

/// The byte range to drop so `text` fits in `max` chars: the chars just
/// before `caret` (the end of the edit that overflowed), or the tail when
/// the caret is too close to the start.
fn summary_overflow(text: &str, caret: usize, max: usize) -> Option<Range<usize>> {
    let excess = text.chars().count().checked_sub(max).filter(|n| *n > 0)?;
    let caret = caret.min(text.len());
    let before = text[..caret].chars().count();
    if before >= excess {
        let start = text[..caret]
            .char_indices()
            .nth(before - excess)
            .map_or(caret, |(i, _)| i);
        Some(start..caret)
    } else {
        let start = text.char_indices().nth(max).map_or(text.len(), |(i, _)| i);
        Some(start..text.len())
    }
}

/// One changes-list row (`ChangedFile`).
#[allow(clippy::too_many_arguments)]
fn file_row(
    file: &WorkingDirectoryFileChange,
    line_stats: Option<corvene_git::LineStats>,
    is_selected: bool,
    list_focused: bool,
    query: &str,
    repo_id: Option<u64>,
    weak: WeakEntity<ChangesSidebar>,
    list_focus: FocusHandle,
    menu_anchor: Option<&crate::context_menu::RowMenuAnchor>,
    // `1310-file-list-tree`: the row's depth in the tree
    depth: Option<usize>,
    // `1313-changelists`: whether the checkbox counts (no active list, or
    // the file is in it)
    checkable: bool,
    cx: &App,
) -> AnyElement {
    let t = cx.ghd();
    let hover_bg = t.list_item_hover_background;
    let (icon, color) = status_icon(file.status.kind, t);
    // `.focus-within .list-item.selected` has no status fill: the icon
    // takes the row's text colour
    let color = if is_selected && list_focused {
        t.box_selected_active_text
    } else {
        color
    };
    let path_for_select = file.path.clone();
    let path_for_toggle = file.path.clone();
    let include_value = include_value(file);
    let weak_for_order = weak.clone();
    let weak_for_left = weak.clone();
    let weak_for_open = weak.clone();
    let file_for_menu = file.clone();
    let checkbox_focus = list_focus.clone();
    // `HighlightText`: the filter's fuzzy hits in bold (`<mark>`), split
    // between the directory and the file name like `PathText`
    let names_only = corvene_core::AppState::try_global(cx).is_some_and(|s| {
        s.read(cx)
            .flags
            .bool(corvene_core::flags::ids::CHANGES_FILE_NAMES_ONLY)
    });
    let directory = crate::format::display_path(file.directory());
    let file_name = file.file_name().to_string();
    let mode = corvene_core::AppState::try_global(cx).map_or("fuzzy".to_string(), |s| {
        s.read(cx)
            .flags
            .text(corvene_core::flags::ids::CHANGES_FILTER_MATCH)
            .to_string()
    });
    let hits = corvene_core::filter::path_match(&mode, query, &file.path)
        .map(|(_, hits)| hits)
        .unwrap_or_default();
    let lfs_lock = file_lfs_lock(repo_id, &file.path, cx);
    // `1314-code-owners`: the work tree's CODEOWNERS
    let code_owners = crate::code_owners::row_owners(
        repo_id,
        corvene_core::codeowners::CodeOwnersSource::WorkTree,
        &file.path,
        cx,
    );
    // `117-file-icons`
    let file_icon = crate::file_icons::file_icons(cx).map(|icons| {
        let color = match (is_selected, list_focused) {
            (true, true) => t.box_selected_active_text,
            (true, false) => t.box_selected_text,
            _ => t.text_secondary,
        };
        div()
            .flex_none()
            .when_some(depth, |d, depth| {
                d.ml(crate::file_tree_rows::file_label_margin(depth))
            })
            .child(crate::file_icons::file_icon(&icons, &file.path, color, cx))
    });
    let has_icon = file_icon.is_some();
    let dir_len = directory.chars().count();
    let name_hits: Vec<usize> = hits
        .iter()
        .filter(|&&h| h >= dir_len)
        .map(|&h| h - dir_len)
        .collect();
    div()
        .id(SharedString::from(format!("file-{}", file.path)))
        .group("changes-row")
        .a11y_row(
            format!(
                "{}, {}{}",
                file.path,
                crate::widgets::status_label(&file.status),
                if !checkable {
                    ", not in the active changelist"
                } else {
                    match include_value {
                        Some(true) => "",
                        Some(false) => ", not included",
                        None => ", partially included",
                    }
                }
            ),
            is_selected,
        )
        .w_full()
        // `758-diff-line-height`: GHD's 29 px rows go with its 20 px diff rows
        .h(crate::diff_view::diff_line_height_setting(cx).map_or_else(ROW_HEIGHT, |h| zpx(h + 9.)))
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(SPACING_HALF())
        .px(SPACING())
        // `.list-item { border-bottom: 1px solid var(--box-border-color) }`
        // (`1310-file-list-tree`: none in the tree)
        .when(depth.is_none(), |d| {
            d.border_b_1().border_color(t.box_border)
        })
        .cursor_pointer()
        .on_mouse_down(MouseButton::Right, {
            // GHD `List.onRowMouseDown`: a right-click selects the row unless
            // it is already part of the selection, then the menu opens
            let list_focus = list_focus.clone();
            let path = file.path.clone();
            let weak_for_cursor = weak.clone();
            move |ev: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                window.focus(&list_focus, cx);
                if !is_selected && let Some(id) = repo_id {
                    weak_for_cursor
                        .update(cx, |this, _| {
                            this.tree_cursor = None;
                            this.group_cursor = None;
                        })
                        .ok();
                    Dispatcher::select_file(id, path.clone(), cx);
                }
                let position = ev.position;
                let file = file_for_menu.clone();
                weak.update(cx, |this, cx| {
                    this.open_file_menu(file, position, window, cx)
                })
                .ok();
            }
        })
        // GHD selects on mouse down (plain left button; ⌘ / ⇧ act on click)
        .on_mouse_down(MouseButton::Left, {
            let list_focus = list_focus.clone();
            let path = file.path.clone();
            let weak = weak_for_left;
            move |ev: &MouseDownEvent, window, cx| {
                let m = ev.modifiers;
                if m.secondary() || m.shift || m.control {
                    return;
                }
                window.focus(&list_focus, cx);
                weak.update(cx, |this, _| {
                    this.tree_cursor = None;
                    this.group_cursor = None;
                })
                .ok();
                if !is_selected && let Some(id) = repo_id {
                    Dispatcher::select_file(id, path.clone(), cx);
                }
            }
        })
        .when(is_selected, |d| {
            if list_focused {
                d.bg(t.box_selected_active_background)
                    .text_color(t.box_selected_active_text)
            } else {
                d.bg(t.box_selected_background)
                    .text_color(t.box_selected_text)
            }
        })
        .when(
            !(is_selected && (list_focused || crate::widgets::selection_keeps_colour_on_hover(cx))),
            move |d| d.hover(move |s| s.bg(hover_bg)),
        )
        .when_some(repo_id, move |d, id| {
            // ⌘-click toggles, ⇧-click extends (GHD `SelectionSource`)
            d.on_click(move |ev: &ClickEvent, window, cx| {
                window.focus(&list_focus, cx);
                let modifiers = ev.modifiers();
                if modifiers.secondary() {
                    Dispatcher::toggle_file_selection(id, path_for_select.clone(), cx)
                } else if modifiers.shift {
                    let order = weak_for_order
                        .upgrade()
                        .map(|this| this.read(cx).selection_order(cx))
                        .unwrap_or_default();
                    Dispatcher::extend_file_selection(id, path_for_select.clone(), order, cx)
                } else {
                    Dispatcher::select_file(id, path_for_select.clone(), cx);
                    // GHD `onChangedFileDoubleClick`
                    if ev.click_count() == 2 {
                        weak_for_open
                            .update(cx, |this, cx| this.open_row(&path_for_select, cx))
                            .ok();
                    }
                }
            })
        })
        .child(
            // `.checkbox-component`: 13 px box + 7 px margin, the label
            // starts 20 px after it (the row's 5 px gap is taken back)
            div().w(zpx(15.)).flex_none().child(
                checkbox_tristate(
                    SharedString::from(format!("include-{}", file.path)),
                    include_value,
                    !checkable,
                    cx,
                )
                .when_some(repo_id.filter(|_| checkable), move |d, id| {
                    d.on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        // the click lands inside the focusable row
                        window.focus(&checkbox_focus, cx);
                        Dispatcher::toggle_file_included(id, path_for_toggle.clone(), cx)
                    })
                }),
            ),
        )
        .children(file_icon)
        .child(if let Some(depth) = depth {
            // `1310-file-list-tree`: the name under its folder
            let (name_color, arrow_color) = match (is_selected, list_focused) {
                (true, true) => (t.box_selected_active_text, t.box_selected_active_text),
                (true, false) => (t.box_selected_text, t.box_selected_text),
                _ => (t.text_secondary, t.text),
            };
            crate::path_label::path_label_element(
                crate::file_tree_rows::tree_label(
                    &file.path,
                    file.status.kind,
                    file.old_path.as_deref(),
                ),
                name_hits.clone(),
                name_color,
                arrow_color,
            )
            .when(!has_icon, |d| {
                d.ml(crate::file_tree_rows::file_label_margin(depth))
            })
            .text_size(FONT_SIZE())
        } else if names_only {
            div()
                .flex_1()
                .min_w_0()
                .text_size(FONT_SIZE())
                .truncate()
                .child(crate::autocompletion::highlighted(&file_name, &name_hits))
        } else {
            // GHD `PathLabel`: `PathText` keeps the file name and shortens
            // the directory from its middle when the row is too narrow;
            // `.list-item.selected .dirname` inherits the row colour
            let (directory_color, arrow_color) = match (is_selected, list_focused) {
                (true, true) => (t.box_selected_active_text, t.box_selected_active_text),
                (true, false) => (t.box_selected_text, t.box_selected_text),
                _ => (t.text_secondary, t.text),
            };
            crate::path_label::path_label_element(
                crate::path_label::path_label(
                    &file.path,
                    file.status.kind,
                    file.old_path.as_deref(),
                ),
                hits,
                directory_color,
                arrow_color,
            )
            .text_size(FONT_SIZE())
        })
        .when_some(line_stats, |d, stats| {
            let colours = (is_selected && list_focused).then_some(t.box_selected_active_text);
            d.child(line_stats_label(stats, colours, t))
        })
        // `1113-lfs-locks`
        .when_some(lfs_lock.as_ref(), |d, lock| {
            d.child(lfs_lock_badge(
                lock,
                if is_selected && list_focused {
                    t.box_selected_active_text
                } else {
                    t.text_secondary
                },
            ))
        })
        // `1314-code-owners`
        .when_some(code_owners, |d, (owners, location)| {
            d.child(crate::code_owners::badge(
                file.path.clone(),
                &owners,
                &location,
                (is_selected && list_focused).then_some(t.box_selected_active_text),
                cx,
            ))
        })
        // `1328-archive-entries-in-file-list`
        .children(repo_id.and_then(|repo| {
            crate::archive_list_rows::toggle(
                repo,
                ArchiveSurface::Changes,
                &file.path,
                (is_selected && list_focused).then_some(t.box_selected_active_text),
                cx,
            )
        }))
        // `1319-trivial-change-icon`
        .child(crate::trivial_change::status_icon(
            icon,
            color,
            is_selected && list_focused,
            repo_id.and_then(|repo| {
                crate::trivial_change::lookup(
                    repo,
                    crate::trivial_change::FileList::Changes,
                    &file.path,
                    cx,
                )
            }),
            cx,
        ))
        // `621-context-menu-buttons`
        .when(crate::context_menu::row_menu_buttons(cx), |d| {
            d.child(
                crate::context_menu::row_menu_button(
                    "row-menu",
                    "changes-row",
                    is_selected,
                    if is_selected && list_focused {
                        t.box_selected_active_text
                    } else {
                        t.text_secondary
                    },
                )
                .ml(SPACING_HALF()),
            )
        })
        .when_some(menu_anchor, |d, anchor| d.child(anchor.track()))
        .into_any_element()
}

/// `1313-changelists`: what a group header row shows.
struct GroupRow<'a> {
    section: usize,
    info: &'a GroupInfo,
    /// `getCheckAllValue` of its visible files.
    include: Option<bool>,
    expanded: bool,
}

/// `1313-changelists`: a changelist's header row (or the "Changes" group of
/// files in no list): a chevron, a checkbox for its files, the name, the
/// count, an Active badge and the row menu. Clicking it selects its files.
#[allow(clippy::too_many_arguments)]
fn group_row(
    group: GroupRow<'_>,
    is_selected: bool,
    list_focused: bool,
    repo_id: Option<u64>,
    weak: WeakEntity<ChangesSidebar>,
    list_focus: FocusHandle,
    menu_anchor: Option<&crate::context_menu::RowMenuAnchor>,
    cx: &App,
) -> AnyElement {
    let t = cx.ghd();
    let info = group.info;
    let (section, id) = (group.section, info.id);
    let hover_bg = t.list_item_hover_background;
    let (text, muted) = match (is_selected, list_focused) {
        (true, true) => (t.box_selected_active_text, t.box_selected_active_text),
        (true, false) => (t.box_selected_text, t.box_selected_text),
        _ => (t.text, t.text_secondary),
    };
    let toggle = {
        let weak = weak.clone();
        let expanded = group.expanded;
        move |_: &mut Window, cx: &mut App| {
            weak.update(cx, |this, cx| this.set_group_collapsed(id, expanded, cx))
                .ok();
        }
    };
    let toggle_on_double = toggle.clone();
    let count = info.total;
    let shown = info.range.len();
    let count_label = if shown == count {
        count.to_string()
    } else {
        format!("{shown} of {count}")
    };
    div()
        .id(SharedString::from(format!("changelist-{id}")))
        .group("changes-row")
        .a11y_row(
            format!(
                "{}, changelist, {count} {}{}{}",
                info.name,
                if count == 1 { "file" } else { "files" },
                if info.active { ", active" } else { "" },
                if !info.checkable {
                    ", not the active changelist"
                } else {
                    match group.include {
                        Some(true) => "",
                        Some(false) => ", not included",
                        None => ", partially included",
                    }
                }
            ),
            is_selected,
        )
        .w_full()
        .h(ROW_HEIGHT())
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(SPACING_HALF())
        .px(SPACING())
        .bg(t.box_alt_background)
        .border_b_1()
        .border_color(t.box_border)
        .cursor_pointer()
        .on_mouse_down(MouseButton::Right, {
            let list_focus = list_focus.clone();
            let weak = weak.clone();
            move |ev: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                window.focus(&list_focus, cx);
                let position = ev.position;
                weak.update(cx, |this, cx| {
                    if !is_selected
                        && let Some(row) =
                            this.layout(cx).sections.get(section).and_then(|s| s.header)
                    {
                        this.select_row(row, cx);
                    }
                    this.open_group_menu(section, position, window, cx)
                })
                .ok();
            }
        })
        .on_mouse_down(MouseButton::Left, {
            let list_focus = list_focus.clone();
            let weak = weak.clone();
            move |ev: &MouseDownEvent, window, cx| {
                let m = ev.modifiers;
                if m.secondary() || m.shift || m.control {
                    return;
                }
                window.focus(&list_focus, cx);
                if !is_selected {
                    weak.update(cx, |this, cx| {
                        if let Some(row) =
                            this.layout(cx).sections.get(section).and_then(|s| s.header)
                        {
                            this.select_row(row, cx);
                        }
                    })
                    .ok();
                }
            }
        })
        .when(is_selected, |d| {
            if list_focused {
                d.bg(t.box_selected_active_background)
                    .text_color(t.box_selected_active_text)
            } else {
                d.bg(t.box_selected_background)
                    .text_color(t.box_selected_text)
            }
        })
        .when(
            !(is_selected && (list_focused || crate::widgets::selection_keeps_colour_on_hover(cx))),
            move |d| d.hover(move |s| s.bg(hover_bg)),
        )
        .when_some(repo_id, {
            let weak = weak.clone();
            let list_focus = list_focus.clone();
            move |d, repo| {
                d.on_click(move |ev: &ClickEvent, window, cx| {
                    window.focus(&list_focus, cx);
                    let Some(this) = weak.upgrade() else { return };
                    let files = this.read(cx).section_files(section, cx);
                    let modifiers = ev.modifiers();
                    if modifiers.secondary() {
                        this.update(cx, |this, _| {
                            this.tree_cursor = None;
                            this.group_cursor = None;
                        });
                        Dispatcher::toggle_files_selection(repo, files, cx)
                    } else if modifiers.shift {
                        let order = this.read(cx).selection_order(cx);
                        if let Some(last) = files.last() {
                            Dispatcher::extend_file_selection(repo, last.clone(), order, cx)
                        }
                    } else if ev.click_count() == 2 {
                        toggle_on_double(window, cx);
                    }
                })
            }
        })
        // the chevron: 16 px, collapses or expands the group
        .child(
            div()
                .id(SharedString::from(format!("changelist-toggle-{id}")))
                .flex_none()
                .size(zpx(16.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(zpx(3.))
                .hover(move |s| s.bg(t.box_selected_background))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    toggle(window, cx)
                })
                .child(octicon(
                    if group.expanded {
                        Octicon::ChevronDown
                    } else {
                        Octicon::ChevronRight
                    },
                    muted,
                )),
        )
        .child(
            div().w(zpx(15.)).flex_none().child(
                checkbox_tristate(
                    SharedString::from(format!("include-changelist-{id}")),
                    group.include,
                    !info.checkable || shown == 0,
                    cx,
                )
                .when_some(repo_id.filter(|_| info.checkable && shown > 0), {
                    let weak = weak.clone();
                    let include = group.include != Some(true);
                    let list_focus = list_focus.clone();
                    move |d, repo| {
                        d.on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            window.focus(&list_focus, cx);
                            let files = weak
                                .upgrade()
                                .map(|this| this.read(cx).section_files(section, cx))
                                .unwrap_or_default();
                            Dispatcher::set_files_included(repo, files, include, cx)
                        })
                    }
                }),
            ),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(FONT_SIZE())
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(text)
                .child(info.name.clone()),
        )
        .child(
            div()
                .flex_none()
                .text_size(FONT_SIZE_SM())
                .text_color(muted)
                .child(count_label),
        )
        .when(info.active, |d| {
            // the Active badge: a small pill in the selection colour
            d.child(
                div()
                    .flex_none()
                    .px(zpx(5.))
                    .rounded(zpx(8.))
                    .bg(t.box_selected_active_background)
                    .text_size(zpx(10.))
                    .text_color(t.box_selected_active_text)
                    .child("Active"),
            )
        })
        .when(!info.description.is_empty(), |d| {
            d.child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(FONT_SIZE_SM())
                    .text_color(muted)
                    .child(info.description.clone()),
            )
        })
        .child(div().flex_1())
        // `621-context-menu-buttons`
        .when(crate::context_menu::row_menu_buttons(cx), |d| {
            d.child(
                crate::context_menu::row_menu_button("row-menu", "changes-row", is_selected, muted)
                    .ml(SPACING_HALF()),
            )
        })
        .when_some(menu_anchor, |d, anchor| d.child(anchor.track()))
        .into_any_element()
}

/// `1313-changelists`: the Move to Changelist ▸ submenu for `paths`: New
/// Changelist…, then each list (ticked when every path is in it) and No
/// Changelist.
fn changelist_submenu(id: u64, paths: Vec<String>, lists: &Changelists) -> MenuItem {
    let new_paths = paths.clone();
    let mut items = vec![MenuItem::new(
        mac_or("New Changelist…", "New changelist…"),
        move |_, cx| {
            Dispatcher::show_popup(
                Popup::NewChangelist {
                    repo: id,
                    paths: new_paths.clone(),
                    edit: None,
                },
                cx,
            )
        },
    )];
    if !lists.is_empty() {
        items.push(MenuItem::separator());
        for list in &lists.lists {
            let all_in = paths.iter().all(|p| list.paths.contains(p));
            let (list_id, to_move) = (list.id, paths.clone());
            items.push(MenuItem::checkbox(
                list.name.clone(),
                all_in,
                move |_, cx| Dispatcher::move_to_changelist(id, to_move.clone(), Some(list_id), cx),
            ));
        }
        items.push(MenuItem::separator());
        let ungrouped = paths.iter().all(|p| lists.group_of(p) == UNGROUPED);
        let to_move = paths.clone();
        items.push(MenuItem::checkbox(
            mac_or("No Changelist", "No changelist"),
            ungrouped,
            move |_, cx| Dispatcher::move_to_changelist(id, to_move.clone(), None, cx),
        ));
    }
    MenuItem::submenu(mac_or("Move to Changelist", "Move to changelist"), items)
}

/// `1310-file-list-tree`: what a folder row shows.
struct FolderRow<'a> {
    /// `1313-changelists`: the section (group) the folder is in.
    section: usize,
    path: &'a str,
    label: &'a str,
    depth: usize,
    count: usize,
    expanded: bool,
    include: Option<bool>,
    /// `1313-changelists`: whether the checkbox counts.
    checkable: bool,
}

/// `1310-file-list-tree`: one folder row of the changes tree.
#[allow(clippy::too_many_arguments)]
fn folder_row(
    folder: FolderRow<'_>,
    is_selected: bool,
    list_focused: bool,
    repo_id: Option<u64>,
    weak: WeakEntity<ChangesSidebar>,
    list_focus: FocusHandle,
    menu_anchor: Option<&crate::context_menu::RowMenuAnchor>,
    cx: &App,
) -> AnyElement {
    let t = cx.ghd();
    let hover_bg = t.list_item_hover_background;
    let path = folder.path.to_string();
    let section = folder.section;
    let checkable = folder.checkable;
    let text_color = match (is_selected, list_focused) {
        (true, true) => Some(t.box_selected_active_text),
        (true, false) => Some(t.box_selected_text),
        _ => None,
    };
    let toggle = {
        let weak = weak.clone();
        let path = path.clone();
        let expanded = folder.expanded;
        move |_: &mut Window, cx: &mut App| {
            weak.update(cx, |this, cx| {
                this.set_collapsed(vec![path.clone()], expanded, cx)
            })
            .ok();
        }
    };
    let toggle_on_double = toggle.clone();
    div()
        .id(SharedString::from(format!("folder-{section}-{path}")))
        .group("changes-row")
        .a11y_row(
            format!(
                "{path}, folder, {} {}{}",
                folder.count,
                if folder.count == 1 { "file" } else { "files" },
                match folder.include {
                    Some(true) => "",
                    Some(false) => ", not included",
                    None => ", partially included",
                }
            ),
            is_selected,
        )
        .w_full()
        .h(crate::diff_view::diff_line_height_setting(cx).map_or_else(ROW_HEIGHT, |h| zpx(h + 9.)))
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(SPACING_HALF())
        .px(SPACING())
        .cursor_pointer()
        .on_mouse_down(MouseButton::Right, {
            let list_focus = list_focus.clone();
            let weak = weak.clone();
            let path = path.clone();
            move |ev: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                window.focus(&list_focus, cx);
                let position = ev.position;
                weak.update(cx, |this, cx| {
                    if !is_selected {
                        this.select_folder(section, &path, cx);
                    }
                    this.open_folder_menu(section, path.clone(), position, window, cx)
                })
                .ok();
            }
        })
        .on_mouse_down(MouseButton::Left, {
            let list_focus = list_focus.clone();
            let weak = weak.clone();
            let path = path.clone();
            move |ev: &MouseDownEvent, window, cx| {
                let m = ev.modifiers;
                if m.secondary() || m.shift || m.control {
                    return;
                }
                window.focus(&list_focus, cx);
                if !is_selected {
                    weak.update(cx, |this, cx| this.select_folder(section, &path, cx))
                        .ok();
                }
            }
        })
        .when(is_selected, |d| {
            if list_focused {
                d.bg(t.box_selected_active_background)
                    .text_color(t.box_selected_active_text)
            } else {
                d.bg(t.box_selected_background)
                    .text_color(t.box_selected_text)
            }
        })
        .when(
            !(is_selected && (list_focused || crate::widgets::selection_keeps_colour_on_hover(cx))),
            move |d| d.hover(move |s| s.bg(hover_bg)),
        )
        .when_some(repo_id, {
            let weak = weak.clone();
            let path = path.clone();
            let list_focus = list_focus.clone();
            move |d, id| {
                d.on_click(move |ev: &ClickEvent, window, cx| {
                    window.focus(&list_focus, cx);
                    let Some(this) = weak.upgrade() else { return };
                    let files = this.read(cx).folder_files(section, &path, cx);
                    let modifiers = ev.modifiers();
                    if modifiers.secondary() {
                        this.update(cx, |this, _| {
                            this.tree_cursor = None;
                            this.group_cursor = None;
                        });
                        Dispatcher::toggle_files_selection(id, files, cx)
                    } else if modifiers.shift {
                        let order = this.read(cx).selection_order(cx);
                        if let Some(last) = files.last() {
                            Dispatcher::extend_file_selection(id, last.clone(), order, cx)
                        }
                    } else if ev.click_count() == 2 {
                        toggle_on_double(window, cx);
                    }
                })
            }
        })
        .child(
            div().w(zpx(15.)).flex_none().child(
                checkbox_tristate(
                    SharedString::from(format!("include-folder-{section}-{path}")),
                    folder.include,
                    !checkable,
                    cx,
                )
                .when_some(repo_id.filter(|_| checkable), {
                    let weak = weak.clone();
                    let path = path.clone();
                    let include = folder.include != Some(true);
                    let list_focus = list_focus.clone();
                    move |d, id| {
                        d.on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            window.focus(&list_focus, cx);
                            let files = weak
                                .upgrade()
                                .map(|this| this.read(cx).folder_files(section, &path, cx))
                                .unwrap_or_default();
                            Dispatcher::set_files_included(id, files, include, cx)
                        })
                    }
                }),
            ),
        )
        .children(crate::file_tree_rows::folder_cells(
            folder.label,
            folder.depth,
            folder.count,
            folder.expanded,
            text_color,
            toggle,
            cx,
        ))
        // `621-context-menu-buttons`
        .when(crate::context_menu::row_menu_buttons(cx), |d| {
            d.child(
                crate::context_menu::row_menu_button(
                    "row-menu",
                    "changes-row",
                    is_selected,
                    text_color.unwrap_or(t.text_secondary),
                )
                .ml(SPACING_HALF()),
            )
        })
        .when_some(menu_anchor, |d, anchor| d.child(anchor.track()))
        .into_any_element()
}

/// Corvene (`1113-lfs-locks`): the file menus' lock items for `paths`, in
/// a repository whose LFS server has locks: Lock File(s) for the unlocked
/// ones, Unlock File(s) for the user's own (or, when the server cannot
/// tell, any), and for one file someone else holds a disabled "Locked by
/// X" with Force Unlock… for admins. Empty without locking.
pub(crate) fn lfs_lock_items(state: &AppState, id: u64, paths: &[String]) -> Vec<MenuItem> {
    let Some(rs) = state
        .repo_states
        .get(&id)
        .filter(|_| state.flags.bool(corvene_core::flags::ids::LFS_LOCKS))
        .filter(|rs| rs.lfs_locking())
    else {
        return Vec::new();
    };
    let paths: Vec<String> = paths
        .iter()
        .filter(|p| !p.ends_with('/'))
        .cloned()
        .collect();
    let unlocked: Vec<String> = paths
        .iter()
        .filter(|p| rs.lfs_lock(p).is_none())
        .cloned()
        .collect();
    let mine: Vec<String> = paths
        .iter()
        .filter(|p| rs.lfs_lock(p).is_some_and(|l| l.ours != Some(false)))
        .cloned()
        .collect();
    let mut items = Vec::new();
    let count_label = |verb: &str, n: usize| match (n, IS_MAC) {
        (1, true) => format!("{verb} File"),
        (1, false) => format!("{verb} file"),
        (n, true) => format!("{verb} {n} Files"),
        (n, false) => format!("{verb} {n} files"),
    };
    if !unlocked.is_empty() {
        let label = count_label("Lock", unlocked.len());
        items.push(MenuItem::new(label, move |_, cx| {
            Dispatcher::lock_lfs_files(id, unlocked.clone(), cx)
        }));
    }
    if !mine.is_empty() {
        let label = count_label("Unlock", mine.len());
        items.push(MenuItem::new(label, move |_, cx| {
            Dispatcher::unlock_lfs_files(id, mine.clone(), false, cx)
        }));
    }
    if let [path] = paths.as_slice()
        && let Some(lock) = rs.lfs_lock(path).filter(|l| l.ours == Some(false))
    {
        items.push(MenuItem::new(format!("Locked by {}", lock.owner), |_, _| {}).enabled(false));
        if state.is_repository_admin(id) {
            let path = path.clone();
            items.push(MenuItem::new(
                mac_or("Force Unlock…", "Force unlock…"),
                move |_, cx| Dispatcher::request_force_unlock(id, path.clone(), cx),
            ));
        }
    }
    items
}

/// Corvene (`1113-lfs-locks`): the lock on `path` in repository `id`.
pub(crate) fn file_lfs_lock(id: Option<u64>, path: &str, cx: &App) -> Option<corvene_git::LfsLock> {
    let s = AppState::try_global(cx)?.read(cx);
    if !s.flags.bool(corvene_core::flags::ids::LFS_LOCKS) {
        return None;
    }
    s.repo_states.get(&id?)?.lfs_lock(path).cloned()
}

/// Corvene (`1113-lfs-locks`): a lock and its holder ("You" for the
/// user's own) at the end of a file row.
pub(crate) fn lfs_lock_badge(lock: &corvene_git::LfsLock, colour: Hsla) -> Div {
    let holder = match lock.ours {
        Some(true) => "You".to_string(),
        _ => lock.owner.clone(),
    };
    div()
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(zpx(2.))
        .max_w(zpx(120.))
        .text_size(FONT_SIZE_SM())
        .text_color(colour)
        .child(octicon(Octicon::Lock, colour).flex_none())
        .child(div().min_w_0().truncate().child(holder))
}

/// "+N -M" in the added / deleted colours (Corvene addition, flag
/// `changes-line-counts`); `colour` overrides both, for a focused selected
/// row. Zero parts are left out.
fn line_stats_label(
    stats: corvene_git::LineStats,
    colour: Option<Hsla>,
    t: &crate::theme::GhdTheme,
) -> Div {
    div()
        .flex_none()
        .flex()
        .flex_row()
        .gap(SPACING_HALF())
        .text_size(FONT_SIZE_SM())
        .when(stats.added > 0, |d| {
            d.child(
                div()
                    .text_color(colour.unwrap_or(t.color_new))
                    .child(format!("+{}", crate::format::format_count(stats.added))),
            )
        })
        .when(stats.deleted > 0, |d| {
            d.child(
                div()
                    .text_color(colour.unwrap_or(t.color_deleted))
                    .child(format!("-{}", crate::format::format_count(stats.deleted))),
            )
        })
}

/// `793-hide-whitespace-only-files`: "N files hidden (whitespace only)"
/// under a file list.
pub(crate) fn whitespace_hidden_note(count: usize, cx: &App) -> Div {
    let t = cx.ghd();
    let text = if count == 1 {
        "1 file hidden (whitespace only)".to_string()
    } else {
        format!(
            "{} files hidden (whitespace only)",
            crate::format::format_count(count as u64)
        )
    };
    div()
        .flex_none()
        .px(SPACING())
        .py(SPACING_HALF())
        .border_t_1()
        .border_color(t.box_border)
        .text_size(FONT_SIZE_SM())
        .text_color(t.text_secondary)
        .child(text)
}

/// `1307-commit-progress`: "1,234 of 5,000 files" staged so far.
fn committed_files_count(progress: &CommitProgress) -> String {
    format!(
        "{} of {} {}",
        crate::format::format_count(progress.staged as u64),
        crate::format::format_count(progress.total as u64),
        if progress.total == 1 { "file" } else { "files" }
    )
}

/// `1307-commit-progress`: the bar along the bottom of the commit button,
/// `fraction` of it filled in the label's colour over a fainter track; it
/// stays clear of the rounded corners and the text's descenders.
fn commit_progress_bar(fraction: f32, color: Hsla) -> Div {
    div()
        .absolute()
        .left(BORDER_RADIUS())
        .right(BORDER_RADIUS())
        .bottom(zpx(1.))
        .h(zpx(2.))
        .rounded(zpx(1.))
        .bg(color.opacity(0.3))
        .child(
            div()
                .h_full()
                .w(gpui_kit::relative(fraction.clamp(0., 1.)))
                .rounded(zpx(1.))
                .bg(color),
        )
}

/// "N changed files", or GHD's "3 of 10 changed files" while a filter hides
/// some.
fn changed_files_label(visible: usize, total: usize) -> String {
    let prefix = if visible != total {
        format!("{} of ", crate::format::format_count(visible as u64))
    } else {
        String::new()
    };
    if total == 1 {
        format!("{prefix}1 changed file")
    } else {
        format!(
            "{prefix}{} changed files",
            crate::format::format_count(total as u64)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::summary_overflow;

    #[test]
    fn summary_overflow_drops_the_end_of_the_edit() {
        assert_eq!(summary_overflow("abc", 3, 3), None);
        // typed "X" at byte 1 of "abc" with max 3
        assert_eq!(summary_overflow("aXbc", 2, 3), Some(1..2));
        // pasted "XYZ" at the end
        assert_eq!(summary_overflow("abXYZ", 5, 3), Some(3..5));
        // multi-byte chars
        assert_eq!(summary_overflow("éé€", 7, 2), Some(4..7));
        // caret at the start: cut the tail
        assert_eq!(summary_overflow("abcd", 0, 3), Some(3..4));
    }
}
