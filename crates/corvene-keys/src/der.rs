//! A small DER / BER reader (definite and indefinite lengths) and writer
//! for the few structures re-encoded to fingerprint public keys.

use std::borrow::Cow;

/// Deepest nesting followed while measuring indefinite lengths or
/// joining constructed strings.
const MAX_DEPTH: usize = 48;

pub const SEQUENCE: u8 = 0x30;
pub const SET: u8 = 0x31;
pub const INTEGER: u8 = 0x02;
pub const BIT_STRING: u8 = 0x03;
pub const OCTET_STRING: u8 = 0x04;
pub const NULL: u8 = 0x05;
pub const OID: u8 = 0x06;
pub const BOOLEAN: u8 = 0x01;

/// One element.
#[derive(Clone, Copy, Debug)]
pub struct Tlv<'a> {
    /// First identifier byte (class, constructed bit, low tag number).
    pub id: u8,
    /// Tag number (also for high tag numbers).
    pub num: u32,
    /// Content octets (without the end-of-contents marker).
    pub content: &'a [u8],
    /// The whole element, header included.
    pub raw: &'a [u8],
}

impl<'a> Tlv<'a> {
    pub fn constructed(&self) -> bool {
        self.id & 0x20 != 0
    }

    /// Context specific tag `[n]`, either form.
    pub fn is_context(&self, n: u32) -> bool {
        self.id & 0xC0 == 0x80 && self.num == n
    }

    /// Children of a constructed element.
    pub fn children(&self) -> Reader<'a> {
        Reader::new(self.content)
    }

    /// Children collected (bounded by the input size).
    pub fn items(&self) -> Vec<Tlv<'a>> {
        self.children().collect()
    }

    /// Octets of a string type, joining BER constructed chunks.
    pub fn octets(&self) -> Cow<'a, [u8]> {
        if !self.constructed() {
            return Cow::Borrowed(self.content);
        }
        let mut out = Vec::new();
        join_chunks(self.content, 0, &mut out);
        Cow::Owned(out)
    }
}

fn join_chunks(data: &[u8], depth: usize, out: &mut Vec<u8>) {
    if depth > MAX_DEPTH {
        return;
    }
    for t in Reader::new(data) {
        if t.constructed() {
            join_chunks(t.content, depth + 1, out);
        } else {
            out.extend_from_slice(t.content);
        }
    }
}

/// Iterates elements of a byte slice; stops at the first malformed one.
#[derive(Clone)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Bytes not read yet.
    pub fn rest(&self) -> &'a [u8] {
        self.data.get(self.pos..).unwrap_or(&[])
    }
}

impl<'a> Iterator for Reader<'a> {
    type Item = Tlv<'a>;

    fn next(&mut self) -> Option<Tlv<'a>> {
        let rest = self.rest();
        if rest.len() < 2 || rest[0] == 0 && rest[1] == 0 {
            return None;
        }
        let (tlv, used) = parse(rest, 0)?;
        self.pos += used;
        Some(tlv)
    }
}

/// Parses one element at the start of `data`, returning it and the bytes
/// it used.
pub fn parse(data: &[u8], depth: usize) -> Option<(Tlv<'_>, usize)> {
    if depth > MAX_DEPTH {
        return None;
    }
    let id = *data.first()?;
    let mut pos = 1;
    let mut num = u32::from(id & 0x1F);
    if num == 0x1F {
        num = 0;
        loop {
            let b = *data.get(pos)?;
            pos += 1;
            if num > (u32::MAX >> 7) {
                return None;
            }
            num = (num << 7) | u32::from(b & 0x7F);
            if b & 0x80 == 0 {
                break;
            }
        }
    }
    let lb = *data.get(pos)?;
    pos += 1;
    if lb == 0x80 {
        if id & 0x20 == 0 {
            return None;
        }
        let start = pos;
        loop {
            let rest = data.get(pos..)?;
            if rest.len() >= 2 && rest[0] == 0 && rest[1] == 0 {
                let content = data.get(start..pos)?;
                let end = pos + 2;
                return Some((
                    Tlv {
                        id,
                        num,
                        content,
                        raw: &data[..end],
                    },
                    end,
                ));
            }
            let (_, used) = parse(rest, depth + 1)?;
            pos += used;
        }
    }
    let len = if lb & 0x80 == 0 {
        usize::from(lb)
    } else {
        let n = usize::from(lb & 0x7F);
        if n == 0 || n > 4 {
            return None;
        }
        let mut len = 0usize;
        for _ in 0..n {
            len = (len << 8) | usize::from(*data.get(pos)?);
            pos += 1;
        }
        len
    };
    let end = pos.checked_add(len)?;
    let content = data.get(pos..end)?;
    Some((
        Tlv {
            id,
            num,
            content,
            raw: &data[..end],
        },
        end,
    ))
}

/// The single element at the start of `data`.
pub fn top(data: &[u8]) -> Option<Tlv<'_>> {
    parse(data, 0).map(|(t, _)| t)
}

/// Dotted form of OID content octets.
pub fn oid_string(content: &[u8]) -> Option<String> {
    use std::fmt::Write;
    if content.is_empty() || content.last().is_some_and(|b| b & 0x80 != 0) {
        return None;
    }
    let mut out = String::with_capacity(content.len() * 3);
    let mut v: u64 = 0;
    let mut first = true;
    for &b in content {
        if v > (u64::MAX >> 8) {
            return None;
        }
        v = (v << 7) | u64::from(b & 0x7F);
        if b & 0x80 == 0 {
            if first {
                let a = (v / 40).min(2);
                let _ = write!(out, "{a}.{}", v - a * 40);
                first = false;
            } else {
                let _ = write!(out, ".{v}");
            }
            v = 0;
        }
    }
    Some(out)
}

/// OID element as a dotted string.
pub fn oid(t: &Tlv) -> Option<String> {
    (t.id == OID).then(|| oid_string(t.content)).flatten()
}

/// Bit length of an unsigned big-endian integer.
pub fn bit_len(mut bytes: &[u8]) -> usize {
    while let [0, rest @ ..] = bytes {
        bytes = rest;
    }
    match bytes.first() {
        None => 0,
        Some(b) => (bytes.len() - 1) * 8 + (8 - b.leading_zeros() as usize),
    }
}

/// Small non-negative INTEGER value.
pub fn small_int(t: &Tlv) -> Option<u64> {
    if t.id != INTEGER || t.content.is_empty() || t.content.len() > 9 {
        return None;
    }
    if t.content[0] & 0x80 != 0 {
        return None;
    }
    let mut v: u64 = 0;
    for &b in t.content {
        if v > (u64::MAX >> 8) {
            return None;
        }
        v = (v << 8) | u64::from(b);
    }
    Some(v)
}

/// Content of a BIT STRING without the unused-bits octet.
pub fn bits<'a>(t: &Tlv<'a>) -> Option<&'a [u8]> {
    if t.id != BIT_STRING {
        return None;
    }
    t.content.get(1..)
}

/// Text of a string element (UTF-8, printable, IA5, T61, BMP, universal).
pub fn string(t: &Tlv) -> Option<String> {
    let c = t.octets();
    let s = match t.num {
        12 | 19 | 22 | 26 | 27 | 18 | 25 => String::from_utf8_lossy(&c).into_owned(),
        20 => c.iter().map(|&b| char::from(b)).collect(),
        30 => {
            let units: Vec<u16> = c
                .as_chunks::<2>()
                .0
                .iter()
                .map(|p| u16::from_be_bytes(*p))
                .collect();
            String::from_utf16_lossy(&units)
        }
        28 => c
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| char::from_u32(u32::from_be_bytes(*p)).unwrap_or('\u{FFFD}'))
            .collect(),
        _ => return None,
    };
    if t.id & 0xC0 != 0 {
        return None;
    }
    Some(s)
}

/// Parses UTCTime / GeneralizedTime into unix seconds.
pub fn time(t: &Tlv) -> Option<i64> {
    let s = std::str::from_utf8(t.content).ok()?;
    let (year, rest) = match t.id {
        0x17 => {
            let yy: i64 = s.get(0..2)?.parse().ok()?;
            (if yy < 50 { 2000 + yy } else { 1900 + yy }, s.get(2..)?)
        }
        0x18 => (s.get(0..4)?.parse().ok()?, s.get(4..)?),
        _ => return None,
    };
    let num = |r: &str, a: usize| -> Option<i64> {
        let p = r.get(a..a + 2)?;
        if !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        p.parse().ok()
    };
    let month = num(rest, 0)?;
    let day = num(rest, 2)?;
    let hour = num(rest, 4)?;
    let min = num(rest, 6)?;
    let mut idx = 8;
    let sec = match num(rest, 8) {
        Some(s) => {
            idx = 10;
            s
        }
        None => 0,
    };
    let mut tail = rest.get(idx..).unwrap_or("");
    if let Some(f) = tail.strip_prefix('.') {
        tail = f.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    let offset = match tail.as_bytes().first() {
        Some(b'+') | Some(b'-') => {
            let h = num(tail, 1)?;
            let m = num(tail, 3).unwrap_or(0);
            let o = h * 3600 + m * 60;
            if tail.starts_with('-') { -o } else { o }
        }
        _ => 0,
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 24 || min > 59 || sec > 60 {
        return None;
    }
    Some(crate::util::unix_from_civil(year, month, day) + hour * 3600 + min * 60 + sec - offset)
}

/// DER writer: an element from a tag and content.
pub fn write(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let len = content.len();
    if len < 0x80 {
        out.push(len as u8);
    } else {
        let bytes = len.to_be_bytes();
        let skip = bytes.iter().take_while(|&&b| b == 0).count();
        out.push(0x80 | (bytes.len() - skip) as u8);
        out.extend_from_slice(&bytes[skip..]);
    }
    out.extend_from_slice(content);
    out
}

/// DER INTEGER for an unsigned big-endian value.
pub fn write_uint(bytes: &[u8]) -> Vec<u8> {
    let mut b = bytes;
    while b.len() > 1 && b[0] == 0 {
        b = &b[1..];
    }
    let mut c = Vec::with_capacity(b.len() + 1);
    if b.first().is_some_and(|x| x & 0x80 != 0) {
        c.push(0);
    }
    c.extend_from_slice(b);
    write(INTEGER, &c)
}

/// Dotted OID encoded as content octets.
pub fn encode_oid(dotted: &str) -> Vec<u8> {
    let parts: Vec<u64> = dotted.split('.').filter_map(|p| p.parse().ok()).collect();
    let mut out = Vec::new();
    if parts.len() < 2 {
        return out;
    }
    let mut push = |mut v: u64| {
        let mut tmp = vec![(v & 0x7F) as u8];
        v >>= 7;
        while v > 0 {
            tmp.push(0x80 | (v & 0x7F) as u8);
            v >>= 7;
        }
        tmp.reverse();
        out.extend(tmp);
    };
    push(parts[0] * 40 + parts[1]);
    for &p in &parts[2..] {
        push(p);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indefinite_lengths() {
        // SEQUENCE (indefinite) { INTEGER 5, OCTET STRING (constructed, indefinite) { "ab", "c" } }
        let data = [
            0x30, 0x80, 0x02, 0x01, 0x05, 0x24, 0x80, 0x04, 0x02, b'a', b'b', 0x04, 0x01, b'c', 0,
            0, 0, 0,
        ];
        let t = top(&data).unwrap();
        assert_eq!(t.raw.len(), data.len());
        let items = t.items();
        assert_eq!(items.len(), 2);
        assert_eq!(small_int(&items[0]), Some(5));
        assert_eq!(&*items[1].octets(), b"abc");
    }

    #[test]
    fn oids_round_trip() {
        let e = encode_oid("1.2.840.113549.1.1.11");
        assert_eq!(oid_string(&e).as_deref(), Some("1.2.840.113549.1.1.11"));
    }

    #[test]
    fn times() {
        let t = Tlv {
            id: 0x17,
            num: 23,
            content: b"491231235959Z",
            raw: &[],
        };
        assert_eq!(time(&t), Some(2524607999));
        let g = Tlv {
            id: 0x18,
            num: 24,
            content: b"19700101000000Z",
            raw: &[],
        };
        assert_eq!(time(&g), Some(0));
    }
}
