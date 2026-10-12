//! Diff viewer - GHD `ui/diff/index.tsx` routing plus `side-by-side-diff.tsx`
//! in unified mode (`styles/ui/_side-by-side-diff.scss`, `.unified-diff.editable`):
//! 29 px header, then 20 px rows of [16 px hunk handle][20 px check mark | 55 |
//! 55 line numbers][prefix + text] in 11 px monospace. Clicking or dragging
//! over line numbers toggles lines for the next commit; the handle strip
//! toggles a whole block of consecutive changes (`hunkStartLine` groups).
//! Hunk headers carry fold handles (`diff_expansion`), the gutter has a
//! right-click "Discard … Lines" menu, ⌘F searches the rows, the header gear
//! opens Diff Settings, and binary / image / submodule / large diffs get their
//! own panels. Rows are virtualized with `gpui::list`.
//!
//! Deviation: "Split" (side-by-side) rendering is not implemented; the radio
//! button is shown disabled.
//!
//! Deviation (flag `diff-open-in-editor-at-line`): a working-directory diff's
//! text context menu offers "Open in <Editor> at Line N" for the clicked
//! row's new-file line when the editor can jump to a line (GHD
//! `onContextMenuText` has Copy, Select All and the expansion item only);
//! ⌥-clicking a line's text opens it there directly.
//!
//! Deviation (`742-file-mode-change-message`): a mode-only change says "The
//! file mode changed from … to …" instead of GHD's "No content changes found".
//!
//! Deviation (`744-unified-diff-for-added-files`): in Split mode a new or
//! deleted file still shows the unified layout, full width.
//!
//! Deviation (`751-diff-font-size`): the rows' font size can be set (9–16 px
//! in the 20 px rows); GHD's is fixed at 11 px.
//!
//! Deviation (`759-wide-hunk-handle`): in the unified diff the whole
//! old-line-number column can act as the hunk handle (GHD: only the 16 px
//! strip; the new-number column keeps selecting single lines).
//!
//! Deviation (`760-discard-from-text-menu`): right-clicking a changed line's
//! text in the Changes tab adds the gutter's "Discard … Line" items (for the
//! line and for its block), and "Discard N Selected Lines" for the changed
//! lines a text selection spans.
//!
//! Deviation (`762-too-large-diff-escape-hatch`): a working-directory diff
//! too large to show offers "Open in external diff tool" (with `diff.tool`
//! configured) and "Open file in <Editor>" (GHD `ui/diff/index.tsx` only says
//! it is too large).
//!
//! Deviation (`763-diff-header-mtime`): the Changes diff header can show
//! when the working file was last modified (GHD's has the path only).
//!
//! Deviation (`758-diff-line-height`): the rows' height can be set (14–32
//! px); GHD's is fixed at 20 px.
//!
//! Deviation (`746-intra-line-graphemes`): intra-line ranges cover whole
//! grapheme clusters, so a combining mark stays with its base character.
//!
//! Deviation (`747-intra-line-max-length`): the line length beyond which no
//! intra-line range is computed can be changed (GHD: 1024, fixed).
//!
//! Deviation (`1311-stacked-diff`): a view made with `new_stacked` is one
//! file of `stacked_diff_view`'s list (GHD `renderDiff` shows one file):
//! it reads that file's entry of `RepositoryState::stacked` and lends its
//! rows, panels and popovers to the list (`stacked_prepare`,
//! `stacked_body`, `stacked_overlays`) instead of rendering itself.
//!
//! Deviation (`792-whitespace-hidden-highlight`): a context row takes the
//! syntax spans of the file whose line it shows (git prints a context line
//! from one file; with Hide Whitespace the other's can differ in
//! indentation), GHD always the old file's unless the diff only adds lines.
//!
//! Deviation (`796-lfs-text-diff`): the contents diff of a file in Git LFS
//! has no line selection or expansion, with a note in the Changes tab.
//!
//! Deviation (`795-lfs-image-previews`): an LFS image whose contents are
//! not downloaded gets a note above its pointer diff.
//!
//! Deviation (`794-svg-image-diff`): an SVG file's header has a Text /
//! Image switch; Image shows its image diff (GHD: text only).
//!
//! Deviation (`791-word-intra-line-diff`): a modified line highlights each
//! changed word (a word diff of the paired lines) instead of GHD's one range
//! between the common prefix and suffix (`ui/diff/diff-helpers.tsx`
//! `getModifiedRows` / `relativeChanges`).
//!
//! Deviation (`748-diff-show-whitespace`): spaces can be marked with dots
//! and tabs with a line.
//!
//! Deviation (`749-binary-diff-as-text`): a binary working-directory file
//! offers "Show the diff as text anyway." (`git diff --text`, read-only).
//!
//! Deviation (`750-diff-expand-whole-file`): diffs can open with the whole
//! file expanded (files up to 20 000 lines).
//!
//! Deviation (`757-typechange-diff`): a type change (e.g. a file replaced by
//! a symbolic link) says so above the rows, and its lines can neither be
//! selected nor expanded, since no partial patch can describe its two file
//! sections (GHD `lib/diff-parser.ts` fails on them and keeps loading).
//!
//! Deviation (`740-diff-loading-indicator`): while a working-directory diff
//! takes longer than [`LOADING_INDICATOR_DELAY`] to compute, a spinner covers
//! the pane (GHD keeps showing the previous diff, or nothing).
//!
//! Deviation (`1304-diff-no-wrap`): View › Wrap Diff Lines and Diff
//! Settings › Wrap Long Lines can keep each line on one row; the text then
//! scrolls sideways under fixed gutters (`diff_view_rows::HScroll`, a
//! horizontal scrollbar, trackpad and ⇧-wheel). GHD always wraps.
//!
//! Deviation (`1305-diff-find-controls`): the ⌘F box shows "N of M",
//! previous / next buttons and an Aa (match case) toggle; GHD's
//! `DiffSearchInput` is a bare text box that ignores case. Enter, ⇧Enter
//! and Esc are unchanged.

use crate::csv_table_diff::CsvTable;
use corvene_core::archive_browse::ArchiveSurface;
use corvene_core::csv_diff::{SheetEntry, SheetStatus};
use corvene_core::diff_line_class::{ClassLine, ClassOptions, LineMark, classify};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use corvene_core::{
    AppState, Diff, DiffLineKind, DiffSelection, DiffSelectionType, Dispatcher, FileStatusKind,
    SubmoduleDiff,
};
use gpui_kit::component::input::{Escape, InputEvent, InputState, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::widgets::{GhdTooltip, IconButtonA11y};

use crate::actions::{Copy, Find, SelectAll};
use crate::context_menu::{ContextMenu, IS_MAC, MenuItem, mac_or};
use crate::diff_expansion::{
    DEFAULT_DIFF_EXPANSION_STEP, ExpansionKind, HunkExpansionType, XHunk, expand_hunk,
    expand_whole, from_hunks,
};
use crate::diff_view_rows::{
    Column, HScroll, IntraLineOptions, RangeType, Row, RowContext, RowText, SearchHit, SearchIndex,
    SplitRow, TempSelection, TextBounds, build_rows, build_split_rows, line_number_width,
    max_line_number, render_row, render_split_row, search_rows, spans_for_row, unified_inner,
    unified_to_split,
};
use crate::document_diff::DocumentDiff;
use crate::font_diff::FontDiffView;
use crate::font_diff::source::FontSourceView;
use crate::icons::{Octicon, octicon};
use crate::image_diff::ImageDiff;
use crate::pdf_diff::PdfDiff;
use crate::scrollbar::{ScrollbarExt, gutter, scrollbar};
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, GhdTheme, mono_font};
use crate::widgets::{
    Inline, button, checkbox_row, code_ref, link_button, paragraph, primary_button, radio_row,
};

/// `750-diff-expand-whole-file` leaves longer files collapsed.
const MAX_AUTO_EXPAND_LINES: usize = 20_000;

thread_local! {
    /// `758-diff-line-height`: the rows' height in CSS px (GHD: 20), set by
    /// the diff views as they render.
    static LINE_HEIGHT: Cell<f32> = const { Cell::new(20.) };
}

#[allow(non_snake_case)]
pub fn DIFF_LINE_HEIGHT() -> Pixels {
    zpx(LINE_HEIGHT.with(Cell::get))
}

/// `758-diff-line-height` in CSS px: 0 keeps GHD's 20 px.
pub fn diff_line_height_setting(cx: &App) -> Option<f32> {
    let height = AppState::try_global(cx)?
        .read(cx)
        .flags
        .number(corvene_core::flags::ids::DIFF_LINE_HEIGHT);
    (height > 0).then(|| height.clamp(14, 32) as f32)
}

/// `757-typechange-diff`: what a git file mode stands for.
fn file_type_name(mode: &str) -> String {
    match mode {
        "100644" => "a regular file".to_string(),
        "100755" => "an executable file".to_string(),
        "120000" => "a symbolic link".to_string(),
        "160000" => "a submodule".to_string(),
        other => format!("mode {other}"),
    }
}

/// `1323-archive-diff`: the file status an archive entry's change shows as.
pub fn archive_entry_kind(status: corvene_core::ArchiveEntryStatus) -> FileStatusKind {
    use corvene_core::ArchiveEntryStatus as S;
    match status {
        S::Added => FileStatusKind::New,
        S::Deleted => FileStatusKind::Deleted,
        S::Renamed => FileStatusKind::Renamed,
        S::Modified | S::MetadataOnly | S::Unchanged => FileStatusKind::Modified,
    }
}

/// Octicon + colour for a file status (`ui/octicons/status.ts`).
pub fn status_icon(kind: FileStatusKind, t: &GhdTheme) -> (Octicon, Hsla) {
    match kind {
        FileStatusKind::New | FileStatusKind::Untracked | FileStatusKind::Copied => {
            (Octicon::DiffAdded, t.color_new)
        }
        FileStatusKind::Modified => (Octicon::DiffModified, t.color_modified),
        FileStatusKind::Deleted => (Octicon::DiffRemoved, t.color_deleted),
        FileStatusKind::Renamed => (Octicon::DiffRenamed, t.color_renamed),
        FileStatusKind::Conflicted => (Octicon::Alert, t.color_conflicted),
    }
}

/// `.diff-header`: path (directory dimmed), the Diff Settings gear and the
/// status icon, 29 px. The gear toggles the popover owned by `view`.
/// `modified` (`763-diff-header-mtime`) adds the working file's modification
/// time before the gear.
pub fn diff_header(
    path: &str,
    kind: FileStatusKind,
    old_path: Option<&str>,
    modified: Option<std::time::SystemTime>,
    view: &Entity<DiffView>,
    cx: &App,
) -> impl IntoElement {
    let t = cx.ghd();
    let (icon, color) = status_icon(kind, t);
    // `.diff-container .header`: 5 px / 10 px padding around the 19 px
    // options button, `--diff-border-color` underneath (30 px in all)
    div()
        .h(zpx(30.))
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .px(SPACING())
        .bg(t.box_alt_background)
        .border_b_1()
        .border_color(t.diff_border)
        // GHD `ChangedFileDetails`' `PathLabel`: a rename's old path too
        .child(
            crate::path_label::path_label_element(
                crate::path_label::path_label(path, kind, old_path),
                Vec::new(),
                t.text_secondary,
                t.text,
            )
            .text_size(FONT_SIZE()),
        )
        .children(modified.map(|at| {
            div()
                .id("diff-header-modified")
                .flex_none()
                .ml(SPACING())
                .text_size(FONT_SIZE_SM())
                .text_color(t.text_secondary)
                .child(format!("Modified {}", crate::relative_time::relative(at)))
                .ghd_tooltip(crate::format::format_date_time(at))
        }))
        // `794-svg-image-diff`
        .children(svg_switch(path, cx))
        // `1318-csv-table-diff`
        .children(csv_switch(path, cx))
        // `1322-pdf-diff`
        .children(pdf_switch(path, view, cx))
        // `1330-font-source-preview`
        .children(font_source_switch(path, cx))
        // `1324-document-diff`
        .children(document_switch(view, cx))
        // `1331-notebook-diff` / `1332-latex-diff`
        .children(rich_text::rich_switch(view, cx))
        // `798-blame`
        .children(blame_button(path, kind, view, cx))
        // `.path-label-component { margin-right: 5px }`,
        // `.diff-options-component { margin-right: 5px }`
        .child(
            div()
                .ml(SPACING_HALF())
                .mr(SPACING_HALF())
                .child(diff_options_button(view, cx)),
        )
        .child(octicon(icon, color))
}

/// `798-blame`: the header's Blame button, for a working file HEAD has
/// (or `HEAD`'s copy of a deleted one) and for a file of a commit.
fn blame_button(
    path: &str,
    kind: FileStatusKind,
    view: &Entity<DiffView>,
    cx: &App,
) -> Option<impl IntoElement + use<>> {
    let s = AppState::try_global(cx)?.read(cx);
    if !s.flags.bool(corvene_core::flags::ids::BLAME) {
        return None;
    }
    let repo = s.selected?;
    let source = view.read(cx).source;
    let working = match source {
        DiffSource::WorkingDirectory if !kind.is_new_or_untracked() => true,
        DiffSource::Commit => false,
        _ => return None,
    };
    let t = cx.ghd();
    let path = path.to_string();
    Some(
        div()
            .id("diff-blame-button")
            .a11y_button("Blame")
            .flex_none()
            .ml(SPACING())
            .h(zpx(19.))
            .px(SPACING_HALF())
            .flex()
            .items_center()
            .rounded(BORDER_RADIUS())
            .border_1()
            .border_color(t.box_border)
            .text_size(FONT_SIZE_SM())
            .text_color(t.text)
            .cursor_pointer()
            .hover(|d| d.bg(t.box_hover_background))
            .ghd_tooltip("Show who last changed each line")
            .child("Blame")
            .on_click(move |_, _, cx| {
                if working {
                    Dispatcher::show_blame_for_change(repo, path.clone(), cx)
                } else {
                    Dispatcher::show_blame_for_commit_file(repo, path.clone(), cx)
                }
            }),
    )
}

/// `794-svg-image-diff`: the Text / Image switch of an SVG file's header
/// (one setting for every SVG file).
fn svg_switch(path: &str, cx: &App) -> Option<impl IntoElement + use<>> {
    let s = AppState::try_global(cx)?.read(cx);
    if !corvene_git::is_svg(path) || !s.flags.bool(corvene_core::flags::ids::SVG_IMAGE_DIFF) {
        return None;
    }
    Some(view_switch(
        ["svg-view-switch", "svg-view-text", "svg-view-image"],
        ["Text", "Image"],
        !s.settings.svg_text_diff,
        |image, cx| Dispatcher::set_svg_image_diff(image, cx),
        cx,
    ))
}

/// `1330-font-source-preview`: the Text / Glyphs switch of a font source's
/// header (one setting for every such file, like the SVG switch).
fn font_source_switch(path: &str, cx: &App) -> Option<impl IntoElement + use<>> {
    let s = AppState::try_global(cx)?.read(cx);
    if corvene_models::font_source_kind(path).is_none()
        || !s.flags.bool(corvene_core::flags::ids::FONT_SOURCE_PREVIEW)
    {
        return None;
    }
    Some(view_switch(
        [
            "font-source-switch",
            "font-source-text",
            "font-source-glyphs",
        ],
        ["Text", "Glyphs"],
        !s.settings.font_source_text_diff,
        |glyphs, cx| Dispatcher::set_font_source_glyphs(glyphs, cx),
        cx,
    ))
}

/// `1318-csv-table-diff`: the Text / Table switch of a CSV or TSV file's
/// header (one setting for every such file).
pub(crate) fn csv_switch(path: &str, cx: &App) -> Option<impl IntoElement + use<>> {
    let s = AppState::try_global(cx)?.read(cx);
    if corvene_core::csv_diff::table_delimiter(path).is_none()
        || !s.flags.bool(corvene_core::flags::ids::CSV_TABLE_DIFF)
    {
        return None;
    }
    Some(view_switch(
        ["csv-view-switch", "csv-view-text", "csv-view-table"],
        ["Text", "Table"],
        s.settings.csv_table_diff,
        |table, cx| Dispatcher::set_csv_table_diff(table, cx),
        cx,
    ))
}

/// `1322-pdf-diff`: the Pages / Text switch of a PDF file's header (one
/// setting for every PDF file), when its diff shows pages.
fn pdf_switch(path: &str, view: &Entity<DiffView>, cx: &App) -> Option<impl IntoElement + use<>> {
    let s = AppState::try_global(cx)?.read(cx);
    if !corvene_git::is_pdf(path) || !s.flags.bool(corvene_core::flags::ids::PDF_DIFF) {
        return None;
    }
    let v = view.read(cx);
    let pages = v
        .pdf
        .as_ref()
        .is_some_and(|p| p.read(cx).failed().is_none());
    if !pages && v.pdf_text.is_none() {
        return None;
    }
    Some(view_switch(
        ["pdf-view-switch", "pdf-view-pages", "pdf-view-text"],
        ["Pages", "Text"],
        s.settings.pdf_text_diff,
        |text, cx| Dispatcher::set_pdf_text_diff(text, cx),
        cx,
    ))
}

/// `1324-document-diff`: the Rendered / Text (/ Pages) switch of a
/// document file's header (one setting for every document), while its diff
/// is a document diff.
fn document_switch(view: &Entity<DiffView>, cx: &App) -> Option<impl IntoElement + use<>> {
    use corvene_core::DocumentDiffView as V;
    let s = AppState::try_global(cx)?.read(cx);
    view.read(cx).document_source.as_ref()?;
    if !s.flags.bool(corvene_core::flags::ids::DOCUMENT_DIFF) {
        return None;
    }
    let pages = document_pages_available(s);
    let mut views = vec![(V::Rendered, "Rendered", "document-view-rendered")];
    views.push((V::Text, "Text", "document-view-text"));
    if pages {
        views.push((V::Pages, "Pages", "document-view-pages"));
    }
    let current = effective_document_view(s);
    let selected = views
        .iter()
        .position(|(v, _, _)| *v == current)
        .unwrap_or(0);
    let ids: Vec<&'static str> = views.iter().map(|(_, _, id)| *id).collect();
    let labels: Vec<&'static str> = views.iter().map(|(_, l, _)| *l).collect();
    let values: Vec<V> = views.iter().map(|(v, _, _)| *v).collect();
    Some(view_switch_n(
        "document-view-switch",
        &ids,
        &labels,
        selected,
        move |ix, cx| Dispatcher::set_document_diff_view(values[ix], cx),
        cx,
    ))
}

/// `1325-document-pages`: the flag, the PDF page view it shows the pages
/// with, and LibreOffice to draw them.
fn document_pages_available(s: &AppState) -> bool {
    s.flags.bool(corvene_core::flags::ids::DOCUMENT_PAGES)
        && s.flags.bool(corvene_core::flags::ids::PDF_DIFF)
        && corvene_core::document_pages::soffice().is_some()
}

/// `1324-document-diff`: the document view to show (Pages falls back to
/// Rendered when it is not available).
fn effective_document_view(s: &AppState) -> corvene_core::DocumentDiffView {
    use corvene_core::DocumentDiffView as V;
    match s.settings.document_diff_view {
        V::Pages if !document_pages_available(s) => V::Rendered,
        v => v,
    }
}

/// A two-segment switch in the diff header (`[container, left, right]`
/// ids); `on_pick(true)` picks the right segment.
pub(crate) fn view_switch(
    ids: [&'static str; 3],
    labels: [&'static str; 2],
    right: bool,
    on_pick: impl Fn(bool, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    view_switch_n(
        ids[0],
        &ids[1..],
        &labels,
        usize::from(right),
        move |ix, cx| on_pick(ix == 1, cx),
        cx,
    )
}

/// A switch of `labels.len()` segments in the diff header; `on_pick` gets
/// the picked segment's index.
fn view_switch_n(
    id: &'static str,
    ids: &[&'static str],
    labels: &[&'static str],
    selected_ix: usize,
    on_pick: impl Fn(usize, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.ghd();
    let on_pick = Rc::new(on_pick);
    let segment = |id: &'static str, label: &'static str, value: usize| {
        let selected = value == selected_ix;
        let (bg, hover) = (t.box_selected_active_background, t.box_hover_background);
        let on_pick = on_pick.clone();
        div()
            .id(id)
            .a11y_button(label)
            .h_full()
            .px(SPACING_HALF())
            .flex()
            .items_center()
            .when(selected, |d| {
                d.bg(bg).text_color(t.box_selected_active_text)
            })
            .when(!selected, |d| {
                d.cursor_pointer()
                    .text_color(t.text)
                    .hover(move |d| d.bg(hover))
                    .on_click(move |_, _, cx| on_pick(value, cx))
            })
            .child(label)
    };
    div()
        .id(id)
        .flex_none()
        .ml(SPACING())
        .h(zpx(19.))
        .flex()
        .flex_row()
        .overflow_hidden()
        .rounded(BORDER_RADIUS())
        .border_1()
        .border_color(t.box_border)
        .text_size(FONT_SIZE_SM())
        .children(
            ids.iter()
                .zip(labels)
                .enumerate()
                .map(|(ix, (id, label))| segment(id, label, ix)),
        )
}

/// `DiffOptions`' gear (`.diff-options-component > button`); opens `view`'s
/// Diff Settings popover under it.
pub fn diff_options_button(view: &Entity<DiffView>, cx: &App) -> impl IntoElement {
    let t = cx.ghd();
    let gear_bounds = view.read(cx).gear_bounds.clone();
    let view = view.clone();
    let hover = t.text_secondary;
    let text = t.text;
    div()
        .id("diff-options-button")
        .group("diff-options-button")
        .icon_button_label(mac_or("Diff Settings", "Diff Options"))
        .relative()
        .h(zpx(19.))
        .flex()
        .flex_row()
        .items_center()
        .cursor_pointer()
        .child(
            canvas(move |bounds, _, _| gear_bounds.set(bounds), |_, _, _, _| {})
                .absolute()
                .inset_0(),
        )
        // gear + ▾ in `--text-color`, `--text-secondary-color` on hover
        .child(
            octicon(Octicon::Gear, text)
                .group_hover("diff-options-button", move |s| s.text_color(hover)),
        )
        .child(
            octicon(Octicon::TriangleDown, text)
                .group_hover("diff-options-button", move |s| s.text_color(hover)),
        )
        .on_click(move |_, _, cx| {
            view.update(cx, |this, cx| this.toggle_options(cx));
        })
}

/// How long a diff loads before the spinner covers the pane.
const LOADING_INDICATOR_DELAY: std::time::Duration = std::time::Duration::from_millis(300);

/// Which diff of the repository state the view shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffSource {
    /// Changes tab: the selected working-directory file (selectable lines).
    WorkingDirectory,
    /// History tab: the selected file of the selected commit (read-only).
    Commit,
    /// Stash viewer: the selected stashed file (read-only).
    Stash,
    /// Preview Pull Request dialog: the selected file's merge-base diff.
    PullRequest,
    /// `348-pull-request-review`: the reviewed pull request's selected
    /// file, with its review threads under the lines.
    Review,
    /// `1218-compare-refs`: the selected file of two refs' combined diff.
    RefCompare,
    /// `1323-archive-diff`: the selected entry of the archive open on a
    /// surface (read-only).
    ArchiveEntry(ArchiveSurface),
}

impl DiffSource {
    /// `1323-archive-diff`: the surface an archive shown by this source's
    /// view belongs to.
    pub fn archive_surface(self) -> ArchiveSurface {
        match self {
            DiffSource::WorkingDirectory => ArchiveSurface::Changes,
            DiffSource::Commit => ArchiveSurface::History,
            DiffSource::Stash => ArchiveSurface::Stash,
            DiffSource::PullRequest => ArchiveSurface::PullRequest,
            DiffSource::Review => ArchiveSurface::Review,
            DiffSource::RefCompare => ArchiveSurface::RefCompare,
            DiffSource::ArchiveEntry(surface) => surface,
        }
    }
}

/// Which "hide whitespace" setting a source uses.
fn set_hide_whitespace(source: DiffSource, hide: bool, cx: &mut App) {
    match source {
        // `1323-archive-diff`: the setting of the archive's own diff
        DiffSource::ArchiveEntry(surface) => {
            let outer = match surface {
                ArchiveSurface::Changes => DiffSource::WorkingDirectory,
                ArchiveSurface::History => DiffSource::Commit,
                ArchiveSurface::Stash => DiffSource::Stash,
                ArchiveSurface::PullRequest => DiffSource::PullRequest,
                ArchiveSurface::Review => DiffSource::Review,
                ArchiveSurface::RefCompare => DiffSource::RefCompare,
            };
            set_hide_whitespace(outer, hide, cx)
        }
        DiffSource::WorkingDirectory => Dispatcher::set_hide_whitespace_in_diff(false, hide, cx),
        DiffSource::Commit | DiffSource::Stash => {
            Dispatcher::set_hide_whitespace_in_diff(true, hide, cx)
        }
        DiffSource::PullRequest | DiffSource::RefCompare => {
            Dispatcher::set_hide_whitespace_in_pull_request_diff(hide, cx)
        }
        DiffSource::Review => {
            Dispatcher::set_hide_whitespace_in_diff(true, hide, cx);
            // the file's diff is read with the new setting
            let reselect = AppState::try_global(cx).and_then(|s| {
                let s = s.read(cx);
                let id = s.selected?;
                let review = crate::review_threads::review_state_of(s, id)?;
                Some((id, review.selected_file()?.to_string()))
            });
            if let Some((id, path)) = reselect {
                Dispatcher::select_review_file(id, path, cx);
            }
        }
    }
}

/// One render's snapshot of the repository state.
/// `1316-diff-moved-lines` / `1317-diff-stylistic-changes`: one mark per
/// unified row.
type LineMarks = Rc<Vec<Option<LineMark>>>;

/// [`LineMarks`] with the rows (kept alive, so their address is not reused)
/// and options they were computed for.
struct MarksCache {
    rows: Rc<Vec<Row>>,
    options: ClassOptions,
    marks: LineMarks,
}

/// `1318-csv-table-diff`: the diff key, both sides' contents (kept alive),
/// the text size and zoom the column widths were measured at, and for a
/// `1321-table-file-diff` table file its diff and the sheet shown.
type CsvKey = (
    (u64, String, u64),
    Option<Arc<Vec<String>>>,
    Option<Arc<Vec<String>>>,
    (u32, u32),
    Option<(Arc<Diff>, String)>,
);

/// `1322-pdf-diff`: the text of a PDF file's two versions, as a diff.
enum PdfText {
    Reading,
    Ready(Arc<Diff>),
    /// A version did not open or had too much text.
    Unreadable,
}

/// `1322-pdf-diff`: flipped into the diff generation of a PDF file's text
/// diff, so its rows are built apart from the page diff's.
const PDF_TEXT_GENERATION: u64 = 1 << 63;

/// `1324-document-diff`: flipped into the diff generation of a document's
/// text and page diffs.
const DOCUMENT_TEXT_GENERATION: u64 = 1 << 62;
const DOCUMENT_PAGES_GENERATION: u64 = 1 << 61;

/// `1325-document-pages`: a document's pages as drawn by LibreOffice.
enum DocumentPages {
    Converting,
    Ready(Arc<Diff>),
    Failed(String),
}

/// `1322-pdf-diff`: a page diff of a PDF file.
fn is_pdf_diff(diff: &Diff) -> bool {
    match diff {
        Diff::Image { previous, current } => [previous, current]
            .into_iter()
            .flatten()
            .any(|b| b.media_type == corvene_git::PDF_MEDIA_TYPE),
        _ => false,
    }
}

struct Snapshot {
    repo: u64,
    repo_path: std::path::PathBuf,
    path: String,
    kind: FileStatusKind,
    /// GHD `renameIncludesModifications` of a renamed file.
    rename_includes_modifications: bool,
    selection: DiffSelection,
    diff: Arc<Diff>,
    contents: Option<Arc<Vec<String>>>,
    /// Old-side lines for highlighting (`fileContents.oldContents`).
    old_contents: Option<Arc<Vec<String>>>,
    key: (u64, String, u64),
    hide_whitespace: bool,
    confirm_discard: bool,
    /// `749-binary-diff-as-text`: a binary file shown with `--text`.
    as_text: bool,
    /// `1324-document-diff`: the document diff `diff` was made from (the
    /// Text and Pages views replace it).
    document_source: Option<Arc<Diff>>,
    /// `1331-notebook-diff` / `1332-latex-diff`: the notebook or LaTeX diff
    /// `diff` was made from (the JSON, Script, Text and Pages views
    /// replace it).
    rich_source: Option<Arc<Diff>>,
}

/// A position in the diff's text: a list row (unified or split index) and a
/// byte offset into that row's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextPos {
    pub row: usize,
    pub col: usize,
}

/// GHD's browser text selection over the rows (`textSelectionStartRow` /
/// `selectingTextInRow`): the anchor is where the mouse went down, the head
/// follows the drag; in split mode the selection lives in one column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextSelection {
    pub anchor: TextPos,
    pub head: TextPos,
    pub column: Column,
    pub dragging: bool,
}

/// The selection with `start <= end`, as the rows paint it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextSelectionSnapshot {
    pub start: TextPos,
    pub end: TextPos,
    pub column: Column,
}

impl TextSelection {
    fn snapshot(&self) -> TextSelectionSnapshot {
        TextSelectionSnapshot {
            start: self.anchor.min(self.head),
            end: self.anchor.max(self.head),
            column: self.column,
        }
    }

    fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

pub struct DiffView {
    state: Entity<AppState>,
    source: DiffSource,
    /// `1311-stacked-diff`: this view is one file of a stacked list and
    /// reads that file's entry of `RepositoryState::stacked` instead of
    /// the tab's selected file.
    stacked_path: Option<String>,
    /// `1311-stacked-diff`: whose diffs the stack reads, set by the
    /// stacked list each frame (computing it per file would repeat the
    /// commit selection's ordering).
    stacked_kind: Option<Rc<corvene_core::stacked_diff::StackKind>>,
    temp: Option<TempSelection>,
    hovered_group: Option<u32>,
    /// Text selection over the rows (see `TextSelection`).
    text_selection: Option<TextSelection>,
    /// Screen bounds of the rendered rows' text, for the selection drag.
    text_bounds: TextBounds,
    /// The zoom factor the list's row heights were measured at.
    zoom_seen: f32,
    /// `758-diff-line-height` as last rendered.
    line_height_seen: f32,
    /// The rows' font size (`751-diff-font-size`; GHD's 11 px otherwise).
    text_size: Pixels,
    list_state: ListState,
    rows: Rc<Vec<Row>>,
    /// (repo, path, diff generation) the cached rows were built from.
    rows_key: Option<(u64, String, u64)>,
    /// Syntax spans for `rows` (see `highlight`).
    tokens: Option<Rc<Vec<Vec<corvene_highlight::Span>>>>,
    /// The rows `tokens` belonged to before a reload of the same file, for
    /// `carry_over`.
    previous_rows: Option<Rc<Vec<Row>>>,
    /// The highlighter (and tree-sitter grammar set) `tokens` came from;
    /// a change (Settings, a grammar pack loaded) highlights again.
    highlighted_with: Option<(corvene_highlight::Engine, u64)>,
    /// The hunks as shown (expanded copies of the model's).
    hunks: Rc<Vec<XHunk>>,
    /// Side-by-side rows built from `rows` (`showSideBySideDiff`).
    split_rows: Rc<Vec<SplitRow>>,
    unified_to_split: Rc<Vec<usize>>,
    /// Intra-line change ranges of the unified rows.
    unified_inner: Rc<Vec<Vec<std::ops::Range<usize>>>>,
    /// `1316-diff-moved-lines` / `1317-diff-stylistic-changes`: the marks
    /// of the unified rows, with the rows (kept alive, so their address is
    /// not reused) and options they were computed for.
    line_marks: Option<MarksCache>,
    /// `1318-csv-table-diff`: the table of a CSV / TSV diff, keyed by the
    /// diff and both sides' contents (`Err`: why it is shown as text).
    csv: Option<(CsvKey, Result<Rc<CsvTable>, String>)>,
    /// `1321-table-file-diff`: the sheet picked in a table file's diff (kept
    /// while it exists, so stepping through files stays on one sheet) and
    /// the current table diff's sheets.
    csv_sheet: Option<String>,
    csv_sheets: Option<(Arc<Diff>, Rc<Vec<SheetEntry>>)>,
    csv_list: ListState,
    csv_h_scroll: HScroll,
    /// Whether the list currently shows `split_rows`.
    split_mode: bool,
    /// `1304-diff-no-wrap`: lines stay on one row (as last rendered).
    no_wrap: bool,
    /// `1304-diff-no-wrap`: the sideways scroll the rows share.
    h_scroll: HScroll,
    /// The rows (`Rc` address) and their longest line in characters, for
    /// the width estimate.
    longest_line: (usize, usize),
    /// New-side file lines for expansion (`fileContents.newContents`).
    contents: Option<Arc<Vec<String>>>,
    /// Old-side file lines, highlighted for deleted rows like GHD.
    old_contents: Option<Arc<Vec<String>>>,
    /// GHD `diffToRestore !== null`: "Collapse Expanded Lines" is available.
    expanded: bool,
    /// GHD `forceShowLargeDiff`.
    show_large: bool,
    // ⌘F (`DiffSearchInput`); the input is created on first use (needs a window)
    searching: bool,
    search_input: Option<Entity<InputState>>,
    search_query: String,
    hits: Vec<SearchHit>,
    selected_hit: Option<usize>,
    /// `1305-diff-find-controls` as last rendered, and its Aa toggle.
    find_controls: bool,
    search_case_sensitive: bool,
    /// `1312-stacked-diff-search`: the rows (`Rc` address) `hits` were
    /// searched for the stack's query; `None` when the stack is not
    /// searching.
    stacked_search_rows: Option<usize>,
    // Diff Settings popover (`DiffOptions`)
    options_open: bool,
    pub(crate) gear_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// `WhitespaceHintPopover` anchor.
    whitespace_hint: Option<Point<Pixels>>,
    image: Option<Entity<ImageDiff>>,
    /// `1322-pdf-diff`: the page view of a PDF file's diff.
    pdf: Option<Entity<PdfDiff>>,
    /// `1322-pdf-diff`: the text diff of a PDF file for the header's Pages
    /// / Text switch, with the page diff it was read from.
    pdf_text: Option<(Arc<Diff>, PdfText)>,
    _pdf_text_task: Option<Task<()>>,
    /// `1322-pdf-diff`: renders again when `pdf` finds the file unreadable.
    _pdf_observer: Option<Subscription>,
    /// `1323-archive-diff`: the archive view of an archive's diff.
    archive: Option<Entity<crate::archive_diff_view::ArchiveDiffView>>,
    /// `1329-font-diff` / `1330-font-source-preview`
    font: Option<FontPane>,
    /// `1324-document-diff`: the document diff last shown (any view), its
    /// rendered view, its text diff and (`1325-document-pages`) its pages.
    document_source: Option<Arc<Diff>>,
    document: Option<Entity<DocumentDiff>>,
    document_text: Option<(Arc<Diff>, PdfText)>,
    _document_text_task: Option<Task<()>>,
    document_pages: Option<(Arc<Diff>, DocumentPages)>,
    _document_pages_task: Option<Task<()>>,
    /// `1331-notebook-diff` / `1332-latex-diff`
    rich: rich_text::RichPane,
    context_menu: Option<Entity<ContextMenu>>,
    focus_handle: FocusHandle,
    /// When the working-directory diff started loading, for the spinner.
    loading_since: Option<std::time::Instant>,
    /// `348-pull-request-review`: the view-local state of the threads.
    review: ReviewLocal,
}

/// `1329-font-diff` / `1330-font-source-preview`: the view of a font or
/// font source diff.
#[derive(Clone)]
enum FontPane {
    Font(Entity<FontDiffView>),
    Source(Entity<FontSourceView>),
}

impl FontPane {
    fn source<'a>(&self, cx: &'a App) -> &'a Arc<Diff> {
        match self {
            FontPane::Font(v) => &v.read(cx).source,
            FontPane::Source(v) => &v.read(cx).source,
        }
    }
}

/// The threads and their parsed bodies at one version of the state.
type ReviewThreadsSnapshot = (
    u64,
    Rc<Vec<corvene_core::pull_request_review::ReviewThread>>,
    Rc<HashMap<String, Arc<Vec<corvene_core::markdown::Block>>>>,
);

/// `348-pull-request-review`: what the threads keep in the view: the open
/// reply boxes, the comment being edited, the resolved threads unfolded,
/// the composer's text box, and a snapshot of the threads shared with the
/// rows (rebuilt when the state's version changes).
#[derive(Default)]
struct ReviewLocal {
    reply_boxes: Rc<HashMap<String, Entity<TextareaState>>>,
    editing: Option<(String, Entity<TextareaState>)>,
    expanded: Rc<HashSet<String>>,
    /// The composer's target and text box (the box is remade for a new
    /// target).
    composer: Option<(
        corvene_core::pull_request_review::ComposeTarget,
        Entity<TextareaState>,
    )>,
    /// The outdated / file-level threads panel is open.
    unplaced_open: bool,
    /// (threads version, threads, bodies) as last snapshotted.
    threads: Option<ReviewThreadsSnapshot>,
    /// What the rows were last measured with; a change remeasures them.
    layout_key: Option<String>,
    /// Avatar URLs already asked for.
    avatars: HashSet<String>,
}

impl DiffView {
    /// The view as a cached element filling the rest of a column (Corvene;
    /// GHD's React tree has no equivalent cost): it re-renders only when it
    /// notifies (the app state changed, it scrolled, its selection moved),
    /// not for every frame of the window around it (a caret blink, hovering
    /// the file list, typing a commit message). A diff is most of a frame.
    pub fn embed(view: &Entity<Self>) -> impl IntoElement + use<> {
        view.clone()
            .cached(StyleRefinement::default().flex_1().min_h_0().w_full())
    }

    pub fn new(state: Entity<AppState>, source: DiffSource, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            source,
            stacked_path: None,
            stacked_kind: None,
            temp: None,
            hovered_group: None,
            text_selection: None,
            text_bounds: Rc::new(RefCell::new(HashMap::new())),
            zoom_seen: crate::theme::sizes::zoom_factor(),
            line_height_seen: 20.,
            text_size: FONT_SIZE_SM(),
            list_state: ListState::new(0, ListAlignment::Top, zpx(200.)),
            rows: Rc::new(Vec::new()),
            rows_key: None,
            tokens: None,
            previous_rows: None,
            highlighted_with: None,
            hunks: Rc::new(Vec::new()),
            split_rows: Rc::new(Vec::new()),
            unified_to_split: Rc::new(Vec::new()),
            unified_inner: Rc::new(Vec::new()),
            line_marks: None,
            csv: None,
            csv_sheet: None,
            csv_sheets: None,
            csv_list: ListState::new(0, ListAlignment::Top, zpx(200.)),
            csv_h_scroll: HScroll::default(),
            split_mode: false,
            no_wrap: false,
            h_scroll: HScroll::default(),
            longest_line: (0, 0),
            contents: None,
            old_contents: None,
            expanded: false,
            show_large: false,
            searching: false,
            search_input: None,
            search_query: String::new(),
            hits: Vec::new(),
            selected_hit: None,
            find_controls: false,
            search_case_sensitive: false,
            stacked_search_rows: None,
            options_open: false,
            gear_bounds: Rc::new(Cell::new(Bounds::default())),
            whitespace_hint: None,
            image: None,
            pdf: None,
            pdf_text: None,
            _pdf_text_task: None,
            _pdf_observer: None,
            archive: None,
            font: None,
            document_source: None,
            document: None,
            document_text: None,
            _document_text_task: None,
            document_pages: None,
            _document_pages_task: None,
            rich: rich_text::RichPane::default(),
            context_menu: None,
            focus_handle: cx.focus_handle().tab_stop(true),
            loading_since: None,
            review: ReviewLocal::default(),
        }
    }

    /// `1311-stacked-diff`: the view for one file of a stacked list
    /// (`source` is the tab's: `WorkingDirectory` or `Commit`).
    pub fn new_stacked(
        state: Entity<AppState>,
        source: DiffSource,
        path: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self::new(state, source, cx);
        view.stacked_path = Some(path);
        view
    }

    /// The rows' font size as last rendered (`751-diff-font-size`).
    pub(crate) fn text_size(&self) -> Pixels {
        self.text_size
    }

    /// `740-diff-loading-indicator`: a spinner over the pane once the
    /// working-directory diff has been loading for a moment.
    fn loading_overlay(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let loading = self.source == DiffSource::WorkingDirectory && {
            let s = self.state.read(cx);
            s.flags
                .bool(corvene_core::flags::ids::DIFF_LOADING_INDICATOR)
                && s.selected_state()
                    .is_some_and(|rs| rs.diff_loading && rs.selected_file.is_some())
        };
        if !loading {
            self.loading_since = None;
            return None;
        }
        let since = *self.loading_since.get_or_insert_with(|| {
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(LOADING_INDICATOR_DELAY)
                    .await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
            std::time::Instant::now()
        });
        if since.elapsed() < LOADING_INDICATOR_DELAY {
            return None;
        }
        let t = cx.ghd();
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(t.background)
                .child(crate::icons::loading("diff-loading", t.text_secondary))
                .into_any_element(),
        )
    }

    fn snapshot(&self, cx: &App) -> Option<Snapshot> {
        let s = self.state.read(cx);
        let id = s.selected?;
        let rs = s.repo_states.get(&id)?;
        let repo_path = s.repository(id)?.path.clone();
        if let Some(path) = self.stacked_path.as_ref() {
            return self.stacked_snapshot(s, id, rs, repo_path, path);
        }
        let (
            path,
            (kind, rename_includes_modifications),
            selection,
            diff,
            generation,
            (contents, old_contents),
            hide_whitespace,
        ) = match self.source {
            DiffSource::WorkingDirectory => {
                let file = rs.selected_file.as_ref().and_then(|p| {
                    rs.status
                        .as_deref()
                        .and_then(|st| st.files.iter().find(|f| &f.path == p))
                })?;
                (
                    file.path.clone(),
                    (
                        file.status.kind,
                        file.status.rename_includes_modifications(),
                    ),
                    file.selection.clone(),
                    rs.diff.clone()?,
                    rs.diff_generation,
                    (rs.diff_contents.clone(), rs.diff_old_contents.clone()),
                    s.settings.hide_whitespace_in_changes_diff,
                )
            }
            DiffSource::Commit => {
                let file = rs.commit_selected_file.as_ref().and_then(|p| {
                    rs.changeset
                        .as_ref()
                        .and_then(|c| c.files.iter().find(|f| &f.path == p))
                })?;
                (
                    file.path.clone(),
                    (
                        file.status.kind,
                        file.status.rename_includes_modifications(),
                    ),
                    DiffSelection::all(),
                    rs.commit_diff.clone()?,
                    rs.commit_diff_generation,
                    (
                        rs.commit_diff_contents.clone(),
                        rs.commit_diff_old_contents.clone(),
                    ),
                    s.settings.hide_whitespace_in_history_diff,
                )
            }
            DiffSource::Stash => {
                let file = rs.stash_selected_file.as_ref().and_then(|p| {
                    rs.stash_files
                        .as_ref()
                        .and_then(|files| files.iter().find(|f| &f.path == p))
                })?;
                (
                    file.path.clone(),
                    (
                        file.status.kind,
                        file.status.rename_includes_modifications(),
                    ),
                    DiffSelection::all(),
                    rs.stash_diff.clone()?,
                    rs.stash_diff_generation,
                    (
                        rs.stash_diff_contents.clone(),
                        rs.stash_diff_old_contents.clone(),
                    ),
                    s.settings.hide_whitespace_in_history_diff,
                )
            }
            DiffSource::Review => {
                let review = corvene_core::pull_request_review::review_of(s, rs)?;
                let path = review.selected_file()?.to_string();
                let file = review.file.as_ref().filter(|f| f.path == path)?;
                let change = review
                    .changeset
                    .as_ref()
                    .and_then(|c| c.files.iter().find(|f| f.path == path))?;
                (
                    path,
                    (
                        change.status.kind,
                        change.status.rename_includes_modifications(),
                    ),
                    DiffSelection::all(),
                    file.diff.clone()?,
                    file.diff_generation,
                    (file.diff_contents.clone(), file.diff_old_contents.clone()),
                    s.settings.hide_whitespace_in_history_diff,
                )
            }
            DiffSource::ArchiveEntry(surface) => {
                let browse = rs.archives.open.get(&surface)?;
                let entry = browse.entry.as_ref()?;
                let change = browse.level().diff.entry(&entry.path)?;
                let kind = archive_entry_kind(change.status);
                (
                    entry.path.clone(),
                    (kind, false),
                    DiffSelection::all(),
                    entry.diff.clone(),
                    entry.generation,
                    (entry.contents.clone(), entry.old_contents.clone()),
                    surface.hide_whitespace(s),
                )
            }
            DiffSource::PullRequest | DiffSource::RefCompare => {
                let preview = if self.source == DiffSource::RefCompare {
                    rs.ref_compare_changes.as_ref()?
                } else {
                    rs.pull_request_preview.as_ref()?
                };
                let file = preview.file.as_ref().and_then(|p| {
                    preview
                        .changeset
                        .as_ref()
                        .and_then(|c| c.files.iter().find(|f| &f.path == p))
                })?;
                (
                    file.path.clone(),
                    (
                        file.status.kind,
                        file.status.rename_includes_modifications(),
                    ),
                    DiffSelection::all(),
                    preview.diff.clone()?,
                    preview.diff_generation,
                    (preview.diff_contents.clone(), None),
                    s.settings.hide_whitespace_in_pull_request_diff,
                )
            }
        };
        let as_text = self.source == DiffSource::WorkingDirectory
            && rs.diff_as_text.as_deref() == Some(path.as_str());
        // `1322-pdf-diff`: the Text side of a PDF file's Pages / Text switch
        let (diff, generation, contents, old_contents) = match self.pdf_text_of(&diff, cx) {
            Some(text) => (text, generation ^ PDF_TEXT_GENERATION, None, None),
            None => (diff, generation, contents, old_contents),
        };
        // `1324-document-diff`: the Text or Pages view of a document
        let document_source = matches!(*diff, Diff::Document { .. }).then(|| diff.clone());
        let (diff, generation, contents, old_contents) = match self.document_view_of(&diff, cx) {
            Some((view, flip)) => (view, generation ^ flip, None, None),
            None => (diff, generation, contents, old_contents),
        };
        // `1331-notebook-diff` / `1332-latex-diff`: the JSON / Script / Text
        // / Pages view of a notebook or LaTeX file
        let rich_source = rich_text::is_rich(&diff).then(|| diff.clone());
        let (diff, generation, contents, old_contents) = match self.rich_view_of(&diff, cx) {
            Some((view, flip, true)) => (view, generation ^ flip, contents, old_contents),
            Some((view, flip, false)) => (view, generation ^ flip, None, None),
            None => (diff, generation, contents, old_contents),
        };
        // `1323-archive-diff`: rows are keyed by the entry's full name, so
        // the same path in another archive (or level) is another diff
        let key_path = match self.source {
            DiffSource::ArchiveEntry(surface) => rs
                .archives
                .open
                .get(&surface)
                .map_or_else(|| path.clone(), |b| b.display_path(&path)),
            _ => path.clone(),
        };
        Some(Snapshot {
            repo: id,
            repo_path,
            key: (id, key_path, generation),
            path,
            kind,
            rename_includes_modifications,
            selection,
            diff,
            contents,
            old_contents,
            hide_whitespace,
            confirm_discard: s.settings.confirm_discard_changes,
            as_text,
            document_source,
            rich_source,
        })
    }

    /// `1311-stacked-diff`: the snapshot of one file of a stack, from its
    /// entry in `RepositoryState::stacked` (none until it has loaded).
    fn stacked_snapshot(
        &self,
        s: &AppState,
        id: u64,
        rs: &corvene_core::RepositoryState,
        repo_path: std::path::PathBuf,
        path: &str,
    ) -> Option<Snapshot> {
        let stack = self.stacked_kind.clone()?;
        // the entry first: most files of a big stack have none yet, and
        // the file lookups below walk the status / changeset
        let entry = corvene_core::stacked_diff::stacked_entry(rs, &stack, path)?;
        let ((kind, rename_includes_modifications), selection, hide_whitespace) = match self.source
        {
            DiffSource::ArchiveEntry(surface) => {
                let change = rs.archives.open.get(&surface)?.level().diff.entry(path)?;
                (
                    (archive_entry_kind(change.status), false),
                    DiffSelection::all(),
                    surface.hide_whitespace(s),
                )
            }
            DiffSource::WorkingDirectory => {
                let file = rs
                    .status
                    .as_deref()
                    .and_then(|st| st.files.iter().find(|f| f.path == path))?;
                (
                    (
                        file.status.kind,
                        file.status.rename_includes_modifications(),
                    ),
                    file.selection.clone(),
                    s.settings.hide_whitespace_in_changes_diff,
                )
            }
            DiffSource::Commit => {
                let file = rs
                    .changeset
                    .as_ref()
                    .and_then(|c| c.files.iter().find(|f| f.path == path))?;
                (
                    (
                        file.status.kind,
                        file.status.rename_includes_modifications(),
                    ),
                    DiffSelection::all(),
                    s.settings.hide_whitespace_in_history_diff,
                )
            }
            _ => return None,
        };
        let as_text =
            self.source == DiffSource::WorkingDirectory && rs.diff_as_text.as_deref() == Some(path);
        // `1323-archive-diff`: keyed by the entry's full name
        let key_path = match self.source {
            DiffSource::ArchiveEntry(surface) => rs
                .archives
                .open
                .get(&surface)
                .map_or_else(|| path.to_string(), |b| b.display_path(path)),
            _ => path.to_string(),
        };
        Some(Snapshot {
            repo: id,
            repo_path,
            key: (id, key_path, entry.generation),
            path: path.to_string(),
            kind,
            rename_includes_modifications,
            selection,
            diff: entry.diff.clone(),
            contents: entry.contents.clone(),
            old_contents: entry.old_contents.clone(),
            hide_whitespace,
            confirm_discard: s.settings.confirm_discard_changes,
            as_text,
            document_source: None,
            rich_source: None,
        })
    }

    /// Tokenize the rows (GHD: highlighter web worker).
    ///
    /// Like GHD (`highlightContents` + `SideBySideDiff.createFullRow`), the
    /// whole old and new files are tokenized, so parser state comes from the
    /// real file; deleted rows take old-file tokens, added rows new-file
    /// tokens and context rows old-file tokens unless the diff only adds
    /// lines (`getLineFilters`). Without file contents the rows themselves
    /// are tokenized in order.
    ///
    /// Deviation (.docs/deviations.md): GHD always highlights in its
    /// worker, so a diff first shows uncoloured. Here a side already
    /// tokenized (`SIDE_CACHE`: revisiting a file, or a reload that changed
    /// one side) is reused, and with GHD's tokenizers the rows on screen are
    /// coloured right here from each file's first lines (`preview`, exactly
    /// the whole run's tokens for them), so the first frame is coloured; a
    /// background task tokenizes the whole files for the rest. With
    /// tree-sitter, or when the preview would be too big, rows whose text did
    /// not change keep their previous colours until the background is done.
    fn highlight(&mut self, key: (u64, String, u64), cx: &mut Context<Self>) {
        let previous = self.tokens.take().zip(self.previous_rows.take());
        let with = highlight_engine(self.state.read(cx));
        let context_side = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::WHITESPACE_HIDDEN_HIGHLIGHT);
        self.highlighted_with = Some(with);
        let path = key.1.clone();
        let rows: Vec<RowSource> = self
            .rows
            .iter()
            .map(|r| (r.kind, r.old, r.new, r.text.clone()))
            .collect();
        let old = self
            .old_contents
            .clone()
            .map(|lines| Side::new(with, &path, lines));
        let new = self
            .contents
            .clone()
            .map(|lines| Side::new(with, &path, lines));
        let pending_bytes: usize = if old.is_none() && new.is_none() {
            rows.iter().map(|r| r.3.len() + 1).sum()
        } else {
            [&old, &new]
                .into_iter()
                .flatten()
                .filter(|side| side.tokens.is_none())
                .map(|side| side.bytes)
                .sum()
        };
        let first_line = [&new, &old]
            .into_iter()
            .flatten()
            .find_map(|side| side.lines.first().map(String::as_str))
            .unwrap_or_default();
        if pending_bytes == 0 {
            let (tokens, sides) = assemble(with.0, &path, &rows, old, new, context_side);
            remember_sides(sides);
            self.tokens = tokens.map(Rc::new);
            return;
        }
        let in_process = tokenizes_in_process(with.0, &path, first_line);
        let (mut old, mut new) = (old, new);
        let preview = in_process
            .then(|| {
                preview(
                    with.0,
                    &path,
                    &rows,
                    self.preview_window(),
                    &old,
                    &new,
                    context_side,
                )
            })
            .flatten();
        match preview {
            Some(preview) => {
                // sides the preview tokenized whole are final
                for (key, tokens) in &preview.whole {
                    for side in [&mut old, &mut new].into_iter().flatten() {
                        if side.key == *key {
                            side.tokens = Some(tokens.clone());
                        }
                    }
                }
                remember_sides(preview.whole);
                self.tokens = preview.tokens.map(Rc::new);
                if preview.complete {
                    return;
                }
            }
            None => {
                self.tokens = previous.map(|(tokens, previous_rows)| {
                    Rc::new(carry_over(
                        &tokens,
                        previous_rows.iter().map(|r| (r.kind, r.text.as_str())),
                        self.rows.iter().map(|r| (r.kind, r.text.as_str())),
                    ))
                });
            }
        }
        let generation = self.rows.len();
        let engine = with.0;
        // the two sides in parallel; the rows are cheap to assemble after
        let tokenize = |side: &Option<Side>| {
            let side = side.as_ref().filter(|side| side.tokens.is_none())?;
            let (lines, path) = (side.lines.clone(), path.clone());
            Some(
                cx.background_executor()
                    .spawn(async move { Arc::new(tokenize_side(engine, &path, &lines)) }),
            )
        };
        let (old_task, new_task) = (tokenize(&old), tokenize(&new));
        let rows_task = (old.is_none() && new.is_none()).then(|| {
            let (path, rows) = (path.clone(), rows.clone());
            cx.background_executor()
                .spawn(async move { assemble(engine, &path, &rows, None, None, context_side).0 })
        });
        cx.spawn(async move |this, cx| {
            let result = match rows_task {
                Some(task) => task.await,
                None => {
                    let (mut old, mut new) = (old, new);
                    let mut fresh = Vec::new();
                    for (side, task) in [(&mut old, old_task), (&mut new, new_task)] {
                        if let (Some(side), Some(task)) = (side.as_mut(), task) {
                            let tokens = task.await;
                            fresh.push((side.key.clone(), tokens.clone()));
                            side.tokens = Some(tokens);
                        }
                    }
                    remember_sides(fresh);
                    assemble(engine, &path, &rows, old, new, context_side).0
                }
            };
            this.update(cx, |this, cx| {
                if this.rows_key.as_ref() == Some(&key) && this.rows.len() == generation {
                    this.tokens = result.map(Rc::new);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// The unified rows on screen first: `PREVIEW_ROWS` from the top one.
    fn preview_window(&self) -> std::ops::Range<usize> {
        let top = self.list_state.logical_scroll_top().item_ix;
        let top = if self.split_mode {
            self.split_rows
                .get(top)
                .and_then(|row| {
                    let (before, after) = row.unified_rows();
                    before.into_iter().chain(after).min()
                })
                .unwrap_or(0)
        } else {
            top
        };
        let top = top.min(self.rows.len());
        top..(top + PREVIEW_ROWS).min(self.rows.len())
    }

    /// Rebuild rows after the hunks changed (new diff or expansion).
    fn rebuild_rows(&mut self, key: (u64, String, u64), cx: &mut Context<Self>) {
        let old_len = self.row_count();
        self.previous_rows = Some(self.rows.clone());
        self.rows = Rc::new(build_rows(&self.hunks));
        self.rebuild_split_rows(cx);
        self.rows_key = Some(key.clone());
        self.text_selection = None;
        self.list_state.splice(0..old_len, self.row_count());
        // Unmeasured rows count as 0 px, which grows the content (and shrinks
        // the scrollbar thumb) as rows get rendered; hint one line each.
        self.list_state = self
            .list_state
            .clone()
            .with_uniform_item_height(DIFF_LINE_HEIGHT());
        self.refresh_search();
        self.highlight(key, cx);
    }

    fn rebuild_split_rows(&mut self, cx: &App) {
        let flags = &self.state.read(cx).flags;
        let options = IntraLineOptions {
            graphemes: flags.bool(corvene_core::flags::ids::INTRA_LINE_GRAPHEMES),
            max_len: match flags.number(corvene_core::flags::ids::INTRA_LINE_MAX_LENGTH) {
                0 => None,
                n => Some(n as usize),
            },
            words: flags.bool(corvene_core::flags::ids::WORD_INTRA_LINE_DIFF),
        };
        let split = build_split_rows(&self.rows, options);
        self.unified_to_split = Rc::new(unified_to_split(&split, self.rows.len()));
        self.unified_inner = Rc::new(unified_inner(&split, self.rows.len()));
        self.split_rows = Rc::new(split);
    }

    /// Rows in the list for the current display mode.
    fn row_count(&self) -> usize {
        if self.split_mode {
            self.split_rows.len()
        } else {
            self.rows.len()
        }
    }

    /// Corvene (`612-navigation-shortcuts`): ⌘3 puts keyboard focus here.
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
    }

    /// The diff's keyboard focus (`619-arrow-keys-between-panes`).
    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    /// Diff Settings › Diff display changed: swap the row set.
    fn set_split_mode(&mut self, split: bool) {
        if self.split_mode != split {
            self.split_mode = split;
            self.text_selection = None;
            self.list_state
                .reset_with_uniform_height(self.row_count(), DIFF_LINE_HEIGHT());
        }
    }

    /// A different diff arrived: start over (`componentDidUpdate` in GHD).
    fn load(&mut self, snap: &Snapshot, cx: &mut Context<Self>) {
        self.temp = None;
        self.hovered_group = None;
        self.expanded = false;
        self.show_large = false;
        self.whitespace_hint = None;
        // `757-typechange-diff`: the two sides are different kinds of file,
        // nothing to expand from (rows are highlighted on their own)
        let whole_files = !self.locked_type_change(&snap.diff, cx);
        self.contents = snap.contents.clone().filter(|_| whole_files);
        self.old_contents = snap.old_contents.clone().filter(|_| whole_files);
        self.hunks = Rc::new(match snap.diff.hunks() {
            Some(hunks) => from_hunks(hunks, self.contents.as_ref().map(|c| c.len())),
            None => Vec::new(),
        });
        // `750-diff-expand-whole-file`: start expanded, like "Expand Whole
        // File" (not for large diffs or files)
        if matches!(*snap.diff, Diff::Text { .. })
            && self
                .state
                .read(cx)
                .flags
                .bool(corvene_core::flags::ids::DIFF_EXPAND_WHOLE_FILE)
            && let Some(contents) = self
                .contents
                .clone()
                .filter(|c| !c.is_empty() && c.len() <= MAX_AUTO_EXPAND_LINES)
            && let Some(hunks) = expand_whole(self.hunks.as_ref().clone(), &contents)
        {
            self.hunks = Rc::new(hunks);
            self.expanded = true;
        }
        self.image = None;
        self.pdf = None;
        self._pdf_observer = None;
        if !is_pdf_diff(&snap.diff) && !snap.diff.warnings().is_some_and(|w| w.pdf_text) {
            self.pdf_text = None;
            self._pdf_text_task = None;
        }
        match &*snap.diff {
            // `1322-pdf-diff`
            Diff::Image { previous, current } if is_pdf_diff(&snap.diff) => {
                let (previous, current, kind) = (previous.clone(), current.clone(), snap.kind);
                let pdf = cx.new(|cx| PdfDiff::new(previous.as_ref(), current.as_ref(), kind, cx));
                // a file that does not open shows the binary panel
                self._pdf_observer = Some(cx.observe(&pdf, |_, _, cx| cx.notify()));
                self.pdf = Some(pdf);
            }
            Diff::Image { previous, current } => {
                let (previous, current, kind) = (previous.clone(), current.clone(), snap.kind);
                self.image = Some(
                    cx.new(|cx| ImageDiff::new(previous.as_ref(), current.as_ref(), kind, cx)),
                );
            }
            _ => {}
        }
        self.load_archive(snap, cx);
        // a reload of the same bytes keeps the font view (and its state)
        let keep = self
            .font
            .as_ref()
            .is_some_and(|f| **f.source(cx) == *snap.diff);
        if !keep {
            self.font = match &*snap.diff {
                Diff::Font { .. } => {
                    let diff = snap.diff.clone();
                    Some(FontPane::Font(cx.new(|cx| FontDiffView::new(diff, cx))))
                }
                Diff::FontSource { .. } => {
                    let diff = snap.diff.clone();
                    Some(FontPane::Source(cx.new(|cx| FontSourceView::new(diff, cx))))
                }
                _ => None,
            };
        }
        // `1324-document-diff`: the rendered view, made again for a new diff
        match &snap.document_source {
            Some(source)
                if matches!(*snap.diff, Diff::Document { .. })
                    && !self
                        .document
                        .as_ref()
                        .is_some_and(|d| Arc::ptr_eq(d.read(cx).diff(), source)) =>
            {
                let source = source.clone();
                self.document = Some(cx.new(|cx| DocumentDiff::new(source, cx)));
            }
            Some(_) => {}
            None => self.document = None,
        }
        self.load_rich(snap, cx);
        let same_file = self
            .rows_key
            .as_ref()
            .is_some_and(|(repo, path, _)| *repo == snap.key.0 && *path == snap.key.1);
        self.previous_rows = same_file.then(|| self.rows.clone());
        if !same_file {
            self.tokens = None;
            // `1304-diff-no-wrap`: another file starts at its lines' start
            self.h_scroll.reset();
        }
        self.rows = Rc::new(build_rows(&self.hunks));
        self.rebuild_split_rows(cx);
        self.rows_key = Some(snap.key.clone());
        self.text_selection = None;
        self.list_state
            .reset_with_uniform_height(self.row_count(), DIFF_LINE_HEIGHT());
        self.refresh_search();
        self.highlight(snap.key.clone(), cx);
    }

    pub fn set_hovered_group(&mut self, group: Option<u32>, cx: &mut Context<Self>) {
        if self.hovered_group != group {
            self.hovered_group = group;
            cx.notify();
        }
    }

    /// `onStartSelection`
    pub fn start_selection(&mut self, temp: TempSelection, cx: &mut Context<Self>) {
        self.temp = Some(temp);
        cx.notify();
    }

    /// `onUpdateSelection`: the pointer crossed a changed row.
    pub fn extend_selection(&mut self, line: u32, cx: &mut Context<Self>) {
        if let Some(t) = self.temp.as_mut()
            && t.to != line
        {
            t.to = line;
            cx.notify();
        }
    }

    /// `onEndSelection`: commit the dragged range.
    pub(crate) fn end_selection(&mut self, cx: &mut Context<Self>) {
        let Some(t) = self.temp.take() else { return };
        let target = {
            let s = self.state.read(cx);
            // `1311-stacked-diff`: this view's file, not the selected one
            let stacked = self.stacked_path.clone();
            s.selected.and_then(|id| {
                s.repo_states
                    .get(&id)
                    .and_then(|r| stacked.or_else(|| r.selected_file.clone()))
                    .map(|p| (id, p))
            })
        };
        if let Some((id, path)) = target {
            let from = t.from.min(t.to);
            let len = t.from.max(t.to) - from + 1;
            Dispatcher::set_diff_lines(id, path, from, len, t.selected, cx);
        }
        cx.notify();
    }

    // ---- text selection (GHD: the browser's selection over the rows) ----

    /// The text at a list row in `column`: the unified row's text, or the
    /// split row's side (`None` when that side is empty, so copying one
    /// column skips the other side's lines as GHD's `exclude` does).
    fn row_text(&self, list_ix: usize, column: Column) -> Option<&str> {
        if !self.split_mode {
            return self.rows.get(list_ix).map(|r| r.text.as_str());
        }
        let (before, after) = self.split_rows.get(list_ix)?.unified_rows();
        let unified = match column {
            Column::Before => before,
            Column::After => after,
        }?;
        self.rows.get(unified).map(|r| r.text.as_str())
    }

    /// The byte offset in the row's text under `position`, hit-tested on
    /// the row's own (wrapped) layout: the visual line under the pointer,
    /// clamped to the first and last, then the closest character boundary.
    fn column_at(&self, list_ix: usize, column: Column, position: Point<Pixels>) -> usize {
        let Some(len) = self.row_text(list_ix, column).map(str::len) else {
            return 0;
        };
        let Some(RowText { bounds, layout }) =
            self.text_bounds.borrow().get(&(list_ix, column)).cloned()
        else {
            return 0;
        };
        if len == 0 {
            return 0;
        }
        let Some(line) = layout.line_layout_for_index(0) else {
            return 0;
        };
        let line_height = layout.line_height();
        let local = position - bounds.origin;
        let last = line.wrap_boundaries().len() as f32;
        let visual_line = (local.y / line_height).floor().clamp(0., last);
        let at = point(local.x.max(Pixels::ZERO), line_height * (visual_line + 0.5));
        match line.closest_index_for_position(at, line_height) {
            Ok(ix) | Err(ix) => ix.min(len),
        }
    }

    /// Mouse down on a row's text: start a selection there, or extend the
    /// existing one with shift (`selectingTextInRow` fixes the column).
    pub fn start_text_selection(
        &mut self,
        list_ix: usize,
        column: Column,
        position: Point<Pixels>,
        shift: bool,
        cx: &mut Context<Self>,
    ) {
        let pos = TextPos {
            row: list_ix,
            col: self.column_at(list_ix, column, position),
        };
        match self.text_selection.as_mut() {
            Some(sel) if shift && sel.column == column => {
                sel.head = pos;
                sel.dragging = true;
            }
            _ => {
                self.text_selection = Some(TextSelection {
                    anchor: pos,
                    head: pos,
                    column,
                    dragging: true,
                });
            }
        }
        cx.notify();
    }

    /// The pointer moved while a text selection is being dragged: the head
    /// follows the row under it; past the visible rows it clamps to the first
    /// or last one and scrolls the list a line (`list.scrollToRow`).
    pub(crate) fn drag_text_selection(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(sel) = self.text_selection else {
            return;
        };
        if !sel.dragging {
            return;
        }
        let column = sel.column;
        type RowHit = Option<(usize, Bounds<Pixels>)>;
        let (rows_above, rows_below, hit): (RowHit, RowHit, RowHit) = {
            let bounds = self.text_bounds.borrow();
            let mut above: Option<(usize, Bounds<Pixels>)> = None;
            let mut below: Option<(usize, Bounds<Pixels>)> = None;
            let mut hit = None;
            for (&(ix, col), t) in bounds.iter() {
                let b = t.bounds;
                if col != column {
                    continue;
                }
                if position.y >= b.origin.y && position.y < b.origin.y + b.size.height {
                    hit = Some((ix, b));
                } else if position.y < b.origin.y {
                    if below.is_none_or(|(_, o)| b.origin.y < o.origin.y) {
                        below = Some((ix, b));
                    }
                } else if above.is_none_or(|(_, o)| b.origin.y > o.origin.y) {
                    above = Some((ix, b));
                }
            }
            (above, below, hit)
        };
        let head = if let Some((ix, _)) = hit {
            TextPos {
                row: ix,
                col: self.column_at(ix, column, position),
            }
        } else if let Some((ix, b)) = rows_below.filter(|_| rows_above.is_none()) {
            // above every rendered row: select from the first visible one up
            if position.y < b.origin.y - DIFF_LINE_HEIGHT() / 2. {
                self.list_state.scroll_by(-DIFF_LINE_HEIGHT());
            }
            TextPos { row: ix, col: 0 }
        } else if let Some((ix, b)) = rows_above {
            if position.y > b.origin.y + b.size.height + DIFF_LINE_HEIGHT() / 2. {
                self.list_state.scroll_by(DIFF_LINE_HEIGHT());
            }
            TextPos {
                row: ix,
                col: self.row_text(ix, column).map(str::len).unwrap_or(0),
            }
        } else {
            return;
        };
        if let Some(sel) = self.text_selection.as_mut()
            && sel.head != head
        {
            sel.head = head;
            cx.notify();
        }
    }

    /// Mouse up: the drag is over; a click without a drag leaves no selection.
    pub(crate) fn end_text_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(sel) = self.text_selection.as_mut()
            && sel.dragging
        {
            sel.dragging = false;
            if sel.is_empty() {
                self.text_selection = None;
            }
            cx.notify();
        }
    }

    /// A left click outside any row's text drops the selection.
    pub(crate) fn clear_text_selection_unless_on_text(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.text_selection.is_none() {
            return;
        }
        let on_text = self
            .text_bounds
            .borrow()
            .values()
            .any(|t| t.bounds.contains(&position));
        if !on_text {
            self.text_selection = None;
            cx.notify();
        }
    }

    fn text_selection_snapshot(&self) -> Option<TextSelectionSnapshot> {
        self.text_selection
            .filter(|s| !s.is_empty())
            .map(|s| s.snapshot())
    }

    /// `onSelectAll` (`document.getSelection().selectAllChildren(diffContainer)`).
    pub fn select_all_text(&mut self, cx: &mut Context<Self>) {
        let count = self.row_count();
        if count == 0 {
            return;
        }
        let column = self
            .text_selection
            .map(|s| s.column)
            .unwrap_or(Column::Before);
        let last = count - 1;
        self.text_selection = Some(TextSelection {
            anchor: TextPos { row: 0, col: 0 },
            head: TextPos {
                row: last,
                col: self.row_text(last, column).map(str::len).unwrap_or(0),
            },
            column,
            dragging: false,
        });
        cx.notify();
    }

    /// The selected text, one line per row (rows without text in the
    /// selected column are skipped), without line numbers or markers.
    pub fn selected_text(&self) -> Option<String> {
        let sel = self.text_selection_snapshot()?;
        Some(join_selected_text(&sel, |ix| self.row_text(ix, sel.column)))
    }

    /// ⌘C / the context menu's Copy.
    pub fn copy_text_selection(&mut self, cx: &mut Context<Self>) -> bool {
        match self.selected_text() {
            Some(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                true
            }
            None => false,
        }
    }

    /// `1322-pdf-diff`: whether a PDF file's diff shows its text (the
    /// header's switch; not inside a stack).
    fn wants_pdf_text(&self, diff: &Diff, cx: &App) -> bool {
        let s = self.state.read(cx);
        // `1325-document-pages`: a document's pages stay pages
        self.document_source.is_none()
            && self.stacked_path.is_none()
            && is_pdf_diff(diff)
            && s.settings.pdf_text_diff
            && s.flags.bool(corvene_core::flags::ids::PDF_DIFF)
    }

    /// `1322-pdf-diff`: the text diff to show for the page diff `diff`,
    /// once read.
    fn pdf_text_of(&self, diff: &Arc<Diff>, cx: &App) -> Option<Arc<Diff>> {
        if !self.wants_pdf_text(diff, cx) {
            return None;
        }
        match self.pdf_text.as_ref()? {
            (source, PdfText::Ready(text)) if Arc::ptr_eq(source, diff) => Some(text.clone()),
            _ => None,
        }
    }

    /// `1322-pdf-diff`: reads the text of both versions of a PDF file in
    /// the background when its diff should show it.
    fn request_pdf_text(&mut self, diff: &Arc<Diff>, cx: &mut Context<Self>) {
        if !self.wants_pdf_text(diff, cx)
            || self
                .pdf_text
                .as_ref()
                .is_some_and(|(source, _)| Arc::ptr_eq(source, diff))
        {
            return;
        }
        let Diff::Image { previous, current } = &**diff else {
            return;
        };
        let (old, new) = (
            previous.as_ref().map(|b| b.bytes.clone()),
            current.as_ref().map(|b| b.bytes.clone()),
        );
        self.pdf_text = Some((diff.clone(), PdfText::Reading));
        let read = cx.background_executor().spawn(async move {
            let text = |bytes: Option<Vec<u8>>| -> Result<Option<String>, ()> {
                let Some(bytes) = bytes else {
                    return Ok(None);
                };
                let doc = corvene_core::pdf::PdfDocument::open(bytes).map_err(|_| ())?;
                doc.text().map(Some).ok_or(())
            };
            let (old, new) = (text(old).ok()?, text(new).ok()?);
            Some(corvene_core::pdf::text_diff(old.as_deref(), new.as_deref()))
        });
        let source = diff.clone();
        self._pdf_text_task = Some(cx.spawn(async move |this, cx| {
            let text = read.await;
            this.update(cx, |this, cx| {
                if this
                    .pdf_text
                    .as_ref()
                    .is_some_and(|(s, _)| Arc::ptr_eq(s, &source))
                {
                    this.pdf_text = Some((
                        source,
                        text.map_or(PdfText::Unreadable, |d| PdfText::Ready(Arc::new(d))),
                    ));
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// `757-typechange-diff`: a type change whose lines are not selectable
    /// or expandable; also `796-lfs-text-diff`'s contents of a file stored
    /// in Git LFS (the index holds its pointer).
    fn locked_type_change(&self, diff: &Diff, cx: &App) -> bool {
        let warnings = diff.warnings();
        warnings.is_some_and(|w| w.lfs_contents || w.pdf_text || w.document_text || w.derived_text)
            || (warnings.is_some_and(|w| w.type_change.is_some())
                && self
                    .state
                    .read(cx)
                    .flags
                    .bool(corvene_core::flags::ids::TYPECHANGE_DIFF))
    }

    // ---- expansion ----

    fn can_expand(&self) -> bool {
        self.contents.as_ref().is_some_and(|c| !c.is_empty())
    }

    /// `onExpandHunk`
    pub fn expand(&mut self, hunk_index: usize, kind: ExpansionKind, cx: &mut Context<Self>) {
        let Some(contents) = self.contents.clone() else {
            return;
        };
        let Some(key) = self.rows_key.clone() else {
            return;
        };
        if let Some(hunks) = expand_hunk(
            &self.hunks,
            hunk_index,
            kind,
            &contents,
            DEFAULT_DIFF_EXPANSION_STEP,
        ) {
            self.hunks = Rc::new(hunks);
            self.expanded = true;
            self.rebuild_rows(key, cx);
            cx.notify();
        }
    }

    /// `onExpandWholeFile`
    fn expand_whole_file(&mut self, cx: &mut Context<Self>) {
        let Some(contents) = self.contents.clone() else {
            return;
        };
        let Some(key) = self.rows_key.clone() else {
            return;
        };
        if let Some(hunks) = expand_whole(self.hunks.as_ref().clone(), &contents) {
            self.hunks = Rc::new(hunks);
            self.expanded = true;
            self.rebuild_rows(key, cx);
            cx.notify();
        }
    }

    /// `onCollapseExpandedLines`
    fn collapse(&mut self, cx: &mut Context<Self>) {
        let Some(snap) = self.snapshot(cx) else {
            return;
        };
        if let Some(hunks) = snap.diff.hunks() {
            self.hunks = Rc::new(from_hunks(hunks, self.contents.as_ref().map(|c| c.len())));
        }
        self.expanded = false;
        self.rebuild_rows(snap.key, cx);
        cx.notify();
    }

    // ---- context menus ----

    fn open_menu(
        &mut self,
        items: Vec<MenuItem>,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::native_menu::show_context_menu(items, position, window, cx);
    }

    /// GHD `buildExpandMenuItem`.
    fn expand_menu_item(&self, cx: &Context<Self>) -> Option<MenuItem> {
        if !self.can_expand() {
            return None;
        }
        let weak = cx.weak_entity();
        Some(if self.expanded {
            MenuItem::new(
                mac_or("Collapse Expanded Lines", "Collapse expanded lines"),
                move |_, cx| {
                    weak.update(cx, |this, cx| this.collapse(cx)).ok();
                },
            )
        } else {
            let enabled =
                self.hunks.len() != 1 || self.hunks[0].expansion != HunkExpansionType::None;
            MenuItem::new(
                mac_or("Expand Whole File", "Expand whole file"),
                move |_, cx| {
                    weak.update(cx, |this, cx| this.expand_whole_file(cx)).ok();
                },
            )
            .enabled(enabled)
        })
    }

    /// `onContextMenuExpandHunk`
    pub fn expand_menu(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(item) = self.expand_menu_item(cx) {
            self.open_menu(vec![item], position, window, cx);
        }
    }

    /// `onContextMenuText`: Copy (with a selection), Select All, then the
    /// expansion item. `line` is the clicked row's new-file line number;
    /// with `diff-open-in-editor-at-line` a working-directory diff adds
    /// "Open in <Editor> at Line N" when the editor can jump to a line.
    /// `discard` is a changed row's selection index and block
    /// ([`Row::discard_target`](crate::diff_view_rows::Row::discard_target)):
    /// with `760-discard-from-text-menu` the gutter's discard items follow.
    pub fn text_menu(
        &mut self,
        position: Point<Pixels>,
        line: Option<u32>,
        discard: Option<(u32, (u32, u32), RangeType)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak = cx.weak_entity();
        let has_selection = self.text_selection_snapshot().is_some();
        let copy = {
            let weak = weak.clone();
            MenuItem::new("Copy", move |_, cx| {
                weak.update(cx, |this, cx| {
                    this.copy_text_selection(cx);
                })
                .ok();
            })
            .enabled(has_selection)
        };
        let select_all = MenuItem::new(mac_or("Select All", "Select all"), move |_, cx| {
            weak.update(cx, |this, cx| this.select_all_text(cx)).ok();
        });
        let mut items = vec![copy, select_all];
        if let Some(item) = self.open_at_line_menu_item(line, cx) {
            items.push(MenuItem::separator());
            items.push(item);
        }
        // `760-discard-from-text-menu`: the line, then its block, then the
        // changed lines the text selection spans
        let discard_menu = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::DISCARD_FROM_TEXT_MENU);
        let mut separated = false;
        if let Some((original, (start, len), kind)) = discard
            && discard_menu
            && let Some(line_item) = self.discard_item(original, 1, kind, cx)
        {
            items.push(MenuItem::separator());
            separated = true;
            items.push(line_item);
            if len > 1 {
                items.extend(self.discard_item(start, len, kind, cx));
            }
        }
        if discard_menu && let Some(item) = self.discard_selected_text_item(cx) {
            if !separated {
                items.push(MenuItem::separator());
                separated = true;
            }
            items.push(item);
        }
        // `1303-partial-stash`: the same lines into a stash
        let mut stash_items: Vec<MenuItem> = Vec::new();
        if let Some((original, (start, len), kind)) = discard {
            stash_items.extend(self.stash_item(original, 1, kind, cx));
            if len > 1 {
                stash_items.extend(self.stash_item(start, len, kind, cx));
            }
        }
        stash_items.extend(self.stash_selected_text_item(cx));
        if !stash_items.is_empty() {
            if !separated {
                items.push(MenuItem::separator());
            }
            items.extend(stash_items);
        }
        if let Some(item) = self.expand_menu_item(cx) {
            items.push(MenuItem::separator());
            items.push(item);
        }
        self.open_menu(items, position, window, cx);
    }

    /// "Open in <Editor> at Line N" (not in GHD, desktop/desktop#14476).
    fn open_at_line_menu_item(&self, line: Option<u32>, cx: &Context<Self>) -> Option<MenuItem> {
        let (full, line, editor) = self.open_at_line_target(line, cx)?;
        let label = if IS_MAC {
            format!("Open in {editor} at Line {line}")
        } else {
            format!("Open in {editor} at line {line}")
        };
        Some(MenuItem::new(label, move |_, cx| {
            Dispatcher::open_in_editor_at(full.clone(), Some(line), cx)
        }))
    }

    /// `741-diff-open-in-editor-at-line`: ⌥-click on a line's text opens the
    /// editor there (desktop/desktop#20254). False when it does not apply,
    /// so the click selects text as usual.
    pub fn open_at_line(&self, line: Option<u32>, cx: &mut Context<Self>) -> bool {
        let Some((full, line, _)) = self.open_at_line_target(line, cx) else {
            return false;
        };
        Dispatcher::open_in_editor_at(full, Some(line), cx);
        true
    }

    /// The file, line and editor name for "Open in <Editor> at Line N".
    fn open_at_line_target(
        &self,
        line: Option<u32>,
        cx: &App,
    ) -> Option<(std::path::PathBuf, u32, String)> {
        let line = line?;
        if self.source != DiffSource::WorkingDirectory {
            return None;
        }
        let snap = self.snapshot(cx)?;
        let s = self.state.read(cx);
        if !s
            .flags
            .bool(corvene_core::flags::ids::DIFF_OPEN_IN_EDITOR_AT_LINE)
            || !s.editor_supports_line()
            || snap.kind == FileStatusKind::Deleted
        {
            return None;
        }
        Some((snap.repo_path.join(&snap.path), line, s.editor_label()))
    }

    /// `onContextMenuLine`: discard one changed line.
    pub fn line_menu(
        &mut self,
        line: u32,
        kind: RangeType,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.discard_menu(line, 1, kind, position, window, cx);
    }

    /// `onContextMenuHunk`: discard a whole block of changes.
    pub fn hunk_menu(
        &mut self,
        start: u32,
        len: u32,
        kind: RangeType,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.discard_menu(start, len, kind, position, window, cx);
    }

    fn discard_menu(
        &mut self,
        start: u32,
        len: u32,
        kind: RangeType,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // `1303-partial-stash`: Stash … Lines after the discard item
        let items: Vec<MenuItem> = self
            .discard_item(start, len, kind, cx)
            .into_iter()
            .chain(self.stash_item(start, len, kind, cx))
            .collect();
        if !items.is_empty() {
            self.open_menu(items, position, window, cx);
        }
    }

    /// The selection indices of the added and removed lines the text
    /// selection spans, each once and in order.
    fn selected_text_lines(&self) -> Option<Vec<u32>> {
        let sel = self.text_selection_snapshot()?;
        // a selection that ends at the start of a row takes nothing from it
        let last = if sel.end.col == 0 && sel.end.row > sel.start.row {
            sel.end.row - 1
        } else {
            sel.end.row
        };
        Some(
            (sel.start.row..=last)
                .filter_map(|ix| {
                    let unified = if self.split_mode {
                        let (before, after) = self.split_rows.get(ix)?.unified_rows();
                        match sel.column {
                            Column::Before => before,
                            Column::After => after,
                        }?
                    } else {
                        ix
                    };
                    Some(self.rows.get(unified)?.discard_target()?.0)
                })
                .collect::<std::collections::BTreeSet<u32>>()
                .into_iter()
                .collect(),
        )
    }

    /// `1303-partial-stash`: "Stash N Selected Lines" for the changed lines
    /// the text selection spans, when there are several.
    fn stash_selected_text_item(&self, cx: &Context<Self>) -> Option<MenuItem> {
        let lines = self.selected_text_lines()?;
        if lines.len() < 2 {
            return None;
        }
        let (repo, path, enabled) = self.stash_target(cx)?;
        let count = lines.len();
        let label = if IS_MAC {
            format!("Stash {count} Selected Lines")
        } else {
            format!("Stash {count} selected lines")
        };
        Some(
            MenuItem::new(label, move |_, cx| {
                Dispatcher::stash_selected_lines(
                    repo,
                    path.clone(),
                    selection_of_lines(&lines),
                    cx,
                );
            })
            .enabled(enabled),
        )
    }

    /// `1303-partial-stash`: "Stash Added Line…" for `len` selection lines
    /// from `start`, where lines can be stashed ([`Self::stash_target`]).
    fn stash_item(
        &self,
        start: u32,
        len: u32,
        kind: RangeType,
        cx: &Context<Self>,
    ) -> Option<MenuItem> {
        let (repo, path, enabled) = self.stash_target(cx)?;
        Some(
            MenuItem::new(kind.stash_label(len), move |_, cx| {
                let selection = DiffSelection::none().with_range(start, len, true);
                Dispatcher::stash_selected_lines(repo, path.clone(), selection, cx);
            })
            .enabled(enabled),
        )
    }

    /// The repository and file whose lines the diff's menus can stash, and
    /// whether that is possible now (not while the branch's only stash is
    /// taken, `PartialStash::Blocked`). `None` outside the Changes tab, for
    /// files other than modified ones and wherever Discard is not offered.
    fn stash_target(&self, cx: &Context<Self>) -> Option<(u64, String, bool)> {
        let snap = self.snapshot(cx)?;
        if self.source != DiffSource::WorkingDirectory
            || snap.kind != FileStatusKind::Modified
            || snap.hide_whitespace
            || snap.as_text
            || self.locked_type_change(&snap.diff, cx)
        {
            return None;
        }
        let s = self.state.read(cx);
        let stash = corvene_core::stash_flows::PartialStash::of(s, s.repo_states.get(&snap.repo)?)?;
        Some((
            snap.repo,
            snap.path.clone(),
            stash != corvene_core::stash_flows::PartialStash::Blocked,
        ))
    }

    /// `760-discard-from-text-menu`: "Discard N Selected Lines…" for the
    /// added and removed lines the text selection spans, when there are
    /// several (desktop/desktop#16415); same conditions as [`Self::discard_item`].
    fn discard_selected_text_item(&self, cx: &Context<Self>) -> Option<MenuItem> {
        let lines = self.selected_text_lines()?;
        if lines.len() < 2 {
            return None;
        }
        let snap = self.snapshot(cx)?;
        if self.source != DiffSource::WorkingDirectory
            || snap.kind == FileStatusKind::Conflicted
            || snap.hide_whitespace
            || snap.as_text
            || self.locked_type_change(&snap.diff, cx)
        {
            return None;
        }
        let count = lines.len();
        let suffix = if snap.confirm_discard { "…" } else { "" };
        let label = if IS_MAC {
            format!("Discard {count} Selected Lines{suffix}")
        } else {
            format!("Discard {count} selected lines{suffix}")
        };
        let (repo, path) = (snap.repo, snap.path.clone());
        Some(MenuItem::new(label, move |_, cx| {
            Dispatcher::request_discard_selection(
                repo,
                path.clone(),
                selection_of_lines(&lines),
                cx,
            );
        }))
    }

    /// "Discard Added Line…" for `len` selection lines from `start`; none
    /// outside the Changes tab, for conflicts or with whitespace hidden.
    fn discard_item(
        &self,
        start: u32,
        len: u32,
        kind: RangeType,
        cx: &Context<Self>,
    ) -> Option<MenuItem> {
        let snap = self.snapshot(cx)?;
        if self.source != DiffSource::WorkingDirectory
            || snap.kind == FileStatusKind::Conflicted
            || snap.hide_whitespace
            // the rows were not selectable (`text_diff`'s `canSelect`)
            || snap.as_text
            || self.locked_type_change(&snap.diff, cx)
        {
            return None;
        }
        let label = kind.discard_label(len, snap.confirm_discard);
        let (repo, path) = (snap.repo, snap.path.clone());
        Some(MenuItem::new(label, move |_, cx| {
            let selection = DiffSelection::none().with_range(start, len, true);
            Dispatcher::request_discard_selection(repo, path.clone(), selection, cx);
        }))
    }

    // ---- search ----

    fn show_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = match self.search_input.clone() {
            Some(input) => input,
            None => {
                let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search…"));
                cx.subscribe(&input, |this, _, ev: &InputEvent, cx| match ev {
                    InputEvent::PressEnter { shift, .. } => this.search(!*shift, cx),
                    InputEvent::Blur => this.close_search(cx),
                    // `1305-diff-find-controls`: the count only describes the
                    // text it was searched for
                    InputEvent::Change if this.find_controls => cx.notify(),
                    _ => {}
                })
                .detach();
                self.search_input = Some(input.clone());
                input
            }
        };
        if !self.searching {
            self.searching = true;
            self.search_query.clear();
            self.hits.clear();
            self.selected_hit = None;
            input.update(cx, |s, cx| s.set_value("", window, cx));
        }
        let handle = input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    /// `onSearchCancel`
    fn close_search(&mut self, cx: &mut Context<Self>) {
        if !self.searching {
            return;
        }
        self.searching = false;
        self.search_query.clear();
        self.hits.clear();
        self.selected_hit = None;
        cx.notify();
    }

    /// `onSearch`: a new query starts at the first hit; the same query moves
    /// to the next / previous one (wrapping).
    fn search(&mut self, forward: bool, cx: &mut Context<Self>) {
        let query = self
            .search_input
            .as_ref()
            .map(|i| i.read(cx).value().to_string())
            .unwrap_or_default();
        if query.is_empty() {
            self.search_query.clear();
            self.hits.clear();
            self.selected_hit = None;
            cx.notify();
            return;
        }
        if query != self.search_query || self.hits.is_empty() {
            self.search_query = query;
            self.hits = search_rows(&self.rows, &self.search_query, self.case_sensitive());
            self.selected_hit = (!self.hits.is_empty()).then_some(0);
        } else if let Some(ix) = self.selected_hit {
            let n = self.hits.len();
            self.selected_hit = Some(if forward {
                (ix + 1) % n
            } else {
                (ix + n - 1) % n
            });
        }
        self.reveal_selected_hit();
        cx.notify();
    }

    /// Scroll the selected search hit into view.
    fn reveal_selected_hit(&mut self) {
        if let Some(hit) = self.selected_hit.and_then(|ix| self.hits.get(ix)) {
            let row = if self.split_mode {
                self.unified_to_split
                    .get(hit.row)
                    .copied()
                    .unwrap_or(hit.row)
            } else {
                hit.row
            };
            self.list_state.scroll_to_reveal_item(row);
            // `1304-diff-no-wrap`: and sideways
            if self.no_wrap
                && let Some(text) = self.rows.get(hit.row).map(|r| r.text.as_str())
            {
                let column = |byte: usize| text.get(..byte).map_or(0, |t| t.chars().count());
                self.h_scroll
                    .reveal_columns(column(hit.range.start), column(hit.range.end));
            }
        }
    }

    /// `1305-diff-find-controls`: the search matches case (Aa).
    fn case_sensitive(&self) -> bool {
        self.find_controls && self.search_case_sensitive
    }

    /// `1305-diff-find-controls`: Aa flips matching case and searches the
    /// same text again, from the first hit.
    fn toggle_search_case(&mut self, cx: &mut Context<Self>) {
        self.search_case_sensitive = !self.search_case_sensitive;
        if !self.search_query.is_empty() {
            self.hits = search_rows(&self.rows, &self.search_query, self.case_sensitive());
            self.selected_hit = (!self.hits.is_empty()).then_some(0);
            self.reveal_selected_hit();
        }
        cx.notify();
    }

    /// `1305-diff-find-controls`: what the count says, once the text in
    /// the box has been searched for ("3 of 12", "No results").
    fn search_status(&self, typed: &str) -> Option<String> {
        if self.search_query.is_empty() || typed != self.search_query {
            return None;
        }
        Some(match self.selected_hit {
            Some(ix) => format!("{} of {}", ix + 1, self.hits.len()),
            None => "No results".to_string(),
        })
    }

    /// `1305-diff-find-controls`: count, Aa, previous and next after the
    /// box. The buttons keep focus in the box (a blur closes the search).
    fn find_controls_row(&self, typed: &str, cx: &Context<Self>) -> AnyElement {
        find_controls_row(
            FindControls {
                status: self.search_status(typed),
                // a new text is searched by the arrows as by Enter
                can_step: !typed.is_empty(),
                case: self.search_case_sensitive,
            },
            cx.listener(|this, _, _, cx| this.toggle_search_case(cx)),
            cx.listener(|this, _, _, cx| this.search(false, cx)),
            cx.listener(|this, _, _, cx| this.search(true, cx)),
            cx,
        )
    }

    /// `1312-stacked-diff-search`: search this file's rows for the stack's
    /// query (hits are recomputed only when the query, the case or the
    /// rows changed) and mark `selected` (an index into this file's hits)
    /// as the selected one; how many hits the file has.
    pub(crate) fn stacked_apply_search(
        &mut self,
        query: &str,
        case: bool,
        selected: Option<usize>,
    ) -> usize {
        let rows_at = Rc::as_ptr(&self.rows) as usize;
        if self.search_query != query
            || self.search_case_sensitive != case
            || self.stacked_search_rows != Some(rows_at)
        {
            self.search_query = query.to_string();
            self.search_case_sensitive = case;
            self.stacked_search_rows = Some(rows_at);
            self.hits = search_rows(&self.rows, query, self.case_sensitive());
        }
        self.selected_hit = selected.filter(|ix| *ix < self.hits.len());
        self.hits.len()
    }

    /// `1312-stacked-diff-search`: the stack's search closed.
    pub(crate) fn stacked_clear_search(&mut self) {
        if self.stacked_search_rows.take().is_some() {
            self.search_query.clear();
            self.hits.clear();
            self.selected_hit = None;
        }
    }

    /// `1312-stacked-diff-search`: the list row (unified or split) of this
    /// file's hit `local`, revealed sideways too when lines do not wrap.
    pub(crate) fn stacked_hit_row(&self, local: usize) -> Option<usize> {
        let hit = self.hits.get(local)?;
        if self.no_wrap
            && let Some(text) = self.rows.get(hit.row).map(|r| r.text.as_str())
        {
            let column = |byte: usize| text.get(..byte).map_or(0, |t| t.chars().count());
            self.h_scroll
                .reveal_columns(column(hit.range.start), column(hit.range.end));
        }
        Some(if self.split_mode {
            self.unified_to_split
                .get(hit.row)
                .copied()
                .unwrap_or(hit.row)
        } else {
            hit.row
        })
    }

    /// Recompute the hits for the same query after the rows changed.
    fn refresh_search(&mut self) {
        if self.search_query.is_empty() {
            return;
        }
        self.hits = search_rows(&self.rows, &self.search_query, self.case_sensitive());
        self.selected_hit = self
            .selected_hit
            .filter(|ix| *ix < self.hits.len())
            .or((!self.hits.is_empty()).then_some(0));
    }

    fn search_index(&self) -> Option<Rc<SearchIndex>> {
        if self.hits.is_empty() {
            return None;
        }
        let mut by_row: HashMap<usize, Vec<(std::ops::Range<usize>, bool)>> = HashMap::new();
        for (ix, hit) in self.hits.iter().enumerate() {
            by_row
                .entry(hit.row)
                .or_default()
                .push((hit.range.clone(), self.selected_hit == Some(ix)));
        }
        Some(Rc::new(SearchIndex { by_row }))
    }

    // ---- popovers ----

    pub fn toggle_options(&mut self, cx: &mut Context<Self>) {
        self.options_open = !self.options_open;
        cx.notify();
    }

    pub fn show_whitespace_hint(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.whitespace_hint = Some(position);
        cx.notify();
    }

    /// `DiffOptions` popover: Diff Settings › Whitespace, Diff display.
    fn options_popover(&self, snap: &Snapshot, window: &Window, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let anchor = self.gear_bounds.get();
        let source = self.source;
        let interactive = self.source == DiffSource::WorkingDirectory;
        let hide = snap.hide_whitespace;
        let split = self.state.read(cx).settings.show_side_by_side_diff;
        // `1304-diff-no-wrap`
        let wrap = {
            let s = self.state.read(cx);
            s.flags
                .bool(corvene_core::flags::ids::DIFF_NO_WRAP)
                .then_some(s.settings.diff_wrap_lines)
        };
        // `1316-diff-moved-lines` / `1317-diff-stylistic-changes`
        let (mark_moved, mark_stylistic) = {
            let s = self.state.read(cx);
            (
                s.flags
                    .bool(corvene_core::flags::ids::DIFF_MOVED_LINES)
                    .then_some(s.settings.diff_mark_moved_lines),
                s.flags
                    .bool(corvene_core::flags::ids::DIFF_STYLISTIC_CHANGES)
                    .then_some(s.settings.diff_mark_stylistic_changes),
            )
        };
        let _ = window;
        let legend = |text: &str| {
            div()
                .font_weight(FontWeight::BOLD)
                .mb(zpx(6.))
                .child(text.to_string())
        };
        crate::popover::balloon_popover(
            anchor,
            crate::popover::PopoverAnchorPosition::BottomRight,
            crate::popover::popover_component(cx)
                .id("diff-options-popover")
                .occlude()
                // `.diff-options-component .popover-component`,
                // `.popover-content { padding: var(--spacing-double) }`
                .w(zpx(250.))
                .p(SPACING_DOUBLE())
                .text_size(FONT_SIZE())
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.options_open = false;
                    cx.notify();
                }))
                .line_height(zpx(18.))
                // `h3` (UA style: 1.17em, bold) with
                // `.diff-options-component` margins: 6 px (0.5em) between
                // it, the fieldsets, their legends and paragraphs
                .child(
                    div()
                        .font_weight(FontWeight::BOLD)
                        .text_size(FONT_SIZE() * 1.17)
                        .line_height(FONT_SIZE() * 1.17 * 1.5)
                        .mb(zpx(6.))
                        .child(mac_or("Diff Settings", "Diff Options")),
                )
                .child(
                    div()
                        .id("diff-options-popover")
                        .occlude()
                        .child(crate::widgets::touch_drag_occluder())
                        .w_full()
                        .p(SPACING())
                        .flex()
                        .flex_col()
                        .child(legend("Whitespace"))
                        .child(checkbox_row(
                            "diff-hide-whitespace",
                            hide,
                            mac_or("Hide Whitespace Changes", "Hide whitespace changes"),
                            move |checked, _, cx| set_hide_whitespace(source, checked, cx),
                            cx,
                        ))
                        // `.secondary-text { margin: 0.5em 0 }`
                        .when(interactive, |d| {
                            d.child(div().my(zpx(6.)).text_color(t.text_secondary).child(
                                "Interacting with individual lines or hunks \
                                 will be disabled while hiding whitespace.",
                            ))
                        }),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .mt(zpx(6.))
                        .child(legend("Diff display"))
                        .child(radio_row(
                            "diff-display-unified",
                            !split,
                            "Unified",
                            |_, cx| Dispatcher::set_show_side_by_side_diff(false, cx),
                            cx,
                        ))
                        // `.radio-button-component + .radio-button-component`
                        .child(div().mt(zpx(5.)).child(radio_row(
                            "diff-display-split",
                            split,
                            "Split",
                            |_, cx| Dispatcher::set_show_side_by_side_diff(true, cx),
                            cx,
                        )))
                        .when_some(wrap, |d, wrap| {
                            d.child(div().mt(zpx(6.)).child(checkbox_row(
                                "diff-wrap-lines",
                                wrap,
                                mac_or("Wrap Long Lines", "Wrap long lines"),
                                |checked, _, cx| Dispatcher::set_diff_wrap_lines(checked, cx),
                                cx,
                            )))
                        }),
                )
                .when(mark_moved.is_some() || mark_stylistic.is_some(), |d| {
                    d.child(
                        div()
                            .flex()
                            .flex_col()
                            .mt(zpx(6.))
                            .child(legend("Marks"))
                            .when_some(mark_moved, |d, on| {
                                d.child(checkbox_row(
                                    "diff-mark-moved",
                                    on,
                                    mac_or("Mark Moved Lines", "Mark moved lines"),
                                    |checked, _, cx| {
                                        Dispatcher::set_diff_mark_moved_lines(checked, cx)
                                    },
                                    cx,
                                ))
                            })
                            .when_some(mark_stylistic, |d, on| {
                                d.child(div().mt(zpx(5.)).child(checkbox_row(
                                    "diff-mark-stylistic",
                                    on,
                                    mac_or("Mark Stylistic Changes", "Mark stylistic changes"),
                                    |checked, _, cx| {
                                        Dispatcher::set_diff_mark_stylistic_changes(checked, cx)
                                    },
                                    cx,
                                )))
                            }),
                    )
                }),
            cx,
        )
        .with_priority(3)
        .into_any_element()
    }

    /// `WhitespaceHintPopover`: "Show whitespace changes?"
    fn whitespace_hint_popover(&self, anchor: Point<Pixels>, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let source = self.source;
        deferred(
            anchored()
                .position(anchor)
                .snap_to_window_with_margin(zpx(8.))
                .child(
                    div()
                        .id("whitespace-hint")
                        .occlude()
                        .child(crate::widgets::touch_drag_occluder())
                        .w(zpx(225.))
                        .p(SPACING())
                        .flex()
                        .flex_col()
                        .gap(zpx(6.))
                        .text_size(FONT_SIZE())
                        .text_color(t.text)
                        .bg(t.background)
                        .border_1()
                        .border_color(t.box_border)
                        .rounded(BORDER_RADIUS())
                        .shadow(vec![BoxShadow {
                            color: hsla(0., 0., 0., 0.3),
                            offset: point(zpx(0.), zpx(0.)),
                            blur_radius: css_blur(8.),
                            spread_radius: zpx(0.),
                            inset: false,
                        }])
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.whitespace_hint = None;
                            cx.notify();
                        }))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(FONT_SIZE_MD())
                                .child("Show whitespace changes?"),
                        )
                        .child(
                            div().text_color(t.text_secondary).child(
                                "Selecting lines is disabled when hiding whitespace changes.",
                            ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .justify_end()
                                .gap(SPACING_HALF())
                                .mt(zpx(4.))
                                .child(
                                    primary_button("whitespace-hint-yes", "Yes", false, cx)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.whitespace_hint = None;
                                            set_hide_whitespace(source, false, cx);
                                        })),
                                )
                                .child(button("whitespace-hint-no", "No", cx).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.whitespace_hint = None;
                                        cx.notify();
                                    }),
                                )),
                        ),
                ),
        )
        .with_priority(3)
        .into_any_element()
    }

    // ---- panels ----

    /// `.panel.empty` and friends: a centred message.
    fn panel(&self, message: impl Into<SharedString>, cx: &App) -> AnyElement {
        let t = cx.ghd();
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .p(SPACING_DOUBLE())
            .text_size(FONT_SIZE())
            .text_color(t.text_secondary)
            .child(message.into())
            .into_any_element()
    }

    /// GHD `renderText` with no hunks.
    fn empty_panel(&self, snap: &Snapshot, cx: &App) -> AnyElement {
        // `742-file-mode-change-message`
        if let Some((old, new)) = snap.diff.warnings().and_then(|w| w.mode_change.as_ref())
            && self
                .state
                .read(cx)
                .flags
                .bool(corvene_core::flags::ids::FILE_MODE_CHANGE_MESSAGE)
        {
            return self.panel(format!("The file mode changed from {old} to {new}"), cx);
        }
        if snap.kind == FileStatusKind::Renamed && snap.rename_includes_modifications {
            // `.panel.renamed` with the alert octicon
            let t = cx.ghd();
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .gap(SPACING_HALF())
                .p(SPACING_DOUBLE())
                .text_size(FONT_SIZE())
                .text_color(t.text_secondary)
                .child(octicon(Octicon::Alert, t.text_secondary))
                .child("The file was renamed and includes changes.")
                .into_any_element();
        }
        let message = match snap.kind {
            FileStatusKind::New | FileStatusKind::Untracked => "The file is empty",
            FileStatusKind::Renamed => "The file was renamed but not changed",
            FileStatusKind::Conflicted => {
                "The file is in conflict and must be resolved via the command line."
            }
            _ if snap.hide_whitespace => "Only whitespace changes found",
            _ => "No content changes found",
        };
        self.panel(message, cx)
    }

    /// GHD `BinaryFile`.
    fn binary_panel(&self, snap: &Snapshot, cx: &Context<Self>) -> AnyElement {
        // `1323-archive-diff`: an archive entry that did not read says why
        let reason = match self.source {
            DiffSource::ArchiveEntry(surface) => self
                .state
                .read(cx)
                .repo_states
                .get(&snap.repo)
                .and_then(|rs| rs.archives.open.get(&surface))
                .and_then(|b| b.entry.as_ref())
                .and_then(|e| e.error.clone()),
            _ => None,
        };
        self.binary_panel_with(snap, reason, cx)
    }

    /// [`Self::binary_panel`] with a line saying why the file could not be
    /// shown otherwise (`1323-archive-diff`).
    fn binary_panel_with(
        &self,
        snap: &Snapshot,
        reason: Option<String>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd();
        let full_path = snap.repo_path.join(&snap.path);
        let open: Rc<dyn Fn(&mut App)> = match self.source {
            DiffSource::ArchiveEntry(surface) => {
                let (repo, path) = (snap.repo, snap.path.clone());
                let side = if snap.kind == FileStatusKind::Deleted {
                    corvene_git::archive::EntrySide::Old
                } else {
                    corvene_git::archive::EntrySide::New
                };
                Rc::new(move |cx| {
                    Dispatcher::open_archive_entry_externally(repo, surface, path.clone(), side, cx)
                })
            }
            _ => Rc::new(move |cx| cx.open_with_system(&full_path)),
        };
        // `749-binary-diff-as-text`
        let as_text = (self.source == DiffSource::WorkingDirectory
            && self
                .state
                .read(cx)
                .flags
                .bool(corvene_core::flags::ids::BINARY_DIFF_AS_TEXT))
        .then(|| {
            let (repo, path) = (snap.repo, snap.path.clone());
            div().py(SPACING_HALF()).child(
                link_button("binary-as-text", "Show the diff as text anyway.", cx).on_click(
                    move |_, _, cx| {
                        Dispatcher::show_binary_diff_as_text_for(repo, path.clone(), cx)
                    },
                ),
            )
        });
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .pt(SPACING())
            .text_size(FONT_SIZE())
            .text_color(t.text)
            .child(
                div()
                    .py(SPACING())
                    .pb(SPACING_HALF())
                    .child("This binary file has changed."),
            )
            .child(
                div().py(SPACING_HALF()).child(
                    link_button("binary-open", "Open file in external program.", cx)
                        .on_click(move |_, _, cx| open(cx)),
                ),
            )
            .children(as_text)
            .children(reason.map(|reason| {
                div()
                    .py(SPACING_HALF())
                    .max_w(zpx(520.))
                    .text_center()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(reason)
            }))
            .into_any_element()
    }

    /// GHD `renderDiff` for an unrenderable diff. With
    /// `762-too-large-diff-escape-hatch` a working-directory file offers its
    /// configured `diff.tool` and the external editor.
    fn too_large_panel(&self, snap: &Snapshot, cx: &Context<Self>) -> AnyElement {
        const MESSAGE: &str = "The diff is too large to be displayed.";
        let s = self.state.read(cx);
        if self.source != DiffSource::WorkingDirectory
            || !s
                .flags
                .bool(corvene_core::flags::ids::TOO_LARGE_DIFF_ESCAPE_HATCH)
        {
            return self.panel(MESSAGE, cx);
        }
        let t = cx.ghd();
        let repo = snap.repo;
        let diff_tool = s
            .repo_states
            .get(&repo)
            .and_then(|rs| rs.diff_tool.clone())
            .map(|tool| {
                div().py(SPACING_HALF()).child(
                    link_button(
                        "too-large-diff-tool",
                        format!("Open in external diff tool ({tool})."),
                        cx,
                    )
                    .on_click(move |_, _, cx| Dispatcher::open_in_diff_tool(repo, cx)),
                )
            });
        let editor = (snap.kind != FileStatusKind::Deleted).then(|| {
            let full_path = snap.repo_path.join(&snap.path);
            div().py(SPACING_HALF()).child(
                link_button(
                    "too-large-editor",
                    format!("Open file in {}.", s.editor_label()),
                    cx,
                )
                .on_click(move |_, _, cx| Dispatcher::open_in_editor(full_path.clone(), cx)),
            )
        });
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .p(SPACING_DOUBLE())
            .text_size(FONT_SIZE())
            .text_color(t.text_secondary)
            .child(div().py(SPACING_HALF()).child(MESSAGE))
            .children(diff_tool)
            .children(editor)
            .into_any_element()
    }

    /// GHD `renderLargeTextDiff`.
    fn large_diff_panel(&self, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .m(SPACING_DOUBLE())
            .gap(SPACING())
            .text_size(FONT_SIZE())
            .text_color(t.text_secondary)
            .child(
                crate::widgets::blankslate_image("ufo-alert.svg", cx)
                    .max_h(zpx(150.))
                    .object_fit(ObjectFit::Contain),
            )
            .child(div().child("The diff is too large to be displayed by default."))
            .child(div().child(
                "You can try to show it anyway, but performance may be negatively impacted.",
            ))
            .child(
                button("show-large-diff", mac_or("Show Diff", "Show diff"), cx).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.show_large = true;
                        cx.notify();
                    }),
                ),
            )
            .into_any_element()
    }

    /// GHD `SubmoduleDiff` (`.changes-interstitial.submodule-diff`).
    fn submodule_panel(&self, diff: &SubmoduleDiff, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let read_only = self.source != DiffSource::WorkingDirectory;
        let item = |icon: Octicon, color: Hsla, body: Div| {
            div()
                .flex()
                .flex_row()
                .items_start()
                .gap(SPACING())
                .mb(SPACING())
                .child(div().flex_none().pt(zpx(2.)).child(octicon(icon, color)))
                .child(div().flex_1().min_w_0().child(body))
        };
        let sha = |sha: &str| -> Vec<Inline> {
            let full = sha.to_string();
            vec![
                Inline::Element(
                    code_ref(sha.chars().take(7).collect::<String>(), cx).into_any_element(),
                ),
                Inline::Element(
                    div()
                        .id(SharedString::from(format!("copy-{sha}")))
                        .cursor_pointer()
                        .ml(zpx(2.))
                        .child(octicon(Octicon::Copy, t.text_secondary).size(zpx(14.)))
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(full.clone()))
                        })
                        .into_any_element(),
                ),
            ]
        };
        let mut items: Vec<AnyElement> = Vec::new();
        // repository link
        if let Some(url) = diff.url.as_deref()
            && let Some(gh) =
                corvene_core::github_from_remote(url, &corvene_core::github_hosts(&[], true))
        {
            let html_url = gh.html_url.clone();
            let host = html_url
                .split('/')
                .nth(2)
                .unwrap_or("github.com")
                .to_string();
            let label = if host == "github.com" {
                format!("{}/{}", gh.owner, gh.name)
            } else {
                format!("{}/{} ({host})", gh.owner, gh.name)
            };
            items.push(
                item(
                    Octicon::Info,
                    t.text_secondary,
                    paragraph(vec![
                        "This is a submodule based on the repository ".into(),
                        Inline::Element(
                            link_button("submodule-repo-link", label, cx)
                                .on_click(move |_, _, cx| {
                                    corvene_core::Dispatcher::open_url(&html_url, cx)
                                })
                                .into_any_element(),
                        ),
                        ".".into(),
                    ]),
                )
                .into_any_element(),
            );
        }
        // commit change
        let verb = if read_only { "was" } else { "has been" };
        let suffix: Vec<Inline> = if read_only {
            vec![]
        } else {
            vec![" This change can be committed to the parent repository.".into()]
        };
        match (&diff.old_sha, &diff.new_sha) {
            (Some(old), Some(new)) => {
                let mut parts: Vec<Inline> = vec!["This submodule changed its commit from ".into()];
                parts.extend(sha(old));
                parts.push(" to ".into());
                parts.extend(sha(new));
                parts.push(".".into());
                parts.extend(suffix);
                items.push(
                    item(Octicon::DiffModified, t.color_modified, paragraph(parts))
                        .into_any_element(),
                );
            }
            (None, Some(new)) => {
                let mut parts: Vec<Inline> =
                    vec![format!("This submodule {verb} added pointing at commit ").into()];
                parts.extend(sha(new));
                parts.push(".".into());
                parts.extend(suffix);
                items.push(
                    item(Octicon::DiffAdded, t.color_new, paragraph(parts)).into_any_element(),
                );
            }
            (Some(old), None) => {
                let mut parts: Vec<Inline> = vec![
                    format!("This submodule {verb} removed while it was pointing at commit ")
                        .into(),
                ];
                parts.extend(sha(old));
                parts.push(".".into());
                parts.extend(suffix);
                items.push(
                    item(Octicon::DiffRemoved, t.color_deleted, paragraph(parts))
                        .into_any_element(),
                );
            }
            (None, None) => {}
        }
        // working-tree changes inside the submodule
        if diff.status.untracked_changes || diff.status.modified_changes {
            let changes = match (diff.status.untracked_changes, diff.status.modified_changes) {
                (true, true) => "modified and untracked",
                (true, false) => "untracked",
                _ => "modified",
            };
            items.push(
                item(
                    Octicon::FileDiff,
                    t.color_new,
                    paragraph(vec![
                        format!(
                            "This submodule has {changes} changes. Those changes must be committed \
                             inside of the submodule before they can be part of the parent \
                             repository."
                        )
                        .into(),
                    ]),
                )
                .into_any_element(),
            );
        }
        let full_path = diff.full_path.clone();
        let name = self.state.read(cx).product_name().to_string();
        let open_action = diff.url.is_some().then(|| {
            crate::no_changes::suggested_action_card(
                crate::no_changes::SuggestedAction {
                    id: "open-submodule",
                    on_click: Rc::new(move |_, cx| {
                        Dispatcher::open_submodule(full_path.clone(), cx)
                    }),
                    title: format!("Open this submodule on {name}").into(),
                    description: Some(
                        format!(
                            "You can open this submodule on {name} as a normal repository to \
                             manage and commit any changes in it."
                        )
                        .into(),
                    ),
                    hint: "".into(),
                    keys: &[],
                    button_label: mac_or("Open Repository", "Open repository").into(),
                    primary: true,
                    menu: None,
                },
                cx,
            )
        });
        div()
            .id("submodule-diff")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .p(zpx(40.))
            .text_size(FONT_SIZE())
            .text_color(t.text)
            .child(
                div()
                    .w_full()
                    .max_w(zpx(600.))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(zpx(20.))
                            .font_weight(FontWeight::LIGHT)
                            .mb(SPACING_DOUBLE())
                            .child("Submodule changes"),
                    )
                    .children(items)
                    .children(open_action),
            )
            .with_scrollbar()
            .into_any_element()
    }
}

/// A diff row as the highlighter sees it: kind, old and new line numbers, text.
type RowSource = (corvene_core::DiffLineKind, Option<u32>, Option<u32>, String);
type Tokens = Vec<Vec<corvene_highlight::Span>>;
/// Which file a side's tokens belong to: the highlighter (and grammar set),
/// the path (it picks the grammar) and the contents' hash and length.
type SideKey = ((corvene_highlight::Engine, u64), String, u64, usize);

/// At most this much text is tokenized while building the rows (`preview`;
/// a few ms with the CodeMirror ports or syntect); more waits for the
/// background task.
const SYNC_HIGHLIGHT_BYTES: usize = 192 * 1024;
/// Rows `preview` colours from the top one on screen: a tall window's worth,
/// with room for a first scroll.
const PREVIEW_ROWS: usize = 200;
/// Files whose tokens `SIDE_CACHE` keeps.
const SIDE_CACHE_LEN: usize = 16;

thread_local! {
    /// Tokens of recently highlighted files, most recent last (main thread).
    static SIDE_CACHE: RefCell<std::collections::VecDeque<(SideKey, Arc<Option<Tokens>>)>> =
        RefCell::default();
}

/// One side's whole file, and its tokens when `SIDE_CACHE` has them.
struct Side {
    lines: Arc<Vec<String>>,
    key: SideKey,
    /// what tokenizing it costs, in bytes (`MAX_HIGHLIGHT_BYTES` at most)
    bytes: usize,
    tokens: Option<Arc<Option<Tokens>>>,
}

impl Side {
    fn new(with: (corvene_highlight::Engine, u64), path: &str, lines: Arc<Vec<String>>) -> Self {
        use std::hash::{Hash, Hasher};
        // FxHash: SipHash over a whole file was a noticeable part of a new
        // diff's frame; this is a cache key, not a defence against collisions
        let mut hasher = rustc_hash::FxHasher::default();
        lines.hash(&mut hasher);
        let bytes: usize = lines.iter().map(|l| l.len() + 1).sum();
        let key = (with, path.to_string(), hasher.finish(), lines.len());
        let tokens = SIDE_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            let ix = cache.iter().position(|(k, _)| *k == key)?;
            let entry = cache.remove(ix)?;
            let tokens = entry.1.clone();
            cache.push_back(entry);
            Some(tokens)
        });
        Self {
            lines,
            key,
            bytes: bytes.min(corvene_highlight::MAX_HIGHLIGHT_BYTES),
            tokens,
        }
    }
}

fn remember_sides(sides: Vec<(SideKey, Arc<Option<Tokens>>)>) {
    SIDE_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        for (key, tokens) in sides {
            cache.retain(|(k, _)| *k != key);
            cache.push_back((key, tokens));
            if cache.len() > SIDE_CACHE_LEN {
                cache.pop_front();
            }
        }
    });
}

/// Whether `engine` tokenizes `path` without possibly loading a tree-sitter
/// grammar pack first (seconds for a pack's first unit).
fn tokenizes_in_process(engine: corvene_highlight::Engine, path: &str, first_line: &str) -> bool {
    use corvene_highlight::Engine;
    // a user tree-sitter grammar parses the whole file: no prefix
    if let Some(claim) = corvene_highlight::user::claims(path, first_line) {
        return claim.kind == corvene_highlight::user::ClaimKind::Syntect;
    }
    match engine {
        Engine::GitHubDesktop => true,
        Engine::TreeSitterFallback => corvene_highlight::cm_covers(path, first_line),
        Engine::TreeSitter => false,
    }
}

/// A file's lines up to `getPartialBlobContents(…,
/// MaxHighlightContentLength)`: what gets tokenized.
fn highlight_texts(lines: &[String]) -> Vec<&str> {
    let mut budget = corvene_highlight::MAX_HIGHLIGHT_BYTES;
    lines
        .iter()
        .take_while(|l| {
            let fits = l.len() < budget;
            budget = budget.saturating_sub(l.len() + 1);
            fits
        })
        .map(String::as_str)
        .collect()
}

/// A whole file's tokens.
fn tokenize_side(
    engine: corvene_highlight::Engine,
    path: &str,
    lines: &[String],
) -> Option<Tokens> {
    corvene_highlight::highlight_lines_with(engine, path, highlight_texts(lines))
}

/// What `preview` coloured.
struct Preview {
    /// every row's tokens; rows past the window have none yet
    tokens: Option<Tokens>,
    /// sides it happened to tokenize whole, for `SIDE_CACHE`
    whole: Vec<(SideKey, Arc<Option<Tokens>>)>,
    /// every side (or the rows) was tokenized whole: nothing left to do
    complete: bool,
}

/// Colour the rows in `window` from each side's first lines, up to the last
/// line those rows show (`highlight_prefix`: GHD's tokenizers run top down,
/// so these are the tokens the whole run gives). `None` when that is more
/// than `SYNC_HIGHLIGHT_BYTES`. Only for engines `tokenizes_in_process`
/// accepts, where the whole run is GHD's tokenizers.
fn preview(
    engine: corvene_highlight::Engine,
    path: &str,
    rows: &[RowSource],
    window: std::ops::Range<usize>,
    old: &Option<Side>,
    new: &Option<Side>,
    context_from_shown_side: bool,
) -> Option<Preview> {
    use corvene_core::DiffLineKind as K;
    let cost =
        |texts: &[&str], stop: usize| -> usize { texts[..stop].iter().map(|t| t.len() + 1).sum() };
    let in_window = &rows[window.clone()];
    if old.is_none() && new.is_none() {
        let texts: Vec<&str> = rows
            .iter()
            .map(|(kind, _, _, text)| if *kind == K::Hunk { "" } else { text.as_str() })
            .collect();
        let stop = window.end;
        if cost(&texts, stop) > SYNC_HIGHLIGHT_BYTES {
            return None;
        }
        let tokens = corvene_highlight::highlight_prefix(path, &texts, stop).map(|mut tokens| {
            tokens.resize(rows.len(), Vec::new());
            tokens
        });
        return Some(Preview {
            tokens,
            whole: Vec::new(),
            complete: stop >= rows.len(),
        });
    }
    let mut budget = SYNC_HIGHLIGHT_BYTES;
    let mut whole = Vec::new();
    let mut complete = true;
    let mut prefix = |side: &Option<Side>, line: fn(&RowSource) -> Option<u32>| {
        let side = side.as_ref()?;
        let tokens = match &side.tokens {
            Some(tokens) => tokens.clone(),
            None => {
                let texts = highlight_texts(&side.lines);
                let last = in_window.iter().filter_map(line).max().unwrap_or(0) as usize;
                let stop = last.min(texts.len());
                let cost = cost(&texts, stop);
                if cost > budget {
                    budget = 0;
                    return None;
                }
                budget -= cost;
                let tokens = Arc::new(corvene_highlight::highlight_prefix(path, &texts, stop));
                if stop == texts.len() {
                    whole.push((side.key.clone(), tokens.clone()));
                } else {
                    complete = false;
                }
                tokens
            }
        };
        Some(Side {
            lines: side.lines.clone(),
            key: side.key.clone(),
            bytes: 0,
            tokens: Some(tokens),
        })
    };
    let preview_old = prefix(old, |r| r.1);
    let preview_new = prefix(new, |r| r.2);
    if budget == 0 {
        return None;
    }
    let (tokens, _) = assemble(
        engine,
        path,
        rows,
        preview_old,
        preview_new,
        context_from_shown_side,
    );
    Some(Preview {
        tokens,
        whole,
        complete,
    })
}

/// The rows' tokens (see `DiffView::highlight`), and the sides tokenized
/// on the way, for `SIDE_CACHE`.
#[allow(clippy::type_complexity)]
fn assemble(
    engine: corvene_highlight::Engine,
    path: &str,
    rows: &[RowSource],
    old: Option<Side>,
    new: Option<Side>,
    context_from_shown_side: bool,
) -> (Option<Tokens>, Vec<(SideKey, Arc<Option<Tokens>>)>) {
    use corvene_core::DiffLineKind as K;
    if old.is_none() && new.is_none() {
        // hunk header rows are fed as empty lines so parser state and indices line up
        let texts = rows
            .iter()
            .map(|(kind, _, _, text)| if *kind == K::Hunk { "" } else { text.as_str() });
        return (
            corvene_highlight::highlight_lines_with(engine, path, texts),
            Vec::new(),
        );
    }
    let mut fresh = Vec::new();
    let mut tokenize = |side: Option<Side>| {
        let side = side?;
        let tokens = side.tokens.unwrap_or_else(|| {
            let tokens = Arc::new(tokenize_side(engine, path, &side.lines));
            fresh.push((side.key, tokens.clone()));
            tokens
        });
        Some((side.lines, tokens))
    };
    let old = tokenize(old);
    let new = tokenize(new);
    let has_tokens = |side: &Option<(Arc<Vec<String>>, Arc<Option<Tokens>>)>| {
        side.as_ref().is_some_and(|(_, tokens)| tokens.is_some())
    };
    if !has_tokens(&old) && !has_tokens(&new) {
        return (None, fresh);
    }
    let any_added = rows.iter().any(|r| r.0 == K::Add);
    let any_deleted = rows.iter().any(|r| r.0 == K::Delete);
    // a file line's spans with the raw line they index (tabs intact)
    let pick = |side: &Option<(Arc<Vec<String>>, Arc<Option<Tokens>>)>, line: Option<u32>| {
        let (lines, tokens) = side.as_ref()?;
        let ix = line?.checked_sub(1)? as usize;
        let spans = (**tokens).as_ref()?.get(ix)?.clone();
        Some((spans, lines.get(ix)?.clone()))
    };
    let tokens = rows
        .iter()
        .map(|(kind, old_line, new_line, text)| {
            let picked = match kind {
                K::Add => pick(&new, *new_line),
                K::Delete => pick(&old, *old_line),
                // `792-whitespace-hidden-highlight`: git prints a context
                // line from one of the files; with whitespace hidden the
                // other file's line can differ in its whitespace, and its
                // spans would land on the wrong characters
                K::Context if context_from_shown_side => {
                    let shows = |picked: &Option<(Vec<corvene_highlight::Span>, String)>| {
                        picked.as_ref().is_some_and(|(_, raw)| {
                            crate::diff_view_rows::expand_tabs(raw) == *text
                        })
                    };
                    let old_line = pick(&old, *old_line);
                    if shows(&old_line) {
                        old_line
                    } else {
                        Some(pick(&new, *new_line)).filter(shows).flatten()
                    }
                }
                K::Context if any_added && !any_deleted => pick(&new, *new_line),
                K::Context => pick(&old, *old_line).or_else(|| pick(&new, *new_line)),
                _ => None,
            };
            // tokens index the raw file line; move them through the row's
            // tab expansion
            picked.map_or_else(Vec::new, |(spans, raw)| spans_for_row(spans, &raw, text))
        })
        .collect();
    (Some(tokens), fresh)
}

/// Colours for `rows` from the same file's previous rows and tokens: a row
/// whose kind and text are unchanged keeps its spans, the rest none, until
/// the new tokens arrive (instead of the whole diff flashing uncoloured).
fn carry_over<'a>(
    tokens: &Tokens,
    previous: impl IntoIterator<Item = (corvene_core::DiffLineKind, &'a str)>,
    rows: impl IntoIterator<Item = (corvene_core::DiffLineKind, &'a str)>,
) -> Tokens {
    use std::mem::discriminant;
    let mut by_text = HashMap::new();
    for ((kind, text), spans) in previous.into_iter().zip(tokens) {
        by_text.entry((discriminant(&kind), text)).or_insert(spans);
    }
    rows.into_iter()
        .map(|(kind, text)| {
            by_text
                .get(&(discriminant(&kind), text))
                .map(|spans| (*spans).clone())
                .unwrap_or_default()
        })
        .collect()
}

impl DiffView {
    /// `756-missing-highlighting-hint`: one line above a text diff no
    /// grammar colours, offering the Language Extensions dialog's Find tab
    /// for the file's suffix. Corvene addition; GHD shows such a diff
    /// without comment.
    fn highlighting_hint(&self, snap: &Snapshot, cx: &Context<Self>) -> Option<AnyElement> {
        use corvene_core::extensions::ExtensionsFocus;
        use corvene_core::flags::ids;
        let s = self.state.read(cx);
        if !s.flags.bool(ids::MISSING_HIGHLIGHTING_HINT) || !s.flags.bool(ids::LANGUAGE_EXTENSIONS)
        {
            return None;
        }
        if !matches!(*snap.diff, Diff::Text { .. } | Diff::LargeText { .. })
            || snap.diff.line_count() == 0
        {
            return None;
        }
        let suffix = s.extensions.hint_suffix(&snap.path)?;
        let first = snap
            .contents
            .as_ref()
            .and_then(|c| c.first())
            .map(String::as_str)
            .unwrap_or("");
        let (engine, _) = highlight_engine(s);
        if corvene_highlight::has_builtin_highlighting(engine, &snap.path, first)
            || corvene_highlight::user::claims(&snap.path, first).is_some()
        {
            return None;
        }
        let t = cx.ghd();
        let find_suffix = suffix.clone();
        let dismiss_suffix = suffix.clone();
        Some(
            div()
                .id("diff-highlighting-hint")
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(SPACING_HALF())
                .px(SPACING())
                .py(SPACING_HALF())
                .text_size(FONT_SIZE_SM())
                .bg(t.box_alt_background)
                .border_b_1()
                .border_color(t.box_border)
                .child(octicon(Octicon::Info, t.text_secondary).size(zpx(14.)))
                .child(
                    div()
                        .text_color(t.text_secondary)
                        .child(format!("No syntax highlighting for .{suffix}.")),
                )
                .child(
                    link_button("diff-highlighting-hint-find", "Find an extension…", cx)
                        .text_size(FONT_SIZE_SM())
                        .on_click(move |_, _, cx| {
                            Dispatcher::lookup_extensions_for_suffix(&find_suffix, cx);
                            Dispatcher::open_language_extensions(
                                Some(ExtensionsFocus::Suffix(find_suffix.clone())),
                                None,
                                cx,
                            );
                        }),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .id("diff-highlighting-hint-dismiss")
                        .cursor_pointer()
                        .p(zpx(2.))
                        .rounded(zpx(3.))
                        .hover(move |d| d.bg(t.list_item_hover_background))
                        .child(octicon(Octicon::X, t.text_secondary).size(zpx(14.)))
                        .on_click(move |_, _, cx| {
                            Dispatcher::dismiss_suffix_hint(&dismiss_suffix, cx)
                        }),
                )
                .into_any_element(),
        )
    }
}

/// The highlighter diffs use (Settings › Appearance › Syntax highlighting,
/// offered by `105-tree-sitter-highlighting`) and, when it runs tree-sitter,
/// the grammar set's generation.
pub(crate) fn highlight_engine(s: &AppState) -> (corvene_highlight::Engine, u64) {
    use corvene_core::SyntaxHighlighter;
    use corvene_highlight::Engine;
    let allowed = s
        .flags
        .bool(corvene_core::flags::ids::TREE_SITTER_HIGHLIGHTING);
    let engine = match corvene_core::flags::effective_syntax_highlighter(
        s.settings.syntax_highlighter,
        allowed,
    ) {
        SyntaxHighlighter::GitHubDesktop => Engine::GitHubDesktop,
        SyntaxHighlighter::TreeSitterFallback => Engine::TreeSitterFallback,
        SyntaxHighlighter::TreeSitter => Engine::TreeSitter,
    };
    // the generation covers the tree-sitter packs and the user's language
    // extensions: either changing re-highlights open diffs
    (engine, corvene_highlight::generation())
}

impl DiffView {
    /// What every render does first: the row metrics (zoom, font size and
    /// line height), the find controls, and the snapshot, loading or
    /// re-highlighting when it changed, then the display mode. `None`
    /// while there is no diff to show.
    fn prepare(&mut self, cx: &mut Context<Self>) -> Option<Snapshot> {
        // first: a stacked file whose diff has not arrived costs nothing more
        let snap = self.snapshot(cx)?;
        self.request_pdf_text(&snap.diff, cx);
        self.document_source = snap.document_source.clone();
        self.request_document_view(&snap, cx);
        self.request_rich_view(&snap, cx);
        // View › Zoom changed the row heights the list has cached
        let zoom = crate::theme::sizes::zoom_factor();
        // … and so does `751-diff-font-size`
        let text_size = match self
            .state
            .read(cx)
            .flags
            .number(corvene_core::flags::ids::DIFF_FONT_SIZE)
        {
            0 => FONT_SIZE_SM(),
            size => zpx(size.clamp(9, 16) as f32),
        };
        // … and `758-diff-line-height`
        let line_height = diff_line_height_setting(cx).unwrap_or(20.);
        LINE_HEIGHT.with(|h| h.set(line_height));
        if self.zoom_seen != zoom
            || self.text_size != text_size
            || self.line_height_seen != line_height
        {
            self.zoom_seen = zoom;
            self.text_size = text_size;
            self.line_height_seen = line_height;
            self.list_state.remeasure();
        }
        self.find_controls = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::DIFF_FIND_CONTROLS);
        if self.rows_key.as_ref() != Some(&snap.key) {
            self.load(&snap, cx);
        } else if self.highlighted_with != Some(highlight_engine(self.state.read(cx))) {
            // Settings › Appearance › Syntax highlighting changed, or a
            // tree-sitter grammar pack was loaded or removed
            self.previous_rows = Some(self.rows.clone());
            self.highlight(snap.key.clone(), cx);
        }
        let split = {
            let s = self.state.read(cx);
            // `744-unified-diff-for-added-files`: one side would be empty
            let one_sided = matches!(
                snap.kind,
                FileStatusKind::New | FileStatusKind::Untracked | FileStatusKind::Deleted
            ) && s
                .flags
                .bool(corvene_core::flags::ids::UNIFIED_DIFF_FOR_ADDED_FILES);
            s.settings.show_side_by_side_diff && !one_sided
        };
        self.set_split_mode(split);
        // `1304-diff-no-wrap`: rows are one line high and measured again
        let no_wrap = {
            let s = self.state.read(cx);
            s.flags.bool(corvene_core::flags::ids::DIFF_NO_WRAP) && !s.settings.diff_wrap_lines
        };
        if self.no_wrap != no_wrap {
            self.no_wrap = no_wrap;
            self.h_scroll.reset();
            self.list_state.remeasure();
        }
        Some(snap)
    }

    /// GHD `ui/diff/index.tsx` routing for everything but text rows:
    /// `None` when the diff is text to render as rows.
    fn non_text_panel(&mut self, snap: &Snapshot, cx: &mut Context<Self>) -> Option<AnyElement> {
        Some(match &*snap.diff {
            Diff::Text { .. } | Diff::LargeText { .. } if snap.diff.line_count() > 0 => {
                if matches!(*snap.diff, Diff::LargeText { .. }) && !self.show_large {
                    self.large_diff_panel(cx)
                } else {
                    return None;
                }
            }
            Diff::Text { .. } | Diff::LargeText { .. } | Diff::Empty => self.empty_panel(snap, cx),
            Diff::Binary => self.binary_panel(snap, cx),
            // `755-tga-image-diff` off: GHD does not know TGA images
            Diff::Image { previous, current }
                if [previous, current]
                    .into_iter()
                    .flatten()
                    .any(|b| b.media_type == crate::image_diff::TGA_MEDIA_TYPE)
                    && !self
                        .state
                        .read(cx)
                        .flags
                        .bool(corvene_core::flags::ids::TGA_IMAGE_DIFF) =>
            {
                self.binary_panel(snap, cx)
            }
            // `1322-pdf-diff`
            Diff::Image { .. } if is_pdf_diff(&snap.diff) => {
                if self.wants_pdf_text(&snap.diff, cx) {
                    return Some(match self.pdf_text.as_ref().map(|(_, t)| t) {
                        Some(PdfText::Unreadable) => {
                            self.panel("The text of this PDF file could not be read.", cx)
                        }
                        _ => self.panel("Reading the text of the PDF file…", cx),
                    });
                }
                match self.pdf.clone() {
                    Some(pdf) if pdf.read(cx).failed().is_none() => pdf.into_any_element(),
                    _ => self.binary_panel(snap, cx),
                }
            }
            Diff::Image { .. } => match self.image.clone() {
                Some(image) => image.into_any_element(),
                None => self.panel("This binary file has changed.", cx),
            },
            // `1329-font-diff` off after the file was read: GHD's binary panel
            Diff::Font { .. }
                if !self
                    .state
                    .read(cx)
                    .flags
                    .bool(corvene_core::flags::ids::FONT_DIFF) =>
            {
                self.binary_panel(snap, cx)
            }
            Diff::Font { .. } | Diff::FontSource { .. } => match self.font.clone() {
                Some(FontPane::Font(v)) => v.into_any_element(),
                Some(FontPane::Source(v)) => v.into_any_element(),
                None => self.panel("This binary file has changed.", cx),
            },
            Diff::TooLarge => self.too_large_panel(snap, cx),
            Diff::Submodule(sub) => self.submodule_panel(sub, cx),
            // `1321-table-file-diff`: drawn by `csv_table_view` (the stacked
            // list has no room for sheet tabs)
            Diff::Table { .. } if self.stacked_kind.is_none() => return None,
            Diff::Table { .. } => self.panel("This table file has changed.", cx),
            // `1323-archive-diff`
            Diff::Archive(archive) => match (&archive.unreadable, self.archive.clone()) {
                (Some(reason), _) => self.binary_panel_with(snap, Some(reason.clone()), cx),
                (None, _) if self.stacked_path.is_some() => {
                    crate::archive_diff_view::stacked_summary(
                        snap.repo,
                        &snap.path,
                        archive,
                        self.source,
                        cx,
                    )
                }
                (None, Some(view)) => view.into_any_element(),
                (None, None) => self.panel("This archive has changed.", cx),
            },
            // `1324-document-diff`
            Diff::Document { .. } => self.document_panel(cx),
            // `1331-notebook-diff` / `1332-latex-diff`
            Diff::Notebook { .. } | Diff::Latex { .. } => self.rich_panel(snap, cx),
        })
    }

    /// `1324-document-diff`: the rendered document, or where its Text or
    /// Pages view stands while it is not ready.
    fn document_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        use corvene_core::DocumentDiffView as V;
        if self.stacked_path.is_some() {
            return self.panel(
                "This document changed. Select the file alone to see the document.",
                cx,
            );
        }
        match effective_document_view(self.state.read(cx)) {
            V::Rendered => match self.document.clone() {
                Some(doc) => doc.into_any_element(),
                None => self.panel("This binary file has changed.", cx),
            },
            V::Text => match self.document_text.as_ref().map(|(_, t)| t) {
                Some(PdfText::Unreadable) => {
                    self.panel("The text of this document could not be read.", cx)
                }
                _ => self.panel("Reading the text of the document…", cx),
            },
            V::Pages => match self.document_pages.as_ref().map(|(_, p)| p) {
                Some(DocumentPages::Failed(reason)) => {
                    let reason = reason.clone();
                    self.panel(reason, cx)
                }
                _ => self.panel("Drawing the pages with LibreOffice…", cx),
            },
        }
    }

    /// `1324-document-diff`: the Text or Pages view of the document diff
    /// `diff` (with the generation bit it is keyed by), once ready.
    fn document_view_of(&self, diff: &Arc<Diff>, cx: &App) -> Option<(Arc<Diff>, u64)> {
        use corvene_core::DocumentDiffView as V;
        if self.stacked_path.is_some() || !matches!(**diff, Diff::Document { .. }) {
            return None;
        }
        match effective_document_view(self.state.read(cx)) {
            V::Rendered => None,
            V::Text => match self.document_text.as_ref()? {
                (source, PdfText::Ready(text)) if Arc::ptr_eq(source, diff) => {
                    Some((text.clone(), DOCUMENT_TEXT_GENERATION))
                }
                _ => None,
            },
            V::Pages => match self.document_pages.as_ref()? {
                (source, DocumentPages::Ready(pages)) if Arc::ptr_eq(source, diff) => {
                    Some((pages.clone(), DOCUMENT_PAGES_GENERATION))
                }
                _ => None,
            },
        }
    }

    /// `1324-document-diff`: reads the text, or (`1325-document-pages`)
    /// draws the pages, of the shown document diff in the background.
    fn request_document_view(&mut self, snap: &Snapshot, cx: &mut Context<Self>) {
        use corvene_core::DocumentDiffView as V;
        let Some(source) = snap.document_source.clone() else {
            return;
        };
        if self.stacked_path.is_some() {
            return;
        }
        let view = effective_document_view(self.state.read(cx));
        let fresh = |s: Option<&Arc<Diff>>| s.is_some_and(|s| Arc::ptr_eq(s, &source));
        match view {
            V::Rendered => {}
            V::Text if !fresh(self.document_text.as_ref().map(|(s, _)| s)) => {
                // RTF: git's own diff of the markup
                if let Diff::Document {
                    source: Some(text), ..
                } = &*source
                {
                    let text = Arc::new((**text).clone());
                    self.document_text = Some((source, PdfText::Ready(text)));
                    cx.notify();
                    return;
                }
                self.document_text = Some((source.clone(), PdfText::Reading));
                let job = source.clone();
                let read = cx.background_executor().spawn(async move {
                    let Diff::Document {
                        previous, current, ..
                    } = &*job
                    else {
                        return None;
                    };
                    Some(corvene_core::document_diff::text_diff(
                        previous.as_ref(),
                        current.as_ref(),
                    ))
                });
                self._document_text_task = Some(cx.spawn(async move |this, cx| {
                    let text = read.await;
                    this.update(cx, |this, cx| {
                        if this
                            .document_text
                            .as_ref()
                            .is_some_and(|(s, _)| Arc::ptr_eq(s, &source))
                        {
                            this.document_text = Some((
                                source,
                                text.map_or(PdfText::Unreadable, |d| PdfText::Ready(Arc::new(d))),
                            ));
                            cx.notify();
                        }
                    })
                    .ok();
                }));
            }
            V::Pages if !fresh(self.document_pages.as_ref().map(|(s, _)| s)) => {
                self.document_pages = Some((source.clone(), DocumentPages::Converting));
                let (job, path, kind) = (source.clone(), snap.path.clone(), snap.kind);
                let draw = cx.background_executor().spawn(async move {
                    let Diff::Document {
                        previous, current, ..
                    } = &*job
                    else {
                        return Err("This is not a document.".to_string());
                    };
                    corvene_core::document_pages::pages_diff(
                        &path,
                        kind,
                        previous.as_ref(),
                        current.as_ref(),
                    )
                });
                self._document_pages_task = Some(cx.spawn(async move |this, cx| {
                    let pages = draw.await;
                    this.update(cx, |this, cx| {
                        if this
                            .document_pages
                            .as_ref()
                            .is_some_and(|(s, _)| Arc::ptr_eq(s, &source))
                        {
                            this.document_pages = Some((
                                source,
                                match pages {
                                    Ok(d) => DocumentPages::Ready(Arc::new(d)),
                                    Err(e) => DocumentPages::Failed(e),
                                },
                            ));
                            cx.notify();
                        }
                    })
                    .ok();
                }));
            }
            _ => {}
        }
    }

    /// `1323-archive-diff`: show an archive diff in the archive view (made
    /// on first use, told which archive to open), or let the view go.
    fn load_archive(&mut self, snap: &Snapshot, cx: &mut Context<Self>) {
        let shows_archive = self.stacked_path.is_none()
            && !matches!(self.source, DiffSource::ArchiveEntry(_))
            && matches!(&*snap.diff, Diff::Archive(a) if a.unreadable.is_none());
        let surface = self.source.archive_surface();
        if !shows_archive {
            if self.archive.take().is_some() {
                let repo = snap.repo;
                cx.defer(move |cx| Dispatcher::hide_archive(repo, surface, cx));
            }
            return;
        }
        if self.archive.is_none() {
            let state = self.state.clone();
            self.archive = Some(
                cx.new(|cx| crate::archive_diff_view::ArchiveDiffView::new(state, surface, cx)),
            );
        }
        let (repo, path, diff) = (snap.repo, snap.path.clone(), snap.diff.clone());
        cx.defer(move |cx| Dispatcher::show_archive(repo, surface, path, diff, cx));
    }

    /// `1311-stacked-diff`: how many list items this file's body takes,
    /// after the same preparation as a render.
    pub(crate) fn stacked_prepare(
        &mut self,
        kind: Rc<corvene_core::stacked_diff::StackKind>,
        cx: &mut Context<Self>,
    ) -> StackedShape {
        self.stacked_kind = Some(kind);
        let Some(snap) = self.prepare(cx) else {
            return StackedShape::Loading;
        };
        match &*snap.diff {
            Diff::Text { .. } | Diff::LargeText { .. } if snap.diff.line_count() > 0 => {
                if matches!(*snap.diff, Diff::LargeText { .. }) && !self.show_large {
                    StackedShape::Panel
                } else {
                    StackedShape::Rows(self.row_count())
                }
            }
            _ => StackedShape::Panel,
        }
    }

    /// `1311-stacked-diff`: this file's body for the stacked list, after
    /// [`Self::stacked_prepare`] in the same frame.
    pub(crate) fn stacked_body(&mut self, window: &Window, cx: &mut Context<Self>) -> StackedBody {
        let Some(snap) = self.snapshot(cx) else {
            return StackedBody::Loading;
        };
        if let Some(panel) = self.non_text_panel(&snap, cx) {
            return StackedBody::Panel(panel);
        }
        StackedBody::Rows {
            ctx: self.row_context(&snap, None, window, cx),
            rows: self.rows.clone(),
            split_rows: self.split_rows.clone(),
            split_mode: self.split_mode,
            warnings: self.warnings(&snap, cx),
            hint: self.highlighting_hint(&snap, cx),
        }
    }
}

impl DiffView {
    /// `1311-stacked-diff`: the popovers and menu this file's rows opened
    /// (Diff Settings, the whitespace hint, a context menu), drawn by the
    /// stacked list since this view is not rendered there; they float over
    /// the window.
    pub(crate) fn stacked_overlays(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut out = Vec::new();
        if self.options_open
            && let Some(snap) = self.snapshot(cx)
        {
            out.push(self.options_popover(&snap, window, cx));
        }
        if let Some(anchor) = self.whitespace_hint {
            out.push(self.whitespace_hint_popover(anchor, cx));
        }
        if let Some(menu) = self.context_menu.clone() {
            out.push(menu.into_any_element());
        }
        out
    }

    pub(crate) fn options_open(&self) -> bool {
        self.options_open
    }

    pub(crate) fn close_options(&mut self, cx: &mut Context<Self>) {
        if self.options_open {
            self.options_open = false;
            cx.notify();
        }
    }
}

/// `1305-diff-find-controls`: what the controls after the search box show.
pub(crate) struct FindControls {
    /// "3 of 12" / "No results", once the typed text has been searched.
    pub status: Option<String>,
    /// The arrows search (a text is typed).
    pub can_step: bool,
    /// Aa is on.
    pub case: bool,
}

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// `1305-diff-find-controls`: count, Aa, previous and next after the box,
/// shared by the single-file diff and the stacked list. The buttons keep
/// focus in the box (a blur closes the search).
pub(crate) fn find_controls_row(
    ui: FindControls,
    on_case: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_previous: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_next: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let t = cx.ghd();
    let FindControls {
        status,
        can_step,
        case,
    } = ui;
    let (on_case, on_previous, on_next): (ClickHandler, ClickHandler, ClickHandler) =
        (Box::new(on_case), Box::new(on_previous), Box::new(on_next));
    let button = |id: &'static str, label: &'static str, on: bool, enabled: bool| {
        div()
            .id(id)
            .icon_button_label(label)
            .flex_none()
            .size(zpx(22.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(BORDER_RADIUS())
            .border_1()
            .border_color(if on {
                t.box_border
            } else {
                transparent_black()
            })
            .when(on, |d| d.bg(t.box_selected_background))
            .when(enabled, |d| {
                d.cursor_pointer().hover(|d| d.bg(t.box_hover_background))
            })
            .when(!enabled, |d| d.opacity(0.5))
            // keep the focus (and the search) in the text box
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
    };
    div()
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(zpx(2.))
        .font_family(crate::theme::ui_font())
        .text_size(FONT_SIZE_SM())
        .child(
            div()
                .id("diff-search-count")
                .flex_none()
                .min_w(zpx(58.))
                .text_color(t.text_secondary)
                .whitespace_nowrap()
                .children(status),
        )
        .child(
            button("diff-search-case", "Match Case", case, true)
                .aria_toggled(if case { Toggled::True } else { Toggled::False })
                .text_color(t.text)
                .child("Aa")
                .on_click(move |ev, window, cx| on_case(ev, window, cx)),
        )
        .child(
            button("diff-search-previous", "Previous Match", false, can_step)
                .child(octicon(Octicon::ChevronUp, t.text))
                .when(can_step, |d| {
                    d.on_click(move |ev, window, cx| on_previous(ev, window, cx))
                }),
        )
        .child(
            button("diff-search-next", "Next Match", false, can_step)
                .child(octicon(Octicon::ChevronDown, t.text))
                .when(can_step, |d| {
                    d.on_click(move |ev, window, cx| on_next(ev, window, cx))
                }),
        )
        .into_any_element()
}

/// The ⌘F box (`DiffSearchInput`, `.diff-search`): top right, hanging from
/// the header; with `controls` (`1305-diff-find-controls`) wider, the
/// controls after the box. Shared by the single-file diff and the stacked
/// list.
pub(crate) fn search_bar(
    input: &Entity<InputState>,
    controls: Option<AnyElement>,
    window: &Window,
    cx: &App,
) -> Div {
    let t = cx.ghd();
    let text_box = crate::widgets::filter_text_box("diff-search", input, None, window, cx);
    let bar = div()
        .absolute()
        .top_0()
        .right(SPACING())
        .p(SPACING_HALF())
        .bg(t.background)
        .border_1()
        .border_t_0()
        .border_color(t.box_border)
        .rounded_b(BORDER_RADIUS());
    match controls {
        None => bar.w(zpx(250.)).child(text_box),
        Some(controls) => bar
            .w(zpx(340.))
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING_HALF())
            .child(div().flex_1().min_w_0().child(text_box))
            .child(controls),
    }
}

/// `1311-stacked-diff`: what one file's body takes in the stacked list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StackedShape {
    /// The diff has not arrived: one placeholder item.
    Loading,
    /// Text rows, one item each.
    Rows(usize),
    /// A panel (binary, image, empty, too large, submodule): one item.
    Panel,
}

/// `1311-stacked-diff`: one file's body, rendered item by item.
pub(crate) enum StackedBody {
    Loading,
    Rows {
        ctx: Rc<RowContext>,
        rows: Rc<Vec<Row>>,
        split_rows: Rc<Vec<SplitRow>>,
        split_mode: bool,
        /// `DiffContentsWarning`, shown under the file's header.
        warnings: Option<AnyElement>,
        /// The "highlighting…" hint, shown under the header too.
        hint: Option<AnyElement>,
    },
    Panel(AnyElement),
}

impl Render for DiffView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let loading = self.loading_overlay(cx);
        self.sync_review_composer(window, cx);
        let Some(snap) = self.prepare(cx) else {
            return div()
                .relative()
                .size_full()
                .children(loading)
                .into_any_element();
        };
        let background = cx.ghd().background;
        let options = self
            .options_open
            .then(|| self.options_popover(&snap, window, cx));
        let body: AnyElement = match self.non_text_panel(&snap, cx) {
            Some(panel) => panel,
            None => match self.csv_table(&snap, window, cx) {
                Some(table) => self.csv_table_view(&snap, table, window, cx),
                None => self.text_diff(&snap, window, cx),
            },
        };
        let hint = self.highlighting_hint(&snap, cx);
        div()
            .id("diff-container")
            .track_focus(&self.focus_handle)
            .key_context("Diff")
            .on_action(cx.listener(|this, _: &Find, window, cx| {
                this.show_search(window, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
                this.select_all_text(cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|this, _: &Copy, _, cx| {
                if this.copy_text_selection(cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                if this.searching {
                    this.close_search(cx);
                    cx.stop_propagation();
                }
            }))
            .relative()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(background)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    if !this.focus_handle.contains_focused(window, cx) {
                        window.focus(&this.focus_handle, cx);
                    }
                    this.clear_text_selection_unless_on_text(ev.position, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if ev.pressed_button == Some(MouseButton::Left) {
                    this.drag_text_selection(ev.position, cx);
                }
            }))
            .children(hint)
            .child(body)
            .children(options)
            .children(
                self.whitespace_hint
                    .map(|anchor| self.whitespace_hint_popover(anchor, cx)),
            )
            .children(self.context_menu.clone())
            .children(loading)
            .into_any_element()
    }
}

impl DiffView {
    /// GHD `DiffContentsWarning`: bidi characters / line-endings notices.
    fn warnings(&self, snap: &Snapshot, cx: &Context<Self>) -> Option<AnyElement> {
        let t = cx.ghd();
        let warnings = snap.diff.warnings()?;
        let mut items: Vec<AnyElement> = Vec::new();
        if warnings.hidden_bidi {
            items.push(
                paragraph(vec![
                    "This diff contains bidirectional Unicode text that may be interpreted or \
                     compiled differently than what appears below. To review, open the file in \
                     an editor that reveals hidden Unicode characters."
                        .into(),
                    Inline::Element(
                        link_button(
                            "bidi-learn-more",
                            "Learn more about bidirectional Unicode characters",
                            cx,
                        )
                        .on_click(|_, _, cx| {
                            corvene_core::Dispatcher::open_url("https://github.co/hiddenchars", cx)
                        })
                        .into_any_element(),
                    ),
                ])
                .into_any_element(),
            );
        }
        // `757-typechange-diff`
        if let Some((old, new)) = &warnings.type_change
            && self.locked_type_change(&snap.diff, cx)
        {
            items.push(
                paragraph(vec![
                    format!(
                        "This file changed from {} to {}. Its lines cannot be selected one by one.",
                        file_type_name(old),
                        file_type_name(new)
                    )
                    .into(),
                ])
                .into_any_element(),
            );
        }
        // `796-lfs-text-diff`
        if warnings.lfs_contents && self.source == DiffSource::WorkingDirectory {
            items.push(
                paragraph(vec![
                    "This file is stored in Git LFS: its contents are compared. Its lines \
                     cannot be selected one by one."
                        .into(),
                ])
                .into_any_element(),
            );
        }
        // `1322-pdf-diff`
        if warnings.pdf_text {
            items.push(
                paragraph(vec![
                    "This is the text on the pages of the PDF file. Its lines cannot be \
                     selected one by one."
                        .into(),
                ])
                .into_any_element(),
            );
        }
        // `1324-document-diff`
        if warnings.document_text {
            items.push(
                paragraph(vec![
                    "This is the text of the document. Its lines cannot be selected one by one."
                        .into(),
                ])
                .into_any_element(),
            );
        }
        // `795-lfs-image-previews`
        if warnings.lfs_not_downloaded {
            items.push(
                paragraph(vec![
                    "This image is stored in Git LFS and its contents are not downloaded, so \
                     the LFS pointer files are compared instead."
                        .into(),
                ])
                .into_any_element(),
            );
        }
        if let Some(change) = &warnings.line_endings {
            items.push(
                paragraph(vec![
                    format!("This file uses '{}' line endings, but", change.from).into(),
                    Inline::Element(
                        link_button("line-endings-docs", "Git is configured to convert them", cx)
                            .on_click(|_, _, cx| {
                                corvene_core::Dispatcher::open_url("https://docs.github.com/get-started/git-basics/configuring-git-to-handle-line-endings", cx)
                            })
                            .into_any_element(),
                    ),
                    format!("to '{}' the next time the file is checked out.", change.to).into(),
                ])
                .into_any_element(),
            );
        }
        if items.is_empty() {
            return None;
        }
        let count = items.len();
        Some(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .px(SPACING_DOUBLE())
                .py(SPACING())
                .bg(t.file_warning_background)
                .border_b_1()
                .border_color(t.file_warning_border)
                .font_family(crate::theme::ui_font())
                .text_size(FONT_SIZE())
                .text_color(t.text)
                .children(items.into_iter().enumerate().map(|(ix, body)| {
                    div()
                        .flex()
                        .flex_row()
                        .items_start()
                        .gap(SPACING())
                        .when(ix + 1 < count, |d| {
                            d.mb(SPACING())
                                .pb(SPACING())
                                .border_b_1()
                                .border_color(t.file_warning_border)
                        })
                        .child(
                            div()
                                .flex_none()
                                .pt(zpx(2.))
                                .child(octicon(Octicon::Alert, t.file_warning)),
                        )
                        .child(div().flex_1().min_w_0().child(body))
                }))
                .into_any_element(),
        )
    }

    /// `1304-diff-no-wrap`: start a frame of the sideways scroll (the
    /// widest line estimated from the longest one in characters).
    fn h_scroll_frame(&mut self, window: &Window) -> HScroll {
        let key = Rc::as_ptr(&self.rows) as usize;
        let changed = self.longest_line.0 != key;
        if changed {
            let longest = self
                .rows
                .iter()
                .map(|r| r.text.chars().count() + if r.no_newline { 30 } else { 0 })
                .max()
                .unwrap_or(0);
            self.longest_line = (key, longest);
        }
        let text = window.text_system();
        let font = text.resolve_font(&gpui_kit::font(mono_font()));
        let column = text
            .em_advance(font, self.text_size)
            .unwrap_or(self.text_size * 0.6);
        // the five-column prefix and a little room after the last character
        let estimate = column * (self.longest_line.1 + 7) as f32;
        if changed {
            self.h_scroll.reset_width(estimate);
        }
        self.h_scroll
            .begin_frame(estimate, column, self.list_state.viewport_bounds());
        self.h_scroll.clone()
    }

    /// `1304-diff-no-wrap`: sideways wheel input over the rows. A trackpad
    /// swipe that is mostly sideways scrolls only sideways (the list keeps
    /// still); ⇧ + a wheel's vertical ticks scroll sideways (macOS already
    /// sends those as sideways ticks, which the scrollbar animates over the
    /// text, so only ticks over the gutters are taken here).
    fn sideways_wheel(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        self.sideways_wheel_for(self.h_scroll.clone(), cx)
    }

    /// Sideways wheel and trackpad scrolling of `h` over this element.
    fn sideways_wheel_for(&self, h: HScroll, cx: &Context<Self>) -> impl IntoElement + use<> {
        let view = cx.entity_id();
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                window.on_mouse_event(move |ev: &ScrollWheelEvent, phase, _, cx| {
                    if phase != DispatchPhase::Capture || !bounds.contains(&ev.position) {
                        return;
                    }
                    let (dx, only) = match ev.delta {
                        ScrollDelta::Pixels(d) => (d.x, d.x.abs() > d.y.abs()),
                        ScrollDelta::Lines(l) if ev.modifiers.shift && l.x == 0. => {
                            (px(l.y * 40.), true)
                        }
                        ScrollDelta::Lines(l)
                            if !gpui_kit::base::ScrollbarHandle::viewport_bounds(&h)
                                .contains(&ev.position) =>
                        {
                            (px(l.x * 40.), false)
                        }
                        ScrollDelta::Lines(_) => (Pixels::ZERO, false),
                    };
                    if dx == Pixels::ZERO {
                        return;
                    }
                    if only {
                        cx.stop_propagation();
                    }
                    if h.scroll_by(-dx) {
                        cx.notify(view);
                    }
                });
            },
        )
        .absolute()
        .inset_0()
    }

    /// `1318-csv-table-diff`: the table to show instead of the text rows
    /// (a CSV or TSV text diff with the table view picked). A file that
    /// cannot be read as a table gets the text diff with a notice.
    fn csv_table(
        &mut self,
        snap: &Snapshot,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Result<Rc<CsvTable>, String>> {
        let file_tables = matches!(*snap.diff, Diff::Table { .. });
        let delimiter = if file_tables {
            b','
        } else {
            corvene_core::csv_diff::table_delimiter(&snap.path)?
        };
        if !file_tables {
            let s = self.state.read(cx);
            if !s.flags.bool(corvene_core::flags::ids::CSV_TABLE_DIFF) || !s.settings.csv_table_diff
            {
                return None;
            }
        }
        if !file_tables
            && (!matches!(*snap.diff, Diff::Text { .. } | Diff::LargeText { .. }) || snap.as_text)
        {
            return None;
        }
        let sheet = file_tables.then(|| (snap.diff.clone(), self.table_sheet(&snap.diff)));
        let same_sheet =
            |a: &Option<(Arc<Diff>, String)>, b: &Option<(Arc<Diff>, String)>| match (a, b) {
                (Some((a, x)), Some((b, y))) => Arc::ptr_eq(a, b) && x == y,
                (None, None) => true,
                _ => false,
            };
        let same = |a: &Option<Arc<Vec<String>>>, b: &Option<Arc<Vec<String>>>| match (a, b) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        let measure = (self.text_size.to_f64() as f32).to_bits();
        let zoom = f32::from(zpx(1.)).to_bits();
        if let Some(((key, contents, old_contents, at, at_sheet), table)) = &self.csv
            && *key == snap.key
            && same(contents, &snap.contents)
            && same(old_contents, &snap.old_contents)
            && *at == (measure, zoom)
            && same_sheet(at_sheet, &sheet)
        {
            return Some(table.clone());
        }
        let table = match (&*snap.diff, &sheet) {
            (Diff::Table { .. }, Some((_, name))) if name.is_empty() => {
                Err("This file has no sheets or tables.".to_string())
            }
            (Diff::Table { previous, current }, Some((_, name))) => {
                corvene_core::csv_diff::sheet_diff(previous.as_ref(), current.as_ref(), name)
                    .map_err(|e| match e {
                        corvene_core::csv_diff::TableError::Empty => {
                            "This sheet is empty.".to_string()
                        }
                        e => format!("This sheet cannot be shown as a table ({e})."),
                    })
            }
            _ => {
                // a side whose contents were not loaded (a pull request
                // preview or a ref comparison has no old contents) gets the
                // text diff
                let new = if snap.kind == FileStatusKind::Deleted {
                    None
                } else {
                    Some(snap.contents.as_ref()?.join("\n"))
                };
                let old = if snap.kind.is_new_or_untracked() {
                    None
                } else {
                    Some(snap.old_contents.as_ref()?.join("\n"))
                };
                corvene_core::csv_diff::table_diff(old.as_deref(), new.as_deref(), delimiter)
                    .map(|diff| (diff, false))
                    .map_err(|e| {
                        format!("Showing the text diff: the table could not be read ({e}).")
                    })
            }
        };
        let table = table.map(|(diff, truncated)| {
            let text = window.text_system();
            let font = text.resolve_font(&gpui_kit::font(mono_font()));
            let column = text
                .em_advance(font, self.text_size)
                .unwrap_or(self.text_size * 0.6);
            let mut table = CsvTable::new(Rc::new(diff), column);
            table.truncated = truncated;
            let table = Rc::new(table);
            let same_file = self.csv.as_ref().is_some_and(|((k, .., at_sheet), _)| {
                k.0 == snap.key.0
                    && k.1 == snap.key.1
                    && at_sheet.as_ref().map(|(_, n)| n) == sheet.as_ref().map(|(_, n)| n)
            });
            if !same_file {
                self.csv_h_scroll.reset();
            }
            self.csv_list
                .reset_with_uniform_height(table.diff.rows.len(), DIFF_LINE_HEIGHT());
            self.csv_h_scroll.reset_width(table.width());
            table
        });
        self.csv = Some((
            (
                snap.key.clone(),
                snap.contents.clone(),
                snap.old_contents.clone(),
                (measure, zoom),
                sheet,
            ),
            table.clone(),
        ));
        Some(table)
    }

    /// `1321-table-file-diff`: the sheets of a table file's diff, worked
    /// out once per diff.
    fn table_sheets(&mut self, diff: &Arc<Diff>) -> Rc<Vec<SheetEntry>> {
        if let Some((at, sheets)) = &self.csv_sheets
            && Arc::ptr_eq(at, diff)
        {
            return sheets.clone();
        }
        let sheets = Rc::new(match &**diff {
            Diff::Table { previous, current } => {
                corvene_core::csv_diff::sheet_list(previous.as_ref(), current.as_ref())
            }
            _ => Vec::new(),
        });
        self.csv_sheets = Some((diff.clone(), sheets.clone()));
        sheets
    }

    /// `1321-table-file-diff`: the sheet to show, the picked one while the
    /// file has it, else the first one the change touched.
    fn table_sheet(&mut self, diff: &Arc<Diff>) -> String {
        let sheets = self.table_sheets(diff);
        if let Some(name) = &self.csv_sheet
            && sheets.iter().any(|s| &s.name == name)
        {
            return name.clone();
        }
        sheets
            .iter()
            .find(|s| s.status != SheetStatus::Same)
            .or(sheets.first())
            .map(|s| s.name.clone())
            .unwrap_or_default()
    }

    /// `1321-table-file-diff`: the sheet tabs of a table file's diff.
    fn sheet_tabs(&mut self, snap: &Snapshot, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !matches!(*snap.diff, Diff::Table { .. }) {
            return None;
        }
        let sheets = self.table_sheets(&snap.diff);
        let selected = self.table_sheet(&snap.diff);
        let view = cx.entity().downgrade();
        Some(
            crate::csv_table_diff::sheet_tabs(
                &sheets,
                &selected,
                move |name, cx| {
                    let name = name.to_string();
                    let _ = view.update(cx, |this, cx| {
                        this.csv_sheet = Some(name);
                        cx.notify();
                    });
                },
                cx,
            )
            .into_any_element(),
        )
    }

    /// `1318-csv-table-diff`: the table with its pinned header, or the
    /// text diff under a notice when the file is not a readable table.
    fn csv_table_view(
        &mut self,
        snap: &Snapshot,
        table: Result<Rc<CsvTable>, String>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tabs = self.sheet_tabs(snap, cx);
        let table = match table {
            Ok(table) => table,
            Err(reason) if tabs.is_some() => {
                let body = self.panel(&reason, cx);
                return div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .children(tabs)
                    .child(body)
                    .into_any_element();
            }
            Err(reason) => {
                let body = self.text_diff(snap, window, cx);
                let t = cx.ghd();
                let notice = div()
                    .flex_none()
                    .px(SPACING())
                    .py(SPACING_HALF())
                    .border_b_1()
                    .border_color(t.diff_border)
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(reason);
                return div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(notice)
                    .child(body)
                    .into_any_element();
            }
        };
        let h = self.csv_h_scroll.clone();
        h.begin_frame(table.width(), px(0.), self.csv_list.viewport_bounds());
        let t = cx.ghd();
        let header = crate::csv_table_diff::header_row(&table, &h, cx);
        let cut_off = table.truncated.then(|| {
            div()
                .flex_none()
                .px(SPACING())
                .py(SPACING_HALF())
                .border_b_1()
                .border_color(t.diff_border)
                .text_size(FONT_SIZE_SM())
                .text_color(t.text_secondary)
                .child(format!(
                    "Only the first {} rows of each version are shown.",
                    crate::format::format_number_with(
                        corvene_git::table_file::MAX_ROWS as f64,
                        ",",
                        "."
                    )
                ))
        });
        let rows_table = table.clone();
        let rows_h = h.clone();
        div()
            .id("csv-table")
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .font_family(mono_font())
            .text_size(self.text_size)
            .line_height(DIFF_LINE_HEIGHT())
            .text_color(t.diff_text)
            .flex()
            .flex_col()
            .children(tabs)
            .children(cut_off)
            .child(header)
            .child(
                list(
                    self.csv_list.clone(),
                    move |ix, _window, cx| match rows_table.diff.rows.get(ix) {
                        Some(row) => {
                            crate::csv_table_diff::table_row(&rows_table, ix, row, &rows_h, cx)
                        }
                        None => div().into_any_element(),
                    },
                )
                .flex_1()
                .min_h_0()
                .w_full()
                .pr(gutter(&self.csv_list)),
            )
            .child(scrollbar("csv-scrollbar", self.csv_list.clone()))
            .child(self.sideways_wheel_for(h.clone(), cx))
            .child(scrollbar("csv-hscrollbar", h).horizontal())
            .into_any_element()
    }

    /// `1316-diff-moved-lines` / `1317-diff-stylistic-changes`: the marks
    /// of the current rows (empty when both are off), recomputed when the
    /// rows or the options change.
    fn line_marks(&mut self, path: &str, cx: &App) -> LineMarks {
        let options = AppState::try_global(cx)
            .map(|s| {
                let s = s.read(cx);
                ClassOptions {
                    moved: s.flags.bool(corvene_core::flags::ids::DIFF_MOVED_LINES)
                        && s.settings.diff_mark_moved_lines,
                    stylistic: s
                        .flags
                        .bool(corvene_core::flags::ids::DIFF_STYLISTIC_CHANGES)
                        && s.settings.diff_mark_stylistic_changes,
                }
            })
            .unwrap_or_default();
        if let Some(cache) = &self.line_marks
            && Rc::ptr_eq(&cache.rows, &self.rows)
            && cache.options == options
        {
            return cache.marks.clone();
        }
        let marks = Rc::new(if options.any() {
            let lines: Vec<ClassLine> = self
                .rows
                .iter()
                .map(|r| ClassLine {
                    kind: r.kind,
                    text: &r.text,
                    old: r.old,
                    new: r.new,
                })
                .collect();
            classify(&lines, path, options)
        } else {
            Vec::new()
        });
        self.line_marks = Some(MarksCache {
            rows: self.rows.clone(),
            options,
            marks: marks.clone(),
        });
        marks
    }

    /// Everything the rows need from the view this frame (`RowContext`).
    fn row_context(
        &mut self,
        snap: &Snapshot,
        review: Option<Rc<crate::review_threads::ReviewUi>>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Rc<RowContext> {
        // `canSelect`: working-directory files that are not conflicted.
        // a binary file shown as text cannot be committed line by line
        // (`749-binary-diff-as-text`: the partial patch is taken without `--text`)
        let selectable = self.source == DiffSource::WorkingDirectory
            && snap.kind != FileStatusKind::Conflicted
            && !snap.as_text
            && !self.locked_type_change(&snap.diff, cx);
        let mut groups: BTreeMap<u32, DiffSelectionType> = BTreeMap::new();
        for row in self.rows.iter() {
            if let Some((start, len)) = row.group {
                groups
                    .entry(start)
                    .or_insert_with(|| snap.selection.range_kind(start, len));
            }
        }
        Rc::new(RowContext {
            repo: snap.repo,
            path: snap.path.clone(),
            selection: snap.selection.clone(),
            groups,
            selectable,
            hide_whitespace: snap.hide_whitespace,
            temp: self.temp,
            hovered_group: self.hovered_group,
            view: cx.weak_entity(),
            tokens: self.tokens.clone(),
            inner: self.unified_inner.clone(),
            marks: self.line_marks(&snap.path, cx),
            search: self.search_index(),
            show_check_marks: AppState::try_global(cx)
                .is_none_or(|s| s.read(cx).settings.show_diff_check_marks),
            line_number_width: line_number_width(max_line_number(&self.rows)),
            text_selection: self.text_selection_snapshot(),
            text_bounds: {
                // rows re-record their bounds as they paint
                self.text_bounds.borrow_mut().clear();
                self.text_bounds.clone()
            },
            show_whitespace: AppState::try_global(cx).is_some_and(|s| {
                s.read(cx)
                    .flags
                    .bool(corvene_core::flags::ids::DIFF_SHOW_WHITESPACE)
            }),
            wide_hunk_handle: AppState::try_global(cx).is_some_and(|s| {
                s.read(cx)
                    .flags
                    .bool(corvene_core::flags::ids::WIDE_HUNK_HANDLE)
            }),
            review,
            h_scroll: self.no_wrap.then(|| self.h_scroll_frame(window)),
        })
    }

    /// The virtualized rows plus the search box (`DiffSearchInput`).
    fn text_diff(
        &mut self,
        snap: &Snapshot,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // `348-pull-request-review`: the threads' row map (borrows `cx`
        // mutably, so before the theme is borrowed)
        let review = self.review_ui(snap, cx);
        let ctx = self.row_context(snap, review.clone(), window, cx);
        let t = cx.ghd();
        let rows = self.rows.clone();
        let split_rows = self.split_rows.clone();
        let split_mode = self.split_mode;
        let warnings = self.warnings(snap, cx);
        // `348-pull-request-review`: the threads without a row, above them
        let unplaced = review.as_ref().and_then(|review| {
            let s = self.state.read(cx);
            let file = crate::review_threads::review_state_of(s, snap.repo)?.shown_file()?;
            review.unplaced_panel(file, self.review.unplaced_open, cx)
        });
        let search = self
            .search_input
            .clone()
            .filter(|_| self.searching)
            .map(|input| {
                let controls = self.find_controls.then(|| {
                    let typed = input.read(cx).value().to_string();
                    self.find_controls_row(&typed, cx)
                });
                search_bar(&input, controls, window, cx)
            });
        div()
            .id("diff")
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .font_family(mono_font())
            .text_size(self.text_size)
            .line_height(DIFF_LINE_HEIGHT())
            .text_color(t.diff_text)
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.end_selection(cx);
                    this.end_text_selection(cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.end_selection(cx);
                    this.end_text_selection(cx);
                }),
            )
            .flex()
            .flex_col()
            .children(warnings)
            .children(unplaced)
            .child(
                list(self.list_state.clone(), move |ix, _window, cx| {
                    let (row, count) = if split_mode {
                        (
                            split_rows.get(ix).map(|row| {
                                let el = render_split_row(&ctx, ix, row, &rows, cx);
                                let (before, after) = row.unified_rows();
                                decorate_row(&ctx, el, before.into_iter().chain(after), cx)
                            }),
                            split_rows.len(),
                        )
                    } else {
                        (
                            rows.get(ix).map(|row| {
                                let el = render_row(&ctx, ix, row, cx);
                                decorate_row(&ctx, el, std::iter::once(ix), cx)
                            }),
                            rows.len(),
                        )
                    };
                    match row {
                        // the grid's inner container ends in a 1 px
                        // `--diff-border-color` line under the last row
                        Some(row) if ix + 1 == count => div()
                            .border_b_1()
                            .border_color(cx.ghd().diff_border)
                            .child(row)
                            .into_any_element(),
                        Some(row) => row,
                        None => div().into_any_element(),
                    }
                })
                .flex_1()
                .min_h_0()
                .w_full()
                .pr(gutter(&self.list_state)),
            )
            .child(scrollbar("diff-scrollbar", self.list_state.clone()))
            .when(self.no_wrap, |d| {
                d.child(self.sideways_wheel(cx))
                    .child(scrollbar("diff-hscrollbar", self.h_scroll.clone()).horizontal())
            })
            .children(search)
            .into_any_element()
    }
}

/// `348-pull-request-review`: the row with its threads and the composer
/// underneath (a column in one list item, so the list's indices stay the
/// rows').
fn decorate_row(
    ctx: &RowContext,
    row: AnyElement,
    unified: impl Iterator<Item = usize>,
    cx: &App,
) -> AnyElement {
    let Some(review) = ctx.review.as_ref() else {
        return row;
    };
    let mut seen = std::collections::BTreeSet::new();
    let decorations: Vec<AnyElement> = unified
        .filter(|ix| seen.insert(*ix))
        .filter_map(|ix| review.decorations(ix, cx))
        .collect();
    if decorations.is_empty() {
        return row;
    }
    div()
        .w_full()
        .flex()
        .flex_col()
        .child(row)
        .children(decorations)
        .into_any_element()
}

impl DiffView {
    /// `348-pull-request-review`: the threads' row map for this render
    /// (`None` for every other source).
    fn review_ui(
        &mut self,
        snap: &Snapshot,
        cx: &mut Context<Self>,
    ) -> Option<Rc<crate::review_threads::ReviewUi>> {
        if self.source != DiffSource::Review {
            return None;
        }
        let (
            threads,
            bodies,
            by_row,
            composer,
            can_write,
            has_pending,
            busy,
            error,
            show_resolved,
            signed_out,
            layout_key,
        ) = {
            let s = self.state.read(cx);
            let review = crate::review_threads::review_state_of(s, snap.repo)?;
            let file = review.shown_file()?;
            // the threads are shared with the rows as one snapshot per version
            if self
                .review
                .threads
                .as_ref()
                .is_none_or(|(v, _, _)| *v != review.threads_version)
                || self
                    .review
                    .threads
                    .as_ref()
                    .is_some_and(|(_, _, b)| b.len() != review.bodies.len())
            {
                self.review.threads = Some((
                    review.threads_version,
                    Rc::new(review.threads.clone()),
                    Rc::new(review.bodies.clone()),
                ));
            }
            let (_, threads, bodies) = self.review.threads.clone().expect("set above");
            // which row carries which thread: new numbers on the right,
            // old numbers on the left
            let mut by_new: HashMap<u32, usize> = HashMap::new();
            let mut by_old: HashMap<u32, usize> = HashMap::new();
            for (ix, row) in self.rows.iter().enumerate() {
                if row.kind == DiffLineKind::Hunk {
                    continue;
                }
                if let Some(n) = row.new {
                    by_new.entry(n).or_insert(ix);
                }
                if let Some(o) = row.old {
                    by_old.entry(o).or_insert(ix);
                }
            }
            use corvene_core::pull_request_review::DiffSide;
            use corvene_core::review_anchor::ThreadAnchor;
            let mut by_row: HashMap<usize, Vec<corvene_core::review_anchor::AnchoredThread>> =
                HashMap::new();
            for anchored in &file.anchors {
                if let ThreadAnchor::Line { side, line, .. } = anchored.anchor {
                    let row = match side {
                        DiffSide::Right => by_new.get(&line),
                        DiffSide::Left => by_old.get(&line),
                    };
                    if let Some(row) = row {
                        by_row.entry(*row).or_default().push(anchored.clone());
                    }
                }
            }
            let composer = review.composer.as_ref().and_then(|target| {
                let row = match target.side {
                    DiffSide::Right => by_new.get(&target.line),
                    DiffSide::Left => by_old.get(&target.line),
                }?;
                let input = self
                    .review
                    .composer
                    .as_ref()
                    .filter(|(t, _)| t == target)
                    .map(|(_, input)| input.clone())?;
                Some((target.clone(), *row, input))
            });
            let can_write = !review.signed_out && review.busy.is_none();
            let layout_key = format!(
                "{}|{:?}|{}|{}|{:?}|{}|{:?}|{:?}|{}",
                review.threads_version,
                review.composer,
                self.review.reply_boxes.len(),
                self.review.expanded.len(),
                self.review.editing.as_ref().map(|(id, _)| id),
                review.show_resolved,
                review.busy,
                review.error,
                file.anchors.len(),
            );
            (
                threads,
                bodies,
                by_row,
                composer,
                can_write,
                review.pending_review.is_some() || review.pending_count() > 0,
                review.busy.clone(),
                review.error.clone(),
                review.show_resolved,
                review.signed_out,
                layout_key,
            )
        };
        if self.review.layout_key.as_ref() != Some(&layout_key) {
            self.review.layout_key = Some(layout_key);
            self.list_state.remeasure();
        }
        // the authors' avatars, once each
        let wanted: Vec<String> = threads
            .iter()
            .flat_map(|t| t.comments.iter())
            .filter_map(|c| c.author.as_ref()?.avatar_url.clone())
            .filter(|url| !url.is_empty() && !self.review.avatars.contains(url))
            .collect();
        if !wanted.is_empty() {
            self.review.avatars.extend(wanted.iter().cloned());
            cx.spawn(async move |_, cx| {
                cx.update(|cx| {
                    for url in wanted {
                        Dispatcher::request_avatar_url(&url, cx);
                    }
                });
            })
            .detach();
        }
        Some(Rc::new(crate::review_threads::ReviewUi {
            repo: snap.repo,
            view: cx.weak_entity(),
            threads,
            bodies,
            by_row,
            composer,
            can_write,
            has_pending,
            busy,
            error,
            show_resolved,
            expanded: self.review.expanded.clone(),
            reply_boxes: self.review.reply_boxes.clone(),
            editing: self.review.editing.clone(),
            signed_out,
        }))
    }

    /// The composer's text box follows the state's target: made (and
    /// focused) for a new target, dropped when the composer closes.
    fn sync_review_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.source != DiffSource::Review {
            return;
        }
        let (repo, target) = {
            let s = self.state.read(cx);
            let Some(id) = s.selected else {
                return;
            };
            let target =
                crate::review_threads::review_state_of(s, id).and_then(|r| r.composer.clone());
            (id, target)
        };
        match target {
            None => {
                if self.review.composer.take().is_some() {
                    cx.notify();
                }
            }
            Some(target) => {
                if self
                    .review
                    .composer
                    .as_ref()
                    .is_some_and(|(t, _)| *t == target)
                {
                    return;
                }
                let keep_text = self
                    .review
                    .composer
                    .as_ref()
                    .filter(|(t, _)| t.path == target.path && t.side == target.side)
                    .map(|(_, input)| input.read(cx).value().to_string());
                let input = cx.new(|cx| {
                    let mut state = TextareaState::new(window, cx)
                        .placeholder("Leave a comment")
                        .rows(3);
                    if let Some(text) = keep_text {
                        state.set_value(text, window, cx);
                    }
                    state
                });
                cx.subscribe(&input, move |_, input, ev: &InputEvent, cx| {
                    if let InputEvent::PressEnter {
                        secondary: true, ..
                    } = ev
                    {
                        let text = input.read(cx).value().to_string();
                        Dispatcher::add_review_comment(repo, text, false, cx);
                    }
                })
                .detach();
                window.focus(&input.read(cx).focus_handle(cx), cx);
                self.review.composer = Some((target, input));
                cx.notify();
            }
        }
    }

    pub(crate) fn toggle_thread_expanded(&mut self, id: &str, cx: &mut Context<Self>) {
        let mut set = (*self.review.expanded).clone();
        if !set.remove(id) {
            set.insert(id.to_string());
        }
        self.review.expanded = Rc::new(set);
        cx.notify();
    }

    pub(crate) fn open_reply_box(
        &mut self,
        id: &str,
        input: Entity<TextareaState>,
        cx: &mut Context<Self>,
    ) {
        let mut boxes = (*self.review.reply_boxes).clone();
        boxes.insert(id.to_string(), input);
        self.review.reply_boxes = Rc::new(boxes);
        cx.notify();
    }

    pub(crate) fn close_reply_box(&mut self, id: &str, cx: &mut Context<Self>) {
        let mut boxes = (*self.review.reply_boxes).clone();
        if boxes.remove(id).is_some() {
            self.review.reply_boxes = Rc::new(boxes);
            cx.notify();
        }
    }

    pub(crate) fn start_comment_edit(
        &mut self,
        id: &str,
        input: Entity<TextareaState>,
        cx: &mut Context<Self>,
    ) {
        self.review.editing = Some((id.to_string(), input));
        cx.notify();
    }

    pub(crate) fn cancel_comment_edit(&mut self, cx: &mut Context<Self>) {
        if self.review.editing.take().is_some() {
            cx.notify();
        }
    }

    pub(crate) fn toggle_unplaced_threads(&mut self, cx: &mut Context<Self>) {
        self.review.unplaced_open = !self.review.unplaced_open;
        cx.notify();
    }
}

/// The selected lines joined with newlines: `row_text` gives a row's text in
/// the selected column (`None` = nothing on that side, skipped), the first
/// and last rows are cut at the selection's columns.
pub(crate) fn join_selected_text<'a>(
    sel: &TextSelectionSnapshot,
    row_text: impl Fn(usize) -> Option<&'a str>,
) -> String {
    let mut lines: Vec<&str> = Vec::new();
    for ix in sel.start.row..=sel.end.row {
        let Some(text) = row_text(ix) else {
            continue;
        };
        let start = if ix == sel.start.row {
            sel.start.col.min(text.len())
        } else {
            0
        };
        let end = if ix == sel.end.row {
            sel.end.col.min(text.len())
        } else {
            text.len()
        };
        // stay on char boundaries whatever the mouse math did
        let start = floor_char_boundary(text, start);
        let end = floor_char_boundary(text, end).max(start);
        lines.push(&text[start..end]);
    }
    lines.join("\n")
}

/// The largest char boundary `<= index` (`str::floor_char_boundary` is unstable).
fn floor_char_boundary(text: &str, index: usize) -> usize {
    let mut i = index.min(text.len());
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// A selection of `lines` (ascending selection indices), consecutive lines
/// as one range each.
fn selection_of_lines(lines: &[u32]) -> DiffSelection {
    let mut selection = DiffSelection::none();
    let mut run: Option<(u32, u32)> = None;
    for &line in lines {
        run = match run {
            Some((start, len)) if start + len == line => Some((start, len + 1)),
            Some((start, len)) => {
                selection = selection.with_range(start, len, true);
                Some((line, 1))
            }
            None => Some((line, 1)),
        };
    }
    if let Some((start, len)) = run {
        selection = selection.with_range(start, len, true);
    }
    selection
}

/// `1331-notebook-diff` / `1332-latex-diff`
mod rich_text;

#[cfg(test)]
mod tests {
    #[::core::prelude::v1::test]
    fn unchanged_rows_keep_their_colours_until_the_new_tokens_arrive() {
        use super::carry_over;
        use corvene_core::DiffLineKind as K;
        use corvene_highlight::{Span, TokenClass};
        let span = |class| vec![Span { range: 0..2, class }];
        let tokens = vec![span(TokenClass::Keyword), span(TokenClass::String), vec![]];
        let previous = [(K::Context, "fn a"), (K::Add, "\"s\""), (K::Delete, "x")];
        let rows = [
            (K::Add, "fn a"),
            (K::Add, "\"s\""),
            (K::Context, "fn a"),
            (K::Add, "new"),
        ];
        assert_eq!(
            carry_over(&tokens, previous, rows),
            vec![
                vec![],
                span(TokenClass::String),
                span(TokenClass::Keyword),
                vec![]
            ]
        );
    }

    #[::core::prelude::v1::test]
    fn the_preview_colours_the_window_as_the_whole_run_does() {
        use super::{Side, assemble, preview};
        use corvene_core::DiffLineKind as K;
        use corvene_highlight::Engine;
        use std::sync::Arc;
        let with = (Engine::GitHubDesktop, 0);
        let mut text = vec!["/* a comment".to_string(), "still comment */".to_string()];
        text.extend((0..50).map(|i| format!("let x{i} = \"s\"; // {i}")));
        let lines = Arc::new(text);
        let rows: Vec<_> = lines
            .iter()
            .enumerate()
            .map(|(i, l)| {
                (
                    K::Context,
                    Some(i as u32 + 1),
                    Some(i as u32 + 1),
                    l.clone(),
                )
            })
            .collect();
        let side = || Some(Side::new(with, "preview-test.js", lines.clone()));
        let (whole, _) = assemble(with.0, "preview-test.js", &rows, side(), side(), false);
        let whole = whole.unwrap();
        let part = preview(
            with.0,
            "preview-test.js",
            &rows,
            0..3,
            &side(),
            &side(),
            false,
        )
        .unwrap();
        let tokens = part.tokens.unwrap();
        assert!(!part.complete && part.whole.is_empty());
        assert_eq!(tokens.len(), rows.len());
        assert_eq!(tokens[..3], whole[..3]);
        assert!(!whole[10].is_empty() && tokens[10].is_empty());
        let all = preview(
            with.0,
            "preview-test.js",
            &rows,
            0..rows.len(),
            &side(),
            &side(),
            false,
        )
        .unwrap();
        assert!(all.complete && all.whole.len() == 2);
        assert_eq!(all.tokens.unwrap(), whole);
    }

    #[::core::prelude::v1::test]
    fn context_rows_take_the_side_they_show() {
        use super::{Side, assemble};
        use corvene_core::DiffLineKind as K;
        use corvene_highlight::Engine;
        use std::sync::Arc;
        let with = (Engine::GitHubDesktop, 0);
        // whitespace hidden: the context row is the new file's indented line
        let old = Arc::new(vec![
            "fn main() {".to_string(),
            "let a = 1;".to_string(),
            "let b = 2;".to_string(),
            "}".to_string(),
        ]);
        let new = Arc::new(vec![
            "fn main() {".to_string(),
            "    let a = 1;".to_string(),
            "}".to_string(),
        ]);
        let rows = vec![
            (K::Hunk, None, None, "@@ -1,4 +1,3 @@".to_string()),
            (K::Context, Some(1), Some(1), "fn main() {".to_string()),
            (K::Context, Some(2), Some(2), "    let a = 1;".to_string()),
            (K::Delete, Some(3), None, "let b = 2;".to_string()),
            (K::Context, Some(4), Some(3), "}".to_string()),
        ];
        let sides = || {
            (
                Some(Side::new(with, "ws-test.rs", old.clone())),
                Some(Side::new(with, "ws-test.rs", new.clone())),
            )
        };
        let starts = |tokens: &Vec<corvene_highlight::Span>| -> Vec<usize> {
            tokens.iter().map(|s| s.range.start).collect()
        };
        // GHD: the old line's spans, four characters off
        let (o, n) = sides();
        let ghd = assemble(with.0, "ws-test.rs", &rows, o, n, false)
            .0
            .unwrap();
        assert_eq!(starts(&ghd[2])[0], 0);
        let (o, n) = sides();
        let fixed = assemble(with.0, "ws-test.rs", &rows, o, n, true).0.unwrap();
        assert_eq!(starts(&fixed[2])[0], 4);
        assert_eq!(fixed[1], ghd[1]);
    }

    #[::core::prelude::v1::test]
    fn sides_come_from_the_cache_the_second_time() {
        use super::{Side, assemble, remember_sides};
        use corvene_core::DiffLineKind as K;
        use corvene_highlight::{Engine, TokenClass};
        use std::sync::Arc;
        let with = (Engine::GitHubDesktop, 0);
        let lines = Arc::new(vec!["fn main() {".to_string(), "}".to_string()]);
        let rows = vec![
            (K::Hunk, None, None, "@@ -0,0 +1,2 @@".to_string()),
            (K::Add, None, Some(1), "fn main() {".to_string()),
            (K::Add, None, Some(2), "}".to_string()),
        ];
        let side = Side::new(with, "cache-test.rs", lines.clone());
        assert!(side.tokens.is_none());
        let (tokens, fresh) = assemble(with.0, "cache-test.rs", &rows, None, Some(side), false);
        let tokens = tokens.expect("rust is highlighted");
        assert!(tokens[0].is_empty());
        assert_eq!(tokens[1][0].class, TokenClass::Keyword);
        assert_eq!(fresh.len(), 1);
        remember_sides(fresh);
        let side = Side::new(with, "cache-test.rs", lines);
        assert!(side.tokens.is_some());
        let (again, fresh) = assemble(with.0, "cache-test.rs", &rows, None, Some(side), false);
        assert!(fresh.is_empty());
        assert_eq!(again, Some(tokens));
    }

    #[::core::prelude::v1::test]
    fn selected_text_cuts_first_and_last_rows_and_skips_empty_sides() {
        use super::{Column, TextPos, TextSelectionSnapshot, join_selected_text};
        let rows = ["@@ -1,3 +1,3 @@", "fn main() {", "    héllo();", "}"];
        let text = |ix: usize| rows.get(ix).copied();
        let sel = TextSelectionSnapshot {
            start: TextPos { row: 1, col: 3 },
            end: TextPos { row: 2, col: 9 },
            column: Column::Before,
        };
        assert_eq!(join_selected_text(&sel, text), "main() {\n    héll");
        // a column inside the two-byte "é" snaps back to its start
        let sel = TextSelectionSnapshot {
            start: TextPos { row: 2, col: 4 },
            end: TextPos { row: 2, col: 6 },
            column: Column::Before,
        };
        assert_eq!(join_selected_text(&sel, text), "h");
        let sel = TextSelectionSnapshot {
            start: TextPos { row: 0, col: 0 },
            end: TextPos { row: 3, col: 1 },
            column: Column::After,
        };
        let one_side = |ix: usize| if ix == 2 { None } else { rows.get(ix).copied() };
        assert_eq!(
            join_selected_text(&sel, one_side),
            "@@ -1,3 +1,3 @@\nfn main() {\n}"
        );
        let sel = TextSelectionSnapshot {
            start: TextPos { row: 0, col: 0 },
            end: TextPos { row: 3, col: 99 },
            column: Column::Before,
        };
        assert_eq!(join_selected_text(&sel, text), rows.join("\n"));
    }
}
