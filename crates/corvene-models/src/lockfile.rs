//! Corvene `1338-lockfile-diff`: a package manager lockfile change read as
//! dependency changes ("serde 1.0.200 → 1.0.210, added foo 0.3").
//!
//! GitHub Desktop shows lockfiles as their text diff only
//! (`app/src/ui/diff/index.tsx`).

use serde::{Deserialize, Serialize};

/// The package ecosystem (registry) a package comes from.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub enum Ecosystem {
    /// npm registry (npm, Yarn, pnpm, Bun, Deno `npm:`).
    Npm,
    /// crates.io.
    Cargo,
    /// PyPI (Poetry, uv, PDM, Pipenv, pixi pypi entries).
    PyPI,
    /// RubyGems (Bundler).
    RubyGems,
    /// Go modules.
    Go,
    /// Packagist (Composer).
    Packagist,
    /// CocoaPods.
    CocoaPods,
    /// Swift Package Manager.
    SwiftPM,
    /// pub.dev (Dart, Flutter).
    Pub,
    /// hex.pm (Mix, rebar3).
    Hex,
    /// NuGet.
    NuGet,
    /// Maven repositories (Gradle).
    Maven,
    /// Nix flake inputs.
    Nix,
    /// jsr.io.
    Jsr,
    /// Deno remote modules (URLs).
    DenoRemote,
    /// Helm chart repositories.
    Helm,
    /// Terraform / OpenTofu providers.
    Terraform,
    /// Conda channels.
    Conda,
    /// Carthage.
    Carthage,
    /// CRAN (renv).
    Cran,
    /// Julia General registry.
    Julia,
    /// Hackage / Stackage.
    Hackage,
    /// Crystal shards.
    Shards,
    /// vcpkg ports.
    Vcpkg,
    /// Anything else.
    #[default]
    Other,
}

impl Ecosystem {
    /// Display name ("npm", "crates.io").
    pub fn name(self) -> &'static str {
        match self {
            Ecosystem::Npm => "npm",
            Ecosystem::Cargo => "crates.io",
            Ecosystem::PyPI => "PyPI",
            Ecosystem::RubyGems => "RubyGems",
            Ecosystem::Go => "Go modules",
            Ecosystem::Packagist => "Packagist",
            Ecosystem::CocoaPods => "CocoaPods",
            Ecosystem::SwiftPM => "Swift Package Manager",
            Ecosystem::Pub => "pub.dev",
            Ecosystem::Hex => "Hex",
            Ecosystem::NuGet => "NuGet",
            Ecosystem::Maven => "Maven",
            Ecosystem::Nix => "Nix",
            Ecosystem::Jsr => "JSR",
            Ecosystem::DenoRemote => "Deno remote",
            Ecosystem::Helm => "Helm",
            Ecosystem::Terraform => "Terraform",
            Ecosystem::Conda => "Conda",
            Ecosystem::Carthage => "Carthage",
            Ecosystem::Cran => "CRAN",
            Ecosystem::Julia => "Julia",
            Ecosystem::Hackage => "Hackage",
            Ecosystem::Shards => "Shards",
            Ecosystem::Vcpkg => "vcpkg",
            Ecosystem::Other => "Other",
        }
    }

    /// The registry page for `name` at `version`, when the ecosystem has a
    /// public registry with stable package URLs.
    pub fn package_url(self, name: &str, version: Option<&str>) -> Option<String> {
        if name.is_empty() {
            return None;
        }
        let v = version.filter(|v| !v.is_empty());
        Some(match self {
            Ecosystem::Npm => match v {
                Some(v) => format!("https://www.npmjs.com/package/{name}/v/{v}"),
                None => format!("https://www.npmjs.com/package/{name}"),
            },
            Ecosystem::Cargo => match v {
                Some(v) => format!("https://crates.io/crates/{name}/{v}"),
                None => format!("https://crates.io/crates/{name}"),
            },
            Ecosystem::PyPI => match v {
                Some(v) => format!("https://pypi.org/project/{name}/{v}/"),
                None => format!("https://pypi.org/project/{name}/"),
            },
            Ecosystem::RubyGems => match v {
                Some(v) => format!("https://rubygems.org/gems/{name}/versions/{v}"),
                None => format!("https://rubygems.org/gems/{name}"),
            },
            Ecosystem::Go => match v {
                Some(v) => format!("https://pkg.go.dev/{name}@{v}"),
                None => format!("https://pkg.go.dev/{name}"),
            },
            Ecosystem::Packagist => format!("https://packagist.org/packages/{name}"),
            Ecosystem::CocoaPods => {
                let pod = name.split('/').next().unwrap_or(name);
                format!("https://cocoapods.org/pods/{pod}")
            }
            Ecosystem::Pub => match v {
                Some(v) => format!("https://pub.dev/packages/{name}/versions/{v}"),
                None => format!("https://pub.dev/packages/{name}"),
            },
            Ecosystem::Hex => match v {
                Some(v) => format!("https://hex.pm/packages/{name}/{v}"),
                None => format!("https://hex.pm/packages/{name}"),
            },
            Ecosystem::NuGet => match v {
                Some(v) => format!("https://www.nuget.org/packages/{name}/{v}"),
                None => format!("https://www.nuget.org/packages/{name}"),
            },
            Ecosystem::Maven => {
                let (group, artifact) = name.split_once(':')?;
                match v {
                    Some(v) => {
                        format!("https://central.sonatype.com/artifact/{group}/{artifact}/{v}")
                    }
                    None => format!("https://central.sonatype.com/artifact/{group}/{artifact}"),
                }
            }
            Ecosystem::Jsr => {
                if !name.starts_with('@') {
                    return None;
                }
                match v {
                    Some(v) => format!("https://jsr.io/{name}@{v}"),
                    None => format!("https://jsr.io/{name}"),
                }
            }
            Ecosystem::DenoRemote => {
                if name.starts_with("https://") || name.starts_with("http://") {
                    name.to_string()
                } else {
                    return None;
                }
            }
            Ecosystem::Terraform => {
                let mut parts = name.split('/');
                let host = parts.next()?;
                let ns = parts.next()?;
                let ty = parts.next()?;
                if host != "registry.terraform.io" && host != "registry.opentofu.org" {
                    return None;
                }
                match v {
                    Some(v) => format!("https://registry.terraform.io/providers/{ns}/{ty}/{v}"),
                    None => format!("https://registry.terraform.io/providers/{ns}/{ty}/latest"),
                }
            }
            Ecosystem::Conda => {
                let (channel, pkg) = match name.split_once('/') {
                    Some((c, p)) => (c, p),
                    None => ("conda-forge", name),
                };
                format!("https://anaconda.org/{channel}/{pkg}")
            }
            Ecosystem::Cran => format!("https://cran.r-project.org/package={name}"),
            Ecosystem::Hackage => match v {
                Some(v) => format!("https://hackage.haskell.org/package/{name}-{v}"),
                None => format!("https://hackage.haskell.org/package/{name}"),
            },
            Ecosystem::Vcpkg => format!("https://vcpkg.io/en/package/{name}"),
            Ecosystem::SwiftPM
            | Ecosystem::Nix
            | Ecosystem::Helm
            | Ecosystem::Carthage
            | Ecosystem::Julia
            | Ecosystem::Shards
            | Ecosystem::Other => return None,
        })
    }
}

/// Where a locked package comes from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SourceKind {
    /// The ecosystem's package registry (or a mirror of it).
    #[default]
    Registry,
    /// A git repository at a revision.
    Git,
    /// A local path, workspace member or link.
    Path,
    /// A direct tarball / archive / file URL.
    Url,
    /// Anything else the lockfile records.
    Other,
}

impl SourceKind {
    /// Short label ("registry", "git").
    pub fn label(self) -> &'static str {
        match self {
            SourceKind::Registry => "registry",
            SourceKind::Git => "git",
            SourceKind::Path => "path",
            SourceKind::Url => "url",
            SourceKind::Other => "other",
        }
    }
}

/// A package's source as the lockfile records it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PackageSource {
    /// Registry, git, path, ...
    pub kind: SourceKind,
    /// Repository / registry / path / archive URL, when recorded (a default
    /// registry is usually `None`).
    pub url: Option<String>,
    /// Git commit (or branch / tag when no commit is recorded).
    pub rev: Option<String>,
}

impl PackageSource {
    /// A source of `kind` with no URL or revision.
    pub fn of(kind: SourceKind) -> Self {
        Self {
            kind,
            url: None,
            rev: None,
        }
    }

    /// One line for the UI: "registry", "git github.com/a/b @ 1a2b3c4".
    pub fn label(&self) -> String {
        let mut out = self.kind.label().to_string();
        if let Some(url) = &self.url {
            out.push(' ');
            let short = url
                .trim_start_matches("git+")
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .trim_start_matches("ssh://")
                .trim_start_matches("git://");
            out.push_str(short);
        }
        if let Some(rev) = &self.rev {
            out.push_str(" @ ");
            out.push_str(short_rev(rev));
        }
        out
    }

    /// The repository web page (`https://github.com/o/r`) for a git source
    /// on a known forge, or for any http(s) git URL.
    pub fn web_url(&self) -> Option<String> {
        let url = self.url.as_deref()?;
        web_repo_url(url).map(|(host, path)| format!("https://{host}/{path}"))
    }
}

/// The first 7 characters of a commit-like revision (full string otherwise).
pub fn short_rev(rev: &str) -> &str {
    if rev.len() >= 40 && rev.bytes().all(|b| b.is_ascii_hexdigit()) {
        &rev[..7]
    } else {
        rev
    }
}

/// `(host, owner/repo)` for a git or https repository URL, without `.git`,
/// query or fragment.
pub fn web_repo_url(url: &str) -> Option<(String, String)> {
    let mut s = url.trim();
    s = s.trim_start_matches("git+");
    let s = s.split(['#', '?']).next().unwrap_or(s);
    if let Some(r) = s.strip_prefix("git@") {
        // git@github.com:owner/repo.git
        let (host, path) = r.split_once(':')?;
        return clean_repo(host, path);
    }
    if let Some(r) = s.strip_prefix("github:") {
        return clean_repo("github.com", r);
    }
    if let Some(r) = s.strip_prefix("gitlab:") {
        return clean_repo("gitlab.com", r);
    }
    let rest = ["https://", "http://", "ssh://", "git://"]
        .iter()
        .find_map(|p| s.strip_prefix(p))?;
    let rest = rest.rsplit_once('@').map_or(rest, |(_, r)| r);
    let (host, path) = rest.split_once('/')?;
    let host = host.split(':').next().unwrap_or(host);
    clean_repo(host, path)
}

fn clean_repo(host: &str, path: &str) -> Option<(String, String)> {
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if host.is_empty() || path.is_empty() {
        return None;
    }
    let path = if host == "github.com" {
        // Keep owner/repo only (codeload / tarball URLs carry more).
        let mut it = path.splitn(3, '/');
        let owner = it.next()?;
        let repo = it.next()?;
        format!("{owner}/{repo}")
    } else if let Some((p, _)) = path.split_once("/-/") {
        p.to_string()
    } else {
        path.to_string()
    };
    Some((host.to_ascii_lowercase(), path))
}

/// How a package's version moved.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub enum ChangeKind {
    /// A newer version.
    #[default]
    Upgraded,
    /// An older version.
    Downgraded,
    /// Not in the old lockfile.
    Added,
    /// Not in the new lockfile.
    Removed,
    /// Same version (or versions that do not order) with a changed source,
    /// checksum or revision.
    Changed,
}

impl ChangeKind {
    /// Every kind in display order.
    pub const ALL: [ChangeKind; 5] = [
        ChangeKind::Upgraded,
        ChangeKind::Downgraded,
        ChangeKind::Added,
        ChangeKind::Removed,
        ChangeKind::Changed,
    ];

    /// Group heading ("Upgraded").
    pub fn label(self) -> &'static str {
        match self {
            ChangeKind::Upgraded => "Upgraded",
            ChangeKind::Downgraded => "Downgraded",
            ChangeKind::Added => "Added",
            ChangeKind::Removed => "Removed",
            ChangeKind::Changed => "Changed",
        }
    }

    /// Lower-case word for the summary line ("upgraded").
    pub fn word(self) -> &'static str {
        match self {
            ChangeKind::Upgraded => "upgraded",
            ChangeKind::Downgraded => "downgraded",
            ChangeKind::Added => "added",
            ChangeKind::Removed => "removed",
            ChangeKind::Changed => "changed",
        }
    }
}

/// The size of a version step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Bump {
    /// First version number changed.
    Major,
    /// Second version number changed.
    Minor,
    /// Third version number changed.
    Patch,
    /// Only the pre-release / build / qualifier part changed.
    Prerelease,
    /// Versions that are not dotted numbers (git revisions, dates, ...).
    Other,
}

impl Bump {
    /// Badge text ("major").
    pub fn label(self) -> &'static str {
        match self {
            Bump::Major => "major",
            Bump::Minor => "minor",
            Bump::Patch => "patch",
            Bump::Prerelease => "prerelease",
            Bump::Other => "other",
        }
    }
}

/// One row of the dependency table.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageChange {
    /// Package name as the ecosystem writes it (`@types/node`,
    /// `golang.org/x/net`, `com.google.guava:guava`).
    pub name: String,
    /// Registry the package comes from.
    pub ecosystem: Ecosystem,
    /// Upgraded, added, ...
    pub kind: ChangeKind,
    /// Version before (`None` when added).
    pub old_version: Option<String>,
    /// Version after (`None` when removed).
    pub new_version: Option<String>,
    /// Size of the version step (`None` for added, removed and same-version
    /// rows).
    pub bump: Option<Bump>,
    /// A direct dependency of the project (`None` when the lockfile does not
    /// say).
    pub direct: Option<bool>,
    /// Only needed for development.
    pub dev: bool,
    /// Optional dependency.
    pub optional: bool,
    /// Peer dependency.
    pub peer: bool,
    /// Source before, when the lockfile records one.
    pub old_source: Option<PackageSource>,
    /// Source after, when the lockfile records one.
    pub new_source: Option<PackageSource>,
    /// The checksum / integrity changed while the version stayed the same:
    /// a republished or tampered package. Shown as a warning row.
    pub integrity_changed: bool,
    /// Packages that depend on this one (transitive rows, when the lockfile
    /// records dependency edges).
    pub required_by: Vec<String>,
}

impl PackageChange {
    /// Whether the source changed between both sides (registry → git, git
    /// revision moved, ...).
    pub fn source_changed(&self) -> bool {
        match (&self.old_source, &self.new_source) {
            (Some(a), Some(b)) => a != b,
            _ => false,
        }
    }

    /// Whether the row should use the warning style.
    pub fn is_warning(&self) -> bool {
        self.integrity_changed
    }

    /// "1.0.200 → 1.0.210", "1.2.3" for added / removed / same-version rows.
    pub fn version_label(&self) -> String {
        match (&self.old_version, &self.new_version) {
            (Some(a), Some(b)) if a != b => format!("{a} → {b}"),
            (_, Some(b)) => b.clone(),
            (Some(a), None) => a.clone(),
            (None, None) => String::new(),
        }
    }

    /// Small tags for the row ("dev", "optional", "peer", "transitive").
    pub fn tags(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        match self.direct {
            Some(true) => out.push("direct"),
            Some(false) => out.push("transitive"),
            None => {}
        }
        if self.dev {
            out.push("dev");
        }
        if self.optional {
            out.push("optional");
        }
        if self.peer {
            out.push("peer");
        }
        out
    }

    fn current_source(&self) -> Option<&PackageSource> {
        self.new_source.as_ref().or(self.old_source.as_ref())
    }

    fn current_version(&self) -> Option<&str> {
        self.new_version.as_deref().or(self.old_version.as_deref())
    }

    /// The package's page: the registry page for registry packages, the
    /// repository for git packages.
    pub fn registry_url(&self) -> Option<String> {
        if let Some(src) = self.current_source() {
            match src.kind {
                SourceKind::Git => return src.web_url(),
                SourceKind::Path => return None,
                SourceKind::Registry | SourceKind::Url | SourceKind::Other => {}
            }
            if matches!(
                self.ecosystem,
                Ecosystem::SwiftPM
                    | Ecosystem::Nix
                    | Ecosystem::Carthage
                    | Ecosystem::Shards
                    | Ecosystem::Helm
                    | Ecosystem::Julia
            ) {
                return src.web_url().or_else(|| {
                    src.url
                        .as_deref()
                        .filter(|u| u.starts_with("https://"))
                        .map(str::to_string)
                });
            }
        }
        self.ecosystem
            .package_url(&self.name, self.current_version())
    }

    /// A compare page between the old and new git revisions, when both are
    /// commits of the same repository on github.com or gitlab.com.
    pub fn compare_url(&self) -> Option<String> {
        let a = self.old_source.as_ref()?;
        let b = self.new_source.as_ref()?;
        let (ra, rb) = (a.rev.as_deref()?, b.rev.as_deref()?);
        if ra == rb {
            return None;
        }
        let (ha, pa) = web_repo_url(a.url.as_deref()?)?;
        let (hb, pb) = web_repo_url(b.url.as_deref()?)?;
        if ha != hb || !pa.eq_ignore_ascii_case(&pb) {
            return None;
        }
        match ha.as_str() {
            "github.com" => Some(format!("https://github.com/{pb}/compare/{ra}...{rb}")),
            "gitlab.com" => Some(format!("https://gitlab.com/{pb}/-/compare/{ra}...{rb}")),
            _ => None,
        }
    }

    /// Rough heap size.
    pub fn weight(&self) -> usize {
        let s = |o: &Option<String>| o.as_ref().map_or(0, String::len);
        let src = |o: &Option<PackageSource>| {
            o.as_ref().map_or(0, |p| {
                s(&p.url) + s(&p.rev) + std::mem::size_of::<PackageSource>()
            })
        };
        std::mem::size_of::<Self>()
            + self.name.len()
            + s(&self.old_version)
            + s(&self.new_version)
            + src(&self.old_source)
            + src(&self.new_source)
            + self.required_by.iter().map(|r| r.len() + 24).sum::<usize>()
    }
}

/// Row counts per kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockfileCounts {
    /// Upgraded rows.
    pub upgraded: usize,
    /// Downgraded rows.
    pub downgraded: usize,
    /// Added rows.
    pub added: usize,
    /// Removed rows.
    pub removed: usize,
    /// Changed-in-place rows.
    pub changed: usize,
    /// Packages identical on both sides.
    pub unchanged: usize,
    /// Rows whose checksum changed under the same version.
    pub integrity_warnings: usize,
}

/// The dependency changes between two versions of a lockfile.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockfileDiff {
    /// Lockfile kind ("package-lock.json", "yarn.lock (Berry)").
    pub format: String,
    /// Main ecosystem of the lockfile (rows carry their own).
    pub ecosystem: Ecosystem,
    /// Lockfile format version before (`lockfileVersion`, `version = 3`).
    pub old_format_version: Option<String>,
    /// Lockfile format version after.
    pub new_format_version: Option<String>,
    /// Whether the old side exists (false when the file was added).
    pub old_present: bool,
    /// Whether the new side exists (false when the file was deleted).
    pub new_present: bool,
    /// Packages read on the old side.
    pub old_package_count: usize,
    /// Packages read on the new side.
    pub new_package_count: usize,
    /// Packages identical on both sides.
    pub unchanged: usize,
    /// Changed packages: grouped by kind in [`ChangeKind::ALL`] order, then
    /// direct before transitive, then by name.
    pub changes: Vec<PackageChange>,
    /// Notes about the file itself ("lockfileVersion 2 → 3", "the old
    /// version could not be read", tool version changes).
    pub notes: Vec<String>,
}

impl LockfileDiff {
    /// Row counts per kind.
    pub fn counts(&self) -> LockfileCounts {
        let mut c = LockfileCounts {
            unchanged: self.unchanged,
            ..Default::default()
        };
        for ch in &self.changes {
            match ch.kind {
                ChangeKind::Upgraded => c.upgraded += 1,
                ChangeKind::Downgraded => c.downgraded += 1,
                ChangeKind::Added => c.added += 1,
                ChangeKind::Removed => c.removed += 1,
                ChangeKind::Changed => c.changed += 1,
            }
            if ch.integrity_changed {
                c.integrity_warnings += 1;
            }
        }
        c
    }

    /// "12 upgraded, 3 added, 1 removed, 2 downgraded" ("No dependency
    /// changes" when empty).
    pub fn summary(&self) -> String {
        let c = self.counts();
        let parts: Vec<String> = [
            (c.upgraded, "upgraded"),
            (c.added, "added"),
            (c.removed, "removed"),
            (c.downgraded, "downgraded"),
            (c.changed, "changed"),
        ]
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, w)| format!("{n} {w}"))
        .collect();
        if parts.is_empty() {
            "No dependency changes".to_string()
        } else {
            parts.join(", ")
        }
    }

    /// The rows of one group, in display order.
    pub fn group(&self, kind: ChangeKind) -> impl Iterator<Item = &PackageChange> {
        self.changes.iter().filter(move |c| c.kind == kind)
    }

    /// The format version change as a note ("lockfileVersion 2 → 3").
    pub fn format_version_note(&self) -> Option<String> {
        match (&self.old_format_version, &self.new_format_version) {
            (Some(a), Some(b)) if a != b => Some(format!("{} {a} → {b}", self.version_key())),
            _ => None,
        }
    }

    fn version_key(&self) -> &'static str {
        if self.format.starts_with("package-lock") || self.format.starts_with("npm-shrinkwrap") {
            "lockfileVersion"
        } else {
            "Format version"
        }
    }

    /// Rough heap size (diff cache sizing).
    pub fn weight(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.format.len()
            + self.old_format_version.as_ref().map_or(0, String::len)
            + self.new_format_version.as_ref().map_or(0, String::len)
            + self
                .changes
                .iter()
                .map(PackageChange::weight)
                .sum::<usize>()
            + self.notes.iter().map(|n| n.len() + 24).sum::<usize>()
    }
}
