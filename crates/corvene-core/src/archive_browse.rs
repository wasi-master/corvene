//! Corvene `1323-archive-diff`: what the archive view shows. A changed
//! archive's diff ([`corvene_models::Diff::Archive`], read by
//! [`corvene_git::archive`]) lists its entries; this module keeps, per diff
//! surface (Changes, History, a stash, …), which archive is open, the
//! archives inside it the user drilled into, the selected entry and that
//! entry's own diff. Leaving an archive remembers the place, so coming back
//! to it (or a refresh that reloads it) keeps the selection.
//!
//! GitHub Desktop shows archives as binary files (`app/src/lib/git/diff.ts`
//! `getBinaryPaths`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use corvene_git::archive::EntrySide;
use corvene_models::{ArchiveDiff, ArchiveEntryChange, Diff};

use crate::dispatcher::{Dispatcher, LoadedDiff};
use crate::host::Host;
use crate::state::AppState;

/// Which diff an archive view belongs to (the UI's `DiffSource`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ArchiveSurface {
    Changes,
    History,
    Stash,
    PullRequest,
    Review,
    RefCompare,
}

impl ArchiveSurface {
    /// The hide-whitespace setting the surface's diffs use.
    pub fn hide_whitespace(self, s: &AppState) -> bool {
        match self {
            ArchiveSurface::Changes => s.settings.hide_whitespace_in_changes_diff,
            ArchiveSurface::History | ArchiveSurface::Stash | ArchiveSurface::Review => {
                s.settings.hide_whitespace_in_history_diff
            }
            ArchiveSurface::PullRequest | ArchiveSurface::RefCompare => {
                s.settings.hide_whitespace_in_pull_request_diff
            }
        }
    }
}

/// One entry's loaded diff.
#[derive(Clone, Debug)]
pub struct EntryLoad {
    pub path: String,
    pub diff: Arc<Diff>,
    pub contents: Option<Arc<Vec<String>>>,
    pub old_contents: Option<Arc<Vec<String>>>,
    /// Why the entry could not be read (the diff is then `Binary`).
    pub error: Option<String>,
    pub generation: u64,
    /// The hide-whitespace setting it was read with.
    pub hide_whitespace: bool,
}

/// One archive level: the outermost file, or an archive inside it.
#[derive(Clone, Debug)]
pub struct ArchiveLevel {
    /// The entry path in the level above (the repository path at the top).
    pub entry: String,
    pub diff: Arc<ArchiveDiff>,
}

/// The archive open on one surface.
#[derive(Clone, Debug)]
pub struct ArchiveBrowse {
    pub outer_path: String,
    /// The file's diff this view was opened for.
    root_diff: Arc<Diff>,
    root: ArchiveLevel,
    /// Archives inside archives, outermost first.
    pub nested: Vec<ArchiveLevel>,
    /// The selected entry of the innermost level.
    pub selected: Option<String>,
    pub entry: Option<EntryLoad>,
    /// The selected entry's diff is being read.
    pub entry_loading: bool,
    /// An entry of the innermost level being opened as an archive.
    pub opening: Option<String>,
    /// Moves on every change the view draws from.
    pub generation: u64,
    /// `archive_stacked`: the loaded diffs of the stacked entries.
    pub stacked: HashMap<String, EntryLoad>,
    stacked_requests: HashMap<String, u64>,
    request: u64,
}

impl ArchiveBrowse {
    fn new(outer_path: String, root_diff: Arc<Diff>, diff: Arc<ArchiveDiff>) -> Self {
        Self {
            root: ArchiveLevel {
                entry: outer_path.clone(),
                diff,
            },
            outer_path,
            root_diff,
            nested: Vec::new(),
            selected: None,
            entry: None,
            entry_loading: false,
            opening: None,
            generation: 0,
            stacked: HashMap::new(),
            stacked_requests: HashMap::new(),
            request: 0,
        }
    }

    /// The level shown (the innermost one).
    pub fn level(&self) -> &ArchiveLevel {
        self.nested.last().unwrap_or(&self.root)
    }

    /// Every level, outermost first (for the breadcrumb).
    pub fn levels(&self) -> impl Iterator<Item = &ArchiveLevel> {
        std::iter::once(&self.root).chain(self.nested.iter())
    }

    /// The entry paths leading to the innermost level.
    pub fn chain(&self) -> Vec<String> {
        self.nested.iter().map(|l| l.entry.clone()).collect()
    }

    /// The selected entry's change.
    pub fn selected_change(&self) -> Option<&ArchiveEntryChange> {
        self.level().diff.entry(self.selected.as_deref()?)
    }

    /// `outer.zip!/libs/lib.jar!/com/A.class`: an entry's full name.
    pub fn display_path(&self, entry: &str) -> String {
        let mut out = self.outer_path.clone();
        for level in &self.nested {
            out.push_str("!/");
            out.push_str(&level.entry);
        }
        out.push_str("!/");
        out.push_str(entry);
        out
    }

    /// `archive_stacked`: whether `path`'s diff is loaded or loading.
    pub fn stacked_requested(&self, path: &str) -> bool {
        self.stacked.contains_key(path) || self.stacked_requests.contains_key(path)
    }

    fn reset_entries(&mut self) {
        self.entry = None;
        self.entry_loading = false;
        self.stacked.clear();
        self.stacked_requests.clear();
        self.generation += 1;
    }
}

/// Where the view was in an archive it left.
#[derive(Clone, Debug, Default)]
struct Place {
    chain: Vec<String>,
    selected: Option<String>,
}

/// Every surface's archive view.
#[derive(Clone, Debug, Default)]
pub struct ArchiveBrowses {
    pub open: HashMap<ArchiveSurface, ArchiveBrowse>,
    places: HashMap<(ArchiveSurface, String), Place>,
    next_request: u64,
}

impl ArchiveBrowses {
    fn next(&mut self) -> u64 {
        self.next_request += 1;
        self.next_request
    }

    /// The archive open on `surface` for `path`.
    pub fn browse(&self, surface: ArchiveSurface, path: &str) -> Option<&ArchiveBrowse> {
        self.open.get(&surface).filter(|b| b.outer_path == path)
    }

    /// The selected entry of `path` on `surface`, open or remembered.
    pub fn selected_entry(&self, surface: ArchiveSurface, path: &str) -> Option<&str> {
        match self.browse(surface, path) {
            Some(b) if b.nested.is_empty() => b.selected.as_deref(),
            Some(_) => None,
            None => self
                .places
                .get(&(surface, path.to_string()))
                .filter(|p| p.chain.is_empty())
                .and_then(|p| p.selected.as_deref()),
        }
    }

    fn remember(&mut self, surface: ArchiveSurface) {
        if let Some(b) = self.open.get(&surface) {
            self.places.insert(
                (surface, b.outer_path.clone()),
                Place {
                    chain: b.chain(),
                    selected: b.selected.clone(),
                },
            );
        }
    }
}

/// The first entry worth showing: a content change of a file.
fn first_change(diff: &ArchiveDiff) -> Option<String> {
    diff.entries
        .iter()
        .find(|e| e.status.is_content_change() && !e.is_dir())
        .or_else(|| diff.entries.iter().find(|e| !e.is_dir()))
        .map(|e| e.path.clone())
}

fn archive_of(diff: &Diff) -> Option<&ArchiveDiff> {
    match diff {
        Diff::Archive(a) => Some(a),
        _ => None,
    }
}

/// Whether the archive view is on.
pub fn enabled(s: &AppState) -> bool {
    s.flags.bool(crate::flags::ids::ARCHIVE_DIFF)
}

type EntryWork = (Arc<corvene_git::GitBinary>, PathBuf, Arc<ArchiveDiff>, bool);

impl Dispatcher {
    fn archives_mut(id: u64, cx: &mut dyn Host, f: impl FnOnce(&mut ArchiveBrowses) -> bool) {
        Self::state(cx).update(cx, |s, cx| {
            if f(&mut s.repo_state_mut(id).archives) {
                cx.notify();
            }
        });
    }

    /// The view shows `root` (an archive diff of `path`) on `surface`: open
    /// it, at the remembered place when it was open before. Called again
    /// on every reload; an unchanged diff is left alone.
    pub fn show_archive(
        id: u64,
        surface: ArchiveSurface,
        path: String,
        root: Arc<Diff>,
        cx: &mut dyn Host,
    ) {
        let Some(diff) = archive_of(&root).map(|a| Arc::new(a.clone())) else {
            return;
        };
        let mut reopen: Option<Vec<String>> = None;
        let mut load = None;
        Self::archives_mut(id, cx, |a| {
            if let Some(b) = a.open.get(&surface)
                && b.outer_path == path
                && (Arc::ptr_eq(&b.root_diff, &root) || *b.root_diff == *root)
            {
                return false;
            }
            // the place in the archive shown before (this one reloaded, or
            // another) is kept for when it comes back
            a.remember(surface);
            let place = a
                .places
                .get(&(surface, path.clone()))
                .cloned()
                .unwrap_or_default();
            let mut browse = ArchiveBrowse::new(path.clone(), root.clone(), diff.clone());
            browse.generation = a.open.get(&surface).map_or(0, |b| b.generation + 1);
            browse.selected = place
                .selected
                .filter(|p| place.chain.is_empty() && diff.entry(p).is_some())
                .or_else(|| first_change(&diff));
            if !place.chain.is_empty() {
                reopen = Some(place.chain.clone());
                browse.selected = None;
            } else if let Some(sel) = browse.selected.clone() {
                load = Some(sel);
            }
            a.open.insert(surface, browse);
            true
        });
        if let Some(chain) = reopen {
            Self::reopen_archive_chain(id, surface, chain, cx);
        } else if let Some(path) = load {
            Self::load_archive_entry(id, surface, path, cx);
        }
    }

    /// The surface no longer shows an archive (another file is selected).
    pub fn hide_archive(id: u64, surface: ArchiveSurface, cx: &mut dyn Host) {
        Self::archives_mut(id, cx, |a| {
            a.remember(surface);
            a.open.remove(&surface).is_some()
        });
    }

    fn entry_work(id: u64, surface: ArchiveSurface, cx: &dyn Host) -> Option<EntryWork> {
        let (git, workdir) = Self::repo_context(id, cx)?;
        let s = Self::state(cx).read(cx);
        let b = s.repo_states.get(&id)?.archives.open.get(&surface)?;
        Some((
            git,
            workdir,
            b.level().diff.clone(),
            surface.hide_whitespace(s),
        ))
    }

    /// Select an entry of the innermost level and read its diff.
    pub fn select_archive_entry(id: u64, surface: ArchiveSurface, path: String, cx: &mut dyn Host) {
        let mut changed = false;
        Self::archives_mut(id, cx, |a| {
            let Some(b) = a.open.get_mut(&surface) else {
                return false;
            };
            if b.selected.as_deref() == Some(path.as_str())
                && (b.entry.is_some() || b.entry_loading)
            {
                return false;
            }
            b.selected = Some(path.clone());
            b.generation += 1;
            changed = true;
            true
        });
        if changed {
            Self::load_archive_entry(id, surface, path, cx);
        }
    }

    fn load_archive_entry(id: u64, surface: ArchiveSurface, path: String, cx: &mut dyn Host) {
        let Some((git, workdir, level, hide_whitespace)) = Self::entry_work(id, surface, cx) else {
            return;
        };
        let key = crate::diff_cache::ArchiveEntryKey::new(&level, &path, hide_whitespace);
        if let Some(loaded) = crate::diff_cache::archive_entry(&key) {
            Self::apply_archive_entry(id, surface, &path, None, loaded, None, hide_whitespace, cx);
            return;
        }
        let mut request = 0;
        Self::archives_mut(id, cx, |a| {
            let next = a.next();
            let Some(b) = a.open.get_mut(&surface) else {
                return false;
            };
            request = next;
            b.request = next;
            b.entry_loading = true;
            true
        });
        crate::remote::spawn_bg(
            cx,
            move || {
                let result = corvene_git::archive::entry_diff(
                    &git,
                    &workdir,
                    &level,
                    &path,
                    hide_whitespace,
                );
                let (loaded, error) = loaded_of(result);
                if error.is_none() {
                    crate::diff_cache::store_archive_entry(key, loaded.clone());
                }
                (path, loaded, error)
            },
            move |(path, loaded, error), cx| {
                Self::apply_archive_entry(
                    id,
                    surface,
                    &path,
                    Some(request),
                    loaded,
                    error,
                    hide_whitespace,
                    cx,
                );
            },
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_archive_entry(
        id: u64,
        surface: ArchiveSurface,
        path: &str,
        request: Option<u64>,
        (diff, contents, old_contents): LoadedDiff,
        error: Option<String>,
        hide_whitespace: bool,
        cx: &mut dyn Host,
    ) {
        Self::archives_mut(id, cx, |a| {
            let Some(b) = a.open.get_mut(&surface) else {
                return false;
            };
            if request.is_some_and(|r| r != b.request) || b.selected.as_deref() != Some(path) {
                return false;
            }
            b.entry_loading = false;
            b.generation += 1;
            b.entry = Some(EntryLoad {
                path: path.to_string(),
                diff,
                contents,
                old_contents,
                error,
                generation: b.generation,
                hide_whitespace,
            });
            true
        });
    }

    /// The hide-whitespace setting changed: read the shown entries again.
    pub fn refresh_archive_entries(id: u64, surface: ArchiveSurface, cx: &mut dyn Host) {
        let mut load = None;
        Self::archives_mut(id, cx, |a| {
            let Some(b) = a.open.get_mut(&surface) else {
                return false;
            };
            load = b.selected.clone();
            b.entry = None;
            b.stacked.clear();
            b.stacked_requests.clear();
            b.generation += 1;
            true
        });
        if let Some(path) = load {
            Self::load_archive_entry(id, surface, path, cx);
        }
    }

    /// Open the selected entry, an archive itself, as the innermost level.
    pub fn open_archive_entry(id: u64, surface: ArchiveSurface, path: String, cx: &mut dyn Host) {
        // its diff, when already read, is the next level
        let loaded = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|rs| {
                let b = rs.archives.open.get(&surface)?;
                let entry = b.entry.as_ref().filter(|e| e.path == path)?;
                archive_of(&entry.diff).map(|d| Arc::new(d.clone()))
            });
        if let Some(diff) = loaded {
            Self::push_archive_level(id, surface, path, diff, cx);
            return;
        }
        let Some((git, workdir, level, hide_whitespace)) = Self::entry_work(id, surface, cx) else {
            return;
        };
        Self::archives_mut(id, cx, |a| {
            a.open.get_mut(&surface).is_some_and(|b| {
                b.opening = Some(path.clone());
                b.generation += 1;
                true
            })
        });
        crate::remote::spawn_bg(
            cx,
            move || {
                let result = corvene_git::archive::entry_diff(
                    &git,
                    &workdir,
                    &level,
                    &path,
                    hide_whitespace,
                );
                (path, result)
            },
            move |(path, result), cx| match result {
                Ok(corvene_git::archive::EntryDiff {
                    diff: Diff::Archive(diff),
                    ..
                }) => Self::push_archive_level(id, surface, path, Arc::new(*diff), cx),
                other => {
                    Self::archives_mut(id, cx, |a| {
                        a.open.get_mut(&surface).is_some_and(|b| {
                            b.opening = None;
                            b.generation += 1;
                            true
                        })
                    });
                    if let Err(err) = other {
                        Self::show_error("Could not open the archive", err, cx);
                    }
                }
            },
        );
    }

    fn push_archive_level(
        id: u64,
        surface: ArchiveSurface,
        entry: String,
        diff: Arc<ArchiveDiff>,
        cx: &mut dyn Host,
    ) {
        let mut load = None;
        Self::archives_mut(id, cx, |a| {
            let Some(b) = a.open.get_mut(&surface) else {
                return false;
            };
            if b.level().diff.entry(&entry).is_none() {
                return false;
            }
            b.opening = None;
            b.selected = first_change(&diff);
            load = b.selected.clone();
            b.nested.push(ArchiveLevel { entry, diff });
            b.reset_entries();
            true
        });
        if let Some(path) = load {
            Self::load_archive_entry(id, surface, path, cx);
        }
    }

    /// Go `levels` archives up, selecting the archive the view came out of.
    pub fn archive_up(id: u64, surface: ArchiveSurface, levels: usize, cx: &mut dyn Host) {
        let mut load = None;
        Self::archives_mut(id, cx, |a| {
            let Some(b) = a.open.get_mut(&surface) else {
                return false;
            };
            let mut came_from = None;
            for _ in 0..levels.min(b.nested.len()) {
                came_from = b.nested.pop().map(|l| l.entry);
            }
            let Some(came_from) = came_from else {
                return false;
            };
            b.selected = Some(came_from.clone());
            load = Some(came_from);
            b.reset_entries();
            true
        });
        if let Some(path) = load {
            Self::load_archive_entry(id, surface, path, cx);
        }
    }

    /// Reopen the archives inside the archive a reload replaced, as deep as
    /// they still exist.
    fn reopen_archive_chain(
        id: u64,
        surface: ArchiveSurface,
        chain: Vec<String>,
        cx: &mut dyn Host,
    ) {
        let Some((git, workdir, root, hide_whitespace)) = Self::entry_work(id, surface, cx) else {
            return;
        };
        let generation = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|rs| rs.archives.open.get(&surface))
            .map(|b| b.generation);
        crate::remote::spawn_bg(
            cx,
            move || {
                let mut levels = Vec::new();
                let mut level = root;
                for entry in chain {
                    match corvene_git::archive::entry_diff(
                        &git,
                        &workdir,
                        &level,
                        &entry,
                        hide_whitespace,
                    ) {
                        Ok(corvene_git::archive::EntryDiff {
                            diff: Diff::Archive(diff),
                            ..
                        }) => {
                            level = Arc::new(*diff);
                            levels.push(ArchiveLevel {
                                entry,
                                diff: level.clone(),
                            });
                        }
                        _ => break,
                    }
                }
                levels
            },
            move |levels, cx| {
                let mut load = None;
                Self::archives_mut(id, cx, |a| {
                    let Some(b) = a.open.get_mut(&surface) else {
                        return false;
                    };
                    if Some(b.generation) != generation {
                        return false;
                    }
                    let place = a
                        .places
                        .get(&(surface, b.outer_path.clone()))
                        .cloned()
                        .unwrap_or_default();
                    let complete = levels.len() == place.chain.len();
                    b.nested = levels;
                    let level = b.level().diff.clone();
                    b.selected = place
                        .selected
                        .filter(|p| complete && level.entry(p).is_some())
                        .or_else(|| first_change(&level));
                    load = b.selected.clone();
                    b.reset_entries();
                    true
                });
                if let Some(path) = load {
                    Self::load_archive_entry(id, surface, path, cx);
                }
            },
        );
    }

    /// `archive_stacked`: read one entry's diff for the stack (the view asks
    /// as its header scrolls into reach).
    pub fn load_archive_stacked(id: u64, surface: ArchiveSurface, path: String, cx: &mut dyn Host) {
        let Some((git, workdir, level, hide_whitespace)) = Self::entry_work(id, surface, cx) else {
            return;
        };
        let key = crate::diff_cache::ArchiveEntryKey::new(&level, &path, hide_whitespace);
        let mut request = None;
        Self::archives_mut(id, cx, |a| {
            let next = a.next();
            let Some(b) = a.open.get_mut(&surface) else {
                return false;
            };
            if b.stacked_requests.contains_key(&path) {
                return false;
            }
            b.stacked_requests.insert(path.clone(), next);
            request = Some(next);
            false
        });
        let Some(request) = request else {
            return;
        };
        let apply = move |path: String,
                          (diff, contents, old_contents): LoadedDiff,
                          error,
                          cx: &mut dyn Host| {
            Self::archives_mut(id, cx, |a| {
                let Some(b) = a.open.get_mut(&surface) else {
                    return false;
                };
                if b.stacked_requests.get(&path) != Some(&request) {
                    return false;
                }
                b.generation += 1;
                let generation = b.generation;
                b.stacked.insert(
                    path.clone(),
                    EntryLoad {
                        path,
                        diff,
                        contents,
                        old_contents,
                        error,
                        generation,
                        hide_whitespace,
                    },
                );
                true
            });
        };
        if let Some(loaded) = crate::diff_cache::archive_entry(&key) {
            apply(path, loaded, None, cx);
            return;
        }
        crate::remote::spawn_bg(
            cx,
            move || {
                let result = corvene_git::archive::entry_diff(
                    &git,
                    &workdir,
                    &level,
                    &path,
                    hide_whitespace,
                );
                let (loaded, error) = loaded_of(result);
                if error.is_none() {
                    crate::diff_cache::store_archive_entry(key, loaded.clone());
                }
                (path, loaded, error)
            },
            move |(path, loaded, error), cx| apply(path, loaded, error, cx),
        );
    }

    pub fn set_archive_show_unchanged(on: bool, cx: &mut dyn Host) {
        Self::update_settings(cx, |s| s.archive_show_unchanged = on);
    }

    pub fn set_archive_show_metadata_only(on: bool, cx: &mut dyn Host) {
        Self::update_settings(cx, |s| s.archive_show_metadata_only = on);
    }

    pub fn set_archive_stacked(on: bool, cx: &mut dyn Host) {
        Self::update_settings(cx, |s| s.archive_stacked = on);
    }

    pub fn set_archive_tree_width(width: f32, cx: &mut dyn Host) {
        Self::update_settings(cx, |s| s.archive_tree_width = width);
    }

    /// The level and entry an action works on, with git.
    fn action_context(
        id: u64,
        surface: ArchiveSurface,
        cx: &dyn Host,
    ) -> Option<(Arc<corvene_git::GitBinary>, PathBuf, Arc<ArchiveDiff>)> {
        let (git, workdir, level, _) = Self::entry_work(id, surface, cx)?;
        Some((git, workdir, level))
    }

    /// Write one version of an entry to `dest` (Save Entry As…).
    pub fn save_archive_entry(
        id: u64,
        surface: ArchiveSurface,
        path: String,
        side: EntrySide,
        dest: PathBuf,
        cx: &mut dyn Host,
    ) {
        let Some((git, workdir, level)) = Self::action_context(id, surface, cx) else {
            return;
        };
        crate::remote::spawn_bg(
            cx,
            move || {
                let bytes = corvene_git::archive::entry_bytes(&git, &workdir, &level, &path, side)?;
                std::fs::write(&dest, &bytes).map_err(|e| e.to_string())
            },
            |result, cx| {
                if let Err(err) = result {
                    Self::show_error("Could not save the entry", err, cx);
                }
            },
        );
    }

    /// Open one version of an entry in its default app (written to a new
    /// temporary directory first).
    pub fn open_archive_entry_externally(
        id: u64,
        surface: ArchiveSurface,
        path: String,
        side: EntrySide,
        cx: &mut dyn Host,
    ) {
        let Some((git, workdir, level)) = Self::action_context(id, surface, cx) else {
            return;
        };
        crate::remote::spawn_bg(
            cx,
            move || corvene_git::archive::extract_to_temp(&git, &workdir, &level, &path, side),
            |result, cx| match result {
                Ok(file) => cx.open_with_system(&file),
                Err(err) => Self::show_error("Could not open the entry", err, cx),
            },
        );
    }

    /// Both versions of an entry in the configured `diff.tool`.
    pub fn open_archive_entry_in_diff_tool(
        id: u64,
        surface: ArchiveSurface,
        path: String,
        cx: &mut dyn Host,
    ) {
        let Some((git, workdir, level)) = Self::action_context(id, surface, cx) else {
            return;
        };
        crate::remote::spawn_bg(
            cx,
            move || -> Result<(), String> {
                let entry = level
                    .entry(&path)
                    .ok_or_else(|| "The entry is not in the archive.".to_string())?;
                let extract = |exists: bool, side| -> Result<PathBuf, String> {
                    if !exists {
                        return Ok(PathBuf::from("/dev/null"));
                    }
                    corvene_git::archive::extract_to_temp(&git, &workdir, &level, &path, side)
                };
                let old = extract(entry.old.is_some(), EntrySide::Old)?;
                let new = extract(entry.new.is_some(), EntrySide::New)?;
                corvene_git::archive::open_difftool_files(git.clone(), &workdir, &old, &new)
                    .map_err(|e| e.to_string())
            },
            |result, cx| {
                if let Err(err) = result {
                    Self::show_error("Could not open the diff tool", err, cx);
                }
            },
        );
    }
}

/// An entry diff as a loaded diff: an entry that does not read is shown as
/// binary, with the reason.
fn loaded_of(
    result: Result<corvene_git::archive::EntryDiff, String>,
) -> (LoadedDiff, Option<String>) {
    match result {
        Ok(e) => (
            (
                Arc::new(e.diff),
                e.new_lines.map(Arc::new),
                e.old_lines.map(Arc::new),
            ),
            None,
        ),
        Err(err) => ((Arc::new(Diff::Binary), None, None), Some(err)),
    }
}
