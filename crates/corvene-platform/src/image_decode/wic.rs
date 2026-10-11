//! Windows Imaging Component: the installed codecs (the Store's HEIF, AV1,
//! JPEG XL and Raw Image extensions included) decode the first frame,
//! scaled by `IWICBitmapScaler` and converted to straight RGBA8. The EXIF
//! orientation (`System.Photo.Orientation`, else the IFD tag 274) is handed
//! back for the caller to apply. Uses nothing from the crate, so it can be
//! type-checked for Windows on its own.

use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppRGBA, IWICBitmapFrameDecode, IWICBitmapSource,
    IWICImagingFactory, WICBitmapInterpolationModeHighQualityCubic, WICConvertBitmapSource,
    WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PropVariantClear, PropVariantToUInt16,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::core::{Interface, w};

/// Straight RGBA8 pixels `width`×`height` (at most `max_side`), the
/// stored (unrotated) natural size and the EXIF orientation (1..=8).
pub struct WicImage {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub natural_width: u32,
    pub natural_height: u32,
    pub orientation: u8,
}

/// Decode with WIC; `size_ok` vets the natural size before any pixels are
/// read.
pub fn decode(bytes: &[u8], max_side: u32, size_ok: fn(u32, u32) -> bool) -> Option<WicImage> {
    // SAFETY: an MTA join for this thread, undone below when it succeeded
    // (S_OK or S_FALSE); RPC_E_CHANGED_MODE keeps the thread's STA, which
    // WIC works in too
    let init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    // every COM object is released inside, before CoUninitialize
    let image = decode_frame(bytes, max_side.max(1), size_ok);
    if init.is_ok() {
        // SAFETY: balances the successful CoInitializeEx above
        unsafe { CoUninitialize() };
    }
    image
}

fn decode_frame(bytes: &[u8], max_side: u32, size_ok: fn(u32, u32) -> bool) -> Option<WicImage> {
    // SAFETY: WIC calls on objects created here; the stream reads `bytes`,
    // which outlives every object (all dropped before returning), and
    // CopyPixels writes into a buffer sized stride × height
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let stream = factory.CreateStream().ok()?;
        stream.InitializeFromMemory(bytes).ok()?;
        let decoder = factory
            .CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)
            .ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        let (mut natural_width, mut natural_height) = (0, 0);
        frame
            .GetSize(&mut natural_width, &mut natural_height)
            .ok()?;
        if !size_ok(natural_width, natural_height) {
            return None;
        }
        let orientation = orientation(&frame);
        let (width, height) = fit(natural_width, natural_height, max_side);
        let source: IWICBitmapSource = if (width, height) == (natural_width, natural_height) {
            frame.cast().ok()?
        } else {
            let scaler = factory.CreateBitmapScaler().ok()?;
            scaler
                .Initialize(
                    &frame,
                    width,
                    height,
                    WICBitmapInterpolationModeHighQualityCubic,
                )
                .ok()?;
            scaler.cast().ok()?
        };
        let rgba = WICConvertBitmapSource(&GUID_WICPixelFormat32bppRGBA, &source).ok()?;
        let stride = width.checked_mul(4)?;
        let length = usize::try_from(stride)
            .ok()?
            .checked_mul(usize::try_from(height).ok()?)?;
        let mut pixels = vec![0u8; length];
        rgba.CopyPixels(std::ptr::null(), stride, &mut pixels)
            .ok()?;
        Some(WicImage {
            rgba: pixels,
            width,
            height,
            natural_width,
            natural_height,
            orientation,
        })
    }
}

/// The frame's EXIF orientation, 1 when it has none.
fn orientation(frame: &IWICBitmapFrameDecode) -> u8 {
    // SAFETY: metadata reads into a PROPVARIANT that is cleared after use
    unsafe {
        let Ok(reader) = frame.GetMetadataQueryReader() else {
            return 1;
        };
        for name in [
            w!("System.Photo.Orientation"),
            w!("/app1/ifd/{ushort=274}"),
            w!("/ifd/{ushort=274}"),
        ] {
            let mut value = PROPVARIANT::default();
            if reader.GetMetadataByName(name, &mut value).is_err() {
                continue;
            }
            let read = PropVariantToUInt16(&value);
            let _ = PropVariantClear(&mut value);
            if let Ok(n @ 1..=8) = read {
                return n as u8;
            }
        }
        1
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn garbage_is_none() {
        assert!(decode(b"not an image", 100, |_, _| true).is_none());
    }

    #[test]
    fn fit_keeps_the_aspect() {
        assert_eq!(fit(100, 50, 20), (20, 10));
        assert_eq!(fit(10, 5, 20), (10, 5));
    }
}
