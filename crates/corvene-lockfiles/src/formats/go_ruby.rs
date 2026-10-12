//! `go.sum` / `go.work.sum` and `Gemfile.lock` / `gems.locked`.

use std::collections::{HashMap, HashSet};

use corvene_models::lockfile::{Ecosystem, PackageSource, SourceKind};

use crate::model::{DirectFlags, Lock, Pkg, hash_set};

/// `go.sum` / `go.work.sum`: module versions with a downloaded zip hash
/// (`/go.mod`-only lines are graph pruning leftovers and are not listed).
pub(crate) fn go_sum(text: &str) -> Option<Lock> {
    let mut lock = Lock::default();
    let mut seen: HashSet<(&str, &str)> = HashSet::new();
    let mut modules: HashSet<&str> = HashSet::new();
    let mut zip: Vec<(&str, &str, &str)> = Vec::new();
    let mut any = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        let mut it = line.split_whitespace();
        let (Some(module), Some(version), Some(hash)) = (it.next(), it.next(), it.next()) else {
            return None;
        };
        if !hash.contains(':') {
            return None;
        }
        any = true;
        modules.insert(module);
        if version.ends_with("/go.mod") {
            continue;
        }
        if seen.insert((module, version)) {
            zip.push((module, version, hash));
        }
    }
    if !any && !text.trim().is_empty() {
        return None;
    }
    for (module, version, hash) in zip {
        let mut p = Pkg::new(module, version, Ecosystem::Go);
        p.integrity = Some(hash.to_string());
        lock.pkgs.push(p);
    }
    Some(lock)
}

/// Splits `1.16.0-arm64-darwin` into the version and platform.
fn gem_version(v: &str) -> (&str, Option<&str>) {
    match v.split_once('-') {
        Some((ver, plat)) if plat.chars().any(|c| c.is_ascii_alphabetic()) => (ver, Some(plat)),
        _ => (v, None),
    }
}

/// `name (version)` to its parts.
fn name_paren(s: &str) -> (&str, Option<&str>) {
    match s.split_once(" (") {
        Some((n, rest)) => (n.trim(), Some(rest.trim_end_matches(')').trim())),
        None => (s.trim(), None),
    }
}

/// `Gemfile.lock` / `gems.locked`.
pub(crate) fn gemfile(text: &str) -> Option<Lock> {
    let mut lock = Lock::default();
    let mut section = "";
    let mut source = PackageSource::default();
    let mut current: Option<usize> = None;
    let mut checksums: HashMap<(String, String), Vec<String>> = HashMap::new();
    let mut seen: HashMap<(String, String), usize> = HashMap::new();
    let mut known = false;
    for line in text.trim_start_matches('\u{feff}').lines() {
        if line.trim().is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let t = line.trim();
        if indent == 0 {
            section = t;
            known |= matches!(t, "GEM" | "GIT" | "PATH" | "DEPENDENCIES" | "PLATFORMS");
            source = match t {
                "GIT" => PackageSource::of(SourceKind::Git),
                "PATH" => PackageSource::of(SourceKind::Path),
                _ => PackageSource::default(),
            };
            current = None;
            continue;
        }
        match section {
            "GEM" | "GIT" | "PATH" | "PLUGIN SOURCE" => {
                if indent == 2 {
                    if let Some((k, v)) = t.split_once(": ") {
                        match k {
                            "remote" => {
                                let default = v.trim_end_matches('/') == "https://rubygems.org";
                                if !(source.kind == SourceKind::Registry && default) {
                                    source.url = Some(v.to_string());
                                }
                            }
                            "revision" => source.rev = Some(v.to_string()),
                            _ => {}
                        }
                    }
                } else if indent == 4 {
                    let (name, ver) = name_paren(t);
                    let (ver, _plat) = gem_version(ver.unwrap_or(""));
                    let key = (name.to_string(), ver.to_string());
                    if let Some(&i) = seen.get(&key) {
                        current = Some(i);
                        continue;
                    }
                    let mut p = Pkg::new(name, ver, Ecosystem::RubyGems);
                    p.source = Some(source.clone());
                    seen.insert(key, lock.pkgs.len());
                    current = Some(lock.pkgs.len());
                    lock.pkgs.push(p);
                } else if indent >= 6
                    && let Some(i) = current
                {
                    let dep = name_paren(t).0.to_string();
                    if !lock.pkgs[i].deps.contains(&dep) {
                        lock.pkgs[i].deps.push(dep);
                    }
                }
            }
            "DEPENDENCIES" => {
                let n = name_paren(t).0.trim_end_matches('!');
                lock.mark_direct(n, DirectFlags::default());
            }
            "CHECKSUMS" => {
                let (rest, sum) = t.rsplit_once(' ').unwrap_or((t, ""));
                let (name, ver) = name_paren(rest);
                let (ver, _) = gem_version(ver.unwrap_or(""));
                if sum.contains('=') {
                    checksums
                        .entry((name.to_string(), ver.to_string()))
                        .or_default()
                        .push(sum.to_string());
                }
            }
            "BUNDLED WITH" => lock.meta.push(("Bundler".into(), t.to_string())),
            "RUBY VERSION" => lock
                .meta
                .push(("Ruby".into(), t.trim_start_matches("ruby ").to_string())),
            _ => {}
        }
    }
    if !known {
        return None;
    }
    for p in &mut lock.pkgs {
        if let Some(sums) = checksums.get(&(p.name.clone(), p.version.clone())) {
            p.integrity = hash_set(sums.iter().map(String::as_str));
        }
        if p.source.as_ref().is_some_and(|s| s.kind == SourceKind::Git) && p.version.is_empty() {
            p.version = p
                .source
                .as_ref()
                .and_then(|s| s.rev.clone())
                .unwrap_or_default();
        }
    }
    lock.apply_direct();
    Some(lock)
}
