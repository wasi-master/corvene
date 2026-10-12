//! Corvene `1338-lockfile-diff`: package manager lockfiles read into
//! packages and compared, for a dependency table instead of thousands of
//! lines of lockfile diff.
//!
//! GitHub Desktop shows lockfiles as their text diff only
//! (`app/src/ui/diff/index.tsx`).

mod diff;
mod formats;
mod model;
mod text;
pub mod version;

use corvene_models::LockfileDiff;
use corvene_models::lockfile::Ecosystem;

use crate::formats::{apple_beam, go_ruby, js, other, rust_python};
use crate::model::Lock;

/// A lockfile format: display label, main ecosystem and parser.
#[derive(Clone, Copy)]
struct Format {
    label: &'static str,
    eco: Ecosystem,
    parse: fn(&str) -> Option<Lock>,
}

const fn f(label: &'static str, eco: Ecosystem, parse: fn(&str) -> Option<Lock>) -> Format {
    Format { label, eco, parse }
}

/// The format for a file name (the last path component).
fn format_for(path: &str) -> Option<Format> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    Some(match name {
        "package-lock.json" => f("package-lock.json", Ecosystem::Npm, js::npm),
        "npm-shrinkwrap.json" => f("npm-shrinkwrap.json", Ecosystem::Npm, js::npm),
        "yarn.lock" => f("yarn.lock", Ecosystem::Npm, js::yarn),
        "pnpm-lock.yaml" => f("pnpm-lock.yaml", Ecosystem::Npm, js::pnpm),
        "bun.lock" => f("bun.lock", Ecosystem::Npm, js::bun),
        "deno.lock" => f("deno.lock", Ecosystem::Jsr, js::deno),
        "Cargo.lock" => f("Cargo.lock", Ecosystem::Cargo, rust_python::cargo),
        "poetry.lock" => f("poetry.lock", Ecosystem::PyPI, rust_python::poetry),
        "uv.lock" => f("uv.lock", Ecosystem::PyPI, rust_python::uv),
        "pdm.lock" => f("pdm.lock", Ecosystem::PyPI, rust_python::pdm),
        "Pipfile.lock" => f("Pipfile.lock", Ecosystem::PyPI, rust_python::pipfile),
        "go.sum" => f("go.sum", Ecosystem::Go, go_ruby::go_sum),
        "go.work.sum" => f("go.work.sum", Ecosystem::Go, go_ruby::go_sum),
        "Gemfile.lock" => f("Gemfile.lock", Ecosystem::RubyGems, go_ruby::gemfile),
        "gems.locked" => f("gems.locked", Ecosystem::RubyGems, go_ruby::gemfile),
        "composer.lock" => f("composer.lock", Ecosystem::Packagist, other::composer),
        "Podfile.lock" => f("Podfile.lock", Ecosystem::CocoaPods, apple_beam::podfile),
        "Package.resolved" => f(
            "Package.resolved",
            Ecosystem::SwiftPM,
            apple_beam::swift_resolved,
        ),
        "Cartfile.resolved" => f(
            "Cartfile.resolved",
            Ecosystem::Carthage,
            apple_beam::cartfile,
        ),
        "pubspec.lock" => f("pubspec.lock", Ecosystem::Pub, other::pubspec),
        "mix.lock" => f("mix.lock", Ecosystem::Hex, apple_beam::mix),
        "rebar.lock" => f("rebar.lock", Ecosystem::Hex, apple_beam::rebar),
        "packages.lock.json" => f("packages.lock.json", Ecosystem::NuGet, other::nuget),
        "gradle.lockfile" => f("gradle.lockfile", Ecosystem::Maven, other::gradle),
        "buildscript-gradle.lockfile" => f(
            "buildscript-gradle.lockfile",
            Ecosystem::Maven,
            other::gradle,
        ),
        "flake.lock" => f("flake.lock", Ecosystem::Nix, other::flake),
        "Chart.lock" => f("Chart.lock", Ecosystem::Helm, other::chart_lock),
        ".terraform.lock.hcl" => f(
            ".terraform.lock.hcl",
            Ecosystem::Terraform,
            other::terraform,
        ),
        "conda-lock.yml" | "conda-lock.yaml" => {
            f("conda-lock.yml", Ecosystem::Conda, other::conda_lock)
        }
        "pixi.lock" => f("pixi.lock", Ecosystem::Conda, other::pixi),
        "renv.lock" => f("renv.lock", Ecosystem::Cran, other::renv),
        "Manifest.toml" | "JuliaManifest.toml" => {
            f("Manifest.toml", Ecosystem::Julia, other::julia)
        }
        "stack.yaml.lock" => f("stack.yaml.lock", Ecosystem::Hackage, other::stack_lock),
        "cabal.project.freeze" => f(
            "cabal.project.freeze",
            Ecosystem::Hackage,
            other::cabal_freeze,
        ),
        "shard.lock" => f("shard.lock", Ecosystem::Shards, other::shard),
        "vcpkg-lock.json" => f("vcpkg-lock.json", Ecosystem::Vcpkg, other::vcpkg),
        n if n.ends_with(".conda-lock.yml") => {
            f("conda-lock.yml", Ecosystem::Conda, other::conda_lock)
        }
        // Julia version-specific manifests (`Manifest-v1.11.toml`).
        n if n.starts_with("Manifest-v") && n.ends_with(".toml") => {
            f("Manifest.toml", Ecosystem::Julia, other::julia)
        }
        // Gradle's older per-configuration lock files.
        n if n.ends_with(".lockfile") && path.contains("dependency-locks/") => {
            f("gradle.lockfile", Ecosystem::Maven, other::gradle)
        }
        _ => return None,
    })
}

/// Whether `path` names a lockfile this crate reads (by file name, case
/// sensitive as the tools write them; `path` may include directories).
pub fn is_lockfile(path: &str) -> bool {
    format_for(path).is_some()
}

/// The dependency changes between two versions of the lockfile at `path`
/// (`None` for a side that does not exist). `None` when the file is not a
/// known lockfile or neither side parses; when only one side parses, the
/// diff carries a note saying which side could not be read.
pub fn lockfile_diff(path: &str, old: Option<&str>, new: Option<&str>) -> Option<LockfileDiff> {
    let fmt = format_for(path)?;
    let old_lock = old.map(|t| (fmt.parse)(t));
    let new_lock = new.map(|t| (fmt.parse)(t));
    let old_ok = old_lock.as_ref().and_then(Option::as_ref);
    let new_ok = new_lock.as_ref().and_then(Option::as_ref);
    if old_ok.is_none() && new_ok.is_none() {
        return None;
    }
    let mut label = fmt.label.to_string();
    if fmt.label == "yarn.lock" {
        let berry = |l: Option<&Lock>| {
            l.and_then(|l| l.format_version.as_deref())
                .is_some_and(|v| v != "1")
        };
        label = if berry(new_ok) || (new_ok.is_none() && berry(old_ok)) {
            "yarn.lock (Berry)".into()
        } else {
            "yarn.lock (classic)".into()
        };
    }
    let mut d = diff::diff(&label, fmt.eco, old_ok, new_ok);
    d.old_present = old.is_some();
    d.new_present = new.is_some();
    if matches!(old_lock, Some(None)) {
        d.notes.insert(
            0,
            "The old version of this lockfile could not be read, so every package shows as added."
                .into(),
        );
    }
    if matches!(new_lock, Some(None)) {
        d.notes.insert(0, "The new version of this lockfile could not be read, so every package shows as removed.".into());
    }
    Some(d)
}
