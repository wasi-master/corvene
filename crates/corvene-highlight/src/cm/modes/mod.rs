//! Ported CodeMirror modes and GHD's file → MIME tables
//! (`app/src/highlighter/index.ts` `extensionModes`, `guessMimeType`).
//!
//! To port a mode: add `<name>.rs` exposing a constructor, list its MIME
//! types in [`mode_for_mime`] and add golden fixtures (`tools/cm-oracle`).
//!
//! Deviation (`765-more-highlight-extensions`): `.jsonc`, `.slnx` and the
//! MQL extensions (`.mq4`, `.mq5`, `.mqh`) map to JSON, XML and C++.

pub mod asciiarmor;
pub mod clike;
pub mod clojure;
pub mod cmake;
pub mod coffeescript;
pub mod crystal;
pub mod css;
pub mod cypher;
pub mod dart;
pub mod diff;
pub mod dockerfile;
pub mod elixir;
pub mod fortran;
pub mod go;
pub mod haml;
pub mod handlebars;
pub mod haxe;
pub mod htmlembedded;
pub mod htmlmixed;
pub mod javascript;
mod js_syntax;
pub mod jsx;
pub mod julia;
pub mod luau;
pub mod markdown;
pub mod mllike;
pub mod multiplex;
pub mod oz;
pub mod pascal;
pub mod perl;
pub mod php;
pub mod pig;
pub mod powershell;
pub mod properties;
pub mod protobuf;
pub mod pug;
pub mod puppet;
pub mod python;
pub mod q;
pub mod r;
pub mod rpm;
pub mod rst;
pub mod ruby;
pub mod sass;
pub mod scheme;
pub mod shell;
pub mod sieve;
pub mod slim;
pub mod smalltalk;
pub mod soy;
pub mod sparql;
pub mod sql;
pub mod stex;
pub mod stylus;
pub mod swift;
pub mod toml;
pub mod vb;
pub mod vue;
pub mod xml;
pub mod yaml;
pub mod zig;

use std::sync::{Arc, OnceLock};

use super::Mode;
use super::simple::{SimpleMode, r, r0, rg};

/// `extensionMIMEMap` (lower-cased extension with the dot).
pub fn mime_for_extension(ext: &str) -> Option<&'static str> {
    Some(match ext {
        ".ts" => "text/typescript",
        ".mts" => "text/typescript",
        ".cts" => "text/typescript",
        ".js" => "text/javascript",
        ".mjs" => "text/javascript",
        ".cjs" => "text/javascript",
        ".json" => "application/json",
        ".coffee" => "text/x-coffeescript",
        ".tsx" => "text/typescript-jsx",
        ".mtsx" => "text/typescript-jsx",
        ".ctsx" => "text/typescript-jsx",
        ".jsx" => "text/jsx",
        ".mjsx" => "text/jsx",
        ".cjsx" => "text/jsx",
        ".html" => "text/html",
        ".htm" => "text/html",
        ".astro" => "text/html",
        ".aspx" => "application/x-aspx",
        ".cshtml" => "application/x-aspx",
        ".jsp" => "application/x-jsp",
        ".css" => "text/css",
        ".scss" => "text/x-scss",
        ".less" => "text/x-less",
        ".vue" => "text/x-vue",
        // GHD 3.6.8 (desktop/desktop#22969)
        ".svelte" => "text/x-vue",
        ".markdown" => "text/x-markdown",
        ".md" => "text/x-markdown",
        ".mdx" => "text/x-markdown",
        ".yaml" => "text/yaml",
        ".yml" => "text/yaml",
        ".xml" => "text/xml",
        ".xaml" => "text/xml",
        ".xsd" => "text/xml",
        ".csproj" => "text/xml",
        ".fsproj" => "text/xml",
        ".vcxproj" => "text/xml",
        ".vbproj" => "text/xml",
        ".svg" => "text/xml",
        ".resx" => "text/xml",
        ".props" => "text/xml",
        ".targets" => "text/xml",
        ".diff" => "text/x-diff",
        ".patch" => "text/x-diff",
        ".m" => "text/x-objectivec",
        ".scala" => "text/x-scala",
        ".sc" => "text/x-scala",
        ".cs" => "text/x-csharp",
        ".cake" => "text/x-csharp",
        ".java" => "text/x-java",
        ".c" => "text/x-c",
        ".h" => "text/x-c",
        ".cpp" => "text/x-c++src",
        ".hpp" => "text/x-c++src",
        ".cc" => "text/x-c++src",
        ".hh" => "text/x-c++src",
        ".hxx" => "text/x-c++src",
        ".cxx" => "text/x-c++src",
        ".ino" => "text/x-c++src",
        ".kt" => "text/x-kotlin",
        ".ml" => "text/x-ocaml",
        ".fs" => "text/x-fsharp",
        ".fsx" => "text/x-fsharp",
        ".fsi" => "text/x-fsharp",
        ".swift" => "text/x-swift",
        ".sh" => "text/x-sh",
        ".sql" => "text/x-sql",
        ".cql" => "application/x-cypher-query",
        ".go" => "text/x-go",
        ".pl" => "text/x-perl",
        ".php" => "application/x-httpd-php",
        ".py" => "text/x-python",
        ".pyi" => "text/x-python",
        ".vpy" => "text/x-python",
        ".rb" => "text/x-ruby",
        ".clj" => "text/x-clojure",
        ".cljc" => "text/x-clojure",
        ".cljs" => "text/x-clojure",
        ".edn" => "text/x-clojure",
        ".rs" => "text/x-rustsrc",
        ".ex" => "text/x-elixir",
        ".exs" => "text/x-elixir",
        ".hx" => "text/x-haxe",
        ".r" => "text/x-rsrc",
        ".ps1" => "application/x-powershell",
        ".vb" => "text/x-vb",
        ".f" => "text/x-fortran",
        ".f90" => "text/x-fortran",
        ".lua" => "text/x-lua",
        ".luau" => "text/x-luau",
        ".cr" => "text/x-crystal",
        ".jl" => "text/x-julia",
        ".tex" => "text/x-stex",
        ".rq" => "application/sparql-query",
        ".styl" => "text/x-styl",
        ".soy" => "text/x-soy",
        ".st" => "text/x-stsrc",
        ".slim" => "application/x-slim",
        ".haml" => "text/x-haml",
        ".sieve" => "application/sieve",
        ".ss" => "text/x-scheme",
        ".sls" => "text/x-scheme",
        ".scm" => "text/x-scheme",
        ".rst" => "text/x-rst",
        ".rpm" => "text/x-rpm-spec",
        ".q" => "text/x-q",
        ".pp" => "text/x-puppet",
        ".pug" => "text/x-pug",
        ".proto" => "text/x-protobuf",
        ".properties" => "text/x-properties",
        ".gitattributes" => "text/x-properties",
        ".gitignore" => "text/x-properties",
        ".editorconfig" => "text/x-properties",
        ".ini" => "text/x-ini",
        ".pig" => "text/x-pig",
        ".pgp" => "application/pgp",
        ".oz" => "text/x-oz",
        ".pas" => "text/x-pascal",
        ".toml" => "text/x-toml",
        ".dart" => "application/dart",
        ".zig" => "text/x-zig",
        ".cmake" => "text/x-cmake",
        _ => return extra_mime_for_extension(ext),
    })
}

/// Corvene `765-more-highlight-extensions`: extensions GHD's map lacks.
static EXTRA_EXTENSIONS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// `765-more-highlight-extensions`: map [`extra_mime_for_extension`]'s
/// extensions too (set by the app from the flag).
pub fn set_extra_extensions(on: bool) {
    EXTRA_EXTENSIONS.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// JSON with comments, the XML solution format and MetaQuotes' C++-like
/// MQL sources (desktop/desktop#22663, #20861).
fn extra_mime_for_extension(ext: &str) -> Option<&'static str> {
    if !EXTRA_EXTENSIONS.load(std::sync::atomic::Ordering::Relaxed) {
        return None;
    }
    Some(match ext {
        ".jsonc" => "application/json",
        ".slnx" => "text/xml",
        ".mq4" | ".mq5" | ".mqh" => "text/x-c++src",
        _ => return None,
    })
}

/// `basenameMIMEMap` (lower-cased file name).
pub fn mime_for_basename(name: &str) -> Option<&'static str> {
    Some(match name {
        "cargo.lock" => "text/x-toml",
        "dockerfile" => "text/x-dockerfile",
        _ => return None,
    })
}

/// `guessMimeType`: `<?xml`, or a shebang naming ts-node / node / sh / bash
/// / python (`/^#!.*?(ts-node|node|bash|sh|python(?:[\d.]+)?)/`).
pub fn guess_mime(first_line: &str) -> Option<&'static str> {
    if first_line.starts_with("<?xml") {
        return Some("text/xml");
    }
    let rest = first_line.strip_prefix("#!")?;
    // the lazy `.*?` takes the earliest position; the alternation order
    // decides between words starting there
    for (i, _) in rest.char_indices() {
        for word in ["ts-node", "node", "bash", "sh", "python"] {
            if rest[i..].starts_with(word) {
                return Some(match word {
                    "ts-node" => "text/typescript",
                    "node" => "text/javascript",
                    "bash" | "sh" => "text/x-sh",
                    _ => "text/x-python",
                });
            }
        }
    }
    None
}

/// One shared instance of a mode per call site.
macro_rules! cached {
    ($mode:expr) => {{
        static MODE: OnceLock<Arc<dyn Mode>> = OnceLock::new();
        MODE.get_or_init(|| Arc::new($mode)).clone()
    }};
}

/// The ported mode for a MIME type (`CodeMirror.getMode({}, mime)`).
pub fn mode_for_mime(mime: &str) -> Option<Arc<dyn Mode>> {
    match mime {
        "text/x-rustsrc" | "text/rust" => Some(rust()),
        "text/css" | "text/x-scss" | "text/x-less" | "text/x-gss" => css::css_for_mime(mime),
        "text/yaml" | "text/x-yaml" => Some(yaml::yaml()),
        "text/x-toml" => Some(toml::toml()),
        "text/x-python" => Some(cached!(python::Python::new())),
        "text/x-cython" => Some(cached!(python::Python::cython())),
        "text/x-ruby" => Some(cached!(ruby::Ruby)),
        "text/x-perl" => Some(cached!(perl::Perl)),
        "application/x-powershell" => Some(cached!(powershell::PowerShell)),
        "text/x-rsrc" => Some(cached!(r::R)),
        "text/x-sh" | "application/x-sh" => Some(cached!(shell::Shell)),
        "text/x-go" => Some(Arc::new(go::Go)),
        "text/x-swift" => Some(Arc::new(swift::Swift)),
        "text/x-diff" => Some(Arc::new(diff::Diff)),
        "text/x-dockerfile" => Some(dockerfile::dockerfile()),
        "text/x-zig" => Some(Arc::new(zig::Zig)),
        "text/x-cmake" => Some(Arc::new(cmake::Cmake)),
        "text/x-protobuf" => Some(Arc::new(protobuf::Protobuf)),
        "text/x-pug" | "text/x-jade" => Some(pug::pug()),
        "text/x-haml" => Some(haml::haml()),
        "text/x-slim" | "application/x-slim" => Some(slim::slim()),
        "text/x-soy" => Some(soy::soy()),
        "text/x-sass" => Some(Arc::new(sass::Sass)),
        "text/x-handlebars-template" => Some(handlebars::handlebars()),
        "text/x-properties" | "text/x-ini" => Some(Arc::new(properties::Properties)),
        "text/x-stex" | "text/x-latex" => Some(Arc::new(stex::Stex::new())),
        "text/x-pascal" => Some(Arc::new(pascal::Pascal)),
        "text/x-scheme" => Some(Arc::new(scheme::Scheme)),
        "text/x-puppet" => Some(Arc::new(puppet::Puppet)),
        "application/sparql-query" => Some(Arc::new(sparql::Sparql)),
        "application/x-cypher-query" => Some(Arc::new(cypher::Cypher)),
        "text/x-stsrc" => Some(Arc::new(smalltalk::Smalltalk)),
        "text/x-q" => Some(Arc::new(q::Q)),
        "text/x-pig" => Some(Arc::new(pig::Pig)),
        "application/sieve" => Some(Arc::new(sieve::Sieve)),
        "text/x-rpm-spec" => Some(Arc::new(rpm::RpmSpec)),
        "text/x-rpm-changes" => Some(Arc::new(rpm::RpmChanges)),
        "text/x-oz" => Some(Arc::new(oz::Oz)),
        "text/x-rst" => Some(cached!(rst::Rst::new())),
        "application/pgp"
        | "application/pgp-encrypted"
        | "application/pgp-keys"
        | "application/pgp-signature" => Some(Arc::new(asciiarmor::AsciiArmor)),
        mime if sql::MIMES.contains(&mime) => sql::sql(mime),
        "text/x-csrc" | "text/x-c" | "text/x-chdr" => Some(clike::c()),
        "text/x-c++src" | "text/x-c++hdr" => Some(clike::cpp()),
        "text/x-java" => Some(clike::java()),
        "text/x-csharp" => Some(clike::csharp()),
        "text/x-scala" => Some(clike::scala()),
        "text/x-kotlin" => Some(clike::kotlin()),
        "x-shader/x-vertex" | "x-shader/x-fragment" => Some(clike::shader()),
        "text/x-nesc" => Some(clike::nesc()),
        "text/x-objectivec" => Some(clike::objectivec()),
        "text/x-objectivec++" => Some(clike::objectivecpp()),
        "text/x-squirrel" => Some(clike::squirrel()),
        "text/x-ceylon" => Some(clike::ceylon()),
        "application/dart" => Some(dart::dart()),
        "text/x-ocaml" => Some(cached!(mllike::MlLike::new(mllike::Dialect::OCaml))),
        "text/x-fsharp" => Some(cached!(mllike::MlLike::new(mllike::Dialect::FSharp))),
        "text/x-sml" => Some(cached!(mllike::MlLike::new(mllike::Dialect::Sml))),
        "text/x-vb" => Some(cached!(vb::Vb)),
        "text/x-haxe" => Some(cached!(haxe::Haxe)),
        "text/x-hxml" => Some(cached!(haxe::Hxml)),
        "text/x-crystal" => Some(cached!(crystal::Crystal)),
        "text/x-fortran" => Some(cached!(fortran::Fortran)),
        "text/x-clojure" | "text/x-clojurescript" | "application/edn" => {
            Some(cached!(clojure::Clojure::new()))
        }
        "text/x-elixir" => Some(cached!(elixir::Elixir)),
        "text/x-julia" => Some(cached!(julia::Julia)),
        "text/x-lua" | "text/x-luau" => Some(cached!(luau::Luau)),
        "text/javascript"
        | "text/ecmascript"
        | "application/javascript"
        | "application/x-javascript"
        | "application/ecmascript" => Some(javascript()),
        "application/json" | "application/x-json" | "application/manifest+json" => Some(json()),
        "application/ld+json" => Some(jsonld()),
        "text/typescript" | "application/typescript" => Some(typescript()),
        // htmlmixed.js redefines xml.js's text/html
        "text/html" => Some(htmlmixed()),
        "application/x-ejs" | "application/x-aspx" | "application/x-jsp" | "application/x-erb" => {
            htmlembedded::for_mime(mime)
        }
        "application/x-httpd-php" => Some(php::php()),
        "application/x-httpd-php-open" => Some(php::php_open()),
        "text/x-php" => Some(php::x_php()),
        "text/xml" | "application/xml" => Some(xml()),
        "text/jsx" => Some(jsx()),
        "text/markdown" | "text/x-markdown" => Some(markdown()),
        "application/vnd.coffeescript" | "text/x-coffeescript" | "text/coffeescript" => {
            Some(cached!(coffeescript::CoffeeScript))
        }
        "text/x-styl" => Some(cached!(stylus::Stylus)),
        "text/x-vue" | "script/x-vue" => Some(vue::vue()),
        "text/typescript-jsx" => Some(typescript_jsx()),
        _ => None,
    }
}

macro_rules! shared_mode {
    ($(#[$doc:meta])* $name:ident, $make:expr) => {
        $(#[$doc])*
        pub fn $name() -> Arc<dyn Mode> {
            static MODE: OnceLock<Arc<dyn Mode>> = OnceLock::new();
            MODE.get_or_init(|| Arc::new($make)).clone()
        }
    };
}

shared_mode!(
    /// `codemirror/mode/javascript/javascript.js` (`text/javascript`)
    javascript,
    javascript::JsMode::new(javascript::JsConfig::default())
);
shared_mode!(
    /// `{name: "javascript", json: true}`
    json,
    javascript::JsMode::new(javascript::JsConfig {
        json: true,
        ..Default::default()
    })
);
shared_mode!(
    /// `{name: "javascript", jsonld: true}`
    jsonld,
    javascript::JsMode::new(javascript::JsConfig {
        jsonld: true,
        ..Default::default()
    })
);
shared_mode!(
    /// `{name: "javascript", typescript: true}`
    typescript,
    javascript::JsMode::new(javascript::JsConfig {
        typescript: true,
        ..Default::default()
    })
);
shared_mode!(
    /// `codemirror/mode/xml/xml.js` (`text/xml`)
    xml,
    xml::XmlMode::new(xml::XmlConfig::xml())
);
shared_mode!(
    /// `{name: "xml", htmlMode: true}`: xml.js's own `text/html` definition,
    /// which markdown gets when htmlmixed is not loaded
    xml_html,
    xml::XmlMode::new(xml::XmlConfig::html())
);
shared_mode!(
    /// `codemirror/mode/htmlmixed/htmlmixed.js` (`text/html`)
    htmlmixed,
    htmlmixed::HtmlMixed::new(Default::default(), html_modes)
);

/// `getMode` for a spec nested in HTML when the worker loaded htmlmixed.js
/// and what it requires: xml, javascript and css (plus core's `null`).
pub fn html_modes(spec: &str) -> Option<Arc<dyn Mode>> {
    match spec {
        "javascript"
        | "text/javascript"
        | "text/ecmascript"
        | "application/javascript"
        | "application/x-javascript"
        | "application/ecmascript" => Some(javascript()),
        "application/json" | "application/x-json" | "application/manifest+json" => Some(json()),
        "application/ld+json" => Some(jsonld()),
        "text/typescript" | "application/typescript" => Some(typescript()),
        "css" | "text/css" | "text/x-scss" | "text/x-less" | "text/x-gss" => {
            css::css_for_mime(if spec == "css" { "text/css" } else { spec })
        }
        "xml" | "text/xml" | "application/xml" => Some(xml()),
        "htmlmixed" | "text/html" => Some(htmlmixed()),
        // resolveMode's `+xml` / `+json` fallbacks
        _ if crate::re!(r"^[A-Za-z0-9_\-]+\/[A-Za-z0-9_\-]+\+xml$")
            .is_match(spec)
            .unwrap_or(false) =>
        {
            Some(xml())
        }
        _ if crate::re!(r"^[A-Za-z0-9_\-]+\/[A-Za-z0-9_\-]+\+json$")
            .is_match(spec)
            .unwrap_or(false) =>
        {
            Some(json())
        }
        _ => None,
    }
}
shared_mode!(
    /// `codemirror/mode/markdown/markdown.js` (`text/markdown`,
    /// `text/x-markdown`)
    markdown,
    markdown::Markdown::new()
);
shared_mode!(
    /// `codemirror/mode/jsx/jsx.js` (`text/jsx`)
    jsx,
    jsx::JsxMode::new(javascript::JsConfig::default())
);
shared_mode!(
    /// `{name: "jsx", base: {name: "javascript", typescript: true}}`
    typescript_jsx,
    jsx::JsxMode::new(javascript::JsConfig {
        typescript: true,
        ..Default::default()
    })
);

/// `codemirror/mode/rust/rust.js`
pub fn rust() -> Arc<dyn Mode> {
    static MODE: OnceLock<Arc<dyn Mode>> = OnceLock::new();
    MODE.get_or_init(|| {
        Arc::new(SimpleMode::new(
            "rust",
            vec![
                (
                    "start",
                    vec![
                        r(r##"b?""##, "string").next("string"),
                        r(r##"b?r""##, "string").next("string_raw"),
                        r(r##"b?r#+""##, "string").next("string_raw_hash"),
                        r(
                            r##"'(?:[^'\\]|\\(?:[nrt0'"]|x[\da-fA-F]{2}|u\{[\da-fA-F]{6}\}))'"##,
                            "string-2",
                        ),
                        r(r##"b'(?:[^']|\\(?:['\\nrt0]|x[\da-fA-F]{2}))'"##, "string-2"),
                        r(
                            r"(?:(?:[0-9][0-9_]*)(?:(?:[Ee][+-]?[0-9_]+)|\.[0-9_]+(?:[Ee][+-]?[0-9_]+)?)(?:f32|f64)?)|(?:0(?:b[01_]+|(?:o[0-7_]+)|(?:x[0-9a-fA-F_]+))|(?:[0-9][0-9_]*))(?:u8|u16|u32|u64|i8|i16|i32|i64|isize|usize)?",
                            "number",
                        ),
                        rg(
                            r"(let(?:\s+mut)?|fn|enum|mod|struct|type|union)(\s+)([a-zA-Z_][a-zA-Z0-9_]*)",
                            &[Some("keyword"), None, Some("def")],
                        ),
                        r(
                            r"(?:abstract|alignof|as|async|await|box|break|continue|const|crate|do|dyn|else|enum|extern|fn|for|final|if|impl|in|loop|macro|match|mod|move|offsetof|override|priv|proc|pub|pure|ref|return|self|sizeof|static|struct|super|trait|type|typeof|union|unsafe|unsized|use|virtual|where|while|yield)\b",
                            "keyword",
                        ),
                        r(
                            r"\b(?:Self|isize|usize|char|bool|u8|u16|u32|u64|f16|f32|f64|i8|i16|i32|i64|str|Option)\b",
                            "atom",
                        ),
                        r(r"\b(?:true|false|Some|None|Ok|Err)\b", "builtin"),
                        rg(
                            r"\b(fn)(\s+)([a-zA-Z_][a-zA-Z0-9_]*)",
                            &[Some("keyword"), None, Some("def")],
                        ),
                        r(r"#!?\[.*\]", "meta"),
                        r(r"\/\/.*", "comment"),
                        r(r"\/\*", "comment").next("comment"),
                        r(r"[-+\/*=<>!]+", "operator"),
                        r(r"[a-zA-Z_]\w*!", "variable-3"),
                        r(r"[a-zA-Z_]\w*", "variable"),
                        r0(r"[\{\[\(]"),
                        r0(r"[\}\]\)]"),
                    ],
                ),
                (
                    "string",
                    vec![
                        r(r##"""##, "string").next("start"),
                        r(r##"(?:[^\\"]|\\(?:.|$))*"##, "string"),
                    ],
                ),
                (
                    "string_raw",
                    vec![r(r##"""##, "string").next("start"), r(r##"[^"]*"##, "string")],
                ),
                (
                    "string_raw_hash",
                    vec![
                        r(r##""#+"##, "string").next("start"),
                        r(r##"(?:[^"]|"(?!#))*"##, "string"),
                    ],
                ),
                (
                    "comment",
                    vec![r(r".*?\*\/", "comment").next("start"), r(r".*", "comment")],
                ),
            ],
        ))
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_extensions_map_to_existing_modes() {
        // on by default (the app sets it from `765-more-highlight-extensions`)
        assert_eq!(mime_for_extension(".jsonc"), Some("application/json"));
        assert_eq!(mime_for_extension(".slnx"), Some("text/xml"));
        assert_eq!(mime_for_extension(".mq5"), Some("text/x-c++src"));
        assert_eq!(mime_for_extension(".nope"), None);
        for mime in ["application/json", "text/xml", "text/x-c++src"] {
            assert!(mode_for_mime(mime).is_some(), "{mime}");
        }
    }
}
