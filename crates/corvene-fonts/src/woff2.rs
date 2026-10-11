//! WOFF 2.0 decoding (W3C "WOFF File Format 2.0"; reference decoder
//! google/woff2 `src/woff2_dec.cc`), rebuilt as an sfnt or, for a WOFF2
//! collection, a TrueType collection.
//!
//! A WOFF2 file is a 48-byte header, a variable-length table directory,
//! an optional collection directory and one Brotli stream holding every
//! table back to back. `glyf` / `loca` may be stored in the transformed
//! form of section 5.1 and `hmtx` in the one of section 5.4; both are
//! rebuilt here. WOFF2 drops table checksums, so every table checksum and
//! `head.checkSumAdjustment` is computed again (per font in a collection,
//! like the reference; a `head` shared by several fonts keeps the value of
//! the last one). Extended metadata and private data are not part of the
//! sfnt and are skipped.

use std::io::Read;
use std::ops::Range;

use crate::FontError;
use crate::woff::{Table, be16, be32, checksum, tag_str, write_collection, write_sfnt};

const FORMAT: &str = "WOFF2";

const SIGNATURE: u32 = 0x774F_4632; // "wOF2"
const TTCF: u32 = 0x7474_6366; // "ttcf"
const HEADER_LEN: usize = 48;

/// Section 4 "Table directory": the tags a flags byte can name by index
/// (63 means an explicit tag follows).
const KNOWN_TAGS: [[u8; 4]; 63] = [
    *b"cmap", *b"head", *b"hhea", *b"hmtx", *b"maxp", *b"name", *b"OS/2", *b"post", *b"cvt ",
    *b"fpgm", *b"glyf", *b"loca", *b"prep", *b"CFF ", *b"VORG", *b"EBDT", *b"EBLC", *b"gasp",
    *b"hdmx", *b"kern", *b"LTSH", *b"PCLT", *b"VDMX", *b"vhea", *b"vmtx", *b"BASE", *b"GDEF",
    *b"GPOS", *b"GSUB", *b"EBSC", *b"JSTF", *b"MATH", *b"CBDT", *b"CBLC", *b"COLR", *b"CPAL",
    *b"SVG ", *b"sbix", *b"acnt", *b"avar", *b"bdat", *b"bloc", *b"bsln", *b"cvar", *b"fdsc",
    *b"feat", *b"fmtx", *b"fvar", *b"gvar", *b"hsty", *b"just", *b"lcar", *b"mort", *b"morx",
    *b"opbd", *b"prop", *b"trak", *b"Zapf", *b"Silf", *b"Glat", *b"Gloc", *b"Feat", *b"Sill",
];

fn bad(reason: impl Into<String>) -> FontError {
    FontError::malformed(FORMAT, reason)
}

/// A bounds-checked big-endian reader; every read returns `None` past the end.
struct Cursor<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let bytes = self.data.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(bytes)
    }

    fn rest(&self) -> &'a [u8] {
        self.data.get(self.at..).unwrap_or_default()
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes(self.take(2)?.try_into().ok()?))
    }

    fn i16(&mut self) -> Option<i16> {
        Some(i16::from_be_bytes(self.take(2)?.try_into().ok()?))
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }

    fn tag(&mut self) -> Option<[u8; 4]> {
        self.take(4)?.try_into().ok()
    }

    /// Section 6.1.1 "UIntBase128": 1 to 5 bytes, 7 bits each, high bit
    /// set on all but the last; no leading zero byte, no overflow.
    fn base128(&mut self) -> Option<u32> {
        let mut value = 0u32;
        for i in 0..5 {
            let byte = self.u8()?;
            if i == 0 && byte == 0x80 {
                return None;
            }
            if value & 0xFE00_0000 != 0 {
                return None;
            }
            value = (value << 7) | u32::from(byte & 0x7F);
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    }

    /// Section 6.1.1 "255UInt16": one byte below 253, or a code byte
    /// followed by one (`253 +`, `506 +`) or two bytes.
    fn u255(&mut self) -> Option<u16> {
        const WORD_CODE: u8 = 253;
        const ONE_MORE_BYTE_CODE_2: u8 = 254;
        const ONE_MORE_BYTE_CODE_1: u8 = 255;
        const LOWEST_U_CODE: u16 = 253;
        match self.u8()? {
            WORD_CODE => self.u16(),
            ONE_MORE_BYTE_CODE_1 => Some(LOWEST_U_CODE + u16::from(self.u8()?)),
            ONE_MORE_BYTE_CODE_2 => Some(LOWEST_U_CODE * 2 + u16::from(self.u8()?)),
            code => Some(u16::from(code)),
        }
    }
}

/// How a table is stored in the compressed stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Transform {
    Null,
    /// Section 5.1 "Transformed glyf table format" (version 0).
    Glyf,
    /// Rebuilt from `glyf`; stored with a zero transformLength.
    Loca,
    /// Section 5.4 "Transformed hmtx table format" (version 1).
    Hmtx,
}

/// One table directory entry.
struct Entry {
    tag: [u8; 4],
    transform: Transform,
    orig_len: u32,
    /// Where the (possibly transformed) table sits in the decompressed stream.
    src: Range<usize>,
}

/// One font of a collection: its flavor and its tables (directory indexes).
struct Font {
    flavor: u32,
    tables: Vec<usize>,
}

/// The parsed header and directories.
struct Layout {
    flavor: u32,
    entries: Vec<Entry>,
    /// `Some` for a collection (`ttcf` flavor).
    fonts: Option<Vec<Font>>,
    compressed: Range<usize>,
    /// Size of the decompressed stream (sum of the stored table lengths).
    stream_len: usize,
}

/// WOFF 2.0 → sfnt bytes (a TrueType collection, `ttcf`, for a WOFF2
/// collection).
pub(crate) fn decode(bytes: &[u8]) -> Result<Vec<u8>, FontError> {
    let layout = parse(bytes)?;
    let stream = decompress(bytes, &layout)?;
    rebuild(&layout, &stream)
}

/// Section 3 "WOFF2 Header", section 4 "Table directory" and section 4.1
/// "Collection directory", plus the reference's checks that the compressed
/// stream, metadata and private data blocks tile the rest of the file.
fn parse(bytes: &[u8]) -> Result<Layout, FontError> {
    let truncated = || bad("truncated header");
    let mut c = Cursor::new(bytes);
    if c.u32() != Some(SIGNATURE) {
        return Err(bad("bad signature"));
    }
    let flavor = c.u32().ok_or_else(truncated)?;
    let length = c.u32().ok_or_else(truncated)? as usize;
    let num_tables = c.u16().ok_or_else(truncated)?;
    c.u16().ok_or_else(truncated)?; // reserved
    let total_sfnt_size = c.u32().ok_or_else(truncated)? as usize;
    let total_compressed = c.u32().ok_or_else(truncated)? as usize;
    c.u32().ok_or_else(truncated)?; // majorVersion, minorVersion
    let meta_offset = c.u32().ok_or_else(truncated)? as usize;
    let meta_length = c.u32().ok_or_else(truncated)? as usize;
    c.u32().ok_or_else(truncated)?; // metaOrigLength
    let priv_offset = c.u32().ok_or_else(truncated)? as usize;
    let priv_length = c.u32().ok_or_else(truncated)? as usize;
    debug_assert_eq!(c.at, HEADER_LEN);
    if length != bytes.len() {
        return Err(bad("length field does not match the file size"));
    }
    if num_tables == 0 {
        return Err(bad("no tables"));
    }
    if total_sfnt_size > crate::MAX_BYTES {
        return Err(FontError::TooLarge);
    }

    let mut entries = Vec::with_capacity(usize::from(num_tables));
    let mut src = 0usize;
    for _ in 0..num_tables {
        let dir = || bad("truncated table directory");
        let flags = c.u8().ok_or_else(dir)?;
        let tag = match usize::from(flags & 0x3F) {
            63 => c.tag().ok_or_else(dir)?,
            i => KNOWN_TAGS[i],
        };
        let version = flags >> 6;
        let transform = match (&tag, version) {
            (b"glyf", 0) => Transform::Glyf,
            (b"loca", 0) => Transform::Loca,
            (b"glyf" | b"loca", 3) => Transform::Null,
            (b"hmtx", 1) => Transform::Hmtx,
            (b"glyf" | b"loca", _) => {
                return Err(bad(format!("unknown {} transform {version}", tag_str(tag))));
            }
            (_, 0) => Transform::Null,
            _ => return Err(bad(format!("unknown {} transform {version}", tag_str(tag)))),
        };
        let orig_len = c.base128().ok_or_else(dir)?;
        if orig_len as usize > crate::MAX_BYTES {
            return Err(FontError::TooLarge);
        }
        let stored = if transform == Transform::Null {
            orig_len
        } else {
            let len = c.base128().ok_or_else(dir)?;
            if transform == Transform::Loca && len != 0 {
                return Err(bad("transformed loca has data"));
            }
            len
        };
        let end = src
            .checked_add(stored as usize)
            .filter(|&end| end <= crate::MAX_BYTES)
            .ok_or(FontError::TooLarge)?;
        entries.push(Entry {
            tag,
            transform,
            orig_len,
            src: src..end,
        });
        src = end;
    }

    let fonts = if flavor == TTCF {
        let coll = || bad("truncated collection directory");
        let version = c.u32().ok_or_else(coll)?;
        if version != 0x0001_0000 && version != 0x0002_0000 {
            return Err(bad("bad collection version"));
        }
        let num_fonts = c.u255().ok_or_else(coll)?;
        if num_fonts == 0 {
            return Err(bad("collection without fonts"));
        }
        let mut fonts = Vec::with_capacity(usize::from(num_fonts));
        for _ in 0..num_fonts {
            let count = c.u255().ok_or_else(coll)?;
            if count == 0 {
                return Err(bad("collection font without tables"));
            }
            let flavor = c.u32().ok_or_else(coll)?;
            let mut tables = Vec::with_capacity(usize::from(count));
            for _ in 0..count {
                let index = usize::from(c.u255().ok_or_else(coll)?);
                if index >= entries.len() {
                    return Err(bad("collection table index out of range"));
                }
                tables.push(index);
            }
            fonts.push(Font { flavor, tables });
        }
        Some(fonts)
    } else {
        None
    };

    // The compressed stream follows the directories directly; metadata and
    // private data follow it, each 4-byte aligned, and end the file.
    let round4 = |n: usize| n.checked_add(3).map(|n| n & !3);
    let start = c.at;
    let end = start
        .checked_add(total_compressed)
        .filter(|&end| end <= length)
        .ok_or_else(|| bad("compressed data outside the file"))?;
    let mut next = round4(end).ok_or_else(|| bad("bad block layout"))?;
    for (offset, len, what) in [
        (meta_offset, meta_length, "metadata"),
        (priv_offset, priv_length, "private data"),
    ] {
        if offset == 0 {
            continue;
        }
        if offset != next || offset.checked_add(len).is_none_or(|e| e > length) {
            return Err(bad(format!("{what} block is misplaced")));
        }
        next = round4(offset + len).ok_or_else(|| bad("bad block layout"))?;
    }
    if Some(next) != round4(length) {
        return Err(bad("extra data after the last block"));
    }

    Ok(Layout {
        flavor,
        entries,
        fonts,
        compressed: start..end,
        stream_len: src,
    })
}

/// Section 5 "Compressed data format": one Brotli stream that must expand
/// to exactly the sum of the stored table lengths.
fn decompress(bytes: &[u8], layout: &Layout) -> Result<Vec<u8>, FontError> {
    let input = bytes
        .get(layout.compressed.clone())
        .ok_or_else(|| bad("compressed data outside the file"))?;
    let want = layout.stream_len;
    // A Brotli stream can expand a lot; start small and let it grow.
    let mut out = Vec::with_capacity(want.min(input.len().saturating_mul(4)));
    brotli_decompressor::Decompressor::new(input, 4096)
        .take(want as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| bad(format!("Brotli stream: {e}")))?;
    if out.len() != want {
        return Err(bad("Brotli stream has the wrong length"));
    }
    Ok(out)
}

/// What a rebuilt `glyf` tells `hmtx` and `head`.
struct GlyfInfo {
    num_glyphs: u16,
    index_format: u16,
    /// Bounding box xMin per glyph (0 for empty glyphs).
    x_mins: Vec<i16>,
}

/// Every font's tables rebuilt (shared ones once) and written out.
fn rebuild(layout: &Layout, stream: &[u8]) -> Result<Vec<u8>, FontError> {
    let entries = &layout.entries;
    let src = |i: usize| -> Result<&[u8], FontError> {
        stream
            .get(entries[i].src.clone())
            .ok_or_else(|| bad("table outside the stream"))
    };
    let single;
    let fonts: &[Font] = match &layout.fonts {
        Some(fonts) => fonts,
        None => {
            single = [Font {
                flavor: layout.flavor,
                tables: (0..entries.len()).collect(),
            }];
            &single
        }
    };

    let mut out: Vec<Option<Vec<u8>>> = entries.iter().map(|_| None).collect();
    let mut glyf_infos: Vec<Option<GlyfInfo>> = entries.iter().map(|_| None).collect();
    for font in fonts {
        let mut tags: Vec<[u8; 4]> = font.tables.iter().map(|&i| entries[i].tag).collect();
        tags.sort_unstable();
        if let Some(w) = tags.windows(2).find(|w| w[0] == w[1]) {
            return Err(bad(format!("table {} appears twice", tag_str(w[0]))));
        }
        let find = |tag: &[u8; 4]| {
            font.tables
                .iter()
                .copied()
                .find(|&i| &entries[i].tag == tag)
        };

        // glyf first: loca, hmtx and head depend on it
        let glyf = match (find(b"glyf"), find(b"loca")) {
            (Some(g), Some(l)) => {
                let transformed = entries[g].transform == Transform::Glyf;
                if transformed != (entries[l].transform == Transform::Loca) {
                    return Err(bad("glyf and loca are not transformed alike"));
                }
                if transformed {
                    if glyf_infos[g].is_none() {
                        if out[l].is_some() {
                            return Err(bad("loca shared by two glyf tables"));
                        }
                        let (glyf, loca, info) = reconstruct_glyf(src(g)?, entries[l].orig_len)?;
                        out[g] = Some(glyf);
                        out[l] = Some(loca);
                        glyf_infos[g] = Some(info);
                    } else if out[l].is_none() {
                        return Err(bad("glyf shared by two loca tables"));
                    }
                    Some(g)
                } else {
                    None
                }
            }
            (None, None) => None,
            _ => return Err(bad("glyf without loca or loca without glyf")),
        };
        let info = glyf.and_then(|g| glyf_infos[g].as_ref());

        for &i in &font.tables {
            if out[i].is_some() {
                continue;
            }
            let entry = &entries[i];
            let data = match entry.transform {
                Transform::Null => {
                    let mut data = src(i)?.to_vec();
                    if &entry.tag == b"head" {
                        let Some(adjustment) = data.get_mut(8..12) else {
                            return Err(bad("head table too short"));
                        };
                        adjustment.fill(0);
                        // indexToLocFormat must agree with the rebuilt loca
                        if let Some(info) = info
                            && let Some(format) = data.get_mut(50..52)
                        {
                            format.copy_from_slice(&info.index_format.to_be_bytes());
                        }
                    }
                    data
                }
                Transform::Hmtx => {
                    let info =
                        info.ok_or_else(|| bad("hmtx transformed without a transformed glyf"))?;
                    let hhea = find(b"hhea").ok_or_else(|| bad("hmtx transformed without hhea"))?;
                    let num_hmetrics =
                        be16(src(hhea)?, 34).ok_or_else(|| bad("hhea table too short"))?;
                    reconstruct_hmtx(src(i)?, info, num_hmetrics)?
                }
                Transform::Glyf | Transform::Loca => {
                    return Err(bad("transformed glyf or loca outside its pair"));
                }
            };
            out[i] = Some(data);
        }
    }

    // Number the tables fonts actually use; shared ones are written once.
    let mut remap = vec![usize::MAX; entries.len()];
    let mut tables = Vec::new();
    let mut size = 12usize;
    let mut font_tables = Vec::with_capacity(fonts.len());
    for font in fonts {
        size += 4 + 12 + 16 * font.tables.len();
        let mut list = Vec::with_capacity(font.tables.len());
        for &i in &font.tables {
            if remap[i] == usize::MAX {
                let data = out[i].take().ok_or_else(|| bad("table not rebuilt"))?;
                size = size.saturating_add(data.len().div_ceil(4) * 4);
                remap[i] = tables.len();
                tables.push(Table {
                    tag: entries[i].tag,
                    checksum: checksum(&data),
                    data,
                });
            }
            list.push(remap[i]);
        }
        font_tables.push(list);
    }
    if size > crate::MAX_BYTES {
        return Err(FontError::TooLarge);
    }

    let mut font = match &layout.fonts {
        Some(fonts) => {
            let flavors: Vec<u32> = fonts.iter().map(|f| f.flavor).collect();
            write_collection(&flavors, &font_tables, &tables)
        }
        None => write_sfnt(layout.flavor, &tables),
    };
    set_checksum_adjustments(&mut font).ok_or_else(|| bad("could not set head checksum"))?;
    Ok(font)
}

/// `head.checkSumAdjustment` = 0xB1B0AFBA minus the font's checksum (its
/// offset table, directory and tables, with the adjustment at 0), per font
/// of a collection.
fn set_checksum_adjustments(font: &mut [u8]) -> Option<()> {
    let dirs: Vec<usize> = if font.get(0..4)? == b"ttcf" {
        let count = be32(font, 8)? as usize;
        (0..count)
            .map(|i| Some(be32(font, 12 + 4 * i)? as usize))
            .collect::<Option<_>>()?
    } else {
        vec![0]
    };
    for dir in dirs {
        let count = usize::from(be16(font, dir + 4)?);
        let mut sum = checksum(font.get(dir..dir + 12 + 16 * count)?);
        let mut head = None;
        for e in 0..count {
            let at = dir + 12 + 16 * e;
            sum = sum.wrapping_add(be32(font, at + 4)?);
            if font.get(at..at + 4)? == b"head" {
                head = Some((be32(font, at + 8)? as usize, be32(font, at + 12)?));
            }
        }
        if let Some((offset, len)) = head
            && len >= 12
        {
            font.get_mut(offset + 8..offset + 12)?
                .copy_from_slice(&0xB1B0_AFBAu32.wrapping_sub(sum).to_be_bytes());
        }
    }
    Some(())
}

/// One point of a simple glyph, in absolute coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Point {
    x: i32,
    y: i32,
    on_curve: bool,
}

/// Section 5.2 "Decoding of variable-length X and Y coordinates": one
/// point per flag byte (bit 7 clear = on curve; the low 7 bits index the
/// 128-entry triplet encoding table, computed here as the reference does)
/// read from `data`. Returns the bytes of `data` used.
fn decode_triplets(flags: &[u8], data: &[u8], points: &mut Vec<Point>) -> Option<usize> {
    fn with_sign(flag: u8, value: i32) -> i32 {
        if flag & 1 != 0 { value } else { -value }
    }
    let mut at = 0usize;
    let (mut x, mut y) = (0i32, 0i32);
    for &flag in flags {
        let on_curve = flag & 0x80 == 0;
        let f = flag & 0x7F;
        let len = match f {
            0..84 => 1,
            84..120 => 2,
            120..124 => 3,
            _ => 4,
        };
        let mut b = [0i32; 4];
        for (to, &from) in b.iter_mut().zip(data.get(at..at.checked_add(len)?)?) {
            *to = i32::from(from);
        }
        let (dx, dy) = match f {
            0..10 => (0, with_sign(f, (i32::from(f & 14) << 7) + b[0])),
            10..20 => (with_sign(f, (i32::from((f - 10) & 14) << 7) + b[0]), 0),
            20..84 => {
                let b0 = i32::from(f - 20);
                (
                    with_sign(f, 1 + (b0 & 0x30) + (b[0] >> 4)),
                    with_sign(f >> 1, 1 + ((b0 & 0x0C) << 2) + (b[0] & 0x0F)),
                )
            }
            84..120 => {
                let b0 = i32::from(f - 84);
                (
                    with_sign(f, 1 + ((b0 / 12) << 8) + b[0]),
                    with_sign(f >> 1, 1 + (((b0 % 12) >> 2) << 8) + b[1]),
                )
            }
            120..124 => (
                with_sign(f, (b[0] << 4) + (b[1] >> 4)),
                with_sign(f >> 1, ((b[1] & 0x0F) << 8) + b[2]),
            ),
            _ => (
                with_sign(f, (b[0] << 8) + b[1]),
                with_sign(f >> 1, (b[2] << 8) + b[3]),
            ),
        };
        at += len;
        x = x.wrapping_add(dx);
        y = y.wrapping_add(dy);
        points.push(Point { x, y, on_curve });
    }
    Some(at)
}

/// Simple glyph flag bits (OpenType `glyf`).
const ON_CURVE: u8 = 0x01;
const X_SHORT: u8 = 0x02;
const Y_SHORT: u8 = 0x04;
const REPEAT: u8 = 0x08;
const X_SAME_OR_POSITIVE: u8 = 0x10;
const Y_SAME_OR_POSITIVE: u8 = 0x20;
const OVERLAP_SIMPLE: u8 = 0x40;

/// Composite glyph flag bits (OpenType `glyf`).
const ARG_1_AND_2_ARE_WORDS: u16 = 0x0001;
const WE_HAVE_A_SCALE: u16 = 0x0008;
const MORE_COMPONENTS: u16 = 0x0020;
const WE_HAVE_AN_X_AND_Y_SCALE: u16 = 0x0040;
const WE_HAVE_A_TWO_BY_TWO: u16 = 0x0080;
const WE_HAVE_INSTRUCTIONS: u16 = 0x0100;

/// A simple glyph's flags and coordinates, written like the reference
/// (`StorePoints`): repeated flags folded, one-byte deltas where they fit,
/// "same" bits for zero deltas.
fn write_points(points: &[Point], overlap: bool, out: &mut Vec<u8>) {
    fn delta(d: i32, short: u8, same: u8, flag: &mut u8, bytes: &mut Vec<u8>) {
        if d == 0 {
            *flag |= same;
        } else if (-255..=255).contains(&d) {
            *flag |= short | if d > 0 { same } else { 0 };
            bytes.push(d.unsigned_abs() as u8);
        } else {
            // out-of-range deltas only come from damaged input; truncate
            bytes.extend_from_slice(&(d as i16).to_be_bytes());
        }
    }
    let mut flags = Vec::with_capacity(points.len());
    let mut xs = Vec::with_capacity(points.len());
    let mut ys = Vec::with_capacity(points.len());
    let (mut last_x, mut last_y) = (0i32, 0i32);
    let mut last_flag = None;
    let mut repeat = 0u8;
    for (i, p) in points.iter().enumerate() {
        let mut flag = if p.on_curve { ON_CURVE } else { 0 };
        if overlap && i == 0 {
            flag |= OVERLAP_SIMPLE;
        }
        delta(
            p.x.wrapping_sub(last_x),
            X_SHORT,
            X_SAME_OR_POSITIVE,
            &mut flag,
            &mut xs,
        );
        delta(
            p.y.wrapping_sub(last_y),
            Y_SHORT,
            Y_SAME_OR_POSITIVE,
            &mut flag,
            &mut ys,
        );
        if last_flag == Some(flag) && repeat != 255 {
            if repeat == 0
                && let Some(last) = flags.last_mut()
            {
                *last |= REPEAT;
            }
            repeat += 1;
        } else {
            if repeat != 0 {
                flags.push(repeat);
            }
            flags.push(flag);
            repeat = 0;
        }
        last_flag = Some(flag);
        last_x = p.x;
        last_y = p.y;
    }
    if repeat != 0 {
        flags.push(repeat);
    }
    out.extend_from_slice(&flags);
    out.extend_from_slice(&xs);
    out.extend_from_slice(&ys);
}

/// Length of a composite glyph's component records at the start of
/// `data`, and whether any component sets WE_HAVE_INSTRUCTIONS.
fn composite_len(data: &[u8]) -> Option<(usize, bool)> {
    let mut c = Cursor::new(data);
    let mut instructions = false;
    loop {
        let flags = c.u16()?;
        instructions |= flags & WE_HAVE_INSTRUCTIONS != 0;
        let mut len = 2 + if flags & ARG_1_AND_2_ARE_WORDS != 0 {
            4
        } else {
            2
        };
        if flags & WE_HAVE_A_SCALE != 0 {
            len += 2;
        } else if flags & WE_HAVE_AN_X_AND_Y_SCALE != 0 {
            len += 4;
        } else if flags & WE_HAVE_A_TWO_BY_TWO != 0 {
            len += 8;
        }
        c.take(len)?;
        if flags & MORE_COMPONENTS == 0 {
            return Some((c.at, instructions));
        }
    }
}

fn bit(bitmap: &[u8], i: usize) -> bool {
    bitmap
        .get(i >> 3)
        .is_some_and(|b| b & (0x80 >> (i & 7)) != 0)
}

fn pad4(v: &mut Vec<u8>) {
    v.resize(v.len().div_ceil(4) * 4, 0);
}

/// Section 5.1 "Transformed glyf table format" → (`glyf`, `loca`, info).
/// Glyphs are 4-byte aligned like the reference; `loca` uses the header's
/// indexFormat and must match the directory's origLength for `loca`
/// (section 5.3 "Transformed loca table format").
fn reconstruct_glyf(data: &[u8], loca_len: u32) -> Result<(Vec<u8>, Vec<u8>, GlyfInfo), FontError> {
    let short = || bad("transformed glyf header is truncated");
    let mut h = Cursor::new(data);
    h.u16().ok_or_else(short)?; // reserved
    let option_flags = h.u16().ok_or_else(short)?;
    let num_glyphs = h.u16().ok_or_else(short)?;
    let index_format = h.u16().ok_or_else(short)?;
    if index_format > 1 {
        return Err(bad("bad glyf indexFormat"));
    }
    let n = usize::from(num_glyphs);
    let loca_width = if index_format == 0 { 2 } else { 4 };
    if loca_len as usize != loca_width * (n + 1) {
        return Err(bad("loca origLength does not match numGlyphs"));
    }
    let mut streams = [&[][..]; 7];
    let mut at = 36usize;
    for stream in &mut streams {
        let len = h.u32().ok_or_else(short)? as usize;
        let end = at
            .checked_add(len)
            .filter(|&end| end <= data.len())
            .ok_or_else(|| bad("glyf substream outside the table"))?;
        *stream = &data[at..end];
        at = end;
    }
    let overlap = if option_flags & 1 != 0 {
        data.get(at..at + n.div_ceil(8))
            .ok_or_else(|| bad("overlapSimpleBitmap outside the table"))?
    } else {
        &[]
    };
    let [
        n_contour,
        n_points,
        flags,
        glyph,
        composite,
        bbox,
        instructions,
    ] = streams;
    let (mut n_contour, mut n_points, mut flags, mut glyph, mut composite, mut instructions) = (
        Cursor::new(n_contour),
        Cursor::new(n_points),
        Cursor::new(flags),
        Cursor::new(glyph),
        Cursor::new(composite),
        Cursor::new(instructions),
    );
    let mut bbox = Cursor::new(bbox);
    let bbox_bitmap = bbox
        .take(n.div_ceil(32) * 4)
        .ok_or_else(|| bad("bboxBitmap outside the table"))?;

    let mut glyf = Vec::new();
    let mut loca = Vec::with_capacity(n + 1);
    let mut x_mins = vec![0i16; n];
    let mut points = Vec::new();
    for (i, x_min) in x_mins.iter_mut().enumerate() {
        let damaged = || bad(format!("glyph {i} is damaged"));
        loca.push(glyf.len());
        let start = glyf.len();
        let has_bbox = bit(bbox_bitmap, i);
        let contours = n_contour.i16().ok_or_else(damaged)?;
        match contours {
            0 => {
                if has_bbox {
                    return Err(bad(format!("empty glyph {i} has a bounding box")));
                }
            }
            -1 => {
                if !has_bbox {
                    return Err(bad(format!("composite glyph {i} has no bounding box")));
                }
                let (len, has_instructions) =
                    composite_len(composite.rest()).ok_or_else(damaged)?;
                glyf.extend_from_slice(&contours.to_be_bytes());
                glyf.extend_from_slice(bbox.take(8).ok_or_else(damaged)?);
                glyf.extend_from_slice(composite.take(len).ok_or_else(damaged)?);
                if has_instructions {
                    let len = glyph.u255().ok_or_else(damaged)?;
                    glyf.extend_from_slice(&len.to_be_bytes());
                    glyf.extend_from_slice(
                        instructions.take(usize::from(len)).ok_or_else(damaged)?,
                    );
                }
            }
            1.. => {
                let mut ends = Vec::with_capacity(contours as usize);
                let mut end = -1i32;
                for _ in 0..contours {
                    end += i32::from(n_points.u255().ok_or_else(damaged)?);
                    if end >= 0x1_0000 {
                        return Err(bad(format!("glyph {i} has too many points")));
                    }
                    ends.push(end as u16);
                }
                let total = (end + 1) as usize;
                let point_flags = flags.take(total).ok_or_else(damaged)?;
                points.clear();
                let used =
                    decode_triplets(point_flags, glyph.rest(), &mut points).ok_or_else(damaged)?;
                glyph.take(used).ok_or_else(damaged)?;
                let instruction_len = glyph.u255().ok_or_else(damaged)?;
                glyf.extend_from_slice(&contours.to_be_bytes());
                if has_bbox {
                    glyf.extend_from_slice(bbox.take(8).ok_or_else(damaged)?);
                } else {
                    let (mut x0, mut y0, mut x1, mut y1) = (0i32, 0i32, 0i32, 0i32);
                    if let Some(p) = points.first() {
                        (x0, y0, x1, y1) = (p.x, p.y, p.x, p.y);
                    }
                    for p in &points {
                        (x0, y0) = (x0.min(p.x), y0.min(p.y));
                        (x1, y1) = (x1.max(p.x), y1.max(p.y));
                    }
                    for v in [x0, y0, x1, y1] {
                        glyf.extend_from_slice(&(v as i16).to_be_bytes());
                    }
                }
                for end in ends {
                    glyf.extend_from_slice(&end.to_be_bytes());
                }
                glyf.extend_from_slice(&instruction_len.to_be_bytes());
                glyf.extend_from_slice(
                    instructions
                        .take(usize::from(instruction_len))
                        .ok_or_else(damaged)?,
                );
                write_points(&points, bit(overlap, i), &mut glyf);
            }
            _ => return Err(bad(format!("glyph {i} has a bad contour count"))),
        }
        if contours != 0 {
            *x_min = be16(&glyf, start + 2).ok_or_else(damaged)? as i16;
        }
        pad4(&mut glyf);
        if glyf.len() > crate::MAX_BYTES {
            return Err(FontError::TooLarge);
        }
    }
    loca.push(glyf.len());

    let mut loca_bytes = Vec::with_capacity(loca_width * (n + 1));
    for offset in loca {
        if index_format == 0 {
            let half =
                u16::try_from(offset / 2).map_err(|_| bad("glyf too large for a short loca"))?;
            loca_bytes.extend_from_slice(&half.to_be_bytes());
        } else {
            loca_bytes.extend_from_slice(&(offset as u32).to_be_bytes());
        }
    }
    let info = GlyfInfo {
        num_glyphs,
        index_format,
        x_mins,
    };
    Ok((glyf, loca_bytes, info))
}

/// Section 5.4 "Transformed hmtx table format": a flags byte, the advance
/// widths, then the lsb arrays the flags keep (bit 0 clear: proportional
/// lsbs present; bit 1 clear: monospaced lsbs present). Missing lsbs are
/// the glyphs' bounding box xMin from the rebuilt `glyf` (composite glyphs
/// included, as the spec and fontTools read it; empty glyphs get 0).
fn reconstruct_hmtx(data: &[u8], info: &GlyfInfo, num_hmetrics: u16) -> Result<Vec<u8>, FontError> {
    let short = || bad("transformed hmtx is truncated");
    let mut c = Cursor::new(data);
    let flags = c.u8().ok_or_else(short)?;
    if flags & 0xFC != 0 {
        return Err(bad("transformed hmtx has reserved flags set"));
    }
    let proportional = flags & 1 == 0;
    let monospaced = flags & 2 == 0;
    if proportional && monospaced {
        return Err(bad("transformed hmtx keeps every lsb"));
    }
    let (metrics, glyphs) = (usize::from(num_hmetrics), usize::from(info.num_glyphs));
    if metrics == 0 || metrics > glyphs {
        return Err(bad("hhea numberOfHMetrics does not fit numGlyphs"));
    }
    let mut advances = Vec::with_capacity(metrics);
    for _ in 0..metrics {
        advances.push(c.u16().ok_or_else(short)?);
    }
    let mut out = Vec::with_capacity(2 * (metrics + glyphs));
    for (i, &x_min) in info.x_mins.iter().enumerate() {
        let stored = if i < metrics {
            proportional
        } else {
            monospaced
        };
        let lsb = if stored {
            c.i16().ok_or_else(short)?
        } else {
            x_min
        };
        if let Some(advance) = advances.get(i) {
            out.extend_from_slice(&advance.to_be_bytes());
        }
        out.extend_from_slice(&lsb.to_be_bytes());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base128_reads_and_rejects() {
        let read = |b: &[u8]| Cursor::new(b).base128();
        assert_eq!(read(&[0x3F]), Some(63));
        assert_eq!(read(&[0x81, 0x00]), Some(128));
        assert_eq!(read(&[0x8F, 0xFF, 0xFF, 0xFF, 0x7F]), Some(u32::MAX));
        // leading zero, overflow, six bytes, truncated
        assert_eq!(read(&[0x80, 0x01]), None);
        assert_eq!(read(&[0x90, 0x80, 0x80, 0x80, 0x00]), None);
        assert_eq!(read(&[0x81, 0x80, 0x80, 0x80, 0x80, 0x00]), None);
        assert_eq!(read(&[0x81]), None);
    }

    #[test]
    fn u255_reads_every_code() {
        let read = |b: &[u8]| Cursor::new(b).u255();
        assert_eq!(read(&[5]), Some(5));
        assert_eq!(read(&[252]), Some(252));
        assert_eq!(read(&[253, 0x12, 0x34]), Some(0x1234));
        assert_eq!(read(&[255, 0]), Some(253));
        assert_eq!(read(&[254, 0]), Some(506));
        assert_eq!(read(&[254, 255]), Some(761));
        assert_eq!(read(&[253, 1]), None);
        assert_eq!(read(&[]), None);
    }

    #[test]
    fn triplets_follow_the_encoding_table() {
        let decode = |flags: &[u8], data: &[u8]| {
            let mut points = Vec::new();
            let used = decode_triplets(flags, data, &mut points)?;
            Some((
                used,
                points
                    .iter()
                    .map(|p| (p.x, p.y, p.on_curve))
                    .collect::<Vec<_>>(),
            ))
        };
        // index 0: dy = -b; index 3: dy = +(256 + b); index 11: dx = +b
        assert_eq!(decode(&[0], &[7]), Some((1, vec![(0, -7, true)])));
        assert_eq!(decode(&[3], &[7]), Some((1, vec![(0, 263, true)])));
        assert_eq!(decode(&[0x80 | 11], &[9]), Some((1, vec![(9, 0, false)])));
        // index 20: 4+4 bits, both negative, deltas from 1
        assert_eq!(decode(&[20], &[0x23]), Some((1, vec![(-3, -4, true)])));
        // index 90: dx = -(1 + b0), dy = +(257 + b1)
        assert_eq!(decode(&[90], &[49, 43]), Some((2, vec![(-50, 300, true)])));
        // index 123: 12+12 bits, both positive
        assert_eq!(
            decode(&[123], &[0x12, 0x34, 0x56]),
            Some((3, vec![(0x123, 0x456, true)]))
        );
        // index 127: 16+16 bits, both positive; coordinates accumulate
        assert_eq!(
            decode(&[127, 11], &[0x01, 0x00, 0x02, 0x00, 5]),
            Some((5, vec![(256, 512, true), (261, 512, true)]))
        );
        assert_eq!(decode(&[127], &[1, 2, 3]), None);
    }

    #[test]
    fn points_fold_repeated_flags() {
        let p = |x, y, on_curve| Point { x, y, on_curve };
        let mut out = Vec::new();
        write_points(
            &[
                p(10, 0, true),
                p(20, 0, true),
                p(30, 0, true),
                p(30, 400, false),
            ],
            true,
            &mut out,
        );
        assert_eq!(out, [0x73, 0x33 | REPEAT, 1, 0x10, 10, 10, 10, 0x01, 0x90]);
    }

    /// A Brotli stream of uncompressed meta-blocks (RFC 7932 section 9.2).
    fn brotli_store(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, chunk) in data.chunks(0x1_0000).enumerate() {
            // [WBITS=16 (0)] ISLAST=0, MNIBBLES=4 (00), MLEN-1 (16 bits),
            // ISUNCOMPRESSED=1, then byte aligned data
            let mut bits = 0u32;
            let mut n = 0;
            if i == 0 {
                n += 1;
            }
            n += 3;
            bits |= ((chunk.len() as u32 - 1) & 0xFFFF) << n;
            n += 16;
            bits |= 1 << n;
            n += 1;
            for b in 0..(n as usize).div_ceil(8) {
                out.push((bits >> (8 * b)) as u8);
            }
            out.extend_from_slice(chunk);
        }
        // ISLAST=1, ISLASTEMPTY=1 (after WBITS when the stream is empty)
        out.push(if data.is_empty() { 0b110 } else { 0b11 });
        out
    }

    fn base128(mut v: u32) -> Vec<u8> {
        let mut bytes = vec![(v & 0x7F) as u8];
        v >>= 7;
        while v != 0 {
            bytes.insert(0, (v & 0x7F) as u8 | 0x80);
            v >>= 7;
        }
        bytes
    }

    /// (tag, transform version, original length, stored bytes)
    type Stored = ([u8; 4], u8, u32, Vec<u8>);

    /// A WOFF2 file; `fonts` makes it a collection (flavor, table indexes).
    fn woff2(flavor: u32, tables: &[Stored], fonts: Option<&[(u32, Vec<u8>)]>) -> Vec<u8> {
        let mut dir = Vec::new();
        let mut stream = Vec::new();
        for (tag, version, orig, data) in tables {
            match KNOWN_TAGS.iter().position(|t| t == tag) {
                Some(i) => dir.push(i as u8 | version << 6),
                None => {
                    dir.push(63 | version << 6);
                    dir.extend_from_slice(tag);
                }
            }
            dir.extend(base128(*orig));
            let transformed = match tag {
                b"glyf" | b"loca" => *version == 0,
                _ => *version != 0,
            };
            if transformed {
                dir.extend(base128(data.len() as u32));
            }
            stream.extend_from_slice(data);
        }
        if let Some(fonts) = fonts {
            dir.extend_from_slice(&0x0001_0000u32.to_be_bytes());
            dir.push(fonts.len() as u8);
            for (flavor, indexes) in fonts {
                dir.push(indexes.len() as u8);
                dir.extend_from_slice(&flavor.to_be_bytes());
                dir.extend_from_slice(indexes);
            }
        }
        let compressed = brotli_store(&stream);
        let length = HEADER_LEN + dir.len() + compressed.len();
        let mut out = Vec::new();
        for word in [SIGNATURE, flavor, length as u32] {
            out.extend_from_slice(&word.to_be_bytes());
        }
        out.extend_from_slice(&(tables.len() as u16).to_be_bytes());
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&0x1000u32.to_be_bytes()); // totalSfntSize (a hint)
        out.extend_from_slice(&(compressed.len() as u32).to_be_bytes());
        out.extend_from_slice(&[0, 1, 0, 0]);
        out.extend_from_slice(&[0; 20]);
        out.extend(dir);
        out.extend(compressed);
        out
    }

    fn be(words: &[i32]) -> Vec<u8> {
        words
            .iter()
            .flat_map(|&w| (w as i16).to_be_bytes())
            .collect()
    }

    /// Three glyphs: empty, a triangle (computed bbox, two instruction
    /// bytes, overlap bit) and a composite of the triangle (explicit bbox).
    fn transformed_glyf() -> Vec<u8> {
        let streams: [Vec<u8>; 7] = [
            be(&[0, 1, -1]),
            vec![3],
            vec![11, 11, 0x80 | 90],
            vec![10, 100, 49, 43, 2],
            be(&[0x0003, 1, 10, 5]),
            [vec![0x20, 0, 0, 0], be(&[20, 5, 120, 305])].concat(),
            vec![0xB0, 0x01],
        ];
        let mut out = be(&[0, 1, 3, 0]);
        for s in &streams {
            out.extend_from_slice(&(s.len() as u32).to_be_bytes());
        }
        for s in &streams {
            out.extend_from_slice(s);
        }
        out.push(0x40); // overlapSimpleBitmap: glyph 1
        out
    }

    fn sample_tables() -> Vec<Stored> {
        let mut head = vec![0u8; 54];
        head[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        head[8..12].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]); // must be reset
        head[12..16].copy_from_slice(&0x5F0F_3CF5u32.to_be_bytes());
        head[18..20].copy_from_slice(&1000u16.to_be_bytes());
        head[50..52].copy_from_slice(&1u16.to_be_bytes()); // fixed to 0
        let mut hhea = vec![0u8; 36];
        hhea[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        hhea[34..36].copy_from_slice(&2u16.to_be_bytes());
        let maxp = [0x0000_5000u32.to_be_bytes().to_vec(), be(&[3])].concat();
        let hmtx = [vec![0x03], be(&[500, 600])].concat();
        vec![
            (*b"head", 0, 54, head),
            (*b"hhea", 0, 36, hhea),
            (*b"hmtx", 1, 10, hmtx),
            (*b"maxp", 0, 6, maxp),
            (*b"glyf", 0, 44, transformed_glyf()),
            (*b"loca", 0, 8, Vec::new()),
            (*b"zzzz", 0, 3, vec![1, 2, 3]),
        ]
    }

    /// The table `tag` of the sfnt whose directory starts at `dir`.
    fn table<'a>(font: &'a [u8], dir: usize, tag: &[u8; 4]) -> &'a [u8] {
        let count = be16(font, dir + 4).unwrap() as usize;
        let at = (0..count)
            .map(|e| dir + 12 + 16 * e)
            .find(|&at| &font[at..at + 4] == tag)
            .unwrap();
        let (offset, len) = (
            be32(font, at + 8).unwrap() as usize,
            be32(font, at + 12).unwrap() as usize,
        );
        assert_eq!(
            checksum(&font[offset..offset + len]) == be32(font, at + 4).unwrap(),
            tag != b"head"
        );
        &font[offset..offset + len]
    }

    #[test]
    fn rebuilds_transformed_glyf_loca_and_hmtx() {
        let file = woff2(0x0001_0000, &sample_tables(), None);
        let font = decode(&file).unwrap();
        assert_eq!(be32(&font, 0), Some(0x0001_0000));
        assert_eq!(be16(&font, 4), Some(7));
        assert_eq!(checksum(&font), 0xB1B0_AFBA);

        let triangle = [
            be(&[1, 10, 0, 110, 300, 2, 2]),
            vec![0xB0, 0x01, 0x73, 0x33, 0x02, 10, 100, 50, 0x01, 0x2C],
        ]
        .concat();
        let composite = [be(&[-1, 20, 5, 120, 305]), be(&[3, 1, 10, 5]), vec![0, 0]].concat();
        assert_eq!(table(&font, 0, b"glyf"), [triangle, composite].concat());
        assert_eq!(table(&font, 0, b"loca"), be(&[0, 0, 12, 22]));
        assert_eq!(table(&font, 0, b"hmtx"), be(&[500, 0, 600, 10, 20]));
        assert_eq!(table(&font, 0, b"zzzz"), [1, 2, 3]);
        let head = table(&font, 0, b"head");
        assert_eq!(be16(head, 50), Some(0));
        let mut zeroed = font.clone();
        let head_at = head.as_ptr() as usize - font.as_ptr() as usize;
        zeroed[head_at + 8..head_at + 12].fill(0);
        assert_eq!(
            0xB1B0_AFBAu32.wrapping_sub(checksum(&zeroed)),
            be32(head, 8).unwrap()
        );
    }

    #[test]
    fn rebuilds_a_collection_with_shared_tables() {
        let mut tables = sample_tables();
        tables.push((*b"name", 0, 2, vec![9, 9]));
        let fonts = [
            (0x0001_0000, vec![0, 1, 2, 3, 4, 5, 6]),
            (0x0001_0000, vec![0, 1, 2, 3, 4, 5, 7]),
        ];
        let file = woff2(TTCF, &tables, Some(&fonts));
        let font = decode(&file).unwrap();
        assert_eq!(&font[0..4], b"ttcf");
        assert_eq!(be32(&font, 8), Some(2));
        let dirs = [
            be32(&font, 12).unwrap() as usize,
            be32(&font, 16).unwrap() as usize,
        ];
        assert_eq!(table(&font, dirs[0], b"zzzz"), [1, 2, 3]);
        assert_eq!(table(&font, dirs[1], b"name"), [9, 9]);
        let glyf = |dir| table(&font, dir, b"glyf").as_ptr();
        assert_eq!(glyf(dirs[0]), glyf(dirs[1]));
        assert_eq!(table(&font, dirs[1], b"hmtx"), be(&[500, 0, 600, 10, 20]));
    }

    #[test]
    fn rejects_damaged_files_without_panicking() {
        let file = woff2(0x0001_0000, &sample_tables(), None);
        for len in 0..file.len() {
            assert!(decode(&file[..len]).is_err(), "prefix of {len} bytes");
        }
        for i in 0..file.len() {
            for flip in [0x01, 0x80, 0xFF] {
                let mut damaged = file.clone();
                damaged[i] ^= flip;
                let _ = decode(&damaged);
            }
        }
        let mut tables = sample_tables();
        tables[5].2 = 10; // loca origLength for long offsets
        assert!(decode(&woff2(0x0001_0000, &tables, None)).is_err());
        let mut tables = sample_tables();
        tables.remove(5); // glyf without loca
        assert!(decode(&woff2(0x0001_0000, &tables, None)).is_err());
    }
}
