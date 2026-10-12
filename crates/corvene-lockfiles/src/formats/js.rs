//! npm `package-lock.json` / `npm-shrinkwrap.json`, `yarn.lock` (classic
//! and Berry), `pnpm-lock.yaml`, `bun.lock` and `deno.lock`.

use std::collections::HashMap;

use corvene_models::lockfile::{Ecosystem, PackageSource, SourceKind};
use serde_json::Value;
use yaml_rust::Yaml;

use crate::model::{DirectFlags, Lock, Pkg, js_source, split_at_version};
use crate::text::{jobj, js, json, strip_jsonc, yaml, yget, yhash};

const MAX_NPM_DEPTH: usize = 64;

fn dep_names(v: &Value, keys: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = keys
        .iter()
        .flat_map(|k| jobj(v, k).map(|(n, _)| n.clone()))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

fn jbool(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// Direct dependency flags from a package.json-like object.
fn mark_manifest_deps(lock: &mut Lock, root: &Value) {
    let groups: [(&str, DirectFlags); 4] = [
        ("dependencies", DirectFlags::default()),
        (
            "devDependencies",
            DirectFlags {
                dev: true,
                ..Default::default()
            },
        ),
        (
            "optionalDependencies",
            DirectFlags {
                optional: true,
                ..Default::default()
            },
        ),
        (
            "peerDependencies",
            DirectFlags {
                peer: true,
                ..Default::default()
            },
        ),
    ];
    for (key, flags) in groups {
        for (name, _) in jobj(root, key) {
            lock.mark_direct(name, flags);
        }
    }
}

/// npm `package-lock.json` / `npm-shrinkwrap.json`, lockfileVersion 1-3.
pub(crate) fn npm(text: &str) -> Option<Lock> {
    let doc = json(text)?;
    doc.as_object()?;
    let mut lock = Lock {
        format_version: js(&doc, "lockfileVersion"),
        ..Default::default()
    };
    if let Some(packages) = doc.get("packages").and_then(Value::as_object) {
        if let Some(root) = packages.get("") {
            mark_manifest_deps(&mut lock, root);
        }
        let direct_names = lock.direct.take().unwrap_or_default();
        let has_root = packages.contains_key("");
        for (path, entry) in packages {
            if path.is_empty() {
                continue;
            }
            let Some(idx) = path.rfind("node_modules/") else {
                // A workspace folder ("packages/a"); its link entry covers it.
                continue;
            };
            let installed = &path[idx + "node_modules/".len()..];
            let top = idx == 0;
            let link = jbool(entry, "link");
            let resolved = js(entry, "resolved").unwrap_or_default();
            let (version, source) = if link {
                let target = packages.get(resolved.as_str());
                let v = target.and_then(|t| js(t, "version")).unwrap_or_default();
                (
                    v,
                    PackageSource {
                        kind: SourceKind::Path,
                        url: Some(resolved.clone()),
                        rev: None,
                    },
                )
            } else {
                (
                    js(entry, "version").unwrap_or_default(),
                    js_source(&resolved),
                )
            };
            // Aliases (`"foo": "npm:bar@1"`) record the real name.
            let name = js(entry, "name").unwrap_or_else(|| installed.to_string());
            let mut p = Pkg::new(name, version, Ecosystem::Npm);
            p.source = Some(source);
            p.integrity = js(entry, "integrity");
            p.dev = jbool(entry, "dev") || jbool(entry, "devOptional");
            p.optional = jbool(entry, "optional") || jbool(entry, "devOptional");
            p.peer = jbool(entry, "peer");
            if has_root {
                let d = top.then(|| direct_names.get(installed)).flatten();
                p.direct = Some(d.is_some());
                if let Some(f) = d {
                    p.dev |= f.dev;
                    p.optional |= f.optional;
                    p.peer |= f.peer;
                }
            }
            p.deps = dep_names(
                entry,
                &["dependencies", "optionalDependencies", "peerDependencies"],
            );
            lock.pkgs.push(p);
        }
    } else {
        let deps = doc.get("dependencies").and_then(Value::as_object)?;
        npm_v1(deps, 0, &mut lock.pkgs);
    }
    Some(lock)
}

fn npm_v1(deps: &serde_json::Map<String, Value>, depth: usize, out: &mut Vec<Pkg>) {
    if depth > MAX_NPM_DEPTH {
        return;
    }
    for (installed, entry) in deps {
        let raw = js(entry, "version").unwrap_or_default();
        let resolved = js(entry, "resolved").unwrap_or_default();
        let (name, version, source) = if let Some(alias) = raw.strip_prefix("npm:") {
            let (n, v) = split_at_version(alias);
            (n.to_string(), v.to_string(), js_source(&resolved))
        } else if raw.contains(':') || raw.contains('/') {
            // git / file / tarball specifier in place of a version.
            let src = js_source(&raw);
            let v = src
                .rev
                .as_deref()
                .map(|r| r.chars().take(12).collect())
                .unwrap_or(raw.clone());
            (installed.clone(), v, src)
        } else {
            (installed.clone(), raw, js_source(&resolved))
        };
        let mut p = Pkg::new(name, version, Ecosystem::Npm);
        p.source = Some(source);
        p.integrity = js(entry, "integrity");
        p.dev = jbool(entry, "dev");
        p.optional = jbool(entry, "optional");
        p.deps = dep_names(entry, &["requires"]);
        out.push(p);
        if let Some(nested) = entry.get("dependencies").and_then(Value::as_object) {
            npm_v1(nested, depth + 1, out);
        }
    }
}

/// Splits a yarn entry key (`"a@^1", a@^2`) into descriptors.
fn yarn_descriptors(key: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in key.chars() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => {
                let t = cur.trim();
                if !t.is_empty() {
                    out.push(t.to_string());
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    let t = cur.trim();
    if !t.is_empty() {
        out.push(t.to_string());
    }
    out
}

fn unquote(s: &str) -> &str {
    let s = s.trim();
    s.strip_prefix('"')
        .and_then(|r| r.strip_suffix('"'))
        .unwrap_or(s)
}

#[derive(Default)]
struct YarnEntry {
    key: String,
    fields: HashMap<String, String>,
    deps: Vec<String>,
}

/// `yarn.lock`, classic v1 and Berry (v2+).
pub(crate) fn yarn(text: &str) -> Option<Lock> {
    let text = text.trim_start_matches('\u{feff}');
    let mut entries: Vec<YarnEntry> = Vec::new();
    let mut cur: Option<YarnEntry> = None;
    let mut block: Option<String> = None;
    let mut saw_header = false;
    for line in text.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            if line.starts_with("# yarn lockfile") {
                saw_header = true;
            }
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let t = line.trim();
        if indent == 0 {
            if let Some(e) = cur.take() {
                entries.push(e);
            }
            let key = t.strip_suffix(':')?;
            cur = Some(YarnEntry {
                key: key.to_string(),
                ..Default::default()
            });
            block = None;
            continue;
        }
        let e = cur.as_mut()?;
        if indent <= 2 {
            if let Some(k) = t.strip_suffix(':') {
                block = Some(unquote(k).to_string());
                continue;
            }
            block = None;
            let (k, v) = match t.split_once(": ") {
                Some((k, v)) => (k, v),
                None => t.split_once(' ').unwrap_or((t, "")),
            };
            e.fields
                .insert(unquote(k).to_string(), unquote(v).to_string());
        } else if let Some(b) = &block
            && matches!(b.as_str(), "dependencies" | "optionalDependencies")
        {
            let name = if let Some(r) = t.strip_prefix('"') {
                r.split('"').next().unwrap_or(r)
            } else {
                let end = t.find([' ', ':']).unwrap_or(t.len());
                // `name: range` (Berry) has the colon after the name.
                t[..end].trim_end_matches(':')
            };
            if !name.is_empty() {
                e.deps.push(name.to_string());
            }
        }
    }
    if let Some(e) = cur.take() {
        entries.push(e);
    }
    let berry = entries.iter().any(|e| e.key == "__metadata");
    if !berry && !saw_header && entries.is_empty() {
        return None;
    }
    let format_version = if berry {
        entries
            .iter()
            .find(|e| e.key == "__metadata")
            .and_then(|e| e.fields.get("version").cloned())
            .or(Some("2".into()))
    } else {
        Some("1".into())
    };
    let mut lock = Lock {
        format_version,
        ..Default::default()
    };
    for e in &entries {
        if e.key == "__metadata" {
            continue;
        }
        let descs = yarn_descriptors(&e.key);
        let Some(first) = descs.first() else { continue };
        let version = e.fields.get("version").cloned().unwrap_or_default();
        let mut p;
        if berry {
            let resolution = e
                .fields
                .get("resolution")
                .cloned()
                .unwrap_or_else(|| first.clone());
            let (name, reference) = split_at_version(&resolution);
            let (proto, rest) = reference.split_once(':').unwrap_or(("npm", reference));
            p = Pkg::new(name, version, Ecosystem::Npm);
            let source = match proto {
                "npm" => PackageSource::of(SourceKind::Registry),
                "workspace" | "link" | "portal" | "file" => PackageSource {
                    kind: SourceKind::Path,
                    url: Some(rest.to_string()),
                    rev: None,
                },
                "patch" => {
                    let hash = rest
                        .rsplit("hash=")
                        .next()
                        .filter(|_| rest.contains("hash="));
                    PackageSource {
                        kind: SourceKind::Other,
                        url: Some("patch".into()),
                        rev: hash.map(|h| h.split('&').next().unwrap_or(h).to_string()),
                    }
                }
                _ => js_source(reference),
            };
            if proto == "workspace" {
                // Workspace dependencies are the project's direct ones.
                for d in &e.deps {
                    lock.mark_direct(d, DirectFlags::default());
                }
            }
            p.source = Some(source);
            p.integrity = e.fields.get("checksum").cloned();
        } else {
            let (mut name, range) = split_at_version(first);
            if let Some(alias) = range.strip_prefix("npm:") {
                name = split_at_version(alias).0;
            }
            p = Pkg::new(name, version, Ecosystem::Npm);
            let resolved = e.fields.get("resolved").cloned().unwrap_or_default();
            let src = js_source(&resolved);
            p.integrity = e.fields.get("integrity").cloned().or_else(|| {
                (src.kind == SourceKind::Registry)
                    .then(|| resolved.split_once('#').map(|(_, h)| h.to_string()))
                    .flatten()
            });
            p.source = Some(src);
        }
        p.deps = e.deps.clone();
        lock.pkgs.push(p);
    }
    lock.apply_direct();
    Some(lock)
}

/// A pnpm package key to `(name, version)`: v5 `/name/1.2.3_peer`, v6
/// `/name@1.2.3(peer)`, v9 `name@1.2.3(peer)`.
fn pnpm_key(key: &str, v5: bool) -> (String, String) {
    let k = key.trim_start_matches('/');
    let k = k.split('(').next().unwrap_or(k);
    if v5 {
        let (scope_len, k2) = if k.starts_with('@') {
            match k.find('/') {
                Some(i) => (i + 1, k),
                None => (0, k),
            }
        } else {
            (0, k)
        };
        if let Some(i) = k2[scope_len..].find('/') {
            let name = &k2[..scope_len + i];
            let ver = &k2[scope_len + i + 1..];
            let ver = ver.split('_').next().unwrap_or(ver);
            return (name.to_string(), ver.to_string());
        }
        return (k.to_string(), String::new());
    }
    let (n, v) = split_at_version(k);
    (n.to_string(), v.to_string())
}

/// `pnpm-lock.yaml`, lockfile versions 5.x, 6 and 9.
pub(crate) fn pnpm(text: &str) -> Option<Lock> {
    let doc = yaml(text)?;
    let ver = yget(&doc, "lockfileVersion")?;
    let v5 = ver.starts_with('5') || ver.starts_with('4') || ver.starts_with('3');
    let mut lock = Lock {
        format_version: Some(ver),
        ..Default::default()
    };
    let mark = |lock: &mut Lock, importer: &Yaml| {
        let groups: [(&str, DirectFlags); 3] = [
            ("dependencies", DirectFlags::default()),
            (
                "devDependencies",
                DirectFlags {
                    dev: true,
                    ..Default::default()
                },
            ),
            (
                "optionalDependencies",
                DirectFlags {
                    optional: true,
                    ..Default::default()
                },
            ),
        ];
        for (key, flags) in groups {
            for (name, _) in yhash(&importer[key]) {
                lock.mark_direct(&name, flags);
            }
        }
    };
    if doc["importers"].as_hash().is_some() {
        for (_, imp) in yhash(&doc["importers"]) {
            mark(&mut lock, imp);
        }
    } else {
        mark(&mut lock, &doc);
    }
    if lock.direct.is_none() {
        lock.direct = Some(HashMap::new());
    }
    // v9 keeps dependency edges and flags in `snapshots`.
    let mut snap_info: HashMap<(String, String), (Vec<String>, bool, bool)> = HashMap::new();
    for (key, s) in yhash(&doc["snapshots"]) {
        let id = pnpm_key(&key, false);
        let deps = pnpm_deps(s);
        let e = snap_info.entry(id).or_insert((Vec::new(), false, false));
        e.0.extend(deps);
        e.1 |= s["optional"].as_bool().unwrap_or(false);
        e.2 |= s["dev"].as_bool().unwrap_or(false);
    }
    for (key, entry) in yhash(&doc["packages"]) {
        let (mut name, mut version) = pnpm_key(&key, v5);
        if let Some(n) = yget(entry, "name") {
            name = n;
        }
        if let Some(v) = yget(entry, "version") {
            version = v;
        }
        let res = &entry["resolution"];
        let source = if yget(res, "type").as_deref() == Some("git") {
            PackageSource {
                kind: SourceKind::Git,
                url: yget(res, "repo"),
                rev: yget(res, "commit"),
            }
        } else if let Some(tb) = yget(res, "tarball") {
            js_source(&tb)
        } else if let Some(dir) = yget(res, "directory") {
            PackageSource {
                kind: SourceKind::Path,
                url: Some(dir),
                rev: None,
            }
        } else {
            PackageSource::of(SourceKind::Registry)
        };
        if source.kind == SourceKind::Git && version.is_empty() {
            version = source
                .rev
                .as_deref()
                .unwrap_or("")
                .chars()
                .take(12)
                .collect();
        }
        let mut p = Pkg::new(name, version, Ecosystem::Npm);
        p.integrity = yget(res, "integrity");
        p.source = Some(source);
        p.dev = entry["dev"].as_bool().unwrap_or(false);
        p.optional = entry["optional"].as_bool().unwrap_or(false);
        p.deps = pnpm_deps(entry);
        if let Some((deps, opt, dev)) = snap_info.get(&(p.name.clone(), p.version.clone())) {
            p.deps.extend(deps.iter().cloned());
            p.optional |= *opt;
            p.dev |= *dev;
        }
        lock.pkgs.push(p);
    }
    lock.apply_direct();
    Some(lock)
}

fn pnpm_deps(entry: &Yaml) -> Vec<String> {
    let mut out: Vec<String> = ["dependencies", "optionalDependencies"]
        .iter()
        .flat_map(|k| yhash(&entry[*k]).map(|(n, _)| n))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// `bun.lock` (text, JSONC).
pub(crate) fn bun(text: &str) -> Option<Lock> {
    let doc = json(&strip_jsonc(text))?;
    let mut lock = Lock {
        format_version: js(&doc, "lockfileVersion"),
        ..Default::default()
    };
    for (_, ws) in jobj(&doc, "workspaces") {
        mark_manifest_deps(&mut lock, ws);
    }
    if lock.direct.is_none() {
        lock.direct = Some(HashMap::new());
    }
    for (_, arr) in jobj(&doc, "packages") {
        let Some(items) = arr.as_array() else {
            continue;
        };
        let Some(ident) = items.first().and_then(Value::as_str) else {
            continue;
        };
        let (name, spec) = split_at_version(ident);
        let (version, source) = if let Some(alias) = spec.strip_prefix("npm:") {
            (
                split_at_version(alias).1.to_string(),
                PackageSource::of(SourceKind::Registry),
            )
        } else if spec.contains(':') || spec.contains('/') {
            let src = js_source(spec);
            let v = match (&src.kind, &src.rev) {
                (SourceKind::Git, Some(r)) => r.chars().take(12).collect(),
                _ => spec.to_string(),
            };
            (v, src)
        } else {
            let reg = items.get(1).and_then(Value::as_str).unwrap_or("");
            let src = if reg.is_empty() {
                PackageSource::of(SourceKind::Registry)
            } else {
                PackageSource {
                    kind: SourceKind::Registry,
                    url: Some(reg.to_string()),
                    rev: None,
                }
            };
            (spec.to_string(), src)
        };
        let mut p = Pkg::new(name, version, Ecosystem::Npm);
        p.source = Some(source);
        if let Some(info) = items.iter().find(|v| v.is_object()) {
            p.deps = dep_names(
                info,
                &["dependencies", "optionalDependencies", "peerDependencies"],
            );
        }
        p.integrity = items
            .iter()
            .skip(1)
            .filter_map(Value::as_str)
            .find(|s| s.starts_with("sha"))
            .map(str::to_string);
        lock.pkgs.push(p);
    }
    lock.apply_direct();
    Some(lock)
}

/// `deno.lock` v2-v5: npm and jsr packages, remote URL modules.
pub(crate) fn deno(text: &str) -> Option<Lock> {
    let doc = json(text)?;
    doc.as_object()?;
    let ver = js(&doc, "version");
    let mut lock = Lock {
        format_version: ver,
        ..Default::default()
    };
    let pk = doc
        .get("packages")
        .filter(|p| p.is_object())
        .unwrap_or(&doc);
    for (eco, key) in [(Ecosystem::Npm, "npm"), (Ecosystem::Jsr, "jsr")] {
        for (id, entry) in jobj(pk, key) {
            let id = id.split('_').next().unwrap_or(id);
            let (name, version) = split_at_version(id);
            let mut p = Pkg::new(name, version, eco);
            p.integrity = js(entry, "integrity");
            p.deps = match entry.get("dependencies") {
                Some(Value::Array(a)) => a
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|d| deno_spec_name(d).to_string())
                    .collect(),
                Some(Value::Object(m)) => m.keys().cloned().collect(),
                _ => Vec::new(),
            };
            lock.pkgs.push(p);
        }
    }
    let remote = doc.get("remote").and_then(Value::as_object);
    let mut versioned: HashMap<(String, String), ()> = HashMap::new();
    for (url, hash) in remote.into_iter().flatten() {
        if let Some((name, version)) = remote_module(url) {
            if versioned
                .insert((name.clone(), version.clone()), ())
                .is_none()
            {
                lock.pkgs
                    .push(Pkg::new(name, version, Ecosystem::DenoRemote));
            }
        } else {
            let h = hash.as_str().unwrap_or("");
            let mut p = Pkg::new(
                url.clone(),
                h.chars().take(12).collect::<String>(),
                Ecosystem::DenoRemote,
            );
            p.source = Some(PackageSource {
                kind: SourceKind::Url,
                url: Some(url.clone()),
                rev: None,
            });
            lock.pkgs.push(p);
        }
    }
    let ws = doc.get("workspace");
    let mut direct: Vec<String> = Vec::new();
    if let Some(ws) = ws {
        for list in [
            ws.get("dependencies"),
            ws.get("packageJson").and_then(|p| p.get("dependencies")),
        ] {
            for d in list
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                direct.push(deno_spec_name(d).to_string());
            }
        }
    }
    if ws.is_some() {
        for d in direct {
            lock.mark_direct(&d, DirectFlags::default());
        }
        if lock.direct.is_none() {
            lock.direct = Some(HashMap::new());
        }
        // Remote modules are imported by URL, not listed as dependencies.
        for p in &mut lock.pkgs {
            if p.eco == Ecosystem::DenoRemote {
                p.direct = Some(true);
            }
        }
        lock.apply_direct();
    }
    Some(lock)
}

/// `jsr:@std/path@^1` / `npm:chalk@5` / `npm:/chalk@5` to the package name.
fn deno_spec_name(spec: &str) -> &str {
    let s = spec
        .strip_prefix("npm:")
        .or_else(|| spec.strip_prefix("jsr:"))
        .unwrap_or(spec);
    let s = s.trim_start_matches('/');
    split_at_version(s).0
}

/// `https://deno.land/std@0.200.0/path/mod.ts` to
/// `("https://deno.land/std", "0.200.0")`.
fn remote_module(url: &str) -> Option<(String, String)> {
    let at = url.find('@')?;
    let after = &url[at + 1..];
    let end = after.find('/').unwrap_or(after.len());
    let version = &after[..end];
    if version.is_empty()
        || !version
            .trim_start_matches('v')
            .starts_with(|c: char| c.is_ascii_digit())
    {
        return None;
    }
    Some((url[..at].to_string(), version.to_string()))
}
