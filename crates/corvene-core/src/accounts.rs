//! Signed-in accounts (GHD `app/src/lib/stores/accounts-store.ts`
//! `AccountsStore`), without gpui so the bookkeeping can be tested.
//!
//! GHD keeps the accounts in its `users` data store item and their tokens
//! in an `ISecureStore` (the keychain). In Corvene the list is
//! `AppState::accounts`, saved whole under the `accounts` key of
//! `corvene_store::Store` (`StoreExt::save_accounts`), and [`SecureStore`]
//! is the token store: [`Keychain`] in the app, anything else in tests.
//! Corvene's `Account` carries no token (it only ever goes to the secure
//! store), so [`add_account`] takes it beside the account.
//!
//! Corvene (flag `527-multiple-accounts`): GHD keeps one account per
//! endpoint (`accountsByEndpoint`). With the flag, [`add_account`] keeps
//! every login, so an endpoint can have several accounts, and
//! [`choose_repository_account`] picks the one a repository uses.

use corvene_models::{Account, RepositoryPermission};
use corvene_store::Store;
use tracing::error;

use crate::persistence::StoreExt;

/// GHD `ISecureStore`: where account tokens live, one item per account.
/// Corvene's items are keyed by the account's host and login (GHD's by
/// `getKeyForAccount(account)` and login).
pub trait SecureStore {
    /// `setItem(key, login, token)`
    fn set_item(&self, host: &str, login: &str, token: &str) -> Result<(), String>;
    /// `deleteItem(key, login)`
    fn delete_item(&self, host: &str, login: &str) -> Result<(), String>;
}

/// The OS keychain (`corvene_platform::keychain`).
pub struct Keychain;

impl SecureStore for Keychain {
    fn set_item(&self, host: &str, login: &str, token: &str) -> Result<(), String> {
        corvene_platform::keychain::store_token(host, login, token).map_err(|e| e.to_string())
    }

    fn delete_item(&self, host: &str, login: &str) -> Result<(), String> {
        corvene_platform::keychain::delete_token(host, login).map_err(|e| e.to_string())
    }
}

/// Why an account could not be added or removed (GHD `emitError`).
#[derive(Debug, thiserror::Error)]
pub enum AccountsError {
    #[error("could not store the account token: {0}")]
    StoreToken(String),
    #[error("could not remove the account token: {0}")]
    RemoveToken(String),
}

/// GHD `sortAccounts`: GitHub.com accounts first, then Enterprise ones,
/// each in the order they were added.
pub fn sort_accounts(accounts: Vec<Account>) -> Vec<Account> {
    let mut accounts = accounts;
    // a stable sort keeps the insertion order within each group
    accounts.sort_by_key(|account| !account.is_dotcom());
    accounts
}

/// GHD `AccountsStore.addAccount`: store the token, put `account` in place
/// of the account with the same endpoint (or after the others), sort
/// GitHub.com first and save the list. Nothing changes when the token
/// cannot be stored.
///
/// With `multiple` (flag `527-multiple-accounts`) only an account with the
/// same endpoint and login is replaced; other logins on the endpoint stay.
pub fn add_account(
    store: &Store,
    accounts: &mut Vec<Account>,
    account: Account,
    token: &str,
    secure_store: &dyn SecureStore,
    multiple: bool,
) -> Result<Account, AccountsError> {
    if let Err(err) = secure_store.set_item(&account.host(), &account.login, token) {
        error!(login = %account.login, %err, "Error adding account");
        return Err(AccountsError::StoreToken(err));
    }

    // `accountsByEndpoint`: one account per endpoint, the last one set
    // winning, in the place of the endpoint's first (a `Map` keeps the
    // first insertion's place)
    let same = |a: &Account, x: &Account| {
        a.endpoint == x.endpoint && (!multiple || a.login.eq_ignore_ascii_case(&x.login))
    };
    let mut by_endpoint: Vec<Account> = Vec::with_capacity(accounts.len() + 1);
    for x in std::mem::take(accounts)
        .into_iter()
        .chain([account.clone()])
    {
        match by_endpoint.iter_mut().find(|a| same(a, &x)) {
            Some(slot) => *slot = x,
            None => by_endpoint.push(x),
        }
    }
    *accounts = sort_accounts(by_endpoint);

    save(store, accounts);
    Ok(account)
}

/// GHD `AccountsStore.removeAccount`: delete the token, drop the account
/// (same endpoint and id) and save the list. Nothing changes when the token
/// cannot be deleted.
pub fn remove_account(
    store: &Store,
    accounts: &mut Vec<Account>,
    account: &Account,
    secure_store: &dyn SecureStore,
) -> Result<(), AccountsError> {
    if let Err(err) = secure_store.delete_item(&account.host(), &account.login) {
        error!(login = %account.login, %err, "Error removing account");
        return Err(AccountsError::RemoveToken(err));
    }
    accounts.retain(|a| !(a.endpoint == account.endpoint && a.id == account.id));
    save(store, accounts);
    Ok(())
}

/// What one signed-in account learned about a repository from
/// `GET /repos/{owner}/{name}` with its own token (flag
/// `527-multiple-accounts`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepositoryAccess {
    pub login: String,
    /// The API answered (a 404 is how GitHub hides a private repository).
    pub found: bool,
    pub private: bool,
    pub permission: Option<RepositoryPermission>,
    /// Organizations the account belongs to (`GET /user/orgs`), read only
    /// when the owner is not the account itself.
    pub orgs: Vec<String>,
}

impl RepositoryAccess {
    fn can_push(&self) -> bool {
        self.found
            && matches!(
                self.permission,
                Some(RepositoryPermission::Write | RepositoryPermission::Admin)
            )
    }

    fn owns(&self, owner: &str) -> bool {
        self.login.eq_ignore_ascii_case(owner)
            || self.orgs.iter().any(|org| org.eq_ignore_ascii_case(owner))
    }
}

/// The account [`choose_repository_account`] picked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountChoice {
    pub login: String,
    /// Several accounts have access, so the user is asked once.
    pub ask: bool,
    /// The logins with access, offered when asking (the pick first).
    pub candidates: Vec<String>,
}

/// Corvene (flag `527-multiple-accounts`): which of an endpoint's accounts
/// a repository owned by `owner` uses. The accounts that can push win;
/// without one, those that see it while it is private; among several the
/// owner (or a member of the owning organization) comes first, else the
/// first in `access` order, and several with access means asking. A public
/// repository nobody can push to goes to its owner's account, else the
/// first. `None` only when `access` is empty.
pub fn choose_repository_account(
    owner: &str,
    access: &[RepositoryAccess],
) -> Option<AccountChoice> {
    let pushers: Vec<&RepositoryAccess> = access.iter().filter(|a| a.can_push()).collect();
    let group = if pushers.is_empty() {
        access.iter().filter(|a| a.found && a.private).collect()
    } else {
        pushers
    };
    let (group, ask) = if group.is_empty() {
        (access.iter().collect::<Vec<_>>(), false)
    } else {
        let ask = group.len() > 1;
        (group, ask)
    };
    let pick = group
        .iter()
        .find(|a| a.login.eq_ignore_ascii_case(owner))
        .or_else(|| group.iter().find(|a| a.owns(owner)))
        .or_else(|| group.first())?;
    let mut candidates = vec![pick.login.clone()];
    if ask {
        candidates.extend(
            group
                .iter()
                .filter(|a| a.login != pick.login)
                .map(|a| a.login.clone()),
        );
    }
    Some(AccountChoice {
        login: pick.login.clone(),
        ask,
        candidates,
    })
}

/// The askpass `host=user` pairs (`CORVENE_ASKPASS_LOGINS`) of `pairs`
/// (host, user, login), one per host: the first account of each host, or
/// on `preferred`'s host the account with its login, the one the
/// repository at hand uses (`527-multiple-accounts`). The askpass helper
/// keeps the last pair of a host, so several would answer with the wrong
/// account.
pub fn askpass_pairs(
    pairs: impl IntoIterator<Item = (String, String, String)>,
    preferred: Option<(&str, &str)>,
) -> Vec<String> {
    let mut picked: Vec<(String, String, bool)> = Vec::new();
    for (host, user, login) in pairs {
        let wins = preferred
            .is_some_and(|(h, l)| h.eq_ignore_ascii_case(&host) && l.eq_ignore_ascii_case(&login));
        match picked
            .iter_mut()
            .find(|(h, _, _)| h.eq_ignore_ascii_case(&host))
        {
            Some(slot) if wins && !slot.2 => *slot = (host, user, true),
            Some(_) => {}
            None => picked.push((host, user, wins)),
        }
    }
    picked
        .into_iter()
        .map(|(host, user, _)| format!("{host}={user}"))
        .collect()
}

/// GHD `save`: the accounts, without tokens, to the data store.
fn save(store: &Store, accounts: &[Account]) {
    if let Err(err) = store.save_accounts(accounts) {
        error!(?err, "could not save accounts");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(login: &str, endpoint: &str, id: u64) -> Account {
        Account {
            endpoint: endpoint.to_string(),
            id,
            login: login.to_string(),
            name: None,
            avatar_url: None,
            emails: Vec::new(),
            scopes: Vec::new(),
            plan: None,
            private_primary_email: false,
            from_gh_cli: false,
        }
    }

    #[test]
    fn github_com_accounts_sort_first_in_insertion_order() {
        let sorted = sort_accounts(vec![
            account("a", "https://ghe.one/api/v3", 1),
            account("b", "https://api.github.com", 2),
            account("c", "https://ghe.two/api/v3", 3),
        ]);
        let logins: Vec<&str> = sorted.iter().map(|a| a.login.as_str()).collect();
        assert_eq!(logins, ["b", "a", "c"]);
    }

    struct NoSecrets;

    impl SecureStore for NoSecrets {
        fn set_item(&self, _: &str, _: &str, _: &str) -> Result<(), String> {
            Ok(())
        }

        fn delete_item(&self, _: &str, _: &str) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn adding_keeps_one_account_per_endpoint_in_its_first_place() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Store::open_in(dir.path()).expect("open the store");
        let mut accounts = vec![
            account("a", "https://ghe.one/api/v3", 1),
            account("b", "https://ghe.two/api/v3", 2),
            account("a2", "https://ghe.one/api/v3", 3),
        ];
        add_account(
            &store,
            &mut accounts,
            account("c", "https://ghe.one/api/v3", 4),
            "",
            &NoSecrets,
            false,
        )
        .expect("add the account");
        let logins: Vec<&str> = accounts.iter().map(|a| a.login.as_str()).collect();
        assert_eq!(logins, ["c", "b"]);
    }

    #[test]
    fn adding_with_multiple_accounts_keeps_other_logins() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Store::open_in(dir.path()).expect("open the store");
        let mut accounts = vec![
            account("work", "https://ghe.one/api/v3", 1),
            account("me", "https://api.github.com", 2),
        ];
        for (login, id) in [("work-me", 3), ("Me", 4)] {
            add_account(
                &store,
                &mut accounts,
                account(login, "https://api.github.com", id),
                "",
                &NoSecrets,
                true,
            )
            .expect("add the account");
        }
        let keys: Vec<(&str, u64)> = accounts.iter().map(|a| (a.login.as_str(), a.id)).collect();
        // the same login again replaces it in place
        assert_eq!(keys, [("Me", 4), ("work-me", 3), ("work", 1)]);
    }

    fn access(
        login: &str,
        found: bool,
        private: bool,
        perm: Option<RepositoryPermission>,
    ) -> RepositoryAccess {
        RepositoryAccess {
            login: login.into(),
            found,
            private,
            permission: perm,
            orgs: Vec::new(),
        }
    }

    #[test]
    fn askpass_names_one_account_per_host_preferring_the_repository_s() {
        let pairs = || {
            [
                ("github.com", "me"),
                ("ghe.one", "work"),
                ("github.com", "me-at-work"),
            ]
            .map(|(h, l)| (h.to_string(), l.to_string(), l.to_string()))
        };
        assert_eq!(
            askpass_pairs(pairs(), None),
            ["github.com=me", "ghe.one=work"]
        );
        assert_eq!(
            askpass_pairs(pairs(), Some(("GitHub.com", "Me-At-Work"))),
            ["github.com=me-at-work", "ghe.one=work"]
        );
        // a login of another host does not count
        assert_eq!(
            askpass_pairs(pairs(), Some(("ghe.one", "me-at-work"))),
            ["github.com=me", "ghe.one=work"]
        );
        let logins = crate::askpass::parse_logins(
            &askpass_pairs(pairs(), Some(("github.com", "me-at-work"))).join(";"),
        );
        assert_eq!(
            logins.get("github.com").map(String::as_str),
            Some("me-at-work")
        );
    }

    #[test]
    fn the_account_that_can_push_wins() {
        use RepositoryPermission::*;
        let choice = choose_repository_account(
            "acme",
            &[
                access("me", true, false, Some(Read)),
                access("me-at-work", true, false, Some(Write)),
            ],
        )
        .expect("a choice");
        assert_eq!(choice.login, "me-at-work");
        assert!(!choice.ask);
    }

    #[test]
    fn several_pushers_ask_with_the_owner_first() {
        use RepositoryPermission::*;
        let mut work = access("me-at-work", true, true, Some(Admin));
        work.orgs = vec!["Acme".into()];
        let choice =
            choose_repository_account("acme", &[access("me", true, true, Some(Write)), work])
                .expect("a choice");
        assert_eq!(choice.login, "me-at-work");
        assert!(choice.ask);
        assert_eq!(choice.candidates, ["me-at-work", "me"]);
    }

    #[test]
    fn private_repositories_seen_by_one_account_go_to_it() {
        use RepositoryPermission::*;
        let choice = choose_repository_account(
            "acme",
            &[
                access("me", false, false, None),
                access("me-at-work", true, true, Some(Read)),
            ],
        )
        .expect("a choice");
        assert_eq!(choice.login, "me-at-work");
        assert!(!choice.ask);
    }

    #[test]
    fn public_repositories_go_to_the_owner_else_the_first() {
        use RepositoryPermission::*;
        let both = [
            access("me", true, false, Some(Read)),
            access("octo", true, false, Some(Read)),
        ];
        let choice = choose_repository_account("OCTO", &both).expect("a choice");
        assert_eq!((choice.login.as_str(), choice.ask), ("octo", false));
        let choice = choose_repository_account("rust-lang", &both).expect("a choice");
        assert_eq!((choice.login.as_str(), choice.ask), ("me", false));
        assert_eq!(choose_repository_account("x", &[]), None);
    }
}
