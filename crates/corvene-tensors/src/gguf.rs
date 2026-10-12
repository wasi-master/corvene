//! GGUF (llama.cpp, versions 1 to 3): metadata key/values, tensor infos
//! and the aligned data section. Values of the plain and common block
//! quant types are decoded in `values`.

use corvene_models::TensorFile;
use corvene_models::tensors::{DType, MetaSection, TensorFormat, TensorInfo, TensorLocation};

use crate::source::{Reader, bad};
use crate::values::GGML_TYPES;
use crate::{Error, Source};

/// Array items kept for display.
const SHOWN_ITEMS: usize = 5;
const MAX_DIMS: u32 = 8;

struct Gguf<'a> {
    r: Reader<'a>,
    version: u32,
}

impl Gguf<'_> {
    fn count(&mut self) -> Result<u64, Error> {
        if self.version == 1 {
            Ok(self.r.u32()? as u64)
        } else {
            self.r.u64()
        }
    }

    fn string(&mut self) -> Result<String, Error> {
        let n = self.count()?;
        self.r.string(n)
    }

    /// One value of type `t`; arrays recurse once (nested arrays are
    /// skipped through, shown by size).
    fn value(&mut self, t: u32, depth: u32) -> Result<String, Error> {
        Ok(match t {
            0 => self.r.u8()?.to_string(),
            1 => (self.r.u8()? as i8).to_string(),
            2 => self.r.u16()?.to_string(),
            3 => (self.r.u16()? as i16).to_string(),
            4 => self.r.u32()?.to_string(),
            5 => self.r.i32()?.to_string(),
            6 => crate::fmt_f64(f32::from_bits(self.r.u32()?) as f64),
            7 => (self.r.u8()? != 0).to_string(),
            8 => crate::clip(&self.string()?, 2000),
            10 => self.r.u64()?.to_string(),
            11 => (self.r.u64()? as i64).to_string(),
            12 => crate::fmt_f64(f64::from_bits(self.r.u64()?)),
            9 => {
                if depth > 4 {
                    return Err(bad("GGUF arrays nested too deeply"));
                }
                let et = self.r.u32()?;
                let n = self.count()?;
                if n > self.r.remaining() {
                    return Err(bad("GGUF array longer than the file"));
                }
                let fixed = match et {
                    0 | 1 | 7 => Some(1),
                    2 | 3 => Some(2),
                    4..=6 => Some(4),
                    10..=12 => Some(8),
                    _ => None,
                };
                let mut shown = Vec::new();
                for i in 0..n {
                    if i as usize >= SHOWN_ITEMS {
                        if let Some(size) = fixed {
                            self.r.skip(
                                (n - i)
                                    .checked_mul(size)
                                    .ok_or_else(|| bad("GGUF array too long"))?,
                            )?;
                            break;
                        }
                        if et == 8 {
                            let len = self.count()?;
                            self.r.skip(len)?;
                        } else {
                            self.value(et, depth + 1)?;
                        }
                        continue;
                    }
                    let v = self.value(et, depth + 1)?;
                    shown.push(if et == 8 {
                        format!("{:?}", crate::clip(&v, 40))
                    } else {
                        v
                    });
                }
                crate::summarize(&shown, n)
            }
            _ => return Err(bad(format!("unknown GGUF value type {t}"))),
        })
    }
}

pub(crate) fn read(src: &dyn Source) -> Result<TensorFile, Error> {
    let mut g = Gguf {
        r: Reader::new(src),
        version: 0,
    };
    if g.r.take(4)? != b"GGUF" {
        return Err(bad("not a GGUF file"));
    }
    g.version = g.r.u32()?;
    if !(1..=3).contains(&g.version) {
        return Err(bad(format!("GGUF version {} not supported", g.version)));
    }
    let n_tensors = g.count()?;
    let n_kv = g.count()?;
    // Smallest possible entries bound the counts.
    if n_tensors > g.r.remaining() / 8 || n_kv > g.r.remaining() / 8 {
        return Err(bad("GGUF counts larger than the file"));
    }

    let mut file = TensorFile {
        format: TensorFormat::Gguf,
        ..TensorFile::default()
    };
    file.metadata
        .push(("GGUF version".into(), g.version.to_string()));
    let mut tokenizer = MetaSection {
        title: "Tokenizer".into(),
        entries: Vec::new(),
    };
    let mut alignment = 32u64;
    for _ in 0..n_kv {
        let key = g.string()?;
        let t = g.r.u32()?;
        if key == "general.alignment" && matches!(t, 4 | 5 | 10 | 11) {
            let at = g.r.pos();
            let v = if t >= 10 {
                g.r.u64()?
            } else {
                g.r.u32()? as u64
            };
            g.r.seek(at);
            if v.is_power_of_two() && v <= 1 << 20 {
                alignment = v;
            }
        }
        let value = g.value(t, 0)?;
        if key.starts_with("tokenizer.") {
            tokenizer.entries.push((key, value));
        } else {
            file.metadata.push((key, value));
        }
    }
    if !tokenizer.entries.is_empty() {
        file.sections.push(tokenizer);
    }

    let mut infos = Vec::with_capacity(n_tensors.min(1 << 20) as usize);
    for _ in 0..n_tensors {
        let name = g.string()?;
        let n_dims = g.r.u32()?;
        if n_dims > MAX_DIMS {
            return Err(bad(format!("tensor {name} has {n_dims} dimensions")));
        }
        let mut shape = Vec::with_capacity(n_dims as usize);
        for _ in 0..n_dims {
            shape.push(if g.version == 1 {
                g.r.u32()? as u64
            } else {
                g.r.u64()?
            });
        }
        let ty = g.r.u32()?;
        let offset = g.r.u64()?;
        infos.push((name, shape, ty, offset));
    }
    let data_start = g.r.pos().div_ceil(alignment) * alignment;
    let data_len = src.len().saturating_sub(data_start);

    for (name, shape, ty, offset) in infos {
        let elements = crate::elements(&shape).ok_or_else(|| bad("shape overflows"))?;
        let ggml = GGML_TYPES.iter().find(|t| t.0 == ty);
        let (dtype, bytes) = match ggml {
            Some(&(_, tn, be, bb)) => {
                let dtype = match tn {
                    "F32" => DType::F32,
                    "F16" => DType::F16,
                    "BF16" => DType::BF16,
                    "F64" => DType::F64,
                    "I8" => DType::I8,
                    "I16" => DType::I16,
                    "I32" => DType::I32,
                    "I64" => DType::I64,
                    q => DType::Quant(q.to_string()),
                };
                (dtype, elements.div_ceil(be).checked_mul(bb))
            }
            None => (DType::Other(format!("ggml type {ty}")), None),
        };
        let location = match bytes {
            Some(len) if offset.checked_add(len).is_some_and(|e| e <= data_len) => {
                TensorLocation::Bytes {
                    offset: data_start + offset,
                    len,
                }
            }
            Some(_) => {
                file.notes
                    .push(format!("{name}: data runs past the end of the file"));
                TensorLocation::Missing
            }
            None => TensorLocation::Missing,
        };
        file.tensors.push(TensorInfo {
            name,
            dtype,
            shape,
            elements,
            bytes: bytes.unwrap_or(0),
            location,
            ..TensorInfo::default()
        });
    }
    Ok(file)
}
