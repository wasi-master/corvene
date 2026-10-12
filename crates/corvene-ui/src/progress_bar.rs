//! Corvene: a determinate progress bar for long reads (Blame's attribution,
//! Insights' history pass), and an indeterminate one for work that reports
//! no progress (drawing an SVG picture). GitHub Desktop has neither.
//!
//! Progress arrives in bursts (a publish every so often), so the fill eases
//! towards the reported fraction instead of jumping: an exponential approach
//! with a 150 ms time constant, one animation frame at a time while it is
//! still moving. A fraction lower than the one shown (a new load) is taken at
//! once. The shown value lives per `key` in a thread local, so it survives
//! re-renders of the view that draws the bar.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Instant;

use gpui_kit::*;

use crate::theme::sizes::*;

/// The easing's time constant, in seconds.
const TAU: f32 = 0.15;
/// Closer than this to the target counts as there.
const SETTLED: f32 = 0.001;

#[derive(Clone, Copy)]
struct Shown {
    value: f32,
    at: Instant,
}

thread_local! {
    static SHOWN: RefCell<HashMap<SharedString, Shown>> = RefCell::new(HashMap::new());
}

/// The fraction to draw for `key` this frame, and whether it is still
/// moving towards `target`.
fn step(key: &SharedString, target: f32, now: Instant) -> (f32, bool) {
    SHOWN.with(|shown| {
        let mut shown = shown.borrow_mut();
        let entry = shown.entry(key.clone()).or_insert(Shown {
            value: target,
            at: now,
        });
        if target < entry.value {
            entry.value = target;
        } else {
            let dt = now.saturating_duration_since(entry.at).as_secs_f32();
            entry.value += (target - entry.value) * (1. - (-dt / TAU).exp());
        }
        entry.at = now;
        let moving = (target - entry.value).abs() > SETTLED;
        if !moving {
            entry.value = target;
        }
        (entry.value, moving)
    })
}

/// Forget `key`'s shown value, so the next bar under it starts from its
/// first fraction instead of easing from the last load's.
pub fn reset(key: &str) {
    SHOWN.with(|shown| {
        shown.borrow_mut().remove(key);
    });
}

/// A `width` wide, 4 px high bar: `fraction` (0…1) of it filled with
/// `color` over a fainter track of the same colour.
pub fn progress_bar(
    key: impl Into<SharedString>,
    fraction: f32,
    color: Hsla,
    width: Pixels,
) -> impl IntoElement {
    let key = key.into();
    let target = if fraction.is_finite() {
        fraction.clamp(0., 1.)
    } else {
        0.
    };
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let (value, moving) = step(&key, target, Instant::now());
            if moving {
                window.request_animation_frame();
            }
            let radius = bounds.size.height / 2.;
            window.paint_quad(fill(bounds, color.opacity(0.25)).corner_radii(radius));
            if value > 0. {
                let filled = Bounds {
                    origin: bounds.origin,
                    size: size(
                        (bounds.size.width * value).max(bounds.size.height),
                        bounds.size.height,
                    ),
                };
                window.paint_quad(fill(filled, color).corner_radii(radius));
            }
        },
    )
    .flex_none()
    .w(width)
    .h(zpx(4.))
}

/// How long the indeterminate bar's segment takes to cross, in seconds.
const SWEEP: f32 = 1.2;

/// A `width` wide, 4 px high bar for work of unknown length: a third of
/// it, in `color`, sweeps across a fainter track, one animation frame at
/// a time while it is drawn.
pub fn indeterminate_bar(color: Hsla, width: Pixels) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            window.request_animation_frame();
            let radius = bounds.size.height / 2.;
            window.paint_quad(fill(bounds, color.opacity(0.25)).corner_radii(radius));
            let t = START.with(|start| start.elapsed().as_secs_f32()) % SWEEP / SWEEP;
            let segment = bounds.size.width / 3.;
            // from fully left of the track to fully right of it, clipped
            let left = (bounds.size.width + segment) * t - segment;
            let x0 = left.max(px(0.));
            let x1 = (left + segment).min(bounds.size.width);
            if x1 > x0 {
                let filled = Bounds {
                    origin: point(bounds.origin.x + x0, bounds.origin.y),
                    size: size(x1 - x0, bounds.size.height),
                };
                window.paint_quad(fill(filled, color).corner_radii(radius));
            }
        },
    )
    .flex_none()
    .w(width)
    .h(zpx(4.))
}

thread_local! {
    static START: Instant = Instant::now();
}
