//! Submodules - GHD `lib/git/submodule.ts` (`updateSubmodulesAfterOperation`,
//! `listSubmodules`, `resetSubmodulePaths`) and `models/submodule.ts`.
//!
//! `update_submodules_after_operation` reports "Updating submodules" and
//! Git LFS transfers inside [`crate::op_progress::with_progress`] (GHD's
//! `progressCallback`). Discard (`commit::discard_changes`) tells submodules
//! apart by their status (`FileStatus::submodule`, the same information)
//! instead of `list_submodules`, which saves a git call per discard.

use std::path::Path;
use std::sync::Arc;

use crate::detect::GitBinary;
use crate::error::Result;
use crate::git_errors::KnownGitError;
use crate::process::GitCommand;
use crate::remote_ops::AskpassEnv;

/// GHD `SubmoduleEntry`: one line of `git submodule status`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmoduleEntry {
    /// The commit checked out in the submodule.
    pub sha: String,
    /// The submodule's path in the repository.
    pub path: String,
    /// `git describe` of that commit (`first-tag~2`, `heads/main`).
    pub describe: String,
}

/// GHD `AuthenticationErrors` (`lib/git/authentication.ts`).
const AUTHENTICATION_ERRORS: [KnownGitError; 4] = [
    KnownGitError::HTTPSAuthenticationFailed,
    KnownGitError::SSHAuthenticationFailed,
    KnownGitError::HTTPSRepositoryNotFound,
    KnownGitError::SSHRepositoryNotFound,
];

/// GHD `updateSubmodulesAfterOperation` (run after a branch checkout):
/// `git [-c protocol.file.allow=always] submodule update --init
/// --recursive`. `allow_file_protocol` lets submodules be cloned from a
/// local path; `askpass` answers credential prompts of submodules being
/// cloned. As in GHD, a submodule that cannot be cloned for want of
/// credentials (`AuthenticationErrors`) is not an error.
pub fn update_submodules_after_operation(
    git: Arc<GitBinary>,
    workdir: &Path,
    allow_file_protocol: bool,
    askpass: Option<&AskpassEnv>,
) -> Result<()> {
    // GHD: `envForRemoteOperation(getFallbackUrlForProxyResolve)`
    let proxy_env = crate::proxy::env_for_fallback(git.clone(), workdir, askpass.is_some());
    let mut cmd = GitCommand::new(git)
        .current_dir(workdir)
        .expected_errors(AUTHENTICATION_ERRORS);
    for (key, value) in proxy_env {
        cmd = cmd.env(key, value);
    }
    if allow_file_protocol {
        cmd = cmd.args(["-c", "protocol.file.allow=always"]);
    }
    cmd = cmd.args(["submodule", "update", "--init", "--recursive"]);
    if let Some(askpass) = askpass {
        cmd = askpass.apply(cmd);
    }
    if !crate::op_progress::active() {
        cmd.run()?;
        return Ok(());
    }
    let title = crate::op_progress::title();
    crate::op_progress::report(&title, "Updating submodules", 0.);
    let mut parser = crate::op_progress::SubmoduleUpdateParser::default();
    crate::lfs_progress::run_with_progress(cmd, &mut parser, &mut |event| {
        crate::op_progress::report(&title, event.text(), event.percent());
    })?;
    crate::op_progress::report(&title, "Submodules updated", 1.);
    Ok(())
}

/// GHD `listSubmodules`: the top-level submodules (`git submodule status
/// --`). Without `.gitmodules`, `.git/modules` or `<common dir>/modules`
/// (a linked worktree) git is not asked and the list is empty; so is it
/// when git cannot read the submodules (exit 128).
pub fn list_submodules(git: Arc<GitBinary>, workdir: &Path) -> Result<Vec<SubmoduleEntry>> {
    if !workdir.join(".gitmodules").exists() && !workdir.join(".git").join("modules").exists() {
        let git_dir = crate::paths::git_dir(workdir);
        let common_dir = std::fs::read_to_string(git_dir.join("commondir"))
            .ok()
            .map(|content| content.trim_end_matches(['\r', '\n']).to_string())
            .filter(|p| !p.is_empty())
            .map(|p| git_dir.join(p));
        if !common_dir.is_some_and(|dir| dir.join("modules").exists()) {
            return Ok(Vec::new());
        }
    }
    let out = GitCommand::new(git)
        .args(["submodule", "status", "--"])
        .current_dir(workdir)
        .allow_exit_code(128)
        .run()?;
    if !out.status.success() {
        return Ok(Vec::new());
    }
    Ok(parse_submodule_status(&out.stdout_string()?))
}

/// Parse `git submodule status` lines: a status character (` `, `-`, `+`,
/// `U`), the sha, the path and `(describe)` (GHD
/// `/^.([^ ]+) (.+) \((.+?)\)$/gm`). Lines without a describe are skipped.
pub fn parse_submodule_status(text: &str) -> Vec<SubmoduleEntry> {
    text.lines()
        .filter_map(|line| {
            let mut chars = line.chars();
            chars.next()?;
            let rest = chars.as_str();
            let (sha, rest) = rest.split_once(' ')?;
            if sha.is_empty() {
                return None;
            }
            let rest = rest.strip_suffix(')')?;
            // the describe is the shortest `(…)` at the end
            let open = rest.rfind(" (")?;
            let (path, describe) = (&rest[..open], &rest[open + 2..]);
            if path.is_empty() || describe.is_empty() {
                return None;
            }
            Some(SubmoduleEntry {
                sha: sha.to_string(),
                path: path.to_string(),
                describe: describe.to_string(),
            })
        })
        .collect()
}

/// GHD `resetSubmodulePaths`: `git submodule update --recursive --force --
/// <paths>`, back to the commit the index records with tracked changes
/// inside reverted (untracked files stay). Nothing for no paths.
pub fn reset_submodule_paths(git: Arc<GitBinary>, workdir: &Path, paths: &[&str]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    GitCommand::new(git)
        .args(["submodule", "update", "--recursive", "--force", "--"])
        .args(paths)
        .current_dir(workdir)
        .run()?;
    Ok(())
}

/// Corvene `1111-submodules`: where a submodule stands, as the Submodules
/// panel shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubmoduleState {
    /// `git submodule init` has not run: no URL in `.git/config`, no
    /// checkout.
    NotInitialized,
    /// Initialized, but nothing is checked out yet (`git submodule update`
    /// clones it).
    NotCloned,
    /// The checkout is at the commit the repository records.
    UpToDate,
    /// The checkout is at another commit (`+` in `git submodule status`).
    DifferentCommit,
    /// The submodule has a merge conflict (`U`).
    Conflicted,
}

/// Corvene `1111-submodules`: one submodule of the Submodules panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmoduleDetails {
    /// The name in `.gitmodules` (the path when it has no entry).
    pub name: String,
    pub path: String,
    /// The URL `.gitmodules` gives.
    pub url: Option<String>,
    /// [`Self::url`] resolved when relative ([`resolve_submodule_url`]).
    pub resolved_url: Option<String>,
    /// The URL `.git/config` has for it (written by `init` and `sync`).
    pub configured_url: Option<String>,
    /// The commit the index records.
    pub recorded: Option<String>,
    /// The commit checked out in the submodule.
    pub checked_out: Option<String>,
    /// `git describe` of the checked-out commit.
    pub describe: Option<String>,
    pub state: SubmoduleState,
}

impl SubmoduleDetails {
    /// The configured URL differs from `.gitmodules`' (resolved, like git,
    /// against `base` when relative): `git submodule sync` would change it.
    pub fn needs_sync(&self) -> bool {
        matches!((&self.resolved_url, &self.configured_url), (Some(a), Some(b)) if a != b)
    }
}

/// git's `resolve_relative_url`: a `./` or `../` submodule URL joined to
/// `base` (the superproject's `origin` URL, or its folder), one path part
/// dropped per `../`. Other URLs are returned as they are.
pub fn resolve_submodule_url(url: &str, base: &str) -> String {
    if !(url.starts_with("./") || url.starts_with("../")) {
        return url.to_string();
    }
    let mut base = base.trim_end_matches('/').to_string();
    let mut rest = url;
    loop {
        if let Some(r) = rest.strip_prefix("./") {
            rest = r;
        } else if let Some(r) = rest.strip_prefix("../") {
            rest = r;
            // `host:path` keeps the colon when its last part goes
            match base.rfind(['/', ':']) {
                Some(i) if base.as_bytes()[i] == b':' => base.truncate(i + 1),
                Some(i) => base.truncate(i),
                None => base = ".".to_string(),
            }
        } else {
            break;
        }
    }
    if base.ends_with(':') {
        format!("{base}{rest}")
    } else {
        format!("{base}/{rest}")
    }
}

/// `git config -z [-f <file>] --get-regexp <pattern>`: (key, value) pairs.
fn config_entries(
    git: Arc<GitBinary>,
    workdir: &Path,
    file: Option<&str>,
    pattern: &str,
) -> Vec<(String, String)> {
    let mut cmd = GitCommand::new(git)
        .current_dir(workdir)
        .arg("config")
        .arg("-z");
    if let Some(file) = file {
        cmd = cmd.args(["-f", file]);
    }
    let Ok(out) = cmd
        .args(["--get-regexp", pattern])
        .allow_any_exit_code()
        .run()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter_map(|entry| {
            let (key, value) = entry.split_once('\n')?;
            Some((key.to_string(), value.to_string()))
        })
        .collect()
}

/// `submodule.<name>.<key>` → (name, key); names may contain dots.
fn split_submodule_key(key: &str) -> Option<(&str, &str)> {
    let rest = key.strip_prefix("submodule.")?;
    let dot = rest.rfind('.')?;
    Some((&rest[..dot], &rest[dot + 1..]))
}

/// Corvene `1111-submodules`: every submodule the index has a gitlink for,
/// with its `.gitmodules` name and URL, the URL `.git/config` has, the
/// commit recorded and the one checked out ([`list_submodules`] for the
/// initialized ones, `rev-parse HEAD` in the checkout otherwise).
pub fn submodule_details(git: Arc<GitBinary>, workdir: &Path) -> Result<Vec<SubmoduleDetails>> {
    let out = GitCommand::new(git.clone())
        .args(["ls-files", "--stage", "-z"])
        .current_dir(workdir)
        .run()?;
    // path → (recorded sha, conflicted), in index order
    let mut gitlinks: Vec<(String, Option<String>, bool)> = Vec::new();
    for entry in out.stdout.split(|b| *b == 0) {
        let Ok(entry) = std::str::from_utf8(entry) else {
            continue;
        };
        let Some((meta, path)) = entry.split_once('\t') else {
            continue;
        };
        let mut parts = meta.split(' ');
        let (Some(mode), Some(sha), Some(stage)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if mode != "160000" {
            continue;
        }
        let conflicted = stage != "0";
        match gitlinks.iter_mut().find(|(p, ..)| p == path) {
            Some(existing) => existing.2 |= conflicted,
            None => gitlinks.push((
                path.to_string(),
                (!conflicted).then(|| sha.to_string()),
                conflicted,
            )),
        }
    }
    if gitlinks.is_empty() {
        return Ok(Vec::new());
    }
    // `.gitmodules`: name → (path, url)
    let mut modules: Vec<(String, Option<String>, Option<String>)> = Vec::new();
    for (key, value) in config_entries(
        git.clone(),
        workdir,
        Some(".gitmodules"),
        r"^submodule\..*\.(path|url)$",
    ) {
        let Some((name, field)) = split_submodule_key(&key) else {
            continue;
        };
        let ix = match modules.iter().position(|(n, ..)| n == name) {
            Some(ix) => ix,
            None => {
                modules.push((name.to_string(), None, None));
                modules.len() - 1
            }
        };
        match field {
            "path" => modules[ix].1 = Some(value),
            _ => modules[ix].2 = Some(value),
        }
    }
    let configured: Vec<(String, String)> =
        config_entries(git.clone(), workdir, None, r"^submodule\..*\.url$")
            .into_iter()
            .filter_map(|(key, value)| {
                let (name, _) = split_submodule_key(&key)?;
                Some((name.to_string(), value))
            })
            .collect();
    let status = list_submodules(git.clone(), workdir).unwrap_or_default();
    let base = crate::remote_ops::config_value(git.clone(), workdir, "remote.origin.url")
        .unwrap_or_else(|| {
            let dir = dunce::canonicalize(workdir)
                .unwrap_or_else(|_| workdir.to_path_buf())
                .to_string_lossy()
                .into_owned();
            // git's own working directory (`xgetcwd`) has forward slashes
            // on Windows too
            if cfg!(windows) {
                dir.replace('\\', "/")
            } else {
                dir
            }
        });
    Ok(gitlinks
        .into_iter()
        .map(|(path, recorded, conflicted)| {
            let module = modules
                .iter()
                .find(|(_, p, _)| p.as_deref() == Some(path.as_str()));
            let name = module.map_or_else(|| path.clone(), |(n, ..)| n.clone());
            let url = module.and_then(|(.., url)| url.clone());
            let configured_url = configured
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, url)| url.clone());
            let populated = workdir.join(&path).join(".git").exists();
            let listed = status.iter().find(|e| e.path == path);
            let checked_out = listed.map(|e| e.sha.clone()).or_else(|| {
                populated
                    .then(|| crate::commit::head_sha(git.clone(), &workdir.join(&path)).ok())
                    .flatten()
            });
            let state = if conflicted {
                SubmoduleState::Conflicted
            } else if !populated || checked_out.is_none() {
                if configured_url.is_some() {
                    SubmoduleState::NotCloned
                } else {
                    SubmoduleState::NotInitialized
                }
            } else if checked_out == recorded {
                SubmoduleState::UpToDate
            } else {
                SubmoduleState::DifferentCommit
            };
            SubmoduleDetails {
                name,
                path,
                resolved_url: url.as_deref().map(|u| resolve_submodule_url(u, &base)),
                url,
                configured_url,
                recorded,
                checked_out: checked_out.filter(|_| populated),
                describe: listed.map(|e| e.describe.clone()),
                state,
            }
        })
        .collect())
}

/// Corvene `1111-submodules`: `git submodule init -- <paths>` (every
/// submodule for none): copies the URLs from `.gitmodules` to
/// `.git/config`.
pub fn submodule_init(git: Arc<GitBinary>, workdir: &Path, paths: &[String]) -> Result<()> {
    GitCommand::new(git)
        .args(["submodule", "init", "--"])
        .args(paths)
        .current_dir(workdir)
        .run()?;
    Ok(())
}

/// Corvene `1111-submodules`: `git submodule update --init [--recursive]
/// -- <paths>` (every submodule for none): clones what is missing and
/// checks out the recorded commits. `askpass` answers the clones'
/// credential prompts.
pub fn submodule_update(
    git: Arc<GitBinary>,
    workdir: &Path,
    paths: &[String],
    recursive: bool,
    askpass: Option<&AskpassEnv>,
) -> Result<()> {
    let mut cmd = GitCommand::new(git)
        .current_dir(workdir)
        .args(["submodule", "update", "--init"]);
    if recursive {
        cmd = cmd.arg("--recursive");
    }
    cmd = cmd.arg("--").args(paths);
    if let Some(askpass) = askpass {
        cmd = askpass.apply(cmd);
    }
    cmd.run()?;
    Ok(())
}

/// Corvene `1111-submodules`: `git submodule sync [--recursive] --
/// <paths>` (every submodule for none): `.gitmodules`' URLs into
/// `.git/config` and the submodules' `origin`.
pub fn submodule_sync(
    git: Arc<GitBinary>,
    workdir: &Path,
    paths: &[String],
    recursive: bool,
) -> Result<()> {
    let mut cmd = GitCommand::new(git)
        .current_dir(workdir)
        .args(["submodule", "sync"]);
    if recursive {
        cmd = cmd.arg("--recursive");
    }
    cmd.arg("--").args(paths).run()?;
    Ok(())
}

/// Corvene `785-embedded-repo-commit`: an untracked folder that is a git
/// repository of its own, and its `origin` URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddedRepository {
    /// The folder, repository-relative, without the trailing slash status
    /// lists it with.
    pub path: String,
    pub url: Option<String>,
}

/// Corvene `785-embedded-repo-commit`: which of the untracked `paths`
/// (status lists a nested repository as one entry, `Sub/`) are git
/// repositories, with their `origin` URL.
pub fn embedded_repositories<S: AsRef<str>>(
    git: Arc<GitBinary>,
    workdir: &Path,
    paths: &[S],
) -> Vec<EmbeddedRepository> {
    paths
        .iter()
        .map(AsRef::as_ref)
        .filter(|p| p.ends_with('/'))
        .map(|p| p.trim_end_matches('/'))
        .filter(|p| !p.is_empty() && workdir.join(p).join(".git").exists())
        .map(|path| EmbeddedRepository {
            path: path.to_string(),
            url: crate::remote_ops::config_value(
                git.clone(),
                &workdir.join(path),
                "remote.origin.url",
            ),
        })
        .collect()
}

/// Corvene `785-embedded-repo-commit`: stage nested repositories: one with
/// an `origin` as a submodule (`git submodule add <url> <path>`, which adds
/// the existing checkout and its `.gitmodules` entry), one without as a bare
/// pointer to its current commit (`update-index --add`, a gitlink with no
/// `.gitmodules` entry, as `git add` does).
pub fn add_embedded_repositories(
    git: Arc<GitBinary>,
    workdir: &Path,
    repositories: &[EmbeddedRepository],
) -> Result<()> {
    for repository in repositories {
        match &repository.url {
            Some(url) => {
                GitCommand::new(git.clone())
                    .args(["submodule", "add", "--"])
                    .arg(url)
                    .arg(&repository.path)
                    .current_dir(workdir)
                    .run()?;
            }
            None => {
                GitCommand::new(git.clone())
                    .args(["update-index", "--add", "--"])
                    .arg(&repository.path)
                    .current_dir(workdir)
                    .run()?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_repositories_become_submodules_or_pointers() {
        use std::process::Command;
        let dir = tempfile::tempdir().unwrap();
        let run = |cwd: &Path, args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(cwd)
                    .status()
                    .unwrap()
                    .success()
            )
        };
        let init = |cwd: &Path| {
            run(cwd, &["init", "-q", "-b", "main"]);
            run(cwd, &["config", "commit.gpgsign", "false"]);
            run(cwd, &["config", "user.name", "T"]);
            run(cwd, &["config", "user.email", "t@example.com"]);
            std::fs::write(cwd.join("f"), "f\n").unwrap();
            run(cwd, &["add", "f"]);
            run(cwd, &["commit", "-q", "-m", "f"]);
        };
        let root = dir.path();
        init(root);
        for name in ["Sub", "Bare"] {
            std::fs::create_dir(root.join(name)).unwrap();
            init(&root.join(name));
        }
        run(
            &root.join("Sub"),
            &["remote", "add", "origin", "https://example.com/sub.git"],
        );
        std::fs::create_dir(root.join("plain")).unwrap();
        let git = Arc::new(crate::find_git().unwrap());
        let found = embedded_repositories(git.clone(), root, &["Sub/", "Bare/", "plain/", "f"]);
        assert_eq!(
            found,
            vec![
                EmbeddedRepository {
                    path: "Sub".into(),
                    url: Some("https://example.com/sub.git".into())
                },
                EmbeddedRepository {
                    path: "Bare".into(),
                    url: None
                },
            ]
        );
        add_embedded_repositories(git, root, &found).unwrap();
        let out = Command::new("git")
            .args(["ls-files", "-s"])
            .current_dir(root)
            .output()
            .unwrap();
        let index = String::from_utf8_lossy(&out.stdout);
        assert!(
            index
                .lines()
                .any(|l| l.starts_with("160000") && l.ends_with("\tSub"))
        );
        assert!(
            index
                .lines()
                .any(|l| l.starts_with("160000") && l.ends_with("\tBare"))
        );
        assert!(index.lines().any(|l| l.ends_with("\t.gitmodules")));
        let modules = std::fs::read_to_string(root.join(".gitmodules")).unwrap();
        assert!(modules.contains("path = Sub") && !modules.contains("Bare"));
    }

    #[test]
    fn submodule_details_follow_init_update_and_checkouts() {
        use std::process::Command;
        let dir = tempfile::tempdir().unwrap();
        let run = |cwd: &Path, args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(cwd)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        let init = |cwd: &Path| {
            std::fs::create_dir_all(cwd).unwrap();
            run(cwd, &["init", "-q", "-b", "main"]);
            run(cwd, &["config", "commit.gpgsign", "false"]);
            run(cwd, &["config", "user.name", "T"]);
            run(cwd, &["config", "user.email", "t@example.com"]);
            run(cwd, &["config", "protocol.file.allow", "always"]);
            std::fs::write(cwd.join("f"), "f\n").unwrap();
            run(cwd, &["add", "f"]);
            run(cwd, &["commit", "-q", "-m", "f"]);
        };
        let lib = dir.path().join("lib");
        init(&lib);
        let root = dir.path().join("root");
        init(&root);
        let url = lib.to_string_lossy().to_string();
        run(
            &root,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                &url,
                "vendor/lib",
            ],
        );
        run(&root, &["commit", "-q", "-m", "Add lib"]);
        let git = Arc::new(crate::find_git().unwrap());
        let details = submodule_details(git.clone(), &root).unwrap();
        assert_eq!(details.len(), 1);
        let d = &details[0];
        assert_eq!(d.name, "vendor/lib");
        assert_eq!(d.path, "vendor/lib");
        assert_eq!(d.url.as_deref(), Some(url.as_str()));
        assert_eq!(d.state, SubmoduleState::UpToDate);
        assert_eq!(d.recorded, d.checked_out);
        assert!(!d.needs_sync());

        // a new commit checked out in the submodule
        std::fs::write(lib.join("g"), "g\n").unwrap();
        run(&lib, &["add", "g"]);
        run(&lib, &["commit", "-q", "-m", "g"]);
        run(&root.join("vendor/lib"), &["pull", "-q", "origin", "main"]);
        let d = submodule_details(git.clone(), &root).unwrap().remove(0);
        assert_eq!(d.state, SubmoduleState::DifferentCommit);
        assert_ne!(d.recorded, d.checked_out);

        // deinit: no URL in .git/config, no checkout
        run(&root, &["submodule", "deinit", "-q", "-f", "vendor/lib"]);
        let d = submodule_details(git.clone(), &root).unwrap().remove(0);
        assert_eq!(d.state, SubmoduleState::NotInitialized);
        assert_eq!(d.checked_out, None);
        submodule_init(git.clone(), &root, &["vendor/lib".to_string()]).unwrap();
        let d = submodule_details(git.clone(), &root).unwrap().remove(0);
        assert_eq!(d.state, SubmoduleState::NotCloned);
        assert_eq!(d.configured_url.as_deref(), Some(url.as_str()));

        // a changed URL in .gitmodules needs a sync
        run(
            &root,
            &[
                "config",
                "-f",
                ".gitmodules",
                "submodule.vendor/lib.url",
                "../elsewhere",
            ],
        );
        assert!(submodule_details(git.clone(), &root).unwrap()[0].needs_sync());
        submodule_sync(git.clone(), &root, &[], true).unwrap();
        assert!(!submodule_details(git.clone(), &root).unwrap()[0].needs_sync());
        run(
            &root,
            &[
                "config",
                "-f",
                ".gitmodules",
                "submodule.vendor/lib.url",
                &url,
            ],
        );
        submodule_sync(git.clone(), &root, &[], false).unwrap();

        crate::process::with_env(
            &[
                ("GIT_CONFIG_COUNT", "1"),
                ("GIT_CONFIG_KEY_0", "protocol.file.allow"),
                ("GIT_CONFIG_VALUE_0", "always"),
            ],
            || submodule_update(git.clone(), &root, &[], true, None).unwrap(),
        );
        let d = submodule_details(git, &root).unwrap().remove(0);
        assert_eq!(d.state, SubmoduleState::UpToDate);
        assert!(d.describe.is_some());
    }

    #[test]
    fn resolves_relative_urls_like_git() {
        assert_eq!(
            resolve_submodule_url("../lib.git", "https://github.com/o/app.git"),
            "https://github.com/o/lib.git"
        );
        assert_eq!(
            resolve_submodule_url("./sub", "https://github.com/o/app/"),
            "https://github.com/o/app/sub"
        );
        assert_eq!(
            resolve_submodule_url("../lib.git", "git@github.com:app.git"),
            "git@github.com:lib.git"
        );
        assert_eq!(resolve_submodule_url("../../x", "/a/b/c"), "/a/x");
        assert_eq!(
            resolve_submodule_url("https://e.com/x", "/a"),
            "https://e.com/x"
        );
    }

    #[test]
    fn parses_submodule_status() {
        let text = " 1eaabe34fc6f486367a176207420378f587d3b48 git (v2.16.0-rc0)\n\
                    +c59617b65080863c4ca72c1f191fa1b423b92223 foo/sub module (heads/main)\n\
                    -0123456789012345678901234567890123456789 uninit\n";
        let entries = parse_submodule_status(text);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, "git");
        assert_eq!(entries[0].describe, "v2.16.0-rc0");
        assert_eq!(entries[1].sha, "c59617b65080863c4ca72c1f191fa1b423b92223");
        assert_eq!(entries[1].path, "foo/sub module");
        assert_eq!(entries[1].describe, "heads/main");
    }
}
