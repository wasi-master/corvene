//! macOS ImageIO: `CGImageSourceCreateThumbnailAtIndex` at the wanted size
//! with the EXIF orientation applied, drawn into an sRGB RGBA8 bitmap.
#![allow(non_upper_case_globals)]

use std::ffi::c_void;

use image::RgbaImage;

type CFTypeRef = *const c_void;

#[repr(C)]
struct CFDictionaryKeyCallBacks {
    version: isize,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
    equal: *const c_void,
    hash: *const c_void,
}

#[repr(C)]
struct CFDictionaryValueCallBacks {
    version: isize,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
    equal: *const c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

const kCFNumberSInt64Type: isize = 4;
const kCFStringEncodingUTF8: u32 = 0x0800_0100;
const kCGImageAlphaPremultipliedLast: u32 = 1;
const kCGBitmapByteOrder32Big: u32 = 4 << 12;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: CFTypeRef);
    fn CFGetTypeID(cf: CFTypeRef) -> usize;
    fn CFNumberGetTypeID() -> usize;
    fn CFDataCreate(allocator: CFTypeRef, bytes: *const u8, length: isize) -> CFTypeRef;
    fn CFDictionaryCreate(
        allocator: CFTypeRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        count: isize,
        key_callbacks: *const CFDictionaryKeyCallBacks,
        value_callbacks: *const CFDictionaryValueCallBacks,
    ) -> CFTypeRef;
    fn CFDictionaryGetValue(dict: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    fn CFNumberCreate(allocator: CFTypeRef, kind: isize, value: *const c_void) -> CFTypeRef;
    fn CFNumberGetValue(number: CFTypeRef, kind: isize, value: *mut c_void) -> u8;
    fn CFStringCreateWithBytes(
        allocator: CFTypeRef,
        bytes: *const u8,
        length: isize,
        encoding: u32,
        is_external: u8,
    ) -> CFTypeRef;
    static kCFTypeDictionaryKeyCallBacks: CFDictionaryKeyCallBacks;
    static kCFTypeDictionaryValueCallBacks: CFDictionaryValueCallBacks;
    static kCFBooleanTrue: CFTypeRef;
    static kCFBooleanFalse: CFTypeRef;
}

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    fn CGImageSourceCreateWithData(data: CFTypeRef, options: CFTypeRef) -> CFTypeRef;
    fn CGImageSourceGetCount(source: CFTypeRef) -> usize;
    fn CGImageSourceCopyPropertiesAtIndex(
        source: CFTypeRef,
        index: usize,
        options: CFTypeRef,
    ) -> CFTypeRef;
    fn CGImageSourceCreateThumbnailAtIndex(
        source: CFTypeRef,
        index: usize,
        options: CFTypeRef,
    ) -> CFTypeRef;
    static kCGImageSourceTypeIdentifierHint: CFTypeRef;
    static kCGImageSourceShouldCache: CFTypeRef;
    static kCGImageSourceShouldCacheImmediately: CFTypeRef;
    static kCGImageSourceCreateThumbnailFromImageAlways: CFTypeRef;
    static kCGImageSourceCreateThumbnailWithTransform: CFTypeRef;
    static kCGImageSourceThumbnailMaxPixelSize: CFTypeRef;
    static kCGImagePropertyPixelWidth: CFTypeRef;
    static kCGImagePropertyPixelHeight: CFTypeRef;
    static kCGImagePropertyOrientation: CFTypeRef;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGImageGetWidth(image: CFTypeRef) -> usize;
    fn CGImageGetHeight(image: CFTypeRef) -> usize;
    fn CGColorSpaceCreateWithName(name: CFTypeRef) -> CFTypeRef;
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: CFTypeRef,
        bitmap_info: u32,
    ) -> CFTypeRef;
    fn CGContextDrawImage(context: CFTypeRef, rect: CGRect, image: CFTypeRef);
    static kCGColorSpaceSRGB: CFTypeRef;
}

/// An owned CF object, released on drop.
struct Cf(CFTypeRef);

impl Cf {
    /// `None` for a null (failed) create/copy.
    fn new(ptr: CFTypeRef) -> Option<Cf> {
        (!ptr.is_null()).then_some(Cf(ptr))
    }
}

impl Drop for Cf {
    fn drop(&mut self) {
        // SAFETY: a non-null object this guard owns one reference to
        unsafe { CFRelease(self.0) }
    }
}

/// Pixels at most `max_side` on the longer side and the natural size
/// (orientation applied).
pub(super) struct Decoded {
    pub rgba: RgbaImage,
    pub width: u32,
    pub height: u32,
}

/// The UTI hinting ImageIO at the format (TGA has no signature to sniff).
fn type_hint(media_type: &str) -> Option<&'static str> {
    Some(match media_type {
        "image/tiff" => "public.tiff",
        "image/x-tga" => "com.truevision.tga-image",
        "image/vnd.adobe.photoshop" => "com.adobe.photoshop-image",
        "image/x-icns" => "com.apple.icns",
        "image/x-exr" => "com.ilm.openexr-image",
        "image/vnd.radiance" => "public.radiance",
        "image/vnd-ms.dds" => "com.microsoft.dds",
        "image/heic" => "public.heic",
        "image/heif" => "public.heif",
        "image/avif" => "public.avif",
        "image/jxl" => "public.jpeg-xl",
        "image/jp2" => "public.jpeg-2000",
        _ => return None,
    })
}

fn cf_string(s: &str) -> Option<Cf> {
    // SAFETY: the bytes are valid UTF-8 for the call's duration
    Cf::new(unsafe {
        CFStringCreateWithBytes(
            std::ptr::null(),
            s.as_ptr(),
            s.len() as isize,
            kCFStringEncodingUTF8,
            0,
        )
    })
}

fn cf_dictionary(pairs: &[(CFTypeRef, CFTypeRef)]) -> Option<Cf> {
    let keys: Vec<CFTypeRef> = pairs.iter().map(|p| p.0).collect();
    let values: Vec<CFTypeRef> = pairs.iter().map(|p| p.1).collect();
    // SAFETY: keys and values are live CF objects, retained by the
    // dictionary through the CFType callbacks
    Cf::new(unsafe {
        CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            pairs.len() as isize,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        )
    })
}

/// A number in a properties dictionary.
fn number(dict: &Cf, key: CFTypeRef) -> Option<i64> {
    // SAFETY: `dict` is a live dictionary; the value is borrowed (Get rule)
    // and checked to be a CFNumber before it is read
    unsafe {
        let value = CFDictionaryGetValue(dict.0, key);
        if value.is_null() || CFGetTypeID(value) != CFNumberGetTypeID() {
            return None;
        }
        let mut out: i64 = 0;
        (CFNumberGetValue(value, kCFNumberSInt64Type, (&raw mut out).cast()) != 0).then_some(out)
    }
}

/// The natural size (orientation applied) from the image's properties.
fn natural_size(source: &Cf) -> Option<(u32, u32)> {
    // SAFETY: `source` is a live image source; the copy is owned by the guard
    let properties =
        Cf::new(unsafe { CGImageSourceCopyPropertiesAtIndex(source.0, 0, std::ptr::null()) })?;
    // SAFETY: reading framework constant keys
    let (width_key, height_key, orientation_key) = unsafe {
        (
            kCGImagePropertyPixelWidth,
            kCGImagePropertyPixelHeight,
            kCGImagePropertyOrientation,
        )
    };
    let width = u32::try_from(number(&properties, width_key)?).ok()?;
    let height = u32::try_from(number(&properties, height_key)?).ok()?;
    let orientation = number(&properties, orientation_key).unwrap_or(1);
    Some(if (5..=8).contains(&orientation) {
        (height, width)
    } else {
        (width, height)
    })
}

pub(super) fn decode(
    bytes: &[u8],
    media_type: &str,
    max_side: u32,
    size_ok: fn(u32, u32) -> bool,
) -> Option<Decoded> {
    let length = isize::try_from(bytes.len()).ok()?;
    // SAFETY: CFDataCreate copies `bytes`
    let data = Cf::new(unsafe { CFDataCreate(std::ptr::null(), bytes.as_ptr(), length) })?;
    let hint = type_hint(media_type).and_then(cf_string);
    // SAFETY: framework constants and live CF objects; every create/copy is
    // owned by a guard
    let source_options = unsafe {
        let mut pairs = vec![(kCGImageSourceShouldCache, kCFBooleanFalse)];
        if let Some(hint) = &hint {
            pairs.push((kCGImageSourceTypeIdentifierHint, hint.0));
        }
        cf_dictionary(&pairs)?
    };
    // SAFETY: live data and options dictionary
    let source = Cf::new(unsafe { CGImageSourceCreateWithData(data.0, source_options.0) })?;
    // SAFETY: live image source
    if unsafe { CGImageSourceGetCount(source.0) } == 0 {
        return None;
    }
    let natural = natural_size(&source);
    if let Some((width, height)) = natural
        && !size_ok(width, height)
    {
        return None;
    }
    let longest = natural.map_or(max_side, |(w, h)| w.max(h).min(max_side));
    let longest = i64::from(longest);
    // SAFETY: the number reads `longest` during the call
    let max_pixels = Cf::new(unsafe {
        CFNumberCreate(
            std::ptr::null(),
            kCFNumberSInt64Type,
            (&raw const longest).cast(),
        )
    })?;
    // SAFETY: framework constants and a live number
    let thumbnail_options = unsafe {
        cf_dictionary(&[
            (kCGImageSourceCreateThumbnailFromImageAlways, kCFBooleanTrue),
            (kCGImageSourceCreateThumbnailWithTransform, kCFBooleanTrue),
            (kCGImageSourceThumbnailMaxPixelSize, max_pixels.0),
            (kCGImageSourceShouldCache, kCFBooleanFalse),
            (kCGImageSourceShouldCacheImmediately, kCFBooleanTrue),
        ])?
    };
    // SAFETY: live source and options
    let image =
        Cf::new(unsafe { CGImageSourceCreateThumbnailAtIndex(source.0, 0, thumbnail_options.0) })?;
    // SAFETY: live image
    let (w, h) = unsafe { (CGImageGetWidth(image.0), CGImageGetHeight(image.0)) };
    let (w32, h32) = (u32::try_from(w).ok()?, u32::try_from(h).ok()?);
    if !size_ok(w32, h32) {
        return None;
    }
    let mut pixels = vec![0u8; w.checked_mul(h)?.checked_mul(4)?];
    {
        // SAFETY: framework constant
        let space = Cf::new(unsafe { CGColorSpaceCreateWithName(kCGColorSpaceSRGB) })?;
        // SAFETY: `pixels` is w×h×4 bytes and outlives the context, which is
        // released at the end of this block
        let context = Cf::new(unsafe {
            CGBitmapContextCreate(
                pixels.as_mut_ptr().cast(),
                w,
                h,
                8,
                w * 4,
                space.0,
                kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big,
            )
        })?;
        let rect = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: w as f64,
                height: h as f64,
            },
        };
        // SAFETY: live context and image
        unsafe { CGContextDrawImage(context.0, rect, image.0) };
    }
    unpremultiply(&mut pixels);
    let (width, height) = natural.unwrap_or((w32, h32));
    Some(Decoded {
        rgba: RgbaImage::from_raw(w32, h32, pixels)?,
        width,
        height,
    })
}

fn unpremultiply(pixels: &mut [u8]) {
    for p in pixels.as_chunks_mut::<4>().0 {
        let a = u32::from(p[3]);
        if a == 0 {
            p[..3].fill(0);
        } else if a < 255 {
            for c in &mut p[..3] {
                *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const kCFNumberSInt32Type: isize = 3;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFDataCreateMutable(allocator: CFTypeRef, capacity: isize) -> CFTypeRef;
        fn CFDataGetBytePtr(data: CFTypeRef) -> *const u8;
        fn CFDataGetLength(data: CFTypeRef) -> isize;
    }

    #[link(name = "ImageIO", kind = "framework")]
    unsafe extern "C" {
        fn CGImageDestinationCreateWithData(
            data: CFTypeRef,
            kind: CFTypeRef,
            count: usize,
            options: CFTypeRef,
        ) -> CFTypeRef;
        fn CGImageDestinationAddImage(dest: CFTypeRef, image: CFTypeRef, properties: CFTypeRef);
        fn CGImageDestinationFinalize(dest: CFTypeRef) -> bool;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGBitmapContextCreateImage(context: CFTypeRef) -> CFTypeRef;
    }

    /// An 80×40 image (left half red, right half blue) written by ImageIO as
    /// `uti` with EXIF orientation 6 (rotate 90° clockwise to display).
    fn write_rotated(uti: &str) -> Option<Vec<u8>> {
        let (w, h) = (80usize, 40usize);
        let mut pixels: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                if i % w < w / 2 {
                    [255, 0, 0, 255]
                } else {
                    [0, 0, 255, 255]
                }
            })
            .collect();
        unsafe {
            let space = Cf::new(CGColorSpaceCreateWithName(kCGColorSpaceSRGB))?;
            let context = Cf::new(CGBitmapContextCreate(
                pixels.as_mut_ptr().cast(),
                w,
                h,
                8,
                w * 4,
                space.0,
                kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big,
            ))?;
            let image = Cf::new(CGBitmapContextCreateImage(context.0))?;
            let out = Cf::new(CFDataCreateMutable(std::ptr::null(), 0))?;
            let kind = cf_string(uti)?;
            let dest = Cf::new(CGImageDestinationCreateWithData(
                out.0,
                kind.0,
                1,
                std::ptr::null(),
            ))?;
            let six: i32 = 6;
            let orientation = Cf::new(CFNumberCreate(
                std::ptr::null(),
                kCFNumberSInt32Type,
                (&raw const six).cast(),
            ))?;
            let properties = cf_dictionary(&[(kCGImagePropertyOrientation, orientation.0)])?;
            CGImageDestinationAddImage(dest.0, image.0, properties.0);
            if !CGImageDestinationFinalize(dest.0) {
                return None;
            }
            let length = usize::try_from(CFDataGetLength(out.0)).ok()?;
            Some(std::slice::from_raw_parts(CFDataGetBytePtr(out.0), length).to_vec())
        }
    }

    #[test]
    fn imageio_decodes_heic_or_jpeg_2000_with_orientation() {
        let (bytes, media_type) = write_rotated("public.heic")
            .map(|b| (b, "image/heic"))
            .or_else(|| write_rotated("public.jpeg-2000").map(|b| (b, "image/jp2")))
            .expect("ImageIO writes HEIC or JPEG 2000");
        let decoded = super::super::decode(&bytes, media_type, 1000).unwrap();
        assert_eq!((decoded.width, decoded.height), (40, 80));
        let png = image::load_from_memory(&decoded.png).unwrap().into_rgba8();
        assert_eq!(png.dimensions(), (40, 80));
        // rotated clockwise: the red left half is on top now
        let top = png.get_pixel(20, 10).0;
        let bottom = png.get_pixel(20, 70).0;
        assert!(top[0] > 200 && top[2] < 60, "{top:?}");
        assert!(bottom[2] > 200 && bottom[0] < 60, "{bottom:?}");

        let small = super::super::decode(&bytes, media_type, 20).unwrap();
        assert_eq!((small.width, small.height), (40, 80));
        let png = image::load_from_memory(&small.png).unwrap();
        assert_eq!((png.width(), png.height()), (10, 20));
    }

    #[test]
    fn unpremultiply_restores_straight_alpha() {
        let mut p = [64, 32, 0, 128, 10, 10, 10, 0, 1, 2, 3, 255];
        unpremultiply(&mut p);
        assert_eq!(p, [128, 64, 0, 128, 0, 0, 0, 0, 1, 2, 3, 255]);
    }
}
