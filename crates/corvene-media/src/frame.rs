//! Decoded pictures and sound, and the pixel work done on them.

use std::sync::Arc;

/// One decoded picture: opaque BGRA, 4 bytes a pixel, rows packed.
#[derive(Clone, Debug)]
pub struct Frame {
    /// When it shows, in seconds from the start.
    pub time: f64,
    pub width: u32,
    pub height: u32,
    pub bgra: Arc<Vec<u8>>,
}

impl Frame {
    pub fn new(time: f64, width: u32, height: u32, bgra: Vec<u8>) -> Self {
        debug_assert_eq!(bgra.len(), (width * height * 4) as usize);
        Self {
            time,
            width,
            height,
            bgra: Arc::new(bgra),
        }
    }
}

/// Some decoded sound: interleaved `f32` samples in -1..1.
#[derive(Clone, Debug)]
pub struct AudioChunk {
    /// When the first sample plays, in seconds from the start.
    pub time: f64,
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

impl AudioChunk {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }
}

/// The largest size with `w`×`h`'s aspect that fits in `max` (never larger
/// than `w`×`h`), each side at least 1 and even when shrunk (some decoders
/// want even sizes).
pub fn fit_size((w, h): (u32, u32), (max_w, max_h): (u32, u32)) -> (u32, u32) {
    if w == 0 || h == 0 {
        return (max_w.max(2), max_h.max(2));
    }
    let k = (max_w as f64 / w as f64)
        .min(max_h as f64 / h as f64)
        .min(1.);
    if k >= 1. {
        return (w, h);
    }
    let even = |v: f64| ((v.round() as u32) & !1).max(2);
    (even(w as f64 * k), even(h as f64 * k))
}

/// `src` (BGRA, `sw`×`sh`, `stride` bytes a row) resampled to `dw`×`dh`:
/// an area average when shrinking, bilinear when growing.
pub fn resize_bgra(src: &[u8], sw: u32, sh: u32, stride: usize, dw: u32, dh: u32) -> Vec<u8> {
    let (sw_, sh_, dw_, dh_) = (sw as usize, sh as usize, dw as usize, dh as usize);
    let mut out = vec![0u8; dw_ * dh_ * 4];
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return out;
    }
    if sw == dw && sh == dh {
        for y in 0..sh_ {
            out[y * dw_ * 4..(y + 1) * dw_ * 4]
                .copy_from_slice(&src[y * stride..y * stride + sw_ * 4]);
        }
        return out;
    }
    if dw <= sw && dh <= sh {
        // box filter over each destination pixel's source rectangle
        let xs: Vec<(usize, usize)> = (0..dw_)
            .map(|x| {
                let a = x * sw_ / dw_;
                let b = ((x + 1) * sw_ / dw_).max(a + 1).min(sw_);
                (a, b)
            })
            .collect();
        let mut acc = vec![0u32; dw_ * 4];
        for y in 0..dh_ {
            let y0 = y * sh_ / dh_;
            let y1 = ((y + 1) * sh_ / dh_).max(y0 + 1).min(sh_);
            acc.iter_mut().for_each(|v| *v = 0);
            for sy in y0..y1 {
                let row = &src[sy * stride..sy * stride + sw_ * 4];
                for (x, &(a, b)) in xs.iter().enumerate() {
                    let mut s = [0u32; 4];
                    for p in row[a * 4..b * 4].as_chunks::<4>().0.iter() {
                        s[0] += p[0] as u32;
                        s[1] += p[1] as u32;
                        s[2] += p[2] as u32;
                        s[3] += p[3] as u32;
                    }
                    let o = &mut acc[x * 4..x * 4 + 4];
                    for i in 0..4 {
                        o[i] += s[i];
                    }
                }
            }
            for (x, &(a, b)) in xs.iter().enumerate() {
                let n = ((b - a) * (y1 - y0)) as u32;
                let o = &mut out[(y * dw_ + x) * 4..(y * dw_ + x) * 4 + 4];
                for i in 0..4 {
                    o[i] = ((acc[x * 4 + i] + n / 2) / n) as u8;
                }
            }
        }
        return out;
    }
    // bilinear
    for y in 0..dh_ {
        let fy = ((y as f32 + 0.5) * sh as f32 / dh as f32 - 0.5).clamp(0., (sh - 1) as f32);
        let y0 = fy as usize;
        let y1 = (y0 + 1).min(sh_ - 1);
        let wy = fy - y0 as f32;
        for x in 0..dw_ {
            let fx = ((x as f32 + 0.5) * sw as f32 / dw as f32 - 0.5).clamp(0., (sw - 1) as f32);
            let x0 = fx as usize;
            let x1 = (x0 + 1).min(sw_ - 1);
            let wx = fx - x0 as f32;
            for i in 0..4 {
                let p = |xx: usize, yy: usize| src[yy * stride + xx * 4 + i] as f32;
                let top = p(x0, y0) * (1. - wx) + p(x1, y0) * wx;
                let bottom = p(x0, y1) * (1. - wx) + p(x1, y1) * wx;
                out[(y * dw_ + x) * 4 + i] = (top * (1. - wy) + bottom * wy).round() as u8;
            }
        }
    }
    out
}

/// `src` (BGRA, `w`×`h`, packed) turned clockwise by `degrees` (0, 90, 180
/// or 270); returns the new size and pixels.
pub fn rotate_bgra(src: Vec<u8>, w: u32, h: u32, degrees: u32) -> (u32, u32, Vec<u8>) {
    let (wu, hu) = (w as usize, h as usize);
    match degrees % 360 {
        90 => {
            let mut out = vec![0u8; src.len()];
            for y in 0..hu {
                for x in 0..wu {
                    // (x, y) -> (h - 1 - y, x) in an h-wide picture
                    let d = (x * hu + (hu - 1 - y)) * 4;
                    out[d..d + 4].copy_from_slice(&src[(y * wu + x) * 4..(y * wu + x) * 4 + 4]);
                }
            }
            (h, w, out)
        }
        180 => {
            let mut out = vec![0u8; src.len()];
            for (i, p) in src.as_chunks::<4>().0.iter().enumerate() {
                let d = (wu * hu - 1 - i) * 4;
                out[d..d + 4].copy_from_slice(p);
            }
            (w, h, out)
        }
        270 => {
            let mut out = vec![0u8; src.len()];
            for y in 0..hu {
                for x in 0..wu {
                    let d = ((wu - 1 - x) * hu + y) * 4;
                    out[d..d + 4].copy_from_slice(&src[(y * wu + x) * 4..(y * wu + x) * 4 + 4]);
                }
            }
            (h, w, out)
        }
        _ => (w, h, src),
    }
}

/// The per-pixel difference `|a - b|` of two pictures, `b` resampled to
/// `a`'s size first when they differ; `gain` brightens faint changes
/// (1 = as is). Opaque BGRA.
pub fn difference_bgra(a: &Frame, b: &Frame, gain: f32) -> Frame {
    let resized;
    let b_pixels: &[u8] = if (a.width, a.height) == (b.width, b.height) {
        &b.bgra
    } else {
        resized = resize_bgra(
            &b.bgra,
            b.width,
            b.height,
            b.width as usize * 4,
            a.width,
            a.height,
        );
        &resized
    };
    let mut out = vec![0u8; a.bgra.len()];
    let lut: Vec<u8> = (0..256)
        .map(|d| ((d as f32) * gain).round().min(255.) as u8)
        .collect();
    for ((o, p), q) in out
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(a.bgra.as_chunks::<4>().0.iter())
        .zip(b_pixels.as_chunks::<4>().0.iter())
    {
        o[0] = lut[p[0].abs_diff(q[0]) as usize];
        o[1] = lut[p[1].abs_diff(q[1]) as usize];
        o[2] = lut[p[2].abs_diff(q[2]) as usize];
        o[3] = 255;
    }
    Frame::new(a.time, a.width, a.height, out)
}

/// How different two small pictures are, 0 (same) to 1: the mean absolute
/// difference of their luma, `b` resampled to `a`'s size.
pub fn picture_distance(a: &Frame, b: &Frame) -> f32 {
    let resized;
    let b_pixels: &[u8] = if (a.width, a.height) == (b.width, b.height) {
        &b.bgra
    } else {
        resized = resize_bgra(
            &b.bgra,
            b.width,
            b.height,
            b.width as usize * 4,
            a.width,
            a.height,
        );
        &resized
    };
    let luma = |p: &[u8]| 0.114 * p[0] as f32 + 0.587 * p[1] as f32 + 0.299 * p[2] as f32;
    let n = (a.width * a.height).max(1) as f32;
    let sum: f32 = a
        .bgra
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b_pixels.as_chunks::<4>().0.iter())
        .map(|(p, q)| (luma(p) - luma(q)).abs())
        .sum();
    sum / n / 255.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_aspect_and_never_grows() {
        assert_eq!(fit_size((1920, 1080), (960, 960)), (960, 540));
        assert_eq!(fit_size((100, 50), (1000, 1000)), (100, 50));
        assert_eq!(fit_size((1080, 1920), (500, 500)), (280, 500));
    }

    #[test]
    fn box_filter_averages() {
        // 2×1 → 1×1: black and white make grey
        let src = [0, 0, 0, 255, 255, 255, 255, 255];
        assert_eq!(resize_bgra(&src, 2, 1, 8, 1, 1), vec![128, 128, 128, 255]);
    }

    #[test]
    fn rotation_moves_corners() {
        // 2×1: red, blue → turned 90° is 1×2: red on top
        let src = vec![0, 0, 255, 255, 255, 0, 0, 255];
        let (w, h, out) = rotate_bgra(src.clone(), 2, 1, 90);
        assert_eq!((w, h), (1, 2));
        assert_eq!(&out[..4], &[0, 0, 255, 255]);
        let (_, _, out) = rotate_bgra(src, 2, 1, 180);
        assert_eq!(&out[..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn difference_and_distance() {
        let a = Frame::new(0., 1, 1, vec![10, 20, 30, 255]);
        let b = Frame::new(0., 1, 1, vec![20, 20, 0, 255]);
        assert_eq!(*difference_bgra(&a, &b, 1.).bgra, vec![10, 0, 30, 255]);
        assert_eq!(*difference_bgra(&a, &b, 4.).bgra, vec![40, 0, 120, 255]);
        assert_eq!(picture_distance(&a, &a), 0.);
        assert!(picture_distance(&a, &b) > 0.);
    }
}
