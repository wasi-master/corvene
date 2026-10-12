//! Decoders for the schemaless binary formats: MessagePack, CBOR, BSON,
//! bencode and raw protocol buffers. Each refuses input it does not fully
//! understand rather than printing a guess.

use std::fmt::Write as _;

use super::MAX_DEPTH;

/// A decoded value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Node {
    Null,
    Undefined,
    Bool(bool),
    /// Integers of any size, as decimal text.
    Int(String),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    Array(Vec<Node>),
    Map(Vec<(Node, Node)>),
    /// A value of a type JSON lacks (`ObjectId("…")`, `tag 1(…)`).
    Typed(String, Box<Node>),
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Cursor { data, pos: 0 }
    }
    fn done(&self) -> bool {
        self.pos >= self.data.len()
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|e| *e <= self.data.len())
            .ok_or("The data ends early.")?;
        let out = &self.data[self.pos..end];
        self.pos = end;
        Ok(out)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn be(&mut self, n: usize) -> Result<u64, String> {
        Ok(self
            .take(n)?
            .iter()
            .fold(0u64, |acc, b| (acc << 8) | u64::from(*b)))
    }
    fn le(&mut self, n: usize) -> Result<u64, String> {
        Ok(self
            .take(n)?
            .iter()
            .rev()
            .fold(0u64, |acc, b| (acc << 8) | u64::from(*b)))
    }
    /// A length that fits in what is left (a hostile length is refused
    /// before anything is allocated for it).
    fn len(&self, n: u64) -> Result<usize, String> {
        usize::try_from(n)
            .ok()
            .filter(|n| *n <= self.data.len() - self.pos.min(self.data.len()))
            .ok_or_else(|| "A length is larger than the data.".to_string())
    }
    /// A count of items, each at least one byte.
    fn count(&self, n: u64) -> Result<usize, String> {
        self.len(n)
    }
}

fn text(bytes: &[u8]) -> Node {
    match std::str::from_utf8(bytes) {
        Ok(s) => Node::Str(s.to_string()),
        Err(_) => Node::Bytes(bytes.to_vec()),
    }
}

fn deep(depth: usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        Err("The data nests too deeply.".into())
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// MessagePack
// ---------------------------------------------------------------------------

/// Every MessagePack value in `bytes` (a stream of them is common).
pub(crate) fn msgpack_all(bytes: &[u8]) -> Result<Vec<Node>, String> {
    let mut c = Cursor::new(bytes);
    let mut out = Vec::new();
    while !c.done() {
        out.push(msgpack(&mut c, 0)?);
        if out.len() > 100_000 {
            break;
        }
    }
    if out.is_empty() {
        return Err("The file is empty.".into());
    }
    Ok(out)
}

fn msgpack(c: &mut Cursor<'_>, depth: usize) -> Result<Node, String> {
    deep(depth)?;
    let b = c.u8()?;
    let array = |c: &mut Cursor<'_>, n: u64| -> Result<Node, String> {
        let n = c.count(n)?;
        let mut items = Vec::with_capacity(n.min(4096));
        for _ in 0..n {
            items.push(msgpack(c, depth + 1)?);
        }
        Ok(Node::Array(items))
    };
    let map = |c: &mut Cursor<'_>, n: u64| -> Result<Node, String> {
        let n = c.count(n.saturating_mul(2))? / 2;
        let mut pairs = Vec::with_capacity(n.min(4096));
        for _ in 0..n {
            let k = msgpack(c, depth + 1)?;
            let v = msgpack(c, depth + 1)?;
            pairs.push((k, v));
        }
        Ok(Node::Map(pairs))
    };
    let ext = |c: &mut Cursor<'_>, n: usize| -> Result<Node, String> {
        let ty = c.u8()? as i8;
        let data = c.take(n)?;
        // the timestamp extension
        if ty == -1 {
            let (secs, nanos) = match data.len() {
                4 => (
                    i64::from(u32::from_be_bytes(data.try_into().unwrap_or_default())),
                    0,
                ),
                8 => {
                    let v = u64::from_be_bytes(data.try_into().unwrap_or_default());
                    ((v & 0x3_ffff_ffff) as i64, (v >> 34) as u32)
                }
                12 => (
                    i64::from_be_bytes(data[4..].try_into().unwrap_or_default()),
                    u32::from_be_bytes(data[..4].try_into().unwrap_or_default()),
                ),
                _ => (0, 0),
            };
            let mut stamp = crate::compiled::pyc::utc(secs);
            if nanos > 0 {
                stamp = stamp.replace(" UTC", &format!(".{nanos:09} UTC"));
            }
            return Ok(Node::Typed("timestamp".into(), Box::new(Node::Str(stamp))));
        }
        Ok(Node::Typed(
            format!("ext {ty}"),
            Box::new(Node::Bytes(data.to_vec())),
        ))
    };
    Ok(match b {
        0x00..=0x7f => Node::Int(b.to_string()),
        0x80..=0x8f => map(c, u64::from(b & 0x0f))?,
        0x90..=0x9f => array(c, u64::from(b & 0x0f))?,
        0xa0..=0xbf => {
            let n = (b & 0x1f) as usize;
            text(c.take(n)?)
        }
        0xc0 => Node::Null,
        0xc2 => Node::Bool(false),
        0xc3 => Node::Bool(true),
        0xc4..=0xc6 => {
            let n = c.be(1 << (b - 0xc4))?;
            let n = c.len(n)?;
            Node::Bytes(c.take(n)?.to_vec())
        }
        0xc7..=0xc9 => {
            let n = c.be(1 << (b - 0xc7))?;
            let n = c.len(n)?;
            ext(c, n)?
        }
        0xca => Node::Float(f64::from(f32::from_bits(c.be(4)? as u32))),
        0xcb => Node::Float(f64::from_bits(c.be(8)?)),
        0xcc..=0xcf => Node::Int(c.be(1 << (b - 0xcc))?.to_string()),
        0xd0 => Node::Int((c.be(1)? as u8 as i8).to_string()),
        0xd1 => Node::Int((c.be(2)? as u16 as i16).to_string()),
        0xd2 => Node::Int((c.be(4)? as u32 as i32).to_string()),
        0xd3 => Node::Int((c.be(8)? as i64).to_string()),
        0xd4..=0xd8 => ext(c, 1 << (b - 0xd4))?,
        0xd9..=0xdb => {
            let n = c.be(1 << (b - 0xd9))?;
            let n = c.len(n)?;
            text(c.take(n)?)
        }
        0xdc | 0xdd => {
            let n = c.be(if b == 0xdc { 2 } else { 4 })?;
            array(c, n)?
        }
        0xde | 0xdf => {
            let n = c.be(if b == 0xde { 2 } else { 4 })?;
            map(c, n)?
        }
        0xe0..=0xff => Node::Int((b as i8).to_string()),
        0xc1 => return Err("Not MessagePack (byte 0xc1).".into()),
    })
}

// ---------------------------------------------------------------------------
// CBOR
// ---------------------------------------------------------------------------

/// Every CBOR item in `bytes` (a sequence, RFC 8742, is allowed).
pub(crate) fn cbor_all(bytes: &[u8]) -> Result<Vec<Node>, String> {
    let mut c = Cursor::new(bytes);
    // the self-describe tag 55799 is a magic number, not data
    let mut out = Vec::new();
    while !c.done() {
        match cbor(&mut c, 0)? {
            Some(node) => out.push(node),
            None => return Err("Not CBOR (a stray break).".into()),
        }
        if out.len() > 100_000 {
            break;
        }
    }
    if out.is_empty() {
        return Err("The file is empty.".into());
    }
    Ok(out)
}

/// One item; `None` for the "break" that ends an indefinite container.
fn cbor(c: &mut Cursor<'_>, depth: usize) -> Result<Option<Node>, String> {
    deep(depth)?;
    let b = c.u8()?;
    let major = b >> 5;
    let info = b & 0x1f;
    if b == 0xff {
        return Ok(None);
    }
    let arg = |c: &mut Cursor<'_>| -> Result<Option<u64>, String> {
        Ok(match info {
            0..=23 => Some(u64::from(info)),
            24 => Some(c.be(1)?),
            25 => Some(c.be(2)?),
            26 => Some(c.be(4)?),
            27 => Some(c.be(8)?),
            31 => None,
            _ => return Err("Not CBOR (reserved length).".into()),
        })
    };
    let chunks = |c: &mut Cursor<'_>, major: u8| -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        loop {
            let b = c.u8()?;
            if b == 0xff {
                return Ok(out);
            }
            if b >> 5 != major {
                return Err("Not CBOR (mixed string chunks).".into());
            }
            let n = match b & 0x1f {
                n @ 0..=23 => u64::from(n),
                24 => c.be(1)?,
                25 => c.be(2)?,
                26 => c.be(4)?,
                27 => c.be(8)?,
                _ => return Err("Not CBOR (nested indefinite string).".into()),
            };
            let n = c.len(n)?;
            out.extend_from_slice(c.take(n)?);
        }
    };
    let node = match major {
        0 => Node::Int(arg(c)?.ok_or("Not CBOR.")?.to_string()),
        1 => {
            let v = arg(c)?.ok_or("Not CBOR.")?;
            Node::Int((-1i128 - i128::from(v)).to_string())
        }
        2 => match arg(c)? {
            Some(n) => {
                let n = c.len(n)?;
                Node::Bytes(c.take(n)?.to_vec())
            }
            None => Node::Bytes(chunks(c, 2)?),
        },
        3 => match arg(c)? {
            Some(n) => {
                let n = c.len(n)?;
                text(c.take(n)?)
            }
            None => text(&chunks(c, 3)?),
        },
        4 => {
            let mut items = Vec::new();
            match arg(c)? {
                Some(n) => {
                    for _ in 0..c.count(n)? {
                        items.push(cbor(c, depth + 1)?.ok_or("Not CBOR (break).")?);
                    }
                }
                None => {
                    while let Some(item) = cbor(c, depth + 1)? {
                        items.push(item);
                    }
                }
            }
            Node::Array(items)
        }
        5 => {
            let mut pairs = Vec::new();
            match arg(c)? {
                Some(n) => {
                    for _ in 0..c.count(n.saturating_mul(2))? / 2 {
                        let k = cbor(c, depth + 1)?.ok_or("Not CBOR (break).")?;
                        let v = cbor(c, depth + 1)?.ok_or("Not CBOR (break).")?;
                        pairs.push((k, v));
                    }
                }
                None => {
                    while let Some(k) = cbor(c, depth + 1)? {
                        let v = cbor(c, depth + 1)?.ok_or("Not CBOR (break).")?;
                        pairs.push((k, v));
                    }
                }
            }
            Node::Map(pairs)
        }
        6 => {
            let tag = arg(c)?.ok_or("Not CBOR.")?;
            let inner = cbor(c, depth + 1)?.ok_or("Not CBOR (break).")?;
            if tag == 55799 {
                inner
            } else {
                let name = match tag {
                    0 => "datetime".to_string(),
                    1 => "epoch".to_string(),
                    2 => "bignum".to_string(),
                    3 => "negative bignum".to_string(),
                    32 => "uri".to_string(),
                    37 => "uuid".to_string(),
                    t => format!("tag {t}"),
                };
                Node::Typed(name, Box::new(inner))
            }
        }
        _ => match info {
            20 => Node::Bool(false),
            21 => Node::Bool(true),
            22 => Node::Null,
            23 => Node::Undefined,
            24 => Node::Typed("simple".into(), Box::new(Node::Int(c.be(1)?.to_string()))),
            25 => Node::Float(half(c.be(2)? as u16)),
            26 => Node::Float(f64::from(f32::from_bits(c.be(4)? as u32))),
            27 => Node::Float(f64::from_bits(c.be(8)?)),
            n @ 0..=19 => Node::Typed("simple".into(), Box::new(Node::Int(n.to_string()))),
            _ => return Err("Not CBOR (reserved simple value).".into()),
        },
    };
    Ok(Some(node))
}

/// An IEEE 754 half-precision float.
fn half(h: u16) -> f64 {
    let exp = (h >> 10) & 0x1f;
    let mant = f64::from(h & 0x3ff);
    let v = match exp {
        0 => mant * 2f64.powi(-24),
        31 if mant == 0.0 => f64::INFINITY,
        31 => f64::NAN,
        e => (mant + 1024.0) * 2f64.powi(i32::from(e) - 25),
    };
    if h & 0x8000 != 0 { -v } else { v }
}

// ---------------------------------------------------------------------------
// BSON
// ---------------------------------------------------------------------------

/// Every BSON document in `bytes` (a `mongodump` file holds many).
pub(crate) fn bson_all(bytes: &[u8]) -> Result<Vec<Node>, String> {
    let mut c = Cursor::new(bytes);
    let mut out = Vec::new();
    while !c.done() {
        out.push(bson_doc(&mut c, false, 0)?);
        if out.len() > 100_000 {
            break;
        }
    }
    if out.is_empty() {
        return Err("The file is empty.".into());
    }
    Ok(out)
}

fn cstring(c: &mut Cursor<'_>) -> Result<String, String> {
    let rest = &c.data[c.pos..];
    let end = rest
        .iter()
        .position(|b| *b == 0)
        .ok_or("Not BSON (unterminated name).")?;
    let s = String::from_utf8_lossy(&rest[..end]).into_owned();
    c.pos += end + 1;
    Ok(s)
}

fn bson_doc(c: &mut Cursor<'_>, array: bool, depth: usize) -> Result<Node, String> {
    deep(depth)?;
    let start = c.pos;
    let size = c.le(4)? as usize;
    if size < 5 || start + size > c.data.len() {
        return Err("Not BSON (bad document size).".into());
    }
    let end = start + size;
    let mut pairs = Vec::new();
    loop {
        let ty = c.u8()?;
        if ty == 0 {
            break;
        }
        let name = cstring(c)?;
        let string = |c: &mut Cursor<'_>| -> Result<String, String> {
            let n = c.le(4)? as usize;
            if n == 0 {
                return Err("Not BSON (bad string).".into());
            }
            let n = c.len(n as u64)?;
            let b = c.take(n)?;
            Ok(String::from_utf8_lossy(&b[..n - 1]).into_owned())
        };
        let value = match ty {
            0x01 => Node::Float(f64::from_bits(c.le(8)?)),
            0x02 => Node::Str(string(c)?),
            0x03 => bson_doc(c, false, depth + 1)?,
            0x04 => bson_doc(c, true, depth + 1)?,
            0x05 => {
                let n = c.le(4)?;
                let n = c.len(n)?;
                let sub = c.u8()?;
                let data = c.take(n)?;
                if sub == 4 && data.len() == 16 {
                    Node::Typed("UUID".into(), Box::new(Node::Str(uuid(data))))
                } else if sub == 0 {
                    Node::Bytes(data.to_vec())
                } else {
                    Node::Typed(
                        format!("Binary {sub}"),
                        Box::new(Node::Bytes(data.to_vec())),
                    )
                }
            }
            0x06 => Node::Undefined,
            0x07 => Node::Typed(
                "ObjectId".into(),
                Box::new(Node::Str(crate::out::hex(c.take(12)?))),
            ),
            0x08 => Node::Bool(c.u8()? != 0),
            0x09 => {
                let ms = c.le(8)? as i64;
                let stamp = crate::compiled::pyc::utc(ms.div_euclid(1000));
                let frac = ms.rem_euclid(1000);
                let stamp = if frac == 0 {
                    stamp
                } else {
                    stamp.replace(" UTC", &format!(".{frac:03} UTC"))
                };
                Node::Typed("Date".into(), Box::new(Node::Str(stamp)))
            }
            0x0a => Node::Null,
            0x0b => {
                let pattern = cstring(c)?;
                let flags = cstring(c)?;
                Node::Typed(
                    "Regex".into(),
                    Box::new(Node::Str(format!("/{pattern}/{flags}"))),
                )
            }
            0x0c => {
                let ns = string(c)?;
                let id = crate::out::hex(c.take(12)?);
                Node::Typed(
                    "DBPointer".into(),
                    Box::new(Node::Str(format!("{ns} {id}"))),
                )
            }
            0x0d => Node::Typed("Code".into(), Box::new(Node::Str(string(c)?))),
            0x0e => Node::Typed("Symbol".into(), Box::new(Node::Str(string(c)?))),
            0x0f => {
                c.le(4)?;
                let code = string(c)?;
                let scope = bson_doc(c, false, depth + 1)?;
                Node::Typed(
                    "CodeWithScope".into(),
                    Box::new(Node::Array(vec![Node::Str(code), scope])),
                )
            }
            0x10 => Node::Int((c.le(4)? as u32 as i32).to_string()),
            0x11 => {
                let v = c.le(8)?;
                Node::Typed(
                    "Timestamp".into(),
                    Box::new(Node::Str(format!("{}, {}", v >> 32, v & 0xffff_ffff))),
                )
            }
            0x12 => Node::Int((c.le(8)? as i64).to_string()),
            0x13 => Node::Typed(
                "Decimal128".into(),
                Box::new(Node::Bytes(c.take(16)?.to_vec())),
            ),
            0xff => Node::Typed("MinKey".into(), Box::new(Node::Null)),
            0x7f => Node::Typed("MaxKey".into(), Box::new(Node::Null)),
            t => return Err(format!("Not BSON (unknown type 0x{t:02x}).")),
        };
        pairs.push((Node::Str(name), value));
        if c.pos > end {
            return Err("Not BSON (a value overruns its document).".into());
        }
    }
    if c.pos != end {
        return Err("Not BSON (document size mismatch).".into());
    }
    Ok(if array {
        Node::Array(pairs.into_iter().map(|(_, v)| v).collect())
    } else {
        Node::Map(pairs)
    })
}

fn uuid(b: &[u8]) -> String {
    let h = crate::out::hex(b);
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

// ---------------------------------------------------------------------------
// bencode
// ---------------------------------------------------------------------------

/// A bencoded value (a `.torrent` file).
pub(crate) fn bencode(bytes: &[u8]) -> Result<Node, String> {
    let mut c = Cursor::new(bytes);
    let node = bvalue(&mut c, 0)?;
    // trailing newlines are tolerated, anything else is not bencode
    if c.data[c.pos..].iter().any(|b| !b.is_ascii_whitespace()) {
        return Err("Not bencode (trailing data).".into());
    }
    Ok(node)
}

fn bvalue(c: &mut Cursor<'_>, depth: usize) -> Result<Node, String> {
    deep(depth)?;
    match c.data.get(c.pos).copied().ok_or("Not bencode.")? {
        b'i' => {
            c.pos += 1;
            let rest = &c.data[c.pos..];
            let end = rest.iter().position(|b| *b == b'e').ok_or("Not bencode.")?;
            let digits = std::str::from_utf8(&rest[..end]).map_err(|_| "Not bencode.")?;
            if digits.is_empty()
                || !digits
                    .trim_start_matches('-')
                    .bytes()
                    .all(|b| b.is_ascii_digit())
            {
                return Err("Not bencode (bad integer).".into());
            }
            c.pos += end + 1;
            Ok(Node::Int(digits.to_string()))
        }
        b'l' => {
            c.pos += 1;
            let mut items = Vec::new();
            while c.data.get(c.pos) != Some(&b'e') {
                items.push(bvalue(c, depth + 1)?);
            }
            c.pos += 1;
            Ok(Node::Array(items))
        }
        b'd' => {
            c.pos += 1;
            let mut pairs = Vec::new();
            while c.data.get(c.pos) != Some(&b'e') {
                let key = bvalue(c, depth + 1)?;
                let value = bvalue(c, depth + 1)?;
                // a torrent's piece hashes are one long binary string
                let value = match (&key, value) {
                    (Node::Str(k), Node::Bytes(b)) if k == "pieces" && b.len() % 20 == 0 => {
                        Node::Typed(
                            format!("{} SHA-1 hashes", b.len() / 20),
                            Box::new(Node::Bytes(b)),
                        )
                    }
                    (_, v) => v,
                };
                pairs.push((key, value));
            }
            c.pos += 1;
            Ok(Node::Map(pairs))
        }
        b'0'..=b'9' => {
            let rest = &c.data[c.pos..];
            let colon = rest.iter().position(|b| *b == b':').ok_or("Not bencode.")?;
            let n: u64 = std::str::from_utf8(&rest[..colon])
                .ok()
                .and_then(|s| s.parse().ok())
                .ok_or("Not bencode (bad length).")?;
            c.pos += colon + 1;
            let n = c.len(n)?;
            Ok(text(c.take(n)?))
        }
        _ => Err("Not bencode.".into()),
    }
}

// ---------------------------------------------------------------------------
// protocol buffers, without a schema
// ---------------------------------------------------------------------------

/// A protocol buffer message as `protoc --decode_raw` prints it.
pub(crate) fn protobuf_text(bytes: &[u8]) -> Result<String, String> {
    let fields = proto_message(bytes, 0).ok_or("Not a protocol buffer message.")?;
    let mut out = String::new();
    proto_print(&mut out, &fields, 0);
    Ok(out)
}

enum ProtoValue {
    Varint(u64),
    Fixed64(u64),
    Fixed32(u32),
    Message(Vec<(u64, ProtoValue)>),
    Bytes(Vec<u8>),
}

fn varint(c: &mut Cursor<'_>) -> Option<u64> {
    let mut v: u64 = 0;
    for shift in (0..70).step_by(7) {
        let b = c.u8().ok()?;
        if shift == 63 && b > 1 {
            return None;
        }
        v |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

fn proto_message(bytes: &[u8], depth: usize) -> Option<Vec<(u64, ProtoValue)>> {
    if depth > MAX_DEPTH {
        return None;
    }
    let mut c = Cursor::new(bytes);
    let mut out = Vec::new();
    while !c.done() {
        let key = varint(&mut c)?;
        let field = key >> 3;
        if field == 0 || field > 536_870_911 {
            return None;
        }
        let value = match key & 7 {
            0 => ProtoValue::Varint(varint(&mut c)?),
            1 => ProtoValue::Fixed64(c.le(8).ok()?),
            2 => {
                let n = varint(&mut c)?;
                let n = c.len(n).ok()?;
                let data = c.take(n).ok()?;
                // a nested message when the bytes parse as one and are not
                // better read as text
                let as_text = std::str::from_utf8(data).ok().filter(|s| {
                    s.chars()
                        .all(|ch| !ch.is_control() || ch == '\n' || ch == '\t')
                });
                match (as_text, proto_message(data, depth + 1)) {
                    (Some(_), _) if data.is_empty() => ProtoValue::Bytes(Vec::new()),
                    (None, Some(fields)) => ProtoValue::Message(fields),
                    (Some(_), _) => ProtoValue::Bytes(data.to_vec()),
                    (None, None) => ProtoValue::Bytes(data.to_vec()),
                }
            }
            5 => ProtoValue::Fixed32(c.le(4).ok()? as u32),
            _ => return None,
        };
        out.push((field, value));
    }
    Some(out)
}

fn proto_print(out: &mut String, fields: &[(u64, ProtoValue)], depth: usize) {
    let pad = "  ".repeat(depth);
    for (field, value) in fields {
        match value {
            ProtoValue::Varint(v) => {
                let _ = writeln!(out, "{pad}{field}: {v}");
            }
            ProtoValue::Fixed64(v) => {
                let _ = writeln!(out, "{pad}{field}: 0x{v:016x}");
            }
            ProtoValue::Fixed32(v) => {
                let _ = writeln!(out, "{pad}{field}: 0x{v:08x}");
            }
            ProtoValue::Bytes(b) => {
                let _ = writeln!(out, "{pad}{field}: {}", crate::out::quote(b));
            }
            ProtoValue::Message(inner) => {
                let _ = writeln!(out, "{pad}{field} {{");
                proto_print(out, inner, depth + 1);
                let _ = writeln!(out, "{pad}}}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msgpack_values() {
        // {"a": 1, "b": [true, nil], "c": -1}
        let bytes = [
            0x83, 0xa1, b'a', 0x01, 0xa1, b'b', 0x92, 0xc3, 0xc0, 0xa1, b'c', 0xff,
        ];
        let nodes = msgpack_all(&bytes).unwrap();
        assert_eq!(
            nodes,
            vec![Node::Map(vec![
                (Node::Str("a".into()), Node::Int("1".into())),
                (
                    Node::Str("b".into()),
                    Node::Array(vec![Node::Bool(true), Node::Null])
                ),
                (Node::Str("c".into()), Node::Int("-1".into())),
            ])]
        );
        // a length past the end is refused before allocating
        assert!(msgpack_all(&[0xdd, 0xff, 0xff, 0xff, 0xff]).is_err());
    }

    #[test]
    fn cbor_values() {
        // {"a": [1, -2, 1.5], "t": 1("x")}
        let bytes = [
            0xa2, 0x61, b'a', 0x83, 0x01, 0x21, 0xf9, 0x3e, 0x00, 0x61, b't', 0xc1, 0x61, b'x',
        ];
        let nodes = cbor_all(&bytes).unwrap();
        assert_eq!(
            nodes,
            vec![Node::Map(vec![
                (
                    Node::Str("a".into()),
                    Node::Array(vec![
                        Node::Int("1".into()),
                        Node::Int("-2".into()),
                        Node::Float(1.5)
                    ])
                ),
                (
                    Node::Str("t".into()),
                    Node::Typed("epoch".into(), Box::new(Node::Str("x".into())))
                ),
            ])]
        );
        // indefinite array
        assert_eq!(
            cbor_all(&[0x9f, 0x01, 0xff]).unwrap(),
            vec![Node::Array(vec![Node::Int("1".into())])]
        );
    }

    #[test]
    fn bson_documents() {
        // {"x": 5, "s": "hi"}
        let mut doc = Vec::new();
        doc.extend([0x10, b'x', 0]);
        doc.extend(5i32.to_le_bytes());
        doc.extend([0x02, b's', 0]);
        doc.extend(3i32.to_le_bytes());
        doc.extend(b"hi\0");
        doc.push(0);
        let mut bytes = ((doc.len() + 4) as i32).to_le_bytes().to_vec();
        bytes.extend(doc);
        let two = [bytes.clone(), bytes].concat();
        let nodes = bson_all(&two).unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(
            nodes[0],
            Node::Map(vec![
                (Node::Str("x".into()), Node::Int("5".into())),
                (Node::Str("s".into()), Node::Str("hi".into())),
            ])
        );
        assert!(bson_all(b"\x05\0\0").is_err());
    }

    #[test]
    fn bencode_torrent_like() {
        let mut file = b"d4:infod4:name3:abc6:pieces20:".to_vec();
        file.extend([0xff; 20]);
        file.extend(b"ee");
        let node = bencode(&file).unwrap();
        let Node::Map(pairs) = node else { panic!() };
        let Node::Map(info) = &pairs[0].1 else {
            panic!()
        };
        assert_eq!(info[0], (Node::Str("name".into()), Node::Str("abc".into())));
        assert!(matches!(&info[1].1, Node::Typed(name, _) if name == "1 SHA-1 hashes"));
        assert!(bencode(b"i12x").is_err());
    }

    #[test]
    fn protobuf_raw() {
        // field 1 varint 150, field 2 { field 1 "hi" }
        let bytes = [0x08, 0x96, 0x01, 0x12, 0x04, 0x0a, 0x02, b'h', b'i'];
        let text = protobuf_text(&bytes).unwrap();
        assert_eq!(text, "1: 150\n2 {\n  1: \"hi\"\n}\n");
        assert!(protobuf_text(b"\x07").is_err());
    }

    #[test]
    fn half_floats() {
        assert_eq!(half(0x3c00), 1.0);
        assert_eq!(half(0xc000), -2.0);
        assert!(half(0x7c00).is_infinite());
    }
}
