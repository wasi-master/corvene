//! What kind of font file some bytes are, and the sfnt inside it.

use std::sync::Arc;

use skrifa::FontRef;
use skrifa::raw::FileRef;

use crate::FontError;

/// How a font file is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    /// Plain TrueType (`00010000`, `true`) or OpenType (`OTTO`).
    Sfnt,
    /// A TrueType / OpenType collection (`ttcf`).
    Collection,
    /// WOFF 1.0.
    Woff,
    /// WOFF 2.0 (a single font or a collection).
    Woff2,
}

impl Format {
    /// Short label for the overview ("TTC", "WOFF2").
    pub fn label(self) -> &'static str {
        match self {
            Format::Sfnt => "sfnt",
            Format::Collection => "Collection",
            Format::Woff => "WOFF",
            Format::Woff2 => "WOFF2",
        }
    }

    /// Whether the stored bytes are compressed (so the decoded size is
    /// worth showing next to the file size).
    pub fn is_compressed(self) -> bool {
        matches!(self, Format::Woff | Format::Woff2)
    }
}

/// The format of a font file by its first four bytes.
pub fn sniff(bytes: &[u8]) -> Option<Format> {
    match bytes.get(..4)? {
        [0, 1, 0, 0] | b"OTTO" | b"true" => Some(Format::Sfnt),
        b"ttcf" => Some(Format::Collection),
        b"wOFF" => Some(Format::Woff),
        b"wOF2" => Some(Format::Woff2),
        _ => None,
    }
}

/// A font file unpacked to sfnt data (one font, or a `ttcf` collection).
#[derive(Clone, Debug)]
pub struct Decoded {
    pub format: Format,
    /// The sfnt or collection bytes the faces are read from.
    pub data: Arc<[u8]>,
    /// The file's size as stored.
    pub file_len: usize,
    pub face_count: u32,
}

impl Decoded {
    /// Face `index` (0 for a single font).
    pub fn face(&self, index: u32) -> Result<FontRef<'_>, FontError> {
        Ok(FontRef::from_index(&self.data, index)?)
    }

    /// Whether the unpacked data holds several faces.
    pub fn is_collection(&self) -> bool {
        self.data.get(..4) == Some(b"ttcf".as_slice())
    }

    /// Size of the unpacked sfnt data.
    pub fn decoded_len(&self) -> usize {
        self.data.len()
    }
}

/// Unpacks a font file. Fails on anything that is not a font, on damaged
/// data and on files over [`crate::MAX_BYTES`].
pub fn decode(bytes: &[u8]) -> Result<Decoded, FontError> {
    if bytes.len() > crate::MAX_BYTES {
        return Err(FontError::TooLarge);
    }
    let format = sniff(bytes).ok_or(FontError::NotAFont)?;
    let data: Vec<u8> = match format {
        Format::Sfnt | Format::Collection => bytes.to_vec(),
        Format::Woff => crate::woff::decode(bytes)?,
        Format::Woff2 => crate::woff2::decode(bytes)?,
    };
    let face_count = match FileRef::new(&data)? {
        FileRef::Font(_) => 1,
        FileRef::Collection(c) => c.len(),
    };
    if face_count == 0 {
        return Err(FontError::malformed(
            format.label(),
            "collection without fonts",
        ));
    }
    let decoded = Decoded {
        format,
        data: data.into(),
        file_len: bytes.len(),
        face_count,
    };
    // every face must at least open
    for i in 0..face_count {
        decoded.face(i)?;
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_by_magic() {
        assert_eq!(sniff(&[0, 1, 0, 0, 9]), Some(Format::Sfnt));
        assert_eq!(sniff(b"OTTO...."), Some(Format::Sfnt));
        assert_eq!(sniff(b"ttcf"), Some(Format::Collection));
        assert_eq!(sniff(b"wOFF"), Some(Format::Woff));
        assert_eq!(sniff(b"wOF2"), Some(Format::Woff2));
        assert_eq!(sniff(b"PK\x03\x04"), None);
        assert_eq!(sniff(b"OT"), None);
    }

    #[test]
    fn rejects_non_fonts() {
        assert_eq!(decode(b"hello world").unwrap_err(), FontError::NotAFont);
        assert!(decode(&[0, 1, 0, 0, 0, 0]).is_err());
    }
}
