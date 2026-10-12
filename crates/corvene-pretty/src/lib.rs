//! Corvene `1339-pretty-diff`: pretty printers for minified files, so a
//! one-line bundle diffs as readable lines.
//!
//! GitHub Desktop diffs a minified `app.min.js` or `data.json` as one huge
//! changed line (`app/src/ui/diff/text-diff.tsx` has no reformatting step).
//! Corvene pretty prints both sides first and diffs the results, so the
//! printers here are deterministic and local: a small change in the input
//! moves only nearby output lines.
//!
//! Every printer only adds, removes or replaces whitespace between tokens:
//! the non-whitespace text of the input always survives in order (for
//! JavaScript, the token sequence survives). Nothing here allocates more
//! than a small multiple of the input or recurses on input structure.

mod css;
mod detect;
mod generic;
mod js;
mod json;
mod markup;
mod out;
#[cfg(test)]
mod tests;

pub use detect::{is_minified, is_minified_name};

/// A language this crate can pretty print.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    /// JSON (also source maps, web manifests, GeoJSON, HAR, JSON-LD).
    Json,
    /// JavaScript, including JSX.
    JavaScript,
    /// TypeScript (same token level printer as JavaScript).
    TypeScript,
    /// CSS.
    Css,
    /// SCSS (CSS printer with `//` line comments).
    Scss,
    /// Less (CSS printer with `//` line comments).
    Less,
    /// HTML (raw text elements kept verbatim or printed by their language).
    Html,
    /// XML and XML vocabularies (SVG, RSS, plist, ...).
    Xml,
}

impl Language {
    /// The language picked by `path`'s extension (case-insensitive).
    pub fn of_path(path: &str) -> Option<Language> {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let (_, ext) = name.rsplit_once('.')?;
        let ext = ext.to_ascii_lowercase();
        Some(match ext.as_str() {
            "json" | "map" | "webmanifest" | "geojson" | "topojson" | "har" | "jsonld" => {
                Language::Json
            }
            "js" | "mjs" | "cjs" | "jsx" => Language::JavaScript,
            "ts" | "mts" | "cts" | "tsx" => Language::TypeScript,
            "css" => Language::Css,
            "scss" => Language::Scss,
            "less" => Language::Less,
            "html" | "htm" | "xhtml" => Language::Html,
            "xml" | "svg" | "xsd" | "xsl" | "xslt" | "rss" | "atom" | "plist" | "xaml" | "kml"
            | "gpx" | "wsdl" | "resx" | "csproj" => Language::Xml,
            _ => return None,
        })
    }

    /// The name shown in the diff header.
    pub fn name(self) -> &'static str {
        match self {
            Language::Json => "JSON",
            Language::JavaScript => "JavaScript",
            Language::TypeScript => "TypeScript",
            Language::Css => "CSS",
            Language::Scss => "SCSS",
            Language::Less => "Less",
            Language::Html => "HTML",
            Language::Xml => "XML",
        }
    }
}

/// `text` pretty printed with 2-space indents, `\n` newlines and a trailing
/// newline; text that does not parse is printed as far as it can be, never
/// dropped. Blank input gives an empty string.
pub fn pretty(language: Language, text: &str) -> String {
    let normalized;
    let mut text = text;
    if text.contains('\r') {
        normalized = text.replace("\r\n", "\n");
        text = &normalized;
    }
    let (bom, body) = match text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text),
    };
    let printed = match language {
        Language::Json => json::pretty(body).unwrap_or_else(|| generic::pretty(body)),
        Language::JavaScript | Language::TypeScript => {
            js::pretty(body, language == Language::TypeScript)
        }
        Language::Css => css::pretty(body, false),
        Language::Scss | Language::Less => css::pretty(body, true),
        Language::Html => markup::pretty(body, true),
        Language::Xml => markup::pretty(body, false),
    };
    let mut result = String::with_capacity(printed.len() + bom.len() + 1);
    result.push_str(bom);
    result.push_str(printed.trim_end_matches(['\n', ' ']));
    if !result.is_empty() {
        result.push('\n');
    }
    result
}
