//! Core ML `.mlmodel` (protobuf `Model`): specification version,
//! description inputs, outputs and metadata, and neural network layer
//! weights (`WeightParams` float, float16 and raw values).

use std::collections::HashMap;

use corvene_models::TensorFile;
use corvene_models::tensors::{DType, MetaSection, TensorFormat, TensorInfo, TensorLocation};

use crate::proto::{self, Field};
use crate::source::{Reader, bad};
use crate::{Error, Source};

fn layer_kind(no: u32) -> String {
    match no {
        100 => "convolution",
        120 => "pooling",
        130 => "activation",
        140 => "innerProduct",
        150 => "embedding",
        160 => "batchnorm",
        165 => "mvn",
        170 => "l2normalize",
        175 => "softmax",
        180 => "lrn",
        190 => "crop",
        200 => "padding",
        210 => "upsample",
        211 => "resizeBilinear",
        212 => "cropResize",
        220 => "unary",
        230 => "add",
        231 => "multiply",
        240 => "average",
        245 => "scale",
        250 => "bias",
        260 => "max",
        261 => "min",
        270 => "dot",
        280 => "reduce",
        290 => "loadConstant",
        300 => "reshape",
        301 => "flatten",
        310 => "permute",
        320 => "concat",
        330 => "split",
        340 => "sequenceRepeat",
        345 => "reorganizeData",
        350 => "slice",
        400 => "simpleRecurrent",
        410 => "gru",
        420 => "uniDirectionalLSTM",
        430 => "biDirectionalLSTM",
        500 => "custom",
        n => return format!("layer {n}"),
    }
    .to_string()
}

/// Names of weight fields of common layer parameter messages.
fn weight_name(kind: u32, field: u32) -> String {
    match (kind, field) {
        (100, 90) | (140, 20) | (150, 20) => "weights",
        (100, 91) | (140, 21) | (150, 21) => "bias",
        (160, 15) => "gamma",
        (160, 16) => "beta",
        (160, 17) => "mean",
        (160, 18) => "variance",
        (245, 2) => "scale",
        (245, 4) => "bias",
        (250, 2) | (290, 2) => "data",
        _ => return format!("field{field}"),
    }
    .to_string()
}

/// A `WeightParams` message, if `start..end` parses as one.
fn weights(r: &mut Reader, start: u64, end: u64) -> Option<(DType, u64, u64)> {
    let mut found = None;
    let mut ok = true;
    let res = proto::fields(r, start, end, |_, no, f| {
        match (no, f) {
            (1, Field::Len { offset, len }) if len % 4 == 0 => {
                found = Some((DType::F32, offset, len))
            }
            (2, Field::Len { offset, len }) if len % 2 == 0 => {
                found = Some((DType::F16, offset, len))
            }
            (30, Field::Len { offset, len }) => found = Some((DType::U8, offset, len)),
            (31, Field::Len { offset, len }) => found = Some((DType::I8, offset, len)),
            (40, Field::Len { .. }) | (50, Field::Varint(_)) => {}
            _ => ok = false,
        }
        Ok(())
    });
    (res.is_ok() && ok).then_some(found).flatten()
}

pub(crate) fn read(src: &dyn Source) -> Result<TensorFile, Error> {
    let mut r = Reader::new(src);
    let len = r.len();
    let mut file = TensorFile {
        format: TensorFormat::CoreMl,
        ..TensorFile::default()
    };
    let mut spec = None;
    let mut desc = None;
    let mut network = None;
    let mut kind = None;
    proto::fields(&mut r, 0, len, |_, no, f| {
        match no {
            1 => spec = Some(f.int()),
            2 => desc = f.range(),
            500 | 501 | 403 => network = f.range(),
            n if n >= 200 => kind = Some(n),
            _ => {}
        }
        Ok(())
    })?;
    let spec = spec.ok_or_else(|| bad("not a Core ML model"))?;
    file.metadata
        .push(("Specification version".into(), spec.to_string()));
    if let Some(k) = kind {
        let label = match k {
            502 => "ML Program".to_string(),
            200..=202 => "Pipeline".to_string(),
            n => format!("model type {n}"),
        };
        file.metadata.push(("Model type".into(), label));
        if k == 502 {
            file.notes
                .push("ML Program weights live outside the .mlmodel (weights/weight.bin)".into());
        }
    }
    if let Some((s, l)) = desc {
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        proto::fields(&mut r, s, s + l, |r, no, f| {
            let Some((s, l)) = f.range() else {
                return Ok(());
            };
            match no {
                1 | 10 => {
                    let mut name = String::new();
                    let mut ty = String::from("?");
                    proto::fields(r, s, s + l, |r, no, f| {
                        match no {
                            1 => name = proto::string(r, f, 256)?,
                            3 => {
                                if let Some((s, l)) = f.range() {
                                    proto::fields(r, s, s + l, |_, no, _| {
                                        ty = match no {
                                            1 => "int64",
                                            2 => "double",
                                            3 => "string",
                                            4 => "image",
                                            5 => "multiarray",
                                            6 => "dictionary",
                                            7 => "sequence",
                                            8 => "state",
                                            _ => return Ok(()),
                                        }
                                        .to_string();
                                        Ok(())
                                    })?;
                                }
                            }
                            _ => {}
                        }
                        Ok(())
                    })?;
                    if no == 1 { &mut inputs } else { &mut outputs }.push((name, ty));
                }
                100 => {
                    proto::fields(r, s, s + l, |r, no, f| {
                        let key = match no {
                            1 => "Description",
                            2 => "Version",
                            3 => "Author",
                            4 => "License",
                            100 => {
                                if let Some((s, l)) = f.range() {
                                    let mut k = String::new();
                                    let mut v = String::new();
                                    proto::fields(r, s, s + l, |r, no, f| {
                                        match no {
                                            1 => k = proto::string(r, f, 256)?,
                                            2 => v = proto::string(r, f, 1000)?,
                                            _ => {}
                                        }
                                        Ok(())
                                    })?;
                                    file.metadata.push((k, v));
                                }
                                return Ok(());
                            }
                            _ => return Ok(()),
                        };
                        let v = proto::string(r, f, 1000)?;
                        file.metadata.push((key.into(), v));
                        Ok(())
                    })?;
                }
                _ => {}
            }
            Ok(())
        })?;
        for (title, entries) in [("Inputs", inputs), ("Outputs", outputs)] {
            if !entries.is_empty() {
                file.sections.push(MetaSection {
                    title: title.into(),
                    entries,
                });
            }
        }
    }

    let mut ops: HashMap<String, u64> = HashMap::new();
    if let Some((s, l)) = network {
        let mut layers = Vec::new();
        proto::fields(&mut r, s, s + l, |_, no, f| {
            if no == 1
                && let Some(range) = f.range()
            {
                layers.push(range);
            }
            Ok(())
        })?;
        for (s, l) in layers {
            let mut name = String::new();
            let mut params = None;
            proto::fields(&mut r, s, s + l, |r, no, f| {
                match no {
                    1 => name = proto::string(r, f, 512)?,
                    n if n >= 100 => {
                        if let Some(range) = f.range() {
                            params = Some((n, range));
                        }
                    }
                    _ => {}
                }
                Ok(())
            })?;
            let Some((kind, (ps, pl))) = params else {
                continue;
            };
            *ops.entry(layer_kind(kind)).or_insert(0) += 1;
            let mut cands = Vec::new();
            proto::fields(&mut r, ps, ps + pl, |_, no, f| {
                if let Some(range) = f.range() {
                    cands.push((no, range));
                }
                Ok(())
            })?;
            for (no, (ws, wl)) in cands {
                if let Some((dtype, offset, len)) = weights(&mut r, ws, ws + wl) {
                    let es = dtype.size().unwrap_or(1);
                    let elements = len / es;
                    file.tensors.push(TensorInfo {
                        name: format!("{name}.{}", weight_name(kind, no)),
                        dtype,
                        shape: vec![elements],
                        elements,
                        bytes: len,
                        location: TensorLocation::Bytes { offset, len },
                        ..TensorInfo::default()
                    });
                }
            }
        }
    }
    if !ops.is_empty() {
        let mut list: Vec<(String, u64)> = ops.into_iter().collect();
        list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        file.sections.push(MetaSection {
            title: "Layers".into(),
            entries: list
                .into_iter()
                .map(|(o, c)| (o, format!("×{c}")))
                .collect(),
        });
    }
    if !file.tensors.is_empty() {
        file.notes
            .push("Core ML weights are listed flat (layer shapes not read)".into());
    }
    Ok(file)
}
