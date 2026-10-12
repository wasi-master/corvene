//! ONNX `ModelProto`: model fields, graph inputs and outputs, initializers
//! (raw or typed data, external data) and operator counts.

use std::collections::HashMap;

use corvene_models::TensorFile;
use corvene_models::tensors::{DType, MetaSection, TensorFormat, TensorInfo, TensorLocation};

use crate::proto;
use crate::source::{Reader, bad};
use crate::{Error, Source};

const MAX_DIMS: usize = 16;

pub(crate) fn dtype(n: u64) -> DType {
    match n {
        1 => DType::F32,
        2 => DType::U8,
        3 => DType::I8,
        4 => DType::U16,
        5 => DType::I16,
        6 => DType::I32,
        7 => DType::I64,
        8 => DType::Str,
        9 => DType::Bool,
        10 => DType::F16,
        11 => DType::F64,
        12 => DType::U32,
        13 => DType::U64,
        14 => DType::C64,
        15 => DType::C128,
        16 => DType::BF16,
        17 => DType::F8E4M3,
        18 => DType::Other("f8_e4m3fnuz".into()),
        19 => DType::F8E5M2,
        20 => DType::Other("f8_e5m2fnuz".into()),
        21 => DType::Other("u4".into()),
        22 => DType::Other("i4".into()),
        23 => DType::Other("f4_e2m1".into()),
        0 => DType::Other("undefined".into()),
        n => DType::Other(format!("type {n}")),
    }
}

/// Byte size of `elements` items of `dtype` in `raw_data`.
fn raw_bytes(dtype: &DType, elements: u64) -> Option<u64> {
    match dtype {
        DType::Other(n) if n == "u4" || n == "i4" || n == "f4_e2m1" => Some(elements.div_ceil(2)),
        DType::Other(n) if n.starts_with("f8") => Some(elements),
        d => d.size()?.checked_mul(elements),
    }
}

/// An ONNX `TensorProto`.
pub(crate) fn tensor(r: &mut Reader, start: u64, end: u64) -> Result<TensorInfo, Error> {
    let mut dims: Vec<u64> = Vec::new();
    let mut dt = 0u64;
    let mut name = String::new();
    let mut loc = TensorLocation::Missing;
    let mut stored = 0u64;
    let mut external: Vec<(String, String)> = Vec::new();
    let mut ext_flag = false;
    proto::fields(r, start, end, |r, no, f| {
        match no {
            1 => proto::ints(r, f, &mut dims, MAX_DIMS + 1)?,
            2 => dt = f.int(),
            8 => name = proto::string(r, f, 4096)?,
            9 | 4 | 10 => {
                if let Some((offset, len)) = f.range() {
                    loc = TensorLocation::Bytes { offset, len };
                    stored = len;
                }
            }
            5 | 7 | 11 => {
                if let Some((offset, len)) = f.range() {
                    loc = TensorLocation::Varints { offset, len };
                    stored = len;
                }
            }
            13 => {
                if let Some((s, l)) = f.range() {
                    let mut k = String::new();
                    let mut v = String::new();
                    proto::fields(r, s, s + l, |r, no, f| {
                        match no {
                            1 => k = proto::string(r, f, 4096)?,
                            2 => v = proto::string(r, f, 4096)?,
                            _ => {}
                        }
                        Ok(())
                    })?;
                    external.push((k, v));
                }
            }
            14 => ext_flag = f.int() == 1,
            _ => {}
        }
        Ok(())
    })?;
    if dims.len() > MAX_DIMS {
        return Err(bad("tensor has too many dimensions"));
    }
    let dtype = dtype(dt);
    let elements = crate::elements(&dims).ok_or_else(|| bad("shape overflows"))?;
    let mut bytes = stored;
    if ext_flag || !external.is_empty() {
        let get = |k: &str| external.iter().find(|e| e.0 == k).map(|e| e.1.clone());
        let file = get("location").unwrap_or_default();
        let offset = get("offset").and_then(|v| v.parse().ok()).unwrap_or(0);
        let len = get("length")
            .and_then(|v| v.parse().ok())
            .or_else(|| raw_bytes(&dtype, elements))
            .unwrap_or(0);
        bytes = len;
        loc = TensorLocation::External { file, offset, len };
    } else if let TensorLocation::Bytes { len, .. } = loc {
        // Packed fixed-width data must hold every element.
        if raw_bytes(&dtype, elements).is_some_and(|want| want != len) {
            loc = TensorLocation::Missing;
        }
    }
    Ok(TensorInfo {
        name,
        dtype,
        shape: dims,
        elements,
        bytes,
        location: loc,
        ..TensorInfo::default()
    })
}

/// `f32 [batch, 3, 224, 224]` for a `TypeProto`.
fn type_label(r: &mut Reader, start: u64, end: u64) -> Result<String, Error> {
    let mut label = String::from("?");
    proto::fields(r, start, end, |r, no, f| {
        let Some((s, l)) = f.range() else {
            return Ok(());
        };
        label = match no {
            1 | 8 => {
                let mut elem = 0u64;
                let mut dims: Vec<String> = Vec::new();
                let mut has_shape = false;
                proto::fields(r, s, s + l, |r, no, f| {
                    match no {
                        1 => elem = f.int(),
                        2 => {
                            has_shape = true;
                            if let Some((s, l)) = f.range() {
                                proto::fields(r, s, s + l, |r, no, f| {
                                    if no == 1
                                        && let Some((s, l)) = f.range()
                                        && dims.len() < 64
                                    {
                                        let mut d = String::from("?");
                                        proto::fields(r, s, s + l, |r, no, f| {
                                            match no {
                                                1 => d = (f.int() as i64).to_string(),
                                                2 => d = proto::string(r, f, 64)?,
                                                _ => {}
                                            }
                                            Ok(())
                                        })?;
                                        dims.push(d);
                                    }
                                    Ok(())
                                })?;
                            }
                        }
                        _ => {}
                    }
                    Ok(())
                })?;
                let shape = if has_shape {
                    format!(" [{}]", dims.join(", "))
                } else {
                    String::new()
                };
                let sparse = if no == 8 { "sparse " } else { "" };
                format!("{sparse}{}{shape}", dtype(elem))
            }
            4 => "sequence".into(),
            5 => "map".into(),
            9 => "optional".into(),
            _ => return Ok(()),
        };
        Ok(())
    })?;
    Ok(label)
}

fn value_info(r: &mut Reader, start: u64, end: u64) -> Result<(String, String), Error> {
    let mut name = String::new();
    let mut ty = String::from("?");
    proto::fields(r, start, end, |r, no, f| {
        match (no, f.range()) {
            (1, _) => name = proto::string(r, f, 1024)?,
            (2, Some((s, l))) => ty = type_label(r, s, s + l)?,
            _ => {}
        }
        Ok(())
    })?;
    Ok((name, ty))
}

pub(crate) fn read(src: &dyn Source) -> Result<TensorFile, Error> {
    let mut r = Reader::new(src);
    let len = r.len();
    let mut file = TensorFile {
        format: TensorFormat::Onnx,
        ..TensorFile::default()
    };
    let mut model: Vec<(String, String)> = Vec::new();
    let mut opsets: Vec<String> = Vec::new();
    let mut graph = None;
    let mut ir = None;
    proto::fields(&mut r, 0, len, |r, no, f| {
        match no {
            1 => ir = Some(f.int()),
            2 => model.push(("Producer".into(), proto::string(r, f, 256)?)),
            3 => model.push(("Producer version".into(), proto::string(r, f, 256)?)),
            4 => model.push(("Domain".into(), proto::string(r, f, 256)?)),
            5 => model.push(("Model version".into(), f.int().to_string())),
            6 => model.push(("Doc string".into(), proto::string(r, f, 500)?)),
            7 => graph = f.range(),
            8 => {
                if let Some((s, l)) = f.range() {
                    let mut domain = String::new();
                    let mut version = 0u64;
                    proto::fields(r, s, s + l, |r, no, f| {
                        match no {
                            1 => domain = proto::string(r, f, 256)?,
                            2 => version = f.int(),
                            _ => {}
                        }
                        Ok(())
                    })?;
                    if domain.is_empty() {
                        domain = "ai.onnx".into();
                    }
                    opsets.push(format!("{domain} {version}"));
                }
            }
            14 => {
                if let Some((s, l)) = f.range() {
                    let mut k = String::new();
                    let mut v = String::new();
                    proto::fields(r, s, s + l, |r, no, f| {
                        match no {
                            1 => k = proto::string(r, f, 256)?,
                            2 => v = proto::string(r, f, 2000)?,
                            _ => {}
                        }
                        Ok(())
                    })?;
                    file.metadata.push((k, v));
                }
            }
            _ => {}
        }
        Ok(())
    })?;
    let (Some(ir), Some((gs, gl))) = (ir, graph) else {
        return Err(bad("not an ONNX model"));
    };
    let mut head = vec![("IR version".to_string(), ir.to_string())];
    if !opsets.is_empty() {
        head.push(("Opset".into(), opsets.join(", ")));
    }
    head.append(&mut model);

    let mut ops: HashMap<String, u64> = HashMap::new();
    let mut op_order: Vec<String> = Vec::new();
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    let mut sparse = 0u64;
    let mut tensors = Vec::new();
    proto::fields(&mut r, gs, gs + gl, |r, no, f| {
        let Some((s, l)) = f.range() else {
            return Ok(());
        };
        match no {
            1 => {
                let mut op = String::new();
                let mut domain = String::new();
                proto::fields(r, s, s + l, |r, no, f| {
                    match no {
                        4 => op = proto::string(r, f, 128)?,
                        7 => domain = proto::string(r, f, 128)?,
                        _ => {}
                    }
                    Ok(())
                })?;
                if !domain.is_empty() && domain != "ai.onnx" {
                    op = format!("{domain}.{op}");
                }
                let c = ops.entry(op.clone()).or_insert(0);
                if *c == 0 {
                    op_order.push(op);
                }
                *c += 1;
            }
            2 => head.push(("Graph".into(), proto::string(r, f, 256)?)),
            5 => tensors.push(tensor(r, s, s + l)?),
            11 => inputs.push(value_info(r, s, s + l)?),
            12 => outputs.push(value_info(r, s, s + l)?),
            15 => sparse += 1,
            _ => {}
        }
        Ok(())
    })?;
    file.metadata.splice(0..0, head);

    // Before IR 4 initializers were listed as inputs too.
    let init: std::collections::HashSet<&str> = tensors.iter().map(|t| t.name.as_str()).collect();
    inputs.retain(|(n, _)| !init.contains(n.as_str()));
    if !inputs.is_empty() {
        file.sections.push(MetaSection {
            title: "Inputs".into(),
            entries: inputs,
        });
    }
    if !outputs.is_empty() {
        file.sections.push(MetaSection {
            title: "Outputs".into(),
            entries: outputs,
        });
    }
    if !op_order.is_empty() {
        let mut list: Vec<(String, u64)> = op_order
            .into_iter()
            .map(|o| {
                let c = ops.get(&o).copied().unwrap_or(0);
                (o, c)
            })
            .collect();
        list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        file.sections.push(MetaSection {
            title: "Operators".into(),
            entries: list
                .into_iter()
                .map(|(o, c)| (o, format!("×{c}")))
                .collect(),
        });
    }
    if sparse > 0 {
        file.notes
            .push(format!("{sparse} sparse initializers not listed"));
    }
    file.tensors = tensors;
    Ok(file)
}
