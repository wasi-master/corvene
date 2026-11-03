//! Editors, shells, Finder, GitHub URLs, the Settings and Repository Settings
//! dialogs' load/save, and repository removal - GHD `app-store.ts`
//! (`_openInExternalEditor`, `_openShell`, `_openInBrowser`, `_setRemoteURL`,
//! `_saveGitIgnore`, `_removeRepository`) plus `preferences.tsx#onSave` and
//! `repository-settings.tsx#onSubmit`.
//!
//! Deviation: View on GitHub also opens a non-GitHub repository's default
//! remote as a web page (`remote_web_url`, `262-view-on-remote`); GHD
//! disables it.
//! Deviation: `518-per-repo-editor` opens a repository and its files in
//! the editor its Repository Settings name, over the one in Settings.
//! Deviation: `519-open-file-in-repository-window` opens a file inside a
//! repository through the editor's command line tool with the repository
//! folder, in that folder's window (GHD `launchExternalEditor` opens the
//! file alone).
//!
//! Repository › Show in Finder ([`Dispatcher::show_repository`]) is GHD's
//! `showFolderContents` (`ui/main-process-proxy.ts`, [`show_folder_contents`]):
//! a directory opens in the file manager, except that on macOS a path that
//! is or may be an application bundle (`lib/is-application-bundle.ts`, read
//! with `mdls`) is only revealed after a native confirmation. GHD makes
//! Cancel the confirmation's default button; GPUI's alert makes the first
//! button (Reveal in Finder) the Return default, Escape cancels.
//!
//! Deviation (`352-remember-pr-base`; GHD `app-store.ts#_startPullRequest`
//! always proposes the default branch): the base picked in Preview Pull
//! Request is remembered per repository and proposed again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::host::{AsyncCtx, Host};
use corvene_platform::app_icons::AppIcon;
use tracing::{error, info, warn};

use crate::dispatcher::Dispatcher;
use crate::persistence::Settings;
use crate::remote::spawn_bg;
use crate::state::{
    GitConfigLocation, GlobalGitConfig, Popup, PreferencesTab, RepositorySettingsData,
    RepositorySettingsTab,
};
use corvene_models::{GitHubRepository, Identity};
use corvene_platform::{editors, shells, trash};

/// GHD `IFileInformation`: what [`show_folder_contents`] reads of a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileInformation {
    pub is_directory: bool,
}

/// A platform operation on a path that may fail with a message (GHD's
/// rejected promise).
pub type PathDependency<T> = Box<dyn Fn(&Path) -> Result<T, String>>;

/// GHD `IShowFolderContentsDependencies`: the platform operations
/// [`show_folder_contents`] uses, replaceable for tests.
pub struct ShowFolderContentsDependencies {
    /// Whether the current platform is macOS.
    pub is_darwin: bool,
    /// Reads file information for the target path.
    pub stat: PathDependency<FileInformation>,
    /// Determines whether a path is a macOS application bundle.
    pub is_application_bundle: PathDependency<bool>,
    /// Requests confirmation before revealing a potentially executable path.
    pub confirm_reveal: Box<dyn Fn() -> Result<bool, String>>,
    /// Opens a directory directly in the platform file manager.
    pub open_directory: Box<dyn Fn(&Path)>,
    /// Reveals and selects a path in the platform file manager.
    pub reveal_item: PathDependency<()>,
}

/// What [`show_folder_contents`] does with a path, decided from its file
/// information and (on macOS) whether it may be an application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderContentsAction {
    /// The file information could not be read (not on macOS): nothing.
    Nothing,
    /// A conclusively safe directory.
    OpenDirectory,
    /// Not a directory (not on macOS).
    RevealItem,
    /// Ask first (`revealAfterConfirmation`), then reveal.
    ConfirmThenReveal,
}

/// The decision half of GHD `showFolderContents`, with its logging.
pub fn folder_contents_action(
    path: &Path,
    is_darwin: bool,
    stat: impl Fn(&Path) -> Result<FileInformation, String>,
    is_application_bundle: impl Fn(&Path) -> Result<bool, String>,
) -> FolderContentsAction {
    let shown = path.display();
    let stats = match stat(path) {
        Ok(stats) => stats,
        Err(err) => {
            error!("Unable to retrieve file information for {shown}: {err}");
            return if is_darwin {
                FolderContentsAction::ConfirmThenReveal
            } else {
                FolderContentsAction::Nothing
            };
        }
    };
    if !stats.is_directory {
        error!("Trying to get the folder contents of a non-folder at '{shown}'");
        return if is_darwin {
            FolderContentsAction::ConfirmThenReveal
        } else {
            FolderContentsAction::RevealItem
        };
    }
    // on Windows and Linux a directory is just a directory
    if !is_darwin {
        return FolderContentsAction::OpenDirectory;
    }
    // on macOS opening an app bundle would run it; without readable
    // metadata, err on the side of caution
    let can_open_safely = match is_application_bundle(path) {
        Ok(is_bundle) => !is_bundle,
        Err(err) => {
            error!("Failed to load metadata for path '{shown}': {err}");
            false
        }
    };
    if can_open_safely {
        FolderContentsAction::OpenDirectory
    } else {
        info!(
            "Preventing direct open of path '{shown}' because it could not be conclusively identified as non-executable"
        );
        FolderContentsAction::ConfirmThenReveal
    }
}

/// GHD `showFolderContents(path, dependencies)` (`ui/main-process-proxy.ts`):
/// open a directory in the file manager without executing an application
/// bundle on macOS ([`folder_contents_action`]). A failed confirmation or
/// reveal after one is logged; only a failed reveal of a non-directory
/// (not on macOS) is an error.
pub fn show_folder_contents(
    path: &Path,
    dependencies: &ShowFolderContentsDependencies,
) -> Result<(), String> {
    match folder_contents_action(
        path,
        dependencies.is_darwin,
        &dependencies.stat,
        &dependencies.is_application_bundle,
    ) {
        FolderContentsAction::Nothing => {}
        FolderContentsAction::OpenDirectory => (dependencies.open_directory)(path),
        FolderContentsAction::RevealItem => (dependencies.reveal_item)(path)?,
        FolderContentsAction::ConfirmThenReveal => {
            // `revealAfterConfirmation`: without a confirmation, leave the
            // path untouched
            let confirmed = (dependencies.confirm_reveal)().unwrap_or_else(|err| {
                error!(
                    "Unable to confirm revealing folder '{}': {err}",
                    path.display()
                );
                false
            });
            if confirmed && let Err(err) = (dependencies.reveal_item)(path) {
                error!("Unable to reveal folder '{}': {err}", path.display());
            }
        }
    }
    Ok(())
}

/// GHD `isApplicationBundle` / `isApplicationBundleFromMetadata`
/// (`lib/is-application-bundle.ts`, `mdls`) are OS integration.
pub use corvene_platform::apps::{is_application_bundle, is_application_bundle_from_metadata};

/// What Settings › Save applies (`preferences.tsx#onSave`).
#[derive(Clone, Debug)]
pub struct PreferencesSave {
    pub settings: Settings,
    pub name: String,
    pub email: String,
    pub default_branch: String,
    /// `517-path-git-settings`: global `core.quotepath` / `core.longpaths`
    /// to write; `None` leaves them alone.
    pub quotepath: Option<bool>,
    pub longpaths: Option<bool>,
    /// `526-commit-signing`: the global signing settings to write; `None`
    /// leaves them alone.
    pub signing: Option<crate::state::SigningConfig>,
}

/// What Repository Settings › Save applies (`repository-settings.tsx#onSubmit`).
/// Each field is `Some` only when the user changed it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RepositorySettingsSave {
    /// `(remote name, new url)`
    pub remote_url: Option<(String, String)>,
    pub gitignore: Option<String>,
    /// Where the author identity lives, with the local name/email to store.
    pub git_config: Option<(GitConfigLocation, String, String)>,
    /// `239-line-endings-setting`: the `--local` `core.autocrlf` to store
    /// (`None`: remove it, so the global value applies).
    pub autocrlf: Option<Option<String>>,
    /// `526-commit-signing`: the repository's signing settings; what
    /// differs from the ones in effect is stored `--local` (`None`: leave
    /// them).
    pub signing: Option<crate::state::SigningConfig>,
    /// `1109-remote-manager`: the remote changes, in the order they run
    /// (`remote_manager::remote_edits`).
    pub remote_edits: Vec<crate::remote_manager::RemoteEdit>,
    /// `1109-remote-manager`: where pushes go (`None`: leave it).
    pub push_config: Option<crate::remote_manager::PushTargetConfig>,
}

/// Corvene (`526-commit-signing`): write what differs between `before` and
/// `after` to the repository at `workdir`'s own config, or the global one.
fn write_signing(
    git: Arc<corvene_git::GitBinary>,
    workdir: Option<&Path>,
    before: &crate::state::SigningConfig,
    after: &crate::state::SigningConfig,
) -> Result<(), corvene_git::GitError> {
    let set = |key: &str, value: Option<&str>| match (workdir, value) {
        (Some(dir), Some(value)) => {
            corvene_git::set_local_config_value(git.clone(), dir, key, value)
        }
        (Some(dir), None) => corvene_git::remove_local_config_value(git.clone(), dir, key),
        (None, Some(value)) => corvene_git::set_global_config_value(git.clone(), key, value),
        (None, None) => corvene_git::remove_global_config_value(git.clone(), key),
    };
    if after.sign != before.sign {
        set(
            "commit.gpgsign",
            Some(if after.sign { "true" } else { "false" }),
        )?;
    }
    let key = after.key.trim();
    if key != before.key.trim() {
        set("user.signingkey", (!key.is_empty()).then_some(key))?;
    }
    if after.ssh != before.ssh {
        set("gpg.format", after.ssh.then_some("ssh"))?;
    }
    Ok(())
}

/// Corvene (`526-commit-signing`): the signing settings `value` reads.
fn read_signing(value: impl Fn(&str) -> Option<String>) -> crate::state::SigningConfig {
    crate::state::SigningConfig {
        sign: value("commit.gpgsign").is_some_and(|v| config_bool(&v, false)),
        key: value("user.signingkey").unwrap_or_default(),
        ssh: value("gpg.format").is_some_and(|v| v.trim().eq_ignore_ascii_case("ssh")),
    }
}

/// Corvene (`524-open-repository-with-editor`): an editor to open a
/// repository in, picked from Open in Editor ▸.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorChoice {
    /// An installed editor, by its friendly name.
    Installed(String),
    Custom(crate::persistence::CustomIntegration),
}

/// `openIssueCreationPage`: GitHub's issue template chooser.
pub fn issue_creation_url(html_url: &str) -> String {
    format!("{html_url}/issues/new/choose")
}

/// GHD `_showGitHubExplore`: `html_url` with its path replaced by
/// `/explore` (`url.pathname = '/explore'`), `None` for a URL without a
/// scheme.
pub fn github_explore_url(html_url: &str) -> Option<String> {
    let scheme_end = html_url.find("://")? + 3;
    let rest = &html_url[scheme_end..];
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    if host_end == 0 {
        return None;
    }
    let after_path = rest[host_end..]
        .find(['?', '#'])
        .map_or("", |ix| &rest[host_end + ix..]);
    Some(format!(
        "{}{}/explore{after_path}",
        &html_url[..scheme_end],
        &rest[..host_end]
    ))
}

/// The web page of a remote that is not a GitHub repository
/// (`262-view-on-remote`): an `http(s)` URL without its credentials and
/// `.git`, and SSH / scp-style / `git://` URLs as `https://host/path`.
/// `None` for local paths and anything else without a host.
pub fn remote_web_url(url: &str) -> Option<String> {
    let url = url.trim();
    let (scheme, host, path) = match ["https://", "http://"]
        .into_iter()
        .find_map(|scheme| url.strip_prefix(scheme).map(|rest| (scheme, rest)))
    {
        Some((scheme, rest)) => {
            let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
            let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
            (scheme, host.to_string(), path.to_string())
        }
        None => {
            let (host, path) = corvene_models::split_remote(url)?;
            ("https://", host, path)
        }
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("{scheme}{host}/{path}"))
}

/// `encodeURIComponent` for branch names in GitHub URLs.
pub fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `333-pr-base-from-branch-origin`: the remote branch (`<remote>/<name>`)
/// to propose as a pull request's base for a branch created from `origin`
/// (a local or remote-tracking branch name, `refs/...` allowed), when that
/// is neither `HEAD`, the branch itself nor the default branch and
/// `remote` has it.
pub fn pull_request_base_candidate(
    origin: &str,
    current: &str,
    default_branch: Option<&str>,
    remote: &str,
    branches: &[corvene_models::Branch],
) -> Option<String> {
    let origin = origin
        .strip_prefix("refs/heads/")
        .or_else(|| origin.strip_prefix("refs/remotes/"))
        .unwrap_or(origin);
    // a remote-tracking start point names its remote first
    let name = branches
        .iter()
        .find(|b| b.kind == corvene_models::BranchKind::Remote && b.name == origin)
        .map(|b| b.name_without_remote())
        .unwrap_or(origin);
    if name == "HEAD" || name == current || Some(name) == default_branch {
        return None;
    }
    let remote_name = format!("{remote}/{name}");
    branches
        .iter()
        .any(|b| b.kind == corvene_models::BranchKind::Remote && b.name == remote_name)
        .then_some(remote_name)
}

/// GHD `_openCreatePullRequestInBrowser`: `${htmlURL}/pull/new/[base...]compare`;
/// a fork contributing to its parent prefixes both refs with `owner:name:`.
///
/// Deviation (`323-fork-own-pr-target`, `own_fork_targets_itself`): a fork
/// set up for its own work names itself on both sides and falls back to its
/// own default branch as the base. GHD leaves the refs bare, and GitHub's
/// `pull/new` page on a fork then proposes merging into the parent.
///
/// Deviation (`332-pr-url-owner-branch-refs`, `owner_refs`): the prefixes are
/// `owner:` instead of `owner:name:`. Older GitHub Enterprise Server compare
/// pages do not know the three-part form and report "nothing to compare";
/// an owner has at most one fork in a network, so `owner:` is unambiguous.
pub fn pull_request_url(
    gh: &GitHubRepository,
    compare: &str,
    base: Option<&str>,
    contributing_to_parent: bool,
    own_fork_targets_itself: bool,
    owner_refs: bool,
) -> String {
    let own_fork = own_fork_targets_itself && !contributing_to_parent && gh.parent.is_some();
    let prefix = |owner: &str, name: &str| match owner_refs {
        true => format!("{owner}:"),
        false => format!("{owner}:{name}:"),
    };
    let self_prefix = prefix(&gh.owner, &gh.name);
    let base_prefix = match (&gh.parent, contributing_to_parent) {
        (Some(parent), true) => prefix(&parent.owner, &parent.name),
        _ if own_fork => self_prefix.clone(),
        _ => String::new(),
    };
    let base = if own_fork {
        base.or(gh.default_branch.as_deref())
    } else {
        base
    };
    let encoded_base = base
        .map(|b| format!("{base_prefix}{}...", encode_component(b)))
        .unwrap_or_default();
    let compare_prefix = if contributing_to_parent || own_fork {
        self_prefix
    } else {
        String::new()
    };
    format!(
        "{}/pull/new/{encoded_base}{compare_prefix}{}",
        gh.html_url,
        encode_component(compare)
    )
}

/// Flag `513-integration-app-icons`: the icons of the applications at
/// these paths, keyed by path (blocking).
fn app_icons<'a>(paths: impl Iterator<Item = &'a PathBuf>) -> HashMap<PathBuf, Arc<AppIcon>> {
    paths
        .filter_map(|path| {
            let icon = corvene_platform::app_icons::icon(path)?;
            Some((path.clone(), Arc::new(icon)))
        })
        .collect()
}

impl Dispatcher {
    // ---- integrations ----

    /// Probe LaunchServices for every known editor and shell (background),
    /// then remember them for the menus and Settings › Integrations, with
    /// their icons under flag `513-integration-app-icons`.
    pub fn detect_integrations(cx: &mut dyn Host) {
        let flags = &Self::state(cx).read(cx).flags;
        let extras = flags.bool(crate::flags::ids::EXTRA_EDITORS);
        let jetbrains_64bit_hive = flags.bool(crate::flags::ids::JETBRAINS_64BIT_HIVE);
        let with_icons = flags.bool(crate::flags::ids::INTEGRATION_APP_ICONS);
        spawn_bg(
            cx,
            move || {
                let editors = editors::available_editors(extras, jetbrains_64bit_hive);
                let shells = shells::available_shells();
                let icons = if with_icons {
                    app_icons(
                        editors
                            .iter()
                            .map(|e| &e.path)
                            .chain(shells.iter().map(|s| &s.path)),
                    )
                } else {
                    HashMap::new()
                };
                (editors, shells, icons)
            },
            |(editors, shells, icons), cx| {
                info!(
                    editors = editors.len(),
                    shells = shells.len(),
                    icons = icons.len(),
                    "integrations detected"
                );
                Self::state(cx).update(cx, |s, cx| {
                    s.editors = editors;
                    s.shells = shells;
                    s.app_icons = icons;
                    cx.notify();
                });
            },
        );
    }

    /// Flag `811`: History › Open with Default Program opens `path` as it
    /// is at `sha`, written to a read-only file under the temporary
    /// directory, rather than today's working copy.
    pub fn open_commit_file_with_default_program(
        id: u64,
        sha: String,
        path: String,
        cx: &mut dyn Host,
    ) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        spawn_bg(
            cx,
            move || -> Result<PathBuf, String> {
                let bytes = corvene_git::blob_bytes(git, &workdir, &sha, &path)
                    .map_err(|e| e.to_string())?;
                let short = &sha[..sha.len().min(12)];
                let file = historical_file_path(&std::env::temp_dir(), short, &path)
                    .ok_or_else(|| format!("Invalid path {path}"))?;
                write_read_only(&file, &bytes).map_err(|e| e.to_string())?;
                Ok(file)
            },
            |result, cx| match result {
                Ok(file) => cx.open_with_system(&file),
                Err(err) => Self::show_error("Could not open file", err, cx),
            },
        );
    }

    /// Repository › Open in <Editor> (`_openInExternalEditor`).
    pub fn open_in_editor(path: PathBuf, cx: &mut dyn Host) {
        Self::open_in_editor_at(path, None, cx);
    }

    /// Corvene (`524-open-repository-with-editor`): the editors of the
    /// repository's Open in Editor ▸ menus, as (label, editor): the
    /// installed ones, then Settings' custom ones (`523-custom-editor-list`);
    /// at most 8, and none unless there are two.
    pub fn menu_editors(s: &crate::state::AppState) -> Vec<(String, EditorChoice)> {
        if !s.flags.bool(crate::flags::ids::OPEN_REPOSITORY_WITH_EDITOR)
            || cfg!(target_os = "android")
        {
            return Vec::new();
        }
        let mut editors: Vec<(String, EditorChoice)> = s
            .editors
            .iter()
            .map(|e| (e.name.clone(), EditorChoice::Installed(e.name.clone())))
            .collect();
        if s.flags.bool(crate::flags::ids::CUSTOM_EDITOR_LIST) {
            editors.extend(
                s.settings
                    .custom_editors()
                    .into_iter()
                    .enumerate()
                    .filter(|(_, c)| !c.path.trim().is_empty())
                    .map(|(ix, c)| (c.display_name(ix, true), EditorChoice::Custom(c))),
            );
        } else if let Some(custom) = s.custom_editor_in_use() {
            let label = s.custom_editor_label().unwrap_or_default();
            editors.push((label, EditorChoice::Custom(custom.clone())));
        }
        if editors.len() < 2 {
            return Vec::new();
        }
        editors.truncate(8);
        editors
    }

    /// Corvene (`524-open-repository-with-editor`): open `path` in `editor`
    /// this once, leaving Settings and the repository's editor as they are.
    pub fn open_in_editor_with(path: PathBuf, editor: EditorChoice, cx: &mut dyn Host) {
        Self::open_in_editor_choosing(path, None, Some(editor), cx);
    }

    /// `open_in_editor` at a 1-based line where the editor supports it
    /// (the diff's "Open in <Editor> at Line N", flag
    /// `diff-open-in-editor-at-line`); a custom editor opens the file.
    pub fn open_in_editor_at(path: PathBuf, line: Option<u32>, cx: &mut dyn Host) {
        Self::open_in_editor_choosing(path, line, None, cx);
    }

    /// [`Self::open_in_editor_at`] in `chosen` when given, else in the
    /// editor the repository and Settings name.
    fn open_in_editor_choosing(
        path: PathBuf,
        line: Option<u32>,
        chosen: Option<EditorChoice>,
        cx: &mut dyn Host,
    ) {
        let (editors, selected, custom, workspace_file, folder, folder_as_workspace) = {
            let s = Self::state(cx).read(cx);
            // `518-per-repo-editor`: the repository's own editor wins over
            // Settings (also over a custom editor)
            let repo_editor = s.repository_editor(&path);
            // `519-open-file-in-repository-window`: the repository holding a
            // file (the innermost one)
            let folder = s
                .flags
                .bool(crate::flags::ids::OPEN_FILE_IN_REPOSITORY_WINDOW)
                .then(|| {
                    s.repositories
                        .iter()
                        .map(|r| &r.path)
                        .filter(|repo| path != **repo && path.starts_with(repo))
                        .max_by_key(|repo| repo.components().count())
                        .cloned()
                })
                .flatten();
            let (repo_editor, repo_custom) = match &chosen {
                Some(_) => (None, None),
                None => (repo_editor, s.repository_custom_editor(&path)),
            };
            let (chosen_installed, chosen_custom) = match chosen {
                Some(EditorChoice::Installed(name)) => (Some(name), None),
                Some(EditorChoice::Custom(custom)) => (None, Some(custom)),
                None => (None, None),
            };
            let chosen_installed_given = chosen_installed.is_some();
            (
                s.editors.clone(),
                chosen_installed.or_else(|| {
                    repo_editor
                        .clone()
                        .or_else(|| s.settings.external_editor.clone())
                }),
                // the one chosen, a repository's own custom editor, else
                // Settings' one (not when another editor was chosen)
                if chosen_installed_given {
                    None
                } else {
                    chosen_custom.or(repo_custom).or_else(|| {
                        s.custom_editor_in_use()
                            .cloned()
                            .filter(|_| repo_editor.is_none())
                    })
                },
                s.flags.bool(crate::flags::ids::VSCODE_WORKSPACE_FILE),
                folder,
                s.flags.bool(crate::flags::ids::NOTEPADPP_FOLDER_WORKSPACE),
            )
        };
        if let Some(custom) = custom {
            // `launchCustomExternalEditor`
            spawn_bg(
                cx,
                move || {
                    corvene_platform::custom_integration::launch(
                        &custom.path,
                        &custom.arguments,
                        &path,
                    )
                },
                |result, cx| {
                    if let Err(message) = result {
                        Self::show_editor_error(
                            editors::EditorError {
                                message: format!(
                                    "{message} Please open {} and check your custom editor.",
                                    corvene_platform::editors::SETTINGS_LABEL
                                ),
                                suggest_default_editor: false,
                                open_preferences: true,
                            },
                            None,
                            cx,
                        );
                    }
                },
            );
            return;
        }
        let editor = match editors::find_editor_or_default(&editors, selected.as_deref()) {
            Ok(Some(editor)) => editor.clone(),
            Ok(None) => {
                Self::show_editor_error(editors::no_editor_error(), None, cx);
                return;
            }
            Err(err) => {
                Self::show_editor_error(err, None, cx);
                return;
            }
        };
        // Android: a Termux editor cannot reach Corvene's own storage; the
        // error offers to move the repository (`shared_storage`)
        #[cfg(target_os = "android")]
        if editor.bundle_id.starts_with(editors::TERMUX_PREFIX)
            && let Some(offer) = Self::shared_storage_move_for(
                &path,
                |relative| crate::AfterSharedStorageMove::OpenEditor { relative, line },
                cx,
            )
        {
            Self::show_editor_error(
                editors::EditorError {
                    message: corvene_platform::android::TERMUX_PRIVATE_STORAGE.to_string(),
                    suggest_default_editor: false,
                    open_preferences: false,
                },
                Some(offer),
                cx,
            );
            return;
        }
        spawn_bg(
            cx,
            move || {
                if let Some(folder) = folder.filter(|_| path.is_file()) {
                    return editors::launch_in_folder(&editor, &folder, &path, line);
                }
                match line {
                    Some(line) => editors::launch_at_line(&editor, &path, line),
                    None => {
                        // `509-vscode-workspace-file`: a repository opens its only
                        // workspace file instead of the folder
                        let target = workspace_file
                            .then(|| editors::code_workspace_file(&editor, &path))
                            .flatten()
                            .unwrap_or(path);
                        editors::launch(&editor, &target, folder_as_workspace)
                    }
                }
            },
            |result, cx| {
                if let Err(err) = result {
                    Self::show_editor_error(err, None, cx);
                }
            },
        );
    }

    fn show_editor_error(
        err: editors::EditorError,
        move_to_shared_storage: Option<crate::SharedStorageMove>,
        cx: &mut dyn Host,
    ) {
        warn!(message = %err.message, "external editor");
        Self::show_popup(
            Popup::ExternalEditorError {
                message: err.message,
                suggest_default_editor: err.suggest_default_editor,
                open_preferences: err.open_preferences,
                move_to_shared_storage,
            },
            cx,
        );
    }

    /// Repository › Open in <Shell> (`_openShell`).
    pub fn open_in_shell(path: &Path, cx: &mut dyn Host) {
        let (shells, selected, custom) = {
            let s = Self::state(cx).read(cx);
            (
                s.shells.clone(),
                s.shell_label(),
                s.settings
                    .use_custom_shell
                    .then(|| s.settings.custom_shell.clone())
                    .flatten(),
            )
        };
        if let Some(custom) = custom {
            // `launchCustomShell`
            let path = path.to_path_buf();
            spawn_bg(
                cx,
                move || {
                    corvene_platform::custom_integration::launch(
                        &custom.path,
                        &custom.arguments,
                        &path,
                    )
                },
                |result, cx| {
                    if let Err(message) = result {
                        Self::show_popup(
                            Popup::ShellError {
                                message: format!(
                                    "{message} Please open {} and check your custom shell.",
                                    corvene_platform::editors::SETTINGS_LABEL
                                ),
                                move_to_shared_storage: None,
                            },
                            cx,
                        );
                    }
                },
            );
            return;
        }
        let wanted = shells::Shell::parse(&selected);
        let Some(found) = shells
            .iter()
            .find(|s| s.shell == wanted)
            .or_else(|| shells.first())
            .cloned()
        else {
            Self::show_popup(
                Popup::ShellError {
                    message: format!(
                        "Could not find shell '{selected}'. Please open {} and choose an installed shell.",
                        corvene_platform::editors::SETTINGS_LABEL
                    ),
                    move_to_shared_storage: None,
                },
                cx,
            );
            return;
        };
        let path = path.to_path_buf();
        // Android: Termux cannot reach Corvene's own storage; the error
        // offers to move the repository (`shared_storage`)
        #[cfg(target_os = "android")]
        if let Some(offer) =
            Self::shared_storage_move_for(&path, |_| crate::AfterSharedStorageMove::OpenShell, cx)
        {
            Self::show_popup(
                Popup::ShellError {
                    message: corvene_platform::android::TERMUX_PRIVATE_STORAGE.to_string(),
                    move_to_shared_storage: Some(offer),
                },
                cx,
            );
            return;
        }
        spawn_bg(
            cx,
            move || shells::launch(&found, &path),
            |result, cx| {
                if let Err(err) = result {
                    Self::show_popup(
                        Popup::ShellError {
                            message: format!(
                                "Something went wrong while trying to start the shell: {err}"
                            ),
                            move_to_shared_storage: None,
                        },
                        cx,
                    );
                }
            },
        );
    }

    /// Repository › Show in Finder, the repository list's Show in Finder
    /// and No Changes' suggestion (GHD `app.tsx` `showRepository` →
    /// [`show_folder_contents`]): open the repository's folder, asking first
    /// on macOS when it may be an application. With a `510-file-manager`
    /// application set, [`Self::show_in_finder`] instead.
    pub fn show_repository(path: &Path, cx: &mut dyn Host) {
        if Self::file_manager_app(cx).is_some() {
            Self::show_in_finder(path, cx);
            return;
        }
        let path = path.to_path_buf();
        let classified = path.clone();
        spawn_bg(
            cx,
            move || {
                folder_contents_action(
                    &classified,
                    cfg!(target_os = "macos"),
                    |path| {
                        std::fs::metadata(path)
                            .map(|m| FileInformation {
                                is_directory: m.is_dir(),
                            })
                            .map_err(|err| err.to_string())
                    },
                    is_application_bundle,
                )
            },
            move |action, cx| match action {
                FolderContentsAction::Nothing => {}
                FolderContentsAction::OpenDirectory => cx.open_with_system(&path),
                FolderContentsAction::RevealItem => Self::reveal_item(&path, cx),
                FolderContentsAction::ConfirmThenReveal => {
                    Self::reveal_after_confirmation(path, cx)
                }
            },
        );
    }

    /// GHD `revealAfterConfirmation` with the main process's
    /// `confirm-reveal-directory` warning: reveal `path` only once the user
    /// picks Reveal in Finder. A confirmation that cannot be shown is logged
    /// and leaves the path alone.
    #[cfg_attr(not(feature = "gpui"), allow(unused_variables))]
    fn reveal_after_confirmation(path: PathBuf, cx: &mut dyn Host) {
        #[cfg(feature = "gpui")]
        if let Some(cx) = cx.gpui_app() {
            Self::reveal_after_gpui_confirmation(path, cx);
            return;
        }
        error!(
            "Unable to confirm revealing folder '{}': no window",
            path.display()
        );
    }

    #[cfg(feature = "gpui")]
    fn reveal_after_gpui_confirmation(path: PathBuf, cx: &mut gpui_kit::App) {
        let Some(window) = cx.active_window().or_else(|| cx.windows().first().copied()) else {
            error!(
                "Unable to confirm revealing folder '{}': no window",
                path.display()
            );
            return;
        };
        let answer = window.update(cx, |_, window, cx| {
            window.prompt(
                gpui_kit::PromptLevel::Warning,
                "This repository might be an application.",
                Some(
                    "Opening it directly could run software. You can reveal and select it in Finder without opening it.",
                ),
                &[
                    gpui_kit::PromptButton::ok("Reveal in Finder"),
                    gpui_kit::PromptButton::cancel("Cancel"),
                ],
                cx,
            )
        });
        let answer = match answer {
            Ok(answer) => answer,
            Err(err) => {
                error!(
                    "Unable to confirm revealing folder '{}': {err}",
                    path.display()
                );
                return;
            }
        };
        cx.spawn(
            async move |cx: &mut gpui_kit::AsyncApp| match answer.await {
                Ok(0) => {
                    cx.update(|cx| Self::reveal_item(&path, cx));
                }
                Ok(_) => {}
                Err(err) => {
                    error!(
                        "Unable to confirm revealing folder '{}': {err}",
                        path.display()
                    );
                }
            },
        )
        .detach();
    }

    /// The `510-file-manager` application, when one is set.
    fn file_manager_app(cx: &dyn Host) -> Option<String> {
        let app = Self::state(cx)
            .read(cx)
            .flags
            .text(crate::flags::ids::FILE_MANAGER)
            .trim()
            .to_string();
        (!app.is_empty()).then_some(app)
    }

    /// Electron's `shell.showItemInFolder`: reveal and select `path` in the
    /// file manager.
    fn reveal_item(path: &Path, cx: &mut dyn Host) {
        // Linux: Electron's route (FileManager1, then xdg-open), which
        // works without a desktop portal too
        #[cfg(not(target_os = "macos"))]
        {
            let path = path.to_path_buf();
            cx.background_executor()
                .spawn(async move {
                    if let Err(err) = corvene_platform::apps::show_item_in_folder(&path) {
                        warn!(%err, path = %path.display(), "could not show the item");
                    }
                })
                .detach();
        }
        #[cfg(target_os = "macos")]
        cx.reveal_path(path);
    }

    /// Every Reveal in Finder item (`revealInFileManager`,
    /// `shell.showItemInFolder`). With a `510-file-manager` application set,
    /// it opens the folder (a file's parent folder) with `open -a <app>`
    /// instead.
    pub fn show_in_finder(path: &Path, cx: &mut dyn Host) {
        let Some(app) = Self::file_manager_app(cx) else {
            Self::reveal_item(path, cx);
            return;
        };
        let dir = if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent()
                .map_or_else(|| path.to_path_buf(), Path::to_path_buf)
        };
        if let Err(err) = corvene_platform::apps::open_with_app(Path::new(&app), &dir) {
            Self::show_error(
                "Unable to Open File Manager",
                format!("Could not open {} with {app}: {err}", dir.display()),
                cx,
            );
        }
    }

    /// Android, Options › Integrations: use the private key in `path` (a
    /// copy the file picker made) as the bundled ssh client's key. An
    /// encrypted key asks for its passphrase (`Popup::SshKeyPassphrase`).
    #[cfg(target_os = "android")]
    pub fn import_ssh_key(path: PathBuf, passphrase: Option<String>, cx: &mut dyn Host) {
        use corvene_platform::android::SshImportError;
        let wrong = passphrase.as_ref().is_some_and(|p| !p.is_empty());
        let file = path.clone();
        spawn_bg(
            cx,
            move || corvene_platform::android::import_ssh_key(&file, passphrase.as_deref()),
            move |result, cx| match result {
                Ok(_) => Self::state(cx).update(cx, |_, cx| cx.notify()),
                Err(SshImportError::Passphrase) => {
                    Self::show_popup(Popup::SshKeyPassphrase { path, wrong }, cx)
                }
                Err(SshImportError::Other(err)) => {
                    Self::show_error("Could not import the SSH key", err, cx)
                }
            },
        );
    }

    /// Android, Options › Integrations: create the SSH key the bundled ssh
    /// client uses (`corvene_platform::android::create_ssh_key`).
    #[cfg(target_os = "android")]
    pub fn create_ssh_key(cx: &mut dyn Host) {
        spawn_bg(
            cx,
            corvene_platform::android::create_ssh_key,
            |result, cx| match result {
                // the dialog reads the key again when the state notifies
                Ok(_) => Self::state(cx).update(cx, |_, cx| cx.notify()),
                Err(err) => Self::show_error("Could not create an SSH key", err, cx),
            },
        );
    }

    /// Android: a changed file's "Share…", the system's share sheet (GHD has
    /// no Android build).
    #[cfg(target_os = "android")]
    pub fn share_file(path: PathBuf, cx: &mut dyn Host) {
        if let Some(bridge) = corvene_platform::android::bridge()
            && let Err(err) = bridge.share_path(&path)
        {
            Self::show_error("Unable to Share", err, cx);
        }
    }

    /// Repository › Open With… (`_openWithSystemDialog`): pick an application,
    /// then `open -a <app> <repository>`. Also a changed file's "Open With…"
    /// (Corvene `713-open-file-with`), whose error names the file.
    pub fn open_with(path: PathBuf, cx: &mut dyn Host) {
        // Android: the system's chooser lists the applications that open it
        #[cfg(target_os = "android")]
        if let Some(bridge) = corvene_platform::android::bridge() {
            if let Err(err) = bridge.view_path_with_chooser(&path) {
                Self::show_error("Unable to Open", err, cx);
            }
            return;
        }
        let (title, what) = if path.is_dir() {
            ("Unable to Open Repository", "the repository")
        } else {
            ("Unable to Open File", "the file")
        };
        let receiver = cx.prompt_for_paths(crate::host::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let app = match receiver.await {
                Some(paths) => paths.into_iter().next(),
                _ => None,
            };
            if let Some(app) = app
                && let Err(err) = corvene_platform::apps::open_with_app(&app, &path)
            {
                cx.update(|cx| {
                    Self::show_error(
                        title,
                        format!("Could not open {what} with {}: {err}", app.display()),
                        cx,
                    )
                });
            }
        })
        .detach();
    }

    // ---- GitHub URLs (`_openInBrowser` callers) ----

    fn github_and_branch(id: u64, cx: &dyn Host) -> Option<(GitHubRepository, Option<String>)> {
        let s = Self::state(cx).read(cx);
        let gh = s.repository(id)?.github.clone()?;
        let branch = s
            .repo_states
            .get(&id)
            .and_then(|rs| rs.info.as_ref())
            .and_then(|info| info.current_branch())
            .map(|b| b.name.clone());
        Some((gh, branch))
    }

    /// GHD `_showGitHubExplore` (`lib/stores/app-store.ts`, the tutorial's
    /// "Open in Browser"): `/explore` on the repository's GitHub host.
    /// Nothing happens for a repository that is not on GitHub.
    pub fn show_github_explore(id: u64, cx: &mut dyn Host) {
        if let Some(url) =
            Self::github_and_branch(id, cx).and_then(|(gh, _)| github_explore_url(&gh.html_url))
        {
            Self::open_url(&url, cx);
        }
    }

    /// Repository › View on GitHub; with `262-view-on-remote` a repository
    /// that is not on GitHub opens its default remote's web page instead.
    pub fn view_on_github(id: u64, cx: &mut dyn Host) {
        if let Some((gh, _)) = Self::github_and_branch(id, cx) {
            Self::open_url(&gh.html_url, cx);
        } else if Self::view_hosted_repository(id, cx) {
            // flags 342-344: View on GitLab / Gitea / Bitbucket
        } else if let Some(url) = Self::non_github_remote_web_url(id, cx) {
            Self::open_url(&url, cx);
        }
    }

    /// Repository › View Upstream on GitHub (Corvene addition, flag
    /// `321-view-upstream-on-github`): the parent of a fork. Nothing happens
    /// for a repository that is not a fork.
    pub fn view_upstream_on_github(id: u64, cx: &mut dyn Host) {
        let url = Self::github_and_branch(id, cx).and_then(|(gh, _)| gh.parent.map(|p| p.html_url));
        if let Some(url) = url {
            Self::open_url(&url, cx);
        }
    }

    /// The default remote's web page (`remote_web_url`) of a loaded
    /// repository that is not on GitHub, when `262-view-on-remote` is on.
    pub fn non_github_remote_web_url(id: u64, cx: &dyn Host) -> Option<String> {
        let s = Self::state(cx).read(cx);
        if !s.flags.bool(crate::flags::ids::VIEW_ON_REMOTE) || s.repository(id)?.github.is_some() {
            return None;
        }
        let info = s.repo_states.get(&id)?.info.as_ref()?;
        remote_web_url(&corvene_git::find_default_remote(&info.remotes)?.url)
    }

    /// Repository › Create Issue on GitHub (`openIssueCreationPage`): the
    /// template chooser of the repository contributions go to (the parent of
    /// a fork unless the fork is set up for its own work,
    /// `getNonForkGitHubRepository`). GitHub answers `/issues/new/choose`
    /// with the plain new-issue form when the repository has no templates,
    /// so, as in GHD, nothing is checked locally.
    pub fn create_issue(id: u64, cx: &mut dyn Host) {
        // flags 342-344: the host's new issue page
        if Self::create_issue_on_host(id, cx) {
            return;
        }
        let url = Self::state(cx)
            .read(cx)
            .repository(id)
            .and_then(|r| r.non_fork_github())
            .map(|gh| issue_creation_url(&gh.html_url));
        if let Some(url) = url {
            Self::open_url(&url, cx);
        }
    }

    /// Branch › Compare on GitHub.
    pub fn compare_on_github(id: u64, cx: &mut dyn Host) {
        if Self::compare_on_host(id, cx) {
            return;
        }
        if let Some((gh, Some(branch))) = Self::github_and_branch(id, cx) {
            Self::open_url(
                &format!("{}/compare/{}", gh.html_url, encode_component(&branch)),
                cx,
            );
        }
    }

    /// Branch › View Branch on GitHub.
    pub fn view_branch_on_github(id: u64, cx: &mut dyn Host) {
        if Self::view_branch_on_host(id, cx) {
            return;
        }
        if let Some((gh, Some(branch))) = Self::github_and_branch(id, cx) {
            Self::open_url(
                &format!("{}/tree/{}", gh.html_url, encode_component(&branch)),
                cx,
            );
        }
    }

    /// Branch › Create Pull Request. An unpublished branch is pushed first
    /// (GHD `_createPullRequest` → `_publishBranch`), then the compare page
    /// opens; with an open pull request the menu shows it instead.
    pub fn create_pull_request(id: u64, cx: &mut dyn Host) {
        if Self::state(cx).read(cx).current_pull_request(id).is_some() {
            Self::show_pull_request(id, cx);
            return;
        }
        let base = Self::proposed_pull_request_base(id, cx);
        Self::create_pull_request_with_base(id, base, cx);
    }

    /// The base Create Pull Request and Preview Pull Request propose before
    /// falling back to the default branch: where the branch started
    /// (`333-pr-base-from-branch-origin`), else the base last picked in this
    /// repository (`352-remember-pr-base`).
    pub fn proposed_pull_request_base(id: u64, cx: &dyn Host) -> Option<String> {
        Self::pull_request_base_from_origin(id, cx)
            .or_else(|| Self::remembered_pull_request_base(id, cx))
    }

    /// Deviation (`352-remember-pr-base`; GHD `_startPullRequest` always
    /// proposes the default branch): the base last picked in Preview Pull
    /// Request for this repository, as a remote branch of the current
    /// branch's remote, when it still exists there and is neither the
    /// current nor the default branch.
    pub fn remembered_pull_request_base(id: u64, cx: &dyn Host) -> Option<String> {
        let s = Self::state(cx).read(cx);
        if !s.flags.bool(crate::flags::ids::REMEMBER_PR_BASE) {
            return None;
        }
        let saved = s.repository(id)?.pull_request_base.as_deref()?;
        let rs = s.repo_states.get(&id)?;
        let info = rs.info.as_ref()?;
        let current = info.current_branch()?;
        let remote = current
            .upstream_remote_name()
            .or_else(|| crate::git_store::default_remote_name(info))?;
        pull_request_base_candidate(
            saved,
            &current.name,
            rs.default_branch.as_deref(),
            remote,
            &info.branches,
        )
    }

    /// `352-remember-pr-base`: remember `base` (a branch name from Preview
    /// Pull Request's base picker) for the repository's next pull request;
    /// the default branch forgets the remembered one.
    pub fn remember_pull_request_base(id: u64, base: Option<&str>, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, _| {
            if !s.flags.bool(crate::flags::ids::REMEMBER_PR_BASE) {
                return;
            }
            let Some(rs) = s.repo_states.get(&id) else {
                return;
            };
            let bare = |name: &str| -> String {
                rs.info
                    .as_ref()
                    .and_then(|i| i.branches.iter().find(|b| b.name == name))
                    .map(|b| b.name_without_remote().to_string())
                    .unwrap_or_else(|| name.to_string())
            };
            let default_branch = rs.default_branch.as_deref().map(bare);
            let base = base
                .map(bare)
                .filter(|b| Some(b) != default_branch.as_ref());
            if let Some(repo) = s.repositories.iter_mut().find(|r| r.id == id)
                && repo.pull_request_base != base
            {
                repo.pull_request_base = base;
                crate::dispatcher::persist_repositories(s);
            }
        });
    }

    /// Deviation (`333-pr-base-from-branch-origin`; GHD `_createPullRequest`
    /// / `_startPullRequest` always propose the default branch): the branch
    /// the current branch was created from (`branch.<name>.vscode-merge-base`,
    /// else its reflog's "Created from"), as the remote branch to propose as
    /// the pull request's base, when it is another branch than the default
    /// one and exists on the current branch's remote.
    pub fn pull_request_base_from_origin(id: u64, cx: &dyn Host) -> Option<String> {
        let s = Self::state(cx).read(cx);
        if !s.flags.bool(crate::flags::ids::PR_BASE_FROM_BRANCH_ORIGIN) {
            return None;
        }
        let git = s.git.clone()?;
        let rs = s.repo_states.get(&id)?;
        let info = rs.info.as_ref()?;
        let current = info.current_branch()?;
        let remote = current
            .upstream_remote_name()
            .map(str::to_string)
            .or_else(|| crate::git_store::default_remote_name(info).map(str::to_string))?;
        let origin = corvene_git::branch_merge_base(git, &info.workdir, &current.name)
            .or_else(|| corvene_git::branch_created_from(&info.workdir, &current.name))?;
        pull_request_base_candidate(
            &origin,
            &current.name,
            rs.default_branch.as_deref(),
            &remote,
            &info.branches,
        )
    }

    /// `_createPullRequest(repository, baseBranch)`: an unpublished branch
    /// or unpushed commits ask `PushBranchCommits` first.
    pub fn create_pull_request_with_base(id: u64, base: Option<String>, cx: &mut dyn Host) {
        let Some((_, Some(branch))) = Self::github_and_branch(id, cx)
            .or_else(|| Self::hosted_and_branch(id, cx).map(|(h, b)| (h.repo, b)))
        else {
            return;
        };
        let ahead_behind = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|rs| rs.ahead_behind);
        match ahead_behind {
            None => Self::show_popup(
                Popup::PushBranchCommits {
                    repo: id,
                    branch,
                    unpushed: None,
                    base,
                },
                cx,
            ),
            Some(ab) if ab.ahead > 0 => Self::show_popup(
                Popup::PushBranchCommits {
                    repo: id,
                    branch,
                    unpushed: Some(ab.ahead),
                    base,
                },
                cx,
            ),
            Some(_) => Self::open_create_pull_request_in_browser(id, base, cx),
        }
    }

    /// `PushBranchCommits.onSubmit`: push (or publish) the current branch,
    /// then open the compare page.
    pub fn push_branch_commits_and_create_pull_request(
        id: u64,
        base: Option<String>,
        cx: &mut dyn Host,
    ) {
        use crate::flags::ids;
        use crate::remote::PushOutcome;
        let (keeps_base, error_stops) = {
            let flags = &Self::state(cx).read(cx).flags;
            (
                flags.bool(ids::PUSH_BRANCH_COMMITS_KEEPS_BASE),
                flags.bool(ids::PUSH_BRANCH_COMMITS_ERROR_STOPS),
            )
        };
        // `304`: GHD's `onConfirm` drops the base picked in Preview Pull Request
        let base = base.filter(|_| keeps_base);
        Self::push_then(
            id,
            false,
            None,
            move |outcome, cx| {
                // the prompt closes once the push is done (an error dialog
                // may be on top of it)
                Self::close_popups_where(|p| matches!(p, Popup::PushBranchCommits { .. }), cx);
                // `305`: GHD opens the compare page even after a failed push
                match outcome {
                    PushOutcome::Pushed => Self::open_create_pull_request_in_browser(id, base, cx),
                    PushOutcome::Failed if !error_stops => {
                        Self::open_create_pull_request_in_browser(id, base, cx)
                    }
                    PushOutcome::Failed | PushOutcome::NotAttempted => {}
                }
            },
            cx,
        );
    }

    /// `_openCreatePullRequestInBrowser`
    pub fn open_create_pull_request_in_browser(id: u64, base: Option<String>, cx: &mut dyn Host) {
        // flags 342-344: the host's new pull (merge) request form
        if Self::open_create_host_pull_request(id, base.clone(), cx) {
            return;
        }
        let Some((gh, Some(branch))) = Self::github_and_branch(id, cx) else {
            return;
        };
        let (contributing_to_parent, own_fork_targets_itself, owner_refs) = {
            let s = Self::state(cx).read(cx);
            (
                s.repository(id)
                    .is_some_and(|r| r.is_fork_contributing_to_parent()),
                s.flags.bool(crate::flags::ids::FORK_OWN_PR_TARGET),
                s.flags.bool(crate::flags::ids::PR_URL_OWNER_BRANCH_REFS),
            )
        };
        // the base is a remote branch name in the dialog; GitHub wants it bare
        let base = base.map(|b| {
            b.split_once('/')
                .map(|(_, name)| name.to_string())
                .unwrap_or(b)
        });
        let mut url = pull_request_url(
            &gh,
            &branch,
            base.as_deref(),
            contributing_to_parent,
            own_fork_targets_itself,
            owner_refs,
        );
        // `345-issues`: a branch made from an issue closes it
        if let Some(number) = Self::current_branch_issue(id, cx) {
            url = crate::issues::with_closes_issue(&url, number);
        }
        Self::open_url(&url, cx);
    }

    /// Settings › Git › Hooks: (re)load the login-shell environment for git
    /// subprocesses, or drop it when the option is off.
    pub fn refresh_hook_env(cx: &mut dyn Host) {
        let (enabled, cache) = {
            let s = Self::state(cx).read(cx).settings.clone();
            (s.enable_git_hook_env, s.cache_git_hook_env)
        };
        Self::sync_hooks_interception(cx);
        if !enabled {
            corvene_git::hook_env::clear_hook_env();
            return;
        }
        spawn_bg(
            cx,
            corvene_git::hook_env::load_shell_env,
            move |result, _| match result {
                Ok(env) => {
                    info!(
                        vars = env.len(),
                        "loaded git hook environment from the shell"
                    );
                    corvene_git::hook_env::set_hook_env(env, cache);
                }
                Err(err) => warn!(%err, "could not load the shell environment for git hooks"),
            },
        );
    }

    /// GHD `getHooksEnvEnabled()` also turns hooks interception on.
    /// Corvene `1121-hook-diagnostics-without-shell-env`
    /// (desktop/desktop#23012): interception, which names the failing hook
    /// and shows its output, is on whatever the shell environment option
    /// says.
    pub fn sync_hooks_interception(cx: &mut dyn Host) {
        let s = Self::state(cx).read(cx);
        corvene_git::hooks::set_enabled(
            s.settings.enable_git_hook_env
                || s.flags
                    .bool(crate::flags::ids::HOOK_DIAGNOSTICS_WITHOUT_SHELL_ENV),
        );
    }

    // ---- Settings (`Preferences` popup) ----

    /// Show Settings on `tab`, refresh the installed editors/shells and read the
    /// global git config in the background (`isLoadingGitConfig`).
    pub fn open_preferences(tab: PreferencesTab, cx: &mut dyn Host) {
        Self::close_foldout(cx);
        Self::state(cx).update(cx, |s, cx| {
            s.global_git = None;
            cx.notify();
        });
        Self::show_popup(Popup::Preferences { tab }, cx);
        Self::detect_integrations(cx);
        // Settings › Advanced (and Appearance › Syntax highlighting) list the
        // on-demand packs from the manifest (`105-tree-sitter-highlighting`)
        let wants_manifest = {
            let s = Self::state(cx).read(cx);
            s.packs.manifest.is_none()
                && crate::packs::offered_packs(&s.flags)
                    .iter()
                    .any(|kind| !s.packs.bundled(*kind))
        };
        if wants_manifest {
            Self::refresh_packs_manifest(cx);
        }
        Self::load_global_git_config(cx);
    }

    /// Reads the global git identity and default branch in the background
    /// into `AppState::global_git` (Settings › Git; the Android Settings
    /// screen without the Preferences popup).
    pub fn load_global_git_config(cx: &mut dyn Host) {
        let Some(git) = Self::state(cx).read(cx).git.clone() else {
            return;
        };
        spawn_bg(
            cx,
            move || {
                let identity = corvene_git::global_identity(git.clone());
                let flag = |key: &str, default: bool| {
                    corvene_git::global_config_value(git.clone(), key)
                        .map_or(default, |v| config_bool(&v, default))
                };
                GlobalGitConfig {
                    name: identity.name,
                    email: identity.email,
                    quotepath: flag("core.quotepath", true),
                    longpaths: flag("core.longpaths", false),
                    signing: read_signing(|key| corvene_git::global_config_value(git.clone(), key)),
                    default_branch: corvene_git::configured_default_branch(git),
                }
            },
            |config, cx| {
                Self::state(cx).update(cx, |s, cx| {
                    s.global_git = Some(config);
                    cx.notify();
                });
            },
        );
    }

    /// GHD `onUpdateUserEmail` (`ui/changes/commit-message.tsx`): the commit
    /// form avatar's Update Email sets the global `user.email`. Corvene
    /// `1309-commit-identity-update` passes `name` too, and with `local`
    /// (the repository already has its own identity) writes both to the
    /// repository's config. Empty values are skipped.
    pub fn update_commit_identity(
        id: u64,
        name: Option<String>,
        email: String,
        local: bool,
        cx: &mut dyn Host,
    ) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
        let email = email.trim().to_string();
        spawn_bg(
            cx,
            move || {
                let set = |key: &str, value: &str| {
                    if local {
                        corvene_git::set_local_config_value(git.clone(), &workdir, key, value)
                    } else {
                        corvene_git::set_global_config_value(git.clone(), key, value)
                    }
                };
                if let Some(name) = &name {
                    set("user.name", name)?;
                }
                if !email.is_empty() {
                    set("user.email", &email)?;
                }
                Ok::<_, corvene_git::GitError>(())
            },
            move |result, cx| {
                if let Err(err) = result {
                    Self::show_error("Could not update your Git identity", &err, cx);
                }
                Self::refresh_repository(id, cx);
                Self::load_global_git_config(cx);
            },
        );
    }

    /// Settings › Git › Default branch name, written on its own
    /// (`init.defaultBranch`), then re-read.
    pub fn set_global_default_branch(name: String, cx: &mut dyn Host) {
        let Some(git) = Self::state(cx).read(cx).git.clone() else {
            return;
        };
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        spawn_bg(
            cx,
            move || corvene_git::set_default_branch(git, &name),
            |result, cx| {
                if let Err(err) = result {
                    Self::show_error("Could not set the default branch", &err, cx);
                }
                Self::load_global_git_config(cx);
            },
        );
    }

    /// Settings › Save: persist the settings, then write the global git
    /// identity and default branch when they changed.
    pub fn save_preferences(save: PreferencesSave, cx: &mut dyn Host) {
        let PreferencesSave {
            settings,
            name,
            email,
            default_branch,
            quotepath,
            longpaths,
            signing,
        } = save;
        let (git, previous) = {
            let s = Self::state(cx).read(cx);
            (s.git.clone(), s.global_git.clone().unwrap_or_default())
        };
        Self::update_settings(cx, |s| *s = settings);
        Self::close_popup(cx);
        Self::refresh_indicators(cx);
        Self::refresh_hook_env(cx);
        let Some(git) = git else { return };
        let name_changed = name.trim() != previous.name.clone().unwrap_or_default().trim();
        let email_changed = email.trim() != previous.email.clone().unwrap_or_default().trim();
        let branch_changed =
            !default_branch.trim().is_empty() && default_branch.trim() != previous.default_branch;
        let quotepath = quotepath.filter(|v| *v != previous.quotepath);
        let longpaths = longpaths.filter(|v| *v != previous.longpaths);
        let signing = signing.filter(|s| *s != previous.signing);
        let previous_signing = previous.signing.clone();
        if !(name_changed || email_changed || branch_changed)
            && quotepath.is_none()
            && longpaths.is_none()
            && signing.is_none()
        {
            return;
        }
        let selected = Self::state(cx).read(cx).selected;
        spawn_bg(
            cx,
            move || -> Result<(), corvene_git::GitError> {
                if name_changed {
                    corvene_git::set_global_config_value(git.clone(), "user.name", name.trim())?;
                }
                if email_changed {
                    corvene_git::set_global_config_value(git.clone(), "user.email", email.trim())?;
                }
                if let Some(on) = quotepath {
                    corvene_git::set_global_config_value(
                        git.clone(),
                        "core.quotepath",
                        if on { "true" } else { "false" },
                    )?;
                }
                if let Some(on) = longpaths {
                    corvene_git::set_global_config_value(
                        git.clone(),
                        "core.longpaths",
                        if on { "true" } else { "false" },
                    )?;
                }
                if let Some(signing) = &signing {
                    write_signing(git.clone(), None, &previous_signing, signing)?;
                }
                if branch_changed {
                    corvene_git::set_default_branch(git, default_branch.trim())?;
                }
                Ok(())
            },
            move |result, cx| {
                if let Err(err) = result {
                    Self::show_error("Could not save Git configuration", &err, cx);
                }
                if let Some(id) = selected {
                    Self::refresh_repository(id, cx);
                }
            },
        );
    }

    // ---- Repository Settings ----

    /// Show Repository Settings on `tab`; the remote, `.gitignore` and git
    /// config are read in the background.
    pub fn open_repository_settings(id: u64, tab: RepositorySettingsTab, cx: &mut dyn Host) {
        Self::close_foldout(cx);
        Self::state(cx).update(cx, |s, cx| {
            s.repo_settings = None;
            cx.notify();
        });
        Self::show_popup(Popup::RepositorySettings { repo: id, tab }, cx);
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let (remotes, branch) = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|rs| rs.info.as_ref())
            .map(|info| {
                (
                    info.remotes.clone(),
                    info.current_branch().map(|b| b.name.clone()),
                )
            })
            .unwrap_or_default();
        spawn_bg(
            cx,
            move || {
                let remote = corvene_git::find_default_remote(&remotes).cloned();
                let push_config = crate::remote_manager::read_push_config(
                    git.clone(),
                    &workdir,
                    branch.as_deref(),
                );
                let gitignore = corvene_git::read_gitignore(&workdir)
                    .map_err(|err| warn!(%err, "could not read .gitignore"))
                    .ok()
                    .flatten();
                let global = corvene_git::global_identity(git.clone());
                let autocrlf = corvene_git::config_value(git.clone(), &workdir, "core.autocrlf")
                    .is_some_and(|v| v.eq_ignore_ascii_case("true"));
                RepositorySettingsData {
                    repo: id,
                    remote,
                    gitignore,
                    local_name: corvene_git::local_config_value(git.clone(), &workdir, "user.name"),
                    local_email: corvene_git::local_config_value(
                        git.clone(),
                        &workdir,
                        "user.email",
                    ),
                    global,
                    autocrlf,
                    local_autocrlf: corvene_git::local_config_value(
                        git.clone(),
                        &workdir,
                        "core.autocrlf",
                    ),
                    local_signing: read_signing(|key| {
                        corvene_git::config_value(git.clone(), &workdir, key)
                    }),
                    remotes,
                    push_config,
                }
            },
            |data, cx| {
                Self::state(cx).update(cx, |s, cx| {
                    s.repo_settings = Some(data);
                    cx.notify();
                });
            },
        );
    }

    /// Repository Settings › Save.
    pub fn save_repository_settings(id: u64, save: RepositorySettingsSave, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            Self::close_popup(cx);
            return;
        };
        let (autocrlf, push_config_before) = Self::state(cx)
            .read(cx)
            .repo_settings
            .as_ref()
            .map(|d| (d.autocrlf, d.push_config.clone()))
            .unwrap_or_default();
        Self::close_popup(cx);
        spawn_bg(
            cx,
            move || {
                let mut errors: Vec<String> = Vec::new();
                // `1109-remote-manager`: the remotes, then where pushes go
                // (which may name a remote just added or renamed)
                errors.extend(crate::remote_manager::apply_remote_edits(
                    git.clone(),
                    &workdir,
                    &save.remote_edits,
                ));
                if let Some(after) = &save.push_config
                    && let Err(err) = crate::remote_manager::write_push_config(
                        git.clone(),
                        &workdir,
                        &push_config_before,
                        after,
                    )
                {
                    errors.push(format!("Failed saving where pushes go: {err}"));
                }
                if let Some((name, url)) = save.remote_url
                    && let Err(err) =
                        corvene_git::set_remote_url(git.clone(), &workdir, &name, &url)
                {
                    errors.push(format!("Failed setting the remote URL: {err}"));
                }
                if let Some(text) = save.gitignore
                    && let Err(err) = corvene_git::save_gitignore(&workdir, &text, autocrlf)
                {
                    errors.push(format!("Failed saving the .gitignore file: {err}"));
                }
                if let Some((location, name, email)) = save.git_config {
                    let result = match location {
                        GitConfigLocation::Global => corvene_git::remove_local_config_value(
                            git.clone(),
                            &workdir,
                            "user.name",
                        )
                        .and_then(|_| {
                            corvene_git::remove_local_config_value(
                                git.clone(),
                                &workdir,
                                "user.email",
                            )
                        }),
                        GitConfigLocation::Local => corvene_git::set_local_config_value(
                            git.clone(),
                            &workdir,
                            "user.name",
                            &name,
                        )
                        .and_then(|_| {
                            corvene_git::set_local_config_value(
                                git.clone(),
                                &workdir,
                                "user.email",
                                &email,
                            )
                        }),
                    };
                    if let Err(err) = result {
                        errors.push(format!("Failed saving the Git config: {err}"));
                    }
                }
                if let Some(signing) = save.signing {
                    // what applies here now; a change is stored locally
                    let before =
                        read_signing(|key| corvene_git::config_value(git.clone(), &workdir, key));
                    if let Err(err) = write_signing(git.clone(), Some(&workdir), &before, &signing)
                    {
                        errors.push(format!("Failed saving the signing settings: {err}"));
                    }
                }
                if let Some(value) = save.autocrlf {
                    let result = match value {
                        Some(value) => corvene_git::set_local_config_value(
                            git.clone(),
                            &workdir,
                            "core.autocrlf",
                            &value,
                        ),
                        None => corvene_git::remove_local_config_value(
                            git.clone(),
                            &workdir,
                            "core.autocrlf",
                        ),
                    };
                    if let Err(err) = result {
                        errors.push(format!("Failed saving the line ending setting: {err}"));
                    }
                }
                errors
            },
            move |errors, cx| {
                if !errors.is_empty() {
                    let title = if cfg!(target_os = "macos") {
                        "Repository Settings"
                    } else {
                        "Repository settings"
                    };
                    Self::show_error(title, errors.join("\n"), cx);
                }
                Self::refresh_repository(id, cx);
            },
        );
    }

    /// The identity the commit form should show for a repository: local
    /// config first, then global (what `git commit` would use).
    pub fn effective_identity(data: &RepositorySettingsData) -> Identity {
        Identity {
            name: data.local_name.clone().or_else(|| data.global.name.clone()),
            email: data
                .local_email
                .clone()
                .or_else(|| data.global.email.clone()),
            local: data.local_name.is_some() || data.local_email.is_some(),
        }
    }

    // ---- removal ----

    /// Repository › Remove…: confirm first unless the prompt is turned off
    /// or the repository is missing (GHD `App.removeRepository`).
    pub fn request_remove_repository(id: u64, cx: &mut dyn Host) {
        let s = Self::state(cx).read(cx);
        let missing = s.repository(id).is_some_and(|r| r.missing);
        let confirm = s.settings.confirm_repository_removal && !missing;
        if confirm {
            Self::close_foldout(cx);
            Self::show_popup(Popup::ConfirmRemoveRepository { repo: id }, cx);
        } else {
            Self::remove_repository(id, cx);
        }
    }

    /// Remove from the list and move the directory to the Trash
    /// (`_removeRepository` with `moveToTrash`).
    pub fn remove_repository_and_trash(id: u64, cx: &mut dyn Host) {
        let path = Self::state(cx)
            .read(cx)
            .repository(id)
            .map(|r| r.path.clone());
        Self::remove_repository(id, cx);
        let Some(path) = path else { return };
        spawn_bg(
            cx,
            move || trash::move_to_trash(&path).map_err(|e| (path, e)),
            |result, cx| {
                if let Err((path, err)) = result {
                    error!(%err, path = %path.display(), "could not move repository to Trash");
                    Self::show_error(
                        if cfg!(windows) {
                            "Unable to Move Repository to Recycle Bin"
                        } else {
                            "Unable to Move Repository to Trash"
                        },
                        format!("{}: {err}", path.display()),
                        cx,
                    );
                }
            },
        );
    }

    /// Settings › Git › "edit your global Git config file" (GHD
    /// `_editGlobalGitConfig`): open the global config file git uses
    /// (`corvene_git::global_config_path`, which creates it) in the external
    /// editor.
    pub fn edit_global_git_config(cx: &mut dyn Host) {
        let Some(git) = Self::state(cx).read(cx).git.clone() else {
            return;
        };
        spawn_bg(
            cx,
            move || corvene_git::global_config_path(git),
            |result, cx| match result {
                Ok(path) => Self::open_in_editor(path, cx),
                Err(err) => warn!(%err, "could not open the global Git config for editing"),
            },
        );
    }
}

/// A git config boolean (`git-config` "Values": true / yes / on / 1 and
/// false / no / off / 0 / empty); `default` for anything else.
fn config_bool(value: &str, default: bool) -> bool {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => true,
        "false" | "no" | "off" | "0" | "" => false,
        _ => default,
    }
}

/// Where a file as of commit `short_sha` is written (flag `811`):
/// `<tmp>/corvene-history/<short sha>/<repository-relative path>`. `None` for
/// a path that would leave that directory.
fn historical_file_path(tmp: &Path, short_sha: &str, path: &str) -> Option<PathBuf> {
    use std::path::Component;
    let relative = Path::new(path);
    if path.is_empty()
        || !relative
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
    {
        return None;
    }
    Some(tmp.join("corvene-history").join(short_sha).join(relative))
}

/// Write `bytes` to `file` (replacing an earlier copy) and make it read-only.
fn write_read_only(file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if let Ok(meta) = std::fs::metadata(file) {
        let mut perms = meta.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(file, perms)?;
    }
    std::fs::write(file, bytes)?;
    let mut perms = std::fs::metadata(file)?.permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(file, perms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_git_config_booleans() {
        assert!(config_bool("Yes", false));
        assert!(!config_bool("off", true));
        assert!(config_bool("maybe", true));
    }

    #[test]
    fn explore_is_on_the_repository_host() {
        assert_eq!(
            github_explore_url("https://github.com/octocat/hello").as_deref(),
            Some("https://github.com/explore")
        );
        assert_eq!(
            github_explore_url("https://ghe.example.com:8443/org/repo").as_deref(),
            Some("https://ghe.example.com:8443/explore")
        );
        assert_eq!(github_explore_url("not a url"), None);
    }

    fn gh(parent: bool) -> GitHubRepository {
        let base = GitHubRepository {
            endpoint: "https://api.github.com".into(),
            owner: "octocat".into(),
            name: "hello".into(),
            html_url: "https://github.com/octocat/hello".into(),
            clone_url: "https://github.com/octocat/hello.git".into(),
            default_branch: Some("main".into()),
            private: false,
            fork: parent,
            parent: None,
            archived: false,
            permissions: None,
            allow_forking: None,
            node_id: None,
        };
        if parent {
            GitHubRepository {
                owner: "me".into(),
                html_url: "https://github.com/me/hello".into(),
                parent: Some(Box::new(base.clone())),
                ..base
            }
        } else {
            base
        }
    }

    #[test]
    fn remote_web_urls() {
        for (remote, web) in [
            (
                "https://gitlab.com/group/sub/proj.git",
                Some("https://gitlab.com/group/sub/proj"),
            ),
            (
                "https://user:tok@git.corp:8443/team/repo/",
                Some("https://git.corp:8443/team/repo"),
            ),
            (
                "http://gitea.local/me/repo",
                Some("http://gitea.local/me/repo"),
            ),
            (
                "git@bitbucket.org:team/repo.git",
                Some("https://bitbucket.org/team/repo"),
            ),
            (
                "ssh://git@git.corp:2222/team/repo.git",
                Some("https://git.corp/team/repo"),
            ),
            (
                "git://example.org/repo.git",
                Some("https://example.org/repo"),
            ),
            ("/srv/git/repo.git", None),
            ("https://host.example/", None),
        ] {
            assert_eq!(remote_web_url(remote).as_deref(), web, "{remote}");
        }
    }

    #[test]
    fn issues_are_created_on_the_contribution_target() {
        let mut repo = corvene_models::Repository::new(1, std::path::PathBuf::from("/tmp/hello"));
        repo.github = Some(gh(false));
        let url = repo
            .non_fork_github()
            .map(|g| issue_creation_url(&g.html_url));
        assert_eq!(
            url.as_deref(),
            Some("https://github.com/octocat/hello/issues/new/choose")
        );
        // a fork files issues on its parent by default (GHD #9232)
        repo.github = Some(gh(true));
        let url = repo
            .non_fork_github()
            .map(|g| issue_creation_url(&g.html_url));
        assert_eq!(
            url.as_deref(),
            Some("https://github.com/octocat/hello/issues/new/choose")
        );
    }

    #[test]
    fn encodes_branch_names() {
        assert_eq!(encode_component("feature/x y"), "feature%2Fx%20y");
        assert_eq!(encode_component("main"), "main");
    }

    #[test]
    fn pull_request_base_comes_from_where_the_branch_started() {
        let branch = |name: &str, kind: corvene_models::BranchKind| corvene_models::Branch {
            name: name.into(),
            kind,
            full_name: String::new(),
            tip: None,
            upstream: None,
            tip_time: None,
            tip_author: None,
            remote_name: None,
        };
        use corvene_models::BranchKind::{Local, Remote};
        let branches = vec![
            branch("main", Local),
            branch("feature", Local),
            branch("stacked", Local),
            branch("origin/main", Remote),
            branch("origin/feature", Remote),
        ];
        let base = |origin: &str| {
            pull_request_base_candidate(origin, "stacked", Some("main"), "origin", &branches)
        };
        assert_eq!(base("feature").as_deref(), Some("origin/feature"));
        assert_eq!(base("origin/feature").as_deref(), Some("origin/feature"));
        assert_eq!(
            base("refs/remotes/origin/feature").as_deref(),
            Some("origin/feature")
        );
        assert_eq!(base("main"), None);
        assert_eq!(base("origin/main"), None);
        assert_eq!(base("HEAD"), None);
        assert_eq!(base("stacked"), None);
        // not on the remote, or a commit
        assert_eq!(base("unpublished"), None);
        assert_eq!(base("1234abcd"), None);
    }

    #[test]
    fn pull_request_urls() {
        assert_eq!(
            pull_request_url(&gh(false), "feat/one", None, false, false, false),
            "https://github.com/octocat/hello/pull/new/feat%2Fone"
        );
        assert_eq!(
            pull_request_url(&gh(false), "feat", Some("develop"), false, false, false),
            "https://github.com/octocat/hello/pull/new/develop...feat"
        );
        assert_eq!(
            pull_request_url(&gh(true), "feat", None, true, false, false),
            "https://github.com/me/hello/pull/new/me:hello:feat"
        );
        assert_eq!(
            pull_request_url(&gh(true), "feat", Some("main"), true, false, false),
            "https://github.com/me/hello/pull/new/octocat:hello:main...me:hello:feat"
        );
        assert_eq!(
            pull_request_url(&gh(true), "feat", None, false, false, false),
            "https://github.com/me/hello/pull/new/feat"
        );
        // `372`: a fork for its own work targets itself
        assert_eq!(
            pull_request_url(&gh(true), "feat", None, false, true, false),
            "https://github.com/me/hello/pull/new/me:hello:main...me:hello:feat"
        );
        assert_eq!(
            pull_request_url(&gh(true), "feat", Some("dev"), false, true, false),
            "https://github.com/me/hello/pull/new/me:hello:dev...me:hello:feat"
        );
        // contributing to the parent and non-forks are unchanged by the flag
        assert_eq!(
            pull_request_url(&gh(true), "feat", None, true, true, false),
            "https://github.com/me/hello/pull/new/me:hello:feat"
        );
        assert_eq!(
            pull_request_url(&gh(false), "feat", None, false, true, false),
            "https://github.com/octocat/hello/pull/new/feat"
        );
        // `332`: owner-only prefixes
        assert_eq!(
            pull_request_url(&gh(true), "feat", Some("main"), true, false, true),
            "https://github.com/me/hello/pull/new/octocat:main...me:feat"
        );
        assert_eq!(
            pull_request_url(&gh(true), "feat", None, false, true, true),
            "https://github.com/me/hello/pull/new/me:main...me:feat"
        );
        assert_eq!(
            pull_request_url(&gh(false), "feat", Some("dev"), false, false, true),
            "https://github.com/octocat/hello/pull/new/dev...feat"
        );
    }
}

#[cfg(test)]
mod historical_file_tests {
    use super::*;

    #[test]
    fn stays_inside_the_temporary_directory() {
        let tmp = Path::new("/tmp");
        assert_eq!(
            historical_file_path(tmp, "abc", "src/a.png"),
            Some(PathBuf::from("/tmp/corvene-history/abc/src/a.png"))
        );
        assert_eq!(historical_file_path(tmp, "abc", "../x"), None);
        assert_eq!(historical_file_path(tmp, "abc", "/etc/x"), None);
        assert_eq!(historical_file_path(tmp, "abc", ""), None);
    }

    #[test]
    fn rewrites_a_read_only_copy() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a/b.txt");
        write_read_only(&file, b"one").unwrap();
        assert!(std::fs::metadata(&file).unwrap().permissions().readonly());
        write_read_only(&file, b"two").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"two");
        assert!(std::fs::metadata(&file).unwrap().permissions().readonly());
    }
}
