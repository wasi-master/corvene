//! Video and sound files read for diffs (`1335-video-diff`,
//! `1336-audio-diff`): what a file holds ([`probe`]), its pictures and its
//! sound ([`Source`]), what changed between two of them ([`analysis`]) and
//! playing both in step ([`player`]).
//!
//! Decoders, in the order they are tried:
//! - sound: Symphonia (pure Rust), then the system (AVFoundation on macOS,
//!   Media Foundation on Windows), then `ffmpeg`;
//! - pictures: the system, then `ffmpeg`.
//!
//! `ffmpeg` / `ffprobe` are only run when `1337-media-ffmpeg` is on
//! ([`set_ffmpeg_enabled`]) and they are installed; they are the only
//! programs this crate starts (`ffmpeg.rs`).

pub mod analysis;
mod ffmpeg;
pub mod frame;
pub mod player;
mod probe;
mod symph;

#[cfg(target_os = "macos")]
mod avf;
#[cfg(windows)]
mod mf;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub use corvene_models::{
    AudioStreamInfo, MediaDecoder, MediaInfo, MediaKind, MediaSide, VideoStreamInfo,
};
pub use frame::{AudioChunk, Frame};
pub use probe::probe;

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("{0}")]
    Decode(String),
    #[error("Nothing on this system can decode it.")]
    NoDecoder,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, MediaError>;

pub(crate) fn decode_error(e: impl std::fmt::Display) -> MediaError {
    MediaError::Decode(e.to_string())
}

static FFMPEG: AtomicBool = AtomicBool::new(true);

/// `1337-media-ffmpeg`: whether an installed `ffmpeg` may be run.
pub fn set_ffmpeg_enabled(on: bool) {
    FFMPEG.store(on, Ordering::Relaxed);
}

pub(crate) fn ffmpeg_enabled() -> bool {
    FFMPEG.load(Ordering::Relaxed)
}

/// Whether an `ffmpeg` that may be run is installed.
pub fn ffmpeg_available() -> bool {
    ffmpeg_enabled() && ffmpeg::tools().is_some()
}

/// Pictures read one after another from some time on.
pub trait VideoReader: Send {
    fn next_frame(&mut self) -> Result<Option<Frame>>;
}

/// Sound read one chunk after another from some time on.
pub trait AudioReader: Send {
    fn next_chunk(&mut self) -> Result<Option<AudioChunk>>;
}

/// A probed file whose pictures and sound can be read.
#[derive(Clone, Debug)]
pub struct Source {
    pub path: PathBuf,
    pub info: MediaInfo,
}

impl Source {
    pub fn new(path: impl Into<PathBuf>, info: MediaInfo) -> Self {
        Self {
            path: path.into(),
            info,
        }
    }

    pub fn of(side: &MediaSide) -> Self {
        Self::new(side.file.clone(), side.info.clone())
    }

    pub fn duration(&self) -> f64 {
        self.info.duration_us.map_or(0., |us| us as f64 / 1e6)
    }

    pub fn has_video(&self) -> bool {
        self.info.video.is_some() && self.info.video_decoder != MediaDecoder::None
    }

    pub fn has_audio(&self) -> bool {
        self.info.audio.is_some() && self.info.audio_decoder != MediaDecoder::None
    }

    /// The picture's size as shown, scaled to fit `max`.
    pub fn frame_size(&self, max: (u32, u32)) -> (u32, u32) {
        let natural = self
            .info
            .video
            .as_ref()
            .map_or((16, 9), |v| v.display_size());
        frame::fit_size(natural, max)
    }

    /// The picture showing at `time`, fitted into `max`.
    pub fn frame_at(&self, time: f64, max: (u32, u32)) -> Result<Frame> {
        let size = self.frame_size(max);
        match self.info.video_decoder {
            #[cfg(target_os = "macos")]
            MediaDecoder::System => avf::frame_at(&self.path, &self.info, time, size),
            #[cfg(windows)]
            MediaDecoder::System => mf::frame_at(&self.path, &self.info, time, size),
            MediaDecoder::Ffmpeg => ffmpeg::frame_at(&self.path, &self.info, time, size),
            _ => Err(MediaError::NoDecoder),
        }
    }

    /// The pictures from `time` on, fitted into `max`.
    pub fn video_from(&self, time: f64, max: (u32, u32)) -> Result<Box<dyn VideoReader>> {
        let size = self.frame_size(max);
        match self.info.video_decoder {
            #[cfg(target_os = "macos")]
            MediaDecoder::System => avf::video_from(&self.path, &self.info, time, size),
            #[cfg(windows)]
            MediaDecoder::System => mf::video_from(&self.path, &self.info, time, size),
            MediaDecoder::Ffmpeg => ffmpeg::video_from(&self.path, &self.info, time, size),
            _ => Err(MediaError::NoDecoder),
        }
    }

    /// The sound from `time` on.
    pub fn audio_from(&self, time: f64) -> Result<Box<dyn AudioReader>> {
        match self.info.audio_decoder {
            MediaDecoder::Symphonia => symph::audio_from(&self.path, time),
            #[cfg(target_os = "macos")]
            MediaDecoder::System => avf::audio_from(&self.path, time),
            #[cfg(windows)]
            MediaDecoder::System => mf::audio_from(&self.path, time),
            MediaDecoder::Ffmpeg => ffmpeg::audio_from(&self.path, &self.info, time),
            _ => Err(MediaError::NoDecoder),
        }
    }
}

/// The extension of `path`, lower case.
pub(crate) fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// A name for the container of a file with extension `ext`.
pub(crate) fn container_name(ext: &str) -> &'static str {
    match ext {
        "mp4" | "m4v" | "m4a" | "m4b" => "MPEG-4",
        "mov" | "qt" => "QuickTime",
        "webm" => "WebM",
        "mkv" | "mka" => "Matroska",
        "avi" => "AVI",
        "ogv" | "ogg" | "oga" | "opus" => "Ogg",
        "3gp" | "3g2" => "3GPP",
        "mpg" | "mpeg" => "MPEG program stream",
        "m2ts" | "mts" => "MPEG transport stream",
        "wmv" | "wma" => "Windows Media",
        "flv" => "Flash Video",
        "mp3" => "MP3",
        "wav" | "wave" => "WAVE",
        "flac" => "FLAC",
        "aac" => "ADTS",
        "aif" | "aiff" | "aifc" => "AIFF",
        "caf" => "Core Audio",
        _ => "Unknown",
    }
}
