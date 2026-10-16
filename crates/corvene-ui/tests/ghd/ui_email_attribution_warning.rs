//! Port of GitHub Desktop's `app/test/unit/ui/email-attribution-warning-test.tsx`.
//!
//! GitHub Desktop's `GitEmailNotFoundWarning`
//! (`ui/lib/git-email-not-found-warning.tsx`) renders nothing without
//! accounts or with a blank email; otherwise ⚠️ or a green check, "This email
//! address does not match / matches your GitHub account. [Your commits will
//! be wrongly attributed.]", a "Learn more." link button when it does not
//! match, and the same sentence in an `AriaLiveContainer` for screen
//! readers. Whether it matches is `isAttributableEmailFor` over the accounts
//! (Corvene: `corvene_core::Account::is_attributable_email`).
//!
//! Corvene's counterpart is
//! `corvene_ui::git_email_not_found_warning::git_email_not_found_warning(
//! accounts, email)`, the content Welcome › Configure Git (`welcome.rs`,
//! the first account) and Settings › Git's "Other email" field
//! (`dialogs/preferences.rs`, every account) draw.

use corvene_core::Account;
use corvene_ui::git_email_not_found_warning::{Indicator, git_email_not_found_warning};

/// GitHub Desktop's `getDotComAPIEndpoint()`.
const DOTCOM_API_ENDPOINT: &str = "https://api.github.com";

/// GitHub Desktop's `createAccount(email)`: `new Account('mona',
/// getDotComAPIEndpoint(), '', [createEmail(email)], '', 1, 'Mona')`, the
/// email verified, primary and public.
fn create_account(email: &str) -> Account {
    Account {
        endpoint: DOTCOM_API_ENDPOINT.to_string(),
        id: 1,
        login: "mona".to_string(),
        name: Some("Mona".to_string()),
        avatar_url: Some(String::new()),
        emails: vec![email.to_string()],
        scopes: Vec::new(),
        plan: None,
        private_primary_email: false,
        from_gh_cli: false,
    }
}

// GHD: unit/ui/email-attribution-warning-test.tsx › GitEmailNotFoundWarning › renders nothing when there are no accounts or the email is blank
#[test]
fn renders_nothing_when_there_are_no_accounts_or_the_email_is_blank() {
    let no_accounts = git_email_not_found_warning(&[], "person@example.com");
    let blank_email = git_email_not_found_warning(&[create_account("mona@example.com")], " ");

    // `container.textContent === ''` and no `.git-email-not-found-warning`
    assert_eq!(no_accounts, None);
    assert_eq!(blank_email, None);
}

// GHD: unit/ui/email-attribution-warning-test.tsx › GitEmailNotFoundWarning › renders a mismatch warning, learn-more link, and screen-reader message
#[test]
fn renders_a_mismatch_warning_learn_more_link_and_screen_reader_message() {
    let warning =
        git_email_not_found_warning(&[create_account("mona@example.com")], "other@example.com")
            .expect("a warning");

    let link = warning.learn_more.as_ref().expect("a learn-more link");

    assert!(
        warning
            .text_content()
            .contains("does not match your GitHub account")
    );
    assert_eq!(link.aria_label, "Learn more about commit attribution");
    assert_eq!(
        link.uri,
        "https://docs.github.com/en/github/committing-changes-to-your-project/why-are-my-commits-linked-to-the-wrong-user"
    );
    // `srOnly.getAttribute('aria-live') === 'polite'`: the screen-reader
    // message is the polite live region's text
    assert!(
        warning
            .screen_reader_message
            .starts_with("This email address does not match your GitHub account.")
    );
}

// GHD: unit/ui/email-attribution-warning-test.tsx › GitEmailNotFoundWarning › renders a success indicator without the learn-more link when the email matches
#[test]
fn renders_a_success_indicator_without_the_learn_more_link_when_the_email_matches() {
    let warning =
        git_email_not_found_warning(&[create_account("mona@example.com")], "mona@example.com")
            .expect("a warning");

    assert_eq!(warning.indicator, Indicator::Check);
    assert!(
        warning
            .text_content()
            .contains("matches your GitHub account")
    );
    assert_eq!(warning.learn_more, None);
}
