//! Worktrees (GHD `app-store.ts` `_switchWorktree`, `_deleteWorktree`,
//! `_moveWorktree`, `_requestDeleteWorktree`, `recoverMissingWorktree`): a
//! repository entry keeps its id and follows the worktree path it is switched
//! to, and remembers its main worktree (`mainWorktreePath`) so that when the
//! linked worktree it points at is deleted outside Corvene it falls back to
//! the main worktree instead of showing a stale repository.
//!
//! Deviation (`291-recent-worktrees`): the worktrees each repository was
//! used in are remembered ([`crate::AppState::recent_worktrees`]) so the
//! repository list's Recent group can list them (GHD's recent repositories
//! are ids only, `app-store.ts` `updateRecentRepositories`).
//!
//! Deviation: GHD records `mainWorktreePath` when it switches worktrees and
//! otherwise asks the worktree's git dir; Corvene also records it on every
//! refresh (from `git worktree list`), since `git worktree remove` deletes the
//! git dir metadata GHD's fallback reads.

use std::path::{Path, PathBuf};

use crate::host::Host;
use corvene_git::open_repository;
use corvene_models::{BranchKind, WorktreeType};
use tracing::{info, warn};

use crate::dispatcher::Dispatcher;
use crate::remote::spawn_bg;
use crate::state::{DeferredCheckout, DeleteWorktreeOptions, Popup};

/// Where new worktrees go by default under the `241-worktree-location`
/// template: `{clone-dir}` is Settings' clone directory, `{repo}` the
/// repository's name, and a leading `~` the home directory. The flag's
/// default, `{clone-dir}`, is GHD's behaviour.
pub fn worktree_location(
    template: &str,
    clone_dir: &Path,
    repo: &str,
    home: Option<&Path>,
) -> PathBuf {
    let template = template.trim();
    if template.is_empty() {
        return clone_dir.to_path_buf();
    }
    let expanded = template
        .replace("{clone-dir}", &clone_dir.to_string_lossy())
        .replace("{repo}", repo);
    match (expanded.strip_prefix('~'), home) {
        (Some(rest), Some(home))
            if rest.is_empty()
                || rest.starts_with('/')
                || (cfg!(windows) && rest.starts_with('\\')) =>
        {
            home.join(rest.trim_start_matches(['/', '\\']))
        }
        _ => PathBuf::from(expanded),
    }
}

impl Dispatcher {
    /// GHD `_switchWorktree`: point the repository at `path` and reload it.
    /// GHD 3.6.8 (desktop/desktop#22607): a worktree whose folder is gone
    /// offers its removal instead.
    pub fn switch_worktree(id: u64, path: PathBuf, cx: &mut dyn Host) {
        if Self::worktree_is_missing(id, &path, cx) {
            return Self::request_delete_worktree_with(id, path, None, cx);
        }
        let probe = path.clone();
        spawn_bg(
            cx,
            move || open_repository(&probe).map_err(|e| e.to_string()),
            move |result, cx| {
                let info = match result {
                    Ok(info) => info,
                    Err(err) => {
                        // `291-recent-worktrees`: not offered again
                        Self::state(cx).update(cx, |s, cx| {
                            s.forget_recent_worktrees(id, Some(&path));
                            cx.notify();
                        });
                        Self::show_error(
                            "Could not switch worktree",
                            format!(
                                "The worktree path '{}' does not appear to be a valid Git repository.\n{err}",
                                path.display()
                            ),
                            cx,
                        );
                        return;
                    }
                };
                Self::apply_worktree_path(id, path, Some(info), cx);
            },
        );
    }

    /// Repoint the repository entry (persisted, GHD
    /// `RepositoriesStore.switchWorktree`) and refresh + rewatch it. When
    /// another entry already has `path`, that one is selected instead.
    ///
    /// Deviation: GHD keeps showing the old worktree's state until its
    /// refresh finishes, behind any refresh already running. Here the
    /// running refresh is abandoned, the old worktree's changes, selection
    /// and diff are dropped at once (a commit must not see them), and `info`
    /// (read while checking the path) shows the new branch right away.
    fn apply_worktree_path(
        id: u64,
        path: PathBuf,
        info: Option<corvene_models::RepositoryInfo>,
        cx: &mut dyn Host,
    ) {
        let switched = Self::state(cx).update(cx, |s, cx| {
            // the main worktree from the last refresh; `None` keeps the
            // recorded one
            let main = s
                .repo_states
                .get(&id)
                .and_then(|rs| rs.worktrees.iter().find(|w| w.kind == WorktreeType::Main))
                .map(|w| w.path.clone());
            let repo = s.repository(id)?.clone();
            if repo.path == path {
                return None;
            }
            info!(id, path = %path.display(), "switching worktree");
            let result =
                s.repositories_store()
                    .switch_worktree(&repo, &path, false, main.as_deref());
            if result.existing_repository {
                return Some(Err(result.repository.id));
            }
            // force the file watcher onto the new directory
            s.watchers.remove(&id);
            let slash_remotes = s.flags.bool(crate::flags::ids::REMOTE_NAMES_WITH_SLASHES);
            let rs = s.repo_state_mut(id);
            rs.loading = false;
            rs.refresh_pending = false;
            rs.refresh_started = None;
            rs.status = None;
            rs.line_stats = Default::default();
            rs.selected_file = None;
            rs.selected_files.clear();
            rs.diff = None;
            rs.diff_contents = None;
            rs.diff_old_contents = None;
            rs.diff_generation += 1;
            if let Some(mut info) = info {
                if !slash_remotes {
                    crate::dispatcher::forget_remote_names(&mut info);
                }
                rs.info = Some(info);
            }
            cx.notify();
            Some(Ok(()))
        });
        match switched {
            Some(Ok(())) => {
                Self::state(cx).update(cx, |s, _| s.record_recent_worktree(id));
                Self::refresh_repository(id, cx);
                Self::start_watching(id, cx);
            }
            // GHD `_switchWorktree` selects the repository `switchWorktree`
            // returned
            Some(Err(existing)) => Self::select_repository(existing, cx),
            None => {}
        }
    }

    /// Corvene (`291-recent-worktrees`): a Recent row of the repository
    /// list: select the repository and switch it to the worktree at `path`
    /// unless it is there already.
    pub fn select_recent_worktree(id: u64, path: PathBuf, cx: &mut dyn Host) {
        Self::select_repository(id, cx);
        let current = Self::state(cx)
            .read(cx)
            .repository(id)
            .map(|r| r.path.clone());
        if current.is_some_and(|current| current != path) {
            Self::switch_worktree(id, path, cx);
        }
    }

    /// GHD `AddWorktreeDialog.onSubmit`: create the worktree for `branch`
    /// (checked out if it exists locally or on a remote, created otherwise)
    /// and switch to it.
    pub fn add_worktree(id: u64, path: PathBuf, branch: String, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let branches = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|rs| rs.info.as_ref())
            .map(|i| i.branches.clone())
            .unwrap_or_default();
        let existing = branches.iter().find(|b| b.name == branch).cloned();
        let target = path.clone();
        spawn_bg(
            cx,
            move || {
                let (create, commitish) = match &existing {
                    Some(b) if b.kind == BranchKind::Remote => (
                        Some(b.name_without_remote().to_string()),
                        Some(b.full_name.clone()),
                    ),
                    Some(b) => (None, Some(b.name.clone())),
                    None => (Some(branch.clone()), None),
                };
                corvene_git::add_worktree(
                    git.clone(),
                    &workdir,
                    &target,
                    create.as_deref(),
                    commitish.as_deref(),
                )
                .map_err(|e| e.to_string())?;
                let worktrees =
                    corvene_git::list_worktrees(git, &workdir).map_err(|e| e.to_string())?;
                if !worktrees.iter().any(|w| same_path(&w.path, &target)) {
                    return Err("Failed to find the newly created worktree".to_string());
                }
                Ok(target)
            },
            move |result, cx| match result {
                Ok(path) => {
                    Self::close_popups_where(|p| matches!(p, Popup::AddWorktree { .. }), cx);
                    Self::apply_worktree_path(id, path, None, cx);
                }
                Err(err) => Self::show_error("Could not create worktree", err, cx),
            },
        );
    }

    /// Whether git lists the worktree at `path` as `prunable` (its folder
    /// is gone).
    pub(crate) fn worktree_is_missing(id: u64, path: &Path, cx: &mut dyn Host) -> bool {
        Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|rs| rs.worktrees.iter().find(|w| same_path(&w.path, path)))
            .is_some_and(|w| w.is_prunable)
    }

    /// GHD `_requestDeleteWorktree`: confirm unless the user opted out.
    pub fn request_delete_worktree(id: u64, path: PathBuf, cx: &mut dyn Host) {
        Self::request_delete_worktree_with(id, path, None, cx);
    }

    /// GHD 3.6.8 `_requestDeleteWorktree(repository, path, checkout)`: a
    /// missing worktree is always confirmed, and `checkout` runs once it is
    /// gone.
    pub fn request_delete_worktree_with(
        id: u64,
        path: PathBuf,
        checkout: Option<DeferredCheckout>,
        cx: &mut dyn Host,
    ) {
        let options = DeleteWorktreeOptions {
            force: false,
            missing: Self::worktree_is_missing(id, &path, cx),
            checkout,
        };
        if Self::state(cx).read(cx).settings.confirm_worktree_removal || options.missing {
            Self::show_popup(
                Popup::DeleteWorktree {
                    repo: id,
                    path,
                    options,
                },
                cx,
            );
        } else {
            Self::delete_worktree(id, path, options, cx);
        }
    }

    /// GHD `_deleteWorktree`: switch to the main worktree first when the
    /// current one is being removed; a failure offers `--force`. GHD 3.6.8:
    /// a missing worktree whose folder came back is left in place, and the
    /// deferred checkout runs after either.
    pub fn delete_worktree(
        id: u64,
        path: PathBuf,
        options: DeleteWorktreeOptions,
        cx: &mut dyn Host,
    ) {
        let (current, main) = {
            let s = Self::state(cx).read(cx);
            let Some(repo) = s.repository(id) else { return };
            let main = s
                .repo_states
                .get(&id)
                .and_then(|rs| rs.worktrees.iter().find(|w| w.kind == WorktreeType::Main))
                .map(|w| w.path.clone());
            (repo.path.clone(), main)
        };
        let deleting_current = same_path(&current, &path);
        let original = deleting_current.then(|| current.clone());
        let base = if deleting_current {
            match main {
                Some(main) => {
                    Self::apply_worktree_path(id, main.clone(), None, cx);
                    main
                }
                None => {
                    Self::show_error(
                        "Could not delete worktree",
                        "Could not find main worktree",
                        cx,
                    );
                    return;
                }
            }
        } else {
            current
        };
        let Some(git) = Self::state(cx).read(cx).git.clone() else {
            return;
        };
        let target = path.clone();
        let (force, missing) = (options.force, options.missing);
        spawn_bg(
            cx,
            move || {
                if missing {
                    let still_missing = corvene_git::list_worktrees(git.clone(), &base)
                        .map_err(|e| e.to_string())?
                        .iter()
                        .any(|w| same_path(&w.path, &target) && w.is_prunable);
                    if !still_missing {
                        return Ok(false);
                    }
                }
                corvene_git::remove_worktree(git, &base, &target, force)
                    .map(|()| true)
                    .map_err(|e| e.to_string())
            },
            move |result, cx| {
                match result {
                    Ok(true) => {
                        info!(id, path = %path.display(), "worktree removed");
                        // the checkout below must not find it again
                        Self::state(cx).update(cx, |s, cx| {
                            if let Some(rs) = s.repo_states.get_mut(&id) {
                                rs.worktrees.retain(|w| !same_path(&w.path, &path));
                            }
                            cx.notify();
                        });
                    }
                    Ok(false) => {
                        // the checkout below switches to it instead
                        Self::state(cx).update(cx, |s, cx| {
                            if let Some(w) = s.repo_states.get_mut(&id).and_then(|rs| {
                                rs.worktrees.iter_mut().find(|w| same_path(&w.path, &path))
                            }) {
                                w.is_prunable = false;
                            }
                            cx.notify();
                        });
                        if options.checkout.is_none() {
                            let name = path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            Self::show_error(
                                "Could not delete worktree",
                                format!(
                                    "The worktree {name} is available again and was left in place."
                                ),
                                cx,
                            );
                        }
                    }
                    Err(error) => {
                        warn!(%error, "worktree removal failed");
                        Self::show_popup(
                            Popup::DeleteWorktreeFailed {
                                repo: id,
                                path,
                                error,
                                original,
                                options,
                            },
                            cx,
                        );
                        Self::refresh_repository(id, cx);
                        return;
                    }
                }
                Self::refresh_repository(id, cx);
                // GHD 3.6.8 `checkoutAfterRemoval`
                if let Some(checkout) = options.checkout {
                    Self::checkout_branch(id, checkout.branch, checkout.strategy, cx);
                }
            },
        );
    }

    /// GHD `_moveWorktree` (Rename…): the current worktree follows its new path.
    pub fn move_worktree(id: u64, old: PathBuf, new: PathBuf, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let (old_for_task, new_for_task) = (old.clone(), new.clone());
        spawn_bg(
            cx,
            move || {
                corvene_git::move_worktree(git, &workdir, &old_for_task, &new_for_task)
                    .map_err(|e| e.to_string())
            },
            move |result, cx| match result {
                Ok(()) => {
                    Self::close_popups_where(|p| matches!(p, Popup::RenameWorktree { .. }), cx);
                    let current = Self::state(cx)
                        .read(cx)
                        .repository(id)
                        .map(|r| r.path.clone());
                    if current.is_some_and(|c| same_path(&c, &old)) {
                        Self::apply_worktree_path(id, new, None, cx);
                    } else {
                        Self::refresh_repository(id, cx);
                    }
                }
                Err(err) => Self::show_error("Could not rename worktree", err, cx),
            },
        );
    }
}

impl Dispatcher {
    /// GHD `recoverMissingWorktree`, run by `refresh_repository` when the
    /// repository's path no longer exists: switch the entry to its main
    /// worktree (the recorded `main_worktree_path`, else the main worktree of
    /// the last `git worktree list`), or select the repository entry that
    /// already points there. With no main worktree to go to, the repository
    /// is marked missing as before.
    pub(crate) fn recover_missing_worktree(id: u64, missing_path: PathBuf, cx: &mut dyn Host) {
        let (main, existing) = {
            let s = Self::state(cx).read(cx);
            let listed = s
                .repo_states
                .get(&id)
                .and_then(|rs| rs.worktrees.iter().find(|w| w.kind == WorktreeType::Main))
                .map(|w| w.path.clone());
            // `resolveMainWorktreePath` without a git dir (Corvene keeps
            // none, so no git runs here); the last `git worktree list` stands
            // in for GHD's git-dir fallback
            let resolved = s.git.clone().zip(s.repository(id)).and_then(|(git, r)| {
                corvene_git::resolve_main_worktree_path(git, r, None)
                    .ok()
                    .flatten()
            });
            let main = resolved.or_else(|| listed.filter(|p| *p != missing_path && p.exists()));
            let existing = main.as_ref().and_then(|main| {
                s.repositories
                    .iter()
                    .find(|r| r.id != id && same_path(&r.path, main))
                    .map(|r| r.id)
            });
            (main, existing)
        };
        let Some(main) = main else {
            Self::mark_missing(id, cx);
            return;
        };
        let probe = main.clone();
        spawn_bg(
            cx,
            move || open_repository(&probe).map(|_| ()),
            move |result, cx| {
                if result.is_err() {
                    Self::mark_missing(id, cx);
                    return;
                }
                info!(
                    id,
                    from = %missing_path.display(),
                    to = %main.display(),
                    "worktree is gone, falling back to the main worktree"
                );
                match existing {
                    // `switchWorktree` found the main worktree already listed
                    Some(other) => {
                        if Self::state(cx).read(cx).selected == Some(id) {
                            Self::select_repository(other, cx);
                        }
                    }
                    None => Self::apply_worktree_path(id, main, None, cx),
                }
            },
        );
    }

    /// `_updateRepositoryMissing(repository, true)`.
    fn mark_missing(id: u64, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            s.repo_state_mut(id).error = Some("repository is missing".into());
            if let Some(repo) = s.repositories.iter_mut().find(|r| r.id == id) {
                repo.missing = true;
            }
            cx.notify();
        });
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[cfg(test)]
mod location_tests {
    use super::worktree_location;
    use std::path::{Path, PathBuf};

    #[test]
    fn worktree_location_expands_the_template() {
        let clone = Path::new("/Users/me/GitHub");
        let home = Some(Path::new("/Users/me"));
        assert_eq!(worktree_location("{clone-dir}", clone, "app", home), clone);
        assert_eq!(worktree_location("", clone, "app", home), clone);
        assert_eq!(
            worktree_location("~/code/worktrees/{repo}", clone, "app", home),
            PathBuf::from("/Users/me/code/worktrees/app")
        );
        assert_eq!(
            worktree_location("{clone-dir}/{repo}-worktrees", clone, "app", home),
            PathBuf::from("/Users/me/GitHub/app-worktrees")
        );
        assert_eq!(
            worktree_location("~", clone, "app", home),
            PathBuf::from("/Users/me")
        );
        assert_eq!(
            worktree_location("~other/x", clone, "app", home),
            PathBuf::from("~other/x")
        );
    }
}
