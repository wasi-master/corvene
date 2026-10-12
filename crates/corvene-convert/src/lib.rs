//! Corvene `1350-compiled-code-diff`, `1351-serialized-data-diff`,
//! `1352-translation-diff`, `1353-diagram-diff`, `1354-map-diff` and
//! `1355-email-diff`: files whose line or byte diff says little, read into
//! a form that diffs well.
//!
//! - compiled code (class files, Python bytecode, WebAssembly, native
//!   binaries) and serialized data (binary property lists, MessagePack,
//!   CBOR, BSON, protobuf, bencode) and mail become text;
//! - string tables (gettext, Apple, Android, XLIFF, Qt, Flutter, Java,
//!   .NET) become a table with a row per key;
//! - diagrams (draw.io, Excalidraw, Graphviz, Mermaid) and maps (GeoJSON,
//!   TopoJSON, KML, GPX, shapefiles, GeoPackages) become SVG pictures.
//!
//! Everything here is a pure function of the bytes: no git, disk (beyond a
//! temporary copy of a GeoPackage for SQLite) or network.
//!
//! GitHub Desktop shows all of these as text or binary diffs
//! (`app/src/ui/diff/index.tsx`).

pub mod compiled;
pub mod diagram;
pub mod email;
pub mod geo;
pub mod serialized;
pub mod translation;

mod out;

pub use corvene_models::ConvertedKind;
use corvene_models::TableBlob;

/// Larger files are not converted (their diff stays git's).
pub const MAX_INPUT_BYTES: usize = 64 << 20;
/// Converted text past this many bytes is cut, with a note.
pub const MAX_TEXT_BYTES: usize = 8 << 20;

/// A file format this crate reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    JavaClass,
    PythonBytecode,
    WebAssembly,
    /// Mach-O, ELF, PE/COFF, static libraries.
    Native,
    BinaryPlist,
    MessagePack,
    Cbor,
    Bson,
    Protobuf,
    Bencode,
    Po,
    Mo,
    AppleStrings,
    StringsDict,
    XcStrings,
    Xliff,
    Arb,
    Properties,
    AndroidStrings,
    Resx,
    QtTs,
    QtQm,
    LocaleJson,
    DrawIo,
    Excalidraw,
    Dot,
    Mermaid,
    GeoJson,
    TopoJson,
    Kml,
    Kmz,
    Gpx,
    Shapefile,
    GeoPackage,
    Eml,
    Emlx,
    Msg,
    Mbox,
}

impl Format {
    /// The family the format belongs to (one flag and view each).
    pub fn kind(self) -> ConvertedKind {
        use Format::*;
        match self {
            JavaClass | PythonBytecode | WebAssembly | Native => ConvertedKind::Compiled,
            BinaryPlist | MessagePack | Cbor | Bson | Protobuf | Bencode => {
                ConvertedKind::Serialized
            }
            Po | Mo | AppleStrings | StringsDict | XcStrings | Xliff | Arb | Properties
            | AndroidStrings | Resx | QtTs | QtQm | LocaleJson => ConvertedKind::Translation,
            DrawIo | Excalidraw | Dot | Mermaid => ConvertedKind::Diagram,
            GeoJson | TopoJson | Kml | Kmz | Gpx | Shapefile | GeoPackage => ConvertedKind::Map,
            Eml | Emlx | Msg | Mbox => ConvertedKind::Email,
        }
    }

    /// What the file is, as the diff names it.
    pub fn name(self) -> &'static str {
        use Format::*;
        match self {
            JavaClass => "Java class file",
            PythonBytecode => "Python bytecode file",
            WebAssembly => "WebAssembly module",
            Native => "compiled binary",
            BinaryPlist => "binary property list",
            MessagePack => "MessagePack file",
            Cbor => "CBOR file",
            Bson => "BSON file",
            Protobuf => "protocol buffer message",
            Bencode => "bencoded file",
            Po => "gettext catalog",
            Mo => "compiled gettext catalog",
            AppleStrings => "strings file",
            StringsDict => "stringsdict file",
            XcStrings => "string catalog",
            Xliff => "XLIFF file",
            Arb => "Flutter ARB file",
            Properties => "properties file",
            AndroidStrings => "Android string resources",
            Resx => ".NET resource file",
            QtTs => "Qt translation source",
            QtQm => "compiled Qt translation",
            LocaleJson => "translation file",
            DrawIo => "draw.io diagram",
            Excalidraw => "Excalidraw drawing",
            Dot => "Graphviz graph",
            Mermaid => "Mermaid diagram",
            GeoJson => "GeoJSON file",
            TopoJson => "TopoJSON file",
            Kml => "KML file",
            Kmz => "KMZ file",
            Gpx => "GPX track",
            Shapefile => "shapefile",
            GeoPackage => "GeoPackage",
            Eml => "email message",
            Emlx => "Apple Mail message",
            Msg => "Outlook message",
            Mbox => "mailbox",
        }
    }

    /// Whether git diffs the file as text (its Source view is a line diff
    /// worth a switch); the others are binary.
    pub fn is_text(self) -> bool {
        use Format::*;
        matches!(
            self,
            Po | AppleStrings
                | XcStrings
                | Xliff
                | Arb
                | Properties
                | AndroidStrings
                | Resx
                | QtTs
                | LocaleJson
                | DrawIo
                | Excalidraw
                | Dot
                | Mermaid
                | GeoJson
                | TopoJson
                | Kml
                | Gpx
                | Eml
                | Emlx
                | Mbox
        )
    }
}

/// The lowercase file name and extension of a repository path.
fn name_and_ext(path: &str) -> (String, String) {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    let ext = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext.to_string(),
        _ => String::new(),
    };
    (name, ext)
}

/// The format a path names by itself (`None` when only the bytes can tell,
/// or it is no format here). [`format_of`] completes it with the bytes.
pub fn format_by_name(path: &str) -> Option<Format> {
    use Format::*;
    let (name, ext) = name_and_ext(path);
    let lower = path.to_ascii_lowercase();
    Some(match ext.as_str() {
        "class" => JavaClass,
        "pyc" | "pyo" => PythonBytecode,
        "wasm" => WebAssembly,
        "o" | "obj" | "a" | "lib" | "so" | "dylib" | "dll" | "exe" | "sys" | "ko" | "elf"
        | "bundle" | "node" | "efi" | "axf" | "out" | "prx" | "mexa64" | "mexmaci64" | "mexw64"
        | "pyd" | "ocx" | "cpl" => Native,
        "bplist" => BinaryPlist,
        "msgpack" | "mpk" | "msgpk" => MessagePack,
        "cbor" => Cbor,
        "bson" => Bson,
        "pb" | "binpb" | "protobin" | "pbbin" => Protobuf,
        "torrent" | "bencode" => Bencode,
        "po" | "pot" => Po,
        "mo" | "gmo" => Mo,
        "strings" => AppleStrings,
        "stringsdict" => StringsDict,
        "xcstrings" => XcStrings,
        "xliff" | "xlf" => Xliff,
        "arb" => Arb,
        "properties" => Properties,
        "resx" | "resw" => Resx,
        "qm" => QtQm,
        "drawio" | "dio" => DrawIo,
        "excalidraw" => Excalidraw,
        "dot" | "gv" => Dot,
        "mmd" | "mermaid" => Mermaid,
        "geojson" => GeoJson,
        "topojson" => TopoJson,
        "kml" => Kml,
        "kmz" => Kmz,
        "gpx" => Gpx,
        "shp" => Shapefile,
        "gpkg" => GeoPackage,
        "eml" => Eml,
        "emlx" => Emlx,
        "msg" => Msg,
        "mbox" | "mbx" => Mbox,
        "json" if name.ends_with(".excalidraw.json") => Excalidraw,
        "json" if name.ends_with(".geojson.json") => GeoJson,
        "json" if translation::is_locale_json_path(&lower) => LocaleJson,
        "xml" if translation::is_android_strings_path(&lower) => AndroidStrings,
        _ => return None,
    })
}

/// Whether [`format_of`] may find a format in `path` once it has the
/// bytes: a name that says nothing (no extension: native executables) or
/// one shared with other formats (`.ts`, `.plist`).
pub fn needs_sample(path: &str) -> bool {
    let (_, ext) = name_and_ext(path);
    matches!(ext.as_str(), "" | "ts" | "plist" | "bin" | "dat")
}

/// The format of `path`, deciding by `sample` (the start of either side's
/// bytes) where the name does not.
pub fn format_of(path: &str, sample: Option<&[u8]>) -> Option<Format> {
    if let Some(format) = format_by_name(path) {
        // a `.class` that is really a fat Mach-O (both start CAFEBABE)
        if format == Format::JavaClass
            && sample
                .is_some_and(|s| compiled::native::is_native(s) && !compiled::class::is_class(s))
        {
            return Some(Format::Native);
        }
        return Some(format);
    }
    let sample = sample?;
    let (_, ext) = name_and_ext(path);
    match ext.as_str() {
        "ts" => translation::qt::is_ts(sample).then_some(Format::QtTs),
        "plist" => sample
            .starts_with(b"bplist00")
            .then_some(Format::BinaryPlist),
        "" | "bin" | "dat" => {
            if compiled::native::is_native(sample) {
                Some(Format::Native)
            } else if sample.starts_with(b"\0asm") {
                Some(Format::WebAssembly)
            } else if sample.starts_with(b"bplist00") {
                Some(Format::BinaryPlist)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// The two sides of a converted file, in the form its kind shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Views {
    /// Text to diff line by line (a side is `None` when the file does not
    /// exist there).
    Text {
        old: Option<String>,
        new: Option<String>,
    },
    /// String tables.
    Table {
        old: Option<TableBlob>,
        new: Option<TableBlob>,
    },
    /// SVG pictures, drawn to the same scale on both sides.
    Svg {
        old: Option<Vec<u8>>,
        new: Option<Vec<u8>>,
    },
}

/// Reads both sides of a file of `format` (`None` for a side that does not
/// exist). `Err` (why) when a side does not read as that format; the
/// caller keeps git's diff then.
pub fn convert(format: Format, old: Option<&[u8]>, new: Option<&[u8]>) -> Result<Views, String> {
    if old.is_none() && new.is_none() {
        return Err("Neither side exists.".into());
    }
    if [old, new]
        .into_iter()
        .flatten()
        .any(|b| b.len() > MAX_INPUT_BYTES)
    {
        return Err("The file is too large to read.".into());
    }
    match format.kind() {
        ConvertedKind::Compiled => text_views(old, new, |b| compiled::to_text(format, b)),
        ConvertedKind::Serialized => text_views(old, new, |b| serialized::to_text(format, b)),
        ConvertedKind::Email => text_views(old, new, |b| email::to_text(format, b)),
        ConvertedKind::Translation => {
            let side = |b: Option<&[u8]>| b.map(|b| translation::to_table(format, b)).transpose();
            Ok(Views::Table {
                old: side(old)?,
                new: side(new)?,
            })
        }
        ConvertedKind::Diagram => {
            let side = |b: Option<&[u8]>| b.map(|b| diagram::to_svg(format, b)).transpose();
            Ok(Views::Svg {
                old: side(old)?,
                new: side(new)?,
            })
        }
        ConvertedKind::Map => {
            let (old, new) = geo::to_svg_pair(format, old, new)?;
            Ok(Views::Svg { old, new })
        }
    }
}

fn text_views(
    old: Option<&[u8]>,
    new: Option<&[u8]>,
    read: impl Fn(&[u8]) -> Result<String, String>,
) -> Result<Views, String> {
    let side = |b: Option<&[u8]>| {
        b.map(|b| read(b).map(|t| out::cap(t, MAX_TEXT_BYTES)))
            .transpose()
    };
    Ok(Views::Text {
        old: side(old)?,
        new: side(new)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_pick_formats() {
        assert_eq!(format_by_name("a/B.class"), Some(Format::JavaClass));
        assert_eq!(
            format_by_name("x.excalidraw.json"),
            Some(Format::Excalidraw)
        );
        assert_eq!(
            format_by_name("app/src/main/res/values-fr/strings.xml"),
            Some(Format::AndroidStrings)
        );
        assert_eq!(
            format_by_name("native/speedups.cp312-win_amd64.pyd"),
            Some(Format::Native)
        );
        assert_eq!(format_by_name("pom.xml"), None);
        assert_eq!(format_by_name(".class"), None);
        assert_eq!(format_by_name("locales/de.json"), Some(Format::LocaleJson));
        assert_eq!(format_by_name("package.json"), None);
    }

    #[test]
    fn samples_decide_shared_names() {
        assert_eq!(format_of("a.ts", Some(b"export const x = 1")), None);
        assert_eq!(
            format_of(
                "a.ts",
                Some(b"<?xml version=\"1.0\"?>\n<!DOCTYPE TS>\n<TS version=\"2.1\">")
            ),
            Some(Format::QtTs)
        );
        assert_eq!(
            format_of("Info.plist", Some(b"bplist00\xd1")),
            Some(Format::BinaryPlist)
        );
        let exe = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        assert_eq!(format_of("bin/tool", Some(&exe)), Some(Format::Native));
        assert_eq!(format_of("README", Some(b"hello")), None);
    }
}
