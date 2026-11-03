//! Filesystem watcher for the selected repository (Corvene addition; GHD only
//! refreshes on focus and after its own actions). FSEvents via `notify`,
//! debounced 300 ms on a helper thread, delivered as a coalesced "refresh"
//! signal (with the time of the burst's last event) over an async channel the
//! foreground awaits.
//!
//! Worktree paths git ignores (`target/`, `node_modules/`, …) do not count,
//! so a build writing into them does not refresh every debounce window. The
//! rules come from gitoxide's exclude stack ([`corvene_git::ignore::IgnoreMatcher`]:
//! the `.gitignore` files, `.git/info/exclude`, `core.excludesFile`), built
//! on the watcher thread and rebuilt after a `.gitignore`, `info/exclude`, the
//! config or the index changes. Inside `.git/` the name rules of
//! [`is_relevant`] apply as before.
//!
//! Where the system cannot report changes in the folder it is polled every
//! [`POLL_INTERVAL`] instead (`notify`'s `PollWatcher`): Windows refuses
//! `ReadDirectoryChangesW` on some network redirectors (a VDI user profile
//! on an SMB share, desktop/desktop#22972, where GHD fails with "UNKNOWN:
//! unknown error, watch"), and inotify runs out of watches in big trees.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use corvene_git::ignore::IgnoreMatcher;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tracing::{debug, warn};

/// The default debounce (`904-fs-watcher-debounce-ms` sets the real one).
pub const DEBOUNCE: Duration = Duration::from_millis(300);

/// How often a folder the system cannot watch is looked at again.
pub const POLL_INTERVAL: Duration = Duration::from_secs(5);

pub struct RepoWatcher {
    _watcher: Box<dyn Watcher>,
}

/// Start watching `workdir`, coalescing bursts closer than `debounce`.
/// Each signal carries when the last event it covers arrived, so a refresh
/// that started later can be skipped. With `leading` (flag
/// `902-fs-watcher-leading-edge`) the first relevant event of a quiet period
/// is signalled at once and the rest of the burst after it settles.
/// Dropping the returned watcher stops everything.
pub fn watch(
    workdir: PathBuf,
    debounce: Duration,
    leading: bool,
) -> anyhow::Result<(RepoWatcher, async_channel::Receiver<Instant>)> {
    let (raw_tx, raw_rx) = mpsc::channel::<Vec<PathBuf>>();
    let handler = |raw_tx: mpsc::Sender<Vec<PathBuf>>| {
        move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res
                && !is_open(&event.kind)
                && !(cfg!(windows) && is_directory_touch(&event.kind, &event.paths))
            {
                let _ = raw_tx.send(event.paths);
            }
        }
    };
    let watcher: Box<dyn Watcher> = match native_watcher(&workdir, handler(raw_tx.clone())) {
        Ok(watcher) => Box::new(watcher),
        Err(err) => {
            warn!(?err, path = %workdir.display(), "no change events for the repository; polling it");
            let config = notify::Config::default().with_poll_interval(POLL_INTERVAL);
            let mut watcher = notify::PollWatcher::new(handler(raw_tx), config)?;
            watcher.watch(&workdir, RecursiveMode::Recursive)?;
            Box::new(watcher)
        }
    };

    // unbounded: every burst's time reaches the dispatcher (a refresh that
    // started before it must not swallow it)
    let (tx, rx) = async_channel::unbounded::<Instant>();
    // FSEvents reports resolved paths (`/private/var/…` for `/var/…`).
    // Windows reports paths under the folder as it was given: `canonicalize`'s
    // `\\?\` form is not a prefix of those (hence dunce), and neither is the
    // resolved path when the folder was given by a short name
    // (`C:\Users\RUNNER~1\…`), so the folder as given counts as the root too
    let root = dunce::canonicalize(&workdir).unwrap_or_else(|_| workdir.clone());
    std::thread::Builder::new()
        .name("repo-watcher".into())
        .spawn(move || {
            let mut rules = Relevance::new(root.clone()).with_alias(workdir);
            // Each iteration: wait for one relevant event, then absorb the burst.
            while let Ok(paths) = raw_rx.recv() {
                let mut relevant = rules.any_relevant(&paths, false);
                let mut last = Instant::now();
                // leading edge: this event is signalled now; only later ones
                // make the burst's own signal necessary
                if relevant && leading {
                    debug!(path = ?paths.first(), "filesystem change, refresh requested (leading)");
                    let _ = tx.try_send(last);
                    relevant = false;
                }
                loop {
                    match raw_rx.recv_timeout(debounce) {
                        Ok(more) => {
                            if rules.any_relevant(&more, false) {
                                relevant = true;
                                last = Instant::now();
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => break,
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
                if relevant {
                    debug!(path = %root.display(), "filesystem change, refresh requested");
                    let _ = tx.try_send(last);
                }
            }
        })
        .map_err(|e| {
            warn!(?e, "could not spawn watcher thread");
            e
        })?;

    Ok((RepoWatcher { _watcher: watcher }, rx))
}

/// The system's change events for `workdir`. On Windows the folder is first
/// asked whether it takes them: `notify` starts without an error where it
/// does not, and then never reports anything.
fn native_watcher(
    workdir: &Path,
    handler: impl notify::EventHandler,
) -> notify::Result<RecommendedWatcher> {
    #[cfg(windows)]
    if !corvene_platform::windows::directory_changes_supported(workdir) {
        return Err(notify::Error::generic(
            "the folder does not report changes (ReadDirectoryChangesW)",
        ));
    }
    let mut watcher = notify::recommended_watcher(handler)?;
    watcher.watch(workdir, RecursiveMode::Recursive)?;
    Ok(watcher)
}

/// inotify also reports files being opened (FSEvents never does): git reading
/// `.git/HEAD` during a refresh would ask for the next refresh, without end.
/// A finished write still arrives as `Access(Close(Write))`.
fn is_open(kind: &notify::EventKind) -> bool {
    matches!(
        kind,
        notify::EventKind::Access(notify::event::AccessKind::Open(_))
    )
}

/// Windows also reports a folder as modified when something in it changes
/// or, for its access time, when it is listed: git listing `.git/refs` during
/// a refresh would ask for the next refresh, without end. What changed in
/// the folder has an event of its own, and a folder that is created, removed
/// or renamed is not reported this way.
fn is_directory_touch(kind: &notify::EventKind, paths: &[PathBuf]) -> bool {
    matches!(
        kind,
        notify::EventKind::Modify(notify::event::ModifyKind::Any)
    ) && !paths.is_empty()
        && paths.iter().all(|path| path.is_dir())
}

/// Relevance with the repository's ignore rules: [`is_relevant`] for `.git/`,
/// worktree paths unless git ignores them.
pub struct Relevance {
    root: PathBuf,
    /// Another spelling of `root` that event paths may start with.
    alias: Option<PathBuf>,
    /// `None` until first needed, after a rules file changed, or when the
    /// repository could not be opened (then every worktree path counts).
    matcher: Option<IgnoreMatcher>,
    stale: bool,
}

impl Relevance {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            alias: None,
            matcher: None,
            stale: true,
        }
    }

    /// Paths under `alias` are paths under the root as well.
    pub fn with_alias(mut self, alias: PathBuf) -> Self {
        self.alias = Some(alias).filter(|alias| *alias != self.root);
        self
    }

    /// `path` relative to the root, by either of its spellings.
    fn relative<'a>(&self, path: &'a Path) -> Option<&'a Path> {
        path.strip_prefix(&self.root).ok().or_else(|| {
            self.alias
                .as_deref()
                .and_then(|alias| path.strip_prefix(alias).ok())
        })
    }

    /// Whether a refresh is due after `paths` changed, given `already` from
    /// earlier events of the same burst. Every path is still looked at for
    /// rule changes, but ignore matching stops once the answer is known.
    pub fn any_relevant(&mut self, paths: &[PathBuf], already: bool) -> bool {
        let mut relevant = already;
        for path in paths {
            if self.changes_rules(path) {
                self.stale = true;
            }
            if !relevant {
                relevant = self.is_relevant(path);
            }
        }
        relevant
    }

    pub fn is_relevant(&mut self, path: &Path) -> bool {
        let Some(rel) = self.relative(path) else {
            return true;
        };
        if rel.starts_with(".git") {
            return is_relevant(Path::new(""), rel);
        }
        !self.is_ignored(rel)
    }

    fn is_ignored(&mut self, rel: &Path) -> bool {
        if self.stale {
            self.stale = false;
            self.matcher = IgnoreMatcher::open(&self.root)
                .map_err(|err| debug!(?err, "no ignore rules for the watcher"))
                .ok();
        }
        self.matcher
            .as_mut()
            .is_some_and(|matcher| matcher.is_ignored(rel))
    }

    /// A change to a file the ignore rules or the tracked set come from. A
    /// `.gitignore` in an ignored directory (`node_modules/pkg/.gitignore`)
    /// is not one: git never reads it.
    fn changes_rules(&mut self, path: &Path) -> bool {
        let Some(rel) = self.relative(path) else {
            return false;
        };
        if rel.file_name().is_some_and(|name| name == ".gitignore") {
            return !rel.starts_with(".git") && (self.stale || !self.is_ignored(rel));
        }
        [
            ".git/index",
            ".git/config",
            ".git/info/exclude",
            ".git/info",
        ]
        .iter()
        .any(|p| rel == Path::new(p))
    }
}

/// Inside `.git/` only the refs/index/HEAD family matters; object writes and
/// lock files are noise. Paths outside `.git/` count here; [`Relevance`]
/// drops the ones git ignores.
pub fn is_relevant(root: &Path, path: &Path) -> bool {
    let rel = match path.strip_prefix(root) {
        Ok(rel) => rel,
        Err(_) => return true,
    };
    let mut comps = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned());
    let first = comps.next().unwrap_or_default();
    if first != ".git" {
        return true;
    }
    let rest: Vec<String> = comps.collect();
    let Some(second) = rest.first() else {
        return false;
    };
    let name = rest.last().cloned().unwrap_or_default();
    if name.ends_with(".lock") {
        return false;
    }
    match second.as_str() {
        "objects" | "modules" | "lfs" | "hooks" | "info" | "worktrees" => false,
        "HEAD" | "index" | "packed-refs" | "MERGE_HEAD" | "REBASE_HEAD" | "CHERRY_PICK_HEAD"
        | "ORIG_HEAD" | "FETCH_HEAD" | "config" => true,
        // `1212-bisect`: `git bisect start` touches nothing else
        "BISECT_START" | "BISECT_TERMS" | "BISECT_LOG" => true,
        "refs" | "logs" | "rebase-merge" | "rebase-apply" => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relevance_rules() {
        let root = Path::new("/r");
        assert!(is_relevant(root, Path::new("/r/src/main.rs")));
        assert!(is_relevant(root, Path::new("/r/.git/HEAD")));
        assert!(is_relevant(root, Path::new("/r/.git/refs/heads/main")));
        assert!(is_relevant(root, Path::new("/r/.git/index")));
        assert!(!is_relevant(root, Path::new("/r/.git/index.lock")));
        assert!(!is_relevant(root, Path::new("/r/.git/objects/ab/cdef")));
        assert!(!is_relevant(root, Path::new("/r/.git/hooks/pre-commit")));
        assert!(!is_relevant(root, Path::new("/r/.git")));
    }

    fn init_repo(root: &Path) {
        let git = std::sync::Arc::new(corvene_git::find_git().unwrap());
        corvene_git::init_repository(
            git,
            corvene_git::InitOptions {
                path: root.to_path_buf(),
                default_branch: None,
                description: None,
                readme: false,
                gitignore: None,
                license: None,
                git_attributes: None,
                keep_existing: false,
            },
        )
        .unwrap();
    }

    #[test]
    fn relevance_follows_ignore_rules() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        init_repo(&root);
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        let mut rules = Relevance::new(root.clone());
        assert!(!rules.is_relevant(&root.join("target/debug/deps/x.o")));
        assert!(rules.is_relevant(&root.join("src/main.rs")));
        assert!(rules.is_relevant(&root.join(".gitignore")));
        assert!(rules.is_relevant(&root.join(".git/HEAD")));
        assert!(!rules.is_relevant(&root.join(".git/objects/ab/cdef")));
        assert!(rules.is_relevant(Path::new("/elsewhere/a.txt")));
        // a burst of ignored writes and object writes stays quiet
        let burst = [
            root.join("target/debug/a.o"),
            root.join("target/debug/b.o"),
            root.join(".git/objects/aa/bb"),
        ];
        assert!(!rules.any_relevant(&burst, false));
        assert!(rules.any_relevant(&burst, true));

        // editing .gitignore counts and rebuilds the rules
        std::fs::write(root.join(".gitignore"), "node_modules/\n").unwrap();
        assert!(rules.any_relevant(&[root.join(".gitignore")], false));
        assert!(rules.is_relevant(&root.join("target/debug/deps/x.o")));
        assert!(!rules.is_relevant(&root.join("node_modules/pkg/index.js")));
        // a nested one too, even when a relevant path came first
        std::fs::create_dir_all(root.join("web")).unwrap();
        std::fs::write(root.join("web/.gitignore"), "dist\n").unwrap();
        assert!(rules.any_relevant(&[root.join("a.txt"), root.join("web/.gitignore")], false));
        assert!(!rules.is_relevant(&root.join("web/dist/app.js")));
        assert!(rules.is_relevant(&root.join("dist/app.js")));

        // .gitignore files git never reads change nothing
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        std::fs::write(root.join("node_modules/pkg/.gitignore"), "*\n").unwrap();
        assert!(!rules.any_relevant(&[root.join("node_modules/pkg/.gitignore")], false));
        assert!(!rules.stale);

        // a new .git/info/exclude applies but does not refresh by itself
        std::fs::create_dir_all(root.join(".git/info")).unwrap();
        std::fs::write(root.join(".git/info/exclude"), "*.tmp\n").unwrap();
        assert!(!rules.any_relevant(&[root.join(".git/info/exclude")], false));
        assert!(!rules.is_relevant(&root.join("scratch.tmp")));
    }

    #[test]
    fn an_alias_of_the_root_is_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        init_repo(&root);
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        // the spelling the events use: a short name on Windows, a link here
        let alias = Path::new("/as/given");
        let mut rules = Relevance::new(root.clone()).with_alias(alias.to_path_buf());
        assert!(!rules.is_relevant(&alias.join("target/debug/x.o")));
        assert!(!rules.is_relevant(&alias.join(".git/objects/ab/cdef")));
        assert!(rules.is_relevant(&alias.join(".git/HEAD")));
        assert!(rules.is_relevant(&alias.join("src/main.rs")));
        assert!(!rules.is_relevant(&root.join("target/debug/x.o")));
        // a rules file under the alias rebuilds the rules
        std::fs::write(root.join(".gitignore"), "dist/\n").unwrap();
        assert!(rules.any_relevant(&[alias.join(".gitignore")], false));
        assert!(rules.is_relevant(&alias.join("target/debug/x.o")));
        assert!(!rules.is_relevant(&alias.join("dist/app.js")));
    }

    #[test]
    fn everything_counts_without_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        let mut rules = Relevance::new(root.clone());
        assert!(rules.is_relevant(&root.join("target/debug/x.o")));
        assert!(!rules.is_relevant(&root.join(".git/objects/ab/cdef")));
    }

    /// Watch `dir`, run `act` and say whether a refresh was signalled, with
    /// the raw events of that time for the failure message. What was
    /// written before the watch began is left to settle first: Windows
    /// reports a write when its cache flushes it, up to seconds later, and
    /// FSEvents replays the last moments before a stream starts.
    fn refreshed_by(dir: &Path, act: impl FnOnce()) -> (bool, Vec<String>) {
        let (_watcher, rx) = watch(dir.to_path_buf(), Duration::from_millis(100), false).unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut raw = notify::recommended_watcher({
            let seen = seen.clone();
            move |res: notify::Result<notify::Event>| {
                if let Ok(event) = res {
                    seen.lock()
                        .unwrap()
                        .push(format!("{:?} {:?}", event.kind, event.paths));
                }
            }
        })
        .unwrap();
        raw.watch(dir, RecursiveMode::Recursive).unwrap();
        let signalled = |wait: Duration| {
            smol::block_on(smol::future::or(async { rx.recv().await.is_ok() }, async {
                smol::Timer::after(wait).await;
                false
            }))
        };
        for _ in 0..10 {
            if !signalled(Duration::from_millis(1500)) {
                break;
            }
        }
        seen.lock().unwrap().clear();
        act();
        let got = signalled(Duration::from_millis(1500));
        let events = seen.lock().unwrap().clone();
        (got, events)
    }

    #[test]
    fn quiet_on_ignored_change() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
        std::fs::create_dir(dir.path().join("target")).unwrap();
        let (got, events) = refreshed_by(dir.path(), || {
            for i in 0..20 {
                std::fs::write(dir.path().join(format!("target/{i}.o")), "x").unwrap();
            }
        });
        assert!(
            !got,
            "writes under an ignored directory must not refresh: {events:#?}"
        );
    }

    #[test]
    fn open_events_are_dropped() {
        use notify::EventKind;
        use notify::event::{AccessKind, AccessMode, DataChange, ModifyKind};
        assert!(is_open(&EventKind::Access(AccessKind::Open(
            AccessMode::Any
        ))));
        assert!(!is_open(&EventKind::Access(AccessKind::Close(
            AccessMode::Write
        ))));
        assert!(!is_open(&EventKind::Modify(ModifyKind::Data(
            DataChange::Any
        ))));
    }

    #[test]
    fn directory_touches_are_told_apart() {
        use notify::EventKind;
        use notify::event::{CreateKind, ModifyKind, RenameMode};
        let dir = tempfile::tempdir().unwrap();
        let refs = dir.path().join("refs");
        std::fs::create_dir(&refs).unwrap();
        let file = refs.join("main");
        std::fs::write(&file, "x").unwrap();
        let touched = EventKind::Modify(ModifyKind::Any);
        assert!(is_directory_touch(&touched, std::slice::from_ref(&refs)));
        // a file's change, a new folder and a renamed one all count
        assert!(!is_directory_touch(&touched, std::slice::from_ref(&file)));
        assert!(!is_directory_touch(
            &EventKind::Create(CreateKind::Any),
            std::slice::from_ref(&refs)
        ));
        assert!(!is_directory_touch(
            &EventKind::Modify(ModifyKind::Name(RenameMode::To)),
            std::slice::from_ref(&refs)
        ));
        // so does a path that is gone by now
        assert!(!is_directory_touch(&touched, &[dir.path().join("gone")]));
        assert!(!is_directory_touch(&touched, &[]));
    }

    #[test]
    fn quiet_on_read() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        let (got, events) = refreshed_by(dir.path(), || {
            // what a refresh does: git reads HEAD, the refs and worktree files
            std::fs::read(dir.path().join(".git/HEAD")).unwrap();
            std::fs::read_dir(dir.path().join(".git/refs"))
                .unwrap()
                .count();
            std::fs::read(dir.path().join("a.txt")).unwrap();
        });
        assert!(!got, "reading files must not refresh: {events:#?}");
    }

    #[test]
    fn signals_on_worktree_change() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let (_watcher, rx) = watch(dir.path().to_path_buf(), DEBOUNCE, false).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        let got = smol::block_on(async {
            smol::future::or(async { rx.recv().await.is_ok() }, async {
                smol::Timer::after(Duration::from_secs(5)).await;
                false
            })
            .await
        });
        assert!(got, "expected a refresh signal");
    }
}
