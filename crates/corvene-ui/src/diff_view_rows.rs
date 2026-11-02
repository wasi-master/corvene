//! Row rendering for the unified diff (split out of `diff_view.rs`): pure
//! functions over a snapshot of the view state, so `gpui::list` can render
//! only the visible rows. Rows are built from the (possibly expanded) hunks
//! of `diff_expansion`; `Row::original` keeps the model's line index so
//! selections and discard patches ignore expanded context.
//!
//! Deviation (`1304-diff-no-wrap`): with View › Wrap Diff Lines off a row's
//! text stays on one line, clipped beside the gutters and moved by the
//! shared horizontal offset in [`HScroll`] (GHD always wraps:
//! `_side-by-side-diff.scss` `white-space: pre-wrap`).

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::rc::Rc;

use corvene_core::diff_line_class::LineMark;
use corvene_core::{DiffLineKind, DiffSelection, DiffSelectionType, Dispatcher};
use corvene_highlight::{Span, TokenClass};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::widgets::GhdTooltip;
use crate::widgets::IconButtonA11y;

use crate::diff_expansion::{ExpansionKind, HunkExpansionType, XHunk};
use crate::diff_view::{DIFF_LINE_HEIGHT, DiffView, TextSelectionSnapshot};
use crate::icons::{Octicon, octicon};
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, GhdTheme};

/// `--hunk-handle-width-with-check-all`
pub const HANDLE_WIDTH: f32 = 16.;
/// `.line-number-check`
pub const CHECK_WIDTH: f32 = 20.;

/// GHD `getLineWidthFromDigitCount(getNumberOfDigits(diff.maxLineNumber))`
/// (`diff-helpers.tsx`): one line-number column, sized for the largest line
/// number in the diff. `--width-line-number: 55px` is only the CSS default;
/// the row overrides it with this inline width.
pub fn line_number_width(max_line_number: u32) -> f32 {
    let digits = max_line_number.checked_ilog10().map_or(1, |d| d + 1);
    digits.max(3) as f32 * 10. + 5.
}

/// The largest old/new line number among `rows` (GHD `diff.maxLineNumber`).
/// Both sides only grow down the diff, so the last numbered rows hold them.
pub fn max_line_number(rows: &[Row]) -> u32 {
    let (mut old, mut new) = (None, None);
    for row in rows.iter().rev() {
        old = old.or(row.old);
        new = new.or(row.new);
        if old.is_some() && new.is_some() {
            break;
        }
    }
    old.unwrap_or(0).max(new.unwrap_or(0))
}

/// GHD `DiffRangeType`: what a block of consecutive changes contains.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeType {
    Additions,
    Deletions,
    Mixed,
}

impl RangeType {
    /// GHD `getDiscardLabel`: "Discard Added Line…", "Discard Modified Lines".
    pub fn discard_label(self, lines: u32, confirm: bool) -> String {
        use crate::context_menu::{IS_MAC, mac_or};
        let kind = match self {
            RangeType::Additions => mac_or("Added", "added"),
            RangeType::Deletions => mac_or("Removed", "removed"),
            RangeType::Mixed => mac_or("Modified", "modified"),
        };
        let plural = if lines > 1 { "s" } else { "" };
        let suffix = if confirm { "…" } else { "" };
        if IS_MAC {
            format!("Discard {kind} Line{plural}{suffix}")
        } else {
            format!("Discard {kind} line{plural}{suffix}")
        }
    }

    /// `1303-partial-stash`: "Stash Added Line", "Stash Modified Lines"…,
    /// worded like [`Self::discard_label`].
    pub fn stash_label(self, lines: u32) -> String {
        use crate::context_menu::{IS_MAC, mac_or};
        let kind = match self {
            RangeType::Additions => mac_or("Added", "added"),
            RangeType::Deletions => mac_or("Removed", "removed"),
            RangeType::Mixed => mac_or("Modified", "modified"),
        };
        let plural = if lines > 1 { "s" } else { "" };
        if IS_MAC {
            format!("Stash {kind} Line{plural}")
        } else {
            format!("Stash {kind} line{plural}")
        }
    }
}

/// One unified-diff row: its index in the expanded diff, its index in the
/// original diff (changed lines and original context only), and, for changed
/// lines, the block of consecutive changes it belongs to (`hunkStartLine`,
/// length, kind).
pub struct Row {
    pub abs: u32,
    pub original: Option<u32>,
    pub kind: DiffLineKind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
    /// Byte offsets in `text` where an expanded tab starts.
    pub tabs: Vec<u32>,
    pub no_newline: bool,
    pub group: Option<(u32, u32)>,
    pub group_type: Option<RangeType>,
    pub hunk_index: usize,
    /// Hunk header rows: which expansion handles to show.
    pub expansion: HunkExpansionType,
}

impl Row {
    pub fn height(&self) -> Pixels {
        match (self.kind, self.expansion) {
            (DiffLineKind::Hunk, HunkExpansionType::Both) => DIFF_LINE_HEIGHT() * 2.,
            // `.hunk-expansion-handle button`: 16 px icon + 4 / 1 px padding +
            // 1 px margins grows the row to 23 px (measured in GHD 3.6.6);
            // a hunk without a handle stays 20 px
            (DiffLineKind::Hunk, HunkExpansionType::Up)
            | (DiffLineKind::Hunk, HunkExpansionType::Down)
            | (DiffLineKind::Hunk, HunkExpansionType::Short) => zpx(23.),
            _ => DIFF_LINE_HEIGHT(),
        }
    }

    /// `760-discard-from-text-menu`: a changed row's selection index, its
    /// block of changes and what the block contains.
    pub fn discard_target(&self) -> Option<(u32, (u32, u32), RangeType)> {
        if !matches!(self.kind, DiffLineKind::Add | DiffLineKind::Delete) {
            return None;
        }
        Some((
            self.original.unwrap_or(self.abs),
            self.group?,
            self.group_type?,
        ))
    }
}

/// GHD `temporarySelection`: a drag in progress over the line numbers
/// (original line indices).
#[derive(Clone, Copy, Debug)]
pub struct TempSelection {
    pub from: u32,
    pub to: u32,
    pub selected: bool,
}

/// One ⌘F hit: row index + byte range in the row text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
    pub row: usize,
    pub range: Range<usize>,
}

/// Search hits grouped per row for rendering (`bool` = the selected hit).
pub struct SearchIndex {
    pub by_row: HashMap<usize, Vec<(Range<usize>, bool)>>,
}

/// Everything a row needs from the view, snapshotted per render.
pub struct RowContext {
    pub repo: u64,
    pub path: String,
    pub selection: DiffSelection,
    pub groups: BTreeMap<u32, DiffSelectionType>,
    pub selectable: bool,
    /// Diff Settings › Hide Whitespace Changes: line interaction shows a hint instead.
    pub hide_whitespace: bool,
    pub temp: Option<TempSelection>,
    pub hovered_group: Option<u32>,
    pub view: WeakEntity<DiffView>,
    /// Syntax spans per row (same indexing as the rows), once highlighted.
    pub tokens: Option<Rc<Vec<Vec<Span>>>>,
    /// Intra-line change ranges per unified row (`unified_inner`).
    pub inner: Rc<Vec<Vec<Range<usize>>>>,
    /// `1316-diff-moved-lines` / `1317-diff-stylistic-changes`: moved and
    /// stylistic marks per unified row (empty when both are off).
    pub marks: Rc<Vec<Option<LineMark>>>,
    pub search: Option<Rc<SearchIndex>>,
    /// Settings › Accessibility › Show check marks in the diff.
    pub show_check_marks: bool,
    /// `lineNumberWidth`: one line-number column (`line_number_width`).
    pub line_number_width: f32,
    /// The text selection (ordered), if any.
    pub text_selection: Option<TextSelectionSnapshot>,
    pub text_bounds: TextBounds,
    /// `748-diff-show-whitespace`: marks spaces and tabs in the text.
    pub show_whitespace: bool,
    /// `759-wide-hunk-handle`: the old-number column toggles the group.
    pub wide_hunk_handle: bool,
    /// `348-pull-request-review`: the review threads under the rows and
    /// the `+` that starts a comment (a pull request's diff only).
    pub review: Option<Rc<crate::review_threads::ReviewUi>>,
    /// `1304-diff-no-wrap`: lines do not wrap and scroll sideways together.
    pub h_scroll: Option<HScroll>,
}

/// `1304-diff-no-wrap`: the horizontal scroll of a diff whose lines do not
/// wrap, shared by every row's text (gutters stay put) and by the
/// horizontal scrollbar, which drives it through [`ScrollbarHandle`]. The
/// rows report where their text is clipped and how wide their lines are
/// as they paint.
#[derive(Clone, Default)]
pub struct HScroll(Rc<HScrollState>);

#[derive(Default)]
struct HScrollState {
    /// How far the text is moved left (≥ 0).
    offset: Cell<Pixels>,
    /// The widest line the rows are known to have: an estimate from the
    /// character count, raised by what painted rows measure.
    widest: Cell<Pixels>,
    /// One monospace column, for the estimate and for revealing a column.
    column: Cell<Pixels>,
    /// Where the scrollbar goes: from the left of the leftmost text clip to
    /// the right of the rightmost, over the list's height.
    span: Cell<Bounds<Pixels>>,
    /// The narrowest text clip (one column's in split mode).
    clip_width: Cell<Pixels>,
    /// The frame `span` was recorded in (the first row of a frame resets it).
    recorded: Cell<usize>,
    frame: Cell<usize>,
}

impl HScroll {
    pub fn offset(&self) -> Pixels {
        self.0.offset.get()
    }

    /// How far the text can move: the widest line past the clip.
    pub fn max_offset(&self) -> Pixels {
        (self.0.widest.get() - self.0.clip_width.get()).max(Pixels::ZERO)
    }

    pub fn set(&self, offset: Pixels) {
        self.0
            .offset
            .set(offset.clamp(Pixels::ZERO, self.max_offset()));
    }

    /// Move by `delta` (positive: right, towards the ends of lines); whether
    /// the offset changed.
    pub fn scroll_by(&self, delta: Pixels) -> bool {
        let before = self.offset();
        self.set(before + delta);
        self.offset() != before
    }

    /// A new frame over a list at `list`: the rows record their clips
    /// again; the widest line is at least `estimate`, and `column` is one
    /// character's advance.
    pub fn begin_frame(&self, estimate: Pixels, column: Pixels, list: Bounds<Pixels>) {
        let s = &self.0;
        s.frame.set(s.frame.get() + 1);
        s.widest.set(s.widest.get().max(estimate));
        s.column.set(column);
        let mut span = s.span.get();
        if span.size.width <= Pixels::ZERO {
            // until a row paints, the list's own bounds
            span = list;
            s.clip_width.set(list.size.width);
        }
        span.origin.y = list.origin.y;
        span.size.height = list.size.height;
        s.span.set(span);
    }

    /// The rows changed: forget the measured widths.
    pub fn reset_width(&self, estimate: Pixels) {
        self.0.widest.set(estimate);
        self.set(self.offset());
    }

    /// The least width of a row's line box: past the right of the clip, so
    /// a press after the end of a short line still lands on its text.
    fn min_line_width(&self) -> Pixels {
        self.0.clip_width.get() + self.offset()
    }

    /// Back to the start of the lines.
    pub fn reset(&self) {
        self.0.offset.set(Pixels::ZERO);
    }

    /// A row's text clip area painted at `bounds`.
    pub(crate) fn record_clip(&self, bounds: Bounds<Pixels>) {
        let s = &self.0;
        let mut span = s.span.get();
        if s.recorded.get() != s.frame.get() {
            s.recorded.set(s.frame.get());
            span.origin.x = bounds.origin.x;
            span.size.width = bounds.size.width;
            s.clip_width.set(bounds.size.width);
        } else {
            let right = span.right().max(bounds.right());
            span.origin.x = span.origin.x.min(bounds.origin.x);
            span.size.width = right - span.origin.x;
            s.clip_width.set(s.clip_width.get().min(bounds.size.width));
        }
        s.span.set(span);
    }

    /// A painted line box is `width` wide (at least
    /// [`Self::min_line_width`], which never moves the end further).
    fn record_line(&self, width: Pixels) {
        if width > self.0.widest.get() {
            self.0.widest.set(width);
        }
    }

    /// Scroll so that columns `start..end` of a line's text (after its
    /// five-column prefix) are visible, with five columns of context.
    pub fn reveal_columns(&self, start: usize, end: usize) {
        let column = self.0.column.get();
        let width = self.0.clip_width.get();
        let left = column * start as f32;
        let right = column * (end + 10) as f32;
        let offset = self.offset();
        if left < offset {
            self.set(left);
        } else if right > offset + width {
            self.set(right - width);
        }
    }
}

impl gpui_kit::base::ScrollbarHandle for HScroll {
    fn viewport_bounds(&self) -> Bounds<Pixels> {
        self.0.span.get()
    }

    fn offset(&self) -> Point<Pixels> {
        point(-self.offset(), Pixels::ZERO)
    }

    fn set_offset(&self, offset: Point<Pixels>) {
        self.set(-offset.x);
    }

    fn content_size(&self) -> Size<Pixels> {
        let span = self.0.span.get().size;
        size(span.width + self.max_offset(), span.height)
    }
}

/// The text part of a row (`parts`: prefix, text, "No newline" note) in
/// `area`: laid out in the row's width, wrapping, or with
/// `1304-diff-no-wrap` on one line, clipped and moved left by the
/// horizontal scroll.
fn row_text_area(
    ctx: &RowContext,
    area: Stateful<Div>,
    parts: impl IntoIterator<Item = AnyElement>,
) -> Stateful<Div> {
    let Some(h) = ctx.h_scroll.clone() else {
        return area.flex().flex_row().children(parts);
    };
    let (clip, line) = (h.clone(), h.clone());
    area.relative()
        .overflow_hidden()
        .child(
            canvas(move |b, _, _| clip.record_clip(b), |_, _, _, _| {})
                .absolute()
                .inset_0(),
        )
        .child(
            div()
                .relative()
                .flex_none()
                .flex()
                .flex_row()
                .whitespace_nowrap()
                .ml(-h.offset())
                .min_w(h.min_line_width())
                .child(
                    canvas(
                        move |b, _, _| line.record_line(b.size.width),
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                )
                .children(parts),
        )
}

/// `348-pull-request-review`: the side and line a row's number stands for
/// (old numbers are the LEFT side).
fn review_line(
    kind: DiffLineKind,
    old: Option<u32>,
    new: Option<u32>,
) -> Option<(corvene_core::pull_request_review::DiffSide, u32)> {
    use corvene_core::pull_request_review::DiffSide;
    match kind {
        DiffLineKind::Delete => old.map(|n| (DiffSide::Left, n)),
        DiffLineKind::Add | DiffLineKind::Context => new.map(|n| (DiffSide::Right, n)),
        DiffLineKind::Hunk => None,
    }
}

/// The hover group of a row (unified) or of one side of a split row.
fn review_group(abs: u32, column: Option<Column>) -> SharedString {
    match column {
        None => SharedString::from(format!("review-row-{abs}")),
        Some(Column::Before) => SharedString::from(format!("review-before-{abs}")),
        Some(Column::After) => SharedString::from(format!("review-after-{abs}")),
    }
}

/// The `+` of a line, when the review can take a comment.
fn review_add_button(
    ctx: &RowContext,
    kind: DiffLineKind,
    old: Option<u32>,
    new: Option<u32>,
    group: SharedString,
    cx: &App,
) -> Option<AnyElement> {
    let review = ctx.review.as_ref().filter(|r| r.can_write)?;
    let (side, line) = review_line(kind, old, new)?;
    Some(crate::review_threads::add_comment_button(
        ctx.repo,
        &ctx.path,
        side,
        line,
        review.composer.as_ref().map(|(t, _, _)| t),
        group,
        cx,
    ))
}

/// Whether the row's line is inside the composer's range.
fn review_in_range(
    ctx: &RowContext,
    kind: DiffLineKind,
    old: Option<u32>,
    new: Option<u32>,
) -> bool {
    match (&ctx.review, review_line(kind, old, new)) {
        (Some(review), Some((side, line))) => review.in_composer_range(side, Some(line)),
        _ => false,
    }
}

impl RowContext {
    /// The moved / stylistic mark of a changed unified row.
    pub fn mark(&self, unified: usize, kind: DiffLineKind) -> Option<LineMark> {
        if !matches!(kind, DiffLineKind::Add | DiffLineKind::Delete) {
            return None;
        }
        self.marks.get(unified).copied().flatten()
    }

    /// The selected byte range of the text at (`list_ix`, `column`).
    fn selection_range(&self, list_ix: usize, column: Column, len: usize) -> Option<Range<usize>> {
        let sel = self.text_selection.as_ref()?;
        if sel.column != column || list_ix < sel.start.row || list_ix > sel.end.row {
            return None;
        }
        let start = if list_ix == sel.start.row {
            sel.start.col.min(len)
        } else {
            0
        };
        let end = if list_ix == sel.end.row {
            sel.end.col.min(len)
        } else {
            len
        };
        (start < end).then_some(start..end)
    }
}

/// `741-diff-open-in-editor-at-line`: ⌥-click on a line's text opens the
/// editor at `line` instead of starting a text selection.
fn open_at_line_on_alt_click(
    view: &WeakEntity<DiffView>,
    ev: &MouseDownEvent,
    line: Option<u32>,
    cx: &mut App,
) {
    let m = ev.modifiers;
    if ev.button != MouseButton::Left || !m.alt || m.shift || m.control || m.platform {
        return;
    }
    if view
        .update(cx, |this, cx| this.open_at_line(line, cx))
        .unwrap_or(false)
    {
        cx.stop_propagation();
    }
}

/// The selectable text of a row: records its bounds for hit-testing, starts
/// a text selection on mouse down (shift extends) and paints the selection.
/// `inner` is the intra-line change background (one range, or several with
/// `791-word-intra-line-diff`), drawn behind the text over
/// the font's content area like the inline `.cm-diff-add-inner` span in GHD
/// (a `HighlightStyle` background would fill the whole 20 px line).
fn selectable_text(
    ctx: &RowContext,
    list_ix: usize,
    column: Column,
    row: &Row,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    inner: Vec<(Range<usize>, Hsla)>,
) -> Div {
    let text = &row.text;
    let bounds = ctx.text_bounds.clone();
    let view = ctx.view.clone();
    let whitespace = (ctx.show_whitespace && text.contains(' ')).then(|| row.tabs.clone());
    let styled = StyledText::new(SharedString::from(text.to_string())).with_highlights(highlights);
    let layout = styled.layout().clone();
    let hit_layout = layout.clone();
    // `1304-diff-no-wrap`: at least as wide as the line
    let no_wrap = ctx.h_scroll.is_some();
    div()
        .when(!no_wrap, |d| d.flex_1().min_w_0())
        .when(no_wrap, |d| d.flex_grow_1().flex_shrink_0())
        .relative()
        .cursor_text()
        .child(
            canvas(
                move |b, _, _| {
                    bounds.borrow_mut().insert(
                        (list_ix, column),
                        RowText {
                            bounds: b,
                            layout: hit_layout,
                        },
                    );
                },
                move |_, _, window, cx| {
                    for (range, color) in &inner {
                        paint_inline_background(&layout, range.clone(), *color, window);
                    }
                    if let Some(tabs) = &whitespace {
                        let color = cx.ghd().text_secondary.opacity(0.6);
                        paint_whitespace(&layout, tabs, color, window);
                    }
                },
            )
            .absolute()
            .inset_0(),
        )
        .child(styled)
        .on_mouse_down(MouseButton::Left, move |ev, _, cx| {
            view.update(cx, |this, cx| {
                this.start_text_selection(list_ix, column, ev.position, ev.modifiers.shift, cx)
            })
            .ok();
        })
}

/// `.cm-s-default` colours; classes that inherit are not emitted by the highlighter.
pub fn token_color(class: TokenClass, t: &GhdTheme) -> Hsla {
    match class {
        TokenClass::Variable => t.syntax_variable,
        TokenClass::AltVariable => t.syntax_alt_variable,
        TokenClass::Keyword => t.syntax_keyword,
        TokenClass::Atom => t.syntax_atom,
        TokenClass::String => t.syntax_string,
        TokenClass::Qualifier => t.syntax_qualifier,
        TokenClass::Type => t.syntax_type,
        TokenClass::Comment => t.syntax_comment,
        TokenClass::Tag => t.syntax_tag,
        TokenClass::Attribute => t.syntax_attribute,
        TokenClass::Link => t.syntax_link,
        TokenClass::Header => t.syntax_header,
        TokenClass::Quote => t.syntax_quote,
    }
}

pub fn build_rows(hunks: &[XHunk]) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    for (hunk_index, hunk) in hunks.iter().enumerate() {
        let mut block_start: Option<usize> = None;
        for (i, xl) in hunk.lines.iter().enumerate() {
            let line = &xl.line;
            let changed = matches!(line.kind, DiffLineKind::Add | DiffLineKind::Delete);
            rows.push(Row {
                abs: hunk.unified_diff_start + i as u32,
                original: xl.original,
                kind: line.kind,
                old: line.old_line,
                new: line.new_line,
                text: expand_tabs(&line.text),
                tabs: tab_offsets(&line.text),
                no_newline: line.no_trailing_newline,
                group: None,
                group_type: None,
                hunk_index,
                expansion: if line.kind == DiffLineKind::Hunk {
                    hunk.expansion
                } else {
                    HunkExpansionType::None
                },
            });
            if changed {
                block_start.get_or_insert(rows.len() - 1);
            } else if let Some(start) = block_start.take() {
                let end = rows.len() - 1;
                close_block(&mut rows, start, end);
            }
        }
        if let Some(start) = block_start {
            let end = rows.len();
            close_block(&mut rows, start, end);
        }
    }
    rows
}

fn close_block(rows: &mut [Row], start: usize, end: usize) {
    let Some(first) = rows[start].original else {
        return;
    };
    let len = (end - start) as u32;
    let mut adds = false;
    let mut dels = false;
    for row in &rows[start..end] {
        match row.kind {
            DiffLineKind::Add => adds = true,
            DiffLineKind::Delete => dels = true,
            _ => {}
        }
    }
    let kind = match (adds, dels) {
        (true, false) => RangeType::Additions,
        (false, true) => RangeType::Deletions,
        _ => RangeType::Mixed,
    };
    for row in &mut rows[start..end] {
        row.group = Some((first, len));
        row.group_type = Some(kind);
    }
}

/// What a tab becomes in a row's text. GHD keeps the tab and lets CSS
/// `tab-size` (Settings › Appearance › Tab Size, applied on
/// `#desktop-app-chrome` in `app/src/ui/app.tsx`) advance to the next tab
/// stop; Corvene draws a fixed four spaces per tab, whatever the column or
/// the setting (`.docs/deviations.md` › Diff viewer).
const TAB_EXPANSION: &str = "    ";

/// A diff line as a row shows it: tabs expanded to [`TAB_EXPANSION`].
pub fn expand_tabs(line: &str) -> String {
    line.replace('\t', TAB_EXPANSION)
}

/// Where each tab of `line` starts in its [`expand_tabs`] text.
pub fn tab_offsets(line: &str) -> Vec<u32> {
    let shift = TAB_EXPANSION.len() - 1;
    line.match_indices('\t')
        .enumerate()
        .map(|(n, (ix, _))| (ix + n * shift) as u32)
        .collect()
}

/// Moves syntax spans computed on a raw file line (tabs intact) onto the
/// row's [`expand_tabs`] text: every tab before an offset pushes it right
/// by `TAB_EXPANSION.len() - 1` bytes. Spans that still do not fit the row
/// (the file line and the diff line disagree) are dropped.
pub fn spans_for_row(spans: Vec<Span>, raw: &str, text: &str) -> Vec<Span> {
    let shift = TAB_EXPANSION.len() - 1;
    let expand = |offset: usize| {
        let tabs = raw
            .as_bytes()
            .get(..offset)
            .map_or(0, |b| b.iter().filter(|c| **c == b'\t').count());
        offset + tabs * shift
    };
    spans
        .into_iter()
        .map(|s| Span {
            range: expand(s.range.start)..expand(s.range.end),
            class: s.class,
        })
        .filter(|s| {
            s.range.end <= text.len()
                && text.is_char_boundary(s.range.start)
                && text.is_char_boundary(s.range.end)
        })
        .collect()
}

/// GHD `calcSearchTokens`: case-insensitive substring hits, hunk rows skipped.
pub fn search_rows(rows: &[Row], query: &str, case_sensitive: bool) -> Vec<SearchHit> {
    if query.is_empty() {
        return Vec::new();
    }
    // `1305-diff-find-controls`: Aa matches case; GHD always ignores it
    // (ASCII folding keeps byte offsets)
    let fold = |text: &str| {
        if case_sensitive {
            text.to_string()
        } else {
            text.to_ascii_lowercase()
        }
    };
    let needle = fold(query);
    let mut hits = Vec::new();
    for (ix, row) in rows.iter().enumerate() {
        if row.kind == DiffLineKind::Hunk {
            continue;
        }
        let hay = fold(&row.text);
        let mut from = 0;
        while let Some(pos) = hay[from..].find(&needle) {
            let start = from + pos;
            let end = start + needle.len();
            hits.push(SearchHit {
                row: ix,
                range: start..end,
            });
            from = end;
        }
    }
    hits
}

/// `isInSelection`: stored selection combined with the drag in progress.
pub fn is_selected(sel: &DiffSelection, temp: Option<TempSelection>, line: u32) -> bool {
    let stored = sel.is_selected(line);
    match temp {
        None => stored,
        Some(t) => {
            let in_temp = line >= t.from.min(t.to) && line <= t.from.max(t.to);
            if t.selected {
                stored || in_temp
            } else {
                stored && !in_temp
            }
        }
    }
}

/// Syntax colours, search backgrounds and the text colour of the intra-line
/// change range (`diff-add-inner` / `diff-delete-inner`; its background is
/// painted by [`selectable_text`]) as one sorted, non-overlapping highlight
/// list for `StyledText`.
fn merge_highlights(
    spans: &[Span],
    hits: &[(Range<usize>, bool)],
    inner: &[(Range<usize>, Hsla)],
    selection: Option<Range<usize>>,
    len: usize,
    t: &GhdTheme,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut cuts: Vec<usize> = vec![0, len];
    for s in spans {
        cuts.push(s.range.start.min(len));
        cuts.push(s.range.end.min(len));
    }
    for (r, _) in hits {
        cuts.push(r.start.min(len));
        cuts.push(r.end.min(len));
    }
    for (r, _) in inner {
        cuts.push(r.start.min(len));
        cuts.push(r.end.min(len));
    }
    if let Some(r) = &selection {
        cuts.push(r.start.min(len));
        cuts.push(r.end.min(len));
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = Vec::new();
    for pair in cuts.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if a >= b {
            continue;
        }
        // `_diff.scss`: "Intra line markings takes precedence over syntax
        // highlighting" (`color: var(--diff-add-text-color) !important`)
        let color = inner
            .iter()
            .find(|(r, _)| r.start <= a && r.end >= b)
            .map(|(_, fg)| *fg)
            .or_else(|| {
                spans
                    .iter()
                    .find(|s| s.range.start <= a && s.range.end >= b)
                    .map(|s| token_color(s.class, t))
            });
        let hit = hits.iter().find(|(r, _)| r.start <= a && r.end >= b);
        let selected = selection
            .as_ref()
            .is_some_and(|r| r.start <= a && r.end >= b);
        if color.is_none() && hit.is_none() && !selected {
            continue;
        }
        let (background, fg) = match hit {
            // `.cm-search-result.cm-selected`: system Highlight
            Some((_, true)) => (
                Some(t.box_selected_active_background),
                Some(t.box_selected_active_text),
            ),
            // `.cm-search-result`: rgba(255, 255, 0, 0.4)
            Some((_, false)) => (Some(hsla(1. / 6., 1., 0.5, 0.4)), None),
            None => (None, None),
        };
        // the text selection paints over everything else
        let background = if selected {
            Some(t.text_selection_background)
        } else {
            background
        };
        out.push((
            a..b,
            HighlightStyle {
                color: fg.or(color),
                background_color: background,
                ..Default::default()
            },
        ));
    }
    out
}

/// `748-diff-show-whitespace`: a centred dot on every space and a line
/// across every expanded tab (`tabs`: where they start).
fn paint_whitespace(layout: &TextLayout, tabs: &[u32], color: Hsla, window: &mut Window) {
    let Some(line) = layout.line_layout_for_index(0) else {
        return;
    };
    let unwrapped = &line.unwrapped_layout;
    let line_height = layout.line_height();
    let text_origin = layout.bounds().origin;
    let middle = line_height / 2.;
    let dot = zpx(2.);
    let text = layout.text();
    let mut tab_end = 0;
    for (ix, ch) in text.char_indices() {
        if ch != ' ' || ix < tab_end {
            continue;
        }
        let origin = text_origin + wrapped_position(&line, ix, line_height);
        let is_tab = tabs.binary_search(&(ix as u32)).is_ok();
        let len = if is_tab { TAB_EXPANSION.len() } else { 1 };
        let width = unwrapped.x_for_index(ix + len) - unwrapped.x_for_index(ix);
        let bounds = if is_tab {
            tab_end = ix + len;
            Bounds::new(
                point(origin.x + zpx(2.), origin.y + middle - px(0.5)),
                size((width - zpx(4.)).max(px(1.)), px(1.)),
            )
        } else {
            Bounds::new(
                point(origin.x + (width - dot) / 2., origin.y + middle - dot / 2.),
                size(dot, dot),
            )
        };
        window.paint_quad(fill(bounds, color));
    }
}

/// Paints `range` of a laid-out text like a CSS inline background: one quad
/// per visual line, as tall as the font's ascent + descent and centred in
/// the line box.
fn paint_inline_background(
    layout: &TextLayout,
    range: Range<usize>,
    color: Hsla,
    window: &mut Window,
) {
    let Some(line) = layout.line_layout_for_index(range.start) else {
        return;
    };
    let unwrapped = &line.unwrapped_layout;
    let content = unwrapped.ascent + unwrapped.descent;
    let line_height = layout.line_height();
    let inset = (line_height - content) / 2.;
    let text_origin = layout.bounds().origin;
    let text = layout.text();
    // (y, x start, x end) of each visual line the range covers
    let mut segments: Vec<(Pixels, Pixels, Pixels)> = Vec::new();
    for (offset, ch) in text.get(range.clone()).unwrap_or("").char_indices() {
        let ix = range.start + offset;
        let origin = text_origin + wrapped_position(&line, ix, line_height);
        let width = unwrapped.x_for_index(ix + ch.len_utf8()) - unwrapped.x_for_index(ix);
        match segments.last_mut() {
            Some((y, _, end)) if *y == origin.y => *end = origin.x + width,
            _ => segments.push((origin.y, origin.x, origin.x + width)),
        }
    }
    for (y, start, end) in segments {
        window.paint_quad(fill(
            Bounds::new(point(start, y + inset), size(end - start, content)),
            color,
        ));
    }
}

/// Where byte `ix` of a one-line text laid out as `line` sits, relative to
/// the text's origin. A wrap boundary starts the visual line it opens: GPUI's
/// `position_for_index` puts it at the end of the line before, which would
/// paint the first character of every wrapped line past the previous line.
pub(crate) fn wrapped_position(
    line: &WrappedLineLayout,
    ix: usize,
    line_height: Pixels,
) -> Point<Pixels> {
    let unwrapped = &line.unwrapped_layout;
    let (row, start) = wrap_starts(line)
        .take_while(|&start| start <= ix)
        .fold((0, 0), |(row, _), start| (row + 1, start));
    point(
        unwrapped.x_for_index(ix) - unwrapped.x_for_index(start),
        line_height * row as f32,
    )
}

/// The byte index each wrapped (second and later) visual line starts at.
pub(crate) fn wrap_starts(line: &WrappedLineLayout) -> impl Iterator<Item = usize> + '_ {
    let runs = &line.unwrapped_layout.runs;
    line.wrap_boundaries().iter().filter_map(|b| {
        runs.get(b.run_ix)
            .and_then(|run| run.glyphs.get(b.glyph_ix))
            .map(|glyph| glyph.index)
    })
}

/// GHD `getHunkExpansionElementInfo`: icon, tooltip and target per handle.
fn expansion_handle(
    ctx: &RowContext,
    row: &Row,
    kind: HunkExpansionType,
    width: f32,
    height: Pixels,
    cx: &App,
) -> AnyElement {
    let t = cx.ghd();
    let (icon, title, target, direction) = match kind {
        HunkExpansionType::Up => (
            Octicon::FoldUp,
            "Expand Up",
            row.hunk_index,
            ExpansionKind::Up,
        ),
        HunkExpansionType::Down => (
            Octicon::FoldDown,
            "Expand Down",
            row.hunk_index.saturating_sub(1),
            ExpansionKind::Down,
        ),
        HunkExpansionType::Short => (
            Octicon::Fold,
            "Expand All",
            row.hunk_index,
            ExpansionKind::Up,
        ),
        _ => {
            return div()
                .w(zpx(width))
                .h(height)
                .flex_none()
                .bg(t.diff_hunk_gutter_background)
                .into_any_element();
        }
    };
    let view = ctx.view.clone();
    let view_for_menu = ctx.view.clone();
    let hover_bg = t.diff_hover_background;
    let hover_text = t.diff_hover_text;
    div()
        .id(("hunk-expansion", row.abs as usize * 2 + direction as usize))
        .w(zpx(width))
        .h(height)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .bg(t.diff_hunk_gutter_background)
        .text_color(t.diff_hunk_text)
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg).text_color(hover_text))
        .a11y_button(title)
        .ghd_tooltip(title)
        .on_click(move |_, _, cx| {
            view.update(cx, |this, cx| this.expand(target, direction, cx))
                .ok();
        })
        .on_mouse_down(MouseButton::Right, move |ev, window, cx| {
            view_for_menu
                .update(cx, |this, cx| this.expand_menu(ev.position, window, cx))
                .ok();
        })
        .child(octicon(icon, t.diff_hunk_text).size(zpx(16.)))
        .into_any_element()
}

/// `1316-diff-moved-lines` / `1317-diff-stylistic-changes`: how far a
/// marked row's text fades.
const MARKED_TEXT_OPACITY: f32 = 0.7;

/// A marked row's background: its add/delete colour faded towards the
/// diff background.
fn faded(bg: Hsla, base: Hsla) -> Hsla {
    base.blend(bg.opacity(0.45))
}

/// The bar at the start of a marked row's text, with a tooltip naming the
/// mark (and where a moved line went or came from). The hover target is
/// wider than the bar.
fn mark_bar(
    mark: LineMark,
    kind: DiffLineKind,
    id: (&'static str, usize),
    t: &GhdTheme,
) -> AnyElement {
    let (color, tip) = match mark {
        LineMark::Moved {
            alternate,
            counterpart,
            ..
        } => {
            let color = if alternate {
                t.diff_moved_bar_alternate
            } else {
                t.diff_moved_bar
            };
            let tip = match (kind, counterpart) {
                (DiffLineKind::Delete, Some(n)) => format!("Moved to line {n}"),
                (DiffLineKind::Add, Some(n)) => format!("Moved from line {n}"),
                _ => "Moved".to_string(),
            };
            (color, tip)
        }
        LineMark::Stylistic => (
            t.diff_stylistic_bar,
            "Stylistic change: only whitespace or formatting differs".to_string(),
        ),
    };
    div()
        .id(id)
        .absolute()
        .top_0()
        .bottom_0()
        .left_0()
        .w(zpx(8.))
        .child(div().h_full().w(zpx(3.)).bg(color))
        .ghd_tooltip(tip)
        .into_any_element()
}

pub fn render_row(ctx: &RowContext, ix: usize, row: &Row, cx: &App) -> AnyElement {
    let t = cx.ghd();
    let abs = row.abs;
    let changed = matches!(row.kind, DiffLineKind::Add | DiffLineKind::Delete);
    let selectable = ctx.selectable;
    let (row_bg, row_text, gutter_bg, gutter_border) = match row.kind {
        DiffLineKind::Add => (
            t.diff_add_background,
            t.diff_add_text,
            t.diff_add_gutter_background,
            t.diff_add_border,
        ),
        DiffLineKind::Delete => (
            t.diff_delete_background,
            t.diff_delete_text,
            t.diff_delete_gutter_background,
            t.diff_delete_border,
        ),
        DiffLineKind::Context => (
            t.background,
            t.diff_text,
            t.diff_gutter_background,
            t.diff_border,
        ),
        DiffLineKind::Hunk => (
            t.diff_hunk_background,
            t.diff_hunk_text,
            t.diff_hunk_gutter_background,
            t.diff_hunk_gutter_background,
        ),
    };
    let mark = ctx.mark(ix, row.kind);
    let row_bg = match mark {
        Some(_) => faded(row_bg, t.background),
        None => row_bg,
    };
    let prefix = match row.kind {
        DiffLineKind::Add => "  +  ",
        DiffLineKind::Delete => "  -  ",
        _ => "     ",
    };
    let view_for_text_menu = ctx.view.clone();
    let view_for_open = ctx.view.clone();
    let line = row.new;
    let discard = row.discard_target();
    let area = div()
        .id(("diff-text", abs as usize))
        .flex_1()
        .min_w_0()
        .capture_any_mouse_down(move |ev, _, cx| {
            open_at_line_on_alt_click(&view_for_open, ev, line, cx)
        })
        .on_mouse_down(MouseButton::Right, move |ev, window, cx| {
            view_for_text_menu
                .update(cx, |this, cx| {
                    this.text_menu(ev.position, line, discard, window, cx)
                })
                .ok();
        });
    let mut parts: Vec<AnyElement> = vec![
        div()
            .flex_none()
            .whitespace_nowrap()
            .child(prefix)
            .into_any_element(),
    ];
    parts.push({
        let spans: &[Span] = ctx
            .tokens
            .as_ref()
            .and_then(|tk| tk.get(ix))
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        let hits: &[(Range<usize>, bool)] = ctx
            .search
            .as_ref()
            .and_then(|s| s.by_row.get(&ix))
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        let (inner_bg_color, inner_fg_color) = if row.kind == DiffLineKind::Delete {
            (t.diff_delete_inner_background, t.diff_delete_text)
        } else {
            (t.diff_add_inner_background, t.diff_add_text)
        };
        let ranges = match mark {
            Some(_) => &[],
            None => ctx.inner.get(ix).map(Vec::as_slice).unwrap_or(&[]),
        };
        let inner_fg: Vec<(Range<usize>, Hsla)> =
            ranges.iter().map(|r| (r.clone(), inner_fg_color)).collect();
        let inner_bg: Vec<(Range<usize>, Hsla)> =
            ranges.iter().map(|r| (r.clone(), inner_bg_color)).collect();
        let selection = ctx.selection_range(ix, Column::Before, row.text.len());
        let highlights =
            if spans.is_empty() && hits.is_empty() && inner_fg.is_empty() && selection.is_none() {
                Vec::new()
            } else {
                merge_highlights(spans, hits, &inner_fg, selection, row.text.len(), t)
            };
        selectable_text(ctx, ix, Column::Before, row, highlights, inner_bg).into_any_element()
    });
    if row.no_newline {
        parts.push(
            div()
                .flex_none()
                .italic()
                .ml(zpx(4.))
                .text_color(t.diff_alt_text)
                .child("No newline at end of file")
                .into_any_element(),
        );
    }
    let content = row_text_area(ctx, area, parts).when_some(mark, |d, mark| {
        d.relative().opacity(MARKED_TEXT_OPACITY).child(mark_bar(
            mark,
            row.kind,
            ("diff-mark", abs as usize),
            t,
        ))
    });
    // `.has-check-all-control`: 16 px strip with check marks, 4 px without
    let check_marks = selectable && ctx.show_check_marks;
    let handle_width = match (selectable, check_marks) {
        (false, _) => 0.,
        (true, true) => HANDLE_WIDTH,
        (true, false) => 4.,
    };
    // `lineGutterWidth`: both columns, plus the check column when shown
    let gutter_width = if check_marks { CHECK_WIDTH } else { 0. } + 2. * ctx.line_number_width;
    let height = row.height();

    let mut el = div()
        .id(("diff-row", abs as usize))
        .min_h(height)
        .w_full()
        .flex_none()
        .flex()
        .flex_row()
        .items_stretch()
        .bg(row_bg)
        .text_color(row_text);

    if row.kind == DiffLineKind::Hunk {
        // `.hunk-info`: the gutter holds the expansion handle(s)
        let width = handle_width + gutter_width + 1.;
        let gutter: AnyElement = match row.expansion {
            HunkExpansionType::Both => div()
                .flex_none()
                .flex()
                .flex_col()
                .child(expansion_handle(
                    ctx,
                    row,
                    HunkExpansionType::Down,
                    width,
                    DIFF_LINE_HEIGHT(),
                    cx,
                ))
                .child(expansion_handle(
                    ctx,
                    row,
                    HunkExpansionType::Up,
                    width,
                    DIFF_LINE_HEIGHT(),
                    cx,
                ))
                .into_any_element(),
            kind => expansion_handle(ctx, row, kind, width, height, cx),
        };
        return el
            .child(gutter)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .child(content),
            )
            .into_any_element();
    }

    // ---- hunk handle strip (16 px) ----
    if selectable {
        let strip = if let (Some((start, len)), Some(group_type)) = (row.group, row.group_type) {
            let kind = ctx
                .groups
                .get(&start)
                .copied()
                .unwrap_or(DiffSelectionType::None);
            let bg = if kind != DiffSelectionType::None {
                t.diff_selected_border
            } else {
                t.diff_empty_hunk_handle
            };
            let repo = ctx.repo;
            let path_for_click = ctx.path.clone();
            let view = ctx.view.clone();
            let view_for_menu = ctx.view.clone();
            let view_for_hint = ctx.view.clone();
            let hide_whitespace = ctx.hide_whitespace;
            div()
                .id(("hunk-handle", abs as usize))
                .w(zpx(handle_width))
                .flex_none()
                .bg(bg)
                .cursor_pointer()
                .on_hover(move |hovered: &bool, _, cx| {
                    let next = if *hovered { Some(start) } else { None };
                    view.update(cx, |this, cx| this.set_hovered_group(next, cx))
                        .ok();
                })
                .on_click(move |ev: &ClickEvent, _, cx| {
                    if hide_whitespace {
                        view_for_hint
                            .update(cx, |this, cx| this.show_whitespace_hint(ev.position(), cx))
                            .ok();
                        return;
                    }
                    Dispatcher::set_diff_lines(
                        repo,
                        path_for_click.clone(),
                        start,
                        len,
                        kind != DiffSelectionType::All,
                        cx,
                    )
                })
                .on_mouse_down(MouseButton::Right, move |ev, window, cx| {
                    view_for_menu
                        .update(cx, |this, cx| {
                            this.hunk_menu(start, len, group_type, ev.position, window, cx)
                        })
                        .ok();
                })
                .when(
                    row.original == Some(start) && len > 1 && ctx.show_check_marks,
                    |d| {
                        d.flex()
                            .justify_center()
                            .items_start()
                            .pt(zpx(3.))
                            .children(
                                match kind {
                                    DiffSelectionType::All => Some(Octicon::DiffCheck),
                                    DiffSelectionType::Partial => Some(Octicon::DiffDash),
                                    DiffSelectionType::None => None,
                                }
                                .map(|icon| octicon(icon, white()).size(zpx(12.))),
                            )
                    },
                )
                .into_any_element()
        } else {
            // `.editable .row.context { border-left: 16px solid diff-border }`
            div()
                .w(zpx(handle_width))
                .flex_none()
                .bg(t.diff_border)
                .into_any_element()
        };
        el = el.child(strip);
    }

    // ---- line numbers (`.line-number`) ----
    let original = row.original.unwrap_or(abs);
    let selected = selectable && changed && is_selected(&ctx.selection, ctx.temp, original);
    let group_hover = selectable
        && row
            .group
            .map(|(s, _)| ctx.hovered_group == Some(s))
            .unwrap_or(false);
    let base = (gutter_bg, gutter_border, t.diff_line_number);
    let normal = if selected {
        (
            t.diff_selected_background,
            t.diff_selected_border,
            t.diff_selected_text,
        )
    } else {
        base
    };
    let hover = if selected {
        (
            t.diff_hover_background,
            t.diff_hover_border,
            t.diff_hover_text,
        )
    } else if row.kind == DiffLineKind::Add {
        (
            t.diff_add_hover_background,
            t.diff_add_hover_border,
            t.diff_add_hover_text,
        )
    } else {
        (
            t.diff_delete_hover_background,
            t.diff_delete_hover_border,
            t.diff_delete_hover_text,
        )
    };
    let (num_bg, num_border, num_text) = if group_hover && changed {
        hover
    } else {
        normal
    };
    let stored_selected = ctx.selection.is_selected(original);
    let number = |n: Option<u32>| {
        div()
            .flex_1()
            .flex()
            .justify_end()
            .items_start()
            .px(SPACING_HALF())
            .child(n.map(|n| n.to_string()).unwrap_or_default())
    };
    let view_for_down = ctx.view.clone();
    let view_for_menu = ctx.view.clone();
    let hide_whitespace = ctx.hide_whitespace;
    let group_type = row.group_type;
    let gutter = div()
        .id(("diff-gutter", abs as usize))
        .w(zpx(gutter_width))
        .flex_none()
        .flex()
        .flex_row()
        .items_stretch()
        .bg(num_bg)
        .border_r_1()
        .border_color(num_border)
        .text_color(num_text)
        .when(selectable && changed, |d| {
            d.cursor_pointer()
                .hover(move |s| s.bg(hover.0).border_color(hover.1).text_color(hover.2))
                .on_mouse_down(MouseButton::Left, move |ev, _, cx| {
                    view_for_down
                        .update(cx, |this, cx| {
                            if hide_whitespace {
                                this.show_whitespace_hint(ev.position, cx);
                                return;
                            }
                            this.start_selection(
                                TempSelection {
                                    from: original,
                                    to: original,
                                    selected: !stored_selected,
                                },
                                cx,
                            )
                        })
                        .ok();
                })
                .on_mouse_down(MouseButton::Right, move |ev, window, cx| {
                    if hide_whitespace {
                        return;
                    }
                    if let Some(group_type) = group_type {
                        view_for_menu
                            .update(cx, |this, cx| {
                                this.line_menu(original, group_type, ev.position, window, cx)
                            })
                            .ok();
                    }
                })
        })
        .when(check_marks, |d| {
            d.child(
                div()
                    .w(zpx(CHECK_WIDTH))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .items_start()
                    // `.line-number-check { padding-top: 3.5px }`
                    .pt(zpx(3.5))
                    .when(selected, |d| {
                        d.child(octicon(Octicon::DiffCheck, num_text).size(zpx(12.)))
                    }),
            )
        })
        .child({
            let old_number = number(row.old).border_r_1().border_color(num_border);
            // `759-wide-hunk-handle`: the old-number column toggles the
            // whole group, like the 16 px handle strip
            match row
                .group
                .filter(|_| ctx.wide_hunk_handle && selectable && changed)
            {
                Some((start, len)) => {
                    let kind = ctx
                        .groups
                        .get(&start)
                        .copied()
                        .unwrap_or(DiffSelectionType::None);
                    let (repo, path) = (ctx.repo, ctx.path.clone());
                    let (view, view_for_hint) = (ctx.view.clone(), ctx.view.clone());
                    old_number
                        .id(("diff-old-number", abs as usize))
                        .on_hover(move |hovered: &bool, _, cx| {
                            let next = if *hovered { Some(start) } else { None };
                            view.update(cx, |this, cx| this.set_hovered_group(next, cx))
                                .ok();
                        })
                        .on_mouse_down(MouseButton::Left, move |ev, _, cx| {
                            cx.stop_propagation();
                            if hide_whitespace {
                                view_for_hint
                                    .update(cx, |this, cx| {
                                        this.show_whitespace_hint(ev.position, cx)
                                    })
                                    .ok();
                                return;
                            }
                            Dispatcher::set_diff_lines(
                                repo,
                                path.clone(),
                                start,
                                len,
                                kind != DiffSelectionType::All,
                                cx,
                            )
                        })
                        .into_any_element()
                }
                None => old_number.into_any_element(),
            }
        })
        .child(number(row.new));

    // `348-pull-request-review`: the composer's range and the `+` on hover
    // (added after the gutter and the text, so it paints over them)
    let review_group_name = ctx.review.as_ref().map(|_| review_group(abs, None));
    if let Some(group) = &review_group_name {
        if review_in_range(ctx, row.kind, row.old, row.new) {
            el = el
                .bg(t.diff_selected_background)
                .text_color(t.diff_selected_text);
        }
        el = el.relative().group(group.clone());
    }
    el = el.child(gutter).child(content);
    if let Some(group) = review_group_name {
        el = el.children(review_add_button(
            ctx, row.kind, row.old, row.new, group, cx,
        ));
    }
    if selectable && changed && !hide_whitespace {
        // extend the drag as the pointer crosses changed rows
        let view = ctx.view.clone();
        el = el.on_mouse_move(move |_, _, cx| {
            view.update(cx, |this, cx| this.extend_selection(original, cx))
                .ok();
        });
    }
    el.into_any_element()
}

// ---- split (side-by-side) rows ----

/// GHD `MaxIntraLineDiffStringLength`.
pub const MAX_INTRA_LINE_DIFF_LEN: usize = 1024;

/// GHD `relativeChanges`: the byte ranges of `a` and `b` that remain once the
/// common prefix and suffix are stripped.
pub fn relative_changes(a: &str, b: &str) -> (Range<usize>, Range<usize>) {
    let ac: Vec<(usize, char)> = a.char_indices().collect();
    let bc: Vec<(usize, char)> = b.char_indices().collect();
    let max = ac.len().min(bc.len());
    let mut prefix = 0;
    while prefix < max && ac[prefix].1 == bc[prefix].1 {
        prefix += 1;
    }
    let remaining = max - prefix;
    let mut suffix = 0;
    while suffix < remaining && ac[ac.len() - 1 - suffix].1 == bc[bc.len() - 1 - suffix].1 {
        suffix += 1;
    }
    let range = |chars: &[(usize, char)], text: &str| {
        let start = chars.get(prefix).map(|c| c.0).unwrap_or(text.len());
        let end = if suffix == 0 {
            text.len()
        } else {
            chars[chars.len() - suffix].0
        };
        start..end.max(start)
    };
    (range(&ac, a), range(&bc, b))
}

/// How [`build_split_rows`] computes intra-line ranges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntraLineOptions {
    /// `746-intra-line-graphemes`: widen each range to whole grapheme
    /// clusters, so a combining mark is not split from its base character
    /// (GHD compares UTF-16 code units).
    pub graphemes: bool,
    /// Lines this long or longer get no intra-line range
    /// ([`MAX_INTRA_LINE_DIFF_LEN`]; `747-intra-line-max-length`, `None` for
    /// no limit).
    pub max_len: Option<usize>,
    /// `791-word-intra-line-diff`: highlight the changed words
    /// ([`word_changes`]) instead of GHD's one range between the common
    /// prefix and suffix.
    pub words: bool,
}

impl Default for IntraLineOptions {
    fn default() -> Self {
        Self {
            graphemes: false,
            max_len: Some(MAX_INTRA_LINE_DIFF_LEN),
            words: false,
        }
    }
}

/// Word, run of whitespace or single other character: the tokens
/// [`word_changes`] compares.
fn word_tokens(text: &str) -> Vec<Range<usize>> {
    #[derive(PartialEq)]
    enum Class {
        Word,
        Space,
        Other,
    }
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            Class::Word
        } else if c.is_whitespace() {
            Class::Space
        } else {
            Class::Other
        }
    };
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut last: Option<Class> = None;
    for (ix, c) in text.char_indices() {
        let current = class(c);
        let extends = current != Class::Other && last.as_ref() == Some(&current);
        match out.last_mut() {
            Some(range) if extends => range.end = ix + c.len_utf8(),
            _ => out.push(ix..ix + c.len_utf8()),
        }
        last = Some(current);
    }
    out
}

/// Corvene `791-word-intra-line-diff`: the byte ranges of `a` and `b` that
/// a word diff of the two finds changed (removed from `a`, added in `b`),
/// adjacent changes merged. GHD highlights a single range from the first to
/// the last changed character ([`relative_changes`]), which covers
/// unchanged words between two edits.
pub fn word_changes(a: &str, b: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let (ta, tb) = (word_tokens(a), word_tokens(b));
    let words_a: Vec<&str> = ta.iter().map(|r| &a[r.clone()]).collect();
    let words_b: Vec<&str> = tb.iter().map(|r| &b[r.clone()]).collect();
    let span = |tokens: &[Range<usize>], range: Range<usize>| {
        (!range.is_empty()).then(|| tokens[range.start].start..tokens[range.end - 1].end)
    };
    let push = |out: &mut Vec<Range<usize>>, range: Option<Range<usize>>| {
        let Some(range) = range else { return };
        match out.last_mut() {
            Some(last) if last.end == range.start => last.end = range.end,
            _ => out.push(range),
        }
    };
    let (mut ra, mut rb) = (Vec::new(), Vec::new());
    for op in similar::capture_diff_slices(similar::Algorithm::Myers, &words_a, &words_b) {
        let (tag, old, new) = op.as_tag_tuple();
        if tag != similar::DiffTag::Equal {
            push(&mut ra, span(&ta, old));
            push(&mut rb, span(&tb, new));
        }
    }
    (ra, rb)
}

/// `range` of `text` widened to the grapheme clusters it touches.
pub fn snap_to_graphemes(text: &str, range: Range<usize>) -> Range<usize> {
    use unicode_segmentation::UnicodeSegmentation;
    // in ASCII every byte is its own cluster, except CR LF
    if text.is_ascii() && !text.contains('\r') {
        return range;
    }
    let (mut start, mut end) = (range.start, range.end);
    for (ix, grapheme) in text.grapheme_indices(true) {
        let grapheme_end = ix + grapheme.len();
        if ix < range.start && range.start < grapheme_end {
            start = ix;
        }
        if ix < range.end && range.end < grapheme_end {
            end = grapheme_end;
        }
    }
    start..end
}

/// One side of a split row: the unified row it shows and, for paired
/// modified lines, the changed ranges highlighted with the inner colour
/// (GHD: one; `791-word-intra-line-diff`: one per run of changed words).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitSide {
    pub unified: usize,
    pub inner: Vec<Range<usize>>,
}

/// GHD `DiffRow` in side-by-side mode (`getDiffRowsFromHunk` / `getModifiedRows`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SplitRow {
    Hunk { unified: usize },
    Context { unified: usize },
    Added { after: SplitSide },
    Deleted { before: SplitSide },
    Modified { before: SplitSide, after: SplitSide },
}

impl SplitRow {
    /// The unified rows this split row shows (before, after).
    pub fn unified_rows(&self) -> (Option<usize>, Option<usize>) {
        match self {
            SplitRow::Hunk { unified } | SplitRow::Context { unified } => {
                (Some(*unified), Some(*unified))
            }
            SplitRow::Added { after } => (None, Some(after.unified)),
            SplitRow::Deleted { before } => (Some(before.unified), None),
            SplitRow::Modified { before, after } => (Some(before.unified), Some(after.unified)),
        }
    }
}

/// Pair the added and deleted lines of every block of changes: paired lines
/// become `Modified` rows (with intra-line ranges when the block has as many
/// additions as deletions), the rest stay on their own side.
pub fn build_split_rows(rows: &[Row], options: IntraLineOptions) -> Vec<SplitRow> {
    let mut out = Vec::with_capacity(rows.len());
    let mut i = 0;
    while i < rows.len() {
        match rows[i].kind {
            DiffLineKind::Hunk => {
                out.push(SplitRow::Hunk { unified: i });
                i += 1;
            }
            DiffLineKind::Context => {
                out.push(SplitRow::Context { unified: i });
                i += 1;
            }
            DiffLineKind::Add | DiffLineKind::Delete => {
                let start = i;
                while i < rows.len()
                    && matches!(rows[i].kind, DiffLineKind::Add | DiffLineKind::Delete)
                {
                    i += 1;
                }
                let added: Vec<usize> = (start..i)
                    .filter(|&ix| rows[ix].kind == DiffLineKind::Add)
                    .collect();
                let deleted: Vec<usize> = (start..i)
                    .filter(|&ix| rows[ix].kind == DiffLineKind::Delete)
                    .collect();
                let with_tokens = added.len() == deleted.len();
                let pairs = added.len().min(deleted.len());
                for k in 0..pairs {
                    let (d, a) = (deleted[k], added[k]);
                    let short =
                        |ix: usize| options.max_len.is_none_or(|max| rows[ix].text.len() < max);
                    let (before_inner, after_inner) =
                        if with_tokens && short(d) && short(a) && options.words {
                            let (b, af) = word_changes(&rows[d].text, &rows[a].text);
                            let snap =
                                |text: &str, ranges: Vec<Range<usize>>| -> Vec<Range<usize>> {
                                    if options.graphemes {
                                        ranges
                                            .into_iter()
                                            .map(|r| snap_to_graphemes(text, r))
                                            .collect()
                                    } else {
                                        ranges
                                    }
                                };
                            (snap(&rows[d].text, b), snap(&rows[a].text, af))
                        } else if with_tokens && short(d) && short(a) {
                            let (mut b, mut af) = relative_changes(&rows[d].text, &rows[a].text);
                            if options.graphemes {
                                // widen both sides alike: the common prefix and
                                // suffix stay the same length
                                let (b2, af2) = (
                                    snap_to_graphemes(&rows[d].text, b.clone()),
                                    snap_to_graphemes(&rows[a].text, af.clone()),
                                );
                                let (lead, trail) = (
                                    (b.start - b2.start).max(af.start - af2.start),
                                    (b2.end - b.end).max(af2.end - af.end),
                                );
                                let (dl, al) = (rows[d].text.len(), rows[a].text.len());
                                b = b.start.saturating_sub(lead)..(b.end + trail).min(dl);
                                af = af.start.saturating_sub(lead)..(af.end + trail).min(al);
                            }
                            (vec![b], vec![af])
                        } else {
                            (Vec::new(), Vec::new())
                        };
                    out.push(SplitRow::Modified {
                        before: SplitSide {
                            unified: d,
                            inner: before_inner,
                        },
                        after: SplitSide {
                            unified: a,
                            inner: after_inner,
                        },
                    });
                }
                for &d in &deleted[pairs..] {
                    out.push(SplitRow::Deleted {
                        before: SplitSide {
                            unified: d,
                            inner: Vec::new(),
                        },
                    });
                }
                for &a in &added[pairs..] {
                    out.push(SplitRow::Added {
                        after: SplitSide {
                            unified: a,
                            inner: Vec::new(),
                        },
                    });
                }
            }
        }
    }
    out
}

/// For each unified row, the split row that shows it.
pub fn unified_to_split(split: &[SplitRow], unified_len: usize) -> Vec<usize> {
    let mut map = vec![0; unified_len];
    for (ix, row) in split.iter().enumerate() {
        let (b, a) = row.unified_rows();
        for u in [b, a].into_iter().flatten() {
            if u < unified_len {
                map[u] = ix;
            }
        }
    }
    map
}

/// GHD `getModifiedRows` without `showSideBySideDiff`: the unified view
/// highlights the same intra-line ranges, the n-th deleted line of a block
/// against its n-th added line.
pub fn unified_inner(split: &[SplitRow], unified_len: usize) -> Vec<Vec<Range<usize>>> {
    let mut map = vec![Vec::new(); unified_len];
    for row in split {
        if let SplitRow::Modified { before, after } = row {
            for side in [before, after] {
                if let Some(slot) = map.get_mut(side.unified) {
                    *slot = side.inner.clone();
                }
            }
        }
    }
    map
}

/// Which column of a split row a side belongs to (`DiffColumn`); unified
/// rows count as `Before`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Column {
    Before,
    After,
}

/// Where the text of every rendered row sits on screen and how it is laid
/// out (wrapped), keyed by list index and column: filled while rows paint,
/// read by the text-selection hit-testing.
pub type TextBounds = Rc<RefCell<HashMap<(usize, Column), RowText>>>;

/// One row's text on screen: [`TextBounds`] entry.
#[derive(Clone)]
pub struct RowText {
    pub bounds: Bounds<Pixels>,
    pub layout: TextLayout,
}

/// `.line-number` of one side: `[check][number]`, selectable when the line
/// is a change (`renderLineNumber`).
#[allow(clippy::too_many_arguments)]
fn split_line_number(
    ctx: &RowContext,
    row: &Row,
    number: Option<u32>,
    column: Column,
    changed: bool,
    empty: bool,
    cx: &App,
) -> AnyElement {
    let t = cx.ghd();
    let selectable = ctx.selectable;
    let check_width = if selectable && ctx.show_check_marks {
        CHECK_WIDTH
    } else {
        0.
    };
    let original = row.original.unwrap_or(row.abs);
    let selected = selectable && changed && is_selected(&ctx.selection, ctx.temp, original);
    let group_hover = selectable
        && row
            .group
            .map(|(s, _)| ctx.hovered_group == Some(s))
            .unwrap_or(false);
    let (gutter_bg, gutter_border, text) = if empty {
        (
            t.diff_empty_row_gutter_background,
            t.diff_border,
            t.diff_line_number,
        )
    } else {
        match (changed, column) {
            (true, Column::Before) => (
                t.diff_delete_gutter_background,
                t.diff_delete_border,
                t.diff_line_number,
            ),
            (true, Column::After) => (
                t.diff_add_gutter_background,
                t.diff_add_border,
                t.diff_line_number,
            ),
            (false, _) => (t.diff_gutter_background, t.diff_border, t.diff_line_number),
        }
    };
    let normal = if selected {
        (
            t.diff_selected_background,
            t.diff_selected_border,
            t.diff_selected_text,
        )
    } else {
        (gutter_bg, gutter_border, text)
    };
    let hover = if selected {
        (
            t.diff_hover_background,
            t.diff_hover_border,
            t.diff_hover_text,
        )
    } else if column == Column::After {
        (
            t.diff_add_hover_background,
            t.diff_add_hover_border,
            t.diff_add_hover_text,
        )
    } else {
        (
            t.diff_delete_hover_background,
            t.diff_delete_hover_border,
            t.diff_delete_hover_text,
        )
    };
    let (bg, border, fg) = if group_hover && changed {
        hover
    } else {
        normal
    };
    let stored_selected = ctx.selection.is_selected(original);
    let view_for_down = ctx.view.clone();
    let view_for_menu = ctx.view.clone();
    let hide_whitespace = ctx.hide_whitespace;
    let group_type = row.group_type;
    let side = if column == Column::Before { 0 } else { 1 };
    // `348-pull-request-review`: the `+` of this side's line (the old
    // side's number is a LEFT comment, the new side's a RIGHT one)
    let review_button = match (ctx.review.as_ref().filter(|r| r.can_write), number, empty) {
        (Some(review), Some(line), false) => {
            let side = if column == Column::Before {
                corvene_core::pull_request_review::DiffSide::Left
            } else {
                corvene_core::pull_request_review::DiffSide::Right
            };
            Some(crate::review_threads::add_comment_button(
                ctx.repo,
                &ctx.path,
                side,
                line,
                review.composer.as_ref().map(|(t, _, _)| t),
                review_group(row.abs, Some(column)),
                cx,
            ))
        }
        _ => None,
    };
    div()
        .id(("split-gutter", row.abs as usize * 2 + side))
        .w(zpx(ctx.line_number_width + check_width))
        .flex_none()
        .flex()
        .flex_row()
        .items_stretch()
        .relative()
        .bg(bg)
        .when(column == Column::Before, |d| d.border_l_1())
        .when(column == Column::After, |d| d.border_r_1())
        .border_color(border)
        .text_color(fg)
        .when(selectable && changed, |d| {
            d.cursor_pointer()
                .hover(move |s| s.bg(hover.0).border_color(hover.1).text_color(hover.2))
                .on_mouse_down(MouseButton::Left, move |ev, _, cx| {
                    view_for_down
                        .update(cx, |this, cx| {
                            if hide_whitespace {
                                this.show_whitespace_hint(ev.position, cx);
                                return;
                            }
                            this.start_selection(
                                TempSelection {
                                    from: original,
                                    to: original,
                                    selected: !stored_selected,
                                },
                                cx,
                            )
                        })
                        .ok();
                })
                .on_mouse_down(MouseButton::Right, move |ev, window, cx| {
                    if hide_whitespace {
                        return;
                    }
                    if let Some(group_type) = group_type {
                        view_for_menu
                            .update(cx, |this, cx| {
                                this.line_menu(original, group_type, ev.position, window, cx)
                            })
                            .ok();
                    }
                })
        })
        .when(check_width > 0., |d| {
            d.child(
                div()
                    .w(zpx(CHECK_WIDTH))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .items_start()
                    // `.line-number-check { padding-top: 3.5px }`
                    .pt(zpx(3.5))
                    .when(selected, |d| {
                        d.child(octicon(Octicon::DiffCheck, fg).size(zpx(12.)))
                    }),
            )
        })
        .child(
            div()
                .flex_1()
                .flex()
                .justify_end()
                .items_start()
                .px(SPACING_HALF())
                .child(number.map(|n| n.to_string()).unwrap_or_default()),
        )
        // the `+` last, so it paints over the number
        .children(review_button)
        .into_any_element()
}

/// `.content` of one side: prefix + text with syntax, search and inner-change
/// highlights (`renderContent`).
fn split_content(
    ctx: &RowContext,
    list_ix: usize,
    column: Column,
    row: &Row,
    prefix: &'static str,
    inner: Option<(&[Range<usize>], Hsla, Hsla)>,
    cx: &App,
) -> AnyElement {
    let t = cx.ghd();
    let unified = row.abs as usize;
    let spans: &[Span] = ctx
        .tokens
        .as_ref()
        .and_then(|tk| tk.get(unified))
        .map(|v| v.as_slice())
        .unwrap_or(&[]);
    let hits: &[(Range<usize>, bool)] = ctx
        .search
        .as_ref()
        .and_then(|s| s.by_row.get(&unified))
        .map(|v| v.as_slice())
        .unwrap_or(&[]);
    let selection = ctx.selection_range(list_ix, column, row.text.len());
    let (inner_fg, inner_bg): (Vec<_>, Vec<_>) = inner
        .map(|(ranges, bg, fg)| {
            ranges
                .iter()
                .map(|r| ((r.clone(), fg), (r.clone(), bg)))
                .unzip()
        })
        .unwrap_or_default();
    let highlights =
        if spans.is_empty() && hits.is_empty() && inner_fg.is_empty() && selection.is_none() {
            Vec::new()
        } else {
            merge_highlights(spans, hits, &inner_fg, selection, row.text.len(), t)
        };
    let body = selectable_text(ctx, list_ix, column, row, highlights, inner_bg);
    let view_for_menu = ctx.view.clone();
    let view_for_open = ctx.view.clone();
    let line = row.new;
    let discard = row.discard_target();
    let area = div()
        .id(("split-text", unified))
        .flex_1()
        .min_w_0()
        .capture_any_mouse_down(move |ev, _, cx| {
            open_at_line_on_alt_click(&view_for_open, ev, line, cx)
        })
        .on_mouse_down(MouseButton::Right, move |ev, window, cx| {
            view_for_menu
                .update(cx, |this, cx| {
                    this.text_menu(ev.position, line, discard, window, cx)
                })
                .ok();
        });
    let mut parts = vec![
        div()
            .flex_none()
            .whitespace_nowrap()
            .child(prefix)
            .into_any_element(),
        body.into_any_element(),
    ];
    if row.no_newline {
        parts.push(
            div()
                .flex_none()
                .italic()
                .ml(zpx(4.))
                .text_color(t.diff_alt_text)
                .child("No newline at end of file")
                .into_any_element(),
        );
    }
    row_text_area(ctx, area, parts).into_any_element()
}

/// `.hunk-handle`: the block toggle laid over a changed split row's
/// place holder (context rows get borders instead).
fn split_handle(ctx: &RowContext, row: &Row, width: f32, cx: &App) -> AnyElement {
    let t = cx.ghd();
    // without a selectable group only the place holder shows
    if row.group.is_none() {
        return div().into_any_element();
    }
    let (start, len) = row.group.unwrap_or((0, 0));
    let group_type = row.group_type.unwrap_or(RangeType::Mixed);
    let kind = ctx
        .groups
        .get(&start)
        .copied()
        .unwrap_or(DiffSelectionType::None);
    let bg = if kind != DiffSelectionType::None {
        t.diff_selected_border
    } else {
        t.diff_empty_hunk_handle
    };
    let repo = ctx.repo;
    let path_for_click = ctx.path.clone();
    let view = ctx.view.clone();
    let view_for_menu = ctx.view.clone();
    let view_for_hint = ctx.view.clone();
    let hide_whitespace = ctx.hide_whitespace;
    div()
        .id(("split-hunk-handle", row.abs as usize))
        .absolute()
        .top_0()
        .bottom_0()
        .left(gpui_kit::relative(0.5))
        .ml(zpx(-width / 2.))
        .w(zpx(width))
        .bg(bg)
        .cursor_pointer()
        .on_hover(move |hovered: &bool, _, cx| {
            let next = if *hovered { Some(start) } else { None };
            view.update(cx, |this, cx| this.set_hovered_group(next, cx))
                .ok();
        })
        .on_click(move |ev: &ClickEvent, _, cx| {
            if hide_whitespace {
                view_for_hint
                    .update(cx, |this, cx| this.show_whitespace_hint(ev.position(), cx))
                    .ok();
                return;
            }
            Dispatcher::set_diff_lines(
                repo,
                path_for_click.clone(),
                start,
                len,
                kind != DiffSelectionType::All,
                cx,
            )
        })
        .on_mouse_down(MouseButton::Right, move |ev, window, cx| {
            view_for_menu
                .update(cx, |this, cx| {
                    this.hunk_menu(start, len, group_type, ev.position, window, cx)
                })
                .ok();
        })
        .when(
            row.original == Some(start) && len > 1 && ctx.show_check_marks,
            |d| {
                d.flex()
                    .justify_center()
                    .items_start()
                    .pt(zpx(3.))
                    .children(
                        match kind {
                            DiffSelectionType::All => Some(Octicon::DiffCheck),
                            DiffSelectionType::Partial => Some(Octicon::DiffDash),
                            DiffSelectionType::None => None,
                        }
                        .map(|icon| octicon(icon, white()).size(zpx(12.))),
                    )
            },
        )
        .into_any_element()
}

/// GHD `SideBySideDiffRow` in split mode.
pub fn render_split_row(
    ctx: &RowContext,
    ix: usize,
    row: &SplitRow,
    rows: &[Row],
    cx: &App,
) -> AnyElement {
    let t = cx.ghd();
    let selectable = ctx.selectable;
    // `--hunk-handle-width`: 4 px, 16 px with the check-all control
    let handle_width = if selectable && ctx.show_check_marks {
        HANDLE_WIDTH
    } else {
        4.
    };
    let base = div()
        .id(("split-row", ix))
        .relative()
        .min_h(DIFF_LINE_HEIGHT())
        .w_full()
        .flex_none()
        .flex()
        .flex_row()
        .items_stretch();

    match row {
        SplitRow::Hunk { unified } => {
            let r = &rows[*unified];
            let check = if selectable && ctx.show_check_marks {
                CHECK_WIDTH
            } else {
                0.
            };
            let width = ctx.line_number_width + check;
            let height = r.height();
            let gutter: AnyElement = match r.expansion {
                HunkExpansionType::Both => div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .child(expansion_handle(
                        ctx,
                        r,
                        HunkExpansionType::Down,
                        width,
                        DIFF_LINE_HEIGHT(),
                        cx,
                    ))
                    .child(expansion_handle(
                        ctx,
                        r,
                        HunkExpansionType::Up,
                        width,
                        DIFF_LINE_HEIGHT(),
                        cx,
                    ))
                    .into_any_element(),
                kind => expansion_handle(ctx, r, kind, width, height, cx),
            };
            base.min_h(height)
                .bg(t.diff_hunk_background)
                .text_color(t.diff_hunk_text)
                .child(gutter)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .child(split_content(ctx, ix, Column::Before, r, "     ", None, cx)),
                )
                .into_any_element()
        }
        SplitRow::Context { unified } => {
            let r = &rows[*unified];
            let side = |column: Column| {
                let number = if column == Column::Before {
                    r.old
                } else {
                    r.new
                };
                let ln = split_line_number(ctx, r, number, column, false, false, cx);
                let content = split_content(ctx, ix, column, r, "     ", None, cx);
                let mut d = div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_row()
                    .items_stretch()
                    .bg(t.background)
                    .text_color(t.diff_text)
                    // `348-pull-request-review`
                    .when(ctx.review.is_some(), |d| {
                        let in_range = ctx.review.as_ref().is_some_and(|review| {
                            use corvene_core::pull_request_review::DiffSide;
                            match column {
                                Column::Before => review.in_composer_range(DiffSide::Left, r.old),
                                Column::After => review.in_composer_range(DiffSide::Right, r.new),
                            }
                        });
                        d.relative()
                            .group(review_group(r.abs, Some(column)))
                            .when(in_range, |d| d.bg(t.diff_selected_background))
                    })
                    // `.editable .row.context .before/.after`: half the
                    // handle width as a border on each side of the centre
                    .when(selectable, |d| {
                        let half = zpx(handle_width / 2.);
                        let d = d.border_color(t.diff_border);
                        if column == Column::Before {
                            d.border_r(half)
                        } else {
                            d.border_l(half)
                        }
                    });
                // `.editable .row .before { flex-direction: row-reverse }`
                if selectable && column == Column::Before {
                    d = d.child(content).child(ln);
                } else {
                    d = d.child(ln).child(content);
                }
                d
            };
            base.child(side(Column::Before))
                .child(side(Column::After))
                .into_any_element()
        }
        SplitRow::Added { .. } | SplitRow::Deleted { .. } | SplitRow::Modified { .. } => {
            let (before, after) = match row {
                SplitRow::Added { after } => (None, Some(after)),
                SplitRow::Deleted { before } => (Some(before), None),
                SplitRow::Modified { before, after } => (Some(before), Some(after)),
                _ => (None, None),
            };
            let group_row = before
                .or(after)
                .map(|s| &rows[s.unified])
                .expect("a changed split row has a side");
            let side = |column: Column, side: Option<&SplitSide>| {
                let mark = side.and_then(|s| ctx.mark(s.unified, rows[s.unified].kind));
                let (bg, fg) = match (side.is_some(), column) {
                    (false, _) => (t.diff_empty_row_background, t.diff_text),
                    (true, Column::Before) => (t.diff_delete_background, t.diff_delete_text),
                    (true, Column::After) => (t.diff_add_background, t.diff_add_text),
                };
                let bg = match mark {
                    Some(_) => faded(bg, t.background),
                    None => bg,
                };
                let mut d = div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_row()
                    .items_stretch()
                    .bg(bg)
                    .text_color(fg)
                    // `348-pull-request-review`
                    .when_some(side.filter(|_| ctx.review.is_some()), |d, s| {
                        let r = &rows[s.unified];
                        let in_range = review_in_range(ctx, r.kind, r.old, r.new);
                        d.relative()
                            .group(review_group(r.abs, Some(column)))
                            .when(in_range, |d| d.bg(t.diff_selected_background))
                    });
                let (ln, content): (AnyElement, AnyElement) = match side {
                    Some(s) => {
                        let r = &rows[s.unified];
                        let number = if column == Column::Before {
                            r.old
                        } else {
                            r.new
                        };
                        let (inner_bg, inner_fg) = if column == Column::Before {
                            (t.diff_delete_inner_background, t.diff_delete_text)
                        } else {
                            (t.diff_add_inner_background, t.diff_add_text)
                        };
                        let ranges: &[Range<usize>] = match mark {
                            Some(_) => &[],
                            None => s.inner.as_slice(),
                        };
                        let inner = Some((ranges, inner_bg, inner_fg));
                        let prefix = if column == Column::Before {
                            "  -  "
                        } else {
                            "  +  "
                        };
                        let content = split_content(ctx, ix, column, r, prefix, inner, cx);
                        let content = match mark {
                            Some(mark) => {
                                let id = match column {
                                    Column::Before => "split-mark-before",
                                    Column::After => "split-mark-after",
                                };
                                div()
                                    .relative()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .opacity(MARKED_TEXT_OPACITY)
                                    .child(content)
                                    .child(mark_bar(mark, r.kind, (id, r.abs as usize), t))
                                    .into_any_element()
                            }
                            None => content,
                        };
                        (
                            split_line_number(ctx, r, number, column, true, false, cx),
                            content,
                        )
                    }
                    None => (
                        split_line_number(ctx, group_row, None, column, false, true, cx),
                        div().flex_1().into_any_element(),
                    ),
                };
                if selectable && column == Column::Before {
                    d = d.child(content).child(ln);
                } else {
                    d = d.child(ln).child(content);
                }
                if let Some(s) = side.filter(|_| selectable) {
                    // extend the drag as the pointer crosses changed rows
                    let original = rows[s.unified].original.unwrap_or(rows[s.unified].abs);
                    let view = ctx.view.clone();
                    let hide_whitespace = ctx.hide_whitespace;
                    d = d.on_mouse_move(move |_, _, cx| {
                        if !hide_whitespace {
                            view.update(cx, |this, cx| this.extend_selection(original, cx))
                                .ok();
                        }
                    });
                }
                d
            };
            // `.hunk-handle-place-holder` keeps the handle's width in the
            // flow; the interactive handle is laid over it
            base.child(side(Column::Before, before))
                .when(selectable, |d| {
                    d.child(
                        div()
                            .flex_none()
                            .w(zpx(handle_width))
                            .bg(t.diff_empty_hunk_handle),
                    )
                })
                .child(side(Column::After, after))
                .when(selectable, |d| {
                    d.child(split_handle(ctx, group_row, handle_width, cx))
                })
                .into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    // explicit imports: `gpui_kit::*` would shadow `#[test]` with GPUI's macro
    use super::{
        IntraLineOptions, MAX_INTRA_LINE_DIFF_LEN, RangeType, SearchHit, SplitRow, build_rows,
        build_split_rows, expand_tabs, relative_changes, search_rows, snap_to_graphemes,
        spans_for_row, tab_offsets, unified_inner, unified_to_split, word_changes,
    };
    use corvene_core::{DiffHunk, DiffLine, DiffLineKind};

    fn hunk() -> DiffHunk {
        let l = |kind, text: &str| DiffLine {
            kind,
            text: text.into(),
            old_line: None,
            new_line: None,
            no_trailing_newline: false,
            carriage_return: false,
            raw: None,
        };
        DiffHunk {
            unified_diff_start: 0,
            header: "@@ -1,3 +1,3 @@".into(),
            old_start: 1,
            old_lines: 3,
            new_start: 1,
            new_lines: 3,
            lines: vec![
                l(DiffLineKind::Hunk, "@@ -1,3 +1,3 @@"),
                l(DiffLineKind::Context, "Alpha"),
                l(DiffLineKind::Delete, "beta"),
                l(DiffLineKind::Add, "Beta"),
                l(DiffLineKind::Add, "gamma"),
                l(DiffLineKind::Context, "delta"),
            ],
        }
    }

    #[test]
    fn groups_carry_original_indices_and_kind() {
        let x = crate::diff_expansion::from_hunks(&[hunk()], None);
        let rows = build_rows(&x);
        assert_eq!(rows[2].group, Some((2, 3)));
        assert_eq!(rows[2].group_type, Some(RangeType::Mixed));
        assert_eq!(rows[4].original, Some(4));
        assert_eq!(rows[1].group, None);
    }

    #[test]
    fn search_is_case_insensitive_and_skips_hunk_rows() {
        let x = crate::diff_expansion::from_hunks(&[hunk()], None);
        let rows = build_rows(&x);
        let hits = search_rows(&rows, "BETA", false);
        assert_eq!(
            hits,
            vec![
                SearchHit {
                    row: 2,
                    range: 0..4
                },
                SearchHit {
                    row: 3,
                    range: 0..4
                }
            ]
        );
        assert!(search_rows(&rows, "@@", false).is_empty());
        assert!(search_rows(&rows, "", false).is_empty());
        // `1305-diff-find-controls`: Aa
        assert!(search_rows(&rows, "BETA", true).is_empty());
        let row_of = |q| {
            search_rows(&rows, q, true)
                .iter()
                .map(|h| h.row)
                .collect::<Vec<_>>()
        };
        assert_eq!(row_of("beta"), vec![2]);
        assert_eq!(row_of("Beta"), vec![3]);
    }

    #[test]
    fn discard_labels_match_ghd() {
        let mac = crate::context_menu::IS_MAC;
        assert_eq!(
            RangeType::Additions.discard_label(1, true),
            if mac {
                "Discard Added Line…"
            } else {
                "Discard added line…"
            }
        );
        assert_eq!(
            RangeType::Mixed.discard_label(3, false),
            if mac {
                "Discard Modified Lines"
            } else {
                "Discard modified lines"
            }
        );
        assert_eq!(
            RangeType::Deletions.discard_label(2, true),
            if mac {
                "Discard Removed Lines…"
            } else {
                "Discard removed lines…"
            }
        );
    }

    #[test]
    fn relative_changes_trim_common_ends() {
        assert_eq!(
            relative_changes("hello world", "hello there world"),
            (6..6, 6..12)
        );
        assert_eq!(relative_changes("abc", "abc"), (3..3, 3..3));
        assert_eq!(relative_changes("café x", "café y"), (6..7, 6..7));
        assert_eq!(relative_changes("", "new"), (0..0, 0..3));
    }

    #[test]
    fn grapheme_snapping_keeps_combining_marks() {
        // "e" + U+0301 against "e": the change starts inside the cluster
        let (a, b) = relative_changes("xe\u{301}y", "xey");
        assert_eq!((a.clone(), b.clone()), (2..4, 2..2));
        assert_eq!(snap_to_graphemes("xe\u{301}y", a), 1..4);
        assert_eq!(snap_to_graphemes("xey", b), 2..2);
        assert_eq!(snap_to_graphemes("abc", 1..2), 1..2);
    }

    #[test]
    fn split_rows_pair_changes() {
        let x = crate::diff_expansion::from_hunks(&[hunk()], None);
        let rows = build_rows(&x);
        let split = build_split_rows(&rows, IntraLineOptions::default());
        // hunk, context, modified(beta/Beta), added(gamma), context
        assert_eq!(split.len(), 5);
        match &split[2] {
            SplitRow::Modified { before, after } => {
                assert_eq!((before.unified, after.unified), (2, 3));
                // counts differ (1 deleted, 2 added) → no intra-line ranges
                assert!(before.inner.is_empty());
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(matches!(split[3], SplitRow::Added { .. }));
        let map = unified_to_split(&split, rows.len());
        assert_eq!(map[3], 2);
        assert_eq!(map[4], 3);
    }

    #[test]
    fn unified_rows_get_intra_line_ranges() {
        let mut h = hunk();
        // drop "gamma": one deleted, one added → the pair is diffed
        h.lines.remove(4);
        let x = crate::diff_expansion::from_hunks(&[h], None);
        let rows = build_rows(&x);
        let inner = unified_inner(
            &build_split_rows(&rows, IntraLineOptions::default()),
            rows.len(),
        );
        assert_eq!(inner, vec![vec![], vec![], vec![0..1], vec![0..1], vec![]]);
        // counts differ → nothing highlighted
        let x = crate::diff_expansion::from_hunks(&[hunk()], None);
        let rows = build_rows(&x);
        let inner = unified_inner(
            &build_split_rows(&rows, IntraLineOptions::default()),
            rows.len(),
        );
        assert!(inner.iter().all(Vec::is_empty));
    }

    #[test]
    fn tab_offsets_follow_expansion() {
        assert_eq!(tab_offsets("\ta\tb"), vec![0, 5]);
        assert_eq!(expand_tabs("\ta\tb").get(5..9), Some("    "));
        assert!(tab_offsets("no tabs").is_empty());
    }

    #[test]
    fn intra_line_length_cap() {
        let mut h = hunk();
        h.lines.remove(4);
        h.lines[2].text = format!("{}beta", "x".repeat(2000));
        h.lines[3].text = format!("{}Beta", "x".repeat(2000));
        let x = crate::diff_expansion::from_hunks(&[h], None);
        let rows = build_rows(&x);
        let inner = |max_len| {
            let options = IntraLineOptions {
                max_len,
                ..Default::default()
            };
            unified_inner(&build_split_rows(&rows, options), rows.len())
        };
        assert!(
            inner(Some(MAX_INTRA_LINE_DIFF_LEN))
                .iter()
                .all(Vec::is_empty)
        );
        assert_eq!(inner(None)[2], one(2000..2001));
    }

    /// One intra-line range (a literal `[a..b]` reads like a mistake to clippy).
    fn one(range: std::ops::Range<usize>) -> Vec<std::ops::Range<usize>> {
        vec![range]
    }

    #[test]
    fn word_changes_highlight_only_the_changed_words() {
        // GHD: one range from the first to the last change
        assert_eq!(
            relative_changes("let total = price * count;", "let sum = price * amount;"),
            (4..21, 4..20)
        );
        let (a, b) = word_changes("let total = price * count;", "let sum = price * amount;");
        assert_eq!(a, [4..9, 20..25]);
        assert_eq!(b, [4..7, 18..24]);
        // adjacent changed tokens merge; an unchanged line has none
        assert_eq!(
            word_changes("a.b", "x.y"),
            (vec![0..1, 2..3], vec![0..1, 2..3])
        );
        assert_eq!(word_changes("same words", "same words"), (vec![], vec![]));
        let (removed, added) = word_changes("foo(1)", "foo(1, 2)");
        assert!(removed.is_empty());
        assert_eq!(added, one(5..8));
    }

    #[test]
    fn word_option_feeds_split_rows() {
        let mut h = hunk();
        h.lines.remove(4);
        h.lines[2].text = "one two three".into();
        h.lines[3].text = "one 2 three".into();
        let x = crate::diff_expansion::from_hunks(&[h], None);
        let rows = build_rows(&x);
        let options = IntraLineOptions {
            words: true,
            ..Default::default()
        };
        let inner = unified_inner(&build_split_rows(&rows, options), rows.len());
        assert_eq!(inner[2], one(4..7));
        assert_eq!(inner[3], one(4..5));
    }

    #[test]
    fn file_spans_follow_tab_expansion() {
        use corvene_highlight::TokenClass;
        let file = [
            "package main",
            "",
            "func f(x bool) int {",
            "\tif x {",
            "\t\treturn 1 // one",
            "\t}",
            "\treturn 0",
            "}",
        ];
        let tokens = corvene_highlight::highlight_lines("main.go", file).expect("go mode");
        for (ix, keyword) in [(3, "if"), (4, "return"), (6, "return")] {
            let raw = file[ix];
            let text = expand_tabs(raw);
            let spans = spans_for_row(tokens[ix].clone(), raw, &text);
            let span = spans
                .iter()
                .find(|s| s.class == TokenClass::Keyword)
                .expect("keyword span");
            assert_eq!(&text[span.range.clone()], keyword, "line {ix}");
        }
        // the comment after two tabs lands on the comment, not 6 bytes early
        let raw = file[4];
        let text = expand_tabs(raw);
        let spans = spans_for_row(tokens[4].clone(), raw, &text);
        let comment = spans
            .iter()
            .find(|s| s.class == TokenClass::Comment)
            .expect("comment span");
        assert_eq!(&text[comment.range.clone()], "// one");
    }
}
