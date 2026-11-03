//! Linux: what went wrong with the secret service, in words
//! (desktop#22762, "User interaction failed" on oo7).
//!
//! `keyring` talks to `org.freedesktop.secrets` (gnome-keyring, KWallet's
//! secret service, oo7). An item goes into the collection behind the
//! `default` alias; a locked collection is unlocked through the service,
//! which asks the user for its password with a prompt of its own (GNOME
//! Shell's or Plasma's dialog, oo7's command line prompter). When there is
//! no one to show that prompt, the service fails the request with a D-Bus
//! error such as oo7's "CLI prompter failed: …", which GHD 3.6.6 shows as
//! libsecret's "User interaction failed" (`app/src/lib/stores/accounts-store.ts`
//! logs "Error adding account"). And a prompt nobody ever sees (a service
//! that hands it to a prompter that never answers) blocks the request for
//! good: the `secret-service` crate waits for its `Completed` signal
//! without a timeout.
//!
//! Corvene runs each keyring call on a thread of its own and gives up after
//! [`timeout`] ([`bounded`]), and turns a failure into a sentence
//! ([`explain`]) after reading the default collection's label and lock
//! state from the service (a read that never prompts). Nothing is ever
//! written anywhere but the secret service: when it fails, signing in fails.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::Duration;

use secret_service::EncryptionType;
use secret_service::blocking::SecretService;

/// How long a keyring call may take, a password prompt included.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// How long reading the collection's state for [`explain`] may take.
const DIAGNOSIS_TIMEOUT: Duration = Duration::from_secs(5);

/// Calls that did not finish in time and are still blocked: the `keyring`
/// store serialises its calls, so the next ones would wait behind them.
static STUCK: AtomicUsize = AtomicUsize::new(0);

/// [`DEFAULT_TIMEOUT`], or `CORVENE_KEYRING_TIMEOUT_SECS` (the keyring
/// test scripts use a short one).
pub fn timeout() -> Duration {
    std::env::var("CORVENE_KEYRING_TIMEOUT_SECS")
        .ok()
        .and_then(|secs| secs.trim().parse().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_TIMEOUT)
}

/// The outcome of [`bounded`].
#[derive(Debug)]
pub enum Bounded<T> {
    Done(T),
    /// The call was still waiting after this long (a prompt nobody
    /// answers), or an earlier one still is.
    TimedOut(Duration),
}

/// Run `op` on a thread of its own and wait at most `limit` for it. A call
/// that is still blocked keeps its thread; while one is, later calls fail
/// at once instead of piling up behind it.
pub fn bounded<T: Send + 'static>(
    limit: Duration,
    op: impl FnOnce() -> T + Send + 'static,
) -> Bounded<T> {
    if STUCK.load(Ordering::SeqCst) > 0 {
        return Bounded::TimedOut(limit);
    }
    let (tx, rx) = mpsc::sync_channel(1);
    // set once the caller stopped waiting; the worker hands its result over
    // or counts itself out under this lock, so neither is missed
    let abandoned = Arc::new(Mutex::new(false));
    let worker_abandoned = abandoned.clone();
    let spawned = std::thread::Builder::new()
        .name("keyring".into())
        .spawn(move || {
            let result = op();
            let abandoned = worker_abandoned
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if *abandoned {
                STUCK.fetch_sub(1, Ordering::SeqCst);
            } else {
                let _ = tx.send(result);
            }
        });
    if spawned.is_err() {
        return Bounded::TimedOut(Duration::ZERO);
    }
    match rx.recv_timeout(limit) {
        Ok(result) => Bounded::Done(result),
        Err(_) => {
            let mut abandoned = abandoned.lock().unwrap_or_else(PoisonError::into_inner);
            // it may have finished between the timeout and the lock
            if let Ok(result) = rx.try_recv() {
                return Bounded::Done(result);
            }
            *abandoned = true;
            STUCK.fetch_add(1, Ordering::SeqCst);
            Bounded::TimedOut(limit)
        }
    }
}

/// The default collection as the service reports it, read without
/// prompting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Collection {
    /// No `org.freedesktop.secrets` on the session bus.
    NoService,
    /// Nothing behind the `default` alias.
    NoDefault,
    Found {
        label: String,
        locked: bool,
    },
    /// The service did not answer the question.
    Unknown,
}

/// The default collection's label and lock state.
pub fn default_collection() -> Collection {
    let read = || -> Result<Collection, secret_service::Error> {
        let ss = match SecretService::connect(EncryptionType::Plain) {
            Ok(ss) => ss,
            Err(secret_service::Error::Unavailable) => return Ok(Collection::NoService),
            Err(err) => return Err(err),
        };
        let collection = match ss.get_default_collection() {
            Ok(collection) => collection,
            Err(secret_service::Error::NoResult) => return Ok(Collection::NoDefault),
            Err(err) => return Err(err),
        };
        Ok(Collection::Found {
            label: collection.get_label().unwrap_or_default(),
            locked: collection.is_locked()?,
        })
    };
    match bounded(DIAGNOSIS_TIMEOUT, read) {
        Bounded::Done(Ok(state)) => state,
        Bounded::Done(Err(err)) => {
            tracing::debug!(%err, "could not read the default collection");
            Collection::Unknown
        }
        Bounded::TimedOut(_) => Collection::Unknown,
    }
}

/// What the service said, out of a `keyring` error.
fn cause(err: &keyring::Error) -> Option<String> {
    let platform = match err {
        keyring::Error::PlatformFailure(err) | keyring::Error::NoStorageAccess(err) => err,
        _ => return None,
    };
    let Some(ss) = platform.downcast_ref::<secret_service::Error>() else {
        return Some(platform.to_string());
    };
    Some(match ss {
        secret_service::Error::Prompt => "the password prompt was dismissed".to_string(),
        secret_service::Error::PromptDisconnected => {
            "the secret service went away while asking for the password".to_string()
        }
        secret_service::Error::Locked => "the secret service reports it as locked".to_string(),
        secret_service::Error::Zbus(zbus::Error::MethodError(_, Some(message), _)) => {
            // oo7 wraps its own reason in `org.freedesktop.DBus.Error.Failed: …`
            let message = message
                .strip_prefix("org.freedesktop.DBus.Error.Failed: ")
                .unwrap_or(message);
            format!("the secret service said: {message}")
        }
        other => other.to_string(),
    })
}

const NOT_SAVED: &str = "Corvene keeps tokens only in the keyring and never in a plain text file.";

/// A sentence for a keyring failure, given the default collection's state
/// afterwards ([`default_collection`]). `None` for errors that are not about
/// reaching the keyring (no such item, a bad value).
pub fn explain(err: &keyring::Error, collection: &Collection) -> Option<String> {
    match err {
        keyring::Error::PlatformFailure(_)
        | keyring::Error::NoStorageAccess(_)
        | keyring::Error::NoDefaultStore => {}
        _ => return None,
    }
    let cause = cause(err);
    let because = cause
        .as_deref()
        .map(|cause| format!(" ({cause})"))
        .unwrap_or_default();
    Some(match collection {
        Collection::NoService => format!(
            "Corvene could not reach a secret service on the session bus, so it has nowhere \
             safe to keep your sign-in. Start your desktop's keyring (GNOME Keyring, KWallet \
             or oo7) and try again. {NOT_SAVED}"
        ),
        Collection::NoDefault => format!(
            "Your keyring has no default collection, so there is nowhere to keep your \
             sign-in{because}. Create a keyring and make it the default one, for example in \
             Passwords and Keys, then try again. {NOT_SAVED}"
        ),
        Collection::Found {
            label,
            locked: true,
        } => format!(
            "Your keyring \"{label}\" is locked and could not be unlocked{because}. Unlock it, \
             for example by signing out and in again or in Passwords and Keys, then try again. \
             {NOT_SAVED}"
        ),
        Collection::Found { .. } | Collection::Unknown => {
            format!("The keyring did not accept the request{because}. {NOT_SAVED}")
        }
    })
}

/// The message for a call that did not finish within `limit`.
pub fn timed_out(limit: Duration) -> String {
    format!(
        "The keyring did not answer within {} seconds. It may be waiting for its password in \
         a prompt that is not showing. Unlock the keyring, for example in Passwords and Keys, \
         then try again. {NOT_SAVED}",
        limit.as_secs()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure(err: secret_service::Error) -> keyring::Error {
        keyring::Error::PlatformFailure(Box::new(err))
    }

    #[test]
    fn a_locked_collection_names_the_collection_and_the_cause() {
        let text = explain(
            &keyring::Error::NoStorageAccess(Box::new(secret_service::Error::Prompt)),
            &Collection::Found {
                label: "Login".into(),
                locked: true,
            },
        )
        .unwrap();
        assert!(
            text.starts_with("Your keyring \"Login\" is locked"),
            "{text}"
        );
        assert!(
            text.contains("(the password prompt was dismissed)"),
            "{text}"
        );
        assert!(text.contains("never in a plain text file"), "{text}");
    }

    #[test]
    fn a_missing_service_or_collection_says_so() {
        let text = explain(&keyring::Error::NoDefaultStore, &Collection::NoService).unwrap();
        assert!(text.contains("could not reach a secret service"), "{text}");
        let text = explain(
            &failure(secret_service::Error::NoResult),
            &Collection::NoDefault,
        )
        .unwrap();
        assert!(text.contains("no default collection"), "{text}");
    }

    #[test]
    fn errors_about_the_item_are_left_alone() {
        assert_eq!(
            explain(&keyring::Error::NoEntry, &Collection::Unknown),
            None
        );
    }

    #[test]
    fn a_call_that_blocks_times_out_and_later_calls_fail_fast() {
        let (release, wait) = mpsc::channel::<()>();
        let limit = Duration::from_millis(50);
        let first = bounded(limit, move || {
            let _ = wait.recv();
        });
        assert!(matches!(first, Bounded::TimedOut(_)));
        // the blocked call still holds the store: no new thread waits behind it
        assert!(matches!(bounded(limit, || 1), Bounded::TimedOut(_)));
        release.send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while STUCK.load(Ordering::SeqCst) > 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(matches!(bounded(limit, || 1), Bounded::Done(1)));
    }
}
