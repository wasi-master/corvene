//! `Cargo.lock`, `poetry.lock`, `uv.lock`, `pdm.lock` and `Pipfile.lock`.

use std::collections::HashMap;

use corvene_models::lockfile::{Ecosystem, PackageSource, SourceKind};
use serde_json::Value;

use crate::model::{DirectFlags, Lock, Pkg, git_url_with_rev, hash_set, pep508_name};
use crate::text::{jobj, js, json, toml, ts};

fn packages(doc: &toml::Table) -> impl Iterator<Item = &toml::Table> {
    doc.get("package")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_table)
}

fn tget(t: &toml::Table, key: &str) -> Option<String> {
    ts(t.get(key))
}

/// `Cargo.lock`, versions 1 to 4.
pub(crate) fn cargo(text: &str) -> Option<Lock> {
    let doc = toml(text)?;
    doc.get("package")?;
    let metadata = doc.get("metadata").and_then(toml::Value::as_table);
    let v1_sums: HashMap<&str, &str> = metadata
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| Some((k.strip_prefix("checksum ")?, v.as_str()?)))
        .collect();
    let format_version = tget(&doc, "version")
        .or_else(|| Some(if v1_sums.is_empty() { "2" } else { "1" }.to_string()));
    let mut lock = Lock {
        format_version,
        ..Default::default()
    };
    for t in packages(&doc) {
        let Some(name) = tget(t, "name") else {
            continue;
        };
        let version = tget(t, "version").unwrap_or_default();
        let mut p = Pkg::new(name, version, Ecosystem::Cargo);
        let src = tget(t, "source");
        p.source = Some(match src.as_deref() {
            None => PackageSource::of(SourceKind::Path),
            Some(s) if s.starts_with("git+") => git_url_with_rev(s),
            Some(s) => {
                let reg = s.split_once('+').map_or(s, |(_, u)| u);
                let default = reg == "https://github.com/rust-lang/crates.io-index"
                    || reg == "https://index.crates.io/";
                PackageSource {
                    kind: SourceKind::Registry,
                    url: (!default).then(|| reg.to_string()),
                    rev: None,
                }
            }
        });
        p.integrity = tget(t, "checksum").or_else(|| {
            let key = match &src {
                Some(s) => format!("{} {} ({s})", p.name, p.version),
                None => format!("{} {}", p.name, p.version),
            };
            v1_sums.get(key.as_str()).map(|s| s.to_string())
        });
        p.deps = t
            .get("dependencies")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|d| d.as_str()?.split(' ').next().map(str::to_string))
            .collect();
        lock.pkgs.push(p);
    }
    Some(lock)
}

fn file_hashes(t: &toml::Table, key: &str) -> Vec<String> {
    t.get(key)
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|f| f.as_table().and_then(|f| tget(f, "hash")))
        .collect()
}

/// `poetry.lock`.
pub(crate) fn poetry(text: &str) -> Option<Lock> {
    let doc = toml(text)?;
    let meta = doc.get("metadata").and_then(toml::Value::as_table);
    let mut lock = Lock {
        format_version: meta.and_then(|m| tget(m, "lock-version")),
        ..Default::default()
    };
    for t in packages(&doc) {
        let Some(name) = tget(t, "name") else {
            continue;
        };
        let mut p = Pkg::new(
            name,
            tget(t, "version").unwrap_or_default(),
            Ecosystem::PyPI,
        );
        p.optional = t
            .get("optional")
            .and_then(toml::Value::as_bool)
            .unwrap_or(false);
        p.dev = tget(t, "category").as_deref() == Some("dev");
        let hashes = file_hashes(t, "files");
        p.integrity = hash_set(hashes.iter().map(String::as_str));
        if let Some(src) = t.get("source").and_then(toml::Value::as_table) {
            let kind = match tget(src, "type").as_deref() {
                Some("git") => SourceKind::Git,
                Some("directory" | "file") => SourceKind::Path,
                Some("url") => SourceKind::Url,
                _ => SourceKind::Registry,
            };
            let rev = tget(src, "resolved_reference").or_else(|| tget(src, "reference"));
            p.source = Some(PackageSource {
                kind,
                url: tget(src, "url"),
                rev: rev.filter(|_| kind == SourceKind::Git),
            });
        }
        p.deps = t
            .get("dependencies")
            .and_then(toml::Value::as_table)
            .map(|d| d.keys().cloned().collect())
            .unwrap_or_default();
        lock.pkgs.push(p);
    }
    Some(lock)
}

fn uv_dep_names(v: Option<&toml::Value>) -> Vec<String> {
    v.and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|d| d.as_table().and_then(|d| tget(d, "name")))
        .collect()
}

/// `uv.lock`.
pub(crate) fn uv(text: &str) -> Option<Lock> {
    let doc = toml(text)?;
    let mut lock = Lock {
        format_version: tget(&doc, "version"),
        ..Default::default()
    };
    if let Some(r) = tget(&doc, "revision") {
        lock.meta.push(("uv lock revision".into(), r));
    }
    let mut any_root = false;
    for t in packages(&doc) {
        let Some(name) = tget(t, "name") else {
            continue;
        };
        let mut p = Pkg::new(
            name,
            tget(t, "version").unwrap_or_default(),
            Ecosystem::PyPI,
        );
        let src = t.get("source").and_then(toml::Value::as_table);
        let root = src.is_some_and(|s| {
            ["editable", "virtual"]
                .iter()
                .any(|k| tget(s, k).as_deref() == Some("."))
        });
        p.source = src.map(|s| {
            if let Some(g) = tget(s, "git") {
                git_url_with_rev(&g)
            } else if let Some(r) = tget(s, "registry") {
                let default = r.starts_with("https://pypi.org/");
                PackageSource {
                    kind: SourceKind::Registry,
                    url: (!default).then_some(r),
                    rev: None,
                }
            } else if let Some(u) = tget(s, "url") {
                PackageSource {
                    kind: SourceKind::Url,
                    url: Some(u),
                    rev: None,
                }
            } else {
                let path = ["editable", "virtual", "path", "directory"]
                    .iter()
                    .find_map(|k| tget(s, k));
                PackageSource {
                    kind: SourceKind::Path,
                    url: path,
                    rev: None,
                }
            }
        });
        let mut hashes = file_hashes(t, "wheels");
        if let Some(sd) = t
            .get("sdist")
            .and_then(toml::Value::as_table)
            .and_then(|s| tget(s, "hash"))
        {
            hashes.push(sd);
        }
        p.integrity = hash_set(hashes.iter().map(String::as_str));
        p.deps = uv_dep_names(t.get("dependencies"));
        if root {
            any_root = true;
            for d in &p.deps {
                lock.mark_direct(d, DirectFlags::default());
            }
            for (key, flags) in [
                (
                    "optional-dependencies",
                    DirectFlags {
                        optional: true,
                        ..Default::default()
                    },
                ),
                (
                    "dev-dependencies",
                    DirectFlags {
                        dev: true,
                        ..Default::default()
                    },
                ),
            ] {
                for (_, group) in t
                    .get(key)
                    .and_then(toml::Value::as_table)
                    .into_iter()
                    .flatten()
                {
                    for d in uv_dep_names(Some(group)) {
                        lock.mark_direct(&d, flags);
                    }
                }
            }
            continue;
        }
        lock.pkgs.push(p);
    }
    if any_root {
        lock.direct.get_or_insert_with(HashMap::new);
        lock.apply_direct();
    }
    Some(lock)
}

/// `pdm.lock`.
pub(crate) fn pdm(text: &str) -> Option<Lock> {
    let doc = toml(text)?;
    let meta = doc.get("metadata").and_then(toml::Value::as_table);
    let mut lock = Lock {
        format_version: meta.and_then(|m| tget(m, "lock_version")),
        ..Default::default()
    };
    for t in packages(&doc) {
        let Some(name) = tget(t, "name") else {
            continue;
        };
        let mut p = Pkg::new(
            name,
            tget(t, "version").unwrap_or_default(),
            Ecosystem::PyPI,
        );
        let groups: Vec<String> = t
            .get("groups")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|g| g.as_str().map(str::to_string))
            .collect();
        p.dev = !groups.is_empty() && !groups.iter().any(|g| g == "default");
        if let Some(g) = tget(t, "git") {
            p.source = Some(PackageSource {
                kind: SourceKind::Git,
                url: Some(g),
                rev: tget(t, "revision").or_else(|| tget(t, "ref")),
            });
        } else if let Some(path) = tget(t, "path") {
            p.source = Some(PackageSource {
                kind: SourceKind::Path,
                url: Some(path),
                rev: None,
            });
        }
        let hashes = file_hashes(t, "files");
        p.integrity = hash_set(hashes.iter().map(String::as_str));
        p.deps = t
            .get("dependencies")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|d| d.as_str().map(|s| pep508_name(s).to_string()))
            .filter(|s| !s.is_empty())
            .collect();
        lock.pkgs.push(p);
    }
    Some(lock)
}

/// `Pipfile.lock` (`default` and `develop`).
pub(crate) fn pipfile(text: &str) -> Option<Lock> {
    let doc = json(text)?;
    let meta = doc.get("_meta")?;
    let mut lock = Lock {
        format_version: js(meta, "pipfile-spec"),
        ..Default::default()
    };
    for (section, dev) in [("default", false), ("develop", true)] {
        for (name, e) in jobj(&doc, section) {
            let version = js(e, "version").unwrap_or_default();
            let version = version.trim_start_matches("==").to_string();
            let mut p = Pkg::new(name.clone(), version, Ecosystem::PyPI);
            p.dev = dev;
            if let Some(g) = js(e, "git") {
                let rev = js(e, "ref");
                if p.version.is_empty() {
                    p.version = rev.as_deref().unwrap_or("").chars().take(12).collect();
                }
                p.source = Some(PackageSource {
                    kind: SourceKind::Git,
                    url: Some(g),
                    rev,
                });
            } else if let Some(path) = js(e, "path") {
                p.source = Some(PackageSource {
                    kind: SourceKind::Path,
                    url: Some(path),
                    rev: None,
                });
            }
            let hashes: Vec<&str> = e
                .get("hashes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            p.integrity = hash_set(hashes);
            lock.pkgs.push(p);
        }
    }
    Some(lock)
}
