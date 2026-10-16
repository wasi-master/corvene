//! Corvene (`529-gh-cli-account`): with no GitHub.com account signed in,
//! borrow the GitHub CLI's. GitHub Desktop has no such fallback
//! (`app/src/lib/stores/accounts-store.ts` only knows its own sign-ins).
//!
//! The borrowed account sits in `AppState::accounts` like any other, marked
//! `Account::from_gh_cli`: its token is injected into this process
//! (`keychain::inject_token`) instead of the keychain, `save_accounts`
//! leaves it out, and askpass asks `gh` for the password git wants
//! (`CORVENE_ASKPASS_GH_CLI`). Signing in to GitHub.com replaces it;
//! signing out of it remembers the login so it is not borrowed again
//! until `gh` signs in to another account.

use tracing::{info, warn};

use crate::dispatcher::Dispatcher;
use crate::host::Host;
use crate::remote::spawn_bg;

const HOST: &str = "github.com";
/// The login last signed out of while borrowed from `gh`.
const SIGNED_OUT_KEY: &str = "gh_cli_signed_out";

impl Dispatcher {
    /// At launch and when the flag changes: borrow `gh`'s GitHub.com
    /// account when the flag is on and none is signed in; drop it when the
    /// flag is off.
    pub fn sync_gh_cli_account(cx: &mut dyn Host) {
        let (enabled, has_dotcom, signed_out) = {
            let s = Self::state(cx).read(cx);
            (
                s.flags.bool(crate::flags::ids::GH_CLI_ACCOUNT),
                s.accounts.iter().any(|a| a.is_dotcom()),
                s.store.get::<String>(SIGNED_OUT_KEY).ok().flatten(),
            )
        };
        if !enabled {
            Self::state(cx).update(cx, |s, cx| {
                let before = s.accounts.len();
                s.accounts.retain(|a| !a.from_gh_cli);
                if s.accounts.len() != before {
                    info!("stopped using the GitHub CLI's account");
                    cx.notify();
                }
            });
            return;
        }
        if has_dotcom {
            return;
        }
        spawn_bg(
            cx,
            move || {
                let token = corvene_platform::gh_cli::token(HOST, None)?;
                let endpoint = corvene_github::Endpoint::github_com();
                let client = corvene_github::Client::new(endpoint, token.clone());
                let scopes = client.token_scopes().ok().flatten().unwrap_or_default();
                match client.current_user(scopes) {
                    Ok(account) => Some((account, token)),
                    Err(err) => {
                        warn!(%err, "could not read the GitHub CLI's account");
                        None
                    }
                }
            },
            move |found, cx| {
                let Some((mut account, token)) = found else {
                    return;
                };
                if signed_out.is_some_and(|login| login.eq_ignore_ascii_case(&account.login)) {
                    return;
                }
                let adopted = Self::state(cx).update(cx, |s, cx| {
                    // signed in, or the flag went off, meanwhile
                    if !s.flags.bool(crate::flags::ids::GH_CLI_ACCOUNT)
                        || s.accounts.iter().any(|a| a.is_dotcom())
                    {
                        return false;
                    }
                    corvene_platform::keychain::inject_token(
                        &account.host(),
                        &account.login,
                        &token,
                    );
                    account.from_gh_cli = true;
                    info!(login = %account.login, "using the GitHub CLI's account");
                    s.accounts.push(account);
                    s.accounts = crate::accounts::sort_accounts(std::mem::take(&mut s.accounts));
                    for rs in s.repo_states.values_mut() {
                        rs.github_rematched = false;
                    }
                    cx.notify();
                    true
                });
                if adopted {
                    for id in Self::state(cx).read(cx).visible_repositories() {
                        Self::refresh_github_repository(id, cx);
                    }
                }
            },
        );
    }

    /// Signing out of a borrowed account: remember its login so the next
    /// launch does not borrow it again.
    pub(crate) fn remember_gh_cli_sign_out(account: &corvene_models::Account, cx: &mut dyn Host) {
        if !account.from_gh_cli {
            return;
        }
        let s = Self::state(cx).read(cx);
        if let Err(err) = s.store.set(SIGNED_OUT_KEY, &account.login) {
            warn!(%err, "could not save the GitHub CLI sign-out");
        }
    }
}
