//! What surrounds the merge editor's panes: the toolbar, pane headers, the
//! ribbons between panes, each conflict's accept bar, the overview ruler, the
//! blame card, the side picker and the "changed on disk" banner.

use corvene_core::merge_editor::{BlameLoad, MergeSession};
use corvene_core::persistence::MergeEditorLayout;
use corvene_core::{AppState, Dispatcher};
use corvene_git::merge_file::PickReason;
use corvene_merge::{ChunkKind, DocMode, Resolution};
use corvene_models::{GitStatusEntry, ManualConflictResolution};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{FileKey, MergeEditorView, PaneGeometry, PaneId, chunk_color};
use crate::icons::{Octicon, octicon};
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, mono_font};
use crate::widgets::{GhdTooltip, button, button_disabled, primary_button, small_button};

/// The width of the gutter holding ribbons.
const RIBBON_GUTTER_WIDTH: f32 = 42.;
/// The overview ruler's width.
const RULER_WIDTH: f32 = 12.;
/// A pane header's height.
const HEADER_HEIGHT: f32 = 24.;

pub(super) fn thin_gutter(cx: &App) -> Div {
    div().w(zpx(1.)).flex_none().bg(cx.ghd().box_border)
}

/// A toolbar button that shows as pressed while its setting is on.
fn toggle(
    id: &'static str,
    label: &'static str,
    tooltip: &'static str,
    on: bool,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.ghd();
    small_button(id, label, cx)
        .ghd_tooltip(tooltip)
        .when(on, |d| d.border_color(t.color_new).text_color(t.color_new))
}

fn chunk_button(id: SharedString, label: &'static str, cx: &App) -> Stateful<Div> {
    let t = cx.ghd();
    div()
        .id(id)
        .px(zpx(5.))
        .h(zpx(16.))
        .flex()
        .items_center()
        .rounded(zpx(3.))
        .cursor_pointer()
        .bg(t.box_background)
        .border_1()
        .border_color(t.box_border_contrast)
        .text_size(zpx(10.))
        .text_color(t.text)
        .font_family(crate::theme::ui_font())
        .hover(|s| s.bg(t.box_alt_background))
        .child(label)
}

impl MergeEditorView {
    pub(super) fn toolbar(
        &mut self,
        key: &FileKey,
        session: &MergeSession,
        layout: MergeEditorLayout,
        show_base: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = cx.ghd().clone();
        let id = key.0;
        let file = self.open.get(key);
        let conflicts = file.map_or(0, |f| {
            f.doc
                .chunks
                .iter()
                .filter(|c| c.kind == ChunkKind::Conflict)
                .count()
        });
        let unresolved = file.map_or(0, |f| f.live.unresolved);
        let automatic = file.map_or(0, |f| {
            f.live
                .unresolved_chunks()
                .filter(|l| {
                    f.doc
                        .chunks
                        .get(l.id as usize)
                        .is_some_and(|c| c.auto.is_some())
                })
                .count()
        });
        let ignore_whitespace = self.state.read(cx).settings.merge_editor_ignore_whitespace;
        let status = if unresolved == 0 {
            "No conflicts remaining".to_string()
        } else {
            format!(
                "{} of {} conflict{} left",
                unresolved,
                conflicts.max(unresolved),
                if conflicts.max(unresolved) == 1 {
                    ""
                } else {
                    "s"
                }
            )
        };
        let editable = file.is_some();
        let (prev_key, next_key, wand_key, mark_key) =
            (key.clone(), key.clone(), key.clone(), key.clone());
        let mark = if unresolved == 0 && editable {
            primary_button("merge-mark-resolved", "Mark as Resolved", false, cx)
                .on_click(cx.listener(move |this, _, _, cx| this.mark_resolved(&mark_key, cx)))
                .into_any_element()
        } else {
            button_disabled("merge-mark-resolved", "Mark as Resolved", cx).into_any_element()
        };
        div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING_HALF())
            .h(zpx(36.))
            .px(SPACING())
            .border_b_1()
            .border_color(t.box_border)
            .bg(t.box_alt_background)
            .child(octicon(Octicon::GitMerge, t.color_conflicted))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(FONT_SIZE())
                    .text_color(t.text)
                    .child(session.path.clone()),
            )
            .when(editable, |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(FONT_SIZE_SM())
                        .text_color(if unresolved == 0 {
                            t.color_new
                        } else {
                            t.color_conflicted
                        })
                        .child(status),
                )
                .child(
                    small_button("merge-previous-conflict", "↑", cx)
                        .ghd_tooltip("Previous conflict (⇧F7)")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.step_conflict(&prev_key, false, window, cx)
                        })),
                )
                .child(
                    small_button("merge-next-conflict", "↓", cx)
                        .ghd_tooltip("Next conflict (F7)")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.step_conflict(&next_key, true, window, cx)
                        })),
                )
                .when(automatic > 0, |d| {
                    let label = format!("Resolve {automatic} automatically");
                    d.child(
                        small_button("merge-resolve-automatically", label, cx)
                            .ghd_tooltip(
                                "Conflicts where the sides changed different words or only \
                                 whitespace",
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.resolve_automatically(&wand_key, window, cx)
                            })),
                    )
                })
                .child(
                    toggle(
                        "merge-toggle-stacked",
                        "Stacked",
                        "Current and Incoming above the Result",
                        layout == MergeEditorLayout::Stacked,
                        cx,
                    )
                    .on_click(move |_, _, cx| {
                        Dispatcher::set_merge_editor_layout(
                            if layout == MergeEditorLayout::Stacked {
                                MergeEditorLayout::ThreePane
                            } else {
                                MergeEditorLayout::Stacked
                            },
                            cx,
                        )
                    }),
                )
                .when(layout == MergeEditorLayout::ThreePane, |d| {
                    d.child(
                        toggle(
                            "merge-toggle-base",
                            "Base",
                            "Show the common ancestor",
                            show_base,
                            cx,
                        )
                        .on_click(move |_, _, cx| {
                            Dispatcher::set_merge_editor_show_base(!show_base, cx)
                        }),
                    )
                })
                .child(
                    toggle(
                        "merge-toggle-whitespace",
                        "Whitespace",
                        "Treat changes that only differ in whitespace as the same",
                        ignore_whitespace,
                        cx,
                    )
                    .on_click(move |_, _, cx| {
                        Dispatcher::set_merge_editor_ignore_whitespace(!ignore_whitespace, cx);
                        Dispatcher::merge_editor_whitespace_changed(id, cx);
                    }),
                )
                .child(mark)
            })
    }

    fn header(&self, label: String, detail: Option<String>, cx: &App) -> Div {
        let t = cx.ghd();
        div()
            .flex_none()
            .h(zpx(HEADER_HEIGHT))
            .px(SPACING_HALF())
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING_HALF())
            .overflow_hidden()
            .border_b_1()
            .border_color(t.box_border)
            .text_size(FONT_SIZE_SM())
            .child(div().flex_none().text_color(t.text).child(label))
            .children(detail.map(|d| {
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(t.text_secondary)
                    .child(d)
            }))
    }

    pub(super) fn side_column(
        &mut self,
        key: &FileKey,
        pane: PaneId,
        layout: MergeEditorLayout,
        text_size: Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        let labels = &self.open[key].doc.labels;
        let stacked = layout == MergeEditorLayout::Stacked;
        let (label, detail) = match pane {
            PaneId::Base => ("Base", labels.base.clone()),
            PaneId::Ours if stacked => ("Current", labels.ours.clone()),
            PaneId::Ours => ("Ours", labels.ours.clone()),
            _ if stacked => ("Incoming", labels.theirs.clone()),
            _ => ("Theirs", labels.theirs.clone()),
        };
        let card = self.blame_card(key, pane, cx);
        let body = self.pane_body(key, pane, text_size, cx);
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.header(label.to_string(), detail, cx))
            .child(body.children(card))
    }

    pub(super) fn result_column(
        &mut self,
        key: &FileKey,
        session: &MergeSession,
        layout: MergeEditorLayout,
        text_size: Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        let dirty = session.dirty();
        let markers_only = self.open[key].doc.mode == DocMode::MarkersOnly;
        let detail = if markers_only {
            Some("Only the conflict markers could be read".to_string())
        } else if dirty {
            Some("Saving…".to_string())
        } else {
            None
        };
        let bars = self.accept_bars(key, layout, cx);
        let ruler = self.ruler(key, cx);
        let body = self.pane_body(key, PaneId::Result, text_size, cx);
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.header("Result".to_string(), detail, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .child(body.children(bars))
                    .child(ruler),
            )
    }

    /// The buttons over each conflict that is in view.
    fn accept_bars(
        &mut self,
        key: &FileKey,
        layout: MergeEditorLayout,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let file = &self.open[key];
        let Some(g) = PaneGeometry::of(file.pane(PaneId::Result), cx) else {
            return Vec::new();
        };
        let stacked = layout == MergeEditorLayout::Stacked;
        let mut bars = Vec::new();
        for (chunk, live) in file.doc.chunks.iter().zip(&file.live.chunks) {
            if chunk.kind != ChunkKind::Conflict {
                continue;
            }
            let line = file
                .result_lines
                .get(chunk.id as usize)
                .map_or(0, |l| l.start);
            let y = g.line_y(line as f32) - g.origin;
            if y < -g.line_height || y > g.height {
                continue;
            }
            let id = chunk.id;
            let action = |label: &'static str,
                          name: &str,
                          resolution: Resolution,
                          cx: &mut Context<Self>| {
                let key = key.clone();
                chunk_button(SharedString::from(format!("merge-{name}-{id}")), label, cx).on_click(
                    cx.listener(move |this, _, window, cx| {
                        this.accept(&key, id, resolution, window, cx)
                    }),
                )
            };
            let mut bar = div()
                .absolute()
                .right(zpx(4.))
                .top(y)
                .flex()
                .flex_row()
                .gap(zpx(2.));
            if live.resolution == Resolution::Unresolved {
                bar = bar
                    .child(action(
                        if stacked { "Current" } else { "Ours" },
                        "ours",
                        Resolution::Ours,
                        cx,
                    ))
                    .child(action(
                        if stacked { "Incoming" } else { "Theirs" },
                        "theirs",
                        Resolution::Theirs,
                        cx,
                    ))
                    .child(action("Both", "both", Resolution::OursThenTheirs, cx))
                    .child(action("Both ↑", "both-rev", Resolution::TheirsThenOurs, cx));
                if file.doc.has_base {
                    bar = bar.child(action("Base", "base", Resolution::Base, cx));
                }
                bar = bar.child(action("Ignore", "ignore", Resolution::Ignore, cx));
                if let Some(auto) = &chunk.auto {
                    bar = bar.child(action("Auto", "auto", auto.resolution, cx));
                }
            } else {
                bar = bar.child(action("Reset", "reset", Resolution::Unresolved, cx));
            }
            bars.push(bar.into_any_element());
        }
        bars
    }

    /// Conflicts along the result's whole length; a click jumps to one.
    fn ruler(&mut self, key: &FileKey, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = cx.ghd().clone();
        let file = &self.open[key];
        let total = file.result_line_count.max(1) as f32;
        let mut ruler = div()
            .id("merge-ruler")
            .relative()
            .flex_none()
            .w(zpx(RULER_WIDTH))
            .border_l_1()
            .border_color(t.box_border);
        for (chunk, live) in file.doc.chunks.iter().zip(&file.live.chunks) {
            if !chunk.kind.is_change() {
                continue;
            }
            let Some(lines) = file.result_lines.get(chunk.id as usize) else {
                continue;
            };
            let id = chunk.id;
            let key = key.clone();
            let conflict = chunk.kind == ChunkKind::Conflict;
            let color = chunk_color(chunk.kind, live.resolution, &t);
            ruler = ruler.child(
                div()
                    .id(("merge-ruler-tick", id as usize))
                    .absolute()
                    .left(zpx(if conflict { 1. } else { 4. }))
                    .right(zpx(1.))
                    .top(relative(lines.start as f32 / total))
                    .min_h(zpx(if conflict { 4. } else { 2. }))
                    .h(relative(lines.len() as f32 / total))
                    .bg(color)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.jump_to_chunk(&key, id, window, cx)
                    })),
            );
        }
        ruler
    }

    /// Ribbons from a side pane to the result, with that side's accept
    /// button on each unresolved conflict.
    pub(super) fn ribbon_gutter(
        &mut self,
        key: &FileKey,
        side: PaneId,
        result: PaneId,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = cx.ghd().clone();
        let file = &self.open[key];
        let side_pane = (file.editor(side).clone(), file.pane(side).body.clone());
        let result_pane = (file.editor(result).clone(), file.pane(result).body.clone());
        let Some(chunk_side) = side.side() else {
            return div();
        };
        let left_is_side = side == PaneId::Ours;
        let ribbons: Vec<_> = file
            .doc
            .chunks
            .iter()
            .zip(&file.live.chunks)
            .filter(|(c, _)| c.kind.is_change() && c.has_ribbons())
            .filter_map(|(chunk, live)| {
                let lines = file.result_lines.get(chunk.id as usize)?.clone();
                Some((
                    chunk.lines(chunk_side),
                    lines,
                    chunk_color(chunk.kind, live.resolution, &t).opacity(0.28),
                ))
            })
            .collect();
        let painter = canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| {
                let (Some(a), Some(b)) = (
                    PaneGeometry::read(&side_pane.0, side_pane.1.get(), cx),
                    PaneGeometry::read(&result_pane.0, result_pane.1.get(), cx),
                ) else {
                    return;
                };
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    let (x0, x1) = (bounds.left(), bounds.right());
                    let mid = (x0 + x1) / 2.;
                    for (side_lines, result_lines, color) in &ribbons {
                        let (s0, s1) = (
                            a.line_y(side_lines.start as f32),
                            a.line_y(side_lines.end as f32),
                        );
                        let (r0, r1) = (
                            b.line_y(result_lines.start as f32),
                            b.line_y(result_lines.end as f32),
                        );
                        let ((l0, l1), (r0, r1)) = if left_is_side {
                            ((s0, s1), (r0, r1))
                        } else {
                            ((r0, r1), (s0, s1))
                        };
                        if l1.max(r1) < bounds.top() || l0.min(r0) > bounds.bottom() {
                            continue;
                        }
                        let mut path = PathBuilder::fill();
                        path.move_to(point(x0, l0));
                        path.cubic_bezier_to(point(x1, r0), point(mid, l0), point(mid, r0));
                        path.line_to(point(x1, r1));
                        path.cubic_bezier_to(point(x0, l1), point(mid, r1), point(mid, l1));
                        path.close();
                        if let Ok(path) = path.build() {
                            window.paint_path(path, *color);
                        }
                    }
                });
            },
        )
        .absolute()
        .size_full();
        let mut gutter = div()
            .relative()
            .flex_none()
            .w(zpx(RIBBON_GUTTER_WIDTH))
            .border_l_1()
            .border_r_1()
            .border_color(t.box_border)
            .overflow_hidden()
            .child(painter);
        let Some(g) = PaneGeometry::of(file.pane(side), cx) else {
            return gutter;
        };
        let (label, resolution) = if left_is_side {
            ("»", Resolution::Ours)
        } else {
            ("«", Resolution::Theirs)
        };
        let mut buttons = Vec::new();
        for (chunk, live) in file.doc.chunks.iter().zip(&file.live.chunks) {
            if chunk.kind != ChunkKind::Conflict
                || live.resolution != Resolution::Unresolved
                || !chunk.has_ribbons()
            {
                continue;
            }
            let y = g.line_y(chunk.lines(chunk_side).start as f32) - g.origin;
            if y < -g.line_height || y > g.height + zpx(HEADER_HEIGHT) {
                continue;
            }
            let id = chunk.id;
            let accept_key = key.clone();
            buttons.push(
                chunk_button(
                    SharedString::from(format!("merge-ribbon-{side:?}-{id}")),
                    label,
                    cx,
                )
                .absolute()
                .top(y + zpx(HEADER_HEIGHT))
                .when(left_is_side, |d| d.left(zpx(1.)))
                .when(!left_is_side, |d| d.right(zpx(1.)))
                .ghd_tooltip(if left_is_side {
                    "Accept ours"
                } else {
                    "Accept theirs"
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.accept(&accept_key, id, resolution, window, cx)
                })),
            );
        }
        gutter = gutter.children(buttons);
        gutter
    }

    /// Who changed the hovered chunk on this side.
    fn blame_card(&self, key: &FileKey, pane: PaneId, cx: &App) -> Option<Div> {
        let (chunk, side) = self.hovered?;
        if pane.side() != Some(side) {
            return None;
        }
        let t = cx.ghd();
        let session = self
            .state
            .read(cx)
            .repo_states
            .get(&key.0)?
            .merge_editors
            .get(&key.1)?;
        let load = session.blame.get(&(chunk, side))?;
        let rows: Vec<AnyElement> = match load {
            BlameLoad::Loading => vec![div().child("Loading…").into_any_element()],
            BlameLoad::Unavailable => {
                vec![
                    div()
                        .child("No commit to show on this side")
                        .into_any_element(),
                ]
            }
            BlameLoad::Loaded(blames) => blames
                .iter()
                .take(3)
                .map(|blame| {
                    let commit = &blame.commit;
                    div()
                        .flex()
                        .flex_row()
                        .gap(SPACING_HALF())
                        .child(
                            div()
                                .flex_none()
                                .font_family(mono_font())
                                .child(commit.sha.chars().take(7).collect::<String>()),
                        )
                        .child(div().flex_none().child(commit.author_name.clone()))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(t.text_secondary)
                                .child(commit.summary.clone()),
                        )
                        .into_any_element()
                })
                .collect(),
        };
        Some(
            div()
                .absolute()
                .left(SPACING_HALF())
                .right(SPACING_HALF())
                .bottom(SPACING_HALF())
                .p(SPACING_HALF())
                .flex()
                .flex_col()
                .gap(zpx(2.))
                .rounded(BORDER_RADIUS())
                .bg(t.box_background)
                .border_1()
                .border_color(t.box_border_contrast)
                .text_size(FONT_SIZE_SM())
                .text_color(t.text)
                .children(rows),
        )
    }
}

/// The file changed behind unsaved edits.
pub(super) fn external_banner(id: u64, path: &str, cx: &App) -> Div {
    let t = cx.ghd();
    let (reload_path, keep_path) = (path.to_string(), path.to_string());
    div()
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(SPACING_HALF())
        .px(SPACING())
        .py(SPACING_HALF())
        .bg(t.box_alt_background)
        .border_b_1()
        .border_color(t.color_conflicted)
        .text_size(FONT_SIZE())
        .text_color(t.text)
        .child(
            div()
                .flex_1()
                .child("This file changed on disk while you had unsaved edits."),
        )
        .child(
            small_button("merge-external-reload", "Reload", cx).on_click(move |_, _, cx| {
                Dispatcher::reload_merge_from_disk(id, reload_path.clone(), cx)
            }),
        )
        .child(
            small_button("merge-external-keep", "Keep Mine", cx)
                .on_click(move |_, _, cx| Dispatcher::keep_merge_edits(id, keep_path.clone(), cx)),
        )
}

/// Conflicts that cannot be merged line by line take one side whole.
pub(super) fn side_picker(id: u64, session: &MergeSession, state: &AppState, cx: &App) -> Div {
    let t = cx.ghd();
    let reason = match session.pick {
        Some(PickReason::Binary) => "This is a binary file, so it can't be merged line by line.",
        Some(PickReason::BinaryMergeDriver) => {
            "Git attributes mark this file as one that can't be merged line by line."
        }
        Some(PickReason::Lfs) => "This file is stored in Git LFS, so only one side can be kept.",
        Some(PickReason::Encoding) => {
            "This file's working tree encoding isn't UTF-8, so it can't be edited here."
        }
        Some(PickReason::Submodule) => "This is a submodule, so only one commit can be kept.",
        Some(PickReason::Symlink) => "This is a symbolic link, so only one target can be kept.",
        Some(PickReason::Deleted) | None => "One side deleted this file and the other changed it.",
    };
    let status = state
        .repo_states
        .get(&id)
        .and_then(|rs| rs.status.as_deref())
        .and_then(|st| st.files.iter().find(|f| f.path == session.path))
        .map(|f| f.status.clone());
    let label = |entry: Option<GitStatusEntry>, side: Option<&String>, fallback: &str| {
        let verb = match entry {
            Some(GitStatusEntry::Deleted) => "Delete the file, as on",
            _ => "Keep the file from",
        };
        format!("{verb} {}", side.map_or(fallback, String::as_str))
    };
    let labels = session.inputs.as_ref().map(|i| &i.labels);
    let ours = label(
        status.as_ref().map(|s| s.us()),
        labels.and_then(|l| l.ours.as_ref()),
        "ours",
    );
    let theirs = label(
        status.as_ref().map(|s| s.them()),
        labels.and_then(|l| l.theirs.as_ref()),
        "theirs",
    );
    let pick = |name: &'static str, text: String, side: ManualConflictResolution| {
        let path = session.path.clone();
        button(name, text, cx)
            .on_click(move |_, _, cx| Dispatcher::pick_merge_side(id, path.clone(), side, cx))
    };
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(SPACING())
        .child(
            div()
                .text_size(FONT_SIZE())
                .text_color(t.text)
                .child(reason),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .gap(SPACING_HALF())
                .child(pick(
                    "merge-pick-ours",
                    ours,
                    ManualConflictResolution::Ours,
                ))
                .child(pick(
                    "merge-pick-theirs",
                    theirs,
                    ManualConflictResolution::Theirs,
                )),
        )
}
