//! `Podfile.lock`, Swift `Package.resolved`, `Cartfile.resolved`,
//! `mix.lock` and `rebar.lock`.

use std::collections::HashMap;

use corvene_models::lockfile::{Ecosystem, PackageSource, SourceKind};
use serde_json::Value;
use yaml_rust::Yaml;

use crate::model::{DirectFlags, Lock, Pkg};
use crate::text::{Term, js, json, terms, yaml, yget, yhash, ylist, ys};

fn pod_entry(s: &str) -> (String, String) {
    match s.split_once(" (") {
        Some((n, v)) => (
            n.trim().to_string(),
            v.trim_end_matches(')').trim().to_string(),
        ),
        None => (s.trim().to_string(), String::new()),
    }
}

/// `Podfile.lock`.
pub(crate) fn podfile(text: &str) -> Option<Lock> {
    let doc = yaml(text)?;
    let pods = doc["PODS"].as_vec()?;
    let mut lock = Lock::default();
    if let Some(v) = yget(&doc, "COCOAPODS") {
        lock.meta.push(("CocoaPods".into(), v));
    }
    let sums: HashMap<String, String> = yhash(&doc["SPEC CHECKSUMS"])
        .filter_map(|(k, v)| Some((k, ys(v)?)))
        .collect();
    let mut sources: HashMap<String, PackageSource> = HashMap::new();
    for (name, opts) in yhash(&doc["EXTERNAL SOURCES"]) {
        let src = if let Some(g) = yget(opts, ":git") {
            PackageSource {
                kind: SourceKind::Git,
                url: Some(g),
                rev: yget(opts, ":commit").or_else(|| yget(opts, ":tag")),
            }
        } else if let Some(p) = yget(opts, ":path") {
            PackageSource {
                kind: SourceKind::Path,
                url: Some(p),
                rev: None,
            }
        } else if let Some(p) = yget(opts, ":podspec") {
            PackageSource {
                kind: SourceKind::Url,
                url: Some(p),
                rev: None,
            }
        } else {
            continue;
        };
        sources.insert(name, src);
    }
    for (name, opts) in yhash(&doc["CHECKOUT OPTIONS"]) {
        if let Some(g) = yget(opts, ":git") {
            let rev = yget(opts, ":commit").or_else(|| yget(opts, ":tag"));
            sources.insert(
                name,
                PackageSource {
                    kind: SourceKind::Git,
                    url: Some(g),
                    rev,
                },
            );
        }
    }
    for item in pods {
        let (head, deps): (String, Vec<String>) = match item {
            Yaml::String(s) => (s.clone(), Vec::new()),
            Yaml::Hash(h) => {
                let Some((k, v)) = h.iter().next() else {
                    continue;
                };
                let Some(k) = ys(k) else { continue };
                let deps = ylist(v).filter_map(ys).map(|d| pod_entry(&d).0).collect();
                (k, deps)
            }
            _ => continue,
        };
        let (name, version) = pod_entry(&head);
        let root = name.split('/').next().unwrap_or(&name).to_string();
        let mut p = Pkg::new(name, version, Ecosystem::CocoaPods);
        p.integrity = sums.get(&root).cloned();
        p.source = sources.get(&root).cloned();
        p.deps = deps;
        lock.pkgs.push(p);
    }
    lock.direct = Some(HashMap::new());
    for d in ylist(&doc["DEPENDENCIES"]).filter_map(ys) {
        lock.mark_direct(&pod_entry(&d).0, DirectFlags::default());
    }
    lock.apply_direct();
    Some(lock)
}

/// Swift `Package.resolved` v1 (`object.pins`) and v2 / v3 (`pins`).
pub(crate) fn swift_resolved(text: &str) -> Option<Lock> {
    let doc = json(text)?;
    let pins = doc
        .get("pins")
        .or_else(|| doc.get("object").and_then(|o| o.get("pins")))
        .and_then(Value::as_array)?;
    let mut lock = Lock {
        format_version: js(&doc, "version"),
        ..Default::default()
    };
    for pin in pins {
        let Some(name) = js(pin, "identity").or_else(|| js(pin, "package")) else {
            continue;
        };
        let url = js(pin, "location").or_else(|| js(pin, "repositoryURL"));
        let state = pin.get("state").cloned().unwrap_or(Value::Null);
        let revision = js(&state, "revision");
        let kind = match js(pin, "kind").as_deref() {
            Some("localSourceControl" | "fileSystem") => SourceKind::Path,
            Some("registry") => SourceKind::Registry,
            _ => SourceKind::Git,
        };
        let mut p;
        if let Some(v) = js(&state, "version") {
            // A tag: the revision is its integrity (a moved tag is a warning).
            p = Pkg::new(name, v, Ecosystem::SwiftPM);
            p.integrity = revision;
            p.source = Some(PackageSource {
                kind,
                url,
                rev: None,
            });
        } else {
            let label = js(&state, "branch")
                .unwrap_or_else(|| revision.as_deref().unwrap_or("").chars().take(12).collect());
            p = Pkg::new(name, label, Ecosystem::SwiftPM);
            p.source = Some(PackageSource {
                kind,
                url,
                rev: revision,
            });
        }
        lock.pkgs.push(p);
    }
    Some(lock)
}

/// `Cartfile.resolved`.
pub(crate) fn cartfile(text: &str) -> Option<Lock> {
    let mut lock = Lock::default();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = t
            .split('"')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        let [kind, name, version] = parts[..] else {
            return None;
        };
        let (src, name) = match kind {
            "github" => {
                let url = if name.starts_with("http") {
                    name.to_string()
                } else {
                    format!("https://github.com/{name}")
                };
                (
                    PackageSource {
                        kind: SourceKind::Git,
                        url: Some(url),
                        rev: None,
                    },
                    name,
                )
            }
            "git" => (
                PackageSource {
                    kind: SourceKind::Git,
                    url: Some(name.to_string()),
                    rev: None,
                },
                name,
            ),
            "binary" => (
                PackageSource {
                    kind: SourceKind::Url,
                    url: Some(name.to_string()),
                    rev: None,
                },
                name,
            ),
            _ => return None,
        };
        let mut p = Pkg::new(name, version, Ecosystem::Carthage);
        if !version.contains('.')
            && version.len() >= 7
            && version.bytes().all(|b| b.is_ascii_hexdigit())
        {
            let mut src = src;
            src.rev = Some(version.to_string());
            p.source = Some(src);
        } else {
            p.source = Some(src);
        }
        lock.pkgs.push(p);
    }
    Some(lock)
}

fn atom_or_str(t: &Term) -> Option<String> {
    t.text().map(str::to_string)
}

/// `mix.lock`.
pub(crate) fn mix(text: &str) -> Option<Lock> {
    let all = terms(text)?;
    let Some(Term::Map(entries)) = all.into_iter().next() else {
        return None;
    };
    let mut lock = Lock::default();
    for (key, val) in entries {
        let Some(name) = key.text().map(str::to_string) else {
            continue;
        };
        let items = val.items();
        match items.first().and_then(Term::text) {
            Some("hex") => {
                let version = items.get(2).and_then(atom_or_str).unwrap_or_default();
                let mut p = Pkg::new(name, version, Ecosystem::Hex);
                p.integrity = items
                    .get(7)
                    .and_then(atom_or_str)
                    .or_else(|| items.get(3).and_then(atom_or_str));
                if let Some(repo) = items.get(6).and_then(atom_or_str).filter(|r| r != "hexpm") {
                    p.source = Some(PackageSource {
                        kind: SourceKind::Registry,
                        url: Some(repo),
                        rev: None,
                    });
                }
                p.deps = items
                    .get(5)
                    .map(|d| {
                        d.items()
                            .iter()
                            .filter_map(|t| t.items().first().and_then(atom_or_str))
                            .collect()
                    })
                    .unwrap_or_default();
                lock.pkgs.push(p);
            }
            Some("git") => {
                let url = items.get(1).and_then(atom_or_str);
                let rev = items.get(2).and_then(atom_or_str);
                let label = items
                    .get(3)
                    .and_then(|o| o.kw("tag").or_else(|| o.kw("branch")).and_then(atom_or_str));
                let version = label
                    .unwrap_or_else(|| rev.as_deref().unwrap_or("").chars().take(12).collect());
                let mut p = Pkg::new(name, version, Ecosystem::Hex);
                p.source = Some(PackageSource {
                    kind: SourceKind::Git,
                    url,
                    rev,
                });
                lock.pkgs.push(p);
            }
            Some("path") => {
                let mut p = Pkg::new(name, "", Ecosystem::Hex);
                p.source = Some(PackageSource {
                    kind: SourceKind::Path,
                    url: items.get(1).and_then(atom_or_str),
                    rev: None,
                });
                lock.pkgs.push(p);
            }
            _ => {}
        }
    }
    Some(lock)
}

/// `rebar.lock` (versioned `{"1.2.0", [...]}` and the old bare list).
pub(crate) fn rebar(text: &str) -> Option<Lock> {
    let all = terms(text)?;
    let mut it = all.iter();
    let first = it.next()?;
    let (format_version, deps) = match first {
        Term::Tuple(v) if v.len() == 2 => (v[0].text().map(str::to_string), &v[1]),
        Term::List(_) => (None, first),
        _ => return None,
    };
    let mut lock = Lock {
        format_version,
        ..Default::default()
    };
    let mut hashes: HashMap<String, String> = HashMap::new();
    for t in it {
        for (key, list) in t.items().iter().filter_map(|e| match e {
            Term::Tuple(v) if v.len() == 2 => Some((v[0].text()?, &v[1])),
            _ => None,
        }) {
            if key == "pkg_hash_ext" || key == "pkg_hash" {
                for pair in list.items() {
                    if let (Some(n), Some(h)) = (
                        pair.items().first().and_then(Term::text),
                        pair.items().get(1).and_then(Term::text),
                    ) {
                        hashes.insert(n.to_string(), h.to_string());
                    }
                }
            }
        }
    }
    for d in deps.items() {
        let v = d.items();
        let Some(name) = v.first().and_then(Term::text) else {
            continue;
        };
        let spec = v.get(1).map(Term::items).unwrap_or(&[]);
        let level = v.get(2).and_then(Term::text).unwrap_or("0");
        let mut p = match spec.first().and_then(Term::text) {
            Some("pkg") => {
                let version = spec.get(2).and_then(Term::text).unwrap_or("");
                let real = spec.get(1).and_then(Term::text).unwrap_or(name);
                let mut p = Pkg::new(real, version, Ecosystem::Hex);
                p.integrity = hashes.get(name).cloned();
                p
            }
            Some("git") => {
                let url = spec.get(1).and_then(Term::text).map(str::to_string);
                let rev = spec
                    .get(2)
                    .and_then(|r| r.items().get(1))
                    .and_then(Term::text)
                    .map(str::to_string);
                let version: String = rev.as_deref().unwrap_or("").chars().take(12).collect();
                let mut p = Pkg::new(name, version, Ecosystem::Hex);
                p.source = Some(PackageSource {
                    kind: SourceKind::Git,
                    url,
                    rev,
                });
                p
            }
            _ => continue,
        };
        p.direct = Some(level == "0");
        lock.pkgs.push(p);
    }
    Some(lock)
}
