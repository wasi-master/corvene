//! Corvene `356-stacked-pull-requests` (desktop/desktop#22660): a stack of
//! branches as a stack of pull requests. GHD opens pull requests one at a
//! time on the compare page (`lib/stores/app-store.ts`,
//! `_createPullRequest`) and knows nothing of stacks.
//!
//! Nothing is stored: the stack is found from the local branches among the
//! current branch's commits ([`crate::stacked_refs`]) and from the open pull
//! requests' bases, so one made elsewhere shows up too.
//!
//! - Create: [`Dispatcher::create_stacked_pull_requests`] pushes each branch
//!   and opens its pull request on the branch below (the lowest on the
//!   default branch).
//! - Lower pull request merged: after a pull request refresh,
//!   [`Dispatcher::check_stack_base`] asks GitHub about the branches below
//!   and offers Update Stack, which retargets the next pull request and runs
//!   `git rebase --onto`, so no merge commit lands on the stack.
//! - Rewritten: [`Dispatcher::push_stacked_branches`] pushes every branch
//!   with force with lease.

use std::collections::HashSet;

use corvene_github::Client;
use corvene_models::{GitHubRepository, PullRequest};

use crate::dispatcher::Dispatcher;
use crate::host::{AsyncCtx, Host};
use crate::mco::Banner;
use crate::remote::spawn_bg;
use crate::state::{AppState, Popup};

/// A branch of the stack the create dialog lists, lowest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackBranch {
    pub name: String,
    /// The branch below, or the default branch.
    pub base: String,
    /// Its open pull request, as far as the cache knows.
    pub pull_request: Option<u64>,
}

/// Where a pull request sits in its stack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackPosition {
    /// From 1, the lowest.
    pub position: usize,
    pub total: usize,
    pub below: Option<PullRequest>,
    pub above: Option<PullRequest>,
}

impl StackPosition {
    /// `2 of 3`
    pub fn label(&self) -> String {
        format!("{} of {}", self.position, self.total)
    }
}

/// A pull request whose head and base are in one repository: only those can
/// stack, since a base branch must be in the repository the pull request
/// opens in.
fn same_repository(pr: &PullRequest) -> bool {
    match (&pr.head.repository, &pr.base.repository) {
        (Some(head), Some(base)) => {
            head.owner.eq_ignore_ascii_case(&base.owner)
                && head.name.eq_ignore_ascii_case(&base.name)
        }
        _ => false,
    }
}

/// `pr`'s stack among `prs`, lowest first: down through the pull request
/// whose head is its base, up through the lowest numbered one based on its
/// head. A branch with two pull requests on it follows the first.
pub fn stack_of<'a>(prs: &'a [PullRequest], pr: &'a PullRequest) -> Vec<&'a PullRequest> {
    let own: Vec<&PullRequest> = prs.iter().filter(|p| same_repository(p)).collect();
    if !same_repository(pr) {
        return vec![pr];
    }
    let mut stack = vec![pr];
    let mut seen: HashSet<&str> = HashSet::from([pr.head.ref_name.as_str()]);
    while let Some(below) = own
        .iter()
        .find(|p| p.head.ref_name == stack[0].base.ref_name)
    {
        if !seen.insert(below.head.ref_name.as_str()) {
            break;
        }
        stack.insert(0, below);
    }
    while let Some(&top) = stack.last() {
        let above = own
            .iter()
            .filter(|p| p.base.ref_name == top.head.ref_name)
            .min_by_key(|p| p.number);
        let Some(above) = above else { break };
        if !seen.insert(above.head.ref_name.as_str()) {
            break;
        }
        stack.push(above);
    }
    stack
}

/// [`StackPosition`] of `pr`, when it has a pull request above or below.
pub fn position_in(prs: &[PullRequest], pr: &PullRequest) -> Option<StackPosition> {
    let stack = stack_of(prs, pr);
    if stack.len() < 2 {
        return None;
    }
    let at = stack.iter().position(|p| p.number == pr.number)?;
    Some(StackPosition {
        position: at + 1,
        total: stack.len(),
        below: at.checked_sub(1).map(|i| stack[i].clone()),
        above: stack.get(at + 1).map(|p| (*p).clone()),
    })
}

/// The branches `order` (newest first) points at, lowest first, one per
/// commit, ending at `current`, whose own tip no other branch can stand for.
fn order_branches(
    order: &[String],
    tips: &std::collections::HashMap<String, Vec<String>>,
    current: &str,
    current_tip: Option<&str>,
) -> Vec<String> {
    let mut names: Vec<String> = order
        .iter()
        .rev()
        .filter(|sha| Some(sha.as_str()) != current_tip)
        .filter_map(|sha| tips.get(sha)?.first().cloned())
        .collect();
    names.push(current.to_string());
    names
}

impl AppState {
    /// `pr`'s place in its stack; `None` with the flag off or alone.
    pub fn stack_position(&self, id: u64, pr: &PullRequest) -> Option<StackPosition> {
        if !self.flags.bool(crate::flags::ids::STACKED_PULL_REQUESTS) {
            return None;
        }
        position_in(self.pull_requests_for(id), pr)
    }

    /// Local branches of the stack below and at the current branch, lowest
    /// first. Empty on the default branch.
    fn local_stack(&self, id: u64) -> Vec<String> {
        let Some(rs) = self.repo_states.get(&id) else {
            return Vec::new();
        };
        let (Some(refs), Some(current)) = (
            rs.stacked_refs.as_ref(),
            rs.info.as_ref().and_then(|i| i.current_branch()),
        ) else {
            return Vec::new();
        };
        order_branches(
            &refs.order,
            &refs.tips,
            &current.name,
            current.tip.as_deref(),
        )
    }

    /// What the create dialog lists.
    pub fn stack_branches(&self, id: u64) -> Vec<StackBranch> {
        let Some(default) = self
            .repo_states
            .get(&id)
            .and_then(|rs| rs.stacked_refs.as_ref())
            .map(|r| r.base.clone())
        else {
            return Vec::new();
        };
        let prs = self.pull_requests_for(id);
        let names = self.local_stack(id);
        names
            .iter()
            .enumerate()
            .map(|(i, name)| StackBranch {
                name: name.clone(),
                base: i
                    .checked_sub(1)
                    .map_or_else(|| default.clone(), |below| names[below].clone()),
                pull_request: prs
                    .iter()
                    .find(|p| p.head.ref_name == *name && same_repository(p))
                    .map(|p| p.number),
            })
            .collect()
    }

    /// Every local branch of the stack, the pull requests above the current
    /// branch included, lowest first.
    fn whole_stack(&self, id: u64) -> Vec<String> {
        let mut names = self.local_stack(id);
        let Some(rs) = self.repo_states.get(&id) else {
            return names;
        };
        let Some(info) = rs.info.as_ref() else {
            return names;
        };
        let current = self.current_pull_request(id);
        if let Some(pr) = current {
            let chain = stack_of(self.pull_requests_for(id), pr);
            let at = chain.iter().position(|p| p.number == pr.number);
            for above in chain.iter().skip(at.map_or(chain.len(), |i| i + 1)) {
                let local = info.branches.iter().any(|b| {
                    b.kind == corvene_models::BranchKind::Local && b.name == above.head.ref_name
                });
                if local && !names.contains(&above.head.ref_name) {
                    names.push(above.head.ref_name.clone());
                }
            }
        }
        names
    }
}

impl Dispatcher {
    /// The repository a stack's pull requests open in, with its API access.
    /// `Err` says why there is none. A fork that contributes to its parent
    /// has none: the parent cannot base a pull request on a fork's branch.
    fn stack_target(
        id: u64,
        cx: &dyn Host,
    ) -> Result<(GitHubRepository, corvene_github::Endpoint, String), String> {
        let s = Self::state(cx).read(cx);
        let repository = s.repository(id).ok_or("The repository is gone.")?;
        if repository.is_fork_contributing_to_parent() {
            return Err(
                "Pull requests from this fork open in its parent, and a pull request \
                        there cannot use a branch of the fork as its base."
                    .to_string(),
            );
        }
        let target = repository
            .github
            .clone()
            .ok_or("This repository is not on GitHub.")?;
        let (endpoint, token, _) = Self::api_for_repository(id, &target, cx)
            .ok_or("Sign in to GitHub to work with pull requests.")?;
        Ok((target, endpoint, token))
    }

    /// Branch › Create Stacked Pull Requests…
    pub fn show_create_stacked_pull_requests(id: u64, cx: &mut dyn Host) {
        if let Err(err) = Self::stack_target(id, cx) {
            return Self::show_error("Could not create stacked pull requests", err, cx);
        }
        if Self::state(cx).read(cx).stack_branches(id).is_empty() {
            return Self::show_error(
                "Could not create stacked pull requests",
                "Check out the top branch of the stack first. The default branch has no \
                 commits to stack."
                    .to_string(),
                cx,
            );
        }
        Self::show_popup(Popup::CreateStackedPullRequests { repo: id }, cx);
    }

    /// The dialog's Create: push each branch and open or retarget its pull
    /// request, lowest first; stop at the first failure.
    pub fn create_stacked_pull_requests(id: u64, draft: bool, cx: &mut dyn Host) {
        let branches = Self::state(cx).read(cx).stack_branches(id);
        let target = Self::stack_target(id, cx);
        let (Ok((target, endpoint, token)), Some((git, workdir)), Some(remote)) = (
            target,
            Self::repo_context(id, cx),
            Self::current_remote(id, cx),
        ) else {
            return Self::close_popup(cx);
        };
        Self::arm_credential_helper_for(id, &remote.url, cx);
        let askpass = Self::askpass_env_for(id, &remote.url, cx);
        let pushes: Vec<_> = branches
            .iter()
            .map(|b| Self::push_arguments(id, &b.name, &remote.name, cx))
            .collect();
        spawn_bg(
            cx,
            move || {
                let client = Client::new(endpoint, token);
                let (mut created, mut updated) = (0, 0);
                for (branch, (push_remote, remote_branch)) in branches.iter().zip(pushes) {
                    let name = &branch.name;
                    let fail = |err: String| format!("{name}: {err}");
                    corvene_git::remote_ops::push(
                        git.clone(),
                        &workdir,
                        &push_remote,
                        name,
                        remote_branch.as_deref(),
                        &[],
                        false,
                        askpass.as_ref(),
                        &mut |_, _| {},
                    )
                    .map_err(|e| fail(e.to_string()))?;
                    let open = client
                        .pull_requests_with_head(&target.owner, &target.name, name, "open")
                        .map_err(|e| fail(e.to_string()))?;
                    match open.first() {
                        Some(pr) if pr.base.ref_name == branch.base => {}
                        Some(pr) => {
                            client
                                .update_pull_request_base(
                                    &target.owner,
                                    &target.name,
                                    pr.number,
                                    &branch.base,
                                )
                                .map_err(|e| fail(e.to_string()))?;
                            updated += 1;
                        }
                        None => {
                            let title = corvene_git::commit_subject(git.clone(), &workdir, name)
                                .map_err(|e| fail(e.to_string()))?;
                            client
                                .create_pull_request(
                                    &target.owner,
                                    &target.name,
                                    &title,
                                    name,
                                    &branch.base,
                                    draft,
                                )
                                .map_err(|e| fail(e.to_string()))?;
                            created += 1;
                        }
                    }
                }
                Ok::<_, String>((created, updated))
            },
            move |result, cx| {
                Self::close_popup(cx);
                match result {
                    Ok((created, updated)) => Self::set_banner(
                        Banner::StackedPullRequestsCreated { created, updated },
                        cx,
                    ),
                    Err(err) => Self::show_error("Could not create the pull requests", err, cx),
                }
                Self::refresh_repository(id, cx);
                Self::refresh_pull_requests(id, true, cx);
            },
        );
    }

    /// The remote and remote branch `name` pushes to: its upstream's, else
    /// `default_remote` with no name, which publishes it.
    fn push_arguments(
        id: u64,
        name: &str,
        default_remote: &str,
        cx: &dyn Host,
    ) -> (String, Option<String>) {
        let branch = Self::branch_by_name(id, name, cx);
        let remote = branch
            .as_ref()
            .and_then(|b| b.upstream_remote_name().map(str::to_string));
        let remote_branch = branch
            .as_ref()
            .and_then(|b| b.upstream_without_remote().map(str::to_string));
        (
            remote.unwrap_or_else(|| default_remote.to_string()),
            remote_branch,
        )
    }

    /// Branch › Push Stacked Branches (and the banner's Push Stack): every
    /// branch, lowest first, with force with lease since a rewrite is why
    /// this runs.
    pub fn push_stacked_branches(id: u64, cx: &mut dyn Host) {
        let names = Self::state(cx).read(cx).whole_stack(id);
        if names.is_empty() {
            return Self::show_error(
                "Could not push the stack",
                "The default branch is checked out. Check out a branch of the stack first."
                    .to_string(),
                cx,
            );
        }
        let (Some((git, workdir)), Some(remote)) =
            (Self::repo_context(id, cx), Self::current_remote(id, cx))
        else {
            return;
        };
        Self::arm_credential_helper_for(id, &remote.url, cx);
        let askpass = Self::askpass_env_for(id, &remote.url, cx);
        let pushes: Vec<_> = names
            .iter()
            .map(|n| Self::push_arguments(id, n, &remote.name, cx))
            .collect();
        let total = names.len();
        Self::set_banner(Banner::StackPushing { done: 0, total }, cx);
        let (tx, rx) = async_channel::unbounded::<usize>();
        let task = cx.background_executor().spawn(async move {
            for (i, (name, (push_remote, remote_branch))) in names.iter().zip(pushes).enumerate() {
                corvene_git::remote_ops::push(
                    git.clone(),
                    &workdir,
                    &push_remote,
                    name,
                    remote_branch.as_deref(),
                    &[],
                    true,
                    askpass.as_ref(),
                    &mut |_, _| {},
                )
                .map_err(|e| format!("{name}: {e}"))?;
                let _ = tx.send_blocking(i + 1);
            }
            Ok::<_, String>(())
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            while let Ok(done) = rx.recv().await {
                cx.update(|cx| Self::set_banner(Banner::StackPushing { done, total }, cx));
            }
            let result = task.await;
            cx.update(|cx| {
                match result {
                    Ok(()) => Self::set_banner(Banner::StackPushed { total }, cx),
                    Err(err) => {
                        Self::clear_banner(cx);
                        Self::show_error("Could not push the stack", err, cx);
                    }
                }
                Self::refresh_repository(id, cx);
                Self::refresh_pull_requests(id, true, cx);
            });
        })
        .detach();
    }

    /// After a pull request refresh: did a branch below the current one
    /// merge? GitHub answers per branch, highest first, so the offer moves
    /// past everything that landed. One request per branch below, and none
    /// for a branch with nothing below it.
    pub(crate) fn check_stack_base(id: u64, cx: &mut dyn Host) {
        let s = Self::state(cx).read(cx);
        if !s.flags.bool(crate::flags::ids::STACKED_PULL_REQUESTS) {
            return;
        }
        let Some(rs) = s.repo_states.get(&id) else {
            return;
        };
        let Some(refs) = rs.stacked_refs.as_ref() else {
            return;
        };
        let onto = refs.base.clone();
        let local_tips: Vec<(String, String)> = refs
            .tips
            .iter()
            .flat_map(|(sha, names)| names.iter().map(move |n| (n.clone(), sha.clone())))
            .collect();
        let mut below = s.local_stack(id);
        below.pop();
        // the base GitHub still has, its local branch maybe gone
        if let Some(pr) = s.current_pull_request(id)
            && pr.base.ref_name != onto
            && !below.contains(&pr.base.ref_name)
        {
            below.push(pr.base.ref_name.clone());
        }
        if below.is_empty() {
            return;
        }
        let Ok((target, endpoint, token)) = Self::stack_target(id, cx) else {
            return;
        };
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        spawn_bg(
            cx,
            move || {
                let client = Client::new(endpoint, token);
                for name in below.iter().rev() {
                    let closed = client
                        .pull_requests_with_head(&target.owner, &target.name, name, "closed")
                        .ok()?;
                    let Some(merged) = closed.into_iter().find(|p| p.merged_at.is_some()) else {
                        continue;
                    };
                    // the head GitHub merged, or the local branch if it has
                    // moved on: whichever HEAD is built on
                    let local = local_tips.iter().find(|(n, _)| n == name).map(|(_, s)| s);
                    let old_base = std::iter::once(&merged.head.sha)
                        .chain(local)
                        .find(|sha| corvene_git::is_ancestor(git.clone(), &workdir, sha, "HEAD"))?
                        .clone();
                    return Some((name.clone(), merged.number, old_base));
                }
                None
            },
            move |found, cx| {
                let Some((branch, number, old_base)) = found else {
                    return;
                };
                let first = Self::state(cx).update(cx, |s, _| {
                    let offered = &mut s.repo_state_mut(id).stack_base_offered;
                    let first = offered.as_ref() != Some(&old_base);
                    *offered = Some(old_base.clone());
                    first
                });
                if first {
                    Self::set_banner(
                        Banner::StackBaseMerged {
                            repo: id,
                            branch,
                            number,
                            old_base,
                            onto,
                        },
                        cx,
                    );
                }
            },
        );
    }

    /// The banner's Update Stack: fetch, point the pull requests based on
    /// the merged `branch` at `onto`, then move the commits above `old_base`
    /// onto the fetched `onto`, the stack's top branch checked out.
    pub fn update_stack(
        id: u64,
        branch: String,
        old_base: String,
        onto: String,
        cx: &mut dyn Host,
    ) {
        if !Self::working_directory_files(id, cx).is_empty() {
            return Self::show_error(
                "Could not update the stack",
                "Commit or stash your local changes first.".to_string(),
                cx,
            );
        }
        let target = match Self::stack_target(id, cx) {
            Ok(target) => target,
            Err(err) => return Self::show_error("Could not update the stack", err, cx),
        };
        let Some(remote) = Self::current_remote(id, cx) else {
            return;
        };
        Self::fetch_remote_then(
            id,
            None,
            false,
            move |_, cx| {
                let (github, endpoint, token) = target;
                let s = Self::state(cx).read(cx);
                let top = s.whole_stack(id).pop();
                let retarget: Vec<u64> = s
                    .pull_requests_for(id)
                    .iter()
                    .filter(|p| p.base.ref_name == branch)
                    .map(|p| p.number)
                    .collect();
                let Some(top) = top else {
                    return;
                };
                let base = onto.clone();
                spawn_bg(
                    cx,
                    move || {
                        let client = Client::new(endpoint, token);
                        retarget.iter().try_for_each(|number| {
                            client
                                .update_pull_request_base(
                                    &github.owner,
                                    &github.name,
                                    *number,
                                    &base,
                                )
                                .map(|_| ())
                        })
                    },
                    move |result, cx| {
                        if let Err(err) = result {
                            return Self::show_error(
                                "Could not update the stack",
                                err.to_string(),
                                cx,
                            );
                        }
                        let onto = format!("{}/{onto}", remote.name);
                        Self::run_stack_rebase(id, onto, old_base, top, cx);
                    },
                );
            },
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use corvene_models::PullRequestRef;
    use std::collections::HashMap;

    fn repository(owner: &str) -> Option<GitHubRepository> {
        serde_json::from_value(serde_json::json!({
            "endpoint": "", "owner": owner, "name": "repo", "html_url": "", "clone_url": "",
        }))
        .ok()
    }

    fn pr(number: u64, head: &str, base: &str) -> PullRequest {
        PullRequest {
            number,
            head: PullRequestRef {
                ref_name: head.to_string(),
                sha: String::new(),
                repository: repository("me"),
            },
            base: PullRequestRef {
                ref_name: base.to_string(),
                sha: String::new(),
                repository: repository("me"),
            },
            ..Default::default()
        }
    }

    fn numbers(stack: Vec<&PullRequest>) -> Vec<u64> {
        stack.iter().map(|p| p.number).collect()
    }

    #[test]
    fn finds_the_stack_from_the_bases() {
        let prs = vec![
            pr(3, "c", "b"),
            pr(1, "a", "main"),
            pr(2, "b", "a"),
            pr(9, "other", "main"),
        ];
        assert_eq!(numbers(stack_of(&prs, &prs[3])), [9]);
        for p in &prs[..3] {
            assert_eq!(numbers(stack_of(&prs, p)), [1, 2, 3]);
        }
        let position = position_in(&prs, &prs[2]).unwrap();
        assert_eq!(position.label(), "2 of 3");
        assert_eq!(position.below.map(|p| p.number), Some(1));
        assert_eq!(position.above.map(|p| p.number), Some(3));
        assert_eq!(position_in(&prs, &prs[3]), None);
    }

    #[test]
    fn a_fork_head_is_not_part_of_a_stack() {
        let mut from_fork = pr(2, "b", "a");
        from_fork.head.repository = repository("someone");
        let prs = vec![pr(1, "a", "main"), from_fork];
        assert_eq!(numbers(stack_of(&prs, &prs[0])), [1]);
        assert_eq!(numbers(stack_of(&prs, &prs[1])), [2]);
    }

    #[test]
    fn a_cycle_of_bases_ends() {
        let prs = vec![pr(1, "a", "b"), pr(2, "b", "a")];
        assert_eq!(stack_of(&prs, &prs[0]).len(), 2);
    }

    #[test]
    fn orders_branches_from_the_commits() {
        let order = ["c3", "c2", "c1"].map(String::from);
        let tips = HashMap::from([
            ("c1".to_string(), vec!["a".to_string(), "a2".to_string()]),
            ("c2".to_string(), vec!["b".to_string()]),
            ("c3".to_string(), vec!["same-as-head".to_string()]),
        ]);
        assert_eq!(
            order_branches(&order, &tips, "top", Some("c3")),
            ["a", "b", "top"]
        );
    }
}
