//! Corvene `1340-tensor-diff`: model weight files read as tensor lists
//! (name, shape, dtype) and metadata, and tensor values compared on demand.
//!
//! GitHub Desktop shows these files as binary (`app/src/ui/diff/binary-file.tsx`).
//! Readers only read headers and indexes (never whole files); values are
//! streamed in chunks through [`Source::read_at`] when asked for.

use std::sync::atomic::AtomicBool;

use corvene_models::TensorFile;
use corvene_models::tensors::{TensorFormat, TensorValues};

mod coreml;
mod gguf;
mod hdf5;
mod numpy;
mod onnx;
mod pickle;
mod proto;
mod safetensors;
mod source;
mod tfgraph;
mod tflite;
mod torch;
mod values;
mod zipfile;

pub use source::Source;

/// Errors reading a weight file.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported or malformed file: {0}")]
    Format(String),
}

const EXTENSIONS: &[&str] = &[
    "safetensors",
    "gguf",
    "npy",
    "npz",
    "pt",
    "pth",
    "bin",
    "ckpt",
    "onnx",
    "tflite",
    "h5",
    "hdf5",
    "keras",
    "mlmodel",
    "pb",
];

fn extension(path: &str) -> String {
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match file.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext.to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// Whether `path` names a weight file this crate reads (by extension).
pub fn is_tensor_file(path: &str) -> bool {
    EXTENSIONS.contains(&extension(path).as_str())
}

/// The tensors and metadata of the weight file at `path` (its extension
/// and magic pick the format). `root` is left for the caller to set.
pub fn read(path: &str, source: &dyn Source) -> Result<TensorFile, Error> {
    let ext = extension(path);
    let mut file = match ext.as_str() {
        "safetensors" => safetensors::read(source)?,
        "gguf" => gguf::read(source)?,
        "npy" => numpy::read_npy(source)?,
        "npz" => numpy::read_npz(source)?,
        "pt" | "pth" | "bin" | "ckpt" => torch::read(source, &ext)?,
        "onnx" => onnx::read(source)?,
        "tflite" => tflite::read(source)?,
        "h5" | "hdf5" => hdf5::read(source, TensorFormat::Hdf5)?,
        "keras" => hdf5::read_keras(source)?,
        "mlmodel" => coreml::read(source)?,
        "pb" => tfgraph::read(source)?,
        _ => {
            return Err(Error::Format(format!(
                "not a weight file extension: .{ext}"
            )));
        }
    };
    file.file_size = source.len();
    file.parameters = file
        .tensors
        .iter()
        .fold(0u64, |a, t| a.saturating_add(t.elements));
    file.tensor_bytes = file
        .tensors
        .iter()
        .fold(0u64, |a, t| a.saturating_add(t.bytes));
    Ok(file)
}

/// Value statistics of `names` (all tensors when empty) on both sides and,
/// where both sides hold the same number of elements, their element-wise
/// differences. Each side reads at most `max_bytes`; tensors beyond it are
/// reported as skipped. Stops early (returning what is done) when `cancel`
/// is set.
pub fn compare_values(
    old: Option<(&TensorFile, &dyn Source)>,
    new: Option<(&TensorFile, &dyn Source)>,
    names: &[String],
    max_bytes: u64,
    cancel: &AtomicBool,
) -> Vec<TensorValues> {
    values::compare(old, new, names, max_bytes, cancel)
}

/// Row-major element count of `shape`, or `None` on overflow.
pub(crate) fn elements(shape: &[u64]) -> Option<u64> {
    shape.iter().try_fold(1u64, |a, d| a.checked_mul(*d))
}

/// `[n items] a, b, c, ...` for long lists.
pub(crate) fn summarize(items: &[String], total: u64) -> String {
    const SHOW: usize = 5;
    let shown: Vec<&str> = items.iter().take(SHOW).map(|s| s.as_str()).collect();
    if total as usize <= SHOW && items.len() as u64 == total {
        format!("[{}]", shown.join(", "))
    } else {
        format!("[{total} items] {}, …", shown.join(", "))
    }
}

/// A float formatted for metadata (shortest round-trip form).
pub(crate) fn fmt_f64(v: f64) -> String {
    format!("{v}")
}

/// Clips long text for display.
pub(crate) fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max).collect();
        t.push('…');
        t
    }
}
