//! Corvene `1360-compiled-summary`: one side of a compiled file read as
//! facts and tables of named parts ([`Side`]), and two sides compared into
//! a [`CompiledDiff`]. The readers live with each format's listing
//! (`native`, `class`, `pyc`, `wasm`).

use std::collections::HashMap;

use corvene_models::compiled::{
    CompiledDiff, CompiledFact, CompiledFamily, CompiledGroup, CompiledGroupKind, CompiledRow,
    CompiledStatus,
};

/// Unchanged rows kept per table (the rest are counted).
pub const MAX_UNCHANGED: usize = 5_000;
/// Parts read per table and side.
pub const MAX_ENTRIES: usize = 50_000;

/// A header value of one side.
#[derive(Clone, Debug, PartialEq)]
pub struct Fact {
    pub label: String,
    pub value: String,
    pub number: Option<u64>,
    pub bytes: bool,
}

/// A named part of one side.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// What matches the part across versions (often the name).
    pub key: String,
    pub name: String,
    pub detail: String,
    pub size: Option<u64>,
    /// A hash of the contents (0: not known).
    pub body: u64,
}

impl Entry {
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        Entry {
            key: name.clone(),
            name,
            detail: String::new(),
            size: None,
            body: 0,
        }
    }

    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = key.into();
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    pub fn size(mut self, size: u64) -> Self {
        self.size = Some(size);
        self
    }

    pub fn body(mut self, body: u64) -> Self {
        self.body = body;
        self
    }
}

/// One side of a compiled file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Side {
    pub title: String,
    pub facts: Vec<Fact>,
    pub groups: Vec<(CompiledGroupKind, Vec<Entry>)>,
}

impl Side {
    pub fn fact(&mut self, label: impl Into<String>, value: impl Into<String>) {
        let value = value.into();
        if value.is_empty() {
            return;
        }
        self.facts.push(Fact {
            label: label.into(),
            value,
            number: None,
            bytes: false,
        });
    }

    pub fn count(&mut self, label: impl Into<String>, n: u64) {
        self.facts.push(Fact {
            label: label.into(),
            value: crate::out::thousands(n),
            number: Some(n),
            bytes: false,
        });
    }

    pub fn bytes(&mut self, label: impl Into<String>, n: u64) {
        self.facts.push(Fact {
            label: label.into(),
            value: format!("{} bytes", crate::out::thousands(n)),
            number: Some(n),
            bytes: true,
        });
    }

    /// The table of `kind` (made on first use).
    pub fn group(&mut self, kind: CompiledGroupKind) -> &mut Vec<Entry> {
        let ix = match self.groups.iter().position(|(k, _)| *k == kind) {
            Some(ix) => ix,
            None => {
                self.groups.push((kind, Vec::new()));
                self.groups.len() - 1
            }
        };
        &mut self.groups[ix].1
    }

    /// Adds `entry` to `kind` unless the table is full.
    pub fn push(&mut self, kind: CompiledGroupKind, entry: Entry) {
        let group = self.group(kind);
        if group.len() < MAX_ENTRIES {
            group.push(entry);
        }
    }
}

/// 64-bit FNV-1a, to tell contents apart.
pub fn hash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    // 0 means "not known"
    h.max(1)
}

/// Gives repeated keys a counter (`<lambda>`, `<lambda>#2`).
fn unique_keys(entries: &[Entry]) -> Vec<String> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    entries
        .iter()
        .map(|e| {
            let n = seen.entry(e.key.as_str()).or_insert(0);
            *n += 1;
            if *n == 1 {
                e.key.clone()
            } else {
                format!("{}#{}", e.key, n)
            }
        })
        .collect()
}

/// The facts of both sides, in the new side's order with the old side's
/// own ones placed after their old neighbours.
fn facts(old: &[Fact], new: &[Fact]) -> Vec<CompiledFact> {
    let mut labels: Vec<&str> = new.iter().map(|f| f.label.as_str()).collect();
    let mut after: Option<usize> = None;
    for fact in old {
        match labels.iter().position(|l| *l == fact.label) {
            Some(ix) => after = Some(ix),
            None => {
                let at = after.map_or(0, |ix| ix + 1).min(labels.len());
                labels.insert(at, &fact.label);
                after = Some(at);
            }
        }
    }
    labels
        .into_iter()
        .map(|label| {
            let o = old.iter().find(|f| f.label == label);
            let n = new.iter().find(|f| f.label == label);
            let delta = match (o.and_then(|f| f.number), n.and_then(|f| f.number)) {
                (Some(a), Some(b)) if a != b => Some(b as i64 - a as i64),
                _ => None,
            };
            CompiledFact {
                label: label.to_string(),
                old: o.map(|f| f.value.clone()),
                new: n.map(|f| f.value.clone()),
                delta,
                bytes: o.or(n).is_some_and(|f| f.bytes),
            }
        })
        .collect()
}

fn group(kind: CompiledGroupKind, old: &[Entry], new: &[Entry]) -> CompiledGroup {
    let old_keys = unique_keys(old);
    let new_keys = unique_keys(new);
    let old_at: HashMap<&str, usize> = old_keys
        .iter()
        .enumerate()
        .map(|(ix, k)| (k.as_str(), ix))
        .collect();
    let new_has: std::collections::HashSet<&str> = new_keys.iter().map(String::as_str).collect();
    let mut rows = Vec::new();
    let mut unchanged = 0usize;
    let mut hidden = 0usize;
    for (entry, key) in new.iter().zip(&new_keys) {
        let row = match old_at.get(key.as_str()).map(|ix| &old[*ix]) {
            None => CompiledRow {
                status: CompiledStatus::Added,
                name: entry.name.clone(),
                detail: entry.detail.clone(),
                old_detail: None,
                old_size: None,
                new_size: entry.size,
                body_changed: false,
            },
            Some(before) => {
                let detail_changed = before.detail != entry.detail;
                let size_changed = before.size != entry.size;
                let body_changed = before.body != 0
                    && entry.body != 0
                    && before.body != entry.body
                    && !size_changed;
                let changed = detail_changed || size_changed || body_changed;
                if !changed {
                    unchanged += 1;
                    if unchanged > MAX_UNCHANGED {
                        hidden += 1;
                        continue;
                    }
                }
                CompiledRow {
                    status: if changed {
                        CompiledStatus::Changed
                    } else {
                        CompiledStatus::Unchanged
                    },
                    name: entry.name.clone(),
                    detail: entry.detail.clone(),
                    old_detail: detail_changed.then(|| before.detail.clone()),
                    old_size: before.size,
                    new_size: entry.size,
                    body_changed,
                }
            }
        };
        rows.push(row);
    }
    for (entry, key) in old.iter().zip(&old_keys) {
        if !new_has.contains(key.as_str()) {
            rows.push(CompiledRow {
                status: CompiledStatus::Removed,
                name: entry.name.clone(),
                detail: entry.detail.clone(),
                old_detail: None,
                old_size: entry.size,
                new_size: None,
                body_changed: false,
            });
        }
    }
    CompiledGroup {
        kind,
        rows,
        unchanged_hidden: hidden,
        old_count: old.len(),
        new_count: new.len(),
    }
}

/// Both sides compared (`None` for a side that does not exist).
pub fn compare(family: CompiledFamily, old: Option<&Side>, new: Option<&Side>) -> CompiledDiff {
    let empty = Side::default();
    let (o, n) = (old.unwrap_or(&empty), new.unwrap_or(&empty));
    let mut kinds: Vec<CompiledGroupKind> = Vec::new();
    for (kind, _) in n.groups.iter().chain(&o.groups) {
        if !kinds.contains(kind) {
            kinds.push(*kind);
        }
    }
    let entries = |side: &Side, kind| {
        side.groups
            .iter()
            .find(|(k, _)| *k == kind)
            .map(|(_, e)| e.clone())
            .unwrap_or_default()
    };
    let groups = kinds
        .into_iter()
        .map(|kind| group(kind, &entries(o, kind), &entries(n, kind)))
        .collect();
    CompiledDiff {
        family,
        old_title: old.map(|s| s.title.clone()),
        new_title: new.map(|s| s.title.clone()),
        facts: facts(&o.facts, &n.facts),
        groups,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_marked() {
        let mut old = Side::default();
        old.fact("Format", "ELF");
        old.bytes("Size", 100);
        old.push(CompiledGroupKind::Sections, Entry::new(".text").size(10));
        old.push(CompiledGroupKind::Sections, Entry::new(".data").size(4));
        old.push(
            CompiledGroupKind::Sections,
            Entry::new(".rodata").size(4).body(5),
        );
        let mut new = Side::default();
        new.fact("Format", "ELF");
        new.bytes("Size", 120);
        new.fact("Build ID", "ab");
        new.push(CompiledGroupKind::Sections, Entry::new(".text").size(12));
        new.push(CompiledGroupKind::Sections, Entry::new(".bss").size(8));
        new.push(
            CompiledGroupKind::Sections,
            Entry::new(".rodata").size(4).body(6),
        );
        let diff = compare(CompiledFamily::Native, Some(&old), Some(&new));
        let size = diff.facts.iter().find(|f| f.label == "Size").unwrap();
        assert_eq!(size.delta, Some(20));
        assert_eq!(
            diff.facts
                .iter()
                .find(|f| f.label == "Build ID")
                .unwrap()
                .status(),
            CompiledStatus::Added
        );
        let rows = &diff.groups[0].rows;
        let status = |name: &str| rows.iter().find(|r| r.name == name).unwrap().status;
        assert_eq!(status(".text"), CompiledStatus::Changed);
        assert_eq!(status(".bss"), CompiledStatus::Added);
        assert_eq!(status(".data"), CompiledStatus::Removed);
        assert!(
            rows.iter()
                .find(|r| r.name == ".rodata")
                .unwrap()
                .body_changed
        );
        assert_eq!(diff.counts(), [1, 1, 2]);
    }

    #[test]
    fn repeated_names_match_in_order() {
        let mut old = Side::default();
        old.push(CompiledGroupKind::CodeObjects, Entry::new("<lambda>"));
        let mut new = Side::default();
        new.push(CompiledGroupKind::CodeObjects, Entry::new("<lambda>"));
        new.push(CompiledGroupKind::CodeObjects, Entry::new("<lambda>"));
        let diff = compare(CompiledFamily::PythonBytecode, Some(&old), Some(&new));
        assert_eq!(diff.counts(), [1, 0, 0]);
    }
}
