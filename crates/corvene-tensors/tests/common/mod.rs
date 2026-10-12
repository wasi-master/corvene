//! Byte builders for test fixtures of every format.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::io::Write;

// ---------------------------------------------------------------- zip

/// A zip archive of `(name, data, deflate)` members.
pub fn zip(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data, deflate) in entries {
        let mut crc = flate2::Crc::new();
        crc.update(data);
        let crc = crc.sum();
        let body = if *deflate {
            let mut e =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            e.write_all(data).unwrap();
            e.finish().unwrap()
        } else {
            data.to_vec()
        };
        let method: u16 = if *deflate { 8 } else { 0 };
        let offset = out.len() as u32;
        out.extend_from_slice(&0x04034b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&method.to_le_bytes());
        out.extend_from_slice(&[0, 0, 0x21, 0]);
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&body);

        central.extend_from_slice(&0x02014b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&method.to_le_bytes());
        central.extend_from_slice(&[0, 0, 0x21, 0]);
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(body.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0; 8]);
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let cd_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x06054b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

pub fn f32s(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

pub fn f16s(v: &[f32]) -> Vec<u8> {
    v.iter()
        .flat_map(|x| half::f16::from_f32(*x).to_le_bytes())
        .collect()
}

pub fn bf16s(v: &[f32]) -> Vec<u8> {
    v.iter()
        .flat_map(|x| half::bf16::from_f32(*x).to_le_bytes())
        .collect()
}

// --------------------------------------------------------- safetensors

pub fn safetensors(entries: &[(&str, &str, &[u64], Vec<u8>)], meta: &[(&str, &str)]) -> Vec<u8> {
    let mut header = serde_json::Map::new();
    let mut data = Vec::new();
    for (name, dtype, shape, bytes) in entries {
        let start = data.len();
        data.extend_from_slice(bytes);
        header.insert(
            name.to_string(),
            serde_json::json!({"dtype": dtype, "shape": shape, "data_offsets": [start, data.len()]}),
        );
    }
    if !meta.is_empty() {
        let m: serde_json::Map<String, serde_json::Value> = meta
            .iter()
            .map(|(k, v)| (k.to_string(), serde_json::Value::String(v.to_string())))
            .collect();
        header.insert("__metadata__".into(), serde_json::Value::Object(m));
    }
    let mut h = serde_json::to_vec(&header).unwrap();
    while !h.len().is_multiple_of(8) {
        h.push(b' ');
    }
    let mut out = (h.len() as u64).to_le_bytes().to_vec();
    out.extend_from_slice(&h);
    out.extend_from_slice(&data);
    out
}

// ---------------------------------------------------------------- GGUF

pub fn gguf_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u64).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

/// A Q8_0 block: scale `d`, then 32 quants.
pub fn q8_0(d: f32, q: &[i8; 32]) -> Vec<u8> {
    let mut b = half::f16::from_f32(d).to_le_bytes().to_vec();
    b.extend(q.iter().map(|v| *v as u8));
    b
}

/// A Q4_0 block: scale `d`, then 16 bytes of nibble pairs.
pub fn q4_0(d: f32, qs: &[u8; 16]) -> Vec<u8> {
    let mut b = half::f16::from_f32(d).to_le_bytes().to_vec();
    b.extend_from_slice(qs);
    b
}

/// A GGUF v3 file with a few metadata kinds and `tensors` of
/// `(name, dims, ggml type, data)`.
pub fn gguf(tensors: &[(&str, &[u64], u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = b"GGUF".to_vec();
    out.extend_from_slice(&3u32.to_le_bytes());
    out.extend_from_slice(&(tensors.len() as u64).to_le_bytes());
    out.extend_from_slice(&6u64.to_le_bytes());
    gguf_str(&mut out, "general.architecture");
    out.extend_from_slice(&8u32.to_le_bytes());
    gguf_str(&mut out, "llama");
    gguf_str(&mut out, "general.alignment");
    out.extend_from_slice(&4u32.to_le_bytes());
    out.extend_from_slice(&32u32.to_le_bytes());
    gguf_str(&mut out, "llama.rope.freq_base");
    out.extend_from_slice(&6u32.to_le_bytes());
    out.extend_from_slice(&10000.5f32.to_le_bytes());
    gguf_str(&mut out, "tokenizer.ggml.tokens");
    out.extend_from_slice(&9u32.to_le_bytes());
    out.extend_from_slice(&8u32.to_le_bytes());
    out.extend_from_slice(&10u64.to_le_bytes());
    for i in 0..10 {
        gguf_str(&mut out, &format!("tok{i}"));
    }
    gguf_str(&mut out, "tokenizer.ggml.scores");
    out.extend_from_slice(&9u32.to_le_bytes());
    out.extend_from_slice(&6u32.to_le_bytes());
    out.extend_from_slice(&3u64.to_le_bytes());
    for v in [0.5f32, 1.0, 1.5] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    gguf_str(&mut out, "general.flag");
    out.extend_from_slice(&7u32.to_le_bytes());
    out.push(1);

    let mut offset = 0u64;
    let mut offsets = Vec::new();
    for (name, dims, ty, data) in tensors {
        gguf_str(&mut out, name);
        out.extend_from_slice(&(dims.len() as u32).to_le_bytes());
        for d in *dims {
            out.extend_from_slice(&d.to_le_bytes());
        }
        out.extend_from_slice(&ty.to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offsets.push(offset);
        offset += (data.len() as u64).div_ceil(32) * 32;
    }
    while !out.len().is_multiple_of(32) {
        out.push(0);
    }
    let start = out.len();
    for ((_, _, _, data), off) in tensors.iter().zip(offsets) {
        out.resize(start + off as usize, 0);
        out.extend_from_slice(data);
    }
    out
}

// --------------------------------------------------------------- NumPy

pub fn npy(descr: &str, fortran: bool, shape: &[u64], data: &[u8]) -> Vec<u8> {
    let dims: Vec<String> = shape.iter().map(|d| d.to_string()).collect();
    let shape = if dims.len() == 1 {
        format!("({},)", dims[0])
    } else {
        format!("({})", dims.join(", "))
    };
    let mut h = format!(
        "{{'descr': '{descr}', 'fortran_order': {}, 'shape': {shape}, }}",
        if fortran { "True" } else { "False" }
    );
    while (10 + h.len() + 1) % 64 != 0 {
        h.push(' ');
    }
    h.push('\n');
    let mut out = b"\x93NUMPY\x01\x00".to_vec();
    out.extend_from_slice(&(h.len() as u16).to_le_bytes());
    out.extend_from_slice(h.as_bytes());
    out.extend_from_slice(data);
    out
}

// -------------------------------------------------------------- pickle

pub struct Pickle(pub Vec<u8>);

impl Pickle {
    pub fn new() -> Self {
        Pickle(vec![0x80, 2])
    }
    pub fn op(&mut self, b: &[u8]) -> &mut Self {
        self.0.extend_from_slice(b);
        self
    }
    pub fn global(&mut self, module: &str, name: &str) -> &mut Self {
        self.0.push(b'c');
        self.0.extend_from_slice(module.as_bytes());
        self.0.push(b'\n');
        self.0.extend_from_slice(name.as_bytes());
        self.0.push(b'\n');
        self
    }
    pub fn str(&mut self, s: &str) -> &mut Self {
        self.0.push(b'X');
        self.0.extend_from_slice(&(s.len() as u32).to_le_bytes());
        self.0.extend_from_slice(s.as_bytes());
        self
    }
    pub fn int(&mut self, v: u8) -> &mut Self {
        self.0.extend_from_slice(&[b'K', v]);
        self
    }
    pub fn put(&mut self, k: u8) -> &mut Self {
        self.0.extend_from_slice(&[b'q', k]);
        self
    }
    pub fn get(&mut self, k: u8) -> &mut Self {
        self.0.extend_from_slice(&[b'h', k]);
        self
    }
    /// Pushes `torch._utils._rebuild_tensor_v2(storage, offset, size,
    /// stride, False, OrderedDict())` the way `torch.save` writes it.
    pub fn tensor(
        &mut self,
        storage: &str,
        key: &str,
        numel: u8,
        offset: u8,
        size: &[u8],
        stride: &[u8],
    ) -> &mut Self {
        self.global("torch._utils", "_rebuild_tensor_v2");
        self.op(b"(");
        self.op(b"(")
            .str("storage")
            .global("torch", storage)
            .str(key)
            .str("cpu")
            .int(numel)
            .op(b"t");
        self.op(b"Q");
        self.int(offset);
        let tuple = |p: &mut Self, v: &[u8]| match v.len() {
            1 => {
                p.int(v[0]).op(&[0x85]);
            }
            2 => {
                p.int(v[0]).int(v[1]).op(&[0x86]);
            }
            _ => {
                p.op(b"(");
                for x in v {
                    p.int(*x);
                }
                p.op(b"t");
            }
        };
        tuple(self, size);
        tuple(self, stride);
        self.op(&[0x89]);
        self.global("collections", "OrderedDict").op(b")R");
        self.op(b"tR");
        self
    }
}

/// `torch.save({"fc.weight": f32[2, 3], "fc.bias": f16[3], "fc.weight_t": a
/// transposed view of fc.weight})`, as an OrderedDict state dict.
pub fn torch_state_dict_pickle() -> Vec<u8> {
    let mut p = Pickle::new();
    p.global("collections", "OrderedDict")
        .put(0)
        .op(b")R")
        .put(1)
        .op(b"(");
    p.str("fc.weight")
        .tensor("FloatStorage", "0", 6, 0, &[2, 3], &[3, 1]);
    p.str("fc.bias")
        .tensor("HalfStorage", "1", 3, 0, &[3], &[1]);
    p.str("fc.weight_t")
        .tensor("FloatStorage", "0", 6, 0, &[3, 2], &[1, 3]);
    p.op(b"u");
    // state: {'_metadata': OrderedDict([('', {'version': 1})])}
    p.op(b"}")
        .str("_metadata")
        .get(0)
        .op(b")R")
        .op(b"(")
        .str("")
        .op(b"}")
        .str("version")
        .int(1)
        .op(b"s")
        .op(b"u");
    p.op(b"s").op(b"b").op(b".");
    p.0
}

pub fn torch_zip(pickle: &[u8], storages: &[(&str, Vec<u8>)], deflate: bool) -> Vec<u8> {
    let mut entries: Vec<(String, Vec<u8>, bool)> = vec![
        ("archive/data.pkl".into(), pickle.to_vec(), deflate),
        ("archive/byteorder".into(), b"little".to_vec(), false),
    ];
    for (k, v) in storages {
        entries.push((format!("archive/data/{k}"), v.clone(), deflate));
    }
    entries.push(("archive/version".into(), b"3\n".to_vec(), false));
    let refs: Vec<(&str, &[u8], bool)> = entries
        .iter()
        .map(|(n, d, z)| (n.as_str(), d.as_slice(), *z))
        .collect();
    zip(&refs)
}

// ------------------------------------------------------------ protobuf

pub fn varint(mut v: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return out;
        }
        out.push(b | 0x80);
    }
}

pub fn pv(no: u32, v: u64) -> Vec<u8> {
    let mut out = varint((no as u64) << 3);
    out.extend(varint(v));
    out
}

pub fn pb(no: u32, data: &[u8]) -> Vec<u8> {
    let mut out = varint(((no as u64) << 3) | 2);
    out.extend(varint(data.len() as u64));
    out.extend_from_slice(data);
    out
}

pub fn ps(no: u32, s: &str) -> Vec<u8> {
    pb(no, s.as_bytes())
}

pub fn cat(parts: &[Vec<u8>]) -> Vec<u8> {
    parts.concat()
}

/// ONNX model: one f32 initializer, one int64 initializer, Conv and two Relu.
pub fn onnx_model() -> Vec<u8> {
    let w = cat(&[
        pv(1, 2),
        pv(1, 3),
        pv(2, 1),
        ps(8, "conv.weight"),
        pb(9, &f32s(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0])),
    ]);
    let packed: Vec<u8> = [1u64, (-2i64) as u64, 3]
        .iter()
        .flat_map(|v| varint(*v))
        .collect();
    let shape_i = cat(&[pv(1, 3), pv(2, 7), ps(8, "shape"), pb(7, &packed)]);
    let ext = cat(&[
        pv(1, 4),
        pv(2, 1),
        ps(8, "big"),
        pb(13, &cat(&[ps(1, "location"), ps(2, "weights.bin")])),
        pb(13, &cat(&[ps(1, "offset"), ps(2, "0")])),
        pv(14, 1),
    ]);
    let dim = |d: Vec<u8>| pb(1, &d);
    let ty = pb(
        1,
        &cat(&[
            pv(1, 1),
            pb(
                2,
                &cat(&[dim(ps(2, "batch")), dim(pv(1, 3)), dim(pv(1, 224))]),
            ),
        ]),
    );
    let input = cat(&[ps(1, "x"), pb(2, &ty)]);
    let output = cat(&[ps(1, "y"), pb(2, &pb(1, &cat(&[pv(1, 1)])))]);
    let graph = cat(&[
        pb(
            1,
            &cat(&[ps(1, "x"), ps(1, "conv.weight"), ps(2, "c"), ps(4, "Conv")]),
        ),
        pb(1, &cat(&[ps(1, "c"), ps(2, "r"), ps(4, "Relu")])),
        pb(1, &cat(&[ps(1, "r"), ps(2, "y"), ps(4, "Relu")])),
        ps(2, "main"),
        pb(5, &w),
        pb(5, &shape_i),
        pb(5, &ext),
        pb(11, &input),
        pb(11, &cat(&[ps(1, "conv.weight"), pb(2, &ty)])),
        pb(12, &output),
    ]);
    cat(&[
        pv(1, 8),
        ps(2, "pytorch"),
        ps(3, "2.1"),
        pb(7, &graph),
        pb(8, &cat(&[ps(1, ""), pv(2, 17)])),
        pb(14, &cat(&[ps(1, "author"), ps(2, "me")])),
    ])
}

// --------------------------------------------------------- flatbuffers

/// A flatbuffer node.
pub enum Fb {
    /// Fields by slot index.
    Table(Vec<(u16, Fld)>),
    Tables(Vec<Fb>),
    /// A vector of raw little-endian scalars: element count and bytes.
    Scalars(u32, Vec<u8>),
    Str(String),
}

pub enum Fld {
    Scalar(Vec<u8>),
    Ref(Fb),
}

fn align(buf: &mut Vec<u8>, n: usize) {
    while !buf.len().is_multiple_of(n) {
        buf.push(0);
    }
}

/// Serializes `root` with file identifier `ident`; children always follow
/// their parents so offsets stay forward.
pub fn flatbuffer(root: Fb, ident: &[u8; 4]) -> Vec<u8> {
    let mut buf = vec![0u8; 4];
    buf.extend_from_slice(ident);
    let mut queue: VecDeque<(usize, Fb)> = VecDeque::new();
    queue.push_back((0, root));
    while let Some((patch, node)) = queue.pop_front() {
        let pos = emit(&mut buf, node, &mut queue);
        let off = (pos - patch) as u32;
        buf[patch..patch + 4].copy_from_slice(&off.to_le_bytes());
    }
    buf
}

fn emit(buf: &mut Vec<u8>, node: Fb, queue: &mut VecDeque<(usize, Fb)>) -> usize {
    match node {
        Fb::Table(fields) => {
            let slots = fields.iter().map(|f| f.0 + 1).max().unwrap_or(0) as usize;
            // Field layout inside the table (after the 4-byte soffset).
            let mut layout = Vec::new();
            let mut size = 4usize;
            for (slot, f) in &fields {
                let w = match f {
                    Fld::Scalar(b) => b.len(),
                    Fld::Ref(_) => 4,
                };
                while !size.is_multiple_of(w.max(1)) {
                    size += 1;
                }
                layout.push((*slot, size));
                size += w;
            }
            align(buf, 2);
            let vt = buf.len();
            buf.extend_from_slice(&((4 + 2 * slots) as u16).to_le_bytes());
            buf.extend_from_slice(&(size as u16).to_le_bytes());
            let mut offs = vec![0u16; slots];
            for (slot, at) in &layout {
                offs[*slot as usize] = *at as u16;
            }
            for o in offs {
                buf.extend_from_slice(&o.to_le_bytes());
            }
            align(buf, 8);
            let t = buf.len();
            buf.extend_from_slice(&((t - vt) as i32).to_le_bytes());
            buf.resize(t + size, 0);
            for ((_, f), (_, at)) in fields.into_iter().zip(layout) {
                match f {
                    Fld::Scalar(b) => buf[t + at..t + at + b.len()].copy_from_slice(&b),
                    Fld::Ref(child) => queue.push_back((t + at, child)),
                }
            }
            t
        }
        Fb::Tables(items) => {
            align(buf, 4);
            let v = buf.len();
            buf.extend_from_slice(&(items.len() as u32).to_le_bytes());
            for item in items {
                let at = buf.len();
                buf.extend_from_slice(&[0; 4]);
                queue.push_back((at, item));
            }
            v
        }
        Fb::Scalars(n, bytes) => {
            align(buf, 8);
            buf.extend_from_slice(&[0; 4]);
            let v = buf.len();
            buf.extend_from_slice(&n.to_le_bytes());
            buf.extend_from_slice(&bytes);
            v
        }
        Fb::Str(s) => {
            align(buf, 4);
            let v = buf.len();
            buf.extend_from_slice(&(s.len() as u32).to_le_bytes());
            buf.extend_from_slice(s.as_bytes());
            buf.push(0);
            v
        }
    }
}

pub fn i32v(v: &[i32]) -> Fb {
    Fb::Scalars(
        v.len() as u32,
        v.iter().flat_map(|x| x.to_le_bytes()).collect(),
    )
}

/// TFLite model: an input, f32 [2, 2] weights (quantized params) and i8
/// [3] bias, two FULLY_CONNECTED ops, one metadata entry.
pub fn tflite_model() -> Vec<u8> {
    let s = |v: &str| Fld::Ref(Fb::Str(v.into()));
    let tensor = |name: &str, shape: &[i32], ty: u8, buffer: u32, quant: Option<Fb>| {
        let mut f = vec![
            (0, Fld::Ref(i32v(shape))),
            (1, Fld::Scalar(vec![ty])),
            (2, Fld::Scalar(buffer.to_le_bytes().to_vec())),
            (3, s(name)),
        ];
        if let Some(q) = quant {
            f.push((4, Fld::Ref(q)));
        }
        Fb::Table(f)
    };
    let quant = Fb::Table(vec![
        (2, Fld::Ref(Fb::Scalars(1, 0.5f32.to_le_bytes().to_vec()))),
        (3, Fld::Ref(Fb::Scalars(1, 0i64.to_le_bytes().to_vec()))),
    ]);
    let subgraph = Fb::Table(vec![
        (
            0,
            Fld::Ref(Fb::Tables(vec![
                tensor("input", &[1, 4], 0, 1, None),
                tensor("weights", &[2, 2], 0, 2, Some(quant)),
                tensor("bias", &[3], 9, 3, None),
            ])),
        ),
        (1, Fld::Ref(i32v(&[0]))),
        (2, Fld::Ref(i32v(&[2]))),
        (
            3,
            Fld::Ref(Fb::Tables(vec![
                Fb::Table(vec![(0, Fld::Scalar(1u32.to_le_bytes().to_vec()))]),
                Fb::Table(vec![(0, Fld::Scalar(1u32.to_le_bytes().to_vec()))]),
            ])),
        ),
        (4, s("main")),
    ]);
    let buffer = |data: Option<Vec<u8>>| match data {
        Some(d) => Fb::Table(vec![(0, Fld::Ref(Fb::Scalars(d.len() as u32, d)))]),
        None => Fb::Table(vec![]),
    };
    let model = Fb::Table(vec![
        (0, Fld::Scalar(3u32.to_le_bytes().to_vec())),
        (
            1,
            Fld::Ref(Fb::Tables(vec![
                Fb::Table(vec![
                    (0, Fld::Scalar(vec![3])),
                    (3, Fld::Scalar(3i32.to_le_bytes().to_vec())),
                ]),
                Fb::Table(vec![
                    (0, Fld::Scalar(vec![9])),
                    (3, Fld::Scalar(9i32.to_le_bytes().to_vec())),
                ]),
            ])),
        ),
        (2, Fld::Ref(Fb::Tables(vec![subgraph]))),
        (3, s("converted")),
        (
            4,
            Fld::Ref(Fb::Tables(vec![
                buffer(None),
                buffer(None),
                buffer(Some(f32s(&[1.0, 2.0, 3.0, 4.0]))),
                buffer(Some(vec![1, 0xff, 3])),
                buffer(Some(b"1.5.0".to_vec())),
            ])),
        ),
        (
            6,
            Fld::Ref(Fb::Tables(vec![Fb::Table(vec![
                (0, s("min_runtime_version")),
                (1, Fld::Scalar(4u32.to_le_bytes().to_vec())),
            ])])),
        ),
    ]);
    flatbuffer(model, b"TFL3")
}

/// Core ML model: one inner product layer with f32 weights and f16 bias.
pub fn coreml_model() -> Vec<u8> {
    let fc = cat(&[
        pv(1, 2),
        pv(2, 2),
        pv(10, 1),
        pb(20, &pb(1, &f32s(&[1.0, 2.0, 3.0, 4.0]))),
        pb(21, &pb(2, &f16s(&[0.5, -0.5]))),
    ]);
    let layer = cat(&[ps(1, "fc"), ps(2, "x"), ps(3, "y"), pb(140, &fc)]);
    let desc = cat(&[
        pb(1, &cat(&[ps(1, "x"), pb(3, &pb(5, &[]))])),
        pb(10, &cat(&[ps(1, "y"), pb(3, &pb(5, &[]))])),
        pb(
            100,
            &cat(&[ps(3, "me"), pb(100, &cat(&[ps(1, "k"), ps(2, "v")]))]),
        ),
    ]);
    cat(&[pv(1, 4), pb(2, &desc), pb(500, &pb(1, &layer))])
}

/// TensorFlow GraphDef: a [2, 2] f32 Const, a fill Const, a MatMul.
pub fn tf_graph() -> Vec<u8> {
    let shape = |dims: &[u64]| {
        pb(
            2,
            &cat(&dims.iter().map(|d| pb(2, &pv(1, *d))).collect::<Vec<_>>()),
        )
    };
    let tensor = cat(&[
        pv(1, 1),
        shape(&[2, 2]),
        pb(4, &f32s(&[1.0, 2.0, 3.0, 4.0])),
    ]);
    let fill = cat(&[pv(1, 1), shape(&[3]), pb(5, &f32s(&[0.0]))]);
    let attr = |t: &[u8]| pb(5, &cat(&[ps(1, "value"), pb(2, &pb(8, t))]));
    cat(&[
        pb(1, &cat(&[ps(1, "w"), ps(2, "Const"), attr(&tensor)])),
        pb(1, &cat(&[ps(1, "zeros"), ps(2, "Const"), attr(&fill)])),
        pb(1, &cat(&[ps(1, "x"), ps(2, "Placeholder")])),
        pb(1, &cat(&[ps(1, "y"), ps(2, "MatMul")])),
        pb(4, &pv(1, 1000)),
    ])
}

pub fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}
