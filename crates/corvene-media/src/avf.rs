//! AVFoundation (macOS): probing, single pictures (`AVAssetImageGenerator`)
//! and pictures and sound in order (`AVAssetReader`). It opens what
//! QuickTime Player opens: MPEG-4 / QuickTime with H.264, HEVC, ProRes,
//! AV1 (on Apple silicon M3 and later), AAC, ALAC, MP3, PCM; not WebM,
//! Matroska or AVI.
//!
//! Core Video, Core Graphics and Core Media C calls are declared here
//! (`extern "C"`), like `corvene_platform`'s ImageIO decoder; the
//! Objective-C classes come from `objc2-av-foundation`.

use std::ffi::c_void;
use std::path::Path;
use std::sync::Mutex;

use corvene_models::{AudioStreamInfo, ImageBlob, MediaDecoder, MediaInfo, VideoStreamInfo};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::AnyObject;
use objc2_av_foundation::{
    AVAsset, AVAssetImageGenerator, AVAssetReader, AVAssetReaderAudioMixOutput,
    AVAssetReaderOutput, AVAssetReaderStatus, AVAssetReaderTrackOutput, AVAssetTrack,
    AVMediaTypeAudio, AVMediaTypeVideo, AVURLAsset,
};
use objc2_core_media::{CMSampleBuffer, CMTime, CMTimeRange, kCMTimePositiveInfinity};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, NSURL};

use crate::frame::{AudioChunk, Frame, resize_bgra, rotate_bgra};
use crate::{AudioReader, MediaError, Result, VideoReader, decode_error};

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Asbd {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CgRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    fn CMSampleBufferGetImageBuffer(sbuf: *const c_void) -> *const c_void;
    fn CMSampleBufferGetDataBuffer(sbuf: *const c_void) -> *const c_void;
    fn CMSampleBufferGetPresentationTimeStamp(sbuf: *const c_void) -> CMTime;
    fn CMBlockBufferGetDataLength(buf: *const c_void) -> usize;
    fn CMBlockBufferCopyDataBytes(
        buf: *const c_void,
        offset: usize,
        len: usize,
        dest: *mut c_void,
    ) -> i32;
    fn CMFormatDescriptionGetMediaSubType(desc: *const c_void) -> u32;
    fn CMAudioFormatDescriptionGetStreamBasicDescription(desc: *const c_void) -> *const Asbd;
}

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    fn CVPixelBufferLockBaseAddress(buf: *const c_void, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(buf: *const c_void, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddress(buf: *const c_void) -> *const u8;
    fn CVPixelBufferGetBytesPerRow(buf: *const c_void) -> usize;
    fn CVPixelBufferGetWidth(buf: *const c_void) -> usize;
    fn CVPixelBufferGetHeight(buf: *const c_void) -> usize;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGColorSpaceCreateDeviceRGB() -> *mut c_void;
    fn CGColorSpaceRelease(space: *mut c_void);
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: *mut c_void,
        bitmap_info: u32,
    ) -> *mut c_void;
    fn CGContextDrawImage(c: *mut c_void, rect: CgRect, image: *const c_void);
    fn CGContextRelease(c: *mut c_void);
    fn CGContextSetInterpolationQuality(c: *mut c_void, quality: i32);
}

/// `kCVPixelFormatType_32BGRA`
const BGRA: u32 = u32::from_be_bytes(*b"BGRA");
/// `kCVPixelBufferLock_ReadOnly`
const READ_ONLY: u64 = 1;
/// `kCGImageAlphaNoneSkipFirst | kCGBitmapByteOrder32Little`: BGRX.
const BITMAP_BGRX: u32 = 6 | (2 << 12);

fn url(path: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
}

fn asset(path: &Path) -> Retained<AVURLAsset> {
    unsafe { AVURLAsset::URLAssetWithURL_options(&url(path), None) }
}

fn tracks(asset: &AVAsset, video: bool) -> Retained<NSArray<AVAssetTrack>> {
    #[allow(deprecated)]
    unsafe {
        let kind = if video {
            AVMediaTypeVideo
        } else {
            AVMediaTypeAudio
        };
        match kind {
            Some(kind) => asset.tracksWithMediaType(kind),
            None => NSArray::new(),
        }
    }
}

fn seconds(t: CMTime) -> Option<f64> {
    let s = unsafe { t.seconds() };
    s.is_finite().then_some(s.max(0.))
}

fn cm_time(seconds: f64) -> CMTime {
    unsafe { CMTime::with_seconds(seconds.max(0.), 90_000) }
}

fn four_cc(code: u32) -> String {
    code.to_be_bytes()
        .iter()
        .map(|b| {
            if b.is_ascii_graphic() || *b == b' ' {
                *b as char
            } else {
                '?'
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}

fn video_codec(code: u32) -> String {
    match &code.to_be_bytes() {
        b"avc1" | b"avc3" => "H.264".into(),
        b"hvc1" | b"hev1" => "HEVC (H.265)".into(),
        b"av01" => "AV1".into(),
        b"vp09" => "VP9".into(),
        b"mp4v" => "MPEG-4 Part 2".into(),
        b"apcn" => "Apple ProRes 422".into(),
        b"apch" => "Apple ProRes 422 HQ".into(),
        b"apcs" => "Apple ProRes 422 LT".into(),
        b"apco" => "Apple ProRes 422 Proxy".into(),
        b"ap4h" => "Apple ProRes 4444".into(),
        b"ap4x" => "Apple ProRes 4444 XQ".into(),
        b"jpeg" => "Motion JPEG".into(),
        b"png " => "PNG".into(),
        b"gif " => "GIF".into(),
        _ => four_cc(code),
    }
}

fn audio_codec(code: u32) -> String {
    match &code.to_be_bytes() {
        b"aac " => "AAC".into(),
        b"aach" => "HE-AAC".into(),
        b"alac" => "Apple Lossless".into(),
        b".mp3" => "MP3".into(),
        b"lpcm" => "PCM".into(),
        b"opus" => "Opus".into(),
        b"flac" => "FLAC".into(),
        b"ac-3" => "Dolby Digital (AC-3)".into(),
        b"ec-3" => "Dolby Digital Plus (E-AC-3)".into(),
        b"ima4" => "IMA 4:1 ADPCM".into(),
        b"ulaw" => "μ-law".into(),
        b"alaw" => "A-law".into(),
        _ => four_cc(code),
    }
}

/// The first format description of a track (a `CMFormatDescriptionRef`).
fn format_description(track: &AVAssetTrack) -> Option<*const c_void> {
    let descriptions = unsafe { track.formatDescriptions() };
    let first: Retained<AnyObject> = descriptions.firstObject()?;
    // kept alive by the track for as long as it is used here
    Some(Retained::as_ptr(&first) as *const c_void)
}

pub(crate) fn probe(path: &Path) -> Option<MediaInfo> {
    autoreleasepool(|_| {
        let asset = asset(path);
        let duration = seconds(unsafe { asset.duration() });
        let mut info = MediaInfo {
            container: crate::container_name(&crate::extension(path)).to_string(),
            duration_us: duration.map(|s| (s * 1e6).round() as u64),
            ..Default::default()
        };
        let video = tracks(&asset, true);
        if let Some(track) = video.firstObject() {
            let playable = unsafe { track.isPlayable() && track.isDecodable() };
            let size = unsafe { track.naturalSize() };
            let t = unsafe { track.preferredTransform() };
            let rotation = (t.b.atan2(t.a).to_degrees().round() as i64).rem_euclid(360) as u32;
            let fps = unsafe { track.nominalFrameRate() } as f64;
            let rate = unsafe { track.estimatedDataRate() } as f64;
            let codec = format_description(&track)
                .map(|d| video_codec(unsafe { CMFormatDescriptionGetMediaSubType(d) }))
                .unwrap_or_default();
            let track_seconds = seconds(unsafe { track.timeRange() }.duration).unwrap_or(0.);
            info.video = Some(VideoStreamInfo {
                codec,
                width: size.width.round().max(0.) as u32,
                height: size.height.round().max(0.) as u32,
                frame_rate: (fps > 0.).then(|| ((fps * 1000.).round() as u32, 1000)),
                frames: (fps > 0. && track_seconds > 0.)
                    .then(|| (fps * track_seconds).round() as u64),
                rotation: (rotation + 45) / 90 % 4 * 90,
                pixel_format: None,
                bit_rate: (rate > 0.).then(|| rate.round() as u64),
            });
            if playable {
                info.video_decoder = MediaDecoder::System;
            }
        }
        let audio = tracks(&asset, false);
        if let Some(track) = audio.firstObject() {
            let playable = unsafe { track.isPlayable() && track.isDecodable() };
            let asbd = format_description(&track)
                .map(|d| unsafe { CMAudioFormatDescriptionGetStreamBasicDescription(d) })
                .filter(|p| !p.is_null())
                .map(|p| unsafe { *p })
                .unwrap_or_default();
            let rate = unsafe { track.estimatedDataRate() } as f64;
            info.audio = Some(AudioStreamInfo {
                codec: audio_codec(asbd.format_id),
                sample_rate: asbd.sample_rate.round() as u32,
                channels: asbd.channels_per_frame as u16,
                bits_per_sample: (asbd.bits_per_channel > 0).then_some(asbd.bits_per_channel),
                bit_rate: (rate > 0.).then(|| rate.round() as u64),
            });
            if playable {
                info.audio_decoder = MediaDecoder::System;
            }
        }
        if info.video.is_none() && info.audio.is_none() {
            return None;
        }
        if let Ok(meta) = std::fs::metadata(path)
            && let Some(d) = duration.filter(|d| *d > 0.)
        {
            info.bit_rate = Some((meta.len() as f64 * 8. / d).round() as u64);
        }
        #[allow(deprecated)]
        let common = unsafe { asset.commonMetadata() };
        for item in common.iter() {
            let Some(key) = (unsafe { item.commonKey() }) else {
                continue;
            };
            let key = key.to_string();
            if key == "artwork" {
                if let Some(data) = unsafe { item.dataValue() } {
                    let bytes = data.to_vec();
                    let media_type = if bytes.starts_with(b"\x89PNG") {
                        "image/png"
                    } else {
                        "image/jpg"
                    };
                    info.cover.get_or_insert(ImageBlob {
                        bytes,
                        media_type: media_type.into(),
                        original: None,
                    });
                }
            } else if let Some(value) = unsafe { item.stringValue() } {
                let name = match key.as_str() {
                    "albumName" => "album".to_string(),
                    "creationDate" => "date".to_string(),
                    "description" => "comment".to_string(),
                    other => other.to_ascii_lowercase(),
                };
                if !info.tags.iter().any(|(k, _)| *k == name) {
                    info.tags.push((name, value.to_string()));
                }
            }
        }
        info.tags.sort();
        Some(info)
    })
}

/// Image generators kept per file (opening an asset costs more than a
/// picture); the newest last.
struct Generators(Vec<(std::path::PathBuf, Retained<AVAssetImageGenerator>)>);

// SAFETY: AVAssetImageGenerator may be used from any thread; the mutex
// keeps the calls on one generator one at a time.
unsafe impl Send for Generators {}

static GENERATORS: Mutex<Generators> = Mutex::new(Generators(Vec::new()));

fn generator(path: &Path) -> Retained<AVAssetImageGenerator> {
    let mut cache = GENERATORS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, g)) = cache.0.iter().find(|(p, _)| p == path) {
        return g.clone();
    }
    let asset = asset(path);
    let g = unsafe { AVAssetImageGenerator::assetImageGeneratorWithAsset(&asset) };
    unsafe {
        g.setAppliesPreferredTrackTransform(true);
        let zero = cm_time(0.);
        g.setRequestedTimeToleranceBefore(zero);
        g.setRequestedTimeToleranceAfter(zero);
    }
    cache.0.push((path.to_path_buf(), g.clone()));
    if cache.0.len() > 4 {
        cache.0.remove(0);
    }
    g
}

/// Draws a `CGImageRef` into a `w`×`h` BGRA picture.
fn draw(image: *const c_void, (w, h): (u32, u32)) -> Option<Vec<u8>> {
    let mut bytes = vec![0u8; (w * h * 4) as usize];
    unsafe {
        let space = CGColorSpaceCreateDeviceRGB();
        let ctx = CGBitmapContextCreate(
            bytes.as_mut_ptr().cast(),
            w as usize,
            h as usize,
            8,
            w as usize * 4,
            space,
            BITMAP_BGRX,
        );
        CGColorSpaceRelease(space);
        if ctx.is_null() {
            return None;
        }
        // kCGInterpolationHigh
        CGContextSetInterpolationQuality(ctx, 3);
        CGContextDrawImage(
            ctx,
            CgRect {
                x: 0.,
                y: 0.,
                w: w as f64,
                h: h as f64,
            },
            image,
        );
        CGContextRelease(ctx);
    }
    for p in bytes.as_chunks_mut::<4>().0.iter_mut() {
        p[3] = 255;
    }
    Some(bytes)
}

pub(crate) fn frame_at(
    path: &Path,
    _info: &MediaInfo,
    time: f64,
    size: (u32, u32),
) -> Result<Frame> {
    autoreleasepool(|_| {
        let g = generator(path);
        unsafe {
            g.setMaximumSize(objc2_core_foundation::CGSize {
                width: size.0 as f64,
                height: size.1 as f64,
            })
        };
        #[allow(deprecated)]
        let image =
            unsafe { g.copyCGImageAtTime_actualTime_error(cm_time(time), std::ptr::null_mut()) }
                .map_err(|e| decode_error(e.localizedDescription()))?;
        let ptr = Retained::as_ptr(&image) as *const c_void;
        let bytes = draw(ptr, size).ok_or_else(|| decode_error("Could not draw the picture."))?;
        Ok(Frame::new(time, size.0, size.1, bytes))
    })
}

fn number(v: f64) -> Retained<NSNumber> {
    NSNumber::new_f64(v)
}

fn settings(pairs: &[(&str, Retained<NSNumber>)]) -> Retained<NSDictionary<NSString, AnyObject>> {
    let keys: Vec<Retained<NSString>> = pairs.iter().map(|(k, _)| NSString::from_str(k)).collect();
    let keys: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
    let values: Vec<&AnyObject> = pairs
        .iter()
        .map(|(_, v)| -> &AnyObject { v.as_ref() })
        .collect();
    NSDictionary::from_slices(&keys, &values)
}

/// An `AVAssetReader` with its one output, used from one thread at a time.
struct Reading {
    reader: Retained<AVAssetReader>,
    output: Retained<AVAssetReaderOutput>,
}

// SAFETY: the reader and its output are only used by the thread that owns
// the `Reading` (moved, never shared).
unsafe impl Send for Reading {}

impl Reading {
    fn start(
        path: &Path,
        time: f64,
        output: impl FnOnce(&AVAsset) -> Option<Retained<AVAssetReaderOutput>>,
    ) -> Result<Self> {
        let asset = asset(path);
        let reader = unsafe { AVAssetReader::assetReaderWithAsset_error(&asset) }
            .map_err(|e| decode_error(e.localizedDescription()))?;
        let output = output(&asset).ok_or(MediaError::NoDecoder)?;
        unsafe {
            output.setAlwaysCopiesSampleData(false);
            if !reader.canAddOutput(&output) {
                return Err(decode_error("The reader takes no output."));
            }
            reader.addOutput(&output);
            reader.setTimeRange(CMTimeRange {
                start: cm_time(time),
                duration: kCMTimePositiveInfinity,
            });
            if !reader.startReading() {
                return Err(decode_error(
                    reader
                        .error()
                        .map_or("The file could not be read.".into(), |e| {
                            e.localizedDescription().to_string()
                        }),
                ));
            }
        }
        Ok(Self { reader, output })
    }

    /// The next sample buffer, `None` at the end.
    fn next(&self) -> Result<Option<Retained<CMSampleBuffer>>> {
        let sample = unsafe { self.output.copyNextSampleBuffer() };
        if sample.is_some() {
            return Ok(sample);
        }
        match unsafe { self.reader.status() } {
            AVAssetReaderStatus::Failed => Err(decode_error(
                unsafe { self.reader.error() }.map_or("Reading failed.".into(), |e| {
                    e.localizedDescription().to_string()
                }),
            )),
            _ => Ok(None),
        }
    }
}

impl Drop for Reading {
    fn drop(&mut self) {
        unsafe { self.reader.cancelReading() };
    }
}

fn sample_ptr(sample: &CMSampleBuffer) -> *const c_void {
    sample as *const CMSampleBuffer as *const c_void
}

struct AvfVideo {
    reading: Reading,
    /// The picture's size before turning.
    size: (u32, u32),
    rotation: u32,
    start: f64,
    /// The last picture before `start`, shown at `start` when the next one
    /// starts after it.
    held: Option<Frame>,
    queued: Option<Frame>,
}

impl AvfVideo {
    fn decode(&self, sample: &CMSampleBuffer) -> Option<Frame> {
        let s = sample_ptr(sample);
        let time = seconds(unsafe { CMSampleBufferGetPresentationTimeStamp(s) })?;
        let buf = unsafe { CMSampleBufferGetImageBuffer(s) };
        if buf.is_null() {
            return None;
        }
        unsafe {
            if CVPixelBufferLockBaseAddress(buf, READ_ONLY) != 0 {
                return None;
            }
        }
        let (w, h, stride, base) = unsafe {
            (
                CVPixelBufferGetWidth(buf) as u32,
                CVPixelBufferGetHeight(buf) as u32,
                CVPixelBufferGetBytesPerRow(buf),
                CVPixelBufferGetBaseAddress(buf),
            )
        };
        let pixels = (!base.is_null() && w > 0 && h > 0).then(|| {
            let src = unsafe { std::slice::from_raw_parts(base, stride * h as usize) };
            resize_bgra(src, w, h, stride, self.size.0, self.size.1)
        });
        unsafe { CVPixelBufferUnlockBaseAddress(buf, READ_ONLY) };
        let mut pixels = pixels?;
        for p in pixels.as_chunks_mut::<4>().0.iter_mut() {
            p[3] = 255;
        }
        let (w, h, pixels) = rotate_bgra(pixels, self.size.0, self.size.1, self.rotation);
        Some(Frame::new(time, w, h, pixels))
    }
}

impl VideoReader for AvfVideo {
    fn next_frame(&mut self) -> Result<Option<Frame>> {
        if let Some(f) = self.queued.take() {
            return Ok(Some(f));
        }
        loop {
            let next = autoreleasepool(|_| -> Result<Option<Option<Frame>>> {
                let Some(sample) = self.reading.next()? else {
                    return Ok(None);
                };
                Ok(Some(self.decode(&sample)))
            })?;
            let Some(frame) = next else {
                // the end: a held picture is still the one at `start`
                return Ok(self.held.take().map(|mut f| {
                    f.time = self.start;
                    f
                }));
            };
            let Some(frame) = frame else { continue };
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
    let rotation = info.video.as_ref().map_or(0, |v| v.rotation);
    // decoded at the size before turning
    let stored = if rotation % 180 == 90 {
        (size.1, size.0)
    } else {
        size
    };
    autoreleasepool(|_| {
        // a little early, so the picture showing at `time` is read too
        let reading = Reading::start(path, (time - 0.1).max(0.), |asset| {
            let track = tracks(asset, true).firstObject()?;
            let settings = settings(&[("PixelFormatType", NSNumber::new_u32(BGRA))]);
            let output = unsafe {
                AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(
                    &track,
                    Some(&settings),
                )
            };
            Some(Retained::into_super(output))
        })?;
        Ok(Box::new(AvfVideo {
            reading,
            size: stored,
            rotation,
            start: time,
            held: None,
            queued: None,
        }) as Box<dyn VideoReader>)
    })
}

struct AvfAudio {
    reading: Reading,
    rate: u32,
    channels: u16,
    start: f64,
}

impl AudioReader for AvfAudio {
    fn next_chunk(&mut self) -> Result<Option<AudioChunk>> {
        loop {
            let chunk = autoreleasepool(|_| -> Result<Option<Option<AudioChunk>>> {
                let Some(sample) = self.reading.next()? else {
                    return Ok(None);
                };
                let s = sample_ptr(&sample);
                let time =
                    seconds(unsafe { CMSampleBufferGetPresentationTimeStamp(s) }).unwrap_or(0.);
                let block = unsafe { CMSampleBufferGetDataBuffer(s) };
                if block.is_null() {
                    return Ok(Some(None));
                }
                let len = unsafe { CMBlockBufferGetDataLength(block) };
                let mut bytes = vec![0u8; len];
                if len == 0
                    || unsafe {
                        CMBlockBufferCopyDataBytes(block, 0, len, bytes.as_mut_ptr().cast())
                    } != 0
                {
                    return Ok(Some(None));
                }
                let mut samples: Vec<f32> = bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .collect();
                let channels = self.channels as usize;
                let mut time = time;
                // the reader starts at a packet: drop what is before `start`
                if time < self.start {
                    let skip = (((self.start - time) * self.rate as f64).round() as usize)
                        .min(samples.len() / channels);
                    samples.drain(..skip * channels);
                    time = self.start;
                }
                if samples.is_empty() {
                    return Ok(Some(None));
                }
                Ok(Some(Some(AudioChunk {
                    time,
                    rate: self.rate,
                    channels: self.channels,
                    samples,
                })))
            })?;
            match chunk {
                None => return Ok(None),
                Some(Some(chunk)) => return Ok(Some(chunk)),
                Some(None) => continue,
            }
        }
    }
}

pub(crate) fn audio_from(path: &Path, time: f64) -> Result<Box<dyn AudioReader>> {
    autoreleasepool(|_| {
        let mut rate = 48_000u32;
        let mut channels = 2u16;
        let reading = Reading::start(path, time, |asset| {
            let all = tracks(asset, false);
            let track = all.firstObject()?;
            let asbd = format_description(&track)
                .map(|d| unsafe { CMAudioFormatDescriptionGetStreamBasicDescription(d) })
                .filter(|p| !p.is_null())
                .map(|p| unsafe { *p })
                .unwrap_or_default();
            if asbd.sample_rate >= 8000. {
                rate = asbd.sample_rate.round() as u32;
            }
            channels = (asbd.channels_per_frame as u16).clamp(1, 2);
            let settings = settings(&[
                (
                    "AVFormatIDKey",
                    NSNumber::new_u32(u32::from_be_bytes(*b"lpcm")),
                ),
                ("AVLinearPCMIsFloatKey", NSNumber::new_bool(true)),
                ("AVLinearPCMBitDepthKey", NSNumber::new_u32(32)),
                ("AVLinearPCMIsNonInterleaved", NSNumber::new_bool(false)),
                ("AVLinearPCMIsBigEndianKey", NSNumber::new_bool(false)),
                ("AVSampleRateKey", number(rate as f64)),
                ("AVNumberOfChannelsKey", NSNumber::new_u32(channels as u32)),
            ]);
            let output = unsafe {
                AVAssetReaderAudioMixOutput::assetReaderAudioMixOutputWithAudioTracks_audioSettings(
                    &all,
                    Some(&settings),
                )
            };
            Some(Retained::into_super(output))
        })?;
        Ok(Box::new(AvfAudio {
            reading,
            rate,
            channels,
            start: time,
        }) as Box<dyn AudioReader>)
    })
}
