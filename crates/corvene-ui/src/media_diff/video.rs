//! The video stage: the current picture of both versions side by side
//! (2-up), split by a divider dragged across them (Swipe), faded into each
//! other (Onion Skin, drag across to fade) or subtracted (Difference).

use corvene_core::corvene_media::player::Side;
use corvene_core::{FileStatusKind, MediaKind, VideoDiffMode};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{MediaDiffView, Shown, format_time, undecodable};
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::GhdTooltip;

/// The largest size of `aspect` (w / h) inside `room`.
fn fit(aspect: f32, room: Size<Pixels>) -> Size<Pixels> {
    let (w, h) = (
        f32::from(room.width).max(1.),
        f32::from(room.height).max(1.),
    );
    if w / h > aspect {
        size(px(h * aspect), px(h))
    } else {
        size(px(w), px(w / aspect))
    }
}

impl MediaDiffView {
    fn side_meta(&self, side: Side) -> Option<&corvene_core::MediaSide> {
        match side {
            Side::Old => self.diff.previous.as_ref(),
            Side::New => self.diff.current.as_ref(),
        }
    }

    fn aspect(&self, side: Side) -> f32 {
        if let Some(s) = &self.shown[side as usize] {
            return s.size.0 as f32 / s.size.1.max(1) as f32;
        }
        self.side_meta(side)
            .and_then(|s| s.info.video.as_ref())
            .map(|v| {
                let (w, h) = v.display_size();
                w as f32 / h.max(1) as f32
            })
            .unwrap_or(16. / 9.)
    }

    /// One version's picture filling `room` (letterboxed on black), or why
    /// there is none.
    fn picture(&self, side: Side, room: Size<Pixels>, cx: &App) -> AnyElement {
        let t = cx.ghd();
        let message = |text: String| {
            div()
                .size_full()
                .p(SPACING())
                .flex()
                .items_center()
                .justify_center()
                .text_center()
                .text_size(FONT_SIZE_SM())
                .text_color(t.text_secondary)
                .bg(t.box_alt_background)
                .child(text)
                .into_any_element()
        };
        if let Some(text) = undecodable(self.side_meta(side), MediaKind::Video) {
            return message(text);
        }
        let ended = self.player.ended(side)
            && self
                .player
                .source(side)
                .is_some_and(|s| self.player.position() > s.duration() + 0.01);
        let size = fit(self.aspect(side), room);
        let position = self.player.position();
        // while a scrub waits for the exact picture: the nearest sample
        let stale = self.shown[side as usize]
            .as_ref()
            .is_none_or(|s: &Shown| (s.time - position).abs() > 0.4);
        let sample = stale
            .then(|| {
                let samples = self.samples.lock().ok()?;
                let i = samples.as_ref()?.index_at(position);
                self.thumbs[side as usize].get(i)?.clone()
            })
            .flatten();
        let image = sample
            .or_else(|| self.shown[side as usize].as_ref().map(|s| s.image.clone()))
            .map(|image| {
                img(image)
                    .w(size.width)
                    .h(size.height)
                    .object_fit(ObjectFit::Contain)
            });
        div()
            .relative()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui_kit::black())
            .children(image)
            .when(ended, |d| {
                let end = self.player.source(side).map_or(0., |s| s.duration());
                d.child(
                    div()
                        .absolute()
                        .bottom(SPACING())
                        .px(zpx(6.))
                        .py(zpx(2.))
                        .rounded(BORDER_RADIUS())
                        .bg(gpui_kit::black().opacity(0.7))
                        .text_color(gpui_kit::white())
                        .text_size(FONT_SIZE_SM())
                        .child(format!("Ended at {}", format_time(end))),
                )
            })
            .into_any_element()
    }

    fn caption(&self, side: Side, cx: &App) -> String {
        let _ = cx;
        let Some(meta) = self.side_meta(side) else {
            return String::new();
        };
        let mut parts = Vec::new();
        if let Some(v) = &meta.info.video {
            let (w, h) = v.display_size();
            parts.push(format!("{w}\u{00d7}{h}"));
            if !v.codec.is_empty() {
                parts.push(v.codec.clone());
            }
            if let Some(fps) = v.fps() {
                parts.push(format!("{} fps", trim(fps)));
            }
        }
        parts.push(crate::format::format_bytes(meta.bytes as i64, 1));
        parts.join(" \u{00b7} ")
    }

    /// Drag across the picture: the swipe divider or the onion skin mix.
    fn mix_at(&mut self, x: Pixels, mode: VideoDiffMode, cx: &mut Context<Self>) {
        let b = self.stage.get();
        let room = size(b.size.width, (b.size.height - zpx(22.)).max(px(0.)));
        let boxed = fit(self.aspect(Side::New).max(0.01), room);
        let w = f32::from(boxed.width);
        if w <= 0. {
            return;
        }
        let left = b.left() + (b.size.width - boxed.width) / 2.;
        let f = (f32::from(x - left) / w).clamp(0., 1.);
        match mode {
            VideoDiffMode::Swipe => self.swipe = f,
            VideoDiffMode::OnionSkin => self.onion = f,
            _ => return,
        }
        cx.notify();
    }
}

fn trim(fps: f64) -> String {
    let text = format!("{fps:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

pub(super) fn stage(
    view: &mut MediaDiffView,
    mode: VideoDiffMode,
    cx: &mut Context<MediaDiffView>,
) -> AnyElement {
    let t = cx.ghd().clone();
    let stage = view.stage.get();
    let scale = view.scale_factor;
    let single = !view.compares();
    // the device pixels one picture covers
    let per_picture = if mode == VideoDiffMode::TwoUp && !single {
        size(stage.size.width / 2., stage.size.height - zpx(44.))
    } else {
        stage.size
    };
    view.player.set_frame_box((
        (f32::from(per_picture.width) * scale).max(16.) as u32,
        (f32::from(per_picture.height) * scale).max(16.) as u32,
    ));
    let measure = {
        let cell = view.stage.clone();
        let entity = cx.entity();
        canvas(
            move |b, window, _| {
                if cell.get() != b {
                    cell.set(b);
                    window.on_next_frame(move |_, cx| entity.update(cx, |_, cx| cx.notify()));
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    };
    let label = |text: &str, color: Hsla| {
        div()
            .flex_none()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(color)
            .child(text.to_string())
    };
    let body: AnyElement = if single || mode == VideoDiffMode::TwoUp {
        let room = size(
            if single {
                stage.size.width
            } else {
                (stage.size.width - SPACING()) / 2.
            },
            (stage.size.height - zpx(44.)).max(px(0.)),
        );
        let column = |side: Side, title: &str, color: Hsla, cx: &App| {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .items_center()
                .gap(zpx(4.))
                .child(label(title, color))
                .child(
                    div()
                        .w(room.width)
                        .h(room.height)
                        .border_1()
                        .border_color(color)
                        .child(view.picture(side, room, cx)),
                )
                .child(
                    div()
                        .text_size(FONT_SIZE_SM())
                        .text_color(t.text_secondary)
                        .child(view.caption(side, cx)),
                )
        };
        let mut row = div()
            .size_full()
            .flex()
            .flex_row()
            .justify_center()
            .gap(SPACING());
        if single {
            let (side, title, color) =
                if view.diff.current.is_some() && view.status != FileStatusKind::Deleted {
                    (Side::New, "Added", t.color_new)
                } else {
                    (Side::Old, "Deleted", t.color_deleted)
                };
            row = row.child(column(side, title, color, cx));
        } else {
            row = row
                .child(column(Side::Old, "Old", t.color_deleted, cx))
                .child(column(Side::New, "New", t.color_new, cx));
        }
        row.into_any_element()
    } else {
        // one box both pictures share
        let room = size(stage.size.width, (stage.size.height - zpx(22.)).max(px(0.)));
        let boxed = fit(view.aspect(Side::New).max(0.01), room);
        let content: AnyElement = match mode {
            VideoDiffMode::Swipe => {
                let split = boxed.width * view.swipe;
                div()
                    .relative()
                    .w(boxed.width)
                    .h(boxed.height)
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .child(view.picture(Side::Old, boxed, cx)),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .h(boxed.height)
                            .w(boxed.width - split)
                            .overflow_hidden()
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .right_0()
                                    .w(boxed.width)
                                    .h(boxed.height)
                                    .child(view.picture(Side::New, boxed, cx)),
                            ),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left(split - px(1.))
                            .w(px(2.))
                            .h(boxed.height)
                            .bg(gpui_kit::white()),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(boxed.height / 2. - zpx(12.))
                            .left(split - zpx(12.))
                            .size(zpx(24.))
                            .rounded_full()
                            .bg(gpui_kit::white())
                            .border_1()
                            .border_color(t.box_border)
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(gpui_kit::black())
                            .text_size(FONT_SIZE_SM())
                            .child("\u{2194}"),
                    )
                    .into_any_element()
            }
            VideoDiffMode::OnionSkin => div()
                .relative()
                .w(boxed.width)
                .h(boxed.height)
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .child(view.picture(Side::Old, boxed, cx)),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .opacity(view.onion)
                        .child(view.picture(Side::New, boxed, cx)),
                )
                .into_any_element(),
            _ => {
                let image = view.difference.as_ref().map(|(_, s)| {
                    img(s.image.clone())
                        .w(boxed.width)
                        .h(boxed.height)
                        .object_fit(ObjectFit::Contain)
                });
                div()
                    .w(boxed.width)
                    .h(boxed.height)
                    .bg(gpui_kit::black())
                    .children(image)
                    .into_any_element()
            }
        };
        let hint: AnyElement = match mode {
            VideoDiffMode::Swipe => div()
                .flex()
                .flex_row()
                .gap(SPACING())
                .child(label("Old", t.color_deleted))
                .child(
                    div()
                        .text_color(t.text_secondary)
                        .child("drag across the picture"),
                )
                .child(label("New", t.color_new))
                .into_any_element(),
            VideoDiffMode::OnionSkin => div()
                .flex()
                .flex_row()
                .gap(SPACING())
                .child(label("Old", t.color_deleted))
                .child(div().text_color(t.text_secondary).child(format!(
                    "drag across to fade \u{00b7} {}% new",
                    (view.onion * 100.).round()
                )))
                .child(label("New", t.color_new))
                .into_any_element(),
            _ => div()
                .flex()
                .flex_row()
                .items_center()
                .gap(SPACING())
                .child(
                    div()
                        .text_color(t.text_secondary)
                        .child("Black where the pictures are the same"),
                )
                .child(
                    div()
                        .id("media-amplify")
                        .px(zpx(6.))
                        .py(zpx(1.))
                        .rounded(BORDER_RADIUS())
                        .border_1()
                        .border_color(if view.amplify { t.accent } else { t.box_border })
                        .text_color(if view.amplify { t.accent } else { t.text })
                        .cursor_pointer()
                        .hover(|d| d.bg(t.box_hover_background))
                        .ghd_tooltip("Brighten small differences four times")
                        .child("Amplify \u{00d7}4")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.amplify = !this.amplify;
                            cx.notify();
                        })),
                )
                .into_any_element(),
        };
        let draggable = matches!(mode, VideoDiffMode::Swipe | VideoDiffMode::OnionSkin);
        div()
            .id("media-overlay")
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .gap(zpx(4.))
            .text_size(FONT_SIZE_SM())
            .child(hint)
            .child(content)
            .when(draggable, |d| {
                d.cursor(CursorStyle::ResizeLeftRight)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            this.mix_at(ev.position.x, mode, cx)
                        }),
                    )
                    .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                        if ev.pressed_button == Some(MouseButton::Left) {
                            this.mix_at(ev.position.x, mode, cx)
                        }
                    }))
            })
            .into_any_element()
    };
    div()
        .relative()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .mx(SPACING())
        .child(measure)
        .child(body)
        .into_any_element()
}
