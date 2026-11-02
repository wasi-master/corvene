//! Corvene `1221-stacked-branch-refs` (desktop/desktop#21256): other local
//! branches stacked on the current one.
//!
//! History marks the current branch's commits that the default branch does
//! not have (`git log <default>..HEAD`): a commit another local branch
//! points at gets a separator line above it and the branch's label, and the
//! row where the default branch begins gets one with the default branch's
//! name, so a squash or reorder never crosses a branch unnoticed
//! ([`StackedRefs`], read with the refresh). GHD's commit list
//! (`ui/history/commit-list-item.tsx`) marks no branch.
//!
//! A squash, reorder, fixup squash or message edit that replays commits
//! other local branches point at first asks whether to move those branches
//! with their rewritten commits (`Popup::WarnStackedBranches`); moving them
//! adds `update-ref` lines to the rebase's todo list
//! ([`corvene_git::with_update_refs`]), what `git rebase --update-refs`
//! does. With `rebase.updateRefs` set in git's config they move without
//! asking. GHD (`lib/git/squash.ts`, `lib/git/reorder.ts`) writes its own
//! todo list, which leaves those branches on the old commits and drops the
//! `update-ref` lines `rebase.updateRefs` would add.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use corvene_git::UpdateRefs;
use corvene_models::{BranchKind, RepositoryInfo, Tip};

use crate::dispatcher::Dispatcher;
use crate::host::Host;
use crate::state::Popup;

/// More commits than this between the default branch and HEAD mark
/// nothing: that is not a stack of branches.
pub const STACK_LIMIT: usize = 1000;

/// History's marks for the current branch's commits that the default
/// branch does not have.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StackedRefs {
    /// The default branch's name, the boundary's label.
    pub base: String,
    /// The commits of `<base>..HEAD`.
    pub range: HashSet<String>,
    /// Commits of the range other local branches point at, with their
    /// names (sorted).
    pub tips: HashMap<String, Vec<String>>,
    /// The commits of [`Self::range`], newest first
    /// (`356-stacked-pull-requests` orders a stack with it).
    pub order: Vec<String>,
    /// The first History row of the default branch below the range
    /// ([`corvene_git::StackRange::boundary`]).
    pub boundary: Option<String>,
    /// The default branch points at [`Self::boundary`] (else HEAD's branch
    /// left it there and it moved on).
    pub base_at_boundary: bool,
}

/// Read in the refresh, with the branches and the default branch it found.
/// `None` on the default branch itself, on an unborn branch, or when the
/// range is empty or longer than [`STACK_LIMIT`].
pub(crate) fn read(
    workdir: &Path,
    info: &RepositoryInfo,
    default_branch: Option<&str>,
) -> Option<StackedRefs> {
    if matches!(info.tip, Tip::Unborn { .. } | Tip::Unknown) {
        return None;
    }
    let name = default_branch?;
    let base = info
        .branches
        .iter()
        .filter(|b| b.name == name)
        .min_by_key(|b| b.kind != BranchKind::Local)?;
    let current = info.current_branch().map(|b| b.full_name.as_str());
    if current == Some(base.full_name.as_str()) {
        return None;
    }
    let range = corvene_git::stack_range(workdir, &base.full_name, STACK_LIMIT).ok()??;
    if range.shas.is_empty() {
        return None;
    }
    let order = range.shas;
    let shas: HashSet<String> = order.iter().cloned().collect();
    let mut tips: HashMap<String, Vec<String>> = HashMap::new();
    for b in &info.branches {
        if b.kind != BranchKind::Local || Some(b.full_name.as_str()) == current {
            continue;
        }
        if let Some(tip) = b.tip.as_ref().filter(|t| shas.contains(*t)) {
            tips.entry(tip.clone()).or_default().push(b.name.clone());
        }
    }
    for names in tips.values_mut() {
        names.sort();
    }
    Some(StackedRefs {
        base: base.name.clone(),
        base_at_boundary: range.boundary.is_some() && base.tip == range.boundary,
        boundary: range.boundary,
        range: shas,
        order,
        tips,
    })
}

/// What a rewrite does with other local branches among its commits.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum StackedBranches {
    /// Not known yet: look for them, and ask when there are some.
    #[default]
    Ask,
    /// The `update-ref` lines to add (none: the branches stay).
    Decided(UpdateRefs),
}

/// The rewrite `Popup::WarnStackedBranches` holds until it is answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StackedOp {
    Squash {
        to_squash: Vec<String>,
        onto: String,
        message: String,
        force_push_checked: bool,
    },
    Reorder {
        to_move: Vec<String>,
        before: Option<String>,
        force_push_checked: bool,
    },
    /// `799-fixup-commits`
    Autosquash { force_push_checked: bool },
    /// `892-edit-commit-message`
    Reword { sha: String, message: String },
}

impl Dispatcher {
    /// Before a rewrite replays `last_retained..HEAD` (the whole branch
    /// for `None`): the `update-ref` lines to run it with, or `None` when
    /// the operation stops here because the other branches are looked for
    /// in the background (it is started again with the answer) or the
    /// warning asks about them.
    pub(crate) fn stacked_update_refs(
        id: u64,
        op: StackedOp,
        choice: StackedBranches,
        last_retained: Option<&str>,
        cx: &mut dyn Host,
    ) -> Option<UpdateRefs> {
        if let StackedBranches::Decided(refs) = choice {
            return Some(refs);
        }
        let (git, workdir, candidates) = {
            let s = Self::state(cx).read(cx);
            if !s.flags.bool(crate::flags::ids::STACKED_BRANCH_REFS) {
                return Some(UpdateRefs::new());
            }
            let git = s.git.clone()?;
            let rs = s.repo_states.get(&id)?;
            let info = rs.info.as_ref()?;
            let current = info.current_branch().map(|b| b.full_name.clone());
            // git's `--update-refs` leaves branches checked out anywhere alone
            let checked_out: HashSet<&str> = rs
                .worktrees
                .iter()
                .filter_map(|w| w.branch.as_deref())
                .collect();
            let candidates: Vec<(String, String, String)> = info
                .branches
                .iter()
                .filter(|b| {
                    b.kind == BranchKind::Local
                        && Some(&b.full_name) != current.as_ref()
                        && !checked_out.contains(b.full_name.as_str())
                })
                .filter_map(|b| Some((b.tip.clone()?, b.full_name.clone(), b.name.clone())))
                .collect();
            (git, info.workdir.clone(), candidates)
        };
        if candidates.is_empty() {
            return Some(UpdateRefs::new());
        }
        let range = match last_retained {
            Some(r) => format!("{r}..HEAD"),
            None => "HEAD".to_string(),
        };
        crate::remote::spawn_bg(
            cx,
            move || {
                let replayed: HashSet<String> =
                    corvene_git::commits_in_range(git.clone(), &workdir, &range)
                        .ok()
                        .flatten()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|c| c.sha)
                        .collect();
                let mut refs = UpdateRefs::new();
                let mut names = Vec::new();
                for (tip, full, name) in candidates {
                    if replayed.contains(&tip) {
                        refs.entry(tip).or_default().push(full);
                        names.push(name);
                    }
                }
                let configured = !refs.is_empty()
                    && corvene_git::boolean_config_value(git, &workdir, "rebase.updateRefs", false)
                        .unwrap_or(false);
                (refs, names, configured)
            },
            move |(refs, mut names, configured), cx| {
                if refs.is_empty() || configured {
                    Self::continue_stacked(id, op, refs, cx);
                    return;
                }
                names.sort();
                Self::show_popup(
                    Popup::WarnStackedBranches {
                        repo: id,
                        branches: names,
                        update_refs: refs,
                        op,
                    },
                    cx,
                );
            },
        );
        None
    }

    /// `Popup::WarnStackedBranches` › Continue: run the rewrite, moving the
    /// branches with their commits when `move_branches`.
    pub fn answer_stacked_branches(
        id: u64,
        op: StackedOp,
        update_refs: UpdateRefs,
        move_branches: bool,
        cx: &mut dyn Host,
    ) {
        Self::close_popup(cx);
        let refs = if move_branches {
            update_refs
        } else {
            UpdateRefs::new()
        };
        Self::continue_stacked(id, op, refs, cx);
    }

    fn continue_stacked(id: u64, op: StackedOp, refs: UpdateRefs, cx: &mut dyn Host) {
        let choice = StackedBranches::Decided(refs);
        match op {
            StackedOp::Squash {
                to_squash,
                onto,
                message,
                force_push_checked,
            } => Self::squash_with(id, to_squash, onto, message, force_push_checked, choice, cx),
            StackedOp::Reorder {
                to_move,
                before,
                force_push_checked,
            } => Self::reorder_commits_with(id, to_move, before, force_push_checked, choice, cx),
            StackedOp::Autosquash { force_push_checked } => {
                Self::autosquash_with(id, force_push_checked, choice, cx)
            }
            StackedOp::Reword { sha, message } => {
                Self::edit_commit_message_with(id, sha, message, choice, cx)
            }
        }
    }
}

/// The branches a rewrite with `refs` moves, with the commits they leave
/// (`(full ref, sha)`), for Undo.
pub(crate) fn moved_branches(refs: &UpdateRefs) -> Vec<(String, String)> {
    refs.iter()
        .flat_map(|(sha, names)| names.iter().map(move |r| (r.clone(), sha.clone())))
        .collect()
}
