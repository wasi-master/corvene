//! Access tokens live in the OS keychain (macOS Keychain via `keyring`,
//! the secret service on Linux, the Android Keystore), one item per
//! (host, login), like GitHub Desktop's token store.
//!
//! Linux (desktop#22762): every call runs through
//! [`crate::secret_service`], which gives up on a call that waits for a
//! password prompt nobody answers and turns a locked collection, a failed
//! or dismissed unlock prompt, a missing default collection or a missing
//! service into a sentence ([`KeychainError::SecretService`]). GHD shows
//! libsecret's "User interaction failed" there. Nothing falls back to a
//! file: when the keyring cannot be used, the operation fails.

use tracing::debug;

/// The `keyring` crate's entry type. Android: `keyring`'s own API has no
/// store there; its Android store (shared preferences encrypted with a key
/// in the Android Keystore) plugs into `keyring-core`, whose `Entry` and
/// `Error` are the same types.
#[cfg(not(target_os = "android"))]
use keyring::Entry;
#[cfg(target_os = "android")]
use keyring_core::Entry;

const SERVICE: &str = crate::BUNDLE_ID;

/// Makes the Android Keystore store `keyring-core`'s default, once. The
/// store finds the Java VM through `ndk-context`, which android-activity
/// sets up before `android_main` runs.
#[cfg(target_os = "android")]
fn ensure_store() -> keyring::Result<()> {
    use std::sync::OnceLock;
    static READY: OnceLock<std::result::Result<(), String>> = OnceLock::new();
    READY
        .get_or_init(|| {
            android_native_keyring_store::Store::new()
                .map(|store| keyring_core::set_default_store(store))
                .map_err(|err| err.to_string())
        })
        .clone()
        .map_err(|err| keyring::Error::NoStorageAccess(err.into()))
}

#[cfg(not(target_os = "android"))]
fn ensure_store() -> keyring::Result<()> {
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum KeychainError {
    #[error("keychain error: {0}")]
    Keyring(#[from] keyring::Error),
    /// Linux: the secret service could not be used, in words
    /// ([`crate::secret_service::explain`]): a locked collection whose
    /// unlock prompt failed or was dismissed, no default collection, no
    /// service, or a call that never came back.
    #[error("{0}")]
    SecretService(String),
}

pub type Result<T> = std::result::Result<T, KeychainError>;

/// One `keyring` call on the entry for `account` (`login@host`, …).
/// Linux: on a thread of its own, given up after
/// [`crate::secret_service::timeout`] (a password prompt nobody sees would
/// block it for good), and a failure to reach the keyring explained
/// (desktop#22762).
fn call<T: Send + 'static>(
    account: String,
    op: impl FnOnce(&Entry) -> keyring::Result<T> + Send + 'static,
) -> Result<T> {
    let run = move || -> keyring::Result<T> {
        ensure_store()?;
        op(&Entry::new(SERVICE, &account)?)
    };
    #[cfg(all(unix, not(any(target_os = "macos", target_os = "android"))))]
    {
        use crate::secret_service::{self as ss, Bounded};
        match ss::bounded(ss::timeout(), run) {
            Bounded::Done(Ok(value)) => Ok(value),
            Bounded::Done(Err(err)) => {
                let collection = match &err {
                    keyring::Error::NoEntry => ss::Collection::Unknown,
                    _ => ss::default_collection(),
                };
                match ss::explain(&err, &collection) {
                    Some(text) => {
                        tracing::warn!(%err, ?collection, "keyring failed");
                        Err(KeychainError::SecretService(text))
                    }
                    None => Err(err.into()),
                }
            }
            Bounded::TimedOut(limit) => {
                tracing::warn!(?limit, "keyring call timed out");
                Err(KeychainError::SecretService(ss::timed_out(limit)))
            }
        }
    }
    #[cfg(not(all(unix, not(any(target_os = "macos", target_os = "android")))))]
    {
        Ok(run()?)
    }
}

fn get(account: String) -> Result<Option<String>> {
    match call(account, |entry| entry.get_password()) {
        Ok(secret) => Ok(Some(secret)),
        Err(KeychainError::Keyring(keyring::Error::NoEntry)) => Ok(None),
        Err(err) => Err(err),
    }
}

fn set(account: String, secret: &str) -> Result<()> {
    let secret = secret.to_string();
    call(account, move |entry| entry.set_password(&secret))
}

fn delete(account: String) -> Result<()> {
    match call(account, |entry| entry.delete_credential()) {
        Ok(()) | Err(KeychainError::Keyring(keyring::Error::NoEntry)) => Ok(()),
        Err(err) => Err(err),
    }
}

/// Account column shows as "login@host" in Keychain Access.
fn token_account(host: &str, login: &str) -> String {
    format!("{login}@{host}")
}

pub fn store_token(host: &str, login: &str, token: &str) -> Result<()> {
    debug!(host, login, "storing token in keychain");
    set(token_account(host, login), token)
}

/// Tokens handed in by [`inject_token`], consulted before the keychain.
static INJECTED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> =
    std::sync::OnceLock::new();

/// A token for `(host, login)` that lives in this process only (the parity
/// harness's stub GitHub API, tests): [`token`] answers with it and the
/// keychain is never touched for that account.
pub fn inject_token(host: &str, login: &str, token: &str) {
    let mut map = INJECTED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    map.insert(token_account(host, login), token.to_string());
}

pub fn token(host: &str, login: &str) -> Result<Option<String>> {
    if let Some(map) = INJECTED.get()
        && let Some(token) = map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&token_account(host, login))
    {
        return Ok(Some(token.clone()));
    }
    get(token_account(host, login))
}

/// Generic git server credentials (GHD `setGenericPassword`): one item per
/// (host, username), separate from the GitHub token entries.
fn generic_account(host: &str, username: &str) -> String {
    format!("git:{username}@{host}")
}

pub fn store_generic_password(host: &str, username: &str, password: &str) -> Result<()> {
    debug!(host, username, "storing generic git password in keychain");
    set(generic_account(host, username), password)
}

pub fn generic_password(host: &str, username: &str) -> Result<Option<String>> {
    get(generic_account(host, username))
}

pub fn delete_generic_password(host: &str, username: &str) -> Result<()> {
    delete(generic_account(host, username))
}

/// Corvene (`528-proxy-credentials`): a proxy's password, one item per
/// (proxy `host:port`, username).
fn proxy_account(proxy: &str, username: &str) -> String {
    format!("proxy:{username}@{proxy}")
}

pub fn store_proxy_password(proxy: &str, username: &str, password: &str) -> Result<()> {
    debug!(proxy, username, "storing proxy password in keychain");
    set(proxy_account(proxy, username), password)
}

pub fn proxy_password(proxy: &str, username: &str) -> Result<Option<String>> {
    get(proxy_account(proxy, username))
}

pub fn delete_proxy_password(proxy: &str, username: &str) -> Result<()> {
    delete(proxy_account(proxy, username))
}

/// Corvene (`530-front-door-credentials`): the password of a proxy in front
/// of a server, one item per (server `host:port`, username).
fn front_door_account(server: &str, username: &str) -> String {
    format!("server:{username}@{server}")
}

pub fn store_front_door_password(server: &str, username: &str, password: &str) -> Result<()> {
    debug!(server, username, "storing front door password in keychain");
    set(front_door_account(server, username), password)
}

pub fn front_door_password(server: &str, username: &str) -> Result<Option<String>> {
    get(front_door_account(server, username))
}

pub fn delete_front_door_password(server: &str, username: &str) -> Result<()> {
    delete(front_door_account(server, username))
}

pub fn delete_token(host: &str, login: &str) -> Result<()> {
    delete(token_account(host, login))
}

/// A GitHub Enterprise OAuth app's client secret (the browser flow's token
/// exchange), one item per (host, client ID).
fn oauth_secret_account(host: &str, client_id: &str) -> String {
    format!("oauth:{client_id}@{host}")
}

pub fn store_oauth_client_secret(host: &str, client_id: &str, secret: &str) -> Result<()> {
    debug!(host, client_id, "storing OAuth client secret in keychain");
    set(oauth_secret_account(host, client_id), secret)
}

pub fn oauth_client_secret(host: &str, client_id: &str) -> Result<Option<String>> {
    get(oauth_secret_account(host, client_id))
}

pub fn delete_oauth_client_secret(host: &str, client_id: &str) -> Result<()> {
    delete(oauth_secret_account(host, client_id))
}

#[cfg(test)]
mod tests {
    /// A real round trip through the OS store. Needs an unlocked keyring:
    /// on Linux, a secret-service daemon on the session bus
    /// (`packaging/linux/keyring-test.sh` runs it under gnome-keyring), so
    /// it only runs with `CORVENE_KEYRING_TEST=1`.
    #[test]
    fn tokens_round_trip_through_the_os_store() {
        if std::env::var_os("CORVENE_KEYRING_TEST").is_none() {
            return;
        }
        let host = format!("corvene-test-{}.invalid", std::process::id());
        super::store_token(&host, "octocat", "gho_secret").unwrap();
        assert_eq!(
            super::token(&host, "octocat").unwrap().as_deref(),
            Some("gho_secret")
        );
        // CORVENE_KEYRING_KEEP: leave it for a look at the keyring files
        if std::env::var_os("CORVENE_KEYRING_KEEP").is_none() {
            super::delete_token(&host, "octocat").unwrap();
            assert_eq!(super::token(&host, "octocat").unwrap(), None);
        }
    }

    /// A keyring that cannot be used fails with a sentence that says why
    /// (`packaging/linux/keyring-test-oo7.sh` sets
    /// `CORVENE_KEYRING_EXPECT_ERROR` to a part of it).
    #[test]
    fn a_keyring_that_cannot_be_used_is_explained() {
        let Some(expected) = std::env::var_os("CORVENE_KEYRING_EXPECT_ERROR") else {
            return;
        };
        let expected = expected.to_string_lossy().into_owned();
        let host = format!("corvene-test-{}.invalid", std::process::id());
        let err = super::store_token(&host, "octocat", "gho_secret").unwrap_err();
        let text = err.to_string();
        eprintln!("keyring error: {text}");
        assert!(
            text.contains(&expected),
            "{text:?} does not say {expected:?}"
        );
        assert!(text.contains("never in a plain text file"), "{text}");
    }
}
