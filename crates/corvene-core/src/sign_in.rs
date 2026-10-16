//! The sign-in state machine (GHD `app/src/lib/stores/sign-in-store.ts`
//! `SignInStore`), without gpui. `AppState::sign_in_store` drives the
//! sign-in dialog (`corvene_ui::dialogs::sign_in`); the `Dispatcher` moves
//! it along (`show_popup(Popup::SignIn { .. })` begins it, the address step
//! calls `set_sign_in_endpoint`, each authentication flow reports back).
//!
//! As in GitHub Desktop: GitHub.com starts at the Authentication step, an
//! Enterprise sign-in at EndpointEntry; a github.com or api.github.com
//! address typed there goes to the GitHub.com flow; an address that is not
//! a valid `https` URL stays at EndpointEntry with an error; signing in to
//! an endpoint that already has an account goes through
//! ExistingAccountWarning ("If you continue, you will first be signed
//! out"), and authenticating from there removes that account first. A
//! result callback hears `Success` and, on `reset`, `Cancelled`; listeners
//! hear every state, `None` after a reset.
//!
//! Corvene differences:
//!
//! - GHD's `authenticateWithBrowser` also opens the browser and waits for
//!   the OAuth callback (`resolveOAuthRequest`). Corvene's flows (device
//!   code, browser, personal access token; `Dispatcher::sign_in_*`,
//!   `web_flow.rs`) run outside the store: [`SignInStore::authenticate_with_browser`]
//!   only moves to Authentication (loading) and hands back the account to
//!   sign out, and the flows end in [`SignInStore::authentication_succeeded`]
//!   / [`SignInStore::authentication_failed`]. GHD's unused
//!   `TwoFactorAuthentication` step is left out.
//! - Flag `314-enterprise-plain-http` ([`SignInStore::allow_plain_http`]):
//!   an Enterprise address typed with `http://` keeps plain HTTP where
//!   GHD's `validateURL` refuses it ("Unsupported protocol").
//! - Errors are their messages (GHD keeps the `Error`).
//! - Flag `527-multiple-accounts` ([`SignInStore::allow_multiple`]): an
//!   endpoint that already has an account goes straight to
//!   Authentication; the new account is added beside it.

use std::cell::RefCell;
use std::rc::Rc;

use corvene_github::Endpoint;
use corvene_models::Account;
use tracing::{error, info, warn};

/// GHD `SignInStep`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignInStep {
    EndpointEntry,
    ExistingAccountWarning,
    Authentication,
    Success,
}

/// GHD `SignInState` (the union of `IEndpointEntryState`,
/// `IExistingAccountWarning`, `IAuthenticationState` and `ISuccessState`,
/// without the result callback, which [`SignInStore`] keeps).
#[derive(Clone, Debug, PartialEq)]
pub struct SignInState {
    pub kind: SignInStep,
    /// The API endpoint being signed in to (`https://api.github.com` for
    /// GitHub.com): set at ExistingAccountWarning and Authentication.
    pub endpoint: Option<String>,
    /// `existingAccount`: the account already signed in to `endpoint`
    /// (ExistingAccountWarning).
    pub existing_account: Option<Account>,
    /// Shown next to the step's inputs.
    pub error: Option<String>,
    /// A request is running: every input but Cancel is disabled.
    pub loading: bool,
}

impl SignInState {
    /// `{ kind: EndpointEntry, error: null, loading: false }`
    pub fn endpoint_entry() -> Self {
        Self {
            kind: SignInStep::EndpointEntry,
            endpoint: None,
            existing_account: None,
            error: None,
            loading: false,
        }
    }

    /// `{ kind: ExistingAccountWarning, endpoint, existingAccount, error:
    /// null, loading: false }`
    pub fn existing_account_warning(
        endpoint: impl Into<String>,
        existing_account: Account,
    ) -> Self {
        Self {
            kind: SignInStep::ExistingAccountWarning,
            endpoint: Some(endpoint.into()),
            existing_account: Some(existing_account),
            error: None,
            loading: false,
        }
    }

    /// `{ kind: Authentication, endpoint, error: null, loading: false }`
    pub fn authentication(endpoint: impl Into<String>) -> Self {
        Self {
            kind: SignInStep::Authentication,
            endpoint: Some(endpoint.into()),
            existing_account: None,
            error: None,
            loading: false,
        }
    }

    /// `{ kind: Success }`
    pub fn success() -> Self {
        Self {
            kind: SignInStep::Success,
            endpoint: None,
            existing_account: None,
            error: None,
            loading: false,
        }
    }
}

/// GHD `SignInResult`.
#[derive(Clone, Debug, PartialEq)]
pub enum SignInResult {
    Success { account: Account },
    Cancelled,
}

impl SignInResult {
    /// `result.kind`
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Success { .. } => "success",
            Self::Cancelled => "cancelled",
        }
    }
}

/// GHD's `resultCallback`.
pub type ResultCallback = Box<dyn FnMut(SignInResult)>;

/// Where the sign-in store finds the signed-in accounts (GHD's
/// `AccountsStore`, `getAll` / `onDidUpdate`): read whenever the store
/// looks for an existing account.
pub trait AccountsSource {
    fn accounts(&self) -> Vec<Account>;
}

/// The app's source: `AppState::accounts`, copied in by the dispatcher
/// before each sign-in step.
impl AccountsSource for RefCell<Vec<Account>> {
    fn accounts(&self) -> Vec<Account> {
        self.borrow().clone()
    }
}

/// GHD `/^(?:https:\/\/)?(?:api\.)?github\.com($|\/)/`: an address of
/// GitHub.com typed in the Enterprise flow.
fn is_github_com_address(url: &str) -> bool {
    let rest = url.strip_prefix("https://").unwrap_or(url);
    let rest = rest.strip_prefix("api.").unwrap_or(rest);
    rest.strip_prefix("github.com")
        .is_some_and(|tail| tail.is_empty() || tail.starts_with('/'))
}

/// GHD `SignInStore`.
pub struct SignInStore {
    state: Option<SignInState>,
    result_callback: Option<ResultCallback>,
    accounts_store: Rc<dyn AccountsSource>,
    listeners: Vec<Box<dyn FnMut(Option<SignInState>)>>,
    /// Flag `314-enterprise-plain-http` (see the module docs).
    pub allow_plain_http: bool,
    /// Flag `527-multiple-accounts`: no ExistingAccountWarning.
    pub allow_multiple: bool,
}

impl SignInStore {
    /// `new SignInStore(accountsStore)`
    pub fn new(accounts_store: Rc<dyn AccountsSource>) -> Self {
        Self {
            state: None,
            result_callback: None,
            accounts_store,
            listeners: Vec::new(),
            allow_plain_http: false,
            allow_multiple: false,
        }
    }

    /// `getState()`: `None` while no sign-in is in progress.
    pub fn get_state(&self) -> Option<&SignInState> {
        self.state.as_ref()
    }

    /// `onDidUpdate(fn)`
    pub fn on_did_update(&mut self, f: impl FnMut(Option<SignInState>) + 'static) {
        self.listeners.push(Box::new(f));
    }

    /// `setState(state)`: also tells the listeners.
    fn set_state(&mut self, state: Option<SignInState>) {
        self.state = state;
        for listener in &mut self.listeners {
            listener(self.state.clone());
        }
    }

    /// `reset()`: back to no sign-in; the result callback hears
    /// `Cancelled`.
    pub fn reset(&mut self) {
        let callback = self.result_callback.take();
        if self.state.is_some()
            && let Some(mut callback) = callback
        {
            callback(SignInResult::Cancelled);
        }
        self.set_state(None);
    }

    /// `beginDotComSignIn(resultCallback?)`
    pub fn begin_dot_com_sign_in(&mut self, result_callback: Option<ResultCallback>) {
        let endpoint = Endpoint::github_com().api_base;

        if self.state.is_some() {
            self.reset();
        }

        let existing_account = self
            .accounts_store
            .accounts()
            .into_iter()
            .find(Account::is_dotcom)
            .filter(|_| !self.allow_multiple);

        self.result_callback = result_callback;
        match existing_account {
            Some(existing_account) => self.set_state(Some(SignInState::existing_account_warning(
                endpoint,
                existing_account,
            ))),
            None => self.set_state(Some(SignInState::authentication(endpoint))),
        }
    }

    /// `beginEnterpriseSignIn(resultCallback?)`
    pub fn begin_enterprise_sign_in(&mut self, result_callback: Option<ResultCallback>) {
        if self.state.is_some() {
            self.reset();
        }

        self.result_callback = result_callback;
        self.set_state(Some(SignInState::endpoint_entry()));
    }

    /// `setEndpoint(url)`: from EndpointEntry (or ExistingAccountWarning)
    /// to Authentication or ExistingAccountWarning for the address' API
    /// endpoint; an invalid address stays with an error. Ignored (and
    /// logged, GHD's `fatalError`) at any other step.
    pub fn set_endpoint(&mut self, url: &str) {
        let Some(current_state) = self.state.clone().filter(|s| {
            matches!(
                s.kind,
                SignInStep::EndpointEntry | SignInStep::ExistingAccountWarning
            )
        }) else {
            error!(
                step = ?self.state.as_ref().map(|s| s.kind),
                "Sign in step not compatible with endpoint entry"
            );
            return;
        };

        // a github.com address in the Enterprise flow goes to GitHub.com's
        if is_github_com_address(url) {
            // `beginDotComSignIn(currentState.resultCallback)`: its reset
            // tells that same callback `Cancelled` before it is used again
            let mut callback = self.result_callback.take();
            if let Some(callback) = callback.as_mut() {
                callback(SignInResult::Cancelled);
            }
            self.set_state(None);
            self.begin_dot_com_sign_in(callback);
            return;
        }

        self.set_state(Some(SignInState {
            loading: true,
            ..current_state.clone()
        }));

        let endpoint = match Endpoint::validate_enterprise(url, self.allow_plain_http) {
            Ok(endpoint) => endpoint.api_base,
            Err(err) => {
                self.set_state(Some(SignInState {
                    loading: false,
                    error: Some(err.to_string()),
                    ..current_state
                }));
                return;
            }
        };

        let existing_account = self
            .accounts_store
            .accounts()
            .into_iter()
            .find(|a| a.endpoint == endpoint)
            .filter(|_| !self.allow_multiple);

        match existing_account {
            Some(existing_account) => self.set_state(Some(SignInState::existing_account_warning(
                endpoint,
                existing_account,
            ))),
            None => self.set_state(Some(SignInState::authentication(endpoint))),
        }
    }

    /// `authenticateWithBrowser()` up to the point where GHD opens the
    /// browser: from Authentication or ExistingAccountWarning to
    /// Authentication, loading. Returns the account the caller must sign
    /// out first (the warning's, when it is still signed in), or `Err` at
    /// any other step (GHD's `fatalError`).
    pub fn authenticate_with_browser(&mut self) -> Result<Option<Account>, SignInStepError> {
        let Some(current_state) = self.state.clone().filter(|s| {
            matches!(
                s.kind,
                SignInStep::Authentication | SignInStep::ExistingAccountWarning
            )
        }) else {
            return Err(SignInStepError(self.state.as_ref().map(|s| s.kind)));
        };

        self.set_state(Some(SignInState {
            loading: true,
            ..current_state.clone()
        }));

        let sign_out = current_state
            .existing_account
            .filter(|_| current_state.kind == SignInStep::ExistingAccountWarning)
            .filter(|existing| {
                // avoid removing an account that is already gone
                self.accounts_store
                    .accounts()
                    .iter()
                    .any(|a| a.endpoint == existing.endpoint)
            });

        info!("[SignInStore] initializing OAuth flow");
        self.set_state(Some(SignInState {
            kind: SignInStep::Authentication,
            endpoint: current_state.endpoint,
            existing_account: None,
            error: None,
            loading: true,
        }));
        Ok(sign_out)
    }

    /// The authentication flow signed `account` in (GHD's `onAuthCompleted`
    /// branch): the result callback hears `Success` and the store moves to
    /// Success. Ignored when the sign-in was given up meanwhile.
    pub fn authentication_succeeded(&mut self, account: Account) {
        if self.state.as_ref().map(|s| s.kind) != Some(SignInStep::Authentication) {
            warn!("[SignInStore] account resolved but session has changed");
            return;
        }
        info!("[SignInStore] account resolved");
        if let Some(callback) = self.result_callback.as_mut() {
            callback(SignInResult::Success { account });
        }
        self.set_state(Some(SignInState::success()));
    }

    /// The authentication flow failed (GHD's `onAuthError` branch): the
    /// error shows at the Authentication step, which stops loading.
    pub fn authentication_failed(&mut self, error: impl Into<String>) {
        let Some(state) = self
            .state
            .clone()
            .filter(|s| s.kind == SignInStep::Authentication)
        else {
            info!("[SignInStore] OAuth error but session has changed");
            return;
        };
        self.set_state(Some(SignInState {
            error: Some(error.into()),
            loading: false,
            ..state
        }));
    }

    /// Corvene: the authentication flow was stopped without a result (the
    /// dialog switched to another way of signing in); the Authentication
    /// step stops loading.
    pub fn authentication_stopped(&mut self) {
        if let Some(state) = self
            .state
            .clone()
            .filter(|s| s.kind == SignInStep::Authentication && s.loading)
        {
            self.set_state(Some(SignInState {
                loading: false,
                ..state
            }));
        }
    }
}

/// A sign-in step that does not allow what was asked (GHD's `fatalError`
/// "Sign in step '…' not compatible with …").
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("sign in step {0:?} not compatible with browser authentication")]
pub struct SignInStepError(pub Option<SignInStep>);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_github_com_addresses_like_ghd() {
        for url in [
            "github.com",
            "https://github.com",
            "https://api.github.com",
            "api.github.com/",
            "https://github.com/desktop",
        ] {
            assert!(is_github_com_address(url), "{url}");
        }
        for url in [
            "http://github.com",
            "https://github.company.com",
            "https://ghe.io",
            "https://example.com/github.com",
        ] {
            assert!(!is_github_com_address(url), "{url}");
        }
    }

    #[test]
    fn authenticating_from_the_warning_signs_the_existing_account_out() {
        let existing = Account {
            endpoint: Endpoint::github_com().api_base,
            id: 1,
            login: "mona".into(),
            name: None,
            avatar_url: None,
            emails: Vec::new(),
            scopes: Vec::new(),
            plan: None,
            private_primary_email: false,
            from_gh_cli: false,
        };
        let accounts = Rc::new(RefCell::new(vec![existing.clone()]));
        let mut store = SignInStore::new(accounts);
        store.begin_dot_com_sign_in(None);
        assert_eq!(
            store.get_state().map(|s| s.kind),
            Some(SignInStep::ExistingAccountWarning)
        );

        assert_eq!(store.authenticate_with_browser(), Ok(Some(existing)));
        let state = store.get_state().cloned();
        assert_eq!(
            state.as_ref().map(|s| s.kind),
            Some(SignInStep::Authentication)
        );
        assert!(state.is_some_and(|s| s.loading));
    }

    #[test]
    fn success_reaches_the_callback_before_the_success_step() {
        let results = Rc::new(RefCell::new(Vec::new()));
        let mut store = SignInStore::new(Rc::new(RefCell::new(Vec::new())));
        store.begin_dot_com_sign_in(Some(Box::new({
            let results = results.clone();
            move |result: SignInResult| results.borrow_mut().push(result.kind())
        })));
        assert_eq!(store.authenticate_with_browser(), Ok(None));
        store.authentication_succeeded(Account {
            endpoint: Endpoint::github_com().api_base,
            id: 2,
            login: "octocat".into(),
            name: None,
            avatar_url: None,
            emails: Vec::new(),
            scopes: Vec::new(),
            plan: None,
            private_primary_email: false,
            from_gh_cli: false,
        });
        assert_eq!(store.get_state().map(|s| s.kind), Some(SignInStep::Success));
        // GHD's dialog resets the store once it sees Success, and the
        // callback travels with every state, so it hears `Cancelled` too
        store.reset();
        assert_eq!(*results.borrow(), ["success", "cancelled"]);
    }
}
