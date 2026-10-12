//! The sound stage: the versions' waveforms in two lanes (Stacked) or one
//! (Overlay, with their difference under it), or their spectrograms with
//! a difference lane. Changed stretches are shaded. Click a lane to play
//! from there and hear that version; drag to scrub, shift-drag to loop,
//! ⌘-scroll to zoom in time, scroll to move along, double-click to see it
//! all.

use std::sync::Arc;

use corvene_core::corvene_media::analysis::{self, AudioComparison, BANDS, LOW_HZ, Level};
use corvene_core::corvene_media::player::Side;
use corvene_core::{AudioDiffMode, MediaKind};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{MediaDiffView, fill_quad, format_time, render_image, time_at, undecodable, x_of};
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;

/// Rows of a spectrogram picture.
const SPECTRUM_ROWS: u32 = 192;

/// What a lane draws.
#[derive(Clone, Copy, PartialEq)]
enum Lane {
    Wave(Side),
    Both,
    Difference,
    Spectrum(usize),
}

/// A tick every this many seconds, for about one per `px_per_tick`.
fn tick_step(span: f64, width: f32, px_per_tick: f32) -> f64 {
    const STEPS: [f64; 18] = [
        0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.25, 0.5, 1., 2., 5., 10., 15., 30., 60., 120., 300.,
        600.,
    ];
    let wanted = span / (width / px_per_tick).max(1.) as f64;
    STEPS
        .iter()
        .copied()
        .find(|s| *s >= wanted)
        .unwrap_or(1800.)
}

fn tick_label(t: f64, step: f64) -> String {
    if step >= 1. {
        let s = t.round() as u64;
        format!("{}:{:02}", s / 60, s % 60)
    } else {
        let text = format_time(t);
        // drop the milliseconds the step does not need
        let keep = if step >= 0.1 {
            2
        } else if step >= 0.01 {
            1
        } else {
            0
        };
        text[..text.len() - keep].to_string()
    }
}

/// The y of `hz` in a spectrogram lane `b` of a sound at `rate`.
fn hz_y(hz: f32, rate: u32, b: Bounds<Pixels>) -> Option<Pixels> {
    let nyquist = rate as f32 / 2.;
    let low = LOW_HZ.min(nyquist / 2.);
    if hz <= low || hz >= nyquist {
        return None;
    }
    let f = (hz / low).ln() / (nyquist / low).ln();
    Some(b.bottom() - b.size.height * f)
}

impl MediaDiffView {
    fn lane_press(
        &mut self,
        position: Point<Pixels>,
        lane: Option<Side>,
        shift: bool,
        clicks: usize,
        cx: &mut Context<Self>,
    ) {
        if clicks >= 2 {
            self.view = None;
            cx.notify();
            return;
        }
        let bounds = self.lanes.get();
        let t = time_at(position.x, bounds, self.view_range());
        if shift {
            self.loop_drag = Some(t);
            self.player.set_loop_range(None);
        } else {
            if let Some(side) = lane {
                self.listen(side, cx);
            }
            self.scrub = Some(self.player.is_playing());
            self.player.pause();
            self.player.seek(t);
        }
        cx.notify();
    }

    fn lane_move(&mut self, position: Point<Pixels>, pressed: bool, cx: &mut Context<Self>) {
        let bounds = self.lanes.get();
        let t = time_at(position.x, bounds, self.view_range());
        if pressed && let Some(start) = self.loop_drag {
            self.player
                .set_loop_range(Some((start.min(t), start.max(t))));
            self.player.set_looping(true);
        } else if pressed && self.scrub.is_some() {
            self.player.seek(t);
        }
        if bounds.contains(&position) {
            self.hover = Some(t);
        }
        cx.notify();
    }

    /// ⌘-scroll zooms around the pointer; scrolling moves along.
    fn lane_scroll(&mut self, ev: &ScrollWheelEvent, line: Pixels, cx: &mut Context<Self>) {
        let bounds = self.lanes.get();
        let duration = self.duration();
        if duration <= 0. {
            return;
        }
        let (start, end) = self.view_range();
        let span = end - start;
        let delta = ev.delta.pixel_delta(line);
        if ev.modifiers.platform || ev.modifiers.control {
            let at = time_at(ev.position.x, bounds, (start, end));
            let k = (f32::from(delta.y) / 200.).exp() as f64;
            // at least 50 ms, at most everything
            let new_span = (span / k).clamp(0.05_f64.min(duration), duration);
            let f = (at - start) / span.max(1e-9);
            let a = (at - f * new_span).clamp(0., duration - new_span);
            self.view = (new_span < duration - 1e-6).then_some((a, a + new_span));
        } else if self.view.is_some() {
            let dx = if f32::from(delta.x).abs() > f32::from(delta.y).abs() {
                delta.x
            } else {
                delta.y
            };
            let w = f32::from(bounds.size.width).max(1.) as f64;
            let shift = -(f32::from(dx) as f64) / w * span;
            let a = (start + shift).clamp(0., duration - span);
            self.view = Some((a, a + span));
        } else {
            return;
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// The spectrogram pictures for the lanes' comparison.
    fn spectrogram_images(
        &mut self,
        cmp: &AudioComparison,
        background: [u8; 4],
    ) -> [Option<Arc<RenderImage>>; 3] {
        let (start, end) = self.view_range();
        let key = (
            cmp.columns,
            start.to_bits(),
            end.to_bits(),
            u32::from_le_bytes(background),
        );
        if let Some((k, images)) = &self.spectrograms
            && *k == key
        {
            return images.clone();
        }
        let make = |f: corvene_core::corvene_media::Frame| render_image(&f).map(|s| s.image);
        let images = [
            make(analysis::spectrogram(&cmp.old, SPECTRUM_ROWS, background)),
            make(analysis::spectrogram(&cmp.new, SPECTRUM_ROWS, background)),
            make(analysis::spectrogram_difference(
                &cmp.old,
                &cmp.new,
                SPECTRUM_ROWS,
                background,
            )),
        ];
        if let Some((_, old)) = self.spectrograms.replace((key, images.clone())) {
            self.retired.extend(old.into_iter().flatten());
        }
        images
    }
}

fn bgra(color: Hsla) -> [u8; 4] {
    let c = color.to_rgb();
    let b = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    [b(c.b), b(c.g), b(c.r), 255]
}

#[allow(clippy::too_many_arguments)]
fn paint_lane(
    lane: Lane,
    b: Bounds<Pixels>,
    cmp: &AudioComparison,
    images: &[Option<Arc<RenderImage>>; 3],
    rate: u32,
    colors: &LaneColors,
    window: &mut Window,
) {
    window.paint_quad(fill(b, colors.background));
    let n = cmp.columns.max(1);
    let w = b.size.width / n as f32;
    let range = (cmp.start, cmp.start + cmp.seconds_per_column * n as f64);
    // changed stretches behind everything
    if !matches!(lane, Lane::Spectrum(_)) {
        for (a, z) in &cmp.changes {
            let x0 = x_of(*a, b, range).max(b.left());
            let x1 = x_of(*z, b, range).min(b.right());
            if x1 > x0 {
                window.paint_quad(fill(
                    Bounds::new(point(x0, b.top()), size(x1 - x0, b.size.height)),
                    colors.changed.opacity(0.13),
                ));
            }
        }
    }
    let mid = b.center().y;
    let half = b.size.height * 0.46;
    let wave =
        |cols: &[Option<(Level, [u8; BANDS])>], color: Hsla, rms: Hsla, window: &mut Window| {
            for (x, col) in cols.iter().enumerate() {
                let Some((level, _)) = col else { continue };
                let left = b.left() + w * x as f32;
                let top = mid - half * level.max.clamp(-1., 1.);
                let bottom = mid - half * level.min.clamp(-1., 1.);
                window.paint_quad(fill(
                    Bounds::new(
                        point(left, top),
                        size(w.max(px(1.)), (bottom - top).max(px(1.))),
                    ),
                    color,
                ));
                let r = half * level.rms.clamp(0., 1.);
                window.paint_quad(fill(
                    Bounds::new(
                        point(left, mid - r),
                        size(w.max(px(1.)), (r * 2.).max(px(1.))),
                    ),
                    rms,
                ));
            }
        };
    match lane {
        Lane::Wave(side) => {
            window.paint_quad(fill(
                Bounds::new(point(b.left(), mid), size(b.size.width, px(1.))),
                colors.axis,
            ));
            let (cols, color) = match side {
                Side::Old => (&cmp.old, colors.old),
                Side::New => (&cmp.new, colors.new),
            };
            wave(cols, color.opacity(0.55), color, window);
        }
        Lane::Both => {
            window.paint_quad(fill(
                Bounds::new(point(b.left(), mid), size(b.size.width, px(1.))),
                colors.axis,
            ));
            wave(
                &cmp.old,
                colors.old.opacity(0.35),
                colors.old.opacity(0.6),
                window,
            );
            wave(
                &cmp.new,
                colors.new.opacity(0.35),
                colors.new.opacity(0.6),
                window,
            );
        }
        Lane::Difference => {
            for (x, d) in cmp.difference.iter().enumerate() {
                if *d <= 0. {
                    continue;
                }
                let h = b.size.height * d.clamp(0., 1.);
                window.paint_quad(fill(
                    Bounds::new(
                        point(b.left() + w * x as f32, b.bottom() - h),
                        size(w.max(px(1.)), h),
                    ),
                    colors.changed,
                ));
            }
        }
        Lane::Spectrum(i) => {
            if let Some(image) = images[i].clone() {
                window
                    .paint_image(b, b, Corners::default(), image, 0, false)
                    .ok();
            }
            for hz in [100., 1000., 10_000.] {
                if let Some(y) = hz_y(hz, rate, b) {
                    window.paint_quad(fill(
                        Bounds::new(point(b.left(), y), size(b.size.width, px(1.))),
                        gpui_kit::white().opacity(0.18),
                    ));
                }
            }
        }
    }
}

struct LaneColors {
    background: Hsla,
    axis: Hsla,
    old: Hsla,
    new: Hsla,
    changed: Hsla,
}

pub(super) fn stage(
    view: &mut MediaDiffView,
    mode: AudioDiffMode,
    window: &mut Window,
    cx: &mut Context<MediaDiffView>,
) -> AnyElement {
    let t = cx.ghd().clone();
    let message = |text: String| {
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_color(t.text_secondary)
            .child(text)
            .into_any_element()
    };
    let lanes_bounds = view.lanes.get();
    let columns = (f32::from(lanes_bounds.size.width).round() as usize).clamp(50, 4000);
    let Some(cmp) = view.lanes_comparison(columns) else {
        let sides = [view.diff.previous.as_ref(), view.diff.current.as_ref()];
        if let Some(text) = sides.iter().find_map(|s| undecodable(*s, MediaKind::Audio))
            && sides
                .iter()
                .flatten()
                .all(|s| undecodable(Some(s), MediaKind::Audio).is_some())
        {
            return message(text);
        }
        if view.sound.failed && view.sound.pending == 0 {
            return message("The sound could not be read.".into());
        }
        let p = view
            .sound
            .progress
            .iter()
            .map(|p| f32::from_bits(p.load(std::sync::atomic::Ordering::Relaxed)))
            .fold(0f32, f32::max);
        // measure the lanes while waiting
        let cell = view.lanes.clone();
        return div()
            .relative()
            .flex_1()
            .child(
                canvas(move |b, _, _| cell.set(b), |_, _, _, _| {})
                    .absolute()
                    .inset_0(),
            )
            .child(message(format!(
                "Reading the sound\u{2026} {}%",
                (p * 100.).round()
            )))
            .into_any_element();
    };
    let compares = view.compares();
    let show_old = view.diff.previous.is_some();
    let show_new = view.diff.current.is_some();
    let lanes: Vec<(Lane, &'static str, Option<Side>, f32)> = match mode {
        AudioDiffMode::Overlay if compares => vec![
            (Lane::Both, "Old and new", None, 3.),
            (Lane::Difference, "Difference", None, 1.),
        ],
        AudioDiffMode::Spectrogram => {
            let mut v = Vec::new();
            if show_old {
                v.push((Lane::Spectrum(0), "Old", Some(Side::Old), 2.));
            }
            if show_new {
                v.push((Lane::Spectrum(1), "New", Some(Side::New), 2.));
            }
            if compares {
                v.push((Lane::Spectrum(2), "Difference", None, 1.));
            }
            v
        }
        _ => {
            let mut v = Vec::new();
            if show_old {
                v.push((Lane::Wave(Side::Old), "Old", Some(Side::Old), 1.));
            }
            if show_new {
                v.push((Lane::Wave(Side::New), "New", Some(Side::New), 1.));
            }
            v
        }
    };
    let images = if mode == AudioDiffMode::Spectrogram {
        view.spectrogram_images(&cmp, bgra(t.box_alt_background))
    } else {
        [None, None, None]
    };
    let rate = [view.diff.current.as_ref(), view.diff.previous.as_ref()]
        .into_iter()
        .flatten()
        .find_map(|s| s.info.audio.as_ref().map(|a| a.sample_rate))
        .unwrap_or(44_100);
    let range = view.view_range();
    let position = view.player.position();
    let hover = view.hover;
    let loop_range = view.player.loop_range();
    let audible = view.player.audible();
    let colors = Arc::new(LaneColors {
        background: t.box_alt_background,
        axis: t.box_border,
        old: t.color_deleted,
        new: t.color_new,
        changed: t.color_modified,
    });
    let head = t.text;
    let accent = t.accent;
    let line = window.line_height();
    // the ruler
    let span = range.1 - range.0;
    let step = tick_step(span, f32::from(lanes_bounds.size.width), 90.);
    let first = (range.0 / step).ceil() as i64;
    let last = (range.1 / step).floor() as i64;
    let ticks: Vec<AnyElement> = (first..=last.min(first + 200))
        .map(|k| {
            let at = k as f64 * step;
            let x = x_of(at, lanes_bounds, range) - lanes_bounds.left();
            div()
                .absolute()
                .left(x)
                .top_0()
                .h_full()
                .flex()
                .flex_row()
                .child(div().w(px(1.)).h_full().bg(t.box_border))
                .child(
                    div()
                        .pl(zpx(3.))
                        .text_size(FONT_SIZE_SM())
                        .text_color(t.text_secondary)
                        .child(tick_label(at, step)),
                )
                .into_any_element()
        })
        .collect();
    let lane_count = lanes.len();
    let lane_elements: Vec<AnyElement> = lanes
        .into_iter()
        .enumerate()
        .map(|(ix, (lane, title, side, grow))| {
            let cmp = cmp.clone();
            let images = images.clone();
            let colors = colors.clone();
            let cell = view.lanes.clone();
            let measures = ix == 0;
            let painter = canvas(
                move |b, _, _| {
                    // the lanes share their x axis; the first one measures it
                    if measures {
                        cell.set(b);
                    }
                },
                move |b, _, window, _| {
                    window.with_content_mask(Some(ContentMask { bounds: b }), |window| {
                        paint_lane(lane, b, &cmp, &images, rate, &colors, window);
                        if let Some((a, z)) = loop_range {
                            let x0 = x_of(a, b, range);
                            let x1 = x_of(z, b, range);
                            window.paint_quad(fill(
                                Bounds::new(point(x0, b.top()), size(x1 - x0, b.size.height)),
                                accent.opacity(0.12),
                            ));
                        }
                        if let Some(h) = hover {
                            let x = x_of(h, b, range);
                            window.paint_quad(fill(
                                Bounds::new(point(x, b.top()), size(px(1.), b.size.height)),
                                head.opacity(0.35),
                            ));
                        }
                        let x = x_of(position, b, range);
                        window.paint_quad(fill_quad(
                            Bounds::new(point(x - px(1.), b.top()), size(px(2.), b.size.height)),
                            head,
                            px(1.),
                        ));
                    });
                },
            )
            .absolute()
            .inset_0();
            let heard = side.is_some_and(|s| s == audible) && view.player.has_sound() && compares;
            let label_color = match side {
                Some(Side::Old) => t.color_deleted,
                Some(Side::New) => t.color_new,
                None => t.text_secondary,
            };
            div()
                .id(("media-lane", ix))
                .relative()
                .flex_grow(grow)
                .flex_basis(px(0.))
                .min_h(zpx(36.))
                .rounded(BORDER_RADIUS())
                .overflow_hidden()
                .when(heard, |d| {
                    d.border_1().border_color(label_color.opacity(0.8))
                })
                .child(painter)
                .child(
                    div()
                        .absolute()
                        .top(zpx(4.))
                        .left(zpx(6.))
                        .px(zpx(5.))
                        .rounded(BORDER_RADIUS())
                        .bg(t.background.opacity(0.75))
                        .text_size(FONT_SIZE_SM())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(label_color)
                        .child(if heard {
                            format!("{title} \u{00b7} heard")
                        } else {
                            title.to_string()
                        }),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                        this.focus.focus(window, cx);
                        this.lane_press(ev.position, side, ev.modifiers.shift, ev.click_count, cx);
                    }),
                )
                .into_any_element()
        })
        .collect();
    let _ = lane_count;
    div()
        .id("media-lanes")
        .flex_1()
        .min_h_0()
        .mx(SPACING())
        .flex()
        .flex_col()
        .gap(zpx(4.))
        .child(
            div()
                .relative()
                .flex_none()
                .h(zpx(16.))
                .overflow_hidden()
                .children(ticks),
        )
        .children(lane_elements)
        .child(
            div()
                .flex_none()
                .text_size(FONT_SIZE_SM())
                .text_color(t.text_secondary)
                .child(if view.view.is_some() {
                    "\u{2318}-scroll to zoom, scroll to move, double-click to see it all"
                } else {
                    "Click to play from there and hear that version \u{00b7} \u{2318}-scroll to zoom \u{00b7} shift-drag to loop"
                }),
        )
        .on_scroll_wheel(cx.listener(move |this, ev: &ScrollWheelEvent, _, cx| {
            this.lane_scroll(ev, line, cx)
        }))
        .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
            this.lane_move(ev.position, ev.pressed_button == Some(MouseButton::Left), cx)
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _: &MouseUpEvent, _, cx| this.release(cx)),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|this, _: &MouseUpEvent, _, cx| this.release(cx)),
        )
        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
            if !hovered && this.hover.take().is_some() {
                cx.notify();
            }
        }))
        .into_any_element()
}
