//! Sound read by Symphonia (pure Rust): WAVE, AIFF, CAF, FLAC, MP3, AAC and
//! ALAC in MP4, Vorbis in Ogg, Matroska/WebM audio it has codecs for.

use std::fs::File;
use std::path::Path;

use corvene_models::{AudioStreamInfo, ImageBlob, MediaDecoder, MediaInfo};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, Decoder, DecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::{MetadataOptions, MetadataRevision, StandardTagKey, StandardVisualKey};
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

use crate::frame::AudioChunk;
use crate::{AudioReader, MediaError, Result, decode_error};

struct Opened {
    format: Box<dyn FormatReader>,
    track: u32,
    decoder: Box<dyn Decoder>,
    rate: u32,
    channels: u16,
    tags: Vec<(String, String)>,
    cover: Option<ImageBlob>,
}

fn open(path: &Path) -> Result<Opened> {
    let file = File::open(path)?;
    let stream = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());
    let mut hint = Hint::new();
    hint.with_extension(&crate::extension(path));
    let mut probed = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions {
                enable_gapless: true,
                ..Default::default()
            },
            &MetadataOptions::default(),
        )
        .map_err(decode_error)?;
    let track = probed
        .format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL && t.codec_params.sample_rate.is_some())
        .ok_or(MediaError::NoDecoder)?;
    let params = track.codec_params.clone();
    let track_id = track.id;
    let decoder = symphonia::default::get_codecs()
        .make(&params, &DecoderOptions::default())
        .map_err(decode_error)?;
    let mut tags = Vec::new();
    let mut cover = None;
    let mut read = |rev: &MetadataRevision| {
        for tag in rev.tags() {
            let key = match tag.std_key {
                Some(k) => tag_name(k).map(str::to_string),
                None => Some(tag.key.to_ascii_lowercase()),
            };
            if let Some(key) = key
                && !tags.iter().any(|(k, _): &(String, String)| *k == key)
            {
                let value = tag.value.to_string();
                if !value.is_empty() && value.len() < 512 {
                    tags.push((key, value));
                }
            }
        }
        for visual in rev.visuals() {
            let front = matches!(visual.usage, Some(StandardVisualKey::FrontCover) | None);
            if cover.is_none() || front {
                cover = Some(ImageBlob {
                    bytes: visual.data.to_vec(),
                    media_type: match visual.media_type.as_str() {
                        "image/jpeg" => "image/jpg".to_string(),
                        other => other.to_string(),
                    },
                    original: None,
                });
            }
        }
    };
    if let Some(mut meta) = probed.metadata.get()
        && let Some(rev) = meta.skip_to_latest()
    {
        read(rev);
    }
    if let Some(rev) = probed.format.metadata().skip_to_latest() {
        read(rev);
    }
    tags.sort();
    let rate = params.sample_rate.unwrap_or(44_100);
    let channels = params.channels.map_or(2, |c| c.count() as u16).max(1);
    Ok(Opened {
        format: probed.format,
        track: track_id,
        decoder,
        rate,
        channels,
        tags,
        cover,
    })
}

/// The names [`StandardTagKey`]s are shown with.
fn tag_name(key: StandardTagKey) -> Option<&'static str> {
    use StandardTagKey::*;
    Some(match key {
        TrackTitle => "title",
        Artist => "artist",
        Album => "album",
        AlbumArtist => "album_artist",
        Composer => "composer",
        Genre => "genre",
        Date | ReleaseDate | OriginalDate => "date",
        TrackNumber => "track",
        DiscNumber => "disc",
        Comment => "comment",
        Copyright => "copyright",
        Encoder => "encoder",
        Bpm => "bpm",
        Label => "label",
        Lyrics => "lyrics",
        IdentIsrc => "isrc",
        Description => "description",
        TrackTotal => "track_total",
        Conductor => "conductor",
        Lyricist => "lyricist",
        ReplayGainTrackGain => "replaygain_track_gain",
        ReplayGainAlbumGain => "replaygain_album_gain",
        _ => return None,
    })
}

pub(crate) fn probe(path: &Path) -> Option<MediaInfo> {
    let opened = open(path).ok()?;
    let track = opened
        .format
        .tracks()
        .iter()
        .find(|t| t.id == opened.track)?;
    let params = &track.codec_params;
    let duration_us = match (params.n_frames, params.time_base) {
        (Some(n), Some(tb)) => {
            let t = tb.calc_time(n);
            Some((t.seconds as f64 * 1e6 + t.frac * 1e6).round() as u64)
        }
        (Some(n), None) => Some((n as f64 / opened.rate as f64 * 1e6).round() as u64),
        _ => None,
    };
    let codec = symphonia::default::get_codecs()
        .get_codec(params.codec)
        .map_or_else(|| "Unknown".to_string(), |d| d.long_name.to_string());
    let bytes = std::fs::metadata(path).map(|m| m.len()).ok();
    let bit_rate = match (bytes, duration_us) {
        (Some(b), Some(us)) if us > 0 => Some((b as f64 * 8. / (us as f64 / 1e6)).round() as u64),
        _ => None,
    };
    Some(MediaInfo {
        container: crate::container_name(&crate::extension(path)).to_string(),
        duration_us,
        bit_rate,
        video: None,
        audio: Some(AudioStreamInfo {
            codec,
            sample_rate: opened.rate,
            channels: opened.channels,
            bits_per_sample: params.bits_per_sample,
            bit_rate,
        }),
        tags: opened.tags,
        cover: opened.cover,
        video_decoder: MediaDecoder::None,
        audio_decoder: MediaDecoder::Symphonia,
    })
}

struct SymphoniaAudio {
    opened: Opened,
    buffer: Option<SampleBuffer<f32>>,
    /// Frames of the next decoded packet to drop (an accurate seek lands
    /// on the packet holding the time).
    skip: u64,
    time: f64,
}

impl AudioReader for SymphoniaAudio {
    fn next_chunk(&mut self) -> Result<Option<AudioChunk>> {
        loop {
            let packet = match self.opened.format.next_packet() {
                Ok(p) => p,
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Ok(None);
                }
                Err(SymError::ResetRequired) => return Ok(None),
                Err(e) => return Err(decode_error(e)),
            };
            if packet.track_id() != self.opened.track {
                continue;
            }
            let decoded = match self.opened.decoder.decode(&packet) {
                Ok(d) => d,
                // a damaged packet: skip it
                Err(SymError::DecodeError(_)) => continue,
                Err(e) => return Err(decode_error(e)),
            };
            let spec = *decoded.spec();
            let frames = decoded.frames();
            if frames == 0 {
                continue;
            }
            let buffer = match &mut self.buffer {
                Some(b) if b.capacity() >= decoded.capacity() * spec.channels.count() => b,
                slot => slot.insert(SampleBuffer::new(decoded.capacity() as u64, spec)),
            };
            buffer.copy_interleaved_ref(decoded);
            let channels = spec.channels.count().max(1);
            let mut samples = buffer.samples().to_vec();
            if self.skip > 0 {
                let drop = (self.skip as usize).min(frames);
                samples.drain(..drop * channels);
                self.skip -= drop as u64;
                if samples.is_empty() {
                    continue;
                }
            }
            let chunk = AudioChunk {
                time: self.time,
                rate: spec.rate,
                channels: channels as u16,
                samples,
            };
            self.time += chunk.frames() as f64 / spec.rate.max(1) as f64;
            return Ok(Some(chunk));
        }
    }
}

pub(crate) fn audio_from(path: &Path, time: f64) -> Result<Box<dyn AudioReader>> {
    let mut opened = open(path)?;
    let mut skip = 0;
    let mut start = 0.;
    if time > 0. {
        let seeked = opened
            .format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time: Time::from(time),
                    track_id: Some(opened.track),
                },
            )
            .map_err(decode_error)?;
        opened.decoder.reset();
        skip = seeked.required_ts.saturating_sub(seeked.actual_ts);
        let tb = opened
            .format
            .tracks()
            .iter()
            .find(|t| t.id == opened.track)
            .and_then(|t| t.codec_params.time_base);
        start = match tb {
            Some(tb) => {
                let t = tb.calc_time(seeked.required_ts);
                t.seconds as f64 + t.frac
            }
            None => time,
        };
        // time stamps count frames for every format Symphonia reads
        if tb.is_some_and(|tb| tb.numer != 1 || tb.denom != opened.rate) {
            let actual = tb.map_or(0., |tb| {
                let t = tb.calc_time(seeked.actual_ts);
                t.seconds as f64 + t.frac
            });
            skip = ((start - actual).max(0.) * opened.rate as f64).round() as u64;
        }
    }
    Ok(Box::new(SymphoniaAudio {
        opened,
        buffer: None,
        skip,
        time: start,
    }))
}
