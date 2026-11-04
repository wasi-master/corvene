//! Branch, merge and stash operations - GHD `lib/git/{branch,checkout,
//! reflog,merge,stash}.ts` and `lib/find-default-branch.ts`.
//!
//! Stash entries are dropped and popped by their commit sha, as GHD's
//! `dropDesktopStashEntry` / `popStashEntry` do, so an entry whose
//! `stash@{n}` shifted since the last refresh is still the one acted on.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use corvene_models::{Branch, BranchKind, StashEntry};

use crate::detect::GitBinary;
use crate::error::{GitError, Result};
use crate::git_errors::KnownGitError;
use crate::process::GitCommand;
use crate::remote_ops::{AskpassEnv, GitProgressEvent};

/// Corvene `787-commit-to-new-branch` (History › Create Branch from
/// Commits…, "Remove them from <branch>"): move `branch`, which is not
/// checked out in any worktree, back to `target`, after `commits` (the
/// branch's newest ones) were put on a new branch. Refuses, touching
/// nothing, when the branch is checked out in a worktree, when it no longer
/// points at `expected_tip`, or when a remote-tracking branch has one of
/// `commits` (it was pushed: rewriting would diverge from the remote). The
/// move is `update-ref` with `expected_tip` as the old value, so a branch
/// that moved meanwhile is left alone.
pub fn move_branch_back(
    git: Arc<GitBinary>,
    workdir: &Path,
    branch: &str,
    target: &str,
    expected_tip: &str,
    commits: &[String],
    reason: &str,
) -> Result<()> {
    let full = format!("refs/heads/{branch}");
    let worktrees = GitCommand::new(git.clone())
        .args(["worktree", "list", "--porcelain"])
        .current_dir(workdir)
        .run()?
        .stdout_string()?;
    if worktrees
        .lines()
        .any(|line| line.strip_prefix("branch ") == Some(full.as_str()))
    {
        return Err(GitError::Gix(format!(
            "{branch} is checked out in a worktree, so its commits were not removed from it."
        )));
    }
    // the commits only this repository has: none may be on a remote
    let local_only = GitCommand::new(git.clone())
        .args(["rev-list", expected_tip, "--not", "--remotes"])
        .current_dir(workdir)
        .run()?
        .stdout_string()?;
    let local_only: std::collections::HashSet<&str> = local_only.lines().collect();
    if commits.iter().any(|sha| !local_only.contains(sha.as_str())) {
        return Err(GitError::Gix(format!(
            "Some of the commits have been pushed, so they were not removed from {branch}."
        )));
    }
    GitCommand::new(git)
        .args(["update-ref", "-m", reason, &full, target, expected_tip])
        .current_dir(workdir)
        .run()?;
    Ok(())
}

/// `createBranch`: `git branch [--no-track] -- <name> [<start point>]`.
pub fn create_branch(
    git: Arc<GitBinary>,
    workdir: &Path,
    name: &str,
    start_point: Option<&str>,
    no_track: bool,
) -> Result<()> {
    let mut cmd = GitCommand::new(git).args(["branch"]).current_dir(workdir);
    if no_track {
        cmd = cmd.arg("--no-track");
    }
    // GHD 3.6.7 (desktop/desktop#22862)
    cmd = cmd.args(["--", name]);
    if let Some(start) = start_point {
        cmd = cmd.arg(start);
    }
    cmd.run()?;
    Ok(())
}

/// Corvene (`1223-checkout-from-fork`): `git branch --track -- <name>
/// refs/remotes/<remote>/<branch>`, a branch tracking that remote branch
/// whatever `branch.autoSetupMerge` says. The full ref keeps a local
/// `<remote>/<branch>` branch from shadowing the start point.
pub fn create_tracking_branch(
    git: Arc<GitBinary>,
    workdir: &Path,
    name: &str,
    remote: &str,
    branch: &str,
) -> Result<()> {
    GitCommand::new(git)
        .args([
            "branch".to_string(),
            "--track".to_string(),
            "--".to_string(),
            name.to_string(),
            format!("refs/remotes/{remote}/{branch}"),
        ])
        .current_dir(workdir)
        .run()?;
    Ok(())
}

/// Text git prints when a checkout would clobber local changes
/// (`isLocalChangesOverwrittenError`).
pub fn is_local_changes_overwritten(err: &GitError) -> bool {
    let text = err.to_string();
    text.contains("would be overwritten by checkout")
        || text.contains("Please commit your changes or stash them before you switch branches")
}

/// Which submodules [`checkout_branch_with`] updates after the checkout.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SubmoduleUpdate {
    /// GHD 3.6.6 (`updateSubmodulesAfterOperation`): `git submodule update
    /// --init --recursive`, every submodule.
    #[default]
    All,
    /// Corvene (flag `263-submodules-follow-checkout`): every gitlink in the
    /// index but these paths (`corvene_git::update_submodules`).
    AllExcept(Vec<String>),
    /// Leave the submodules alone; the caller updates them itself.
    None,
}

/// The optional parameters of GHD `checkoutBranch`.
#[derive(Default)]
pub struct CheckoutOptions {
    /// GHD `allowFileProtocol`: `-c protocol.file.allow=always` for the
    /// submodule update (submodules cloned from a local path).
    pub allow_file_protocol: bool,
    pub submodules: SubmoduleUpdate,
    /// Credentials for submodules the update clones (GHD
    /// `envForRemoteOperation`).
    pub askpass: Option<AskpassEnv>,
}

/// `checkoutBranch(repository, branch, null)`: [`checkout_branch_with`]
/// GHD's defaults (every submodule updated, no file protocol, no askpass).
pub fn checkout_branch(git: Arc<GitBinary>, workdir: &Path, branch: &Branch) -> Result<()> {
    checkout_branch_with(git, workdir, branch, &CheckoutOptions::default())
}

/// `checkoutBranch`: local branches by name (`checkout --end-of-options
/// <name> --`); remote branches become a tracking local branch (`checkout
/// -b <short> refs/remotes/<remote>/<short> --`, GHD 3.6.7). Then the
/// submodules follow the checkout as `options.submodules` says (GHD 3.6.6
/// `updateSubmodulesAfterOperation`); a failure there is returned although
/// the branch is checked out, as in GHD. Inside
/// [`crate::op_progress::with_progress`] it runs with `--progress` and
/// reports "Checking out files" and Git LFS transfers as the first 90 %,
/// the submodule update the rest (GHD `CheckoutStepWeight`).
pub fn checkout_branch_with(
    git: Arc<GitBinary>,
    workdir: &Path,
    branch: &Branch,
    options: &CheckoutOptions,
) -> Result<()> {
    // GHD `checkoutBranch`: `envForRemoteOperation(getFallbackUrlForProxyResolve)`
    let progress = crate::op_progress::active();
    let mut cmd = GitCommand::new(git.clone())
        .args(["checkout"])
        .current_dir(workdir);
    if progress {
        cmd = cmd.arg("--progress");
    }
    for (key, value) in crate::proxy::env_for_fallback(git.clone(), workdir, false) {
        cmd = cmd.env(key, value);
    }
    // GHD 3.6.7 (desktop/desktop#22862): a remote branch starts from its
    // full ref and a local one comes after `--end-of-options`, so a name
    // starting with `-` is not read as an option; the `--` ends the
    // revisions (paths would follow)
    cmd = match branch.kind {
        BranchKind::Local => cmd.args(["--end-of-options", &branch.name, "--"]),
        BranchKind::Remote => cmd
            .arg("-b")
            .arg(branch.name_without_remote())
            .arg(if branch.full_name.is_empty() {
                &branch.name
            } else {
                &branch.full_name
            })
            .arg("--"),
    };
    // Corvene (`1114-hook-results`): GHD never intercepts a checkout's
    // hooks; post-checkout's outcome is shown when the caller asks for it
    if crate::hooks::scoped_reports_results() {
        cmd = cmd.intercept_hooks(&["post-checkout"]);
    }
    if progress {
        let title = format!("Checking out branch {}", branch.name);
        crate::op_progress::clamped(0., crate::op_progress::CHECKOUT_STEP_WEIGHT, || {
            let initial = if cfg!(target_os = "macos") {
                "Switching to Branch"
            } else {
                "Switching to branch"
            };
            crate::op_progress::report(&title, initial, 0.);
            let mut parser = crate::op_progress::checkout_parser();
            // GHD forwards only the parser's steps and Git LFS
            crate::lfs_progress::run_with_progress(cmd, &mut parser, &mut |event| {
                if let GitProgressEvent::Progress { percent, details } = event {
                    crate::op_progress::report(&title, &details.text, percent);
                }
            })
        })?;
        return crate::op_progress::submodule_step(|| {
            update_submodules_after_checkout(git, workdir, options)
        });
    }
    cmd.run()?;
    update_submodules_after_checkout(git, workdir, options)
}

/// The submodule half of [`checkout_branch_with`], for a caller that checked
/// out with [`SubmoduleUpdate::None`] and runs it later: the submodules
/// follow the checkout as `options.submodules` says.
pub fn update_submodules_after_checkout(
    git: Arc<GitBinary>,
    workdir: &Path,
    options: &CheckoutOptions,
) -> Result<()> {
    match &options.submodules {
        SubmoduleUpdate::All => crate::submodule::update_submodules_after_operation(
            git,
            workdir,
            options.allow_file_protocol,
            options.askpass.as_ref(),
        ),
        SubmoduleUpdate::AllExcept(skip) => {
            crate::remote_ops::update_submodules(git, workdir, skip, options.askpass.as_ref())
        }
        SubmoduleUpdate::None => Ok(()),
    }
}

/// `git checkout -b <name>`: creates the branch from HEAD, or renames an
/// unborn branch (GHD's create-branch behaviour on unborn repositories).
pub fn checkout_new_branch(git: Arc<GitBinary>, workdir: &Path, name: &str) -> Result<()> {
    GitCommand::new(git)
        .args(["checkout", "-b", name])
        .current_dir(workdir)
        .run()?;
    Ok(())
}

/// `renameBranch`: `git branch -m -- <old> <new>`.
pub fn rename_branch(git: Arc<GitBinary>, workdir: &Path, old: &str, new: &str) -> Result<()> {
    GitCommand::new(git)
        .args(["branch", "-m", "--", old, new])
        .current_dir(workdir)
        .run()?;
    Ok(())
}

/// `git branch --set-upstream-to=<upstream> <name>` (Corvene,
/// `1103-implicit-upstream-push-default`: a pull records the upstream a
/// branch had only implicitly, as a push with `--set-upstream` does).
pub fn set_upstream(git: Arc<GitBinary>, workdir: &Path, name: &str, upstream: &str) -> Result<()> {
    GitCommand::new(git)
        .args([
            "branch".to_string(),
            format!("--set-upstream-to={upstream}"),
            "--".to_string(),
            name.to_string(),
        ])
        .current_dir(workdir)
        .run()?;
    Ok(())
}

/// `git branch --unset-upstream <name>` (Corvene, `1222-push-target-guard`:
/// a branch that tracked a differently named branch is published under its
/// own name).
pub fn unset_upstream(git: Arc<GitBinary>, workdir: &Path, name: &str) -> Result<()> {
    GitCommand::new(git)
        .args(["branch", "--unset-upstream", "--", name])
        .current_dir(workdir)
        .run()?;
    Ok(())
}

/// `deleteLocalBranch`: `git branch -D -- <name>`.
pub fn delete_local_branch(git: Arc<GitBinary>, workdir: &Path, name: &str) -> Result<()> {
    GitCommand::new(git)
        .args(["branch", "-D", "--", name])
        .current_dir(workdir)
        .run()?;
    Ok(())
}

/// `deleteRemoteBranch`: `git push -- <remote> :<name>`. When the remote no
/// longer has the branch (`BranchDeletionFailed`) the push failing is
/// expected: the remote-tracking ref `refs/remotes/<remote>/<name>` is
/// deleted instead, as the push would have done.
pub fn delete_remote_branch(
    git: Arc<GitBinary>,
    workdir: &Path,
    remote: &str,
    name: &str,
) -> Result<()> {
    delete_remote_branch_with(git, workdir, remote, name, None)
}

/// [`delete_remote_branch`] with the signed-in accounts' credentials
/// (`askpass`): GHD runs it with `envForRemoteOperation(remote.url)`, the
/// credentials and the system proxy.
pub fn delete_remote_branch_with(
    git: Arc<GitBinary>,
    workdir: &Path,
    remote: &str,
    name: &str,
    askpass: Option<&AskpassEnv>,
) -> Result<()> {
    let out = crate::remote_ops::remote_operation(git.clone(), workdir, remote, askpass)
        .args(["push", "--", remote])
        .arg(format!(":{name}"))
        .expected_errors([KnownGitError::BranchDeletionFailed])
        .run()?;
    // a failed push only gets here as the expected error
    if !out.status.success() {
        crate::refs::delete_ref(git, workdir, &format!("refs/remotes/{remote}/{name}"), None)?;
    }
    Ok(())
}

/// `getBranchesPointedAt`: the local branches whose tip is `commitish`
/// (`git branch --points-at`), `None` when git cannot resolve it (exit 1)
/// or the ref is malformed (exit 129).
pub fn get_branches_pointed_at(
    git: Arc<GitBinary>,
    workdir: &Path,
    commitish: &str,
) -> Result<Option<Vec<String>>> {
    let out = GitCommand::new(git)
        .arg("branch")
        .arg(format!("--points-at={commitish}"))
        .arg("--format=%(refname:short)")
        .current_dir(workdir)
        .allow_exit_code(1)
        .allow_exit_code(129)
        .run()?;
    if !out.status.success() {
        return Ok(None);
    }
    Ok(Some(
        out.stdout_string()?
            .split('\n')
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect(),
    ))
}

/// `getMergedBranches`: the local branches merged into `branch_name`
/// (`git branch --merged`), canonical ref to tip sha, without `branch_name`
/// itself.
pub fn get_merged_branches(
    git: Arc<GitBinary>,
    workdir: &Path,
    branch_name: &str,
) -> Result<HashMap<String, String>> {
    let canonical = crate::refs::format_as_local_ref(branch_name);
    let out = GitCommand::new(git)
        .args([
            "branch",
            "--format=%(objectname)%00%(refname)",
            // one argument, so a name starting with `-` stays the value
            &format!("--merged={branch_name}"),
        ])
        .current_dir(workdir)
        .run()?;
    Ok(out
        .stdout_string()?
        .lines()
        .filter_map(|line| line.split_once('\0'))
        .filter(|(_, reference)| *reference != canonical)
        .map(|(sha, reference)| (reference.to_string(), sha.to_string()))
        .collect())
}

/// A local branch's relation to its upstream (`%(upstream:track)`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BranchTracking {
    pub ahead: usize,
    pub behind: usize,
    /// The upstream is configured but its remote-tracking branch is gone
    /// (deleted on the remote and pruned).
    pub gone: bool,
}

/// `git for-each-ref refs/heads`: the tracking state of every local branch
/// that has an upstream, by short name.
pub fn branch_tracking(
    git: Arc<GitBinary>,
    workdir: &Path,
) -> Result<std::collections::HashMap<String, BranchTracking>> {
    let out = GitCommand::new(git)
        .args([
            "for-each-ref",
            "--format=%(refname:lstrip=2)%00%(upstream)%00%(upstream:track,nobracket)",
            "refs/heads",
        ])
        .current_dir(workdir)
        .run()?;
    Ok(parse_branch_tracking(&out.stdout_string()?))
}

/// Parse [`branch_tracking`]'s `name NUL upstream NUL track` lines.
pub fn parse_branch_tracking(text: &str) -> std::collections::HashMap<String, BranchTracking> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split('\0');
            let (name, upstream, track) = (parts.next()?, parts.next()?, parts.next()?);
            if upstream.is_empty() {
                return None;
            }
            let mut tracking = BranchTracking::default();
            for part in track.split(", ") {
                match part.split_once(' ') {
                    Some(("ahead", n)) => tracking.ahead = n.parse().unwrap_or(0),
                    Some(("behind", n)) => tracking.behind = n.parse().unwrap_or(0),
                    _ => tracking.gone |= part == "gone",
                }
            }
            Some((name.to_string(), tracking))
        })
        .collect()
}

/// `getRecentBranches`: branch names from the last checkouts in HEAD's reflog.
pub fn recent_branches(git: Arc<GitBinary>, workdir: &Path, limit: usize) -> Result<Vec<String>> {
    let out = GitCommand::new(git)
        .args([
            "log",
            "-g",
            "--no-abbrev-commit",
            "--pretty=oneline",
            "HEAD",
            "-n",
            "2500",
            "--",
        ])
        .current_dir(workdir)
        .allow_exit_code(128)
        .run()?;
    if !out.status.success() {
        // unborn branch: no reflog yet
        return Ok(Vec::new());
    }
    Ok(parse_recent_branches(&out.stdout_string()?, limit))
}

/// Parse `log -g --pretty=oneline` lines: `<sha> checkout: moving from A to B`
/// (and `renamed refs/heads/A to refs/heads/B`, whose source is excluded).
pub fn parse_recent_branches(text: &str, limit: usize) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut excluded: Vec<String> = Vec::new();
    for line in text.lines() {
        // `<sha> HEAD@{n}: checkout: moving from A to B` / `…: Branch: renamed A to B`
        let (renamed, rest) = if let Some(i) = line.find("checkout: moving from ") {
            (false, &line[i + "checkout: moving from ".len()..])
        } else if let Some(i) = line.find("renamed ") {
            (true, &line[i + "renamed ".len()..])
        } else {
            continue;
        };
        let Some((from, to)) = rest.rsplit_once(" to ") else {
            continue;
        };
        let strip = |s: &str| s.strip_prefix("refs/heads/").unwrap_or(s).to_string();
        let (from, to) = (strip(from), strip(to));
        if renamed {
            excluded.push(from);
        }
        if !excluded.contains(&to) && !names.contains(&to) {
            names.push(to);
        }
        if names.len() == limit {
            break;
        }
    }
    names
}

/// `getBranchCheckouts`: the branches checked out on or after `after` (from
/// HEAD's reflog), each with the time of its latest checkout. An orphan
/// branch without commits has no reflog to read: empty.
pub fn get_branch_checkouts(
    git: Arc<GitBinary>,
    workdir: &Path,
    after: SystemTime,
) -> Result<HashMap<String, SystemTime>> {
    // more than 8 digits: git's date parser reads seconds since the epoch
    let after = after
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let out = GitCommand::new(git)
        .args(["reflog", "--date=iso"])
        .arg(format!("--after={after}"))
        .args([
            "--pretty=%H %gd %gs",
            "--grep-reflog=checkout: moving from .* to .*$",
            "--",
        ])
        .current_dir(workdir)
        .allow_exit_code(128)
        .run()?;
    Ok(parse_branch_checkouts(&out.stdout_string()?))
}

/// Parse [`get_branch_checkouts`]' `<sha> HEAD@{<iso date>} checkout: moving
/// from A to B` lines (newest first): B with the newest date seen for it.
pub fn parse_branch_checkouts(text: &str) -> HashMap<String, SystemTime> {
    let mut checkouts = HashMap::new();
    for line in text.lines() {
        let Some((sha, rest)) = line.split_once(' ') else {
            continue;
        };
        if sha.is_empty() || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let Some(rest) = rest.strip_prefix("HEAD@{") else {
            continue;
        };
        let Some((date, subject)) = rest.split_once("} checkout: moving from ") else {
            continue;
        };
        let Some((_, branch)) = subject.rsplit_once(" to ") else {
            continue;
        };
        let Ok(time) = gix::date::parse(date, None) else {
            continue;
        };
        let at = if time.seconds >= 0 {
            UNIX_EPOCH + Duration::from_secs(time.seconds.unsigned_abs())
        } else {
            UNIX_EPOCH - Duration::from_secs(time.seconds.unsigned_abs())
        };
        checkouts.entry(branch.to_string()).or_insert(at);
    }
    checkouts
}

/// `getRemoteHEAD`: the branch `refs/remotes/<remote>/HEAD` points at.
pub fn remote_head(git: Arc<GitBinary>, workdir: &Path, remote: &str) -> Result<Option<String>> {
    let namespace = format!("refs/remotes/{remote}/");
    let target = crate::refs::get_symbolic_ref(git, workdir, &format!("{namespace}HEAD"))?;
    Ok(target
        .as_deref()
        .and_then(|t| t.strip_prefix(&namespace))
        .filter(|name| !name.is_empty())
        .map(str::to_string))
}

/// `getDefaultBranch`: `init.defaultBranch` from the global config, else `main`.
pub fn configured_default_branch(git: Arc<GitBinary>) -> String {
    GitCommand::new(git)
        .args(["config", "--global", "init.defaultBranch"])
        .allow_exit_code(1)
        .run()
        .ok()
        .and_then(|o| o.stdout_string().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "main".to_string())
}

/// `findDefaultBranch`: prefer a local branch tracking the remote's default,
/// then a local branch named like it, then the remote branch itself.
pub fn find_default_branch<'a>(
    branches: &'a [Branch],
    remote: Option<&str>,
    remote_head: Option<&str>,
    configured: &str,
) -> Option<&'a Branch> {
    let default_name = remote_head.unwrap_or(configured);
    let remote_ref = match (remote, remote_head) {
        (Some(r), Some(h)) => Some(format!("{r}/{h}")),
        _ => None,
    };
    let mut local_hit = None;
    let mut local_tracking_hit: Option<&Branch> = None;
    let mut remote_hit = None;
    for branch in branches {
        match branch.kind {
            BranchKind::Local => {
                if branch.name == default_name {
                    local_hit = Some(branch);
                }
                if let Some(remote_ref) = &remote_ref
                    && branch
                        .upstream
                        .as_deref()
                        .map(|u| u.strip_prefix("refs/remotes/").unwrap_or(u))
                        == Some(remote_ref.as_str())
                    && (local_tracking_hit.is_none() || branch.name == default_name)
                {
                    local_tracking_hit = Some(branch);
                }
            }
            BranchKind::Remote => {
                if remote_ref.as_deref() == Some(branch.name.as_str()) {
                    remote_hit = Some(branch);
                }
            }
        }
    }
    local_tracking_hit.or(local_hit).or(remote_hit)
}

/// Outcome of `git merge` (GHD `MergeResult`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeOutcome {
    Success,
    AlreadyUpToDate,
    /// The merge stopped on conflicts (`MERGE_HEAD` is set).
    Conflicts,
    Failed(String),
}

/// `merge(repository, branch, { squash })`.
pub fn merge_branch(
    git: Arc<GitBinary>,
    workdir: &Path,
    branch: &str,
    squash: bool,
) -> Result<MergeOutcome> {
    merge_branch_with_message(git, workdir, branch, squash, None)
}

/// GHD `createSquashMergeCommit`'s `interceptHooks`.
const SQUASH_MERGE_COMMIT_HOOKS: &[&str] = &[
    "pre-merge-commit",
    "prepare-commit-msg",
    "commit-msg",
    "post-commit",
    "pre-auto-gc",
];

/// [`merge_branch`]; a squash merge given `message` commits with it instead
/// of git's "Squashed commit of the following" list (on conflicts it becomes
/// `SQUASH_MSG`, and `MERGE_MSG`'s conflict note goes, so the commit made
/// after resolving uses it too).
pub fn merge_branch_with_message(
    git: Arc<GitBinary>,
    workdir: &Path,
    branch: &str,
    squash: bool,
    message: Option<&str>,
) -> Result<MergeOutcome> {
    let message = message.filter(|m| squash && !m.trim().is_empty());
    let mut cmd = GitCommand::new(git.clone())
        .args(["merge"])
        .current_dir(workdir)
        .intercept_hooks(&["pre-merge-commit", "post-merge", "commit-msg"]);
    if squash {
        cmd = cmd.arg("--squash");
    }
    // GHD 3.6.7 (desktop/desktop#22862)
    let out = cmd.args(["--", branch]).allow_exit_code(1).run()?;
    if !out.status.success() {
        let git_dir = crate::paths::git_dir(workdir);
        let stdout = out.stdout_string().unwrap_or_default();
        // a squash merge has no MERGE_HEAD: it leaves SQUASH_MSG and conflicted files
        if git_dir.join("MERGE_HEAD").exists()
            || (squash && git_dir.join("SQUASH_MSG").exists())
            || stdout.contains("Automatic merge failed")
        {
            if let Some(message) = message
                && git_dir.join("SQUASH_MSG").exists()
            {
                std::fs::write(git_dir.join("SQUASH_MSG"), message)?;
                let _ = std::fs::remove_file(git_dir.join("MERGE_MSG"));
            }
            return Ok(MergeOutcome::Conflicts);
        }
        return Ok(MergeOutcome::Failed(out.stderr.trim().to_string()));
    }
    if let Some(message) = message {
        GitCommand::new(git)
            .args(["commit", "-F", "-"])
            .stdin(message.as_bytes().to_vec())
            .current_dir(workdir)
            .intercept_hooks(SQUASH_MERGE_COMMIT_HOOKS)
            .run()?;
    } else if squash {
        GitCommand::new(git)
            .args(["commit", "--no-edit"])
            .current_dir(workdir)
            .intercept_hooks(SQUASH_MERGE_COMMIT_HOOKS)
            .run()?;
    }
    let stdout = out.stdout_string()?;
    Ok(if stdout.trim() == "Already up to date." {
        MergeOutcome::AlreadyUpToDate
    } else {
        MergeOutcome::Success
    })
}

/// `git merge --abort`
pub fn abort_merge(git: Arc<GitBinary>, workdir: &Path) -> Result<()> {
    GitCommand::new(git)
        .args(["merge", "--abort"])
        .current_dir(workdir)
        .run()?;
    Ok(())
}

/// Commits in `other` that are not in `base` (`getAheadBehind(base..other)`).
pub fn commits_ahead(git: Arc<GitBinary>, workdir: &Path, base: &str, other: &str) -> Result<u32> {
    let out = GitCommand::new(git)
        .args(["rev-list", "--count", "--end-of-options"])
        .arg(format!("{base}..{other}"))
        .current_dir(workdir)
        .run()?;
    Ok(out.stdout_string()?.trim().parse().unwrap_or(0))
}

/// Commits reachable from `branch` but from none of `bases`
/// (`rev-list --count --not <bases…> --not --end-of-options <branch> --`);
/// the delete-branch warning. With no bases every commit of the branch
/// counts. Only `branch` is past `--end-of-options` (git refuses `^-…`, and
/// the `--not` toggle must come before it), so `bases` are full ref names.
pub fn commits_not_in(
    git: Arc<GitBinary>,
    workdir: &Path,
    branch: &str,
    bases: &[String],
) -> Result<u32> {
    let out = GitCommand::new(git)
        .args(["rev-list", "--count", "--not"])
        .args(bases.iter().map(String::as_str))
        .args(["--not", "--end-of-options", branch, "--"])
        .current_dir(workdir)
        .run()?;
    Ok(out.stdout_string()?.trim().parse().unwrap_or(0))
}

/// The start point a local branch was created from, read from the first
/// entry of its reflog (`branch: Created from <start>`): a branch name (a
/// remote-tracking one as `origin/<name>` or `refs/remotes/...`), `HEAD`
/// or a commit. `None` without a reflog or such an entry. Feeds the pull
/// request base (Corvene, `333-pr-base-from-branch-origin`).
pub fn branch_created_from(workdir: &Path, branch: &str) -> Option<String> {
    use std::io::BufRead;
    let path = crate::paths::common_dir(workdir)
        .join("logs/refs/heads")
        .join(branch);
    let file = std::fs::File::open(path).ok()?;
    let mut first = String::new();
    std::io::BufReader::new(file).read_line(&mut first).ok()?;
    parse_created_from(&first)
}

/// The `<start>` of a reflog line whose message is `branch: Created from
/// <start>`.
pub fn parse_created_from(line: &str) -> Option<String> {
    let (_, message) = line.split_once('\t')?;
    let start = message.trim().strip_prefix("branch: Created from ")?.trim();
    (!start.is_empty()).then(|| start.to_string())
}

/// GHD `DesktopStashEntryMarker`
pub const DESKTOP_STASH_MARKER: &str = "!!GitHub_Desktop";

pub fn desktop_stash_message(branch: &str) -> String {
    format!("{DESKTOP_STASH_MARKER}<{branch}>")
}

/// Files marked `--assume-unchanged` (lower-case tag in `ls-files -v`)
/// whose working copy differs from the index. `git stash push` leaves them
/// out of the stash but still resets them, so their changes are lost.
pub fn modified_assume_unchanged(git: Arc<GitBinary>, workdir: &Path) -> Result<Vec<String>> {
    let out = GitCommand::new(git.clone())
        .args(["ls-files", "-v", "-s", "-z"])
        .current_dir(workdir)
        .run()?;
    let text = String::from_utf8_lossy(&out.stdout);
    // `<tag> <mode> <sha> <stage>\t<path>`
    let candidates: Vec<(String, String)> = text
        .split('\0')
        .filter_map(|record| {
            let (meta, path) = record.split_once('\t')?;
            let mut cols = meta.split(' ');
            let tag = cols.next()?;
            let sha = cols.nth(1)?;
            (tag.chars().all(|c| c.is_ascii_lowercase()) && workdir.join(path).is_file())
                .then(|| (path.to_string(), sha.to_string()))
        })
        .collect();
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let list: String = candidates.iter().map(|(p, _)| format!("{p}\n")).collect();
    let hashes = GitCommand::new(git)
        .args(["hash-object", "--stdin-paths"])
        .current_dir(workdir)
        .stdin(list)
        .run()?
        .stdout_string()?;
    Ok(candidates
        .into_iter()
        .zip(hashes.lines())
        .filter(|((_, indexed), now)| indexed != now.trim())
        .map(|((path, _), _)| path)
        .collect())
}

/// Corvene addition (flag `869`): refuse to stash while
/// [`modified_assume_unchanged`] finds files a stash would silently reset.
pub fn ensure_no_modified_assume_unchanged(git: Arc<GitBinary>, workdir: &Path) -> Result<()> {
    let files = modified_assume_unchanged(git, workdir)?;
    if files.is_empty() {
        return Ok(());
    }
    let shown: Vec<&str> = files.iter().take(5).map(String::as_str).collect();
    let more = match files.len() - shown.len() {
        0 => String::new(),
        n => format!(" and {n} more"),
    };
    Err(GitError::Gix(format!(
        "Your changes were not stashed: {}{more} {} marked assume-unchanged and {} local \
         changes that a stash would discard without saving. Commit or copy those changes, or \
         run git update-index --no-assume-unchanged on {}, then try again.",
        shown.join(", "),
        if files.len() == 1 { "is" } else { "are" },
        if files.len() == 1 { "has" } else { "have" },
        if files.len() == 1 { "it" } else { "them" },
    )))
}

/// `createDesktopStashEntry`: stage untracked files first so they are
/// included, then `stash push -m !!GitHub_Desktop<branch>`. Returns false
/// when there was nothing to stash. `guard_assume_unchanged` (flag `869`)
/// first stops with [`ensure_no_modified_assume_unchanged`]'s error.
pub fn create_desktop_stash(
    git: Arc<GitBinary>,
    workdir: &Path,
    branch: &str,
    guard_assume_unchanged: bool,
) -> Result<bool> {
    if guard_assume_unchanged {
        ensure_no_modified_assume_unchanged(git.clone(), workdir)?;
    }
    // stage untracked files (only those) so `stash push` picks them up
    let untracked = GitCommand::new(git.clone())
        .args(["ls-files", "--others", "--exclude-standard", "-z"])
        .current_dir(workdir)
        .run()?;
    let paths: Vec<u8> = untracked.stdout.clone();
    if paths.iter().any(|b| *b != 0) {
        GitCommand::new(git.clone())
            .args(["update-index", "--add", "-z", "--stdin"])
            .current_dir(workdir)
            .stdin(paths)
            .run()?;
    }
    let out = GitCommand::new(git)
        .args(["stash", "push", "-m", &desktop_stash_message(branch)])
        .current_dir(workdir)
        .allow_exit_code(1)
        .run()?;
    let stdout = out.stdout_string()?;
    if stdout.trim() == "No local changes to save" {
        return Ok(false);
    }
    if !out.status.success() {
        let stderr = out.stderr.clone();
        if stderr.lines().any(|l| l.starts_with("error: ")) {
            return Err(GitError::Failed {
                args: "stash push".into(),
                code: out.status.code(),
                stderr,
            });
        }
    }
    Ok(true)
}

/// `getStashes`: every entry of `refs/stash`; Corvene/GHD entries carry the
/// branch they were made on in their message. Corvene also reads when each
/// entry was made (`%ct`, for `797-stash-list`).
pub fn get_stashes(git: Arc<GitBinary>, workdir: &Path) -> Result<(Vec<StashEntry>, usize)> {
    let out = GitCommand::new(git)
        .args([
            "log",
            "-g",
            "-z",
            "--format=%gD%x00%H%x00%gs%x00%T%x00%P%x00%ct",
            "refs/stash",
            "--",
        ])
        .current_dir(workdir)
        .allow_exit_code(128)
        .run()?;
    if !out.status.success() {
        return Ok((Vec::new(), 0));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut entries = Vec::new();
    let mut total = 0usize;
    for record in text.split('\0').collect::<Vec<_>>().chunks(6) {
        if record.len() < 6 || record[0].trim().is_empty() {
            continue;
        }
        total += 1;
        let message = record[2].trim();
        let branch = message
            .rsplit_once(DESKTOP_STASH_MARKER)
            .and_then(|(_, tail)| tail.strip_prefix('<'))
            .and_then(|t| t.strip_suffix('>'))
            .map(|s| s.to_string());
        entries.push(StashEntry {
            name: record[0].trim().to_string(),
            sha: record[1].trim().to_string(),
            branch,
            message: message.to_string(),
            tree: record[3].trim().to_string(),
            parents: record[4]
                .split_whitespace()
                .map(|s| s.to_string())
                .collect(),
            date: record[5].trim().parse().unwrap_or(0),
        });
    }
    Ok((entries, total))
}

/// `getStashedFiles`: `stash show <sha> --raw --numstat -z`, plus the
/// untracked files of a `stash -u` entry (see [`stash_untracked_commit`]).
pub fn stashed_files(
    git: Arc<GitBinary>,
    workdir: &Path,
    sha: &str,
) -> Result<corvene_models::ChangesetData> {
    let out = GitCommand::new(git.clone())
        .args([
            "stash",
            "show",
            sha,
            "--raw",
            "--numstat",
            "-z",
            "--format=format:",
            "--no-color",
            "--",
        ])
        .current_dir(workdir)
        .run()?;
    let mut data = crate::log::parse_raw_log_with_numstat(&out.stdout, sha);
    if let Some(untracked) = stash_untracked_commit(git.clone(), workdir, sha)? {
        // Corvene `797-stash-list`: `stash push -u` keeps the untracked
        // files in a root commit, the stash's third parent, which `stash
        // show` leaves out before git 2.32 (`--include-untracked`). List them
        // as new files with that commit as their commitish, so their
        // contents are read from it. GHD's own stashes put new files in the
        // index and never have this parent.
        let out = GitCommand::new(git)
            .args([
                "diff-tree",
                "--root",
                "-r",
                "--raw",
                "--numstat",
                "-z",
                "--no-commit-id",
                "--no-color",
                &untracked,
                "--",
            ])
            .current_dir(workdir)
            .run()?;
        let extra = crate::log::parse_raw_log_with_numstat(&out.stdout, &untracked);
        data.lines_added += extra.lines_added;
        data.lines_deleted += extra.lines_deleted;
        for file in extra.files {
            // a path can be both: removed from the index, then left untracked
            if !data.files.iter().any(|f| f.path == file.path) {
                data.files.push(file);
            }
        }
    }
    Ok(data)
}

/// The commit holding a stash entry's untracked files (`<sha>^3`, made by
/// `stash push --include-untracked`), if it has one.
fn stash_untracked_commit(
    git: Arc<GitBinary>,
    workdir: &Path,
    sha: &str,
) -> Result<Option<String>> {
    let out = GitCommand::new(git)
        .args([
            "rev-parse",
            "-q",
            "--verify",
            &format!("{sha}^3^{{commit}}"),
        ])
        .current_dir(workdir)
        .allow_exit_code(1)
        .run()?
        .stdout_string()?;
    let out = out.trim();
    Ok((!out.is_empty()).then(|| out.to_string()))
}

/// The find half of `getLastDesktopStashEntryForBranch` over entries
/// [`get_stashes`] already read: the first (newest, the order is LIFO)
/// Desktop entry made on `branch`.
pub fn last_desktop_stash_entry_index(entries: &[StashEntry], branch: &str) -> Option<usize> {
    entries
        .iter()
        .position(|e| e.branch.as_deref() == Some(branch))
}

/// Corvene addition (flag `868`): pop the stash entry `sha` only while
/// `branch` is checked out, so a Restore clicked during a branch switch
/// does not land the stash on the other branch (GHD pops `stash@{n}` as
/// it was when the stash was listed).
pub fn pop_stash_on_branch(
    git: Arc<GitBinary>,
    workdir: &Path,
    sha: &str,
    branch: &str,
    options: crate::stash_ops::StashPopOptions,
) -> Result<crate::stash_ops::StashPop> {
    let head = GitCommand::new(git.clone())
        .args(["symbolic-ref", "--short", "-q", "HEAD"])
        .current_dir(workdir)
        .allow_exit_code(1)
        .run()?
        .stdout_string()?;
    let head = head.trim();
    if head != branch {
        let now = match head {
            "" => "a detached HEAD is".to_string(),
            head => format!("\"{head}\" is"),
        };
        return Err(GitError::Gix(format!(
            "The stash was made on \"{branch}\", but {now} checked out now. Switch back to \
             \"{branch}\" to restore it."
        )));
    }
    if stash_entry_matching_sha(git.clone(), workdir, sha)?.is_none() {
        return Err(GitError::Gix(
            "The stash is no longer there; it may have been restored or discarded already.".into(),
        ));
    }
    crate::stash_ops::pop_stash_entry_with(git, workdir, sha, options)
}

/// `getLastDesktopStashEntryForBranch`: the newest Desktop stash entry made
/// on `branch`.
pub fn get_last_desktop_stash_entry_for_branch(
    git: Arc<GitBinary>,
    workdir: &Path,
    branch: &str,
) -> Result<Option<StashEntry>> {
    let (mut entries, _) = get_stashes(git, workdir)?;
    Ok(last_desktop_stash_entry_index(&entries, branch).map(|i| entries.swap_remove(i)))
}

/// `getStashEntryMatchingSha`: the entry whose commit is `sha`. GHD looks
/// among the Desktop entries only; Corvene takes any, since the branch can
/// show a stash Desktop did not make (`728-show-latest-other-stash`).
pub(crate) fn stash_entry_matching_sha(
    git: Arc<GitBinary>,
    workdir: &Path,
    sha: &str,
) -> Result<Option<StashEntry>> {
    let (mut entries, _) = get_stashes(git, workdir)?;
    Ok(entries
        .iter()
        .position(|e| e.sha == sha)
        .map(|i| entries.swap_remove(i)))
}

/// `popStashEntry`: `git stash pop --quiet <name>` of the entry whose commit
/// is `stash_sha` (nothing when there is none). A pop that conflicts
/// exits 1 and keeps the entry; with nothing on stderr the changes were
/// applied, so the entry is dropped as GHD does. Merge conflicts git names
/// (`MergeConflicts`) are expected and keep the entry.
pub fn pop_stash_entry(git: Arc<GitBinary>, workdir: &Path, stash_sha: &str) -> Result<()> {
    crate::stash_ops::pop_stash_entry_with(
        git,
        workdir,
        stash_sha,
        crate::stash_ops::StashPopOptions::default(),
    )
    .map(|_| ())
}

/// `dropDesktopStashEntry`: `git stash drop <name>` of the entry whose
/// commit is `stash_sha`; nothing when there is none.
pub fn drop_desktop_stash_entry(
    git: Arc<GitBinary>,
    workdir: &Path,
    stash_sha: &str,
) -> Result<()> {
    if let Some(entry) = stash_entry_matching_sha(git.clone(), workdir, stash_sha)? {
        GitCommand::new(git)
            .args(["stash", "drop", &entry.name])
            .current_dir(workdir)
            .run()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn repo() -> (tempfile::TempDir, Arc<GitBinary>) {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(dir.path())
                    .env("GIT_AUTHOR_NAME", "T")
                    .env("GIT_AUTHOR_EMAIL", "t@example.com")
                    .env("GIT_COMMITTER_NAME", "T")
                    .env("GIT_COMMITTER_EMAIL", "t@example.com")
                    .status()
                    .unwrap()
                    .success()
            )
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "commit.gpgsign", "false"]);
        // Git for Windows turns autocrlf on globally
        run(&["config", "core.autocrlf", "false"]);
        // the code under test commits too; a CI runner has no identity
        run(&["config", "user.name", "T"]);
        run(&["config", "user.email", "t@example.com"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "first"]);
        (dir, Arc::new(crate::find_git().unwrap()))
    }

    #[test]
    fn moving_a_branch_back_keeps_pushed_and_moved_branches() {
        let (dir, git) = repo();
        let path = dir.path();
        let run = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(path)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        let base = run(&["rev-parse", "HEAD"]);
        for n in ["b", "c"] {
            std::fs::write(path.join(format!("{n}.txt")), n).unwrap();
            run(&["add", "."]);
            run(&["commit", "-q", "-m", n]);
        }
        let tip = run(&["rev-parse", "HEAD"]);
        let commits: Vec<String> = run(&["rev-list", &format!("{base}..{tip}")])
            .lines()
            .map(String::from)
            .collect();
        run(&["checkout", "-q", "-b", "feature"]);
        // a stale tip: nothing moves
        assert!(move_branch_back(git.clone(), path, "main", &base, &base, &commits, "m").is_err());
        assert_eq!(run(&["rev-parse", "main"]), tip);
        // pushed (a remote-tracking ref has them): nothing moves
        run(&["update-ref", "refs/remotes/origin/main", &tip]);
        assert!(move_branch_back(git.clone(), path, "main", &base, &tip, &commits, "m").is_err());
        run(&["update-ref", "-d", "refs/remotes/origin/main"]);
        // checked out (here): nothing moves
        assert!(
            move_branch_back(git.clone(), path, "feature", &base, &tip, &commits, "m").is_err()
        );
        move_branch_back(git, path, "main", &base, &tip, &commits, "moved").unwrap();
        assert_eq!(run(&["rev-parse", "main"]), base);
        assert_eq!(run(&["rev-parse", "feature"]), tip);
    }

    #[test]
    fn created_from_is_read_from_the_first_reflog_message() {
        let line = "0000000000000000000000000000000000000000 1111111111111111111111111111111111111111 A U Thor <a@example.com> 1700000000 +0000\tbranch: Created from origin/feature/x\n";
        assert_eq!(
            parse_created_from(line).as_deref(),
            Some("origin/feature/x")
        );
        let checkout = "0 1 A <a@b> 1 +0000\tbranch: Created from HEAD\n";
        assert_eq!(parse_created_from(checkout).as_deref(), Some("HEAD"));
        assert_eq!(parse_created_from("0 1 A <a@b> 1 +0000\tcommit: x\n"), None);
        assert_eq!(parse_created_from("no tab"), None);
    }

    #[test]
    fn parses_branch_tracking() {
        let text = "main\0refs/remotes/origin/main\0\n\
                    feature\0refs/remotes/origin/feature\0ahead 2, behind 1\n\
                    old\0refs/remotes/origin/old\0gone\n\
                    local\0\0\n";
        let map = parse_branch_tracking(text);
        assert_eq!(map.len(), 3);
        assert_eq!(map["main"], BranchTracking::default());
        assert_eq!(
            map["feature"],
            BranchTracking {
                ahead: 2,
                behind: 1,
                gone: false
            }
        );
        assert!(map["old"].gone);
    }

    #[test]
    fn reads_branch_tracking() {
        let (dir, git) = repo();
        let path = dir.path();
        let run = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(path)
                    .status()
                    .unwrap()
                    .success()
            )
        };
        run(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        run(&["config", "branch.main.remote", "origin"]);
        run(&["config", "branch.main.merge", "refs/heads/main"]);
        run(&["config", "remote.origin.url", "https://example.com/r.git"]);
        run(&[
            "config",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/origin/*",
        ]);
        assert_eq!(
            branch_tracking(git.clone(), path).unwrap()["main"],
            BranchTracking::default()
        );
        run(&["update-ref", "-d", "refs/remotes/origin/main"]);
        assert!(branch_tracking(git, path).unwrap()["main"].gone);
    }

    #[test]
    fn parses_recent_branches() {
        let text = "\
aaaa checkout: moving from main to feature\n\
bbbb Branch: renamed refs/heads/tmp to refs/heads/feature\n\
cccc checkout: moving from feature to tmp\n\
dddd checkout: moving from main to other\n\
eeee commit: something\n";
        assert_eq!(
            parse_recent_branches(text, 5),
            vec!["feature".to_string(), "other".to_string()]
        );
        assert_eq!(parse_recent_branches(text, 1), vec!["feature".to_string()]);
    }

    #[test]
    fn branch_lifecycle_and_merge() {
        let (dir, git) = repo();
        let path = dir.path();
        create_branch(git.clone(), path, "feature", None, false).unwrap();
        let info = crate::open_repository(path).unwrap();
        let feature = info
            .branches
            .iter()
            .find(|b| b.name == "feature")
            .cloned()
            .unwrap();
        assert!(feature.tip_time.is_some());
        checkout_branch(git.clone(), path, &feature).unwrap();
        std::fs::write(path.join("b.txt"), "b\n").unwrap();
        crate::stage_files(
            git.clone(),
            path,
            &crate::get_status(git.clone(), path).unwrap().files,
        )
        .unwrap();
        crate::commit(git.clone(), path, "feat\n", &Default::default()).unwrap();
        let main = Branch {
            name: "main".into(),
            kind: BranchKind::Local,
            full_name: "refs/heads/main".into(),
            tip: None,
            upstream: None,
            tip_time: None,
            tip_author: None,
            remote_name: None,
        };
        checkout_branch(git.clone(), path, &main).unwrap();
        assert_eq!(
            recent_branches(git.clone(), path, 5).unwrap(),
            vec!["main", "feature"]
        );
        assert_eq!(
            commits_ahead(git.clone(), path, "main", "feature").unwrap(),
            1
        );
        assert_eq!(
            commits_not_in(git.clone(), path, "feature", &["main".into()]).unwrap(),
            1
        );
        assert_eq!(
            commits_not_in(
                git.clone(),
                path,
                "refs/heads/main",
                &["refs/heads/feature".into()]
            )
            .unwrap(),
            0
        );
        assert_eq!(
            merge_branch(git.clone(), path, "feature", false).unwrap(),
            MergeOutcome::Success
        );
        assert_eq!(
            merge_branch(git.clone(), path, "feature", false).unwrap(),
            MergeOutcome::AlreadyUpToDate
        );
        rename_branch(git.clone(), path, "feature", "renamed").unwrap();
        delete_local_branch(git.clone(), path, "renamed").unwrap();
        let info = crate::open_repository(path).unwrap();
        assert!(info.branches.iter().all(|b| b.name != "renamed"));
        let configured = configured_default_branch(git.clone());
        assert!(!configured.is_empty());
        let default = find_default_branch(&info.branches, None, None, "main").unwrap();
        assert_eq!(default.name, "main");
    }

    #[test]
    fn desktop_stash_round_trip() {
        let (dir, git) = repo();
        let path = dir.path();
        std::fs::write(path.join("a.txt"), "changed\n").unwrap();
        std::fs::write(path.join("new.txt"), "n\n").unwrap();
        assert!(create_desktop_stash(git.clone(), path, "main", false).unwrap());
        assert!(
            crate::get_status(git.clone(), path)
                .unwrap()
                .files
                .is_empty()
        );
        let (entries, total) = get_stashes(git.clone(), path).unwrap();
        assert_eq!(total, 1);
        assert_eq!(entries[0].branch.as_deref(), Some("main"));
        pop_stash_entry(git.clone(), path, &entries[0].sha).unwrap();
        let files = crate::get_status(git.clone(), path).unwrap().files;
        assert_eq!(files.len(), 2);
        assert!(create_desktop_stash(git.clone(), path, "main", false).unwrap());
        let (entries, _) = get_stashes(git.clone(), path).unwrap();
        drop_desktop_stash_entry(git.clone(), path, &entries[0].sha).unwrap();
        assert_eq!(get_stashes(git, path).unwrap().1, 0);
    }

    #[test]
    fn merge_conflict_is_reported() {
        let (dir, git) = repo();
        let path = dir.path();
        create_branch(git.clone(), path, "other", None, false).unwrap();
        std::fs::write(path.join("a.txt"), "main change\n").unwrap();
        crate::commit(git.clone(), path, "main\n", &Default::default()).unwrap_or_default();
        let run = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(path)
                .env("GIT_AUTHOR_NAME", "T")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "T")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .status()
                .unwrap()
        };
        run(&["commit", "-q", "-am", "main change"]);
        run(&["checkout", "-q", "other"]);
        std::fs::write(path.join("a.txt"), "other change\n").unwrap();
        run(&["commit", "-q", "-am", "other change"]);
        run(&["checkout", "-q", "main"]);
        assert_eq!(
            merge_branch(git.clone(), path, "other", false).unwrap(),
            MergeOutcome::Conflicts
        );
        abort_merge(git, path).unwrap();
    }

    #[test]
    fn stash_lists_files_and_pops() {
        let (dir, git) = repo();
        let path = dir.path();
        std::fs::write(path.join("a.txt"), "changed\n").unwrap();
        std::fs::write(path.join("new.txt"), "untracked\n").unwrap();
        create_desktop_stash(git.clone(), path, "main", false).unwrap();
        let (stashes, count) = get_stashes(git.clone(), path).unwrap();
        assert_eq!(count, 1);
        let stash = stashes
            .iter()
            .find(|s| s.branch.as_deref() == Some("main"))
            .unwrap();
        let files = stashed_files(git.clone(), path, &stash.sha).unwrap().files;
        let mut names: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        names.sort();
        assert_eq!(names, ["a.txt", "new.txt"]);
        pop_stash_entry(git.clone(), path, &stash.sha).unwrap();
        assert_eq!(get_stashes(git, path).unwrap().1, 0);
        assert!(path.join("new.txt").exists());
    }

    #[test]
    fn stash_lists_untracked_files_of_a_stash_u_entry() {
        let (dir, git) = repo();
        let path = dir.path();
        std::fs::write(path.join("a.txt"), "changed\n").unwrap();
        std::fs::create_dir(path.join("dir")).unwrap();
        std::fs::write(path.join("dir/new.txt"), "one\ntwo\n").unwrap();
        let ok = Command::new("git")
            .args(["stash", "push", "-q", "--include-untracked"])
            .current_dir(path)
            .env("GIT_AUTHOR_NAME", "T")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "T")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let (stashes, _) = get_stashes(git.clone(), path).unwrap();
        let sha = &stashes[0].sha;
        let data = stashed_files(git.clone(), path, sha).unwrap();
        let mut files: Vec<_> = data.files.iter().collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "a.txt");
        assert_eq!(&files[0].commitish, sha);
        assert_eq!(files[1].path, "dir/new.txt");
        assert_eq!(files[1].status.kind, corvene_models::FileStatusKind::New);
        let untracked = stash_untracked_commit(git.clone(), path, sha)
            .unwrap()
            .unwrap();
        assert_eq!(files[1].commitish, untracked);
        assert_eq!(data.lines_added, 3);
        // the untracked file's patch comes from the root commit, even with
        // `log.showRoot` off
        let ok = Command::new("git")
            .args(["config", "log.showRoot", "false"])
            .current_dir(path)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        match crate::stash_file_diff(git.clone(), path, files[1], false).unwrap() {
            corvene_models::Diff::Text { hunks, .. } => {
                assert_eq!(hunks[0].new_lines, 2);
            }
            other => panic!("expected a text diff, got {other:?}"),
        }
        // a plain stash has no untracked parent
        pop_stash_entry(git.clone(), path, sha).unwrap();
        create_desktop_stash(git.clone(), path, "main", false).unwrap();
        let (stashes, _) = get_stashes(git.clone(), path).unwrap();
        assert_eq!(
            stash_untracked_commit(git, path, &stashes[0].sha).unwrap(),
            None
        );
    }

    #[test]
    fn stash_pops_only_on_its_branch() {
        let (dir, git) = repo();
        let path = dir.path();
        std::fs::write(path.join("a.txt"), "changed\n").unwrap();
        create_desktop_stash(git.clone(), path, "main", false).unwrap();
        let (stashes, _) = get_stashes(git.clone(), path).unwrap();
        let sha = stashes[0].sha.clone();
        checkout_new_branch(git.clone(), path, "other").unwrap();
        let err =
            pop_stash_on_branch(git.clone(), path, &sha, "main", Default::default()).unwrap_err();
        assert!(
            err.to_string().contains("\"other\" is checked out"),
            "{err}"
        );
        assert_eq!(get_stashes(git.clone(), path).unwrap().1, 1);
        crate::GitCommand::new(git.clone())
            .args(["checkout", "-q", "main"])
            .current_dir(path)
            .run()
            .unwrap();
        pop_stash_on_branch(git.clone(), path, &sha, "main", Default::default()).unwrap();
        assert_eq!(get_stashes(git.clone(), path).unwrap().1, 0);
        assert!(pop_stash_on_branch(git, path, &sha, "main", Default::default()).is_err());
    }

    #[test]
    fn stash_refuses_to_reset_modified_assume_unchanged_files() {
        let (dir, git) = repo();
        let path = dir.path();
        GitCommand::new(git.clone())
            .args(["update-index", "--assume-unchanged", "a.txt"])
            .current_dir(path)
            .run()
            .unwrap();
        assert!(
            modified_assume_unchanged(git.clone(), path)
                .unwrap()
                .is_empty()
        );
        std::fs::write(path.join("a.txt"), "local only\n").unwrap();
        std::fs::write(path.join("new.txt"), "untracked\n").unwrap();
        assert_eq!(
            modified_assume_unchanged(git.clone(), path).unwrap(),
            ["a.txt"]
        );
        let err = create_desktop_stash(git.clone(), path, "main", true).unwrap_err();
        assert!(err.to_string().contains("a.txt is marked"), "{err}");
        assert_eq!(get_stashes(git, path).unwrap().1, 0);
        assert_eq!(
            std::fs::read_to_string(path.join("a.txt")).unwrap(),
            "local only\n"
        );
    }

    #[test]
    fn checkout_reports_progress_inside_with_progress() {
        let (dir, git) = repo();
        let status = Command::new("git")
            .args(["branch", "other"])
            .current_dir(dir.path())
            .status()
            .unwrap();
        assert!(status.success());
        let branch = Branch {
            name: "other".into(),
            kind: BranchKind::Local,
            full_name: "refs/heads/other".into(),
            tip: None,
            upstream: None,
            tip_time: None,
            tip_author: None,
            remote_name: None,
        };
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink = seen.clone();
        crate::op_progress::with_progress(
            move |p| sink.borrow_mut().push((p.description, p.value)),
            || checkout_branch(git.clone(), dir.path(), &branch),
        )
        .unwrap();
        let seen = seen.borrow();
        let initial = if cfg!(target_os = "macos") {
            "Switching to Branch"
        } else {
            "Switching to branch"
        };
        // GHD: the initial report, then the submodule update in 90..100 %
        assert_eq!(seen.first(), Some(&(initial.to_string(), 0.)));
        let tail: Vec<_> = seen.iter().rev().take(2).rev().cloned().collect();
        assert_eq!(
            tail,
            vec![
                ("Updating submodules".to_string(), 0.9),
                ("Submodules updated".to_string(), 1.)
            ]
        );
        // and without a sink nothing is asked for
        checkout_branch(
            git,
            dir.path(),
            &Branch {
                name: "main".into(),
                full_name: "refs/heads/main".into(),
                ..branch
            },
        )
        .unwrap();
    }

    /// GHD 3.6.7 (desktop/desktop#22862): git refuses to create a branch
    /// whose name starts with `-`, but `update-ref` or a fetch can still
    /// make one, and it must never be read as an option.
    #[test]
    fn names_starting_with_a_dash_are_not_options() {
        let (dir, git) = repo();
        let path = dir.path();
        let run = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(path)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        let first = run(&["rev-parse", "HEAD"]);
        run(&["update-ref", "refs/heads/--local", "HEAD"]);
        std::fs::write(path.join("a.txt"), "two\n").unwrap();
        run(&["commit", "-q", "-am", "second"]);
        let second = run(&["rev-parse", "HEAD"]);

        // rev-list, log, merge-base and diff
        assert_eq!(
            commits_ahead(git.clone(), path, "--local", "main").unwrap(),
            1
        );
        let main = ["refs/heads/main".to_string()];
        assert_eq!(
            commits_not_in(git.clone(), path, "--local", &main).unwrap(),
            0
        );
        let local = ["refs/heads/--local".to_string()];
        assert_eq!(
            commits_not_in(git.clone(), path, "main", &local).unwrap(),
            1
        );
        let between = crate::rebase_ops::commits_between(git.clone(), path, "--local", "main")
            .unwrap()
            .unwrap();
        assert_eq!(between.len(), 1);
        assert_eq!(between[0].sha, second);
        assert_eq!(
            crate::log::merge_base(git.clone(), path, "--local", "main").unwrap(),
            Some(first.clone())
        );
        assert!(crate::rebase_ops::is_ancestor(
            git.clone(),
            path,
            "--local",
            "main"
        ));
        assert_eq!(
            crate::rebase_ops::commit_subject(git.clone(), path, "--local").unwrap(),
            "first"
        );
        assert_eq!(
            crate::log::local_only_commits(git.clone(), path, "--local", 10).unwrap(),
            vec![first.clone()]
        );
        let history = crate::log::filtered_history(
            git.clone(),
            path,
            "--local",
            &crate::log::HistoryQuery::default(),
            false,
            None,
        )
        .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].sha, first);
        assert_eq!(
            crate::repo::symmetric_ahead_behind(git.clone(), path, "--local", "main").unwrap(),
            Some(corvene_models::AheadBehind {
                ahead: 0,
                behind: 1
            })
        );
        let changed =
            crate::log::range_changed_files(git.clone(), path, "--local", "main", true, &second)
                .unwrap()
                .unwrap();
        assert_eq!(changed.files.len(), 1);
        assert_eq!(
            crate::diff::resolve_blob_revision(git.clone(), path, "--local").unwrap(),
            first
        );
        assert_eq!(
            crate::file_browser::list_files(git.clone(), path, Some("--local")).unwrap(),
            vec!["a.txt".to_string()]
        );

        // branches and tags
        create_branch(git.clone(), path, "from-dash", Some("--local"), false).unwrap();
        assert_eq!(run(&["rev-parse", "from-dash"]), first);
        assert!(
            get_merged_branches(git.clone(), path, "--local")
                .unwrap()
                .contains_key("refs/heads/from-dash")
        );
        crate::history_ops::create_tag(git.clone(), path, "v1", "--local", "").unwrap();
        assert_eq!(run(&["rev-parse", "v1^{commit}"]), first);
        crate::history_ops::delete_tag(git.clone(), path, "v1").unwrap();
        delete_local_branch(git.clone(), path, "from-dash").unwrap();

        // checkout, then merge and reset by the dash name
        let branch = Branch {
            name: "--local".into(),
            kind: BranchKind::Local,
            full_name: "refs/heads/--local".into(),
            tip: Some(first.clone()),
            upstream: None,
            tip_time: None,
            tip_author: None,
            remote_name: None,
        };
        checkout_branch(git.clone(), path, &branch).unwrap();
        assert_eq!(run(&["symbolic-ref", "HEAD"]), "refs/heads/--local");
        assert_eq!(
            std::fs::read_to_string(path.join("a.txt")).unwrap(),
            "one\n"
        );
        run(&["checkout", "-q", "main"]);
        crate::history_ops::reset_to(
            git.clone(),
            path,
            crate::history_ops::ResetMode::Hard,
            "--local",
        )
        .unwrap();
        assert_eq!(run(&["rev-parse", "HEAD"]), first);
        assert_eq!(
            merge_branch(git.clone(), path, "--local", false).unwrap(),
            MergeOutcome::AlreadyUpToDate
        );
        rename_branch(git, path, "--local", "renamed").unwrap();
        assert_eq!(run(&["rev-parse", "renamed"]), first);
    }
}
