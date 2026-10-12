//! The Glyphs tab: every glyph of both versions in a grid, added ones
//! edged green, removed red, changed yellow with the old outline in red
//! under the new one. A filter, a search (a character, `U+0061`,
//! `uni0061` or part of a name) and a tile size narrow it down. Selecting
//! a glyph opens the inspector: the two outlines overlaid, side by side or
//! flipped (Space), with their points, handles and metric lines, and what
//! changed about the glyph, the features that use it and its kerning.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use corvene_core::Dispatcher;
use corvene_fonts::compare::{FaceDiff, Field, GlyphRow, Status};
use corvene_fonts::render::{render_glyph, tile};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::widgets::IconButtonA11y;

use super::raster::{Drawn, RasterKey, image_el};
use super::specimen::Ink2;
use super::{FontDiffView, codepoint_label, segmented, status_color};
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;

/// Which glyphs the grid lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GlyphFilter {
    All,
    #[default]
    Changed,
    Added,
    Removed,
    Unchanged,
}

impl GlyphFilter {
    fn keeps(self, status: Status) -> bool {
        match self {
            GlyphFilter::All => true,
            GlyphFilter::Changed => status.is_change(),
            GlyphFilter::Added => status == Status::Added,
            GlyphFilter::Removed => status == Status::Removed,
            GlyphFilter::Unchanged => status == Status::Unchanged,
        }
    }
}

const FILTERS: [GlyphFilter; 5] = [
    GlyphFilter::All,
    GlyphFilter::Changed,
    GlyphFilter::Added,
    GlyphFilter::Removed,
    GlyphFilter::Unchanged,
];

/// How the inspector shows the two outlines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Compare {
    #[default]
    Overlay,
    SideBySide,
    Flip,
}

/// The inspector's glyph size, points.
const INSPECTOR: f32 = 300.;
/// Height of a tile's label line.
const LABEL: f32 = 16.;

#[derive(Default)]
pub(super) struct GlyphsUi {
    pub(super) filter: GlyphFilter,
    /// Index into the face's glyph rows.
    pub(super) selected: Option<usize>,
    scroll: UniformListScrollHandle,
    width: Rc<Cell<Pixels>>,
    compare: Compare,
    points: bool,
    metrics: bool,
    /// Flip shows the old outline.
    flipped: bool,
    /// (filter, query, rows) of the last filtering.
    listed: Option<(GlyphFilter, String, Rc<Vec<usize>>)>,
    tile: Option<Entity<SliderState>>,
    /// Columns of the last layout, for keyboard moves.
    columns: usize,
}

/// A search: a single character or code point, or part of a name.
enum Query {
    Codepoint(u32),
    Name(String),
}

fn parse_query(q: &str) -> Option<Query> {
    let q = q.trim();
    if q.is_empty() {
        return None;
    }
    let mut chars = q.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Some(Query::Codepoint(c as u32));
    }
    let lower = q.to_ascii_lowercase();
    for prefix in ["u+", "uni", "0x", "u"] {
        if let Some(hex) = lower.strip_prefix(prefix)
            && (4..=6).contains(&hex.len())
            && let Ok(cp) = u32::from_str_radix(hex, 16)
        {
            return Some(Query::Codepoint(cp));
        }
    }
    Some(Query::Name(lower))
}

fn matches(row: &GlyphRow, q: &Query) -> bool {
    match q {
        Query::Codepoint(cp) => row.codepoints.contains(cp),
        Query::Name(n) => {
            row.name.to_ascii_lowercase().contains(n.as_str())
                || row
                    .old_name
                    .as_ref()
                    .is_some_and(|o| o.to_ascii_lowercase().contains(n.as_str()))
        }
    }
}

impl FontDiffView {
    /// The glyph rows the grid lists (cached per filter and query).
    fn listed(&mut self, face: &FaceDiff, cx: &App) -> Rc<Vec<usize>> {
        let query = self
            .inputs
            .as_ref()
            .map(|i| i.glyph_search.read(cx).value().to_string())
            .unwrap_or_default();
        let ui = &mut self.ui.entry(self.face).or_default().glyphs;
        if let Some((f, q, rows)) = &ui.listed
            && *f == ui.filter
            && *q == query
        {
            return rows.clone();
        }
        let parsed = parse_query(&query);
        let rows: Rc<Vec<usize>> = Rc::new(
            face.glyphs
                .iter()
                .enumerate()
                // a search looks through every glyph
                .filter(|(_, g)| match &parsed {
                    Some(q) => matches(g, q),
                    None => ui.filter.keeps(g.status),
                })
                .map(|(i, _)| i)
                .collect(),
        );
        ui.listed = Some((ui.filter, query, rows.clone()));
        rows
    }

    /// A glyph of one side drawn `w`×`h` points.
    #[allow(clippy::too_many_arguments)]
    fn glyph_image(
        &mut self,
        face: &FaceDiff,
        new_side: bool,
        gid: u32,
        w: f32,
        h: f32,
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
        let (pw, ph) = ((w * scale).round() as u32, (h * scale).round() as u32);
        let key = RasterKey::of(("glyph", self.face, new_side, gid, pw, ph, color.key()));
        let index = report.index;
        let ink = color.ink();
        self.rasters.get(
            key,
            scale,
            move || {
                let font = decoded.face(index).ok()?;
                let t = tile(&font, &[], pw.max(1), ph.max(1), 0.8);
                Some(render_glyph(&font, gid, &[], t, ink))
            },
            cx,
        )
    }

    /// One tile of the grid.
    fn glyph_tile(
        &mut self,
        face: &FaceDiff,
        row_ix: usize,
        size: f32,
        selected: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd().clone();
        let Some(row) = face.glyphs.get(row_ix).cloned() else {
            return div().into_any_element();
        };
        let h = size - LABEL;
        let under = match (row.status, row.old) {
            (Status::Modified, Some(old))
                if row.changes.0
                    & !(corvene_fonts::compare::GlyphChanges::NAME
                        | corvene_fonts::compare::GlyphChanges::CODEPOINTS)
                    != 0 =>
            {
                self.glyph_image(
                    face,
                    false,
                    old,
                    size,
                    h,
                    Ink2::of(t.color_deleted, 0.7),
                    window,
                    cx,
                )
            }
            _ => None,
        };
        let main = match (row.new, row.old) {
            (Some(g), _) => self.glyph_image(
                face,
                true,
                g,
                size,
                h,
                Ink2::of(t.text, if under.is_some() { 0.85 } else { 1. }),
                window,
                cx,
            ),
            (None, Some(g)) => self.glyph_image(
                face,
                false,
                g,
                size,
                h,
                Ink2::of(t.text_secondary, 1.),
                window,
                cx,
            ),
            _ => None,
        };
        let edge = match row.status {
            Status::Unchanged => t.box_border,
            s => status_color(s, &t),
        };
        let label = row
            .codepoints
            .first()
            .map(|cp| codepoint_label(*cp))
            .unwrap_or_else(|| row.name.clone());
        let tip = {
            let mut s = row.name.clone();
            if let Some(old) = &row.old_name {
                s.push_str(&format!(" (was {old})"));
            }
            for cp in &row.codepoints {
                s.push_str(&format!(" · {}", codepoint_label(*cp)));
            }
            s.push_str(&format!(" · {}", super::status_label(row.status)));
            let labels = row.changes.labels();
            if !labels.is_empty() {
                s.push_str(&format!(": {}", labels.join(", ")));
            }
            s
        };
        let hover = t.box_hover_background;
        div()
            .id(("font-glyph", row_ix))
            .a11y_button(tip.clone())
            .flex_none()
            .w(px(size))
            .h(px(size))
            .relative()
            .flex()
            .flex_col()
            .rounded(zpx(3.))
            .border_1()
            .border_color(if selected { t.focus } else { edge })
            .when(selected, |d| d.bg(t.box_selected_background))
            .when(!selected, |d| d.hover(move |d| d.bg(hover)))
            .when(row.status != Status::Unchanged, |d| {
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(zpx(3.))
                        .bg(edge),
                )
            })
            .cursor_pointer()
            .overflow_hidden()
            .child(
                div()
                    .relative()
                    .h(px(h))
                    .children(under.map(|u| div().absolute().top_0().left_0().child(image_el(&u))))
                    .children(main.map(|m| div().absolute().top_0().left_0().child(image_el(&m)))),
            )
            .child(
                div()
                    .h(px(LABEL - 2.))
                    .px(zpx(3.))
                    .text_size(zpx(10.))
                    .text_color(t.text_secondary)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(label),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.face_ui().glyphs.selected = Some(row_ix);
                this.focus_handle.focus(window, cx);
                cx.notify();
            }))
            .into_any_element()
    }

    fn move_glyph(&mut self, by: isize, cx: &mut Context<Self>) {
        let Some(face) = self.current_face() else {
            return;
        };
        let rows = self.listed(&face, cx);
        if rows.is_empty() {
            return;
        }
        let ui = &mut self.face_ui().glyphs;
        let at = ui
            .selected
            .and_then(|s| rows.iter().position(|r| *r == s))
            .map(|p| p as isize + by)
            .unwrap_or(0)
            .clamp(0, rows.len() as isize - 1) as usize;
        ui.selected = Some(rows[at]);
        let columns = ui.columns.max(1);
        ui.scroll
            .scroll_to_item(at / columns, ScrollStrategy::Nearest);
        cx.notify();
    }

    pub(super) fn glyphs(
        &mut self,
        face: &Arc<FaceDiff>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd().clone();
        let size = corvene_core::AppState::try_global(cx)
            .map(|s| s.read(cx).settings.font_glyph_tile)
            .unwrap_or(72.);
        if self.face_ui().glyphs.tile.is_none() {
            let slider = cx.new(|_| {
                SliderState::new()
                    .min(40.)
                    .max(200.)
                    .step(4.)
                    .default_value(size)
            });
            cx.subscribe(&slider, |this, s, ev: &SliderEvent, cx| {
                if let SliderEvent::Release(_) = ev {
                    Dispatcher::set_font_glyph_tile(s.read(cx).value().start(), cx);
                }
                this.rasters.clear();
                cx.notify();
            })
            .detach();
            self.face_ui().glyphs.tile = Some(slider);
        }
        let tile_slider = self.face_ui().glyphs.tile.clone();
        let size = tile_slider
            .as_ref()
            .map(|s| s.read(cx).value().start())
            .unwrap_or(size);
        let rows = self.listed(face, cx);
        let both = face.old.is_some() && face.new.is_some();
        let (filter, selected, width_cell, scroll) = {
            let ui = &self.face_ui().glyphs;
            (ui.filter, ui.selected, ui.width.clone(), ui.scroll.clone())
        };
        let counts = |f: GlyphFilter| face.glyphs.iter().filter(|g| f.keeps(g.status)).count();
        let labels: Vec<SharedString> = FILTERS
            .iter()
            .zip(["All", "Changed", "Added", "Removed", "Unchanged"])
            .map(|(f, l)| format!("{l} {}", counts(*f)).into())
            .collect();
        let filter_ix = FILTERS.iter().position(|f| *f == filter).unwrap_or(0);
        let view = cx.entity().downgrade();
        let gap = 4.;
        let columns = ((f32::from(width_cell.get()) + gap) / (size + gap))
            .floor()
            .max(1.) as usize;
        self.face_ui().glyphs.columns = columns;
        let row_count = rows.len().div_ceil(columns);
        let toolbar = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING())
            .p(SPACING())
            .border_b_1()
            .border_color(t.box_border)
            .when(both, |d| {
                let view = view.clone();
                d.child(segmented(
                    "font-glyph-filter",
                    labels,
                    filter_ix,
                    move |i, _, cx| {
                        view.update(cx, |v, cx| {
                            let ui = &mut v.face_ui().glyphs;
                            ui.filter = FILTERS[i];
                            cx.notify();
                        })
                        .ok();
                    },
                    cx,
                ))
            })
            .children(self.inputs.as_ref().map(|i| {
                div().w(zpx(240.)).child(crate::widgets::filter_text_box(
                    "font-glyph-search",
                    &i.glyph_search,
                    Some(crate::icons::octicon(
                        crate::icons::Octicon::Search,
                        t.text_secondary,
                    )),
                    window,
                    cx,
                ))
            }))
            .child(div().flex_1())
            .child(
                div()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(format!("{} shown", rows.len())),
            )
            .children(tile_slider.map(|s| div().w(zpx(110.)).child(Slider::new(&s).horizontal())));
        let grid: AnyElement = if rows.is_empty() {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(t.text_secondary)
                .child(if both && filter == GlyphFilter::Changed {
                    "No glyph changed."
                } else {
                    "No glyphs match."
                })
                .into_any_element()
        } else {
            let face = face.clone();
            let rows = rows.clone();
            uniform_list(
                "font-glyph-rows",
                row_count,
                cx.processor(move |this, range: Range<usize>, window, cx| {
                    range
                        .map(|r| {
                            let start = r * columns;
                            let end = (start + columns).min(rows.len());
                            div()
                                .flex()
                                .flex_row()
                                .gap(px(gap))
                                .pb(px(gap))
                                .children((start..end).map(|i| {
                                    let row_ix = rows[i];
                                    this.glyph_tile(
                                        &face,
                                        row_ix,
                                        size,
                                        selected == Some(row_ix),
                                        window,
                                        cx,
                                    )
                                }))
                                .into_any_element()
                        })
                        .collect()
                }),
            )
            .track_scroll(&scroll)
            .size_full()
            .into_any_element()
        };
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
        let inspector = selected
            .and_then(|ix| face.glyphs.get(ix).cloned())
            .map(|row| self.inspector(face, &row, window, cx));
        div()
            .id("font-glyphs")
            .size_full()
            .flex()
            .flex_col()
            .on_key_down(cx.listener(move |this, ev: &KeyDownEvent, _, cx| {
                let columns = this.face_ui().glyphs.columns.max(1) as isize;
                match ev.keystroke.key.as_str() {
                    "left" => this.move_glyph(-1, cx),
                    "right" => this.move_glyph(1, cx),
                    "up" => this.move_glyph(-columns, cx),
                    "down" => this.move_glyph(columns, cx),
                    "escape" => {
                        this.face_ui().glyphs.selected = None;
                        cx.notify();
                    }
                    "space" => {
                        let ui = &mut this.face_ui().glyphs;
                        ui.compare = Compare::Flip;
                        ui.flipped = !ui.flipped;
                        cx.notify();
                    }
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(toolbar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .p(SPACING())
                            .child(div().relative().size_full().child(measure).child(grid)),
                    )
                    .children(inspector),
            )
            .into_any_element()
    }

    /// The selected glyph, large, with what changed about it.
    fn inspector(
        &mut self,
        face: &Arc<FaceDiff>,
        row: &GlyphRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd().clone();
        let (compare, points, metrics, flipped) = {
            let ui = &self.face_ui().glyphs;
            (ui.compare, ui.points, ui.metrics, ui.flipped)
        };
        let both_sides = row.old.is_some() && row.new.is_some();
        let compare = if both_sides {
            compare
        } else {
            Compare::Overlay
        };
        let size = INSPECTOR;
        let big = |this: &mut Self,
                   new_side: bool,
                   gid: u32,
                   color: Ink2,
                   w: f32,
                   cx: &mut Context<Self>| {
            this.glyph_image(face, new_side, gid, w, w, color, window, cx)
        };
        // which outlines to draw, in which colour, at which width
        let mut layers: Vec<(bool, u32, Ink2, f32)> = Vec::new();
        match compare {
            Compare::Overlay => {
                if let (Some(o), true) = (row.old, both_sides) {
                    layers.push((false, o, Ink2::of(t.color_deleted, 0.75), size));
                }
                if let Some(n) = row.new {
                    layers.push((
                        true,
                        n,
                        Ink2::of(t.text, if both_sides { 0.8 } else { 1. }),
                        size,
                    ));
                } else if let Some(o) = row.old {
                    layers.push((false, o, Ink2::of(t.text, 1.), size));
                }
            }
            Compare::SideBySide => {
                let half = size / 2. - 4.;
                if let Some(o) = row.old {
                    layers.push((false, o, Ink2::of(t.text, 1.), half));
                }
                if let Some(n) = row.new {
                    layers.push((true, n, Ink2::of(t.text, 1.), half));
                }
            }
            Compare::Flip => match (flipped, row.old, row.new) {
                (true, Some(o), _) | (false, Some(o), None) => {
                    layers.push((false, o, Ink2::of(t.color_deleted, 1.), size))
                }
                (_, _, Some(n)) => layers.push((true, n, Ink2::of(t.text, 1.), size)),
                _ => {}
            },
        }
        let drawn: Vec<(bool, Option<Drawn>, f32)> = layers
            .iter()
            .map(|&(side, gid, color, w)| (side, big(self, side, gid, color, w, cx), w))
            .collect();
        // points, handles and metric lines of the drawn sides
        let guides = self.guides(face, row, &layers, points, metrics, &t);
        let stage: AnyElement =
            match compare {
                Compare::SideBySide => div()
                    .flex()
                    .flex_row()
                    .gap(zpx(8.))
                    .children(drawn.into_iter().zip(guides).map(|((side, d, w), g)| {
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
                                    .children(d.map(|d| {
                                        div().absolute().top_0().left_0().child(image_el(&d))
                                    }))
                                    .children(g),
                            )
                            .child(
                                div()
                                    .text_size(FONT_SIZE_SM())
                                    .text_color(t.text_secondary)
                                    .child(if side { "After" } else { "Before" }),
                            )
                    }))
                    .into_any_element(),
                _ => div()
                    .relative()
                    .size(px(size))
                    .rounded(zpx(3.))
                    .bg(t.box_alt_background)
                    .children(drawn.into_iter().filter_map(|(_, d, _)| {
                        d.map(|d| div().absolute().top_0().left_0().child(image_el(&d)))
                    }))
                    .children(guides.into_iter().flatten())
                    .into_any_element(),
            };
        let view = cx.entity().downgrade();
        let compare_ix = match compare {
            Compare::Overlay => 0,
            Compare::SideBySide => 1,
            Compare::Flip => 2,
        };
        let details = glyph_fields(face, row);
        let used_by = used_by(face, row);
        let old_name = row.old_name.clone();
        div()
            .id("font-glyph-inspector")
            .w(zpx(INSPECTOR + 2. * 12.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .border_l_1()
            .border_color(t.box_border)
            .bg(t.box_background)
            .p(zpx(12.))
            .flex()
            .flex_col()
            .gap(SPACING())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING_HALF())
                    .child(super::status_dot(row.status, &t))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(FONT_SIZE_MD())
                            .child(row.name.clone()),
                    )
                    .children(old_name.map(|o| {
                        div()
                            .text_size(FONT_SIZE_SM())
                            .text_color(t.text_secondary)
                            .child(format!("was {o}"))
                    }))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("font-inspector-close")
                            .a11y_button("Close")
                            .cursor_pointer()
                            .child(crate::icons::octicon(
                                crate::icons::Octicon::X,
                                t.text_secondary,
                            ))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.face_ui().glyphs.selected = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child({
                        let mut s = super::status_label(row.status).to_string();
                        let labels = row.changes.labels();
                        if !labels.is_empty() {
                            s.push_str(": ");
                            s.push_str(&labels.join(", "));
                        }
                        s
                    }),
            )
            .when(both_sides, |d| {
                let view = view.clone();
                d.child(segmented(
                    "font-inspector-compare",
                    ["Overlay", "Side by side", "Flip"]
                        .map(SharedString::from)
                        .to_vec(),
                    compare_ix,
                    move |i, _, cx| {
                        view.update(cx, |v, cx| {
                            let ui = &mut v.face_ui().glyphs;
                            ui.compare = [Compare::Overlay, Compare::SideBySide, Compare::Flip][i];
                            cx.notify();
                        })
                        .ok();
                    },
                    cx,
                ))
            })
            .child(stage)
            .when(compare == Compare::Flip, |d| {
                d.child(
                    div()
                        .text_size(FONT_SIZE_SM())
                        .text_color(t.text_secondary)
                        .child(format!(
                            "Showing {} · press Space to flip",
                            if flipped { "before" } else { "after" }
                        )),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(SPACING())
                    .child(crate::widgets::checkbox_row(
                        "font-inspector-points",
                        points,
                        "Points",
                        {
                            let view = view.clone();
                            move |on, _, cx| {
                                view.update(cx, |v, cx| {
                                    v.face_ui().glyphs.points = on;
                                    cx.notify();
                                })
                                .ok();
                            }
                        },
                        cx,
                    ))
                    .child(crate::widgets::checkbox_row(
                        "font-inspector-metrics",
                        metrics,
                        "Metrics",
                        {
                            let view = view.clone();
                            move |on, _, cx| {
                                view.update(cx, |v, cx| {
                                    v.face_ui().glyphs.metrics = on;
                                    cx.notify();
                                })
                                .ok();
                            }
                        },
                        cx,
                    )),
            )
            .child(super::overview::section("Glyph", &t))
            .child(super::overview::field_grid(
                "font-glyph-fields",
                &details,
                both_sides,
                &t,
            ))
            .when(!used_by.is_empty(), |d| {
                d.child(super::overview::section("Used by", &t)).child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(zpx(2.))
                        .text_size(FONT_SIZE_SM())
                        .children(used_by),
                )
            })
            .into_any_element()
    }

    /// Point, handle and metric overlays for each drawn layer.
    /// One overlay per layer (`None` where there is nothing to draw).
    fn guides(
        &self,
        face: &FaceDiff,
        _row: &GlyphRow,
        layers: &[(bool, u32, Ink2, f32)],
        points: bool,
        metrics: bool,
        t: &crate::theme::GhdTheme,
    ) -> Vec<Option<AnyElement>> {
        layers
            .iter()
            .map(|&(new_side, gid, _, w)| {
                if !points && !metrics {
                    return None;
                }
                let report = if new_side {
                    face.new.clone()
                } else {
                    face.old.clone()
                }?;
                let decoded = self.decoded(new_side)?;
                let font = decoded.face(report.index).ok()?;
                let tl = tile(&font, &[], w.round() as u32, w.round() as u32, 0.8);
                let path = corvene_fonts::report::draw(&font, gid, &[]);
                let advance = corvene_fonts::render::advance(&font, gid, &[]);
                let x0 = (w - advance * tl.scale) / 2.;
                let to = move |x: f32, y: f32| {
                    point(px(x0 + x * tl.scale), px(tl.baseline - y * tl.scale))
                };
                let vm = report.vmetrics;
                let lines: Vec<(f32, &'static str)> = [
                    Some((0., "baseline")),
                    vm.x_height.map(|v| (v, "x-height")),
                    vm.cap_height.map(|v| (v, "cap height")),
                    Some((vm.ascender, "ascender")),
                    Some((vm.descender, "descender")),
                ]
                .into_iter()
                .flatten()
                .collect();
                let label_lines = lines.clone();
                let marks = path.points();
                let handles = path.handles();
                let (on_color, off_color, line_color, metric_color) = (
                    if new_side { t.focus } else { t.color_deleted },
                    t.text_secondary,
                    t.text_secondary.opacity(0.6),
                    t.color_modified.opacity(0.55),
                );
                let label_color = t.text_secondary;
                let el = canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let o = bounds.origin;
                        if metrics {
                            for (y, _) in &lines {
                                let p = to(0., *y);
                                window.paint_quad(fill(
                                    Bounds::new(
                                        point(o.x, o.y + p.y),
                                        size(bounds.size.width, px(1.)),
                                    ),
                                    metric_color,
                                ));
                            }
                            for x in [0., advance] {
                                let p = to(x, 0.);
                                window.paint_quad(fill(
                                    Bounds::new(
                                        point(o.x + p.x, o.y),
                                        size(px(1.), bounds.size.height),
                                    ),
                                    metric_color,
                                ));
                            }
                        }
                        if points {
                            for [a, b] in &handles {
                                let (pa, pb) = (to(a.0, a.1), to(b.0, b.1));
                                let mut builder = PathBuilder::stroke(px(1.));
                                builder.move_to(o + pa);
                                builder.line_to(o + pb);
                                if let Ok(path) = builder.build() {
                                    window.paint_path(path, line_color);
                                }
                            }
                            for m in &marks {
                                let p = to(m.x, m.y);
                                let r = if m.on_curve { px(3.) } else { px(2.5) };
                                let b = Bounds::new(
                                    point(o.x + p.x - r, o.y + p.y - r),
                                    size(r * 2., r * 2.),
                                );
                                if m.on_curve {
                                    window.paint_quad(fill(b, on_color));
                                } else {
                                    window.paint_quad(
                                        fill(b, gpui_kit::transparent_black())
                                            .corner_radii(r)
                                            .border_widths(px(1.))
                                            .border_color(off_color),
                                    );
                                }
                            }
                        }
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full();
                let labels = metrics.then(move || {
                    div().absolute().top_0().left_0().size_full().children(
                        label_lines.into_iter().map(move |(y, name)| {
                            let p = to(0., y);
                            div()
                                .absolute()
                                .top(p.y - px(11.))
                                .right(px(2.))
                                .text_size(zpx(9.))
                                .text_color(label_color)
                                .child(name)
                        }),
                    )
                });
                Some(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .child(el)
                        .children(labels)
                        .into_any_element(),
                )
            })
            .collect()
    }
}

impl Ink2 {
    pub(super) fn key(self) -> u32 {
        self.0
    }
    pub(super) fn ink(self) -> corvene_fonts::render::Ink {
        self.1
    }
}

/// What the inspector lists about a glyph, old next to new.
fn glyph_fields(face: &FaceDiff, row: &GlyphRow) -> Vec<Field> {
    let info = |new_side: bool, gid: Option<u32>| {
        let report = if new_side {
            face.new.as_ref()
        } else {
            face.old.as_ref()
        }?;
        let g = report.glyphs.get(gid? as usize)?;
        let names = |gids: &[u32]| {
            gids.iter()
                .map(|&c| report.glyph_name(c))
                .collect::<Vec<_>>()
                .join(" + ")
        };
        let mut out = vec![
            ("Name".to_string(), g.name.clone()),
            ("Glyph id".to_string(), gid?.to_string()),
            (
                "Code points".to_string(),
                if g.codepoints.is_empty() {
                    "none".to_string()
                } else {
                    g.codepoints
                        .iter()
                        .map(|c| codepoint_label(*c))
                        .collect::<Vec<_>>()
                        .join(" ")
                },
            ),
            ("Advance width".to_string(), format!("{}", g.advance)),
            ("Left side bearing".to_string(), format!("{}", g.lsb)),
            (
                "Bounds".to_string(),
                g.bounds
                    .map(|b| format!("{} {} → {} {}", b[0], b[1], b[2], b[3]))
                    .unwrap_or_else(|| "empty".to_string()),
            ),
            ("Contours".to_string(), g.contours.to_string()),
            ("Points".to_string(), g.points.to_string()),
        ];
        if !g.components.is_empty() {
            out.push(("Components".to_string(), names(&g.components)));
        }
        if g.color {
            out.push(("Colour".to_string(), "COLR layers".to_string()));
        }
        Some(out)
    };
    let (o, n) = (info(false, row.old), info(true, row.new));
    corvene_fonts::compare::merge_fields(o.as_deref(), n.as_deref())
}

/// Features whose substitutions take or make the glyph, and its kerning.
fn used_by(face: &FaceDiff, row: &GlyphRow) -> Vec<AnyElement> {
    let mut out = Vec::new();
    for f in &face.features {
        let in_rules = |new_side: bool| {
            let report = if new_side {
                face.new.as_ref()
            } else {
                face.old.as_ref()
            };
            let feature = if new_side {
                f.new.as_ref()
            } else {
                f.old.as_ref()
            };
            let gid = if new_side { row.new } else { row.old };
            match (report, feature, gid) {
                (Some(_), Some(feature), Some(gid)) => feature
                    .rules
                    .iter()
                    .any(|r| r.input.contains(&gid) || r.output.contains(&gid)),
                _ => false,
            }
        };
        let (o, n) = (in_rules(false), in_rules(true));
        if o || n {
            let what = match (o, n) {
                (true, true) => "",
                (false, true) => " (new)",
                _ => " (no longer)",
            };
            out.push(
                div()
                    .child(format!("{} {}{what}", f.table.label(), f.tag))
                    .into_any_element(),
            );
        }
    }
    let kern: Vec<_> = face
        .kerning
        .iter()
        .filter(|k| k.left == row.name || k.right == row.name)
        .take(12)
        .collect();
    for k in kern {
        out.push(
            div()
                .child(format!(
                    "kern {} {}: {} → {}",
                    k.left,
                    k.right,
                    k.old.map(|v| v.to_string()).unwrap_or_else(|| "—".into()),
                    k.new.map(|v| v.to_string()).unwrap_or_else(|| "—".into())
                ))
                .into_any_element(),
        );
    }
    out
}
