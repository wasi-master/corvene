//! Corvene (flag `1222-push-target-guard`, desktop/desktop#21021): a branch
//! whose upstream has another name (`issue-134` tracking `origin/main`)
//! pushes there. GHD pushes `<branch>:<upstreamWithoutRemote>`
//! (`lib/git/push.ts`, `performPush` in `app-store.ts`) without asking, so
//! such a branch looks published while Push lands its commits on main; only
//! the button's "Push origin/main" (`ui/app.tsx`, `upstreamWithoutRemote
//! !== name`) hints at it. Such a branch is easily made by accident: Create
//! a Branch based on the default branch while only `origin/main` exists
//! (`findDefaultBranch`'s remote hit) runs `git branch issue-134
//! origin/main`, which git's `branch.autoSetupMerge` makes track it.
//!
//! With the flag on, the toolbar and the branch list show the real target
//! ([`Dispatcher::upstream_mismatch_in`]), Push asks first when the target is
//! the remote's default branch (`Popup::ConfirmPushToUpstream`), and a new
//! branch started from a differently named remote branch is created with
//! `--no-track` ([`Dispatcher::create_branch_no_track`]).

use corvene_models::{Branch, BranchKind};

use crate::dispatcher::Dispatcher;
use crate::host::Host;
use crate::state::{AppState, Popup};

/// A local branch whose upstream has another name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpstreamMismatch {
    /// The local branch (`issue-134`).
    pub branch: String,
    /// Its upstream, short (`origin/main`).
    pub upstream: String,
    /// The upstream's branch without its remote (`main`).
    pub remote_branch: String,
    /// The upstream is the remote's default branch.
    pub default: bool,
}

impl UpstreamMismatch {
    /// The tooltip of the push button and the branch list's arrow.
    pub fn describe(&self) -> String {
        if self.default {
            format!(
                "{} tracks {}, the remote's default branch: Push asks before sending its commits there.",
                self.branch, self.upstream
            )
        } else {
            format!(
                "{} tracks {}: Push sends its commits to {}.",
                self.branch, self.upstream, self.remote_branch
            )
        }
    }
}

/// `branch`'s mismatch, when it has one; `default_upstream` is the remote
/// default branch's short name (`origin/main`).
pub fn mismatch_of(branch: &Branch, default_upstream: Option<&str>) -> Option<UpstreamMismatch> {
    if branch.kind != BranchKind::Local {
        return None;
    }
    let remote_branch = branch.upstream_without_remote()?;
    if remote_branch == branch.name {
        return None;
    }
    let upstream = branch.upstream_short()?.to_string();
    Some(UpstreamMismatch {
        branch: branch.name.clone(),
        default: default_upstream == Some(upstream.as_str()),
        remote_branch: remote_branch.to_string(),
        upstream,
    })
}

impl Dispatcher {
    pub fn push_target_guard(s: &AppState) -> bool {
        s.flags.bool(crate::flags::ids::PUSH_TARGET_GUARD)
    }

    /// The remote default branch of repository `id` as `<remote>/<branch>`:
    /// the default branch's upstream, or the default branch itself when it
    /// is a remote branch (`findDefaultBranch`).
    pub fn default_upstream_in(s: &AppState, id: u64) -> Option<String> {
        let rs = s.repo_states.get(&id)?;
        let name = rs.default_branch.as_deref()?;
        let info = rs.info.as_ref()?;
        let branch = info
            .branches
            .iter()
            .find(|b| b.name == name && b.kind == BranchKind::Local)
            .or_else(|| info.branches.iter().find(|b| b.name == name))?;
        match branch.kind {
            BranchKind::Local => branch.upstream_short().map(str::to_string),
            BranchKind::Remote => Some(branch.name.clone()),
        }
    }

    /// The current branch's [`UpstreamMismatch`] (flag or not: GHD names
    /// the upstream on the push button too).
    pub fn upstream_mismatch_in(s: &AppState, id: u64) -> Option<UpstreamMismatch> {
        let branch = s.repo_states.get(&id)?.info.as_ref()?.current_branch()?;
        mismatch_of(branch, Self::default_upstream_in(s, id).as_deref())
    }

    /// `1222-push-target-guard`: before a push of the current branch to its
    /// differently named upstream, when that is the remote's default branch,
    /// asks first. `true` when the push should wait for the answer.
    pub(crate) fn push_needs_target_confirmation(
        id: u64,
        branch: &Branch,
        force_with_lease: bool,
        up_to: Option<String>,
        cx: &mut dyn Host,
    ) -> bool {
        let s = Self::state(cx).read(cx);
        if !Self::push_target_guard(s) {
            return false;
        }
        let Some(mismatch) =
            mismatch_of(branch, Self::default_upstream_in(s, id).as_deref()).filter(|m| m.default)
        else {
            return false;
        };
        let confirmed = s
            .repo_states
            .get(&id)
            .and_then(|r| r.push_target_confirmed.as_ref())
            == Some(&mismatch.upstream);
        if confirmed {
            // one push per confirmation
            Self::state(cx).update(cx, |s, _| s.repo_state_mut(id).push_target_confirmed = None);
            return false;
        }
        Self::show_popup(
            Popup::ConfirmPushToUpstream {
                repo: id,
                branch: mismatch.branch,
                upstream: mismatch.upstream,
                remote_branch: mismatch.remote_branch,
                force_with_lease,
                up_to,
            },
            cx,
        );
        true
    }

    /// Confirm Push › Push to main: the push the dialog held back.
    pub fn confirm_push_to_upstream(
        id: u64,
        branch: String,
        upstream: String,
        force_with_lease: bool,
        up_to: Option<String>,
        cx: &mut dyn Host,
    ) {
        Self::close_popup(cx);
        Self::state(cx).update(cx, |s, _| {
            s.repo_state_mut(id).push_target_confirmed = Some(upstream)
        });
        Self::push_inner(id, force_with_lease, Some(branch), up_to, |_, _| {}, cx);
    }

    /// Confirm Push › Publish branch: drops the upstream and publishes the
    /// branch under its own name (`git push --set-upstream origin issue-134`).
    pub fn publish_under_own_name(id: u64, branch: String, cx: &mut dyn Host) {
        Self::close_popup(cx);
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let branch_done = branch.clone();
        let task = cx
            .background_executor()
            .spawn(async move { corvene_git::unset_upstream(git, &workdir, &branch) });
        cx.spawn(async move |cx: &mut crate::host::AsyncCtx| {
            let result = task.await;
            cx.update(|cx| match result {
                Ok(()) => {
                    // the push reads the upstream from the state: drop it
                    // there too, so it publishes rather than push to it
                    Self::state(cx).update(cx, |s, cx| {
                        if let Some(info) = s.repo_state_mut(id).info.as_mut() {
                            for b in info.branches.iter_mut().filter(|b| b.name == branch_done) {
                                b.upstream = None;
                            }
                            if let corvene_models::Tip::Valid { branch } = &mut info.tip
                                && branch.name == branch_done
                            {
                                branch.upstream = None;
                            }
                        }
                        cx.notify();
                    });
                    Self::push(id, false, Some(branch_done), cx);
                }
                Err(err) => Self::show_error("Could not publish branch", &err, cx),
            });
        })
        .detach();
    }

    /// `1222-push-target-guard`: a branch started from a remote branch of
    /// another name does not track it (GHD only passes `--no-track` for the
    /// upstream default branch of a fork).
    pub(crate) fn create_branch_no_track(
        id: u64,
        name: &str,
        start_point: Option<&str>,
        cx: &mut dyn Host,
    ) -> bool {
        let s = Self::state(cx).read(cx);
        // GHD `_createBranch`: never tracks the upstream's default branch
        if start_point.is_some_and(|start| {
            s.upstream_default_branch_of(id)
                .is_some_and(|b| b.name == start)
        }) {
            return true;
        }
        if !Self::push_target_guard(s) {
            return false;
        }
        let Some(start) = start_point else {
            return false;
        };
        let Some(info) = s.repo_states.get(&id).and_then(|r| r.info.as_ref()) else {
            return false;
        };
        // a local branch of that name wins, as in git's rev parsing
        if info
            .branches
            .iter()
            .any(|b| b.kind == BranchKind::Local && b.name == start)
        {
            return false;
        }
        info.branches
            .iter()
            .find(|b| b.kind == BranchKind::Remote && b.name == start)
            .is_some_and(|b| b.name_without_remote() != name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(name: &str, upstream: Option<&str>) -> Branch {
        Branch {
            name: name.into(),
            kind: BranchKind::Local,
            full_name: format!("refs/heads/{name}"),
            tip: None,
            upstream: upstream.map(|u| format!("refs/remotes/{u}")),
            tip_time: None,
            tip_author: None,
            remote_name: None,
        }
    }

    #[test]
    fn a_differently_named_upstream_is_a_mismatch() {
        let m = mismatch_of(
            &local("issue-134", Some("origin/main")),
            Some("origin/main"),
        )
        .unwrap();
        assert_eq!(m.upstream, "origin/main");
        assert_eq!(m.remote_branch, "main");
        assert!(m.default);
        let m = mismatch_of(&local("pr/12", Some("fork/feature")), Some("origin/main")).unwrap();
        assert!(!m.default);
    }

    #[test]
    fn same_names_and_missing_upstreams_are_not() {
        assert_eq!(
            mismatch_of(&local("main", Some("origin/main")), Some("origin/main")),
            None
        );
        assert_eq!(
            mismatch_of(&local("feature", None), Some("origin/main")),
            None
        );
    }
}
