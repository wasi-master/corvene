//! The timeline under the stage: a filmstrip (video) or the whole
//! waveform (sound), a strip marking where the versions differ, the loop
//! range, the zoomed range of the sound lanes and the playhead. Click or
//! drag to scrub; shift-drag to loop a range.

use std::sync::Arc;

use corvene_core::MediaKind;
use corvene_core::corvene_media::analysis::VideoComparison;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{MediaDiffView, fill_quad, format_time, render_image, time_at, x_of};
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;

const STRIP: f32 = 30.;
const MARKS: f32 = 6.;

impl MediaDiffView {
    /// Thumbnails of the samples taken so far (GPUI images, made once).
    fn thumbnails(&mut self, samples: &VideoComparison) -> [Vec<Option<Arc<RenderImage>>>; 2] {
        for (side, frames) in [&samples.old, &samples.new].into_iter().enumerate() {
            let thumbs = &mut self.thumbs[side];
            thumbs.resize(frames.len(), None);
            for (i, frame) in frames.iter().enumerate() {
                if thumbs[i].is_none()
                    && let Some(frame) = frame
                {
                    thumbs[i] = render_image(frame).map(|s| s.image);
                }
            }
        }
        self.thumbs.clone()
    }

    fn timeline_press(&mut self, position: Point<Pixels>, shift: bool, cx: &mut Context<Self>) {
        let bounds = self.timeline.get();
        let t = time_at(position.x, bounds, (0., self.duration()));
        if shift {
            self.loop_drag = Some(t);
            self.player.set_loop_range(None);
        } else {
            self.scrub = Some(self.player.is_playing());
            self.player.pause();
            self.player.seek(t);
            self.reveal(t);
        }
        cx.notify();
    }

    fn timeline_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let bounds = self.timeline.get();
        let t = time_at(position.x, bounds, (0., self.duration()));
        self.hover = Some(t);
        if let Some(start) = self.loop_drag {
            let range = (start.min(t), start.max(t));
            self.player.set_loop_range(Some(range));
            self.player.set_looping(true);
        } else if self.scrub.is_some() {
            self.player.seek(t);
            self.reveal(t);
        }
        cx.notify();
    }

    /// Ends a scrub or a loop drag.
    pub(super) fn release(&mut self, cx: &mut Context<Self>) {
        if let Some(start) = self.loop_drag.take() {
            if self.player.loop_range().is_none() {
                // a shift-click: just seek
                self.player.seek(start);
            } else if let Some((a, _)) = self.player.loop_range() {
                self.player.seek(a);
            }
        }
        if let Some(was_playing) = self.scrub.take()
            && was_playing
        {
            self.player.play();
        }
        cx.notify();
    }
}

pub(super) fn timeline(view: &mut MediaDiffView, cx: &mut Context<MediaDiffView>) -> AnyElement {
    let t = cx.ghd().clone();
    let duration = view.duration().max(0.001);
    let position = view.player.position();
    let loop_range = view.player.loop_range();
    let zoomed = if view.kind() == MediaKind::Audio {
        view.view
    } else {
        None
    };
    let samples = view.samples.lock().ok().and_then(|s| s.clone());
    let thumbs = samples.as_ref().map(|s| view.thumbnails(s));
    let overview = view.overview();
    let video_marks: Vec<(f64, f64, f32)> = samples
        .as_ref()
        .map(|s| {
            (0..s.len())
                .filter_map(|i| {
                    let d = s.difference[i]?;
                    let w = s.seconds();
                    Some((i as f64 * w, (i + 1) as f64 * w, d))
                })
                .collect()
        })
        .unwrap_or_default();
    let sound_marks = overview
        .as_ref()
        .map(|o| o.changes.clone())
        .unwrap_or_default();
    // how far the analyses are, 0..1 (`None` when done)
    let progress = {
        let mut parts = Vec::new();
        if let Some(s) = &samples
            && s.done < s.len()
        {
            parts.push(s.done as f32 / s.len().max(1) as f32);
        } else if samples.is_none() && !view.samples_done {
            parts.push(0.);
        }
        if view.sound.pending > 0 {
            for p in &view.sound.progress {
                parts.push(f32::from_bits(p.load(std::sync::atomic::Ordering::Relaxed)));
            }
        }
        (!parts.is_empty()).then(|| parts.iter().sum::<f32>() / parts.len() as f32)
    };
    let show_new = view.diff.current.is_some();
    let aspect = view
        .diff
        .current
        .as_ref()
        .or(view.diff.previous.as_ref())
        .and_then(|s| s.info.video.as_ref())
        .map(|v| {
            let (w, h) = v.display_size();
            w as f32 / h.max(1) as f32
        })
        .unwrap_or(16. / 9.);
    let bounds_cell = view.timeline.clone();
    let colors = (
        t.box_alt_background,
        t.color_modified,
        t.accent,
        t.text,
        t.color_deleted,
        t.color_new,
        t.box_border,
    );
    let painter = canvas(
        move |b, _, _| bounds_cell.set(b),
        move |b, _, window, _| {
            let (track, mark, accent, head, old_c, new_c, border) = colors;
            let range = (0., duration);
            let strip = Bounds::new(b.origin, size(b.size.width, zpx(STRIP)));
            window.paint_quad(fill_quad(strip, track, px(3.)));
            window.with_content_mask(Some(ContentMask { bounds: strip }), |window| {
                // the filmstrip: one tile per thumbnail width
                if let (Some(thumbs), Some(samples)) = (&thumbs, &samples) {
                    let side = if show_new { 1 } else { 0 };
                    let tile_w = (zpx(STRIP) * aspect).max(px(8.));
                    let n = (f32::from(b.size.width) / f32::from(tile_w)).ceil() as usize;
                    for k in 0..n {
                        let x = b.left() + tile_w * k as f32;
                        let t = time_at(x + tile_w / 2., b, range);
                        let i = samples.index_at(t);
                        // the nearest sample in that tile that is in
                        let image = (0..samples.len())
                            .filter(|j| thumbs[side].get(*j).is_some_and(|t| t.is_some()))
                            .min_by_key(|j| j.abs_diff(i))
                            .and_then(|j| thumbs[side][j].clone());
                        if let Some(image) = image {
                            let r = Bounds::new(point(x, b.top()), size(tile_w, zpx(STRIP)));
                            window
                                .paint_image(r, r, Corners::default(), image, 0, false)
                                .ok();
                        }
                    }
                }
                // the whole waveform, old faint behind new
                if let Some(o) = &overview {
                    let n = o.columns.max(1);
                    let w = b.size.width / n as f32;
                    let mid = strip.center().y;
                    let h = zpx(STRIP) * 0.45;
                    for (cols, color) in
                        [(&o.old, old_c.opacity(0.35)), (&o.new, new_c.opacity(0.8))]
                    {
                        for (x, col) in cols.iter().enumerate() {
                            let Some((level, _)) = col else { continue };
                            let top = mid - h * level.max.clamp(-1., 1.);
                            let bottom = mid - h * level.min.clamp(-1., 1.);
                            let r = Bounds::new(
                                point(b.left() + w * x as f32, top),
                                size(w.max(px(1.)), (bottom - top).max(px(1.))),
                            );
                            window.paint_quad(fill(r, color));
                        }
                    }
                }
            });
            // where the versions differ
            let marks = Bounds::new(
                point(b.left(), b.top() + zpx(STRIP) + px(3.)),
                size(b.size.width, zpx(MARKS)),
            );
            window.paint_quad(fill_quad(marks, track, px(2.)));
            for (a, z, score) in &video_marks {
                if *score <= 0.01 {
                    continue;
                }
                let x0 = x_of(*a, b, range);
                let x1 = x_of(*z, b, range);
                let alpha = (0.35 + score * 6.).min(1.);
                let r = Bounds::new(
                    point(x0, marks.top()),
                    size((x1 - x0).max(px(2.)), marks.size.height),
                );
                window.paint_quad(fill(r, mark.opacity(alpha)));
            }
            for (a, z) in &sound_marks {
                let x0 = x_of(*a, b, range);
                let x1 = x_of(*z, b, range);
                let r = Bounds::new(
                    point(x0, marks.top()),
                    size((x1 - x0).max(px(2.)), marks.size.height),
                );
                window.paint_quad(fill(r, mark));
            }
            if let Some(p) = progress {
                let r = Bounds::new(
                    point(b.left(), marks.bottom() + px(1.)),
                    size(b.size.width * p, px(2.)),
                );
                window.paint_quad(fill(r, accent.opacity(0.7)));
            }
            // the sound lanes' zoomed range
            if let Some((a, z)) = zoomed {
                let x0 = x_of(a, b, range);
                let x1 = x_of(z, b, range);
                let r = Bounds::new(point(x0, strip.top()), size(x1 - x0, strip.size.height));
                window.paint_quad(
                    fill(r, accent.opacity(0.08))
                        .border_widths(px(1.))
                        .border_color(accent.opacity(0.8)),
                );
            }
            if let Some((a, z)) = loop_range {
                let x0 = x_of(a, b, range);
                let x1 = x_of(z, b, range);
                let r = Bounds::new(point(x0, b.top()), size(x1 - x0, b.size.height));
                window.paint_quad(fill(r, accent.opacity(0.18)));
                for x in [x0, x1] {
                    window.paint_quad(fill(
                        Bounds::new(point(x - px(1.), b.top()), size(px(2.), b.size.height)),
                        accent,
                    ));
                }
            }
            let x = x_of(position, b, range);
            window.paint_quad(fill_quad(
                Bounds::new(
                    point(x - px(1.), b.top() - px(2.)),
                    size(px(2.), b.size.height + px(4.)),
                ),
                head,
                px(1.),
            ));
            window.paint_quad(fill_quad(
                Bounds::new(point(x - px(5.), b.top() - px(4.)), size(px(10.), px(6.))),
                head,
                px(2.),
            ));
            let _ = border;
        },
    )
    .size_full();
    // the time under the pointer
    let hover_label = view.hover.map(|h| {
        let b = view.timeline.get();
        let x = x_of(h, b, (0., duration)) - b.left();
        div()
            .absolute()
            .bottom(zpx(STRIP + MARKS + 10.))
            .left((x - zpx(36.)).max(px(0.)))
            .px(zpx(5.))
            .py(zpx(1.))
            .rounded(BORDER_RADIUS())
            .bg(t.tooltip_background)
            .text_color(t.tooltip_text)
            .text_size(FONT_SIZE_SM())
            .child(format_time(h))
    });
    div()
        .flex_none()
        .px(SPACING())
        .pt(SPACING_HALF())
        .pb(SPACING())
        .child(
            div()
                .id("media-timeline")
                .relative()
                .h(zpx(STRIP + MARKS + 6.))
                .cursor_pointer()
                .child(painter)
                .children(hover_label)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                        this.focus.focus(window, cx);
                        this.timeline_press(ev.position, ev.modifiers.shift, cx);
                    }),
                )
                .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                    let b = this.timeline.get();
                    if ev.pressed_button == Some(MouseButton::Left)
                        && (this.scrub.is_some() || this.loop_drag.is_some())
                    {
                        this.timeline_drag(ev.position, cx);
                    } else if b.contains(&ev.position) {
                        this.hover = Some(time_at(ev.position.x, b, (0., this.duration())));
                        cx.notify();
                    } else if this.hover.take().is_some() {
                        cx.notify();
                    }
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
                })),
        )
        .into_any_element()
}
