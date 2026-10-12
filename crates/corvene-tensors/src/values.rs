//! Tensor values: element decoding (plain types and GGML block quants),
//! chunked streams through `Source::read_at`, statistics and the
//! element-wise comparison of two versions.

use std::collections::HashMap;
use std::io::Read;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use corvene_models::TensorFile;
use corvene_models::tensors::{
    DType, DiffSample, TensorInfo, TensorLocation, TensorStats, TensorValues, ValueDiff,
};

use crate::Source;
use crate::zipfile::{self, Member};

/// Bytes read per step.
const CHUNK: usize = 4 * 1024 * 1024;
/// Largest compressed zip member decompressed whole (HDF5 inside `.keras`).
const MEMBER_CAP: u64 = 256 * 1024 * 1024;
/// Values sampled for the histogram.
const HIST_SAMPLES: u64 = 65536;
const BUCKETS: usize = 16;

/// GGML tensor types: id, name, elements per block, bytes per block.
pub(crate) const GGML_TYPES: &[(u32, &str, u64, u64)] = &[
    (0, "F32", 1, 4),
    (1, "F16", 1, 2),
    (2, "Q4_0", 32, 18),
    (3, "Q4_1", 32, 20),
    (6, "Q5_0", 32, 22),
    (7, "Q5_1", 32, 24),
    (8, "Q8_0", 32, 34),
    (9, "Q8_1", 32, 36),
    (10, "Q2_K", 256, 84),
    (11, "Q3_K", 256, 110),
    (12, "Q4_K", 256, 144),
    (13, "Q5_K", 256, 176),
    (14, "Q6_K", 256, 210),
    (15, "Q8_K", 256, 292),
    (16, "IQ2_XXS", 256, 66),
    (17, "IQ2_XS", 256, 74),
    (18, "IQ3_XXS", 256, 98),
    (19, "IQ1_S", 256, 50),
    (20, "IQ4_NL", 32, 18),
    (21, "IQ3_S", 256, 110),
    (22, "IQ2_S", 256, 82),
    (23, "IQ4_XS", 256, 136),
    (24, "I8", 1, 1),
    (25, "I16", 1, 2),
    (26, "I32", 1, 4),
    (27, "I64", 1, 8),
    (28, "F64", 1, 8),
    (29, "IQ1_M", 256, 56),
    (30, "BF16", 1, 2),
    (34, "TQ1_0", 256, 54),
    (35, "TQ2_0", 256, 66),
    (39, "MXFP4", 32, 17),
];

/// Block layout of a GGML quant type, by name.
pub(crate) fn quant_layout(name: &str) -> Option<(u64, u64)> {
    GGML_TYPES
        .iter()
        .find(|t| t.1 == name && t.2 > 1)
        .map(|t| (t.2, t.3))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Quant {
    Q4_0,
    Q4_1,
    Q5_0,
    Q5_1,
    Q8_0,
    Q8_1,
    Q2K,
    Q3K,
    Q4K,
    Q5K,
    Q6K,
    Q8K,
    Iq4Nl,
    Iq4Xs,
    Mxfp4,
}

enum Decoder {
    Plain { dtype: DType, size: usize, be: bool },
    Quant { q: Quant, block_bytes: usize },
}

fn decoder(info: &TensorInfo) -> Result<Decoder, String> {
    if let DType::Quant(name) = &info.dtype {
        let q = match name.as_str() {
            "Q4_0" => Quant::Q4_0,
            "Q4_1" => Quant::Q4_1,
            "Q5_0" => Quant::Q5_0,
            "Q5_1" => Quant::Q5_1,
            "Q8_0" => Quant::Q8_0,
            "Q8_1" => Quant::Q8_1,
            "Q2_K" => Quant::Q2K,
            "Q3_K" => Quant::Q3K,
            "Q4_K" => Quant::Q4K,
            "Q5_K" => Quant::Q5K,
            "Q6_K" => Quant::Q6K,
            "Q8_K" => Quant::Q8K,
            "IQ4_NL" => Quant::Iq4Nl,
            "IQ4_XS" => Quant::Iq4Xs,
            "MXFP4" => Quant::Mxfp4,
            _ => return Err(format!("values not decoded for {name}")),
        };
        let (_, bytes) =
            quant_layout(name).ok_or_else(|| format!("values not decoded for {name}"))?;
        return Ok(Decoder::Quant {
            q,
            block_bytes: bytes as usize,
        });
    }
    match info.dtype {
        DType::C64 | DType::C128 | DType::Str | DType::Other(_) | DType::Quant(_) => {
            Err(format!("values not decoded for {}", info.dtype))
        }
        _ => Ok(Decoder::Plain {
            dtype: info.dtype.clone(),
            size: info.dtype.size().unwrap_or(1) as usize,
            be: info.big_endian,
        }),
    }
}

impl Decoder {
    fn unit(&self) -> usize {
        match self {
            Decoder::Plain { size, .. } => *size,
            Decoder::Quant { block_bytes, .. } => *block_bytes,
        }
    }

    /// Decodes whole units of `bytes` into `out`.
    fn decode(&self, bytes: &[u8], out: &mut Vec<f64>) {
        match self {
            Decoder::Plain { dtype, size, be } => {
                let mut tmp = [0u8; 8];
                for e in bytes.chunks_exact(*size) {
                    let b = &mut tmp[..*size];
                    b.copy_from_slice(e);
                    if *be {
                        b.reverse();
                    }
                    out.push(plain(dtype, b));
                }
            }
            Decoder::Quant { q, block_bytes } => {
                for blk in bytes.chunks_exact(*block_bytes) {
                    dequant(*q, blk, out);
                }
            }
        }
    }
}

fn le<const N: usize>(b: &[u8]) -> [u8; N] {
    let mut a = [0u8; N];
    a.copy_from_slice(&b[..N]);
    a
}

/// One little-endian element.
fn plain(dtype: &DType, b: &[u8]) -> f64 {
    match dtype {
        DType::Bool => (b[0] != 0) as u8 as f64,
        DType::U8 => b[0] as f64,
        DType::I8 => b[0] as i8 as f64,
        DType::U16 => u16::from_le_bytes(le(b)) as f64,
        DType::I16 => i16::from_le_bytes(le(b)) as f64,
        DType::U32 => u32::from_le_bytes(le(b)) as f64,
        DType::I32 => i32::from_le_bytes(le(b)) as f64,
        DType::U64 => u64::from_le_bytes(le(b)) as f64,
        DType::I64 => i64::from_le_bytes(le(b)) as f64,
        DType::F16 => half::f16::from_bits(u16::from_le_bytes(le(b))).to_f64(),
        DType::BF16 => half::bf16::from_bits(u16::from_le_bytes(le(b))).to_f64(),
        DType::F32 => f32::from_le_bytes(le(b)) as f64,
        DType::F64 => f64::from_le_bytes(le(b)),
        DType::F8E4M3 => f8_e4m3(b[0]),
        DType::F8E5M2 => half::f16::from_bits((b[0] as u16) << 8).to_f64(),
        _ => f64::NAN,
    }
}

fn f8_e4m3(b: u8) -> f64 {
    let sign = if b & 0x80 != 0 { -1.0 } else { 1.0 };
    let exp = ((b >> 3) & 0xf) as i32;
    let man = (b & 7) as f64;
    if exp == 15 && b & 7 == 7 {
        return f64::NAN;
    }
    if exp == 0 {
        sign * man / 8.0 * 2f64.powi(-6)
    } else {
        sign * (1.0 + man / 8.0) * 2f64.powi(exp - 7)
    }
}

fn h(b: &[u8], at: usize) -> f64 {
    half::f16::from_bits(u16::from_le_bytes([b[at], b[at + 1]])).to_f64()
}

fn scale_min_k4(j: usize, q: &[u8]) -> (f64, f64) {
    if j < 4 {
        ((q[j] & 63) as f64, (q[j + 4] & 63) as f64)
    } else {
        let d = (q[j + 4] & 0xf) | ((q[j - 4] >> 6) << 4);
        let m = (q[j + 4] >> 4) | ((q[j] >> 6) << 4);
        (d as f64, m as f64)
    }
}

const IQ4NL: [i8; 16] = [
    -127, -104, -83, -65, -49, -35, -22, -10, 1, 13, 25, 38, 53, 69, 89, 113,
];
const MXFP4: [i8; 16] = [0, 1, 2, 3, 4, 6, 8, 12, 0, -1, -2, -3, -4, -6, -8, -12];

/// Dequantizes one GGML block (ggml-quants.c `dequantize_row_*`).
fn dequant(q: Quant, b: &[u8], y: &mut Vec<f64>) {
    match q {
        Quant::Q4_0 | Quant::Q4_1 => {
            let d = h(b, 0);
            let (m, qs) = if q == Quant::Q4_1 {
                (h(b, 2), &b[4..20])
            } else {
                (0.0, &b[2..18])
            };
            let off = if q == Quant::Q4_0 { 8.0 } else { 0.0 };
            y.extend(qs.iter().map(|v| ((v & 0xf) as f64 - off) * d + m));
            y.extend(qs.iter().map(|v| ((v >> 4) as f64 - off) * d + m));
        }
        Quant::Q5_0 | Quant::Q5_1 => {
            let d = h(b, 0);
            let (m, at) = if q == Quant::Q5_1 {
                (h(b, 2), 4)
            } else {
                (0.0, 2)
            };
            let qh = u32::from_le_bytes(le(&b[at..]));
            let qs = &b[at + 4..at + 20];
            let off = if q == Quant::Q5_0 { 16.0 } else { 0.0 };
            let mut hi = [0.0f64; 16];
            for j in 0..16 {
                let xh0 = ((qh >> j) << 4) & 0x10;
                let xh1 = (qh >> (j + 12)) & 0x10;
                y.push((((qs[j] & 0xf) as u32 | xh0) as f64 - off) * d + m);
                hi[j] = (((qs[j] >> 4) as u32 | xh1) as f64 - off) * d + m;
            }
            y.extend_from_slice(&hi);
        }
        Quant::Q8_0 => {
            let d = h(b, 0);
            y.extend(b[2..34].iter().map(|v| *v as i8 as f64 * d));
        }
        Quant::Q8_1 => {
            let d = h(b, 0);
            y.extend(b[4..36].iter().map(|v| *v as i8 as f64 * d));
        }
        Quant::Q2K => {
            let scales = &b[0..16];
            let qs = &b[16..80];
            let d = h(b, 80);
            let min = h(b, 82);
            let mut is = 0;
            for n in 0..2 {
                let q = &qs[n * 32..n * 32 + 32];
                let mut shift = 0;
                for _ in 0..4 {
                    for half in 0..2 {
                        let sc = scales[is];
                        is += 1;
                        let dl = d * (sc & 0xf) as f64;
                        let ml = min * (sc >> 4) as f64;
                        for l in 0..16 {
                            y.push(dl * ((q[l + 16 * half] >> shift) & 3) as f64 - ml);
                        }
                    }
                    shift += 2;
                }
            }
        }
        Quant::Q3K => {
            let hm = &b[0..32];
            let qs = &b[32..96];
            let sc = &b[96..108];
            let d = h(b, 108);
            let a0 = u32::from_le_bytes(le(&sc[0..]));
            let a1 = u32::from_le_bytes(le(&sc[4..]));
            let tmp = u32::from_le_bytes(le(&sc[8..]));
            let (k1, k2) = (0x0303_0303u32, 0x0f0f_0f0fu32);
            let aux = [
                (a0 & k2) | ((tmp & k1) << 4),
                (a1 & k2) | (((tmp >> 2) & k1) << 4),
                ((a0 >> 4) & k2) | (((tmp >> 4) & k1) << 4),
                ((a1 >> 4) & k2) | (((tmp >> 6) & k1) << 4),
            ];
            let mut scales = [0i8; 16];
            for (i, a) in aux.iter().enumerate() {
                for (k, v) in a.to_le_bytes().iter().enumerate() {
                    scales[i * 4 + k] = *v as i8;
                }
            }
            let mut is = 0;
            let mut m = 1u8;
            for n in 0..2 {
                let q = &qs[n * 32..n * 32 + 32];
                let mut shift = 0;
                for _ in 0..4 {
                    for half in 0..2 {
                        let dl = d * (scales[is] as f64 - 32.0);
                        is += 1;
                        for l in 0..16 {
                            let i = l + 16 * half;
                            let sub = if hm[i] & m != 0 { 0.0 } else { 4.0 };
                            y.push(dl * (((q[i] >> shift) & 3) as f64 - sub));
                        }
                    }
                    shift += 2;
                    m = m.wrapping_shl(1);
                }
            }
        }
        Quant::Q4K | Quant::Q5K => {
            let d = h(b, 0);
            let min = h(b, 2);
            let scales = &b[4..16];
            let (qh, qs): (&[u8], &[u8]) = if q == Quant::Q5K {
                (&b[16..48], &b[48..176])
            } else {
                (&[], &b[16..144])
            };
            let mut u1 = 1u8;
            let mut u2 = 2u8;
            for j in 0..4 {
                let (s1, m1) = scale_min_k4(2 * j, scales);
                let (s2, m2) = scale_min_k4(2 * j + 1, scales);
                let ql = &qs[j * 32..j * 32 + 32];
                for l in 0..32 {
                    let hb = if !qh.is_empty() && qh[l] & u1 != 0 {
                        16.0
                    } else {
                        0.0
                    };
                    y.push(d * s1 * ((ql[l] & 0xf) as f64 + hb) - min * m1);
                }
                for l in 0..32 {
                    let hb = if !qh.is_empty() && qh[l] & u2 != 0 {
                        16.0
                    } else {
                        0.0
                    };
                    y.push(d * s2 * ((ql[l] >> 4) as f64 + hb) - min * m2);
                }
                u1 = u1.wrapping_shl(2);
                u2 = u2.wrapping_shl(2);
            }
        }
        Quant::Q6K => {
            let ql_all = &b[0..128];
            let qh_all = &b[128..192];
            let sc_all = &b[192..208];
            let d = h(b, 208);
            for n in 0..2 {
                let ql = &ql_all[n * 64..];
                let qh = &qh_all[n * 32..];
                let sc = &sc_all[n * 8..];
                let mut out = [0.0f64; 128];
                for l in 0..32 {
                    let is = l / 16;
                    let q1 = ((ql[l] & 0xf) | (qh[l] & 3) << 4) as i32 - 32;
                    let q2 = ((ql[l + 32] & 0xf) | ((qh[l] >> 2) & 3) << 4) as i32 - 32;
                    let q3 = ((ql[l] >> 4) | ((qh[l] >> 4) & 3) << 4) as i32 - 32;
                    let q4 = ((ql[l + 32] >> 4) | ((qh[l] >> 6) & 3) << 4) as i32 - 32;
                    out[l] = d * (sc[is] as i8 as f64) * q1 as f64;
                    out[l + 32] = d * (sc[is + 2] as i8 as f64) * q2 as f64;
                    out[l + 64] = d * (sc[is + 4] as i8 as f64) * q3 as f64;
                    out[l + 96] = d * (sc[is + 6] as i8 as f64) * q4 as f64;
                }
                y.extend_from_slice(&out);
            }
        }
        Quant::Q8K => {
            let d = f32::from_le_bytes(le(b)) as f64;
            y.extend(b[4..260].iter().map(|v| *v as i8 as f64 * d));
        }
        Quant::Iq4Nl => {
            let d = h(b, 0);
            let qs = &b[2..18];
            y.extend(qs.iter().map(|v| d * IQ4NL[(v & 0xf) as usize] as f64));
            y.extend(qs.iter().map(|v| d * IQ4NL[(v >> 4) as usize] as f64));
        }
        Quant::Iq4Xs => {
            let d = h(b, 0);
            let sh = u16::from_le_bytes([b[2], b[3]]);
            let sl = &b[4..8];
            let qs_all = &b[8..136];
            for ib in 0..8 {
                let ls = ((sl[ib / 2] >> (4 * (ib % 2))) & 0xf) as i32
                    | ((((sh >> (2 * ib)) & 3) as i32) << 4);
                let dl = d * (ls - 32) as f64;
                let qs = &qs_all[ib * 16..ib * 16 + 16];
                y.extend(qs.iter().map(|v| dl * IQ4NL[(v & 0xf) as usize] as f64));
                y.extend(qs.iter().map(|v| dl * IQ4NL[(v >> 4) as usize] as f64));
            }
        }
        Quant::Mxfp4 => {
            // E8M0 scale; the table holds doubled values, so halve it.
            let d = 2f64.powi(b[0] as i32 - 128);
            let qs = &b[1..17];
            y.extend(qs.iter().map(|v| d * MXFP4[(v & 0xf) as usize] as f64));
            y.extend(qs.iter().map(|v| d * MXFP4[(v >> 4) as usize] as f64));
        }
    }
}

/// A source owned by the stream or borrowed from the caller.
#[derive(Clone)]
enum Src<'a> {
    Borrowed(&'a dyn Source),
    Owned(Rc<Vec<u8>>),
}

impl Src<'_> {
    fn get(&self) -> &dyn Source {
        match self {
            Src::Borrowed(s) => *s,
            Src::Owned(v) => v.as_ref(),
        }
    }
}

/// One side of a comparison: its source, zip members and decompressed
/// member cache, and how many bytes were read so far.
pub(crate) struct Side<'a> {
    src: &'a dyn Source,
    members: Option<Vec<Member>>,
    cache: HashMap<String, Rc<Vec<u8>>>,
    used: u64,
}

impl<'a> Side<'a> {
    pub fn new(src: &'a dyn Source) -> Self {
        Side {
            src,
            members: None,
            cache: HashMap::new(),
            used: 0,
        }
    }

    fn member(&mut self, name: &str) -> Result<&Member, String> {
        if self.members.is_none() {
            self.members = Some(zipfile::list(self.src).map_err(|e| e.to_string())?);
        }
        self.members
            .as_ref()
            .and_then(|m| m.iter().find(|m| m.name == name))
            .ok_or_else(|| format!("{name} is missing from the archive"))
    }

    fn whole_member(&mut self, name: &str) -> Result<Rc<Vec<u8>>, String> {
        if let Some(v) = self.cache.get(name) {
            return Ok(v.clone());
        }
        let src = self.src;
        let m = self.member(name)?;
        let data = zipfile::read_member(src, m, MEMBER_CAP).map_err(|e| e.to_string())?;
        let rc = Rc::new(data);
        self.cache.insert(name.to_string(), rc.clone());
        Ok(rc)
    }
}

/// Bytes a tensor's values take to read.
fn read_cost(info: &TensorInfo) -> u64 {
    let es = info.dtype.size().unwrap_or(1);
    match &info.location {
        TensorLocation::Bytes { len, .. } | TensorLocation::Varints { len, .. } => *len,
        TensorLocation::Strided { storage_len, .. } => *storage_len,
        TensorLocation::Chunked { chunks, .. } => chunks
            .iter()
            .fold(info.elements.saturating_mul(es), |a, c| {
                a.saturating_add(c.size)
            }),
        TensorLocation::Missing | TensorLocation::External { .. } => 0,
    }
}

enum Stream<'a> {
    Range {
        src: Src<'a>,
        pos: u64,
        end: u64,
        dec: Decoder,
    },
    Mem {
        data: Vec<u8>,
        pos: usize,
        dec: Decoder,
    },
    Values(Option<Vec<f64>>),
}

/// Values of one tensor in chunks, cut at its element count.
struct Values<'a> {
    stream: Stream<'a>,
    left: u64,
}

impl Values<'_> {
    fn next(&mut self) -> Result<Option<Vec<f64>>, String> {
        if self.left == 0 {
            return Ok(None);
        }
        let mut out = match &mut self.stream {
            Stream::Range { src, pos, end, dec } => {
                let unit = dec.unit() as u64;
                let step = ((CHUNK as u64 / unit).max(1) * unit).min(*end - *pos);
                let step = step - step % unit;
                if step == 0 {
                    return Ok(None);
                }
                let mut buf = vec![0u8; step as usize];
                src.get()
                    .read_at(*pos, &mut buf)
                    .map_err(|e| e.to_string())?;
                *pos += step;
                let mut out = Vec::with_capacity(buf.len());
                dec.decode(&buf, &mut out);
                out
            }
            Stream::Mem { data, pos, dec } => {
                let unit = dec.unit();
                let left = data.len() - *pos;
                let step = ((CHUNK / unit).max(1) * unit).min(left);
                let step = step - step % unit;
                if step == 0 {
                    return Ok(None);
                }
                let mut out = Vec::with_capacity(step);
                dec.decode(&data[*pos..*pos + step], &mut out);
                *pos += step;
                out
            }
            Stream::Values(v) => match v.take() {
                Some(v) => v,
                None => return Ok(None),
            },
        };
        if out.len() as u64 > self.left {
            out.truncate(self.left as usize);
        }
        self.left -= out.len() as u64;
        Ok(Some(out))
    }
}

fn open<'a>(side: &mut Side<'a>, info: &TensorInfo) -> Result<Values<'a>, String> {
    if info.elements == 0 {
        return Ok(Values {
            stream: Stream::Values(None),
            left: 0,
        });
    }
    let dec = || decoder(info);
    let es = info.dtype.size().unwrap_or(1);
    // The bytes a location's offsets point into.
    let base = |side: &mut Side<'a>| -> Result<Src<'a>, String> {
        match &info.member {
            Some(m) => Ok(Src::Owned(side.whole_member(m)?)),
            None => Ok(Src::Borrowed(side.src)),
        }
    };
    let stream = match &info.location {
        TensorLocation::Missing => return Err("the file holds no values for it".into()),
        TensorLocation::External { file, .. } => {
            return Err(format!("values are in the external file {file}"));
        }
        TensorLocation::Bytes { offset, len } => {
            let dec = dec()?;
            match &info.member {
                Some(name) => {
                    let src = side.src;
                    let m = side.member(name)?;
                    let data =
                        zipfile::read_range(src, m, *offset, *len).map_err(|e| e.to_string())?;
                    Stream::Mem { data, pos: 0, dec }
                }
                None => {
                    let end = offset.checked_add(*len).ok_or("bad range")?;
                    if end > side.src.len() {
                        return Err("values run past the end of the file".into());
                    }
                    Stream::Range {
                        src: Src::Borrowed(side.src),
                        pos: *offset,
                        end,
                        dec,
                    }
                }
            }
        }
        TensorLocation::Strided {
            offset,
            storage_len,
            strides,
        } => {
            let dec = dec()?;
            let src = base(side)?;
            let span = read_bytes(src.get(), *offset, *storage_len)?;
            let data =
                gather(&span, es, &info.shape, strides).ok_or("strides run past the storage")?;
            Stream::Mem { data, pos: 0, dec }
        }
        TensorLocation::Varints { offset, len } => {
            let src = base(side)?;
            let raw = read_bytes(src.get(), *offset, *len)?;
            Stream::Values(Some(varints(&raw, &info.dtype)?))
        }
        TensorLocation::Chunked {
            chunk_shape,
            filters,
            chunks,
        } => {
            let dec = dec()?;
            let src = base(side)?;
            let data = assemble(src.get(), info, es, chunk_shape, filters, chunks)?;
            Stream::Mem { data, pos: 0, dec }
        }
    };
    Ok(Values {
        stream,
        left: info.elements,
    })
}

fn read_bytes(src: &dyn Source, offset: u64, len: u64) -> Result<Vec<u8>, String> {
    if offset.checked_add(len).is_none_or(|e| e > src.len()) {
        return Err("values run past the end of the file".into());
    }
    let mut buf = vec![0u8; usize::try_from(len).map_err(|_| "too large")?];
    src.read_at(offset, &mut buf).map_err(|e| e.to_string())?;
    Ok(buf)
}

/// Copies a strided view out of its storage, densely in row-major order.
fn gather(storage: &[u8], es: u64, shape: &[u64], strides: &[u64]) -> Option<Vec<u8>> {
    if shape.len() != strides.len() {
        return None;
    }
    let es = es as usize;
    let n: u64 = shape.iter().product();
    let mut out = Vec::with_capacity(
        (n as usize)
            .saturating_mul(es)
            .min(storage.len().saturating_mul(4)),
    );
    let mut idx = vec![0u64; shape.len()];
    for _ in 0..n {
        let at = idx.iter().zip(strides).map(|(i, s)| i * s).sum::<u64>() as usize * es;
        out.extend_from_slice(storage.get(at..at + es)?);
        for k in (0..idx.len()).rev() {
            idx[k] += 1;
            if idx[k] < shape[k] {
                break;
            }
            idx[k] = 0;
        }
    }
    Some(out)
}

fn varints(raw: &[u8], dtype: &DType) -> Result<Vec<f64>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let mut v = 0u64;
        let mut shift = 0;
        loop {
            let b = *raw.get(i).ok_or("truncated varint")?;
            i += 1;
            if shift < 64 {
                v |= ((b & 0x7f) as u64) << shift;
            }
            shift += 7;
            if b & 0x80 == 0 {
                break;
            }
        }
        out.push(match dtype {
            DType::F16 => half::f16::from_bits(v as u16).to_f64(),
            DType::BF16 => half::bf16::from_bits(v as u16).to_f64(),
            DType::I8 | DType::I16 | DType::I32 | DType::I64 => v as i64 as f64,
            DType::Bool => (v != 0) as u8 as f64,
            _ => v as f64,
        });
    }
    Ok(out)
}

/// Reads and unfilters every HDF5 chunk into one dense row-major array.
fn assemble(
    src: &dyn Source,
    info: &TensorInfo,
    es: u64,
    chunk_shape: &[u64],
    filters: &[u32],
    chunks: &[corvene_models::tensors::TensorChunk],
) -> Result<Vec<u8>, String> {
    let rank = info.shape.len();
    if chunk_shape.len() != rank || rank == 0 || chunk_shape.contains(&0) {
        return Err("bad chunk layout".into());
    }
    let total = info.elements.checked_mul(es).ok_or("too large")?;
    let mut out = vec![0u8; usize::try_from(total).map_err(|_| "too large")?];
    let chunk_elems: u64 = chunk_shape.iter().product();
    let chunk_bytes = chunk_elems.checked_mul(es).ok_or("too large")?;
    for c in chunks {
        if c.coords.len() < rank {
            return Err("bad chunk coordinates".into());
        }
        let mut data = read_bytes(src, c.offset, c.size)?;
        for (i, f) in filters.iter().enumerate().rev() {
            if c.filter_mask & (1 << i) != 0 {
                continue;
            }
            data = match f {
                1 => {
                    let mut d = Vec::new();
                    flate2::read::ZlibDecoder::new(&data[..])
                        .take(chunk_bytes)
                        .read_to_end(&mut d)
                        .map_err(|e| format!("deflate: {e}"))?;
                    d
                }
                2 => unshuffle(&data, es as usize),
                3 => {
                    let keep = data.len().saturating_sub(4);
                    data.truncate(keep);
                    data
                }
                other => return Err(format!("values not decoded for HDF5 filter {other}")),
            };
        }
        if (data.len() as u64) < chunk_bytes {
            return Err("chunk shorter than its shape".into());
        }
        // Copy rows along the last dimension.
        let last = rank - 1;
        let row = chunk_shape[last];
        let start_last = c.coords[last];
        if start_last >= info.shape[last] {
            continue;
        }
        let run = row.min(info.shape[last] - start_last);
        let rows: u64 = chunk_shape[..last].iter().product();
        let mut idx = vec![0u64; last];
        for r in 0..rows {
            let mut inside = true;
            let mut dst = 0u64;
            for (k, i) in idx.iter().enumerate() {
                let g = c.coords[k] + i;
                if g >= info.shape[k] {
                    inside = false;
                    break;
                }
                dst = dst * info.shape[k] + g;
            }
            if inside {
                let dst = (dst * info.shape[last] + start_last) * es;
                let srcp = r * row * es;
                let n = run * es;
                let (d, s) = (dst as usize, srcp as usize);
                if let (Some(to), Some(from)) =
                    (out.get_mut(d..d + n as usize), data.get(s..s + n as usize))
                {
                    to.copy_from_slice(from);
                }
            }
            for k in (0..last).rev() {
                idx[k] += 1;
                if idx[k] < chunk_shape[k] {
                    break;
                }
                idx[k] = 0;
            }
        }
    }
    Ok(out)
}

fn unshuffle(data: &[u8], es: usize) -> Vec<u8> {
    if es <= 1 {
        return data.to_vec();
    }
    let n = data.len() / es;
    let mut out = data.to_vec();
    for b in 0..es {
        for i in 0..n {
            out[i * es + b] = data[b * n + i];
        }
    }
    out
}

/// Running statistics of one side.
struct StatsAcc {
    count: u64,
    finite: u64,
    min: f64,
    max: f64,
    mean: f64,
    m2: f64,
    zeros: u64,
    nan: u64,
    inf: u64,
    step: u64,
    index: u64,
    sample: Vec<f64>,
}

impl StatsAcc {
    fn new(elements: u64) -> Self {
        StatsAcc {
            count: 0,
            finite: 0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
            mean: 0.0,
            m2: 0.0,
            zeros: 0,
            nan: 0,
            inf: 0,
            step: elements.div_ceil(HIST_SAMPLES).max(1),
            index: 0,
            sample: Vec::new(),
        }
    }

    fn push(&mut self, v: &[f64]) {
        for x in v {
            self.count += 1;
            if x.is_nan() {
                self.nan += 1;
            } else if x.is_infinite() {
                self.inf += 1;
            } else {
                if *x == 0.0 {
                    self.zeros += 1;
                }
                self.finite += 1;
                self.min = self.min.min(*x);
                self.max = self.max.max(*x);
                let d = x - self.mean;
                self.mean += d / self.finite as f64;
                self.m2 += d * (x - self.mean);
                if self.index.is_multiple_of(self.step) {
                    self.sample.push(*x);
                }
            }
            self.index += 1;
        }
    }

    fn finish(self) -> TensorStats {
        let mut histogram = vec![0u64; BUCKETS];
        let span = self.max - self.min;
        for x in &self.sample {
            let b = if span > 0.0 {
                (((x - self.min) / span) * BUCKETS as f64) as usize
            } else {
                0
            };
            histogram[b.min(BUCKETS - 1)] += 1;
        }
        let (min, max) = if self.finite == 0 {
            (0.0, 0.0)
        } else {
            (self.min, self.max)
        };
        TensorStats {
            count: self.count,
            min,
            max,
            mean: self.mean,
            std: if self.finite > 0 {
                (self.m2 / self.finite as f64).sqrt()
            } else {
                0.0
            },
            zeros: self.zeros,
            nan: self.nan,
            inf: self.inf,
            histogram,
        }
    }
}

/// Running element-wise comparison.
#[derive(Default)]
struct DiffAcc {
    n: u64,
    counted: u64,
    max_abs: f64,
    sum_abs: f64,
    sum_sq: f64,
    changed: u64,
    dot: f64,
    no: f64,
    nn: f64,
    samples: Vec<DiffSample>,
}

impl DiffAcc {
    fn push(&mut self, a: &[f64], b: &[f64]) {
        for (x, y) in a.iter().zip(b) {
            let i = self.n;
            self.n += 1;
            if x.to_bits() == y.to_bits() {
                if x.is_finite() {
                    self.counted += 1;
                    self.dot += x * y;
                    self.no += x * x;
                    self.nn += y * y;
                }
                continue;
            }
            self.changed += 1;
            let d = (x - y).abs();
            if d.is_finite() {
                self.counted += 1;
                self.max_abs = self.max_abs.max(d);
                self.sum_abs += d;
                self.sum_sq += d * d;
                self.dot += x * y;
                self.no += x * x;
                self.nn += y * y;
            }
            let key = if d.is_nan() { f64::INFINITY } else { d };
            let worst = self.samples.last().map(|s| (s.old - s.new).abs());
            if self.samples.len() < 8 || worst.is_some_and(|w| key > w || w.is_nan()) {
                self.samples.push(DiffSample {
                    index: i,
                    old: *x,
                    new: *y,
                });
                self.samples.sort_by(|p, q| {
                    let kp = (p.old - p.new).abs();
                    let kq = (q.old - q.new).abs();
                    let kp = if kp.is_nan() { f64::INFINITY } else { kp };
                    let kq = if kq.is_nan() { f64::INFINITY } else { kq };
                    kq.total_cmp(&kp)
                });
                self.samples.truncate(8);
            }
        }
    }

    fn finish(self) -> ValueDiff {
        let c = self.counted.max(1) as f64;
        ValueDiff {
            max_abs_diff: self.max_abs,
            mean_abs_diff: self.sum_abs / c,
            rms_diff: (self.sum_sq / c).sqrt(),
            changed: self.changed,
            cosine: (self.no > 0.0 && self.nn > 0.0)
                .then(|| self.dot / (self.no.sqrt() * self.nn.sqrt())),
            identical: self.changed == 0,
            samples: self.samples,
        }
    }
}

/// Opens one side's tensor, charging its read cost against the budget.
fn open_side<'a>(
    side: &mut Side<'a>,
    info: &TensorInfo,
    max_bytes: u64,
) -> Result<Values<'a>, String> {
    let cost = read_cost(info);
    if side.used.saturating_add(cost) > max_bytes {
        return Err(format!(
            "skipped: {} bytes would exceed the read budget",
            cost
        ));
    }
    side.used += cost;
    open(side, info)
}

/// See `crate::compare_values`.
pub(crate) fn compare(
    old: Option<(&TensorFile, &dyn Source)>,
    new: Option<(&TensorFile, &dyn Source)>,
    names: &[String],
    max_bytes: u64,
    cancel: &AtomicBool,
) -> Vec<TensorValues> {
    let mut old_side = old.map(|(_, s)| Side::new(s));
    let mut new_side = new.map(|(_, s)| Side::new(s));
    // by name, first wins (as a linear search would)
    fn index(f: Option<&TensorFile>) -> std::collections::HashMap<&str, &TensorInfo> {
        let mut map = std::collections::HashMap::new();
        for t in f.map_or(&[][..], |f| &f.tensors[..]) {
            map.entry(t.name.as_str()).or_insert(t);
        }
        map
    }
    let (old_index, new_index) = (index(old.map(|o| o.0)), index(new.map(|n| n.0)));
    let mut list: Vec<String> = names.to_vec();
    if list.is_empty() {
        let mut seen = std::collections::HashSet::new();
        for f in [new, old].into_iter().flatten() {
            for t in &f.0.tensors {
                if seen.insert(t.name.clone()) {
                    list.push(t.name.clone());
                }
            }
        }
    }

    let mut out = Vec::new();
    for name in list {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let o = old_index.get(name.as_str()).map(|t| (*t).clone());
        let n = new_index.get(name.as_str()).map(|t| (*t).clone());
        let mut notes: Vec<String> = Vec::new();
        if o.is_none() && n.is_none() {
            notes.push("no tensor of that name".into());
        }
        let mut so = None;
        let mut sn = None;
        if let (Some(t), Some(side)) = (&o, old_side.as_mut()) {
            match open_side(side, t, max_bytes) {
                Ok(v) => so = Some((v, StatsAcc::new(t.elements))),
                Err(e) => notes.push(format!("old: {e}")),
            }
        }
        if let (Some(t), Some(side)) = (&n, new_side.as_mut()) {
            match open_side(side, t, max_bytes) {
                Ok(v) => sn = Some((v, StatsAcc::new(t.elements))),
                Err(e) => notes.push(format!("new: {e}")),
            }
        }
        let mut diff = match (&o, &n) {
            (Some(a), Some(b)) if a.elements == b.elements => {
                if a.shape != b.shape {
                    notes.push("shapes differ; compared element by element".into());
                }
                if a.column_major != b.column_major {
                    notes.push("element orders differ (row- and column-major)".into());
                }
                (so.is_some() && sn.is_some()).then(DiffAcc::default)
            }
            (Some(_), Some(_)) => {
                notes.push("element counts differ".into());
                None
            }
            _ => None,
        };

        let mut bo: Vec<f64> = Vec::new();
        let mut bn: Vec<f64> = Vec::new();
        let mut done_o = so.is_none();
        let mut done_n = sn.is_none();
        let mut cancelled = false;
        let mut failed = false;
        while !(done_o && done_n) {
            if cancel.load(Ordering::Relaxed) {
                cancelled = true;
                break;
            }
            // Pull from the side that is behind (or the only one).
            let pull_old = !done_o && (done_n || bo.len() <= bn.len());
            let (stream, buf, done, label) = if pull_old {
                (so.as_mut(), &mut bo, &mut done_o, "old")
            } else {
                (sn.as_mut(), &mut bn, &mut done_n, "new")
            };
            let Some((values, stats)) = stream else {
                *done = true;
                continue;
            };
            match values.next() {
                Ok(Some(chunk)) => {
                    stats.push(&chunk);
                    if diff.is_some() {
                        buf.extend_from_slice(&chunk);
                    }
                }
                Ok(None) => *done = true,
                Err(e) => {
                    notes.push(format!("{label}: {e}"));
                    failed = true;
                    *done = true;
                }
            }
            if let Some(d) = diff.as_mut() {
                let k = bo.len().min(bn.len());
                if k > 0 {
                    d.push(&bo[..k], &bn[..k]);
                    bo.drain(..k);
                    bn.drain(..k);
                }
            }
        }
        if cancelled {
            break;
        }
        if failed {
            diff = None;
        }
        out.push(TensorValues {
            name,
            old: if failed && notes.iter().any(|n| n.starts_with("old:")) {
                None
            } else {
                so.map(|s| s.1.finish())
            },
            new: if failed && notes.iter().any(|n| n.starts_with("new:")) {
                None
            } else {
                sn.map(|s| s.1.finish())
            },
            diff: diff.map(DiffAcc::finish),
            note: (!notes.is_empty()).then(|| notes.join("; ")),
        });
    }
    out
}
