//! Typeset TeX math in the UI (`1331-notebook-diff`, `1332-latex-diff`,
//! `1333-markdown-math`), drawn by `corvene_math` (RaTeX, a KaTeX port) on
//! the background executor and kept as GPUI images for the session.
//!
//! A formula is asked for by what it depends on (TeX, inline or display,
//! size, ink colour, the document's macros). The first ask reserves a box
//! of about the right size and starts the job once the element is laid
//! out; when the raster is ready the windows refresh. A formula that does
//! not parse shows its source in the error colour, the message on hover,
//! as KaTeX does with `throwOnError: false`.
//!
//! GitHub Desktop renders no math. Rasters are drawn at 2× (Retina) and
//! scaled for other displays, since a render has no window to ask.

use std::collections::{HashMap, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use corvene_math::{Macros, Mode};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::zpx;
use crate::widgets::GhdTooltip;

/// Device pixels per logical pixel the rasters are drawn at.
const SCALE: f32 = 2.0;
/// Formulas kept; the oldest go first.
const MAX_ENTRIES: usize = 1024;
/// KaTeX sets math at 1.21 × the surrounding text.
const EM_RATIO: f32 = 1.21;

#[derive(Clone)]
enum Entry {
    Pending,
    Ready {
        image: Arc<RenderImage>,
        width: f32,
        height: f32,
        /// Logical px from the top to the baseline.
        baseline: f32,
    },
    Failed(SharedString),
}

#[derive(Default)]
struct MathImages {
    entries: HashMap<u64, Entry>,
    order: VecDeque<u64>,
}

impl Global for MathImages {}

/// The macros of a document, shared by its formulas.
pub type SharedMacros = Arc<Macros>;

/// What a formula's drawing depends on.
#[derive(Clone)]
struct Job {
    tex: String,
    mode: Mode,
    em: f32,
    color: [u8; 3],
    macros: SharedMacros,
}

impl Job {
    fn key(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.tex.hash(&mut h);
        self.mode.hash(&mut h);
        self.em.to_bits().hash(&mut h);
        self.color.hash(&mut h);
        self.macros.hash(&mut h);
        h.finish()
    }
}

fn rgb(color: Hsla) -> [u8; 3] {
    let c: Rgba = color.into();
    [
        (c.r * 255.0).round() as u8,
        (c.g * 255.0).round() as u8,
        (c.b * 255.0).round() as u8,
    ]
}

fn lookup(key: u64, cx: &App) -> Option<Entry> {
    cx.try_global::<MathImages>()?.entries.get(&key).cloned()
}

/// Starts drawing `job` unless it is drawn or drawing.
fn request(job: Job, cx: &mut App) {
    let key = job.key();
    let images = cx.default_global::<MathImages>();
    if images.entries.contains_key(&key) {
        return;
    }
    images.entries.insert(key, Entry::Pending);
    images.order.push_back(key);
    while images.order.len() > MAX_ENTRIES {
        if let Some(old) = images.order.pop_front() {
            images.entries.remove(&old);
        }
    }
    let task = cx.background_executor().spawn(async move {
        let req = corvene_math::Request {
            tex: &job.tex,
            mode: job.mode,
            em: job.em,
            scale: SCALE,
            color: job.color,
            macros: &job.macros,
        };
        match corvene_math::render(&req) {
            Ok(raster) => {
                let buffer =
                    image::RgbaImage::from_raw(raster.width, raster.height, raster.bgra.clone());
                match buffer {
                    Some(buffer) => {
                        let (width, height) = raster.size();
                        Entry::Ready {
                            image: Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])),
                            width,
                            height,
                            baseline: raster.logical_baseline(),
                        }
                    }
                    None => Entry::Failed("could not draw formula".into()),
                }
            }
            Err(e) => Entry::Failed(e.to_string().into()),
        }
    });
    cx.spawn(async move |cx| {
        let entry = task.await;
        cx.update(|cx| {
            let images = cx.default_global::<MathImages>();
            if images.entries.contains_key(&key) {
                images.entries.insert(key, entry);
            }
            cx.refresh_windows();
        });
    })
    .detach();
}

/// A formula in running text (`$…$`; `display` for `$$…$$` amid text), its
/// baseline on the text's, at `font_size` text.
pub fn inline(
    id: impl Into<ElementId>,
    tex: &str,
    display: bool,
    macros: &SharedMacros,
    font_size: Pixels,
    cx: &App,
) -> AnyElement {
    let mode = if display { Mode::Display } else { Mode::Inline };
    formula(id.into(), tex, mode, macros, font_size, cx)
}

/// A display formula on its own line, centred, with a LaTeX equation
/// number on the right.
pub fn block(
    id: impl Into<ElementId>,
    tex: &str,
    number: Option<&str>,
    macros: &SharedMacros,
    font_size: Pixels,
    cx: &App,
) -> AnyElement {
    let t = cx.ghd();
    div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .py(zpx(4.))
        .child(div().flex_1())
        .child(
            div()
                .id("math-block-scroll")
                .min_w_0()
                .max_w_full()
                .overflow_x_scroll()
                .child(formula(
                    id.into(),
                    tex,
                    Mode::Display,
                    macros,
                    font_size,
                    cx,
                )),
        )
        .child(
            div()
                .flex_1()
                .flex()
                .flex_row()
                .justify_end()
                .text_color(t.text_secondary)
                .children(number.map(|n| SharedString::from(n.to_string()))),
        )
        .into_any_element()
}

fn formula(
    id: ElementId,
    tex: &str,
    mode: Mode,
    macros: &SharedMacros,
    font_size: Pixels,
    cx: &App,
) -> AnyElement {
    let t = cx.ghd();
    let em = f32::from(font_size) * EM_RATIO;
    let job = Job {
        tex: tex.trim().to_string(),
        mode,
        em,
        color: rgb(t.text),
        macros: macros.clone(),
    };
    let key = job.key();
    let source = SharedString::from(tex.trim().to_string());
    match lookup(key, cx) {
        Some(Entry::Ready {
            image,
            width,
            height,
            baseline,
        }) => {
            // the depth below the baseline hangs under the text's bottom
            let depth = (height - baseline).max(0.);
            let descent = f32::from(font_size) * 0.28;
            div()
                .id(id)
                .flex_none()
                .when(mode == Mode::Inline, |d| d.mb(px(descent - depth)))
                .child(img(ImageSource::Render(image)).w(px(width)).h(px(height)))
                .ghd_tooltip(source)
                .into_any_element()
        }
        Some(Entry::Failed(message)) => div()
            .id(id)
            .flex_none()
            .font_family(crate::theme::mono_font())
            .text_color(t.error)
            .child(source.clone())
            .ghd_tooltip(message)
            .into_any_element(),
        other => {
            // about the size it will be, so the layout does not jump much
            let chars = tex.trim().chars().count().clamp(1, 80) as f32;
            let (w, h) = match mode {
                Mode::Inline => (chars * em * 0.38, em * 1.1),
                Mode::Display => (chars * em * 0.32, em * 2.2),
            };
            let pending = other.is_some();
            div()
                .id(id)
                .flex_none()
                .w(px(w.min(600.)))
                .h(px(h))
                .rounded(zpx(2.))
                .bg(t.md_neutral_muted)
                .when(!pending, |d| {
                    d.child(canvas(
                        move |_, _, cx| {
                            let job = job.clone();
                            cx.defer(move |cx| request(job, cx))
                        },
                        |_, _, _, _| {},
                    ))
                })
                .into_any_element()
        }
    }
}
