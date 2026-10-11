//! Corvene `1324-document-diff`: Word, PowerPoint, OpenDocument, EPUB and
//! RTF files read into blocks (headings, paragraphs, lists, tables,
//! pictures; a section per slide or chapter), so their diff can show the
//! document as a text editor would, or its extracted text.
//!
//! GitHub Desktop shows these files as "This binary file has changed."
//! (`app/src/lib/git/diff.ts` `getDiffType`, `ui/diff/binary-file.tsx`),
//! and RTF or flat OpenDocument files as their markup. The readers are
//! plain Rust (zip + roxmltree), so every release target gets them. They
//! read the text a reader sees: tracked insertions are kept, tracked
//! deletions left out, headers, footers and comments skipped.

mod epub;
mod odf;
mod ooxml;
mod rtf;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::sync::atomic::{AtomicBool, Ordering};

use corvene_models::{
    Diff, DocBlob, DocBlock, DocKind, DocRun, DocSection, FileStatusKind, ImageBlob, RunStyle,
};

/// Set by the dispatcher from the flag.
static READ_DOCUMENT_FILES: AtomicBool = AtomicBool::new(false);

/// Turn document diffs on or off.
pub fn set_read_document_files(enabled: bool) {
    READ_DOCUMENT_FILES.store(enabled, Ordering::Relaxed);
}

fn read_document_files() -> bool {
    READ_DOCUMENT_FILES.load(Ordering::Relaxed)
}

/// Larger files stay binary.
pub const MAX_BYTES: usize = 64 << 20;
/// Blocks read per side; the rest are left out.
pub const MAX_BLOCKS: usize = 20_000;
/// Entries in a zip package (more and the file is not read).
const MAX_ENTRIES: usize = 20_000;
/// Uncompressed size of one part.
const MAX_PART: u64 = 32 << 20;
/// Uncompressed bytes read from one package (zip bombs).
const MAX_TOTAL: usize = 128 << 20;
/// Picture bytes kept per side; later pictures show as placeholders.
const MAX_IMAGE_BYTES: usize = 24 << 20;
/// XML nodes per part.
const MAX_NODES: u32 = 4_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocFormat {
    /// .docx and its template and macro variants.
    Word,
    /// .pptx and its template, show and macro variants.
    PowerPoint,
    /// .odt / .odp and their templates (a zip package).
    OpenDocument,
    /// .fodt / .fodp: one XML file, which git diffs as text.
    FlatOpenDocument,
    Epub,
    /// Git diffs it as text.
    Rtf,
}

impl DocFormat {
    /// Git reads the file as text: its own diff is the Text view.
    pub fn is_text(self) -> bool {
        matches!(self, DocFormat::FlatOpenDocument | DocFormat::Rtf)
    }
}

/// The document format of a file, from its extension.
pub fn document_format(path: &str) -> Option<DocFormat> {
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "docx" | "docm" | "dotx" | "dotm" => DocFormat::Word,
        "pptx" | "pptm" | "potx" | "potm" | "ppsx" | "ppsm" => DocFormat::PowerPoint,
        "odt" | "ott" | "odp" | "otp" => DocFormat::OpenDocument,
        "fodt" | "fodp" => DocFormat::FlatOpenDocument,
        "epub" => DocFormat::Epub,
        "rtf" => DocFormat::Rtf,
        _ => return None,
    })
}

/// A binary document file (the flag is on): kept away from the UTF-16
/// decoding of binary files.
pub fn is_binary_document(path: &str) -> bool {
    read_document_files() && document_format(path).is_some_and(|f| !f.is_text())
}

/// A document file git diffs as text (RTF, flat OpenDocument) while the
/// flag is on.
pub fn is_text_document(path: &str) -> bool {
    read_document_files() && document_format(path).is_some_and(DocFormat::is_text)
}

/// A change of a document file as a [`Diff::Document`]; `None` when the
/// flag is off, the path is not a document, a side is too large or does
/// not read (the caller keeps its own diff). `source` is git's text diff
/// of an RTF or flat OpenDocument file.
pub fn document_diff(
    path: &str,
    kind: FileStatusKind,
    current: impl FnOnce() -> Option<Vec<u8>>,
    previous: impl FnOnce() -> Option<Vec<u8>>,
    source: Option<Diff>,
) -> Option<Diff> {
    if !read_document_files() {
        return None;
    }
    let format = document_format(path)?;
    let side = |bytes: Option<Vec<u8>>| -> Option<Option<DocBlob>> {
        match bytes {
            None => Some(None),
            Some(bytes) if bytes.len() > MAX_BYTES => None,
            Some(bytes) => match read(format, &bytes) {
                Ok(blob) => Some(Some(blob)),
                Err(e) => {
                    tracing::debug!(path, error = %e, "document file did not read");
                    None
                }
            },
        }
    };
    let current = side((kind != FileStatusKind::Deleted).then(current).flatten())?;
    let previous = side((!kind.is_new_or_untracked()).then(previous).flatten())?;
    if current.is_none() && previous.is_none() {
        return None;
    }
    Some(Diff::Document {
        previous,
        current,
        source: source.map(Box::new),
    })
}

/// One version of a document file.
pub fn read(format: DocFormat, bytes: &[u8]) -> Result<DocBlob, String> {
    let mut blob = match format {
        DocFormat::Word => ooxml::read_word(bytes),
        DocFormat::PowerPoint => ooxml::read_presentation(bytes),
        DocFormat::OpenDocument => odf::read_package(bytes),
        DocFormat::FlatOpenDocument => odf::read_flat(bytes),
        DocFormat::Epub => epub::read(bytes),
        DocFormat::Rtf => rtf::read(bytes),
    }?;
    blob.file = std::sync::Arc::new(bytes.to_vec());
    Ok(blob)
}

// ---------------------------------------------------------------------------
// Zip packages and XML
// ---------------------------------------------------------------------------

/// A zip package (OOXML, OpenDocument, EPUB) read within size limits.
pub(crate) struct Package<'a> {
    zip: zip::ZipArchive<Cursor<&'a [u8]>>,
    /// Lower-cased part name → stored name (OOXML part names ignore case).
    names: HashMap<String, String>,
    read_total: usize,
}

impl<'a> Package<'a> {
    pub(crate) fn open(bytes: &'a [u8]) -> Result<Self, String> {
        let zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
        if zip.len() > MAX_ENTRIES {
            return Err(format!("{} entries", zip.len()));
        }
        let names = zip
            .file_names()
            .map(|n| (n.to_ascii_lowercase(), n.to_string()))
            .collect();
        Ok(Package {
            zip,
            names,
            read_total: 0,
        })
    }

    /// The bytes of a part (`word/document.xml`), `None` when missing or
    /// over a limit.
    pub(crate) fn bytes(&mut self, name: &str) -> Option<Vec<u8>> {
        let name = name.trim_start_matches('/');
        let stored = self.names.get(&name.to_ascii_lowercase())?.clone();
        let file = self.zip.by_name(&stored).ok()?;
        if file.size() > MAX_PART {
            return None;
        }
        let mut out = Vec::new();
        file.take(MAX_PART + 1).read_to_end(&mut out).ok()?;
        if out.len() as u64 > MAX_PART {
            return None;
        }
        self.read_total += out.len();
        (self.read_total <= MAX_TOTAL).then_some(out)
    }

    /// The text of an XML part.
    pub(crate) fn xml(&mut self, name: &str) -> Option<String> {
        xml_text(self.bytes(name)?)
    }
}

/// XML bytes as text (UTF-8, a byte order mark dropped).
pub(crate) fn xml_text(bytes: Vec<u8>) -> Option<String> {
    let mut text = String::from_utf8(bytes).ok()?;
    if text.starts_with('\u{feff}') {
        text.remove(0);
    }
    Some(text)
}

/// Parse XML without a DTD (no entity expansion).
pub(crate) fn parse_xml(text: &str) -> Result<roxmltree::Document<'_>, String> {
    roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: MAX_NODES,
            ..roxmltree::ParsingOptions::default()
        },
    )
    .map_err(|e| e.to_string())
}

/// The attribute with this local name, whatever its namespace.
pub(crate) fn attr<'a>(node: roxmltree::Node<'a, '_>, local: &str) -> Option<&'a str> {
    node.attributes()
        .find(|a| a.name() == local)
        .map(|a| a.value())
}

/// The element's local name ("" for text and other nodes).
pub(crate) fn name<'a>(node: roxmltree::Node<'a, '_>) -> &'a str {
    if node.is_element() {
        node.tag_name().name()
    } else {
        ""
    }
}

/// The first child element with this local name.
pub(crate) fn child<'a, 'i>(
    node: roxmltree::Node<'a, 'i>,
    local: &str,
) -> Option<roxmltree::Node<'a, 'i>> {
    node.children().find(|c| name(*c) == local)
}

/// `target` relative to the folder of `part` (`word/document.xml` +
/// `media/a.png` → `word/media/a.png`; `..` and absolute targets work).
pub(crate) fn resolve(part: &str, target: &str) -> String {
    let target = percent_decode(target);
    let mut parts: Vec<&str> = if let Some(abs) = target.strip_prefix('/') {
        return abs.to_string();
    } else {
        part.split('/').collect()
    };
    parts.pop();
    for seg in target.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            seg => parts.push(seg),
        }
    }
    parts.join("/")
}

fn percent_decode(text: &str) -> String {
    if !text.contains('%') {
        return text.to_string();
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3])
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------------------
// Building a DocBlob
// ---------------------------------------------------------------------------

/// Collects the sections, blocks and pictures of one side.
pub(crate) struct Builder {
    kind: DocKind,
    sections: Vec<DocSection>,
    images: Vec<ImageBlob>,
    image_bytes: usize,
    blocks: usize,
    truncated: bool,
}

impl Builder {
    pub(crate) fn new(kind: DocKind) -> Self {
        Builder {
            kind,
            sections: Vec::new(),
            images: Vec::new(),
            image_bytes: 0,
            blocks: 0,
            truncated: false,
        }
    }

    /// Start a slide or chapter.
    pub(crate) fn section(&mut self, name: String, hidden: bool) {
        self.sections.push(DocSection {
            name,
            hidden,
            ..DocSection::default()
        });
    }

    fn current(&mut self) -> &mut DocSection {
        if self.sections.is_empty() {
            self.sections.push(DocSection::default());
        }
        let last = self.sections.len() - 1;
        &mut self.sections[last]
    }

    /// Room for one more block.
    fn room(&mut self) -> bool {
        if self.blocks >= MAX_BLOCKS {
            self.truncated = true;
            return false;
        }
        self.blocks += 1;
        true
    }

    /// Add a block; empty paragraphs (spacing) are left out.
    pub(crate) fn push(&mut self, block: DocBlock) {
        if is_empty(&block) || !self.room() {
            return;
        }
        self.current().blocks.push(block);
    }

    /// Add a block to the current slide's notes.
    pub(crate) fn push_note(&mut self, block: DocBlock) {
        if is_empty(&block) || !self.room() {
            return;
        }
        self.current().notes.push(block);
    }

    /// Name the current section when it has no name yet.
    pub(crate) fn name_section(&mut self, name: &str) {
        let section = self.current();
        if section.name.is_empty() {
            section.name = name.trim().to_string();
        }
    }

    /// Keep a picture; `None` when it is not a drawable image or over the
    /// byte budget.
    pub(crate) fn image(&mut self, part: &str, bytes: Option<Vec<u8>>) -> Option<usize> {
        let bytes = bytes?;
        let media_type = sniff_image(&bytes).or_else(|| corvene_models::image_media_type(part))?;
        if self.image_bytes + bytes.len() > MAX_IMAGE_BYTES {
            return None;
        }
        if let Some(i) = self.images.iter().position(|i| i.bytes == bytes) {
            return Some(i);
        }
        self.image_bytes += bytes.len();
        self.images.push(ImageBlob {
            bytes,
            media_type: media_type.to_string(),
            original: None,
        });
        Some(self.images.len() - 1)
    }

    pub(crate) fn finish(mut self) -> DocBlob {
        if self.kind == DocKind::Text && self.sections.is_empty() {
            self.sections.push(DocSection::default());
        }
        DocBlob {
            kind: self.kind,
            sections: self.sections,
            images: self.images,
            truncated: self.truncated,
            file: Default::default(),
        }
    }
}

fn is_empty(block: &DocBlock) -> bool {
    match block {
        DocBlock::Heading { runs, .. } | DocBlock::Paragraph { runs, .. } => {
            runs.iter().all(|r| r.text.trim().is_empty())
        }
        DocBlock::Table { rows } => rows.is_empty(),
        DocBlock::Code(text) => text.trim().is_empty(),
        DocBlock::Image { .. } | DocBlock::Break => false,
    }
}

/// The media type of PNG, JPEG, GIF, BMP and WebP bytes.
fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    Some(if bytes.starts_with(b"\x89PNG") {
        "image/png"
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "image/jpg"
    } else if bytes.starts_with(b"GIF8") {
        "image/gif"
    } else if bytes.starts_with(b"BM") {
        "image/bmp"
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        return None;
    })
}

/// Append text to `runs`, merged into the last run when it has the same
/// formatting and link.
pub(crate) fn push_run(runs: &mut Vec<DocRun>, text: &str, style: RunStyle, link: Option<&str>) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = runs.last_mut()
        && last.style == style
        && last.link.as_deref() == link
    {
        last.text.push_str(text);
        return;
    }
    runs.push(DocRun {
        text: text.to_string(),
        style,
        link: link.map(str::to_string),
    });
}

/// Trim the whitespace at both ends of a paragraph's runs.
pub(crate) fn trim_runs(mut runs: Vec<DocRun>) -> Vec<DocRun> {
    while let Some(first) = runs.first_mut() {
        let trimmed = first.text.trim_start().len();
        first.text.drain(..first.text.len() - trimmed);
        if first.text.is_empty() {
            runs.remove(0);
        } else {
            break;
        }
    }
    while let Some(last) = runs.last_mut() {
        let len = last.text.trim_end().len();
        last.text.truncate(len);
        if last.text.is_empty() {
            runs.pop();
        } else {
            break;
        }
    }
    runs
}

/// Collapse XML whitespace the way a renderer would (HTML, ODF): runs of
/// spaces, tabs and newlines become one space.
pub(crate) fn collapse_ws(text: &str, after_space: &mut bool) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_whitespace() && c != '\u{a0}' {
            if !*after_space {
                out.push(' ');
                *after_space = true;
            }
        } else {
            out.push(c);
            *after_space = false;
        }
    }
    out
}
