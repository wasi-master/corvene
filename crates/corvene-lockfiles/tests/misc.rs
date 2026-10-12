//! NuGet, Gradle, Nix, Helm, Terraform, Conda, pixi, R, Julia, Haskell,
//! Crystal and vcpkg lockfiles, plus file name detection.

mod common;

use common::{absent, diff, row};
use corvene_lockfiles::is_lockfile;
use corvene_models::lockfile::{Bump, ChangeKind, Ecosystem};

#[test]
fn detects_lockfiles_by_name() {
    for p in [
        "package-lock.json",
        "a/b/npm-shrinkwrap.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "bun.lock",
        "Cargo.lock",
        "poetry.lock",
        "uv.lock",
        "pdm.lock",
        "Pipfile.lock",
        "go.sum",
        "go.work.sum",
        "Gemfile.lock",
        "composer.lock",
        "ios/Podfile.lock",
        "Package.resolved",
        "pubspec.lock",
        "mix.lock",
        "packages.lock.json",
        "gradle.lockfile",
        "buildscript-gradle.lockfile",
        "gradle/dependency-locks/compileClasspath.lockfile",
        "flake.lock",
        "deno.lock",
        "charts/app/Chart.lock",
        "infra/.terraform.lock.hcl",
        "conda-lock.yml",
        "env.conda-lock.yml",
        "pixi.lock",
        "Cartfile.resolved",
        "renv.lock",
        "Manifest.toml",
        "Manifest-v1.11.toml",
        "stack.yaml.lock",
        "cabal.project.freeze",
        "rebar.lock",
        "shard.lock",
        "vcpkg-lock.json",
    ] {
        assert!(is_lockfile(p), "{p}");
    }
    for p in [
        "bun.lockb",
        "cargo.lock",
        "package.json",
        "Cargo.toml",
        "lock",
        "yarn.lock.bak",
        "foo.lockfile",
    ] {
        assert!(!is_lockfile(p), "{p}");
    }
}

#[test]
fn nuget_packages_lock() {
    let old = r#"{
  "version": 1,
  "dependencies": {
    "net8.0": {
      "Newtonsoft.Json": {"type": "Direct", "requested": "[13.0.1, )", "resolved": "13.0.1", "contentHash": "a"},
      "Serilog": {"type": "Direct", "requested": "[3.0.0, )", "resolved": "3.0.0", "contentHash": "s", "dependencies": {"System.Memory": "4.5.0"}},
      "System.Memory": {"type": "Transitive", "resolved": "4.5.0", "contentHash": "m"},
      "MyLib": {"type": "Project"}
    },
    "net6.0": {
      "Newtonsoft.Json": {"type": "Direct", "requested": "[13.0.1, )", "resolved": "13.0.1", "contentHash": "a"}
    }
  }
}"#;
    let new = old.replace("13.0.1", "13.0.3").replace(
        "\"4.5.0\", \"contentHash\": \"m\"",
        "\"4.5.5\", \"contentHash\": \"m2\"",
    );
    let d = diff("src/App/packages.lock.json", old, &new);
    // Two frameworks with the same version: one row.
    let n = row(&d, "Newtonsoft.Json");
    assert_eq!((n.direct, n.bump), (Some(true), Some(Bump::Patch)));
    assert_eq!(
        n.registry_url().as_deref(),
        Some("https://www.nuget.org/packages/Newtonsoft.Json/13.0.3")
    );
    let m = row(&d, "System.Memory");
    assert_eq!(m.direct, Some(false));
    assert_eq!(m.required_by, ["Serilog"]);
}

#[test]
fn gradle_lockfile() {
    let old = "# This is a Gradle generated file for dependency locking.
# Manual edits can break the build and are not advised.
# This file is expected to be part of source control.
com.google.guava:failureaccess:1.0.1=compileClasspath,runtimeClasspath
com.google.guava:guava:31.1-jre=compileClasspath,runtimeClasspath
junit:junit:4.13.2=testCompileClasspath,testRuntimeClasspath
org.jetbrains.kotlin:kotlin-stdlib:1.9.0=compileClasspath
empty=annotationProcessor
";
    let new = old
        .replace("31.1-jre", "32.1.3-jre")
        .replace("kotlin-stdlib:1.9.0", "kotlin-stdlib:1.9.0-RC");
    let d = diff("gradle.lockfile", old, &new);
    let g = row(&d, "com.google.guava:guava");
    assert_eq!(g.bump, Some(Bump::Major));
    assert_eq!(
        g.registry_url().as_deref(),
        Some("https://central.sonatype.com/artifact/com.google.guava/guava/32.1.3-jre")
    );
    assert_eq!(
        row(&d, "org.jetbrains.kotlin:kotlin-stdlib").kind,
        ChangeKind::Downgraded
    );
    assert!(absent(&d, "junit:junit"));
    let old_style = "com.google.guava:guava:31.1-jre\n";
    let d = diff(
        "gradle/dependency-locks/compileClasspath.lockfile",
        old_style,
        "com.google.guava:guava:31.2-jre\n",
    );
    assert_eq!(row(&d, "com.google.guava:guava").bump, Some(Bump::Minor));
}

#[test]
fn flake_lock() {
    let old = r#"{
  "nodes": {
    "flake-utils": {
      "inputs": {"systems": "systems"},
      "locked": {"lastModified": 1701680307, "narHash": "sha256-fu", "owner": "numtide", "repo": "flake-utils", "rev": "4022d587cbbfd70fe950c1e2083a02621806a725", "type": "github"},
      "original": {"owner": "numtide", "repo": "flake-utils", "type": "github"}
    },
    "nixpkgs": {
      "locked": {"lastModified": 1704290814, "narHash": "sha256-old", "owner": "NixOS", "repo": "nixpkgs", "rev": "70bdadeb94ffc8806c0570eb5c2695ad29f0e421", "type": "github"},
      "original": {"owner": "NixOS", "ref": "nixos-23.11", "repo": "nixpkgs", "type": "github"}
    },
    "root": {"inputs": {"flake-utils": "flake-utils", "nixpkgs": "nixpkgs"}},
    "systems": {
      "locked": {"lastModified": 1681028828, "narHash": "sha256-sys", "owner": "nix-systems", "repo": "default", "rev": "da67096a3b9bf56a91d16901293e51ba5b49a27e", "type": "github"},
      "original": {"owner": "nix-systems", "repo": "default", "type": "github"}
    }
  },
  "root": "root",
  "version": 7
}"#;
    let new = old
        .replace("1704290814", "1705000000")
        .replace("sha256-old", "sha256-new")
        .replace(
            "70bdadeb94ffc8806c0570eb5c2695ad29f0e421",
            "317484b1ead87b9c1b8ac5261a8d2dd748a0492d",
        )
        .replace("sha256-sys", "sha256-sys-tampered");
    let d = diff("flake.lock", old, &new);
    let n = row(&d, "nixpkgs");
    assert_eq!(n.version_label(), "2024-01-03 → 2024-01-11");
    assert_eq!(
        (n.kind, n.bump, n.direct),
        (ChangeKind::Upgraded, Some(Bump::Other), Some(true))
    );
    assert!(!n.integrity_changed);
    assert_eq!(
        n.compare_url().as_deref(),
        Some(
            "https://github.com/NixOS/nixpkgs/compare/70bdadeb94ffc8806c0570eb5c2695ad29f0e421...317484b1ead87b9c1b8ac5261a8d2dd748a0492d"
        )
    );
    let s = row(&d, "systems");
    assert!(s.integrity_changed);
    assert_eq!(s.required_by, ["flake-utils"]);
}

#[test]
fn helm_chart_lock() {
    let old = "dependencies:
- name: postgresql
  repository: https://charts.bitnami.com/bitnami
  version: 12.1.0
- name: redis
  repository: oci://registry-1.docker.io/bitnamicharts
  version: 18.0.0
digest: sha256:aaa
generated: \"2023-10-01T00:00:00Z\"
";
    let new = old
        .replace("12.1.0", "13.2.0")
        .replace("digest: sha256:aaa", "digest: sha256:bbb");
    let d = diff("charts/app/Chart.lock", old, &new);
    let p = row(&d, "postgresql");
    assert_eq!((p.bump, p.direct), (Some(Bump::Major), Some(true)));
    assert_eq!(d.unchanged, 1);
}

#[test]
fn terraform_lock() {
    let old = r#"# This file is maintained automatically by "terraform init".
# Manual edits may be lost in future updates.

provider "registry.terraform.io/hashicorp/aws" {
  version     = "5.31.0"
  constraints = "~> 5.0"
  hashes = [
    "h1:aaa=",
    "zh:111",
    "zh:222",
  ]
}

provider "registry.terraform.io/hashicorp/random" {
  version = "3.6.0"
  hashes = [
    "h1:rrr=",
    "zh:333",
  ]
}
"#;
    // aws upgraded; random gains a platform hash (fine).
    let new = old
        .replace("5.31.0", "5.40.0")
        .replace("    \"zh:333\",", "    \"zh:333\",\n    \"h1:extra=\",");
    let d = diff(".terraform.lock.hcl", old, &new);
    let a = row(&d, "registry.terraform.io/hashicorp/aws");
    assert_eq!(a.bump, Some(Bump::Minor));
    assert_eq!(
        a.registry_url().as_deref(),
        Some("https://registry.terraform.io/providers/hashicorp/aws/5.40.0")
    );
    assert!(absent(&d, "registry.terraform.io/hashicorp/random"));
    let tampered = old.replace("zh:333", "zh:999");
    let d = diff(".terraform.lock.hcl", old, &tampered);
    assert!(row(&d, "registry.terraform.io/hashicorp/random").integrity_changed);
}

#[test]
fn conda_lock() {
    let old = "version: 1
metadata:
  channels:
  - url: conda-forge
package:
- name: python
  version: 3.11.5
  manager: conda
  platform: linux-64
  dependencies:
    libffi: '>=3.4'
  url: https://conda.anaconda.org/conda-forge/linux-64/python-3.11.5-hab00c5b_0_cpython.conda
  hash:
    md5: m1
    sha256: s1
  category: main
  optional: false
- name: python
  version: 3.11.5
  manager: conda
  platform: osx-arm64
  dependencies: {}
  url: https://conda.anaconda.org/conda-forge/osx-arm64/python-3.11.5-h47c9636_0_cpython.conda
  hash:
    md5: m2
    sha256: s2
  category: main
  optional: false
- name: libffi
  version: 3.4.2
  manager: conda
  platform: linux-64
  dependencies: {}
  url: https://conda.anaconda.org/conda-forge/linux-64/libffi-3.4.2-h7f98852_5.tar.bz2
  hash:
    md5: m3
    sha256: s3
  category: main
  optional: false
- name: requests
  version: 2.31.0
  manager: pip
  platform: linux-64
  dependencies: {}
  url: https://files.pythonhosted.org/packages/requests-2.31.0-py3-none-any.whl
  hash:
    sha256: s4
  category: main
  optional: false
";
    let new = old
        .replace("3.11.5", "3.12.1")
        .replace("version: 2.31.0", "version: 2.32.0");
    let d = diff("conda-lock.yml", old, &new);
    let py = row(&d, "python");
    assert_eq!(py.bump, Some(Bump::Minor));
    assert_eq!(py.ecosystem, Ecosystem::Conda);
    assert_eq!(
        py.registry_url().as_deref(),
        Some("https://anaconda.org/conda-forge/python")
    );
    assert_eq!(py.required_by, Vec::<String>::new());
    assert!(absent(&d, "libffi"));
    assert_eq!(d.unchanged, 1);
    assert_eq!(row(&d, "requests").ecosystem, Ecosystem::PyPI);
}

#[test]
fn pixi_lock() {
    let old = "version: 6
environments:
  default:
    channels:
    - url: https://conda.anaconda.org/conda-forge/
    packages:
      linux-64:
      - conda: https://conda.anaconda.org/conda-forge/linux-64/python-3.12.0-hab00c5b_0_cpython.conda
      - pypi: https://files.pythonhosted.org/packages/requests-2.31.0-py3-none-any.whl
packages:
- conda: https://conda.anaconda.org/conda-forge/linux-64/python-3.12.0-hab00c5b_0_cpython.conda
  sha256: aaa
  md5: bbb
  depends:
  - libzlib >=1.2.13,<2.0a0
  license: Python-2.0
- conda: https://conda.anaconda.org/conda-forge/linux-64/libzlib-1.2.13-hd590300_5.conda
  sha256: zzz
- pypi: https://files.pythonhosted.org/packages/requests-2.31.0-py3-none-any.whl
  name: requests
  version: 2.31.0
  sha256: ccc
  requires_dist:
  - charset-normalizer<4,>=2
";
    let new = old
        .replace("python-3.12.0-hab00c5b_0", "python-3.12.1-hab00c5b_0")
        .replace("sha256: zzz", "sha256: yyy");
    let d = diff("pixi.lock", old, &new);
    assert_eq!(row(&d, "python").version_label(), "3.12.0 → 3.12.1");
    let z = row(&d, "libzlib");
    assert!(z.integrity_changed);
    assert_eq!(z.required_by, ["python"]);
    assert!(absent(&d, "requests"));
}

#[test]
fn renv_lock() {
    let old = r#"{
  "R": {"Version": "4.3.1", "Repositories": [{"Name": "CRAN", "URL": "https://cloud.r-project.org"}]},
  "Packages": {
    "dplyr": {"Package": "dplyr", "Version": "1.1.3", "Source": "Repository", "Repository": "CRAN", "Hash": "h1", "Requirements": ["rlang"]},
    "rlang": {"Package": "rlang", "Version": "1.1.1", "Source": "Repository", "Repository": "CRAN", "Hash": "h2"},
    "mypkg": {"Package": "mypkg", "Version": "0.1.0", "Source": "GitHub", "RemoteUsername": "acme", "RemoteRepo": "mypkg", "RemoteSha": "1111111111111111111111111111111111111111"}
  }
}"#;
    let new = old
        .replace("1.1.1", "1.1.3")
        .replace("4.3.1", "4.4.0")
        .replace(
            "1111111111111111111111111111111111111111",
            "2222222222222222222222222222222222222222",
        );
    let d = diff("renv.lock", old, &new);
    let r = row(&d, "rlang");
    assert_eq!(r.required_by, ["dplyr"]);
    assert_eq!(
        r.registry_url().as_deref(),
        Some("https://cran.r-project.org/package=rlang")
    );
    assert!(row(&d, "mypkg").compare_url().is_some());
    assert!(d.notes.iter().any(|n| n == "R 4.3.1 → 4.4.0"));
}

#[test]
fn julia_manifest() {
    let old = r#"# This file is machine-generated - editing it directly is not advised

julia_version = "1.10.0"
manifest_format = "2.0"
project_hash = "x"

[[deps.JSON]]
deps = ["Dates", "Parsers"]
git-tree-sha1 = "a"
uuid = "682c06a0-de6a-54ab-a142-c8b1cf79cde6"
version = "0.21.4"

[[deps.Parsers]]
git-tree-sha1 = "b"
uuid = "69de0a69-1ddd-5017-9359-2bf0b02dc9f0"
version = "2.7.2"

[[deps.Dates]]
uuid = "ade2ca70-3891-5945-98fb-dc099432e06a"
"#;
    let new = old
        .replace("0.21.4", "0.21.5")
        .replace("git-tree-sha1 = \"b\"", "git-tree-sha1 = \"c\"");
    let d = diff("Manifest.toml", old, &new);
    assert_eq!(row(&d, "JSON").bump, Some(Bump::Patch));
    assert!(row(&d, "Parsers").integrity_changed);
    assert_eq!(row(&d, "Parsers").required_by, ["JSON"]);
    assert!(absent(&d, "Dates"));
}

#[test]
fn stack_yaml_lock() {
    let old = "# This file was autogenerated by Stack.
packages:
- completed:
    hackage: aeson-2.1.2.1@sha256:aaa,1234
    pantry-tree:
      sha256: t1
      size: 100
  original:
    hackage: aeson-2.1.2.1
- completed:
    commit: 1111111111111111111111111111111111111111
    git: https://github.com/acme/lib.git
    name: lib
    pantry-tree:
      sha256: t2
      size: 10
    version: 0.1.0
  original:
    commit: 1111111111111111111111111111111111111111
    git: https://github.com/acme/lib.git
snapshots:
- completed:
    sha256: s
    size: 1
    url: https://raw.githubusercontent.com/commercialhaskell/stackage-snapshots/master/lts/21/24.yaml
  original: lts-21.24
";
    let new = old
        .replace("aeson-2.1.2.1", "aeson-2.2.0.0")
        .replace("lts-21.24", "lts-21.25");
    let d = diff("stack.yaml.lock", old, &new);
    let a = row(&d, "aeson");
    assert_eq!(a.bump, Some(Bump::Minor));
    assert_eq!(
        a.registry_url().as_deref(),
        Some("https://hackage.haskell.org/package/aeson-2.2.0.0")
    );
    assert!(
        d.notes
            .iter()
            .any(|n| n == "Snapshot lts-21.24 → lts-21.25")
    );
}

#[test]
fn cabal_freeze() {
    let old = "active-repositories: hackage.haskell.org:merge
constraints: any.Cabal ==3.8.1.0,
             any.aeson ==2.1.2.1,
             aeson +ordered-keymap,
             any.base ==4.17.2.0,
             warp -allow-sendfile
index-state: hackage.haskell.org 2023-12-01T00:00:00Z
";
    let new = old.replace("2.1.2.1", "2.1.2.2");
    let d = diff("cabal.project.freeze", old, &new);
    assert_eq!(row(&d, "aeson").bump, Some(Bump::Patch));
    assert_eq!(d.unchanged, 2);
}

#[test]
fn shard_lock() {
    let old = "version: 2.0
shards:
  kemal:
    git: https://github.com/kemalcr/kemal.git
    version: 1.4.0
  radix:
    git: https://github.com/luislavena/radix.git
    version: 0.4.1+git.commit.aaaaaaa
";
    let new = old.replace("1.4.0", "1.5.0").replace("aaaaaaa", "bbbbbbb");
    let d = diff("shard.lock", old, &new);
    assert_eq!(row(&d, "kemal").bump, Some(Bump::Minor));
    let r = row(&d, "radix");
    assert_eq!(r.kind, ChangeKind::Changed);
    assert_eq!(
        r.compare_url().as_deref(),
        Some("https://github.com/luislavena/radix/compare/aaaaaaa...bbbbbbb")
    );
}

#[test]
fn vcpkg_lock() {
    let old = r#"{"https://github.com/microsoft/vcpkg": {"HEAD": "1111111111111111111111111111111111111111"}}"#;
    let new = old.replace('1', "2");
    let d = diff("vcpkg-lock.json", old, &new);
    let v = row(&d, "https://github.com/microsoft/vcpkg");
    assert_eq!(v.kind, ChangeKind::Changed);
    assert!(v.compare_url().is_some());
}

#[test]
fn malformed_inputs_do_not_panic() {
    let deep = "[".repeat(100_000);
    let yaml_deep = (0..1000)
        .map(|i| format!("{}a:\n", " ".repeat(i)))
        .collect::<String>();
    let junk = [
        "",
        "\u{0}\u{1}",
        &deep,
        &yaml_deep,
        "{\"packages\": 5}",
        "%{\"a\": {:hex",
        "<<\"x",
        "provider \"x\" {",
        "a\n  b\n    c",
    ];
    for name in [
        "package-lock.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "bun.lock",
        "deno.lock",
        "Cargo.lock",
        "poetry.lock",
        "uv.lock",
        "pdm.lock",
        "Pipfile.lock",
        "go.sum",
        "Gemfile.lock",
        "composer.lock",
        "Podfile.lock",
        "Package.resolved",
        "Cartfile.resolved",
        "pubspec.lock",
        "mix.lock",
        "rebar.lock",
        "packages.lock.json",
        "gradle.lockfile",
        "flake.lock",
        "Chart.lock",
        ".terraform.lock.hcl",
        "conda-lock.yml",
        "pixi.lock",
        "renv.lock",
        "Manifest.toml",
        "stack.yaml.lock",
        "cabal.project.freeze",
        "shard.lock",
        "vcpkg-lock.json",
    ] {
        for j in junk {
            let _ = corvene_lockfiles::lockfile_diff(name, Some(j), Some(j));
            let _ = corvene_lockfiles::lockfile_diff(name, None, Some(j));
        }
    }
}
