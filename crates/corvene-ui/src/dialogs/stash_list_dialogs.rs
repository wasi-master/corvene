//! Corvene additions (`797-stash-list`): Stash All Changes with Message
//! and Create Branch from Stash. GHD stashes with a fixed message and has
//! no `git stash branch`.

use corvene_core::{AppState, Dispatcher, StashEntry};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::context_menu::mac_or;
use crate::dialog::{GroupButtonSpec, OkCancelButtonGroup, dialog};
use crate::dialogs::branch_dialogs::{ref_name_notice_with, sanitize_branch_name};
use crate::theme::sizes::*;
use crate::widgets::{checkbox_row, input_error, labeled, text_box};

/// Stash All Changes with Message: a message (optional) and whether new
/// files go into the stash too.
pub struct StashWithMessageDialog {
    repo: u64,
    message: Entity<InputState>,
    include_untracked: bool,
}

impl StashWithMessageDialog {
    pub fn new(repo: u64, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let message = cx.new(|cx| InputState::new(window, cx));
        cx.observe(&message, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&message, |this, _, ev: &InputEvent, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.submit(cx);
            }
        })
        .detach();
        let handle = message.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        Self {
            repo,
            message,
            include_untracked: true,
        }
    }

    fn submit(&self, cx: &mut App) {
        let message = self.message.read(cx).value().trim().to_string();
        Dispatcher::close_popup(cx);
        Dispatcher::stash_with_message(self.repo, message, self.include_untracked, cx);
    }
}

impl Render for StashWithMessageDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let repo = self.repo;
        let message = self.message.read(cx).value().trim().to_string();
        let include_untracked = self.include_untracked;
        let this = cx.entity().downgrade();
        dialog(
            "stash-with-message",
            mac_or("Stash All Changes", "Stash all changes"),
            crate::dialog::own_enter(div())
                .w(crate::theme::fit_width(400.))
                .flex()
                .flex_col()
                .gap(SPACING())
                .child(labeled(
                    "Message",
                    text_box("stash-with-message-text", &self.message, None, window, cx),
                    cx,
                ))
                .child(checkbox_row(
                    "stash-with-message-untracked",
                    include_untracked,
                    "Include new files",
                    move |checked, _, cx| {
                        let _ = this.update(cx, |this, cx| {
                            this.include_untracked = checked;
                            cx.notify();
                        });
                    },
                    cx,
                )),
            OkCancelButtonGroup {
                destructive: false,
                cancel: GroupButtonSpec {
                    id: "stash-with-message-cancel",
                    label: "Cancel".into(),
                    disabled: false,
                    on_click: Box::new(close),
                },
                ok: GroupButtonSpec {
                    id: "stash-with-message-ok",
                    label: mac_or("Stash Changes", "Stash changes").into(),
                    disabled: false,
                    on_click: Box::new(move |_, cx| {
                        Dispatcher::close_popup(cx);
                        Dispatcher::stash_with_message(
                            repo,
                            message.clone(),
                            include_untracked,
                            cx,
                        );
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

/// Create Branch from Stash: the name of the branch `git stash branch`
/// makes at the commit the stash was made on.
pub struct CreateBranchFromStashDialog {
    repo: u64,
    stash: StashEntry,
    name: Entity<InputState>,
}

impl CreateBranchFromStashDialog {
    pub fn new(repo: u64, stash: StashEntry, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx));
        cx.observe(&name, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&name, |this, _, ev: &InputEvent, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.submit(cx);
            }
        })
        .detach();
        let handle = name.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        Self { repo, stash, name }
    }

    /// The sanitized name, and whether a branch has it already.
    fn branch_name(&self, cx: &App) -> (String, bool) {
        let name = sanitize_branch_name(self.name.read(cx).value().as_ref(), cx);
        let exists = AppState::global(cx)
            .read(cx)
            .repo_states
            .get(&self.repo)
            .and_then(|rs| rs.info.as_ref())
            .is_some_and(|info| info.branches.iter().any(|b| b.name == name));
        (name, exists)
    }

    fn submit(&self, cx: &mut App) {
        let (name, exists) = self.branch_name(cx);
        if name.is_empty() || exists {
            return;
        }
        Dispatcher::close_popup(cx);
        Dispatcher::create_branch_from_stash(self.repo, self.stash.clone(), name, cx);
    }
}

impl Render for CreateBranchFromStashDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let proposed = self.name.read(cx).value().to_string();
        let (name, exists) = self.branch_name(cx);
        let disabled = name.is_empty() || exists;
        let forbidden = crate::dialogs::branch_dialogs::branch_forbidden_chars(cx);
        let notice = if exists {
            Some(
                input_error(format!("A branch named {name} already exists."), cx)
                    .into_any_element(),
            )
        } else {
            ref_name_notice_with(&proposed, &forbidden, "created", cx)
        };
        let (repo, stash) = (self.repo, self.stash.clone());
        let title = corvene_core::stash_list::stash_title(&self.stash);
        dialog(
            "create-branch-from-stash",
            mac_or("Create Branch from Stash", "Create branch from stash"),
            crate::dialog::own_enter(div())
                .w(crate::theme::fit_width(400.))
                .flex()
                .flex_col()
                .gap(SPACING())
                .child(format!(
                    "The branch starts at the commit \"{title}\" was stashed on, is checked \
                     out, and gets the stashed changes. The stash is dropped once they apply."
                ))
                .child(labeled(
                    "Name",
                    text_box(
                        "create-branch-from-stash-name",
                        &self.name,
                        None,
                        window,
                        cx,
                    ),
                    cx,
                ))
                .children(notice),
            OkCancelButtonGroup {
                destructive: false,
                cancel: GroupButtonSpec {
                    id: "create-branch-from-stash-cancel",
                    label: "Cancel".into(),
                    disabled: false,
                    on_click: Box::new(close),
                },
                ok: GroupButtonSpec {
                    id: "create-branch-from-stash-ok",
                    label: mac_or("Create Branch", "Create branch").into(),
                    disabled,
                    on_click: Box::new(move |_, cx| {
                        if !disabled {
                            Dispatcher::close_popup(cx);
                            Dispatcher::create_branch_from_stash(
                                repo,
                                stash.clone(),
                                name.clone(),
                                cx,
                            );
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
