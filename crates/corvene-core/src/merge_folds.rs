//! Corvene `1227-fold-merge-commits` (desktop/desktop#20106): History folds
//! the commits a merge brought in under the merge. GitHub Desktop lists them
//! flat (`app/src/ui/history/commit-list.tsx`).
//!
//! The first-parent line from the tip stays visible. Commits only reachable
//! through a merge's other parents belong to that merge, within the commits
//! loaded so far. The list is rebuilt when the loaded commits change, not in
//! render ([`Dispatcher::sync_merge_folds`]).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use corvene_models::Commit;

use crate::Dispatcher;
use crate::host::Host;

/// What the rows draw.
#[derive(Clone, Debug, Default)]
pub struct MergeFoldMarks {
    /// A merge's commits, by the merge's sha.
    children: HashMap<String, Vec<String>>,
    /// The merge a brought-in commit belongs to.
    merge_of: HashMap<String, String>,
    expanded: HashSet<String>,
}

impl MergeFoldMarks {
    /// `(expanded, commits it brought in)` for a merge with any.
    pub fn fold(&self, sha: &str) -> Option<(bool, usize)> {
        let children = self.children.get(sha)?;
        Some((self.expanded.contains(sha), children.len()))
    }

    pub fn is_child(&self, sha: &str) -> bool {
        self.merge_of.contains_key(sha)
    }

    pub fn merge_of(&self, sha: &str) -> Option<&str> {
        self.merge_of.get(sha).map(String::as_str)
    }

    /// Any commit is under a merge: row positions are not `commits` positions.
    pub fn any(&self) -> bool {
        !self.children.is_empty()
    }
}

/// The folded list of a repository's plain history.
#[derive(Clone, Debug, Default)]
pub struct MergeFolds {
    /// The loaded commits it was made from: first sha and count.
    key: (Option<String>, usize),
    pub marks: Arc<MergeFoldMarks>,
    pub commits: Vec<Commit>,
}

/// Each merge on the first-parent line of `commits` (tip first) with the
/// commits only its other parents reach.
fn group(commits: &[Commit]) -> (HashMap<String, Vec<String>>, HashMap<String, String>) {
    let by_sha: HashMap<&str, &Commit> = commits.iter().map(|c| (c.sha.as_str(), c)).collect();
    let mut line: Vec<&Commit> = Vec::new();
    let mut on_line: HashSet<&str> = HashSet::new();
    let mut at = commits.first();
    while let Some(commit) = at {
        if !on_line.insert(commit.sha.as_str()) {
            break;
        }
        line.push(commit);
        at = commit
            .parents
            .first()
            .and_then(|p| by_sha.get(p.as_str()).copied());
    }
    let mut children: HashMap<String, Vec<String>> = HashMap::new();
    let mut merge_of: HashMap<String, String> = HashMap::new();
    for merge in line.iter().filter(|c| c.parents.len() > 1) {
        let mut stack: Vec<&str> = merge.parents[1..].iter().map(String::as_str).collect();
        while let Some(sha) = stack.pop() {
            let Some(commit) = by_sha.get(sha) else {
                continue;
            };
            if on_line.contains(sha) || merge_of.contains_key(sha) {
                continue;
            }
            merge_of.insert(sha.to_string(), merge.sha.clone());
            children
                .entry(merge.sha.clone())
                .or_default()
                .push(sha.to_string());
            stack.extend(commit.parents.iter().map(String::as_str));
        }
    }
    (children, merge_of)
}

fn build(commits: &[Commit], expanded: HashSet<String>) -> MergeFolds {
    let (children, merge_of) = group(commits);
    let shown = commits
        .iter()
        .filter(|c| merge_of.get(&c.sha).is_none_or(|m| expanded.contains(m)))
        .cloned()
        .collect();
    MergeFolds {
        key: (commits.first().map(|c| c.sha.clone()), commits.len()),
        marks: Arc::new(MergeFoldMarks {
            children,
            merge_of,
            expanded,
        }),
        commits: shown,
    }
}

impl Dispatcher {
    /// Keep repository `id`'s folded list in step with its loaded commits.
    pub fn sync_merge_folds(id: u64, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, _| {
            let on = s.flags.bool(crate::flags::ids::FOLD_MERGE_COMMITS);
            let rs = s.repo_state_mut(id);
            if !on {
                rs.merge_folds = None;
                return;
            }
            let key = (rs.commits.first().map(|c| c.sha.clone()), rs.commits.len());
            if rs.merge_folds.as_ref().is_some_and(|f| f.key == key) {
                return;
            }
            let expanded = rs
                .merge_folds
                .as_ref()
                .map(|f| f.marks.expanded.clone())
                .unwrap_or_default();
            rs.merge_folds = Some(build(&rs.commits, expanded));
        });
    }

    /// Fold or unfold merge `sha`. A selected commit that gets hidden gives
    /// its selection to the merge.
    pub fn set_merge_expanded(id: u64, sha: String, expanded: bool, cx: &mut dyn Host) {
        let select_merge = Self::state(cx).update(cx, |s, cx| {
            let rs = s.repo_state_mut(id);
            let Some(folds) = rs.merge_folds.as_ref() else {
                return false;
            };
            let mut open = folds.marks.expanded.clone();
            if expanded {
                open.insert(sha.clone());
            } else {
                open.remove(&sha);
            }
            let folds = build(&rs.commits, open);
            let hidden = !expanded
                && rs
                    .selected_commit
                    .as_deref()
                    .and_then(|c| folds.marks.merge_of(c))
                    == Some(sha.as_str());
            rs.merge_folds = Some(folds);
            cx.notify();
            hidden
        });
        if select_merge {
            Self::select_commits(id, vec![sha], cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(sha: &str, parents: &[&str]) -> Commit {
        let identity = corvene_models::CommitIdentity {
            name: "t".into(),
            email: "t@t".into(),
            seconds: 0,
            offset: 0,
        };
        Commit {
            sha: sha.into(),
            summary: sha.into(),
            body: String::new(),
            author: identity.clone(),
            committer: identity,
            parents: parents.iter().map(|p| p.to_string()).collect(),
            trailers: Vec::new(),
            tags: Vec::new(),
            signature: None,
        }
    }

    /// `m2` merges `b2 ← b1` (forked from `a`), `m1` merges `c1`.
    fn log() -> Vec<Commit> {
        vec![
            commit("tip", &["m2"]),
            commit("m2", &["m1", "b2"]),
            commit("b2", &["b1"]),
            commit("b1", &["a"]),
            commit("m1", &["a", "c1"]),
            commit("c1", &["a"]),
            commit("a", &[]),
        ]
    }

    fn shas(folds: &MergeFolds) -> Vec<&str> {
        folds.commits.iter().map(|c| c.sha.as_str()).collect()
    }

    #[test]
    fn merges_start_folded() {
        let folds = build(&log(), HashSet::new());
        assert_eq!(shas(&folds), ["tip", "m2", "m1", "a"]);
        assert_eq!(folds.marks.fold("m2"), Some((false, 2)));
        assert_eq!(folds.marks.fold("m1"), Some((false, 1)));
        assert_eq!(folds.marks.fold("tip"), None);
        assert_eq!(folds.marks.merge_of("b1"), Some("m2"));
    }

    #[test]
    fn unfolding_shows_a_merges_commits_in_log_order() {
        let folds = build(&log(), HashSet::from(["m2".to_string()]));
        assert_eq!(shas(&folds), ["tip", "m2", "b2", "b1", "m1", "a"]);
        assert!(folds.marks.is_child("b2"));
        assert!(!folds.marks.is_child("m1"));
    }

    #[test]
    fn a_commit_the_first_parent_line_has_stays_on_it() {
        // `c` is on both the line and the merge's side
        let log = vec![
            commit("m", &["c", "b"]),
            commit("b", &["c"]),
            commit("c", &[]),
        ];
        let folds = build(&log, HashSet::new());
        assert_eq!(shas(&folds), ["m", "c"]);
        assert_eq!(folds.marks.fold("m"), Some((false, 1)));
    }

    #[test]
    fn a_merge_side_that_is_not_loaded_yet_is_skipped() {
        let log = vec![commit("m", &["c", "unloaded"]), commit("c", &[])];
        let folds = build(&log, HashSet::new());
        assert_eq!(folds.marks.fold("m"), None);
        assert!(!folds.marks.any());
        assert_eq!(shas(&folds), ["m", "c"]);
    }
}
