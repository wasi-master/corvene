//! TensorFlow Lite flatbuffers (`schema.fbs` `Model`): version,
//! description, operator codes, subgraph tensors with constant buffers
//! (inline, or by offset for models over 2 GB), quantization and metadata.

use std::collections::HashMap;

use corvene_models::TensorFile;
use corvene_models::tensors::{DType, MetaSection, TensorFormat, TensorInfo, TensorLocation};

use crate::source::{Reader, bad};
use crate::{Error, Source};

const MAX_LIST: u64 = 10_000_000;

struct Fb<'a> {
    r: Reader<'a>,
}

impl Fb<'_> {
    fn u32_at(&mut self, at: u64) -> Result<u32, Error> {
        self.r.seek(at);
        self.r.u32()
    }
    fn u16_at(&mut self, at: u64) -> Result<u16, Error> {
        self.r.seek(at);
        self.r.u16()
    }

    /// Absolute position of a table's field, if present.
    fn field(&mut self, table: u64, index: u64) -> Result<Option<u64>, Error> {
        let soff = self.u32_at(table)? as i32 as i64;
        let vt = table as i64 - soff;
        if vt < 0 {
            return Err(bad("flatbuffer vtable out of bounds"));
        }
        let vt = vt as u64;
        let vt_len = self.u16_at(vt)? as u64;
        let slot = 4 + 2 * index;
        if slot + 2 > vt_len {
            return Ok(None);
        }
        let off = self.u16_at(vt + slot)? as u64;
        Ok((off != 0).then_some(table + off))
    }

    fn deref(&mut self, at: u64) -> Result<u64, Error> {
        let off = self.u32_at(at)? as u64;
        let to = at + off;
        if to >= self.r.len() {
            return Err(bad("flatbuffer offset out of bounds"));
        }
        Ok(to)
    }

    fn table(&mut self, table: u64, index: u64) -> Result<Option<u64>, Error> {
        match self.field(table, index)? {
            Some(p) => Ok(Some(self.deref(p)?)),
            None => Ok(None),
        }
    }

    /// `(first element, count)` of a vector field of `elem`-byte items.
    fn vector(&mut self, table: u64, index: u64, elem: u64) -> Result<Option<(u64, u64)>, Error> {
        let Some(v) = self.table(table, index)? else {
            return Ok(None);
        };
        let n = self.u32_at(v)? as u64;
        let start = v + 4;
        if n > MAX_LIST || n.saturating_mul(elem) > self.r.len().saturating_sub(start) {
            return Err(bad("flatbuffer vector out of bounds"));
        }
        Ok(Some((start, n)))
    }

    fn string(&mut self, table: u64, index: u64) -> Result<String, Error> {
        let Some((start, n)) = self.vector(table, index, 1)? else {
            return Ok(String::new());
        };
        self.r.seek(start);
        let s = self.r.string(n.min(4096))?;
        Ok(s)
    }

    fn scalar(&mut self, table: u64, index: u64, size: usize) -> Result<Option<u64>, Error> {
        match self.field(table, index)? {
            Some(p) => {
                self.r.seek(p);
                Ok(Some(self.r.uint(size)?))
            }
            None => Ok(None),
        }
    }

    fn i32s(&mut self, table: u64, index: u64) -> Result<Option<Vec<i64>>, Error> {
        let Some((start, n)) = self.vector(table, index, 4)? else {
            return Ok(None);
        };
        self.r.seek(start);
        let mut v = Vec::with_capacity(n.min(64) as usize);
        for _ in 0..n.min(64) {
            v.push(self.r.i32()? as i64);
        }
        Ok(Some(v))
    }

    /// The tables of a vector-of-tables field.
    fn tables(&mut self, table: u64, index: u64) -> Result<Vec<u64>, Error> {
        let Some((start, n)) = self.vector(table, index, 4)? else {
            return Ok(Vec::new());
        };
        let mut out = Vec::with_capacity(n.min(1 << 16) as usize);
        for i in 0..n {
            out.push(self.deref(start + 4 * i)?);
        }
        Ok(out)
    }
}

fn dtype(t: u64) -> DType {
    match t {
        0 => DType::F32,
        1 => DType::F16,
        2 => DType::I32,
        3 => DType::U8,
        4 => DType::I64,
        5 => DType::Str,
        6 => DType::Bool,
        7 => DType::I16,
        8 => DType::C64,
        9 => DType::I8,
        10 => DType::F64,
        11 => DType::C128,
        12 => DType::U64,
        13 => DType::Other("resource".into()),
        14 => DType::Other("variant".into()),
        15 => DType::U32,
        16 => DType::U16,
        17 => DType::Other("i4".into()),
        18 => DType::BF16,
        n => DType::Other(format!("type {n}")),
    }
}

const OPS: &[&str] = &[
    "ADD",
    "AVERAGE_POOL_2D",
    "CONCATENATION",
    "CONV_2D",
    "DEPTHWISE_CONV_2D",
    "DEPTH_TO_SPACE",
    "DEQUANTIZE",
    "EMBEDDING_LOOKUP",
    "FLOOR",
    "FULLY_CONNECTED",
    "HASHTABLE_LOOKUP",
    "L2_NORMALIZATION",
    "L2_POOL_2D",
    "LOCAL_RESPONSE_NORMALIZATION",
    "LOGISTIC",
    "LSH_PROJECTION",
    "LSTM",
    "MAX_POOL_2D",
    "MUL",
    "RELU",
    "RELU_N1_TO_1",
    "RELU6",
    "RESHAPE",
    "RESIZE_BILINEAR",
    "RNN",
    "SOFTMAX",
    "SPACE_TO_DEPTH",
    "SVDF",
    "TANH",
    "CONCAT_EMBEDDINGS",
    "SKIP_GRAM",
    "CALL",
    "CUSTOM",
    "EMBEDDING_LOOKUP_SPARSE",
    "PAD",
    "UNIDIRECTIONAL_SEQUENCE_RNN",
    "GATHER",
    "BATCH_TO_SPACE_ND",
    "SPACE_TO_BATCH_ND",
    "TRANSPOSE",
    "MEAN",
    "SUB",
    "DIV",
    "SQUEEZE",
    "UNIDIRECTIONAL_SEQUENCE_LSTM",
    "STRIDED_SLICE",
    "BIDIRECTIONAL_SEQUENCE_RNN",
    "EXP",
    "TOPK_V2",
    "SPLIT",
    "LOG_SOFTMAX",
    "DELEGATE",
    "BIDIRECTIONAL_SEQUENCE_LSTM",
    "CAST",
    "PRELU",
    "MAXIMUM",
    "ARG_MAX",
    "MINIMUM",
    "LESS",
    "NEG",
    "PADV2",
    "GREATER",
    "GREATER_EQUAL",
    "LESS_EQUAL",
    "SELECT",
    "SLICE",
    "SIN",
    "TRANSPOSE_CONV",
    "SPARSE_TO_DENSE",
    "TILE",
    "EXPAND_DIMS",
    "EQUAL",
    "NOT_EQUAL",
    "LOG",
    "SUM",
    "SQRT",
    "RSQRT",
    "SHAPE",
    "POW",
    "ARG_MIN",
    "FAKE_QUANT",
    "REDUCE_PROD",
    "REDUCE_MAX",
    "PACK",
    "LOGICAL_OR",
    "ONE_HOT",
    "LOGICAL_AND",
    "LOGICAL_NOT",
    "UNPACK",
    "REDUCE_MIN",
    "FLOOR_DIV",
    "REDUCE_ANY",
    "SQUARE",
    "ZEROS_LIKE",
    "FILL",
    "FLOOR_MOD",
    "RANGE",
    "RESIZE_NEAREST_NEIGHBOR",
    "LEAKY_RELU",
    "SQUARED_DIFFERENCE",
    "MIRROR_PAD",
    "ABS",
    "SPLIT_V",
    "UNIQUE",
    "CEIL",
    "REVERSE_V2",
    "ADD_N",
    "GATHER_ND",
    "COS",
    "WHERE",
    "RANK",
    "ELU",
    "REVERSE_SEQUENCE",
    "MATRIX_DIAG",
    "QUANTIZE",
    "MATRIX_SET_DIAG",
    "ROUND",
    "HARD_SWISH",
    "IF",
    "WHILE",
    "NON_MAX_SUPPRESSION_V4",
    "NON_MAX_SUPPRESSION_V5",
    "SCATTER_ND",
    "SELECT_V2",
    "DENSIFY",
    "SEGMENT_SUM",
    "BATCH_MATMUL",
    "PLACEHOLDER_FOR_GREATER_OP_CODES",
    "CUMSUM",
    "CALL_ONCE",
    "BROADCAST_TO",
    "RFFT2D",
    "CONV_3D",
    "IMAG",
    "REAL",
    "COMPLEX_ABS",
    "HASHTABLE",
    "HASHTABLE_FIND",
    "HASHTABLE_IMPORT",
    "HASHTABLE_SIZE",
    "REDUCE_ALL",
    "CONV_3D_TRANSPOSE",
    "VAR_HANDLE",
    "READ_VARIABLE",
    "ASSIGN_VARIABLE",
    "BROADCAST_ARGS",
    "RANDOM_STANDARD_NORMAL",
    "BUCKETIZE",
    "RANDOM_UNIFORM",
    "MULTINOMIAL",
    "GELU",
    "DYNAMIC_UPDATE_SLICE",
    "RELU_0_TO_1",
    "UNSORTED_SEGMENT_PROD",
    "UNSORTED_SEGMENT_MAX",
    "UNSORTED_SEGMENT_SUM",
    "ATAN2",
    "UNSORTED_SEGMENT_MIN",
    "SIGN",
    "BITCAST",
    "BITWISE_XOR",
    "RIGHT_SHIFT",
];

struct Buffer {
    offset: u64,
    len: u64,
}

pub(crate) fn read(src: &dyn Source) -> Result<TensorFile, Error> {
    let mut fb = Fb {
        r: Reader::new(src),
    };
    if fb.r.len() < 8 || fb.r.bytes_at(4, 4)? != b"TFL3" {
        return Err(bad("not a TensorFlow Lite model"));
    }
    let model = fb.deref(0)?;
    let mut file = TensorFile {
        format: TensorFormat::TfLite,
        ..TensorFile::default()
    };
    if let Some(v) = fb.scalar(model, 0, 4)? {
        file.metadata.push(("Schema version".into(), v.to_string()));
    }
    let desc = fb.string(model, 3)?;
    if !desc.is_empty() {
        file.metadata.push(("Description".into(), desc));
    }

    let mut op_names = Vec::new();
    for oc in fb.tables(model, 1)? {
        let dep = fb.scalar(oc, 0, 1)?.unwrap_or(0) as i8 as i64;
        let builtin = fb.scalar(oc, 3, 4)?.unwrap_or(0) as i32 as i64;
        let code = dep.max(builtin);
        let custom = fb.string(oc, 1)?;
        op_names.push(if !custom.is_empty() {
            custom
        } else {
            OPS.get(code as usize)
                .map_or_else(|| format!("op {code}"), |s| s.to_string())
        });
    }

    let mut buffers = Vec::new();
    for b in fb.tables(model, 4)? {
        let (offset, len) = match fb.vector(b, 0, 1)? {
            Some((s, n)) if n > 0 => (s, n),
            _ => {
                let off = fb.scalar(b, 1, 8)?.unwrap_or(0);
                let size = fb.scalar(b, 2, 8)?.unwrap_or(0);
                if off > 1 && off.checked_add(size).is_some_and(|e| e <= src.len()) {
                    (off, size)
                } else {
                    (0, 0)
                }
            }
        };
        buffers.push(Buffer { offset, len });
    }

    let mut ops: HashMap<String, u64> = HashMap::new();
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    let mut quant = Vec::new();
    let subgraphs = fb.tables(model, 2)?;
    let many = subgraphs.len() > 1;
    for (si, sg) in subgraphs.iter().enumerate() {
        let sg_name = fb.string(*sg, 4)?;
        let label = |n: &str| {
            if many {
                format!(
                    "{}{n}",
                    if sg_name.is_empty() {
                        format!("subgraph {si}/")
                    } else {
                        format!("{sg_name}/")
                    }
                )
            } else {
                n.to_string()
            }
        };
        let mut described = Vec::new();
        for t in fb.tables(*sg, 0)? {
            let name = fb.string(t, 3)?;
            let dtype = dtype(fb.scalar(t, 1, 1)?.unwrap_or(0));
            let shape_raw = fb.i32s(t, 0)?.unwrap_or_default();
            let sig = fb.i32s(t, 7)?.unwrap_or_default();
            let shape: Vec<u64> = shape_raw.iter().map(|d| (*d).max(0) as u64).collect();
            let dim_names: Vec<String> = if sig.len() == shape.len() && sig.iter().any(|d| *d < 0) {
                sig.iter()
                    .map(|d| if *d < 0 { "?".into() } else { String::new() })
                    .collect()
            } else {
                Vec::new()
            };
            let buf = fb.scalar(t, 2, 4)?.unwrap_or(0) as usize;
            if let Some(q) = fb.table(t, 4)? {
                let scales = fb.vector(q, 2, 4)?;
                let zps = fb.vector(q, 3, 8)?;
                if let Some((s, n)) = scales.filter(|v| v.1 > 0) {
                    let summary = if n == 1 {
                        fb.r.seek(s);
                        let scale = f32::from_bits(fb.r.u32()?);
                        let zp = match zps.filter(|z| z.1 > 0) {
                            Some((z, _)) => {
                                fb.r.seek(z);
                                fb.r.u64()? as i64
                            }
                            None => 0,
                        };
                        format!("scale {}, zero point {zp}", crate::fmt_f64(scale as f64))
                    } else {
                        let dim = fb.scalar(q, 6, 4)?.unwrap_or(0);
                        format!("{n} scales along dimension {dim}")
                    };
                    if quant.len() < 1000 {
                        quant.push((label(&name), summary));
                    }
                }
            }
            let elements = crate::elements(&shape).unwrap_or(0);
            let info = TensorInfo {
                name: label(&name),
                dtype,
                shape,
                dim_names,
                elements,
                ..TensorInfo::default()
            };
            match buffers.get(buf) {
                Some(b) if b.len > 0 => {
                    let mut info = info;
                    info.bytes = b.len;
                    if info.dtype.size().and_then(|s| s.checked_mul(elements)) == Some(b.len) {
                        info.location = TensorLocation::Bytes {
                            offset: b.offset,
                            len: b.len,
                        };
                    }
                    file.tensors.push(info.clone());
                    described.push(info);
                }
                _ => described.push(info),
            }
        }
        let io = |fb: &mut Fb, idx: u64, out: &mut Vec<(String, String)>| -> Result<(), Error> {
            if let Some(list) = fb.i32s(*sg, idx)? {
                for i in list {
                    if let Some(t) = usize::try_from(i).ok().and_then(|i| described.get(i)) {
                        out.push((t.name.clone(), format!("{} {}", t.dtype, t.shape_label())));
                    }
                }
            }
            Ok(())
        };
        io(&mut fb, 1, &mut inputs)?;
        io(&mut fb, 2, &mut outputs)?;
        for op in fb.tables(*sg, 3)? {
            let idx = fb.scalar(op, 0, 4)?.unwrap_or(0) as usize;
            let name = op_names
                .get(idx)
                .cloned()
                .unwrap_or_else(|| format!("opcode {idx}"));
            *ops.entry(name).or_insert(0) += 1;
        }
    }
    if many {
        file.metadata
            .push(("Subgraphs".into(), subgraphs.len().to_string()));
    }

    for m in fb.tables(model, 6)? {
        let name = fb.string(m, 0)?;
        let buf = fb.scalar(m, 1, 4)?.unwrap_or(0) as usize;
        let value = match buffers.get(buf) {
            Some(b) if b.len > 0 && b.len <= 256 => {
                let bytes = fb.r.bytes_at(b.offset, b.len)?;
                let text = String::from_utf8_lossy(&bytes)
                    .trim_end_matches('\0')
                    .to_string();
                if text.chars().all(|c| !c.is_control()) {
                    text
                } else {
                    format!("{} bytes", b.len)
                }
            }
            Some(b) => format!("{} bytes", b.len),
            None => String::new(),
        };
        file.metadata.push((name, value));
    }

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
    if !quant.is_empty() {
        file.sections.push(MetaSection {
            title: "Quantization".into(),
            entries: quant,
        });
    }
    Ok(file)
}
