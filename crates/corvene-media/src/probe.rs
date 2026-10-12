//! What a file holds, from the first decoder that opens it.

use std::path::Path;

use corvene_models::{MediaInfo, MediaKind};

/// `None` when nothing on this system reads the file.
pub fn probe(path: &Path, kind: MediaKind) -> Option<MediaInfo> {
    let _span = tracing::debug_span!("media probe", path = %path.display()).entered();
    match kind {
        MediaKind::Audio => {
            if let Some(mut info) = crate::symph::probe(path) {
                if info.duration_us.is_none() {
                    // an MP3 without a frame count: ask the others how long it is
                    info.duration_us = system(path, kind)
                        .or_else(|| crate::ffmpeg::probe(path))
                        .and_then(|i| i.duration_us);
                }
                return Some(info);
            }
            system(path, kind)
                .filter(|i| i.audio.is_some())
                .or_else(|| crate::ffmpeg::probe(path))
        }
        MediaKind::Video => {
            let system = system(path, kind).filter(|i| i.video.is_some());
            match system {
                Some(info) => Some(info),
                None => crate::ffmpeg::probe(path),
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn system(path: &Path, _kind: MediaKind) -> Option<MediaInfo> {
    crate::avf::probe(path)
}

#[cfg(windows)]
fn system(path: &Path, _kind: MediaKind) -> Option<MediaInfo> {
    crate::mf::probe(path)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn system(_path: &Path, _kind: MediaKind) -> Option<MediaInfo> {
    None
}
