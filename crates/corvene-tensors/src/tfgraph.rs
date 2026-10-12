//! TensorFlow `GraphDef` (frozen `.pb`) and `SavedModel` (`saved_model.pb`):
//! `Const` nodes with their tensor attribute (dtype, shape, content) and
//! operator counts. SavedModel variables live in `variables/`, not read.

use std::collections::HashMap;

use corvene_models::TensorFile;
use corvene_models::tensors::{DType, MetaSection, TensorFormat, TensorInfo, TensorLocation};

use crate::proto::{self, Field};
use crate::source::{Reader, bad};
use crate::{Error, Source};

fn dtype(n: u64) -> DType {
    match n {
        1 => DType::F32,
        2 => DType::F64,
        3 => DType::I32,
        4 => DType::U8,
        5 => DType::I16,
        6 => DType::I8,
        7 => DType::Str,
        8 => DType::C64,
        9 => DType::I64,
        10 => DType::Bool,
        14 => DType::BF16,
        17 => DType::U16,
        18 => DType::C128,
        19 => DType::F16,
        22 => DType::U32,
        23 => DType::U64,
        n => DType::Other(format!("tf type {n}")),
    }
}

/// A TF `TensorProto` named `name`.
fn tensor(r: &mut Reader, name: String, start: u64, end: u64) -> Result<TensorInfo, Error> {
    let mut dt = 0;
    let mut shape: Vec<u64> = Vec::new();
    let mut content = None;
    let mut fixed = None;
    let mut varints = None;
    proto::fields(r, start, end, |r, no, f| {
        match no {
            1 => dt = f.int(),
            2 => {
                if let Some((s, l)) = f.range() {
                    proto::fields(r, s, s + l, |r, no, f| {
                        if no == 2
                            && let Some((s, l)) = f.range()
                            && shape.len() < 32
                        {
                            let mut size = 0i64;
                            proto::fields(r, s, s + l, |_, no, f| {
                                if no == 1 {
                                    size = f.int() as i64;
                                }
                                Ok(())
                            })?;
                            shape.push(size.max(0) as u64);
                        }
                        Ok(())
                    })?;
                }
            }
            4 => content = f.range(),
            5 | 6 => fixed = f.range(),
            7 | 10 | 11 | 13 | 16 | 17 => varints = f.range(),
            _ => {}
        }
        Ok(())
    })?;
    let dtype = dtype(dt);
    let elements = crate::elements(&shape).ok_or_else(|| bad("shape overflows"))?;
    let want = dtype.size().and_then(|s| s.checked_mul(elements));
    let (location, bytes) = match (content, fixed, varints) {
        (Some((offset, len)), _, _) if Some(len) == want => {
            (TensorLocation::Bytes { offset, len }, len)
        }
        (None, Some((offset, len)), _) if Some(len) == want => {
            (TensorLocation::Bytes { offset, len }, len)
        }
        (None, None, Some((offset, len))) if elements > 0 => {
            (TensorLocation::Varints { offset, len }, len)
        }
        (c, f, v) => (TensorLocation::Missing, c.or(f).or(v).map_or(0, |r| r.1)),
    };
    // A short typed list fills the tensor with its last value.
    let location = match location {
        TensorLocation::Varints { offset, len } => {
            let keep = r.pos();
            r.seek(offset);
            let mut n = 0u64;
            while r.pos() < offset + len {
                proto::varint(r)?;
                n += 1;
            }
            r.seek(keep);
            if n == elements {
                TensorLocation::Varints { offset, len }
            } else {
                TensorLocation::Missing
            }
        }
        l => l,
    };
    Ok(TensorInfo {
        name,
        dtype,
        shape,
        elements,
        bytes,
        location,
        ..TensorInfo::default()
    })
}

fn graph(
    r: &mut Reader,
    start: u64,
    end: u64,
    file: &mut TensorFile,
    ops: &mut HashMap<String, u64>,
) -> Result<(), Error> {
    proto::fields(r, start, end, |r, no, f| {
        let Some((s, l)) = f.range() else {
            return Ok(());
        };
        match no {
            1 => {
                let mut name = String::new();
                let mut op = String::new();
                let mut value = None;
                proto::fields(r, s, s + l, |r, no, f| {
                    match no {
                        1 => name = proto::string(r, f, 1024)?,
                        2 => op = proto::string(r, f, 128)?,
                        5 => {
                            if let Some((s, l)) = f.range() {
                                let mut key = String::new();
                                let mut val = None;
                                proto::fields(r, s, s + l, |r, no, f| {
                                    match no {
                                        1 => key = proto::string(r, f, 64)?,
                                        2 => val = f.range(),
                                        _ => {}
                                    }
                                    Ok(())
                                })?;
                                if key == "value"
                                    && let Some((s, l)) = val
                                {
                                    proto::fields(r, s, s + l, |_, no, f| {
                                        if no == 8 {
                                            value = f.range();
                                        }
                                        Ok(())
                                    })?;
                                }
                            }
                        }
                        _ => {}
                    }
                    Ok(())
                })?;
                *ops.entry(op.clone()).or_insert(0) += 1;
                if op == "Const"
                    && let Some((s, l)) = value
                {
                    file.tensors.push(tensor(r, name, s, s + l)?);
                }
            }
            4 => {
                proto::fields(r, s, s + l, |_, no, f| {
                    if no == 1 {
                        file.metadata.push(("Producer".into(), f.int().to_string()));
                    }
                    Ok(())
                })?;
            }
            _ => {}
        }
        Ok(())
    })
}

pub(crate) fn read(src: &dyn Source) -> Result<TensorFile, Error> {
    let mut r = Reader::new(src);
    let len = r.len();
    let mut file = TensorFile {
        format: TensorFormat::TfGraph,
        ..TensorFile::default()
    };
    // SavedModel: field 1 is a varint (schema version), 2 the meta graphs.
    let mut first: Option<Field> = None;
    let mut graphs = Vec::new();
    let mut nodes = false;
    proto::fields(&mut r, 0, len, |r, no, f| {
        if first.is_none() {
            first = Some(f);
        }
        match (no, f) {
            (1, Field::Varint(v)) => file
                .metadata
                .push(("SavedModel schema version".into(), v.to_string())),
            (2, Field::Len { offset, len }) if matches!(first, Some(Field::Varint(_))) => {
                proto::fields(r, offset, offset + len, |_, no, f| {
                    if no == 2
                        && let Some(range) = f.range()
                    {
                        graphs.push(range);
                    }
                    Ok(())
                })?;
            }
            (1, Field::Len { .. }) => nodes = true,
            _ => {}
        }
        Ok(())
    })?;
    let mut ops = HashMap::new();
    if graphs.is_empty() {
        if !nodes {
            return Err(bad("not a TensorFlow graph"));
        }
        graph(&mut r, 0, len, &mut file, &mut ops)?;
    } else {
        for (s, l) in graphs {
            graph(&mut r, s, s + l, &mut file, &mut ops)?;
        }
        file.notes
            .push("SavedModel variables (variables/) not read".into());
    }
    if !ops.is_empty() {
        let mut list: Vec<(String, u64)> = ops.into_iter().collect();
        list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        file.sections.push(MetaSection {
            title: "Operators".into(),
            entries: list
                .into_iter()
                .map(|(o, c)| (o, format!("×{c}")))
                .collect(),
        });
    }
    Ok(file)
}
