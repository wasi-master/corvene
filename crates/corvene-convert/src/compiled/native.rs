//! Native binaries (Mach-O, universal binaries, ELF, PE/COFF) and static
//! libraries as a summary that diffs well: format and architecture, linked
//! libraries, sections with their sizes, defined and imported symbols
//! (demangled, sorted). Addresses and offsets are left out: they move with
//! any change and would bury the real ones.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use object::read::elf::{Dyn as _, ProgramHeader as _};
use object::{
    Endianness, FileKind, Object as _, ObjectSection as _, ObjectSymbol as _, SymbolKind,
};

use crate::out::thousands;

/// Symbols listed per kind; the rest are counted.
const MAX_SYMBOLS: usize = 50_000;
/// Members of a static library described.
const MAX_MEMBERS: usize = 2_000;

/// Whether `bytes` start like a native binary this module reads.
pub fn is_native(bytes: &[u8]) -> bool {
    match FileKind::parse(bytes) {
        Ok(FileKind::MachOFat32 | FileKind::MachOFat64) => {
            // a Java class file starts CAFEBABE too, then its version
            // (major 45 and up) where a fat header has a small arch count
            bytes.len() >= 8 && u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) < 40
        }
        Ok(
            FileKind::MachO32
            | FileKind::MachO64
            | FileKind::Elf32
            | FileKind::Elf64
            | FileKind::Pe32
            | FileKind::Pe64
            | FileKind::Coff
            | FileKind::CoffBig
            | FileKind::Archive,
        ) => true,
        _ => false,
    }
}

/// The summary of one side.
pub fn to_text(bytes: &[u8]) -> Result<String, String> {
    let mut out = String::new();
    match FileKind::parse(bytes).map_err(|e| format!("Not a readable binary: {e}."))? {
        FileKind::MachOFat32 => {
            let fat = object::read::macho::MachOFatFile32::parse(bytes).map_err(err)?;
            fat_arches(&mut out, bytes, fat.arches())?;
        }
        FileKind::MachOFat64 => {
            let fat = object::read::macho::MachOFatFile64::parse(bytes).map_err(err)?;
            fat_arches(&mut out, bytes, fat.arches())?;
        }
        FileKind::Archive => archive(&mut out, bytes)?,
        _ => object_file(&mut out, bytes, "")?,
    }
    Ok(out)
}

fn err(e: object::Error) -> String {
    format!("Not a readable binary: {e}.")
}

fn fat_arches<A: object::read::macho::FatArch>(
    out: &mut String,
    bytes: &[u8],
    arches: &[A],
) -> Result<(), String> {
    let _ = writeln!(out, "Universal binary, {} architectures", arches.len());
    for arch in arches {
        let data = arch.data(bytes).map_err(err)?;
        let _ = writeln!(out, "\n=== {:?} ===", arch.architecture());
        object_file(out, data, "")?;
    }
    Ok(())
}

fn archive(out: &mut String, bytes: &[u8]) -> Result<(), String> {
    let archive = object::read::archive::ArchiveFile::parse(bytes).map_err(err)?;
    let members: Vec<_> = archive.members().filter_map(Result::ok).collect();
    let _ = writeln!(
        out,
        "Static library ({:?}), {} members",
        archive.kind(),
        members.len()
    );
    let mut names: Vec<(String, u64)> = members
        .iter()
        .map(|m| (String::from_utf8_lossy(m.name()).into_owned(), m.size()))
        .collect();
    names.sort();
    let _ = writeln!(out, "\nMembers");
    for (name, size) in &names {
        let _ = writeln!(out, "  {name}  {} bytes", thousands(*size));
    }
    let mut described: Vec<(String, &[u8])> = members
        .iter()
        .take(MAX_MEMBERS)
        .filter_map(|m| {
            let data = m.data(bytes).ok()?;
            object::File::parse(data).ok()?;
            Some((String::from_utf8_lossy(m.name()).into_owned(), data))
        })
        .collect();
    described.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, data) in described {
        let _ = writeln!(out, "\n=== {name} ===");
        // a member's own sections and symbols, without the file header
        let file = object::File::parse(data).map_err(err)?;
        sections(out, &file);
        symbols(out, &file);
    }
    Ok(())
}

fn object_file(out: &mut String, bytes: &[u8], indent: &str) -> Result<(), String> {
    let file = object::File::parse(bytes).map_err(err)?;
    let _ = writeln!(
        out,
        "{indent}{:?} {}-bit {:?}, {:?}, {:?} endian",
        file.format(),
        if file.is_64() { 64 } else { 32 },
        file.kind(),
        file.architecture(),
        file.endianness(),
    );
    match FileKind::parse(bytes) {
        Ok(FileKind::MachO32) => {
            if let Ok(f) = object::read::macho::MachOFile32::<Endianness>::parse(bytes) {
                macho_commands(out, f.macho_header(), f.endian(), bytes);
            }
        }
        Ok(FileKind::MachO64) => {
            if let Ok(f) = object::read::macho::MachOFile64::<Endianness>::parse(bytes) {
                macho_commands(out, f.macho_header(), f.endian(), bytes);
            }
        }
        Ok(FileKind::Elf32) => {
            if let Ok(f) = object::read::elf::ElfFile32::<Endianness>::parse(bytes) {
                elf_dynamic(out, f.elf_header(), f.endian(), bytes);
            }
        }
        Ok(FileKind::Elf64) => {
            if let Ok(f) = object::read::elf::ElfFile64::<Endianness>::parse(bytes) {
                elf_dynamic(out, f.elf_header(), f.endian(), bytes);
            }
        }
        _ => {}
    }
    if let Ok(Some(id)) = file.build_id() {
        let _ = writeln!(out, "Build ID  {}", crate::out::hex(id));
    }
    if let Ok(Some(uuid)) = file.mach_uuid() {
        let _ = writeln!(out, "UUID  {}", crate::out::hex(&uuid));
    }
    if let Ok(Some(pdb)) = file.pdb_info() {
        let _ = writeln!(out, "PDB  {}", String::from_utf8_lossy(pdb.path()));
    }
    // PE imports name their DLLs; Mach-O ones are listed above
    if let Ok(imports) = file.imports()
        && matches!(
            file.format(),
            object::BinaryFormat::Pe | object::BinaryFormat::Coff
        )
        && !imports.is_empty()
    {
        let mut by_lib: std::collections::BTreeMap<String, BTreeSet<String>> = Default::default();
        for import in imports {
            by_lib
                .entry(String::from_utf8_lossy(import.library()).into_owned())
                .or_default()
                .insert(demangle(&String::from_utf8_lossy(import.name())));
        }
        let _ = writeln!(out, "\nImports");
        for (lib, names) in by_lib {
            let _ = writeln!(out, "  {lib}");
            for name in names {
                let _ = writeln!(out, "    {name}");
            }
        }
    }
    if let Ok(exports) = file.exports()
        && matches!(file.format(), object::BinaryFormat::Pe)
        && !exports.is_empty()
    {
        let names: BTreeSet<String> = exports
            .iter()
            .map(|e| demangle(&String::from_utf8_lossy(e.name())))
            .collect();
        let _ = writeln!(out, "\nExports ({})", names.len());
        for name in names {
            let _ = writeln!(out, "  {name}");
        }
    }
    sections(out, &file);
    symbols(out, &file);
    Ok(())
}

fn macho_commands<M: object::read::macho::MachHeader<Endian = Endianness>>(
    out: &mut String,
    header: &M,
    endian: Endianness,
    bytes: &[u8],
) {
    let Ok(mut commands) = header.load_commands(endian, bytes, 0) else {
        return;
    };
    let mut libraries = Vec::new();
    let mut rpaths = Vec::new();
    while let Ok(Some(command)) = commands.next() {
        let cmd = command.cmd();
        if let Ok(Some(dylib)) = command.dylib() {
            let name = command
                .string(endian, dylib.dylib.name)
                .map(|n| String::from_utf8_lossy(n).into_owned())
                .unwrap_or_default();
            let how = match cmd {
                object::macho::LC_LOAD_WEAK_DYLIB => " (weak)",
                object::macho::LC_REEXPORT_DYLIB => " (re-exported)",
                object::macho::LC_LAZY_LOAD_DYLIB => " (lazy)",
                object::macho::LC_LOAD_UPWARD_DYLIB => " (upward)",
                _ => "",
            };
            libraries.push(format!("{name}{how}"));
        } else if cmd == object::macho::LC_ID_DYLIB {
            if let Ok(object::read::macho::LoadCommandVariant::IdDylib(id)) = command.variant() {
                let name = command
                    .string(endian, id.dylib.name)
                    .map(|n| String::from_utf8_lossy(n).into_owned())
                    .unwrap_or_default();
                let _ = writeln!(out, "Install name  {name}");
            }
        } else if cmd == object::macho::LC_RPATH {
            if let Ok(object::read::macho::LoadCommandVariant::Rpath(rpath)) = command.variant()
                && let Ok(path) = command.string(endian, rpath.path)
            {
                rpaths.push(String::from_utf8_lossy(path).into_owned());
            }
        } else if let Ok(Some(build)) = command.build_version() {
            let platform = match build.platform.get(endian) {
                object::macho::PLATFORM_MACOS => "macOS",
                object::macho::PLATFORM_IOS => "iOS",
                object::macho::PLATFORM_TVOS => "tvOS",
                object::macho::PLATFORM_WATCHOS => "watchOS",
                object::macho::PLATFORM_IOSSIMULATOR => "iOS Simulator",
                object::macho::PLATFORM_MACCATALYST => "Mac Catalyst",
                object::macho::PLATFORM_DRIVERKIT => "DriverKit",
                object::macho::PLATFORM_XROS => "visionOS",
                _ => "platform",
            };
            let _ = writeln!(
                out,
                "Minimum {platform}  {}, SDK {}",
                version(build.minos.get(endian)),
                version(build.sdk.get(endian)),
            );
        } else if let Ok(object::read::macho::LoadCommandVariant::VersionMin(v)) = command.variant()
        {
            let _ = writeln!(
                out,
                "Minimum OS  {}, SDK {}",
                version(v.version.get(endian)),
                version(v.sdk.get(endian)),
            );
        }
    }
    if !libraries.is_empty() {
        let _ = writeln!(out, "\nLinked libraries");
        for lib in libraries {
            let _ = writeln!(out, "  {lib}");
        }
    }
    if !rpaths.is_empty() {
        let _ = writeln!(out, "\nRun paths");
        for path in rpaths {
            let _ = writeln!(out, "  {path}");
        }
    }
}

/// A Mach-O `xxxx.yy.zz` version.
fn version(v: u32) -> String {
    let (major, minor, patch) = (v >> 16, (v >> 8) & 0xff, v & 0xff);
    if patch == 0 {
        format!("{major}.{minor}")
    } else {
        format!("{major}.{minor}.{patch}")
    }
}

fn elf_dynamic<E: object::read::elf::FileHeader<Endian = Endianness>>(
    out: &mut String,
    header: &E,
    endian: Endianness,
    bytes: &[u8],
) {
    if let Ok(segments) = header.program_headers(endian, bytes) {
        for segment in segments {
            if let Ok(Some(interp)) = segment.interpreter(endian, bytes) {
                let _ = writeln!(out, "Interpreter  {}", String::from_utf8_lossy(interp));
            }
        }
    }
    let Ok(sections) = header.sections(endian, bytes) else {
        return;
    };
    let Ok(Some((dynamic, link))) = sections.dynamic(endian, bytes) else {
        return;
    };
    let Ok(strings) = sections.strings(endian, bytes, link) else {
        return;
    };
    let mut needed = Vec::new();
    for entry in dynamic {
        let Some(tag) = entry.tag32(endian) else {
            continue;
        };
        if !entry.is_string(endian) {
            continue;
        }
        let Ok(value) = entry.string(endian, strings) else {
            continue;
        };
        let value = String::from_utf8_lossy(value).into_owned();
        match tag {
            object::elf::DT_NEEDED => needed.push(value),
            object::elf::DT_SONAME => {
                let _ = writeln!(out, "Soname  {value}");
            }
            object::elf::DT_RPATH => {
                let _ = writeln!(out, "RPATH  {value}");
            }
            object::elf::DT_RUNPATH => {
                let _ = writeln!(out, "RUNPATH  {value}");
            }
            _ => {}
        }
    }
    if !needed.is_empty() {
        let _ = writeln!(out, "\nNeeded libraries");
        for lib in needed {
            let _ = writeln!(out, "  {lib}");
        }
    }
}

fn sections(out: &mut String, file: &object::File<'_>) {
    let rows: Vec<String> = file
        .sections()
        .filter_map(|s| {
            let name = s.name().ok()?;
            if name.is_empty() {
                return None;
            }
            let name = match s.segment_name() {
                Ok(Some(segment)) => format!("{segment},{name}"),
                _ => name.to_string(),
            };
            Some(format!("  {name}  {} bytes", thousands(s.size())))
        })
        .collect();
    if rows.is_empty() {
        return;
    }
    let _ = writeln!(out, "\nSections");
    for row in rows {
        let _ = writeln!(out, "{row}");
    }
}

fn symbols(out: &mut String, file: &object::File<'_>) {
    let mut defined = BTreeSet::new();
    let mut imported = BTreeSet::new();
    let table: Vec<_> = if file.symbols().next().is_some() {
        file.symbols().collect()
    } else {
        file.dynamic_symbols().collect()
    };
    for symbol in table {
        if matches!(
            symbol.kind(),
            SymbolKind::Section | SymbolKind::File | SymbolKind::Label
        ) {
            continue;
        }
        let Ok(name) = symbol.name() else {
            continue;
        };
        if name.is_empty() || name.starts_with("ltmp") || name.starts_with(".L") {
            continue;
        }
        if symbol.is_undefined() {
            imported.insert(demangle(name));
        } else if symbol.is_global() {
            defined.insert(demangle(name));
        }
    }
    list(out, "Defined symbols", &defined);
    list(out, "Imported symbols", &imported);
}

fn list(out: &mut String, title: &str, names: &BTreeSet<String>) {
    if names.is_empty() {
        return;
    }
    let _ = writeln!(out, "\n{title} ({})", thousands(names.len() as u64));
    for name in names.iter().take(MAX_SYMBOLS) {
        let _ = writeln!(out, "  {name}");
    }
    if names.len() > MAX_SYMBOLS {
        let _ = writeln!(
            out,
            "  … {} more",
            thousands((names.len() - MAX_SYMBOLS) as u64)
        );
    }
}

/// A Rust symbol demangled (without its hash); others as they are.
fn demangle(name: &str) -> String {
    // Mach-O prefixes C names with an underscore
    let bare = name.strip_prefix('_').unwrap_or(name);
    for candidate in [name, bare] {
        if let Ok(d) = rustc_demangle::try_demangle(candidate) {
            return format!("{d:#}");
        }
    }
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_this_test_binary() {
        let exe = std::env::current_exe().unwrap();
        let bytes = std::fs::read(exe).unwrap();
        assert!(is_native(&bytes));
        let text = to_text(&bytes).unwrap();
        assert!(
            text.contains("Sections"),
            "{}",
            &text[..text.len().min(2000)]
        );
        assert!(text.contains("Defined symbols"));
    }

    #[test]
    fn class_files_are_not_fat_binaries() {
        let mut class = vec![0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 52];
        class.resize(64, 0);
        assert!(!is_native(&class));
        let mut fat = vec![0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 2];
        fat.resize(64, 0);
        assert!(is_native(&fat));
    }

    #[test]
    fn versions_print_short() {
        assert_eq!(version(0x000e_0000), "14.0");
        assert_eq!(version(0x000d_0301), "13.3.1");
    }
}
