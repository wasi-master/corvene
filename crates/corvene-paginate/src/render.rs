//! Laid out pages written as PDF with krilla (fonts subset and embedded,
//! pictures as image objects).

use corvene_models::ImageBlob;
use krilla::color::rgb;
use krilla::geom::{PathBuilder, Point, Rect, Size, Transform};
use krilla::image::Image;
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::{Fill, Stroke};
use krilla::surface::Surface;

use crate::layout::{Item, Page};

/// A document's pictures decoded for embedding (`None`: not drawable).
pub struct Images(Vec<Option<Image>>);

impl Images {
    pub fn new(blobs: &[ImageBlob]) -> Self {
        Images(blobs.iter().map(decode).collect())
    }

    pub fn sizes(&self) -> Vec<Option<(u32, u32)>> {
        self.0.iter().map(|i| i.as_ref().map(Image::size)).collect()
    }
}

fn decode(blob: &ImageBlob) -> Option<Image> {
    let data = || krilla::Data::from(blob.bytes.clone());
    let bytes = &blob.bytes;
    if bytes.starts_with(b"\x89PNG") {
        Image::from_png(data(), true).ok()
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Image::from_jpeg(data(), true).ok()
    } else if bytes.starts_with(b"GIF8") {
        Image::from_gif(data(), true).ok()
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Image::from_webp(data(), true).ok()
    } else if bytes.starts_with(b"BM") {
        let decoded = image::load_from_memory_with_format(bytes, image::ImageFormat::Bmp).ok()?;
        let rgba = decoded.to_rgba8();
        let (w, h) = rgba.dimensions();
        Some(Image::from_rgba8(rgba.into_raw(), w, h))
    } else {
        None
    }
}

fn fill(color: [u8; 3]) -> Fill {
    Fill {
        paint: rgb::Color::new(color[0], color[1], color[2]).into(),
        opacity: NormalizedF32::ONE,
        rule: Default::default(),
    }
}

fn stroke(color: [u8; 3], width: f32) -> Stroke {
    Stroke {
        paint: rgb::Color::new(color[0], color[1], color[2]).into(),
        width,
        ..Stroke::default()
    }
}

fn rect_path(rect: [f32; 4]) -> Option<krilla::geom::Path> {
    let mut pb = PathBuilder::new();
    pb.push_rect(Rect::from_xywh(rect[0], rect[1], rect[2], rect[3])?);
    pb.finish()
}

fn fill_rect(surface: &mut Surface, rect: [f32; 4], color: [u8; 3]) {
    if let Some(path) = rect_path(rect) {
        surface.set_stroke(None);
        surface.set_fill(Some(fill(color)));
        surface.draw_path(&path);
    }
}

/// The pages as a PDF file.
pub fn pdf(pages: &[Page], images: &Images) -> Result<Vec<u8>, String> {
    let mut document = krilla::Document::new();
    for page in pages {
        let settings = PageSettings::from_wh(page.width, page.height)
            .ok_or_else(|| "The page size is not valid.".to_string())?;
        let mut p = document.start_page_with(settings);
        let mut surface = p.surface();
        fill_rect(
            &mut surface,
            [0.0, 0.0, page.width, page.height],
            [255, 255, 255],
        );
        for item in &page.items {
            draw(&mut surface, item, images);
        }
        surface.finish();
        p.finish();
    }
    document
        .finish()
        .map_err(|e| format!("The PDF could not be written: {e:?}"))
}

fn draw(surface: &mut Surface, item: &Item, images: &Images) {
    match item {
        Item::Fill { rect, color } => fill_rect(surface, *rect, *color),
        Item::Frame { rect, color, width } => {
            if let Some(path) = rect_path(*rect) {
                surface.set_fill(None);
                surface.set_stroke(Some(stroke(*color, *width)));
                surface.draw_path(&path);
                surface.set_stroke(None);
            }
        }
        Item::Image { rect, index } => {
            let Some(Some(image)) = images.0.get(*index) else {
                return;
            };
            let Some(size) = Size::from_wh(rect[2].max(0.1), rect[3].max(0.1)) else {
                return;
            };
            surface.push_transform(&Transform::from_translate(rect[0], rect[1]));
            surface.draw_image(image.clone(), size);
            surface.pop();
        }
        Item::Text { x, baseline, run } => {
            let y = baseline - run.rise;
            let face = &run.face;
            let slanted = face.fake_italic;
            if slanted {
                // slant about the baseline
                surface.push_transform(&Transform::from_row(1.0, 0.0, -0.2, 1.0, 0.2 * y, 0.0));
            }
            surface.set_fill(Some(fill(run.color)));
            surface.set_stroke(face.fake_bold.then(|| stroke(run.color, run.size * 0.03)));
            surface.draw_glyphs(
                Point::from_xy(*x, y),
                &run.glyphs,
                face.font.clone(),
                &run.text,
                run.size,
                false,
            );
            surface.set_stroke(None);
            let thick = (run.size * 0.06).max(0.5);
            if run.underline {
                fill_rect(
                    surface,
                    [*x, y + run.size * 0.12, run.width, thick],
                    run.color,
                );
            }
            if run.strike {
                fill_rect(
                    surface,
                    [*x, y - run.size * 0.3, run.width, thick],
                    run.color,
                );
            }
            if slanted {
                surface.pop();
            }
        }
    }
}
