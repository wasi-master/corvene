//! Proxies: git's system proxy lookup (GHD's `resolveGitProxy` through
//! Electron, `corvene_git::proxy`) and, Corvene (`528-proxy-credentials`,
//! desktop#10026), proxies that answer 407 Proxy Authentication Required.
//!
//! GHD 3.6.6 has neither an Electron `login` handler
//! (`app/src/main-process/main.ts`) nor a git error for 407
//! (`app/src/lib/git/core.ts` `getDescriptionForError`): an authenticating
//! proxy shows git's "CONNECT tunnel failed, response 407" and makes API
//! requests fail, unless the proxy URL carries the password. Corvene asks for the proxy's username and password
//! ([`Popup::ProxyAuthentication`]) when a remote operation, a clone or one
//! of its own requests gets a 407, keeps the password in the keychain
//! (`keychain::store_proxy_password`) and the username in the store
//! (`proxy_logins`, by proxy `host:port`), and tries the operation again.
//! git gets the username in the proxy URL and asks the askpass helper for
//! the password; Corvene's requests send both (`corvene_platform::proxy`).
//! A prompt for a background operation or request shows once per proxy:
//! Cancel keeps it quiet until an operation the user started gets a 407.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use corvene_platform::proxy::AuthRequired;
use corvene_store::Store;
use tracing::{info, warn};

use crate::dispatcher::Dispatcher;
use crate::host::{AsyncCtx, Host};
use crate::persistence::StoreExt;
use crate::remote::spawn_bg;
use crate::state::{Popup, RetryAction};

/// What a saved proxy password lets Corvene try again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProxyRetry {
    /// A request of Corvene's own (it runs again on its own schedule).
    None,
    /// A remote operation on a repository.
    Remote { repo: u64, action: RetryAction },
    /// A clone into `path` (`clone_repository_with`'s options).
    Clone {
        url: String,
        path: PathBuf,
        default_branch: Option<String>,
        depth: Option<u32>,
    },
}

/// The proxy GitHub.com requests go through (Settings › Advanced).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProxyStatus {
    /// `host:port`, or none when they connect directly.
    pub proxy: Option<String>,
    /// Where it was found ("the system settings").
    pub source: String,
}

/// The proxy part of `AppState`.
#[derive(Clone, Debug, Default)]
pub struct ProxyState {
    /// Saved usernames per proxy `host:port` (passwords: the keychain).
    pub logins: HashMap<String, String>,
    /// Proxies whose prompt was cancelled: background 407s stay quiet.
    pub dismissed: HashSet<String>,
    /// Settings › Advanced's line, once looked up.
    pub status: Option<ProxyStatus>,
    /// Operations that got a 407 while the proxy's prompt was open: they
    /// run again after Save too (the open prompt is not rebuilt, which
    /// would lose what was typed).
    pub pending: HashMap<String, Vec<ProxyRetry>>,
    /// `530-front-door-credentials`: saved usernames per server
    /// `host:port` (passwords: the keychain), and the servers whose prompt
    /// was cancelled.
    pub front_doors: HashMap<String, String>,
    pub dismissed_front_doors: HashSet<String>,
}

impl ProxyState {
    pub fn load(store: &Store) -> Self {
        Self {
            logins: store.proxy_logins().unwrap_or_default(),
            front_doors: store.front_door_logins().unwrap_or_default(),
            ..Self::default()
        }
    }
}

static AUTH_REQUESTS: OnceLock<async_channel::Sender<AuthRequired>> = OnceLock::new();

/// `corvene_platform::proxy`'s handler: any thread, so through a channel.
fn on_auth_required(request: AuthRequired) {
    if let Some(tx) = AUTH_REQUESTS.get() {
        let _ = tx.try_send(request);
    }
}

impl Dispatcher {
    /// At launch: git's system proxy lookup (GHD's `resolveGitProxy`), the
    /// 407 reports of Corvene's own requests, and the saved credentials.
    pub fn start_proxy(cx: &mut dyn Host) {
        corvene_git::set_system_proxy_resolver(Some(corvene_platform::proxy::system_pac_string));
        let (tx, rx) = async_channel::unbounded::<AuthRequired>();
        if AUTH_REQUESTS.set(tx).is_ok() {
            corvene_platform::proxy::set_auth_required_handler(Some(on_auth_required));
            cx.spawn(async move |cx: &mut AsyncCtx| {
                while let Ok(request) = rx.recv().await {
                    cx.update(move |cx| {
                        Self::proxy_auth_required(
                            request.proxy,
                            request.rejected,
                            ProxyRetry::None,
                            false,
                            cx,
                        )
                    });
                }
            })
            .detach();
        }
        Self::sync_proxy_credentials(cx);
        Self::start_front_doors(cx);
    }

    /// `528-proxy-credentials` on: git gets the saved usernames, Corvene's
    /// requests the passwords (read from the keychain off the main thread;
    /// 407s are reported once they are in). Off: both forget them.
    pub fn sync_proxy_credentials(cx: &mut dyn Host) {
        let s = Self::state(cx).read(cx);
        if !s.flags.bool(crate::flags::ids::PROXY_CREDENTIALS) {
            corvene_git::set_proxy_logins(None);
            corvene_platform::proxy::load_credentials(None);
            return;
        }
        let logins = s.proxy.logins.clone();
        corvene_git::set_proxy_logins(Some(logins.clone()));
        spawn_bg(
            cx,
            move || {
                logins
                    .into_iter()
                    .filter_map(|(proxy, user)| {
                        match corvene_platform::keychain::proxy_password(&proxy, &user) {
                            Ok(Some(password)) => Some((proxy, (user, password))),
                            Ok(None) => None,
                            Err(err) => {
                                warn!(proxy, %err, "could not read the proxy password");
                                None
                            }
                        }
                    })
                    .collect::<HashMap<_, _>>()
            },
            |credentials, cx| {
                // the flag may have gone off meanwhile
                if Self::state(cx)
                    .read(cx)
                    .flags
                    .bool(crate::flags::ids::PROXY_CREDENTIALS)
                {
                    corvene_platform::proxy::load_credentials(Some(credentials));
                }
            },
        );
    }

    /// The proxy `proxy` (`host:port`) answered 407: ask for its username
    /// and password, unless that prompt is open already, or `user_started`
    /// is false and the user cancelled it before.
    pub fn proxy_auth_required(
        proxy: String,
        rejected: bool,
        retry: ProxyRetry,
        user_started: bool,
        cx: &mut dyn Host,
    ) {
        let s = Self::state(cx).read(cx);
        if !s.flags.bool(crate::flags::ids::PROXY_CREDENTIALS) {
            return;
        }
        let open = s.popups.all_popups().iter().any(|p| {
            matches!(&p.popup, Popup::ProxyAuthentication { proxy: open, server: false, .. } if *open == proxy)
        });
        if open {
            if retry != ProxyRetry::None {
                Self::state(cx).update(cx, |s, _| {
                    let pending = s.proxy.pending.entry(proxy).or_default();
                    if !pending.contains(&retry) {
                        pending.push(retry);
                    }
                });
            }
            return;
        }
        if !user_started && s.proxy.dismissed.contains(&proxy) {
            info!(proxy, "proxy prompt was cancelled before");
            return;
        }
        let username = s.proxy.logins.get(&proxy).cloned();
        Self::state(cx).update(cx, |s, _| {
            s.proxy.dismissed.remove(&proxy);
        });
        Self::show_popup(
            Popup::ProxyAuthentication {
                proxy,
                server: false,
                username,
                rejected,
                retry,
            },
            cx,
        );
    }

    /// git's 407 for `remote_url` (in `workdir`, none for a clone): the
    /// proxy git used is looked up off the main thread, then asked for.
    pub(crate) fn git_proxy_auth_required(
        git: Arc<corvene_git::GitBinary>,
        workdir: Option<PathBuf>,
        remote_url: String,
        retry: ProxyRetry,
        user_started: bool,
        cx: &mut dyn Host,
    ) {
        spawn_bg(
            cx,
            move || corvene_git::effective_proxy(git, workdir.as_deref(), &remote_url),
            move |proxy, cx| match proxy {
                Some(proxy) => {
                    let rejected = Self::state(cx)
                        .read(cx)
                        .proxy
                        .logins
                        .contains_key(&proxy.key);
                    Self::proxy_auth_required(proxy.key, rejected, retry, user_started, cx)
                }
                None if user_started => Self::show_error(
                    "Proxy authentication required",
                    "A proxy asked for a username and password, but Corvene could not tell \
                     which proxy Git used. Check Git's http.proxy setting and the proxy \
                     environment variables.",
                    cx,
                ),
                None => warn!("407 from an unknown proxy"),
            },
        );
    }

    /// `ProxyAuthentication` › Save: the keychain (off the main thread: a
    /// keychain may ask the user first), the store, then the operation
    /// again.
    pub fn save_proxy_credentials(
        proxy: String,
        username: String,
        password: String,
        retry: ProxyRetry,
        cx: &mut dyn Host,
    ) {
        let (previous, pending) = Self::state(cx).update(cx, |s, _| {
            (
                s.proxy.logins.get(&proxy).cloned(),
                s.proxy.pending.remove(&proxy).unwrap_or_default(),
            )
        });
        Self::close_popup_if(
            |p| matches!(p, Popup::ProxyAuthentication { server: false, .. }),
            cx,
        );
        let stored = (proxy.clone(), username.clone(), password.clone());
        spawn_bg(
            cx,
            move || {
                let (proxy, username, password) = stored;
                corvene_platform::keychain::store_proxy_password(&proxy, &username, &password)?;
                if let Some(previous) = previous.filter(|p| *p != username) {
                    let _ = corvene_platform::keychain::delete_proxy_password(&proxy, &previous);
                }
                Ok::<_, corvene_platform::keychain::KeychainError>(())
            },
            move |result, cx| {
                if let Err(err) = result {
                    Self::show_error("Could not save the proxy password", err.to_string(), cx);
                    return;
                }
                Self::state(cx).update(cx, |s, cx| {
                    s.proxy.logins.insert(proxy.clone(), username.clone());
                    s.proxy.dismissed.remove(&proxy);
                    if let Err(err) = s.store.save_proxy_logins(&s.proxy.logins) {
                        warn!(%err, "could not save the proxy logins");
                    }
                    corvene_git::set_proxy_logins(Some(s.proxy.logins.clone()));
                    cx.notify();
                });
                corvene_platform::proxy::set_credentials(&proxy, Some((username, password)));
                info!(proxy, "proxy credentials saved");
                for retry in std::iter::once(retry).chain(pending) {
                    match retry {
                        ProxyRetry::None => {}
                        ProxyRetry::Remote { repo, action } => {
                            Self::perform_retry(repo, action, cx)
                        }
                        ProxyRetry::Clone {
                            url,
                            path,
                            default_branch,
                            depth,
                        } => Self::clone_repository_with(url, path, default_branch, depth, cx),
                    }
                }
            },
        );
    }

    /// `ProxyAuthentication` › Cancel: no more prompts for this proxy from
    /// background work.
    pub fn dismiss_proxy_credentials(proxy: String, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, _| {
            s.proxy.pending.remove(&proxy);
            s.proxy.dismissed.insert(proxy);
        });
        Self::close_popup_if(
            |p| matches!(p, Popup::ProxyAuthentication { server: false, .. }),
            cx,
        );
    }

    /// Settings › Advanced › Forget: the saved password and username.
    pub fn forget_proxy_credentials(proxy: String, cx: &mut dyn Host) {
        let username = Self::state(cx).update(cx, |s, cx| {
            let username = s.proxy.logins.remove(&proxy);
            if let Err(err) = s.store.save_proxy_logins(&s.proxy.logins) {
                warn!(%err, "could not save the proxy logins");
            }
            corvene_git::set_proxy_logins(Some(s.proxy.logins.clone()));
            cx.notify();
            username
        });
        corvene_platform::proxy::set_credentials(&proxy, None);
        info!(proxy, "proxy credentials forgotten");
        if let Some(username) = username {
            spawn_bg(
                cx,
                move || {
                    if let Err(err) =
                        corvene_platform::keychain::delete_proxy_password(&proxy, &username)
                    {
                        warn!(proxy, %err, "could not delete the proxy password");
                    }
                },
                |_, _| {},
            );
        }
    }

    /// Settings › Advanced: look up the proxy GitHub.com requests use (a
    /// PAC script may take a moment, so off the main thread).
    pub fn refresh_proxy_status(cx: &mut dyn Host) {
        spawn_bg(
            cx,
            || {
                let choice = corvene_platform::proxy::proxy_for("https://api.github.com/");
                ProxyStatus {
                    proxy: choice.as_ref().map(|c| c.server.key()),
                    source: choice
                        .map(|c| c.source.describe().to_string())
                        .unwrap_or_default(),
                }
            },
            |status, cx| {
                Self::state(cx).update(cx, |s, cx| {
                    s.proxy.status = Some(status);
                    cx.notify();
                });
            },
        );
    }
}
