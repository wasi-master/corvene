//! Sign-in dialog (`ui/sign-in/sign-in.tsx`). GitHub.com uses the OAuth
//! device flow (code shown here, authorised in the browser); GitHub
//! Enterprise takes an address, then the same flows when the host has an
//! OAuth app (entered here or built in with `CORVENE_GHES_OAUTH`), else a
//! personal access token. GHD signs in to Enterprise with its own OAuth app
//! in the browser (`ui/sign-in/sign-in.tsx`); a GHES server does not know
//! Corvene's, so its client ID has to come from the user.
//!
//! The steps are GHD's sign-in store's (`AppState::sign_in_store`,
//! `corvene_core::sign_in`): EndpointEntry, ExistingAccountWarning and
//! Authentication. At the last two the dialog offers Corvene's ways of
//! authenticating ([`Method`]). The ExistingAccountWarning step shows GHD's
//! warning (`ui/sign-in/sign-in.tsx` `renderExistingAccountWarningStep`)
//! above them; GHD's dialog then has a "Continue With Browser" button that
//! signs the account out and moves to the Authentication step, where the
//! browser is opened by a second click. Here the warning stays above the
//! Authentication step's form and starting any authentication signs the
//! account out first (what GHD's shared `SignIn`, `ui/lib/sign-in.tsx`,
//! does with its "Sign in using your browser" button).
//!
//! Deviation (flag `enterprise-plain-http`, off in every preset): an
//! Enterprise address typed with `http://` keeps plain HTTP (GHD's
//! `validateURL` refuses any scheme but `https`, "Unsupported protocol").
//!
//! Deviation (flag `527-multiple-accounts`): an endpoint with an account
//! skips ExistingAccountWarning and the new account is added beside it.
//! Opened by Settings › Accounts › Add account, the dialog says to sign in
//! to the other account in the browser first, the browser flow asks GitHub
//! for its account picker, and the same account coming back is an error.

use corvene_core::sign_in::{SignInState, SignInStep};
use corvene_core::{AppState, AuthenticationStep, Dispatcher};
use corvene_github::Endpoint;
use gpui_kit::component::input::InputState;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::dialog::{DialogButton, dialog};
use crate::icons::{Octicon, octicon};
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::{button, labeled, password_text_box, primary_button, text_box};

/// What a link or button of the sign-in form does (GHD `SignIn`'s
/// callbacks).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignInAction {
    /// `dispatcher.setSignInEndpoint(url)`
    SetEndpoint,
    /// `dispatcher.requestBrowserAuthentication()` (Corvene:
    /// `Dispatcher::begin_sign_in`)
    RequestBrowserAuthentication,
}

/// GHD's `.existing-account-warning` paragraph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExistingAccountWarning {
    /// The whole paragraph.
    pub text: String,
    /// The endpoint's host (GHD's first `Ref`).
    pub host: String,
    /// The signed-in account's login (the second `Ref`).
    pub login: String,
}

/// What the sign-in form shows for a sign-in store state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignInContent {
    pub existing_account_warning: Option<ExistingAccountWarning>,
    /// The form's main button: label and what it does.
    pub actions: Vec<(String, SignInAction)>,
}

/// GHD `SignIn` (`ui/lib/sign-in.tsx`): the endpoint form at
/// EndpointEntry, the authentication form at Authentication, the latter
/// under the existing-account warning at ExistingAccountWarning, and
/// nothing at Success.
pub fn sign_in_content(state: &SignInState) -> Option<SignInContent> {
    let browser = (
        "Sign in using your browser".to_string(),
        SignInAction::RequestBrowserAuthentication,
    );
    match state.kind {
        SignInStep::EndpointEntry => Some(SignInContent {
            existing_account_warning: None,
            actions: vec![("Continue".to_string(), SignInAction::SetEndpoint)],
        }),
        SignInStep::ExistingAccountWarning => {
            let host = state
                .endpoint
                .as_deref()
                .map(|endpoint| Endpoint::from_api_base(endpoint).host().to_string())
                .unwrap_or_default();
            let login = state
                .existing_account
                .as_ref()
                .map(|account| account.login.clone())
                .unwrap_or_default();
            Some(SignInContent {
                existing_account_warning: Some(ExistingAccountWarning {
                    text: format!(
                        "You're already signed in to {host} with the account {login}. If you \
                         continue, you will first be signed out."
                    ),
                    host,
                    login,
                }),
                actions: vec![browser],
            })
        }
        SignInStep::Authentication => Some(SignInContent {
            existing_account_warning: None,
            actions: vec![browser],
        }),
        SignInStep::Success => None,
    }
}

/// How the Authentication step signs in (Corvene's choices; GHD only has
/// the browser).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Method {
    /// Device flow, with the browser flow and a token as alternatives.
    Authentication,
    TokenEntry,
    /// GHES only: the OAuth app registered on the host.
    OAuthAppEntry,
}

/// A click handler shared between buttons.
type SharedHandler = std::rc::Rc<dyn Fn(&mut Window, &mut App)>;

pub struct SignInDialog {
    state: Entity<AppState>,
    enterprise: bool,
    /// Drawn inside the Welcome flow's "Sign in to your GitHub Enterprise"
    /// step (GHD `SignInEnterprise` around the shared `SignIn`) instead of
    /// as a dialog: no frame, and Cancel runs the step's handler (GHD's
    /// `SignIn` children).
    embedded: Option<SharedHandler>,
    method: Method,
    address: Entity<InputState>,
    token: Entity<InputState>,
    client_id: Entity<InputState>,
    client_secret: Entity<InputState>,
}

impl SignInDialog {
    pub fn new(
        state: Entity<AppState>,
        enterprise: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let address =
            cx.new(|cx| InputState::new(window, cx).placeholder("https://github.example.com"));
        let token = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("ghp_…")
                .masked(true)
        });
        let client_id = cx.new(|cx| InputState::new(window, cx).placeholder("Ov23li…"));
        let client_secret = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Optional")
                .masked(true)
        });
        for e in [&address, &token, &client_id, &client_secret] {
            cx.observe(e, |_, _, cx| cx.notify()).detach();
        }
        if enterprise {
            let handle = address.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
        Self {
            state,
            enterprise,
            embedded: None,
            method: Method::Authentication,
            address,
            token,
            client_id,
            client_secret,
        }
    }

    /// The Enterprise form for the Welcome flow's sign-in step, whose
    /// Cancel runs `on_cancel`.
    pub fn new_embedded(
        state: Entity<AppState>,
        on_cancel: impl Fn(&mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            embedded: Some(std::rc::Rc::new(on_cancel)),
            ..Self::new(state, true, window, cx)
        }
    }

    fn title(&self) -> &'static str {
        if self.enterprise {
            "Sign in to GitHub Enterprise"
        } else {
            "Sign in to GitHub.com"
        }
    }

    /// The sign-in store's state (`None` once the sign-in is over).
    fn store_state(&self, cx: &App) -> Option<SignInState> {
        self.state.read(cx).sign_in_store.get_state().cloned()
    }

    /// GHD `setSignInEndpoint`; a host without an OAuth app goes on to the
    /// personal access token.
    fn continue_endpoint(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let raw = self.address.read(cx).value().to_string();
        Dispatcher::set_sign_in_endpoint(raw, cx);
        let authenticating = self.store_state(cx).is_some_and(|s| {
            matches!(
                s.kind,
                SignInStep::Authentication | SignInStep::ExistingAccountWarning
            )
        });
        if authenticating {
            if Dispatcher::oauth_client_id(&self.endpoint(cx), cx).is_some() {
                self.method = Method::Authentication;
            } else {
                self.show_token_entry(window, cx);
            }
        }
        cx.notify();
    }

    /// The endpoint the store is signing in to (GitHub.com until there is
    /// one).
    fn endpoint(&self, cx: &App) -> Endpoint {
        self.store_state(cx)
            .and_then(|s| s.endpoint)
            .map(|api| Endpoint::from_api_base(&api))
            .unwrap_or_else(Endpoint::github_com)
    }

    fn show_token_entry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        Dispatcher::cancel_sign_in(cx);
        self.method = Method::TokenEntry;
        let handle = self.token.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    /// The OAuth app step, prefilled with the host's current client ID.
    fn show_oauth_app_entry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        Dispatcher::cancel_sign_in(cx);
        let current = Dispatcher::oauth_client_id(&self.endpoint(cx), cx).unwrap_or_default();
        self.client_id
            .update(cx, |input, cx| input.set_value(current, window, cx));
        self.client_secret
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.method = Method::OAuthAppEntry;
        let handle = self.client_id.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    fn submit_oauth_app(&mut self, cx: &mut Context<Self>) {
        let client_id = self.client_id.read(cx).value().trim().to_string();
        if client_id.is_empty() {
            return;
        }
        let secret = self.client_secret.read(cx).value().to_string();
        Dispatcher::set_enterprise_oauth_app(&self.endpoint(cx), client_id, secret, cx);
        self.method = Method::Authentication;
        cx.notify();
    }

    fn submit_token(&mut self, cx: &mut Context<Self>) {
        let token = self.token.read(cx).value().trim().to_string();
        if !token.is_empty() {
            Dispatcher::sign_in_with_token(self.endpoint(cx), token, cx);
        }
    }

    fn authentication_body(&self, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let endpoint = self.endpoint(cx);
        let enterprise = self.enterprise;
        // `307-sign-in-flow`: which flow the primary button starts; the
        // other one stays a link away
        let browser_first = Dispatcher::browser_sign_in_first(&endpoint, cx);
        let authentication = self.state.read(cx).authentication.clone();
        let browser_label = self
            .store_state(cx)
            .as_ref()
            .and_then(sign_in_content)
            .and_then(|content| {
                content
                    .actions
                    .into_iter()
                    .find(|(_, action)| *action == SignInAction::RequestBrowserAuthentication)
            })
            .map_or_else(
                || "Sign in using your browser".to_string(),
                |(label, _)| label,
            );
        let this = cx.entity();
        // `527-multiple-accounts`: Add account, beside a signed-in account
        let adding = {
            let s = self.state.read(cx);
            (s.multiple_accounts() && s.adding_account)
                .then(|| {
                    s.accounts_for(&endpoint.api_base)
                        .first()
                        .map(|a| a.login.clone())
                })
                .flatten()
        };
        match authentication.map(|s| s.step) {
            None => div()
                .flex()
                .flex_col()
                .items_start()
                .gap(SPACING())
                .when_some(adding, |d, login| {
                    d.child(div().text_color(t.text_secondary).child(format!(
                        "You are signed in as @{login}. To add another account, sign in \
                             to it in your browser first, or pick it when GitHub asks which \
                             account to use."
                    )))
                })
                .child(if browser_first {
                    "Corvene will open GitHub in your browser. Authorise this app there to \
                     sign in."
                } else {
                    "Corvene will show you a one-time code and open GitHub in your browser. \
                     Enter the code there to authorise this app."
                })
                .child(
                    primary_button("sign-in-browser", browser_label, false, cx).on_click({
                        let endpoint = endpoint.clone();
                        move |_, _, cx| Dispatcher::begin_sign_in(endpoint.clone(), cx)
                    }),
                )
                .child(
                    div()
                        .id("sign-in-token-link")
                        .text_color(t.link)
                        .cursor_pointer()
                        .child("Sign in with a personal access token instead")
                        .on_click({
                            let this = this.clone();
                            move |_, window, cx| {
                                this.update(cx, |d, cx| d.show_token_entry(window, cx))
                            }
                        }),
                )
                .child(
                    // GHD's `authenticateWithBrowser` (web application flow)
                    // or the device flow, whichever is not the default
                    div()
                        .id("sign-in-web-flow-link")
                        .text_color(t.link)
                        .cursor_pointer()
                        .child(if browser_first {
                            "Use a one-time code instead"
                        } else {
                            "Use the browser flow instead (no code to type)"
                        })
                        .on_click({
                            let endpoint = endpoint.clone();
                            move |_, _, cx| {
                                if browser_first {
                                    Dispatcher::sign_in_device_flow(endpoint.clone(), cx)
                                } else {
                                    Dispatcher::sign_in_web_flow(endpoint.clone(), cx)
                                }
                            }
                        }),
                )
                .when(enterprise, |d| {
                    d.child(
                        div()
                            .id("sign-in-oauth-app-link")
                            .text_color(t.link)
                            .cursor_pointer()
                            .child("Use a different OAuth app")
                            .on_click(move |_, window, cx| {
                                this.update(cx, |d, cx| d.show_oauth_app_entry(window, cx))
                            }),
                    )
                })
                .into_any_element(),
            Some(AuthenticationStep::Requesting) => div()
                .text_color(t.text_secondary)
                .child("Requesting a sign-in code from GitHub…")
                .into_any_element(),
            Some(AuthenticationStep::DeviceCode {
                user_code,
                verification_uri,
            }) => {
                let code_for_copy = user_code.clone();
                let uri = verification_uri.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACING())
                    .child(format!("Enter this code at {verification_uri} to sign in:"))
                    .child(
                        div().flex().child(
                            div()
                                .px(SPACING_DOUBLE())
                                .py(SPACING())
                                .rounded(BORDER_RADIUS())
                                .border_1()
                                .border_color(t.box_border_contrast)
                                .bg(t.box_alt_background)
                                .font_family(crate::theme::mono_font())
                                .text_size(zpx(28.))
                                .line_height(zpx(34.))
                                .child(user_code.clone()),
                        ),
                    )
                    // the code box and both buttons overflow the dialog on one row
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(SPACING())
                            .child(
                                button("sign-in-copy", "Copy code", cx)
                                    .child(
                                        octicon(Octicon::Copy, t.secondary_button_text)
                                            .ml(SPACING_HALF()),
                                    )
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            code_for_copy.clone(),
                                        ))
                                    }),
                            )
                            .child(
                                button("sign-in-open", "Open GitHub", cx)
                                    .child(
                                        octicon(Octicon::LinkExternal, t.secondary_button_text)
                                            .ml(SPACING_HALF()),
                                    )
                                    .on_click(move |_, _, cx| {
                                        corvene_core::Dispatcher::open_url(&uri, cx)
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .text_color(t.text_secondary)
                            .child("Waiting for you to authorise Corvene in the browser…"),
                    )
                    .into_any_element()
            }
            Some(AuthenticationStep::Verifying) => div()
                .text_color(t.text_secondary)
                .child("Signing in…")
                .into_any_element(),
            Some(AuthenticationStep::Browser { authorize_url }) => {
                let url = authorize_url.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACING())
                    .child(
                        "Authorise Corvene on the GitHub page that opened in your browser. \
                         GitHub sends you back here when you are done.",
                    )
                    .child(
                        button("sign-in-open-again", "Open GitHub again", cx)
                            .child(
                                octicon(Octicon::LinkExternal, t.secondary_button_text)
                                    .ml(SPACING_HALF()),
                            )
                            .on_click(move |_, _, cx| corvene_core::Dispatcher::open_url(&url, cx)),
                    )
                    .child(
                        div()
                            .text_color(t.text_secondary)
                            .child("Waiting for GitHub to send the sign-in back to Corvene…"),
                    )
                    .into_any_element()
            }
            Some(AuthenticationStep::Error(message)) => div()
                .flex()
                .flex_col()
                .items_start()
                .gap(SPACING())
                .child(div().text_color(t.error).child(message))
                .child(
                    primary_button("sign-in-retry", "Try again", false, cx).on_click(
                        move |_, _, cx| {
                            Dispatcher::cancel_sign_in(cx);
                            Dispatcher::begin_sign_in(endpoint.clone(), cx)
                        },
                    ),
                )
                .when(enterprise, |d| {
                    d.child(
                        div()
                            .id("sign-in-error-token-link")
                            .text_color(t.link)
                            .cursor_pointer()
                            .child("Sign in with a personal access token instead")
                            .on_click({
                                let this = this.clone();
                                move |_, window, cx| {
                                    this.update(cx, |d, cx| d.show_token_entry(window, cx))
                                }
                            }),
                    )
                    .child(
                        div()
                            .id("sign-in-error-oauth-app-link")
                            .text_color(t.link)
                            .cursor_pointer()
                            .child("Use a different OAuth app")
                            .on_click(move |_, window, cx| {
                                this.update(cx, |d, cx| d.show_oauth_app_entry(window, cx))
                            }),
                    )
                })
                .into_any_element(),
        }
    }
}

impl Render for SignInDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd();
        // GHD `onDismissed`: `resetSignInState` (closing the SignIn popup
        // also stops the authentication flow) + `closePopup`
        let close = |_: &mut Window, cx: &mut App| {
            Dispatcher::close_popup(cx);
        };
        let this = cx.entity();
        let sign_in_error =
            self.state
                .read(cx)
                .authentication
                .as_ref()
                .and_then(|s| match &s.step {
                    AuthenticationStep::Error(m) => Some(m.clone()),
                    _ => None,
                });
        let store_state = self.store_state(cx);
        let at_endpoint_entry = match store_state.as_ref().map(|s| s.kind) {
            Some(kind) => kind == SignInStep::EndpointEntry,
            None => self.enterprise,
        };
        let address_error = store_state.as_ref().and_then(|s| s.error.clone());
        let address_loading = store_state.as_ref().is_some_and(|s| s.loading);
        let warning = store_state
            .as_ref()
            .and_then(sign_in_content)
            .and_then(|content| content.existing_account_warning);

        let (body, buttons): (AnyElement, Vec<DialogButton>) = match self.method {
            _ if at_endpoint_entry => (
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACING())
                    .child(labeled(
                        "Enterprise address",
                        text_box("sign-in-address", &self.address, None, window, cx),
                        cx,
                    ))
                    .when_some(address_error, |d, m| {
                        d.child(div().text_color(t.error).child(m))
                    })
                    .into_any_element(),
                vec![
                    DialogButton {
                        id: "sign-in-cancel",
                        label: "Cancel".into(),
                        primary: false,
                        disabled: false,
                        on_click: Box::new(close),
                    },
                    DialogButton {
                        id: "sign-in-continue",
                        label: "Continue".into(),
                        primary: true,
                        // GHD: an empty address, or a check in progress
                        disabled: address_loading
                            || self.address.read(cx).value().trim().is_empty(),
                        on_click: Box::new(move |window, cx| {
                            this.update(cx, |d, cx| d.continue_endpoint(window, cx))
                        }),
                    },
                ],
            ),
            Method::TokenEntry => (
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACING())
                    .child(
                        "Create a token with the repo, workflow, read:user and user:email scopes, \
                         then paste it here.",
                    )
                    .child(labeled(
                        "Personal access token",
                        password_text_box("sign-in-token", &self.token, window, cx),
                        cx,
                    ))
                    .when_some(sign_in_error, |d, m| {
                        d.child(div().text_color(t.error).child(m))
                    })
                    .when(self.enterprise, |d| {
                        let this = this.clone();
                        d.child(
                            div()
                                .id("sign-in-use-oauth-app-link")
                                .text_color(t.link)
                                .cursor_pointer()
                                .child(
                                    "Sign in with an OAuth app registered on this server instead",
                                )
                                .on_click(move |_, window, cx| {
                                    this.update(cx, |d, cx| d.show_oauth_app_entry(window, cx))
                                }),
                        )
                    })
                    .into_any_element(),
                vec![
                    DialogButton {
                        id: "sign-in-cancel",
                        label: "Cancel".into(),
                        primary: false,
                        disabled: false,
                        on_click: Box::new(close),
                    },
                    DialogButton {
                        id: "sign-in-submit",
                        label: "Sign in".into(),
                        primary: true,
                        disabled: false,
                        on_click: Box::new(move |_, cx| {
                            this.update(cx, |d, cx| d.submit_token(cx))
                        }),
                    },
                ],
            ),
            Method::OAuthAppEntry => (
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACING())
                    .child(format!(
                        "Register an OAuth app on {} (Settings › Developer settings › OAuth Apps) \
                         with the callback URL {} and Enable Device Flow ticked, then enter its \
                         client ID. The client secret is only needed for the browser flow.",
                        self.endpoint(cx).host(),
                        corvene_github::auth::SCHEME_REDIRECT_URI
                    ))
                    .child(labeled(
                        "Client ID",
                        text_box("sign-in-client-id", &self.client_id, None, window, cx),
                        cx,
                    ))
                    .child(labeled(
                        "Client secret",
                        password_text_box("sign-in-client-secret", &self.client_secret, window, cx),
                        cx,
                    ))
                    .into_any_element(),
                vec![
                    DialogButton {
                        id: "sign-in-cancel",
                        label: "Cancel".into(),
                        primary: false,
                        disabled: false,
                        on_click: Box::new(close),
                    },
                    DialogButton {
                        id: "sign-in-oauth-app-continue",
                        label: "Continue".into(),
                        primary: true,
                        disabled: self.client_id.read(cx).value().trim().is_empty(),
                        on_click: Box::new(move |_, cx| {
                            this.update(cx, |d, cx| d.submit_oauth_app(cx))
                        }),
                    },
                ],
            ),
            Method::Authentication => (
                self.authentication_body(cx),
                vec![DialogButton {
                    id: "sign-in-cancel",
                    label: "Cancel".into(),
                    primary: false,
                    disabled: false,
                    on_click: Box::new(close),
                }],
            ),
        };

        // GHD `renderExistingAccountWarningStep`: the warning above the
        // authentication form
        let body = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(SPACING())
            .when_some(warning.filter(|_| !at_endpoint_entry), |d, warning| {
                d.child(div().id("existing-account-warning").child(warning.text))
            })
            .child(body);

        if let Some(on_cancel) = self.embedded.clone() {
            return embedded_form(body.into_any_element(), buttons, on_cancel, cx);
        }
        dialog("sign-in", self.title(), body, buttons, close, window, cx).into_any_element()
    }
}

/// The form of an embedded [`SignInDialog`]: the body, then its buttons,
/// Cancel running `on_cancel`, Enter in a text box taking the primary one
/// (a `<form>`).
fn embedded_form(
    body: AnyElement,
    buttons: Vec<DialogButton>,
    on_cancel: SharedHandler,
    cx: &App,
) -> AnyElement {
    let mut submit: Option<SharedHandler> = None;
    let row: Vec<AnyElement> = buttons
        .into_iter()
        .map(|b| {
            let on_click: SharedHandler = if b.id == "sign-in-cancel" {
                on_cancel.clone()
            } else {
                std::rc::Rc::from(b.on_click)
            };
            if b.primary && !b.disabled {
                submit = Some(on_click.clone());
            }
            let el = if b.primary {
                primary_button(b.id, b.label, b.disabled, cx)
            } else {
                button(b.id, b.label, cx)
            };
            el.when(!b.disabled, |d| {
                d.on_click(move |_, window, cx| on_click(window, cx))
            })
            .into_any_element()
        })
        .collect();
    div()
        .id("sign-in-embedded")
        .w_full()
        .flex()
        .flex_col()
        .gap(SPACING_DOUBLE())
        .when_some(submit, |d, submit| {
            d.on_action(
                move |action: &gpui_kit::component::input::Enter, window, cx| {
                    if action.secondary || action.shift {
                        cx.propagate();
                        return;
                    }
                    submit(window, cx);
                },
            )
        })
        .child(body)
        .child(div().flex().flex_row().gap(SPACING()).children(row))
        .into_any_element()
}
