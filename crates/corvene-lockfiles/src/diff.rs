//! Pairs the packages of both sides and builds the dependency table.

use std::cmp::Ordering;

use corvene_models::lockfile::{
    ChangeKind, Ecosystem, LockfileDiff, PackageChange, PackageSource, SourceKind,
};

use crate::model::{FastMap, Lock, Pkg, integrity_conflict};
use crate::version;

/// Most "required by" names kept per row.
const MAX_REQUIRED_BY: usize = 20;
/// Above this many version pairs per name, pair in sorted order instead of
/// by nearest version.
const MAX_PAIR_CANDIDATES: usize = 4096;

type Key<'a> = (Ecosystem, &'a str);

/// Distinct entries per name (identical version + source counted once).
fn group(lock: &Lock) -> FastMap<Key<'_>, Vec<&Pkg>> {
    type Ident<'a> = (
        Ecosystem,
        &'a str,
        &'a str,
        Option<(SourceKind, Option<&'a str>)>,
    );
    let mut map: FastMap<Key<'_>, Vec<&Pkg>> = FastMap::default();
    map.reserve(lock.pkgs.len());
    let mut seen: FastMap<Ident<'_>, usize> = FastMap::default();
    seen.reserve(lock.pkgs.len());
    for p in &lock.pkgs {
        if p.name.is_empty() {
            continue;
        }
        let src = p.source.as_ref().map(|s| (s.kind, s.url.as_deref()));
        let ident = (p.eco, p.name.as_str(), p.version.as_str(), src);
        let v = map.entry((p.eco, p.name.as_str())).or_default();
        match seen.get(&ident) {
            // Keep the most direct copy (npm nests the same version).
            Some(&i) => {
                if p.direct == Some(true) && v[i].direct != Some(true) {
                    v[i] = p;
                }
            }
            None => {
                seen.insert(ident, v.len());
                v.push(p);
            }
        }
    }
    map
}

fn reverse_edges(lock: &Lock) -> FastMap<&str, Vec<&str>> {
    let mut map: FastMap<&str, Vec<&str>> = FastMap::default();
    for p in &lock.pkgs {
        for d in &p.deps {
            map.entry(d.as_str()).or_default().push(p.name.as_str());
        }
    }
    map
}

fn same_source(a: &Pkg, b: &Pkg) -> bool {
    match (&a.source, &b.source) {
        (Some(x), Some(y)) => x == y,
        _ => true,
    }
}

/// Pairs left-over versions of one name: nearest versions first.
fn pair_nearest(olds: &[&Pkg], news: &[&Pkg]) -> Vec<(Option<usize>, Option<usize>)> {
    let mut out = Vec::new();
    let mut used_o = vec![false; olds.len()];
    let mut used_n = vec![false; news.len()];
    if olds.len().saturating_mul(news.len()) <= MAX_PAIR_CANDIDATES {
        let mut cands = Vec::with_capacity(olds.len() * news.len());
        for (i, o) in olds.iter().enumerate() {
            for (j, n) in news.iter().enumerate() {
                let same_kind =
                    o.source.as_ref().map(|s| s.kind) == n.source.as_ref().map(|s| s.kind);
                cands.push((version::distance(&o.version, &n.version), !same_kind, i, j));
            }
        }
        cands.sort_unstable();
        for (_, _, i, j) in cands {
            if !used_o[i] && !used_n[j] {
                used_o[i] = true;
                used_n[j] = true;
                out.push((Some(i), Some(j)));
            }
        }
    } else {
        let order = |v: &[&Pkg]| {
            let mut idx: Vec<usize> = (0..v.len()).collect();
            idx.sort_by(|&a, &b| {
                version::compare(&v[a].version, &v[b].version)
                    .unwrap_or_else(|| v[a].version.cmp(&v[b].version))
            });
            idx
        };
        for (i, j) in order(olds).into_iter().zip(order(news)) {
            used_o[i] = true;
            used_n[j] = true;
            out.push((Some(i), Some(j)));
        }
    }
    out.extend(
        (0..olds.len())
            .filter(|&i| !used_o[i])
            .map(|i| (Some(i), None)),
    );
    out.extend(
        (0..news.len())
            .filter(|&j| !used_n[j])
            .map(|j| (None, Some(j))),
    );
    out
}

fn row(old: Option<&Pkg>, new: Option<&Pkg>) -> Option<PackageChange> {
    let cur = new.or(old)?;
    let mut ch = PackageChange {
        name: cur.name.clone(),
        ecosystem: cur.eco,
        old_version: old.map(|p| p.version.clone()),
        new_version: new.map(|p| p.version.clone()),
        direct: cur.direct,
        dev: cur.dev,
        optional: cur.optional,
        peer: cur.peer,
        old_source: old.and_then(|p| p.source.clone()),
        new_source: new.and_then(|p| p.source.clone()),
        ..Default::default()
    };
    match (old, new) {
        (Some(o), Some(n)) => {
            if o.version == n.version {
                let integrity = matches!((&o.integrity, &n.integrity), (Some(a), Some(b)) if integrity_conflict(a, b));
                let integrity = integrity && same_source(o, n);
                if !integrity && same_source(o, n) {
                    return None;
                }
                ch.kind = ChangeKind::Changed;
                ch.integrity_changed = integrity;
            } else {
                ch.kind = match version::compare(&o.version, &n.version) {
                    Some(Ordering::Less) => ChangeKind::Upgraded,
                    Some(Ordering::Greater) => ChangeKind::Downgraded,
                    _ => ChangeKind::Changed,
                };
                ch.bump = Some(version::bump(&o.version, &n.version));
            }
        }
        (None, Some(_)) => ch.kind = ChangeKind::Added,
        (Some(_), None) => ch.kind = ChangeKind::Removed,
        (None, None) => return None,
    }
    // Only show sources that say something: drop a plain default registry
    // on both sides.
    let plain =
        |s: &Option<PackageSource>| s.as_ref().is_none_or(|s| *s == PackageSource::default());
    if plain(&ch.old_source) && plain(&ch.new_source) {
        ch.old_source = None;
        ch.new_source = None;
    }
    Some(ch)
}

/// Builds the table. `old` / `new` are `None` for a missing or unreadable
/// side.
pub(crate) fn diff(
    format: &str,
    eco: Ecosystem,
    old: Option<&Lock>,
    new: Option<&Lock>,
) -> LockfileDiff {
    let empty = Lock::default();
    let o = old.unwrap_or(&empty);
    let n = new.unwrap_or(&empty);
    let og = group(o);
    let mut ng = group(n);
    let old_package_count = og.values().map(Vec::len).sum();
    let new_package_count = ng.values().map(Vec::len).sum();
    let mut changes = Vec::new();
    let mut unchanged = 0usize;

    let mut handle = |olds: &[&Pkg], news: &[&Pkg], changes: &mut Vec<PackageChange>| {
        let mut used_o = vec![false; olds.len()];
        let mut used_n = vec![false; news.len()];
        // Same version: same source first, then any source.
        let mut by_version: FastMap<&str, Vec<usize>> = FastMap::default();
        for (j, np) in news.iter().enumerate() {
            by_version.entry(np.version.as_str()).or_default().push(j);
        }
        for (i, op) in olds.iter().enumerate() {
            let Some(cands) = by_version.get(op.version.as_str()) else {
                continue;
            };
            let hit = cands
                .iter()
                .copied()
                .find(|&j| !used_n[j] && same_source(op, news[j]))
                .or_else(|| cands.iter().copied().find(|&j| !used_n[j]));
            if let Some(j) = hit {
                used_o[i] = true;
                used_n[j] = true;
                match row(Some(op), Some(news[j])) {
                    Some(c) => changes.push(c),
                    None => unchanged += 1,
                }
            }
        }
        let lo: Vec<&Pkg> = (0..olds.len())
            .filter(|&i| !used_o[i])
            .map(|i| olds[i])
            .collect();
        let ln: Vec<&Pkg> = (0..news.len())
            .filter(|&j| !used_n[j])
            .map(|j| news[j])
            .collect();
        for (i, j) in pair_nearest(&lo, &ln) {
            if let Some(c) = row(i.map(|i| lo[i]), j.map(|j| ln[j])) {
                changes.push(c);
            }
        }
    };

    for (key, olds) in &og {
        let news = ng.remove(key).unwrap_or_default();
        handle(olds, &news, &mut changes);
    }
    for news in ng.values() {
        handle(&[], news, &mut changes);
    }

    // Required by (transitive rows only, when edges are recorded).
    if changes.iter().any(|c| c.direct != Some(true)) {
        let rev_old = reverse_edges(o);
        let rev_new = reverse_edges(n);
        for c in changes.iter_mut().filter(|c| c.direct != Some(true)) {
            let rev = if c.kind == ChangeKind::Removed {
                &rev_old
            } else {
                &rev_new
            };
            if let Some(parents) = rev.get(c.name.as_str()) {
                let mut v: Vec<String> = parents
                    .iter()
                    .filter(|p| **p != c.name)
                    .map(|p| p.to_string())
                    .collect();
                v.sort_unstable();
                v.dedup();
                v.truncate(MAX_REQUIRED_BY);
                c.required_by = v;
            }
        }
    }

    let direct_rank = |d: Option<bool>| match d {
        Some(true) => 0,
        None => 1,
        Some(false) => 2,
    };
    changes.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| direct_rank(a.direct).cmp(&direct_rank(b.direct)))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.old_version.cmp(&b.old_version))
            .then_with(|| a.new_version.cmp(&b.new_version))
    });

    let mut d = LockfileDiff {
        format: format.to_string(),
        ecosystem: eco,
        old_format_version: old.and_then(|l| l.format_version.clone()),
        new_format_version: new.and_then(|l| l.format_version.clone()),
        old_package_count,
        new_package_count,
        unchanged,
        changes,
        ..Default::default()
    };
    if let Some(note) = d.format_version_note() {
        d.notes.push(note);
    }
    if let (Some(o), Some(n)) = (old, new) {
        for (k, nv) in &n.meta {
            if let Some((_, ov)) = o.meta.iter().find(|(ok, _)| ok == k)
                && ov != nv
            {
                d.notes.push(format!("{k} {ov} → {nv}"));
            }
        }
    }
    d
}
