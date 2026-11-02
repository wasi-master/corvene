//! Corvene `1366-diff-line-edits` (desktop/desktop#14243): Edit Line in the
//! Changes diff. GitHub Desktop can only discard lines.

use corvene_core::Dispatcher;
use gpui_kit::component::input::InputState;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::dialog::{GroupButtonSpec, OkCancelButtonGroup, dialog};
use crate::theme::sizes::*;
use crate::widgets::text_box;

pub struct EditLineDialog {
    repo: u64,
    path: String,
    line: u32,
    original: String,
    input: Entity<InputState>,
}

impl EditLineDialog {
    pub fn new(
        repo: u64,
        path: String,
        line: u32,
        original: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let text = original.clone();
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(text, window, cx);
            state
        });
        cx.observe(&input, |_, _, cx| cx.notify()).detach();
        let handle = input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        Self {
            repo,
            path,
            line,
            original,
            input,
        }
    }
}

impl Render for EditLineDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let value = self.input.read(cx).value().to_string();
        let unchanged = value == self.original;
        let (repo, path, line, original) = (
            self.repo,
            self.path.clone(),
            self.line,
            self.original.clone(),
        );
        let content = div()
            .w(crate::theme::fit_width(520.))
            .flex()
            .flex_col()
            .gap(SPACING())
            .child(format!("Line {line} of {path}"))
            .child(text_box("edit-line-input", &self.input, None, window, cx));
        dialog(
            "edit-line",
            "Edit Line",
            content,
            OkCancelButtonGroup {
                destructive: false,
                cancel: GroupButtonSpec {
                    id: "edit-line-cancel",
                    label: "Cancel".into(),
                    disabled: false,
                    on_click: Box::new(|_, cx| Dispatcher::close_popup(cx)),
                },
                ok: GroupButtonSpec {
                    id: "edit-line-ok",
                    label: "Save".into(),
                    disabled: unchanged,
                    on_click: Box::new(move |_, cx| {
                        Dispatcher::save_line_edit(
                            repo,
                            path.clone(),
                            line,
                            original.clone(),
                            value.clone(),
                            cx,
                        )
                    }),
                },
            }
            .into_buttons(),
            |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx),
            window,
            cx,
        )
    }
}

/// Keep editing, or lose the edit (and with `close_browser` leave the browser).
pub struct ConfirmDiscardFileEditDialog {
    pub repo: u64,
    pub close_browser: bool,
}

impl Render for ConfirmDiscardFileEditDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (repo, close_browser) = (self.repo, self.close_browser);
        dialog(
            "confirm-discard-file-edit",
            "Discard Your Edit?",
            div()
                .w(crate::theme::fit_width(400.))
                .child("The changes you made in this file have not been saved."),
            OkCancelButtonGroup {
                destructive: true,
                cancel: GroupButtonSpec {
                    id: "discard-edit-keep",
                    label: "Keep Editing".into(),
                    disabled: false,
                    on_click: Box::new(|_, cx| Dispatcher::close_popup(cx)),
                },
                ok: GroupButtonSpec {
                    id: "discard-edit-ok",
                    label: "Discard".into(),
                    disabled: false,
                    on_click: Box::new(move |_, cx| {
                        Dispatcher::close_popup(cx);
                        Dispatcher::set_file_editing(repo, false, cx);
                        if close_browser {
                            Dispatcher::close_browse_files(repo, cx);
                        }
                    }),
                },
            }
            .into_buttons(),
            |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx),
            window,
            cx,
        )
    }
}
