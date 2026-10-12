//! Corvene `1350-compiled-code-diff`: compiled files as text.
//!
//! - Java class files: a `javap -c -p`-like listing (members, signatures,
//!   bytecode with resolved constants);
//! - Python bytecode: the header and the code objects' names, constants
//!   and variables;
//! - WebAssembly: the module in the text format (WAT);
//! - native binaries: what changes between builds and matters (linked
//!   libraries, sections with their sizes, defined and imported symbols),
//!   without addresses, which move with every change.
//!
//! Corvene `1360-compiled-summary`: each kind is also read as structured
//! data ([`summary::Side`]) and both versions compared ([`summary_diff`]),
//! for the diff's Summary view.

pub mod class;
pub mod native;
pub mod native_summary;
pub mod pyc;
pub mod summary;
pub mod wasm;

use crate::Format;

/// The text of one side.
pub fn to_text(format: Format, bytes: &[u8]) -> Result<String, String> {
    match format {
        Format::JavaClass => class::to_text(bytes),
        Format::PythonBytecode => pyc::to_text(bytes),
        Format::WebAssembly => wasm_text(bytes),
        Format::Native => native::to_text(bytes),
        _ => Err("Not compiled code.".into()),
    }
}

/// The structured summary of one side.
pub fn to_summary(format: Format, bytes: &[u8]) -> Result<summary::Side, String> {
    match format {
        Format::JavaClass => class::summary(bytes),
        Format::PythonBytecode => pyc::summary(bytes),
        Format::WebAssembly => wasm::summary(bytes),
        Format::Native => native_summary::summary(bytes),
        _ => Err("Not compiled code.".into()),
    }
}

/// Both sides of a compiled file compared (`None` for a side that does
/// not exist). `Err` (why) when a side does not read as `format`.
pub fn summary_diff(
    format: Format,
    old: Option<&[u8]>,
    new: Option<&[u8]>,
) -> Result<corvene_models::compiled::CompiledDiff, String> {
    use corvene_models::compiled::CompiledFamily;
    let family = match format {
        Format::JavaClass => CompiledFamily::JavaClass,
        Format::PythonBytecode => CompiledFamily::PythonBytecode,
        Format::WebAssembly => CompiledFamily::WebAssembly,
        Format::Native => CompiledFamily::Native,
        _ => return Err("Not compiled code.".into()),
    };
    if old.is_none() && new.is_none() {
        return Err("Neither side exists.".into());
    }
    let old = old.map(|b| to_summary(format, b)).transpose()?;
    let new = new.map(|b| to_summary(format, b)).transpose()?;
    Ok(summary::compare(family, old.as_ref(), new.as_ref()))
}

/// A WebAssembly module (binary) in the text format.
fn wasm_text(bytes: &[u8]) -> Result<String, String> {
    if !bytes.starts_with(b"\0asm") {
        return Err("Not a WebAssembly module.".into());
    }
    let mut config = wasmprinter::Config::new();
    config.fold_instructions(false);
    config.print_offsets(false);
    config.name_unnamed(false);
    let mut out = String::new();
    config
        .print(bytes, &mut wasmprinter::PrintFmtWrite(&mut out))
        .map_err(|e| format!("Not a readable WebAssembly module: {e}."))?;
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wasm_prints_as_wat() {
        // (module (func (export "f") (result i32) i32.const 42))
        let module: &[u8] = &[
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01,
            0x7f, 0x03, 0x02, 0x01, 0x00, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x00, 0x0a, 0x06,
            0x01, 0x04, 0x00, 0x41, 0x2a, 0x0b,
        ];
        let text = wasm_text(module).unwrap();
        assert!(text.contains("(export \"f\""), "{text}");
        assert!(text.contains("i32.const 42"), "{text}");
        assert!(wasm_text(b"nope").is_err());
    }
}
