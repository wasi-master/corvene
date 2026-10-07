//! Images in Markdown (issue and pull request bodies, release notes, the
//! comments): downloaded once, without credentials, into
//! `~/Library/Caches/Corvene/markdown-images` and drawn from there by
//! `corvene_ui::markdown`, which asks for them as they first show
//! ([`Dispatcher::request_markdown_image`]). The pixel size is read here so
//! the image can be laid out at its natural size before it is decoded.
//!
//! GitHub serves `user-attachments` and `camo` image URLs to anyone for a
//! public repository; a private repository's attachments need a signed-in
//! browser session, which Corvene does not have, so those show their alt
//! text (GHD's sandboxed `marked` renderer cannot load them either).

use std::collections::HashMap;
use std::path::PathBuf;

use tracing::debug;

use crate::dispatcher::Dispatcher;
use crate::host::Host;
use crate::remote::spawn_bg;

/// One image by URL.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MarkdownImage {
    pub loading: bool,
    /// The cached file once downloaded.
    pub path: Option<PathBuf>,
    /// Its size in pixels, when the format is known.
    pub size: Option<(u32, u32)>,
    pub failed: bool,
}

pub type MarkdownImages = HashMap<String, MarkdownImage>;

/// The largest image read (GitHub caps attachments at 10 MB, GIFs too).
const MAX_BYTES: usize = 25 * 1024 * 1024;

fn cache_file(url: &str) -> PathBuf {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in url.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    corvene_platform::paths::cache_dir()
        .join("markdown-images")
        .join(format!("{hash:016x}"))
}

/// The pixel size of an image file; SVGs from their `width` / `height` or
/// `viewBox`.
fn image_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if let Ok(size) = imagesize::blob_size(bytes) {
        return Some((size.width as u32, size.height as u32)).filter(|(w, h)| *w > 0 && *h > 0);
    }
    svg_size(std::str::from_utf8(bytes.get(..bytes.len().min(4096))?).ok()?)
}

fn svg_size(text: &str) -> Option<(u32, u32)> {
    let start = text.find("<svg")?;
    let tag = &text[start..start + text[start..].find('>')?];
    let attr = |key: &str| -> Option<f32> {
        let at = tag.find(&format!(" {key}=\""))? + key.len() + 3;
        let value = &tag[at..at + tag[at..].find('"')?];
        value.trim_end_matches("px").trim().parse().ok()
    };
    let size = match (attr("width"), attr("height")) {
        (Some(w), Some(h)) => (w, h),
        _ => {
            let at = tag.find(" viewBox=\"")? + 10;
            let value = &tag[at..at + tag[at..].find('"')?];
            let parts: Vec<f32> = value
                .split([' ', ','])
                .filter_map(|p| p.trim().parse().ok())
                .collect();
            (*parts.get(2)?, *parts.get(3)?)
        }
    };
    (size.0 >= 1. && size.1 >= 1.).then(|| (size.0.round() as u32, size.1.round() as u32))
}

/// Download (or find cached) `url`.
fn fetch(url: &str) -> Option<(PathBuf, Option<(u32, u32)>)> {
    let file = cache_file(url);
    if let Ok(bytes) = std::fs::read(&file) {
        return Some((file, image_size(&bytes)));
    }
    match corvene_github::download(url) {
        Ok(bytes) if !bytes.is_empty() && bytes.len() <= MAX_BYTES => {
            if let Some(dir) = file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let size = image_size(&bytes);
            std::fs::write(&file, &bytes).ok()?;
            Some((file, size))
        }
        Ok(_) => None,
        Err(err) => {
            debug!(%url, %err, "markdown image download failed");
            None
        }
    }
}

impl Dispatcher {
    /// Download a Markdown image (an absolute `http(s)` URL) unless it is
    /// cached or on its way; observers redraw once it is there.
    pub fn request_markdown_image(url: &str, cx: &mut dyn Host) {
        let lower = url.to_ascii_lowercase();
        if !lower.starts_with("http://") && !lower.starts_with("https://") {
            return;
        }
        let url = url.to_string();
        let started = Self::state(cx).update(cx, |s, cx| {
            if s.markdown_images.contains_key(&url) {
                return false;
            }
            s.markdown_images.insert(
                url.clone(),
                MarkdownImage {
                    loading: true,
                    ..MarkdownImage::default()
                },
            );
            cx.notify();
            true
        });
        if !started {
            return;
        }
        let fetch_url = url.clone();
        spawn_bg(
            cx,
            move || fetch(&fetch_url),
            move |result, cx| {
                Self::state(cx).update(cx, |s, cx| {
                    let entry = match result {
                        Some((path, size)) => MarkdownImage {
                            loading: false,
                            path: Some(path),
                            size,
                            failed: false,
                        },
                        None => MarkdownImage {
                            loading: false,
                            failed: true,
                            ..MarkdownImage::default()
                        },
                    };
                    s.markdown_images.insert(url, entry);
                    cx.notify();
                });
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_of_pngs_and_svgs() {
        // the PNG signature and IHDR of a 3 × 2 image
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        png.extend([0, 0, 0, 13]);
        png.extend(b"IHDR");
        png.extend(3u32.to_be_bytes());
        png.extend(2u32.to_be_bytes());
        png.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(image_size(&png), Some((3, 2)));
        assert_eq!(
            image_size(br#"<svg xmlns="http://www.w3.org/2000/svg" width="90" height="20">"#),
            Some((90, 20))
        );
        assert_eq!(
            image_size(br#"<?xml?><svg viewBox="0 0 120 40.4" xmlns="x">"#),
            Some((120, 40))
        );
        assert_eq!(image_size(b"nope"), None);
        assert_ne!(cache_file("a"), cache_file("b"));
    }
}
