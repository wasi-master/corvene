//! Media Foundation (Windows): probing and reading through an
//! `IMFSourceReader`, which converts pictures to RGB32 (BGRX in memory) and
//! sound to `f32`. It opens what Windows' own player opens (MPEG-4, AVI,
//! WMV, MP3, AAC, FLAC; WebM and AV1 with the store extensions).
//!
//! Never run on Windows by the author; type-checked for the
//! `x86_64-pc-windows-msvc` target only.

use std::path::Path;
use std::sync::OnceLock;

use corvene_models::{AudioStreamInfo, MediaDecoder, MediaInfo, VideoStreamInfo};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::System::Variant::VT_I8;
use windows_core::{GUID, HSTRING};

use crate::frame::{AudioChunk, Frame, resize_bgra, rotate_bgra};
use crate::{AudioReader, MediaError, Result, VideoReader, decode_error};

const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
const AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
const SOURCE: u32 = MF_SOURCE_READER_MEDIASOURCE.0 as u32;
/// Media Foundation counts time in 100 ns units.
const UNITS: f64 = 10_000_000.;

fn start() -> Result<()> {
    static STARTED: OnceLock<bool> = OnceLock::new();
    // COM on this thread (already set up is fine)
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok().ok();
    }
    let ok = *STARTED.get_or_init(|| unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL).is_ok() });
    if ok {
        Ok(())
    } else {
        Err(MediaError::NoDecoder)
    }
}

fn open(path: &Path, video_processing: bool) -> Result<IMFSourceReader> {
    start()?;
    unsafe {
        let mut attributes = None;
        MFCreateAttributes(&mut attributes, 2).map_err(decode_error)?;
        let attributes = attributes.ok_or(MediaError::NoDecoder)?;
        if video_processing {
            attributes
                .SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)
                .map_err(decode_error)?;
        }
        let url = HSTRING::from(path.as_os_str());
        MFCreateSourceReaderFromURL(&url, &attributes).map_err(decode_error)
    }
}

fn four_cc(guid: &GUID) -> String {
    let named = match guid.data1 {
        0x0001 => "PCM",
        0x0003 => "PCM (float)",
        0x0055 => "MP3",
        0x0161 => "Windows Media Audio",
        0x0162 => "Windows Media Audio Pro",
        0x0163 => "Windows Media Audio Lossless",
        0x1610 => "AAC",
        0x2000 => "Dolby Digital (AC-3)",
        0xF1AC => "FLAC",
        0x704F => "Opus",
        0x6C61 => "Apple Lossless",
        _ => "",
    };
    if !named.is_empty() {
        return named.to_string();
    }
    let bytes = guid.data1.to_le_bytes();
    let text: String = bytes
        .iter()
        .map(|b| {
            if b.is_ascii_graphic() {
                *b as char
            } else {
                '?'
            }
        })
        .collect();
    match text.as_str() {
        "H264" => "H.264".into(),
        "HEVC" | "H265" => "HEVC (H.265)".into(),
        "AV01" => "AV1".into(),
        "VP90" => "VP9".into(),
        "VP80" => "VP8".into(),
        "WMV3" => "Windows Media Video 9".into(),
        "MP4V" => "MPEG-4 Part 2".into(),
        "MJPG" => "Motion JPEG".into(),
        _ => text,
    }
}

fn split(v: u64) -> (u32, u32) {
    ((v >> 32) as u32, v as u32)
}

pub(crate) fn probe(path: &Path) -> Option<MediaInfo> {
    let reader = open(path, false).ok()?;
    let mut info = MediaInfo {
        container: crate::container_name(&crate::extension(path)).to_string(),
        ..Default::default()
    };
    unsafe {
        if let Ok(pv) = reader.GetPresentationAttribute(SOURCE, &MF_PD_DURATION) {
            let units = pv.Anonymous.Anonymous.Anonymous.uhVal;
            info.duration_us = Some(units / 10);
        }
        if let Ok(t) = reader.GetNativeMediaType(VIDEO, 0) {
            let (w, h) = t.GetUINT64(&MF_MT_FRAME_SIZE).map(split).unwrap_or((0, 0));
            let rate = t.GetUINT64(&MF_MT_FRAME_RATE).map(split).ok();
            let rotation = t.GetUINT32(&MF_MT_VIDEO_ROTATION).unwrap_or(0);
            info.video = Some(VideoStreamInfo {
                codec: t
                    .GetGUID(&MF_MT_SUBTYPE)
                    .map(|g| four_cc(&g))
                    .unwrap_or_default(),
                width: w,
                height: h,
                frame_rate: rate.filter(|(n, d)| *n > 0 && *d > 0),
                frames: None,
                // Media Foundation's rotation is counter-clockwise
                rotation: (360 - rotation % 360) % 360,
                pixel_format: None,
                bit_rate: t.GetUINT32(&MF_MT_AVG_BITRATE).ok().map(u64::from),
            });
            info.video_decoder = MediaDecoder::System;
        }
        if let Ok(t) = reader.GetNativeMediaType(AUDIO, 0) {
            info.audio = Some(AudioStreamInfo {
                codec: t
                    .GetGUID(&MF_MT_SUBTYPE)
                    .map(|g| four_cc(&g))
                    .unwrap_or_default(),
                sample_rate: t.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).unwrap_or(0),
                channels: t.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(0) as u16,
                bits_per_sample: t
                    .GetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE)
                    .ok()
                    .filter(|b| *b > 0),
                bit_rate: t
                    .GetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND)
                    .ok()
                    .map(|b| b as u64 * 8),
            });
            info.audio_decoder = MediaDecoder::System;
        }
    }
    if info.video.is_none() && info.audio.is_none() {
        return None;
    }
    if let (Ok(meta), Some(us)) = (std::fs::metadata(path), info.duration_us)
        && us > 0
    {
        info.bit_rate = Some((meta.len() as f64 * 8. / (us as f64 / 1e6)).round() as u64);
    }
    Some(info)
}

fn seek(reader: &IMFSourceReader, time: f64) -> Result<()> {
    if time <= 0. {
        return Ok(());
    }
    let mut pv = PROPVARIANT::default();
    unsafe {
        let inner = &mut pv.Anonymous.Anonymous;
        inner.vt = VT_I8;
        inner.Anonymous.hVal = (time * UNITS) as i64;
        reader
            .SetCurrentPosition(&GUID::zeroed(), &pv)
            .map_err(decode_error)
    }
}

/// Selects only `stream` and asks for `subtype` from it.
fn select(
    reader: &IMFSourceReader,
    stream: u32,
    major: &GUID,
    subtype: &GUID,
) -> Result<IMFMediaType> {
    unsafe {
        reader
            .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
            .map_err(decode_error)?;
        reader
            .SetStreamSelection(stream, true)
            .map_err(decode_error)?;
        let t = MFCreateMediaType().map_err(decode_error)?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, major).map_err(decode_error)?;
        t.SetGUID(&MF_MT_SUBTYPE, subtype).map_err(decode_error)?;
        reader
            .SetCurrentMediaType(stream, None, &t)
            .map_err(decode_error)?;
        reader.GetCurrentMediaType(stream).map_err(decode_error)
    }
}

/// The next sample's time and bytes, `None` at the end.
fn read(reader: &IMFSourceReader, stream: u32) -> Result<Option<(f64, Vec<u8>)>> {
    loop {
        let mut flags = 0u32;
        let mut time = 0i64;
        let mut sample: Option<IMFSample> = None;
        unsafe {
            reader
                .ReadSample(
                    stream,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut time),
                    Some(&mut sample),
                )
                .map_err(decode_error)?;
        }
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            return Ok(None);
        }
        if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
            return Err(decode_error("Media Foundation could not read the file."));
        }
        let Some(sample) = sample else { continue };
        unsafe {
            let buffer = sample.ConvertToContiguousBuffer().map_err(decode_error)?;
            let mut data = std::ptr::null_mut();
            let mut len = 0u32;
            buffer
                .Lock(&mut data, None, Some(&mut len))
                .map_err(decode_error)?;
            let bytes = std::slice::from_raw_parts(data, len as usize).to_vec();
            buffer.Unlock().ok();
            return Ok(Some((time as f64 / UNITS, bytes)));
        }
    }
}

struct MfVideo {
    reader: IMFSourceReader,
    natural: (u32, u32),
    stride: i32,
    size: (u32, u32),
    rotation: u32,
    start: f64,
    held: Option<Frame>,
    queued: Option<Frame>,
}

// SAFETY: the source reader is free-threaded; one thread uses it at a time.
unsafe impl Send for MfVideo {}

impl MfVideo {
    fn decode(&self, time: f64, bytes: &[u8]) -> Frame {
        let (w, h) = self.natural;
        let row = w as usize * 4;
        let stride = self.stride.unsigned_abs() as usize;
        // a bottom-up picture (negative stride) is turned the right way up
        let mut packed = vec![0u8; row * h as usize];
        for y in 0..h as usize {
            let src_y = if self.stride < 0 {
                h as usize - 1 - y
            } else {
                y
            };
            let src = &bytes[(src_y * stride).min(bytes.len())..];
            let n = row.min(src.len());
            packed[y * row..y * row + n].copy_from_slice(&src[..n]);
        }
        let mut pixels = resize_bgra(&packed, w, h, row, self.size.0, self.size.1);
        for p in pixels.as_chunks_mut::<4>().0.iter_mut() {
            p[3] = 255;
        }
        let (w, h, pixels) = rotate_bgra(pixels, self.size.0, self.size.1, self.rotation);
        Frame::new(time, w, h, pixels)
    }
}

impl VideoReader for MfVideo {
    fn next_frame(&mut self) -> Result<Option<Frame>> {
        if let Some(f) = self.queued.take() {
            return Ok(Some(f));
        }
        loop {
            let Some((time, bytes)) = read(&self.reader, VIDEO)? else {
                return Ok(self.held.take().map(|mut f| {
                    f.time = self.start;
                    f
                }));
            };
            let frame = self.decode(time, &bytes);
            if frame.time < self.start - 0.0005 {
                self.held = Some(frame);
                continue;
            }
            if let Some(mut held) = self.held.take()
                && frame.time > self.start + 0.0005
            {
                held.time = self.start;
                self.queued = Some(frame);
                return Ok(Some(held));
            }
            return Ok(Some(frame));
        }
    }
}

pub(crate) fn video_from(
    path: &Path,
    info: &MediaInfo,
    time: f64,
    size: (u32, u32),
) -> Result<Box<dyn VideoReader>> {
    let reader = open(path, true)?;
    let t = select(&reader, VIDEO, &MFMediaType_Video, &MFVideoFormat_RGB32)?;
    let natural = unsafe { t.GetUINT64(&MF_MT_FRAME_SIZE) }
        .map(split)
        .map_err(decode_error)?;
    let stride = unsafe { t.GetUINT32(&MF_MT_DEFAULT_STRIDE) }
        .map(|s| s as i32)
        .unwrap_or(natural.0 as i32 * 4);
    seek(&reader, time)?;
    let rotation = info.video.as_ref().map_or(0, |v| v.rotation);
    let stored = if rotation % 180 == 90 {
        (size.1, size.0)
    } else {
        size
    };
    Ok(Box::new(MfVideo {
        reader,
        natural,
        stride,
        size: stored,
        rotation,
        start: time,
        held: None,
        queued: None,
    }))
}

pub(crate) fn frame_at(
    path: &Path,
    info: &MediaInfo,
    time: f64,
    size: (u32, u32),
) -> Result<Frame> {
    video_from(path, info, time, size)?
        .next_frame()?
        .ok_or_else(|| decode_error("No picture at this time."))
}

struct MfAudio {
    reader: IMFSourceReader,
    rate: u32,
    channels: u16,
    start: f64,
}

// SAFETY: as `MfVideo`.
unsafe impl Send for MfAudio {}

impl AudioReader for MfAudio {
    fn next_chunk(&mut self) -> Result<Option<AudioChunk>> {
        loop {
            let Some((mut time, bytes)) = read(&self.reader, AUDIO)? else {
                return Ok(None);
            };
            let mut samples: Vec<f32> = bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect();
            let channels = self.channels.max(1) as usize;
            if time < self.start {
                let skip = (((self.start - time) * self.rate as f64).round() as usize)
                    .min(samples.len() / channels);
                samples.drain(..skip * channels);
                time = self.start;
            }
            if samples.is_empty() {
                continue;
            }
            return Ok(Some(AudioChunk {
                time,
                rate: self.rate,
                channels: self.channels,
                samples,
            }));
        }
    }
}

pub(crate) fn audio_from(path: &Path, time: f64) -> Result<Box<dyn AudioReader>> {
    let reader = open(path, false)?;
    let t = select(&reader, AUDIO, &MFMediaType_Audio, &MFAudioFormat_Float)?;
    let rate = unsafe { t.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND) }.unwrap_or(48_000);
    let channels = unsafe { t.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS) }.unwrap_or(2) as u16;
    seek(&reader, time)?;
    Ok(Box::new(MfAudio {
        reader,
        rate,
        channels,
        start: time,
    }))
}
