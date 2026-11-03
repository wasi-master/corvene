//! Progress of a branch checkout, the submodule update after it and a
//! revert - GHD's `progressCallback` of `checkoutBranch`
//! (`lib/git/checkout.ts`), `updateSubmodulesAfterOperation`
//! (`lib/git/submodule.ts`) and `revertCommit` (`lib/git/revert.ts`), all
//! run with `trackLFSProgress` so Git LFS transfers show
//! ([`crate::lfs_progress::run_with_progress`]).
//!
//! GHD passes the callback down as an argument. Here a caller wraps the
//! operation in [`with_progress`] (as [`crate::hooks::with_hook_callbacks`]
//! does for hooks) and the git functions report to it while it is set; with
//! none set they run as before, without `--progress`. The callback is
//! called on the thread that runs the git function.

use std::cell::{Cell, RefCell};

use crate::lfs_progress::LineParser;
use crate::remote_ops::{GitProgressEvent, ProgressParser};

/// GHD `ICheckoutProgress` / `IRevertProgress` (`models/progress.ts`)
/// without the kind.
#[derive(Clone, Debug, PartialEq)]
pub struct OperationProgress {
    /// The operation (`Checking out branch main`), or for a revert the
    /// progress line's title (`Downloading "big.bin"`, empty for context).
    pub title: String,
    pub description: String,
    /// 0..1.
    pub value: f64,
}

type Sink = Box<dyn FnMut(OperationProgress)>;

thread_local! {
    static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
    /// GHD `clampProgress(min, max, callback)`: the part of the whole the
    /// running step reports into.
    static RANGE: Cell<(f64, f64)> = const { Cell::new((0., 1.)) };
    /// The title the last report had (the submodule update reports under
    /// the checkout's).
    static TITLE: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Run `f` with `sink` receiving the progress of the checkouts, submodule
/// updates and reverts it runs on this thread.
pub fn with_progress<R>(sink: impl FnMut(OperationProgress) + 'static, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Sink>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            SINK.with(|s| *s.borrow_mut() = previous);
        }
    }
    let _restore = Restore(SINK.with(|s| s.replace(Some(Box::new(sink)))));
    f()
}

/// Whether a [`with_progress`] sink is set on this thread.
pub(crate) fn active() -> bool {
    SINK.with(|s| s.borrow().is_some())
}

/// Report `value` (0..1 of the running step) to the sink.
pub(crate) fn report(title: &str, description: &str, value: f64) {
    let (min, max) = RANGE.with(Cell::get);
    TITLE.with(|t| title.clone_into(&mut t.borrow_mut()));
    let progress = OperationProgress {
        title: title.to_string(),
        description: description.to_string(),
        value: min + (max - min) * value.clamp(0., 1.),
    };
    SINK.with(|s| {
        if let Some(sink) = s.borrow_mut().as_mut() {
            sink(progress);
        }
    });
}

/// The title of the last report.
pub(crate) fn title() -> String {
    TITLE.with(|t| t.borrow().clone())
}

/// GHD `CheckoutStepWeight`: a checkout's share of its progress, the
/// submodule update after it taking the rest.
pub const CHECKOUT_STEP_WEIGHT: f64 = 0.9;

/// The submodule update run after a checkout that skipped it
/// (`SubmoduleUpdate::None`): its reports go where the checkout's own
/// update would have.
pub fn submodule_step<R>(f: impl FnOnce() -> R) -> R {
    clamped(CHECKOUT_STEP_WEIGHT, 1., f)
}

/// GHD `clampProgress(min, max, …)`: run `f` with its reports scaled into
/// `min..max` of the current range.
pub(crate) fn clamped<R>(min: f64, max: f64, f: impl FnOnce() -> R) -> R {
    struct Restore((f64, f64));
    impl Drop for Restore {
        fn drop(&mut self) {
            RANGE.with(|r| r.set(self.0));
        }
    }
    let outer = RANGE.with(Cell::get);
    let _restore = Restore(outer);
    let span = outer.1 - outer.0;
    RANGE.with(|r| r.set((outer.0 + span * min, outer.0 + span * max)));
    f()
}

/// GHD `CheckoutProgressParser` (`lib/progress/checkout.ts`).
pub(crate) fn checkout_parser() -> ProgressParser {
    ProgressParser::new(&[("Checking out files", 1.)])
}

/// GHD `RevertProgressParser` (`lib/progress/revert.ts`): one step titled
/// `''`, which no line has, so git's lines are all context (GHD's weight 0
/// makes its own weights `NaN`, never used).
pub(crate) fn revert_parser() -> ProgressParser {
    ProgressParser::new(&[("", 1.)])
}

/// The parser `updateSubmodulesAfterOperation` hands
/// `executionOptionsWithProgress`: every line is context, `Updating
/// submodules: <line>`, at a percent that slows down as submodules are
/// checked out or cloned (git does not say how many there are).
#[derive(Default)]
pub(crate) struct SubmoduleUpdateParser {
    events: u32,
}

impl LineParser for SubmoduleUpdateParser {
    fn parse_event(&mut self, line: &str) -> GitProgressEvent {
        let checked_out = line
            .strip_prefix("Submodule path ")
            .and_then(|rest| rest.find(": checked out "))
            .is_some_and(|at| at > 0);
        if checked_out || line.starts_with("Cloning into ") {
            self.events += 1;
        }
        GitProgressEvent::Context {
            // https://math.stackexchange.com/a/2323106, as in GHD
            percent: 1. - (-f64::from(self.events) * 0.25).exp(),
            text: format!("Updating submodules: {line}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    fn record(f: impl FnOnce()) -> Vec<OperationProgress> {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let sink = seen.clone();
        with_progress(move |p| sink.borrow_mut().push(p), f);
        Rc::try_unwrap(seen)
            .map(RefCell::into_inner)
            .unwrap_or_default()
    }

    #[test]
    fn reports_scale_into_the_clamped_range() {
        let seen = record(|| {
            clamped(0., 0.9, || report("t", "half", 0.5));
            clamped(0.9, 1., || report("t", "done", 1.));
        });
        let values: Vec<f64> = seen
            .iter()
            .map(|p| (p.value * 1000.).round() / 1000.)
            .collect();
        assert_eq!(values, vec![0.45, 1.]);
        assert!(!active());
    }

    #[test]
    fn no_sink_no_reports() {
        assert!(!active());
        report("t", "nobody listens", 1.);
    }

    #[test]
    fn submodule_lines_slow_down() {
        let mut parser = SubmoduleUpdateParser::default();
        let first = parser.parse_event("Cloning into '/repo/sub'...");
        let GitProgressEvent::Context { percent, text } = first else {
            panic!("context");
        };
        assert_eq!(text, "Updating submodules: Cloning into '/repo/sub'...");
        assert!((percent - (1. - (-0.25f64).exp())).abs() < 1e-9);
        let other = parser.parse_event("remote: Counting objects");
        assert!((other.percent() - percent).abs() < 1e-9);
        let checked = parser.parse_event("Submodule path 'sub': checked out 'abc'");
        assert!(checked.percent() > percent);
    }
}
