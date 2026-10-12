//! Corvene `1330-font-source-preview`: a font source shown as glyphs after
//! the diff header's Text / Glyphs switch. GitHub Desktop only has the text
//! diff for these files (`app/src/ui/diff/text-diff.tsx`).
//!
//! - A UFO glyph (`.glif`): the old and new outlines overlaid or side by
//!   side, with points, handles and anchors, and what changed.
//! - A Glyphs file (`.glyphs`): its changed glyphs per master in a grid
//!   (the inspector as above for the one selected), and its font info,
//!   masters, axes, instances, features and kerning.
//! - A designspace: its axes, sources, instances and rules.

use std::collections::BTreeMap;
use std::sync::Arc;

use corvene_fonts::compare::{Field, Status};
use corvene_fonts::outline::GlyphPath;
use corvene_fonts::render::render_outline;
use corvene_fonts::sources::{
    DesignspaceDiff, GlyphChange, GlyphsDiff, SourceGlyph, diff_designspace, diff_glif,
    diff_glyphs, parse_designspace, parse_glif, parse_glyphs,
};
use corvene_models::{FontSourceBlob, FontSourceKind};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::widgets::IconButtonA11y;

use super::raster::{Drawn, RasterCache, RasterHost, RasterKey, image_el};
use super::specimen::Ink2;
use super::{segmented, status_color, status_dot, status_label};
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, GhdTheme};

/// Inspector size, points.
const STAGE: f32 = 320.;
const TILE: f32 = 84.;

enum Model {
    Glif(Arc<GlyphChange>),
    Glyphs {
        diff: Arc<GlyphsDiff>,
        /// master id → (name, ascender, descender) of the newest side.
        masters: Vec<(String, String, f32, f32)>,
        upm: f32,
    },
    Designspace(Arc<DesignspaceDiff>),
}

enum Phase {
    Reading,
    Failed(String),
    Ready(Model),
}

pub struct FontSourceView {
    /// The diff this view reads.
    pub(crate) source: Arc<corvene_models::Diff>,
    phase: Phase,
    rasters: RasterCache,
    /// Glyphs: the master shown, the selected glyph, the pane.
    master: usize,
    selected: Option<usize>,
    info: bool,
    overlay: bool,
    points: bool,
}

impl RasterHost for FontSourceView {
    fn rasters(&mut self) -> &mut RasterCache {
        &mut self.rasters
    }
}

/// A parse failure of a side as text.
fn err(e: corvene_fonts::FontError) -> String {
    e.to_string()
}

fn read(
    kind: FontSourceKind,
    previous: Option<&FontSourceBlob>,
    current: Option<&FontSourceBlob>,
) -> Result<Model, String> {
    Ok(match kind {
        FontSourceKind::Glif => {
            let parse = |b: Option<&FontSourceBlob>| -> Result<Option<SourceGlyph>, String> {
                b.map(|b| parse_glif(&b.bytes, &b.siblings).map_err(err))
                    .transpose()
            };
            let (o, n) = (parse(previous)?, parse(current)?);
            Model::Glif(Arc::new(diff_glif(o.as_ref(), n.as_ref())))
        }
        FontSourceKind::Glyphs => {
            let parse = |b: Option<&FontSourceBlob>| {
                b.map(|b| parse_glyphs(&b.bytes).map_err(err)).transpose()
            };
            let (o, n) = (parse(previous)?, parse(current)?);
            let side = n.as_ref().or(o.as_ref());
            let metric = |m: &corvene_fonts::sources::Master, name: &str| {
                m.metrics
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(name))
                    .map(|(_, v)| *v)
            };
            let upm = side.map(|f| f.units_per_em).unwrap_or(1000.);
            let masters = side
                .map(|f| {
                    f.masters
                        .iter()
                        .map(|m| {
                            (
                                m.id.clone(),
                                m.name.clone(),
                                metric(m, "ascender").unwrap_or(upm * 0.8),
                                metric(m, "descender").unwrap_or(-upm * 0.2),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            Model::Glyphs {
                diff: Arc::new(diff_glyphs(o.as_ref(), n.as_ref())),
                masters,
                upm,
            }
        }
        FontSourceKind::Designspace => {
            let parse = |b: Option<&FontSourceBlob>| {
                b.map(|b| parse_designspace(&b.bytes).map_err(err))
                    .transpose()
            };
            let (o, n) = (parse(previous)?, parse(current)?);
            Model::Designspace(Arc::new(diff_designspace(o.as_ref(), n.as_ref())))
        }
    })
}

/// The vertical frame both versions of a glyph are drawn in: ascender and
/// descender, widened to the outlines' bounds.
fn frame(change: &GlyphChange, asc: f32, desc: f32) -> (f32, f32) {
    let mut top = asc;
    let mut bottom = desc;
    for g in [&change.old, &change.new].into_iter().flatten() {
        if let Some(b) = g.path.bounds() {
            bottom = bottom.min(b[1]);
            top = top.max(b[3]);
        }
        for a in &g.anchors {
            bottom = bottom.min(a.y);
            top = top.max(a.y);
        }
    }
    (top, bottom)
}

impl FontSourceView {
    /// Starts reading the sides of `diff` (a `Diff::FontSource`).
    pub fn new(diff: Arc<corvene_models::Diff>, cx: &mut Context<Self>) -> Self {
        let source = diff.clone();
        let task = cx.background_executor().spawn(async move {
            match &*source {
                corvene_models::Diff::FontSource {
                    kind,
                    previous,
                    current,
                } => read(*kind, previous.as_ref(), current.as_ref()),
                _ => Err("not a font source".to_string()),
            }
        });
        cx.spawn(async move |this, cx| {
            let model = task.await;
            this.update(cx, |this, cx| {
                this.phase = match model {
                    Ok(m) => Phase::Ready(m),
                    Err(e) => Phase::Failed(e),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self {
            source: diff,
            phase: Phase::Reading,
            rasters: RasterCache::default(),
            master: 0,
            selected: None,
            info: false,
            overlay: true,
            points: true,
        }
    }

    /// One side of a glyph drawn `w`×`h` points in the shared frame.
    #[allow(clippy::too_many_arguments)]
    fn outline_image(
        &mut self,
        glyph: &SourceGlyph,
        top: f32,
        bottom: f32,
        w: f32,
        h: f32,
        color: Ink2,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Drawn> {
        let scale = window.scale_factor();
        let (pw, ph) = ((w * scale) as u32, (h * scale) as u32);
        let key = RasterKey::of((
            "source",
            glyph.name.as_str(),
            glyph.path.hash(),
            glyph.advance.to_bits(),
            top.to_bits(),
            bottom.to_bits(),
            pw,
            ph,
            color.key(),
        ));
        let path = glyph.path.clone();
        let advance = glyph.advance;
        let ink = color.ink();
        self.rasters.get(
            key,
            scale,
            move || {
                Some(render_outline(
                    &path, advance, top, bottom, pw, ph, 0.82, ink,
                ))
            },
            cx,
        )
    }

    /// The glyph inspector: outlines, points, anchors, fields.
    fn glyph_inspector(
        &mut self,
        change: &GlyphChange,
        asc: f32,
        desc: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd().clone();
        let (top, bottom) = frame(change, asc, desc);
        let both = change.old.is_some() && change.new.is_some();
        let overlay = self.overlay || !both;
        let w = if overlay { STAGE } else { STAGE / 2. - 4. };
        let mut layers: Vec<(SourceGlyph, Ink2, bool)> = Vec::new();
        if overlay {
            if let (Some(o), true) = (&change.old, both) {
                layers.push((o.clone(), Ink2::of(t.color_deleted, 0.75), false));
            }
            if let Some(n) = &change.new {
                layers.push((
                    n.clone(),
                    Ink2::of(t.text, if both { 0.8 } else { 1. }),
                    true,
                ));
            } else if let Some(o) = &change.old {
                layers.push((o.clone(), Ink2::of(t.text, 1.), false));
            }
        } else {
            for (g, new_side) in [(&change.old, false), (&change.new, true)] {
                if let Some(g) = g {
                    layers.push((g.clone(), Ink2::of(t.text, 1.), new_side));
                }
            }
        }
        let points = self.points;
        let mut cells: Vec<AnyElement> = Vec::new();
        let mut overlay_children: Vec<AnyElement> = Vec::new();
        for (g, ink, new_side) in &layers {
            let drawn = self.outline_image(g, top, bottom, w, w, *ink, window, cx);
            let guides = points.then(|| {
                guides(
                    &g.path, g.advance, &g.anchors, top, bottom, w, *new_side, &t,
                )
            });
            let body = div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .children(drawn.map(|d| image_el(&d)))
                .children(guides);
            if overlay {
                overlay_children.push(body.into_any_element());
            } else {
                cells.push(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .child(
                            div()
                                .relative()
                                .size(px(w))
                                .rounded(zpx(3.))
                                .bg(t.box_alt_background)
                                .child(body),
                        )
                        .child(
                            div()
                                .text_size(FONT_SIZE_SM())
                                .text_color(t.text_secondary)
                                .child(if *new_side { "After" } else { "Before" }),
                        )
                        .into_any_element(),
                );
            }
        }
        let stage = if overlay {
            div()
                .relative()
                .size(px(STAGE))
                .rounded(zpx(3.))
                .bg(t.box_alt_background)
                .children(overlay_children)
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_row()
                .gap(zpx(8.))
                .children(cells)
                .into_any_element()
        };
        let view = cx.entity().downgrade();
        div()
            .flex()
            .flex_col()
            .gap(SPACING())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING_HALF())
                    .child(status_dot(change.status, &t))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(FONT_SIZE_MD())
                            .child(change.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(FONT_SIZE_SM())
                            .text_color(status_color(change.status, &t))
                            .child(status_label(change.status)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING())
                    .when(both, |d| {
                        let view = view.clone();
                        d.child(segmented(
                            "font-source-compare",
                            vec!["Overlay".into(), "Side by side".into()],
                            if self.overlay { 0 } else { 1 },
                            move |i, _, cx| {
                                view.update(cx, |v, cx| {
                                    v.overlay = i == 0;
                                    cx.notify();
                                })
                                .ok();
                            },
                            cx,
                        ))
                    })
                    .child(crate::widgets::checkbox_row(
                        "font-source-points",
                        points,
                        "Points & anchors",
                        move |on, _, cx| {
                            view.update(cx, |v, cx| {
                                v.points = on;
                                cx.notify();
                            })
                            .ok();
                        },
                        cx,
                    )),
            )
            .child(stage)
            .children(
                change
                    .new
                    .as_ref()
                    .or(change.old.as_ref())
                    .filter(|g| !g.unresolved.is_empty())
                    .map(|g| {
                        div()
                            .text_size(FONT_SIZE_SM())
                            .text_color(t.text_secondary)
                            .child(format!("Components not found: {}", g.unresolved.join(", ")))
                    }),
            )
            .child(super::overview::field_grid(
                "font-source-fields",
                &change.fields,
                both,
                &t,
            ))
            .into_any_element()
    }

    fn glyphs_view(
        &mut self,
        diff: &Arc<GlyphsDiff>,
        masters: &[(String, String, f32, f32)],
        upm: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd().clone();
        let view = cx.entity().downgrade();
        let pane = segmented(
            "font-source-pane",
            vec!["Glyphs".into(), "Font info".into()],
            usize::from(self.info),
            {
                let view = view.clone();
                move |i, _, cx| {
                    view.update(cx, |v, cx| {
                        v.info = i == 1;
                        cx.notify();
                    })
                    .ok();
                }
            },
            cx,
        );
        let changed: Vec<usize> = diff
            .glyphs
            .iter()
            .enumerate()
            .filter(|(_, g)| g.status.is_change())
            .map(|(i, _)| i)
            .collect();
        let body: AnyElement =
            if self.info {
                glyphs_info(diff, &t)
            } else {
                let master = masters
                    .get(self.master)
                    .cloned()
                    .unwrap_or_else(|| (String::new(), "Master".into(), upm * 0.8, -upm * 0.2));
                let master_tabs = segmented(
                    "font-source-master",
                    masters
                        .iter()
                        .map(|m| SharedString::from(m.1.clone()))
                        .collect(),
                    self.master,
                    {
                        let view = view.clone();
                        move |i, _, cx| {
                            view.update(cx, |v, cx| {
                                v.master = i;
                                cx.notify();
                            })
                            .ok();
                        }
                    },
                    cx,
                );
                let layer_of = |gi: usize| -> Option<GlyphChange> {
                    let g = diff.glyphs.get(gi)?;
                    g.layers
                        .iter()
                        .find(|(id, _)| *id == master.0)
                        .or_else(|| g.layers.first())
                        .map(|(_, c)| c.clone())
                };
                let mut tiles: Vec<AnyElement> = Vec::new();
                for (k, gi) in changed.iter().enumerate() {
                    let Some(change) = layer_of(*gi) else {
                        continue;
                    };
                    let status = diff.glyphs[*gi].status;
                    let (top, bottom) = frame(&change, master.2, master.3);
                    let h = TILE - 16.;
                    let under = match (&change.old, &change.new) {
                        (Some(o), Some(_)) => self.outline_image(
                            o,
                            top,
                            bottom,
                            TILE,
                            h,
                            Ink2::of(t.color_deleted, 0.7),
                            window,
                            cx,
                        ),
                        _ => None,
                    };
                    let main = match (&change.new, &change.old) {
                        (Some(n), _) => self.outline_image(
                            n,
                            top,
                            bottom,
                            TILE,
                            h,
                            Ink2::of(t.text, 0.85),
                            window,
                            cx,
                        ),
                        (None, Some(o)) => self.outline_image(
                            o,
                            top,
                            bottom,
                            TILE,
                            h,
                            Ink2::of(t.text_secondary, 1.),
                            window,
                            cx,
                        ),
                        _ => None,
                    };
                    let selected = self.selected == Some(*gi);
                    let edge = status_color(status, &t);
                    let gi = *gi;
                    tiles.push(
                        div()
                            .id(("font-source-glyph", k))
                            .a11y_button(diff.glyphs[gi].name.clone())
                            .w(px(TILE))
                            .h(px(TILE))
                            .relative()
                            .rounded(zpx(3.))
                            .border_1()
                            .border_color(if selected { t.focus } else { edge })
                            .overflow_hidden()
                            .cursor_pointer()
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .right_0()
                                    .h(zpx(3.))
                                    .bg(edge),
                            )
                            .child(
                                div()
                                    .relative()
                                    .h(px(h))
                                    .children(under.map(|u| {
                                        div().absolute().top_0().left_0().child(image_el(&u))
                                    }))
                                    .children(main.map(|m| {
                                        div().absolute().top_0().left_0().child(image_el(&m))
                                    })),
                            )
                            .child(
                                div()
                                    .px(zpx(3.))
                                    .text_size(zpx(10.))
                                    .text_color(t.text_secondary)
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(diff.glyphs[gi].name.clone()),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected = Some(gi);
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
                let inspector = self
                    .selected
                    .and_then(layer_of)
                    .map(|change| self.glyph_inspector(&change, master.2, master.3, window, cx));
                div()
                    .size_full()
                    .flex()
                    .flex_row()
                    .child(
                        div()
                            .id("font-source-grid")
                            .flex_1()
                            .min_w_0()
                            .overflow_y_scroll()
                            .p(SPACING())
                            .flex()
                            .flex_col()
                            .gap(SPACING())
                            .when(masters.len() > 1, |d| d.child(master_tabs))
                            .when(tiles.is_empty(), |d| {
                                d.child(
                                    div()
                                        .text_color(t.text_secondary)
                                        .child("No glyph changed."),
                                )
                            })
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .flex_wrap()
                                    .gap(zpx(4.))
                                    .children(tiles),
                            ),
                    )
                    .children(inspector.map(|i| {
                        div()
                            .id("font-source-inspector")
                            .w(zpx(STAGE + 24.))
                            .flex_none()
                            .h_full()
                            .overflow_y_scroll()
                            .p(zpx(12.))
                            .border_l_1()
                            .border_color(t.box_border)
                            .bg(t.box_background)
                            .child(i)
                    }))
                    .into_any_element()
            };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING())
                    .p(SPACING())
                    .border_b_1()
                    .border_color(t.box_border)
                    .child(pane)
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(FONT_SIZE_SM())
                            .text_color(t.text_secondary)
                            .child(format!(
                                "{} of {} glyphs changed",
                                changed.len(),
                                diff.glyphs.len()
                            )),
                    ),
            )
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }
}

/// Points, handles, anchors and metric lines over a source outline drawn
/// by `render_outline` in a `w`-point square.
#[allow(clippy::too_many_arguments)]
fn guides(
    path: &GlyphPath,
    advance: f32,
    anchors: &[corvene_fonts::sources::Anchor],
    top: f32,
    bottom: f32,
    w: f32,
    new_side: bool,
    t: &GhdTheme,
) -> AnyElement {
    let span = (top - bottom).max(1.);
    let scale = w * 0.82 / span;
    let baseline = (w - span * scale) / 2. + top * scale;
    let x0 = (w - advance * scale) / 2.;
    let to = move |x: f32, y: f32| point(px(x0 + x * scale), px(baseline - y * scale));
    let marks = path.points();
    let handles = path.handles();
    let anchors: Vec<(f32, f32)> = anchors.iter().map(|a| (a.x, a.y)).collect();
    let on_color = if new_side { t.focus } else { t.color_deleted };
    let off_color = t.text_secondary;
    let line_color = t.text_secondary.opacity(0.6);
    let metric_color = t.color_modified.opacity(0.5);
    let anchor_color = t.color_renamed;
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let o = bounds.origin;
            for x in [0., advance] {
                let p = to(x, 0.);
                window.paint_quad(fill(
                    Bounds::new(point(o.x + p.x, o.y), size(px(1.), bounds.size.height)),
                    metric_color,
                ));
            }
            let p = to(0., 0.);
            window.paint_quad(fill(
                Bounds::new(point(o.x, o.y + p.y), size(bounds.size.width, px(1.))),
                metric_color,
            ));
            for [a, b] in &handles {
                let mut builder = PathBuilder::stroke(px(1.));
                builder.move_to(o + to(a.0, a.1));
                builder.line_to(o + to(b.0, b.1));
                if let Ok(path) = builder.build() {
                    window.paint_path(path, line_color);
                }
            }
            for m in &marks {
                let p = to(m.x, m.y);
                let r = if m.on_curve { px(3.) } else { px(2.5) };
                let b = Bounds::new(point(o.x + p.x - r, o.y + p.y - r), size(r * 2., r * 2.));
                if m.on_curve {
                    window.paint_quad(fill(b, on_color));
                } else {
                    window.paint_quad(
                        fill(b, transparent_black())
                            .corner_radii(r)
                            .border_widths(px(1.))
                            .border_color(off_color),
                    );
                }
            }
            for (x, y) in &anchors {
                let p = to(*x, *y);
                let r = px(4.);
                let b = Bounds::new(point(o.x + p.x - r, o.y + p.y - r), size(r * 2., r * 2.));
                window.paint_quad(
                    fill(b, transparent_black())
                        .border_widths(px(1.5))
                        .border_color(anchor_color),
                );
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

/// Items with fields under a heading each (masters, instances, axes …).
fn items(
    id: &'static str,
    title: &str,
    items: &[(String, Status, Vec<Field>)],
    t: &GhdTheme,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(SPACING_HALF())
        .child(super::overview::section(title, t))
        .when(items.is_empty(), |d| {
            d.child(div().text_color(t.text_secondary).child("None."))
        })
        .children(items.iter().enumerate().map(|(i, (name, status, fields))| {
            div()
                .id((id, i))
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(SPACING_HALF())
                        .child(status_dot(*status, t))
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(name.clone()))
                        .child(
                            div()
                                .text_size(FONT_SIZE_SM())
                                .text_color(status_color(*status, t))
                                .child(status_label(*status)),
                        ),
                )
                .child(super::overview::field_grid(id, fields, true, t))
        }))
        .into_any_element()
}

fn glyphs_info(diff: &GlyphsDiff, t: &GhdTheme) -> AnyElement {
    let features: Vec<(String, Status, Vec<Field>)> = diff
        .features
        .iter()
        .map(|(n, s)| (n.clone(), *s, Vec::new()))
        .collect();
    let mut kerning: BTreeMap<String, Vec<Field>> = BTreeMap::new();
    for (master, l, r, o, n) in &diff.kerning {
        kerning.entry(master.clone()).or_default().push(Field::new(
            format!("{l} {r}"),
            o.map(|v| v.to_string()),
            n.map(|v| v.to_string()),
        ));
    }
    let kerning: Vec<(String, Status, Vec<Field>)> = kerning
        .into_iter()
        .map(|(m, f)| (m, Status::Modified, f))
        .collect();
    div()
        .id("font-source-info")
        .size_full()
        .overflow_y_scroll()
        .p(SPACING())
        .flex()
        .flex_col()
        .gap(SPACING())
        .child(super::overview::section("Font", t))
        .child(super::overview::field_grid(
            "font-source-font",
            &diff.info,
            true,
            t,
        ))
        .child(super::overview::section("Axes", t))
        .child(super::overview::field_grid(
            "font-source-axes",
            &diff.axes,
            true,
            t,
        ))
        .child(items("font-source-masters", "Masters", &diff.masters, t))
        .child(items(
            "font-source-instances",
            "Instances",
            &diff.instances,
            t,
        ))
        .child(items("font-source-features", "Features", &features, t))
        .child(items("font-source-kerning", "Kerning changes", &kerning, t))
        .into_any_element()
}

fn designspace(diff: &DesignspaceDiff, t: &GhdTheme) -> AnyElement {
    div()
        .id("font-source-designspace")
        .size_full()
        .overflow_y_scroll()
        .p(SPACING())
        .flex()
        .flex_col()
        .gap(SPACING())
        .child(items("font-ds-axes", "Axes", &diff.axes, t))
        .child(items("font-ds-sources", "Sources", &diff.sources, t))
        .child(items("font-ds-instances", "Instances", &diff.instances, t))
        .child(items("font-ds-rules", "Rules", &diff.rules, t))
        .into_any_element()
}

impl Render for FontSourceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd().clone();
        let body: AnyElement = match &self.phase {
            Phase::Reading => div()
                .p(SPACING())
                .text_color(t.text_secondary)
                .child("Reading…")
                .into_any_element(),
            Phase::Failed(e) => div()
                .p(SPACING())
                .flex()
                .flex_col()
                .gap(SPACING_HALF())
                .child("This file could not be read as a font source.")
                .child(
                    div()
                        .text_size(FONT_SIZE_SM())
                        .text_color(t.text_secondary)
                        .child(e.clone()),
                )
                .into_any_element(),
            Phase::Ready(Model::Glif(change)) => {
                let change = change.clone();
                div()
                    .id("font-source-glif")
                    .size_full()
                    .overflow_y_scroll()
                    .p(SPACING())
                    .child(self.glyph_inspector(&change, 750., -250., window, cx))
                    .into_any_element()
            }
            Phase::Ready(Model::Glyphs { diff, masters, upm }) => {
                let (diff, masters, upm) = (diff.clone(), masters.clone(), *upm);
                self.glyphs_view(&diff, &masters, upm, window, cx)
            }
            Phase::Ready(Model::Designspace(diff)) => designspace(diff, &t),
        };
        div()
            .id("font-source")
            .flex_1()
            .size_full()
            .min_h_0()
            .min_w_0()
            .bg(t.background)
            .text_color(t.text)
            .text_size(FONT_SIZE())
            .child(body)
    }
}
