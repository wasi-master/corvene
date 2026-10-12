//! Protobuf wire format over a `Reader`: fields are walked in place and
//! length-delimited payloads are only read when asked for, so large
//! `raw_data` blobs are skipped.

use crate::Error;
use crate::source::{Reader, bad};

/// One field's value.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Field {
    Varint(u64),
    I64(u64),
    I32(u32),
    /// A length-delimited payload at `offset..offset + len`.
    Len {
        offset: u64,
        len: u64,
    },
}

impl Field {
    pub fn int(self) -> u64 {
        match self {
            Field::Varint(v) | Field::I64(v) => v,
            Field::I32(v) => v as u64,
            Field::Len { .. } => 0,
        }
    }
    pub fn range(self) -> Option<(u64, u64)> {
        match self {
            Field::Len { offset, len } => Some((offset, len)),
            _ => None,
        }
    }
}

pub(crate) fn varint(r: &mut Reader) -> Result<u64, Error> {
    let mut v = 0u64;
    for shift in (0..70).step_by(7) {
        let b = r.u8()?;
        if shift < 64 {
            v |= ((b & 0x7f) as u64) << shift;
        }
        if b & 0x80 == 0 {
            return Ok(v);
        }
    }
    Err(bad("varint too long"))
}

/// Calls `f` for every field of the message at `start..end`. `f` may move
/// the reader; it is put back after each field.
pub(crate) fn fields(
    r: &mut Reader,
    start: u64,
    end: u64,
    mut f: impl FnMut(&mut Reader, u32, Field) -> Result<(), Error>,
) -> Result<(), Error> {
    if end > r.len() || start > end {
        return Err(bad("protobuf message out of bounds"));
    }
    r.seek(start);
    while r.pos() < end {
        let key = varint(r)?;
        let no = (key >> 3) as u32;
        let field = match key & 7 {
            0 => Field::Varint(varint(r)?),
            1 => Field::I64(r.u64()?),
            5 => Field::I32(r.u32()?),
            2 => {
                let len = varint(r)?;
                let offset = r.pos();
                if offset.checked_add(len).is_none_or(|e| e > end) {
                    return Err(bad("protobuf field runs past its message"));
                }
                Field::Len { offset, len }
            }
            w => return Err(bad(format!("protobuf wire type {w} not supported"))),
        };
        let next = match field {
            Field::Len { offset, len } => offset + len,
            _ => r.pos(),
        };
        if next > end {
            return Err(bad("protobuf field runs past its message"));
        }
        f(r, no, field)?;
        r.seek(next);
    }
    Ok(())
}

/// A length-delimited field as text (clipped).
pub(crate) fn string(r: &mut Reader, field: Field, max: u64) -> Result<String, Error> {
    let Some((offset, len)) = field.range() else {
        return Ok(String::new());
    };
    let keep = r.pos();
    r.seek(offset);
    let s = r.string(len.min(max));
    r.seek(keep);
    let mut s = s?;
    if len > max {
        s.push('…');
    }
    Ok(s)
}

/// Repeated integers, packed or one per field.
pub(crate) fn ints(
    r: &mut Reader,
    field: Field,
    out: &mut Vec<u64>,
    cap: usize,
) -> Result<(), Error> {
    match field {
        Field::Len { offset, len } => {
            let keep = r.pos();
            r.seek(offset);
            while r.pos() < offset + len && out.len() < cap {
                out.push(varint(r)?);
            }
            r.seek(keep);
        }
        other => {
            if out.len() < cap {
                out.push(other.int());
            }
        }
    }
    Ok(())
}
