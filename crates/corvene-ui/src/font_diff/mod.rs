//! Corvene `1329-font-diff`: a changed font file in a font viewer. GitHub
//! Desktop shows fonts as "Binary file not shown" (`app/src/lib/git/diff.ts`
//! treats every non-image binary file alike).
//!
//! The file is read and compared on the background executor
//! (`corvene_fonts::compare`, then [`corvene_fonts::FontDiff::face`] for
//! the face on screen), so a large collection never blocks a frame. Five
//! tabs share one header:
//!
//! - Overview: what the font says about itself, old next to new.
//! - Specimen: sample text set in both versions, stacked, side by side,
//!   overlaid, swiped or highlighted (red only old, green only new), with
//!   axis sliders for a variable font.
//! - Glyphs: every glyph, filterable, with an inspector that overlays the
//!   two outlines with their points and metrics.
//! - Tables: the sfnt tables, field by field.
//! - Features: scripts, OpenType features, their substitutions, kerning,
//!   and a playground that shapes text with features on or off.
//!
//! A collection gets a tab per face above them. Glyphs and text are drawn
//! with tiny-skia at the device pixels they cover ([`raster`]).

mod features;
mod glyphs;
mod overview;
pub mod raster;
pub mod source;
mod specimen;
mod tables;

use std::collections::HashMap;
use std::sync::Arc;

use corvene_core::Dispatcher;
use corvene_fonts::Decoded;
use corvene_fonts::compare::{FaceDiff, FontDiff, Status};
use corvene_models::Diff;
use corvene_models::FontDiffView as Tab;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::widgets::IconButtonA11y;

use crate::icons::{Octicon, octicon};
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, GhdTheme};
use crate::widgets::GhdTooltip;
use raster::{RasterCache, RasterHost};

pub use glyphs::GlyphFilter;

/// A face's full comparison, read on demand.
enum FaceSlot {
    Loading,
    Ready(Arc<FaceDiff>),
}

enum Phase {
    Reading,
    Failed(String),
    Ready(Arc<FontDiff>),
}

/// Per-face view state that would be wrong on another face.
#[derive(Default)]
struct FaceUi {
    glyphs: glyphs::GlyphsUi,
    tables: tables::TablesUi,
    features: features::FeaturesUi,
    specimen: specimen::SpecimenUi,
}

pub struct FontDiffView {
    /// The diff this view reads (kept to tell a reload of the same file
    /// from a new one).
    pub(crate) source: Arc<Diff>,
    phase: Phase,
    face: usize,
    faces: HashMap<usize, FaceSlot>,
    ui: HashMap<usize, FaceUi>,
    rasters: RasterCache,
    focus_handle: FocusHandle,
    /// Text boxes (made on first render: they need a window).
    inputs: Option<Inputs>,
    /// The device scale the rasters were drawn for.
    scale: f32,
}

/// The view's text boxes.
struct Inputs {
    glyph_search: Entity<InputState>,
    specimen: Entity<InputState>,
    playground: Entity<InputState>,
    field_filter: Entity<InputState>,
}

impl RasterHost for FontDiffView {
    fn rasters(&mut self) -> &mut RasterCache {
        &mut self.rasters
    }
}

impl FontDiffView {
    /// Starts reading the sides of `diff` (a `Diff::Font`).
    pub fn new(diff: Arc<Diff>, cx: &mut Context<Self>) -> Self {
        let source = diff.clone();
        let task = cx.background_executor().spawn(async move {
            let Diff::Font { previous, current } = &*source else {
                return Err(corvene_fonts::FontError::NotAFont);
            };
            corvene_fonts::compare(
                previous.as_ref().map(|b| b.bytes.as_slice()),
                current.as_ref().map(|b| b.bytes.as_slice()),
            )
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.phase = match result {
                    Ok(diff) => Phase::Ready(Arc::new(diff)),
                    Err(e) => Phase::Failed(e.to_string()),
                };
                this.load_face(0, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self {
            source: diff,
            phase: Phase::Reading,
            face: 0,
            faces: HashMap::new(),
            ui: HashMap::new(),
            rasters: RasterCache::default(),
            focus_handle: cx.focus_handle(),
            inputs: None,
            scale: 0.,
        }
    }

    fn diff(&self) -> Option<&Arc<FontDiff>> {
        match &self.phase {
            Phase::Ready(d) => Some(d),
            _ => None,
        }
    }

    /// Reads face pair `i` in the background unless it is read or being read.
    fn load_face(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(diff) = self.diff().cloned() else {
            return;
        };
        if self.faces.contains_key(&i) || i >= diff.faces.len() {
            return;
        }
        self.faces.insert(i, FaceSlot::Loading);
        let task = cx.background_executor().spawn(async move { diff.face(i) });
        cx.spawn(async move |this, cx| {
            let face = task.await;
            this.update(cx, |this, cx| {
                match face {
                    Some(face) => {
                        this.faces.insert(i, FaceSlot::Ready(Arc::new(face)));
                    }
                    None => {
                        this.faces.remove(&i);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn select_face(&mut self, i: usize, cx: &mut Context<Self>) {
        self.face = i;
        self.load_face(i, cx);
        cx.notify();
    }

    fn current_face(&self) -> Option<Arc<FaceDiff>> {
        match self.faces.get(&self.face)? {
            FaceSlot::Ready(f) => Some(f.clone()),
            FaceSlot::Loading => None,
        }
    }

    fn face_ui(&mut self) -> &mut FaceUi {
        self.ui.entry(self.face).or_default()
    }

    /// The unpacked file of a side.
    fn decoded(&self, new_side: bool) -> Option<Decoded> {
        let diff = self.diff()?;
        if new_side {
            diff.new.clone()
        } else {
            diff.old.clone()
        }
    }

    fn tab(cx: &App) -> Tab {
        corvene_core::AppState::try_global(cx)
            .map(|s| s.read(cx).settings.font_diff_view)
            .unwrap_or_default()
    }

    fn ensure_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.inputs.is_some() {
            return;
        }
        let saved = corvene_core::AppState::try_global(cx)
            .map(|s| s.read(cx).settings.font_specimen_text.clone())
            .unwrap_or_default();
        let glyph_search = cx
            .new(|cx| InputState::new(window, cx).placeholder("Search: a, U+0061, uni0061, name"));
        let specimen = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Type your own sample text")
                .default_value(saved)
        });
        let playground = cx.new(|cx| {
            InputState::new(window, cx).default_value("office affine 1/2 Hamburgefonstiv")
        });
        let field_filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter fields"));
        for input in [&glyph_search, &playground, &field_filter] {
            cx.subscribe(input, |_, _, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change) {
                    cx.notify();
                }
            })
            .detach();
        }
        cx.subscribe(&specimen, |_, input, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                let text = input.read(cx).value().to_string();
                Dispatcher::set_font_specimen_text(text, cx);
                cx.notify();
            }
        })
        .detach();
        self.inputs = Some(Inputs {
            glyph_search,
            specimen,
            playground,
            field_filter,
        });
    }

    /// Opens the Glyphs tab filtered to `filter` (the header chips).
    fn show_glyphs(&mut self, filter: GlyphFilter, cx: &mut Context<Self>) {
        self.face_ui().glyphs.filter = filter;
        self.face_ui().glyphs.selected = None;
        Dispatcher::set_font_diff_view(Tab::Glyphs, cx);
        cx.notify();
    }
}

fn status_color(status: Status, t: &GhdTheme) -> Hsla {
    match status {
        Status::Added => t.color_new,
        Status::Removed => t.color_deleted,
        Status::Modified => t.color_modified,
        Status::Unchanged => t.text_secondary,
    }
}

fn status_label(status: Status) -> &'static str {
    match status {
        Status::Added => "Added",
        Status::Removed => "Removed",
        Status::Modified => "Changed",
        Status::Unchanged => "Unchanged",
    }
}

/// A small coloured dot for a status.
fn status_dot(status: Status, t: &GhdTheme) -> Div {
    div()
        .flex_none()
        .size(zpx(7.))
        .rounded_full()
        .bg(status_color(status, t))
}

/// A segmented control: `labels`, `selected` highlighted, `on_pick(i)`.
fn segmented(
    id: &'static str,
    labels: Vec<SharedString>,
    selected: usize,
    on_pick: impl Fn(usize, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.ghd();
    let on_pick = std::rc::Rc::new(on_pick);
    div()
        .id(id)
        .flex_none()
        .flex()
        .flex_row()
        .h(zpx(24.))
        .p(zpx(2.))
        .gap(zpx(2.))
        .rounded(BORDER_RADIUS())
        .bg(t.box_alt_background)
        .border_1()
        .border_color(t.box_border)
        .children(labels.into_iter().enumerate().map(|(i, label)| {
            let on_pick = on_pick.clone();
            let selected = i == selected;
            let hover = t.box_hover_background;
            div()
                .id((id, i))
                .a11y_button(label.clone())
                .px(SPACING())
                .flex()
                .items_center()
                .rounded(zpx(3.))
                .text_size(FONT_SIZE_SM())
                .when(selected, |d| {
                    d.bg(t.tab_bar_active)
                        .text_color(t.box_selected_active_text)
                })
                .when(!selected, |d| {
                    d.text_color(t.text)
                        .cursor_pointer()
                        .hover(move |d| d.bg(hover))
                        .on_click(move |_, window, cx| on_pick(i, window, cx))
                })
                .child(label)
        }))
}

/// A pill showing a count with a status colour; clicking runs `on_click`.
fn chip(
    id: impl Into<ElementId>,
    text: String,
    color: Hsla,
    tooltip: &'static str,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.ghd();
    let hover = t.box_hover_background;
    div()
        .id(id)
        .a11y_button(tooltip)
        .flex_none()
        .h(zpx(20.))
        .px(SPACING_HALF())
        .flex()
        .items_center()
        .gap(zpx(4.))
        .rounded_full()
        .border_1()
        .border_color(t.box_border)
        .text_size(FONT_SIZE_SM())
        .text_color(t.text)
        .cursor_pointer()
        .hover(move |d| d.bg(hover))
        .ghd_tooltip(tooltip)
        .child(div().size(zpx(7.)).rounded_full().bg(color))
        .child(text)
        .on_click(move |_, window, cx| on_click(window, cx))
}

/// Code point as `U+0041`.
fn codepoint_label(cp: u32) -> String {
    format!("U+{cp:04X}")
}

/// A character for display (`None` for controls and unassigned values).
fn printable(cp: u32) -> Option<char> {
    char::from_u32(cp).filter(|c| !c.is_control())
}

impl FontDiffView {
    fn face_tabs(&self, diff: &FontDiff, cx: &mut Context<Self>) -> Option<AnyElement> {
        if diff.faces.len() < 2 {
            return None;
        }
        let t = cx.ghd().clone();
        Some(
            div()
                .id("font-face-tabs")
                .flex_none()
                .h(zpx(28.))
                .flex()
                .flex_row()
                .items_end()
                .px(SPACING())
                .gap(zpx(2.))
                .overflow_x_scroll()
                .border_b_1()
                .border_color(t.box_border)
                .children(diff.faces.iter().enumerate().map(|(i, f)| {
                    let selected = i == self.face;
                    let index = f
                        .new
                        .as_ref()
                        .or(f.old.as_ref())
                        .map(|s| s.index + 1)
                        .unwrap_or(0);
                    let tip = format!("{} · face {index} · {}", f.title, status_label(f.status));
                    div()
                        .id(("font-face-tab", i))
                        .a11y_button(f.title.clone())
                        .flex_none()
                        .h(zpx(24.))
                        .px(SPACING())
                        .flex()
                        .items_center()
                        .gap(zpx(6.))
                        .rounded_t(BORDER_RADIUS())
                        .text_size(FONT_SIZE_SM())
                        .text_color(if selected { t.text } else { t.text_secondary })
                        .when(selected, |d| {
                            d.bg(t.box_background)
                                .border_1()
                                .border_b_0()
                                .border_color(t.box_border)
                        })
                        .when(!selected, |d| {
                            d.cursor_pointer().hover(|d| d.text_color(t.text))
                        })
                        .ghd_tooltip(tip)
                        .child(status_dot(f.status, &t))
                        .child(f.title.clone())
                        .on_click(cx.listener(move |this, _, _, cx| this.select_face(i, cx)))
                }))
                .into_any_element(),
        )
    }

    fn summary_chips(&self, face: &FaceDiff, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = cx.ghd().clone();
        let s = face.summary;
        let mut out = Vec::new();
        let view = cx.entity().downgrade();
        let glyph_chip = |id: &'static str,
                          n: u32,
                          sign: &str,
                          color: Hsla,
                          tip: &'static str,
                          filter: GlyphFilter,
                          cx: &App| {
            let view = view.clone();
            chip(
                id,
                format!("{sign}{n}"),
                color,
                tip,
                move |_, cx| {
                    view.update(cx, |v, cx| v.show_glyphs(filter, cx)).ok();
                },
                cx,
            )
            .into_any_element()
        };
        if face.old.is_some() && face.new.is_some() {
            if s.glyphs_added > 0 {
                out.push(glyph_chip(
                    "font-chip-added",
                    s.glyphs_added,
                    "+",
                    t.color_new,
                    "Glyphs added",
                    GlyphFilter::Added,
                    cx,
                ));
            }
            if s.glyphs_removed > 0 {
                out.push(glyph_chip(
                    "font-chip-removed",
                    s.glyphs_removed,
                    "−",
                    t.color_deleted,
                    "Glyphs removed",
                    GlyphFilter::Removed,
                    cx,
                ));
            }
            if s.glyphs_modified > 0 {
                out.push(glyph_chip(
                    "font-chip-modified",
                    s.glyphs_modified,
                    "~",
                    t.color_modified,
                    "Glyphs changed",
                    GlyphFilter::Changed,
                    cx,
                ));
            }
            let tab_chip =
                |id: &'static str, text: String, tip: &'static str, tab: Tab, cx: &App| {
                    chip(
                        id,
                        text,
                        t.color_modified,
                        tip,
                        move |_, cx| Dispatcher::set_font_diff_view(tab, cx),
                        cx,
                    )
                    .into_any_element()
                };
            if s.features_changed > 0 {
                out.push(tab_chip(
                    "font-chip-features",
                    format!("{} features", s.features_changed),
                    "OpenType features changed",
                    Tab::Features,
                    cx,
                ));
            }
            if s.kerning_changed > 0 {
                out.push(tab_chip(
                    "font-chip-kerning",
                    format!("{} kerning pairs", s.kerning_changed),
                    "Kerning pairs changed",
                    Tab::Features,
                    cx,
                ));
            }
            if s.axes_changed + s.instances_changed > 0 {
                out.push(tab_chip(
                    "font-chip-axes",
                    format!("{} axes & instances", s.axes_changed + s.instances_changed),
                    "Variation axes or named instances changed",
                    Tab::Overview,
                    cx,
                ));
            }
            if s.tables_changed > 0 {
                out.push(tab_chip(
                    "font-chip-tables",
                    format!("{} tables", s.tables_changed),
                    "Tables whose bytes changed",
                    Tab::Tables,
                    cx,
                ));
            }
        } else {
            let n = face.glyphs.len();
            out.push(
                div()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(format!("{n} glyphs"))
                    .into_any_element(),
            );
        }
        out
    }

    fn tab_bar(&self, face: Option<&FaceDiff>, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.ghd().clone();
        let tab = Self::tab(cx);
        let tabs = [
            Tab::Overview,
            Tab::Specimen,
            Tab::Glyphs,
            Tab::Tables,
            Tab::Features,
        ];
        let labels = ["Overview", "Specimen", "Glyphs", "Tables", "Features"];
        let selected = tabs.iter().position(|x| *x == tab).unwrap_or(0);
        div()
            .flex_none()
            .h(zpx(36.))
            .px(SPACING())
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING())
            .border_b_1()
            .border_color(t.box_border)
            .bg(t.box_background)
            .child(segmented(
                "font-diff-tabs",
                labels.map(SharedString::from).to_vec(),
                selected,
                move |i, _, cx| Dispatcher::set_font_diff_view(tabs[i], cx),
                cx,
            ))
            .child(div().flex_1())
            .children(face.map(|f| {
                div()
                    .flex()
                    .flex_row()
                    .gap(zpx(6.))
                    .children(self.summary_chips(f, cx))
            }))
            .into_any_element()
    }

    fn message(&self, icon: Octicon, title: &str, detail: Option<String>, cx: &App) -> AnyElement {
        let t = cx.ghd();
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(SPACING())
            .text_color(t.text_secondary)
            .child(octicon(icon, t.text_secondary))
            .child(
                div()
                    .text_size(FONT_SIZE_MD())
                    .text_color(t.text)
                    .child(title.to_string()),
            )
            .children(detail.map(|d| div().text_size(FONT_SIZE_SM()).child(d)))
            .into_any_element()
    }
}

impl Focusable for FontDiffView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for FontDiffView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_inputs(window, cx);
        let scale = window.scale_factor();
        if scale != self.scale {
            self.rasters.clear();
            self.scale = scale;
        }
        let t = cx.ghd().clone();
        let body: AnyElement = match &self.phase {
            Phase::Reading => self.message(Octicon::Sync, "Reading font…", None, cx),
            Phase::Failed(e) => self.message(
                Octicon::FileBinary,
                "This font could not be read.",
                Some(e.clone()),
                cx,
            ),
            Phase::Ready(diff) => {
                let diff = diff.clone();
                let face = self.current_face();
                let errors: Vec<String> = [(&diff.old_error, "old"), (&diff.new_error, "new")]
                    .into_iter()
                    .filter_map(|(e, side)| {
                        e.as_ref()
                            .map(|e| format!("The {side} version could not be read: {e}"))
                    })
                    .collect();
                let content = match &face {
                    None => self.message(Octicon::Sync, "Comparing glyphs…", None, cx),
                    Some(face) => match Self::tab(cx) {
                        Tab::Overview => self.overview(face, window, cx),
                        Tab::Specimen => self.specimen(face, window, cx),
                        Tab::Glyphs => self.glyphs(face, window, cx),
                        Tab::Tables => self.tables(face, window, cx),
                        Tab::Features => self.features(face, window, cx),
                    },
                };
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .children(self.face_tabs(&diff, cx))
                    .child(self.tab_bar(face.as_deref(), cx))
                    .children(errors.into_iter().map(|e| {
                        div()
                            .flex_none()
                            .px(SPACING())
                            .py(SPACING_HALF())
                            .bg(t.file_warning_background)
                            .border_b_1()
                            .border_color(t.file_warning_border)
                            .text_size(FONT_SIZE_SM())
                            .text_color(t.text)
                            .child(e)
                    }))
                    .child(div().flex_1().min_h_0().child(content))
                    .into_any_element()
            }
        };
        div()
            .id("font-diff")
            .track_focus(&self.focus_handle)
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
