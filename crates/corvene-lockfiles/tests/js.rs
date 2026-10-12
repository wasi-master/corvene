//! npm, Yarn, pnpm, Bun and Deno lockfiles.

mod common;

use common::{absent, diff, names, row};
use corvene_models::lockfile::{Bump, ChangeKind, Ecosystem, SourceKind};

const NPM_OLD: &str = r#"{
  "name": "app", "version": "1.0.0", "lockfileVersion": 2, "requires": true,
  "packages": {
    "": {
      "name": "app", "version": "1.0.0",
      "dependencies": {"react": "^18.2.0", "lodash": "^4.17.0", "my-alias": "npm:left-pad@^1.3.0", "local-lib": "file:packages/local-lib", "gitdep": "github:acme/gitdep"},
      "devDependencies": {"@types/node": "^20.0.0"}
    },
    "node_modules/react": {"version": "18.2.0", "resolved": "https://registry.npmjs.org/react/-/react-18.2.0.tgz", "integrity": "sha512-A", "dependencies": {"loose-envify": "^1.1.0"}},
    "node_modules/loose-envify": {"version": "1.4.0", "integrity": "sha512-B", "dependencies": {"js-tokens": "^4.0.0"}},
    "node_modules/js-tokens": {"version": "4.0.0", "integrity": "sha512-C"},
    "node_modules/lodash": {"version": "4.17.20", "integrity": "sha512-D"},
    "node_modules/@types/node": {"version": "20.1.0", "dev": true, "integrity": "sha512-E"},
    "node_modules/my-alias": {"name": "left-pad", "version": "1.3.0", "resolved": "https://registry.npmjs.org/left-pad/-/left-pad-1.3.0.tgz", "integrity": "sha512-F"},
    "node_modules/foo": {"version": "1.0.0", "integrity": "sha512-G", "dependencies": {"bar": "^2"}},
    "node_modules/bar": {"version": "1.0.0", "integrity": "sha512-H"},
    "node_modules/foo/node_modules/bar": {"version": "2.0.0", "integrity": "sha512-I"},
    "node_modules/local-lib": {"resolved": "packages/local-lib", "link": true},
    "packages/local-lib": {"version": "0.1.0"},
    "node_modules/gitdep": {"version": "1.0.0", "resolved": "git+ssh://git@github.com/acme/gitdep.git#aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}
  },
  "dependencies": {"react": {"version": "18.2.0"}}
}"#;

const NPM_NEW: &str = r#"{
  "name": "app", "version": "1.0.0", "lockfileVersion": 3, "requires": true,
  "packages": {
    "": {
      "name": "app", "version": "1.0.0",
      "dependencies": {"react": "^18.2.0", "lodash": "^4.17.0", "my-alias": "npm:left-pad@^1.3.0", "local-lib": "file:packages/local-lib", "gitdep": "github:acme/gitdep"},
      "devDependencies": {"@types/node": "^20.0.0"}
    },
    "node_modules/react": {"version": "18.3.1", "resolved": "https://registry.npmjs.org/react/-/react-18.3.1.tgz", "integrity": "sha512-A2", "dependencies": {"loose-envify": "^1.1.0", "chalk": "^5"}},
    "node_modules/loose-envify": {"version": "1.4.0", "integrity": "sha512-TAMPERED"},
    "node_modules/chalk": {"version": "5.3.0", "integrity": "sha512-J"},
    "node_modules/lodash": {"version": "4.17.21", "integrity": "sha512-D2"},
    "node_modules/@types/node": {"version": "20.2.0", "dev": true, "integrity": "sha512-E2"},
    "node_modules/my-alias": {"name": "left-pad", "version": "1.3.0", "resolved": "https://registry.npmjs.org/left-pad/-/left-pad-1.3.0.tgz", "integrity": "sha512-F"},
    "node_modules/foo": {"version": "1.0.0", "integrity": "sha512-G", "dependencies": {"bar": "^2"}},
    "node_modules/bar": {"version": "1.0.1", "integrity": "sha512-H2"},
    "node_modules/foo/node_modules/bar": {"version": "2.0.0", "integrity": "sha512-I"},
    "node_modules/local-lib": {"resolved": "packages/local-lib", "link": true},
    "packages/local-lib": {"version": "0.1.0"},
    "node_modules/gitdep": {"version": "1.0.0", "resolved": "git+ssh://git@github.com/acme/gitdep.git#bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}
  }
}"#;

#[test]
fn npm_v2_to_v3() {
    let d = diff("web/package-lock.json", NPM_OLD, NPM_NEW);
    assert_eq!(d.format, "package-lock.json");
    assert_eq!(d.summary(), "4 upgraded, 1 added, 1 removed, 2 changed");
    assert!(
        d.notes.iter().any(|n| n == "lockfileVersion 2 → 3"),
        "{:?}",
        d.notes
    );
    assert_eq!(
        names(&d, ChangeKind::Upgraded),
        ["@types/node", "lodash", "react", "bar"]
    );

    let react = row(&d, "react");
    assert_eq!(react.version_label(), "18.2.0 → 18.3.1");
    assert_eq!(react.bump, Some(Bump::Minor));
    assert_eq!(react.direct, Some(true));
    assert_eq!(
        react.registry_url().as_deref(),
        Some("https://www.npmjs.com/package/react/v/18.3.1")
    );

    let types = row(&d, "@types/node");
    assert!(types.dev);
    assert_eq!(
        types.registry_url().as_deref(),
        Some("https://www.npmjs.com/package/@types/node/v/20.2.0")
    );

    // Two copies of bar: only the moved one shows.
    let bar = row(&d, "bar");
    assert_eq!(bar.version_label(), "1.0.0 → 1.0.1");
    assert_eq!(bar.bump, Some(Bump::Patch));
    assert_eq!(bar.direct, Some(false));

    let le = row(&d, "loose-envify");
    assert_eq!(le.kind, ChangeKind::Changed);
    assert!(le.integrity_changed && le.is_warning());
    assert_eq!(le.required_by, ["react"]);

    let chalk = row(&d, "chalk");
    assert_eq!(chalk.kind, ChangeKind::Added);
    assert_eq!(chalk.required_by, ["react"]);
    assert_eq!(row(&d, "js-tokens").kind, ChangeKind::Removed);
    assert_eq!(row(&d, "js-tokens").required_by, ["loose-envify"]);

    let git = row(&d, "gitdep");
    assert_eq!(git.kind, ChangeKind::Changed);
    assert!(git.source_changed());
    assert!(!git.integrity_changed);
    assert_eq!(
        git.new_source.as_ref().map(|s| s.kind),
        Some(SourceKind::Git)
    );
    assert_eq!(
        git.compare_url().as_deref(),
        Some(
            "https://github.com/acme/gitdep/compare/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa...bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        )
    );
    assert_eq!(
        git.registry_url().as_deref(),
        Some("https://github.com/acme/gitdep")
    );

    // Alias kept under its real name, unchanged; link unchanged.
    assert!(absent(&d, "left-pad") && absent(&d, "my-alias") && absent(&d, "local-lib"));
    // left-pad, foo, bar@2, local-lib.
    assert_eq!(d.unchanged, 4);
    assert!(d.weight() > 0);
}

#[test]
fn npm_v1_nested_and_alias() {
    let old = r#"{"name":"x","lockfileVersion":1,"dependencies":{
        "a": {"version": "1.0.0", "integrity": "sha1-a", "requires": {"b": "^1"}},
        "b": {"version": "1.0.0", "dev": true},
        "c": {"version": "1.0.0", "dependencies": {"b": {"version": "2.0.0"}}},
        "pad": {"version": "npm:left-pad@1.1.0"}
    }}"#;
    let new = r#"{"name":"x","lockfileVersion":1,"dependencies":{
        "a": {"version": "1.0.0", "integrity": "sha1-a", "requires": {"b": "^1"}},
        "b": {"version": "1.2.0", "dev": true},
        "c": {"version": "1.0.0", "dependencies": {"b": {"version": "3.0.0-beta.1"}}},
        "pad": {"version": "npm:left-pad@1.3.0"}
    }}"#;
    let d = diff("package-lock.json", old, new);
    let rows: Vec<_> = d
        .changes
        .iter()
        .map(|c| (c.name.as_str(), c.version_label()))
        .collect();
    assert!(rows.contains(&("b", "1.0.0 → 1.2.0".into())), "{rows:?}");
    assert!(
        rows.contains(&("b", "2.0.0 → 3.0.0-beta.1".into())),
        "{rows:?}"
    );
    assert_eq!(row(&d, "left-pad").version_label(), "1.1.0 → 1.3.0");
    assert_eq!(d.unchanged, 2);
    // v1 does not say which are direct.
    assert_eq!(row(&d, "left-pad").direct, None);
}

#[test]
fn npm_added_and_deleted_file() {
    let added = corvene_lockfiles::lockfile_diff("package-lock.json", None, Some(NPM_NEW)).unwrap();
    assert!(!added.old_present && added.new_present);
    assert_eq!(added.counts().added, added.new_package_count);
    assert!(added.changes.iter().all(|c| c.kind == ChangeKind::Added));
    let deleted =
        corvene_lockfiles::lockfile_diff("package-lock.json", Some(NPM_OLD), None).unwrap();
    assert!(
        deleted
            .changes
            .iter()
            .all(|c| c.kind == ChangeKind::Removed)
    );
    assert_eq!(deleted.changes.len(), deleted.old_package_count);
}

#[test]
fn unreadable_side_gets_a_note() {
    let d =
        corvene_lockfiles::lockfile_diff("package-lock.json", Some("{ not json"), Some(NPM_NEW))
            .unwrap();
    assert!(d.notes[0].contains("old version"), "{:?}", d.notes);
    assert!(corvene_lockfiles::lockfile_diff("package-lock.json", Some("{"), Some("[")).is_none());
    assert!(corvene_lockfiles::lockfile_diff("README.md", Some("a"), Some("b")).is_none());
}

const YARN_V1_OLD: &str = r#"# THIS IS AN AUTOGENERATED FILE. DO NOT EDIT THIS FILE DIRECTLY.
# yarn lockfile v1


"@babel/code-frame@^7.0.0", "@babel/code-frame@^7.10.4":
  version "7.12.13"
  resolved "https://registry.yarnpkg.com/@babel/code-frame/-/code-frame-7.12.13.tgz#dcfc826beef65e75c50e21d3837d7d95798dd658"
  integrity sha512-HV1Cm0Q3ZrpCR93tkWOYiuYIgLxZXZFVG2VgK+MBWjUqZTundupbfx2aXarXuw5Ko5aMcjtJgbSs4vUGBS5v6g==
  dependencies:
    "@babel/highlight" "^7.12.13"

"@babel/highlight@^7.12.13":
  version "7.13.10"
  resolved "https://registry.yarnpkg.com/@babel/highlight/-/highlight-7.13.10.tgz#a8b2a66148f5b27d666b15d81774347a731d52d1"
  integrity sha512-old
  dependencies:
    js-tokens "^4.0.0"

js-tokens@^4.0.0:
  version "4.0.0"
  resolved "https://registry.yarnpkg.com/js-tokens/-/js-tokens-4.0.0.tgz#19203fb59991df98e3a287050d4647cdeaf32499"
  integrity sha512-RdJUflcE3cUzKiMqQgsCu06FPu9UdIJO0beYbPhHN4k6apgJtifcoCtT9bcxOpYBtpD2kCM6Sbzg4CausW/PKQ==

"string-width-cjs@npm:string-width@^4.2.0":
  version "4.2.3"
  resolved "https://registry.yarnpkg.com/string-width/-/string-width-4.2.3.tgz#269c7117d27b05ad2e536830a8ec895ef9c6d010"
  integrity sha512-sw

my-fork@acme/my-fork:
  version "1.0.0"
  resolved "git+https://github.com/acme/my-fork.git#1111111111111111111111111111111111111111"
"#;

#[test]
fn yarn_classic() {
    let new = YARN_V1_OLD
        .replace("version \"7.13.10\"", "version \"7.14.0\"")
        .replace("integrity sha512-sw", "integrity sha512-sw-changed")
        .replace(
            "1111111111111111111111111111111111111111",
            "2222222222222222222222222222222222222222",
        );
    let d = diff("yarn.lock", YARN_V1_OLD, &new);
    assert_eq!(d.format, "yarn.lock (classic)");
    let h = row(&d, "@babel/highlight");
    assert_eq!(h.version_label(), "7.13.10 → 7.14.0");
    assert_eq!(h.bump, Some(Bump::Minor));
    assert_eq!(h.required_by, ["@babel/code-frame"]);
    // The alias is listed under the real package name.
    let sw = row(&d, "string-width");
    assert!(sw.integrity_changed);
    let fork = row(&d, "my-fork");
    assert_eq!(fork.kind, ChangeKind::Changed);
    assert!(
        fork.compare_url()
            .unwrap()
            .starts_with("https://github.com/acme/my-fork/compare/1111")
    );
    assert_eq!(d.unchanged, 2);
}

const BERRY: &str = r#"# This file is generated by running "yarn install" inside your project.
# Manual changes might be lost - proceed with caution!

__metadata:
  version: 6
  cacheKey: 8

"@types/react@npm:^18.0.0, @types/react@npm:^18.2.0":
  version: 18.2.0
  resolution: "@types/react@npm:18.2.0"
  dependencies:
    csstype: ^3.0.2
  checksum: aaaa
  languageName: node
  linkType: hard

"csstype@npm:^3.0.2":
  version: 3.1.2
  resolution: "csstype@npm:3.1.2"
  checksum: bbbb
  languageName: node
  linkType: hard

"resolve@patch:resolve@npm%3A^1.22.1#~builtin<compat/resolve>":
  version: 1.22.1
  resolution: "resolve@patch:resolve@npm%3A1.22.1#~builtin<compat/resolve>::version=1.22.1&hash=c3c19d"
  checksum: cccc
  languageName: node
  linkType: hard

"resolve@npm:^1.22.1":
  version: 1.22.1
  resolution: "resolve@npm:1.22.1"
  checksum: dddd
  languageName: node
  linkType: hard

"root@workspace:.":
  version: 0.0.0-use.local
  resolution: "root@workspace:."
  dependencies:
    "@types/react": ^18.2.0
    resolve: ^1.22.1
  languageName: unknown
  linkType: soft
"#;

#[test]
fn yarn_berry() {
    let new = BERRY
        .replace("version: 6\n", "version: 8\n")
        .replace(
            "version: 18.2.0\n  resolution: \"@types/react@npm:18.2.0\"",
            "version: 18.3.3\n  resolution: \"@types/react@npm:18.3.3\"",
        )
        .replace("hash=c3c19d", "hash=07638b")
        .replace("checksum: bbbb", "checksum: bbbb2");
    let d = diff("yarn.lock", BERRY, &new);
    assert_eq!(d.format, "yarn.lock (Berry)");
    assert_eq!(
        d.format_version_note().as_deref(),
        Some("Format version 6 → 8")
    );
    let t = row(&d, "@types/react");
    assert_eq!(t.direct, Some(true));
    assert_eq!(t.bump, Some(Bump::Minor));
    let c = row(&d, "csstype");
    assert!(c.integrity_changed);
    assert_eq!(c.direct, Some(false));
    assert_eq!(c.required_by, ["@types/react"]);
    // The patched copy changed its patch hash; the plain copy did not.
    let r = row(&d, "resolve");
    assert_eq!(r.kind, ChangeKind::Changed);
    assert!(r.source_changed());
    assert_eq!(
        r.new_source.as_ref().and_then(|s| s.rev.as_deref()),
        Some("07638b")
    );
}

const PNPM_V5: &str = "lockfileVersion: 5.4

specifiers:
  react: ^18.2.0
  '@types/react': ^18.0.0

dependencies:
  react: 18.2.0

devDependencies:
  '@types/react': 18.0.28

packages:

  /react/18.2.0:
    resolution: {integrity: sha512-react}
    dependencies:
      loose-envify: 1.4.0
    dev: false

  /loose-envify/1.4.0:
    resolution: {integrity: sha512-le}
    dev: false

  /@types/react/18.0.28:
    resolution: {integrity: sha512-types}
    dev: true

  /react-dom/18.2.0_react@18.2.0:
    resolution: {integrity: sha512-dom}
    peerDependencies:
      react: ^18.2.0
    dev: false
";

#[test]
fn pnpm_v5() {
    let new = PNPM_V5
        .replace("/@types/react/18.0.28", "/@types/react/18.2.7")
        .replace("'@types/react': 18.0.28", "'@types/react': 18.2.7")
        .replace(
            "/react-dom/18.2.0_react@18.2.0",
            "/react-dom/18.3.0_react@18.2.0",
        );
    let d = diff("pnpm-lock.yaml", PNPM_V5, &new);
    let t = row(&d, "@types/react");
    assert_eq!(t.version_label(), "18.0.28 → 18.2.7");
    assert!(t.dev);
    assert_eq!(t.direct, Some(true));
    // Peer suffix stripped.
    assert_eq!(row(&d, "react-dom").version_label(), "18.2.0 → 18.3.0");
    assert_eq!(d.unchanged, 2);
}

const PNPM_V6: &str = "lockfileVersion: '6.0'

importers:
  .:
    dependencies:
      react:
        specifier: ^18.2.0
        version: 18.2.0
    devDependencies:
      typescript:
        specifier: ^5.0.0
        version: 5.0.4

packages:

  /react@18.2.0:
    resolution: {integrity: sha512-react}
    dependencies:
      loose-envify: 1.4.0
    dev: false

  /loose-envify@1.4.0:
    resolution: {integrity: sha512-le}
    dev: false

  /typescript@5.0.4:
    resolution: {integrity: sha512-ts}
    dev: true

  /use-sync@1.2.0(react@18.2.0):
    resolution: {integrity: sha512-us}
    dev: false

  github.com/acme/widget/1111111:
    resolution: {tarball: https://codeload.github.com/acme/widget/tar.gz/1111111}
    name: widget
    version: 0.1.0
    dev: false
";

#[test]
fn pnpm_v6() {
    let new = PNPM_V6
        .replace("5.0.4", "5.4.2")
        .replace(
            "/use-sync@1.2.0(react@18.2.0)",
            "/use-sync@1.2.0(react@18.3.0)",
        )
        .replace("1111111", "2222222")
        .replace("sha512-le", "sha512-le2");
    let d = diff("pnpm-lock.yaml", PNPM_V6, &new);
    assert_eq!(row(&d, "typescript").bump, Some(Bump::Minor));
    assert!(row(&d, "typescript").dev);
    assert!(row(&d, "loose-envify").integrity_changed);
    assert_eq!(row(&d, "loose-envify").required_by, ["react"]);
    // Only the peer suffix moved: not a change.
    assert!(absent(&d, "use-sync"));
    let w = row(&d, "widget");
    assert_eq!(w.kind, ChangeKind::Changed);
    assert_eq!(
        w.compare_url().as_deref(),
        Some("https://github.com/acme/widget/compare/1111111...2222222")
    );
}

const PNPM_V9: &str = "lockfileVersion: '9.0'

settings:
  autoInstallPeers: true

importers:

  .:
    dependencies:
      '@tanstack/react-query':
        specifier: ^5.0.0
        version: 5.17.0(react@18.2.0)
      react:
        specifier: ^18.2.0
        version: 18.2.0

packages:

  '@tanstack/query-core@5.17.0':
    resolution: {integrity: sha512-core}

  '@tanstack/react-query@5.17.0':
    resolution: {integrity: sha512-rq}
    peerDependencies:
      react: ^18.0.0

  react@18.2.0:
    resolution: {integrity: sha512-react}

snapshots:

  '@tanstack/query-core@5.17.0': {}

  '@tanstack/react-query@5.17.0(react@18.2.0)':
    dependencies:
      '@tanstack/query-core': 5.17.0
      react: 18.2.0

  react@18.2.0: {}
";

#[test]
fn pnpm_v9_snapshots() {
    let new = PNPM_V9.replace("5.17.0", "5.18.1");
    let d = diff("pnpm-lock.yaml", PNPM_V9, &new);
    assert_eq!(d.summary(), "2 upgraded");
    assert_eq!(
        names(&d, ChangeKind::Upgraded),
        ["@tanstack/react-query", "@tanstack/query-core"]
    );
    assert_eq!(
        row(&d, "@tanstack/query-core").required_by,
        ["@tanstack/react-query"]
    );
    assert_eq!(row(&d, "@tanstack/query-core").direct, Some(false));
}

#[test]
fn bun_lock_jsonc() {
    let old = r#"{
  "lockfileVersion": 1,
  "workspaces": {
    "": {
      "name": "app",
      "dependencies": { "zod": "^3.22.0", },
      "devDependencies": { "@types/bun": "latest", },
    },
  },
  "packages": {
    "zod": ["zod@3.22.4", "", {}, "sha512-zod"],
    "@types/bun": ["@types/bun@1.0.0", "", { "dependencies": { "bun-types": "1.0.0" } }, "sha512-tb"],
    "bun-types": ["bun-types@1.0.0", "", {}, "sha512-bt"],
    // a comment
    "tiny": ["tiny@github:acme/tiny#abc1234", {}, "acme-tiny-abc1234"],
  }
}"#;
    let new = old.replace("zod@3.22.4", "zod@3.23.8").replace(
        "bun-types@1.0.0\", \"\", {}, \"sha512-bt",
        "bun-types@1.0.0\", \"\", {}, \"sha512-bt2",
    );
    let d = diff("bun.lock", old, &new);
    let z = row(&d, "zod");
    assert_eq!(z.direct, Some(true));
    assert_eq!(z.bump, Some(Bump::Minor));
    let bt = row(&d, "bun-types");
    assert!(bt.integrity_changed);
    assert_eq!(bt.required_by, ["@types/bun"]);
    assert!(!corvene_lockfiles::is_lockfile("bun.lockb"));
}

#[test]
fn deno_v3_and_v4() {
    let v3 = r#"{
  "version": "3",
  "packages": {
    "specifiers": { "jsr:@std/path@^0.220": "jsr:@std/path@0.220.1", "npm:chalk@5": "npm:chalk@5.3.0" },
    "jsr": { "@std/path@0.220.1": { "integrity": "p1" } },
    "npm": { "chalk@5.3.0": { "integrity": "c1", "dependencies": {} } }
  },
  "remote": {
    "https://deno.land/std@0.200.0/path/mod.ts": "aa",
    "https://deno.land/std@0.200.0/fs/mod.ts": "bb"
  },
  "workspace": { "dependencies": ["jsr:@std/path@^0.220", "npm:chalk@5"] }
}"#;
    let v4 = r#"{
  "version": "4",
  "specifiers": { "jsr:@std/path@^1": "1.0.8", "npm:chalk@5": "5.3.0" },
  "jsr": { "@std/path@1.0.8": { "integrity": "p2" } },
  "npm": { "chalk@5.3.0": { "integrity": "c1" } },
  "remote": { "https://deno.land/std@0.224.0/path/mod.ts": "cc" },
  "workspace": { "dependencies": ["jsr:@std/path@^1", "npm:chalk@5"] }
}"#;
    let d = diff("deno.lock", v3, v4);
    let p = row(&d, "@std/path");
    assert_eq!(p.ecosystem, Ecosystem::Jsr);
    assert_eq!(p.bump, Some(Bump::Major));
    assert_eq!(p.direct, Some(true));
    assert_eq!(
        p.registry_url().as_deref(),
        Some("https://jsr.io/@std/path@1.0.8")
    );
    let std = row(&d, "https://deno.land/std");
    assert_eq!(std.version_label(), "0.200.0 → 0.224.0");
    assert!(absent(&d, "chalk"));
    assert!(d.notes.iter().any(|n| n.contains("3 → 4")));
}
