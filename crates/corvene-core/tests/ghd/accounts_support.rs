//! Helpers shared by `corvene-core`'s ports of GitHub Desktop's account
//! tests (`unit/accounts-store-test.ts`, `unit/find-account-test.ts`,
//! `unit/stores/sign-in-store-test.ts`).
//!
//! GitHub Desktop's `AccountsStore` (`lib/stores/accounts-store.ts`) keeps
//! the accounts in an `IDataStore` (`localStorage`, item `users`) and their
//! tokens in an `ISecureStore` (the keychain); its tests use
//! `InMemoryStore` / `AsyncInMemoryStore` (`app/test/helpers/stores`).
//! Corvene keeps the accounts in `AppState::accounts`, persisted whole under
//! the key `accounts` of `corvene_store::Store` by `StoreExt::save_accounts`
//! and read back at launch by `StoreExt::accounts` (`Dispatcher::init`);
//! tokens go to a `corvene_core::accounts::SecureStore` (the OS keychain in
//! the app).
//!
//! - [`InMemoryStore`] is a `corvene_store::Store` in a temporary directory,
//!   with GitHub Desktop's `setItem` / `getItem` over JSON values;
//! - [`AsyncInMemoryStore`] is GitHub Desktop's secure store helper of the
//!   same name, a `SecureStore` in memory;
//! - [`AccountsStore::get_all`] is `StoreExt::accounts`, the read
//!   `Dispatcher::init` does (GitHub Desktop's `getAll` after
//!   `loadFromStore`);
//! - [`AccountsStore::add_account`] is `corvene_core::accounts::add_account`,
//!   which `Dispatcher::finish_sign_in` calls on `AppState::accounts`;
//! - [`AccountsStore`] is the sign-in store's `AccountsSource`
//!   (`new SignInStore(accountsStore)`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use corvene_core::Account;
use corvene_core::accounts::SecureStore;
use corvene_core::persistence::StoreExt;
use corvene_core::sign_in::AccountsSource;
use corvene_github::Endpoint;
use corvene_store::Store;
use tempfile::TempDir;

/// GitHub Desktop's `users` item of the data store is Corvene's `accounts`
/// key.
pub const USERS: &str = "accounts";

/// GitHub Desktop's `InMemoryStore` (`IDataStore`): a `corvene_store::Store`
/// in a temporary directory. Clones share the store.
#[derive(Clone)]
pub struct InMemoryStore {
    store: Arc<Store>,
    _dir: Arc<TempDir>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        let dir = corvene_test_support::create_temp_directory();
        let store = Store::open_in(dir.path()).expect("open the store");
        Self {
            store: Arc::new(store),
            _dir: Arc::new(dir),
        }
    }

    /// `setItem(key, JSON.stringify(value))`.
    pub fn set_item(&self, key: &str, value: &serde_json::Value) {
        self.store.set(key, value).expect("write the store");
    }

    /// `JSON.parse(getItem(key))`.
    pub fn get_item(&self, key: &str) -> serde_json::Value {
        self.store
            .get::<serde_json::Value>(key)
            .expect("read the store")
            .expect("the item is set")
    }
}

/// GitHub Desktop's `AsyncInMemoryStore` (`ISecureStore`): tokens by
/// (key, login) in memory.
#[derive(Default)]
pub struct AsyncInMemoryStore {
    store: RefCell<HashMap<(String, String), String>>,
}

impl SecureStore for AsyncInMemoryStore {
    fn set_item(&self, host: &str, login: &str, token: &str) -> Result<(), String> {
        self.store
            .borrow_mut()
            .insert((host.to_string(), login.to_string()), token.to_string());
        Ok(())
    }

    fn delete_item(&self, host: &str, login: &str) -> Result<(), String> {
        self.store
            .borrow_mut()
            .remove(&(host.to_string(), login.to_string()));
        Ok(())
    }
}

/// GitHub Desktop's `AccountsStore` over an `InMemoryStore` (see the module
/// docs).
pub struct AccountsStore {
    data_store: InMemoryStore,
    secure_store: AsyncInMemoryStore,
}

impl AccountsStore {
    /// `new AccountsStore(dataStore, new AsyncInMemoryStore())`.
    pub fn new(data_store: InMemoryStore) -> Self {
        Self {
            data_store,
            secure_store: AsyncInMemoryStore::default(),
        }
    }

    /// `getAll()`: the accounts as loaded from the data store.
    pub fn get_all(&self) -> Vec<Account> {
        self.data_store.store.accounts().expect("read the accounts")
    }

    /// `addAccount(account)`: `corvene_core::accounts::add_account` on the
    /// loaded accounts. The token went with [`new_account`] (Corvene's
    /// `Account` has none), so an empty one is stored.
    pub fn add_account(&self, account: Account) -> Option<Account> {
        let mut accounts = self.get_all();
        corvene_core::accounts::add_account(
            &self.data_store.store,
            &mut accounts,
            account,
            "",
            &self.secure_store,
            false,
        )
        .ok()
    }
}

impl AccountsSource for AccountsStore {
    fn accounts(&self) -> Vec<Account> {
        self.get_all()
    }
}

/// GitHub Desktop's `new Account(login, endpoint, token, emails, avatarURL,
/// id, name, plan)`. The token is dropped: Corvene keeps it in the keychain,
/// never in `Account`.
#[allow(clippy::too_many_arguments)]
pub fn new_account(
    login: &str,
    endpoint: &str,
    _token: &str,
    emails: &[&str],
    avatar_url: &str,
    id: u64,
    name: &str,
    plan: &str,
) -> Account {
    Account {
        endpoint: endpoint.to_string(),
        id,
        login: login.to_string(),
        name: Some(name.to_string()),
        avatar_url: Some(avatar_url.to_string()),
        emails: emails.iter().map(|e| e.to_string()).collect(),
        scopes: Vec::new(),
        plan: Some(plan.to_string()),
        private_primary_email: false,
        from_gh_cli: false,
    }
}

/// GitHub Desktop's `getDotComAPIEndpoint()` (`lib/api.ts`):
/// `Endpoint::github_com().api_base`.
pub fn get_dot_com_api_endpoint() -> String {
    Endpoint::github_com().api_base
}

/// GitHub Desktop's `getEnterpriseAPIURL(endpoint)` (`lib/api.ts`):
/// `Endpoint::enterprise(endpoint, false).api_base`.
pub fn get_enterprise_api_url(endpoint: &str) -> String {
    Endpoint::enterprise(endpoint, false)
        .expect("a valid Enterprise address")
        .api_base
}
