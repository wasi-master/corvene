//! "Clone a Repository" (`ui/clone-repository/clone-repository.tsx`):
//! tabs GitHub.com | GitHub Enterprise | URL. The account tabs list the
//! account's repositories (`clone-github-repository.tsx`,
//! `cloneable-repository-filter-list.tsx`, grouped as in
//! `group-repositories.ts`); the URL tab takes any clone URL. The local path
//! is validated like `validateEmptyFolder` and errors show as a `DialogError`.
//! Cloning first resolves the input through the API
//! (`corvene_core::clone_info`, GHD `resolveCloneInfo`) for the canonical URL,
//! the default branch and the "couldn't find that repository" error. With
//! several accounts for a tab, the `AccountPicker` (`ui/account-picker.tsx`,
//! `styles/ui/_account-picker.scss`, a `PopoverDropdown`) picks which one
//! lists repositories. The list and the picker live in
//! `crate::cloneable_repositories` (the blank slate shows them too).
//!
//! Deviation (`233-shallow-clone`): a "Shallow clone" checkbox under the
//! local path clones with `--depth 1`.
//!
//! Deviation (`230-clone-path-includes-owner`): the path derived from the
//! URL can be `<clone dir>/<owner>/<name>` rather than `<clone dir>/<name>`.
//!
//! Deviation (`231-clone-offer-add-existing`): when the local path is
//! already a Git repository, "Add this repository instead?" adds it (GHD
//! only says the folder contains files).
//!
//! Deviation (`232-clone-local-sources`): the URL tab takes a local
//! folder (`/path`, `~/path`) or `file://` URL, see `corvene_core::clone_info`.
//!
//! Deviation (`235-clone-failure-keeps-input`): a failed clone reopens
//! this dialog on the URL tab with its URL and local path, git's error in
//! the banner (GHD closes it and shows an error dialog, so both are typed
//! again).
//!
//! Deviation (`224-alias-when-adding`): an optional Alias field under the
//! local path names the clone in the repository list.
//!
//! Deviation (`226-clone-prefers-ssh`): repositories picked from the list
//! and `owner/name` shorthands can clone over SSH.
//!
//! Deviation (`293-clone-multiple`): ⌘/⇧-click picks several repositories
//! on the GitHub tabs; the local path then names a parent folder, every
//! destination in it is checked first, and `Dispatcher::clone_repositories`
//! clones them one after the other (GHD clones one at a time).
//!
//! Deviation (`330-clone-path-validation`): the local path expands a leading
//! `~/`, must be absolute, and a missing folder must be creatable (its
//! nearest existing parent is a writable folder); GHD only checks that the
//! path is not a non-empty folder, so `~/x` clones into a folder named `~`
//! next to the app and other bad paths fail inside git.
//!
//! Deviation (`1122-visual-studio-clone-folder`, desktop/desktop#23103):
//! with Visual Studio installed, a link under the local path switches the
//! folder the path is derived in to Visual Studio's `source\repos` and
//! back; GHD has only the last clone folder and Choose….

use std::path::{Path, PathBuf};
use std::rc::Rc;

use corvene_core::{Account, AppState, Dispatcher, GitHubRepository, Popup};
use gpui_kit::component::input::InputState;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::cloneable_repositories::{
    AccountPickerState, CloneRow, ListStyle, account_picker, account_popover, group_rows, no_items,
    refresh_button, repository_list,
};
use crate::context_menu::mac_or;
use crate::dialog::{DialogFrame, GroupButtonSpec, OkCancelButtonGroup, dialog_loading_framed};
use crate::icons::{Octicon, octicon};
use crate::tab_bar::TabModel;
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::{button, dialog_error_banner, labeled, text_box};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    DotCom,
    Enterprise,
    Url,
}

#[allow(non_snake_case)]
fn LIST_HEIGHT() -> Pixels {
    zpx(290.)
}
pub struct CloneRepositoryDialog {
    state: Entity<AppState>,
    tab: Tab,
    url: Entity<InputState>,
    path: Entity<InputState>,
    filter: Entity<InputState>,
    /// The user edited the path by hand; stop deriving it from the URL.
    path_edited: bool,
    last_derived: String,
    initial_path: String,
    /// `clone_url` of the list item picked on an account tab.
    selected_repo: Option<String>,
    /// The selected tab's focus ring while it holds the dialog's first focus.
    tab_focus_visible: bool,
    /// `validateEmptyFolder` result for the current path.
    path_error: Option<&'static str>,
    /// `resolveCloneInfo` is running (GHD `loading`).
    resolving: bool,
    /// `resolveCloneInfo` failed: the repository was not found.
    resolve_error: Option<&'static str>,
    /// `235-clone-failure-keeps-input`: git's error from the failed clone
    /// this dialog reopened after, with that clone's URL and path; shown
    /// until either field differs.
    clone_error: Option<(SharedString, String, String)>,
    /// `224-alias-when-adding`.
    alias: Entity<InputState>,
    /// `selectedAccount` per GitHub tab, as `(endpoint, login)`.
    dotcom_account: Option<(String, String)>,
    enterprise_account: Option<(String, String)>,
    /// `AccountPicker` popover.
    picker: AccountPickerState,
    /// `233-shallow-clone`: "Shallow clone" is ticked.
    shallow: bool,
    /// `231-clone-offer-add-existing`: the local path is already a
    /// repository, offered to be added instead.
    existing_repo: Option<PathBuf>,
    /// `293-clone-multiple`: the clone URLs picked with ⌘/⇧-click, in
    /// order; two or more make the local path a parent folder.
    multi: Vec<String>,
    /// `293-clone-multiple`: the destinations that already exist and are
    /// not empty (or that two picks share).
    multi_conflicts: Vec<String>,
    /// `1122-visual-studio-clone-folder`: Visual Studio's clone folder,
    /// once found (Windows with Visual Studio installed).
    visual_studio: Option<PathBuf>,
    /// The path is derived in [`Self::visual_studio`] instead of the clone
    /// folder.
    in_visual_studio: bool,
}

thread_local! {
    static PREFILL_FILTER: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

/// Corvene (`225-clone-prefills-filter`): the next clone dialog opens with
/// this text in its GitHub tabs' filter box (the repository list's filter,
/// when Add › Clone Repository… is picked from it).
pub fn prefill_filter(text: String) {
    PREFILL_FILTER.with(|f| *f.borrow_mut() = Some(text));
}

impl CloneRepositoryDialog {
    pub fn new(
        state: Entity<AppState>,
        initial_url: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let clone_dir = state.read(cx).clone_dir();
        let url = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("URL or username/repository");
            if let Some(u) = &initial_url {
                s = s.default_value(u.clone());
            }
            s
        });
        let path = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("repository path")
                .default_value(clone_dir.display().to_string())
        });
        let prefill = PREFILL_FILTER.with(|f| f.borrow_mut().take());
        let filter = cx.new(|cx| {
            let input = InputState::new(window, cx).placeholder("Filter your repositories");
            match prefill {
                Some(text) => input.default_value(text),
                None => input,
            }
        });
        let picker = AccountPickerState::new(window, cx);
        cx.observe(&picker.filter, |_, _, cx| cx.notify()).detach();
        cx.observe_in(&url, window, |this, _, window, cx| {
            this.resolve_error = None;
            this.forget_stale_clone_error(cx);
            this.derive_path(window, cx);
            this.validate(cx);
            cx.notify()
        })
        .detach();
        cx.observe(&path, |this, _, cx| {
            this.resolve_error = None;
            this.forget_stale_clone_error(cx);
            this.validate(cx);
            cx.notify()
        })
        .detach();
        cx.observe_in(&filter, window, |this, _, window, cx| {
            this.filter_changed(window, cx);
            cx.notify()
        })
        .detach();
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let tab = if initial_url.is_some() {
            Tab::Url
        } else {
            Tab::DotCom
        };
        let handle = if tab == Tab::Url {
            url.read(cx).focus_handle(cx)
        } else {
            filter.read(cx).focus_handle(cx)
        };
        window.focus(&handle, cx);
        let initial_path = clone_dir.display().to_string();
        let this = Self {
            state,
            tab,
            url,
            path,
            filter,
            path_edited: false,
            last_derived: initial_path.clone(),
            initial_path,
            selected_repo: None,
            tab_focus_visible: true,
            path_error: None,
            resolving: false,
            resolve_error: None,
            dotcom_account: None,
            enterprise_account: None,
            picker,
            shallow: false,
            existing_repo: None,
            multi: Vec::new(),
            multi_conflicts: Vec::new(),
            clone_error: None,
            alias: cx.new(|cx| InputState::new(window, cx).placeholder("optional")),
            visual_studio: None,
            in_visual_studio: false,
        };
        this.ensure_loaded(cx);
        this.find_visual_studio(cx);
        this
    }

    /// `1122-visual-studio-clone-folder`: `vswhere` runs off the main thread.
    fn find_visual_studio(&self, cx: &mut Context<Self>) {
        if !cfg!(windows)
            || !self
                .state
                .read(cx)
                .flags
                .bool(corvene_core::flags::ids::VISUAL_STUDIO_CLONE_FOLDER)
        {
            return;
        }
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async { corvene_platform::paths::visual_studio_clone_dir() })
                .await;
            this.update(cx, |d, cx| {
                d.visual_studio = found;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The folder the path is derived in: the clone folder, or Visual
    /// Studio's while picked.
    fn base_dir(&self, cx: &App) -> PathBuf {
        match &self.visual_studio {
            Some(dir) if self.in_visual_studio => dir.clone(),
            _ => self.state.read(cx).clone_dir(),
        }
    }

    /// Switch the path between the clone folder and Visual Studio's,
    /// deriving it again (a path typed by hand included).
    fn toggle_visual_studio(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.in_visual_studio = !self.in_visual_studio;
        self.path_edited = false;
        self.last_derived = self.path.read(cx).value().to_string();
        self.derive_path(window, cx);
        self.validate(cx);
        cx.notify();
    }

    /// `235-clone-failure-keeps-input`: the URL tab with the failed clone's
    /// URL and local path, and git's error above.
    pub fn retry(
        state: Entity<AppState>,
        url: String,
        path: PathBuf,
        error: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let url_value = url.clone();
        let mut this = Self::new(state, Some(url), window, cx);
        let path = path.display().to_string();
        this.path_edited = true;
        this.path
            .update(cx, |s, cx| s.set_value(path.clone(), window, cx));
        this.clone_error = Some((error.into(), url_value, path));
        this
    }

    fn forget_stale_clone_error(&mut self, cx: &App) {
        let url = self.url.read(cx).value();
        let path = self.path.read(cx).value();
        if self
            .clone_error
            .as_ref()
            .is_some_and(|(_, u, p)| u.as_str() != url.as_ref() || p.as_str() != path.as_ref())
        {
            self.clone_error = None;
        }
    }

    /// GHD `getAccountsForTab`.
    fn accounts_for_tab(&self, cx: &App) -> Vec<Account> {
        let s = self.state.read(cx);
        match self.tab {
            Tab::DotCom => s
                .accounts
                .iter()
                .filter(|a| a.is_dotcom())
                .cloned()
                .collect(),
            Tab::Enterprise => s
                .accounts
                .iter()
                .filter(|a| !a.is_dotcom())
                .cloned()
                .collect(),
            Tab::Url => Vec::new(),
        }
    }

    /// GHD `getAccountForTab`: the picked account while it is still signed
    /// in, else the tab's first account (`228-clone-default-account`: the
    /// default account).
    fn account(&self, cx: &App) -> Option<Account> {
        let accounts = self.accounts_for_tab(cx);
        let picked = match self.tab {
            Tab::DotCom => self.dotcom_account.as_ref(),
            Tab::Enterprise => self.enterprise_account.as_ref(),
            Tab::Url => None,
        };
        picked
            .and_then(|(endpoint, login)| {
                accounts
                    .iter()
                    .find(|a| a.endpoint == *endpoint && a.login == *login)
            })
            .or_else(|| crate::cloneable_repositories::default_account(&accounts, cx))
            .cloned()
    }

    /// `onSelectedAccountChanged`.
    fn pick_account(&mut self, account: &Account, cx: &mut Context<Self>) {
        let key = Some((account.endpoint.clone(), account.login.clone()));
        match self.tab {
            Tab::DotCom => self.dotcom_account = key,
            Tab::Enterprise => self.enterprise_account = key,
            Tab::Url => {}
        }
        self.picker.open = false;
        self.selected_repo = None;
        self.multi.clear();
        self.ensure_loaded(cx);
        cx.notify();
    }

    /// Fetch the account's repositories once per dialog tab.
    fn ensure_loaded(&self, cx: &mut Context<Self>) {
        let Some(account) = self.account(cx) else {
            return;
        };
        let loaded = {
            let s = self.state.read(cx);
            s.api_repositories.contains_key(&account.key())
                || s.api_repositories_loading.contains(&account.key())
        };
        if !loaded {
            Dispatcher::load_api_repositories(account, cx);
        }
    }

    fn set_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = tab;
        self.resolve_error = None;
        if !self.multi.is_empty() {
            self.multi.clear();
            self.derive_path(window, cx);
            self.validate(cx);
        }
        self.picker.open = false;
        let handle = if tab == Tab::Url {
            self.url.read(cx).focus_handle(cx)
        } else {
            self.filter.read(cx).focus_handle(cx)
        };
        window.focus(&handle, cx);
        self.ensure_loaded(cx);
        cx.notify();
    }

    /// GHD keeps `<clone dir>/<repo name>` in sync with the URL until edited.
    fn derive_path(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.path.read(cx).value().to_string();
        if current != self.last_derived {
            self.path_edited = true;
        }
        if self.path_edited {
            return;
        }
        let base = self.base_dir(cx);
        let url = self.url.read(cx).value().to_string();
        let with_owner = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::CLONE_PATH_INCLUDES_OWNER);
        let local_ok = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::CLONE_LOCAL_SOURCES);
        // `293-clone-multiple`: the parent folder of the clones
        let derived = if self.multi_active() {
            base.clone()
        } else {
            derived_path(&base, &url, with_owner, local_ok)
        }
        .display()
        .to_string();
        if derived != current {
            self.last_derived = derived.clone();
            self.path
                .update(cx, |s, cx| s.set_value(derived, window, cx));
        }
    }

    /// `validatePath`: nothing to say while the path is still the default and
    /// no URL was entered; otherwise `validateEmptyFolder`.
    fn validate(&mut self, cx: &App) {
        // `293-clone-multiple`: every destination in the parent folder
        if self.multi_active() {
            self.existing_repo = None;
            let parent = self.path.read(cx).value().trim().to_string();
            self.path_error = parent
                .is_empty()
                .then_some("Choose the folder to clone the repositories into.");
            let targets = self.multi_targets(cx);
            let mut seen: Vec<&PathBuf> = Vec::new();
            self.multi_conflicts = targets
                .iter()
                .filter_map(|(_, path)| {
                    let shared = seen.contains(&path);
                    seen.push(path);
                    (shared || validate_empty_folder(path).is_some())
                        .then(|| path.display().to_string())
                })
                .collect();
            self.multi_conflicts.dedup();
            return;
        }
        self.multi_conflicts.clear();
        let raw = self.path.read(cx).value().trim().to_string();
        let url_empty = self.url.read(cx).value().trim().is_empty();
        let strict = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::CLONE_PATH_VALIDATION);
        let (expanded, strict_error) = if strict {
            validate_clone_path(&raw, &dirs_home())
        } else {
            (PathBuf::from(&raw), None)
        };
        self.path_error = if raw == self.initial_path && url_empty {
            None
        } else if strict {
            strict_error
        } else {
            validate_empty_folder(Path::new(&raw))
        };
        let path = expanded.to_string_lossy().to_string();
        let offer_add = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::CLONE_OFFER_ADD_EXISTING);
        self.existing_repo = (offer_add
            && self.path_error.is_some()
            && corvene_git::root_path_status(Path::new(&path))
                == corvene_git::PathStatus::Repository)
            .then(|| PathBuf::from(&path));
    }

    fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Clone".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await
                && let Some(p) = paths.into_iter().next()
            {
                this.update_in(cx, |d, window, cx| {
                    d.path_edited = true;
                    d.path
                        .update(cx, |s, cx| s.set_value(p.display().to_string(), window, cx));
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn clone_target(&self, cx: &App) -> Option<(String, PathBuf)> {
        if self.tab != Tab::Url && self.selected_repo.is_none() {
            return None;
        }
        let local_ok = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::CLONE_LOCAL_SOURCES);
        let url = clone_source(&self.url.read(cx).value(), local_ok)?;
        let path = self.path.read(cx).value().trim().to_string();
        if path.is_empty() {
            return None;
        }
        // `330-clone-path-validation`: `~/` means the home folder
        if self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::CLONE_PATH_VALIDATION)
        {
            return Some((url, expand_home(&path, &dirs_home())));
        }
        Some((url, PathBuf::from(path)))
    }

    /// `293-clone-multiple`: two or more repositories are picked.
    fn multi_active(&self) -> bool {
        self.multi.len() > 1
    }

    /// `293-clone-multiple`: each picked clone URL with its folder in the
    /// parent folder (`<parent>/<name>`, `<parent>/<owner>/<name>` with
    /// `230-clone-path-includes-owner`).
    fn multi_targets(&self, cx: &App) -> Vec<(String, PathBuf)> {
        let raw = self.path.read(cx).value().trim().to_string();
        if raw.is_empty() {
            return Vec::new();
        }
        let flags = &self.state.read(cx).flags;
        let parent = if flags.bool(corvene_core::flags::ids::CLONE_PATH_VALIDATION) {
            expand_home(&raw, &dirs_home())
        } else {
            PathBuf::from(raw)
        };
        let with_owner = flags.bool(corvene_core::flags::ids::CLONE_PATH_INCLUDES_OWNER);
        self.multi
            .iter()
            // (a list's clone URL; a local one in a test fixture names its folder too)
            .map(|url| (url.clone(), derived_path(&parent, url, with_owner, true)))
            .collect()
    }

    /// A list row clicked: with `293-clone-multiple`, ⌘-click (Ctrl-click
    /// elsewhere) adds or removes a repository and ⇧-click picks the range
    /// from the last plain pick; a plain click picks one (GHD).
    fn click_repository(
        &mut self,
        repo: &GitHubRepository,
        modifiers: Modifiers,
        rows: &[CloneRow],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let multiple = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::CLONE_MULTIPLE);
        let url = repo.clone_url.clone();
        if multiple && modifiers.secondary() {
            let mut set: Vec<String> = if self.multi.is_empty() {
                self.selected_repo.iter().cloned().collect()
            } else {
                self.multi.clone()
            };
            match set.iter().position(|u| *u == url) {
                Some(ix) => {
                    set.remove(ix);
                }
                None => set.push(url),
            }
            self.set_multi(set, window, cx);
        } else if multiple
            && modifiers.shift
            && let Some(anchor) = self.selected_repo.clone()
        {
            let urls: Vec<&String> = rows
                .iter()
                .filter_map(|r| match r {
                    CloneRow::Item(repo, _) => Some(&repo.clone_url),
                    CloneRow::Header(_) => None,
                })
                .collect();
            let (Some(a), Some(b)) = (
                urls.iter().position(|u| **u == anchor),
                urls.iter().position(|u| **u == url),
            ) else {
                return self.click_repository(repo, Modifiers::default(), rows, window, cx);
            };
            let mut set: Vec<String> = urls[a.min(b)..=a.max(b)]
                .iter()
                .map(|u| (*u).clone())
                .collect();
            // the anchor stays the first pick
            if let Some(ix) = set.iter().position(|u| *u == anchor) {
                let anchor = set.remove(ix);
                set.insert(0, anchor);
            }
            self.set_multi(set, window, cx);
        } else {
            let was_multi = self.multi_active();
            self.multi.clear();
            self.select_repository(repo, window, cx);
            if was_multi {
                self.derive_path(window, cx);
                self.validate(cx);
            }
        }
    }

    /// `293-clone-multiple`: the picks after a ⌘/⇧-click; one pick is the
    /// plain single selection again.
    fn set_multi(&mut self, set: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        match set.len() {
            0 => {
                self.multi.clear();
                self.selected_repo = None;
                self.url.update(cx, |s, cx| s.set_value("", window, cx));
            }
            1 => {
                self.multi.clear();
                let url = set[0].clone();
                self.selected_repo = Some(url.clone());
                self.url.update(cx, |s, cx| s.set_value(url, window, cx));
            }
            _ => {
                if self.selected_repo.as_ref().is_none_or(|s| !set.contains(s)) {
                    self.selected_repo = set.first().cloned();
                }
                self.multi = set;
            }
        }
        self.derive_path(window, cx);
        self.validate(cx);
        cx.notify();
    }

    /// GHD `clone`: `resolveCloneInfo`, then clone or show the error.
    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.path_error.is_some() || self.resolving {
            return;
        }
        // `293-clone-multiple`: one clone after the other
        if self.multi_active() {
            if !self.multi_conflicts.is_empty() {
                return;
            }
            let flags = &self.state.read(cx).flags;
            let prefer_ssh = flags.bool(corvene_core::flags::ids::CLONE_PREFERS_SSH);
            let depth =
                (self.shallow && flags.bool(corvene_core::flags::ids::SHALLOW_CLONE)).then_some(1);
            let items = self.multi_targets(cx);
            // `527-multiple-accounts`: the clones use the list's account
            if let Some(account) = self.account(cx) {
                for (_, path) in &items {
                    Dispatcher::account_when_added(path, account.login.clone(), cx);
                }
            }
            Dispatcher::clone_repositories(items, prefer_ssh, depth, cx);
            return;
        }
        let Some((_, path)) = self.clone_target(cx) else {
            return;
        };
        let input = self.url.read(cx).value().trim().to_string();
        // `226-clone-prefers-ssh`: list picks and shorthands clone over SSH;
        // an http(s) URL typed on the URL tab keeps its protocol
        let prefer_ssh = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::CLONE_PREFERS_SSH)
            && (self.tab != Tab::Url
                || !(input.starts_with("https://") || input.starts_with("http://")));
        self.resolving = true;
        self.resolve_error = None;
        cx.notify();
        let weak = cx.weak_entity();
        let shallow = self.shallow
            && self
                .state
                .read(cx)
                .flags
                .bool(corvene_core::flags::ids::SHALLOW_CLONE);
        let depth = shallow.then_some(1);
        if self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::ALIAS_WHEN_ADDING)
        {
            let alias = self.alias.read(cx).value().to_string();
            Dispatcher::alias_when_added(&path, alias, cx);
        }
        // `527-multiple-accounts`: a pick from an account's list uses that
        // account, a URL the account that could see it
        let list_account = self.account(cx).map(|a| a.login);
        Dispatcher::resolve_clone_info(
            input,
            prefer_ssh,
            move |result, cx| match result {
                Ok(info) => {
                    if let Some(login) = list_account.or(info.account) {
                        Dispatcher::account_when_added(&path, login, cx);
                    }
                    Dispatcher::clone_repository_with(
                        info.url,
                        path,
                        info.default_branch,
                        depth,
                        cx,
                    )
                }
                Err(message) => {
                    let Some(cx) = cx.gpui_app() else { return };
                    weak.update(cx, |this, cx| {
                        this.resolving = false;
                        this.resolve_error = Some(message);
                        cx.notify();
                    })
                    .ok();
                }
            },
            cx,
        );
    }

    /// `onSelectionChanged { kind: 'filter' }`: a filter that hides the
    /// selection selects the first match instead.
    fn filter_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account) = self.account(cx) else {
            return;
        };
        let query = self.filter.read(cx).value().to_string();
        let rows = match self.state.read(cx).api_repositories.get(&account.key()) {
            Some(repos) => group_rows(
                &crate::cloneable_repositories::visible_repositories(repos, &account, cx),
                &account.login,
                &crate::cloneable_repositories::filter_query(&query, cx),
            ),
            None => return,
        };
        // `293-clone-multiple`: the filter drops the picks it hides
        if !self.multi.is_empty() && !query.trim().is_empty() {
            let visible: Vec<String> = self
                .multi
                .iter()
                .filter(|url| {
                    rows.iter()
                        .any(|r| matches!(r, CloneRow::Item(repo, _) if repo.clone_url == **url))
                })
                .cloned()
                .collect();
            if visible.len() != self.multi.len() {
                self.set_multi(visible, window, cx);
            }
            if self.multi_active() {
                return;
            }
        }
        match crate::cloneable_repositories::filtered_selection(
            &rows,
            &query,
            self.selected_repo.as_deref(),
        ) {
            Some(Some(repo)) => self.select_repository(&repo, window, cx),
            Some(None) => self.selected_repo = None,
            None => {}
        }
    }

    /// `onSelectionChanged` on the repository list: the clone URL drives the
    /// path like a typed URL would.
    fn select_repository(
        &mut self,
        repo: &GitHubRepository,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_repo = Some(repo.clone_url.clone());
        let url = repo.clone_url.clone();
        self.url.update(cx, |s, cx| s.set_value(url, window, cx));
        cx.notify();
    }

    fn url_tab(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        // `dialog.clone-repository .dialog-content .row-component { margin-bottom: 0 }`
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACING_THIRD())
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child("Repository URL or GitHub username and repository")
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .child("(")
                                    .child(crate::widgets::code_ref("hubot/cool-repo", cx))
                                    .child(")"),
                            ),
                    )
                    .child(text_box("clone-url", &self.url, None, window, cx)),
            )
            .child(self.path_row(window, cx))
    }

    /// `.local-path-field`: Local Path + Choose… (and, with
    /// `233-shallow-clone`, the "Shallow clone" checkbox below it).
    fn path_row(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let shallow_option = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::SHALLOW_CLONE);
        let weak = cx.weak_entity();
        let shallow = crate::widgets::checkbox_row(
            "clone-shallow",
            self.shallow,
            "Shallow clone (only the latest commit)",
            move |checked, _, cx| {
                weak.update(cx, |this, cx| {
                    this.shallow = checked;
                    cx.notify();
                })
                .ok();
            },
            cx,
        );
        let add_existing = self.existing_repo.clone().map(|path| {
            let t = cx.ghd();
            div()
                .mt(SPACING())
                .flex()
                .flex_row()
                .flex_wrap()
                .gap(zpx(4.))
                .text_color(t.text_secondary)
                .child("This folder is already a Git repository.")
                .child(
                    div()
                        .id("clone-add-existing")
                        .text_color(t.link)
                        .cursor_pointer()
                        .child("Add this repository instead?")
                        .on_click(move |_, _, cx| {
                            Dispatcher::close_popup(cx);
                            Dispatcher::add_repository(path.clone(), cx);
                        }),
                )
        });
        let visual_studio =
            self.visual_studio
                .as_ref()
                .filter(|_| {
                    self.state
                        .read(cx)
                        .flags
                        .bool(corvene_core::flags::ids::VISUAL_STUDIO_CLONE_FOLDER)
                })
                .map(|dir| {
                    let t = cx.ghd();
                    let label = if self.in_visual_studio {
                        format!(
                            "Clone into {} instead",
                            self.state.read(cx).clone_dir().display()
                        )
                    } else {
                        format!("Clone into Visual Studio's folder ({})", dir.display())
                    };
                    div().mt(SPACING()).child(
                        div()
                            .id("clone-visual-studio")
                            .text_color(t.link)
                            .cursor_pointer()
                            .child(label)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_visual_studio(window, cx)
                            })),
                    )
                });
        div()
            .flex()
            .flex_col()
            .child(self.path_field(window, cx))
            .children(visual_studio)
            .children(
                crate::widgets::android_storage_note(
                    crate::widgets::StorageNote::NewRepository,
                    cx,
                )
                .map(|note| div().mt(SPACING()).child(note)),
            )
            .children(add_existing)
            .when(self.multi_active(), |d| {
                let t = cx.ghd();
                d.child(div().mt(SPACING()).text_color(t.text_secondary).child(format!(
                    "{} repositories selected; each is cloned into its own folder here, one after \
                     the other.",
                    self.multi.len()
                )))
            })
            .when(
                !self.multi_active()
                    && self
                        .state
                        .read(cx)
                        .flags
                        .bool(corvene_core::flags::ids::ALIAS_WHEN_ADDING),
                |d| {
                    d.child(div().mt(SPACING()).child(labeled(
                        "Alias",
                        text_box("clone-alias", &self.alias, None, window, cx),
                        cx,
                    )))
                },
            )
            .when(shallow_option, |d| {
                d.child(div().mt(SPACING()).child(shallow))
            })
    }

    fn path_field(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_end()
            .gap(SPACING())
            .child(labeled(
                // `293-clone-multiple`: each repository goes into its own
                // folder in this one
                if self.multi_active() {
                    mac_or("Parent Folder", "Parent folder")
                } else {
                    mac_or("Local Path", "Local path")
                },
                text_box("clone-path", &self.path, None, window, cx),
                cx,
            ))
            .child(
                button("clone-choose", "Choose…", cx)
                    .flex_none()
                    .on_click(cx.listener(|this, _, window, cx| this.choose(window, cx))),
            )
    }

    fn account_tab(&self, enterprise: bool, window: &Window, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let host = if enterprise {
            "GitHub Enterprise"
        } else {
            "GitHub.com"
        };
        let Some(account) = self.account(cx) else {
            let enterprise_flag = enterprise;
            // `CallToAction`: the text beside a 120 px Sign In button
            return crate::widgets::call_to_action(
                "clone-sign-in",
                format!("Sign in to your {host} account to access your repositories."),
                mac_or("Sign In", "Sign in"),
                move |_, cx| {
                    Dispatcher::show_popup(
                        Popup::SignIn {
                            enterprise: enterprise_flag,
                        },
                        cx,
                    )
                },
                cx,
            )
            .into_any_element();
        };
        let (loading, loaded, rows) = {
            let s = self.state.read(cx);
            let repos = s.api_repositories.get(&account.key());
            (
                s.api_repositories_loading.contains(&account.key()),
                repos.is_some(),
                repos
                    .map(|r| {
                        group_rows(
                            &crate::cloneable_repositories::visible_repositories(r, &account, cx),
                            &account.login,
                            &crate::cloneable_repositories::filter_query(
                                &self.filter.read(cx).value(),
                                cx,
                            ),
                        )
                    })
                    .unwrap_or_default(),
            )
        };
        let owner_picker = self
            .state
            .read(cx)
            .api_repositories
            .get(&account.key())
            .and_then(|repos| {
                crate::cloneable_repositories::owner_picker("clone-owner", &account, repos, cx)
            });
        let query = self.filter.read(cx).value().trim().to_string();
        let list: AnyElement = if rows.is_empty() {
            no_items(
                "clone-refresh-link",
                &account,
                loading,
                loaded,
                &query,
                true,
                cx,
            )
        } else {
            let weak = cx.weak_entity();
            let rows = Rc::new(rows);
            let clicked_rows = rows.clone();
            repository_list(
                "clone-repository-list",
                rows,
                if self.multi.is_empty() {
                    self.selected_repo.iter().cloned().collect()
                } else {
                    self.multi.clone()
                },
                ListStyle {
                    inset: 10.,
                    small_headers: true,
                    zoom: 1.,
                    focused: true,
                },
                Rc::new(move |repo, modifiers, window, cx| {
                    let rows = clicked_rows.clone();
                    weak.update(cx, |this, cx| {
                        this.click_repository(repo, modifiers, &rows, window, cx)
                    })
                    .ok();
                }),
            )
        };
        let picker = (self.accounts_for_tab(cx).len() > 1).then(|| {
            let weak = cx.weak_entity();
            account_picker(
                "clone-account-picker",
                &account,
                &self.picker,
                move |window, cx| {
                    weak.update(cx, |this, cx| {
                        this.picker.open = !this.picker.open;
                        // the popover's filter box has `autoFocus`
                        if this.picker.open {
                            let handle = this.picker.filter.read(cx).focus_handle(cx);
                            window.focus(&handle, cx);
                        }
                        cx.notify();
                    })
                    .ok();
                },
                cx,
            )
        });
        let popover = (picker.is_some() && self.picker.open).then(|| {
            let pick = cx.weak_entity();
            let close = cx.weak_entity();
            account_popover(
                "clone-account",
                &self.picker,
                crate::cloneable_repositories::PopoverPlacement {
                    scale: 1.,
                    gap: 4.,
                    fixed_height: false,
                    balloon: false,
                },
                self.accounts_for_tab(cx),
                Some(&account),
                Rc::new(move |account, _, cx| {
                    pick.update(cx, |this, cx| this.pick_account(account, cx))
                        .ok();
                }),
                Rc::new(move |_, cx| {
                    close
                        .update(cx, |this, cx| {
                            this.picker.open = false;
                            cx.notify();
                        })
                        .ok();
                }),
                window,
                cx,
            )
        });
        div()
            .flex()
            .flex_col()
            .gap(SPACING())
            .children(picker)
            .children(popover)
            .child(
                // filter row: text box + refresh (`renderPostFilter`)
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING())
                    .child(crate::widgets::filter_text_box(
                        "clone-filter",
                        &self.filter,
                        Some(octicon(Octicon::Search, t.text_secondary)),
                        window,
                        cx,
                    ))
                    // Corvene (`292-clone-owner-picker`)
                    .children(owner_picker)
                    .child(refresh_button("clone-refresh", &account, loading, cx)),
            )
            .child(
                div()
                    .h(LIST_HEIGHT())
                    .w_full()
                    .border_1()
                    .border_color(t.box_border)
                    .rounded(BORDER_RADIUS())
                    .overflow_hidden()
                    .child(list),
            )
            .child(self.path_row(window, cx))
            .into_any_element()
    }
}

/// What the URL field clones: `normalizeCloneUrl`'s URL, or with
/// `232-clone-local-sources` a local folder / `file://` URL as typed.
fn clone_source(input: &str, local_ok: bool) -> Option<String> {
    if local_ok && corvene_core::clone_info::local_source(input).is_some() {
        return Some(input.trim().to_string());
    }
    corvene_git::normalize_clone_url(input)
}

/// `<clone dir>/<name>` for a clone URL (`<clone dir>/<owner>/<name>` with
/// `230-clone-path-includes-owner` when the URL has an owner), the clone
/// dir itself when the input is not a URL yet.
fn derived_path(base: &Path, url: &str, with_owner: bool, local_ok: bool) -> PathBuf {
    let Some(url) = clone_source(url, local_ok) else {
        return base.to_path_buf();
    };
    let Some(name) = corvene_git::repository_name_from_url(&url) else {
        return base.to_path_buf();
    };
    let owner = with_owner
        .then(|| corvene_core::clone_info::parse_repository_identifier(&url))
        .flatten()
        .map(|id| id.owner)
        .filter(|owner| !owner.contains(['/', '\\']) && owner != ".." && owner != ".");
    match owner {
        Some(owner) => base.join(owner).join(name),
        None => base.join(name),
    }
}

/// `validateEmptyFolder`: the destination must be missing or an empty folder.
fn validate_empty_folder(path: &Path) -> Option<&'static str> {
    if path.as_os_str().is_empty() {
        return None;
    }
    match std::fs::read_dir(path) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                Some("This folder contains files. Git can only clone to empty folders.")
            } else {
                None
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) if err.kind() == std::io::ErrorKind::NotADirectory => {
            Some("There is already a file with this name. Git can only clone to a folder.")
        }
        Err(_) => Some("Unable to read path on disk. Please check the path and try again."),
    }
}

/// `330-clone-path-validation`: a leading `~` or `~/` is the home folder.
fn expand_home(raw: &str, home: &Path) -> PathBuf {
    let rest = raw
        .strip_prefix("~/")
        .or_else(|| raw.strip_prefix("~\\").filter(|_| cfg!(windows)));
    match rest {
        Some(rest) => home.join(rest),
        None if raw == "~" => home.to_path_buf(),
        None => PathBuf::from(raw),
    }
}

/// `330-clone-path-validation`: the expanded path and what is wrong with it:
/// relative, a non-empty folder or a file (`validateEmptyFolder`), or a
/// missing folder whose nearest existing parent is a file or not writable.
fn validate_clone_path(raw: &str, home: &Path) -> (PathBuf, Option<&'static str>) {
    let path = expand_home(raw, home);
    if raw.is_empty() {
        return (path, None);
    }
    if !path.is_absolute() {
        return (
            path,
            Some("Enter a full path, such as ~/Documents/repository."),
        );
    }
    if path.exists() {
        let error = validate_empty_folder(&path);
        return (path, error);
    }
    let error = match path.ancestors().skip(1).find(|a| a.exists()) {
        Some(parent) if !parent.is_dir() => {
            Some("Part of this path is a file. Git can only clone into a folder.")
        }
        Some(parent) if !dir_writable(parent) => {
            Some("You don't have permission to create a folder here. Choose another location.")
        }
        _ => None,
    };
    (path, error)
}

/// `access(path, W_OK | X_OK)`: new entries can be created in `path`.
#[cfg(unix)]
fn dir_writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return true;
    };
    // SAFETY: `c_path` is a valid NUL-terminated string that outlives the call.
    unsafe { libc::access(c_path.as_ptr(), libc::W_OK | libc::X_OK) == 0 }
}

/// Windows ACLs are left to git.
#[cfg(not(unix))]
fn dir_writable(_: &Path) -> bool {
    true
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

impl Render for CloneRepositoryDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let can_clone = self.path_error.is_none()
            && !self.resolving
            && if self.multi_active() {
                self.multi_conflicts.is_empty()
            } else {
                self.clone_target(cx).is_some()
            };
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let this = cx.entity();
        let selected = match self.tab {
            Tab::DotCom => 0,
            Tab::Enterprise => 1,
            Tab::Url => 2,
        };
        let tabs_entity = cx.entity();
        let body: AnyElement = match self.tab {
            Tab::DotCom => self.account_tab(false, window, cx),
            Tab::Enterprise => self.account_tab(true, window, cx),
            Tab::Url => self.url_tab(window, cx).into_any_element(),
        };
        // `293-clone-multiple`: the destinations in the way
        let conflicts = (!self.multi_conflicts.is_empty()).then(|| {
            SharedString::from(format!(
                "These folders already exist and are not empty (or two repositories would share \
                 one): {}",
                self.multi_conflicts.join(", ")
            ))
        });
        let error: Option<SharedString> = conflicts
            .or_else(|| self.resolve_error.map(SharedString::from))
            .or_else(|| self.clone_error.as_ref().map(|(e, _, _)| e.clone()))
            .or_else(|| self.path_error.map(SharedString::from));
        // signed out, the account tabs are only a call to action: no footer
        let has_footer = self.tab == Tab::Url || self.account(cx).is_some();
        let tab_ring = self.tab_focus_visible && !has_footer;

        dialog_loading_framed(
            "clone-repository",
            mac_or("Clone a Repository", "Clone a repository"),
            self.resolving,
            div()
                .flex()
                .flex_col()
                .when_some(error, |d, message| {
                    d.child(
                        dialog_error_banner(message, cx)
                            .mx(zpx(0.))
                            .mt(zpx(0.))
                            .mb(zpx(0.)),
                    )
                })
                // the tab bar runs edge to edge above the padded tab content;
                // signed out, nothing precedes the selected tab (tabIndex 0) so
                // it takes the dialog's first focus
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        if this.tab_focus_visible {
                            this.tab_focus_visible = false;
                            cx.notify();
                        }
                    }),
                )
                .child(div().child(crate::tab_bar::tab_bar_focus(
                    vec![
                        TabModel {
                            dot: false,
                            id: "clone-tab-dotcom",
                            label: "GitHub.com".into(),
                            count: None,
                        },
                        TabModel {
                            dot: false,
                            id: "clone-tab-enterprise",
                            label: "GitHub Enterprise".into(),
                            count: None,
                        },
                        TabModel {
                            dot: false,
                            id: "clone-tab-url",
                            label: "URL".into(),
                            count: None,
                        },
                    ],
                    selected,
                    tab_ring,
                    move |ix, window, cx| {
                        tabs_entity.update(cx, |d, cx| {
                            let tab = match ix {
                                0 => Tab::DotCom,
                                1 => Tab::Enterprise,
                                _ => Tab::Url,
                            };
                            d.set_tab(tab, window, cx);
                        })
                    },
                    cx,
                )))
                .child(div().p(SPACING_DOUBLE()).child(body)),
            if !has_footer {
                Vec::new()
            } else {
                OkCancelButtonGroup {
                    destructive: false,
                    cancel: GroupButtonSpec {
                        id: "clone-cancel",
                        label: "Cancel".into(),
                        disabled: false,
                        on_click: Box::new(close),
                    },
                    ok: GroupButtonSpec {
                        id: "clone-ok",
                        // `293-clone-multiple`
                        label: if self.multi_active() {
                            if cfg!(target_os = "macos") {
                                format!("Clone {} Repositories", self.multi.len())
                            } else {
                                format!("Clone {} repositories", self.multi.len())
                            }
                            .into()
                        } else {
                            "Clone".into()
                        },
                        disabled: !can_clone,
                        on_click: Box::new(move |_, cx| {
                            if can_clone {
                                this.update(cx, |d, cx| d.submit(cx));
                            }
                        }),
                    },
                }
                .into_buttons()
            },
            DialogFrame {
                content_padding: false,
                ..DialogFrame::default()
            },
            close,
            window,
            cx,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    #[cfg(unix)]
    fn validates_clone_paths() {
        let home = Path::new("/home/me");
        assert_eq!(
            expand_home("~/src/app", home),
            Path::new("/home/me/src/app")
        );
        assert_eq!(expand_home("~", home), Path::new("/home/me"));
        assert_eq!(expand_home("/abs/~/x", home), Path::new("/abs/~/x"));
        assert!(validate_clone_path("relative/app", home).1.is_some());
        assert!(validate_clone_path("", home).1.is_none());
        let dir = std::env::temp_dir().join(format!("corvene-clone-path-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("file");
        std::fs::write(&file, "x").unwrap();
        let under_file = file.join("app");
        assert_eq!(
            validate_clone_path(&under_file.to_string_lossy(), home).1,
            Some("Part of this path is a file. Git can only clone into a folder.")
        );
        let fresh = dir.join("new").join("app");
        assert_eq!(validate_clone_path(&fresh.to_string_lossy(), home).1, None);
        // a home-relative path lands in the home folder
        let (expanded, _) = validate_clone_path("~/new", &dir);
        assert_eq!(expanded, dir.join("new"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn derived_path_can_include_the_owner() {
        let base = Path::new("/c");
        let url = "https://github.com/octocat/Hello.git";
        assert_eq!(derived_path(base, url, false, false), Path::new("/c/Hello"));
        assert_eq!(
            derived_path(base, url, true, false),
            Path::new("/c/octocat/Hello")
        );
        assert_eq!(
            derived_path(base, "octocat/Hello", true, false),
            Path::new("/c/octocat/Hello")
        );
        assert_eq!(
            derived_path(base, "git@ghe.corp:team/app.git", true, false),
            Path::new("/c/team/app")
        );
        assert_eq!(derived_path(base, "nope", true, false), Path::new("/c"));
        // `232-clone-local-sources`: a local folder names the clone after itself
        #[cfg(not(windows))]
        assert_eq!(
            derived_path(base, "/src/a/tool.git", true, true),
            Path::new("/c/tool")
        );
        #[cfg(windows)]
        assert_eq!(
            derived_path(base, r"C:\src\a\tool.git", true, true),
            Path::new("/c/tool")
        );
        assert_eq!(
            derived_path(base, "file:///src/my-app", true, true),
            Path::new("/c/my-app")
        );
    }
}
