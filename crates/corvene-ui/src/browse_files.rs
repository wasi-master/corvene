//! Corvene `1117-browse-files` and `1118-quick-edit` (desktop/desktop#13831,
//! #20440): a read-only file browser for any commit or the working tree, and
//! a plain editor for a working file. GitHub Desktop has neither.
//!
//! Shown in place of the repository view. The state and the reads are in
//! `corvene_core::browse_files`; rows are flattened here from the sorted
//! paths on each change of the filter or the folds.

use std::sync::Arc;

use corvene_core::browse_files::{BrowseContent, BrowseFilesState};
use corvene_core::filter::fuzzy_score;
use corvene_core::{AppState, Dispatcher};
use corvene_highlight::Span;
use gpui_kit::component::input::{InputState, Textarea, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::context_menu::MenuItem;
use crate::diff_view::DIFF_LINE_HEIGHT;
use crate::diff_view_rows::{expand_tabs, line_number_width, spans_for_row, token_color};
use crate::icons::{Octicon, octicon};
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, GhdTheme, mono_font};
use crate::widgets::{button, primary_button, text_box};

const TREE_WIDTH: f32 = 300.;
const ROW_HEIGHT: f32 = 26.;
/// A filter lists this many matches at most.
const MAX_MATCHES: usize = 2000;

/// One row of the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeRow {
    Folder {
        path: String,
        name: String,
        depth: usize,
        collapsed: bool,
    },
    File {
        path: String,
        name: String,
        depth: usize,
    },
}

/// The rows of sorted `files`, skipping what a collapsed folder hides.
pub fn tree_rows(files: &[String], collapsed: &std::collections::HashSet<String>) -> Vec<TreeRow> {
    let mut rows = Vec::new();
    let mut open: Vec<&str> = Vec::new();
    let mut hidden_under: Option<String> = None;
    for path in files {
        if let Some(prefix) = &hidden_under {
            if path.starts_with(prefix.as_str()) {
                continue;
            }
            hidden_under = None;
        }
        let parts: Vec<&str> = path.split('/').collect();
        let folders = &parts[..parts.len() - 1];
        let common = open.iter().zip(folders).take_while(|(a, b)| a == b).count();
        open.truncate(common);
        let mut hidden = false;
        for (depth, folder) in folders.iter().enumerate().skip(common) {
            let folder_path = folders[..=depth].join("/");
            let is_collapsed = collapsed.contains(&folder_path);
            rows.push(TreeRow::Folder {
                name: folder.to_string(),
                depth,
                collapsed: is_collapsed,
                path: folder_path.clone(),
            });
            open.push(folder);
            if is_collapsed {
                hidden_under = Some(format!("{folder_path}/"));
                // the next path decides again what is open
                open.pop();
                hidden = true;
                break;
            }
        }
        if !hidden {
            rows.push(TreeRow::File {
                name: parts[parts.len() - 1].to_string(),
                path: path.clone(),
                depth: folders.len(),
            });
        }
    }
    rows
}

/// What the highlighted lines were computed from.
#[derive(Clone, PartialEq)]
struct HighlightKey {
    path: String,
    contents: (usize, usize),
    engine: (corvene_highlight::Engine, u64),
}

pub struct BrowseFilesView {
    state: Entity<AppState>,
    filter: Entity<InputState>,
    tree_scroll: UniformListScrollHandle,
    text_scroll: UniformListScrollHandle,
    spans: Option<(HighlightKey, Arc<Vec<Vec<Span>>>)>,
    highlighting: Option<(HighlightKey, Task<()>)>,
    editor: Entity<TextareaState>,
    /// The file and read the editor holds, and the text it started from.
    editing: Option<(String, u64, String)>,
}

impl BrowseFilesView {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter files"));
        cx.observe(&filter, |_, _, cx| cx.notify()).detach();
        let editor = cx.new(|cx| TextareaState::new(window, cx).rows(40));
        cx.observe(&editor, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            filter,
            tree_scroll: UniformListScrollHandle::new(),
            text_scroll: UniformListScrollHandle::new(),
            spans: None,
            highlighting: None,
            editor,
            editing: None,
        }
    }

    fn highlight(&mut self, path: &str, lines: &Arc<Vec<String>>, cx: &mut Context<Self>) {
        let engine = crate::diff_view::highlight_engine(self.state.read(cx));
        let key = HighlightKey {
            path: path.to_string(),
            contents: (Arc::as_ptr(lines) as usize, lines.len()),
            engine,
        };
        if self.spans.as_ref().is_some_and(|(k, _)| *k == key)
            || self.highlighting.as_ref().is_some_and(|(k, _)| *k == key)
        {
            return;
        }
        let (lines, path) = (lines.clone(), path.to_string());
        let work = cx.background_executor().spawn(async move {
            corvene_highlight::highlight_lines_with(
                engine.0,
                &path,
                lines.iter().map(String::as_str),
            )
        });
        let task_key = key.clone();
        let task = cx.spawn(async move |this, cx| {
            let spans = work.await;
            this.update(cx, |this, cx| {
                this.highlighting = None;
                this.spans = spans.map(|s| (task_key, Arc::new(s)));
                cx.notify();
            })
            .ok();
        });
        self.highlighting = Some((key, task));
    }

    fn rows(&self, browse: &BrowseFilesState, cx: &App) -> Vec<TreeRow> {
        let query = self.filter.read(cx).value().trim().to_string();
        if query.is_empty() {
            return tree_rows(&browse.files, &browse.collapsed);
        }
        let mut hits: Vec<(f32, &String)> = browse
            .files
            .iter()
            .filter_map(|path| Some((fuzzy_score(&query, path)?, path)))
            .collect();
        hits.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        hits.truncate(MAX_MATCHES);
        hits.into_iter()
            .map(|(_, path)| TreeRow::File {
                path: path.clone(),
                name: path.clone(),
                depth: 0,
            })
            .collect()
    }

    fn tree(
        &self,
        id: u64,
        browse: &BrowseFilesState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd().clone();
        let rows = Arc::new(self.rows(browse, cx));
        let selected = browse.selected.clone();
        let count = rows.len();
        div()
            .flex_none()
            .w(zpx(TREE_WIDTH))
            .h_full()
            .min_h_0()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(t.box_border)
            .child(
                div()
                    .flex_none()
                    .p(SPACING())
                    .border_b_1()
                    .border_color(t.box_border)
                    .child(text_box("browse-filter", &self.filter, None, window, cx)),
            )
            .child(
                uniform_list("browse-tree", count, move |range, _, cx| {
                    let t = cx.ghd().clone();
                    range
                        .map(|ix| match &rows[ix] {
                            TreeRow::Folder {
                                path,
                                name,
                                depth,
                                collapsed,
                            } => {
                                let (path, collapsed) = (path.clone(), *collapsed);
                                tree_row(ix, *depth, &t)
                                    .on_click(move |_, _, cx| {
                                        Dispatcher::set_browse_folder_collapsed(
                                            id,
                                            path.clone(),
                                            !collapsed,
                                            cx,
                                        )
                                    })
                                    .child(octicon(
                                        if collapsed {
                                            Octicon::ChevronRight
                                        } else {
                                            Octicon::ChevronDown
                                        },
                                        t.text_secondary,
                                    ))
                                    .child(div().truncate().child(name.clone()))
                            }
                            TreeRow::File { path, name, depth } => {
                                let on = selected.as_deref() == Some(path.as_str());
                                let path = path.clone();
                                tree_row(ix, *depth, &t)
                                    .pl(zpx(10. + 14. * *depth as f32 + 16.))
                                    .when(on, |d| {
                                        d.bg(t.box_selected_active_background)
                                            .text_color(t.box_selected_active_text)
                                    })
                                    .on_click(move |_, _, cx| {
                                        Dispatcher::select_browse_file(id, path.clone(), cx)
                                    })
                                    .child(div().truncate().child(name.clone()))
                            }
                        })
                        .collect()
                })
                .track_scroll(&self.tree_scroll)
                .flex_1()
                .min_h_0(),
            )
            .into_any_element()
    }

    fn header(
        &self,
        id: u64,
        browse: &BrowseFilesState,
        dirty: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = cx.ghd().clone();
        let path = browse.selected.clone().unwrap_or_default();
        let ref_label = match &browse.rev {
            None => "Working Tree".to_string(),
            Some(rev) if rev.len() == 40 => rev[..7].to_string(),
            Some(rev) => rev.clone(),
        };
        let branches: Vec<String> = self
            .state
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|rs| rs.info.as_ref())
            .map(|info| info.branches.iter().map(|b| b.name.clone()).collect())
            .unwrap_or_default();
        let edit_allowed = matches!(
            browse.content,
            Some(BrowseContent::Text { edit: Ok(_), .. })
        ) && AppState::global(cx)
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::QUICK_EDIT);
        let why_not = match &browse.content {
            Some(BrowseContent::Text { edit: Err(why), .. }) => Some(why.clone()),
            _ => None,
        };
        let editing = browse.editing;
        div()
            .flex_none()
            .h(zpx(40.))
            .px(SPACING())
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING())
            .border_b_1()
            .border_color(t.box_border)
            .child(button("browse-ref", ref_label, cx).on_mouse_down(
                MouseButton::Left,
                move |ev, window, cx| {
                    cx.stop_propagation();
                    let mut items = vec![MenuItem::new("Working Tree", move |_, cx| {
                        Dispatcher::show_browse_files(id, None, cx)
                    })];
                    items.push(MenuItem::separator());
                    items.extend(branches.iter().map(|name| {
                        let rev = name.clone();
                        MenuItem::new(name.clone(), move |_, cx| {
                            Dispatcher::show_browse_files(id, Some(rev.clone()), cx)
                        })
                    }));
                    crate::native_menu::show_context_menu(items, ev.position, window, cx);
                },
            ))
            .child(div().flex_1().min_w_0().truncate().child(path.clone()))
            .children(why_not.filter(|_| !path.is_empty()).map(|why| {
                div()
                    .flex_none()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(why)
            }))
            .when(editing, |d| {
                let save_path = path.clone();
                let editor = self.editor.clone();
                d.child(primary_button("browse-save", "Save", !dirty, cx).on_click(
                    move |_, _, cx| {
                        let text = editor.read(cx).value().to_string();
                        Dispatcher::save_file_edit(id, save_path.clone(), text, cx);
                    },
                ))
                .child(
                    button("browse-cancel", "Cancel", cx).on_click(move |_, _, cx| {
                        if dirty {
                            Dispatcher::show_popup(
                                corvene_core::Popup::ConfirmDiscardFileEdit {
                                    repo: id,
                                    close_browser: false,
                                },
                                cx,
                            );
                        } else {
                            Dispatcher::set_file_editing(id, false, cx);
                        }
                    }),
                )
            })
            .when(!editing && edit_allowed, |d| {
                d.child(
                    button("browse-edit", "Edit", cx)
                        .on_click(move |_, _, cx| Dispatcher::set_file_editing(id, true, cx)),
                )
            })
            .child(
                button("browse-close", "Close", cx).on_click(move |_, _, cx| {
                    if dirty {
                        Dispatcher::show_popup(
                            corvene_core::Popup::ConfirmDiscardFileEdit {
                                repo: id,
                                close_browser: true,
                            },
                            cx,
                        );
                    } else {
                        Dispatcher::close_browse_files(id, cx);
                    }
                }),
            )
    }

    fn lines(&mut self, lines: Arc<Vec<String>>, path: &str, cx: &mut Context<Self>) -> AnyElement {
        self.highlight(path, &lines, cx);
        let spans = self
            .spans
            .as_ref()
            .filter(|(key, _)| key.path == path)
            .map(|(_, spans)| spans.clone());
        let t = cx.ghd().clone();
        let count = lines.len();
        let number_width = zpx(line_number_width(count as u32));
        let longest = lines
            .iter()
            .enumerate()
            .max_by_key(|(_, l)| l.len() + l.matches('\t').count() * 3)
            .map(|(ix, _)| ix);
        let text_color = t.diff_text;
        uniform_list("browse-lines", count, move |range, _, _| {
            range
                .map(|ix| {
                    let raw = &lines[ix];
                    let text = expand_tabs(raw);
                    let styles = spans
                        .as_ref()
                        .and_then(|s| s.get(ix))
                        .map(|s| highlights(&spans_for_row(s.clone(), raw, &text), &t))
                        .unwrap_or_default();
                    div()
                        .id(("browse-line", ix))
                        .min_w_full()
                        .h(DIFF_LINE_HEIGHT())
                        .flex()
                        .flex_row()
                        .child(
                            div()
                                .flex_none()
                                .w(number_width)
                                .h_full()
                                .flex()
                                .justify_end()
                                .px(SPACING_HALF())
                                .bg(t.diff_gutter_background)
                                .border_r_1()
                                .border_color(t.diff_border)
                                .text_color(t.diff_line_number)
                                .child((ix + 1).to_string()),
                        )
                        .child(div().flex_none().px(SPACING()).whitespace_nowrap().child(
                            StyledText::new(SharedString::from(text)).with_highlights(styles),
                        ))
                })
                .collect()
        })
        .track_scroll(&self.text_scroll)
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .with_width_from_item(longest)
        .flex_1()
        .min_h_0()
        .font_family(mono_font())
        .line_height(DIFF_LINE_HEIGHT())
        .text_color(text_color)
        .into_any_element()
    }

    fn message(text: &'static str, t: &GhdTheme) -> AnyElement {
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_color(t.text_secondary)
            .child(text)
            .into_any_element()
    }
}

fn tree_row(ix: usize, depth: usize, t: &GhdTheme) -> Stateful<Div> {
    div()
        .id(("browse-row", ix))
        .h(zpx(ROW_HEIGHT))
        .w_full()
        .pl(zpx(10. + 14. * depth as f32))
        .pr(SPACING())
        .flex()
        .flex_row()
        .items_center()
        .gap(zpx(4.))
        .cursor_pointer()
        .hover(|d| d.bg(t.box_hover_background))
}

fn highlights(spans: &[Span], t: &GhdTheme) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
    spans
        .iter()
        .map(|s| {
            (
                s.range.clone(),
                HighlightStyle {
                    color: Some(token_color(s.class, t)),
                    ..HighlightStyle::default()
                },
            )
        })
        .collect()
}

fn image_format(path: &str) -> ImageFormat {
    let name = path.to_ascii_lowercase();
    if name.ends_with(".jpg") || name.ends_with(".jpeg") {
        ImageFormat::Jpeg
    } else if name.ends_with(".gif") {
        ImageFormat::Gif
    } else if name.ends_with(".webp") {
        ImageFormat::Webp
    } else if name.ends_with(".bmp") {
        ImageFormat::Bmp
    } else if name.ends_with(".svg") {
        ImageFormat::Svg
    } else {
        ImageFormat::Png
    }
}

impl Render for BrowseFilesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd().clone();
        let s = self.state.read(cx);
        let Some(id) = s.selected else {
            return div().into_any_element();
        };
        let Some(browse) = s.repo_states.get(&id).and_then(|rs| rs.browse.clone()) else {
            return div().into_any_element();
        };
        let path = browse.selected.clone().unwrap_or_default();

        // the editor starts from the text just read
        if browse.editing {
            if let Some(BrowseContent::Text {
                edit: Ok(editable), ..
            }) = &browse.content
            {
                let held = self
                    .editing
                    .as_ref()
                    .is_some_and(|(p, g, _)| *p == path && *g == browse.generation);
                if !held {
                    let text = editable.text.clone();
                    self.editor
                        .update(cx, |e, cx| e.set_value(text.clone(), window, cx));
                    self.editing = Some((path.clone(), browse.generation, text));
                    let handle = self.editor.read(cx).focus_handle(cx);
                    window.focus(&handle, cx);
                }
            }
        } else {
            self.editing = None;
        }
        let dirty = self
            .editing
            .as_ref()
            .is_some_and(|(_, _, start)| *start != self.editor.read(cx).value());

        let body: AnyElement = match (&browse.content, browse.editing) {
            _ if browse.selected.is_none() => Self::message("Choose a file.", &t),
            (None, _) => Self::message("Loading…", &t),
            (Some(BrowseContent::Text { .. }), true) => div()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .font_family(mono_font())
                .child(Textarea::new(&self.editor).appearance(false).size_full())
                .into_any_element(),
            (Some(BrowseContent::Text { lines, .. }), false) => {
                self.lines(lines.clone(), &path, cx)
            }
            (Some(BrowseContent::Image { bytes }), _) => div()
                .flex_1()
                .min_h_0()
                .p(SPACING())
                .flex()
                .items_center()
                .justify_center()
                .child(
                    img(Arc::new(Image::from_bytes(
                        image_format(&path),
                        bytes.as_ref().clone(),
                    )))
                    .max_w_full()
                    .max_h_full()
                    .object_fit(ObjectFit::Contain),
                )
                .into_any_element(),
            (Some(BrowseContent::Binary), _) => {
                Self::message("This binary file can't be shown.", &t)
            }
            (Some(BrowseContent::TooLarge), _) => {
                Self::message("This file is too large to show.", &t)
            }
        };
        let header = self.header(id, &browse, dirty, cx);
        let save_path = path.clone();
        let tree = self.tree(id, &browse, window, cx);
        div()
            .id("browse-files")
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_row()
            .bg(t.background)
            .border_t_1()
            .border_color(t.box_border)
            .capture_key_down(cx.listener(move |this, ev: &KeyDownEvent, _, cx| {
                // ⌘S (Ctrl+S) saves an open edit
                if ev.keystroke.key == "s"
                    && ev.keystroke.modifiers.secondary()
                    && this.editing.is_some()
                {
                    let text = this.editor.read(cx).value().to_string();
                    Dispatcher::save_file_edit(id, save_path.clone(), text, cx);
                    cx.stop_propagation();
                }
            }))
            .child(tree)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(body),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use std::collections::HashSet;

    fn names(rows: &[TreeRow]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                TreeRow::Folder { name, depth, .. } => format!("{}{name}/", " ".repeat(*depth)),
                TreeRow::File { name, depth, .. } => format!("{}{name}", " ".repeat(*depth)),
            })
            .collect()
    }

    fn files() -> Vec<String> {
        ["README", "a/b/c.rs", "a/b/d.rs", "a/e.rs", "z.rs"]
            .map(String::from)
            .to_vec()
    }

    #[test]
    fn folders_open_once_in_path_order() {
        let mut sorted = files();
        sorted.sort();
        assert_eq!(
            names(&tree_rows(&sorted, &HashSet::new())),
            ["README", "a/", " b/", "  c.rs", "  d.rs", " e.rs", "z.rs"]
        );
    }

    #[test]
    fn a_collapsed_folder_hides_what_is_under_it() {
        let mut sorted = files();
        sorted.sort();
        assert_eq!(
            names(&tree_rows(&sorted, &HashSet::from(["a/b".to_string()]))),
            ["README", "a/", " b/", " e.rs", "z.rs"]
        );
        assert_eq!(
            names(&tree_rows(&sorted, &HashSet::from(["a".to_string()]))),
            ["README", "a/", "z.rs"]
        );
    }
}
