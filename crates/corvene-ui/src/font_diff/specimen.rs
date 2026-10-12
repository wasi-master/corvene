//! The Specimen tab: sample text set in both versions. Stacked (old above
//! new), side by side, overlaid (old in red under new), swiped, or
//! highlighted (ink only the old version has in red, only the new one has
//! in green). A waterfall sets one line at many sizes. A variable font gets
//! a slider per axis and its named instances.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use corvene_core::Dispatcher;
use corvene_fonts::Tag;
use corvene_fonts::compare::{FaceDiff, Status};
use corvene_fonts::render::{Raster, TextBlock, TextOptions, highlight, render_text};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::widgets::IconButtonA11y;

use super::raster::{Drawn, RasterKey, image_el, ink, ink_alpha};
use super::{FontDiffView, segmented};
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;

const PANGRAM: &str =
    "The quick brown fox jumps over the lazy dog. Sphinx of black quartz, judge my vow! 0123456789";
const UPPER: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const LOWER: &str = "abcdefghijklmnopqrstuvwxyz";
const FIGURES: &str = "0123456789 ½ ¼ ¾ +−×÷=≠<>%‰ $€£¥ .,:;!?¡¿ ‘’“” ()[]{} /\\|@&#*-–—_";
const PARAGRAPH: &str = "Typography is the craft of endowing human language with a durable visual form. \
Every letter is drawn to work with the others: the counters of a, e and o share a rhythm, the stems \
of h, n and m keep an even colour, and the spacing between them decides whether a line reads as words \
or as a row of shapes.";
/// Waterfall sizes, points.
const WATERFALL: [f32; 9] = [10., 12., 14., 18., 24., 32., 48., 64., 96.];

/// How the two versions are shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(super) enum Mode {
    #[default]
    Stacked,
    SideBySide,
    Overlay,
    Swipe,
    Highlight,
}

const MODES: [Mode; 5] = [
    Mode::Stacked,
    Mode::SideBySide,
    Mode::Overlay,
    Mode::Swipe,
    Mode::Highlight,
];
const MODE_LABELS: [&str; 5] = ["Stacked", "Side by side", "Overlay", "Swipe", "Highlight"];

#[derive(Default)]
pub(super) struct SpecimenUi {
    mode: Mode,
    waterfall: bool,
    /// User-space axis values (empty until the face's axes are read).
    axes: Vec<(Tag, f32)>,
    sliders: Vec<Entity<SliderState>>,
    size: Option<Entity<SliderState>>,
    swipe: Option<Entity<SliderState>>,
    width: Rc<Cell<Pixels>>,
}

/// The characters of the glyphs that changed, for the "Changed" preset.
fn changed_text(face: &FaceDiff) -> String {
    let mut out = String::new();
    for g in face
        .glyphs
        .iter()
        .filter(|g| g.status.is_change() && g.status != Status::Removed)
    {
        if let Some(c) = g.codepoints.first().copied().and_then(super::printable) {
            out.push(c);
        }
        if out.chars().count() > 120 {
            break;
        }
    }
    if out.is_empty() {
        "No changed glyph has a character".to_string()
    } else {
        out
    }
}

impl FontDiffView {
    fn specimen_sliders(&mut self, face: &FaceDiff, cx: &mut Context<Self>) {
        let size = corvene_core::AppState::try_global(cx)
            .map(|s| s.read(cx).settings.font_specimen_size)
            .unwrap_or(48.);
        let ui = &mut self.ui.entry(self.face).or_default().specimen;
        if ui.size.is_none() {
            let slider = cx.new(|_| {
                SliderState::new()
                    .min(8.)
                    .max(200.)
                    .step(1.)
                    .default_value(size)
            });
            cx.subscribe(&slider, |_, s, ev: &SliderEvent, cx| {
                if let SliderEvent::Release(_) = ev {
                    Dispatcher::set_font_specimen_size(s.read(cx).value().start(), cx);
                }
                cx.notify();
            })
            .detach();
            ui.size = Some(slider);
        }
        if ui.swipe.is_none() {
            let slider = cx.new(|_| {
                SliderState::new()
                    .min(0.)
                    .max(100.)
                    .step(0.1)
                    .default_value(50.)
            });
            cx.subscribe(&slider, |_, _, _: &SliderEvent, cx| cx.notify())
                .detach();
            ui.swipe = Some(slider);
        }
        let axes = face
            .new
            .as_ref()
            .or(face.old.as_ref())
            .map(|r| r.axes.clone())
            .unwrap_or_default();
        if ui.sliders.len() != axes.len() {
            ui.axes = axes
                .iter()
                .map(|a| {
                    (
                        Tag::new_checked(format!("{:<4}", a.tag).as_bytes())
                            .unwrap_or(Tag::new(b"wght")),
                        a.default,
                    )
                })
                .collect();
            ui.sliders = axes
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    let slider = cx.new(|_| {
                        SliderState::new()
                            .min(a.min)
                            .max(a.max.max(a.min + 0.001))
                            .step(((a.max - a.min) / 1000.).max(0.01))
                            .default_value(a.default)
                    });
                    cx.subscribe(&slider, move |this, s, _: &SliderEvent, cx| {
                        let v = s.read(cx).value().start();
                        if let Some(axis) = this.face_ui().specimen.axes.get_mut(i) {
                            axis.1 = v;
                        }
                        cx.notify();
                    })
                    .detach();
                    slider
                })
                .collect();
        }
    }

    /// The text to set: the user's own, or the pangram.
    fn specimen_text(&self, cx: &App) -> String {
        let text = self
            .inputs
            .as_ref()
            .map(|i| i.specimen.read(cx).value().to_string())
            .unwrap_or_default();
        if text.trim().is_empty() {
            PANGRAM.to_string()
        } else {
            text
        }
    }

    /// The text of one side set at `width` points.
    #[allow(clippy::too_many_arguments)]
    fn specimen_raster(
        &mut self,
        face: &FaceDiff,
        new_side: bool,
        text: &str,
        size: f32,
        waterfall: bool,
        width: f32,
        color: Ink2,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Drawn> {
        let report = if new_side {
            face.new.clone()
        } else {
            face.old.clone()
        }?;
        let decoded = self.decoded(new_side)?;
        let scale = window.scale_factor();
        let axes = self.face_ui().specimen.axes.clone();
        let key = RasterKey::of((
            "specimen",
            self.face,
            new_side,
            text,
            size.to_bits(),
            waterfall,
            (width * scale) as u32,
            color.0,
            axes.iter()
                .map(|(t, v)| (t.to_be_bytes(), v.to_bits()))
                .collect::<Vec<_>>(),
            scale.to_bits(),
        ));
        let index = report.index;
        let text = text.to_string();
        let ink = color.1;
        self.rasters.get(
            key,
            scale,
            move || {
                let font = decoded.face(index).ok()?;
                let blocks: Vec<TextBlock> = if waterfall {
                    let line = text.lines().next().unwrap_or_default().to_string();
                    WATERFALL
                        .iter()
                        .map(|s| TextBlock {
                            text: line.clone(),
                            size: s * scale,
                        })
                        .collect()
                } else {
                    vec![TextBlock {
                        text: text.clone(),
                        size: size * scale,
                    }]
                };
                Some(
                    render_text(
                        &font,
                        &blocks,
                        &TextOptions {
                            features: Vec::new(),
                            variations: axes,
                            width: (width * scale).max(1.),
                            line_height: 1.25,
                            block_gap: 6. * scale,
                            padding: 8. * scale,
                            ink,
                        },
                    )
                    .raster,
                )
            },
            cx,
        )
    }

    pub(super) fn specimen(
        &mut self,
        face: &Arc<FaceDiff>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.specimen_sliders(face, cx);
        let t = cx.ghd().clone();
        let both = face.old.is_some() && face.new.is_some();
        let (mode, waterfall, width_cell, size_slider, swipe_slider, sliders) = {
            let ui = &self.face_ui().specimen;
            (
                if both { ui.mode } else { Mode::Stacked },
                ui.waterfall,
                ui.width.clone(),
                ui.size.clone(),
                ui.swipe.clone(),
                ui.sliders.clone(),
            )
        };
        let size = size_slider
            .as_ref()
            .map(|s| s.read(cx).value().start())
            .unwrap_or(48.);
        let text = self.specimen_text(cx);
        let width = f32::from(width_cell.get()).max(200.);
        let column = if mode == Mode::SideBySide {
            (width - 12.) / 2.
        } else {
            width
        };
        let ink_color = Ink2::of(t.text, 1.);
        // the drawings
        let stage: AnyElement = {
            let old = |this: &mut Self, c: Ink2, w, cx: &mut Context<Self>| {
                this.specimen_raster(face, false, &text, size, waterfall, w, c, window, cx)
            };
            let new = |this: &mut Self, c: Ink2, w, cx: &mut Context<Self>| {
                this.specimen_raster(face, true, &text, size, waterfall, w, c, window, cx)
            };
            let label = |s: &'static str| {
                div()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(s)
            };
            let placeholder = || div().h(zpx(40.)).into_any_element();
            let shown = |d: Option<Drawn>| {
                d.map(|d| image_el(&d).into_any_element())
                    .unwrap_or_else(placeholder)
            };
            match mode {
                _ if !both => {
                    let side = face.new.is_some();
                    let d = if side {
                        new(self, ink_color, column, cx)
                    } else {
                        old(self, ink_color, column, cx)
                    };
                    div().child(shown(d)).into_any_element()
                }
                Mode::Stacked => {
                    let (o, n) = (
                        old(self, ink_color, column, cx),
                        new(self, ink_color, column, cx),
                    );
                    div()
                        .flex()
                        .flex_col()
                        .gap(SPACING_HALF())
                        .child(label("Before"))
                        .child(
                            div()
                                .rounded(BORDER_RADIUS())
                                .bg(t.diff_delete_background.opacity(0.35))
                                .child(shown(o)),
                        )
                        .child(label("After"))
                        .child(
                            div()
                                .rounded(BORDER_RADIUS())
                                .bg(t.diff_add_background.opacity(0.35))
                                .child(shown(n)),
                        )
                        .into_any_element()
                }
                Mode::SideBySide => {
                    let (o, n) = (
                        old(self, ink_color, column, cx),
                        new(self, ink_color, column, cx),
                    );
                    div()
                        .flex()
                        .flex_row()
                        .gap(zpx(12.))
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap(SPACING_HALF())
                                .child(label("Before"))
                                .child(shown(o)),
                        )
                        .child(div().w(px(1.)).bg(t.box_border))
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap(SPACING_HALF())
                                .child(label("After"))
                                .child(shown(n)),
                        )
                        .into_any_element()
                }
                Mode::Overlay => {
                    let o = old(self, Ink2::of(t.color_deleted, 0.9), column, cx);
                    let n = new(self, Ink2::of(t.text, 0.75), column, cx);
                    div()
                        .relative()
                        .child(shown(o))
                        .children(n.map(|n| div().absolute().top_0().left_0().child(image_el(&n))))
                        .into_any_element()
                }
                Mode::Swipe => {
                    let o = old(self, ink_color, column, cx);
                    let n = new(self, ink_color, column, cx);
                    let pct = swipe_slider
                        .as_ref()
                        .map(|s| s.read(cx).value().start())
                        .unwrap_or(50.)
                        / 100.;
                    let height = o
                        .as_ref()
                        .map(|d| d.height)
                        .unwrap_or_default()
                        .max(n.as_ref().map(|d| d.height).unwrap_or_default());
                    let split = px(column * pct);
                    div()
                        .flex()
                        .flex_col()
                        .gap(SPACING_HALF())
                        .children(
                            swipe_slider
                                .map(|s| div().w(px(column)).child(Slider::new(&s).horizontal())),
                        )
                        .child(
                            div()
                                .relative()
                                .w(px(column))
                                .h(height)
                                .child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .left_0()
                                        .children(o.map(|o| image_el(&o))),
                                )
                                .child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .left(split)
                                        .w(px(column) - split)
                                        .h(height)
                                        .overflow_hidden()
                                        .bg(t.background)
                                        .child(
                                            div()
                                                .absolute()
                                                .top_0()
                                                .left(-split)
                                                .children(n.map(|n| image_el(&n))),
                                        ),
                                )
                                .child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .left(split)
                                        .w(px(1.))
                                        .h(height)
                                        .bg(t.focus),
                                ),
                        )
                        .into_any_element()
                }
                Mode::Highlight => {
                    let o = old(self, ink_color, column, cx);
                    let n = new(self, ink_color, column, cx);
                    let drawn = match (o, n) {
                        (Some(o), Some(n)) => {
                            let scale = window.scale_factor();
                            let key = RasterKey::of((
                                "highlight",
                                o.image.id.0,
                                n.image.id.0,
                                t.color_new.to_rgb().g.to_bits(),
                            ));
                            let (removed, added, same) = (
                                ink(t.color_deleted),
                                ink(t.color_new),
                                ink(t.text.opacity(0.85)),
                            );
                            let (a, b): (Arc<Raster>, Arc<Raster>) =
                                (o.raster.clone(), n.raster.clone());
                            self.rasters.get(
                                key,
                                scale,
                                move || Some(highlight(&a, &b, removed, added, same)),
                                cx,
                            )
                        }
                        _ => None,
                    };
                    div()
                        .flex()
                        .flex_col()
                        .gap(SPACING_HALF())
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .gap(SPACING())
                                .text_size(FONT_SIZE_SM())
                                .child(legend(t.color_deleted, "Only before"))
                                .child(legend(t.color_new, "Only after"))
                                .child(legend(t.text, "Both")),
                        )
                        .child(shown(drawn))
                        .into_any_element()
                }
            }
        };
        let mode_index = MODES.iter().position(|m| *m == mode).unwrap_or(0);
        let view = cx.entity().downgrade();
        let presets: [(&'static str, String); 6] = [
            ("Pangram", String::new()),
            ("Changed", changed_text(face)),
            ("A–Z", UPPER.to_string()),
            ("a–z", LOWER.to_string()),
            ("Figures", FIGURES.to_string()),
            ("Paragraph", PARAGRAPH.to_string()),
        ];
        let instances = face
            .new
            .as_ref()
            .or(face.old.as_ref())
            .map(|r| r.instances.clone())
            .unwrap_or_default();
        let axes_info = face
            .new
            .as_ref()
            .or(face.old.as_ref())
            .map(|r| r.axes.clone())
            .unwrap_or_default();
        let controls =
            div()
                .flex_none()
                .flex()
                .flex_col()
                .gap(SPACING_HALF())
                .p(SPACING())
                .border_b_1()
                .border_color(t.box_border)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(SPACING())
                        .children(self.inputs.as_ref().map(|i| {
                            div().flex_1().child(crate::widgets::text_box(
                                "font-specimen-text",
                                &i.specimen,
                                None,
                                window,
                                cx,
                            ))
                        }))
                        .when(both, |d| {
                            let view = view.clone();
                            d.child(segmented(
                                "font-specimen-mode",
                                MODE_LABELS.map(SharedString::from).to_vec(),
                                mode_index,
                                move |i, _, cx| {
                                    view.update(cx, |v, cx| {
                                        v.face_ui().specimen.mode = MODES[i];
                                        cx.notify();
                                    })
                                    .ok();
                                },
                                cx,
                            ))
                        }),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(zpx(6.))
                        .children(presets.into_iter().enumerate().map(|(i, (label, text))| {
                            let view = view.clone();
                            let hover = t.box_hover_background;
                            div()
                                .id(("font-specimen-preset", i))
                                .a11y_button(label)
                                .px(SPACING_HALF())
                                .h(zpx(20.))
                                .flex()
                                .items_center()
                                .rounded_full()
                                .border_1()
                                .border_color(t.box_border)
                                .text_size(FONT_SIZE_SM())
                                .cursor_pointer()
                                .hover(move |d| d.bg(hover))
                                .child(label)
                                .on_click(move |_, window, cx| {
                                    view.update(cx, |v, cx| {
                                        if let Some(i) = &v.inputs {
                                            i.specimen.update(cx, |s, cx| {
                                                s.set_value(text.clone(), window, cx)
                                            });
                                        }
                                        Dispatcher::set_font_specimen_text(text.clone(), cx);
                                        cx.notify();
                                    })
                                    .ok();
                                })
                        }))
                        .child(div().flex_1())
                        .child(crate::widgets::checkbox_row(
                            "font-specimen-waterfall",
                            waterfall,
                            "Waterfall",
                            {
                                let view = view.clone();
                                move |on, _, cx| {
                                    view.update(cx, |v, cx| {
                                        v.face_ui().specimen.waterfall = on;
                                        cx.notify();
                                    })
                                    .ok();
                                }
                            },
                            cx,
                        ))
                        .when(!waterfall, |d| {
                            d.child(
                                div()
                                    .text_size(FONT_SIZE_SM())
                                    .text_color(t.text_secondary)
                                    .child(format!("{size:.0} pt")),
                            )
                            .children(
                                size_slider.map(|s| {
                                    div().w(zpx(140.)).child(Slider::new(&s).horizontal())
                                }),
                            )
                        }),
                )
                .when(!sliders.is_empty(), |d| {
                    d.child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .items_center()
                            .gap(SPACING())
                            .children(sliders.iter().zip(&axes_info).enumerate().map(
                                |(i, (s, a))| {
                                    let v = s.read(cx).value().start();
                                    div()
                                        .id(("font-axis", i))
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(zpx(6.))
                                        .child(
                                            div()
                                                .text_size(FONT_SIZE_SM())
                                                .text_color(t.text_secondary)
                                                .child(format!("{} {:.0}", a.name, v)),
                                        )
                                        .child(
                                            div().w(zpx(140.)).child(Slider::new(s).horizontal()),
                                        )
                                },
                            ))
                            .children((!instances.is_empty()).then(|| {
                                div().flex().flex_row().flex_wrap().gap(zpx(4.)).children(
                                    instances.iter().enumerate().map(|(i, inst)| {
                                        let view = view.clone();
                                        let coords = inst.coords.clone();
                                        let hover = t.box_hover_background;
                                        div()
                                            .id(("font-instance", i))
                                            .a11y_button(inst.name.clone())
                                            .px(SPACING_HALF())
                                            .h(zpx(20.))
                                            .flex()
                                            .items_center()
                                            .rounded_full()
                                            .border_1()
                                            .border_color(t.box_border)
                                            .text_size(FONT_SIZE_SM())
                                            .cursor_pointer()
                                            .hover(move |d| d.bg(hover))
                                            .child(inst.name.clone())
                                            .on_click(move |_, window, cx| {
                                                view.update(cx, |v, cx| {
                                                    let ui = &mut v.face_ui().specimen;
                                                    for (k, value) in coords.iter().enumerate() {
                                                        if let Some(axis) = ui.axes.get_mut(k) {
                                                            axis.1 = *value;
                                                        }
                                                        if let Some(s) = ui.sliders.get(k).cloned()
                                                        {
                                                            s.update(cx, |s, cx| {
                                                                s.set_value(*value, window, cx)
                                                            });
                                                        }
                                                    }
                                                    cx.notify();
                                                })
                                                .ok();
                                            })
                                    }),
                                )
                            })),
                    )
                });
        let measure = {
            let cell = width_cell.clone();
            canvas(
                move |bounds, _, _| cell.set(bounds.size.width),
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(controls)
            .child(
                div()
                    .id("font-specimen-stage")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(SPACING())
                    .child(div().relative().w_full().child(measure).child(stage)),
            )
            .into_any_element()
    }
}

/// Ink as a hashable key part and the raster ink.
#[derive(Clone, Copy)]
pub(super) struct Ink2(pub(super) u32, pub(super) corvene_fonts::render::Ink);

impl Ink2 {
    pub(super) fn of(color: Hsla, alpha: f32) -> Self {
        let i = ink_alpha(color, alpha);
        Self(u32::from_be_bytes([i.r, i.g, i.b, i.a]), i)
    }
}

fn legend(color: Hsla, label: &'static str) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(zpx(4.))
        .child(div().size(zpx(9.)).rounded(zpx(2.)).bg(color))
        .child(label)
}
