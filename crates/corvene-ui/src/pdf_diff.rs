//! Corvene `1322-pdf-diff` (desktop/desktop#5009, #15875): a changed PDF
//! file shown page by page. GHD shows "This binary file has changed."
//! (`lib/git/diff.ts` `getDiffType`, `ui/diff/binary-file.tsx`).
//!
//! Both versions are opened once, off the main thread
//! (`corvene_core::pdf`). The current page pair is drawn to PNGs in the
//! background and shown by an [`ImageDiff`], so the usual modes (2-up,
//! Swipe, Onion Skin, Difference) and image settings apply. Pages pair up by
//! number; a page only one version has shows as added or deleted. A
//! toolbar above steps through the pages and jumps to the next page whose
//! thumbnail differs (documents of up to
//! [`corvene_core::pdf::MAX_SCANNED_PAGES`] pages are scanned). A file
//! that does not open (encrypted, broken) reports [`PdfDiff::failed`] and the
//! diff view shows the binary panel.

use std::collections::VecDeque;

use corvene_core::pdf::{PdfDocument, PdfError};
use corvene_core::{FileStatusKind, ImageBlob};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::format::format_bytes;
use crate::image_diff::ImageDiff;
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::{GhdTooltip, IconButtonA11y};

/// The longer side of a drawn page, in pixels: sharp on a 2x display at
/// most window sizes, without drawing again on every resize.
const PAGE_PX: u32 = 2000;
/// Drawn page pairs kept (their image diffs keep their sliders).
const KEPT_PAGES: usize = 6;

enum Docs {
    Loading,
    Failed(PdfError),
    Ready {
        previous: Option<PdfDocument>,
        current: Option<PdfDocument>,
    },
}

pub struct PdfDiff {
    status: FileStatusKind,
    /// The file sizes of (previous, current), for the footers.
    bytes: (Option<usize>, Option<usize>),
    docs: Docs,
    /// The shown page (0-based).
    page: usize,
    /// Whether each page pair differs, once scanned (`None` while scanning
    /// or for long documents).
    changed: Option<Vec<bool>>,
    /// Drawn page pairs, most recent last.
    drawn: VecDeque<(usize, Entity<ImageDiff>)>,
    /// The page being drawn.
    drawing: Option<usize>,
    _tasks: Vec<Task<()>>,
}

impl PdfDiff {
    pub fn new(
        previous: Option<&ImageBlob>,
        current: Option<&ImageBlob>,
        status: FileStatusKind,
        cx: &mut Context<Self>,
    ) -> Self {
        let bytes = (
            previous.map(|b| b.bytes.len()),
            current.map(|b| b.bytes.len()),
        );
        let (old, new) = (
            previous.map(|b| b.bytes.clone()),
            current.map(|b| b.bytes.clone()),
        );
        let open = cx.background_executor().spawn(async move {
            let open = |bytes: Option<Vec<u8>>| bytes.map(PdfDocument::open).transpose();
            Ok::<_, PdfError>((open(old)?, open(new)?))
        });
        let task = cx.spawn(async move |this, cx| {
            let docs = open.await;
            this.update(cx, |this, cx| {
                this.docs = match docs {
                    Ok((previous, current)) => Docs::Ready { previous, current },
                    Err(e) => Docs::Failed(e),
                };
                this.scan(cx);
                cx.notify();
            })
            .ok();
        });
        Self {
            status,
            bytes,
            docs: Docs::Loading,
            page: 0,
            changed: None,
            drawn: VecDeque::new(),
            drawing: None,
            _tasks: vec![task],
        }
    }

    /// Why the file cannot be shown as pages.
    pub fn failed(&self) -> Option<PdfError> {
        match self.docs {
            Docs::Failed(e) => Some(e),
            _ => None,
        }
    }

    fn page_count(&self) -> usize {
        match &self.docs {
            Docs::Ready { previous, current } => {
                let count = |d: &Option<PdfDocument>| d.as_ref().map_or(0, PdfDocument::page_count);
                count(previous).max(count(current))
            }
            _ => 0,
        }
    }

    /// Finds the changed pages in the background.
    fn scan(&mut self, cx: &mut Context<Self>) {
        let Docs::Ready { previous, current } = &self.docs else {
            return;
        };
        // one side only: every page is added or deleted
        if previous.is_none() || current.is_none() {
            self.changed = Some(vec![true; self.page_count()]);
            return;
        }
        let (previous, current) = (previous.clone(), current.clone());
        let scan = cx.background_executor().spawn(async move {
            corvene_core::pdf::changed_pages(previous.as_ref(), current.as_ref())
        });
        self._tasks.push(cx.spawn(async move |this, cx| {
            let changed = scan.await;
            this.update(cx, |this, cx| {
                this.changed = changed;
                cx.notify();
            })
            .ok();
        }));
    }

    fn go_to(&mut self, page: usize, cx: &mut Context<Self>) {
        let page = page.min(self.page_count().saturating_sub(1));
        if page != self.page {
            self.page = page;
            cx.notify();
        }
    }

    fn next_changed(&self) -> Option<usize> {
        next_changed(self.page, self.changed.as_deref()?)
    }

    /// The image diff of the shown page, drawing it when needed.
    fn page_diff(&mut self, cx: &mut Context<Self>) -> Option<Entity<ImageDiff>> {
        let page = self.page;
        if let Some(ix) = self.drawn.iter().position(|(p, _)| *p == page) {
            let entry = self.drawn.remove(ix)?;
            let diff = entry.1.clone();
            self.drawn.push_back(entry);
            return Some(diff);
        }
        if self.drawing == Some(page) {
            return None;
        }
        let Docs::Ready { previous, current } = &self.docs else {
            return None;
        };
        self.drawing = Some(page);
        let (previous, current) = (previous.clone(), current.clone());
        let draw = cx.background_executor().spawn(async move {
            let draw = |doc: &Option<PdfDocument>| {
                let doc = doc.as_ref()?;
                let size = doc.page_size(page)?;
                let image = doc.render(page, PAGE_PX)?;
                Some((image, size))
            };
            (draw(&previous), draw(&current))
        });
        self._tasks.push(cx.spawn(async move |this, cx| {
            let (previous, current) = draw.await;
            this.update(cx, |this, cx| {
                if this.drawing == Some(page) {
                    this.drawing = None;
                }
                let blob = |side: &Option<(corvene_core::pdf::PageImage, (f32, f32))>| {
                    side.as_ref().map(|(image, _)| ImageBlob {
                        bytes: image.png.clone(),
                        media_type: "image/png".to_string(),
                    })
                };
                let (old, new) = (blob(&previous), blob(&current));
                let footer = |side: &Option<(corvene_core::pdf::PageImage, (f32, f32))>,
                              bytes: Option<usize>| {
                    let size = side
                        .as_ref()
                        .map(|(_, (w, h))| format!("{} × {} pt", w.round(), h.round()))
                        .unwrap_or_default();
                    let bytes = bytes
                        .map(|b| format!(" | File: {}", format_bytes(b as i64, 2)))
                        .unwrap_or_default();
                    SharedString::from(format!("{size}{bytes}"))
                };
                let footers = (
                    footer(&previous, this.bytes.0),
                    footer(&current, this.bytes.1),
                );
                let status = this.status;
                let diff = cx.new(|cx| {
                    ImageDiff::new(old.as_ref(), new.as_ref(), status, cx)
                        .with_page_footers(footers.0, footers.1)
                });
                this.drawn.push_back((page, diff));
                while this.drawn.len() > KEPT_PAGES {
                    this.drawn.pop_front();
                }
                cx.notify();
            })
            .ok();
        }));
        None
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.ghd().clone();
        let pages = self.page_count();
        let counts = match &self.docs {
            Docs::Ready {
                previous: Some(p),
                current: Some(c),
            } if p.page_count() != c.page_count() => {
                Some(format!("{} → {} pages", p.page_count(), c.page_count()))
            }
            _ => None,
        };
        let summary = match &self.changed {
            Some(changed) => match changed.iter().filter(|c| **c).count() {
                0 => "No visible changes".to_string(),
                1 => "1 page changed".to_string(),
                n => format!("{n} pages changed"),
            },
            None if pages > corvene_core::pdf::MAX_SCANNED_PAGES => String::new(),
            None => "Comparing pages…".to_string(),
        };
        let page_changed = self
            .changed
            .as_ref()
            .and_then(|c| c.get(self.page).copied());
        let next_changed = self.next_changed();
        let button = |id: &'static str, label: &'static str, enabled: bool| {
            div()
                .id(id)
                .a11y_button(label)
                .flex_none()
                .h(zpx(21.))
                .px(SPACING())
                .flex()
                .items_center()
                .rounded(BORDER_RADIUS())
                .border_1()
                .border_color(t.box_border)
                .text_size(FONT_SIZE())
                .text_color(if enabled { t.text } else { t.text_secondary })
                .when(enabled, |d| {
                    let hover = t.box_hover_background;
                    d.cursor_pointer().hover(move |s| s.bg(hover))
                })
                .child(label)
        };
        let page = self.page;
        div()
            .id("pdf-diff-toolbar")
            .flex_none()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .justify_center()
            .gap(SPACING())
            .pt(SPACING())
            .text_size(FONT_SIZE())
            .text_color(t.text_secondary)
            .child(
                button("pdf-diff-previous", "Previous", page > 0).when(page > 0, |d| {
                    d.on_click(cx.listener(move |this, _, _, cx| this.go_to(page - 1, cx)))
                }),
            )
            .child(
                div()
                    .id("pdf-diff-page")
                    .min_w(zpx(90.))
                    .flex()
                    .justify_center()
                    .text_color(match page_changed {
                        Some(true) => t.text,
                        _ => t.text_secondary,
                    })
                    .when(page_changed == Some(true), |d| {
                        d.font_weight(FontWeight::SEMIBOLD)
                    })
                    .child(format!("Page {} of {}", page + 1, pages.max(1)))
                    .when(page_changed == Some(false), |d| {
                        d.ghd_tooltip("This page looks the same in both versions")
                    }),
            )
            .child(
                button("pdf-diff-next", "Next", page + 1 < pages).when(page + 1 < pages, |d| {
                    d.on_click(cx.listener(move |this, _, _, cx| this.go_to(page + 1, cx)))
                }),
            )
            .child(
                button(
                    "pdf-diff-next-changed",
                    "Next Change",
                    next_changed.is_some(),
                )
                .when_some(next_changed, |d, next| {
                    d.on_click(cx.listener(move |this, _, _, cx| this.go_to(next, cx)))
                }),
            )
            .children(counts)
            .when(!summary.is_empty(), |d| d.child(summary))
            .into_any_element()
    }
}

/// The first changed page after `page` (wrapping around), not `page`.
fn next_changed(page: usize, changed: &[bool]) -> Option<usize> {
    let n = changed.len();
    (1..=n)
        .map(|k| (page + k) % n)
        .find(|&i| changed[i] && i != page)
}

impl Render for PdfDiff {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd().clone();
        let message = |text: &str| {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .gap(SPACING())
                .text_size(FONT_SIZE())
                .text_color(t.text_secondary)
                .child(crate::icons::loading("pdf-diff-loading", t.text_secondary))
                .child(text.to_string())
                .into_any_element()
        };
        let body = match &self.docs {
            Docs::Loading => message("Opening the PDF file…"),
            // the diff view shows the binary panel
            Docs::Failed(_) => div().into_any_element(),
            Docs::Ready { .. } => {
                let page = match self.page_diff(cx) {
                    Some(diff) => diff.into_any_element(),
                    None => message("Drawing the page…"),
                };
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .min_h_0()
                    .child(self.toolbar(cx))
                    .child(div().flex_1().min_h_0().flex().child(page))
                    .into_any_element()
            }
        };
        div()
            .id("pdf-diff")
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(t.background)
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::next_changed;

    #[test]
    fn next_change_wraps_and_skips_the_shown_page() {
        assert_eq!(next_changed(0, &[true, false, true]), Some(2));
        assert_eq!(next_changed(2, &[true, false, true]), Some(0));
        assert_eq!(next_changed(1, &[false, true, false]), None);
        assert_eq!(next_changed(0, &[]), None);
    }
}
