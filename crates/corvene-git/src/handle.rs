//! Shared gitoxide handles. `gix::open` discovers the repository and parses
//! every configuration file (system, global, repository, includes) each time;
//! reads run on every refresh, history page and diff, so the opened
//! repository is kept per path and handed out as a thread-local copy while
//! none of its configuration files changed. References and the object
//! database are read live by gitoxide, so nothing else can go stale.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

struct Entry {
    repo: gix::ThreadSafeRepository,
    /// The configuration files the handle was built from and their mtimes.
    stamps: Vec<(PathBuf, Option<SystemTime>)>,
}

static CACHE: LazyLock<Mutex<HashMap<PathBuf, Entry>>> = LazyLock::new(Default::default);

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn config_files(repo: &gix::Repository) -> Vec<PathBuf> {
    let mut files = vec![
        repo.git_dir().join("config"),
        repo.common_dir().join("config"),
        repo.git_dir().join("config.worktree"),
    ];
    // Windows: git looks in %USERPROFILE% when %HOME% is not set
    if let Some(home) = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
    {
        files.push(home.join(".gitconfig"));
        let xdg = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        files.push(xdg.join("git").join("config"));
    }
    // includes (`include.path`) are not tracked: they rarely change while
    // the app runs, and the next launch reads them again
    files.dedup();
    files
}

/// `gix::open(path)`, reusing the last handle for `path` while its
/// configuration is unchanged.
pub fn open(path: &Path) -> gix::Result<gix::Repository> {
    if let Ok(cache) = CACHE.lock()
        && let Some(entry) = cache.get(path)
        && entry
            .stamps
            .iter()
            .all(|(file, stamp)| mtime(file) == *stamp)
    {
        return Ok(entry.repo.to_thread_local());
    }
    let repo = gix::open(path)?;
    let stamps = config_files(&repo)
        .into_iter()
        .map(|file| {
            let stamp = mtime(&file);
            (file, stamp)
        })
        .collect();
    if let Ok(mut cache) = CACHE.lock() {
        cache.insert(
            path.to_path_buf(),
            Entry {
                repo: repo.clone().into_sync(),
                stamps,
            },
        );
    }
    Ok(repo)
}

/// [`open`], for answers that must equal git's: `None` unless gitoxide
/// trusts the repository fully. One git calls unsafe (owned by another
/// user, not in `safe.directory`) is refused by git but opened by gitoxide
/// without its repository configuration, so the two would disagree.
pub fn open_trusted(path: &Path) -> Option<gix::Repository> {
    open(path)
        .ok()
        .filter(|repo| repo.git_dir_trust() == gix::sec::Trust::Full)
}

/// The raw bytes of `<rev>:<path>` read in-process (what `git show
/// <rev>:<path>` prints for a blob); `None` when gitoxide cannot resolve it.
pub fn blob_bytes(workdir: &Path, rev: &str, path: &str) -> Option<Vec<u8>> {
    let repo = open(workdir).ok()?;
    let id = repo
        .rev_parse_single(format!("{rev}:{path}").as_str())
        .ok()?;
    let object = id.object().ok()?;
    (object.kind == gix::object::Kind::Blob).then(|| object.detach().data)
}

/// An object's bytes by id, read in-process; `None` when gitoxide cannot
/// find it (Corvene `1361-merge-editor` reads conflict stages this way).
pub fn object_bytes(workdir: &Path, oid: &str) -> Option<Vec<u8>> {
    let repo = open(workdir).ok()?;
    let id = gix::ObjectId::from_hex(oid.as_bytes()).ok()?;
    let object = repo.find_object(id).ok()?;
    (object.kind == gix::object::Kind::Blob).then(|| object.detach().data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reuses_the_handle_until_the_config_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        let git = |args: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(path)
                    .env("GIT_AUTHOR_NAME", "T")
                    .env("GIT_AUTHOR_EMAIL", "t@example.com")
                    .env("GIT_COMMITTER_NAME", "T")
                    .env("GIT_COMMITTER_EMAIL", "t@example.com")
                    .status()
                    .unwrap()
                    .success()
            )
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "commit.gpgsign", "false"]);
        std::fs::write(path.join("a.txt"), "one\ntwo\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        assert_eq!(
            blob_bytes(path, "HEAD", "a.txt").as_deref(),
            Some(&b"one\ntwo\n"[..])
        );
        assert_eq!(blob_bytes(path, "HEAD", "missing.txt"), None);
        // a new commit is visible through the cached handle
        std::fs::write(path.join("a.txt"), "three\n").unwrap();
        git(&["commit", "-q", "-am", "second"]);
        assert_eq!(
            blob_bytes(path, "HEAD", "a.txt").as_deref(),
            Some(&b"three\n"[..])
        );
        assert_eq!(
            blob_bytes(path, "HEAD^", "a.txt").as_deref(),
            Some(&b"one\ntwo\n"[..])
        );
        // so is a configuration change (mtime granularity: force a new stamp)
        std::thread::sleep(std::time::Duration::from_millis(20));
        git(&["config", "user.name", "Changed"]);
        let repo = open(path).unwrap();
        assert_eq!(
            repo.config_snapshot()
                .string("user.name")
                .map(|v| v.to_string()),
            Some("Changed".to_string())
        );
    }
}
