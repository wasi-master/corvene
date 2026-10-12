//! Version ordering and bump sizes.

use std::cmp::Ordering::{Equal, Greater, Less};

use corvene_lockfiles::version::{bump, compare};
use corvene_models::lockfile::Bump;

fn lt(a: &str, b: &str) {
    assert_eq!(compare(a, b), Some(Less), "{a} < {b}");
    assert_eq!(compare(b, a), Some(Greater), "{b} > {a}");
}

#[test]
fn semver_order() {
    // The semver.org precedence example.
    let chain = [
        "1.0.0-alpha",
        "1.0.0-alpha.1",
        "1.0.0-alpha.beta",
        "1.0.0-beta",
        "1.0.0-beta.2",
        "1.0.0-beta.11",
        "1.0.0-rc.1",
        "1.0.0",
        "1.0.1",
        "1.1.0",
        "2.0.0",
        "10.0.0",
    ];
    for w in chain.windows(2) {
        lt(w[0], w[1]);
    }
    assert_eq!(compare("1.0.0+build.1", "1.0.0+build.2"), Some(Equal));
    assert_eq!(compare("v1.2.3", "1.2.3"), Some(Equal));
    assert_eq!(compare("1.0", "1.0.0"), Some(Equal));
}

#[test]
fn tolerant_orders() {
    // PEP 440.
    lt("1.0.dev1", "1.0a1");
    lt("1.0a1", "1.0b2");
    lt("1.0b2", "1.0rc1");
    lt("1.0rc1", "1.0");
    lt("1.0", "1.0.post1");
    lt("2.0", "1!0.5");
    // RubyGems.
    lt("7.1.0.rc1", "7.1.0");
    lt("1.16.0", "1.16.0.1");
    // Go pseudo-versions.
    lt(
        "v0.0.0-20220722155237-a158d28d115b",
        "v0.0.0-20230101000000-abcdefabcdef",
    );
    lt("v0.0.0-20230101000000-abcdefabcdef", "v0.1.0");
    lt("v1.2.4-0.20230101000000-abcdefabcdef", "v1.2.4");
    lt("v1.2.3", "v1.2.4-0.20230101000000-abcdefabcdef");
    // Maven.
    lt("1.0-SNAPSHOT", "1.0");
    lt("31.1-jre", "32.1.3-jre");
    lt("1.9.0-RC", "1.9.0");
    lt("1.0.0-M1", "1.0.0-RC1");
    // Numeric, not lexical.
    lt("1.9", "1.10");
    lt("2023.9.1", "2024.1.0");
    // Huge numbers saturate instead of overflowing.
    assert!(compare("99999999999999999999999999.0", "1.0").is_some());
}

#[test]
fn non_versions_do_not_order() {
    assert_eq!(compare("main", "1.0"), None);
    assert_eq!(compare("1a2b3c4d", "2b3c4d5e"), None);
    assert_eq!(compare("1111111111111", "2222222222222"), None);
    assert_eq!(compare("", "1"), None);
}

#[test]
fn bumps() {
    assert_eq!(bump("1.0.200", "1.0.210"), Bump::Patch);
    assert_eq!(bump("1.2.0", "1.3.0"), Bump::Minor);
    assert_eq!(bump("1.9.9", "2.0.0"), Bump::Major);
    assert_eq!(bump("1.0.0-beta.1", "1.0.0"), Bump::Prerelease);
    assert_eq!(bump("1.2.3.4", "1.2.3.5"), Bump::Patch);
    assert_eq!(bump("1.0", "1!1.0"), Bump::Major);
    assert_eq!(bump("abcdef0", "1234567"), Bump::Other);
    assert_eq!(bump("2024-01-01", "2024-02-01"), Bump::Other);
}
