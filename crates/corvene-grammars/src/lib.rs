//! Tree-sitter grammars and highlight queries for Corvene's opt-in tree-sitter
//! highlighter (`corvene_highlight::treesitter`, Settings › Appearance ›
//! Syntax highlighting; flag `105-tree-sitter-highlighting`).
//!
//! GitHub Desktop has no tree-sitter: its highlighter runs CodeMirror 5 modes,
//! which `corvene_highlight::cm` ports. This crate is the Corvene addition's
//! grammar set, built two ways from one table:
//!
//! - as an rlib, linked into the "full" build (`bundled-tree-sitter`);
//! - as a cdylib, shipped in the `tree-sitter-all` / `tree-sitter-rest` packs
//!   (`packaging/packs.sh`) and opened with `dlopen` by the default build.
//!
//! Both read the grammars through the same C-ABI table
//! ([`corvene_grammars_v1`]), so the loader has a single code path and the
//! pack does not depend on Rust's unstable ABI. Queries live in
//! `queries/<name>/` (synced by `tools/ts-queries/sync.py`, each file names
//! its source and license).

use std::ffi::c_char;
use std::sync::OnceLock;

/// Layout version of [`Table`] / [`Grammar`]. Bump on any field change; the
/// loader refuses a pack whose table has another version.
pub const ABI: u32 = 1;

macro_rules! pack_version {
    () => {
        "1.1.0"
    };
}

/// Version of the grammar packs (`packaging/packs.sh` reads it).
pub const PACK_VERSION: &str = pack_version!();

/// One grammar. Strings are NUL-terminated UTF-8; lists are `\n`-separated.
#[repr(C)]
pub struct Grammar {
    /// the grammar's name (`rust`, `markdown_inline`)
    pub name: *const c_char,
    /// the grammar's `tree_sitter_<name>` function
    pub language: unsafe extern "C" fn() -> *const (),
    pub highlights: *const c_char,
    pub injections: *const c_char,
    pub locals: *const c_char,
    /// lowercase suffixes after a dot, possibly with dots themselves (`d.ts`)
    pub extensions: *const c_char,
    /// lowercase exact file names (`makefile`, `.bashrc`)
    pub filenames: *const c_char,
    /// a regex over the first line (shebangs), empty for none
    pub first_line: *const c_char,
    /// names injections and code fences use for it besides `name`
    pub aliases: *const c_char,
    /// grammars its queries inject by name (kept in the same pack)
    pub injects: *const c_char,
}

/// Every grammar this build carries.
#[repr(C)]
pub struct Table {
    pub abi: u32,
    pub version: *const c_char,
    pub len: usize,
    pub grammars: *const Grammar,
}

struct Shared(Table, #[allow(dead_code)] Vec<Grammar>);

// SAFETY: the table only points at `'static` data (string literals, the
// grammar functions and the Vec it owns), never mutated after construction.
unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

/// What the table says about a grammar, without the grammar: see
/// [`GRAMMAR_INFO`].
pub struct Info {
    pub name: &'static str,
    pub extensions: &'static [&'static str],
    pub filenames: &'static [&'static str],
    pub first_line: &'static str,
    pub injects: &'static [&'static str],
}

#[allow(dead_code)]
const fn cstr(s: &'static str) -> *const c_char {
    s.as_ptr().cast()
}

macro_rules! grammars {
    ($(
        $(#[$attr:meta])*
        $name:literal ($feature:literal) => $language:expr,
        extensions: [$($ext:literal),* $(,)?],
        filenames: [$($file:literal),* $(,)?],
        first_line: $first:literal,
        aliases: [$($alias:literal),* $(,)?],
        injects: [$($inject:literal),* $(,)?];
    )*) => {
        /// Every grammar the crate knows with the Cargo feature that enables it.
        pub const GRAMMAR_FEATURES: &[(&str, &str)] = &[$(($name, $feature)),*];

        /// Every grammar the crate knows, whether or not this build carries
        /// it: the table leaves out a grammar whose feature is off, and one
        /// built from fetched sources when `target/grammar-src` lacks them
        /// (build.rs).
        pub const GRAMMAR_INFO: &[Info] = &[$(Info {
            name: $name,
            extensions: &[$($ext),*],
            filenames: &[$($file),*],
            first_line: $first,
            injects: &[$($inject),*],
        }),*];

        // one `push` per grammar, each behind its feature
        #[allow(clippy::vec_init_then_push)]
        fn build() -> Vec<Grammar> {
            #[allow(unused_mut)]
            let mut grammars = Vec::new();
            $(
                #[cfg(feature = $feature)]
                $(#[$attr])*
                grammars.push(Grammar {
                    name: cstr(concat!($name, "\0")),
                    language: $language.into_raw(),
                    highlights: cstr(concat!(
                        include_str!(concat!("../queries/", $name, "/highlights.scm")),
                        "\0"
                    )),
                    injections: cstr(concat!(
                        include_str!(concat!("../queries/", $name, "/injections.scm")),
                        "\0"
                    )),
                    locals: cstr(concat!(
                        include_str!(concat!("../queries/", $name, "/locals.scm")),
                        "\0"
                    )),
                    extensions: cstr(concat!($($ext, "\n",)* "\0")),
                    filenames: cstr(concat!($($file, "\n",)* "\0")),
                    first_line: cstr(concat!($first, "\0")),
                    aliases: cstr(concat!($($alias, "\n",)* "\0")),
                    injects: cstr(concat!($($inject, "\n",)* "\0")),
                });
            )*
            grammars
        }
    };
}

/// A `LanguageFn` for a grammar whose package has pre-0.23 bindings (no
/// `LanguageFn` constant): its C entry point, linked from the package.
macro_rules! c_language {
    ($symbol:ident) => {{
        unsafe extern "C" {
            fn $symbol() -> *const ();
        }
        // SAFETY: `$symbol` is the grammar's `tree_sitter_<name>` function.
        unsafe { tree_sitter_language::LanguageFn::from_raw($symbol) }
    }};
}

include!("grammars.rs");

fn shared() -> &'static Shared {
    static TABLE: OnceLock<Shared> = OnceLock::new();
    TABLE.get_or_init(|| {
        let grammars = build();
        Shared(
            Table {
                abi: ABI,
                version: cstr(concat!(pack_version!(), "\0")),
                len: grammars.len(),
                grammars: grammars.as_ptr(),
            },
            grammars,
        )
    })
}

/// The grammar table. The pack's dylib exports this symbol; the loader looks
/// it up with `dlsym` and checks [`Table::abi`] before reading anything else.
#[unsafe(no_mangle)]
pub extern "C" fn corvene_grammars_v1() -> *const Table {
    &shared().0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_string_matches_the_constant() {
        let table = unsafe { &*corvene_grammars_v1() };
        let version = unsafe { std::ffi::CStr::from_ptr(table.version) };
        assert_eq!(version.to_str(), Ok(PACK_VERSION));
        assert_eq!(table.abi, ABI);
    }

    #[test]
    fn rest_is_what_the_rest_feature_enables() {
        let manifest = include_str!("../Cargo.toml");
        let start = manifest.find("\nrest = [").expect("rest feature");
        let array = &manifest[start..];
        let array = &array[..array.find(']').expect("end of rest")];
        let mut features: Vec<String> = array
            .split('"')
            .skip(1)
            .step_by(2)
            .map(|f| f.to_string())
            .collect();
        features.sort();
        let mut crates: Vec<String> = REST
            .iter()
            .map(|name| {
                let (_, feature) = GRAMMAR_FEATURES
                    .iter()
                    .find(|(n, _)| n == name)
                    .expect("REST names a known grammar");
                feature.to_string()
            })
            .collect();
        crates.sort();
        crates.dedup();
        assert_eq!(features, crates);
    }
}
