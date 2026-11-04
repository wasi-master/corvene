//! Read-side repository inspection via gitoxide.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use corvene_models::{AheadBehind, Branch, BranchKind, Identity, Remote, RepositoryInfo, Tip};
use gix::bstr::ByteSlice;

use crate::detect::GitBinary;
use crate::error::{GitError, Result};
use crate::process::GitCommand;

/// GHD `getRepositoryType` → `topLevelWorkingDirectory`: the working
/// directory containing `path` (a subdirectory of a repository resolves to
/// its root, as `git rev-parse --show-toplevel`); `None` outside one.
pub fn top_level_working_directory(path: &Path) -> Option<PathBuf> {
    let repo = gix::discover(path).ok()?;
    repo.workdir().map(Path::to_path_buf)
}

/// The main worktree of the repository at `path` (itself unless it is a
/// linked worktree), read by gitoxide from the common git dir so it works
/// where the git CLI refuses to run (unsafe repositories).
pub fn main_worktree_path(path: &Path) -> Option<PathBuf> {
    let repo = crate::handle::open(path).ok()?;
    let common = repo.common_dir();
    // dunce: without the `\\?\` prefix `canonicalize` adds on Windows
    let common = dunce::canonicalize(common).unwrap_or_else(|_| common.to_path_buf());
    if common.file_name().is_some_and(|n| n == ".git") {
        common.parent().map(Path::to_path_buf)
    } else {
        repo.workdir().map(Path::to_path_buf)
    }
}

/// Open `path` (a worktree or `.git` dir) and collect tip, branches, remotes
/// and identity. Cheap enough to run on every refresh.
pub fn open_repository(path: &Path) -> Result<RepositoryInfo> {
    let repo = crate::handle::open(path).map_err(|err| {
        if err.is_not_found() {
            GitError::NotARepository(path.to_path_buf())
        } else {
            GitError::from(err)
        }
    })?;
    let workdir = repo
        .workdir()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| repo.git_dir().to_path_buf());

    let remotes = remotes(&repo);
    let branches = branches(&repo, &remotes)?;
    let tip = tip(&repo, &branches)?;
    let identity = identity(&repo);
    let commit_template = crate::commit_template::read(&repo, &workdir);
    let diff_order = crate::commit_template::read_diff_order(&repo, &workdir);

    Ok(RepositoryInfo {
        workdir,
        tip,
        branches,
        remotes,
        identity,
        ahead_behind: None,
        commit_template,
        diff_order,
    })
}

fn identity(repo: &gix::Repository) -> Identity {
    let cfg = repo.config_snapshot();
    let local = cfg
        .plumbing()
        .sections_by_name("user")
        .into_iter()
        .flatten()
        .any(|section| {
            section.meta().source == gix::config::Source::Local
                && (section.value("name").is_some() || section.value("email").is_some())
        });
    Identity {
        name: cfg.string("user.name").map(|v| v.to_string()),
        email: cfg.string("user.email").map(|v| v.to_string()),
        local,
    }
}

fn remotes(repo: &gix::Repository) -> Vec<Remote> {
    let mut out = Vec::new();
    for name in repo.remote_names() {
        let name_str = name.to_string();
        if let Ok(remote) = repo.find_remote(name.as_bstr()) {
            let url = remote
                .url(gix::remote::Direction::Fetch)
                .map(|u| u.to_bstring().to_string())
                .unwrap_or_default();
            out.push(Remote {
                name: name_str,
                url,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn branches(repo: &gix::Repository, remote_list: &[Remote]) -> Result<Vec<Branch>> {
    let mut out = Vec::new();
    let platform = repo
        .references()
        .map_err(|e| GitError::Gix(e.to_string()))?;

    let locals = platform
        .local_branches()
        .map_err(|e| GitError::Gix(e.to_string()))?;
    for reference in locals {
        let mut reference = match reference {
            Ok(r) => r,
            Err(_) => continue,
        };
        let full_name = reference.name().as_bstr().to_string();
        let name = reference.name().shorten().to_string();
        let tip_id = reference.peel_to_id().ok();
        let tip = tip_id.map(|id| id.to_string());
        let tip_commit = tip_id.and_then(|id| repo.find_commit(id).ok());
        let tip_time = tip_commit
            .as_ref()
            .and_then(|c| c.time().ok())
            .map(|t| t.seconds);
        let tip_author = tip_commit.as_ref().and_then(tip_author_name);
        let upstream = repo
            .branch_remote_tracking_ref_name(reference.name(), gix::remote::Direction::Fetch)
            .and_then(|r| r.ok())
            .map(|r| r.as_bstr().to_string());
        let remote_name = upstream
            .as_deref()
            .and_then(|u| u.strip_prefix("refs/remotes/"))
            .and_then(|short| Branch::match_remote(short, remote_list))
            .map(str::to_string);
        out.push(Branch {
            name,
            kind: BranchKind::Local,
            full_name,
            tip,
            upstream,
            tip_time,
            tip_author,
            remote_name,
        });
    }

    let remotes = platform
        .remote_branches()
        .map_err(|e| GitError::Gix(e.to_string()))?;
    for reference in remotes {
        let mut reference = match reference {
            Ok(r) => r,
            Err(_) => continue,
        };
        let full_name = reference.name().as_bstr().to_string();
        let name = reference.name().shorten().to_string();
        if name.ends_with("/HEAD") {
            continue;
        }
        let tip_id = reference.peel_to_id().ok();
        let tip = tip_id.map(|id| id.to_string());
        let tip_commit = tip_id.and_then(|id| repo.find_commit(id).ok());
        let tip_time = tip_commit
            .as_ref()
            .and_then(|c| c.time().ok())
            .map(|t| t.seconds);
        let tip_author = tip_commit.as_ref().and_then(tip_author_name);
        let remote_name = Branch::match_remote(&name, remote_list).map(str::to_string);
        out.push(Branch {
            name,
            kind: BranchKind::Remote,
            full_name,
            tip,
            upstream: None,
            tip_time,
            tip_author,
            remote_name,
        });
    }
    Ok(out)
}

/// The name of a branch tip's author, for the branch list (Corvene,
/// `1201-branch-list-tip-author`).
fn tip_author_name(commit: &gix::Commit<'_>) -> Option<String> {
    commit
        .author()
        .ok()
        .map(|a| a.name.to_string())
        .filter(|n| !n.trim().is_empty())
}

/// Whether the repository has stash entries the app can show, for the
/// repository list's stash icon (Corvene, `270-repository-list-stash-icon`):
/// any entry of `refs/stash` when `any` (`728-show-latest-other-stash`),
/// else only a `!!GitHub_Desktop<branch>` one.
pub fn has_stash(path: &Path, any: bool) -> bool {
    let Ok(repo) = crate::handle::open(path) else {
        return false;
    };
    let Ok(Some(stash)) = repo.try_find_reference("refs/stash") else {
        return false;
    };
    if any {
        return true;
    }
    let mut log = stash.log_iter();
    let Ok(Some(lines)) = log.all() else {
        return false;
    };
    lines.flatten().any(|line| {
        line.message
            .to_str_lossy()
            .contains(crate::DESKTOP_STASH_MARKER)
    })
}

fn tip(repo: &gix::Repository, branches: &[Branch]) -> Result<Tip> {
    let head = repo.head().map_err(|e| GitError::Gix(e.to_string()))?;
    if head.is_unborn() {
        let name = head
            .referent_name()
            .map(|n| n.shorten().to_string())
            .unwrap_or_else(|| "main".to_string());
        return Ok(Tip::Unborn { name });
    }
    if head.is_detached() {
        let sha = head.id().map(|id| id.to_string()).unwrap_or_default();
        return Ok(Tip::Detached { sha });
    }
    let full = head
        .referent_name()
        .map(|n| n.as_bstr().to_string())
        .unwrap_or_default();
    let branch = branches
        .iter()
        .find(|b| b.full_name == full)
        .cloned()
        .unwrap_or_else(|| Branch {
            name: full.trim_start_matches("refs/heads/").to_string(),
            kind: BranchKind::Local,
            full_name: full.clone(),
            tip: head.id().map(|id| id.to_string()),
            upstream: None,
            tip_time: None,
            tip_author: None,
            remote_name: None,
        });
    Ok(Tip::Valid { branch })
}

/// `getAheadBehind(revSymmetricDifference(base, other))`: how many commits
/// `base` is ahead of / behind `other`. `None` when a ref cannot be resolved.
pub fn symmetric_ahead_behind(
    git: Arc<GitBinary>,
    workdir: &Path,
    base: &str,
    other: &str,
) -> Result<Option<AheadBehind>> {
    let out = GitCommand::new(git)
        .args([
            "rev-list",
            "--left-right",
            "--count",
            "--end-of-options",
            &format!("{base}...{other}"),
            "--",
        ])
        .current_dir(workdir)
        .allow_exit_code(128)
        .run()?;
    if !out.status.success() {
        return Ok(None);
    }
    let text = out.stdout_string()?;
    let mut parts = text.split_whitespace();
    let ahead = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let behind = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    Ok(Some(AheadBehind { ahead, behind }))
}

/// `git rev-list --left-right --count --end-of-options <branch>...<upstream>`
/// → (ahead, behind).
/// CLI for now; gix gains a revwalk-with-hidden helper later.
pub fn ahead_behind(
    git: Arc<GitBinary>,
    workdir: &Path,
    branch: &Branch,
) -> Result<Option<AheadBehind>> {
    let Some(upstream) = &branch.upstream else {
        return Ok(None);
    };
    let out = GitCommand::new(git)
        .args([
            "rev-list",
            "--left-right",
            "--count",
            "--end-of-options",
            &format!("{}...{}", branch.full_name, upstream),
        ])
        .current_dir(workdir)
        .allow_exit_code(128)
        .run()?;
    if !out.status.success() {
        // upstream ref missing (deleted remote branch)
        return Ok(None);
    }
    let text = out.stdout_string()?;
    let mut parts = text.split_whitespace();
    let ahead = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let behind = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    Ok(Some(AheadBehind { ahead, behind }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn opens_fresh_and_committed_repos() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        git(path, &["init", "-q", "-b", "main"]);
        git(path, &["config", "commit.gpgsign", "false"]);
        git(path, &["config", "user.name", "Test"]);
        git(path, &["config", "user.email", "test@example.com"]);

        let info = open_repository(path).unwrap();
        assert_eq!(
            info.tip,
            Tip::Unborn {
                name: "main".into()
            }
        );
        assert!(info.branches.is_empty());
        assert_eq!(info.identity.name.as_deref(), Some("Test"));

        std::fs::write(path.join("a.txt"), "hi").unwrap();
        git(path, &["add", "."]);
        git(path, &["commit", "-q", "-m", "init"]);
        git(
            path,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/wasi-master/corvene.git",
            ],
        );

        let info = open_repository(path).unwrap();
        let branch = info.current_branch().expect("valid tip");
        assert_eq!(branch.name, "main");
        assert!(branch.tip.is_some());
        assert_eq!(info.remotes.len(), 1);
        assert_eq!(info.remotes[0].name, "origin");
        assert!(info.remotes[0].url.contains("github.com"));
    }

    #[test]
    fn has_stash_follows_refs_stash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        git(path, &["init", "-q", "-b", "main"]);
        git(path, &["config", "commit.gpgsign", "false"]);
        std::fs::write(path.join("a.txt"), "1").unwrap();
        git(path, &["add", "."]);
        git(path, &["commit", "-q", "-m", "one"]);
        assert!(!has_stash(path, true));
        std::fs::write(path.join("a.txt"), "2").unwrap();
        git(path, &["stash", "-q"]);
        assert!(has_stash(path, true));
        // a command-line stash is not a Desktop one
        assert!(!has_stash(path, false));
        std::fs::write(path.join("a.txt"), "3").unwrap();
        git(
            path,
            &["stash", "push", "-q", "-m", "!!GitHub_Desktop<main>"],
        );
        assert!(has_stash(path, false));
        git(path, &["stash", "clear"]);
        assert!(!has_stash(path, true));
    }

    #[test]
    fn not_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            open_repository(dir.path()),
            Err(GitError::NotARepository(_))
        ));
    }

    #[test]
    fn ahead_behind_counts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        let remote = dir.path().join("remote.git");
        git(path, &["init", "-q", "--bare", remote.to_str().unwrap()]);
        let work = dir.path().join("work");
        std::fs::create_dir(&work).unwrap();
        git(&work, &["init", "-q", "-b", "main"]);
        git(&work, &["config", "commit.gpgsign", "false"]);
        std::fs::write(work.join("a.txt"), "1").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-q", "-m", "one"]);
        git(
            &work,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git(&work, &["push", "-q", "-u", "origin", "main"]);
        std::fs::write(work.join("b.txt"), "2").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-q", "-m", "two"]);

        let bin = Arc::new(crate::find_git().unwrap());
        let info = open_repository(&work).unwrap();
        let branch = info.current_branch().unwrap();
        assert_eq!(branch.upstream.as_deref(), Some("refs/remotes/origin/main"));
        let ab = ahead_behind(bin, &work, branch).unwrap().unwrap();
        assert_eq!(
            ab,
            AheadBehind {
                ahead: 1,
                behind: 0
            }
        );
    }
}

#[cfg(test)]
mod unsafe_repository_tests {
    use std::process::Command;

    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@e.com",
            ])
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn main_worktree_of_linked_worktree_and_subdirectory() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        std::fs::create_dir_all(main.join("src/deep")).unwrap();
        git(&main, &["init", "-q", "-b", "main"]);
        git(&main, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let linked = dir.path().join("linked");
        git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                linked.to_str().unwrap(),
                "-b",
                "feature",
            ],
        );
        let main = dunce::canonicalize(&main).unwrap();
        assert_eq!(main_worktree_path(&main), Some(main.clone()));
        assert_eq!(main_worktree_path(&linked), Some(main.clone()));
        assert_eq!(
            top_level_working_directory(&main.join("src/deep"))
                .map(|p| dunce::canonicalize(p).unwrap()),
            Some(main)
        );
    }
}
