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
//!
//! [`match_pages`] pairs the pages of the two versions the way a line diff
//! pairs lines (identical drawings first, then the most alike text or ink
//! in between), so an inserted page shows as added instead of every later
//! page as changed. The page strip of `1359-page-strip` lists these pairs.

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

    /// Page `index` as straight RGBA pixels (width, height, bytes) on
    /// white, its longer side `longest_px` (at most [`MAX_PAGE_PX`]); for
    /// thumbnails.
    pub fn render_pixels(&self, index: usize, longest_px: u32) -> Option<(u32, u32, Vec<u8>)> {
        self.render_rgba(index, longest_px.min(MAX_PAGE_PX))
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
        let cache = InterpreterCache::new();
        let mut out = String::new();
        for i in 0..pages.len() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&format!("Page {}\n\n", i + 1));
            let text = self.page_text_with(i, &cache);
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

    /// The text of page `index` (empty past the last page).
    pub fn page_text(&self, index: usize) -> String {
        self.page_text_with(index, &InterpreterCache::new())
    }

    fn page_text_with<'a>(&'a self, index: usize, cache: &InterpreterCache<'a>) -> String {
        let pages = self.pdf.pages();
        let Some(page) = pages.get(index) else {
            return String::new();
        };
        let (w, h) = page.render_dimensions();
        let mut context = Context::new(
            Affine::IDENTITY,
            Rect::new(0.0, 0.0, f64::from(w.max(1.0)), f64::from(h.max(1.0))),
            cache,
            page.xref(),
            InterpreterSettings::default(),
        );
        let mut device = TextDevice::default();
        interpret_page(page, &mut context, &mut device);
        device.finish()
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
    match line_diff(old, new) {
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

/// A line diff of two extracted texts (`None`: the side does not exist);
/// `Empty` when the text is the same. Shared with `1324-document-diff`.
pub fn line_diff(old: Option<&str>, new: Option<&str>) -> corvene_models::Diff {
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
    corvene_git::parse_unified(&patch)
}

/// How the two versions of a page compare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageStatus {
    /// Drawn the same.
    Same,
    /// Drawn differently.
    Changed,
    /// Only the new version has it.
    Added,
    /// Only the old version has it.
    Removed,
    /// Not compared (yet, or the document is too long).
    Unknown,
}

/// A page of the old version and the page of the new one it became.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PagePair {
    pub previous: Option<usize>,
    pub current: Option<usize>,
    pub status: PageStatus,
}

impl PagePair {
    /// Changed, added or removed.
    pub fn is_change(&self) -> bool {
        matches!(
            self.status,
            PageStatus::Changed | PageStatus::Added | PageStatus::Removed
        )
    }
}

/// Pages paired by number (the pairs before [`match_pages`] is done, or
/// past [`MAX_SCANNED_PAGES`]); pages on both sides are `Unknown`.
pub fn pairs_by_number(previous: usize, current: usize) -> Vec<PagePair> {
    (0..previous.max(current))
        .map(|i| {
            let (old, new) = ((i < previous).then_some(i), (i < current).then_some(i));
            let status = match (old, new) {
                (Some(_), Some(_)) => PageStatus::Unknown,
                (None, _) => PageStatus::Added,
                (_, None) => PageStatus::Removed,
            };
            PagePair {
                previous: old,
                current: new,
                status,
            }
        })
        .collect()
}

/// The longest side of the thumbnails compared to pair pages.
const MATCH_THUMB_PX: u32 = THUMB_PX;
/// Two pages count as the same page edited when they are more alike than
/// this (0 to 1).
const MIN_LIKENESS: f64 = 0.3;
/// A run of differing pages longer than this (old × new) pairs by position.
const MAX_ALIGN_CELLS: usize = 40_000;

/// What a page is compared by.
struct PageSig {
    pixels: Option<(u32, u32, Vec<u8>)>,
    key: PageKey,
    words: std::collections::HashMap<String, u32>,
    word_count: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum PageKey {
    Pixels(u64),
    /// Not drawn: equal to no other page.
    Unique(bool, usize),
}

fn page_sig(doc: &PdfDocument, i: usize, new: bool) -> PageSig {
    use std::hash::{Hash, Hasher};
    let pixels = doc.render_rgba(i, MATCH_THUMB_PX);
    let key = match &pixels {
        Some((w, h, rgba)) => {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            (w, h).hash(&mut hasher);
            for b in rgba {
                (b >> 4).hash(&mut hasher);
            }
            PageKey::Pixels(hasher.finish())
        }
        None => PageKey::Unique(new, i),
    };
    let mut words = std::collections::HashMap::new();
    let mut word_count = 0;
    let text = doc.page_text(i).to_lowercase();
    for word in text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        *words.entry(word.to_string()).or_insert(0) += 1;
        word_count += 1;
    }
    PageSig {
        pixels,
        key,
        words,
        word_count,
    }
}

/// How alike two pages are, 0 to 1: their words when both have text,
/// else their ink.
fn likeness(a: &PageSig, b: &PageSig) -> f64 {
    if a.word_count > 0 && b.word_count > 0 {
        let common: u32 = a
            .words
            .iter()
            .map(|(w, n)| (*n).min(b.words.get(w).copied().unwrap_or(0)))
            .sum();
        return 2.0 * f64::from(common) / f64::from(a.word_count + b.word_count);
    }
    match (&a.pixels, &b.pixels) {
        (Some((aw, ah, a)), Some((bw, bh, b))) if aw == bw && ah == bh => {
            let gray = |p: &[u8; 4]| (u16::from(p[0]) + u16::from(p[1]) + u16::from(p[2])) / 3;
            let (mut ink, mut differ) = (0u32, 0u32);
            for (p, q) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0) {
                let (g, h) = (gray(p), gray(q));
                if g < 224 || h < 224 {
                    ink += 1;
                    if g.abs_diff(h) > 48 {
                        differ += 1;
                    }
                }
            }
            if ink == 0 {
                1.0
            } else {
                1.0 - f64::from(differ) / f64::from(ink)
            }
        }
        _ => 0.0,
    }
}

/// Pairs the pages of the two versions (`None` past
/// [`MAX_SCANNED_PAGES`]): pages drawn identically pair as in a line diff,
/// and between them the most alike pages pair as changed (or the same, when
/// their drawings are within a rounding step); the rest are added or
/// removed.
pub fn match_pages(old: Option<&PdfDocument>, new: Option<&PdfDocument>) -> Option<Vec<PagePair>> {
    let count = |doc: Option<&PdfDocument>| doc.map_or(0, PdfDocument::page_count);
    let (old_count, new_count) = (count(old), count(new));
    let (Some(old), Some(new)) = (old, new) else {
        return Some(pairs_by_number(old_count, new_count));
    };
    if old_count.max(new_count) > MAX_SCANNED_PAGES {
        return None;
    }
    let olds: Vec<PageSig> = (0..old_count).map(|i| page_sig(old, i, false)).collect();
    let news: Vec<PageSig> = (0..new_count).map(|i| page_sig(new, i, true)).collect();
    Some(pair_sigs(&olds, &news))
}

fn pair_sigs(olds: &[PageSig], news: &[PageSig]) -> Vec<PagePair> {
    let old_keys: Vec<PageKey> = olds.iter().map(|s| s.key).collect();
    let new_keys: Vec<PageKey> = news.iter().map(|s| s.key).collect();
    let ops = similar::capture_diff_slices(similar::Algorithm::Myers, &old_keys, &new_keys);
    let mut out = Vec::new();
    // the differing run since the last equal pages
    let (mut old_run, mut new_run) = (0..0, 0..0);
    let flush = |out: &mut Vec<PagePair>,
                 old_run: &mut std::ops::Range<usize>,
                 new_run: &mut std::ops::Range<usize>| {
        align_run(olds, news, old_run.clone(), new_run.clone(), out);
        *old_run = old_run.end..old_run.end;
        *new_run = new_run.end..new_run.end;
    };
    for op in ops {
        match op {
            similar::DiffOp::Equal {
                old_index,
                new_index,
                len,
            } => {
                flush(&mut out, &mut old_run, &mut new_run);
                out.extend((0..len).map(|k| PagePair {
                    previous: Some(old_index + k),
                    current: Some(new_index + k),
                    status: PageStatus::Same,
                }));
                old_run = old_index + len..old_index + len;
                new_run = new_index + len..new_index + len;
            }
            other => {
                let (o, n) = (other.old_range(), other.new_range());
                if old_run.is_empty() {
                    old_run = o.start..o.start;
                }
                if new_run.is_empty() {
                    new_run = n.start..n.start;
                }
                old_run.end = old_run.end.max(o.end);
                new_run.end = new_run.end.max(n.end);
            }
        }
    }
    flush(&mut out, &mut old_run, &mut new_run);
    out
}

/// Pairs a run of pages that differ between the versions: the pairing
/// with the most likeness (each pair above [`MIN_LIKENESS`]), in order.
fn align_run(
    olds: &[PageSig],
    news: &[PageSig],
    old_run: std::ops::Range<usize>,
    new_run: std::ops::Range<usize>,
    out: &mut Vec<PagePair>,
) {
    let (n, m) = (old_run.len(), new_run.len());
    if n == 0 && m == 0 {
        return;
    }
    let pair = |i: usize, j: usize| {
        let (a, b) = (&olds[i], &news[j]);
        let same = match (&a.pixels, &b.pixels) {
            (Some(p), Some(q)) => same_pixels(p, q),
            _ => false,
        };
        PagePair {
            previous: Some(i),
            current: Some(j),
            status: if same {
                PageStatus::Same
            } else {
                PageStatus::Changed
            },
        }
    };
    let removed = |i: usize| PagePair {
        previous: Some(i),
        current: None,
        status: PageStatus::Removed,
    };
    let added = |j: usize| PagePair {
        previous: None,
        current: Some(j),
        status: PageStatus::Added,
    };
    // the matched (old, new) pages, in order
    let matched: Vec<(usize, usize)> = if n * m > MAX_ALIGN_CELLS {
        (0..n.min(m)).map(|k| (k, k)).collect()
    } else {
        let mut score = vec![vec![0.0f64; m + 1]; n + 1];
        for i in 1..=n {
            for j in 1..=m {
                let gain = likeness(&olds[old_run.start + i - 1], &news[new_run.start + j - 1]);
                let mut best = score[i - 1][j].max(score[i][j - 1]);
                if gain > MIN_LIKENESS {
                    best = best.max(score[i - 1][j - 1] + gain - MIN_LIKENESS);
                }
                score[i][j] = best;
            }
        }
        let mut matched = Vec::new();
        let (mut i, mut j) = (n, m);
        while i > 0 && j > 0 {
            if score[i][j] == score[i - 1][j] {
                i -= 1;
            } else if score[i][j] == score[i][j - 1] {
                j -= 1;
            } else {
                matched.push((i - 1, j - 1));
                i -= 1;
                j -= 1;
            }
        }
        matched.reverse();
        matched
    };
    let (mut i, mut j) = (0, 0);
    for (a, b) in matched.into_iter().chain([(n, m)]) {
        out.extend((i..a).map(|k| removed(old_run.start + k)));
        out.extend((j..b).map(|k| added(new_run.start + k)));
        if a < n && b < m {
            out.push(pair(old_run.start + a, new_run.start + b));
        }
        (i, j) = (a + 1, b + 1);
    }
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
    fn pairs_pages_around_an_inserted_one() {
        let old = PdfDocument::open(sample_pdf(&[
            &["Title"],
            &["Revenue grew 12% year on year", "Costs were flat"],
            &["Figures"],
            &["Appendix"],
        ]))
        .unwrap();
        let new = PdfDocument::open(sample_pdf(&[
            &["Title"],
            &["Revenue grew 14% year on year", "Costs were flat"],
            &["Inserted page", "This page is new"],
            &["Figures"],
            &["Appendix"],
        ]))
        .unwrap();
        let pairs: Vec<_> = match_pages(Some(&old), Some(&new))
            .unwrap()
            .into_iter()
            .map(|p| (p.previous, p.current, p.status))
            .collect();
        use PageStatus::*;
        assert_eq!(
            pairs,
            [
                (Some(0), Some(0), Same),
                (Some(1), Some(1), Changed),
                (None, Some(2), Added),
                (Some(2), Some(3), Same),
                (Some(3), Some(4), Same),
            ]
        );
    }

    #[test]
    fn unlike_pages_are_removed_and_added() {
        let old = PdfDocument::open(sample_pdf(&[&["same"], &["alpha beta gamma"]])).unwrap();
        let new =
            PdfDocument::open(sample_pdf(&[&["same"], &["delta epsilon"], &["zeta"]])).unwrap();
        let statuses: Vec<_> = match_pages(Some(&old), Some(&new))
            .unwrap()
            .into_iter()
            .map(|p| p.status)
            .collect();
        use PageStatus::*;
        assert_eq!(statuses, [Same, Removed, Added, Added]);
        let all_new: Vec<_> = match_pages(None, Some(&new))
            .unwrap()
            .into_iter()
            .map(|p| p.status)
            .collect();
        assert_eq!(all_new, [Added, Added, Added]);
    }

    #[test]
    fn pairs_by_number_marks_the_longer_side() {
        let pairs = pairs_by_number(1, 2);
        assert_eq!(pairs[0].status, PageStatus::Unknown);
        assert_eq!(
            pairs[1],
            PagePair {
                previous: None,
                current: Some(1),
                status: PageStatus::Added
            }
        );
    }
}
