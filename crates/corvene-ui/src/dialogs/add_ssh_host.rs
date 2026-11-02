//! GHD `PopupType.AddSSHHost` (`app/src/ui/ssh/add-ssh-host.tsx`): ssh does
//! not know the host's key. Yes lets ssh add it to `known_hosts`.

use gpui_kit::prelude::*;
use gpui_kit::*;

use corvene_core::Dispatcher;
use corvene_git::AddSshHostInfo;

use crate::dialog::{GroupButtonSpec, OkCancelButtonGroup, dialog};
use crate::theme::sizes::*;

pub struct AddSshHostDialog {
    request: u64,
    info: AddSshHostInfo,
}

impl AddSshHostDialog {
    pub fn new(request: u64, info: AddSshHostInfo) -> Self {
        Self { request, info }
    }
}

impl Render for AddSshHostDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let request = self.request;
        let answer = move |yes: bool, cx: &mut App| {
            let text = if yes { "yes" } else { "no" };
            Dispatcher::answer_ssh_prompt(request, Some((text.to_string(), false)), cx);
        };
        let AddSshHostInfo {
            host,
            ip,
            key_type,
            fingerprint,
        } = &self.info;
        let content = div()
            .w(crate::theme::fit_width(400.))
            .flex()
            .flex_col()
            .gap(SPACING())
            .child(format!(
                "The authenticity of host '{host} ({ip})' can't be established. {key_type} key \
                 fingerprint is {fingerprint}."
            ))
            .child("Are you sure you want to continue connecting?");
        dialog(
            "add-ssh-host",
            "SSH Host",
            content,
            OkCancelButtonGroup {
                destructive: false,
                cancel: GroupButtonSpec {
                    id: "add-ssh-host-no",
                    label: "No".into(),
                    disabled: false,
                    on_click: Box::new(move |_, cx| answer(false, cx)),
                },
                ok: GroupButtonSpec {
                    id: "add-ssh-host-yes",
                    label: "Yes".into(),
                    disabled: false,
                    on_click: Box::new(move |_, cx| answer(true, cx)),
                },
            }
            .into_buttons(),
            move |_: &mut Window, cx: &mut App| answer(false, cx),
            window,
            cx,
        )
    }
}
