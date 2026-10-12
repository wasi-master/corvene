//! Video and sound diffs (`1335-video-diff`, `1336-audio-diff`). GHD shows
//! these files as "This binary file has changed." (`ui/diff/binary-file.tsx`).
//!
//! Both versions play in step from one clock (`corvene_media::player`):
//! the mode tabs choose how the current picture is compared (2-up, Swipe,
//! Onion Skin, Difference, like the image diff) or how the sound is drawn
//! (stacked or overlaid waveforms, spectrograms); the timeline under them
//! marks where the picture or the sound changed, and the transport bar
//! plays, steps, loops and switches which version is heard.
//!
//! The view only drives `corvene_media`; the files are opened, decoded and
//! analysed there, on its own threads and the background executor.

mod audio;
mod info;
mod timeline;
mod video;

use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use corvene_core::corvene_media::analysis::{self, AudioComparison, AudioSummary, VideoComparison};
use corvene_core::corvene_media::player::{Player, Side};
use corvene_core::corvene_media::{Frame, Source};
use corvene_core::{
    AudioDiffMode, Dispatcher, FileStatusKind, MediaDecoder, MediaDiff, MediaKind, VideoDiffMode,
};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::icons::{Octicon, octicon};
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::{GhdTooltip, IconButtonA11y};

gpui_kit::actions!(
    media_diff,
    [
        PlayPause,
        FrameBack,
        FrameForward,
        SecondBack,
        SecondForward,
        JumpBack,
        JumpForward,
        PreviousChange,
        NextChange,
        ToggleLoop,
        LoopIn,
        LoopOut,
        GoToStart,
        GoToEnd,
        ListenOld,
        ListenNew,
        FlipListen,
        ToggleMute,
        SlowDown,
        SpeedUp,
        Mode1,
        Mode2,
        Mode3,
        Mode4,
        Mode5,
    ]
);

/// The key context of a focused media diff (`keymap.rs`).
pub const KEY_CONTEXT: &str = "MediaDiff";

const SPEEDS: [f64; 6] = [0.25, 0.5, 0.75, 1., 1.5, 2.];

/// A picture on screen: the frame it came from (by its pixels' address)
/// and its GPUI image.
#[derive(Clone)]
struct Shown {
    pixels: usize,
    time: f64,
    size: (u32, u32),
    image: Arc<RenderImage>,
}

fn render_image(frame: &Frame) -> Option<Shown> {
    let buffer = image::RgbaImage::from_raw(frame.width, frame.height, (*frame.bgra).clone())?;
    Some(Shown {
        pixels: Arc::as_ptr(&frame.bgra) as usize,
        time: frame.time,
        size: (frame.width, frame.height),
        image: Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])),
    })
}

/// Columns, view start and end (bits) and background of spectrogram pictures.
type SpectrogramKey = (usize, u64, u64, u32);

/// Sound analysis: each side's summary as it comes in.
#[derive(Default)]
struct SoundAnalysis {
    old: Option<Arc<AudioSummary>>,
    new: Option<Arc<AudioSummary>>,
    /// Sides still being read, and how far (0..1, f32 bits).
    progress: [Arc<AtomicU32>; 2],
    pending: usize,
    failed: bool,
    /// The whole length in the timeline's columns.
    overview: Option<Arc<AudioComparison>>,
    /// The stage's columns over the zoomed range: (columns, start, end).
    lanes: Option<((usize, u64, u64), Arc<AudioComparison>)>,
}

pub struct MediaDiffView {
    diff: Arc<MediaDiff>,
    status: FileStatusKind,
    player: Player,
    focus: FocusHandle,
    cancel: Arc<AtomicBool>,
    sound: SoundAnalysis,
    /// Sampled pictures, filled in by a background task.
    samples: Arc<Mutex<Option<VideoComparison>>>,
    samples_done: bool,
    /// The pictures on screen (old, new) and the difference blend.
    shown: [Option<Shown>; 2],
    difference: Option<((usize, usize, bool), Shown)>,
    difference_pending: bool,
    /// Images to free from the sprite atlas at the next frame.
    retired: Vec<Arc<RenderImage>>,
    /// The timeline's thumbnails, made once per sample.
    thumbs: [Vec<Option<Arc<RenderImage>>>; 2],
    /// Spectrogram pictures for a lanes key: old, new, difference.
    spectrograms: Option<(SpectrogramKey, [Option<Arc<RenderImage>>; 3])>,
    window: Option<AnyWindowHandle>,
    /// Swipe position and onion skin mix, 0 (old) to 1 (new).
    swipe: f32,
    onion: f32,
    /// Difference: brighten faint changes ×4.
    amplify: bool,
    /// The stretch of time the sound lanes show (zoomed with ⌘-scroll).
    view: Option<(f64, f64)>,
    /// The time under the pointer on the timeline or the lanes.
    hover: Option<f64>,
    /// A scrub in progress: whether it was playing when it started.
    scrub: Option<bool>,
    /// A loop range being dragged: where it started.
    loop_drag: Option<f64>,
    stage: Rc<Cell<Bounds<Pixels>>>,
    timeline: Rc<Cell<Bounds<Pixels>>>,
    lanes: Rc<Cell<Bounds<Pixels>>>,
    volume_track: Rc<Cell<Bounds<Pixels>>>,
    volume_drag: bool,
    scale_factor: f32,
}

impl MediaDiffView {
    pub fn new(diff: Arc<MediaDiff>, status: FileStatusKind, cx: &mut Context<Self>) -> Self {
        let old = diff.previous.as_ref().map(Source::of);
        let new = diff.current.as_ref().map(Source::of);
        let player = Player::new(old.clone(), new.clone());
        if let Some(state) = corvene_core::AppState::try_global(cx) {
            let settings = &state.read(cx).settings;
            player.set_volume(settings.media_volume);
            player.set_muted(settings.media_muted);
        }
        // a new file is heard as it is; a deleted one as it was
        player.set_audible(if new.is_some() { Side::New } else { Side::Old });
        let cancel = Arc::new(AtomicBool::new(false));
        cx.on_release({
            let cancel = cancel.clone();
            move |this: &mut Self, cx| {
                cancel.store(true, Ordering::Relaxed);
                let images: Vec<_> = this
                    .shown
                    .iter()
                    .flatten()
                    .map(|s| s.image.clone())
                    .chain(this.difference.iter().map(|(_, s)| s.image.clone()))
                    .chain(this.retired.drain(..))
                    .chain(this.thumbs.iter().flatten().flatten().cloned())
                    .chain(
                        this.spectrograms
                            .iter()
                            .flat_map(|(_, s)| s.iter().flatten().cloned()),
                    )
                    .collect();
                if let Some(window) = this.window {
                    window
                        .update(cx, |_, window, _| {
                            for image in images {
                                window.drop_image(image).ok();
                            }
                        })
                        .ok();
                }
            }
        })
        .detach();
        let mut this = Self {
            diff,
            status,
            player,
            focus: cx.focus_handle(),
            cancel,
            sound: SoundAnalysis::default(),
            samples: Arc::new(Mutex::new(None)),
            samples_done: false,
            shown: [None, None],
            difference: None,
            difference_pending: false,
            retired: Vec::new(),
            thumbs: [Vec::new(), Vec::new()],
            spectrograms: None,
            window: None,
            swipe: 0.5,
            onion: 0.5,
            amplify: false,
            view: None,
            hover: None,
            scrub: None,
            loop_drag: None,
            stage: Rc::default(),
            timeline: Rc::default(),
            lanes: Rc::default(),
            volume_track: Rc::default(),
            volume_drag: false,
            scale_factor: 2.,
        };
        this.start_sound_analysis(old.clone(), new.clone(), cx);
        if this.kind() == MediaKind::Video {
            this.start_video_sampling(old, new, cx);
        }
        this
    }

    pub fn diff(&self) -> &Arc<MediaDiff> {
        &self.diff
    }

    fn kind(&self) -> MediaKind {
        self.diff.kind
    }

    /// Both versions exist (a modified file), so the views compare.
    fn compares(&self) -> bool {
        self.diff.previous.is_some() && self.diff.current.is_some()
    }

    fn duration(&self) -> f64 {
        self.player.duration()
    }

    fn start_sound_analysis(
        &mut self,
        old: Option<Source>,
        new: Option<Source>,
        cx: &mut Context<Self>,
    ) {
        let keys = [
            self.diff.previous.as_ref().map(|s| s.key.clone()),
            self.diff.current.as_ref().map(|s| s.key.clone()),
        ];
        for (i, source) in [old, new].into_iter().enumerate() {
            let Some(source) = source.filter(|s| s.has_audio()) else {
                continue;
            };
            let key = keys[i].clone().unwrap_or_default();
            if let Some(summary) = analysis::cached_audio(&key) {
                self.set_summary(i, summary);
                continue;
            }
            self.sound.pending += 1;
            let cancel = self.cancel.clone();
            let progress = self.sound.progress[i].clone();
            let task = cx.background_executor().spawn(async move {
                let summary = analysis::summarize_audio(&source, &cancel, &mut |p| {
                    progress.store(p.to_bits(), Ordering::Relaxed)
                });
                match summary {
                    Ok(Some(s)) => {
                        let s = Arc::new(s);
                        analysis::cache_audio(key, s.clone());
                        Some(s)
                    }
                    Ok(None) => None,
                    Err(err) => {
                        tracing::debug!(%err, "sound analysis");
                        None
                    }
                }
            });
            cx.spawn(async move |this, cx| {
                let summary = task.await;
                this.update(cx, |this, cx| {
                    this.sound.pending -= 1;
                    match summary {
                        Some(s) => this.set_summary(i, s),
                        None => this.sound.failed = true,
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        if self.sound.pending > 0 {
            self.poll_progress(cx);
        }
    }

    fn set_summary(&mut self, side: usize, summary: Arc<AudioSummary>) {
        if side == 0 {
            self.sound.old = Some(summary);
        } else {
            self.sound.new = Some(summary);
        }
        self.sound.overview = None;
        self.sound.lanes = None;
        self.spectrograms = None;
    }

    fn start_video_sampling(
        &mut self,
        old: Option<Source>,
        new: Option<Source>,
        cx: &mut Context<Self>,
    ) {
        let duration = self.duration();
        if duration <= 0.
            || !(old.as_ref().is_some_and(|s| s.has_video())
                || new.as_ref().is_some_and(|s| s.has_video()))
        {
            self.samples_done = true;
            return;
        }
        let samples = self.samples.clone();
        let cancel = self.cancel.clone();
        cx.background_executor()
            .spawn(async move {
                VideoComparison::sample(duration, old.as_ref(), new.as_ref(), &cancel, &mut |c| {
                    if let Ok(mut s) = samples.lock() {
                        *s = Some(c.clone());
                    }
                });
            })
            .detach();
        self.poll_progress(cx);
    }

    /// Redraws while analyses run, a few times a second.
    fn poll_progress(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                let Ok(running) = this.update(cx, |this, cx| {
                    let samples = this
                        .samples
                        .lock()
                        .ok()
                        .and_then(|s| s.as_ref().map(|s| (s.done, s.len())));
                    if let Some((done, len)) = samples {
                        this.samples_done = done >= len;
                    }
                    cx.notify();
                    this.sound.pending > 0 || !this.samples_done
                }) else {
                    return;
                };
                if !running {
                    return;
                }
            }
        })
        .detach();
    }

    /// The sound comparison over the whole length, for the timeline.
    fn overview(&mut self) -> Option<Arc<AudioComparison>> {
        if self.sound.pending > 0 || (self.sound.old.is_none() && self.sound.new.is_none()) {
            return None;
        }
        if self.sound.overview.is_none() {
            let cmp = analysis::compare_audio_range(
                self.sound.old.as_deref(),
                self.sound.new.as_deref(),
                800,
                0.,
                self.duration().max(analysis::audio_duration(
                    self.sound.old.as_deref(),
                    self.sound.new.as_deref(),
                )),
            );
            self.sound.overview = Some(Arc::new(cmp));
        }
        self.sound.overview.clone()
    }

    /// The sound lanes' comparison for `columns` over the view.
    fn lanes_comparison(&mut self, columns: usize) -> Option<Arc<AudioComparison>> {
        self.overview()?;
        let (start, end) = self.view_range();
        let key = (columns.max(1), start.to_bits(), end.to_bits());
        if let Some((k, cmp)) = &self.sound.lanes
            && *k == key
        {
            return Some(cmp.clone());
        }
        let cmp = Arc::new(analysis::compare_audio_range(
            self.sound.old.as_deref(),
            self.sound.new.as_deref(),
            key.0,
            start,
            end,
        ));
        self.sound.lanes = Some((key, cmp.clone()));
        Some(cmp)
    }

    fn view_range(&self) -> (f64, f64) {
        self.view.unwrap_or((0., self.duration().max(0.001)))
    }

    /// Where the picture or the sound changed, in time order.
    fn changes(&mut self) -> Vec<(f64, f64)> {
        let mut all: Vec<(f64, f64)> = self
            .samples
            .lock()
            .ok()
            .and_then(|s| s.as_ref().map(|s| s.changes()))
            .unwrap_or_default();
        if let Some(o) = self.overview() {
            all.extend(o.changes.iter().copied());
        }
        all.sort_by(|a, b| a.0.total_cmp(&b.0));
        // merge overlaps
        let mut out: Vec<(f64, f64)> = Vec::new();
        for (a, b) in all {
            match out.last_mut() {
                Some((_, end)) if a <= *end + 0.05 => *end = end.max(b),
                _ => out.push((a, b)),
            }
        }
        out
    }

    // ---- actions --------------------------------------------------------

    fn play_pause(&mut self, cx: &mut Context<Self>) {
        self.player.toggle();
        cx.notify();
    }

    fn seek(&mut self, t: f64, cx: &mut Context<Self>) {
        self.player.seek(t.clamp(0., self.duration()));
        cx.notify();
    }

    fn step(&mut self, frames: i32, cx: &mut Context<Self>) {
        if self.kind() == MediaKind::Video {
            self.player.step(frames);
        } else {
            let t = self.player.position();
            self.player.pause();
            self.player
                .seek((t + frames as f64 * 0.1).clamp(0., self.duration()));
        }
        cx.notify();
    }

    fn jump(&mut self, seconds: f64, cx: &mut Context<Self>) {
        let t = self.player.position() + seconds;
        self.seek(t, cx);
    }

    fn go_to_change(&mut self, forward: bool, cx: &mut Context<Self>) {
        let t = self.player.position();
        let changes = self.changes();
        let target = if forward {
            changes.iter().find(|(a, _)| *a > t + 0.05).map(|c| c.0)
        } else {
            changes
                .iter()
                .rev()
                .find(|(a, _)| *a < t - 0.3)
                .map(|c| c.0)
        };
        if let Some(target) = target {
            self.seek(target, cx);
            self.reveal(target);
        }
    }

    /// Scrolls zoomed lanes so `t` shows.
    fn reveal(&mut self, t: f64) {
        if let Some((a, b)) = self.view
            && (t < a || t > b)
        {
            let w = b - a;
            let start = (t - w * 0.1).clamp(0., (self.duration() - w).max(0.));
            self.view = Some((start, start + w));
        }
    }

    fn toggle_loop(&mut self, cx: &mut Context<Self>) {
        if self.player.loop_range().is_some() {
            self.player.set_loop_range(None);
            self.player.set_looping(false);
        } else {
            let looping = !self.player.looping();
            self.player.set_looping(looping);
        }
        cx.notify();
    }

    fn loop_mark(&mut self, start: bool, cx: &mut Context<Self>) {
        let t = self.player.position();
        let (a, b) = self.player.loop_range().unwrap_or((0., self.duration()));
        let range = if start {
            (t, b.max(t + 0.1))
        } else {
            (a.min(t - 0.1).max(0.), t)
        };
        self.player.set_loop_range(Some(range));
        cx.notify();
    }

    fn listen(&mut self, side: Side, cx: &mut Context<Self>) {
        let available = match side {
            Side::Old => self.diff.previous.is_some(),
            Side::New => self.diff.current.is_some(),
        };
        if available {
            self.player.set_audible(side);
            cx.notify();
        }
    }

    fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        let (volume, muted) = Self::volume(cx);
        self.set_volume(volume, !muted, cx);
    }

    /// The remembered volume and mute.
    fn volume(cx: &App) -> (f32, bool) {
        corvene_core::AppState::try_global(cx).map_or((1., false), |s| {
            let s = &s.read(cx).settings;
            (s.media_volume, s.media_muted)
        })
    }

    fn set_volume(&mut self, volume: f32, muted: bool, cx: &mut Context<Self>) {
        self.player.set_volume(volume);
        self.player.set_muted(muted);
        Dispatcher::set_media_volume(volume, muted, cx);
        cx.notify();
    }

    fn change_speed(&mut self, up: bool, cx: &mut Context<Self>) {
        let now = self.player.speed();
        let i = SPEEDS
            .iter()
            .position(|s| (*s - now).abs() < 1e-6)
            .unwrap_or(3);
        let i = if up {
            (i + 1).min(SPEEDS.len() - 1)
        } else {
            i.saturating_sub(1)
        };
        self.player.set_speed(SPEEDS[i]);
        cx.notify();
    }

    fn set_mode(&mut self, index: usize, cx: &mut Context<Self>) {
        let tabs = self.tabs(cx);
        if let Some((_, mode)) = tabs.get(index) {
            match mode {
                Mode::Video(m) => Dispatcher::set_video_diff_mode(*m, cx),
                Mode::Audio(m) => Dispatcher::set_audio_diff_mode(*m, cx),
            }
            cx.notify();
        }
    }
}

/// A tab of the mode switcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Video(VideoDiffMode),
    Audio(AudioDiffMode),
}

/// `m:ss.mmm`, or `h:mm:ss.mmm` past an hour.
pub(crate) fn format_time(t: f64) -> String {
    let t = t.max(0.);
    let ms = (t * 1000.).round() as u64;
    let (h, m, s, ms) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}.{ms:03}")
    } else {
        format!("{m}:{s:02}.{ms:03}")
    }
}

/// A signed time difference: `+2.0 s`, `−0.5 s`.
fn format_delta(d: f64) -> String {
    let sign = if d >= 0. { "+" } else { "\u{2212}" };
    let d = d.abs();
    if d >= 60. {
        format!("{sign}{}", format_time(d))
    } else {
        format!("{sign}{d:.1} s")
    }
}

impl MediaDiffView {
    fn tabs(&self, cx: &App) -> Vec<(&'static str, Mode)> {
        let _ = cx;
        match (self.kind(), self.compares()) {
            (MediaKind::Video, true) => vec![
                ("2-up", Mode::Video(VideoDiffMode::TwoUp)),
                ("Swipe", Mode::Video(VideoDiffMode::Swipe)),
                ("Onion Skin", Mode::Video(VideoDiffMode::OnionSkin)),
                ("Difference", Mode::Video(VideoDiffMode::Difference)),
                ("Info", Mode::Video(VideoDiffMode::Info)),
            ],
            (MediaKind::Video, false) => vec![
                ("Video", Mode::Video(VideoDiffMode::TwoUp)),
                ("Info", Mode::Video(VideoDiffMode::Info)),
            ],
            (MediaKind::Audio, true) => vec![
                ("Stacked", Mode::Audio(AudioDiffMode::Stacked)),
                ("Overlay", Mode::Audio(AudioDiffMode::Overlay)),
                ("Spectrogram", Mode::Audio(AudioDiffMode::Spectrogram)),
                ("Info", Mode::Audio(AudioDiffMode::Info)),
            ],
            (MediaKind::Audio, false) => vec![
                ("Waveform", Mode::Audio(AudioDiffMode::Stacked)),
                ("Spectrogram", Mode::Audio(AudioDiffMode::Spectrogram)),
                ("Info", Mode::Audio(AudioDiffMode::Info)),
            ],
        }
    }

    /// The chosen tab, mapped onto the tabs this diff has.
    fn mode(&self, cx: &App) -> Mode {
        let settings = corvene_core::AppState::try_global(cx).map(|s| s.read(cx).settings.clone());
        let mode = match self.kind() {
            MediaKind::Video => {
                Mode::Video(settings.map(|s| s.video_diff_mode).unwrap_or_default())
            }
            MediaKind::Audio => {
                Mode::Audio(settings.map(|s| s.audio_diff_mode).unwrap_or_default())
            }
        };
        let tabs = self.tabs(cx);
        if tabs.iter().any(|(_, m)| *m == mode) {
            return mode;
        }
        // one version only: every compare mode shows it
        match mode {
            Mode::Audio(AudioDiffMode::Overlay) => Mode::Audio(AudioDiffMode::Stacked),
            Mode::Video(VideoDiffMode::Info) | Mode::Audio(AudioDiffMode::Info) => mode,
            Mode::Video(_) => Mode::Video(VideoDiffMode::TwoUp),
            m => m,
        }
    }

    /// Takes the players' current pictures; starts the difference blend.
    fn update_frames(&mut self, mode: Mode, cx: &mut Context<Self>) {
        for side in [Side::Old, Side::New] {
            let Some(frame) = self.player.frame(side) else {
                continue;
            };
            let i = side as usize;
            let pixels = Arc::as_ptr(&frame.bgra) as usize;
            if self.shown[i].as_ref().is_some_and(|s| s.pixels == pixels) {
                continue;
            }
            if let Some(shown) = render_image(&frame)
                && let Some(old) = self.shown[i].replace(shown)
            {
                self.retired.push(old.image);
            }
        }
        if mode != Mode::Video(VideoDiffMode::Difference) || self.difference_pending {
            return;
        }
        let (Some(a), Some(b)) = (self.player.frame(Side::Old), self.player.frame(Side::New))
        else {
            return;
        };
        let key = (
            Arc::as_ptr(&a.bgra) as usize,
            Arc::as_ptr(&b.bgra) as usize,
            self.amplify,
        );
        if self.difference.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        self.difference_pending = true;
        let gain = if self.amplify { 4. } else { 1. };
        let task = cx.background_executor().spawn(async move {
            corvene_core::corvene_media::frame::difference_bgra(&a, &b, gain)
        });
        cx.spawn(async move |this, cx| {
            let frame = task.await;
            this.update(cx, |this, cx| {
                this.difference_pending = false;
                if let Some(shown) = render_image(&frame)
                    && let Some((_, old)) = this.difference.replace((key, shown))
                {
                    this.retired.push(old.image);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The changes that tell the two versions apart, as short phrases.
    fn summary_chips(&mut self) -> Vec<(String, bool)> {
        let mut chips = Vec::new();
        let (Some(old), Some(new)) = (&self.diff.previous, &self.diff.current) else {
            return chips;
        };
        let (a, b) = (&old.info, &new.info);
        if let (Some(x), Some(y)) = (a.duration_us, b.duration_us)
            && x.abs_diff(y) >= 50_000
        {
            chips.push((
                format!(
                    "{} \u{2192} {} ({})",
                    format_time(x as f64 / 1e6),
                    format_time(y as f64 / 1e6),
                    format_delta((y as f64 - x as f64) / 1e6)
                ),
                true,
            ));
        }
        if let (Some(x), Some(y)) = (&a.video, &b.video) {
            if x.display_size() != y.display_size() {
                let (w0, h0) = x.display_size();
                let (w1, h1) = y.display_size();
                chips.push((format!("{w0}\u{00d7}{h0} \u{2192} {w1}\u{00d7}{h1}"), true));
            }
            if x.codec != y.codec {
                chips.push((format!("{} \u{2192} {}", x.codec, y.codec), true));
            }
            if let (Some(f0), Some(f1)) = (x.fps(), y.fps())
                && (f0 - f1).abs() > 0.01
            {
                chips.push((
                    format!("{f0:.3} \u{2192} {f1:.3} fps").replace(".000", ""),
                    true,
                ));
            }
        }
        if let (Some(x), Some(y)) = (&a.audio, &b.audio)
            && x.codec != y.codec
        {
            chips.push((format!("{} \u{2192} {}", x.codec, y.codec), true));
        }
        if let (Some(x), Some(y)) = (&a.audio, &b.audio)
            && x.sample_rate != y.sample_rate
        {
            chips.push((
                format!("{} \u{2192} {} Hz", x.sample_rate, y.sample_rate),
                true,
            ));
        }
        let analysed = self.samples_done && self.sound.pending == 0;
        let changes = self.changes();
        if analysed {
            chips.push((
                match changes.len() {
                    0 => "No changes found".to_string(),
                    1 => "1 change".to_string(),
                    n => format!("{n} changes"),
                },
                false,
            ));
        }
        chips
    }

    fn icon_button(
        &self,
        id: &'static str,
        icon: Octicon,
        label: impl Into<SharedString>,
        active: bool,
        enabled: bool,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let t = cx.ghd();
        let color = if !enabled {
            t.text_secondary_muted
        } else if active {
            t.accent
        } else {
            t.text
        };
        div()
            .id(id)
            .flex_none()
            .size(zpx(26.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(BORDER_RADIUS())
            .when(active, |d| d.bg(t.box_selected_background))
            .when(enabled, |d| {
                let hover = t.box_hover_background;
                d.cursor_pointer().hover(move |s| s.bg(hover))
            })
            .icon_button_label(label)
            .child(octicon(icon, color))
    }

    fn mode_tabs(&self, selected: Mode, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd().clone();
        let tabs = self.tabs(cx);
        div()
            .id("media-diff-tabs")
            .flex_none()
            .h(zpx(29.))
            .flex()
            .flex_row()
            .gap(SPACING_HALF())
            .children(tabs.into_iter().enumerate().map(|(ix, (label, mode))| {
                let is_selected = mode == selected;
                div()
                    .id(("media-diff-tab", ix))
                    .px(SPACING())
                    .min_w(zpx(64.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(BORDER_RADIUS())
                    .cursor_pointer()
                    .text_size(FONT_SIZE())
                    .when(is_selected, |d| {
                        d.bg(t.tab_bar_active)
                            .text_color(t.box_selected_active_text)
                    })
                    .when(!is_selected, |d| {
                        let hover = t.tab_bar_hover_background;
                        d.hover(move |s| s.bg(hover))
                    })
                    .ghd_tooltip(format!("{label} ({})", ix + 1))
                    .on_click(move |_, _, cx| match mode {
                        Mode::Video(m) => Dispatcher::set_video_diff_mode(m, cx),
                        Mode::Audio(m) => Dispatcher::set_audio_diff_mode(m, cx),
                    })
                    .child(label)
            }))
            .into_any_element()
    }

    fn header(&mut self, mode: Mode, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.ghd().clone();
        let chips = self.summary_chips();
        let first_change = self.changes().first().map(|c| c.0);
        div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .flex_wrap()
            .gap(SPACING())
            .px(SPACING())
            .pt(SPACING())
            .pb(SPACING_HALF())
            .child(self.mode_tabs(mode, cx))
            .child(div().flex_1())
            .children(chips.into_iter().enumerate().map(|(ix, (text, fact))| {
                let chip = div()
                    .id(("media-chip", ix))
                    .flex_none()
                    .px(zpx(7.))
                    .py(zpx(2.))
                    .rounded(zpx(10.))
                    .border_1()
                    .border_color(t.box_border)
                    .text_size(FONT_SIZE_SM())
                    .text_color(if fact { t.text } else { t.text_secondary })
                    .child(text);
                match (fact, first_change) {
                    (false, Some(at)) => chip
                        .cursor_pointer()
                        .hover(|d| d.bg(t.box_hover_background))
                        .ghd_tooltip("Go to the first change (])")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.seek(at, cx);
                            this.reveal(at);
                        }))
                        .into_any_element(),
                    _ => chip.into_any_element(),
                }
            }))
            .into_any_element()
    }

    fn transport(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.ghd().clone();
        let playing = self.player.is_playing();
        let position = self.player.position();
        let duration = self.duration();
        let video = self.kind() == MediaKind::Video;
        let has_sound = self.player.has_sound();
        let audible = self.player.audible();
        let looping = self.player.looping() || self.player.loop_range().is_some();
        let speed = self.player.speed();
        let (volume, muted) = Self::volume(cx);
        let changes = !self.changes().is_empty();
        let step = if video { "frame" } else { "0.1 s" };
        let listen = |side: Side, label: &'static str, enabled: bool| {
            let selected = audible == side && enabled;
            let color = match side {
                Side::Old => t.color_deleted,
                Side::New => t.color_new,
            };
            div()
                .id(match side {
                    Side::Old => "media-listen-old",
                    Side::New => "media-listen-new",
                })
                .px(SPACING())
                .h_full()
                .flex()
                .items_center()
                .gap(zpx(5.))
                .text_size(FONT_SIZE_SM())
                .when(selected, |d| {
                    d.bg(t.box_selected_background).text_color(t.text)
                })
                .when(!selected, |d| d.text_color(t.text_secondary))
                .when(enabled, |d| {
                    let hover = t.box_hover_background;
                    d.cursor_pointer().hover(move |s| s.bg(hover))
                })
                .when(!enabled, |d| d.opacity(0.5))
                .a11y_button(format!("Hear the {label} version"))
                .ghd_tooltip(match side {
                    Side::Old => "Hear the old version (A)",
                    Side::New => "Hear the new version (B)",
                })
                .child(div().size(zpx(7.)).rounded_full().bg(color))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| this.listen(side, cx)))
        };
        let volume_track = self.volume_track.clone();
        let volume_bar = div()
            .id("media-volume")
            .relative()
            .w(zpx(64.))
            .h(zpx(16.))
            .flex_none()
            .cursor_pointer()
            .child(
                canvas(move |b, _, _| volume_track.set(b), {
                    let (track, fill, knob) = (t.box_border, t.accent, t.text);
                    move |b, _, window, _| {
                        let y = b.center().y;
                        let rail =
                            Bounds::new(point(b.left(), y - px(1.5)), size(b.size.width, px(3.)));
                        window.paint_quad(fill_quad(rail, track, px(1.5)));
                        let v = if muted { 0. } else { volume };
                        let filled = Bounds::new(rail.origin, size(b.size.width * v, px(3.)));
                        window.paint_quad(fill_quad(filled, fill, px(1.5)));
                        let x = b.left() + b.size.width * v;
                        let k = Bounds::new(point(x - px(4.), y - px(4.)), size(px(8.), px(8.)));
                        window.paint_quad(fill_quad(k, knob, px(4.)));
                    }
                })
                .size_full(),
            )
            .ghd_tooltip(format!("Volume {}%", (volume * 100.).round()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    this.volume_drag = true;
                    this.volume_at(ev.position.x, cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if this.volume_drag && ev.pressed_button == Some(MouseButton::Left) {
                    this.volume_at(ev.position.x, cx);
                } else {
                    this.volume_drag = false;
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| this.volume_drag = false),
            );
        div()
            .flex_none()
            .h(zpx(40.))
            .px(SPACING())
            .flex()
            .flex_row()
            .items_center()
            .gap(zpx(2.))
            .border_t_1()
            .border_color(t.box_border)
            .bg(t.box_background)
            .child(
                self.icon_button(
                    "media-prev-change",
                    Octicon::MediaPrevChange,
                    "Previous change ([)",
                    false,
                    changes,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.go_to_change(false, cx))),
            )
            .child(
                self.icon_button(
                    "media-step-back",
                    Octicon::MediaFrameBack,
                    format!("Back one {step} (\u{2190})"),
                    false,
                    true,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.step(-1, cx))),
            )
            .child(
                self.icon_button(
                    "media-play",
                    if playing {
                        Octicon::MediaPause
                    } else {
                        Octicon::MediaPlay
                    },
                    if playing {
                        "Pause (Space)"
                    } else {
                        "Play (Space)"
                    },
                    false,
                    duration > 0.,
                    cx,
                )
                .size(zpx(30.))
                .on_click(cx.listener(|this, _, _, cx| this.play_pause(cx))),
            )
            .child(
                self.icon_button(
                    "media-step-forward",
                    Octicon::MediaFrameForward,
                    format!("Forward one {step} (\u{2192})"),
                    false,
                    true,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.step(1, cx))),
            )
            .child(
                self.icon_button(
                    "media-next-change",
                    Octicon::MediaNextChange,
                    "Next change (])",
                    false,
                    changes,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.go_to_change(true, cx))),
            )
            .child(
                div()
                    .ml(SPACING())
                    .flex_none()
                    .text_size(FONT_SIZE_SM())
                    .font_family("Menlo")
                    .text_color(t.text)
                    .child(format_time(position))
                    .child(
                        div()
                            .text_color(t.text_secondary)
                            .child(format!(" / {}", format_time(duration))),
                    )
                    .flex()
                    .flex_row(),
            )
            .child(div().flex_1())
            .when(has_sound && self.compares(), |d| {
                d.child(
                    div()
                        .flex_none()
                        .h(zpx(24.))
                        .mr(SPACING())
                        .flex()
                        .flex_row()
                        .overflow_hidden()
                        .rounded(BORDER_RADIUS())
                        .border_1()
                        .border_color(t.box_border)
                        .child(listen(Side::Old, "Old", self.diff.previous.is_some()))
                        .child(div().w(px(1.)).h_full().bg(t.box_border))
                        .child(listen(Side::New, "New", self.diff.current.is_some())),
                )
            })
            .child(
                self.icon_button(
                    "media-loop",
                    Octicon::MediaLoop,
                    if self.player.loop_range().is_some() {
                        "Stop looping the range (L); shift-drag the timeline to change it"
                    } else {
                        "Loop (L); shift-drag the timeline to loop a range"
                    },
                    looping,
                    true,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.toggle_loop(cx))),
            )
            .child(
                div()
                    .id("media-speed")
                    .flex_none()
                    .h(zpx(26.))
                    .min_w(zpx(40.))
                    .px(zpx(4.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(BORDER_RADIUS())
                    .text_size(FONT_SIZE_SM())
                    .text_color(if (speed - 1.).abs() > 1e-6 {
                        t.accent
                    } else {
                        t.text
                    })
                    .cursor_pointer()
                    .hover(|d| d.bg(t.box_hover_background))
                    .a11y_button("Playback speed")
                    .ghd_tooltip("Speed: click for faster, shift-click for slower (< / >)")
                    .child(format!("{}\u{00d7}", trim_speed(speed)))
                    .on_click(cx.listener(|this, ev: &ClickEvent, _, cx| {
                        let slower = ev.modifiers().shift;
                        let now = this.player.speed();
                        if !slower && (now - 2.).abs() < 1e-6 {
                            this.player.set_speed(1.);
                            cx.notify();
                        } else {
                            this.change_speed(!slower, cx);
                        }
                    })),
            )
            .when(has_sound, |d| {
                d.child(
                    self.icon_button(
                        "media-mute",
                        if muted || volume == 0. {
                            Octicon::MediaMute
                        } else {
                            Octicon::MediaVolume
                        },
                        if muted { "Unmute (M)" } else { "Mute (M)" },
                        false,
                        true,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_mute(cx))),
                )
                .child(volume_bar)
            })
            .into_any_element()
    }

    fn volume_at(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let b = self.volume_track.get();
        let w = f32::from(b.size.width);
        if w <= 0. {
            return;
        }
        let v = (f32::from(x - b.left()) / w).clamp(0., 1.);
        self.set_volume(v, false, cx);
    }
}

fn trim_speed(s: f64) -> String {
    let text = format!("{s:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A filled rounded rectangle.
pub(crate) fn fill_quad(bounds: Bounds<Pixels>, color: Hsla, radius: Pixels) -> PaintQuad {
    fill(bounds, color).corner_radii(radius)
}

impl Render for MediaDiffView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd().clone();
        self.window = Some(window.window_handle());
        self.scale_factor = window.scale_factor();
        for image in self.retired.drain(..) {
            window.drop_image(image).ok();
        }
        self.player.tick();
        let mode = self.mode(cx);
        if self.kind() == MediaKind::Video {
            self.update_frames(mode, cx);
        }
        if self.player.is_playing() || self.scrub.is_some() {
            window.request_animation_frame();
        }
        let stage = match mode {
            Mode::Video(VideoDiffMode::Info) | Mode::Audio(AudioDiffMode::Info) => {
                info::info(self, cx)
            }
            Mode::Video(m) => video::stage(self, m, cx),
            Mode::Audio(m) => audio::stage(self, m, window, cx),
        };
        let header = self.header(mode, cx);
        let timeline = timeline::timeline(self, cx);
        let transport = self.transport(cx);
        div()
            .id("media-diff")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .flex_1()
            .size_full()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(t.background)
            .text_color(t.text)
            .text_size(FONT_SIZE())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.focus.focus(window, cx)),
            )
            .on_action(cx.listener(|this, _: &PlayPause, _, cx| this.play_pause(cx)))
            .on_action(cx.listener(|this, _: &FrameBack, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &FrameForward, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &SecondBack, _, cx| this.jump(-1., cx)))
            .on_action(cx.listener(|this, _: &SecondForward, _, cx| this.jump(1., cx)))
            .on_action(cx.listener(|this, _: &JumpBack, _, cx| this.jump(-5., cx)))
            .on_action(cx.listener(|this, _: &JumpForward, _, cx| this.jump(5., cx)))
            .on_action(cx.listener(|this, _: &PreviousChange, _, cx| this.go_to_change(false, cx)))
            .on_action(cx.listener(|this, _: &NextChange, _, cx| this.go_to_change(true, cx)))
            .on_action(cx.listener(|this, _: &ToggleLoop, _, cx| this.toggle_loop(cx)))
            .on_action(cx.listener(|this, _: &LoopIn, _, cx| this.loop_mark(true, cx)))
            .on_action(cx.listener(|this, _: &LoopOut, _, cx| this.loop_mark(false, cx)))
            .on_action(cx.listener(|this, _: &GoToStart, _, cx| this.seek(0., cx)))
            .on_action(cx.listener(|this, _: &GoToEnd, _, cx| {
                let end = this.duration();
                this.seek(end, cx)
            }))
            .on_action(cx.listener(|this, _: &ListenOld, _, cx| this.listen(Side::Old, cx)))
            .on_action(cx.listener(|this, _: &ListenNew, _, cx| this.listen(Side::New, cx)))
            .on_action(cx.listener(|this, _: &FlipListen, _, cx| {
                let other = this.player.audible().other();
                this.listen(other, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleMute, _, cx| this.toggle_mute(cx)))
            .on_action(cx.listener(|this, _: &SlowDown, _, cx| this.change_speed(false, cx)))
            .on_action(cx.listener(|this, _: &SpeedUp, _, cx| this.change_speed(true, cx)))
            .on_action(cx.listener(|this, _: &Mode1, _, cx| this.set_mode(0, cx)))
            .on_action(cx.listener(|this, _: &Mode2, _, cx| this.set_mode(1, cx)))
            .on_action(cx.listener(|this, _: &Mode3, _, cx| this.set_mode(2, cx)))
            .on_action(cx.listener(|this, _: &Mode4, _, cx| this.set_mode(3, cx)))
            .on_action(cx.listener(|this, _: &Mode5, _, cx| this.set_mode(4, cx)))
            .child(header)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(stage),
            )
            .child(timeline)
            .child(transport)
    }
}

/// Seconds at `x` in `bounds` showing `range`.
pub(crate) fn time_at(x: Pixels, bounds: Bounds<Pixels>, (start, end): (f64, f64)) -> f64 {
    let w = f32::from(bounds.size.width).max(1.) as f64;
    let f = (f32::from(x - bounds.left()) as f64 / w).clamp(0., 1.);
    start + f * (end - start)
}

/// The x of `time` in `bounds` showing `range`.
pub(crate) fn x_of(time: f64, bounds: Bounds<Pixels>, (start, end): (f64, f64)) -> Pixels {
    let span = (end - start).max(1e-9);
    bounds.left() + bounds.size.width * (((time - start) / span) as f32)
}

/// The message shown in place of a version nothing can decode.
pub(crate) fn undecodable(
    side: Option<&corvene_core::MediaSide>,
    kind: MediaKind,
) -> Option<String> {
    let side = side?;
    let decoder = match kind {
        MediaKind::Video => side.info.video_decoder,
        MediaKind::Audio => side.info.audio_decoder,
    };
    (decoder == MediaDecoder::None).then(|| {
        let ext = side
            .file
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("this")
            .to_ascii_uppercase();
        if corvene_core::corvene_media::ffmpeg_available() {
            format!("This {ext} file could not be decoded.")
        } else {
            format!("Nothing on this system plays {ext} files. Install ffmpeg to preview them.")
        }
    })
}
