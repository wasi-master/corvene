//! Font files read for diffs (Corvene `font-diff` / `font-source-preview`;
//! GHD shows every font as "Binary file not shown", `app/src/lib/git/diff.ts`).
//!
//! - [`container`]: sniffs and unpacks TrueType / OpenType (CFF, CFF2),
//!   WOFF 1.0, WOFF 2.0 and collections (TTC / OTC) into sfnt bytes.
//! - [`report`]: everything one face says about itself (names, metrics,
//!   tables, cmap, glyphs, layout features, kerning, axes, colour).
//! - [`compare`]: two reports matched face by face and glyph by glyph.
//! - [`render`]: glyphs and shaped text rasterised with tiny-skia.
//! - [`sources`]: UFO `.glif`, Glyphs `.glyphs` and `.designspace` files.
//!
//! The crate reads bytes only; it never touches git, disk or UI.

pub mod blocks;
pub mod compare;
pub mod container;
pub mod layout;
pub mod outline;
pub mod render;
pub mod report;
pub mod sources;
mod woff;
mod woff2;

pub use compare::{FaceDiff, FontDiff, GlyphRow, Status, compare};
pub use container::{Decoded, Format, decode, sniff};
pub use report::FaceReport;
/// The font handle the render functions take.
pub use skrifa::FontRef;
/// Normalised variation coordinates.
pub use skrifa::instance::NormalizedCoord;
/// OpenType tags (features, axes, tables).
pub use skrifa::raw::types::Tag;

/// Fonts larger than this (either side, as stored) are not read: the diff
/// stays "Binary file not shown".
pub const MAX_BYTES: usize = 64 * 1024 * 1024;

/// Why a font could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FontError {
    #[error("not a font file")]
    NotAFont,
    #[error("font file is larger than 64 MB")]
    TooLarge,
    #[error("damaged {format} file: {reason}")]
    Malformed {
        format: &'static str,
        reason: String,
    },
    #[error("unreadable font: {0}")]
    Read(String),
}

impl FontError {
    pub(crate) fn malformed(format: &'static str, reason: impl Into<String>) -> Self {
        Self::Malformed {
            format,
            reason: reason.into(),
        }
    }
}

impl From<skrifa::raw::ReadError> for FontError {
    fn from(e: skrifa::raw::ReadError) -> Self {
        Self::Read(e.to_string())
    }
}
