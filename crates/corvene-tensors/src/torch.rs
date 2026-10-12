//! PyTorch `torch.save` zip archives (`.pt`, `.pth`, `.bin`, `.ckpt`):
//! `<prefix>/data.pkl` names tensors and their storages, stored as
//! `<prefix>/data/<key>`. Nested dicts (state dicts, Lightning
//! checkpoints) are flattened with dots; scalars become metadata.

use std::collections::HashSet;
use std::rc::Rc;

use corvene_models::TensorFile;
use corvene_models::tensors::{TensorFormat, TensorInfo, TensorLocation};

use crate::pickle::{self, TensorRef, V};
use crate::source::bad;
use crate::zipfile::{self, Member};
use crate::{Error, Source};

const MAX_PICKLE: u64 = 64 * 1024 * 1024;
const MAX_META: usize = 300;
const MAX_VISITS: usize = 2_000_000;
const MAX_DEPTH: u32 = 48;

/// `torch.save` before 1.6 starts with a pickled magic number.
const LEGACY_MAGIC: &[u8] = b"\x80\x02\x8a\x0a\x6c\xfc\x9c\x46\xf9\x20\x6a\xa8\x50\x19";

pub(crate) fn read(src: &dyn Source, ext: &str) -> Result<TensorFile, Error> {
    if !zipfile::is_zip(src) {
        let mut head = [0u8; 14];
        if src.read_at(0, &mut head).is_ok() && head == LEGACY_MAGIC {
            return Ok(TensorFile {
                format: TensorFormat::PyTorchLegacy,
                notes: vec!["legacy PyTorch format (before 1.6): tensors not read".into()],
                ..TensorFile::default()
            });
        }
        return Err(bad(format!(".{ext} file is not a PyTorch archive")));
    }
    let members = zipfile::list(src)?;
    let pkl = members
        .iter()
        .filter(|m| m.name == "data.pkl" || m.name.ends_with("/data.pkl"))
        .min_by_key(|m| m.name.matches('/').count())
        .ok_or_else(|| bad("zip without data.pkl is not a PyTorch archive"))?;
    let prefix = pkl.name.trim_end_matches("data.pkl").to_string();
    let data = zipfile::read_member(src, pkl, MAX_PICKLE)?;
    let root = pickle::load(&data)?;

    let mut file = TensorFile {
        format: TensorFormat::PyTorch,
        ..TensorFile::default()
    };
    let small = |name: &str| -> Option<String> {
        let m = members
            .iter()
            .find(|m| m.name == format!("{prefix}{name}"))?;
        let b = zipfile::read_member(src, m, 64).ok()?;
        Some(String::from_utf8_lossy(&b).trim().to_string())
    };
    if let Some(v) = small("version") {
        file.metadata.push(("Serialization version".into(), v));
    }
    let big_endian = small("byteorder").as_deref() == Some("big");
    if members
        .iter()
        .any(|m| m.name.starts_with(&format!("{prefix}code/")))
    {
        file.metadata
            .push(("Kind".into(), "TorchScript module".into()));
    }

    let mut w = Walk {
        tensors: Vec::new(),
        meta: Vec::new(),
        visits: 0,
        opaque: HashSet::new(),
    };
    w.walk(&root, "", 0);
    file.metadata.append(&mut w.meta);
    if w.visits >= MAX_VISITS {
        file.notes
            .push("object graph too large: read partially".into());
    }
    let mut opaque: Vec<String> = w.opaque.into_iter().collect();
    opaque.sort();
    if !opaque.is_empty() {
        file.notes.push(format!(
            "objects not read: {}",
            crate::clip(&opaque.join(", "), 400)
        ));
    }

    for (name, t) in w.tensors {
        match tensor(&name, &t, &members, &prefix, big_endian) {
            Ok(info) => file.tensors.push(info),
            Err(e) => file.notes.push(format!("{name}: {e}")),
        }
    }
    Ok(file)
}

fn tensor(
    name: &str,
    t: &TensorRef,
    members: &[Member],
    prefix: &str,
    big_endian: bool,
) -> Result<TensorInfo, String> {
    let member_name = format!("{prefix}data/{}", t.storage.key);
    let m = members
        .iter()
        .find(|m| m.name == member_name)
        .ok_or_else(|| format!("storage {member_name} is missing"))?;
    let elements = crate::elements(&t.size).ok_or("shape overflows")?;
    let es = t.storage.dtype.size();
    let (base, member) = if m.stored {
        (m.data_start, None)
    } else {
        (0, Some(m.name.clone()))
    };
    let mut info = TensorInfo {
        name: name.to_string(),
        dtype: t.storage.dtype.clone(),
        shape: t.size.clone(),
        elements,
        big_endian: big_endian && es.is_some_and(|s| s > 1),
        member,
        ..TensorInfo::default()
    };
    let Some(es) = es else {
        return Ok(info);
    };
    let start = t.offset.checked_mul(es).ok_or("offset overflows")?;
    info.bytes = elements.checked_mul(es).ok_or("size overflows")?;
    if elements == 0 {
        return Ok(info);
    }
    // Row-major strides, ignoring dimensions of size 1.
    let mut expect = 1u64;
    let mut contiguous = t.stride.len() == t.size.len();
    for (d, s) in t.size.iter().zip(&t.stride).rev() {
        if *d != 1 && *s != expect {
            contiguous = false;
        }
        expect = expect.saturating_mul(*d);
    }
    if contiguous {
        if start.checked_add(info.bytes).is_none_or(|e| e > m.size) {
            return Err("tensor runs past its storage".into());
        }
        info.location = TensorLocation::Bytes {
            offset: base + start,
            len: info.bytes,
        };
    } else {
        if t.stride.len() != t.size.len() {
            return Err("strides do not match the shape".into());
        }
        let last = t
            .size
            .iter()
            .zip(&t.stride)
            .try_fold(0u64, |a, (d, s)| a.checked_add((d - 1).checked_mul(*s)?))
            .ok_or("strides overflow")?;
        let span = last
            .checked_add(1)
            .and_then(|n| n.checked_mul(es))
            .ok_or("strides overflow")?;
        if start.checked_add(span).is_none_or(|e| e > m.size) {
            return Err("tensor runs past its storage".into());
        }
        info.location = TensorLocation::Strided {
            offset: base + start,
            storage_len: span,
            strides: t.stride.clone(),
        };
    }
    Ok(info)
}

struct Walk {
    tensors: Vec<(String, Rc<TensorRef>)>,
    meta: Vec<(String, String)>,
    visits: usize,
    opaque: HashSet<String>,
}

impl Walk {
    fn join(prefix: &str, key: &str) -> String {
        if prefix.is_empty() {
            key.to_string()
        } else {
            format!("{prefix}.{key}")
        }
    }

    fn scalar(&mut self, path: &str, v: String) {
        if self.meta.len() < MAX_META && !path.is_empty() {
            self.meta.push((path.to_string(), crate::clip(&v, 200)));
        }
    }

    fn walk(&mut self, v: &V, path: &str, depth: u32) {
        self.visits += 1;
        if self.visits >= MAX_VISITS || depth > MAX_DEPTH {
            return;
        }
        match v {
            V::Tensor(t) => self.tensors.push((path.to_string(), t.clone())),
            V::Dict(d) => {
                let items = d.borrow().clone();
                for (k, v) in items {
                    let key = match &k {
                        V::Str(s) => s.to_string(),
                        V::Int(i) => i.to_string(),
                        V::Bool(b) => b.to_string(),
                        _ => continue,
                    };
                    self.walk(&v, &Self::join(path, &key), depth + 1);
                }
            }
            V::List(_) | V::Tuple(_) => {
                let items = match v {
                    V::List(l) => l.borrow().clone(),
                    V::Tuple(t) => t.as_ref().clone(),
                    _ => Vec::new(),
                };
                for (i, it) in items.iter().enumerate() {
                    self.walk(it, &Self::join(path, &i.to_string()), depth + 1);
                }
            }
            V::Object { class, state } => {
                let st = state.borrow().clone();
                if matches!(st, V::None) {
                    self.opaque.insert(class.to_string());
                } else {
                    self.walk(&st, path, depth + 1);
                }
            }
            V::Int(i) => self.scalar(path, i.to_string()),
            V::Float(f) => self.scalar(path, crate::fmt_f64(*f)),
            V::Bool(b) => self.scalar(path, b.to_string()),
            V::Str(s) => self.scalar(path, s.to_string()),
            V::None | V::Bytes | V::Global(_) | V::Storage(_) => {}
        }
    }
}
