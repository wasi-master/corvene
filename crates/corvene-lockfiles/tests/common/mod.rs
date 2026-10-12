//! Shared test helpers.

#![allow(dead_code)]

use corvene_models::LockfileDiff;
use corvene_models::lockfile::{ChangeKind, PackageChange};

/// Diffs two texts of `path`, panicking when no diff comes back.
pub fn diff(path: &str, old: &str, new: &str) -> LockfileDiff {
    corvene_lockfiles::lockfile_diff(path, Some(old), Some(new)).expect("diff")
}

/// The single row for `name`.
pub fn row<'a>(d: &'a LockfileDiff, name: &str) -> &'a PackageChange {
    let rows: Vec<_> = d.changes.iter().filter(|c| c.name == name).collect();
    assert_eq!(rows.len(), 1, "rows for {name}: {:#?}", d.changes);
    rows[0]
}

/// Names of the rows of `kind`, in order.
pub fn names(d: &LockfileDiff, kind: ChangeKind) -> Vec<&str> {
    d.group(kind).map(|c| c.name.as_str()).collect()
}

/// Whether there is no row for `name`.
pub fn absent(d: &LockfileDiff, name: &str) -> bool {
    d.changes.iter().all(|c| c.name != name)
}
