//! Corvene `1326-more-image-formats`: hands `corvene_git` the platform's
//! image decoder (`corvene_platform::image_decode`), so HEIC, PSD, TIFF,
//! camera RAW and similar files become image diffs. GHD
//! (`app/src/lib/git/diff.ts` `imageFileExtensions`) shows them as binary.

/// Longest side of the PNG a decoded image is shown from.
const MAX_SIDE: u32 = 4096;

fn decode(bytes: &[u8], media_type: &str) -> Option<corvene_git::DecodedImage> {
    let decoded = corvene_platform::image_decode::decode(bytes, media_type, MAX_SIDE)?;
    Some(corvene_git::DecodedImage {
        png: decoded.png,
        width: decoded.width,
        height: decoded.height,
    })
}

/// Turn the decoder on or off with the flag.
pub fn set_more_image_formats(enabled: bool) {
    corvene_git::set_image_decoder(enabled.then_some(decode as corvene_git::ImageDecoder));
}
