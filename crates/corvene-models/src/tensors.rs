//! Corvene `1340-tensor-diff`: model weight files (safetensors, GGUF,
//! ONNX, TFLite, HDF5 / Keras, PyTorch, NumPy, ...) read as their tensor
//! lists and metadata; values compared on demand.
//!
//! GitHub Desktop shows them as binary files (`app/src/ui/diff/binary-file.tsx`).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// The container format of a weight file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TensorFormat {
    /// Hugging Face safetensors.
    #[default]
    Safetensors,
    /// llama.cpp GGUF.
    Gguf,
    /// A NumPy `.npy` array.
    Npy,
    /// A zip of NumPy arrays (`.npz`).
    Npz,
    /// A `torch.save` zip archive.
    PyTorch,
    /// A pre-1.6 `torch.save` file (bare pickles), not read.
    PyTorchLegacy,
    /// ONNX `ModelProto`.
    Onnx,
    /// TensorFlow Lite flatbuffer.
    TfLite,
    /// HDF5 (`.h5`, Keras 2 weights).
    Hdf5,
    /// Keras 3 `.keras` zip (HDF5 weights plus JSON config).
    Keras,
    /// Core ML `.mlmodel` protobuf.
    CoreMl,
    /// TensorFlow `GraphDef` / `SavedModel` protobuf.
    TfGraph,
}

impl TensorFormat {
    /// Name shown in the diff header.
    pub fn display_name(self) -> &'static str {
        match self {
            TensorFormat::Safetensors => "safetensors",
            TensorFormat::Gguf => "GGUF",
            TensorFormat::Npy => "NumPy array",
            TensorFormat::Npz => "NumPy archive",
            TensorFormat::PyTorch => "PyTorch",
            TensorFormat::PyTorchLegacy => "PyTorch (legacy format)",
            TensorFormat::Onnx => "ONNX",
            TensorFormat::TfLite => "TensorFlow Lite",
            TensorFormat::Hdf5 => "HDF5",
            TensorFormat::Keras => "Keras",
            TensorFormat::CoreMl => "Core ML",
            TensorFormat::TfGraph => "TensorFlow graph",
        }
    }
}

/// An element type. The plain variants are named after their Rust
/// counterparts (`I8` is `i8`, `BF16` is bfloat16, `Bool` one byte).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DType {
    Bool,
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    U64,
    I64,
    F16,
    BF16,
    #[default]
    F32,
    F64,
    /// 8-bit float, 4 exponent bits (`float8_e4m3fn`).
    F8E4M3,
    /// 8-bit float, 5 exponent bits.
    F8E5M2,
    /// Complex of two f32.
    C64,
    /// Complex of two f64.
    C128,
    /// Strings or other variable-length items.
    Str,
    /// A block-quantized type, by its GGML name (`Q4_K`, `Q8_0`, ...).
    Quant(String),
    /// Anything else, by the format's own name.
    Other(String),
}

impl DType {
    /// Short display name (`f32`, `bf16`, `Q4_K`).
    pub fn name(&self) -> String {
        match self {
            DType::Bool => "bool".into(),
            DType::U8 => "u8".into(),
            DType::I8 => "i8".into(),
            DType::U16 => "u16".into(),
            DType::I16 => "i16".into(),
            DType::U32 => "u32".into(),
            DType::I32 => "i32".into(),
            DType::U64 => "u64".into(),
            DType::I64 => "i64".into(),
            DType::F16 => "f16".into(),
            DType::BF16 => "bf16".into(),
            DType::F32 => "f32".into(),
            DType::F64 => "f64".into(),
            DType::F8E4M3 => "f8_e4m3".into(),
            DType::F8E5M2 => "f8_e5m2".into(),
            DType::C64 => "c64".into(),
            DType::C128 => "c128".into(),
            DType::Str => "string".into(),
            DType::Quant(n) | DType::Other(n) => n.clone(),
        }
    }

    /// Bytes per element for fixed-size, unquantized types.
    pub fn size(&self) -> Option<u64> {
        Some(match self {
            DType::Bool | DType::U8 | DType::I8 | DType::F8E4M3 | DType::F8E5M2 => 1,
            DType::U16 | DType::I16 | DType::F16 | DType::BF16 => 2,
            DType::U32 | DType::I32 | DType::F32 => 4,
            DType::U64 | DType::I64 | DType::F64 | DType::C64 => 8,
            DType::C128 => 16,
            DType::Str | DType::Quant(_) | DType::Other(_) => return None,
        })
    }
}

impl std::fmt::Display for DType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name())
    }
}

/// One stored chunk of an HDF5 chunked dataset.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TensorChunk {
    /// Byte offset of the (filtered) chunk.
    pub offset: u64,
    /// Stored byte size.
    pub size: u64,
    /// Bit `i` set: filter `i` of the pipeline was skipped for this chunk.
    pub filter_mask: u32,
    /// Element coordinates of the chunk's first element, one per dimension.
    pub coords: Vec<u64>,
}

/// Where a tensor's values are, so they can be read later. Offsets are in
/// the file, or in the decompressed zip member `TensorInfo::member` names.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TensorLocation {
    /// The file holds no values for it (graph inputs, empty tensors).
    #[default]
    Missing,
    /// Raw elements, densely packed in row-major order (unless
    /// `TensorInfo::column_major`).
    Bytes { offset: u64, len: u64 },
    /// A strided view into a storage (PyTorch): element `i` of the view
    /// is at `offset + elem_size * sum(index_k * strides[k])`.
    Strided {
        offset: u64,
        storage_len: u64,
        strides: Vec<u64>,
    },
    /// A packed protobuf varint field (`int32_data`, `int64_data`); the
    /// dtype says how to read each varint.
    Varints { offset: u64, len: u64 },
    /// HDF5 chunked storage. `filters` are the pipeline's filter ids
    /// (1 deflate, 2 shuffle, 3 fletcher32).
    Chunked {
        chunk_shape: Vec<u64>,
        filters: Vec<u32>,
        chunks: Vec<TensorChunk>,
    },
    /// Values in another file (ONNX external data).
    External { file: String, offset: u64, len: u64 },
}

/// One tensor of a weight file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TensorInfo {
    /// Full name (`model.layers.0.self_attn.q_proj.weight`, an HDF5 path).
    pub name: String,
    /// Element type.
    pub dtype: DType,
    /// Dimensions in the format's own order (GGUF lists the fastest first).
    pub shape: Vec<u64>,
    /// Symbolic names of dimensions, parallel to `shape` (empty when every
    /// dimension is a number; "" for numeric ones otherwise).
    pub dim_names: Vec<String>,
    /// Number of elements.
    pub elements: u64,
    /// Stored byte length (compressed size for chunked data).
    pub bytes: u64,
    /// Where the values are, for `compare_values`.
    pub location: TensorLocation,
    /// Elements stored big-endian.
    pub big_endian: bool,
    /// Elements stored in Fortran (column-major) order.
    pub column_major: bool,
    /// The zip member the location's offsets are relative to, when that
    /// member is compressed (`.npz`, `.keras`); `None` for file offsets.
    pub member: Option<String>,
}

impl TensorInfo {
    /// `[4096, 4096]`, with symbolic names where present.
    pub fn shape_label(&self) -> String {
        let dims: Vec<String> = self
            .shape
            .iter()
            .enumerate()
            .map(|(i, d)| match self.dim_names.get(i) {
                Some(n) if !n.is_empty() => n.clone(),
                _ => d.to_string(),
            })
            .collect();
        format!("[{}]", dims.join(", "))
    }
}

/// A titled group of metadata entries ("Inputs", "Operators", "Tokenizer").
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MetaSection {
    /// Section heading.
    pub title: String,
    /// Key/value pairs in file order.
    pub entries: Vec<(String, String)>,
}

/// One side of a tensor diff: the file's tensors and metadata.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TensorFile {
    /// The container format.
    pub format: TensorFormat,
    /// Byte size of the whole file.
    pub file_size: u64,
    /// Total element count over all tensors.
    pub parameters: u64,
    /// Total stored tensor bytes.
    pub tensor_bytes: u64,
    /// File-level key/value metadata, in file order (floats as strings).
    pub metadata: Vec<(String, String)>,
    /// Further named groups of metadata.
    pub sections: Vec<MetaSection>,
    /// Tensors in file order.
    pub tensors: Vec<TensorInfo>,
    /// Things the reader skipped or could not read.
    pub notes: Vec<String>,
    /// Where the file's bytes are, for reading values later.
    pub root: Option<crate::ArchiveRoot>,
}

impl TensorFile {
    /// Rough heap size (diff cache sizing).
    pub fn weight(&self) -> usize {
        fn s(v: &str) -> usize {
            v.len() + 24
        }
        let pairs = |v: &[(String, String)]| v.iter().map(|(k, v)| s(k) + s(v)).sum::<usize>();
        let mut w = std::mem::size_of::<Self>() + pairs(&self.metadata);
        w += self
            .sections
            .iter()
            .map(|m| s(&m.title) + pairs(&m.entries))
            .sum::<usize>();
        w += self.notes.iter().map(|n| s(n)).sum::<usize>();
        w += self
            .tensors
            .iter()
            .map(|t| {
                let loc = match &t.location {
                    TensorLocation::Chunked { chunks, .. } => chunks.len() * 64,
                    TensorLocation::Strided { strides, .. } => strides.len() * 8,
                    TensorLocation::External { file, .. } => s(file),
                    _ => 0,
                };
                std::mem::size_of::<TensorInfo>()
                    + s(&t.name)
                    + t.shape.len() * 8
                    + t.dim_names.iter().map(|n| s(n)).sum::<usize>()
                    + t.member.as_deref().map_or(0, s)
                    + loc
            })
            .sum::<usize>();
        w
    }
}

/// How a tensor differs between the two sides, by its layout only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TensorChange {
    /// Only in the new file.
    Added,
    /// Only in the old file.
    Removed,
    /// Same name; dtype, shape or byte length differ.
    Changed,
    /// Different name, same dtype, shape and position.
    Renamed,
    /// Same name, dtype, shape and byte length (values not compared).
    Unchanged,
}

/// One row of the tensor list of a diff.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TensorRow {
    /// How the tensor changed.
    pub change: TensorChange,
    /// The old version (missing for added tensors).
    pub old: Option<TensorInfo>,
    /// The new version (missing for removed tensors).
    pub new: Option<TensorInfo>,
    /// Element types differ.
    pub dtype_changed: bool,
    /// Shapes differ.
    pub shape_changed: bool,
    /// Stored byte lengths differ.
    pub bytes_changed: bool,
}

impl TensorRow {
    /// The new name, or the old one for removed tensors.
    pub fn name(&self) -> &str {
        self.new
            .as_ref()
            .or(self.old.as_ref())
            .map_or("", |t| t.name.as_str())
    }
}

/// Pairs two files' tensors by name (in new-file order, removed tensors
/// after their old predecessor); unmatched tensors at the same position
/// with the same dtype and shape count as renamed.
pub fn align(old: Option<&TensorFile>, new: Option<&TensorFile>) -> Vec<TensorRow> {
    let empty: Vec<TensorInfo> = Vec::new();
    let olds = old.map_or(&empty, |f| &f.tensors);
    let news = new.map_or(&empty, |f| &f.tensors);

    // Key by (name, occurrence) so repeated names pair up in order.
    fn keyed(list: &[TensorInfo]) -> Vec<(&str, usize)> {
        let mut seen: HashMap<&str, usize> = HashMap::new();
        list.iter()
            .map(|t| {
                let n = seen.entry(t.name.as_str()).or_insert(0);
                *n += 1;
                (t.name.as_str(), *n)
            })
            .collect()
    }
    let old_keys = keyed(olds);
    let new_keys = keyed(news);
    let old_index: HashMap<(&str, usize), usize> =
        old_keys.iter().enumerate().map(|(i, k)| (*k, i)).collect();

    let mut new_match: Vec<Option<usize>> =
        new_keys.iter().map(|k| old_index.get(k).copied()).collect();
    let mut old_used = vec![false; olds.len()];
    for m in new_match.iter().flatten() {
        if let Some(u) = old_used.get_mut(*m) {
            *u = true;
        }
    }
    // Renames: same position, same layout, both unmatched.
    for (j, m) in new_match.iter_mut().enumerate() {
        if m.is_none()
            && j < olds.len()
            && !old_used[j]
            && olds[j].dtype == news[j].dtype
            && olds[j].shape == news[j].shape
        {
            *m = Some(j);
            old_used[j] = true;
        }
    }

    let row = |o: Option<&TensorInfo>, n: Option<&TensorInfo>| {
        let (change, d, s, b) = match (o, n) {
            (Some(o), Some(n)) => {
                let d = o.dtype != n.dtype;
                let s = o.shape != n.shape;
                let b = o.bytes != n.bytes;
                let change = if o.name != n.name {
                    TensorChange::Renamed
                } else if d || s || b {
                    TensorChange::Changed
                } else {
                    TensorChange::Unchanged
                };
                (change, d, s, b)
            }
            (Some(_), None) => (TensorChange::Removed, false, false, false),
            _ => (TensorChange::Added, false, false, false),
        };
        TensorRow {
            change,
            old: o.cloned(),
            new: n.cloned(),
            dtype_changed: d,
            shape_changed: s,
            bytes_changed: b,
        }
    };

    let mut rows = Vec::with_capacity(olds.len().max(news.len()));
    let mut emitted = vec![false; olds.len()];
    let mut next_old = 0usize;
    for (j, n) in news.iter().enumerate() {
        if let Some(i) = new_match[j] {
            // Removed tensors that came before this one in the old file.
            while next_old < i {
                if !old_used[next_old] && !emitted[next_old] {
                    emitted[next_old] = true;
                    rows.push(row(Some(&olds[next_old]), None));
                }
                next_old += 1;
            }
            emitted[i] = true;
            rows.push(row(Some(&olds[i]), Some(n)));
        } else {
            rows.push(row(None, Some(n)));
        }
    }
    for (i, o) in olds.iter().enumerate() {
        if !old_used[i] && !emitted[i] {
            rows.push(row(Some(o), None));
        }
    }
    rows
}

/// One metadata key across both sides.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MetaRow {
    /// Section title; "" for the file-level metadata.
    pub section: String,
    /// Metadata key.
    pub key: String,
    /// Old value (missing for added keys).
    pub old: Option<String>,
    /// New value (missing for removed keys).
    pub new: Option<String>,
}

impl MetaRow {
    /// Both sides hold the same value.
    pub fn unchanged(&self) -> bool {
        self.old == self.new
    }
}

/// Pairs the metadata entries (file-level first, then each section) of two
/// files by section and key, in new-file order with removed keys last.
pub fn align_metadata(old: Option<&TensorFile>, new: Option<&TensorFile>) -> Vec<MetaRow> {
    fn groups(f: Option<&TensorFile>) -> Vec<(&str, &[(String, String)])> {
        let Some(f) = f else { return Vec::new() };
        let mut g: Vec<(&str, &[(String, String)])> = vec![("", &f.metadata)];
        g.extend(
            f.sections
                .iter()
                .map(|s| (s.title.as_str(), s.entries.as_slice())),
        );
        g
    }
    let og = groups(old);
    let ng = groups(new);
    let mut titles: Vec<&str> = Vec::new();
    for (t, _) in ng.iter().chain(og.iter()) {
        if !titles.contains(t) {
            titles.push(t);
        }
    }
    let mut rows = Vec::new();
    for title in titles {
        let find = |g: &[(&str, &[(String, String)])]| -> Vec<(String, String)> {
            g.iter()
                .filter(|(t, _)| *t == title)
                .flat_map(|(_, e)| e.iter().cloned())
                .collect()
        };
        let o = find(&og);
        let n = find(&ng);
        let mut old_map: HashMap<&str, Vec<&str>> = HashMap::new();
        for (k, v) in &o {
            old_map.entry(k.as_str()).or_default().push(v.as_str());
        }
        let mut taken: HashMap<&str, usize> = HashMap::new();
        for (k, v) in &n {
            let i = taken.entry(k.as_str()).or_insert(0);
            let ov = old_map
                .get(k.as_str())
                .and_then(|vs| vs.get(*i))
                .map(|s| s.to_string());
            *i += 1;
            rows.push(MetaRow {
                section: title.to_string(),
                key: k.clone(),
                old: ov,
                new: Some(v.clone()),
            });
        }
        // Removed keys in old order.
        let mut seen: HashMap<&str, usize> = HashMap::new();
        for (k, v) in &o {
            let i = seen.entry(k.as_str()).or_insert(0);
            *i += 1;
            if *i > taken.get(k.as_str()).copied().unwrap_or(0) {
                rows.push(MetaRow {
                    section: title.to_string(),
                    key: k.clone(),
                    old: Some(v.clone()),
                    new: None,
                });
            }
        }
    }
    rows
}

/// Value statistics of one tensor.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TensorStats {
    /// Elements read.
    pub count: u64,
    /// Smallest finite value.
    pub min: f64,
    /// Largest finite value.
    pub max: f64,
    /// Mean of finite values.
    pub mean: f64,
    /// Population standard deviation of finite values.
    pub std: f64,
    /// Elements equal to zero.
    pub zeros: u64,
    /// NaN elements.
    pub nan: u64,
    /// Infinite elements.
    pub inf: u64,
    /// 16 equal buckets from `min` to `max` over up to 65536 evenly spaced
    /// finite values.
    pub histogram: Vec<u64>,
}

/// One element that differs between the two sides.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DiffSample {
    /// Flat (row-major as stored) element index.
    pub index: u64,
    /// Old value.
    pub old: f64,
    /// New value.
    pub new: f64,
}

/// Element-wise comparison of two versions of a tensor.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ValueDiff {
    /// Largest absolute difference.
    pub max_abs_diff: f64,
    /// Mean absolute difference over finite pairs.
    pub mean_abs_diff: f64,
    /// Root mean square difference over finite pairs.
    pub rms_diff: f64,
    /// Elements whose decoded bits differ.
    pub changed: u64,
    /// Cosine similarity; `None` when a side is all zeros.
    pub cosine: Option<f64>,
    /// Every element is bitwise equal.
    pub identical: bool,
    /// Up to 8 elements with the largest differences, largest first.
    pub samples: Vec<DiffSample>,
}

/// The value comparison of one tensor (not part of `Diff`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TensorValues {
    /// Tensor name.
    pub name: String,
    /// Statistics of the old version, when read.
    pub old: Option<TensorStats>,
    /// Statistics of the new version, when read.
    pub new: Option<TensorStats>,
    /// Set when both sides were read with the same element count.
    pub diff: Option<ValueDiff>,
    /// Why something is missing (over budget, not decoded, shapes differ).
    pub note: Option<String>,
}
