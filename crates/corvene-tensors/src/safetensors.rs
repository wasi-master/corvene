//! safetensors: an 8-byte little-endian header length, a JSON header of
//! `name -> {dtype, shape, data_offsets}` plus `__metadata__`, then data.

use corvene_models::TensorFile;
use corvene_models::tensors::{DType, TensorFormat, TensorInfo, TensorLocation};

use crate::source::{bad, truncated};
use crate::{Error, Source};

/// The reference implementation refuses larger headers too.
const MAX_HEADER: u64 = 100 * 1024 * 1024;

fn dtype(s: &str) -> DType {
    match s {
        "F64" => DType::F64,
        "F32" => DType::F32,
        "F16" => DType::F16,
        "BF16" => DType::BF16,
        "I64" => DType::I64,
        "I32" => DType::I32,
        "I16" => DType::I16,
        "I8" => DType::I8,
        "U64" => DType::U64,
        "U32" => DType::U32,
        "U16" => DType::U16,
        "U8" => DType::U8,
        "BOOL" => DType::Bool,
        "F8_E4M3" => DType::F8E4M3,
        "F8_E5M2" => DType::F8E5M2,
        "C64" => DType::C64,
        other => DType::Other(other.to_string()),
    }
}

pub(crate) fn read(src: &dyn Source) -> Result<TensorFile, Error> {
    let mut len = [0u8; 8];
    src.read_at(0, &mut len).map_err(|_| truncated())?;
    let n = u64::from_le_bytes(len);
    if n > MAX_HEADER || n.saturating_add(8) > src.len() {
        return Err(bad("safetensors header length out of range"));
    }
    let mut header = vec![0u8; n as usize];
    src.read_at(8, &mut header)?;
    let json: serde_json::Value =
        serde_json::from_slice(&header).map_err(|e| bad(format!("safetensors header: {e}")))?;
    let obj = json
        .as_object()
        .ok_or_else(|| bad("safetensors header is not an object"))?;
    let data_start = 8 + n;
    let data_len = src.len() - data_start;

    let mut file = TensorFile {
        format: TensorFormat::Safetensors,
        ..TensorFile::default()
    };
    let mut tensors = Vec::with_capacity(obj.len());
    for (name, v) in obj {
        if name == "__metadata__" {
            if let Some(m) = v.as_object() {
                for (k, v) in m {
                    let s = match v {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    file.metadata.push((k.clone(), crate::clip(&s, 4000)));
                }
            }
            continue;
        }
        let dt = dtype(v.get("dtype").and_then(|d| d.as_str()).unwrap_or("?"));
        let shape: Vec<u64> = v
            .get("shape")
            .and_then(|s| s.as_array())
            .map(|a| a.iter().map(|d| d.as_u64().unwrap_or(0)).collect())
            .unwrap_or_default();
        let offs = v.get("data_offsets").and_then(|o| o.as_array());
        let (start, end) = match offs.map(|o| o.as_slice()) {
            Some([a, b]) => (a.as_u64().unwrap_or(0), b.as_u64().unwrap_or(0)),
            _ => return Err(bad(format!("tensor {name} has no data_offsets"))),
        };
        if start > end || end > data_len {
            return Err(bad(format!("tensor {name} data runs past the file")));
        }
        let elements = crate::elements(&shape).ok_or_else(|| bad("shape overflows"))?;
        tensors.push((
            start,
            TensorInfo {
                name: name.clone(),
                dtype: dt,
                shape,
                elements,
                bytes: end - start,
                location: TensorLocation::Bytes {
                    offset: data_start + start,
                    len: end - start,
                },
                ..TensorInfo::default()
            },
        ));
    }
    // serde_json sorts keys; file order is data order.
    tensors.sort_by_key(|t| t.0);
    file.tensors = tensors.into_iter().map(|t| t.1).collect();
    Ok(file)
}
