//! `1337-media-ffmpeg`: an installed `ffmpeg` / `ffprobe` for what the
//! system and Symphonia cannot open (WebM, Opus, AVI, … and everything on
//! Linux). Pictures and sound come out of a pipe as raw BGRA / `f32`; the
//! child is killed when its reader is dropped.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::OnceLock;

use corvene_models::{AudioStreamInfo, ImageBlob, MediaDecoder, MediaInfo, VideoStreamInfo};

use crate::frame::{AudioChunk, Frame};
use crate::{AudioReader, MediaError, Result, VideoReader, decode_error};

pub(crate) struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

/// The installed tools, looked up once: `PATH`, then the Homebrew and
/// MacPorts prefixes (an app started from Finder has a short `PATH`).
pub(crate) fn tools() -> Option<&'static Tools> {
    static TOOLS: OnceLock<Option<Tools>> = OnceLock::new();
    TOOLS
        .get_or_init(|| {
            let exe = |name: &str| {
                if cfg!(windows) {
                    format!("{name}.exe")
                } else {
                    name.to_string()
                }
            };
            let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
                .map(|p| std::env::split_paths(&p).collect())
                .unwrap_or_default();
            for extra in [
                "/opt/homebrew/bin",
                "/usr/local/bin",
                "/opt/local/bin",
                "/usr/bin",
            ] {
                dirs.push(PathBuf::from(extra));
            }
            let find = |name: &str| dirs.iter().map(|d| d.join(exe(name))).find(|p| p.is_file());
            Some(Tools {
                ffmpeg: find("ffmpeg")?,
                ffprobe: find("ffprobe")?,
            })
        })
        .as_ref()
}

fn usable() -> Option<&'static Tools> {
    if crate::ffmpeg_enabled() {
        tools()
    } else {
        None
    }
}

fn command(program: &Path) -> Command {
    let mut c = Command::new(program);
    c.stdin(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW
        c.creation_flags(0x0800_0000);
    }
    c
}

/// A running `ffmpeg` whose output is read; killed when dropped.
struct Pipe {
    child: Child,
    out: ChildStdout,
}

impl Pipe {
    fn spawn(mut cmd: Command) -> Result<Self> {
        let mut child = cmd.stdout(Stdio::piped()).spawn()?;
        let out = child
            .stdout
            .take()
            .ok_or_else(|| decode_error("ffmpeg has no output"))?;
        Ok(Self { child, out })
    }

    /// Fills `buf`; `false` at the end of the output.
    fn read_exact_or_end(&mut self, buf: &mut [u8]) -> Result<bool> {
        let mut filled = 0;
        while filled < buf.len() {
            match self.out.read(&mut buf[filled..])? {
                0 => return Ok(false),
                n => filled += n,
            }
        }
        Ok(true)
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok();
    }
}

fn seconds(s: &str) -> Option<f64> {
    s.parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.)
}

fn fraction(s: &str) -> Option<(u32, u32)> {
    let (n, d) = s.split_once('/')?;
    let (n, d) = (n.parse().ok()?, d.parse().ok()?);
    (n > 0 && d > 0).then_some((n, d))
}

fn number(v: &serde_json::Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

/// What `ffprobe` says about `path`.
pub(crate) fn probe(path: &Path) -> Option<MediaInfo> {
    let tools = usable()?;
    let mut cmd = command(&tools.ffprobe);
    cmd.args([
        "-v",
        "error",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
    ])
    .arg(path);
    let output = cmd.stdout(Stdio::piped()).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let format = &json["format"];
    let mut info = MediaInfo {
        container: format["format_long_name"]
            .as_str()
            .or(format["format_name"].as_str())
            .unwrap_or_default()
            .to_string(),
        duration_us: format["duration"]
            .as_str()
            .and_then(seconds)
            .map(|s| (s * 1e6).round() as u64),
        bit_rate: number(&format["bit_rate"]),
        ..Default::default()
    };
    if let Some(tags) = format["tags"].as_object() {
        info.tags = tags
            .iter()
            .filter_map(|(k, v)| Some((k.to_ascii_lowercase(), v.as_str()?.to_string())))
            .collect();
        info.tags.sort();
    }
    let mut cover_stream = None;
    for stream in json["streams"].as_array().into_iter().flatten() {
        let codec = stream["codec_long_name"]
            .as_str()
            .or(stream["codec_name"].as_str())
            .unwrap_or_default()
            .to_string();
        match stream["codec_type"].as_str() {
            Some("video") if stream["disposition"]["attached_pic"].as_u64() == Some(1) => {
                cover_stream.get_or_insert(stream["index"].as_u64().unwrap_or(0));
            }
            Some("video") if info.video.is_none() => {
                let rotation = stream["side_data_list"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find_map(|d| d["rotation"].as_i64())
                    .or_else(|| {
                        stream["tags"]["rotate"]
                            .as_str()
                            .and_then(|s| s.parse::<i64>().ok())
                            .map(|r| -r)
                    })
                    .unwrap_or(0);
                // ffprobe's rotation is counter-clockwise
                let rotation = ((-rotation).rem_euclid(360) as u32 / 90) * 90;
                info.video = Some(VideoStreamInfo {
                    codec,
                    width: stream["width"].as_u64().unwrap_or(0) as u32,
                    height: stream["height"].as_u64().unwrap_or(0) as u32,
                    frame_rate: stream["avg_frame_rate"]
                        .as_str()
                        .and_then(fraction)
                        .or_else(|| stream["r_frame_rate"].as_str().and_then(fraction)),
                    frames: number(&stream["nb_frames"]),
                    rotation,
                    pixel_format: stream["pix_fmt"].as_str().map(str::to_string),
                    bit_rate: number(&stream["bit_rate"]),
                });
                info.video_decoder = MediaDecoder::Ffmpeg;
                if info.duration_us.is_none() {
                    info.duration_us = stream["duration"]
                        .as_str()
                        .and_then(seconds)
                        .map(|s| (s * 1e6).round() as u64);
                }
            }
            Some("audio") if info.audio.is_none() => {
                info.audio = Some(AudioStreamInfo {
                    codec,
                    sample_rate: number(&stream["sample_rate"]).unwrap_or(0) as u32,
                    channels: stream["channels"].as_u64().unwrap_or(0) as u16,
                    bits_per_sample: number(&stream["bits_per_raw_sample"])
                        .or_else(|| number(&stream["bits_per_sample"]))
                        .filter(|b| *b > 0)
                        .map(|b| b as u32),
                    bit_rate: number(&stream["bit_rate"]),
                });
                info.audio_decoder = MediaDecoder::Ffmpeg;
                if let Some(tags) = stream["tags"].as_object()
                    && info.tags.is_empty()
                {
                    // Ogg keeps its comments on the stream
                    info.tags = tags
                        .iter()
                        .filter(|(k, _)| !k.eq_ignore_ascii_case("encoder"))
                        .filter_map(|(k, v)| {
                            Some((k.to_ascii_lowercase(), v.as_str()?.to_string()))
                        })
                        .collect();
                    info.tags.sort();
                }
            }
            _ => {}
        }
    }
    if let Some(index) = cover_stream {
        info.cover = cover(tools, path, index);
    }
    if info.video.is_none() && info.audio.is_none() {
        return None;
    }
    Some(info)
}

/// An attached picture, as PNG.
fn cover(tools: &Tools, path: &Path, stream: u64) -> Option<ImageBlob> {
    let mut cmd = command(&tools.ffmpeg);
    cmd.args(["-v", "error", "-nostdin", "-i"])
        .arg(path)
        .args(["-map", &format!("0:{stream}"), "-frames:v", "1"])
        .args(["-c:v", "png", "-f", "image2pipe", "-"]);
    let output = cmd.stdout(Stdio::piped()).output().ok()?;
    (output.status.success() && output.stdout.starts_with(b"\x89PNG")).then(|| ImageBlob {
        bytes: output.stdout,
        media_type: "image/png".to_string(),
        original: None,
    })
}

fn video_command(
    tools: &Tools,
    path: &Path,
    time: f64,
    (w, h): (u32, u32),
    fps: Option<f64>,
) -> Command {
    let mut cmd = command(&tools.ffmpeg);
    cmd.args(["-v", "error", "-nostdin"]);
    if time > 0. {
        cmd.args(["-ss", &format!("{time:.6}")]);
    }
    cmd.arg("-i").arg(path);
    let mut filter = String::new();
    if let Some(fps) = fps {
        filter.push_str(&format!("fps={fps:.6},"));
    }
    filter.push_str(&format!("scale={w}:{h}:flags=area"));
    cmd.args(["-an", "-sn", "-vf", &filter])
        .args(["-f", "rawvideo", "-pix_fmt", "bgra", "-"]);
    cmd
}

pub(crate) fn frame_at(
    path: &Path,
    _info: &MediaInfo,
    time: f64,
    size: (u32, u32),
) -> Result<Frame> {
    let tools = usable().ok_or(MediaError::NoDecoder)?;
    let mut cmd = video_command(tools, path, time, size, None);
    cmd.args(["-frames:v", "1"]);
    let mut pipe = Pipe::spawn(cmd)?;
    let mut buf = vec![0u8; (size.0 * size.1 * 4) as usize];
    if pipe.read_exact_or_end(&mut buf)? {
        Ok(Frame::new(time, size.0, size.1, buf))
    } else {
        Err(decode_error("No picture at this time."))
    }
}

struct FfmpegVideo {
    pipe: Pipe,
    size: (u32, u32),
    start: f64,
    fps: f64,
    index: u64,
}

impl VideoReader for FfmpegVideo {
    fn next_frame(&mut self) -> Result<Option<Frame>> {
        let mut buf = vec![0u8; (self.size.0 * self.size.1 * 4) as usize];
        if !self.pipe.read_exact_or_end(&mut buf)? {
            return Ok(None);
        }
        let time = self.start + self.index as f64 / self.fps;
        self.index += 1;
        Ok(Some(Frame::new(time, self.size.0, self.size.1, buf)))
    }
}

/// The pictures from `time` on at the stream's rate (at most 60 a second),
/// so each one's time is known from its index.
pub(crate) fn video_from(
    path: &Path,
    info: &MediaInfo,
    time: f64,
    size: (u32, u32),
) -> Result<Box<dyn VideoReader>> {
    let tools = usable().ok_or(MediaError::NoDecoder)?;
    let fps = info
        .video
        .as_ref()
        .and_then(|v| v.fps())
        .unwrap_or(30.)
        .clamp(1., 60.);
    let cmd = video_command(tools, path, time, size, Some(fps));
    Ok(Box::new(FfmpegVideo {
        pipe: Pipe::spawn(cmd)?,
        size,
        start: time,
        fps,
        index: 0,
    }))
}

struct FfmpegAudio {
    pipe: Pipe,
    rate: u32,
    channels: u16,
    time: f64,
}

/// Frames read per chunk.
const CHUNK_FRAMES: usize = 4096;

impl AudioReader for FfmpegAudio {
    fn next_chunk(&mut self) -> Result<Option<AudioChunk>> {
        let mut buf = vec![0u8; CHUNK_FRAMES * self.channels as usize * 4];
        let mut filled = 0;
        while filled < buf.len() {
            match self.pipe.out.read(&mut buf[filled..])? {
                0 => break,
                n => filled += n,
            }
        }
        let whole = filled / (self.channels as usize * 4) * self.channels as usize * 4;
        if whole == 0 {
            return Ok(None);
        }
        let samples: Vec<f32> = buf[..whole]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        let chunk = AudioChunk {
            time: self.time,
            rate: self.rate,
            channels: self.channels,
            samples,
        };
        self.time += chunk.frames() as f64 / self.rate as f64;
        Ok(Some(chunk))
    }
}

/// The sound from `time` on, at its own rate, mixed down to at most two
/// channels.
pub(crate) fn audio_from(path: &Path, info: &MediaInfo, time: f64) -> Result<Box<dyn AudioReader>> {
    let tools = usable().ok_or(MediaError::NoDecoder)?;
    let audio = info.audio.as_ref().ok_or(MediaError::NoDecoder)?;
    let rate = if audio.sample_rate > 0 {
        audio.sample_rate
    } else {
        48_000
    };
    let channels = audio.channels.clamp(1, 2);
    let mut cmd = command(&tools.ffmpeg);
    cmd.args(["-v", "error", "-nostdin"]);
    if time > 0. {
        cmd.args(["-ss", &format!("{time:.6}")]);
    }
    cmd.arg("-i")
        .arg(path)
        .args([
            "-vn",
            "-sn",
            "-ac",
            &channels.to_string(),
            "-ar",
            &rate.to_string(),
        ])
        .args(["-f", "f32le", "-"]);
    Ok(Box::new(FfmpegAudio {
        pipe: Pipe::spawn(cmd)?,
        rate,
        channels,
        time,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractions_and_seconds() {
        assert_eq!(fraction("30000/1001"), Some((30000, 1001)));
        assert_eq!(fraction("0/0"), None);
        assert_eq!(seconds("1.5"), Some(1.5));
        assert_eq!(seconds("N/A"), None);
    }
}
