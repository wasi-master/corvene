//! `composer.lock`, `pubspec.lock`, NuGet `packages.lock.json`, Gradle
//! lockfiles, `flake.lock`, Helm `Chart.lock`, `.terraform.lock.hcl`,
//! `conda-lock.yml`, `pixi.lock`, `renv.lock`, Julia `Manifest.toml`,
//! `stack.yaml.lock`, `cabal.project.freeze`, `shard.lock` and
//! `vcpkg-lock.json`.

use std::collections::HashMap;

use corvene_models::lockfile::{Ecosystem, PackageSource, SourceKind};
use serde_json::Value;

use crate::model::{Lock, Pkg, epoch_date, hash_set};
use crate::text::{jobj, js, json, toml, ts, yaml, yget, yhash, ylist, ys};

/// `composer.lock` (`packages` and `packages-dev`).
pub(crate) fn composer(text: &str) -> Option<Lock> {
    let doc = json(text)?;
    doc.get("packages")?;
    let mut lock = Lock::default();
    if let Some(v) = js(&doc, "plugin-api-version") {
        lock.meta.push(("Composer plugin API".into(), v));
    }
    for (key, dev) in [("packages", false), ("packages-dev", true)] {
        for e in doc.get(key).and_then(Value::as_array).into_iter().flatten() {
            let Some(name) = js(e, "name") else { continue };
            let version = js(e, "version").unwrap_or_default();
            let mut p = Pkg::new(name, version, Ecosystem::Packagist);
            p.dev = dev;
            let source = e.get("source");
            let dist = e.get("dist");
            let is_dev_branch = p.version.starts_with("dev-") || p.version.ends_with("-dev");
            let reference = source
                .and_then(|s| js(s, "reference"))
                .or_else(|| dist.and_then(|d| js(d, "reference")));
            if is_dev_branch {
                p.source = Some(PackageSource {
                    kind: SourceKind::Git,
                    url: source.and_then(|s| js(s, "url")),
                    rev: reference,
                });
            } else if let Some(t) = dist.and_then(|d| js(d, "type"))
                && t == "path"
            {
                p.source = Some(PackageSource {
                    kind: SourceKind::Path,
                    url: dist.and_then(|d| js(d, "url")),
                    rev: None,
                });
            } else {
                // A tag: its commit is the integrity (a moved tag is a warning).
                p.integrity = dist
                    .and_then(|d| js(d, "shasum"))
                    .filter(|s| !s.is_empty())
                    .or(reference);
            }
            p.deps = jobj(e, "require")
                .map(|(n, _)| n.clone())
                .filter(|n| n.contains('/'))
                .collect();
            lock.pkgs.push(p);
        }
    }
    Some(lock)
}

/// `pubspec.lock`.
pub(crate) fn pubspec(text: &str) -> Option<Lock> {
    let doc = yaml(text)?;
    doc["packages"].as_hash()?;
    let mut lock = Lock::default();
    for (k, v) in yhash(&doc["sdks"]) {
        if let Some(c) = ys(v) {
            lock.meta.push((format!("SDK {k}"), c));
        }
    }
    for (name, e) in yhash(&doc["packages"]) {
        let version = yget(e, "version").unwrap_or_default();
        let mut p = Pkg::new(name, version, Ecosystem::Pub);
        let dep = yget(e, "dependency").unwrap_or_default();
        p.direct = Some(dep.starts_with("direct"));
        p.dev = dep == "direct dev";
        let desc = &e["description"];
        p.source = Some(match yget(e, "source").as_deref() {
            Some("git") => PackageSource {
                kind: SourceKind::Git,
                url: yget(desc, "url"),
                rev: yget(desc, "resolved-ref").or_else(|| yget(desc, "ref")),
            },
            Some("path") => PackageSource {
                kind: SourceKind::Path,
                url: yget(desc, "path"),
                rev: None,
            },
            Some("sdk") => PackageSource {
                kind: SourceKind::Other,
                url: ys(desc),
                rev: None,
            },
            _ => {
                let url = yget(desc, "url")
                    .filter(|u| u != "https://pub.dev" && u != "https://pub.dartlang.org");
                PackageSource {
                    kind: SourceKind::Registry,
                    url,
                    rev: None,
                }
            }
        });
        p.integrity = yget(desc, "sha256");
        lock.pkgs.push(p);
    }
    Some(lock)
}

/// NuGet `packages.lock.json` (per target framework).
pub(crate) fn nuget(text: &str) -> Option<Lock> {
    let doc = json(text)?;
    let fws = doc.get("dependencies")?.as_object()?;
    let mut lock = Lock {
        format_version: js(&doc, "version"),
        ..Default::default()
    };
    let mut direct: HashMap<String, bool> = HashMap::new();
    for (_, deps) in fws {
        for (name, e) in deps.as_object().into_iter().flatten() {
            let ty = js(e, "type").unwrap_or_default();
            let d = direct.entry(name.clone()).or_insert(false);
            *d |= ty == "Direct";
            let mut p = if ty == "Project" {
                let mut p = Pkg::new(name.clone(), "project", Ecosystem::NuGet);
                p.source = Some(PackageSource::of(SourceKind::Path));
                p
            } else {
                let Some(v) = js(e, "resolved") else { continue };
                let mut p = Pkg::new(name.clone(), v, Ecosystem::NuGet);
                p.integrity = js(e, "contentHash");
                p
            };
            p.deps = jobj(e, "dependencies").map(|(n, _)| n.clone()).collect();
            lock.pkgs.push(p);
        }
    }
    for p in &mut lock.pkgs {
        p.direct = direct.get(&p.name).copied();
    }
    Some(lock)
}

/// `gradle.lockfile` / `buildscript-gradle.lockfile` and the older
/// per-configuration `dependency-locks/*.lockfile`.
pub(crate) fn gradle(text: &str) -> Option<Lock> {
    let mut lock = Lock::default();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let (coord, configs) = t.split_once('=').unwrap_or((t, ""));
        if coord == "empty" {
            continue;
        }
        let parts: Vec<&str> = coord.split(':').collect();
        if parts.len() < 3 {
            return None;
        }
        let name = format!("{}:{}", parts[0], parts[1]);
        let mut p = Pkg::new(name, parts[2], Ecosystem::Maven);
        let cfgs: Vec<&str> = configs.split(',').filter(|c| !c.is_empty()).collect();
        p.dev = !cfgs.is_empty() && cfgs.iter().all(|c| c.to_ascii_lowercase().contains("test"));
        lock.pkgs.push(p);
    }
    Some(lock)
}

/// `flake.lock`.
pub(crate) fn flake(text: &str) -> Option<Lock> {
    let doc = json(text)?;
    let nodes = doc.get("nodes")?.as_object()?;
    let root_name = js(&doc, "root").unwrap_or_else(|| "root".into());
    let mut lock = Lock {
        format_version: js(&doc, "version"),
        ..Default::default()
    };
    let root_inputs: Vec<String> = nodes
        .get(&root_name)
        .map(|r| {
            jobj(r, "inputs")
                .filter_map(|(_, v)| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    for (name, node) in nodes {
        if *name == root_name {
            continue;
        }
        let Some(locked) = node.get("locked") else {
            continue;
        };
        let ty = js(locked, "type").unwrap_or_default();
        let rev = js(locked, "rev");
        let url = match ty.as_str() {
            "github" | "gitlab" | "sourcehut" => {
                let host = match ty.as_str() {
                    "github" => "github.com".to_string(),
                    "gitlab" => "gitlab.com".to_string(),
                    _ => "git.sr.ht".to_string(),
                };
                let host = js(locked, "host").unwrap_or(host);
                match (js(locked, "owner"), js(locked, "repo")) {
                    (Some(o), Some(r)) => Some(format!("https://{host}/{o}/{r}")),
                    _ => None,
                }
            }
            "path" => js(locked, "path"),
            _ => js(locked, "url"),
        };
        let kind = match ty.as_str() {
            "path" => SourceKind::Path,
            "tarball" | "file" => SourceKind::Url,
            "indirect" => SourceKind::Other,
            _ => SourceKind::Git,
        };
        let version = locked
            .get("lastModified")
            .and_then(Value::as_i64)
            .map(|s| epoch_date(s.clamp(0, 253_402_300_799)))
            .or_else(|| rev.as_deref().map(|r| r.chars().take(12).collect()))
            .unwrap_or_default();
        let mut p = Pkg::new(name.clone(), version, Ecosystem::Nix);
        p.source = Some(PackageSource { kind, url, rev });
        p.integrity = js(locked, "narHash");
        p.direct = Some(root_inputs.contains(name));
        p.deps = jobj(node, "inputs")
            .filter_map(|(_, v)| v.as_str().map(str::to_string))
            .collect();
        lock.pkgs.push(p);
    }
    Some(lock)
}

/// Helm `Chart.lock`.
pub(crate) fn chart_lock(text: &str) -> Option<Lock> {
    let doc = yaml(text)?;
    let deps = doc["dependencies"].as_vec()?;
    let mut lock = Lock::default();
    for d in deps {
        let Some(name) = yget(d, "name") else {
            continue;
        };
        let mut p = Pkg::new(
            name,
            yget(d, "version").unwrap_or_default(),
            Ecosystem::Helm,
        );
        p.source = Some(PackageSource {
            kind: SourceKind::Registry,
            url: yget(d, "repository"),
            rev: None,
        });
        p.direct = Some(true);
        lock.pkgs.push(p);
    }
    Some(lock)
}

/// `.terraform.lock.hcl`.
pub(crate) fn terraform(text: &str) -> Option<Lock> {
    let mut lock = Lock::default();
    let mut cur: Option<Pkg> = None;
    let mut hashes: Vec<String> = Vec::new();
    let mut in_hashes = false;
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with("//") {
            continue;
        }
        if let Some(rest) = t.strip_prefix("provider ") {
            let name = rest.trim_end_matches('{').trim().trim_matches('"');
            cur = Some(Pkg::new(name, "", Ecosystem::Terraform));
            hashes.clear();
            continue;
        }
        let Some(p) = cur.as_mut() else { continue };
        if in_hashes {
            if t.starts_with(']') {
                in_hashes = false;
            } else {
                let h = t.trim_end_matches(',').trim_matches('"');
                if !h.is_empty() {
                    hashes.push(h.to_string());
                }
            }
            continue;
        }
        if t == "}" {
            if let Some(mut p) = cur.take() {
                p.integrity = hash_set(hashes.iter().map(String::as_str));
                lock.pkgs.push(p);
            }
            continue;
        }
        if let Some((k, v)) = t.split_once('=') {
            let v = v.trim();
            match k.trim() {
                "version" => p.version = v.trim_matches('"').to_string(),
                "hashes" => {
                    if v.starts_with('[') && v.ends_with(']') {
                        hashes.extend(
                            v[1..v.len() - 1]
                                .split(',')
                                .map(|h| h.trim().trim_matches('"').to_string())
                                .filter(|h| !h.is_empty()),
                        );
                    } else {
                        in_hashes = true;
                    }
                }
                _ => {}
            }
        }
    }
    if lock.pkgs.is_empty() && !text.trim().is_empty() && !text.contains("provider") {
        return None;
    }
    Some(lock)
}

/// `{name}-{version}-{build}.conda` / `.tar.bz2` to name and version.
fn conda_file(url: &str) -> Option<(String, String)> {
    let file = url.rsplit('/').next()?;
    let stem = file
        .strip_suffix(".conda")
        .or_else(|| file.strip_suffix(".tar.bz2"))?;
    let mut it = stem.rsplitn(3, '-');
    let _build = it.next()?;
    let version = it.next()?;
    let name = it.next()?;
    Some((name.to_string(), version.to_string()))
}

/// A wheel / sdist file name to name and version.
fn pypi_file(url: &str) -> Option<(String, String)> {
    let file = url.rsplit('/').next()?.split('#').next()?;
    if let Some(stem) = file.strip_suffix(".whl") {
        let mut it = stem.split('-');
        return Some((it.next()?.replace('_', "-"), it.next()?.to_string()));
    }
    let stem = file
        .strip_suffix(".tar.gz")
        .or_else(|| file.strip_suffix(".zip"))?;
    let (n, v) = stem.rsplit_once('-')?;
    Some((n.replace('_', "-"), v.to_string()))
}

fn channel_of(url: &str) -> Option<String> {
    // https://conda.anaconda.org/<channel>/<subdir>/<file>
    let rest = url.strip_prefix("https://conda.anaconda.org/")?;
    let ch = rest.split('/').next()?;
    (ch != "conda-forge").then(|| ch.to_string())
}

/// Collects multi-platform entries: one package per name and version, with
/// every platform's hash in its integrity set.
#[derive(Default)]
struct Merger {
    index: HashMap<(Ecosystem, String, String), usize>,
    hashes: Vec<Vec<String>>,
    lock: Lock,
}

impl Merger {
    fn add(&mut self, p: Pkg, hash: Option<String>) {
        let key = (p.eco, p.name.clone(), p.version.clone());
        let i = match self.index.get(&key) {
            Some(&i) => i,
            None => {
                self.index.insert(key, self.lock.pkgs.len());
                self.lock.pkgs.push(p);
                self.hashes.push(Vec::new());
                self.lock.pkgs.len() - 1
            }
        };
        if let Some(h) = hash {
            self.hashes[i].push(h);
        }
    }

    fn finish(mut self) -> Lock {
        for (p, h) in self.lock.pkgs.iter_mut().zip(&self.hashes) {
            p.integrity = hash_set(h.iter().map(String::as_str));
        }
        self.lock
    }
}

/// `conda-lock.yml`.
pub(crate) fn conda_lock(text: &str) -> Option<Lock> {
    let doc = yaml(text)?;
    let pkgs = doc["package"].as_vec()?;
    let mut m = Merger::default();
    m.lock.format_version = yget(&doc, "version");
    for e in pkgs {
        let Some(name) = yget(e, "name") else {
            continue;
        };
        let eco = if yget(e, "manager").as_deref() == Some("pip") {
            Ecosystem::PyPI
        } else {
            Ecosystem::Conda
        };
        let url = yget(e, "url").unwrap_or_default();
        let name = match (eco, channel_of(&url)) {
            (Ecosystem::Conda, Some(ch)) => format!("{ch}/{name}"),
            _ => name,
        };
        let mut p = Pkg::new(name, yget(e, "version").unwrap_or_default(), eco);
        p.dev = yget(e, "category").is_some_and(|c| c != "main");
        p.optional = e["optional"].as_bool().unwrap_or(false);
        p.deps = yhash(&e["dependencies"]).map(|(n, _)| n).collect();
        let hash = yget(&e["hash"], "sha256").or_else(|| yget(&e["hash"], "md5"));
        m.add(p, hash);
    }
    Some(m.finish())
}

/// `pixi.lock` (versions 4 to 6).
pub(crate) fn pixi(text: &str) -> Option<Lock> {
    let doc = yaml(text)?;
    let pkgs = doc["packages"].as_vec()?;
    let mut m = Merger::default();
    m.lock.format_version = yget(&doc, "version");
    for e in pkgs {
        let (eco, url) = if let Some(u) = yget(e, "conda") {
            (Ecosystem::Conda, u)
        } else if let Some(u) = yget(e, "pypi") {
            (Ecosystem::PyPI, u)
        } else {
            match yget(e, "kind").as_deref() {
                Some("pypi") => (Ecosystem::PyPI, yget(e, "url").unwrap_or_default()),
                Some("conda") => (Ecosystem::Conda, yget(e, "url").unwrap_or_default()),
                _ => continue,
            }
        };
        let from_file = match eco {
            Ecosystem::Conda => conda_file(&url),
            _ => pypi_file(&url),
        };
        let name = yget(e, "name").or_else(|| from_file.as_ref().map(|f| f.0.clone()));
        let version = yget(e, "version").or_else(|| from_file.as_ref().map(|f| f.1.clone()));
        let Some(name) = name else { continue };
        let name = match (eco, channel_of(&url)) {
            (Ecosystem::Conda, Some(ch)) => format!("{ch}/{name}"),
            _ => name,
        };
        let mut p = Pkg::new(name, version.unwrap_or_default(), eco);
        if !url.starts_with("http") && !url.is_empty() {
            p.source = Some(PackageSource {
                kind: SourceKind::Path,
                url: Some(url.clone()),
                rev: None,
            });
        } else if url.starts_with("git+") {
            p.source = Some(crate::model::git_url_with_rev(&url));
        }
        let deps = if eco == Ecosystem::Conda {
            &e["depends"]
        } else {
            &e["requires_dist"]
        };
        p.deps = ylist(deps)
            .filter_map(ys)
            .map(|d| {
                d.split([' ', '<', '>', '=', ';', '['])
                    .next()
                    .unwrap_or("")
                    .to_string()
            })
            .filter(|d| !d.is_empty())
            .collect();
        let hash = yget(e, "sha256").or_else(|| yget(e, "md5"));
        m.add(p, hash);
    }
    let mut lock = m.finish();
    // Direct dependencies are in pixi.toml, not the lockfile.
    lock.direct = None;
    Some(lock)
}

/// `renv.lock`.
pub(crate) fn renv(text: &str) -> Option<Lock> {
    let doc = json(text)?;
    let pkgs = doc.get("Packages")?.as_object()?;
    let mut lock = Lock::default();
    if let Some(r) = doc.get("R").and_then(|r| js(r, "Version")) {
        lock.meta.push(("R".into(), r));
    }
    for (key, e) in pkgs {
        let name = js(e, "Package").unwrap_or_else(|| key.clone());
        let mut p = Pkg::new(name, js(e, "Version").unwrap_or_default(), Ecosystem::Cran);
        let src = js(e, "Source").unwrap_or_default();
        p.source = Some(match src.as_str() {
            "GitHub" | "GitLab" | "Bitbucket" | "git" => {
                let host = match src.as_str() {
                    "GitLab" => "gitlab.com",
                    "Bitbucket" => "bitbucket.org",
                    _ => "github.com",
                };
                let url = js(e, "RemoteUrl").or_else(|| {
                    match (js(e, "RemoteUsername"), js(e, "RemoteRepo")) {
                        (Some(u), Some(r)) => Some(format!("https://{host}/{u}/{r}")),
                        _ => None,
                    }
                });
                PackageSource {
                    kind: SourceKind::Git,
                    url,
                    rev: js(e, "RemoteSha"),
                }
            }
            "Local" => PackageSource {
                kind: SourceKind::Path,
                url: js(e, "RemoteUrl"),
                rev: None,
            },
            _ => {
                let repo = js(e, "Repository").filter(|r| r != "CRAN");
                PackageSource {
                    kind: SourceKind::Registry,
                    url: repo,
                    rev: None,
                }
            }
        });
        p.integrity = js(e, "Hash");
        p.deps = e
            .get("Requirements")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|r| r.as_str().map(str::to_string))
            .collect();
        lock.pkgs.push(p);
    }
    Some(lock)
}

/// Julia `Manifest.toml` (format 1 and 2).
pub(crate) fn julia(text: &str) -> Option<Lock> {
    let doc = toml(text)?;
    let mut lock = Lock {
        format_version: ts(doc.get("manifest_format")),
        ..Default::default()
    };
    if let Some(v) = ts(doc.get("julia_version")) {
        lock.meta.push(("Julia".into(), v));
    }
    let deps = match doc.get("deps").and_then(toml::Value::as_table) {
        Some(d) => d,
        None => &doc,
    };
    for (name, entries) in deps {
        for e in entries
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(toml::Value::as_table)
        {
            // Standard libraries carry no version.
            let Some(version) = ts(e.get("version")) else {
                continue;
            };
            let mut p = Pkg::new(name.clone(), version, Ecosystem::Julia);
            p.integrity = ts(e.get("git-tree-sha1"));
            if let Some(url) = ts(e.get("repo-url")) {
                let kind = if url.starts_with('/') || url.starts_with('.') {
                    SourceKind::Path
                } else {
                    SourceKind::Git
                };
                p.source = Some(PackageSource {
                    kind,
                    url: Some(url),
                    rev: ts(e.get("repo-rev")),
                });
            } else if let Some(path) = ts(e.get("path")) {
                p.source = Some(PackageSource {
                    kind: SourceKind::Path,
                    url: Some(path),
                    rev: None,
                });
            }
            p.deps = match e.get("deps") {
                Some(toml::Value::Array(a)) => a
                    .iter()
                    .filter_map(|d| d.as_str().map(str::to_string))
                    .collect(),
                Some(toml::Value::Table(t)) => t.keys().cloned().collect(),
                _ => Vec::new(),
            };
            lock.pkgs.push(p);
        }
    }
    Some(lock)
}

/// `name-1.2.3` (Hackage package id) to its parts.
fn hackage_id(s: &str) -> Option<(String, String)> {
    let s = s.split('@').next()?;
    let (n, v) = s.rsplit_once('-')?;
    v.starts_with(|c: char| c.is_ascii_digit())
        .then(|| (n.to_string(), v.to_string()))
}

/// `stack.yaml.lock`.
pub(crate) fn stack_lock(text: &str) -> Option<Lock> {
    let doc = yaml(text)?;
    let mut lock = Lock::default();
    for s in ylist(&doc["snapshots"]) {
        let orig = &s["original"];
        let name = ys(orig)
            .or_else(|| yget(orig, "url"))
            .or_else(|| yget(&s["completed"], "url"));
        if let Some(n) = name {
            lock.meta.push(("Snapshot".into(), n));
        }
    }
    for e in ylist(&doc["packages"]) {
        let c = &e["completed"];
        if let Some(h) = yget(c, "hackage") {
            let Some((name, version)) = hackage_id(&h) else {
                continue;
            };
            let mut p = Pkg::new(name, version, Ecosystem::Hackage);
            p.integrity = h.split_once('@').map(|(_, s)| s.to_string());
            lock.pkgs.push(p);
        } else if let Some(url) = yget(c, "git").or_else(|| yget(c, "repo")) {
            let rev = yget(c, "commit");
            let name = yget(c, "name").unwrap_or_else(|| url.clone());
            let version = yget(c, "version")
                .unwrap_or_else(|| rev.as_deref().unwrap_or("").chars().take(12).collect());
            let mut p = Pkg::new(name, version, Ecosystem::Hackage);
            p.source = Some(PackageSource {
                kind: SourceKind::Git,
                url: Some(url),
                rev,
            });
            lock.pkgs.push(p);
        }
    }
    Some(lock)
}

/// `cabal.project.freeze`.
pub(crate) fn cabal_freeze(text: &str) -> Option<Lock> {
    let mut lock = Lock::default();
    let mut in_constraints = false;
    for line in text.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with("--") {
            continue;
        }
        let indented = line.starts_with([' ', '\t']);
        let mut t = line.trim();
        if !indented {
            in_constraints = false;
            if let Some((k, v)) = t.split_once(':') {
                match k.trim() {
                    "constraints" => {
                        in_constraints = true;
                        t = v.trim();
                    }
                    "index-state" => {
                        lock.meta.push(("Index state".into(), v.trim().to_string()));
                        continue;
                    }
                    _ => continue,
                }
            }
        }
        if !in_constraints {
            continue;
        }
        for c in t.split(',') {
            let c = c.trim();
            let Some((pkg, ver)) = c.split_once("==") else {
                continue;
            };
            let pkg = pkg.trim();
            let name = pkg.strip_prefix("any.").unwrap_or(pkg);
            lock.pkgs
                .push(Pkg::new(name, ver.trim(), Ecosystem::Hackage));
        }
    }
    if lock.pkgs.is_empty() && !text.contains("constraints") {
        return None;
    }
    Some(lock)
}

/// `shard.lock`.
pub(crate) fn shard(text: &str) -> Option<Lock> {
    let doc = yaml(text)?;
    doc["shards"].as_hash()?;
    let mut lock = Lock {
        format_version: yget(&doc, "version"),
        ..Default::default()
    };
    for (name, e) in yhash(&doc["shards"]) {
        let version = yget(e, "version")
            .or_else(|| yget(e, "commit"))
            .unwrap_or_default();
        let url = ["git", "github", "gitlab", "bitbucket", "path"]
            .iter()
            .find_map(|k| {
                let v = yget(e, k)?;
                Some(match *k {
                    "github" => format!("https://github.com/{v}"),
                    "gitlab" => format!("https://gitlab.com/{v}"),
                    "bitbucket" => format!("https://bitbucket.org/{v}"),
                    _ => v,
                })
            });
        let kind = if yget(e, "path").is_some() {
            SourceKind::Path
        } else {
            SourceKind::Git
        };
        let rev = version
            .split_once("+git.commit.")
            .map(|(_, c)| c.to_string());
        let mut p = Pkg::new(name, version, Ecosystem::Shards);
        p.source = Some(PackageSource { kind, url, rev });
        lock.pkgs.push(p);
    }
    Some(lock)
}

/// `vcpkg-lock.json`: registry URL → ref → commit.
pub(crate) fn vcpkg(text: &str) -> Option<Lock> {
    let doc = json(text)?;
    let mut lock = Lock::default();
    for (url, refs) in doc.as_object()? {
        for (r, commit) in refs.as_object().into_iter().flatten() {
            let Some(c) = commit.as_str() else { continue };
            let name = if r == "HEAD" {
                url.clone()
            } else {
                format!("{url}#{r}")
            };
            let mut p = Pkg::new(
                name,
                c.chars().take(12).collect::<String>(),
                Ecosystem::Vcpkg,
            );
            p.source = Some(PackageSource {
                kind: SourceKind::Git,
                url: Some(url.clone()),
                rev: Some(c.to_string()),
            });
            lock.pkgs.push(p);
        }
    }
    Some(lock)
}
