//! Pairing the entries of two versions of an archive.

use std::collections::{BTreeMap, HashMap};

use corvene_models::{ArchiveEntryChange, ArchiveEntryKind, ArchiveEntryMeta, ArchiveEntryStatus};

/// Every entry of both versions (each sorted by path), sorted by path. A
/// path in both is unchanged, changed only in its metadata, or modified;
/// a file only in the old version whose exact contents (size and CRC-32)
/// appear in exactly one file only in the new version, and nowhere else on
/// either side, is a rename. Empty files never pair: they all look alike.
pub(crate) fn compare(
    old: &[(String, ArchiveEntryMeta)],
    new: &[(String, ArchiveEntryMeta)],
) -> Vec<ArchiveEntryChange> {
    let old_map: BTreeMap<&str, &ArchiveEntryMeta> =
        old.iter().map(|(p, m)| (p.as_str(), m)).collect();
    let new_map: BTreeMap<&str, &ArchiveEntryMeta> =
        new.iter().map(|(p, m)| (p.as_str(), m)).collect();
    let mut out = Vec::with_capacity(old.len().max(new.len()));
    let mut deleted = Vec::new();
    let mut added = Vec::new();
    for (path, meta) in &old_map {
        match new_map.get(path) {
            Some(now) => {
                let status = if !meta.same_contents(now) {
                    ArchiveEntryStatus::Modified
                } else if meta.same_metadata(now) {
                    ArchiveEntryStatus::Unchanged
                } else {
                    ArchiveEntryStatus::MetadataOnly
                };
                out.push(ArchiveEntryChange {
                    path: path.to_string(),
                    status,
                    old_path: None,
                    old: Some((*meta).clone()),
                    new: Some((*now).clone()),
                });
            }
            None => deleted.push((*path, *meta)),
        }
    }
    for (path, meta) in &new_map {
        if !old_map.contains_key(path) {
            added.push((*path, *meta));
        }
    }

    type Key = (u64, u32);
    let renamable = |m: &ArchiveEntryMeta| m.kind == ArchiveEntryKind::File && m.size > 0;
    let key = |m: &ArchiveEntryMeta| -> Key { (m.size, m.crc32) };
    let mut uses: HashMap<Key, (usize, usize)> = HashMap::new();
    for (_, m) in old.iter().chain(new.iter()) {
        if renamable(m) {
            uses.entry(key(m)).or_default().0 += 1;
        }
    }
    for (_, m) in &deleted {
        if renamable(m) {
            uses.entry(key(m)).or_default().1 += 1;
        }
    }
    // a key seen exactly twice overall, once among the deleted: the other
    // sighting must be the added file
    let mut pair_of: HashMap<Key, &str> = HashMap::new();
    for (path, m) in &deleted {
        if renamable(m) && uses.get(&key(m)) == Some(&(2, 1)) {
            pair_of.insert(key(m), path);
        }
    }
    let mut renamed_old = std::collections::HashSet::new();
    for (path, meta) in &added {
        let paired = renamable(meta)
            .then(|| pair_of.get(&key(meta)))
            .flatten()
            .copied();
        match paired {
            Some(old_path) => {
                renamed_old.insert(old_path);
                out.push(ArchiveEntryChange {
                    path: path.to_string(),
                    status: ArchiveEntryStatus::Renamed,
                    old_path: Some(old_path.to_string()),
                    old: old_map.get(old_path).map(|m| (*m).clone()),
                    new: Some((*meta).clone()),
                });
            }
            None => out.push(ArchiveEntryChange {
                path: path.to_string(),
                status: ArchiveEntryStatus::Added,
                old_path: None,
                old: None,
                new: Some((*meta).clone()),
            }),
        }
    }
    for (path, meta) in deleted {
        if !renamed_old.contains(path) {
            out.push(ArchiveEntryChange {
                path: path.to_string(),
                status: ArchiveEntryStatus::Deleted,
                old_path: None,
                old: Some(meta.clone()),
                new: None,
            });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(size: u64, crc32: u32) -> ArchiveEntryMeta {
        ArchiveEntryMeta {
            kind: ArchiveEntryKind::File,
            size,
            compressed: None,
            crc32,
            mode: Some(0o644),
            mtime: Some(1),
            mtime_naive: false,
            method: None,
            owner: None,
            encrypted: false,
        }
    }

    fn status(changes: &[ArchiveEntryChange], path: &str) -> ArchiveEntryStatus {
        changes.iter().find(|c| c.path == path).unwrap().status
    }

    #[test]
    fn classifies_entries() {
        let mut touched = file(3, 3);
        touched.mtime = Some(2);
        let old = vec![
            ("a".into(), file(1, 1)),
            ("b".into(), file(2, 2)),
            ("c".into(), file(3, 3)),
            ("gone".into(), file(4, 4)),
        ];
        let new = vec![
            ("a".into(), file(1, 1)),
            ("b".into(), file(2, 9)),
            ("c".into(), touched),
            ("fresh".into(), file(5, 5)),
        ];
        let changes = compare(&old, &new);
        assert_eq!(status(&changes, "a"), ArchiveEntryStatus::Unchanged);
        assert_eq!(status(&changes, "b"), ArchiveEntryStatus::Modified);
        assert_eq!(status(&changes, "c"), ArchiveEntryStatus::MetadataOnly);
        assert_eq!(status(&changes, "gone"), ArchiveEntryStatus::Deleted);
        assert_eq!(status(&changes, "fresh"), ArchiveEntryStatus::Added);
        let paths: Vec<_> = changes.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(paths, ["a", "b", "c", "fresh", "gone"]);
    }

    #[test]
    fn pairs_moved_files_by_contents() {
        let old = vec![("old/name.txt".into(), file(7, 7))];
        let new = vec![("new/name.txt".into(), file(7, 7))];
        let changes = compare(&old, &new);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].status, ArchiveEntryStatus::Renamed);
        assert_eq!(changes[0].old_path.as_deref(), Some("old/name.txt"));
    }

    #[test]
    fn leaves_ambiguous_and_empty_moves_unpaired() {
        // two copies of the same contents were added: no rename
        let old = vec![("x".into(), file(7, 7)), ("empty".into(), file(0, 0))];
        let new = vec![
            ("y".into(), file(7, 7)),
            ("z".into(), file(7, 7)),
            ("also-empty".into(), file(0, 0)),
        ];
        let changes = compare(&old, &new);
        assert_eq!(status(&changes, "x"), ArchiveEntryStatus::Deleted);
        assert_eq!(status(&changes, "y"), ArchiveEntryStatus::Added);
        assert_eq!(status(&changes, "z"), ArchiveEntryStatus::Added);
        assert_eq!(status(&changes, "empty"), ArchiveEntryStatus::Deleted);
        assert_eq!(status(&changes, "also-empty"), ArchiveEntryStatus::Added);
    }

    #[test]
    fn a_kept_copy_is_not_a_rename() {
        // the contents still exist at the old path too
        let old = vec![("keep".into(), file(7, 7)), ("moved".into(), file(7, 7))];
        let new = vec![("keep".into(), file(7, 7)), ("copy".into(), file(7, 7))];
        let changes = compare(&old, &new);
        assert_eq!(status(&changes, "copy"), ArchiveEntryStatus::Added);
        assert_eq!(status(&changes, "moved"), ArchiveEntryStatus::Deleted);
    }
}
