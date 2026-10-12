//! The Features tab. On top a playground: text shaped in both versions
//! with each OpenType feature left to the shaper, forced on or forced off.
//! Below, the features of `GSUB` and `GPOS` with what changed: the
//! substitutions a feature makes (drawn, old and new), the kerning pairs
//! whose value changed (drawn kerned, largest change first) and the scripts
//! and languages with the features they turn on.

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use corvene_fonts::Tag;
use corvene_fonts::compare::{FaceDiff, FeatureRow, KernRow, NamedRule, ScriptRow, Status};
use corvene_fonts::layout::{LayoutTable, RuleKind, on_by_default, registered_name};
use corvene_fonts::render::{TextBlock, TextOptions, advance, render_run, render_text, tile};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::widgets::GhdTooltip;

use crate::widgets::IconButtonA11y;

use super::raster::{Drawn, RasterKey, image_el};
use super::specimen::Ink2;
use super::{FontDiffView, status_color};
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, GhdTheme};

const KERNING: &str = "\u{0}kerning";
const SCRIPTS: &str = "\u{0}scripts";
/// Height of a drawn rule or pair, points.
const RUN_HEIGHT: f32 = 40.;
const RUN_WIDTH: f32 = 140.;

#[derive(Default)]
pub(super) struct FeaturesUi {
    /// Forced feature values (absent: the shaper decides).
    toggles: BTreeMap<String, bool>,
    selected: Option<String>,
    scroll: UniformListScrollHandle,
}

fn key(f: &FeatureRow) -> String {
    format!("{}:{}", f.table.label(), f.tag)
}

fn feature_name(f: &FeatureRow) -> Option<String> {
    f.ui_name
        .clone()
        .or_else(|| registered_name(&f.tag).map(str::to_string))
}

fn lookup_types(f: &FeatureRow) -> String {
    let feature = f.new.as_ref().or(f.old.as_ref());
    let Some(feature) = feature else {
        return String::new();
    };
    feature
        .lookup_types
        .iter()
        .map(|&t| match (f.table, t) {
            (LayoutTable::Gsub, 1) => "single",
            (LayoutTable::Gsub, 2) => "multiple",
            (LayoutTable::Gsub, 3) => "alternate",
            (LayoutTable::Gsub, 4) => "ligature",
            (LayoutTable::Gsub, 5 | 6) => "contextual",
            (LayoutTable::Gsub, 8) => "reverse chaining",
            (LayoutTable::Gpos, 1) => "single adjustment",
            (LayoutTable::Gpos, 2) => "pair adjustment",
            (LayoutTable::Gpos, 3) => "cursive",
            (LayoutTable::Gpos, 4) => "mark to base",
            (LayoutTable::Gpos, 5) => "mark to ligature",
            (LayoutTable::Gpos, 6) => "mark to mark",
            (LayoutTable::Gpos, 7 | 8) => "contextual",
            _ => "other",
        })
        .collect::<Vec<_>>()
        .join(", ")
}

impl FontDiffView {
    /// Glyphs by name drawn in a row on one side (`None` if a glyph is
    /// missing there). `kern` adds that much space before the second glyph.
    #[allow(clippy::too_many_arguments)]
    fn run_image(
        &mut self,
        face: &FaceDiff,
        new_side: bool,
        names: &[String],
        kern: i32,
        color: Ink2,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Drawn> {
        let report = if new_side {
            face.new.clone()
        } else {
            face.old.clone()
        }?;
        let gids: Vec<u32> = names
            .iter()
            .map(|n| report.glyph_by_name(n))
            .collect::<Option<_>>()?;
        let decoded = self.decoded(new_side)?;
        let scale = window.scale_factor();
        let (pw, ph) = ((RUN_WIDTH * scale) as u32, (RUN_HEIGHT * scale) as u32);
        let key = RasterKey::of(("run", self.face, new_side, &gids, kern, pw, color.key()));
        let index = report.index;
        let ink = color.ink();
        self.rasters.get(
            key,
            scale,
            move || {
                let font = decoded.face(index).ok()?;
                let t = tile(&font, &[], pw, ph, 0.85);
                let mut x = 0.;
                let mut run = Vec::new();
                for (i, &g) in gids.iter().enumerate() {
                    if i == 1 {
                        x += kern as f32;
                    }
                    run.push((g, x));
                    x += advance(&font, g, &[]);
                }
                // a run wider than the cell is shrunk to fit
                let mut t = t;
                let fit = (pw as f32 * 0.9) / (x * t.scale).max(1.);
                if fit < 1. {
                    t.scale *= fit;
                    t.baseline = ph as f32 / 2. + (t.baseline - ph as f32 / 2.) * fit;
                }
                Some(render_run(&font, &run, &[], t, ink))
            },
            cx,
        )
    }

    fn playground(
        &mut self,
        face: &FaceDiff,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd().clone();
        let text = self
            .inputs
            .as_ref()
            .map(|i| i.playground.read(cx).value().to_string())
            .unwrap_or_default();
        let toggles = self.face_ui().features.toggles.clone();
        let features: Vec<(Tag, u32)> = toggles
            .iter()
            .filter_map(|(tag, on)| {
                Some((
                    Tag::new_checked(format!("{tag:<4}").as_bytes()).ok()?,
                    u32::from(*on),
                ))
            })
            .collect();
        let scale = window.scale_factor();
        let width = 900.;
        let mut lines = Vec::new();
        for new_side in [false, true] {
            let report = if new_side {
                face.new.clone()
            } else {
                face.old.clone()
            };
            let (Some(report), Some(decoded)) = (report, self.decoded(new_side)) else {
                continue;
            };
            let key = RasterKey::of((
                "playground",
                self.face,
                new_side,
                &text,
                &toggles,
                scale.to_bits(),
                t.text.to_rgb().r.to_bits(),
            ));
            let index = report.index;
            let ink = Ink2::of(t.text, 1.).ink();
            let text = text.clone();
            let features = features.clone();
            let drawn = self.rasters.get(
                key,
                scale,
                move || {
                    let font = decoded.face(index).ok()?;
                    Some(
                        render_text(
                            &font,
                            &[TextBlock {
                                text,
                                size: 40. * scale,
                            }],
                            &TextOptions {
                                features,
                                variations: Vec::new(),
                                width: width * scale,
                                line_height: 1.2,
                                block_gap: 0.,
                                padding: 4. * scale,
                                ink,
                            },
                        )
                        .raster,
                    )
                },
                cx,
            );
            let both = face.old.is_some() && face.new.is_some();
            lines.push(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING())
                    .when(both, |d| {
                        d.child(
                            div()
                                .w(zpx(44.))
                                .flex_none()
                                .text_size(FONT_SIZE_SM())
                                .text_color(t.text_secondary)
                                .child(if new_side { "After" } else { "Before" }),
                        )
                    })
                    .child(
                        drawn
                            .map(|d| image_el(&d).into_any_element())
                            .unwrap_or_else(|| div().h(zpx(56.)).into_any_element()),
                    ),
            );
        }
        // every feature tag of either side, once
        let mut tags: Vec<(String, Option<String>, Status)> = Vec::new();
        for f in &face.features {
            if f.tag == "kern" && f.table == LayoutTable::Gpos {
                // kern is shown too, like the others
            }
            if let Some(entry) = tags.iter_mut().find(|e| e.0 == f.tag) {
                if f.status.is_change() {
                    entry.2 = f.status;
                }
            } else {
                tags.push((f.tag.clone(), feature_name(f), f.status));
            }
        }
        let view = cx.entity().downgrade();
        let chips = tags
            .into_iter()
            .enumerate()
            .map(|(i, (tag, name, status))| {
                let forced = toggles.get(&tag).copied();
                let default_on = on_by_default(&tag);
                let effective = forced.unwrap_or(default_on);
                let view = view.clone();
                let hover = t.box_hover_background;
                let tip = format!(
                    "{}{} · {} · click to {}",
                    tag,
                    name.as_ref().map(|n| format!(" ({n})")).unwrap_or_default(),
                    match forced {
                        None => format!("default ({})", if default_on { "on" } else { "off" }),
                        Some(true) => "forced on".to_string(),
                        Some(false) => "forced off".to_string(),
                    },
                    match forced {
                        None => "force it on",
                        Some(true) => "force it off",
                        Some(false) => "leave it to the shaper",
                    }
                );
                let tag2 = tag.clone();
                div()
                    .id(("font-feature-chip", i))
                    .a11y_button(tip.clone())
                    .h(zpx(22.))
                    .px(SPACING_HALF())
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(zpx(4.))
                    .rounded(BORDER_RADIUS())
                    .border_1()
                    .border_color(if forced.is_some() {
                        t.focus
                    } else {
                        t.box_border
                    })
                    .when(effective, |d| d.bg(t.box_selected_background))
                    .font_family(crate::theme::mono_font())
                    .text_size(FONT_SIZE_SM())
                    .text_color(if effective { t.text } else { t.text_secondary })
                    .when(forced == Some(false), |d| d.line_through())
                    .cursor_pointer()
                    .hover(move |d| d.bg(hover))
                    .ghd_tooltip(tip)
                    .when(status.is_change(), |d| {
                        d.child(super::status_dot(status, &t))
                    })
                    .child(tag)
                    .on_click(move |_, _, cx| {
                        view.update(cx, |v, cx| {
                            let toggles = &mut v.face_ui().features.toggles;
                            match toggles.get(&tag2).copied() {
                                None => {
                                    toggles.insert(tag2.clone(), true);
                                }
                                Some(true) => {
                                    toggles.insert(tag2.clone(), false);
                                }
                                Some(false) => {
                                    toggles.remove(&tag2);
                                }
                            }
                            cx.notify();
                        })
                        .ok();
                    })
            });
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(SPACING_HALF())
            .p(SPACING())
            .border_b_1()
            .border_color(t.box_border)
            .children(self.inputs.as_ref().map(|i| {
                crate::widgets::text_box("font-playground-text", &i.playground, None, window, cx)
            }))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap(zpx(4.))
                    .children(chips),
            )
            .children(lines)
            .into_any_element()
    }

    pub(super) fn features(
        &mut self,
        face: &Arc<FaceDiff>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd().clone();
        let both = face.old.is_some() && face.new.is_some();
        let selected = {
            let ui = &mut self.face_ui().features;
            if ui.selected.is_none() {
                ui.selected = Some(
                    face.features
                        .iter()
                        .find(|f| f.status.is_change())
                        .map(key)
                        .unwrap_or_else(|| {
                            if face.kerning.is_empty() {
                                SCRIPTS.to_string()
                            } else {
                                KERNING.to_string()
                            }
                        }),
                );
            }
            ui.selected.clone().unwrap_or_default()
        };
        let playground = self.playground(face, window, cx);
        let mut entries: Vec<(String, String, Option<String>, Status)> = vec![
            (
                KERNING.to_string(),
                "Kerning".to_string(),
                Some(format!(
                    "{} pairs",
                    face.kerning_counts.1.max(face.kerning_counts.0)
                )),
                if both && !face.kerning.is_empty() {
                    Status::Modified
                } else {
                    Status::Unchanged
                },
            ),
            (
                SCRIPTS.to_string(),
                "Scripts & languages".to_string(),
                None,
                if both && face.scripts.iter().any(|s| s.status().is_change()) {
                    Status::Modified
                } else {
                    Status::Unchanged
                },
            ),
        ];
        for f in &face.features {
            entries.push((
                key(f),
                format!("{} {}", f.table.label(), f.tag),
                feature_name(f),
                f.status,
            ));
        }
        let list =
            div()
                .id("font-feature-list")
                .w(zpx(260.))
                .flex_none()
                .h_full()
                .overflow_y_scroll()
                .border_r_1()
                .border_color(t.box_border)
                .py(SPACING_HALF())
                .children(entries.into_iter().enumerate().map(
                    |(i, (k, label, detail, status))| {
                        let is_selected = k == selected;
                        let hover = t.box_hover_background;
                        div()
                            .id(("font-feature", i))
                            .a11y_button(label.clone())
                            .px(SPACING())
                            .py(zpx(3.))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(SPACING_HALF())
                            .cursor_pointer()
                            .when(is_selected, |d| {
                                d.bg(t.box_selected_active_background)
                                    .text_color(t.box_selected_active_text)
                            })
                            .when(!is_selected, |d| d.hover(move |d| d.bg(hover)))
                            .child(
                                super::status_dot(status, &t)
                                    .when(status == Status::Unchanged, |d| {
                                        d.bg(transparent_black())
                                    }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .min_w_0()
                                    .child(div().text_size(FONT_SIZE_SM()).child(label))
                                    .children(detail.map(|d| {
                                        div()
                                            .text_size(zpx(10.))
                                            .text_color(if is_selected {
                                                t.box_selected_active_text
                                            } else {
                                                t.text_secondary
                                            })
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .child(d)
                                    })),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.face_ui().features.selected = Some(k.clone());
                                cx.notify();
                            }))
                    },
                ));
        let detail = match selected.as_str() {
            KERNING => self.kerning(face, both, window, cx),
            SCRIPTS => scripts(&face.scripts, both, &t),
            k => match face.features.iter().find(|f| key(f) == k).cloned() {
                Some(f) => self.feature_detail(face, &f, both, window, cx),
                None => div().into_any_element(),
            },
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(playground)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .child(list)
                    .child(div().flex_1().min_w_0().h_full().child(detail)),
            )
            .into_any_element()
    }

    fn feature_detail(
        &mut self,
        face: &FaceDiff,
        f: &FeatureRow,
        both: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd().clone();
        let lookups = |side: Option<&corvene_fonts::layout::Feature>| {
            side.map(|s| s.lookups.len()).unwrap_or(0)
        };
        let mut notes = vec![format!(
            "{} · {} lookups{}",
            lookup_types(f),
            lookups(f.new.as_ref().or(f.old.as_ref())),
            if both && lookups(f.old.as_ref()) != lookups(f.new.as_ref()) {
                format!(" (was {})", lookups(f.old.as_ref()))
            } else {
                String::new()
            }
        )];
        let contextual = f
            .new
            .as_ref()
            .or(f.old.as_ref())
            .map(|x| x.contextual)
            .unwrap_or(0);
        if contextual > 0 {
            notes.push(format!(
                "{contextual} contextual lookups are compared as a whole, not rule by rule."
            ));
        }
        if both
            && f.status == Status::Modified
            && f.added_rules.is_empty()
            && f.removed_rules.is_empty()
            && f.lookups_changed == Some(true)
        {
            notes.push(
                "The lookups changed in ways not listed here (positioning, context or flags)."
                    .to_string(),
            );
        }
        if f.new
            .as_ref()
            .or(f.old.as_ref())
            .is_some_and(|x| x.truncated)
        {
            notes.push(format!(
                "Only the first {} rules were read.",
                corvene_fonts::layout::MAX_RULES
            ));
        }
        let mut sections: Vec<AnyElement> = Vec::new();
        let rule_rows = |this: &mut Self,
                         rules: &[NamedRule],
                         new_side: bool,
                         status: Status,
                         cx: &mut Context<Self>|
         -> Vec<AnyElement> {
            rules
                .iter()
                .take(300)
                .enumerate()
                .map(|(i, r)| this.rule_row(face, r, new_side, status, i, window, cx))
                .collect()
        };
        if !both {
            let feature = f.new.as_ref().or(f.old.as_ref());
            let report = if face.new.is_some() {
                face.new.clone()
            } else {
                face.old.clone()
            };
            if let (Some(feature), Some(report)) = (feature, report) {
                let named: Vec<NamedRule> = feature
                    .rules
                    .iter()
                    .map(|r| NamedRule {
                        kind: r.kind,
                        input: r.input.iter().map(|&g| report.glyph_name(g)).collect(),
                        output: r.output.iter().map(|&g| report.glyph_name(g)).collect(),
                    })
                    .collect();
                if !named.is_empty() {
                    sections.push(
                        super::overview::section(&format!("{} rules", named.len()), &t)
                            .into_any_element(),
                    );
                    sections.extend(rule_rows(
                        self,
                        &named,
                        face.new.is_some(),
                        Status::Unchanged,
                        cx,
                    ));
                }
            }
        } else {
            if !f.added_rules.is_empty() {
                sections.push(
                    super::overview::section(&format!("{} rules added", f.added_rules.len()), &t)
                        .into_any_element(),
                );
                sections.extend(rule_rows(self, &f.added_rules, true, Status::Added, cx));
            }
            if !f.removed_rules.is_empty() {
                sections.push(
                    super::overview::section(
                        &format!("{} rules removed", f.removed_rules.len()),
                        &t,
                    )
                    .into_any_element(),
                );
                sections.extend(rule_rows(
                    self,
                    &f.removed_rules,
                    false,
                    Status::Removed,
                    cx,
                ));
            }
            if f.unchanged_rules > 0 {
                sections.push(
                    div()
                        .text_size(FONT_SIZE_SM())
                        .text_color(t.text_secondary)
                        .child(format!("{} rules unchanged", f.unchanged_rules))
                        .into_any_element(),
                );
            }
        }
        div()
            .id("font-feature-detail")
            .size_full()
            .overflow_y_scroll()
            .p(SPACING())
            .flex()
            .flex_col()
            .gap(zpx(4.))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING_HALF())
                    .child(super::status_dot(f.status, &t))
                    .child(
                        div()
                            .font_family(crate::theme::mono_font())
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(FONT_SIZE_MD())
                            .child(format!("{} {}", f.table.label(), f.tag)),
                    )
                    .children(feature_name(f).map(|n| div().text_color(t.text_secondary).child(n)))
                    .child(
                        div()
                            .text_size(FONT_SIZE_SM())
                            .text_color(status_color(f.status, &t))
                            .child(super::status_label(f.status)),
                    ),
            )
            .children(notes.into_iter().map(|n| {
                div()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(n)
            }))
            .children(sections)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn rule_row(
        &mut self,
        face: &FaceDiff,
        r: &NamedRule,
        new_side: bool,
        status: Status,
        i: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd().clone();
        let ink = Ink2::of(t.text, 1.);
        let input = self.run_image(face, new_side, &r.input, 0, ink, window, cx);
        // alternates are choices, not a sequence: draw the first
        let output_names: Vec<String> = match r.kind {
            RuleKind::Alternate => r.output.iter().take(1).cloned().collect(),
            _ => r.output.clone(),
        };
        let output = self.run_image(face, new_side, &output_names, 0, ink, window, cx);
        let arrow = match r.kind {
            RuleKind::Alternate => "→ one of",
            _ => "→",
        };
        let cell = |d: Option<Drawn>| {
            div()
                .w(zpx(RUN_WIDTH))
                .h(zpx(RUN_HEIGHT))
                .flex_none()
                .rounded(zpx(3.))
                .bg(t.box_alt_background)
                .children(d.map(|d| image_el(&d)))
        };
        div()
            .id(("font-rule", i))
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING())
            .py(zpx(2.))
            .pl(zpx(4.))
            .border_l_2()
            .border_color(status_color(status, &t))
            .child(cell(input))
            .child(div().text_color(t.text_secondary).child(arrow))
            .child(cell(output))
            .child(
                div()
                    .font_family(crate::theme::mono_font())
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(format!(
                        "{} {} {}",
                        r.input.join(" "),
                        arrow,
                        r.output.join(" ")
                    )),
            )
            .into_any_element()
    }

    fn kerning(
        &mut self,
        face: &Arc<FaceDiff>,
        both: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd().clone();
        if !both {
            let n = face.kerning_counts.0.max(face.kerning_counts.1);
            return div()
                .p(SPACING())
                .text_color(t.text_secondary)
                .child(format!("{n} kerning pairs."))
                .into_any_element();
        }
        if face.kerning.is_empty() {
            return div()
                .p(SPACING())
                .text_color(t.text_secondary)
                .child(format!(
                    "No kerning pair changed ({} pairs before, {} after).",
                    face.kerning_counts.0, face.kerning_counts.1
                ))
                .into_any_element();
        }
        let scroll = self.face_ui().features.scroll.clone();
        let face2 = face.clone();
        let header = div()
            .flex_none()
            .h(zpx(26.))
            .px(SPACING())
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING())
            .border_b_1()
            .border_color(t.box_border)
            .text_size(FONT_SIZE_SM())
            .text_color(t.text_secondary)
            .child(div().w(zpx(RUN_WIDTH)).child("Before"))
            .child(div().w(zpx(RUN_WIDTH)).child("After"))
            .child(div().flex_1().child("Pair"))
            .child(div().w(zpx(140.)).child("Value"))
            .child(div().child(format!(
                "{} changed of {} → {}{}",
                face.kerning.len(),
                face.kerning_counts.0,
                face.kerning_counts.1,
                if face.kerning_truncated {
                    " (truncated)"
                } else {
                    ""
                }
            )));
        let list = uniform_list(
            "font-kern-rows",
            face.kerning.len(),
            cx.processor(move |this, range: Range<usize>, window, cx| {
                range
                    .filter_map(|i| {
                        let k = face2.kerning.get(i)?.clone();
                        Some(this.kern_row(&face2, &k, i, window, cx))
                    })
                    .collect()
            }),
        )
        .track_scroll(&scroll)
        .size_full();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(div().flex_1().min_h_0().child(list))
            .into_any_element()
    }

    fn kern_row(
        &mut self,
        face: &FaceDiff,
        k: &KernRow,
        i: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd().clone();
        let names = [k.left.clone(), k.right.clone()];
        let ink = Ink2::of(t.text, 1.);
        let old = self.run_image(face, false, &names, k.old.unwrap_or(0), ink, window, cx);
        let new = self.run_image(face, true, &names, k.new.unwrap_or(0), ink, window, cx);
        let delta = k.delta();
        let value = |v: Option<i32>| v.map(|v| v.to_string()).unwrap_or_else(|| "—".to_string());
        let cell = |d: Option<Drawn>, bg: Hsla| {
            div()
                .w(zpx(RUN_WIDTH))
                .h(zpx(RUN_HEIGHT))
                .flex_none()
                .rounded(zpx(3.))
                .bg(bg)
                .children(d.map(|d| image_el(&d)))
        };
        div()
            .id(("font-kern", i))
            .h(zpx(RUN_HEIGHT + 6.))
            .px(SPACING())
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING())
            .border_b_1()
            .border_color(t.box_border.opacity(0.3))
            .child(cell(old, t.diff_delete_background.opacity(0.4)))
            .child(cell(new, t.diff_add_background.opacity(0.4)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_family(crate::theme::mono_font())
                    .text_size(FONT_SIZE_SM())
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(format!("{} {}", k.left, k.right)),
            )
            .child(
                div()
                    .w(zpx(140.))
                    .flex_none()
                    .flex()
                    .flex_row()
                    .gap(zpx(6.))
                    .font_family(crate::theme::mono_font())
                    .text_size(FONT_SIZE_SM())
                    .child(format!("{} → {}", value(k.old), value(k.new)))
                    .child(
                        div()
                            .text_color(if delta < 0 {
                                t.color_deleted
                            } else {
                                t.color_new
                            })
                            .child(format!("{}{delta}", if delta > 0 { "+" } else { "" })),
                    ),
            )
            .into_any_element()
    }
}

/// Scripts and languages with the features they turn on.
fn scripts(rows: &[ScriptRow], both: bool, t: &GhdTheme) -> AnyElement {
    div()
        .id("font-scripts")
        .size_full()
        .overflow_y_scroll()
        .p(SPACING())
        .flex()
        .flex_col()
        .children(rows.iter().enumerate().map(|(i, r)| {
            let status = if both { r.status() } else { Status::Unchanged };
            let tags = |list: &Option<Vec<String>>, other: &Option<Vec<String>>, color: Hsla| {
                div().flex().flex_row().flex_wrap().gap(zpx(4.)).children(
                    list.iter().flatten().map(|tag| {
                        let differs = both && !other.as_ref().is_some_and(|o| o.contains(tag));
                        div()
                            .px(zpx(4.))
                            .rounded(zpx(3.))
                            .font_family(crate::theme::mono_font())
                            .text_size(FONT_SIZE_SM())
                            .when(differs, |d| d.bg(color.opacity(0.25)))
                            .child(tag.clone())
                    }),
                )
            };
            div()
                .id(("font-script", i))
                .flex()
                .flex_row()
                .gap(SPACING())
                .py(zpx(4.))
                .border_b_1()
                .border_color(t.box_border.opacity(0.4))
                .child(
                    super::status_dot(status, t)
                        .mt(zpx(5.))
                        .when(status == Status::Unchanged, |d| d.bg(transparent_black())),
                )
                .child(
                    div()
                        .w(zpx(150.))
                        .flex_none()
                        .font_family(crate::theme::mono_font())
                        .text_size(FONT_SIZE_SM())
                        .child(format!("{} {} / {}", r.table.label(), r.script, r.language)),
                )
                .map(|d| {
                    if both && status.is_change() {
                        d.child(div().flex_1().child(tags(&r.old, &r.new, t.color_deleted)))
                            .child(div().flex_1().child(tags(&r.new, &r.old, t.color_new)))
                    } else {
                        d.child(div().flex_1().child(tags(
                            if r.new.is_some() { &r.new } else { &r.old },
                            &None,
                            t.text,
                        )))
                    }
                })
        }))
        .into_any_element()
}
