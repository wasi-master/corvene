//! Image diffs (GHD `ui/diff/image-diffs/*` + `styles/ui/_diff.scss`
//! `.panel.image`): a new or deleted image, or the modified-image switcher
//! with 2-up, Swipe, Onion Skin and Difference. Images are decoded once for
//! their dimensions; "Difference" is a CPU blend (GPUI has no `mix-blend-mode`)
//! of the two images at their on-screen relative scale, recomputed when that
//! scale changes.
//!
//! Deviation (`752-image-diff-border-outside`): the image's 1 px border sits
//! outside its fitted size (GHD's `border-box` shrinks the image by 2 px,
//! which blurs small images).
//!
//! Deviation (`753-image-diff-background`): the checkerboard behind the
//! images can be dark, or follow the app theme.
//!
//! Deviation (`755-tga-image-diff`): `.tga` files are image diffs (decoded to
//! PNG); GHD shows them as binary.
//!
//! Deviation (`754-image-diff-alignment`): images of different sizes can
//! share the top left corner instead of the centre.
//!
//! Deviation (`794-svg-image-diff`): an SVG drawing is drawn by GPUI's SVG
//! renderer (system fonts for its text, embedded images) at the device
//! pixels it covers, again on a background thread whenever that size
//! changes, so it is sharp at any size and zoom; a small drawing is
//! enlarged like `761-pixelated-small-images` enlarges small images.
//!
//! Deviation (`1326-more-image-formats`): a side decoded from a format GPUI
//! cannot draw (`ImageBlob::original`) shows the file's own size and bytes.
//! A side that cannot be drawn says so in place of the image.
//!
//! Deviation (`1327-image-diff-zoom`): zoom buttons, pinch, ⌘-scroll and
//! double-click zoom every view; dragging and scrolling pan it. Both sides
//! share the zoom and the pan (as a fraction of the image).
//!
//! Deviation (`1322-pdf-diff`): a PDF page pair (`crate::pdf_diff`) is
//! shown with its pages' sizes in points under each side and no file size
//! summary.
//!
//! Deviation (`761-pixelated-small-images`): images under 64 px are enlarged
//! by a whole factor with nearest-neighbour sampling, so pixel art and icons
//! show their pixels (GHD draws them at their natural size, tiny).
//!
//! Fix (`1320-sharp-image-difference`, desktop/desktop#19804): the
//! Difference blend is resampled to the device pixels it covers (area
//! average down, nearest neighbour up) after blending and drawn 1:1, so a
//! fractional scale (zoom, a 1.5x display) neither blurs it nor turns
//! unchanged edges grey. GHD blends two `<img>` layers the browser already
//! scaled (`difference-blend.tsx`, `.image-container` composited by
//! `translate3d`), which shows edges as differences at 1.5x.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use corvene_core::{Dispatcher, FileStatusKind, ImageBlob, ImageDiffType};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::format::format_bytes;
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::{GhdTooltip, IconButtonA11y};

/// GHD `.tab-bar` inside `.panel.image`.
#[allow(non_snake_case)]
fn TAB_BAR_WIDTH() -> Pixels {
    zpx(350.)
}
/// GHD swipe `SliderOverflow`.
#[allow(non_snake_case)]
fn SLIDER_OVERFLOW() -> Pixels {
    zpx(14.)
}

/// `755-tga-image-diff`: GPUI cannot draw TGA, so it is decoded and shown
/// as PNG.
pub const TGA_MEDIA_TYPE: &str = "image/x-tga";

/// `794-svg-image-diff`: an SVG drawing and its latest rendering.
struct Svg {
    bytes: Arc<[u8]>,
    /// Parsed with fonts on the first rendering.
    parsed: Option<Arc<ParsedSvg>>,
    /// The rendering on screen and its size in device pixels.
    render: Option<(Arc<RenderImage>, (u32, u32))>,
    /// The device pixels the drawing covered in the last frame.
    wanted: Cell<Option<(u32, u32)>>,
    pending: bool,
}

/// `794-svg-image-diff`: a drawing is rendered to at most this many pixels
/// on its longer side, and this many in all.
const MAX_SVG_SIDE: f32 = 8192.;
const MAX_SVG_AREA: f32 = 32_000_000.;

struct Side {
    /// A raster image; `None` for a drawing or an image that cannot be shown.
    image: Option<Arc<Image>>,
    /// Size of `image`'s pixels.
    bitmap: Option<(u32, u32)>,
    svg: Option<Svg>,
    /// The file's size in bytes, for the footer.
    bytes: usize,
    /// The size the image is fitted from (`None` when it could not be
    /// decoded): the natural size unless `761-pixelated-small-images`
    /// enlarged it.
    size: Option<(u32, u32)>,
    /// The file's own size in pixels, for the footer.
    natural: Option<(u32, u32)>,
    /// Why the image cannot be shown.
    error: Option<SharedString>,
}

/// `761-pixelated-small-images`: images whose longer side is under this are
/// enlarged.
const SMALL_IMAGE: u32 = 64;

/// `761-pixelated-small-images`: the whole factor that brings a small image's
/// longer side close to 256 px (at most 16×); 1 for larger images.
fn upscale_factor((w, h): (u32, u32)) -> u32 {
    let longest = w.max(h);
    if longest == 0 || longest >= SMALL_IMAGE {
        return 1;
    }
    (256 / longest).clamp(1, 16)
}

/// The size an image that cannot be shown takes, with its message.
const BROKEN_SIZE: (u32, u32) = (240, 120);

impl Side {
    fn from_blob(blob: &ImageBlob) -> Self {
        if blob.media_type == TGA_MEDIA_TYPE {
            return Self::from_tga(blob);
        }
        if blob.media_type == corvene_git::SVG_MEDIA_TYPE {
            return Self::from_svg(blob);
        }
        let format = match blob.media_type.as_str() {
            "image/jpg" | "image/jpeg" => ImageFormat::Jpeg,
            "image/gif" => ImageFormat::Gif,
            "image/webp" => ImageFormat::Webp,
            "image/bmp" => ImageFormat::Bmp,
            "image/x-icon" => ImageFormat::Ico,
            _ => ImageFormat::Png,
        };
        let bitmap = image::ImageReader::new(std::io::Cursor::new(&blob.bytes))
            .with_guessed_format()
            .ok()
            .and_then(|r| r.into_dimensions().ok());
        // `1326-more-image-formats`: a PNG made from the file
        let (bytes, natural) = match &blob.original {
            Some(original) => (
                original.bytes,
                bitmap.map(|_| (original.width, original.height)),
            ),
            None => (blob.bytes.len(), bitmap),
        };
        Self {
            image: bitmap.map(|_| Arc::new(Image::from_bytes(format, blob.bytes.clone()))),
            bitmap,
            svg: None,
            bytes,
            size: natural,
            natural,
            error: bitmap
                .is_none()
                .then(|| "This image could not be shown.".into()),
        }
    }

    /// `761-pixelated-small-images`: a small still image re-encoded at a
    /// whole multiple of its size with nearest-neighbour sampling (GPUI
    /// would blur it when drawing it larger). GIFs keep their animation.
    fn pixelated(mut self) -> Self {
        let (Some(image), Some((w, h))) = (&self.image, self.bitmap) else {
            return self;
        };
        if self.size != Some((w, h)) {
            return self;
        }
        let k = upscale_factor((w, h));
        if k < 2 || image.format == ImageFormat::Gif {
            return self;
        }
        let Ok(decoded) = image::load_from_memory(&image.bytes) else {
            return self;
        };
        let big = image::imageops::resize(
            &decoded.to_rgba8(),
            w * k,
            h * k,
            image::imageops::FilterType::Nearest,
        );
        let mut png = Vec::new();
        if big
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .is_err()
        {
            return self;
        }
        self.image = Some(Arc::new(Image::from_bytes(ImageFormat::Png, png)));
        self.bitmap = Some((w * k, h * k));
        self.size = self.bitmap;
        self
    }

    /// `794-svg-image-diff`: an SVG drawing (`.svg`, or gzipped `.svgz`),
    /// measured here and rendered later at the size it shows at; the footer
    /// shows the drawing's own size.
    fn from_svg(blob: &ImageBlob) -> Self {
        // fonts do not change the size: the quick parse skips them (no
        // font lookups, so no "No match for font-family" warnings)
        let options = resvg::usvg::Options {
            font_resolver: resvg::usvg::FontResolver {
                select_font: Box::new(|_, _| None),
                select_fallback: Box::new(|_, _, _| None),
            },
            ..Default::default()
        };
        let parsed = resvg::usvg::Tree::from_data(&blob.bytes, &options);
        let (size, natural, error) = match parsed {
            Ok(tree) => {
                let size = tree.size();
                let natural = (
                    size.width().ceil().max(1.) as u32,
                    size.height().ceil().max(1.) as u32,
                );
                let k = upscale_factor(natural);
                (Some((natural.0 * k, natural.1 * k)), Some(natural), None)
            }
            Err(err) => (
                None,
                None,
                Some(format!("This SVG drawing could not be read: {err}.").into()),
            ),
        };
        Self {
            image: None,
            bitmap: None,
            svg: size.is_some().then(|| Svg {
                bytes: blob.bytes.clone().into(),
                parsed: None,
                render: None,
                wanted: Cell::new(None),
                pending: false,
            }),
            bytes: blob.bytes.len(),
            size,
            natural,
            error,
        }
    }

    /// A TGA image (no magic number to guess from) re-encoded as PNG; the
    /// footer still shows the file's own size.
    fn from_tga(blob: &ImageBlob) -> Self {
        let png = image::load_from_memory_with_format(&blob.bytes, image::ImageFormat::Tga)
            .ok()
            .and_then(|decoded| {
                let mut png = Vec::new();
                decoded
                    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
                    .ok()?;
                Some(((decoded.width(), decoded.height()), png))
            });
        let (size, image) = match png {
            Some((size, png)) => (
                Some(size),
                Some(Arc::new(Image::from_bytes(ImageFormat::Png, png))),
            ),
            None => (None, None),
        };
        Self {
            image,
            bitmap: size,
            svg: None,
            bytes: blob.bytes.len(),
            size,
            natural: size,
            error: size
                .is_none()
                .then(|| "This image could not be shown.".into()),
        }
    }

    /// What `img` draws: the raster image or the drawing's latest rendering.
    fn source(&self) -> Option<ImageSource> {
        if let Some(image) = &self.image {
            return Some(image.clone().into());
        }
        let (render, _) = self.svg.as_ref()?.render.as_ref()?;
        Some(render.clone().into())
    }

    /// The size fitting starts from, a placeholder for an image that cannot
    /// be shown.
    fn fit_size(&self) -> (u32, u32) {
        self.size.unwrap_or(BROKEN_SIZE)
    }

    /// `794-svg-image-diff`: note that the drawing covers `size` (logical
    /// pixels at `scale_factor`) this frame.
    fn want(&self, size: Size<Pixels>, scale_factor: f32) {
        if let Some(svg) = &self.svg {
            let w = (f32::from(size.width) * scale_factor).round().max(1.);
            let h = (f32::from(size.height) * scale_factor).round().max(1.);
            let cap = (MAX_SVG_SIDE / w.max(h))
                .min((MAX_SVG_AREA / (w * h)).sqrt())
                .min(1.);
            svg.wanted.set(Some((
                ((w * cap).round() as u32).max(1),
                ((h * cap).round() as u32).max(1),
            )));
        }
    }
}

/// `size` times `k`.
fn scaled(size: Size<Pixels>, k: f32) -> Size<Pixels> {
    gpui_kit::size(size.width * k, size.height * k)
}

/// `1327-image-diff-zoom`: zoom steps of the buttons.
const ZOOM_STEP: f32 = 1.25;

pub struct ImageDiff {
    previous: Option<Side>,
    current: Option<Side>,
    status: FileStatusKind,
    swipe: Entity<SliderState>,
    onion: Entity<SliderState>,
    /// Measured size of the sizing container (one frame behind).
    container: Rc<Cell<Size<Pixels>>>,
    difference: Option<Arc<Image>>,
    /// Display scales of (previous, current) the blend was computed for.
    difference_scales: Option<(f32, f32)>,
    /// Whether that blend aligned the images top-left.
    difference_top_left: bool,
    /// `1320-sharp-image-difference`: the device-pixel size that blend was
    /// resampled to.
    difference_target: Option<(u32, u32)>,
    difference_pending: bool,
    /// `794-svg-image-diff`: bumped by every new rendering of a drawing, so
    /// the blend follows.
    svg_generation: u64,
    difference_svg_generation: u64,
    /// `752-image-diff-border-outside`: the 1 px border is drawn around the
    /// fitted image instead of inside it (`box-sizing: content-box`).
    border_outside: bool,
    /// `1322-pdf-diff`: the (previous, current) footers of a PDF page pair,
    /// in place of the PNG's pixels and bytes; also hides the 2-up size
    /// summary.
    page_footers: Option<(SharedString, SharedString)>,
    /// `1327-image-diff-zoom`: the zoom (1 = fitted) and the pan, the
    /// image's centre offset from the view's as a fraction of its size.
    zoomable: bool,
    zoom: f32,
    pan: (f32, f32),
    /// Where a pan drag started and the pan then.
    drag: Option<(Point<Pixels>, (f32, f32))>,
    /// The zoom that shows the larger image at its own pixels (last frame).
    actual_zoom: Cell<f32>,
    scale_factor: Cell<f32>,
    /// `794-svg-image-diff`: to free the renderings from the sprite atlas.
    window: Option<AnyWindowHandle>,
}

impl ImageDiff {
    /// `1322-pdf-diff`: shows `previous` / `current` under the pages.
    pub fn with_page_footers(mut self, previous: SharedString, current: SharedString) -> Self {
        self.page_footers = Some((previous, current));
        self
    }

    pub fn new(
        previous: Option<&ImageBlob>,
        current: Option<&ImageBlob>,
        status: FileStatusKind,
        cx: &mut Context<Self>,
    ) -> Self {
        let swipe = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(100.)
                .step(0.1)
                .default_value(0.)
        });
        // GHD starts the onion skin at 1 %
        let onion = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(100.)
                .step(0.1)
                .default_value(1.)
        });
        for s in [&swipe, &onion] {
            cx.subscribe(s, |_, _, _: &SliderEvent, cx| cx.notify())
                .detach();
        }
        let flag =
            |id| corvene_core::AppState::try_global(cx).is_some_and(|s| s.read(cx).flags.bool(id));
        let pixelated = flag(corvene_core::flags::ids::PIXELATED_SMALL_IMAGES);
        let border_outside = flag(corvene_core::flags::ids::IMAGE_DIFF_BORDER_OUTSIDE);
        let zoomable = flag(corvene_core::flags::ids::IMAGE_DIFF_ZOOM);
        let side = |blob: &ImageBlob| {
            let side = Side::from_blob(blob);
            if pixelated { side.pixelated() } else { side }
        };
        cx.on_release(|this: &mut Self, cx| {
            let renders: Vec<_> = [&this.previous, &this.current]
                .into_iter()
                .flatten()
                .filter_map(|side| side.svg.as_ref()?.render.as_ref())
                .map(|(render, _)| render.clone())
                .collect();
            if let Some(window) = this.window
                && !renders.is_empty()
            {
                window
                    .update(cx, |_, window, _| {
                        for render in renders {
                            window.drop_image(render).ok();
                        }
                    })
                    .ok();
            }
        })
        .detach();
        Self {
            previous: previous.map(side),
            current: current.map(side),
            status,
            swipe,
            onion,
            container: Rc::new(Cell::new(Size::default())),
            difference: None,
            difference_scales: None,
            difference_top_left: false,
            difference_target: None,
            difference_pending: false,
            svg_generation: 0,
            difference_svg_generation: 0,
            border_outside,
            page_footers: None,
            zoomable,
            zoom: 1.,
            pan: (0., 0.),
            drag: None,
            actual_zoom: Cell::new(1.),
            scale_factor: Cell::new(1.),
            window: None,
        }
    }

    /// GHD `getAspectFitSize`: shrink (never grow) to fit the container.
    fn aspect_fit(image: (u32, u32), container: Size<Pixels>) -> Size<Pixels> {
        let (w, h) = (image.0 as f32, image.1 as f32);
        let (cw, ch) = (f32::from(container.width), f32::from(container.height));
        let height_ratio = if ch < h && ch > 0. { h / ch } else { 1. };
        let width_ratio = if cw < w && cw > 0. { w / cw } else { 1. };
        let mut ratio = width_ratio.max(1.);
        if width_ratio < height_ratio {
            ratio = height_ratio.max(1.);
        }
        size(zpx(w / ratio), zpx(h / ratio))
    }

    /// GHD `getMaxFitSize`: the box both images share.
    fn max_fit(&self, container: Size<Pixels>) -> Size<Pixels> {
        let fit = |side: &Option<Side>| {
            side.as_ref()
                .map(|s| Self::aspect_fit(s.fit_size(), container))
                .unwrap_or_default()
        };
        let (p, c) = (fit(&self.previous), fit(&self.current));
        size(p.width.max(c.width), p.height.max(c.height))
    }

    /// `1327-image-diff-zoom`: note the zoom at which `side`, fitted to
    /// `fit`, shows its own pixels.
    fn note_actual_zoom(&self, side: &Side, fit: Size<Pixels>) {
        let natural = side.natural.unwrap_or(side.fit_size());
        let fit_w = f32::from(fit.width);
        if fit_w > 0. {
            let actual = (natural.0 as f32 / fit_w).max(1.);
            self.actual_zoom.set(self.actual_zoom.get().max(actual));
        }
    }

    /// Captures the sizing container's bounds for the next frame.
    fn measure(&self, cx: &Context<Self>) -> AnyElement {
        let cell = self.container.clone();
        let entity = cx.entity();
        canvas(
            move |bounds, window, _| {
                if cell.get() != bounds.size {
                    cell.set(bounds.size);
                    window.on_next_frame(move |_, cx| entity.update(cx, |_, cx| cx.notify()));
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
        .into_any_element()
    }

    /// `753-image-diff-background`: whether the checkerboard is dark
    /// (`dark`, or `theme` with a dark app theme).
    fn dark_checkerboard(cx: &App) -> bool {
        let choice = corvene_core::AppState::try_global(cx).map_or(String::new(), |s| {
            s.read(cx)
                .flags
                .text(corvene_core::flags::ids::IMAGE_DIFF_BACKGROUND)
                .to_string()
        });
        match choice.as_str() {
            "dark" => true,
            "theme" => cx.ghd().is_dark(),
            _ => false,
        }
    }

    /// `754-image-diff-alignment`: images of different sizes share the top
    /// left corner in Swipe, Onion Skin and Difference instead of the centre.
    fn top_left(cx: &App) -> bool {
        corvene_core::AppState::try_global(cx).is_some_and(|s| {
            s.read(cx)
                .flags
                .text(corvene_core::flags::ids::IMAGE_DIFF_ALIGNMENT)
                == "top-left"
        })
    }

    /// What a 1 px border adds around an image of the fitted size: nothing
    /// in GHD (`border-box`, the border eats into the image), 2 px with
    /// `752-image-diff-border-outside`.
    fn border_extra(&self) -> Pixels {
        if self.border_outside { px(2.) } else { px(0.) }
    }

    /// `1327-image-diff-zoom`: the largest pan at `zoom`.
    fn pan_limit(zoom: f32) -> f32 {
        ((1. - 1. / zoom) / 2.).max(0.)
    }

    fn max_zoom(&self) -> f32 {
        (self.actual_zoom.get() * 8.).clamp(16., 256.)
    }

    /// `1327-image-diff-zoom`: zoom to `zoom`, keeping the point under
    /// `pointer` (in `view`, a viewport) where it is.
    fn zoom_at(&mut self, view: Bounds<Pixels>, pointer: Point<Pixels>, zoom: f32) {
        let zoom = zoom.clamp(1., self.max_zoom());
        let (vw, vh) = (f32::from(view.size.width), f32::from(view.size.height));
        if vw <= 0. || vh <= 0. {
            return;
        }
        let d = pointer - view.center();
        let (dx, dy) = (f32::from(d.x), f32::from(d.y));
        // the point under the pointer, as a fraction of the image from its centre
        let (ux, uy) = (
            dx / (vw * self.zoom) - self.pan.0,
            dy / (vh * self.zoom) - self.pan.1,
        );
        let limit = Self::pan_limit(zoom);
        self.pan = (
            (dx / (vw * zoom) - ux).clamp(-limit, limit),
            (dy / (vh * zoom) - uy).clamp(-limit, limit),
        );
        self.zoom = zoom;
    }

    /// `1327-image-diff-zoom`: zoom around the views' centre.
    fn zoom_centred(&mut self, zoom: f32, cx: &mut Context<Self>) {
        let zoom = zoom.clamp(1., self.max_zoom());
        let limit = Self::pan_limit(zoom);
        self.pan = (
            self.pan.0.clamp(-limit, limit),
            self.pan.1.clamp(-limit, limit),
        );
        self.zoom = zoom;
        cx.notify();
    }

    /// `1327-image-diff-zoom`: `content` built at `viewport × zoom`, seen
    /// through `viewport`, panned; the zoom and pan gestures.
    fn zoomed(
        &self,
        viewport: Size<Pixels>,
        content: AnyElement,
        cx: &Context<Self>,
    ) -> AnyElement {
        let content_size = scaled(viewport, self.zoom);
        let left = (viewport.width - content_size.width) / 2. + content_size.width * self.pan.0;
        let top = (viewport.height - content_size.height) / 2. + content_size.height * self.pan.1;
        let inner = div()
            .absolute()
            .left(left)
            .top(top)
            .w(content_size.width)
            .h(content_size.height)
            .child(content);
        let view = div()
            .relative()
            .flex_none()
            .w(viewport.width)
            .h(viewport.height)
            .overflow_hidden();
        if !self.zoomable {
            return view.child(inner).into_any_element();
        }
        let bounds = Rc::new(Cell::new(Bounds::default()));
        let measure = {
            let bounds = bounds.clone();
            canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                .absolute()
                .inset_0()
        };
        let line = |window: &Window| window.line_height();
        view.when(self.zoom > 1., |d| {
            d.cursor(if self.drag.is_some() {
                CursorStyle::ClosedHand
            } else {
                CursorStyle::OpenHand
            })
        })
        .child(measure)
        .child(inner)
        .on_scroll_wheel(cx.listener({
            let bounds = bounds.clone();
            move |this, ev: &ScrollWheelEvent, window, cx| {
                let delta = ev.delta.pixel_delta(line(window));
                if ev.modifiers.platform || ev.modifiers.control {
                    let factor = (f32::from(delta.y) / 200.).exp();
                    this.zoom_at(bounds.get(), ev.position, this.zoom * factor);
                } else if this.zoom > 1. {
                    let size = scaled(bounds.get().size, this.zoom);
                    let limit = Self::pan_limit(this.zoom);
                    this.pan = (
                        (this.pan.0 + f32::from(delta.x) / f32::from(size.width).max(1.))
                            .clamp(-limit, limit),
                        (this.pan.1 + f32::from(delta.y) / f32::from(size.height).max(1.))
                            .clamp(-limit, limit),
                    );
                } else {
                    return;
                }
                cx.stop_propagation();
                cx.notify();
            }
        }))
        .on_pinch(cx.listener({
            let bounds = bounds.clone();
            move |this, ev: &PinchEvent, _, cx| {
                this.zoom_at(bounds.get(), ev.position, this.zoom * (1. + ev.delta));
                cx.stop_propagation();
                cx.notify();
            }
        }))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener({
                let bounds = bounds.clone();
                move |this, ev: &MouseDownEvent, _, cx| {
                    if ev.click_count >= 2 {
                        // fitted: to its own pixels (at least twice); else fitted
                        let zoom = if this.zoom > 1. {
                            1.
                        } else {
                            this.actual_zoom.get().max(2.)
                        };
                        this.drag = None;
                        this.zoom_at(bounds.get(), ev.position, zoom);
                    } else if this.zoom > 1. {
                        this.drag = Some((ev.position, this.pan));
                    } else {
                        return;
                    }
                    cx.notify();
                }
            }),
        )
        .on_mouse_move(cx.listener({
            let bounds = bounds.clone();
            move |this, ev: &MouseMoveEvent, _, cx| {
                let Some((start, pan)) = this.drag else {
                    return;
                };
                if ev.pressed_button != Some(MouseButton::Left) {
                    this.drag = None;
                    cx.notify();
                    return;
                }
                let size = scaled(bounds.get().size, this.zoom);
                let moved = ev.position - start;
                let limit = Self::pan_limit(this.zoom);
                this.pan = (
                    (pan.0 + f32::from(moved.x) / f32::from(size.width).max(1.))
                        .clamp(-limit, limit),
                    (pan.1 + f32::from(moved.y) / f32::from(size.height).max(1.))
                        .clamp(-limit, limit),
                );
                cx.notify();
            }
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _: &MouseUpEvent, _, cx| {
                if this.drag.take().is_some() {
                    cx.notify();
                }
            }),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|this, _: &MouseUpEvent, _, cx| {
                if this.drag.take().is_some() {
                    cx.notify();
                }
            }),
        )
        .into_any_element()
    }

    /// `1327-image-diff-zoom`: out, the zoom as a percentage of the image's
    /// own pixels, in, fit and actual pixels.
    fn zoom_bar(&self, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let button = |id: &'static str, label: &'static str, tip: &'static str| {
            div()
                .id(id)
                .a11y_button(tip)
                .h(zpx(21.))
                .min_w(zpx(23.))
                .px(zpx(5.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(BORDER_RADIUS())
                .border_1()
                .border_color(t.box_border)
                .bg(t.background)
                .text_color(t.text)
                .cursor_pointer()
                .hover(|d| d.bg(t.box_hover_background))
                .ghd_tooltip(tip)
                .child(label)
        };
        let percent = (self.zoom / self.actual_zoom.get() * 100.).round();
        div()
            .absolute()
            .top(SPACING())
            .right(SPACING())
            .flex()
            .flex_row()
            .items_center()
            .gap(zpx(4.))
            .text_size(FONT_SIZE_SM())
            .child(button("image-zoom-out", "\u{2212}", "Zoom out").on_click(
                cx.listener(|this, _, _, cx| this.zoom_centred(this.zoom / ZOOM_STEP, cx)),
            ))
            .child(
                div()
                    .min_w(zpx(38.))
                    .flex()
                    .justify_center()
                    .text_color(t.text_secondary)
                    .child(format!("{percent}%")),
            )
            .child(button("image-zoom-in", "+", "Zoom in").on_click(
                cx.listener(|this, _, _, cx| this.zoom_centred(this.zoom * ZOOM_STEP, cx)),
            ))
            .child(
                button("image-zoom-fit", "Fit", "Fit to the view").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.pan = (0., 0.);
                        this.zoom_centred(1., cx)
                    },
                )),
            )
            .child(
                button("image-zoom-actual", "1:1", "Actual pixels").on_click(cx.listener(
                    |this, _, _, cx| {
                        let actual = this.actual_zoom.get();
                        this.zoom_centred(actual, cx)
                    },
                )),
            )
            .into_any_element()
    }

    /// `794-svg-image-diff`: render each drawing again whose size on screen
    /// changed enough (larger, or under half the rendering), one at a time
    /// per side, on a background thread.
    fn update_svg_renders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for previous in [true, false] {
            let side = if previous {
                &mut self.previous
            } else {
                &mut self.current
            };
            let Some(svg) = side.as_mut().and_then(|s| s.svg.as_mut()) else {
                continue;
            };
            let Some(wanted) = svg.wanted.get() else {
                continue;
            };
            let stale = svg.render.as_ref().is_none_or(|(_, (w, h))| {
                (*w as f32) < wanted.0 as f32 * 0.98
                    || (*h as f32) < wanted.1 as f32 * 0.98
                    || *w > wanted.0 * 2
            });
            if !stale || svg.pending {
                continue;
            }
            svg.pending = true;
            let renderer = cx.svg_renderer();
            let bytes = svg.bytes.clone();
            let parsed = svg.parsed.clone();
            let task = cx.background_executor().spawn(async move {
                let parsed = match parsed {
                    Some(parsed) => parsed,
                    None => Arc::new(renderer.parse_svg(&bytes).map_err(|e| e.to_string())?),
                };
                let size = size(DevicePixels(wanted.0 as i32), DevicePixels(wanted.1 as i32));
                let render = renderer
                    .render_parsed(&parsed, SvgSize::ExactSize(size))
                    .map_err(|e| e.to_string())?;
                Ok::<_, String>((parsed, render))
            });
            cx.spawn_in(window, async move |this, cx| {
                let result = task.await;
                this.update_in(cx, |this, window, cx| {
                    let side = if previous {
                        &mut this.previous
                    } else {
                        &mut this.current
                    };
                    let Some(side) = side.as_mut() else { return };
                    let Some(svg) = side.svg.as_mut() else { return };
                    svg.pending = false;
                    match result {
                        Ok((parsed, render)) => {
                            svg.parsed = Some(parsed);
                            if let Some((old, _)) = svg.render.replace((render, wanted)) {
                                window.drop_image(old).ok();
                            }
                            this.svg_generation += 1;
                        }
                        Err(err) => {
                            side.error =
                                Some(format!("This SVG drawing could not be drawn: {err}.").into());
                            side.svg = None;
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    /// One image in its box (`.image-diff-current` and friends): the
    /// checkerboard, the image zoomed in its border, or why it cannot be
    /// shown.
    fn image_element(
        &self,
        side: &Side,
        box_size: Size<Pixels>,
        border: Hsla,
        dark: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let extra = self.border_extra();
        let fit = Self::aspect_fit(side.fit_size(), box_size);
        self.note_actual_zoom(side, fit);
        let outer = size(fit.width + extra, fit.height + extra);
        // the image inside the 1 px border
        let inner = size(
            (outer.width - px(2.)).max(px(0.)),
            (outer.height - px(2.)).max(px(0.)),
        );
        let body = match (&side.error, side.source()) {
            (Some(error), _) => div()
                .size_full()
                .p(SPACING())
                .flex()
                .items_center()
                .justify_center()
                .text_center()
                .text_size(FONT_SIZE_SM())
                .text_color(cx.ghd().text_secondary)
                .bg(cx.ghd().background)
                .child(error.clone())
                .into_any_element(),
            (None, source) => {
                side.want(scaled(inner, self.zoom), self.scale_factor.get());
                self.zoomed(
                    inner,
                    match source {
                        Some(source) => img(source)
                            .size_full()
                            .object_fit(ObjectFit::Contain)
                            .into_any_element(),
                        None => div().into_any_element(),
                    },
                    cx,
                )
            }
        };
        div()
            .relative()
            .flex_none()
            .w(outer.width)
            .h(outer.height)
            .child(checkerboard(dark))
            .child(
                div()
                    .absolute()
                    .top(px(1.))
                    .left(px(1.))
                    .w(inner.width)
                    .h(inner.height)
                    .child(body),
            )
            .child(div().absolute().inset_0().border_1().border_color(border))
            .into_any_element()
    }

    /// An overlaid image (`.image-diff-previous` / `.image-diff-current` inside
    /// `.image-container`): absolutely positioned over the whole box (`zoom`
    /// times the shared box), centered, at most the box size (`maxSize`),
    /// transparent background.
    fn overlay_image(
        &self,
        side: &Side,
        box_size: Size<Pixels>,
        border: Option<Hsla>,
        top_left: bool,
    ) -> AnyElement {
        let extra = self.border_extra();
        let fit = Self::aspect_fit(side.fit_size(), box_size);
        self.note_actual_zoom(side, fit);
        let fit = scaled(fit, self.zoom);
        let border_extra = if border.is_some() { extra } else { px(0.) };
        let image_size = size(fit.width + border_extra, fit.height + border_extra);
        side.want(fit, self.scale_factor.get());
        let image = match side.source() {
            Some(source) => img(source)
                .flex_none()
                .w(image_size.width)
                .h(image_size.height)
                .object_fit(ObjectFit::Contain)
                .when_some(border, |d, color| d.border_1().border_color(color))
                .into_any_element(),
            None => div()
                .flex_none()
                .w(image_size.width)
                .h(image_size.height)
                .when_some(border, |d, color| d.border_1().border_color(color))
                .into_any_element(),
        };
        div()
            .absolute()
            .top_0()
            .left_0()
            .w(box_size.width * self.zoom)
            .h(box_size.height * self.zoom)
            .flex()
            .when(!top_left, |d| d.items_center().justify_center())
            .child(image)
            .into_any_element()
    }

    /// Display scale of `side` inside the overlay box (1 = natural size),
    /// at the zoom.
    fn display_scale(&self, side: &Side, box_size: Size<Pixels>) -> f32 {
        let (w, h) = side.fit_size();
        if w == 0 {
            return 1.;
        }
        f32::from(Self::aspect_fit((w, h), box_size).width) / w as f32 * self.zoom
    }

    fn footer(side: &Side, page: Option<&SharedString>, cx: &App) -> AnyElement {
        let t = cx.ghd();
        if let Some(page) = page {
            return div()
                .text_size(FONT_SIZE())
                .text_color(t.text_secondary)
                .child(page.clone())
                .into_any_element();
        }
        let (w, h) = side.natural.unwrap_or((0, 0));
        let strong = |s: &str| div().font_weight(FontWeight::SEMIBOLD).child(s.to_string());
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .justify_center()
            .gap(zpx(3.))
            .text_size(FONT_SIZE())
            .text_color(t.text_secondary)
            .child(strong("W:"))
            .child(format!("{w}px |"))
            .child(strong("H:"))
            .child(format!("{h}px |"))
            .child(strong("Size:"))
            .child(format_bytes(side.bytes as i64, 2))
            .into_any_element()
    }

    /// `TwoUp`
    fn two_up(&self, previous: &Side, current: &Side, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let dark = Self::dark_checkerboard(cx);
        let container = self.container.get();
        // room for the headers / footers / summary rows
        let image_box = size(
            ((container.width - SPACING_DOUBLE() * 2.) / 2. - SPACING_HALF()).max(zpx(0.)),
            (container.height - zpx(90.)).max(zpx(0.)),
        );
        let max_fit = self.max_fit(image_box);
        let column = |side: &Side, page: Option<&SharedString>, label: &str, color: Hsla| {
            div()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .min_w_0()
                .min_h_0()
                .max_w(max_fit.width.max(zpx(200.)))
                .mb(SPACING_HALF())
                .text_color(color)
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .pb(zpx(10.))
                        .child(label.to_string()),
                )
                .child(self.image_element(side, image_box, color, dark, cx))
                .child(Self::footer(side, page, cx))
        };
        let diff_bytes = current.bytes as i64 - previous.bytes as i64;
        let percent = if previous.bytes == 0 {
            0
        } else {
            ((current.bytes as f64 / previous.bytes as f64) * 100.)
                .round()
                .abs() as i64
        };
        let summary = if diff_bytes != 0 {
            format!(
                "{}{} ({percent}%)",
                if diff_bytes >= 0 { "+" } else { "" },
                format_bytes(diff_bytes, 2)
            )
        } else {
            "No size difference".to_string()
        };
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .min_h_0()
            .min_w_0()
            .child(self.measure(cx))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .max_h_full()
                    .min_h_0()
                    .min_w_0()
                    .px(SPACING_DOUBLE())
                    .gap(SPACING())
                    .child(column(
                        previous,
                        self.page_footers.as_ref().map(|f| &f.0),
                        "Deleted",
                        t.color_deleted,
                    ))
                    .child(column(
                        current,
                        self.page_footers.as_ref().map(|f| &f.1),
                        "Added",
                        t.color_new,
                    )),
            )
            .when(self.page_footers.is_none(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(zpx(4.))
                        .text_color(t.text_secondary)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Diff:")
                        .child(
                            div()
                                .when(diff_bytes > 0, |d| d.text_color(t.color_new))
                                .when(diff_bytes < 0, |d| d.text_color(t.color_deleted))
                                .child(summary),
                        ),
                )
            })
            .into_any_element()
    }

    /// The overlay box shared by Swipe / Onion Skin / Difference.
    fn overlay_box(&self) -> Size<Pixels> {
        let container = self.container.get();
        self.max_fit(size(container.width, container.height))
    }

    /// `Swipe`: the slider reveals the new image from the right.
    fn swipe(&self, previous: &Side, current: &Side, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let top_left = Self::top_left(cx);
        let box_size = self.overlay_box();
        let zbox = scaled(box_size, self.zoom);
        let percentage = self.swipe.read(cx).value().start();
        let swiper_width = (zbox.width * (1. - percentage / 100.)).floor();
        let images = div()
            .relative()
            .size_full()
            .child(
                // previous: `clip-path: inset(0 swiper 0 0)`
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .h(zbox.height)
                    .w((zbox.width - swiper_width).max(zpx(0.)))
                    .overflow_hidden()
                    .child(self.overlay_image(previous, box_size, Some(t.color_deleted), top_left)),
            )
            .child(
                // current: `clip-path: inset(0 0 0 width - swiper)`
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .h(zbox.height)
                    .w(swiper_width.max(zpx(0.)))
                    .overflow_hidden()
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .w(zbox.width)
                            .h(zbox.height)
                            .child(self.overlay_image(
                                current,
                                box_size,
                                Some(t.color_new),
                                top_left,
                            )),
                    ),
            )
            .into_any_element();
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .min_h_0()
            .child(
                div()
                    .w(box_size.width + SLIDER_OVERFLOW())
                    .mb(SPACING())
                    .child(Slider::new(&self.swipe).horizontal()),
            )
            .child(
                div()
                    .id("image-diff-sizing")
                    .relative()
                    .flex_1()
                    .w_full()
                    .mb(SPACING_HALF())
                    .flex()
                    .justify_center()
                    .items_center()
                    .min_h_0()
                    .child(self.measure(cx))
                    .child(
                        // `.image-container` with the checkerboard behind both
                        div()
                            .relative()
                            .flex_none()
                            .w(box_size.width)
                            .h(box_size.height)
                            .child(checkerboard(Self::dark_checkerboard(cx)))
                            .child(self.zoomed(box_size, images, cx)),
                    ),
            )
            .into_any_element()
    }

    /// `OnionSkin`: the slider cross-fades the new image over the old one.
    fn onion_skin(&self, previous: &Side, current: &Side, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let top_left = Self::top_left(cx);
        let box_size = self.overlay_box();
        let crossfade = self.onion.read(cx).value().start() / 100.;
        let images = div()
            .relative()
            .size_full()
            .child(self.overlay_image(previous, box_size, Some(t.color_deleted), top_left))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .opacity(crossfade)
                    .child(self.overlay_image(current, box_size, Some(t.color_new), top_left)),
            )
            .into_any_element();
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .min_h_0()
            .child(
                div()
                    .w(box_size.width / 2.)
                    .mb(SPACING())
                    .child(Slider::new(&self.onion).horizontal()),
            )
            .child(
                div()
                    .id("image-diff-sizing")
                    .relative()
                    .flex_1()
                    .w_full()
                    .mb(SPACING_HALF())
                    .flex()
                    .justify_center()
                    .items_center()
                    .min_h_0()
                    .child(self.measure(cx))
                    .child(
                        div()
                            .relative()
                            .flex_none()
                            .w(box_size.width)
                            .h(box_size.height)
                            .child(checkerboard(Self::dark_checkerboard(cx)))
                            .child(self.zoomed(box_size, images, cx)),
                    ),
            )
            .into_any_element()
    }

    /// `DifferenceBlend`: the current image drawn over the previous one with
    /// `mix-blend-mode: difference`, both centered at their display scale; no
    /// borders, no checkerboard.
    fn difference(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let box_size = self.overlay_box();
        let scale_factor = window.scale_factor();
        let (inputs, scales) = match (&self.previous, &self.current) {
            (Some(p), Some(c)) => {
                for side in [p, c] {
                    let fit = Self::aspect_fit(side.fit_size(), box_size);
                    self.note_actual_zoom(side, fit);
                    side.want(scaled(fit, self.zoom), scale_factor);
                }
                (
                    BlendInput::of(p).zip(BlendInput::of(c)),
                    (self.blend_scale(p, box_size), self.blend_scale(c, box_size)),
                )
            }
            _ => (None, (1., 1.)),
        };
        let top_left = Self::top_left(cx);
        // `1320-sharp-image-difference`: the blend's size on screen in
        // device pixels (the larger fitted image, as the box is), at most
        // what a drawing is rendered to
        let zbox = scaled(box_size, self.zoom);
        let target = corvene_core::AppState::try_global(cx)
            .is_some_and(|s| {
                s.read(cx)
                    .flags
                    .bool(corvene_core::flags::ids::SHARP_IMAGE_DIFFERENCE)
            })
            .then(|| {
                let w = f32::from(zbox.width) * scale_factor;
                let h = f32::from(zbox.height) * scale_factor;
                let cap = (MAX_SVG_SIDE / w.max(h).max(1.)).min(1.);
                (
                    ((w * cap).round() as u32).max(1),
                    ((h * cap).round() as u32).max(1),
                )
            });
        let stale = top_left != self.difference_top_left
            || target != self.difference_target
            || self.svg_generation != self.difference_svg_generation
            || self.difference_scales.is_none_or(|(p, c)| {
                (p / c - scales.0 / scales.1).abs() > 0.005 * (scales.0 / scales.1)
            });
        if stale
            && !self.difference_pending
            && box_size.width > zpx(0.)
            && let Some((a, b)) = inputs
        {
            self.difference_pending = true;
            let generation = self.svg_generation;
            let task = cx
                .background_executor()
                .spawn(async move { difference_image(a, b, scales.0, scales.1, top_left, target) });
            cx.spawn(async move |this, cx| {
                let result = task.await;
                this.update(cx, |this, cx| {
                    this.difference_pending = false;
                    this.difference_scales = Some(scales);
                    this.difference_top_left = top_left;
                    this.difference_target = target;
                    this.difference_svg_generation = generation;
                    this.difference = result.map(Arc::new);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        let blend = self.difference.clone().map(|image| {
            // drawn at exactly the device pixels it was made for
            let (w, h) = match self.difference_target {
                Some((w, h)) => (px(w as f32 / scale_factor), px(h as f32 / scale_factor)),
                None => (zbox.width, zbox.height),
            };
            div()
                .size_full()
                .flex()
                .justify_center()
                .items_center()
                .child(
                    img(image)
                        .flex_none()
                        .w(w)
                        .h(h)
                        .object_fit(ObjectFit::Contain),
                )
                .into_any_element()
        });
        div()
            .id("image-diff-sizing")
            .relative()
            .size_full()
            .flex()
            .justify_center()
            .items_center()
            .min_h_0()
            .child(self.measure(cx))
            .when_some(blend, |d, blend| d.child(self.zoomed(box_size, blend, cx)))
            .into_any_element()
    }

    /// The on-screen size of one pixel of `side`'s blend input, in logical
    /// pixels.
    fn blend_scale(&self, side: &Side, box_size: Size<Pixels>) -> f32 {
        let fit = Self::aspect_fit(side.fit_size(), box_size);
        let pixels = match (&side.svg, side.bitmap) {
            (Some(svg), _) => svg.render.as_ref().map(|(_, (w, _))| *w),
            (None, Some((w, _))) => Some(w),
            _ => None,
        };
        match pixels {
            Some(w) if w > 0 => f32::from(fit.width) * self.zoom / w as f32,
            _ => self.display_scale(side, box_size),
        }
    }

    /// `NewImageDiff` / `DeletedImageDiff`: one image with its header.
    fn single(&self, side: &Side, label: &str, color: Hsla, cx: &Context<Self>) -> AnyElement {
        let dark = Self::dark_checkerboard(cx);
        let container = self.container.get();
        let image_box = size(
            (container.width - SPACING_DOUBLE()).max(zpx(0.)),
            (container.height - zpx(40.)).max(zpx(0.)),
        );
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .min_h_0()
            .text_color(color)
            .child(self.measure(cx))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .pb(zpx(10.))
                    .child(label.to_string()),
            )
            .child(self.image_element(side, image_box, color, dark, cx))
            .into_any_element()
    }
}

/// The pixels a Difference blend reads for one side.
enum BlendInput {
    /// An encoded image.
    Encoded(Arc<Image>),
    /// `794-svg-image-diff`: a drawing's rendering (BGRA).
    Rendered(Arc<RenderImage>),
}

impl BlendInput {
    fn of(side: &Side) -> Option<Self> {
        if let Some(image) = &side.image {
            return Some(Self::Encoded(image.clone()));
        }
        let (render, _) = side.svg.as_ref()?.render.as_ref()?;
        Some(Self::Rendered(render.clone()))
    }

    fn rgba(&self) -> Option<image::RgbaImage> {
        match self {
            Self::Encoded(image) => Some(image::load_from_memory(&image.bytes).ok()?.to_rgba8()),
            Self::Rendered(render) => {
                let size = render.size(0);
                let mut bytes = render.as_bytes(0)?.to_vec();
                for pixel in bytes.as_chunks_mut::<4>().0 {
                    pixel.swap(0, 2);
                }
                image::RgbaImage::from_raw(size.width.0 as u32, size.height.0 as u32, bytes)
            }
        }
    }
}

impl Render for ImageDiff {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd().clone();
        self.window = Some(window.window_handle());
        self.scale_factor.set(window.scale_factor());
        self.actual_zoom.set(1.);
        // a pan out of range after a resize or a new zoom limit
        let limit = Self::pan_limit(self.zoom);
        self.pan = (
            self.pan.0.clamp(-limit, limit),
            self.pan.1.clamp(-limit, limit),
        );
        let kind = corvene_core::AppState::try_global(cx)
            .map(|s| s.read(cx).settings.image_diff_type)
            .unwrap_or_default();
        let body: AnyElement = match (&self.previous, &self.current) {
            (Some(previous), Some(current)) => {
                let tabs = ["2-up", "Swipe", "Onion Skin", "Difference"];
                let selected = match kind {
                    ImageDiffType::TwoUp => 0,
                    ImageDiffType::Swipe => 1,
                    ImageDiffType::OnionSkin => 2,
                    ImageDiffType::Difference => 3,
                };
                let switcher = div()
                    .id("image-diff-tabs")
                    .w(TAB_BAR_WIDTH())
                    .h(zpx(29.))
                    .flex_none()
                    .flex()
                    .flex_row()
                    .gap(SPACING())
                    .mt(SPACING())
                    .mx_auto()
                    .mb(SPACING())
                    .children(tabs.iter().enumerate().map(|(ix, label)| {
                        let is_selected = ix == selected;
                        div()
                            .id(("image-diff-tab", ix))
                            .flex_1()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(BORDER_RADIUS())
                            .cursor_pointer()
                            .text_size(FONT_SIZE())
                            .when(is_selected, |d| {
                                d.bg(t.tab_bar_active)
                                    .text_color(t.box_selected_active_text)
                            })
                            .when(!is_selected, |d| {
                                let hover = t.tab_bar_hover_background;
                                d.hover(move |s| s.bg(hover))
                            })
                            .on_click(move |_, _, cx| {
                                let next = match ix {
                                    0 => ImageDiffType::TwoUp,
                                    1 => ImageDiffType::Swipe,
                                    2 => ImageDiffType::OnionSkin,
                                    _ => ImageDiffType::Difference,
                                };
                                Dispatcher::set_image_diff_type(next, cx);
                            })
                            .child(label.to_string())
                    }));
                let content = match kind {
                    ImageDiffType::TwoUp => self.two_up(previous, current, cx),
                    ImageDiffType::Swipe => self.swipe(previous, current, cx),
                    ImageDiffType::OnionSkin => self.onion_skin(previous, current, cx),
                    ImageDiffType::Difference => {
                        // the borrow of `previous`/`current` ends before the mutable call
                        let _ = (previous, current);
                        self.difference(window, cx)
                    }
                };
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .min_h_0()
                    .child(switcher)
                    .child(div().flex_1().min_h_0().child(content))
                    .into_any_element()
            }
            (None, Some(current))
                if matches!(self.status, FileStatusKind::New | FileStatusKind::Untracked) =>
            {
                self.single(current, "Added", t.color_new, cx)
            }
            (Some(previous), None) if self.status == FileStatusKind::Deleted => {
                self.single(previous, "Deleted", t.color_deleted, cx)
            }
            (_, Some(current)) => self.single(current, "Added", t.color_new, cx),
            (Some(previous), None) => self.single(previous, "Deleted", t.color_deleted, cx),
            (None, None) => div().into_any_element(),
        };
        self.update_svg_renders(window, cx);
        let drawable = [&self.previous, &self.current]
            .into_iter()
            .flatten()
            .any(|side| side.error.is_none());
        // `.panel.image`
        div()
            .id("image-diff")
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .p(SPACING())
            .bg(t.background)
            .text_size(FONT_SIZE())
            .child(body)
            .when(self.zoomable && drawable, |d| d.child(self.zoom_bar(cx)))
    }
}

/// GHD `checkboard-background` mixin behind transparent images; `dark`
/// (`753-image-diff-background`) swaps in a dark pair of greys.
fn checkerboard(dark: bool) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let (light, dark) = if dark {
                (rgb(0x2b2b2b), rgb(0x1e1e1e))
            } else {
                (rgb(0xffffff), rgb(0xcccccc))
            };
            window.paint_quad(fill(bounds, light));
            let cell = zpx(10.);
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                let mut y = bounds.top();
                let mut row = 0;
                while y < bounds.bottom() {
                    let mut x = bounds.left() + if row % 2 == 0 { zpx(0.) } else { cell };
                    while x < bounds.right() {
                        window.paint_quad(fill(Bounds::new(point(x, y), size(cell, cell)), dark));
                        x += cell * 2.;
                    }
                    y += cell;
                    row += 1;
                }
            });
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

/// CSS `mix-blend-mode: difference` of `b` (current) over `a` (previous), each
/// centered (or, `top_left`, in the top left corner) in the shared box at its
/// display scale. Rendered at the larger of
/// the two scales so that image keeps its natural resolution; encoded as PNG.
fn difference_image(
    a: BlendInput,
    b: BlendInput,
    scale_a: f32,
    scale_b: f32,
    top_left: bool,
    target: Option<(u32, u32)>,
) -> Option<Image> {
    use image::imageops::FilterType;
    let a = a.rgba()?;
    let b = b.rgba()?;
    let top = scale_a.max(scale_b).max(f32::EPSILON);
    let resize = |img: image::RgbaImage, scale: f32| {
        let factor = scale / top;
        let w = ((img.width() as f32 * factor).round() as u32).max(1);
        let h = ((img.height() as f32 * factor).round() as u32).max(1);
        if w == img.width() && h == img.height() {
            img
        } else {
            image::imageops::resize(&img, w, h, FilterType::Triangle)
        }
    };
    let (a, b) = (resize(a, scale_a), resize(b, scale_b));
    let width = a.width().max(b.width());
    let height = a.height().max(b.height());
    let offset = |img: &image::RgbaImage| {
        if top_left {
            (0, 0)
        } else {
            ((width - img.width()) / 2, (height - img.height()) / 2)
        }
    };
    let ((ax, ay), (bx, by)) = (offset(&a), offset(&b));
    let sample = |img: &image::RgbaImage, ox: u32, oy: u32, x: u32, y: u32| {
        if x >= ox && y >= oy && x - ox < img.width() && y - oy < img.height() {
            img.get_pixel(x - ox, y - oy).0
        } else {
            [0; 4]
        }
    };
    let mut out = image::RgbaImage::new(width, height);
    for (x, y, px_out) in out.enumerate_pixels_mut() {
        *px_out = image::Rgba(blend_difference(
            sample(&a, ax, ay, x, y),
            sample(&b, bx, by, x, y),
        ));
    }
    // `1320-sharp-image-difference`: resample the blend (never the inputs)
    // to its device pixels, keeping its aspect inside the target
    if let Some((tw, th)) = target {
        let fit = (tw as f32 / width as f32).min(th as f32 / height as f32);
        let w = ((width as f32 * fit).round() as u32).max(1);
        let h = ((height as f32 * fit).round() as u32).max(1);
        if (w, h) != (width, height) {
            let filter = if w < width {
                FilterType::Triangle
            } else {
                FilterType::Nearest
            };
            out = image::imageops::resize(&out, w, h, filter);
        }
    }
    let mut bytes = Vec::new();
    out.write_to(
        &mut std::io::Cursor::new(&mut bytes),
        image::ImageFormat::Png,
    )
    .ok()?;
    Some(Image::from_bytes(ImageFormat::Png, bytes))
}

/// W3C compositing: `source` blended onto `backdrop` with the `difference`
/// mode, then composited source-over (non-premultiplied RGBA8 in and out).
fn blend_difference(backdrop: [u8; 4], source: [u8; 4]) -> [u8; 4] {
    let ab = backdrop[3] as f32 / 255.;
    let as_ = source[3] as f32 / 255.;
    let ao = as_ + ab * (1. - as_);
    if ao <= 0. {
        return [0; 4];
    }
    let mut out = [0u8; 4];
    for i in 0..3 {
        let cb = backdrop[i] as f32 / 255.;
        let cs = source[i] as f32 / 255.;
        let mixed = (1. - ab) * cs + ab * (cb - cs).abs();
        let co = as_ * mixed + (1. - as_) * ab * cb;
        out[i] = ((co / ao) * 255.).round().clamp(0., 255.) as u8;
    }
    out[3] = (ao * 255.).round() as u8;
    out
}

#[cfg(test)]
mod tests {
    use super::{Side, TGA_MEDIA_TYPE, blend_difference, upscale_factor};
    use corvene_core::{ImageBlob, ImageOriginal};

    fn blob(bytes: Vec<u8>, media_type: &str) -> ImageBlob {
        ImageBlob {
            bytes,
            media_type: media_type.to_string(),
            original: None,
        }
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(w, h, image::Rgba([0, 0, 255, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        png
    }

    #[test]
    fn tga_decodes_to_png() {
        let mut tga = Vec::new();
        image::RgbaImage::from_pixel(3, 2, image::Rgba([255, 0, 0, 255]))
            .write_to(&mut std::io::Cursor::new(&mut tga), image::ImageFormat::Tga)
            .unwrap();
        let side = Side::from_blob(&blob(tga.clone(), TGA_MEDIA_TYPE));
        assert_eq!(side.size, Some((3, 2)));
        assert_eq!(side.bytes, tga.len());
        assert!(side.image.unwrap().bytes.starts_with(b"\x89PNG"));
    }

    #[test]
    fn svg_is_measured_and_enlarged_by_a_whole_factor() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="12"><rect width="24" height="12" fill="#ff0000"/></svg>"##;
        let side = Side::from_blob(&blob(svg.to_vec(), corvene_git::SVG_MEDIA_TYPE));
        assert_eq!(side.natural, Some((24, 12)));
        // 256 / 24 = 10
        assert_eq!(side.size, Some((240, 120)));
        assert!(side.svg.is_some() && side.error.is_none());
        // drawn later at the size it shows at, never re-encoded
        assert_eq!(side.pixelated().size, Some((240, 120)));
        let broken = Side::from_blob(&blob(b"<svg".to_vec(), corvene_git::SVG_MEDIA_TYPE));
        assert_eq!(broken.size, None);
        assert!(broken.svg.is_none());
        assert!(broken.error.unwrap().contains("could not be read"));
    }

    #[test]
    fn svgz_is_measured_too() {
        use std::io::Write;
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="300" height="100"/>"##;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(svg).unwrap();
        let side = Side::from_blob(&blob(gz.finish().unwrap(), corvene_git::SVG_MEDIA_TYPE));
        assert_eq!(side.natural, Some((300, 100)));
    }

    #[test]
    fn converted_images_keep_the_file_size() {
        let mut converted = blob(png(40, 20), "image/png");
        converted.original = Some(ImageOriginal {
            media_type: "image/heic".to_string(),
            bytes: 12_345,
            width: 4000,
            height: 2000,
        });
        let side = Side::from_blob(&converted);
        assert_eq!(side.bytes, 12_345);
        assert_eq!(side.natural, Some((4000, 2000)));
        assert_eq!(side.size, Some((4000, 2000)));
        assert_eq!(side.bitmap, Some((40, 20)));
        // a smaller picture of a large image is not enlarged
        assert_eq!(side.pixelated().bitmap, Some((40, 20)));
        let broken = Side::from_blob(&blob(b"not an image".to_vec(), "image/png"));
        assert!(broken.image.is_none() && broken.error.is_some());
    }

    #[test]
    fn small_images_get_whole_factors() {
        assert_eq!(upscale_factor((16, 16)), 16);
        assert_eq!(upscale_factor((32, 8)), 8);
        assert_eq!(upscale_factor((63, 10)), 4);
        assert_eq!(upscale_factor((64, 10)), 1);
        assert_eq!(upscale_factor((0, 0)), 1);
        let side = Side::from_blob(&blob(png(4, 2), "image/png")).pixelated();
        assert_eq!(side.size, Some((64, 32)));
        assert_eq!(side.natural, Some((4, 2)));
    }

    #[test]
    fn difference_blend_matches_css() {
        // opaque over opaque: |b - s|
        assert_eq!(
            blend_difference([200, 100, 0, 255], [50, 100, 255, 255]),
            [150, 0, 255, 255]
        );
        // nothing under the source: the source shows as is
        assert_eq!(
            blend_difference([0; 4], [10, 20, 30, 255]),
            [10, 20, 30, 255]
        );
        // nothing over the backdrop: the backdrop shows as is
        assert_eq!(
            blend_difference([10, 20, 30, 255], [0; 4]),
            [10, 20, 30, 255]
        );
        assert_eq!(blend_difference([0; 4], [0; 4]), [0; 4]);
    }
}
