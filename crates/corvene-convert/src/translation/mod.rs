//! Corvene `1352-translation-diff`: string tables as a table with a row per
//! key, so a changed translation shows as one changed cell instead of
//! lines moving around (and a catalog with many languages, like Xcode's
//! `.xcstrings`, gets a column per language).
//!
//! Formats: gettext `.po`/`.pot` and compiled `.mo` ([`po`]), Apple
//! `.strings`, `.stringsdict` and `.xcstrings` ([`apple`]), XLIFF 1.2 and
//! 2.0 ([`xliff`]), Android `values*/strings.xml` and .NET `.resx`
//! ([`xml`]), Qt `.ts` and `.qm` ([`qt`]), Flutter `.arb`, Java
//! `.properties` and nested JSON message files under a `locales`-like
//! folder ([`simple`]).
//!
//! The first column is the key: the table diff pairs a changed row with
//! its old version by it.

pub mod apple;
pub mod po;
pub mod qt;
pub mod simple;
pub mod xliff;
pub mod xml;

use corvene_models::{TableBlob, TableSheet};

use crate::Format;

/// Rows kept per table; the rest are left out (marked truncated).
pub const MAX_ROWS: usize = 50_000;

/// The table of one side.
pub fn to_table(format: Format, bytes: &[u8]) -> Result<TableBlob, String> {
    let sheets = match format {
        Format::Po => po::po(&text(bytes)?)?,
        Format::Mo => po::mo(bytes)?,
        Format::AppleStrings => apple::strings(bytes)?,
        Format::StringsDict => apple::stringsdict(bytes)?,
        Format::XcStrings => apple::xcstrings(&text(bytes)?)?,
        Format::Xliff => xliff::xliff(&text(bytes)?)?,
        Format::AndroidStrings => xml::android(&text(bytes)?)?,
        Format::Resx => xml::resx(&text(bytes)?)?,
        Format::QtTs => qt::ts(&text(bytes)?)?,
        Format::QtQm => qt::qm(bytes)?,
        Format::Arb => simple::arb(&text(bytes)?)?,
        Format::Properties => simple::properties(&text(bytes)?)?,
        Format::LocaleJson => simple::locale_json(&text(bytes)?)?,
        _ => return Err("Not a string table.".into()),
    };
    if sheets.is_empty() {
        return Err("No strings were found.".into());
    }
    Ok(TableBlob { sheets })
}

/// `bytes` as text: UTF-8 (a byte order mark dropped), or UTF-16 with a
/// byte order mark or the zero bytes of mostly-ASCII UTF-16.
pub(crate) fn text(bytes: &[u8]) -> Result<String, String> {
    if let Some(rest) = bytes.strip_prefix(b"\xef\xbb\xbf") {
        return Ok(String::from_utf8_lossy(rest).into_owned());
    }
    let utf16 = |le: bool, body: &[u8]| {
        let units: Vec<u16> = body
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| {
                if le {
                    u16::from_le_bytes([c[0], c[1]])
                } else {
                    u16::from_be_bytes([c[0], c[1]])
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    if let Some(rest) = bytes.strip_prefix(b"\xff\xfe") {
        return Ok(utf16(true, rest));
    }
    if let Some(rest) = bytes.strip_prefix(b"\xfe\xff") {
        return Ok(utf16(false, rest));
    }
    if bytes.len() >= 4 {
        let sample = &bytes[..bytes.len().min(512) & !1];
        let odd_zeros = sample
            .iter()
            .skip(1)
            .step_by(2)
            .filter(|b| **b == 0)
            .count();
        let even_zeros = sample.iter().step_by(2).filter(|b| **b == 0).count();
        let half = sample.len() / 2;
        if odd_zeros * 10 > half * 8 && even_zeros == 0 {
            return Ok(utf16(true, bytes));
        }
        if even_zeros * 10 > half * 8 && odd_zeros == 0 {
            return Ok(utf16(false, bytes));
        }
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => Ok(s.to_string()),
        Err(_) => {
            let (text, _, _) = encoding_rs::WINDOWS_1252.decode(bytes);
            Ok(text.into_owned())
        }
    }
}

/// A table under construction: a header, then rows; columns that stay
/// empty in every row (other than the key) are dropped at the end.
pub(crate) struct Builder {
    name: String,
    header: Vec<String>,
    rows: Vec<Vec<String>>,
    truncated: bool,
}

impl Builder {
    pub(crate) fn new(name: impl Into<String>, header: &[&str]) -> Self {
        Builder {
            name: name.into(),
            header: header.iter().map(|h| h.to_string()).collect(),
            rows: Vec::new(),
            truncated: false,
        }
    }

    /// A header made at run time (one column per language).
    pub(crate) fn with_header(name: impl Into<String>, header: Vec<String>) -> Self {
        Builder {
            name: name.into(),
            header,
            rows: Vec::new(),
            truncated: false,
        }
    }

    pub(crate) fn push(&mut self, mut row: Vec<String>) {
        if self.rows.len() >= MAX_ROWS {
            self.truncated = true;
            return;
        }
        row.resize(self.header.len(), String::new());
        self.rows.push(row);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub(crate) fn finish(self) -> TableSheet {
        let keep: Vec<bool> = (0..self.header.len())
            .map(|c| c == 0 || self.rows.iter().any(|r| !r[c].is_empty()))
            .collect();
        let pick = |row: Vec<String>| -> Vec<String> {
            row.into_iter()
                .zip(&keep)
                .filter(|(_, k)| **k)
                .map(|(v, _)| v)
                .collect()
        };
        let mut rows = Vec::with_capacity(self.rows.len() + 1);
        rows.push(pick(self.header));
        rows.extend(self.rows.into_iter().map(pick));
        TableSheet {
            name: self.name,
            first_line: 0,
            rows,
            truncated: self.truncated,
        }
    }
}

/// Whether a lowercase repository path looks like a nested-JSON message
/// file: a `.json` file in a `locales`/`i18n`/`lang`/`translations`
/// folder, named for a language (`de.json`, `pt-BR.json`) or inside a
/// language's folder (`locales/de/common.json`, `_locales/de/messages.json`).
pub fn is_locale_json_path(lower: &str) -> bool {
    let parts: Vec<&str> = lower.split('/').collect();
    let Some((file, dirs)) = parts.split_last() else {
        return false;
    };
    let Some(stem) = file.strip_suffix(".json") else {
        return false;
    };
    let i18n = |d: &&str| {
        matches!(
            *d,
            "locales"
                | "locale"
                | "_locales"
                | "i18n"
                | "l10n"
                | "lang"
                | "langs"
                | "languages"
                | "translations"
                | "messages"
        )
    };
    let Some(at) = dirs.iter().rposition(i18n) else {
        return false;
    };
    let below = &dirs[at + 1..];
    (below.is_empty() && is_language_code(stem)) || (below.len() == 1 && is_language_code(below[0]))
}

/// `en`, `de-at`, `pt_br`, `zh-hans`, `sr-latn-rs`.
fn is_language_code(s: &str) -> bool {
    let mut parts = s.split(['-', '_']);
    let Some(first) = parts.next() else {
        return false;
    };
    (2..=3).contains(&first.len())
        && first.bytes().all(|b| b.is_ascii_lowercase())
        && parts.all(|p| (2..=4).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// Whether a lowercase repository path is an Android string resource file
/// (`res/values-fr/strings.xml`, `values/plurals.xml`).
pub fn is_android_strings_path(lower: &str) -> bool {
    let mut parts = lower.rsplit('/');
    let (Some(file), Some(dir)) = (parts.next(), parts.next()) else {
        return false;
    };
    (dir == "values" || dir.starts_with("values-"))
        && file.ends_with(".xml")
        && (file.starts_with("strings")
            || file.starts_with("plurals")
            || file.starts_with("arrays"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_paths() {
        assert!(is_locale_json_path("src/locales/de.json"));
        assert!(is_locale_json_path("public/locales/pt-br/common.json"));
        assert!(is_locale_json_path("_locales/fr/messages.json"));
        assert!(is_locale_json_path("i18n/zh-hans.json"));
        assert!(!is_locale_json_path("locales/index.json"));
        assert!(!is_locale_json_path("src/de.json"));
        assert!(!is_locale_json_path("package.json"));
    }

    #[test]
    fn android_paths() {
        assert!(is_android_strings_path(
            "app/src/main/res/values/strings.xml"
        ));
        assert!(is_android_strings_path("res/values-fr-rca/strings.xml"));
        assert!(!is_android_strings_path("res/layout/strings.xml"));
        assert!(!is_android_strings_path("res/values/colors.xml"));
    }

    #[test]
    fn utf16_text_is_found() {
        let le: Vec<u8> = "\"a\" = \"b\";"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(text(&le).unwrap(), "\"a\" = \"b\";");
        let mut bom = vec![0xfe, 0xff];
        bom.extend("x".encode_utf16().flat_map(u16::to_be_bytes));
        assert_eq!(text(&bom).unwrap(), "x");
    }

    #[test]
    fn empty_columns_are_dropped() {
        let mut b = Builder::new("t", &["Key", "Context", "Value"]);
        b.push(vec!["a".into(), String::new(), "1".into()]);
        let sheet = b.finish();
        assert_eq!(sheet.rows, vec![vec!["Key", "Value"], vec!["a", "1"]]);
    }
}
