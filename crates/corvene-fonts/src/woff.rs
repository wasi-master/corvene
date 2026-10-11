//! WOFF 1.0 decoding (W3C) and the sfnt / collection writer WOFF 2.0
//! shares. Moved from `corvene-extensions` `icon_theme::decode_woff`.

use std::io::Read;

use crate::FontError;

const FORMAT: &str = "WOFF";

pub(crate) fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

pub(crate) fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// One table of an sfnt being written.
pub(crate) struct Table {
    pub tag: [u8; 4],
    pub checksum: u32,
    pub data: Vec<u8>,
}

/// The OpenType table checksum (sum of big-endian u32s, zero padded).
pub(crate) fn checksum(data: &[u8]) -> u32 {
    let mut sum = 0u32;
    let (words, rest) = data.as_chunks::<4>();
    for word in words {
        sum = sum.wrapping_add(u32::from_be_bytes(*word));
    }
    if !rest.is_empty() {
        let mut last = [0u8; 4];
        last[..rest.len()].copy_from_slice(rest);
        sum = sum.wrapping_add(u32::from_be_bytes(last));
    }
    sum
}

/// WOFF 1.0: a 44-byte header, a 20-byte directory entry per table, each
/// table zlib-compressed unless its compressed length equals its original
/// one. Rebuilt as an sfnt.
pub(crate) fn decode(b: &[u8]) -> Result<Vec<u8>, FontError> {
    let bad = |reason: &str| FontError::malformed(FORMAT, reason);
    let flavor = be32(b, 4).ok_or_else(|| bad("truncated header"))?;
    let count = be16(b, 12).ok_or_else(|| bad("truncated header"))? as usize;
    if count == 0 || count > 1024 {
        return Err(bad("bad table count"));
    }
    let mut tables = Vec::with_capacity(count);
    for i in 0..count {
        let at = 44 + i * 20;
        let entry = (|| {
            Some((
                b.get(at..at + 4)?.try_into().ok()?,
                be32(b, at + 4)? as usize,
                be32(b, at + 8)? as usize,
                be32(b, at + 12)? as usize,
                be32(b, at + 16)?,
            ))
        })();
        let (tag, offset, comp, orig, checksum): ([u8; 4], _, _, _, _) =
            entry.ok_or_else(|| bad("truncated table directory"))?;
        if orig > crate::MAX_BYTES {
            return Err(FontError::TooLarge);
        }
        let raw = offset
            .checked_add(comp)
            .and_then(|end| b.get(offset..end))
            .ok_or_else(|| bad("table outside the file"))?;
        let data = if comp == orig {
            raw.to_vec()
        } else {
            let mut out = Vec::with_capacity(orig);
            flate2::read::ZlibDecoder::new(raw)
                .take(orig as u64 + 1)
                .read_to_end(&mut out)
                .map_err(|e| bad(&format!("table {}: {e}", tag_str(tag))))?;
            if out.len() != orig {
                return Err(bad(&format!("table {} has the wrong length", tag_str(tag))));
            }
            out
        };
        tables.push(Table {
            tag,
            checksum,
            data,
        });
    }
    Ok(write_sfnt(flavor, &tables))
}

pub(crate) fn tag_str(tag: [u8; 4]) -> String {
    String::from_utf8_lossy(&tag).into_owned()
}

/// The 12-byte sfnt header for `count` tables.
fn sfnt_header(flavor: u32, count: usize) -> Vec<u8> {
    let mut search = 1usize;
    let mut selector = 0u16;
    while search * 2 <= count {
        search *= 2;
        selector += 1;
    }
    let search_range = (search * 16) as u16;
    let mut out = Vec::with_capacity(12 + count * 16);
    out.extend_from_slice(&flavor.to_be_bytes());
    out.extend_from_slice(&(count as u16).to_be_bytes());
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&selector.to_be_bytes());
    out.extend_from_slice(&((count as u16 * 16).saturating_sub(search_range)).to_be_bytes());
    out
}

fn pad4(v: &mut Vec<u8>) {
    let pad = (4 - v.len() % 4) % 4;
    v.extend(std::iter::repeat_n(0u8, pad));
}

/// An sfnt with `tables` (sorted by tag, as the directory requires), each
/// 4-byte aligned.
pub(crate) fn write_sfnt(flavor: u32, tables: &[Table]) -> Vec<u8> {
    let mut order: Vec<&Table> = tables.iter().collect();
    order.sort_by_key(|t| t.tag);
    let mut out = sfnt_header(flavor, order.len());
    let mut offset = 12 + order.len() * 16;
    for t in &order {
        out.extend_from_slice(&t.tag);
        out.extend_from_slice(&t.checksum.to_be_bytes());
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(t.data.len() as u32).to_be_bytes());
        offset += t.data.len().div_ceil(4) * 4;
    }
    for t in &order {
        out.extend_from_slice(&t.data);
        pad4(&mut out);
    }
    out
}

/// A TrueType collection: every font's directory points into one shared
/// table area (`fonts[i]` lists indexes into `tables`).
pub(crate) fn write_collection(flavors: &[u32], fonts: &[Vec<usize>], tables: &[Table]) -> Vec<u8> {
    let header_len = 12 + 4 * fonts.len();
    let dirs_len: usize = fonts.iter().map(|f| 12 + 16 * f.len()).sum();
    // table offsets in the shared area
    let mut offsets = Vec::with_capacity(tables.len());
    let mut at = header_len + dirs_len;
    at = at.div_ceil(4) * 4;
    for t in tables {
        offsets.push(at);
        at += t.data.len().div_ceil(4) * 4;
    }
    let mut out = Vec::with_capacity(at);
    out.extend_from_slice(b"ttcf");
    out.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // 1.0: no DSIG fields
    out.extend_from_slice(&(fonts.len() as u32).to_be_bytes());
    let mut dir_at = header_len;
    for f in fonts {
        out.extend_from_slice(&(dir_at as u32).to_be_bytes());
        dir_at += 12 + 16 * f.len();
    }
    for (font, flavor) in fonts.iter().zip(flavors) {
        let mut order = font.clone();
        order.sort_by_key(|&i| tables[i].tag);
        out.extend_from_slice(&sfnt_header(*flavor, order.len()));
        for &i in &order {
            let t = &tables[i];
            out.extend_from_slice(&t.tag);
            out.extend_from_slice(&t.checksum.to_be_bytes());
            out.extend_from_slice(&(offsets[i] as u32).to_be_bytes());
            out.extend_from_slice(&(t.data.len() as u32).to_be_bytes());
        }
    }
    pad4(&mut out);
    for t in tables {
        out.extend_from_slice(&t.data);
        pad4(&mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_pads_the_last_word() {
        assert_eq!(checksum(&[0, 0, 0, 1, 0, 0, 0, 2]), 3);
        assert_eq!(checksum(&[1]), 0x0100_0000);
    }

    #[test]
    fn sfnt_tables_are_sorted_and_aligned() {
        let tables = [
            Table {
                tag: *b"name",
                checksum: 1,
                data: vec![1, 2, 3],
            },
            Table {
                tag: *b"head",
                checksum: 2,
                data: vec![4; 8],
            },
        ];
        let out = write_sfnt(0x0001_0000, &tables);
        assert_eq!(&out[12..16], b"head");
        assert_eq!(&out[28..32], b"name");
        assert_eq!(be32(&out, 12 + 8), Some(44));
        assert_eq!(be32(&out, 28 + 8), Some(52));
        assert_eq!(out.len(), 44 + 8 + 4);
    }
}
