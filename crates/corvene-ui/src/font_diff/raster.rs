//! Rasters of glyphs and text, drawn by `corvene_fonts::render` on the
//! background executor and kept as GPUI images until the view goes away.
//!
//! A raster is asked for by a key that names everything it depends on
//! (side, face, what is drawn, size in device pixels, ink); the first ask
//! starts the job, later asks get `None` until it is done, then the image.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use corvene_fonts::render::{Ink, Raster};
use gpui_kit::*;

/// At most this many images are kept; the oldest go first.
const MAX_IMAGES: usize = 4096;

/// What a raster depends on. Built by hashing whatever the caller passes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RasterKey(u64);

impl RasterKey {
    pub fn of(parts: impl Hash) -> Self {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        parts.hash(&mut h);
        Self(h.finish())
    }
}

/// A drawn raster with its size in logical pixels.
#[derive(Clone)]
pub struct Drawn {
    pub image: Arc<RenderImage>,
    pub width: Pixels,
    pub height: Pixels,
    /// The raster itself, for comparisons drawn from two rasters.
    pub raster: Arc<Raster>,
}

#[derive(Default)]
pub struct RasterCache {
    done: HashMap<RasterKey, Drawn>,
    order: Vec<RasterKey>,
    pending: HashSet<RasterKey>,
}

/// A GPUI image of `raster`, shown at `scale` device pixels per point.
pub fn to_image(raster: &Raster, scale: f32) -> Option<Drawn> {
    let buffer = image::RgbaImage::from_raw(raster.width, raster.height, raster.bgra())?;
    let frame = image::Frame::new(buffer);
    Some(Drawn {
        image: Arc::new(RenderImage::new(vec![frame])),
        width: px(raster.width as f32 / scale),
        height: px(raster.height as f32 / scale),
        raster: Arc::new(raster.clone()),
    })
}

impl RasterCache {
    /// The raster for `key`, or `None` while `job` runs (started on the
    /// first ask). `on_done` is the view to notify.
    pub fn get<V: RasterHost + 'static>(
        &mut self,
        key: RasterKey,
        scale: f32,
        job: impl FnOnce() -> Option<Raster> + Send + 'static,
        cx: &mut Context<V>,
    ) -> Option<Drawn> {
        if let Some(d) = self.done.get(&key) {
            return Some(d.clone());
        }
        if !self.pending.insert(key) {
            return None;
        }
        let task = cx
            .background_executor()
            .spawn(async move { job().and_then(|r| to_image(&r, scale)) });
        cx.spawn(async move |this, cx| {
            let drawn = task.await;
            this.update(cx, |this, cx| {
                let cache = this.rasters();
                cache.pending.remove(&key);
                if let Some(drawn) = drawn {
                    cache.insert(key, drawn);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        None
    }

    /// Stores an image made on the main thread.
    pub fn insert(&mut self, key: RasterKey, drawn: Drawn) {
        if self.done.insert(key, drawn).is_none() {
            self.order.push(key);
        }
        if self.order.len() > MAX_IMAGES {
            let drop = self.order.len() - MAX_IMAGES;
            for old in self.order.drain(..drop) {
                self.done.remove(&old);
            }
        }
    }

    /// The finished raster for `key`, without starting anything.
    pub fn peek(&self, key: RasterKey) -> Option<Drawn> {
        self.done.get(&key).cloned()
    }

    pub fn clear(&mut self) {
        self.done.clear();
        self.order.clear();
        self.pending.clear();
    }
}

/// A view that owns a [`RasterCache`].
pub trait RasterHost {
    fn rasters(&mut self) -> &mut RasterCache;
}

/// A theme colour as raster ink.
pub fn ink(color: Hsla) -> Ink {
    let rgba = color.to_rgb();
    let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    Ink::rgba(byte(rgba.r), byte(rgba.g), byte(rgba.b), byte(rgba.a))
}

/// `color` at `alpha` opacity.
pub fn ink_alpha(color: Hsla, alpha: f32) -> Ink {
    ink(color.opacity(alpha))
}

/// An image element of `drawn` at its logical size.
pub fn image_el(drawn: &Drawn) -> Img {
    img(ImageSource::Render(drawn.image.clone()))
        .w(drawn.width)
        .h(drawn.height)
        .flex_none()
}
