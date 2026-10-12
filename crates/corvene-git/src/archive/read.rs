//! Reading one version of an archive into memory: its entry list, and the
//! contents of an entry on demand. Nothing is ever written to disk, entry
//! names are only compared and shown, and every size is capped (see the
//! limits in [`super`]) so a zip bomb costs at most the caps.

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::sync::Arc;

use corvene_models::{ArchiveEntryKind, ArchiveEntryMeta, ArchiveFormat};

use super::{MAX_ENTRIES, MAX_ENTRY_BYTES, MAX_UNPACKED, archive_format};

/// Entry contents a tar archive keeps after listing, so an entry diff does
/// not stream the archive again (beyond it entries are read by re-streaming).
const KEEP_BUDGET: u64 = 128 << 20;
/// Longest symbolic link target read from a zip entry.
const MAX_LINK: u64 = 4096;

/// One version of an archive, listed.
pub(crate) struct OpenArchive {
    pub format: ArchiveFormat,
    /// The archive's own size.
    pub bytes: u64,
    /// Sorted by path; a path stored twice keeps its last entry.
    pub entries: Vec<(String, ArchiveEntryMeta)>,
    pub truncated: bool,
    pub comment: Option<String>,
    /// A single compressed file: the name of the file inside.
    pub single: Option<String>,
    data: Data,
}

enum Data {
    Zip {
        archive: zip::ZipArchive<Cursor<Arc<[u8]>>>,
        index: HashMap<String, usize>,
    },
    Tar {
        raw: Arc<[u8]>,
        compressor: Option<Compressor>,
        kept: HashMap<String, Arc<[u8]>>,
    },
    Single {
        contents: Option<Arc<[u8]>>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Compressor {
    Gzip,
    Bzip2,
    Xz,
    Zstd,
}

impl Compressor {
    fn sniff(bytes: &[u8]) -> Option<Compressor> {
        if bytes.starts_with(&[0x1f, 0x8b]) {
            Some(Compressor::Gzip)
        } else if bytes.starts_with(b"BZh") {
            Some(Compressor::Bzip2)
        } else if bytes.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0]) {
            Some(Compressor::Xz)
        } else if bytes.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
            Some(Compressor::Zstd)
        } else {
            None
        }
    }

    fn decoder<'a>(self, bytes: &'a [u8]) -> Result<Box<dyn Read + 'a>, String> {
        Ok(match self {
            Compressor::Gzip => Box::new(flate2::read::MultiGzDecoder::new(bytes)),
            Compressor::Bzip2 => Box::new(bzip2::read::MultiBzDecoder::new(bytes)),
            Compressor::Xz => Box::new(lzma_rust2::XzReader::new(bytes, true)),
            Compressor::Zstd => Box::new(ZstdFrames::new(bytes)?),
        })
    }

    fn tar_format(self) -> ArchiveFormat {
        match self {
            Compressor::Gzip => ArchiveFormat::TarGz,
            Compressor::Bzip2 => ArchiveFormat::TarBz2,
            Compressor::Xz => ArchiveFormat::TarXz,
            Compressor::Zstd => ArchiveFormat::TarZst,
        }
    }

    fn single_format(self) -> ArchiveFormat {
        match self {
            Compressor::Gzip => ArchiveFormat::Gzip,
            Compressor::Bzip2 => ArchiveFormat::Bzip2,
            Compressor::Xz => ArchiveFormat::Xz,
            Compressor::Zstd => ArchiveFormat::Zstd,
        }
    }
}

/// Every zstd frame of a stream one after the other (`ruzstd` decodes one).
struct ZstdFrames<'a> {
    frame: Option<ruzstd::decoding::StreamingDecoder<&'a [u8], ruzstd::decoding::FrameDecoder>>,
}

impl<'a> ZstdFrames<'a> {
    fn new(source: &'a [u8]) -> Result<Self, String> {
        let first = ruzstd::decoding::StreamingDecoder::new(source).map_err(|e| e.to_string())?;
        Ok(ZstdFrames { frame: Some(first) })
    }
}

impl<'a> Read for ZstdFrames<'a> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let Some(frame) = self.frame.as_mut() else {
                return Ok(0);
            };
            let n = frame.read(buf)?;
            if n > 0 || buf.is_empty() {
                return Ok(n);
            }
            // this frame is done: the next one starts where its input ended
            let rest: &'a [u8] = frame.get_ref();
            self.frame = None;
            if rest.is_empty() {
                return Ok(0);
            }
            match ruzstd::decoding::StreamingDecoder::new(rest) {
                Ok(next) => self.frame = Some(next),
                // trailing bytes that are not a frame end the stream
                Err(_) => return Ok(0),
            }
        }
    }
}

/// Stops after `left` bytes and records that it did.
struct Limited<R> {
    inner: R,
    left: u64,
    hit: Arc<std::sync::atomic::AtomicBool>,
}

impl<R: Read> Read for Limited<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.left == 0 {
            if !buf.is_empty() {
                // more is there: say so instead of a silent end
                let mut probe = [0u8; 1];
                if self.inner.read(&mut probe)? > 0 {
                    self.hit.store(true, std::sync::atomic::Ordering::Relaxed);
                    return Err(std::io::Error::other("unpacked size limit"));
                }
            }
            return Ok(0);
        }
        let max = buf
            .len()
            .min(usize::try_from(self.left).unwrap_or(usize::MAX));
        let n = self.inner.read(&mut buf[..max])?;
        self.left -= n as u64;
        Ok(n)
    }
}

/// An entry path as shown and compared: `/`-separated, without a leading
/// `./` or `/` or a trailing `/`.
pub(crate) fn normalize(name: &str) -> String {
    let name = name.replace('\\', "/");
    let mut parts: Vec<&str> = Vec::new();
    for part in name.split('/') {
        match part {
            "" | "." => {}
            part => parts.push(part),
        }
    }
    parts.join("/")
}

/// The name of the file a single compressed file holds (`log.txt` for
/// `logs/log.txt.gz`).
pub(crate) fn single_name(name: &str) -> String {
    let file = name.rsplit('/').next().unwrap_or(name);
    match file.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem.to_string(),
        _ => format!("{file}.out"),
    }
}

/// Days from 1970-01-01 to a proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn zip_time(time: zip::DateTime) -> i64 {
    days_from_civil(
        i64::from(time.year()),
        i64::from(time.month()),
        i64::from(time.day()),
    ) * 86_400
        + i64::from(time.hour()) * 3600
        + i64::from(time.minute()) * 60
        + i64::from(time.second())
}

fn method_name(method: zip::CompressionMethod) -> String {
    use zip::CompressionMethod as M;
    #[allow(deprecated)]
    match method {
        M::Stored => "stored".into(),
        M::Deflated => "deflate".into(),
        M::Deflate64 => "deflate64".into(),
        M::Bzip2 => "bzip2".into(),
        M::Lzma => "lzma".into(),
        M::Xz => "xz".into(),
        // zstd and AES are not built in: named from their ids
        other => match format!("{other:?}").to_ascii_lowercase() {
            name if name.contains("93") => "zstd".into(),
            name if name.contains("99") => "aes".into(),
            name => name,
        },
    }
}

/// Reads everything up to `max` bytes; `Err` past it.
fn read_capped(reader: &mut dyn Read, max: u64) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    reader
        .take(max + 1)
        .read_to_end(&mut out)
        .map_err(|e| e.to_string())?;
    if out.len() as u64 > max {
        return Err(too_large());
    }
    Ok(out)
}

pub(crate) fn too_large() -> String {
    format!(
        "The entry is larger than {} MB, the most Corvene unpacks to compare.",
        MAX_ENTRY_BYTES >> 20
    )
}

impl OpenArchive {
    /// Lists `bytes`, an archive whose path is `name`. The extension names
    /// the format, the bytes' magic numbers decide it: a `.gz` holding a
    /// tar archive is listed as one.
    pub fn parse(name: &str, bytes: Arc<[u8]>) -> Result<OpenArchive, String> {
        let expected = archive_format(name);
        let size = bytes.len() as u64;
        if bytes.starts_with(b"PK\x03\x04")
            || bytes.starts_with(b"PK\x05\x06")
            || (expected == Some(ArchiveFormat::Zip) && bytes.starts_with(b"PK"))
        {
            return Self::parse_zip(bytes, size);
        }
        if let Some(compressor) = Compressor::sniff(&bytes) {
            // the first block decides between a tar archive and one file
            let tar_named = matches!(
                expected,
                Some(
                    ArchiveFormat::TarGz
                        | ArchiveFormat::TarBz2
                        | ArchiveFormat::TarXz
                        | ArchiveFormat::TarZst
                        | ArchiveFormat::Tar
                )
            );
            let mut head = vec![0u8; 512];
            let mut decoder = compressor.decoder(&bytes)?;
            let got = read_fully(&mut *decoder, &mut head).map_err(|e| e.to_string())?;
            head.truncate(got);
            drop(decoder);
            if tar_named || is_tar_header(&head) {
                let decoder = compressor.decoder(&bytes)?;
                let (entries, kept, truncated) = read_tar(decoder)?;
                return Ok(OpenArchive {
                    format: compressor.tar_format(),
                    bytes: size,
                    entries,
                    truncated,
                    comment: None,
                    single: None,
                    data: Data::Tar {
                        raw: bytes,
                        compressor: Some(compressor),
                        kept,
                    },
                });
            }
            return Self::parse_single(name, compressor, &bytes, size);
        }
        if expected == Some(ArchiveFormat::Tar) || is_tar_header(&bytes) {
            let (entries, kept, truncated) = read_tar(&bytes[..])?;
            return Ok(OpenArchive {
                format: ArchiveFormat::Tar,
                bytes: size,
                entries,
                truncated,
                comment: None,
                single: None,
                data: Data::Tar {
                    raw: bytes,
                    compressor: None,
                    kept,
                },
            });
        }
        Err(match expected {
            Some(format) => format!("It is not a readable {} file.", format.label()),
            None => "It is not an archive Corvene can read.".into(),
        })
    }

    fn parse_zip(bytes: Arc<[u8]>, size: u64) -> Result<OpenArchive, String> {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|e| format!("Not a readable ZIP file: {e}."))?;
        let comment = String::from_utf8_lossy(archive.comment())
            .trim()
            .to_string();
        let total = archive.len();
        let truncated = total > MAX_ENTRIES;
        let mut entries: Vec<(String, ArchiveEntryMeta)> =
            Vec::with_capacity(total.min(MAX_ENTRIES));
        let mut index = HashMap::new();
        for ix in 0..total.min(MAX_ENTRIES) {
            let (path, mut meta, symlink) = {
                let file = archive
                    .by_index_raw(ix)
                    .map_err(|e| format!("Not a readable ZIP file: {e}."))?;
                let path = normalize(file.name());
                let meta = ArchiveEntryMeta {
                    kind: if file.is_dir() {
                        ArchiveEntryKind::Dir
                    } else {
                        ArchiveEntryKind::File
                    },
                    size: file.size(),
                    compressed: Some(file.compressed_size()),
                    crc32: file.crc32(),
                    mode: file.unix_mode().map(|m| m & 0o7777),
                    mtime: file.last_modified().map(zip_time),
                    mtime_naive: true,
                    method: Some(method_name(file.compression())),
                    owner: None,
                    encrypted: file.encrypted(),
                };
                (path, meta, file.is_symlink() && !file.is_dir())
            };
            if path.is_empty() {
                continue;
            }
            if meta.encrypted {
                // no password: the packed bytes stand in for the contents
                let mut file = archive.by_index_raw(ix).map_err(|e| e.to_string())?;
                let mut hasher = crc32fast::Hasher::new();
                let mut buf = vec![0u8; 64 << 10];
                let mut left = MAX_ENTRY_BYTES;
                loop {
                    let n = file.read(&mut buf).map_err(|e| e.to_string())?;
                    if n == 0 || left == 0 {
                        break;
                    }
                    let n = n.min(usize::try_from(left).unwrap_or(n));
                    hasher.update(&buf[..n]);
                    left -= n as u64;
                }
                meta.crc32 = hasher.finalize();
            } else if symlink {
                let target = archive
                    .by_index(ix)
                    .ok()
                    .and_then(|mut file| read_capped(&mut file, MAX_LINK).ok())
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .unwrap_or_default();
                meta.kind = ArchiveEntryKind::Symlink(target);
            }
            index.insert(path.clone(), ix);
            entries.push((path, meta));
        }
        let entries = sort_dedup(entries);
        Ok(OpenArchive {
            format: ArchiveFormat::Zip,
            bytes: size,
            entries,
            truncated,
            comment: (!comment.is_empty()).then_some(comment),
            single: None,
            data: Data::Zip { archive, index },
        })
    }

    fn parse_single(
        name: &str,
        compressor: Compressor,
        bytes: &[u8],
        size: u64,
    ) -> Result<OpenArchive, String> {
        let inner = single_name(name);
        let mut decoder = compressor.decoder(bytes)?;
        let mut hasher = crc32fast::Hasher::new();
        let mut kept = Vec::new();
        let mut total: u64 = 0;
        let mut buf = vec![0u8; 64 << 10];
        let mut truncated = false;
        loop {
            let n = decoder.read(&mut buf).map_err(|e| {
                format!(
                    "Not a readable {} file: {e}.",
                    compressor.single_format().label()
                )
            })?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            if total + (n as u64) <= MAX_ENTRY_BYTES {
                kept.extend_from_slice(&buf[..n]);
            }
            total += n as u64;
            if total > MAX_UNPACKED {
                truncated = true;
                break;
            }
        }
        let mtime = match compressor {
            Compressor::Gzip => flate2::read::GzDecoder::new(bytes)
                .header()
                .map(|h| i64::from(h.mtime()))
                .filter(|t| *t > 0),
            _ => None,
        };
        let meta = ArchiveEntryMeta {
            kind: ArchiveEntryKind::File,
            size: total,
            compressed: Some(size),
            crc32: hasher.finalize(),
            mode: None,
            mtime,
            mtime_naive: false,
            method: None,
            owner: None,
            encrypted: false,
        };
        let contents = (total <= MAX_ENTRY_BYTES && !truncated).then(|| Arc::from(kept));
        Ok(OpenArchive {
            format: compressor.single_format(),
            bytes: size,
            entries: vec![(inner.clone(), meta)],
            truncated,
            comment: None,
            single: Some(inner),
            data: Data::Single { contents },
        })
    }

    /// The memory this archive holds while cached.
    pub fn cost(&self) -> usize {
        let entries = self.entries.len() * 160;
        entries
            + match &self.data {
                Data::Zip { archive, .. } => archive.clone().into_inner().get_ref().len(),
                Data::Tar { raw, kept, .. } => {
                    raw.len() + kept.values().map(|v| v.len()).sum::<usize>()
                }
                Data::Single { contents } => contents.as_ref().map_or(0, |c| c.len()),
            }
    }

    /// The listed entry at `path`.
    pub fn meta(&self, path: &str) -> Option<&ArchiveEntryMeta> {
        self.entries
            .binary_search_by(|(p, _)| p.as_str().cmp(path))
            .ok()
            .map(|ix| &self.entries[ix].1)
    }

    /// The contents of the file entry at `path`, at most
    /// [`MAX_ENTRY_BYTES`].
    pub fn read(&self, path: &str) -> Result<Arc<[u8]>, String> {
        let meta = self
            .meta(path)
            .ok_or_else(|| "The entry is not in the archive.".to_string())?;
        if meta.size > MAX_ENTRY_BYTES {
            return Err(too_large());
        }
        match &self.data {
            Data::Zip { archive, index } => {
                let ix = *index
                    .get(path)
                    .ok_or_else(|| "The entry is not in the archive.".to_string())?;
                let mut archive = archive.clone();
                let mut file = archive.by_index(ix).map_err(|e| match e {
                    zip::result::ZipError::UnsupportedArchive(msg)
                        if msg.contains("assword") || meta.encrypted =>
                    {
                        "The entry is encrypted.".to_string()
                    }
                    zip::result::ZipError::UnsupportedArchive(_) => format!(
                        "The entry is compressed with {}, which Corvene can't unpack.",
                        meta.method.as_deref().unwrap_or("an unknown method")
                    ),
                    other => other.to_string(),
                })?;
                Ok(Arc::from(read_capped(&mut file, MAX_ENTRY_BYTES)?))
            }
            Data::Tar {
                raw,
                compressor,
                kept,
            } => {
                if let Some(bytes) = kept.get(path) {
                    return Ok(bytes.clone());
                }
                let reader: Box<dyn Read + '_> = match compressor {
                    Some(c) => c.decoder(raw)?,
                    None => Box::new(&raw[..]),
                };
                tar_entry(reader, path)
            }
            Data::Single { contents } => contents.clone().ok_or_else(too_large),
        }
    }
}

fn read_fully(reader: &mut dyn Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut got = 0;
    while got < buf.len() {
        match reader.read(&mut buf[got..])? {
            0 => break,
            n => got += n,
        }
    }
    Ok(got)
}

/// A POSIX (`ustar`) or GNU tar header.
fn is_tar_header(block: &[u8]) -> bool {
    block.len() >= 262 && &block[257..262] == b"ustar"
}

/// Sorts by path and keeps the last of a path stored more than once.
fn sort_dedup(mut entries: Vec<(String, ArchiveEntryMeta)>) -> Vec<(String, ArchiveEntryMeta)> {
    // stable: equal paths stay in archive order, the last one wins
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out: Vec<(String, ArchiveEntryMeta)> = Vec::with_capacity(entries.len());
    for entry in entries {
        match out.last_mut() {
            Some(last) if last.0 == entry.0 => *last = entry,
            _ => out.push(entry),
        }
    }
    out
}

type TarListing = (
    Vec<(String, ArchiveEntryMeta)>,
    HashMap<String, Arc<[u8]>>,
    bool,
);

fn tar_kind(entry: &tar::Entry<'_, impl Read>) -> Option<ArchiveEntryKind> {
    use tar::EntryType as T;
    let link = || {
        entry
            .link_name()
            .ok()
            .flatten()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    Some(match entry.header().entry_type() {
        T::Regular | T::Continuous | T::GNUSparse => ArchiveEntryKind::File,
        T::Directory => ArchiveEntryKind::Dir,
        T::Symlink => ArchiveEntryKind::Symlink(link()),
        T::Link => ArchiveEntryKind::Hardlink(link()),
        T::XGlobalHeader | T::XHeader | T::GNULongName | T::GNULongLink => return None,
        _ => ArchiveEntryKind::Other,
    })
}

fn read_tar(reader: impl Read) -> Result<TarListing, String> {
    let hit = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let limited = Limited {
        inner: reader,
        left: MAX_UNPACKED,
        hit: hit.clone(),
    };
    let mut archive = tar::Archive::new(limited);
    let mut entries = Vec::new();
    let mut kept = HashMap::new();
    let mut kept_bytes: u64 = 0;
    let mut truncated = false;
    let iter = archive
        .entries()
        .map_err(|e| format!("Not a readable tar archive: {e}."))?;
    for entry in iter {
        let mut entry = match entry {
            Ok(entry) => entry,
            Err(_) if hit.load(std::sync::atomic::Ordering::Relaxed) => {
                truncated = true;
                break;
            }
            Err(e) => return Err(format!("Not a readable tar archive: {e}.")),
        };
        if entries.len() >= MAX_ENTRIES {
            truncated = true;
            break;
        }
        let Some(kind) = tar_kind(&entry) else {
            continue;
        };
        let path = normalize(&String::from_utf8_lossy(&entry.path_bytes()));
        if path.is_empty() {
            continue;
        }
        let header = entry.header();
        let mode = header.mode().ok().map(|m| m & 0o7777);
        let mtime = header.mtime().ok().and_then(|t| i64::try_from(t).ok());
        let owner = match (
            header.username().ok().flatten(),
            header.groupname().ok().flatten(),
        ) {
            (Some(u), Some(g)) if !u.is_empty() || !g.is_empty() => Some(format!("{u}:{g}")),
            _ => match (header.uid().ok(), header.gid().ok()) {
                (Some(u), Some(g)) => Some(format!("{u}:{g}")),
                _ => None,
            },
        };
        let mut size = 0u64;
        let mut hasher = crc32fast::Hasher::new();
        let mut contents = Vec::new();
        if kind == ArchiveEntryKind::File {
            let declared = entry.size();
            let keep = declared <= MAX_ENTRY_BYTES && kept_bytes + declared <= KEEP_BUDGET;
            let mut buf = vec![0u8; 64 << 10];
            loop {
                let n = match entry.read(&mut buf) {
                    Ok(n) => n,
                    Err(_) if hit.load(std::sync::atomic::Ordering::Relaxed) => {
                        truncated = true;
                        break;
                    }
                    Err(e) => return Err(format!("Not a readable tar archive: {e}.")),
                };
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                if keep {
                    contents.extend_from_slice(&buf[..n]);
                }
                size += n as u64;
            }
            if truncated {
                break;
            }
            if keep {
                kept_bytes += size;
                kept.insert(path.clone(), Arc::from(contents));
            }
        }
        entries.push((
            path,
            ArchiveEntryMeta {
                kind,
                size,
                compressed: None,
                crc32: hasher.finalize(),
                mode,
                mtime,
                mtime_naive: false,
                method: None,
                owner,
                encrypted: false,
            },
        ));
    }
    Ok((sort_dedup(entries), kept, truncated))
}

/// Streams a tar archive again for one entry (the last one at `path`).
fn tar_entry(reader: impl Read, path: &str) -> Result<Arc<[u8]>, String> {
    let hit = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut archive = tar::Archive::new(Limited {
        inner: reader,
        left: MAX_UNPACKED,
        hit,
    });
    let mut found = None;
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let Ok(mut entry) = entry else {
            break;
        };
        if normalize(&String::from_utf8_lossy(&entry.path_bytes())) != path
            || tar_kind(&entry) != Some(ArchiveEntryKind::File)
        {
            continue;
        }
        found = Some(read_capped(&mut entry, MAX_ENTRY_BYTES)?);
    }
    found
        .map(Arc::from)
        .ok_or_else(|| "The entry is not in the archive.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_entry_paths() {
        assert_eq!(normalize("./src/main.rs"), "src/main.rs");
        assert_eq!(normalize("/abs//x/"), "abs/x");
        assert_eq!(normalize("win\\path\\a.txt"), "win/path/a.txt");
        assert_eq!(normalize("./"), "");
    }

    #[test]
    fn names_the_file_inside_a_compressed_file() {
        assert_eq!(single_name("logs/run.txt.gz"), "run.txt");
        assert_eq!(single_name("data.gz"), "data");
        assert_eq!(single_name(".gz"), ".gz.out");
    }

    #[test]
    fn converts_dos_times() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
    }
}
