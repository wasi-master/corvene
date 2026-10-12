//! Corvene `1322-pdf-diff` (desktop/desktop#5009, #15875): a changed PDF
//! file shown page by page. GHD shows "This binary file has changed."
//! (`lib/git/diff.ts` `getDiffType`, `ui/diff/binary-file.tsx`).
//!
//! Both versions are opened once, off the main thread
//! (`corvene_core::pdf`). The current page pair is drawn to PNGs in the
//! background and shown by an [`ImageDiff`], so the usual modes (2-up,
//! Swipe, Onion Skin, Difference) and image settings apply. Pages pair up by
//! number until [`corvene_core::pdf::match_pages`] has paired them as a
//! line diff pairs lines (documents of up to
//! [`corvene_core::pdf::MAX_SCANNED_PAGES`] pages), so an inserted page shows
//! as added; a page only one version has shows alone. A toolbar above steps
//! through the pairs and jumps to the next changed one; once the pages are
//! matched a view still on the first pair moves to the first changed one.
//! A file that does not open (encrypted, broken) reports
//! [`PdfDiff::failed`] and the diff view shows the binary panel.
//!
//! `1359-page-strip`: a resizable strip of thumbnails beside the page
//! (`page_strip`), one row per pair with its old → new page numbers and
//! its mark; thumbnails are drawn in the background as their rows scroll
//! into view and a bounded number are kept.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use corvene_core::pdf::{PagePair, PageStatus, PdfDocument, PdfError};
use corvene_core::{FileStatusKind, ImageBlob};
use gpui_kit::component::resizable::ResizableState;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::actions::{SelectFirstFile, SelectLastFile, SelectNextFile, SelectPreviousFile};
use crate::format::format_bytes;
use crate::image_diff::ImageDiff;
use crate::page_strip::{self, RowLook};
use crate::scrollbar::ScrollbarExt;
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::{GhdTooltip, IconButtonA11y};

/// The longer side of a drawn page, in pixels: sharp on a 2x display at
/// most window sizes, without drawing again on every resize.
const PAGE_PX: u32 = 2000;
/// Drawn page pairs kept (their image diffs keep their sliders).
const KEPT_PAGES: usize = 6;
/// Strip thumbnails kept.
const KEPT_THUMBS: usize = 240;
/// Thumbnails drawn per background batch.
const THUMB_BATCH: usize = 6;

/// A drawn pair's pages (old, new).
type PairKey = (Option<usize>, Option<usize>);
/// A thumbnail: (new version, page).
type ThumbKey = (bool, usize);

enum Docs {
    Loading,
    Failed(PdfError),
    Ready {
        previous: Option<PdfDocument>,
        current: Option<PdfDocument>,
        /// Page sizes in points of (previous, current).
        sizes: Arc<[Vec<(f32, f32)>; 2]>,
    },
}

/// Where pairing the pages stands.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Matching {
    Running,
    Done,
    /// Too many pages: pairs stay by number, not compared.
    Skipped,
}

pub struct PdfDiff {
    status: FileStatusKind,
    /// The file sizes of (previous, current), for the footers.
    bytes: (Option<usize>, Option<usize>),
    docs: Docs,
    /// The page pairs, in order.
    pairs: Vec<PagePair>,
    /// The shown pair (index into `pairs`).
    row: usize,
    /// The user picked a page.
    moved: bool,
    matching: Matching,
    /// Drawn page pairs, most recent last.
    drawn: VecDeque<(PairKey, Entity<ImageDiff>)>,
    /// The pair being drawn.
    drawing: Option<PairKey>,
    strip_focus: FocusHandle,
    strip_scroll: UniformListScrollHandle,
    strip_split: Entity<ResizableState>,
    /// The row last scrolled into view.
    scrolled_to: Option<usize>,
    /// Drawn thumbnails (`None`: could not be drawn), oldest first in
    /// `thumb_order`.
    thumbs: HashMap<ThumbKey, Option<Arc<RenderImage>>>,
    thumb_order: VecDeque<ThumbKey>,
    drawing_thumbs: bool,
    _thumb_task: Option<Task<()>>,
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
            let (previous, current) = (open(old)?, open(new)?);
            let sizes = |doc: &Option<PdfDocument>| {
                doc.as_ref().map_or_else(Vec::new, |d| {
                    (0..d.page_count())
                        .map(|i| d.page_size(i).unwrap_or((612., 792.)))
                        .collect()
                })
            };
            let sizes = Arc::new([sizes(&previous), sizes(&current)]);
            Ok::<_, PdfError>((previous, current, sizes))
        });
        let task = cx.spawn(async move |this, cx| {
            let docs = open.await;
            this.update(cx, |this, cx| {
                this.docs = match docs {
                    Ok((previous, current, sizes)) => Docs::Ready {
                        previous,
                        current,
                        sizes,
                    },
                    Err(e) => Docs::Failed(e),
                };
                let (old, new) = this.page_counts();
                this.pairs = corvene_core::pdf::pairs_by_number(old, new);
                this.match_pages(cx);
                cx.notify();
            })
            .ok();
        });
        Self {
            status,
            bytes,
            docs: Docs::Loading,
            pairs: Vec::new(),
            row: 0,
            moved: false,
            matching: Matching::Running,
            drawn: VecDeque::new(),
            drawing: None,
            strip_focus: cx.focus_handle(),
            strip_scroll: UniformListScrollHandle::new(),
            strip_split: page_strip::split_state(cx),
            scrolled_to: None,
            thumbs: HashMap::new(),
            thumb_order: VecDeque::new(),
            drawing_thumbs: false,
            _thumb_task: None,
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

    /// The page counts of (previous, current).
    fn page_counts(&self) -> (usize, usize) {
        match &self.docs {
            Docs::Ready {
                previous, current, ..
            } => {
                let count = |d: &Option<PdfDocument>| d.as_ref().map_or(0, PdfDocument::page_count);
                (count(previous), count(current))
            }
            _ => (0, 0),
        }
    }

    /// Pairs the pages in the background.
    fn match_pages(&mut self, cx: &mut Context<Self>) {
        let Docs::Ready {
            previous, current, ..
        } = &self.docs
        else {
            return;
        };
        // one side only: every page is added or deleted
        if previous.is_none() || current.is_none() {
            self.matching = Matching::Done;
            return;
        }
        let (previous, current) = (previous.clone(), current.clone());
        let scan = cx.background_executor().spawn(async move {
            corvene_core::pdf::match_pages(previous.as_ref(), current.as_ref())
        });
        self._tasks.push(cx.spawn(async move |this, cx| {
            let pairs = scan.await;
            this.update(cx, |this, cx| {
                match pairs {
                    Some(pairs) => {
                        let shown = this.pairs.get(this.row).copied();
                        this.row = if this.moved {
                            // keep the shown page
                            shown
                                .and_then(|s| {
                                    pairs.iter().position(|p| {
                                        (s.current.is_some() && p.current == s.current)
                                            || (s.current.is_none() && p.previous == s.previous)
                                    })
                                })
                                .unwrap_or(0)
                        } else {
                            // open at the first changed page
                            pairs.iter().position(PagePair::is_change).unwrap_or(0)
                        };
                        this.pairs = pairs;
                        this.matching = Matching::Done;
                    }
                    None => this.matching = Matching::Skipped,
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn go_to(&mut self, row: usize, cx: &mut Context<Self>) {
        let row = row.min(self.pairs.len().saturating_sub(1));
        self.moved = true;
        if row != self.row {
            self.row = row;
            cx.notify();
        }
    }

    fn next_changed(&self) -> Option<usize> {
        let changed: Vec<bool> = self.pairs.iter().map(PagePair::is_change).collect();
        next_changed(self.row, &changed)
    }

    /// The image diff of the shown pair, drawing it when needed.
    fn page_diff(&mut self, cx: &mut Context<Self>) -> Option<Entity<ImageDiff>> {
        let pair = self.pairs.get(self.row)?;
        let key = (pair.previous, pair.current);
        if let Some(ix) = self.drawn.iter().position(|(k, _)| *k == key) {
            let entry = self.drawn.remove(ix)?;
            let diff = entry.1.clone();
            self.drawn.push_back(entry);
            return Some(diff);
        }
        if self.drawing == Some(key) {
            return None;
        }
        let Docs::Ready {
            previous, current, ..
        } = &self.docs
        else {
            return None;
        };
        self.drawing = Some(key);
        let (previous, current) = (previous.clone(), current.clone());
        let draw = cx.background_executor().spawn(async move {
            let draw = |doc: &Option<PdfDocument>, page: Option<usize>| {
                let (doc, page) = (doc.as_ref()?, page?);
                let size = doc.page_size(page)?;
                let image = doc.render(page, PAGE_PX)?;
                Some((image, size))
            };
            (draw(&previous, key.0), draw(&current, key.1))
        });
        self._tasks.push(cx.spawn(async move |this, cx| {
            let (previous, current) = draw.await;
            this.update(cx, |this, cx| {
                if this.drawing == Some(key) {
                    this.drawing = None;
                }
                let blob = |side: &Option<(corvene_core::pdf::PageImage, (f32, f32))>| {
                    side.as_ref().map(|(image, _)| ImageBlob {
                        bytes: image.png.clone(),
                        media_type: "image/png".to_string(),
                        original: None,
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
                this.drawn.push_back((key, diff));
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
        let rows = self.pairs.len();
        let (old_pages, new_pages) = self.page_counts();
        let counts = match &self.docs {
            Docs::Ready {
                previous: Some(_),
                current: Some(_),
                ..
            } if old_pages != new_pages => Some(format!("{old_pages} → {new_pages} pages")),
            _ => None,
        };
        let summary = match self.matching {
            Matching::Done => match self.pairs.iter().filter(|p| p.is_change()).count() {
                0 => "No visible changes".to_string(),
                1 => "1 page changed".to_string(),
                n => format!("{n} pages changed"),
            },
            Matching::Skipped => String::new(),
            Matching::Running => "Comparing pages…".to_string(),
        };
        let pair = self.pairs.get(self.row).copied();
        let page_changed = pair
            .filter(|p| p.status != PageStatus::Unknown)
            .map(|p| p.is_change());
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
        let row = self.row;
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
                button("pdf-diff-previous", "Previous", row > 0).when(row > 0, |d| {
                    d.on_click(cx.listener(move |this, _, _, cx| this.go_to(row - 1, cx)))
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
                    .child(page_title(pair.as_ref(), old_pages.max(new_pages)))
                    .when(page_changed == Some(false), |d| {
                        d.ghd_tooltip("This page looks the same in both versions")
                    }),
            )
            .child(
                button("pdf-diff-next", "Next", row + 1 < rows).when(row + 1 < rows, |d| {
                    d.on_click(cx.listener(move |this, _, _, cx| this.go_to(row + 1, cx)))
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

    /// `1359-page-strip`: the rows the strip lists.
    fn strip_rows(&self, cx: &App) -> Vec<usize> {
        page_strip::visible_rows(
            self.pairs.iter().map(|p| p.status),
            page_strip::changed_only(cx),
        )
    }

    /// `1359-page-strip`: steps the shown page through the strip's rows.
    fn strip_step(&mut self, step: isize, cx: &mut Context<Self>) {
        let rows = self.strip_rows(cx);
        if let Some(row) = page_strip::step_row(&rows, self.row, step) {
            self.go_to(row, cx);
        }
    }

    fn strip_edge(&mut self, last: bool, cx: &mut Context<Self>) {
        let rows = self.strip_rows(cx);
        if let Some(&row) = if last { rows.last() } else { rows.first() } {
            self.go_to(row, cx);
        }
    }

    /// The thumbnail a row shows: its new page, or its old one when removed.
    fn thumb_key(pair: &PagePair) -> Option<ThumbKey> {
        match (pair.previous, pair.current) {
            (_, Some(page)) => Some((true, page)),
            (Some(page), None) => Some((false, page)),
            _ => None,
        }
    }

    /// Draws the missing thumbnails among `keys` in the background, a
    /// batch at a time.
    fn request_thumbs(&mut self, keys: Vec<ThumbKey>, cx: &mut Context<Self>) {
        if self.drawing_thumbs {
            return;
        }
        let Docs::Ready {
            previous, current, ..
        } = &self.docs
        else {
            return;
        };
        let wanted: Vec<ThumbKey> = keys
            .into_iter()
            .filter(|k| !self.thumbs.contains_key(k))
            .take(THUMB_BATCH)
            .collect();
        if wanted.is_empty() {
            return;
        }
        self.drawing_thumbs = true;
        let (previous, current) = (previous.clone(), current.clone());
        let draw = cx.background_executor().spawn(async move {
            wanted
                .into_iter()
                .map(|key| {
                    let doc = if key.0 { &current } else { &previous };
                    let image = doc
                        .as_ref()
                        .and_then(|d| d.render_pixels(key.1, page_strip::THUMB_PX))
                        .and_then(|(w, h, rgba)| page_strip::render_image(w, h, rgba));
                    (key, image)
                })
                .collect::<Vec<_>>()
        });
        self._thumb_task = Some(cx.spawn(async move |this, cx| {
            let drawn = draw.await;
            this.update(cx, |this, cx| {
                this.drawing_thumbs = false;
                for (key, image) in drawn {
                    if this.thumbs.insert(key, image).is_none() {
                        this.thumb_order.push_back(key);
                    }
                }
                while this.thumb_order.len() > KEPT_THUMBS {
                    if let Some(old) = this.thumb_order.pop_front() {
                        this.thumbs.remove(&old);
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// `1359-page-strip`: the thumbnail strip.
    fn strip(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.ghd().clone();
        let rows = Arc::new(self.strip_rows(cx));
        if self.scrolled_to != Some(self.row) {
            self.scrolled_to = Some(self.row);
            if let Some(ix) = rows.iter().position(|&r| r == self.row) {
                self.strip_scroll
                    .scroll_to_item(ix, ScrollStrategy::Nearest);
            }
        }
        let focused = self.strip_focus.contains_focused(window, cx);
        let list = if rows.is_empty() {
            page_strip::empty_note("No pages changed.", &t)
        } else {
            let count = rows.len();
            uniform_list(
                "pdf-page-strip-rows",
                count,
                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                    let t = cx.ghd().clone();
                    let keys: Vec<ThumbKey> = range
                        .clone()
                        .filter_map(|ix| Self::thumb_key(this.pairs.get(rows[ix])?))
                        .collect();
                    this.request_thumbs(keys, cx);
                    let sizes = match &this.docs {
                        Docs::Ready { sizes, .. } => Some(sizes.clone()),
                        _ => None,
                    };
                    range
                        .map(|ix| {
                            let row = rows[ix];
                            let pair = this.pairs[row];
                            let key = Self::thumb_key(&pair);
                            let thumb = key.and_then(|k| this.thumbs.get(&k).cloned().flatten());
                            let aspect = key
                                .and_then(|(new, page)| {
                                    sizes.as_ref()?[usize::from(new)].get(page).copied()
                                })
                                .map_or(612. / 792., |(w, h)| w / h.max(1.));
                            page_strip::thumb_row(
                                ("pdf-page-strip-row", row).into(),
                                &pair,
                                RowLook {
                                    selected: row == this.row,
                                    focused,
                                },
                                thumb,
                                aspect,
                                &t,
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                window.focus(&this.strip_focus, cx);
                                this.go_to(row, cx);
                            }))
                            .into_any_element()
                        })
                        .collect()
                }),
            )
            .flex_1()
            .min_h_0()
            .with_scrollbar_handle(&self.strip_scroll)
            .into_any_element()
        };
        page_strip::frame("pdf-page-strip", "Pages", &self.strip_focus, list, cx)
            .on_action(cx.listener(|this, _: &SelectNextFile, _, cx| this.strip_step(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPreviousFile, _, cx| this.strip_step(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectFirstFile, _, cx| this.strip_edge(false, cx)))
            .on_action(cx.listener(|this, _: &SelectLastFile, _, cx| this.strip_edge(true, cx)))
            .into_any_element()
    }
}

/// The toolbar's name of the shown pair.
fn page_title(pair: Option<&PagePair>, pages: usize) -> String {
    match pair.map(|p| (p.previous, p.current)) {
        Some((Some(a), Some(b))) if a == b => format!("Page {} of {}", a + 1, pages.max(1)),
        Some((Some(a), Some(b))) => format!("Page {} → {}", a + 1, b + 1),
        Some((None, Some(b))) => format!("Page {} (added)", b + 1),
        Some((Some(a), None)) => format!("Page {} (removed)", a + 1),
        _ => format!("Page 1 of {}", pages.max(1)),
    }
}

/// The first changed row after `row` (wrapping around), not `row`.
fn next_changed(row: usize, changed: &[bool]) -> Option<usize> {
    let n = changed.len();
    (1..=n)
        .map(|k| (row + k) % n)
        .find(|&i| changed[i] && i != row)
}

impl Render for PdfDiff {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                let main = div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .min_h_0()
                    .min_w_0()
                    .child(self.toolbar(cx))
                    .child(div().flex_1().min_h_0().flex().child(page))
                    .into_any_element();
                if page_strip::enabled(cx) && self.pairs.len() > 1 {
                    let strip = self.strip(window, cx);
                    page_strip::split("pdf-page-split", &self.strip_split, strip, main, cx)
                } else {
                    main
                }
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
    use super::{next_changed, page_title};
    use corvene_core::pdf::{PagePair, PageStatus};

    #[test]
    fn next_change_wraps_and_skips_the_shown_page() {
        assert_eq!(next_changed(0, &[true, false, true]), Some(2));
        assert_eq!(next_changed(2, &[true, false, true]), Some(0));
        assert_eq!(next_changed(1, &[false, true, false]), None);
        assert_eq!(next_changed(0, &[]), None);
    }

    #[test]
    fn titles_name_both_page_numbers() {
        let pair = |previous, current| PagePair {
            previous,
            current,
            status: PageStatus::Changed,
        };
        assert_eq!(page_title(Some(&pair(Some(1), Some(1))), 5), "Page 2 of 5");
        assert_eq!(page_title(Some(&pair(Some(2), Some(3))), 5), "Page 3 → 4");
        assert_eq!(page_title(Some(&pair(None, Some(2))), 5), "Page 3 (added)");
        assert_eq!(
            page_title(Some(&pair(Some(4), None)), 5),
            "Page 5 (removed)"
        );
    }
}
