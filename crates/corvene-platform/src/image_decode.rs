//! Corvene flag `1326-more-image-formats`: image files GitHub Desktop shows
//! as "This binary file has changed." (its `imageFileExtensions` in
//! app/src/lib/git/diff.ts are png, jpg, jpeg, gif, ico, webp, bmp and
//! avif), decoded to a PNG the image diff can draw.
//!
//! Every OS: TIFF, PNM, QOI, Radiance HDR, OpenEXR, DDS, farbfeld and TGA
//! (the `image` crate), Photoshop (`psd`: 8-bit RGB and grayscale) and ICNS
//! (`icns`: its largest icon). macOS adds ImageIO (HEIC/HEIF, AVIF,
//! JPEG XL, JPEG 2000, camera RAW), Windows WIC (the same when the codec
//! extensions are installed); both also take over when the Rust decoder
//! fails. Linux and Android have the Rust decoders only.
//!
//! Third-party decoders run under `catch_unwind`, which only helps where
//! panics unwind (not the `panic = "abort"` release profile).

use std::cmp::Reverse;
use std::io::Cursor;
use std::panic::{AssertUnwindSafe, catch_unwind};

use image::codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder};
use image::imageops::{self, FilterType};
use image::metadata::Orientation;
use image::{
    DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageFormat, ImageReader, Limits,
    RgbaImage,
};

#[cfg(target_os = "macos")]
mod imageio;
#[cfg(windows)]
mod wic;

/// A decoded image for display: `png` is at most `max_side` px on its longer side
/// (downscaled with a good filter, aspect kept); `width`/`height` are the image's
/// own size (after EXIF/TIFF orientation is applied).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedImage {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Inputs past this are not decoded.
const MAX_INPUT_BYTES: usize = 256 << 20;
/// Longest side an image may have.
const MAX_DIMENSION: u32 = 30_000;
/// Most pixels an image may have.
const MAX_AREA: u64 = 100_000_000;
/// Decoder allocation cap: 100 MP of RGBA f32 is 1.6 GB.
const MAX_ALLOC: u64 = 2 << 30;

/// Types only the OS decoders read.
#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
const OS_ONLY: &[&str] = &[
    "image/heic",
    "image/heif",
    "image/avif",
    "image/jxl",
    "image/jp2",
    "image/x-raw",
];

/// Decode `bytes` of an image whose media type is `media_type` (see list below).
/// `None` when nothing on this platform can decode it.
///
/// Rust decoders: `image/tiff`, `image/x-portable-anymap`, `image/qoi`,
/// `image/vnd.radiance`, `image/x-exr`, `image/vnd-ms.dds`,
/// `image/x-farbfeld`, `image/x-tga`, `image/vnd.adobe.photoshop`,
/// `image/x-icns`. macOS and Windows also: `image/heic`, `image/heif`,
/// `image/avif`, `image/jxl`, `image/jp2`, `image/x-raw`.
pub fn decode(bytes: &[u8], media_type: &str, max_side: u32) -> Option<DecodedImage> {
    if bytes.len() > MAX_INPUT_BYTES || !known(media_type) {
        return None;
    }
    let max_side = max_side.max(1);
    guarded(|| decode_rust(bytes, media_type, max_side))
        .or_else(|| guarded(|| decode_os(bytes, media_type, max_side)))
}

fn known(media_type: &str) -> bool {
    image_format(media_type).is_some()
        || matches!(media_type, "image/vnd.adobe.photoshop" | "image/x-icns")
        || OS_ONLY.contains(&media_type)
}

fn image_format(media_type: &str) -> Option<ImageFormat> {
    Some(match media_type {
        "image/tiff" => ImageFormat::Tiff,
        "image/x-portable-anymap" => ImageFormat::Pnm,
        "image/qoi" => ImageFormat::Qoi,
        "image/vnd.radiance" => ImageFormat::Hdr,
        "image/x-exr" => ImageFormat::OpenExr,
        "image/vnd-ms.dds" => ImageFormat::Dds,
        "image/x-farbfeld" => ImageFormat::Farbfeld,
        "image/x-tga" => ImageFormat::Tga,
        _ => return None,
    })
}

/// `f()`, with a panic turned into `None`.
fn guarded<T>(f: impl FnOnce() -> Option<T>) -> Option<T> {
    catch_unwind(AssertUnwindSafe(f)).ok().flatten()
}

fn size_ok(width: u32, height: u32) -> bool {
    width > 0
        && height > 0
        && width <= MAX_DIMENSION
        && height <= MAX_DIMENSION
        && u64::from(width) * u64::from(height) <= MAX_AREA
}

fn decode_rust(bytes: &[u8], media_type: &str, max_side: u32) -> Option<DecodedImage> {
    let rgba = match media_type {
        "image/vnd.adobe.photoshop" => psd_rgba(bytes)?,
        "image/x-icns" => icns_rgba(bytes)?,
        _ => image_rgba(bytes, image_format(media_type)?)?,
    };
    let (width, height) = rgba.dimensions();
    finish(&rgba, width, height, max_side)
}

/// Downscale `rgba` to `max_side` and PNG-encode it; `width`/`height` are
/// the natural size.
fn finish(rgba: &RgbaImage, width: u32, height: u32, max_side: u32) -> Option<DecodedImage> {
    let png = match downscale(rgba, max_side) {
        Some(small) => encode_png(&small)?,
        None => encode_png(rgba)?,
    };
    Some(DecodedImage { png, width, height })
}

/// The `image` crate's decoder, size-checked before the pixels are read,
/// with its orientation applied.
fn image_rgba(bytes: &[u8], format: ImageFormat) -> Option<RgbaImage> {
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().ok()?;
    let (width, height) = decoder.dimensions();
    if !size_ok(width, height) {
        return None;
    }
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder).ok()?;
    image.apply_orientation(orientation);
    to_rgba8(image)
}

/// 8-bit RGBA; float images (HDR, EXR) are linear light and go through the
/// sRGB transfer curve, deeper integer formats scale down.
fn to_rgba8(image: DynamicImage) -> Option<RgbaImage> {
    match image {
        DynamicImage::ImageRgb32F(_) | DynamicImage::ImageRgba32F(_) => {
            let linear = image.into_rgba32f();
            let (width, height) = linear.dimensions();
            let data = linear
                .into_raw()
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|&[r, g, b, a]| [srgb(r), srgb(g), srgb(b), unit(a)])
                .collect();
            RgbaImage::from_raw(width, height, data)
        }
        other => Some(other.into_rgba8()),
    }
}

fn clamp01(v: f32) -> f32 {
    if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) }
}

fn unit(v: f32) -> u8 {
    (clamp01(v) * 255.0 + 0.5) as u8
}

/// Linear light to the sRGB transfer curve.
fn srgb(v: f32) -> u8 {
    let v = clamp01(v);
    let s = if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    unit(s)
}

/// Photoshop's composite image, read by `psd` for 8-bit RGB and
/// single-channel grayscale (other modes, depths and PSB go to the OS).
fn psd_rgba(bytes: &[u8]) -> Option<RgbaImage> {
    // header: "8BPS", version (1; PSB is 2), 6 reserved, channels,
    // height, width, depth, colour mode, all big-endian
    let header = bytes.get(..26)?;
    let be16 = |at: usize| u16::from_be_bytes([header[at], header[at + 1]]);
    let be32 = |at: usize| {
        u32::from_be_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
    };
    if &header[..4] != b"8BPS" || be16(4) != 1 || be16(22) != 8 {
        return None;
    }
    let (channels, height, width, mode) = (be16(12), be32(14), be32(18), be16(24));
    if !size_ok(width, height) || !matches!((mode, channels), (3, _) | (1, 1)) {
        return None;
    }
    let psd = psd::Psd::from_bytes(bytes).ok()?;
    RgbaImage::from_raw(psd.width(), psd.height(), psd.rgba())
}

/// The largest icon of an `.icns` that decodes.
fn icns_rgba(bytes: &[u8]) -> Option<RgbaImage> {
    let family = icns::IconFamily::read(Cursor::new(bytes)).ok()?;
    let mut types = family.available_icons();
    types.sort_by_key(|t| Reverse(u64::from(t.pixel_width()) * u64::from(t.pixel_height())));
    types.into_iter().find_map(|t| {
        let icon = family
            .get_icon_with_type(t)
            .ok()?
            .convert_to(icns::PixelFormat::RGBA);
        RgbaImage::from_raw(icon.width(), icon.height(), icon.into_data().into_vec())
    })
}

/// The size `width`×`height` fits in `max_side`, aspect kept.
fn fit(width: u32, height: u32, max_side: u32) -> (u32, u32) {
    let longer = width.max(height);
    if longer <= max_side {
        return (width, height);
    }
    let scale = f64::from(max_side) / f64::from(longer);
    let side = |v: u32| ((f64::from(v) * scale).round() as u32).clamp(1, max_side);
    (side(width), side(height))
}

/// `rgba` shrunk to fit `max_side` (Catmull-Rom; an area-averaging pass
/// first when it shrinks more than 4×), `None` when it already fits.
fn downscale(rgba: &RgbaImage, max_side: u32) -> Option<RgbaImage> {
    let (width, height) = rgba.dimensions();
    let (w, h) = fit(width, height, max_side);
    if (w, h) == (width, height) {
        return None;
    }
    if width.max(height) / 4 > w.max(h) {
        let coarse = imageops::thumbnail(rgba, w * 2, h * 2);
        return Some(imageops::resize(&coarse, w, h, FilterType::CatmullRom));
    }
    Some(imageops::resize(rgba, w, h, FilterType::CatmullRom))
}

fn encode_png(rgba: &RgbaImage) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    PngEncoder::new_with_quality(&mut out, CompressionType::Fast, PngFilter::Adaptive)
        .write_image(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            ExtendedColorType::Rgba8,
        )
        .ok()?;
    Some(out)
}

#[cfg(target_os = "macos")]
fn decode_os(bytes: &[u8], media_type: &str, max_side: u32) -> Option<DecodedImage> {
    let image = imageio::decode(bytes, media_type, max_side, size_ok)?;
    finish(&image.rgba, image.width, image.height, max_side)
}

#[cfg(windows)]
fn decode_os(bytes: &[u8], _media_type: &str, max_side: u32) -> Option<DecodedImage> {
    let decoded = wic::decode(bytes, max_side, size_ok)?;
    let rgba = RgbaImage::from_raw(decoded.width, decoded.height, decoded.rgba)?;
    let orientation =
        Orientation::from_exif(decoded.orientation).unwrap_or(Orientation::NoTransforms);
    let mut image = DynamicImage::ImageRgba8(rgba);
    image.apply_orientation(orientation);
    let (width, height) = if (5..=8).contains(&decoded.orientation) {
        (decoded.natural_height, decoded.natural_width)
    } else {
        (decoded.natural_width, decoded.natural_height)
    };
    finish(&image.into_rgba8(), width, height, max_side)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn decode_os(_bytes: &[u8], _media_type: &str, _max_side: u32) -> Option<DecodedImage> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb, Rgba};

    fn gradient(width: u32, height: u32) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| {
            Rgba([x as u8 * 2, y as u8 * 3, 128, 255])
        })
    }

    fn encode(image: &DynamicImage, format: ImageFormat) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        image.write_to(&mut out, format).unwrap();
        out.into_inner()
    }

    fn png_pixels(decoded: &DecodedImage) -> RgbaImage {
        image::load_from_memory_with_format(&decoded.png, ImageFormat::Png)
            .unwrap()
            .into_rgba8()
    }

    fn assert_round_trip(image: DynamicImage, format: ImageFormat, media_type: &str) {
        let bytes = encode(&image, format);
        let decoded = decode(&bytes, media_type, 1000).unwrap();
        assert_eq!((decoded.width, decoded.height), (40, 30), "{media_type}");
        let pixels = png_pixels(&decoded);
        assert_eq!(pixels.dimensions(), (40, 30), "{media_type}");
        assert_eq!(pixels.get_pixel(5, 7).0[..3], [10, 21, 128], "{media_type}");
    }

    #[test]
    fn integer_formats_round_trip() {
        let rgba = DynamicImage::ImageRgba8(gradient(40, 30));
        assert_round_trip(rgba.clone(), ImageFormat::Tiff, "image/tiff");
        assert_round_trip(rgba.clone(), ImageFormat::Qoi, "image/qoi");
        assert_round_trip(rgba.clone(), ImageFormat::Tga, "image/x-tga");
        assert_round_trip(
            DynamicImage::ImageRgb8(rgba.to_rgb8()),
            ImageFormat::Pnm,
            "image/x-portable-anymap",
        );
        // farbfeld is 16-bit: scaled down to 8
        assert_round_trip(
            DynamicImage::ImageRgba16(rgba.to_rgba16()),
            ImageFormat::Farbfeld,
            "image/x-farbfeld",
        );
    }

    #[test]
    fn float_formats_get_the_srgb_curve() {
        // linear 0.5 is sRGB 188, not 128
        let hdr = ImageBuffer::from_pixel(4, 2, Rgb([0.5_f32, 0.0, 1.0]));
        let bytes = encode(&DynamicImage::ImageRgb32F(hdr), ImageFormat::Hdr);
        let decoded = decode(&bytes, "image/vnd.radiance", 100).unwrap();
        assert_eq!((decoded.width, decoded.height), (4, 2));
        let p = png_pixels(&decoded).get_pixel(1, 1).0;
        assert!(
            p[0].abs_diff(188) <= 1 && p[1] == 0 && p[2] == 255 && p[3] == 255,
            "{p:?}"
        );

        let exr = ImageBuffer::from_pixel(3, 5, Rgba([0.5_f32, 0.25, 2.0, 0.5]));
        let bytes = encode(&DynamicImage::ImageRgba32F(exr), ImageFormat::OpenExr);
        let decoded = decode(&bytes, "image/x-exr", 100).unwrap();
        assert_eq!((decoded.width, decoded.height), (3, 5));
        let p = png_pixels(&decoded).get_pixel(2, 4).0;
        assert!(p[0].abs_diff(188) <= 1, "{p:?}");
        assert!(p[1].abs_diff(137) <= 1, "{p:?}");
        assert_eq!(p[2], 255);
        assert!(p[3].abs_diff(128) <= 1, "{p:?}");
    }

    #[test]
    fn large_images_are_downscaled_to_max_side() {
        let bytes = encode(
            &DynamicImage::ImageRgba8(gradient(100, 50)),
            ImageFormat::Tiff,
        );
        let decoded = decode(&bytes, "image/tiff", 20).unwrap();
        assert_eq!((decoded.width, decoded.height), (100, 50));
        assert_eq!(png_pixels(&decoded).dimensions(), (20, 10));

        // past 4× goes through the coarse pass too
        let decoded = decode(&bytes, "image/tiff", 10).unwrap();
        assert_eq!(png_pixels(&decoded).dimensions(), (10, 5));
    }

    #[test]
    fn fit_keeps_the_aspect() {
        assert_eq!(fit(100, 50, 20), (20, 10));
        assert_eq!(fit(50, 100, 20), (10, 20));
        assert_eq!(fit(10, 5, 20), (10, 5));
        assert_eq!(fit(10_000, 1, 100), (100, 1));
    }

    #[test]
    fn garbage_and_unknown_types_are_none() {
        assert_eq!(decode(b"not an image at all", "image/tiff", 100), None);
        assert_eq!(
            decode(b"not an image at all", "image/vnd.adobe.photoshop", 100),
            None
        );
        assert_eq!(decode(b"not an image at all", "image/x-icns", 100), None);
        assert_eq!(decode(b"not an image at all", "image/heic", 100), None);
        let png = encode(&DynamicImage::ImageRgba8(gradient(4, 4)), ImageFormat::Tiff);
        assert_eq!(decode(&png, "image/png", 100), None);
        assert_eq!(decode(&png, "application/octet-stream", 100), None);
    }

    #[test]
    fn oversized_images_are_refused() {
        assert!(size_ok(30_000, 3_000));
        assert!(!size_ok(30_001, 1));
        assert!(!size_ok(20_000, 6_000));
        assert!(!size_ok(0, 10));
    }

    #[test]
    fn psd_header_checks_refuse_unsupported_files() {
        let mut header = b"8BPS\0\x01\0\0\0\0\0\0\0\x03\0\0\0\x02\0\0\0\x02\0\x10\0\x03".to_vec();
        // 16-bit: left to the OS decoder
        assert_eq!(psd_rgba(&header), None);
        // PSB
        header[5] = 2;
        assert_eq!(psd_rgba(&header), None);
    }
}
