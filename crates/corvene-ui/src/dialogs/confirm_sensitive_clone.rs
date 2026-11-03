//! Clone into a sensitive folder: a Corvene extra (flag
//! `1123-confirm-sensitive-clone`, desktop/desktop#23002). GHD's `clone`
//! refuses the home directory and its credentials / configuration folders
//! (`isClonePathSensitive`, `corvene_git::is_clone_path_sensitive`) with an
//! error; Corvene names the folder and clones there when confirmed. Built
//! like Force Unlock (`dialogs::confirm_force_unlock`).

use std::path::PathBuf;

use corvene_core::Dispatcher;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::context_menu::mac_or;
use crate::dialog::{DialogKind, GroupButtonSpec, OkCancelButtonGroup, dialog_with_kind};
use crate::theme::sizes::*;

pub struct ConfirmSensitiveCloneDialog {
    url: String,
    path: PathBuf,
    default_branch: Option<String>,
    depth: Option<u32>,
}

impl ConfirmSensitiveCloneDialog {
    pub fn new(
        url: String,
        path: PathBuf,
        default_branch: Option<String>,
        depth: Option<u32>,
    ) -> Self {
        Self {
            url,
            path,
            default_branch,
            depth,
        }
    }
}

impl Render for ConfirmSensitiveCloneDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let path = crate::format::display_path(&self.path.to_string_lossy());
        let content = div()
            .w(crate::theme::fit_width(400.))
            .flex()
            .flex_col()
            .gap(SPACING())
            .child(
                div()
                    .child(format!(
                        "{path} is your home folder or holds keys or settings."
                    ))
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .child(
                "Files from the repository will be written there and can replace what is \
                 there now. Only clone here if you chose this folder yourself.",
            );
        let (url, target, branch, depth) = (
            self.url.clone(),
            self.path.clone(),
            self.default_branch.clone(),
            self.depth,
        );
        dialog_with_kind(
            "dialog-confirm-sensitive-clone",
            DialogKind::Warning,
            mac_or("Clone Into This Folder?", "Clone into this folder?"),
            content,
            OkCancelButtonGroup {
                destructive: true,
                cancel: GroupButtonSpec {
                    id: "sensitive-clone-cancel",
                    label: "Cancel".into(),
                    disabled: false,
                    on_click: Box::new(close),
                },
                ok: GroupButtonSpec {
                    id: "sensitive-clone-ok",
                    label: mac_or("Clone Anyway", "Clone anyway").into(),
                    disabled: false,
                    on_click: Box::new(move |_, cx| {
                        Dispatcher::clone_into_sensitive_path(
                            url.clone(),
                            target.clone(),
                            branch.clone(),
                            depth,
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
