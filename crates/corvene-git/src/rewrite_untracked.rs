//! Corvene `1226-rewrite-keeps-untracked` (desktop/desktop#18331): a history
//! rewrite must not touch a file that is untracked now.
//!
//! GitHub Desktop replays commits with `git rebase -i`
//! (`app/src/lib/git/rebase.ts`, `squash.ts`, `reorder.ts`). A file a commit
//! in the range stopped tracking (`git rm --cached`) is written by the replay
//! of an older commit, over whatever is on disk when it is ignored, and
//! deleted by the replay of the commit that removed it. When it is not
//! ignored the replay stops with "untracked working tree files would be
//! overwritten". So those files are moved into the git directory first and
//! brought back once no rebase is in progress: after the rebase, or after a
//! `--continue` / `--abort` when it stopped on a conflict.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::detect::GitBinary;
use crate::error::Result;
use crate::paths::git_dir;
use crate::process::GitCommand;

const FOLDER: &str = "corvene-set-aside";
const MANIFEST: &str = "manifest";

/// The paths a commit after `last_retained_ref` deleted that `HEAD` does not
/// track and that are on disk as files.
fn paths_to_set_aside(
    git: Arc<GitBinary>,
    workdir: &Path,
    last_retained_ref: Option<&str>,
) -> Result<Vec<String>> {
    let range = match last_retained_ref {
        Some(base) => format!("{base}..HEAD"),
        None => "HEAD".to_string(),
    };
    let deleted = GitCommand::new(git.clone())
        .args([
            "log",
            "--no-renames",
            "--diff-filter=D",
            "--name-only",
            "--format=",
            "-z",
            &range,
        ])
        .current_dir(workdir)
        .run()?
        .stdout;
    if deleted.is_empty() {
        return Ok(Vec::new());
    }
    let tracked = GitCommand::new(git)
        .args(["ls-tree", "-r", "-z", "--name-only", "HEAD"])
        .current_dir(workdir)
        .run()?
        .stdout;
    let tracked: HashSet<&[u8]> = tracked.split(|b| *b == 0).collect();
    let mut seen = HashSet::new();
    let mut paths = Vec::new();
    for path in deleted.split(|b| *b == 0) {
        // the log's format lines leave empty pieces and newlines
        let path = path.strip_prefix(b"\n").unwrap_or(path);
        if path.is_empty() || tracked.contains(path) || !seen.insert(path) {
            continue;
        }
        let path = String::from_utf8_lossy(path).into_owned();
        let on_disk = workdir
            .join(&path)
            .symlink_metadata()
            .is_ok_and(|meta| !meta.is_dir());
        if on_disk {
            paths.push(path);
        }
    }
    Ok(paths)
}

/// Move the files a rewrite of the commits after `last_retained_ref` would
/// overwrite or delete out of the working directory, until
/// [`restore_set_aside`].
pub fn set_aside_for_rewrite(
    git: Arc<GitBinary>,
    workdir: &Path,
    last_retained_ref: Option<&str>,
) -> Result<()> {
    // what an earlier rewrite finished outside the app left behind
    restore_set_aside(workdir)?;
    let paths = paths_to_set_aside(git, workdir, last_retained_ref)?;
    if paths.is_empty() {
        return Ok(());
    }
    let folder = git_dir(workdir).join(FOLDER);
    std::fs::create_dir_all(&folder)?;
    // written first: a file is never away without the list that finds it
    std::fs::write(folder.join(MANIFEST), paths.join("\0"))?;
    for (index, path) in paths.iter().enumerate() {
        move_file(&workdir.join(path), &folder.join(index.to_string()))?;
    }
    Ok(())
}

/// Bring back what [`set_aside_for_rewrite`] moved, unless a rebase is still
/// in progress. A file the rewritten history now tracks stays as it is and
/// the set-aside copy goes next to it as `<name>.orig` when they differ.
pub fn restore_set_aside(workdir: &Path) -> Result<()> {
    if crate::rebase_ops::rebase_head_set(workdir) {
        return Ok(());
    }
    let folder = git_dir(workdir).join(FOLDER);
    let Ok(manifest) = std::fs::read_to_string(folder.join(MANIFEST)) else {
        return Ok(());
    };
    for (index, path) in manifest.split('\0').enumerate() {
        let kept = folder.join(index.to_string());
        if kept.symlink_metadata().is_err() {
            continue;
        }
        let target = workdir.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if target.symlink_metadata().is_err() {
            move_file(&kept, &target)?;
        } else if same_content(&kept, &target) {
            std::fs::remove_file(&kept)?;
        } else {
            move_file(&kept, &free_name(&target))?;
        }
    }
    std::fs::remove_dir_all(folder)?;
    Ok(())
}

fn same_content(a: &Path, b: &Path) -> bool {
    matches!((std::fs::read(a), std::fs::read(b)), (Ok(a), Ok(b)) if a == b)
}

/// `<name>.orig`, numbered when that is taken.
fn free_name(target: &Path) -> PathBuf {
    let with_suffix = |suffix: String| {
        let mut name = target.as_os_str().to_os_string();
        name.push(suffix);
        PathBuf::from(name)
    };
    let mut candidate = with_suffix(".orig".to_string());
    let mut n = 1;
    while candidate.symlink_metadata().is_ok() {
        candidate = with_suffix(format!(".orig{n}"));
        n += 1;
    }
    candidate
}

/// A linked worktree's git directory can be on another volume.
fn move_file(from: &Path, to: &Path) -> Result<()> {
    if std::fs::rename(from, to).is_err() {
        std::fs::copy(from, to)?;
        std::fs::remove_file(from)?;
    }
    Ok(())
}
