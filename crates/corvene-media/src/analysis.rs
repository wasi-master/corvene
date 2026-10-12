//! What changed between two versions of a sound or a video, for the
//! timeline's change marks, the waveforms and the spectrograms.
//!
//! Sound is cut into blocks of [`BLOCK`] frames (mono); each block keeps
//! its peaks, its loudness and its spectrum in [`BANDS`] bands. Blocks are
//! gathered into the columns the view draws, on a time axis both sides
//! share, and two columns differ when their spectra or loudness do (a
//! spectral comparison does not mind a sample of delay or a re-encode).
//!
//! Video is sampled: a small picture of both sides at the same times,
//! compared by their mean luma difference.

use std::collections::VecDeque;
use std::f32::consts::PI;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::frame::{Frame, picture_distance};
use crate::{Result, Source};

/// Frames per block, and the FFT's size.
pub const BLOCK: usize = 1024;
/// Spectrum bands, low to high, log-spaced from [`LOW_HZ`].
pub const BANDS: usize = 96;
pub const LOW_HZ: f32 = 30.;
/// The spectrum's floor, in dB below full scale (byte 0).
const FLOOR_DB: f32 = 96.;

/// One block or one column of sound.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Level {
    pub min: f32,
    pub max: f32,
    /// Root mean square.
    pub rms: f32,
}

/// A whole sound, block by block.
#[derive(Clone, Debug, Default)]
pub struct AudioSummary {
    pub rate: u32,
    /// Seconds a block covers.
    pub block_seconds: f64,
    pub levels: Vec<Level>,
    /// `BANDS` bytes a block: 0 = `FLOOR_DB` below full scale, 255 = full.
    pub spectrum: Vec<u8>,
}

impl AudioSummary {
    pub fn duration(&self) -> f64 {
        self.levels.len() as f64 * self.block_seconds
    }

    fn blocks_in(&self, from: f64, to: f64) -> std::ops::Range<usize> {
        let n = self.levels.len();
        if n == 0 || self.block_seconds <= 0. {
            return 0..0;
        }
        let a = ((from / self.block_seconds).floor() as usize).min(n);
        let b = ((to / self.block_seconds).floor() as usize)
            .max(a + 1)
            .min(n);
        a..b
    }

    /// The columns of `[start, start + columns × seconds)`; `None` past
    /// the end.
    pub fn columns(
        &self,
        start: f64,
        columns: usize,
        seconds: f64,
    ) -> Vec<Option<(Level, [u8; BANDS])>> {
        (0..columns)
            .map(|c| {
                let range =
                    self.blocks_in(start + c as f64 * seconds, start + (c + 1) as f64 * seconds);
                if range.is_empty() {
                    return None;
                }
                let mut level = Level {
                    min: f32::MAX,
                    max: f32::MIN,
                    rms: 0.,
                };
                let mut bands = [0u32; BANDS];
                for b in range.clone() {
                    let l = self.levels[b];
                    level.min = level.min.min(l.min);
                    level.max = level.max.max(l.max);
                    level.rms += l.rms * l.rms;
                    for (i, v) in self.spectrum[b * BANDS..(b + 1) * BANDS].iter().enumerate() {
                        bands[i] += *v as u32;
                    }
                }
                let n = range.len() as u32;
                level.rms = (level.rms / n as f32).sqrt();
                let mut out = [0u8; BANDS];
                for (o, v) in out.iter_mut().zip(bands) {
                    *o = (v / n) as u8;
                }
                Some((level, out))
            })
            .collect()
    }
}

/// Radix-2 FFT of [`BLOCK`] points with a Hann window, magnitudes only.
struct Fft {
    window: Vec<f32>,
    twiddles: Vec<(f32, f32)>,
    bits: u32,
    /// For each band, the FFT bins it averages.
    bands: Vec<std::ops::Range<usize>>,
}

impl Fft {
    fn new(rate: u32) -> Self {
        let n = BLOCK;
        let window = (0..n)
            .map(|i| 0.5 - 0.5 * (2. * PI * i as f32 / n as f32).cos())
            .collect();
        let twiddles = (0..n / 2)
            .map(|k| {
                let a = -2. * PI * k as f32 / n as f32;
                (a.cos(), a.sin())
            })
            .collect();
        let nyquist = rate as f32 / 2.;
        let hz_per_bin = rate as f32 / n as f32;
        let low = LOW_HZ.min(nyquist / 2.);
        let bands = (0..BANDS)
            .map(|b| {
                let f0 = low * (nyquist / low).powf(b as f32 / BANDS as f32);
                let f1 = low * (nyquist / low).powf((b + 1) as f32 / BANDS as f32);
                let a = ((f0 / hz_per_bin).floor() as usize).clamp(1, n / 2 - 1);
                let e = ((f1 / hz_per_bin).ceil() as usize).clamp(a + 1, n / 2);
                a..e
            })
            .collect();
        Self {
            window,
            twiddles,
            bits: n.trailing_zeros(),
            bands,
        }
    }

    fn spectrum(&self, samples: &[f32], out: &mut [u8]) {
        let n = BLOCK;
        let mut re = vec![0f32; n];
        let mut im = vec![0f32; n];
        for i in 0..n {
            let j = (i as u32).reverse_bits() >> (32 - self.bits);
            re[j as usize] = samples.get(i).copied().unwrap_or(0.) * self.window[i];
        }
        let mut size = 2;
        while size <= n {
            let half = size / 2;
            let step = n / size;
            for start in (0..n).step_by(size) {
                for k in 0..half {
                    let (wr, wi) = self.twiddles[k * step];
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            size *= 2;
        }
        // a full-scale sine peaks at n/4 with the Hann window
        let scale = 4. / n as f32;
        for (o, range) in out.iter_mut().zip(&self.bands) {
            let peak = range
                .clone()
                .map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt())
                .fold(0f32, f32::max)
                * scale;
            let db = 20. * peak.max(1e-9).log10();
            *o = (((db + FLOOR_DB) / FLOOR_DB).clamp(0., 1.) * 255.).round() as u8;
        }
    }
}

/// Reads all of `source`'s sound (mixed to mono) into a summary. Stops
/// with `Ok(None)` once `cancel` is set; `progress` gets 0..1.
pub fn summarize_audio(
    source: &Source,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<Option<AudioSummary>> {
    let mut reader = source.audio_from(0.)?;
    let duration = source.duration();
    let mut summary = AudioSummary::default();
    let mut fft: Option<Fft> = None;
    let mut block: Vec<f32> = Vec::with_capacity(BLOCK);
    let mut spectrum = [0u8; BANDS];
    let mut flush = |block: &mut Vec<f32>, summary: &mut AudioSummary, fft: &Fft| {
        if block.is_empty() {
            return;
        }
        let (mut min, mut max, mut sq) = (f32::MAX, f32::MIN, 0f32);
        for &s in block.iter() {
            min = min.min(s);
            max = max.max(s);
            sq += s * s;
        }
        let rms = (sq / block.len() as f32).sqrt();
        fft.spectrum(block, &mut spectrum);
        summary.levels.push(Level { min, max, rms });
        summary.spectrum.extend_from_slice(&spectrum);
        block.clear();
    };
    let mut last_report = 0.;
    while let Some(chunk) = reader.next_chunk()? {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        if summary.rate == 0 {
            summary.rate = chunk.rate;
            summary.block_seconds = BLOCK as f64 / chunk.rate.max(1) as f64;
            fft = Some(Fft::new(chunk.rate));
        }
        let Some(fft) = fft.as_ref() else { continue };
        let channels = chunk.channels.max(1) as usize;
        for frame in chunk.samples.chunks_exact(channels) {
            block.push(frame.iter().sum::<f32>() / channels as f32);
            if block.len() == BLOCK {
                flush(&mut block, &mut summary, fft);
            }
        }
        if duration > 0. {
            let done = (summary.duration() / duration) as f32;
            if done - last_report >= 0.02 {
                last_report = done;
                progress(done.min(1.));
            }
        }
    }
    if let Some(fft) = fft.as_ref() {
        flush(&mut block, &mut summary, fft);
    }
    progress(1.);
    Ok(Some(summary))
}

/// A comparison of two sounds' columns on a shared time axis.
#[derive(Clone, Debug, Default)]
pub struct AudioComparison {
    /// Where the first column starts, in seconds.
    pub start: f64,
    pub columns: usize,
    pub seconds_per_column: f64,
    pub old: Vec<Option<(Level, [u8; BANDS])>>,
    pub new: Vec<Option<(Level, [u8; BANDS])>>,
    /// 0 (same) to 1, a column.
    pub difference: Vec<f32>,
    /// Stretches of time that changed, in seconds.
    pub changes: Vec<(f64, f64)>,
}

/// A column differs from the other side's by more than this.
const AUDIO_CHANGED: f32 = 0.04;
/// Silence (an RMS under -60 dBFS) is the same as nothing.
const SILENT: f32 = 0.001;

/// The longer of two sounds, in seconds.
pub fn audio_duration(old: Option<&AudioSummary>, new: Option<&AudioSummary>) -> f64 {
    old.map_or(0., |s| s.duration())
        .max(new.map_or(0., |s| s.duration()))
}

/// Both sounds in `columns` columns from start to end of the longer one.
pub fn compare_audio(
    old: Option<&AudioSummary>,
    new: Option<&AudioSummary>,
    columns: usize,
) -> AudioComparison {
    compare_audio_range(old, new, columns, 0., audio_duration(old, new))
}

/// Both sounds in `columns` columns over `start..end` seconds; the changes
/// are those of that stretch.
pub fn compare_audio_range(
    old: Option<&AudioSummary>,
    new: Option<&AudioSummary>,
    columns: usize,
    start: f64,
    end: f64,
) -> AudioComparison {
    let columns = columns.max(1);
    let seconds = ((end - start) / columns as f64).max(1e-6);
    let cols = |s: Option<&AudioSummary>| match s {
        Some(s) => s.columns(start, columns, seconds),
        None => vec![None; columns],
    };
    let (a, b) = (cols(old), cols(new));
    let difference: Vec<f32> = a
        .iter()
        .zip(&b)
        .map(|(a, b)| match (a, b) {
            (Some((la, sa)), Some((lb, sb))) => {
                let spectral = sa
                    .iter()
                    .zip(sb.iter())
                    .map(|(x, y)| x.abs_diff(*y) as f32)
                    .sum::<f32>()
                    / (BANDS as f32 * 255.);
                let loud = (la.rms - lb.rms).abs();
                (spectral * 4.).max(loud * 4.).min(1.)
            }
            (Some((l, _)), None) | (None, Some((l, _))) => {
                if l.rms > SILENT {
                    1.
                } else {
                    0.
                }
            }
            (None, None) => 0.,
        })
        .collect();
    let changes = ranges(&difference, AUDIO_CHANGED, seconds, 2)
        .into_iter()
        .map(|(a, b)| (a + start, b + start))
        .collect();
    AudioComparison {
        start,
        columns,
        seconds_per_column: seconds,
        old: a,
        new: b,
        difference,
        changes,
    }
}

/// The stretches where `scores` is over `threshold` (`seconds` a score),
/// joined across gaps of up to `bridge` scores.
fn ranges(scores: &[f32], threshold: f32, seconds: f64, bridge: usize) -> Vec<(f64, f64)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (i, s) in scores.iter().enumerate() {
        if *s <= threshold {
            continue;
        }
        match out.last_mut() {
            Some((_, end)) if i <= *end + bridge => *end = i + 1,
            _ => out.push((i, i + 1)),
        }
    }
    out.into_iter()
        .map(|(a, b)| (a as f64 * seconds, b as f64 * seconds))
        .collect()
}

/// The order video samples are taken in: coarse to fine, so the change
/// strip fills in evenly.
pub fn sample_order(n: usize) -> Vec<usize> {
    let mut order = Vec::with_capacity(n);
    let mut seen = vec![false; n];
    let mut step = n.next_power_of_two();
    while step >= 1 {
        let mut i = 0;
        while i < n {
            if !seen[i] {
                seen[i] = true;
                order.push(i);
            }
            i += step;
        }
        if step == 1 {
            break;
        }
        step /= 2;
    }
    order
}

/// How many video samples a timeline of `duration` seconds gets.
pub fn video_samples(duration: f64) -> usize {
    ((duration * 2.).ceil() as usize).clamp(4, 120)
}

/// The longer side of a sample picture, in pixels.
pub const THUMB: u32 = 160;

/// A video sample changed when its pictures differ by more than this.
const VIDEO_CHANGED: f32 = 0.01;

/// The video samples taken so far.
#[derive(Clone, Debug, Default)]
pub struct VideoComparison {
    pub duration: f64,
    pub old: Vec<Option<Frame>>,
    pub new: Vec<Option<Frame>>,
    /// `None` until both pictures of a sample are in.
    pub difference: Vec<Option<f32>>,
    pub done: usize,
}

impl VideoComparison {
    pub fn new(duration: f64) -> Self {
        let n = video_samples(duration);
        Self {
            duration,
            old: vec![None; n],
            new: vec![None; n],
            difference: vec![None; n],
            done: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.old.len()
    }

    pub fn is_empty(&self) -> bool {
        self.old.is_empty()
    }

    pub fn seconds(&self) -> f64 {
        self.duration / self.len().max(1) as f64
    }

    /// When sample `i` is taken.
    pub fn time(&self, i: usize) -> f64 {
        (i as f64 + 0.5) * self.seconds()
    }

    /// The sample nearest `time`.
    pub fn index_at(&self, time: f64) -> usize {
        ((time / self.seconds().max(1e-9)).floor().max(0.) as usize)
            .min(self.len().saturating_sub(1))
    }

    pub fn changes(&self) -> Vec<(f64, f64)> {
        let scores: Vec<f32> = self.difference.iter().map(|d| d.unwrap_or(0.)).collect();
        ranges(&scores, VIDEO_CHANGED, self.seconds(), 0)
    }

    /// Takes the samples one at a time (both sides in parallel), calling
    /// `on_sample` after each, until all are in or `cancel` is set.
    pub fn sample(
        duration: f64,
        old: Option<&Source>,
        new: Option<&Source>,
        cancel: &AtomicBool,
        on_sample: &mut dyn FnMut(&VideoComparison),
    ) {
        let mut cmp = Self::new(duration);
        let thumb = |s: Option<&Source>, t: f64| {
            let s = s.filter(|s| s.has_video() && t < s.duration().max(0.001))?;
            s.frame_at(t, (THUMB, THUMB)).ok()
        };
        for i in sample_order(cmp.len()) {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let t = cmp.time(i);
            let (a, b) = std::thread::scope(|scope| {
                let a = scope.spawn(|| thumb(old, t));
                let b = thumb(new, t);
                (a.join().ok().flatten(), b)
            });
            cmp.difference[i] = Some(match (&a, &b) {
                (Some(a), Some(b)) => picture_distance(a, b),
                (None, None) => 0.,
                _ => 1.,
            });
            cmp.old[i] = a;
            cmp.new[i] = b;
            cmp.done += 1;
            on_sample(&cmp);
        }
    }
}

/// Analyses kept across reloads of the same files, newest last.
static AUDIO_CACHE: Mutex<VecDeque<(String, Arc<AudioSummary>)>> = Mutex::new(VecDeque::new());
const AUDIO_CACHE_LEN: usize = 8;

/// A summary made earlier for `key` ([`corvene_models::MediaSide::key`]).
pub fn cached_audio(key: &str) -> Option<Arc<AudioSummary>> {
    let cache = AUDIO_CACHE.lock().ok()?;
    cache.iter().find(|(k, _)| k == key).map(|(_, s)| s.clone())
}

pub fn cache_audio(key: String, summary: Arc<AudioSummary>) {
    if let Ok(mut cache) = AUDIO_CACHE.lock() {
        cache.retain(|(k, _)| *k != key);
        cache.push_back((key, summary));
        while cache.len() > AUDIO_CACHE_LEN {
            cache.pop_front();
        }
    }
}

/// The inferno colour map (dark purple → yellow), sampled at 9 stops.
const INFERNO: [[u8; 3]; 9] = [
    [0, 0, 4],
    [31, 12, 72],
    [85, 15, 109],
    [136, 34, 106],
    [186, 54, 85],
    [227, 89, 51],
    [249, 140, 10],
    [249, 201, 50],
    [252, 255, 164],
];

/// The colour of a spectrum byte, as BGRA.
pub fn inferno(v: u8) -> [u8; 4] {
    let x = v as f32 / 255. * (INFERNO.len() - 1) as f32;
    let i = (x.floor() as usize).min(INFERNO.len() - 2);
    let f = x - i as f32;
    let (a, b) = (INFERNO[i], INFERNO[i + 1]);
    let c = |k: usize| (a[k] as f32 + (b[k] as f32 - a[k] as f32) * f).round() as u8;
    [c(2), c(1), c(0), 255]
}

/// A spectrogram picture: a column of pixels per column, high bands on top,
/// `height` rows. Columns past the end are `background` (BGRA).
pub fn spectrogram(
    columns: &[Option<(Level, [u8; BANDS])>],
    height: u32,
    background: [u8; 4],
) -> Frame {
    let w = columns.len().max(1) as u32;
    let h = height.max(1);
    let mut out = vec![0u8; (w * h * 4) as usize];
    for (x, col) in columns.iter().enumerate() {
        for y in 0..h {
            let band = ((h - 1 - y) as usize * BANDS / h as usize).min(BANDS - 1);
            let px = match col {
                Some((_, bands)) => inferno(bands[band]),
                None => background,
            };
            let o = ((y * w) as usize + x) * 4;
            out[o..o + 4].copy_from_slice(&px);
        }
    }
    Frame::new(0., w, h, out)
}

/// The difference of two spectrograms: brighter where they differ.
pub fn spectrogram_difference(
    a: &[Option<(Level, [u8; BANDS])>],
    b: &[Option<(Level, [u8; BANDS])>],
    height: u32,
    background: [u8; 4],
) -> Frame {
    let joined: Vec<Option<(Level, [u8; BANDS])>> = a
        .iter()
        .zip(b)
        .map(|(a, b)| {
            let (sa, sb) = (a.map(|c| c.1), b.map(|c| c.1));
            if sa.is_none() && sb.is_none() {
                return None;
            }
            let (sa, sb) = (sa.unwrap_or([0; BANDS]), sb.unwrap_or([0; BANDS]));
            let mut d = [0u8; BANDS];
            for i in 0..BANDS {
                d[i] = (sa[i].abs_diff(sb[i]) as u32 * 3).min(255) as u8;
            }
            Some((Level::default(), d))
        })
        .collect();
    spectrogram(&joined, height, background)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, rate: u32, seconds: f32, amp: f32) -> Vec<f32> {
        (0..(rate as f32 * seconds) as usize)
            .map(|i| (2. * PI * hz * i as f32 / rate as f32).sin() * amp)
            .collect()
    }

    fn summary_of(samples: &[f32], rate: u32) -> AudioSummary {
        let fft = Fft::new(rate);
        let mut s = AudioSummary {
            rate,
            block_seconds: BLOCK as f64 / rate as f64,
            ..Default::default()
        };
        let mut spectrum = [0u8; BANDS];
        for block in samples.chunks(BLOCK) {
            let rms = (block.iter().map(|v| v * v).sum::<f32>() / block.len() as f32).sqrt();
            fft.spectrum(block, &mut spectrum);
            s.levels.push(Level {
                min: block.iter().copied().fold(f32::MAX, f32::min),
                max: block.iter().copied().fold(f32::MIN, f32::max),
                rms,
            });
            s.spectrum.extend_from_slice(&spectrum);
        }
        s
    }

    #[test]
    fn a_sine_peaks_in_its_band() {
        let rate = 48_000;
        let s = summary_of(&sine(1000., rate, 0.1, 1.), rate);
        let bands = &s.spectrum[BANDS * 2..BANDS * 3];
        let loudest = (0..BANDS).max_by_key(|&i| bands[i]).unwrap();
        let fft = Fft::new(rate);
        let hz_per_bin = rate as f32 / BLOCK as f32;
        let range = &fft.bands[loudest];
        assert!(
            (range.start as f32 * hz_per_bin..=range.end as f32 * hz_per_bin).contains(&1000.),
            "band {loudest} {range:?}"
        );
        // close to full scale
        assert!(bands[loudest] > 240, "{}", bands[loudest]);
    }

    #[test]
    fn same_sound_has_no_changes_and_an_edit_is_found() {
        let rate = 44_100;
        let a = sine(440., rate, 4., 0.5);
        let mut b = a.clone();
        // a different note from 2 s to 2.5 s
        let edit = sine(880., rate, 0.5, 0.5);
        b[2 * rate as usize..2 * rate as usize + edit.len()].copy_from_slice(&edit);
        let (sa, sb) = (summary_of(&a, rate), summary_of(&b, rate));
        assert!(compare_audio(Some(&sa), Some(&sa), 400).changes.is_empty());
        let changes = compare_audio(Some(&sa), Some(&sb), 400).changes;
        assert_eq!(changes.len(), 1, "{changes:?}");
        let (start, end) = changes[0];
        assert!((1.9..=2.05).contains(&start), "{start}");
        assert!((2.45..=2.6).contains(&end), "{end}");
    }

    #[test]
    fn a_longer_sound_changes_at_its_tail() {
        let rate = 8_000;
        let a = sine(300., rate, 1., 0.5);
        let b = sine(300., rate, 2., 0.5);
        let cmp = compare_audio(
            Some(&summary_of(&a, rate)),
            Some(&summary_of(&b, rate)),
            100,
        );
        assert_eq!(cmp.changes.len(), 1);
        assert!((cmp.changes[0].0 - 1.).abs() < 0.15, "{:?}", cmp.changes);
    }

    #[test]
    fn coarse_to_fine_order_covers_everything_once() {
        let order = sample_order(10);
        assert_eq!(order[0], 0);
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(sorted, (0..10).collect::<Vec<_>>());
        assert_eq!(sample_order(1), vec![0]);
    }

    #[test]
    fn ranges_join_close_changes() {
        let s = [0., 1., 0., 1., 0., 0., 0., 1.];
        assert_eq!(ranges(&s, 0.5, 1., 1), vec![(1., 4.), (7., 8.)]);
    }
}
