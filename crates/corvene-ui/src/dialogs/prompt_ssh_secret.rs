//! GHD `PopupType.SSHKeyPassphrase` and `SSHUserPassword`
//! (`app/src/ui/ssh/ssh-key-passphrase.tsx`, `ssh-user-password.tsx`).
//! Deviation: `wrong` says the last answer failed; GHD asks again with
//! nothing on the dialog to tell why.

use gpui_kit::component::input::InputState;
use gpui_kit::prelude::*;
use gpui_kit::*;

use corvene_core::Dispatcher;
use corvene_core::askpass::ReplyTo;

use crate::dialog::{GroupButtonSpec, OkCancelButtonGroup, dialog};
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::{checkbox_row, password_text_box};

pub struct PromptSshSecretDialog {
    request: u64,
    kind: ReplyTo,
    subject: String,
    wrong: bool,
    remember: bool,
    secret: Entity<InputState>,
}

impl PromptSshSecretDialog {
    pub fn new(
        request: u64,
        kind: ReplyTo,
        subject: String,
        wrong: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let secret = cx.new(|cx| InputState::new(window, cx).masked(true));
        cx.observe(&secret, |_, _, cx| cx.notify()).detach();
        let handle = secret.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        Self {
            request,
            kind,
            subject,
            wrong,
            remember: false,
            secret,
        }
    }
}

impl Render for PromptSshSecretDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd();
        let request = self.request;
        let (id, title, label, remember_label) = match self.kind {
            ReplyTo::Key => (
                "ssh-key-passphrase-prompt",
                "SSH Key Passphrase",
                format!("Enter passphrase for key '{}':", self.subject),
                "Remember passphrase",
            ),
            ReplyTo::User => (
                "ssh-user-password",
                "SSH User Password",
                format!("Enter password for '{}':", self.subject),
                "Remember password",
            ),
        };
        let value = self.secret.read(cx).value().to_string();
        let remember = self.remember;
        let view = cx.entity().downgrade();
        let content = div()
            .w(crate::theme::fit_width(400.))
            .flex()
            .flex_col()
            .gap(SPACING())
            .child(label)
            .child(password_text_box(
                "ssh-secret-input",
                &self.secret,
                window,
                cx,
            ))
            .when(self.wrong, |d| {
                d.child(
                    div()
                        .text_size(FONT_SIZE_SM())
                        .text_color(t.error)
                        .child("That did not work. Try again."),
                )
            })
            .child(checkbox_row(
                "ssh-secret-remember",
                remember,
                remember_label,
                move |checked, _, cx| {
                    view.update(cx, |this, cx| {
                        this.remember = checked;
                        cx.notify();
                    })
                    .ok();
                },
                cx,
            ));
        dialog(
            id,
            title,
            content,
            OkCancelButtonGroup {
                destructive: false,
                cancel: GroupButtonSpec {
                    id: "ssh-secret-cancel",
                    label: "Cancel".into(),
                    disabled: false,
                    on_click: Box::new(move |_, cx| {
                        Dispatcher::answer_ssh_prompt(request, None, cx)
                    }),
                },
                ok: GroupButtonSpec {
                    id: "ssh-secret-ok",
                    label: "OK".into(),
                    disabled: value.is_empty(),
                    on_click: Box::new(move |_, cx| {
                        Dispatcher::answer_ssh_prompt(request, Some((value.clone(), remember)), cx)
                    }),
                },
            }
            .into_buttons(),
            move |_: &mut Window, cx: &mut App| Dispatcher::answer_ssh_prompt(request, None, cx),
            window,
            cx,
        )
    }
}
