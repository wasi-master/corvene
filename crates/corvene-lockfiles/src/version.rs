//! Version ordering: semver with pre-release ordering, tolerant of PEP 440
//! (`1.0a1`, `1.0.post1`, epochs), RubyGems (`1.0.0.rc1`), Go
//! pseudo-versions and Maven qualifiers (`1.0-SNAPSHOT`, `31.1-jre`).

use std::cmp::Ordering;

use corvene_models::lockfile::Bump;

#[derive(Debug, PartialEq, Eq)]
enum Item {
    Num(u64),
    Word(i8, String),
}

#[derive(Debug)]
struct Parsed {
    epoch: u64,
    release: Vec<u64>,
    tail: Vec<Item>,
}

const MAX_ITEMS: usize = 32;

fn num(s: &str) -> u64 {
    s.bytes().fold(0u64, |acc, b| {
        acc.saturating_mul(10).saturating_add(u64::from(b - b'0'))
    })
}

fn word_rank(w: &str) -> i8 {
    match w {
        "dev" => -5,
        "alpha" | "a" => -4,
        "beta" | "b" => -3,
        "milestone" | "m" => -2,
        "rc" | "c" | "cr" | "pre" | "preview" | "snapshot" => -1,
        "final" | "ga" | "release" => 0,
        "post" | "p" | "sp" | "rev" => 1,
        _ => -1,
    }
}

/// A commit id (`1a2b3c4`, `20211230`): not a version.
fn looks_like_rev(s: &str) -> bool {
    s.len() >= 7 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn parse(s: &str) -> Option<Parsed> {
    let mut s = s.trim();
    if looks_like_rev(s) {
        return None;
    }
    if let Some(r) = s.strip_prefix(['v', 'V'])
        && r.starts_with(|c: char| c.is_ascii_digit())
    {
        s = r;
    }
    // Build metadata (semver) / local version (PEP 440) does not order.
    let s = s.split('+').next().unwrap_or(s);
    let mut epoch = 0;
    let mut s = s;
    if let Some((e, rest)) = s.split_once('!')
        && !e.is_empty()
        && e.bytes().all(|b| b.is_ascii_digit())
    {
        epoch = num(e);
        s = rest;
    }
    let b = s.as_bytes();
    let mut i = 0;
    let mut release = Vec::new();
    loop {
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            break;
        }
        if release.len() < MAX_ITEMS {
            release.push(num(&s[start..i]));
        }
        if i + 1 < b.len() && b[i] == b'.' && b[i + 1].is_ascii_digit() {
            i += 1;
        } else {
            break;
        }
    }
    if release.is_empty() {
        return None;
    }
    let mut tail = Vec::new();
    let rest = &s[i..];
    let rb = rest.as_bytes();
    let mut j = 0;
    while j < rb.len() && tail.len() < MAX_ITEMS {
        let c = rb[j];
        if c.is_ascii_digit() {
            let st = j;
            while j < rb.len() && rb[j].is_ascii_digit() {
                j += 1;
            }
            tail.push(Item::Num(num(&rest[st..j])));
        } else if c.is_ascii_alphabetic() {
            let st = j;
            while j < rb.len() && rb[j].is_ascii_alphabetic() {
                j += 1;
            }
            let w = rest[st..j].to_ascii_lowercase();
            tail.push(Item::Word(word_rank(&w), w));
        } else {
            j += 1;
        }
    }
    // `1.2.3-0.2021...` (semver numeric pre-release, Go pseudo-versions).
    if matches!(tail.first(), Some(Item::Num(_))) {
        tail.insert(0, Item::Word(-1, String::new()));
    }
    Some(Parsed {
        epoch,
        release,
        tail,
    })
}

fn cmp_item(a: &Item, b: &Item) -> Ordering {
    match (a, b) {
        (Item::Num(x), Item::Num(y)) => x.cmp(y),
        (Item::Word(r1, w1), Item::Word(r2, w2)) => r1.cmp(r2).then_with(|| w1.cmp(w2)),
        (Item::Num(_), Item::Word(..)) => Ordering::Less,
        (Item::Word(..), Item::Num(_)) => Ordering::Greater,
    }
}

/// Where a missing tail item sorts against `item`.
fn missing_vs(item: &Item) -> Ordering {
    match item {
        Item::Num(_) => Ordering::Less,
        Item::Word(r, _) => 0.cmp(r),
    }
}

fn cmp_parsed(a: &Parsed, b: &Parsed) -> Ordering {
    let o = a.epoch.cmp(&b.epoch);
    if o != Ordering::Equal {
        return o;
    }
    let n = a.release.len().max(b.release.len());
    for k in 0..n {
        let x = a.release.get(k).copied().unwrap_or(0);
        let y = b.release.get(k).copied().unwrap_or(0);
        if x != y {
            return x.cmp(&y);
        }
    }
    let n = a.tail.len().max(b.tail.len());
    for k in 0..n {
        let o = match (a.tail.get(k), b.tail.get(k)) {
            (Some(x), Some(y)) => cmp_item(x, y),
            (None, Some(y)) => missing_vs(y),
            (Some(x), None) => missing_vs(x).reverse(),
            (None, None) => Ordering::Equal,
        };
        if o != Ordering::Equal {
            return o;
        }
    }
    Ordering::Equal
}

/// Orders two versions; `None` when either is not a dotted-number version
/// (a git revision, a branch name).
pub fn compare(a: &str, b: &str) -> Option<Ordering> {
    Some(cmp_parsed(&parse(a)?, &parse(b)?))
}

/// `YYYY-MM-DD` (flake inputs are versioned by date).
fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// The size of the step from `a` to `b`.
pub fn bump(a: &str, b: &str) -> Bump {
    if is_date(a) || is_date(b) {
        return Bump::Other;
    }
    let (Some(x), Some(y)) = (parse(a), parse(b)) else {
        return Bump::Other;
    };
    if x.epoch != y.epoch {
        return Bump::Major;
    }
    let n = x.release.len().max(y.release.len());
    for k in 0..n {
        let p = x.release.get(k).copied().unwrap_or(0);
        let q = y.release.get(k).copied().unwrap_or(0);
        if p != q {
            return match k {
                0 => Bump::Major,
                1 => Bump::Minor,
                _ => Bump::Patch,
            };
        }
    }
    if x.tail != y.tail {
        Bump::Prerelease
    } else {
        Bump::Other
    }
}

/// Distance between two versions for pairing several versions of one
/// package (major, minor, patch differences).
pub(crate) fn distance(a: &str, b: &str) -> (u64, u64, u64) {
    let (Some(x), Some(y)) = (parse(a), parse(b)) else {
        return (u64::MAX, u64::MAX, u64::MAX);
    };
    let d = |k: usize| {
        let p = x.release.get(k).copied().unwrap_or(0);
        let q = y.release.get(k).copied().unwrap_or(0);
        p.abs_diff(q)
    };
    (d(0), d(1), d(2))
}
