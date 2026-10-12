//! The offline extension index: one JSON file, built by `tools/ext-index`
//! from the three registries and published with the packs, mapping file
//! suffixes and names to the extensions that cover them. The diff's "no
//! syntax highlighting" hint answers from it at once; the registries are
//! asked only when it has nothing.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::ExtensionError;
use crate::registry::Candidate;

/// The schema this build reads.
pub const SCHEMA: u32 = 1;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Index {
    pub schema: u32,
    /// RFC 3339
    #[serde(default)]
    pub generated: String,
    /// suffix (no dot, lowercase) → candidates, best first
    #[serde(default)]
    pub suffixes: HashMap<String, Vec<Candidate>>,
    /// whole file name (lowercase) → candidates
    #[serde(default)]
    pub filenames: HashMap<String, Vec<Candidate>>,
}

impl Index {
    /// Read `index.json` or `index.json.gz`.
    pub fn load(path: &Path) -> Result<Self, ExtensionError> {
        let bytes = std::fs::read(path)?;
        let text = if bytes.starts_with(&[0x1f, 0x8b]) {
            let mut out = String::new();
            flate2::read::GzDecoder::new(&bytes[..])
                .take(64 * 1024 * 1024)
                .read_to_string(&mut out)?;
            out
        } else {
            String::from_utf8(bytes)
                .map_err(|_| ExtensionError::parse("json", "the index is not UTF-8"))?
        };
        let index: Index = serde_json::from_str(&text)
            .map_err(|err| ExtensionError::parse("json", format!("the extension index: {err}")))?;
        if index.schema > SCHEMA {
            return Err(ExtensionError::parse(
                "json",
                "the extension index was written for a newer Corvene",
            ));
        }
        Ok(index)
    }

    /// Candidates for a file name or a suffix: the whole name first, then
    /// its suffixes from the longest (`d.ts` before `ts`).
    pub fn lookup(&self, name_or_suffix: &str) -> Vec<Candidate> {
        let (name, suffixes) = lookup_keys(name_or_suffix);
        let mut out: Vec<Candidate> = Vec::new();
        let mut push = |list: &[Candidate]| {
            for c in list {
                if !out.iter().any(|o| o.registry == c.registry && o.id == c.id) {
                    out.push(c.clone());
                }
            }
        };
        if let Some(list) = self.filenames.get(&name) {
            push(list);
        }
        for suffix in &suffixes {
            if let Some(list) = self.suffixes.get(suffix) {
                push(list);
            }
        }
        out
    }

    /// The names and suffixes that have candidates, without the
    /// candidates: what stays in memory between lookups.
    pub fn keys(&self) -> IndexKeys {
        IndexKeys {
            generated: self.generated.clone(),
            suffixes: self
                .suffixes
                .iter()
                .filter(|(_, list)| !list.is_empty())
                .map(|(k, _)| k.clone())
                .collect(),
            filenames: self
                .filenames
                .iter()
                .filter(|(_, list)| !list.is_empty())
                .map(|(k, _)| k.clone())
                .collect(),
        }
    }

    /// Days since `generated`, when it parses (date part only).
    pub fn age_days(&self, now_unix: u64) -> Option<u64> {
        age_days(&self.generated, now_unix)
    }
}

/// [`Index::keys`]: an index's names and suffixes. The candidates (about
/// 9 MB in memory for the published index) are read again from disk when
/// [`IndexKeys::knows`] says a lookup will find some.
#[derive(Clone, Debug, Default)]
pub struct IndexKeys {
    pub generated: String,
    pub suffixes: HashSet<String>,
    pub filenames: HashSet<String>,
}

impl IndexKeys {
    /// Whether [`Index::lookup`] finds candidates for `name_or_suffix`.
    pub fn knows(&self, name_or_suffix: &str) -> bool {
        let (name, suffixes) = lookup_keys(name_or_suffix);
        self.filenames.contains(&name) || suffixes.iter().any(|s| self.suffixes.contains(s))
    }

    /// [`Index::age_days`].
    pub fn age_days(&self, now_unix: u64) -> Option<u64> {
        age_days(&self.generated, now_unix)
    }
}

/// [`Index::lookup`]'s keys: the lowercase file name, and its suffixes from
/// the longest (a name without a dot is a suffix itself).
fn lookup_keys(name_or_suffix: &str) -> (String, Vec<String>) {
    let name = name_or_suffix
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name_or_suffix)
        .trim_start_matches('.')
        .to_ascii_lowercase();
    let parts: Vec<&str> = name.split('.').collect();
    let suffixes = (0..parts.len())
        .map(|start| parts[start..].join("."))
        .collect();
    (name, suffixes)
}

/// Days from the date part of an RFC 3339 `generated` to `now_unix`.
fn age_days(generated: &str, now_unix: u64) -> Option<u64> {
    let date = generated.get(..10)?;
    let mut it = date.split('-');
    let (y, m, d): (i64, i64, i64) = (
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
    );
    // days from civil (Howard Hinnant)
    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let now_days = (now_unix / 86_400) as i64;
    Some((now_days - days).max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{GrammarHint, Registry};

    fn candidate(registry: Registry, id: &str) -> Candidate {
        Candidate {
            registry,
            id: id.into(),
            name: id.into(),
            display_name: id.into(),
            publisher: None,
            version: Some("1".into()),
            description: None,
            repository: None,
            download_url: "https://example.invalid".into(),
            grammar: GrammarHint::TextMate,
            suffixes: Vec::new(),
            downloads: 0,
            icon_themes: false,
        }
    }

    #[test]
    fn looks_up_names_then_suffixes() {
        let mut index = Index {
            schema: SCHEMA,
            generated: "2026-10-01T00:00:00Z".into(),
            ..Default::default()
        };
        index
            .suffixes
            .insert("ts".into(), vec![candidate(Registry::OpenVsx, "a.ts")]);
        index
            .suffixes
            .insert("d.ts".into(), vec![candidate(Registry::Zed, "dts")]);
        index
            .filenames
            .insert("justfile".into(), vec![candidate(Registry::Zed, "just")]);
        let ids: Vec<String> = index
            .lookup("src/types.d.ts")
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(ids, vec!["dts", "a.ts"]);
        let ids: Vec<String> = index.lookup("Justfile").into_iter().map(|c| c.id).collect();
        assert_eq!(ids, vec!["just"]);
        assert!(index.lookup(".nothing").is_empty());
        assert_eq!(index.age_days(1_791_417_600), Some(7)); // 2026-10-08 - 2026-10-01
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("index.json");
        std::fs::write(&path, serde_json::to_string(&index).expect("json")).expect("write");
        assert_eq!(Index::load(&path).expect("load").suffixes.len(), 2);
    }

    #[test]
    fn keys_know_what_lookup_finds() {
        let mut index = Index {
            schema: SCHEMA,
            generated: "2026-10-01T00:00:00Z".into(),
            ..Default::default()
        };
        index
            .suffixes
            .insert("d.ts".into(), vec![candidate(Registry::Zed, "dts")]);
        index
            .filenames
            .insert("justfile".into(), vec![candidate(Registry::Zed, "just")]);
        index.suffixes.insert("empty".into(), Vec::new());
        let keys = index.keys();
        for name in [
            "src/types.d.ts",
            ".d.ts",
            "Justfile",
            "x.empty",
            "ts",
            ".nothing",
        ] {
            assert_eq!(keys.knows(name), !index.lookup(name).is_empty(), "{name}");
        }
        assert_eq!(keys.age_days(1_791_417_600), Some(7));
    }
}
