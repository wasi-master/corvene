//! Corvene `1311-stacked-diff`: every changed file's diff in one scrolling
//! list. GitHub Desktop shows one file's diff at a time (`ui/history/
//! selected-commits.tsx` `renderDiff`, `ui/repository.tsx`
//! `renderContentForChanges`), so a commit touching many files is read by
//! clicking through them (desktop/desktop#5218) and the files about to be
//! committed cannot be looked over as a whole (desktop/desktop#18184).
//!
//! The diffs of a stack are loaded one file at a time, on demand: the view
//! asks for a file's diff when its header scrolls into reach
//! ([`Dispatcher::load_stacked_diff`]), so a commit with a thousand files
//! costs one `git` process per file shown, not a thousand up front. Each
//! file's result is kept in [`StackedDiffs`] with its own generation, and
//! the same caches as the single-file diff (`diff_cache`) answer repeats
//! and refreshes without a process. A refresh reloads the working-directory
//! entries like the selected file's diff; a change of the hide-whitespace
//! setting drops them.

use std::collections::HashMap;
use std::sync::Arc;

use corvene_models::{CommittedFileChange, Diff, WorkingDirectoryFileChange};
use tracing::warn;

use crate::dispatcher::{
    CommitDiffOptions, Dispatcher, LoadedDiff, WorkingDiffOptions, compute_commit_diff,
    compute_working_diff, replace_diff,
};
use crate::host::{AsyncCtx, Host};
use crate::state::{AppState, RepositoryState};

/// Whose diffs a stack holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StackKind {
    /// The Changes tab: working-directory files against `HEAD`.
    Working,
    /// The History tab: the files of these commits (oldest first, as
    /// [`Dispatcher::ordered_selection`] lists them).
    Commit(Vec<String>),
}

/// One file's loaded diff in a stack.
#[derive(Clone, Debug)]
pub struct StackedEntry {
    pub diff: Arc<Diff>,
    /// New-side lines (hunk expansion, highlighting).
    pub contents: Option<Arc<Vec<String>>>,
    /// Old-side lines (highlighting).
    pub old_contents: Option<Arc<Vec<String>>>,
    /// Bumped when `diff` or the contents change, so the view rebuilds
    /// its rows (like `diff_generation`).
    pub generation: u64,
}

/// The diffs loaded for the stack a repository shows.
#[derive(Clone, Debug, Default)]
pub struct StackedDiffs {
    /// What the entries belong to; another kind starts over.
    pub kind: Option<StackKind>,
    pub entries: HashMap<String, StackedEntry>,
    /// Paths whose diff is being computed, with the request that will
    /// fill them (an older request's result is dropped).
    pending: HashMap<String, u64>,
    requests: u64,
    generations: u64,
}

impl StackedDiffs {
    /// `path`'s diff, when the stack is of `kind`.
    pub fn entry(&self, kind: &StackKind, path: &str) -> Option<&StackedEntry> {
        if self.kind.as_ref() != Some(kind) {
            return None;
        }
        self.entries.get(path)
    }

    /// Whether `path`'s diff is loaded or loading for a stack of `kind`.
    pub fn requested(&self, kind: &StackKind, path: &str) -> bool {
        self.kind.as_ref() == Some(kind)
            && (self.entries.contains_key(path) || self.pending.contains_key(path))
    }

    /// Start over when the stack changes kind.
    fn retarget(&mut self, kind: &StackKind) {
        if self.kind.as_ref() != Some(kind) {
            self.entries.clear();
            self.pending.clear();
            self.kind = Some(kind.clone());
        }
    }

    /// Register a load of `path`; the request number its result must carry.
    fn request(&mut self, path: &str) -> u64 {
        self.requests += 1;
        self.pending.insert(path.to_string(), self.requests);
        self.requests
    }

    /// Store a loaded diff; `false` when the request is stale.
    fn apply(&mut self, kind: &StackKind, path: &str, request: u64, loaded: LoadedDiff) -> bool {
        if self.kind.as_ref() != Some(kind) || self.pending.get(path) != Some(&request) {
            return false;
        }
        self.pending.remove(path);
        let (diff, contents, old_contents) = loaded;
        match self.entries.get_mut(path) {
            Some(entry) => {
                let mut diff_slot = Some(entry.diff.clone());
                let mut contents_slot = entry.contents.take();
                let mut old_slot = entry.old_contents.take();
                let changed = replace_diff(
                    (&mut diff_slot, &mut contents_slot, &mut old_slot),
                    (diff, contents, old_contents),
                );
                if let Some(diff) = diff_slot {
                    entry.diff = diff;
                }
                entry.contents = contents_slot;
                entry.old_contents = old_slot;
                if changed {
                    self.generations += 1;
                    entry.generation = self.generations;
                }
                changed
            }
            None => {
                self.generations += 1;
                self.entries.insert(
                    path.to_string(),
                    StackedEntry {
                        diff,
                        contents,
                        old_contents,
                        generation: self.generations,
                    },
                );
                true
            }
        }
    }

    /// Forget everything (a setting the diffs depend on changed).
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// The flag is on.
pub fn enabled(s: &AppState) -> bool {
    s.flags.bool(crate::flags::ids::STACKED_DIFF)
}

impl Dispatcher {
    /// Load `path`'s diff for the stack of `kind` (the view asks as the
    /// file scrolls into reach). A diff already loaded or loading for the
    /// same stack is left alone; `force` reloads it (a refresh).
    pub fn load_stacked_diff(
        id: u64,
        kind: StackKind,
        path: String,
        force: bool,
        cx: &mut dyn Host,
    ) {
        let skip = Self::state(cx).update(cx, |s, _| {
            let rs = s.repo_state_mut(id);
            rs.stacked.retarget(&kind);
            !force && rs.stacked.requested(&kind, &path)
        });
        if skip {
            return;
        }
        match kind {
            StackKind::Working => Self::load_stacked_working(id, path, cx),
            StackKind::Commit(shas) => Self::load_stacked_commit(id, shas, path, cx),
        }
    }

    fn load_stacked_working(id: u64, path: String, cx: &mut dyn Host) {
        let state = Self::state(cx);
        let (git, workdir, file, options, head) = {
            let s = state.read(cx);
            let Some(git) = s.git.clone() else { return };
            let Some(rs) = s.repo_states.get(&id) else {
                return;
            };
            let Some(info) = rs.info.as_ref() else { return };
            let Some(status) = rs.status.as_deref() else {
                return;
            };
            let Some(file) = status.files.iter().find(|f| f.path == path).cloned() else {
                return;
            };
            (
                git,
                info.workdir.clone(),
                file,
                WorkingDiffOptions::of(s, rs, &path),
                status.current_tip.clone(),
            )
        };
        let kind = StackKind::Working;
        let request = state.update(cx, |s, _| s.repo_state_mut(id).stacked.request(&path));
        let stamp =
            crate::diff_cache::working_stamp(&workdir, &file, head.as_deref(), options.key());
        if let Some(loaded) = stamp
            .as_ref()
            .and_then(|stamp| crate::diff_cache::working_diff(&workdir, &path, stamp))
        {
            Self::apply_stacked_diff(id, &kind, &path, request, loaded, cx);
            return;
        }
        let work = cx.background_executor().spawn(async move {
            let loaded = compute_working_diff(git, &workdir, &file, options, None);
            if let Some(stamp) = stamp {
                crate::diff_cache::store_working_diff(&workdir, &file.path, stamp, loaded.clone());
            }
            loaded
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let loaded = work.await;
            cx.update(|cx| Self::apply_stacked_diff(id, &kind, &path, request, loaded, cx));
        })
        .detach();
    }

    fn load_stacked_commit(id: u64, shas: Vec<String>, path: String, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let state = Self::state(cx);
        let (file, hide_whitespace, remerge, svg_image, options) = {
            let s = state.read(cx);
            let Some(rs) = s.repo_states.get(&id) else {
                return;
            };
            // the commit selection moved on
            if Self::ordered_selection(rs) != shas {
                return;
            }
            let Some(file) = rs
                .changeset
                .as_ref()
                .and_then(|c| c.files.iter().find(|f| f.path == path).cloned())
            else {
                return;
            };
            (
                file,
                s.settings.hide_whitespace_in_history_diff,
                Self::remerge_applies(s, rs),
                Self::svg_shown_as_image(s, &path),
                CommitDiffOptions::of(s),
            )
        };
        let kind = StackKind::Commit(shas.clone());
        let request = state.update(cx, |s, _| s.repo_state_mut(id).stacked.request(&path));
        if !remerge
            && !svg_image
            && let Some(loaded) =
                crate::diff_cache::commit_diff(&workdir, &shas, &path, hide_whitespace)
        {
            Self::apply_stacked_diff(id, &kind, &path, request, loaded, cx);
            return;
        }
        let work = cx.background_executor().spawn(async move {
            if remerge {
                return remerge_diff(git, &workdir, &file, hide_whitespace);
            }
            if svg_image {
                return svg_image_diff(git, &workdir, &shas, &file);
            }
            let loaded = compute_commit_diff(git, &workdir, &shas, &file, options);
            crate::diff_cache::store_commit_diff(
                &workdir,
                &shas,
                &file.path,
                hide_whitespace,
                loaded.clone(),
            );
            loaded
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let loaded = work.await;
            cx.update(|cx| Self::apply_stacked_diff(id, &kind, &path, request, loaded, cx));
        })
        .detach();
    }

    fn apply_stacked_diff(
        id: u64,
        kind: &StackKind,
        path: &str,
        request: u64,
        loaded: LoadedDiff,
        cx: &mut dyn Host,
    ) {
        Self::state(cx).update(cx, |s, cx| {
            if s.repo_state_mut(id)
                .stacked
                .apply(kind, path, request, loaded)
            {
                cx.notify();
            }
        });
    }

    /// After a status refresh: reload the loaded working-directory entries
    /// (the caches answer unchanged files at once) and drop files no longer
    /// changed. A diff still computing is left to arrive; the refresh that
    /// follows any change to its file reloads it.
    pub(crate) fn refresh_stacked_working(id: u64, cx: &mut dyn Host) {
        let paths: Vec<String> = {
            let s = Self::state(cx).read(cx);
            if !enabled(s) {
                return;
            }
            let Some(rs) = s.repo_states.get(&id) else {
                return;
            };
            if rs.stacked.kind != Some(StackKind::Working) {
                return;
            }
            let mut paths: Vec<String> = rs.stacked.entries.keys().cloned().collect();
            paths.sort();
            paths
        };
        if paths.is_empty() {
            return;
        }
        let gone: Vec<String> = Self::state(cx).update(cx, |s, cx| {
            let rs = s.repo_state_mut(id);
            let status = rs.status.clone();
            let gone: Vec<String> = paths
                .iter()
                .filter(|p| {
                    !status
                        .as_deref()
                        .is_some_and(|st| st.files.iter().any(|f| &f.path == *p))
                })
                .cloned()
                .collect();
            for p in &gone {
                rs.stacked.entries.remove(p);
                rs.stacked.pending.remove(p);
            }
            if !gone.is_empty() {
                cx.notify();
            }
            gone
        });
        for path in paths.into_iter().filter(|p| !gone.contains(p)) {
            Self::load_stacked_working(id, path, cx);
        }
    }

    /// A setting the diffs depend on changed (hide whitespace, an SVG
    /// shown as image): the loaded entries are stale.
    pub(crate) fn clear_stacked_diffs(id: u64, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            let rs = s.repo_state_mut(id);
            if rs.stacked.kind.is_some() {
                rs.stacked.clear();
                cx.notify();
            }
        });
    }

    /// Reload one file's entry after something about that file changed
    /// (its SVG view, a binary shown as text).
    pub(crate) fn reload_stacked_diff(id: u64, path: &str, cx: &mut dyn Host) {
        let kind = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|rs| {
                rs.stacked
                    .entries
                    .contains_key(path)
                    .then(|| rs.stacked.kind.clone())
                    .flatten()
            });
        if let Some(kind) = kind {
            Self::load_stacked_diff(id, kind, path.to_string(), true, cx);
        }
    }

    /// Settings › Advanced / the file list's toggle: show every file's
    /// diff stacked in Changes (`history == false`) or History.
    pub fn set_stacked_diff(history: bool, on: bool, cx: &mut dyn Host) {
        Self::update_settings(cx, |s| {
            if history {
                s.stacked_diff_history = on;
            } else {
                s.stacked_diff_changes = on;
            }
        });
    }

    /// The stack History shows for `rs`'s commit selection.
    pub fn commit_stack_kind(rs: &RepositoryState) -> StackKind {
        StackKind::Commit(Self::ordered_selection(rs))
    }
}

/// `773-merge-remerge-diff`: one file's diff from the re-merge to the
/// recorded merge, with the new side for expansion and highlighting (the
/// re-merge with its conflict markers is no blob). Shared with
/// `Dispatcher::load_commit_diff`.
pub(crate) fn remerge_diff(
    git: Arc<corvene_git::GitBinary>,
    workdir: &std::path::Path,
    file: &CommittedFileChange,
    hide_whitespace: bool,
) -> LoadedDiff {
    let diff = corvene_git::remerge_file_diff(git.clone(), workdir, file, hide_whitespace)
        .unwrap_or_else(|err| {
            warn!(%err, "remerge diff failed");
            Diff::Empty
        });
    let contents = (file.status.kind != corvene_models::FileStatusKind::Deleted)
        .then(|| corvene_git::blob_lines(git, workdir, &file.commitish, &file.path))
        .flatten();
    (Arc::new(diff), contents.map(Arc::new), None)
}

/// `794-svg-image-diff`: an SVG file's before and after as images. Shared
/// with `Dispatcher::load_commit_diff`.
pub(crate) fn svg_image_diff(
    git: Arc<corvene_git::GitBinary>,
    workdir: &std::path::Path,
    ordered: &[String],
    file: &CommittedFileChange,
) -> LoadedDiff {
    let newest = match ordered {
        [_, .., newest] => newest.clone(),
        _ => file.commitish.clone(),
    };
    let oldest = ordered
        .first()
        .cloned()
        .unwrap_or_else(|| file.commitish.clone());
    let previous_path = file.old_path.as_deref().unwrap_or(&file.path);
    let diff = corvene_git::image_diff_as(
        corvene_git::SVG_MEDIA_TYPE,
        file.status.kind,
        || corvene_git::blob_bytes(git.clone(), workdir, &newest, &file.path).ok(),
        || corvene_git::blob_bytes(git.clone(), workdir, &format!("{oldest}^"), previous_path).ok(),
    );
    (Arc::new(diff), None, None)
}

/// The files a stack shows, in list order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackedFile {
    pub path: String,
    pub kind: corvene_models::FileStatusKind,
    pub old_path: Option<String>,
}

impl From<&WorkingDirectoryFileChange> for StackedFile {
    fn from(f: &WorkingDirectoryFileChange) -> Self {
        Self {
            path: f.path.clone(),
            kind: f.status.kind,
            old_path: f.old_path.clone(),
        }
    }
}

impl From<&CommittedFileChange> for StackedFile {
    fn from(f: &CommittedFileChange) -> Self {
        Self {
            path: f.path.clone(),
            kind: f.status.kind,
            old_path: f.old_path.clone(),
        }
    }
}

/// The working-directory files the Changes tab stacks: the ⌘/⇧-selected
/// files when there are several, else (with the setting on) the files
/// included in the next commit, in list order. `None`: the single-file
/// diff as usual.
pub fn working_stack(s: &AppState, rs: &RepositoryState) -> Option<Vec<StackedFile>> {
    if !enabled(s) {
        return None;
    }
    let status = rs.status.as_deref()?;
    if rs.selected_files.len() > 1 {
        let files: Vec<StackedFile> = status
            .files
            .iter()
            .filter(|f| rs.selected_files.contains(&f.path))
            .map(StackedFile::from)
            .collect();
        return (files.len() > 1).then_some(files);
    }
    if !s.settings.stacked_diff_changes {
        return None;
    }
    // `1313-changelists`: only the active list's files are committed
    let lists = s
        .repo_states
        .iter()
        .find(|(_, r)| std::ptr::eq(*r, rs))
        .and_then(|(id, _)| crate::changelists::of(s, *id));
    let files: Vec<StackedFile> = status
        .files
        .iter()
        .filter(|f| crate::changelists::committed(lists.as_deref(), f))
        .map(StackedFile::from)
        .collect();
    (!files.is_empty()).then_some(files)
}

/// The commit files History stacks: `multi` (the ⌘/⇧-selected paths of
/// flag `810`) when there are several, else (with the setting on) every
/// file of the commit. `None`: the single-file diff as usual.
pub fn commit_stack(
    s: &AppState,
    rs: &RepositoryState,
    multi: &[String],
) -> Option<Vec<StackedFile>> {
    if !enabled(s) {
        return None;
    }
    let changeset = rs.changeset.as_ref()?;
    if multi.len() > 1 {
        let files: Vec<StackedFile> = changeset
            .files
            .iter()
            .filter(|f| multi.contains(&f.path))
            .map(StackedFile::from)
            .collect();
        return (files.len() > 1).then_some(files);
    }
    if !s.settings.stacked_diff_history {
        return None;
    }
    let files: Vec<StackedFile> = changeset.files.iter().map(StackedFile::from).collect();
    (!files.is_empty()).then_some(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded(text: &str) -> LoadedDiff {
        (
            Arc::new(Diff::Text {
                hunks: Vec::new(),
                warnings: Default::default(),
            }),
            Some(Arc::new(vec![text.to_string()])),
            None,
        )
    }

    #[test]
    fn a_stale_request_is_dropped() {
        let mut stack = StackedDiffs::default();
        stack.retarget(&StackKind::Working);
        let first = stack.request("a.rs");
        let second = stack.request("a.rs");
        assert!(!stack.apply(&StackKind::Working, "a.rs", first, loaded("one")));
        assert!(stack.entries.is_empty());
        assert!(stack.apply(&StackKind::Working, "a.rs", second, loaded("two")));
        assert_eq!(stack.entries["a.rs"].generation, 1);
        assert!(!stack.requested(&StackKind::Commit(vec![]), "a.rs"));
        assert!(stack.requested(&StackKind::Working, "a.rs"));
    }

    #[test]
    fn an_unchanged_reload_keeps_the_generation() {
        let mut stack = StackedDiffs::default();
        stack.retarget(&StackKind::Working);
        let r = stack.request("a.rs");
        assert!(stack.apply(&StackKind::Working, "a.rs", r, loaded("one")));
        let r = stack.request("a.rs");
        assert!(!stack.apply(&StackKind::Working, "a.rs", r, loaded("one")));
        assert_eq!(stack.entries["a.rs"].generation, 1);
        let r = stack.request("a.rs");
        assert!(stack.apply(&StackKind::Working, "a.rs", r, loaded("two")));
        assert_eq!(stack.entries["a.rs"].generation, 2);
    }

    #[test]
    fn another_kind_starts_over() {
        let mut stack = StackedDiffs::default();
        stack.retarget(&StackKind::Working);
        let r = stack.request("a.rs");
        assert!(stack.apply(&StackKind::Working, "a.rs", r, loaded("one")));
        stack.retarget(&StackKind::Commit(vec!["abc".into()]));
        assert!(stack.entries.is_empty());
        assert!(stack.entry(&StackKind::Working, "a.rs").is_none());
    }
}
