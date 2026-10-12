//! Zip containers (`.npz`, `torch.save`, `.keras`): the central directory
//! and member reads, never the whole archive.

use std::io::Read;

use crate::source::{ReadSeek, bad};
use crate::{Error, Source};

/// One member of a zip archive.
pub(crate) struct Member {
    pub name: String,
    pub size: u64,
    /// Stored without compression: its bytes are `data_start..+size`.
    pub stored: bool,
    pub data_start: u64,
}

pub(crate) fn is_zip(src: &dyn Source) -> bool {
    let mut m = [0u8; 4];
    src.read_at(0, &mut m).is_ok() && (&m == b"PK\x03\x04" || &m == b"PK\x05\x06")
}

type Archive<'a> = zip::ZipArchive<ReadSeek<'a>>;

fn open(src: &dyn Source) -> Result<Archive<'_>, Error> {
    zip::ZipArchive::new(ReadSeek { src, pos: 0 }).map_err(|e| bad(format!("zip: {e}")))
}

/// The archive's members, in central directory order.
pub(crate) fn list(src: &dyn Source) -> Result<Vec<Member>, Error> {
    let mut z = open(src)?;
    let mut out = Vec::with_capacity(z.len().min(1 << 16));
    for i in 0..z.len() {
        let f = z.by_index_raw(i).map_err(|e| bad(format!("zip: {e}")))?;
        if f.is_dir() {
            continue;
        }
        let stored = f.compression() == zip::CompressionMethod::Stored;
        out.push(Member {
            name: f.name().to_string(),
            size: f.size(),
            stored,
            data_start: f.data_start().unwrap_or(0),
        });
    }
    Ok(out)
}

/// Bytes `offset..offset + len` of a member's decompressed contents.
pub(crate) fn read_range(
    src: &dyn Source,
    member: &Member,
    offset: u64,
    len: u64,
) -> Result<Vec<u8>, Error> {
    if offset.checked_add(len).is_none_or(|e| e > member.size) {
        return Err(bad("zip member range out of bounds"));
    }
    if len > crate::source::MAX_ALLOC {
        return Err(bad("zip member range too large"));
    }
    if member.stored {
        let mut buf = vec![0u8; len as usize];
        src.read_at(member.data_start + offset, &mut buf)?;
        return Ok(buf);
    }
    read_member_range(src, &member.name, offset, len)
}

/// Decompresses `name` and returns `offset..offset + len` of it.
pub(crate) fn read_member_range(
    src: &dyn Source,
    name: &str,
    offset: u64,
    len: u64,
) -> Result<Vec<u8>, Error> {
    let mut z = open(src)?;
    let f = z.by_name(name).map_err(|e| bad(format!("zip: {e}")))?;
    let mut r = f.take(offset.saturating_add(len));
    std::io::copy(&mut (&mut r).take(offset), &mut std::io::sink())?;
    let mut out = Vec::with_capacity(len.min(crate::source::MAX_ALLOC) as usize);
    r.read_to_end(&mut out)?;
    if (out.len() as u64) < len {
        return Err(bad("zip member shorter than its size"));
    }
    Ok(out)
}

/// A whole member's contents, refused above `cap` bytes.
pub(crate) fn read_member(src: &dyn Source, member: &Member, cap: u64) -> Result<Vec<u8>, Error> {
    if member.size > cap {
        return Err(bad(format!(
            "{} is too large to read ({} bytes)",
            member.name, member.size
        )));
    }
    read_range(src, member, 0, member.size)
}
