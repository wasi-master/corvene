//! Port of GitHub Desktop's
//! `app/test/unit/ui/welcome-and-sign-in-wrappers-test.tsx` (the
//! endpoint-entry case is skipped in `tools/ghd-tests/skips/ui2.tsv`).
//!
//! Corvene's counterparts are GPUI views dispatching to `corvene_core`;
//! each case reads what the GitHub Desktop component shows from a content
//! function, or from a stand-in where Corvene has none:
//!
//! - `SignIn` (`ui/lib/sign-in.tsx`) is `corvene_ui::dialogs::sign_in_content`,
//!   what the sign-in dialog (`crates/corvene-ui/src/dialogs/sign_in.rs`)
//!   shows for a sign-in store state (`corvene_core::sign_in::SignInState`,
//!   built with its constructors in place of GitHub Desktop's object
//!   literals): in the `ExistingAccountWarning` step "You're already signed
//!   in to <host> with the account <login>. If you continue, you will first
//!   be signed out." above the authentication form, whose "Sign in using
//!   your browser" button calls `requestBrowserAuthentication`
//!   (`Dispatcher::begin_sign_in`); in the `Success` step nothing.
//! - `SignInEnterprise` (`ui/welcome/sign-in-enterprise.tsx`): the Welcome
//!   step "Sign in to your GitHub Enterprise" around `SignIn`, nothing
//!   without a sign-in state, Cancel returning to the Start step. Corvene's
//!   counterpart is `corvene_ui::welcome::sign_in_enterprise_content`,
//!   which the Welcome flow's `SignInToEnterprise` step draws around the
//!   embedded sign-in form.
//! - `ConfigureGit` (`ui/welcome/configure-git.tsx`): "Configure Git", "This
//!   is used to identify the commits you create. …", a Finish button
//!   (`done`) and Cancel returning to Start without `done`. Corvene's
//!   counterpart is `corvene_ui::welcome::configure_git_content(accounts,
//!   global_user_name, global_user_email)`, which `Welcome::configure_git`
//!   draws (Finish: `Welcome::finish`, Cancel: `Welcome::back_to_start`).
//!
//! A click on a button is GitHub Desktop's callback prop; here the content
//! names the action the button runs, which is what `fireEvent.click` then
//! checks. GitHub Desktop's `WelcomeStep` is `corvene_ui::welcome::WelcomeStep`.
//! Not ported (React DOM only): the `children` buttons `SignIn` renders
//! unchanged.

use corvene_core::Account;
use corvene_core::sign_in::SignInState;
use corvene_ui::dialogs::{SignInAction, sign_in_content};
use corvene_ui::welcome::{
    WelcomeAction, WelcomeStep, configure_git_content as configure_git,
    sign_in_enterprise_content as sign_in_enterprise,
};

/// The action of the button labelled `label`, `None` when there is none.
fn action_of<A: Copy>(actions: &[(String, A)], label: &str) -> Option<A> {
    actions
        .iter()
        .find(|(button, _)| button == label)
        .map(|(_, action)| *action)
}

/// GitHub Desktop's `createExistingAccountWarningState()`: github.com with
/// the account `mona` (`new Account('mona', 'https://api.github.com',
/// 'token', [], '', 1, 'Mona Lisa')`; Corvene keeps the token in the
/// Keychain, not in `Account`).
fn create_existing_account_warning_state() -> SignInState {
    SignInState::existing_account_warning(
        "https://api.github.com",
        Account {
            endpoint: "https://api.github.com".to_string(),
            id: 1,
            login: "mona".to_string(),
            name: Some("Mona Lisa".to_string()),
            avatar_url: None,
            emails: Vec::new(),
            scopes: Vec::new(),
            plan: None,
            private_primary_email: false,
            from_gh_cli: false,
        },
    )
}

// GHD: unit/ui/welcome-and-sign-in-wrappers-test.tsx › welcome and sign-in wrappers › renders warning and browser-authentication states in the shared sign-in wrapper
#[test]
fn renders_warning_and_browser_authentication_states_in_the_shared_sign_in_wrapper() {
    let view = sign_in_content(&create_existing_account_warning_state())
        .expect("SignIn renders the warning step");

    let warning = view
        .existing_account_warning
        .expect("SignIn renders the existing-account warning");
    // `getByText(…, { exact: false })`
    assert!(warning.text.contains("You're already signed in to"));
    assert!(warning.host.contains("github.com"));
    // `getByText('mona')`
    assert_eq!(warning.login, "mona");

    // `getByRole('link', { name: 'Sign in using your browser' })`, clicked
    // once
    assert_eq!(
        action_of(&view.actions, "Sign in using your browser"),
        Some(SignInAction::RequestBrowserAuthentication)
    );

    // `view.rerender(…)` in the `Success` step: nothing at all
    assert!(sign_in_content(&SignInState::success()).is_none());
}

// GHD: unit/ui/welcome-and-sign-in-wrappers-test.tsx › welcome and sign-in wrappers › renders the enterprise welcome step only when sign-in state exists and its cancel button returns to start
#[test]
fn renders_the_enterprise_welcome_step_only_with_sign_in_state_and_cancel_returns_to_start() {
    // `signInState={null}`: nothing at all
    assert!(sign_in_enterprise(None).is_none());

    let state = SignInState::authentication("https://api.github.com");
    let view =
        sign_in_enterprise(Some(&state)).expect("SignInEnterprise renders with a sign-in state");

    assert_eq!(view.title, "Sign in to your GitHub Enterprise");
    // Cancel, clicked once
    assert_eq!(
        action_of(&view.buttons, "Cancel"),
        Some(WelcomeAction::Advance(WelcomeStep::Start))
    );
}

// GHD: unit/ui/welcome-and-sign-in-wrappers-test.tsx › welcome and sign-in wrappers › renders the configure-git welcome step and returns to start when cancelled
#[test]
fn renders_the_configure_git_welcome_step_and_returns_to_start_when_cancelled() {
    let view = configure_git(&[], None, None);

    assert_eq!(view.title, "Configure Git");
    // `getByText(…, { exact: false })`
    assert!(
        view.text
            .contains("This is used to identify the commits you create.")
    );
    // `getByRole('button', { name: 'Finish' })`
    assert!(action_of(&view.buttons, "Finish").is_some());

    // Cancel, clicked once: back to Start, `done` not called
    assert_eq!(
        action_of(&view.buttons, "Cancel"),
        Some(WelcomeAction::Advance(WelcomeStep::Start))
    );
}
