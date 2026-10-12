//! Random access byte sources and a bounds-checked, buffered cursor.

use crate::Error;

/// Random access to a file's bytes (a file on disk or a blob in memory).
pub trait Source {
    /// Total byte length.
    fn len(&self) -> u64;
    /// Fills `buf` with the bytes at `offset`; errors when they run past
    /// the end.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<()>;
    /// Whether the source holds no bytes.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn eof() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "read past the end")
}

impl Source for [u8] {
    fn len(&self) -> u64 {
        <[u8]>::len(self) as u64
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        let start = usize::try_from(offset).map_err(|_| eof())?;
        let end = start.checked_add(buf.len()).ok_or_else(eof)?;
        let src = self.get(start..end).ok_or_else(eof)?;
        buf.copy_from_slice(src);
        Ok(())
    }
}

impl Source for Vec<u8> {
    fn len(&self) -> u64 {
        self.as_slice().len() as u64
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        self.as_slice().read_at(offset, buf)
    }
}

impl Source for std::fs::File {
    fn len(&self) -> u64 {
        self.metadata().map(|m| m.len()).unwrap_or(0)
    }

    #[cfg(unix)]
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        std::os::unix::fs::FileExt::read_exact_at(self, buf, offset)
    }

    #[cfg(windows)]
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        let mut done = 0usize;
        while done < buf.len() {
            let n = std::os::windows::fs::FileExt::seek_read(
                self,
                &mut buf[done..],
                offset + done as u64,
            )?;
            if n == 0 {
                return Err(eof());
            }
            done += n;
        }
        Ok(())
    }

    #[cfg(not(any(unix, windows)))]
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = self;
        f.seek(SeekFrom::Start(offset))?;
        f.read_exact(buf)
    }
}

/// A window of another source.
pub(crate) struct SubSource<'a> {
    pub inner: &'a dyn Source,
    pub offset: u64,
    pub len: u64,
}

impl Source for SubSource<'_> {
    fn len(&self) -> u64 {
        self.len
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        let end = offset.checked_add(buf.len() as u64).ok_or_else(eof)?;
        if end > self.len {
            return Err(eof());
        }
        self.inner.read_at(self.offset + offset, buf)
    }
}

/// `Read + Seek` over a source (for the zip reader).
pub(crate) struct ReadSeek<'a> {
    pub src: &'a dyn Source,
    pub pos: u64,
}

impl std::io::Read for ReadSeek<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let left = self.src.len().saturating_sub(self.pos);
        let n = (buf.len() as u64).min(left) as usize;
        if n == 0 {
            return Ok(0);
        }
        self.src.read_at(self.pos, &mut buf[..n])?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl std::io::Seek for ReadSeek<'_> {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        let len = self.src.len() as i128;
        let next = match pos {
            std::io::SeekFrom::Start(p) => p as i128,
            std::io::SeekFrom::End(d) => len + d as i128,
            std::io::SeekFrom::Current(d) => self.pos as i128 + d as i128,
        };
        if next < 0 || next > u64::MAX as i128 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek out of range",
            ));
        }
        self.pos = next as u64;
        Ok(self.pos)
    }
}

const BLOCK: usize = 64 * 1024;

/// Largest single allocation a header field may ask for.
pub(crate) const MAX_ALLOC: u64 = 256 * 1024 * 1024;

/// A buffered cursor over a source; every read is checked against the end.
pub(crate) struct Reader<'a> {
    pub src: &'a dyn Source,
    len: u64,
    pos: u64,
    buf: Vec<u8>,
    start: u64,
}

pub(crate) fn truncated() -> Error {
    Error::Format("truncated".into())
}

pub(crate) fn bad(msg: impl Into<String>) -> Error {
    Error::Format(msg.into())
}

impl<'a> Reader<'a> {
    pub fn new(src: &'a dyn Source) -> Self {
        Reader {
            src,
            len: src.len(),
            pos: 0,
            buf: Vec::new(),
            start: 0,
        }
    }

    pub fn len(&self) -> u64 {
        self.len
    }
    pub fn pos(&self) -> u64 {
        self.pos
    }
    pub fn seek(&mut self, pos: u64) {
        self.pos = pos;
    }
    pub fn remaining(&self) -> u64 {
        self.len.saturating_sub(self.pos)
    }
    pub fn skip(&mut self, n: u64) -> Result<(), Error> {
        if n > self.remaining() {
            return Err(truncated());
        }
        self.pos += n;
        Ok(())
    }

    /// The next `n` bytes (borrowed from the buffer), advancing past them.
    pub fn take(&mut self, n: usize) -> Result<&[u8], Error> {
        if n as u64 > self.remaining() || n as u64 > MAX_ALLOC {
            return Err(truncated());
        }
        let in_buf =
            self.pos >= self.start && self.pos + n as u64 <= self.start + self.buf.len() as u64;
        if !in_buf {
            let want = n.max(BLOCK) as u64;
            let size = want.min(self.remaining()) as usize;
            self.buf.resize(size, 0);
            self.src.read_at(self.pos, &mut self.buf)?;
            self.start = self.pos;
        }
        let at = (self.pos - self.start) as usize;
        self.pos += n as u64;
        Ok(&self.buf[at..at + n])
    }

    pub fn bytes(&mut self, n: u64) -> Result<Vec<u8>, Error> {
        let n = usize::try_from(n).map_err(|_| truncated())?;
        Ok(self.take(n)?.to_vec())
    }

    /// Bytes at an absolute position, leaving the cursor where it was.
    pub fn bytes_at(&mut self, at: u64, n: u64) -> Result<Vec<u8>, Error> {
        let keep = self.pos;
        self.pos = at;
        let r = self.bytes(n);
        self.pos = keep;
        r
    }

    fn arr<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let s = self.take(N)?;
        let mut a = [0u8; N];
        a.copy_from_slice(s);
        Ok(a)
    }
    pub fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.arr::<1>()?[0])
    }
    pub fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(self.arr()?))
    }
    pub fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.arr()?))
    }
    pub fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.arr()?))
    }
    pub fn i32(&mut self) -> Result<i32, Error> {
        Ok(i32::from_le_bytes(self.arr()?))
    }
    /// Little-endian unsigned integer of `n` (0 to 8) bytes.
    pub fn uint(&mut self, n: usize) -> Result<u64, Error> {
        if n > 8 {
            return Err(bad("integer wider than 8 bytes"));
        }
        let s = self.take(n)?;
        let mut v = 0u64;
        for (i, b) in s.iter().enumerate() {
            v |= (*b as u64) << (8 * i);
        }
        Ok(v)
    }

    /// A UTF-8 string of `n` bytes (lossy), refusing absurd lengths.
    pub fn string(&mut self, n: u64) -> Result<String, Error> {
        if n > self.remaining() {
            return Err(truncated());
        }
        let n = usize::try_from(n).map_err(|_| truncated())?;
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
}
