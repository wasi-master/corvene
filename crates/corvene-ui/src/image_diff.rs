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
//! Deviation (`794-svg-image-diff`): an SVG file switched to Image is
//! rendered with resvg (a small drawing enlarged, drawn sharp).
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

struct Side {
    image: Arc<Image>,
    bytes: usize,
    /// Size of `image` in pixels (`None` when the format could not be
    /// decoded): the natural size unless `761-pixelated-small-images`
    /// enlarged it.
    size: Option<(u32, u32)>,
    /// The file's own size in pixels, for the footer.
    natural: Option<(u32, u32)>,
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
        let size = image::ImageReader::new(std::io::Cursor::new(&blob.bytes))
            .with_guessed_format()
            .ok()
            .and_then(|r| r.into_dimensions().ok());
        Self {
            image: Arc::new(Image::from_bytes(format, blob.bytes.clone())),
            bytes: blob.bytes.len(),
            size,
            natural: size,
        }
    }

    /// `761-pixelated-small-images`: a small still image re-encoded at a
    /// whole multiple of its size with nearest-neighbour sampling (GPUI
    /// would blur it when drawing it larger). GIFs keep their animation.
    fn pixelated(mut self) -> Self {
        let Some((w, h)) = self.size else {
            return self;
        };
        // an SVG is already drawn at the size it shows at
        if self.size != self.natural {
            return self;
        }
        let k = upscale_factor((w, h));
        if k < 2 || self.image.format == ImageFormat::Gif {
            return self;
        }
        let Ok(decoded) = image::load_from_memory(&self.image.bytes) else {
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
        self.image = Arc::new(Image::from_bytes(ImageFormat::Png, png));
        self.size = Some((w * k, h * k));
        self
    }
}

/// `794-svg-image-diff`: an SVG drawing is rendered to at most this many
/// pixels on its longer side.
const MAX_SVG_PIXELS: f32 = 4096.;

impl Side {
    /// `794-svg-image-diff`: an SVG drawing rendered as PNG (resvg, the
    /// renderer GPUI uses for its icons) at its own size, a small one
    /// enlarged like `761-pixelated-small-images` enlarges small images but
    /// drawn sharp; the footer shows the drawing's own size. An SVG that
    /// does not parse shows no image.
    fn from_svg(blob: &ImageBlob) -> Self {
        let rendered = (|| {
            let tree =
                resvg::usvg::Tree::from_data(&blob.bytes, &resvg::usvg::Options::default()).ok()?;
            let size = tree.size();
            let natural = (
                size.width().ceil().max(1.) as u32,
                size.height().ceil().max(1.) as u32,
            );
            let longest = size.width().max(size.height()).max(1.);
            let scale = (upscale_factor(natural) as f32).min(MAX_SVG_PIXELS / longest);
            let (w, h) = (
                ((size.width() * scale).ceil() as u32).max(1),
                ((size.height() * scale).ceil() as u32).max(1),
            );
            let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
            resvg::render(
                &tree,
                resvg::tiny_skia::Transform::from_scale(scale, scale),
                &mut pixmap.as_mut(),
            );
            Some(((w, h), natural, pixmap.encode_png().ok()?))
        })();
        let (size, natural, bytes) = match rendered {
            Some((size, natural, png)) => (Some(size), Some(natural), png),
            None => (None, None, Vec::new()),
        };
        Self {
            image: Arc::new(Image::from_bytes(ImageFormat::Png, bytes)),
            bytes: blob.bytes.len(),
            size,
            natural,
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
        let (size, bytes) = match png {
            Some((size, png)) => (Some(size), png),
            None => (None, Vec::new()),
        };
        Self {
            image: Arc::new(Image::from_bytes(ImageFormat::Png, bytes)),
            bytes: blob.bytes.len(),
            size,
            natural: size,
        }
    }
}

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
    /// `752-image-diff-border-outside`: the 1 px border is drawn around the
    /// fitted image instead of inside it (`box-sizing: content-box`).
    border_outside: bool,
    /// `1322-pdf-diff`: the (previous, current) footers of a PDF page pair,
    /// in place of the PNG's pixels and bytes; also hides the 2-up size
    /// summary.
    page_footers: Option<(SharedString, SharedString)>,
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
        let pixelated = corvene_core::AppState::try_global(cx).is_some_and(|s| {
            s.read(cx)
                .flags
                .bool(corvene_core::flags::ids::PIXELATED_SMALL_IMAGES)
        });
        let side = |blob: &ImageBlob| {
            let side = Side::from_blob(blob);
            if pixelated { side.pixelated() } else { side }
        };
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
            border_outside: corvene_core::AppState::try_global(cx).is_some_and(|s| {
                s.read(cx)
                    .flags
                    .bool(corvene_core::flags::ids::IMAGE_DIFF_BORDER_OUTSIDE)
            }),
            page_footers: None,
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
                .and_then(|s| s.size)
                .map(|s| Self::aspect_fit(s, container))
                .unwrap_or_default()
        };
        let (p, c) = (fit(&self.previous), fit(&self.current));
        size(p.width.max(c.width), p.height.max(c.height))
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

    fn image_element(
        side: &Side,
        box_size: Size<Pixels>,
        border: Hsla,
        extra: Pixels,
        dark: bool,
    ) -> AnyElement {
        let fit = side
            .size
            .map(|s| Self::aspect_fit(s, box_size))
            .unwrap_or(box_size);
        div()
            .relative()
            .w(fit.width + extra)
            .h(fit.height + extra)
            .child(checkerboard(dark))
            .child(
                img(side.image.clone())
                    .absolute()
                    .inset_0()
                    .size_full()
                    .object_fit(ObjectFit::Contain)
                    .border_1()
                    .border_color(border),
            )
            .into_any_element()
    }

    /// An overlaid image (`.image-diff-previous` / `.image-diff-current` inside
    /// `.image-container`): absolutely positioned over the whole box, centered,
    /// at most the box size (`maxSize`), transparent background.
    fn overlay_image(
        side: &Side,
        box_size: Size<Pixels>,
        border: Option<Hsla>,
        extra: Pixels,
        top_left: bool,
    ) -> AnyElement {
        let fit = side
            .size
            .map(|s| Self::aspect_fit(s, box_size))
            .unwrap_or(box_size);
        div()
            .absolute()
            .top_0()
            .left_0()
            .w(box_size.width)
            .h(box_size.height)
            .flex()
            .when(!top_left, |d| d.items_center().justify_center())
            .child(
                img(side.image.clone())
                    .flex_none()
                    .w(fit.width + if border.is_some() { extra } else { px(0.) })
                    .h(fit.height + if border.is_some() { extra } else { px(0.) })
                    .object_fit(ObjectFit::Contain)
                    .when_some(border, |d, color| d.border_1().border_color(color)),
            )
            .into_any_element()
    }

    /// Display scale of `side` inside the overlay box (1 = natural size).
    fn display_scale(side: &Side, box_size: Size<Pixels>) -> f32 {
        match side.size {
            Some((w, h)) if w > 0 => f32::from(Self::aspect_fit((w, h), box_size).width) / w as f32,
            _ => 1.,
        }
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
        let extra = self.border_extra();
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
                .child(Self::image_element(side, image_box, color, extra, dark))
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
    fn overlay_box(&self, cx: &Context<Self>) -> Size<Pixels> {
        let container = self.container.get();
        let _ = cx;
        self.max_fit(size(container.width, container.height))
    }

    /// `Swipe`: the slider reveals the new image from the right.
    fn swipe(&self, previous: &Side, current: &Side, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let extra = self.border_extra();
        let top_left = Self::top_left(cx);
        let box_size = self.overlay_box(cx);
        let percentage = self.swipe.read(cx).value().start();
        let swiper_width = (box_size.width * (1. - percentage / 100.)).floor();
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
                            .child(
                                // previous: `clip-path: inset(0 swiper 0 0)`
                                div()
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .h(box_size.height)
                                    .w((box_size.width - swiper_width).max(zpx(0.)))
                                    .overflow_hidden()
                                    .child(Self::overlay_image(
                                        previous,
                                        box_size,
                                        Some(t.color_deleted),
                                        extra,
                                        top_left,
                                    )),
                            )
                            .child(
                                // current: `clip-path: inset(0 0 0 width - swiper)`
                                div()
                                    .absolute()
                                    .top_0()
                                    .right_0()
                                    .h(box_size.height)
                                    .w(swiper_width.max(zpx(0.)))
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .absolute()
                                            .top_0()
                                            .right_0()
                                            .w(box_size.width)
                                            .h(box_size.height)
                                            .child(Self::overlay_image(
                                                current,
                                                box_size,
                                                Some(t.color_new),
                                                extra,
                                                top_left,
                                            )),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// `OnionSkin`: the slider cross-fades the new image over the old one.
    fn onion_skin(&self, previous: &Side, current: &Side, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let extra = self.border_extra();
        let top_left = Self::top_left(cx);
        let box_size = self.overlay_box(cx);
        let crossfade = self.onion.read(cx).value().start() / 100.;
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
                            .child(Self::overlay_image(
                                previous,
                                box_size,
                                Some(t.color_deleted),
                                extra,
                                top_left,
                            ))
                            .child(div().absolute().inset_0().opacity(crossfade).child(
                                Self::overlay_image(
                                    current,
                                    box_size,
                                    Some(t.color_new),
                                    extra,
                                    top_left,
                                ),
                            )),
                    ),
            )
            .into_any_element()
    }

    /// `DifferenceBlend`: the current image drawn over the previous one with
    /// `mix-blend-mode: difference`, both centered at their display scale; no
    /// borders, no checkerboard.
    fn difference(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let box_size = self.overlay_box(cx);
        let scales = match (&self.previous, &self.current) {
            (Some(p), Some(c)) => (
                Self::display_scale(p, box_size),
                Self::display_scale(c, box_size),
            ),
            _ => (1., 1.),
        };
        let top_left = Self::top_left(cx);
        // `1320-sharp-image-difference`: the blend's size on screen in
        // device pixels (the larger fitted image, as the box is)
        let scale_factor = window.scale_factor();
        let target = corvene_core::AppState::try_global(cx)
            .is_some_and(|s| {
                s.read(cx)
                    .flags
                    .bool(corvene_core::flags::ids::SHARP_IMAGE_DIFFERENCE)
            })
            .then(|| {
                let w = (f32::from(box_size.width) * scale_factor).round() as u32;
                let h = (f32::from(box_size.height) * scale_factor).round() as u32;
                (w.max(1), h.max(1))
            });
        let stale = top_left != self.difference_top_left
            || target != self.difference_target
            || self.difference_scales.is_none_or(|(p, c)| {
                (p / c - scales.0 / scales.1).abs() > 0.005 * (scales.0 / scales.1)
            });
        if stale && !self.difference_pending && box_size.width > zpx(0.) {
            self.difference_pending = true;
            let a = self.previous.as_ref().map(|s| s.image.clone());
            let b = self.current.as_ref().map(|s| s.image.clone());
            let task = cx.background_executor().spawn(async move {
                let (a, b) = (a?, b?);
                difference_image(&a.bytes, &b.bytes, scales.0, scales.1, top_left, target)
            });
            cx.spawn(async move |this, cx| {
                let result = task.await;
                this.update(cx, |this, cx| {
                    this.difference_pending = false;
                    this.difference_scales = Some(scales);
                    this.difference_top_left = top_left;
                    this.difference_target = target;
                    this.difference = result.map(Arc::new);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        div()
            .id("image-diff-sizing")
            .relative()
            .size_full()
            .flex()
            .justify_center()
            .items_center()
            .min_h_0()
            .child(self.measure(cx))
            .when_some(self.difference.clone(), |d, image| {
                // drawn at exactly the device pixels it was made for
                let (w, h) = match self.difference_target {
                    Some((w, h)) => (px(w as f32 / scale_factor), px(h as f32 / scale_factor)),
                    None => (box_size.width, box_size.height),
                };
                d.child(
                    img(image)
                        .flex_none()
                        .w(w)
                        .h(h)
                        .object_fit(ObjectFit::Contain),
                )
            })
            .into_any_element()
    }

    /// `NewImageDiff` / `DeletedImageDiff`: one image with its header.
    fn single(&self, side: &Side, label: &str, color: Hsla, cx: &Context<Self>) -> AnyElement {
        let extra = self.border_extra();
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
            .child(Self::image_element(side, image_box, color, extra, dark))
            .into_any_element()
    }
}

impl Render for ImageDiff {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd().clone();
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
        // `.panel.image`
        div()
            .id("image-diff")
            .flex_1()
            .min_h_0()
            .min_w_0()
            .p(SPACING())
            .bg(t.background)
            .text_size(FONT_SIZE())
            .child(body)
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
    a: &[u8],
    b: &[u8],
    scale_a: f32,
    scale_b: f32,
    top_left: bool,
    target: Option<(u32, u32)>,
) -> Option<Image> {
    use image::imageops::FilterType;
    let a = image::load_from_memory(a).ok()?.to_rgba8();
    let b = image::load_from_memory(b).ok()?.to_rgba8();
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
    use corvene_core::ImageBlob;

    #[test]
    fn tga_decodes_to_png() {
        let mut tga = Vec::new();
        image::RgbaImage::from_pixel(3, 2, image::Rgba([255, 0, 0, 255]))
            .write_to(&mut std::io::Cursor::new(&mut tga), image::ImageFormat::Tga)
            .unwrap();
        let blob = ImageBlob {
            bytes: tga.clone(),
            media_type: TGA_MEDIA_TYPE.to_string(),
        };
        let side = Side::from_blob(&blob);
        assert_eq!(side.size, Some((3, 2)));
        assert_eq!(side.bytes, tga.len());
        assert!(side.image.bytes.starts_with(b"\x89PNG"));
    }

    #[test]
    fn svg_renders_sharp_at_a_whole_factor() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="12"><rect width="24" height="12" fill="#ff0000"/></svg>"##;
        let blob = ImageBlob {
            bytes: svg.to_vec(),
            media_type: corvene_git::SVG_MEDIA_TYPE.to_string(),
        };
        let side = Side::from_blob(&blob);
        assert_eq!(side.natural, Some((24, 12)));
        // 256 / 24 = 10
        assert_eq!(side.size, Some((240, 120)));
        let png = image::load_from_memory(&side.image.bytes)
            .unwrap()
            .to_rgba8();
        assert_eq!(png.dimensions(), (240, 120));
        assert_eq!(png.get_pixel(120, 60), &image::Rgba([255, 0, 0, 255]));
        // already enlarged: left alone
        assert_eq!(side.pixelated().size, Some((240, 120)));
        let broken = ImageBlob {
            bytes: b"<svg".to_vec(),
            media_type: corvene_git::SVG_MEDIA_TYPE.to_string(),
        };
        assert_eq!(Side::from_blob(&broken).size, None);
    }

    #[test]
    fn small_images_get_whole_factors() {
        assert_eq!(upscale_factor((16, 16)), 16);
        assert_eq!(upscale_factor((32, 8)), 8);
        assert_eq!(upscale_factor((63, 10)), 4);
        assert_eq!(upscale_factor((64, 10)), 1);
        assert_eq!(upscale_factor((0, 0)), 1);
        let blob = ImageBlob {
            bytes: {
                let mut png = Vec::new();
                image::RgbaImage::from_pixel(4, 2, image::Rgba([0, 0, 255, 255]))
                    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
                    .unwrap();
                png
            },
            media_type: "image/png".to_string(),
        };
        let side = Side::from_blob(&blob).pixelated();
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
