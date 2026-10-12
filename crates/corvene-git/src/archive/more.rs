//! Corvene `1356-package-archive-diff` and `1357-disk-image-diff`: more
//! archive formats for the archive diff.
//!
//! - Unix `ar` archives and Debian packages (the control archive under
//!   `DEBIAN/`, the data archive at the root);
//! - RPM packages: the payload's files plus a `.rpm-header` entry with
//!   the package's fields (name, version, dependencies, scripts), so a
//!   changed dependency is a text diff;
//! - cpio archives (newc, odc, binary), macOS installer packages and other
//!   xar archives, whose `Payload` (gzip or pbzx compressed cpio) opens as
//!   an archive of its own;
//! - 7-Zip archives;
//! - ISO 9660 disc images (Rock Ridge and Joliet names);
//! - Apple disk images: their partitions only (reading HFS+ or APFS is out
//!   of scope);
//! - Docker and OCI images saved as tar archives: the layers merged into
//!   one file tree (whiteouts applied), with the image configuration as
//!   `.image/config.json`.
//!
//! Like the other readers, nothing is written to disk and every size is
//! capped.

use std::collections::{BTreeMap, HashMap};
use std::io::{Cursor, Read};
use std::sync::Arc;

use corvene_models::{ArchiveEntryKind, ArchiveEntryMeta, ArchiveFormat};

use super::read::{Codec, Data, normalize};
use super::{MAX_ENTRIES, MAX_ENTRY_BYTES, MAX_UNPACKED};

/// Contents of formats that must be unpacked whole (7-Zip solid blocks,
/// compressed cpio) kept in memory; entries past it cannot be diffed.
const KEEP_BUDGET: u64 = 128 << 20;

/// A format this module reads, as the bytes tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Ar,
    Rpm,
    Cpio,
    SevenZip,
    Xar,
    Pbzx,
    Iso,
    Dmg,
}

/// One version, listed.
pub(crate) struct Parsed {
    pub format: ArchiveFormat,
    pub entries: Vec<(String, ArchiveEntryMeta)>,
    pub truncated: bool,
    pub comment: Option<String>,
    pub data: Data,
}

pub(crate) fn sniff(bytes: &[u8]) -> Option<Kind> {
    if bytes.starts_with(b"!<arch>\n") {
        return Some(Kind::Ar);
    }
    if bytes.starts_with(&[0xed, 0xab, 0xee, 0xdb]) {
        return Some(Kind::Rpm);
    }
    if is_cpio(bytes) {
        return Some(Kind::Cpio);
    }
    if bytes.starts_with(b"7z\xbc\xaf\x27\x1c") {
        return Some(Kind::SevenZip);
    }
    if bytes.starts_with(b"xar!") {
        return Some(Kind::Xar);
    }
    if bytes.starts_with(b"pbzx") {
        return Some(Kind::Pbzx);
    }
    if [0x8001usize, 0x8801, 0x9001]
        .iter()
        .any(|&at| bytes.get(at..at + 5) == Some(b"CD001"))
    {
        return Some(Kind::Iso);
    }
    if bytes.len() >= 512 && bytes[bytes.len() - 512..].starts_with(b"koly") {
        return Some(Kind::Dmg);
    }
    None
}

pub(crate) fn is_cpio(bytes: &[u8]) -> bool {
    bytes.starts_with(b"070701")
        || bytes.starts_with(b"070702")
        || bytes.starts_with(b"070707")
        || bytes.starts_with(&[0xc7, 0x71])
        || bytes.starts_with(&[0x71, 0xc7])
}

pub(crate) fn parse(kind: Kind, bytes: Arc<[u8]>) -> Result<Parsed, String> {
    match kind {
        Kind::Ar => ar(bytes),
        Kind::Rpm => rpm(&bytes),
        Kind::Cpio => cpio_parsed(&bytes, ArchiveFormat::Cpio),
        Kind::SevenZip => seven_zip(&bytes),
        Kind::Xar => xar(bytes),
        Kind::Pbzx => {
            let cpio = pbzx(&bytes)?;
            cpio_parsed(&cpio, ArchiveFormat::Cpio)
        }
        Kind::Iso => iso(bytes),
        Kind::Dmg => dmg(&bytes),
    }
}

fn meta(kind: ArchiveEntryKind, size: u64, crc32: u32) -> ArchiveEntryMeta {
    ArchiveEntryMeta {
        kind,
        size,
        compressed: None,
        crc32,
        mode: None,
        mtime: None,
        mtime_naive: false,
        method: None,
        owner: None,
        encrypted: false,
    }
}

fn crc(bytes: &[u8]) -> u32 {
    let mut h = crc32fast::Hasher::new();
    h.update(bytes);
    h.finalize()
}

fn sorted(mut entries: Vec<(String, ArchiveEntryMeta)>) -> Vec<(String, ArchiveEntryMeta)> {
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out: Vec<(String, ArchiveEntryMeta)> = Vec::with_capacity(entries.len());
    for e in entries {
        match out.last_mut() {
            Some(last) if last.0 == e.0 => *last = e,
            _ => out.push(e),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// ar and Debian packages
// ---------------------------------------------------------------------------

/// An `ar` member: name, header mtime, mode, offset, size.
type ArMember = (String, i64, u32, usize, usize);

/// The members of an `ar` archive.
fn ar_members(bytes: &[u8]) -> Result<Vec<ArMember>, String> {
    let mut out = Vec::new();
    let mut pos = 8;
    let mut long_names: &[u8] = &[];
    while pos + 60 <= bytes.len() {
        let h = &bytes[pos..pos + 60];
        if &h[58..60] != b"`\n" {
            return Err("Not a readable ar archive.".into());
        }
        let field = |r: std::ops::Range<usize>| String::from_utf8_lossy(&h[r]).trim().to_string();
        let size: usize = field(48..58).parse().map_err(|_| "Bad ar member size.")?;
        let mtime: i64 = field(16..28).parse().unwrap_or(0);
        let mode = u32::from_str_radix(&field(40..48), 8).unwrap_or(0o644);
        let mut start = pos + 60;
        let end = start
            .checked_add(size)
            .filter(|e| *e <= bytes.len())
            .ok_or("An ar member runs past the end.")?;
        let raw = field(0..16);
        let name = if raw == "//" {
            long_names = &bytes[start..end];
            pos = end + (end & 1);
            continue;
        } else if raw == "/"
            || raw == "/SYM64/"
            || raw == "__.SYMDEF"
            || raw.starts_with("__.SYMDEF")
        {
            pos = end + (end & 1);
            continue;
        } else if let Some(len) = raw
            .strip_prefix("#1/")
            .and_then(|l| l.parse::<usize>().ok())
        {
            // BSD: the name follows the header
            let n = String::from_utf8_lossy(bytes.get(start..start + len).unwrap_or(&[]))
                .trim_end_matches('\0')
                .to_string();
            start += len;
            n
        } else if let Some(off) = raw.strip_prefix('/').and_then(|o| o.parse::<usize>().ok()) {
            let rest = long_names.get(off..).unwrap_or(&[]);
            let endn = rest.iter().position(|b| *b == b'\n').unwrap_or(rest.len());
            String::from_utf8_lossy(&rest[..endn])
                .trim_end_matches('/')
                .to_string()
        } else {
            raw.trim_end_matches('/').to_string()
        };
        out.push((name, mtime, mode, start, end.saturating_sub(start)));
        pos = end + (end & 1);
        if out.len() > MAX_ENTRIES {
            break;
        }
    }
    Ok(out)
}

fn ar(bytes: Arc<[u8]>) -> Result<Parsed, String> {
    let members = ar_members(&bytes)?;
    if members.iter().any(|m| m.0 == "debian-binary") {
        return deb(bytes, &members);
    }
    let mut entries = Vec::new();
    let mut map = HashMap::new();
    for (name, mtime, mode, start, size) in members {
        let path = normalize(&name);
        if path.is_empty() {
            continue;
        }
        let mut m = meta(
            ArchiveEntryKind::File,
            size as u64,
            crc(&bytes[start..start + size]),
        );
        m.mtime = (mtime > 0).then_some(mtime);
        m.mode = Some(mode & 0o7777);
        map.insert(path.clone(), (start, size, Codec::Stored));
        entries.push((path, m));
    }
    Ok(Parsed {
        format: ArchiveFormat::Ar,
        entries: sorted(entries),
        truncated: false,
        comment: None,
        data: Data::Encoded { raw: bytes, map },
    })
}

fn deb(bytes: Arc<[u8]>, members: &[ArMember]) -> Result<Parsed, String> {
    let mut entries = Vec::new();
    let mut parts: Vec<(String, Data)> = Vec::new();
    let mut truncated = false;
    let mut comment = None;
    for (name, _, _, start, size) in members {
        let member: Arc<[u8]> = Arc::from(&bytes[*start..start + size]);
        if name == "debian-binary" {
            let mut m = meta(ArchiveEntryKind::File, *size as u64, crc(&member));
            m.mode = Some(0o644);
            entries.push((name.clone(), m));
            let mut files = HashMap::new();
            files.insert(name.clone(), member);
            parts.push((String::new(), Data::Mem { files, why: None }));
            continue;
        }
        let (prefix, is_tar) = if name.starts_with("control.tar") {
            ("DEBIAN/", true)
        } else if name.starts_with("data.tar") {
            ("", true)
        } else {
            ("", false)
        };
        if !is_tar {
            continue;
        }
        let (listed, data, cut) = super::read::list_tar_member(member)?;
        truncated |= cut;
        for (path, m) in listed {
            if prefix == "DEBIAN/" && path == "control" {
                // the package's name and version as the summary's comment
                if let Ok(text) = data.read_plain(&path) {
                    let text = String::from_utf8_lossy(&text);
                    let field = |k: &str| {
                        text.lines()
                            .find_map(|l| l.strip_prefix(k).map(|v| v.trim().to_string()))
                    };
                    if let (Some(p), Some(v)) = (field("Package:"), field("Version:")) {
                        comment = Some(format!("{p} {v}"));
                    }
                }
            }
            entries.push((format!("{prefix}{path}"), m));
        }
        parts.push((prefix.to_string(), data));
    }
    if entries.len() > MAX_ENTRIES {
        entries.truncate(MAX_ENTRIES);
        truncated = true;
    }
    Ok(Parsed {
        format: ArchiveFormat::Deb,
        entries: sorted(entries),
        truncated,
        comment,
        data: Data::Multi(parts),
    })
}

// ---------------------------------------------------------------------------
// cpio
// ---------------------------------------------------------------------------

struct CpioListing {
    entries: Vec<(String, ArchiveEntryMeta)>,
    files: HashMap<String, Arc<[u8]>>,
    truncated: bool,
}

fn cpio(bytes: &[u8]) -> Result<CpioListing, String> {
    let mut out = CpioListing {
        entries: Vec::new(),
        files: HashMap::new(),
        truncated: false,
    };
    let mut kept: u64 = 0;
    let mut pos = 0;
    let hex =
        |b: &[u8]| -> Option<u64> { u64::from_str_radix(std::str::from_utf8(b).ok()?, 16).ok() };
    let oct = |b: &[u8]| -> Option<u64> {
        u64::from_str_radix(std::str::from_utf8(b).ok()?.trim(), 8).ok()
    };
    while pos + 6 <= bytes.len() {
        let (mode, mtime, size, name, data_start) = if bytes[pos..].starts_with(b"070701")
            || bytes[pos..].starts_with(b"070702")
        {
            let h = bytes
                .get(pos..pos + 110)
                .ok_or("A cpio header is cut short.")?;
            let f = |i: usize| hex(&h[6 + i * 8..14 + i * 8]).unwrap_or(0);
            let (mode, mtime, size, namesize) = (f(1), f(5), f(6), f(11) as usize);
            let name_start = pos + 110;
            let name = bytes
                .get(name_start..name_start + namesize)
                .ok_or("A cpio name is cut short.")?;
            let data_start = (name_start + namesize + 3) & !3;
            (mode, mtime, size, name, data_start)
        } else if bytes[pos..].starts_with(b"070707") {
            let h = bytes
                .get(pos..pos + 76)
                .ok_or("A cpio header is cut short.")?;
            let mode = oct(&h[18..24]).unwrap_or(0);
            let mtime = oct(&h[48..59]).unwrap_or(0);
            let namesize = oct(&h[59..65]).unwrap_or(0) as usize;
            let size = oct(&h[65..76]).unwrap_or(0);
            let name_start = pos + 76;
            let name = bytes
                .get(name_start..name_start + namesize)
                .ok_or("A cpio name is cut short.")?;
            (mode, mtime, size, name, name_start + namesize)
        } else if bytes[pos..].starts_with(&[0xc7, 0x71]) || bytes[pos..].starts_with(&[0x71, 0xc7])
        {
            let le = bytes[pos] == 0xc7;
            let h = bytes
                .get(pos..pos + 26)
                .ok_or("A cpio header is cut short.")?;
            let w = |i: usize| -> u64 {
                let b = [h[i], h[i + 1]];
                u64::from(if le {
                    u16::from_le_bytes(b)
                } else {
                    u16::from_be_bytes(b)
                })
            };
            let mode = w(6);
            let mtime = (w(16) << 16) | w(18);
            let namesize = w(20) as usize;
            let size = (w(22) << 16) | w(24);
            let name_start = pos + 26;
            let name = bytes
                .get(name_start..name_start + namesize)
                .ok_or("A cpio name is cut short.")?;
            let data_start = (name_start + namesize + 1) & !1;
            (mode, mtime, size, name, data_start)
        } else {
            break;
        };
        let name = String::from_utf8_lossy(name)
            .trim_end_matches('\0')
            .to_string();
        if name == "TRAILER!!!" {
            break;
        }
        let size_us = usize::try_from(size).map_err(|_| "A cpio entry is too large.")?;
        let data_end = data_start
            .checked_add(size_us)
            .filter(|e| *e <= bytes.len())
            .ok_or("A cpio entry runs past the end.")?;
        let data = &bytes[data_start..data_end];
        let new_ascii = bytes[pos..].starts_with(b"0707010") || bytes[pos..].starts_with(b"070702");
        pos = if new_ascii {
            (data_end + 3) & !3
        } else if bytes[pos..].starts_with(b"070707") {
            data_end
        } else {
            (data_end + 1) & !1
        };
        let path = normalize(&name);
        if path.is_empty() {
            continue;
        }
        if out.entries.len() >= MAX_ENTRIES {
            out.truncated = true;
            break;
        }
        let kind = match mode & 0o170000 {
            0o040000 => ArchiveEntryKind::Dir,
            0o100000 => ArchiveEntryKind::File,
            0o120000 => ArchiveEntryKind::Symlink(String::from_utf8_lossy(data).into_owned()),
            _ => ArchiveEntryKind::Other,
        };
        let mut m = meta(
            kind.clone(),
            if kind == ArchiveEntryKind::File {
                size
            } else {
                0
            },
            if kind == ArchiveEntryKind::File {
                crc(data)
            } else {
                0
            },
        );
        m.mode = Some((mode & 0o7777) as u32);
        m.mtime = (mtime > 0).then_some(mtime as i64);
        if kind == ArchiveEntryKind::File && size <= MAX_ENTRY_BYTES && kept + size <= KEEP_BUDGET {
            kept += size;
            out.files.insert(path.clone(), Arc::from(data));
        }
        out.entries.push((path, m));
    }
    if out.entries.is_empty() && pos == 0 {
        return Err("Not a readable cpio archive.".into());
    }
    Ok(out)
}

fn cpio_parsed(bytes: &[u8], format: ArchiveFormat) -> Result<Parsed, String> {
    let listing = cpio(bytes)?;
    Ok(Parsed {
        format,
        entries: sorted(listing.entries),
        truncated: listing.truncated,
        comment: None,
        data: Data::Mem {
            files: listing.files,
            why: None,
        },
    })
}

/// A compressed cpio archive (an old installer `Payload`, an initramfs).
pub(crate) fn compressed_cpio(decoded: &[u8]) -> Result<Parsed, String> {
    cpio_parsed(decoded, ArchiveFormat::Cpio)
}

/// Decompresses a stream by its magic (gzip, bzip2, xz, zstd, raw LZMA),
/// at most `MAX_UNPACKED` bytes.
fn decompress(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let reader: Box<dyn Read + '_> = if bytes.starts_with(&[0x1f, 0x8b]) {
        Box::new(flate2::read::MultiGzDecoder::new(bytes))
    } else if bytes.starts_with(b"BZh") {
        Box::new(bzip2::read::MultiBzDecoder::new(bytes))
    } else if bytes.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0]) {
        Box::new(lzma_rust2::XzReader::new(bytes, true))
    } else if bytes.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        Box::new(ruzstd::decoding::StreamingDecoder::new(bytes).map_err(|e| e.to_string())?)
    } else if bytes.first() == Some(&0x5d) {
        Box::new(
            lzma_rust2::LzmaReader::new_mem_limit(bytes, u32::MAX, None)
                .map_err(|e| e.to_string())?,
        )
    } else {
        return Ok(bytes.to_vec());
    };
    reader
        .take(MAX_UNPACKED)
        .read_to_end(&mut out)
        .map_err(|e| format!("The payload does not unpack: {e}."))?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// RPM
// ---------------------------------------------------------------------------

struct RpmHeader {
    tags: BTreeMap<u32, RpmValue>,
    end: usize,
}

#[derive(Clone, Debug)]
enum RpmValue {
    Ints(Vec<u64>),
    Strings(Vec<String>),
    Binary,
}

fn rpm_header(bytes: &[u8], at: usize) -> Result<RpmHeader, String> {
    let h = bytes
        .get(at..at + 16)
        .ok_or("The RPM header is cut short.")?;
    if h[..3] != [0x8e, 0xad, 0xe8] {
        return Err("Not an RPM header.".into());
    }
    let be = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    let count = be(&h[8..12]) as usize;
    let size = be(&h[12..16]) as usize;
    if count > 100_000 || size > bytes.len() {
        return Err("A bad RPM header.".into());
    }
    let index = at + 16;
    let store = index + count * 16;
    let end = store
        .checked_add(size)
        .filter(|e| *e <= bytes.len())
        .ok_or("The RPM header runs past the end.")?;
    let data = &bytes[store..end];
    let mut tags = BTreeMap::new();
    for i in 0..count {
        let e = &bytes[index + i * 16..index + i * 16 + 16];
        let (tag, ty, off, n) = (
            be(&e[0..4]),
            be(&e[4..8]),
            be(&e[8..12]) as usize,
            be(&e[12..16]) as usize,
        );
        if off > data.len() || n > data.len() + 1 {
            continue;
        }
        let ints = |w: usize| -> Vec<u64> {
            (0..n)
                .filter_map(|k| data.get(off + k * w..off + k * w + w))
                .map(|b| b.iter().fold(0u64, |a, x| (a << 8) | u64::from(*x)))
                .collect()
        };
        let value = match ty {
            2 => RpmValue::Ints(ints(1)),
            3 => RpmValue::Ints(ints(2)),
            4 => RpmValue::Ints(ints(4)),
            5 => RpmValue::Ints(ints(8)),
            6 | 8 | 9 => {
                let mut strings = Vec::new();
                let mut p = off;
                let wanted = if ty == 6 { 1 } else { n };
                for _ in 0..wanted.min(100_000) {
                    let rest = &data[p.min(data.len())..];
                    let z = rest.iter().position(|b| *b == 0).unwrap_or(rest.len());
                    strings.push(String::from_utf8_lossy(&rest[..z]).into_owned());
                    p += z + 1;
                    if p > data.len() {
                        break;
                    }
                }
                RpmValue::Strings(strings)
            }
            7 => RpmValue::Binary,
            _ => continue,
        };
        tags.insert(tag, value);
    }
    Ok(RpmHeader { tags, end })
}

fn rpm_text(h: &RpmHeader) -> String {
    let s = |t: u32| -> Option<String> {
        match h.tags.get(&t)? {
            RpmValue::Strings(v) => v.first().cloned(),
            RpmValue::Ints(v) => v.first().map(|n| n.to_string()),
            RpmValue::Binary => None,
        }
    };
    let list = |t: u32| -> Vec<String> {
        match h.tags.get(&t) {
            Some(RpmValue::Strings(v)) => v.clone(),
            Some(RpmValue::Ints(v)) => v.iter().map(u64::to_string).collect(),
            _ => Vec::new(),
        }
    };
    let flags = |t: u32| -> Vec<u64> {
        match h.tags.get(&t) {
            Some(RpmValue::Ints(v)) => v.clone(),
            _ => Vec::new(),
        }
    };
    let mut out = String::new();
    for (tag, label) in [
        (1000, "Name"),
        (1003, "Epoch"),
        (1001, "Version"),
        (1002, "Release"),
        (1022, "Architecture"),
        (1021, "OS"),
        (1004, "Summary"),
        (1014, "License"),
        (1016, "Group"),
        (1020, "URL"),
        (1011, "Vendor"),
        (1015, "Packager"),
        (1010, "Distribution"),
        (1044, "Source RPM"),
        (1009, "Installed size"),
        (1124, "Payload format"),
        (1125, "Payload compressor"),
    ] {
        if let Some(v) = s(tag) {
            out.push_str(&format!("{label}: {v}\n"));
        }
    }
    if let Some(d) = s(1005) {
        out.push_str("\nDescription:\n");
        for line in d.lines() {
            out.push_str(&format!("  {line}\n"));
        }
    }
    // dependency lists with their version constraints
    for (names, versions, flag_tag, label) in [
        (1047, 1113, 1112, "Provides"),
        (1049, 1050, 1048, "Requires"),
        (1054, 1055, 1053, "Conflicts"),
        (1090, 1115, 1114, "Obsoletes"),
        (5046, 5047, 5048, "Recommends"),
        (5049, 5050, 5051, "Suggests"),
    ] {
        let n = list(names);
        if n.is_empty() {
            continue;
        }
        let v = list(versions);
        let f = flags(flag_tag);
        let mut items: Vec<String> = n
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let version = v.get(i).filter(|x| !x.is_empty());
                let op = f.get(i).map(|fl| {
                    let (lt, gt, eq) = (fl & 2 != 0, fl & 4 != 0, fl & 8 != 0);
                    match (lt, gt, eq) {
                        (true, false, true) => "<=",
                        (false, true, true) => ">=",
                        (true, false, false) => "<",
                        (false, true, false) => ">",
                        (false, false, true) => "=",
                        _ => "",
                    }
                });
                match (version, op) {
                    (Some(ver), Some(op)) if !op.is_empty() => format!("{name} {op} {ver}"),
                    _ => name.clone(),
                }
            })
            .collect();
        items.sort();
        items.dedup();
        out.push_str(&format!("\n{label}:\n"));
        for i in items {
            out.push_str(&format!("  {i}\n"));
        }
    }
    for (tag, label) in [
        (1023, "Pre-install script"),
        (1024, "Post-install script"),
        (1025, "Pre-uninstall script"),
        (1026, "Post-uninstall script"),
    ] {
        if let Some(script) = s(tag) {
            out.push_str(&format!("\n{label}:\n"));
            for line in script.lines() {
                out.push_str(&format!("  {line}\n"));
            }
        }
    }
    out
}

/// The name of the pseudo entry holding an RPM's header fields.
pub(crate) const RPM_HEADER_ENTRY: &str = ".rpm-header";

fn rpm(bytes: &[u8]) -> Result<Parsed, String> {
    if bytes.len() < 96 {
        return Err("Not an RPM package.".into());
    }
    let signature = rpm_header(bytes, 96)?;
    let main_at = (signature.end + 7) & !7;
    let header = rpm_header(bytes, main_at)?;
    let text = rpm_text(&header);
    let payload = &bytes[header.end..];
    let decoded = decompress(payload)?;
    let mut listing = cpio(&decoded)?;
    let header_bytes: Arc<[u8]> = Arc::from(text.as_bytes());
    let mut m = meta(
        ArchiveEntryKind::File,
        header_bytes.len() as u64,
        crc(&header_bytes),
    );
    m.mode = Some(0o644);
    listing.entries.push((RPM_HEADER_ENTRY.to_string(), m));
    listing
        .files
        .insert(RPM_HEADER_ENTRY.to_string(), header_bytes);
    let field = |t: u32| match header.tags.get(&t) {
        Some(RpmValue::Strings(v)) => v.first().cloned().unwrap_or_default(),
        _ => String::new(),
    };
    let comment = format!(
        "{}-{}-{}.{}",
        field(1000),
        field(1001),
        field(1002),
        field(1022)
    );
    Ok(Parsed {
        format: ArchiveFormat::Rpm,
        entries: sorted(listing.entries),
        truncated: listing.truncated,
        comment: Some(comment),
        data: Data::Mem {
            files: listing.files,
            why: None,
        },
    })
}

// ---------------------------------------------------------------------------
// xar (macOS installer packages) and pbzx
// ---------------------------------------------------------------------------

fn xar(bytes: Arc<[u8]>) -> Result<Parsed, String> {
    let be16 = |at: usize| {
        bytes
            .get(at..at + 2)
            .map(|b| u16::from_be_bytes([b[0], b[1]]) as usize)
    };
    let be64 = |at: usize| {
        bytes
            .get(at..at + 8)
            .and_then(|b| b.try_into().ok())
            .map(u64::from_be_bytes)
    };
    let header = be16(4).ok_or("Not a xar archive.")?;
    let toc_len = be64(8).ok_or("Not a xar archive.")? as usize;
    let toc_raw = bytes
        .get(header..header.checked_add(toc_len).ok_or("Bad xar header.")?)
        .ok_or("The xar table of contents is cut short.")?;
    let mut toc = String::new();
    flate2::read::ZlibDecoder::new(toc_raw)
        .take(64 << 20)
        .read_to_string(&mut toc)
        .map_err(|e| format!("The xar table of contents does not unpack: {e}."))?;
    let heap = header + toc_len;
    let doc = roxmltree::Document::parse(&toc)
        .map_err(|e| format!("The xar table of contents does not read: {e}."))?;
    let mut entries = Vec::new();
    let mut map = HashMap::new();
    let child_text = |n: roxmltree::Node<'_, '_>, tag: &str| -> Option<String> {
        n.children()
            .find(|c| c.is_element() && c.tag_name().name() == tag)
            .and_then(|c| c.text())
            .map(|t| t.trim().to_string())
    };
    let mut stack: Vec<(roxmltree::Node<'_, '_>, String)> = doc
        .descendants()
        .find(|n| n.tag_name().name() == "toc")
        .map(|toc| {
            toc.children()
                .filter(|c| c.is_element() && c.tag_name().name() == "file")
                .map(|c| (c, String::new()))
                .collect()
        })
        .unwrap_or_default();
    while let Some((file, parent)) = stack.pop() {
        if entries.len() >= MAX_ENTRIES {
            break;
        }
        let name = child_text(file, "name").unwrap_or_default();
        let path = normalize(&if parent.is_empty() {
            name
        } else {
            format!("{parent}/{name}")
        });
        if path.is_empty() {
            continue;
        }
        let ty = child_text(file, "type").unwrap_or_else(|| "file".into());
        let mode = child_text(file, "mode").and_then(|m| u32::from_str_radix(&m, 8).ok());
        let kind = match ty.as_str() {
            "directory" => ArchiveEntryKind::Dir,
            "symlink" => ArchiveEntryKind::Symlink(child_text(file, "link").unwrap_or_default()),
            "hardlink" => ArchiveEntryKind::Hardlink(String::new()),
            "file" => ArchiveEntryKind::File,
            _ => ArchiveEntryKind::Other,
        };
        let mut m = meta(kind.clone(), 0, 0);
        m.mode = mode;
        if let Some(data) = file
            .children()
            .find(|c| c.is_element() && c.tag_name().name() == "data")
        {
            let num = |tag: &str| {
                child_text(data, tag)
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0)
            };
            let (offset, length, size) =
                (num("offset") as usize, num("length") as usize, num("size"));
            let style = data
                .children()
                .find(|c| c.is_element() && c.tag_name().name() == "encoding")
                .and_then(|e| e.attribute("style"))
                .unwrap_or("application/octet-stream");
            let codec = match style {
                "application/x-gzip" => Codec::Zlib,
                "application/x-bzip2" => Codec::Bzip2,
                "application/x-lzma" | "application/x-xz" => Codec::Xz,
                _ => Codec::Stored,
            };
            m.size = size;
            // the stored bytes' checksum stands in for the contents'
            let start = heap.saturating_add(offset);
            if let Some(stored) = bytes.get(start..start.saturating_add(length)) {
                m.crc32 = crc(stored);
                m.compressed = Some(length as u64);
                m.method = Some(
                    match codec {
                        Codec::Stored => "stored",
                        Codec::Zlib => "gzip",
                        Codec::Bzip2 => "bzip2",
                        Codec::Xz => "xz",
                    }
                    .into(),
                );
                map.insert(path.clone(), (start, length, codec));
            }
        }
        for c in file
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == "file")
        {
            stack.push((c, path.clone()));
        }
        entries.push((path, m));
    }
    Ok(Parsed {
        format: ArchiveFormat::Xar,
        entries: sorted(entries),
        truncated: false,
        comment: None,
        data: Data::Encoded { raw: bytes, map },
    })
}

/// An Apple pbzx stream (a modern installer `Payload`) unpacked to the cpio
/// archive it holds.
fn pbzx(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let be64 = |at: usize| {
        bytes
            .get(at..at + 8)
            .and_then(|b| b.try_into().ok())
            .map(u64::from_be_bytes)
    };
    let mut pos = 12;
    let mut out = Vec::new();
    while pos + 16 <= bytes.len() {
        let _flags = be64(pos).ok_or("Bad pbzx chunk.")?;
        let len = be64(pos + 8).ok_or("Bad pbzx chunk.")? as usize;
        let start = pos + 16;
        let chunk = bytes
            .get(start..start.checked_add(len).ok_or("Bad pbzx chunk.")?)
            .ok_or("A pbzx chunk runs past the end.")?;
        if chunk.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0]) {
            lzma_rust2::XzReader::new(chunk, true)
                .take(MAX_UNPACKED - out.len() as u64)
                .read_to_end(&mut out)
                .map_err(|e| format!("A pbzx chunk does not unpack: {e}."))?;
        } else {
            out.extend_from_slice(chunk);
        }
        if out.len() as u64 >= MAX_UNPACKED {
            break;
        }
        pos = start + len;
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// 7-Zip
// ---------------------------------------------------------------------------

fn seven_zip(bytes: &[u8]) -> Result<Parsed, String> {
    let mut reader =
        sevenz_rust2::ArchiveReader::new(Cursor::new(bytes), sevenz_rust2::Password::empty())
            .map_err(|e| format!("Not a readable 7-Zip archive: {e}."))?;
    let mut entries = Vec::new();
    for f in reader.archive().files.iter().take(MAX_ENTRIES) {
        let path = normalize(&f.name);
        if path.is_empty() || f.is_anti_item {
            continue;
        }
        let kind = if f.is_directory {
            ArchiveEntryKind::Dir
        } else {
            ArchiveEntryKind::File
        };
        let mut m = meta(kind, f.size, if f.has_crc { f.crc as u32 } else { 0 });
        if f.has_last_modified_date {
            let ticks: u64 = f.last_modified_date.into();
            m.mtime = Some((ticks / 10_000_000) as i64 - 11_644_473_600);
        }
        if f.has_windows_attributes && f.windows_attributes & 0x8000 != 0 {
            m.mode = Some((f.windows_attributes >> 16) & 0o7777);
        }
        entries.push((path, m));
    }
    let truncated = reader.archive().files.len() > MAX_ENTRIES;
    // solid blocks only unpack in order: keep what fits
    let mut files = HashMap::new();
    let mut kept: u64 = 0;
    let mut why = None;
    let result = reader.for_each_entries(|entry, data| {
        if entry.is_directory || !entry.has_stream {
            return Ok(true);
        }
        let mut buf = Vec::new();
        if entry.size <= MAX_ENTRY_BYTES && kept + entry.size <= KEEP_BUDGET {
            data.take(MAX_ENTRY_BYTES).read_to_end(&mut buf)?;
            kept += buf.len() as u64;
            files.insert(normalize(&entry.name), Arc::from(buf));
        } else {
            std::io::copy(data, &mut std::io::sink())?;
        }
        Ok(kept < KEEP_BUDGET)
    });
    if let Err(e) = result {
        why = Some(format!("The archive's contents do not unpack: {e}."));
    } else if kept >= KEEP_BUDGET {
        why = Some(super::read::too_large());
    }
    Ok(Parsed {
        format: ArchiveFormat::SevenZip,
        entries: sorted(entries),
        truncated,
        comment: None,
        data: Data::Mem { files, why },
    })
}

// ---------------------------------------------------------------------------
// ISO 9660
// ---------------------------------------------------------------------------

struct IsoDir {
    lba: u64,
    len: u64,
}

fn iso(bytes: Arc<[u8]>) -> Result<Parsed, String> {
    const SECTOR: usize = 2048;
    let mut pvd: Option<usize> = None;
    let mut joliet: Option<usize> = None;
    for i in 16..64 {
        let at = i * SECTOR;
        let Some(vd) = bytes.get(at..at + SECTOR) else {
            break;
        };
        if &vd[1..6] != b"CD001" {
            break;
        }
        match vd[0] {
            1 if pvd.is_none() => pvd = Some(at),
            2 if matches!(&vd[88..91], b"%/@" | b"%/C" | b"%/E") => joliet = Some(at),
            255 => break,
            _ => {}
        }
    }
    let pvd = pvd.ok_or("Not an ISO 9660 image (no primary volume descriptor).")?;
    let block = u16::from_le_bytes([bytes[pvd + 128], bytes[pvd + 129]]) as usize;
    let block = if block == 0 { SECTOR } else { block };
    let volume = String::from_utf8_lossy(&bytes[pvd + 40..pvd + 72])
        .trim_matches(|c: char| c == '\0' || c.is_whitespace())
        .to_string();
    let root_of = |vd: usize| -> IsoDir {
        let r = &bytes[vd + 156..vd + 190];
        IsoDir {
            lba: u64::from(u32::from_le_bytes([r[2], r[3], r[4], r[5]])),
            len: u64::from(u32::from_le_bytes([r[10], r[11], r[12], r[13]])),
        }
    };
    // Rock Ridge names (`NM` entries) on the primary tree beat Joliet's;
    // some writers add Rock Ridge without names
    let has_entry = |rec: &[u8], sig: &[u8; 2]| -> bool {
        let name_len = rec.get(32).copied().unwrap_or(0) as usize;
        let mut p = 33 + name_len + ((name_len + 1) & 1);
        while p + 4 <= rec.len() {
            let l = rec[p + 2] as usize;
            if l < 4 {
                break;
            }
            if &rec[p..p + 2] == sig {
                return true;
            }
            p += l;
        }
        false
    };
    let rock_ridge = {
        let root = root_of(pvd);
        let at = root.lba as usize * block;
        let sector = bytes
            .get(at..at + block.min(root.len as usize))
            .unwrap_or(&[]);
        let mut p = 0;
        let mut names = false;
        let mut first = true;
        let mut sp = false;
        while p < sector.len() && sector[p] != 0 {
            let l = sector[p] as usize;
            let rec = &sector[p..(p + l).min(sector.len())];
            if first {
                sp = has_entry(rec, b"SP");
                first = false;
            } else if has_entry(rec, b"NM") {
                names = true;
                break;
            }
            p += l;
        }
        sp && (names || joliet.is_none())
    };
    let (start, ucs2) = match (rock_ridge, joliet) {
        (false, Some(j)) => (root_of(j), true),
        _ => (root_of(pvd), false),
    };
    let mut entries = Vec::new();
    let mut map = HashMap::new();
    let mut stack: Vec<(IsoDir, String, usize)> = vec![(start, String::new(), 0)];
    let mut seen = std::collections::HashSet::new();
    while let Some((dir, parent, depth)) = stack.pop() {
        if depth > 64 || !seen.insert(dir.lba) {
            continue;
        }
        let begin = dir.lba as usize * block;
        let end = begin.saturating_add(dir.len as usize).min(bytes.len());
        let mut pos = begin;
        while pos < end {
            let len = bytes[pos] as usize;
            if len == 0 {
                // records do not cross sectors: skip to the next one
                pos = (pos / SECTOR + 1) * SECTOR;
                continue;
            }
            let Some(rec) = bytes.get(pos..pos + len) else {
                break;
            };
            pos += len;
            if rec.len() < 34 {
                continue;
            }
            let lba = u64::from(u32::from_le_bytes([rec[2], rec[3], rec[4], rec[5]]));
            let size = u64::from(u32::from_le_bytes([rec[10], rec[11], rec[12], rec[13]]));
            let flags = rec[25];
            let name_len = rec[32] as usize;
            let Some(raw_name) = rec.get(33..33 + name_len) else {
                continue;
            };
            if raw_name == [0] || raw_name == [1] {
                continue;
            }
            let mut name = if ucs2 {
                let units: Vec<u16> = raw_name
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_be_bytes(*c))
                    .collect();
                String::from_utf16_lossy(&units)
            } else {
                String::from_utf8_lossy(raw_name).into_owned()
            };
            let mut mode = None;
            let mut link = None;
            if rock_ridge {
                let su = 33 + name_len + ((name_len + 1) & 1);
                let mut p = su;
                let mut rr_name = String::new();
                while p + 4 <= rec.len() {
                    let sig = &rec[p..p + 2];
                    let l = rec[p + 2] as usize;
                    if l < 4 || p + l > rec.len() {
                        break;
                    }
                    match sig {
                        b"NM" => rr_name.push_str(&String::from_utf8_lossy(&rec[p + 5..p + l])),
                        b"PX" if l >= 12 => {
                            mode = Some(u32::from_le_bytes([
                                rec[p + 4],
                                rec[p + 5],
                                rec[p + 6],
                                rec[p + 7],
                            ]))
                        }
                        b"SL" => {
                            let mut q = p + 5;
                            let mut target = String::new();
                            while q + 2 <= p + l {
                                let (cflags, clen) = (rec[q], rec[q + 1] as usize);
                                let part = String::from_utf8_lossy(
                                    rec.get(q + 2..q + 2 + clen).unwrap_or(&[]),
                                );
                                if !target.is_empty() && !target.ends_with('/') {
                                    target.push('/');
                                }
                                match cflags & 0x0e {
                                    0x02 => target.push('.'),
                                    0x04 => target.push_str(".."),
                                    0x08 => target.push('/'),
                                    _ => target.push_str(&part),
                                }
                                q += 2 + clen;
                            }
                            link = Some(target);
                        }
                        _ => {}
                    }
                    p += l;
                }
                if !rr_name.is_empty() {
                    name = rr_name;
                }
            }
            if !rock_ridge {
                if let Some(semi) = name.find(';') {
                    name.truncate(semi);
                }
                if name.ends_with('.') {
                    name.pop();
                }
            }
            let path = normalize(&if parent.is_empty() {
                name
            } else {
                format!("{parent}/{name}")
            });
            if path.is_empty() {
                continue;
            }
            if entries.len() >= MAX_ENTRIES {
                break;
            }
            let date = &rec[18..25];
            let mtime = (date[0] > 0).then(|| {
                let days = super::read::civil_days(
                    1900 + i64::from(date[0]),
                    i64::from(date[1]),
                    i64::from(date[2]),
                );
                let offset = i64::from(date[6] as i8) * 15 * 60;
                days * 86_400
                    + i64::from(date[3]) * 3600
                    + i64::from(date[4]) * 60
                    + i64::from(date[5])
                    - offset
            });
            let is_dir = flags & 0x02 != 0;
            let kind = match (is_dir, link) {
                (true, _) => ArchiveEntryKind::Dir,
                (false, Some(target)) => ArchiveEntryKind::Symlink(target),
                (false, None) => ArchiveEntryKind::File,
            };
            let start_at = lba as usize * block;
            let mut m = meta(
                kind.clone(),
                if kind == ArchiveEntryKind::File {
                    size
                } else {
                    0
                },
                0,
            );
            m.mtime = mtime;
            m.mode = mode.map(|m| m & 0o7777);
            if kind == ArchiveEntryKind::File {
                let data = bytes
                    .get(start_at..start_at.saturating_add(size as usize))
                    .unwrap_or(&[]);
                m.crc32 = crc(data);
                map.insert(path.clone(), (start_at, data.len(), Codec::Stored));
            }
            if is_dir {
                stack.push((IsoDir { lba, len: size }, path.clone(), depth + 1));
            }
            entries.push((path, m));
        }
    }
    Ok(Parsed {
        format: ArchiveFormat::Iso,
        entries: sorted(entries),
        truncated: false,
        comment: (!volume.is_empty()).then_some(volume),
        data: Data::Encoded { raw: bytes, map },
    })
}

// ---------------------------------------------------------------------------
// Apple disk images
// ---------------------------------------------------------------------------

fn dmg(bytes: &[u8]) -> Result<Parsed, String> {
    let koly = &bytes[bytes.len() - 512..];
    let be64 = |at: usize| u64::from_be_bytes(koly[at..at + 8].try_into().unwrap_or_default());
    let (plist_at, plist_len) = (be64(0xd8) as usize, be64(0xe0) as usize);
    let xml = bytes
        .get(
            plist_at
                ..plist_at
                    .checked_add(plist_len)
                    .ok_or("Bad disk image trailer.")?,
        )
        .ok_or("The disk image's property list lies outside the file.")?;
    let value = plist::Value::from_reader(Cursor::new(xml))
        .map_err(|e| format!("The disk image's property list does not read: {e}."))?;
    let blkx = value
        .as_dictionary()
        .and_then(|d| d.get("resource-fork"))
        .and_then(|r| r.as_dictionary())
        .and_then(|r| r.get("blkx"))
        .and_then(|b| b.as_array())
        .ok_or("The disk image lists no partitions.")?;
    let mut entries = Vec::new();
    let mut methods = std::collections::BTreeSet::new();
    for (ix, part) in blkx.iter().enumerate() {
        let Some(d) = part.as_dictionary() else {
            continue;
        };
        let name = d
            .get("CFName")
            .or_else(|| d.get("Name"))
            .and_then(|v| v.as_string())
            .unwrap_or("partition")
            .replace('/', "∕");
        let Some(mish) = d.get("Data").and_then(|v| v.as_data()) else {
            continue;
        };
        if mish.len() < 0xcc || &mish[..4] != b"mish" {
            continue;
        }
        let be64m = |at: usize| u64::from_be_bytes(mish[at..at + 8].try_into().unwrap_or_default());
        let be32m = |at: usize| u32::from_be_bytes(mish[at..at + 4].try_into().unwrap_or_default());
        let sectors = be64m(0x10);
        let checksum = if be32m(0x40) == 2 {
            be32m(0x48)
        } else {
            crc(mish)
        };
        let chunks = be32m(0xc8) as usize;
        let mut kind_bytes: BTreeMap<&str, u64> = BTreeMap::new();
        for c in 0..chunks.min((mish.len() - 0xcc) / 40) {
            let at = 0xcc + c * 40;
            let ty = be32m(at);
            let method = match ty {
                0x8000_0005 => "zlib",
                0x8000_0006 => "bzip2",
                0x8000_0007 => "lzfse",
                0x8000_0008 => "lzma",
                0x8000_0004 => "adc",
                0x0000_0001 => "raw",
                _ => continue,
            };
            *kind_bytes.entry(method).or_default() += be64m(at + 32);
        }
        let method = kind_bytes
            .iter()
            .max_by_key(|(_, n)| **n)
            .map(|(m, _)| m.to_string());
        if let Some(m) = &method {
            methods.insert(m.clone());
        }
        let mut m = meta(ArchiveEntryKind::File, sectors * 512, checksum);
        m.method = method;
        entries.push((format!("{ix:02} {name}"), m));
    }
    if entries.is_empty() {
        return Err("The disk image lists no partitions.".into());
    }
    let comment = format!(
        "Apple disk image ({})",
        if methods.is_empty() {
            "uncompressed".to_string()
        } else {
            methods.into_iter().collect::<Vec<_>>().join(", ")
        }
    );
    Ok(Parsed {
        format: ArchiveFormat::Dmg,
        entries: sorted(entries),
        truncated: false,
        comment: Some(comment),
        data: Data::Mem {
            files: HashMap::new(),
            why: Some(
                "Corvene lists a disk image's partitions; it does not read the files inside them."
                    .into(),
            ),
        },
    })
}

// ---------------------------------------------------------------------------
// Docker and OCI images
// ---------------------------------------------------------------------------

/// Whether a tar listing is a saved container image.
pub(crate) fn is_image(entries: &[(String, ArchiveEntryMeta)]) -> bool {
    let has = |p: &str| entries.binary_search_by(|(e, _)| e.as_str().cmp(p)).is_ok();
    has("manifest.json") || (has("oci-layout") && has("index.json"))
}

/// A saved image's layers merged; `read` reads an entry of the outer tar.
/// Reads an entry of the outer tar archive.
pub(crate) type ReadEntry<'a> = &'a dyn Fn(&str) -> Result<Arc<[u8]>, String>;

pub(crate) fn image(read: ReadEntry<'_>) -> Result<Parsed, String> {
    let json = |p: &str| -> Result<serde_json::Value, String> {
        let b = read(p)?;
        serde_json::from_slice(&b).map_err(|e| format!("{p} does not read: {e}."))
    };
    let blob = |digest: &str| digest.replacen(':', "/", 1);
    let (layers, config, tags): (Vec<String>, Option<String>, Vec<String>) =
        match json("manifest.json") {
            Ok(manifest) => {
                let first = manifest.get(0).ok_or("The image manifest is empty.")?;
                let layers = first
                    .get("Layers")
                    .and_then(|l| l.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                let config = first
                    .get("Config")
                    .and_then(|c| c.as_str())
                    .map(str::to_string);
                let tags = first
                    .get("RepoTags")
                    .and_then(|t| t.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                (layers, config, tags)
            }
            Err(_) => {
                let index = json("index.json")?;
                let digest = index
                    .get("manifests")
                    .and_then(|m| m.get(0))
                    .and_then(|m| m.get("digest"))
                    .and_then(|d| d.as_str())
                    .ok_or("The image index lists no manifest.")?;
                let manifest = json(&format!("blobs/{}", blob(digest)))?;
                let layers = manifest
                    .get("layers")
                    .and_then(|l| l.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|l| l.get("digest").and_then(|d| d.as_str()))
                            .map(|d| format!("blobs/{}", blob(d)))
                            .collect()
                    })
                    .unwrap_or_default();
                let config = manifest
                    .get("config")
                    .and_then(|c| c.get("digest"))
                    .and_then(|d| d.as_str())
                    .map(|d| format!("blobs/{}", blob(d)));
                let tags = index
                    .get("manifests")
                    .and_then(|m| m.get(0))
                    .and_then(|m| m.get("annotations"))
                    .and_then(|a| a.get("org.opencontainers.image.ref.name"))
                    .and_then(|n| n.as_str())
                    .map(|n| vec![n.to_string()])
                    .unwrap_or_default();
                (layers, config, tags)
            }
        };
    let mut merged: BTreeMap<String, ArchiveEntryMeta> = BTreeMap::new();
    let mut files: HashMap<String, Arc<[u8]>> = HashMap::new();
    let mut truncated = false;
    for layer in &layers {
        let bytes = read(layer)?;
        let (listed, data, cut) = super::read::list_tar_member(bytes)?;
        truncated |= cut;
        for (path, m) in listed {
            let (dir, base) = match path.rsplit_once('/') {
                Some((d, b)) => (format!("{d}/"), b),
                None => (String::new(), path.as_str()),
            };
            if base == ".wh..wh..opq" {
                let gone: Vec<String> = merged
                    .range(dir.clone()..)
                    .take_while(|(k, _)| k.starts_with(&dir))
                    .map(|(k, _)| k.clone())
                    .collect();
                for k in gone {
                    merged.remove(&k);
                    files.remove(&k);
                }
                continue;
            }
            if let Some(name) = base.strip_prefix(".wh.") {
                let target = format!("{dir}{name}");
                let prefix = format!("{target}/");
                let gone: Vec<String> = merged
                    .range(target.clone()..)
                    .take_while(|(k, _)| **k == target || k.starts_with(&prefix))
                    .map(|(k, _)| k.clone())
                    .collect();
                for k in gone {
                    merged.remove(&k);
                    files.remove(&k);
                }
                continue;
            }
            if m.kind == ArchiveEntryKind::File {
                match data.read_plain(&path) {
                    Ok(b) => {
                        files.insert(path.clone(), b);
                    }
                    Err(_) => {
                        files.remove(&path);
                    }
                }
            }
            merged.insert(path, m);
            if merged.len() > MAX_ENTRIES {
                truncated = true;
                break;
            }
        }
    }
    if let Some(config) = config
        && let Ok(bytes) = read(&config)
    {
        let pretty = serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()
            .and_then(|v| serde_json::to_string_pretty(&v).ok())
            .map(|s| Arc::from(format!("{s}\n").as_bytes()))
            .unwrap_or(bytes);
        let path = ".image/config.json".to_string();
        merged.insert(
            path.clone(),
            meta(ArchiveEntryKind::File, pretty.len() as u64, crc(&pretty)),
        );
        files.insert(path, pretty);
    }
    let comment = if tags.is_empty() {
        format!("{} layers", layers.len())
    } else {
        format!("{}, {} layers", tags.join(", "), layers.len())
    };
    Ok(Parsed {
        format: ArchiveFormat::OciImage,
        entries: merged.into_iter().collect(),
        truncated,
        comment: Some(comment),
        data: Data::Mem {
            files,
            why: Some(super::read::too_large()),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ar_archive(members: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = b"!<arch>\n".to_vec();
        for (name, data) in members {
            out.extend(
                format!(
                    "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
                    format!("{name}/"),
                    1_700_000_000,
                    0,
                    0,
                    "100644",
                    data.len()
                )
                .as_bytes(),
            );
            out.extend(*data);
            if data.len() % 2 == 1 {
                out.push(b'\n');
            }
        }
        out
    }

    fn tar_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for (name, data) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_mtime(1_700_000_000);
            h.set_cksum();
            b.append_data(&mut h, name, *data).unwrap();
        }
        b.into_inner().unwrap()
    }

    fn gz(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut e, data).unwrap();
        e.finish().unwrap()
    }

    fn newc(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut push = |name: &str, data: &[u8], mode: u32| {
            let header = format!(
                "070701{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}",
                1,
                mode,
                0,
                0,
                1,
                1_700_000_000u32,
                data.len(),
                0,
                0,
                0,
                0,
                name.len() + 1,
                0
            );
            out.extend(header.as_bytes());
            out.extend(name.as_bytes());
            out.push(0);
            while out.len() % 4 != 0 {
                out.push(0);
            }
            out.extend(data);
            while out.len() % 4 != 0 {
                out.push(0);
            }
        };
        for (name, data) in files {
            push(name, data, 0o100644);
        }
        push("TRAILER!!!", b"", 0);
        out
    }

    #[test]
    fn debian_packages() {
        let control = gz(&tar_of(&[(
            "./control",
            b"Package: hello\nVersion: 2.10-1\n",
        )]));
        let data = gz(&tar_of(&[
            ("./usr/bin/hello", b"\x7fELF..."),
            ("./usr/share/doc/hello/copyright", b"GPL"),
        ]));
        let deb = ar_archive(&[
            ("debian-binary", b"2.0\n"),
            ("control.tar.gz", &control),
            ("data.tar.gz", &data),
        ]);
        let p = parse(sniff(&deb).unwrap(), Arc::from(deb)).unwrap();
        assert_eq!(p.format, ArchiveFormat::Deb);
        let names: Vec<&str> = p.entries.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "DEBIAN/control",
                "debian-binary",
                "usr/bin/hello",
                "usr/share/doc/hello/copyright"
            ]
        );
        assert_eq!(p.comment.as_deref(), Some("hello 2.10-1"));
        assert_eq!(
            &*p.data.read_plain("usr/share/doc/hello/copyright").unwrap(),
            b"GPL"
        );
        assert_eq!(
            &*p.data.read_plain("DEBIAN/control").unwrap(),
            b"Package: hello\nVersion: 2.10-1\n"
        );
    }

    #[test]
    fn cpio_archives() {
        let c = newc(&[("./a.txt", b"hello"), ("b/c.txt", b"x")]);
        let p = parse(sniff(&c).unwrap(), Arc::from(c)).unwrap();
        let names: Vec<&str> = p.entries.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["a.txt", "b/c.txt"]);
        assert_eq!(&*p.data.read_plain("a.txt").unwrap(), b"hello");
        assert_eq!(p.entries[0].1.mode, Some(0o644));
    }

    #[test]
    fn rpm_packages() {
        // a minimal header: Name, Version, Release, Arch, Requires
        fn header(tags: &[(u32, u32, Vec<u8>, u32)]) -> Vec<u8> {
            let mut index = Vec::new();
            let mut store: Vec<u8> = Vec::new();
            for (tag, ty, data, count) in tags {
                index.extend(tag.to_be_bytes());
                index.extend(ty.to_be_bytes());
                index.extend((store.len() as u32).to_be_bytes());
                index.extend(count.to_be_bytes());
                store.extend(data);
            }
            let mut out = vec![0x8e, 0xad, 0xe8, 0x01, 0, 0, 0, 0];
            out.extend((tags.len() as u32).to_be_bytes());
            out.extend((store.len() as u32).to_be_bytes());
            out.extend(index);
            out.extend(store);
            out
        }
        let s = |v: &str| format!("{v}\0").into_bytes();
        let mut rpm = vec![0xed, 0xab, 0xee, 0xdb];
        rpm.resize(96, 0);
        let sig = header(&[]);
        rpm.extend(&sig);
        while rpm.len() % 8 != 0 {
            rpm.push(0);
        }
        rpm.extend(header(&[
            (1000, 6, s("hello"), 1),
            (1001, 6, s("2.10"), 1),
            (1002, 6, s("1.fc40"), 1),
            (1022, 6, s("x86_64"), 1),
            (1049, 8, [s("glibc"), s("bash")].concat(), 2),
            (1050, 8, [s("2.34"), s("")].concat(), 2),
            (
                1048,
                4,
                [12u32.to_be_bytes(), 0u32.to_be_bytes()].concat(),
                2,
            ),
        ]));
        rpm.extend(gz(&newc(&[("./usr/bin/hello", b"bin")])));
        let p = parse(sniff(&rpm).unwrap(), Arc::from(rpm)).unwrap();
        assert_eq!(p.format, ArchiveFormat::Rpm);
        assert_eq!(p.comment.as_deref(), Some("hello-2.10-1.fc40.x86_64"));
        let header_text =
            String::from_utf8(p.data.read_plain(RPM_HEADER_ENTRY).unwrap().to_vec()).unwrap();
        assert!(header_text.contains("Name: hello\n"), "{header_text}");
        assert!(
            header_text.contains("Requires:\n  bash\n  glibc >= 2.34\n"),
            "{header_text}"
        );
        assert!(p.entries.iter().any(|(n, _)| n == "usr/bin/hello"));
    }

    #[test]
    fn iso_images() {
        // one file `HELLO.TXT;1` in the root of a minimal image
        const S: usize = 2048;
        let mut img = vec![0u8; S * 20];
        let record = |lba: u32, len: u32, flags: u8, name: &[u8]| -> Vec<u8> {
            let mut r = vec![0u8; 33];
            r[2..6].copy_from_slice(&lba.to_le_bytes());
            r[10..14].copy_from_slice(&len.to_le_bytes());
            r[18] = 124; // 2024
            r[19] = 1;
            r[20] = 2;
            r[25] = flags;
            r[32] = name.len() as u8;
            r.extend(name);
            if r.len() % 2 == 1 {
                r.push(0);
            }
            r[0] = r.len() as u8;
            r
        };
        let pvd = 16 * S;
        img[pvd] = 1;
        img[pvd + 1..pvd + 6].copy_from_slice(b"CD001");
        img[pvd + 40..pvd + 45].copy_from_slice(b"DISC1");
        img[pvd + 128..pvd + 130].copy_from_slice(&2048u16.to_le_bytes());
        let root = record(18, S as u32, 2, &[0]);
        img[pvd + 156..pvd + 156 + root.len()].copy_from_slice(&root);
        let term = 17 * S;
        img[term] = 255;
        img[term + 1..term + 6].copy_from_slice(b"CD001");
        let mut dir = Vec::new();
        dir.extend(record(18, S as u32, 2, &[0]));
        dir.extend(record(18, S as u32, 2, &[1]));
        dir.extend(record(19, 5, 0, b"HELLO.TXT;1"));
        img[18 * S..18 * S + dir.len()].copy_from_slice(&dir);
        img[19 * S..19 * S + 5].copy_from_slice(b"hello");
        let p = parse(sniff(&img).unwrap(), Arc::from(img)).unwrap();
        assert_eq!(p.format, ArchiveFormat::Iso);
        assert_eq!(p.comment.as_deref(), Some("DISC1"));
        assert_eq!(p.entries.len(), 1);
        assert_eq!(p.entries[0].0, "HELLO.TXT");
        assert_eq!(&*p.data.read_plain("HELLO.TXT").unwrap(), b"hello");
    }

    #[test]
    fn container_images_merge_layers() {
        let layer1 = tar_of(&[
            ("etc/a.conf", b"one"),
            ("etc/gone.conf", b"x"),
            ("opt/data/x", b"1"),
        ]);
        let layer2 = gz(&tar_of(&[
            ("etc/a.conf", b"two"),
            ("etc/.wh.gone.conf", b""),
            ("opt/data/.wh..wh..opq", b""),
            ("opt/data/y", b"2"),
        ]));
        let manifest = br#"[{"Config":"cfg.json","RepoTags":["app:1"],"Layers":["l1/layer.tar","l2/layer.tar"]}]"#;
        let mut files: HashMap<String, Arc<[u8]>> = HashMap::new();
        files.insert("manifest.json".into(), Arc::from(&manifest[..]));
        files.insert("l1/layer.tar".into(), Arc::from(layer1));
        files.insert("l2/layer.tar".into(), Arc::from(layer2));
        files.insert(
            "cfg.json".into(),
            Arc::from(&br#"{"config":{"Env":["A=1"]}}"#[..]),
        );
        let read = |p: &str| files.get(p).cloned().ok_or_else(|| "missing".to_string());
        let p = image(&read).unwrap();
        let names: Vec<&str> = p.entries.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec![".image/config.json", "etc/a.conf", "opt/data/y"]
        );
        assert_eq!(&*p.data.read_plain("etc/a.conf").unwrap(), b"two");
        assert_eq!(p.comment.as_deref(), Some("app:1, 2 layers"));
    }
}
