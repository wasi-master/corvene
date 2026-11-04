//! Corvene `1360-compiled-summary`: a native binary (Mach-O, universal
//! binary, ELF, PE/COFF) or static library read as a [`Side`]: header
//! facts (format, architecture, file type, entry point, UUID or build ID,
//! minimum OS and SDK, flags, code signature), then its segments,
//! sections, linked libraries, run paths, defined and imported symbols
//! (demangled, with sizes) and a static library's members.

use std::collections::{BTreeMap, HashMap};

use corvene_models::compiled::CompiledGroupKind as G;
use object::read::elf::{Dyn as _, ProgramHeader as _};
use object::read::pe::{ImageNtHeaders, ImageOptionalHeader};
use object::{
    Architecture, BinaryFormat, Endianness, FileFlags, FileKind, Object as _, ObjectKind,
    ObjectSection as _, ObjectSegment as _, ObjectSymbol as _, SectionKind, SymbolKind,
    SymbolSection,
};

use super::native::{demangle, version};
use super::summary::{Entry, Side, hash};

/// Members of a static library read for sections and symbols.
const MAX_MEMBERS: usize = 2_000;

fn err(e: object::Error) -> String {
    format!("Not a readable binary: {e}.")
}

/// The summary of one side.
pub fn summary(bytes: &[u8]) -> Result<Side, String> {
    let mut side = Side::default();
    match FileKind::parse(bytes).map_err(|e| format!("Not a readable binary: {e}."))? {
        FileKind::MachOFat32 => {
            let fat = object::read::macho::MachOFatFile32::parse(bytes).map_err(err)?;
            fat_arches(&mut side, bytes, fat.arches())?;
        }
        FileKind::MachOFat64 => {
            let fat = object::read::macho::MachOFatFile64::parse(bytes).map_err(err)?;
            fat_arches(&mut side, bytes, fat.arches())?;
        }
        FileKind::Archive => archive(&mut side, bytes)?,
        _ => {
            let file = object::File::parse(bytes).map_err(err)?;
            side.title = title(&file, bytes);
            side.fact("Format", format_name(file.format()));
            side.fact("File type", kind_name(&file, bytes));
            side.fact("Architecture", arch_name(file.architecture()));
            let bits = match file.architecture().address_size() {
                Some(size) => size.bytes() as usize * 8,
                None if file.is_64() => 64,
                None => 32,
            };
            side.fact("Word size", format!("{bits}-bit"));
            side.fact(
                "Byte order",
                if file.is_little_endian() {
                    "little endian"
                } else {
                    "big endian"
                },
            );
            side.bytes("Size", bytes.len() as u64);
            object_file(&mut side, &file, bytes, "");
        }
    }
    Ok(side)
}

fn format_name(format: BinaryFormat) -> &'static str {
    match format {
        BinaryFormat::MachO => "Mach-O",
        BinaryFormat::Elf => "ELF",
        BinaryFormat::Pe => "PE",
        BinaryFormat::Coff => "COFF",
        BinaryFormat::Wasm => "WebAssembly",
        BinaryFormat::Xcoff => "XCOFF",
        _ => "binary",
    }
}

pub(crate) fn arch_name(arch: Architecture) -> String {
    match arch {
        Architecture::Aarch64 => "arm64".into(),
        Architecture::Aarch64_Ilp32 => "arm64_32".into(),
        Architecture::Arm => "arm".into(),
        Architecture::X86_64 => "x86_64".into(),
        Architecture::X86_64_X32 => "x32".into(),
        Architecture::I386 => "i386".into(),
        Architecture::PowerPc => "ppc".into(),
        Architecture::PowerPc64 => "ppc64".into(),
        Architecture::Riscv32 => "riscv32".into(),
        Architecture::Riscv64 => "riscv64".into(),
        Architecture::Wasm32 => "wasm32".into(),
        Architecture::Wasm64 => "wasm64".into(),
        other => format!("{other:?}").to_lowercase(),
    }
}

/// What the file is: object file, executable, dynamic library, bundle.
fn kind_name(file: &object::File<'_>, bytes: &[u8]) -> String {
    if file.format() == BinaryFormat::MachO {
        let filetype = match FileKind::parse(bytes) {
            Ok(FileKind::MachO32) => object::read::macho::MachOFile32::<Endianness>::parse(bytes)
                .ok()
                .map(|f| {
                    use object::read::macho::MachHeader as _;
                    f.macho_header().filetype(f.endian())
                }),
            Ok(FileKind::MachO64) => object::read::macho::MachOFile64::<Endianness>::parse(bytes)
                .ok()
                .map(|f| {
                    use object::read::macho::MachHeader as _;
                    f.macho_header().filetype(f.endian())
                }),
            _ => None,
        };
        match filetype {
            Some(object::macho::MH_BUNDLE) => return "bundle".into(),
            Some(object::macho::MH_DYLIB) => return "dynamic library".into(),
            Some(object::macho::MH_DYLINKER) => return "dynamic linker".into(),
            Some(object::macho::MH_KEXT_BUNDLE) => return "kernel extension".into(),
            Some(object::macho::MH_DSYM) => return "debug symbols".into(),
            _ => {}
        }
    }
    if let FileFlags::Coff { characteristics } = file.flags()
        && characteristics & object::pe::IMAGE_FILE_DLL != 0
    {
        return "DLL".into();
    }
    match file.kind() {
        ObjectKind::Relocatable => "object file",
        ObjectKind::Executable => "executable",
        ObjectKind::Dynamic => {
            if file.format() == BinaryFormat::Elf {
                "shared library"
            } else {
                "dynamic library"
            }
        }
        ObjectKind::Core => "core dump",
        _ => "file",
    }
    .into()
}

fn title(file: &object::File<'_>, bytes: &[u8]) -> String {
    format!(
        "{} {}, {}",
        format_name(file.format()),
        kind_name(file, bytes),
        arch_name(file.architecture())
    )
}

fn fat_arches<A: object::read::macho::FatArch>(
    side: &mut Side,
    bytes: &[u8],
    arches: &[A],
) -> Result<(), String> {
    let names: Vec<String> = arches.iter().map(|a| arch_name(a.architecture())).collect();
    side.title = format!("Mach-O universal binary, {}", names.join(" + "));
    side.fact("Format", "Mach-O universal binary");
    side.fact("Architectures", names.join(", "));
    side.bytes("Size", bytes.len() as u64);
    let mut kind = None;
    for (arch, name) in arches.iter().zip(&names) {
        let data = arch.data(bytes).map_err(err)?;
        side.push(
            G::Architectures,
            Entry::new(name.clone())
                .size(data.len() as u64)
                .body(hash(data)),
        );
        let file = object::File::parse(data).map_err(err)?;
        kind.get_or_insert_with(|| kind_name(&file, data));
        object_file(side, &file, data, name);
    }
    if let Some(kind) = kind {
        side.facts.insert(
            1.min(side.facts.len()),
            super::summary::Fact {
                label: "File type".into(),
                value: kind,
                number: None,
                bytes: false,
            },
        );
    }
    Ok(())
}

fn archive(side: &mut Side, bytes: &[u8]) -> Result<(), String> {
    let archive = object::read::archive::ArchiveFile::parse(bytes).map_err(err)?;
    let members: Vec<_> = archive.members().filter_map(Result::ok).collect();
    let kind = format!("{:?}", archive.kind());
    side.title = format!("Static library, {} members", members.len());
    side.fact("Format", "Static library");
    side.fact("Archive kind", kind);
    side.count("Members", members.len() as u64);
    side.bytes("Size", bytes.len() as u64);
    let mut arches: Vec<String> = Vec::new();
    let mut sections: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for (ix, member) in members.iter().enumerate() {
        let name = String::from_utf8_lossy(member.name()).into_owned();
        let data = member.data(bytes).ok();
        let parsed = (ix < MAX_MEMBERS)
            .then(|| data.and_then(|d| object::File::parse(d).ok().map(|f| (f, d))))
            .flatten();
        let detail = parsed
            .as_ref()
            .map(|(f, d)| title(f, d))
            .unwrap_or_default();
        side.push(
            G::Members,
            Entry::new(name.clone())
                .detail(detail)
                .size(member.size())
                .body(data.map_or(0, hash)),
        );
        let Some((file, _)) = parsed else {
            continue;
        };
        let arch = arch_name(file.architecture());
        if !arches.contains(&arch) {
            arches.push(arch);
        }
        for section in file.sections() {
            let Some(key) = section_name(&section) else {
                continue;
            };
            let slot = sections.entry(key).or_default();
            slot.0 += section.size();
            slot.1 = slot
                .1
                .wrapping_mul(31)
                .wrapping_add(section.data().map_or(0, hash));
        }
        symbols(side, &file, Some(&name));
    }
    if !arches.is_empty() {
        side.facts.insert(
            2.min(side.facts.len()),
            super::summary::Fact {
                label: "Architecture".into(),
                value: arches.join(", "),
                number: None,
                bytes: false,
            },
        );
    }
    for (name, (size, body)) in sections {
        side.push(G::Sections, Entry::new(name).size(size).body(body.max(1)));
    }
    Ok(())
}

/// A section's name (`__TEXT,__text` in Mach-O), `None` for unnamed ones.
fn section_name(section: &object::Section<'_, '_>) -> Option<String> {
    let name = section.name().ok()?;
    if name.is_empty() {
        return None;
    }
    Some(match section.segment_name() {
        Ok(Some(segment)) => format!("{segment},{name}"),
        _ => name.to_string(),
    })
}

fn section_kind(kind: SectionKind) -> &'static str {
    match kind {
        SectionKind::Text => "code",
        SectionKind::Data => "data",
        SectionKind::ReadOnlyData | SectionKind::ReadOnlyDataWithRel => "read-only data",
        SectionKind::ReadOnlyString => "strings",
        SectionKind::UninitializedData => "zero-filled",
        SectionKind::Tls => "thread-local data",
        SectionKind::UninitializedTls => "thread-local zero-filled",
        SectionKind::Debug | SectionKind::DebugString => "debug info",
        SectionKind::Linker => "linker",
        SectionKind::Note => "note",
        SectionKind::Metadata => "metadata",
        SectionKind::OtherString => "strings",
        _ => "",
    }
}

/// One object file's facts and tables; `arch` names it inside a universal
/// binary (its facts and sections are marked with it).
fn object_file(side: &mut Side, file: &object::File<'_>, bytes: &[u8], arch: &str) {
    let label = |l: &str| {
        if arch.is_empty() {
            l.to_string()
        } else {
            format!("{l} ({arch})")
        }
    };
    let prefix = |n: String| {
        if arch.is_empty() {
            n
        } else {
            format!("{arch}: {n}")
        }
    };
    let entry = file.entry();
    if entry != 0 {
        side.fact(label("Entry point"), format!("0x{entry:x}"));
    }
    if let Ok(Some(uuid)) = file.mach_uuid() {
        side.fact(label("UUID"), uuid_text(&uuid));
    }
    if let Ok(Some(id)) = file.build_id() {
        side.fact(label("Build ID"), crate::out::hex(id));
    }
    if let Ok(Some(pdb)) = file.pdb_info() {
        side.fact(
            label("PDB"),
            String::from_utf8_lossy(pdb.path()).into_owned(),
        );
    }
    match FileKind::parse(bytes) {
        Ok(FileKind::MachO32) => {
            if let Ok(f) = object::read::macho::MachOFile32::<Endianness>::parse(bytes) {
                macho(side, f.macho_header(), f.endian(), bytes, &label);
            }
        }
        Ok(FileKind::MachO64) => {
            if let Ok(f) = object::read::macho::MachOFile64::<Endianness>::parse(bytes) {
                macho(side, f.macho_header(), f.endian(), bytes, &label);
            }
        }
        Ok(FileKind::Elf32) => {
            if let Ok(f) = object::read::elf::ElfFile32::<Endianness>::parse(bytes) {
                elf(side, f.elf_header(), f.endian(), bytes);
            }
        }
        Ok(FileKind::Elf64) => {
            if let Ok(f) = object::read::elf::ElfFile64::<Endianness>::parse(bytes) {
                elf(side, f.elf_header(), f.endian(), bytes);
            }
        }
        Ok(FileKind::Pe32) => {
            if let Ok(f) = object::read::pe::PeFile32::parse(bytes) {
                pe(
                    side,
                    f.nt_headers(),
                    f.data_directory(object::pe::IMAGE_DIRECTORY_ENTRY_SECURITY),
                );
            }
        }
        Ok(FileKind::Pe64) => {
            if let Ok(f) = object::read::pe::PeFile64::parse(bytes) {
                pe(
                    side,
                    f.nt_headers(),
                    f.data_directory(object::pe::IMAGE_DIRECTORY_ENTRY_SECURITY),
                );
            }
        }
        _ => {}
    }
    if let FileFlags::Elf {
        os_abi, e_flags, ..
    } = file.flags()
    {
        if os_abi != 0 {
            side.fact("OS ABI", os_abi_name(os_abi));
        }
        if e_flags != 0 {
            side.fact(label("Flags"), format!("0x{e_flags:x}"));
        }
    }
    // PE and COFF segments are their sections again
    for segment in file
        .segments()
        .filter(|_| file.format() == BinaryFormat::MachO)
    {
        let Ok(Some(name)) = segment.name() else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        side.push(
            G::Segments,
            Entry::new(prefix(name.to_string()))
                .size(segment.size())
                .body(segment.data().map_or(0, hash)),
        );
    }
    for section in file.sections() {
        let Some(name) = section_name(&section) else {
            continue;
        };
        let body = if section.kind() == SectionKind::UninitializedData {
            0
        } else {
            section.data().map_or(0, hash)
        };
        side.push(
            G::Sections,
            Entry::new(prefix(name))
                .detail(section_kind(section.kind()))
                .size(section.size())
                .body(body),
        );
    }
    // PE images list what they import and export by DLL
    if file.format() == BinaryFormat::Pe {
        pe_imports_exports(side, file);
    }
    symbols(side, file, None);
}

fn uuid_text(uuid: &[u8; 16]) -> String {
    let hex: String = uuid.iter().map(|b| format!("{b:02X}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

fn os_abi_name(abi: u8) -> String {
    match abi {
        3 => "Linux".into(),
        6 => "Solaris".into(),
        9 => "FreeBSD".into(),
        12 => "OpenBSD".into(),
        97 => "ARM".into(),
        255 => "standalone".into(),
        n => format!("{n}"),
    }
}

fn macho<M: object::read::macho::MachHeader<Endian = Endianness>>(
    side: &mut Side,
    header: &M,
    endian: Endianness,
    bytes: &[u8],
    label: &dyn Fn(&str) -> String,
) {
    let flags = header.flags(endian);
    let names: Vec<&str> = [
        (object::macho::MH_NOUNDEFS, "no undefined"),
        (object::macho::MH_DYLDLINK, "dyld link"),
        (object::macho::MH_TWOLEVEL, "two-level"),
        (object::macho::MH_SUBSECTIONS_VIA_SYMBOLS, "subsections"),
        (object::macho::MH_PIE, "PIE"),
        (object::macho::MH_HAS_TLV_DESCRIPTORS, "TLV"),
        (object::macho::MH_NO_HEAP_EXECUTION, "no heap execution"),
        (object::macho::MH_APP_EXTENSION_SAFE, "app extension safe"),
    ]
    .iter()
    .filter(|(bit, _)| flags & bit != 0)
    .map(|(_, n)| *n)
    .collect();
    if flags != 0 {
        side.fact(
            label("Flags"),
            format!("{} (0x{flags:x})", names.join(", ")),
        );
    }
    let Ok(mut commands) = header.load_commands(endian, bytes, 0) else {
        return;
    };
    let mut signature = None;
    while let Ok(Some(command)) = commands.next() {
        let cmd = command.cmd();
        if let Ok(Some(dylib)) = command.dylib() {
            let name = command
                .string(endian, dylib.dylib.name)
                .map(|n| String::from_utf8_lossy(n).into_owned())
                .unwrap_or_default();
            let how = match cmd {
                object::macho::LC_LOAD_WEAK_DYLIB => "weak",
                object::macho::LC_REEXPORT_DYLIB => "re-exported",
                object::macho::LC_LAZY_LOAD_DYLIB => "lazy",
                object::macho::LC_LOAD_UPWARD_DYLIB => "upward",
                _ => "",
            };
            let mut detail = how.to_string();
            let current = dylib.dylib.current_version.get(endian);
            if current != 0 {
                if !detail.is_empty() {
                    detail.push_str(", ");
                }
                detail.push_str(&format!("version {}", version(current)));
            }
            if !side.group(G::Libraries).iter().any(|e| e.key == name) {
                side.push(G::Libraries, Entry::new(name).detail(detail));
            }
        } else if cmd == object::macho::LC_ID_DYLIB {
            if let Ok(object::read::macho::LoadCommandVariant::IdDylib(id)) = command.variant() {
                let name = command
                    .string(endian, id.dylib.name)
                    .map(|n| String::from_utf8_lossy(n).into_owned())
                    .unwrap_or_default();
                side.fact(label("Install name"), name);
                let current = id.dylib.current_version.get(endian);
                if current != 0 {
                    side.fact(label("Current version"), version(current));
                }
            }
        } else if cmd == object::macho::LC_RPATH {
            if let Ok(object::read::macho::LoadCommandVariant::Rpath(rpath)) = command.variant()
                && let Ok(path) = command.string(endian, rpath.path)
            {
                let path = String::from_utf8_lossy(path).into_owned();
                if !side.group(G::RunPaths).iter().any(|e| e.key == path) {
                    side.push(G::RunPaths, Entry::new(path));
                }
            }
        } else if cmd == object::macho::LC_CODE_SIGNATURE {
            if let Ok(object::read::macho::LoadCommandVariant::LinkeditData(data)) =
                command.variant()
            {
                signature = Some(u64::from(data.datasize.get(endian)));
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
            side.fact(
                label("Minimum OS"),
                format!("{platform} {}", version(build.minos.get(endian))),
            );
            side.fact(label("SDK"), version(build.sdk.get(endian)));
        } else if let Ok(object::read::macho::LoadCommandVariant::VersionMin(v)) = command.variant()
        {
            side.fact(label("Minimum OS"), version(v.version.get(endian)));
            side.fact(label("SDK"), version(v.sdk.get(endian)));
        }
    }
    side.fact(
        label("Code signature"),
        match signature {
            Some(n) => format!("present ({} bytes)", crate::out::thousands(n)),
            None => "none".into(),
        },
    );
}

fn elf<E: object::read::elf::FileHeader<Endian = Endianness>>(
    side: &mut Side,
    header: &E,
    endian: Endianness,
    bytes: &[u8],
) {
    if let Ok(segments) = header.program_headers(endian, bytes) {
        for segment in segments {
            if let Ok(Some(interp)) = segment.interpreter(endian, bytes) {
                side.fact("Interpreter", String::from_utf8_lossy(interp).into_owned());
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
            object::elf::DT_NEEDED => side.push(G::Libraries, Entry::new(value)),
            object::elf::DT_SONAME => side.fact("Soname", value),
            object::elf::DT_RPATH | object::elf::DT_RUNPATH => {
                for path in value.split(':').filter(|p| !p.is_empty()) {
                    side.push(
                        G::RunPaths,
                        Entry::new(path).detail(if tag == object::elf::DT_RPATH {
                            "RPATH"
                        } else {
                            "RUNPATH"
                        }),
                    );
                }
            }
            _ => {}
        }
    }
}

fn pe<H: ImageNtHeaders>(
    side: &mut Side,
    headers: &H,
    security: Option<&object::pe::ImageDataDirectory>,
) {
    let opt = headers.optional_header();
    let subsystem = match opt.subsystem() {
        object::pe::IMAGE_SUBSYSTEM_NATIVE => "native",
        object::pe::IMAGE_SUBSYSTEM_WINDOWS_GUI => "Windows GUI",
        object::pe::IMAGE_SUBSYSTEM_WINDOWS_CUI => "Windows console",
        object::pe::IMAGE_SUBSYSTEM_EFI_APPLICATION => "EFI application",
        object::pe::IMAGE_SUBSYSTEM_EFI_BOOT_SERVICE_DRIVER => "EFI boot driver",
        object::pe::IMAGE_SUBSYSTEM_EFI_RUNTIME_DRIVER => "EFI runtime driver",
        object::pe::IMAGE_SUBSYSTEM_WINDOWS_BOOT_APPLICATION => "Windows boot application",
        _ => "",
    };
    side.fact("Subsystem", subsystem);
    side.fact(
        "Minimum OS",
        format!(
            "Windows {}.{}",
            opt.major_operating_system_version(),
            opt.minor_operating_system_version()
        ),
    );
    side.fact(
        "Subsystem version",
        format!(
            "{}.{}",
            opt.major_subsystem_version(),
            opt.minor_subsystem_version()
        ),
    );
    side.fact(
        "Linker version",
        format!(
            "{}.{}",
            opt.major_linker_version(),
            opt.minor_linker_version()
        ),
    );
    let dll = opt.dll_characteristics();
    let names: Vec<&str> = [
        (
            object::pe::IMAGE_DLLCHARACTERISTICS_HIGH_ENTROPY_VA,
            "high entropy VA",
        ),
        (object::pe::IMAGE_DLLCHARACTERISTICS_DYNAMIC_BASE, "ASLR"),
        (
            object::pe::IMAGE_DLLCHARACTERISTICS_FORCE_INTEGRITY,
            "force integrity",
        ),
        (object::pe::IMAGE_DLLCHARACTERISTICS_NX_COMPAT, "NX"),
        (object::pe::IMAGE_DLLCHARACTERISTICS_NO_SEH, "no SEH"),
        (
            object::pe::IMAGE_DLLCHARACTERISTICS_APPCONTAINER,
            "AppContainer",
        ),
        (object::pe::IMAGE_DLLCHARACTERISTICS_GUARD_CF, "CFG"),
        (
            object::pe::IMAGE_DLLCHARACTERISTICS_TERMINAL_SERVER_AWARE,
            "terminal server aware",
        ),
    ]
    .iter()
    .filter(|(bit, _)| dll & bit != 0)
    .map(|(_, n)| *n)
    .collect();
    if dll != 0 {
        side.fact("Flags", format!("{} (0x{dll:x})", names.join(", ")));
    }
    let signed = security.map_or(0, |d| d.size.get(object::LittleEndian));
    side.fact(
        "Code signature",
        if signed > 0 {
            format!(
                "Authenticode ({} bytes)",
                crate::out::thousands(u64::from(signed))
            )
        } else {
            "none".into()
        },
    );
}

fn pe_imports_exports(side: &mut Side, file: &object::File<'_>) {
    if let Ok(imports) = file.imports() {
        let mut by_lib: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for import in &imports {
            let lib = String::from_utf8_lossy(import.library()).into_owned();
            let name = String::from_utf8_lossy(import.name()).into_owned();
            by_lib.entry(lib.clone()).or_default().push(name.clone());
            side.push(
                G::ImportedSymbols,
                Entry::new(demangle(&name))
                    .key(format!("{}!{name}", lib.to_ascii_lowercase()))
                    .detail(lib),
            );
        }
        for (lib, mut names) in by_lib {
            names.sort();
            let n = names.len();
            side.push(
                G::Libraries,
                Entry::new(lib.clone())
                    .key(lib.to_ascii_lowercase())
                    .detail(format!(
                        "{n} {}",
                        if n == 1 { "function" } else { "functions" }
                    ))
                    .body(hash(names.join("\n").as_bytes())),
            );
        }
    }
    if let Ok(exports) = file.exports() {
        for export in exports {
            let name = String::from_utf8_lossy(export.name()).into_owned();
            side.push(
                G::DefinedSymbols,
                Entry::new(demangle(&name)).key(name).detail("export"),
            );
        }
    }
}

fn symbol_kind(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Text => "function",
        SymbolKind::Data => "data",
        SymbolKind::Tls => "thread-local",
        _ => "",
    }
}

/// Defined and imported symbols; a static library's `member` is noted on
/// each and its imports left out (they resolve inside the library).
fn symbols(side: &mut Side, file: &object::File<'_>, member: Option<&str>) {
    let table: Vec<_> = if file.symbols().next().is_some() {
        file.symbols().collect()
    } else {
        file.dynamic_symbols().collect()
    };
    if table.is_empty() {
        return;
    }
    // sizes Mach-O does not record: up to the next symbol of the section
    let mut starts: HashMap<usize, Vec<u64>> = HashMap::new();
    for symbol in &table {
        if let SymbolSection::Section(ix) = symbol.section() {
            starts.entry(ix.0).or_default().push(symbol.address());
        }
    }
    for list in starts.values_mut() {
        list.sort_unstable();
        list.dedup();
    }
    let section_end = |ix: usize| {
        file.section_by_index(object::SectionIndex(ix))
            .ok()
            .map(|s| s.address() + s.size())
    };
    let pe = file.format() == BinaryFormat::Pe;
    let mut defined: std::collections::HashSet<String> = side
        .group(G::DefinedSymbols)
        .iter()
        .map(|e| e.key.clone())
        .collect();
    for symbol in &table {
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
            if member.is_none() && !pe {
                side.push(
                    G::ImportedSymbols,
                    Entry::new(demangle(name)).key(name.to_string()),
                );
            }
            continue;
        }
        if !symbol.is_global() {
            continue;
        }
        let mut size = symbol.size();
        // the Mach-O header symbol "spans" the load commands
        if size == 0
            && !name.starts_with("__mh_")
            && let SymbolSection::Section(ix) = symbol.section()
        {
            let addr = symbol.address();
            let next = starts
                .get(&ix.0)
                .and_then(|l| l.iter().find(|a| **a > addr).copied())
                .or_else(|| section_end(ix.0));
            size = next.map_or(0, |n| n.saturating_sub(addr));
        }
        let mut detail = symbol_kind(symbol.kind()).to_string();
        if symbol.is_weak() {
            detail.push_str(if detail.is_empty() { "weak" } else { ", weak" });
        }
        if let Some(member) = member {
            if !detail.is_empty() {
                detail.push_str(", ");
            }
            detail.push_str("in ");
            detail.push_str(member);
        }
        if !defined.insert(name.to_string()) {
            continue;
        }
        side.push(
            G::DefinedSymbols,
            Entry::new(demangle(name))
                .key(name.to_string())
                .detail(detail)
                .size(size),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_this_test_binary() {
        let bytes = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        let side = summary(&bytes).unwrap();
        assert!(side.facts.iter().any(|f| f.label == "Architecture"));
        assert!(
            side.groups
                .iter()
                .any(|(k, e)| *k == G::Sections && !e.is_empty())
        );
        // a Windows executable keeps its symbols in the PDB and a Linux
        // test executable has no global ones
        if cfg!(target_os = "macos") {
            assert!(
                side.groups
                    .iter()
                    .any(|(k, e)| *k == G::DefinedSymbols && e.iter().any(|e| e.size.is_some()))
            );
        }
    }

    #[test]
    fn uuids_read_as_uuids() {
        let uuid = [
            0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab,
            0xcd, 0xef,
        ];
        assert_eq!(uuid_text(&uuid), "12345678-9ABC-DEF0-0123-456789ABCDEF");
    }
}
