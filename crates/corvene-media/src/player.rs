//! Both sides of a media diff played in step.
//!
//! One clock drives both sides: the sound card's count of played frames
//! while there is one, else the wall clock. Each side has a picture thread
//! that keeps a few frames ahead of the clock and a sound thread that keeps
//! half a second of sound ahead, resampled to the card's rate. The card
//! plays one side at a time ([`Player::set_audible`]); switching fades over
//! 10 ms so A/B listening does not click. Seeking bumps a generation that
//! restarts every reader at the new time.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::frame::Frame;
use crate::{AudioReader, Source, VideoReader};

/// Which side of the diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Old = 0,
    New = 1,
}

impl Side {
    pub fn other(self) -> Self {
        match self {
            Side::Old => Side::New,
            Side::New => Side::Old,
        }
    }
}

/// Pictures decoded ahead of the clock, a side.
const FRAMES_AHEAD: usize = 6;
/// Seconds of sound decoded ahead of the clock.
const SOUND_AHEAD: f64 = 0.5;
/// The longer side of a played picture is at most this.
pub const MAX_FRAME_SIDE: u32 = 1920;

struct Clock {
    playing: bool,
    /// The media time at `since` / `played_at`.
    anchor: f64,
    since: Instant,
    played_at: u64,
    speed: f64,
    generation: u64,
    frame_box: (u32, u32),
    loop_range: Option<(f64, f64)>,
    looping: bool,
}

#[derive(Default)]
struct VideoQueue {
    generation: u64,
    frames: VecDeque<Frame>,
    /// The picture on screen, kept while a seek waits for its first frame.
    shown: Option<Frame>,
    ended: bool,
}

#[derive(Default)]
struct SoundQueue {
    generation: u64,
    /// Stereo frames at the card's rate; the first plays at `front`
    /// (frames since the clock's anchor).
    frames: VecDeque<[f32; 2]>,
    front: u64,
}

struct Shared {
    quit: AtomicBool,
    clock: Mutex<Clock>,
    generation: AtomicU64,
    wake: Condvar,
    wake_lock: Mutex<()>,
    sources: [Option<Source>; 2],
    video: [Mutex<VideoQueue>; 2],
    sound: [Mutex<SoundQueue>; 2],
    audible: AtomicUsize,
    volume: AtomicU32,
    muted: AtomicBool,
    /// The card's rate, 0 without a card.
    rate: AtomicU32,
    /// Frames the card played while playing.
    played: AtomicU64,
    playing: AtomicBool,
    duration: f64,
}

impl Shared {
    fn notify(&self) {
        let _g = self.wake_lock.lock();
        self.wake.notify_all();
    }

    fn wait(&self, ms: u64) {
        if let Ok(g) = self.wake_lock.lock() {
            self.wake.wait_timeout(g, Duration::from_millis(ms)).ok();
        }
    }

    fn position(&self, clock: &Clock) -> f64 {
        if !clock.playing {
            return clock.anchor;
        }
        let rate = self.rate.load(Ordering::Relaxed);
        let elapsed = if rate > 0 {
            self.played
                .load(Ordering::Relaxed)
                .saturating_sub(clock.played_at) as f64
                / rate as f64
        } else {
            clock.since.elapsed().as_secs_f64()
        };
        clock.anchor + elapsed * clock.speed
    }

    /// Re-anchors the clock at `time` and restarts the readers there.
    fn seek_locked(&self, clock: &mut Clock, time: f64) {
        clock.anchor = time.clamp(0., self.duration.max(0.));
        clock.since = Instant::now();
        clock.played_at = self.played.load(Ordering::Relaxed);
        clock.generation += 1;
        self.generation.store(clock.generation, Ordering::Release);
        for q in &self.sound {
            if let Ok(mut q) = q.lock() {
                q.frames.clear();
                q.front = 0;
            }
        }
        self.notify();
    }
}

pub struct Player {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
    output: Option<output::Output>,
    output_tried: bool,
}

impl Player {
    /// A player for the sides; nothing is decoded until a frame is asked
    /// for or it plays.
    pub fn new(old: Option<Source>, new: Option<Source>) -> Self {
        let duration = [&old, &new]
            .into_iter()
            .flatten()
            .map(|s| s.duration())
            .fold(0., f64::max);
        let shared = Arc::new(Shared {
            quit: AtomicBool::new(false),
            clock: Mutex::new(Clock {
                playing: false,
                anchor: 0.,
                since: Instant::now(),
                played_at: 0,
                speed: 1.,
                generation: 1,
                frame_box: (960, 540),
                loop_range: None,
                looping: false,
            }),
            generation: AtomicU64::new(1),
            wake: Condvar::new(),
            wake_lock: Mutex::new(()),
            sources: [old, new],
            video: Default::default(),
            sound: Default::default(),
            audible: AtomicUsize::new(Side::New as usize),
            volume: AtomicU32::new(1f32.to_bits()),
            muted: AtomicBool::new(false),
            rate: AtomicU32::new(0),
            played: AtomicU64::new(0),
            playing: AtomicBool::new(false),
            duration,
        });
        let mut threads = Vec::new();
        for side in [Side::Old, Side::New] {
            let Some(source) = &shared.sources[side as usize] else {
                continue;
            };
            if source.has_video() {
                let s = shared.clone();
                threads.extend(
                    std::thread::Builder::new()
                        .name("corvene-media-video".into())
                        .spawn(move || video_thread(s, side))
                        .ok(),
                );
            }
            if source.has_audio() {
                let s = shared.clone();
                threads.extend(
                    std::thread::Builder::new()
                        .name("corvene-media-sound".into())
                        .spawn(move || sound_thread(s, side))
                        .ok(),
                );
            }
        }
        Self {
            shared,
            threads,
            output: None,
            output_tried: false,
        }
    }

    pub fn duration(&self) -> f64 {
        self.shared.duration
    }

    pub fn source(&self, side: Side) -> Option<&Source> {
        self.shared.sources[side as usize].as_ref()
    }

    pub fn is_playing(&self) -> bool {
        self.shared.clock.lock().is_ok_and(|c| c.playing)
    }

    pub fn position(&self) -> f64 {
        self.shared
            .clock
            .lock()
            .map_or(0., |c| self.shared.position(&c))
    }

    /// The clock now, after looping or stopping at the end; call it every
    /// frame while playing.
    pub fn tick(&self) -> f64 {
        let Ok(mut c) = self.shared.clock.lock() else {
            return 0.;
        };
        let t = self.shared.position(&c);
        if !c.playing {
            return t;
        }
        if let Some((a, b)) = c.loop_range
            && t >= b
        {
            self.shared.seek_locked(&mut c, a);
            return a;
        }
        if t >= self.shared.duration {
            if c.looping {
                self.shared.seek_locked(&mut c, 0.);
                return 0.;
            }
            c.playing = false;
            self.shared.playing.store(false, Ordering::Relaxed);
            let end = self.shared.duration;
            c.anchor = end;
            return end;
        }
        t
    }

    pub fn play(&mut self) {
        if !self.output_tried && self.has_sound() {
            self.output_tried = true;
            self.output = output::Output::open(self.shared.clone());
        }
        let Ok(mut c) = self.shared.clock.lock() else {
            return;
        };
        if c.playing {
            return;
        }
        let mut at = c.anchor;
        if at >= self.shared.duration - 0.01 {
            at = c.loop_range.map_or(0., |r| r.0);
        }
        c.playing = true;
        self.shared.playing.store(true, Ordering::Relaxed);
        // the readers already wait at `at` when nothing moved
        if (at - c.anchor).abs() > 1e-6 {
            self.shared.seek_locked(&mut c, at);
        } else {
            c.since = Instant::now();
            c.played_at = self.shared.played.load(Ordering::Relaxed);
        }
    }

    pub fn pause(&mut self) {
        let Ok(mut c) = self.shared.clock.lock() else {
            return;
        };
        if !c.playing {
            return;
        }
        let t = self.shared.position(&c);
        c.playing = false;
        self.shared.playing.store(false, Ordering::Relaxed);
        // shows exactly the picture at the paused time
        self.shared.seek_locked(&mut c, t);
    }

    pub fn toggle(&mut self) {
        if self.is_playing() {
            self.pause()
        } else {
            self.play()
        }
    }

    pub fn seek(&self, time: f64) {
        if let Ok(mut c) = self.shared.clock.lock() {
            self.shared.seek_locked(&mut c, time);
        }
    }

    /// Moves `frames` pictures (of the old side's rate, or 30 a second) and
    /// pauses.
    pub fn step(&mut self, frames: i32) {
        self.pause();
        let fps = [Side::Old, Side::New]
            .iter()
            .filter_map(|s| self.source(*s)?.info.video.as_ref()?.fps())
            .next()
            .unwrap_or(30.);
        let t = self.position();
        // land in the middle of the frame, past rounding
        let index = (t * fps).floor() + frames as f64;
        self.seek(((index + 0.5) / fps).max(0.));
    }

    pub fn speed(&self) -> f64 {
        self.shared.clock.lock().map_or(1., |c| c.speed)
    }

    pub fn set_speed(&self, speed: f64) {
        if let Ok(mut c) = self.shared.clock.lock() {
            let t = self.shared.position(&c);
            c.speed = speed.clamp(0.25, 4.);
            self.shared.seek_locked(&mut c, t);
        }
    }

    pub fn audible(&self) -> Side {
        if self.shared.audible.load(Ordering::Relaxed) == 0 {
            Side::Old
        } else {
            Side::New
        }
    }

    pub fn set_audible(&self, side: Side) {
        self.shared.audible.store(side as usize, Ordering::Relaxed);
    }

    pub fn set_volume(&self, volume: f32) {
        self.shared
            .volume
            .store(volume.clamp(0., 1.).to_bits(), Ordering::Relaxed);
    }

    pub fn set_muted(&self, muted: bool) {
        self.shared.muted.store(muted, Ordering::Relaxed);
    }

    pub fn loop_range(&self) -> Option<(f64, f64)> {
        self.shared.clock.lock().ok()?.loop_range
    }

    /// Plays `range` over and over (`None` for all of it).
    pub fn set_loop_range(&self, range: Option<(f64, f64)>) {
        if let Ok(mut c) = self.shared.clock.lock() {
            c.loop_range = range.filter(|(a, b)| b - a > 0.05);
        }
    }

    pub fn looping(&self) -> bool {
        self.shared.clock.lock().is_ok_and(|c| c.looping)
    }

    pub fn set_looping(&self, looping: bool) {
        if let Ok(mut c) = self.shared.clock.lock() {
            c.looping = looping;
        }
    }

    /// The device pixels a picture may cover; the readers restart at the
    /// new size when it grows or halves.
    pub fn set_frame_box(&self, (w, h): (u32, u32)) {
        let k = (MAX_FRAME_SIDE as f32 / w.max(h).max(1) as f32).min(1.);
        let want = (
            ((w as f32 * k) as u32).max(16),
            ((h as f32 * k) as u32).max(16),
        );
        if let Ok(mut c) = self.shared.clock.lock() {
            let (ow, oh) = c.frame_box;
            let bigger = want.0 > ow + ow / 8 || want.1 > oh + oh / 8;
            let much_smaller = want.0 * 2 < ow && want.1 * 2 < oh;
            if bigger || much_smaller {
                c.frame_box = want;
                let t = self.shared.position(&c);
                self.shared.seek_locked(&mut c, t);
            }
        }
    }

    /// The picture of `side` showing now (or the last one shown while a
    /// seek is under way).
    pub fn frame(&self, side: Side) -> Option<Frame> {
        let t = self.position();
        let generation = self.shared.generation.load(Ordering::Acquire);
        let mut q = self.shared.video[side as usize].lock().ok()?;
        if q.generation == generation {
            while q.frames.len() > 1 && q.frames[1].time <= t + 0.002 {
                q.frames.pop_front();
            }
            if let Some(front) = q.frames.front().cloned()
                && (front.time <= t + 0.05 || q.shown.is_none())
            {
                q.shown = Some(front);
            }
        }
        let shown = q.shown.clone();
        drop(q);
        self.shared.notify();
        shown
    }

    /// Whether `side`'s pictures ran out before the clock.
    pub fn ended(&self, side: Side) -> bool {
        let t = self.position();
        self.source(side)
            .is_some_and(|s| s.duration() > 0. && t >= s.duration() - 0.001)
            || self.shared.video[side as usize]
                .lock()
                .is_ok_and(|q| q.ended && q.frames.len() <= 1)
    }

    pub fn has_sound(&self) -> bool {
        self.shared.sources.iter().flatten().any(|s| s.has_audio())
    }

    /// Whether sound can be heard (a card opened); `None` before the first
    /// play.
    pub fn can_play_sound(&self) -> Option<bool> {
        self.output_tried.then(|| self.output.is_some())
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shared.quit.store(true, Ordering::Relaxed);
        self.shared.notify();
        self.output.take();
        // the readers notice within one frame; do not wait for them here
        self.threads.clear();
    }
}

fn video_thread(shared: Arc<Shared>, side: Side) {
    let Some(source) = shared.sources[side as usize].clone() else {
        return;
    };
    let mut reader: Option<Box<dyn VideoReader>> = None;
    let mut mine = 0;
    while !shared.quit.load(Ordering::Relaxed) {
        let generation = shared.generation.load(Ordering::Acquire);
        if generation != mine {
            let (anchor, frame_box) = match shared.clock.lock() {
                Ok(c) => (c.anchor, c.frame_box),
                Err(_) => return,
            };
            mine = generation;
            reader = None;
            if let Ok(mut q) = shared.video[side as usize].lock() {
                q.generation = generation;
                q.frames.clear();
                q.ended = false;
            }
            let end = source.duration();
            let at = if end > 0. {
                anchor.min((end - 0.001).max(0.))
            } else {
                anchor
            };
            match source.video_from(at, frame_box) {
                Ok(r) => reader = Some(r),
                Err(err) => {
                    tracing::debug!(%err, "video reader");
                    mark_ended(&shared, side, generation);
                }
            }
        }
        let full = shared.video[side as usize]
            .lock()
            .map_or(true, |q| q.frames.len() >= FRAMES_AHEAD || q.ended);
        let Some(r) = reader.as_mut().filter(|_| !full) else {
            shared.wait(8);
            continue;
        };
        match r.next_frame() {
            Ok(Some(frame)) => {
                if let Ok(mut q) = shared.video[side as usize].lock()
                    && q.generation == mine
                    && shared.generation.load(Ordering::Acquire) == mine
                {
                    q.frames.push_back(frame);
                }
            }
            Ok(None) => mark_ended(&shared, side, mine),
            Err(err) => {
                tracing::debug!(%err, "video frame");
                mark_ended(&shared, side, mine);
            }
        }
    }
}

fn mark_ended(shared: &Shared, side: Side, generation: u64) {
    if let Ok(mut q) = shared.video[side as usize].lock()
        && q.generation == generation
    {
        q.ended = true;
    }
}

/// Linear resampling of a chunked stream to the card's rate (`step` source
/// frames an output frame, the speed included).
struct Resampler {
    step: f64,
    pos: f64,
    pending: Vec<[f32; 2]>,
}

impl Resampler {
    fn push(&mut self, chunk: &crate::AudioChunk, out: &mut VecDeque<[f32; 2]>) {
        let channels = chunk.channels.max(1) as usize;
        self.pending.extend(
            chunk
                .samples
                .chunks_exact(channels)
                .map(|f| match channels {
                    1 => [f[0], f[0]],
                    _ => [f[0], f[1]],
                }),
        );
        while self.pos + 1. < self.pending.len() as f64 {
            let i = self.pos as usize;
            let frac = (self.pos - i as f64) as f32;
            let (a, b) = (self.pending[i], self.pending[i + 1]);
            out.push_back([a[0] + (b[0] - a[0]) * frac, a[1] + (b[1] - a[1]) * frac]);
            self.pos += self.step;
        }
        let used = (self.pos as usize).min(self.pending.len());
        self.pending.drain(..used);
        self.pos -= used as f64;
    }
}

fn sound_thread(shared: Arc<Shared>, side: Side) {
    let Some(source) = shared.sources[side as usize].clone() else {
        return;
    };
    let mut reader: Option<Box<dyn AudioReader>> = None;
    let mut resampler: Option<Resampler> = None;
    let mut mine = 0;
    let mut anchor = 0.;
    let mut speed = 1.;
    while !shared.quit.load(Ordering::Relaxed) {
        let rate = shared.rate.load(Ordering::Relaxed);
        if rate == 0 {
            // no card (yet): nothing to feed
            shared.wait(50);
            continue;
        }
        let generation = shared.generation.load(Ordering::Acquire);
        if generation != mine {
            match shared.clock.lock() {
                Ok(c) => {
                    anchor = c.anchor;
                    speed = c.speed;
                }
                Err(_) => return,
            }
            mine = generation;
            resampler = None;
            if let Ok(mut q) = shared.sound[side as usize].lock() {
                q.generation = generation;
                q.frames.clear();
                q.front = 0;
            }
            reader = if anchor < source.duration() || source.duration() <= 0. {
                source
                    .audio_from(anchor)
                    .inspect_err(|err| tracing::debug!(%err, "sound reader"))
                    .ok()
            } else {
                None
            };
        }
        let ahead = (rate as f64 * SOUND_AHEAD) as usize;
        let full = shared.sound[side as usize]
            .lock()
            .map_or(true, |q| q.frames.len() >= ahead);
        let Some(r) = reader.as_mut().filter(|_| !full) else {
            shared.wait(10);
            continue;
        };
        match r.next_chunk() {
            Ok(Some(chunk)) => {
                let Ok(mut q) = shared.sound[side as usize].lock() else {
                    return;
                };
                if q.generation != mine || shared.generation.load(Ordering::Acquire) != mine {
                    continue;
                }
                let resampler = resampler.get_or_insert_with(|| {
                    // sound that starts after the anchor: silence first
                    let gap = ((chunk.time - anchor) / speed * rate as f64).round();
                    if gap > 0. {
                        q.frames.extend(std::iter::repeat_n([0.; 2], gap as usize));
                    }
                    Resampler {
                        step: chunk.rate as f64 * speed / rate as f64,
                        pos: 0.,
                        pending: Vec::new(),
                    }
                });
                resampler.push(&chunk, &mut q.frames);
            }
            Ok(None) => reader = None,
            Err(err) => {
                tracing::debug!(%err, "sound chunk");
                reader = None;
            }
        }
    }
}

#[cfg(not(target_os = "android"))]
mod output {
    use super::*;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    /// The sound card stream, owned by its own thread (a stream is not
    /// `Send` everywhere); dropped to stop it.
    pub struct Output {
        stop: std::sync::mpsc::Sender<()>,
    }

    impl Output {
        pub fn open(shared: Arc<Shared>) -> Option<Self> {
            let (stop, stopped) = std::sync::mpsc::channel::<()>();
            let (ready, opened) = std::sync::mpsc::channel::<bool>();
            std::thread::Builder::new()
                .name("corvene-media-output".into())
                .spawn(move || {
                    let stream = match build(shared.clone()) {
                        Ok(stream) => stream,
                        Err(err) => {
                            tracing::warn!(%err, "no sound output");
                            ready.send(false).ok();
                            return;
                        }
                    };
                    if let Err(err) = stream.play() {
                        tracing::warn!(%err, "sound output did not start");
                        shared.rate.store(0, Ordering::Relaxed);
                        ready.send(false).ok();
                        return;
                    }
                    ready.send(true).ok();
                    stopped.recv().ok();
                    drop(stream);
                    shared.rate.store(0, Ordering::Relaxed);
                })
                .ok()?;
            opened
                .recv_timeout(Duration::from_secs(3))
                .unwrap_or(false)
                .then_some(Self { stop })
        }
    }

    impl Drop for Output {
        fn drop(&mut self) {
            self.stop.send(()).ok();
        }
    }

    fn build(shared: Arc<Shared>) -> Result<cpal::Stream, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "no output device".to_string())?;
        let config = device.default_output_config().map_err(|e| e.to_string())?;
        let format = config.sample_format();
        let config = config.config();
        let channels = config.channels.max(1) as usize;
        shared.rate.store(config.sample_rate, Ordering::Relaxed);
        let err = |e: cpal::Error| tracing::warn!(%e, "sound output");
        let mut mixer = Mixer::new(shared, config.sample_rate);
        match format {
            cpal::SampleFormat::F32 => device.build_output_stream::<f32, _, _>(
                config,
                move |data, _| mixer.fill(data, channels, |v| v),
                err,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_output_stream::<i16, _, _>(
                config,
                move |data, _| mixer.fill(data, channels, |v| (v * i16::MAX as f32) as i16),
                err,
                None,
            ),
            cpal::SampleFormat::I32 => device.build_output_stream::<i32, _, _>(
                config,
                move |data, _| mixer.fill(data, channels, |v| (v as f64 * i32::MAX as f64) as i32),
                err,
                None,
            ),
            other => return Err(format!("unsupported sample format {other:?}")),
        }
        .map_err(|e| e.to_string())
    }

    struct Mixer {
        shared: Arc<Shared>,
        /// The new side's share of the mix, moving towards the audible side.
        new_gain: f32,
        fade_step: f32,
    }

    impl Mixer {
        fn new(shared: Arc<Shared>, rate: u32) -> Self {
            let new_gain = shared.audible.load(Ordering::Relaxed) as f32;
            Self {
                shared,
                new_gain,
                fade_step: 1. / (0.01 * rate.max(1) as f32),
            }
        }

        fn fill<T: Copy>(&mut self, data: &mut [T], channels: usize, convert: impl Fn(f32) -> T) {
            let s = &self.shared;
            let silence = convert(0.);
            if !s.playing.load(Ordering::Relaxed) {
                data.fill(silence);
                return;
            }
            let generation = s.generation.load(Ordering::Acquire);
            let played_at = s.clock.try_lock().ok().map(|c| c.played_at);
            let target = s.audible.load(Ordering::Relaxed) as f32;
            let volume = if s.muted.load(Ordering::Relaxed) {
                0.
            } else {
                f32::from_bits(s.volume.load(Ordering::Relaxed))
            };
            let mut queues = [s.sound[0].try_lock().ok(), s.sound[1].try_lock().ok()];
            for frame in data.chunks_mut(channels) {
                let k = s.played.fetch_add(1, Ordering::Relaxed);
                let mut mixed = [[0f32; 2]; 2];
                if let Some(played_at) = played_at {
                    let index = k.saturating_sub(played_at);
                    for (side, q) in queues.iter_mut().enumerate() {
                        let Some(q) = q.as_mut().filter(|q| q.generation == generation) else {
                            continue;
                        };
                        // drop what the card already passed
                        while q.front < index && !q.frames.is_empty() {
                            q.frames.pop_front();
                            q.front += 1;
                        }
                        if q.front == index
                            && let Some(f) = q.frames.pop_front()
                        {
                            mixed[side] = f;
                            q.front += 1;
                        }
                    }
                }
                if self.new_gain < target {
                    self.new_gain = (self.new_gain + self.fade_step).min(target);
                } else if self.new_gain > target {
                    self.new_gain = (self.new_gain - self.fade_step).max(target);
                }
                let g = self.new_gain;
                let l = (mixed[0][0] * (1. - g) + mixed[1][0] * g) * volume;
                let r = (mixed[0][1] * (1. - g) + mixed[1][1] * g) * volume;
                match channels {
                    1 => frame[0] = convert(((l + r) / 2.).clamp(-1., 1.)),
                    _ => {
                        frame[0] = convert(l.clamp(-1., 1.));
                        frame[1] = convert(r.clamp(-1., 1.));
                        for v in &mut frame[2..] {
                            *v = silence;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(target_os = "android")]
mod output {
    use super::*;

    /// No sound output on Android (the app plays nothing).
    pub struct Output;

    impl Output {
        pub fn open(_shared: Arc<Shared>) -> Option<Self> {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_keeps_time() {
        let mut r = Resampler {
            step: 0.5,
            pos: 0.,
            pending: Vec::new(),
        };
        let mut out = VecDeque::new();
        let chunk = crate::AudioChunk {
            time: 0.,
            rate: 1,
            channels: 1,
            samples: vec![0., 1., 2., 3.],
        };
        r.push(&chunk, &mut out);
        let got: Vec<f32> = out.iter().map(|f| f[0]).collect();
        assert_eq!(got, vec![0., 0.5, 1., 1.5, 2., 2.5]);
        // the next chunk continues from where this one stopped
        out.clear();
        r.push(
            &crate::AudioChunk {
                samples: vec![4.],
                ..chunk
            },
            &mut out,
        );
        let got: Vec<f32> = out.iter().map(|f| f[0]).collect();
        assert_eq!(got, vec![3., 3.5]);
    }

    #[test]
    fn a_silent_player_keeps_wall_time() {
        let info = corvene_models::MediaInfo {
            duration_us: Some(10_000_000),
            ..Default::default()
        };
        let mut p = Player::new(None, Some(Source::new("/nonexistent.mp4", info)));
        assert_eq!(p.duration(), 10.);
        p.seek(4.);
        assert_eq!(p.position(), 4.);
        p.set_loop_range(Some((4., 4.2)));
        p.play();
        std::thread::sleep(Duration::from_millis(300));
        // looped back
        let t = p.tick();
        assert!((4.0..4.2).contains(&t), "{t}");
        p.pause();
        p.step(1);
        assert!(!p.is_playing());
    }
}
