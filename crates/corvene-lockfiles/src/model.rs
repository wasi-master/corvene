//! One parsed side of a lockfile: its packages and a few file-level facts.

use corvene_models::lockfile::{Ecosystem, PackageSource, SourceKind};

/// A small fast hasher for the diff's string-keyed maps (the inputs are
/// local files, so hash flooding is not a concern).
#[derive(Default, Clone, Copy)]
pub(crate) struct FxHasher(u64);

impl std::hash::Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        let (chunks, rest) = bytes.as_chunks::<8>();
        for c in chunks {
            self.add(u64::from_le_bytes(*c));
        }
        for &b in rest {
            self.add(u64::from(b));
        }
    }

    fn write_u8(&mut self, i: u8) {
        self.add(u64::from(i));
    }

    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

impl FxHasher {
    fn add(&mut self, w: u64) {
        self.0 = (self.0.rotate_left(5) ^ w).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}

/// A `HashMap` with [`FxHasher`].
pub(crate) type FastMap<K, V> =
    std::collections::HashMap<K, V, std::hash::BuildHasherDefault<FxHasher>>;

/// A locked package as read from one side.
#[derive(Clone, Debug, Default)]
pub(crate) struct Pkg {
    pub name: String,
    pub version: String,
    pub eco: Ecosystem,
    pub source: Option<PackageSource>,
    /// Checksum / integrity / content hash, compared only when the version
    /// and source match.
    pub integrity: Option<String>,
    pub direct: Option<bool>,
    pub dev: bool,
    pub optional: bool,
    pub peer: bool,
    /// Names of the packages this one depends on.
    pub deps: Vec<String>,
}

impl Pkg {
    pub fn new(name: impl Into<String>, version: impl Into<String>, eco: Ecosystem) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            eco,
            ..Default::default()
        }
    }
}

/// Dependency flags of a direct dependency.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct DirectFlags {
    pub dev: bool,
    pub optional: bool,
    pub peer: bool,
}

/// One parsed side.
#[derive(Debug, Default)]
pub(crate) struct Lock {
    pub format_version: Option<String>,
    pub pkgs: Vec<Pkg>,
    /// Tool / runtime versions recorded in the file ("Bundler", "2.5.3").
    pub meta: Vec<(String, String)>,
    /// The project's direct dependencies when the file records them. When
    /// set, every package gets `direct` from it.
    pub direct: Option<std::collections::HashMap<String, DirectFlags>>,
}

impl Lock {
    /// Applies `direct` to every package (and dev / optional / peer flags of
    /// direct dependencies).
    pub fn apply_direct(&mut self) {
        let Some(direct) = &self.direct else { return };
        for p in &mut self.pkgs {
            match direct.get(&p.name) {
                Some(f) => {
                    if p.direct.is_none() {
                        p.direct = Some(true);
                    }
                    p.dev |= f.dev;
                    p.optional |= f.optional;
                    p.peer |= f.peer;
                }
                None => {
                    if p.direct.is_none() {
                        p.direct = Some(false);
                    }
                }
            }
        }
    }

    pub fn mark_direct(&mut self, name: &str, flags: DirectFlags) {
        let map = self.direct.get_or_insert_with(Default::default);
        let e = map.entry(name.to_string()).or_insert(flags);
        // A dependency listed both as prod and dev counts as prod.
        e.dev &= flags.dev;
        e.optional &= flags.optional;
        e.peer &= flags.peer;
    }
}

/// Several hashes (file hashes of one release) as one integrity value:
/// sorted, newline separated. Compared as sets by [`integrity_conflict`].
pub(crate) fn hash_set<'a>(items: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let mut v: Vec<&str> = items
        .into_iter()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    v.dedup();
    Some(v.join("\n"))
}

/// Whether two integrity values disagree: a hash changed rather than files
/// being added or dropped (neither set contains the other).
pub(crate) fn integrity_conflict(a: &str, b: &str) -> bool {
    if a == b {
        return false;
    }
    if !a.contains('\n') && !b.contains('\n') {
        return true;
    }
    let sa: std::collections::HashSet<&str> = a.split('\n').collect();
    let sb: std::collections::HashSet<&str> = b.split('\n').collect();
    !(sa.is_subset(&sb) || sb.is_subset(&sa))
}

/// Classifies an npm-style `resolved` / version specifier (git URL, local
/// path, registry tarball).
pub(crate) fn js_source(resolved: &str) -> PackageSource {
    let r = resolved.trim();
    let git_like = r.starts_with("git+")
        || r.starts_with("git:")
        || r.starts_with("git@")
        || r.starts_with("github:")
        || r.starts_with("gitlab:")
        || r.starts_with("bitbucket:")
        || (r.contains('#') && (r.contains(".git#") || r.contains("#commit=")));
    if git_like {
        let (url, frag) = r.split_once('#').unwrap_or((r, ""));
        let rev = frag.strip_prefix("commit=").unwrap_or(frag);
        let rev = rev.split('&').next().unwrap_or(rev);
        return PackageSource {
            kind: SourceKind::Git,
            url: Some(expand_shorthand(url)),
            rev: (!rev.is_empty()).then(|| rev.to_string()),
        };
    }
    if let Some(rest) = r.strip_prefix("https://codeload.github.com/") {
        // https://codeload.github.com/owner/repo/tar.gz/<sha>
        let parts: Vec<&str> = rest.split('/').collect();
        if parts.len() >= 4 {
            return PackageSource {
                kind: SourceKind::Git,
                url: Some(format!("https://github.com/{}/{}", parts[0], parts[1])),
                rev: Some(parts[3].to_string()),
            };
        }
    }
    for p in ["file:", "link:", "portal:", "workspace:"] {
        if let Some(rest) = r.strip_prefix(p) {
            return PackageSource {
                kind: SourceKind::Path,
                url: Some(rest.to_string()),
                rev: None,
            };
        }
    }
    if r.starts_with("http://") || r.starts_with("https://") {
        if r.contains("/-/") {
            let reg = r.split("/-/").next().unwrap_or(r);
            // Registry tarball: keep the registry host only when not npmjs.
            let host = reg.split('/').nth(2).unwrap_or("");
            let url = (host != "registry.npmjs.org" && host != "registry.yarnpkg.com")
                .then(|| format!("https://{host}"));
            return PackageSource {
                kind: SourceKind::Registry,
                url,
                rev: None,
            };
        }
        return PackageSource {
            kind: SourceKind::Url,
            url: Some(r.split('#').next().unwrap_or(r).to_string()),
            rev: None,
        };
    }
    if r.is_empty() {
        return PackageSource::of(SourceKind::Registry);
    }
    // A relative path (npm link targets).
    PackageSource {
        kind: SourceKind::Path,
        url: Some(r.to_string()),
        rev: None,
    }
}

fn expand_shorthand(url: &str) -> String {
    let url = url.trim_start_matches("git+");
    if let Some(r) = url.strip_prefix("github:") {
        return format!("https://github.com/{r}");
    }
    if let Some(r) = url.strip_prefix("gitlab:") {
        return format!("https://gitlab.com/{r}");
    }
    if let Some(r) = url.strip_prefix("bitbucket:") {
        return format!("https://bitbucket.org/{r}");
    }
    url.to_string()
}

/// `git+https://host/repo?branch=x#rev` (Cargo, uv) as a git source.
pub(crate) fn git_url_with_rev(s: &str) -> PackageSource {
    let s = s.trim_start_matches("git+");
    let (url, rev) = s.split_once('#').unwrap_or((s, ""));
    let (url, query) = url.split_once('?').unwrap_or((url, ""));
    let rev = if rev.is_empty() {
        query.split('&').find_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            matches!(k, "rev" | "tag" | "branch").then(|| v.to_string())
        })
    } else {
        Some(rev.to_string())
    };
    PackageSource {
        kind: SourceKind::Git,
        url: Some(url.to_string()),
        rev,
    }
}

/// The name in a `name@range` / `@scope/name@range` descriptor.
pub(crate) fn split_at_version(desc: &str) -> (&str, &str) {
    let start = usize::from(desc.starts_with('@'));
    match desc[start..].find('@') {
        Some(i) => (&desc[..start + i], &desc[start + i + 1..]),
        None => (desc, ""),
    }
}

/// The leading package name of a PEP 508 requirement ("requests>=2; ...").
pub(crate) fn pep508_name(req: &str) -> &str {
    let req = req.trim();
    let end = req
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
        .unwrap_or(req.len());
    &req[..end]
}

/// Days since 1970-01-01 to a civil date (`YYYY-MM-DD`).
pub(crate) fn epoch_date(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}
