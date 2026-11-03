//! Corvene (`528-proxy-credentials`, desktop#10026): a proxy answered 407
//! Proxy Authentication Required; ask for its username and password. GHD
//! 3.6.6 has no such dialog (git's 407 is a plain error), so it is built
//! like `GenericGitAuthentication` (`ui/generic-git-auth/generic-git-auth.tsx`,
//! `dialog#generic-git-auth`): the same width, fields and buttons.
//!
//! Corvene (`530-front-door-credentials`, desktop#8783): the same dialog
//! asks for the credentials of a proxy in front of a server (`server`),
//! which answered 401 with its own challenge. GHD shows the failed request.

use corvene_core::Dispatcher;
use corvene_core::proxy::ProxyRetry;
use gpui_kit::component::input::InputState;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::context_menu::mac_or;
use crate::dialog::{GroupButtonSpec, OkCancelButtonGroup, dialog};
use crate::theme::sizes::*;
use crate::widgets::{Inline, code_ref, labeled, paragraph, password_text_box, text_box};

/// Where the password is kept, as the platform calls it.
fn keychain_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "your keychain"
    } else if cfg!(windows) {
        "Windows Credential Manager"
    } else {
        "your keyring"
    }
}

pub struct ProxyAuthenticationDialog {
    proxy: String,
    server: bool,
    rejected: bool,
    retry: ProxyRetry,
    username: Entity<InputState>,
    password: Entity<InputState>,
}

impl ProxyAuthenticationDialog {
    pub fn new(
        proxy: String,
        server: bool,
        username: Option<String>,
        rejected: bool,
        retry: ProxyRetry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let username_state = cx.new(|cx| InputState::new(window, cx));
        if let Some(u) = &username {
            username_state.update(cx, |s, cx| s.set_value(u.clone(), window, cx));
        }
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        cx.observe(&username_state, |_, _, cx| cx.notify()).detach();
        cx.observe(&password, |_, _, cx| cx.notify()).detach();
        // `Dialog.focusFirstSuitableChild`: the first empty field
        let first = if username.is_some() {
            &password
        } else {
            &username_state
        };
        let handle = first.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        Self {
            proxy,
            server,
            rejected,
            retry,
            username: username_state,
            password,
        }
    }
}

impl Render for ProxyAuthenticationDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let username = self.username.read(cx).value().trim().to_string();
        let password = self.password.read(cx).value().to_string();
        let disabled = username.is_empty() || password.is_empty();
        let proxy = self.proxy.clone();
        let retry = self.retry.clone();
        let cancel_proxy = self.proxy.clone();
        let server = self.server;
        let close = move |_: &mut Window, cx: &mut App| {
            if server {
                Dispatcher::dismiss_front_door_credentials(cancel_proxy.clone(), cx)
            } else {
                Dispatcher::dismiss_proxy_credentials(cancel_proxy.clone(), cx)
            }
        };
        let ok_label = match self.retry {
            ProxyRetry::None => "Save",
            _ => "Save and Retry",
        };
        let what = if self.server {
            "The server "
        } else {
            "The proxy server "
        };
        let intro: Vec<Inline> = if self.rejected {
            vec![
                what.into(),
                code_ref(self.proxy.clone(), cx).into_any_element().into(),
                " did not accept the username and password. Please enter them again.".into(),
            ]
        } else if self.server {
            vec![
                what.into(),
                code_ref(self.proxy.clone(), cx).into_any_element().into(),
                " asks for a username and password of its own before it lets requests through \
                 to GitHub."
                    .into(),
            ]
        } else {
            vec![
                what.into(),
                code_ref(self.proxy.clone(), cx).into_any_element().into(),
                " requires a username and password.".into(),
            ]
        };
        // `dialog#generic-git-auth { width: 450px }` sizes the box
        let content = div()
            .flex()
            .flex_col()
            .gap(SPACING())
            .child(paragraph(intro))
            .child(labeled(
                "Username",
                text_box("proxy-username", &self.username, None, window, cx),
                cx,
            ))
            .child(labeled(
                "Password",
                password_text_box("proxy-password", &self.password, window, cx),
                cx,
            ))
            .child(paragraph(vec![
                format!(
                    "They are saved in {} and used for Git and for Corvene's connections to \
                     GitHub.",
                    keychain_name()
                )
                .into(),
            ]));
        dialog(
            "dialog-proxy-auth",
            mac_or(
                "Proxy Authentication Required",
                "Proxy authentication required",
            ),
            content,
            OkCancelButtonGroup {
                destructive: false,
                cancel: GroupButtonSpec {
                    id: "proxy-auth-cancel",
                    label: "Cancel".into(),
                    disabled: false,
                    on_click: Box::new(close.clone()),
                },
                ok: GroupButtonSpec {
                    id: "proxy-auth-save",
                    label: ok_label.into(),
                    disabled,
                    on_click: Box::new(move |_, cx| {
                        if disabled {
                            return;
                        }
                        if server {
                            Dispatcher::save_front_door_credentials(
                                proxy.clone(),
                                username.clone(),
                                password.clone(),
                                cx,
                            );
                        } else {
                            Dispatcher::save_proxy_credentials(
                                proxy.clone(),
                                username.clone(),
                                password.clone(),
                                retry.clone(),
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
