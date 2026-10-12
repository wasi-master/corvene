//! Corvene `1360-compiled-summary`: a WebAssembly module read as a
//! [`Side`]: its imports, exports, functions (named from the name section
//! or exports, with their code size), memories, tables, globals and custom
//! sections.

use std::collections::HashMap;

use corvene_models::compiled::CompiledGroupKind as G;
use wasmparser::{
    CompositeInnerType, Encoding, ExternalKind, KnownCustom, Name, Parser, Payload, TypeRef,
};

use super::summary::{Entry, Side, hash};

fn err(e: wasmparser::BinaryReaderError) -> String {
    format!("Not a readable WebAssembly module: {e}.")
}

fn limits(initial: u64, maximum: Option<u64>) -> String {
    match maximum {
        Some(max) => format!("{initial}..{max}"),
        None => format!("{initial}.."),
    }
}

fn memory_text(m: &wasmparser::MemoryType) -> String {
    let mut out = format!("{} pages", limits(m.initial, m.maximum));
    if m.memory64 {
        out.push_str(", 64-bit");
    }
    if m.shared {
        out.push_str(", shared");
    }
    out
}

fn table_text(t: &wasmparser::TableType) -> String {
    format!("{} {}", t.element_type, limits(t.initial, t.maximum))
}

fn global_text(g: &wasmparser::GlobalType) -> String {
    format!("{}{}", if g.mutable { "mut " } else { "" }, g.content_type)
}

/// The summary of one module.
pub fn summary(bytes: &[u8]) -> Result<Side, String> {
    if !bytes.starts_with(b"\0asm") {
        return Err("Not a WebAssembly module.".into());
    }
    let mut side = Side::default();
    // function types by index
    let mut types: Vec<String> = Vec::new();
    // the type of each function, imported ones first
    let mut func_types: Vec<u32> = Vec::new();
    let mut imported_funcs = 0usize;
    let mut names: HashMap<u32, String> = HashMap::new();
    let mut export_names: HashMap<u32, String> = HashMap::new();
    let mut bodies: Vec<(u64, u64)> = Vec::new();
    let mut component = false;
    let mut depth = 0usize;
    let mut data_segments = 0u64;
    let mut data_bytes = 0u64;
    let mut start = None;
    let (mut tables, mut memories, mut globals) = (0usize, 0usize, 0usize);
    for payload in Parser::new(0).parse_all(bytes) {
        let payload = payload.map_err(err)?;
        match payload {
            Payload::Version { num, encoding, .. } => {
                depth += 1;
                if depth == 1 {
                    component = encoding == Encoding::Component;
                    side.fact(
                        "Format",
                        if component {
                            "WebAssembly component"
                        } else {
                            "WebAssembly module"
                        },
                    );
                    side.fact("Version", num.to_string());
                }
                continue;
            }
            Payload::End(_) => {
                depth = depth.saturating_sub(1);
                continue;
            }
            _ if depth != 1 => continue,
            Payload::TypeSection(reader) => {
                for group in reader {
                    for ty in group.map_err(err)?.into_types() {
                        types.push(match &ty.composite_type.inner {
                            CompositeInnerType::Func(f) => {
                                let list = |v: &[wasmparser::ValType]| {
                                    v.iter()
                                        .map(|t| t.to_string())
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                };
                                format!("({}) -> ({})", list(f.params()), list(f.results()))
                            }
                            _ => "type".into(),
                        });
                    }
                }
            }
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.map_err(err)?;
                    let detail = match import.ty {
                        TypeRef::Func(t) | TypeRef::FuncExact(t) => {
                            func_types.push(t);
                            imported_funcs += 1;
                            format!(
                                "function {}",
                                types.get(t as usize).cloned().unwrap_or_default()
                            )
                        }
                        TypeRef::Table(t) => format!("table {}", table_text(&t)),
                        TypeRef::Memory(m) => format!("memory {}", memory_text(&m)),
                        TypeRef::Global(g) => format!("global {}", global_text(&g)),
                        TypeRef::Tag(_) => "tag".into(),
                    };
                    let name = format!("{}.{}", import.module, import.name);
                    side.push(G::Imports, Entry::new(name).detail(detail));
                }
            }
            Payload::FunctionSection(reader) => {
                for t in reader {
                    func_types.push(t.map_err(err)?);
                }
            }
            Payload::TableSection(reader) => {
                for table in reader {
                    let table = table.map_err(err)?;
                    side.push(
                        G::Tables,
                        Entry::new(format!("table {tables}")).detail(table_text(&table.ty)),
                    );
                    tables += 1;
                }
            }
            Payload::MemorySection(reader) => {
                for memory in reader {
                    let memory = memory.map_err(err)?;
                    side.push(
                        G::Memories,
                        Entry::new(format!("memory {memories}")).detail(memory_text(&memory)),
                    );
                    memories += 1;
                }
            }
            Payload::GlobalSection(reader) => {
                for global in reader {
                    let global = global.map_err(err)?;
                    let mut init = global.init_expr.get_binary_reader();
                    let body = init
                        .read_bytes(init.bytes_remaining())
                        .map(hash)
                        .unwrap_or(0);
                    side.push(
                        G::Globals,
                        Entry::new(format!("global {globals}"))
                            .detail(global_text(&global.ty))
                            .body(body),
                    );
                    globals += 1;
                }
            }
            Payload::ExportSection(reader) => {
                for export in reader {
                    let export = export.map_err(err)?;
                    let kind = match export.kind {
                        ExternalKind::Func | ExternalKind::FuncExact => {
                            export_names
                                .entry(export.index)
                                .or_insert_with(|| export.name.to_string());
                            "function"
                        }
                        ExternalKind::Table => "table",
                        ExternalKind::Memory => "memory",
                        ExternalKind::Global => "global",
                        ExternalKind::Tag => "tag",
                    };
                    side.push(G::Exports, Entry::new(export.name).detail(kind));
                }
            }
            Payload::StartSection { func, .. } => start = Some(func),
            Payload::DataSection(reader) => {
                for data in reader {
                    let data = data.map_err(err)?;
                    data_segments += 1;
                    data_bytes += data.data.len() as u64;
                }
            }
            Payload::CodeSectionEntry(body) => {
                let code = body.as_bytes();
                bodies.push((code.len() as u64, hash(code)));
            }
            Payload::CustomSection(reader) => {
                if let KnownCustom::Name(names_reader) = reader.as_known() {
                    for name in names_reader.into_iter().flatten() {
                        if let Name::Function(map) = name {
                            for naming in map.into_iter().flatten() {
                                names.insert(naming.index, super::native::demangle(naming.name));
                            }
                        }
                    }
                }
                side.push(
                    G::CustomSections,
                    Entry::new(reader.name())
                        .size(reader.data().len() as u64)
                        .body(hash(reader.data())),
                );
            }
            _ => {}
        }
    }
    let name_of = |index: u32| {
        names
            .get(&index)
            .or_else(|| export_names.get(&index))
            .cloned()
            .unwrap_or_else(|| format!("func[{index}]"))
    };
    for (ix, (size, body)) in bodies.iter().enumerate() {
        let index = (imported_funcs + ix) as u32;
        let detail = func_types
            .get(index as usize)
            .and_then(|t| types.get(*t as usize))
            .cloned()
            .unwrap_or_default();
        side.push(
            G::Functions,
            Entry::new(name_of(index))
                .detail(detail)
                .size(*size)
                .body(*body),
        );
    }
    side.title = format!(
        "WebAssembly {}, {} functions",
        if component { "component" } else { "module" },
        bodies.len()
    );
    side.count("Imports", count(&side, G::Imports));
    side.count("Exports", count(&side, G::Exports));
    side.count("Functions", bodies.len() as u64);
    if let Some(func) = start {
        side.fact("Start function", name_of(func));
    }
    if data_segments > 0 {
        side.count("Data segments", data_segments);
        side.bytes("Data", data_bytes);
    }
    side.bytes("Size", bytes.len() as u64);
    Ok(side)
}

fn count(side: &Side, kind: G) -> u64 {
    side.groups
        .iter()
        .find(|(k, _)| *k == kind)
        .map_or(0, |(_, e)| e.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_module() {
        // (module (func (export "f") (result i32) i32.const 42))
        let module: &[u8] = &[
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01,
            0x7f, 0x03, 0x02, 0x01, 0x00, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0a, 0x06,
            0x01, 0x04, 0x00, 0x41, 0x2a, 0x0b,
        ];
        let side = summary(module).unwrap();
        let (_, functions) = side
            .groups
            .iter()
            .find(|(k, _)| *k == G::Functions)
            .unwrap();
        assert_eq!(functions[0].name, "f");
        assert_eq!(functions[0].detail, "() -> (i32)");
        assert_eq!(functions[0].size, Some(4));
        assert!(summary(b"nope").is_err());
    }
}
