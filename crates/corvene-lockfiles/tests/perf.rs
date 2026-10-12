//! A large package-lock diff stays fast.

use std::fmt::Write;
use std::time::{Duration, Instant};

fn package_lock(n: usize, bump_every: usize) -> String {
    let mut s = String::from(
        r#"{"name":"big","lockfileVersion":3,"packages":{"":{"name":"big","dependencies":{"#,
    );
    for i in 0..50 {
        let _ = write!(s, "{}\"pkg-{i}\":\"^1\"", if i > 0 { "," } else { "" });
    }
    s.push_str("}}");
    for i in 0..n {
        let patch = if bump_every > 0 && i % bump_every == 0 {
            1
        } else {
            0
        };
        let _ = write!(
            s,
            r#","node_modules/pkg-{i}":{{"version":"1.{}.{patch}","resolved":"https://registry.npmjs.org/pkg-{i}/-/pkg-{i}-1.0.0.tgz","integrity":"sha512-{i}","dependencies":{{"pkg-{}":"^1","@scope/dep-{}":"^2"}}}}"#,
            i % 7,
            (i + 1) % n,
            i % 13
        );
        if i % 10 == 0 {
            let _ = write!(
                s,
                r#","node_modules/pkg-{i}/node_modules/@scope/dep-{}":{{"version":"2.0.{patch}"}}"#,
                i % 13
            );
        }
    }
    s.push_str("}}");
    s
}

#[test]
fn three_thousand_packages() {
    let old = package_lock(3000, 0);
    let new = package_lock(3000, 3);
    let start = Instant::now();
    let d = corvene_lockfiles::lockfile_diff("package-lock.json", Some(&old), Some(&new)).unwrap();
    let elapsed = start.elapsed();
    assert!(elapsed < Duration::from_secs(2), "took {elapsed:?}");
    assert_eq!(d.counts().upgraded, 1000);
    // Nested copies gain a second version next to the unchanged one.
    assert_eq!(d.counts().added, 13);
    assert!(d.unchanged > 1000);
    // Direct rows sort first.
    assert_eq!(d.changes[0].direct, Some(true));
}
