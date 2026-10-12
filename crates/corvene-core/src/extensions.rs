//! Language extensions in the app (Corvene addition, flag
//! `111-language-extensions`; GitHub Desktop highlights a fixed set of
//! CodeMirror modes). `corvene_extensions` reads and converts the
//! extensions; this module owns what the UI sees ([`ExtensionsState`]),
//! the install / remove / enable flows, and hands the prepared grammars to
//! `corvene_highlight::user` (the syntect set, rebuilt and cached as a dump
//! when the enabled set changes) and to the tree-sitter registry.
//!
//! Testing hooks: `CORVENE_EXTENSIONS_DIR=<dir>` (where extensions live),
//! `CORVENE_INSTALL_EXTENSION=<path>` installs that file or folder at
//! launch.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use std::sync::Arc;

use crate::host::{AsyncCtx, Host};
use corvene_extensions::github::RepoRef;
use corvene_extensions::importer::ImportCandidate;
use corvene_extensions::index::{Index, IndexKeys};
use corvene_extensions::install::{
    self, GrammarKind, Installed, Metadata, Resolution, Source, SourceKind, Status,
};
use corvene_extensions::manifest::GrammarRef;
use corvene_extensions::registry::{self, Candidate, Registry};
use corvene_extensions::tsbuild::{self, BuildPlan, Stage, compiler::Compiler};
use corvene_highlight::treesitter::{self, UserGrammar, UserLanguage};
use corvene_highlight::user;
use tracing::{info, warn};

use crate::dispatcher::Dispatcher;
use crate::flags::ids;
use crate::persistence::{ExtensionSwitches, LanguageExtensionsPrefs, StoreExt};
use crate::remote::spawn_bg;

/// Where installed extensions live (`~/Library/Application Support/Corvene/extensions`).
pub fn extensions_dir() -> PathBuf {
    std::env::var_os("CORVENE_EXTENSIONS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| corvene_platform::paths::app_support_dir().join("extensions"))
}

/// Downloads, staging and built grammar libraries
/// (`~/Library/Caches/Corvene/extensions`).
pub fn extensions_cache_dir() -> PathBuf {
    corvene_platform::paths::cache_dir().join("extensions")
}

/// What the Language Extensions dialog opens on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtensionsFocus {
    /// the Find tab with this file suffix looked up
    Suffix(String),
    Find,
    Import,
    /// the Find tab listing file icon themes (`117-file-icons`)
    IconThemes,
}

/// An install in flight, by stage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtensionProgress {
    Downloading { received: u64, total: Option<u64> },
    Unpacking,
    Converting,
}

impl ExtensionProgress {
    pub fn describe(&self) -> String {
        match self {
            ExtensionProgress::Downloading { received, total } => match total {
                Some(total) if *total > 0 => format!(
                    "Downloading {} of {}",
                    human_bytes(*received),
                    human_bytes(*total)
                ),
                _ => format!("Downloading {}", human_bytes(*received)),
            },
            ExtensionProgress::Unpacking => "Unpacking…".to_string(),
            ExtensionProgress::Converting => "Converting grammars…".to_string(),
        }
    }
}

pub fn human_bytes(n: u64) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    } else if n >= 1024 {
        format!("{} KB", n / 1024)
    } else {
        format!("{n} B")
    }
}

/// Where an extension to install comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum InstallSource {
    /// a file (archive or grammar) or folder on this machine
    LocalPath(PathBuf),
    /// an archive or grammar file at an `https://` address
    Url(String),
    /// a GitHub repository (optionally a folder in it)
    GitHub(RepoRef),
    /// a registry's extension
    Registry(Candidate),
    /// an extension of an editor installed on this machine
    Import(ImportCandidate),
}

impl InstallSource {
    /// The progress / error key before the extension id is known.
    pub fn key(&self) -> String {
        match self {
            InstallSource::LocalPath(path) => format!("pending:{}", path.display()),
            InstallSource::Url(url) => format!("pending:{url}"),
            InstallSource::GitHub(repo) => format!("pending:{}", repo.url()),
            InstallSource::Registry(candidate) => {
                format!("pending:{}", registry::extension_id(candidate))
            }
            InstallSource::Import(candidate) => format!("pending:{}", candidate.path.display()),
        }
    }

    /// What the pending row calls it.
    pub fn label(&self) -> String {
        match self {
            InstallSource::LocalPath(path) => path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            InstallSource::Url(url) => url
                .rsplit('/')
                .next()
                .filter(|n| !n.is_empty())
                .unwrap_or(url)
                .to_string(),
            InstallSource::GitHub(repo) => format!("{}/{}", repo.owner, repo.repo),
            InstallSource::Registry(candidate) => candidate.display_name.clone(),
            InstallSource::Import(candidate) => candidate.display_name.clone(),
        }
    }

    /// Whether this source is downloaded.
    pub fn is_remote(&self) -> bool {
        matches!(
            self,
            InstallSource::Url(_) | InstallSource::GitHub(_) | InstallSource::Registry(_)
        )
    }
}

/// The Find tab.
#[derive(Clone, Debug, Default)]
pub struct SearchState {
    pub query: String,
    /// results of the latest search, sorted by registry then downloads
    pub results: Vec<Candidate>,
    pub in_flight: BTreeSet<Registry>,
    pub errors: HashMap<Registry, String>,
    /// bumped per search so late answers to an earlier one are dropped
    pub generation: u64,
    /// the search was a suffix lookup (`.foo`) rather than free text
    pub suffix: Option<String>,
    /// `117-file-icons`: the Find tab looks for file icon themes
    pub icon_themes: bool,
}

/// What a suffix lookup (the diff hint) found.
#[derive(Clone, Debug, Default)]
pub struct HintResult {
    pub candidates: Vec<Candidate>,
    pub in_flight: bool,
    pub error: Option<String>,
    /// answered from the offline index
    pub from_index: bool,
}

/// A grammar build waiting for the user's consent (the sheet in the
/// Language Extensions dialog).
#[derive(Clone, Debug)]
pub struct BuildConsent {
    pub extension: String,
    pub grammar: String,
    pub plan: BuildPlan,
    /// the compiler that would run, or why none can
    pub compiler: Result<Compiler, String>,
}

/// A grammar build in flight (`builds`, by `<extension id>/<grammar>`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildProgress {
    pub stage: Stage,
}

/// The offline index's state. Only its names and suffixes stay in memory;
/// a lookup that will find candidates reads the file again (Corvene: the
/// whole index held about 9 MB).
#[derive(Clone, Debug, Default)]
pub struct IndexState {
    pub keys: Option<Arc<IndexKeys>>,
    pub refreshing: bool,
    pub error: Option<String>,
}

/// Days after which the offline index is fetched again.
const INDEX_MAX_AGE_DAYS: u64 = 7;

/// `AppState::extensions`.
#[derive(Clone, Debug, Default)]
pub struct ExtensionsState {
    /// Installed extensions, sorted by display name.
    pub installed: Vec<Installed>,
    /// The folder has been read once.
    pub loaded: bool,
    pub loading: bool,
    pub prefs: LanguageExtensionsPrefs,
    /// Installs in flight: by extension id, or `pending:<source>` before
    /// the id is known.
    pub progress: HashMap<String, ExtensionProgress>,
    /// The last error per extension id (or pending key).
    pub errors: HashMap<String, String>,
    /// The user grammar set is being rebuilt.
    pub rebuilding: bool,
    /// The row shown in the details panel.
    pub selected: Option<String>,
    /// Extension ids whose tree-sitter grammars are registered.
    pub registered_tree_sitter: BTreeSet<String>,
    pub search: SearchState,
    /// Import from editors: `None` until scanned.
    pub import_candidates: Option<Vec<ImportCandidate>>,
    pub import_scanning: bool,
    /// Suffix → the registries' candidates (the diff hint, the Find tab).
    pub hint_lookup: HashMap<String, HintResult>,
    /// Extension id → the newer version a registry has.
    pub updates: HashMap<String, Candidate>,
    pub checking_updates: bool,
    pub pending_consent: Option<BuildConsent>,
    /// `<extension id>/<grammar>` → the build's stage.
    pub builds: HashMap<String, BuildProgress>,
    /// `<extension id>/<grammar>` → the last build error.
    pub build_errors: HashMap<String, String>,
    pub index: IndexState,
}

impl ExtensionsState {
    pub fn get(&self, id: &str) -> Option<&Installed> {
        self.installed.iter().find(|i| i.metadata.id == id)
    }

    /// Extensions enabled, in order.
    pub fn enabled(&self) -> impl Iterator<Item = &Installed> {
        self.installed.iter().filter(|i| i.metadata.enabled)
    }

    /// The suffix the "no syntax highlighting" hint would name for `path`:
    /// its extension, unless the hint was dismissed for it or the file has
    /// none.
    pub fn hint_suffix(&self, path: &str) -> Option<String> {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let (stem, suffix) = name.rsplit_once('.')?;
        if stem.is_empty()
            || suffix.is_empty()
            || suffix.len() > 16
            || suffix
                .chars()
                .any(|c| !c.is_ascii_alphanumeric() && c != '_' && c != '-')
        {
            return None;
        }
        let suffix = suffix.to_ascii_lowercase();
        (!self.prefs.dismissed_suffixes.contains(&suffix)).then_some(suffix)
    }

    /// Whether `language` of extension `id` wins over the built-in
    /// highlighting: its own switch, else the extension's default.
    pub fn language_preferred(&self, id: &str, language: &str) -> bool {
        if let Some(switches) = self.prefs.switches.get(id)
            && let Some(answer) = switches.languages.get(language)
        {
            return *answer;
        }
        self.get(id).is_some_and(|i| i.metadata.prefer_over_builtin)
    }

    /// Apply the stored switches to freshly read metadata (the store wins).
    fn apply_switches(&mut self) {
        for installed in &mut self.installed {
            if let Some(switches) = self.prefs.switches.get(&installed.metadata.id) {
                installed.metadata.enabled = switches.enabled;
                installed.metadata.prefer_over_builtin = switches.prefer_over_builtin;
            }
        }
    }

    fn upsert(&mut self, installed: Installed) {
        self.installed
            .retain(|i| i.metadata.id != installed.metadata.id);
        self.installed.push(installed);
        self.installed.sort_by(|a, b| {
            a.metadata
                .display_name
                .to_lowercase()
                .cmp(&b.metadata.display_name.to_lowercase())
        });
    }
}

/// Syntax name → (extension id, prefers its grammar over the built-ins).
type Owners = HashMap<String, (String, bool)>;

/// What a rebuild of the user layer produced.
struct Rebuilt {
    set: Option<(corvene_extensions::cache::SyntaxSet, Owners)>,
    tree_sitter: Vec<(String, bool, Vec<UserGrammar>)>,
    errors: Vec<(String, String)>,
}

impl Dispatcher {
    fn extensions_enabled(cx: &dyn Host) -> bool {
        Self::state(cx)
            .read(cx)
            .flags
            .bool(ids::LANGUAGE_EXTENSIONS)
    }

    fn save_extension_prefs(cx: &mut dyn Host) {
        let (store, prefs) = {
            let s = Self::state(cx).read(cx);
            (s.store.clone(), s.extensions.prefs.clone())
        };
        if let Err(err) = store.save_language_extensions(&prefs) {
            warn!(%err, "could not save the language extension settings");
        }
    }

    /// At launch (and when `111-language-extensions` turns on): read the
    /// installed extensions and load their grammars. Honours
    /// `CORVENE_INSTALL_EXTENSION` afterwards.
    pub fn load_language_extensions(cx: &mut dyn Host) {
        if !Self::extensions_enabled(cx) {
            return;
        }
        let state = Self::state(cx);
        let prefs = state
            .read(cx)
            .store
            .language_extensions()
            .unwrap_or_default();
        state.update(cx, |s, cx| {
            s.extensions.prefs = prefs;
            s.extensions.loading = true;
            cx.notify();
        });
        let dir = extensions_dir();
        spawn_bg(
            cx,
            move || install::list(&dir),
            move |installed, cx| {
                Self::state(cx).update(cx, |s, cx| {
                    s.extensions.installed = installed;
                    s.extensions.apply_switches();
                    s.extensions.loaded = true;
                    s.extensions.loading = false;
                    cx.notify();
                });
                let count = Self::state(cx).read(cx).extensions.installed.len();
                if count > 0 {
                    info!(count, "language extensions found");
                }
                Self::rebuild_user_syntaxes(cx);
                Self::load_extension_index(cx);
                if let Ok(path) = std::env::var("CORVENE_INSTALL_EXTENSION")
                    && !path.is_empty()
                {
                    Self::install_extension(InstallSource::LocalPath(PathBuf::from(path)), cx);
                }
            },
        );
    }

    /// `111-language-extensions` turned off: drop the user grammars from
    /// the highlighter (the folders stay).
    pub(crate) fn unload_language_extensions(cx: &mut dyn Host) {
        let registered = Self::state(cx).update(cx, |s, cx| {
            s.extensions.loaded = false;
            cx.notify();
            std::mem::take(&mut s.extensions.registered_tree_sitter)
        });
        for id in registered {
            treesitter::unregister_user(&id);
        }
        user::clear();
    }

    /// Rebuild the user grammar set from the enabled extensions and hand it
    /// to the highlighter; register their tree-sitter grammars.
    pub fn rebuild_user_syntaxes(cx: &mut dyn Host) {
        let state = Self::state(cx);
        let enabled: Vec<Installed> = state.read(cx).extensions.enabled().cloned().collect();
        let switches = state.read(cx).extensions.prefs.switches.clone();
        let previously = state.update(cx, |s, cx| {
            s.extensions.rebuilding = true;
            cx.notify();
            s.extensions.registered_tree_sitter.clone()
        });
        let dir = extensions_dir();
        spawn_bg(
            cx,
            move || rebuild(&enabled, &dir, &switches),
            move |rebuilt, cx| {
                for id in &previously {
                    treesitter::unregister_user(id);
                }
                let mut registered = BTreeSet::new();
                for (id, preferred, grammars) in rebuilt.tree_sitter {
                    if !grammars.is_empty() {
                        treesitter::register_user(&id, preferred, grammars);
                        registered.insert(id);
                    }
                }
                match rebuilt.set {
                    Some((set, owners)) => user::install_set(set, owners),
                    None => user::clear(),
                }
                Self::state(cx).update(cx, |s, cx| {
                    s.extensions.rebuilding = false;
                    s.extensions.registered_tree_sitter = registered;
                    for (id, error) in rebuilt.errors {
                        s.extensions.errors.insert(id, error);
                    }
                    cx.notify();
                });
                // `117-file-icons`: the themes came or went with them
                Self::load_file_icon_theme(true, cx);
            },
        );
    }

    /// Install an extension from `source`: download or copy, unpack, read,
    /// convert, then rebuild the user grammars.
    pub fn install_extension(source: InstallSource, cx: &mut dyn Host) {
        let key = source.key();
        let state = Self::state(cx);
        if state.read(cx).extensions.progress.contains_key(&key) {
            return;
        }
        let first = if source.is_remote() {
            ExtensionProgress::Downloading {
                received: 0,
                total: None,
            }
        } else {
            ExtensionProgress::Unpacking
        };
        state.update(cx, |s, cx| {
            s.extensions.errors.remove(&key);
            s.extensions.progress.insert(key.clone(), first);
            cx.notify();
        });
        let (tx, rx) = async_channel::unbounded::<ExtensionProgress>();
        cx.spawn({
            let key = key.clone();
            async move |cx: &mut AsyncCtx| {
                while let Ok(progress) = rx.recv().await {
                    state.update(cx, |s, cx| {
                        if let Some(slot) = s.extensions.progress.get_mut(&key) {
                            *slot = progress;
                            cx.notify();
                        }
                    });
                }
            }
        })
        .detach();
        let dir = extensions_dir();
        let staging = extensions_cache_dir()
            .join("downloads")
            .join(format!("staging-{}", install::now()));
        let for_bg = source.clone();
        spawn_bg(
            cx,
            move || {
                let mut report = |progress: ExtensionProgress| {
                    let _ = tx.try_send(progress);
                };
                let result = install_from(&for_bg, &dir, &staging, &mut report);
                let _ = std::fs::remove_dir_all(&staging);
                result
            },
            move |result, cx| {
                Self::state(cx).update(cx, |s, cx| {
                    s.extensions.progress.remove(&key);
                    match &result {
                        Ok(installed) => {
                            let id = installed.metadata.id.clone();
                            s.extensions.errors.remove(&id);
                            s.extensions.updates.remove(&id);
                            s.extensions.upsert(installed.clone());
                            // a reinstall keeps the user's switches
                            s.extensions.apply_switches();
                            s.extensions.selected = Some(id);
                        }
                        Err(err) => {
                            warn!(%err, "could not install the language extension");
                            s.extensions.errors.insert(key.clone(), err.to_string());
                        }
                    }
                    cx.notify();
                });
                if result.is_ok() {
                    Self::rebuild_user_syntaxes(cx);
                }
            },
        );
    }

    /// Search the registries (the Find tab). Each registry answers on its
    /// own; a later search drops the earlier one's late answers.
    pub fn search_extensions(query: String, cx: &mut dyn Host) {
        let query = query.trim().to_string();
        // `117-file-icons`: icon themes list the popular ones without a query
        let icon_themes = Self::state(cx).read(cx).extensions.search.icon_themes;
        let suffix = query
            .strip_prefix('.')
            .filter(|s| !icon_themes && !s.is_empty() && !s.contains(char::is_whitespace))
            .map(str::to_ascii_lowercase);
        let registries: Vec<Registry> = if icon_themes {
            vec![Registry::OpenVsx, Registry::Zed]
        } else {
            Registry::ALL.to_vec()
        };
        let generation = Self::state(cx).update(cx, |s, cx| {
            s.extensions.search.generation += 1;
            s.extensions.search.query = query.clone();
            s.extensions.search.suffix = suffix.clone();
            s.extensions.search.results.clear();
            s.extensions.search.errors.clear();
            s.extensions.search.in_flight = if query.is_empty() && !icon_themes {
                BTreeSet::new()
            } else {
                registries.iter().copied().collect()
            };
            cx.notify();
            s.extensions.search.generation
        });
        if query.is_empty() && !icon_themes {
            return;
        }
        if let Some(suffix) = suffix
            .clone()
            .filter(|suffix| Self::index_knows(suffix, cx))
        {
            spawn_bg(
                cx,
                move || Self::read_index_candidates(&suffix),
                move |from_index, cx| {
                    Self::state(cx).update(cx, |s, cx| {
                        let search = &mut s.extensions.search;
                        if search.generation != generation {
                            return;
                        }
                        // the registries' answers, when they came first, are fresher
                        for candidate in from_index {
                            if !search
                                .results
                                .iter()
                                .any(|c| c.registry == candidate.registry && c.id == candidate.id)
                            {
                                search.results.push(candidate);
                            }
                        }
                        Self::sort_search_results(&mut search.results);
                        cx.notify();
                    });
                },
            );
        }
        for registry in registries {
            let (query, suffix) = (query.clone(), suffix.clone());
            spawn_bg(
                cx,
                move || match &suffix {
                    _ if icon_themes => registry.search_icon_themes(&query),
                    Some(suffix) => registry.for_suffix(suffix),
                    None => registry.search(&query),
                },
                move |result, cx| {
                    Self::state(cx).update(cx, |s, cx| {
                        let search = &mut s.extensions.search;
                        if search.generation != generation {
                            return;
                        }
                        search.in_flight.remove(&registry);
                        match result {
                            Ok(found) => {
                                for candidate in found {
                                    if let Some(slot) = search.results.iter_mut().find(|c| {
                                        c.registry == candidate.registry && c.id == candidate.id
                                    }) {
                                        *slot = candidate;
                                    } else {
                                        search.results.push(candidate);
                                    }
                                }
                                Self::sort_search_results(&mut search.results);
                            }
                            Err(err) => {
                                search.errors.insert(registry, err.to_string());
                            }
                        }
                        cx.notify();
                    });
                },
            );
        }
    }

    /// `117-file-icons`: switch the Find tab between language extensions
    /// and file icon themes; `search` searches again, else the old results
    /// go.
    pub fn set_extension_search_icon_themes(icon_themes: bool, search: bool, cx: &mut dyn Host) {
        let query = Self::state(cx).update(cx, |s, cx| {
            let search = &mut s.extensions.search;
            search.icon_themes = icon_themes;
            search.generation += 1;
            search.results.clear();
            search.in_flight.clear();
            search.errors.clear();
            cx.notify();
            search.query.clone()
        });
        if search {
            Self::search_extensions(query, cx);
        }
    }

    /// Whether the offline index has candidates for `suffix`.
    fn index_knows(suffix: &str, cx: &mut dyn Host) -> bool {
        Self::state(cx)
            .read(cx)
            .extensions
            .index
            .keys
            .as_ref()
            .is_some_and(|keys| keys.knows(suffix))
    }

    /// The offline index's candidates for `suffix`, read from disk (none
    /// when it does not read). Blocking.
    fn read_index_candidates(suffix: &str) -> Vec<Candidate> {
        match Index::load(&Self::index_path()) {
            Ok(index) => index.lookup(suffix),
            Err(err) => {
                warn!(%err, "the extension index on disk could not be read");
                Vec::new()
            }
        }
    }

    /// The Find tab's order: by registry, then the most downloaded.
    fn sort_search_results(results: &mut [Candidate]) {
        results.sort_by(|a, b| {
            a.registry
                .cmp(&b.registry)
                .then(b.downloads.cmp(&a.downloads))
                .then(a.display_name.cmp(&b.display_name))
        });
    }

    /// Where the offline index lives (`<extensions dir>/index.json.gz`).
    fn index_path() -> PathBuf {
        extensions_dir().join("index.json.gz")
    }

    /// Read the offline index from disk (in the background) and fetch a
    /// fresh one when it is missing or older than a week.
    pub fn load_extension_index(cx: &mut dyn Host) {
        let path = Self::index_path();
        let refreshed_at = Self::state(cx).read(cx).extensions.prefs.index_refreshed_at;
        spawn_bg(
            cx,
            move || {
                path.is_file()
                    .then(|| Index::load(&path).map(|index| index.keys()))
                    .transpose()
            },
            move |result, cx| {
                let loaded = match result {
                    Ok(Some(keys)) => Some(Arc::new(keys)),
                    Ok(None) => None,
                    Err(err) => {
                        warn!(%err, "the extension index on disk could not be read");
                        None
                    }
                };
                let age = loaded.as_ref().and_then(|i| i.age_days(install::now()));
                Self::state(cx).update(cx, |s, cx| {
                    s.extensions.index.keys = loaded.clone();
                    cx.notify();
                });
                let stale = loaded.is_none()
                    || age.is_none_or(|d| d >= INDEX_MAX_AGE_DAYS)
                        && refreshed_at.is_none_or(|t| {
                            install::now().saturating_sub(t) >= INDEX_MAX_AGE_DAYS * 86_400
                        });
                if stale {
                    Self::refresh_extension_index(cx);
                }
            },
        );
    }

    /// Fetch the index the packs manifest lists (sha256-checked) and use it.
    pub fn refresh_extension_index(cx: &mut dyn Host) {
        let state = Self::state(cx);
        if state.read(cx).extensions.index.refreshing {
            return;
        }
        state.update(cx, |s, cx| {
            s.extensions.index.refreshing = true;
            s.extensions.index.error = None;
            cx.notify();
        });
        let path = Self::index_path();
        spawn_bg(
            cx,
            move || -> Result<IndexKeys, String> {
                let manifest = corvene_packs::fetch_manifest().map_err(|e| e.to_string())?;
                let entry = manifest
                    .entry_for(
                        corvene_packs::PackKind::LanguageExtensionsIndex,
                        env!("CARGO_PKG_VERSION"),
                    )
                    .ok_or_else(|| "the packs manifest lists no extension index".to_string())?;
                let mut progress = |_, _| {};
                let downloaded = corvene_extensions::http::download(
                    &entry.url,
                    &path,
                    corvene_extensions::http::MAX_JSON_BYTES,
                    None,
                    &mut progress,
                )
                .map_err(|e| e.to_string())?;
                if !downloaded.sha256.eq_ignore_ascii_case(entry.sha256.trim()) {
                    let _ = std::fs::remove_file(&path);
                    return Err("the downloaded index is corrupt (sha256 mismatch)".to_string());
                }
                Index::load(&path)
                    .map(|index| index.keys())
                    .map_err(|e| e.to_string())
            },
            |result, cx| {
                Self::state(cx).update(cx, |s, cx| {
                    s.extensions.index.refreshing = false;
                    match result {
                        Ok(keys) => {
                            s.extensions.index.keys = Some(Arc::new(keys));
                            s.extensions.prefs.index_refreshed_at = Some(install::now());
                        }
                        Err(err) => {
                            info!(%err, "the extension index was not refreshed");
                            s.extensions.index.error = Some(err);
                        }
                    }
                    cx.notify();
                });
                Self::save_extension_prefs(cx);
            },
        );
    }

    /// The registries' extensions for files with `suffix` (the diff hint):
    /// the offline index answers at once when it knows the suffix, else the
    /// registries are asked.
    pub fn lookup_extensions_for_suffix(suffix: &str, cx: &mut dyn Host) {
        let suffix = suffix.trim_start_matches('.').to_ascii_lowercase();
        let state = Self::state(cx);
        if state
            .read(cx)
            .extensions
            .hint_lookup
            .get(&suffix)
            .is_some_and(|h| h.in_flight || !h.candidates.is_empty())
        {
            return;
        }
        let from_index = Self::index_knows(&suffix, cx);
        state.update(cx, |s, cx| {
            s.extensions.hint_lookup.insert(
                suffix.clone(),
                HintResult {
                    in_flight: true,
                    ..Default::default()
                },
            );
            cx.notify();
        });
        let wanted = suffix.clone();
        spawn_bg(
            cx,
            move || {
                if from_index {
                    let candidates = Self::read_index_candidates(&wanted);
                    if !candidates.is_empty() {
                        return (candidates, Vec::new(), true);
                    }
                }
                let mut candidates = Vec::new();
                let mut errors = Vec::new();
                for registry in Registry::ALL {
                    match registry.for_suffix(&wanted) {
                        Ok(found) => candidates.extend(found),
                        Err(err) => errors.push(format!("{}: {err}", registry.title())),
                    }
                }
                candidates.sort_by_key(|c| std::cmp::Reverse(c.downloads));
                (candidates, errors, false)
            },
            move |(candidates, errors, from_index), cx| {
                Self::state(cx).update(cx, |s, cx| {
                    s.extensions.hint_lookup.insert(
                        suffix,
                        HintResult {
                            candidates,
                            in_flight: false,
                            error: (!errors.is_empty()).then(|| errors.join("; ")),
                            from_index,
                        },
                    );
                    cx.notify();
                });
            },
        );
    }

    /// Scan the editors installed on this machine for grammar extensions
    /// (the Import tab).
    pub fn scan_installed_editors(cx: &mut dyn Host) {
        let state = Self::state(cx);
        if state.read(cx).extensions.import_scanning {
            return;
        }
        state.update(cx, |s, cx| {
            s.extensions.import_scanning = true;
            cx.notify();
        });
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        spawn_bg(
            cx,
            move || corvene_extensions::importer::scan(&home),
            |found, cx| {
                Self::state(cx).update(cx, |s, cx| {
                    s.extensions.import_scanning = false;
                    s.extensions.import_candidates = Some(found);
                    cx.notify();
                });
            },
        );
    }

    /// Ask each registry-installed extension's registry for a newer version.
    pub fn check_extension_updates(cx: &mut dyn Host) {
        let state = Self::state(cx);
        if state.read(cx).extensions.checking_updates {
            return;
        }
        let targets: Vec<(String, Registry, String, Option<String>)> = state
            .read(cx)
            .extensions
            .installed
            .iter()
            .filter_map(|i| {
                let md = &i.metadata;
                let registry = match md.source.kind {
                    SourceKind::OpenVsx => Registry::OpenVsx,
                    SourceKind::Zed => Registry::Zed,
                    SourceKind::Pulsar => Registry::Pulsar,
                    _ => return None,
                };
                let registry_id = md.source.registry_id.clone()?;
                Some((md.id.clone(), registry, registry_id, md.version.clone()))
            })
            .collect();
        if targets.is_empty() {
            return;
        }
        state.update(cx, |s, cx| {
            s.extensions.checking_updates = true;
            cx.notify();
        });
        spawn_bg(
            cx,
            move || {
                let mut newer = Vec::new();
                for (id, registry, registry_id, version) in targets {
                    let latest = match registry {
                        Registry::OpenVsx => registry::openvsx::latest(&registry_id),
                        Registry::Zed => registry::zed::latest(&registry_id),
                        Registry::Pulsar => registry::pulsar::latest(&registry_id),
                    };
                    match latest {
                        Ok(Some(candidate))
                            if candidate.version.is_some() && candidate.version != version =>
                        {
                            newer.push((id, candidate));
                        }
                        Ok(_) => {}
                        Err(err) => warn!(%err, id, "could not check the extension for updates"),
                    }
                }
                newer
            },
            |newer, cx| {
                Self::state(cx).update(cx, |s, cx| {
                    s.extensions.checking_updates = false;
                    for (id, candidate) in newer {
                        s.extensions.updates.insert(id, candidate);
                    }
                    cx.notify();
                });
            },
        );
    }

    /// `1001-build-grammars-from-source`: offer to build `grammar` of
    /// extension `id`. Shows the consent sheet unless the user remembered
    /// their answer for this repository and commit.
    pub fn request_grammar_build(id: &str, grammar: &str, cx: &mut dyn Host) {
        let state = Self::state(cx);
        let (plan, remembered) = {
            let s = state.read(cx);
            if !s.flags.bool(ids::BUILD_GRAMMARS_FROM_SOURCE) {
                return;
            }
            let Some(installed) = s.extensions.get(id) else {
                return;
            };
            let Some(status) = installed
                .metadata
                .grammars
                .iter()
                .find(|g| g.name == grammar)
            else {
                return;
            };
            let Some(repository) = &status.repository else {
                return;
            };
            let plan = BuildPlan::new(
                id,
                grammar,
                repository,
                status.rev.as_deref().unwrap_or("HEAD"),
                status.path.as_deref(),
                &extensions_cache_dir(),
            );
            let plan = match plan {
                Ok(plan) => plan,
                Err(err) => {
                    Self::state(cx).update(cx, |s, cx| {
                        s.extensions
                            .build_errors
                            .insert(format!("{id}/{grammar}"), err.to_string());
                        cx.notify();
                    });
                    return;
                }
            };
            let remembered = s
                .extensions
                .prefs
                .build_consents
                .contains(&plan.consent_key());
            (plan, remembered)
        };
        let (id, grammar) = (id.to_string(), grammar.to_string());
        spawn_bg(cx, Compiler::find, move |compiler, cx| {
            match (remembered, compiler) {
                (true, Ok(compiler)) => Self::build_grammar(plan, compiler, cx),
                (_, compiler) => {
                    Self::state(cx).update(cx, |s, cx| {
                        s.extensions.pending_consent = Some(BuildConsent {
                            extension: id,
                            grammar,
                            plan,
                            compiler,
                        });
                        cx.notify();
                    });
                }
            }
        });
    }

    /// `CORVENE_POPUP=language-extensions:consent`: offer the first grammar
    /// waiting for a build (dev / snapshot convenience).
    pub fn request_first_grammar_build(cx: &mut dyn Host) {
        let found = Self::state(cx)
            .read(cx)
            .extensions
            .installed
            .iter()
            .find_map(|i| {
                i.metadata
                    .grammars
                    .iter()
                    .find(|g| g.resolution == Resolution::NeedsBuild)
                    .map(|g| (i.metadata.id.clone(), g.name.clone()))
            });
        if let Some((id, grammar)) = found {
            Self::request_grammar_build(&id, &grammar, cx);
        }
    }

    /// The consent sheet's answer.
    pub fn respond_to_build_consent(accept: bool, remember: bool, cx: &mut dyn Host) {
        let consent = Self::state(cx).update(cx, |s, cx| {
            cx.notify();
            s.extensions.pending_consent.take()
        });
        let Some(consent) = consent else {
            return;
        };
        if !accept {
            return;
        }
        let Ok(compiler) = consent.compiler else {
            return;
        };
        if remember {
            Self::state(cx).update(cx, |s, _| {
                s.extensions
                    .prefs
                    .build_consents
                    .insert(consent.plan.consent_key());
            });
            Self::save_extension_prefs(cx);
        }
        Self::build_grammar(consent.plan, compiler, cx);
    }

    /// Download, compile, verify (in a helper process) and register a
    /// grammar; on success the extension's metadata records the library.
    fn build_grammar(plan: BuildPlan, compiler: Compiler, cx: &mut dyn Host) {
        let key = format!("{}/{}", plan.extension, plan.grammar);
        let state = Self::state(cx);
        if state.read(cx).extensions.builds.contains_key(&key) {
            return;
        }
        state.update(cx, |s, cx| {
            s.extensions.build_errors.remove(&key);
            s.extensions.builds.insert(
                key.clone(),
                BuildProgress {
                    stage: Stage::Downloading {
                        received: 0,
                        total: None,
                    },
                },
            );
            cx.notify();
        });
        let (tx, rx) = async_channel::unbounded::<Stage>();
        cx.spawn({
            let key = key.clone();
            async move |cx: &mut AsyncCtx| {
                while let Ok(stage) = rx.recv().await {
                    state.update(cx, |s, cx| {
                        if let Some(build) = s.extensions.builds.get_mut(&key) {
                            build.stage = stage;
                            cx.notify();
                        }
                    });
                }
            }
        })
        .detach();
        let (extension, grammar) = (plan.extension.clone(), plan.grammar.clone());
        let dir = extensions_dir().join(&plan.extension);
        spawn_bg(
            cx,
            move || {
                let mut progress = |stage: Stage| {
                    let _ = tx.try_send(stage);
                };
                let built = tsbuild::build(&plan, &compiler, &mut progress, &verify_in_helper)?;
                // record the library in the extension's metadata
                let mut metadata = install::read(&dir)?;
                if let Some(status) = metadata.grammars.iter_mut().find(|g| g.name == grammar) {
                    status.resolution = Resolution::Built {
                        library: built.library.to_string_lossy().into_owned(),
                    };
                    status.error = None;
                    if status.queries.is_some() {
                        status.status = Status::Ok;
                    }
                }
                install::write(&dir, &metadata)?;
                Ok::<Metadata, corvene_extensions::ExtensionError>(metadata)
            },
            move |result, cx| {
                Self::state(cx).update(cx, |s, cx| {
                    s.extensions.builds.remove(&key);
                    match &result {
                        Ok(metadata) => {
                            if let Some(installed) = s
                                .extensions
                                .installed
                                .iter_mut()
                                .find(|i| i.metadata.id == extension)
                            {
                                let (enabled, prefer) = (
                                    installed.metadata.enabled,
                                    installed.metadata.prefer_over_builtin,
                                );
                                installed.metadata = metadata.clone();
                                installed.metadata.enabled = enabled;
                                installed.metadata.prefer_over_builtin = prefer;
                            }
                        }
                        Err(err) => {
                            warn!(%err, key, "grammar build failed");
                            s.extensions
                                .build_errors
                                .insert(key.clone(), err.to_string());
                        }
                    }
                    cx.notify();
                });
                if result.is_ok() {
                    info!(key, "grammar built from source");
                    Self::rebuild_user_syntaxes(cx);
                }
            },
        );
    }

    /// Turn an extension on or off.
    pub fn set_extension_enabled(id: &str, enabled: bool, cx: &mut dyn Host) {
        Self::update_switches(id, cx, |switches| switches.enabled = enabled);
        Self::rebuild_user_syntaxes(cx);
    }

    /// Whether an extension's grammars win over the built-in highlighting
    /// for the files they claim: the default for every language of the
    /// extension (per-language answers are cleared).
    pub fn set_extension_preferred(id: &str, preferred: bool, cx: &mut dyn Host) {
        Self::update_switches(id, cx, |switches| {
            switches.prefer_over_builtin = preferred;
            switches.languages.clear();
        });
        Self::rebuild_user_syntaxes(cx);
    }

    /// One language's own answer.
    pub fn set_language_preferred(id: &str, language: &str, preferred: bool, cx: &mut dyn Host) {
        let language = language.to_string();
        Self::update_switches(id, cx, |switches| {
            switches.languages.insert(language, preferred);
        });
        Self::rebuild_user_syntaxes(cx);
    }

    fn update_switches(id: &str, cx: &mut dyn Host, change: impl FnOnce(&mut ExtensionSwitches)) {
        let changed = Self::state(cx).update(cx, |s, cx| {
            let installed = s
                .extensions
                .installed
                .iter_mut()
                .find(|i| i.metadata.id == id)?;
            let mut switches =
                s.extensions
                    .prefs
                    .switches
                    .get(id)
                    .cloned()
                    .unwrap_or(ExtensionSwitches {
                        enabled: installed.metadata.enabled,
                        prefer_over_builtin: installed.metadata.prefer_over_builtin,
                        languages: HashMap::new(),
                    });
            change(&mut switches);
            installed.metadata.enabled = switches.enabled;
            installed.metadata.prefer_over_builtin = switches.prefer_over_builtin;
            s.extensions.prefs.switches.insert(id.to_string(), switches);
            cx.notify();
            Some((installed.dir.clone(), installed.metadata.clone()))
        });
        if let Some((dir, metadata)) = changed {
            Self::save_extension_prefs(cx);
            spawn_bg(
                cx,
                move || install::write(&dir, &metadata),
                |result, _| {
                    if let Err(err) = result {
                        warn!(%err, "could not update the extension's metadata");
                    }
                },
            );
        }
    }

    /// Delete an extension.
    pub fn remove_extension(id: &str, cx: &mut dyn Host) {
        let removed = Self::state(cx).update(cx, |s, cx| {
            let index = s
                .extensions
                .installed
                .iter()
                .position(|i| i.metadata.id == id)?;
            let installed = s.extensions.installed.remove(index);
            s.extensions.prefs.switches.remove(id);
            s.extensions.errors.remove(id);
            s.extensions.registered_tree_sitter.remove(id);
            if s.extensions.selected.as_deref() == Some(id) {
                s.extensions.selected = None;
            }
            cx.notify();
            Some(installed)
        });
        let Some(installed) = removed else {
            return;
        };
        treesitter::unregister_user(id);
        Self::save_extension_prefs(cx);
        let cache = extensions_cache_dir().join("grammars").join(id);
        spawn_bg(
            cx,
            move || {
                let result = install::remove(&installed.dir);
                let _ = std::fs::remove_dir_all(cache);
                result
            },
            |result, cx| {
                if let Err(err) = result {
                    warn!(%err, "could not remove the extension's folder");
                }
                Self::rebuild_user_syntaxes(cx);
            },
        );
    }

    /// Open the Language Extensions dialog, on top of Settings when opened
    /// from there. `return_to` reopens Settings on that tab when it closes
    /// and Settings is no longer open underneath.
    pub fn open_language_extensions(
        focus: Option<ExtensionsFocus>,
        return_to: Option<crate::PreferencesTab>,
        cx: &mut dyn Host,
    ) {
        if !Self::state(cx).read(cx).extensions.loaded {
            Self::load_language_extensions(cx);
        }
        Self::show_popup(crate::Popup::LanguageExtensions { focus, return_to }, cx);
    }

    /// Close the Language Extensions dialog, back to Settings when it was
    /// opened from there.
    pub fn close_language_extensions(cx: &mut dyn Host) {
        let (return_to, settings_below) = {
            let s = Self::state(cx).read(cx);
            let return_to = match s.popup() {
                Some(crate::Popup::LanguageExtensions { return_to, .. }) => *return_to,
                _ => None,
            };
            let settings_below = s
                .popups
                .all_popups()
                .iter()
                .any(|p| matches!(p.popup, crate::Popup::Preferences { .. }));
            (return_to, settings_below)
        };
        Self::close_popup_if(|p| matches!(p, crate::Popup::LanguageExtensions { .. }), cx);
        if let Some(tab) = return_to.filter(|_| !settings_below) {
            Self::show_popup(crate::Popup::Preferences { tab }, cx);
        }
    }

    /// The details panel's row.
    pub fn select_extension(id: Option<String>, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            if s.extensions.selected != id {
                s.extensions.selected = id;
                cx.notify();
            }
        });
    }

    /// `756-missing-highlighting-hint`: never show the hint for this
    /// suffix again.
    pub fn dismiss_suffix_hint(suffix: &str, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            s.extensions
                .prefs
                .dismissed_suffixes
                .insert(suffix.to_ascii_lowercase());
            cx.notify();
        });
        Self::save_extension_prefs(cx);
    }
}

/// Load a built library in a helper process (`corvene --verify-grammar`):
/// a library that crashes on load fails here instead of in the app.
fn verify_in_helper(library: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    let output = std::process::Command::new(exe)
        .arg("--verify-grammar")
        .arg(library)
        .output()
        .map_err(|err| format!("could not run the verifier: {err}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = stderr.lines().last().unwrap_or("").trim().to_string();
        Err(if message.is_empty() {
            format!("the verifier exited with {}", output.status)
        } else {
            message
        })
    }
}

/// Where a tree-sitter grammar's parser comes from: one of Corvene's own
/// when the repository or name matches, else a build from source.
pub fn resolve_grammar(grammar: &GrammarRef) -> Resolution {
    match grammar {
        GrammarRef::TreeSitter {
            name,
            repository,
            path,
            ..
        } => match treesitter::resolve_bundled(name, repository.as_deref(), path.as_deref()) {
            Some(bundled) => Resolution::Bundled { name: bundled },
            None if repository.is_some() => Resolution::NeedsBuild,
            None => Resolution::Failed {
                error: format!(
                    "Corvene has no grammar named {name} and the extension names no repository to build it from"
                ),
            },
        },
        _ => Resolution::Unresolved,
    }
}

/// The background half of [`Dispatcher::install_extension`].
fn install_from(
    source: &InstallSource,
    extensions_dir: &Path,
    staging: &Path,
    report: &mut dyn FnMut(ExtensionProgress),
) -> Result<Installed, corvene_extensions::ExtensionError> {
    use corvene_extensions::{http, scan};
    std::fs::create_dir_all(staging)?;
    let limits = corvene_extensions::archive::Limits::default();
    let mut download = |url: &str, name: &str, extra_host: Option<&str>| {
        let dest = staging.join(name);
        let mut progress = |received, total| {
            report(ExtensionProgress::Downloading { received, total });
        };
        let downloaded = http::download(
            url,
            &dest,
            http::MAX_ARCHIVE_BYTES,
            extra_host,
            &mut progress,
        )?;
        report(ExtensionProgress::Unpacking);
        Ok::<_, corvene_extensions::ExtensionError>((dest, downloaded))
    };
    let (scanned, record, id) = match source {
        InstallSource::LocalPath(path) => {
            let scanned = scan::scan(path, staging, &limits)?;
            let kind = if path.is_dir() {
                SourceKind::LocalFolder
            } else {
                SourceKind::LocalFile
            };
            let record = Source {
                kind,
                url: None,
                path: Some(path.to_string_lossy().into_owned()),
                sha256: None,
                editor: None,
                registry_id: None,
            };
            let id = install::extension_id(
                kind,
                scanned.manifest.publisher.as_deref(),
                &scanned.manifest.name,
            );
            (scanned, record, id)
        }
        InstallSource::Url(url) => {
            let name = url
                .rsplit('/')
                .next()
                .filter(|n| !n.is_empty() && !n.contains('?'))
                .unwrap_or("download.bin");
            let (file, downloaded) = download(url, name, http::host_of(url).as_deref())?;
            let scanned = scan::scan(&file, staging, &limits)?;
            let record = Source {
                kind: SourceKind::Url,
                url: Some(url.clone()),
                path: None,
                sha256: Some(downloaded.sha256),
                editor: None,
                registry_id: None,
            };
            let id = install::extension_id(
                SourceKind::Url,
                scanned.manifest.publisher.as_deref(),
                &scanned.manifest.name,
            );
            (scanned, record, id)
        }
        InstallSource::GitHub(repo) => {
            let (file, downloaded) = download(&repo.tarball_url(), "repository.tar.gz", None)?;
            let unpacked = staging.join("unpacked");
            corvene_extensions::archive::extract(&file, &unpacked, &limits)?;
            // codeload wraps the tree in `<repo>-<ref>/`
            let top = std::fs::read_dir(&unpacked)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .find(|p| p.is_dir())
                .unwrap_or(unpacked);
            let root = match &repo.path {
                Some(path) => top.join(path),
                None => top,
            };
            if !root.is_dir() {
                return Err(corvene_extensions::ExtensionError::NotAnExtension(format!(
                    "{} has no folder {}",
                    repo.url(),
                    repo.path.as_deref().unwrap_or("")
                )));
            }
            let mut scanned = scan::scan_dir(&root)?;
            if scanned.root == root
                && scanned.manifest.name == root.file_name().and_then(|n| n.to_str()).unwrap_or("")
            {
                scanned.manifest.name = repo.id_parts().1;
            }
            if scanned.manifest.repository.is_none() {
                scanned.manifest.repository = Some(repo.url());
            }
            let record = Source {
                kind: SourceKind::GitHub,
                url: Some(repo.url()),
                path: repo.path.clone(),
                sha256: Some(downloaded.sha256),
                editor: None,
                registry_id: Some(format!("{}/{}@{}", repo.owner, repo.repo, repo.reference)),
            };
            let (owner, name) = repo.id_parts();
            let id = install::extension_id(SourceKind::GitHub, Some(&owner), &name);
            (scanned, record, id)
        }
        InstallSource::Registry(candidate) => {
            let (file, downloaded) = download(&candidate.download_url, "extension.archive", None)?;
            let mut scanned = scan::scan(&file, staging, &limits)?;
            if scanned.manifest.version.is_none() {
                scanned.manifest.version = candidate.version.clone();
            }
            if scanned.manifest.repository.is_none() {
                scanned.manifest.repository = candidate.repository.clone();
            }
            if scanned.manifest.display_name.is_none() {
                scanned.manifest.display_name = Some(candidate.display_name.clone());
            }
            let record = Source {
                kind: candidate.registry.source_kind(),
                url: Some(downloaded.url),
                path: None,
                sha256: Some(downloaded.sha256),
                editor: None,
                registry_id: Some(candidate.id.clone()),
            };
            (scanned, record, registry::extension_id(candidate))
        }
        InstallSource::Import(candidate) => {
            let scanned = scan::scan(&candidate.path, staging, &limits)?;
            let record = Source {
                kind: SourceKind::Imported,
                url: None,
                path: Some(candidate.path.to_string_lossy().into_owned()),
                sha256: None,
                editor: Some(candidate.editor.title().to_string()),
                registry_id: None,
            };
            let editor = install::slug(candidate.editor.title());
            let id =
                install::extension_id(SourceKind::Imported, Some(&editor), &scanned.manifest.name);
            (scanned, record, id)
        }
    };
    report(ExtensionProgress::Converting);
    let out = extensions_dir.join(&id);
    let prepared = install::prepare::prepare(&scanned, &id, record, &out, &resolve_grammar)?;
    info!(
        id,
        grammars = prepared.metadata.grammars.len(),
        "language extension installed"
    );
    Ok(Installed {
        dir: out,
        metadata: prepared.metadata,
    })
}

/// The background half of [`Dispatcher::rebuild_user_syntaxes`]: compile
/// the enabled extensions' sublime-syntax files into one set (from the
/// cached dump when its key matches), and collect their tree-sitter
/// grammars.
fn rebuild(
    enabled: &[Installed],
    extensions_dir: &Path,
    switches: &HashMap<String, ExtensionSwitches>,
) -> Rebuilt {
    use corvene_extensions::cache;
    use corvene_extensions::tm::compile::compile;
    let mut errors = Vec::new();
    let mut key_parts: Vec<String> = Vec::new();
    let mut sources: Vec<(String, bool, PathBuf, String)> = Vec::new(); // id, preferred, file, grammar name
    let mut tree_sitter = Vec::new();
    for installed in enabled {
        let md = &installed.metadata;
        // a grammar is preferred when any language it highlights is
        let language_preferred = |language: &corvene_extensions::manifest::Language| {
            switches
                .get(&md.id)
                .and_then(|sw| sw.languages.get(&language.id).copied())
                .unwrap_or(md.prefer_over_builtin)
        };
        let grammar_preferred = |name: &str| {
            let mut languages = md
                .languages
                .iter()
                .filter(|l| l.grammar.as_deref() == Some(name))
                .peekable();
            if languages.peek().is_none() {
                md.prefer_over_builtin
            } else {
                languages.any(language_preferred)
            }
        };
        let mut grammars_preferred = Vec::new();
        let mut grammars_fallback = Vec::new();
        for grammar in &md.grammars {
            match grammar.kind {
                GrammarKind::TextMate | GrammarKind::Sublime => {
                    if grammar.status == Status::Rejected {
                        continue;
                    }
                    let Some(file) = &grammar.file else {
                        continue;
                    };
                    let path = installed.dir.join(file);
                    if let Ok(meta) = std::fs::metadata(&path) {
                        key_parts.push(format!(
                            "{}@{}:{}:{}:{}",
                            md.id,
                            md.version.as_deref().unwrap_or(""),
                            file,
                            meta.len(),
                            meta.modified()
                                .ok()
                                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                                .map(|d| d.as_secs())
                                .unwrap_or(0)
                        ));
                        sources.push((
                            md.id.clone(),
                            grammar_preferred(&grammar.name),
                            path,
                            grammar.name.clone(),
                        ));
                    }
                }
                GrammarKind::TreeSitter => {
                    let language = match &grammar.resolution {
                        Resolution::Bundled { name } => {
                            UserLanguage::Derived { base: name.clone() }
                        }
                        Resolution::Built { library } => {
                            UserLanguage::Library(PathBuf::from(library))
                        }
                        Resolution::Unresolved
                        | Resolution::NeedsBuild
                        | Resolution::Failed { .. } => continue,
                    };
                    let Some(queries) = &grammar.queries else {
                        continue;
                    };
                    let read = |file: &str| {
                        std::fs::read_to_string(installed.dir.join(queries).join(file))
                            .unwrap_or_default()
                    };
                    let languages: Vec<_> = md
                        .languages
                        .iter()
                        .filter(|l| l.grammar.as_deref() == Some(grammar.name.as_str()))
                        .collect();
                    let first_line = languages
                        .iter()
                        .find_map(|l| l.first_line.as_deref())
                        .and_then(|re| regex::Regex::new(re).ok());
                    let bucket = if grammar_preferred(&grammar.name) {
                        &mut grammars_preferred
                    } else {
                        &mut grammars_fallback
                    };
                    bucket.push(UserGrammar {
                        name: format!("{}/{}", md.id, grammar.name),
                        extensions: languages.iter().flat_map(|l| l.suffixes.clone()).collect(),
                        filenames: languages.iter().flat_map(|l| l.filenames.clone()).collect(),
                        first_line,
                        aliases: languages.iter().flat_map(|l| l.aliases.clone()).collect(),
                        highlights: read("highlights.scm"),
                        injections: read("injections.scm"),
                        locals: read("locals.scm"),
                        language,
                    });
                }
            }
        }
        if !grammars_preferred.is_empty() {
            tree_sitter.push((md.id.clone(), true, grammars_preferred));
        }
        if !grammars_fallback.is_empty() {
            tree_sitter.push((md.id.clone(), false, grammars_fallback));
        }
    }
    if sources.is_empty() {
        for stale in dumps(extensions_dir) {
            let _ = std::fs::remove_file(stale);
        }
        return Rebuilt {
            set: None,
            tree_sitter,
            errors,
        };
    }
    let key = cache::cache_key(&key_parts);
    let dump_path = extensions_dir.join(format!("syntaxes-{key}.packdump"));
    let mut owners: Owners = HashMap::new();
    let mut definitions = Vec::new();
    for (id, preferred, path, grammar_name) in &sources {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) => {
                errors.push((id.clone(), format!("{grammar_name}: {err}")));
                continue;
            }
        };
        match compile(&text, grammar_name) {
            Ok(definition) => {
                owners.insert(definition.name.clone(), (id.clone(), *preferred));
                definitions.push(definition);
            }
            Err(err) => errors.push((id.clone(), format!("{grammar_name}: {err}"))),
        }
    }
    if definitions.is_empty() {
        return Rebuilt {
            set: None,
            tree_sitter,
            errors,
        };
    }
    let set = match cache::load(&dump_path) {
        Ok(set) if dump_path.is_file() => set,
        _ => {
            let external = cache::external_scopes(&definitions);
            let base = (!external.is_empty()).then(|| {
                info!(scopes = ?external, "user grammars include built-in scopes: building on the bundled set");
                (*corvene_highlight::syntaxes::current()).clone()
            });
            let set = cache::build_set(definitions, base);
            for stale in dumps(extensions_dir) {
                let _ = std::fs::remove_file(stale);
            }
            if let Err(err) = cache::dump(&set, &dump_path) {
                warn!(%err, "could not cache the user grammar set");
            }
            set
        }
    };
    Rebuilt {
        set: Some((set, owners)),
        tree_sitter,
        errors,
    }
}

/// Every `syntaxes-*.packdump` in the folder.
fn dumps(extensions_dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(extensions_dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("syntaxes-") && n.ends_with(".packdump"))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[allow(dead_code)]
fn _metadata_type_check(_: &Metadata) {}
