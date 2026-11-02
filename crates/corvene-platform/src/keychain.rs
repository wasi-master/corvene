//! Access tokens live in the OS keychain (macOS Keychain via `keyring`,
//! the secret service on Linux, the Android Keystore), one item per
//! (host, login), like GitHub Desktop's token store.

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
}

pub type Result<T> = std::result::Result<T, KeychainError>;

fn entry(host: &str, login: &str) -> Result<Entry> {
    ensure_store()?;
    // Account column shows as "login@host" in Keychain Access.
    Ok(Entry::new(SERVICE, &format!("{login}@{host}"))?)
}

pub fn store_token(host: &str, login: &str, token: &str) -> Result<()> {
    debug!(host, login, "storing token in keychain");
    entry(host, login)?.set_password(token)?;
    Ok(())
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
    map.insert(format!("{login}@{host}"), token.to_string());
}

pub fn token(host: &str, login: &str) -> Result<Option<String>> {
    if let Some(map) = INJECTED.get()
        && let Some(token) = map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&format!("{login}@{host}"))
    {
        return Ok(Some(token.clone()));
    }
    match entry(host, login)?.get_password() {
        Ok(token) => Ok(Some(token)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

/// Generic git server credentials (GHD `setGenericPassword`): one item per
/// (host, username), separate from the GitHub token entries.
fn generic_entry(host: &str, username: &str) -> Result<Entry> {
    ensure_store()?;
    Ok(Entry::new(SERVICE, &format!("git:{username}@{host}"))?)
}

pub fn store_generic_password(host: &str, username: &str, password: &str) -> Result<()> {
    debug!(host, username, "storing generic git password in keychain");
    generic_entry(host, username)?.set_password(password)?;
    Ok(())
}

pub fn generic_password(host: &str, username: &str) -> Result<Option<String>> {
    match generic_entry(host, username)?.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

pub fn delete_generic_password(host: &str, username: &str) -> Result<()> {
    match generic_entry(host, username)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// Corvene (`528-proxy-credentials`): a proxy's password, one item per
/// (proxy `host:port`, username).
fn proxy_entry(proxy: &str, username: &str) -> Result<Entry> {
    ensure_store()?;
    Ok(Entry::new(SERVICE, &format!("proxy:{username}@{proxy}"))?)
}

pub fn store_proxy_password(proxy: &str, username: &str, password: &str) -> Result<()> {
    debug!(proxy, username, "storing proxy password in keychain");
    proxy_entry(proxy, username)?.set_password(password)?;
    Ok(())
}

pub fn proxy_password(proxy: &str, username: &str) -> Result<Option<String>> {
    match proxy_entry(proxy, username)?.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

pub fn delete_proxy_password(proxy: &str, username: &str) -> Result<()> {
    match proxy_entry(proxy, username)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

pub fn delete_token(host: &str, login: &str) -> Result<()> {
    match entry(host, login)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// A GitHub Enterprise OAuth app's client secret (the browser flow's token
/// exchange), one item per (host, client ID).
fn oauth_secret_entry(host: &str, client_id: &str) -> Result<Entry> {
    ensure_store()?;
    Ok(Entry::new(SERVICE, &format!("oauth:{client_id}@{host}"))?)
}

pub fn store_oauth_client_secret(host: &str, client_id: &str, secret: &str) -> Result<()> {
    debug!(host, client_id, "storing OAuth client secret in keychain");
    oauth_secret_entry(host, client_id)?.set_password(secret)?;
    Ok(())
}

pub fn oauth_client_secret(host: &str, client_id: &str) -> Result<Option<String>> {
    match oauth_secret_entry(host, client_id)?.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

pub fn delete_oauth_client_secret(host: &str, client_id: &str) -> Result<()> {
    match oauth_secret_entry(host, client_id)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(err.into()),
    }
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
}
