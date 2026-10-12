//! Blocks placed on pages: a text document or book flows down pages of its
//! size and margins (headings kept with what follows, paragraphs and table
//! rows split between pages, a chapter or page break starting a new page);
//! a slide is one page of the slide size with each shape's blocks in the
//! shape's rectangle, shrunk until they fit as PowerPoint's autofit would.

use corvene_models::{DocBlob, DocBlock, DocKind, DocRun, DocSection, ListMark};

use crate::fonts::Family;
use crate::text::{self, GlyphRun, Line, TextStyle};

/// Something drawn on a page, in points from the top left.
pub enum Item {
    Text {
        x: f32,
        baseline: f32,
        run: GlyphRun,
    },
    Fill {
        rect: [f32; 4],
        color: [u8; 3],
    },
    Frame {
        rect: [f32; 4],
        color: [u8; 3],
        width: f32,
    },
    Image {
        rect: [f32; 4],
        index: usize,
    },
}

pub struct Page {
    pub width: f32,
    pub height: f32,
    pub items: Vec<Item>,
}

/// Page size and margins in points: width, height, [top, right, bottom,
/// left].
#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub width: f32,
    pub height: f32,
    pub margins: [f32; 4],
}

const BORDER: [u8; 3] = [0xd0, 0xd7, 0xde];
const SUBTLE: [u8; 3] = [0xf6, 0xf8, 0xfa];
const MUTED: [u8; 3] = [0x59, 0x63, 0x6e];
const PLACEHOLDER: [u8; 3] = [0xea, 0xee, 0xf2];

/// The sizes of a document's images in pixels (`None`: not drawable).
pub type ImageSizes = [Option<(u32, u32)>];

/// Lay out a whole document. `progress(done, total)` after each block.
pub fn pages(
    doc: &DocBlob,
    geometry: Geometry,
    images: &ImageSizes,
    progress: &mut dyn FnMut(usize, usize),
) -> Vec<Page> {
    let total: usize = doc.sections.iter().map(|s| s.blocks.len()).sum();
    let mut done = 0;
    let mut out = Vec::new();
    match doc.kind {
        DocKind::Presentation => {
            for section in doc.sections.iter().filter(|s| !s.hidden) {
                out.push(slide(section, geometry, images));
                done += section.blocks.len();
                progress(done, total);
            }
        }
        DocKind::Text | DocKind::Book => {
            let mut flow = Flow::new(geometry, true, 1.0, images);
            for (i, section) in doc.sections.iter().enumerate() {
                if i > 0 {
                    flow.break_page();
                }
                for block in &section.blocks {
                    flow.block(block, doc.kind);
                    done += 1;
                    if done % 16 == 0 {
                        progress(done, total);
                    }
                }
            }
            progress(done, total);
            out = flow.finish();
        }
    }
    if out.is_empty() {
        out.push(Page {
            width: geometry.width,
            height: geometry.height,
            items: Vec::new(),
        });
    }
    out
}

/// Blocks flowing down a column, onto new pages when `paginate`.
struct Flow<'a> {
    geometry: Geometry,
    paginate: bool,
    /// Font sizes are multiplied by this (a slide's autofit).
    scale: f32,
    /// Body text size before scaling.
    body: f32,
    images: &'a ImageSizes,
    pages: Vec<Page>,
    items: Vec<Item>,
    left: f32,
    width: f32,
    top: f32,
    bottom: f32,
    y: f32,
    /// Nothing placed on the current page yet.
    fresh: bool,
    /// A slide: larger type, bullets sized for the slide.
    slide: bool,
}

impl<'a> Flow<'a> {
    fn new(geometry: Geometry, paginate: bool, scale: f32, images: &'a ImageSizes) -> Self {
        let [mt, mr, mb, ml] = geometry.margins;
        Flow {
            geometry,
            paginate,
            scale,
            body: 11.0,
            images,
            pages: Vec::new(),
            items: Vec::new(),
            left: ml,
            width: (geometry.width - ml - mr).max(36.0),
            top: mt,
            bottom: if paginate {
                (geometry.height - mb).max(mt + 36.0)
            } else {
                f32::INFINITY
            },
            y: mt,
            fresh: true,
            slide: false,
        }
    }

    /// A column inside a slide shape (no pages).
    fn in_box(rect: [f32; 4], body: f32, scale: f32, images: &'a ImageSizes) -> Self {
        let geometry = Geometry {
            width: rect[0] + rect[2],
            height: rect[1] + rect[3],
            margins: [rect[1], 0.0, 0.0, rect[0]],
        };
        let mut flow = Flow::new(geometry, false, scale, images);
        flow.body = body;
        flow.slide = true;
        flow
    }

    fn size(&self, points: f32) -> f32 {
        points * self.scale
    }

    fn new_page(&mut self) {
        if !self.paginate {
            return;
        }
        self.pages.push(Page {
            width: self.geometry.width,
            height: self.geometry.height,
            items: std::mem::take(&mut self.items),
        });
        self.y = self.top;
        self.fresh = true;
    }

    /// A page or chapter break.
    fn break_page(&mut self) {
        if !self.fresh {
            self.new_page();
        }
    }

    /// Room for `height` more on this page (a new page when not, unless
    /// the page is still empty).
    fn ensure(&mut self, height: f32) {
        if !self.fresh && self.y + height > self.bottom {
            self.new_page();
        }
    }

    fn gap(&mut self, height: f32) {
        if !self.fresh {
            self.y += height;
        }
    }

    fn finish(mut self) -> Vec<Page> {
        if !self.items.is_empty() || self.pages.is_empty() {
            self.new_page();
        }
        self.pages
    }

    fn place_line(&mut self, line: &Line, x: f32) {
        self.ensure(line.height);
        let baseline = self.y + line.baseline;
        for run in &line.runs {
            self.items.push(Item::Text {
                x: x + run.x,
                baseline,
                run: run.clone(),
            });
        }
        self.y += line.height;
        self.fresh = false;
    }

    fn block(&mut self, block: &DocBlock, kind: DocKind) {
        match block {
            DocBlock::Heading { level, runs } => self.heading(*level, runs),
            DocBlock::Paragraph { runs, list, quote } => self.paragraph(runs, *list, *quote),
            DocBlock::Table { rows } => self.table(rows),
            DocBlock::Image { index, alt } => self.image(*index, alt),
            DocBlock::Code(code) => self.code(code),
            DocBlock::Break => {
                if kind != DocKind::Presentation {
                    self.break_page();
                }
            }
        }
    }

    fn heading(&mut self, level: u8, runs: &[DocRun]) {
        let factor = if self.slide {
            match level {
                1 => 1.65,
                2 => 1.0,
                _ => 1.15,
            }
        } else {
            match level {
                1 => 1.7,
                2 => 1.4,
                3 => 1.2,
                _ => 1.05,
            }
        };
        let size = self.size(self.body * factor);
        let style = TextStyle {
            bold: true,
            ..TextStyle::body(size)
        };
        let lines = text::lay_out(runs, style, self.width);
        self.gap(size * 0.7);
        // keep the heading with two lines of what follows
        let body_line = self.size(self.body) * 1.3;
        let total: f32 = lines.iter().map(|l| l.height).sum::<f32>() + body_line * 2.0;
        if self.paginate && total < self.bottom - self.top {
            self.ensure(total);
        }
        for line in &lines {
            self.place_line(line, self.left);
        }
        self.y += size * 0.25;
    }

    fn paragraph(&mut self, runs: &[DocRun], list: Option<ListMark>, quote: bool) {
        let mut size = self.size(self.body);
        if self.slide
            && let Some(mark) = list
        {
            size = self.size(self.body - 2.0 * f32::from(mark.level.min(4)));
        }
        let style = TextStyle {
            color: if quote {
                MUTED
            } else {
                TextStyle::body(size).color
            },
            ..TextStyle::body(size)
        };
        let mut indent = 0.0;
        if quote {
            indent += size * 1.2;
        }
        let marker = list.map(|mark| {
            let step = size * 1.6;
            indent += step * f32::from(mark.level) + step;
            let text = if mark.ordered {
                format!("{}.", mark.number)
            } else {
                ["\u{2022}", "\u{25e6}", "\u{25aa}"][usize::from(mark.level) % 3].to_string()
            };
            text::lay_out(&[DocRun::plain(text)], style, step)
        });
        let lines = text::lay_out(runs, style, (self.width - indent).max(size * 4.0));
        let start_y = self.y;
        let mut first_page_y = None;
        for (i, line) in lines.iter().enumerate() {
            self.place_line(line, self.left + indent);
            if i == 0 {
                let top = self.y - line.height;
                first_page_y = Some(top);
                if let Some(marker) = marker.as_ref().and_then(|m| m.first()) {
                    let x = self.left + indent - size * 1.6;
                    for run in &marker.runs {
                        self.items.push(Item::Text {
                            x: x + run.x,
                            baseline: top + line.baseline,
                            run: run.clone(),
                        });
                    }
                }
            }
        }
        if quote {
            let top = first_page_y.unwrap_or(start_y);
            if self.y > top {
                self.items.push(Item::Fill {
                    rect: [self.left, top, size * 0.22, self.y - top],
                    color: BORDER,
                });
            }
        }
        self.y += if self.slide { size * 0.35 } else { size * 0.55 };
    }

    fn code(&mut self, code: &str) {
        let size = self.size(self.body * 0.9);
        let style = TextStyle {
            family: Family::Mono,
            leading: 1.35,
            ..TextStyle::body(size)
        };
        let pad = size * 0.6;
        self.gap(size * 0.3);
        let lines = text::lay_out(&[DocRun::plain(code)], style, self.width - pad * 2.0);
        for line in &lines {
            self.ensure(line.height);
            self.items.push(Item::Fill {
                rect: [self.left, self.y, self.width, line.height],
                color: SUBTLE,
            });
            self.place_line(line, self.left + pad);
        }
        self.y += size * 0.8;
    }

    fn image(&mut self, index: Option<usize>, alt: &str) {
        let size = index.and_then(|i| self.images.get(i).copied().flatten());
        let max_h = if self.paginate {
            self.bottom - self.top
        } else {
            f32::INFINITY
        };
        let (w, h) = match size {
            Some((w, h)) if w > 0 && h > 0 => {
                // 96 dpi pixels in points, at most the column
                let (w, h) = (w as f32 * 0.75 * self.scale, h as f32 * 0.75 * self.scale);
                let fit = (self.width / w).min(max_h / h).min(1.0);
                (w * fit, h * fit)
            }
            _ => (self.width.min(self.size(160.0)), self.size(90.0)),
        };
        self.gap(self.size(4.0));
        self.ensure(h);
        let rect = [self.left, self.y, w, h];
        match (index, size) {
            (Some(index), Some(_)) => self.items.push(Item::Image { rect, index }),
            _ => {
                self.items.push(Item::Fill {
                    rect,
                    color: PLACEHOLDER,
                });
                let label = if alt.is_empty() { "Picture" } else { alt };
                let style = TextStyle {
                    color: MUTED,
                    ..TextStyle::body(self.size(self.body * 0.85))
                };
                let pad = self.size(6.0);
                let lines = text::lay_out(&[DocRun::plain(label)], style, w - pad * 2.0);
                let mut y = self.y + pad;
                let bottom = self.y + h;
                for line in &lines {
                    if y + line.height > bottom {
                        break;
                    }
                    for run in &line.runs {
                        self.items.push(Item::Text {
                            x: self.left + pad + run.x,
                            baseline: y + line.baseline,
                            run: run.clone(),
                        });
                    }
                    y += line.height;
                }
            }
        }
        self.y += h + self.size(6.0);
        self.fresh = false;
    }

    fn table(&mut self, rows: &[Vec<Vec<DocRun>>]) {
        let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
        if columns == 0 {
            return;
        }
        let size = self.size(self.body * if self.slide { 0.75 } else { 0.95 });
        let pad_x = size * 0.45;
        let pad_y = size * 0.3;
        let style = TextStyle::body(size);
        let header = TextStyle {
            bold: true,
            ..style
        };
        // column widths from the cells' natural and longest-word widths
        let mut natural = vec![0f32; columns];
        let mut least = vec![0f32; columns];
        for (r, row) in rows.iter().enumerate() {
            for (c, cell) in row.iter().enumerate() {
                let (n, l) = text::measure(cell, if r == 0 { header } else { style });
                natural[c] = natural[c].max(n + pad_x * 2.0);
                least[c] = least[c].max(l + pad_x * 2.0);
            }
        }
        let widths = column_widths(&natural, &least, self.width);
        self.gap(size * 0.4);
        for (r, row) in rows.iter().enumerate() {
            let cells: Vec<Vec<Line>> = (0..columns)
                .map(|c| {
                    let runs = row.get(c).map(Vec::as_slice).unwrap_or(&[]);
                    text::lay_out(
                        runs,
                        if r == 0 { header } else { style },
                        (widths[c] - pad_x * 2.0).max(1.0),
                    )
                })
                .collect();
            self.table_row(&cells, &widths, r == 0, pad_x, pad_y);
        }
        self.y += size * 0.6;
    }

    /// One row, split between pages when it does not fit on one.
    fn table_row(&mut self, cells: &[Vec<Line>], widths: &[f32], header: bool, px: f32, py: f32) {
        let height = |lines: &[Line]| lines.iter().map(|l| l.height).sum::<f32>();
        let full = cells.iter().map(|c| height(c)).fold(0.0, f32::max) + py * 2.0;
        if self.paginate && full <= self.bottom - self.top {
            self.ensure(full);
        }
        let mut next = vec![0usize; cells.len()];
        loop {
            let room = self.bottom - self.y - py * 2.0;
            // the lines of each cell that fit here
            let mut take = vec![0usize; cells.len()];
            let mut used = 0f32;
            for (c, lines) in cells.iter().enumerate() {
                let mut h = 0.0;
                for line in &lines[next[c]..] {
                    if h + line.height > room && (h > 0.0 || !self.fresh) {
                        break;
                    }
                    h += line.height;
                    take[c] += 1;
                }
                used = used.max(h);
            }
            let any = take.iter().any(|t| *t > 0);
            let left_over = cells
                .iter()
                .enumerate()
                .any(|(c, l)| next[c] + take[c] < l.len());
            if !any && left_over && !self.fresh {
                self.new_page();
                continue;
            }
            let row_h = used + py * 2.0;
            if header {
                self.items.push(Item::Fill {
                    rect: [self.left, self.y, widths.iter().sum(), row_h],
                    color: SUBTLE,
                });
            }
            let mut x = self.left;
            for (c, lines) in cells.iter().enumerate() {
                let mut y = self.y + py;
                for line in &lines[next[c]..next[c] + take[c]] {
                    for run in &line.runs {
                        self.items.push(Item::Text {
                            x: x + px + run.x,
                            baseline: y + line.baseline,
                            run: run.clone(),
                        });
                    }
                    y += line.height;
                }
                self.items.push(Item::Frame {
                    rect: [x, self.y, widths[c], row_h],
                    color: BORDER,
                    width: 0.75,
                });
                next[c] += take[c];
                x += widths[c];
            }
            self.y += row_h;
            self.fresh = false;
            if !left_over || !self.paginate {
                break;
            }
            self.new_page();
        }
    }
}

/// Share `width` between columns: their natural widths when they fit,
/// else at least each column's longest word and the rest by natural width.
fn column_widths(natural: &[f32], least: &[f32], width: f32) -> Vec<f32> {
    let sum: f32 = natural.iter().sum();
    if sum <= width {
        // spread the slack evenly (tables span the column, as Word's do)
        let extra = (width - sum) / natural.len() as f32;
        return natural.iter().map(|n| n + extra).collect();
    }
    let floor: f32 = least.iter().sum();
    if floor >= width {
        return least.iter().map(|l| l * width / floor).collect();
    }
    let flexible: f32 = natural.iter().zip(least).map(|(n, l)| n - l).sum();
    let room = width - floor;
    natural
        .iter()
        .zip(least)
        .map(|(n, l)| {
            if flexible > 0.0 {
                l + (n - l) * room / flexible
            } else {
                l + room / natural.len() as f32
            }
        })
        .collect()
}

/// One slide: each shape's blocks in its rectangle, the rest in the
/// slide's body area.
fn slide(section: &DocSection, geometry: Geometry, images: &ImageSizes) -> Page {
    let (w, h) = (geometry.width, geometry.height);
    // PowerPoint's default text sizes on a 7.5 inch high slide
    let body = 24.0 * h / 540.0;
    let mut items = Vec::new();
    let mut framed = vec![false; section.blocks.len()];
    for frame in &section.frames {
        let range = frame.start as usize..(frame.end as usize).min(section.blocks.len());
        if range.is_empty() {
            continue;
        }
        for f in &mut framed[range.clone()] {
            *f = true;
        }
        let r = frame.rect;
        let rect = [
            r.x as f32 / 100.0,
            r.y as f32 / 100.0,
            r.w as f32 / 100.0,
            r.h as f32 / 100.0,
        ];
        let blocks: Vec<&DocBlock> = section.blocks[range].iter().collect();
        items.extend(fill_box(&blocks, rect, body, images, true));
    }
    let rest: Vec<&DocBlock> = section
        .blocks
        .iter()
        .zip(&framed)
        .filter(|(_, f)| !**f)
        .map(|(b, _)| b)
        .collect();
    if !rest.is_empty() {
        let rect = [w * 0.06, h * 0.06, w * 0.88, h * 0.88];
        items.extend(fill_box(&rest, rect, body, images, false));
    }
    Page {
        width: w,
        height: h,
        items,
    }
}

/// A shape's blocks in `rect` (points), shrunk until they fit; a lone
/// picture fills the shape, a lone title is centred vertically.
fn fill_box(
    blocks: &[&DocBlock],
    rect: [f32; 4],
    body: f32,
    images: &ImageSizes,
    shape: bool,
) -> Vec<Item> {
    if let [DocBlock::Image { index, alt }] = blocks
        && shape
    {
        let size = index.and_then(|i| images.get(i).copied().flatten());
        return match (index, size) {
            (Some(index), Some((iw, ih))) if iw > 0 && ih > 0 => {
                let fit = (rect[2] / iw as f32).min(rect[3] / ih as f32);
                let (dw, dh) = (iw as f32 * fit, ih as f32 * fit);
                vec![Item::Image {
                    rect: [
                        rect[0] + (rect[2] - dw) / 2.0,
                        rect[1] + (rect[3] - dh) / 2.0,
                        dw,
                        dh,
                    ],
                    index: *index,
                }]
            }
            _ => {
                let mut flow = Flow::in_box(rect, body * 0.6, 1.0, images);
                flow.image(None, alt);
                flow.items
            }
        };
    }
    // PowerPoint's text box insets: 0.1 inch left and right, 0.05 top and
    // bottom
    let inner = if shape {
        [
            rect[0] + 7.2,
            rect[1] + 3.6,
            (rect[2] - 14.4).max(12.0),
            (rect[3] - 7.2).max(6.0),
        ]
    } else {
        rect
    };
    let titles = blocks.iter().all(|b| matches!(b, DocBlock::Heading { .. }));
    let mut best = None;
    for scale in [1.0, 0.9, 0.8, 0.7, 0.62, 0.55, 0.48, 0.42, 0.36, 0.3] {
        let mut flow = Flow::in_box(inner, body, scale, images);
        for block in blocks {
            flow.block(block, DocKind::Presentation);
        }
        let used = flow.y - inner[1];
        let fits = used <= inner[3] + 0.5;
        best = Some((flow.items, used));
        if fits {
            break;
        }
    }
    let Some((mut items, used)) = best else {
        return Vec::new();
    };
    if titles && used < inner[3] {
        let dy = (inner[3] - used) / 2.0;
        for item in &mut items {
            shift(item, dy);
        }
    }
    items
}

fn shift(item: &mut Item, dy: f32) {
    match item {
        Item::Text { baseline, .. } => *baseline += dy,
        Item::Fill { rect, .. } | Item::Frame { rect, .. } | Item::Image { rect, .. } => {
            rect[1] += dy
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_share_the_column() {
        let w = column_widths(&[50.0, 50.0], &[10.0, 10.0], 200.0);
        assert_eq!(w, vec![100.0, 100.0]);
        let w = column_widths(&[300.0, 100.0], &[50.0, 50.0], 200.0);
        assert!((w.iter().sum::<f32>() - 200.0).abs() < 0.01);
        assert!(w[0] > w[1]);
    }
}
