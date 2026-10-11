//! Corvene `1322-pdf-diff` (desktop/desktop#5009, #15875): reading PDF
//! files for the diff view, with the pure Rust hayro rasterizer (no
//! platform library, so every release target gets it).
//!
//! GitHub Desktop shows a PDF as "This binary file has changed."
//! (`lib/git/diff.ts` `getDiffType`, `ui/diff/binary-file.tsx`). Corvene
//! draws each page to a PNG, which the image diff shows in its usual modes,
//! and extracts the text of the pages for the diff header's Pages / Text
//! switch. The text follows the order the page draws it in: a line break
//! where the baseline moves, a space where the gap between two glyphs is
//! wider than the letters around it. Encrypted (with a password) or broken
//! files do not open, and the diff shows the binary panel.

use std::sync::Arc;

use hayro::hayro_interpret::font::Glyph;
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache,
    InterpreterSettings, Paint, PathDrawMode, SoftMask, interpret_page,
};
use hayro::hayro_syntax::Pdf;
use hayro::{RenderCache, RenderSettings};
use kurbo::{Affine, BezPath, Point, Rect};

/// The longest side of a drawn page, in pixels (like an SVG drawn as an
/// image).
pub const MAX_PAGE_PX: u32 = 4096;
/// Text extraction stops after this many pages.
pub const MAX_TEXT_PAGES: usize = 2000;
/// Or after this much text.
pub const MAX_TEXT_BYTES: usize = 5 * 1024 * 1024;
/// The longest side of the thumbnails compared to find changed pages.
const THUMB_PX: u32 = 96;
/// Documents with more pages than this are not scanned for changed pages.
pub const MAX_SCANNED_PAGES: usize = 300;

/// Why a PDF file cannot be shown as pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PdfError {
    #[error("The PDF file is encrypted.")]
    Encrypted,
    #[error("The PDF file could not be read.")]
    Invalid,
}

/// One opened PDF file.
#[derive(Clone)]
pub struct PdfDocument {
    pdf: Arc<Pdf>,
}

/// A drawn page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageImage {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl PdfDocument {
    pub fn open(bytes: Vec<u8>) -> Result<Self, PdfError> {
        match Pdf::new(bytes) {
            Ok(pdf) => Ok(Self { pdf: Arc::new(pdf) }),
            Err(hayro::hayro_syntax::LoadPdfError::Decryption(_)) => Err(PdfError::Encrypted),
            Err(_) => Err(PdfError::Invalid),
        }
    }

    pub fn page_count(&self) -> usize {
        self.pdf.pages().len()
    }

    /// A page's size in points, rotation applied.
    pub fn page_size(&self, index: usize) -> Option<(f32, f32)> {
        Some(self.pdf.pages().get(index)?.render_dimensions())
    }

    /// Draws page `index` on white so its longer side is `longest_px`
    /// pixels (at most [`MAX_PAGE_PX`]).
    pub fn render(&self, index: usize, longest_px: u32) -> Option<PageImage> {
        let pixmap = self.pixmap(index, longest_px.min(MAX_PAGE_PX))?;
        let (width, height) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
        let png = pixmap.into_png().ok()?;
        Some(PageImage { png, width, height })
    }

    /// Page `index` as straight RGBA pixels (width, height, bytes).
    fn render_rgba(&self, index: usize, longest_px: u32) -> Option<(u32, u32, Vec<u8>)> {
        let pixmap = self.pixmap(index, longest_px)?;
        let (width, height) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
        let rgba = pixmap
            .take_unpremultiplied()
            .into_iter()
            .flat_map(|p| [p.r, p.g, p.b, p.a])
            .collect();
        Some((width, height, rgba))
    }

    fn pixmap(&self, index: usize, longest_px: u32) -> Option<hayro::vello_cpu::Pixmap> {
        let pages = self.pdf.pages();
        let page = pages.get(index)?;
        let (w, h) = page.render_dimensions();
        if !(w > 0.0 && h > 0.0) {
            return None;
        }
        let scale = longest_px.max(1) as f32 / w.max(h);
        let settings = RenderSettings {
            x_scale: scale,
            y_scale: scale,
            width: Some(((w * scale).round() as u16).max(1)),
            height: Some(((h * scale).round() as u16).max(1)),
            bg_color: hayro::vello_cpu::color::palette::css::WHITE,
        };
        Some(hayro::render(
            page,
            &RenderCache::new(),
            &InterpreterSettings::default(),
            &settings,
        ))
    }

    /// The text of every page, each page after a `Page N` line. `None`
    /// past [`MAX_TEXT_PAGES`] or [`MAX_TEXT_BYTES`].
    pub fn text(&self) -> Option<String> {
        let pages = self.pdf.pages();
        if pages.len() > MAX_TEXT_PAGES {
            return None;
        }
        let settings = InterpreterSettings::default();
        let cache = InterpreterCache::new();
        let mut out = String::new();
        for (i, page) in pages.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&format!("Page {}\n\n", i + 1));
            let (w, h) = page.render_dimensions();
            let mut context = Context::new(
                Affine::IDENTITY,
                Rect::new(0.0, 0.0, f64::from(w.max(1.0)), f64::from(h.max(1.0))),
                &cache,
                page.xref(),
                settings.clone(),
            );
            let mut device = TextDevice::default();
            interpret_page(page, &mut context, &mut device);
            let text = device.finish();
            out.push_str(&text);
            if !text.is_empty() {
                out.push('\n');
            }
            if out.len() > MAX_TEXT_BYTES {
                return None;
            }
        }
        Some(out)
    }
}

/// How long the line diff of two texts may take; past it the hunks are
/// coarser, never wrong.
const TEXT_DIFF_BUDGET: std::time::Duration = std::time::Duration::from_millis(500);

/// A line diff of the text of two versions of a PDF file (`None`: the side
/// does not exist), marked `DiffWarnings::pdf_text`. `Empty` when the text
/// is the same.
pub fn text_diff(old: Option<&str>, new: Option<&str>) -> corvene_models::Diff {
    use corvene_models::Diff;
    let (old, new) = (old.unwrap_or_default(), new.unwrap_or_default());
    if old == new {
        return Diff::Empty;
    }
    let diff = similar::TextDiff::configure()
        .algorithm(similar::Algorithm::Myers)
        .timeout(TEXT_DIFF_BUDGET)
        .diff_lines(old, new);
    let patch = diff
        .unified_diff()
        .context_radius(3)
        .header("old", "new")
        .to_string();
    match corvene_git::parse_unified(&patch) {
        Diff::Text {
            hunks,
            mut warnings,
        } => {
            warnings.pdf_text = true;
            Diff::Text { hunks, warnings }
        }
        Diff::LargeText {
            hunks,
            mut warnings,
        } => {
            warnings.pdf_text = true;
            Diff::LargeText { hunks, warnings }
        }
        other => other,
    }
}

/// Whether each page pair differs (by index; a page only one side has is
/// changed), from small thumbnails. `None` past [`MAX_SCANNED_PAGES`].
pub fn changed_pages(old: Option<&PdfDocument>, new: Option<&PdfDocument>) -> Option<Vec<bool>> {
    let count = |doc: Option<&PdfDocument>| doc.map_or(0, PdfDocument::page_count);
    let pages = count(old).max(count(new));
    if pages > MAX_SCANNED_PAGES {
        return None;
    }
    let thumb = |doc: Option<&PdfDocument>, i: usize| doc.and_then(|d| d.render_rgba(i, THUMB_PX));
    Some(
        (0..pages)
            .map(|i| match (thumb(old, i), thumb(new, i)) {
                (Some(a), Some(b)) => !same_pixels(&a, &b),
                _ => true,
            })
            .collect(),
    )
}

/// Same size and no channel more than a rounding step apart.
fn same_pixels(a: &(u32, u32, Vec<u8>), b: &(u32, u32, Vec<u8>)) -> bool {
    a.0 == b.0 && a.1 == b.1 && a.2.iter().zip(&b.2).all(|(x, y)| x.abs_diff(*y) <= 8)
}

/// Collects the Unicode text of the glyphs a page draws.
#[derive(Default)]
struct TextDevice {
    out: String,
    /// The previous glyph's origin and font size.
    last: Option<(Point, f64)>,
    /// The usual distance between glyph origins on this line.
    advance: Option<f64>,
}

impl TextDevice {
    fn push(&mut self, text: &str, origin: Point, size: f64) {
        if let Some((last, last_size)) = self.last {
            let size = size.max(last_size).max(1.0);
            let dx = origin.x - last.x;
            if (origin.y - last.y).abs() > size * 0.5 || dx < -size {
                self.out.push('\n');
                self.advance = None;
            } else {
                let advance = self.advance.unwrap_or(size * 0.5);
                let ends_in_space = self.out.ends_with(char::is_whitespace);
                if dx > advance + size * 0.35 && !ends_in_space && !text.starts_with(' ') {
                    self.out.push(' ');
                } else if dx > 0.0 {
                    self.advance = Some(advance * 0.7 + dx * 0.3);
                }
            }
        }
        self.out.push_str(text);
        self.last = Some((origin, size));
    }

    fn finish(self) -> String {
        self.out
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string()
    }
}

impl<'a> Device<'a> for TextDevice {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'a>, _: &PathDrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_glyph(
        &mut self,
        glyph: &Glyph<'a>,
        transform: Affine,
        glyph_transform: Affine,
        _: &Paint<'a>,
        _: &GlyphDrawMode,
    ) {
        let Some(text) = glyph.as_unicode() else {
            return;
        };
        let m = transform * glyph_transform;
        // glyph space is 1000 units per em
        let size = m.determinant().abs().sqrt() * 1000.0;
        let origin = m * Point::ZERO;
        match text {
            BfString::Char(c) if !c.is_control() => {
                self.push(c.encode_utf8(&mut [0; 4]), origin, size)
            }
            BfString::Char(_) => {}
            BfString::String(s) => self.push(&s, origin, size),
        }
    }
    fn draw_image(&mut self, _: Image<'a, '_>, _: Affine) {}
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PDF with one page per entry, each line of it drawn in 24 pt
    /// Helvetica (the same writer as the parity fixture).
    pub(crate) fn sample_pdf(pages: &[&[&str]]) -> Vec<u8> {
        let mut objects: Vec<Vec<u8>> =
            vec![b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec()];
        let pages_id = 1 + 2 * pages.len() + 1;
        let mut kids = Vec::new();
        for lines in pages {
            let mut ops = String::from("BT /F1 24 Tf 72 720 Td 30 TL\n");
            for line in *lines {
                ops.push_str(&format!("({line}) Tj T*\n"));
            }
            ops.push_str("ET");
            objects.push(
                format!("<< /Length {} >>\nstream\n{ops}\nendstream", ops.len()).into_bytes(),
            );
            let content = objects.len();
            objects.push(
                format!(
                    "<< /Type /Page /Parent {pages_id} 0 R /MediaBox [0 0 612 792] \
                     /Resources << /Font << /F1 1 0 R >> >> /Contents {content} 0 R >>"
                )
                .into_bytes(),
            );
            kids.push(format!("{} 0 R", objects.len()));
        }
        objects.push(
            format!(
                "<< /Type /Pages /Kids [{}] /Count {} >>",
                kids.join(" "),
                kids.len()
            )
            .into_bytes(),
        );
        objects.push(format!("<< /Type /Catalog /Pages {pages_id} 0 R >>").into_bytes());
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, object) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n", i + 1).bytes());
            out.extend(object);
            out.extend(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for offset in offsets {
            out.extend(format!("{offset:010} 00000 n \n").bytes());
        }
        out.extend(
            format!(
                "trailer\n<< /Size {} /Root {} 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1,
                objects.len()
            )
            .bytes(),
        );
        out
    }

    #[test]
    fn opens_and_counts_pages() {
        let doc = PdfDocument::open(sample_pdf(&[&["one"], &["two"], &["three"]])).unwrap();
        assert_eq!(doc.page_count(), 3);
        assert_eq!(doc.page_size(0), Some((612.0, 792.0)));
        assert_eq!(doc.page_size(3), None);
    }

    #[test]
    fn broken_file_does_not_open() {
        assert!(matches!(
            PdfDocument::open(b"not a pdf".to_vec()),
            Err(PdfError::Invalid)
        ));
    }

    #[test]
    fn renders_within_the_longest_side() {
        let doc = PdfDocument::open(sample_pdf(&[&["Hello"]])).unwrap();
        let page = doc.render(0, 400).unwrap();
        assert_eq!((page.width, page.height), (309, 400));
        assert!(page.png.starts_with(b"\x89PNG"));
        let big = doc.render(0, 100_000).unwrap();
        assert_eq!(big.height, MAX_PAGE_PX);
    }

    #[test]
    fn extracts_lines_and_words() {
        let doc = PdfDocument::open(sample_pdf(&[
            &["Hello page one", "second line"],
            &["Page two"],
        ]))
        .unwrap();
        assert_eq!(
            doc.text().unwrap(),
            "Page 1\n\nHello page one\nsecond line\n\nPage 2\n\nPage two\n"
        );
    }

    #[test]
    fn diffs_the_text() {
        let old = "Page 1\n\nHello\nsecond line\n";
        let new = "Page 1\n\nHello\nthird line\n";
        let corvene_models::Diff::Text { hunks, warnings } = text_diff(Some(old), Some(new)) else {
            panic!("not a text diff");
        };
        assert!(warnings.pdf_text);
        let lines: Vec<_> = hunks[0]
            .lines
            .iter()
            .map(|l| (l.kind, l.text.as_str()))
            .collect();
        use corvene_models::DiffLineKind::*;
        assert_eq!(
            lines,
            [
                (Hunk, "@@ -1,4 +1,4 @@"),
                (Context, "Page 1"),
                (Context, ""),
                (Context, "Hello"),
                (Delete, "second line"),
                (Add, "third line"),
            ]
        );
        assert_eq!(text_diff(Some(old), Some(old)), corvene_models::Diff::Empty);
        assert!(matches!(
            text_diff(None, Some(new)),
            corvene_models::Diff::Text { .. }
        ));
    }

    #[test]
    fn finds_changed_pages() {
        let old = PdfDocument::open(sample_pdf(&[&["same"], &["before"]])).unwrap();
        let new = PdfDocument::open(sample_pdf(&[&["same"], &["after"], &["added"]])).unwrap();
        assert_eq!(
            changed_pages(Some(&old), Some(&new)),
            Some(vec![false, true, true])
        );
        assert_eq!(
            changed_pages(None, Some(&new)),
            Some(vec![true, true, true])
        );
    }
}
