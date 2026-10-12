//! NumPy `.npy` (format versions 1 to 3) and `.npz` (a zip of `.npy`
//! members, stored or deflated).

use corvene_models::TensorFile;
use corvene_models::tensors::{DType, TensorFormat, TensorInfo, TensorLocation};

use crate::source::{bad, truncated};
use crate::zipfile;
use crate::{Error, Source};

const MAX_HEADER: u64 = 1024 * 1024;
const MAGIC: &[u8] = b"\x93NUMPY";

/// A parsed `.npy` header: its fields and where the data starts.
struct Header {
    dtype: DType,
    big_endian: bool,
    fortran: bool,
    shape: Vec<u64>,
    data_offset: u64,
}

/// Reads the header from the first bytes of an array (`read(offset, len)`).
fn header(
    read: &mut dyn FnMut(u64, u64) -> Result<Vec<u8>, Error>,
    total: u64,
) -> Result<Header, Error> {
    let pre = read(0, 10.min(total))?;
    if pre.len() < 10 || &pre[..6] != MAGIC {
        return Err(bad("not a NumPy array"));
    }
    let major = pre[6];
    let (len, start) = match major {
        1 => (u16::from_le_bytes([pre[8], pre[9]]) as u64, 10),
        2 | 3 => {
            let more = read(8, 4)?;
            if more.len() < 4 {
                return Err(truncated());
            }
            (
                u32::from_le_bytes([more[0], more[1], more[2], more[3]]) as u64,
                12,
            )
        }
        v => return Err(bad(format!("NumPy format version {v} not supported"))),
    };
    if len > MAX_HEADER || start + len > total {
        return Err(bad("NumPy header length out of range"));
    }
    let text = read(start, len)?;
    let text = String::from_utf8_lossy(&text);
    let descr = dict_value(&text, "descr").ok_or_else(|| bad("NumPy header has no descr"))?;
    let fortran = dict_value(&text, "fortran_order").is_some_and(|v| v.trim() == "True");
    let shape_text = dict_value(&text, "shape").ok_or_else(|| bad("NumPy header has no shape"))?;
    let shape = parse_shape(&shape_text)?;
    let descr = descr.trim().trim_matches(|c| c == '\'' || c == '"');
    let (dtype, big_endian) = parse_descr(descr);
    Ok(Header {
        dtype,
        big_endian,
        fortran,
        shape,
        data_offset: start + len,
    })
}

/// The raw text of `key`'s value in a Python dict literal.
fn dict_value(text: &str, key: &str) -> Option<String> {
    let pat_a = format!("'{key}'");
    let pat_b = format!("\"{key}\"");
    let at = text.find(&pat_a).or_else(|| text.find(&pat_b))?;
    let rest = &text[at + key.len() + 2..];
    let rest = rest.trim_start().strip_prefix(':')?.trim_start();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for (i, c) in rest.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(c),
            (None, '(' | '[' | '{') => depth += 1,
            (None, ')' | ']' | '}') => {
                if depth == 0 {
                    return Some(rest[..i].to_string());
                }
                depth -= 1;
            }
            (None, ',') if depth == 0 => return Some(rest[..i].to_string()),
            _ => {}
        }
    }
    None
}

fn parse_shape(s: &str) -> Result<Vec<u64>, Error> {
    let inner = s.trim().trim_start_matches('(').trim_end_matches(')');
    inner
        .split(',')
        .map(|p| p.trim().trim_end_matches('L'))
        .filter(|p| !p.is_empty())
        .map(|p| {
            p.parse::<u64>()
                .map_err(|_| bad(format!("bad NumPy shape {s}")))
        })
        .collect()
}

fn parse_descr(d: &str) -> (DType, bool) {
    let (order, code) = match d.chars().next() {
        Some(c @ ('<' | '>' | '|' | '=')) => (c, &d[1..]),
        _ => ('<', d),
    };
    let dtype = match code {
        "f2" | "e" => DType::F16,
        "f4" | "f" => DType::F32,
        "f8" | "d" => DType::F64,
        "i1" | "b" => DType::I8,
        "u1" | "B" => DType::U8,
        "i2" | "h" => DType::I16,
        "u2" | "H" => DType::U16,
        "i4" | "i" => DType::I32,
        "u4" | "I" => DType::U32,
        "i8" | "q" => DType::I64,
        "u8" | "Q" => DType::U64,
        "b1" | "?" => DType::Bool,
        "c8" => DType::C64,
        "c16" => DType::C128,
        // ml_dtypes registers bfloat16 as a void type; the descr is opaque.
        other if other.starts_with('U') || other.starts_with('S') || other.starts_with('O') => {
            DType::Str
        }
        other => DType::Other(other.to_string()),
    };
    (dtype, order == '>')
}

fn info(
    name: String,
    h: &Header,
    offset: u64,
    available: u64,
    member: Option<String>,
) -> Result<TensorInfo, Error> {
    let elements = crate::elements(&h.shape).ok_or_else(|| bad("shape overflows"))?;
    let bytes = h.dtype.size().and_then(|s| s.checked_mul(elements));
    let location = match bytes {
        Some(len) if len <= available => TensorLocation::Bytes { offset, len },
        Some(_) => return Err(bad(format!("{name}: data runs past the end"))),
        None => TensorLocation::Missing,
    };
    Ok(TensorInfo {
        name,
        dtype: h.dtype.clone(),
        shape: h.shape.clone(),
        elements,
        bytes: bytes.unwrap_or(available),
        location,
        big_endian: h.big_endian && h.dtype.size().is_some_and(|s| s > 1),
        column_major: h.fortran,
        member,
        ..TensorInfo::default()
    })
}

pub(crate) fn read_npy(src: &dyn Source) -> Result<TensorFile, Error> {
    let total = src.len();
    let mut rd = |o: u64, n: u64| -> Result<Vec<u8>, Error> {
        let n = n.min(total.saturating_sub(o));
        let mut b = vec![0u8; n as usize];
        src.read_at(o, &mut b)?;
        Ok(b)
    };
    let h = header(&mut rd, total)?;
    let mut file = TensorFile {
        format: TensorFormat::Npy,
        ..TensorFile::default()
    };
    file.metadata.push(("dtype".into(), h.dtype.name()));
    if h.fortran {
        file.metadata
            .push(("order".into(), "Fortran (column-major)".into()));
    }
    let t = info(
        "array".into(),
        &h,
        h.data_offset,
        total - h.data_offset,
        None,
    )?;
    file.tensors.push(t);
    Ok(file)
}

pub(crate) fn read_npz(src: &dyn Source) -> Result<TensorFile, Error> {
    if !zipfile::is_zip(src) {
        return Err(bad("not a zip archive"));
    }
    let members = zipfile::list(src)?;
    let mut file = TensorFile {
        format: TensorFormat::Npz,
        ..TensorFile::default()
    };
    for m in &members {
        let name = m.name.strip_suffix(".npy").unwrap_or(&m.name).to_string();
        let mut rd = |o: u64, n: u64| -> Result<Vec<u8>, Error> {
            let n = n.min(m.size.saturating_sub(o));
            zipfile::read_range(src, m, o, n)
        };
        match header(&mut rd, m.size) {
            Ok(h) => {
                let (offset, member) = if m.stored {
                    (m.data_start + h.data_offset, None)
                } else {
                    (h.data_offset, Some(m.name.clone()))
                };
                match info(name, &h, offset, m.size - h.data_offset, member) {
                    Ok(t) => file.tensors.push(t),
                    Err(e) => file.notes.push(e.to_string()),
                }
            }
            Err(e) => file.notes.push(format!("{}: {e}", m.name)),
        }
    }
    Ok(file)
}
