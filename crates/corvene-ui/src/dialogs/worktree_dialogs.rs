//! Worktree dialogs (GHD `ui/worktrees/*-dialog.tsx`): Add Worktree,
//! Rename Worktree, Delete Worktree and Delete Worktree Failed.
//! Move Changes to Worktree is a Corvene addition
//! (`283-move-changes-to-worktree`).
//! The Add Worktree "Branch Name" box autocompletes branch names
//! (`ui/autocompletion/branch-autocompletion-provider.tsx`).

use std::path::PathBuf;
use std::rc::Rc;

use corvene_core::{AppState, BranchKind, Dispatcher};
use gpui_kit::component::input::{
    Enter, Escape, IndentInline, InputEvent, InputState, MoveDown, MoveUp,
};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::autocompletion::{self, Autocompletion, PickHandler};
use crate::context_menu::mac_or;
use crate::dialog::{DialogKind, GroupButtonSpec, OkCancelButtonGroup, dialog, dialog_with_kind};
use crate::dialogs::branch_dialogs::{ref_chip, sanitize_branch_name};
use crate::scrollbar::ScrollbarExt;
use crate::theme::ActiveGhdTheme;
use crate::theme::mono_font;
use crate::theme::sizes::*;
use crate::widgets::{Inline, button, checkbox, labeled, paragraph, radio, text_box};

/// GHD `sanitizedRepositoryName`: the folder name for a worktree name.
fn sanitized_folder_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------

/// GHD `AddWorktreeDialog` (`#add-worktree`, 500 px).
pub struct AddWorktreeDialog {
    state: Entity<AppState>,
    repo: u64,
    name: Entity<InputState>,
    path: Entity<InputState>,
    branch: Entity<InputState>,
    creating: bool,
    /// Branch-name autocompletion popup (`AutocompletingTextInput`).
    autocomplete: Option<Autocompletion>,
    /// The text just inserted from the popup; its own change event must not
    /// reopen the popup.
    completed: Option<String>,
}

impl AddWorktreeDialog {
    pub fn new(
        state: Entity<AppState>,
        repo: u64,
        initial_branch_name: Option<String>,
        initial_worktree_name: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let dir = crate::worktree_list::default_worktree_dir(state.read(cx), repo);
        // `RepositoryPath initialName`: the worktree name, else the branch
        let initial_name = initial_worktree_name
            .or_else(|| initial_branch_name.clone())
            .unwrap_or_default();
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("worktree name")
                .default_value(initial_name)
        });
        let path = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("worktree path")
                .default_value(dir.display().to_string())
        });
        let branch = cx.new(|cx| {
            InputState::new(window, cx).default_value(initial_branch_name.unwrap_or_default())
        });
        for e in [&name, &path, &branch] {
            cx.observe(e, |_, _, cx| cx.notify()).detach();
        }
        cx.subscribe(&branch, |this, _, ev: &InputEvent, cx| match ev {
            InputEvent::Change => this.open_autocomplete(cx),
            InputEvent::Blur => {
                this.autocomplete = None;
                cx.notify();
            }
            _ => {}
        })
        .detach();
        let handle = name.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        Self {
            state,
            repo,
            name,
            path,
            branch,
            creating: false,
            autocomplete: None,
            completed: None,
        }
    }

    /// Every local and remote branch name (`allBranches`).
    fn branch_names(&self, cx: &App) -> Vec<String> {
        self.state
            .read(cx)
            .repo_states
            .get(&self.repo)
            .and_then(|rs| rs.info.as_ref())
            .map(|i| i.branches.iter().map(|b| b.name.clone()).collect())
            .unwrap_or_default()
    }

    /// GHD `onChange` → `open`: filter the branches by the whole input.
    fn open_autocomplete(&mut self, cx: &mut Context<Self>) {
        let text = self.branch.read(cx).value().to_string();
        if self.completed.take().is_some_and(|c| c == text) {
            self.autocomplete = None;
        } else {
            self.autocomplete = autocompletion::attempt_branch(&text, &self.branch_names(cx));
        }
        cx.notify();
    }

    fn autocomplete_move(&mut self, delta: i64, cx: &mut Context<Self>) -> bool {
        match self.autocomplete.as_mut() {
            Some(ac) => {
                ac.move_selection(delta);
                cx.notify();
                true
            }
            None => false,
        }
    }

    /// Enter / Tab with a selected row inserts it (GHD `insertCompletion`).
    fn autocomplete_accept(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match self.autocomplete.as_ref().and_then(|ac| ac.selected) {
            Some(ix) => {
                self.autocomplete_insert(ix, window, cx);
                true
            }
            None => false,
        }
    }

    fn autocomplete_insert(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ac) = self.autocomplete.take() else {
            return;
        };
        let Some(hit) = ac.hits.get(ix) else {
            return;
        };
        let text = hit.completion_text();
        self.completed = Some(text.clone());
        self.branch
            .update(cx, |s, cx| s.set_value(text, window, cx));
        let handle = self.branch.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    fn full_path(&self, cx: &App) -> Option<PathBuf> {
        let name = self.name.read(cx).value().trim().to_string();
        let dir = self.path.read(cx).value().trim().to_string();
        if name.is_empty() || dir.is_empty() {
            return None;
        }
        Some(PathBuf::from(dir).join(sanitized_folder_name(&name)))
    }

    /// GHD `getEffectiveBranchName`: the typed branch, else the sanitized name.
    fn effective_branch(&self, cx: &App) -> String {
        let typed = self.branch.read(cx).value().trim().to_string();
        if !typed.is_empty() {
            // GHD's Branch name box is a `RefNameTextBox`
            return sanitize_branch_name(&typed, cx);
        }
        sanitize_branch_name(self.name.read(cx).value().trim(), cx)
    }

    fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(mac_or("Create Worktree", "Create worktree").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await
                && let Some(p) = paths.into_iter().next()
            {
                this.update_in(cx, |d, window, cx| {
                    d.path
                        .update(cx, |s, cx| s.set_value(p.display().to_string(), window, cx));
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.full_path(cx) else {
            return;
        };
        let branch = self.effective_branch(cx);
        if branch.is_empty() || self.creating {
            return;
        }
        self.creating = true;
        cx.notify();
        Dispatcher::add_worktree(self.repo, path, branch, cx);
    }
}

impl Render for AddWorktreeDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd();
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let full = self.full_path(cx);
        let effective = self.effective_branch(cx);
        let branch_placeholder = sanitize_branch_name(self.name.read(cx).value().trim(), cx);
        // `renderBranchStatus`
        let existing = {
            let s = self.state.read(cx);
            s.repo_states
                .get(&self.repo)
                .and_then(|rs| rs.info.as_ref())
                .and_then(|i| i.branches.iter().find(|b| b.name == effective))
                .map(|b| b.kind)
        };
        let disabled = full.is_none() || self.creating || effective.is_empty();
        let this = cx.entity();
        // autocompletion popup at the caret's bottom-left
        let popup = self.autocomplete.as_ref().and_then(|ac| {
            let (bounds, line_height) = self.branch.read(cx).cursor_layout()?;
            let anchor = point(bounds.origin.x, bounds.origin.y + line_height);
            let weak = cx.weak_entity();
            let on_pick: PickHandler = Rc::new(move |ix, window, cx| {
                weak.update(cx, |this, cx| this.autocomplete_insert(ix, window, cx))
                    .ok();
            });
            Some(autocompletion::dialog_popup(ac, anchor, on_pick, cx))
        });
        let branch_hint = existing.map(|kind| {
            let verb = match kind {
                BranchKind::Remote => "Will check out remote branch ",
                BranchKind::Local => "Will check out existing branch ",
            };
            paragraph(vec![
                verb.into(),
                Inline::Element(ref_chip(effective.clone(), cx).into_any_element()),
                ".".into(),
            ])
            .text_size(FONT_SIZE_SM())
            .text_color(t.text_secondary)
        });
        let path_message = full.as_ref().map(|p| {
            paragraph(vec![
                "Worktree will be created at ".into(),
                Inline::Element(ref_chip(p.display().to_string(), cx).into_any_element()),
                ".".into(),
            ])
            .mb(SPACING())
        });
        dialog(
            "add-worktree",
            mac_or("Add Worktree", "Add worktree"),
            div()
                .flex()
                .flex_col()
                .gap(SPACING())
                .w(crate::theme::fit_width(460.))
                .child(labeled(
                    mac_or("Worktree Name", "Worktree name"),
                    text_box("worktree-name", &self.name, None, window, cx),
                    cx,
                ))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_end()
                        .gap(SPACING())
                        .child(labeled(
                            mac_or("Local Path", "Local path"),
                            text_box("worktree-path", &self.path, None, window, cx),
                            cx,
                        ))
                        .child(
                            button("worktree-choose", "Choose…", cx)
                                .flex_none()
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.choose(window, cx)),
                                ),
                        ),
                )
                .child(
                    // popup keys win over the field's own bindings (GHD `onKeyDown`)
                    div()
                        .key_context("AutocompletingTextInput")
                        .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                            if this.autocomplete_move(-1, cx) {
                                cx.stop_propagation();
                            }
                        }))
                        .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                            if this.autocomplete_move(1, cx) {
                                cx.stop_propagation();
                            }
                        }))
                        .capture_action(cx.listener(|this, _: &Enter, window, cx| {
                            if this.autocomplete_accept(window, cx) {
                                cx.stop_propagation();
                            }
                        }))
                        .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                            if this.autocomplete_accept(window, cx) {
                                cx.stop_propagation();
                            }
                        }))
                        .capture_action(cx.listener(
                            |this, _: &crate::actions::FocusNext, window, cx| {
                                if this.autocomplete_accept(window, cx) {
                                    cx.stop_propagation();
                                }
                            },
                        ))
                        .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                            if this.autocomplete.take().is_some() {
                                cx.notify();
                                cx.stop_propagation();
                            }
                        }))
                        .child(labeled(
                            mac_or("Branch Name", "Branch name"),
                            text_box("worktree-branch", &self.branch, None, window, cx),
                            cx,
                        ))
                        .children(popup),
                )
                .children(crate::dialogs::branch_dialogs::ref_name_notice_with(
                    self.branch.read(cx).value().trim(),
                    &crate::dialogs::branch_dialogs::branch_forbidden_chars(cx),
                    "created",
                    cx,
                ))
                .when(
                    !branch_placeholder.is_empty() && self.branch.read(cx).value().is_empty(),
                    |d| {
                        d.child(
                            div()
                                .mt(zpx(-6.))
                                .text_size(FONT_SIZE_SM())
                                .text_color(t.text_secondary)
                                .child(format!("Defaults to {branch_placeholder}")),
                        )
                    },
                )
                .children(branch_hint)
                .children(path_message),
            OkCancelButtonGroup {
                destructive: false,
                cancel: GroupButtonSpec {
                    id: "add-worktree-cancel",
                    label: "Cancel".into(),
                    disabled: false,
                    on_click: Box::new(close),
                },
                ok: GroupButtonSpec {
                    id: "add-worktree-ok",
                    label: if self.creating {
                        "Creating Worktree…".into()
                    } else {
                        mac_or("Create Worktree", "Create worktree").into()
                    },
                    disabled,
                    on_click: Box::new(move |_, cx| {
                        if !disabled {
                            this.update(cx, |d, cx| d.submit(cx));
                        }
                    }),
                },
            }
            .into_buttons(),
            close,
            window,
            cx,
        )
    }
}

// ---------------------------------------------------------------------------

/// GHD `RenameWorktreeDialog`.
pub struct RenameWorktreeDialog {
    repo: u64,
    path: PathBuf,
    name: Entity<InputState>,
}

impl RenameWorktreeDialog {
    pub fn new(repo: u64, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let current = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = cx.new(|cx| InputState::new(window, cx));
        name.update(cx, |s, cx| s.set_value(current, window, cx));
        cx.observe(&name, |_, _, cx| cx.notify()).detach();
        let handle = name.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        Self { repo, path, name }
    }
}

impl Render for RenameWorktreeDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let current = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let new_name = self.name.read(cx).value().trim().to_string();
        let disabled = new_name.is_empty() || new_name == current;
        let (repo, old) = (self.repo, self.path.clone());
        let new_path = old
            .parent()
            .map(|p| p.join(&new_name))
            .unwrap_or_else(|| PathBuf::from(&new_name));
        dialog(
            "rename-worktree",
            mac_or("Rename Worktree", "Rename worktree"),
            div().w(crate::theme::fit_width(400.)).child(labeled(
                "Name",
                text_box("rename-worktree-name", &self.name, None, window, cx),
                cx,
            )),
            OkCancelButtonGroup {
                destructive: false,
                cancel: GroupButtonSpec {
                    id: "rename-worktree-cancel",
                    label: "Cancel".into(),
                    disabled: false,
                    on_click: Box::new(close),
                },
                ok: GroupButtonSpec {
                    id: "rename-worktree-ok",
                    label: format!("Rename {current}").into(),
                    disabled,
                    on_click: Box::new(move |_, cx| {
                        if !disabled {
                            Dispatcher::move_worktree(repo, old.clone(), new_path.clone(), cx);
                        }
                    }),
                },
            }
            .into_buttons(),
            close,
            window,
            cx,
        )
    }
}

// ---------------------------------------------------------------------------

/// GHD `DeleteWorktreeDialog`.
pub struct DeleteWorktreeDialog {
    repo: u64,
    path: PathBuf,
    dont_show_again: bool,
}

impl DeleteWorktreeDialog {
    pub fn new(repo: u64, path: PathBuf) -> Self {
        Self {
            repo,
            path,
            dont_show_again: false,
        }
    }
}

impl Render for DeleteWorktreeDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (repo, path, dont_show_again) = (self.repo, self.path.clone(), self.dont_show_again);
        let content = div()
            .flex()
            .flex_col()
            .gap(SPACING())
            .child(paragraph(vec![
                "Are you sure you want to delete the worktree ".into(),
                Inline::Element(ref_chip(name, cx).into_any_element()),
                "?".into(),
            ]))
            .child(
                div()
                    .id("delete-worktree-dont-show")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING_HALF())
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dont_show_again = !this.dont_show_again;
                        cx.notify();
                    }))
                    .child(checkbox(
                        "delete-worktree-dont-show-checkbox",
                        self.dont_show_again,
                        false,
                        cx,
                    ))
                    .child("Do not show this message again"),
            );
        // destructive Ok: Cancel is the default button
        dialog_with_kind(
            "delete-worktree",
            DialogKind::Warning,
            mac_or("Delete Worktree", "Delete worktree"),
            content,
            OkCancelButtonGroup {
                destructive: true,
                cancel: GroupButtonSpec {
                    id: "delete-worktree-cancel",
                    label: "Cancel".into(),
                    disabled: false,
                    on_click: Box::new(close),
                },
                ok: GroupButtonSpec {
                    id: "delete-worktree-ok",
                    label: crate::dialog::confirm_label(
                        "Delete",
                        "Delete Worktree",
                        "Delete worktree",
                        cx,
                    ),
                    disabled: false,
                    on_click: Box::new(move |_, cx| {
                        if dont_show_again {
                            Dispatcher::update_settings(cx, |s| s.confirm_worktree_removal = false);
                        }
                        Dispatcher::close_popup(cx);
                        Dispatcher::delete_worktree(repo, path.clone(), false, cx);
                    }),
                },
            }
            .into_buttons(),
            close,
            window,
            cx,
        )
    }
}

// ---------------------------------------------------------------------------

/// GHD `DeleteWorktreeFailedDialog`.
pub struct DeleteWorktreeFailedDialog {
    repo: u64,
    path: PathBuf,
    error: String,
    original: Option<PathBuf>,
}

impl DeleteWorktreeFailedDialog {
    pub fn new(repo: u64, path: PathBuf, error: String, original: Option<PathBuf>) -> Self {
        Self {
            repo,
            path,
            error,
            original,
        }
    }
}

impl Render for DeleteWorktreeFailedDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd();
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (repo, path) = (self.repo, self.path.clone());
        // dismissing switches back to the worktree that was current
        let original = self.original.clone();
        let dismiss = move |_: &mut Window, cx: &mut App| {
            Dispatcher::close_popup(cx);
            if let Some(original) = original.clone() {
                Dispatcher::switch_worktree(repo, original, cx);
            }
        };
        let content = div()
            .flex()
            .flex_col()
            .gap(SPACING())
            .w(crate::theme::fit_width(460.))
            .child(paragraph(vec![
                "Deleting the worktree ".into(),
                Inline::Element(ref_chip(name.clone(), cx).into_any_element()),
                " failed.".into(),
            ]))
            .child(
                // GHD `Terminal` output
                div()
                    .id("delete-worktree-error")
                    .max_h(zpx(150.))
                    .overflow_y_scroll()
                    .p(SPACING_HALF())
                    .rounded(BORDER_RADIUS())
                    .bg(t.box_alt_background)
                    .border_1()
                    .border_color(t.box_border)
                    .font_family(mono_font())
                    .text_size(FONT_SIZE_SM())
                    .child(self.error.clone())
                    .with_scrollbar(),
            )
            .child(paragraph(vec![
                "Would you like to forcefully delete the worktree ".into(),
                Inline::Element(ref_chip(name, cx).into_any_element()),
                "?".into(),
            ]));
        dialog_with_kind(
            "delete-worktree-failed",
            DialogKind::Error,
            mac_or("Delete Worktree Failed", "Delete worktree failed"),
            content,
            OkCancelButtonGroup {
                destructive: true,
                cancel: GroupButtonSpec {
                    id: "delete-worktree-failed-cancel",
                    label: "Cancel".into(),
                    disabled: false,
                    on_click: Box::new(dismiss.clone()),
                },
                ok: GroupButtonSpec {
                    id: "delete-worktree-failed-ok",
                    label: "Forcefully delete".into(),
                    disabled: false,
                    on_click: Box::new(move |_, cx| {
                        Dispatcher::close_popup(cx);
                        Dispatcher::delete_worktree(repo, path.clone(), true, cx);
                    }),
                },
            }
            .into_buttons(),
            dismiss,
            window,
            cx,
        )
    }
}

// ---------------------------------------------------------------------------

/// Corvene addition (`283-move-changes-to-worktree`): pick another worktree
/// of the repository; the changes are stashed here and restored there.
pub struct MoveChangesToWorktreeDialog {
    state: Entity<AppState>,
    repo: u64,
    selected: Option<PathBuf>,
    switch: bool,
}

impl MoveChangesToWorktreeDialog {
    pub fn new(state: Entity<AppState>, repo: u64) -> Self {
        Self {
            state,
            repo,
            selected: None,
            switch: true,
        }
    }
}

impl Render for MoveChangesToWorktreeDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let t = cx.ghd().clone();
        let (others, changed) = {
            let s = self.state.read(cx);
            let current = s.repository(self.repo).map(|r| r.path.clone());
            let rs = s.repo_states.get(&self.repo);
            (
                rs.map(|r| {
                    r.worktrees
                        .iter()
                        .filter(|w| Some(&w.path) != current.as_ref() && !w.is_prunable)
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
                rs.map(|r| r.changed_files()).unwrap_or(0),
            )
        };
        if self
            .selected
            .as_ref()
            .is_none_or(|p| !others.iter().any(|w| &w.path == p))
        {
            self.selected = others.first().map(|w| w.path.clone());
        }
        let rows = others.iter().enumerate().map(|(i, w)| {
            let selected = self.selected.as_ref() == Some(&w.path);
            let path = w.path.clone();
            div()
                .id(("move-changes-worktree", i))
                .flex()
                .flex_row()
                .items_center()
                .gap(SPACING_HALF())
                .py(zpx(3.))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = Some(path.clone());
                    cx.notify();
                }))
                .child(radio(("move-changes-worktree-radio", i), selected, cx))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(w.display_name()),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(t.text_secondary)
                        .child(w.description()),
                )
        });
        let files = if changed == 1 {
            "1 changed file".to_string()
        } else {
            format!("{changed} changed files")
        };
        let content = div()
            .w(crate::theme::fit_width(408.))
            .flex()
            .flex_col()
            .gap(SPACING())
            .child(format!(
                "Your {files} will be stashed here and restored in the worktree you pick. \
                 That worktree must have no changes of its own."
            ))
            .child(
                div()
                    .id("move-changes-worktrees")
                    .flex()
                    .flex_col()
                    .max_h(zpx(200.))
                    .overflow_y_scroll()
                    .children(rows)
                    .with_scrollbar(),
            )
            .child(
                div()
                    .id("move-changes-switch")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING_HALF())
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.switch = !this.switch;
                        cx.notify();
                    }))
                    .child(checkbox(
                        "move-changes-switch-checkbox",
                        self.switch,
                        false,
                        cx,
                    ))
                    .child(mac_or(
                        "Switch to the Worktree Afterwards",
                        "Switch to the worktree afterwards",
                    )),
            );
        let (repo, target, switch) = (self.repo, self.selected.clone(), self.switch);
        dialog(
            "move-changes-to-worktree",
            mac_or("Move Changes to Worktree", "Move changes to worktree"),
            content,
            OkCancelButtonGroup {
                destructive: false,
                cancel: GroupButtonSpec {
                    id: "move-changes-cancel",
                    label: "Cancel".into(),
                    disabled: false,
                    on_click: Box::new(close),
                },
                ok: GroupButtonSpec {
                    id: "move-changes-ok",
                    label: mac_or("Move Changes", "Move changes").into(),
                    disabled: target.is_none(),
                    on_click: Box::new(move |_, cx| {
                        Dispatcher::close_popup(cx);
                        if let Some(target) = &target {
                            Dispatcher::move_changes_to_worktree(repo, target.clone(), switch, cx);
                        }
                    }),
                },
            }
            .into_buttons(),
            close,
            window,
            cx,
        )
    }
}
